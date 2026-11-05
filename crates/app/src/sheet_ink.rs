//! **纸面墨线层**（v1.9.90）：版框 / 界行 / 版心全部改由 Canvas 按**绝对坐标**
//! 绘制，不再让 GPUI 的 flex 去「分配」它们。
//!
//! ## 为什么要把边框从布局树里搬出来
//!
//! 旧制把版框、界行、中缝都画成 `border_*` 的 div，尺寸由流式布局协商：
//! 哪一层 `flex_1`、哪一列被压成零宽、`justify_end` 把可见列推到哪边——
//! 框线的位置就跟着这些**协商结果**漂移。中缝尤其吃亏：它曾经按「一行
//! 字宽」参与平分，8 列时 57px、12 列时 39px，宽屏上就是一道「大峡谷」。
//!
//! 现在分成两层，各管一件事：
//!
//! ```text
//!   墨线层（本模块）：Canvas + 绝对坐标 —— 版框、界行、版心、鱼尾、象鼻、折痕
//!   文字层（reader）：GPUI 文本节点   —— 正文、夹注、句读、划线、高亮
//! ```
//!
//! 墨线层的坐标全部由**常量与格数**算出（`FRAME_INSET` + `k × cell_w`），
//! 与窗口宽、列数、flex 的协商结果**没有任何关系**；文字层只用同一个
//! `LeafInk` 把网格摆到 `(grid_x, grid_y)`。两层读同一份几何，故永远对齐。
//!
//! ## 一条铁律
//!
//! 这里只许画**线**（`paint_quad(fill(..))` / `paint_path`），不许画字。
//! 文字一律留在文字层——Canvas 接不了输入法、也不参与命中测试。
//!
//! ## 坐标原点
//!
//! 本模块的所有坐标都以「**版框外缘左上角**」为 (0, 0)。调用方必须把墨线
//! 画布放在一个**没有 padding 的 `relative` 容器**里（`top(0)/left(0)`），
//! 否则 GPUI 的绝对定位会以 padding box 为原点，整层偏移一个内衬。

use gpui::{
    canvas, div, point, prelude::*, px, rgb, size, App, Bounds, Div, ParentElement, PathBuilder,
    Pixels, Styled, Window,
};

/// 版框**外粗框**的线宽（3px）。
pub(crate) const FRAME_OUT: f32 = 3.0;
/// 外框与内框之间的**框内留边**（3px）：刻本「文武边栏」的两道线之间那道白。
pub(crate) const FRAME_PAD: f32 = 3.0;
/// 版框**内细线**的线宽（1px，与界行同粗）。
pub(crate) const FRAME_IN: f32 = 1.0;
/// 版框外缘 → 内框内缘的距离（3 + 3 + 1 = 7）。
///
/// 这一层既是**墨线的位置**，也是文字层容器的 padding——两者必须同源，
/// 差 1px 就会看到「框线压字」或「字离框一线」。
pub(crate) const FRAME_INSET: f32 = FRAME_OUT + FRAME_PAD + FRAME_IN;

/// 内框与字格之间的横向内衬（8px）：字与界行之间留一线白。
///
/// **只加横向**：天头地脚的 3:2 铁律一寸不动（见 `grid::TOP_OF_BOT`）。
pub(crate) const INNER_PAD: f32 = 8.0;

/// 一叶的墨线几何（纯数据，可单测）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LeafInk {
    /// 版框外缘宽 / 高。
    pub w: f32,
    pub h: f32,
    /// 一格的宽（每列等宽，`k × cell_w` 就是第 k 道界行）。
    pub cell_w: f32,
    /// 竖排总列数（**含章题列**——它也是一格，也要有栏线）。
    pub cols: usize,
    /// 外框四边（上 / 右 / 下 / 左）。
    pub sides: [bool; 4],
    /// 内框四边（裁边款式：四周双边全画 / 左右双边只画左右 / 四周单边不画）。
    pub inner_sides: [bool; 4],
}

impl LeafInk {
    /// 字格区左缘（= 版框外缘 + 内框 + 横向内衬）。
    pub fn grid_x(&self) -> f32 {
        FRAME_INSET + INNER_PAD
    }

    /// 字格区上缘（纵向没有内衬，界行直接顶到内框）。
    pub fn grid_y(&self) -> f32 {
        FRAME_INSET
    }

    /// 字格区宽（= 每列等宽 × 总列数）。
    pub fn grid_w(&self) -> f32 {
        self.cell_w * self.cols as f32
    }

    /// 字格区高（= 版框内高）。
    pub fn grid_h(&self) -> f32 {
        self.h - FRAME_INSET * 2.0
    }

    /// **每一道界行的 x**（相对版框外缘）。
    ///
    /// `k = 0` 是内框左线、`k = cols` 是内框右线，中间 `k = 1..cols-1`
    /// 是列与列之间的乌丝栏。三道式样完全一致（`k × cell_w` 等距）——
    /// 这正是用户要的「每一列都是等宽的」。
    ///
    /// 最左、最右两道由内框承担（见 [`Self::inner_sides`]）：它们比字格
    /// 区还要各外扩一道 `INNER_PAD`，故本函数**只返回列间界行**，免得
    /// 与内框线并成 2px 的黑边。
    pub fn rule_xs(&self) -> Vec<f32> {
        (1..self.cols.max(1))
            .map(|k| self.grid_x() + k as f32 * self.cell_w)
            .collect()
    }
}

/// 把一叶的墨线画成一张**绝对定位**的画布（盖在文字层下面）。
pub(crate) fn leaf_ink_canvas(ink: LeafInk, outer: u32, rule: u32) -> Div {
    let (w, h) = (ink.w, ink.h);
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w(px(w))
        .h(px(h))
        .child(
            canvas(
                move |_b: Bounds<gpui::Pixels>, _w: &mut Window, _cx: &mut App| {},
                move |b: Bounds<gpui::Pixels>, _s: (), window: &mut Window, _cx: &mut App| {
                    let ox = f32::from(b.origin.x);
                    let oy = f32::from(b.origin.y);
                    paint_leaf_ink(ink, ox, oy, outer, rule, window);
                },
            )
            .w(px(w))
            .h(px(h)),
        )
}

/// 画一叶的全部墨线：外框 → 内框 → 界行。
fn paint_leaf_ink(ink: LeafInk, ox: f32, oy: f32, outer: u32, rule: u32, window: &mut Window) {
    let (w, h) = (ink.w, ink.h);
    if w < 4.0 || h < 4.0 {
        return;
    }
    // ① 外框：3px 粗栏，四边等粗（v1.65.0 起不再「横细竖粗」）。
    let s = ink.sides;
    if s[0] {
        rect(window, ox, oy, w, FRAME_OUT, outer);
    }
    if s[2] {
        rect(window, ox, oy + h - FRAME_OUT, w, FRAME_OUT, outer);
    }
    if s[3] {
        rect(window, ox, oy, FRAME_OUT, h, outer);
    }
    if s[1] {
        rect(window, ox + w - FRAME_OUT, oy, FRAME_OUT, h, outer);
    }
    // ② 内框：1px 细线，与界行同粗、同色、同起止——看上去就是「界行围了一圈」。
    let inset = FRAME_OUT + FRAME_PAD;
    let iw = w - inset * 2.0;
    let ih = h - inset * 2.0;
    if iw <= 0.0 || ih <= 0.0 {
        return;
    }
    let si = ink.inner_sides;
    if si[0] {
        rect(window, ox + inset, oy + inset, iw, FRAME_IN, rule);
    }
    if si[2] {
        rect(window, ox + inset, oy + h - inset - FRAME_IN, iw, FRAME_IN, rule);
    }
    if si[3] {
        rect(window, ox + inset, oy + inset, FRAME_IN, ih, rule);
    }
    if si[1] {
        rect(window, ox + w - inset - FRAME_IN, oy + inset, FRAME_IN, ih, rule);
    }
    // ③ 界行（乌丝栏）：**从顶画到底**，跨过地脚叶码带，一道不缺。
    //    刻本的行格是刻在整块版上的——这一叶没写满，栏线照样到底，
    //    绝不在空列处截断（用户「满栏」口径）。
    let y0 = ink.grid_y();
    let y1 = h - ink.grid_y();
    for x in ink.rule_xs() {
        rect(window, ox + x, oy + y0, FRAME_IN, (y1 - y0).max(0.0), rule);
    }
}

// ── 版心（中缝）──────────────────────────────────────────────────────
/// 版心的墨线几何（纯数据）：一条**定宽**的极窄通道。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GutterInk {
    /// 中缝全宽（恒为 `grid::BANXIN_W`）。
    pub w: f32,
    /// 纸面高（与两叶的 sheet 同高）。
    pub h: f32,
    /// 纸面上下留白：天头 / 地脚（与两叶同值，含盈余分配）。
    pub mt: f32,
    pub mb: f32,
    /// 鱼尾宽 / 高、象鼻长（三量同源，见 `banxin`）。
    pub tail_w: f32,
    pub tail_h: f32,
    pub xiangbi: f32,
    /// 版心内芯高（= 一格行高 × 行数 + 地脚叶码条）。
    pub core_h: f32,
    /// 内框是否再压一道 1px（四周双边款式）。
    pub inner_line: bool,
}

impl GutterInk {
    /// 版心内芯（书口字、鱼尾、象鼻的活动范围）的上缘。
    ///
    /// = 天头 + 外框 3 + 框内留边 3 (+ 内框 1)。
    pub fn core_y(&self) -> f32 {
        self.mt + FRAME_OUT + FRAME_PAD + if self.inner_line { FRAME_IN } else { 0.0 }
    }

    /// 上鱼尾的上缘（象鼻末端）。
    pub fn tail_top_y(&self) -> f32 {
        self.core_y() + self.xiangbi
    }

    /// 下鱼尾的上缘（严格垂直镜像：底边贴下象鼻）。
    pub fn tail_bot_y(&self) -> f32 {
        self.core_y() + self.core_h - self.xiangbi - self.tail_h
    }
}

/// 把版心的全部墨线画成一张**绝对定位**的画布。
pub(crate) fn gutter_ink_canvas(
    ink: GutterInk,
    outer: u32,
    rule: u32,
    fish: u32,
    fold: u32,
) -> Div {
    let (w, h) = (ink.w, ink.h);
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .w(px(w))
        .h(px(h))
        .child(
            canvas(
                move |_b: Bounds<gpui::Pixels>, _w: &mut Window, _cx: &mut App| {},
                move |b: Bounds<gpui::Pixels>, _s: (), window: &mut Window, _cx: &mut App| {
                    let ox = f32::from(b.origin.x);
                    let oy = f32::from(b.origin.y);
                    paint_gutter_ink(ink, ox, oy, outer, rule, fish, fold, window);
                },
            )
            .w(px(w))
            .h(px(h)),
        )
}

fn paint_gutter_ink(
    ink: GutterInk,
    ox: f32,
    oy: f32,
    outer: u32,
    rule: u32,
    fish: u32,
    fold: u32,
    window: &mut Window,
) {
    let (w, h) = (ink.w, ink.h);
    if w < 4.0 || h < 8.0 {
        return;
    }
    // ① 上下横框：3px 粗栏，与两叶的外框**同高同粗**，摊开是一整张纸。
    //    左右竖框不画——两叶闭合的版框正好把中缝夹住，再画就成两条并线。
    rect(window, ox, oy + ink.mt, w, FRAME_OUT, outer);
    rect(
        window,
        ox,
        oy + h - ink.mb - FRAME_OUT,
        w,
        FRAME_OUT,
        outer,
    );
    // ② 内框细线（仅「四周双边」款式）。
    if ink.inner_line {
        let iy = ink.mt + FRAME_OUT + FRAME_PAD;
        rect(window, ox, oy + iy, w, FRAME_IN, rule);
        rect(window, ox, oy + h - ink.mb - FRAME_OUT - FRAME_PAD - FRAME_IN, w, FRAME_IN, rule);
    }
    // ③ 象鼻：一根竖线，从版框内缘垂到鱼尾上缘（上下严格镜像）。
    //    线宽 = `XIANG_BI_LINE_W`（与版框外框同粗）。
    let lw = crate::banxin::XIANG_BI_LINE_W;
    let cx = (w - lw) * 0.5;
    let core_y = ink.core_y();
    rect(window, ox + cx, oy + core_y, lw, ink.xiangbi, fish);
    let bot_trunk_y = core_y + ink.core_h - ink.xiangbi;
    rect(window, ox + cx, oy + bot_trunk_y, lw, ink.xiangbi, fish);
    // ④ 两枚黑鱼尾（五点多边形，直线刀法，绝不贝塞尔）。
    paint_fishtail(window, ox, oy + ink.tail_top_y(), ink.tail_w, ink.tail_h, true, fish);
    paint_fishtail(window, ox, oy + ink.tail_bot_y(), ink.tail_w, ink.tail_h, false, fish);
    // ⑤ 折痕：纸对折的那道物理褶，落在版心正中；极细、极淡，只作纸的痕迹。
    let fold_w = crate::banxin::FOLD_W;
    let top = ink.mt + FRAME_OUT + FRAME_PAD + FRAME_IN;
    let len = (ink.h - ink.mb - FRAME_OUT - FRAME_PAD - FRAME_IN - top).max(0.0);
    rect(window, ox + (w - fold_w) * 0.5, oy + top, fold_w, len, fold);
}

/// 一枚黑鱼尾：`banxin::fishtail_points` 给顶点，这里逐点落笔。
fn paint_fishtail(
    window: &mut Window,
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    top: bool,
    color: u32,
) {
    if w < 2.0 || h < 2.0 {
        return;
    }
    let pts = crate::banxin::fishtail_points(top, h);
    let at = |fx: f32, fy: f32| point(px(x0 + fx * w / 100.0), px(y0 + fy));
    let (p1, p2, p3, p4, p5) = (
        at(pts[0].0, pts[0].1),
        at(pts[1].0, pts[1].1),
        at(pts[2].0, pts[2].1),
        at(pts[3].0, pts[3].1),
        at(pts[4].0, pts[4].1),
    );
    let mut pb = PathBuilder::fill();
    pb.move_to(p1);
    pb.line_to(p2);
    pb.line_to(p3);
    pb.line_to(p4);
    pb.line_to(p5);
    pb.line_to(p1);
    if let Ok(path) = pb.build() {
        window.paint_path(path, rgb(color));
    }
}

/// 一根 1px 竖线（界行 / 象鼻）或横线——统一走 `paint_quad(fill(..))`。
fn rect(window: &mut Window, x: f32, y: f32, w: f32, h: f32, color: u32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    window.paint_quad(gpui::fill(
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        },
        rgb(color),
    ));
}

/// 供单测用：本模块只画线，不描边、不填底——边框一律 `paint_quad(fill(..))`。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::BANXIN_W;

    fn ink(cols: usize) -> LeafInk {
        LeafInk {
            w: FRAME_INSET * 2.0 + INNER_PAD * 2.0 + cols as f32 * 37.0,
            h: 600.0,
            cell_w: 37.0,
            cols,
            sides: [true; 4],
            inner_sides: [true; 4],
        }
    }

    /// **每一列都等宽**：相邻两道界行的间距恒为 `cell_w`，一道都不许宽窄不一。
    #[test]
    fn every_rule_is_exactly_one_cell_apart() {
        for cols in [8usize, 10, 12, 14] {
            let k = ink(cols);
            let xs = k.rule_xs();
            assert_eq!(xs.len(), cols - 1, "{cols} 列该有 {} 道列间界行", cols - 1);
            for pair in xs.windows(2) {
                assert!(
                    (pair[1] - pair[0] - k.cell_w).abs() < 1e-4,
                    "{cols} 列：界行间距 {:.2} ≠ 格宽 {:.2}",
                    pair[1] - pair[0],
                    k.cell_w
                );
            }
            // 首道界行 = 字格左缘 + 一格；末道 = 右缘 − 一格：左右严格对称。
            assert!((xs[0] - (k.grid_x() + k.cell_w)).abs() < 1e-4);
            assert!((xs[xs.len() - 1] - (k.grid_x() + k.grid_w() - k.cell_w)).abs() < 1e-4);
        }
    }

    /// **界行从顶画到底**：两端恰好落在内框上，既不短一截、也不越出框外。
    #[test]
    fn rules_span_the_whole_block() {
        let k = ink(12);
        assert!((k.grid_y() - FRAME_INSET).abs() < 1e-6);
        assert!((k.grid_y() + k.grid_h() - (k.h - FRAME_INSET)).abs() < 1e-6);
        // 字格区宽 = 版框宽 − 两层内衬 − 两层框线，一像素不差。
        let want = k.w - (FRAME_INSET + INNER_PAD) * 2.0;
        assert!((k.grid_w() - want).abs() < 1e-4, "字格 {:.1} vs 可用 {:.1}", k.grid_w(), want);
    }

    /// **版心是死的**：32px 定宽，鱼尾不出框、两枚鱼尾严格镜像、象鼻同长。
    #[test]
    fn gutter_geometry_is_fixed_and_symmetric() {
        let tail_w = BANXIN_W + crate::banxin::FISHTAIL_OVERLAP;
        let tail_h = crate::banxin::tail_h_for_width(tail_w);
        let g = GutterInk {
            w: BANXIN_W,
            h: 700.0,
            mt: 120.0,
            mb: 80.0,
            tail_w,
            tail_h,
            xiangbi: crate::banxin::xiangbi_len(tail_h),
            core_h: 486.0,
            inner_line: false,
        };
        // 鱼尾只许在版心内：最宽也就是「中缝 + 两翼各探 1px」。
        assert!(g.tail_w <= g.w + 2.0 + 1e-6, "鱼尾 {} 探出了版心 {}", g.tail_w, g.w);
        // 上鱼尾在象鼻之下，下鱼尾在最下方象鼻之上，中间给书口字留出整块。
        assert!(g.tail_top_y() > g.core_y());
        assert!(g.tail_bot_y() > g.tail_top_y());
        assert!(g.tail_bot_y() + g.tail_h + g.xiangbi <= g.core_y() + g.core_h + 1e-4);
        // 上下严格镜像：两端到内芯边界各自留了同样多的象鼻。
        let top_gap = g.tail_top_y() - g.core_y();
        let bot_gap = (g.core_y() + g.core_h) - (g.tail_bot_y() + g.tail_h);
        assert!((top_gap - bot_gap).abs() < 1e-4, "象鼻上下不等长 {top_gap} / {bot_gap}");
        // 折痕落在版心正中，且整根都在通道内。
        let fx = (g.w - crate::banxin::FOLD_W) * 0.5;
        assert!(fx >= 0.0 && fx + crate::banxin::FOLD_W <= g.w);
    }
}
