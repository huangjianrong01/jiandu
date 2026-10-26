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

/// 收集全部脚注体：`id` → 纯文本（已解实体、去标签、压空白、清回注链接）。
///
/// v1.52.0（P0-2 升级）：支持**嵌套**脚注容器。真实 EPUB（Pandoc / 中华书局
/// 等）常用 `<div class="footnotes"><ol><li id="fn1">…</li></ol></div>`——
/// 旧版只认「自身带 footnote 类又带 id」的顶层块，遇到这种容器（容器无 id、
/// 注文在内部 `<li>` 上）会整块跳过、内层注文全丢。现改为：进入脚注容器后，
/// 凡内部**带 id 的块级元素**逐条收为一条注文（按各自 id 键入），无论嵌套
/// 几层。
///
/// 只收带 `id` 的元素——没有 id 就无法被 `href` 引用，留着也无用。
fn collect_footnotes(html: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let low = html.to_ascii_lowercase();
    // 栈：每层已开块是否为脚注容器，用于判定「此刻是否在脚注容器内」。
    let mut stack: Vec<bool> = Vec::new();
    let mut from = 0usize;
    let len = html.len();
    while from < len {
        let Some(k) = low[from..].find('<') else { break; };
        let at = from + k;
        let (tag, attrs, closing, self_close) = read_tag(&html[at..], &low[at..]);
        if tag.is_empty() {
            from = at + 1;
            continue;
        }
        if !BLOCK_CANDIDATES.contains(&tag.as_str()) || self_close {
            from = at + 1;
            continue;
        }
        let open_end = match low[at..].find('>') {
            Some(g) => at + g + 1,
            None => {
                from = at + 1;
                continue;
            }
        };
        if closing {
            stack.pop();
            from = at + 1;
            continue;
        }
        let is_fn = is_footnote_block(&attrs);
        let id = attr(&attrs, "id");
        if is_fn {
            // 脚注容器自身带 id：整块收为一条（罕见，但 Pandoc 偶尔如此）。
            if !id.is_empty() {
                if let Some(end) = matching_close(&low, at, &tag) {
                    let body = between_open_close(&html[at..end], &low[at..end]);
                    let text = strip_backlinks(&squeeze_spaces(&pipeline::decode_entities(
                        &pipeline::strip_tags(body),
                    )));
                    if !text.is_empty() {
                        map.entry(id).or_insert(text);
                    }
                }
            }
            stack.push(true);
            from = open_end;
            continue;
        }
        // 非脚注容器：处在脚注容器内且自身带 id → 收为一条注文，并整块跳过，
        // 避免其内文被重复扫到。
        if stack.iter().any(|b| *b) && !id.is_empty() {
            if let Some(end) = matching_close(&low, at, &tag) {
                let body = between_open_close(&html[at..end], &low[at..end]);
                let text = strip_backlinks(&squeeze_spaces(&pipeline::decode_entities(
                    &pipeline::strip_tags(body),
                )));
                if !text.is_empty() {
                    map.entry(id).or_insert(text);
                }
                from = end;
                continue;
            }
        }
        stack.push(false);
        from = open_end;
    }
    map
}

/// 清掉注文里的「回注链接」残渣：`↩`（U+21A9）`↑`（U+2191）`⇧` `↺` `↻` 等，
/// 以及它们被 `strip_tags` 留下的空壳；随后压一次空白。
fn strip_backlinks(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, '↩' | '↑' | '⇧' | '↺' | '↻'))
        .collect();
    squeeze_spaces(&cleaned)
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
/// v1.52.0（P0-2 升级）：注脚判定放宽——除了显式 `noteref` 类，只要 `<a>`
/// 的 `href="#id"` 命中一条**已收集的注文**，也当作注脚处理。这样中华书局等
/// 用纯 `<a href="#note1">[1]</a>`（不带 noteref 类）的 EPUB 也能把注文精准
/// 绑回正文，而不是把 `[1]` 当正文数字留下、注文整块被删。
///
/// 取不到注文（id 未收集 / 纯链接）时退回链内文字，至少不让「1」变成正文里
/// 一个孤零零的阿拉伯数字。
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
        let id = href_id(attrs);
        // 注脚：要么显式 noteref 类，要么 href 指向一条已收集的注文。
        let is_ref = is_noteref(attrs) || id.as_ref().is_some_and(|i| notes.contains_key(i));
        if !is_ref {
            cursor = at + g + 1;
            continue;
        }
        let Some(close) = low[at..].find("</a>") else {
            break;
        };
        let close_at = at + close;
        out.push_str(&s[rest_from..at]);
        let body = pipeline::strip_tags(&s[at + g + 1..close_at]);
        let text = match id.and_then(|i| notes.get(&i).cloned()) {
            Some(n) => n, // 注文已抽，直接用
            None => pipeline::decode_entities(body.trim()).to_string(), // 否则退回链内文字
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
///
/// 标记集合见 [`MARKERS`]（`[注]`/`[/注]`/`[高亮]`/`[/高亮]`/`[拼音:`/`[数字:`），
/// 这些是有语义的排版指令，清洗（全角标点、书名号、省略号替换……）绝不能
/// 碰它们的内部——否则 `[拼音:hàn]` 的冒号会被转成全角、断掉注音解析。
///
/// 提升为 `pub(crate)` 供 `parsers::cleanup_ancient_text` 复用，免得两套清洗
/// 各写一份「绕开标记」的逻辑。
pub(crate) fn map_outside_markers(s: &str, f: impl Fn(&str) -> String) -> String {
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

/// 是不是一个 CJK 统一表意文字（U+4E00–U+9FFF）。
///
/// 用于 P0-3 区分「汉字括注」与「拉丁拼音括注」：前者转 `[注]…[/注]`，
/// 后者转 `[拼音:…]`。
fn is_cjk_char(c: char) -> bool {
    let u = c as u32;
    (0x4E00..=0x9FFF).contains(&u)
}

/// 括注的开括号 → 对应的闭括号（`（）` `()` `［］` `[]` `【】` 五族）。
fn paren_close(c: char) -> Option<char> {
    match c {
        '（' => Some('）'),
        '(' => Some(')'),
        '[' => Some(']'),
        '［' => Some('］'),
        '【' => Some('】'),
        _ => None,
    }
}

/// 括注 → `[注]…[/注]`（**双行夹注**）；空括号直接删除。
///
/// 三条去路（v1.25.0 用户定的口径）：
/// * **空括号**（`（）` `()` `[]` `【】` 里一个字都没有）→ **整个删掉**：
///   它只是原书排版留下的空壳，留着就是两个空格子，竖排里格外刺眼；
/// * 有字、且不超 [`NOTE_MAX_CHARS`] → `[注]…[/注]`，引擎排双行小字；
/// * 过长的括注 / 套着别的标记 / 括号不配对 → 退竖排双弯引号 `﹁ ﹂`，
///   **绝不吞掉后文**（这是最容易出的「吃掉半页」事故）。
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
        let Some(close_ch) = paren_close(cs[i]) else {
            out.push(cs[i]);
            i += 1;
            continue;
        };
        let open_ch = cs[i];
        // 找配对的闭括号（容许同类嵌套：古籍括注里偶尔再套一个括注）。
        let mut depth = 0i32;
        let mut j = i;
        let mut close: Option<usize> = None;
        while j < cs.len() {
            if cs[j] == open_ch {
                depth += 1;
            } else if cs[j] == close_ch {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            j += 1;
        }
        match close {
            Some(k) => {
                let inner: String = cs[i + 1..k].iter().collect();
                let inner_t = inner.trim();
                if inner_t.is_empty() {
                    // 空括号：连壳一起丢掉。
                    i = k + 1;
                    continue;
                }
                let ok = inner_t.chars().count() <= NOTE_MAX_CHARS
                    && !inner_t.contains('[')
                    && !inner_t.chars().any(|c| paren_close(c).is_some());
                if ok {
                    // P0-3（v1.52.0）：括号里若是拉丁字母注音（拼音 / 罗马字），
                    // 转 `[拼音:…]` 保留，绝不当成乱码删掉；中文括注仍是双行夹注
                    // `[注]…[/注]`。判据：含 ASCII 字母且不含任何汉字。
                    let is_pinyin = inner_t.chars().any(|c| c.is_ascii_alphabetic())
                        && !inner_t.chars().any(is_cjk_char);
                    if is_pinyin {
                        out.push_str(&format!("[拼音:{}]", inner_t));
                    } else {
                        out.push_str("[注]");
                        out.push_str(inner_t);
                        out.push_str("[/注]");
                    }
                } else {
                    out.push('﹁');
                    out.push_str(&inner);
                    out.push('﹂');
                }
                i = k + 1;
            }
            None => {
                // 不配对：圆括号退竖排双弯引号；方括号原样留着（它可能
                // 是残缺的标记，改字反而更难查）。**绝不吞掉后文**。
                out.push(if open_ch == '（' || open_ch == '(' {
                    '﹁'
                } else {
                    open_ch
                });
                i += 1;
            }
        }
    }
    out
}

/// 现代破折号 → 竖排专用竖线 `︱`（U+FE31）。
///
/// 横排的 `——` 是一根**躺着**横穿字格的杠，竖排里它拦腰截断整列的行气
/// （用户原话：「横版破折号在竖排里太出戏」）。竖排该是**一根立着的线**：
/// 连续几道并成一枚（中文常写两道 `——`），一根足够，两道并排反成两道黑杠。
/// `︱` 是 Unicode 的竖排呈现形（U+FE31），字库里存的就是立着的那一枚。
///
/// 它**不进 `is_punct`**：句读（，。）由 `punct` 手绘小小一枚，而破折号要
/// 顶满字格才像刻本里的那条竖线，故按正文字排，只是字形换成立着的。
pub fn dash_to_vertical(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        if is_dash(cs[i]) {
            // 连续几道并成一枚。
            while i + 1 < cs.len() && is_dash(cs[i + 1]) {
                i += 1;
            }
            out.push('︱');
        } else {
            out.push(cs[i]);
        }
        i += 1;
    }
    out
}

/// 横排破折号一族：`—` `―` `–` `‐` `‑` 与全角 `－`。
fn is_dash(c: char) -> bool {
    matches!(c, '—' | '―' | '–' | '‐' | '‑' | '－')
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
            // 破折号一族（`——` / `—` / `–` / `－`…）→ 一枚竖排竖线。
            c if is_dash(c) => {
                while i + 1 < cs.len() && is_dash(cs[i + 1]) {
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

/// 阶段五：段落重构 —— 丢页码孤行 / 乱码行、刮孤立数字、压空白、
/// `{ title, content }` 出章。
///
/// 三道过滤的**顺序**是死的：
///
/// 1. 整行是不是页码（还看得见 ASCII 数字时判，最准）；
/// 2. 行内孤立数字（`20161` 这种夹在段中的扫描残渣）——**必须在数字转
///    汉字之前**，先转成「二〇一六一」就再也认不出了；
/// 3. 清完只剩标点 / 数字的乱码行。
pub fn stage5_chapters(chapters: Vec<RawChapter>) -> Vec<RawChapter> {
    let mut out: Vec<RawChapter> = Vec::new();
    for c in chapters {
        let mut lines: Vec<String> = Vec::new();
        for line in c.content.lines() {
            let raw = line.trim();
            if raw.is_empty() || is_page_number(raw) {
                continue;
            }
            if let Some(l) = clean_line(raw) {
                lines.push(l);
            }
        }
        if lines.is_empty() {
            continue;
        }
        // P0-1：诗词 / 硬回车防碎裂合并——连续短段（无终结标点、少于 10 字）
        // 先并成整段，再送引擎，杜绝「一列只有一个字」。
        crate::parsers::merge_poetry_lines(&mut lines);
        out.push(RawChapter::new(clean_text(&c.title), lines.join("\n")));
    }
    out
}

/// 一行正文的完整清洗：刮孤立数字 → 标点古籍化 → 数字转汉字 → 压空白。
///
/// 空行 / 乱码行返回 `None`（调用方直接丢）。
fn clean_line(raw: &str) -> Option<String> {
    let s = scrub_stray_digits(raw);
    let s = stage2_numbers(&stage3_punct(&s));
    let s = squeeze_spaces(&s);
    let s = s.trim().to_string();
    if s.is_empty() || is_junk_line(&s) {
        return None;
    }
    Some(s)
}

/// 一行「短文本」（章题 / 书名用）：不刮数字，只做标点与空白归一。
fn clean_text(raw: &str) -> String {
    squeeze_spaces(&stage2_numbers(&stage3_punct(raw)))
        .trim()
        .to_string()
}

/// 是不是一串「横排页码残留」：整行只有数字（可带分隔点 / 括号 / 「第…页」）。
///
/// 位数**不设上限**：PDF 转出来的页码常是 `20161`、`20162` 这种「年份前缀
/// + 页码」的五位串（v1.25.0 用户截图里的正是它），按「1~4 位」判会漏。
///
/// 整行只剩数字时，它绝不可能是正文——正文的数总归要挨着汉字。
fn is_page_number(line: &str) -> bool {
    let t = line.trim();
    let mut digits = 0usize;
    for c in t.chars() {
        if numcn::is_digit(c) {
            digits += 1;
        } else if !matches!(
            c,
            '·' | '-' | '—' | '.' | ' ' | '[' | ']' | '（' | '）' | '(' | ')' | '第' | '页' | '／'
                | '/'
        ) {
            return false;
        }
    }
    digits > 0
}

/// 只剩标点 / 数字的「乱码行」：`·`、`——`、`，，`、`[]`、`[〕`。
///
/// 逐字判定：只要出现任何一个汉字 / 西文字母，它就不是乱码行。
fn is_junk_line(s: &str) -> bool {
    let mut any = false;
    for c in s.chars() {
        if c.is_whitespace() || numcn::is_transparent(c) {
            continue;
        }
        if numcn::is_digit(c)
            || crate::types::is_punct(c)
            || matches!(
                c,
                '·' | '—' | '―' | '…' | '︙' | '︱' | '﹁' | '﹂' | '﹃' | '﹄'
                    | '(' | ')' | '[' | ']' | '{' | '}' | '【' | '】' | '「' | '」' | '『' | '』'
                    | '《' | '》' | '〈' | '〉'
            )
        {
            any = true;
            continue;
        }
        return false;
    }
    any
}

/// 剔除正文里**孤立**的阿拉伯数字（扫描残渣 / 行号 / 页眉角标）。
///
/// 「孤立」的判据只有一条：**数字串两侧都不挨汉字**。
///
/// * `20161` 独自立在段中 / 行末（两侧是空白或标点）→ 扫描页码，删；
/// * `· 7 ·`、`12`、`3.` → 同为页眉角标，删；
/// * `第3章`、`共15卷`、`1980年代` → 两侧有汉字（或数字本身粘着「年」），
///   是正文里的数，**留给 `stage2_numbers` 转汉字**。
///
/// 为什么必须先刮再转：`20161` 一旦被读成「二〇一六一」，下游就再也
/// 认不出它原本是页码了（这一条是踩过的坑）。
fn scrub_stray_digits(line: &str) -> String {
    let cs: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;
    while i < cs.len() {
        if let Some(end) = marker_end(&cs, i) {
            out.extend(&cs[i..end]);
            i = end;
            continue;
        }
        if !numcn::is_digit(cs[i]) {
            out.push(cs[i]);
            i += 1;
            continue;
        }
        let (len, _digits) = numcn::scan_run(&cs, i);
        if len == 0 {
            out.push(cs[i]);
            i += 1;
            continue;
        }
        let before = cs[..i]
            .iter()
            .rev()
            .find(|c| !c.is_whitespace() && !numcn::is_transparent(**c))
            .copied();
        let after = cs[i + len..]
            .iter()
            .find(|c| !c.is_whitespace() && !numcn::is_transparent(**c))
            .copied();
        let attached = before.map(crate::west::is_cjk).unwrap_or(false)
            || after.map(crate::west::is_cjk).unwrap_or(false);
        // 行尾孤零零的一串数字（后面只剩空白 / 标点）：扫描页码的典型位置，
        // 即便前面紧挨着汉字也删——`…许 20161` 里的 20161 绝不是正文。
        let at_tail = cs[i + len..]
            .iter()
            .all(|c| c.is_whitespace() || crate::types::is_punct(*c))
            // 带小数点的串不是页码（`圆周率3.14` 恰好也在行尾）。
            && !cs[i..i + len].iter().any(|c| matches!(c, '.' | '．'));
        if !attached || at_tail {
            // 孤立数字：整串删掉（含尾随的零宽字符）。
            i += len;
            continue;
        }
        out.extend(&cs[i..i + len]);
        i += len;
    }
    out
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
            // 两边都得有字才谈得上「中西之间补空格」；用 if let 拆包，不 unwrap。
            if let (Some(prev), Some(next)) = (prev, next) {
                if ideographic {
                    out.push('\u{3000}');
                } else if !crate::west::is_cjk(prev)
                    && !crate::west::is_cjk(next)
                    && !out.ends_with(' ')
                {
                    out.push(' ');
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
// 六、书级清洗（跨章：书眉 / 元数据）
// ------------------------------------------------------------------

/// 书眉短行的长度上限（字）：再长的「重复行」是正文（诗里的叠句），不动。
const HEAD_MAX_CHARS: usize = 30;
/// 「同一行重复多少次才算书眉」：OCR 的书眉几乎是每页一条。
const HEAD_MIN_REPEAT: usize = 3;

/// **书级清洗**：把「页眉 / 书口文字」从章题与正文里赶出去。
///
/// 电子书的每一页顶端都印着同一行书眉（`历代名家词集精华录卷十九`），
/// PDF/OCR 转 EPUB 时它被当成正文段落抓进来，于是竖排以后：
///
/// * 章题成了「历代名家词集精华录卷十九」——版心（章题列）里赫然一行
///   书眉，读者以为是书名，其实是每页都重复的那行废字；
/// * 正文里每隔几十行就冒出一遍同样的字，把行气割断。
///
/// 两道处理：
///
/// 1. **章题**：剥掉书名前缀与「卷十九」这类卷次尾巴（卷次由引擎自己按
///    章序生成，书里那一份一律不要）；
/// 2. **正文**：整段去标点后重复 ≥ [`HEAD_MIN_REPEAT`] 次、且形态像书眉
///    （以书名开头，或末尾挂「卷 + 数词」）的短段，整段删除。
///
/// 只删「重复 + 像书眉」两条同时成立的段，宁可漏删也不误伤正文
/// （诗里的叠句、经文的复句都可能重复出现，但它们不含书名、也不以卷次结尾）。
pub fn purge_running_headers(chapters: &mut Vec<crate::types::Chapter>, book_title: &str) {
    let book = clean_book_title(book_title);
    for c in chapters.iter_mut() {
        c.title = strip_head_noise(&c.title, &book);
    }
    // 统计短段出现次数（去标点、去空白后比对，OCR 的空格抖动不影响）。
    let mut freq: HashMap<String, usize> = HashMap::new();
    for c in chapters.iter() {
        for p in &c.paragraphs {
            let k = norm_key(&p.text);
            if !k.is_empty() && k.chars().count() <= HEAD_MAX_CHARS {
                *freq.entry(k).or_insert(0) += 1;
            }
        }
    }
    for c in chapters.iter_mut() {
        c.paragraphs.retain(|p| {
            let k = norm_key(&p.text);
            let n = freq.get(&k).copied().unwrap_or(0);
            // 两种都算书眉：① 重复 ≥3 次的短行；② **以书名开头的短行**
            // （书名不会是一句正文，只要开头撞上就够定罪——短书里书眉
            // 可能只出现一两次，靠重复次数抓不住）。
            let repeated = n >= HEAD_MIN_REPEAT;
            let titled = starts_with_book(&k, &book);
            !(looks_like_running_head(&k, &book) && (repeated || titled))
        });
    }
    chapters.retain(|c| !c.paragraphs.is_empty());
}

/// 书名清洗：去书名号、截掉书眉污染（`卷十九` / `（全二册）` / 页码）。
///
/// 版心只有一格宽，书名多一个字就挤掉一个卷次；而 `dc:title` 里常带着
/// 「历代名家词集精华录卷十九」这种整条书眉——从「卷」「册」「（」处截断。
pub fn clean_book_title(raw: &str) -> String {
    let t: String = raw
        .chars()
        .filter(|c| !matches!(c, '《' | '》' | '〈' | '〉' | '【' | '】' | '「' | '」'))
        .collect();
    let t = squeeze_spaces(&t);
    let cut = t
        .char_indices()
        .find(|(_, c)| matches!(c, '卷' | '册' | '冊' | '（' | '(' | '：' | ':' | '·'))
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    let head = t[..cut].trim().to_string();
    if head.chars().count() >= 2 {
        head
    } else {
        t.trim().to_string()
    }
}

/// 章题清洗：剥书名前缀 + 卷次尾巴；剥空了就留空（版心自有卷次）。
///
/// `…精华录卷十九　湘云遗稿` → `湘云遗稿`；`卷二` → 空。
fn strip_head_noise(title: &str, book: &str) -> String {
    let mut t = title.trim().to_string();
    if book.chars().count() >= 4 {
        t = t.replace(book, "");
    }
    let cs: Vec<char> = t.trim().chars().collect();
    // 剥「卷 + 数词」前缀（`卷十九　湘云遗稿`）：书眉总把卷次顶在题名前面。
    let mut i = 0usize;
    if i < cs.len() && matches!(cs[i], '卷' | '第' | '之') {
        i += 1;
    }
    let num_from = i;
    while i < cs.len() && (numcn::is_digit(cs[i]) || is_han_numeral(cs[i])) {
        i += 1;
    }
    if i > num_from {
        let mut j = i;
        while j < cs.len() && (cs[j].is_whitespace() || matches!(cs[j], '·' | '　')) {
            j += 1;
        }
        // 数词后**紧贴着文字**（`第一章　道可道`）——这不是卷次前缀，不动。
        let tight = j == i && j < cs.len();
        if !tight {
            let rest: String = cs[j..].iter().collect();
            let rest = rest.trim().trim_end_matches(['卷', '冊', '册']).trim();
            return rest.to_string();
        }
    }
    t.trim().to_string()
}

/// 去标点 / 空白后的比对键（OCR 的空格抖动不该影响「是不是同一行」）。
fn norm_key(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !crate::types::is_punct(*c))
        .filter(|c| !matches!(c, '[' | ']' | '﹁' | '﹂' | '︽' | '︾' | '︙' | '︱'))
        .collect()
}

/// 像不像书眉：以书名开头，或以「卷 + 数词」结尾。
fn looks_like_running_head(t: &str, book: &str) -> bool {
    if t.is_empty() {
        return false;
    }
    if starts_with_book(t, book) {
        return true;
    }
    ends_with_juan(t)
}

/// 这一行是不是「书名 + 卷次」的书眉（书名 ≥4 字才算数）。
///
/// 只看「以书名开头」会误伤——序跋里常有 `红楼梦第一回……` 这种正文；
/// 故要求书名之后**只剩一小截**，且其中带卷次（`卷十九` / `十二册`）
/// 或整截都是数词。
fn starts_with_book(t: &str, book: &str) -> bool {
    let bn = book.chars().count();
    if bn < 4 || !t.starts_with(book) {
        return false;
    }
    let rest: String = t.chars().skip(bn).collect();
    rest.is_empty()
        || (rest.chars().count() <= 12
            && (rest.contains('卷')
                || rest.contains('册')
                || rest.contains('冊')
                || rest
                    .chars()
                    .all(|c| is_han_numeral(c) || numcn::is_digit(c))))
}

/// 末尾是不是「卷 + 数词」（`…卷十九`、`…卷之三`、`…十二册`）。
fn ends_with_juan(t: &str) -> bool {
    let cs: Vec<char> = t.chars().collect();
    let mut i = cs.len();
    while i > 0 && (is_han_numeral(cs[i - 1]) || cs[i - 1] == '之') {
        i -= 1;
    }
    i < cs.len() && i > 0 && matches!(cs[i - 1], '卷' | '冊' | '册' | '集' | '部')
}

/// 汉字数词（含〇零两）。
fn is_han_numeral(c: char) -> bool {
    matches!(
        c,
        '〇' | '零'
            | '一'
            | '二'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '万'
            | '两'
            | '壹'
            | '贰'
            | '叁'
            | '肆'
            | '伍'
            | '陆'
            | '柒'
            | '捌'
            | '玖'
            | '拾'
    )
}

// ------------------------------------------------------------------
// 总装
// ------------------------------------------------------------------

/// 五阶段全跑：EPUB 的 xhtml 字节解出来的字符串 → 标准章数组。
///
/// 注意第一步不是降噪而是**抽注文**：脚注体装在 `<aside>` 里，降噪会把它们
/// 一并删掉，注号就只能退回一个光秃秃的数字了。
pub fn clean_epub(html: &str) -> Vec<RawChapter> {
    // `<sup>` 幽灵字先吃掉（见 `parsers::strip_sup_marks`）：导入走的是逐份
    // xhtml 直接调本函数，不经过 `UniversalParser`，这道闸必须装在这里。
    let html = crate::parsers::strip_sup_marks(html);
    let notes = collect_footnotes(&html);
    let s = stage1_denoise(&html);
    let s = stage4_marks_with(&s, &notes);
    let chapters = pipeline::parse_html_raw(&s);
    let chapters: Vec<RawChapter> = stage5_chapters(chapters)
        .into_iter()
        .map(|mut c| {
            // 独字夹注（`[注]已[/注]` / `[注]商[/注]`）也是上标残渣的一种：
            // 竖排里它是一个人孤零零占半列的杂质，连壳带字吃掉。
            c.content = crate::parsers::scrub_ghost_notes(&c.content);
            c
        })
        .collect();
    // 前言 / 目录 / 版权页 / 书影：正文起手之前的一律不进书架。
    crate::parsers::drop_front_matter(chapters)
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
        s = rewrite_attr(&s, "style", strip_style_colors);
    }
    if has_color_attr {
        s = rewrite_attr(&s, "color", |_| String::new());
        s = rewrite_attr(&s, "bgcolor", |_| String::new());
    }
    if has_class {
        s = rewrite_attr(&s, "class", strip_color_classes);
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
        // **空括号：连壳一起丢掉**（v1.25.0 用户指定的口径）。
        assert_eq!(stage3_punct("曰（）者"), "曰者");
        assert_eq!(stage3_punct("曰[]者"), "曰者");
        assert_eq!(stage3_punct("曰【】者"), "曰者");
        // 半角空括号同样命中（先全角化）。
        assert_eq!(stage3_punct("曰()者"), "曰者");
        // 超长括注：不转夹注，改竖排双弯引号。
        let long = "甲".repeat(NOTE_MAX_CHARS + 1);
        let out = stage3_punct(&format!("曰（{long}）者"));
        assert!(out.contains('﹁') && out.contains('﹂'), "{out}");
        // 不配对也绝不吞后文。
        let out = stage3_punct("曰（未闭合而后文很长");
        assert!(out.contains("未闭合而后文很长"), "{out}");
    }

    #[test]
    fn stage3_square_and_lenticular_brackets_become_notes() {
        // 方括号 / 六角括号里的字也是括注——一律转双行夹注，不留方括号。
        assert_eq!(stage3_punct("天[自然也]"), "天[注]自然也[/注]");
        assert_eq!(stage3_punct("天【自然也】"), "天[注]自然也[/注]");
        // 但**自己的标记**不能被当成括注再套一层。
        assert_eq!(stage3_punct("玄[拼音:xuán]"), "玄[拼音:xuán]");
        assert_eq!(stage3_punct("甲[注]乙[/注]丙"), "甲[注]乙[/注]丙");
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
        // v1.25.0：孤立的 `14428` 与孤行 `7` 都按「扫描页码」丢弃——
        // 正文的数必须挨着汉字（`共15卷` → `共十五卷`）才算数。
        assert!(!c.content.contains("一四四二八"), "孤立数字应删：{}", c.content);
        assert!(!c.content.contains("14428"), "{}", c.content);
        assert!(!c.content.lines().any(|l| l.trim() == "七"), "{}", c.content);
        assert!(c.content.contains('\n'), "段间应有换行：{}", c.content);
    }

    #[test]
    fn page_numbers_of_any_length_are_dropped() {
        // 用户截图里的 `20161` / `20162`：五位页码，按位数上限判会漏。
        assert!(is_page_number("20161"));
        assert!(is_page_number("20162"));
        assert!(is_page_number("· 3 ·"));
        assert!(is_page_number("第 12 页"));
        assert!(!is_page_number("十二"));
        assert!(!is_page_number("道可道"));

        let out = stage5_chapters(vec![RawChapter::new(
            "卷一",
            "见牡丹盛开有感\n20161\n又一首\n20162\n",
        )]);
        assert!(!out[0].content.contains("二〇一六一"), "{}", out[0].content);
        assert!(!out[0].content.contains("2016"), "{}", out[0].content);
        // 两位页码孤行已删；剩下的两条正文短句被 P0-1 诗词合并并成一行
        // （「一列只有一个字」碎裂防护），但正文一字未丢、页码彻底清除。
        assert!(out[0].content.contains("见牡丹盛开有感"), "{}", out[0].content);
        assert!(out[0].content.contains("又一首"), "{}", out[0].content);
        assert_eq!(out[0].content.lines().count(), 1, "{}", out[0].content);
    }

    #[test]
    fn stray_digits_are_scraped_only_when_isolated() {
        // 两侧都不挨汉字 → 扫描残渣，删。
        assert_eq!(scrub_stray_digits("见牡丹盛开 20161 "), "见牡丹盛开  ");
        assert_eq!(scrub_stray_digits("· 7 ·"), "·  ·");
        // 行尾的孤串（后面只剩标点空白）也删：扫描页码的典型位置。
        assert_eq!(scrub_stray_digits("空余许 20161，"), "空余许 ，");
        // 挨着汉字、又在句中 → 正文里的数，留给数字清洗。
        assert_eq!(scrub_stray_digits("第3章"), "第3章");
        assert_eq!(scrub_stray_digits("共15卷"), "共15卷");
        assert_eq!(scrub_stray_digits("全书1980页"), "全书1980页");
        assert_eq!(
            scrub_stray_digits("见牡丹盛开 20161 有感"),
            "见牡丹盛开 20161 有感",
            "句中且两侧有汉字，不敢动"
        );
        // 小数是整串（`3.14` 一起判），不拆。
        assert_eq!(scrub_stray_digits("圆周率3.14"), "圆周率3.14");
        // 已入标记的数字不动（`[数字:20161]` 是兜底产物，另由下游去壳）。
        assert_eq!(scrub_stray_digits("甲[数字:20161]乙"), "甲[数字:20161]乙");
    }

    #[test]
    fn junk_lines_are_dropped() {
        let out = stage5_chapters(vec![RawChapter::new("卷一", "正文一句\n·\n，，\n[]\n")]);
        assert_eq!(out[0].content, "正文一句");
    }

    #[test]
    fn running_heads_are_purged_book_wide() {
        use crate::types::{Chapter, Paragraph};
        let head = "历代名家词集精华录卷十九".to_string();
        let mut chapters = vec![
            Chapter {
                title: head.clone(),
                paragraphs: vec![
                    Paragraph::plain(head.clone()),
                    Paragraph::plain("燕燕于飞差池其羽"),
                    Paragraph::plain(head.clone()),
                ],
            },
            Chapter {
                title: format!("{head}　湘云遗稿"),
                paragraphs: vec![
                    Paragraph::plain(head.clone()),
                    Paragraph::plain("湘云遗稿二卷"),
                    Paragraph::plain("买陂塘旋栽杨柳"),
                ],
            },
            Chapter {
                title: "卷二".into(),
                paragraphs: vec![
                    Paragraph::plain(head.clone()),
                    Paragraph::plain("依稀淮岸江皋"),
                ],
            },
        ];
        purge_running_headers(&mut chapters, "历代名家词集精华录");
        // 每一条书眉都清干净了。
        for c in &chapters {
            assert!(
                !c.paragraphs.iter().any(|p| p.text.contains("精华录")),
                "书眉未清：{:?}",
                c.paragraphs
            );
        }
        // 章题剥掉书名与卷次。
        assert_eq!(chapters[0].title, "");
        assert_eq!(chapters[1].title, "湘云遗稿");
        assert_eq!(chapters[2].title, "");
        // 正文一句不丢。
        assert!(chapters.iter().any(|c| c
            .paragraphs
            .iter()
            .any(|p| p.text == "买陂塘旋栽杨柳")));
    }

    #[test]
    fn book_title_is_trimmed_at_the_juan() {
        assert_eq!(
            clean_book_title("《历代名家词集精华录》卷十九"),
            "历代名家词集精华录"
        );
        assert_eq!(clean_book_title("宋词三百首（全二册）"), "宋词三百首");
        // 短名不受影响。
        assert_eq!(clean_book_title("道德经"), "道德经");
    }
}
