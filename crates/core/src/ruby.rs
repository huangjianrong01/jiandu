//! 竖排注音（生僻字拼音）几何：**整串顺时针旋转 90°**、贴字格右侧安居。
//!
//! v1.85.0 定式。旧做法「一字母一行」是错的：每个字母独占一整个行高
//! （`fs × 1.02`），四个字母就散成一串被拉长的「小蚂蚁」，既不紧凑又容易
//! 顶破行高、与邻字粘连——正是用户截图里「挤压的小蚂蚁 + 跟旁边的字粘住」。
//!
//! 正确形态：**整串拼音作为一个整体顺时针旋转 90°**。旋转之后：
//!
//! ```text
//! ┌────────┐
//! │      m │   ← 纵向长 = 整串连写的自然宽度（Σ 字母步进 ≈ 字母数 × fs × ADVANCE）
//! │      ě │   ← 横向宽 = 单个字母的高度（asc + desc ≈ fs × CAP）
//! │      n │   ← 右侧留 RUBY_GUTTER 呼吸间距，绝不贴死界栏
//! │      g │
//! │   猛   │
//! └────────┘
//! ```
//!
//! 于是「横排拼音串的宽度」变成了「纵向长度」：4 个字母 ≈ `2.24 × fs`，
//! 比旧式 `4.08 × fs` 短了近一半，字号反倒能从 30% 提到 **40%** 仍放得下。
//!
//! ⚠️ 渲染层怎么真的转起来：GPUI 0.2.2 的 `Window::paint_glyph` 把
//! `TransformationMatrix::unit()` 写死，屏幕侧**无法旋转字形**。故屏幕走
//! `ruby_glyph`：CoreGraphics 离屏按本文件的度量把整串转 90° 渲成位图，
//! 再由 `img` 元素贴上来（PDF 侧 CoreGraphics 直接转 CTM）。屏幕与纸面共用
//! 同一套度量，逐字同位。

/// 目标拼音字号 = 正文的 **40%**（v1.85.0 定稿，旧为 30%）。
pub const RUBY_SCALE: f32 = 0.40;
/// 收敛下限 = 正文的 22%（再小就不可读；旧版 18% 正是「小蚂蚁」的成因）。
pub const RUBY_SCALE_MIN: f32 = 0.22;
/// 拉丁字母的平均步进宽度（相对字号）——整串连写的自然宽度按它算。
/// 取 0.56 略偏保守：宁可把串量长一点、字号收一档，也不能真的溢出格高。
pub const RUBY_ADVANCE: f32 = 0.56;
/// 横向占宽 = 字号 × 1.05：旋转后横向是**字母的高度**（asc + desc），留一丝余量。
pub const RUBY_CAP: f32 = 1.05;
/// 右侧呼吸间距 = 字格宽的这个比例（注音绝不贴死界栏、不压进邻列）。
pub const RUBY_GUTTER: f32 = 0.08;
/// 纵向最多只能吃掉的格高比例（余下的留给行气）。
pub const RUBY_AVAIL: f32 = 0.94;

/// 一次注音排版的度量结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyLayout {
    /// 拼音字号（短拼音恒为正文 40%，超长时收敛）。
    pub fs: f32,
    /// 旋转后**单个字母占的纵向长**（= `fs × RUBY_ADVANCE`）。
    pub line_h: f32,
    /// 整条拼音的纵向总长（字母数 × `line_h`）。
    pub block_h: f32,
    /// 整条拼音的横向宽（= 单字母的高度）。
    pub block_w: f32,
    /// 顶部偏移：右上角安居，只留一丝上边距。
    pub top: f32,
    /// 右侧呼吸间距（px）：注音外缘到界栏之间必须留出的空隙。
    pub gutter: f32,
    /// 是否已降到下限仍放不下（渲染层允许极少量下凸）。
    pub tight: bool,
}

/// 计算一条拼音的竖排度量（旋转 90° 之后的形态）。
///
/// * `pinyin`：注音串（可含声调字符，如 `xiǎn` / `zhuāng`）；
/// * `body_fs`：正文字号；
/// * `cell_w` / `cell_h`：字格宽高。
pub fn layout(pinyin: &str, body_fs: f32, cell_w: f32, cell_h: f32) -> RubyLayout {
    let letters = pinyin.chars().count().max(1) as f32;
    let avail_h = (cell_h * RUBY_AVAIL).max(1.0);
    let mut fs = body_fs * RUBY_SCALE;
    let mut tight = false;

    // ① 纵向（旋转后 = 串长）放不下 → 按可用高反算字号；触底则锁下限。
    if letters * fs * RUBY_ADVANCE > avail_h {
        fs = avail_h / (letters * RUBY_ADVANCE);
        if fs < body_fs * RUBY_SCALE_MIN {
            fs = body_fs * RUBY_SCALE_MIN;
            tight = true;
        }
    }
    // ② 横向兜底：右侧先扣掉呼吸间距，余下的才是注音能占的宽。
    let gutter = (cell_w * RUBY_GUTTER).max(1.0);
    let max_w = ((cell_w * 0.45) - gutter).max(4.0);
    if fs * RUBY_CAP > max_w {
        fs = max_w / RUBY_CAP;
    }
    let fs = fs.max(4.0).min(body_fs * RUBY_SCALE);
    let line_h = fs * RUBY_ADVANCE;
    RubyLayout {
        fs,
        line_h,
        block_h: letters * line_h,
        block_w: fs * RUBY_CAP,
        top: body_fs * 0.02,
        gutter,
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
                // 行款字数：按列数查行款表（v1.32.0）。
                target_chars: crate::grid::LayoutConfig::by_columns(cols)
                    .map(|c| c.target_chars)
                    .unwrap_or(21),
                // 留白默认档「中（75%）」（v1.33.0）。
                banxin_ratio: crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT],
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
            assert!(l.block_w + l.gutter <= cw * 0.45, "{cols} 列：拼音压到左列");
            assert!(l.fs >= fs * RUBY_SCALE_MIN - 0.01);
        }
    }

    #[test]
    fn long_pinyin_shrinks_and_still_fits() {
        for cols in [8usize, 14] {
            let (cw, chh, fs) = cell(cols);
            let l = layout("zhuāng", fs, cw, chh);
            assert!(l.fs <= fs * RUBY_SCALE + 0.01);
            assert!(l.fs >= fs * RUBY_SCALE_MIN - 0.01, "{cols} 列降到下限以下");
            assert!(l.block_h <= chh * 1.10 + 0.01, "{cols} 列：长拼音纵向溢出");
            assert!(l.block_w + l.gutter <= cw * 0.45, "{cols} 列：长拼音压到左列");
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
            assert!(l.block_w + l.gutter <= cw * 0.45, "{py} 越界");
        }
    }

    /// v1.85.0 定式：短拼音**吃满正文 40%**，且纵向长 = 整串连写的自然宽度
    /// （旋转 90° 后的长度），绝不再是「一字母一行」的松散竖叠。
    #[test]
    fn short_pinyin_reaches_the_forty_percent_cap() {
        let (cw, chh, fs) = cell(10);
        for py in ["a", "xǐ", "bō", "rě", "měng"] {
            let l = layout(py, fs, cw, chh);
            assert!(
                (l.fs - fs * RUBY_SCALE).abs() < 0.01,
                "{py} 没吃满 40%（实得 {:.3}）",
                l.fs / fs
            );
            assert!((l.line_h - l.fs * RUBY_ADVANCE).abs() < 1e-4, "{py} 步进不是字母自然宽");
            assert!((l.block_w - l.fs * RUBY_CAP).abs() < 1e-4, "{py} 横向不是单字母高");
        }
        // 旧式「一字母一行」：四个字母要占到 4.08 × fs；旋转式只有 2.24 × fs。
        let l = layout("měng", fs, cw, chh);
        assert!(
            l.block_h < fs * 1.02 * 4.0 * 0.75,
            "仍是「一字母一行」的松散排法（block_h={:.3}, fs={:.3}）",
            l.block_h,
            fs
        );
    }

    /// 与上下字、左右列之间都留得出气：**纵向不溢出格高，横向不越过界栏**。
    #[test]
    fn ruby_never_glues_to_the_neighbouring_glyph_or_column() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["a", "xi", "měng", "suān", "zhuāng", "xiǎn"] {
                let l = layout(py, fs, cw, chh);
                assert!(l.gutter > 0.0, "{cols} 列：没有呼吸间距");
                assert!(
                    l.block_h <= chh * RUBY_AVAIL + 0.01 || l.tight,
                    "{cols} 列 / {py}：注音纵向吃到下一个字（{:.2} > {:.2}）",
                    l.block_h,
                    chh * RUBY_AVAIL
                );
                assert!(
                    l.gutter + l.block_w <= cw * 0.45 + 0.01,
                    "{cols} 列 / {py}：注音压过界栏（{:.2} > {:.2}）",
                    l.gutter + l.block_w,
                    cw * 0.45
                );
            }
        }
    }
}
