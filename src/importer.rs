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
        "json" => import_json(path)?,
        "epub" => import_epub(path)?,
        other => {
            return Err(anyhow!(
                "不支持的格式：.{other}（支持 md / txt / epub / json）"
            ))
        }
    };
    // 没有目录的格式（txt / md / json）在这里统一按章节标题补齐。
    book.ensure_toc();
    // 书名统一夹到 12 字以内（自动识别 + 限长，书架 / 封面竖排区域有限）。
    book.title = clip_chars(&book.title, 12);
    if book.cover.title.chars().count() > 12 {
        book.cover.title = clip_chars(&book.cover.title, 12);
    }
    if book.author.chars().count() > 12 {
        book.author = clip_chars(&book.author, 12);
    }
    Ok(book)
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未命名")
        .to_string()
}

/// 按字数截断：超长缀省略号（书名 / 作者名夹字用）。
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
        // ---- 书名：第一个短行（无句读、非章回标题）----
        if title.is_empty() && !is_heading(line) && line.chars().count() <= 16 {
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
fn para_of(line: &str) -> Paragraph {
    let (text, notes) = parse_inline_marks(line.trim());
    Paragraph { text, notes }
}

// ---------------------------------------------------------------------------
// txt
// ---------------------------------------------------------------------------
pub fn import_txt(path: &Path) -> Result<Book> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 txt 失败: {}", path.display()))?;
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
pub fn book_from_plaintext(fallback_title: &str, text: &str) -> Book {
    let (detected, author) = detect_title_author(text);
    let title = if detected.is_empty() {
        fallback_title.to_string()
    } else {
        detected
    };
    let title = clip_chars(&title, 12);
    let author = clip_chars(&author, 12);
    let mut book = Book::new(&title, &author);
    let normalized = text.replace('\r', "");
    let mut current: Option<Chapter> = None;
    let mut para: Vec<String> = Vec::new();

    for raw in normalized.lines() {
        let line = raw.trim();
        if line.is_empty() {
            flush_para(&mut para, &mut current, &mut book);
            continue;
        }
        if is_heading(line) {
            flush_para(&mut para, &mut current, &mut book);
            if let Some(c) = current.take() {
                if !c.paragraphs.is_empty() {
                    book.chapters.push(c);
                }
            }
            current = Some(Chapter {
                title: line.trim_start_matches('#').trim().to_string(),
                paragraphs: Vec::new(),
            });
            continue;
        }
        para.push(line.to_string());
    }
    flush_para(&mut para, &mut current, &mut book);
    if let Some(c) = current.take() {
        if !c.paragraphs.is_empty() {
            book.chapters.push(c);
        }
    }

    if book.chapters.is_empty() {
        book.chapters.push(Chapter {
            title: "正文".into(),
            paragraphs: vec![para_of(&normalized)],
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
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 markdown 失败: {}", path.display()))?;
    Ok(book_from_markdown(&file_stem(path), &text))
}

/// Markdown -> Book：`#` 标题切章（首个一级标题作书名），空行分段。
pub fn book_from_markdown(fallback_title: &str, md: &str) -> Book {
    let mut book = Book::new(fallback_title, "");
    let mut current: Option<Chapter> = None;
    let mut para: Vec<String> = Vec::new();
    let mut title_taken = false;

    for raw in md.replace('\r', "").lines() {
        let line = raw.trim();
        if line.is_empty() {
            flush_para(&mut para, &mut current, &mut book);
            continue;
        }
        // 标题
        if line.starts_with('#') {
            flush_para(&mut para, &mut current, &mut book);
            let level = line.chars().take_while(|c| *c == '#').count();
            let heading = strip_md_inline(line.trim_start_matches('#').trim());
            if level == 1 && !title_taken && book.chapters.is_empty() {
                book.title = heading.clone();
                book.cover.title = heading;
                title_taken = true;
                continue;
            }
            if let Some(c) = current.take() {
                if !c.paragraphs.is_empty() {
                    book.chapters.push(c);
                }
            }
            current = Some(Chapter {
                title: heading,
                paragraphs: Vec::new(),
            });
            continue;
        }
        // 分隔线 / 引用标记
        if line.chars().all(|c| c == '-' || c == '*' || c == '_') {
            flush_para(&mut para, &mut current, &mut book);
            continue;
        }
        para.push(strip_md_inline(line));
    }
    flush_para(&mut para, &mut current, &mut book);
    if let Some(c) = current.take() {
        if !c.paragraphs.is_empty() {
            book.chapters.push(c);
        }
    }
    if book.chapters.is_empty() {
        book.chapters.push(Chapter {
            title: "正文".into(),
            paragraphs: Vec::new(),
        });
    }
    if book.cover.title.is_empty() {
        book.cover.title = book.title.clone();
    }
    book
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
pub fn import_json(path: &Path) -> Result<Book> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("读取 json 失败: {}", path.display()))?;
    parse_json_book(&text, &file_stem(path))
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

pub fn parse_json_book(text: &str, fallback_title: &str) -> Result<Book> {
    // 形态 1：完整的 Book 结构
    if let Ok(mut book) = serde_json::from_str::<Book>(text) {
        if book.cover.title.is_empty() {
            book.cover.title = book.title.clone();
        }
        return Ok(book);
    }

    // 形态 2：{title, author, chapters:[{title, content | paragraphs}]}
    let v: serde_json::Value = serde_json::from_str(text)
        .with_context(|| "json 解析失败（需要 Book 结构或等价的 title/chapters 结构）")?;
    let title = v
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or(fallback_title)
        .to_string();
    let author = v
        .get("author")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
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
        for ch in chs {
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
                            if let Ok(pp) =
                                serde_json::from_value::<Paragraph>(p.clone())
                            {
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
            book.chapters.push(Chapter {
                title: ct,
                paragraphs,
            });
        }
    }

    if book.chapters.is_empty() {
        return Err(anyhow!("json 中未找到 chapters"));
    }
    Ok(book)
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
    let title = extract_tag_text(&opf, "dc:title").unwrap_or_else(|| file_stem(path));
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
    book.cover.title = title;

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
            let text = html_to_text(&html);
            if text.trim().is_empty() {
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
            let mut chapter = Chapter {
                title: guessed,
                paragraphs: Vec::new(),
            };
            for line in text.lines() {
                let l = line.trim();
                if !l.is_empty() {
                    chapter.paragraphs.push(para_of(l));
                }
            }
            if !chapter.paragraphs.is_empty() {
                file_to_chapter.insert(file_key.clone(), book.chapters.len());
                book.chapters.push(chapter);
            }
        }
    }

    if book.chapters.is_empty() {
        return Err(anyhow!("epub 中未解析出正文"));
    }

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

fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    s = remove_block(&s, "<script", "</script>");
    s = remove_block(&s, "<style", "</style>");
    s = remove_block(&s, "<head", "</head>");

    for (a, b) in [
        ("</p>", "\n"),
        ("</div>", "\n"),
        ("</h1>", "\n"),
        ("</h2>", "\n"),
        ("</h3>", "\n"),
        ("<br/>", "\n"),
        ("<br />", "\n"),
        ("<br>", "\n"),
    ] {
        s = s.replace(a, b);
    }

    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    let mut cleaned = String::new();
    let mut blank = 0;
    for line in out.lines() {
        let l = line.trim();
        if l.is_empty() {
            blank += 1;
            if blank <= 1 {
                cleaned.push('\n');
            }
        } else {
            blank = 0;
            cleaned.push_str(l);
            cleaned.push('\n');
        }
    }
    cleaned
}

fn remove_block(s: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        match rest[i..].find(close) {
            Some(j) => rest = &rest[i + j + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
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

    #[test]
    fn json_plain_paragraph() {
        let j = r#"{"title":"T","chapters":[{"title":"C","paragraphs":["甲乙"]}]}"#;
        let b = parse_json_book(j, "fb").unwrap();
        assert_eq!(b.chapters[0].paragraphs[0].text, "甲乙");
        assert_eq!(b.cover.title, "T");
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
        assert_eq!(b.title, "水浒传");
        assert_eq!(b.author, "施耐庵");
    }
}
