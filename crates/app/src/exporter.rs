//! 导出：Book -> JSON / 带目录的 TXT / Markdown。
//!
//! 用途：把 epub 这类「结构复杂、体积大」的书转成便于二次编辑与检索的文本，
//! 或者反过来，把 txt / md 转成带封面、目录、注释的 json 工程文件。
//!
//!   * json —— 完整保留 cover / toc / chapters / notes / tongjia，回到笺读读最完整；
//!   * txt  —— 顶部先列目录，正文按章分节；注释以〔注〕形式随文附在段后；
//!   * md   —— 标题用 `#` / `##` 表达层级，便于再导入或被别的工具读取。

use crate::types::{Book, Note, NoteKind};
use jiandu_core::cards::{Card, CardKind};
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

/// 移动（改名）一个已存在的文件（`src` → `to`）：流式 PDF 导出先写到临时
/// 文件、再按用户选的位置落盘，这一步就是第二步。
///
/// 与 `copy_file` 不同，这里用 `rename` 原地挪动（临时文件与目标同卷时近乎
/// 原子、且不复制字节），省掉一份整本 PDF 的拷贝开销。补后缀、建父目录、
/// 记住导出目录都复用既有逻辑。
pub fn move_file(src: &Path, dest: PathBuf, ext: &str) -> Result<PathBuf> {
    let path = with_extension(dest, ext);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::rename(src, &path).with_context(|| {
        format!("保存失败：{} → {}", src.display(), path.display())
    })?;
    remember_export_dir(&path);
    Ok(path)
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

// ------------------------------------------------------------------
// Obsidian / Anki 卡片（v1.81.0 · P3 模块三）
// ------------------------------------------------------------------
//
// 用户拍板：**一卡一 md**（不是一本书一个 md）。理由是对的——Obsidian 的
// 价值在双链与原子笔记，一卷长文塞进一个档就只是一个「文档」，拆成一卡
// 一档才成为可被别的笔记 `[[ ]]` 引用的**节点**。
//
// 落盘口径不变：仍然只走 [`write_bytes`] / [`save_bytes`] 那一个出口，
// 绝不在这里手写 `std::fs::write`。

/// Anki 制卡的三种形状。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnkiKind {
    /// **挖空**：原文里最该背的那个字包成 `{{c1::字}}`（默认）。
    Cloze,
    /// **问答**：正面首句，背面全句 + 批注。
    Qa,
    /// **释义**：正面难字，背面读音 + 批注。
    Gloss,
}

impl AnkiKind {
    pub const ALL: [AnkiKind; 3] = [AnkiKind::Cloze, AnkiKind::Qa, AnkiKind::Gloss];

    pub fn label(self) -> &'static str {
        match self {
            AnkiKind::Cloze => "挖空",
            AnkiKind::Qa => "问答",
            AnkiKind::Gloss => "释义",
        }
    }

    /// 导入 Anki 时要选的笔记类型——CSV 里写不进去，只能告诉用户。
    ///
    /// 挖空必须挂 **Cloze** 类型，否则 `{{c1::}}` 会被当成普通文本，卡片
    /// 正面就是一行带花括号的字——这是「导进去了却不生效」的头号原因。
    pub fn note_type(self) -> &'static str {
        match self {
            AnkiKind::Cloze => "Cloze（挖空）",
            _ => "Basic（基础）",
        }
    }
}

/// epoch 毫秒 → 公历 `(年, 月, 日)`（Howard Hinnant 的 civil-from-days）。
///
/// 不引 chrono：这十行算术就够了，而且可以被单测钉死（含闰年 / 跨年）。
fn ymd(ms: i64) -> (i32, u32, u32) {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe.div_euclid(1460) + doe.div_euclid(36_524) - doe.div_euclid(146_096))
        .div_euclid(365); // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe.div_euclid(4) - yoe.div_euclid(100)); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = (if m <= 2 { y + 1 } else { y }) as i32;
    (y, m, d)
}

/// YAML 双引号字符串：`"` 与 `\` 必须转义，否则 frontmatter 直接解析失败。
fn yaml_str(s: &str) -> String {
    let esc: String = s
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '"' => "\\\"".to_string(),
            '\n' | '\r' => " ".to_string(),
            _ => c.to_string(),
        })
        .collect();
    format!("\"{esc}\"")
}

/// Obsidian 标签：空格 / `#` / `,` / `[` / `]` / `/` 换成 `-`。
///
/// 标签里带空格会被切成两个标签，带 `#` 会让 Obsidian 把后面整段当成嵌套
/// 层级——书名来自 epub 元数据，什么都可能带，必须洗。
fn tag_of(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_whitespace() || matches!(c, '#' | ',' | '[' | ']' | '/' | '"' | '\'') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let t = t.trim_matches('-').to_string();
    if t.is_empty() {
        "未标注".to_string()
    } else {
        t
    }
}

/// 卡片标题：取原文第一句（≤24 字），没有句读就截断。
fn card_title(c: &Card) -> String {
    let cs: Vec<char> = c.quote.chars().collect();
    let end = cs
        .iter()
        .position(|x| matches!(x, '。' | '！' | '？' | '；' | '!' | '?' | ';' | '\n'))
        .map(|i| i + 1)
        .unwrap_or(cs.len());
    let head: String = cs[..end.min(cs.len())].iter().collect();
    let head = head.trim().to_string();
    if head.is_empty() {
        "摘录".to_string()
    } else if head.chars().count() > 24 {
        format!("{}…", head.chars().take(24).collect::<String>())
    } else {
        head
    }
}

/// **难字注音行**：只给生僻字注音——与 App 内「生僻字注音」**同一把尺子**
/// （`analysis::is_rare_char` + `dict::auto_pinyin`）。
///
/// 全句注音会把卡片变成一张拼音表，反而没法读；只注难字才是「拐杖」的
/// 本分。一个难字都没有就返回空串，调用方据此整段不写。
fn gloss_line(text: &str) -> String {
    let mut seen: Vec<char> = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    for c in text.chars() {
        if !jiandu_core::analysis::is_rare_char(c) || seen.contains(&c) {
            continue;
        }
        seen.push(c);
        if let Some(p) = jiandu_core::dict::auto_pinyin(c) {
            parts.push(format!("{c} {p}"));
        }
    }
    parts.join(" · ")
}

/// **一张卡一份 Markdown**（Obsidian 原子笔记）。
///
/// ```text
/// ---
/// date / book / chapter / author / tags / source
/// ---
/// # 首句
/// ## 原文 … ## 注音 … ## 朱批 …
/// [[书名]] · [[章名]]     ← 双链的落点
/// ```
pub fn to_card_md(c: &Card, author: &str) -> String {
    let (y, m, d) = ymd(c.created_ms);
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("date: {y:04}-{m:02}-{d:02}\n"));
    out.push_str(&format!("book: {}\n", yaml_str(&c.book)));
    if !c.chapter.trim().is_empty() {
        out.push_str(&format!("chapter: {}\n", yaml_str(&c.chapter)));
    }
    if !author.trim().is_empty() {
        out.push_str(&format!("author: {}\n", yaml_str(author.trim())));
    }
    out.push_str("tags:\n");
    let kind_tag = match c.kind {
        CardKind::Quote => "笺读/摘录",
        CardKind::Ai => "笺读/问笺",
    };
    out.push_str(&format!("  - {kind_tag}\n"));
    out.push_str(&format!("  - {}\n", tag_of(&c.book)));
    // 回跳锚点：书还在架上时，凭它能在笺读里找回这一处。
    out.push_str(&format!(
        "source: {}\n",
        yaml_str(&format!(
            "jiandu://{}/{}/{}/{}",
            c.store_id, c.key.0, c.key.1, c.key.2
        ))
    ));
    out.push_str("---\n\n");

    out.push_str(&format!("# {}\n\n", card_title(c)));

    out.push_str("## 原文\n\n");
    out.push_str(&c.quote.trim());
    out.push_str("\n\n");

    let gloss = gloss_line(&c.quote);
    if !gloss.is_empty() {
        out.push_str("## 注音\n\n");
        out.push_str(&gloss);
        out.push_str("\n\n");
    }

    let note = c.note.trim();
    if !note.is_empty() {
        out.push_str("## 朱批\n\n");
        out.push_str(note);
        out.push_str("\n\n");
    }

    // 双链：书名与章名各一枚，于是「按书聚合」「按章聚合」都成立。
    out.push_str("---\n\n");
    let book_link = if c.book.trim().is_empty() {
        String::new()
    } else {
        format!("[[{}]]", c.book.trim())
    };
    let chap_link = if c.chapter.trim().is_empty() {
        String::new()
    } else {
        format!("[[{}]]", c.chapter.trim())
    };
    match (book_link.is_empty(), chap_link.is_empty()) {
        (true, true) => {}
        (false, true) => out.push_str(&format!("{book_link}\n")),
        (true, false) => out.push_str(&format!("{chap_link}\n")),
        (false, false) => out.push_str(&format!("{book_link} · {chap_link}\n")),
    }
    out
}

/// 卡片 → 文件名主干：`20261028-庄子-逍遥游-北冥有鱼`。
///
/// 日期打头是刻意的：Obsidian 的文件浏览器按名字排序，日期打头＝入匣
/// 顺序，扫一眼就知道最近读了什么。
pub fn card_file_name(c: &Card) -> String {
    let (y, m, d) = ymd(c.created_ms);
    let head: String = c.quote.chars().take(12).collect();
    let stem = safe_file_stem(&format!("{}-{}-{}", c.book, c.chapter, head));
    let stem = stem.replace('·', "-");
    format!("{y:04}{m:02}{d:02}-{stem}")
}

/// 同名就加序号，绝不覆盖用户已有的笔记。
///
/// 两张卡抄了同一句、同一天入匣——文件名会撞。覆盖等于静默丢一篇笔记，
/// 这是最不可接受的错误。
fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for i in 2..999 {
        let mut p = path.clone();
        p.set_file_name(format!("{stem}-{i}.md"));
        if !p.exists() {
            return p;
        }
    }
    path
}

// ── Anki ──────────────────────────────────────────────────────────────────

/// **挖空**：把原文里「最该背的那个字」包成 `{{c1::字}}`。
///
/// 挑字的规矩：**生僻字优先**——常用字谁都认得，挖了没有训练价值；
/// 一句里没有生僻字就挖正中那个字（位置确定，不随机，故可单测、可复现）。
pub fn anki_cloze_front(text: &str) -> String {
    let cs: Vec<char> = text.chars().collect();
    if cs.is_empty() {
        return text.to_string();
    }
    let idx = cs
        .iter()
        .position(|c| jiandu_core::analysis::is_rare_char(*c))
        .unwrap_or(cs.len() / 2);
    let mut out = String::new();
    for (i, c) in cs.iter().enumerate() {
        if i == idx {
            out.push_str(&format!("{{{{c1::{c}}}}}"));
        } else {
            out.push(*c);
        }
    }
    out
}

/// 一张卡 → `(正面, 背面)`，形状由 [`AnkiKind`] 定。
pub fn anki_cells(c: &Card, kind: AnkiKind) -> (String, String) {
    let src = c.source();
    let gloss = gloss_line(&c.quote);
    let note = c.note.trim();
    let tail = {
        // 背面固定三段：出处 → 注音（有就给）→ 朱批（有就给）。
        let mut t = format!("【出处】{src}");
        if !gloss.is_empty() {
            t.push_str(&format!("<br>【注音】{gloss}"));
        }
        if !note.is_empty() {
            t.push_str(&format!("<br>【朱批】{note}"));
        }
        t
    };
    match kind {
        AnkiKind::Cloze => (anki_cloze_front(&c.quote), tail),
        AnkiKind::Qa => {
            // 正面只给首句，背面给全句——「下一句是什么」是最合背诵的问法。
            let cs: Vec<char> = c.quote.chars().collect();
            let end = cs
                .iter()
                .position(|x| matches!(x, '。' | '！' | '？' | '；' | '!' | '?' | ';'))
                .map(|i| i + 1)
                .unwrap_or(cs.len());
            let first: String = cs[..end].iter().collect();
            let front = format!("{first}<br><br>（下句为何？）<br><small>{src}</small>");
            let back = format!("{}<br>{}", c.quote.trim(), tail);
            (front, back)
        }
        AnkiKind::Gloss => {
            // 正面就那一枚难字；没有难字时退回整句（否则会导出一张空卡）。
            let cs: Vec<char> = c.quote.chars().collect();
            let rare = cs
                .iter()
                .copied()
                .find(|c| jiandu_core::analysis::is_rare_char(*c));
            let front = match rare {
                Some(ch) => ch.to_string(),
                None => c.quote.trim().to_string(),
            };
            (front, tail)
        }
    }
}

/// 整匣卡片 → Anki 可导入的 CSV（两列：正面 / 背面，恒加引号）。
pub fn to_anki_csv(cards: &[Card], kind: AnkiKind) -> String {
    let mut out = String::new();
    for c in cards {
        let (f, b) = anki_cells(c, kind);
        if f.trim().is_empty() {
            continue;
        }
        out.push_str(&csv_two_quoted(&f, &b));
    }
    out
}

/// **导出整匣为 Obsidian 笔记**：选一个目录 → 一卡一 md。
///
/// `authors`：`store_id → 作者`，从书架条目带出来。查不到就留空——
/// frontmatter 里少一行 `author:` 只是一行，硬填「佚名」才是造假。
///
/// 返回 `Ok(None)` = 取消；`Ok(Some((目录, 写出张数)))`。
pub fn export_obsidian_folder(
    cards: &[Card],
    authors: &std::collections::HashMap<String, String>,
) -> Result<Option<(PathBuf, usize)>> {
    let Some(dir) = pick_folder("把卡片匣导出为 Obsidian 笔记（一卡一档）") else {
        return Ok(None);
    };
    let mut n = 0usize;
    for c in cards {
        if c.quote.trim().is_empty() {
            continue;
        }
        let author = authors.get(&c.store_id).map(|s| s.as_str()).unwrap_or("");
        let path = unique_path(dir.join(format!("{}.md", card_file_name(c))));
        save_bytes(path, "md", to_card_md(c, author).as_bytes())?;
        n += 1;
    }
    remember_export_dir(&dir);
    Ok(Some((dir, n)))
}

/// **导出整匣为 Anki 卡片**：系统保存框 → 一份 CSV。
pub fn export_anki_cards_csv(
    cards: &[Card],
    kind: AnkiKind,
    file_name: &str,
) -> Result<Option<PathBuf>> {
    let Some(picked) = pick_save_path(
        &format!("导出卡片匣为 Anki 卡片（{}）", kind.label()),
        file_name,
        "CSV",
        &["csv"],
    ) else {
        return Ok(None);
    };
    let path = with_extension(picked, "csv");
    let body = to_anki_csv(cards, kind);
    // 走 `save_bytes`：与其余六条导出链路同一出口（补后缀 / 建父目录 /
    // 记住目录），不在这里另起一套写盘。
    save_bytes(path, "csv", body.as_bytes()).map(Some)
}

/// 选一个**目录**（Obsidian 库用）。返回 `None` = 取消。
///
/// 与 [`pick_save_path`] 同一条规矩：必须在主线程直调，不许包
/// `background_executor`（rfd 同步面板会被 GPUI 主循环挤掉、静默返回 None）。
pub fn pick_folder(title: &str) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new().set_title(title);
    if let Some(dir) = remembered_export_dir() {
        dlg = dlg.set_directory(dir);
    } else if let Some(d) = desktop_dir() {
        dlg = dlg.set_directory(d);
    }
    dlg.pick_folder()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Book, Chapter, Paragraph, TocEntry};
    use jiandu_core::cards::{Card, CardKind};

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


    // ── v1.81.0 · P3：Obsidian / Anki 卡片 ────────────────────────────

    fn card(quote: &str, note: &str) -> Card {
        Card {
            id: 1,
            book: "庄子".into(),
            chapter: "逍遥游".into(),
            store_id: "abc123".into(),
            key: (3, 1, 7),
            len: quote.chars().count(),
            quote: quote.into(),
            note: note.into(),
            kind: CardKind::Quote,
            created_ms: 1_763_000_000_000, // 2025-11-11 前后
        }
    }

    #[test]
    fn civil_date_from_epoch_millis() {
        assert_eq!(ymd(0), (1970, 1, 1));
        // 闰年 2 月 29（2024-02-29 = day 19782；次日即 3 月 1 日）
        assert_eq!(ymd(19782 * 86_400_000), (2024, 2, 29));
        assert_eq!(ymd((19782 + 1) * 86_400_000), (2024, 3, 1));
        // 跨年
        assert_eq!(ymd((365 * 55 + 14) * 86_400_000), (2025, 1, 1));
    }

    #[test]
    fn card_md_has_parseable_frontmatter_and_backlinks() {
        let md = to_card_md(&card("北冥有鱼，其名为鲲。", "此句最妙"), "庄周");
        assert!(md.starts_with("---\n"), "frontmatter 必须在最前：\n{md}");
        let body = md.trim_start_matches("---\n");
        let end = body.find("\n---").expect("frontmatter 要有收束");
        let fm = &body[..end];
        assert!(fm.contains("date: "), "缺 date：\n{fm}");
        assert!(fm.contains("book: \"庄子\""), "缺 book：\n{fm}");
        assert!(fm.contains("chapter: \"逍遥游\""), "缺 chapter：\n{fm}");
        assert!(fm.contains("author: \"庄周\""), "缺 author：\n{fm}");
        assert!(fm.contains("  - 笺读/摘录"), "缺标签：\n{fm}");
        assert!(fm.contains("jiandu://abc123/3/1/7"), "缺回跳锚点：\n{fm}");
        // 正文三段
        assert!(md.contains("## 原文"), "缺原文段：\n{md}");
        assert!(md.contains("北冥有鱼，其名为鲲。"));
        assert!(md.contains("**朱批**") || md.contains("## 朱批"), "缺朱批段：\n{md}");
        // 双链：书名与章名都要能被 [[ ]] 引用
        assert!(md.contains("[[庄子]]"), "缺书名双链：\n{md}");
        assert!(md.contains("[[逍遥游]]"), "缺章名双链：\n{md}");
    }

    #[test]
    fn card_md_escapes_quotes_so_frontmatter_still_parses() {
        // 书名里带双引号 / 反斜杠：不转义会让整个 frontmatter 解析失败，
        // Obsidian 直接不认这份笔记。
        let mut c = card("原文", "");
        c.book = "《庄\"子》\\".into();
        let md = to_card_md(&c, "");
        assert!(md.contains("book: \"《庄\\\"子》\\\\\""), "没转义：\n{md}");
    }

    #[test]
    fn card_md_omits_empty_sections() {
        // 没有任何批注 / 难字时，不该留下「## 朱批」底下空一行。
        let md = to_card_md(&card("道可道，非常道。", ""), "");
        assert!(!md.contains("## 朱批"), "空批注不该出段：\n{md}");
        assert!(md.contains("## 原文"));
    }

    #[test]
    fn card_title_takes_the_first_sentence_only() {
        let c = card("北冥有鱼，其名为鲲。化而为鸟，其名为鹏。", "");
        assert_eq!(card_title(&c), "北冥有鱼，其名为鲲。");
    }

    #[test]
    fn card_file_name_starts_with_the_date() {
        // 日期打头：Obsidian 文件浏览器按名排序＝入匣顺序。
        let name = card_file_name(&card("北冥有鱼", ""));
        assert!(name.ends_with(".md") == false, "不该带后缀：{name}");
        assert!(
            name.chars().take(8).all(|c| c.is_ascii_digit()),
            "必须以日期打头：{name}"
        );
        assert!(name.contains("庄子"), "{name}");
    }

    #[test]
    fn same_named_cards_never_overwrite_each_other() {
        let dir = std::env::temp_dir().join(format!("jiandu-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let first = dir.join("同一天.md");
        std::fs::write(&first, "旧").ok();
        let second = unique_path(first.clone());
        assert_ne!(second, first, "撞名必须让位：{second:?}");
        assert!(!second.exists(), "挑出来的必须是空位：{second:?}");
        assert!(second.to_string_lossy().contains("-2"), "{second:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cloze_hides_the_rarest_character() {
        // 「埵」是生僻字（不在常用字表里），该被挖掉；常用字不挖。
        let front = anki_cloze_front("道可道埵非常道");
        assert!(front.contains("{{c1::埵}}"), "没挖到生僻字：{front}");
        // 没有生僻字时挖正中那个字（位置确定，可复现）。
        // 四字句取偏右一枚（idx = len/2 = 2 → 玄）；两处都钉死，免得日后
        // 有人「顺手改成 (len-1)/2」却忘了同步测试。
        assert_eq!(anki_cloze_front("天地玄黄"), "天地{{c1::玄}}黄");
    }

    #[test]
    fn anki_csv_has_exactly_two_columns_per_row() {
        let cards = vec![card("北冥有鱼，其名为鲲。", "此句最妙")];
        let csv = to_anki_csv(&cards, AnkiKind::Cloze);
        let line = csv.lines().next().expect("应有一行");
        assert!(line.starts_with('"'), "两列都必须包引号：{line}");
        assert!(line.contains("\",\"") , "列分隔只该有一处：{line}");
        assert!(csv.contains("{{c1::"), "挖空标记要落到 CSV 里：{csv}");
        assert!(csv.contains("【出处】庄子·逍遥游"), "背面要有出处：{csv}");
        assert!(csv.contains("【朱批】此句最妙"), "背面要有批注：{csv}");
    }

    #[test]
    fn anki_kinds_differ_in_front_but_share_the_back() {
        let c = card("北冥有鱼，其名为鲲。", "批");
        let (cf, cb) = anki_cells(&c, AnkiKind::Cloze);
        let (qf, qb) = anki_cells(&c, AnkiKind::Qa);
        let (gf, gb) = anki_cells(&c, AnkiKind::Gloss);
        assert_ne!(cf, qf, "挖空与问答的正面不该一样：{cf} / {qf}");
        // 背面只有 **问答** 多带一句全句（正面只给首句，答题要对照），
        // 故只比挖空 / 释义；问答的背面单独核它带全句。
        assert_eq!(cb, gb, "挖空与释义背面应当一致");
        assert!(qb.contains("北冥有鱼，其名为鲲。"), "问答背面要带全句：{qb}");
        assert!(qb.contains("【出处】"), "问答背面也要有出处：{qb}");
        assert!(qf.contains("（下句为何？）"), "{qf}");
        // 释义卡正面就那一枚难字：「冥」不在 GB2312 一级字里，故判为难字
        // （与 App 内「生僻字注音」同一把尺子，两边不许各说一套）。
        assert_eq!(gf, "冥", "释义卡该只出难字：{gf}");
    }

    #[test]
    fn gloss_card_falls_back_to_the_whole_quote_when_no_rare_char() {
        // 一句里没有难字时，释义卡退回整句——绝不导出一张空卡。
        let plain = "人之初性本善";
        let cs: Vec<char> = plain.chars().collect();
        assert!(
            !cs.iter().any(|c| jiandu_core::analysis::is_rare_char(*c)),
            "这句里已经有难字了，换个样例：{plain}"
        );
        let (gf, _) = anki_cells(&card(plain, ""), AnkiKind::Gloss);
        assert_eq!(gf, plain, "无难字该退回整句：{gf}");
    }

    #[test]
    fn anki_note_type_is_cloze_only_for_cloze() {
        // 挖空挂错笔记类型 = `{{c1::}}` 变成普通文本，卡片静默失效。
        assert!(AnkiKind::Cloze.note_type().contains("Cloze"));
        assert!(!AnkiKind::Qa.note_type().contains("Cloze"));
    }

    #[test]
    fn tags_are_sanitized_for_obsidian() {
        // 空格会被切成两个标签；`#` 会被当成嵌套层级——书名来自 epub，
        // 什么都可能带。
        assert_eq!(tag_of("庄 子"), "庄-子");
        assert_eq!(tag_of("a#b/c"), "a-b-c");
        assert_eq!(tag_of("  "), "未标注");
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
