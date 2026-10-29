//! 资源导入：md / txt / epub / json。
//!
//! 统一入口 `import_from_path`，按扩展名分派，返回 `Book`。
//!   * txt  —— 按「第 X 章」标题切章，空行分段。
//!   * md   —— 剥离行内标记，按 `#` 标题切章，空行分段。
//!   * json —— Book 结构（含 cover / tongjia / notes），或等价的简易结构。
//!   * epub —— 解压，读 META-INF/container.xml 找 OPF，按 spine 顺序抽取 XHTML 正文。
//!
//! 另外支持行内注释标记，便于手写：
//!   `道〖宇宙万物的本原〗可道`      -> 普通注（圈号注释）
//!   `不见≪现≫可欲`                 -> 通假标（靛蓝）

use crate::stream::{ChapterSink, MAX_CHAPTER_CHARS};
use crate::types::{Book, Chapter, Note, NoteKind, Paragraph, TocEntry};
use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::path::Path;

/// 根据扩展名导入。
pub fn import_from_path(path: &Path) -> Result<Book> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut book = match ext.as_str() {
        "txt" | "text" => import_txt(path)?,
        "md" | "markdown" => import_md(path)?,
        "html" | "htm" | "xhtml" | "xml" => import_html(path)?,
        "json" => import_json(path)?,
        "epub" => import_epub(path)?,
        "docx" => import_docx(path)?,
        other => {
            return Err(anyhow!(
                "不支持的格式：.{other}（支持 md / txt / html / xml / epub / docx / json）"
            ))
        }
    };
    // **统一清洗**（v1.54.0）：剥壳之后先落 IR，两遍扫完再交回排版引擎——
    // 从此引擎只见一种数据，新增格式只要写解析器。这一步同时做**生僻字兜底**：
    // 生僻但可渲染的字（CJK 扩展 A/B 等）原样保留、交字体回退显示，只进扫描清单；
    // PUA 私用区 / 乱码（任何字库都无字形）换成「□」古风占位，绝不 Panic。
    crate::cleaner::clean_book(&mut book, crate::ir::IrSource::from_ext(&ext));
    // 正文里残留（或旧 JSON 里本就存着）的 `[注]…[/注]` 标签：摘成夹注，
    // 绝不让 `[` `注` `]` 四个字占掉字格。
    book.ingest_inline_notes();
    // 前言 / 序言 / 导读同样不进书架（epub 在自己的流程末尾已砍过一次）。
    skip_front_matter(&mut book);
    // 没有目录的格式（txt / md / json）在这里统一按章节标题补齐。
    book.ensure_toc();
    // 书名 / 作者统一过「元数据粉碎机」再限长（v1.69.0）：盗版 EPUB 的题名
    // 常是那一长串文件名（`…(z-library.sk, 1lib.sk, z-lib.sk)`），里面塞着
    // 网址、拼音和站名。限长按 `META_TITLE_MAX`（15 字）——书架卡片一行、
    // 版心书口一格，超长既排不下也说明那不是书名。
    let max = crate::vclean::META_TITLE_MAX;
    book.title = tidy_meta(&book.title, max);
    book.cover.title = tidy_meta(&book.cover.title, max);
    book.author = tidy_meta(&book.author, max);
    Ok(book)
}

/// **智能跳过前言**：把「总目录 / 版权页 / 书影 / 前言 / 目录」从卷首砍掉，
/// 让书架第一页就是真正文。
///
/// 判据全在 `parsers::front_matter_cut`（题名 / 薄章 / 版权页元数据三道），
/// 这里只负责动刀：章序前移 + 目录条目跟着重编号。
/// 砍不砍得动由判据说了算，判据说不砍就一章不动。
/// 删掉给定章号（升序），并把目录条目按新章号重排。
///
/// 指向被删章的条目整条丢；其余条目的 `chapter` 减去「它前面被删了几章」。
fn drop_chapters(book: &mut Book, doomed: &[usize]) {
    if doomed.is_empty() {
        return;
    }
    for i in doomed.iter().rev() {
        if *i < book.chapters.len() {
            book.chapters.remove(*i);
        }
    }
    let mut toc = std::mem::take(&mut book.toc);
    toc.retain(|e| !doomed.contains(&e.chapter));
    for e in toc.iter_mut() {
        e.chapter -= doomed.iter().filter(|&&d| d < e.chapter).count();
    }
    book.toc = toc;
}

/// 取「题名 + 正文」对照表，供两道判据共用（正文按段拼，段间换行）。
fn chapter_pairs<'a>(book: &'a Book, bodies: &'a [String]) -> Vec<(&'a str, &'a str)> {
    book.chapters
        .iter()
        .zip(bodies.iter())
        .map(|(c, b)| (c.title.as_str(), b.as_str()))
        .collect()
}

fn chapter_bodies(book: &Book) -> Vec<String> {
    book.chapters
        .iter()
        .map(|c| {
            c.paragraphs
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

pub fn skip_front_matter(book: &mut Book) {
    if book.chapters.len() <= 1 {
        return;
    }
    // ① 夹在**正文中间**的散页：书商常把「目录」「书影」插在卷与卷之间。
    //
    // `front_matter_cut` 只砍卷首那一串前缀（它要先认出「正文起手」才动刀），
    // 当第一章本来就是正文时它返回 0，夹在后面的目录页就漏网了——实测《史记》
    // 第 14 章「目录」（11 段 119 字、通篇无句读）正是这么躲过去的。
    let bodies = chapter_bodies(book);
    let stray = crate::parsers::stray_front_indices(&chapter_pairs(book, &bodies));
    drop_chapters(book, &stray);

    if book.chapters.len() <= 1 {
        if book.toc.is_empty() {
            book.rebuild_toc_from_chapters();
        }
        return;
    }
    // ② 卷首那一串连续的前言（总目录 / 版权页 / 书影 / 前言 / 目录）。
    let bodies = chapter_bodies(book);
    let cut = crate::parsers::front_matter_cut(&chapter_pairs(book, &bodies));
    if cut > 0 && cut < book.chapters.len() {
        drop_chapters(book, &(0..cut).collect::<Vec<_>>());
    }
    // ③ 目录条目再按题名扫一道：EPUB 的 nav/ncx 里常有「封面」「目录」这类
    // **单独一条**的条目，它们指向的章可能已被上面吃掉、也可能侥幸躲过——
    // 只认章号会漏，索性题名也判一次（判据与砍章同一把，不会分叉）。
    let mut toc = std::mem::take(&mut book.toc);
    toc.retain(|e| !crate::parsers::is_front_matter_title(&e.title));
    book.toc = toc;
    if book.toc.is_empty() {
        book.rebuild_toc_from_chapters();
    }
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未命名")
        .to_string()
}

/// 按字数截断：超长缀省略号（书名 / 作者名夹字用）。
/// 元数据（书名 / 作者）统一收口（v1.69.0）：先粉碎网址 / 拼音 / 站名
/// / 特殊符号，再按 `max` 字截断。
///
/// 与 `vclean::clean_book_title` 同源，但**不做「遇卷册括号即截」**——
/// 那一步只该在刚拿到 `dc:title` 时做一次；这里跑在收尾处，题名已经
/// 定下来了，再截一次会把「卷五十一」这类正经题名截没。
fn tidy_meta(s: &str, max: usize) -> String {
    let t = crate::vclean::strip_meta_junk(s);
    let t = crate::vclean::tidy_meta_title(&t);
    clip_chars(&t, max)
}

fn clip_chars(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let mut out: String = chars.iter().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// 是否含句读标点（书名 / 作者名候选不应有）。
fn has_sentence_punct(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '，' | '。' | '！' | '？' | '；' | '、' | '…' | ',' | '.' | '!' | '?' | ';' | '/' | '·'
        )
    })
}

/// 从正文开头自动识别书名与作者（常见 txt 版式）：
/// 书名——第一个非空短行（剥掉《》【》等包裹），不超过 12 字；
/// 作者——「作者：X」「X 著」「文 / X」等式样，不超过 12 字。
/// 识别不到返回空串，由调用方回退到文件名。
pub fn detect_title_author(text: &str) -> (String, String) {
    let mut title = String::new();
    let mut author = String::new();
    // 书名与作者声明通常在正文之前，只看开头若干行。
    for raw in text.lines().take(16) {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // ---- 作者声明 ----
        if author.is_empty() {
            // 「作者：X」「作者:X」「作者 X」。
            if let Some(rest) = line.strip_prefix("作者") {
                let a = rest.trim_start_matches([':', '：', ' ']).trim();
                if !a.is_empty() && a.chars().count() <= 12 && !has_sentence_punct(a) {
                    author = a.to_string();
                    continue;
                }
            }
            // 「X 著」「X著」。
            if let Some(rest) = line.strip_suffix("著") {
                let a = rest.trim();
                if !a.is_empty() && a.chars().count() <= 12 && !has_sentence_punct(a) {
                    author = a.to_string();
                    continue;
                }
            }
            // 「文 / X」「文/X」。
            if let Some(rest) = line.strip_prefix("文") {
                let a = rest
                    .trim_start()
                    .trim_start_matches(['/', '／'])
                    .trim();
                if !a.is_empty() && a.chars().count() <= 12 && !has_sentence_punct(a) {
                    author = a.to_string();
                    continue;
                }
            }
        }
        // ---- 书名：第一个短行（无句读、非章回标题、非叶码）----
        // 扫描本 / 分页 txt 的第一行常常就是个叶码（「十三」「６」），它短且
        // 无句读，最容易被当书名——认出来跳过（v1.68.1）。
        if title.is_empty()
            && !is_heading(line)
            && line.chars().count() <= 16
            && !crate::cleaner::is_lone_number_para(line)
        {
            let t = line
                .split_whitespace()
                .next()
                .unwrap_or(line)
                .trim_matches(|c| {
                    matches!(
                        c,
                        '《' | '》'
                            | '【'
                            | '】'
                            | '['
                            | ']'
                            | '('
                            | ')'
                            | '（'
                            | '）'
                            | '〈'
                            | '〉'
                            | '"'
                            | '\''
                            | '“'
                            | '”'
                    )
                })
                .trim();
            if !t.is_empty() && t.chars().count() <= 12 && !has_sentence_punct(t) {
                title = t.to_string();
            }
        }
    }
    (title, author)
}

// ---------------------------------------------------------------------------
// 行内标记解析：〖注〗 与 ≪通假≫
// ---------------------------------------------------------------------------

/// 解析一行文本，抽出注释并返回纯正文 + 注释列表。
///
/// 标记会从正文中移除，注释的 `at` 记录其锚定字在**正文**中的偏移。
pub fn parse_inline_marks(line: &str) -> (String, Vec<Note>) {
    let mut text = String::new();
    let mut notes: Vec<Note> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        // 普通注：〖...〗 锚在前一个字
        if c == '〖' {
            let mut j = i + 1;
            let mut buf = String::new();
            while j < chars.len() && chars[j] != '〗' {
                buf.push(chars[j]);
                j += 1;
            }
            let anchor = text.chars().count().saturating_sub(1);
            notes.push(Note::new(anchor, NoteKind::Note, buf));
            i = if j < chars.len() { j + 1 } else { j };
            continue;
        }
        // 通假：≪本字≫ 锚在前一个字
        if c == '≪' {
            let mut j = i + 1;
            let mut buf = String::new();
            while j < chars.len() && chars[j] != '≫' {
                buf.push(chars[j]);
                j += 1;
            }
            let anchor = text.chars().count().saturating_sub(1);
            notes.push(Note::new(anchor, NoteKind::Tongjia, format!("通「{buf}」")));
            i = if j < chars.len() { j + 1 } else { j };
            continue;
        }
        // 划线标记：〔人:老子〕〔地:晋阳〕〔书:史记〕——括号内即词组本身。
        if c == '〔' {
            let mut j = i + 1;
            let mut buf = String::new();
            while j < chars.len() && chars[j] != '〕' {
                buf.push(chars[j]);
                j += 1;
            }
            if let Some((k, term)) = buf.split_once(':') {
                let kind = match k.trim() {
                    "人" => NoteKind::Person,
                    "地" => NoteKind::Place,
                    "书" => NoteKind::Book,
                    _ => NoteKind::Note,
                };
                if kind.is_mark() && !term.is_empty() {
                    let anchor = text.chars().count();
                    notes.push(Note::span(anchor, term.chars().count(), kind, term));
                }
            }
            i = if j < chars.len() { j + 1 } else { j };
            continue;
        }
        text.push(c);
        i += 1;
    }
    (text, notes)
}

/// 把纯文本行转成段落（解析行内标记）。
pub(crate) fn para_of(line: &str) -> Paragraph {
    let (text, notes) = parse_inline_marks(line.trim());
    Paragraph { text, notes }
}

// ---------------------------------------------------------------------------
// txt
// ---------------------------------------------------------------------------
pub fn import_txt(path: &Path) -> Result<Book> {
    // 走管线：先解码（UTF-8 / GBK），再切章、全角化、数字转汉字。
    let bytes = std::fs::read(path).with_context(|| format!("读取 txt 失败: {}", path.display()))?;
    let text = crate::pipeline::decode_bytes(&bytes);
    Ok(book_from_plaintext(&file_stem(path), &text))
}

fn is_heading(l: &str) -> bool {
    let l = l.trim();
    (l.starts_with('第')
        && (l.contains('章') || l.contains('回') || l.contains('节') || l.contains('卷')))
        || l.starts_with('#')
}

/// 纯文本 -> Book。书名自动识别（正文首行），识别不到回退文件名；
/// 书名与作者名统一夹到 12 字以内（书架 / 封面竖排区域有限）。
///
/// 切章与清洗统一交给 [`crate::pipeline::parse_txt`]：认「第 X 章/回/卷/节」，
/// 通篇无标题且超长按 3000 字切卷，标点转全角、数字转汉字。
pub fn book_from_plaintext(fallback_title: &str, text: &str) -> Book {
    // 头部幽灵先切一刀（BOM / 零宽 / 全角空格 / YAML 元数据块 / 连续空行）：
    // 书名自动识别看的是「第一行」，不先洗就会把 `---` / `title:` 认成书名。
    // 切章（`parse_txt`）内部还会再洗一次，两次结果一致（幂等）。
    let clean = crate::cleaner::strip_head_junk(text);
    let (detected, author) = detect_title_author(&clean);
    let chapters = crate::pipeline::parse_txt(&clean);
    let mut book = crate::pipeline::chapters_to_book(fallback_title, &author, chapters);
    // 书名：正文首行识别出来的优先，其次文件名。
    let title = if detected.is_empty() {
        fallback_title.to_string()
    } else {
        detected
    };
    book.title = clip_chars(&title, 12);
    book.cover.title = book.title.clone();
    book.author = clip_chars(&author, 12);
    if book.chapters.is_empty() {
        book.chapters.push(Chapter {
            title: "正文".into(),
            paragraphs: vec![para_of(&clean)],
        });
    }
    book
}

fn flush_para(para: &mut Vec<String>, chap: &mut Option<Chapter>, book: &mut Book) {
    if para.is_empty() {
        return;
    }
    let joined = para.join("").trim().to_string();
    para.clear();
    if joined.is_empty() {
        return;
    }
    let p = para_of(&joined);
    match chap {
        Some(c) => c.paragraphs.push(p),
        None => {
            if book.chapters.is_empty() {
                book.chapters.push(Chapter {
                    title: "正文".into(),
                    paragraphs: Vec::new(),
                });
            }
            book.chapters[0].paragraphs.push(p);
        }
    }
}

// ---------------------------------------------------------------------------
// markdown
// ---------------------------------------------------------------------------
pub fn import_md(path: &Path) -> Result<Book> {
    let bytes =
        std::fs::read(path).with_context(|| format!("读取 markdown 失败: {}", path.display()))?;
    let text = crate::pipeline::decode_bytes(&bytes);
    Ok(book_from_markdown(&file_stem(path), &text))
}

/// Markdown -> Book：走统一管线（`#` 切章、`>` 转夹注、`**` 转着重、
/// 代码块与图片丢弃）。首个一级标题若独占一章则由管线提升为书名。
pub fn book_from_markdown(fallback_title: &str, md: &str) -> Book {
    let chapters = crate::pipeline::parse_markdown(md);
    let mut book = crate::pipeline::chapters_to_book(fallback_title, "", chapters);
    if book.cover.title.is_empty() {
        book.cover.title = book.title.clone();
    }
    book
}

// ---------------------------------------------------------------------------
// html / xml
// ---------------------------------------------------------------------------
pub fn import_html(path: &Path) -> Result<Book> {
    let bytes =
        std::fs::read(path).with_context(|| format!("读取 html 失败: {}", path.display()))?;
    let text = crate::pipeline::decode_bytes(&bytes);
    let chapters = crate::pipeline::parse_html(&text);
    // 书名优先取 <title> / <h1>，取不到才回退文件名。
    let title = extract_html_title(&text)
        .map(|t| clip_chars(&t, 12))
        .unwrap_or_else(|| file_stem(path));
    let mut book = crate::pipeline::chapters_to_book(&title, "", chapters);
    if book.cover.title.is_empty() {
        book.cover.title = book.title.clone();
    }
    Ok(book)
}

/// 去掉行内 Markdown 标记（保留 〖〗 ≪≫ 注释标记）。
fn strip_md_inline(line: &str) -> String {
    let mut s = line.to_string();
    s = strip_links(&s, "![", "](");
    s = strip_links(&s, "[", "](");
    for token in ["**", "__", "*", "_", "~~", "`"] {
        s = s.replace(token, "");
    }
    // 行首引用符
    s.trim_start_matches('>').trim().to_string()
}

fn strip_links(s: &str, open: &str, mid: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        let after = &rest[i + open.len()..];
        if let Some(j) = after.find(mid) {
            let label = &after[..j];
            let tail = &after[j + mid.len()..];
            if let Some(k) = tail.find(')') {
                out.push_str(label);
                rest = &tail[k + 1..];
                continue;
            }
        }
        out.push_str(open);
        rest = after;
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// json
// ---------------------------------------------------------------------------
/// DOCX 导入：zip 包 → `word/document.xml` → 标准章数组（样式全丢，见
/// [`crate::pipeline::parse_docx`]）。
pub fn import_docx(path: &Path) -> Result<Book> {
    let bytes = std::fs::read(path).with_context(|| format!("读不出 {}", path.display()))?;
    let chapters = crate::pipeline::parse_docx(&bytes)?;
    let mut book = crate::pipeline::chapters_to_book(&file_stem(path), "", chapters);
    let (t, a) = detect_title_author(&book.chapters.first().map(|c| {
        c.paragraphs
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }).unwrap_or_default());
    if !t.is_empty() {
        book.title = t;
    }
    if !a.is_empty() {
        book.author = a;
    }
    book.cover.title = book.title.clone();
    Ok(book)
}

pub fn import_json(path: &Path) -> Result<Book> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 json 失败: {}", path.display()))?;
    parse_json_book(&text, &file_stem(path))
}

/// JSON **流式**导入：整本读入 → core 数据适配器归一为统一 `Book` → 逐章
/// 交 sink 落盘、随即释放。
///
/// 与 [`import_json`]（旧入口，给老条目 / 导出 / 单测用）的唯一差别：这里
/// 边解析边向 `sink` 报进度，进度条才会动、看门狗才不会误杀大文件——JSON
/// 整条解析是「读整本 + 一次性反序列化」，没有 EPUB 那种逐篇粒度，但至少
/// 在「读毕」「落盘」「完成」三个节点各 tick 一次，足以让 UI 不假死。
pub fn import_json_into(path: &Path, sink: &mut dyn ChapterSink) -> Result<Book> {
    sink.tick(0.05, "雕版 读取 JSON");
    // 读整本 + 反序列化（JSON 天然是一棵对象，不能像 EPUB 那样逐篇流式；
    // 但只读一次、反序列化后即逐章落盘释放，峰值内存只多「一份原文」）。
    let full = import_json(path)?;
    sink.tick(
        0.6,
        &format!("雕版 {} 章 · 落盘", full.chapters.len().max(1)),
    );
    let mut skeleton = Book::new(&full.title, &full.author);
    skeleton.cover = full.cover.clone();
    skeleton.tongjia = full.tongjia.clone();
    skeleton.head_notes = full.head_notes.clone();
    for ch in full.chapters {
        sink.push(ch);
    }
    sink.tick(1.0, "雕版 完成");
    Ok(skeleton)
}

/// 把解析好的书写成书架用的 JSON。
///
/// EPUB 入库时走这条路：一次性解析成结构化 `Book` 后存成 `.json`，
/// 之后打开直接读 JSON，不再解压 / 解析 XHTML；也方便用户拿到可读的
/// 纯文本结构做二次加工（与 `jiandu --convert x.epub -o x.json` 一致）。
pub fn write_book_json(book: &Book, dst: &Path) -> Result<()> {
    let s = serde_json::to_string_pretty(book).context("序列化书籍失败")?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(dst, s).with_context(|| format!("写入失败: {}", dst.display()))?;
    Ok(())
}

/// 兼容旧签名的薄封装：内部统一走 [`adapt_json_book`] 数据适配器。
pub fn parse_json_book(text: &str, fallback_title: &str) -> Result<Book> {
    let mut book = adapt_json_book(text, fallback_title)?;
    // 旧 JSON 里可能还留着 `[注]…[/注]` 字面标签（早先没拆），开卷前吸一遍。
    book.ingest_inline_notes();
    Ok(book)
}

/// 数据适配器（Data Adapter）：把各种不规范的 JSON / 纯文本导入文件
/// 归一成标准 `Book`，让引擎无差别跑通。
///
/// 兼容优先级（命中即返回，不再报错）：
///   1. 完整 `Book` 结构（可反序列化即直用）。
///   2. `{title, author, chapters:[...]}` 多章标准格式。
///   3. `{..., chapter:{...}}` 单数章节 —— 自动包成 `[chapter]`。
///   4. 根节点即单章 `{title, content}` —— 自动包成 `[root]`。
///   5. 根直接是数组 `[{...}, ...]` —— 当作 chapters。
///   6. 内容根本不是 JSON（纯文本）—— 包成
///      `{title:"无题", chapters:[{title:"正文", content: 文本}]}`。
///   7. 本该是 JSON 却解析失败（畸形 / 截断 / 单引号等）—— 返回古籍风错误，
///      交由上层弹「此卷竹简散乱，无法识读」浮窗，绝不崩溃。
pub const ERR_JSON_FORMAT: &str = "此卷竹简散乱，无法识读（JSON 格式有误）";

pub fn adapt_json_book(raw: &str, fallback_title: &str) -> Result<Book> {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => {
            // 不是合法 JSON：先判断它「本该是 JSON」还是纯文本。
            if looks_like_json(raw) {
                // 长得像 JSON 却解析失败 —— 畸形文件，报古籍风错误。
                return Err(anyhow!(ERR_JSON_FORMAT));
            }
            // 纯文本兜底：包成「无题 · 正文」单章。
            return Ok(plain_text_book(raw));
        }
    };

    match v {
        serde_json::Value::Array(items) => {
            // 形态 5：根即章节数组。
            let mut book = Book::new(fallback_title, "");
            book.cover.title = fallback_title.to_string();
            for ch in &items {
                book.chapters.push(value_to_chapter(ch));
            }
            if book.chapters.is_empty() {
                return Err(anyhow!(ERR_JSON_FORMAT));
            }
            Ok(book)
        }
        serde_json::Value::Object(_) => {
            // 形态 1：完整 Book 直用（章节 / 夹注 / 题署齐全）。
            if let Ok(mut book) = serde_json::from_str::<Book>(raw) {
                if book.cover.title.is_empty() {
                    book.cover.title = book.title.clone();
                }
                return Ok(book);
            }
            // v1.69.0：书名键**按优先级认**——开源古籍 JSON 常把书名放在
            // `book` 字段（另有 `volume` / `pinyin` 等一堆元数据）。旧版只
            // 认 `title`，于是书名退化成文件名（`dirty_pinyin`）。
            // 这些键一律**只当元数据读**，绝不进正文流（正文只认
            // `content` / `paragraphs` / `text` / `body`，见 `value_to_chapter`）。
            let title = ["title", "book", "name", "书名", "标题"]
                .iter()
                .find_map(|k| v.get(k).and_then(|x| x.as_str()))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback_title)
                .to_string();
            let author = ["author", "creator", "authors", "作者"]
                .iter()
                .find_map(|k| v.get(k).and_then(|x| match x {
                    serde_json::Value::String(s) => Some(s.trim().to_string()),
                    serde_json::Value::Array(a) => Some(
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join("、"),
                    ),
                    _ => None,
                }))
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            let mut book = Book::new(title.clone(), author);
            book.cover.title = title;
            if let Some(sub) = v.get("subtitle").and_then(|x| x.as_str()) {
                book.cover.subtitle = sub.to_string();
            }
            if let Some(tj) = v.get("tongjia").and_then(|x| x.as_object()) {
                for (k, val) in tj {
                    if let Some(s) = val.as_str() {
                        book.tongjia.insert(k.clone(), s.to_string());
                    }
                }
            }

            if let Some(chs) = v.get("chapters").and_then(|x| x.as_array()) {
                // 形态 2：标准多章。
                for ch in chs {
                    book.chapters.push(value_to_chapter(ch));
                }
            } else if let Some(ch) = v.get("chapter") {
                // 形态 3：单数章节 —— 自动包成 [chapter]。
                book.chapters.push(value_to_chapter(ch));
            } else {
                // 形态 4：根节点本身就是一章。
                book.chapters.push(value_to_chapter(&v));
            }

            if book.chapters.is_empty() {
                return Err(anyhow!(ERR_JSON_FORMAT));
            }
            Ok(book)
        }
        _ => Err(anyhow!(ERR_JSON_FORMAT)),
    }
}

/// 把单个章节值（来自 chapters 数组 / 单数 chapter / 根节点）归一为 `Chapter`。
/// 正文支持两种写法：`paragraphs:[...]` 或 `content:"..."`（按换行切段）。
fn value_to_chapter(ch: &serde_json::Value) -> Chapter {
    let ct = ch
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    if let Some(ps) = ch.get("paragraphs").and_then(|x| x.as_array()) {
        for p in ps {
            match p {
                serde_json::Value::String(s) => paragraphs.push(para_of(s)),
                serde_json::Value::Object(_) => {
                    if let Ok(pp) = serde_json::from_value::<Paragraph>(p.clone()) {
                        paragraphs.push(pp);
                    }
                }
                _ => {}
            }
        }
    } else if let Some(content) = ch.get("content").and_then(|x| x.as_str()) {
        for seg in content.split('\n') {
            let seg = seg.trim();
            if !seg.is_empty() {
                paragraphs.push(para_of(seg));
            }
        }
    }
    Chapter { title: ct, paragraphs }
}

/// 纯文本（非 JSON）兜底：包成 `{title:"无题", chapters:[{title:"正文", content: 文本}]}`。
fn plain_text_book(text: &str) -> Book {
    let mut book = Book::new("无题", "");
    book.cover.title = "无题".to_string();
    let mut chapter = Chapter {
        title: "正文".into(),
        paragraphs: Vec::new(),
    };
    for seg in text.split('\n') {
        let seg = seg.trim();
        if !seg.is_empty() {
            chapter.paragraphs.push(para_of(seg));
        }
    }
    book.chapters.push(chapter);
    book
}

/// 粗略判断一段文本「本该是 JSON」还是纯文本：以 `{` 或 `[` 开头即视为 JSON。
fn looks_like_json(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('{') || t.starts_with('[')
}

// ---------------------------------------------------------------------------
// epub
// ---------------------------------------------------------------------------
pub fn import_epub(path: &Path) -> Result<Book> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("打开 epub 失败: {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("epub 不是合法的 zip 容器")?;

    let container =
        read_zip_text(&mut zip, "META-INF/container.xml").context("缺少 META-INF/container.xml")?;
    let opf_path = find_attr(&container, "full-path")
        .ok_or_else(|| anyhow!("container.xml 里没有 full-path"))?;

    let opf = read_zip_text(&mut zip, &opf_path).context("读取 OPF 失败")?;
    // 书名先过一道清洗：`dc:title` 里常带着整条书眉（`…精华录卷十九`），
    // 版心只有一格宽，多一个字就挤掉一个卷次。
    let title = crate::vclean::clean_book_title(
        &extract_tag_text(&opf, "dc:title").unwrap_or_else(|| file_stem(path)),
    );
    let author = extract_tag_text(&opf, "dc:creator").unwrap_or_default();

    let base_dir = {
        let p = Path::new(&opf_path);
        p.parent().map(|d| d.to_path_buf()).unwrap_or_default()
    };
    let base_prefix = base_dir.to_string_lossy().replace('\\', "/");
    let join = |href: &str| -> String {
        if base_prefix.is_empty() {
            href.to_string()
        } else {
            format!("{}/{}", base_prefix, href)
        }
    };

    let spine_ids = extract_spine_ids(&opf);
    let items = extract_items(&opf); // id -> (href, media_type, properties)
    let manifest: std::collections::HashMap<String, String> = items
        .iter()
        .map(|(id, (href, _, _))| (id.clone(), href.clone()))
        .collect();

    let mut book = Book::new(title.clone(), author);
    book.cover.title = title.clone();

    // ---- 先按 spine 顺序抽出正文 ----
    let mut ordered: Vec<String> = Vec::new();
    for id in &spine_ids {
        if let Some(href) = manifest.get(id) {
            ordered.push(join(href));
        }
    }
    if ordered.is_empty() {
        ordered.extend(items.iter().map(|(_, (h, _, _))| join(h)));
    }

    // 先记下每个 spine 文件对应的章序号，供目录定位。
    let mut file_to_chapter: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for raw_href in ordered {
        let file_key = normalize_href(&raw_href);
        if let Ok(html) = read_zip_text(&mut zip, &file_key) {
            // 走 **EPUB 竖排清洗管线**（`vclean`，五阶段）：结构降噪（剔广告 /
            // 剥颜色）→ ruby 注音转 `[拼音:…]` / noteref 转 `[注]…[/注]` →
            // 数字转汉字（转不动的整串包 `[数字:…]`）→ 括号转夹注、
            // 《》→︽︾、……→︙ → 丢页码孤行、段间 `\n`。
            // 与 md / txt 的 `parse_html` 是同一台标签行走器，差别只在前后的工序。
            let subs = crate::vclean::clean_epub(&html);
            if subs.iter().all(|c| c.content.trim().is_empty()) {
                continue;
            }
            // 章名先用 HTML 里的 <title> / <h1>，之后目录会覆盖。
            let guessed = extract_html_title(&html).unwrap_or_else(|| {
                Path::new(&file_key)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("章")
                    .to_string()
            });
            let mut first_idx: Option<usize> = None;
            for c in subs {
                // 管线没从 <h?> 取到标题时（整篇无标题），用 <title> 兜底。
                let title = if c.title == "正文" {
                    guessed.clone()
                } else {
                    c.title.clone()
                };
                let mut chapter = Chapter {
                    title,
                    paragraphs: Vec::new(),
                };
                for line in c.content.lines() {
                    let l = line.trim();
                    if !l.is_empty() {
                        chapter.paragraphs.push(crate::pipeline::paragraph_of(l));
                    }
                }
                if !chapter.paragraphs.is_empty() {
                    if first_idx.is_none() {
                        first_idx = Some(book.chapters.len());
                    }
                    book.chapters.push(chapter);
                }
            }
            if let Some(i) = first_idx {
                file_to_chapter.insert(file_key.clone(), i);
            }
        }
    }

    if book.chapters.is_empty() {
        return Err(anyhow!("epub 中未解析出正文"));
    }

    // ---- 书级清洗：书眉 / 书口文字逐章赶出去（跨章才能判「重复」）----
    // 章题剥掉书名前缀与卷次尾巴；正文里重复出现的书眉段整段删除。
    // 版心（中缝 / 章题列）从此只渲染引擎自己算出来的书名、卷次、叶码。
    crate::vclean::purge_running_headers(&mut book.chapters, &title);

    // ---- 再解析目录：EPUB3 nav 优先，回退 EPUB2 ncx ----
    let nav_href = items
        .iter()
        .find(|(_, (_, _, props))| props.split_whitespace().any(|p| p == "nav"))
        .map(|(_, (h, _, _))| join(h));

    let ncx_href = spine_toc_id(&opf)
        .and_then(|id| manifest.get(&id).cloned())
        .map(|h| join(&h))
        .or_else(|| {
            items
                .iter()
                .find(|(_, (h, mtype, _))| {
                    mtype.contains("ncx") || h.to_ascii_lowercase().ends_with(".ncx")
                })
                .map(|(_, (h, _, _))| join(h))
        });

    let mut toc: Vec<TocEntry> = Vec::new();

    if let Some(href) = nav_href {
        if let Ok(nav) = read_zip_text(&mut zip, &href) {
            let entries = parse_nav_toc(&nav, &base_prefix);
            toc = resolve_toc(entries, &file_to_chapter);
        }
    }
    if toc.is_empty() {
        if let Some(href) = ncx_href {
            if let Ok(ncx) = read_zip_text(&mut zip, &href) {
                let entries = parse_ncx_toc(&ncx, &base_prefix);
                toc = resolve_toc(entries, &file_to_chapter);
            }
        }
    }

    if toc.is_empty() {
        // 没有可用目录：退回章节标题。
        book.rebuild_toc_from_chapters();
    } else {
        // 用目录标题润色章节名（保留层级条目里的章名）。
        for e in &toc {
            if let Some(ch) = book.chapters.get_mut(e.chapter) {
                if ch.title.trim().is_empty() || looks_like_filename(&ch.title) {
                    ch.title = e.title.clone();
                }
            }
        }
        // 章名统一取目录里**第一个**映射到该章的标题。
        let mut seen = std::collections::HashSet::new();
        for e in &toc {
            if seen.insert(e.chapter) {
                if let Some(ch) = book.chapters.get_mut(e.chapter) {
                    ch.title = e.title.clone();
                }
            }
        }
        book.toc = toc;
    }

    // ---- 收尾：智能跳过前言（目录润色之后题名最准，此时才动刀）----
    //
    // 现代 EPUB 把「总目录 / 版权页 / 书影 / 前言 / 目录」各做成独立的 spine
    // 文件，逐文件清洗砍不掉（每份文件自己只有一章，看不出它是附件）；
    // 而章名又要等目录（nav / ncx）润色完才从 `未知` 变成 `总目录` `前言`。
    // 故这一刀放在最后：章序摆齐、题名就位，才看得出「正文从卷一开始」。
    // 与管线里的 `drop_front_matter` 共用 `front_matter_cut`，判据不分叉。
    {
        let bodies: Vec<String> = book
            .chapters
            .iter()
            .map(|c| {
                c.paragraphs
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect();
        let pairs: Vec<(&str, &str)> = book
            .chapters
            .iter()
            .zip(bodies.iter())
            .map(|(c, b)| (c.title.as_str(), b.as_str()))
            .collect();
        let cut = crate::parsers::front_matter_cut(&pairs);
        if cut > 0 && cut < book.chapters.len() {
            book.chapters.drain(..cut);
            // 目录条目跟着前移；指向被砍掉那几章的条目整条丢。
            let mut toc = std::mem::take(&mut book.toc);
            toc.retain(|e| e.chapter >= cut);
            for e in toc.iter_mut() {
                e.chapter -= cut;
            }
            book.toc = toc;
            if book.toc.is_empty() {
                book.rebuild_toc_from_chapters();
            }
        }
    }

    Ok(book)
}

/// 目录里的一条原始记录（文件 href + 标题 + 层级）。
struct RawToc {
    href: String,
    title: String,
    level: usize,
}

fn looks_like_filename(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    low.starts_with("chapter") || low.starts_with("part") || low.starts_with("text")
        || low.starts_with("ch0") || low.starts_with("ch1") || low.starts_with("ch2")
        || low.starts_with("ch3")
}

/// 去掉锚点、统一分隔符、解 URL 编码（只处理常见的 %20）。
fn normalize_href(href: &str) -> String {
    let no_frag = href.split('#').next().unwrap_or(href);
    let unescaped = no_frag.replace("%20", " ");
    unescaped.replace('\\', "/").trim_start_matches('/').to_string()
}

fn href_key(href: &str, base_prefix: &str) -> String {
    let h = normalize_href(href);
    if h.starts_with(base_prefix) || base_prefix.is_empty() {
        h
    } else {
        // nav / ncx 里的 href 是相对 nav 文件自身目录的，个别书会出现 ../。
        let mut parts: Vec<&str> = base_prefix.split('/').filter(|s| !s.is_empty()).collect();
        for seg in h.split('/') {
            match seg {
                ".." => {
                    parts.pop();
                }
                "." => {}
                s => parts.push(s),
            }
        }
        parts.join("/")
    }
}

/// 把原始目录映射成带章序号的 TocEntry。
fn resolve_toc(
    raw: Vec<RawToc>,
    file_to_chapter: &std::collections::HashMap<String, usize>,
) -> Vec<TocEntry> {
    let mut out = Vec::new();
    for r in raw {
        let key = normalize_href(&r.href);
        let hit = file_to_chapter
            .get(&key)
            .copied()
            .or_else(|| {
                // 有些书 href 没带目录前缀，退化为按文件名匹配。
                let stem = Path::new(&key)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                file_to_chapter
                    .iter()
                    .find(|(k, _)| {
                        Path::new(k)
                            .file_name()
                            .map(|f| f.to_string_lossy() == stem)
                            .unwrap_or(false)
                    })
                    .map(|(_, v)| *v)
            });
        if let Some(ci) = hit {
            out.push(TocEntry::new(r.title, ci, r.level));
        }
    }
    // 去重：同一章保留第一条（通常是最上层的标题）。
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert(e.chapter));
    out
}

/// 解析 EPUB3 nav.xhtml 里的 `<nav epub:type="toc">`，按嵌套 ol/li 计算层级。
fn parse_nav_toc(nav: &str, base_prefix: &str) -> Vec<RawToc> {
    // 截取 toc 那个 nav 区块。
    let region = {
        let low = nav.to_ascii_lowercase();
        match low.find("epub:type=\"toc\"") {
            Some(i) => {
                let start = nav[..i].rfind("<nav").unwrap_or(0);
                let rest = &nav[start..];
                match rest.to_ascii_lowercase().find("</nav>") {
                    Some(j) => &rest[..j],
                    None => rest,
                }
            }
            None => nav,
        }
    };

    let mut out = Vec::new();
    let mut level = 0usize;
    let bytes: Vec<char> = region.chars().collect();
    let mut i = 0usize;
    let mut buf = String::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c == '<' {
            // 标签起点，先把 buf 里的可见文本冲掉（不是链接就丢弃）。
            buf.clear();
            let mut j = i + 1;
            let mut tag = String::new();
            while j < bytes.len() && bytes[j] != '>' {
                tag.push(bytes[j]);
                j += 1;
            }
            let tl = tag.to_ascii_lowercase();
            if tl.starts_with("ol") {
                level += 1;
            } else if tl.starts_with("/ol") {
                level = level.saturating_sub(1);
            } else if tl.starts_with("a ") || tl.starts_with("a>") {
                // 取 href 与链接文字。
                let href = attr_in_tag(&tag, "href").unwrap_or_default();
                let mut k = j + 1;
                let mut text = String::new();
                while k < bytes.len() && bytes[k] != '<' {
                    text.push(bytes[k]);
                    k += 1;
                }
                let t = decode_entities(text.trim());
                if !t.is_empty() {
                    out.push(RawToc {
                        href: href_key(&href, base_prefix),
                        title: t,
                        level: level.max(1),
                    });
                }
                i = k;
                continue;
            }
            i = j + 1;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    out
}

/// 解析 EPUB2 toc.ncx 的 navPoint 树。
fn parse_ncx_toc(ncx: &str, base_prefix: &str) -> Vec<RawToc> {
    let mut out = Vec::new();
    let lower = ncx.to_ascii_lowercase();
    let mut level = 0usize;
    let mut pos = 0usize;

    while pos < ncx.len() {
        let rest_low = &lower[pos..];
        let next_open = rest_low.find("<navpoint");
        let next_close = rest_low.find("</navpoint>");
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                level += 1;
                pos += o + "<navpoint".len();
            }
            (_, Some(c)) => {
                level = level.saturating_sub(1);
                pos += c + "</navpoint>".len();
            }
            (Some(o), None) => {
                level += 1;
                pos += o + "<navpoint".len();
            }
            (None, None) => break,
        }

        // 从当前位置往后找最近的 navLabel/text 与 content src，且必须在本节点范围内。
        let seg_end = lower[pos..]
            .find("</navpoint>")
            .map(|i| pos + i)
            .unwrap_or(ncx.len());
        let seg = &ncx[pos..seg_end];
        let seg_low = &lower[pos..seg_end];

        if let Some(ti) = seg_low.find("<text") {
            if let Some(gt) = seg[ti..].find('>') {
                let start = ti + gt + 1;
                if let Some(end_rel) = seg[start..].find("</text") {
                    let t = decode_entities(seg[start..start + end_rel].trim());
                    if !t.is_empty() {
                        let href = seg_low
                            .find("<content")
                            .and_then(|ci| {
                                let tag_end = seg[ci..].find('>')? + ci;
                                attr_in_tag(&seg[ci..tag_end], "src")
                            })
                            .unwrap_or_default();
                        if !href.is_empty() {
                            out.push(RawToc {
                                href: href_key(&href, base_prefix),
                                title: t,
                                level: level.max(1),
                            });
                        }
                    }
                }
            }
        }
    }
    out
}

/// 从某个标签文本里取属性值，兼容单双引号。
fn attr_in_tag(tag: &str, attr: &str) -> Option<String> {
    let low = tag.to_ascii_lowercase();
    let needle = format!("{}=", attr);
    let i = low.find(&needle)? + needle.len();
    let rest = &tag[i..];
    let rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let body = &rest[1..];
        let end = body.find(quote)?;
        Some(body[..end].to_string())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// 从 HTML 里猜章名：<title> 优先，其次 <h1>/<h2>。
fn extract_html_title(html: &str) -> Option<String> {
    for tag in ["title", "h1", "h2", "h3"] {
        let open = format!("<{tag}");
        let low = html.to_ascii_lowercase();
        if let Some(i) = low.find(&open) {
            if let Some(gt) = html[i..].find('>') {
                let start = i + gt + 1;
                let close = format!("</{tag}>");
                if let Some(end_rel) = low[start..].find(&close) {
                    let t = decode_entities(html[start..start + end_rel].trim());
                    let t = t.trim().to_string();
                    if !t.is_empty() {
                        return Some(t);
                    }
                }
            }
        }
    }
    None
}

/// 取 manifest：id -> (href, media-type, properties)。
fn extract_items(
    opf: &str,
) -> Vec<(String, (String, String, String))> {
    let mut out = Vec::new();
    let mut rest = opf;
    while let Some(i) = rest.find("<item") {
        let seg = &rest[i..];
        let stop = seg.find('>').unwrap_or(seg.len().saturating_sub(1));
        let tag = &seg[..stop];
        let id = attr_in_tag(tag, "id");
        let href = attr_in_tag(tag, "href");
        if let (Some(id), Some(href)) = (id, href) {
            let mtype = attr_in_tag(tag, "media-type").unwrap_or_default();
            let props = attr_in_tag(tag, "properties").unwrap_or_default();
            out.push((id, (href, mtype, props)));
        }
        rest = &seg[stop..];
    }
    out
}

/// 取 `<spine toc="ncx">` 里的 toc id。
fn spine_toc_id(opf: &str) -> Option<String> {
    let i = opf.find("<spine")?;
    let seg = &opf[i..];
    let stop = seg.find('>')?;
    attr_in_tag(&seg[..stop], "toc")
}

fn read_zip_text<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<String> {
    let mut file = zip.by_name(name)?;
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(s)
}

fn find_attr(xml: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let i = xml.find(&needle)? + needle.len();
    let rest = &xml[i..];
    let j = rest.find('"')?;
    Some(rest[..j].to_string())
}

fn extract_tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let after = &xml[start..];
    let gt = after.find('>')? + start + 1;
    let end = xml[gt..].find(&close)? + gt;
    let inner = xml[gt..end].trim();
    if inner.is_empty() {
        None
    } else {
        Some(inner.to_string())
    }
}

fn extract_manifest(opf: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut rest = opf;
    while let Some(i) = rest.find("<item ") {
        let seg = &rest[i..];
        let end = seg.find('>').unwrap_or(seg.len().saturating_sub(1));
        let tag = &seg[..end];
        let id = find_attr(tag, "id");
        let href = find_attr(tag, "href");
        let mtype = find_attr(tag, "media-type").unwrap_or_default();
        if let (Some(id), Some(href)) = (id, href) {
            if mtype.contains("xhtml") || href.ends_with(".xhtml") || href.ends_with(".html") {
                map.insert(id, href);
            }
        }
        rest = &seg[end..];
    }
    map
}

fn extract_spine_ids(opf: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let spine = match opf.find("<spine") {
        Some(i) => match opf[i..].find('>') {
            Some(g) => &opf[i + g..],
            None => return ids,
        },
        None => return ids,
    };
    let end = spine.find("</spine>").unwrap_or(spine.len());
    let region = &spine[..end];
    let mut rest = region;
    while let Some(i) = rest.find("<itemref") {
        let seg = &rest[i..];
        let stop = seg.find('>').unwrap_or(seg.len().saturating_sub(1));
        if let Some(idref) = find_attr(&seg[..stop], "idref") {
            ids.push(idref);
        }
        rest = &seg[stop..];
    }
    ids
}

/// HTML → 纯文本（旧接口，测试与兜底用）。
///
/// 真正的活儿已交给 [`crate::pipeline::parse_html`]：剥 script/style/nav、
/// `<h?>` 切章、`<em>` 转夹注、实体还原、数字转汉字，一并在管线里做完；
/// 这里只把各章正文接成一段文本。
fn html_to_text(html: &str) -> String {
    let chapters = crate::pipeline::parse_html(html);
    let mut out = String::new();
    for c in &chapters {
        for line in c.content.lines() {
            let l = line.trim();
            if !l.is_empty() {
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    if out.trim().is_empty() {
        // 一个标签都没解析出来：退回「去标签」的老办法。
        out = crate::pipeline::decode_entities(&crate::pipeline::strip_tags(html));
    }
    out
}


// ─────────────────────────────────────────────────────────────────────────────
// v1.50.0 **流式导入**：解析出一章就交给 `ChapterSink`（落盘并释放），
// 绝不把整本书攒在内存里。以下三个入口与上面那套「攒成 Book 再返回」的
// 旧入口并存——旧入口给小书 / 导出 / 单测用，新入口给百万字丛书用。
// ─────────────────────────────────────────────────────────────────────────────

/// 按扩展名分派的**流式**导入。返回的是骨架 `Book`（只有题名与目录），
/// 正文已随解析逐章落盘。
pub fn import_into(path: &Path, sink: &mut dyn ChapterSink) -> Result<Book> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut book = match ext.as_str() {
        "txt" | "text" => import_txt_into(path, sink)?,
        "epub" => import_epub_into(path, sink)?,
        // DOCX：OOXML 是「带样式的段落流」，先还原成纯文本，再走 TXT 漏斗
        // （切章 / 夹注归一 / 全角化 / 数字转汉字全复用同一套）。
        // 抽取阶段每 64 段 tick 一次，切章后逐章 tick——大文档不会假死。
        "docx" => crate::docx::import_docx_into(path, sink)?,
        // JSON：结构化书稿，整本读入后由 core 数据适配器归一为统一 `Book`，
        // 再逐章交 sink 落盘。
        // **必须向 sink 报进度**——否则进度条停在 0%，看门狗会在 3 秒后误判
        // 「卡死」而中止导入（v1.9.4 修复：此前 JSON 整条链路不报进度，
        // 大文件会被 False Kill 成「文件可能损坏」）。
        "json" => import_json_into(path, sink)?,
        // md / html 一般是单篇小书：走旧解析（整体读入），解析完立刻逐章
        // 交出去落盘、随即释放——内存峰值只多出「一份原文」。
        other => {
            let full = match other {
                "md" | "markdown" => import_md(path)?,
                "html" | "htm" | "xhtml" | "xml" => import_html(path)?,
                _ => {
                    return Err(anyhow!(
                        "不支持的格式：.{other}（支持 md / txt / html / xml / epub / docx / json）"
                    ))
                }
            };
            // 单篇小书也报一下进度，免得看门狗误杀。
            sink.tick(0.5, "雕版 落盘");
            let mut skeleton = Book::new(&full.title, &full.author);
            skeleton.cover = full.cover.clone();
            skeleton.tongjia = full.tongjia.clone();
            skeleton.head_notes = full.head_notes.clone();
            for ch in full.chapters {
                sink.push(ch);
            }
            sink.tick(1.0, "雕版 完成");
            skeleton
        }
    };
    // 行内夹注标记、前言跳过、目录补齐、书名限长——与旧入口同一套收尾。
    // 注意此时 `book.chapters` 还是空的（正文在库里），故这几步里凡是
    // 要读正文的（前言判据）都在 `import_*_into` 内部对着 sink 做过了。
    skip_front_matter_skeleton(&mut book, sink);
    book.ensure_toc();
    let max = crate::vclean::META_TITLE_MAX;
    book.title = tidy_meta(&book.title, max);
    book.cover.title = tidy_meta(&book.cover.title, max);
    book.author = tidy_meta(&book.author, max);
    Ok(book)
}

/// 骨架书的前言跳过：判据读的是 sink 里的**题名 + 样本**，
/// 动刀动的是库里的章（不把正文拉回内存）。
fn skip_front_matter_skeleton(book: &mut Book, sink: &mut dyn ChapterSink) {
    let titles = sink.titles();
    if titles.len() <= 1 {
        return;
    }
    let pairs: Vec<(&str, &str)> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| (t.as_str(), sink.sample(i)))
        .collect::<Vec<_>>();
    // ① 夹在正文中间的散页（书商插的「目录」「书影」）。
    let stray = crate::parsers::stray_front_indices(&pairs);
    if !stray.is_empty() {
        sink.drop_at(&stray);
    }
    // ② 卷首那一串连续的前言（题名表重新取一次：上面可能刚砍过散页）。
    let titles = sink.titles();
    let pairs: Vec<(&str, &str)> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| (t.as_str(), sink.sample(i)))
        .collect();
    let cut = crate::parsers::front_matter_cut(&pairs);
    if cut > 0 && cut < pairs.len() {
        sink.drop_front(cut);
    }
    // ③ 目录条目再按题名扫一道。
    let mut toc = std::mem::take(&mut book.toc);
    toc.retain(|e| !crate::parsers::is_front_matter_title(&e.title));
    book.toc = toc;
}

/// TXT **流式**导入：按行读（`BufReader::read_until`），绝不整本读进内存。
///
/// 编码先探一次头（8KB）：UTF-8 就按 UTF-8，否则按 GBK（老书 txt 多为
/// GBK）。换行符在两种编码里都不会落在多字节序列中间，故逐行解码安全。
pub fn import_txt_into(path: &Path, sink: &mut dyn ChapterSink) -> Result<Book> {
    use std::io::BufRead;

    let utf8 = detect_utf8(path);
    let file = std::fs::File::open(path)
        .with_context(|| format!("打开 txt 失败: {}", path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);

    let fallback = file_stem(path);
    let mut cur_title: String = "正文".into();
    let mut paras: Vec<Paragraph> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut done: usize = 0;
    let mut head: Vec<String> = Vec::new(); // 前几行留着认书名 / 作者
    // YAML / TOML 式元数据块（`---` … `---`）：notion / Obsidian / Jekyll 导出的
    // txt 常见。整块跳过——否则 `title:` `author:` 会变成第一章的第一段，
    // 第一列顶上凭空多出几行洋文。
    let mut front: u8 = 0; // 0=还没定 1=块内 2=已剥完

    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        done += n;
        // 每读满 256KB 报一次进度（别每汇报一次就打断解析）。
        if done % (256 * 1024) < n {
            sink.advance(done, "刻版");
        }
        let line = if utf8 {
            String::from_utf8_lossy(&buf).into_owned()
        } else {
            encoding_rs::GBK.decode(&buf).0.into_owned()
        };
        let l = line.trim().trim_start_matches('\u{FEFF}').trim();
        if l.is_empty() {
            continue;
        }
        // 元数据块：只判开头那一段，块一收口就再也不问（`front == 2`）。
        if front != 2 {
            if front == 0 {
                if l == "---" {
                    front = 1;
                    continue;
                }
                front = 2; // 第一行不是 `---` → 压根没有元数据块
            } else {
                if l == "---" || l == "..." {
                    front = 2;
                }
                continue; // 块内一律丢
            }
        }
        if head.len() < 6 {
            head.push(l.to_string());
        }
        // 章题：交掉手上的一章，另起一章。
        if crate::pipeline::is_chapter_head(l) && l.chars().count() <= 40 {
            if !paras.is_empty() {
                sink.push(Chapter {
                    title: cur_title.clone(),
                    paragraphs: std::mem::take(&mut paras),
                });
            }
            cur_title = l.to_string();
            continue;
        }
        paras.push(crate::pipeline::paragraph_of(l));
        // **硬上限**：攒够 5000 字就交出去——整本没有一条章题的书（扫描本
        // 常见）靠这一刀切成等长的卷，绝不会攒成百万字的一章。
        let acc: usize = paras.iter().map(|p| p.text.chars().count()).sum();
        if acc >= MAX_CHAPTER_CHARS {
            sink.push(Chapter {
                title: cur_title.clone(),
                paragraphs: std::mem::take(&mut paras),
            });
        }
    }
    if !paras.is_empty() {
        sink.push(Chapter {
            title: cur_title,
            paragraphs: paras,
        });
    }
    sink.advance(done.max(1), "刻版");

    let (title, author) = detect_title_author(&head.join("\n"));
    let title = if title.is_empty() {
        fallback
    } else {
        title
    };
    let mut book = Book::new(&title, &author);
    book.cover.title = title.clone();
    Ok(book)
}

/// 探一下文件是不是 UTF-8（只看头 8KB，比读全书便宜得多）。
fn detect_utf8(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return true;
    };
    let mut head = vec![0u8; 8192];
    let Ok(n) = f.read(&mut head) else {
        return true;
    };
    head.truncate(n);
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return true;
    }
    std::str::from_utf8(&head).is_ok()
}

/// EPUB **流式**导入：按 spine 顺序**逐篇**读一个 zip 条目、清洗、切片、
/// 交 sink 落盘，然后立刻丢掉这一篇——内存里从不存在第二篇。
///
/// 与 `import_epub` 同一套工序（vclean 五阶段清洗、nav/ncx 目录润色），
/// 差别只在「章往哪儿放」：旧的是 `book.chapters.push`，新的是
/// `sink.push`（落盘即弃）。
pub fn import_epub_into(path: &Path, sink: &mut dyn ChapterSink) -> Result<Book> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("打开 epub 失败: {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("epub 不是合法的 zip 容器")?;

    let container =
        read_zip_text(&mut zip, "META-INF/container.xml").context("缺少 META-INF/container.xml")?;
    let opf_path = find_attr(&container, "full-path")
        .ok_or_else(|| anyhow!("container.xml 里没有 full-path"))?;
    let opf = read_zip_text(&mut zip, &opf_path).context("读取 OPF 失败")?;
    let title = crate::vclean::clean_book_title(
        &extract_tag_text(&opf, "dc:title").unwrap_or_else(|| file_stem(path)),
    );
    let author = extract_tag_text(&opf, "dc:creator").unwrap_or_default();

    let base_dir = {
        let p = Path::new(&opf_path);
        p.parent().map(|d| d.to_path_buf()).unwrap_or_default()
    };
    let base_prefix = base_dir.to_string_lossy().replace('\\', "/");
    let join = |href: &str| -> String {
        if base_prefix.is_empty() {
            href.to_string()
        } else {
            format!("{}/{}", base_prefix, href)
        }
    };

    let spine_ids = extract_spine_ids(&opf);
    let items = extract_items(&opf);
    let manifest: std::collections::HashMap<String, String> = items
        .iter()
        .map(|(id, (href, _, _))| (id.clone(), href.clone()))
        .collect();

    let mut book = Book::new(title.clone(), author);
    book.cover.title = title.clone();

    let mut ordered: Vec<String> = Vec::new();
    for id in &spine_ids {
        if let Some(href) = manifest.get(id) {
            ordered.push(join(href));
        }
    }
    if ordered.is_empty() {
        ordered.extend(items.iter().map(|(_, (h, _, _))| join(h)));
    }

    let mut file_to_chapter: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let total = ordered.len().max(1);
    // **书眉去重**（流式版）：旧路径靠 `vclean::purge_running_headers`
    // 在全书到齐之后横扫一遍删重复段；流式下全书从不同时在场，故改成
    // **边走边记**：短段的出现次数攒够 3 次，往后见一次删一次（书眉是
    // 每叶都印的，正文里同一句短话不会连着出现这么多回）。
    // 只针对 ≤16 字的短段——长段不可能是书眉，绝不误伤正文。
    let mut header_seen: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    // ── v1.60.0：进度量纲修正（「卡死在 0%」的根因）──────────────────
    // 旧代码 `sink.advance(fi * 1_000_000 / total, …)` 传的是**千分比**，
    // 而 `DbSink::advance` 的分母是 `total_hint`（文件字节数，动辄几千万）→
    // 两者量纲不同，算出来的 pct 恒在 2% 以内，进度条纹丝不动。
    // 这里改用 `tick` 直报 0~1 的比率（EPUB 只能按「spine 文件数」推进，
    // 总字数要等解开才知道），并把**已解析字数**写进文案——用户看到的
    // 是「雕版 37 万字 / 第 120/860 篇」，不是一根死掉的进度条。
    let mut carved_chars = 0usize;
    for (fi, raw_href) in ordered.iter().enumerate() {
        let file_key = normalize_href(raw_href);
        let label = format!("雕版 {} 万字 · {}/{}", carved_chars / 10_000, fi, total);
        sink.tick(fi as f32 / total.max(1) as f32, &label);
        let Ok(html) = read_zip_text(&mut zip, &file_key) else {
            continue;
        };
        // 逐篇清洗（vclean 五阶段），清洗完 `html` 立刻离开作用域。
        let subs = crate::vclean::clean_epub(&html);
        if subs.iter().all(|c| c.content.trim().is_empty()) {
            continue;
        }
        let guessed = extract_html_title(&html).unwrap_or_else(|| {
            Path::new(&file_key)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("章")
                .to_string()
        });
        let mut first_idx: Option<usize> = None;
        for c in subs {
            let t = if c.title == "正文" {
                guessed.clone()
            } else {
                c.title.clone()
            };
            let mut chapter = Chapter {
                title: t,
                paragraphs: Vec::new(),
            };
            for line in c.content.lines() {
                let l = line.trim();
                if !l.is_empty() {
                    chapter.paragraphs.push(crate::pipeline::paragraph_of(l));
                }
            }
            // 书眉：同一句短段见到第 4 回起，直接剔掉（见上 `header_seen`）。
            //
            // 先按 `&str` 查表（命中就不必给这一句再分配一个 String）：书眉
            // 是每叶都印的，重复率极高，大书上这一手能省下几十万次堆分配
            // （v1.67.2）。
            chapter.paragraphs.retain(|p| {
                let t = p.text.trim();
                if t.chars().count() > 16 {
                    return true;
                }
                if let Some(n) = header_seen.get_mut(t) {
                    *n += 1;
                    return *n <= 3;
                }
                header_seen.insert(t.to_string(), 1);
                true
            });
            if !chapter.paragraphs.is_empty() {
                if first_idx.is_none() {
                    first_idx = Some(sink.len());
                }
                // v1.60.0：进度按**已解析字数**报（下一轮的标签就带它）。
                carved_chars += chapter.char_count();
                // **交出去**：切片 + 落盘 + 释放，这一章的正文到此为止。
                sink.push(chapter);
            }
        }
        if let Some(i) = first_idx {
            file_to_chapter.insert(file_key.clone(), i);
        }
    }

    if sink.is_empty() {
        return Err(anyhow!("epub 中未解析出正文"));
    }

    // ---- 目录：EPUB3 nav 优先，回退 EPUB2 ncx ----
    let nav_href = items
        .iter()
        .find(|(_, (_, _, props))| props.split_whitespace().any(|p| p == "nav"))
        .map(|(_, (h, _, _))| join(h));
    let ncx_href = spine_toc_id(&opf)
        .and_then(|id| manifest.get(&id).cloned())
        .map(|h| join(&h))
        .or_else(|| {
            items
                .iter()
                .find(|(_, (h, mtype, _))| {
                    mtype.contains("ncx") || h.to_ascii_lowercase().ends_with(".ncx")
                })
                .map(|(_, (h, _, _))| join(h))
        });

    let mut toc: Vec<TocEntry> = Vec::new();
    if let Some(href) = nav_href {
        if let Ok(nav) = read_zip_text(&mut zip, &href) {
            let entries = parse_nav_toc(&nav, &base_prefix);
            toc = resolve_toc(entries, &file_to_chapter);
        }
    }
    if toc.is_empty() {
        if let Some(href) = ncx_href {
            if let Ok(ncx) = read_zip_text(&mut zip, &href) {
                let entries = parse_ncx_toc(&ncx, &base_prefix);
                toc = resolve_toc(entries, &file_to_chapter);
            }
        }
    }

    if toc.is_empty() {
        // 没有目录：题名就用落盘时记下的那些。
        let titles = sink.titles();
        toc = titles
            .iter()
            .enumerate()
            .map(|(i, t)| TocEntry {
                title: t.clone(),
                chapter: i,
                level: 1,
                start_char_index: None,
            })
            .collect();
    } else {
        // 用目录标题改写库里的章题（正文早已落盘，改的是库里的 title 列）。
        for e in &toc {
            if let Some(t) = sink.titles().get(e.chapter).cloned() {
                if t.trim().is_empty() || looks_like_filename(&t) {
                    sink.set_title(e.chapter, &e.title);
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for e in &toc {
            if seen.insert(e.chapter) {
                sink.set_title(e.chapter, &e.title);
            }
        }
    }
    book.toc = toc;
    Ok(book)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_chapters() {
        let t = "第一章\n道可道。\n\n名可名。\n\n第二章\n天下皆知。";
        let b = book_from_plaintext("测试", t);
        assert_eq!(b.chapters.len(), 2);
        assert_eq!(b.chapters[0].title, "第一章");
        assert_eq!(b.chapters[0].paragraphs.len(), 2);
    }

    #[test]
    fn md_links_and_title() {
        let md = "# 道德经\n\n## 第一章\n\n这是[链接](http://a.com)和**粗体**。";
        let b = book_from_markdown("md", md);
        assert_eq!(b.title, "道德经");
        assert_eq!(b.chapters[0].title, "第一章");
        assert_eq!(b.chapters[0].paragraphs[0].text, "这是链接和粗体。");
    }

    #[test]
    fn inline_marks() {
        let (text, notes) = parse_inline_marks("不见≪现≫可欲〖注文〗");
        assert_eq!(text, "不见可欲");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].kind, NoteKind::Tongjia);
        assert_eq!(notes[0].at, 1); // 锚在「见」
        assert_eq!(notes[1].kind, NoteKind::Note);
        assert_eq!(notes[1].at, 3); // 锚在「欲」
    }

    /// v1.39.0 夹带散页：第一章本来就是正文时，`front_matter_cut` 返回 0，
    /// 夹在卷与卷之间的「目录」页就漏了。书级清扫必须按题名 + 无句读把
    /// 它抓出来，同时目录条目跟着重编号（后面几章的章号要减一）。
    #[test]
    fn stray_toc_page_between_volumes_is_dropped() {
        let mut b = Book::new("史记", "司马迁");
        for (title, body) in [
            ("卷一 五帝本纪第一", "黄帝者，少典之子，姓公孙。"),
            (
                "目录",
                "卷一 五帝本纪第一\n卷二 夏本纪第二\n卷三 殷本纪第三",
            ),
            ("卷二 夏本纪第二", "夏禹，名曰文命。"),
        ] {
            let mut c = Chapter {
                title: title.into(),
                paragraphs: Vec::new(),
            };
            c.paragraphs.push(Paragraph::plain(body));
            b.chapters.push(c);
        }
        b.toc = vec![
            TocEntry::new("卷一 五帝本纪第一", 0, 1),
            TocEntry::new("目录", 1, 2),
            TocEntry::new("卷二 夏本纪第二", 2, 1),
        ];
        skip_front_matter(&mut b);

        let left: Vec<&str> = b.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(
            left,
            vec!["卷一 五帝本纪第一", "卷二 夏本纪第二"],
            "夹带的目录页没删：{left:?}"
        );
        // 目录条目：删掉「目录」且后面那章的 chapter 从 2 变成 1。
        let got: Vec<(&str, usize)> = b.toc.iter().map(|e| (e.title.as_str(), e.chapter)).collect();
        assert_eq!(
            got,
            vec![("卷一 五帝本纪第一", 0), ("卷二 夏本纪第二", 1)],
            "目录没跟着重编号：{got:?}"
        );
    }

    /// v1.37.0 目录智能清洗：砍掉卷首附件章后，**目录条目必须同步前移并重
    /// 编号**，且 nav 里那些单独一条的「封面」「目录」也要一并丢——只认
    /// 章号会漏（它们可能指向一个侥幸没被砍掉的章）。
    #[test]
    fn toc_is_purged_along_with_the_front_matter() {
        let mut b = Book::new("通鉴", "司马光");
        for (title, body) in [
            ("封面", ""),
            ("胡三省简介", "胡三省，宋元之际史学家，以注通鉴名世。"),
            (
                "卷第一【周纪一】",
                "威烈王二十三年。初命晋大夫魏斯、赵籍、韩虔为诸侯。臣光曰。",
            ),
            ("卷第二【周纪二】", "周纪二正文。臣光曰。"),
        ] {
            let mut c = Chapter {
                title: title.into(),
                paragraphs: Vec::new(),
            };
            c.paragraphs.push(Paragraph::plain(body));
            b.chapters.push(c);
        }
        b.toc = vec![
            TocEntry::new("封面", 0, 1),
            TocEntry::new("胡三省简介", 1, 1),
            TocEntry::new("目录", 1, 2),
            TocEntry::new("卷第一【周纪一】", 2, 1),
            TocEntry::new("卷第二【周纪二】", 3, 1),
        ];
        skip_front_matter(&mut b);

        let left: Vec<&str> = b.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(left, vec!["卷第一【周纪一】", "卷第二【周纪二】"], "附件章没砍干净：{left:?}");
        let titles: Vec<&str> = b.toc.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["卷第一【周纪一】", "卷第二【周纪二】"],
            "目录没洗干净：{titles:?}"
        );
        assert_eq!(b.toc[0].chapter, 0, "目录章号没跟着重编号");
        assert_eq!(b.toc[1].chapter, 1);
    }

    /// `卷第一`（《资治通鉴》的卷次写法）必须能认成正文起手——v1.37.0 之前
    /// 只认 `卷一`，通鉴全书找不到一个起手，前言一章都砍不动。
    #[test]
    fn juan_di_yi_counts_as_the_body_start() {
        assert!(crate::parsers::is_body_start_title("卷第一【周纪一】"));
        assert!(crate::parsers::is_body_start_title("卷一　五帝本纪第一"));
        assert!(!crate::parsers::is_body_start_title("卷帙浩繁"));
    }

    #[test]
    fn json_plain_paragraph() {
        let j = r#"{"title":"T","chapters":[{"title":"C","paragraphs":["甲乙"]}]}"#;
        let b = parse_json_book(j, "fb").unwrap();
        assert_eq!(b.chapters[0].paragraphs[0].text, "甲乙");
        assert_eq!(b.cover.title, "T");
    }

    // ---- 数字清洗（源头治理）----

    #[test]
    fn plaintext_import_converts_digits_to_han() {
        // EPUB / 纯文本导入时数字一律转汉字：竖排不会再被拆成「2 / 2 / 3」。
        let b = book_from_plaintext("测试", "第一章\n建安223年，策为尚书郎。\n\n全书1980页，共15卷。");
        let got = &b.chapters[0].paragraphs[0].text;
        assert_eq!(got, "建安二二三年，策为尚书郎。");
        assert_eq!(&b.chapters[0].paragraphs[1].text, "全书一千九百八十页，共十五卷。");
    }

    #[test]
    fn epub_html_text_converts_digits() {
        // 与 import_epub 同一条清洗链（html_to_text 的最后一步）。
        let html = "<html><body><p>公元223年</p><p>1980页</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("二二三年"), "{text}");
        assert!(text.contains("一千九百八十页"), "{text}");
    }

    // ---- 数据适配器：兼容各种不规范导入 ----

    #[test]
    fn adapter_single_chapter_field() {
        // 单数 chapter（非 chapters）自动包成单章。
        let j = r#"{"title":"老子","chapter":{"title":"道经","content":"道可道\n名可名"}}"#;
        let b = adapt_json_book(j, "fb").unwrap();
        assert_eq!(b.chapters.len(), 1);
        assert_eq!(b.chapters[0].title, "道经");
        assert_eq!(b.chapters[0].paragraphs.len(), 2);
        assert_eq!(b.chapters[0].paragraphs[0].text, "道可道");
        assert_eq!(b.cover.title, "老子");
    }

    #[test]
    fn adapter_root_is_single_chapter() {
        // 根节点直接就是 { title, content }，整棵当作一章。
        let j = r#"{"title":"无题一章","content":"天地玄黄\n宇宙洪荒"}"#;
        let b = adapt_json_book(j, "fb").unwrap();
        assert_eq!(b.chapters.len(), 1);
        assert_eq!(b.chapters[0].title, "无题一章");
        assert_eq!(b.chapters[0].paragraphs.len(), 2);
        assert_eq!(b.cover.title, "无题一章");
    }

    #[test]
    fn adapter_root_is_array() {
        // 根直接是章节数组。
        let j = r#"[{"title":"一","content":"甲"},{"title":"二","content":"乙"}]"#;
        let b = adapt_json_book(j, "fb").unwrap();
        assert_eq!(b.chapters.len(), 2);
        assert_eq!(b.chapters[0].title, "一");
        assert_eq!(b.chapters[1].title, "二");
    }

    #[test]
    fn adapter_plain_text_not_json() {
        // 文件名 .json 但内容是纯文本 —— 包成「无题 · 正文」单章。
        let t = "春眠不觉晓\n处处闻啼鸟";
        let b = adapt_json_book(t, "fb").unwrap();
        assert_eq!(b.title, "无题");
        assert_eq!(b.cover.title, "无题");
        assert_eq!(b.chapters.len(), 1);
        assert_eq!(b.chapters[0].title, "正文");
        assert_eq!(b.chapters[0].paragraphs.len(), 2);
    }

    #[test]
    fn adapter_malformed_json_errors_classical() {
        // 像 JSON 却畸形（缺右括号 / 单引号）—— 报古籍风错误，且不崩溃。
        let j = r#"{"title":"断简","chapters":[{"title":"残篇","content":"残文"}"#;
        let err = adapt_json_book(j, "fb").unwrap_err();
        assert!(err.to_string().contains("竹简散乱"), "{}", err);
    }


    #[test]
    fn detect_title_author_from_txt() {
        // 常见版式：书名行 + 「作者：X」+ 章回正文。
        let t = "西游记\n作者：吴承恩\n\n第一回 灵根育孕源流出\n\n诗曰。";
        let (title, author) = detect_title_author(t);
        assert_eq!(title, "西游记");
        assert_eq!(author, "吴承恩");

        // 书名带《》，作者用「X 著」。
        let t = "《红楼梦》\n曹雪芹 著\n\n第一回 甄士隐";
        let (title, author) = detect_title_author(t);
        assert_eq!(title, "红楼梦");
        assert_eq!(author, "曹雪芹");

        // 正文首行是句子（有句读）→ 识别不出书名，交回退。
        let t = "道可道，非常道。\n作者：老子\n\n第一章";
        let (title, author) = detect_title_author(t);
        assert_eq!(title, "");
        assert_eq!(author, "老子");

        // 章回标题打头 → 不算书名。
        let t = "第一回 灵根育孕源流出\n\n诗曰。";
        let (title, _) = detect_title_author(t);
        assert_eq!(title, "");
    }

    #[test]
    fn book_from_plaintext_uses_detected_title() {
        let t = "西游记\n作者：吴承恩\n\n第一回 灵根育孕源流出\n\n诗曰。";
        let b = book_from_plaintext("下载的文件名", t);
        assert_eq!(b.title, "西游记");
        assert_eq!(b.author, "吴承恩");

        // 识别不出时回退文件名；超长名夹到 12 字以内。
        let b = book_from_plaintext("一二三四五六七八九十十一十二十三", "道可道，非常道。");
        let n = b.title.chars().count();
        assert!(n <= 12, "书名应夹到 12 字以内，实际 {n}");
        assert!(b.title.ends_with('…'));

        // 「文 / X」作者式样。
        let t = "水浒传\n文 / 施耐庵\n\n第一回 张天师祈禳瘟疫";
        let b = book_from_plaintext("x", t);
        // 首行是叶码（扫描本常态）→ 绝不能当成书名（v1.68.1）。
        let (title, _) = detect_title_author("十三\n道德经\n作者：老子");
        assert_eq!(title, "道德经", "叶码被当成书名了：{title}");
        assert_eq!(b.title, "水浒传");
        assert_eq!(b.author, "施耐庵");
    }

    /// v1.9.4 修复验证：**大 JSON 导入必须报进度，且不能 panic / 被看门狗误杀**。
    ///
    /// 旧实现 `import_into` 的 JSON 分支整条链路不向 sink 报进度——进度条停在
    /// 0%，看门狗会在 3 秒后误判「卡死」而中止导入（用户侧表现就是「JSON 导入
    /// 不丝滑 / 大文件报文件损坏」）。这里造一份 >10MB 的结构化 Book JSON，走
    /// `import_json_into`，断言：① 进度回调 `tick` 至少被调三次（读毕 / 落盘 /
    /// 完成）；② 大书被正常拆成多章；③ 不 panic。
    #[test]
    fn json_import_reports_progress_and_handles_large_file() {
        use std::io::Write;
        // 造料：每章一段长正文，凑到 >10MB。
        let para = "天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏闰余成岁律吕调阳云腾致雨露结为霜金生丽水玉出昆冈剑号巨阙珠称夜光。";
        let mut chapters = Vec::new();
        for i in 0..2000u32 {
            let content: String = (0..40).map(|_| para).collect::<String>();
            chapters.push(format!(
                "{{\"title\":\"卷{}\",\"paragraphs\":[\"{}\"]}}",
                i, content
            ));
        }
        let book_json = format!(
            "{{\"title\":\"大书\",\"author\":\"佚名\",\"chapters\":[{}]}}",
            chapters.join(",")
        );
        let bytes = book_json.len();
        assert!(bytes > 10_000_000, "测试样本须 >10MB，实际 {bytes} 字节");

        let dir = std::env::temp_dir().join(format!("jiandu-json-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("大书.json");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(book_json.as_bytes()).unwrap();
        }

        // 记录进度的 sink。
        struct RecSink {
            ticks: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
            pushes: usize,
        }
        impl ChapterSink for RecSink {
            fn push(&mut self, _c: Chapter) {
                self.pushes += 1;
            }
            fn len(&self) -> usize {
                self.pushes
            }
            fn sizes(&self) -> &[usize] {
                &[]
            }
            fn titles(&self) -> Vec<String> {
                Vec::new()
            }
            fn sample(&self, _i: usize) -> &str {
                ""
            }
            fn tick(&self, pct: f32, _label: &str) {
                self.ticks.lock().unwrap().push(pct);
            }
            fn advance(&mut self, _d: usize, _l: &str) {}
            fn drop_front(&mut self, _n: usize) {}
            fn drop_at(&mut self, _idxs: &[usize]) {}
            fn set_title(&mut self, _i: usize, _t: &str) {}
        }

        let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut sink = RecSink {
            ticks: ticks.clone(),
            pushes: 0,
        };
        let book = import_json_into(&path, &mut sink).expect("大 JSON 解析失败");
        let ticks = ticks.lock().unwrap().clone();
        assert!(
            ticks.len() >= 3,
            "JSON 导入必须至少报三次进度（读毕/落盘/完成），实际 {} 次：{:?}",
            ticks.len(),
            ticks
        );
        assert!(sink.pushes >= 100, "大书应拆成多章，实际 {} 章", sink.pushes);
        assert!(!book.title.is_empty());
        std::fs::remove_file(&path).ok();
    }
}
