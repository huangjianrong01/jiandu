//! EPUB：一个 zip，里面装着 OPF 目录 + 一堆 XHTML。
//!
//! 步骤固定三步，缺一步都不行：
//!   1. `META-INF/container.xml` → 找到 `.opf` 的路径（书脊目录在这里）；
//!   2. 读 OPF 的 `manifest`（id → href）与 `spine`（阅读顺序的 idref 列表）——
//!      **顺序必须照 spine 走**，按文件名排序会把书排乱（`.xhtml` 的编号
//!      常常是倒着的，或者干脆没有编号）；
//!   3. 逐节抽出正文（复用 [`super::html_parser`]），节与节之间断行。
//!
//! 容错：任何一节打不开 / 畸形，**跳过那一节继续**；OPF 彻底读不出来时，
//! 退化成「把 zip 里所有 xhtml 按文件名排序」——总比整本导入失败强。

use std::io::{BufReader, Read};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::html_parser::{extract_html_reader, extract_html_reader_keep_tables};
use super::traits::FormatParser;
use super::ImportError;

/// EPUB 解析器。
pub struct EpubParser;

impl FormatParser for EpubParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let file = std::fs::File::open(path).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| ImportError::Zip(e.to_string()))?;

        // ① 找 .opf
        let mut opf_path = read_container_opf(&mut zip);
        if opf_path.is_none() {
            // 退路：zip 里第一个 .opf
            opf_path = find_first(&mut zip, |n| n.to_ascii_lowercase().ends_with(".opf"));
        }

        // ② spine 给出的节顺序（拿不到就为空，走退路）
        let (opf_dir, sections) = match opf_path.as_deref() {
            Some(opf) => (dir_of(opf), read_opf_spine(&mut zip, opf)),
            None => (String::new(), Vec::new()),
        };

        // ③ 逐节抽正文
        let mut out = String::new();
        let mut used = 0usize;
        for rel in &sections {
            let name = join_zip_path(&opf_dir, rel);
            if let Some(text) = read_section(&mut zip, &name, true) {
                if !text.trim().is_empty() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&text);
                    used += 1;
                }
            }
        }

        // 一节都没抽到 → 退化成「全部 xhtml 按名排序」（仍先弃表）
        if used == 0 {
            let mut names: Vec<String> = (0..zip.len())
                .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
                .filter(|n| is_html_name(n))
                .collect();
            names.sort();
            for name in names {
                if let Some(text) = read_section(&mut zip, &name, true) {
                    if !text.trim().is_empty() {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(&text);
                        used += 1;
                    }
                }
            }
        }

        // ── v1.69.0 退路：整本都用 `<table>` 排版的劣质转换件 ──────────
        // 「表格整块丢弃」是死命令，但真有本子把全部正文塞进表格里，
        // 全本弃表后一个字都不剩。那时**保留表格文字**再抽一遍：排版再丑
        // 也比空书强，且这一步只在「弃表后真的空了」时才跑，绝不影响
        // 正常的本子。
        if used == 0 {
            let mut names: Vec<String> = (0..zip.len())
                .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
                .filter(|n| is_html_name(n))
                .collect();
            names.sort();
            for name in names {
                if let Some(text) = read_section(&mut zip, &name, false) {
                    if !text.trim().is_empty() {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(&text);
                        used += 1;
                    }
                }
            }
        }

        if used == 0 {
            return Err(ImportError::Xml(
                "整本没有一个能读的 XHTML 节（可能是加密 EPUB 或纯图片扫描版）".to_string(),
            ));
        }
        Ok(out)
    }

    fn extensions() -> &'static [&'static str] {
        &["epub"]
    }
}

/// 读 `META-INF/container.xml`，取出第一个 `rootfile` 的 `full-path`。
fn read_container_opf(zip: &mut zip::ZipArchive<std::fs::File>) -> Option<String> {
    let mut f = zip.by_name("META-INF/container.xml").ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    let mut xml = Reader::from_str(&s);
    xml.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local(&e.name().as_ref().to_vec()) == "rootfile" {
                    for a in e.attributes().flatten() {
                        if local(&a.key.as_ref().to_vec()) == "full-path" {
                            let v = a.unescape_value().ok()?.to_string();
                            return Some(v);
                        }
                    }
                }
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

/// 读 OPF 的 spine 顺序，返回相对 OPF 所在目录的 href 列表。
fn read_opf_spine(zip: &mut zip::ZipArchive<std::fs::File>, opf: &str) -> Vec<String> {
    let Ok(mut f) = zip.by_name(opf) else {
        return Vec::new();
    };
    let mut s = String::new();
    if f.read_to_string(&mut s).is_err() {
        return Vec::new();
    }
    // manifest: id → href
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut spine: Vec<String> = Vec::new();
    let mut xml = Reader::from_str(&s);
    xml.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let tag = local(&e.name().as_ref().to_vec());
                if tag == "item" {
                    let (mut id, mut href) = (String::new(), String::new());
                    for a in e.attributes().flatten() {
                        let k = local(&a.key.as_ref().to_vec());
                        let v = a.unescape_value().map(|s| s.to_string()).unwrap_or_default();
                        match k.as_str() {
                            "id" => id = v,
                            "href" => href = v,
                            _ => {}
                        }
                    }
                    if !id.is_empty() && !href.is_empty() {
                        map.insert(id, href);
                    }
                } else if tag == "itemref" {
                    for a in e.attributes().flatten() {
                        if local(&a.key.as_ref().to_vec()) == "idref" {
                            if let Ok(v) = a.unescape_value() {
                                spine.push(v.to_string());
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    spine
        .into_iter()
        .filter_map(|id| map.get(&id).cloned())
        .map(|h| percent_decode(&h))
        .collect()
}

/// 打开 zip 里的某一节并抽正文（流式：`BufReader` 直接喂给 XML 解析器）。
///
/// `drop_tables = true` 时**整块丢弃 `<table>`**（v1.69.0 死命令）；
/// `false` 是「弃表后整本空了」的救回退路。
fn read_section(
    zip: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
    drop_tables: bool,
) -> Option<String> {
    if !is_html_name(name) {
        return None;
    }
    let f = zip.by_name(name).ok()?;
    let (text, _err) = if drop_tables {
        extract_html_reader(BufReader::new(f))
    } else {
        extract_html_reader_keep_tables(BufReader::new(f))
    };
    Some(text)
}

fn is_html_name(n: &str) -> bool {
    let l = n.to_ascii_lowercase();
    l.ends_with(".xhtml") || l.ends_with(".html") || l.ends_with(".htm")
}

/// zip 内路径拼接（`OEBPS/content.opf` 相对的是 `OEBPS/`）。
fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..=i].to_string(),
        None => String::new(),
    }
}

fn join_zip_path(dir: &str, rel: &str) -> String {
    let rel = rel.trim_start_matches("./");
    if dir.is_empty() {
        return rel.to_string();
    }
    format!("{dir}{rel}")
}

fn find_first<F: Fn(&str) -> bool>(
    zip: &mut zip::ZipArchive<std::fs::File>,
    pred: F,
) -> Option<String> {
    (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.name().to_string()))
        .find(|n| pred(n))
}

fn local(q: &[u8]) -> String {
    let s = String::from_utf8_lossy(q);
    s.rsplit(':').next().unwrap_or("").to_ascii_lowercase()
}

/// `%E7%AC%AC` → `第`（OPF 里的 href 常带百分号转义）。
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
