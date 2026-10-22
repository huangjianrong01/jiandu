/*!
 * 笺读 JianDu · EPUB 垂直化清洗与结构化预处理器（Data Pipeline）
 * v1.24.0 · 零依赖 · 浏览器 / Node 通用
 * ---------------------------------------------------------------------------
 * 一句话：cleanEpub(xhtml) → [{ title, content }]，出去的是能直接喂给竖排
 * 排版引擎的干净结构。content 里只允许出现三种标记：
 *
 *   [注]…[/注]        双行夹注（竖排小字，占半格宽）
 *   [高亮]…[/高亮]    着重（刻本里的朱砂圈点，不是西式加粗）
 *   [拼音:…]          格内注音（逐字母竖叠在字格右上角）
 *
 * 为什么要这一层：EPUB 是**横排电子书**的产物，直接扔进竖排引擎有三处崩坏——
 *
 *   ┌──────────────┬────────────────────┬──────────────────────────────┐
 *   │ 病           │ 横排世界           │ 竖排世界                     │
 *   ├──────────────┼────────────────────┼──────────────────────────────┤
 *   │ 14428        │ 一行读完           │ 拆成 1/4/4/2/8 五个字格摞下  │
 *   │ 《书名》     │ 尖括号包书名       │ 竖排书名号是 ︽ ︾            │
 *   │ <sup>1</sup> │ 上标小号，无伤大雅 │ 一行插进两个小字，行气全断   │
 *   └──────────────┴────────────────────┴──────────────────────────────┘
 *
 * 五个阶段（顺序不可换，每一步的「为什么」都写在函数头注释里）：
 *
 *   ① denoiseDOM        结构降噪：剔噪块 / 广告 / 颜色
 *   ② extractRuby + 注   标记提取：<ruby><rt> → [拼音:…]；noteref → [注]…[/注]
 *   ③ classicizePunct   标点古籍化：《》→︽︾、（）→[注]、……→︙、——→︱
 *   ④ cleanNumbers      数字清洗：数值读 / 逐字读 / [数字:…] 整串兜底
 *   ⑤ collectChapters   段落重构：丢页码孤行、压空白、{ title, content }
 *
 * 三个「重点实现」在文件后半段的独立小节里（可直接抄走）：
 *   · 连续数字转汉字 —— numToCn / digitsToCn / cleanNumbers
 *   · 括号转夹注     —— parensToNote
 *   · ruby 提拼音    —— extractRuby（DOMParser 版）/ rubyToPinyin（正则兜底版）
 */

(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  if (root) root.JianDuEpubClean = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, function () {
  'use strict';

  // =========================================================================
  // 0. 基础工具
  // =========================================================================

  const DIGIT_CN = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

  /** 半角 / 全角数字 → 0..9；其它返回 -1。 */
  function digitOf(c) {
    if (c >= '0' && c <= '9') return c.charCodeAt(0) - 48;
    if (c >= '０' && c <= '９') return c.charCodeAt(0) - 65296;
    return -1;
  }
  const isDigit = (c) => !!c && digitOf(c) >= 0;

  /** 零宽 / 不可见字符（EPUB 常在数字间塞 `&#8203;`，不能算断开）。 */
  const TRANSPARENT = /[\u200B\u200C\u200D\uFEFF\u00AD\u2060]/;

  /** 句读 / 括号 / 引号一类「非文字」记号（判乱码行用）。 */
  const PUNCT = /[，。、；：？！…—―·"'“”‘’「」『』﹁﹂﹃﹄（）()［］\[\]【】《》〈〉︽︾︿﹀︙︱·]/;

  /** 一个数字串最多允许转汉字：更长的（ISBN / 时间戳）只包标记、不转。 */
  const MAX_DIGITS = 6;

  /** CJK 判定：竖排里「词间空格」是噪声，两侧挨着汉字就把空格整个丢掉。 */
  const isCJK = (c) =>
    !!c && /[\u2E80-\u2FFF\u3040-\u30FF\u3400-\u4DBF\u4E00-\u9FFF\uF900-\uFAFF\uAC00-\uD7AF]/.test(c);

  /**
   * 连续空白 / 软换行 / 零宽字符压成一个。
   *
   * 两条判据分得很清：
   *   · **半角空白**（空格 / Tab / 软换行）是横排排版的产物，两侧挨着汉字就
   *     整个丢掉（竖排里没有「词间空格」，留着就是一个空字格）；
   *   · **全角空格 `　`（U+3000）**是作者有意留的间隔（`第一章　道可道`），
   *     原样保住一个——当噪声删掉，题名就跟章次黏成一串了。
   */
  function squeezeSpaces(s) {
    let out = '';
    for (let i = 0; i < s.length; i++) {
      if (/\s/.test(s[i]) || TRANSPARENT.test(s[i])) {
        let ideographic = false;
        do {
          if (s[i] === '\u3000') ideographic = true;
          i++;
        } while (i < s.length && (/\s/.test(s[i]) || TRANSPARENT.test(s[i])));
        i--;
        const prev = out.slice(-1);
        const next = s[i + 1];
        if (prev && next) {
          if (ideographic) out += '\u3000';
          else if (!isCJK(prev) && !isCJK(next) && !out.endsWith(' ')) out += ' ';
        }
        continue;
      }
      out += s[i];
    }
    return out;
  }

  // =========================================================================
  // 1. 连续数字转汉字
  // =========================================================================
  //
  // 竖排的致命伤：引擎按「字」排，`14428` 会被当成五个独立西文字符，一格一个
  // 竖着摞成 1/4/4/2/8，读起来断成五截。根治要**在导入期**就把数字写成汉字，
  // 从源头断绝被拆散的可能；实在转不动的（ISBN、超长流水号）包成整串标记，
  // 让引擎「整串横躺」——无论如何都不许拆单字。

  /** 四位数一节的读法（0..9999）。`lowest` = 最低节（决定 10~19 念「十」还是「一十」）。 */
  function fourDigits(n, lowest) {
    const th = Math.floor(n / 1000);
    const h = Math.floor((n % 1000) / 100);
    const t = Math.floor((n % 100) / 10);
    const o = n % 10;
    let s = '';
    let zero = false;
    if (th > 0) {
      s += DIGIT_CN[th] + '千';
    }
    if (h > 0) {
      s += DIGIT_CN[h] + '百';
    } else if (th > 0 && (t > 0 || o > 0)) {
      zero = true;
    }
    if (t > 0) {
      if (zero) {
        s += '零';
        zero = false;
      }
      // 最低节的 10~19 念「十 / 十五」，不念「一十」。
      if (!(t === 1 && s === '' && lowest)) s += DIGIT_CN[t];
      s += '十';
    } else if ((th > 0 || h > 0) && o > 0) {
      zero = true;
    }
    if (o > 0) {
      if (zero) s += '零';
      s += DIGIT_CN[o];
    }
    return s;
  }

  /** 数值读法：15→十五、223→二百二十三、1980→一千九百八十、27637→二万七千六百三十七。 */
  function numToCn(n) {
    if (n === 0) return '零';
    const YI = 100000000;
    const WAN = 10000;
    const yi = Math.floor(n / YI);
    const rest = n % YI;
    const wan = Math.floor(rest / WAN);
    const low = rest % WAN;
    let out = '';
    if (yi > 0) out += fourDigits(yi, false) + '亿';
    if (wan > 0) {
      if (yi > 0 && wan < 1000) out += '零';
      out += fourDigits(wan, false) + '万';
    }
    if (low > 0) {
      if ((yi > 0 || wan > 0) && low < 1000) out += '零';
      out += fourDigits(low, true);
    }
    return out || '零';
  }

  /** 逐字读：223→二二三、1980→一九八〇（零位读「〇」，合刻本纪年习惯）。 */
  function digitsToCn(s) {
    let out = '';
    for (const c of s) {
      const d = digitOf(c);
      if (d >= 0) out += DIGIT_CN[d];
    }
    return out;
  }

  /** 后接「年」且不超过四位 → 纪年（逐字读：「二二三年」）。 */
  function isYearContext(tail, digits) {
    if (!digits || digits > 4) return false;
    return /^[年載载]/.test(tail || '');
  }

  /** 该不该「放过不转」（编号 / 与字母粘连 / 超长）——放过就包 `[数字:…]`。 */
  function shouldSkipNumber(prev, next, digits) {
    if (!digits || digits > MAX_DIGITS) return true;
    const alpha = (c) => !!c && /[A-Za-z]/.test(c);
    if (alpha(prev) || alpha(next)) return true;
    if (prev && '#/\\@_+*='.includes(prev)) return true;
    return false;
  }

  /**
   * 数字串 → 汉字（含小数点）。
   * 小数点一律逐字读（三点一四）；超过四位、纪年也逐字读（与刻本叶码一致）；
   * 其余走数值读法（十五、二百二十三）。
   */
  function runToCn(raw, tail) {
    // 先把数字串剥成纯 ASCII 数字（全角 `１９８０` 也归一到 `1980`），
    // 才谈得上「念」——直接拿汉字去 Number() 会得到 0。
    let ascii = '';
    for (const c of raw) {
      const d = digitOf(c);
      if (d >= 0) ascii += String(d);
    }
    const count = ascii.length;
    const hasPoint = /[.．·]/.test(raw);
    if (hasPoint) {
      let out = '';
      let dotDone = false;
      for (const c of raw) {
        const d = digitOf(c);
        if (d >= 0) out += DIGIT_CN[d];
        else if (/[.．·]/.test(c) && out && !dotDone) {
          out += '点';
          dotDone = true;
        }
      }
      return out;
    }
    if (isYearContext(tail, count) || count > 4) return digitsToCn(raw);
    const n = Number(ascii);
    return Number.isSafeInteger(n) ? numToCn(n) : digitsToCn(raw);
  }

  // 一个「完整数字串」：起于数字、止于数字，中间允许小数点 / 千分位 / 零宽字符。
  // 收尾拖着的分隔符不算进串里（`14.` 的句点还给正文）。
  const NUM_RUN = /[0-9０-９][0-9０-９\u200B\u200C\u200D\uFEFF\u00AD.,．·\-–－]*[0-9０-９]|[0-9０-９]/g;

  // 标记保护：方括号标记内部（注号 / 拼音 / 数字串）不参与清洗——
  // 否则 `[拼音:cháng]` 会被当成「数字前是字母」，包成 `[拼音:ch[数字:á]ng]`。
  const MARKER_SPLIT = /(\[(?:注|高亮|拼音|数字)[^\]]*\]|\[\/(?:注|高亮)\])/g;

  /** 只对「标记之外」的文本段跑 fn，标记原样保留。 */
  function outsideMarkers(text, fn) {
    return text
      .split(MARKER_SPLIT)
      .map((seg, i) => (i % 2 === 1 ? seg : fn(seg)))
      .join('');
  }

  /**
   * 阶段四：连续数字转汉字；转不动的整串包成 `[数字:…]`。
   *
   * @example cleanNumbers('公元223年')      // '公元二二三年'
   * @example cleanNumbers('全书1980页')     // '全书一千九百八十页'
   * @example cleanNumbers('14428')          // '一四四二八'
   * @example cleanNumbers('ISBN 978-7-02')  // 'ISBN [数字:978-7-02]'
   */
  function cleanNumbers(text) {
    return outsideMarkers(text, (seg) =>
      seg.replace(NUM_RUN, (raw, offset, whole) => {
        const before = whole.slice(0, offset);
        const after = whole.slice(offset + raw.length);
        const prev = (before.replace(/\s+$/, '').slice(-1) || '');
        const next = (after.replace(/^\s+/, '')[0] || '');
        const digits = (raw.match(/[0-9０-９]/g) || []).length;
        if (shouldSkipNumber(prev, next, digits)) return '[数字:' + raw + ']';
        return runToCn(raw, after.slice(0, 3));
      })
    );
  }

  // =========================================================================
  // 2. 括号转夹注（现代标点古籍化）
  // =========================================================================

  const NOTE_MAX_CHARS = 60;

  /**
   * 阶段三之一：括注 → `[注]…[/注]`（双行夹注）；空括号直接删除。
   *
   * 五族括号一视同仁：`（）` `()` `[]` `［］` `【】`（`()` 会先被全角化成 `（）`）。
   *
   * 三条去路（v1.25.0 口径）：
   *   · **空括号**（`（）` `[]` `【】` 里一个字都没有）→ **整个删掉**：
   *     它只是排版留下的空壳，竖排里就是两个空格子；
   *   · 有字且 ≤ 60 字 → `[注]…[/注]`；
   *   · 超长 / 套着别的标记 / 不配对 → 退竖排双弯引号 ﹁ ﹂，**绝不吞后文**。
   *
   * @example parensToNote('道可道（可以说出的道）非常道')
   *          // '道可道[注]可以说出的道[/注]非常道'
   * @example parensToNote('曰（）者')   // '曰者'
   * @example parensToNote('天[自然也]') // '天[注]自然也[/注]'
   */
  function parensToNote(text) {
    return outsideMarkers(text, (seg) =>
      seg
        // 一行正则收「非空、无嵌套、≤60 字」的括注（五族括号都算）。
        .replace(/[（(［【\[]([^（）()［］【】\[\]]{1,60})[）)］】\]]/g, (m, inner) => {
          const t = inner.trim();
          if (!t) return ''; // 空壳：连括号一起丢掉
          if (t.includes('[')) return '﹁' + inner + '﹂';
          return '[注]' + t + '[/注]';
        })
        // 剩下的空括号（`（）` `[]`）：整对删掉，不留空格子。
        .replace(/[（(［【\[]\s*[）)］】\]]/g, '')
        // 不配对的圆括号退竖排双弯引号；残缺方括号原样留着（可能是坏标记）。
        .replace(/[（(]/g, '﹁')
        .replace(/[）)]/g, '﹂')
    );
  }

  /** 半角 → 全角标点；数字之间的 `.` `,` 不动（`3.14` 不能被断成 `3。14`）。 */
  function fullwidthPunct(s) {
    return s.replace(/[,.]|[?!;:()]/g, (m, offset, whole) => {
      if (m === ',' || m === '.') {
        if (isDigit(whole[offset - 1]) && isDigit(whole[offset + 1])) return m;
        return m === ',' ? '，' : '。';
      }
      return { '?': '？', '!': '！', ';': '；', ':': '：', '(': '（', ')': '）' }[m];
    });
  }

  /**
   * 阶段三之二：竖排标点替换。
   *
   *   《》→︽︾（竖排书名号）  ……→︙（竖排省略线）  ——→︱（竖排破折号）
   *   “”→「」（竖排引号）
   */
  function classicPunct(text) {
    return outsideMarkers(text, (seg) =>
      seg
        .replace(/《/g, '︽')
        .replace(/》/g, '︾')
        .replace(/…+/g, '︙')
        .replace(/[—―]+/g, '︱')
        .replace(/[“„]/g, '「')
        .replace(/[”]/g, '」')
    );
  }

  /**
   * 阶段三全跑：全角化 → 括号转夹注 → 竖排标点。
   *
   * 三段都必须绕开标记：`[拼音:xuán]` 里那个冒号一旦被全角化成 `：`，
   * 下游就认不出标记了——注音会原样印成方括号。
   */
  function classicizePunct(text) {
    return classicPunct(parensToNote(outsideMarkers(text, fullwidthPunct)));
  }

  // =========================================================================
  // 3. ruby 注音提取
  // =========================================================================
  //
  // EPUB3 的注音是结构化标签：
  //     <ruby>汉<rp>(</rp><rt>hàn</rt><rp>)</rp></ruby>
  // 竖排引擎要的是「基字 + 格内注音标记」：`汉[拼音:hàn]`。
  // `<rp>` 是给不支持 ruby 的阅读器看的兜底括号，必须扔掉，否则正文里
  // 会多出两个莫名的小括号。

  /**
   * **DOMParser 版（推荐）**：就地改写 document，把每个 `<ruby>` 换成文本节点。
   *
   * 用真 DOM 的好处：`rt` 里的 `<span>`/`<em>` 之类脏标签会被 `textContent`
   * 自动扒平；`rp` 可以被精确 `.remove()`；不用担心自闭合写法差异。
   *
   * @param {Document} doc  DOMParser 产出的文档
   * @returns {number} 处理掉的 ruby 个数
   */
  function extractRuby(doc) {
    let n = 0;
    const rubies = Array.from(doc.querySelectorAll('ruby'));
    for (const ruby of rubies) {
      // ① 先收拼音：一个 ruby 可能有多段 rt（多字词逐字注音）。
      const rts = Array.from(ruby.querySelectorAll('rt'))
        .map((rt) => (rt.textContent || '').trim())
        .filter(Boolean);
      // ② 再删兜底括号与 rt，剩下的才是基字。
      ruby.querySelectorAll('rp,rt').forEach((el) => el.remove());
      const base = (ruby.textContent || '').replace(/\s+/g, '');
      const py = rts.join(' ').trim();
      const text = py ? base + '[拼音:' + py + ']' : base;
      ruby.replaceWith(doc.createTextNode(text));
      n++;
    }
    return n;
  }

  /**
   * **正则版（无 DOM 环境兜底）**：字符串进、字符串出，语义与 DOM 版一致。
   *
   * Node 里没有 DOMParser（除非装 jsdom），服务端批量转换时用这个。
   * 只处理单层 ruby —— EPUB 里 ruby 不嵌套。
   *
   * @example rubyToPinyin('<ruby>汉<rp>(</rp><rt>hàn</rt><rp>)</rp></ruby>字')
   *          // '汉[拼音:hàn]字'
   */
  function rubyToPinyin(html) {
    return html.replace(
      /<ruby\b[^>]*>([\s\S]*?)<\/ruby>/gi,
      (m, inner) => {
        const py = [];
        // 收 rt 内容（可能多段）。
        inner.replace(/<rt\b[^>]*>([\s\S]*?)<\/rt>/gi, (mm, body) => {
          const t = body.replace(/<[^>]+>/g, '').trim();
          if (t) py.push(t);
          return '';
        });
        const base = inner
          .replace(/<rp\b[^>]*>[\s\S]*?<\/rp>/gi, '') // 兜底括号整块扔
          .replace(/<rt\b[^>]*>[\s\S]*?<\/rt>/gi, '') // 去掉注音
          .replace(/<[^>]+>/g, '') // 扒平基字里的行内标签
          .replace(/\s+/g, '');
        return py.length ? base + '[拼音:' + py.join(' ') + ']' : base;
      }
    );
  }

  // =========================================================================
  // 4. 结构降噪 + 注释提取（DOM 层）
  // =========================================================================

  /** 整块连内容一起丢的容器。 */
  const DROP_SELECTOR = [
    'script',
    'style',
    'head',
    'nav',
    'footer',
    'aside',
    'form',
    'svg',
    'iframe',
    'noscript',
    'button',
    'select',
    'textarea',
    'object',
    'embed',
    'audio',
    'video',
    'canvas',
    'template',
  ].join(',');

  const AD_TOKENS = ['ad', 'ads', 'adv', 'advert', 'adverts', 'advertisement', 'promo', 'promotion', 'banner', 'sponsor', 'dfp'];
  const COLOR_CLASSES = ['red', 'green', 'blue', 'yellow', 'cyan', 'magenta', 'purple', 'orange', 'brown', 'gray', 'grey', 'color', 'colour'];

  /** class / id 里是不是广告语汇（按词比对，`adult` 不中招）。 */
  function isAdElement(el) {
    for (const v of [el.getAttribute('class') || '', el.getAttribute('id') || '']) {
      const words = v.split(/[^A-Za-z0-9]+/).filter(Boolean);
      if (words.some((w) => AD_TOKENS.includes(w.toLowerCase()))) return true;
      // `class="adBanner"` 这种驼峰靠前缀兜一层。
      if (/^ad[A-Z_-]/.test(v)) return true;
    }
    return false;
  }

  /** 脚注体：`epub:type="footnote"` / `role="doc-footnote"` / `class="footnote"`。 */
  function isFootnoteElement(el) {
    const et = (el.getAttribute('epub:type') || el.getAttributeNS('http://www.idpf.org/2007/ops', 'type') || '').toLowerCase();
    if (/footnote|endnote|rearnote/.test(et)) return true;
    if (/doc-footnote/.test((el.getAttribute('role') || '').toLowerCase())) return true;
    return /(^|[\s_-])(footnote|footnotes|endnote|endnotes|fn)([\s_-]|$)/i.test(el.getAttribute('class') || '');
  }

  /**
   * 阶段一：结构降噪。
   *
   *   · 剔噪块（script / style / nav / aside / 多媒体壳）；
   *   · 剔广告块（`class="ad-banner"` 之类，整块连内容）；
   *   · **剥一切颜色**：`style="color:#c00"`、`<font color>`、`class="red"`。
   *     颜色归主题引擎管——正文里嵌死红字，在夜读主题下就是一团看不见的墨。
   */
  function denoiseDOM(doc) {
    doc.querySelectorAll(DROP_SELECTOR).forEach((el) => el.remove());
    doc.querySelectorAll('[class],[id]').forEach((el) => {
      if (el.isConnected !== false && isAdElement(el)) el.remove();
    });
    doc.querySelectorAll('[style]').forEach((el) => {
      const st = el.getAttribute('style') || '';
      const kept = st
        .split(';')
        .map((d) => d.trim())
        .filter(Boolean)
        .filter((d) => !/^(color|background-color)\s*:/i.test(d))
        .join(';');
      if (kept) el.setAttribute('style', kept);
      else el.removeAttribute('style');
    });
    doc.querySelectorAll('[color],[bgcolor]').forEach((el) => {
      el.removeAttribute('color');
      el.removeAttribute('bgcolor');
    });
    doc.querySelectorAll('[class]').forEach((el) => {
      const kept = (el.getAttribute('class') || '')
        .split(/\s+/)
        .filter(Boolean)
        .filter((w) => {
          const low = w.toLowerCase();
          const bare = low.replace(/^(text|font|c)-?/, '');
          return !COLOR_CLASSES.includes(bare) && !COLOR_CLASSES.includes(low);
        });
      if (kept.length) el.setAttribute('class', kept.join(' '));
      else el.removeAttribute('class');
    });
    // `<font>` 剥成空壳就没意义了（它唯一的作用就是上色）。
    doc.querySelectorAll('font').forEach((el) => {
      if (!el.attributes.length) el.replaceWith(...Array.from(el.childNodes));
    });
    return doc;
  }

  /**
   * 阶段二：把脚注「抽出来、按 href 落回注位」。
   *
   * 顺序很关键：**先收脚注体**（`id` → 纯文本）并删掉原件，再把 noteref
   * 换成 `[注]注文[/注]`。反过来做就找不到落点了，只能退回一个光秃秃的注号。
   *
   * @returns {Map<string,string>} id → 注文
   */
  function resolveNoteref(doc) {
    const notes = new Map();
    doc.querySelectorAll('[epub\\:type="footnote"],[role="doc-footnote"],[class*="footnote"],[epub\\:type="endnote"]').forEach((el) => {
      if (!el.id) return;
      if (!isFootnoteElement(el)) return;
      const t = squeezeSpaces(el.textContent || '');
      if (t) notes.set(el.id, t);
      el.remove(); // 抽完即删，避免篇末再重复一遍
    });
    doc.querySelectorAll('a[epub\\:type="noteref"],a[role="doc-noteref"],a[href^="#fn"],a[href^="#note"]').forEach((a) => {
      const id = (a.getAttribute('href') || '').replace(/^#/, '');
      const body = notes.get(id) || (a.textContent || '').trim();
      a.replaceWith(doc.createTextNode(body ? '[注]' + body + '[/注]' : ''));
    });
    return notes;
  }

  // =========================================================================
  // 5. 章节与段落重构
  // =========================================================================

  const PARA_TAGS = new Set(['p', 'li', 'td', 'dd', 'dt', 'blockquote', 'pre', 'figcaption']);
  const BOX_TAGS = new Set(['div', 'section', 'article', 'main', 'body', 'html', 'figure', 'ul', 'ol', 'table', 'tr', 'dl', 'header']);

  /** 题署：`h1~h6`、`epub:type="title"`、`role="doc-title"`、`class="title"` 一族。 */
  function isTitleElement(el) {
    const tag = el.tagName.toLowerCase();
    if (/^h[1-6]$/.test(tag)) return true;
    const et = (el.getAttribute('epub:type') || el.getAttributeNS('http://www.idpf.org/2007/ops', 'type') || '').toLowerCase();
    if (/(^|\s)(title|fulltitle|subtitle)(\s|$)/.test(et)) return true;
    if ((el.getAttribute('role') || '') === 'doc-title') return true;
    return /\b(title|booktitle|chapter-title|chaptertitle|chapter_title|headline)\b/i.test(el.getAttribute('class') || '');
  }

  /**
   * 横排页码残留：整行只有数字（可带 `· - — . 空格 [ ] （） 第…页` 装饰）。
   *
   * **位数不设上限**——PDF 转 EPUB 的页码常是 `20161`、`20162` 这种
   * 「年份前缀 + 页码」的五位串，按「1~4 位」判会漏一大片。
   */
  function isPageNumber(line) {
    const t = line.trim();
    let digits = 0;
    for (const c of t) {
      if (isDigit(c)) digits++;
      else if (!'·-—. []（）()第页/／'.includes(c)) return false;
    }
    return digits > 0;
  }

  /** 只剩标点 / 数字的乱码行：`·`、`——`、`[]`、`[〕`。 */
  function isJunkLine(s) {
    let any = false;
    for (const c of s) {
      if (/\s/.test(c) || TRANSPARENT.test(c)) continue;
      if (isDigit(c) || PUNCT.test(c)) {
        any = true;
        continue;
      }
      return false;
    }
    return any;
  }

  /**
   * **剔除正文里孤立的阿拉伯数字**（扫描残渣 / 行号 / 页眉角标）。
   *
   * 「孤立」只有一条判据：**数字串两侧都不挨汉字**。
   *
   *   '20161'          两侧是边界          → 删（扫描页码）
   *   '· 7 ·'          两侧是标点 / 空白     → 删（页眉角标）
   *   '第3章'  '共15卷' 一侧挨着汉字          → 留（交给 cleanNumbers）
   *   '圆周率3.14'      '3.14' 是整串，左侧挨「率」→ 留
   *
   * 顺序铁律：**必须先刮再转**——`20161` 一旦读成「二〇一六一」，
   * 下游就再也认不出它是页码了。
   *
   * @example scrubStrayDigits('见牡丹盛开采撷 20161 有感')  // '见牡丹盛开采撷 有感'
   * @example scrubStrayDigits('全书共14428字')              // 原样（两侧有汉字）
   */
  function scrubStrayDigits(line) {
    return outsideMarkers(line, (seg) =>
      seg.replace(/[0-9０-９]+(?:[.,．·][0-9０-９]+)*/g, (run, at, whole) => {
        const before = whole.slice(0, at).replace(/\s+$/, '').slice(-1);
        const tailRaw = whole.slice(at + run.length);
        const after = tailRaw.replace(/^\s+/, '').slice(0, 1);
        const attach = (c) => !!c && isCJK(c);
        if (attach(before) || attach(after)) {
          // 行尾孤零零的一串（后面只剩空白 / 标点）：扫描页码的典型位置，
          // 即便前面紧挨着汉字也删。带小数点的不是页码。
          const atTail = /^[\s，。、；：？！…—·「」『』﹁﹂【】︽︾︙︱]*$/.test(tailRaw);
          if (atTail && !/[.．]/.test(run)) return '';
          return run;
        }
        return ''; // 两侧都不挨汉字 → 孤立数字，整串删掉
      })
    );
  }

  /**
   * 阶段五：段落重构。
   *
   * 三道过滤的**顺序**是死的：
   *   1. 整行是页码（还看得见 ASCII 时判最准）→ 丢；
   *   2. 行内孤立数字（`20161`）**先刮再转**——先读成「二〇一六一」就认不出了；
   *   3. 清完只剩标点 / 数字的乱码行 → 丢。
   */
  function stageParagraph(raw) {
    const line = raw.trim();
    if (!line || isPageNumber(line)) return '';
    const out = squeezeSpaces(cleanNumbers(classicizePunct(scrubStrayDigits(line))));
    return isJunkLine(out) ? '' : out;
  }

  /** 走一遍 DOM，按题署切章。 */
  function collectChapters(doc) {
    const out = [];
    let title = '正文';
    let paras = [];
    const flush = () => {
      if (paras.length) out.push({ title: squeezeSpaces(cleanNumbers(classicizePunct(title))), content: paras.join('\n') });
      paras = [];
    };
    const push = (text) => {
      const line = stageParagraph(text);
      if (line) paras.push(line);
    };
    const walk = (el) => {
      for (const child of Array.from(el.children)) {
        const tag = child.tagName.toLowerCase();
        if (['script', 'style', 'br', 'hr'].includes(tag)) continue;
        if (isTitleElement(child)) {
          flush();
          const t = (child.textContent || '').trim();
          if (t) title = t;
          continue;
        }
        if (PARA_TAGS.has(tag)) {
          push(child.textContent || '');
          continue;
        }
        if (BOX_TAGS.has(tag)) {
          walk(child);
          continue;
        }
        if (!child.children.length) push(child.textContent || '');
      }
    };
    walk(doc.body || doc.documentElement);
    flush();
    return out.filter((c) => c.content.trim());
  }

  // =========================================================================
  // 5.5 书级清洗：书眉 / 书口文字（跨章才判得出「重复」）
  // =========================================================================

  const HEAD_MAX_CHARS = 30;
  const HEAD_MIN_REPEAT = 3;

  /** 汉字数词（含〇零两与大写）。 */
  const HAN_NUMERAL = /[〇零一二三四五六七八九十百千万两壹贰叁肆伍陆柒捌玖拾]/;

  /**
   * **书级清洗**：把「页眉 / 书口文字」从章题与正文里赶出去。
   *
   * 电子书每页顶端都印着同一行书眉（`历代名家词集精华录卷十九`），
   * PDF/OCR 转 EPUB 时被当成正文段落抓进来——竖排以后，章题列（版心）
   * 里赫然一行书眉，正文每隔几十行又冒一遍。
   *
   * 两道处理：
   *   ① 章题：剥书名前缀与「卷十九」这类卷次尾巴（卷次由引擎按章序生成）；
   *   ② 正文：去标点后重复 ≥3 次、且形态像书眉（以书名开头 / 以卷次数词
   *      结尾）的短段，整段删除。**两条同时成立才删**，宁可漏删也不误伤
   *      （诗里的叠句也会重复，但它不含书名、也不以卷次结尾）。
   *
   * @param {{title:string, content:string}[]} chapters
   * @param {string} bookTitle OPF 的 dc:title
   */
  function purgeRunningHeaders(chapters, bookTitle) {
    const book = cleanBookTitle(bookTitle || '');
    for (const c of chapters) c.title = stripHeadNoise(c.title, book);

    const norm = (s) => s.replace(/[\s，。、；：？！…—·「」『』﹁﹂【】︽︾︙︱[\]]/g, '');
    const freq = new Map();
    for (const c of chapters) {
      for (const line of c.content.split('\n')) {
        const k = norm(line);
        if (k && k.length <= HEAD_MAX_CHARS) freq.set(k, (freq.get(k) || 0) + 1);
      }
    }
    for (const c of chapters) {
      c.content = c.content
        .split('\n')
        .filter((line) => {
          const k = norm(line);
          const n = freq.get(k) || 0;
          // ① 重复 ≥3 次的短行；② 以书名 + 卷次开头的短行（短书里书眉
          // 可能只出现一两次，靠重复次数抓不住）。
          const repeated = n >= HEAD_MIN_REPEAT;
          const titled = startsWithBook(k, book);
          return !(looksLikeRunningHead(k, book) && (repeated || titled));
        })
        .join('\n');
    }
    return chapters.filter((c) => c.content.trim());
  }

  /** 书名清洗：去书名号、从「卷 / 册 / （」处截断书眉污染。 */
  function cleanBookTitle(raw) {
    const t = squeezeSpaces(raw.replace(/[《》〈〉【】「」]/g, ''));
    const i = t.search(/[卷册冊（(：:·]/);
    const head = i > 0 ? t.slice(0, i).trim() : t.trim();
    return head.length >= 2 ? head : t.trim();
  }

  /** 章题清洗：剥书名前缀 + 卷次前缀；剥空了就留空（版心自有卷次）。 */
  function stripHeadNoise(title, book) {
    let t = (title || '').trim();
    if (book.length >= 4) t = t.split(book).join('');
    const m = t.trim().match(/^(?:卷|第|之)?[〇零一二三四五六七八九十百千万两0-9０-９]+([\s　·]*)(.*)$/);
    if (m) {
      const tight = !m[1] && m[2]; // 数词后紧贴文字（「第一章　道可道」）→ 不动
      if (!tight) return (m[2] || '').replace(/[卷冊册]$/, '').trim();
    }
    return t.trim();
  }

  /** 像不像书眉：书名 + 卷次，或以「卷 + 数词」结尾。 */
  function looksLikeRunningHead(t, book) {
    if (!t) return false;
    if (startsWithBook(t, book)) return true;
    return /[卷冊册集部][之〇零一二三四五六七八九十百千两0-9０-９]+$/.test(t);
  }

  /**
   * 这一行是不是「书名 + 卷次」的书眉（书名 ≥4 字才算数）。
   *
   * 只看「以书名开头」会误伤——序跋里常有 `红楼梦第一回……` 这种正文；
   * 故要求书名之后只剩一小截，且其中带卷次（`卷十九`）或整截都是数词。
   */
  function startsWithBook(t, book) {
    if (book.length < 4 || !t.startsWith(book)) return false;
    const rest = t.slice(book.length);
    if (!rest) return true;
    if (rest.length > 12) return false;
    return /[卷冊册]/.test(rest) || /^[〇零一二三四五六七八九十百千两0-9０-９]+$/.test(rest);
  }

  // =========================================================================
  // 5.6 渲染几何：书名号 / 直角引号 / 句读落位（与 Rust 引擎同源）
  // =========================================================================

  /**
   * **书名号**：横排 `《》` → 竖排专用 `︽︾`（单书名号 `〈〉` → `︿﹀`）。
   *
   * 竖排里 `《` 的尖角朝下，插在列中把行气戳断；`︽` 是它的竖排形态。
   */
  function titleMark(c) {
    return { '《': '︽', '》': '︾', '〈': '︿', '〉': '﹀' }[c] || c;
  }

  /**
   * **直角引号旋转 90°**：`「`→`﹁`、`」`→`﹂`、`『`→`﹃`、`』`→`﹄`。
   *
   * 竖排里引号的开口要朝下（读序方向）。GPUI 没有元素级旋转，故直接用
   * Unicode 的竖排呈现形（U+FE41..FE44）——字库里存的就是转好的那一枚。
   */
  function verticalQuote(c) {
    return { '「': '﹁', '」': '﹂', '『': '﹃', '』': '﹄', '“': '﹁', '”': '﹂', '‘': '﹃', '’': '﹄' }[c] || c;
  }

  /** 标点字号 / 正文字号 = **0.4**；书名号 = **0.8**。 */
  const PUNCT_FS_RATIO = 0.4;
  const TITLE_FS_RATIO = 0.8;
  /** 句读起点偏移 / 字格：**0.3**（贴格子右下角）。 */
  const PUNCT_DX_RATIO = 0.3;
  const PUNCT_DY_RATIO = 0.3;

  /**
   * **句读落位公式**（`punct::place` 的 JS 版，逐字对齐）：
   *
   * ```text
   * dx = cellW × 0.30            y = cellH × 0.30
   * w  = cellW − dx              h  = cellH − dy
   * fs = clamp(正文 × 0.4, 6, 22) 再 min(w, h)  —— 再大也压不出格
   * 盒内**靠右下对齐**：字体把 。，、 的墨迹放在 em box 左下，
   * 靠右下对齐后墨点才落在格子的右下角（靠左上会飘回格心）。
   * ```
   *
   * @returns {{fs:number, dx:number, dy:number, w:number, h:number}}
   */
  function punctPlace(fsBody, cellW, cellH) {
    const dx = Math.max(cellW * PUNCT_DX_RATIO, 0);
    const dy = Math.max(cellH * PUNCT_DY_RATIO, 0);
    const w = Math.max(cellW - dx, 1);
    const h = Math.max(cellH - dy, 1);
    const room = Math.min(w, h);
    let fs = Math.min(fsBody * PUNCT_FS_RATIO, room);
    if (room >= 6) fs = Math.max(fs, 6);
    fs = Math.min(fs, 22);
    return { fs, dx, dy, w, h };
  }

  /** 书名号落位：同一格，字号 **0.8 倍**正文（不与句读同档）。 */
  function titlePlace(fsBody, cellH) {
    return { fs: Math.min(Math.max(fsBody * TITLE_FS_RATIO, 9), cellH) };
  }

  // =========================================================================
  // 6. 总装
  // =========================================================================

  /**
   * 五阶段全跑：EPUB 的 xhtml 字符串 → `[{ title, content }]`。
   *
   * @param {string} html  xhtml / html 源码
   * @param {{textFallback?: boolean}} [opts]
   *        `textFallback: true` → 没有 DOMParser 时退「正则版」路径（精度略低）
   * @returns {{title: string, content: string}[]}
   */
  function cleanEpub(html, opts) {
    const textFallback = opts && opts.textFallback;
    if (typeof DOMParser === 'undefined') {
      if (!textFallback) {
        throw new Error('cleanEpub 需要 DOMParser；Node 环境请传 { textFallback: true } 走正则路径');
      }
      return cleanEpubText(html);
    }
    const doc = new DOMParser().parseFromString(html, 'text/html');
    // ② **先**注文落位，① **后**结构降噪 —— 顺序不能反：
    //    脚注体通常装在 `<aside>` 里，而降噪会把 `<aside>` 整块删掉。
    //    先降噪的话，注号就只能退回一个光秃秃的数字了。
    resolveNoteref(doc); // ② 脚注体抽出来 → noteref 换成 [注]注文[/注]
    extractRuby(doc); // ② ruby → [拼音:…]
    denoiseDOM(doc); // ① 结构降噪：噪块 / 广告 / 颜色
    return collectChapters(doc); // ⑤ 段落与章节重构（③④ 逐段做）
  }

  /** 从标签属性串里取一个值（引号单双都认）。 */
  function attrOf(attrs, key) {
    const m = attrs.match(new RegExp('\\b' + key + '\\s*=\\s*["\']([^"\']*)["\']', 'i'));
    return m ? m[1] : '';
  }

  /**
   * 无 DOM 的兜底管线（正则版）：注文落位 → ruby → 降噪 → 按 h1~h6 切章。
   *
   * 精度不如 DOM 版（嵌套 div 的广告块只能按「同名标签配对」近似），
   * 但服务端批量转换 / Node 脚本够用。
   */
  function cleanEpubText(html) {
    let s = html;

    // ② 注文先落位：脚注体常装在 `<aside>` 里，降噪会连它一起删掉。
    //    属性引号单双都要认——XHTML 里 `epub:type='footnote'` 合法。
    const notes = new Map();
    s = s.replace(/<(\w+)\b([^>]*\b(?:footnote|endnote)s?\b[^>]*)>([\s\S]*?)<\/\1>/gi, (m, tag, attrs, body) => {
      const id = attrOf(attrs, 'id');
      const t = squeezeSpaces(body.replace(/<[^>]+>/g, ''));
      if (id && t) notes.set(id, t);
      return ''; // 抽完即删，免得篇末重复
    });
    s = s.replace(/<a\b([^>]*\bnoteref\b[^>]*)>([\s\S]*?)<\/a>/gi, (m, attrs, body) => {
      const id = attrOf(attrs, 'href').replace(/^#/, '');
      const t = (id && notes.get(id)) || body.replace(/<[^>]+>/g, '').trim();
      return t ? '[注]' + t + '[/注]' : '';
    });

    // ② ruby 注音
    s = rubyToPinyin(s);

    // ① 降噪：噪块 + 注释 + 颜色
    for (const tag of ['script', 'style', 'head', 'nav', 'footer', 'aside', 'form', 'svg', 'iframe', 'noscript', 'object', 'embed', 'audio', 'video']) {
      s = s.replace(new RegExp('<' + tag + '\\b[\\s\\S]*?<\\/' + tag + '>', 'gi'), '');
    }
    s = s.replace(/<!--[\s\S]*?-->/g, '');
    s = s.replace(/(\s(?:style|color|bgcolor)\s*=\s*["'])([^"']*)(["'])/gi, (m, a, v, b) => {
      const kept = v
        .split(';')
        .map((d) => d.trim())
        .filter(Boolean)
        .filter((d) => !/^(color|background-color)\s*:/i.test(d))
        .join(';');
      return kept ? a + kept + b : '';
    });
    s = s.replace(/\sclass\s*=\s*["']([^"']*)["']/gi, (m, v) => {
      const kept = v
        .split(/\s+/)
        .filter(Boolean)
        .filter((w) => !COLOR_CLASSES.includes(w.replace(/^(text|font|c)-?/, '').toLowerCase()));
      return kept.length ? ' class="' + kept.join(' ') + '"' : '';
    });
    // ⑤ 切章：按 h1~h6 / class="title" 分块
    const parts = s.split(/(<h[1-6]\b[^>]*>[\s\S]*?<\/h[1-6]>|<(?:\w+)\b[^>]*(?:epub:type="title"|class="[^"]*\btitle\b[^"]*")[^>]*>[\s\S]*?<\/\w+>)/i);
    const out = [];
    let title = '正文';
    let paras = [];
    const flush = () => {
      if (paras.length) out.push({ title: squeezeSpaces(cleanNumbers(classicizePunct(title))), content: paras.join('\n') });
      paras = [];
    };
    for (const part of parts) {
      if (!part) continue;
      const m = part.match(/^<(h[1-6]|\w+)\b/i);
      if (m && /^<h[1-6]\b/i.test(part)) {
        flush();
        const t = part.replace(/<[^>]+>/g, '').trim();
        if (t) title = t;
        continue;
      }
      if (m && /epub:type="title"|class="[^"]*\btitle\b/i.test(part.slice(0, part.indexOf('>')))) {
        flush();
        const t = part.replace(/<[^>]+>/g, '').trim();
        if (t) title = t;
        continue;
      }
      // 正文段：按块级标签切片，逐片清洗
      const chunks = part.split(/<\/(?:p|div|li|td|dd|blockquote|section|article)>/i);
      for (const ch of chunks) {
        const text = ch.replace(/<[^>]+>/g, ' ');
        const line = stageParagraph(text);
        if (line) paras.push(line);
      }
    }
    flush();
    return out.filter((c) => c.content.trim());
  }

  // =========================================================================
  // 7. 导出
  // =========================================================================

  return {
    // 总装
    cleanEpub,
    cleanEpubText,
    // 五阶段（可单独取用）
    denoiseDOM,
    resolveNoteref,
    extractRuby, // DOMParser 版
    rubyToPinyin, // 正则版
    classicizePunct,
    parensToNote,
    classicPunct,
    fullwidthPunct,
    cleanNumbers,
    // 数字小件
    numToCn,
    digitsToCn,
    runToCn,
    shouldSkipNumber,
    isYearContext,
    // 书级清洗
    purgeRunningHeaders,
    cleanBookTitle,
    stripHeadNoise,
    looksLikeRunningHead,
    scrubStrayDigits,
    isJunkLine,
    // 渲染几何（与 Rust 引擎同源）
    titleMark,
    verticalQuote,
    punctPlace,
    titlePlace,
    PUNCT_FS_RATIO,
    TITLE_FS_RATIO,
    PUNCT_DX_RATIO,
    PUNCT_DY_RATIO,
    // 工具
    squeezeSpaces,
    isPageNumber,
    stageParagraph,
    outsideMarkers,
  };
});
