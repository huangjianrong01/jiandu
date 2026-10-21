//! 统一的**文件导入与古籍化清洗管线**（Data Pipeline）。
//!
//! 一句话：`parse_to_jiandu_format(name, bytes) -> Vec<RawChapter>`。
//! 不管进来的是 txt / md / html / xml / json，出去的都是同一份干净结构：
//!
//! ```text
//! [ { title: "卷一", content: "正文[注]夹注[/注]正文[高亮]着重[/高亮]" } ]
//! ```
//!
//! 四道工序，各自独立、可单测：
//!   1. **解码**  —— UTF-8 / UTF-16（BOM）/ GBK 自动判别，乱码不留到下游；
//!   2. **分章**  —— txt 认「第 X 章/回/卷/节」，无标识且超长按 3000 字切；
//!   3. **降噪**  —— md 去代码块与图片、html 去 script/style/nav，只留正文骨架；
//!   4. **古籍化**——半角标点转全角、引用 / em 转双行夹注 `[注]…[/注]`、
//!                 加粗 / strong 转着重 `[高亮]…[/高亮]`、阿拉伯数字转汉字。
//!
//! 最后 `chapters_to_book` 把 `[高亮]` 摘成 `Note`（`NoteKind::Emph`，渲染为
//! 字右的朱砂圈点），`[注]` 留给排版引擎在布局时展开成双行夹注。

use crate::types::{Book, Chapter, Note, NoteKind, Paragraph};
use anyhow::{anyhow, Result};

/// 无章节标识时的强制切分字数。
pub const CHUNK_CHARS: usize = 3000;

/// 切分时允许回退找句读的窗口（尽量在句号处断，不把一个句子劈成两半）。
const CHUNK_LOOKBACK: usize = 240;

/// 管线输出的标准章：`title` + `content`，段落之间以 `\n` 分隔。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawChapter {
    pub title: String,
    /// 正文（可含 `[注]…[/注]`、`[高亮]…[/高亮]`）。
    pub content: String,
}

impl RawChapter {
    pub fn new(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
        }
    }

    /// 空章（只有标题没有正文）：markdown 的一级书名章会是这样。
    pub fn is_empty_body(&self) -> bool {
        self.content.trim().is_empty()
    }
}

/// 源头格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SrcKind {
    Txt,
    Markdown,
    Html,
    Json,
    Epub,
}

impl SrcKind {
    pub fn label(self) -> &'static str {
        match self {
            SrcKind::Txt => "txt",
            SrcKind::Markdown => "md",
            SrcKind::Html => "html",
            SrcKind::Json => "json",
            SrcKind::Epub => "epub",
        }
    }
}

/// 按扩展名 + 内容嗅探判断格式；认不出来就退回 txt（纯文本最兜底）。
pub fn sniff_kind(name: &str, head: &str) -> SrcKind {
    let lower = name.to_ascii_lowercase();
    if let Some(ext) = lower.rsplit('.').next() {
        match ext {
            "md" | "markdown" | "mdown" => return SrcKind::Markdown,
            "html" | "htm" | "xhtml" | "xml" | "opf" | "xht" => return SrcKind::Html,
            "json" => return SrcKind::Json,
            "epub" => return SrcKind::Epub,
            "txt" | "text" => return SrcKind::Txt,
            _ => {}
        }
    }
    // 大小写不敏感：`<!DOCTYPE HTML>` 也是 HTML。
    let t = head.trim_start().to_ascii_lowercase();
    if t.starts_with("<!doctype html") || t.starts_with("<html") || t.starts_with("<?xml") {
        return SrcKind::Html;
    }
    if t.starts_with('{') || t.starts_with('[') {
        return SrcKind::Json;
    }
    if t.starts_with('#') || t.contains("\n# ") || t.contains("```") || t.contains("](") {
        return SrcKind::Markdown;
    }
    SrcKind::Txt
}

// ------------------------------------------------------------------
// 一、解码：UTF-8 / UTF-16 / GBK
// ------------------------------------------------------------------

/// 把任意编码的字节解码成字符串：BOM 优先，其次 UTF-8 合法性，最后退回 GBK。
///
/// 中文世界的老书大多是 GBK（网上下载的 txt 尤甚），读错码会整页方块，
/// 所以这里不能只看 `from_utf8` 成功与否就完事——但也不能一上来就按 GBK
/// 解（那会把 UTF-8 的中文解成「锟斤拷」）。顺序是：BOM → UTF-8 → GBK。
pub fn decode_bytes(bytes: &[u8]) -> String {
    // UTF-8 BOM
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    // UTF-16 LE / BE BOM
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| {
                if bytes[0] == 0xFF {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        return String::from_utf16_lossy(&u16s);
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // 不是合法 UTF-8：按 GBK 解（encoding_rs 与浏览器同一套码表）。
    let (cow, _enc, _had_errors) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}

// ------------------------------------------------------------------
// 二、古籍化清洗
// ------------------------------------------------------------------

fn is_ascii_digit(c: char) -> bool {
    c.is_ascii_digit()
}

/// 半角标点 → 全角（`，。？！；：（）`）。
///
/// 两处留手：数字里的 `.`（小数 `3.14`）与 `,`（千分位 `1,980`）**不转**，
/// 否则 `3。14` 这种断句妖风会直接毁掉数字串（数字随后由 `numcn` 转汉字）。
pub fn fullwidth_punct(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, c) in cs.iter().copied().enumerate() {
        let prev = if i > 0 { Some(cs[i - 1]) } else { None };
        let next = cs.get(i + 1).copied();
        let in_number =
            prev.map(is_ascii_digit).unwrap_or(false) && next.map(is_ascii_digit).unwrap_or(false);
        let mapped = match c {
            ',' if !in_number => '，',
            '.' if !in_number => '。',
            '?' => '？',
            '!' => '！',
            ';' => '；',
            ':' => '：',
            '(' => '（',
            ')' => '）',
            other => other,
        };
        out.push(mapped);
    }
    out
}

/// 一道「古籍化」：全角标点 + 阿拉伯数字转汉字。
pub fn classicize(s: &str) -> String {
    crate::numcn::normalize_numbers(&fullwidth_punct(s))
}

/// 章序号的中文写法（`1 -> 一`、`15 -> 十五`），切卷时用。
pub fn cn_ordinal(n: usize) -> String {
    crate::numcn::num_to_cn(n as u64)
}

// ------------------------------------------------------------------
// 三、txt：智能切章
// ------------------------------------------------------------------

/// 是否章节标题行：`第` + 若干中文数字 / 阿拉伯数字 + `章回卷节篇折出`。
///
/// 用字符扫描而不是正则——`[0-9]` 与全角数字、`两`、`廿` 都得认，
/// 而且标题后面常跟整句标题名（「第一章 宴桃园豪杰三结义」）。
pub fn is_chapter_head(line: &str) -> bool {
    let cs: Vec<char> = line.trim().chars().collect();
    if cs.len() < 3 || cs.len() > 40 {
        return false;
    }
    if cs[0] != '第' {
        return false;
    }
    let mut i = 1usize;
    while i < cs.len() && is_numeral(cs[i]) {
        i += 1;
    }
    if i == 1 {
        return false;
    }
    matches!(cs[i], '章' | '回' | '卷' | '节' | '篇' | '折' | '出')
}

fn is_numeral(c: char) -> bool {
    matches!(
        c,
        '零' | '〇'
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
            | '亿'
            | '两'
            | '廿'
            | '卅'
    ) || c.is_ascii_digit()
        || ('０'..='９').contains(&c)
}

/// txt → 标准章数组。
pub fn parse_txt(text: &str) -> Vec<RawChapter> {
    let norm = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut chapters: Vec<RawChapter> = Vec::new();
    let mut title = "正文".to_string();
    let mut paras: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut saw_head = false;

    let flush = |paras: &mut Vec<String>, buf: &mut String| {
        // 中文 txt 常硬折行：同一段的多行直接接起来，空行才分段。
        if !buf.trim().is_empty() {
            paras.push(buf.trim().to_string());
        }
        buf.clear();
    };

    for raw in norm.lines() {
        let line = raw.trim();
        if line.is_empty() {
            flush(&mut paras, &mut buf);
            continue;
        }
        if is_chapter_head(line) {
            flush(&mut paras, &mut buf);
            push_chapter(&mut chapters, &title, &mut paras);
            title = line.to_string();
            saw_head = true;
            continue;
        }
        buf.push_str(line);
    }
    flush(&mut paras, &mut buf);
    push_chapter(&mut chapters, &title, &mut paras);

    if chapters.is_empty() {
        chapters.push(RawChapter::new("正文", String::new()));
    }
    // 通篇没有章节标识 + 字数过长：按 3000 字强制切卷，否则一章几千字
    // 在竖排里要翻到手酸。
    if !saw_head {
        let total: usize = chapters.iter().map(|c| c.content.chars().count()).sum();
        if total > CHUNK_CHARS {
            chapters = rechunk(&chapters);
        }
    }
    chapters.iter_mut().for_each(|c| {
        c.title = classicize(&c.title);
        c.content = classicize(&c.content);
    });
    chapters
}

fn push_chapter(out: &mut Vec<RawChapter>, title: &str, paras: &mut Vec<String>) {
    if paras.is_empty() {
        return;
    }
    out.push(RawChapter::new(title, paras.join("\n")));
    paras.clear();
}

/// 把已有的章（通常只有一章「正文」）按 [`CHUNK_CHARS`] 重新切成若干卷。
fn rechunk(chapters: &[RawChapter]) -> Vec<RawChapter> {
    let all: Vec<String> = chapters
        .iter()
        .flat_map(|c| c.content.lines().map(|l| l.to_string()))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let mut out: Vec<RawChapter> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut n = 0usize;
    for p in all {
        let len = p.chars().count();
        if n + len > CHUNK_CHARS && !cur.is_empty() {
            // 尽量在句读处断：回看最后 CHUNK_LOOKBACK 字里的最后一个句末标点，
            // 断下来的后半句顺延到下一卷，绝不把一句话劈成两半。
            let (done, carry) = cut_at_sentence(std::mem::take(&mut cur));
            out.push(RawChapter::new(
                format!("卷{}", cn_ordinal(out.len() + 1)),
                done.join("\n"),
            ));
            n = carry.iter().map(|s| s.chars().count()).sum();
            cur = carry;
        }
        // 单段本身就超过阈值：按字硬切，避免一章无限长。
        if len > CHUNK_CHARS {
            let cs: Vec<char> = p.chars().collect();
            let mut start = 0usize;
            while start < cs.len() {
                let end = (start + CHUNK_CHARS).min(cs.len());
                let piece: String = cs[start..end].iter().collect();
                if !cur.is_empty() {
                    out.push(RawChapter::new(
                        format!("卷{}", cn_ordinal(out.len() + 1)),
                        cur.join("\n"),
                    ));
                    cur = Vec::new();
                }
                out.push(RawChapter::new(
                    format!("卷{}", cn_ordinal(out.len() + 1)),
                    piece,
                ));
                start = end;
            }
            n = 0;
            continue;
        }
        cur.push(p);
        n += len;
    }
    if !cur.is_empty() {
        out.push(RawChapter::new(
            format!("卷{}", cn_ordinal(out.len() + 1)),
            cur.join("\n"),
        ));
    }
    out
}

/// 把最后一段在窗口内的句读处劈开：返回（本卷前半, 顺延到下一卷的后半）。
fn cut_at_sentence(mut cur: Vec<String>) -> (Vec<String>, Vec<String>) {
    let Some(last) = cur.last_mut() else {
        return (cur, Vec::new());
    };
    let cs: Vec<char> = last.chars().collect();
    if cs.len() > CHUNK_LOOKBACK {
        let lo = cs.len() - CHUNK_LOOKBACK;
        if let Some(pos) = (lo..cs.len()).rev().find(|i| {
            matches!(cs[*i], '。' | '！' | '？' | '；' | '…') && *i + 1 < cs.len()
        }) {
            let head: String = cs[..=pos].iter().collect();
            let tail: String = cs[pos + 1..].iter().collect();
            *last = head;
            return (cur, vec![tail]);
        }
    }
    (cur, Vec::new())
}

// ------------------------------------------------------------------
// 四、markdown：结构化转换
// ------------------------------------------------------------------

/// markdown → 标准章数组。
///
/// * `#`~`######` → 章标题（正文里的一级标题若独占一章，交给
///   [`chapters_to_book`] 提升为书名）；
/// * `>` 引用 → `[注]…[/注]`（竖排里就是双行夹注）；
/// * `**加粗**` / `__加粗__` → `[高亮]…[/高亮]`（字右朱砂圈点）；
/// * ``` 代码块、`行内码`、![图片]() 一律去干净。
pub fn parse_markdown(md: &str) -> Vec<RawChapter> {
    let norm = md.replace("\r\n", "\n").replace('\r', "\n");
    let mut chapters: Vec<RawChapter> = Vec::new();
    // 空标题 = 还没遇到 `#`（正文开头的零散段落归「正文」）。
    let mut title = String::new();
    let mut paras: Vec<String> = Vec::new();
    let mut quote = String::new();
    let mut in_fence = false;

    for raw in norm.lines() {
        let line = raw.trim_end();
        let t = line.trim();

        // ---- 代码块：整段跳过 ----
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            flush_quote(&mut quote, &mut paras);
            continue;
        }
        if in_fence {
            continue;
        }
        if t.is_empty() {
            flush_quote(&mut quote, &mut paras);
            continue;
        }
        // ---- 标题 ----
        if t.starts_with('#') && t.chars().take_while(|c| *c == '#').count() <= 6 {
            let hashes = t.chars().take_while(|c| *c == '#').count();
            let rest = t[hashes..].trim();
            if !rest.is_empty() {
                flush_quote(&mut quote, &mut paras);
                if title.is_empty() {
                    // 标题之前还有零散正文：先收作「正文」章。
                    push_chapter_md(&mut chapters, "正文", &mut paras, false);
                } else {
                    // 标题章即使没有正文也要留一个空章：`# 书名` 独占一章时
                    // 由 `chapters_to_book` 提升为书名。
                    push_chapter_md(&mut chapters, &title, &mut paras, true);
                }
                title = md_inline(rest);
                continue;
            }
        }
        // ---- 分隔线 ----
        if t.chars().all(|c| matches!(c, '-' | '*' | '_' | '=')) && t.len() >= 3 {
            flush_quote(&mut quote, &mut paras);
            continue;
        }
        // ---- 引用：攒成一条夹注 ----
        if let Some(rest) = t.strip_prefix('>') {
            let r = rest.trim();
            if !r.is_empty() {
                if !quote.is_empty() {
                    quote.push('\n');
                }
                quote.push_str(&md_inline(r));
            }
            continue;
        }
        flush_quote(&mut quote, &mut paras);
        // ---- 列表 / 表格：留下文字，丢掉标记 ----
        let body = md_inline(strip_list_marker(t));
        if !body.trim().is_empty() {
            paras.push(body.trim().to_string());
        }
    }
    flush_quote(&mut quote, &mut paras);
    if title.is_empty() {
        push_chapter_md(&mut chapters, "正文", &mut paras, false);
    } else {
        push_chapter_md(&mut chapters, &title, &mut paras, true);
    }
    if chapters.is_empty() {
        chapters.push(RawChapter::new("正文", String::new()));
    }
    chapters.iter_mut().for_each(|c| {
        c.title = classicize(&c.title);
        c.content = classicize(&c.content);
    });
    chapters
}

/// `allow_empty`：标题章没有正文也照收（markdown 的 `# 书名` 就是这样）。
fn push_chapter_md(
    out: &mut Vec<RawChapter>,
    title: &str,
    paras: &mut Vec<String>,
    allow_empty: bool,
) {
    if paras.is_empty() && !allow_empty {
        return;
    }
    out.push(RawChapter::new(title, paras.join("\n")));
    paras.clear();
}

/// 把攒下的引用整段包成一条夹注，接进当前段落。
fn flush_quote(quote: &mut String, paras: &mut Vec<String>) {
    if quote.trim().is_empty() {
        quote.clear();
        return;
    }
    let note = format!("[注]{}[/注]", quote.trim());
    quote.clear();
    match paras.last_mut() {
        // 引用紧接正文：贴在同一段末尾（夹注本来就是插在正文里的）。
        Some(p) => p.push_str(&note),
        None => paras.push(note),
    }
}

fn strip_list_marker(s: &str) -> &str {
    let t = s.trim_start();
    if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        return r;
    }
    if let Some(r) = t.strip_prefix("+ ") {
        return r;
    }
    // 有序列表 `1. `
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i + 1 < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') && bytes[i + 1] == b' '
    {
        return &t[i + 2..];
    }
    t
}

/// 行内 markdown 标记 → 笺读标记。
pub fn md_inline(s: &str) -> String {
    // 1) 图片整块去掉（连 alt 一起，图在竖排里没有位置）。
    let mut s = strip_md_image(s);
    // 2) 链接只留文字。
    s = strip_md_link(&s);
    // 3) 加粗 → 着重标记（放在去星号之前，否则 `**` 已被抹平认不出）。
    s = bold_to_highlight(&s);
    // 4) 剩余的强调 / 删除线 / 行内码：只留文字。
    for token in ["*", "_", "~~", "`"] {
        s = s.replace(token, "");
    }
    s.trim().to_string()
}

fn strip_md_image(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("![") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        // 找到 `](` 之后那个 `)` 才算一条完整的图片语法。
        if let Some(j) = after.find("](") {
            if let Some(k) = after[j + 2..].find(')') {
                rest = &after[j + 2 + k + 1..];
                continue;
            }
        }
        out.push_str("![");
        rest = after;
    }
    out.push_str(rest);
    out
}

fn strip_md_link(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find("](") {
            let label = &after[..j];
            if let Some(k) = after[j + 2..].find(')') {
                out.push_str(label);
                rest = &after[j + 2 + k + 1..];
                continue;
            }
        }
        out.push('[');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `**x**` / `__x__` → `[高亮]x[/高亮]`。
fn bold_to_highlight(s: &str) -> String {
    for (open, close) in [("**", "**"), ("__", "__")] {
        if !s.contains(open) {
            continue;
        }
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find(open) {
            out.push_str(&rest[..i]);
            let after = &rest[i + open.len()..];
            match after.find(close) {
                Some(j) if j > 0 => {
                    out.push_str("[高亮]");
                    out.push_str(&after[..j]);
                    out.push_str("[/高亮]");
                    rest = &after[j + close.len()..];
                }
                _ => {
                    out.push_str(open);
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        // 上一轮可能又造出新的成对标记，交给下一轮处理（最多两轮）。
        return out;
    }
    s.to_string()
}

// ------------------------------------------------------------------
// 五、html / xml：降噪与提取
// ------------------------------------------------------------------

/// 彻底剥离的容器标签（连带内容一起丢）。
const DROP_BLOCKS: [&str; 14] = [
    "script", "style", "head", "nav", "header", "footer", "aside", "form",
    "svg", "iframe", "noscript", "button", "select", "textarea",
];
/// 不成对的自闭合 / 空标签：不会进栈。
const VOID_TAGS: [&str; 10] = [
    "br", "img", "hr", "meta", "link", "input", "source", "area", "col", "wbr",
];
/// 块级标签：一个 = 一段（遇到就断段）。
const BLOCK_TAGS: [&str; 11] = [
    "p", "div", "li", "tr", "section", "article", "td", "dt", "dd", "pre", "figcaption",
];

/// html / xml → 标准章数组。
///
/// * `<script> <style> <nav> <header> <footer> …` 整块丢弃；
/// * `<h1>~<h6>` 作章标题；`<p> <div> <li>` 作正文段落；`<br>` 断行；
/// * `<em> <i> <cite> <q>` 与 `<span class="note">` → `[注]…[/注]`；
/// * `<strong> <b>` → `[高亮]…[/高亮]`；
/// * `<blockquote>` 整块 → `[注]…[/注]`；
/// * `&nbsp; &lt; &#123;` 之类实体一律还原。
pub fn parse_html(html: &str) -> Vec<RawChapter> {
    let mut s = html.to_string();
    for tag in DROP_BLOCKS {
        s = remove_block(&s, tag);
    }
    // 注释也去掉（条件注释里常有整段 IE 脚本）。
    while let Some(i) = s.find("<!--") {
        match s[i..].find("-->") {
            Some(j) => s.replace_range(i..i + j + 3, ""),
            None => {
                s.truncate(i);
                break;
            }
        }
    }

    let mut chapters: Vec<RawChapter> = Vec::new();
    let mut title = "正文".to_string();
    let mut paras: Vec<String> = Vec::new();
    let mut buf = String::new();
    // 待闭合的标签栈：记「标签名 + 它对应的笺读标记」。
    // 只记标记种类是不够的——闭合标签 `</span>` **不带** class 属性，
    // 光看名字认不出它是「注」，得靠开标签时记下的那一笔。
    let mut stack: Vec<OpenTag> = Vec::new();
    let mut heading: Option<String> = None;
    let mut in_blockquote = 0usize;

    // 段落入库时才还原实体：`&nbsp;` `&#12298;` 之类是**文本**层面的噪声，
    // 必须在这里解，否则会被后面的全角化把 `;` 转成 `；`（变成字面 `&nbsp；`）。
    let flush = |paras: &mut Vec<String>, buf: &mut String| {
        let t = decode_entities(buf.trim());
        let t = t.trim();
        if !t.is_empty() {
            paras.push(t.to_string());
        }
        buf.clear();
    };

    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != '<' {
            let ch = bytes[i];
            match &mut heading {
                Some(h) => h.push(ch),
                None => buf.push(ch),
            }
            i += 1;
            continue;
        }
        // ---- 读一个标签 ----
        let end = match (i..bytes.len()).find(|k| bytes[*k] == '>') {
            Some(k) => k,
            None => break,
        };
        let raw: String = bytes[i + 1..end].iter().collect();
        let (name, attrs, closing, _self_close) = parse_tag(&raw);
        i = end + 1;

        if name.is_empty() {
            continue;
        }
        // 注释 `<!-- -->` 已在上面清掉；`<?xml ?>` / `<!doctype>` 直接跳过。
        if name.starts_with('!') || name.starts_with('?') {
            continue;
        }

        let lower = name.to_ascii_lowercase();

        if closing {
            match lower.as_str() {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    if let Some(h) = heading.take() {
                        let h = decode_entities(h.trim());
                        if !h.is_empty() {
                            flush(&mut paras, &mut buf);
                            push_chapter_html(&mut chapters, &title, &mut paras);
                            title = h;
                        }
                    }
                }
                _ => {
                    // 栈顶正是这个标签名 → 收口，并把开标签时记下的标记补上。
                    let matches = stack.last().map(|t| t.name == lower).unwrap_or(false);
                    if matches {
                        let top = stack.pop().unwrap();
                        if lower == "blockquote" {
                            in_blockquote = in_blockquote.saturating_sub(1);
                        }
                        if let Some(kind) = top.kind {
                            close_one(kind, &mut buf, &mut heading);
                        }
                    }
                    if BLOCK_TAGS.contains(&lower.as_str()) {
                        flush(&mut paras, &mut buf);
                    }
                }
            }
            continue;
        }

        if VOID_TAGS.contains(&lower.as_str()) {
            if lower == "br" || lower == "hr" {
                flush(&mut paras, &mut buf);
            }
            continue;
        }

        // ---- 开始标签 ----
        if let Some(h) = lower.strip_prefix('h') {
            if h.len() == 1 && h.chars().all(|c| ('1'..='6').contains(&c)) {
                flush(&mut paras, &mut buf);
                heading = Some(String::new());
                continue;
            }
        }
        if BLOCK_TAGS.contains(&lower.as_str()) {
            flush(&mut paras, &mut buf);
            continue;
        }
        if lower == "blockquote" {
            flush(&mut paras, &mut buf);
            if in_blockquote == 0 {
                open_tag("blockquote", Some("注"), &mut stack, &mut buf, &mut heading);
            }
            in_blockquote += 1;
            continue;
        }
        // 其余标签：可能带「注 / 高亮」语义（em / span.note / strong …），
        // 也可能只是排版容器；无论哪种都入栈，闭合时按名字对上。
        let kind = tag_kind(&lower, &attrs);
        if kind.is_some() || !VOID_TAGS.contains(&lower.as_str()) {
            open_tag(&lower, kind, &mut stack, &mut buf, &mut heading);
        }
    }
    // 收尾：未闭合的标记补齐，残留文本入库。
    while let Some(top) = stack.pop() {
        if let Some(kind) = top.kind {
            close_one(kind, &mut buf, &mut heading);
        }
    }
    if let Some(h) = heading.take() {
        let h = decode_entities(h.trim());
        if !h.is_empty() {
            title = h;
        }
    }
    flush(&mut paras, &mut buf);
    push_chapter_html(&mut chapters, &title, &mut paras);
    if chapters.is_empty() {
        // 一个标签都没有（或全是噪块）：退回「整段去标签」的兜底路径。
        let plain = strip_tags(&s);
        let text = decode_entities(&plain);
        return parse_txt(&text);
    }
    chapters.iter_mut().for_each(|c| {
        c.title = classicize(&c.title);
        c.content = classicize(&c.content);
    });
    chapters
}

fn push_chapter_html(out: &mut Vec<RawChapter>, title: &str, paras: &mut Vec<String>) {
    if paras.is_empty() {
        return;
    }
    out.push(RawChapter::new(title, paras.join("\n")));
    paras.clear();
}

/// 标签名 → 笺读标记种类：`em / i / cite / q / span.note` 是注，
/// `strong / b / mark` 是着重。
fn tag_kind(name: &str, attrs: &str) -> Option<&'static str> {
    match name {
        "em" | "i" | "cite" | "q" | "small" | "sub" | "sup" => Some("注"),
        "strong" | "b" | "mark" => Some("高亮"),
        "span" => {
            let cls = attr_value(attrs, "class").to_ascii_lowercase();
            if cls.split_whitespace().any(|c| {
                matches!(
                    c,
                    "note" | "notes" | "annotation" | "comment" | "gloss" | "footnote"
                )
            }) {
                Some("注")
            } else if cls
                .split_whitespace()
                .any(|c| matches!(c, "highlight" | "hl" | "emph" | "strong" | "mark"))
            {
                Some("高亮")
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 一个已打开、等待闭合的标签。
struct OpenTag {
    /// 标签名（小写）。
    name: String,
    /// 它对应的笺读标记（`Some("注")` / `Some("高亮")`），排版容器为 `None`。
    kind: Option<&'static str>,
}

fn open_tag(
    name: &str,
    kind: Option<&'static str>,
    stack: &mut Vec<OpenTag>,
    buf: &mut String,
    heading: &mut Option<String>,
) {
    stack.push(OpenTag {
        name: name.to_string(),
        kind,
    });
    if let Some(kind) = kind {
        let open = format!("[{}]", kind);
        match heading {
            Some(h) => h.push_str(&open),
            None => buf.push_str(&open),
        }
    }
}

fn close_one(kind: &'static str, buf: &mut String, heading: &mut Option<String>) {
    let close = format!("[/{}]", kind);
    match heading {
        Some(h) => h.push_str(&close),
        None => buf.push_str(&close),
    }
}

/// 解析 `<div class="x">` → `("div", " class=\"x\"", false, false)`。
fn parse_tag(raw: &str) -> (String, String, bool, bool) {
    let t = raw.trim();
    let closing = t.starts_with('/');
    let t = t.trim_start_matches('/').trim_start();
    let self_close = t.trim_end().ends_with('/');
    let t = t.trim_end().trim_end_matches('/').trim_end();
    let split = t.find(|c: char| c.is_whitespace()).unwrap_or(t.len());
    (
        t[..split].to_ascii_lowercase(),
        t[split..].to_string(),
        closing,
        self_close,
    )
}

fn attr_value(attrs: &str, key: &str) -> String {
    let needle = format!("{key}=");
    let Some(i) = attrs.to_ascii_lowercase().find(&needle) else {
        return String::new();
    };
    let rest = &attrs[i + needle.len()..];
    let rest = rest.trim_start_matches(|c| c == '"' || c == '\'');
    let end = rest.find(|c| c == '"' || c == '\'').unwrap_or(rest.len());
    rest[..end].to_string()
}

/// 兜底：把整段文本里的标签一律抹掉。
pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// 整块删除 `<tag …>…</tag>`（大小写不敏感、含属性）。
pub fn remove_block(s: &str, tag: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut rest_lower = lower.as_str();
    while let Some(i) = rest_lower.find(&open) {
        // 必须是真标签名边界（`<script` 后面跟空格 / `>` / `/`）。
        let after = &rest_lower[i + open.len()..];
        let is_tag = after.is_empty()
            || after.starts_with('>')
            || after.starts_with(' ')
            || after.starts_with('/')
            || after.starts_with('\t')
            || after.starts_with('\n');
        if !is_tag {
            out.push_str(&rest[..i + open.len()]);
            rest = &rest[i + open.len()..];
            rest_lower = after;
            continue;
        }
        out.push_str(&rest[..i]);
        match rest_lower[i..].find(&close) {
            Some(j) => {
                let skip = i + j + close.len();
                rest = &rest[skip..];
                rest_lower = &rest_lower[skip..];
            }
            None => {
                // 没闭合：吃到下一个 `>` 为止（或干脆丢弃到结尾）。
                match rest_lower[i..].find('>') {
                    Some(j) => {
                        let skip = i + j + 1;
                        rest = &rest[skip..];
                        rest_lower = &rest_lower[skip..];
                    }
                    None => {
                        rest = "";
                        rest_lower = "";
                    }
                }
            }
        }
    }
    out.push_str(rest);
    out
}

/// HTML 实体还原：具名（`&nbsp;` `&lt;` …）+ 数字（`&#123;` `&#x1F;`）。
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        // 数字实体
        if let Some(hex) = after.strip_prefix("#x").or_else(|| after.strip_prefix("#X")) {
            let digits: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if let Some(end) = after.find(';') {
                if end <= digits.len() + 2 {
                    if let Ok(n) = u32::from_str_radix(&digits, 16) {
                        if let Some(c) = char::from_u32(n) {
                            out.push(c);
                            rest = &after[end + 1..];
                            continue;
                        }
                    }
                }
            }
        } else if let Some(dec) = after.strip_prefix('#') {
            let digits: String = dec.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Some(end) = after.find(';') {
                if end <= digits.len() + 1 && !digits.is_empty() {
                    if let Ok(n) = digits.parse::<u32>() {
                        if let Some(c) = char::from_u32(n) {
                            out.push(c);
                            rest = &after[end + 1..];
                            continue;
                        }
                    }
                }
            }
        } else if let Some(end) = after.find(';') {
            let name = &after[..end];
            if let Some(c) = named_entity(name) {
                out.push_str(c);
                rest = &after[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn named_entity(name: &str) -> Option<&'static str> {
    let s = match name {
        "nbsp" => " ",
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" | "#39" => "'",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "ldquo" => "“",
        "rdquo" => "”",
        "lsquo" => "‘",
        "rsquo" => "’",
        "middot" => "·",
        "times" => "×",
        "deg" => "°",
        "copy" => "©",
        "reg" => "®",
        "sect" => "§",
        "para" => "¶",
        "laquo" => "《",
        "raquo" => "》",
        "yen" => "¥",
        "eacute" => "é",
        "szlig" => "ß",
        "emsp" => "　",
        "ensp" => " ",
        "thinsp" => " ",
        "zwj" | "zwnj" => "",
        _ => return None,
    };
    Some(s)
}

// ------------------------------------------------------------------
// 六、json（已是结构化的书稿）
// ------------------------------------------------------------------

/// json → 标准章数组（兼容 `chapters: [{title, paragraphs: [...]}]`
/// 与 `chapters: [{title, content: "..."}]` 两种写法）。
pub fn parse_jsonish(text: &str) -> Result<Vec<RawChapter>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let arr = v
        .get("chapters")
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow!("json 里没有 chapters 数组"))?;
    let mut out = Vec::new();
    for ch in arr {
        let title = ch
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("正文")
            .to_string();
        let content = if let Some(c) = ch.get("content").and_then(|c| c.as_str()) {
            c.to_string()
        } else {
            ch.get("paragraphs")
                .and_then(|p| p.as_array())
                .map(|ps| {
                    ps.iter()
                        .filter_map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };
        out.push(RawChapter::new(title, content));
    }
    Ok(out)
}

// ------------------------------------------------------------------
// 七、统一入口
// ------------------------------------------------------------------

/// 管线总入口：给文件名与字节，返回标准章数组。
pub fn parse_to_jiandu_format(name: &str, bytes: &[u8]) -> Result<Vec<RawChapter>> {
    let text = decode_bytes(bytes);
    let head: String = text.chars().take(4096).collect();
    parse_str(sniff_kind(name, &head), &text)
}

/// 已知格式时直接解析字符串（测试与 EPUB 内部调用都走这里）。
pub fn parse_str(kind: SrcKind, text: &str) -> Result<Vec<RawChapter>> {
    match kind {
        SrcKind::Txt => Ok(parse_txt(text)),
        SrcKind::Markdown => Ok(parse_markdown(text)),
        SrcKind::Html => Ok(parse_html(text)),
        SrcKind::Json => parse_jsonish(text),
        // epub 是容器：先由 importer 拆出 xhtml，再逐份走 Html 分支。
        SrcKind::Epub => Ok(parse_html(text)),
    }
}

/// 摘出 `[高亮]…[/高亮]`，返回（干净文本, [(起始字序, 字数, 文本)]）。
///
/// `[注]…[/注]` **不动**——它留在文本里，由排版引擎在布局时展开成双行夹注。
pub fn split_highlights(s: &str) -> (String, Vec<(usize, usize, String)>) {
    let mut out = String::with_capacity(s.len());
    let mut spans = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("[高亮]") {
            let (body, after) = match tail.find("[/高亮]") {
                Some(i) => (&tail[..i], &tail[i + "[/高亮]".len()..]),
                None => (tail, ""),
            };
            let at = out.chars().count();
            let len = body.chars().count();
            out.push_str(body);
            if len > 0 {
                spans.push((at, len, body.to_string()));
            }
            rest = after;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    (out, spans)
}

/// 一行正文 → 段落：摘掉 `[高亮]`（转成着重 `Note`），`〖〗≪≫` 由 `para_of` 解析。
///
/// 导入管线的每一条正文都从这里过，EPUB 与普通文本共用同一条清洗链，
/// 免得「epub 有高亮、txt 没有」这类分叉。
pub fn paragraph_of(line: &str) -> Paragraph {
    let (clean, spans) = split_highlights(line);
    let mut p = crate::importer::para_of(&clean);
    for (at, len, text) in spans {
        p.notes.push(Note::span(at, len, NoteKind::Emph, text));
    }
    p
}

/// 标准章数组 → `Book`。
///
/// 首章只有标题没有正文时（markdown 常见：`# 书名` 独占一章），把它提升为书名。
pub fn chapters_to_book(fallback_title: &str, author: &str, chapters: Vec<RawChapter>) -> Book {
    let mut chapters = chapters;
    let mut title = fallback_title.to_string();
    if chapters.len() > 1 && chapters[0].is_empty_body() {
        let t = chapters.remove(0).title;
        if !t.trim().is_empty() {
            title = t;
        }
    }
    let mut book = Book::new(&title, author);
    book.cover.title = title.clone();
    for c in chapters {
        let mut paragraphs = Vec::new();
        for line in c.content.lines() {
            let l = line.trim();
            if l.is_empty() {
                continue;
            }
            paragraphs.push(paragraph_of(l));
        }
        if paragraphs.is_empty() {
            continue;
        }
        book.chapters.push(Chapter {
            title: crate::numcn::normalize_numbers(&c.title),
            paragraphs,
        });
    }
    if book.chapters.is_empty() {
        book.chapters.push(Chapter {
            title: "正文".into(),
            paragraphs: Vec::new(),
        });
    }
    book
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 解码 ----

    #[test]
    fn decode_utf8_and_bom() {
        assert_eq!(decode_bytes("道德经".as_bytes()), "道德经");
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("道德经".as_bytes());
        assert_eq!(decode_bytes(&bom), "道德经");
    }

    #[test]
    fn decode_gbk_fallback() {
        // 「道德经」的 GBK 字节（不是合法 UTF-8）。
        let gbk: Vec<u8> = vec![0xB5, 0xC0, 0xB5, 0xC2, 0xBE, 0xAD];
        assert!(std::str::from_utf8(&gbk).is_err());
        assert_eq!(decode_bytes(&gbk), "道德经");
    }

    // ---- 标点全角 ----

    #[test]
    fn punct_goes_fullwidth_but_keeps_decimals() {
        assert_eq!(fullwidth_punct("你好,世界.真的?是的!"), "你好，世界。真的？是的！");
        // 数字里的点与千分位不动：3.14 / 1,980 保持原样
        assert_eq!(fullwidth_punct("3.14"), "3.14");
        assert_eq!(fullwidth_punct("1,980"), "1,980");
    }

    // ---- txt ----

    #[test]
    fn txt_splits_by_chapter_head() {
        let cs = parse_txt("第一章 宴桃园\n话说天下大势。\n\n第二章\n分久必合。");
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].title, "第一章 宴桃园");
        assert_eq!(cs[1].title, "第二章");
        assert!(cs[0].content.contains("话说天下大势。"));
    }

    #[test]
    fn txt_recognises_cn_and_arabic_numerals() {
        assert!(is_chapter_head("第十二回"));
        assert!(is_chapter_head("第108回"));
        assert!(is_chapter_head("第三卷"));
        // 标题行后面拖了太长的内容，就不是章名了（多半是正文被折行拆出来的）。
        let long = format!("第一章{}", "内容".repeat(30));
        assert!(!is_chapter_head(&long));
        assert!(!is_chapter_head("天下大势"));
        assert!(!is_chapter_head("第二天")); // 有数字，但后面不是章回卷节
        assert!(is_chapter_head("第一章")); // 最短的章名也认
    }

    #[test]
    fn txt_chunks_long_text_without_headings() {
        let long: String = "天地玄黄宇宙洪荒。".repeat(400); // 9*400 = 3600 字
        let cs = parse_txt(&long);
        assert!(cs.len() >= 2, "超长无标题文本应按 3000 字切卷：{}", cs.len());
        assert!(cs[0].title.starts_with("卷"));
        let total: usize = cs.iter().map(|c| c.content.chars().count()).sum();
        assert_eq!(total, 3600, "切卷不许丢字");
    }

    #[test]
    fn txt_classicizes_digits_and_punct() {
        let cs = parse_txt("第一章\n建安223年，策为尚书郎.共15卷。");
        assert!(cs[0].content.contains("建安二二三年"), "{}", cs[0].content);
        assert!(cs[0].content.contains("共十五卷"));
    }

    // ---- markdown ----

    #[test]
    fn md_headings_become_chapter_titles() {
        let cs = parse_markdown("# 道德经\n\n## 第一章\n\n道可道，非常道。");
        assert_eq!(cs[0].title, "道德经");
        assert_eq!(cs[1].title, "第一章");
        assert_eq!(cs[1].content, "道可道，非常道。");
    }

    #[test]
    fn md_quote_becomes_inline_note() {
        let cs = parse_markdown("## 第一章\n\n道可道。\n> 王弼曰：此二者同出而异名。\n> 又说一句。\n\n名可名。");
        let c = &cs[0];
        assert!(
            c.content.contains("[注]王弼曰：此二者同出而异名。\n又说一句。[/注]"),
            "引用应合成一条夹注：{}",
            c.content
        );
        // 引用接在正文之后：贴在同一段里，不另起一段。
        assert!(c.content.starts_with("道可道。[注]"), "{}", c.content);
    }

    #[test]
    fn md_bold_becomes_highlight() {
        let cs = parse_markdown("## 章\n\n这是**要紧**的一句话。");
        assert_eq!(cs[0].content, "这是[高亮]要紧[/高亮]的一句话。");
    }

    #[test]
    fn md_strips_code_fences_and_images() {
        let md = "## 章\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n\n![封面](cover.png)\n\n正文`code`在此。";
        let cs = parse_markdown(md);
        assert!(!cs[0].content.contains("println"), "代码块应被丢弃：{}", cs[0].content);
        assert!(!cs[0].content.contains("cover.png"), "图片应被丢弃：{}", cs[0].content);
        assert!(cs[0].content.contains("正文code在此"), "{}", cs[0].content);
    }

    #[test]
    fn md_links_keep_label_only() {
        let cs = parse_markdown("## 章\n\n见[维基](https://a.b/c)所说。");
        assert_eq!(cs[0].content, "见维基所说。");
    }

    // ---- html ----

    #[test]
    fn html_drops_noise_and_uses_headings() {
        let html = r#"<html><head><title>T</title></head><body>
<script>var a=1;</script><style>p{color:red}</style><nav>目录</nav>
<h1>卷一</h1><p>道可道，非常道。</p><p>名可名。</p>
<h2>第二节</h2><p>无名天地之始。</p></body></html>"#;
        let cs = parse_html(html);
        assert_eq!(cs[0].title, "卷一");
        assert!(!cs[0].content.contains("var a"), "script 应剥离");
        assert!(!cs[0].content.contains("color:red"), "style 应剥离");
        assert!(!cs[0].content.contains("目录"), "nav 应剥离");
        assert_eq!(cs[1].title, "第二节");
        assert!(cs[1].content.contains("无名天地之始"));
    }

    #[test]
    fn html_em_becomes_note_and_strong_becomes_highlight() {
        let html = r#"<p>道可<em>非常</em>道，<span class="note">王弼注</span>也。</p>
<p>此<strong>要紧</strong>处。</p>"#;
        let cs = parse_html(html);
        let all = cs[0].content.clone();
        assert!(all.contains("[注]非常[/注]"), "{}", all);
        assert!(all.contains("[注]王弼注[/注]"), "{}", all);
        assert!(all.contains("[高亮]要紧[/高亮]"), "{}", all);
    }

    #[test]
    fn html_note_closes_even_without_class_on_close_tag() {
        // `</span>` 不带 class：闭合必须靠开标签时记下的那一笔，
        // 否则 `[注]` 会一路开到章尾，把后面的正文全吃成夹注。
        let cs = parse_html(
            r#"<p>甲<span class="note">注一</span>乙</p><h2>下一章</h2><p>丙丁</p>"#,
        );
        assert_eq!(cs[0].content, "甲[注]注一[/注]乙", "{}", cs[0].content);
        assert_eq!(cs[1].title, "下一章");
        assert_eq!(cs[1].content, "丙丁", "注不该漏到下一章：{}", cs[1].content);
    }

    #[test]
    fn html_blockquote_is_note() {
        let cs = parse_html("<p>正文。</p><blockquote>引用一段话。</blockquote>");
        assert!(cs[0].content.contains("[注]引用一段话。[/注]"), "{}", cs[0].content);
    }

    #[test]
    fn html_decodes_entities() {
        assert_eq!(decode_entities("a&nbsp;b&lt;c&gt;d&amp;e"), "a b<c>d&e");
        assert_eq!(decode_entities("&#36947;&#x5FB7;"), "道德");
        let cs = parse_html("<p>甲&nbsp;乙&mdash;丙&ldquo;丁&rdquo;</p>");
        assert_eq!(cs[0].content, "甲 乙—丙“丁”");
    }

    #[test]
    fn html_falls_back_to_plain_text_when_no_structure() {
        // 一个标签都没有：退回 txt 路径（按 3000 字切卷也仍成立）。
        let cs = parse_html("道可道，非常道。");
        assert_eq!(cs[0].content, "道可道，非常道。");
    }

    // ---- 高亮摘取 ----

    #[test]
    fn highlights_are_extracted_with_offsets() {
        let (clean, spans) = split_highlights("道可[高亮]非常[/高亮]道");
        assert_eq!(clean, "道可非常道");
        assert_eq!(spans, vec![(2, 2, "非常".to_string())]);
    }

    #[test]
    fn chapters_to_book_marks_emphasis_notes() {
        let cs = vec![RawChapter::new(
            "卷一",
            "道可[高亮]非常[/高亮]道[注]夹注[/注]".to_string(),
        )];
        let b = chapters_to_book("道德经", "老子", cs);
        let p = &b.chapters[0].paragraphs[0];
        assert_eq!(p.text, "道可非常道[注]夹注[/注]");
        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].kind, NoteKind::Emph);
        assert_eq!(p.notes[0].at, 2);
        assert_eq!(p.notes[0].len, 2);
    }

    #[test]
    fn first_empty_chapter_is_promoted_to_book_title() {
        let cs = vec![
            RawChapter::new("道德经", String::new()),
            RawChapter::new("第一章", "道可道。"),
        ];
        let b = chapters_to_book("fallback", "", cs);
        assert_eq!(b.title, "道德经");
        assert_eq!(b.chapters.len(), 1);
    }

    // ---- 嗅探 ----

    #[test]
    fn sniff_by_extension_and_content() {
        assert_eq!(sniff_kind("a.md", ""), SrcKind::Markdown);
        assert_eq!(sniff_kind("a.html", ""), SrcKind::Html);
        assert_eq!(sniff_kind("a.xml", ""), SrcKind::Html);
        assert_eq!(sniff_kind("noext", "<!DOCTYPE html><p>x</p>"), SrcKind::Html);
        assert_eq!(sniff_kind("noext", "{"), SrcKind::Json);
    }
}
