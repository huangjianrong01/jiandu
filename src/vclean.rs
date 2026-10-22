//! **EPUB 竖排清洗与结构化预处理器**（五阶段，Data Pipeline）。
//!
//! 一句话：`clean_epub(xhtml) -> Vec<RawChapter>`，出去的是能直接喂给竖排引擎的
//! 干净结构 —— `{ title, content }`，`content` 里只允许出现三种标记：
//! `[注]…[/注]`（双行夹注）、`[高亮]…[/高亮]`（朱砂圈点）、`[拼音:…]`（格内注音）。
//!
//! 为什么需要它：EPUB 是**横排电子书**的产物，直接扔进竖排引擎会三处崩坏——
//!
//! | 病 | 横排世界 | 竖排世界 |
//! |---|---|---|
//! | `14428` | 一行读完 | 被拆成 1 / 4 / 4 / 2 / 8 五个字格竖着摞 |
//! | `《书名》` | 尖括号包书名 | ︽ ︾ 才是竖排书名号，横排的 `《》` 立在列里很怪 |
//! | `<sup>1</sup>` | 上标小号，无伤大雅 | 一行里插进两个小字，行气全断 |
//!
//! 五道工序（各自独立、可单测，顺序不可换）：
//!
//! 1. [`stage1_denoise`] —— **结构降噪**：剔 `script/style/nav/aside` 等整块、
//!    剔 `class` 含 AD 语汇的广告块、剥掉所有 `color` 声明（颜色归主题引擎管）；
//! 2. [`stage4_marks`] —— **标记提取**（先于数字，因为 `<rt>` 里的拼音带音调数字，
//!    晚做会被数字清洗误伤）：`<ruby><rt>` → `[拼音:…]`、`<a epub:type="noteref">`
//!    → `[注]注文[/注]`（注文按 `href="#id"` 从脚注体里取，取不到才退回注号）；
//! 3. `pipeline::parse_html_raw` —— **走标签树**出章与段（`h1~h6`、
//!    `epub:type="title"`、`class="title"` 都认作题署）；
//! 4. [`stage3_punct`] + [`stage2_numbers`] —— **逐行古籍化**：括号转夹注、
//!    `《》`→`︽︾`、`……`→`︙`、`——`→`︱`、数字转汉字（转不动的整串包进
//!    `[数字:…]` 交引擎成组横躺，绝不拆单字）；
//! 5. [`stage5_chapters`] —— **段落重构**：丢纯数字孤行（横排页码残留）、
//!    合并多余空格与软换行、段间 `\n`、输出 `{ title, content }`。
//!
//! 顺序上的两个坑（都是踩过的）：
//! * **数字清洗必须在「页码行过滤」之后** —— 先把 `12` 读成「十二」，
//!   下游就再也认不出这行原本是个页码了；
//! * **标点清洗必须在数字清洗之前** —— 括号转成 `[注]…[/注]` 之后，
//!   括注里的数字才跟着正文一起转，不会漏。

use crate::numcn;
use crate::pipeline::{self, RawChapter};
use std::collections::HashMap;

// ------------------------------------------------------------------
// 一、结构降噪
// ------------------------------------------------------------------

/// 整块连内容一起丢弃的容器（在 `pipeline::DROP_BLOCKS` 之外补几个多媒体壳）。
const DENOISE_BLOCKS: [&str; 19] = [
    "script", "style", "head", "nav", "footer", "form", "svg", "iframe", "noscript", "button",
    "select", "textarea", "object", "embed", "audio", "video", "canvas", "template", "map",
];

/// 广告 / 推广容器的 class / id 语汇（按词比对，不做子串包含，免得 `adult` 中招）。
const AD_TOKENS: [&str; 11] = [
    "ad",
    "ads",
    "adv",
    "advert",
    "adverts",
    "advertisement",
    "promo",
    "promotion",
    "banner",
    "sponsor",
    "dfp",
];

/// 会被当作「颜色」剥掉的 class 词（`class="red"` / `class="cRed"` 这类）。
const COLOR_CLASSES: [&str; 13] = [
    "red", "green", "blue", "yellow", "cyan", "magenta", "purple", "orange", "brown", "gray",
    "grey", "color", "colour",
];

/// 可整块删除的候选块级标签（只有它们才做深度配对查找）。
const BLOCK_CANDIDATES: [&str; 12] = [
    "div", "p", "section", "aside", "span", "a", "figure", "ul", "ol", "li", "table", "center",
];

/// 阶段一：HTML 结构降噪。
///
/// * 整块丢弃 `script / style / nav / footer / object / audio …`（连带内容）；
/// * 丢弃 HTML 注释（条件注释里常藏整段 IE 脚本）；
/// * 丢弃 `class` / `id` 命中 [`AD_TOKENS`] 的广告块（深度配对，嵌套 div 不误伤）；
/// * 剥掉所有 `color` 声明与颜色 class —— **颜色归主题引擎**，正文里嵌死
///   「红字赐」会让夜读主题下出现看不见的墨。
pub fn stage1_denoise(html: &str) -> String {
    let mut s = html.to_string();
    for tag in DENOISE_BLOCKS {
        s = pipeline::remove_block(&s, tag);
    }
    // `aside` 单列一步：脚注体也是 aside，但注文已在 stage4 之前抽走，
    // 这里的 aside 只剩「作者简介 / 版权声明」一类噪声。
    s = pipeline::remove_block(&s, "aside");
    s = strip_comments(&s);
    s = drop_blocks(&s, |_tag, attrs| is_ad_block(attrs));
    strip_color_attrs(&s)
}

/// 去 HTML 注释（`<!-- … -->`；未闭合则截断到末尾）。
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `class="ad"` / `id="advert-2"` 之类：按词比对（`ad`、`ads`、`promo` …）。
fn is_ad_block(attrs: &str) -> bool {
    let cls = attr(attrs, "class");
    let id = attr(attrs, "id");
    for v in [cls, id] {
        if v.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| AD_TOKENS.contains(&w))
        {
            return true;
        }
        // `class="ad-banner"` 已在上面按 `-` 切词命中；`class="adBanner"` 这种
        // 驼峰靠前缀兜一层。
        let low = v.to_ascii_lowercase();
        if low.starts_with("ad")
            && low.len() >= 2
            && low.as_bytes()[2..]
                .first()
                .map(|b| b.is_ascii_uppercase() || *b == b'_' || *b == b'-')
                .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// 从属性串里取一个值（小写化后按 `key=` 找，去引号）。
fn attr(attrs: &str, key: &str) -> String {
    let low = attrs.to_ascii_lowercase();
    let needle = format!("{key}=");
    let Some(i) = low.find(&needle) else {
        return String::new();
    };
    // 必须是属性名边界（`data-class=` 不算 `class=`）。
    if i > 0 {
        let prev = low.as_bytes()[i - 1];
        if !(prev.is_ascii_whitespace() || prev == b'"' || prev == b'\'') {
            return String::new();
        }
    }
    let rest = attrs[i + needle.len()..].trim_start();
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c.is_whitespace())
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

// ------------------------------------------------------------------
// 四、标记提取（放在数字清洗之前）
// ------------------------------------------------------------------

/// 阶段四：把「横排电子书才有的富文本」翻成竖排引擎认识的标记。
///
/// * `<ruby>汉<rt>hàn</rt></ruby>` → `汉[拼音:hàn]`（`<rp>` 兜底括号先剔掉）；
/// * `<a epub:type="noteref" href="#fn1">1</a>` → `[注]注文[/注]`
///   （注文按 `href` 去脚注体里取；取不到退回注号本身）；
/// * `<sup>1</sup>` / `<span class="note">` 由标签行走器的 `tag_kind` 转夹注，
///   这里不重复处理。
pub fn stage4_marks(html: &str) -> String {
    let notes = collect_footnotes(html);
    stage4_marks_with(html, &notes)
}

/// 同上，但注文表由调用方先备好。
///
/// 为什么需要这个入口：脚注体在 EPUB 里通常装在 `<aside>` 里，而
/// [`stage1_denoise`] 会把 `<aside>` 整块当噪声删掉。所以总装必须
/// **先抽注文、再降噪**，否则注号永远只能退回一个光秃秃的数字。
pub fn stage4_marks_with(html: &str, notes: &HashMap<String, String>) -> String {
    // 脚注体已抽进注文，正文里那份原件要删干净，否则会在篇末重复出现。
    let s = drop_blocks(html, |_tag, attrs| is_footnote_block(attrs));
    let s = pipeline::remove_block(&s, "rp");
    let s = ruby_to_pinyin(&s);
    noteref_to_note(&s, notes)
}

/// 脚注体：`epub:type="footnote"` / `role="doc-footnote"` / `class="footnote"`。
fn is_footnote_block(attrs: &str) -> bool {
    let et = attr(attrs, "epub:type").to_ascii_lowercase();
    if et.contains("footnote") || et.contains("endnote") || et.contains("rearnote") {
        return true;
    }
    if attr(attrs, "role").to_ascii_lowercase().contains("doc-footnote") {
        return true;
    }
    let cls = attr(attrs, "class").to_ascii_lowercase();
    cls.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| matches!(w, "footnote" | "footnotes" | "endnote" | "endnotes" | "fn"))
}

/// 收集全部脚注体：`id` → 纯文本（已解实体、去标签、压空白）。
///
/// 只认带 `id` 的块——没有 id 就没法被 `href` 引用，留着也无用。
fn collect_footnotes(html: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let low = html.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(k) = low[from..].find('<') {
        let at = from + k;
        let (tag, attrs, closing, self_close) = read_tag(&html[at..], &low[at..]);
        if closing || self_close {
            from = at + 1;
            continue;
        }
        if !BLOCK_CANDIDATES.contains(&tag.as_str()) || !is_footnote_block(&attrs) {
            from = at + 1;
            continue;
        }
        let id = attr(&attrs, "id");
        if id.is_empty() {
            from = at + 1;
            continue;
        }
        let Some(end) = matching_close(&low, at, &tag) else {
            from = at + 1;
            continue;
        };
        let body = between_open_close(&html[at..end], &low[at..end]);
        let text = squeeze_spaces(&pipeline::decode_entities(&pipeline::strip_tags(body)));
        if !text.is_empty() {
            map.entry(id).or_insert(text);
        }
        from = end;
    }
    map
}

/// `<ruby>` → 基字 + `[拼音:…]`。
///
/// 只处理**单层** ruby（EPUB 里 ruby 不嵌套）。基字里若还有行内标签
/// （`<ruby><span>汉</span><rt>hàn</rt></ruby>`），标签会被剥掉——注音
/// 只关心字与音，样式在竖排里本来也不保留。
fn ruby_to_pinyin(s: &str) -> String {
    let low = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut rest_from = 0usize;
    let mut cursor = 0usize;
    while let Some(k) = low[cursor..].find("<ruby") {
        let at = cursor + k;
        // 必须有标签名边界（`<rubyish>` 不算）。
        if !tag_boundary(&low[at + 5..]) {
            cursor = at + 5;
            continue;
        }
        let Some(close) = low[at..].find("</ruby>") else {
            break;
        };
        let close_at = at + close;
        out.push_str(&s[rest_from..at]);
        let open_end = match low[at..].find('>') {
            Some(g) => at + g + 1,
            None => break,
        };
        let inner = &s[open_end..close_at];
        let (base, py) = split_ruby_inner(inner);
        out.push_str(&base);
        if !py.is_empty() {
            out.push_str(&format!("[拼音:{py}]"));
        }
        rest_from = close_at + "</ruby>".len();
        cursor = rest_from;
    }
    out.push_str(&s[rest_from..]);
    out
}

/// 拆 ruby 内部：返回（基字纯文本, 拼音串）。
fn split_ruby_inner(inner: &str) -> (String, String) {
    let low = inner.to_ascii_lowercase();
    let mut base = String::new();
    let mut py = String::new();
    let mut i = 0usize;
    while i < inner.len() {
        let Some(k) = low[i..].find('<') else {
            base.push_str(&inner[i..]);
            break;
        };
        base.push_str(&inner[i..i + k]);
        let at = i + k;
        let (tag, _attrs, closing, _sc) = read_tag(&inner[at..], &low[at..]);
        let Some(g) = low[at..].find('>') else {
            break;
        };
        let body_from = at + g + 1;
        if tag == "rt" && !closing {
            if let Some(c) = low[body_from..].find("</rt>") {
                py.push_str(&pipeline::strip_tags(&inner[body_from..body_from + c]));
                i = body_from + c + "</rt>".len();
                continue;
            }
        }
        // `<rp>`（ruby 兜底括号）与其它行内标签：丢掉标签壳，内容照收
        // ——`<rp>` 的内容是「(」「)」，收进来会变成正文括号，故对 rp 单独跳过。
        if tag == "rp" {
            if closing {
                i = body_from;
                continue;
            }
            if let Some(c) = low[body_from..].find("</rp>") {
                i = body_from + c + "</rp>".len();
                continue;
            }
        }
        i = body_from;
    }
    (base, py.trim().to_string())
}

/// `<a …noteref…>1</a>` → `[注]注文[/注]`。
///
/// `notes` 是 `href="#id"` 的落点表；取不到就退回链内文字（注号），
/// 至少不让「1」变成正文里一个孤零零的阿拉伯数字。
fn noteref_to_note(s: &str, notes: &HashMap<String, String>) -> String {
    let low = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut rest_from = 0usize;
    let mut cursor = 0usize;
    while let Some(k) = low[cursor..].find("<a") {
        let at = cursor + k;
        if !tag_boundary(&low[at + 2..]) {
            cursor = at + 2;
            continue;
        }
        let Some(g) = low[at..].find('>') else {
            break;
        };
        let attrs = &low[at + 2..at + g];
        if !is_noteref(attrs) {
            cursor = at + g + 1;
            continue;
        }
        let Some(close) = low[at..].find("</a>") else {
            break;
        };
        let close_at = at + close;
        out.push_str(&s[rest_from..at]);
        let body = pipeline::strip_tags(&s[at + g + 1..close_at]);
        let text = match href_id(attrs).and_then(|id| notes.get(&id).cloned()) {
            Some(n) => n,
            None => pipeline::decode_entities(body.trim()),
        };
        if !text.trim().is_empty() {
            out.push_str(&format!("[注]{}[/注]", text.trim()));
        }
        rest_from = close_at + "</a>".len();
        cursor = rest_from;
    }
    out.push_str(&s[rest_from..]);
    out
}

fn is_noteref(attrs_lower: &str) -> bool {
    attrs_lower.contains("noteref")
        || attrs_lower.contains("doc-noteref")
        || attr(attrs_lower, "role").contains("doc-noteref")
}

/// `href="#fn1"` → `fn1`。
fn href_id(attrs: &str) -> Option<String> {
    let h = attr(attrs, "href");
    let h = h.trim();
    let id = h.strip_prefix('#')?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

// ------------------------------------------------------------------
// 三、标点古籍化
// ------------------------------------------------------------------

/// 夹注长度上限：超过就不再转夹注（几十字的括注排成双行小字会吃掉半列）。
const NOTE_MAX_CHARS: usize = 60;

/// 阶段三：现代标点 → 古籍竖排形态。
///
/// * 半角 `, . ? ! ; : ( )` → 全角（数字之间的 `.` `,` 不动，交给数字清洗）；
/// * `（…）` → `[注]…[/注]`（**双行夹注**）；括注过长 / 空 / 套着别的标记 /
///   括号不配对时，退回竖排专用双弯引号 `﹁ ﹂`；
/// * `《…》` → `︽…︾`（竖排书名号）；
/// * `……` / `…` → `︙`、`——` / `—` → `︱`（竖排省略线与破折号）；
/// * `“”` → `「」`（竖排引号，与渲染层 `bracket_to_quote` 的口径一致）。
pub fn stage3_punct(text: &str) -> String {
    // 全角化也必须绕开标记：`[拼音:xuán]` 里那个冒号一旦被转成全角 `：`，
    // 下游 `paragraph_of` 就认不出标记了（注音会原样印成方括号）。
    let s = map_outside_markers(text, pipeline::fullwidth_punct);
    let s = parens_to_note(&s);
    classic_punct(&s)
}

/// 只对「标记之外」的文本段跑 `f`，标记原样保留。
fn map_outside_markers(s: &str, f: impl Fn(&str) -> String) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut buf = String::new();
    let mut i = 0usize;
    while i < cs.len() {
        if let Some(end) = marker_end(&cs, i) {
            if !buf.is_empty() {
                out.push_str(&f(&buf));
                buf.clear();
            }
            out.extend(&cs[i..end]);
            i = end;
            continue;
        }
        buf.push(cs[i]);
        i += 1;
    }
    if !buf.is_empty() {
        out.push_str(&f(&buf));
    }
    out
}

/// `（…）` → `[注]…[/注]`；不合适时退 `﹁ … ﹂`。
fn parens_to_note(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        if let Some(end) = marker_end(&cs, i) {
            out.extend(&cs[i..end]);
            i = end;
            continue;
        }
        if cs[i] != '（' {
            out.push(cs[i]);
            i += 1;
            continue;
        }
        // 找配对的 `）`（容许一层嵌套：古籍括注里偶尔再套一个括注）。
        let mut depth = 0i32;
        let mut j = i;
        let mut close: Option<usize> = None;
        while j < cs.len() {
            match cs[j] {
                '（' => depth += 1,
                '）' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        match close {
            Some(k) => {
                let inner: String = cs[i + 1..k].iter().collect();
                let inner_t = inner.trim();
                let ok = !inner_t.is_empty()
                    && inner_t.chars().count() <= NOTE_MAX_CHARS
                    && !inner_t.contains('[')
                    && !inner_t.contains('（');
                if ok {
                    out.push_str("[注]");
                    out.push_str(inner_t);
                    out.push_str("[/注]");
                } else {
                    out.push('﹁');
                    out.push_str(&inner);
                    out.push('﹂');
                }
                i = k + 1;
            }
            None => {
                // 不配对：当单书名式引号用，绝不吞掉后文。
                out.push('﹁');
                i += 1;
            }
        }
    }
    out
}

/// `《》……——""` 的竖排替换（逐字扫，避免误伤已生成的标记）。
fn classic_punct(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        if let Some(end) = marker_end(&cs, i) {
            out.extend(&cs[i..end]);
            i = end;
            continue;
        }
        match cs[i] {
            '《' => out.push('︽'),
            '》' => out.push('︾'),
            '…' => {
                // `……` 与 `…` 都收成一个竖排省略号。
                while i + 1 < cs.len() && cs[i + 1] == '…' {
                    i += 1;
                }
                out.push('︙');
            }
            '—' | '―' => {
                while i + 1 < cs.len() && (cs[i + 1] == '—' || cs[i + 1] == '―') {
                    i += 1;
                }
                out.push('︱');
            }
            '“' => out.push('「'),
            '”' => out.push('」'),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

// ------------------------------------------------------------------
// 二、数字与西文清洗
// ------------------------------------------------------------------

/// 阶段二：连续数字转汉字；转不动的整串包进 `[数字:…]`。
///
/// 四种去路：
/// * **纪年 / 长串**（`223年`、`14428`）→ 逐字读（二二三年、一四四二八），
///   长度为四以内的一般数字走数值读法（十五、二百二十三）；
/// * **编号 / 与字母粘连 / 超长**（`ISBN 978-7`、`A4`、`19980203`）→
///   `[数字:978-7]`：**整串一个标记**，竖排引擎据此成组横躺，绝不拆成单字；
/// * **小数**（`3.14`）→ 三点一四；
/// * 已是汉字的数字天然不动（幂等）。
pub fn stage2_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let policy = numcn::NumPolicy::default();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        if let Some(end) = marker_end(&chars, i) {
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if !numcn::is_digit(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let (len, _digits) = numcn::scan_run(&chars, i);
        if len == 0 {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let raw: String = chars[i..i + len].iter().collect();
        let tail: String = chars[i + len..].iter().take(3).collect();
        let prev = chars[..i]
            .iter()
            .rev()
            .find(|c| !c.is_whitespace() && !numcn::is_transparent(**c))
            .copied();
        let next = chars[i + len..]
            .iter()
            .find(|c| !c.is_whitespace() && !numcn::is_transparent(**c))
            .copied();
        let digits = raw.chars().filter_map(numcn::ascii_digit).count();
        if numcn::should_skip(prev, next, digits) {
            // 兜底：包成「整串」标记。`[数字:` 本身是被保护的标记前缀，
            // 下游 stage3 已是最后一道，不会再被改写。
            out.push_str("[数字:");
            out.push_str(&raw);
            out.push(']');
        } else {
            out.push_str(&numcn::run_to_cn(&raw, &tail, &policy).cn);
        }
        i += len;
    }
    out
}

// ------------------------------------------------------------------
// 五、段落与章节重构
// ------------------------------------------------------------------

/// 阶段五：段落重构 —— 丢页码孤行、压空白、`{ title, content }` 出章。
///
/// 页码孤行（`1` `12` `· 3 ·` 这类）必须在数字转汉字**之前**认出来丢掉，
/// 所以它排在这一阶段的最后一步、却是整条链里最晚的一次「还看得见 ASCII」。
pub fn stage5_chapters(chapters: Vec<RawChapter>) -> Vec<RawChapter> {
    let mut out: Vec<RawChapter> = Vec::new();
    for c in chapters {
        let mut lines: Vec<String> = Vec::new();
        for line in c.content.lines() {
            let raw = line.trim();
            if raw.is_empty() || is_page_number(raw) {
                continue;
            }
            let l = stage2_numbers(&stage3_punct(raw));
            let l = squeeze_spaces(&l);
            if !l.trim().is_empty() {
                lines.push(l);
            }
        }
        if lines.is_empty() {
            continue;
        }
        out.push(RawChapter::new(
            squeeze_spaces(&stage2_numbers(&stage3_punct(&c.title))),
            lines.join("\n"),
        ));
    }
    out
}

/// 是不是一串「横排页码残留」：1~4 位纯数字（可带分隔点）。
fn is_page_number(line: &str) -> bool {
    let t = line.trim();
    let mut digits = 0usize;
    for c in t.chars() {
        if numcn::is_digit(c) {
            digits += 1;
        } else if !matches!(c, '·' | '-' | '—' | '.' | ' ') {
            return false;
        }
    }
    digits > 0 && digits <= 4
}

/// 压空白：把连续空白 / 软换行 / 零宽字符收成一个。
///
/// 两条判据分得很清：
/// * **半角空白**（空格 / Tab / 软换行）是横排排版的产物，两侧挨着汉字就整个
///   丢掉（竖排里没有「词间空格」，留着就是一个空字格）；
/// * **全角空格 `　`（U+3000）**是作者有意留的间隔（`第一章　道可道`），
///   原样保住一个——把它当噪声删掉，题名就跟章次黏成一串了。
fn squeeze_spaces(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() || numcn::is_transparent(c) {
            let mut j = i;
            let mut ideographic = false;
            while j < cs.len() && (cs[j].is_whitespace() || numcn::is_transparent(cs[j])) {
                if cs[j] == '\u{3000}' {
                    ideographic = true;
                }
                j += 1;
            }
            let prev = out.chars().last();
            let next = cs.get(j).copied();
            if prev.is_some() && next.is_some() {
                if ideographic {
                    out.push('\u{3000}');
                } else if !crate::west::is_cjk(prev.unwrap()) && !crate::west::is_cjk(next.unwrap()) {
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                }
            }
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

// ------------------------------------------------------------------
// 总装
// ------------------------------------------------------------------

/// 五阶段全跑：EPUB 的 xhtml 字节解出来的字符串 → 标准章数组。
///
/// 注意第一步不是降噪而是**抽注文**：脚注体装在 `<aside>` 里，降噪会把它们
/// 一并删掉，注号就只能退回一个光秃秃的数字了。
pub fn clean_epub(html: &str) -> Vec<RawChapter> {
    let notes = collect_footnotes(html);
    let s = stage1_denoise(html);
    let s = stage4_marks_with(&s, &notes);
    let chapters = pipeline::parse_html_raw(&s);
    stage5_chapters(chapters)
}

// ------------------------------------------------------------------
// HTML 层面的一把「小刀」：读标签 / 深度配对 / 条件删块
// ------------------------------------------------------------------

/// 从 `s` 的 `<` 处读一个标签：`(小写标签名, 原样属性串, 是否闭合, 是否自闭合)`。
fn read_tag(s: &str, low: &str) -> (String, String, bool, bool) {
    let body = low.strip_prefix('<').unwrap_or(low);
    let closing = body.starts_with('/');
    let body = body.trim_start_matches('/');
    let name_end = body
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(body.len());
    let name = body[..name_end].to_string();
    let gt = low.find('>').unwrap_or(low.len());
    // 属性取原文（保留大小写，值里可能有 `#C00`），起点按名字长度对齐。
    let start = (low.len() - body.len()).min(low.len());
    let attr_from = (start + name_end).min(s.len()).min(gt);
    let attrs = s[attr_from..gt.min(s.len())].trim().to_string();
    let self_close = low[..gt].trim_end().ends_with('/');
    (name, attrs, closing, self_close)
}

/// 标签名后面是不是一个「真边界」（`<ruby>` 命中、`<rubyish>` 不命中）。
fn tag_boundary(after: &str) -> bool {
    after
        .chars()
        .next()
        .map(|c| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(true)
}

/// 条件删块：命中 `hit(tag, attrs)` 的块级元素，连内容一起删（深度配对）。
fn drop_blocks<F>(html: &str, hit: F) -> String
where
    F: Fn(&str, &str) -> bool,
{
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    'again: loop {
        let low = rest.to_ascii_lowercase();
        let mut i = 0usize;
        while let Some(k) = low[i..].find('<') {
            let at = i + k;
            let (tag, attrs, closing, self_close) = read_tag(&rest[at..], &low[at..]);
            if !closing
                && !self_close
                && BLOCK_CANDIDATES.contains(&tag.as_str())
                && !attrs.is_empty()
                && hit(&tag, &attrs)
            {
                out.push_str(&rest[..at]);
                let end = matching_close(&low, at, &tag).unwrap_or(rest.len());
                rest = &rest[end..];
                continue 'again;
            }
            match low[at..].find('>') {
                Some(g) => i = at + g + 1,
                None => break,
            }
            if i >= low.len() {
                break;
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

/// 从 `start` 处的开标签起，找配对的闭合标签之后的位置（深度计数）。
fn matching_close(low: &str, start: usize, tag: &str) -> Option<usize> {
    let close = format!("</{tag}");
    let mut depth = 0i32;
    let mut i = start;
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 4096 {
            return None;
        }
        let o = find_tag_open(low, i, tag);
        let c = low[i..].find(&close).map(|k| i + k);
        match (o, c) {
            (None, None) => return None,
            (Some(oo), None) => {
                depth += 1;
                i = oo + 1;
            }
            (None, Some(cc)) => {
                depth -= 1;
                let after = cc + close.len();
                let gt = low[after..].find('>').map(|g| after + g + 1).unwrap_or(after);
                if depth <= 0 {
                    return Some(gt);
                }
                i = gt;
            }
            (Some(oo), Some(cc)) => {
                if oo < cc {
                    depth += 1;
                    i = oo + 1;
                } else {
                    depth -= 1;
                    let after = cc + close.len();
                    let gt = low[after..].find('>').map(|g| after + g + 1).unwrap_or(after);
                    if depth <= 0 {
                        return Some(gt);
                    }
                    i = gt;
                }
            }
        }
    }
}

/// 从 `from` 起找 `<tag`（带边界）。
fn find_tag_open(low: &str, from: usize, tag: &str) -> Option<usize> {
    let mut i = from;
    loop {
        let k = low[i..].find('<')?;
        let at = i + k;
        let after = &low[at + 1..];
        if after.starts_with(tag) && tag_boundary(&after[tag.len()..]) {
            return Some(at);
        }
        if after.starts_with('/') {
            i = at + 2;
            continue;
        }
        i = at + 1;
    }
}

/// 取一个元素块的内文：去掉首个开标签与末个闭合标签。
fn between_open_close<'a>(block: &'a str, block_low: &'a str) -> &'a str {
    let from = block_low.find('>').map(|g| g + 1).unwrap_or(0);
    let to = block_low.rfind("</").unwrap_or(block.len());
    if from >= to {
        return "";
    }
    &block[from..to]
}

// --- 颜色剥离 -------------------------------------------------------

/// 剥掉一切颜色痕迹：`style="color:red"`、`<font color="#c00">`、`class="red"`。
///
/// GPUI 没有「文字级颜色覆盖」这套东西，而竖排阅读器的墨色归主题引擎——
/// 正文里嵌死 `color:#c00`（红字「赐」）在夜读主题下就是一团看不见的墨。
fn strip_color_attrs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('>') else {
            out.push_str(&rest[i..]);
            return out;
        };
        out.push_str(&clean_tag(&rest[i..=i + j]));
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

/// 重建一个标签，去掉颜色属性；没有颜色痕迹时原样返回。
fn clean_tag(raw: &str) -> String {
    let low = raw.to_ascii_lowercase();
    let has_style = low.contains("style=");
    let has_color_attr = low.contains("color=") || low.contains("color =");
    let has_class = low.contains("class=");
    if !has_style && !has_color_attr && !has_class {
        return raw.to_string();
    }
    let mut s = raw.to_string();
    if has_style {
        s = rewrite_attr(&s, "style", |v| strip_style_colors(v));
    }
    if has_color_attr {
        s = rewrite_attr(&s, "color", |_| String::new());
        s = rewrite_attr(&s, "bgcolor", |_| String::new());
    }
    if has_class {
        s = rewrite_attr(&s, "class", |v| strip_color_classes(v));
    }
    // `<font>` 剥成空壳就没意义了（它唯一的作用就是上色）。
    if s.to_ascii_lowercase().starts_with("<font") && s.trim_end().ends_with('>') {
        let inner = s.trim_start_matches("<font").trim_end_matches('>').trim_end_matches('/');
        if inner.trim().is_empty() {
            return String::new();
        }
    }
    s
}

/// 重写标签里的一个属性值（`key="v"` / `key='v'` / `key=v`）；空值则整条删掉。
fn rewrite_attr(tag: &str, key: &str, f: impl Fn(&str) -> String) -> String {
    let low = tag.to_ascii_lowercase();
    let needle = format!("{key}=");
    let Some(i) = low.find(&needle) else {
        return tag.to_string();
    };
    // 属性名边界：`data-class=` 不算 `class=`。
    if i > 0 {
        let prev = low.as_bytes()[i - 1];
        if !(prev.is_ascii_whitespace() || prev == b'<' || prev == b'"' || prev == b'\'') {
            // 可能是 `xclass=`：换一个位置再找一次。
            let next = low[i + needle.len()..]
                .find(&needle)
                .map(|k| i + needle.len() + k);
            match next {
                Some(j) => return rewrite_attr_at(tag, j, needle.len(), key, &f),
                None => return tag.to_string(),
            }
        }
    }
    rewrite_attr_at(tag, i, needle.len(), key, &f)
}

fn rewrite_attr_at(
    tag: &str,
    at: usize,
    needle_len: usize,
    _key: &str,
    f: &impl Fn(&str) -> String,
) -> String {
    let rest = &tag[at + needle_len..];
    let (value, value_span, tail_from) = if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"').map(|k| k + 1).unwrap_or(stripped.len());
        (&rest[1..end], (at + needle_len, at + needle_len + end + 1), at + needle_len + end + 1)
    } else if let Some(stripped) = rest.strip_prefix('\'') {
        let end = stripped.find('\'').map(|k| k + 1).unwrap_or(stripped.len());
        (&rest[1..end], (at + needle_len, at + needle_len + end + 1), at + needle_len + end + 1)
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
        (&rest[..end], (at + needle_len, at + needle_len + end), at + needle_len + end)
    };
    let new = f(value);
    let mut out = String::with_capacity(tag.len());
    out.push_str(&tag[..at]);
    if !new.trim().is_empty() {
        out.push_str(&format!("{}=\"{}\"", &tag[at..at + needle_len - 1], new));
    } else {
        // 整条属性删掉：顺手吃掉前导空白，别留下双空格。
        while out.ends_with(' ') {
            out.pop();
        }
    }
    let _ = value_span;
    out.push_str(&tag[tail_from..]);
    out
}

/// 去掉 `color:` / `background-color:` 声明，其余样式保留。
fn strip_style_colors(style: &str) -> String {
    style
        .split(';')
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
        .filter(|d| {
            let low = d.to_ascii_lowercase();
            !(low.starts_with("color") || low.starts_with("background-color"))
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// 去掉颜色类 class 词（`red` / `cRed` / `text-red`）。
fn strip_color_classes(v: &str) -> String {
    v.split_whitespace()
        .filter(|w| {
            let low = w.to_ascii_lowercase();
            let bare = low
                .trim_start_matches("text-")
                .trim_start_matches("font-")
                .trim_start_matches("c")
                .trim_start_matches('-');
            !COLOR_CLASSES.contains(&bare) && !COLOR_CLASSES.contains(&low.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ------------------------------------------------------------------
// 标记保护：数字 / 标点清洗一律跳过方括号标记内部
// ------------------------------------------------------------------

/// 受保护的标记前缀。`[注]` / `[高亮]` 只是三五个字，本身不含数字，
/// 保护它是为了不让 `[` `]` 被后续规则当普通标点处理。
const MARKERS: [&str; 6] = ["[注]", "[/注]", "[高亮]", "[/高亮]", "[拼音:", "[数字:"];

/// 若 `i` 处是一个标记的开头，返回它（含内容）之后的字符下标。
fn marker_end(chars: &[char], i: usize) -> Option<usize> {
    if chars.get(i) != Some(&'[') {
        return None;
    }
    for m in MARKERS {
        let mc: Vec<char> = m.chars().collect();
        if chars.len() < i + mc.len() || chars[i..i + mc.len()] != mc[..] {
            continue;
        }
        if m.ends_with(':') {
            return (i + mc.len()..chars.len())
                .find(|&k| chars[k] == ']')
                .map(|k| k + 1);
        }
        return Some(i + mc.len());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 阶段一：结构降噪 ────────────────────────────────────────────

    #[test]
    fn stage1_kills_scripts_ads_and_colors() {
        let html = r##"<html><head><style>p{color:red}</style></head><body>
            <div class="ad-banner">广告位</div>
            <div class="content"><p>正文<span style="color:#c00">赐</span>也</p></div>
            <script>alert(1)</script></body></html>"##;
        let s = stage1_denoise(html);
        assert!(!s.contains("广告位"), "广告块未删：{s}");
        assert!(!s.contains("alert"), "script 未删：{s}");
        assert!(!s.contains("color:#c00"), "颜色声明未剥：{s}");
        assert!(!s.contains("p{color"), "style 块未删：{s}");
        assert!(s.contains("赐"), "正文不得丢：{s}");
        assert!(s.contains("正文"), "正文不得丢：{s}");
    }

    #[test]
    fn stage1_keeps_words_that_only_look_like_ads() {
        // `adult` / `shadow` 不该被当成广告块。
        let html = r##"<div class="adult">正文甲</div><div class="shadow">正文乙</div>"##;
        let s = stage1_denoise(html);
        assert!(s.contains("正文甲"), "{s}");
        assert!(s.contains("正文乙"), "{s}");
    }

    #[test]
    fn stage1_strips_color_classes_but_keeps_semantic_ones() {
        let html = r##"<span class="red note">夹注</span>"##;
        let out = strip_color_attrs(html);
        assert!(!out.contains("red"), "{out}");
        assert!(out.contains("note"), "{out}");
    }

    // ── 阶段四：标记提取 ────────────────────────────────────────────

    #[test]
    fn stage4_ruby_becomes_pinyin_mark() {
        let html = "<p><ruby>汉<rp>(</rp><rt>hàn</rt><rp>)</rp></ruby>字</p>";
        let s = stage4_marks(html);
        assert!(s.contains("汉[拼音:hàn]"), "{s}");
        assert!(!s.contains("rp>"), "rp 兜底括号应清掉：{s}");
        assert!(!s.contains('('), "括号不得留在正文；{s}");
    }

    #[test]
    fn stage4_noteref_pulls_footnote_body() {
        let html = r##"<p>道可道<a epub:type="noteref" href="#fn1">1</a>。</p>
            <aside epub:type="footnote" id="fn1"><p>王弼曰：可道之道，非常道。</p></aside>"##;
        let s = stage4_marks(html);
        assert!(s.contains("[注]王弼曰：可道之道，非常道。[/注]"), "{s}");
        assert!(!s.contains("footnote"), "脚注体应删净：{s}");
    }

    #[test]
    fn stage4_noteref_without_body_falls_back_to_number() {
        let html = r##"<p>天<a epub:type="noteref" href="#missing">2</a></p>"##;
        let s = stage4_marks(html);
        assert!(s.contains("[注]2[/注]"), "{s}");
    }

    // ── 阶段二：数字 ────────────────────────────────────────────────

    #[test]
    fn stage2_reads_numbers_as_han() {
        assert_eq!(stage2_numbers("公元223年"), "公元二二三年");
        assert_eq!(stage2_numbers("共15卷"), "共十五卷");
        assert_eq!(stage2_numbers("全书1980页"), "全书一千九百八十页");
        assert_eq!(stage2_numbers("圆周率3.14"), "圆周率三点一四");
    }

    #[test]
    fn stage2_wraps_unconvertible_runs_as_one_mark() {
        // 与字母粘连 / 超长：不转汉字，但必须整串包成**一个**标记。
        assert_eq!(stage2_numbers("ISBN 978-7-02"), "ISBN [数字:978-7-02]");
        assert_eq!(stage2_numbers("19980203"), "[数字:19980203]");
        // 五位长串（14428）：转汉字（逐字读），不进兜底。
        assert_eq!(stage2_numbers("14428"), "一四四二八");
    }

    #[test]
    fn stage2_leaves_pinyin_and_note_markers_alone() {
        assert_eq!(stage2_numbers("长[拼音:cháng]安"), "长[拼音:cháng]安");
        assert_eq!(stage2_numbers("[注]洪武3年[/注]"), "[注]洪武三年[/注]");
    }

    // ── 阶段三：标点 ────────────────────────────────────────────────

    #[test]
    fn stage3_parens_become_inline_note() {
        assert_eq!(
            stage3_punct("道可道（可以说出的道）非常道"),
            "道可道[注]可以说出的道[/注]非常道"
        );
        // 半角括号同样命中（先全角化）。
        assert_eq!(stage3_punct("天(自然也)"), "天[注]自然也[/注]");
    }

    #[test]
    fn stage3_unfit_parens_fall_back_to_corner_quotes() {
        // 空括注 / 超长括注：不转夹注，改竖排双弯引号。
        assert_eq!(stage3_punct("曰（）者"), "曰﹁﹂者");
        let long = "甲".repeat(NOTE_MAX_CHARS + 1);
        let out = stage3_punct(&format!("曰（{long}）者"));
        assert!(out.contains('﹁') && out.contains('﹂'), "{out}");
        // 不配对也绝不吞后文。
        let out = stage3_punct("曰（未闭合而后文很长");
        assert!(out.contains("未闭合而后文很长"), "{out}");
    }

    #[test]
    fn stage3_book_title_and_leaders() {
        assert_eq!(stage3_punct("《道德经》"), "︽道德经︾");
        assert_eq!(stage3_punct("曰……曰"), "曰︙曰");
        assert_eq!(stage3_punct("甲——乙"), "甲︱乙");
        assert_eq!(stage3_punct("曰“道”"), "曰「道」");
    }

    // ── 阶段五：段落重构 ────────────────────────────────────────────

    #[test]
    fn stage5_drops_page_numbers_and_squeezes_spaces() {
        let chapters = vec![RawChapter::new("卷一", "12\n道可  道也\n· 3 ·")];
        let out = stage5_chapters(chapters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "道可道也");
    }

    #[test]
    fn stage5_keeps_ideographic_space_in_titles() {
        // 全角空格是作者留的间隔（章次与题名之间），不能当噪声删；
        // 半角空格挨着汉字才是排版噪声。
        let out = stage5_chapters(vec![RawChapter::new("第一章　道可道", "道 可 道")]);
        assert_eq!(out[0].title, "第一章　道可道");
        assert_eq!(out[0].content, "道可道");
    }

    // ── 总装 ───────────────────────────────────────────────────────

    #[test]
    fn clean_epub_end_to_end() {
        let html = r##"<?xml version="1.0"?><html><head><title>道德经</title>
            <style>p{color:#c00}</style></head><body>
            <div class="ad">买书立减</div>
            <div epub:type="title">第一章　道可道</div>
            <p>道可道（可以说出的道），非常道。</p>
            <p><ruby>玄<rt>xuán</rt></ruby>之又玄，众妙之门。</p>
            <p>公元223年，《道德经》<a epub:type="noteref" href="#fn1">1</a>成书。</p>
            <p>14428</p>
            <p>7</p>
            <aside epub:type="footnote" id="fn1"><p>此说出自史记。</p></aside>
            </body></html>"##;
        let cs = clean_epub(html);
        assert_eq!(cs.len(), 1, "应只剩一章：{cs:?}");
        let c = &cs[0];
        assert_eq!(c.title, "第一章　道可道");
        assert!(!c.content.contains("买书立减"), "广告未删");
        assert!(c.content.contains("[注]可以说出的道[/注]"), "{}", c.content);
        assert!(c.content.contains("玄[拼音:xuán]"), "{}", c.content);
        assert!(c.content.contains("二二三年"), "{}", c.content);
        assert!(c.content.contains("︽道德经︾"), "{}", c.content);
        assert!(c.content.contains("[注]此说出自史记。[/注]"), "{}", c.content);
        // 14428 已转汉字，孤行 7 被当页码丢掉。
        assert!(c.content.contains("一四四二八"), "{}", c.content);
        assert!(!c.content.lines().any(|l| l.trim() == "七"), "{}", c.content);
        assert!(c.content.contains('\n'), "段间应有换行：{}", c.content);
    }
}
