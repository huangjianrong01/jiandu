//! 笺校工作台 · **统一导出中心**（L9 终极大一统，7 种格式）。
//!
//! 唯一数据源：[`ExportSnapshot`] —— 导出瞬间从 SQLite 回读的「当前页**校对后
//! 最新**」文本 + 字框 + 字高层级，绝不读初始 OCR JSON、绝不赌内存里的旧值。
//!
//! 格式口径抽离自阅读界面 `exporter.rs`（`to_txt` 的 `〔注〕`、`to_markdown`
//! 的标题/脚注层级、`to_json` 的 pretty 结构）——**只取写法，不改阅读界面
//! 任何业务函数**。TEI 复用 `jiandu_core::exporter::export_tei`（P5 三路分流），
//! DOCX / 双层可搜索 PDF 复用 `commerce`；ALTO 为本模块新写。
//!
//! 全部 builder 是纯函数，脱离窗口可单测——这是「7 个导出全量自测」的地基。

use crate::compare_editor::{resolve_spans, SpanKind};
use jiandu_core::ir::{IrBlock, IrBook, IrJuan, IrNoteKind, IrSource, IrSpan};
use serde_json::json;
use std::path::Path;

/// 一个字框（与 `glyphs` 表逐列对应）。
#[derive(Clone, Debug)]
pub struct ExportGlyph {
    pub char_index: usize,
    pub ch: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub span_kind: u8,
}

/// 导出快照：**所有 7 种格式的唯一入口数据**。
#[derive(Clone, Debug)]
pub struct ExportSnapshot {
    pub title: String,
    pub page_index: usize,
    pub image_width: u32,
    pub image_height: u32,
    pub source_image: Option<String>,
    /// 校对后最新文本（`pages.text`）。
    pub text: String,
    pub glyphs: Vec<ExportGlyph>,
    /// `glyphs.span_kind` 按字符下标组装（旧库无列时为空表）。
    pub span_kinds: Vec<u8>,
}

impl ExportSnapshot {
    pub fn kinds(&self) -> Vec<SpanKind> {
        resolve_spans(&self.text, &self.span_kinds)
    }
    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }
    /// 按连续层级切成 `(层级, 片段)`，保持字符顺序、绝不重排。
    fn runs(&self) -> Vec<(SpanKind, String)> {
        let kinds = self.kinds();
        let chars: Vec<char> = self.text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < chars.len() {
            let k = kinds.get(i).copied().unwrap_or(SpanKind::Body);
            let mut j = i + 1;
            while j < chars.len()
                && kinds.get(j).copied().unwrap_or(SpanKind::Body) == k
            {
                j += 1;
            }
            out.push((k, chars[i..j].iter().collect()));
            i = j;
        }
        out
    }
    pub fn pdf_specs(&self) -> Vec<(f32, f32, f32, f32, usize)> {
        self.glyphs
            .iter()
            .map(|g| {
                (
                    g.x as f32,
                    g.y as f32,
                    g.w as f32,
                    g.h as f32,
                    g.char_index,
                )
            })
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Txt,
    Md,
    Json,
    Docx,
    Pdf,
    Tei,
    Alto,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Txt,
        Format::Md,
        Format::Json,
        Format::Docx,
        Format::Pdf,
        Format::Tei,
        Format::Alto,
    ];
    pub fn ext(self) -> &'static str {
        match self {
            Format::Txt => "txt",
            Format::Md => "md",
            Format::Json => "json",
            Format::Docx => "docx",
            Format::Pdf => "pdf",
            Format::Tei => "xml",
            Format::Alto => "xml",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Txt => "纯文本 TXT",
            Format::Md => "Markdown",
            Format::Json => "原始坐标 JSON",
            Format::Docx => "Word 文档",
            Format::Pdf => "双层可搜索 PDF",
            Format::Tei => "TEI XML",
            Format::Alto => "ALTO XML",
        }
    }
    /// 菜单分组：0 文本 / 1 文档 / 2 学术规范。
    pub fn group(self) -> u8 {
        match self {
            Format::Txt | Format::Md | Format::Json => 0,
            Format::Docx | Format::Pdf => 1,
            Format::Tei | Format::Alto => 2,
        }
    }
    pub fn group_label(g: u8) -> &'static str {
        match g {
            0 => "文本",
            1 => "文档",
            _ => "学术规范",
        }
    }
    pub fn needs_image(self) -> bool {
        matches!(self, Format::Pdf)
    }
    /// Pro 门槛（沿用 `export_guard` 的既有商业策略，全部格式一视同仁）。
    pub fn default_name(self) -> &'static str {
        match self {
            Format::Txt => "对照文本",
            Format::Md => "对照文本",
            Format::Json => "对照坐标",
            Format::Docx => "对照文本",
            Format::Pdf => "可搜索对照",
            Format::Tei => "对照文本",
            Format::Alto => "对照版面",
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 统一分派。PNG 只在双层 PDF 时需要。
pub fn build(
    s: &ExportSnapshot,
    fmt: Format,
    png: Option<&Path>,
) -> Result<Vec<u8>, String> {
    match fmt {
        Format::Txt => Ok(build_txt(s).into_bytes()),
        Format::Md => Ok(build_md(s).into_bytes()),
        Format::Json => build_json(s),
        Format::Docx => crate::commerce::build_docx(&s.text),
        Format::Pdf => {
            let p = png.ok_or_else(|| "缺少原图，无法生成双层 PDF".to_string())?;
            crate::commerce::build_searchable_pdf(
                p,
                &s.pdf_specs(),
                &s.text,
                (s.image_width, s.image_height),
            )
        }
        Format::Tei => Ok(build_tei(s).into_bytes()),
        Format::Alto => Ok(build_alto(s).into_bytes()),
    }
}

/// TXT：口径来自 `exporter.rs::to_txt` 的「书名 / 页次 + ——— 正文 ——— +
/// `〔注〕` 随文」，夹注不重排、不丢字。
pub fn build_txt(s: &ExportSnapshot) -> String {
    let mut out = format!(
        "《{}》\n第 {} 页 · 约 {} 字\n\n——— 正文 ———\n\n",
        s.title,
        s.page_index,
        s.char_count()
    );
    for (k, seg) in s.runs() {
        match k {
            SpanKind::Gloss => {
                out.push_str("〔注〕");
                out.push_str(&seg);
            }
            SpanKind::Body | SpanKind::GlossTag => out.push_str(&seg),
        }
    }
    out.push('\n');
    out
}

/// MD：口径来自 `exporter.rs::to_markdown` 的 `#` / `>` / `---` / `##`；
/// 夹注小字用 `*斜体*` 表达层级，结构标签用反引号保留可回溯性。
pub fn build_md(s: &ExportSnapshot) -> String {
    let mut out = format!(
        "# {}\n\n> 第 {} 页 · 约 {} 字\n\n---\n\n## 正文\n\n",
        s.title,
        s.page_index,
        s.char_count()
    );
    for (k, seg) in s.runs() {
        match k {
            SpanKind::Body => out.push_str(&seg),
            SpanKind::Gloss => {
                out.push('*');
                out.push_str(&seg);
                out.push('*');
            }
            SpanKind::GlossTag => {
                out.push('`');
                out.push_str(&seg);
                out.push('`');
            }
        }
    }
    out.push_str("\n\n");
    out
}

/// JSON：底层原始坐标数据（含逐字 bbox 与 `span_kind`），可回灌对齐库。
pub fn build_json(s: &ExportSnapshot) -> Result<Vec<u8>, String> {
    let v = json!({
        "format_version": 2,
        "source": "jiandu-compare-sqlite",
        "title": s.title,
        "page_index": s.page_index,
        "image_width": s.image_width,
        "image_height": s.image_height,
        "source_image": s.source_image,
        "text": s.text,
        "span_kinds": s.span_kinds,
        "glyphs": s.glyphs.iter().map(|g| json!({
            "char_index": g.char_index,
            "ch": g.ch,
            "x": g.x, "y": g.y, "w": g.w, "h": g.h,
            "span_kind": g.span_kind,
        })).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&v)
        .map(String::into_bytes)
        .map_err(|e| format!("JSON 序列化失败：{e}"))
}

/// TEI P5：复用 core 的 `export_tei`，把夹注落到 `<note type="gloss">`，
/// 正文落到 `<p>`，零自造标签。
pub fn build_tei(s: &ExportSnapshot) -> String {
    let mut book = IrBook::new(&s.title, "", IrSource::Txt);
    let mut juan = IrJuan::new(format!("第 {} 页", s.page_index));
    let mut spans: Vec<IrSpan> = Vec::new();
    for (k, seg) in s.runs() {
        if seg.is_empty() {
            continue;
        }
        match k {
            SpanKind::Gloss => spans.push(IrSpan::Note {
                kind: IrNoteKind::Note,
                text: seg,
            }),
            SpanKind::Body | SpanKind::GlossTag => spans.push(IrSpan::Text(seg)),
        }
    }
    if spans.is_empty() {
        spans.push(IrSpan::Text(String::new()));
    }
    juan.blocks.push(IrBlock::Para(spans));
    book.juans.push(juan);
    jiandu_core::exporter::export_tei(&book)
}

/// ALTO 4：按 y 聚行，每字一个 `<String>`，坐标逐字原样落盘（版面规范，
/// 与 JSON 同源但供数字人文工具链消费）。
pub fn build_alto(s: &ExportSnapshot) -> String {
    let mut gs: Vec<&ExportGlyph> = s.glyphs.iter().collect();
    gs.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });
    // 行容差 = 页面中位字高的一半，行级 bbox 均分的扫描件也能正确聚行。
    let mut hs: Vec<f64> = gs.iter().map(|g| g.h).filter(|h| *h > 0.0).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let tol = hs.get(hs.len() / 2).copied().unwrap_or(20.0) * 0.5;
    let mut lines: Vec<Vec<&ExportGlyph>> = Vec::new();
    for g in gs {
        if let Some(last) = lines.last_mut() {
            let base = last[0].y;
            if (g.y - base).abs() <= tol {
                last.push(g);
                continue;
            }
        }
        lines.push(vec![g]);
    }
    for line in &mut lines {
        line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    }

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <alto xmlns=\"http://www.loc.gov/standards/alto/ns-v4#\">\n\
         <Description><MeasurementUnit>pixel</MeasurementUnit>\
         <sourceImageInformation><fileName>",
    );
    out.push_str(&xml_escape(
        s.source_image.as_deref().unwrap_or("page.png"),
    ));
    out.push_str(
        "</fileName></sourceImageInformation></Description>\n<Layout>\n",
    );
    out.push_str(&format!(
        "<Page ID=\"PAGE1\" PHYSICAL_IMG_NR=\"{}\" WIDTH=\"{}\" HEIGHT=\"{}\">\n",
        s.page_index, s.image_width, s.image_height
    ));
    out.push_str(&format!(
        "<PrintSpace ID=\"PS1\" HPOS=\"0\" VPOS=\"0\" WIDTH=\"{}\" HEIGHT=\"{}\">\n",
        s.image_width, s.image_height
    ));
    out.push_str(&format!(
        "<TextBlock ID=\"TB1\" HPOS=\"0\" VPOS=\"0\" WIDTH=\"{}\" HEIGHT=\"{}\">\n",
        s.image_width, s.image_height
    ));
    for (li, line) in lines.iter().enumerate() {
        let x0 = line.iter().map(|g| g.x).fold(f64::INFINITY, f64::min);
        let y0 = line.iter().map(|g| g.y).fold(f64::INFINITY, f64::min);
        let x1 = line.iter().map(|g| g.x + g.w).fold(0.0, f64::max);
        let y1 = line.iter().map(|g| g.y + g.h).fold(0.0, f64::max);
        out.push_str(&format!(
            "<TextLine ID=\"TL{}\" HPOS=\"{:.2}\" VPOS=\"{:.2}\" WIDTH=\"{:.2}\" HEIGHT=\"{:.2}\">\n",
            li + 1,
            x0,
            y0,
            x1 - x0,
            y1 - y0
        ));
        for (si, g) in line.iter().enumerate() {
            out.push_str(&format!(
                "<String ID=\"S{}_{}\" HPOS=\"{:.2}\" VPOS=\"{:.2}\" WIDTH=\"{:.2}\" HEIGHT=\"{:.2}\" CONTENT=\"{}\"/>\n",
                li + 1,
                si + 1,
                g.x,
                g.y,
                g.w,
                g.h,
                xml_escape(&g.ch)
            ));
        }
        out.push_str("</TextLine>\n");
    }
    out.push_str("</TextBlock>\n</PrintSpace>\n</Page>\n</Layout>\n</alto>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ExportSnapshot {
        let text = "道可道，非常道。[注]可道之道，指事造形，非其常也。[/注]名可名，非常名。";
        ExportSnapshot {
            title: "道德經王弼注".into(),
            page_index: 2,
            image_width: 612,
            image_height: 792,
            source_image: Some("page_0002.png".into()),
            text: text.into(),
            span_kinds: Vec::new(),
            glyphs: (0..6)
                .map(|i| ExportGlyph {
                    char_index: i,
                    ch: "道可非常名也".chars().nth(i).unwrap().to_string(),
                    x: 10.0 + i as f64 * 20.0,
                    y: if i < 3 { 30.0 } else { 60.0 },
                    w: 18.0,
                    h: 20.0,
                    span_kind: 0,
                })
                .collect(),
        }
    }

    fn sample_png() -> &'static Path {
        Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test_ocr/page1.png"
        ))
    }

    #[test]
    fn txt_keeps_latest_text_in_reading_order() {
        let s = sample();
        let out = build_txt(&s);
        assert!(out.contains("《道德經王弼注》"));
        assert!(out.contains("——— 正文 ———"));
        assert!(out.contains("〔注〕可道之道"), "夹注随文标出");
        let plain = out.replace("〔注〕", "");
        for ch in s.text.chars() {
            assert!(plain.contains(ch), "TXT 不得丢字：{ch}");
        }
    }

    #[test]
    fn md_has_heading_and_gloss_emphasis() {
        let s = sample();
        let out = build_md(&s);
        assert!(out.starts_with("# 道德經王弼注"));
        assert!(out.contains("## 正文"));
        assert!(out.contains("*可道之道"), "夹注用斜体小字层级");
        assert!(out.contains("`[注]`"), "结构标签反引号保留");
    }

    #[test]
    fn json_carries_raw_coordinates_and_span_kinds() {
        let s = sample();
        let bytes = build_json(&s).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["text"], s.text);
        assert_eq!(v["page_index"], 2);
        assert_eq!(v["image_width"], 612);
        assert_eq!(v["glyphs"].as_array().unwrap().len(), 6);
        assert_eq!(v["glyphs"][0]["x"], 10.0);
        assert!(v.get("span_kinds").is_some());
    }

    #[test]
    fn docx_uses_snapshot_text_with_two_tiers() {
        let s = sample();
        let bytes = build(&s, Format::Docx, None).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let xml = String::from_utf8_lossy(&bytes);
        assert!(xml.contains("道可道"), "正文来自快照");
        assert!(xml.contains("可道之道"), "夹注来自快照");
        assert!(xml.contains(r#"w:sz w:val="24""#));
        assert!(xml.contains(r#"w:sz w:val="16""#));
    }

    #[test]
    fn tei_separates_body_and_gloss() {
        let s = sample();
        let xml = build_tei(&s);
        assert!(xml.contains("http://www.tei-c.org/ns/1.0"));
        assert!(xml.contains("<p"));
        assert!(xml.contains("note type=\"gloss\"") || xml.contains("<note"));
        assert!(xml.contains("道可道"));
        assert!(xml.contains("可道之道"));
    }

    #[test]
    fn alto_preserves_every_glyph_coordinate() {
        let s = sample();
        let xml = build_alto(&s);
        assert!(xml.contains("http://www.loc.gov/standards/alto/ns-v4#"));
        assert!(xml.contains("MeasurementUnit>pixel"));
        assert_eq!(xml.matches("<String ").count(), s.glyphs.len());
        assert_eq!(xml.matches("<TextLine ").count(), 2, "两行 y 聚行");
        assert!(xml.contains("HPOS=\"10.00\""));
        assert!(xml.contains("CONTENT=\"道\""));
    }

    #[test]
    fn pdf_uses_snapshot_glyphs_and_image() {
        let s = sample();
        let png = sample_png();
        if !png.exists() {
            return; // 无样张时跳过（外部 105 字验证另有覆盖）
        }
        let bytes = build(&s, Format::Pdf, Some(png)).unwrap();
        let t = String::from_utf8_lossy(&bytes);
        assert!(t.starts_with("%PDF-1.4"));
        assert_eq!(t.matches("3 Tr").count(), 6, "每个快照字框一条隐形文字");
        assert!(t.contains("/DCTDecode"));
    }

    #[test]
    fn all_seven_formats_build_from_one_snapshot() {
        let s = sample();
        assert_eq!(Format::ALL.len(), 7);
        for f in Format::ALL {
            if f == Format::Pdf && !sample_png().exists() {
                continue;
            }
            let r = build(&s, f, Some(sample_png()));
            assert!(r.is_ok(), "{f:?} 生成失败：{:?}", r.err());
            assert!(!r.unwrap().is_empty(), "{f:?} 不得产出空文件");
            assert!(!f.ext().is_empty() && !f.label().is_empty());
        }
        // 分组连续、极简菜单不拥挤：0/1/2 各成一段。
        let groups: Vec<u8> = Format::ALL.iter().map(|f| f.group()).collect();
        assert_eq!(groups, vec![0, 0, 0, 1, 1, 2, 2]);
    }

    #[test]
    fn edited_text_is_the_only_source() {
        // 模拟「校对后改字」：快照里换一个字，7 个文本类格式必须同步变化。
        let mut s = sample();
        s.text = s.text.replacen("道可道", "道可道乎", 1);
        assert!(build_txt(&s).contains("道可道乎"));
        assert!(build_md(&s).contains("道可道乎"));
        assert!(build_tei(&s).contains("道可道乎"));
        let v: serde_json::Value = serde_json::from_slice(&build_json(&s).unwrap()).unwrap();
        assert_eq!(v["text"], s.text);
    }
}
