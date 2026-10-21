/* ============================================================================
 * 笺读 · 统一导入与古籍化清洗管线（JS 参考实现）
 * ----------------------------------------------------------------------------
 * 与 Rust 侧 `src/pipeline.rs` 同构：同样的四道工序、同样的输出格式，
 * 便于把这套逻辑搬到 Web / 小程序 / Electron 端，或拿来对照排查导入问题。
 *
 *   parseToJianDuFormat(file) -> Promise<[{ title, content }, ...]>
 *
 * 四道工序：
 *   1. 解码   —— UTF-8 / UTF-16(BOM) / GBK
 *   2. 分章   —— txt 认 /^第[一二三四五六七八九十百千万0-9]+[章回卷节]/m，无标识按 3000 字切
 *   3. 降噪   —— md 去代码块与图片；html 去 script/style/nav，只留正文骨架
 *   4. 古籍化 —— 半角标点转全角、`>` / `<em>` 转 [注]…[/注]、`**` / `<strong>` 转 [高亮]…[/高亮]
 *
 * 无依赖，浏览器与 Node 都能跑（Node 下没有 DOMParser 时用内置标签扫描器）。
 * ==========================================================================*/

// ---------------------------------------------------------------------------
// 0. 常量
// ---------------------------------------------------------------------------

const CHUNK_CHARS = 3000;        // 无章节标识时的强制切分字数
const CHUNK_LOOKBACK = 240;      // 切分时回找句读的窗口

const NOTE_OPEN = '[注]', NOTE_CLOSE = '[/注]';
const HI_OPEN = '[高亮]', HI_CLOSE = '[/高亮]';

// ---------------------------------------------------------------------------
// 1. 解码：UTF-8 / UTF-16 / GBK
// ---------------------------------------------------------------------------

/** 任意编码字节 → 字符串。BOM 优先，其次 UTF-8 合法性，最后退回 GBK。 */
function decodeBytes(bytes) {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  // UTF-8 BOM
  if (u8[0] === 0xef && u8[1] === 0xbb && u8[2] === 0xbf) {
    return new TextDecoder('utf-8').decode(u8.subarray(3));
  }
  // UTF-16 BOM
  if (u8[0] === 0xff && u8[1] === 0xfe) return new TextDecoder('utf-16le').decode(u8.subarray(2));
  if (u8[0] === 0xfe && u8[1] === 0xff) return new TextDecoder('utf-16be').decode(u8.subarray(2));
  // UTF-8 合法性（fatal 严格解码，非法即抛）
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(u8);
  } catch (_) {
    /* 不是 UTF-8：按 GBK 解 */
  }
  try {
    // 浏览器（含 Electron / 小程序 WebView）都内置 GBK 码表；
    // Node 18+ 需要 full-icu，否则这里会抛 —— 那就退回 latin1 兜底并告警。
    return new TextDecoder('gbk').decode(u8);
  } catch (_) {
    console.warn('[笺读管线] 当前环境不支持 GBK 解码，已按 latin1 兜底');
    return new TextDecoder('latin1').decode(u8);
  }
}

// ---------------------------------------------------------------------------
// 2. 古籍化：半角标点 → 全角，阿拉伯数字 → 汉字
// ---------------------------------------------------------------------------

const CN_DIGITS = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
const CN_UNITS = ['', '十', '百', '千'];
const CN_SECTIONS = ['', '万', '亿', '兆'];

/** 一节（0~9999）的中文写法。 */
function sectionToCn(n) {
  const ds = String(n).padStart(4, '0').split('').map(Number);
  let s = '', zero = false;
  for (let i = 0; i < 4; i++) {
    const d = ds[i], unit = CN_UNITS[3 - i];
    if (d === 0) { zero = true; continue; }
    if (zero && s) s += '零';
    zero = false;
    s += CN_DIGITS[d] + unit;
  }
  return s;
}

/** 数值读法：`15 -> 十五`，`223 -> 二百二十三`，`1980 -> 一千九百八十`。 */
function numToCn(n) {
  if (!Number.isFinite(n) || n < 0) return String(n);
  if (n === 0) return '零';
  const secs = [];
  let x = Math.floor(n);
  while (x > 0) { secs.push(x % 10000); x = Math.floor(x / 10000); }
  let out = '';
  for (let i = secs.length - 1; i >= 0; i--) {
    const sec = secs[i];
    if (sec === 0) {
      // 只有后面还有非零节才补「零」：10000 是「一万」不是「一万零」
      if (out && !out.endsWith('零') && secs.slice(0, i).some((v) => v !== 0)) out += '零';
      continue;
    }
    // 一十一 → 十一（仅在最高节且 < 100 时省「一」，与刻本习惯一致）
    let piece = sectionToCn(sec);
    if (i === 0 && sec < 100 && piece.startsWith('一十')) piece = piece.slice(1);
    if (out && sec < 1000) out += '零';
    out += piece + CN_SECTIONS[i];
  }
  return out;
}

/** 逐字读：`223 -> 二二三`（零读「〇」更合刻本习惯）。 */
function numToCnDigits(digits) {
  return digits.replace(/[0-9]/g, (d) => CN_DIGITS[Number(d)]);
}

/** 是否纪年上下文（`223年` / `1980年代`）。 */
function isYearContext(tail, nDigits) {
  if (nDigits > 4) return false;
  return /^\s*(年|年代|年度|世纪|年春|年秋|年冬|年夏|年间)/.test(tail);
}

/** 该不该放过不转：与字母粘连（ISBN978 / A4）、编号前导、过长流水号。 */
function shouldSkip(prev, next, nDigits) {
  if (nDigits === 0 || nDigits > 12) return true;
  const isLetter = (c) => !!c && /[A-Za-z]/.test(c);
  if (isLetter(prev) || isLetter(next)) return true;
  if (prev && '#/\\@_=+*'.includes(prev)) return true;
  return false;
}

/** 阿拉伯数字 → 汉字（对应 Rust `numcn::normalize_numbers`）。 */
function normalizeNumbers(s) {
  // 全角数字先转半角，零宽空格去掉（EPUB 常在数字间插 &#8203;）
  const src = s.replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/[ \t]/g, (c) => c);
  return src.replace(/[0-9][0-9,.\u200b]*/g, (m, offset) => {
    const prev = offset > 0 ? src[offset - 1] : null;
    const next = src[offset + m.length] || null;
    const raw = m.replace(/[\u200b]/g, '');
    const digits = raw.replace(/[^0-9]/g, '');
    if (!digits) return m;
    if (shouldSkip(prev, next, digits.length)) return m;
    const tail = src.slice(offset + m.length, offset + m.length + 3);
    const year = isYearContext(tail, digits.length);
    const hasPoint = /[.．]/.test(raw);
    // 小数 / 纪年 → 逐字读（三点一四、二二三年）
    if (year || hasPoint) {
      let out = '';
      for (const c of raw) {
        if (/[0-9]/.test(c)) out += CN_DIGITS[Number(c)];
        else if (/[.．]/.test(c) && out) out += '点';
      }
      return out;
    }
    return numToCn(Number(digits));
  });
}

/** 半角标点 → 全角；数字里的小数点与千分位不动。 */
function fullwidthPunct(s) {
  const cs = [...s];
  return cs.map((c, i) => {
    const prev = i > 0 ? cs[i - 1] : null;
    const next = cs[i + 1] ?? null;
    const inNumber =
      prev !== null && /[0-9]/.test(prev) && next !== null && /[0-9]/.test(next);
    switch (c) {
      case ',': return inNumber ? c : '，';
      case '.': return inNumber ? c : '。';
      case '?': return '？';
      case '!': return '！';
      case ';': return '；';
      case ':': return '：';
      case '(': return '（';
      case ')': return '）';
      default: return c;
    }
  }).join('');
}

/** 一道古籍化：全角标点 + 数字转汉字。 */
function classicize(s) {
  return normalizeNumbers(fullwidthPunct(s));
}

// ---------------------------------------------------------------------------
// 3. 格式嗅探
// ---------------------------------------------------------------------------

function sniffKind(name, head) {
  const ext = (name.split('.').pop() || '').toLowerCase();
  if (['md', 'markdown', 'mdown'].includes(ext)) return 'markdown';
  if (['html', 'htm', 'xhtml', 'xml', 'opf', 'xht'].includes(ext)) return 'html';
  if (ext === 'json') return 'json';
  if (ext === 'epub') return 'epub';
  if (['txt', 'text'].includes(ext)) return 'txt';
  const t = head.trimStart();
  if (/^<!doctype html|^<html|^\<\?xml/i.test(t)) return 'html';
  if (t.startsWith('{') || t.startsWith('[')) return 'json';
  if (t.startsWith('#') || t.includes('\n# ') || t.includes('```') || t.includes('](')) return 'markdown';
  return 'txt';
}

// ---------------------------------------------------------------------------
// 4. TXT 解析器：兜底与智能切章
// ---------------------------------------------------------------------------

/** 章节标题行：`第` + 中文数字/阿拉伯数字 + `章回卷节篇折出`。 */
function isChapterHead(line) {
  const s = line.trim();
  if (s.length < 3 || s.length > 40) return false;
  // 用户点名的正则： /^第[一二三四五六七八九十百千万0-9]+[章回卷节]/m
  return /^第[〇零一二三四五六七八九十百千万亿两廿卅0-9０-９]+[章回卷节篇折出]/.test(s);
}

function parseTxt(text) {
  const norm = text.replace(/\r\n?/g, '\n');
  const chapters = [];
  let title = '正文';
  let paras = [];
  let buf = '';
  let sawHead = false;

  const flushBuf = () => {
    if (buf.trim()) paras.push(buf.trim());
    buf = '';
  };
  const pushChap = () => {
    if (paras.length) chapters.push({ title, content: paras.join('\n') });
    paras = [];
  };

  for (const raw of norm.split('\n')) {
    const line = raw.trim();
    if (!line) { flushBuf(); continue; }
    if (isChapterHead(line)) {
      flushBuf();
      pushChap();
      title = line;
      sawHead = true;
      continue;
    }
    buf += line; // 中文 txt 常硬折行：同段直接相接，空行才分段
  }
  flushBuf();
  pushChap();
  if (!chapters.length) chapters.push({ title: '正文', content: '' });

  // 无章节标识 + 字数过长：按 3000 字强制切卷
  if (!sawHead) {
    const total = chapters.reduce((n, c) => n + [...c.content].length, 0);
    if (total > CHUNK_CHARS) return rechunk(chapters);
  }
  return chapters.map((c) => ({ title: classicize(c.title), content: classicize(c.content) }));
}

function rechunk(chapters) {
  const all = chapters.flatMap((c) => c.content.split('\n')).filter((l) => l.trim());
  const out = [];
  let cur = [], n = 0;
  for (const p of all) {
    const len = [...p].length;
    if (n + len > CHUNK_CHARS && cur.length) {
      cur = cutAtSentence(cur);
      out.push({ title: `卷${numToCn(out.length + 1)}`, content: cur.join('\n') });
      cur = []; n = 0;
    }
    if (len > CHUNK_CHARS) {
      // 单段本身超长：按字硬切
      const cs = [...p];
      for (let i = 0; i < cs.length; i += CHUNK_CHARS) {
        if (cur.length) {
          out.push({ title: `卷${numToCn(out.length + 1)}`, content: cur.join('\n') });
          cur = [];
        }
        out.push({ title: `卷${numToCn(out.length + 1)}`, content: cs.slice(i, i + CHUNK_CHARS).join('') });
      }
      n = 0;
      continue;
    }
    cur.push(p); n += len;
  }
  if (cur.length) out.push({ title: `卷${numToCn(out.length + 1)}`, content: cur.join('\n') });
  return out;
}

/** 尽量在句读处断：回看窗口内最后一个句末标点。 */
function cutAtSentence(cur) {
  const last = cur[cur.length - 1];
  if (last && [...last].length > CHUNK_LOOKBACK) {
    const cs = [...last];
    for (let i = cs.length - 1; i >= cs.length - CHUNK_LOOKBACK; i--) {
      if ('。！？；…'.includes(cs[i]) && i + 1 < cs.length) {
        cur[cur.length - 1] = cs.slice(i + 1).join('');
        return cur;
      }
    }
  }
  return cur;
}

// ---------------------------------------------------------------------------
// 5. Markdown 解析器：结构化转换
// ---------------------------------------------------------------------------

/** 行内标记 → 笺读标记：图片丢弃、链接留字、加粗转 [高亮]。 */
function mdInline(s) {
  let out = s;
  // 1) 图片整块去掉 ![alt](url)
  out = out.replace(/!\[[^\]]*\]\([^)]*\)/g, '');
  // 2) 链接只留文字 [text](url)
  out = out.replace(/\[([^\]]*)\]\([^)]*\)/g, '$1');
  // 3) 加粗 → 着重标记（必须在去星号之前）
  out = out.replace(/\*\*([^*]+)\*\*/g, `${HI_OPEN}$1${HI_CLOSE}`)
    .replace(/__([^_]+)__/g, `${HI_OPEN}$1${HI_CLOSE}`);
  // 4) 剩余强调 / 删除线 / 行内码：只留文字
  out = out.replace(/[*_~`]/g, '');
  return out.trim();
}

function parseMarkdown(md) {
  const norm = md.replace(/\r\n?/g, '\n');
  const chapters = [];
  let title = '';           // 空 = 还没遇到 `#`
  let paras = [];
  let quote = '';
  let inFence = false;

  const flushQuote = () => {
    if (!quote.trim()) { quote = ''; return; }
    const note = `${NOTE_OPEN}${quote.trim()}${NOTE_CLOSE}`;
    quote = '';
    if (paras.length) paras[paras.length - 1] += note; // 夹注本来就是插在正文里的
    else paras.push(note);
  };
  const pushChap = (allowEmpty) => {
    if (!paras.length && !allowEmpty) return;
    chapters.push({ title: title || '正文', content: paras.join('\n') });
    paras = [];
  };

  for (const raw of norm.split('\n')) {
    const t = raw.trim();
    // ---- 代码块：整段跳过 ----
    if (/^(```|~~~)/.test(t)) { inFence = !inFence; flushQuote(); continue; }
    if (inFence) continue;
    if (!t) { flushQuote(); continue; }
    // ---- 标题 ----
    const m = /^(#{1,6})\s+(.*)$/.exec(t);
    if (m) {
      flushQuote();
      // 已有标题 → 收章（`# 书名` 独占一章时留空章，供上层提升为书名）
      pushChap(title !== '');
      title = mdInline(m[2]);
      continue;
    }
    // ---- 分隔线 ----
    if (/^[-*_=]{3,}$/.test(t)) { flushQuote(); continue; }
    // ---- 引用：攒成一条夹注 ----
    if (t.startsWith('>')) {
      const r = mdInline(t.slice(1));
      if (r) quote += (quote ? '\n' : '') + r;
      continue;
    }
    flushQuote();
    // ---- 列表 / 表格：留下文字，丢掉标记 ----
    const body = mdInline(t.replace(/^\s*(?:[-*+]\s+|\d+[.)]\s+)/, ''));
    if (body) paras.push(body);
  }
  flushQuote();
  pushChap(title !== '');
  if (!chapters.length) chapters.push({ title: '正文', content: '' });
  return chapters.map((c) => ({ title: classicize(c.title), content: classicize(c.content) }));
}

// ---------------------------------------------------------------------------
// 6. HTML / XML 解析器：降噪与提取
// ---------------------------------------------------------------------------

const DROP_BLOCKS = ['script', 'style', 'head', 'nav', 'header', 'footer', 'aside',
  'form', 'svg', 'iframe', 'noscript', 'button', 'select', 'textarea'];
const VOID_TAGS = ['br', 'img', 'hr', 'meta', 'link', 'input', 'source', 'area', 'col', 'wbr'];
const BLOCK_TAGS = ['p', 'div', 'li', 'tr', 'section', 'article', 'td', 'dt', 'dd', 'pre', 'figcaption'];

/** HTML 实体还原。 */
function decodeEntities(s) {
  const named = {
    nbsp: ' ', amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", hellip: '…',
    mdash: '—', ndash: '–', ldquo: '“', rdquo: '”', lsquo: '‘', rsquo: '’',
    middot: '·', times: '×', deg: '°', copy: '©', reg: '®', sect: '§',
    para: '¶', laquo: '《', raquo: '》', yen: '¥', eacute: 'é', szlig: 'ß',
    emsp: '　', ensp: ' ', thinsp: ' ', zwj: '', zwnj: '',
  };
  return s.replace(/&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);/g, (m, body) => {
    if (body[0] === '#') {
      const n = body[1] === 'x' || body[1] === 'X'
        ? parseInt(body.slice(2), 16)
        : parseInt(body.slice(1), 10);
      return Number.isFinite(n) ? String.fromCodePoint(n) : m;
    }
    return named[body] ?? m;
  });
}

/** 标签名 → 笺读标记种类：em/i/cite/q/span.note 是注，strong/b/mark 是着重。 */
function tagKind(name, cls) {
  switch (name) {
    case 'em': case 'i': case 'cite': case 'q': case 'small': case 'sub': case 'sup':
      return '注';
    case 'strong': case 'b': case 'mark':
      return '高亮';
    case 'span': {
      const c = (cls || '').toLowerCase().split(/\s+/);
      if (c.some((x) => ['note', 'notes', 'annotation', 'comment', 'gloss', 'footnote'].includes(x))) return '注';
      if (c.some((x) => ['highlight', 'hl', 'emph', 'strong', 'mark'].includes(x))) return '高亮';
      return null;
    }
    default: return null;
  }
}

/**
 * HTML → 标准章数组。
 * 浏览器里优先用 DOMParser（真正按 DOM 树走，最稳）；Node / 无 DOM 环境
 * 退回内置标签扫描器（与 Rust 侧同一套状态机）。
 */
function parseHtml(html) {
  if (typeof DOMParser !== 'undefined') return parseHtmlByDom(html);
  return parseHtmlByScan(html);
}

/** 路径 A：DOMParser（浏览器）。 */
function parseHtmlByDom(html) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const body = doc.body || doc.documentElement;
  // 降噪：整块丢掉无用容器
  for (const tag of DROP_BLOCKS) {
    body.querySelectorAll(tag).forEach((el) => el.remove());
  }
  const chapters = [];
  let cur = { title: '正文', paras: [] };

  const flush = () => {
    if (cur.paras.length) chapters.push({ title: cur.title, content: cur.paras.join('\n') });
    cur = { title: '', paras: [] };
  };

  const walk = (node, note) => {
    for (const n of Array.from(node.childNodes)) {
      if (n.nodeType === 3) {                       // 文本
        const t = decodeEntities(n.nodeValue);
        if (t.trim()) cur.paras.push(t.replace(/\s+/g, ' ').trim());
        continue;
      }
      if (n.nodeType !== 1) continue;               // 只看元素
      const name = n.tagName.toLowerCase();
      if (DROP_BLOCKS.includes(name)) continue;
      if (/^h[1-6]$/.test(name)) {
        flush();
        cur.title = decodeEntities(n.textContent.trim());
        continue;
      }
      if (VOID_TAGS.includes(name)) {
        if (name === 'br' || name === 'hr') flush();
        continue;
      }
      if (BLOCK_TAGS.includes(name)) {
        const before = cur.paras.length;
        walk(n, note);
        if (cur.paras.length > before) flush();
        continue;
      }
      // 夹注 / 着重
      const kind = tagKind(name, n.getAttribute && n.getAttribute('class'));
      if (kind === '注') {
        const before = cur.paras.length;
        cur.paras.push(NOTE_OPEN);
        walk(n, '注');
        if (cur.paras.length === before) cur.paras.pop();
        else cur.paras[cur.paras.length - 1] += NOTE_CLOSE;
        continue;
      }
      if (kind === '高亮') {
        const before = cur.paras.length;
        cur.paras.push(HI_OPEN);
        walk(n, '高亮');
        if (cur.paras.length === before) cur.paras.pop();
        else cur.paras[cur.paras.length - 1] += HI_CLOSE;
        continue;
      }
      walk(n, note);
    }
  };

  walk(body, null);
  flush();
  const out = chapters.filter((c) => c.content.trim());
  if (!out.length) return parseTxt(decodeEntities(stripTags(html)));
  return out.map((c) => ({ title: classicize(c.title || '正文'), content: classicize(c.content) }));
}

/** 路径 B：标签扫描器（Node / 无 DOM）。与 Rust `parse_html` 同一套状态机。 */
function parseHtmlByScan(html) {
  let s = html;
  for (const tag of DROP_BLOCKS) s = removeBlock(s, tag);
  s = s.replace(/<!--[\s\S]*?-->/g, '');

  const chapters = [];
  let title = '正文', paras = [], buf = '';
  const stack = [];
  let heading = null, inQuote = 0;

  const flush = () => { if (buf.trim()) paras.push(buf.trim()); buf = ''; };
  const pushChap = () => {
    if (paras.length) chapters.push({ title, content: paras.join('\n') });
    paras = [];
  };
  // 栈里记「标签名 + 标记」：`</span>` 不带 class，得靠开标签时记下的那一笔
  const open = (name, kind) => {
    stack.push({ name, kind });
    if (kind) { const m = `[${kind}]`; if (heading) heading.t += m; else buf += m; }
  };
  const close = (kind) => {
    if (!kind) return;
    const m = `[/${kind}]`;
    if (heading) heading.t += m; else buf += m;
  };

  const re = /<(\/?)([a-zA-Z!?][\w:-]*)((?:"[^"]*"|'[^']*'|[^>"'])*?)(\/?)>/g;
  let last = 0, m;
  while ((m = re.exec(s))) {
    const text = decodeEntities(s.slice(last, m.index));
    if (text) { if (heading) heading.t += text; else buf += text.replace(/\s+/g, ' '); }
    last = re.lastIndex;

    const closing = m[1] === '/';
    const name = (m[2] || '').toLowerCase();
    const attrs = m[3] || '';
    if (!name || name.startsWith('!') || name.startsWith('?')) continue;

    if (closing) {
      if (/^h[1-6]$/.test(name)) {
        if (heading) {
          const h = heading.t.trim();
          heading = null;
          if (h) { flush(); pushChap(); title = h; }
        }
      } else {
        // 栈顶正是这个标签名 → 收口（闭合标签不带 class，按名字配对）
        if (stack.length && stack[stack.length - 1].name === name) {
          const top = stack.pop();
          if (name === 'blockquote') inQuote = Math.max(0, inQuote - 1);
          close(top.kind);
        }
        if (BLOCK_TAGS.includes(name)) flush();
      }
      continue;
    }
    if (VOID_TAGS.includes(name)) { if (name === 'br' || name === 'hr') flush(); continue; }
    if (/^h[1-6]$/.test(name)) { flush(); heading = { t: '' }; continue; }
    if (BLOCK_TAGS.includes(name)) { flush(); continue; }
    if (name === 'blockquote') {
      flush();
      if (inQuote === 0) open(name, '注');
      inQuote += 1;
      continue;
    }
    open(name, tagKind(name, classOf(attrs)));
  }
  const tail = decodeEntities(s.slice(last));
  if (tail) { if (heading) heading.t += tail; else buf += tail.replace(/\s+/g, ' '); }
  while (stack.length) close(stack.pop().kind);
  if (heading && heading.t.trim()) title = heading.t.trim();
  flush();
  pushChap();

  if (!chapters.length) return parseTxt(decodeEntities(stripTags(s)));
  return chapters.map((c) => ({ title: classicize(c.title), content: classicize(c.content) }));
}

function classOf(attrs) {
  const m = /class\s*=\s*("([^"]*)"|'([^']*)')/i.exec(attrs || '');
  return m ? (m[2] ?? m[3] ?? '') : '';
}

/** 整块删除 `<tag …>…</tag>`。 */
function removeBlock(s, tag) {
  const re = new RegExp(`<${tag}(?=[\\s/>])[\\s\\S]*?</${tag}\\s*>`, 'gi');
  return s.replace(re, '');
}

/** 兜底：抹掉所有标签。 */
function stripTags(s) {
  return s.replace(/<[^>]*>/g, '');
}

// ---------------------------------------------------------------------------
// 7. JSON（已是结构化的书稿）
// ---------------------------------------------------------------------------

function parseJson(text) {
  const v = JSON.parse(text);
  const arr = Array.isArray(v) ? v : (v.chapters || [v]);
  return arr.map((ch) => ({
    title: ch.title || '正文',
    content: typeof ch.content === 'string'
      ? ch.content
      : (ch.paragraphs || []).join('\n'),
  }));
}

// ---------------------------------------------------------------------------
// 8. 统一入口
// ---------------------------------------------------------------------------

/** 已知格式时直接解析字符串。 */
function parseStr(kind, text) {
  switch (kind) {
    case 'txt': return parseTxt(text);
    case 'markdown': return parseMarkdown(text);
    case 'html': return parseHtml(text);
    case 'json': return parseJson(text);
    case 'epub': return parseHtml(text); // 容器：先拆出 xhtml，再逐份走 html 分支
    default: return parseTxt(text);
  }
}

/**
 * 管线总入口：给一个 File / Blob，返回标准章数组。
 * @returns {Promise<Array<{title: string, content: string}>>}
 */
async function parseToJianDuFormat(file) {
  const buf = await file.arrayBuffer();
  const bytes = new Uint8Array(buf);
  const text = decodeBytes(bytes);
  const kind = sniffKind(file.name || '', text.slice(0, 4096));
  return parseStr(kind, text);
}

// Node 下顺手导出一个同步版本（给 CLI / 单测用）
function parseToJianDuFormatSync(name, bytes) {
  const text = decodeBytes(bytes);
  return parseStr(sniffKind(name, text.slice(0, 4096)), text);
}

// ---------------------------------------------------------------------------
// 导出：浏览器挂到全局 `JianDuPipeline`，Node 走 module.exports
// ---------------------------------------------------------------------------
const API = {
  decodeBytes, sniffKind, classicize, fullwidthPunct, normalizeNumbers,
  numToCn, numToCnDigits, isChapterHead,
  parseTxt, parseMarkdown, parseHtml, parseJson, parseStr, decodeEntities, stripTags, mdInline,
  parseToJianDuFormat, parseToJianDuFormatSync,
};
if (typeof globalThis !== 'undefined') globalThis.JianDuPipeline = API;
if (typeof module !== 'undefined' && module.exports) module.exports = API;
