//! **统一书稿模型（IR，Intermediate Representation）**——v1.54.0 立。
//!
//! 架构位置：
//! ```text
//! EPUB ─┐
//! TXT ──┤                          ┌→ [排版引擎]
//! MD ───┼→ 解析器 → **清洗器** → IR ┤
//! HTML ─┤                          └→ [导出层 MD / PDF]
//! DOCX ─┘
//! ```
//!
//! 各格式进来先「断头」——剥掉自己的壳（zip / 标签 / YAML 头 / 样式），
//! 清洗成**这一份** IR，再喂给排版引擎。于是：
//!   * 引擎永远只面对一种数据，**新增一种格式只需写一个解析器，引擎零改动**；
//!   * IR 与导出的 Markdown 规范**同构**（`to_markdown` / `from_markdown`），
//!     导入与导出是同一份数据的两个方向的序列化——一份模型，两个入口。
//!
//! 与 `types::Book` 的分工：`Book` 是**排版态**（带 `Note` 下标、`toc`、眉批坐标、
//! 仓库 id），IR 是**内容态**（只有「哪些字、哪些注、怎么分级」）。两者互转：
//! [`IrBook::from_book`] / [`IrBook::write_chapters_into`]。
//!
//! ⚠️ 铁律：本模块**不得**引入 GPUI / 平台类型（与 core 其余部分同）。

use crate::pipeline::RawChapter;
use crate::types::{Book, Chapter, Note, NoteKind, Paragraph};

// ------------------------------------------------------------------
// 一、源头格式
// ------------------------------------------------------------------

/// 这书是从什么「壳」里剥出来的。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IrSource {
    /// 纯文本：没有结构，最难——靠启发式建卷章。
    #[default]
    Txt,
    /// Markdown：壳最薄（`#` 标题 + 行内标记）。
    Markdown,
    Html,
    Epub,
    /// Office Open XML（zip 包 + `word/document.xml`）。
    Docx,
    /// 笺读书稿 JSON。
    Json,
    /// TEI / DocBook 等古籍 XML。
    Xml,
}

impl IrSource {
    pub fn label(self) -> &'static str {
        match self {
            IrSource::Txt => "txt",
            IrSource::Markdown => "md",
            IrSource::Html => "html",
            IrSource::Epub => "epub",
            IrSource::Docx => "docx",
            IrSource::Json => "json",
            IrSource::Xml => "xml",
        }
    }

    /// 按扩展名（小写、不含点）判断来源；认不出就当纯文本（最兜底）。
    pub fn from_ext(ext: &str) -> Self {
        match ext {
            "md" | "markdown" | "mdown" => IrSource::Markdown,
            "html" | "htm" | "xhtml" | "xht" => IrSource::Html,
            "epub" => IrSource::Epub,
            "docx" => IrSource::Docx,
            "json" => IrSource::Json,
            "xml" | "tei" => IrSource::Xml,
            _ => IrSource::Txt,
        }
    }

    /// 这种格式**自带结构**（标题层级 / 目录），清洗器不必再启发式重建。
    ///
    /// TXT 与 DOCX 要重建：txt 只有空行、docx 虽有 `Heading` 样式但多数
    /// 网文 / 扫描转档的 docx 全是 `Normal` 正文段落。
    pub fn has_structure(self) -> bool {
        matches!(
            self,
            IrSource::Markdown | IrSource::Html | IrSource::Epub | IrSource::Json | IrSource::Xml
        )
    }
}

// ------------------------------------------------------------------
// 二、Span：段内的最小单元
// ------------------------------------------------------------------

/// 注记种类（IR 层只留「排版会画出东西」的那几种）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrNoteKind {
    /// 双行夹注（朱红编号）。
    Note,
    /// 通假标（靛蓝小字）。
    Tongjia,
    /// 着重（朱砂圈点）——对应 `[高亮]…[/高亮]`。
    Emph,
    /// 人名线。
    Person,
    /// 地名线。
    Place,
    /// 书名线（波浪线）。
    Book,
}

impl IrNoteKind {
    pub fn to_note_kind(self) -> NoteKind {
        match self {
            IrNoteKind::Note => NoteKind::Note,
            IrNoteKind::Tongjia => NoteKind::Tongjia,
            IrNoteKind::Emph => NoteKind::Emph,
            IrNoteKind::Person => NoteKind::Person,
            IrNoteKind::Place => NoteKind::Place,
            IrNoteKind::Book => NoteKind::Book,
        }
    }

    /// 不是 IR 关心的（注音走 [`IrSpan::Ruby`]）就返回 `None`。
    pub fn from_note_kind(k: NoteKind) -> Option<Self> {
        match k {
            NoteKind::Note => Some(IrNoteKind::Note),
            NoteKind::Tongjia => Some(IrNoteKind::Tongjia),
            NoteKind::Emph => Some(IrNoteKind::Emph),
            NoteKind::Person => Some(IrNoteKind::Person),
            NoteKind::Place => Some(IrNoteKind::Place),
            NoteKind::Book => Some(IrNoteKind::Book),
            NoteKind::Ruby => None,
        }
    }

    /// 跨字标记（着重 / 专名线）：占格，按 `len` 覆盖一串字。
    pub fn is_span_mark(self) -> bool {
        matches!(
            self,
            IrNoteKind::Emph | IrNoteKind::Person | IrNoteKind::Place | IrNoteKind::Book
        )
    }
}

/// 一段正文里的一个片段。
///
/// 与导出的 Markdown 标记一一对应：
///   * `Text`   —— 纯字
///   * `Ruby`   —— `字[拼音:hàn]`
///   * `Note`   —— `[注]…[/注]` / `[高亮]…[/高亮]`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrSpan {
    /// 正字（可含多个字）。
    Text(String),
    /// 注音：`base` 是那个字，`rt` 是拼音 / 反切。
    Ruby { base: String, rt: String },
    /// 注记：夹注 / 通假 / 着重 / 专名线。
    Note { kind: IrNoteKind, text: String },
}

impl IrSpan {
    /// 落进字格的字数（`Note::Note` 恒 0——夹注不占格，挂在宿主字旁）。
    pub fn glyph_len(&self) -> usize {
        match self {
            IrSpan::Text(t) => t.chars().count(),
            IrSpan::Ruby { base, .. } => base.chars().count(),
            IrSpan::Note { kind, text } => {
                if kind.is_span_mark() {
                    text.chars().count()
                } else {
                    0
                }
            }
        }
    }

    /// 纯文本（去标记，供清洗器判据 / 搜索索引用）。
    pub fn plain(&self) -> String {
        match self {
            IrSpan::Text(t) => t.clone(),
            IrSpan::Ruby { base, .. } => base.clone(),
            IrSpan::Note { kind, text } => {
                if kind.is_span_mark() {
                    text.clone()
                } else {
                    String::new()
                }
            }
        }
    }

    /// 序列化成导入管线的行内标记串（Markdown 导出的同一套记号）。
    pub fn to_inline(&self) -> String {
        match self {
            IrSpan::Text(t) => t.clone(),
            IrSpan::Ruby { base, rt } => format!("{base}[拼音:{rt}]"),
            IrSpan::Note { kind, text } => match kind {
                IrNoteKind::Note => format!("[注]{text}[/注]"),
                IrNoteKind::Tongjia => format!("[注]{text}[/注]"),
                IrNoteKind::Emph => format!("[高亮]{text}[/高亮]"),
                // 专名线在导入管线里没有对应记号（只存在于笺读书稿 JSON），
                // 导出 MD 时降级为纯字——线是排版态的装饰，丢了不伤正文。
                _ => text.clone(),
            },
        }
    }
}

// ------------------------------------------------------------------
// 三、Block / Juan / Book
// ------------------------------------------------------------------

/// 一个语义块。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrBlock {
    /// 正文一段。
    Para(Vec<IrSpan>),
    /// 眉批 / 题解：章首那段独立小字，排在天头。
    HeadNote(String),
    /// 独立成块的注文（夹注 / 注疏）。
    Annotation { kind: IrNoteKind, spans: Vec<IrSpan> },
    /// 章内小节标题（`level` 从 2 起：1 是卷，2 是章内小节）。
    SubHead { level: u8, text: String },
}

impl IrBlock {
    pub fn para(text: impl Into<String>) -> Self {
        IrBlock::Para(vec![IrSpan::Text(text.into())])
    }

    /// 纯文本（去标记）。
    pub fn plain_text(&self) -> String {
        match self {
            IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
                let mut s = String::new();
                for sp in spans {
                    s.push_str(&sp.plain());
                }
                s
            }
            IrBlock::HeadNote(t) => t.clone(),
            IrBlock::SubHead { text, .. } => text.clone(),
        }
    }

    /// 序列化成一行（带行内标记）。
    pub fn to_line(&self) -> String {
        match self {
            IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
                let mut s = String::new();
                for sp in spans {
                    s.push_str(&sp.to_inline());
                }
                s
            }
            IrBlock::HeadNote(t) => t.clone(),
            IrBlock::SubHead { text, .. } => text.clone(),
        }
    }

    /// 正文块 → 排版态段落（`HeadNote` / `SubHead` 不是正文，返回 `None`）。
    pub fn to_paragraph(&self) -> Option<Paragraph> {
        match self {
            IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => Some(spans_to_para(spans)),
            _ => None,
        }
    }

    /// 落进字格的字数。
    pub fn glyph_len(&self) -> usize {
        match self {
            IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
                spans.iter().map(|s| s.glyph_len()).sum()
            }
            IrBlock::HeadNote(t) => t.chars().count(),
            IrBlock::SubHead { text, .. } => text.chars().count(),
        }
    }
}

/// 一巻 / 一章。
#[derive(Clone, Debug, Default)]
pub struct IrJuan {
    pub title: String,
    /// 层级：1 = 卷（目录一级），2 = 章内小节。
    pub level: u8,
    pub blocks: Vec<IrBlock>,
}

impl IrJuan {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            level: 1,
            blocks: Vec::new(),
        }
    }

    pub fn push(&mut self, block: IrBlock) {
        self.blocks.push(block);
    }

    pub fn para(&mut self, text: impl Into<String>) {
        self.blocks.push(IrBlock::para(text));
    }

    /// 全巻纯文本（段间换行）。
    pub fn body_text(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for b in &self.blocks {
            let t = b.plain_text();
            if !t.trim().is_empty() {
                lines.push(t);
            }
        }
        lines.join("\n")
    }

    pub fn char_count(&self) -> usize {
        self.blocks.iter().map(|b| b.glyph_len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() || self.body_text().trim().is_empty()
    }

    /// 排版态 → IR。
    pub fn from_chapter(c: &Chapter) -> Self {
        let mut j = IrJuan::new(c.title.as_str());
        for p in &c.paragraphs {
            if p.text.trim().is_empty() {
                continue;
            }
            j.blocks.push(IrBlock::Para(para_to_spans(p)));
        }
        j
    }

    /// IR → 排版态（只取正文块；`SubHead` 降级为普通一段，眉批另走 `head_notes`）。
    pub fn to_chapter(&self) -> Chapter {
        let mut paragraphs: Vec<Paragraph> = Vec::new();
        for b in &self.blocks {
            match b {
                IrBlock::Para(spans) => paragraphs.push(spans_to_para(spans)),
                IrBlock::Annotation { spans, .. } => paragraphs.push(spans_to_para(spans)),
                // 小节标题在竖排里就是一段居中的大字，仍要排进版面——
                // 不能丢，否则目录有这一条、正文却跳过去了。
                IrBlock::SubHead { text, .. } => paragraphs.push(Paragraph::plain(text.as_str())),
                // 眉批不属于版面正文，导入态下并入正文首段之前（罕见路径）。
                IrBlock::HeadNote(t) => paragraphs.push(Paragraph::plain(t.as_str())),
            }
        }
        Chapter {
            title: crate::numcn::normalize_numbers(&self.title),
            paragraphs,
        }
    }
}

/// 书稿元数据。
#[derive(Clone, Debug, Default)]
pub struct IrMeta {
    pub title: String,
    pub author: String,
}

/// **统一书稿模型**：所有格式剥壳之后的共同终点。
#[derive(Clone, Debug, Default)]
pub struct IrBook {
    pub meta: IrMeta,
    pub source: IrSource,
    pub juans: Vec<IrJuan>,
}

impl IrBook {
    pub fn new(title: impl Into<String>, author: impl Into<String>, source: IrSource) -> Self {
        Self {
            meta: IrMeta {
                title: title.into(),
                author: author.into(),
            },
            source,
            juans: Vec::new(),
        }
    }

    /// 排版态 `Book` → IR（**保留**夹注 / 专名线 / 注音的全部信息）。
    pub fn from_book(book: &Book, source: IrSource) -> Self {
        let mut ir = IrBook::new(book.title.as_str(), book.author.as_str(), source);
        for c in &book.chapters {
            ir.juans.push(IrJuan::from_chapter(c));
        }
        ir
    }

    /// 解析器的 `RawChapter`（行内标记串）→ IR。
    pub fn from_chapters(
        title: &str,
        author: &str,
        source: IrSource,
        chapters: &[RawChapter],
    ) -> Self {
        let mut ir = IrBook::new(title, author, source);
        for c in chapters {
            let mut j = IrJuan::new(c.title.as_str());
            for line in c.content.lines() {
                let l = line.trim();
                if l.is_empty() {
                    continue;
                }
                let spans = line_to_spans(l);
                if spans.is_empty() {
                    continue;
                }
                j.blocks.push(IrBlock::Para(spans));
            }
            if !j.is_empty() || !c.title.trim().is_empty() {
                ir.juans.push(j);
            }
        }
        ir
    }

    /// IR → 解析器的 `RawChapter`（供重新走一遍既有建书链路）。
    pub fn to_raw_chapters(&self) -> Vec<RawChapter> {
        self.juans
            .iter()
            .map(|j| RawChapter::new(j.title.as_str(), j.body_lines().join("\n")))
            .collect()
    }

    /// IR 的正文（带行内标记）按段铺开。
    pub fn body_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for j in &self.juans {
            for b in &j.blocks {
                let l = b.to_line();
                if !l.trim().is_empty() {
                    out.push(l);
                }
            }
        }
        out
    }

    /// 把 IR 的卷章**写回**排版态 `Book`：只换 `chapters`，其余（封面 / 通假表 /
    /// 眉批 / 仓库 id）原样不动——清洗不该顺手把用户数据抹了。
    pub fn write_chapters_into(&self, book: &mut Book) {
        book.chapters = self.juans.iter().map(|j| j.to_chapter()).collect();
        if book.chapters.is_empty() {
            book.chapters.push(Chapter {
                title: "正文".to_string(),
                paragraphs: Vec::new(),
            });
        }
        // 章数变了，旧目录的下标就不作数了——整份按新卷章重建。
        book.toc = Vec::new();
        book.rebuild_toc_from_chapters();
    }

    /// 从零建一本排版态 `Book`（元数据 + 卷章 + 目录）。
    pub fn to_book(&self) -> Book {
        let mut book = Book::new(&self.meta.title, &self.meta.author);
        book.cover.title = self.meta.title.clone();
        self.write_chapters_into(&mut book);
        book
    }

    pub fn char_count(&self) -> usize {
        self.juans.iter().map(|j| j.char_count()).sum()
    }

    // -- Markdown 双向序列化：IR 与导出规范同构 --------------------

    /// IR → Markdown（`# 书名` / `## 卷` / 正文段，`[注]` 记号原样保留）。
    ///
    /// 与导入同构：**导出的 md 再导回来，得到的 IR 与导出前一致**。
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# ");
        out.push_str(self.meta.title.as_str());
        out.push('\n');
        if !self.meta.author.trim().is_empty() {
            out.push_str("> ");
            out.push_str(self.meta.author.as_str());
            out.push('\n');
        }
        for j in &self.juans {
            out.push('\n');
            out.push_str("## ");
            out.push_str(j.title.as_str());
            out.push('\n');
            for b in &j.blocks {
                let line = b.to_line();
                if line.trim().is_empty() {
                    continue;
                }
                match b {
                    IrBlock::HeadNote(_) => {
                        out.push_str("> ");
                        out.push_str(&line);
                    }
                    IrBlock::SubHead { level, .. } => {
                        for _ in 0..(*level).min(5) {
                            out.push('#');
                        }
                        out.push(' ');
                        out.push_str(&line);
                    }
                    _ => out.push_str(&line),
                }
                out.push('\n');
            }
        }
        out
    }

    /// Markdown → IR。
    ///
    /// **不走** `pipeline::parse_markdown`：那条链是「给人读的导入」，会顺手
    /// 把 md 语法（含 `[…](…)`，与笺读的 `[注]…[/注]` 长得像）剥掉；
    /// 这里是 IR 自己的序列化方向，**必须保真**——导出的 md 再读回来要跟
    /// 导出前一模一样。故自己扫一遍 md 骨架（标题 / 引用 / 空行）。
    pub fn from_markdown(title: &str, md: &str) -> Self {
        let mut ir = IrBook::new(title, "", IrSource::Markdown);
        let mut cur: Option<IrJuan> = None;
        let mut in_yaml = false;
        let mut saw_author = false;

        let flush = |ir: &mut IrBook, cur: &mut Option<IrJuan>| {
            if let Some(j) = cur.take() {
                if !j.is_empty() || !j.title.trim().is_empty() {
                    ir.juans.push(j);
                }
            }
        };

        for raw in md.lines() {
            let line = raw.trim();
            // YAML front matter（`---` 成对包裹）整段跳过。
            if line == "---" {
                in_yaml = !in_yaml;
                continue;
            }
            if in_yaml || line.is_empty() {
                continue;
            }
            let hashes = line.chars().take_while(|c| *c == '#').count();
            if hashes > 0 && line.len() > hashes {
                let text = md_inline_plain(line[hashes..].trim());
                if text.is_empty() {
                    continue;
                }
                match hashes {
                    // 一级：书名（还没开卷时），否则降级为新卷——
                    // 「一本 md 里有多个 # 一级标题」是分卷的常见写法。
                    1 => {
                        if ir.juans.is_empty() && cur.is_none() {
                            ir.meta.title = text;
                        } else {
                            flush(&mut ir, &mut cur);
                            cur = Some(IrJuan::new(text));
                        }
                    }
                    2 => {
                        flush(&mut ir, &mut cur);
                        cur = Some(IrJuan::new(text));
                    }
                    _ => {
                        let j = cur.get_or_insert_with(|| IrJuan::new("正文"));
                        j.blocks.push(IrBlock::SubHead {
                            level: hashes.min(6) as u8,
                            text,
                        });
                    }
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix('>') {
                let text = md_inline_plain(rest.trim());
                if text.is_empty() {
                    continue;
                }
                // 卷首第一条引用是作者行（与 `to_markdown` 的写法对应）。
                if ir.juans.is_empty() && cur.is_none() && !saw_author {
                    saw_author = true;
                    ir.meta.author = text;
                    continue;
                }
                let j = cur.get_or_insert_with(|| IrJuan::new("正文"));
                j.blocks.push(IrBlock::HeadNote(text));
                continue;
            }
            let spans = line_to_spans(&md_inline_plain(line));
            if spans.is_empty() {
                continue;
            }
            let j = cur.get_or_insert_with(|| IrJuan::new("正文"));
            j.blocks.push(IrBlock::Para(spans));
        }
        flush(&mut ir, &mut cur);
        if ir.juans.is_empty() {
            ir.juans.push(IrJuan::new("正文"));
        }
        ir
    }
}

impl IrJuan {
    /// 正文各段（带行内标记）。
    pub fn body_lines(&self) -> Vec<String> {
        self.blocks
            .iter()
            .map(|b| b.to_line())
            .filter(|l| !l.trim().is_empty())
            .collect()
    }
}

// ------------------------------------------------------------------
// 四、Span ⇄ Paragraph 互转（核心：下标同源，绝不串位）
// ------------------------------------------------------------------

/// 排版态段落 → Span 序列。
///
/// **下标口径**：`Note::at` 是段内字序；夹注挂「宿主字之后」，注音挂「本字上」，
/// 着重 / 专名线是「从 `at` 起跨 `len` 个字」的区间。这里一趟扫完四类，
/// 谁也不会把别人的下标算错。
pub fn para_to_spans(p: &Paragraph) -> Vec<IrSpan> {
    let cs: Vec<char> = p.text.chars().collect();
    let mut out: Vec<IrSpan> = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;
    while i < cs.len() {
        // ① 跨字标记（着重 / 专名线）：整段换成一枚 span。
        let mark = p
            .notes
            .iter()
            .find(|n| n.at == i && n.len > 0 && IrNoteKind::from_note_kind(n.kind).is_some())
            .and_then(|n| {
                IrNoteKind::from_note_kind(n.kind)
                    .filter(|k| k.is_span_mark())
                    .map(|k| (k, n.len))
            });
        if let Some((kind, len)) = mark {
            if !buf.is_empty() {
                out.push(IrSpan::Text(std::mem::take(&mut buf)));
            }
            let end = (i + len).min(cs.len());
            out.push(IrSpan::Note {
                kind,
                text: cs[i..end].iter().collect(),
            });
            i = end;
            continue;
        }
        // ② 注音：本字单独成一枚 Ruby span。
        if let Some(rt) = p
            .notes
            .iter()
            .find(|n| n.kind == NoteKind::Ruby && n.at == i)
            .map(|n| n.text.trim().to_string())
        {
            if !buf.is_empty() {
                out.push(IrSpan::Text(std::mem::take(&mut buf)));
            }
            out.push(IrSpan::Ruby {
                base: cs[i].to_string(),
                rt,
            });
        } else {
            buf.push(cs[i]);
        }
        // ③ 夹注 / 通假：跟在宿主字之后，不占格。
        for n in p.notes.iter().filter(|n| n.at == i) {
            match IrNoteKind::from_note_kind(n.kind) {
                Some(IrNoteKind::Note) | Some(IrNoteKind::Tongjia) => {
                    if !buf.is_empty() {
                        out.push(IrSpan::Text(std::mem::take(&mut buf)));
                    }
                    out.push(IrSpan::Note {
                        kind: IrNoteKind::from_note_kind(n.kind).unwrap_or(IrNoteKind::Note),
                        text: n.text.clone(),
                    });
                }
                _ => {}
            }
        }
        i += 1;
    }
    if !buf.is_empty() {
        out.push(IrSpan::Text(buf));
    }
    out
}

/// Span 序列 → 排版态段落（`at` 由累加算出，与 [`para_to_spans`] 严格互逆）。
pub fn spans_to_para(spans: &[IrSpan]) -> Paragraph {
    let mut text = String::new();
    let mut notes: Vec<Note> = Vec::new();
    for sp in spans {
        match sp {
            IrSpan::Text(t) => text.push_str(t),
            IrSpan::Ruby { base, rt } => {
                let at = text.chars().count();
                text.push_str(base);
                let mut n = Note::new(at, NoteKind::Ruby, rt.trim());
                n.len = base.chars().count().max(1);
                notes.push(n);
            }
            IrSpan::Note { kind, text: body } => {
                let k = kind.to_note_kind();
                if kind.is_span_mark() {
                    let at = text.chars().count();
                    let len = body.chars().count();
                    if len == 0 {
                        continue;
                    }
                    text.push_str(body);
                    let mut n = Note::new(at, k, body.as_str());
                    n.len = len;
                    notes.push(n);
                } else {
                    // 夹注：宿主 = 前一个已落格的字；段首无字则锚在首字。
                    let at = text.chars().count().saturating_sub(1);
                    notes.push(Note::new(at, k, body.as_str()));
                }
            }
        }
    }
    Paragraph::with_notes(text, notes)
}

/// 行内标记串 → Span 序列（导入管线：`[注]` / `[高亮]` / `[拼音:]` / `[数字:]`）。
///
/// 一趟扫完四类标记，**下标以纯正文为准**——不像分步解析那样把标签字数
/// 算进正文，故注音与夹注并存时也不会串位。
pub fn line_to_spans(line: &str) -> Vec<IrSpan> {
    let mut out: Vec<IrSpan> = Vec::new();
    let mut buf = String::new();
    // byte 偏移推进：一趟扫到底，绝不为每个字重切一次字符串（百万字丛书
    // 走的是这条路径，O(n²) 的切片能把导入拖成几十秒）。
    let mut pos = 0usize;
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(IrSpan::Text(std::mem::take(&mut buf)));
            }
        };
    }
    while pos < line.len() {
        let tail = &line[pos..];
        // 每个标记分支：吃掉「标记壳 + 内容 + 闭壳」，把 pos 推到壳后。
        let mut stepped = false;
        for open in ["[注]", "[高亮]", "[数字:", "[拼音:"] {
            let close = match open {
                "[注]" => "[/注]",
                "[高亮]" => "[/高亮]",
                _ => "]",
            };
            let Some(rest) = tail.strip_prefix(open) else {
                continue;
            };
            let (body, after) = match rest.find(close) {
                Some(k) => (&rest[..k], &rest[k + close.len()..]),
                None => (rest, ""),
            };
            let eaten = tail.len() - after.len();
            match open {
                "[注]" => {
                    flush!();
                    let t = body.trim();
                    if !t.is_empty() {
                        out.push(IrSpan::Note {
                            kind: IrNoteKind::Note,
                            text: t.to_string(),
                        });
                    }
                }
                "[高亮]" => {
                    flush!();
                    if !body.is_empty() {
                        out.push(IrSpan::Note {
                            kind: IrNoteKind::Emph,
                            text: body.to_string(),
                        });
                    }
                }
                "[数字:" => buf.push_str(body),
                // 注音写在宿主字之后（`汉[拼音:hàn]`），故遇到标记时 buf 的
                // 末字就是宿主——把它提出来单独成一枚 Ruby span。
                _ => {
                    let py = body.trim();
                    if !py.is_empty() {
                        let base = buf.pop().unwrap_or('　');
                        flush!();
                        out.push(IrSpan::Ruby {
                            base: base.to_string(),
                            rt: py.to_string(),
                        });
                    }
                }
            }
            pos += eaten;
            stepped = true;
            break;
        }
        if stepped {
            continue;
        }
        let Some(c) = tail.chars().next() else { break };
        buf.push(c);
        pos += c.len_utf8();
    }
    flush!();
    out
}

/// Markdown 行内标记 → 纯文本：图片整枚丢掉（竖排不排图），链接降级为文字，
/// 强调符号剥掉。
///
/// 笺读自己的 `[注]…[/注]` / `[拼音:…]` 记号**不会**被误当链接——它们后面没有
/// `](`，故原样穿过。
pub fn md_inline_plain(s: &str) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    while pos < s.len() {
        let tail = &s[pos..];
        if let Some(rest) = tail.strip_prefix("![") {
            if let Some(k) = rest.find("](") {
                let after = &rest[k..];
                let end = match after.find(')') {
                    Some(e) => after[e + 1..].to_string(),
                    None => String::new(),
                };
                pos += if end.is_empty() {
                    tail.len()
                } else {
                    tail.len() - end.len()
                };
                continue;
            }
        }
        if let Some(rest) = tail.strip_prefix('[') {
            if let Some(k) = rest.find("](") {
                let after = &rest[k..];
                if let Some(e) = after.find(')') {
                    let end = &after[e + 1..];
                    out.push_str(&rest[..k]);
                    pos += tail.len() - end.len();
                    continue;
                }
            }
        }
        let Some(c) = tail.chars().next() else { break };
        match c {
            '*' | '_' | '`' | '~' => {}
            // 转义符：吃掉反斜杠，把后一字当普通字。
            '\\' => {
                if let Some(n) = tail.chars().nth(1) {
                    out.push(n);
                    pos += n.len_utf8();
                }
            }
            _ => out.push(c),
        }
        pos += c.len_utf8();
    }
    out
}

/// 排版态段落 → 行内标记串（与 [`line_to_spans`] 互逆，供导出 / 回灌用）。
pub fn para_to_line(p: &Paragraph) -> String {
    let mut s = String::new();
    for sp in para_to_spans(p) {
        s.push_str(&sp.to_inline());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_text(spans: &[IrSpan]) -> String {
        spans.iter().map(|s| s.plain()).collect()
    }

    #[test]
    fn span_roundtrip_keeps_note_offsets() {
        let p = Paragraph::with_notes(
            "道可道非常道".to_string(),
            vec![
                Note::new(0, NoteKind::Note, "宇宙本原"),
                Note::new(3, NoteKind::Ruby, "fēi"),
            ],
        );
        let spans = para_to_spans(&p);
        assert_eq!(spans_text(&spans), "道可道非常道");
        let back = spans_to_para(&spans);
        assert_eq!(back.text, "道可道非常道");
        let kinds: Vec<(usize, NoteKind, String)> = back
            .notes
            .iter()
            .map(|n| (n.at, n.kind, n.text.clone()))
            .collect();
        assert!(kinds.contains(&(0, NoteKind::Note, "宇宙本原".to_string())));
        assert!(kinds.contains(&(3, NoteKind::Ruby, "fēi".to_string())));
    }

    #[test]
    fn line_marks_parse_in_one_pass() {
        let spans = line_to_spans("道[注]本原[/注]可道非[高亮]常[/高亮]名");
        assert_eq!(spans_text(&spans), "道可道非常名");
        let kinds: Vec<&str> = spans
            .iter()
            .map(|s| match s {
                IrSpan::Text(_) => "t",
                IrSpan::Ruby { .. } => "r",
                IrSpan::Note { kind, .. } => match kind {
                    IrNoteKind::Note => "n",
                    IrNoteKind::Emph => "e",
                    _ => "?",
                },
            })
            .collect();
        assert_eq!(kinds, vec!["t", "n", "t", "e", "t"]);
    }

    #[test]
    fn ruby_marks_ride_on_previous_char() {
        let spans = line_to_spans("漢[拼音:hàn]字");
        assert_eq!(spans_text(&spans), "漢字");
        assert!(matches!(
            spans.first(),
            Some(IrSpan::Ruby { base, rt }) if base == "漢" && rt == "hàn"
        ));
    }

    #[test]
    fn book_to_ir_and_back_is_lossless() {
        let mut book = Book::new("道德經", "老子");
        book.chapters.push(Chapter {
            title: "第一章".to_string(),
            paragraphs: vec![
                Paragraph::with_notes(
                    "道可道非常道".to_string(),
                    vec![Note::new(0, NoteKind::Note, "本原")],
                ),
                Paragraph::plain("名可名非常名"),
            ],
        });
        let ir = IrBook::from_book(&book, IrSource::Txt);
        assert_eq!(ir.juans.len(), 1);
        assert_eq!(ir.char_count(), 12);
        let back = ir.to_book();
        assert_eq!(back.chapters.len(), 1);
        assert_eq!(back.chapters[0].title, "第一章");
        assert_eq!(back.chapters[0].paragraphs[0].text, "道可道非常道");
        assert_eq!(back.chapters[0].paragraphs[0].notes.len(), 1);
    }

    #[test]
    fn markdown_roundtrip_is_stable() {
        let mut ir = IrBook::new("莊子", "莊周", IrSource::Markdown);
        let mut j = IrJuan::new("逍遙遊");
        j.para("北冥有魚[注]北海[/注]，其名為鯤。");
        j.blocks.push(IrBlock::SubHead {
            level: 3,
            text: "小大之辯".to_string(),
        });
        ir.juans.push(j);
        let md = ir.to_markdown();
        let back = IrBook::from_markdown("莊子", &md);
        assert!(!back.juans.is_empty(), "导出再导入不该丢卷：\n{md}");
        // `body_text` 是去标记的纯字（夹注不占格，故不出现在纯正文里）；
        // 校验保真要看带标记的那一份 `body_lines`。
        let plain: String = back.juans.iter().map(|j| j.body_text()).collect();
        assert!(plain.contains("北冥有魚"), "正文丢了：{plain}");
        let marked: String = back.juans.iter().flat_map(|j| j.body_lines()).collect();
        assert!(marked.contains("[注]北海[/注]"), "夹注丢了：{marked}");
        assert!(plain.contains("小大之辯"), "小节标题丢了：{plain}");
    }

    #[test]
    fn source_structure_flags() {
        assert_eq!(IrSource::from_ext("txt").label(), "txt");
        assert_eq!(IrSource::from_ext("txt"), IrSource::Txt);
        assert!(!IrSource::Txt.has_structure());
        assert!(IrSource::Epub.has_structure());
        assert!(IrSource::Markdown.has_structure());
        assert!(!IrSource::Docx.has_structure());
        assert_eq!(IrSource::from_ext("docx"), IrSource::Docx);
    }

    #[test]
    fn write_chapters_into_preserves_meta() {
        let mut book = Book::new("論語", "孔門");
        book.cover.title = "論語".to_string();
        book.chapters.push(Chapter {
            title: "學而".to_string(),
            paragraphs: vec![Paragraph::plain("學而時習之")],
        });
        let mut ir = IrBook::from_book(&book, IrSource::Txt);
        ir.juans.push(IrJuan::new("為政"));
        ir.juans[1].para("為政以德");
        ir.write_chapters_into(&mut book);
        assert_eq!(book.title, "論語");
        assert_eq!(book.chapters.len(), 2);
        // 目录跟着重建（章数变了，旧下标不作数）。
        assert!(book.toc.iter().any(|t| t.title == "為政"));
    }
}
