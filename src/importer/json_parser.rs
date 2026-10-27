//! JSON：书稿 / 剪藏 / 备份都可能长成 JSON，而**每家的键名都不一样**。
//!
//! 所以这里不用 `serde` 反序列化成固定结构（一条字段对不上就整本失败），
//! 而是走 `serde_json::Value` **容错遍历**：
//!
//! * 认得 `content` / `text` / `paragraphs` / `body` / `chapters` 这几组常见键；
//! * 对象里同时有 `title` 与正文键时，**先出标题再出正文**（章名不能掉到章尾）；
//! * 认不出的对象就递归往下翻——多收不漏；
//! * 整份 JSON 语法就坏了（截断、缺右括号）时，退化成正则捞 `"content": "..."`
//!   的字符串值，能救多少救多少——这就是「忽略所有解析失败的节点」。

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
pub fn extract_json(src: &str) -> String {
    match serde_json::from_str::<Value>(src) {
        Ok(v) => {
            let mut paras: Vec<String> = Vec::new();
            collect(&v, &mut paras);
            paras.join("\n")
        }
        // 整份不成立：正则 Salvage（只认带正文语义的键）。
        Err(_) => salvage_by_regex(src).join("\n"),
    }
}

/// 正文键（认得出就用这个顺序）。
const BODY_KEYS: [&str; 5] = ["content", "text", "paragraphs", "body", "chapters"];

fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => push_para(out, s),
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(m) => {
            // ① 有标题 + 正文：标题先出（这是「章名在章末」的根因）。
            let title = get_ci(m, "title").or_else(|| get_ci(m, "name"));
            let body = find_body(m);
            if let (Some(t), Some(b)) = (title, body) {
                if let Value::String(ts) = t {
                    push_para(out, ts);
                }
                collect(b, out);
                return;
            }
            // ② 只有正文。
            if let Some(b) = body {
                collect(b, out);
                return;
            }
            // ③ 都不认识：把值递归翻一遍，但跳过纯元信息字段。
            for (k, val) in m {
                if is_meta_key(k) {
                    continue;
                }
                collect(val, out);
            }
        }
        _ => {}
    }
}

/// 只在顶层对象里找正文键（大小写不敏感）。
fn find_body(m: &serde_json::Map<String, Value>) -> Option<&Value> {
    BODY_KEYS.iter().find_map(|k| get_ci(m, k))
}

fn get_ci<'a>(m: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(v) = m.get(key) {
        return Some(v);
    }
    m.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// 元信息字段：不当作正文，避免把 `id`/`uuid` 收进书里。
fn is_meta_key(k: &str) -> bool {
    matches!(
        k.to_ascii_lowercase().as_str(),
        "id" | "uuid" | "isbn" | "author" | "authors" | "meta" | "metadata" | "toc" | "cover"
            | "language" | "lang" | "date" | "publisher" | "version" | "created" | "updated"
            | "url" | "source" | "site" | "tags" | "icon" | "css" | "style"
    )
}

/// 收一段：去掉首尾空白、压掉纯空白段；内部换行原样保留（一段里可以有多句）。
fn push_para(out: &mut Vec<String>, s: &str) {
    let t = s.trim();
    if t.is_empty() {
        return;
    }
    // 一坨里如果自带换行，按行拆开——下游按行断段。
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
}
