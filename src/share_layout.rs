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
//! 1. **强制首行对齐**：每一列的第一个字 `y` 恒等于 `y0`，绝不因列长而错动
//!    （对应阅读侧的 `grid::top_baseline`）。
//! 2. **从右向左**：第 0 列在最右，符合古籍行款。
//! 3. **禁用 `unwrap`**：越界 / 除零一律走默认值。

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
    pub lines: Vec<PlacedLine>,
    pub chars: Vec<PlacedChar>,
    pub seal: Option<PlacedSeal>,
}

/// 版式输入（调用方快照，渲染期只读）。
pub struct LayoutSpec<'a> {
    pub body: &'a [char],
    /// 与 `body` 等长：第 i 字是否被用户划了红线（朱批高亮）。
    pub red: &'a [bool],
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
/// 朱批引线宽（px，按卡片宽缩放后的值）。
pub const LEAD_W: f32 = 1.0;

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
pub fn punct_box(p: &PlacedChar) -> (f32, f32, f32) {
    // 竖排古籍的句读挂在字的右上方，不占整格——正文仍按整格推进。
    (p.x + p.cell * 0.42, p.y + p.cell * 0.04, p.fs * 0.62)
}

/// 排一张卡：底色 → 界框 → 竖排正文（含高亮 / 批注列 / 引线）→ 落款 → 朱印。
pub fn layout(s: &LayoutSpec) -> CardLayout {
    let p = palette(s.bg);
    let w = if s.w.is_finite() { s.w } else { 1080.0 }.max(240.0);
    let h = if s.h.is_finite() { s.h } else { 1920.0 }.max(360.0);
    let pad = (w * 0.075).clamp(18.0, 46.0);

    let mut out = CardLayout {
        w,
        h,
        bg: p.bg,
        font: s.font,
        ..Default::default()
    };

    // ---- 界框：双线（外淡内深），刻本「界行」的规矩 ----
    if let Some(ink) = frame_ink(s.frame, &p) {
        let o1 = (pad * 0.38).max(5.0);
        let o2 = (pad * 0.66).max(9.0);
        out.rects.push(PlacedRect {
            x: o1,
            y: o1,
            w: (w - o1 * 2.0).max(20.0),
            h: (h - o1 * 2.0).max(20.0),
            color: (ink << 8) | 0x4D,
            stroke: Some((w / 1080.0).max(0.6)),
        });
        out.rects.push(PlacedRect {
            x: o2,
            y: o2,
            w: (w - o2 * 2.0).max(20.0),
            h: (h - o2 * 2.0).max(20.0),
            color: (ink << 8) | 0xA6,
            stroke: Some((w / 1080.0 * 1.6).max(1.0)),
        });
    }

    // ---- 内容区 ----
    let inner_x = pad;
    let inner_y = pad;
    let inner_w = (w - pad * 2.0).max(80.0);
    let inner_h = (h - pad * 2.0).max(120.0);

    let n = s.body.len();
    let cols_max = norm_columns(s.columns).max(1);
    // 开「展示朱批」且有批注：最左让出一列给批注（竖排右起，左为后进）。
    let want_note = s.show_notes && !s.note.is_empty() && n > 0;
    let note_cols = if want_note { 1 } else { 0 };

    // 字格：先按「满列铺开」取，装不下就整体缩字号（绝不加列、绝不溢出）。
    let mut cell = inner_w / (cols_max + note_cols) as f32;
    let mut fs = (cell / CELL_RATIO).max(8.0);
    let mut rows = (inner_h / cell).floor().max(3.0);
    let mut guard = 0;
    while (cols_max as f32 * rows) < n as f32 && fs > 9.0 && guard < 40 {
        fs = (fs * 0.92).max(9.0);
        cell = fs * CELL_RATIO;
        rows = (inner_h / cell).floor().max(3.0);
        guard += 1;
    }

    // 容量上限：字太多（超长选区）就按能装下的量截断——绝不为了塞满而把
    // 行数撑爆、把列首顶出纸外（`rows_used` 因此永不超过 `rows`）。
    let cap = cols_max * rows.max(1.0) as usize;
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
    let block_h = rows_used as f32 * cell;
    // 居中：批注列留在正文块的左侧，故整体右移一列。
    let x0 = inner_x + ((inner_w - block_w - note_cols as f32 * cell) * 0.5).max(0.0)
        + note_cols as f32 * cell;
    // 铁律 1：列首 y 恒等于 y0 —— 每列都从这里起笔，绝不因列长而错动。
    let y0 = inner_y + ((inner_h - block_h) * 0.5).max(0.0);

    // ---- 正文：从右向左，逐格落字 ----
    let mut first_marked: Option<usize> = None;
    for (i, &ch) in s.body.iter().enumerate().take(shown) {
        let col = i / rows_used.max(1);
        let row = i % rows_used.max(1);
        let x = x0 + (cols_used as f32 - 1.0 - col as f32) * cell;
        let y = y0 + row as f32 * cell;
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
        out.chars.push(PlacedChar {
            ch,
            x,
            y,
            cell,
            fs,
            color,
            punct: is_punct(ch),
        });
    }

    // ---- 朱批列（杀手锏）：正文块左侧一列小字 + 极细朱砂引线 ----
    if want_note {
        let note_fs = (fs * NOTE_FS_RATIO).max(9.0);
        let note_cell = note_fs * CELL_RATIO;
        let nx = x0 - cell; // 最左一列的左邻
        let take = ((inner_h * 0.62) / note_cell).floor() as usize;
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
            });
            ny += note_cell;
            let _ = i;
        }
        // 引线：被批的第一个字（无划线则取首字）→ 批注首字，极细朱砂横引。
        let anchor = first_marked.unwrap_or(0);
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

    // ---- 落款与朱印：卡片左下角（古籍里「版心外」的题识位）----
    let seal_size = (w * 0.085).clamp(28.0, 44.0);
    let inset = (pad * 0.55).max(8.0);
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
    let base_bottom = h - inset - seal_size - 10.0;
    // 可用高：从版框天头到印章上沿。落款按它截字，绝不溢出卡片。
    let avail = (base_bottom - pad).max(meta_cell);
    let take_meta = (avail / meta_cell).floor() as usize;
    for (k, (_x, text)) in cols_meta.iter().enumerate() {
        let cx = inset + (cols_meta.len() as f32 - 1.0 - k as f32) * meta_cell;
        let chars: Vec<char> = text.chars().take(take_meta).collect();
        let block = chars.len() as f32 * meta_cell;
        let top = (base_bottom - block).max(pad);
        for (i, ch) in chars.iter().enumerate() {
            out.chars.push(PlacedChar {
                ch: *ch,
                x: cx,
                y: top + i as f32 * meta_cell,
                cell: meta_cell,
                fs: meta_fs,
                color: p.meta,
                punct: is_punct(*ch),
            });
        }
    }

    out.seal = Some(PlacedSeal {
        x: inset,
        y: h - inset - seal_size,
        size: seal_size,
        kind: s.seal,
        text: s.seal_text.to_string(),
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1080.0;
    const H: f32 = 1920.0;

    fn spec<'a>(body: &'a [char], red: &'a [bool]) -> LayoutSpec<'a> {
        LayoutSpec {
            body,
            red,
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
            w: W,
            h: H,
        }
    }

    /// 铁律 1：每一列的第一个字，y 必须绝对相同（首行顶格）。
    #[test]
    fn every_column_starts_on_the_same_baseline() {
        let body: Vec<char> = "道可道非常道名可名非常名無名天地之始有名萬物之母".chars().collect();
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
}
