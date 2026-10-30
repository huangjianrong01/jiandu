//! **DOCX 导入**（v1.9.5 · `feature/docx-import`）：zip 解包 → 流式读
//! `word/document.xml` → 抽 `<w:t>` → 喂给 TXT 漏斗。
//!
//! # 为什么走「喂 TXT 漏斗」这条路
//!
//! Word 文档在结构上就是「一堆段落」，只不过每段带了个样式名。而 TXT 漏斗
//! （[`crate::importer::book_from_plaintext`] → `parse_txt`）已经把中文书稿
//! 该处理的事全做完了：剥头部幽灵、认章节标题、全角化、数字转汉字、以及
//! **把 `〔…〕` / `【…】` 归一成 `[注]…[/注]` 双行夹注**。再写一套 DOCX 专用
//! 切章逻辑，等于把踩过的坑重踩一遍。
//!
//! 所以这里的职责只有一件事：**把 OOXML 还原成「像 TXT 一样的纯文本流」**。
//!
//! # 只认三样东西
//!
//! * `<w:p>` 的起止 = 段落边界（段落之间留一个空行，`parse_txt` 据此分段）；
//! * `<w:t>` 的文本 = 正文（必须解实体：`&amp;` 不解开就是字面的 `&amp;`）；
//! * `<w:pStyle w:val="…">` / `<w:outlineLvl w:val="…">` = 段落级别（标题 / 正文）。
//!
//! # 主动跳过的四类「隐性内容」
//!
//! | 节点 | 为什么要跳 |
//! |---|---|
//! | `<w:tbl>` | 表格里的字不是行文；MVP 直接整块忽略（用户口径） |
//! | `<w:drawing>` / `<w:pict>` / `<w:object>` | 图、图形、嵌入对象，含文本框——里面的 `w:t` 排进正文只会变成乱码般的碎片 |
//! | `<w:instrText>` | 域代码（`PAGE \* MERGEFORMAT`），不是读者要看的字 |
//! | `<w:delText>` / `<w:del>` | 修订模式下**被删除**的文字，排进去等于把作者删掉的稿子捡回来 |
//!
//! 另外：Word 的**目录域**（`TOC \h`）会自动生成一份「章名列表」，那是索引不是
//! 正文。整段跳过——否则一本书前面会凭空多出几百个只有标题、没有内容的幽灵章。
//! 域外的「Table of Contents / 目录 / 目錄」这类**整段标签行**也一律丢弃
//! （见 [`is_toc_label`]）；正文里出现「目录」两个字的句子不受影响。
//!
//! # 书名从哪来
//!
//! 先读包里的 `docProps/core.xml` → `dc:title`（作者导出时填的真书名），
//! 读不到才退回**文件名**。两者都不例外地过一遍 [`tidy_docx_title`]：
//! 先剥序跋 / 注疏题名（`胡三省新註資治通鑑序` → `資治通鑑`），再走全项目
//! 统一的 [`crate::vclean::clean_book_title`]。**绝不用正文首行猜书名**——
//! 首行多是「某某序」，一本《資治通鑑》会被命名成《胡三省新註資治通鑑序》。
//!
//! # 容错（绝不 Panic）
//!
//! * 缺 `word/document.xml` → 一句人话报错（老版 `.doc` 就长这样）；
//! * XML 截断 / 半路坏掉 → 保留已读到的部分，不抛错；
//! * 括号数量对不上 → 交给 TXT 漏斗的栈式容错，这里只做「配对且够短」的
//!   `{…}` → `〔…〕` 预归一（见 [`curly_to_note`]）。

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::stream::ChapterSink;
use crate::types::Book;

/// 一段的归类（MVP 只有两级：标题 / 正文）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParaKind {
    /// 正文（`Normal` / 无样式 / 认不出的样式）。
    #[default]
    Body,
    /// 章节标题（`Heading 1..9` / `标题 1` / `Title` / 段落带 `outlineLvl ≤ 5`）。
    Heading,
}

/// 抽取结果的小账本（给日志与单测看，不参与排版）。
#[derive(Debug, Default, Clone, Copy)]
pub struct ExtractStats {
    /// 非空段落数。
    pub paragraphs: usize,
    /// 其中被判为标题的段落数。
    pub headings: usize,
}

/// 从 `document.xml` 抽正文（纯函数版，给单测用）。返回（纯文本, 账本）。
pub fn extract_docx_text(xml: &str) -> (String, ExtractStats) {
    let mut sink = ExtractSink::default();
    let reader = Reader::from_str(xml);
    // 截断 / 坏 XML 一律保已读部分：这里不接受 Err。
    let _ = run_extract(reader, &mut sink, &mut |_, _| {});
    (sink.out, sink.stats)
}

/// 从任意 `Read`（zip 里的 `document.xml` 条目）流式抽取。
///
/// `on_tick(段落数, 标题数)` 会在每 64 段回调一次——给看门狗喂心跳，
/// 免得大文件解析期间被判「卡死」。
pub fn extract_docx_stream<R: Read>(
    r: R,
    on_tick: &mut dyn FnMut(usize, usize),
) -> Result<(String, ExtractStats)> {
    let mut sink = ExtractSink::default();
    let reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, r));
    run_extract(reader, &mut sink, on_tick)?;
    Ok((sink.out, sink.stats))
}

/// **导入入口**：解开 docx → 抽文本 → 喂 TXT 漏斗 → 逐章交 sink 落盘。
///
/// 与 [`crate::importer::import_docx`]（老条目路径，整体读入内存、返回整本
/// `Book`）的区别：这里全程向 `sink` 报进度，且章节是「一路推下去」的，
/// 大文件不会让进度条假死、也不会被看门狗误杀。
pub fn import_docx_into(path: &Path, sink: &mut dyn ChapterSink) -> Result<Book> {
    sink.tick(0.02, "刻本 解开 DOCX");

    let file = std::fs::File::open(path).with_context(|| format!("读不出 {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| anyhow!("这不是一个能打开的 docx（zip 打不开：{e}）"))?;

    // v1.9.9：先读 `word/footnotes.xml`。古籍的双行夹注在 Word 里常常就是
    // 脚注，老漏斗只抽 document.xml，注文整片丢，正文里只剩一个光秃秃的注号。
    // 读不到（包里没有脚注）就是空表，后面 `attach_footnotes` 自然什么也不做。
    let notes = read_footnotes(&mut zip);

    // 正文只认主文档；缺了它就是坏包 / 老版 .doc。
    let (text, stats) = {
        let mut entry = zip.by_name("word/document.xml").map_err(|_| {
            anyhow!("包里没有 word/document.xml——是老版 .doc 吧？请用 Word 另存为 .docx")
        })?;
        let mut cb = |paras: usize, heads: usize| {
            // 抽取阶段占 0.05 → 0.55：段落越多越接近上限，永不到顶（不确定总量）。
            let frac = paras as f32 / (paras as f32 + 2_500.0);
            sink.tick(
                0.05 + 0.50 * frac,
                &format!("刻本 抽取 {paras} 段 · 标题 {heads}"),
            );
        };
        extract_docx_stream(&mut entry, &mut cb)?
    };

    // 哨兵 → 真注文（`[注]…[/注]`，下游 TXT 漏斗自然落成双行夹注）。
    let text = crate::dclean::attach_footnotes(&text, &notes);

    if text.trim().is_empty() {
        return Err(anyhow!(
            "这份 docx 里一个字都没取到——正文可能全在图片里（扫描件），或文档是空的"
        ));
    }

    sink.tick(
        0.58,
        &format!("刻本 {} 段 · 切章", stats.paragraphs.max(1)),
    );

    // ── 喂现有 TXT 漏斗：这里同时完成 夹注归一 / 切章 / 全角化 / 数字转汉字 ──
    let fallback = crate::importer::file_stem(path);
    let full = crate::importer::book_from_plaintext(&fallback, &text);
    drop(text); // 原文到此为止，后面只剩章数组

    // 元数据：书名 / 作者都从这里来（见 `read_core_props`）。
    // 书名**先信 `dc:title`**，读不到才退回文件名。无论如何都不猜正文首行——
    // 首行多是「某某序」，一本《資治通鑑》会被命名成《胡三省新註資治通鑑序》。
    // 两条来源都要过一遍 `tidy_docx_title`：元数据本身也可能是脏的
    // （序跋 / 注疏题名 / 网址 / 拼音），而书架只该摆一个干净的名字。
    let (meta_title, meta_author) = read_core_props(&mut zip);
    let mut full = full;
    let title = meta_title
        .as_deref()
        .and_then(tidy_docx_title)
        .or_else(|| tidy_docx_title(&fallback));
    if let Some(t) = title {
        full.title = t;
        full.cover.title = full.title.clone();
    }
    // 作者：元数据里的 `dc:creator` 比正文首行可靠得多（古籍序跋本的
    // 首行是篇题，认不出作者）。清洗后仍为空才保留漏斗认出来的那个。
    if let Some(a) = meta_author.as_deref().and_then(tidy_docx_author) {
        full.author = a;
    }

    let n = full.chapters.len().max(1);
    let mut skeleton = Book::new(&full.title, &full.author);
    skeleton.cover = full.cover.clone();
    skeleton.tongjia = full.tongjia.clone();
    skeleton.head_notes = full.head_notes.clone();

    for (i, ch) in full.chapters.into_iter().enumerate() {
        sink.push(ch);
        let pct = 0.62 + 0.38 * ((i + 1) as f32 / n as f32);
        sink.tick(pct.min(0.999), &format!("刻本 {}/{} 章", i + 1, n));
    }
    sink.tick(1.0, "刻本 完成");
    Ok(skeleton)
}

/// 从包里的 `word/footnotes.xml` 读脚注表（`脚注 id → 注文`）。
///
/// 包里没这个条目（多数 docx 根本没有脚注）→ 返回空表，不是错误。XML 坏 /
/// 截断 → 保留已读到的部分（与 `dclean::parse_footnotes_xml` 同口径）。
fn read_footnotes<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> std::collections::HashMap<i64, String> {
    let Ok(mut entry) = zip.by_name("word/footnotes.xml") else {
        return std::collections::HashMap::new();
    };
    let mut xml = String::new();
    if std::io::Read::read_to_string(&mut entry, &mut xml).is_err() {
        return std::collections::HashMap::new();
    }
    crate::dclean::parse_footnotes_xml(&xml)
}

// ---------------------------------------------------------------------------
// 元数据：docProps/core.xml → dc:title / dc:creator
// ---------------------------------------------------------------------------

/// 从包里的 `docProps/core.xml` 读 `(dc:title, dc:creator)`。
///
/// Word / Pages / WPS / Calibre 导出时都会写这两项，比猜正文首行可靠得多。
/// 读不到（包不完整、字段是空的、XML 坏）→ 对应项为 `None`。
fn read_core_props<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> (Option<String>, Option<String>) {
    // 元数据文件正常只有几百字节；仍上 256KiB 上限，防畸形包把内存吃掉。
    let Some(entry) = zip.by_name("docProps/core.xml").ok() else {
        return (None, None);
    };
    let mut buf = Vec::with_capacity(1024);
    if entry.take(256 * 1024).read_to_end(&mut buf).is_err() {
        return (None, None);
    }
    parse_core_props(&String::from_utf8_lossy(&buf))
}

/// 从 `core.xml` 里刮出 `dc:title` 与 `dc:creator`（纯函数，给单测用）。
///
/// 按**本地名**认元素（`title` / `creator`），不认前缀——不同工具写出来的
/// 前缀花样很多（`dc:` `dcterms:` 甚至没有前缀），认前缀就会漏。
/// 每一项都**必须读到闭合标签才算数**：包被截断时留下的半个书名 /
/// 半个作者名比没有还糟。
fn parse_core_props(xml: &str) -> (Option<String>, Option<String>) {
    #[derive(Clone, Copy, PartialEq)]
    enum F {
        Title,
        Creator,
    }
    let local = |e: &quick_xml::events::BytesStart<'_>| -> String {
        let n = e.name();
        String::from_utf8_lossy(n.local_name().as_ref()).to_ascii_lowercase()
    };
    let mut reader = Reader::from_str(xml);
    let mut field: Option<F> = None;
    let mut buf = String::new();
    let mut title: Option<String> = None;
    let mut creator: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local(&e);
                field = match name.as_str() {
                    "title" => Some(F::Title),
                    "creator" => Some(F::Creator),
                    _ => None,
                };
                buf.clear();
            }
            Ok(Event::Text(e)) => {
                if field.is_some() {
                    if let Ok(s) = e.unescape() {
                        buf.push_str(&s);
                    }
                }
            }
            Ok(Event::CData(e)) => {
                if field.is_some() {
                    buf.push_str(&String::from_utf8_lossy(e.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().local_name().as_ref()).to_lowercase();
                let t = buf.trim().to_string();
                if name == "title" && field == Some(F::Title) && title.is_none() && !t.is_empty() {
                    title = Some(t.clone());
                }
                if name == "creator" && field == Some(F::Creator) && creator.is_none() && !t.is_empty()
                {
                    creator = Some(t);
                }
                field = None;
                buf.clear();
            }
            // 坏 XML / 读到尾：拿已经刮到的部分，绝不报错。
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    (title, creator)
}

/// 作者名清洗：`[宋] 司馬光 編著 / [元] 胡三省 音注` → `司馬光、胡三省`。
///
/// `dc:creator` 是「人 + 角色」的自由文本，直接上书架会又长又乱。三步：
/// 1. 按 `/ ／ ; ；` 拆成多个人；
/// 2. 每个剥掉朝代 / 籍贯括注（`[宋]` `〔元〕` `（清）`）与角色词
///    （編著 / 音注 / 撰 / 校注…）；
/// 3. 多个人用「、」连起来，超 24 字截断。
fn tidy_docx_author(raw: &str) -> Option<String> {
    /// 角色词：从长到短匹配（「校注」要先于「注」被试到）。
    const ROLES: &[&str] = &[
        "編著", "编著", "主編", "主编", "校注", "校訂", "校订", "音註", "音注", "註釋", "注释",
        "譯註", "译注", "翻譯", "翻译", "撰寫", "撰写", "原著", "著", "撰", "編", "编", "譯", "译",
        "註", "注", "輯", "辑", "續", "续", "補", "补", "訂", "订", "等",
    ];
    let mut names: Vec<String> = Vec::new();
    for part in raw.split(['/', '／', ';', '；', '、', ',', '，']) {
        // ① 剥括注（朝代 / 籍贯 / 生卒）——统一走 vclean 的实现。
        let mut t = crate::vclean::drop_bracketed(part);
        // ② 剥角色词。
        for r in ROLES {
            t = t.replace(r, " ");
        }
        // ③ 空白折成一个、首尾去白。
        let folded = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if folded.chars().count() >= 2 {
            names.push(folded);
        }
    }
    // 去重（同一人重复署名很常见），保持原顺序。
    names.dedup();
    if names.is_empty() {
        return None;
    }
    let mut out = names.join("、");
    if out.chars().count() > 24 {
        out = out.chars().take(24).collect();
    }
    Some(out)
}

/// 序跋标记：出现即视为「书名到此为止」。
const PREFACE_TAIL: &[&str] = &["序言", "自序", "原序", "後序", "后序", "序", "跋", "弁言", "引言"];

/// 注释**版本**标记：书名常写成「某書集注 / 某書音注」，真书名在标记**之前**。
/// 只收两字以上的组合——单字「注」不能当尾标记，否则《水經注》会被切歪。
const COMMENT_TAIL: &[&str] = &[
    "新註", "新注", "音註", "音注", "箋註", "箋注", "集註", "集注", "註疏", "注疏",
];

/// 注释标记：书名常写成「某某新註某書」，真书名在标记**之后**。
const COMMENT_HEAD: &[&str] = &[
    "新註", "新注", "音註", "音注", "箋註", "箋注", "集註", "集注", "註", "注", "箋", "笺", "疏",
    "評", "评",
];

/// 剥序跋 / 注疏题名：`胡三省新註資治通鑑序` → `資治通鑑`。
///
/// 只下两刀，且每刀都要求**切口两侧都还有字**，否则宁可不动——
/// 《秩序》《程序》《水經注》这些带「序 / 注」的正常书名一个字都不能少。
fn strip_preface_noise(raw: &str) -> String {
    let src = raw.trim();
    if src.is_empty() {
        return String::new();
    }

    // 第一刀：切掉「序 / 跋」与「集注 / 音注」这类**尾标记**。
    // 两个条件缺一不可：
    //   * 标记**后面基本没字**（≤2 字）——否则它就不是尾巴，而是前缀
    //     （`胡三省新註資治通鑑` 的「新註」后面还有四个字，不能在这儿切）；
    //   * 标记**前面至少两个字**——《秩序》的「序」在第二字，不切。
    let mut cut = None;
    for m in PREFACE_TAIL.iter().chain(COMMENT_TAIL.iter()) {
        if let Some(i) = src.find(m) {
            let head = src[..i].trim().chars().count();
            let tail = src[i + m.len()..].trim().chars().count();
            if head >= 2 && tail <= 2 && cut.map_or(true, |c: usize| i < c) {
                cut = Some(i);
            }
        }
    }
    let t = match cut {
        Some(i) => src[..i].trim().to_string(),
        None => src.to_string(),
    };

    // 第二刀：切掉「注 / 疏 / 評」之前的头。两侧都要有字。
    let mut head_end = None;
    for m in COMMENT_HEAD {
        if let Some(i) = t.find(m) {
            let before = t[..i].trim().chars().count();
            let after = t[i + m.len()..].trim().chars().count();
            if before >= 2 && after >= 2 && head_end.map_or(true, |c: usize| i < c) {
                head_end = Some(i + m.len());
            }
        }
    }
    let out = match head_end {
        Some(i) => t[i..].trim().to_string(),
        None => t,
    };

    // 切得太狠（剩不到两个字）→ 退回原样，让后面的清洗去处理。
    if out.chars().count() >= 2 {
        out
    } else {
        src.to_string()
    }
}

/// 书名收口：剥序跋题名 → 走全项目统一的 [`crate::vclean::clean_book_title`]
/// （粉碎网址 / 拼音 / 站名 / 符号，并按 `META_TITLE_MAX` 硬截）。
///
/// 与 TXT / EPUB 链**同一个清洗器**，不另立规矩。清洗后不足两字 → `None`
/// （调用方保留漏斗自己认出来的书名）。
fn tidy_docx_title(raw: &str) -> Option<String> {
    let t = strip_preface_noise(raw);
    let t = crate::vclean::clean_book_title(&t);
    let t = t.trim().to_string();
    if t.chars().count() >= 2 {
        Some(t)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 内部：一趟 quick-xml 扫描
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ExtractSink {
    /// 最终纯文本（段落之间空行分隔）。
    out: String,
    /// 当前段落正在攒的字。
    para: String,
    /// 当前段落的级别（`w:pStyle` / `w:outlineLvl` 决定）。
    kind: ParaKind,
    /// 表格 / 图形 / 嵌入对象 / 修订删除的嵌套深度：>0 时一个字都不要。
    skip: usize,
    /// `<w:instrText>` 嵌套深度（域代码：不输出，但要读出来认 TOC）。
    instr: usize,
    /// 当前域代码内容。
    instr_buf: String,
    /// 域嵌套深度。
    fld: usize,
    /// 刚读到过 `TOC` 域代码，等它的 `separate` 出现就进入目录区。
    toc_pending: bool,
    /// 正在目录域的结果区里：整段丢弃。
    in_toc: bool,
    stats: ExtractStats,
}

impl ExtractSink {
    fn flush(&mut self) {
        // v1.9.9：`<w:br/>`（软回车）与缩进垃圾必须在**这里**收拾掉——一旦
        // 进了纯文本就分不出「哪个换行是软回车」。软回车两侧都是中文就直接
        // 接上（不留缝，留缝就是竖排里的空格子），段首的全角空格 / Tab 全
        // 部 trim。见 [`crate::dclean::tidy_docx_para`]。
        let t = crate::dclean::tidy_docx_para(&self.para);
        if t.is_empty() {
            self.para.clear();
            return;
        }
        // 目录的**标题行**（「Table of Contents」/「目录」）站在域外面，
        // 不单独挡掉就会变成「正文」第一章里孤零零的一行洋文。
        if is_toc_label(&t) {
            self.para.clear();
            return;
        }
        self.stats.paragraphs += 1;
        if self.kind == ParaKind::Heading {
            self.stats.headings += 1;
        }
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        self.out.push_str(&curly_to_note(&t));
        self.out.push('\n');
        self.para.clear();
    }
}

fn run_extract<R: BufRead>(
    mut reader: Reader<R>,
    st: &mut ExtractSink,
    on_tick: &mut dyn FnMut(usize, usize),
) -> Result<()> {
    reader.config_mut().trim_text(false);
    let mut buf: Vec<u8> = Vec::new();
    let mut since_tick = 0usize;

    loop {
        // 坏 XML / 截断：保已读部分，绝不把整本书判死。
        let ev = match reader.read_event_into(&mut buf) {
            Ok(ev) => ev,
            Err(_) => break,
        };
        match ev {
            Event::Eof => break,

            Event::Start(e) => match local(e.local_name().as_ref()).as_str() {
                "p" if st.skip == 0 => {
                    st.para.clear();
                    st.kind = ParaKind::Body;
                }
                "pstyle" if st.skip == 0 => {
                    if let Some(v) = attr(&e, b"val") {
                        st.kind = style_kind(&v);
                    }
                }
                "outlinelvl" if st.skip == 0 => {
                    if let Some(v) = attr(&e, b"val") {
                        if v.trim().parse::<u8>().map(|n| n <= 5).unwrap_or(false) {
                            st.kind = ParaKind::Heading;
                        }
                    }
                }
                "instrtext" => {
                    st.instr += 1;
                    st.instr_buf.clear();
                }
                "tbl" | "drawing" | "pict" | "object" | "del" | "deltext" => st.skip += 1,
                "tab" if st.skip == 0 && st.instr == 0 && !st.in_toc => st.para.push(' '),
                "br" if st.skip == 0 && st.instr == 0 && !st.in_toc => st.para.push('\n'),
                _ => {}
            },

            Event::Empty(e) => match local(e.local_name().as_ref()).as_str() {
                // `<w:pStyle w:val="Heading1"/>` 是自闭合的，最常见形态。
                "pstyle" if st.skip == 0 => {
                    if let Some(v) = attr(&e, b"val") {
                        st.kind = style_kind(&v);
                    }
                }
                "outlinelvl" if st.skip == 0 => {
                    if let Some(v) = attr(&e, b"val") {
                        if v.trim().parse::<u8>().map(|n| n <= 5).unwrap_or(false) {
                            st.kind = ParaKind::Heading;
                        }
                    }
                }
                "fldchar" => {
                    let t = attr(&e, b"fldCharType").unwrap_or_default().trim().to_ascii_lowercase();
                    match t.as_str() {
                        "begin" => st.fld += 1,
                        "separate" => {
                            if st.toc_pending {
                                st.in_toc = true;
                            }
                        }
                        "end" => {
                            st.fld = st.fld.saturating_sub(1);
                            if st.fld == 0 {
                                st.in_toc = false;
                                st.toc_pending = false;
                            }
                        }
                        _ => {}
                    }
                }
                "tab" if st.skip == 0 && st.instr == 0 && !st.in_toc => st.para.push(' '),
                "br" if st.skip == 0 && st.instr == 0 && !st.in_toc => st.para.push('\n'),
                // v1.9.9：脚注引用 —— 埋一个哨兵，等整篇抽完再换成真注文
                // （见 `dclean::attach_footnotes`）。注号本身不落进正文，
                // 免得变成光秃秃一个数字杵在句子里。
                "footnotereference" if st.skip == 0 => {
                    if let Some(v) = attr(&e, b"id") {
                        if let Ok(n) = v.trim().parse::<i64>() {
                            st.para.push_str(&crate::dclean::footnote_sentinel(n));
                        }
                    }
                }
                _ => {}
            },

            Event::End(e) => {
                let name = local(e.local_name().as_ref());
                match name.as_str() {
                    "instrtext" => {
                        st.instr = st.instr.saturating_sub(1);
                        if st.instr == 0 {
                            // 目录域：`TOC \h` / `TOC \o "1-3"`。只是**登记**，
                            // 等 separate 才真正开始丢。
                            if st
                                .instr_buf
                                .trim_start()
                                .to_ascii_uppercase()
                                .starts_with("TOC")
                            {
                                st.toc_pending = true;
                            }
                            st.instr_buf.clear();
                        }
                    }
                    "tbl" | "drawing" | "pict" | "object" | "del" | "deltext" => {
                        st.skip = st.skip.saturating_sub(1);
                    }
                    "p" if st.skip == 0 => st.flush(),
                    _ => {}
                }
            }

            Event::Text(t) => {
                if st.instr > 0 {
                    if let Ok(s) = t.unescape() {
                        if st.instr_buf.len() < 256 {
                            st.instr_buf.push_str(&s);
                        }
                    }
                } else if st.skip == 0 && !st.in_toc {
                    match t.unescape() {
                        Ok(s) => st.para.push_str(&s),
                        // 实体坏了也不丢字：原样塞进去。
                        Err(_) => st.para.push_str(&String::from_utf8_lossy(t.as_ref())),
                    }
                }
            }

            // `<![CDATA[…]]>`：Word 基本不用，但真遇到要留字。
            Event::CData(c) => {
                if st.skip == 0 && st.instr == 0 && !st.in_toc {
                    st.para
                        .push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            _ => {}
        }

        buf.clear();
        since_tick += 1;
        if since_tick >= 64 {
            since_tick = 0;
            on_tick(st.stats.paragraphs, st.stats.headings);
        }
    }

    st.flush();
    on_tick(st.stats.paragraphs, st.stats.headings);
    Ok(())
}

/// 只取标签名（去掉 `w:` 前缀），小写化。
fn local(q: &[u8]) -> String {
    let s = String::from_utf8_lossy(q);
    s.rsplit(':').next().unwrap_or("").to_ascii_lowercase()
}

/// 取属性值（大小写不敏感——OOXML 里 `w:val` 也可能写成 `w:Val`）。
fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    let want = String::from_utf8_lossy(key).to_ascii_lowercase();
    for a in e.attributes().flatten() {
        let k = String::from_utf8_lossy(a.key.as_ref()).to_ascii_lowercase();
        if k == want || k.rsplit(':').next() == Some(want.as_str()) {
            return Some(String::from_utf8_lossy(&a.value).into_owned());
        }
    }
    None
}

/// 样式名 → 段落级别。
///
/// **只认「Heading 相关」，其余一律正文**（用户口径：Normal → 正文，无样式
/// 默认正文）。故意不按 `outlineLvl` 判整份文档——Calibre 导出的 docx 会给
/// `Normal` 也写上 `outlineLvl=1`，照它判就是「全书都是标题」。
fn style_kind(style: &str) -> ParaKind {
    let s = style.trim().to_ascii_lowercase();
    if s.is_empty() {
        return ParaKind::Body;
    }
    let is_head = s.contains("heading")
        || s.contains("标题")
        || s == "title"
        || s == "章标题"
        || s == "篇标题"
        || (s.len() == 2
            && s.starts_with('h')
            && s.as_bytes()[1].is_ascii_digit()
            && s.as_bytes()[1] != b'0');
    if is_head {
        ParaKind::Heading
    } else {
        ParaKind::Body
    }
}

/// 把 `{…}` / `｛…｝` 预归一成 `〔…〕`，交给 TXT 漏斗的夹注逻辑。
///
/// 只在**同一段内配对、且内容够短**（≤16 字、不含句读 / 括号）时转换：
/// 古籍扫描稿里的 `{竹奧}` `{宀介}` 是「缺字拆部件」的校勘注，转成双行夹注
/// 正合适；但正文里的花括号若不成对（代码、公式、残缺标记），照转就会把
/// 后文几千字全吞进一条注里——所以宁可不转。
fn curly_to_note(s: &str) -> String {
    if !s.contains(['{', '｛']) {
        return s.to_string();
    }
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        if c == '{' || c == '｛' {
            let close = if c == '{' { '}' } else { '｝' };
            // 往后找配对的闭括号，限制在 16 字内。
            let mut j = i + 1;
            let mut ok = false;
            while j < cs.len() && j - i <= 17 {
                if cs[j] == close {
                    ok = true;
                    break;
                }
                // 中间又见开括号 / 句读 / 已有方注括号 → 不认这一对。
                if cs[j] == '{' || cs[j] == '｛' || is_sentence_punct(cs[j]) {
                    break;
                }
                j += 1;
            }
            if ok && j > i + 1 {
                out.push('〔');
                for ch in &cs[i + 1..j] {
                    out.push(*ch);
                }
                out.push('〕');
                i = j + 1;
                continue;
            }
            // 不成对：原样留（宁可少一条注，不可吞掉正文）。
            out.push(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn is_sentence_punct(c: char) -> bool {
    matches!(
        c,
        '。' | '，' | '、' | '；' | '！' | '？' | '：' | '…' | '—' | '〔' | '〕' | '【' | '】'
    )
}

/// 目录页的标题行：`Table of Contents` / `Contents` / `目录` / `目錄` / `目次`。
///
/// 只认**整段就等于**这几个词的行——正文里出现「目录」两个字的句子不受影响。
fn is_toc_label(t: &str) -> bool {
    let s = t.trim().trim_end_matches(['：', ':']).trim().to_ascii_lowercase();
    matches!(
        s.as_str(),
        "table of contents" | "contents" | "目录" | "目錄" | "目次" | "總目" | "总目"
    )
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 极简 OOXML 骨架：只为验证「段落 → 纯文本」这条主干。
    fn doc(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
        )
    }

    fn p(text: &str) -> String {
        format!(r#"<w:p><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>"#)
    }

    fn p_styled(style: &str, text: &str) -> String {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
        )
    }

    #[test]
    fn extracts_paragraphs_in_order() {
        let xml = doc(&format!("{}{}{}", p("第一段。"), p("第二段。"), p("第三段。")));
        let (text, st) = extract_docx_text(&xml);
        assert_eq!(text.trim(), "第一段。\n\n第二段。\n\n第三段。");
        assert_eq!(st.paragraphs, 3);
        assert_eq!(st.headings, 0);
    }

    #[test]
    fn style_maps_heading_and_body() {
        let xml = doc(&format!(
            "{}{}{}",
            p_styled("Heading1", "卷一"),
            p_styled("Normal", "正文一段。"),
            p("无样式段。")
        ));
        let (text, st) = extract_docx_text(&xml);
        assert_eq!(st.headings, 1, "只有 Heading1 该算标题：{text}");
        assert_eq!(st.paragraphs, 3);
        assert!(text.contains("卷一"));
    }

    #[test]
    fn outline_lvl_counts_as_heading() {
        let xml = doc(r#"<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>卷首</w:t></w:r></w:p>"#);
        let (_, st) = extract_docx_text(&xml);
        assert_eq!(st.headings, 1);
    }

    #[test]
    fn tables_and_drawings_are_skipped() {
        let xml = doc(&format!(
            "{}{}{}",
            p("表前。"),
            r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>表格里的字</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
            p("表后。")
        ));
        let (text, _) = extract_docx_text(&xml);
        assert!(!text.contains("表格里的字"), "表格内容漏进正文：{text}");
        assert!(text.contains("表前") && text.contains("表后"));

        let xml2 = doc(&format!(
            "{}{}{}",
            p("图前。"),
            // 文本框里的字在 `<w:drawing>` **内部**（真 Word 导出的形态），必须整块跳过。
            r#"<w:p><w:r><w:drawing><wp:inline><a:graphic><a:graphicData><w:txbxContent><w:p><w:r><w:t>图形内的字</w:t></w:r></w:p></w:txbxContent></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
            p("图后。")
        ));
        let (t2, _) = extract_docx_text(&xml2);
        assert!(!t2.contains("图形内的字"), "图形内容漏进正文：{t2}");
    }

    #[test]
    fn field_codes_and_deleted_text_do_not_leak() {
        let xml = doc(&format!(
            "{}{}",
            r#"<w:p><w:r><w:instrText> PAGE \* MERGEFORMAT </w:instrText></w:r><w:r><w:t>正文。</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:del><w:r><w:delText>删掉的字</w:delText></w:r></w:del><w:r><w:t>留下的字</w:t></w:r></w:p>"#
        ));
        let (text, _) = extract_docx_text(&xml);
        assert!(!text.contains("PAGE"), "域代码漏进正文：{text}");
        assert!(!text.contains("删掉的字"), "修订删除的字漏进正文：{text}");
        assert!(text.contains("留下的字"));
    }

    #[test]
    fn toc_field_is_dropped_but_body_headings_kept() {
        let xml = doc(&format!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> TOC \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>目录里的卷第一</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>{}{}"#,
            p_styled("Heading1", "卷第一"),
            p("正文。")
        ));
        let (text, _) = extract_docx_text(&xml);
        assert!(!text.contains("目录里的卷第一"), "目录域没被跳过：{text}");
        assert!(text.contains("卷第一") && text.contains("正文。"));
    }

    #[test]
    fn curly_braces_become_notes_only_when_paired_and_short() {
        assert_eq!(curly_to_note("烽，如覆米{竹奧}。"), "烽，如覆米〔竹奧〕。");
        // 不成对：原样保留，绝不吞掉后文。
        assert_eq!(curly_to_note("代码 { 未闭合 后面还有几千字"), "代码 { 未闭合 后面还有几千字");
        // 中间有句读：不认。
        assert_eq!(curly_to_note("{一，二}"), "{一，二}");
    }

    #[test]
    fn truncated_xml_keeps_what_was_read() {
        // 故意砍掉闭合标签：不 panic、不报错，前面读到的字留着。
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>已经读到的字</w:t></w:r></w:p><w:p><w:r><w:t>半截"#;
        let (text, _) = extract_docx_text(xml);
        assert!(
            text.contains("已经读到的字") || text.contains("半截"),
            "截断 XML 应当保住已读内容：{text:?}"
        );
    }

    #[test]
    fn empty_document_is_not_a_panic() {
        let (text, st) = extract_docx_text(&doc(""));
        assert!(text.trim().is_empty());
        assert_eq!(st.paragraphs, 0);
    }

    /// 端到端：真造一个最小 docx（zip 里塞 document.xml），走
    /// `import_docx_into`，断言章被切出来、进度被汇报、夹注归一到位。
    #[test]
    fn end_to_end_import_splits_chapters_and_reports_progress() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("jiandu-docx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("测试.docx");

        let body = format!(
            "{}{}{}{}",
            p_styled("Heading1", "卷一 周紀一"),
            p("起著雍攝提格，盡玄黓困敦。〔胡三省注：凡三十二年。〕"),
            p_styled("Heading1", "卷二 周紀二"),
            p("起昭陽赤奮若，盡上章困敦。"),
        );
        let xml = doc(&body);

        {
            let f = std::fs::File::create(&path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            z.start_file("word/document.xml", opt).unwrap();
            z.write_all(xml.as_bytes()).unwrap();
            z.finish().unwrap();
        }

        struct Rec {
            ticks: std::cell::RefCell<Vec<(f32, String)>>,
            chapters: Vec<String>,
            sizes: Vec<usize>,
            sample: Vec<String>,
        }
        impl ChapterSink for Rec {
            fn push(&mut self, ch: crate::types::Chapter) {
                self.sizes.push(
                    ch.paragraphs
                        .iter()
                        .map(|p| p.text.chars().count())
                        .sum(),
                );
                self.sample
                    .push(ch.paragraphs.first().map(|p| p.text.clone()).unwrap_or_default());
                self.chapters.push(ch.title);
            }
            fn len(&self) -> usize {
                self.chapters.len()
            }
            fn sizes(&self) -> &[usize] {
                &self.sizes
            }
            fn titles(&self) -> Vec<String> {
                self.chapters.clone()
            }
            fn sample(&self, i: usize) -> &str {
                self.sample.get(i).map(|s| s.as_str()).unwrap_or("")
            }
            fn tick(&self, pct: f32, label: &str) {
                self.ticks.borrow_mut().push((pct, label.to_string()));
            }
            fn advance(&mut self, _done: usize, _label: &str) {}
            fn drop_front(&mut self, n: usize) {
                let n = n.min(self.chapters.len());
                self.chapters.drain(..n);
                self.sizes.drain(..n);
                self.sample.drain(..n);
            }
            fn drop_at(&mut self, idxs: &[usize]) {
                let mut v: Vec<usize> = idxs.to_vec();
                v.sort_unstable();
                for i in v.into_iter().rev() {
                    if i < self.chapters.len() {
                        self.chapters.remove(i);
                        self.sizes.remove(i);
                        self.sample.remove(i);
                    }
                }
            }
            fn set_title(&mut self, idx: usize, title: &str) {
                if let Some(t) = self.chapters.get_mut(idx) {
                    *t = title.to_string();
                }
            }
        }

        let mut rec = Rec {
            ticks: std::cell::RefCell::new(Vec::new()),
            chapters: Vec::new(),
            sizes: Vec::new(),
            sample: Vec::new(),
        };
        let book = import_docx_into(&path, &mut rec).expect("docx 导入不该失败");

        assert!(!book.title.is_empty());
        assert!(
            rec.chapters.len() >= 2,
            "应至少切出 2 章，实际 {:?}",
            rec.chapters
        );
        assert!(
            rec.chapters.iter().any(|t| t.contains("周紀一")),
            "章名没认出来：{:?}",
            rec.chapters
        );
        // 〔…〕 已被 TXT 漏斗归一成 [注]…[/注]（正文里不再有全角方括号）。
        let all: String = rec.sample.join("|");
        assert!(!all.contains('〔'), "夹注没被归一：{all}");
        // 进度：必须有心跳，且最终到 1.0（否则看门狗会把大文件误杀）。
        let ticks = rec.ticks.borrow();
        assert!(ticks.len() >= 3, "进度回调太少：{:?}", ticks.len());
        assert!(
            ticks.last().map(|t| t.0).unwrap_or(0.0) >= 0.99,
            "进度没走到 100%：{:?}",
            ticks.last()
        );
        std::fs::remove_file(&path).ok();
    }

    // -- 目录标签 / 书名元数据 ------------------------------------------------

    /// 域外的目录标题行（「Table of Contents」/「目录」）整段丢弃；
    /// 正文里带「目录」二字的句子不受影响。
    #[test]
    fn toc_label_lines_are_dropped() {
        let xml = doc(&format!(
            "{}{}{}{}",
            p("Table of Contents"),
            p("目錄"),
            p_styled("Heading1", "卷一 周紀一"),
            p("此書目錄甚詳，今不具錄。")
        ));
        let (text, _) = extract_docx_text(&xml);
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines,
            vec!["卷一 周紀一", "此書目錄甚詳，今不具錄。"],
            "目录标签没被挡掉：{lines:?}"
        );
    }

    #[test]
    fn strip_preface_noise_keeps_real_titles() {
        // 序跋 / 注疏题名：切到真书名。
        assert_eq!(
            strip_preface_noise("胡三省新註資治通鑑序"),
            "資治通鑑",
            "序跋题名没剥干净"
        );
        assert_eq!(strip_preface_noise("資治通鑑序"), "資治通鑑");
        // 「集注 / 注疏 / 音注」是**版本后缀**：切掉它，书名留前半。
        assert_eq!(strip_preface_noise("論語集注"), "論語");
        assert_eq!(strip_preface_noise("十三經注疏"), "十三經");
        assert_eq!(strip_preface_noise("資治通鑑音注"), "資治通鑑");
        // 带「序 / 注 / 疏」的正常书名，一个字都不许少。
        assert_eq!(strip_preface_noise("秩序"), "秩序");
        assert_eq!(strip_preface_noise("程序"), "程序");
        assert_eq!(strip_preface_noise("水經注"), "水經注");
        assert_eq!(strip_preface_noise("論語疏證"), "論語疏證");
        assert_eq!(strip_preface_noise("資治通鑑"), "資治通鑑");
    }

    #[test]
    fn core_props_title_and_creator_are_parsed() {
        let core = r#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>資治通鑑</dc:title><dc:creator>[宋] 司馬光 編著 / [元] 胡三省 音注</dc:creator><dc:language>zh</dc:language></cp:coreProperties>"#;
        let (t, c) = parse_core_props(core);
        assert_eq!(t.as_deref(), Some("資治通鑑"));
        assert_eq!(c.as_deref(), Some("[宋] 司馬光 編著 / [元] 胡三省 音注"));
        // 实体要解开；空字段 → None；截断（无闭合标签）→ None。
        let (t2, c2) = parse_core_props("<dc:title>A&amp;B</dc:title><dc:creator>  </dc:creator>");
        assert_eq!(t2.as_deref(), Some("A&B"));
        assert_eq!(c2, None);
        assert_eq!(parse_core_props("<dc:title>截断").0, None);
        assert_eq!(parse_core_props("").0, None);
    }

    #[test]
    fn creator_is_cleaned_into_a_shelf_author() {
        // 人 + 角色 + 朝代括注 → 只留人名，多人用「、」连。
        assert_eq!(
            tidy_docx_author("[宋] 司馬光 編著 / [元] 胡三省 音注").as_deref(),
            Some("司馬光、胡三省")
        );
        assert_eq!(tidy_docx_author("曹雪芹 著").as_deref(), Some("曹雪芹"));
        assert_eq!(tidy_docx_author("（清）曹雪芹，高鶚 續").as_deref(), Some("曹雪芹、高鶚"));
        // 只有角色词 / 空 → None（不能把「編著」当作者）。
        assert_eq!(tidy_docx_author("編著"), None);
        assert_eq!(tidy_docx_author("   "), None);
    }

    /// 造一个最小 docx（可选塞 `docProps/core.xml`）。
    fn write_docx(path: &std::path::Path, body: &str, core: Option<&str>) {
        use std::io::Write;
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        z.start_file("word/document.xml", opt).unwrap();
        z.write_all(doc(body).as_bytes()).unwrap();
        if let Some(c) = core {
            z.start_file("docProps/core.xml", opt).unwrap();
            z.write_all(c.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    /// 只关心书名时用的空 sink。
    struct NullSink;
    impl ChapterSink for NullSink {
        fn push(&mut self, _ch: crate::types::Chapter) {}
        fn len(&self) -> usize {
            0
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
        fn tick(&self, _pct: f32, _label: &str) {}
        fn advance(&mut self, _done: usize, _label: &str) {}
        fn drop_front(&mut self, _n: usize) {}
        fn drop_at(&mut self, _idxs: &[usize]) {}
        fn set_title(&mut self, _idx: usize, _title: &str) {}
    }

    #[test]
    fn book_title_comes_from_core_xml_not_first_line() {
        let dir = std::env::temp_dir().join(format!("jiandu-docx-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("胡三省新註資治通鑑序.docx");
        let core = r#"<cp:coreProperties xmlns:cp="x" xmlns:dc="y"><dc:title>資治通鑑</dc:title><dc:creator>[宋] 司馬光 編著</dc:creator></cp:coreProperties>"#;
        write_docx(
            &path,
            &format!("{}{}", p("胡三省新註資治通鑑序"), p("古者國各有史以紀年書事。")),
            Some(core),
        );
        let book = import_docx_into(&path, &mut NullSink).expect("导入不该失败");
        assert_eq!(book.title, "資治通鑑", "没优先用 dc:title：{}", book.title);
        assert_eq!(book.cover.title, "資治通鑑");
        assert_eq!(book.author, "司馬光", "作者没从 dc:creator 取：{}", book.author);
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn book_title_falls_back_to_filename_and_gets_cleaned() {
        let dir = std::env::temp_dir().join(format!("jiandu-docx-stem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("胡三省新註資治通鑑序.docx");
        // 没有 core.xml → 退文件名，且文件名同样要被剥序跋。
        write_docx(&path, &p("古者國各有史以紀年書事。"), None);
        let book = import_docx_into(&path, &mut NullSink).expect("导入不该失败");
        assert_eq!(book.title, "資治通鑑", "文件名没被清洗：{}", book.title);
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir_all(&dir).ok();
    }
}
