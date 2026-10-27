//! 句读（标点）排版：古籍里的句读是**手写的小记号**——极小、偏居一格
//! 之右上、与正文同墨。
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
//!   │         │          │    ◢    │   ← 读点：手绘「瓜子点」，落在格右上
//!   │  ﹀     │          └─────────┘
//!   └─────────┘
//! ```
//!
//! * **落点** = 字格的 72% × 28%（[`ANCHOR_X`] / [`ANCHOR_Y`]）——**右上角**，
//!   正是竖排句读的规范位（上一字在上方，句读贴它的下沿、偏右），
//!   瓜子点再向左 / 向下各让 [`SEED_NUDGE_PX`]，悬着不对齐、不贴死前一字；
//! * **形制**：读类（，、；：—）画**实心瓜子点**（[`PunctKind::Seed`]）：
//!   **肥头在右上、细尾扫向左下**（顺时针斜 [`SEED_TILT_DEG`] 度），是运笔
//!   一顿的笔势，不是左右对称的梭形；句类（。！？…）画**细线空心圈**
//!   （[`PunctKind::Dot`]）；两者一律 `paint_path` 手绘，绝不取字体字形；
//! * **大小**：瓜子点长 = 字号 × 0.54（[`SEED_H_RATIO`]）、宽 = 长的 0.60，
//!   圈径 = 字号 × 0.35（[`FS_RATIO`]）——都是一粒记号，不是缩小的正字；
//! * **墨色** = **朱砂红**（`palette.note`，与夹注同源）——形状与位置是新制的，
//!   颜色照旧不动（用户点名「还是原来的颜色」）。
//!
//! 竖排里「上」是上一字那一侧、「右」是版心外缘，故**右上角**正是刻本句读
//! 的位置：记号贴在上一字的下沿、偏右悬着，不挡字、不占位。
//!
//! 另有一事：[`place`] 只管几何（纯函数、有单测），不管颜色与绘制；
//! 弯引号 → 直角引号的替换在 [`crate::types::bracket_to_quote`]，
//! 悬挂 / 挤压在 [`crate::gloss::layout_book_from_opts`]。

use gpui::{PathBuilder, Pixels, Point, Rgba, Window, point, px, rgb};

/// 句读直径 / 正文字号：**0.35**（v1.34.0 用户定「极小，像个手点的小记号」）。
///
/// 现在只作用于「句」类的空心圈（[`PunctKind::Dot`]）；读点走
/// [`SEED_H_RATIO`]（瓜子点，另有一组比例）。
pub const FS_RATIO: f32 = 0.35;
/// **瓜子点的长 / 正文字号**（v1.67.2）：0.50。
///
/// 一路改上来的：0.28 → 0.32 → 0.40 → 0.42 都还嫌小。实测 12 列行款下
/// 字号约 17pt，0.42 只有约 7pt 的一小粒，放大看仍是红色碎屑，且与同页
/// 的句号（0.35 倍、空心圈）轻重失衡——逗号显得又小又细（真机观感
/// 「不够灵动」）。0.50 在 17pt 上约 8.5pt，仍是字格的一半不到，但「露锋—
/// 顿肚—出锋」这一整套笔势终于撑得起来，与句号也协调了。
pub const SEED_H_RATIO: f32 = 0.54;
/// 瓜子点的**宽 / 长**：0.60——饱满但不臃肿（再瘦像根针，再胖像颗豆）。
///
/// 0.54 × 0.60 的实心面积已接近同页句号（0.35 倍空心圈）的视觉分量，
/// 二者轻重协调；配合 [`SEED_TILT_DEG`] 的 20° 倾角与 asymmetric 锚点，
/// 肚子在右上、尾扫左下，小字号下既有存在感又不抢字格。
pub const SEED_W_OF_H: f32 = 0.60;
/// 瓜子点的最小长（px）：行款兜底锁到 12px 正文时，它仍得是一粒看得见的墨点。
pub const SEED_MIN: f32 = 3.2;
/// 瓜子点的**笔势倾角**（度，顺时针）：20°。
///
/// 这是「瓜子」与「梭形」的分水岭：不倾的时候，形状是左右对称的两头尖，
/// 落地成一颗小菱形（v1.66.0 的真机观感）；顺时针倾 20° 后，起笔抬到
/// **右上**、出锋扫到**左下**，正是灵飞经小楷里那一「点」的走向。
pub const SEED_TILT_DEG: f32 = 20.0;
/// 瓜子点的**悬空微调**（px）：落在格右上后再**向左、向下**各让这么多——
/// 竖排里上一字在上方，向下即离开它；向左即离开版心右缘。1px 的错位
/// 足以让记号「浮」在字外，又不会看着像跑偏。
pub const SEED_NUDGE_PX: f32 = 1.0;
/// 句读落点 / 字格宽：记号**中心**落在格宽的 72% 处——右上象限，不居中。
pub const ANCHOR_X: f32 = 0.72;
/// 句读落点 / 字格高：**28%**——竖排句读的规范位在**左上偏上**那一角
/// （上一字在上方，记号贴在它的下沿）。v1.34.0 的 70%（右下）是把横排
/// 习惯搬到竖排，读起来像「掉在字底下」，v1.66.0 改回右上。
pub const ANCHOR_Y: f32 = 0.28;
/// **悬挂**句读的落点 / 字格宽（v1.59.0）：比常规更靠右，贴到字格右缘。
///
/// 常规句读独占一格，落在格右上；悬挂句读是**挤进末字那一格**的，
/// 它和末字共格——若仍落 70%，记号会压在末字的字身上，看着像「叠字」而不是
/// 「悬挂」。推到 0.86，记号贴着字格右缘挂出去，与末字的字身在 X 上错开，
/// 才是刻本里那枚「挂」在字旁的手写读点。
pub const HANG_ANCHOR_X: f32 = 0.86;
/// **悬挂**句读的落点 / 字格高：比常规更靠下，沉到末字的脚下。
pub const HANG_ANCHOR_Y: f32 = 0.80;
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
/// 逗号（读点）的**悬空偏移**（px）：旧制（落格右下时）向上挪 1.5px 用。
/// v1.66.0 起读点改落格右上，悬空改由 [`SEED_NUDGE_PX`] 统管，本常量留档。
pub const COMMA_FLOAT_PX: f32 = 1.5;

/// 句读墨色：**暗朱砂 / 熟褐 `#9A3B3B`**（v1.34.0 定的旧色）。
///
/// **v1.66.0 起退役**：用户口径「颜色跟正文颜色保持一致」，渲染层改为直接
/// 传 `palette.ink`（见 `reader::punct_ink`）。常量留档并仍被一条单测钉着，
/// 只为记录「曾经是朱砂」这件事，不再参与绘制。
#[allow(dead_code)]
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
    /// 「读」类（，、；：—）：**实心瓜子点**——肥头在右上、细尾扫向左下，
    /// 顺时针斜 16°，是运笔一顿的笔势（v1.67.0，替掉旧的水滴与细弧）。
    Seed,
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
/// 画瓜子点，其余句读（句号 / 问号 / 叹号）画空心圈。
///
/// 分类口径与 `types::is_comma_punct` 同源：`is_punct` 集合内、非
/// `is_stop_punct` 的都算「读」。
pub fn kind_of(ch: char) -> PunctKind {
    if crate::types::is_comma_punct(ch) {
        PunctKind::Seed
    } else {
        PunctKind::Dot
    }
}

/// 句读落位：**瓜子点长 × 0.50、中心钉在字格 72% × 28% 的右上角**。
///
/// ```text
/// 读类（瓜子点）：d = max(正文 × 0.54, SEED_MIN)，宽 = d × 0.60，顺时针斜 20°
/// 句类（空心圈）：d = max(正文 × 0.35, D_MIN)
/// cx = 格宽 × 0.72  − 1px（读类）    （右上象限，绝不居中）
/// cy = 格高 × 0.28  + 1px（读类）
/// cx/cy 再夹紧：整个记号（含线宽）不探出字格——竖排右邻是上一列，
///        压过去就是串行。
/// ```
///
/// `d` 封顶到格宽 / 格高的一半：即便正文大到离谱（或格子窄到离谱），
/// 记号也不会探出字格、压到邻列。极端小格（比 `D_MIN` 还小）时随格缩，
/// 不硬撑出一个比格还大的记号。
pub fn place(fs_body: f32, cell_w: f32, cell_h: f32, kind: PunctKind) -> PunctMark {
    let room = (cell_w * 0.5).min(cell_h * 0.5).max(1.0);
    let d = match kind {
        PunctKind::Seed => (fs_body * SEED_H_RATIO).max(SEED_MIN),
        PunctKind::Dot => (fs_body * FS_RATIO).max(D_MIN),
    }
    .min(room);
    let stroke = (d * STROKE_OF_D).clamp(STROKE_MIN, STROKE_MAX);
    // 中心先取右上角，再按「中心 ± (d/2 + stroke) 不出格」夹紧。
    let half = d * 0.5 + stroke;
    // 瓜子点的悬空微调：向左 / 向下各让 1.5px（不贴死前一字、不贴右缘）。
    let nudge = match kind {
        PunctKind::Seed => SEED_NUDGE_PX,
        PunctKind::Dot => 0.0,
    };
    let cx = ((cell_w * ANCHOR_X) - nudge).clamp(half, (cell_w - half).max(half));
    let cy = ((cell_h * ANCHOR_Y) + nudge).clamp(half, (cell_h - half).max(half));
    PunctMark {
        kind,
        d,
        cx,
        cy,
        stroke,
    }
}

/// **悬挂**句读的落位（v1.59.0）：X 走 [`HANG_ANCHOR_X`]，与常规句读分家。
///
/// 只有**被挤压进末字格**的那枚句读走这里（分页层 `GlyphKind::Hang`）。
/// 它不独占格位，故：
///
/// * X 推到字格右缘并与末字错开——这是「悬挂」，不是「叠在字上」；
/// * 纵向沉到末字脚下（[`HANG_ANCHOR_Y`]）；
/// * **绝不顶格**：X 只在本格内取值，永不落到列首格位上——悬挂标点的
///   存在与否，完全不改变下一列首字的 `top_baseline`。
///
/// 夹紧规则与 [`place`] 同：整个记号（含线宽）不探出字格，绝不压到邻列。
pub fn place_hang(fs_body: f32, cell_w: f32, cell_h: f32, kind: PunctKind) -> PunctMark {
    let m = place(fs_body, cell_w, cell_h, kind);
    let half = m.d * 0.5 + m.stroke;
    let cx = (cell_w * HANG_ANCHOR_X).clamp(half, (cell_w - half).max(half));
    let cy = (cell_h * HANG_ANCHOR_Y).clamp(half, (cell_h - half).max(half));
    PunctMark {
        kind,
        d: m.d,
        cx,
        cy,
        stroke: m.stroke,
    }
}

/// **手绘**一枚句读：纯 `paint_path`，绝不调用字体的 `，。` 字形。
///
/// `cx` / `cy` 是记号中心在**窗口**里的坐标（调用方把字格原点加好）。
/// 两种形制：
///
/// * [`PunctKind::Seed`] —— 五段二次贝塞尔围成的**实心瓜子点**：
///   下方收成尖（像水珠的坠点），上方鼓起圆头，`PathBuilder::fill` 填实；
/// * [`PunctKind::Dot`] —— 四段二次贝塞尔凑一个圆（[`CIRCLE_K`] 是凑圆
///   的控制点系数），`PathBuilder::stroke` 细线描边，**空心**。
///
/// 全程只读传入的几何与颜色，不碰任何状态——canvas 的 paint 闭包签名
/// （`&mut Window`）也从类型上保证了「绘制不改状态」。
pub fn paint_mark(window: &mut Window, cx: f32, cy: f32, m: PunctMark, ink: u32) {
    let color = rgb(ink);
    match m.kind {
        PunctKind::Dot => paint_ring(window, cx, cy, m, color),
        PunctKind::Seed => paint_seed(window, cx, cy, m, color),
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

/// **实心瓜子点**（逗号 / 顿号，v1.67.1）：**灵飞经笔意**的一笔「点」。
///
/// 形（局部坐标，中心为原点；`d` 为**沿笔势的长**，`w = d × SEED_W_OF_H` 为宽）：
///
/// ```text
///        ╱╲              ← 露锋入笔（尖，略偏右，不落中轴）
///       │▓▓│             ← 顿笔成肚：最宽处在**上半**，左右腹不等高
///       │ ▓ ╲
///        ╲  ╲            ← 提笔：右腹一路收细
///         ╲  ╲
///          ╲ ↙           ← 出锋：尾巴细长，末端往回一挑（回锋）
/// ```
///
/// **不镜像、不走直线、不落中轴**（三点都靠下面这套锚点 + 控制点做到）：
///
/// * **肥头在右上**：右腹（a[1]）是整枚最鼓、最靠右的点，左侧不镜像对称；
/// * **细尾扫向左下**：从右腹一路提笔收细，尾尖（a[2]）落在左下，且尾段
///   明显长于头段——正是「，」的走向；
/// * **左腰内凹**：尾尖 → 左腹（a[3]）那段的控制点向内（右）探，左缘先收
///   进再外鼓，形成提按的「腰」，不是一条直弦；
/// * **顶部圆润**：起笔（a[0]）到右上、到左上的两段控制点都把顶角撑圆。
///
/// 整枚再绕中心顺时针倾 [`SEED_TILT_DEG`] 度落地——起笔在右上、出锋在左下，
/// 正是竖排里「，」的走向（v1.67.2，离线高分辨率调形定稿，见 `seed_anchors`）。
///
/// 只碰几何，不碰排版：调用方已经把记号中心算在「格右上 + 悬空微调」处。
fn paint_seed(window: &mut Window, cx: f32, cy: f32, m: PunctMark, color: Rgba) {
    let a = seed_anchors(m.d);
    let (s, c) = SEED_TILT_DEG.to_radians().sin_cos();
    let hh = m.d * 0.5;
    let hw = hh * SEED_W_OF_H;
    let mut b = PathBuilder::fill();
    // ① 起笔（顶，略偏右）——露锋入纸的那一点
    b.move_to(rotated(cx, cy, s, c, a[0].0, a[0].1));
    // ② 起笔 → 右腹：顶弧圆过右上（控制点把顶角撑圆、右缘外鼓）
    b.curve_to(
        rotated(cx, cy, s, c, hw * 0.60, -hh * 1.02),
        rotated(cx, cy, s, c, a[1].0, a[1].1),
    );
    // ③ 右腹 → 尾尖：右腰收束，细长尾巴扫向左下（最长的一段）
    b.curve_to(
        rotated(cx, cy, s, c, hw * 0.50, hh * 0.45),
        rotated(cx, cy, s, c, a[2].0, a[2].1),
    );
    // ④ 尾尖 → 左腹：左腰内凹（控制点向内探），向上收
    b.curve_to(
        rotated(cx, cy, s, c, -hw * 0.22, hh * 0.62),
        rotated(cx, cy, s, c, a[3].0, a[3].1),
    );
    // ⑤ 左腹 → 起笔：左缘圆顺接顶
    b.curve_to(
        rotated(cx, cy, s, c, -hw * 0.92, -hh * 0.50),
        rotated(cx, cy, s, c, a[4].0, a[4].1),
    );
    b.close();
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// 瓜子点的**五个锚点**（相对记号中心的局部坐标，**已含倾角**）：
/// `[起笔, 右腹, 尾尖, 左腹, 起笔]`（首尾同点，闭合）。
///
/// 这套坐标是 v1.67.2 在 600px 画布、d=300 的离线高分辨率下逐轮调出来的，
/// 单独抽出来只为让单测能钉住那条笔势——「起笔在右上、出锋在左下、左腰
/// 内凹」——而不必去戳 `px` 像素。绘制与断言看的是同一份坐标，不会走偏：
///
/// ```text
///           起笔(顶,略右)
///          ╱           ╲
///         │  肥头在右上  │
///          ╲           ╱
///           ╲  左腰内凹 ╱
///            ╲         ╱
///             ↘ 尾尖(左下)
/// ```
pub fn seed_anchors(d: f32) -> [(f32, f32); 5] {
    let hh = d * 0.5;
    let hw = hh * SEED_W_OF_H;
    let (s, c) = SEED_TILT_DEG.to_radians().sin_cos();
    let rot = |x: f32, y: f32| (x * c - y * s, x * s + y * c);
    let entry = rot(hw * 0.12, -hh * 0.90); // 起笔：顶，略偏右（露锋）
    let right = rot(hw * 0.88, -hh * 0.52); // 右腹：最鼓、最靠右（肥头）
    let tail = rot(-hw * 0.45, hh * 1.08); // 尾尖：细长，扫向左下（出锋）
    let left = rot(-hw * 0.62, -hh * 0.28); // 左腹：左缘、略下
    [entry, right, tail, left, entry]
}

/// 把局部坐标绕记号中心旋转后落到窗口坐标。
///
/// `s` / `c` 是 [`SEED_TILT_DEG`] 的 sin / cos；屏幕坐标 y 向下，故
/// 顺时针旋转即 `x' = x·c − y·s`、`y' = x·s + y·c`。
fn rotated(cx: f32, cy: f32, s: f32, c: f32, x: f32, y: f32) -> Point<Pixels> {
    point(px(cx + x * c - y * s), px(cy + x * s + y * c))
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

    /// 悬挂句读的 X 必须比常规更靠右——否则它压在末字身上，是「叠字」不是「悬挂」。
    #[test]
    fn hanged_punct_hangs_to_the_right_of_the_normal_one() {
        for fs in [12.0, 21.9, 34.0, 60.0] {
            let (cw, ch) = cell(fs);
            let plain = place(fs, cw, ch, PunctKind::Dot);
            let hung = place_hang(fs, cw, ch, PunctKind::Dot);
            assert!(
                hung.cx > plain.cx + 1e-6,
                "悬挂未向右挂出：{} ≤ {}",
                hung.cx,
                plain.cx
            );
            assert!(hung.cy >= plain.cy - 1e-6, "悬挂未下沉：{hung:?}");
            // 共格不叠字：记号右缘要把末字字身让出来（≥ 格宽 60%）。
            assert!(hung.cx > cw * 0.60, "悬挂仍压在末字身上：{}", hung.cx);
        }
    }

    /// **绝不顶格**：悬挂只在本格内取值，永不越到列首格位。
    #[test]
    fn hanged_punct_never_claims_a_head_cell() {
        for fs in [12.0, 21.9, 34.0, 60.0] {
            let (cw, ch) = cell(fs);
            let m = place_hang(fs, cw, ch, PunctKind::Seed);
            // 记号整体（含线宽）必须留在本格内：不探出右缘、不探出下缘。
            let half = m.d * 0.5 + m.stroke;
            assert!(m.cx + half <= cw + 0.01, "悬挂探出右缘：{m:?} / cw={cw}");
            assert!(m.cy + half <= ch + 0.01, "悬挂探出下缘：{m:?} / ch={ch}");
            assert!(m.cx - half >= -0.01, "悬挂探出左缘：{m:?}");
        }
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
    fn center_sits_at_the_top_right_never_centered() {
        let m = place(34.0, 40.0, 44.0, PunctKind::Dot);
        assert!((m.cx - 40.0 * ANCHOR_X).abs() < 0.01, "横向落点错：{}", m.cx);
        assert!((m.cy - 44.0 * ANCHOR_Y).abs() < 0.01, "纵向落点错：{}", m.cy);
        // 「绝不能占据格子的正中心」：中心明显偏在右上象限（竖排句读的规范位）。
        assert!(m.cx > 40.0 * 0.55, "太靠左（近乎居中）：{}", m.cx);
        assert!(m.cy < 44.0 * 0.45, "太靠下（掉到字底下）：{}", m.cy);
    }

    /// 瓜子点（读类）的尺寸与形：长 = 字号 × 0.40、宽 = 长 × 0.55，
    /// 竖长的一粒，落在格右上再向左下让 1px（悬空感）。
    #[test]
    fn seed_is_a_small_melon_seed_scaled_to_the_body() {
        for fs in [12.0f32, 17.0, 21.9, 34.0] {
            let (cw, ch) = cell(fs);
            let m = place(fs, cw, ch, PunctKind::Seed);
            assert!(
                (m.d - fs * SEED_H_RATIO).abs() < 0.01,
                "瓜子点长应为 0.40 倍正文：{:.2} vs {:.2}",
                m.d,
                fs * SEED_H_RATIO
            );
            // 竖长形：长 > 宽，才收得出「细尾」。
            assert!(m.d > m.d * SEED_W_OF_H, "瓜子点应是竖长形：{m:?}");
            // 整体仍是一粒记号，绝不抢格心（≤ 0.60 倍正文）。
            assert!(m.d / fs <= 0.60, "瓜子点偏大：{:.3} 倍正文", m.d / fs);
            // 悬空微调：中心比「72%×28%」再左、再下各 1px。
            assert!(
                (m.cx - (cw * ANCHOR_X - SEED_NUDGE_PX)).abs() < 0.01,
                "未向左让出悬空：{}",
                m.cx
            );
            assert!(
                (m.cy - (ch * ANCHOR_Y + SEED_NUDGE_PX)).abs() < 0.01,
                "未向下让出悬空：{}",
                m.cy
            );
        }
    }

    /// **笔势**：露锋必须在右上、出锋必须在左下（灵飞经那一「点」的走向）。
    ///
    /// 这是「瓜子」与「小菱形」的分野——v1.66.0 忘了倾角，画出来是左右
    /// 对称的两头尖，真机上就是一颗红色碎屑。此测钉死倾角方向：
    /// 尾尖 x < 0 < 起笔 x，尾尖 y > 起笔 y（屏幕 y 向下）。
    #[test]
    fn seed_head_is_up_right_and_the_tail_hooks_down_left() {
        for d in [4.0f32, 8.0, 9.6, 13.6] {
            let a = seed_anchors(d);
            let (entry, right, tail, left) = (a[0], a[1], a[2], a[3]);
            assert!(tail.0 < 0.0, "出锋没扫向左：{tail:?}");
            assert!(entry.0 > 0.0, "露锋没落在右上：{entry:?}");
            assert!(tail.1 > entry.1, "出锋应在露锋下方：{tail:?} / {entry:?}");
            // 头尾的高差要有一枚瓜子的「长」：≥ 0.8 倍 d。
            assert!(
                tail.1 - entry.1 >= d * 0.8,
                "瓜子太短，看着是块碎屑：{} / d={d}",
                tail.1 - entry.1
            );
            // 起笔与收笔同一点（闭合路径，不留缺口）。
            assert_eq!(a[0], a[4], "瓜子没闭合");
            // **不镜像**：右腹（肥头）比左腹明显更靠右——一笔写下来的点，
            // 两侧绝不相等。旋转后两侧在 X 上拉开（胖的一侧探出更多）。
            assert!(
                right.0 - left.0 > d * 0.10,
                "左右对称回去了，没有偏侧肥头：左 {left:?} / 右 {right:?}"
            );
            // 肚子在上半：出锋那一截明显比入笔那一截长（才有「一撇」的爽利）。
            assert!(
                tail.1 - right.1 > (right.1 - entry.1) * 2.0,
                "出锋太短，肚子沉到下面去了：{a:?}"
            );
        }
    }

    #[test]
    fn mark_never_spills_out_of_its_cell() {
        // 任意字号 × 任意格子：整个记号（含线宽）都留在格内。
        for fs in [6.0, 12.0, 17.0, 24.0, 34.0, 60.0, 96.0] {
            for (cw, ch) in [(16.0, 18.0), (24.0, 26.0), (40.0, 44.0), (80.0, 88.0)] {
                for kind in [PunctKind::Dot, PunctKind::Seed] {
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
    /// 右上象限（过格心偏右、且偏在格上半），整个记号都留在格内。
    ///
    /// 为什么这条是「不被盖住」的前提：版框（外粗框 / 内细框 / 乌丝栏）
    /// 全在字格**之外**，记号只要不出格就与任何边框物理不相交；
    /// 一旦哪天把落点改回「居中」或让它溢出格外，这条测试就红——
    /// 那是句读被界行 / 版框裁掉的先兆（v1.34.0 用户点名「千万不能
    /// 被边框的渲染循环盖住或重置」，渲染顺序的保证见
    /// `reader::punct_overlay` 的注释）。
    #[test]
    fn mark_stays_in_the_top_right_quadrant_across_the_whole_space() {
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
                for kind in [PunctKind::Dot, PunctKind::Seed] {
                    let m = place(fs, cw, ch, kind);
                    // 右上象限：中心严格偏过格心（用户「不能居中」）。
                    assert!(
                        m.cx > cw * 0.5 && m.cy < ch * 0.5,
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
        // 句类画空心圈，读类画实心瓜子点。
        for c in ['。', '！', '？', '…', '.'] {
            assert_eq!(kind_of(c), PunctKind::Dot, "{c} 应画空心圈");
        }
        for c in ['，', '、', '；', '：', '—'] {
            assert_eq!(kind_of(c), PunctKind::Seed, "{c} 应画瓜子点");
        }
    }

    /// 目检用：`cargo test dump_punct -- --ignored --nocapture`。
    ///
    /// 打印常用字号下句读的直径、线宽与落点——一眼看出「0.35 倍、
    /// 钉在右上角、实心瓜子」。
    #[test]
    #[ignore]
    fn dump_punct_mark() {
        for fs in [12.0f32, 17.0, 21.9, 24.0, 34.0, 60.0] {
            let cw = fs / crate::grid::FS_OF_ROW;
            let ch = cw;
            for kind in [PunctKind::Dot, PunctKind::Seed] {
                let m = place(fs, cw, ch, kind);
                println!(
                    "正文 {fs:>5.1} → {kind:?} 径 {:>5.2}（{:.3}×正文）线宽 {:>4.2} | 格 {:>5.1}×{:>5.1} → 中心 ({:>5.2}, {:>5.2})",
                    m.d, m.d / fs, m.stroke, cw, ch, m.cx, m.cy
                );
            }
        }
    }
}
