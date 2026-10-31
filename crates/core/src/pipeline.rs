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
//!      加粗 / strong 转着重 `[高亮]…[/高亮]`、阿拉伯数字转汉字。
//!
//! 最后 `chapters_to_book` 把 `[高亮]` 摘成 `Note`（`NoteKind::Emph`，渲染为
//! 字右的朱砂圈点），`[注]` 留给排版引擎在布局时展开成双行夹注。

use crate::types::{Book, Chapter, Note, NoteKind, Paragraph};
use anyhow::{anyhow, Result};
use std::io::{Cursor, Read};

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
    /// 古籍 TEI / 通用 XML：按白名单标签抽 textContent（见 `parsers::parse_xml`）。
    Xml,
}

impl SrcKind {
    pub fn label(self) -> &'static str {
        match self {
            SrcKind::Txt => "txt",
            SrcKind::Markdown => "md",
            SrcKind::Html => "html",
            SrcKind::Json => "json",
            SrcKind::Epub => "epub",
            SrcKind::Xml => "xml",
        }
    }
}

/// 按扩展名 + 内容嗅探判断格式；认不出来就退回 txt（纯文本最兜底）。
pub fn sniff_kind(name: &str, head: &str) -> SrcKind {
    let lower = name.to_ascii_lowercase();
    if let Some(ext) = lower.rsplit('.').next() {
        match ext {
            "md" | "markdown" | "mdown" => return SrcKind::Markdown,
            "html" | "htm" | "xhtml" | "opf" | "xht" => return SrcKind::Html,
            "xml" | "tei" => return SrcKind::Xml,
            "json" => return SrcKind::Json,
            "epub" => return SrcKind::Epub,
            "txt" | "text" => return SrcKind::Txt,
            _ => {}
        }
    }
    // 大小写不敏感：`<!DOCTYPE HTML>` 也是 HTML。
    let t = head.trim_start().to_ascii_lowercase();
    if t.starts_with("<!doctype html") || t.starts_with("<html") {
        return SrcKind::Html;
    }
    if t.starts_with("<?xml") {
        // XML 声明后面通常跟着 `<TEI` / `<book` 等古籍标签；没有 doctype 的
        // 纯 `<xxx …>` 根节点也按 XML 处理（TEI / DocBook 等）。
        return SrcKind::Xml;
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
    // v1.9.9：**加强版编码嗅探**。老逻辑只要 `from_utf8` 成功就交差，于是
    // 「UTF-8 合法但其实是乱码」（BOM 骗人 / 中途被转过码 / PUA 缺字成片）
    // 会一路放行，用户看到的就是满页方框。新漏斗多加两道：乱码体检 + Big5，
    // 见 [`crate::dclean::sniff_decode`]（它是本函数的严格超集，行为兼容）。
    crate::dclean::sniff_decode(bytes)
}

/// 老解码路径，原样保留给单测对照（BOM → UTF-8 → GBK，不做体检）。
#[doc(hidden)]
pub fn decode_bytes_legacy(bytes: &[u8]) -> String {
    // UTF-8 BOM
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    // UTF-16 LE / BE BOM
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let u16s: Vec<u16> = bytes
            .as_chunks::<2>().0.iter()
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
    // 破折号竖排化与 EPUB 那条链同源（`vclean::dash_to_vertical`）：
    // 横躺的 `——` 在竖排里就是一根拦腰截断行气的横杠。
    let s = crate::vclean::dash_to_vertical(s);
    // **注疏壳先归一、只去壳留肉身**（v1.68.1）：`[ 注 ]` / `[疏]` / `[/疏]`
    // → `[注]` / `[/注]`。必须在 `purge_bracket_marks` **之前**——那一位只认
    // 自家的定长壳，认不出的 `[ 注 ]` 会被当杂质连括号削掉、把「注」当成
    // 正文印在版心（注疏正文虽留着，却再也不成夹注了）。
    let s = crate::cleaner::normalize_note_shells(&s);
    // 落单的书名号（跨段截断的半个《》）删掉，成对的留给下游转 ︽︾。
    let s = crate::cleaner::strip_orphan_title_marks(&s);
    // 现代括号不进正文流（与 `parsers::cleanup_ancient_text` 同源）：EPUB
    // 那条链也走这里，两链一致才不会「时有时无」。
    let s = crate::parsers::purge_bracket_marks(&s);
    // ⚠️ 全角化**只跑在标记之外**：`[拼音:hàn]` 里的冒号是注音语法不是标点，
    // 折成全角 `：` 之后下游 `line_to_spans` 就认不出这枚标记，注音会散进
    // 正文——竖排里就是一排「电线杆」。这条链（txt / epub / docx 都走
    // `parse_txt`）此前正栽在这里。
    let s = crate::vclean::map_outside_markers(&s, |seg| fullwidth_punct(seg));
    crate::numcn::normalize_numbers(&s)
}

/// 章序号的中文写法（`1 -> 一`、`15 -> 十五`），切卷时用。
pub fn cn_ordinal(n: usize) -> String {
    crate::numcn::num_to_cn(n as u64)
}

// ------------------------------------------------------------------
// 三、txt：智能切章
// ------------------------------------------------------------------

/// 是否章节标题行：`第` + 若干中文数字 / 阿拉伯数字 + 章节单位字。
///
/// 单位字见 [`crate::cleaner::HEAD_UNITS`]——传统六种（章 / 回 / 卷 / 节 /
/// 篇 / 折 / 出）之外，v1.75.0 补了佛经与子书的分节单位 **品 / 颂 / 偈 /
/// 门 / 赞**：《金刚经》的三十二品、 《法华经》的二十八品此前一个都不认，
/// 整段品题连着正文挤在同一段里。
///
/// 另支持**后置序数**（佛经常见「法会因由分第一」「威仪寂静分第二十九」），
/// 见 [`trailing_ordinal`]。
///
/// 用字符扫描而不是正则——`[0-9]` 与全角数字、`两`、`廿` 都得认，
/// 而且标题后面常跟整句标题名（「第一章 宴桃园豪杰三结义」）。
pub fn is_chapter_head(line: &str) -> bool {
    let cs: Vec<char> = line.trim().chars().collect();
    if cs.len() < 3 || cs.len() > 40 {
        return false;
    }
    // 标题行不带句读——这一条先挡掉绝大多数正文（「见第二十章，…」）。
    if cs.iter().any(|c| matches!(c, '。' | '，' | '、' | '；' | '！' | '？')) {
        return false;
    }
    // ① 前置序数：「第」+ 数字 + 单位字。
    //    允许**短前缀**：佛经的品题常把书名写在前面（「金刚经第一品 法会因
    //    由分」），前缀限 8 个汉字——「见第二十章」这类正文虽然也满足，
    //    但它通常跟在句读后、进不了短行，配合上面的无句读判据足矣。
    if let Some(i) = cs.iter().position(|c| *c == '第') {
        if i <= 8 && cs[..i].iter().all(|c| crate::cleaner::is_han(*c)) {
            let mut j = i + 1;
            while j < cs.len() && is_numeral(cs[j]) {
                j += 1;
            }
            // ⚠️ `j` 可能刚好走到末尾（「第二十九」「第108」这种孤零零的序
            // 数）：先挡越界再取字符——此前直接 `cs[j]` 会 panic，把整次导
            // 入炸掉。
            if j > i + 1 && j < cs.len() && crate::cleaner::HEAD_UNITS.contains(&cs[j]) {
                return true;
            }
        }
    }
    // ② 后置序数：「法会因由分第一」——数字在末尾。
    trailing_ordinal(&cs)
}

/// 后置序数：以 `第` + 数字**收尾**的短行（佛经品题的常见写法）。
///
/// 判据很窄——短行（≤20 字）+ 不含句读 + 末尾是「第」+ 数字 + 「第」前还有
/// 品名。三条都满足才认，宁可漏判也绝不把正文当标题切走。
fn trailing_ordinal(cs: &[char]) -> bool {
    if cs.len() > 20 {
        return false;
    }
    if cs.iter().any(|c| matches!(c, '。' | '！' | '？' | '；' | '，' | '、')) {
        return false;
    }
    let mut k = cs.len();
    while k > 0 && is_numeral(cs[k - 1]) {
        k -= 1;
    }
    // 没有数字 / 全是数字 / 「第」不在数字前 → 都不是。
    if k == 0 || k == cs.len() || cs[k - 1] != '第' {
        return false;
    }
    // 「第」之前还得有品名（「第二十九」这种孤序数不算标题）。
    k >= 2 && cs[k - 2] != '第'
}

pub(crate) fn is_numeral(c: char) -> bool {
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

/// **TXT 专用**：把全角括号 `〔…〕` / `【…】` 整段连同内容转成 `[注]…[/注]`
/// （双行夹注）。这是「裸文本」里被 Calibre 之类压平的胡三省注 / 校勘记
/// 的唯一救赎——它们丢掉了所有 HTML 标签，只剩一对全角括号。
///
/// 与 [`crate::parsers::purge_bracket_marks`]（通用、有 80 字上限、六格式共用）
/// 不同，这里**不设长度上限**：裸文本里一整段索隱 / 爾雅引文也该是夹注，不该
/// 散进正文。且**只作用于 TXT 解析链**（`parse_txt`），绝不波及 EPUB / HTML。
///
/// 容错：左 / 右括号数量对不上（实测《资治通鉴》90,757 对 91,551）绝对不能
/// Panic——用栈单趟扫描，落单的开 / 闭括号直接丢弃，内容照常保留；结尾把
/// 仍未闭合的开括号补上收壳，保证 `[注]…[/注]` 成对，下游 `split_body_marks`
/// 不会看到悬空开壳。
/// 兼容别名：旧代码仍可调它；新代码请直接用 `crate::cleaner::brackets_to_notes`
/// （后者**同时**处理 `（）`/`()`，是真正的全格式入口）。
pub fn txt_bracket_notes(s: &str) -> String {
    crate::cleaner::brackets_to_notes(s)
}

/// **TXT 专用**：极短且无句读的纯文本行 → 章节标题（标题大字）。
///
/// 兜底 `is_chapter_head` 认不到的「裸标题」：古籍裸文本常把章题写成一行
/// 孤零零的短字（如「序」「跋」「卷一」「周紀一」），既没有「第 X 章」也
/// 没有后置序数。这类行在竖排里本就该是标题大字，不该混进正文。
///
/// 判据（全部满足才认，宁可漏判不可误杀）：
/// 1. 长度 2–14 字（用户规定「少于 15 字」）；
/// 2. 不含任何句读 / 括号 / 书名号（带这些的是正文或带注疏的正文）；
/// 3. 至少含一个汉字（纯数字 / 字母孤行如页码、编号不认）。
///
/// 只加在 `parse_txt` 的行循环里，**绝不**写进 `is_chapter_head`——后者被
/// EPUB / HTML 的 `resplit_by_heads` 共用，动它就是动 EPUB 逻辑。
pub fn txt_is_title_line(line: &str) -> bool {
    let cs: Vec<char> = line.chars().collect();
    let n = cs.len();
    if n < 2 || n >= 15 {
        return false;
    }
    if cs.iter().any(|c| {
        matches!(
            c,
            '。' | '，' | '、' | '；' | '！' | '？' | '：' | '…' | '—'
                | '（' | '）' | '〔' | '〕' | '【' | '】' | '《' | '》'
                | '“' | '”' | '‘' | '’' | '．' | '·'
        )
    }) {
        return false;
    }
    cs.iter().any(|c| crate::cleaner::is_han(*c))
}

/// txt → 标准章数组。
pub fn parse_txt(text: &str) -> Vec<RawChapter> {
    // 头部幽灵先切一刀（BOM / 零宽 / 全角空格 / YAML 元数据块 / 连续空行）：
    // 它们每一个都占一个正文字格，留着就是第一列顶部的「幽灵空行」。
    // v1.75.0：品题断行必须在**逐行判章之前**——网上的佛经 txt 常是一整坨，
    // 「第二十九品 威仪寂静分须菩提。…」黏成一行的情况下，行首不是「第」，
    // `is_chapter_head` 根本没机会认。先切开，品题才成行、才被认作章。
    let norm = crate::cleaner::split_inline_heads(&crate::cleaner::strip_head_junk(
        &text.replace("\r\n", "\n").replace('\r', "\n"),
    ));
    // TXT 专用：裸文本里被压平的全角括号注疏（胡三省注 / 校勘记）先归一成
    // `[注]…[/注]`，下游 `split_body_marks` 自然落成双行夹注。这一步只在 TXT
    // 链走，不动 EPUB / HTML（它们有自己的清洗，且 `purge_bracket_marks` 有
    // 长度上限，不在本函数里改）。
    // 全格式夹注归一：`〔…〕`/`【…】`/`（）`/`()` 全部归一成 `[注]…[/注]`，
    // 下游 `split_body_marks` 自然落成双行夹注。这一行让 TXT / DOCX 走通夹注。
    let norm = crate::cleaner::brackets_to_notes(&norm);
    let mut chapters: Vec<RawChapter> = Vec::new();
    let mut title = "正文".to_string();
    let mut paras: Vec<String> = Vec::new();
    let mut buf = String::new();
    // `buf` 的字数**增量累加**：每加一行就加上该行的字数。`should_break_para`
    // 每行都要问一次「攒够了没」，每次对 buf 全量 `chars().count()` 是 O(n²)
    // ——50MB 文件实测白白吃掉两秒。
    let mut buf_chars = 0usize;
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
            buf_chars = 0;
            continue;
        }
        if is_chapter_head(line) || txt_is_title_line(line) {
            flush(&mut paras, &mut buf);
            buf_chars = 0;
            push_chapter(&mut chapters, &title, &mut paras);
            title = line.to_string();
            saw_head = true;
            continue;
        }
        // v1.9.9 **智能断行**：原本是「非空行一律接到 buf，空行才分段」，
        // 结果整章几千字糊成一段。现在改成攒够 [`dclean::MERGE_PARA_CHARS`]
        // 字且上一行已收句才另起一段——碎片行照样被并掉（没标点绝不断），
        // 长章也不会糊成一坨。见 `dclean::should_break_para`。
        if crate::dclean::should_break_para(&buf, buf_chars, crate::dclean::MERGE_PARA_CHARS) {
            flush(&mut paras, &mut buf);
            buf_chars = 0;
        }
        buf.push_str(line);
        buf_chars += line.chars().count();
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

/// **按品 / 章标题行重新切章**（v1.75.0）：一品为一章。
///
/// 各格式的解析器切章口径不一——TXT 认 `is_chapter_head`，EPUB / HTML 只认
/// 结构标签（`h1`~`h6`）。佛经的「第X品」在 EPUB 里往往只是普通段落：断行
/// 之后它虽然单独成行，却仍赖在上一品的章里。这里统一再扫一遍，**行首是
/// 章节标题行就开新章**，让六种格式的品 / 章口径归于一律。
///
/// 幂等：TXT 那条链已经这么切过，再跑一遍不会多切（切完的章里不再有行首
/// 标题行）。
pub fn resplit_by_heads(chapters: Vec<RawChapter>) -> Vec<RawChapter> {
    if chapters.is_empty() {
        return chapters;
    }
    let mut out: Vec<RawChapter> = Vec::new();
    for c in chapters {
        let mut title = c.title;
        let mut paras: Vec<String> = Vec::new();
        let mut pushed_any = false;
        for line in c.content.lines() {
            let t = line.trim();
            if !t.is_empty() && is_chapter_head(t) {
                push_chapter(&mut out, &title, &mut paras);
                title = t.to_string();
                pushed_any = true;
                continue;
            }
            paras.push(line.to_string());
        }
        push_chapter(&mut out, &title, &mut paras);
        let _ = pushed_any;
    }
    // 扉页孤行：开头那行往往只是书名（「金刚经」三个字），它既不是品也不是
    // 章。**尊重原文——一个字都不删**：把它从正文挪去当章标题，随后
    // `chapters_to_book` 的既有约定（首章无正文 → 提升为书名）自然会把它
    // 收作书名。这样目录不空，「金刚经」三个字也还在。
    if out.len() > 1 {
        let n = out[0].content.chars().count();
        if n > 0 && n <= 8 && !out[0].content.contains('。') {
            let t = out[0].content.trim().to_string();
            out[0].title = t;
            out[0].content.clear();
        }
    }
    out
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
        // v1.9.15：药名 + 剂量拆成「正文[注]剂量[/注]」（中医方书专用，
        // 详见 `dclean::split_dosage_to_gloss`）。炮制词（炒 / 炙…）留正文。
        let body = crate::dclean::split_dosage_to_gloss(&md_inline(strip_list_marker(t)));
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
///
/// 末尾统一做一道 [`classicize`]（全角标点 + 数字转汉字）。EPUB 那条更细的
/// 五阶段链（[`crate::vclean`]）要自己在「古籍化」之前插工序，故另走
/// [`parse_html_raw`]——两者共用同一台标签行走器，不分叉。
pub fn parse_html(html: &str) -> Vec<RawChapter> {
    let mut chapters = parse_html_raw(html);
    chapters.iter_mut().for_each(|c| {
        c.title = classicize(&c.title);
        c.content = classicize(&c.content);
    });
    chapters
}

/// html / xml → 标准章数组，**不做**古籍化（标点 / 数字原样）。
///
/// 给 [`crate::vclean`] 用：竖排清洗要先把「页码孤行」按 ASCII 数字认出来
/// 丢掉，再把括号转夹注、最后把数字读成汉字；若这里先把数字化成汉字，
/// 下游就再也认不出「这一行原本是个页码」了。
pub fn parse_html_raw(html: &str) -> Vec<RawChapter> {
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
    // 正在由「非 h1~h6 的题署标签」撑起的标题：值为该标签名，闭合即收口。
    let mut heading_owner: Option<String> = None;
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
            // 题署标签（`<div epub:type="title">` 一类）闭合：收口成章标题。
            // 它开时没进栈（见下面的开始标签分支），故不能走栈对名那条路。
            if heading_owner.as_deref() == Some(lower.as_str()) {
                heading_owner = None;
                if let Some(h) = heading.take() {
                    let h = decode_entities(h.trim());
                    if !h.is_empty() {
                        flush(&mut paras, &mut buf);
                        push_chapter_html(&mut chapters, &title, &mut paras);
                        title = h;
                    }
                }
                continue;
            }
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
        // EPUB3 的章题不一定写成 `<h1>`：`<div epub:type="title">`、
        // `<p class="title">`、`<span role="doc-title">` 遍地都是。
        // 这类块一开就当「题署」起头，闭合同名标签时收口。
        if is_doc_title_tag(&lower, &attrs) {
            flush(&mut paras, &mut buf);
            heading = Some(String::new());
            heading_owner = Some(lower.clone());
            continue;
        }
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
    chapters
}

fn push_chapter_html(out: &mut Vec<RawChapter>, title: &str, paras: &mut Vec<String>) {
    if paras.is_empty() {
        return;
    }
    out.push(RawChapter::new(title, paras.join("\n")));
    paras.clear();
}

/// 这个标签是不是「章节题署」（EPUB3 的 `epub:type="title"` 一族）。
///
/// 只认块级的几个容器名——`<title>`（head 里）与 `<h1>~<h6>` 另有专路，
/// 不能在这里重复吞掉。
fn is_doc_title_tag(name: &str, attrs: &str) -> bool {
    if !matches!(name, "div" | "p" | "span" | "section" | "hgroup") {
        return false;
    }
    let et = attr_value(attrs, "epub:type").to_ascii_lowercase();
    if et
        .split_whitespace()
        .any(|t| matches!(t, "title" | "fulltitle" | "subtitle"))
    {
        return true;
    }
    let role = attr_value(attrs, "role").to_ascii_lowercase();
    if role == "doc-title" {
        return true;
    }
    let cls = attr_value(attrs, "class").to_ascii_lowercase();
    cls.split_whitespace().any(|c| {
        matches!(
            c,
            "title" | "booktitle" | "chapter-title" | "chaptertitle" | "chapter_title" | "headline"
        )
    })
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
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest.find(['"', '\'']).unwrap_or(rest.len());
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
///
/// v1.31.0 起统一走 [`crate::parsers::UniversalParser`]：按格式选策略解析，
/// 再经过「古籍化后处理」绝对红线（`cleanup_ancient_text`）——数字汉字化、
/// 标点竖排化、残留标签清理、页码过滤，六格式共用一道闸。
pub fn parse_str(kind: SrcKind, text: &str) -> Result<Vec<RawChapter>> {
    crate::parsers::UniversalParser::parse(kind, text)
}

/// 摘出 `[注]…[/注]`（双行夹注），返回（干净文本, [(宿主字下标, 注文)]）。
///
/// **标记本身绝不进正文流**：它只是「从这里开始排双行小字」的开关信号，
/// 连一个字格都不占（v1.36 用户口径）。摘出后正文只剩纯字，注文挂到宿主字
/// 上（宿主 = 标记前那个字，与 `〖〗` 同口径），由排版引擎在布局时展开成
/// 双行夹注——标签若留在文本里，就会被当成「[」「注」「]」「/」四个正文字
/// 排进字格，行气当场断成四截。
///
/// 未闭合的 `[注]` 到文本末尾视为一条注文（容错，不丢字）；空注文连壳丢弃
/// （`道[注][/注]可` → `道可`，不留空夹注）。
pub fn split_notes(s: &str) -> (String, Vec<(usize, String)>) {
    let mut out = String::with_capacity(s.len());
    let mut notes = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("[注]") {
            let (body, after) = match tail.find("[/注]") {
                Some(i) => (&tail[..i], &tail[i + "[/注]".len()..]),
                None => (tail, ""),
            };
            // 宿主 = 标记前那个已输出的正文字（段首无字则锚在首字上）。
            // 先 `trim_end()` 再数：`尔雅 [注] 二…` 这种带尾空格的，按下标算到
            // 空格之后会在下游 trim 时越界，整条注文连肉身一起被丢（真丢内容）。
            // 宿主下标取「**去掉尾部空白后**的最后一个字」：`[注]` 前面带着
            // 空格时（`尔雅 [注] 二…`），按未去尾空白算的下标会落在空格之后，
            // 下游 `denoise_text` 一 trim 就越界 → 整条夹注连同注疏正文一起
            // 被当无宿主丢掉。绝不能让「去壳」变成「丢肉身」。
            let at = out.trim_end().chars().count().saturating_sub(1);
            let text = body.trim();
            if !text.is_empty() {
                notes.push((at, text.to_string()));
            }
            rest = after;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    (out, notes)
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

/// 摘出 `[拼音:…]`（EPUB `<ruby><rt>` 转来的注音），返回（干净文本, [(宿主字下标, 拼音)]）。
///
/// 标记的语义是「**前一个字**的读音」——清洗时把 `<ruby>汉<rt>hàn</rt></ruby>`
/// 写成 `汉[拼音:hàn]`，故宿主下标取标记前一个已输出字的位次。段首无字则丢弃
/// （注音无处可挂，留着反而变成字面方括号）。
///
/// 竖排渲染侧把它当**格内附属标记**：拼音逐字母竖叠在字格右上角
/// （`ruby::layout`），既不占格、也不进双行夹注，行气不断。
pub fn split_ruby(s: &str) -> (String, Vec<(usize, String)>) {
    let mut out = String::with_capacity(s.len());
    let mut marks: Vec<(usize, String)> = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("[拼音:") {
            let (body, after) = match tail.find(']') {
                Some(i) => (&tail[..i], &tail[i + 1..]),
                None => (tail, ""),
            };
            let n = out.chars().count();
            let py = body.trim();
            if n > 0 && !py.is_empty() {
                marks.push((n - 1, py.to_string()));
            }
            rest = after;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    (out, marks)
}

/// 摘掉 `[数字:…]` 兜底标记，只留中间的数字串本身。
///
/// 这个标记是**跨渲染器的约定**（「这一串不许拆成单字，整串横躺」）。笺读自己的
/// 竖排引擎本来就把连续数字整串成组（`west::scan_digit_run` + 半格账），所以
/// 入库时把壳去掉即可；留着壳会被当成正文方括号画出来。
pub fn strip_digit_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("[数字:") {
            let (body, after) = match tail.find(']') {
                Some(i) => (&tail[..i], &tail[i + 1..]),
                None => (tail, ""),
            };
            out.push_str(body);
            rest = after;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

/// 一行正文 → 段落：摘掉 `[高亮]`（转成着重 `Note`），`〖〗≪≫` 由 `para_of` 解析。
///
/// 顺序有讲究：先拆 `[数字:…]` 壳（它可能包着 `[高亮]`），再拆 `[高亮]`，
/// 最后拆 `[拼音:…]`——注音下标必须与「去掉着重标记之后」的正文同口径，
/// 否则挂错字。
///
/// 导入管线的每一条正文都从这里过，EPUB 与普通文本共用同一条清洗链，
/// 免得「epub 有高亮、txt 没有」这类分叉。
/// 一趟扫完 `[高亮]` 与 `[注]` 两类标记。
///
/// 为什么必须一趟：两种标记的宿主下标都是「**已输出正文的字数**」。分两次
/// 扫的话，后扫的那种会把先扫那一种的标签长度也算进正文里——`道可[高亮]非常
/// [/高亮]道[注]夹注[/注]` 里，「夹注」本该挂在正文第 4 字，先摘夹注会挂到
/// 第 12 字（把 `[高亮]` 那几个标签字当成了正文）。一趟扫，两者的 `at`
/// 都以同一份纯正文为准，绝不会互相错位。
///
/// 返回（干净文本, [(着重起始字序, 字数, 文本)], [(夹注宿主字序, 注文)]）。
pub type EmphSpan = (usize, usize, String);
pub type GlossMark = (usize, String);
pub fn split_body_marks(s: &str) -> (String, Vec<EmphSpan>, Vec<GlossMark>) {
    let mut out = String::with_capacity(s.len());
    let mut spans = Vec::new();
    let mut notes = Vec::new();
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
        } else if let Some(tail) = rest.strip_prefix("[注]") {
            let (body, after) = match tail.find("[/注]") {
                Some(i) => (&tail[..i], &tail[i + "[/注]".len()..]),
                None => (tail, ""),
            };
            // 宿主下标取「**去掉尾部空白后**的最后一个字」：`[注]` 前面带着
            // 空格时（`尔雅 [注] 二…`），按未去尾空白算的下标会落在空格之后，
            // 下游 `denoise_text` 一 trim 就越界 → 整条夹注连同注疏正文一起
            // 被当无宿主丢掉。绝不能让「去壳」变成「丢肉身」。
            let at = out.trim_end().chars().count().saturating_sub(1);
            let text = body.trim();
            if !text.is_empty() {
                notes.push((at, text.to_string()));
            }
            rest = after;
        } else {
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    (out, spans, notes)
}

pub fn paragraph_of(line: &str) -> Paragraph {
    // 注疏壳先规范化（`[ 注 ]` / `[疏]` / `[/疏]` → `[注]` / `[/注]`）。
    // 只去壳、留肉身：注疏正文一个字都不动，方括号一个都不留。
    // 没有 `[` 时整段借走，一个字节都不多分配（大书导入的热路径）。
    let line = if line.contains('[') {
        std::borrow::Cow::Owned(crate::cleaner::normalize_note_shells(line))
    } else {
        std::borrow::Cow::Borrowed(line)
    };
    let line = line.as_ref();
    let line = strip_digit_marks(line);
    // 一趟摘净着重与夹注（下标同源，见 `split_body_marks`），再摘注音。
    let (clean, spans, notes) = split_body_marks(&line);
    let (clean, rubies) = split_ruby(&clean);
    let mut p = crate::importer::para_of(&clean);
    for (at, text) in notes {
        p.notes.push(Note::new(at, NoteKind::Note, text));
    }
    for (at, len, text) in spans {
        p.notes.push(Note::span(at, len, NoteKind::Emph, text));
    }
    for (at, py) in rubies {
        p.notes.push(Note::span(at, 1, NoteKind::Ruby, py));
    }
    p
}

// ------------------------------------------------------------------
// 四之二、DOCX：zip 包 + word/document.xml
// ------------------------------------------------------------------

/// DOCX（Office Open XML）→ 标准章数组。
///
/// docx 的「壳」就是一个 zip：正文全躺在 `word/document.xml` 里。剥法：
///   * **样式全丢**（字体 / 字号 / 颜色 / 行距）——竖排版式由笺读接管；
///   * 只取两样：`w:p` 段落的文本，与段落的标题级别（`w:outlineLvl` /
///     `w:pStyle`：Heading1 / 标题1 / 大纲级别）；
///   * 段落里的 `<w:br/>`（软换行）与 `<w:tab/>` 归一成空格——中文 docx 常
///     用软换行冒充分段，接起来才是一句完整的话。
pub fn parse_docx(bytes: &[u8]) -> Result<Vec<RawChapter>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| anyhow!("这不是一个能打开的 docx（zip 打不开：{e}）"))?;
    // v1.9.9：脚注表先读（双行夹注在 Word 里常常就是脚注）。包里没有就是
    // 空表，`attach_footnotes` 自然什么也不做——与 `docx::import_docx_into`
    // 那条链同口径，两条路径都要支持，否则 UI 走老路径时注文照样丢。
    let notes = read_footnotes_zip(&mut zip);
    let mut xml = String::new();
    {
        let mut f = zip.by_name("word/document.xml").map_err(|_| {
            anyhow!("包里没有 word/document.xml——是老版 .doc 吧？请另存为 .docx 再导入")
        })?;
        f.read_to_string(&mut xml)
            .map_err(|e| anyhow!("读不出正文：{e}"))?;
    }
    // 哨兵 → `[注]…[/注]`，下游自然落成双行夹注。
    let xml = crate::dclean::attach_footnotes(&xml, &notes);
    Ok(docx_xml_to_chapters(&xml))
}

/// 从已打开的 docx 包里读 `word/footnotes.xml`（没有就是空表，不算错）。
fn read_footnotes_zip<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> std::collections::HashMap<i64, String> {
    let Ok(mut f) = zip.by_name("word/footnotes.xml") else {
        return std::collections::HashMap::new();
    };
    let mut s = String::new();
    if f.read_to_string(&mut s).is_err() {
        return std::collections::HashMap::new();
    }
    crate::dclean::parse_footnotes_xml(&s)
}

/// `w:p` 段落流 → 章数组（标题级 ≤2 开新章，3 级以下当章内小标题段）。
fn docx_xml_to_chapters(xml: &str) -> Vec<RawChapter> {
    let lower = xml.to_ascii_lowercase();
    let mut out: Vec<RawChapter> = Vec::new();
    let mut title = "正文".to_string();
    let mut paras: Vec<String> = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find("<w:p") {
        let open = pos + rel;
        let Some(gt) = lower[open..].find('>') else { break };
        let head_end = open + gt + 1;
        let Some(k) = lower[head_end..].find("</w:p>") else { break };
        let close = head_end + k;
        let text = docx_para_text(&xml[head_end..close]);
        if !text.trim().is_empty() {
            // 标题级别藏在段内 `<w:pPr>` 里（不在 `<w:p …>` 的属性上），
            // 故把整段都交给判定函数。
            let level = docx_head_level(&xml[open..close]);
            match level {
                Some(l) if l <= 2 => {
                    push_chapter(&mut out, &title, &mut paras);
                    title = text;
                }
                _ => paras.push(text),
            }
        }
        pos = close + "</w:p>".len();
    }
    push_chapter(&mut out, &title, &mut paras);
    if out.is_empty() {
        out.push(RawChapter::new("正文", String::new()));
    }
    out.iter_mut().for_each(|c| {
        c.title = classicize(&c.title);
        c.content = classicize(&c.content);
    });
    out
}

/// 取 `<w:t …>文本</w:t>` 里的文本（其余节点一概不要）。
fn docx_para_text(inner: &str) -> String {
    let lower = inner.to_ascii_lowercase();
    let mut out = String::new();
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find("<w:t") {
        let open = pos + rel;
        let Some(gt) = lower[open..].find('>') else { break };
        let start = open + gt + 1;
        let Some(k) = lower[start..].find("</w:t>") else { break };
        out.push_str(&inner[start..start + k]);
        pos = start + k + "</w:t>".len();
    }
    decode_entities(&out.replace(['\t', '\r', '\n'], " "))
        .trim()
        .to_string()
}

/// 段落的标题级别：`w:outlineLvl`（0 起）优先，其次 `w:pStyle`。
///
/// 这两个都是 `<w:pPr>` 里的**子标签**（`<w:pStyle w:val="Heading1"/>`），
/// 不是 `<w:p>` 的属性，故传进来的是整段。
fn docx_head_level(seg: &str) -> Option<u8> {
    let low = seg.to_ascii_lowercase();
    if let Some(v) = docx_attr(&low, "w:outlinelvl") {
        if let Ok(n) = v.trim().parse::<u8>() {
            return Some(n.saturating_add(1));
        }
    }
    if let Some(v) = docx_attr(&low, "w:pstyle") {
        let v = v.trim().to_ascii_lowercase();
        let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
        if v.contains("heading") || v.contains("head") || v.contains("标题") {
            return match digits.parse::<u8>() {
                Ok(n) => Some(n.max(1)),
                Err(_) => Some(1),
            };
        }
        // 光秃秃的「1」「2」也是标题样式（中文 docx 常见）
        if v.len() <= 2 {
            if let Ok(n) = digits.parse::<u8>() {
                if (1..=9).contains(&n) {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// 在一段 XML 里取某个子标签的值：`<w:pStyle w:val="Heading1"/>` → `Heading1`。
fn docx_attr(seg_low: &str, tag: &str) -> Option<String> {
    let found = seg_low.find(&format!("<{tag}"))?;
    let rest = &seg_low[found..];
    let end = rest.find('>')?;
    let inside = &rest[..end];
    let open = inside.find('"')?;
    let after = &inside[open + 1..];
    let close = after.find('"')?;
    Some(after[..close].to_string())
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

/// **单章限长切片**（v1.50.0 流式架构的硬闸门）。
///
/// 一章超过 `limit`（5000 字）就切成两章、三章……**只切在段落边界**：
/// 段是古籍排版的最小完整单位，切开段落会连累段首缩进与夹注的 `offset`
/// 记账（夹注是挂在段内字偏移上的），故宁可让某一章略超上限，也绝不把
/// 一段劈成两半。
///
/// 续章题名沿用原章名——目录里连续几条同名卷目，正是「卷帙浩繁」的本色，
/// 比自造「（续一）」这种现代标记干净。
pub fn split_oversized_chapter(title: &str, paras: Vec<Paragraph>, limit: usize) -> Vec<Chapter> {
    let limit = limit.max(1);
    // 先数总字数：不超限就原样返回，连 clone 都不做。
    let total: usize = paras.iter().map(|p| p.text.chars().count()).sum();
    if total <= limit {
        return vec![Chapter {
            title: title.to_string(),
            paragraphs: paras,
        }];
    }
    let mut out: Vec<Chapter> = Vec::new();
    let mut cur: Vec<Paragraph> = Vec::new();
    let mut n = 0usize;
    for p in paras {
        let c = p.text.chars().count();
        // 手上这章再加这一段就超了 → 先封章，段归下一章。
        // 单段本身超限（罕见：整段无换行的长文）时也只能整段入章，
        // 绝不切段（见上：切段会毁掉夹注的 offset）。
        if n > 0 && n + c > limit {
            out.push(Chapter {
                title: title.to_string(),
                paragraphs: std::mem::take(&mut cur),
            });
            n = 0;
        }
        n += c;
        cur.push(p);
    }
    if !cur.is_empty() {
        out.push(Chapter {
            title: title.to_string(),
            paragraphs: cur,
        });
    }
    out
}

/// 无章节的大书**按字数切片**（v1.50.0）：整本没有一条章题时，按 `limit`
/// 字一刀刀切成等长的卷，免得百万字全压在一章里——那样取回一章就等于
/// 把整本书拉回内存，虚拟化彻底失效。
pub fn slice_by_chars(title: &str, paras: Vec<Paragraph>, limit: usize) -> Vec<Chapter> {
    split_oversized_chapter(title, paras, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- DOCX（zip + word/document.xml）----

    /// 造一个内存里的 docx（只有正文部件的迷你 zip）。
    fn zip_of(name: &str, body: &str) -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            w.start_file(name, SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, body.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn docx_headings_split_chapters_and_text_is_kept() {
        let doc = concat!(
            r#"<w:document xmlns:w="http://x"><w:body>"#,
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>卷一</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>道可道，非常道。</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>第一章</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>名可名，</w:t></w:r><w:r><w:t>非常名。</w:t></w:r></w:p>"#,
            r#"</w:body></w:document>"#,
        );
        let chapters = parse_docx(&zip_of("word/document.xml", doc)).expect("docx 应当解析成功");
        assert_eq!(chapters.len(), 2, "{chapters:?}");
        assert_eq!(chapters[0].title, "卷一");
        assert_eq!(chapters[1].title, "第一章");
        // 同一段里的多个 run 要接成一句，不能断成两半。
        assert_eq!(chapters[1].content, "名可名，非常名。");
        assert_eq!(chapters[0].content, "道可道，非常道。");
    }

    #[test]
    fn docx_outline_level_counts_as_heading() {
        let doc = concat!(
            r#"<w:document xmlns:w="http://x"><w:body>"#,
            r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>自序</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>余少好讀書。</w:t></w:r></w:p>"#,
            r#"<w:p><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:r><w:t>卷上</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>學而時習之。</w:t></w:r></w:p>"#,
            r#"</w:body></w:document>"#,
        );
        let chapters = parse_docx(&zip_of("word/document.xml", doc)).expect("应当解析成功");
        let titles: Vec<&str> = chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["自序", "卷上"]);
    }

    #[test]
    fn non_docx_bytes_fail_with_a_readable_message() {
        // 不是 zip
        let err = parse_docx(b"not a zip at all").expect_err("应当报错");
        assert!(format!("{err}").contains("docx"), "报错要看得出是 docx 的问题：{err}");
        // 是 zip 但没有正文部件（老版 .doc 转的假 docx）
        let err = parse_docx(&zip_of("readme.txt", "hello")).expect_err("应当报错");
        assert!(format!("{err}").contains("document.xml"), "{err}");
    }

    // ---- 标记解析（EPUB 竖排清洗产出的三种标记）----

    #[test]
    fn ruby_mark_attaches_pinyin_to_previous_char() {
        // `[拼音:…]` 的语义是「**前一个字**的读音」，宿主下标取标记前一字。
        let (clean, marks) = split_ruby("玄[拼音:xuán]之又玄");
        assert_eq!(clean, "玄之又玄");
        assert_eq!(marks, vec![(0usize, "xuán".to_string())]);
        // 段首无字：注音无处可挂，丢弃（不留字面方括号）。
        let (clean, marks) = split_ruby("[拼音:o]甲");
        assert_eq!(clean, "甲");
        assert!(marks.is_empty());
    }

    #[test]
    fn ruby_mark_becomes_ruby_note_on_the_host_glyph() {
        use crate::types::NoteKind;
        let p = paragraph_of("玄[拼音:xuán]之又玄");
        assert_eq!(p.text, "玄之又玄", "标记不得进正文");
        let ruby = p
            .notes
            .iter()
            .find(|n| n.kind == NoteKind::Ruby)
            .expect("应挂一条注音注");
        assert_eq!(ruby.at, 0);
        assert_eq!(ruby.text, "xuán");
        assert_eq!(ruby.len, 1);
    }

    /// v1.68.1 防误杀：带空格的 `[ 注 ]` / 疏体 `[疏]` 必须**连同注疏正文**
    /// 落成夹注，绝不能把「注疏内容」当正文印、更不能留方括号在版心。
    #[test]
    fn spaced_note_shell_becomes_inline_note_meat_intact() {
        use crate::types::NoteKind;
        let p = paragraph_of("正文[ 注 ]注疏内容[/注]正文");
        assert_eq!(p.text, "正文正文", "标记不得进正文：{:?}", p.text);
        assert!(!p.text.contains('[') && !p.text.contains(']'), "方括号漏进正文：{:?}", p.text);
        let note = p
            .notes
            .iter()
            .find(|n| n.kind == NoteKind::Note)
            .expect("应落成一条夹注");
        assert_eq!(note.text, "注疏内容", "注疏肉身丢了 / 被动过：{:?}", note.text);

        // 疏体壳同理（疏也是注）。
        let p = paragraph_of("甲[疏]某某曰：某也。[/疏]乙");
        assert!(!p.text.contains('['), "疏体壳漏进正文：{:?}", p.text);
        let note = p.notes.iter().find(|n| n.kind == NoteKind::Note).unwrap();
        assert_eq!(note.text, "某某曰：某也。", "疏文丢了：{:?}", note.text);
    }

    /// v1.68.1 真·内容丢失回归：`[注]` 前面带尾空格时，宿主下标按未去尾空白
    /// 算会越界，整条夹注（**连同注疏正文**）被静默丢掉。去壳绝不能丢肉身。
    #[test]
    fn note_after_trailing_space_keeps_its_body() {
        use crate::types::NoteKind;
        let p = paragraph_of("尔雅 [注] 二落单书名号一段话。");
        assert_eq!(p.text.trim(), "尔雅", "正文被动了：{:?}", p.text);
        let note = p
            .notes
            .iter()
            .find(|n| n.kind == NoteKind::Note)
            .expect("注文被丢了——注疏正文跟着一起没了");
        assert_eq!(note.text, "二落单书名号一段话。", "注疏肉身不全：{:?}", note.text);
        // 没尾空格时下标本来就对，别改坏。
        let p = paragraph_of("甲乙[注]丙丁");
        assert_eq!(p.text, "甲乙");
        assert!(p.notes.iter().any(|n| n.text == "丙丁"));
    }

    #[test]
    fn digit_mark_is_unwrapped_at_ingest() {
        // `[数字:…]` 是跨渲染器约定；竖排引擎本就整串成组，入库时去壳即可。
        assert_eq!(strip_digit_marks("ISBN [数字:978-7-02]"), "ISBN 978-7-02");
        assert_eq!(paragraph_of("编号[数字:978-7-02]").text, "编号978-7-02");
    }

    #[test]
    fn markers_coexist_with_highlights() {
        use crate::types::NoteKind;
        let p = paragraph_of("道[高亮]可[/高亮]道[拼音:dào]");
        assert_eq!(p.text, "道可道");
        assert!(p.notes.iter().any(|n| n.kind == NoteKind::Emph && n.at == 1));
        // 注音下标按「去掉着重标记之后」的口径算，挂在末字（下标 2）。
        let ruby = p.notes.iter().find(|n| n.kind == NoteKind::Ruby).unwrap();
        assert_eq!(ruby.at, 2, "注音挂错字：{:?}", p.notes);
    }

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

    // ── v1.75.0：佛经「品」与行内断行 ─────────────────────────────
    #[test]
    fn chapter_head_knows_pin_and_book_prefix() {
        // 《金刚经》三十二品的真实形态：书名前缀 + 第X品 + 品名。
        assert!(is_chapter_head("金刚经第一品 法会因由分"));
        assert!(is_chapter_head("金刚经第三十二品 应化非真分"));
        assert!(is_chapter_head("第二十九品 威仪寂静分"));
        // 后置序数（佛经另一路写法）
        assert!(is_chapter_head("法会因由分第一"));
        // 传统章回不受影响
        assert!(is_chapter_head("第十二回"));
        assert!(is_chapter_head("第一章"));
        // ⚠️ 越界保护：孤零零一个序数，此前 `cs[i]` 会 panic。
        assert!(!is_chapter_head("第二十九"));
        assert!(!is_chapter_head("第108"));
        // 带句读的一律不是标题（正文里引用章节）
        assert!(!is_chapter_head("《论语》第二十章，子曰"));
        assert!(!is_chapter_head("见第二十章。"));
    }

    /// 尊重原文：品题长什么样，章标题就是什么样——不自作主张剥书名前缀。
    #[test]
    fn chapter_title_keeps_original_text() {
        let raw = "金刚经\n\n金刚经第一品 法会因由分\n\n如是我闻，一时佛在舍卫国祗树给孤独园。\n\n金刚经第二品 善现启请分\n\n时长老须菩提在大众中。";
        let cs = parse_txt(raw);
        let titles: Vec<&str> = cs.iter().map(|c| c.title.as_str()).collect();
        assert!(titles.iter().any(|t| t == &"金刚经第一品 法会因由分"), "{titles:?}");
        assert!(titles.iter().any(|t| t == &"金刚经第二品 善现启请分"), "{titles:?}");
    }

    #[test]
    fn resplit_makes_one_pin_one_chapter() {
        use crate::pipeline::RawChapter;
        // 模拟 EPUB 的产出：解析器只认结构标签，整本书就一章，品题只是普通行。
        let one = RawChapter::new(
            "金刚经",
            "金刚经第一品 法会因由分\n如是我闻，一时佛在舍卫国。\n金刚经第二品 善现启请分\n时长老须菩提在大众中。".to_string(),
        );
        let out = resplit_by_heads(vec![one]);
        assert_eq!(out.len(), 2, "一品一章：{:?}", out.iter().map(|c| c.title.clone()).collect::<Vec<_>>());
        // 尊重原文：标题就是原文那一行，一个字不改。
        assert_eq!(out[0].title, "金刚经第一品 法会因由分");
        assert_eq!(out[1].title, "金刚经第二品 善现启请分");
        assert!(out[0].content.contains("如是我闻"));
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

    // ── v1.9.4：裸文本全角括号 → 双行夹注 ─────────────────────────
    #[test]
    fn txt_bracket_notes_turns_parens_into_gloss() {
        // 短注：正常成对 → [注]…[/注]
        assert_eq!(
            txt_bracket_notes("分，扶問翻；下同。〔惎，毒也。〕尾"),
            "分，扶問翻；下同。[注]惎，毒也。[/注]尾"
        );
        // 校勘记 【…】 同样归一成夹注
        assert_eq!(
            txt_bracket_notes("正文【章：乙十一行本，「六」作「四」。】续"),
            "正文[注]章：乙十一行本，「六」作「四」。[/注]续"
        );
    }

    #[test]
    fn txt_bracket_notes_long_gloss_not_dropped() {
        // 超过旧 80 字上限的长注疏（索隱 / 爾雅引文）现在也落为双行夹注。
        let long = format!("〔{}〕", "甲".repeat(120));
        let out = txt_bracket_notes(&long);
        assert!(out.starts_with("[注]"), "{}", out);
        assert!(out.ends_with("[/注]"), "{}", out);
        assert!(!out.contains('〔'), "{}", out);
    }

    #[test]
    fn txt_bracket_notes_never_panics_on_mismatched() {
        // 左少右多 / 只有开 / 只有闭 都不该 Panic，且标记成对。
        let only_close = txt_bracket_notes("正文〕尾巴");
        assert!(!only_close.contains('〕'));
        assert!(!only_close.contains("[注]")); // 没有 opener 就不开壳
        let only_open = txt_bracket_notes("〔孤零零的注");
        assert!(only_open.starts_with("[注]"));
        assert!(only_open.ends_with("[/注]")); // 结尾补收壳，成对
        let nested = txt_bracket_notes("〔外〔內〕尾〕");
        assert_eq!(nested, "[注]外[注]內[/注]尾[/注]");
    }

    #[test]
    fn txt_long_bracket_note_becomes_gloss_in_parse() {
        let cs = parse_txt(&format!(
            "卷一\n正文一句。{}又一句。",
            format!("〔{}〕", "註".repeat(100))
        ));
        let joined: String = cs.iter().map(|c| c.content.clone()).collect();
        assert!(joined.contains("[注]"), "长括注应转夹注: {}", joined);
        assert!(!joined.contains('〔'), "不应残留全角开括号: {}", joined);
    }

    #[test]
    fn txt_is_title_line_heuristic_rules() {
        // 极短无句读的汉字行 → 标题
        assert!(txt_is_title_line("周紀一"));
        assert!(txt_is_title_line("卷一"));
        // 带句读 / 太长 / 纯数字 / 带括号 → 不是标题
        assert!(!txt_is_title_line("话说天下大势。"));
        assert!(!txt_is_title_line("建安二二三年，策为尚书郎。"));
        assert!(!txt_is_title_line("12345"));
        assert!(!txt_is_title_line("周紀一（起著雍攝提格）"));
        assert!(!txt_is_title_line("这是一句明显超过十四字长度限制的正文内容行"));
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
        // 破折号竖排化：`&mdash;`（`—`）不得横躺在竖排里，收成竖线 ︱。
        assert_eq!(cs[0].content, "甲 乙︱丙“丁”");
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
        // 夹注标签**不进正文**：它只是双行小字的开关，正文里一个字格都不占。
        assert_eq!(p.text, "道可非常道");
        assert_eq!(p.notes.len(), 2);
        // 着重（`[高亮]`）：跨两字的着重号。
        let emph = p.notes.iter().find(|n| n.kind == NoteKind::Emph).unwrap();
        assert_eq!(emph.at, 2);
        assert_eq!(emph.len, 2);
        // 夹注（`[注]`）：挂在前一字「道」上，文本留在注里。
        let note = p.notes.iter().find(|n| n.kind == NoteKind::Note).unwrap();
        assert_eq!(note.at, 4);
        assert_eq!(note.text, "夹注");
    }

    /// 夹注标签只作开关：**绝不占字格**（v1.36）。
    #[test]
    fn note_marks_never_occupy_a_cell() {
        let (clean, notes) = split_notes("黄帝者，少典之子[注]子：后代。[/注]，姓公孙。");
        assert!(!clean.contains('['), "标签残留：{clean}");
        assert!(!clean.contains("注"), "标签残留：{clean}");
        assert_eq!(clean, "黄帝者，少典之子，姓公孙。");
        // 宿主 = 标记前那个字（「子」在正文里的下标 7）。
        assert_eq!(notes, vec![(7usize, "子：后代。".to_string())]);
        // 空夹注连壳丢弃，不留空档。
        let (clean, notes) = split_notes("道[注][/注]可");
        assert_eq!(clean, "道可");
        assert!(notes.is_empty(), "空夹注不该留：{notes:?}");
        // 未闭合的 `[注]` 吞到结尾（不丢字）。
        let (clean, notes) = split_notes("道[注]未尽");
        assert_eq!(clean, "道");
        assert_eq!(notes, vec![(0usize, "未尽".to_string())]);
    }

    /// 破折号 → 竖排竖线（两条链路同源：`vclean::classic_punct` 与
    /// `pipeline::classicize` 都走 `dash_to_vertical`）。
    #[test]
    fn dashes_become_vertical_rule() {
        assert_eq!(crate::vclean::dash_to_vertical("甲——乙"), "甲︱乙");
        assert_eq!(crate::vclean::dash_to_vertical("甲—乙—丙"), "甲︱乙︱丙");
        assert_eq!(crate::vclean::dash_to_vertical("甲－乙"), "甲︱乙");
        assert_eq!(classicize("甲——乙"), "甲︱乙");
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
        assert_eq!(sniff_kind("a.xml", ""), SrcKind::Xml);
        assert_eq!(sniff_kind("noext", "<!DOCTYPE html><p>x</p>"), SrcKind::Html);
        assert_eq!(sniff_kind("noext", "{"), SrcKind::Json);
    }
}
