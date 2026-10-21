//! 版心（中缝）版式：**对鱼尾** + 折痕 + 书口文字（书名 / 卷次 / 篇题 / 叶码）。
//!
//! 刻本的版心只有「一格字」宽，却要立住整叶的门面：上下一对鱼尾夹住
//! 书名、卷次、叶码，中间一道纸对折的折痕。这套几何全部是**纯计算**
//! （与 `west.rs` / `ruby.rs` 同路数），渲染层只负责把算好的尺寸与文字
//! 摆上去，方便单测与回归。
//!
//! ## 一、鱼尾：燕尾形，不是矩形
//!
//! 早先直接 `fillRect` 两枚实心黑块，既不符合刻本形制，视觉上也极重。
//! 现在用**路径**（`M` = moveTo、`L` = lineTo、`C` = 三次贝塞尔）画真正的
//! 燕尾（对鱼尾）：
//!
//! ```text
//!       上鱼尾（∧ 口朝下）            下鱼尾（∨ 口朝上）
//!     ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄          ▄▄            ▄▄
//!     ██████████████████         ████          ████
//!     ████▄▄▄▄▄▄▄▄▄▄████        ██████▄▄  ▄▄██████
//!      ████  ▝▀▀▘  ████           ▀████▄▄▄▄████▀
//!       ▀██         ██▀              ▀███▄▄███▀
//! ```
//!
//! * 上缘平直略弓（贴版框的一边），两侧外缘下行为**肩**（最宽处）；
//! * 自肩收出一对**流线尾翼**，翼尖略向内倾，形似鱼尾分叉；
//! * 中央开一道**凹口**（上鱼尾 Λ、下鱼尾 V），两枚**开口相对**；
//! * 全部坐标走贝塞尔，另加 ±0.7 的确定性抖点，缩到几十像素时略带
//!   木刻毛边，不是矢量图那种绝对平直的机器边。
//!
//! > 尺寸上的硬约束：鱼尾宽不得超过一格字宽（界栏）。宽定在版心的
//! > 0.78（两侧各留一点余白），高按 **1.5 字**给（`FISHTAIL_H_CHARS`），
//! > 再用高宽比 1.75 封住——两道约束一起把鱼尾压在「一列宽、一字半高」
//! > 的刻本尺度里，既不像原先的黑方块那样压纸，也不会拉成两条竖杠。
//! > 要更大 / 更小改这两个数即可，渲染层不用动。
//!
//! ## 二、折痕：极细虚线
//!
//! 版心正中那道线是**纸对折的物理折痕**，不是界栏：0.5px、`#999999`、
//! 虚线。GPUI 0.2.2 没有元素级 dashed 边框，故用一串短横拼出来，
//! 段距按可用高反算（`fold_dashes`），头尾严丝合缝不留残段。
//!
//! ## 三、书口文字：按预算分配
//!
//! 版心高是死的（跟着版框走），文字是活的：`layout` 先扣掉两枚鱼尾与
//! 朱印，再按「叶码 → 卷次 → 书名 → 篇题」的优先级把剩余高度分下去，
//! 分不够就截字（篇题至少要两个字才肯露面），绝不溢出、绝不压字。

/// 鱼尾宽 / 版心宽：不超过界栏，两侧各留一点余白。
pub const FISHTAIL_W_RATIO: f32 = 0.78;
/// 信笺（14 列）的鱼尾更宽：它独占一列，照参考图几乎顶满。
pub const LETTER_TAIL_W_RATIO: f32 = 0.92;
/// 版心鱼尾的**高 / 宽**：竖长的燕尾。刻意略低于「1.5 字」的上限：
/// 39 宽 × 1.65 ≈ 64 ≈ 1.43 字——再拉高（1.9）两枚尾翼就细成獠牙，
/// 反而比原先的黑方块更刺眼。
pub const FISHTAIL_ASPECT: f32 = 1.65;
/// 信笺鱼尾的高 / 宽：横展（照用户「信笺」参考图的比例）。
pub const LETTER_TAIL_ASPECT: f32 = 0.78;
/// 鱼尾高的目标值与硬上限（以「字高」为单位，用户定的 1.5 ~ 2 字）。
/// 实际值受 `FISHTAIL_ASPECT` 的宽度封顶约束，通常落在 1.0~1.2 字。
pub const FISHTAIL_H_CHARS: f32 = 1.5;
pub const FISHTAIL_H_MAX_CHARS: f32 = 2.0;
/// 骨架：上缘 / 肩 / 凹口顶点 / 尾尖的**相对高**（0 = 上缘，1 = 底）。
pub const Y_TOP: f32 = 0.06;
pub const Y_SHOULDER: f32 = 0.24;
pub const Y_APEX: f32 = 0.40;
pub const Y_TIP: f32 = 0.97;
/// 骨架：右半的四个关键 x（左半一律取 `100 - x`，镜像不会再写反）。
pub const X_TIP: f32 = 76.0;
pub const X_INNER: f32 = 65.0;
pub const X_OUTER: f32 = 92.0;
/// 信笺鱼尾不追求 1.5 字高（那会把横展的燕尾拉成竖条）。
pub const LETTER_TAIL_H_CHARS: f32 = 1.2;
/// 下限：再窄的屏也认得出这是枚鱼尾。
pub const FISHTAIL_W_MIN: f32 = 9.0;
pub const FISHTAIL_H_MIN: f32 = 10.0;
/// 抖点幅度（viewBox 100 单位下 ±0.7）：木刻毛边。
pub const JITTER: f32 = 1.4;

/// **黑鱼尾**的墨色：深灰 `#333333`，不是纯黑——纯黑在纸面上太跳，
/// 也压过了正文的墨色。深底（夜读）时向纸色兑淡，否则整枚看不见。
pub const FISH_INK: u32 = 0x333333;
/// 折痕色：极淡的灰 `#999999`。
pub const FOLD_INK: u32 = 0x999999;
/// 折痕线宽（px）。
pub const FOLD_W: f32 = 0.5;
/// 折痕虚线的段长与理想段距（px）。
pub const FOLD_DASH: f32 = 4.0;
pub const FOLD_GAP: f32 = 4.0;

/// 鱼尾尺寸：给定版心宽、字高与比例，算出（宽, 高）。
///
/// 高取 `字高 × FISHTAIL_H_CHARS`，再用「宽 × 比例」封顶——鱼尾再想要
/// 1.5 字高，也不能长得比版心宽还离谱（那会变成两条竖杠）；下限保护
/// 窄屏。要更大 / 更小改上面四个常量即可，渲染层不用动。
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

/// 生成一枚鱼尾的 SVG（`top = true` 上鱼尾，开口朝下；`false` 下鱼尾，
/// 开口朝上——两枚相对成「对鱼尾」）。
///
/// `aspect` 是 高/宽：版心用 `FISHTAIL_ASPECT`（竖长），信笺用
/// `LETTER_TAIL_ASPECT`（横展）。viewBox 恒为 `100 × 100×aspect`。
///
/// **绝不用 `<rect>`**：整枚由一条闭合路径构成——上缘 `C` 略弓、两侧
/// 外缘 `C` 下行、尾翼内外缘 `C` 收成流线、中央 `C` 拐出凹口顶点。
/// GPUI 只取 SVG 的 alpha 通道再用元素 text_color 上色，故 fill 必须
/// 是不透明实色。
pub fn fishtail_svg(top: bool, aspect: f32) -> String {
    let a = aspect.clamp(0.45, 2.6);
    let h = 100.0 * a;
    // 归一化骨架（宽 100、高 100×aspect）。右半四个关键 x 由常量给，
    // 左半一律 `100 - x`——镜像只在这一处体现，不会再写反。
    let y_top = h * Y_TOP;
    let y_sh = h * Y_SHOULDER;
    let y_ax = h * Y_APEX;
    let y_tip = h * Y_TIP;
    let mut pen = Pen { h, top, n: 0 };
    let mut t: Vec<String> = Vec::new();
    t.push(format!("M{}", pen.p(3.0, y_top)));
    // 上缘：一条略向上弓的横线（刻本里鱼尾的上缘贴着版框，微微鼓起）。
    t.push("C".into());
    t.push(pen.p(32.0, y_top - h * 0.030));
    t.push(pen.p(68.0, y_top - h * 0.030));
    t.push(pen.p(97.0, y_top));
    // 右外缘：贴边下行到肩（最宽处）。
    t.push("C".into());
    t.push(pen.p(100.0, y_top + (y_sh - y_top) * 0.50));
    t.push(pen.p(100.0, y_sh - (y_sh - y_top) * 0.35));
    t.push(pen.p(99.0, y_sh));
    // 右外缘：自肩收向尾尖（尾翼外缘，先外凸后内收的流线）。
    t.push("C".into());
    t.push(pen.p(99.0, y_sh + (y_tip - y_sh) * 0.40));
    t.push(pen.p(X_OUTER, y_sh + (y_tip - y_sh) * 0.80));
    t.push(pen.p(X_TIP, y_tip));
    // 右尾内缘：尾尖回升到凹口顶点——横坐标一路内收，凹口的 V 壁
    // 因此是弧而不是直边（「优美的凹陷」全在这一段）。
    t.push("C".into());
    t.push(pen.p(X_INNER, y_ax + (y_tip - y_ax) * 0.58));
    t.push(pen.p(X_INNER - 6.0, y_ax + (y_tip - y_ax) * 0.22));
    t.push(pen.p(50.0, y_ax));
    // 左尾内缘：顶点下行到左尾尖（镜像）。
    t.push("C".into());
    t.push(pen.p(100.0 - X_INNER + 6.0, y_ax + (y_tip - y_ax) * 0.22));
    t.push(pen.p(100.0 - X_INNER, y_ax + (y_tip - y_ax) * 0.58));
    t.push(pen.p(100.0 - X_TIP, y_tip));
    // 左外缘：自左尾尖回升到肩，再回到上缘。
    t.push("C".into());
    t.push(pen.p(100.0 - X_OUTER, y_sh + (y_tip - y_sh) * 0.80));
    t.push(pen.p(1.0, y_sh + (y_tip - y_sh) * 0.40));
    t.push(pen.p(1.0, y_sh));
    t.push("C".into());
    t.push(pen.p(0.0, y_sh - (y_sh - y_top) * 0.35));
    t.push(pen.p(0.0, y_top + (y_sh - y_top) * 0.50));
    t.push(pen.p(3.0, y_top));
    t.push("Z".into());
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 {:.1}\"><path d=\"{}\" fill=\"#333333\"/></svg>",
        h,
        t.join(" ")
    )
}

/// 画路径用的小笔：负责「下鱼尾 = 上鱼尾上下翻转」与确定性抖点。
struct Pen {
    h: f32,
    top: bool,
    n: usize,
}

impl Pen {
    /// 取一个点：`top = false` 时整枚沿水平中轴翻转（开口朝上，与
    /// 上鱼尾相对）；每个坐标再加一个由序号决定的微小抖点（木刻毛边）。
    fn p(&mut self, x: f32, y: f32) -> String {
        self.n += 1;
        let j = |i: usize| -> f32 {
            let v = (i.wrapping_mul(2_654_435_761) % 997) as f32;
            (v / 997.0 - 0.5) * JITTER
        };
        let y = if self.top { y } else { self.h - y };
        // 抖点后夹回 viewBox，免得边缘被裁出一段不该有的直边。
        let x = (x + j(self.n)).clamp(0.0, 100.0);
        let y = (y + j(self.n + 61)).clamp(0.0, self.h);
        format!("{:.1} {:.1}", x, y)
    }
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
        // 高按 1.5 字高起算，但封顶在「宽 × 比例」——不会长成两条竖杠。
        assert!(h <= CH * FISHTAIL_H_MAX_CHARS + 0.01, "鱼尾高 {h} 超过 2 字");
        assert!(h >= CH * 1.0, "鱼尾高 {h} 不足 1 字高（太瘦弱）");
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
            // 贝塞尔：一枚燕尾至少七段 C（上缘 / 两侧外缘 / 两侧内缘 / 两肩）。
            assert!(s.matches("C").count() >= 7, "曲线段太少：{s}");
            // viewBox 高 = 100 × 高宽比（1.30 → 130.0）。
            let want = format!("viewBox=\"0 0 100 {:.1}\"", 100.0 * FISHTAIL_ASPECT);
            assert!(s.contains(&want), "viewBox 错（应为 {want}）：{s}");
        }
    }

    #[test]
    fn pair_opens_toward_each_other() {
        // 上鱼尾的凹口在下半（顶点 y 大），下鱼尾的凹口在上半——
        // 两枚开口相对。取中轴附近（x≈50）那一点比 y：上鱼的顶点靠上、
        // 翻转后靠下。不按下标硬取，免得路径一改测试就假失败。
        let apex_y = |top: bool| -> f32 {
            let s = fishtail_svg(top, FISHTAIL_ASPECT);
            let d = s.split("d=\"").nth(1).unwrap().split('"').next().unwrap();
            let nums: Vec<f32> = d
                .replace(['M', 'C', 'Z'], " ")
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            let pts: Vec<(f32, f32)> = nums.chunks(2).map(|c| (c[0], c[1])).collect();
            let mid: Vec<f32> = pts
                .iter()
                .filter(|(x, _)| (*x - 50.0).abs() < 1.5)
                .map(|(_, y)| *y)
                .collect();
            assert!(!mid.is_empty(), "路径里找不到中轴顶点：{s}");
            mid.iter().sum::<f32>() / mid.len() as f32
        };
        let up = apex_y(true);
        let down = apex_y(false);
        assert!(up < down, "上鱼尾凹口应在下：{up} vs {down}");
    }

    #[test]
    fn fishtail_svg_is_closed_and_stays_in_bounds() {
        for aspect in [FISHTAIL_ASPECT, LETTER_TAIL_ASPECT] {
            for top in [true, false] {
                let s = fishtail_svg(top, aspect);
                assert!(s.contains("Z\""), "路径未闭合：{s}");
                let d = s.split("d=\"").nth(1).unwrap().split('"').next().unwrap();
                let nums: Vec<f32> = d
                    .replace(['M', 'C', 'Z'], " ")
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                // 1 个起点 + 7 段 × 3 个控制点 = 22 个点。
                assert_eq!(nums.len(), 44, "点数不对：{}", nums.len() / 2);
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
