//! DOCX：又是一个 zip，正文在 `word/document.xml`。
//!
//! Word 的正文结构很朴素：`<w:body>` 里一串 `<w:p>`（段落），每个段落里
//! 一串 `<w:r>`（run），run 的有效文字在 `<w:t>` 里。所以只认三件事：
//!
//! * `w:p` 的起止 = 段落边界；
//! * `w:t` 的文本 = 正文（**必须先解实体**：`&amp;` 不解开就是字面的 `&amp;`）；
//! * `w:br` / `w:tab` = 换行 / 空格；
//!
//! 另外要**主动跳过**两类东西，否则会把隐性内容排进书里：
//! * `w:instrText`（域代码，如 `PAGE \* MERGEFORMAT`）；
//! * `w:delText`（修订模式下被删除的文字）。
//!
//! # 为什么不用 docx-rs
//!
//! 本机 `crates.io` 不通，`docx-rs` 拉不下来；而它本身是**写** docx 的库，
//! 读文档要经过它自己的模型，反而多一层不确定性。这里用 `zip` + `quick-xml`
//! 直接读 OOXML——少一层依赖，也少一层「读到一半发现模型不全」的风险。

use std::io::{BufReader, Read};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::html_parser::unescape_html;
use super::traits::FormatParser;
use super::ImportError;

/// DOCX 解析器。
pub struct DocxParser;

impl FormatParser for DocxParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let file = std::fs::File::open(path).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| ImportError::Zip(e.to_string()))?;

        // 正文可能被拆成多个 part（`document.xml` + `header*.xml`…），
        // 但阅读用的正文只认主文档；主文档缺失就是坏包。
        let mut entry = zip
            .by_name("word/document.xml")
            .map_err(|_| ImportError::MissingEntry("word/document.xml".to_string()))?;
        let mut xml = String::new();
        entry
            .read_to_string(&mut xml)
            .map_err(|e| ImportError::Zip(e.to_string()))?;
        drop(entry);

        Ok(extract_docx_text(&xml))
    }

    fn extensions() -> &'static [&'static str] {
        &["docx"]
    }
}

/// 从 `word/document.xml` 的 XML 文本里抽段落（纯函数，好测）。
pub fn extract_docx_text(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf: Vec<u8> = Vec::new();

    let mut out: String = String::new();
    let mut para = String::new();
    // 域代码 / 已删除文字：整段跳过（它们不是读者要看的字）。
    let mut skip = 0usize;
    let mut saw_any = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                match local(e.name().as_ref()).as_str() {
                    // 域代码 / 修订删除：整段跳过。
                    "instrtext" | "deltext" | "del" => skip += 1,
                    "br" if skip == 0 => para.push('\n'),
                    "tab" if skip == 0 => para.push(' '),
                    _ => {}
                }
            }
            // 自闭合的空元素（`<w:br/>` 最常见）：不改变 skip 状态。
            Ok(Event::Empty(e)) => {
                match local(e.name().as_ref()).as_str() {
                    "br" if skip == 0 => para.push('\n'),
                    "tab" if skip == 0 => para.push(' '),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = local(e.name().as_ref());
                if matches!(name.as_str(), "instrtext" | "deltext" | "del") {
                    skip = skip.saturating_sub(1);
                } else if name == "p" {
                    flush(&mut para, &mut out, &mut saw_any);
                }
            }
            Ok(Event::Text(t)) => {
                if skip == 0 {
                    let raw = String::from_utf8_lossy(t.as_ref());
                    para.push_str(&unescape_html(&raw));
                }
            }
            Ok(_) => {}
            Err(_) => break, // 截断的包：保已读到的部分
        }
        buf.clear();
    }
    flush(&mut para, &mut out, &mut saw_any);
    if !saw_any {
        String::new()
    } else {
        out
    }
}

fn flush(para: &mut String, out: &mut String, saw_any: &mut bool) {
    let t = para.trim();
    if !t.is_empty() {
        *saw_any = true;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(t);
    }
    para.clear();
}

fn local(q: &[u8]) -> String {
    let s = String::from_utf8_lossy(q);
    s.rsplit(':').next().unwrap_or("").to_ascii_lowercase()
}

/// 只读一遍文件、不做全量的流式版本（给超大 docx 用；目前 parse 走内存版）。
///
/// 留着是因为 docx 动辄几十 MB 解压后过百 MB：真遇到时把 `parse` 里的
/// `read_to_string` 换成它即可，接口不变。
#[allow(dead_code)]
pub fn extract_docx_streaming<R: Read>(r: R) -> Result<String, ImportError> {
    let mut reader = Reader::from_reader(BufReader::new(r));
    reader.config_mut().trim_text(false);
    let mut buf: Vec<u8> = Vec::new();
    let mut out = String::new();
    let mut para = String::new();
    let mut saw = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                if local(e.name().as_ref()) == "p" {
                    flush(&mut para, &mut out, &mut saw);
                }
            }
            Ok(Event::End(e)) => {
                if local(e.name().as_ref()) == "p" {
                    flush(&mut para, &mut out, &mut saw);
                }
            }
            Ok(Event::Empty(e)) => {
                let _ = local(e.name().as_ref());
            }
            Ok(Event::Text(t)) => {
                para.push_str(&unescape_html(&String::from_utf8_lossy(t.as_ref())));
            }
            Ok(_) => {}
            Err(e) => return Err(ImportError::Xml(e.to_string())),
        }
        buf.clear();
    }
    flush(&mut para, &mut out, &mut saw);
    Ok(out)
}
