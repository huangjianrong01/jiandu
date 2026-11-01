//! uniffi 暴露给 SwiftUI 的接口。
//!
//! 设计要点：
//! * 全部走 uniffi proc-macro（0.28），不需要 `setup_scaffolding!()`；
//! * `GlyphBoxFfi` 是**页空间坐标**（原点=纸面左上，单位=px），与 macOS 端
//!   `hit::GlyphBox` 同系——这正是将来 Apple Pencil 红线坐标反推 PDF 空间的
//!   对照基准（见 IPAD_POC_CHECKLIST.md §3）。

use jiandu_core::grid::{calculate_grid, GridDir, GridInput, LayoutMode};
use uniffi::Record;

/// 一个字的页空间包围盒（PoC 用；与渲染同源、与 Pencil 对齐同系）。
#[derive(Record, Clone, Debug)]
pub struct GlyphBoxFfi {
    /// 单字（UTF-8）。
    pub ch: String,
    /// 左上角 X（px，纸面坐标）。
    pub x: f64,
    /// 左上角 Y（px，纸面坐标，Y 向下）。
    pub y: f64,
    /// 格宽。
    pub w: f64,
    /// 格高。
    pub h: f64,
}

/// iOS PDF 导出的错误类型（uniffi 惯用错误枚举，避免 String 作 throw 类型的解析 bug）。
#[derive(uniffi::Error, Clone, Debug)]
pub enum PdfError {
    /// 当前平台不支持 PDF 导出（仅 iOS 可用）。
    UnsupportedPlatform,
    /// Core Graphics / Core Text 调用失败。
    CoreFramework(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::UnsupportedPlatform => {
                write!(f, "PDF export is only available on iOS (CoreGraphics / CoreText bridge)")
            }
            PdfError::CoreFramework(msg) => write!(f, "Core framework error: {msg}"),
        }
    }
}

impl From<String> for PdfError {
    fn from(s: String) -> Self {
        PdfError::CoreFramework(s)
    }
}

/// 当前笺读版本号（标识 FFI 与 core 同源）。
#[uniffi::export]
pub fn jiandu_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 排版探针：把一段文本按 core 真实的 `calculate_grid` 排成竖排网格，
/// 返回每个字的页空间包围盒。SwiftUI 可据此绘制，Pencil 红线落点可与
/// 之逐字比对，验证坐标对齐（IPAD_POC_CHECKLIST.md §3.4 的决定性测试）。
#[uniffi::export]
pub fn layout_probe(text: String, columns: u32) -> Vec<GlyphBoxFfi> {
    let cols = columns.max(1) as usize;
    let m = calculate_grid(GridInput {
        columns: cols,
        mode: LayoutMode::Facing,
        content_w: 800.0,
        content_h: 1000.0,
        zoom: 1.0,
        edge: 40.0,
        head: 70.0,
        foot: 35.0,
        margin: 0.06,
        line_scale: 1.0,
        fs_ratio: 0.0,
        line_ratio: 0.0,
        banxin_ratio: 0.0,
        dir: GridDir::Vertical,
        fs_max: f32::INFINITY,
    })
    .m;

    let rows = m.rows.max(1);
    let col_w = m.col_w as f64;
    let row_h = m.row_h as f64;
    let outer_w = m.outer_w as f64;
    let margin_top = m.margin_top as f64;

    // 自洽映射：竖排、列从右向左、行自上而下，原点=纸面左上（与 SwiftUI/Pencil 同系）。
    let n = text.chars().count();
    let total_cols = ((n + rows - 1) / rows).max(1);
    let grid_left = (outer_w - total_cols as f64 * col_w) / 2.0;

    let mut out = Vec::with_capacity(n);
    for (i, ch) in text.chars().enumerate() {
        let c = i / rows; // 从左数第几列
        let r = i % rows; // 列内第几行
        let x = grid_left + (total_cols - 1 - c) as f64 * col_w; // 右起顶格
        let y = margin_top + r as f64 * row_h;
        out.push(GlyphBoxFfi {
            ch: ch.to_string(),
            x,
            y,
            w: col_w,
            h: row_h,
        });
    }
    out
}

/// 把排版结果导出为 PDF（iOS 专属实现见 `pdf_export::render`）。
///
/// 非 iOS 平台（Mac 本机构建 / Simulator 的 host 校验）会返回 Err，
/// 保证同一份 Swift 绑定在任意 target 上签名一致、不会编译期差异。
#[uniffi::export]
pub fn export_layout_to_pdf(out_path: String, text: String, columns: u32) -> Result<(), PdfError> {
    #[cfg(target_os = "ios")]
    {
        crate::pdf_export::render(&out_path, &text, columns)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (out_path, text, columns);
        Err(PdfError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_nonempty() {
        assert!(!jiandu_version().is_empty());
    }

    #[test]
    fn probe_returns_boxes_for_real_grid() {
        let boxes = layout_probe("天地玄黄宇宙洪荒".to_string(), 12);
        assert_eq!(boxes.len(), 8);
        // 真实度量：字格宽应 > 0，且首字在纸面内。
        assert!(boxes[0].w > 0.0);
        assert!(boxes[0].x >= 0.0 && boxes[0].y >= 0.0);
    }
}
