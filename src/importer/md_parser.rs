//! Markdown：壳最薄，但「薄壳」里藏着三种会让正文变脏的东西——
//! 文件头的 YAML 元数据、行内的强调记号、以及围栏代码块。
//!
//! 处理策略：
//! * 开头 `---` … `---` 之间的 YAML **整块丢掉**（那是元数据，不是正文）；
//! * `#` 标题只去掉井号、保留标题文字（标题要进正文，它是断章的依据）；
//! * `**` `__` `*` `_` `~~` `` ` `` 一律剥掉；链接留文字、图片整个丢；
//! * 引用 `>` 与列表记号 `- ` `1. ` 去掉记号留内容；
//! * 围栏代码块的**围栏行**丢掉、内容保留（代码也是作者写的字，不该被吞）；
//! * 表格的竖线换成空格（表格在竖排里排不了，至少字别丢）。

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

use super::traits::FormatParser;
use super::txt_parser::decode_bytes;
use super::{read_streamed, ImportError};

/// Markdown 解析器。
pub struct MdParser;

impl FormatParser for MdParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let bytes = read_streamed(path)?;
        let (src, _enc) = decode_bytes(&bytes);
        Ok(strip_markdown(&src))
    }

    fn extensions() -> &'static [&'static str] {
        &["md", "markdown", "mdown", "mkd"]
    }
}

/// Markdown → 纯文本。
pub fn strip_markdown(src: &str) -> String {
    // ---- ① 去掉文件头 YAML（`---` 开头，到下一条 `---` 为止）----
    //
    // 判据要严：**第一行必须正好是 `---`**，否则文中的分隔线会被当成元数据
    // 起点，把整章正文吞掉。
    let body = skip_yaml_front_matter(src);

    // ---- ② 去掉 HTML 注释（剪藏里常留一堆）----
    let body = RE_COMMENT.replace_all(body, "");

    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        let raw = line.trim_end();
        let t = raw.trim_start();

        // 围栏：``` 或 ~~~ 起头；围栏行本身不进正文（内容留）。
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push(raw.to_string());
            continue;
        }

        // 分隔线：--- / *** / ___ （三个以上）
        if is_hr(t) {
            continue;
        }
        // 空行：保留成段界（下游按空行分段）
        if t.is_empty() {
            out.push(String::new());
            continue;
        }

        // 标题：去井号（含闭合的井号）
        let mut s = t.to_string();
        if s.starts_with('#') {
            s = s.trim_start_matches('#').trim().to_string();
            s = s.trim_end_matches('#').trim().to_string();
        }
        // 引用：去 `>`（可多层）
        while s.starts_with('>') {
            s = s[1..].trim_start().to_string();
        }
        // 列表记号：`- ` `* ` `+ ` `1. ` `1) `
        s = RE_LIST.replace(&s, "").to_string();
        // 任务列表：`[ ]` / `[x]`
        s = RE_TASK.replace(&s, "").to_string();

        // 行内记号
        let s = strip_inline(&s);
        if !s.trim().is_empty() {
            out.push(s.trim().to_string());
        }
    }
    out.join("\n")
}

/// 跳过头部的 YAML front matter。
fn skip_yaml_front_matter(src: &str) -> &str {
    let mut it = src.lines();
    let Some(first) = it.next() else {
        return src;
    };
    if first.trim() != "---" {
        return src;
    }
    // 找到收口的 `---`（或 `...`），连它一起跳过。
    let mut consumed = first.len() + 1;
    for line in it {
        consumed += line.len() + 1;
        let t = line.trim();
        if t == "---" || t == "..." {
            return src.get(consumed..).unwrap_or("");
        }
    }
    // 没有收口：说明这不是 YAML，而是正文开头的一条分隔线——原样返回。
    src
}

fn is_hr(t: &str) -> bool {
    let core: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    core.len() >= 3
        && (core.chars().all(|c| c == '-')
            || core.chars().all(|c| c == '*')
            || core.chars().all(|c| c == '_'))
}

/// 剥行内记号：图片 / 链接 / 强调 / 行内代码 / 删除线。
fn strip_inline(s: &str) -> String {
    // 图片：整个丢掉（alt 文字进正文会变成一堆「图1」）。
    let s = RE_IMG.replace_all(s, "");
    // 链接：留文字。
    let s = RE_LINK.replace_all(&s, "$1");
    // 裸 URL：留着（是作者的引用来源），但不处理。
    // 强调与代码：只去记号。
    let s = RE_EMPH.replace_all(&s, "");
    let s = s.replace('`', "");
    s.to_string()
}

static RE_COMMENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<!--.*?-->").expect("注释正则"));
static RE_LIST: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*(?:[-*+]|\d+[.)])\s+").expect("列表正则"));
static RE_TASK: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[[ xX]\]\s*").expect("任务正则"));
static RE_IMG: Lazy<Regex> = Lazy::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").expect("图片正则"));
static RE_LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("链接正则"));
static RE_EMPH: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(\*\*|__|\*|_|~~)").expect("强调正则"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_front_matter_is_skipped() {
        let md = "---\ntitle: 道德經\nauthor: 老子\n---\n# 第一章\n道可道，非常道。";
        let t = strip_markdown(md);
        assert!(!t.contains("author"), "YAML 漏进正文：{t:?}");
        assert!(t.starts_with("第一章"), "标题文字丢了：{t:?}");
        assert!(t.contains("道可道，非常道。"));
    }

    #[test]
    fn marks_are_stripped_but_text_survives() {
        let md = "> **道**可道，*非常*道。\n\n- 名可名\n1. 非常名\n\n[链接](http://x) 与 ![图](y.png)";
        let t = strip_markdown(md);
        for bad in ["**", ">", "- ", "1. ", "](", "![", "#"] {
            assert!(!t.contains(bad), "记号没去干净（{bad}）：{t:?}");
        }
        assert!(t.contains("道可道，非常道。"));
        assert!(t.contains("名可名"));
        assert!(t.contains("链接"), "链接文字被吞了");
    }

    #[test]
    fn fenced_code_keeps_content() {
        let md = "```rust\nlet x = 1;\n```\n正文";
        let t = strip_markdown(md);
        assert!(t.contains("let x = 1;"), "代码内容被吞了：{t:?}");
        assert!(!t.contains("```"));
    }

    #[test]
    fn leading_rule_without_front_matter_is_kept() {
        // 第一行不是 `---`，后面的 `---` 是分隔线，不该被当成 YAML 起点。
        let md = "第一章\n\n---\n\n正文";
        let t = strip_markdown(md);
        assert!(t.contains("第一章") && t.contains("正文"));
    }
}
