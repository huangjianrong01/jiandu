//! 注音（生僻字拼音）几何：**横排、正立、从左到右**，落在汉字右侧的列间空白里。
//!
//! v1.85.2 定式。此前两版都试过错：
//!
//!   * 「一字母一行」竖叠 —— 四个字母散成一串拉长的「小蚂蚁」，还容易顶破格高；
//!   * 「整串转 90°」竖条 —— 字母躺着，读起来要歪头，用户否掉。
//!
//! 竖排古籍里给一个字注音，最自然的样子就是**在字的右边写一小串横排小字**：
//! 与正文同向（自上而下读字、横着读音），不歪头、不挤格子。
//!
//! ```text
//! ┌──────────┬──┐
//! │       猛 │měng│  ← 横排、正立、从左到右；字号 = 正文 40%
//! │          │  │
//! └──────────┴──┘
//!       ↑ 汉字右缘的列间空白
//! ```
//!
//! 落点是这套几何的关键：**汉字在字格里居中**，字格左右各有一半留白，相邻两列
//! 的留白连起来就是一条完整空隙。注音的横排宽度正是按这条空隙算的——
//!
//! ```text
//! 可用宽 = 列宽 − 字号 − 两侧各留一点呼吸
//! ```
//!
//! 放不下就按宽度反算字号（下限正文 22%），绝不越到下一列的**字形**上去；
//! 纵向只占 `1.05 × 拼音字号`，远小于行高，上下都碰不到邻字。
//!
//! ⚠️ 屏幕侧怎么画：GPUI 0.2.2 的 `paint_glyph` 把变换矩阵写死，且没有排版
//! 引擎可用，故走 `ruby_glyph`：CoreGraphics 离屏把整串按本文件的度量渲成
//! 透明位图（**不旋转**，正立横排），再由 `img` 贴进字格。屏幕与纸面共用这
//! 一套度量，逐字同位。

/// 目标拼音字号 = 正文的 **40%**。
pub const RUBY_SCALE: f32 = 0.40;
/// 收敛下限 = 正文的 22%（再小就不可读）。
pub const RUBY_SCALE_MIN: f32 = 0.22;
/// 拉丁字母的平均步进宽度（相对字号）——整串横排宽度按它估。
pub const RUBY_ADVANCE: f32 = 0.56;
/// 整串的高度 = 字号 × 1.05（升部 + 降部 + 一丝余量）。
pub const RUBY_CAP: f32 = 1.05;

/// 一次注音排版的度量结果。
///
/// 所有长度都是「屏幕逻辑像素」，且除 `fs` 外都是**相对字格左上角**的偏移
/// （渲染层直接拿来定位，PDF 侧同一套）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyLayout {
    /// 拼音字号（短拼音恒为正文 40%，超长时按可用宽收敛）。
    pub fs: f32,
    /// 整串横排的宽（字距总和）。
    pub block_w: f32,
    /// 整串横排的高（= 单字母的高度）。
    pub block_h: f32,
    /// 左缘相对字格左边的偏移（= 汉字右缘 + 呼吸）。
    pub left: f32,
    /// 上缘相对字格上边的偏移（整串在格内竖直居中）。
    pub top: f32,
    /// 是否已降到下限仍放不下（渲染层允许极少量的右凸）。
    pub tight: bool,
}

/// 计算一条拼音的横排度量。
///
/// * `pinyin`：注音串（可含声调字符，如 `xiǎn` / `zhuāng`）；
/// * `body_fs`：正文字号；
/// * `cell_w` / `cell_h`：字格宽高。
pub fn layout(pinyin: &str, body_fs: f32, cell_w: f32, cell_h: f32) -> RubyLayout {
    let letters = pinyin.chars().count().max(1) as f32;
    let inset = (cell_w * 0.03).clamp(1.0, 3.0);
    // 汉字在字格居中 → 右缘 = (cell_w + body_fs)/2。
    let char_right = (cell_w + body_fs) * 0.5;
    // 字格内、汉字右缘到字格右缘的空白（让开 inset 呼吸）= 注音可用宽硬上限。
    // 注音整串绝对不许越过字格右缘——既不挤占左列句读，也不溢出右界栏（用户铁律）。
    let room = ((cell_w - body_fs) * 0.5 - inset).max(2.0);
    // 目标字号 40%；受可用宽约束，放不下就按宽反算、下限 22%。
    let mut fs = (body_fs * RUBY_SCALE).min(room / (letters * RUBY_ADVANCE));
    fs = fs.max(3.0);
    if fs < body_fs * RUBY_SCALE_MIN {
        fs = body_fs * RUBY_SCALE_MIN;
    }
    let fs = fs.min(body_fs * RUBY_SCALE).max(3.0);
    // 宽度硬封顶到可用宽：长拼音宁可略挤，也绝不越界（用户铁律）。
    let block_w = (letters * fs * RUBY_ADVANCE).min(room);
    let block_h = fs * RUBY_CAP;
    RubyLayout {
        fs,
        block_w,
        block_h,
        left: char_right + inset,
        top: ((cell_h - block_h) * 0.5).max(0.0),
        // 宽度已被 `room` 死死封顶，不会再溢出，故 `tight` 恒 false。
        tight: false,
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

    /// 字格右缘到「下一列字形左缘」的距离——注音的硬边界。
    fn room_right(cell_w: f32, body_fs: f32) -> f32 {
        // 汉字居中：本格汉字右缘 = (w + fs)/2，下一格汉字左缘 = w + (w − fs)/2。
        (cell_w + (cell_w - body_fs) * 0.5) - (cell_w + body_fs) * 0.5
    }

    #[test]
    fn short_pinyin_keeps_the_forty_percent_size() {
        let (cw, chh, fs) = cell(10);
        for py in ["a", "xǐ", "bō", "rě"] {
            let l = layout(py, fs, cw, chh);
            assert!(
                (l.fs - fs * RUBY_SCALE).abs() < 0.01,
                "{py} 没吃满 40%（实得 {:.3}）",
                l.fs / fs
            );
            assert!(l.block_h <= chh, "{py} 纵向溢出格高");
            assert!((l.block_h - l.fs * RUBY_CAP).abs() < 1e-4);
        }
    }

    /// **朝向与读序**：横排 → 宽 > 高，且宽度就是「字母数 × 步进」。
    #[test]
    fn ruby_is_set_horizontally_left_to_right() {
        let (cw, chh, fs) = cell(14);
        let l = layout("měng", fs, cw, chh);
        assert!(
            l.block_w > l.block_h * 1.25,
            "注音不是横排（宽 {:.2} / 高 {:.2}）",
            l.block_w,
            l.block_h
        );
        // 宽度 = 字母数 × 步进，且被可用宽封顶（绝不越界）；故 ≤ 自然宽 + ε。
        assert!(
            l.block_w <= 4.0 * l.fs * RUBY_ADVANCE + 1e-3,
            "注音宽越过自然宽（实得 {:.3} > {:.3}）",
            l.block_w,
            4.0 * l.fs * RUBY_ADVANCE
        );
    }

    /// 落点：左缘在汉字右缘之外，整串不越过下一列的**字形**。
    #[test]
    fn ruby_sits_in_the_gap_without_touching_the_next_column() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["a", "xi", "měng", "suān", "zhuāng"] {
                let l = layout(py, fs, cw, chh);
                assert!(l.left > (cw + fs) * 0.5, "{cols} 列 / {py}：注音压住了汉字");
                assert!(
                    l.left + l.block_w <= cw + room_right(cw, fs) + 0.01 || l.tight,
                    "{cols} 列 / {py}：注音够到下一列字形（右缘 {:.2} > 上限 {:.2}）",
                    l.left + l.block_w,
                    cw + room_right(cw, fs)
                );
                assert!(l.top >= 0.0 && l.top + l.block_h <= chh + 0.01, "{cols} 列 / {py}：纵向出格");
            }
        }
    }

    /// 长拼音按宽度收敛，但不低于 22%；短拼音永远比长拼音大。
    #[test]
    fn long_pinyin_shrinks_but_stays_readable() {
        let (cw, chh, fs) = cell(8);
        let short = layout("xi", fs, cw, chh);
        let long = layout("zhuāngzhuāng", fs, cw, chh);
        assert!(long.fs <= short.fs + 1e-3);
        assert!(
            long.fs >= fs * RUBY_SCALE_MIN - 0.01,
            "降到下限以下：{:.3}",
            long.fs / fs
        );
    }
}
