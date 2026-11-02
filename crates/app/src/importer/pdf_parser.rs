//! PDF 解析器：复用 `pdf_text` 的系统原生文本层抽取（macOS PDFKit /
//! 其他平台预留 pdfium 入口），只出「原始纯文本」，坐标走旁路塞进 `ParsedDoc`。
//!
//! 扫描图 PDF（无文本层）在 `parse` 里返回空串，由 `import_file` 判空后改报
//! `ImportError::OcrNeeded`，绝不把一本空笺塞给用户。

use std::path::Path;

use super::traits::FormatParser;
use super::ImportError;
use crate::pdf_text::{extract_pdf_text_regions, SourceRegion};

/// PDF 解析器（统一路由用：让 `Format` 体系认得 `.pdf`）。
///
/// 文本层抽取与坐标抽取共用 `extract_pdf_text_regions`；本 trait 方法只返回
/// 纯文本，坐标由 [`collect_regions`] 旁路装入 `ParsedDoc.pdf_regions`。
pub struct PdfParser;

impl FormatParser for PdfParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let regions = extract_pdf_text_regions(path);
        Ok(regions
            .iter()
            .map(|r| r.text.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn extensions() -> &'static [&'static str] {
        &["pdf"]
    }
}

/// 抽取 PDF 文本层坐标（供 `import_file` 旁路装入 `ParsedDoc.pdf_regions`）。
///
/// 扫描图 PDF 返回空 `Vec`，调用方据此判定「需 OCR」。
pub fn collect_regions(path: &Path) -> Vec<SourceRegion> {
    extract_pdf_text_regions(path)
}

/// **整本**逐页抽取（供书架上导入 PDF 用）。
///
/// 每抽完一页回调一次 `on_page(已抽页数, 总页数)`——导入看门狗 3 秒不见进度
/// 就判「文件损坏」并中止，一本两百页的书没有这个心跳必然被误杀。
///
/// 与 [`collect_regions`] 的差别：后者只取第 1 页，是给**校对台左侧参考图**
/// 用的（一次只显示一页，坐标不必跨页）；入书要的是整本。
pub fn collect_pages(
    path: &Path,
    on_page: &mut dyn FnMut(usize, usize),
) -> Vec<Vec<SourceRegion>> {
    crate::pdf_text::extract_pdf_pages(path, on_page)
}
