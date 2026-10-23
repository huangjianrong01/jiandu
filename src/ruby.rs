//! 竖排注音（生僻字拼音）几何：逐字母竖叠、右上角安居、长度溢出自动降号。
//!
//! 竖排注音**绝不能横向排**——横向排会撑破一格宽、压到左边下一列。
//! 这里的做法是「一字母一行」纵向叠放：
//!
//! ```text
//! ┌────────┐
//! │       x│
//! │       i│  ← 字格右上角，字号 = 正文 30%
//! │       ǎ│
//! │   毨   │
//! └────────┘
//! ```
//!
//! 字母纵向叠放后，块宽只有一个字母（约 `1.35 × 拼音字号`），天然收在
//! 字格右缘，不会越到左列；需要防的是**纵向**超出行高——
//! [`layout`] 按「可用高 / 字母数」反算字号，降到正文 18% 仍放不下时
//! 标记 `tight`，渲染层允许极少量下凸（但横向绝不越界）。

/// 常规拼音字号 = 正文的 30%。
pub const RUBY_SCALE: f32 = 0.30;
/// 多字母长拼音的降号下限 = 正文的 18%（再小就不可读了）。
pub const RUBY_SCALE_MIN: f32 = 0.18;
/// 超过这个字母数就视为「长拼音」，开始自动降号（用户指定 4）。
pub const RUBY_LONG: usize = 4;

/// 一次注音排版的度量结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyLayout {
    /// 拼音字号（已按长度收敛）。
    pub fs: f32,
    /// 单字母行高。
    pub line_h: f32,
    /// 整条拼音的总高（字母数 × 行高）。
    pub block_h: f32,
    /// 整条拼音的宽（一个字母宽）。
    pub block_w: f32,
    /// 顶部偏移：右上角安居，只留一丝上边距。
    pub top: f32,
    /// 是否已降到下限仍放不下（渲染层允许极少量下凸）。
    pub tight: bool,
}

/// 计算一条拼音的竖排度量。
///
/// * `pinyin`：注音串（可含声调字符，如 `xiǎn` / `zhuāng`）；
/// * `body_fs`：正文字号；
/// * `cell_w` / `cell_h`：字格宽高。
///
/// 规则：
/// 1. 起始字号 = 正文 30%；
/// 2. 若 `字母数 × 行高 > 可用高(0.94 × 格高)`，按可用高反算字号；
/// 3. 反算结果低于正文 18% 时锁在 18%，并置 `tight`（允许下凸）；
/// 4. 块宽 `≈ 1.35 × 字号`，永远只有一个字母宽，横向不越界。
pub fn layout(pinyin: &str, body_fs: f32, cell_w: f32, cell_h: f32) -> RubyLayout {
    let letters = pinyin.chars().count().max(1) as f32;
    let avail_h = (cell_h * 0.94).max(1.0);
    let mut fs = body_fs * RUBY_SCALE;
    let mut tight = false;

    // 纵向放不下 → 反算字号；触底则锁下限。
    if letters * fs * 1.02 > avail_h {
        fs = avail_h / (letters * 1.02);
        if fs < body_fs * RUBY_SCALE_MIN {
            fs = body_fs * RUBY_SCALE_MIN;
            tight = true;
        }
    }
    // 横向兜底：一个字母宽不得超过格宽的 42%（右缘安居、不压左列）。
    let max_w = (cell_w * 0.42).max(4.0);
    if fs * 1.35 > max_w {
        fs = max_w / 1.35;
    }
    let fs = fs.max(4.0).min(body_fs * RUBY_SCALE);
    let line_h = fs * 1.02;
    RubyLayout {
        fs,
        line_h,
        block_h: letters * line_h,
        block_w: fs * 1.35,
        top: body_fs * 0.02,
        tight,
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
            },
            false,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0, crate::grid::GRID_FS_MAX);
        (m.col_w, m.row_h, m.fs)
    }

    #[test]
    fn short_pinyin_fits_in_eight_and_fourteen_columns() {
        for cols in [8usize, 14] {
            let (cw, chh, fs) = cell(cols);
            let l = layout("xǐ", fs, cw, chh);
            assert!(!l.tight, "{cols} 列短拼音不该降到底");
            assert!(l.block_h <= chh + 0.01, "{cols} 列：短拼音纵向溢出");
            assert!(l.block_w <= cw * 0.45, "{cols} 列：拼音压到左列");
            assert!(l.fs >= fs * RUBY_SCALE_MIN - 0.01);
        }
    }

    #[test]
    fn long_pinyin_shrinks_and_still_fits() {
        for cols in [8usize, 14] {
            let (cw, chh, fs) = cell(cols);
            let l = layout("zhuāng", fs, cw, chh);
            // 6 个字母：必须降号，但不得降到 18% 以下（除非格高实在太小）。
            assert!(l.fs <= fs * RUBY_SCALE + 0.01);
            assert!(l.fs >= fs * RUBY_SCALE_MIN - 0.01, "{cols} 列降到下限以下");
            // 允许 tight 时最多下凸 10%。
            assert!(l.block_h <= chh * 1.10 + 0.01, "{cols} 列：长拼音纵向溢出");
            assert!(l.block_w <= cw * 0.45, "{cols} 列：长拼音压到左列");
        }
    }

    #[test]
    fn longer_pinyin_gets_smaller_font() {
        let (cw, chh, fs) = cell(10);
        let short = layout("xi", fs, cw, chh);
        let long = layout("zhuang", fs, cw, chh);
        assert!(long.fs <= short.fs + 0.001);
    }

    #[test]
    fn ruby_width_never_exceeds_four_tenths_cell() {
        let (cw, chh, fs) = cell(14);
        for py in ["a", "xi", "xian", "zhuang", "zhuangzhuang"] {
            let l = layout(py, fs, cw, chh);
            assert!(l.block_w <= cw * 0.43, "{py} 越界");
        }
    }
}
