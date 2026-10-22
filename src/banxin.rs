//! 版心（中缝）版式：**对鱼尾** + 折痕 + 书口文字（书名 / 卷次 / 篇题 / 叶码）。
//!
//! 刻本的版心只有「一格字」宽，却要立住整叶的门面：上下一对鱼尾夹住
//! 书名、卷次、叶码，中间一道纸对折的折痕。这套几何全部是**纯计算**
//! （与 `west.rs` / `ruby.rs` 同路数），渲染层只负责把算好的尺寸与文字
//! 摆上去，方便单测与回归。
//!
//! ## 一、鱼尾：**黑鱼尾**是直线几何，不是圆滚滚的桃形，更不是黑方块
//!
//! 宋刻本、明刻本里的「黑鱼尾」是刀刻出来的**锐利几何图形**：顶边一
//! 刀平直，两侧竖直下垂成一对尾翼，中央劈出一道 V 形凹口——**通体直
//! 线，没有一处圆润的曲线**。早先用 `<rect>` 画两枚实心黑块（不合形制），
//! 中间一度改用贝塞尔画「燕尾」（画成了黑桃 / 蝙蝠翅膀，圆滚滚的，
//! 更错）。现在严格按刻本的五个折点画**多边形**，只有 `M` / `L` / `Z`，
//! 严禁任何 `C`（贝塞尔）：
//!
//! ```text
//!             cx-w/2        cx        cx+w/2
//!   上鱼尾  ████████████████████████████   y = cy          顶边（平齐，贴版框）
//!          ████                      ████
//!          ██████                    ██████  y = cy+h·0.38  凹口顶点（锐角，64°）
//!          ████████                ████████
//!          ██████████            ██████████
//!          ████████████        ████████████  y = cy+h       尾翼下缘
//!
//!   下鱼尾  同上，整体上下镜像（底边平齐、凹口朝上）
//! ```
//!
//! 折点（上鱼尾，`cx` 中缝中线、`cy` 顶边、`w` 宽、`h` 高）：
//!
//! 1. 左上 `(cx-w/2, cy)` → 2. 右上 `(cx+w/2, cy)` → 3. 右翼下角
//!    `(cx+w/2, cy+h)` → 4. 中心凹口 `(cx, cy+h·0.38)` → 5. 左翼下角
//!    `(cx-w/2, cy+h)` → 闭合。下鱼尾是它的**垂直镜像**（底边 `by`）。
//!
//! * 顶边贴版框，两侧外缘**竖直**下垂到底——刀刻的行刀，不是手绘的弧；
//! * 中央凹口的顶点在中轴上（`(cx, cy+h·APEX)`），比两侧尾翼下缘
//!   高一截，缺口是**开口朝下的锐角 V**（约 64°，`TAIL_WING=1.0` 让
//!   尾翼一直垂到底、缺口吃掉六成高度，才「锐」得出来）；
//! * 两枚上下相对（上鱼尾凹口朝下、下鱼尾凹口朝上），中间那块空当
//!   正是书口文字（书名 / 卷次 / 叶码「二十三」）的地盘；
//! * **不加抖点**：毛边加在直线上只会看成锯齿，锐利才是刻本的味道。
//!
//! > v1.25.0 形体收细：用户指出「鱼尾太粗笨」。宽度由版心 0.78 收到
//! > 0.70、高由 1.2 字收到 0.95 字、缺口由三成加深到六成（`TAIL_APEX`
//! > 0.38 + 尾翼垂到底）——同一枚鱼尾，墨量少了三分之一，锐角却更利。
//!
//! ## 一之二、黑口：版心顶 / 底的粗黑块
//!
//! 大黑口本子的版心，天头地脚各有一道**粗黑横块**（纸对折后书口处露出的
//! 墨线），把整列书口夹住。渲染层按 `heikou_h` 在版心最顶 / 最底各铺一块
//! `HEIKOU_H_RATIO` 字高的墨块——它与鱼尾同色（`FISH_INK`），上承版框、
//! 下接地脚，中间才是鱼尾与书口字。
//!
//! ## 二、折痕：极细实线
//!
//! 版心正中那道线是**纸对折的物理折痕**，不是界栏：0.5px、`#999999`、
//! **实线**（v1.28.0 用户口径「移除虚线，改为极细的灰色实线」）。
//! 旧的虚线是拿一串短横拼的（GPUI 0.2.2 无 dashed 边框），放大看是一排
//! 断续墨点，在中缝里比鱼尾还吵；现在只剩一条淡灰直线，安静得像纸的褶。
//!
//! ## 三、书口文字：按预算分配
//!
//! 版心高是死的（跟着版框走），文字是活的：`layout` 先扣掉两枚鱼尾与
//! 朱印，再按「叶码 → 卷次 → 书名 → 篇题」的优先级把剩余高度分下去，
//! 分不够就截字（篇题至少要两个字才肯露面），绝不溢出、绝不压字。

/// 鱼尾宽 / 版心宽：约一列字宽的三成半（v1.28.0 由 0.70 再收一半）。
///
/// 用户口径「鱼尾尺寸缩小 50%」——旧版在纸上是一大块墨，压过书口文字；
/// 收到三成半后，鱼尾退成一枚记号，版心留白才透得过气。
pub const FISHTAIL_W_RATIO: f32 = 0.36;
/// 信笺（14 列）的鱼尾更宽：它独占一列，照参考图几乎顶满。
pub const LETTER_TAIL_W_RATIO: f32 = 0.92;
/// 版心鱼尾的**高 / 宽**：略竖长的黑鱼尾（太瘦长就成两条竖杠）。
pub const FISHTAIL_ASPECT: f32 = 1.30;
/// 信笺鱼尾的高 / 宽：横展（照用户「信笺」参考图的比例）。
pub const LETTER_TAIL_ASPECT: f32 = 0.78;
/// 鱼尾高（以「字高」为单位）：v1.28.0 收到 **0.50 字**——与宽度同步
/// 减半（0.95 → 0.50），整枚等比缩小 50%，比例不变、只变小。
/// 实际值还受 `FISHTAIL_ASPECT` 封顶。
pub const FISHTAIL_H_CHARS: f32 = 0.50;
pub const FISHTAIL_H_MAX_CHARS: f32 = 0.80;
/// **骨架**（高的相对比例，0 = 顶边）：两侧尾翼的深度。
/// 1.0 = 尾翼一直垂到形状下缘（刀刻的直边）。
pub const TAIL_WING: f32 = 1.00;
/// **骨架**：中央凹口顶点的深度——比尾翼**浅**得多，缺口因此是锐角 V。
///
/// v1.28.0 由 0.38 加深到 **0.62**：旧值只挖掉三成八的高度，缺口是个
/// 浅槽，整枚看着像「一块带豁口的墨」；0.62 时缺口斜边与顶边的夹角约
/// 64°，才是刻本那种刀刻的锐利倒 V（下鱼尾自动镜像成正 V）。
pub const TAIL_APEX: f32 = 0.62;
/// 信笺鱼尾不追求字高比例（那会把横展的鱼尾拉成竖条）。
pub const LETTER_TAIL_H_CHARS: f32 = 1.2;
/// 下限：再窄的屏也认得出这是枚鱼尾。
pub const FISHTAIL_W_MIN: f32 = 9.0;
pub const FISHTAIL_H_MIN: f32 = 10.0;

/// **黑口**的厚度 / 字高：版心天头、地脚各一道黑块（大黑口本子）。
///
/// v1.28.0 由 0.30 收到 **0.13**：0.30 字高在纸上是一条宽墨带，把版心
/// 上下堵死；0.13 只是一道「口线」，翻书时够认、平时不抢戏。
pub const HEIKOU_H_RATIO: f32 = 0.13;
/// 黑口最薄处（px）：再薄的两道墨就糊成一条灰线了。
pub const HEIKOU_H_MIN: f32 = 2.0;

/// **黑鱼尾**的墨色：深灰 `#333333`，不是纯黑——纯黑在纸面上太跳，
/// 也压过了正文的墨色。深底（夜读）时向纸色兑淡，否则整枚看不见。
pub const FISH_INK: u32 = 0x333333;
/// 折痕色：极淡的灰 `#999999`。
pub const FOLD_INK: u32 = 0x999999;
/// 折痕线宽（px）。
pub const FOLD_W: f32 = 0.5;
/// 折痕虚线的段长与理想段距（px）——v1.28.0 起不再用（保留给旧测试）。
pub const FOLD_DASH: f32 = 4.0;
pub const FOLD_GAP: f32 = 4.0;

/// 鱼尾尺寸：给定版心宽、字高与比例，算出（宽, 高）。
///
/// 高取 `字高 × FISHTAIL_H_CHARS`（v1.25.0 收到 0.95 字），再用
/// 「宽 × 比例」封顶——鱼尾再想要一字高，也不能长得比版心宽还离谱
/// （那会变成两条竖杠）；下限保护窄屏。要更大 / 更小改上面四个常量
/// 即可，渲染层不用动。
pub fn tail_size(gutter_w: f32, cell_h: f32, w_ratio: f32, aspect: f32, max_chars: f32) -> (f32, f32) {
    let w = (gutter_w * w_ratio)
        .min((gutter_w - 2.0).max(FISHTAIL_W_MIN))
        .max(FISHTAIL_W_MIN);
    let h = (cell_h * FISHTAIL_H_CHARS)
        .min(w * aspect)
        .min(cell_h * max_chars)
        .max(FISHTAIL_H_MIN);
    (w, h)
}

/// 黑口厚度：版心天头 / 地脚那两道粗黑块。约三成字高，最薄 4px。
///
/// 它与鱼尾同色同料，但职责不同：鱼尾是「口的形」，黑口是「口的线」——
/// 大黑口本子的书口，上下各压一道墨，翻书时一眼定位。
pub fn heikou_h(cell_h: f32) -> f32 {
    (cell_h * HEIKOU_H_RATIO).max(HEIKOU_H_MIN)
}

/// 版心（中缝）对鱼尾的尺寸。
pub fn banxin_tail_size(gutter_w: f32, cell_h: f32) -> (f32, f32) {
    tail_size(
        gutter_w,
        cell_h,
        FISHTAIL_W_RATIO,
        FISHTAIL_ASPECT,
        FISHTAIL_H_MAX_CHARS,
    )
}

/// 信笺中缝对鱼尾的尺寸（更宽、更横展）。
pub fn letter_tail_size(gutter_w: f32, cell_h: f32) -> (f32, f32) {
    tail_size(
        gutter_w,
        cell_h,
        LETTER_TAIL_W_RATIO,
        LETTER_TAIL_ASPECT,
        LETTER_TAIL_H_CHARS,
    )
}

/// 折痕虚线的分段：返回 `(段数, 段距)`。
///
/// 段长固定，段距按可用高反算，故头尾严丝合缝；太短（不足两段）就
/// 退化成一整段实线，免得画出半截。
pub fn fold_dashes(len: f32, dash: f32) -> (usize, f32) {
    let dash = dash.max(1.0);
    if len <= dash * 2.0 + FOLD_GAP {
        return (1, 0.0);
    }
    let unit = dash + FOLD_GAP;
    let n = ((len + FOLD_GAP) / unit).floor().max(2.0) as usize;
    let gap = (len - n as f32 * dash) / (n as f32 - 1.0);
    (n, gap.max(0.5))
}

/// 一枚鱼尾的**五个折点**（viewBox 坐标：宽 100、高 `h`）。
///
/// 上鱼尾（`top = true`，顶边在 y=0、凹口朝下）：
///
/// ```text
///   1 左上 (0, 0)  →  2 右上 (100, 0)  →  3 右翼下角 (100, h·0.6)
///                  →  4 中心凹口 (50, h·0.3)  →  5 左翼下角 (0, h·0.6)
/// ```
///
/// 下鱼尾是上鱼尾的**垂直镜像**（底边 `by = h·TAIL_WING`，凹口朝上），
/// 故整枚 `y → span - y`（`span = h·TAIL_WING` 是形状实际占的高度）
/// ——镜像只在这一处体现，不会再写反。
///
/// 全是 `f32` 常量运算，**绝无贝塞尔**：刻本的刀是直的，鱼尾就得是
/// 直线多边形。要改形制动这两个骨架常量即可（`TAIL_WING` / `TAIL_APEX`）。
pub fn fishtail_points(top: bool, h: f32) -> [(f32, f32); 5] {
    let span = h * TAIL_WING;
    let apex = h * TAIL_APEX;
    let up = [
        (0.0, 0.0),    // 1 左上
        (100.0, 0.0),  // 2 右上
        (100.0, span), // 3 右翼下角
        (50.0, apex),  // 4 中心凹口（锐角）
        (0.0, span),   // 5 左翼下角
    ];
    if top {
        up
    } else {
        up.map(|(x, y)| (x, span - y))
    }
}

/// 生成一枚鱼尾的 SVG（`top = true` 上鱼尾，凹口朝下；`false` 下鱼尾，
/// 凹口朝上——两枚相对成「对鱼尾」）。
///
/// `aspect` 是 高/宽：版心用 `FISHTAIL_ASPECT`（竖长），信笺用
/// `LETTER_TAIL_ASPECT`（横展）。viewBox 恒为 `100 × 100×aspect`。
///
/// **绝不用 `<rect>`，也绝不用 `C`（贝塞尔）**：整枚是 `fishtail_points`
/// 五个折点连成的闭合多边形，只有 `M` / `L` / `Z`。GPUI 只取 SVG 的
/// alpha 通道再用元素 text_color 上色，故 fill 必须是不透明实色。
pub fn fishtail_svg(top: bool, aspect: f32) -> String {
    let a = aspect.clamp(0.45, 2.6);
    let h = 100.0 * a;
    // 折点最深只到 `cy + 基准·TAIL_WING`，故基准放大到 `h / TAIL_WING`
    // 再算——整枚等比放大到**铺满 viewBox**，形状一模一样，但不会在鱼尾
    // 下方凭空多出一截留白把书口字挤走（`TAIL_WING = 1.0` 时基准即 `h`）。
    let base = h / TAIL_WING;
    let d: Vec<String> = fishtail_points(top, base)
        .iter()
        .enumerate()
        .map(|(i, (x, y))| {
            let cmd = if i == 0 { 'M' } else { 'L' };
            format!("{}{:.2} {:.2}", cmd, x, y)
        })
        .collect();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 {:.1}\"><path d=\"{} Z\" fill=\"#333333\"/></svg>",
        h,
        d.join(" ")
    )
}

/// 版心的一行竖排文字（书名 / 卷次 / 篇题 / 叶码同型）。
#[derive(Clone, Debug, PartialEq)]
pub struct BanxinLine {
    /// 已按预算截好的文本（逐字竖排）。
    pub text: String,
    /// 字号。
    pub fs: f32,
    /// 每字占位高（字号 + 行间）。
    pub lh: f32,
    /// true = 淡墨（书名 / 卷次 / 叶码）；false = 正墨（篇题）。
    pub soft: bool,
}

/// 版心版式：两枚鱼尾的尺寸 + 夹在中间的文字。
#[derive(Clone, Debug, PartialEq)]
pub struct BanxinLayout {
    /// 鱼尾宽 / 高（两枚同形）。
    pub tail_w: f32,
    pub tail_h: f32,
    /// 上鱼尾之下、叶码之上：书名 → 卷次 → 篇题。
    pub above: Vec<BanxinLine>,
    /// 叶码：紧贴下鱼尾之上。
    pub below: Vec<BanxinLine>,
    /// 朱印高（0 = 不画印）。
    pub seal_h: f32,
    /// 段间留白（鱼尾与文字、文字与文字之间）。
    pub gap: f32,
}

/// `layout` 的入参（全是纯数据，便于单测）。
pub struct BanxinOpts<'a> {
    /// 版心宽（界栏宽）。
    pub gutter_w: f32,
    /// 版心可用高（已扣版框与上下留白）。
    pub band_h: f32,
    /// 一格字的高（行高）。
    pub cell_h: f32,
    /// 正文字号。
    pub fs: f32,
    /// 书名（已去括号）。
    pub book: &'a str,
    /// 卷次（如「卷五十一」，已拼好汉字数词）。
    pub juan: &'a str,
    /// 叶码（如「五十」，已拼好汉字数词）。
    pub page: &'a str,
    /// 篇题（章题）。
    pub chapter_title: &'a str,
    /// false = 只立一对鱼尾（左半叶是章首时，章题列已经当了版心）。
    pub detailed: bool,
    /// 是否在下鱼尾之下落朱印。
    pub seal: bool,
}

/// 排一版版心：`上鱼尾 → 书名 / 卷次 / 篇题 → 叶码 → 下鱼尾 → 朱印`。
///
/// 高度账：先扣死两枚鱼尾、朱印与段间留白，剩下的按优先级
/// 「叶码 → 卷次 → 书名 → 篇题」分。分不够就截字；篇题至少两个字
/// 才肯露面（一个字的篇题不如不写）。
pub fn layout(o: BanxinOpts) -> BanxinLayout {
    let (tail_w, mut tail_h) = banxin_tail_size(o.gutter_w, o.cell_h);
    // 书口小字：极小号宋体——收在版心宽的一半以内，且不超过正文字号
    // 的三分之一（刻本书口字远小于正文）。
    let small = (o.gutter_w * 0.46).min(o.fs * 0.34).max(7.0);
    let lh_small = small * 1.22;
    // 篇题比书口字大一档，但仍小于正文（它是版心里的题名，不是标题）。
    let title_fs = (o.gutter_w * 0.60).min(o.fs * 0.66).max(8.0);
    let lh_title = title_fs * 1.20;
    let gap = (o.cell_h * 0.10).clamp(4.0, 10.0);
    let mut seal_h = if o.seal { o.gutter_w * 1.60 } else { 0.0 };
    // 固定支出：两枚鱼尾 + 朱印 + 四段留白（鱼尾下 / 叶码上 / 下鱼尾下 / 印上）。
    let mut budget = o.band_h - tail_h * 2.0 - seal_h - gap * 4.0;
    let mut above: Vec<BanxinLine> = Vec::new();
    let mut below: Vec<BanxinLine> = Vec::new();
    if !o.detailed {
        return BanxinLayout {
            tail_w,
            tail_h,
            above,
            below,
            seal_h,
            gap,
        };
    }
    // 叶码是翻检的命根子：先给它留位。留不出就先砍朱印（装饰），
    // 再压鱼尾高度（形制），绝不把叶码挤没了。
    let need_page = o.page.chars().count() as f32 * lh_small;
    if budget < need_page {
        seal_h = 0.0;
        budget = o.band_h - tail_h * 2.0 - gap * 4.0;
    }
    if budget < need_page {
        tail_h = ((o.band_h - need_page - gap * 4.0) / 2.0)
            .max(FISHTAIL_H_MIN)
            .min(tail_h);
        budget = o.band_h - tail_h * 2.0 - gap * 4.0;
    }
    // 1) 叶码。
    let page = take(o.page, &mut budget, lh_small, o.page.chars().count());
    if !page.is_empty() {
        below.push(BanxinLine {
            text: page,
            fs: small,
            lh: lh_small,
            soft: true,
        });
    }
    // 2) 卷次（卷五十一）——整条要么全上，要么不上：截半截「卷五」不像话。
    let jn = o.juan.chars().count();
    if jn > 0 && jn as f32 * lh_small <= budget {
        let juan = take(o.juan, &mut budget, lh_small, jn);
        above.push(BanxinLine {
            text: juan,
            fs: small,
            lh: lh_small,
            soft: true,
        });
    }
    // 3) 书名（宋词三百首）：最多 10 字，截到预算为止（刻本也常省称）。
    let book = take(o.book, &mut budget, lh_small, 10);
    if !book.is_empty() {
        above.push(BanxinLine {
            text: book,
            fs: small,
            lh: lh_small,
            soft: true,
        });
    }
    // 排定：书名在上、卷次次之（刻本书口「书名 → 卷次 → 叶码」的次序）。
    if above.len() == 2 {
        above.swap(0, 1);
    }
    // 4) 篇题：余量有多少排多少，不足两字则整条不排。
    let n_max = (budget / lh_title).floor() as usize;
    if n_max >= 2 {
        let tt = take(o.chapter_title, &mut budget, lh_title, n_max.min(8));
        if tt.chars().count() >= 2 {
            above.push(BanxinLine {
                text: tt,
                fs: title_fs,
                lh: lh_title,
                soft: false,
            });
        }
    }
    BanxinLayout {
        tail_w,
        tail_h,
        above,
        below,
        seal_h,
        gap,
    }
}

/// 从 `s` 里按预算取字：每取一个字扣一格高，取够 `max` 或预算见底为止。
fn take(s: &str, budget: &mut f32, lh: f32, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if out.chars().count() >= max || *budget < lh {
            break;
        }
        *budget -= lh;
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一张典型纸面：版心宽 50（≈一行字宽）、字高 55、版心可用高 740。
    const GW: f32 = 50.0;
    const CH: f32 = 55.0;
    const BAND: f32 = 740.0;

    #[test]
    fn tail_never_wider_than_the_gutter() {
        let (w, h) = banxin_tail_size(GW, CH);
        assert!(w <= GW, "鱼尾宽 {w} 超过界栏 {GW}");
        // 高按 0.50 字起算，但封顶在「宽 × 比例」——不会长成两条竖杠。
        assert!(h <= CH * FISHTAIL_H_MAX_CHARS + 0.01, "鱼尾高 {h} 超过 0.8 字");
        // v1.28.0「缩小 50%」的修正：高四成字、宽三成半版心。
        assert!(h <= CH * 0.52, "鱼尾仍嫌高（应 ≲ 半字）：{h}");
        assert!(w <= GW * 0.40, "鱼尾仍嫌宽（应 ≲ 0.36 版心）：{w}");
        // 也不能瘦成一条线：至少要有三分之一个字高。
        assert!(h >= CH * 0.30, "鱼尾太瘦弱：{h}");
        // 相对旧版（0.95 字 / 0.70 版心）确实缩到了五成上下。
        assert!(h < CH * 0.95 * 0.60, "没缩到位：{h}");
    }

    #[test]
    fn notch_is_a_sharp_v_not_a_shallow_dip() {
        // 尾翼垂到底（TAIL_WING = 1.0）、凹口顶点深达 0.62：
        // 缺口斜边与顶边夹角约 64°，才是刀刻的锐角 V；
        // 旧值 0.38 只挖掉三成八，是「一道豁口」不是鱼尾。
        assert!((TAIL_WING - 1.0).abs() < 1e-6);
        assert!(TAIL_APEX > 0.55, "缺口太浅（{}），不够锐", TAIL_APEX);
        // 顶边两点同在 y = 0：顶部平齐，不是尖顶也不是圆顶。
        let pts = fishtail_points(true, 100.0);
        assert_eq!(pts[0].1, 0.0);
        assert_eq!(pts[1].1, 0.0);
        assert_eq!(pts[0].0, 0.0);
        assert_eq!(pts[1].0, 100.0);
        // 缺口半角 = atan(半宽 / 深)，像素空间里宽 = 100、深 = 100·aspect·APEX。
        let half = (50.0 / (100.0 * FISHTAIL_ASPECT * TAIL_APEX)).atan().to_degrees();
        assert!(half * 2.0 <= 70.0, "缺口不够尖：{:.1}°", half * 2.0);
    }

    #[test]
    fn heikou_is_a_thin_band_at_head_and_foot() {
        // 一成三字高（v1.28.0 由 0.30 收到 0.13），且永不薄于 2px。
        assert!((heikou_h(50.0) - 50.0 * HEIKOU_H_RATIO).abs() < 0.01);
        assert_eq!(heikou_h(8.0), HEIKOU_H_MIN);
        assert!(heikou_h(CH) <= CH * 0.16, "黑口仍嫌厚：{}", heikou_h(CH));
        // 黑口 + 上鱼尾 + 书口字 + 叶码仍要塞得进一列版心。
        assert!(heikou_h(CH) * 2.0 + CH < BAND);
    }

    #[test]
    fn letter_tail_is_wide_not_tall() {
        let (w, h) = letter_tail_size(46.0, CH);
        assert!(w > 40.0, "信笺鱼尾应几乎顶满中缝：{w}");
        assert!(h < w, "信笺鱼尾应横展：{w}×{h}");
    }

    #[test]
    fn fish_tail_is_a_path_never_a_rect() {
        for top in [true, false] {
            let s = fishtail_svg(top, FISHTAIL_ASPECT);
            assert!(!s.contains("<rect"), "不许再用矩形：{s}");
            assert!(s.contains("<path d=\"M"), "必须是路径：{s}");
            // **严禁贝塞尔**：黑鱼尾是刀刻的直线几何，一处 C 都不能有。
            assert!(!s.contains('C'), "不许用贝塞尔曲线（要锐利的直线）：{s}");
            // 一枚五边形：1 个起点 + 4 段 lineTo。
            assert_eq!(s.matches('L').count(), 4, "折点数不对：{s}");
            // viewBox 高 = 100 × 高宽比。
            let want = format!("viewBox=\"0 0 100 {:.1}\"", 100.0 * FISHTAIL_ASPECT);
            assert!(s.contains(&want), "viewBox 错（应为 {want}）：{s}");
        }
    }

    #[test]
    fn fishtail_follows_the_five_point_skeleton() {
        let h = 100.0 * FISHTAIL_ASPECT;
        // 上鱼尾：左上 → 右上 → 右翼下角 → 中心凹口 → 左翼下角。
        let up = fishtail_points(true, h);
        let want = [
            (0.0, 0.0),
            (100.0, 0.0),
            (100.0, h * TAIL_WING),
            (50.0, h * TAIL_APEX),
            (0.0, h * TAIL_WING),
        ];
        assert_eq!(up, want, "上鱼尾折点偏离刻本骨架：{up:?}");
        // 凹口顶点必须比尾翼下缘**高**（浅），缺口才是锐角 V、开口朝下。
        assert!(TAIL_APEX < TAIL_WING);
        // 下鱼尾 = 上鱼尾的垂直镜像（绕形状自身的中轴翻，底边 by = span）。
        let span = h * TAIL_WING;
        let down = fishtail_points(false, h);
        for i in 0..5 {
            assert!((down[i].0 - want[i].0).abs() < 1e-6, "镜像不该动 x：{down:?}");
            assert!(
                (down[i].1 - (span - want[i].1)).abs() < 1e-6,
                "下鱼尾必须是垂直镜像：{down:?}"
            );
        }
    }

    #[test]
    fn both_tails_fill_their_own_box() {
        // 两枚各自铺满自己的元素高（`fishtail_svg` 把基准放大到
        // `h / TAIL_WING`）——不然鱼尾下方会凭空空出一截，把中间书口
        // （叶码「二十三」）的地盘吃掉。
        let h = 100.0 * FISHTAIL_ASPECT / TAIL_WING;
        let span = h * TAIL_WING;
        for top in [true, false] {
            let s = fishtail_svg(top, FISHTAIL_ASPECT);
            let d = s.split("d=\"").nth(1).unwrap().split('"').next().unwrap();
            let nums: Vec<f32> = d
                .replace(['M', 'L', 'Z'], " ")
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            let ys: Vec<f32> = nums.iter().skip(1).step_by(2).copied().collect();
            let lo = ys.iter().fold(f32::INFINITY, |a, b| a.min(*b));
            let hi = ys.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b));
            // 包围盒正好是 [0, viewBox 高]：0 → 100×aspect。
            assert!(lo.abs() < 0.01, "顶边没贴住：{lo}");
            assert!((hi - span).abs() < 0.01, "底边没铺满：{hi} / {span}");
        }
    }

    #[test]
    fn pair_opens_toward_each_other() {
        // 上鱼尾：顶边一刀平直、两侧尾翼垂到下缘、中间 V 形凹口朝下；
        // 下鱼尾是它的镜像——底边平直、尾翼向上、凹口朝上。两枚相对。
        let h = 100.0 * FISHTAIL_ASPECT / TAIL_WING; // fishtail_svg 内部的基准
        let span = h * TAIL_WING;
        let up = fishtail_points(true, h);
        let down = fishtail_points(false, h);
        // 上鱼尾：顶边（点 1/2）在 y=0，两个翼角（点 3/5）在 y=span。
        assert_eq!(up[0].1, 0.0, "上鱼尾顶边应贴 y=0：{up:?}");
        assert!((up[2].1 - span).abs() < 1e-6, "上鱼尾尾翼应垂到下缘：{up:?}");
        // 下鱼尾：底边在 y=span，两个翼角在 y=0。
        assert!((down[0].1 - span).abs() < 1e-6, "下鱼尾底边应在 y=span：{down:?}");
        assert!(down[2].1.abs() < 1e-6, "下鱼尾尾翼应向上：{down:?}");
        // 两枚的凹口顶点关于水平中轴互补（一枚的深 = 另一枚的浅）。
        assert!(
            (up[3].1 + down[3].1 - span).abs() < 1e-6,
            "凹口不成镜像：{:?} {:?}",
            up,
            down
        );
        // 凹口比尾翼浅，缺口才是锐角 V 而不是一条缝。
        assert!(up[3].1 < up[2].1 && up[3].1 > 0.0, "凹口深度不对：{up:?}");
    }

    #[test]
    fn fishtail_svg_is_closed_and_stays_in_bounds() {
        for aspect in [FISHTAIL_ASPECT, LETTER_TAIL_ASPECT] {
            for top in [true, false] {
                let s = fishtail_svg(top, aspect);
                assert!(s.contains('Z'), "路径未闭合：{s}");
                let d = s.split("d=\"").nth(1).unwrap().split('"').next().unwrap();
                let nums: Vec<f32> = d
                    .replace(['M', 'L', 'Z'], " ")
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                // 一枚五边形 = 5 个折点 = 10 个数。
                assert_eq!(nums.len(), 10, "点数不对：{}", nums.len() / 2);
                let h = 100.0 * aspect;
                for c in nums.chunks(2) {
                    assert!((-0.01..=100.01).contains(&c[0]), "x 越界：{}", c[0]);
                    assert!((-0.01..=h + 0.01).contains(&c[1]), "y 越界：{}", c[1]);
                }
            }
        }
    }

    /// 目检用：`cargo test dump_fishtail -- --ignored --nocapture`。
    #[test]
    #[ignore]
    fn dump_fishtail() {
        println!("{}", fishtail_svg(true, FISHTAIL_ASPECT));
        println!("{}", fishtail_svg(false, FISHTAIL_ASPECT));
    }

    #[test]
    fn fold_dashes_fit_the_band_exactly() {
        let (n, gap) = fold_dashes(700.0, FOLD_DASH);
        assert!(n >= 2);
        let total = n as f32 * FOLD_DASH + (n - 1) as f32 * gap;
        assert!((total - 700.0).abs() < 0.5, "虚线没铺满：{total} / 700");
        assert!(gap > 0.0);
        // 太短时退化成一整段，不画半截。
        let (n2, _) = fold_dashes(5.0, FOLD_DASH);
        assert_eq!(n2, 1);
    }

    #[test]
    fn book_and_juan_sandwiched_by_the_tails() {
        let lay = layout(BanxinOpts {
            gutter_w: GW,
            band_h: BAND,
            cell_h: CH,
            fs: 37.6,
            book: "宋词三百首",
            juan: "卷五十一",
            page: "五十",
            chapter_title: "菩萨蛮",
            detailed: true,
            seal: true,
        });
        // 两枚鱼尾之间：上段书名 → 卷次 → 篇题，下段叶码。
        assert_eq!(lay.above.len(), 3, "{:?}", lay.above);
        assert_eq!(lay.above[0].text, "宋词三百首");
        assert_eq!(lay.above[1].text, "卷五十一");
        assert_eq!(lay.above[2].text, "菩萨蛮");
        assert_eq!(lay.below[0].text, "五十");
        // 全部文字 + 两枚鱼尾 + 朱印 不超出可用高。
        let used = lay.tail_h * 2.0
            + lay.seal_h
            + lay.gap * 4.0
            + lay.above.iter().chain(lay.below.iter())
                .map(|l| l.text.chars().count() as f32 * l.lh)
                .sum::<f32>();
        assert!(used <= BAND, "版心塞不下：{used} > {BAND}");
    }

    #[test]
    fn title_drops_out_when_the_band_is_short() {
        let lay = layout(BanxinOpts {
            gutter_w: GW,
            band_h: 260.0, // 极短的版心：只够鱼尾 + 叶码
            cell_h: CH,
            fs: 37.6,
            book: "宋词三百首",
            juan: "卷五十一",
            page: "五十",
            chapter_title: "菩萨蛮",
            detailed: true,
            seal: true,
        });
        let used = lay.tail_h * 2.0
            + lay.seal_h
            + lay.gap * 4.0
            + lay.above.iter().chain(lay.below.iter())
                .map(|l| l.text.chars().count() as f32 * l.lh)
                .sum::<f32>();
        assert!(used <= 260.0, "短版心溢出了：{used}");
        // 叶码是翻检的命根子，再挤也要在。
        assert_eq!(lay.below[0].text, "五十");
    }

    #[test]
    fn bare_gutter_has_only_the_pair() {
        // 左半叶是章首：章题列已经当了版心，中缝只立一对鱼尾。
        let lay = layout(BanxinOpts {
            gutter_w: GW,
            band_h: BAND,
            cell_h: CH,
            fs: 37.6,
            book: "宋词三百首",
            juan: "卷五十一",
            page: "五十",
            chapter_title: "菩萨蛮",
            detailed: false,
            seal: true,
        });
        assert!(lay.above.is_empty() && lay.below.is_empty());
        assert!(lay.tail_h > 0.0);
    }

    #[test]
    fn tiny_font_stays_inside_the_gutter() {
        let lay = layout(BanxinOpts {
            gutter_w: 24.0, // 极窄屏
            band_h: 400.0,
            cell_h: 26.0,
            fs: 18.0,
            book: "宋词三百首",
            juan: "卷五十一",
            page: "五十",
            chapter_title: "菩萨蛮",
            detailed: true,
            seal: false,
        });
        for l in lay.above.iter().chain(lay.below.iter()) {
            // 书口字（书名 / 卷次 / 叶码）收在版心宽的 46%，篇题大一档 60%。
            let cap = if l.soft { 24.0 * 0.46 } else { 24.0 * 0.60 };
            assert!(l.fs <= cap + 0.01, "书口字溢出版心：{}", l.fs);
            assert!(l.fs >= 7.0);
        }
    }
}
