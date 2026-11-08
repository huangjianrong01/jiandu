//! **结构化数据导出（TEI / XML）** —— V2.0 商业化模块之一。
//!
//! 设计原则（死律）：
//!   * **完全独立**：本文件零 UI 依赖，不碰 `reader` 排版引擎、不碰任何 GPUI /
//!     平台类型；与 `ir`（统一书稿模型）处于同一纯度层级（见 `check_core_purity.sh`）。
//!   * **复用底层结构**：直接消费 `IrBook` / `IrJuan` / `IrBlock` / `IrSpan`
//!     （即 `types::Book` / `Chapter` 的「内容态」），不重新发明数据结构。
//!   * **三路分离**：遍历 IR 时，把每一片严格归类到三条互不串位的流——
//!       - 正文（body）   ：`IrSpan::Text` / `IrSpan::Ruby`
//!       - 夹注（gloss）  ：`IrSpan::Note{Note|Tongjia}` 及夹注块
//!       - 批注（annotation）：`IrSpan::Note{Emph|Person|Place|Book}` / 眉批 / 批注块
//!   * **标准 TEI（P5）**：输出带 `http://www.tei-c.org/ns/1.0` 命名空间的合法 XML，
//!     实体名物用专用标签（`<persName>` / `<placeName>` / `<title>`），方便下游
//!     古籍机构 / 图书馆直接抽取知识图谱。
//!   * **V1.0 纯文本兼容**：导出逻辑只认 `IrBook`；V1.0 纯文本书只要先 `IrJuan`
//!     化（或 `export_plain_text`）即可复用同一管线，几乎零改动。
//!
//! 用法：
//!   * `export_tei(&ir_book)`            —— 主入口，吃任意 `IrBook`。
//!   * `export_chapter(&chapter)`        —— 直接吃 `types::Chapter`。
//!   * `export_plain_text(t, a, text)`   —— V1.0 纯文本书的快捷入口。
//!   * `IrBook::from_book(&book, src)`   —— 真实书稿 → IR（UI 层导出菜单接这一行）。

use crate::ir::{IrBlock, IrBook, IrJuan, IrNoteKind, IrSource, IrSpan};
use crate::types::Chapter;

// ------------------------------------------------------------------
// 一、导出器（带配置）
// ------------------------------------------------------------------

/// TEI 导出器。目前只暴露「署名」与「缩进」，未来要加命名空间前缀 / 字符集 /
/// 是否内联坐标等，都从这里扩展，不动遍历逻辑。
#[derive(Clone, Debug)]
pub struct TeiExporter {
    /// 生成器署名（写进 `publisher`）。
    pub generator: String,
    /// 每层缩进串（默认两个空格）。
    pub indent: &'static str,
}

impl Default for TeiExporter {
    fn default() -> Self {
        Self {
            generator: "笺读 JianDu".to_string(),
            indent: "  ",
        }
    }
}

impl TeiExporter {
    /// 把整本 `IrBook` 序列化成标准 TEI XML 字符串。
    pub fn export(&self, book: &IrBook) -> String {
        let ind = self.indent;
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        w(&mut out, 0, ind, "<TEI xmlns=\"http://www.tei-c.org/ns/1.0\">");

        // ---- teiHeader ----
        w(&mut out, 1, ind, "<teiHeader>");
        w(&mut out, 2, ind, "<fileDesc>");
        w(&mut out, 3, ind, "<titleStmt>");
        w(&mut out, 4, ind, &format!("<title>{}</title>", esc_text(&book.meta.title)));
        if !book.meta.author.trim().is_empty() {
            w(&mut out, 4, ind, &format!("<author>{}</author>", esc_text(&book.meta.author)));
        }
        w(&mut out, 3, ind, "</titleStmt>");
        w(&mut out, 3, ind, "<publicationStmt>");
        w(&mut out, 4, ind, &format!("<publisher>{}</publisher>", esc_text(&self.generator)));
        w(&mut out, 4, ind, "<date>由笺读 JianDu 导出（TEI P5 兼容）</date>");
        w(&mut out, 3, ind, "</publicationStmt>");
        w(&mut out, 3, ind, "<sourceDesc>");
        w(
            &mut out,
            4,
            ind,
            &format!("<p>数据来源类型：{}；经笺读 IR 序列化。</p>", esc_text(book.source.label())),
        );
        w(&mut out, 3, ind, "</sourceDesc>");
        w(&mut out, 2, ind, "</fileDesc>");
        w(&mut out, 1, ind, "</teiHeader>");

        // ---- text / body ----
        w(&mut out, 1, ind, "<text>");
        w(&mut out, 2, ind, "<body>");
        let mut n = 0usize;
        for juan in &book.juans {
            n += 1;
            emit_juan(&mut out, 3, ind, juan, n);
        }
        w(&mut out, 2, ind, "</body>");
        w(&mut out, 1, ind, "</text>");
        w(&mut out, 0, ind, "</TEI>");
        out
    }
}

// ------------------------------------------------------------------
// 二、遍历 IR → TEI（三路分离在此强制落地）
// ------------------------------------------------------------------

/// 一个巻 / 小节。
fn emit_juan(out: &mut String, depth: usize, ind: &str, juan: &IrJuan, n: usize) {
    let div_type = if juan.level <= 1 { "juan" } else { "section" };
    w(out, depth, ind, &format!("<div type=\"{div_type}\" n=\"{n}\">"));
    w(out, depth + 1, ind, &format!("<head>{}</head>", esc_text(&juan.title)));

    let mut para_n = 0usize;
    for b in &juan.blocks {
        match b {
            // 结构：小节标题（非数据流，按层级落 <head type="sub">）。
            IrBlock::SubHead { level, text } => {
                w(
                    out,
                    depth + 1,
                    ind,
                    &format!("<head type=\"sub\" level=\"{level}\">{}</head>", esc_text(text)),
                );
            }
            // 批注流：眉批 / 题解（章首独立小字）。
            IrBlock::HeadNote(t) => {
                w(
                    out,
                    depth + 1,
                    ind,
                    &format!(
                        "<note type=\"annotation\" subtype=\"headnote\" place=\"top\">{}</note>",
                        esc_text(t)
                    ),
                );
            }
            // 夹注 / 批注块（独立成块）。按 kind 决定它是夹注还是批注。
            IrBlock::Annotation { kind, spans } => {
                let typ = match kind {
                    IrNoteKind::Note | IrNoteKind::Tongjia => "gloss",
                    _ => "annotation",
                };
                w(out, depth + 1, ind, &format!("<note type=\"{typ}\" subtype=\"block\">"));
                let inner: String = spans.iter().map(|s| s.plain()).collect();
                w(out, depth + 2, ind, &esc_text(&inner));
                w(out, depth + 1, ind, "</note>");
            }
            // 正文段：内部再逐片三路分离。
            IrBlock::Para(spans) => {
                para_n += 1;
                emit_para(out, depth + 1, ind, spans, para_n);
            }
        }
    }
    w(out, depth, ind, "</div>");
}

/// 一段正文：body 落到 `<p>` 文本流，gloss / annotation 落到各自的专用标签。
fn emit_para(out: &mut String, depth: usize, ind: &str, spans: &[IrSpan], n: usize) {
    w(out, depth, ind, &format!("<p n=\"{n}\">"));
    let mut body = String::new();
    for sp in spans {
        match sp {
            // —— 正文流 ——
            IrSpan::Text(t) => body.push_str(t),
            // 注音：TEI P5 标准 <ruby>。
            IrSpan::Ruby { base, rt } => {
                flush_body(out, &mut body, depth + 1, ind);
                w(
                    out,
                    depth + 1,
                    ind,
                    &format!("<ruby><rb>{}</rb><rt>{}</rt></ruby>", esc_text(base), esc_text(rt)),
                );
            }
            // —— 夹注 / 批注流（按 kind 强制分流）——
            IrSpan::Note { kind, text } => {
                flush_body(out, &mut body, depth + 1, ind);
                let el = match kind {
                    // 夹注（双行夹注 / 通假）：落在 <note type="gloss">。
                    IrNoteKind::Note | IrNoteKind::Tongjia => {
                        format!("<note type=\"gloss\" place=\"interlinear\">{text}</note>", text = esc_text(text))
                    }
                    // 着重：<hi rend="emph">。
                    IrNoteKind::Emph => format!("<hi rend=\"emph\">{text}</hi>", text = esc_text(text)),
                    // 人物：专用 <persName>（知识图谱实体）。
                    IrNoteKind::Person => format!("<persName>{text}</persName>", text = esc_text(text)),
                    // 地名：专用 <placeName>。
                    IrNoteKind::Place => format!("<placeName>{text}</placeName>", text = esc_text(text)),
                    // 书名：专用 <title level="m">（monograph）。
                    IrNoteKind::Book => format!("<title level=\"m\">{text}</title>", text = esc_text(text)),
                };
                w(out, depth + 1, ind, &el);
            }
        }
    }
    flush_body(out, &mut body, depth + 1, ind);
    w(out, depth, ind, "</p>");
}

// ------------------------------------------------------------------
// 三、工具：缩进写入 / XML 转义 / body 缓冲冲刷
// ------------------------------------------------------------------

/// 按深度写一行。
fn w(out: &mut String, depth: usize, ind: &str, s: &str) {
    for _ in 0..depth {
        out.push_str(ind);
    }
    out.push_str(s);
    out.push('\n');
}

/// 把累积的正文缓冲作为独立一行冲刷（文本节点不内联进标签行，便于阅读）。
fn flush_body(out: &mut String, body: &mut String, depth: usize, ind: &str) {
    if !body.is_empty() {
        w(out, depth, ind, &esc_text(body));
        body.clear();
    }
}

/// 文本节点转义（XML 1.0 必转 &:<>）。
fn esc_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            _ => o.push(c),
        }
    }
    o
}

// ------------------------------------------------------------------
// 四、便捷入口（兼容 V1.0 纯文本 / Chapter）
// ------------------------------------------------------------------

/// 主入口：导出任意 `IrBook`。真实书稿 = `IrBook::from_book(&book, src)` 后喂进来。
pub fn export_tei(book: &IrBook) -> String {
    TeiExporter::default().export(book)
}

/// 直接吃 `types::Chapter`（复用 Chapter 结构）。
pub fn export_chapter(ch: &Chapter) -> String {
    let j = IrJuan::from_chapter(ch);
    let mut book = IrBook::new(&ch.title, "", IrSource::Txt);
    book.juans.push(j);
    export_tei(&book)
}

/// V1.0 纯文本书快捷入口：把纯文本按行拆段，复用同一导出管线，零额外代码。
pub fn export_plain_text(title: &str, author: &str, text: &str) -> String {
    let mut book = IrBook::new(title, author, IrSource::Txt);
    let mut j = IrJuan::new("正文");
    for line in text.lines() {
        let l = line.trim();
        if !l.is_empty() {
            j.para(l);
        }
    }
    book.juans.push(j);
    export_tei(&book)
}

// ------------------------------------------------------------------
// 五、自检
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrBook, IrJuan};

    /// 一份覆盖三路 + 实体的样张。
    fn sample() -> IrBook {
        let mut book = IrBook::new("測試", "佚名", IrSource::Txt);
        let mut j = IrJuan::new("卷一");
        j.blocks.push(IrBlock::HeadNote("題解示例".into()));
        j.blocks.push(IrBlock::Para(vec![
            IrSpan::Text("昔".into()),
            IrSpan::Note { kind: IrNoteKind::Person, text: "劉邦".into() },
            IrSpan::Text("都".into()),
            IrSpan::Note { kind: IrNoteKind::Place, text: "長安".into() },
            IrSpan::Text("，讀".into()),
            IrSpan::Note { kind: IrNoteKind::Book, text: "史記".into() },
            IrSpan::Text("，".into()),
            IrSpan::Note { kind: IrNoteKind::Note, text: "紀傳體也。".into() },
            IrSpan::Text("又".into()),
            IrSpan::Ruby { base: "觀".into(), rt: "guān".into() },
            IrSpan::Text("之。".into()),
        ]));
        j.blocks
            .push(IrBlock::Annotation { kind: IrNoteKind::Note, spans: vec![IrSpan::Text("夾注塊示例".into())] });
        book.juans.push(j);
        book
    }

    #[test]
    fn tei_is_well_formed_shell() {
        let xml = export_tei(&sample());
        assert!(xml.starts_with("<?xml"), "缺 XML 声明");
        assert!(xml.contains("http://www.tei-c.org/ns/1.0"), "缺 TEI 命名空间");
        assert!(xml.contains("<TEI") && xml.contains("</TEI>"), "TEI 根不闭合");
        assert!(xml.contains("<teiHeader>") && xml.contains("<text>") && xml.contains("<body>"));
    }

    #[test]
    fn tei_separates_three_streams() {
        let xml = export_tei(&sample());
        // 正文
        assert!(xml.contains("<p n=\"1\">"), "缺正文段");
        // 夹注（inline + block）
        assert!(xml.contains("<note type=\"gloss\""), "缺夹注");
        // 批注（眉批）
        assert!(xml.contains("<note type=\"annotation\""), "缺批注（眉批）");
        // 实体标签
        assert!(xml.contains("<persName>劉邦</persName>"), "缺人物标签");
        assert!(xml.contains("<placeName>長安</placeName>"), "缺地名标签");
        assert!(xml.contains("<title level=\"m\">史記</title>"), "缺书名标签");
        assert!(xml.contains("<ruby><rb>觀</rb><rt>guān</rt></ruby>"), "缺注音标签");
    }

    #[test]
    fn tei_escapes_special_chars() {
        let mut book = IrBook::new("X", "", IrSource::Txt);
        let mut j = IrJuan::new("卷");
        j.blocks.push(IrBlock::Para(vec![IrSpan::Text("a & b < c > d".into())]));
        book.juans.push(j);
        let xml = export_tei(&book);
        assert!(xml.contains("a &amp; b &lt; c &gt; d"), "特殊字符未转义");
        assert!(!xml.contains("a & b < c"));
    }

    #[test]
    fn plain_text_compat_has_no_notes() {
        let xml = export_plain_text("純文本", "佚名", "第一行\n\n第二行");
        assert!(xml.contains("<p n=\"1\">第一行</p>") || xml.contains("第一行"), "纯文本段丢失");
        assert!(!xml.contains("<note"), "纯文本不该出现 note");
    }
}
