//! 注音（生僻字读音 / 多字注记）几何：**竖排、小字、贴在主字右侧**，古籍双行夹注形态。
//!
//! v1.9.51 彻底重构——此前把注音横排「飘」在主字右上角，完全违背古籍版式。
//! 现在严格按**刻本双行小字夹注**的规矩来：主字在字格内贴左缘，注音作为一摞
//! 竖排小字排在主字**右侧的空当里**，字号只有主字的 50% 左右（短注音 0.55、
//! 多字注记 0.45），多字则拆成**双列竖排**（一列写前几个字、一列写后几个字），
//! 整块**垂直居中**于字格、绝不悬浮在间隙里，也**绝不压到右边的界栏线**。
//!
//! ```text
//! ┌─┬────────┐
//! │猛│四行    │   ← 古籍竖排注音：主字贴左缘，注音小字竖排于其右
//! │  │  本    │     短注音 55%，多字 45%，双列竖排（四行／本），紧贴、不越界栏
//! └─┴────────┘
//! ```
//!
//! 字号与排版的铁律：
//! * **短注音（≤2 字）= 主字 55%**；**多字注记（≥3 字）= 45%**；再放不下就按
//!   可用宽等比缩小，硬下限 30%（绝不压到字上、绝不越过界栏）。
//! * 列数由「字格竖向能容几行」反推（`rows = ⌊(格高 − 1) / 字号⌋`），多字自动
//!   拆成双列 / 多列竖排；整块宽一旦超过主字右缘 → 字格右缘的可用宽，立即缩字号，
//!   直到放得下——**横排被彻底禁止**，永远竖排。
//! * **主字与注音之间恒留 ≥1px 呼吸缝**（`RUBY_GAP_MIN`），绝不粘连；注音右缘
//!   恒在字格右缘内侧 `RUBY_RIGHT_INSET`（绝不压界栏）。
//! * **极简渲染**：取消注音周围冗余间距，小字步进 = 字号（紧贴如刻本原貌）。
//!
//! ⚠️ 屏幕与 PDF 共用本文件的度量（`layout` + `glyph_xy`），逐字同位。

/// 短注音（≤2 字）的目标字号 = 主字的 **55%**（古籍双行小字约半大的观感）。
pub const RUBY_SCALE_SHORT: f32 = 0.55;
/// 多字注记（≥3 字）的目标字号 = 主字的 **45%**（字数多，再小一档才塞得进右侧空当）。
pub const RUBY_SCALE_LONG: f32 = 0.45;
/// 字号硬下限 = 主字的 30%（再小就不可读；仅当右侧空当极窄时兜底触发）。
pub const RUBY_SCALE_MIN: f32 = 0.30;
/// 带注音的主字在字格内**贴左缘**时的左内边距（像素）。屏幕 `ancient_cell` 与
/// PDF `draw_cell` 共用，确保「主字左移」与注音落点严格同位。
pub const RUBY_CHAR_LEFT: f32 = 1.0;
/// 主字与注音之间的最小呼吸缝（像素）：用户铁律「绝不能粘在一起」——
/// 注音整块左缘恒在主字右缘右侧 ≥ 此值处。
pub const RUBY_GAP_MIN: f32 = 1.0;
/// 注音整块右缘距字格右缘的内边距（像素）：绝不压到右边的界栏线。
pub const RUBY_RIGHT_INSET: f32 = 1.0;
/// 注音多列之间的列距（像素）：极简，仅一丝呼吸。
pub const RUBY_COL_GAP: f32 = 1.0;

/// 一次注音排版的度量结果（**竖排双行夹注**形态）。
///
/// 所有长度都是「屏幕逻辑像素」，且都**相对字格左上角**（渲染层直接拿来定位，
/// PDF 侧同一套）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyLayout {
    /// 注音字号。
    pub fs: f32,
    /// 竖排的列数（多字注记会拆成 2 列或更多）。
    pub cols: usize,
    /// 每列最多行数（= ⌊(格高 − 1) / 字号⌋，竖向紧贴）。
    pub rows: usize,
    /// 整块宽（cols 列 + 列距）。
    pub block_w: f32,
    /// 整块高（rows × 字号，已垂直居中）。
    pub block_h: f32,
    /// **整块左缘**（紧贴主字右缘那一侧）相对字格左边的偏移。
    pub left: f32,
    /// 整块上缘相对字格上边的偏移（已垂直居中）。
    pub top: f32,
}

impl RubyLayout {
    /// 第 `i` 个字（0 基，按古籍阅读序：先左列后右列、列内自上而下）的
    /// **左上角**坐标（相对字格左上角）。
    ///
    /// 列 0 最贴近主字（最左列），列号向右递增；每列内自上而下排。
    pub fn glyph_xy(&self, i: usize) -> (f32, f32) {
        let rows = self.rows.max(1);
        let col = i / rows; // 0 = 紧贴主字
        let r = i % rows;
        let x = self.left + col as f32 * (self.fs + RUBY_COL_GAP);
        let y = self.top + r as f32 * self.fs;
        (x, y)
    }
}

/// 计算一条注音的竖排度量。
///
/// * `text`：注音串（中文注记，如 `四行本`；或拼音，如 `měng`）；
/// * `body_fs`：正文字号；
/// * `cell_w` / `cell_h`：字格宽高（本 app 竖排：字格**宽 > 高**，故主字右侧
///   恒有横向空当容得下竖排小字）。
pub fn layout(text: &str, body_fs: f32, cell_w: f32, cell_h: f32) -> RubyLayout {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len().max(1);
    // 短注音 55%，多字 45%。
    let scale0 = if n <= 2 {
        RUBY_SCALE_SHORT
    } else {
        RUBY_SCALE_LONG
    };
    // 主字贴左缘：右缘 = RUBY_CHAR_LEFT + body_fs。
    let char_right = RUBY_CHAR_LEFT + body_fs;
    // 注音可用宽 = 主字右缘 → 字格右缘 之间、扣掉两侧呼吸。
    let avail = (cell_w - char_right - RUBY_GAP_MIN - RUBY_RIGHT_INSET).max(2.0);
    // 迭代求字号：先用目标比例；若整块宽 > 可用宽，则按比例缩小，直到放得下
    // 或触底 RUBY_SCALE_MIN。竖向恒满足 block_h ≤ cell_h（rows 由格高 / 字号反推），
    // 故唯一会失败的是横向——缩字号即可。
    let mut scale = scale0;
    let (fs, cols, rows, block_w, block_h, top);
    loop {
        let fs_cand = (body_fs * scale).max(3.0);
        // 每列最多行数（竖向紧贴，步进 = 字号；留 1px 防粘连）。
        let rows_cand = (((cell_h - 1.0) / fs_cand).floor().max(1.0)) as usize;
        let cols_cand = (n + rows_cand - 1) / rows_cand; // ⌈n / rows⌉
        let bw =
            cols_cand as f32 * fs_cand + (cols_cand - 1).max(0) as f32 * RUBY_COL_GAP;
        if bw <= avail + 1e-3 || scale <= RUBY_SCALE_MIN + 1e-3 {
            fs = fs_cand;
            cols = cols_cand;
            rows = rows_cand;
            block_w = bw;
            block_h = rows as f32 * fs_cand;
            top = ((cell_h - block_h) * 0.5).max(0.0);
            break;
        }
        scale = (scale * 0.9).max(RUBY_SCALE_MIN);
    }
    // 整块左缘紧贴主字右缘（≥1px 呼吸缝）。
    let left = char_right + RUBY_GAP_MIN;
    RubyLayout {
        fs,
        cols,
        rows,
        block_w,
        block_h,
        left,
        top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{compute_grid, GridConfig, GridDir};

    /// 8 / 14 列两档的真实几何（宽屏 1320×880，与 `grid` 测试同源）。
    fn cell(cols: usize) -> (f32, f32, f32) {
        let m = compute_grid(
            1320.0,
            880.0 - 78.0,
            GridConfig {
                columns: cols,
                dir: GridDir::Vertical,
                margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
                target_chars: crate::grid::LayoutConfig::by_columns(cols)
                    .map(|c| c.target_chars)
                    .unwrap_or(21),
                banxin_ratio: crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT],
            },
            false,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,
            crate::grid::GRID_FS_MAX,
        );
        (m.col_w, m.row_h, m.fs)
    }

    /// 短注音（≤2 字）吃满 55%、多字注记（≥3 字）吃满 45%，且都不溢出字格右缘。
    #[test]
    fn short_keeps_055_long_keeps_045() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            // 单字 / 双字：55%
            for py in ["猛", "四行"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    (l.fs - fs * RUBY_SCALE_SHORT).abs() < 0.01,
                    "{cols} 列 / {py}：短注音没吃满 0.55（实得 {:.3}）",
                    l.fs / fs
                );
            }
            // 三字注记：45%
            for py in ["四行本"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    (l.fs - fs * RUBY_SCALE_LONG).abs() < 0.01,
                    "{cols} 列 / {py}：多字注记没吃满 0.45（实得 {:.3}）",
                    l.fs / fs
                );
            }
        }
    }

    /// 必为**竖排**：单列时整块高 ≥ 宽；且整块右缘绝不越过字格右缘（不压界栏）。
    #[test]
    fn is_vertical_and_stays_in_cell() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["猛", "四行", "四行本", "双行夹注示例"] {
                let l = layout(py, fs, cw, chh);
                // 单列 → 高 ≥ 宽（竖排特征）。
                if l.cols == 1 {
                    assert!(l.block_h >= l.block_w - 1e-3, "{py} 不是竖排（高 {:.2} < 宽 {:.2}）", l.block_h, l.block_w);
                }
                // 整块右缘 = left + block_w ≤ 字格右缘 − 内边距（绝不压界栏）。
                assert!(
                    l.left + l.block_w <= cw - RUBY_RIGHT_INSET + 0.01,
                    "{cols} 列 / {py}：注音越过字格右缘（右缘 {:.2} > {:.2}）",
                    l.left + l.block_w,
                    cw - RUBY_RIGHT_INSET
                );
                // 整块左缘在主字右缘右侧 ≥ 1px（绝不粘连主字）。
                let char_right = RUBY_CHAR_LEFT + fs;
                assert!(
                    l.left >= char_right + RUBY_GAP_MIN - 1e-3,
                    "{cols} 列 / {py}：注音压住了主字（左缘 {:.2} ≤ 字右缘 {:.2}）",
                    l.left,
                    char_right + RUBY_GAP_MIN
                );
                // 竖向不溢出字格。
                assert!(l.top >= 0.0 && l.top + l.block_h <= chh + 0.01, "{cols} 列 / {py}：纵向出格");
            }
        }
    }

    /// 多字注记正确拆成双列（或更多列）竖排：列数 × 每列行数覆盖全部字数。
    #[test]
    fn multi_char_splits_into_columns() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            // 「四行本」三字 → 应拆成 2 列（前 2 字 / 后 1 字）。
            let l = layout("四行本", fs, cw, chh);
            assert_eq!(l.cols, 2, "{cols} 列：四行本应拆 2 列");
            assert!(l.rows >= 2, "{cols} 列：每列应至少容 2 字");
            // 覆盖全部 3 字：cols × rows ≥ 3。
            assert!(l.cols * l.rows >= 3, "列数 × 行数 不足以容下 3 字");
            // 第 0 字（最贴近主字那列、首行）与第 2 字（次列）横向不同列。
            let (x0, _) = l.glyph_xy(0);
            let (x2, _) = l.glyph_xy(2);
            assert!((x2 - x0).abs() > l.fs * 0.5, "第 3 字应与首字不同列");
        }
    }

    /// 字号下限兜底：即使右侧空当极窄，也不跌破 30%、也不压到字上、也不越界栏。
    #[test]
    fn never_below_floor() {
        let (cw, chh, fs) = cell(14);
        // 较长注记（8 字）逼出收缩直至 30% 下限，且仍放得进右侧空当。
        let l = layout("双行夹注示例", fs, cw, chh);
        assert!(
            l.fs >= fs * RUBY_SCALE_MIN - 0.01,
            "字号跌破 30% 下限（{:.3}）",
            l.fs / fs
        );
        // 仍不越字格右缘、不与主字粘连。
        assert!(l.left + l.block_w <= cw - RUBY_RIGHT_INSET + 0.01);
        let char_right = RUBY_CHAR_LEFT + fs;
        assert!(l.left >= char_right + RUBY_GAP_MIN - 1e-3);
    }
}
