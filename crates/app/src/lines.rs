//! 专名线与书名线（古籍竖排）。
//!
//! 标记为人名 / 地名的词组：在其所在列的**右侧**画一条**实线**（专名线）；
//! 标记为书名的词组：画**波浪线**（书名线）。
//!
//! 两条线都贴在字面右侧的同一线位上，逐字首尾相接：
//!   * 实线按字格通高画，相邻字格的线段在格界处正好接上，跨字、跨列连贯；
//!   * 波浪线是一串圆点连成的竖波，每个字格取**整数个周期**、首末相位归零，
//!     相邻字格以同一相位起收，点链跨格无缝；
//!   * 线位 / 波幅 / 波长都按字格宽与字号伸缩，列数（8~14）切换后依然贴合。
//!
//! # 为什么在右（v1.81.0 用户定）
//!
//! GB/T 15834 给直行文稿定的位置是**左侧**；这份实现照用户口径改到**右侧**，
//! 与朱批、着重号分成两边（那条「划线」的朱线才走左侧），一眼能分清
//! 「书里印的」与「读者加的」。改回左侧只需把 [`line_x`] 的镜像换回去。

use gpui::{div, prelude::*, px, rgb, Div};

use crate::types::MarkKind;

/// 线位：离字格**右缘**的比例（竖排文字居于格中央，线贴在字右侧）。
pub const LINE_INSET_FRAC: f32 = 0.10;
/// 线道：字格右侧留给专名线 / 书名线的留白带（占字格宽的比例）。
///
/// 竖排每字占满一个字格，字面墨迹大致落在格的 5%~95%。线道取右缘 20%，
/// 线的任何一部分（含波峰与点径）都被夹在这条带子里 —— 这是
/// 「不遮挡文字」的硬约束：线永远走字面右侧的空白，不压进墨迹。
pub const LINE_LANE_FRAC: f32 = 0.20;
/// 专名线粗细 = 字号比例（随字号伸缩；另有上下限，见 `name_line_w`）。
pub const NAME_LINE_W_FRAC: f32 = 0.045;
/// 专名线粗细的绝对上下限（px）：字号再小也看得见，再大也不糊。
pub const NAME_LINE_W_MIN: f32 = 1.0;
pub const NAME_LINE_W_MAX: f32 = 3.2;
/// 实线在字格上下各探出的长度（px）——消去格界处的抗锯齿缝。
const NAME_LINE_BLEED: f32 = 0.6;
/// 书名线波幅 = 字号比例。
pub const WAVE_AMP_FRAC: f32 = 0.10;
/// 书名线波长 = 字号比例（每格取整数个周期，跨格相位才接得上）。
pub const WAVE_PERIOD_FRAC: f32 = 0.9;
/// 每周期的采样点数（够密，点链才连成线）。
const WAVE_STEPS: f32 = 14.0;
/// 书名线点径 = 字号比例（随字号伸缩；与步长取大者，保证点链重叠成连续波）。
const WAVE_DOT_FRAC: f32 = 0.075;
/// 点径的绝对上下限（px）。
const WAVE_DOT_MIN: f32 = 1.6;
const WAVE_DOT_MAX: f32 = 6.0;

/// 线道宽度（px）：字格右缘留给线画的那条带子。
pub fn line_lane(cell_w: f32) -> f32 {
    cell_w * LINE_LANE_FRAC
}

/// 线位（px）：**距字格右缘**的距离，线以此为中心左右摆动。
pub fn line_x(cell_w: f32) -> f32 {
    cell_w * LINE_INSET_FRAC
}

/// 专名线粗细：**随字号自适应**。
///
/// 基准是字号的 `NAME_LINE_W_FRAC`（34px 字约 1.5px，17px 字约 0.77px），
/// 再夹进 `[MIN, MAX]` 与「不超过半个线道」—— 小字不细到看不见，
/// 大字不粗到糊成一团，且永远留在线道内不压字。
pub fn name_line_w(fs: f32, cell_w: f32) -> f32 {
    let base = fs * NAME_LINE_W_FRAC;
    let cap = (line_lane(cell_w) * 0.5).max(NAME_LINE_W_MIN);
    base.clamp(NAME_LINE_W_MIN, NAME_LINE_W_MAX.min(cap))
}

/// 波浪线的几何：波点序列 `(y, dx)`（`dx` 相对线位）与点径。
#[derive(Clone, Debug)]
pub struct WaveGeo {
    pub dots: Vec<(f32, f32)>,
    pub dot: f32,
}

/// 波浪线的波点序列。`dx = amp·sin(2πy/period)`，首末两点 `dx = 0`
/// （相位归零）——相邻字格同一相位起收，点链跨格无缝。
pub fn wave_geometry(cell_w: f32, cell_h: f32, fs: f32) -> WaveGeo {
    let want = fs * WAVE_PERIOD_FRAC;
    let periods = (cell_h / want).round().max(1.0);
    let period = cell_h / periods;
    let n = (periods * WAVE_STEPS).round().max(1.0) as usize;
    let step = cell_h / n as f32;
    let lane = line_lane(cell_w);
    // 点径：随字号伸缩，且必须 >= 步长 —— 相邻点充分重叠，
    // 读起来是一条**连续**的波（不再是一粒粒圆点）。
    let dot = (fs * WAVE_DOT_FRAC)
        .max(step * 1.5)
        .clamp(WAVE_DOT_MIN, WAVE_DOT_MAX)
        .min(lane * 0.6);
    // 波幅：随字号伸缩，但夹进线道 —— 波峰连同点半径不得探出线道，
    // 否则线会压到字面上（即便线画在字下，压进墨迹仍显脏）。
    let room = (lane - line_x(cell_w) - dot / 2.0).max(0.4);
    let amp = (fs * WAVE_AMP_FRAC).max(0.8).min(room);
    let mut dots = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let y = i as f32 * step;
        let dx = amp * (std::f32::consts::TAU * y / period).sin();
        dots.push((y, dx));
    }
    WaveGeo { dots, dot }
}

/// 专名线（人名 / 地名）：通高实线一条，上下两格的线段首尾正好接上。
pub fn name_line(cell_w: f32, cell_h: f32, fs: f32, color: u32) -> Div {
    // 与书名线共用同一条轴线（线位）：实线以线位为中心左右均分，
    // 换到波浪线时位置不跳。粗细随字号自适应。
    let w = name_line_w(fs, cell_w);
    // 从右缘反算 left：线心距右缘 `line_x`，故左缘是 `cell_w − line_x − w/2`。
    let x = (cell_w - line_x(cell_w) - w / 2.0).max(0.0);
    div()
        .absolute()
        .left(px(x))
        .top(px(-NAME_LINE_BLEED))
        .w(px(w))
        .h(px(cell_h + 2.0 * NAME_LINE_BLEED))
        .bg(rgb(color))
}

/// 书名线（书名）：一串圆点连成的竖波，与专名线同一线位（帖字右侧）。
pub fn book_wave(cell_w: f32, cell_h: f32, fs: f32, color: u32) -> Div {
    let x = line_x(cell_w);
    let geo = wave_geometry(cell_w, cell_h, fs);
    let mut line = div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(cell_w))
        .h(px(cell_h));
    for (y, dx) in geo.dots {
        let d = geo.dot;
        line = line.child(
            div()
                .absolute()
                // 镜像：中心距右缘 `x + dx`，左缘 = 格宽 − 该距离 − 半径。
                .left(px((cell_w - (x + dx) - d / 2.0).max(0.0)))
                .top(px(y - d / 2.0))
                .w(px(d))
                .h(px(d))
                .rounded(px(d / 2.0))
                .bg(rgb(color)),
        );
    }
    line
}

/// 按标记种类取对应的线（实线 / 波浪线）。
pub fn mark_line(kind: MarkKind, cell_w: f32, cell_h: f32, fs: f32, color: u32) -> Div {
    match kind {
        MarkKind::Name => name_line(cell_w, cell_h, fs, color),
        MarkKind::Book => book_wave(cell_w, cell_h, fs, color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CW: f32 = 46.0; // 一个典型字格宽

    #[test]
    fn wave_phase_returns_to_axis_at_cell_edges() {
        // 任意格高 / 字号：首末点都应回到线位（相位归零），跨格才接得上。
        for (ch, fs) in [(37.4, 34.0), (18.7, 17.0), (90.0, 60.0), (12.0, 14.0)] {
            let geo = wave_geometry(CW, ch, fs);
            assert!(!geo.dots.is_empty());
            let (y0, dx0) = geo.dots[0];
            assert!(y0.abs() < 0.01, "首点不在格顶");
            assert!(dx0.abs() < 0.01, "首点相位未归零：{dx0}");
            let (y1, dx1) = *geo.dots.last().unwrap();
            assert!((y1 - ch).abs() < 0.01, "末点不在格底");
            assert!(dx1.abs() < 0.01, "末点相位未归零：{dx1}");
        }
    }

    #[test]
    fn wave_stays_inside_lane() {
        // 波峰连同点半径都必须落在左缘线道内 —— 「不遮挡文字」的硬约束。
        for (cw, fs) in [(46.0, 34.0), (24.0, 17.0), (80.0, 60.0), (16.0, 12.0)] {
            let geo = wave_geometry(cw, 37.4, fs);
            let lane = line_lane(cw);
            let x = line_x(cw);
            for (_, dx) in &geo.dots {
                // 距**右缘**的距离：线心 `x + dx`，再各让半个点径。
                let lo = x + dx - geo.dot / 2.0;
                let hi = x + dx + geo.dot / 2.0;
                assert!(lo >= -0.01, "波点探出字格右缘：{lo}");
                assert!(
                    hi <= lane + 0.01,
                    "波点探进字面（越出线道）：{hi} > {lane}"
                );
            }
        }
    }

    #[test]
    fn wave_dots_overlap_so_the_line_is_continuous() {
        // 点径 >= 相邻点间距：点链重叠，读起来是连续的波而非一粒粒圆点。
        for fs in [12.0, 17.0, 34.0, 60.0] {
            let geo = wave_geometry(CW, 37.4, fs);
            let step = 37.4 / (geo.dots.len() - 1) as f32;
            assert!(
                geo.dot >= step - 0.01,
                "点径 {:.2} < 步长 {:.2}，波会断成点：fs={fs}",
                geo.dot,
                step
            );
            assert!(geo.dots.len() >= 8, "点太疏不成线：{}", geo.dots.len());
        }
    }

    #[test]
    fn name_line_width_scales_with_font_size() {
        // 线宽随字号自适应：字号越大线越粗，但有上下限（小字看得见、大字不糊）。
        let small = name_line_w(17.0, CW);
        let mid = name_line_w(34.0, CW);
        let big = name_line_w(60.0, CW);
        assert!(small < mid, "小字线应更细：{small} >= {mid}");
        assert!(mid < big, "大字线应更粗：{mid} >= {big}");
        assert!(small >= NAME_LINE_W_MIN);
        assert!(big <= NAME_LINE_W_MAX + 0.01);
        // 极小字号也保底可见。
        assert!(name_line_w(6.0, CW) >= NAME_LINE_W_MIN);
    }

    #[test]
    fn name_line_stays_inside_lane() {
        // 实线整条（含左右边）都在线道内，不压进字面。
        for (cw, fs) in [(46.0, 34.0), (24.0, 17.0), (80.0, 60.0), (16.0, 12.0)] {
            let w = name_line_w(fs, cw);
            let left = (line_x(cw) - w / 2.0).max(0.0);
            let lane = line_lane(cw);
            assert!(left >= 0.0, "线探出字格：{left}");
            assert!(
                left + w <= lane + 0.01,
                "线探进字面：{} > {}",
                left + w,
                lane
            );
            assert!(w < cw * 0.1 + 0.01, "线太粗压字：{w} / 格宽 {cw}");
        }
    }

    #[test]
    fn line_sits_left_of_glyph() {
        // 线位在字格左缘 10% 处：稳稳落在字面左侧的留白里。
        let x = line_x(CW);
        assert!(x < CW * 0.2);
        assert!(name_line_w(34.0, CW) < CW * 0.1);
    }
}
