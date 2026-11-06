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
    Styled, Window,
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

/// **焊接余量**（v1.9.97）：同色墨线在交汇处各多探 1px。
///
/// `f32 → px` 落到缩放后的半像素上时，相邻两条线各自取整的方向可能相反，
/// 于是接口处裂出一道 1px 白缝（用户口径「漏风」）。多探 1px 由同色墨
/// 盖死：肉眼不可见，几何上却永不断线。⚠️ 只用在**同色**线之间——
/// 异色线（象鼻 / 折痕 与外框）一律靠 [`snap_span`] 的整数栅格精确对接，
/// 不许互相压色，免得留下「多余线头」。
const WELD: f32 = 1.0;

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
pub(crate) fn leaf_ink_canvas(
    ink: LeafInk,
    outer: u32,
    rule: u32,
    grain: f32,
    grain_ink: u32,
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
                    // ① 先铺纸纹（grain=0 时整个函数直接返回），② 再落墨线。
                    //    噪点永远压在文字 / 框线之下，绝不糊字。
                    paint_paper_grain(window, ox, oy, w, h, grain, grain_ink);
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
    //    横竖两端一律走 `frame_rect` 的整数栅格：四角严丝合缝，不会因
    //    f32 → px 的半像素取整裂开 1px（v1.9.97「焊死漏风」）。
    let s = ink.sides;
    if s[0] {
        frame_rect(window, ox, oy, w, FRAME_OUT, outer);
    }
    if s[2] {
        frame_rect(window, ox, oy + h - FRAME_OUT, w, FRAME_OUT, outer);
    }
    if s[3] {
        frame_rect(window, ox, oy, FRAME_OUT, h, outer);
    }
    if s[1] {
        frame_rect(window, ox + w - FRAME_OUT, oy, FRAME_OUT, h, outer);
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
        frame_rect(window, ox + inset, oy + inset, iw, FRAME_IN, rule);
    }
    if si[2] {
        frame_rect(window, ox + inset, oy + h - inset - FRAME_IN, iw, FRAME_IN, rule);
    }
    if si[3] {
        frame_rect(window, ox + inset, oy + inset, FRAME_IN, ih, rule);
    }
    if si[1] {
        frame_rect(window, ox + w - inset - FRAME_IN, oy + inset, FRAME_IN, ih, rule);
    }
    // ③ 界行（乌丝栏）：**从顶画到底**，跨过地脚叶码带，一道不缺。
    //    刻本的行格是刻在整块版上的——这一叶没写满，栏线照样到底，
    //    绝不在空列处截断（用户「满栏」口径）。上下各探 `WELD` 1px 焊进
    //    内框细线（同色，肉眼不可见）；横向 x 一字不动，等宽界行不许被取整。
    let y0 = ink.grid_y() - WELD;
    let y1 = h - ink.grid_y() + WELD;
    for x in ink.rule_xs() {
        weld_y_rect(window, ox + x, oy + y0, FRAME_IN, (y1 - y0).max(0.0), rule);
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

    // ── 版框的**唯一**纵向基准（v1.9.97「焊死漏风」）────────────────
    //
    // 两叶的框高是 `cell_h × rows + foot_h + FRAME_INSET × 2`
    // （见 `reader.rs::render_ancient` 的 `frame_h`）；本结构的 `core_h`
    // 正是「`cell_h × rows + foot_h`」那一段，两者读同一份 `self.grid_rows`
    // / `self.row_h` / `self.size_ancient`，故此处复刻**一字不差**。
    /// 版框高（与两叶同源）：内芯 + 上下两层框线。
    pub fn frame_h(&self) -> f32 {
        self.core_h + FRAME_INSET * 2.0
    }

    /// **版框下沿**（纸面绝对 y）。
    ///
    /// 版心画布原点在纸面顶，两叶画布原点在「纸面顶 + 天头」，因此两叶
    /// 下框的纸面绝对 y 同样是 `mt + frame_h`——这是横框唯一准星。
    ///
    /// ⚠️ **绝不能用 `h - mb`**：那是**内容盒**下沿，不是版框下沿。引擎把
    /// `FRAME_INSET × 2`（14px）算进版框却没算进内容盒，版框本来就比内容盒
    /// 高 14px，两叶一起往下溢出（`reader.rs` 的「只裁横向」）。旧版按内容盒
    /// 给中缝画横框，于是中缝底框比两叶**整整高出 14px**：横线断开、竖框
    /// 凸出，正是用户蓝圈里那个丑陋的十字缺口。
    pub fn frame_bottom(&self) -> f32 {
        self.mt + self.frame_h()
    }

    /// 内容盒下沿（`h - mb`）：**只作画布高度兜底与回归对照，绝不拿来画框线**。
    pub fn content_bottom(&self) -> f32 {
        self.h - self.mb
    }

    /// 墨线画布高（**框原点相对**）：盖住整个版框，也不小于纸面剩余高度。
    ///
    /// 画布与两叶共用同一个原点（见 `gutter_ink_canvas` 的 `.top(ink.mt)`），
    /// 故这里全部按「从框顶算起」给值，不再掺纸面天头。
    pub fn paint_h(&self) -> f32 {
        self.frame_h()
            .max(self.content_bottom() - self.mt)
            .max(self.h - self.mt)
    }

    /// 下外框横线的上缘（与两叶同式：`frame_h − FRAME_OUT`）。
    pub fn outer_bot_y(&self) -> f32 {
        self.frame_bottom() - FRAME_OUT
    }

    /// 下内框细线的上缘（与两叶同式：`frame_h − inset − FRAME_IN`）。
    pub fn inner_bot_y(&self) -> f32 {
        self.frame_bottom() - FRAME_OUT - FRAME_PAD - FRAME_IN
    }

    /// 中缝竖线的**上终点** = 上外框下缘。与横线误差恒为 0，不许悬在留白里。
    pub fn trunk_top(&self) -> f32 {
        self.mt + FRAME_OUT
    }

    /// 中缝竖线的**下终点** = 下外框上缘。与横线误差恒为 0，绝不许凸出横线。
    pub fn trunk_bottom(&self) -> f32 {
        self.outer_bot_y()
    }
}

/// 把版心的全部墨线画成一张**绝对定位**的画布。
pub(crate) fn gutter_ink_canvas(
    ink: GutterInk,
    outer: u32,
    rule: u32,
    fish: u32,
    fold: u32,
    grain: f32,
    grain_ink: u32,
) -> Div {
    let w = ink.w;
    // 画布必须盖住**整个版框**：框底允许比内容盒高 `FRAME_INSET × 2`
    // （两叶一起往下溢出），画布若只有 `ink.h` 高就会把下横框裁掉半截。
    // 它是绝对定位的覆盖层，加高不改变任何一叶的流式布局。
    let paint_h = ink.paint_h();
    div()
        .absolute()
        // ⚠️ **与两叶共用同一个原点**（v1.9.97 二次焊死）：两叶的墨线画布
        // 挂在 `pt(margin_top)` 之下，原点 = 纸面顶 + 天头（经 GPUI 布局取整）。
        // 版心画布若挂在纸面顶，就得自己加 `mt`——`mt` 是引擎给的裸 f32，
        // 与布局取整后的 `mt'` 差不到 1px，却足以让 `frame_rect` 把两条
        // 横线分别 round 到相邻整数，留下 1px 台阶。把画布也压到天头之下，
        // 再在绘制时把 `mt` 减回原点，两条横线的绝对 y 就成了**同一个式子**。
        .top(px(ink.mt))
        .left(px(0.0))
        .w(px(w))
        .h(px(paint_h))
        .child(
            canvas(
                move |_b: Bounds<gpui::Pixels>, _w: &mut Window, _cx: &mut App| {},
                move |b: Bounds<gpui::Pixels>, _s: (), window: &mut Window, _cx: &mut App| {
                    let ox = f32::from(b.origin.x);
                    let canvas_y = f32::from(b.origin.y);
                    // 纸纹用**画布真实原点**，铺满整条版心；不减 mt。
                    paint_paper_grain(window, ox, canvas_y, w, paint_h, grain, grain_ink);
                    // 画布原点已含布局取整后的天头；减掉裸 mt 后，`oy + 纸面绝对 y`
                    // 全部自动映射成「框原点相对」坐标，与两叶逐值同源。
                    // v1.9.97 焊死的 1px 对齐逻辑一字未动。
                    let oy = canvas_y - ink.mt;
                    paint_gutter_ink(ink, ox, oy, outer, rule, fish, fold, window);
                },
            )
            .w(px(w))
            .h(px(paint_h)),
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
    let w = ink.w;
    // 画布高以**版框**为准（见 `gutter_ink_canvas`），内容盒只作兜底。
    let paint_h = ink.paint_h();
    if w < 4.0 || paint_h < 8.0 {
        return;
    }
    // ① 上下横框：3px 粗栏，与两叶的外框**同一 y、同一粗**，摊开是一整张纸。
    //    下框的 y 一律走 `outer_bot_y()`（= 天头 + 与两叶同源的版框高 − 3），
    //    绝不再用 `h − mb`（内容盒），否则整整高出 14px 就成了十字缺口。
    //    左右各探 `WELD` 1px：焊进两叶的竖粗框（同色，看不见接缝）。
    //    左右竖框不画——两叶闭合的版框正好把中缝夹住，再画就成两条并线。
    let bx = ox - WELD;
    let bw = w + 2.0 * WELD;
    frame_rect(window, bx, oy + ink.mt, bw, FRAME_OUT, outer);
    frame_rect(window, bx, oy + ink.outer_bot_y(), bw, FRAME_OUT, outer);
    // ② 内框细线（仅「四周双边」款式），与两叶内框同一 y。
    if ink.inner_line {
        let iy = ink.mt + FRAME_OUT + FRAME_PAD;
        frame_rect(window, bx, oy + iy, bw, FRAME_IN, rule);
        frame_rect(window, bx, oy + ink.inner_bot_y(), bw, FRAME_IN, rule);
    }
    // ③ 象鼻：一根竖线，**上终点 = 上外框下缘、下终点 = 下外框上缘**。
    //    两端与横框走同一个整数栅格，误差恒为 0：既不悬空、也不凸出。
    //    与鱼尾相接处各探 1px（同为鱼尾墨色），免得留下针尖大的白缝。
    //    线宽 = `XIANG_BI_LINE_W`（与版框外框同粗）。
    let lw = crate::banxin::XIANG_BI_LINE_W;
    let cx = (w - lw) * 0.5;
    let t_top = ink.trunk_top();
    let t_bot = ink.trunk_bottom();
    let top_end = ink.tail_top_y() + WELD;
    weld_y_rect(window, ox + cx, oy + t_top, lw, (top_end - t_top).max(0.0), fish);
    let bot_start = ink.tail_bot_y() + ink.tail_h - WELD;
    weld_y_rect(window, ox + cx, oy + bot_start, lw, (t_bot - bot_start).max(0.0), fish);
    // ④ 两枚黑鱼尾（五点多边形，直线刀法，绝不贝塞尔）。
    paint_fishtail(window, ox, oy + ink.tail_top_y(), ink.tail_w, ink.tail_h, true, fish);
    paint_fishtail(window, ox, oy + ink.tail_bot_y(), ink.tail_w, ink.tail_h, false, fish);
    // ⑤ 折痕：纸对折的那道物理褶，落在版心正中；极细、极淡，只作纸的痕迹。
    //    同样两端焊在上下横框上，绝不出现「横线断、竖线凸」的断头。
    let fold_w = crate::banxin::FOLD_W;
    weld_y_rect(
        window,
        ox + (w - fold_w) * 0.5,
        oy + t_top,
        fold_w,
        (t_bot - t_top).max(0.0),
        fold,
    );
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

/// 把一条墨线的两端吸附到**整数逻辑像素**，返回 `(起点, 长度)`。
///
/// GPUI 的 `px(f32)` 缩放后可能落在半像素上，横框与竖框各自取整的方向
/// 一旦相反，接口就裂 1px 白缝。所有框线共用这一个栅格后，横线端点与
/// 竖线端点的 Y 坐标**误差恒为 0**——这是「焊死」的数值保证。
fn snap_span(start: f32, len: f32) -> (f32, f32) {
    let a = start.round();
    let b = (start + len).round();
    (a, (b - a).max(0.0))
}

/// 框线专用：横、竖两端都吸附整数栅格（外粗框 / 内细框 / 四角）。
fn frame_rect(window: &mut Window, x: f32, y: f32, w: f32, h: f32, color: u32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (x0, ww) = snap_span(x, w);
    let (y0, hh) = snap_span(y, h);
    if ww <= 0.0 || hh <= 0.0 {
        return;
    }
    rect(window, x0, y0, ww, hh, color);
}

/// 细线专用：**只吸附 Y**，横向 x / w 原样保留。
///
/// 界行必须保持 `k × cell_w` 的严格等宽，折痕只有 0.5px——横向取整会把
/// 它们整成宽窄不一或 1px 的粗线。纵向端点仍与框线同一栅格，故交汇处
/// 严丝合缝。
fn weld_y_rect(window: &mut Window, x: f32, y: f32, w: f32, h: f32, color: u32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (y0, hh) = snap_span(y, h);
    if hh <= 0.0 {
        return;
    }
    rect(window, x, y0, w, hh, color);
}

/// **陈年纸的烘焙噪点**：在墨线之前铺一层极淡的深浅颗粒。
///
/// 纯确定性 LCG（不引随机数依赖、不随帧抖动、不缓存位图），整数栅格对齐；
/// 每叶最多 `MAX_DOTS` 枚点，大纸面也不会拖慢翻页。`grain` 是 0~1 的强度
/// （宋本泛黄 = 0.10），0 表示这套纸不要纹理——其余七套走的就是这条短路。
fn paint_paper_grain(
    window: &mut Window,
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
    grain: f32,
    ink: u32,
) {
    const STEP: f32 = 9.0;
    const MAX_DOTS: usize = 900;
    if grain <= 0.0 || w < 16.0 || h < 16.0 {
        return;
    }
    let alpha = (grain * 110.0).clamp(3.0, 40.0) as u32; // 0.10 → 11/255
    let dark = ((ink & 0x00FF_FFFF) << 8) | alpha;
    let light = (0x00FF_FFFF << 8) | alpha;
    let mut seed: u32 = 0x9E37_79B9;
    let mut dots = 0usize;
    let mut y = 2.0;
    while y < h - 2.0 {
        let mut x = 2.0;
        while x < w - 2.0 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let r = seed >> 16;
            if r % 6 == 0 {
                let color = if r & 1 == 0 { dark } else { light };
                window.paint_quad(gpui::fill(
                    Bounds {
                        origin: point(px((ox + x).floor()), px((oy + y).floor())),
                        size: size(px(1.0), px(1.0)),
                    },
                    gpui::rgba(color),
                ));
                dots += 1;
                if dots >= MAX_DOTS {
                    return;
                }
            }
            x += STEP;
        }
        y += STEP;
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

    fn gutter_ink(content_h: f32, mt: f32, mb: f32, core_h: f32, inner_line: bool) -> GutterInk {
        let tail_w = BANXIN_W + crate::banxin::FISHTAIL_OVERLAP;
        let tail_h = crate::banxin::tail_h_for_width(tail_w);
        GutterInk {
            w: BANXIN_W,
            h: content_h,
            mt,
            mb,
            tail_w,
            tail_h,
            xiangbi: crate::banxin::xiangbi_len(tail_h),
            core_h,
            inner_line,
        }
    }

    /// **焊死漏风**（v1.9.97）：中缝下横框必须与两叶下横框**同一 y**。
    ///
    /// 现场还原：引擎把 `FRAME_INSET × 2` 算进版框却没算进内容盒，旧版
    /// 中缝按 `h − mb` 画横框，比两叶高出整整 14px —— 于是横线断开、
    /// 竖框凸出，成了用户蓝圈里的十字缺口。
    #[test]
    fn gutter_bottom_frame_welds_to_leaf_bottom_frame() {
        let content_h = 900.0;
        let mt = 180.0;
        let mb = 120.0;
        // grid 引擎：inner_h = content_h − mt − mb − foot_h，core_h = inner_h + foot_h
        // 两处 foot_h 相消，故 core_h 恰是内容高扣掉天头地脚。
        let core_h = content_h - mt - mb;
        let g = gutter_ink(content_h, mt, mb, core_h, true);
        // render_ancient 的 frame_h = cell_h × rows + foot_h + FRAME_INSET × 2
        let leaf_h = core_h + FRAME_INSET * 2.0;
        let leaf_bot_bar_y = mt + leaf_h - FRAME_OUT;
        assert!(
            (g.outer_bot_y() - leaf_bot_bar_y).abs() < 1e-6,
            "中缝下框 {:.3} 与两叶下框 {:.3} 必须同一 y",
            g.outer_bot_y(),
            leaf_bot_bar_y
        );
        assert!(
            (g.inner_bot_y() - (leaf_bot_bar_y - FRAME_PAD - FRAME_IN)).abs() < 1e-6,
            "中缝下内框细线也必须与两叶对齐"
        );
        // 旧口径（内容盒）确实整整差 14px：把「漏风」的根因钉死在测试里。
        let old_y = g.content_bottom() - FRAME_OUT;
        assert!(
            (leaf_bot_bar_y - old_y - FRAME_INSET * 2.0).abs() < 1e-6,
            "内容盒口径与版框口径之差必须恰是两层框线"
        );
        // 画布（框原点相对）必须盖得住整块版框，否则下横框会被裁掉半截。
        assert!(g.paint_h() >= g.frame_h());
        // 画布原点与两叶同在天头之下：框底换算成框原点相对，恰是 frame_h。
        assert!((g.frame_bottom() - g.mt - g.frame_h()).abs() < 1e-6);
    }

    /// 中缝竖线的 Y 轴终点与横框**误差为 0**，且端点全部落在整数栅格上。
    #[test]
    fn gutter_verticals_end_exactly_on_horizontal_frame() {
        let g = gutter_ink(880.0, 160.0, 106.0, 614.0, false);
        assert!((g.trunk_top() - (g.mt + FRAME_OUT)).abs() < 1e-6);
        assert!((g.trunk_bottom() - g.outer_bot_y()).abs() < 1e-6);
        // 同一栅格：竖线的下终点，与下横框的上起点取整后是同一个整数。
        assert_eq!(
            snap_span(g.trunk_bottom(), 0.0).0,
            snap_span(g.outer_bot_y(), FRAME_OUT).0,
        );
        // 所有框线端点都吸附成整数，f32 半像素缝不可能出现。
        for (start, len) in [
            (g.mt, FRAME_OUT),
            (g.outer_bot_y(), FRAME_OUT),
            (g.inner_bot_y(), FRAME_IN),
            (g.trunk_top(), 1.0),
            (g.trunk_bottom(), 0.0),
        ] {
            let (a, l) = snap_span(start, len);
            assert_eq!(a, a.round(), "起点必须落在整数像素");
            assert_eq!(a + l, (a + l).round(), "终点必须落在整数像素");
        }
    }
}
