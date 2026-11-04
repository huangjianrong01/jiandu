//! JSON：书稿 / 剪藏 / 备份都可能长成 JSON，而**每家的键名都不一样**。
//!
//! 本文件只负责**字节 → 字符串**（编码识别 + 语法坏掉时的正则抢救）。
//! 结构怎么认、注疏怎么转 `[注]…[/注]`、嵌套多深才降级为纯文本——那是
//! [`jiandu_core::jsonkit`] 的事，core 导入路径与本路径**共用同一份实现**，
//! 同一条脏数据从书架进来和从工作台进来不会得到两本书。

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use super::traits::FormatParser;
use super::txt_parser::decode_bytes;
use super::{read_streamed, ImportError};

/// JSON 解析器。
pub struct JsonParser;

impl FormatParser for JsonParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let bytes = read_streamed(path)?;
        let (src, _enc) = decode_bytes(&bytes);
        Ok(extract_json(&src))
    }

    fn extensions() -> &'static [&'static str] {
        &["json"]
    }
}

/// 从 JSON 文本里抠出正文（语法坏了也不报错，能救多少救多少）。
///
/// v1.9.77：**正文抽取本身不在这个文件里了**——统一交给
/// [`jiandu_core::jsonkit`]（智能 JSON 兼容层）。这里只剩「字节 → 字符串」与
/// 「语法坏掉时的正则抢救」两件事。
///
/// 为什么不再自己写一份：同一套键名表在 core 与 app 各存一份，迟早会分叉
/// （core 那边已经会认注疏字段、会往下递归四层，app 这份还停在「只认第一层 +
/// 把 `note` 当元数据丢掉」）。两处口径必须**同一个函数**，否则同一份 JSON 从
/// 书架进来和从工作台进来会得到两本书。
pub fn extract_json(src: &str) -> String {
    match serde_json::from_str::<Value>(src) {
        Ok(v) => jiandu_core::jsonkit::flatten_text(&v),
        // 整份不成立：正则 Salvage（只认带正文语义的键）。
        Err(_) => salvage_by_regex(src).join("\n"),
    }
}

/// **结构化提取元数据**（v1.69.0）：书名 / 作者。
///
/// 开源古籍 JSON 的 `book` / `author` / `volume` 是正经元数据，既不该被
/// 丢掉（那样书名就退化成文件名了），也绝不能混进正文流（旧版就是这么
/// 干的——「佚名」「卷三」堂而皇之地排在正文里）。这里单独提出来交给
/// 调用方当书名 / 作者用。
///
/// 找不到就返回 `None`，由调用方退回文件名 / 正文识别。
pub fn extract_json_meta(src: &str) -> (Option<String>, Option<String>) {
    let Ok(v) = serde_json::from_str::<Value>(src) else {
        return (None, None);
    };
    // 先看顶层，顶层没有再往下翻一层（很多书把元数据包在 `meta` 里）。
    let mut title: Option<String> = None;
    let mut author: Option<String> = None;
    if let Value::Object(m) = &v {
        scan_meta(m, &mut title, &mut author);
        if title.is_none() || author.is_none() {
            if let Some(Value::Object(mm)) = get_ci(m, "meta").or_else(|| get_ci(m, "metadata")) {
                scan_meta(mm, &mut title, &mut author);
            }
        }
    }
    (
        title.map(|t| jiandu_core::vclean::clean_book_title(&t)),
        author.map(|a| jiandu_core::vclean::tidy_meta_title(&a)),
    )
}

/// 从一个对象里认书名 / 作者（**只认键名**，绝不把值当正文收）。
fn scan_meta(
    m: &serde_json::Map<String, Value>,
    title: &mut Option<String>,
    author: &mut Option<String>,
) {
    if title.is_none() {
        for k in ["book", "title", "name", "书名", "标题"] {
            if let Some(Value::String(s)) = get_ci(m, k) {
                let s = s.trim();
                if !s.is_empty() {
                    *title = Some(s.to_string());
                    break;
                }
            }
        }
    }
    if author.is_none() {
        for k in ["author", "authors", "creator", "作者"] {
            if let Some(x) = get_ci(m, k) {
                let s = match x {
                    Value::String(s) => s.trim().to_string(),
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join("、"),
                    _ => String::new(),
                };
                if !s.is_empty() {
                    *author = Some(s);
                    break;
                }
            }
        }
    }
}

/// 只在顶层对象里按大小写不敏感取键（元数据认书名的几十行仍在用）。
fn get_ci<'a>(m: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(v) = m.get(key) {
        return Some(v);
    }
    m.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// 收一段（只给下面的正则抢救用）：去掉首尾空白、压掉纯空白段。
///
/// 正常路径的段落清洗在 `jiandu_core::jsonkit` 里（含 HTML 标签与实体处理），
/// 这里只管「半截文件里捞出来的裸字符串」。
fn push_para(out: &mut Vec<String>, s: &str) {
    let t = s.trim();
    if t.is_empty() {
        return;
    }
    for line in t.split('\n') {
        let l = line.trim();
        if !l.is_empty() {
            out.push(l.to_string());
        }
    }
}

/// 语法坏掉时的抢救：正则捞 `"content": "…"` / `"text": "…"` / `"paragraphs": [...]`。
fn salvage_by_regex(src: &str) -> Vec<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#""(?:content|text|paragraphs|body)"\s*:\s*"((?:[^"\\]|\\.)*)""#)
            .expect("正则写错了")
    });
    let mut out = Vec::new();
    for cap in RE.captures_iter(src) {
        let Some(raw) = cap.get(1) else { continue };
        // 用 serde_json 反解转义（\n / \" / \uXXXX 都能还原），失败就当原文用。
        let lit = format!("\"{}\"", raw.as_str());
        let s = serde_json::from_str::<String>(&lit).unwrap_or_else(|_| raw.as_str().to_string());
        push_para(&mut out, &s);
    }
    // 截断的文件：最后一个值的**收尾引号根本不存在**，上面那条正则匹配不到。
    // 这里补一条「到文件末尾」的兜底，把剩下的字捞出来。
    if out.is_empty() {
        static TAIL: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r#""(?:content|text|paragraphs|body)"\s*:\s*"([^"]*)$"#)
                .expect("正则写错了")
        });
        if let Some(cap) = TAIL.captures(src.trim_end()) {
            if let Some(raw) = cap.get(1) {
                push_para(&mut out, raw.as_str());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_content_text_and_paragraphs() {
        let j = r#"{"title":"道德經","content":"道可道，非常道。\n名可名，非常名。"}"#;
        let t = extract_json(j);
        assert!(t.starts_with("道德經"), "标题没排在最前：{t:?}");
        assert!(t.contains("道可道，非常道。"));
        assert!(t.contains("名可名，非常名。"));
    }

    #[test]
    fn nested_chapters_keep_order() {
        let j = r#"{"chapters":[{"title":"第一章","content":"甲"},{"title":"第二章","content":"乙"}]}"#;
        let t = extract_json(j);
        assert_eq!(t, "第一章\n甲\n第二章\n乙");
    }

    #[test]
    fn broken_json_is_salvaged() {
        let j = r#"{"content":"道可道","text":"非常道""#; // 少了右括号
        let t = extract_json(j);
        assert!(t.contains("道可道"), "坏 JSON 一个字都没救回来：{t:?}");
    }

    #[test]
    fn paragraphs_array_of_strings() {
        let j = r#"{"paragraphs":["一","二",""],"id":"x"}"#;
        assert_eq!(extract_json(j), "一\n二");
    }

    /// v1.69.0：开源古籍 JSON 的元数据污染——拼音 / 作者 / 卷名绝不进正文。
    #[test]
    fn metadata_fields_never_leak_into_body() {
        let j = r#"{
            "book": "論語",
            "volume": "卷三",
            "author": "佚名",
            "pinyin": "lùn yǔ",
            "chapter_no": 3,
            "content": "子曰：學而時習之，不亦說乎？"
        }"#;
        let t = extract_json(j);
        assert!(t.contains("子曰：學而時習之，不亦說乎？"), "正文丢了：{t:?}");
        assert!(!t.contains("lùn"), "拼音漏进正文：{t:?}");
        assert!(!t.contains("yu"), "拼音漏进正文：{t:?}");
        assert!(!t.contains("佚名"), "作者漏进正文：{t:?}");
        assert!(!t.contains("論語"), "书名漏进正文：{t:?}");
        assert!(!t.contains("卷三"), "卷名漏进正文：{t:?}");
    }

    /// 嵌套的章数组：每章只收题名 + 正文，元数据照丢。
    #[test]
    fn nested_chapters_drop_their_metadata() {
        let j = r#"{"chapters":[
            {"title":"學而","pinyin":"xué ér","author":"佚名","content":"子曰：學而時習之。"},
            {"title":"為政","pinyin":"wéi zhèng","content":"子曰：為政以德。"}
        ]}"#;
        let t = extract_json(j);
        assert_eq!(t, "學而\n子曰：學而時習之。\n為政\n子曰：為政以德。", "实际：{t:?}");
    }

    /// 认不出正文键时不能整本变空：退回宽松遍历兜底。
    #[test]
    fn unknown_schema_falls_back_to_lenient_scan() {
        let j = r#"{"weird_key":"道可道，非常道。"}"#;
        let t = extract_json(j);
        assert!(t.contains("道可道，非常道。"), "兜底遍历没救回正文：{t:?}");
    }
}
