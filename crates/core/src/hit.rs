//! 字形坐标反查与浮签智能避让（纯计算，零 GPUI 依赖，可单测）。
//!
//! 两件事，都是「几何账」，与渲染同源、与渲染分开：
//!
//! **一、坐标反查（长按取字）**
//! 元素级事件（每个字格挂 `on_mouse_down`）能告诉「点了哪个字」，但
//! 落在格子缝隙、夹注块、或者拖选结束的那一下，元素不一定命中。这里
//! 走另一条路：**只用窗口 (x, y)** 反推字——先定位落在哪一叶哪一列，
//! 再按列内各片的**半格账**往下数，直接得到那个字的 `Glyph`
//! （`chapter / para / offset` 就是 charIndex）与它在屏幕上的矩形。
//! 半格账与 `gloss::Filler` 同源：正文一格 2 半格、夹注一段
//! `ceil(字数/2)` 半格、西文串按 `west::run_cells` 折算。
//!
//! **二、浮签避让**
//! 浮签不能压住被查的那个字（压住就看不見自己查的是谁）。`place_card`
//! 先按「哪边空间大往哪边展开」定边，再用「底边对齐字底 / 顶边对齐字顶」
//! 定纵向，最后做一次**相交检查**：若卡片矩形与字的矩形相交，就沿展开
//! 方向把卡片推到字外；推不动才退到纵向推开，最后统一夹回视口。

use crate::gloss::{Page, Piece};
use crate::types::{Glyph, GlyphKind};

/// 一叶纸面的几何（反查用；与 `render_ancient` 同源）。
// 字段是与渲染逐项对齐的**完整**几何（`rows` 供分页校验用），不一定都被
// 反查读到，故整块不参与死代码检查。
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct LeafFrame {
    /// 字格宽（列宽）。
    pub cell_w: f32,
    /// 字格高（行高）。
    pub cell_h: f32,
    /// 每列行数。
    pub rows: usize,
    /// 正文列数（章首页已扣掉章题列）。
    pub text_cols: usize,
    /// 本叶是否章首（最右多一个章题列）。
    pub start_chapter: bool,
    /// 章题列宽（格数，见 `ANCIENT_TITLE_COL`）。
    pub title_cols: f32,
    /// 信笺模板：叶内插一道「一列宽」的中缝。
    pub show_fish_tail: bool,
    /// 中缝插在哪一列**之左**（读序号，见 `render_ancient` 的 `split_i`）。
    pub gutter_at: usize,
    /// 章首页开卷列顶部预留的空白行数（卷首题署）。
    pub reserve: usize,
    /// 西文成组（`!han_digits`）：两字符一格。
    pub west_mode: bool,
}

/// 一个字的包围盒（**纸面内坐标**：原点 = 该叶内框左上，与 `NoteBox` 同系）。
// `contains` 与 `key` 是给反查 / 浮层避让用的公开几何接口，本 crate 内部
// 未必每帧都走到，故整块不参与死代码检查。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct GlyphBox {
    pub glyph: Glyph,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[allow(dead_code)]
impl GlyphBox {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
    /// 在纸面（内框）坐标系里的键——换算 charIndex 就靠它。
    pub fn key(&self) -> (usize, usize, usize) {
        (self.glyph.chapter, self.glyph.para, self.glyph.offset)
    }
}

/// 一片在列内的纵向占位（半格账的结果）。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Span {
    /// 该片在 `items` 里的下标。
    pub index: usize,
    pub y: f32,
    pub h: f32,
    pub kind: SpanKind,
}

/// 一片的内容：正文单字 / 成组西文 / 夹注 / 避头尾空位。
#[derive(Clone, Debug)]
pub enum SpanKind {
    Main(Glyph),
    /// 连续西文（两字一格）：组内字符按格分组，与渲染的槽位一致。
    West(Vec<Glyph>),
    Gloss,
    Gap,
}

/// 一列内逐片的纵向占位（与渲染、与 `gloss::Filler` 同源）。
///
/// * 正文一格 = `cell_h`；
/// * 避头尾空位一格 = `cell_h`（渲染里确确实实占了一格）；
/// * 夹注一段 = `ceil(字数/2)` 个**半格**（半格 = `cell_h/2`）；
/// * 连续西文 = `west::run_cells` 格（两字一格）。
pub fn column_spans(items: &[Piece], cell_h: f32, west_mode: bool) -> Vec<Span> {
    let half = cell_h / 2.0;
    let mut out = Vec::new();
    let mut y = 0.0f32;
    let mut i = 0usize;
    while i < items.len() {
        match &items[i] {
            // 数字串（防呆）：整串成组，纵向每字半格（`223` → 1.5 格），
            // 与分页（半格账）和渲染（整串横躺）同源。
            Piece::Main(g) if west_mode && crate::west::is_digit(g.ch) => {
                let mut j = i;
                let mut chars: Vec<Glyph> = Vec::new();
                while j < items.len() {
                    match &items[j] {
                        Piece::Main(g2) if crate::west::in_digit_run(g2.ch, true) => {
                            chars.push(*g2);
                            j += 1;
                        }
                        _ => break,
                    }
                }
                let h = chars.len() as f32 * half;
                out.push(Span {
                    index: i,
                    y,
                    h,
                    kind: SpanKind::West(chars),
                });
                y += h;
                i = j;
            }
            Piece::Main(g) if west_mode && crate::west::is_west(g.ch) => {
                let mut j = i;
                let mut chars: Vec<Glyph> = Vec::new();
                while j < items.len() {
                    match &items[j] {
                        Piece::Main(g2) if crate::west::is_west(g2.ch) => {
                            chars.push(*g2);
                            j += 1;
                        }
                        _ => break,
                    }
                }
                let cells = crate::west::run_cells(chars.iter().map(|g| g.ch));
                let h = cells as f32 * cell_h;
                out.push(Span {
                    index: i,
                    y,
                    h,
                    kind: SpanKind::West(chars),
                });
                y += h;
                i = j;
            }
            // 挤压标点：与前一个字同格，纵向**不占位**（h = 0）。
            // 记成 cell_h 会把它后面所有片整体下移一格 → 点字点偏。
            Piece::Main(g) if g.is_hang() => {
                out.push(Span {
                    index: i,
                    y,
                    h: 0.0,
                    kind: SpanKind::Main(*g),
                });
                i += 1;
            }
            Piece::Main(g) => {
                out.push(Span {
                    index: i,
                    y,
                    h: cell_h,
                    kind: SpanKind::Main(*g),
                });
                y += cell_h;
                i += 1;
            }
            Piece::Gloss(text, _, _) => {
                let h = text.chars().count().div_ceil(2) as f32 * half;
                out.push(Span {
                    index: i,
                    y,
                    h,
                    kind: SpanKind::Gloss,
                });
                y += h;
                i += 1;
            }
            Piece::Gap => {
                out.push(Span {
                    index: i,
                    y,
                    h: cell_h,
                    kind: SpanKind::Gap,
                });
                y += cell_h;
                i += 1;
            }
        }
    }
    out
}

/// 一列内所有**正文字**的包围盒（纸面内坐标）。
///
/// 西文成组时一格两字并排（当前渲染退化为同槽位并排直立），故一格出
/// **两个**盒，各占半格宽——反查能精确到那一个字符。段首缩进占位
/// （`GlyphKind::Indent`）不渲染，不产盒。
///
/// `li` 是**布局序**（flex_row 里左数第几列），故 x = `li × cell_w`。
#[allow(dead_code)]
pub fn glyph_boxes_in_column(
    items: &[Piece],
    li: usize,
    cell_w: f32,
    cell_h: f32,
    west_mode: bool,
) -> Vec<GlyphBox> {
    boxes_in_column(items, li as f32 * cell_w, cell_w, cell_h, west_mode)
}

/// [`glyph_boxes_in_column`] 的实段：列左缘 x 由调用方直接给。
fn boxes_in_column(
    items: &[Piece],
    x0: f32,
    cell_w: f32,
    cell_h: f32,
    west_mode: bool,
) -> Vec<GlyphBox> {
    let mut out = Vec::new();
    for sp in column_spans(items, cell_h, west_mode) {
        match sp.kind {
            SpanKind::Main(g) => {
                // 缩进占位不渲染；挤压标点与前一字同格，它的命中区就是
                // 前一字那一格（由前一字的盒覆盖），不再另出盒。
                if g.kind == GlyphKind::Indent || g.kind == GlyphKind::Hang {
                    continue;
                }
                out.push(GlyphBox {
                    glyph: g,
                    x: x0,
                    y: sp.y,
                    w: cell_w,
                    h: sp.h,
                });
            }
            SpanKind::West(chars) => {
                let per = (sp.h / cell_h).round().max(1.0) as usize;
                for (k, g) in chars.iter().enumerate() {
                    let cell = k / 2;
                    let slot = k % 2;
                    if cell >= per {
                        break;
                    }
                    out.push(GlyphBox {
                        glyph: *g,
                        x: x0 + slot as f32 * (cell_w / 2.0),
                        y: sp.y + cell as f32 * cell_h,
                        w: cell_w / 2.0,
                        h: cell_h,
                    });
                }
            }
            // 夹注 / 空位不是正文字符：不参与取字（夹注有自己的命中与菜单）。
            SpanKind::Gloss | SpanKind::Gap => {}
        }
    }
    out
}

/// 一叶里**全部正文字**的包围盒（纸面内坐标，原点 = 内框左上）。
///
/// 按**读序**逐列收集（`i = 0` 在最右），x 走 [`col_x`]（含信笺中缝那一列
/// 的让位），章首开卷列顶部的卷首题署预留带整条下移——与
/// [`glyph_at`] 反查用的是同一套坐标，故「点到的字」与「算出的字」是同一个。
///
/// 天头眉批的**绝对锚定**就靠它：`MarginNote::anchor_char_index` →
/// `Book::key_from_flat` → 在 `Page` 里找这个 key 的盒 → 屏幕坐标。
pub fn glyph_boxes_of_columns(columns: &[Vec<Piece>], f: &LeafFrame) -> Vec<GlyphBox> {
    let mut out = Vec::new();
    for i in 0..f.text_cols {
        let items: &[Piece] = columns.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
        let x = col_x(f, i);
        // 卷首题署预留带：带内的正文整体下移（与 `ancient_column` 的
        // `top_skip` 同源），否则算出来的字会比屏幕上看到的高一截。
        let skip = if f.start_chapter && i == 0 {
            f.reserve as f32 * f.cell_h
        } else {
            0.0
        };
        for mut gb in boxes_in_column(items, x, f.cell_w, f.cell_h, f.west_mode) {
            gb.y += skip;
            out.push(gb);
        }
    }
    out
}

/// 一叶里所有正文字的包围盒（[`glyph_boxes_of_columns`] 的 `Page` 版）。
#[allow(dead_code)]
pub fn glyph_boxes(page: &Page, f: &LeafFrame) -> Vec<GlyphBox> {
    glyph_boxes_of_columns(&page.columns, f)
}

/// **按字形键**在这一叶里找那个字的包围盒——`anchor_char_index` 落到屏幕
/// 坐标的最后一步。
///
/// 键即 `(章, 段, 段内偏移)`，与 [`GlyphBox::key`] 同口径。不在本叶返回
/// `None`（眉批锚的字不在当前这一面，这一帧就不画它）。
pub fn glyph_box_of_columns(
    columns: &[Vec<Piece>],
    f: &LeafFrame,
    key: (usize, usize, usize),
) -> Option<GlyphBox> {
    glyph_boxes_of_columns(columns, f)
        .into_iter()
        .find(|gb| gb.key() == key)
}

/// 读序号 `i` 的列在纸面里的左缘 x（flex_row 里「左数第几列」）。
///
/// 竖排自右向左读：`i = 0` 落在最右；信笺中缝插在 `gutter_at` 之左，
/// 故读序号 ≤ `gutter_at` 的列整体右移一格。
pub fn col_x(f: &LeafFrame, i: usize) -> f32 {
    if i >= f.text_cols {
        return f.text_cols as f32 * f.cell_w;
    }
    let li = f.text_cols.saturating_sub(1).saturating_sub(i);
    let gutter = if f.show_fish_tail && i <= f.gutter_at {
        f.cell_w
    } else {
        0.0
    };
    li as f32 * f.cell_w + gutter
}

/// 纸面总宽（正文列 + 中缝 + 章题列），反查越界判定用。
pub fn frame_w(f: &LeafFrame) -> f32 {
    let mut w = f.text_cols as f32 * f.cell_w;
    if f.show_fish_tail {
        w += f.cell_w;
    }
    if f.start_chapter {
        w += f.title_cols * f.cell_w;
    }
    w
}

/// 纸面内坐标 → 字：只用 (x, y) 反推，不依赖元素级事件。
///
/// 命中策略：先按 x 落在第几列（含中缝那一列的空当 → 不命中），再在
/// 列内按片的 y 区间找字；落在夹注块或空位上返回 `None`（那些有各自的
/// 交互入口）。章首页开卷列顶部是卷首题署预留带，带内不算命中。
pub fn glyph_at(page: &Page, x: f32, y: f32, f: &LeafFrame) -> Option<GlyphBox> {
    if x < 0.0 || y < 0.0 || x > frame_w(f) {
        return None;
    }
    // 逐列找：命中最靠左的那列（列不重叠，找到即返回）。
    for i in 0..f.text_cols {
        let cx = col_x(f, i);
        if x < cx || x > cx + f.cell_w {
            continue;
        }
        let skip = if f.start_chapter && i == 0 {
            f.reserve as f32 * f.cell_h
        } else {
            0.0
        };
        let ly = y - skip;
        if ly < 0.0 {
            return None;
        }
        let items: &[Piece] = page.columns.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
        for sp in column_spans(items, f.cell_h, f.west_mode) {
            if ly < sp.y || ly > sp.y + sp.h {
                continue;
            }
            match &sp.kind {
                SpanKind::Main(g) => {
                    // 缩进占位 / 挤压标点不单独命中：前者无字，后者归前一字那格。
                    if g.kind == GlyphKind::Indent || g.kind == GlyphKind::Hang {
                        return None;
                    }
                    return Some(GlyphBox {
                        glyph: *g,
                        x: cx,
                        y: sp.y + skip,
                        w: f.cell_w,
                        h: sp.h,
                    });
                }
                SpanKind::West(chars) => {
                    // 组内一格两字并排：按 y 定格、按 x 定格内左右。
                    let cell = ((ly - sp.y) / f.cell_h).floor() as usize;
                    let k = cell * 2;
                    let slot = if x >= cx + f.cell_w / 2.0 { 1 } else { 0 };
                    let g = chars.get(k + slot).or_else(|| chars.get(k)).copied()?;
                    return Some(GlyphBox {
                        glyph: g,
                        x: cx + slot as f32 * (f.cell_w / 2.0),
                        y: sp.y + cell as f32 * f.cell_h + skip,
                        w: f.cell_w / 2.0,
                        h: f.cell_h,
                    });
                }
                // 夹注 / 空位：不是正文字符。
                SpanKind::Gloss | SpanKind::Gap => return None,
            }
        }
        return None;
    }
    None
}

// ------------------------------------------------------------------
// 浮签智能避让
// ------------------------------------------------------------------

/// 被查的那个字在屏幕上的矩形（浮签要绕开它）。
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Anchor {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn overlaps(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let ox = (self.x - (x + w)).min(x - self.right());
        let oy = (self.y - (y + h)).min(y - self.bottom());
        ox < 0.0 && oy < 0.0
    }
}

/// 浮签往哪边展开（决定「向左 / 向右」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
}

/// 浮签纵向怎么落（决定「向下 / 向上展开」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vert {
    Down,
    Up,
}

/// 浮签落位结果（视口坐标）。
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub left: f32,
    pub top: f32,
    pub side: Side,
    pub vert: Vert,
}

#[allow(dead_code)]
impl Placement {
    pub fn rect(&self, w: f32, h: f32) -> (f32, f32, f32, f32) {
        (self.left, self.top, w, h)
    }
}

/// 视口留白：浮签不贴屏幕边。
pub const CARD_PAD: f32 = 8.0;
/// 浮签与被查字之间的**最小安全间距**（v1.45.0 交互重构 · 任务二）：
/// 浮签边缘与选中文字必须保持至少 20px 的绝对距离，绝不覆盖在被选中的字上；
/// `place_card` 定边 / 翻转 / 排遮挡三步都以此为基准外推。
pub const CARD_GAP: f32 = 20.0;

/// 浮签的智能落位：**优先不遮挡被查的那个字**。
///
/// 步骤（纯几何，无分支嵌套地狱）：
///   1. **定边**：字右边放得下就往右展开；放不下就看左边；两边都不够就
///      选空间大的一侧（随后夹回视口）。—— 这就是「字在屏幕右侧，浮签
///      向左展开」。
///   2. **定纵向**：默认顶边与字顶齐（向下展开）；若下方超出视口，改成
///      底边与字底齐（**向上展开**）；再不够就贴视口底。
///   3. **排遮挡**：若夹回视口后卡片仍压住那个字，沿展开方向把卡片推到
///      字外；横推不动就纵推（推到字下方 / 上方），最后统一夹回视口。
pub fn place_card(
    a: &Anchor,
    card_w: f32,
    card_h: f32,
    vw: f32,
    vh: f32,
    pad: f32,
    gap: f32,
) -> Placement {
    let max_left = (vw - pad - card_w).max(pad);
    let max_top = (vh - pad - card_h).max(pad);

    // 1) 定边
    let room_right = vw - pad - (a.right() + gap);
    let room_left = (a.x - gap) - pad;
    let side = if room_right >= card_w {
        Side::Right
    } else if room_left >= card_w {
        Side::Left
    } else if room_right >= room_left {
        Side::Right
    } else {
        Side::Left
    };
    let left = match side {
        Side::Right => a.right() + gap,
        Side::Left => a.x - gap - card_w,
    };

    // 2) 定纵向
    let room_down = vh - pad - (a.y + card_h);
    let vert = if room_down >= 0.0 {
        Vert::Down
    } else {
        Vert::Up
    };
    let top = match vert {
        Vert::Down => a.y,
        // 底边与字底齐：卡片整体向上展开（字在屏幕底部时就是这条路）。
        Vert::Up => a.bottom() - card_h,
    };

    // 3) 夹回视口
    let left = left.clamp(pad, max_left);
    let top = top.clamp(pad, max_top);

    // 4) 排遮挡：夹回后若仍压住字，先横推、再纵推。
    let (left, top) = if a.overlaps(left, top, card_w, card_h) {
        let want = match side {
            Side::Right => a.right() + gap,
            Side::Left => a.x - gap - card_w,
        };
        if want >= pad && want <= max_left {
            // 横推得出去：卡片整体挪到字的另一侧，绝不再压着它。
            (want, top)
        } else {
            // 横向推不出去（视口太窄）：改成纵向推到字下 / 字上。
            let down = a.bottom() + gap;
            let up = a.y - gap - card_h;
            let t = if down + card_h <= vh - pad { down } else { up };
            (left, t)
        }
    } else {
        (left, top)
    };
    let left = left.clamp(pad, max_left);
    let top = top.clamp(pad, max_top);

    Placement {
        left,
        top,
        side,
        vert,
    }
}

/// 浮签的竖排排版度量：卡片多大、释义排几列、每列几字。
///
/// 浮签是**竖排**的：第一列大字、第二列拼音、后几列释义。卡片高度先定
/// （视口高的六成，但不越过视口），再按高度反推「一列能排几个字」，
/// 最后才得出宽度——宽是算出来的，不是拍的，这样长释义才不会撑破屏幕。
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct CardMetrics {
    /// 卡片宽（含双线框与内边距）。
    pub w: f32,
    /// 卡片高。
    pub h: f32,
    /// 释义列数。
    pub cols: usize,
    /// 每列字数。
    pub per_col: usize,
    /// 释义字号。
    pub gloss_fs: f32,
}

/// 竖排浮签的度量：按视口与释义长度反算。
#[allow(dead_code)]
pub fn card_metrics(gloss_len: usize, vw: f32, vh: f32) -> CardMetrics {
    // 释义字号：小字（13），行距 1.16 倍（竖排逐字叠下，行距即字距）。
    let gloss_fs = 13.0f32;
    let line = gloss_fs * 1.16;
    // 高度上限：视口六成（再高就成了半屏，失去「小纸条」的轻）。
    let max_h = (vh * 0.62).clamp(220.0, (vh - CARD_PAD * 2.0).max(220.0));
    // 扣双线框 + 内边距 + 页眉 + **两行页脚**（charIndex 一行、按钮一行）。
    let inner_max = (max_h - 56.0).max(60.0);
    let per_col_limit = ((inner_max / line).floor() as usize).max(6);
    // 列数：按「一列最多 per_col_limit 字」铺开；最多 6 列（再多就成条幅了）。
    // v1.38.0：上限 6 → 8。双引擎浮签要塞「康熙 + 說文」两段，
    // 6 列装不下就截尾，八列恰好容下 110 字康熙 + 55 字說文而不必省略。
    let cols = if gloss_len == 0 {
        1
    } else {
        gloss_len.div_ceil(per_col_limit).clamp(1, 8)
    };
    // 每列字数：**均分**，让各列齐平（末列不至于只剩两个字）。
    let per_col = if gloss_len == 0 {
        per_col_limit
    } else {
        gloss_len.div_ceil(cols).min(per_col_limit).max(1)
    };
    // 高度随内容收缩：短释义就是一张矮纸条，不硬撑到六成屏。
    let content_h = per_col as f32 * line;
    let h = (content_h + 56.0).clamp(140.0, max_h);
    // 宽：字头列 40 + 拼音列 18 + 释义 n 列（每列 = 字号 × 1.6）+ 内边距。
    let w = 40.0 + 18.0 + cols as f32 * gloss_fs * 1.6 + 30.0;
    let w = w.clamp(240.0, (vw - CARD_PAD * 2.0).max(240.0));
    CardMetrics {
        w,
        h,
        cols,
        per_col,
        gloss_fs,
    }
}

/// 两个矩形是否分离（测试的判据：浮签不得压住被查的字）。
#[allow(dead_code)]
pub fn disjoint(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    ax + aw <= bx || bx + bw <= ax || ay + ah <= by || by + bh <= ay
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GlyphKind;

    fn g(ch: char, off: usize) -> Glyph {
        Glyph {
            chapter: 0,
            para: 0,
            offset: off,
            ch,
            kind: GlyphKind::Char,
        }
    }

    fn main_piece(ch: char, off: usize) -> Piece {
        Piece::Main(g(ch, off))
    }

    fn frame() -> LeafFrame {
        LeafFrame {
            cell_w: 40.0,
            cell_h: 44.0,
            rows: 10,
            text_cols: 8,
            start_chapter: false,
            title_cols: 1.0,
            show_fish_tail: false,
            gutter_at: 6,
            reserve: 0,
            west_mode: true,
        }
    }

    fn page_of(cols: Vec<Vec<Piece>>) -> Page {
        Page {
            glyphs: Vec::new(),
            start_chapter: false,
            columns: cols,
        }
    }

    #[test]
    fn column_spans_charge_notes_by_half_cells() {
        // 正文一格 44；夹注 5 字 = ceil(5/2)=3 半格 = 3×22 = 66。
        let items = vec![
            main_piece('甲', 0),
            Piece::Gloss(
                "注文共五字".into(),
                crate::types::NoteSource::Original,
                g('甲', 0),
            ),
            main_piece('乙', 1),
        ];
        let sp = column_spans(&items, 44.0, true);
        assert_eq!(sp.len(), 3);
        assert!((sp[0].h - 44.0).abs() < 0.01);
        assert!((sp[1].h - 66.0).abs() < 0.01);
        assert!((sp[2].y - 110.0).abs() < 0.01, "y = {}", sp[2].y);
    }

    #[test]
    fn reverse_lookup_finds_the_char_under_the_point() {
        // 一列 5 字，取第 3 字的中心。
        let col: Vec<Piece> = (0..5).map(|i| main_piece('字', i)).collect();
        let page = page_of(vec![col]);
        let f = frame();
        let cx = col_x(&f, 0);
        let hit = glyph_at(&page, cx + 20.0, 2.0 * 44.0 + 22.0, &f).expect("应命中");
        assert_eq!(hit.glyph.offset, 2);
        assert_eq!(hit.key(), (0, 0, 2));
        // 包围盒与渲染同源：一整格。
        assert!((hit.w - 40.0).abs() < 0.01);
        assert!((hit.h - 44.0).abs() < 0.01);
    }

    #[test]
    fn reverse_lookup_reads_right_to_left() {
        // 竖排自右向左：读序号 0 在最右（x 最大）。
        let mut cols = Vec::new();
        for i in 0..4 {
            cols.push(vec![main_piece('字', i)]);
        }
        let page = page_of(cols);
        let f = frame();
        // 第一列（i=0）应在最右：x = 7*40 = 280。
        assert!((col_x(&f, 0) - 280.0).abs() < 0.01);
        let hit = glyph_at(&page, 280.0 + 20.0, 22.0, &f).expect("最右列应命中");
        assert_eq!(hit.glyph.offset, 0);
        let hit2 = glyph_at(&page, 6.0 * 40.0 + 20.0, 22.0, &f).expect("次右列应命中");
        assert_eq!(hit2.glyph.offset, 1);
    }

    #[test]
    fn reverse_lookup_skips_gloss_and_indent() {
        let items = vec![
            main_piece('甲', 0),
            Piece::Gloss("注".into(), crate::types::NoteSource::Original, g('甲', 0)),
        ];
        let page = page_of(vec![items]);
        let f = frame();
        // 夹注区间内（y = 44..66）不返回正文字符。
        assert!(glyph_at(&page, 20.0, 50.0, &f).is_none());

        let mut indent = g(' ', 7);
        indent.kind = GlyphKind::Indent;
        let page2 = page_of(vec![vec![Piece::Main(indent)]]);
        assert!(glyph_at(&page2, 20.0, 22.0, &f).is_none());
    }

    #[test]
    fn reverse_lookup_groups_western_runs() {
        // "1628" 两字一格 → 两格；点到第二格右半 → 命中第 4 个字符。
        let items: Vec<Piece> = "1628".chars().enumerate().map(|(i, c)| main_piece(c, i)).collect();
        let page = page_of(vec![items]);
        let f = frame();
        let cx = col_x(&f, 0);
        let hit = glyph_at(&page, cx + 30.0, 44.0 + 22.0, &f).expect("第二格右半应命中");
        assert_eq!(hit.glyph.ch, '8');
        assert!((hit.w - 20.0).abs() < 0.01, "西文半格宽");
    }

    #[test]
    fn glyph_boxes_cover_every_column_in_reading_order() {
        // 四列各两字：盒数 = 8，且读序 0（最右）的 x 最大。
        let mut cols = Vec::new();
        for i in 0..4 {
            cols.push(vec![main_piece('字', i), main_piece('字', i + 10)]);
        }
        let f = frame();
        let boxes = glyph_boxes_of_columns(&cols, &f);
        assert_eq!(boxes.len(), 8);
        assert!((boxes[0].x - col_x(&f, 0)).abs() < 0.01, "首盒应在最右列");
        // 同一列内自上而下：y 递增。
        assert!(boxes[1].y > boxes[0].y);
    }

    #[test]
    fn glyph_box_of_key_finds_the_anchored_char() {
        // 眉批锚定「第 1 列第 2 字」：必须原样找回来，这是绝对锚定的地基。
        let mut cols = Vec::new();
        for i in 0..4 {
            cols.push(vec![main_piece('字', i), main_piece('字', i + 10)]);
        }
        let f = frame();
        let gb = glyph_box_of_columns(&cols, &f, (0, 0, 11)).expect("第 1 列第 2 字");
        assert_eq!(gb.key(), (0, 0, 11));
        // 与反查（点这个位置得到哪个字）必须一致。
        let hit = glyph_at(
            &page_of(cols.clone()),
            gb.x + gb.w * 0.5,
            gb.y + gb.h * 0.5,
            &f,
        )
        .expect("反查应命中同一处");
        assert_eq!(hit.key(), gb.key(), "算出的字与点到的字不是同一个");
        // 不在本叶的键 → None（这一帧不画）。
        assert!(glyph_box_of_columns(&cols, &f, (9, 9, 9)).is_none());
    }

    #[test]
    fn reserve_band_on_chapter_head_is_not_text() {
        let mut f = frame();
        f.start_chapter = true;
        f.reserve = 3;
        let page = page_of(vec![vec![main_piece('甲', 0)]]);
        let cx = col_x(&f, 0);
        // 预留带内（y < 3*44）不算命中；带下才是正文。
        assert!(glyph_at(&page, cx + 20.0, 44.0, &f).is_none());
        assert!(glyph_at(&page, cx + 20.0, 3.0 * 44.0 + 10.0, &f).is_some());
    }

    #[test]
    fn card_flips_left_when_the_char_sits_on_the_right() {
        let a = Anchor {
            x: 1200.0,
            y: 300.0,
            w: 40.0,
            h: 44.0,
        };
        let p = place_card(&a, 300.0, 220.0, 1280.0, 800.0, CARD_PAD, CARD_GAP);
        assert_eq!(p.side, Side::Left);
        assert!(p.left + 300.0 <= a.x, "必须落在字的左侧：{}", p.left);
        assert!(
            disjoint(p.rect(300.0, 220.0), (a.x, a.y, a.w, a.h)),
            "浮签不得压住被查的字"
        );
    }

    #[test]
    fn card_grows_upward_when_the_char_sits_low() {
        let a = Anchor {
            x: 400.0,
            y: 740.0,
            w: 40.0,
            h: 44.0,
        };
        let p = place_card(&a, 300.0, 220.0, 1280.0, 800.0, CARD_PAD, CARD_GAP);
        assert_eq!(p.vert, Vert::Up);
        assert!(p.top < a.y, "必须向上展开：{} vs {}", p.top, a.y);
        assert!(p.top + 220.0 <= 800.0 - CARD_PAD + 0.01, "不得超出视口底");
        assert!(disjoint(p.rect(300.0, 220.0), (a.x, a.y, a.w, a.h)));
    }

    #[test]
    fn card_never_leaves_the_viewport() {
        // 四个角落各来一次：结果必须整体在视口内。
        let corners = [
            (10.0, 10.0),
            (1270.0, 10.0),
            (10.0, 790.0),
            (1270.0, 790.0),
        ];
        for (x, y) in corners {
            let a = Anchor {
                x,
                y,
                w: 40.0,
                h: 44.0,
            };
            let p = place_card(&a, 320.0, 240.0, 1280.0, 800.0, CARD_PAD, CARD_GAP);
            assert!(p.left >= CARD_PAD - 0.01, "left={}", p.left);
            assert!(p.top >= CARD_PAD - 0.01, "top={}", p.top);
            assert!(p.left + 320.0 <= 1280.0 - CARD_PAD + 0.01, "right={}", p.left + 320.0);
            assert!(p.top + 240.0 <= 800.0 - CARD_PAD + 0.01, "bottom={}", p.top + 240.0);
        }
    }

    #[test]
    fn card_metrics_fit_the_viewport_and_wrap_long_glosses() {
        // 短释义：一列装得下，卡片不至于过宽。
        let m = card_metrics(20, 1280.0, 800.0);
        assert_eq!(m.cols, 1);
        assert!(m.w <= 1280.0 - 2.0 * CARD_PAD);
        // 长释义：多列铺开，宽度仍在视口内（绝不撑破屏幕）。
        let long = card_metrics(400, 1280.0, 800.0);
        assert!(long.cols > 1);
        assert!(long.cols <= 8);
        assert!(long.w <= 1280.0 - 2.0 * CARD_PAD + 0.01);
        // 高度不越过视口（留白两侧）。
        assert!(long.h <= 800.0 - 2.0 * CARD_PAD + 0.01);
        // 字头 + 拼音 + 释义列全在卡片内：每列至少 6 字，不会一列只排一两个字。
        assert!(m.per_col >= 6);
        // 短释义的卡片比长释义矮：高度随内容收缩，不硬撑成半屏。
        assert!(m.h < long.h, "短释义卡片应更矮：{} vs {}", m.h, long.h);
        // 极窄屏也不至于给出比屏还宽的卡片（宽高各有下限，只断言不崩、不越界）。
        let tiny = card_metrics(400, 320.0, 500.0);
        assert!(tiny.w >= 240.0 - 0.01 && tiny.h >= 140.0 - 0.01);
        assert!(tiny.w <= 320.0 - 2.0 * CARD_PAD + 0.01);
    }

    #[test]
    fn narrow_viewport_still_avoids_the_char_if_possible() {
        // 极窄屏：横推不出去，卡片只能压在字上——但至少不越界。
        let a = Anchor {
            x: 100.0,
            y: 200.0,
            w: 40.0,
            h: 44.0,
        };
        let p = place_card(&a, 300.0, 200.0, 320.0, 500.0, CARD_PAD, CARD_GAP);
        assert!(p.left >= CARD_PAD - 0.01);
        assert!(p.left + 300.0 <= 320.0 - CARD_PAD + 0.01);
    }
}
