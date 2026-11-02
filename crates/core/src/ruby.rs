//! 注音（生僻字拼音）几何：**横排、正立、从左到右**，落在汉字右侧的列间空白里。
//!
//! v1.9.13 曾试过「整串竖排、旋转 90°」，用户最终要求回到**横排**：拼音作为
//! 一个整体、字母横向排列，严格垂直居中于汉字右侧，且整串宽度**紧贴列边线（字格右缘）**——
//! 既不挤占左列句读、也不溢出右界栏、更不横跨整列宽。
//!
//! ```text
//! ┌──────────┬──┐
//! │       猛 │měng│  ← 横排、正立、从左到右；字号 = 正文 50%
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

/// 目标拼音字号 = 正文的 **70%**（v1.9.47 由 62% 上调；40% → 50% → 62% → 70%
/// 一路加：用户要求拼音「够大、能看清」，且确认短拼音永远有富余列间空白）。
///
/// 单字母永远吃得满这一档（列间空白足够宽）；**双字母才是真正的天花板**——
/// 它受「汉字右缘 → 字格右缘」那条列间空白的物理封顶，再大就会越过界栏，
/// 故双字母在 12/14 列只能吃到约 0.61 倍（见 `layout` 的 `room` 公式）。
/// 三、四字母以上的串由 `block_w = min(letters × fs × ADVANCE, room)` 收敛，
/// 永远不越界栏。
///
/// | 列数 | 字格宽 | 正文字号 | 单字母 | 双字母 | 四字母（被宽度收敛） |
/// |-----:|------:|--------:|------:|------:|------------------:|
/// |  8 列 | 86.9 | 32.0 | **22.4** | 22.4 | 12.4 |
/// | 10 列 | 61.0 | 25.9 | **18.1** | 15.9 | 7.6 |
/// | 12 列 | 55.5 | 23.6 | **16.5** | 14.4 | 6.9 |
/// | 14 列 | 47.0 | 20.0 | **14.0** | 12.0 | 5.8 |
///
/// 想要双字母也突破 0.61，唯一办法是把汉字在字格里**左移**腾出右侧空白
/// （那是改主字排版的活，不是这个常量的事）——当前 0.61 已是「字居中」前提下
/// 的物理上限。
pub const RUBY_SCALE: f32 = 0.70;
/// 收敛下限 = 正文的 22%（再小就不可读）。
pub const RUBY_SCALE_MIN: f32 = 0.22;
/// 拉丁字母的平均步进宽度（相对字号）——整串横排宽度按它估。
///
/// v1.9.39：0.56 → 0.52。0.56 是按「最宽的 m/w」估的，可宋体小写里
/// `i` / `l` / `t` 只有半个身位，整串照 0.56 算会**白让**一成多宽度——
/// 那正是长拼音被压到 5px 的原因。0.52 留的仍是安全余量（真渲染宽度由
/// CoreGraphics 量，超了由 `block_w` 封顶，不会压到下一列字）。
pub const RUBY_ADVANCE: f32 = 0.52;
/// 整串的高度 = 字号 × 1.05（升部 + 降部 + 一丝余量）。
pub const RUBY_CAP: f32 = 1.05;

/// 一次注音排版的度量结果。
///
/// 所有长度都是「屏幕逻辑像素」，且除 `fs` 外都是**相对字格左上角**的偏移
/// （渲染层直接拿来定位，PDF 侧同一套）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyLayout {
    /// 拼音字号（短拼音恒为正文 50%，超长时按可用宽收敛）。
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
    // 汉字在字格居中 → 右缘 = (cell_w + body_fs)/2。
    let char_right = (cell_w + body_fs) * 0.5;
    // 左留白：与汉字右缘的呼吸——注音干净落在生字右边、不贴脸。v1.9.47 起压到
    // 0.5px：用户要求拼音「够大」，双字母被列间空白物理封顶，每一分留白都
    // 直接换成更大的字号（0.5px 仍清晰可辨，绝不压到字上）。
    let left_inset = 0.5;
    // 右留白：与「列边线」（字格右缘）的呼吸。v1.9.47 同样压到 0.5px——整串
    // 右缘仍由 `room` 硬封顶死死钉在 `cell_w − right_inset` 之内（绝不越界栏），
    // 0.5px 即是「留 0.5px 不贴线」；再大只会把双字母往下压，破 0.55 可读性底线。
    let right_inset = 0.5;
    // 注音可用宽硬上限 = 汉字右缘 → 列边线之间、扣掉两侧呼吸。
    // 整串右缘死死钉在 `cell_w − right_inset`：既不挤占左列句读，也不溢出右界栏。
    let room = (cell_w - char_right - left_inset - right_inset).max(2.0);
    // 目标字号 62%；受可用宽约束，放不下就按宽反算、下限 22%。
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
        // 紧贴生字右缘、整串向右铺到列边线（只留 right_inset 一丝呼吸）。
        left: char_right + left_inset,
        top: ((cell_h - block_h) * 0.5).max(0.0),
        // 右缘 ≤ 列边线 − right_inset，不会再溢出，故 `tight` 恒 false。
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

    /// 短拼音要吃到「新版比例」（v1.9.40：0.50 → 0.62）。
    ///
    /// 单字母一定吃得满（`room` 够宽）；**双字母不一定**——10 列时列间空白
    /// 只放得下 0.58 倍字号，那是几何，不是参数没调对。故按两条分别断言：
    /// 单字母 = 比例值；双字母 ≤ 比例值且明显大于旧值 0.50。
    #[test]
    fn short_pinyin_reaches_the_new_scale() {
        let (cw, chh, fs) = cell(10);
        // 真正的一字母（带声调的 `ō` 也算一个字母，故 `bō` 是两字母）。
        for py in ["a"] {
            let l = layout(py, fs, cw, chh);
            assert!(
                (l.fs - fs * RUBY_SCALE).abs() < 0.01,
                "{py} 单字母没吃满 {}（实得 {:.3}）",
                RUBY_SCALE,
                l.fs / fs
            );
            assert!(l.block_h <= chh, "{py} 纵向溢出格高");
            assert!((l.block_h - l.fs * RUBY_CAP).abs() < 1e-4);
        }
        for py in ["bō", "xǐ", "rě"] {
            let l = layout(py, fs, cw, chh);
            assert!(l.fs <= fs * RUBY_SCALE + 0.01, "{py} 超过比例上限");
            assert!(
                l.fs >= fs * 0.55,
                "{py} 双字母太小（{:.3}，旧值 0.62）",
                l.fs / fs
            );
            assert!(l.block_h <= chh, "{py} 纵向溢出格高");
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
                // 铁律一（v1.9.39 加严）：**整串不越出本列**——右缘不许过字格右缘，
                // 也就是不许压在界栏上。50% 字号之所以是上限，正是被这一条卡住的。
                assert!(
                    l.left + l.block_w <= cw + 0.01 || l.tight,
                    "{cols} 列 / {py}：注音越过本列右缘（{:.2} > {:.2}）",
                    l.left + l.block_w,
                    cw
                );
                // 铁律二：更不许够到下一列的字形。
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

    /// **0.55 可读性底线**（v1.9.46 用户铁律，v1.9.47 仍守住）：双字母注音在
    /// 8/10/12/14 列全部列里都不得低于正文字号的 0.55 倍。v1.9.47 把比例提到
    /// 0.70、留白压到 0.5px 后，双字母在 12/14 列约为 0.61（仍远高于 0.55 底线）；
    /// 本测试守的是下限，确保任何把留白放大、把双字母压到 0.55 以下的改动都被拦下。
    ///
    /// 注：`měng` 是四字母，本就属于「长拼音按宽收敛」的范畴（8 列仅 0.37），
    /// 不在此底线内；底线只覆盖一、二字母的短注音。
    #[test]
    fn two_letter_pinyin_never_drops_below_055() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["bō", "xǐ", "rě"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.fs >= fs * 0.55 - 1e-4,
                    "{cols} 列 / {py}：双字母跌破 0.55 底线（{:.3}）",
                    l.fs / fs
                );
                assert!(l.block_h <= chh + 0.01, "{cols} 列 / {py}：纵向溢出格高");
            }
        }
    }
}
