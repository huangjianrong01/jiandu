//! 句读（标点）排版：古籍里的句读是**朱砂手绘的小记号**——极小、偏居一格
//! 之右下、色暗而沉。
//!
//! 刻本的句读绝不是「缩小的正字」，更不是现代印刷体的 `，。` 字形：
//! 字库自带的标点在极小字号下会缩成一枚「奇怪的黑痣」（v1.30.0 用户原话），
//! 故本模块**严禁调用字体的句读字形**——所有标点一律在渲染前拦截，用
//! `paint_path` 纯手绘（见 [`paint_mark`]）：
//!
//! ```text
//!   ┌─────────┐          ┌─────────┐
//!   │         │          │         │
//!   │   道    │    →     │   道    │
//!   │         │          │      ○  │   ← 句号：细线空心小圈，贴在格右下
//!   │  ﹀     │          └─────────┘
//!   └─────────┘
//! ```
//!
//! * **直径** = 正文字号 × 0.35（[`FS_RATIO`]）——小到只是一粒朱砂记号；
//! * **落点** = 字格的 70% × 70%（[`ANCHOR_X`] / [`ANCHOR_Y`]）——右下角，
//!   绝不居中、绝不与正文争格心；
//! * **形制**：句类（。！？…）画**细线空心圈**（[`PunctKind::Dot`]），
//!   读类（，、；：—）画**向左下倾斜的微小弧线**（[`PunctKind::Arc`]）；
//! * **墨色** = 暗朱砂 / 熟褐 [`PUNCT_INK`]（`#9A3B3B`）——深沉、不鲜艳，
//!   深底主题由渲染层向纸色提亮（见 `reader::punct_ink`）。
//!
//! 竖排里「右」是上一列那一侧、「下」是读序前进的方向，故右下角正是刻本
//! 句读的位置：它跟着读序走在前一字的**行进前方**贴边，不挡字、不占位。
//!
//! 另有一事：[`place`] 只管几何（纯函数、有单测），不管颜色与绘制；
//! 弯引号 → 直角引号的替换在 [`crate::types::bracket_to_quote`]，
//! 悬挂 / 挤压在 [`crate::gloss::layout_book_from_opts`]。

use gpui::{PathBuilder, Rgba, Window, point, px, rgb};

/// 句读直径 / 正文字号：**0.35**（v1.34.0 用户定「极小，像个手点的小记号」）。
///
/// 0.4 起步、0.35 收敛：再小（0.3 以下）在 12 列窄格里就只剩一粒灰屑；
/// 再大（0.5 以上）开始像一颗正字、抢格心。0.35 配 0.22 的线宽，
/// 空心圈内还剩得下一多半透亮，才读得出是「圈」不是「饼」。
pub const FS_RATIO: f32 = 0.35;
/// 句读落点 / 字格宽：记号**中心**落在格宽的 70% 处——右下象限，不居中。
pub const ANCHOR_X: f32 = 0.70;
/// 句读落点 / 字格高：记号**中心**落在格高的 70% 处。
pub const ANCHOR_Y: f32 = 0.70;
/// 线宽 / 记号直径：空心圈与弧线的粗细。
///
/// 0.22 是「细而不虚」的一档：圈径 7.7px 时线宽 1.7px，内孔仍有 4.3px，
/// 放大看是一笔写出来的小圈，不是填充的饼。
pub const STROKE_OF_D: f32 = 0.22;
/// 线宽下限（px）：再细就在纸上看不见了。
pub const STROKE_MIN: f32 = 0.8;
/// 线宽上限（px）：正文再大，句读也不许粗成一条墨虫。
pub const STROKE_MAX: f32 = 2.2;
/// 记号直径下限（px）：正文再小（行款兜底锁 12px），句读仍是一粒看得出的朱点。
pub const D_MIN: f32 = 3.5;

/// 句读墨色：**暗朱砂 / 熟褐 `#9A3B3B`**（v1.34.0 用户定「深沉、不要太鲜艳」）。
///
/// 比印章的亮朱砂（`CINNABAR` `#C3272D`）沉一档——句读是读书的痕迹，
/// 该像退了火的泥金，不像刚盖的印。深底主题（夜读）下由渲染层向纸色
/// 提亮（`reader::punct_ink`），不会沉进深纸里。
pub const PUNCT_INK: u32 = 0x9A3B3B;

/// 书名号（`︽ ︾`）/ 正文字号：**0.7 倍**（v1.30.0 用户定；原 0.8）。
///
/// 书名号不是句读——它包着一整串书名，0.3 倍会小到看不出在「括」什么；
/// 但也不该与正文齐平（刻本的书名号总比正文矮一头），更不许撑满格子：
/// 它附在相邻字的格右上角（见 `GlyphKind::Attach`），70% 才既看得清
/// 又不抢正文的格心。
pub const TITLE_FS_RATIO: f32 = 0.7;

/// 句读的**形制**：手绘哪种记号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PunctKind {
    /// 「句」类（。！？…）：**细线空心小圈**——一枚极小的、线条极细的圆。
    Dot,
    /// 「读」类（，、；：—）：**向左下倾斜的微小弧线**。
    Arc,
}

/// 一枚句读的手绘坐标（纯几何，渲染层照抄即可；单位 px，相对字格左上角）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PunctMark {
    /// 形制（空心圈 / 弧线）。
    pub kind: PunctKind,
    /// 记号直径（px）：圈的外径、弧线的跨度都按它取。
    pub d: f32,
    /// 记号**中心**的横向坐标（px，相对字格左上角）。
    pub cx: f32,
    /// 记号**中心**的纵向坐标（px，相对字格左上角）。
    pub cy: f32,
    /// 手绘线宽（px）。
    pub stroke: f32,
}

/// 字符 → 句读形制：读类（逗号 / 顿号 / 冒号 / 分号 / 省略号 / 破折号）
/// 画弧线，其余句读（句号 / 问号 / 叹号）画空心圈。
///
/// 分类口径与 `types::is_comma_punct` 同源：`is_punct` 集合内、非
/// `is_stop_punct` 的都算「读」。
pub fn kind_of(ch: char) -> PunctKind {
    if crate::types::is_comma_punct(ch) {
        PunctKind::Arc
    } else {
        PunctKind::Dot
    }
}

/// 句读落位：**直径 × 0.35、中心钉在字格 70% × 70% 的右下角**。
///
/// ```text
/// d      = max(正文 × 0.35, D_MIN)     再夹进格内（≤ 格宽 / 格高的一半）
/// stroke = clamp(d × 0.22, 0.8, 2.2)
/// cx     = 格宽 × 0.70                 （右下象限，绝不居中）
/// cy     = 格高 × 0.70
/// cx/cy  再夹紧：整个记号（含线宽）不探出字格——竖排右邻是上一列，
///          压过去就是串行。
/// ```
///
/// `d` 封顶到格宽 / 格高的一半：即便正文大到离谱（或格子窄到离谱），
/// 记号也不会探出字格、压到邻列。极端小格（比 `D_MIN` 还小）时随格缩，
/// 不硬撑出一个比格还大的圈。
pub fn place(fs_body: f32, cell_w: f32, cell_h: f32, kind: PunctKind) -> PunctMark {
    let room = (cell_w * 0.5).min(cell_h * 0.5).max(1.0);
    let d = (fs_body * FS_RATIO).max(D_MIN).min(room);
    let stroke = (d * STROKE_OF_D).clamp(STROKE_MIN, STROKE_MAX);
    // 中心先取 70% × 70%（右下象限），再按「中心 ± (d/2 + stroke) 不出格」夹紧。
    let half = d * 0.5 + stroke;
    let cx = (cell_w * ANCHOR_X).clamp(half, (cell_w - half).max(half));
    let cy = (cell_h * ANCHOR_Y).clamp(half, (cell_h - half).max(half));
    PunctMark {
        kind,
        d,
        cx,
        cy,
        stroke,
    }
}

/// **手绘**一枚句读：纯 `paint_path`，绝不调用字体的 `，。` 字形。
///
/// `cx` / `cy` 是记号中心在**窗口**里的坐标（调用方把字格原点加好）。
/// 两种形制：
///
/// * [`PunctKind::Dot`] —— 四段二次贝塞尔凑一个圆（[`CIRCLE_K`] 是凑圆
///   的控制点系数），`PathBuilder::stroke` 细线描边，**空心**；
/// * [`PunctKind::Arc`] —— 一段二次贝塞尔弧，从右上往左下走、弧腹朝左，
///   同一支细笔描出来。
///
/// 全程只读传入的几何与颜色，不碰任何状态——canvas 的 paint 闭包签名
/// （`&mut Window`）也从类型上保证了「绘制不改状态」。
pub fn paint_mark(window: &mut Window, cx: f32, cy: f32, m: PunctMark, ink: u32) {
    let color = rgb(ink);
    match m.kind {
        PunctKind::Dot => paint_ring(window, cx, cy, m, color),
        PunctKind::Arc => paint_arc(window, cx, cy, m, color),
    }
}

/// 二次贝塞尔凑圆的控制点系数：四段二次曲线画一个圆，控制点落在
/// 「半径 × CIRCLE_K」的对角位上（t=0.5 处与真圆误差 < 2%，肉眼是圆的）。
const CIRCLE_K: f32 = 0.9142;

/// 空心小圈（句号）：四段二次贝塞尔 + 细线描边。
fn paint_ring(window: &mut Window, cx: f32, cy: f32, m: PunctMark, color: Rgba) {
    let r = m.d * 0.5;
    let k = r * CIRCLE_K;
    let mut b = PathBuilder::stroke(px(m.stroke));
    // 从最右点起笔，顺时针（屏幕坐标 y 向下）走四段quarter arc。
    b.move_to(point(px(cx + r), px(cy)));
    b.curve_to(point(px(cx), px(cy + r)), point(px(cx + k), px(cy + k)));
    b.curve_to(point(px(cx - r), px(cy)), point(px(cx - k), px(cy + k)));
    b.curve_to(point(px(cx), px(cy - r)), point(px(cx - k), px(cy - k)));
    b.curve_to(point(px(cx + r), px(cy)), point(px(cx + k), px(cy - k)));
    b.close();
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// 微小弧线（逗号）：从右上往左下走的一段二次贝塞尔，弧腹朝左——
/// 像一笔写下去、收在左下的读点。起落点都在记号直径之内，绝不探出格。
fn paint_arc(window: &mut Window, cx: f32, cy: f32, m: PunctMark, color: Rgba) {
    let d = m.d;
    let mut b = PathBuilder::stroke(px(m.stroke));
    // 起笔右上 → 控制点居中偏左 → 收笔左下：弧线整体「向左下倾斜」。
    b.move_to(point(px(cx + 0.34 * d), px(cy - 0.42 * d)));
    b.curve_to(point(px(cx - 0.26 * d), px(cy + 0.44 * d)), point(px(cx - 0.30 * d), px(cy - 0.10 * d)));
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 引擎真实格子：行款模型下格宽 = 格高 = 行高（字格取方）。
    fn cell(fs: f32) -> (f32, f32) {
        (fs / 0.85, fs / 0.85)
    }

    #[test]
    fn diameter_is_35_percent_of_the_body() {
        // 常用字号段里严格是 0.35 倍（只是个记号，不是缩小的正字）。
        for fs in [12.0, 17.0, 21.9, 24.0, 34.0, 44.0] {
            let (cw, ch) = cell(fs);
            let m = place(fs, cw, ch, PunctKind::Dot);
            assert!(
                (m.d - fs * FS_RATIO).abs() < 0.01,
                "直径应为 0.35 倍正文：{:.2} vs {:.2}",
                m.d,
                fs * FS_RATIO
            );
        }
        // 句号约正字三分之一高：一眼是「点」不是「字」。
        let m = place(34.0, 40.0, 44.0, PunctKind::Dot);
        assert!(m.d < 34.0 * 0.4, "句读仍偏大：{}", m.d);
    }

    #[test]
    fn stroke_is_thin_but_visible() {
        for fs in [12.0, 21.9, 34.0, 60.0] {
            let (cw, ch) = cell(fs);
            let m = place(fs, cw, ch, PunctKind::Dot);
            assert!(m.stroke >= STROKE_MIN - 1e-6, "线细没了：{}", m.stroke);
            assert!(m.stroke <= STROKE_MAX + 1e-6, "线粗成虫：{}", m.stroke);
            // 空心圈内孔要剩得下一半以上（读得出是圈不是饼）。
            assert!(m.d - 2.0 * m.stroke > m.d * 0.4, "圈被线填满了：{m:?}");
        }
    }

    #[test]
    fn center_sits_at_the_bottom_right_never_centered() {
        let m = place(34.0, 40.0, 44.0, PunctKind::Dot);
        assert!((m.cx - 40.0 * ANCHOR_X).abs() < 0.01, "横向落点错：{}", m.cx);
        assert!((m.cy - 44.0 * ANCHOR_Y).abs() < 0.01, "纵向落点错：{}", m.cy);
        // 「绝不能占据格子的正中心」：中心明显偏在右下象限。
        assert!(m.cx > 40.0 * 0.55, "太靠左（近乎居中）：{}", m.cx);
        assert!(m.cy > 44.0 * 0.55, "太靠上（近乎居中）：{}", m.cy);
    }

    #[test]
    fn mark_never_spills_out_of_its_cell() {
        // 任意字号 × 任意格子：整个记号（含线宽）都留在格内。
        for fs in [6.0, 12.0, 17.0, 24.0, 34.0, 60.0, 96.0] {
            for (cw, ch) in [(16.0, 18.0), (24.0, 26.0), (40.0, 44.0), (80.0, 88.0)] {
                for kind in [PunctKind::Dot, PunctKind::Arc] {
                    let m = place(fs, cw, ch, kind);
                    let lo = m.stroke * 0.5;
                    let hi_x = m.cx + m.d * 0.5 + lo;
                    let hi_y = m.cy + m.d * 0.5 + lo;
                    assert!(m.cx - m.d * 0.5 - lo >= -0.01, "左出格：{m:?}");
                    assert!(m.cy - m.d * 0.5 - lo >= -0.01, "上出格：{m:?}");
                    assert!(hi_x <= cw + 0.01, "右出格：{m:?} / 格宽 {cw}");
                    assert!(hi_y <= ch + 0.01, "下出格：{m:?} / 格高 {ch}");
                }
            }
        }
    }

    #[test]
    fn tiny_cell_shrinks_the_mark_with_it() {
        // 格子比 D_MIN 还小：随格缩，不硬撑。
        let m = place(34.0, 8.0, 9.0, PunctKind::Dot);
        assert!(m.d <= 8.0 * 0.5 + 0.01, "小格里仍撑破：{m:?}");
        // 行款兜底锁 12px 正文字号时，句读仍有一粒看得出的朱点。
        let m = place(12.0, 14.1, 14.1, PunctKind::Dot);
        assert!(m.d >= D_MIN - 0.01, "兜底字号下句读消失了：{m:?}");
    }

    /// Invariant 3（句读永不被边框盖住 / 重置）的几何一侧：
    /// **全参数空间**扫描——无论字号、格子、形制怎么配，记号中心都钉在
    /// 右下象限（> 格心）、整个记号都留在格内。
    ///
    /// 为什么这条是「不被盖住」的前提：版框（外粗框 / 内细框 / 乌丝栏）
    /// 全在字格**之外**，记号只要不出格就与任何边框物理不相交；
    /// 一旦哪天把落点改回「居中」或让它溢出格外，这条测试就红——
    /// 那是句读被界行 / 版框裁掉的先兆（v1.34.0 用户点名「千万不能
    /// 被边框的渲染循环盖住或重置」，渲染顺序的保证见
    /// `reader::punct_overlay` 的注释）。
    #[test]
    fn mark_stays_in_the_bottom_right_quadrant_across_the_whole_space() {
        for fs in [6.0, 12.0, 17.0, 21.9, 24.0, 34.0, 48.0, 96.0] {
            for (cw, ch) in [
                (10.0, 11.0),
                (14.1, 14.1),
                (24.0, 26.0),
                (31.0, 34.0),
                (40.0, 44.0),
                (80.0, 88.0),
                (200.0, 120.0), // 极端扁格 / 极端瘦格都要活
            ] {
                for kind in [PunctKind::Dot, PunctKind::Arc] {
                    let m = place(fs, cw, ch, kind);
                    // 右下象限：中心严格偏过格心（用户「不能居中」）。
                    assert!(
                        m.cx > cw * 0.5 && m.cy > ch * 0.5,
                        "落点漂回格心了：{m:?} / 格 {cw}×{ch}"
                    );
                    // 格内：整个记号（含线宽）四边都不出格。
                    let half = m.d * 0.5 + m.stroke * 0.5;
                    assert!(m.cx - half >= -0.01, "左出格：{m:?}");
                    assert!(m.cy - half >= -0.01, "上出格：{m:?}");
                    assert!(m.cx + half <= cw + 0.01, "右出格：{m:?} / 格宽 {cw}");
                    assert!(m.cy + half <= ch + 0.01, "下出格：{m:?} / 格高 {ch}");
                    // 极小：直径永远小于格宽格高（是个记号，不是个字）。
                    assert!(m.d < cw && m.d < ch, "记号大过格子：{m:?}");
                }
            }
        }
    }

    #[test]
    // 断言的都是常量之间的关系（编译期就定死），钉的是「配色不许改艳」这条
    // 设计约束，不是运行时状态——放行 clippy 的常量断言告警。
    #[allow(clippy::assertions_on_constants)]
    fn ink_is_the_dead_cinnabar_the_user_asked_for() {
        // 暗朱砂 #9A3B3B：深沉、不鲜艳（比亮朱砂 C3272D 沉一档）。
        assert_eq!(PUNCT_INK, 0x9A3B3B);
        assert!(PUNCT_INK < 0xC3272D, "句读色不该比印泥还艳");
        // 红色分量主导（朱砂调），但绿蓝也不低（熟褐感，不是正红）。
        let (r, g, b) = (0x9A, 0x3B, 0x3B);
        assert!(r > g && g == b, "应是偏褐的暗朱砂，不是正红");
    }

    #[test]
    fn kind_follows_stop_vs_comma() {
        // 句类画空心圈，读类画弧线。
        for c in ['。', '！', '？', '…', '.'] {
            assert_eq!(kind_of(c), PunctKind::Dot, "{c} 应画空心圈");
        }
        for c in ['，', '、', '；', '：', '—'] {
            assert_eq!(kind_of(c), PunctKind::Arc, "{c} 应画弧线");
        }
    }

    /// 目检用：`cargo test dump_punct -- --ignored --nocapture`。
    ///
    /// 打印常用字号下句读的直径、线宽与落点——一眼看出「0.35 倍、
    /// 钉在右下角、细线空心」。
    #[test]
    #[ignore]
    fn dump_punct_mark() {
        for fs in [12.0f32, 17.0, 21.9, 24.0, 34.0, 60.0] {
            let cw = fs / crate::grid::FS_OF_ROW;
            let ch = cw;
            for kind in [PunctKind::Dot, PunctKind::Arc] {
                let m = place(fs, cw, ch, kind);
                println!(
                    "正文 {fs:>5.1} → {kind:?} 径 {:>5.2}（{:.3}×正文）线宽 {:>4.2} | 格 {:>5.1}×{:>5.1} → 中心 ({:>5.2}, {:>5.2})",
                    m.d, m.d / fs, m.stroke, cw, ch, m.cx, m.cy
                );
            }
        }
    }
}
