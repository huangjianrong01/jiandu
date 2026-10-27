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
///
/// v1.69.0：**结构化提取，绝不无脑遍历**。开源古籍 JSON 里 `pinyin`
/// `book` `volume` `author` 这些字段和 `content` 平级躺着，旧版「认不出
/// 就递归翻一遍」会把它们连同正文一起收进来——于是拼音被**一个字母一格**
/// 竖着排成人间惨剧。现在只认键名：正文键收正文，其余一律当元数据丢掉。
pub fn extract_json(src: &str) -> String {
    match serde_json::from_str::<Value>(src) {
        Ok(v) => {
            let mut paras: Vec<String> = Vec::new();
            collect(&v, &mut paras);
            // 一个字都没抠到：说明这份 JSON 的正文键我们没认出来（小众
            // schema）。这时**退回宽松遍历**兜底——有字胜过无字，反正
            // 下游 `cleaner` 还会把拼音 / 站名再洗一遍。
            if paras.is_empty() {
                collect_lenient(&v, &mut paras);
            }
            paras.join("\n")
        }
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

/// 正文键（认得出就用这个顺序）。
const BODY_KEYS: [&str; 11] = [
    "content", "text", "paragraphs", "body", "chapters", "paragraph", "sentences", "lines",
    // 中文 schema 里常见的正文键。
    "原文", "正文", "内容",
];

/// 题名键（先出标题再出正文，章名才不会掉到章尾）。
const TITLE_KEYS: [&str; 6] = ["title", "name", "标题", "篇", "章", "章题"];

fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        // 走到这里的 `Value::String` 只可能是**正文键**递下来的（顶层裸串
        // 同理），元数据键在下面已经被拦掉了。
        Value::String(s) => push_para(out, s),
        Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Value::Object(m) => {
            let body = find_body(m);
            if let Some(b) = body {
                // ① 有正文键：先出题名（章名不能掉到章尾），再出正文。
                //    其余键（pinyin / book / volume / author …）**一律不收**。
                if let Some(Value::String(ts)) = find_title(m) {
                    push_para(out, ts);
                }
                collect(b, out);
                return;
            }
            // ② 没有正文键：只往下翻**数组 / 对象**——不认识的键名下的
            //    字符串一律丢弃（那正是「佚名」「卷三」「lùn yǔ」这类
            //    元数据混进正文的入口，v1.69.0）。
            for (k, val) in m {
                if is_meta_key(k) {
                    continue;
                }
                match val {
                    Value::Array(_) | Value::Object(_) => collect(val, out),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// 兜底遍历（只在**严格提取一无所获**时才跑）：认不出键名就把所有字符串
/// 都捞一遍。宁可脏一点，也不能整本导入成空白。
fn collect_lenient(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => push_para(out, s),
        Value::Array(a) => a.iter().for_each(|x| collect_lenient(x, out)),
        Value::Object(m) => {
            for (k, val) in m {
                if is_meta_key(k) {
                    continue;
                }
                collect_lenient(val, out);
            }
        }
        _ => {}
    }
}

/// 只在顶层对象里找正文键（大小写不敏感）。
fn find_body(m: &serde_json::Map<String, Value>) -> Option<&Value> {
    BODY_KEYS.iter().find_map(|k| get_ci(m, k))
}

fn find_title(m: &serde_json::Map<String, Value>) -> Option<&Value> {
    TITLE_KEYS.iter().find_map(|k| get_ci(m, k))
}

fn get_ci<'a>(m: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(v) = m.get(key) {
        return Some(v);
    }
    m.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// 元信息字段：**绝不进正文流**。
///
/// v1.69.0 大扩军：开源古籍 JSON 的元数据字段多到离谱（`pinyin`、
/// `book`、`volume`、`author`、`translator`…），旧表只认得 `id`/`author`
/// 那几个，剩下的全被当正文收进来了——竖排里就是「拼音字母竖着排」、
/// 「佚名」占一列、「卷三」混在正文中间。
fn is_meta_key(k: &str) -> bool {
    matches!(
        k.to_ascii_lowercase().as_str(),
        "id" | "uuid" | "isbn" | "author" | "authors" | "meta" | "metadata" | "toc" | "cover"
            | "language" | "lang" | "date" | "publisher" | "version" | "created" | "updated"
            | "url" | "source" | "site" | "tags" | "icon" | "css" | "style"
            // —— 以下 v1.69.0 新增：古籍 JSON 的元数据四件套 ——
            | "pinyin" | "py" | "pron" | "pronunciation" | "phonetic" | "zhuyin" | "注音"
            | "拼音" | "romaji" | "tone" | "tones"
            | "book" | "books" | "书名" | "volume" | "vol" | "juan" | "卷" | "卷名"
            | "chapter_no" | "chapter_num" | "num" | "index" | "idx" | "order" | "序号"
            | "translator" | "dynasty" | "editor" | "annotator" | "series" | "category"
            | "keywords" | "commentary" | "annotation" | "note" | "notes" | "注释" | "译文"
            | "translation" | "meaning" | "explain" | "shiyi" | "english" | "en"
            | "simplified" | "traditional"
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
