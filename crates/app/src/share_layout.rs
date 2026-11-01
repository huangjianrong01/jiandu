//! 分享卡的**纯数据版式层**（v1.57.0 重构）
//!
//! 旧制是「GPUI 元素直接拼出来 + `screencapture` 截屏」：截图依赖屏幕录制权限、
//! 分辨率随窗口走、锁屏时直接失败——用户明令**禁止截屏、必须离屏高清出图**。
//! 故把版式抽成与渲染后端无关的一层数据：
//!
//! ```text
//!   LayoutSpec  ──►  layout()  ──►  CardLayout
//!                                      ├─► GPUI 预览（屏幕上的小卡，`paint_gpu`）
//!                                      └─► CoreGraphics 出图（1080×1920 PNG，`share_paint`）
//! ```
//!
//! 两个后端读同一份 `CardLayout`，所以**预览就是成图**：同样的底色、界框、
//! 竖排列款、朱批引线、印章位置。本文件零 GPUI / 零 macOS API，纯函数可单测。
//!
//! 排版铁律（与阅读引擎同宗）：
//! 1. **强制首行顶格**：每一列的第一个字 `y` 恒等于 `y0`，绝不因列长而错动
//!    （对应阅读侧的 `grid::top_baseline`）。且 `y0` **紧贴版框内缘**
//!    （见 [`top_baseline`]）——正文块不再垂直居中，天头不留空档。
//! 2. **从右向左**：第 0 列在最右，符合古籍行款。
//! 3. **禁用 `unwrap`**：越界 / 除零一律走默认值。
//! 4. **边栏 / 界行齐全**（v1.61.0）：版框（外框线）与界行（列间竖线）
//!    由版式层一次算好，绘制层只管按序落笔——预览与成图同源。

use crate::share::{ShareBg, ShareFrame, ShareSeal, frame_ink, norm_columns, palette};

/// 分享卡正文的字体**语义类**（离屏渲染器据此挑系统 PostScript 字体）。
///
/// GPUI 侧用的是注册过的家族名（文津宋体 / 霞鹜文楷…），那些字体不一定在系统里
/// 有同名 PostScript 字体；CoreGraphics 只认 PostScript 名，故另立一档语义。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShareFont {
    #[default]
    /// 宋体（刻本正统）。
    Song,
    /// 楷体（手写气）。
    Kai,
    /// 黑体（现代极简）。
    Hei,
}

impl ShareFont {
    /// 离屏渲染用的候选 PostScript 名（按序回退，第一个系统真有的生效）。
    pub fn postscript_chain(self) -> &'static [&'static str] {
        match self {
            ShareFont::Song => &["Songti SC", "STSong", "SimSun", "Songti TC", "serif"],
            ShareFont::Kai => &["Kaiti SC", "STKaiti", "KaiTi", "Kaiti TC", "serif"],
            ShareFont::Hei => &["PingFang SC", "Heiti SC", "Heiti TC", "sans-serif"],
        }
    }
}

/// 单字的 **Markdown 装饰位**（由 [`clean_md`] 产出，与 `LayoutSpec::body` 等长）。
///
/// 只记「这个字要怎么印」，不记语法本身——`#` / `**` 在清洗期就已经从串里
/// 剔干净了，版式层拿到的永远是纯文字 + 一排小旗子。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MdMark {
    /// 标题字（`#` 行）：字号放大一档，**字格不动**（见 [`HEAD_FS_RATIO`]）。
    pub head: bool,
    /// 加粗字（`**…**`）：离屏落笔时同字加厚，不换字体。
    pub bold: bool,
}

/// 一枚落位的字（字格左上为锚点，绘制时按字格居中）。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedChar {
    pub ch: char,
    /// 字格左上 x / y。
    pub x: f32,
    pub y: f32,
    /// 字格边长（正方形字格，正文与批注同制）。
    pub cell: f32,
    /// 字号。
    pub fs: f32,
    /// 墨色（`0xRRGGBB`）。
    pub color: u32,
    /// 标点：竖排里靠字格**右上**安置，不占整格（见 `punct_box`）。
    pub punct: bool,
    /// 列首句读被**悬挂**到网格右缘之外（禁则·避头尾）：不占列内格位，
    /// 正文首字因此仍钉在基准线上，绝不被往下挤。
    pub hung: bool,
    /// 加粗（来自 Markdown `**…**`）：光栅层用「同字微量偏移再落两笔」加厚。
    /// 不换字体面——系统宋体未必有 PostScript Bold 名，回退链一断整张卡就没了。
    pub bold: bool,
}

/// 一块矩形：填充（`stroke = None`）或描边（`stroke = Some(线宽)`）。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `0xRRGGBBAA`。
    pub color: u32,
    pub stroke: Option<f32>,
}

/// 一条线（朱批引线 / 界行）。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLine {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    /// `0xRRGGBBAA`。
    pub color: u32,
    pub width: f32,
}

/// 朱印。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSeal {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub kind: ShareSeal,
    pub text: String,
}

/// 一张卡的全部绘制指令。
#[derive(Clone, Debug, Default)]
pub struct CardLayout {
    pub w: f32,
    pub h: f32,
    /// 底色 `0xRRGGBB`。
    pub bg: u32,
    /// 正文字体语义（离屏渲染据此挑系统 PostScript 字体）。
    pub font: ShareFont,
    pub rects: Vec<PlacedRect>,
    /// **界行**（列间竖细线，v1.61.0）：绘制序在 `rects` 之后、`chars` 之前
    /// ——线在下、字在上，绝不让界行压住墨迹。
    pub rules: Vec<PlacedLine>,
    pub lines: Vec<PlacedLine>,
    pub chars: Vec<PlacedChar>,
    pub seal: Option<PlacedSeal>,
    /// 本幅**能装多少字**（＝网格列数 × 行数，与本次喂进来的字数无关）。
    ///
    /// v1.21.0：分页要用它。原来「一页能装多少字」是调用方按 720×1280 手写的
    /// 经验表（8→50 / 12→110…），换个幅面（A4 / A3）整张表就作废。现在版式
    /// 自己把实测容量交出来，任何幅面都能一步问出「这页能放多少字」。
    pub cols_cap: usize,
    pub rows_cap: usize,
}

impl CardLayout {
    /// 整幅容量（字）。`0` ＝ 还没排过（`Default`）。
    pub fn capacity(&self) -> usize {
        self.cols_cap.saturating_mul(self.rows_cap)
    }
}

/// 版式输入（调用方快照，渲染期只读）。
pub struct LayoutSpec<'a> {
    pub body: &'a [char],
    /// 与 `body` 等长：第 i 字是否被用户划了红线（朱批高亮）。
    pub red: &'a [bool],
    /// 与 `body` 等长：第 i 字的 Markdown 装饰位（标题 / 加粗）。
    ///
    /// **空切片 = 全部正文**——阅读器出图、分享卡等既有调用点根本不知道
    /// Markdown 的存在，它们传 `&[]`，版式与 v1.9.x 逐像素一致。
    pub md: &'a [MdMark],
    /// 眉批正文（已裁切；空 = 不画批注列）。
    pub note: &'a str,
    pub show_notes: bool,
    /// 落款：书名 · 章节名。
    pub sign: &'a str,
    pub show_sign: bool,
    pub date: &'a str,
    pub show_date: bool,
    pub columns: usize,
    pub bg: ShareBg,
    pub frame: ShareFrame,
    pub seal: ShareSeal,
    pub seal_text: &'a str,
    pub font: ShareFont,
    /// **出图时间戳**（v1.82.0）：一行极简小字画在**右下地脚带**里。
    /// 空串 = 不画。
    ///
    /// 它与 `date` 是两回事：`date` 走「版心内左下竖排列」，会占 `reserve`
    /// 竖带、动到正文网格宽；时间戳**一个数都不改**，只往地脚留白里补一行。
    /// 「不破坏版面」靠的就是这个分家。
    pub stamp: &'a str,
    pub w: f32,
    pub h: f32,
}

/// 行高 / 字格比：一格 = 字号 × 1.55（竖排古籍的行款，字与字之间有气口）。
pub const CELL_RATIO: f32 = 1.55;
/// 朱批高亮底的透明度（极淡朱砂，不盖字）。
pub const MARK_ALPHA: u8 = 0x26;
/// 朱批小字与正文的字号比。
pub const NOTE_FS_RATIO: f32 = 0.44;
/// 落款小字与正文的字号比。
pub const META_FS_RATIO: f32 = 0.40;
/// **出图时间戳的字号**（px，1080 基准，随卡宽缩放）：全卡最小的一档——
/// 水印只要「看得见、认得出」，绝不能抢正文。硬下限 9px：再小就成噪点了。
pub const STAMP_FS: f32 = 11.0;
/// 时间戳的**淡度**：`0` = 全纸色（看不见），`1` = 满墨色。
/// 0.5 是「比落款再淡一档」——落款是给人读的，时间戳只是个记号。
pub const STAMP_FADE: f32 = 0.50;
/// 朱批引线宽（px，按卡片宽缩放后的值）。
pub const LEAD_W: f32 = 1.0;

// ----------------------------------------------------------------------
// v1.61.0：版框（边栏）与界行（列间竖线）
//
// 古籍版刻的三件套是「边栏 + 界行 + 中缝」：边栏是四周的框线，界行是界在
// 列与列之间的竖细线。刻本里两者同色、粗细有别——边栏粗（朱丝栏 2px 上下）、
// 界行细（1px 且更淡），字始终压在最上层。
// ----------------------------------------------------------------------

/// **版框（边栏）线宽**（px，以 1080 宽为基准，按卡宽缩放）。
pub const FRAME_W: f32 = 2.0;
/// **界行（列间竖线）线宽**（px，1080 基准）——比版框细，绝不抢字。
pub const RULE_W: f32 = 1.0;
/// 版框墨色不透明度（`0xRRGGBBAA` 的低字节）：实心但不刺眼。
pub const FRAME_ALPHA: u32 = 0xC8;
/// **双边的内框**不透明度（v1.83.0）：比外框淡一档。
///
/// 照片上量的就是这个层次——外框浓黑压得住整版，内框与界行同属「退一档
/// 的灰」。两者同浓的话，双线看着像两圈一样重的框，反而比单线更乱。
pub const INNER_ALPHA: u32 = 0x96;
/// **双边内外框之间的气口**（px，1080 基准，随卡宽缩放）。
///
/// 教科书上的文武边栏约「外线一分、气口半分、内线半分」。2px 在 1080 卡上
/// 就是那个半分——再宽，双线会散成两个不相干的框。
pub const DOUBLE_GAP: f32 = 2.0;
/// 界行墨色不透明度：比版框**淡一档**，只作分列的骨架。
pub const RULE_ALPHA: u32 = 0x38;
/// 版框内缘 → 首字顶端的**气口**（px，1080 基准）：字顶不压线，也不留天头。
pub const TOP_GAP: f32 = 4.0;

/// **列首字的绝对基准线**（v1.59.0）：所有列的第一个字，顶边一律从它起算。
///
/// 取 0 是刻意的——基准线本身不带任何位移，位移只来自版框天头（见
/// [`top_baseline`]）。渲染层（GPUI 预览 / CoreGraphics 出图）都只读这一个
/// 常量，谁也不许就地加偏移：两处算法一旦分家，顶格就悄悄漂了。
pub const TOP_BASELINE: f32 = 0.0;

/// 列首基准线的**唯一取用口径**：`TOP_BASELINE + 版框内缘 + 气口`。
///
/// v1.61.0 起**不再垂直居中**：正文块的上沿与版框内缘（框线的下缘）只隔一个
/// [`TOP_GAP`]——第一个字的最高点正好卡在框线下方的起点上，不压线、不悬空。
/// 这是「首行顶格」与「版框」的视觉联动，唯一允许的天头位移就是这一段气口。
pub fn top_baseline(inner_top: f32, scale: f32) -> f32 {
    TOP_BASELINE + inner_top + TOP_GAP * scale
}

/// **悬挂沟**：网格右缘外侧留出的一条竖带（字格比例），专供列首句读挂靠。
///
/// 有了它，句读被推出列外时**不会画出画布 / 画到版框上**，正文也照旧顶格。
pub const HANG_GUTTER: f32 = 0.62;
/// 悬挂句读与网格右缘之间的横向间隙（字格比例）。
pub const HANG_GAP: f32 = 0.08;

/// **列首标点**（逗号 / 句号等）挂到版框**外侧**（最右列之右）时的横向外探量
/// （字格比例）。标点被挂出去后**不再占列内格位**，正文首字因此仍钉在
/// [`TOP_BASELINE`] 上——禁则（避头尾）绝不牺牲顶格。
pub const HEAD_PUNCT_OUTSET: f32 = 0.30;
/// 多枚列首标点连排时，在框外的纵向错开量（字格比例），避免叠成一坨。
pub const HEAD_PUNCT_STEP: f32 = 0.60;

/// 竖排里需要「靠右上安置」的标点（占半格、不顶格）。
pub fn is_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '、' | '；' | '：' | '！' | '？' | '「' | '」' | '『' | '』'
            | '《' | '》' | '〈' | '〉' | '（' | '）' | '—' | '…' | ',' | '.' | ';'
            | ':' | '!' | '?' | '(' | ')' | '　' | ' '
    )
}

/// 标点的落位盒：字格**右上**象限，字号六成。
///
/// 被悬挂的句读（[`PlacedChar::hung`]）走另一路：它的 `x` 已经是「网格右缘
/// 之外」的最终落点，故不再加 `0.42` 格的内偏移，只留 [`HANG_GAP`] 的气口。
pub fn punct_box(p: &PlacedChar) -> (f32, f32, f32) {
    if p.hung {
        (p.x + p.cell * 0.02, p.y + p.cell * 0.10, p.fs * 0.62)
    } else {
        // 竖排古籍的句读挂在字的右上方，不占整格——正文仍按整格推进。
        (p.x + p.cell * 0.42, p.y + p.cell * 0.04, p.fs * 0.62)
    }
}

// ----------------------------------------------------------------------
// 纸张口径（v1.21.0，v1.22.0 加拼版）——「排版物理尺寸参数化」的唯一换算口
//
// 版式层从头到尾只吃两个数：`LayoutSpec.w / .h`。所以支持 A3 **不需要**第二套
// 排版代码，只需要有人回答「这一页是多少像素」。本段就是那个唯一答案：
//     mm --(dpi)--> px        `page_geom()`      排版 / 出图（PDF 走 72dpi＝pt）
//     mm --(比例)--> px        `preview_*_geom()` 屏幕取景（长边封顶）
// 调用方一律经这里取尺寸，代码里再不许出现「1080×1920」「720×1280」这类魔数。
//
// ⚠️ v1.22.0 起**两个尺寸分家**，绝不能混用：
//     page_*  单页尺寸 —— 喂 `layout` 的 `w/h`，决定列数、字号、切字块；
//     sheet_* 整张纸   —— PDF 媒体框 / 拼版画布，双页拼版时是横的、宽一倍。
// 混用一次就是「整张 A3 排一页」的大字海报（v1.21.0 的原罪）。
//
// 为什么 PDF 用 72dpi：PDF 是矢量，坐标单位是 pt（1/72 英寸）。A3 单页 = 297×420mm
// 恰好 = 841.89×1190.55pt；双页拼版整张 = 420×297mm = 1190.55×841.89pt。
// 用 72dpi 排出来的「像素」与 pt 一一对应，字级直接就是磅值，缩放由阅读器
// 无限插值——绝不靠 300dpi 位图去凑清晰度。
// ----------------------------------------------------------------------

/// 一张纸的规格（`mm()` 返回**竖放**的单页口径，宽 < 高，与竖排版式同向）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Paper {
    /// 屏幕预览小卡：9:16 取景，**不是**真实纸张（沿用 v1.9.x 的观感基准）。
    #[default]
    Card,
    /// A4（210 × 297 mm）。
    A4,
    /// A3（297 × 420 mm）——印厂常用的两张 A4 拼版大纸。
    A3,
}

/// **拼版方式**（v1.22.0）：一件印刷品怎么摊在纸上。
///
/// 这是 v1.21.0 留下的那个问题的正解。那一版把 A3 当成「更大的单页」，
/// 结果是**把一页正文放大铺满整张 A3**——大字海报，不是书。真正的书是
/// 折页的：一张大纸上并排两个单页，沿中缝一折就是左右两页。
///
/// ```text
///   Single（單頁）              Spread（雙頁拼版 · 右開本）
///   ┌──────────┐               ┌──────────┬───┬──────────┐
///   │          │               │  第 1 頁 │中 │  第 2 頁 │
///   │  一 頁   │               │  （奇）  │縫 │  （偶）  │
///   │          │               │          │   │          │
///   └──────────┘               └──────────┴───┴──────────┘
///      210×297                    420×297（横）
/// ```
///
/// 关键纪律有两条，缺一条就不是书：
/// 1. **排版引擎只吃「半页」的宽高**（`Paper::page_mm`），绝不能拿整张 A3 的
///    宽度去算列数——那正是海报的来路；
/// 2. **两个单页各自在属于自己的半边上居中**（`spread_slots`），中间留中缝，
///    绝不把一页拉宽去铺满整张纸。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Imposition {
    /// 一页一张纸（A4 就是 A4，A3 就是竖放的大页）。
    #[default]
    Single,
    /// 双页拼版：整张纸**横放**，左右各一个单页，中间留中缝（书籍折页）。
    Spread,
}

impl Imposition {
    /// 两档，UI 按此顺序排列。
    pub const ALL: [Imposition; 2] = [Imposition::Single, Imposition::Spread];

    /// 面板上的显示名。
    pub fn label(self) -> &'static str {
        match self {
            Imposition::Single => "單頁",
            Imposition::Spread => "雙頁拼版",
        }
    }

    /// 是否双页拼版（预览 / 导出处处要判这一句）。
    pub fn is_spread(self) -> bool {
        matches!(self, Imposition::Spread)
    }
}

/// 折页**中缝**的宽度（mm）——用户 2026-11-01 硬指令：**至少 30pt**。
///
/// A3 横向 1191pt，左右两个半区各 595.5pt，两个半区之间必须空出一条中缝，
/// 且左右内容绝不能越界重叠。30pt = 30 × 25.4 / 72 ≈ 10.583mm。
///
/// 它从整张纸的宽度里**先扣掉**，剩下的才平分给两个单页：
///
/// ```text
///   1191pt = 580.3 + 30 + 580.3        （420mm 纸）
///            └左页┘  中缝  └右页┘
/// ```
///
/// 所以中缝不是「事后在两页之间画的一条缝」——它压根不算进任何一页的
/// 排版宽度。两页版框之外各自还有留白，纸面正中那道白因此比 30pt 还宽，
/// 这正是刻本版心之间的常态（「黑口 / 白口」本来就是空的）。
pub const SPREAD_GUTTER_MM: f32 = 30.0 * 25.4 / 72.0;

/// 中缝的磅值下限（pt）——排版与测试都只认这一个数。
pub const SPREAD_GUTTER_PT: f32 = 30.0;

/// **装订模式**（v1.22.0）：折页后书脊在哪边，决定奇偶两页谁在左谁在右。
///
/// 用户明令（2026-11-01）：
/// * **右开本**（古籍线装 / 竖排，默认）——**奇左偶右**：第 1 页在左，第 2 页在右；
/// * **左开本**（现代横排西书）——**偶左奇右**：第 2 页在左，第 1 页在右。
///
/// 别拿「竖排从右往左读」去反推：那是**行内**的读序；折页成册后哪一页在左，
/// 由装订线在哪边定，与行序是两件事。故这里独立成档，不跟版式方向绑定。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Binding {
    /// 右开本（古籍 / 线装）：奇数页在左半边，偶数页在右半边。
    #[default]
    Right,
    /// 左开本（现代 / 横排）：偶数页在左半边，奇数页在右半边。
    Left,
}

impl Binding {
    /// 两档，UI 按此顺序排列。
    pub const ALL: [Binding; 2] = [Binding::Right, Binding::Left];

    /// 面板上的显示名。
    pub fn label(self) -> &'static str {
        match self {
            Binding::Right => "右開本",
            Binding::Left => "左開本",
        }
    }

    /// 补一句人话（浮层标题里用），把「奇偶怎么摆」说清，避免用户猜。
    pub fn hint(self) -> &'static str {
        match self {
            Binding::Right => "古籍 · 奇左偶右",
            Binding::Left => "現代 · 偶左奇右",
        }
    }
}

/// 双页拼版时两个单页在纸上的**横向落点**（左缘 x，单位同 `page_w`）。
///
/// 返回 `[奇数页的 x, 偶数页的 x]`——顺序是**页序**，不是左右；谁在左由
/// `binding` 定。落法极简：两页各贴纸的一侧，中间剩下的**全部**是中缝。
///
/// ```text
///   右開本：[0, sheet − page]      左页第 1 页 · 右页第 2 页
///   左開本：[sheet − page, 0]      左页第 2 页 · 右页第 1 页
/// ```
///
/// 于是中缝宽度恒等于 `sheet − 2 × page`（＝ [`SPREAD_GUTTER_PT`]），两页
/// **绝不可能重叠**、也绝不越出纸外——不需要任何额外钳制。
pub fn spread_slots(sheet_w: f32, page_w: f32, binding: Binding) -> [f32; 2] {
    let right = (sheet_w - page_w).max(page_w);
    match binding {
        // 奇左偶右
        Binding::Right => [0.0, right],
        // 偶左奇右
        Binding::Left => [right, 0.0],
    }
}

/// 纸张的像素尺寸（＝版式层的 `w / h`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeom {
    pub w: f32,
    pub h: f32,
}

impl PageGeom {
    /// 宽高比（宽 / 高）。竖放纸恒 < 1；预览据此等比贴图，绝不被容器裁切。
    pub fn aspect(self) -> f32 {
        if self.h.abs() < 1e-6 {
            1.0
        } else {
            self.w / self.h
        }
    }
}

/// 预览光栅的**长边上限**（像素）：A3 只按比例取样，绝不按 300dpi 全尺寸贴图。
///
/// A3@300dpi 是 3508×4961（位图约 70 MB），解码 / 上纹理足以把主线程压死；
/// 预览只需「看出纸形」，真分辨率留给出图那一刻（见 [`Paper::geom`]）。
pub const PREVIEW_LONG_EDGE: f32 = 1280.0;

/// 毫米 → 像素（`dpi` 下的取整值）。`None`（不是真实纸张）走「取景卡」720×1280。
fn geom_of_mm(mm: Option<(f32, f32)>, dpi: f32) -> PageGeom {
    match mm {
        None => PageGeom {
            w: 720.0,
            h: 1280.0,
        },
        Some((mw, mh)) => {
            let k = dpi.max(1.0) / 25.4;
            PageGeom {
                w: (mw * k).round(),
                h: (mh * k).round(),
            }
        }
    }
}

/// 屏幕预览尺寸：保住纸的宽高比，长边封顶到 [`PREVIEW_LONG_EDGE`]。
fn preview_of(g: PageGeom) -> PageGeom {
    let long = g.w.max(g.h).max(1.0);
    let k = PREVIEW_LONG_EDGE / long;
    PageGeom {
        w: (g.w * k).round().max(1.0),
        h: (g.h * k).round().max(1.0),
    }
}

impl Paper {
    /// 三档，UI 按此顺序排列。
    pub const ALL: [Paper; 3] = [Paper::Card, Paper::A4, Paper::A3];

    /// 面板上的显示名。
    pub fn label(self) -> &'static str {
        match self {
            Paper::Card => "預覽卡",
            Paper::A4 => "A4",
            Paper::A3 => "A3",
        }
    }

    /// **单页拼版**的纸幅（竖放）；[`Paper::Card`] 不是真实纸张，返回 `None`。
    pub fn mm(self) -> Option<(f32, f32)> {
        match self {
            Paper::Card => None,
            Paper::A4 => Some((210.0, 297.0)),
            Paper::A3 => Some((297.0, 420.0)),
        }
    }

    /// **双页拼版**的整张纸：把单页纸幅**横过来**（宽 > 高）。
    ///
    /// A3 = 420×297mm。用户明令「A3 应该是横向的」——纵向摊两个竖排单页，
    /// 纸只能是横的。
    pub fn spread_sheet_mm(self) -> Option<(f32, f32)> {
        self.mm().map(|(w, h)| (h, w))
    }

    /// **双页拼版里单个页面**的排版尺寸（竖放）。
    ///
    /// ＝ (整宽 − [`SPREAD_GUTTER_MM`]) ÷ 2，高不变。
    ///
    /// A3：整宽 420mm、中缝 10.583mm → 单页 204.71mm × 297mm
    /// ＝ **580.3pt × 842pt**（半区 595.3pt 减去中缝的一半）。版心网格、
    /// 列数、切字块**只准认这一组数**，绝不能用整张 A3 的宽度去算（那是海报）。
    ///
    /// 两页 + 一条中缝正好铺满整张纸：`580.3 × 2 + 30 = 1190.6pt`。
    pub fn spread_page_mm(self) -> Option<(f32, f32)> {
        self.spread_sheet_mm()
            .map(|(sw, h)| (((sw - SPREAD_GUTTER_MM).max(40.0)) * 0.5, h))
    }

    /// 当前拼版下**整张纸**的毫米尺寸（PDF 媒体框只认它）。
    pub fn sheet_mm(self, imp: Imposition) -> Option<(f32, f32)> {
        match imp {
            Imposition::Single => self.mm(),
            Imposition::Spread => self.spread_sheet_mm(),
        }
    }

    /// 当前拼版下**单个页面**的毫米尺寸（版心网格只认它）。
    pub fn page_mm(self, imp: Imposition) -> Option<(f32, f32)> {
        match imp {
            Imposition::Single => self.mm(),
            Imposition::Spread => self.spread_page_mm(),
        }
    }

    /// **排版物理尺寸**（单页拼版口径，保留给老调用点）：`dpi` 下这张纸有多少像素。
    ///
    /// * `Card` 不吃 DPI（屏幕取景与分辨率无关），恒 720×1280；
    /// * `A4 / A3` 按 `mm / 25.4 * dpi` 取整——四舍五入到整像素，避免半像素
    ///   边框在 PDF 里被抗锯齿糊成两行灰边。
    pub fn geom(self, dpi: f32) -> PageGeom {
        geom_of_mm(self.mm(), dpi)
    }

    /// 当前拼版下**单页**的像素尺寸——喂给 `layout` 的 `w / h` 就是它。
    pub fn page_geom(self, imp: Imposition, dpi: f32) -> PageGeom {
        geom_of_mm(self.page_mm(imp), dpi)
    }

    /// 当前拼版下**整张纸**的像素尺寸——PDF 画布 / 拼版位图的外框。
    pub fn sheet_geom(self, imp: Imposition, dpi: f32) -> PageGeom {
        geom_of_mm(self.sheet_mm(imp), dpi)
    }

    /// PDF 的**矢量口径**（单页拼版）：72dpi 下的尺寸＝磅值（pt）。
    pub fn pdf_geom(self) -> PageGeom {
        self.geom(72.0)
    }

    /// PDF 矢量口径的单页尺寸（排版用）。
    pub fn page_pdf_geom(self, imp: Imposition) -> PageGeom {
        self.page_geom(imp, 72.0)
    }

    /// PDF 矢量口径的整张纸尺寸（写媒体框用）。
    pub fn sheet_pdf_geom(self, imp: Imposition) -> PageGeom {
        self.sheet_geom(imp, 72.0)
    }

    /// **屏幕预览**尺寸（单页拼版口径，保留给老调用点）。
    pub fn preview_geom(self) -> PageGeom {
        preview_of(self.geom(72.0))
    }

    /// 屏幕预览里**单页**的尺寸（竖放）。
    ///
    /// ⚠️ 拼版时它**不能**各按长边封顶：单页的长边是「高」，整张纸的长边是
    /// 「宽」，各封一次就得到两套比例尺——页面相对纸偏大，摆上去必然互相压叠。
    /// 故 Spread 走「与整张纸同一把尺子」：按纸宽的毫米比例取宽，高直接跟纸。
    pub fn preview_page_geom(self, imp: Imposition) -> PageGeom {
        match imp {
            Imposition::Single => preview_of(self.page_geom(imp, 72.0)),
            Imposition::Spread => {
                let sheet = self.preview_sheet_geom(imp);
                let (sw, _) = self.sheet_mm(imp).unwrap_or((1.0, 1.0));
                let (pw, _) = self.page_mm(imp).unwrap_or((sw * 0.5, 1.0));
                let k = if sw > 1e-6 { pw / sw } else { 0.5 };
                PageGeom {
                    w: (sheet.w * k).round().max(1.0),
                    h: sheet.h,
                }
            }
        }
    }

    /// 屏幕预览里**整张纸**的尺寸（双页拼版时是横的）。
    pub fn preview_sheet_geom(self, imp: Imposition) -> PageGeom {
        preview_of(self.sheet_geom(imp, 72.0))
    }
}

// ----------------------------------------------------------------------
// Markdown 清洗器（v1.20.0）——「源码编辑 + 预览解析」的解析那一半
//
// 笺校工作台允许直接导入 .md：右侧编辑器**原样显示源码**（`#`、`**`、`---`
// 都在，用户改的是原文），只有竖排预览出图前才过这道清洗器。这样校对精度
// 不受影响——源码里第几行第几字，用户眼睛看得见、手也改得到；成图里则是
// 干净的竖排文字。
//
// 清洗只做五件事（刻意不做完整 Markdown 语义，古籍稿用不到）：
//   ① 块级标记整行丢：`---` / `***` / `___` / `===` 分隔线；
//   ② 块级前缀剥掉：`#`（并记成标题）、`>` 引用、`- ` / `1. ` 列表；
//   ③ 行内标记剥掉：`**` / `__`（记成加粗）、`*` / `_` 斜体、`` ` `` 代码；
//   ④ 链接 / 图片只留文字：`[文](url)` → `文`；
//   ⑤ 表格竖线、行内 HTML 标签一并剔掉。
//
// 全部按 `char` 走，绝不用字节下标切 CJK——那是 v1.9.18 崩过一回的老坑。
// ----------------------------------------------------------------------

/// 标题字相对正文的**字号倍率**（`#` 行）。
///
/// 只放大字号、**不动字格**：竖排网格的列宽 / 行数 / 界行全由 `cell` 定，
/// 一格一变大，整版网格立刻散架。1.28 是「一眼看出是标题、又不挤到邻格」
/// 的那一档（放大后字身约占格高 0.71，上下仍有余气）。
pub const HEAD_FS_RATIO: f32 = 1.28;
/// 放大后的字号上限（字格比例）：即便标题倍率被调大，墨迹也绝不撑破本格。
pub const HEAD_CELL_FILL: f32 = 0.86;

/// Markdown 清洗结果：只剩「要印出来的字」，每字配一枚装饰位。
#[derive(Clone, Debug, Default)]
pub struct CleanMd {
    pub body: Vec<char>,
    pub md: Vec<MdMark>,
}

impl CleanMd {
    /// 清洗后的纯文本（不含任何 Markdown 记号）。
    pub fn text(&self) -> String {
        self.body.iter().collect()
    }
}

/// 把 Markdown 源码洗成「可竖排的纯文字 + 装饰位」。
///
/// 幂等：拿清洗结果再洗一遍，字一个不变（回归测试钉着）。
pub fn clean_md(src: &str) -> CleanMd {
    let mut out = CleanMd::default();
    for raw in src.split('\n') {
        let line = raw.trim_end_matches('\r');
        let mut t = line.trim();
        if t.is_empty() {
            continue; // 空行不留空白格，段落自然接排（竖排里换行只是排版噪声）
        }
        if is_md_rule(t) {
            continue; // ① 分隔线：整行丢弃，绝不印成一道横杠
        }
        // ② 块级前缀：可叠加（`> - 文字` / `> 1. 文字`），最多剥四层防死循环。
        let mut guard = 0;
        loop {
            let before = t;
            if let Some(rest) = t.strip_prefix('>') {
                t = rest.trim_start();
            }
            if let Some(rest) = strip_list_marker(t) {
                t = rest;
            }
            if t == before || guard >= 4 {
                break;
            }
            guard += 1;
        }
        if t.is_empty() {
            continue;
        }
        // ②b ATX 标题：`#`～`######` + 空白开头才认（`#tag` 不是标题）。
        let mut head = false;
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && hashes <= 6 {
            let after = &t[hashes..]; // '#' 是 ASCII：字节下标恰好等于个数
            if after.is_empty() || after.starts_with(' ') || after.starts_with('\t') {
                head = true;
                // 收尾的 `#`（`## 标题 ##`）一并剥掉。
                t = after.trim().trim_end_matches('#').trim_end();
            }
        }
        if t.is_empty() {
            continue;
        }
        inline_md_into(t, head, &mut out);
    }
    out
}

/// 整行是否是一条 Markdown 分隔线（`---` / `***` / `___` / `===`，允许空格）。
fn is_md_rule(t: &str) -> bool {
    let cs: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    if cs.len() < 3 {
        return false;
    }
    let first = cs[0];
    if !matches!(first, '-' | '*' | '_' | '=') {
        return false;
    }
    cs.iter().all(|c| *c == first)
}

/// 剥列表前缀（`- ` / `* ` / `+ ` / `1. ` / `1) ` / `1、`），返回余下内容。
fn strip_list_marker(t: &str) -> Option<&str> {
    if let Some(c) = t.chars().next() {
        if matches!(c, '-' | '*' | '+') {
            let rest = &t[c.len_utf8()..];
            return rest.strip_prefix(' ').map(|r| r.trim_start());
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits <= 3 {
        let rest = &t[digits..]; // ASCII 数字：字节数 == 个数
        if let Some(c) = rest.chars().next() {
            if matches!(c, '.' | ')' | '、') {
                let tail = &rest[c.len_utf8()..];
                if let Some(r) = tail.strip_prefix(' ') {
                    return Some(r.trim_start());
                }
            }
        }
    }
    None
}

/// 剥行内标记，把干净的字连同装饰位推进 `out`。
fn inline_md_into(line: &str, head: bool, out: &mut CleanMd) {
    let cs: Vec<char> = line.chars().collect();
    let mut buf: Vec<(char, bool)> = Vec::with_capacity(cs.len());
    let mut i = 0usize;
    let mut bold = false;
    while i < cs.len() {
        let c = cs[i];
        // 反斜杠转义：`\*` 出 `*`，`\\` 出 `\`
        if c == '\\' && i + 1 < cs.len() {
            buf.push((cs[i + 1], bold));
            i += 2;
            continue;
        }
        // 行内代码：反引号本身丢，里面的字照印
        if c == '`' {
            i += 1;
            continue;
        }
        // ③ 加粗 `**` / `__`：成对翻转，跨行自然复位（本函数一次只吃一行）
        if (c == '*' || c == '_') && cs.get(i + 1) == Some(&c) {
            bold = !bold;
            i += 2;
            continue;
        }
        // ③b 斜体 `*`：记号丢、字留
        if c == '*' {
            i += 1;
            continue;
        }
        // ③c 下划线只在「两侧不都是字母数字」时当标记（保住 `foo_bar`）
        if c == '_' {
            let prev = if i > 0 { cs[i - 1] } else { ' ' };
            let next = cs.get(i + 1).copied().unwrap_or(' ');
            if !(prev.is_alphanumeric() && next.is_alphanumeric()) {
                i += 1;
                continue;
            }
        }
        // ④ 图片 `![alt](url)`：`!` 丢掉，剩下的 `[alt](url)` 交给下一轮
        if c == '!' && cs.get(i + 1) == Some(&'[') {
            i += 1;
            continue;
        }
        if c == '[' {
            if let Some((txt, next)) = take_bracket(&cs, i) {
                for ch in txt.chars() {
                    buf.push((ch, bold));
                }
                i = next;
                continue;
            }
        }
        // ⑤ 表格竖线当空格；行内 HTML 标签整段丢弃
        if c == '|' {
            buf.push((' ', bold));
            i += 1;
            continue;
        }
        if c == '<' {
            if let Some(j) = cs[i..].iter().position(|c| *c == '>') {
                i += j + 1;
                continue;
            }
        }
        buf.push((c, bold));
        i += 1;
    }
    for (ch, b) in buf {
        out.body.push(ch);
        out.md.push(MdMark { head, bold: b });
    }
}

/// `[文字](目标)` → `("文字", 目标之后的下标)`；不是链接就返回 `None`。
fn take_bracket(cs: &[char], i: usize) -> Option<(String, usize)> {
    let close = cs[i..].iter().position(|c| *c == ']')? + i;
    if close == i {
        return None; // `[]` 空文字：不是链接，交给后面照常印
    }
    let text: String = cs[i + 1..close].iter().collect();
    let mut next = close + 1;
    if cs.get(next) == Some(&'(') {
        if let Some(j) = cs[next..].iter().position(|c| *c == ')') {
            next += j + 1;
        }
    }
    Some((text, next))
}

/// 排一张卡：底色 → 界框 → 竖排正文（含高亮 / 批注列 / 引线）→ 落款 → 朱印。
pub fn layout(s: &LayoutSpec) -> CardLayout {
    let p = palette(s.bg);
    let w = if s.w.is_finite() { s.w } else { 1080.0 }.max(240.0);
    let h = if s.h.is_finite() { s.h } else { 1920.0 }.max(360.0);
    // 缩放：所有线宽 / 气口都以 1080 宽为基准，小卡同步收细，绝不糊成粗杠。
    let scale = (w / 1080.0).clamp(0.5, 2.0);
    // 版框外缘留白：左右按宽、上下按高各取一次（竖长卡的天头地脚要更从容）。
    //
    // v1.21.0：上限**随幅面线性放宽**。原来上限钉死 46/72px——那在 1080 卡上
    // 是对的，但 A3@300dpi（3508px 宽）会算出「纸上只有 3.9mm 白边」，印出来
    // 是笑话。现在 `cap × (w/1080)`：1080 及以下**一位小数都不变**（cap 仍是
    // 46/72），A3 则自然放大到 ≈12.6mm 的天头地脚，是正常刻本的版框留白。
    let pad_cap_x = 46.0 * (w / 1080.0).max(1.0);
    let pad_cap_y = 72.0 * (h / 1920.0).max(1.0);
    let pad_x = (w * 0.075).clamp(18.0, pad_cap_x);
    let pad_y = (h * 0.035).clamp(18.0, pad_cap_y);
    let fw = FRAME_W * scale;

    let mut out = CardLayout {
        w,
        h,
        bg: p.bg,
        font: s.font,
        ..Default::default()
    };

    // ---- 版框（边栏）：一整圈矩形框线，朱丝栏取朱砂、乌丝栏取墨 ----
    //
    // v1.61.0：旧制是「内外两条细线」，框感太弱且不与首行咬合。现在只画一
    // 圈实心粗线（1080 卡上 2px），且**描边骑在路径上**（线宽一半在内、一半
    // 在外），故框的「内缘」= `inset + fw/2`——首行基准线就认这条内缘。
    //
    // v1.83.0：`ShareFrame::Double` 把「双线」请回来，但**不是**旧制那两条
    // 同粗的细线——外粗内细（文武边栏），中间隔一道气口。刻本的层次全靠
    // 这个粗细差，不是靠调淡墨色。
    let border = frame_ink(s.frame, &p);
    let double = s.frame == ShareFrame::Double;
    // 内细线宽（外框的一半）与气口（1080 基准，随卡宽缩放）。
    let fi = (fw * 0.5).max(0.6);
    let dg = if double { DOUBLE_GAP * scale } else { 0.0 };
    if let Some(ink) = border {
        out.rects.push(PlacedRect {
            x: pad_x,
            y: pad_y,
            w: (w - pad_x * 2.0).max(20.0),
            h: (h - pad_y * 2.0).max(20.0),
            color: (ink << 8) | FRAME_ALPHA,
            stroke: Some(fw),
        });
        if double {
            // 内框：整圈细线，向内让 `fw/2 + dg`（外框内缘 + 气口）。
            let o = fw * 0.5 + dg;
            out.rects.push(PlacedRect {
                x: pad_x + o,
                y: pad_y + o,
                w: (w - (pad_x + o) * 2.0).max(20.0),
                h: (h - (pad_y + o) * 2.0).max(20.0),
                color: (ink << 8) | INNER_ALPHA,
                stroke: Some(fi),
            });
        }
    }

    // ---- 版框内的工作区（版心）----
    // 双边时再往里退一条内细线：正文绝不压在内框上。
    let frame_pad = fw * 0.5 + if double { dg + fi } else { 0.0 };
    let inner_l = pad_x + frame_pad;
    let inner_r = (w - pad_x - frame_pad).max(inner_l + 40.0);
    let inner_t = pad_y + frame_pad;
    let inner_b = (h - pad_y - frame_pad).max(inner_t + 40.0);

    let n = s.body.len();
    let cols_max = norm_columns(s.columns).max(1);
    // 开「展示朱批」且有批注：最左让出一列给批注（竖排右起，左为后进）。
    let want_note = s.show_notes && !s.note.is_empty() && n > 0;
    let note_cols = if want_note { 1 } else { 0 };

    // 字格：先按「满列铺开」取，装不下就整体缩字号（绝不加列、绝不溢出）。
    //
    // 分母多算三段：批注列 + 右侧**悬挂沟**（列首句读挂出去的地方）。悬挂沟
    // 先按字格比例占位，句读因此绝不会画到版框上、更不会出画布。
    let denom = (cols_max + note_cols) as f32 + HANG_GUTTER;
    // 左下角落款 / 朱印要占一条竖带，先从版心宽里扣掉（否则满列时字压落款）。
    let meta_on = (s.show_sign && !s.sign.is_empty()) || (s.show_date && !s.date.is_empty());
    let meta_cell_of = |cell: f32| -> f32 {
        ((cell / CELL_RATIO) * META_FS_RATIO).clamp(11.0, 26.0) * CELL_RATIO
    };
    let reserve = if meta_on {
        meta_cell_of((inner_r - inner_l) / denom) * 2.0 + 14.0 * scale
    } else {
        0.0
    };
    let grid_l = inner_l + reserve; // 正文网格的左界（版心内）
    let avail_w = (inner_r - grid_l).max(60.0);
    let mut cell = avail_w / denom;
    let mut fs = (cell / CELL_RATIO).max(8.0);
    let mut rows = ((inner_b - top_baseline(inner_t, scale)) / cell).floor().max(3.0);
    let mut guard = 0;
    while (cols_max as f32 * rows) < n as f32 && fs > 9.0 && guard < 40 {
        fs = (fs * 0.92).max(9.0);
        cell = fs * CELL_RATIO;
        rows = ((inner_b - top_baseline(inner_t, scale)) / cell).floor().max(3.0);
        guard += 1;
    }

    // 容量上限：字太多（超长选区）就按能装下的量截断——绝不为了塞满而把
    // 行数撑爆、把列首顶出纸外（`rows_used` 因此永不超过 `rows`）。
    let cap = cols_max * rows.max(1.0) as usize;
    // 把实测容量交给调用方（分页 / A4·A3 导出的唯一口径，见 `capacity()`）。
    out.cols_cap = cols_max;
    out.rows_cap = rows.max(1.0) as usize;
    let shown = n.min(cap);
    let cols_used = if shown == 0 {
        0
    } else {
        ((shown as f32 / rows).ceil() as usize).clamp(1, cols_max)
    };
    let rows_used = if cols_used == 0 {
        0
    } else {
        (shown as f32 / cols_used as f32).ceil() as usize
    };
    let block_w = cols_used as f32 * cell;
    // **右起顶格**（v1.22.0）：列块贴版心**右缘**（再让出悬挂沟），不足的余白
    // 全部落在**左侧**。
    //
    // 原来是居中的，真机 A3 双页样张上立刻露馅：满页（12 列）贴右，末页只剩
    // 7 列时却被推到版心正中，x∈[141.5, 414.3]（左余 141 / 右余 166），
    // 字悬在纸当中、右边空出一条 —— 既不合古法（竖排书末页永远是「右排满、
    // 左留白」），也和同一本书满页的版口对不上。
    //
    // 满页时 `avail_w - span ≈ 0`，此式与居中**逐像素等价**，故既有观感不变；
    // 只有「不满一页」才会露出差别（正是要修的那种页）。
    let span = note_cols as f32 * cell + block_w + HANG_GUTTER * cell;
    let x0 = grid_l + (avail_w - span).max(0.0) + note_cols as f32 * cell;
    // 铁律 1 + 版框联动：列首 y 恒等于 y0 —— 每列都从这里起笔，绝不因列长而
    // 错动；y0 本身紧贴**版框内缘**（`top_baseline` 是它的唯一口径），故第一
    // 个字的最高点正好卡在框线下方的起点上：不压线、也不留天头空档。
    let y0 = top_baseline(inner_t, scale);

    // ---- 界行（列间竖细线）：通版心高，界在列与列之间 ----
    //
    // 刻本的界行只画**内部列界**（最左 / 最右由版框自己收口），故条数是
    // 「总列数 − 1」；线色比版框淡一档、线宽 1px，绝不抢墨迹的视觉焦点。
    if border.is_some() && cols_used > 0 {
        let grid_left = x0 - note_cols as f32 * cell; // 最左一列（含批注列）的左缘
        let total = cols_used + note_cols;
        let rule_c = ((border.unwrap_or(p.ink)) << 8) | RULE_ALPHA;
        for k in 1..total {
            let x = grid_left + k as f32 * cell;
            out.rules.push(PlacedLine {
                x1: x,
                y1: inner_t,
                x2: x,
                y2: inner_b,
                color: rule_c,
                width: RULE_W * scale,
            });
        }
    }

    // ---- 正文：从右向左，逐格落字 ----
    let mut first_marked: Option<usize> = None;
    // 列内占格游标：与「源串下标 i」分家——列首标点被挂出框外后**不占格位**，
    // 故它不推进游标，正文首字因此仍落在 row 0（＝基准线）。
    let (mut col, mut row, mut hung) = (0usize, 0usize, 0usize);
    for (i, &ch) in s.body.iter().enumerate().take(shown) {
        // 禁则·列首标点悬挂（v1.59.0 / v1.61.0 收进悬挂沟）：这一格若落在列首
        // 且是句读，就**不占格**，改挂到网格右缘之外的悬挂沟里（X 轴偏移），
        // 正文照旧顶格——绝不为让标点而把后面的字往下挤。
        let head_punct = row == 0 && is_punct(ch);
        let x = if head_punct {
            x0 + block_w + cell * HANG_GAP
        } else {
            x0 + (cols_used as f32 - 1.0 - col as f32) * cell
        };
        let y = if head_punct {
            y0 + hung as f32 * cell * HEAD_PUNCT_STEP
        } else {
            y0 + row as f32 * cell
        };
        if head_punct {
            hung += 1;
        } else {
            row += 1;
            if row >= rows_used.max(1) {
                row = 0;
                col += 1;
                hung = 0;
            }
        }
        let marked = s.red.get(i).copied().unwrap_or(false);
        if marked && first_marked.is_none() {
            first_marked = Some(i);
        }
        // 高亮底：极淡朱砂，垫在字下（先于字绘制，见 `rects` 的顺序保证）。
        if marked {
            out.rects.push(PlacedRect {
                x,
                y,
                w: cell,
                h: cell,
                color: ((p.accent) << 8) | MARK_ALPHA as u32,
                stroke: None,
            });
        }
        // 朱批字本身染朱砂（红线 + 红字，两重标记）。
        let color = if marked { p.accent } else { p.ink };
        // Markdown 装饰位：标题放大字号（**字格不动**，网格因此一点不变），
        // 加粗只记旗子、由光栅层落笔加厚。空切片时 `0` = 正文，行为与旧版一致。
        let mark = s.md.get(i).copied().unwrap_or_default();
        let ch_fs = if mark.head {
            (fs * HEAD_FS_RATIO).min(cell * HEAD_CELL_FILL)
        } else {
            fs
        };
        out.chars.push(PlacedChar {
            ch,
            x,
            y,
            cell,
            fs: ch_fs,
            color,
            punct: is_punct(ch),
            hung: head_punct,
            bold: mark.bold,
        });
    }

    // ---- 朱批列（杀手锏）：正文块左侧一列小字 + 极细朱砂引线 ----
    if want_note {
        let note_fs = (fs * NOTE_FS_RATIO).max(9.0);
        let note_cell = note_fs * CELL_RATIO;
        let nx = x0 - cell; // 最左一列的左邻
        let take = (((inner_b - y0) * 0.62) / note_cell).floor() as usize;
        let mut ny = y0;
        for (i, ch) in s.note.chars().take(take).enumerate() {
            out.chars.push(PlacedChar {
                ch,
                x: nx,
                y: ny,
                cell: note_cell,
                fs: note_fs,
                color: p.accent,
                punct: is_punct(ch),
                hung: false,
                bold: false,
            });
            ny += note_cell;
            let _ = i;
        }
        // 引线：被批的第一个字（无划线则取**最贴近批注列的首字**）→ 批注首字。
        //
        // 无划线时不能取全篇首字（它在最右一列！）：那根引线会横穿整片正文，
        // 把字划得乱七八糟。改取最左一列的首字，引线只在批注列与正文之间。
        let anchor = first_marked.unwrap_or_else(|| {
            let left_col = cols_used.saturating_sub(1);
            (left_col * rows_used).min(shown.saturating_sub(1))
        });
        let acol = anchor / rows_used.max(1);
        let arow = anchor % rows_used.max(1);
        let ax = x0 + (cols_used as f32 - 1.0 - acol as f32) * cell;
        let ay = y0 + arow as f32 * cell + cell * 0.5;
        out.lines.push(PlacedLine {
            x1: ax,
            y1: ay,
            x2: nx + note_cell,
            y2: y0 + note_cell * 0.5,
            color: (p.accent << 8) | 0x9A,
            width: LEAD_W * (w / 1080.0).max(0.6),
        });
    }

    // ---- 落款与朱印：版心内左下角（古籍里「版框内」的题识位）----
    //
    // v1.61.0：整体收进版框内缘，且左侧已预留 `reserve` 竖带，落款与正文
    // 网格永不叠字。
    let seal_size = (w * 0.085).clamp(28.0, 44.0);
    let inset = inner_l + 6.0 * scale;
    let meta_fs = (fs * META_FS_RATIO).clamp(11.0, 26.0);
    let meta_cell = meta_fs * CELL_RATIO;

    // 竖排两列：书名·章节在右（靠内），日期在左；自内而外排，读序自右向左。
    let mut cols_meta: Vec<(f32, &str)> = Vec::new();
    if s.show_sign && !s.sign.is_empty() {
        cols_meta.push((0.0, s.sign));
    }
    if s.show_date && !s.date.is_empty() {
        cols_meta.push((0.0, s.date));
    }
    // 印章上方留 12px 气口给落款，落款末字与印面不粘。
    let base_bottom = inner_b - 6.0 * scale - seal_size - 12.0 * scale;
    // 可用高：从「首行基准线」到落款底——落款按它截字，绝不溢出版框。
    let avail = (base_bottom - y0).max(meta_cell);
    let take_meta = (avail / meta_cell).floor() as usize;
    for (k, (_x, text)) in cols_meta.iter().enumerate() {
        let cx = inset + (cols_meta.len() as f32 - 1.0 - k as f32) * meta_cell;
        let chars: Vec<char> = text.chars().take(take_meta).collect();
        let block = chars.len() as f32 * meta_cell;
        let top = (base_bottom - block).max(y0);
        for (i, ch) in chars.iter().enumerate() {
            out.chars.push(PlacedChar {
                ch: *ch,
                x: cx,
                y: top + i as f32 * meta_cell,
                cell: meta_cell,
                fs: meta_fs,
                color: p.meta,
                punct: is_punct(*ch),
                hung: false,
                bold: false,
            });
        }
    }

    out.seal = Some(PlacedSeal {
        x: inset,
        y: (inner_b - 6.0 * scale - seal_size).max(y0),
        size: seal_size,
        kind: s.seal,
        text: s.seal_text.to_string(),
    });

    // ---- 出图时间戳（v1.82.0）：右下**地脚带**里一行极简小字 ----
    //
    // 位置刻意选在版框**之外**的下留白（地脚）：那一带是空的，落一行小字
    // 既不压正文、也不用给正文让 `reserve`，版面一个数都不用改——这就是
    // 「不破坏版面」的落法。横排（竖排网格里塞一行横字太怪），右缘与版框
    // 外缘齐平，颜色比落款再淡一档。
    if !s.stamp.is_empty() {
        let cs: Vec<char> = s.stamp.chars().collect();
        let sfs = (STAMP_FS * scale).clamp(9.0, 18.0);
        let total = cs.len() as f32 * sfs;
        // 右缘：版框外缘（pad_x 处）往里收一个气口；左界兜住，绝不越出版心。
        let right = (w - pad_x - 2.0 * scale).max(inner_l + total);
        // 地脚带中心：版框下缘（h - pad_y）到画布底之间。
        let cy = h - pad_y * 0.5;
        let color = fade(p.meta, p.bg, STAMP_FADE);
        for (i, ch) in cs.iter().enumerate() {
            out.chars.push(PlacedChar {
                ch: *ch,
                x: right - total + i as f32 * sfs,
                y: cy - sfs * 0.62,
                cell: sfs,
                fs: sfs,
                color,
                punct: false,
                hung: false,
                bold: false,
            });
        }
    }

    out
}

/// 把 `fg` 往 `bg` 方向掺淡（`ink` = 1 满色、0 全底），返回 `0xRRGGBB`。
///
/// 不走 alpha：字是逐个 `draw_glyphs` 落的，填充色里带 alpha 会在字与字的
/// 重叠处叠出深浅不一的斑；直接算一个实色的中间色最稳。
fn fade(fg: u32, bg: u32, ink: f32) -> u32 {
    let k = ink.clamp(0.0, 1.0);
    let mut out = 0u32;
    for shift in (0..24).step_by(8) {
        let a = ((fg >> shift) & 0xFF) as f32;
        let b = ((bg >> shift) & 0xFF) as f32;
        let v = (a * k + b * (1.0 - k)).round().clamp(0.0, 255.0) as u32;
        out |= v << shift;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1080.0;
    const H: f32 = 1920.0;

    /// 带时间戳的 spec（其余与 [`spec`] 同）。
    fn spec_stamp<'a>(body: &'a [char], red: &'a [bool], stamp: &'a str) -> LayoutSpec<'a> {
        let mut s = spec(body, red);
        s.stamp = stamp;
        s
    }

    fn spec<'a>(body: &'a [char], red: &'a [bool]) -> LayoutSpec<'a> {
        LayoutSpec {
            body,
            red,
            md: &[],
            note: "",
            show_notes: false,
            sign: "道德經 · 第一章",
            show_sign: true,
            date: "二〇二六年十月",
            show_date: true,
            columns: 12,
            bg: ShareBg::Xuan,
            frame: ShareFrame::Red,
            seal: ShareSeal::Twin,
            seal_text: "道德",
            font: ShareFont::Song,
            stamp: "",
            w: W,
            h: H,
        }
    }

    /// 铁律 1：每一列的第一个字，y 必须绝对相同（首行顶格）。
    #[test]
    fn stamp_lands_in_the_foot_margin_and_never_moves_the_grid() {
        let body: Vec<char> = "道可道非常道名可名非常名无名天地之始".chars().collect();
        let red = vec![false; body.len()];
        let a = layout(&spec_stamp(&body, &red, ""));
        let b = layout(&spec_stamp(&body, &red, "丙午年季秋下浣"));
        // ① 正文一个字都没挪：版画指令**逐条相同**，只在末尾多出戳那几个字。
        //    这就是「不破坏版面」——戳不进正文网格、不占 reserve 竖带。
        assert_eq!(
            b.chars.len(),
            a.chars.len() + 7,
            "戳应是纯增量：{} vs {}",
            b.chars.len(),
            a.chars.len()
        );
        assert_eq!(&b.chars[..a.chars.len()], &a.chars[..], "正文被戳挪动了");
        // ② 戳落在**版框之外**的下留白（地脚带）里，绝不压正文。
        let pad_y = (H * 0.035).clamp(18.0, 72.0);
        let frame_bottom = H - pad_y;
        for c in &b.chars[a.chars.len()..] {
            assert!(c.y > frame_bottom, "戳压进版框了：y={}", c.y);
            assert!(c.y + c.fs * 1.3 < H, "戳掉出画布了：y={}", c.y);
            assert!(c.x > 0.0 && c.x + c.fs <= W, "戳横向越界：x={}", c.x);
        }
        // ③ 右对齐：最后一个字的右缘贴住版框外缘（差一个气口）。
        let last = b.chars.last().unwrap();
        let pad_x = (W * 0.075).clamp(18.0, 46.0);
        assert!(
            (W - pad_x - (last.x + last.fs)).abs() < 6.0,
            "戳没贴住右下角：右缘 {} / 版框外缘 {}",
            last.x + last.fs,
            W - pad_x
        );
        // ④ 字号是全卡最小的一档，且比正文小得多。
        let body_fs = a.chars[0].fs;
        assert!(last.fs < body_fs, "戳不该比正文还大：{} vs {}", last.fs, body_fs);
        assert!(last.fs <= STAMP_FS + 0.01, "戳超过最小档：{}", last.fs);
        // ⑤ 颜色比落款淡（往纸色方向掺过），不能是满墨。
        let p = crate::share::palette(ShareBg::Xuan);
        assert_ne!(last.color, p.meta, "戳该比落款淡");
        let lum = |c: u32| -> i32 {
            (((c >> 16) & 0xFF) as i32 * 3 + ((c >> 8) & 0xFF) as i32 * 6 + (c & 0xFF) as i32) / 10
        };
        assert!(
            lum(last.color) > lum(p.meta),
            "戳应比落款亮（更淡）：{} vs {}",
            lum(last.color),
            lum(p.meta)
        );
    }

    #[test]
    fn every_column_starts_on_the_same_baseline() {
        // 字数要能撑出多列（12 列档下每列约 24 字），否则无从比较列首。
        let body: Vec<char> = "道可道非常道名可名非常名無名天地之始有名萬物之母故常無欲以觀其妙常有欲以觀其徼此兩者同出而異名同謂之玄玄之又玄眾妙之門"
            .chars()
            .collect();
        let red = vec![false; body.len()];
        let l = layout(&spec(&body, &red));
        let body_chars: Vec<&PlacedChar> =
            l.chars.iter().filter(|c| c.fs > 30.0).collect(); // 排除落款小字
        assert!(!body_chars.is_empty());
        // 按 x 分组（一列一个 x），每组最小 y 必须等于该列的列首 y0。
        let mut xs: Vec<f32> = body_chars.iter().map(|c| c.x).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert!(xs.len() > 1, "应当排出多列：{:?}", xs);
        let heads: Vec<f32> = xs
            .iter()
            .map(|x| {
                body_chars
                    .iter()
                    .filter(|c| (c.x - *x).abs() < 0.01)
                    .map(|c| c.y)
                    .fold(f32::MAX, f32::min)
            })
            .collect();
        let first = heads[0];
        for yy in &heads {
            assert!(
                (yy - first).abs() < 0.01,
                "列首未顶格：{heads:?}（应全部相等）"
            );
        }
    }

    /// 从右向左：第 0 个字必在最右一列。
    #[test]
    fn first_char_sits_in_the_rightmost_column() {
        let body: Vec<char> = "道可道非常道名可名非常名".chars().collect();
        let red = vec![false; body.len()];
        let l = layout(&spec(&body, &red));
        let max_x = l.chars.iter().map(|c| c.x).fold(f32::MIN, f32::max);
        assert!((l.chars[0].x - max_x).abs() < 0.01, "首字不在最右列");
    }

    /// 朱批高亮：划线的字下面垫一块极淡朱砂，未划线的没有。
    #[test]
    fn marked_chars_get_a_faint_cinnabar_ground() {
        let body: Vec<char> = "道可道非常道".chars().collect();
        let red = vec![true, false, false, true, false, false];
        let l = layout(&spec(&body, &red));
        // 高亮块数 = 划线的字数（落款 / 批注不产生高亮）。
        assert_eq!(l.rects.iter().filter(|r| r.stroke.is_none()).count(), 2);
        // 划线的字染朱砂色（accent = 0xB03A2E，宣纸黄档）。
        let accented = l.chars.iter().filter(|c| c.color == 0xB03A2E).count();
        assert_eq!(accented, 2);
    }

    /// 开「展示朱批」：多出一列批注小字 + 一条引线，且批注列在正文左侧。
    #[test]
    fn note_column_and_leader_line_appear_when_enabled() {
        let body: Vec<char> = "道可道非常道名可名非常名".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.note = "此句疑有衍文";
        s.show_notes = true;
        let l = layout(&s);
        assert_eq!(l.lines.len(), 1, "应恰好一条引线");
        // 批注小字：朱砂色 + 字号明显小于正文（落款是灰色，不在此列）。
        let body_fs = l.chars[0].fs;
        let note_chars: Vec<&PlacedChar> = l
            .chars
            .iter()
            .filter(|c| c.color == 0xB03A2E && c.fs < body_fs * 0.6)
            .collect();
        assert!(note_chars.len() >= 5, "批注列字数不足：{}", note_chars.len());
    }

    /// 关掉批注：一条引线都不画。
    #[test]
    fn no_leader_line_when_notes_are_off() {
        let body: Vec<char> = "道可道非常道".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.note = "此句疑有衍文";
        s.show_notes = false;
        let l = layout(&s);
        assert!(l.lines.is_empty());
    }

    /// 无边框：一层面框都不画（界框属于 `rects` 的 stroke 项）。
    /// **四周双边**（v1.83.0）：外框一粗、内框一细，中间隔一道气口；
    /// 正文退到内框以里，绝不压线。
    ///
    /// 钉死四件事：① 确实画出两条（不是一圈粗线冒充双线）；② 外粗内细；
    /// ③ 内框比外框淡（照片上的层次）；④ 首字落在内框以里。
    /// 另加一条反向闸：非双边的卡仍只画一条——老版面一个像素都不许变。
    #[test]
    fn double_frame_draws_an_outer_and_an_inner_line() {
        let body: Vec<char> = "道可道非常道名可名非常名無名天地之始".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.frame = ShareFrame::Double;
        s.bg = ShareBg::Keben;
        let d = layout(&s);
        // 只有「框线」带 stroke；引线 / 朱批是实心块，用 stroke 挑出来。
        let frames: Vec<&PlacedRect> =
            d.rects.iter().filter(|r| r.stroke.is_some()).collect();
        assert_eq!(frames.len(), 2, "双边应画两条框线，实为 {}", frames.len());
        let (outer, inner) = (frames[0], frames[1]);
        let (wo, wi) = (outer.stroke.unwrap(), inner.stroke.unwrap());
        assert!(wo > wi, "必须外粗内细：外 {wo} / 内 {wi}");
        // 两条线不重叠：内框整圈向内让「外框半宽 + 气口」。
        let off = inner.x - outer.x;
        assert!(
            off >= wo * 0.5 + 1.0,
            "内外框贴在一起了（分不出双线）：偏移 {off} / 外框半宽 {}",
            wo * 0.5
        );
        assert!(
            (inner.y - outer.y - off).abs() < 0.01,
            "内框四边让位不均：x {off} / y {}",
            inner.y - outer.y
        );
        // 内框比外框淡一档（层次：外框最重）。
        assert!(
            (inner.color & 0xFF) < (outer.color & 0xFF),
            "内框该比外框淡：{:02X} / {:02X}",
            inner.color & 0xFF,
            outer.color & 0xFF
        );
        // 首字必须落在内框以里——双边不是画着看的，正文得给它让位。
        let first = d.chars.first().expect("应有正文");
        assert!(
            first.y >= inner.y,
            "首字压到内框上了：{} < {}",
            first.y,
            inner.y
        );
        assert!(
            first.x + first.cell <= inner.x + inner.w + 0.01,
            "首字压到右内框：{} > {}",
            first.x + first.cell,
            inner.x + inner.w
        );
        // 反向闸：其余三档仍只画一条。
        for f in [ShareFrame::Red, ShareFrame::Black] {
            let mut one = spec(&body, &red);
            one.frame = f;
            let n = layout(&one)
                .rects
                .iter()
                .filter(|r| r.stroke.is_some())
                .count();
            assert_eq!(n, 1, "{f:?} 不该画成双线");
        }
    }

    #[test]
    fn frameless_card_draws_no_border() {
        let body: Vec<char> = "道可道".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.frame = ShareFrame::None;
        let l = layout(&s);
        assert!(l.rects.iter().all(|r| r.stroke.is_none()));
    }

    /// 防崩：空正文 / 超大字号请求 / 长文都不 panic，且字格为正。
    #[test]
    fn degenerate_inputs_never_panic_and_never_overflow() {
        let red: Vec<bool> = Vec::new();
        let l = layout(&spec(&[], &red));
        assert_eq!(l.chars.iter().filter(|c| c.fs > 30.0).count(), 0);
        assert!(l.seal.is_some());

        let body: Vec<char> = "道".repeat(4000).chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.w = 300.0;
        s.h = 500.0;
        let l = layout(&s);
        for c in &l.chars {
            assert!(c.cell > 0.0 && c.fs > 0.0);
            assert!(c.x >= -1.0 && c.x <= l.w + 1.0, "字溢出：{}", c.x);
            assert!(c.y >= -1.0 && c.y <= l.h + 1.0, "字溢出：{}", c.y);
        }
    }

    /// 标点靠右上安置，且不占整格（仍按整格推进）。
    #[test]
    fn punctuation_hangs_in_the_upper_right() {
        let body: Vec<char> = "道可道，非常道。".chars().collect();
        let red = vec![false; body.len()];
        let l = layout(&spec(&body, &red));
        let comma = match l.chars.iter().find(|c| c.ch == '，') {
            Some(c) => c,
            None => panic!("未排出逗号"),
        };
        assert!(comma.punct);
        let (px, py, pfs) = punct_box(comma);
        assert!(px > comma.x, "句读应靠右");
        assert!(py < comma.y + comma.cell * 0.5, "句读应靠上");
        assert!(pfs < comma.fs, "句读字号应收小");
    }

    // ── Markdown 清洗器（v1.20.0）───────────────────────────────────────

    /// 带 Markdown 装饰位的 spec（`red` 由调用方提供，与 `body` 等长）。
    fn spec_md<'a>(c: &'a CleanMd, red: &'a [bool]) -> LayoutSpec<'a> {
        let mut s = spec(&c.body, red);
        s.md = &c.md;
        s
    }

    /// ① 语法记号一个都不印：`#` / `**` / `---` / `- ` / 表格竖线全部消失。
    #[test]
    fn md_cleaner_strips_syntax_and_marks_headings() {
        let src = "# 千字文\n\n天地玄黃，宇宙洪荒。\n\n---\n\n**金生麗水**，玉出昆岡。\n\n- 龍師火帝\n";
        let c = clean_md(src);
        let s = c.text();
        for bad in ['#', '*', '|', '`'] {
            assert!(!s.contains(bad), "记号没洗掉：{bad:?} → {s}");
        }
        assert!(!s.contains("---"), "分隔线没丢：{s}");
        assert!(!s.contains("- "), "列表标记没丢：{s}");
        // 标题字：开头三字（千字文）是 head，正文不是。
        assert_eq!(c.body.len(), c.md.len(), "装饰位必须与字等长");
        assert!(c.md.iter().take(3).all(|m| m.head), "标题字未标记");
        assert!(
            c.md.iter().skip(3).all(|m| !m.head),
            "非标题字被误标成标题"
        );
        // 换行与空行不产字（竖排是连续流，`\n` 只会变成空白格）。
        assert!(!c.body.iter().any(|c| c.is_whitespace() && *c != ' '));
        // 加粗：`**金生麗水**` 四字带 bold 旗子，其余不带。
        let at = c.body.windows(4).position(|w| w == ['金', '生', '麗', '水']).expect("正文丢了");
        assert!(c.md[at..at + 4].iter().all(|m| m.bold), "加粗未标记");
        assert!(c.md[at + 5].bold == false);
        assert!(!c.md[0].bold, "标题不该被当成加粗");
    }

    /// ①b **绝不从 UTF-8 中间切字**：多字节 CJK 走 `char` 迭代，一个字都不许丢。
    ///
    /// 用户 2026-11-01 点名要查这条。清洗器全程 `Vec<char>`，没有任何字节下标
    /// 切片碰过正文（只碰过 ASCII 的 `#` / 数字前缀），故长度与内容必须严格相等。
    #[test]
    fn clean_md_never_splits_a_cjk_char() {
        let base = "壹貳參肆伍陸柒捌玖拾";
        let src = format!("{}\n\n# 題\n\n**粗**體", base.repeat(50));
        let c = clean_md(&src);
        // 正文 500 字 + 标题「題」1 字 + 加粗「粗體」2 字 = 503 字，一字不丢。
        let want = base.chars().count() * 50 + 1 + 2;
        assert_eq!(
            c.body.len(),
            want,
            "字数对不上（有字被切掉了）：{}",
            c.text()
        );
        assert_eq!(c.text(), format!("{}題粗體", base.repeat(50)));
    }

    /// ② 链接只留文字、斜体只去记号、行内代码去反引号、HTML 标签整段丢。
    #[test]
    fn md_cleaner_keeps_only_the_words() {
        let c = clean_md("見[莊子](https://example.com)注，*逍遙*遊，`齊物`論，<span>人間世</span>\n");
        let s = c.text();
        assert_eq!(s, "見莊子注，逍遙遊，齊物論，人間世", "清洗结果不符：{s}");
    }

    /// ③ 幂等：清洗结果再洗一遍，**字一个不变**（用户会在编辑器里反复刷预览）。
    ///
    /// 装饰位不参与幂等——记号已经洗掉了，第二遍自然标不出标题 / 加粗；
    /// 这是「先出干净文字、再谈样式」的题中之义，不是缺陷。
    #[test]
    fn md_cleaner_is_idempotent() {
        let src = "# 題\n**甲**乙[丙](x)\n---\n- 丁\n";
        let a = clean_md(src);
        let b = clean_md(&a.text());
        assert_eq!(a.body, b.body, "清洗不幂等：{} vs {}", a.text(), b.text());
        assert!(
            b.md.iter().all(|m| *m == MdMark::default()),
            "二次清洗不该再产生装饰位"
        );
    }

    /// ④ 标题只放大字号，**字格 / 位置一格不动**——网格是竖排的命。
    #[test]
    fn heading_chars_get_a_bigger_font_but_the_same_cell() {
        let c = clean_md("# 千字文\n天地玄黃宇宙洪荒\n");
        let red = vec![false; c.body.len()];
        let l = layout(&spec_md(&c, &red));
        let head = &l.chars[0];
        let body = &l.chars[3];
        assert!(head.fs > body.fs * 1.1, "标题没放大：{} vs {}", head.fs, body.fs);
        assert!(
            head.fs <= head.cell * HEAD_CELL_FILL + 0.01,
            "放大后撑破字格：fs={} cell={}",
            head.fs,
            head.cell
        );
        assert!(
            (head.cell - body.cell).abs() < 0.01,
            "字格被标题改动了：{} vs {}",
            head.cell,
            body.cell
        );
        // 每个字的 x/y 网格位置与「无装饰」版完全一致（只有字号不同）。
        let plain: Vec<char> = c.body.clone();
        let red = vec![false; plain.len()];
        let l0 = layout(&spec(&plain, &red));
        for (a, b) in l0.chars.iter().zip(l.chars.iter()) {
            assert!((a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01, "标题挪动了网格");
        }
    }

    /// ⑤ 加粗只改笔重，不改版式：坐标逐字相同。
    #[test]
    fn bold_changes_weight_only() {
        let body: Vec<char> = "金生麗水玉出昆岡".chars().collect();
        let red = vec![false; body.len()];
        let plain = layout(&spec(&body, &red));
        let md: Vec<MdMark> = (0..body.len())
            .map(|i| MdMark {
                head: false,
                bold: i < 4,
            })
            .collect();
        let mut s = spec(&body, &red);
        s.md = &md;
        let bolded = layout(&s);
        for (a, b) in plain.chars.iter().zip(bolded.chars.iter()) {
            assert!((a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01, "加粗挪动了网格");
            assert!((a.fs - b.fs).abs() < 0.01, "加粗改了字号");
        }
        assert_eq!(
            bolded.chars.iter().filter(|c| c.bold).count(),
            4,
            "加粗旗子没带到落位结果里"
        );
    }
    // ── 纸张口径（v1.21.0）：A3 只是「同一套版式的另一组 w/h」 ──────────────

    /// 单字 spec（只为问容量 / 字号，不求排满）。
    fn one_char(w: f32, h: f32) -> CardLayout {
        let body = ['道'];
        let red = [false];
        let mut s = spec(&body, &red);
        s.show_sign = false;
        s.show_date = false;
        s.w = w;
        s.h = h;
        layout(&s)
    }

    /// A3 / A4 的像素尺寸必须是**毫米算出来的**，不是手写魔数。
    #[test]
    fn paper_sizes_come_from_millimetres() {
        // 300dpi：A4 = 2480×3508，A3 = 3508×4961（印刷业标准像素数）。
        let a4 = Paper::A4.geom(300.0);
        assert_eq!((a4.w as u32, a4.h as u32), (2480, 3508));
        let a3 = Paper::A3.geom(300.0);
        assert_eq!((a3.w as u32, a3.h as u32), (3508, 4961));
        // PDF 走 72dpi＝pt：A3 = 842×1191pt（297×420mm）。
        let pdf = Paper::A3.pdf_geom();
        assert_eq!((pdf.w as u32, pdf.h as u32), (842, 1191));
        // 预览只取样、不按 300dpi 贴图：长边恒 1280，比例不变。
        let pv = Paper::A3.preview_geom();
        assert_eq!(pv.h as u32, 1280);
        assert!((pv.aspect() - 297.0 / 420.0).abs() < 0.01, "预览丢了纸的比例");
        // 预览卡是 9:16 的老取景，与纸张无关。
        let card = Paper::Card.preview_geom();
        assert_eq!((card.w as u32, card.h as u32), (720, 1280));
    }

    /// 同一份正文，A3 的字必须比 A4 大——「物理尺寸参数化」的肉眼证据。
    #[test]
    fn a3_sets_bigger_type_than_a4() {
        let a4 = one_char(Paper::A4.pdf_geom().w, Paper::A4.pdf_geom().h);
        let a3 = one_char(Paper::A3.pdf_geom().w, Paper::A3.pdf_geom().h);
        let fs4 = a4.chars[0].fs;
        let fs3 = a3.chars[0].fs;
        assert!(fs3 > fs4 * 1.3, "A3 字号没跟上幅面：A4={fs4} A3={fs3}");
        // 容量是真排出来的，不是写死的常数表。
        assert!(a3.capacity() > 120, "A3 一页装不下 120 字：{}", a3.capacity());
    }

    /// 大纸的版框留白必须按幅面放宽：A3@300dpi 绝不能只有 3.9mm 的白边。
    #[test]
    fn big_paper_gets_proportional_margins() {
        let g = Paper::A3.geom(300.0);
        let big = one_char(g.w, g.h);
        // rects[0] 是版框外框，x 就是左侧留白。
        let pad = big.rects[0].x;
        let mm = pad / 300.0 * 25.4;
        assert!(mm > 9.0, "A3 天头地脚只有 {mm:.1}mm，印厂会笑");
        // 1080 卡的留白一位小数都不许变（v1.9.x 基准）。
        let card = one_char(1080.0, 1920.0);
        assert!((card.rects[0].x - 46.0).abs() < 0.01, "1080 卡留白被改动");
    }

    // ── 双页拼版（v1.22.0）：A3 横放、两个单页并排 ────────────────────────

    /// **A3 拼版是横的**，且它的半边就是 A4 那一档大小。
    ///
    /// 这条钉的是用户的原话：「A3 应该是横向的（宽 > 高）」「把 A3 的宽度除以 2，
    /// 得到单页宽度（大概是 A4 的大小）」。任何一处退回竖放 / 整宽排版都必崩。
    #[test]
    fn a3_spread_is_a_landscape_sheet_of_two_a4_sized_pages() {
        let sheet = Paper::A3.sheet_pdf_geom(Imposition::Spread);
        assert!(
            sheet.w > sheet.h,
            "拼版整张纸必须是横的：{}×{}",
            sheet.w,
            sheet.h
        );
        // 420×297mm @72dpi = 1190.55×841.89pt
        assert_eq!((sheet.w as u32, sheet.h as u32), (1191, 842));

        let page = Paper::A3.page_pdf_geom(Imposition::Spread);
        let a4 = Paper::A4.page_pdf_geom(Imposition::Single);
        // 单页 ≈ A4（两侧各让半个中缝），高完全等于 A4。
        assert_eq!(page.h as u32, a4.h as u32, "拼版单页的纸高应等于 A4");
        assert!(
            (page.w - a4.w).abs() < 0.05 * a4.w,
            "拼版单页宽度不是 A4 那一档：{} vs {}",
            page.w,
            a4.w
        );
        // 两页 + 中缝 = 整张纸，一分不差（否则拼出来会偏出纸外）。
        let gutter = Paper::A3.sheet_pdf_geom(Imposition::Spread).w - page.w * 2.0;
        // 取整会多给半个点到一个点，故只要求「不低于下限、且不多出 2pt」。
        assert!(
            gutter >= SPREAD_GUTTER_PT - 0.01 && gutter <= SPREAD_GUTTER_PT + 2.0,
            "中缝必须 ≥ {SPREAD_GUTTER_PT}pt，实为 {gutter:.2}pt"
        );
        // 半区宽度：1191 ÷ 2 = 595.5，单页是「半区 − 半个中缝」。
        let half = Paper::A3.sheet_pdf_geom(Imposition::Spread).w / 2.0;
        assert!(
            (half - 595.5).abs() < 1.0,
            "半区应约 595.5pt，实为 {half:.2}pt"
        );
        assert!(
            (half - page.w - SPREAD_GUTTER_PT / 2.0).abs() < 1.0,
            "单页宽度不是「半区 − 半个中缝」：{} vs {}",
            page.w,
            half - SPREAD_GUTTER_PT / 2.0
        );

        // 单页拼版照旧竖放（老口径一个字都不许动）。
        let single = Paper::A3.page_pdf_geom(Imposition::Single);
        assert!(single.h > single.w, "單頁 A3 不该被横过来");
        assert_eq!((single.w as u32, single.h as u32), (842, 1191));
    }

    /// **装订模式**：右开本奇左偶右，左开本偶左奇右；两页各自在半边居中。
    ///
    /// 用户 2026-11-01 的原话：「左边放置奇数页（第1页），右边放置偶数页（第2页）」
    /// 「奇偶两页分别在这两半边居中排版，中间留出中缝间距，不要直接拉伸铺满」。
    #[test]
    fn binding_decides_which_side_the_odd_page_takes() {
        let sheet = 1191.0; // A3 拼版整张纸（pt）
        let page = 580.3; // 单页（pt）＝ (1191 − 30) / 2
        // 右开本（默认）：第 1 页（奇）在左，第 2 页（偶）在右。
        let [odd, even] = spread_slots(sheet, page, Binding::Right);
        assert_eq!(odd, 0.0, "右开本的第 1 页应贴住纸的左缘");
        assert!(even > odd, "右开本应奇左偶右：{odd} / {even}");
        assert_eq!(Binding::default(), Binding::Right, "默认必须是右开本");
        // 左开本：反过来。
        let [odd2, even2] = spread_slots(sheet, page, Binding::Left);
        assert!(odd2 > even2, "左开本应偶左奇右：{odd2} / {even2}");

        // 中缝：两页之间的空隙，必须 ≥ 30pt 且落在纸的正中。
        let gutter = even - (odd + page);
        assert!(
            gutter >= SPREAD_GUTTER_PT - 0.5,
            "中缝只有 {gutter:.2}pt，低于 {SPREAD_GUTTER_PT}pt"
        );
        assert!(
            (odd + page + gutter * 0.5 - sheet * 0.5).abs() < 0.01,
            "中缝没落在纸的正中（折不准）"
        );
        // 绝不越界、绝不重叠。
        assert!(odd >= 0.0 && even + page <= sheet + 0.01, "页子跑出纸外");
        assert!(gutter > 0.0, "两页重叠了");
        // 左开本同样是「贴外缘 + 中间一条缝」。
        let g2 = odd2 - (even2 + page);
        assert!(g2 >= SPREAD_GUTTER_PT - 0.5, "左开本中缝不足：{g2:.2}pt");
    }

    /// **一列只该排一页的字数，绝不是 44 个**：右起、列满换行、逐列向左。
    ///
    /// 用户 2026-11-01 的原话：「最右侧第一列排满（比如 15 个字），再往左排
    /// 第二列。绝对不能把 44 个字强行塞进一列！」44 字一列是**窄页**的症状
    /// （页宽被算小了，字格跟着变小、一列就拉得老长），这条把它挡住。
    #[test]
    fn a_column_holds_a_page_worth_of_chars_not_forty_four() {
        let g = Paper::A3.page_pdf_geom(Imposition::Spread);
        let body: Vec<char> = "天地玄黃宇宙洪荒日月盈昃辰宿列張寒來暑往秋收冬藏".chars().cycle().take(300).collect();
        let red = vec![false; body.len()];
        let mut sp = spec(&body, &red);
        sp.show_sign = false;
        sp.show_date = false;
        sp.columns = 12;
        sp.w = g.w;
        sp.h = g.h;
        let l = layout(&sp);

        // 正文字（滤掉落款小字 / 戳），按字格 x 归列。
        let body_fs = l.chars.iter().map(|c| c.fs).fold(0.0f32, f32::max);
        let mut cols: std::collections::BTreeMap<i64, (f32, usize)> = std::collections::BTreeMap::new();
        for c in l.chars.iter().filter(|c| (c.fs - body_fs).abs() < 0.01) {
            let key = (c.x * 4.0).round() as i64;
            let e = cols.entry(key).or_insert((c.x, 0));
            e.1 += 1;
        }
        assert!(cols.len() >= 3, "应排出多列，实为 {} 列", cols.len());
        let per_col: Vec<usize> = cols.values().map(|(_, n)| *n).collect();
        let max = *per_col.iter().max().unwrap_or(&0);
        assert!(
            max <= 30,
            "一列塞了 {max} 个字（每列应在 10–30 之间）：{per_col:?}"
        );
        assert!(max >= 8, "一列只有 {max} 个字，字格过大：{per_col:?}");
        // 右起：x 最大的那一列必须是**第一列**（钱最多的那列在右）。
        let (rightmost_x, _) = cols.values().next_back().copied().unwrap_or((0.0, 0));
        let first_char = &l.chars[0];
        assert!(
            (first_char.x - rightmost_x).abs() < 0.01,
            "正文没有从最右列起排：首字 x={} 最右列 x={rightmost_x}",
            first_char.x
        );
        // 读序逐列向左：按「最右 → 最左」取 x，必须严格递减。
        let xs: Vec<f32> = cols.values().map(|(x, _)| *x).rev().collect();
        for w in xs.windows(2) {
            assert!(w[0] > w[1], "列的 x 没往左走：{xs:?}");
        }
    }

    /// 拼版单页排出来的字**必须比单页 A3 小**——这就是「书」与「海报」的分界。
    #[test]
    fn spread_page_sets_book_type_not_poster_type() {
        let poster = Paper::A3.page_pdf_geom(Imposition::Single);
        let book = Paper::A3.page_pdf_geom(Imposition::Spread);
        let a = one_char(poster.w, poster.h);
        let b = one_char(book.w, book.h);
        assert!(
            b.chars[0].fs < a.chars[0].fs * 0.75,
            "拼版单页的字还是海报尺寸：拼版 {} / 海报 {}",
            b.chars[0].fs,
            a.chars[0].fs
        );
        // 版心绝不越出自己那半边：越界就是「一页拉宽铺满整纸」的海报。
        for c in &b.chars {
            assert!(
                c.x + c.cell <= book.w + 0.01,
                "字越出单页边界：{} > {}",
                c.x + c.cell,
                book.w
            );
        }
        let sheet = Paper::A3.sheet_pdf_geom(Imposition::Spread);
        assert!(
            book.w * 2.0 < sheet.w,
            "两页加中缝应当小于整张纸：{} vs {}",
            book.w * 2.0,
            sheet.w
        );
    }

    /// **末页不满时，列块必须右起顶格，绝不居中。**
    ///
    /// v1.22.0 真机 A3 双页样张抓出的真 bug：满页（12 列）贴右，只剩 7 列的
    /// 末页却被推到版心正中（x∈[141.5, 414.3]，左余 141 / 右余 166），字悬在
    /// 纸当中、右边空出一条，同一本书的两页版口对不上。
    ///
    /// 竖排古法：右排满、左留白。故判据只有一条——**满页与末页的右余必须
    /// 相等**（同一版心、同一悬挂沟），余白全部落在左侧。
    #[test]
    fn a_short_last_page_hangs_from_the_right_not_the_centre() {
        let g = Paper::A3.page_pdf_geom(Imposition::Spread);
        let page = |n: usize| {
            let body: Vec<char> = "天地玄黃宇宙洪荒日月盈昃辰宿列張寒來暑往秋收冬藏"
                .chars()
                .cycle()
                .take(n)
                .collect();
            let red = vec![false; body.len()];
            let mut sp = spec(&body, &red);
            sp.show_sign = false;
            sp.show_date = false;
            sp.columns = 12;
            sp.w = g.w;
            sp.h = g.h;
            layout(&sp)
        };
        // 只量正文字：落款 / 朱印的小字另有字号，不参与版口对齐。
        let body_chars = |l: &crate::share_layout::CardLayout| -> (f32, f32) {
            let fs = l.chars.iter().map(|c| c.fs).fold(0.0f32, f32::max);
            let mut left = f32::MAX;
            let mut right = 0.0f32;
            for c in l.chars.iter().filter(|c| (c.fs - fs).abs() < 0.01) {
                left = left.min(c.x);
                right = right.max(c.x + c.cell);
            }
            (left, right)
        };

        let full = page(240); // 满页：12 列 × 20 行
        let short = page(100); // 末页：只剩 5 列
        assert!(full.chars.len() > short.chars.len(), "两页字数应当不同");

        let (full_l, full_r) = body_chars(&full);
        let (short_l, short_r) = body_chars(&short);
        let full_gap = g.w - full_r;
        let short_gap = g.w - short_r;

        assert!(
            (full_gap - short_gap).abs() < 0.5,
            "末页没有右起顶格（居中了吧）：满页右余 {full_gap:.1} / 末页右余 {short_gap:.1}"
        );
        // 余白必须全部落在左侧，且相当可观（末页只用了半页宽）。
        assert!(
            short_l > full_l + g.w * 0.25,
            "末页的余白没落在左侧：满页左缘 {full_l:.1} / 末页左缘 {short_l:.1}"
        );
    }
}
