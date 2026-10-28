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
//! * **形制**：读类（，、；：—）画**实心墨滴**（[`PunctKind::Seed`]）：
//!   **圆头落在右上、通体一路收细、末端圆润一提**（顺时针斜
//!   [`SEED_TILT_DEG`] 度）——一枚纯粹的小墨点，不鼓肚、不出锋、不分叉
//!   （v1.83.0「放生锦鲤」：旧版鼓肚 + 尾尖两侧切线张开，真机上是一条
//!   红锦鲤）；句类（。！？…）画**细线空心圈**（[`PunctKind::Dot`]）；
//!   两者一律 `paint_path` 手绘，绝不取字体字形；
//! * **大小**：瓜子点长 = 字号 × 0.54（[`SEED_H_RATIO`]）、宽 = 长的 0.56，
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

use gpui::{PathBuilder, Rgba, Window, point, px, rgb};

/// 句读直径 / 正文字号：**0.35**（v1.34.0 用户定「极小，像个手点的小记号」）。
///
/// 现在只作用于「句」类的空心圈（[`PunctKind::Dot`]）；读点走
/// [`SEED_H_RATIO`]（瓜子点，另有一组比例）。
pub const FS_RATIO: f32 = crate::types::PUNCT_FS_RATIO;
/// **瓜子点的长 / 正文字号**（v1.67.2）：0.50。
///
/// 一路改上来的：0.28 → 0.32 → 0.40 → 0.42 都还嫌小。实测 12 列行款下
/// 字号约 17pt，0.42 只有约 7pt 的一小粒，放大看仍是红色碎屑，且与同页
/// 的句号（0.35 倍、空心圈）轻重失衡——逗号显得又小又细（真机观感
/// 「不够灵动」）。0.50 在 17pt 上约 8.5pt，仍是字格的一半不到，但「露锋—
/// 顿肚—出锋」这一整套笔势终于撑得起来，与句号也协调了。
pub const SEED_H_RATIO: f32 = 0.54;
/// 瓜子点的**宽 / 长**：**0.32**（v1.83.0——「放生锦鲤」）。
///
/// 沿革：0.55 → 0.56 → 0.58 → **0.32**。
///
/// 注意口径：`hw = hh × SEED_W_OF_H` 是**半宽**，锚点再乘到 1.05 倍，
/// 故实际总宽 ≈ `2 × 1.05 × 0.32 × hh = 0.336 × d`——比 v1.80.0 的
/// 0.354 d 窄约 5%，正是用户要的「稍微变窄一点」，又不至于细成一根发丝。
///
/// 0.58 那档配上外扩的肚部控制点，真机上活脱脱一条**红锦鲤**：中段鼓出
/// 一个胖身子、尾根两侧切线一撑开就成了分叉的尾鳍——用户原话「分叉的
/// 尾巴和胖胖的身体」。收窄只是一个方面，真正的刀口在**控制点贴弦**。
///
/// **0.37（v1.84.0）** 是照用户给的精刻本照片反推的：照片里那枚朱砂读点
/// 沿笔势长 95px、最宽 34px，长宽比 **2.8**；宽度剖面（从细端起八等分）
/// `0.32 / 0.54 / 0.67 / 0.83 / 0.91 / 0.96 / 1.00 / 0.87`——**重的一头压在
/// 下右、细尾拖向左上**，正是毛笔按下后自然拖曳出的那道水痕。这一档的
/// 剖面与照片逐段相差不到 0.04。
pub const SEED_W_OF_H: f32 = 0.37;

/// 墨滴**最宽处（顿笔）**的纵向位置（局部坐标，单位为 `hh`；+1 是底端、
/// −1 是尾尖）。两档都做过 A/B，几何上**都合法**（各有单测钉着）：
///
/// | 档位 | 值 | 宽度峰值落在（从细尾起算） | 出处 |
/// |---|---|---|---|
/// | [`SEED_BELLY_GOLDEN`] | `0.20` | **0.618** | 黄金分割：峰落在全长 61.8% 处 |
/// | [`SEED_BELLY_BRUSH`]  | `0.47` | **0.75**  | 毛笔古意：照片里那枚读点正是 6/8 |
///
/// 峰位往**下**挪，重的一头更沉、更像「按下去」；往上挪则更瘦长、更
/// 数理。
///
/// **v1.84.0 定稿：用户 2026-10-28 在 3x 真机放大对比图上亲自选了
/// B 版（0.75 / 毛笔古意）**——理由是要「书卷气」而非秩序感，且与参考
/// 照片（峰在 6/8）同源。换档仍只改 [`SEED_BELLY_AT`] 一行，形制与全部
/// 单测两档都能过。
pub const SEED_BELLY_GOLDEN: f32 = 0.20;
/// 见 [`SEED_BELLY_GOLDEN`]：峰落在 0.75（照片的 6/8）。
pub const SEED_BELLY_BRUSH: f32 = 0.47;
/// **出货档**：`0.47`（B 版 · 峰 0.75），见上文定稿说明。
pub const SEED_BELLY_AT: f32 = SEED_BELLY_BRUSH;
/// 瓜子点的最小长（px）：行款兜底锁到 12px 正文时，它仍得是一粒看得见的墨点。
pub const SEED_MIN: f32 = 3.2;
/// 墨滴的**笔势倾角**（度，顺时针）：**42°**（v1.84.0；沿革 20° → 10° →
/// 17° → 23° → 42°）。
///
/// 23° 那几档是灵飞经小楷的侧势，写在屏幕上偏「立」——用户拿精刻本照片
/// 一比：「倾斜度似乎比我们定的还要大」。照片实测主轴**相对竖直 41.4°**
/// （PCA 量出来），故定 42°：整枚顺势倒向**右下**，细尾拖向**左上**，
/// 与照片里那枚读点同一走向。
///
/// 斜到这里会不会躺平？不会——记号整体只占 0.37d 宽，42° 下它在字格里
/// 仍是一枚斜置的墨点，不是一横。
///
/// ⚠️ **这个角只许乘一次**（v1.84.0 修的第二个坑）：`seed_anchors_for` /
/// `seed_controls_for` 里已经转过，`paint_seed` 只做平移。v1.83.1 两处都转，
/// 屏幕上因此是 2×23° = 46°——用户拿照片一比说「比我们定的还大」，正是这
/// 一刀造成的。区间由 `seed_axis_is_tilted_like_a_woodblock_comma` 钉着。
pub const SEED_TILT_DEG: f32 = 42.0;
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
/// 古风「〇」的**笔画厚 / 直径**（v1.81.0）：0.30。
///
/// 现代句号那条 0.22 的细线是「勾」出来的几何圈，刻本的「〇」是**写**
/// 出来的——墨要厚一档才立得住。9.2px 真机上 d≈4.2、笔画≈1.26px：抗锯齿
/// 吃不掉，仍看得出是个圈（0.22 那档只剩 0.9px，屏幕上就是个灰点）。
pub const RING_STROKE_OF_D: f32 = 0.30;
/// 古风「〇」的**直径下限**（px）：比现代句号的 [`D_MIN`] 抬一档。
pub const RING_MIN: f32 = 4.0;
/// 古风「〇」的**笔画上限**（px）：比 [`STROKE_MAX`] 松一档，大字号的「〇」
/// 才压得住（它是句末的收束，不能细得像根发丝）。
pub const RING_STROKE_MAX: f32 = 3.2;
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
///
/// 四种与 core 的符号库 [`Jodu`] 一一对应（v1.81.0）——**分类归排版引擎
/// 管**（`gloss::jodu_of` 是唯一数据源），这里只把手绘接上去，屏幕与 PDF
/// 因此永远画同一枚形。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PunctKind {
    /// **现代「句」**（。！？…）：**细线空心小圈**——一枚极小的、线条极细的圆。
    Dot,
    /// 「读」类（，、；：—）：**实心墨滴**——圆头在右上、通体一路收细、
    /// 末端圆润一提，顺时针斜 [`SEED_TILT_DEG`]，是毛笔按下再轻轻提起的
    /// 那一点（v1.67.1 起，替掉旧的水滴、对称梭形与 20° 肥头版；v1.83.0
    /// 定稿：收掉鼓肚与分叉的尾，回到一枚纯粹的墨点）。
    Seed,
    /// **古籍「句」**（。．）：**古风小圆圈「〇」**。
    ///
    /// 不是现代那种几何细线空心圆——更大一圈、笔画更厚，且**略带椭圆**
    /// （毛笔写出来的圈不会是正圆），起笔处有一顿，是「写」出来的圈。
    Ring,
    /// **疑 / 叹**（？！）：**大一号**的墨滴。
    ///
    /// 古人极少用问号叹号；真遇上，也只是把「读」那一点写重、写大——绝不
    /// 画「？」那个西式弯钩。
    BigSeed,
}

impl PunctKind {
    /// 直径 / 正文字号：**一律读 core 符号库的口径**（屏幕与 PDF 同一张表）。
    pub fn fs_ratio(self) -> f32 {
        self.jodu().fs_ratio()
    }

    /// 线宽 / 直径（只有圈类用；斜点是填实，不看这个）。
    pub fn stroke_frac(self) -> f32 {
        match self {
            PunctKind::Dot => STROKE_OF_D,
            // 「〇」的笔画要厚：它是写出来的圈，不是细线勾的圈。
            PunctKind::Ring => RING_STROKE_OF_D,
            PunctKind::Seed | PunctKind::BigSeed => STROKE_OF_D,
        }
    }

    /// 直径的**绝对下限**（px）：行款锁到最小字号时，它仍得是一粒看得见的墨点。
    pub fn min_d(self) -> f32 {
        match self {
            PunctKind::Dot => D_MIN,
            // 「〇」比现代句号大一圈，下限也跟着抬——4px 以下那个圈就
            // 只剩一枚像素团，看不出是圈了。
            PunctKind::Ring => RING_MIN,
            PunctKind::Seed | PunctKind::BigSeed => SEED_MIN,
        }
    }

    /// 手绘线宽（px）：只有圈类用（斜点是填实的实心笔画）。
    pub fn stroke_of(self, d: f32) -> f32 {
        let (frac, hi) = match self {
            PunctKind::Ring => (RING_STROKE_OF_D, RING_STROKE_MAX),
            _ => (STROKE_OF_D, STROKE_MAX),
        };
        (d * frac).clamp(STROKE_MIN, hi)
    }

    /// 反查 core 符号库的那一项（归档用，也保证两边不漂移）。
    pub fn jodu(self) -> jiandu_core::gloss::Jodu {
        use jiandu_core::gloss::Jodu;
        match self {
            PunctKind::Dot => Jodu::Dot,
            PunctKind::Seed => Jodu::Seed,
            PunctKind::Ring => Jodu::Ring,
            PunctKind::BigSeed => Jodu::BigSeed,
        }
    }
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
    /// **回锋**（v1.80.0）：出锋末端带不带那一小勾（[`SEED_HOOK`]）。
    /// 只对瓜子点（读类）有意义。
    pub hook: bool,
}

/// 字符 → 句读形制（**现代模式**，兼容旧调用点）。
///
/// 分类口径与 `types::is_comma_punct` 同源：`is_punct` 集合内、非
/// `is_stop_punct` 的都算「读」。古籍模式请走 [`kind_for`]。
pub fn kind_of(ch: char) -> PunctKind {
    kind_for(ch, jiandu_core::gloss::PunctMode::Modern).unwrap_or(PunctKind::Dot)
}

/// 字符 + **句读模式** → 形制（v1.81.0）。
///
/// 查表的是 core 的符号库 [`jiandu_core::gloss::jodu_of`]——排版引擎说
/// 画什么，这里就画什么，绝不在渲染层另判一次（两处各判一次必走偏）。
///
/// `None` = 这一枚不归符号库管（引号 / 括号 / 书名号，仍按原字形排）；
/// 调用方（字格渲染）本来就只对句读字符调这里，故 `None` 兜成圈也无害。
pub fn kind_for(ch: char, mode: jiandu_core::gloss::PunctMode) -> Option<PunctKind> {
    jiandu_core::gloss::jodu_of(ch, mode).map(|j| match j {
        jiandu_core::gloss::Jodu::Dot => PunctKind::Dot,
        jiandu_core::gloss::Jodu::Seed => PunctKind::Seed,
        jiandu_core::gloss::Jodu::Ring => PunctKind::Ring,
        jiandu_core::gloss::Jodu::BigSeed => PunctKind::BigSeed,
    })
}

/// 句读落位：**墨滴长 × 0.54、中心钉在字格 72% × 28% 的右上角**。
///
/// ```text
/// 读类（墨滴）：d = max(正文 × 0.54, SEED_MIN)，宽 ≈ d × 0.34，顺时针斜 23°
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
    // 直径一律从 core 的符号库取（屏幕 / PDF 同一张表），不再在本模块
    // 另写一份 `match kind` —— 加一种形制只需改 `gloss::Jodu::fs_ratio`。
    let d = (fs_body * kind.fs_ratio()).max(kind.min_d()).min(room);
    let stroke = kind.stroke_of(d);
    // 中心先取右上角，再按「中心 ± (d/2 + stroke) 不出格」夹紧。
    let half = d * 0.5 + stroke;
    // 瓜子点的悬空微调：向左 / 向下各让 1px（不贴死前一字、不贴右缘）。
    // 「疑叹」那枚大点同是斜撇，跟着一起悬；两款圈子是圆的，居中即正，
    // 不悬（悬了反而看着歪）。
    let nudge = match kind {
        PunctKind::Seed | PunctKind::BigSeed => SEED_NUDGE_PX,
        PunctKind::Dot | PunctKind::Ring => 0.0,
    };
    let cx = ((cell_w * ANCHOR_X) - nudge).clamp(half, (cell_w - half).max(half));
    let cy = ((cell_h * ANCHOR_Y) + nudge).clamp(half, (cell_h - half).max(half));
    PunctMark {
        kind,
        d,
        cx,
        cy,
        stroke,
        hook: SEED_HOOK,
    }
}

/// 给一枚记号**挂上开关**——只改 [`PunctMark::hook`]，其余照旧。
///
/// 排版层（[`place`] / [`place_hang`]）不管用户偏好，画之前由渲染层按
/// `Prefs::punct_hook` 盖一道，省得给两个 `place` 各加一个参数。
pub fn with_hook(mut m: PunctMark, on: bool) -> PunctMark {
    m.hook = on;
    m
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
        hook: m.hook,
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
        PunctKind::Ring => paint_classic_ring(window, cx, cy, m, color),
        PunctKind::Seed | PunctKind::BigSeed => paint_seed(window, cx, cy, m, color),
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

/// **古风小圆圈「〇」**（v1.81.0）：句号在古籍模式里的形。
///
/// 与现代 [`paint_ring`] 那枚「细线空心圆」的区别有三，全是**照毛笔写的
/// 样子**改的，不是换个尺寸了事：
///
/// * **笔画厚**：线宽走 [`RING_STROKE_OF_D`]（0.30，现代那枚是 0.22）——
///   刻本的「〇」是写出来的一圈墨，不是几何勾出的细线；
/// * **略带椭圆**：横径比纵径宽 [`RING_OVAL`]（4%）——手写的圈起笔时腕子
///   是斜的，正圆一眼就是机器画的；
/// * **起笔一顿**：落笔在正上方偏右（[`RING_START_DEG`]），绕满一圈后再多
///   走 18°收笔（[`RING_OVERLAP_DEG`]），接口处叠出一小段重墨，正是毛笔
///   收口那一下；小字号下它让圈更「闭」，不会看成断开的弧。
///
/// 采样 24 段折线（`line_to`）：3~4px 的真机尺寸下，折线与曲线的差别在
/// 一个像素内，但省掉四段贝塞尔的控制点推演，形更好调。
fn paint_classic_ring(window: &mut Window, cx: f32, cy: f32, m: PunctMark, color: Rgba) {
    // 直径 m.d 是「纵径」；横径 = 纵径 × (1 + RING_OVAL)。
    let ry = m.d * 0.5;
    let rx = ry * (1.0 + RING_OVAL);
    let span = 360.0 + RING_OVERLAP_DEG;
    let steps = 24usize;
    let mut b = PathBuilder::stroke(px(m.stroke));
    let at = |deg: f32| {
        let t = deg.to_radians();
        (cx + rx * t.cos(), cy + ry * t.sin())
    };
    let (x0, y0) = at(RING_START_DEG);
    b.move_to(point(px(x0), px(y0)));
    for i in 1..=steps {
        let (x, y) = at(RING_START_DEG + span * i as f32 / steps as f32);
        b.line_to(point(px(x), px(y)));
    }
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// 「〇」的**椭圆度**：横径比纵径宽的百分比（PDF 那头也读它，故为 pub）。
pub const RING_OVAL: f32 = 0.04;
/// 「〇」的**起笔角**（度，0 = 正右、顺时针）：从正上方偏右 18° 落笔。
const RING_START_DEG: f32 = -72.0;
/// 「〇」的**收笔重叠**（度）：绕满一圈后多走一截，接口叠出重墨。
const RING_OVERLAP_DEG: f32 = 18.0;

/// **实心墨滴**（逗号 / 顿号，v1.83.0「放生锦鲤」）：**一枚纯粹的小墨点**。
///
/// 形（局部坐标，中心为原点；`d` 为**沿笔势的长**，`w = d × SEED_W_OF_H` 为宽）：
///
/// ```text
///          ╭─╮             ← 圆头：头顶就是一个半圆（不露锋、不尖）
///         ╱   ╲
///        ╱     ╲           ← 最宽处紧跟头顶（只低 0.28hh），头帽才圆
///       ╱       ╲
///      ╱         ╲         ← 腰身贴弦一路收细，中段绝不鼓肚
///     ╱           ╲
///    ╰──────╯       ╲      ← 收尾：向左后兜回的一个圆口（轻轻提起）
/// ```
///
/// **不鼓肚、不分叉、不走直线**（靠下面这套锚点 + 控制点做到）：
///
/// * **圆头**：头顶（a[0]）正落在笔势轴上，最宽处（a[1]）只比它低
///   0.28hh——一条**短弧**就把头收圆了。旧版最宽处低到 −0.29 处，头被拉成
///   一条斜边、顶上还留个尖，放大看正是鱼吻；
/// * **腰身一路收细**：a[1] → 尾尖（a[2]）的**控制点贴着弦**走。旧版把它
///   推到弦外 0.14hw，中段鼓出一个胖身子——真机上就是那条锦鲤；
/// * **收尾是一个圆口**：尾尖两侧的切线张开约 117°（来向斜下、去向近乎
///   水平向左），末端合成一段圆润的收口，绝不张开成两片鳍；
/// * **外缘近直**：尾尖 → 左腰（a[3]）→ 起笔两段贴弦上行，与右侧对称收敛。
///
/// 整枚再绕中心顺时针倾 [`SEED_TILT_DEG`]（17°）定走向——解剖本身已经
/// 画好了，这一手把头抬向右上、把尾压向左下，落成灵飞经侧点那一斜
/// （v1.80.0 离线高分辨率逐档比对定稿，见 `seed_anchors`）。
///
/// 只碰几何，不碰排版：调用方已经把记号中心算在「格右上 + 悬空微调」处。
fn paint_seed(window: &mut Window, cx: f32, cy: f32, m: PunctMark, color: Rgba) {
    // 几何全部来自 `seed_anchors_for` / `seed_controls_for`（纯函数、有单测），
    // 这里只负责串路径——「画出什么形」不在这里定。
    let a = seed_anchors_for(m.d, m.hook); // 7 个锚点（第 8 个即回到 a[0]）
    let k = seed_controls_for(m.d, m.hook); // 7 个控制点
    // ⚠️ **倾角只许转一次**：锚点与控制点从 `seed_*_for` 出来时**已经含倾角**
    // （屏幕坐标下的偏移），这里只做平移。v1.83.1 在这里又转了一次，屏幕上
    // 于是成了 2×23° = 46°——用户一看「比我们定的还要大」，就是这一刀。
    let mut b = PathBuilder::fill();
    b.move_to(point(px(cx + a[0].0), px(cy + a[0].1)));
    for i in 0..7 {
        let n = (i + 1) % 7;
        b.curve_to(
            point(px(cx + k[i].0), px(cy + k[i].1)),
            point(px(cx + a[n].0), px(cy + a[n].1)),
        );
    }
    b.close();
    if let Ok(p) = b.build() {
        window.paint_path(p, color);
    }
}

/// 七段曲线的**控制点**（窗口坐标，已含倾角）：`[c0..c6]`。
///
/// 与 [`seed_anchors`] 同尺（都是绕记号中心转 [`SEED_TILT_DEG`] 之后的
/// 绝对坐标），绘制与单测看的是同一份数，不会走偏：
///
/// * **c0 / c6** —— 细尾那一段的两侧：c0 微微外鼓，让尾巴**贴着弦一路
///   收细**（不鼓出第二个肚子）；c6 是尾口的小圆帽，把末端收钝；
/// * **c1 / c2** —— 右外缘下行到最宽处：c2 落在 (1.02hw, 0.78hh)，把「顿
///   笔」那一下推到**下半**，重的一头才压得住；
/// * **c3 / c4** —— 底部圆弧：两个控制点都落在 y ≈ 1.05hh 之外，底端是
///   一个圆转的收口，不是切平的方底；
/// * **c5** —— 左外缘上行：贴弦回到细尾，与右侧构成一路收细的拖曳。
///
/// 三个约束各有单测钉着：`seed_tail_is_blunt_not_pinched`（尾口不尖）、
/// `seed_taper_is_monotonic_from_tip_to_belly`（一路收细、不鼓第二只肚）、
/// `seed_belly_sits_at_the_lower_right`（最宽处压在下半）。
pub fn seed_controls(d: f32) -> [(f32, f32); 7] {
    seed_controls_for(d, false)
}

/// 指定**最宽处**的那一版（A/B 用）：见 [`SEED_BELLY_AT`]。
pub fn seed_controls_belly(d: f32, hook: bool, belly: f32) -> [(f32, f32); 7] {
    seed_controls_at(d, hook, belly)
}

/// 回锋档的控制点：只把尾口那一段收短，其余同常档。
pub fn seed_controls_hook(d: f32) -> [(f32, f32); 7] {
    seed_controls_for(d, true)
}

fn seed_controls_for(d: f32, hook: bool) -> [(f32, f32); 7] {
    seed_controls_at(d, hook, SEED_BELLY_AT)
}

/// 控制点的真身：`belly` 是**最宽处**的纵向位置（见 [`SEED_BELLY_AT`]）。
fn seed_controls_at(d: f32, hook: bool, belly: f32) -> [(f32, f32); 7] {
    let hh = d * 0.5;
    let hw = hh * SEED_W_OF_H;
    let (s, c) = SEED_TILT_DEG.to_radians().sin_cos();
    // 屏幕 y 向下。锚点的局部 **+y 是重的一头**，要让它落到**右下**
    // （照片里正是如此），旋转取 `(x·c + y·s, −x·s + y·c)`——即屏幕上的
    // 逆时针 42°：局部的「下」被甩向「右下」。
    let rot = |x: f32, y: f32| (x * c + y * s, -x * s + y * c);
    if hook {
        // 回锋档：尾短一截（两个控制点跟着往里收），其余与常档同一条外缘。
        [
            rot(hw * 0.52, -hh * 0.62), // 尾段右缘
            rot(hw * 0.80, -hh * 0.04), // 右外缘下行
            rot(hw * 1.02, hh * 0.78),  // 顿笔：最宽处
            rot(hw * 0.24, hh * 1.08),  // 底部圆弧
            rot(-hw * 0.74, hh * 1.02), // 底部回收
            rot(-hw * 1.02, hh * 0.02), // 左外缘上行
            rot(hw * 0.16, -hh * 0.86), // 尾口小圆帽（收短）
        ]
    } else {
        [
            rot(hw * 0.41, -hh * 0.69), // 尾段右缘：贴弦，一路收细
            rot(hw * 0.82, hh * belly * 0.05),   // 右外缘下行
            rot(hw * 1.03, hh * (belly + 0.34)), // 顿笔：最宽处压在下半
            rot(hw * 0.22, hh * 1.09),           // 底部圆弧
            rot(-hw * 0.75, hh * 1.04),          // 底部回收
            rot(-hw * 1.03, hh * belly * 0.40),  // 左外缘上行
            rot(hw * 0.00, -hh * 1.04), // 尾口小圆帽：钝口，抗锯齿吃不掉
        ]
    }
}

/// **采样成折线**（相对记号中心，**已含倾角**）：PDF 那一头与单测共用。
///
/// 屏幕走 `paint_path`（GPUI 自己光栅化贝塞尔）；PDF 那头是 CG，只能吃
/// 折线。两边看的是同一份 `seed_anchors_for` / `seed_controls_for`，故
/// 纸上与屏幕上**逐字同位、同形**——`STEPS` 取 12，一段 12 根折线在
/// d≈9px 下与真曲线的偏差远小于 0.1px（肉眼与印刷都看不出）。
pub fn seed_outline(d: f32, hook: bool) -> Vec<(f32, f32)> {
    seed_outline_belly(d, hook, SEED_BELLY_AT)
}

/// 指定**最宽处**的那一版（A/B 用）：见 [`SEED_BELLY_AT`]。
pub fn seed_outline_belly(d: f32, hook: bool, belly: f32) -> Vec<(f32, f32)> {
    const STEPS: usize = 12;
    let a = seed_anchors_at(d, hook, belly);
    let k = seed_controls_at(d, hook, belly);
    let mut out = Vec::with_capacity(7 * STEPS);
    for i in 0..7 {
        let (p0, p1, kp) = (a[i], a[(i + 1) % 7], k[i]);
        for s in 0..STEPS {
            let t = s as f32 / STEPS as f32;
            let mt = 1.0 - t;
            out.push((
                mt * mt * p0.0 + 2.0 * mt * t * kp.0 + t * t * p1.0,
                mt * mt * p0.1 + 2.0 * mt * t * kp.1 + t * t * p1.1,
            ));
        }
    }
    out
}

/// 墨滴的**六个锚点**（相对记号中心的局部坐标，**已含倾角**）：
/// `[尾外, 右腰, 右顿, 右底, 底内, 左顿, 尾内]`，第 7 段回到 `尾外` 闭合。
///
/// v1.84.0 照用户给的精刻本照片重画的**走向**（此前是「重头在上」，照片里
/// 恰恰相反）：**重的一头压在下右、细尾拖向左上**——毛笔按下、顺势拖出
/// 的那一道水痕，收笔处钝而不尖。
///
/// ```text
///        ╱╲                 ← 尾口：一小段圆帽（钝，抗锯齿吃不掉）
///       ╱  ╲
///      ╱    ╲               ← 细尾：贴弦一路收细，绝不鼓第二只肚
///     ╱      ╲
///    │        ╲
///    │         │            ← 最宽处：压在**下半**（照片量出来在 0.8 处）
///    │         │
///     ╲       ╱
///      ╰─────╯              ← 底部：圆转的收口，不是切平的方底
/// ```
///
/// 两个拓扑细节各有单测钉着：
///
/// * **尾口是两个锚点之间的一小段圆弧**（不是单顶点）。单顶点拓扑下末端
///   宽度恒为 0，真机 d≈9.3px 时尾巴不到半个设备像素，2× 光栅化后与主块
///   **断开**——用户原话「骨折了，像断开的双截棍」（v1.83.1）；
/// * **最宽处在下半**（[`seed_belly_sits_at_the_lower_right`]）——照片的
///   宽度剖面在 7/8 处才到顶，重的一头明显偏下。
pub fn seed_anchors(d: f32) -> [(f32, f32); 7] {
    seed_anchors_for(d, false)
}

/// 指定**最宽处**的那一版（A/B 用）：见 [`SEED_BELLY_AT`]。
pub fn seed_anchors_belly(d: f32, hook: bool, belly: f32) -> [(f32, f32); 7] {
    seed_anchors_at(d, hook, belly)
}

/// 墨滴的**六个锚点**（回锋档）：只尾口收短一截，见 [`seed_anchors`]。
pub fn seed_anchors_hook(d: f32) -> [(f32, f32); 7] {
    seed_anchors_for(d, true)
}

/// 两档共用的锚点计算：`hook` 只改尾口两点（其余四点完全一样）。
fn seed_anchors_for(d: f32, hook: bool) -> [(f32, f32); 7] {
    seed_anchors_at(d, hook, SEED_BELLY_AT)
}

/// 锚点的真身：`belly` 是**最宽处**的纵向位置（见 [`SEED_BELLY_AT`]）。
fn seed_anchors_at(d: f32, hook: bool, belly: f32) -> [(f32, f32); 7] {
    let hh = d * 0.5;
    let hw = hh * SEED_W_OF_H;
    let (s, c) = SEED_TILT_DEG.to_radians().sin_cos();
    // 屏幕 y 向下。锚点的局部 **+y 是重的一头**，要让它落到**右下**
    // （照片里正是如此），旋转取 `(x·c + y·s, −x·s + y·c)`——即屏幕上的
    // 逆时针 42°：局部的「下」被甩向「右下」。
    let rot = |x: f32, y: f32| (x * c + y * s, -x * s + y * c);
    let (tip_out, tip_in) = if hook {
        (rot(hw * 0.30, -hh * 0.84), rot(hw * 0.02, -hh * 0.76)) // 回锋：尾短一截
    } else {
        (rot(hw * 0.18, -hh * 1.00), rot(-hw * 0.16, -hh * 0.92)) // 尾口：钝口
    };
    [
        tip_out,
        rot(hw * 0.48, -hh * 0.24), // 右腰：细尾与顿笔之间的转折
        rot(hw * 1.00, hh * belly),      // 右顿：最宽处
        rot(hw * 0.60, hh * 0.95),       // 右底
        rot(-hw * 0.35, hh * 0.91),      // 底内：与右底之间是底部圆弧
        rot(-hw * 0.94, hh * (belly + 0.06)), // 左顿（跟着 belly 走）
        tip_in,
    ]
}

/// **回锋开关**（默认关）：出锋末端是不是带一小勾。
///
/// `false`（默认）= 一路提笔出锋，尾巴爽利地尖出去；
/// `true` = 锋尖回收一档并在末端留一个小回勾——灵飞经小楷里「顿后回锋」
/// 那一笔的笔意，多一分含蓄。开关落在 `Prefs::punct_hook`，版字面板可切；
/// 只改**尾尖一点**（[`seed_anchors_hook`]）与它的两个控制点，头的圆润
/// 与整体斜度不受影响。
pub const SEED_HOOK: bool = false;

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

    /// A/B 两档（v1.84.0）：`(最宽处, 宽度峰值应落在)`。
    ///
    /// 用户要的正是这两版：**0.618（黄金分割）** 与 **0.75（照片的 6/8）**。
    /// 两档都在下面的每一条形制测试里跑一遍——**不许画鱼 / 不许断**这条
    /// 底线，换哪一档都得过；将来再调峰位，只需往这里加一行。
    const BELLY_VARIANTS: [(f32, f32); 2] = [
        (SEED_BELLY_GOLDEN, 0.618),
        (SEED_BELLY_BRUSH, 0.75),
    ];

    /// **笔势**（v1.84.0）：**重的一头压在右下、细尾拖向左上**。
    ///
    /// 这是照用户给的精刻本照片改的走向——照片里那枚朱砂读点，顿笔那一头在
    /// 下右、细尾顺着笔势拖向左上（PCA 量得主轴相对竖直 41.4°）。此前的版本
    /// 恰恰是反的（重头在上），用户一眼看出「不像」。
    #[test]
    fn seed_points_down_right_with_the_tail_dragging_up_left() {
        for (belly, _) in BELLY_VARIANTS {
            for d in [4.0f32, 8.0, 9.27, 13.6] {
                let a = seed_anchors_belly(d, false, belly);
                // 七锚点口径：0 尾外 / 1 右腰 / 2 右顿（最宽）/ 3 右底 /
                //             4 底内 / 5 左顿 / 6 尾内
                let (tip_out, belly_p, tip_in, bottom) = (a[0], a[2], a[6], a[3]);
                // 顿笔在细尾的**下右**：屏幕 y 向下，故 y 与 x 都明显更大。
                // 纵向拉开 0.30d 起：A 版峰位偏高（0.618）实测 0.34d，
                // B 版（0.75）实测 0.44d——两版都要过得去，故取 0.30。
                assert!(
                    belly_p.1 - tip_out.1 > d * 0.30,
                    "顿笔没压到下半：{belly_p:?} / 尾 {tip_out:?}"
                );
                assert!(
                    belly_p.0 - tip_out.0 > d * 0.30,
                    "顿笔没探向右：{belly_p:?} / 尾 {tip_out:?}"
                );
                // 尾口那两个锚点都落在**上左**（离顿笔至少 0.3d）。
                // 同上：A 版实测 0.27d、B 版 0.37d，取 0.25。
                assert!(belly_p.1 - tip_in.1 > d * 0.25, "尾口没拖向左上：{tip_in:?}");
                assert!(belly_p.0 - tip_in.0 > d * 0.25, "尾口没拖向左上：{tip_in:?}");
                // 沿笔势的长要有一枚墨滴的「长」：尾口 → 底部 ≥ 0.85 倍 d。
                let span =
                    ((bottom.0 - tip_out.0).powi(2) + (bottom.1 - tip_out.1).powi(2)).sqrt();
                assert!(span >= d * 0.85, "墨滴太短，看着是块碎屑：{span} / d={d}");
                // **不镜像**：顿笔一侧明显比另一侧探出更多。
                let left = a[5];
                assert!(
                    belly_p.0 - left.0 > d * 0.10,
                    "左右对称回去了，没有偏侧重头：左 {left:?} / 右 {belly_p:?}"
                );
            }
        }
    }

    /// 沿笔势轴的**宽度剖面**：`(按 `nb` 等分的宽度, 轴长, 相对竖直的角度)`。
    ///
    /// 轴 = 「细尾中点 → 底部中点」，分段从细尾起算。测形不看锚点下标，
    /// 直接量成品轮廓（[`seed_outline_belly`]），与真机画出来的是同一条曲线。
    fn width_profile(d: f32, hook: bool, belly: f32, nb: usize) -> (Vec<f32>, f32, f32) {
        let a = seed_anchors_at(d, hook, belly);
        let pts = seed_outline_belly(d, hook, belly);
        let mid = |p: (f32, f32), q: (f32, f32)| ((p.0 + q.0) * 0.5, (p.1 + q.1) * 0.5);
        let (tip, bot) = (mid(a[0], a[6]), mid(a[3], a[4]));
        let (ax, ay) = (bot.0 - tip.0, bot.1 - tip.1);
        let len = (ax * ax + ay * ay).sqrt();
        let (ux, uy) = (ax / len, ay / len); // 轴向（细尾 → 顿笔）
        let (vx, vy) = (-uy, ux);            // 垂直方向
        let mut tmin = f32::MAX;
        let mut tmax = f32::MIN;
        let proj: Vec<(f32, f32)> = pts
            .iter()
            .map(|&(x, y)| {
                let t = x * ux + y * uy;
                tmin = tmin.min(t);
                tmax = tmax.max(t);
                (t, x * vx + y * vy)
            })
            .collect();
        let mut lo = vec![f32::MAX; nb];
        let mut hi = vec![f32::MIN; nb];
        for &(t, u) in &proj {
            let k = (((t - tmin) / (tmax - tmin + 1e-9)) * nb as f32) as usize;
            let k = k.min(nb - 1);
            lo[k] = lo[k].min(u);
            hi[k] = hi[k].max(u);
        }
        let w: Vec<f32> = (0..nb)
            .map(|k| if hi[k] >= lo[k] { hi[k] - lo[k] } else { 0.0 })
            .collect();
        // 相对竖直的角度：轴与「正下方」的夹角。
        (w, len, ux.atan2(uy).to_degrees().abs())
    }

    /// **倾角**：主轴相对竖直 **42°**（照片实测 41.4°）。
    ///
    /// 这条同时钉住「**倾角只许转一次**」：v1.83.1 在 `seed_anchors_at` 与
    /// `paint_seed` 里各转了一次，屏幕上实际是 2×23° = 46°，用户拿照片一比
    /// 「比我们定的还要大」——再犯就是 84°，这里立刻红。
    #[test]
    fn seed_axis_is_tilted_like_a_woodblock_comma() {
        for (belly, _) in BELLY_VARIANTS {
            for (d, hook) in [(4.0f32, false), (9.27, false), (13.6, false), (24.0, false), (9.27, true)] {
                let (_, _, deg) = width_profile(d, hook, belly, 8);
                assert!(
                    (36.0..=48.0).contains(&deg),
                    "主轴 {deg:.1}°：照片是 41.4°——<36° 又立回去了，>48° 是倾角被乘了两次"
                );
            }
        }
    }

    /// **一路收细**（v1.84.0）：从细尾到最宽处，宽度**单调不减**。
    ///
    /// v1.80.0 的「红锦鲤」就是中段鼓出第二只肚子——宽度剖面在腰上先鼓一个
    /// 包再收，真机上活脱脱一条鱼。这条只查**细尾 → 峰**那一段（峰在底部圆
    /// 弧之前），底部收口那一段本就该变窄，不在查的范围内。
    #[test]
    fn seed_taper_is_monotonic_from_tip_to_belly() {
        for (belly, _) in BELLY_VARIANTS {
            for (d, hook) in [(4.0f32, false), (9.27, false), (13.6, false), (24.0, false), (9.27, true)] {
                let (w, len, _) = width_profile(d, hook, belly, 8);
                let max = w.iter().cloned().fold(0.0, f32::max);
                let peak = w
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                    .unwrap()
                    .0;
                // 细尾足够细（照片里是 0.32）：不许胖成一枚方块。
                assert!(
                    w[0] <= max * 0.45,
                    "细尾太胖（{:.0}%）：像方块不像墨点",
                    w[0] / max * 100.0
                );
                // 细尾又不能细到没有（那是 v1.83.1 的「双截棍」）。
                assert!(w[0] >= d * 0.05, "细尾太虚（{:.2}px），真机会断", w[0]);
                // 单调（留 3% 的容差给折线采样）。
                for k in 1..=peak {
                    assert!(
                        w[k] >= w[k - 1] - max * 0.03,
                        "第 {k} 段收细回去了（{:.2} < {:.2}）：中段又鼓出一只肚子——画鱼了",
                        w[k],
                        w[k - 1]
                    );
                }
                // 长宽比：照片是 2.8，不许胖过 3.6 也不许瘦成一根针。
                assert!(
                    len / max > 2.2 && len / max < 3.6,
                    "长宽比 {:.2}：照片是 2.8",
                    len / max
                );
            }
        }
    }

    /// **最宽处落在它该在的位置**（v1.84.0）：A 版 0.618、B 版 0.75。
    ///
    /// 这条是 A/B 两档的**定盘星**——峰位即「重的一头沉在哪」。峰若跑到上半，
    /// 就又变回那枚「头重脚轻」的斜刀片；两档都必须在下半，且各自命中
    /// 自己的目标（容差 0.05，两档相差 0.13，分得开）。
    #[test]
    fn seed_belly_sits_where_its_variant_says() {
        for (belly, target) in BELLY_VARIANTS {
            for (d, hook) in [(4.0f32, false), (9.27, false), (13.6, false), (24.0, false), (9.27, true)] {
                // 32 段精测（8 段太粗，分不出 0.618 与 0.75）。
                let (w, _, _) = width_profile(d, hook, belly, 32);
                let peak = w
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                    .unwrap()
                    .0;
                let frac = (peak as f32 + 0.5) / 32.0;
                assert!(
                    (frac - target).abs() <= 0.05,
                    "最宽处落在 {frac:.3}（应 {target:.3}）：belly={belly} d={d} hook={hook}"
                );
                // 两版都必须在下半（≥ 0.5）——重的一头不许跑上去。
                assert!(frac >= 0.5, "重的一头跑到上半去了：{frac:.3}");
            }
        }
    }

    /// **尾口必须钝到不断**（v1.83.1「双截棍」，v1.84.0 沿用）：尾口两个锚点
    /// 之间的距离就是末端的**实际宽度**，它必须宽到抗锯齿吃不掉。
    ///
    /// 病根：v1.83.0 用**单顶点**收尾（尾宽恒为 0）。真机 d≈9.27px 时，
    /// 尾巴末端只剩不到半个设备像素，2× 光栅化后与主块之间被淡成纸色——
    /// 视觉上就是「断成两截的双截棍」（用户原话）。
    ///
    /// 判据用**相对 d** 的比例，与字号无关：尾口宽落在 [6%, 16%] 之间。
    /// 下限 6% 保证 d=9.27px 时尾口 ≥ 0.56px 逻辑 → 1.1 设备像素，连得住；
    /// 上限 16% 防止它胖成方头（那就不是「轻轻提起」了）。
    #[test]
    fn seed_tail_is_blunt_not_pinched() {
        for (belly, _) in BELLY_VARIANTS {
            for (d, hook) in [(4.0f32, false), (8.0, false), (9.27, false), (9.6, false), (24.0, false), (9.6, true)] {
                let a = seed_anchors_at(d, hook, belly);
                let w = ((a[6].0 - a[0].0).powi(2) + (a[6].1 - a[0].1).powi(2)).sqrt();
                let ratio = w / d;
                assert!(
                    (0.06..=0.16).contains(&ratio),
                    "尾口宽 {w:.2}px（d 的 {:.1}%）：< 6% 会被抗锯齿淡成断口，> 16% 是方头",
                    ratio * 100.0
                );
            }
        }
    }

    /// **回锋档**：只把尾口那一小段收短——其余五点不动，整枚仍闭合、仍在左上。
    #[test]
    fn hook_pulls_the_tip_back_without_breaking_the_shape() {
        for (belly, _) in BELLY_VARIANTS {
            for d in [4.0f32, 8.0, 9.6, 13.6] {
                let plain = seed_anchors_belly(d, false, belly);
                let hooked = seed_anchors_belly(d, true, belly);
                // 右腰 / 右顿 / 右底 / 底内 / 左顿 五点完全不动（回锋只碰尾口）。
                for i in [1, 2, 3, 4, 5] {
                    assert_eq!(plain[i], hooked[i], "回锋动了第 {i} 个锚点");
                }
                // 尾口回收：回锋档的尾更短（沿笔势往顿笔那头缩）。
                let tip_mid =
                    |a: [(f32, f32); 7]| ((a[0].0 + a[6].0) * 0.5, (a[0].1 + a[6].1) * 0.5);
                let (tp, th) = (tip_mid(plain), tip_mid(hooked));
                let dist = |p: (f32, f32), q: (f32, f32)| {
                    ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt()
                };
                assert!(
                    dist(th, plain[2]) < dist(tp, plain[2]),
                    "回锋没收锋：尾中点 {th:?} 应比 {tp:?} 更靠近顿笔",
                );
                // 仍闭合、尾口仍在**左上**（不能因为回锋就翻到右下去了）。
                assert!(
                    th.0 < plain[2].0 && th.1 < plain[2].1,
                    "回锋把尾口甩到右下：{th:?}"
                );
                // 回锋档同样不许被捏成针尖。
                let w =
                    ((hooked[6].0 - hooked[0].0).powi(2) + (hooked[6].1 - hooked[0].1).powi(2)).sqrt();
                assert!(w / d >= 0.06, "回锋档尾口太尖（{:.1}%），会断", w / d * 100.0);
            }
        }
    }
}
