//! HTML / XHTML：抽正文（EPUB 的每一节也走这里）。
//!
//! 只做一件事：**把标签剥掉、把块级边界还原成段落**。
//! * `script` / `style` / `nav` / `header` / `footer` / `aside` / `head` 整块丢弃
//!   （脚本与导航进正文是电子书里最刺眼的一类脏）；
//! * `<p>` `<div>` `<h1..h6>` `<li>` `<blockquote>` `<tr>` 视为块，块与块之间断行；
//! * `<br>` → 换行；`&nbsp;` → 空格；其余实体按表还原（数字实体直接认码位）。
//!
//! # 为什么不用 scraper
//!
//! 本机 `crates.io` 不通（403），`scraper` / `html5ever` 拉不下来；而 `quick-xml`
//! 本来就在依赖里（EPUB 的 OPF 要用）。HTML 的容错性确实不如 HTML5 解析器，
//! 但电子书里的 XHTML 基本是良构的；真遇到野 HTML，`quick-xml` 会报错，本模块
//! **保留已读到的部分**而不是整本导入失败——丢几个畸形段好过整本书打不开。

use std::io::BufRead;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::traits::FormatParser;
use super::txt_parser::decode_bytes;
use super::{read_streamed, ImportError};

/// HTML 解析器（`.html` / `.htm` / `.xhtml` / `.xml`）。
pub struct HtmlParser;

impl FormatParser for HtmlParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let bytes = read_streamed(path)?;
        let (src, _enc) = decode_bytes(&bytes);
        let (text, err) = extract_html_str(&src);
        if text.trim().is_empty() {
            if let Some(e) = err {
                return Err(ImportError::Xml(e));
            }
        }
        Ok(text)
    }

    fn extensions() -> &'static [&'static str] {
        &["html", "htm", "xhtml", "xml"]
    }
}

/// 从字符串抽正文（EPUB 的每一节都用它）。
pub fn extract_html_str(src: &str) -> (String, Option<String>) {
    extract_html_reader(std::io::Cursor::new(src.as_bytes()))
}

/// 流式抽正文：`BufRead` 进、纯文本出。
///
/// 返回（正文, 解析错误）。**即使出错也返回已读到的部分**：一本 300 节的 EPUB
/// 里有 1 节畸形，不该让整本书打不开。
pub fn extract_html_reader<R: BufRead>(reader: R) -> (String, Option<String>) {
    let mut xml = Reader::from_reader(reader);
    // 不 trim：行内空格是词边界（`<b>道</b> 可道` 里的空格不能丢）。
    xml.config_mut().trim_text(false);

    let mut buf: Vec<u8> = Vec::new();
    let mut out = String::new();
    let mut para = String::new();
    let mut skip = 0usize; // >0 表示正处在丢弃块里（可嵌套）
    let mut err: Option<String> = None;

    // 块级标签：结束时要断段。
    let is_block = |n: &str| -> bool {
        matches!(
            n,
            "p" | "div" | "section" | "article" | "blockquote" | "li" | "ul" | "ol" | "tr"
                | "td" | "th" | "table" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre"
                | "figure" | "figcaption" | "dl" | "dt" | "dd" | "body" | "html"
        )
    };
    // 丢弃块：整块内容都不进正文。
    let is_skip = |n: &str| -> bool {
        matches!(
            n,
            "script" | "style" | "nav" | "header" | "footer" | "aside" | "head" | "noscript"
                | "svg" | "math" | "template"
        )
    };

    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                if is_skip(&name) {
                    skip += 1;
                } else if skip == 0 && is_block(&name) {
                    push_break(&mut para, &mut out);
                }
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref());
                if skip == 0 && matches!(name.as_str(), "br" | "hr") {
                    push_break(&mut para, &mut out);
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                if is_skip(&name) {
                    skip = skip.saturating_sub(1);
                } else if skip == 0 && is_block(&name) {
                    push_break(&mut para, &mut out);
                }
            }
            Ok(Event::Text(t)) => {
                if skip == 0 {
                    let raw = String::from_utf8_lossy(t.as_ref());
                    para.push_str(&unescape_html(&raw));
                }
            }
            Ok(Event::CData(c)) => {
                if skip == 0 {
                    para.push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            Ok(_) => {}
            Err(e) => {
                err = Some(e.to_string());
                break; // 保已读到的部分
            }
        }
        buf.clear();
    }
    push_break(&mut para, &mut out);
    (out, err)
}

/// 段收口：把当前累积的文本压成一段（行内空白归一）后挂到输出。
fn push_break(para: &mut String, out: &mut String) {
    let t = collapse_inline_space(para);
    para.clear();
    if t.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&t);
}

/// 段内空白折叠：连续空格 / 换行 → 一个空格；句首句尾不留白。
///
/// 注意**不动全角空格**：`　　` 是段首缩进记号，折成一个空格就丢了。
fn collapse_inline_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true; // 段首当作已有空格，天然去头
    for c in s.chars() {
        match c {
            ' ' | '\t' | '\n' | '\u{00A0}' | '\u{200B}' => {
                if !prev_space {
                    out.push(' ');
                    prev_space = true;
                }
            }
            _ => {
                out.push(c);
                prev_space = c == '\u{3000}';
            }
        }
    }
    out.trim_end().to_string()
}

/// `ns:tag` → `tag`（XHTML 常带 `xhtml:` 前缀）。
fn local_name(q: &[u8]) -> String {
    let s = String::from_utf8_lossy(q);
    match s.rsplit(':').next() {
        Some(t) => t.to_ascii_lowercase(),
        None => s.to_ascii_lowercase(),
    }
}

/// HTML 实体还原：数字实体按码位、常见命名实体查表，**不认识的原样保留**。
///
/// 不认识就保留而不是删掉：`&foo;` 出现在正文里，读者看得见就知道原文有东西；
/// 删掉则永久丢字。
pub fn unescape_html(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let b: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != '&' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        // 往前找分号（最多 12 个字符，避免把长段落当实体）。
        let end = (i + 1..b.len().min(i + 12)).find(|&j| b[j] == ';');
        let Some(end) = end else {
            out.push('&');
            i += 1;
            continue;
        };
        let name: String = b[i + 1..end].iter().collect();
        let rep = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = name.strip_prefix('#') {
            dec.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            named_entity(&name)
        };
        match rep {
            Some(c) => {
                out.push(c);
                i = end + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// 常见命名实体（够用即可，未知的交给调用方原样保留）。
fn named_entity(n: &str) -> Option<char> {
    Some(match n {
        "nbsp" => '\u{00A0}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{3000}',
        "thinsp" => '\u{2009}',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "laquo" => '«',
        "raquo" => '»',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "middot" => '·',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "deg" => '°',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "sect" => '§',
        "para" => '¶',
        "dagger" => '†',
        "bull" => '•',
        "prime" => '′',
        "Prime" => '″',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "zwsp" => '\u{200B}',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_scripts_nav_and_keeps_paragraphs() {
        let src = r#"<html><head><title>書名</title><style>p{color:red}</style></head>
            <body><nav><a href="/">首页</a></nav>
            <h1>道德經</h1>
            <p>道可道，<b>非常</b>道。</p>
            <script>alert(1)</script>
            <header>页眉</header>
            <p>名可名<br/>非常名&nbsp;也。</p>
            </body></html>"#;
        let (t, e) = extract_html_str(src);
        assert!(e.is_none(), "不该报错：{e:?}");
        assert!(!t.contains("alert"), "脚本漏进正文");
        assert!(!t.contains("页眉"), "页眉漏进正文");
        assert!(!t.contains("color:red"), "CSS 漏进正文");
        assert!(t.contains("道可道，非常道。"), "行内标签没剥干净：{t}");
        // `<br/>` 按规矩转成换行（诗行由此分行），故第 2 段被切成两行。
        assert_eq!(t.lines().count(), 4, "段落/换行数不对：{t:?}");
        assert!(t.contains("非常名 也。"), "&nbsp; 没还原成空格：{t:?}");
    }

    #[test]
    fn unknown_entities_are_kept_verbatim() {
        assert_eq!(unescape_html("&foo;道"), "&foo;道");
        assert_eq!(unescape_html("&#36947;"), "道");
        assert_eq!(unescape_html("&#x9053;"), "道");
    }
}
