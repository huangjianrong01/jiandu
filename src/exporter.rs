//! 导出：Book -> JSON / 带目录的 TXT / Markdown。
//!
//! 用途：把 epub 这类「结构复杂、体积大」的书转成便于二次编辑与检索的文本，
//! 或者反过来，把 txt / md 转成带封面、目录、注释的 json 工程文件。
//!
//!   * json —— 完整保留 cover / toc / chapters / notes / tongjia，回到笺读读最完整；
//!   * txt  —— 顶部先列目录，正文按章分节；注释以〔注〕形式随文附在段后；
//!   * md   —— 标题用 `#` / `##` 表达层级，便于再导入或被别的工具读取。

use crate::types::{Book, Note, NoteKind};
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
                    // 着重（导入时 `**加粗**` 转来的朱砂圈点）：导出还原成
                    // markdown 的加粗，round-trip 不丢标记。
                    NoteKind::Emph => "着重",
                    // 注音（EPUB `<ruby>` 导入）：导出成「拼音」条，不回写标记
                    // ——正文里注音是格内附属物，独立成行更好读。
                    NoteKind::Ruby => "拼音",
                };
                out.push_str(&format!("〔{tag}〕{}\n", n.text));
            }
            out.push('\n');
        }
    }
    out
}

/// 生成 Markdown：章节用二级标题，层级目录用列表缩进。
///
/// 正文里的注释（注 / 通假 / 人名 / 地名 / 书名 / 拼音）转成标准 Markdown
/// 脚注 `[^1]`，定义集中在每章末尾；着重（朱砂圈点）保留为 `**加粗**`。
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

    let mut footnote_counter: usize = 0;
    for (ci, ch) in book.chapters.iter().enumerate() {
        out.push_str(&format!("## {}\n\n", ch.title));
        let mut defs: Vec<(usize, String)> = Vec::new();
        for p in &ch.paragraphs {
            let (body, mut fs) = md_with_footnotes(&p.text, &p.notes, &mut footnote_counter);
            out.push_str(&body);
            out.push_str("\n\n");
            defs.append(&mut fs);
        }
        if !defs.is_empty() {
            out.push_str("**脚注**\n\n");
            for (n, d) in defs {
                out.push_str(&format!("[^{n}]: {d}\n"));
            }
            out.push('\n');
        }
        // 天头眉批（v1.37.0）：按章收在这一章末尾——批语钉在某一面上，
        // 导出成纯文本没有「面」这个概念，挂在所属章下最不容易走失。
        let heads: Vec<&str> = book
            .head_notes
            .iter()
            .filter(|n| n.key.0 == ci)
            .map(|n| n.text.as_str())
            .collect();
        if !heads.is_empty() {
            out.push_str("**眉批**\n\n");
            for h in heads {
                out.push_str(&format!("- {h}\n"));
            }
            out.push('\n');
        }
    }
    out
}

/// 把一段正文里的注释转成行内 `[^n]` 标记，并返回本段用到的脚注定义。
///
/// 仅处理带位置（`at`/`len`）的注释；无位置的（导入缺位）退化为随文
/// `〔tag〕text`，不丢信息。`Emph` 仍走加粗，不当脚注。
fn md_with_footnotes(
    text: &str,
    notes: &[Note],
    counter: &mut usize,
) -> (String, Vec<(usize, String)>) {
    let cs: Vec<char> = text.chars().collect();
    // 按位置排序，避免乱序插入错位。
    let mut placed: Vec<&Note> = notes
        .iter()
        .filter(|n| n.len > 0 && n.at < cs.len())
        .collect();
    placed.sort_by_key(|n| n.at);

    let mut out = String::with_capacity(text.len() + placed.len() * 6);
    let mut defs: Vec<(usize, String)> = Vec::new();
    let mut i = 0usize;
    for n in placed {
        // 先把光标到注释起点之间的正文原样写出。
        while i < n.at {
            out.push(cs[i]);
            i += 1;
        }
        match n.kind {
            NoteKind::Emph => {
                out.push_str("**");
                let hi = (n.at + n.len).min(cs.len());
                out.extend(cs[n.at..hi].iter());
                out.push_str("**");
            }
            _ => {
                *counter += 1;
                let tag = note_tag(n.kind);
                out.push_str(&format!("[^{}]", *counter));
                defs.push((*counter, format!("{tag}{}", n.text)));
            }
        }
        i = (n.at + n.len).min(cs.len());
    }
    // 余下正文。
    while i < cs.len() {
        out.push(cs[i]);
        i += 1;
    }
    (out, defs)
}

/// 注释种类的脚注标签。
fn note_tag(kind: NoteKind) -> String {
    match kind {
        NoteKind::Note => "注：".to_string(),
        NoteKind::Tongjia => "通假：".to_string(),
        NoteKind::Person => "人名：".to_string(),
        NoteKind::Place => "地名：".to_string(),
        NoteKind::Book => "书名：".to_string(),
        NoteKind::Ruby => "拼音：".to_string(),
        NoteKind::Emph => String::new(),
    }
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

// ------------------------------------------------------------------
// 原生保存对话框（rfd）与「另存为」写盘
// ------------------------------------------------------------------

/// 弹出**系统原生**保存对话框，返回用户选中的路径。
///
/// `None` = 用户点了「取消」——这是**正常结果，不是错误**，调用方必须原样
/// 吞掉（顶多提示一句「已取消」），绝不能 `unwrap` / `expect`，否则取消
/// 一次就 panic 闪退。
///
/// # ⚠️ 必须在主线程调用（v1.52.4 血的教训）
///
/// rfd 0.14.1 的 macOS 后端：同步 `save_file()` → `run_on_main` → 若当前
/// 不在主线程，走 `dispatch::Queue::main().exec_sync(panel.runModal)`。
/// GPUI 主线程此刻正忙着自己的事件循环 / 渲染，`dispatch_sync` 的 block
/// 要么根本排不上、要么嵌套 modal session 与 GPUI run loop 不兼容导致
/// `runModal` 立刻返回非 1 —— 两种结果都是 `save_file()` 静默返回
/// `None`，调用方只当「用户取消」，于是**点了导出什么都没发生、也不报错**。
///
/// 正确做法：在 GPUI **前台 executor**（`cx.spawn` 的 async 体本身就在
/// 主线程被 poll）里直接调用本函数——`is_main_thread()` 为 true，
/// `run_on_main` 直接同步跑模态面板，与原生 App 行为一致。
/// **不要**再包一层 `cx.background_executor().spawn(...)`。
pub fn pick_save_path(title: &str, file_name: &str, filter_name: &str, exts: &[&str]) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new()
        .set_title(title)
        // 默认文件名：调用方拼好「笺卡_书名_页码.png」这类名字传进来。
        .set_file_name(file_name)
        .add_filter(filter_name, exts);
    // 默认目录兜底：定位到用户桌面。NSSavePanel 在没有显式目录时会落到
    // App 的当前工作目录（从 Finder /  Dock 启动时可能是 `/`，从 dist
    // 目录旁启动时甚至是 App 包内部）——用户直接点「存储」后根本找不到
    // 文件，表现为「点了导出但没生成」。
    if let Some(home) = std::env::var_os("HOME") {
        let desktop = Path::new(&home).join("Desktop");
        if desktop.is_dir() {
            dlg = dlg.set_directory(desktop);
        }
    }
    dlg.save_file()
}

/// 把路径**补上**指定扩展名（用户手打了不带后缀的名字时兜底）。
///
/// 对话框在某些系统上不会自动补后缀，缺后缀会让后面按扩展名分派的写盘
/// 逻辑走错分支（例如 .md 被当成 txt 导出）。
pub fn with_extension(path: PathBuf, ext: &str) -> PathBuf {
    if path.extension().and_then(|e| e.to_str()).is_some() {
        path
    } else {
        let mut p = path.into_os_string();
        p.push(".");
        p.push(ext);
        PathBuf::from(p)
    }
}

/// 写字节到 `path`：**先建父目录再写**，失败只返回 `Err`，不 panic。
pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<PathBuf> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(path, bytes).with_context(|| format!("写入失败：{}", path.display()))?;
    Ok(path.to_path_buf())
}

/// 拷一份文件（`from` → `to`）：PNG 导出走「先截到临时文件、再按用户选的
/// 位置落盘」两步，这一步就是第二步。
pub fn copy_file(from: &Path, to: &Path) -> Result<PathBuf> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::copy(from, to).with_context(|| format!("保存失败：{}", to.display()))?;
    Ok(to.to_path_buf())
}

/// 书名 → 安全的文件名主干：去掉路径分隔符与控制字符，并给长度设上限。
///
/// 书名来自 epub 元数据，可能带 `/`、`:`、换行——直接拼进默认文件名会让
/// 保存对话框把路径拐到别处，甚至写到意料之外的目录。
pub fn safe_file_stem(title: &str) -> String {
    let mut s: String = title
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r'))
        .collect();
    s = s.trim().to_string();
    if s.is_empty() {
        return "未命名".to_string();
    }
    // 文件名有 255 字节上限，中文按 3 字节算，取 60 字足够安全。
    let n = s.chars().count();
    if n > 60 {
        s = s.chars().take(60).collect();
    }
    s
}

/// 导出 Markdown 到用户选定的路径（正文 + `[^n]` 脚注）。
///
/// 返回 `Ok(None)` = 用户取消了保存；`Ok(Some(path))` = 已写出。
pub fn export_markdown_picked(book: &Book, title: &str, file_name: &str) -> Result<Option<PathBuf>> {
    let Some(picked) = pick_save_path(title, file_name, "Markdown", &["md", "markdown"]) else {
        return Ok(None);
    };
    let path = with_extension(picked, "md");
    let body = to_markdown(book);
    write_bytes(&path, body.as_bytes()).map(Some)
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

    #[test]
    fn md_uses_footnotes_for_notes() {
        use crate::types::{Note, NoteKind};
        let mut b = Book::new("注书", "某");
        let mut c = Chapter {
            title: "章一".into(),
            paragraphs: Vec::new(),
        };
        let mut p = Paragraph::plain("关关雎鸠");
        // 在「雎」字处挂一条注释（at=2, len=1）。
        p.notes.push(Note {
            kind: NoteKind::Note,
            at: 2,
            len: 1,
            text: "雎鸠，水鸟也".into(),
            source: crate::types::NoteSource::Original,
            anchor: None,
        });
        c.paragraphs.push(p);
        b.chapters.push(c);
        let m = to_markdown(&b);
        assert!(m.contains("[^1]"), "正文应出现脚注引用");
        assert!(m.contains("[^1]: 注：雎鸠，水鸟也"), "应出现脚注定义");
        assert!(m.contains("**脚注**"), "章末应有脚注区");
    }
}
