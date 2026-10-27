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

/// **朱批录**：全书 `MarginNote` 按**章节顺序**排成一份 Markdown（v1.63.0）。
///
/// 一条批注 = 章节名 + 原文摘录 + 批语。天头批语本身只记「锚在全书第几个
/// 字」，脱离书页就只是一句孤零零的话；故每条都回正文取锚点所在的**那一
/// 句**作摘录——隔几天再看，也知道当时批的是哪句话。
///
/// 排序刻意不按数组顺序：`margin_notes` 是追加的，写入序＝当时读到哪儿，
/// 补批几次就乱了；按 `anchor_char_index` 归章归段升序，才是一份能当读后感
/// 通读的稿子。
pub fn to_annotations_md(book: &Book) -> String {
    // ① 定位并排序；锚点落在书外的（换过书稿、删过章）直接丢，不写孤儿批注。
    //    收两类**用户亲手写的字**：天头朱批（MarginNote）与夹注/通假
    //    （Note::Note / Tongjia）。专名线、着重、注音只是标记，不出文字。
    let mut rows: Vec<(usize, usize, usize, &'static str, String)> = Vec::new();
    for m in book.margin_notes.iter() {
        let t = m.content.trim();
        if t.is_empty() {
            continue;
        }
        if let Some((ci, pi, off)) = locate_char(book, m.anchor_char_index) {
            rows.push((ci, pi, off, "眉批", t.to_string()));
        }
    }
    for (ci, ch) in book.chapters.iter().enumerate() {
        for (pi, p) in ch.paragraphs.iter().enumerate() {
            for n in p.notes.iter() {
                let label = match n.kind {
                    NoteKind::Note => "夹注",
                    NoteKind::Tongjia => "通假",
                    _ => continue,
                };
                let t = n.text.trim();
                if t.is_empty() {
                    continue;
                }
                rows.push((ci, pi, n.at, label, t.to_string()));
            }
        }
    }
    rows.sort_by_key(|(ci, pi, off, _, _)| (*ci, *pi, *off));

    let mut out = String::new();
    out.push_str(&format!("# 《{}》朱批录\n\n", book.title.trim()));
    if rows.is_empty() {
        out.push_str("> 此书尚无朱批。\n\n");
        out.push_str("正文里选中文字 →「眉批」即可写批语，写完再回来导出。\n");
        return out;
    }
    let mut chs = 0usize;
    let mut prev = usize::MAX;
    for (ci, _, _, _, _) in rows.iter() {
        if *ci != prev {
            prev = *ci;
            chs += 1;
        }
    }
    out.push_str(&format!(
        "> 共 {} 条批注 · 分属 {} 章 · 依章节顺序\n",
        rows.len(),
        chs
    ));

    let mut cur = usize::MAX;
    for (ci, pi, off, label, text) in rows {
        if ci != cur {
            cur = ci;
            let title = book
                .chapters
                .get(ci)
                .map(|c| c.title.trim())
                .filter(|t| !t.is_empty())
                .unwrap_or("（无题）");
            out.push_str(&format!("\n---\n\n## {title}\n\n"));
        }
        let para = book
            .chapters
            .get(ci)
            .and_then(|c| c.paragraphs.get(pi))
            .map(|p| p.text.as_str())
            .unwrap_or("");
        let quote = excerpt_at(para, off);
        if !quote.is_empty() {
            out.push_str(&format!("> {quote}\n\n"));
        }
        out.push_str(&format!("**{label}**：{text}\n\n"));
    }
    out
}

/// 全书扁平字符下标 → `(章, 段, 段内偏移)`。
///
/// 口径必须与 `Book::flat_index` 完全一致（章内按 `Paragraph::text` 的字符
/// 数累加）：一旦两处算法分家，摘录就会指到隔壁段去。
fn locate_char(book: &Book, mut idx: usize) -> Option<(usize, usize, usize)> {
    for (ci, ch) in book.chapters.iter().enumerate() {
        for (pi, p) in ch.paragraphs.iter().enumerate() {
            let n = p.text.chars().count();
            if idx < n {
                return Some((ci, pi, idx));
            }
            idx -= n;
        }
    }
    None
}

/// 取「锚点所在的那一句」作摘录：向左右各找到最近的句读，含句末标点。
///
/// 长句按 80 字截断（以锚点为中心），两端补省略号——批注锚的是单字，整段
/// 几百字全抄进来反而看不出批的是哪儿。
fn excerpt_at(text: &str, off: usize) -> String {
    const EXCERPT_MAX: usize = 80;
    let cs: Vec<char> = text.chars().collect();
    if cs.is_empty() {
        return String::new();
    }
    let off = off.min(cs.len() - 1);
    // 竖排正文里句读就是断句处；换行同样是硬边界。
    let brk = |c: char| matches!(c, '。' | '！' | '？' | '；' | '!' | '?' | ';' | '\n');
    let mut s = off;
    while s > 0 && !brk(cs[s - 1]) {
        s -= 1;
    }
    let mut e = off;
    while e < cs.len() && !brk(cs[e]) {
        e += 1;
    }
    if e < cs.len() {
        e += 1;
    }
    if s >= e {
        return String::new();
    }
    if e - s <= EXCERPT_MAX {
        let raw: String = cs[s..e].iter().collect();
        return raw.trim().to_string();
    }
    let half = EXCERPT_MAX / 2;
    let a = s.max(off.saturating_sub(half));
    let b = (a + EXCERPT_MAX).min(e);
    let mut cut = String::new();
    if a > s {
        cut.push('…');
    }
    cut.push_str(&cs[a..b].iter().collect::<String>());
    if b < e {
        cut.push('…');
    }
    cut.trim().to_string()
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
        // 默认文件名：调用方拼好「笺读_书名_第N叶.png」这类名字传进来。
        .set_file_name(file_name)
        .add_filter(filter_name, exts);
    // 起始目录优先级：**上次导出目录 → 桌面 → 交给系统记忆**。
    //
    // NSSavePanel 在没有显式目录时会落到 App 的当前工作目录（从 Finder /
    // Dock 启动时可能是 `/`，从 dist 目录旁启动时甚至是 App 包内部）——
    // 用户直接点「存储」后根本找不到文件，表现为「点了导出但没生成」。
    if let Some(dir) = remembered_export_dir() {
        dlg = dlg.set_directory(dir);
    } else if let Some(desktop) = desktop_dir() {
        dlg = dlg.set_directory(desktop);
    }
    dlg.save_file()
}

/// 桌面目录（保存面板的最后兜底）。
///
/// App Sandbox 下 `HOME` 指向**容器**（`~/Library/Containers/<bundle>/Data`），
/// 容器里的 `Desktop` 是系统建的符号链接，`is_dir()` 跟随链接仍成立；
/// 万一不存在就返回 `None`，让 `NSSavePanel` 退回它自己记住的位置——
/// 不设目录不会出错，只是起点变成系统默认。
fn desktop_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let d = Path::new(&home).join("Desktop");
    if d.is_dir() {
        Some(d)
    } else {
        None
    }
}

// ------------------------------------------------------------------
// 沙盒合规：导出目录记忆（v1.73.0）
// ------------------------------------------------------------------

/// 记「上次导出到哪个目录」的那一行字，存在**应用数据目录**下。
///
/// 为什么落这里而不是真 `~`：App Sandbox 会把
/// `~/Library/Application Support/com.jiandu.app` **透明重定向到容器**，
/// 沙盒内外同一份代码、同一条路径都永远可写、且不需要任何授权；
/// 而真 `~` 下的自定义文件在沙盒里读不到。
fn last_dir_file() -> PathBuf {
    jiandu_core::library::Library::home().join("last_export_dir.txt")
}

/// 上次导出的**所在目录**；没记过、或那个目录已经不在了（外置盘拔了、
/// 用户删了、沙盒里无权访问）就返回 `None`。
///
/// ⚠️ 它只决定保存面板的**起始位置**，不是访问授权——真正的写权限由
/// `NSSavePanel` 每次现场授予（entitlement:
/// `com.apple.security.files.user-selected.read-write`）。所以哪怕这里
/// 返回一个当前无权访问的目录也**不会崩**：面板自己会退回可用位置。
pub fn remembered_export_dir() -> Option<PathBuf> {
    remembered_from(&last_dir_file())
}

/// 记住导出目录：存的是文件**所在目录**，不是文件名本身。
///
/// 失败一律静默——记不住只是下次回到桌面起点，不该打断已经成功的导出。
pub fn remember_export_dir(path: &Path) {
    let Some(dir) = path.parent() else { return };
    remember_to(&last_dir_file(), dir);
}

/// 从记忆文件里读出目录（**纯 I/O，路径由参数给**，故可单测）。
///
/// 抽出来是为了让测试不必去动全局 `JIANDU_HOME`——cargo test 是多线程跑
/// 同一个二进制里的所有用例，谁改了环境变量，别人的 `Library::home()`
/// 就跟着错位（数据目录一旦指错，store / library 的用例会随机红）。
fn remembered_from(file: &Path) -> Option<PathBuf> {
    let s = std::fs::read_to_string(file).ok()?;
    let p = PathBuf::from(s.trim());
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// 把目录写进记忆文件（同样是**纯 I/O**，便于单测）。
fn remember_to(file: &Path, dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let _ = std::fs::write(file, dir.to_string_lossy().as_bytes());
}

/// **写盘唯一出口**：补后缀 → 建父目录 → 写字节 → 记住这个目录。
///
/// 六条导出链路（PNG / 分享图 / MD / 批注 / 卡片 CSV ×2）都走这里，
/// 免得各处手写 `std::fs::write` 时漏掉「建父目录」——用户手敲一个
/// 不存在的子目录就会「导出失败：No such file or directory」。
pub fn save_bytes(picked: PathBuf, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = with_extension(picked, ext);
    write_bytes(&path, bytes)?;
    remember_export_dir(&path);
    Ok(path)
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

/// **朱批录落盘**：`to_annotations_md` → 系统保存框 → 写 `.md`。
///
/// 返回 `Ok(None)` = 用户取消了保存。
pub fn export_annotations_picked(book: &Book, file_name: &str) -> Result<Option<PathBuf>> {
    let Some(picked) = pick_save_path(
        "导出全书批注（Markdown）",
        file_name,
        "Markdown",
        &["md", "markdown"],
    ) else {
        return Ok(None);
    };
    let path = with_extension(picked, "md");
    let body = to_annotations_md(book);
    write_bytes(&path, body.as_bytes()).map(Some)
}

/// **RFC 4180 字段转义**（v1.56.0 Anki 导出）。
///
/// 古籍正文里逗号、引号极常见（「…，…」「…」），不转义的话 Anki 导入
/// 时列会直接错位。规则：字段里出现 逗号 / 双引号 / 换行 之一，整段用
/// 双引号包起来，段内的双引号写成两个。
pub fn csv_field(s: &str) -> String {
    let need = s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r');
    if need {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 拼一行 CSV（两列及以上），末尾补 `\n`。Anki 导入不认表头，故不写。
pub fn csv_line(cells: &[&str]) -> String {
    let mut out = String::new();
    for (i, c) in cells.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&csv_field(c));
    }
    out.push('\n');
    out
}

/// **恒加引号的两列 CSV 行**（v1.63.0 选区制卡）。
///
/// 与 [`csv_line`] 唯一的差别：每个字段**无条件**包双引号（内部引号仍双写）。
/// Anki 两种写法都能吃，但恒加引号能让「正文里逗号成片」的古籍句子在任何
/// 表格工具里都不错列——这是选区制卡唯一可能踩的坑，索性写死。
pub fn csv_two_quoted(front: &str, back: &str) -> String {
    format!(
        "\"{}\",\"{}\"\n",
        front.replace('"', "\"\""),
        back.replace('"', "\"\"")
    )
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

    /// 朱批录：按**章节顺序**排（不是批注数组顺序），每条带回原文摘录。
    #[test]
    fn annotations_follow_chapter_order_with_quotes() {
        use crate::types::MarginNote;
        let mut b = sample();
        // 故意乱序写入：先第二章，再第一章。
        b.margin_notes.push(MarginNote::new(1, 4, "此句最妙"));
        b.margin_notes.push(MarginNote::new(2, 1, "开篇立骨"));
        let md = to_annotations_md(&b);
        let i1 = md.find("## 第一章").expect("应有第一章");
        let i2 = md.find("## 第二章").expect("应有第二章");
        assert!(i1 < i2, "必须按章节顺序而非写入顺序：\n{md}");
        assert!(md.contains("> 甲乙丙"), "应回正文摘出锚点所在句：\n{md}");
        assert!(md.contains("**眉批**：开篇立骨"));
        assert!(md.contains("**眉批**：此句最妙"));
        assert!(md.contains("共 2 条批注 · 分属 2 章"));
    }

    /// 空书 / 孤儿锚点：给一句人话，绝不把错位的批注写到别的句子上。
    #[test]
    fn annotations_report_empty_and_skip_orphans() {
        use crate::types::MarginNote;
        let mut b = sample();
        b.margin_notes
            .push(MarginNote::new(1, 9999, "锚点早已不在书里"));
        b.margin_notes.push(MarginNote::new(2, 0, "   "));
        let md = to_annotations_md(&b);
        assert!(md.contains("尚无朱批"), "{md}");
        assert!(
            !md.contains("锚点早已不在书里"),
            "孤儿批注不该落盘：\n{md}"
        );
    }

    /// 夹注/通假也算批语；超长句按 80 字截断，不把下一句带进来。
    #[test]
    fn annotations_include_glosses_and_trim_long_sentences() {
        use crate::types::{MarginNote, Note, NoteKind};
        let mut b = Book::new("长书", "");
        let mut c = Chapter {
            title: "卷上".into(),
            paragraphs: Vec::new(),
        };
        let long = format!("甲乙丙丁。{}。己庚辛。", "戊".repeat(200));
        let mut p = Paragraph::plain(long);
        p.notes.push(Note {
            kind: NoteKind::Note,
            at: 0,
            len: 1,
            text: "甲者始也".into(),
            source: crate::types::NoteSource::Original,
            anchor: None,
        });
        c.paragraphs.push(p);
        b.chapters.push(c);
        b.margin_notes.push(MarginNote::new(1, 10, "长句之批"));
        let md = to_annotations_md(&b);
        assert!(md.contains("**夹注**：甲者始也"), "夹注也要进录：\n{md}");
        assert!(md.contains("**眉批**：长句之批"));
        assert!(md.contains('…'), "超长句应截断：\n{md}");
        assert!(
            !md.contains("己庚辛"),
            "截断后不该把下一句带进来：\n{md}"
        );
        assert!(md.contains("共 2 条批注"));
    }

    #[test]
    fn csv_field_quotes_only_when_needed() {
        // 全角标点（古籍里的「，」是 U+FF0C）不破坏 CSV 的列结构，不必加引号。
        assert_eq!(csv_field("逍遥游"), "逍遥游");
        assert_eq!(csv_field("北冥有鱼，其名为鲲"), "北冥有鱼，其名为鲲");
        assert_eq!(csv_field("子曰「道」"), "子曰「道」");
        // 半角逗号 / 双引号 / 换行 会错位，整段必须包起来。
        assert_eq!(csv_field("北冥有鱼, 其名为鲲"), "\"北冥有鱼, 其名为鲲\"");
        // 引号加倍：内部每一个 " 写成两个。
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("上\n下"), "\"上\n下\"");
    }

    #[test]
    fn csv_line_keeps_two_columns_even_with_commas() {
        let line = csv_line(&["北冥有鱼, 其名为鲲", "【出处】《庄子·内篇》"]);
        // 逗号只出现在被引号包住的字段里，列数恒为 2。
        assert!(
            line.starts_with("\"北冥有鱼, 其名为鲲\","),
            "含半角逗号的字段必须被双引号包住：{line}"
        );
        assert!(line.ends_with('\n'));
    }

    /// 选区制卡：两列**无条件**包双引号，内部引号双写，列数恒为 2。
    #[test]
    fn csv_two_quoted_always_wraps_both_fields() {
        let line = csv_two_quoted("道可道, 非常道", "【出处】老子·第一章<br>按：此句为全书纲领");
        assert_eq!(
            line,
            "\"道可道, 非常道\",\"【出处】老子·第一章<br>按：此句为全书纲领\"\n"
        );
        // 字段里本来就有双引号：写成两个，外加最外层一对。
        assert_eq!(csv_two_quoted("a\"b", "c"), "\"a\"\"b\",\"c\"\n");
        // 不含任何标点的短字段也照样包——这正是与 `csv_line` 的差别。
        assert_eq!(csv_two_quoted("天", "地"), "\"天\",\"地\"\n");
    }


    // ── v1.73.0 沙盒合规：导出目录记忆 ──────────────────────────────
    #[test]
    fn export_dir_memory_roundtrip() {
        let dir = std::env::temp_dir().join(format!("jiandu-dir-mem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let file = dir.join("mem.txt");
        remember_to(&file, &dir);
        assert_eq!(remembered_from(&file).as_deref(), Some(dir.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 记过的目录没了（外置盘拔了 / 用户删了 / 沙盒里无权访问）→ **静默降级
    /// 为「没记过」**，绝不能把保存面板指到一个死路径上。
    #[test]
    fn export_dir_memory_degrades_when_dir_gone() {
        let base = std::env::temp_dir().join(format!("jiandu-dir-gone-{}", std::process::id()));
        let file = base.join("mem.txt");
        std::fs::create_dir_all(&base).ok();
        std::fs::write(&file, "/nonexistent/jiandu/definitely/not/here").ok();
        assert!(
            remembered_from(&file).is_none(),
            "目录已不在就该当没记过（降级到桌面 / 系统记忆）"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// `save_bytes`：补后缀 + 建父目录——用户手敲一个还不存在的子目录也该成。
    #[test]
    fn save_bytes_adds_extension_and_creates_parent() {
        let orig = remembered_export_dir();
        let base = std::env::temp_dir().join(format!("jiandu-save-{}", std::process::id()));
        // 既没有后缀，父目录也还不存在。
        let dest = base.join("子目录").join("朱批录");
        let got = save_bytes(dest, "csv", b"a,b\n");
        assert!(got.is_ok(), "建父目录后应当写成功：{got:?}");
        let p = got.unwrap();
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("csv"),
            "用户没打后缀时要补上"
        );
        assert!(p.is_file());
        std::fs::remove_dir_all(&base).ok();
        // 副作用回滚：别把用户真实的「上次导出目录」冲掉。
        if let Some(d) = orig {
            remember_to(&last_dir_file(), &d);
        }
    }
}
