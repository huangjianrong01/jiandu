//! 导出：Book -> JSON / 带目录的 TXT / Markdown。
//!
//! 用途：把 epub 这类「结构复杂、体积大」的书转成便于二次编辑与检索的文本，
//! 或者反过来，把 txt / md 转成带封面、目录、注释的 json 工程文件。
//!
//!   * json —— 完整保留 cover / toc / chapters / notes / tongjia，回到笺读读最完整；
//!   * txt  —— 顶部先列目录，正文按章分节；注释以〔注〕形式随文附在段后；
//!   * md   —— 标题用 `#` / `##` 表达层级，便于再导入或被别的工具读取。

use crate::types::{Book, NoteKind};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 序列化为完整 JSON（含目录、注释、通假表）。
pub fn to_json(book: &Book) -> Result<String> {
    serde_json::to_string_pretty(book).context("序列化 JSON 失败")
}

/// 生成带目录的纯文本。
///
/// 结构：
/// ```text
/// 《书名》
/// 作者：xxx
/// 全书 3 章 · 约 5000 字
///
/// ——— 目录 ———
/// 第一章　道可道
/// 第二章　天下皆知
///
/// ——— 正文 ———
/// 第一章　道可道
///
/// 道可道，非常道。……
/// ```
pub fn to_txt(book: &Book) -> String {
    let mut out = String::new();
    out.push_str(&format!("《{}》\n", book.title));
    if !book.author.trim().is_empty() {
        out.push_str(&format!("作者：{}\n", book.author.trim()));
    }
    out.push_str(&format!(
        "全书 {} 章 · 约 {} 字\n",
        book.chapters.len(),
        book.total_chars()
    ));

    out.push_str("\n——— 目录 ———\n\n");
    for e in &book.toc {
        let pad = "    ".repeat(e.level.saturating_sub(1));
        out.push_str(&format!("{pad}{}\n", e.title));
    }

    out.push_str("\n——— 正文 ———\n");
    for ch in &book.chapters {
        out.push_str(&format!("\n{}\n\n", ch.title));
        for p in &ch.paragraphs {
            out.push_str(&p.text);
            out.push('\n');
            // 注释随段输出，读文本时不必回翻。
            for n in &p.notes {
                let tag = match n.kind {
                    NoteKind::Note => "注",
                    NoteKind::Tongjia => "通假",
                    NoteKind::Person => "人名",
                    NoteKind::Place => "地名",
                    NoteKind::Book => "书名",
                };
                out.push_str(&format!("〔{tag}〕{}\n", n.text));
            }
            out.push('\n');
        }
    }
    out
}

/// 生成 Markdown：章节用二级标题，层级目录用列表缩进。
pub fn to_markdown(book: &Book) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", book.title));
    if !book.author.trim().is_empty() {
        out.push_str(&format!("> 作者：{}\n\n", book.author.trim()));
    }

    out.push_str("## 目录\n\n");
    for e in &book.toc {
        let pad = "    ".repeat(e.level.saturating_sub(1));
        out.push_str(&format!("{pad}- {}\n", e.title));
    }
    out.push_str("\n---\n\n");

    for ch in &book.chapters {
        out.push_str(&format!("## {}\n\n", ch.title));
        for p in &ch.paragraphs {
            out.push_str(&p.text);
            out.push_str("\n\n");
        }
    }
    out
}

/// 按扩展名导出。返回实际写出的路径。
pub fn export_book(book: &Book, out: &Path) -> Result<PathBuf> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let ext = out
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let body = match ext.as_str() {
        "json" => to_json(book)?,
        "md" | "markdown" => to_markdown(book),
        _ => to_txt(book),
    };
    std::fs::write(out, body).with_context(|| format!("写入失败：{}", out.display()))?;
    Ok(out.to_path_buf())
}

/// 导入再导出：一条命令做格式转换。
pub fn convert(input: &Path, out: &Path) -> Result<PathBuf> {
    let book = crate::importer::import_from_path(input)?;
    export_book(&book, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Book, Chapter, Paragraph, TocEntry};

    fn sample() -> Book {
        let mut b = Book::new("测试书", "某人");
        let mut c1 = Chapter {
            title: "第一章".into(),
            paragraphs: Vec::new(),
        };
        c1.paragraphs.push(Paragraph::plain("甲乙丙"));
        let mut c2 = Chapter {
            title: "第二章".into(),
            paragraphs: Vec::new(),
        };
        c2.paragraphs.push(Paragraph::plain("丁戊己"));
        b.chapters.push(c1);
        b.chapters.push(c2);
        b.toc = vec![
            TocEntry::new("第一章", 0, 1),
            TocEntry::new("第一节", 0, 2),
            TocEntry::new("第二章", 1, 1),
        ];
        b
    }

    #[test]
    fn txt_has_toc_section() {
        let t = to_txt(&sample());
        assert!(t.contains("——— 目录 ———"));
        assert!(t.contains("——— 正文 ———"));
        assert!(t.contains("    第一节")); // 二级缩进
        assert!(t.contains("甲乙丙"));
    }

    #[test]
    fn md_headings() {
        let m = to_markdown(&sample());
        assert!(m.starts_with("# 测试书"));
        assert!(m.contains("## 第一章"));
    }
}
