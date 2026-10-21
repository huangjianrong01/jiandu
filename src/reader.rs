//! 阅读视图：古文竖笺（八行、从右到左、朱丝栏）与现代横笺（仅横线）。
//!
//! 标注体系：
//!   * 繁体标注 —— 朱红小字，竖文标字左、横文标字上；
//!   * 通假标注 —— 靛蓝小字，位置同繁体；
//!   * 注释     —— 朱红圈号 ①②③ 标字旁，页脚注释区列出注文；
//!   * 段落分隔 —— 段首空两格（首行缩进），不再放置任何段落标记符。

use crate::app::{
    Root, Screen, ANCIENT_EDGE, ANCIENT_TITLE_COL,
};
use crate::dict;
use crate::gloss::{Page, Piece};
use crate::grid::BorderStyle;
use crate::lines;
use crate::theme::{InkPalette, Theme};
use crate::trad;
use crate::types::{
    is_punct, underline_run_start, Book, CardLayout, Duplex, FontChoice, Glyph, GlyphHit,
    GlyphKey, GlyphKind, MarkKind, NoteKind, NoteSource, Script, StyleTab, TocEntry,
};
use gpui::{
    div, prelude::*, px, rgb, rgba, svg, Context, Div, FontWeight, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Overflow, ParentElement, SharedString,
    Styled, Window,
};
use std::collections::HashMap;

/// 分享卡底色：(底色, 正文字色, 落款 / 日期色)。对应分享面板的**四枚圆形**。
///
/// 原先五块（含靛青）：靛蓝是现代后台系统的颜色，与「古卷雅韵」相冲，
/// 故只留墨夜 / 朱漆 / 松烟 / 宣纸四色——正好四枚圆，一排排得开。
pub(crate) const SHARE_BGS: [(u32, u32, u32); 4] = [
    (0x161310, 0xE9E2D2, 0xA8957A), // 墨夜
    (0x2A1512, 0xF0DFD2, 0xC49A83), // 朱漆
    (0x14211B, 0xDDEAE0, 0x93AE9E), // 松烟
    (0xF2EAD6, 0x2B2622, 0x8A8175), // 宣纸
];

/// 朱砂红：笺读的落款印泥色（印在分享笺左下角）。
pub(crate) const CINNABAR: u32 = 0xC3272D;

/// 分享面板「朱砂印章」按钮的**外框**：比印面大一圈，悬停放大时从中心
/// 向外长，不会推挤旁边的控件（定位算式也按这个高度算面板总高）。
pub(crate) const SEAL_BOX: f32 = 52.0;
/// 朱砂印章的印面边长：古印是方的，不做圆角胶囊。
const SEAL_SIZE: f32 = 44.0;

/// 用户自加夹注的**深朱砂红**字色（#C62828）。
///
/// 与古籍原本的夹注（墨色兑淡）区分开：用户注用朱砂系，一眼可辨，
/// 且可右键 / 长按改删。比落款印泥色 `CINNABAR` 略深一档，避免二者混同。
pub(crate) const USER_NOTE_INK: u32 = 0xC62828;

/// 用户夹注底衬：深朱砂向纸色兑淡的比例（0.88 ≈ 只剩一成多朱砂）。
/// 要求「极其微弱」——只作可辨识的淡红提示，不抢正文。
const USER_NOTE_BG_FADE: f32 = 0.88;

/// 分享卡高度系数：卡片高 = 视口高 × 此值（再给控制面板让位收一档）。
/// 「高度」滑杆已按用户要求连同代码删除，取原五档的中高档一档定值。
pub(crate) const SHARE_CARD_H_FRAC: f32 = 0.84;

/// 一叶的渲染几何与模式：默认取自当前阅读视图（`LeafGeo::current`）；
/// **晒本长图**按卡片可用面积另算一套传进来（`render_ancient` 的
/// 全部几何读这一份，不再各自读 `self`——两处渲染才能同帧共存）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct LeafGeo {
    /// 字格宽（列宽）。
    pub cell_w: f32,
    /// 字格高（行高）= cell_w × `GRID_ROW_RATIO`。
    pub cell_h: f32,
    /// 每列行数。
    pub rows: usize,
    /// 正文字号 = cell_w × `GRID_FS_RATIO`（收敛在[min,max]）。
    pub fs: f32,
    /// 版框样式：单线 / 文武（外粗内细），由当前 `LayoutConfig` 决定。
    pub border_style: BorderStyle,
    /// 信笺模板：叶内第 7/8 列之间插一列宽中缝（对鱼尾）。
    pub show_fish_tail: bool,
    /// 夹注行内显示（双行小字夹注）；false = 页脚注横排列表。
    pub notes_inline: bool,
    /// 字格 / 注块是否挂鼠标交互（晒本预览关掉：纯展示，不弹工具条）。
    pub interactive: bool,
}

impl LeafGeo {
    /// 当前阅读视图的几何（与 `fit_sizes` / `fit_letter` 算出的同源）。
    /// 边框样式与鱼尾开关直接从当前 `LayoutConfig` 取，不再读 `letter_mode` 布尔。
    pub(crate) fn current(r: &Root) -> Self {
        let cfg = r.current_layout();
        Self {
            cell_w: r.col_w,
            cell_h: r.row_h,
            rows: r.grid_rows,
            fs: r.size_ancient,
            border_style: cfg.border_style,
            show_fish_tail: cfg.show_fish_tail,
            notes_inline: r.notes_inline,
            interactive: true,
        }
    }
}

/// 长按阈值：按住正文超过这么久再抬起，算「长按」——弹气泡菜单
/// （复制 / 检索 / 字典），而不是普通的点字 / 拖选。
const LONG_PRESS: std::time::Duration = std::time::Duration::from_millis(450);

/// 圈号（1-20）。
const CIRCLED: [&str; 20] = [
    "①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩", "⑪", "⑫", "⑬", "⑭", "⑮", "⑯", "⑰",
    "⑱", "⑲", "⑳",
];

fn circled(n: usize) -> String {
    if n >= 1 && n <= CIRCLED.len() {
        CIRCLED[n - 1].to_string()
    } else {
        format!("({n})")
    }
}

/// 0–99 的整数转中文数字（用于分享卡日期的月 / 日）。
fn cn_small(n: usize) -> String {
    const D: [&str; 10] = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    match n {
        0..=9 => D[n].to_string(),
        10 => "十".to_string(),
        11..=19 => format!("十{}", D[n - 10]),
        20..=99 => {
            let tens = D[n / 10];
            if n % 10 == 0 {
                format!("{tens}十")
            } else {
                format!("{tens}十{}", D[n % 10])
            }
        }
        _ => n.to_string(),
    }
}

/// 把「2026-10-18」转成「二〇二六年十月十八日」；解析失败返回空串。
fn cn_date_from_str(ymd: &str) -> String {
    let p: Vec<&str> = ymd.split('-').collect();
    if p.len() != 3 {
        return String::new();
    }
    let (Ok(m), Ok(d)) = (p[1].parse::<usize>(), p[2].parse::<usize>()) else {
        return String::new();
    };
    if p[0].is_empty() || !p[0].chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    let ys: String = p[0]
        .chars()
        .map(|c| match c {
            '0' => "〇",
            '1' => "一",
            '2' => "二",
            '3' => "三",
            '4' => "四",
            '5' => "五",
            '6' => "六",
            '7' => "七",
            '8' => "八",
            '9' => "九",
            _ => "",
        })
        .collect();
    format!("{ys}年{}月{}日", cn_small(m), cn_small(d))
}

/// 取系统当前日期（YYYY-MM-DD），失败返回空串。
fn today_ymd() -> String {
    std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// 按字符数截断：超长缀省略号（目录章名等竖排短文本用）。
fn clip_chars(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let mut out: String = chars.iter().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// 两个字在字形流里是否同一位置：章节 / 段 / 偏移 / 字 / 种类全同。
/// 段首缩进占位每段两个、字段全同，所以必须连 `kind` 一起比对——
/// 晒本长图靠它在流里定当前叶的切片起点（见 `Root::sunben_sheet`）。
fn same_glyph(a: &Glyph, b: &Glyph) -> bool {
    a.chapter == b.chapter
        && a.para == b.para
        && a.offset == b.offset
        && a.ch == b.ch
        && a.kind == b.kind
}

/// 把草稿按每行字符数折行（GPUI 文本不自动换行，显示侧手动折）。
/// 返回非空 Vec；空串返回一个空串行。书架的重命名框也复用它。
pub(crate) fn wrap_lines(s: &str, per_line: usize) -> Vec<String> {
    let per = per_line.max(1);
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(per)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

fn small_button(label: &str, on: bool, t: Theme) -> Div {
    let (bg, fg) = if on {
        (t.chip_on(), t.accent())
    } else {
        (t.chip(), t.chrome_text())
    };
    div()
        .px_2p5()
        .py_1()
        .rounded(px(7.0))
        .bg(rgb(bg))
        .text_size(px(11.5))
        .text_color(rgb(fg))
        .child(SharedString::from(label.to_string()))
}

/// 注释全局编号：(章, 段, 偏移) -> 序号。
type NoteIndex = HashMap<(usize, usize, usize), usize>;

/// 鱼尾纹：一枚分叉尾（top=true 朝下、false 朝上），贴竹简条两端。
///
/// 形状由 `app::ensure_assets()` 写到书架主目录的 SVG 提供，经 `svg()` + AssetSource 加载；
/// SVG 内的 fill 仅决定 alpha（GPUI 用元素 text_color 重新上色），故这里用近黑着色。
fn fish_tail(width: f32, top: bool) -> Div {
    fish_tail_ink(width, top, 0x1A150F)
}

/// 鱼尾纹（指定墨色）：版框里用近墨色，目录里用默认近黑。
fn fish_tail_ink(width: f32, top: bool, color: u32) -> Div {
    let h = width / 1.846; // 与 viewBox 24×13 的宽高比一致，避免拉伸。
    let path = if top {
        "fishtail_top.svg"
    } else {
        "fishtail_bottom.svg"
    };
    div()
        .w(px(width))
        .h(px(h))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(SharedString::from(path.to_string()))
                .w(px(width))
                .h(px(h))
                .text_color(rgb(color)),
        )
}

/// 版心「对鱼尾」（黑鱼尾）：**直线几何多边形**——顶边一刀平直、两侧
/// 竖直下垂成尾翼、中央劈出 V 形凹口，上下两枚**开口相对**。
///
/// 早先是两枚 `<rect>` 实心黑块（33×12），既不合刻本形制，视觉上也极
/// 重；中间一度改用贝塞尔画「燕尾」，画成了圆滚滚的黑桃 / 蝙蝠翅膀。
/// 现在整枚只有 `M` / `L` / `Z`（`banxin::fishtail_svg`），尺寸由
/// `banxin::banxin_tail_size` 按「宽不越界栏、高约 1.2 倍行高」算好
/// 传进来。墨色渲染层给 `#333333` 深灰——黑鱼尾不取纯黑。见
/// app::ensure_assets。
fn banxin_tail(w: f32, h: f32, top: bool, color: u32) -> Div {
    let path = if top {
        "banxin_top.svg"
    } else {
        "banxin_bottom.svg"
    };
    div()
        .w(px(w))
        .h(px(h))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(SharedString::from(path.to_string()))
                .w(px(w))
                .h(px(h))
                .text_color(rgb(color)),
        )
}

/// 信笺「对鱼尾」：与版心同一条直线多边形路径，比例取**横展**
/// （`banxin::LETTER_TAIL_ASPECT`，照用户「信笺」参考图的 47×34 量级）。
/// 见 app::ensure_assets。
fn letter_fishtail(w: f32, h: f32, top: bool, color: u32) -> Div {
    let path = if top {
        "yu_top.svg"
    } else {
        "yu_bottom.svg"
    };
    div()
        .w(px(w))
        .h(px(h))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(SharedString::from(path.to_string()))
                .w(px(w))
                .h(px(h))
                .text_color(rgb(color)),
        )
}

/// 版心折痕：纸**对折**的那道物理褶——0.5px、`#999999`、**虚线**，
/// 比界行还淡一档，只作「纸的痕迹」，不抢鱼尾与书口文字。
///
/// GPUI 0.2.2 没有元素级 dashed 边框，故用一串短横拼出来：段长固定，
/// 段距由 `banxin::fold_dashes` 按可用高反算，头尾严丝合缝不留残段。
fn fold_crease(x: f32, top: f32, len: f32, color: u32) -> Div {
    let (n, gap) = crate::banxin::fold_dashes(len, crate::banxin::FOLD_DASH);
    let mut line = div()
        .absolute()
        .top(px(top))
        .left(px(x))
        .w(px(crate::banxin::FOLD_W))
        .flex()
        .flex_col()
        .gap(px(gap));
    for _ in 0..n {
        line = line.child(
            div()
                .w(px(crate::banxin::FOLD_W))
                .h(px(crate::banxin::FOLD_DASH))
                .bg(rgb(color)),
        );
    }
    line
}

/// 版心书口的一行文字：一字一格、居中竖排，**极小号宋体**（族名由调用
/// 方给：走正文字体的回退链）。淡墨用于书名 / 卷次 / 叶码，正墨给篇题。
fn banxin_vtext(line: &crate::banxin::BanxinLine, w: f32, family: &SharedString, t: Theme) -> Div {
    let mut d = div().w(px(w)).flex().flex_col().items_center();
    for c in line.text.chars() {
        d = d.child(
            div()
                .w(px(w))
                .h(px(line.lh))
                .flex()
                .items_center()
                .justify_center()
                .font_family(family.clone())
                .text_size(px(line.fs))
                .text_color(rgb(if line.soft { t.ink_soft() } else { t.ink() }))
                .child(SharedString::from(c.to_string())),
        );
    }
    d
}

/// 版心小印：朱砂红「箋讀」小方印，嵌在中缝的下鱼尾之下（影印本版心
/// 下端常见的小印记）。篆字是 `app::ensure_assets()` 写出的两枚 SVG
/// （`seal_jian.svg` / `seal_du.svg`，等粗圆头线稿），这里只用
/// `text_color` 着朱砂色，印面自留白（朱文）。
fn banxin_seal(w: f32, h: f32) -> Div {
    // 每枚篆字约占印面一半高：两字竖排相叠，正好是一方竖长小印。
    let gw = (w * 0.74).max(9.0);
    let gh = (h - 2.0) / 2.0;
    div()
        .w(px(w))
        .h(px(h))
        .border_1()
        .border_color(rgb(CINNABAR))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(1.0))
        .child(
            svg()
                .path(SharedString::from("seal_jian.svg".to_string()))
                .w(px(gw))
                .h(px(gh))
                .text_color(rgb(CINNABAR)),
        )
        .child(
            svg()
                .path(SharedString::from("seal_du.svg".to_string()))
                .w(px(gw))
                .h(px(gh))
                .text_color(rgb(CINNABAR)),
        )
}

/// 书耳：刻本贴在书脑侧版框上角的一小片「耳朵」（题签小耳，线装 / 包背装
/// 常见）。样式参照用户给的「天 / 地 / 鱼尾 / 书脑 / 耳子」图：一片竖长
/// 小牌，外侧带线、内侧贴住版框，紧贴左上角不放空。
///
/// `mirror` = true 时贴右上（右半叶的外侧在其右），否则贴左上。
/// 耳内竖排题签（篇章名 / 卷名 / 提要，见 `types::ear_label`），
/// 单列自适应收缩——耳片极窄，读的是个「形」与位置。
fn book_ear(t: Theme, w: f32, h: f32, x: f32, top: f32, mirror: bool, label: &str) -> Div {
    // 耳内小字：高出一成给行距，宽不超出耳片。
    let fs = (w * 0.62).max(6.5).min(10.0);
    // 耳片只有一列宽：单列自适应（字号随耳高收缩，装不下才截断缀「…」）。
    let (efs, ecol, _cut) = crate::shelf::fit_single_column(label, h - 6.0, fs);
    let mut e = div()
        .absolute()
        .top(px(top))
        .w(px(w))
        .h(px(h))
        .bg(rgb(t.paper()))
        .border_t_1()
        .border_b_1()
        .border_color(rgb(t.frame_ink()))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden();
    e = if mirror {
        e.left(px(x))
            .border_r_1()
            .border_color(rgb(t.frame_ink()))
    } else {
        e.left(px(x))
            .border_l_1()
            .border_color(rgb(t.frame_ink()))
    };
    e.child(crate::shelf::vertical_cols(
        &[ecol],
        efs,
        t.ink_soft(),
        FontWeight::NORMAL,
    ))
}

/// 一叶在「一张纸」里的位置：对开时两半内侧要收掉留白，才能接成一整张。
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeafEdge {
    /// 独占一纸（单面阅读，或对开的最后一叶）。
    Alone,
    /// 左半叶：内侧是右边，右留白收掉。
    Left,
    /// 右半叶：内侧是左边，左留白收掉。
    Right,
}

/// 章末统计（篇 / 章 / 字），排在该章末葉正文之后的第一片空列里。
#[derive(Clone, Default)]
struct ChapStat {
    /// 篇名（章题）。
    title: String,
    /// 本章是全书第几章（表内「章」列，写作一二三…）。
    index: usize,
    /// 本章字数（去标点、去空白）。
    chars: usize,
}

/// 正文之后的第一片空列（读序：列号自 0 递增、向右向左）。
/// 整页排满（无空列）时返回 None —— 章末统计表便无处可落，宁可不画也不压字。
fn first_empty_column(columns: &[Vec<Piece>], text_cols: usize) -> Option<usize> {
    let mut last_used: Option<usize> = None;
    for i in 0..text_cols {
        if columns.get(i).map(|c| !c.is_empty()).unwrap_or(false) {
            last_used = Some(i);
        }
    }
    let e = last_used.map(|i| i + 1).unwrap_or(0);
    if e < text_cols {
        Some(e)
    } else {
        None
    }
}

/// 这一叶上「收尾」的章号：本章正文到此为止（下一叶是新章首，或已是全书末叶）。
/// 通常只有一章；短章恰好同叶收尾时，统计表就一行一篇并排列出——正是影印本
/// 卷尾那条「篇 / 章 / 字」小表的样子。
fn chapters_ended_at(pages: &[Page], idx: usize) -> Vec<usize> {
    let Some(cur) = pages.get(idx) else {
        return Vec::new();
    };
    // 下一叶仍出现的章：正文还没收完，统计表不赶它。
    let carries_on: std::collections::HashSet<usize> = pages
        .get(idx + 1)
        .map(|p| p.glyphs.iter().map(|g| g.chapter).collect())
        .unwrap_or_default();
    let mut seen: Vec<usize> = Vec::new();
    for g in &cur.glyphs {
        if !seen.contains(&g.chapter) {
            seen.push(g.chapter);
        }
    }
    let last_leaf = idx + 1 >= pages.len();
    seen.retain(|&c| last_leaf || !carries_on.contains(&c));
    seen
}

/// 葉码用汉字数字：刊本叶码写作「一、二 … 十、十一 … 二十」，不用阿拉伯数字。
fn han_num(n: usize) -> String {
    const D: [&str; 10] = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    match n {
        0 => "〇".to_string(),
        1..=9 => D[n].to_string(),
        10 => "十".to_string(),
        11..=19 => format!("十{}", D[n % 10]),
        20..=99 => {
            let head = format!("{}十", D[n / 10]);
            if n % 10 == 0 {
                head
            } else {
                format!("{}{}", head, D[n % 10])
            }
        }
        _ => n.to_string(),
    }
}

fn build_note_index(book: &Book) -> NoteIndex {
    let mut map = HashMap::new();
    let mut n = 0usize;
    for (ci, ch) in book.chapters.iter().enumerate() {
        for (pi, p) in ch.paragraphs.iter().enumerate() {
            for note in &p.notes {
                if note.kind == NoteKind::Note {
                    n += 1;
                    map.insert((ci, pi, note.at), n);
                }
            }
        }
    }
    map
}

impl Root {
    // ------------------------------------------------------------------
    // 阅读视图
    // ------------------------------------------------------------------
    /// 眉心（版心）水平点击带：`Some((左缘, 右缘))`（视口坐标）。
    /// 几何与渲染同源（`render_gutter` / `render_letter_gutter`），
    /// 纯函数在 `types::meixin_band`（含单测）。对开末页没有左半叶、
    /// 中缝不渲染，返回 `None`——那一屏点中间照旧翻页。
    fn meixin_band(&self, total: usize) -> Option<(f32, f32)> {
        crate::types::meixin_band(
            self.viewport_w,
            self.is_letter_mode(),
            self.duplex == Duplex::Facing,
            // 中缝渲染条件：对开且下一面（左半叶）存在。
            self.page + 1 < total,
            crate::grid::gutter_w(
                self.content_w,
                self.grid.columns,
                ANCIENT_EDGE,
                self.grid.margin,
            ),
            self.col_w,
            self.grid.columns,
        )
    }

    pub fn render_reading(&mut self, cx: &mut Context<Self>) -> Div {
        if self.book.is_none() {
            return self.render_shelf(cx);
        }
        // 按当前窗口视口把字格缩放到铺满屏。
        self.fit_sizes();
        let t = self.theme;
        // 「面 → 列 → 片」一次排好：分页与切列同源（gloss::layout_book），
        // 双行夹注的跨列流转也在这里定型，渲染侧不再重算。
        let pages = self.layout_pages_timed();
        let total = pages.len();
        if self.page >= total {
            self.page = total.saturating_sub(1);
        }
        // 每帧重算可见面的注文包围盒（Canvas 命中测试 / 卡片定位用）。
        self.refresh_note_boxes(&pages);

        // 双面对开 / 单页由**屏幕方向**驱动（见 fit_sizes）：
        //   · 横屏（宽 > 高，平板 / iPad 横持）→ 双面对开：两叶竖排并排；
        //   · 竖屏（高 ≥ 宽，手机竖持）      → 单页：一叶居中。
        // 两叶均为古籍竖排，读序自右向左。
        let stage = if self.duplex == Duplex::Facing {
            let cur = self.page; // 当前面（右半叶）
            let next = self.page + 1; // 下一面（左半叶）
            // 古籍对开自右向左读：当前面（cur）在右、下一面（next）在左，读序 N → N+1。
            // flex_row 首个子元素落在最左，故先排下一面（左）、后排当前面（右）。
            //
            // 对开＝**一张纸的两半**：不留缝、纸色统一（不再让左叶显纸背色）、
            // 内侧也不留白，两半在纸心直接相接；纸心再立一道「中缝」（版心）。
            // 这样摊开的就是一整张印张，而不是两片孤立的书叶。
            let mut row = div().flex().flex_row().items_start();
            let left_is_head = pages.get(next).map(|p| p.start_chapter).unwrap_or(false);
            let cur_is_head = pages.get(cur).map(|p| p.start_chapter).unwrap_or(false);
            if next < total {
                row = row.child(self.render_leaf(&pages, next, false, LeafEdge::Left, cx));
                let ch = pages
                    .get(cur)
                    .and_then(|p| p.glyphs.first())
                    .map(|g| g.chapter)
                    .unwrap_or(0);
                // 中缝常在：它把两半的版框隔开，纸心才像一道折缝而不是一道黑杠。
                // 篇题与叶码只出现在一处：某一半叶是章首时，那一叶的「章题列」
                // 已当了版心，中缝便只留一对鱼尾；否则中缝载篇题与叶码。
                let detailed = !left_is_head && !cur_is_head;
                row = row.child(self.render_gutter(ch, cur, detailed));
            }
            if cur < total {
                let edge = if next < total { LeafEdge::Right } else { LeafEdge::Alone };
                row = row.child(self.render_leaf(&pages, cur, false, edge, cx));
            }
            row
        } else {
            // 窄屏（手机竖持）：纸面**贴屏幕右边缘**——竖排自右向左读，
            // 右缘正是当前叶所在，整幅宽都归这一叶（字号最大化）；
            // 宽屏单叶仍居中（像摊在桌上的一叶）。
            let narrow = crate::grid::is_narrow(self.viewport_w, self.viewport_h);
            div()
                .flex()
                .flex_row()
                .when(narrow, |d| d.justify_end())
                .when(!narrow, |d| d.justify_center())
                .child(self.render_leaf(&pages, self.page, false, LeafEdge::Alone, cx))
        };

        let toolbar = self.render_reading_toolbar(total, cx);

        // 正文区：单页 / 对开舞台居中铺满。样式面板改为浮层（见下方 root.overlay），
        // 允许盖住正文下部，因此这里不再为它预留高度。
        //
        // 翻页：点正文区右半屏翻下一页、左半屏翻上一页（替代原翻页动画）。
        // 关键：翻页判定放在「抬起」而非「按下」——
        //   ① capture 阶段记录按下点（先于字格处理器，落点无遗漏）；
        //   ② 抬起时若本次交互碰过正文文字（拖选 / 点字），一律不翻页，
        //      并补弹工具条（拖选松手常落在纸面，字格不会弹，这里兜底）；
        //   ③ 只有「按下-抬起位移很小且全程在纸面」才算单击纸面，才翻页。
        // 这样拖选划线绝不会误触发翻页；字格上单击由字格自己的处理器
        // stop_propagation 拦住，照旧只弹工具条。
        //
        // 专注模式：四边留白也去掉（p_0），纸面撑满整屏；
        // 普通模式保留 p_4 留白，纸面如书中一叶。
        let mut body = div().flex_1().flex().flex_col().items_center().justify_center();
        if self.focus_mode {
            body = body.p_0();
        } else {
            body = body.p_4();
        }
        let body = body
            .capture_any_mouse_down(cx.listener(
                move |this, event: &MouseDownEvent, _window, _cx| {
                    this.press_at = Some((
                        event.position.x.to_f64() as f32,
                        event.position.y.to_f64() as f32,
                    ));
                    // 记下按下时刻：抬起时据此判定「长按」（弹气泡菜单）。
                    this.press_time = Some(std::time::Instant::now());
                    this.touched_glyph = false;
                },
            ))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseUpEvent, _window, cx| {
                        // 笔记浮层开着：在正文区松手＝保存笔记并收起浮层，不翻页。
                        // 但「刚打开浮层那一下」要跳过：它的 mouseup 会重新命中
                        // 正文，若在此提交空草稿，浮层会闪开即关、根本写不了。
                        if this.note_editing {
                            if this.note_suppress_commit {
                                this.note_suppress_commit = false;
                            } else {
                                this.commit_note_edit();
                            }
                            this.press_at = None;
                            cx.notify();
                            return;
                        }
                        let (ux, uy) = (
                            event.position.x.to_f64() as f32,
                            event.position.y.to_f64() as f32,
                        );
                        let pressed = this.press_at.take();
                        // 长按：按住正文超过阈值 → 弹气泡菜单（复制 / 检索 / 字典）。
                        // 判在「抬起」而非「按下」，避免和拖选取字打架：
                        // 只有确实按住了一段时间、且落在一个正文字符上才算长按。
                        let held = this.press_time.take().map(|t| t.elapsed()).unwrap_or_default();
                        // 长按**用户自加夹注** → 弹「修改 / 删除」菜单（≡ 右键）。
                        // 判在「抬起」且先于正文长按：注块按下时也会置
                        // `touched_glyph`，若让正文长按先命中就永远弹不到菜单。
                        // 古籍原本的夹注不弹——只有 `NoteSource::User` 可改删。
                        if held >= LONG_PRESS {
                            if let Some(b) = this.note_box_at(ux, uy) {
                                if matches!(b.source, NoteSource::User) {
                                    let (key, text) = (b.key, b.text.clone());
                                    this.note_menu = Some(crate::gloss::NoteMenu {
                                        key,
                                        text,
                                        x: ux,
                                        y: uy,
                                    });
                                    this.note_zoom = None;
                                    this.dragging = false;
                                    this.popup = None;
                                    cx.notify();
                                    return;
                                }
                            }
                        }
                        if held >= LONG_PRESS {
                            // 长按查词：**坐标反查**为主，元素命中为辅。
                            //
                            // 元素命中（字格自带 `on_mouse_down`）落在格子正中时最准，
                            // 但点在格子缝隙、夹注旁边、或拖选刚结束那一下，元素不一定
                            // 命中——那就长按没反应了。故补一条纯几何的路：只用
                            // (x, y) 反推落在哪一叶哪一列哪一行（`glyph_hit_at`），
                            // 直接得到 charIndex `(chapter, para, offset)` 与那个
                            // 字在屏幕上的矩形（浮签避让要用）。
                            let mut got: Option<(GlyphKey, char, String, Option<(f32, f32, f32, f32)>)> =
                                None;
                            if this.touched_glyph && this.sel_char.is_some() {
                                let key = this.sel_focus.or(this.sel_anchor).unwrap_or((0, 0, 0));
                                let ch = this.sel_char.unwrap_or(' ');
                                let text = if this.has_selection() {
                                    this.selected_text()
                                } else {
                                    ch.to_string()
                                };
                                got = Some((key, ch, text, None));
                            }
                            if got.is_none() {
                                if let Some(gb) = this.glyph_hit_at(ux, uy) {
                                    let key = gb.key();
                                    let ch = gb.glyph.ch;
                                    // 词组：从命中字起取 4 字交给词典做最长匹配
                                    // （「长安」成一条词，而不是「长」+「安」）。
                                    let mut text = this.text_from(key, 4);
                                    if text.is_empty() {
                                        text = ch.to_string();
                                    }
                                    got = Some((key, ch, text, Some((gb.x, gb.y, gb.w, gb.h))));
                                }
                            }
                            if let Some((key, ch, mut text, rect)) = got {
                                // 有选区就以选区为准：拖选后长按，查的是选的那段。
                                if this.has_selection() {
                                    text = this.selected_text();
                                }
                                this.bubble = Some(GlyphHit {
                                    key,
                                    ch,
                                    text: text.clone(),
                                    x: ux,
                                    y: uy,
                                });
                                // 长按直接呼出「字义浮签」：本地字书释义 + charIndex
                                // + 选中文本 + 在线查阅，一步到位（复制 / 检索并入
                                // 浮签底部）。`rect` 是那个字的屏幕矩形——浮签据此
                                // 避让，绝不把它压在底下。
                                this.open_lookup(key, ch, text, Some((ux, uy)), rect);
                                this.dragging = false;
                                this.popup = None;
                                cx.notify();
                                return;
                            }
                        }
                        // 是否翻页：碰过正文 / 拖动中 / 位移过大，一律不翻。
                        let fwd = crate::types::tap_turns_page(
                            this.touched_glyph,
                            this.dragging,
                            pressed,
                            (ux, uy),
                            this.viewport_w * 0.5,
                        );
                        if this.touched_glyph || this.dragging {
                            // 本次交互碰过正文：拖选 / 点字 / 点注文，绝不翻页。
                            this.dragging = false;
                            // 拖选松手落在纸面时字格不会弹工具条，这里兜底补弹，
                            // 保证「选中后一定能划线 / 复制 / 分享」。
                            if this.popup.is_none() && this.sel_anchor.is_some() {
                                let key = this.sel_focus.or(this.sel_anchor).unwrap_or((0, 0, 0));
                                let ch = this.sel_char.unwrap_or(' ');
                                let text = if this.has_selection() {
                                    this.selected_text()
                                } else {
                                    ch.to_string()
                                };
                                this.popup = Some(GlyphHit {
                                    key,
                                    ch,
                                    text,
                                    x: ux,
                                    y: uy,
                                });
                            }
                            cx.notify();
                        } else if let Some(hit) = this.note_box_at(ux, uy) {
                            // Canvas 级 fallback：注块元素事件没拦住（极端情况
                            // 如命中缝隙）时，用包围盒命中，同样弹放大卡片。
                            let text = hit.text.clone();
                            let page = hit.page;
                            this.note_zoom = Some(crate::gloss::NoteZoom {
                                text,
                                x: ux,
                                y: uy,
                                page,
                                pinyin: None,
                            });
                            cx.notify();
                        } else if let Some(fwd) = fwd {
                            // 全程在纸面且是单击：眉心（版心）那道一字宽的
                            // 纸带——鱼尾 / 篇题 / 叶码都立于此——点它才呼出
                            // 目录；带外照旧按落点左右半屏翻页。单页古籍
                            // 没有中缝（没有眉心），点中间仍翻页；对开末页
                            // 没有左半叶、中缝不渲染，同样没有眉心。
                            let in_meixin = this
                                .meixin_band(total)
                                .map(|(lo, hi)| ux >= lo && ux <= hi)
                                .unwrap_or(false);
                            if in_meixin {
                                this.dragging = false;
                                this.open_toc();
                                cx.notify();
                            } else {
                                this.step(fwd, cx);
                            }
                        }
                    }),
                )
            .child(stage);

        // 根容器设为相对定位，便于叠加浮层（目录 / 样式 / 字书 / 笔记）。
        // 全局背景：**纸色**（配置表 `paperColor`）铺满整窗。
        // 工具栏 / 状态栏各自再压一层界面色（`chrome`），纸面本体直接透出纸色。
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(t.paper()))
            // 点字弹出的工具条：点正文某字才出现；点其它地方（正文空白处 / 工具栏 / 浮层）即收起，
            // 同时清除拖选选区。
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                // 笔记浮层开着时，点浮层以外的任何地方＝自动保存笔记；
                // 同时「吃掉」本次按下记录，避免松手时误翻页。
                // 打开浮层那一下的按下要放过（见 note_suppress_commit）。
                if this.note_editing {
                    if this.note_suppress_commit {
                        this.note_suppress_commit = false;
                    } else {
                        this.commit_note_edit();
                    }
                    this.press_at = None;
                }
                if this.popup.is_some() || this.has_selection() {
                    this.popup = None;
                    this.clear_selection();
                }
                // 点气泡菜单以外的任何地方：收起气泡菜单。
                this.bubble = None;
                // 点字义浮签以外的任何地方：收起浮签。
                // 只收 `lookup_at` 有值的（贴着长按点浮起的那张）；右上角常驻
                // 字书面板是点「查询」主动开出来的，须点「关闭」才收。
                if this.lookup_at.is_some() {
                    this.close_lookup();
                }
                // 点注文卡片之外：收起放大卡（卡片自身 stop_propagation，
                // 点卡片内容不会到达这里）。
                this.note_zoom = None;
                // 点夹注菜单之外：收起「修改 / 删除」菜单（同理）。
                this.note_menu = None;
                cx.notify();
            }),
        );
        // 键盘总入口不挂在这里：GPUI 只把按键派发给「焦点节点 → 根元素」这条
        // 路径上的监听器，而本函数是根元素的子层（见 app.rs 的 Render::render）。

        if !self.focus_mode {
            root = root
                .child(toolbar)
                .child(body)
                .child(self.render_reading_status(total));
        } else {
            root = root.child(body);
            // 左上角常驻「退出专注」（避开右上角的字书浮层）。
            root = root.child(
                div()
                    .absolute()
                    .top(px(12.0))
                    .left(px(12.0))
                    .child(
                        small_button("退出专注", false, t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.focus_mode = false;
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }

        // 点字查字浮层（右上角常驻面板）。
        // `lookup_at` 有值说明是长按呼出的「字义浮签」（贴着长按点浮起，
        // 由 `render_lookup_card` 画），这里不能再叠一张，否则一屏两张字书。
        if self.lookup.is_some() && self.lookup_at.is_none() {
            root = root.child(self.render_dict_panel(cx));
        }
        // 目录浮层：竹简条（鱼尾纹）。
        if self.show_toc && !self.focus_mode {
            root = root.child(self.render_toc_sheet(cx));
        }
        // 样式面板浮层：点工具栏「样式」展开，宽度与单张页面一致，
        // 可盖住正文下部（像照片说明压在照片上），但全部样式内容完整显示。
        if self.show_style && !self.focus_mode {
            root = root.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(42.0))
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(self.render_style_sheet(cx)),
            );
        }
        // 版式控制面板：列数 8/10/12/14 + 四周留白（浮层，点工具栏「版式」展开）。
        if self.show_grid_panel && !self.focus_mode {
            root = root.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(42.0))
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(self.render_grid_panel(cx)),
            );
        }
        // 笔记 / 批注浮层。
        if self.show_notes_panel && !self.focus_mode {
            root = root.child(self.render_notes_panel(cx));
        }
        // 分享卡片浮层（深底竖排，可存为图片）。
        if self.show_share_panel {
            root = root.child(self.render_share_sheet(cx));
        }
        // 全文检索浮层：输入词 → 全书查找 → 跳面 + 朱砂红高亮。
        // 分享面板开着时不画——那条「搜索 / 上一处 / 下一处」输入框挂在
        // 屏幕底部，会把导出界面的沉浸感整个破坏掉（分享关掉后自动回来）。
        if self.show_search && !self.focus_mode && !self.show_share_panel {
            root = root.child(self.render_search_sheet(cx));
        }
        // 帮助面板：快捷键与操作说明。
        if self.show_help {
            root = root.child(self.render_help_sheet(cx));
        }
        // 左上角书名签条：挂在所有浮层之前，浮层打开时会盖住它（不抢戏）；
        // 专注模式沉浸阅读，一并收起。
        if !self.focus_mode {
            root = root.child(self.render_spine_label(cx));
        }

        // 点字弹出的工具条：仅在点击正文某字时出现，点别处自动收起。
        if self.popup.is_some() {
            root = root.child(self.render_glyph_menu(cx));
        }

        // 长按正文弹出的「字义浮签」（释义 / charIndex / 选中文本 / 在线查阅）。
        if self.bubble.is_some() {
            root = root.child(self.render_lookup_card(cx));
        }

        // 注文放大悬浮卡：点行内夹注小字弹出，点卡片外任意处收起。
        if self.note_zoom.is_some() {
            root = root.child(self.render_note_zoom(cx));
        }

        // 用户自加夹注的「修改 / 删除」菜单：右键 / 长按弹出，点别处收起。
        if self.note_menu.is_some() {
            root = root.child(self.render_note_menu(cx));
        }

        // 浮层笔记框：划线 / 点字后直接盖在文本上写笔记——
        // 不另开面板、不推开正文，Esc 取消、⏎ 保存、点别处自动保存。
        if self.note_editing {
            root = root.child(self.render_note_editor(cx));
        }

        root
    }


    /// 键盘总入口：由窗口根元素上的监听器调用（见 `app.rs` 的 `Render::render`）。
    ///
    /// GPUI 0.2.2 只把按键派发给「焦点节点 → 根元素」这条路径上的监听器；
    /// `render_screen` 返回的是根元素的子层，且没有任何可聚焦元素，
    /// 挂在它上面的 `on_key_down` 永远收不到键——所以入口必须放在根元素。
    pub fn dispatch_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match self.screen {
            Screen::Reading => self.handle_reading_key(event, cx),
            _ => self.handle_shelf_key(event, cx),
        }
    }

    /// 阅读视图的按键处理（笔记浮层 → Esc → 方向键翻页 → ⌘ 组合快捷键）。
    ///
    /// 注意：GPUI 0.2.2（macOS）的 keystroke.key 是全小写命名——
    /// 方向键为 "left"/"right"/"up"/"down"，Esc 为 "escape"；
    /// 这里同时兼容旧写法，避免键名拼写差异让整组快捷键静默失效。
    pub fn handle_reading_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
                    let k = event.keystroke.key.as_str();
                    let mods = event.keystroke.modifiers;

                    // ① 笔记编辑态：键盘全部路由给浮层笔记框。
                    //    Esc 取消 / ⏎ 保存 / 退格删字 / ← → 移光标 /
                    //    ⌘V 粘贴中文。
                    //    可打印字符**不在此插入**——无论 ABC 还是中文输入法，
                    //    inputContext 都会回调 insertText / setMarkedText
                    //    （实测 ABC 下每个键都触发 insertText），keydown
                    //    直插必与回落叠字。文字只走 InputHandler 一条路，
                    //    与 NSTextField 的架构一致（见 ime.rs 架构说明）。
                    if self.note_editing {
                        match k {
                            "escape" | "Escape" | "esc" => self.cancel_note_edit(),
                            "enter" | "Enter" => self.commit_note_edit(),
                            // 组字期间退格由输入法自己消化（删的是未上屏的
                            // 拼音串），应用再删就是双重删除。
                            "backspace" | "Backspace" => {
                                if self.note_marked.is_none() {
                                    self.note_backspace();
                                }
                            }
                            "left" | "Left" | "ArrowLeft" => {
                                crate::app::draft_move(
                                    &self.note_draft,
                                    &mut self.note_cursor,
                                    true,
                                );
                            }
                            "right" | "Right" | "ArrowRight" => {
                                crate::app::draft_move(
                                    &self.note_draft,
                                    &mut self.note_cursor,
                                    false,
                                );
                            }
                            "home" | "Home" => self.note_cursor = 0,
                            "end" | "End" => {
                                self.note_cursor = self.note_draft.chars().count()
                            }
                            "v" | "V" if mods.platform || mods.control => self.note_paste(),
                            // 可打印键：交给输入法（insertText / setMarkedText
                            // 经 InputHandler 回落），此处直插只会叠字。
                            _ => {}
                        }
                        cx.notify();
                        return;
                    }

                    // ①′ 检索框：面板开着时键盘先给它。
                    //      ⏎ 检索 / Esc 收起 / 退格 / ← →。
                    //      可打印字符不在此插入——中文与英文都经
                    //      InputHandler 回落（insertText / setMarkedText），
                    //      keydown 直插会与回落叠字（见 ime.rs 架构说明）。
                    if self.show_search {
                        match k {
                            "enter" | "Enter" => self.run_search(cx),
                            "escape" | "Escape" | "esc" => {
                                self.show_search = false;
                                self.clear_search();
                            }
                            "backspace" | "Backspace" => {
                                if self.search_marked.is_none() {
                                    self.search_backspace();
                                }
                            }
                            "left" | "Left" | "ArrowLeft" => {
                                crate::app::draft_move(
                                    &self.search_draft,
                                    &mut self.search_cursor,
                                    true,
                                );
                            }
                            "right" | "Right" | "ArrowRight" => {
                                crate::app::draft_move(
                                    &self.search_draft,
                                    &mut self.search_cursor,
                                    false,
                                );
                            }
                            // 可打印键交给输入法回落，不直插。
                            _ => {}
                        }
                        cx.notify();
                        return;
                    }

                    // ② Esc：退出专注模式，并收起全部浮层与选区。
                    if k == "escape" || k == "Escape" || k == "esc" {
                        self.focus_mode = false;
                        self.close_toc();
                        self.show_style = false;
                        self.show_notes_panel = false;
                        self.show_help = false;
                        self.show_share_panel = false;
                        self.close_lookup();
                        self.clear_selection();
                        self.popup = None;
                        self.bubble = None;
                        self.note_zoom = None;
                        self.note_menu = None;
                        cx.notify();
                        return;
                    }

                    // ③ 翻页：← / → 左右翻页（上一页 / 下一页），↑ / ↓ 亦可；
                    // 单面 / 双面 / 专注模式下同样生效（判定见 types::arrow_key_turns）。
                    let fwd = crate::types::arrow_key_turns(k);
                    if let Some(fwd) = fwd {
                        self.step(fwd, cx);
                        return;
                    }

                    // ④ ⌘C 复制：有选区复制选区，否则复制整页正文。
                    if (k == "c" || k == "C") && (mods.platform || mods.control) {
                        self.copy_page_text();
                        self.popup = None;
                        self.clear_selection();
                        cx.notify();
                        return;
                    }

                    // ⑤ ⌘ 组合快捷键：⌘+数字 选调色板，⌘+字母 调功能。
                    //    翻页：← / → （及 ⌘+← / ⌘+→）左右翻页。
                    //    复制：⌘C（见上方 ④）。
                    //    为避免与 macOS 系统保留键冲突，刻意避开了 ⌘H（隐藏）、
                    //    ⌘M（最小化）、⌘S（保存）、⌘Q（退出）。
                    if mods.platform && !mods.control && !mods.alt {
                        match k {
                            // 调色板：⌘1–⌘5（朱、墨、纸…… 五套）。
                            "1" => {
                                self.set_palette(0);
                                cx.notify();
                            }
                            "2" => {
                                self.set_palette(1);
                                cx.notify();
                            }
                            "3" => {
                                self.set_palette(2);
                                cx.notify();
                            }
                            "4" => {
                                self.set_palette(3);
                                cx.notify();
                            }
                            "5" => {
                                self.set_palette(4);
                                cx.notify();
                            }
                            // 新增第六套：暗金（⌘6）——金墨正文 + 米白夹注。
                            "6" => {
                                self.set_palette(5);
                                cx.notify();
                            }
                            // 繁简切换：⌘T。
                            "t" | "T" => {
                                self.show_traditional = !self.show_traditional;
                                cx.notify();
                            }
                            // 字体切换：⌘F。
                            "f" | "F" => {
                                self.cycle_font();
                                cx.notify();
                            }
                            // 主题切换：⌘P。
                            "p" | "P" => {
                                self.cycle_theme();
                                cx.notify();
                            }
                            // 笔记（行间注）显隐：⌘N。
                            "n" | "N" => {
                                self.show_notes = !self.show_notes;
                                cx.notify();
                            }
                            // 通假显隐：⌘J。
                            "j" | "J" => {
                                self.show_tongjia = !self.show_tongjia;
                                cx.notify();
                            }
                            // 标点显隐：⌘B。
                            "b" | "B" => {
                                self.show_punct = !self.show_punct;
                                cx.notify();
                            }
                            // 生僻注音显隐：⌘R。
                            "r" | "R" => {
                                self.show_rare = !self.show_rare;
                                cx.notify();
                            }
                            // 样式面板：⌘E（同时收起目录 / 帮助）。
                            "e" | "E" => {
                                self.show_help = false;
                                self.show_style = !self.show_style;
                                cx.notify();
                            }
                            // 目录：⌘L（同时收起样式 / 帮助）。
                            "l" | "L" => {
                                self.show_help = false;
                                if self.show_toc {
                                    self.close_toc();
                                } else {
                                    self.open_toc();
                                }
                                cx.notify();
                            }
                            // 查字：⌘G（查选区 / 最近碰过的字，没有则查本页首字）。
                            "g" | "G" => {
                                let c = self
                                    .sel_char
                                    .or_else(|| self.popup.as_ref().map(|h| h.ch))
                                    .or_else(|| {
                                        self.paginate().get(self.page).and_then(|p| {
                                            p.iter().find(|g| !g.is_indent()).map(|g| g.ch)
                                        })
                                    });
                                if let Some(c) = c {
                                    // 连同 charIndex / 选中文本一起记下：查字面
                                    // 板上也要显示「第几章第几段第几字」。
                                    let key = self
                                        .sel_focus
                                        .or(self.sel_anchor)
                                        .or_else(|| self.popup.as_ref().map(|h| h.key))
                                        .unwrap_or((self.current_chapter, 0, 0));
                                    let text = if self.has_selection() {
                                        self.selected_text()
                                    } else {
                                        c.to_string()
                                    };
                                    self.open_lookup(key, c, text, None, None);
                                }
                                self.clear_selection();
                                self.popup = None;
                                cx.notify();
                            }
                            // 专注模式：⌘Z（同时收起样式 / 目录 / 帮助）。
                            "z" | "Z" => {
                                self.focus_mode = !self.focus_mode;
                                self.show_style = false;
                                self.close_toc();
                                self.show_help = false;
                                cx.notify();
                            }
                            // 笔记面板（侧栏）：⌘K。
                            "k" | "K" => {
                                self.show_notes_panel = !self.show_notes_panel;
                                self.note_editing = false;
                                cx.notify();
                            }
                            // 帮助：⌘/。
                            "/" => {
                                self.show_help = !self.show_help;
                                cx.notify();
                            }
                            // 划线：⌘U。有选区划选区；无选区划本页第一个正文字。
                            "u" | "U" => {
                                if self.has_selection() {
                                    self.underline_selection();
                                } else {
                                    let first = self
                                        .paginate()
                                        .get(self.page)
                                        .and_then(|p| {
                                            p.iter().find(|g| !g.is_indent()).map(|g| {
                                                (g.chapter, g.para, g.offset)
                                            })
                                        });
                                    if let Some(key) = first {
                                        if !self.underlines.remove(&key) {
                                            self.underlines.insert(key);
                                        }
                                        self.save_annotations();
                                    }
                                }
                                self.popup = None;
                                cx.notify();
                            }
                            _ => {}
                        }
                    }
    }

    /// 目录侧栏（从右侧滑入的古风卷目面板）。
    ///
    /// 不画现代列表：章名以「竹简条」竖排、从右向左排列，宋体、月白；
    /// 当前所在章用朱砂红标出（加粗 + 描边）。点击章即按该章首字下标
    /// （`start_char_index`）重新排版并跳到第一叶。点面板外空白处平滑收起
    /// （由 `toc_slide` 驱动滑出动画）。
    fn render_toc_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let Some(book) = self.book.as_ref() else {
            return div();
        };
        // 古风夜色面板配色：深靛底 + 月白字 + 朱砂当前章，三色压出一档层次。
        const PANEL_BG: u32 = 0x1E2A3C;
        const PANEL_BG2: u32 = 0x223146; // 略浅的底，用于表头 / 当前竹简条
        const MOON: u32 = 0xE8EEF2; // 月白：章名字色
        const MOON_DIM: u32 = 0x9FB0C4; // 月白兑暗：小字 / 叶码 / 提示
        const VERMILION: u32 = 0xD0634E; // 朱砂红：当前章 / 标记
        const LINE: u32 = 0x33445C; // 分隔线
        let song = crate::fonts::family_for(FontChoice::SongTi, Script::Ancient);

        // 目录条目先克隆出来：后面要用 &mut self 构建侧栏（划线跳页），
        // 不能一直借着 self.book。
        let toc: Vec<TocEntry> = book
            .toc
            .iter()
            .filter(|e| e.level <= 3)
            .cloned()
            .collect();
        let pages = self.paginate();
        // 章 → 首面叶码：**一次**分页建好映射，下面循环查表即可。
        // 原先每条竹简都调一次 `page_of_chapter`，而它内部每次都全量
        // 重排全书 —— N 条目录就是 N 次全书排版，面板一开就卡。
        let mut chapter_page: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();
        for (i, p) in pages.iter().enumerate() {
            if let Some(g) = p.first() {
                chapter_page.entry(g.chapter).or_insert(i);
            }
        }
        // 竖排（古文）目录从右到左：首章在最右，划线栏在右侧；
        // 横排（现代）从左到右：首章在最左，划线栏在左侧。
        let rtl = self.script == Script::Ancient;
        // 当前章：用后台悄悄对齐好的 current_chapter，目录朱砂红点落在准确卷目上。
        let cur_chapter = self.current_chapter;

        // 侧栏宽度：占视口 82%、上限 520、下限 280；满高抽屉。
        let panel_w = (vw * 0.82).min(520.0).max(280.0);
        let head_h = 52.0;
        let strip_h = (vh - head_h).max(160.0);

        let mut strip_nodes: Vec<Div> = Vec::new();
        // 竖排从右到左：首章在最右，竹简条整体靠右排；
        // 横排从左到右：首章在最左，竹简条靠左排。
        let mut strips = div()
            .id("toc-strips")
            .flex_1()
            .flex()
            .flex_row()
            .items_end()
            .gap_2()
            .p_3()
            .overflow_x_scroll();
        if rtl {
            // 用「可伸缩的空白」把竹简条推到右边，而不是 justify_end：
            // 章目多到溢出时 justify_end 会把左侧内容裁掉且滚不到，
            // 空白 spacer 在空间不足时缩到 0，仍可从最左一条开始滚动。
            strips = strips.child(div().flex_1());
        }

        if toc.is_empty() {
            strip_nodes.push(
                div()
                    .p_3()
                    .text_size(px(12.0))
                    .text_color(rgb(MOON_DIM))
                    .child(SharedString::from("这本书没有目录")),
            );
        }

        for e in toc.iter() {
            let cur = e.chapter == cur_chapter;
            let chapter = e.chapter;
            // 跳转下标：优先用已算好的 start_char_index，缺失则退回章首面。
            let jump_idx = e.start_char_index.unwrap_or(0);
            // 竹简条宽度：整体加宽，手指点得准（尤其触屏），仍是细竹简。
            // 三档只调宽度与字号区分层级，**高度统一**成一根根等高的竹简，
            // 整束目录像真竹简一样整齐，不会因章名长短参差（占位符防跳动）。
            let (w, fs) = match e.level {
                1 => (46.0, 16.0),
                2 => (40.0, 14.0),
                _ => (34.0, 12.5),
            };
            let fg = if cur { VERMILION } else { MOON };
            // 章名按竹条可用高度尽量排全，排不下才截断加省略号（高度统一后
            // 各档可用高度一致，只因字号不同容纳字数有别）。
            let fish_h = w * 0.84 / 1.846;
            let avail = (strip_h - fish_h * 2.0 - 26.0).max(16.0);
            let max_chars = (avail / (fs + 4.0)).floor().max(2.0) as usize;
            let title_txt = clip_chars(&e.title, max_chars);

            let mut title = div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .pt_3()
                .font_family(song.clone());
            for c in title_txt.chars() {
                title = title.child(
                    div()
                        .text_size(px(fs))
                        // 当前章加粗，一眼能认出「我在这卷」。
                        .when(cur, |d| d.font_weight(FontWeight::BOLD))
                        .text_color(rgb(fg))
                        .child(SharedString::from(c.to_string())),
                );
            }
            // 该章首页的叶码（小字），点这条简就跳到那一叶。查表，不再重排。
            let page_no = chapter_page.get(&chapter).copied().unwrap_or(0) + 1;

            let bg = if cur { PANEL_BG2 } else { PANEL_BG };
            strip_nodes.push(
                div()
                    .flex_shrink_0()
                    .w(px(w))
                    .h(px(strip_h))
                    .flex()
                    .flex_col()
                    .items_center()
                    .pb_2()
                    .bg(rgb(bg))
                    // 当前章描边加粗一档，朱砂红标出。
                    .border_2()
                    .border_color(rgb(if cur { VERMILION } else { LINE }))
                    .rounded(px(3.0))
                    .cursor_pointer()
                    // 鱼尾纹（月白，方能在深底上显形）。
                    .child(fish_tail_ink(w * 0.84, true, MOON))
                    .child(title)
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(rgb(if cur { VERMILION } else { MOON_DIM }))
                            .child(SharedString::from(format!("第{page_no}葉"))),
                    )
                    .child(div().flex_1())
                    .child(fish_tail_ink(w * 0.84, false, MOON))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            // 精准跳转：把引擎位置设到该章开头的字符下标，
                            // 清空当前画布、重新分页排版并渲染第一叶。
                            this.goto_char_index(jump_idx);
                            this.close_toc();
                            cx.notify();
                        }),
                    ),
            );
        }

        // 竹简条排序：竖排首章在最右（紧邻右侧划线栏），横排首章在最左。
        if rtl {
            for d in strip_nodes.into_iter().rev() {
                strips = strips.child(d);
            }
        } else {
            for d in strip_nodes {
                strips = strips.child(d);
            }
        }

        // 划线栏：与竹简条**并排平行**（同一条基线、同高），古风夜色配色。
        let mk_col = self.render_toc_column(&pages, rtl, 220.0, cx);

        let title_div = div()
            .text_size(px(14.0))
            .font_family(song)
            .text_color(rgb(MOON))
            .child(SharedString::from("目 錄"));
        let count_div = div()
            .text_size(px(10.5))
            .text_color(rgb(MOON_DIM))
            .child(SharedString::from(format!("共 {} 卷目", toc.len())));
        let close_div = small_button("关闭", false, self.theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_toc();
                cx.notify();
            }),
        );
        // 标题与关闭钮的左右位置随排版方向翻转：竖排时标题靠右（书从右翻开），
        // 关闭钮紧挨标题；条目数放在另一侧。
        let head = if rtl {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .h(px(head_h))
                .px_4()
                .bg(rgb(PANEL_BG2))
                .border_b_1()
                .border_color(rgb(LINE))
                .child(count_div)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(close_div)
                        .child(title_div),
                )
        } else {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .h(px(head_h))
                .px_4()
                .bg(rgb(PANEL_BG2))
                .border_b_1()
                .border_color(rgb(LINE))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(title_div)
                        .child(close_div),
                )
                .child(count_div)
        };

        // 书目主体：竹简条 + 划线栏，两栏平行并排。
        //   竖排（从右到左）：目录条 → 划线（划线紧贴目录、居最右）；
        //   横排（从左到右）：整体镜像，划线 → 目录条。
        let mut body_row = div().flex().flex_row().flex_1().overflow_hidden();
        body_row = if rtl {
            body_row.child(strips).child(mk_col)
        } else {
            body_row.child(mk_col).child(strips)
        };

        // 右侧滑入侧栏：用 toc_slide 计算右偏移与遮罩透明度。
        //   slide=1 → right=0（完全展开）；slide=0 → right=-panel_w（滑出屏外）。
        let slide = self.toc_slide.clamp(0.0, 1.0);
        let off = -(1.0 - slide) * panel_w;
        let mask_a = ((0.45 * slide * 255.0) as u32).min(255);
        let mask = (mask_a << 24) | 0x000000;
        let panel = div()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .right(px(off))
            .w(px(panel_w))
            .flex()
            .flex_col()
            .bg(rgb(PANEL_BG))
            .border_l_1()
            .border_color(rgb(LINE))
            .overflow_hidden()
            // 拦住冒泡：点面板内（章 / 划线 / 关闭钮）不触发外层遮罩的关闭。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(head)
            .child(body_row);

        // 全屏遮罩层：点面板外空白处平滑收起侧栏（mask 透明度随 slide 淡入）。
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(vw))
            .h(px(vh))
            .bg(rgba(mask))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // 拦住冒泡：否则关闭后事件继续传到正文区，会顺带翻一页。
                    cx.stop_propagation();
                    this.close_toc();
                    cx.notify();
                }),
            )
            .child(panel)
    }


    /// 某页的文字预览（跳过段首缩进占位，按简繁开关显示）。
    fn page_preview(&self, glyphs: &[Glyph], max: usize) -> String {
        let s: String = glyphs
            .iter()
            .filter(|g| !g.is_indent())
            .map(|g| self.shown(g.ch))
            .collect();
        clip_chars(s.trim(), max)
    }

    /// 目录侧栏里与卷目**并排平行**的划线栏（古风夜色配色）。
    ///
    /// 两栏同高、共用一条基线，与竹简条平行排布：
    ///   * 竖排（从右到左）：目录条在最左，划线栏紧贴其右；
    ///   * 横排（从左到右）：镜像——划线栏在最左，目录条在其右。
    /// 点任一条即跳到那一叶。
    fn render_toc_column(
        &self,
        pages: &[Vec<Glyph>],
        rtl: bool,
        w: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        const MOON: u32 = 0xE8EEF2;
        const MOON_DIM: u32 = 0x9FB0C4;
        const VERMILION: u32 = 0xD0634E;
        const LINE: u32 = 0x33445C;
        const CARD: u32 = 0x223146; // 划线卡底
        const COL_BG: u32 = 0x18222F; // 比面板更深的栏底，拉开层次

        let mut list = div()
            .id(SharedString::from("toc-col-mk"))
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .overflow_y_scroll();

        let hint = |s: &'static str| {
            div()
                .p_2()
                .text_size(px(10.5))
                .text_color(rgb(MOON_DIM))
                .child(SharedString::from(s))
        };

        // 划线按「面」聚合：一面上连续划过的字合成一条，便于回看。
        let mut marks: Vec<(usize, String, Option<String>)> = Vec::new();
        for (pi, pg) in pages.iter().enumerate() {
            let txt: String = pg
                .iter()
                .filter(|g| !g.is_indent())
                .filter(|g| self.underlines.contains(&(g.chapter, g.para, g.offset)))
                .map(|g| self.shown(g.ch))
                .collect();
            if txt.is_empty() {
                continue;
            }
            marks.push((
                pi,
                clip_chars(txt.trim(), 18),
                self.notes.get(&pi).map(|s| clip_chars(s, 22)),
            ));
        }
        let count = marks.len();
        if marks.is_empty() {
            list = list.child(hint(
                "还没有划线。\n拖选文字后点工具条的「划线」，起笔处会落一个朱点。",
            ));
        }
        for (page, txt, note) in marks {
            list = list.child(
                div()
                    .id(SharedString::from(format!("mk-{page}")))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(LINE))
                    .bg(rgb(CARD))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded(px(3.0))
                                    .bg(rgb(VERMILION)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(MOON))
                                    .child(SharedString::from(txt)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(rgb(MOON_DIM))
                            .child(SharedString::from(match note {
                                Some(n) => format!("第 {} 葉 · {n}", page + 1),
                                None => format!("第 {} 葉", page + 1),
                            })),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.goto_page(page);
                            this.close_toc();
                            cx.notify();
                        }),
                    ),
            );
        }

        let head = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(LINE))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(VERMILION))
                    .child(SharedString::from(format!("划线 {count}"))),
            )
            .child(
                div()
                    .text_size(px(9.5))
                    .text_color(rgb(MOON_DIM))
                    .child(SharedString::from("点一条即跳")),
            );

        let mut col = div()
            .w(px(w))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(COL_BG));
        // 朝目录的一侧画分隔线：竖排（栏在目录右侧）画左边，横排（栏在目录左侧）画右边。
        col = if rtl {
            col.border_l_1().border_color(rgb(LINE))
        } else {
            col.border_r_1().border_color(rgb(LINE))
        };
        col.child(head).child(list)
    }

    /// 左上角竖写书名签条。
    ///
    /// 线装书的题签贴在书衣左上角，翻开后仍要让人知道在读哪一部书，
    /// 所以这里让它常驻阅读视图左上角（天头留白处，不压正文）。
    /// 点它即呼出卷目——签条本身就是「这是什么书」的入口。
    ///
    /// 配色跟着主题走：日间＝宣纸底浓墨字，夜读＝深褐底米白字，
    /// 顶端一枚朱砂短线代替藏印，不为装饰而抢正文。
    fn render_spine_label(&self, cx: &mut Context<Self>) -> Div {
        let Some(book) = &self.book else {
            return div();
        };
        // 题签文字：书名或封面题签（【】显作「」，与版面其它文字一致）。
        let raw = if book.cover.title.is_empty() {
            &book.title
        } else {
            &book.cover.title
        };
        let title: String = raw.chars().map(crate::types::bracket_to_quote).collect();
        if title.is_empty() {
            return div();
        }
        let t = self.theme;
        // 贴向书页左上「天头」，而不是压在顶栏上：
        //   · 竖向落在工具栏之下（约 56px 起），让开顶栏按钮区；
        //   · 横向贴着正文页左缘——页在正文区居中，左缘 ≈ (视口宽-内容宽)/2，
        //     签条放在它左侧留白里，既不压字，又一眼看得出在读哪部书。
        let page_left = ((self.viewport_w - self.content_w) / 2.0).max(0.0);
        let left = (page_left - 62.0).max(8.0);
        let top = 56.0;
        div()
            .absolute()
            .top(px(top))
            .left(px(left))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(7.0))
            .pb(px(9.0))
            .px(px(4.0))
            .gap(px(5.0))
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.rule()))
            .rounded(px(2.0))
            .cursor_pointer()
            // 点签条出卷目，并吃掉事件：不当作翻页，也不收起别的浮层。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.open_toc();
                    this.press_at = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(div().w(px(18.0)).h(px(2.0)).bg(rgb(t.accent())))
            // 宽度给到两列（≈46px）、高度 200px：长书名自动分两列竖排，
            // 不再被单列截断缀「…」，可看到完整书名。
            .child(crate::shelf::vertical_block(
                &title,
                46.0,
                200.0,
                14.0,
                t.ink(),
                FontWeight::MEDIUM,
            ))
    }

    fn render_reading_toolbar(&mut self, total: usize, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        // 单页 / 双面对开由屏幕方向驱动（横屏对开、竖屏单页），不再提供手动切换；
        // 古籍竖笺为唯一排版格式（现代横笺已移除）。列数 / 留白见「版式」面板。

        // 顶部工具栏：横栏，含书架 / 封面 / 目录 / 样式 / 专注 / 版式等。
        // 字体、字号已收进「样式」面板；翻页改为「点正文区右半屏下一页 /
        // 左半屏上一页」，键盘 ← / → 方向键同样可用。
        let mut bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_2()
            .bg(rgb(t.chrome()))
            .border_b_1()
            .border_color(rgb(t.chrome_line()));

        bar = bar.child(small_button("‹ 书架", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.back_to_shelf();
                cx.notify();
            }),
        ));
        bar = bar.child(small_button("封面", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.persist();
                this.screen = Screen::Cover;
                cx.notify();
            }),
        ));

        let toc_on = self.show_toc;
        bar = bar.child(small_button("☰ 目录", toc_on, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                // 底部菜单「目录」：展开 / 收起侧栏（带滑入滑出动画）。
                if this.show_toc {
                    this.close_toc();
                } else {
                    this.open_toc();
                }
                cx.notify();
            }),
        ));

        // 样式面板：纸色 / 字级 / 版式 / 注文。
        let style_on = self.show_style;
        bar = bar.child(small_button("样式", style_on, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.show_style = !this.show_style;
                cx.notify();
            }),
        ));

        // 全文检索：输入词 → 全书查找 → 跳到那一面并朱砂红高亮。
        // 打开时把焦点交给检索框，否则输入法没有落点、打不进中文。
        let search_on = self.show_search;
        bar = bar.child(small_button("检索", search_on, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                this.show_search = !this.show_search;
                if this.show_search {
                    window.focus(&this.search_focus);
                } else {
                    this.clear_search();
                }
                cx.notify();
            }),
        ));

        // 专注模式：沉浸阅读，隐藏工具栏 / 状态栏 / 目录。
        let focus_on = self.focus_mode;
        bar = bar.child(small_button("专注", focus_on, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.focus_mode = !this.focus_mode;
                cx.notify();
            }),
        ));

        // 版式控制面板：列数 8 / 10 / 12 / 14 与四周留白随时可调
        // （单页 / 对开由屏幕方向自动决定）。
        let grid_on = self.show_grid_panel;
        bar = bar.child(small_button("版式", grid_on, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.show_grid_panel = !this.show_grid_panel;
                cx.notify();
            }),
        ));

        // 繁体 / 通假 / 注释 已收进正文下方的「样式条」与快捷键，避免顶栏重复。
        // 首行缩进固定两个字（横竖排一视同仁），不再提供开关。

        // 笔记 / 批注面板。
        let notes_on = self.show_notes_panel;
        bar = bar.child(small_button("笔记", notes_on, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.show_notes_panel = !this.show_notes_panel;
                cx.notify();
            }),
        ));

        // 帮助：快捷键与操作说明一览。
        let help_on = self.show_help;
        bar = bar.child(small_button("帮助", help_on, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.show_help = !this.show_help;
                cx.notify();
            }),
        ));

        bar = bar.child(div().flex_1());

        // 翻页：点正文区左 / 右半屏，或键盘方向键；章节跳转走「目录」。
        // 工具栏不再放上一页 / 下一页 / 上章 / 下章按钮。

        let _ = total;
        bar
    }

    /// 跳到上一章 / 下一章的首面。
    pub fn jump_chapter(&mut self, forward: bool) {
        let pages = self.paginate();
        let Some(cur_glyph) = pages.get(self.page).and_then(|p| p.first()).copied() else {
            return;
        };
        let cur_ch = cur_glyph.chapter;
        // 找到当前章的首面
        let cur_start = pages
            .iter()
            .position(|p| p.first().map(|g| g.chapter == cur_ch).unwrap_or(false))
            .unwrap_or(0);

        if forward {
            if let Some(p) = pages
                .iter()
                .position(|p| p.first().map(|g| g.chapter > cur_ch).unwrap_or(false))
            {
                self.page = p;
            }
        } else if self.page > cur_start {
            self.page = cur_start;
        } else {
            // 已经在章首，跳到上一章章首
            if let Some(prev_ch) = pages
                .get(cur_start)
                .and_then(|p| p.first())
                .map(|g| g.chapter)
                .filter(|c| *c > 0)
            {
                if let Some(p) = pages.iter().position(|pg| {
                    pg.first().map(|g| g.chapter == prev_ch - 1).unwrap_or(false)
                }) {
                    self.page = p;
                }
            }
        }
        self.persist();
    }

    /// 渲染一面（一篇叶子）。
    fn render_leaf(
        &self,
        pages: &[Page],
        idx: usize,
        back: bool,
        edge: LeafEdge,
        cx: &mut Context<Self>,
    ) -> Div {
        let page = pages.get(idx);
        let glyphs: Vec<Glyph> = page.map(|p| p.glyphs.clone()).unwrap_or_default();
        let start = page.map(|p| p.start_chapter).unwrap_or(false);
        let columns: Vec<Vec<Piece>> = page.map(|p| p.columns.clone()).unwrap_or_default();
        // 章末统计：本章在此收尾（下一葉即新章首，或已是全书末葉）时，
        // 把「篇 / 章 / 字」小表排在正文之后的第一片空列里。
        let stats: Vec<ChapStat> = self
            .chapters_ended_here(pages, idx)
            .iter()
            .filter_map(|&c| self.chapter_stat(c))
            .collect();
        // 现只保留古籍竖笺（现代横笺已移除）：一律走竖排渲染。
        let inner = self.render_ancient(
            &glyphs,
            &columns,
            back,
            start,
            idx,
            edge,
            &stats,
            LeafGeo::current(self),
            cx,
        );
        // 最小字号保护：窄屏 + 高列数（12/14）触发最小字号后，纸面可能超出
        // 视口高度——这里把该面包进一个限高（=内容区高）的滚动容器，允许
        // 单页内上下滑动查看，而不是把字压到更小或裁掉。
        let inner = if self.grid_min_font {
            let leaf_w = if self.duplex == Duplex::Facing {
                ((self.content_w
                    - crate::grid::gutter_w(
                        self.content_w,
                        self.grid.columns,
                        ANCIENT_EDGE,
                        self.grid.margin,
                    ))
                    / 2.0)
                    .max(80.0)
            } else {
                self.content_w.max(80.0)
            };
            let mut sc = div()
                .w(px(leaf_w))
                .h(px(self.content_h.max(120.0)))
                .flex()
                .flex_col()
                .items_center();
            sc.style().overflow.y = Some(Overflow::Scroll);
            if self.is_letter_mode() {
                // 信笺 14 列在窄屏上可能横向也放不下：允许左右滚动，
                // 而不是把字继续压小（最小字号保护的另一半）。
                sc.style().overflow.x = Some(Overflow::Scroll);
            }
            sc.child(inner)
        } else {
            inner
        };
        // 有笔记的叶：右上角探出一颗朱批红点。
        let mut wrap = div().relative().child(inner);
        if self.notes.contains_key(&idx) {
            wrap = wrap.child(
                div()
                    .absolute()
                    .top(px(2.0))
                    .right(px(2.0))
                    .w(px(9.0))
                    .h(px(9.0))
                    .rounded(px(5.0))
                    .bg(rgb(self.theme.accent())),
            );
        }
        // 牌记：书末最后一页（idx == 末页）加一枚方框，内竖排「某年某刻」，
        // 仿实体书版权页（刻本多在末叶边栏落此印记）。
        // 只在末叶**留白够多**时落印：正文排满时硬盖上去会遮字。
        let used_cols = columns.iter().filter(|c| !c.is_empty()).count();
        let sparse_tail = used_cols * 3 <= columns.len().max(1);
        if idx + 1 >= pages.len() && !pages.is_empty() && sparse_tail {
            let paper = if back {
                self.theme.paper_back()
            } else {
                self.theme.paper()
            };
            let colophon = "丙午年 笺读重梓";
            wrap = wrap.child(
                div()
                    .absolute()
                    .bottom(px(40.0))
                    .right(px(24.0))
                    .w(px(104.0))
                    .h(px(158.0))
                    .bg(rgb(paper))
                    .border_2()
                    .border_color(rgb(self.theme.frame_ink()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .child(crate::shelf::vertical_block(
                        colophon,
                        86.0,
                        132.0,
                        15.0,
                        self.theme.ink_soft(),
                        FontWeight::MEDIUM,
                    )),
            );
        }
        // 天头眉批：本页笔记以竖排小字显示在叶子顶部（天头）边栏，仿古人
        // 读书时的批注；字数不限，过长则折叠成多列并裁切。点工具条「笔记」
        // 即可在本页天头写下感悟。
        if let Some(note) = self.notes.get(&idx) {
            if !note.trim().is_empty() {
                // 天头眉批：竖排小字，且按繁简开关转繁体显示。
                let note_trad: String = note.chars().map(|c| self.shown(c)).collect();
                wrap = wrap.child(
                    div()
                        .absolute()
                        .top(px(2.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .h(px(54.0))
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .justify_center()
                        .child(crate::shelf::vertical_block(
                            &note_trad,
                            self.content_w,
                            48.0,
                            11.0,
                            self.theme.ink_soft(),
                            FontWeight::NORMAL,
                        )),
                );
            }
        }
        wrap
    }

    /// 章题（展示用）：源数据里的【】在此一律显作「」——
    /// 章题列、中缝篇题、卷首题署、分享题识都经这一道。
    fn chapter_title(&self, chapter: usize) -> String {
        self.book
            .as_ref()
            .and_then(|b| b.chapters.get(chapter))
            .map(|c| c.title.chars().map(crate::types::bracket_to_quote).collect())
            .unwrap_or_default()
    }

    /// 书耳题签：篇章名 → 卷名 → 提要，按本章内容自适应
    /// （见 `types::ear_label`）：章有标题用标题；无标题的多章书用
    /// 卷次（卷一 / 卷二……）；单章无题书摘正文开头作提要。
    fn ear_label(&self, chapter: usize) -> String {
        let Some(book) = self.book.as_ref() else {
            return String::new();
        };
        let Some(ch) = book.chapters.get(chapter) else {
            return String::new();
        };
        let opening = ch
            .paragraphs
            .first()
            .map(|p| p.text.as_str())
            .unwrap_or("");
        crate::types::ear_label(&ch.title, book.chapters.len(), chapter, opening)
    }

    // ------------------------------------------------------------------
    // 古文竖笺：八列、从右到左、四周双边、双行小字夹注
    // ------------------------------------------------------------------
    fn render_ancient(
        &self,
        glyphs: &[Glyph],
        columns: &[Vec<Piece>],
        back: bool,
        start_chapter: bool,
        idx: usize,
        edge: LeafEdge,
        stats: &[ChapStat],
        // 渲染几何（当前视图 / 晒本长图各算一套；全部几何读这一份）。
        geo: LeafGeo,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        let fs = geo.fs; // == 网格引擎字号（self.grid_settings 驱动）
        // 字格宽 / 高来自网格引擎：竖排列宽 = col_w，单字竖高 = row_h。
        let cell_w = geo.cell_w;
        let cell_h = geo.cell_h;
        let paper = if back { t.paper_back() } else { t.paper() };
        let chapter = glyphs.first().map(|g| g.chapter).unwrap_or(0);
        let note_index = if !geo.notes_inline && self.show_notes {
            self.book.as_ref().map(build_note_index).unwrap_or_default()
        } else {
            HashMap::new()
        };

        // 章首页留一列给章题，正文列数少一列（分页时已按同一规则算过）。
        // 列数来自网格引擎（8 / 10 / 12 / 14 可切）。
        let total_cols = self.grid.columns;
        let text_cols = if start_chapter {
            total_cols - 1
        } else {
            total_cols
        };
        let rows = geo.rows;
        // 卷首题署预留行数：与 layout_book 收到的 reserve_rows 同源
        // （按本帧实际行数算——晒本长图行数与阅读视图不同）。
        let reserve = crate::gloss::inscription_reserve(self.chapter_inscription_need(), rows);
        // 列已由 gloss::layout_book 切好（含双行夹注的跨列流转），直接排。

        // 地脚高度：先算出来，交给每一列一起加高——界行（乌丝栏）才能
        // 一直通到版框下沿，而不是在地脚上方断掉（刻本的行格是通的）。
        let small = (fs * 0.36).max(9.0);
        let foot_h = small + 10.0;
        let grid_w = text_cols as f32 * cell_w
            + if start_chapter {
                cell_w * ANCIENT_TITLE_COL
            } else {
                0.0
            }
            // 信笺模板在列间插了一道「一列宽」的中缝：地脚叶码 / 书耳
            // 的定位要把这一列也算进纸面宽度，才不至于偏到一边。
            + if geo.show_fish_tail { cell_w } else { 0.0 };

        // 界行（乌丝栏）：从右往左先排正文——`i = text_cols-1` 落在最左，
        // `i = 0` 落在最右，正是竖排的读序。每列左缘一道通高细线，
        // 空列也照画（刻本里字未排满，栏线一样到底）。
        let mut grid = div().flex().flex_row().items_start();
        for i in (0..text_cols).rev() {
            let items: &[Piece] = columns.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
            // 章首页的开卷列（i==0，紧邻版心）：顶部预留一段空白带给卷首题署，
            // 正文从带下接着排（带高 = chapter_inscription_reserve_rows）。
            let top_skip = if start_chapter && i == 0 { reserve } else { 0 };
            // 信笺模板：在第 7、8 个正文列之间插一道一列宽的中缝。
            // 章首页最右多一个章题列（也算一列），分割点随之前移一列，
            // 左右两边仍各是 7 列。
            let split_i = if start_chapter { 5 } else { 6 };
            if geo.show_fish_tail && i == split_i {
                grid = grid.child(self.render_letter_gutter(cell_w, cell_h, rows, foot_h));
            }
            grid = grid.child(self.ancient_column(
                items,
                cell_w,
                cell_h,
                rows,
                foot_h,
                fs,
                self.grid.columns,
                back,
                top_skip,
                &note_index,
                geo.interactive,
                cx,
            ));
        }
        // 章题列最后加：flex_row 末尾即最右，成为本章第一列（读序之首）。
        if start_chapter {
            grid = grid.child(self.ancient_title_column(
                chapter,
                cell_w,
                cell_h,
                rows,
                foot_h,
                fs,
            ));
        }

        // 地脚：章首页的叶码在中缝（对开）/ 居中（单页），此处不重复；
        // 地脚叶码：每一叶都补一枚小字叶码（居中）——章题列里放的是
        // **篇题数**（本篇第几章），叶码改由地脚与中缝分担，免得章首页
        // 丢了叶码。用绝对定位而非另起一行：地脚不占行格，界行才能
        // 贯通到版框底。
        let foot = div()
            .absolute()
            .left(px(0.0))
            .bottom(px(0.0))
            .w(px(grid_w))
            .h(px(foot_h))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(px(small))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from(han_num(idx + 1))),
            );

        // 版框：由配置驱动——single = 单线；double = 外粗内细（文武边栏）。
        // 文武的内层细线只在 double 时画；single 只需最外一根。对开拼合时
        // **内侧（贴中缝那一侧）不设框线**：左右两叶拼成一整张纸，外框只围
        // 整页外缘；单叶（Alone）才四边全封闭。内层 `relative`：地脚 / 卷首
        // 题署都用绝对定位叠在行格之上。
        let mut inner = div()
            .relative()
            .border_color(rgb(t.frame_ink()))
            .child(grid)
            .child(foot);
        // 文武边栏的内层细线（single 不画，避免两根细线并行像伪双边）。
        if geo.border_style == BorderStyle::Double {
            inner = match edge {
                LeafEdge::Left => inner.border_t_1().border_b_1().border_l_1(),
                LeafEdge::Right => inner.border_t_1().border_b_1().border_r_1(),
                LeafEdge::Alone => inner.border_1(),
            };
        }
        // 卷首题署（卷端）：章首页开卷列顶部叠一卷端题名——
        // 第一行（顶格）写书名·卷次，第二行（低几格）写署名；
        // 落在 layout_book 预留的空白带上，与正文互不遮挡。
        if start_chapter && reserve > 0 {
            inner = inner.child(self.render_inscription(chapter, cell_w, cell_h, fs, cx));
        }
        // 章末统计表：排在正文末列之后的第一片空列里、贴着天头，
        // 仿刻本卷尾的「篇—章—字」小表（版心之左、正文之尾）。
        if !stats.is_empty() {
            if let Some(empty) = first_empty_column(&columns, text_cols) {
                inner = inner.child(self.render_chapter_stats(
                    stats,
                    empty,
                    text_cols,
                    cell_w,
                    cell_h,
                    fs,
                ));
            }
        }
        let mut frame = div()
            .border_color(rgb(t.frame_ink()))
            .p(px(3.0))
            .child(inner);
        // 外层框线：single = 1px 单线；double = 3px 粗框（与内层细线夹 3px 间隙
        // 形成外粗内细的文武边栏）。对开内侧仍留空，两半拼合不打架。
        frame = match (edge, geo.border_style) {
            (LeafEdge::Left, BorderStyle::Double) => frame.border_t_3().border_b_3().border_l_3(),
            (LeafEdge::Right, BorderStyle::Double) => frame.border_t_3().border_b_3().border_r_3(),
            (LeafEdge::Alone, BorderStyle::Double) => frame.border_3(),
            (LeafEdge::Left, BorderStyle::Single) => frame.border_t_1().border_b_1().border_l_1(),
            (LeafEdge::Right, BorderStyle::Single) => frame.border_t_1().border_b_1().border_r_1(),
            (LeafEdge::Alone, BorderStyle::Single) => frame.border_1(),
        };

        // 纸面留白：对开时内侧（贴纸心那一侧）收掉，两半才能拼成一张纸。
        // `relative`：书耳用绝对定位挂在纸面左上 / 右上角。
        let pad = 16.0;
        let mut sheet = div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(pad))
            .pb(px(pad))
            .pl(px(if edge == LeafEdge::Right { 0.0 } else { pad }))
            .pr(px(if edge == LeafEdge::Left { 0.0 } else { pad }))
            .bg(rgb(paper))
            .child(frame);

        // 书耳：贴版框上角的一小片「耳朵」（刻本书脑侧的题签小耳）。
        // 位置随叶位走：单叶 / 左半叶贴左上（左外侧留白里），右半叶镜像到右上
        // ——紧贴版框不放空，与用户给的「天 / 地 / 鱼尾 / 书脑 / 耳子」图同款。
        {
            let pad = 16.0;
            let ear_w = (cell_w * 0.16).clamp(11.0, 14.0).min(pad - 2.0);
            // 耳高约一格（刻本书耳只有一两字高，不是一整条边）。
            let ear_h = (cell_h * 1.25).clamp(34.0, 150.0);
            // 耳内题签按内容自适应：篇章名 / 卷名 / 提要（types::ear_label）。
            let ear_label = self.ear_label(chapter);
            let (ear_x, ear_mirror) = match edge {
                // 右半叶：外侧在右，耳朵贴右上（右缘探进右外侧留白里）。
                LeafEdge::Right => (grid_w + 14.0, true),
                // 单叶 / 左半叶：外侧在左，耳朵贴左上。
                _ => (pad - ear_w, false),
            };
            sheet = sheet.child(book_ear(
                t,
                ear_w,
                ear_h,
                ear_x,
                pad,
                ear_mirror,
                &ear_label,
            ));
        }

        // 注文不进正文时，退回页脚注。
        if !geo.notes_inline {
            let notes = self.page_footnotes(glyphs, &note_index);
            if !notes.is_empty() {
                let mut area = div()
                    .w_full()
                    .mt_3()
                    .pt_2()
                    .border_t_1()
                    .border_color(rgb(t.rule()))
                    .flex()
                    .flex_col()
                    .gap_1();
                for (num, text) in notes {
                    area = area.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.accent()))
                                    .child(SharedString::from(circled(num))),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(t.ink_soft()))
                                    .child(SharedString::from(text)),
                            ),
                    );
                }
                sheet = sheet.child(area);
            }
        }

        sheet
    }

    /// 一列：正文单字与夹注小字自上而下相接。
    /// `rows` = 每列字数（来自网格引擎）；`foot_h` 是地脚高度：
    /// 列高照加，界行才能通到版框下沿。
    /// `columns` = 当前列数（≥10 时夹注走阈值字号 / 单行，避免小字糊掉）。
    fn ancient_column(
        &self,
        items: &[Piece],
        cell_w: f32,
        cell_h: f32,
        rows: usize,
        foot_h: f32,
        fs: f32,
        columns: usize,
        back: bool,
        // 顶部预留给卷首题署的空白行数（0 = 不预留，普通列）。
        top_skip: usize,
        note_index: &NoteIndex,
        // 字格 / 注块是否挂鼠标交互（晒本预览传 false：纯展示）。
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        // 列外框（含乌丝栏界行）高度固定，与邻列对齐；顶部预留带用内边距把
        // 正文内容压到带下，预留带内仍是空白纸面，留给卷首题署叠绘。
        let col = div()
            .w(px(cell_w))
            .h(px(cell_h * rows as f32 + foot_h))
            .flex()
            .flex_col()
            .items_center()
            // 丝栏：列间界行按样式面板所选（乌 / 金 / 朱）取色。
            .border_l_1()
            .border_color(rgb(t.rule_by(self.rule_color)));
        let mut content = div().w_full().flex().flex_col().items_center();
        if top_skip > 0 {
            content = content.pt(px(cell_h * top_skip as f32));
        }
        // 逐片渲染。连续西文（`1628` / `ISBN`）在这里**成组**：
        // 每两字占一格，组高 = `run_cells * cell_h`，与分页的半格账同源。
        // 「数字转汉字」开启时不分组——每个汉字独立一格，与分页一致。
        let west_mode = !self.han_digits;
        let mut idx = 0usize;
        while idx < items.len() {
            match &items[idx] {
                Piece::Main(g) if west_mode && crate::west::is_west(g.ch) => {
                    let mut j = idx;
                    while j < items.len() {
                        match &items[j] {
                            Piece::Main(g2) if crate::west::is_west(g2.ch) => j += 1,
                            _ => break,
                        }
                    }
                    content = content.child(self.ancient_west_run(
                        &items[idx..j],
                        cell_w,
                        cell_h,
                        fs,
                        back,
                        interactive,
                        cx,
                    ));
                    idx = j;
                }
                Piece::Main(g) => {
                    let mark = self.mark_kind(*g);
                    content = content.child(self.ancient_cell(
                        *g,
                        cell_w,
                        cell_h,
                        fs,
                        back,
                        mark,
                        note_index,
                        interactive,
                        cx,
                    ));
                    idx += 1;
                }
                Piece::Gloss(txt, src, host) => {
                    content = content.child(self.ancient_gloss(
                        txt,
                        *src,
                        *host,
                        cell_w,
                        cell_h,
                        fs,
                        columns,
                        back,
                        interactive,
                        cx,
                    ));
                    idx += 1;
                }
                // 避头尾禁则让出的空白格：占一个整字格，不渲染任何字，
                // 只把后续正文压到第二格（列顶避头）或留一列底空当（列底避尾）。
                Piece::Gap => {
                    content = content.child(div().w_full().h(px(cell_h)));
                    idx += 1;
                }
            }
        }
        col.child(content)
    }

    /// 一段连续西文（`1628` / `ISBN` / `3.14`）的竖排渲染。
    ///
    /// **宽度账**：两个字符占一个汉字格（[`crate::west::run_cells`]），
    /// 与分页的半格账同源；组高 = `格数 × cell_h`，整组在列中垂直居中。
    ///
    /// **旋转坐标**：传统刻本是整串顺时针转 90° 横躺（字头朝右）；
    /// 旋转后的逐字槽位由 [`crate::west::rotated_slots`] 给出（中心 / 原点 /
    /// 角度）。当前 GPUI 0.2.2 没有元素级旋转变换（`TransformationMatrix`
    /// 只在内部 sprite 上、`paint_glyph` 固定单位矩阵），故渲染退化为
    /// **同槽位并排直立**：每格两字并排、逐格向下，读序仍是自上而下，
    /// 宽度与分页完全一致。将来 GPUI 支持变换时，把这里换成按
    /// `rotated_slots` 的 `(cx, cy, angle)` 绘制即可，账目无需改动。
    ///
    /// 小数点（`.`）零宽：缩成一个小墨点贴在前一数字右下，不占独立格。
    fn ancient_west_run(
        &self,
        items: &[Piece],
        cell_w: f32,
        cell_h: f32,
        fs: f32,
        back: bool,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        let keys: Vec<(usize, usize, usize)> = items
            .iter()
            .filter_map(|p| match p {
                Piece::Main(g) => Some((g.chapter, g.para, g.offset)),
                _ => None,
            })
            .collect();
        let chars: Vec<char> = items
            .iter()
            .filter_map(|p| match p {
                Piece::Main(g) => Some(crate::west::vertical_glyph(g.ch)),
                _ => None,
            })
            .collect();
        let groups = crate::west::pack_cells(&chars);
        let rows = groups.len().max(1);
        let wfs = crate::west::west_font_size(cell_w, fs);
        let ink = if back { t.ink_soft() } else { self.palette.ink(t) };
        // 西文与数字走 `FontRole::Latin`（Computer Modern）——学术排版的
        // 衬线数字，与汉字的宋体字面最搭；字体名不写死，走配置表。
        let latin = crate::fonts::family_for_role(crate::fonts::FontRole::Latin);

        let mut block = div()
            .w(px(cell_w))
            .h(px(rows as f32 * cell_h))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .font_family(latin.clone());
        // 选区高亮：西文已无逐字框，整组垫一层朱底作提示。
        if keys.iter().any(|k| self.in_selection(*k)) {
            block = block.relative().child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(cell_w))
                    .h(px(rows as f32 * cell_h))
                    .bg(rgba(0xA63A2B2E)),
            );
        }
        for group in &groups {
            let mut row = div()
                .w(px(cell_w))
                .h(px(cell_h))
                .flex()
                .flex_row()
                .items_center()
                .justify_center();
            for ch in group.chars() {
                if crate::west::is_west_dot(ch) {
                    // 小数点：小墨点贴在数字右下角，不占宽。
                    row = row.child(
                        div()
                            .w(px(wfs * 0.42))
                            .h(px(cell_h))
                            .flex()
                            .items_end()
                            .justify_center()
                            .pb(px(cell_h * 0.20))
                            .child(
                                div()
                                    .w(px((wfs * 0.16).max(1.5)))
                                    .h(px((wfs * 0.16).max(1.5)))
                                    .rounded(px(wfs * 0.08))
                                    .bg(rgb(ink)),
                            ),
                    );
                } else {
                    row = row.child(
                        div()
                            .text_size(px(wfs))
                            .text_color(rgb(ink))
                            .child(SharedString::from(ch.to_string())),
                    );
                }
            }
            block = block.child(row);
        }
        if interactive {
            block = block.on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                    // 西文组不可逐字选，按下只标记「碰过正文」，绝不翻页。
                    this.touched_glyph = true;
                    cx.stop_propagation();
                    this.popup = None;
                    this.clear_selection();
                    cx.notify();
                }),
            );
        }
        block
    }

    /// 双行小字夹注：一条注文排成两个半宽小列（先右列后左列），
    /// 占 `ceil(n/2)` 个半格；小字字号取**正文一半**。
    /// 注文过长时由分页侧跨列流转，这里只渲染本列分到的那一段
    /// （注尾会是另一列列首的另一个 gloss 块）。
    /// `columns` ≥ 10 时（正文本就偏小）小字设可读性阈值，避免完全糊掉。
    ///
    /// **左侧留白**：注块左缘留 `NOTE_PAD_RATIO` 格宽的空白，两个小列
    /// 偏右安居、右缘仍贴列右缘——夹注小字因此「夹」在正文列里，
    /// 不与界行齐平（刻本的夹注也是嵌在正文之间的）。
    ///
    /// **颜色**：小字色由「正文墨色向纸色兑淡」自动推导（`z_note`），
    /// 明暗底自己切换，永远比正文淡一档；暗金墨色组深底下用米白。
    ///
    /// 交互层：每个注块都是可点的——点它弹出放大悬浮卡（`NoteZoom`），
    /// 把小字注文放大显示（手机上小字看不清的问题就这么解决）。
    /// 事件先于 Canvas 的翻页判定消费（`stop_propagation` +
    /// `touched_glyph`），点注文绝不翻页。
    fn ancient_gloss(
        &self,
        text: &str,
        source: NoteSource,
        host: Glyph,
        cell_w: f32,
        cell_h: f32,
        fs: f32,
        columns: usize,
        back: bool,
        // 是否挂鼠标交互（晒本预览传 false：注块不可点）。
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        let is_user = matches!(source, NoteSource::User);
        let chars: Vec<char> = text.chars().collect();
        let rows = (chars.len() + 1) / 2;
        let row_h = cell_h / 2.0;
        // 小字 = 正文一半（0.5）；列数 ≥10 时下限抬到 10（可读性阈值），
        // 但上沿不越过半格高，保证小字始终在格内、不与上一字重叠
        // （放不下由 overflow_hidden 裁齐；跨列流转已由分页保证一段注
        // 不会超过一列容量，这里的 hidden 只是兜底）。
        let floor = if columns >= 10 { 10.0 } else { 7.0 };
        let nfs = ((fs * 0.5).max(floor)).min(row_h.max(7.0));
        // 左侧留白：一道格宽 1/6 的空带，两个小列压缩进余下宽度，
        // 右缘仍贴列右缘——夹注因此嵌在正文列里，不与界行齐平。
        let pad = cell_w * crate::gloss::NOTE_PAD_RATIO;
        let sub_w = (cell_w - pad) / 2.0;
        // 注文小字色：墨色 × 纸色自动推导（永远比正文淡一档）。
        // **用户自加夹注例外**：改深朱砂红（#C62828），与原本夹注一眼可辨。
        let note_color = if is_user {
            USER_NOTE_INK
        } else {
            self.palette.z_note(t, back)
        };
        // 用户夹注的底衬：深朱砂向纸色兑到「极其微弱」，只作可辨识提示。
        let note_bg = if is_user {
            let paper = if back { t.paper_back() } else { t.paper() };
            Some(crate::theme::mix(USER_NOTE_INK, paper, USER_NOTE_BG_FADE))
        } else {
            None
        };
        // 注文全体（跨列流转的一段）——点它时放大显示的就是这段。
        // 【】在这里也显作「」（放大卡与行内注保持同一套字）。
        let note_text: String = text.chars().map(crate::types::bracket_to_quote).collect();
        // 批注字体：楷体（玄冬楷书 / 霞鹜文楷），与正文宋体分开——
        // 刻本里正是「正文匠体、注文手写体」的层次。走古文链取字，
        // 横笺没有楷体链时才不会退化成系统黑体。
        let note_family = crate::fonts::note_family(self.note_font());
        let mut block = div()
            .w(px(cell_w))
            .h(px(rows as f32 * row_h))
            .flex()
            .flex_col()
            .items_center()
            .overflow_hidden()
            .font_family(note_family.clone());
        // 用户夹注：整块垫一层极淡朱砂底衬（不抢正文，只作「这是我加的」提示）。
        if let Some(bg) = note_bg {
            block = block.bg(rgb(bg));
        }
        // 两个监听器各持一份文本副本（闭包是 `move`，且都要独立存活）。
        let zoom_text = note_text.clone();
        let menu_text = note_text.clone();
        if interactive {
            block = block
                // 可点注块：命中范围 = 整个夹注块矩形（Canvas 级 bbox 命中
                // 的另一入口，元素级事件更准——滚动容器里也贴着视觉位置）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        // 碰过正文：松手时不翻页；并收起字工具条 / 拖选。
                        this.touched_glyph = true;
                        this.popup = None;
                        this.clear_selection();
                        this.note_zoom = Some(crate::gloss::NoteZoom {
                            text: zoom_text.clone(),
                            x: event.position.x.to_f64() as f32,
                            y: event.position.y.to_f64() as f32,
                            page: this.page,
                            pinyin: None,
                        });
                        cx.notify();
                    }),
                );
            // 右键（桌面）/ 长按（移动）：**仅用户自加夹注**弹「修改 / 删除」菜单。
            // 古籍原本的夹注不可改删——那是底本的一部分，动了就失真。
            if is_user {
                let host_key = (host.chapter, host.para, host.offset);
                block = block.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.touched_glyph = true;
                        this.popup = None;
                        this.clear_selection();
                        this.note_menu = Some(crate::gloss::NoteMenu {
                            key: host_key,
                            text: menu_text.clone(),
                            x: event.position.x.to_f64() as f32,
                            y: event.position.y.to_f64() as f32,
                        });
                        cx.notify();
                    }),
                );
            }
        }
        for r in 0..rows {
            // 右列读在前、左列续后，所以先排左列字、后排右列字。
            let pair = [chars.get(r + rows).copied(), chars.get(r).copied()];
            let mut line = div()
                .w(px(cell_w))
                .h(px(row_h))
                .flex()
                .flex_row()
                .items_center();
            // 注块左缘的留白带（不排字）。
            line = line.child(div().w(px(pad)).h(px(row_h)));
            for c in pair {
                line = line.child(match c {
                    // 注文里的【】同样显作「」（夹注不过 shown，这里单独过一道）。
                    Some(ch) => div()
                        .w(px(sub_w))
                        .h(px(row_h))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(nfs))
                        .text_color(rgb(note_color))
                        .child(SharedString::from(crate::types::bracket_to_quote(ch).to_string())),
                    None => div().w(px(sub_w)).h(px(row_h)),
                });
            }
            block = block.child(line);
        }
        block
    }

    /// 章题列：照刻本「版心」的样子——天头留白、上鱼尾、竖排篇题、
    /// **篇题数**（本篇第几章）、下鱼尾，一列通高（与八列界行同高，
    /// 栏线上下都顶到版框）。位序与样式照用户参考图「横版」。
    fn ancient_title_column(
        &self,
        chapter: usize,
        cell_w: f32,
        cell_h: f32,
        rows: usize,
        foot_h: f32,
        fs: f32,
    ) -> Div {
        let t = self.theme;
        let title = self.chapter_title(chapter);
        let col_w = cell_w * ANCIENT_TITLE_COL;
        // 篇题字号与正文齐（刻本不把篇题放大，靠独占一列区分）。
        // 丝栏：列间界行按样式面板所选（乌 / 金 / 朱）取色。
        let mut col = div()
            .w(px(col_w))
            .h(px(cell_h * rows as f32 + foot_h))
            .flex()
            .flex_col()
            .items_center()
            .border_l_1()
            .border_color(rgb(t.rule_by(self.rule_color)));
        // 天头：留约一字高的空白，鱼尾才落在版心上部而不是贴框。
        let head = cell_h * 1.0;
        col = col.child(div().h(px(head)));
        // 上鱼尾：与版心同一枚燕尾（深灰 `#333333`），尺寸同式。
        let (tw, th) = crate::banxin::banxin_tail_size(col_w, cell_h);
        col = col.child(banxin_tail(tw, th, true, t.fish_ink()));
        col = col.child(div().h(px(cell_h * 0.18)));
        // 篇题能排几字：总高扣天头、两枚鱼尾、篇题数与几段留白，
        // 余下全给篇题——鱼尾取了 1.5 字高后，短叶也不会溢出。
        let nfs = (fs * 0.42).max(9.0);
        let num_h = (nfs + 2.0) * han_num(chapter + 1).chars().count() as f32;
        let avail = cell_h * rows as f32 + foot_h
            - head
            - th * 2.0
            - num_h
            - cell_h * (0.18 + 0.1 + 0.38);
        let take_n = (avail / cell_h).floor().clamp(0.0, 7.0) as usize;
        // 篇题：一字一格，自上面下。
        for c in title.chars().take(take_n) {
            col = col.child(
                div()
                    .w(px(col_w))
                    .h(px(cell_h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(fs))
                    .text_color(rgb(t.ink()))
                    .child(SharedString::from(c.to_string())),
            );
        }
        // 篇题数（本篇第几章，汉字数词）：小字，竖排压在下鱼尾之上——
        // 位序「上鱼尾 → 篇题 → 篇题数 → 下鱼尾」（参考图「横版」）。
        // 叶码不在此处：中缝（对开）与地脚（单页）另有一份。
        col = col.child(div().flex_1());
        for c in han_num(chapter + 1).chars() {
            col = col.child(
                div()
                    .w(px(col_w))
                    .h(px(nfs + 2.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(nfs))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from(c.to_string())),
            );
        }
        col = col.child(div().h(px(cell_h * 0.1)));
        // 下鱼尾：与上鱼尾对合，夹住篇题与篇题数（同形，开口相对）。
        col = col.child(banxin_tail(tw, th, false, t.fish_ink()));
        col.child(div().h(px(cell_h * 0.38)))
    }

    /// 信笺模板的中缝：**一列宽**的一道纸带，立在 14 个正文列的正中
    /// （第 7、8 列之间）。左缘一条细线（本栏 `border_l`），右缘那条由
    /// 右邻列的界行充当——两条线与页内丝栏同粗细、同起止，上下通到
    /// 内框，正是参考图里「双细线夹一字宽纸带」的样子。
    /// 带内正中立「对鱼尾」：上鱼尾 Λ 口朝下、下鱼尾 V 口朝上，开口
    /// 相对，尾间空约三格；参考图的中缝除这枚对鱼尾外是空的
    /// （信笺不印篇题 / 叶码 / 朱印）。
    fn render_letter_gutter(&self, cell_w: f32, cell_h: f32, rows: usize, foot_h: f32) -> Div {
        let t = self.theme;
        let gw = cell_w;
        let (tw, th) = crate::banxin::letter_tail_size(gw, cell_h);
        let mut col = div()
            .w(px(gw))
            .h(px(cell_h * rows as f32 + foot_h))
            .flex()
            .flex_col()
            .items_center()
            // 左缘细线；右缘由右邻列（正文列或章题列）的 border_l 提供。
            .border_l_1()
            .border_color(rgb(t.rule_by(self.rule_color)))
            .bg(rgb(t.paper()))
            .relative()
            // 地脚那一段不参与「垂直居中」，对鱼尾才落在正文区的正中。
            .pb(px(foot_h));
        // 折痕：纸对折的物理褶，落在中缝正中（极细虚线，见 `fold_crease`）。
        col = col.child(fold_crease(
            gw * 0.5 - crate::banxin::FOLD_W * 0.5,
            0.0,
            cell_h * rows as f32,
            t.fold_ink(),
        ));
        col = col.child(div().flex_1());
        // 上鱼尾：∧ 口朝下（开口相对）。
        col = col.child(letter_fishtail(tw, th, true, t.fish_ink()));
        // 尾间空约三格（参考图实测 157px ≈ 3 个列宽）。
        col = col.child(div().h(px(cell_h * 3.0)));
        // 下鱼尾：∨ 口朝上。
        col = col.child(letter_fishtail(tw, th, false, t.fish_ink()));
        col.child(div().flex_1())
    }

    /// 中缝（版心）：对开时立在纸心的一道竖栏——左缘一条细线（与页内
    /// 界行同粗细、与内框同起止）夹一字宽的纸带，右缘那条由右半叶最左
    /// 一列的界行充当（两线间距恰为一列字宽，与左右丝栏等距延续）。
    /// 带内自上而下排布：
    ///
    /// ```text
    ///   上鱼尾（∧ 口朝下）
    ///   书名   宋词三百首     ← 极小号宋体，竖排
    ///   卷次   卷五十一
    ///   篇题   （余量有多少排多少）
    ///          ⋯⋯
    ///   叶码   五十
    ///   下鱼尾（∨ 口朝上）
    ///   朱印
    /// ```
    ///
    /// 正中另有一道**极细虚线**（0.5px、`#999999`）作纸对折的折痕。
    /// 缝宽约等于一行字宽（`grid::gutter_w`）：刻本版心本就是一列字的
    /// 宽度；文字的高度账由 `banxin::layout` 按优先级分（叶码 → 卷次 →
    /// 书名 → 篇题），分不够就截字，绝不溢出、绝不压字。
    /// 左半叶是章首时不用它——那一叶自带的「章题列」本身就是版心。
    fn render_gutter(&self, chapter: usize, page_idx: usize, detailed: bool) -> Div {
        let t = self.theme;
        let fs = self.size_ancient;
        let cell_h = self.row_h;
        // 中缝宽＝网格引擎预留的两叶间隙（`grid::gutter_w`）：约一行字宽，
        // 纸面总宽不变，只是把原来「两叶之间的空白」换成了纸心的一道版心。
        let gw = crate::grid::gutter_w(
            self.content_w,
            self.grid.columns,
            ANCIENT_EDGE,
            self.grid.margin,
        );
        // 地脚高与 `render_ancient` 同式，中缝才能与纸面上下对齐。
        let foot_h = (fs * 0.36).max(9.0) + 10.0;
        // 与纸面同高：上下留白 16、版框双边 3+3、框内留边 3+3、内栏线 1+1，
        // 再加行格与地脚——和 `render_ancient` 里那张纸严丝合缝。
        let h = 16.0 * 2.0 + 14.0 + cell_h * self.grid_rows as f32 + foot_h;
        // 天头：留约三成字高，上鱼尾才落在版心上部而不是贴着内框。
        let head = cell_h * 0.30;
        // 书口文字：书名（去括号）、卷次「卷五十一」、叶码「五十」。
        let book = self.book_title();
        let juan = format!("卷{}", han_num(chapter + 1));
        let page = han_num(page_idx + 1);
        let lay = crate::banxin::layout(crate::banxin::BanxinOpts {
            gutter_w: gw,
            // 可用高 = 纸高 − 上下留白(20×2) − 天头；`banxin::layout`
            // 再扣两枚鱼尾与朱印，剩下的才是文字。
            band_h: (h - 40.0 - head).max(40.0),
            cell_h,
            fs,
            book: &book,
            juan: &juan,
            page: &page,
            chapter_title: &self.chapter_title(chapter),
            detailed,
            seal: true,
        });
        // 书口小字**极小号宋体**：走正文字体（默认文津宋体）的回退链，
        // 横笺里也不会退化成黑体。
        let family = crate::fonts::family_for(self.current_font(), Script::Ancient);
        let mut col = div()
            .w(px(gw))
            .h(px(h))
            .flex()
            .flex_col()
            .items_center()
            // `relative`：版心细线用绝对定位锚在本栏内（GPUI 的 absolute
            // 子元素以最近的 positioned 祖先为定位上下文，缺了它线会飘到
            // 根节点上去）。
            .relative()
            .overflow_hidden()
            .pt(px(16.0 + 4.0))
            .pb(px(16.0 + 4.0))
            .bg(rgb(t.paper()));
        // 版心细线（界栏）：与左右叶的**内框**（border_1）同起同止——参考图
        // 「横版」里中缝线不顶纸缘、也不碰外粗框。线位 = 纸面顶 + 留白16
        // + 外框3 + 框内留边3 = 22（col 与纸面同高同顶，故 absolute
        // top/bottom 取 22，即内框上/下沿），头尾与内框严丝合缝。
        // 只画**左缘一条**：右缘那条由右半叶最左一列的界行（丝栏）充当
        // ——否则 col 右线与右叶第一根界行只隔几像素，中缝靠右就像
        // 并排的两条竖线（用户实测指出）。左右两线因此分居中缝两侧、
        // 与左右叶的界行等距延续（间距恰为一列字宽）。
        // 取色与页内界行同源（乌 / 金 / 朱丝栏）。
        let rule_lines = div()
            .absolute()
            .top(px(22.0))
            .bottom(px(22.0))
            .left(px(0.0))
            .w(px(1.0))
            .bg(rgb(t.rule_by(self.rule_color)));
        col = col.child(rule_lines);
        // 折痕：纸**对折**的那一道物理褶，落在版心正中——0.5px、`#999999`、
        // 虚线，只作「纸的痕迹」，比界行还淡，不抢鱼尾与书口文字。
        col = col.child(fold_crease(
            gw * 0.5 - crate::banxin::FOLD_W * 0.5,
            22.0,
            (h - 44.0).max(0.0),
            t.fold_ink(),
        ));
        // 纸心头顶 / 脚底的框线贯通：对开两叶拼成**一张平铺的整纸**，
        // 外框粗线与内框细线必须横贯全页、全封闭——左叶的上下框只画到
        // 纸心左缘、右叶只从纸心右缘起，若不在这里补上中缝那一段，
        // 上框 / 下框就在纸心断成两截（用户指出「头尾没有封闭」）。
        // 位置与 `render_ancient` 的版框逐层对齐：纸面顶下 16（留白）=
        // 外框粗线（3px，与左右叶 frame 的 border_t_3 同粗同位），
        // 再下 3px（框内留边）= 内框细线（1px，与 inner 的 border_t_1
        // 同位）；底边对称（bottom 16 = 粗线、bottom 22 = 细线）。
        // 列数（8/10/12/14）只改缝宽 gw，这几条线的位置恒定不动。
        let frame_col = t.frame_ink();
        let edge = |top: bool, thick: f32, inset: f32| -> Div {
            let mut e = div()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
                .h(px(thick))
                .bg(rgb(frame_col));
            e = if top {
                e.top(px(inset))
            } else {
                e.bottom(px(inset))
            };
            e
        };
        // 上外框粗线 / 上内框细线
        col = col.child(edge(true, 3.0, 16.0));
        col = col.child(edge(true, 1.0, 22.0));
        // 下外框粗线 / 下内框细线
        col = col.child(edge(false, 3.0, 16.0));
        col = col.child(edge(false, 1.0, 22.0));
        col = col.child(div().h(px(head)));
        // 上鱼尾：凹口朝下（直线多边形，深灰 `#333333`，见 `banxin_tail`）。
        // 尺寸由 `banxin::layout` 按「宽不越界栏、高约 1.2 倍行高」算好。
        col = col.child(banxin_tail(lay.tail_w, lay.tail_h, true, t.fish_ink()));
        col = col.child(div().h(px(lay.gap)));
        // 书名 → 卷次 → 篇题（极小号宋体，一字一格竖排）。
        for line in &lay.above {
            col = col.child(banxin_vtext(line, gw, &family, t));
        }
        // 余量全给中间：书名卷次贴着上鱼尾、叶码贴着下鱼尾，两枚鱼尾
        // 「夹」住书口文字，正是刻本版心的样子。
        col = col.child(div().flex_1());
        // 叶码。
        for line in &lay.below {
            col = col.child(banxin_vtext(line, gw, &family, t));
        }
        col = col.child(div().h(px(lay.gap)));
        // 下鱼尾：∨ 口朝上，与上鱼尾开口相对。
        col = col.child(banxin_tail(lay.tail_w, lay.tail_h, false, t.fish_ink()));
        // 朱砂小印「箋讀」：贴版心下端（影印本版心印落在下鱼尾之下）。
        // 版面太挤时 `banxin::layout` 会把印砍掉（叶码优先）。
        if lay.seal_h > 0.0 {
            col = col.child(div().h(px(lay.gap)));
            col = col.child(banxin_seal(gw * 0.80, gw * 1.60));
        }
        col
    }

    /// 书名（版心书口用）：去《》【】「」等括号与空白，只留题名——
    /// 版心只有一格宽，题名外的符号一律不排。
    fn book_title(&self) -> String {
        self.book
            .as_ref()
            .map(|b| {
                b.title
                    .chars()
                    .filter(|c| {
                        !matches!(c, '《' | '》' | '〈' | '〉' | '【' | '】' | '「' | '」' | '『' | '』')
                            && !c.is_whitespace()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 一章的「篇 / 章 / 字」（章序与字数都写汉字数词）。
    fn chapter_stat(&self, ci: usize) -> Option<ChapStat> {
        let b = self.book.as_ref()?;
        let c = b.chapters.get(ci)?;
        let mut chars = 0usize;
        for p in &c.paragraphs {
            chars += p
                .text
                .chars()
                .filter(|ch| !ch.is_whitespace() && !is_punct(*ch))
                .count();
        }
        Some(ChapStat {
            // 统计表里的篇名同样显「」不显【】。
            title: c.title.chars().map(crate::types::bracket_to_quote).collect(),
            index: ci + 1,
            chars,
        })
    }

    /// 这一叶上「收尾」的章：本章正文到此为止（下一叶是新章首，或已是全书末叶）。
    /// 通常只有一章；短章恰好两、三章同叶收尾时，统计表就一行一篇并排列出
    /// ——正是影印本卷尾那条「篇 / 章 / 字」小表的样子。
    fn chapters_ended_here(&self, pages: &[Page], idx: usize) -> Vec<usize> {
        chapters_ended_at(pages, idx)
    }

    /// 章末统计表：照用户给的影印本样式——三列「篇 / 章 / 字」，一行一篇，
    /// 章序与字数都写作汉字数词（「一」「六十」），不加全书合计行。
    /// 排在 `empty` 列（正文之后第一片空列）的天头，右缘贴住正文末列。
    fn render_chapter_stats(
        &self,
        stats: &[ChapStat],
        empty: usize,
        text_cols: usize,
        cell_w: f32,
        cell_h: f32,
        fs: f32,
    ) -> Div {
        let t = self.theme;
        if stats.is_empty() {
            return div();
        }
        // 正文之后还剩多少宽可用：表就缩在这段空白里，绝不压到正文上。
        let avail = text_cols.saturating_sub(empty) as f32 * cell_w;
        if avail < cell_w * 1.2 {
            // 只剩不到一列出头（章末正好收在版框左缘），宁可不画。
            return div();
        }
        let cw = (avail / 3.0).min(cell_w * 1.12).max(18.0);
        let rh = (cell_h * 0.5).max(12.0);
        let sfs = (fs * 0.40).max(8.0);
        let tw = cw * 3.0;
        let x = (avail - tw).max(0.0);
        let line = t.frame_ink();
        let cell = |s: String| -> Div {
            // 汉字数词也是一「字」一宽（「六十」比「六」宽），按字数缩格内字号。
            let len = s.chars().count().max(1) as f32;
            let cfs = (cw * 0.86 / len).min(sfs).max(6.0);
            div()
                .w(px(cw))
                .h(px(rh))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .border_l_1()
                .border_b_1()
                .border_color(rgb(line))
                .text_size(px(cfs))
                .text_color(rgb(t.ink()))
                .child(SharedString::from(s))
        };
        let row = |a: String, b: String, c: String| -> Div {
            div()
                .flex()
                .flex_row()
                .child(cell(a))
                .child(cell(b))
                .child(cell(c))
        };
        let mut table = div()
            .absolute()
            .top(px(0.0))
            .left(px(x))
            .w(px(tw))
            .border_t_1()
            .border_r_1()
            .border_color(rgb(line))
            .bg(rgb(t.paper()))
            .child(row("篇".to_string(), "章".to_string(), "字".to_string()));
        for st in stats {
            // 篇名过长时只留三字，宁缺毋滥。
            let title: String = st.title.chars().take(3).collect();
            table = table.child(row(title, han_num(st.index), han_num(st.chars)));
        }
        table
    }

    /// 卷首题署（卷端）：章首页开卷列顶部叠一卷端题名。
    ///
    /// 与 `layout_book` 预留的空白带（高 = `chapter_inscription_reserve_rows` 行）
    /// 严丝合缝对齐：第一行（顶格）写**书名·卷次**，第二行（低几格、向右退让）
    /// 写**署名**——最正宗的开卷体验。正文已从带下接着排，互不遮挡。
    ///
    /// 定位：章首页 `text_cols = 列数 - 1`，开卷列（i==0）在格内最右、紧邻版心，
    /// 其 x = (列数 - 2) × 字格宽；遮罩 / 卷首题署都据此落位。
    fn render_inscription(
        &self,
        chapter: usize,
        cell_w: f32,
        cell_h: f32,
        fs: f32,
        _cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        let reserve = self.chapter_inscription_reserve_rows() as f32;
        let song = crate::fonts::family_for(FontChoice::SongTi, Script::Ancient);
        // 书名·卷次（第一行，顶格） + 署名（第二行，低几格）。
        let (book_title, author) = match &self.book {
            Some(b) => (
                b.title.clone(),
                if b.author.trim().is_empty() {
                    "佚名".to_string()
                } else {
                    b.author.clone()
                },
            ),
            None => (String::new(), "佚名".to_string()),
        };
        let vol = crate::types::chapter_volume(chapter, &self.chapter_title(chapter));
        let title_line = format!("{book_title}{vol}");

        // 开卷列 x：章首页 text_cols = 列数-1，开卷列 i==0 → (列数-2) × 字格宽。
        let x = (self.grid.columns.saturating_sub(2)) as f32 * cell_w;
        let mut block = div()
            .absolute()
            .top(px(0.0))
            .left(px(x))
            .w(px(cell_w))
            .h(px(reserve * cell_h))
            .flex()
            .flex_col()
            .items_center()
            // 题署只许占住预留带：书名 / 署名过长时裁在带内，
            // 绝不越到带下的正文行上去（越界就成了「压字」）。
            .overflow_hidden()
            .font_family(song);

        // 第一行：书名·卷次，顶格，墨色，逐字竖排。
        let mut used = 0usize;
        for c in title_line.chars() {
            if used as f32 >= reserve - 1.0 {
                break; // 预留带容不下则裁切（标题过长不会顶穿版框）。
            }
            block = block.child(
                div()
                    .h(px(cell_h))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(fs))
                    .text_color(rgb(t.ink()))
                    .child(SharedString::from(c.to_string())),
            );
            used += 1;
        }
        // 间隔一行（卷首题署「书名」与「署名」之间留一道空白，对应预留带的 +1）。
        if (used as f32) < reserve - 1.0 {
            block = block.child(div().h(px(cell_h)));
            used += 1;
        }
        // 第二行：署名，低几格——向右退让（pl）并改用淡墨，落款意味。
        for c in author.chars() {
            if used as f32 >= reserve {
                break;
            }
            block = block.child(
                div()
                    .h(px(cell_h))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pl(px(cell_w * 0.16))
                    .text_size(px(fs * 0.92))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from(c.to_string())),
            );
            used += 1;
        }
        block
    }

    /// 正文单字格：主字 + 旁注（繁体/通假）+ 句读朱点。
    fn ancient_cell(
        &self,
        g: Glyph,
        cell_w: f32,
        cell_h: f32,
        fs: f32,
        back: bool,
        mark: Option<MarkKind>,
        note_index: &NoteIndex,
        // 是否挂鼠标交互（晒本预览传 false：字格不可点）。
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        if g.is_indent() {
            // 段首缩进占位：纯空白字格，但按下也算「碰过正文」，避免误翻页。
            return div()
                .w(px(cell_w))
                .h(px(cell_h))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.touched_glyph = true;
                        cx.stop_propagation();
                        this.popup = None;
                        this.clear_selection();
                        cx.notify();
                    }),
                );
        }
        let key = (g.chapter, g.para, g.offset);
        let marked = self.underlines.contains(&key);
        // 生僻字注音：开启且命中生僻字表时，在字格顶部标读音（不影响正文排布）。
        let rare_py = if self.show_rare && !is_punct(g.ch) {
            self.rare_pinyin(g.ch)
        } else {
            None
        };
        // 专名线 / 书名线：人名、地名画实线，书名画波浪线，贴字左侧、逐字连贯。
        // 标点字不画线条——线在标点处自然断开，绝不穿过标点。
        let mark_line = if is_punct(g.ch) {
            None
        } else {
            mark.map(|mk| {
                lines::mark_line(
                    mk,
                    cell_w,
                    cell_h,
                    fs,
                    match mk {
                        MarkKind::Name => if back { t.ink_soft() } else { t.ink() },
                        MarkKind::Book => t.accent(),
                    },
                )
            })
        };
        // 检索命中：朱砂红衬底，当前那一处更重、其余较淡（用 `mix` 兑出
        // 半透感，不依赖 opacity —— 深色主题下也稳）。
        let hit = if g.kind == GlyphKind::Char {
            self.search_state_at(g.chapter, g.para, g.offset)
        } else {
            None
        };
        let mut cell = div()
            .w(px(cell_w))
            .h(px(cell_h))
            .flex()
            .flex_row()
            .items_center()
            .justify_center();
        if marked || rare_py.is_some() || mark_line.is_some() || hit.is_some() {
            cell = cell.relative();
        }
        if let Some(is_cur) = hit {
            // 朱砂兑纸色：当前命中重一档，其它命中轻一档。
            let k = if is_cur { 0.42 } else { 0.20 };
            let paper = if back { t.paper_back() } else { t.paper() };
            cell = cell.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(cell_w))
                    .h(px(cell_h))
                    .bg(rgb(crate::theme::mix(paper, t.accent(), k))),
            );
        }
        // 专名 / 书名线画在句读与正文之下：先落线，字与标点压其上，
        // 线与字面之间不留缝（刻本里线贴着字，不嵌在字上）。
        if let Some(line) = mark_line {
            cell = cell.child(line);
        }
        if marked {
            // 朱线贯穿划线各字：画在字格内侧（离右边的朱丝栏 4px），
            // 否则会和版框的界行红线重合、看不出「划了线」。
            // 上下相接、不留缝，读起来是一整条线。
            cell = cell.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .right(px(4.0))
                    .w(px(3.0))
                    .h(px(cell_h))
                    .bg(rgb(t.accent())),
            );
            // 起笔处落一个朱点（🔴）：一段划线的第一字才画，
            // 与截图里的「红点 + 朱线」一致，一眼看出这段线从哪起。
            if underline_run_start(key, &self.underlines) {
                let d = (cell_w * 0.32).clamp(5.0, 11.0);
                cell = cell.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .right(px(5.5 - d / 2.0))
                        .w(px(d))
                        .h(px(d))
                        .rounded(px(d / 2.0))
                        .bg(rgb(t.accent())),
                );
            }
        }
        // 正文墨色：带注音的字略调暗（提示「此处有信息」），其余用常规墨色。
        let base_ink = if back { t.ink_soft() } else { self.palette.ink(t) };
        let ink = if rare_py.is_some() {
            crate::theme::mix(
                base_ink,
                if back { t.paper_back() } else { t.paper() },
                0.22,
            )
        } else {
            base_ink
        };

        // ── 竖排注音（生僻字拼音）──────────────────────────────────
        // 一字母一行、贴字格右上角；字号 30%，长拼音自动降号（下限 18%）。
        // 横向排会撑破一格宽、压到左边下一列——这正是必须修的病根。
        if let Some(py) = rare_py {
            let rl = crate::ruby::layout(py, fs, cell_w, cell_h);
            let mut ruby = div()
                .absolute()
                .top(px(rl.top))
                .right(px(0.0))
                .w(px(rl.block_w))
                .flex()
                .flex_col()
                .items_center();
            for ch in py.chars() {
                ruby = ruby.child(
                    div()
                        .w_full()
                        .h(px(rl.line_h))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(rl.fs))
                        .text_color(rgb(self.palette.note(t)))
                        .child(SharedString::from(ch.to_string())),
                );
            }
            cell = cell.child(ruby);
            // 极细「虚」线：三点短横（GPUI 0.2.2 无 dashed 边框），
            // 贴字格下缘右侧，只作提示、不抢正文。
            let dots = 3usize;
            let dw = (cell_w * 0.34) / (dots as f32 * 1.9);
            let mut hint = div()
                .absolute()
                .bottom(px(cell_h * 0.055))
                .right(px(cell_w * 0.10))
                .flex()
                .flex_row()
                .gap(px(dw * 0.9));
            for _ in 0..dots {
                hint = hint
                    .child(div().w(px(dw)).h(px(1.0)).bg(rgb(self.palette.note(t))));
            }
            cell = cell.child(hint);
        }
        if let Some((txt, color)) = self.annotation_of(g) {
            cell = cell.child(
                div()
                    .text_size(px((fs * 0.34).max(7.0)))
                    .text_color(rgb(if back { t.ink_soft() } else { color }))
                    .child(SharedString::from(txt)),
            );
        }
        if let Some(vp) = crate::west::vertical_paren(g.ch) {
            // 括号换竖排专用字形 ︵ / ︶：本身已横躺，在列中上下相接，
            // 贴合汉字基线（不再用右下角小字的句读摆法）。
            let pf = (fs * 0.88).max(8.0).min(cell_h);
            cell = cell.child(
                div()
                    .text_size(px(pf))
                    .text_color(rgb(ink))
                    .child(SharedString::from(vp.to_string())),
            );
        } else if is_punct(g.ch) {
            // 句读（标点）单独绘制：用「相对坐标」置在字格右下角，距右下
            // 各 `fontSize*0.3`；字号随字号缩放但被字格收敛，行高压小时
            // 也不会溢出到下一个字格（避头尾 / 拥挤问题的根因）。
            let pf = (fs * 0.42).max(8.0).min(cell_h);
            let off = (fs * 0.3).max(2.0);
            cell = cell.relative().child(
                div()
                    .absolute()
                    .left(px(off))
                    .top(px(off))
                    .w(px((cell_w - 2.0 * off).max(off)))
                    .h(px((cell_h - 2.0 * off).max(off)))
                    .flex()
                    .items_end()
                    .justify_end()
                    .text_size(px(pf))
                    .text_color(rgb(t.accent()))
                    .child(SharedString::from(self.shown(g.ch).to_string())),
            );
        } else if crate::west::is_west_hyphen(g.ch) {
            // 单独出现的连字符：竖排里旋转成一道竖线（丨）。
            cell = cell.child(
                div()
                    .text_size(px((fs * 0.92).min(cell_h)))
                    .text_color(rgb(ink))
                    .child(SharedString::from("丨")),
            );
        } else {
            // 数字转汉字（可选）：`1628` → `一六二八`，保持古籍的纯粹感。
            let shown = self.shown(g.ch).to_string();
            let shown = if self.han_digits {
                crate::west::digits_to_han(&shown, false)
            } else {
                shown
            };
            cell = cell.child(
                div()
                    .text_size(px(fs))
                    .text_color(rgb(ink))
                    .child(SharedString::from(shown)),
            );
        }
        // 脚注模式下补圈号角标。
        if !self.notes_inline && self.show_notes {
            if let Some(n) = note_index.get(&(g.chapter, g.para, g.offset)) {
                cell = cell.child(
                    div()
                        .text_size(px((fs * 0.3).max(6.5)))
                        .text_color(rgb(t.accent()))
                        .child(SharedString::from(circled(*n))),
                );
            }
        }
        // 选区高亮：半透明朱底，逐字连成一条（仅正文选中的字才画）。
        if g.kind == GlyphKind::Char && !is_punct(g.ch) && self.in_selection(key) {
            cell = cell.relative().child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(cell_w))
                    .h(px(cell_h))
                    .bg(rgba(0xA63A2B2E)),
            );
        }

        // 拖拽选字：按下起锚、拖动扩展选区（竖排连续选字）、松开决定动作——
        // 单击弹工具条作用于单字；拖选后工具条的「划线 / 复制」作用于整段选区。
        // 关键修复：任何字格（含标点 / 缩进 / 空格）按下都算「碰过正文」，
        // 杜绝「竖排划线 / 点字时误翻下一页」。
        // 晒本预览（interactive=false）不挂任何监听：纯展示，点它不影响阅读状态。
        if interactive {
            cell = cell.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                    // 任意字格按下都先标记「碰过正文」，拦下翻页。
                    this.touched_glyph = true;
                    cx.stop_propagation();
                    if g.kind == GlyphKind::Char && !is_punct(g.ch) {
                        // 正文可点击字：起锚、进入拖选、收起旧工具条。
                        this.sel_anchor = Some(key);
                        this.sel_focus = Some(key);
                        this.dragging = true;
                        this.sel_char = Some(g.ch);
                        this.popup = None;
                    } else {
                        // 标点 / 缩进 / 空格：碰过正文，绝不翻页；并收起既有工具条 / 选区。
                        this.popup = None;
                        this.clear_selection();
                    }
                    cx.notify();
                }),
            );

            if g.kind == GlyphKind::Char && !is_punct(g.ch) {
                cell = cell
                    .on_mouse_move(cx.listener(
                        move |this, _event: &MouseMoveEvent, _window, cx| {
                            if this.dragging && this.sel_focus != Some(key) {
                                this.sel_focus = Some(key);
                                this.sel_char = Some(g.ch);
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseUpEvent, _window, cx| {
                            if this.sel_anchor.is_some() {
                                this.dragging = false;
                                this.sel_focus = Some(key);
                                // 单击（未拖选）带注音的字：直接呼出「注音浮签」，
                                // 大字显示拼音与本字；拖选仍走原来的选区工具条。
                                if this.sel_anchor == this.sel_focus {
                                    if let Some(py) = rare_py {
                                        this.popup = None;
                                        this.note_zoom = Some(crate::gloss::NoteZoom {
                                            text: g.ch.to_string(),
                                            pinyin: Some(py.to_string()),
                                            x: event.position.x.to_f64() as f32,
                                            y: event.position.y.to_f64() as f32,
                                            page: this.page,
                                        });
                                        cx.stop_propagation();
                                        cx.notify();
                                        return;
                                    }
                                }
                                this.popup = Some(GlyphHit {
                                    key,
                                    ch: g.ch,
                                    text: if this.has_selection() {
                                        this.selected_text()
                                    } else {
                                        g.ch.to_string()
                                    },
                                    x: event.position.x.to_f64() as f32,
                                    y: event.position.y.to_f64() as f32,
                                });
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseUpEvent, _window, cx| {
                            // 拖到字格外松开：结束拖拽，保留已选选区。
                            if this.dragging {
                                this.dragging = false;
                                cx.notify();
                            }
                        }),
                    );
            }
        }
        cell
    }



    /// 收集本面出现的注释（保持全局序号）。
    fn page_footnotes(
        &self,
        glyphs: &[Glyph],
        note_index: &NoteIndex,
    ) -> Vec<(usize, String)> {
        if !self.show_notes {
            return Vec::new();
        }
        let Some(book) = &self.book else {
            return Vec::new();
        };
        let mut out: Vec<(usize, String)> = Vec::new();
        let mut seen: Vec<usize> = Vec::new();
        for g in glyphs {
            if g.is_indent() {
                continue;
            }
            let Some(ch) = book.chapters.get(g.chapter) else {
                continue;
            };
            let Some(p) = ch.paragraphs.get(g.para) else {
                continue;
            };
            for note in &p.notes {
                if note.at != g.offset || note.kind != NoteKind::Note {
                    continue;
                }
                if let Some(n) = note_index.get(&(g.chapter, g.para, g.offset)) {
                    if !seen.contains(n) {
                        seen.push(*n);
                        out.push((*n, note.text.clone()));
                    }
                }
            }
        }
        out.sort_by_key(|(n, _)| *n);
        out
    }


    /// 取某个字的标注：(文字, 颜色)。仅通假字标注（蓝色）。
    /// 原「每字红色繁体小注」已删除：正文字体本身的繁简由「繁/简」
    /// 开关统一切换（见 `shown`），不再逐字挂红色繁体小字，避免重复。
    fn annotation_of(&self, g: Glyph) -> Option<(String, u32)> {
        if g.is_indent() {
            return None;
        }
        if self.show_tongjia {
            if let Some(book) = &self.book {
                if let Some(tj) = trad::tongjia_of(g.ch, &book.tongjia) {
                    return Some((tj.to_string(), self.palette.gloss(self.theme)));
                }
            }
        }
        None
    }

    /// 按当前繁简开关把单字转成展示形态：繁体模式转繁，简体模式转简。
    /// 正文与小字注释据此「全部」统一为繁体或简体。
    fn shown(&self, c: char) -> char {
        // 【】→「」先做，再按繁简开关转字（竖排古籍里没有方框括号的位置）。
        let c = crate::types::bracket_to_quote(c);
        if self.show_traditional {
            trad::traditional_of(c).unwrap_or(c)
        } else {
            trad::simplified_of(c).unwrap_or(c)
        }
    }

    /// 生僻字注音：命中内置生僻字表时返回读音；会尝试该字的简/繁变体以兼容不同书写。
    fn rare_pinyin(&self, c: char) -> Option<&'static str> {
        dict::pinyin_of(c)
            .or_else(|| trad::simplified_of(c).and_then(dict::pinyin_of))
            .or_else(|| trad::traditional_of(c).and_then(dict::pinyin_of))
    }

    fn render_reading_status(&self, total: usize) -> Div {
        let t = self.theme;
        let unit = match self.script {
            Script::Ancient => "叶",
            Script::Modern => "页",
        };
        // 列数随「版式」面板切换（8 / 10 / 12 / 14），状态栏同步显示。
        let cols_label = match self.grid.columns {
            8 => "八列".to_string(),
            10 => "十列".to_string(),
            12 => "十二列".to_string(),
            14 => "十四列".to_string(),
            n => format!("{n} 列"),
        };
        let mode = match self.script {
            Script::Ancient => {
                if self.notes_inline && self.show_notes {
                    format!("古文竖笺 · {cols_label} · 双行小字夹注 · 从右到左")
                } else {
                    format!("古文竖笺 · {cols_label} · 四周双边 · 从右到左")
                }
            }
            Script::Modern => "现代横笺 · 仅横线".to_string(),
        };
        let duplex = match self.duplex {
            Duplex::Single => "单面",
            Duplex::Facing => "双面对开",
        };
        let label = self
            .book
            .as_ref()
            .map(|b| b.title.clone())
            .unwrap_or_default();
        let left = format!("第 {} {} / 共 {} {}", self.page + 1, unit, total, unit);
        let mut right = format!(
            "{label} · {mode} · {duplex} · {} {}pt · {}{} · 点右半屏/→翻下页 · 点左半屏/←翻上页",
            self.current_font().label(),
            self.current_size() as i32,
            t.label(),
            if self.show_tongjia || self.show_notes {
                " · 标注开"
            } else {
                " · 标注关"
            },
        );
        if self.show_rare {
            right.push_str(&format!(" · 生僻字{}", crate::dict::rare_len()));
        }
        if !self.show_punct {
            right.push_str(" · 标点隐");
        }
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_2p5()
            .bg(rgb(t.chrome()))
            .border_t_1()
            .border_color(rgb(t.chrome_line()))
            .text_size(px(11.5))
            .text_color(rgb(t.chrome_text()))
            .child(SharedString::from(left))
            .child(SharedString::from(right))
    }

    /// 把草稿按每行字符数折行，并标出光标落在哪一行、行内第几个字。
    /// 返回 (行文字, 光标行内位置——只有光标所在行带 Some)。
    /// 空草稿返回一行空串且光标在行首。
    fn draft_lines_with_cursor(draft: &str, cursor: usize, per: usize) -> Vec<(String, Option<usize>)> {
        let per = per.max(1);
        let chars: Vec<char> = draft.chars().collect();
        let cursor = cursor.min(chars.len());
        if chars.is_empty() {
            return vec![(String::new(), Some(0))];
        }
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < chars.len() {
            let end = (i + per).min(chars.len());
            let line: String = chars[i..end].iter().collect();
            let cur = (cursor >= i && cursor <= end).then(|| cursor - i);
            out.push((line, cur));
            i = end;
        }
        out
    }

    /// 浮层笔记框：盖在正文上的一张小卡片——标题、草稿（带光标）、保存 / 取消。
    /// 锚点取点击处（没有则视口居中偏上），并夹进视口；纸底半透明，
    /// 底下正文若隐若现，写笔记的同时不影响阅读。
    fn render_note_editor(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let page = self.page;
        let draft = self.note_draft.clone();
        let cursor = self.note_cursor;

        let w = 336.0f32;
        let h = 212.0f32;
        // 锚点：优先点击处，否则视口居中偏上；贴边时翻到另一侧，永不出界。
        let (ax, ay) = self.note_pos.unwrap_or((vw * 0.5, vh * 0.22));
        let mut left = ax + 12.0;
        if left + w > vw - 10.0 {
            left = (ax - w - 12.0).max(10.0);
        }
        let mut top = ay + 18.0;
        if top + h > vh - 10.0 {
            top = (ay - h - 18.0).max(10.0);
        }

        // 草稿区：每行 16 字；光标所在行在字间插一道朱色光标块。
        let mut body = div().flex().flex_col().gap_1().min_h(px(100.0));
        if draft.is_empty() {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("在此键入…（⏎ 保存 · Esc 取消）")),
            );
        }
        for (line, cur) in Self::draft_lines_with_cursor(&draft, cursor, 16) {
            let mut row = div().flex().flex_row().items_center().h(px(19.0));
            let head: String = line.chars().take(cur.unwrap_or(0)).collect();
            let tail: String = line.chars().skip(cur.unwrap_or(line.chars().count())).collect();
            if !head.is_empty() {
                row = row.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(t.ink()))
                        .child(SharedString::from(head)),
                );
            }
            if cur.is_some() {
                row = row.child(
                    div()
                        .w(px(1.5))
                        .h(px(14.0))
                        .mx(px(0.5))
                        .bg(rgb(t.accent())),
                );
            }
            if !tail.is_empty() {
                row = row.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(t.ink()))
                        .child(SharedString::from(tail)),
                );
            }
            body = body.child(row);
        }

        let save = small_button("⏎ 保存", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.commit_note_edit();
                cx.notify();
            }),
        );
        let cancel = small_button("取消", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.cancel_note_edit();
                cx.notify();
            }),
        );

        let card = div()
            .w(px(w))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            // 半透明纸底：盖在文本上，正文仍可透见，不影响阅读。
            .bg(rgba(0xF7F1E3F2))
            .border_2()
            .border_color(rgb(t.accent()))
            .rounded(px(6.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(t.accent()))
                            .child(SharedString::from(format!("第 {} 葉 · 笔记", page + 1))),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from(format!(
                                "{} 字",
                                draft.chars().count()
                            ))),
                    ),
            )
            .child(
                // 草稿区外套一层「可聚焦 + 可接 IME」的元素：
                // 中文输入法不走 keydown，全靠它把组好的字送回 note_draft。
                // 少了这层，笔记框只能打英文——切中文就打不出字。
                crate::ime::InputField::new(
                    div()
                        .flex_1()
                        .p_2()
                        .bg(rgba(0xFFFFFFB8))
                        .border_1()
                        .border_color(rgb(t.rule()))
                        .rounded(px(4.0))
                        .child(body),
                    self.note_focus.clone(),
                    cx.entity(),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from("Esc 取消 · ⌘V 粘贴中文 · ←→ 移光标")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(cancel)
                            .child(save),
                    ),
            )
            // 卡片内任何点击都不外传：不会误触翻页 / 收起工具条。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            );

        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .child(card)
    }

    /// 长按正文弹出的**古风字义浮签**（竖排）。
    ///
    /// 一张「贴在书页边上的小纸条」：宣纸底、朱砂红双线细边框、极细微
    /// 阴影；里头**一律竖排**——第一列大字汉字（宋体）、第二列拼音
    /// （竖排小字，逐字母叠下、走西文字体）、后面几列释义。
    ///
    /// 四样信息一步给足：
    ///   * **charIndex** —— 被查字的 `(章, 段, 字)` 与全书统一字符下标，
    ///     由长按的**坐标反查**得来（`glyph_hit_at`），落在卡片脚注；
    ///   * **选中文本** —— 拖选后长按取选区全文，单字长按就退回这一个字；
    ///   * **本地释义** —— 先成词后单字（`dict::lookup`）：「长安」出一条
    ///     词条，而不是「长」+「安」两条；带拼音与词性；
    ///   * **考据原典** —— 拼 `https://www.zdic.net/hans/{字}` 交系统浏览器。
    ///
    /// 位置由 [`crate::hit::place_card`] 算出：**绝不压住被查的那个字**
    /// （字在屏幕右侧则向左展开、字在底部则向上展开）。
    fn render_lookup_card(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let Some(hit) = self.bubble.clone() else {
            return div();
        };
        let vw = self.viewport_w;
        let vh = self.viewport_h;

        // ── 词条：先成词（长安），再退单字 ─────────────────────────
        let entry = crate::dict::lookup(&hit.text).or_else(|| crate::dict::entry_of(hit.ch));
        let word = entry
            .as_ref()
            .map(|e| e.word.clone())
            .unwrap_or_else(|| hit.ch.to_string());
        let pinyin = entry.as_ref().map(|e| e.pinyin.clone()).unwrap_or_default();
        let pos = entry.as_ref().map(|e| e.pos.clone()).unwrap_or_default();
        let gloss = entry.as_ref().map(|e| e.gloss.clone()).unwrap_or_default();

        // ── 竖排度量：宽高按释义长度与视口反算（绝不撑破屏幕）────────
        let m = crate::hit::card_metrics(gloss.chars().count(), vw, vh);

        // ── 智能避让：绕开被查的那个字 ──────────────────────────────
        // `lookup_rect` 是坐标反查量出的**字的屏幕矩形**；没走反查（元素
        // 命中那一路）就以落点为中心造一个字格，避让照旧成立。
        let anchor = match self.lookup_rect {
            Some((x, y, w, h)) => crate::hit::Anchor { x, y, w, h },
            None => crate::hit::Anchor {
                x: hit.x - self.col_w * 0.5,
                y: hit.y - self.row_h * 0.5,
                w: self.col_w,
                h: self.row_h,
            },
        };
        let place = crate::hit::place_card(
            &anchor,
            m.w,
            m.h,
            vw,
            vh,
            crate::hit::CARD_PAD,
            crate::hit::CARD_GAP,
        );

        // 字体：正文（字头 / 释义）与西文（拼音）各按角色取，不写字族名。
        let main_family = self.main_family();
        let latin_family = crate::fonts::family_for_role(crate::fonts::FontRole::Latin);
        // 内容区高：扣双线框 + 内边距 + 页眉一行 + 页脚两行（与 `card_metrics`
        // 预留的 56 同源）。
        let inner_h = (m.h - 56.0).max(60.0);
        // 竖排的**行距由格子高显式给死**（不是靠字体行高）：每字一个
        // `line_h` 高的格子，`per_col` 个字正好铺满 `inner_h`——字体行高
        // 偏大偏小都不会溢出卡片，也不会在底部留一截空白。
        let line_h = ((inner_h - 2.0) / m.per_col.max(1) as f32).clamp(10.0, 22.0);
        let gloss_fs = m.gloss_fs.min(line_h * 0.92);

        // ── 第一列：大字汉字（宋体，每字一格，朱线细框）─────────────
        let head_fs = 30.0f32.min(m.w * 0.16);
        let mut head_col = div()
            .w(px(40.0))
            .h(px(inner_h))
            .flex()
            .flex_col()
            .items_center()
            .justify_start()
            .gap(px(2.0));
        for c in word.chars().take(4) {
            head_col = head_col.child(
                div()
                    .w(px(34.0))
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(rgb(t.accent()))
                    .rounded(px(2.0))
                    .font_family(main_family.clone())
                    .text_size(px(head_fs))
                    .text_color(rgb(t.ink()))
                    .child(SharedString::from(c.to_string())),
            );
        }

        // ── 第二列：拼音（竖排小字，逐字母叠下，走西文字体）──────────
        let py_letters: Vec<char> = pinyin
            .chars()
            .filter(|c| !c.is_whitespace())
            .take(12)
            .collect();
        // 拼音同样给死行距：字母多就压紧（最小 9），少就 12——绝不溢出。
        let py_line = if py_letters.is_empty() {
            12.0
        } else {
            ((inner_h - 2.0) / py_letters.len() as f32).clamp(9.0, 12.0)
        };
        let mut py_col = div()
            .w(px(18.0))
            .h(px(inner_h))
            .flex()
            .flex_col()
            .items_center()
            .justify_start();
        if py_letters.is_empty() {
            py_col = py_col.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("—")),
            );
        } else {
            for c in py_letters {
                py_col = py_col.child(
                    div()
                        .h(px(py_line))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(latin_family.clone())
                        .text_size(px((py_line * 0.92).min(11.5)))
                        .text_color(rgb(t.accent()))
                        .child(SharedString::from(c.to_string())),
                );
            }
        }

        // ── 后面几列：释义（竖排小字，自右向左分列）─────────────────
        // 先切列再倒序 push：首列落在最右（紧邻拼音列），读序自右向左。
        let mut chunks: Vec<String> = Vec::new();
        let mut rest: Vec<char> = gloss.chars().collect();
        if rest.is_empty() {
            chunks.push("（本地字书未收，可点「考据原典」上汉典）".to_string());
        }
        while !rest.is_empty() && chunks.len() < m.cols.max(1) {
            let n = rest.len().min(m.per_col);
            let s: String = rest.drain(..n).collect();
            chunks.push(s);
        }
        if !rest.is_empty() {
            // 列数用尽还有剩：末列补省略号（不撑高卡片）。
            if let Some(last) = chunks.last_mut() {
                last.push('…');
            }
        }
        let mut cols = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(1.0))
            .h(px(inner_h));
        for s in chunks.iter().rev() {
            let mut c = div()
                .w(px(m.gloss_fs * 1.5))
                .flex()
                .flex_col()
                .items_center();
            for ch in s.chars() {
                c = c.child(
                    div()
                        .h(px(line_h))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(main_family.clone())
                        .text_size(px(gloss_fs))
                        .text_color(rgb(t.ink_soft()))
                        .child(SharedString::from(ch.to_string())),
                );
            }
            cols = cols.child(c);
        }

        // ── 脚注：charIndex ────────────────────────────────────────
        let (ci, pi, oi) = hit.key;
        // 脚注写短一点：卡片窄，长串会挤掉右侧按钮（这里只留「哪一段哪一字」）。
        let mut idx_line = format!("第{}章·{}段·{}字", ci + 1, pi + 1, oi + 1);
        if let Some(f) = self.lookup_flat_index() {
            idx_line.push_str(&format!(" · 全书{f}"));
        }
        let foot = div()
            .flex_1()
            .overflow_hidden()
            .text_size(px(9.0))
            .text_color(rgb(t.ink_soft()))
            .child(SharedString::from(idx_line));

        // ── 底部：考据原典 / 复制 / 检索 / 关闭 ────────────────────
        let zdic = crate::dict::zdic_url(&word);
        let zdic_word = word.clone();
        let btn_zdic = div()
            .px_2p5()
            .py_1()
            .border_1()
            .border_color(rgb(t.accent()))
            .rounded(px(2.0))
            .text_size(px(11.5))
            .text_color(rgb(t.accent()))
            .cursor_pointer()
            .child(SharedString::from("考據原典"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if crate::app::open_external(&zdic) {
                        this.notice = format!("已用浏览器打开汉典：{zdic_word}");
                    } else {
                        this.notice = "未能调用系统浏览器".to_string();
                    }
                    cx.notify();
                }),
            );
        let copy_key = hit.key;
        let copy_text = hit.text.clone();
        let btn_copy = small_button("复制", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                // 长按单字时没有选区：临时把这一个字设为选区再复制，
                // 免得退化成「复制整页」。
                if !this.has_selection() {
                    this.sel_anchor = Some(copy_key);
                    this.sel_focus = Some(copy_key);
                }
                this.copy_page_text();
                this.clear_selection();
                this.bubble = None;
                this.close_lookup();
                cx.notify();
            }),
        );
        let search_text = copy_text.clone();
        let btn_search = small_button("检索", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let text = if search_text.is_empty() {
                    this.selected_text()
                } else {
                    search_text.clone()
                };
                this.search_for(&text, cx);
                this.bubble = None;
                this.close_lookup();
                cx.notify();
            }),
        );
        let btn_close = small_button("收", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.bubble = None;
                this.close_lookup();
                cx.notify();
            }),
        );

        let body = div()
            .w_full()
            .flex_1()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(4.0))
            .overflow_hidden()
            .child(cols)
            .child(py_col)
            .child(head_col);
        // 页脚分两行：上行 charIndex（可截断），下行按钮右对齐——
        // 挤成一行时脚注会把「考據原典」压扁，窄卡片上尤其难看。
        let bar = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(foot)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(btn_zdic)
                    .child(btn_copy)
                    .child(btn_search)
                    .child(btn_close),
            );

        // ── 纸签：宣纸底 + 朱砂双线细框 + 细微阴影 ──────────────────
        // 外层一根朱线、内层再一根淡朱线（中间留 3px 纸色），就是刻本
        // 牌记那种「双边框」；不用系统弹窗，是叠在正文之上的一层 div。
        let inner_line = crate::theme::mix(t.accent(), t.paper(), 0.62);
        div()
            .absolute()
            .left(px(place.left))
            .top(px(place.top))
            .w(px(m.w))
            .h(px(m.h))
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.accent()))
            .rounded(px(3.0))
            .shadow_lg()
            .p(px(3.0))
            .child(
                div()
                    .w_full()
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_1()
                    .border_color(rgb(inner_line))
                    .rounded(px(2.0))
                    .p_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(9.5))
                                    .text_color(rgb(t.ink_soft()))
                                    .child(SharedString::from(format!("字書 · {pos}"))),
                            )
                            .child(
                                div()
                                    .text_size(px(9.5))
                                    .text_color(rgb(t.ink_soft()))
                                    .child(SharedString::from(hit.text.clone())),
                            ),
                    )
                    .child(body)
                    .child(bar),
            )
            // 点浮签内部不让它冒到根容器的「点别处收起」监听上去，
            // 否则点一下「考据原典」浮签就没了。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
    }

    /// 全文检索浮层：输入一个词，全书瞬时查找，跳到那一面并用朱砂红高亮。
    ///
    /// 输入框套 `ime::InputField` —— 中文输入法不走 keydown，
    /// 只有挂了 InputHandler 才能把组好的「长安」送回草稿。
    fn render_search_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let q = self.search_draft.trim().to_string();
        let n = self.search_hits.len();
        let label = if q.is_empty() {
            "输入字词后回车检索".to_string()
        } else if n == 0 {
            format!("未找到「{q}」")
        } else {
            format!("「{q}」共 {n} 处 · 第 {}/{n}", self.search_idx + 1)
        };

        // 输入框：草稿按光标切成两段，中间立一根朱色光标。
        let draft = self.search_draft.clone();
        let cursor = self.search_cursor.min(draft.chars().count());
        let before: String = draft.chars().take(cursor).collect();
        let after: String = draft.chars().skip(cursor).collect();
        let box_body = div()
            .flex()
            .flex_row()
            .items_center()
            .text_size(px(14.0))
            .text_color(rgb(t.ink()))
            .child(SharedString::from(before))
            .child(div().w(px(1.5)).h(px(16.0)).bg(rgb(t.accent())))
            .child(SharedString::from(after));

        let input = crate::ime::InputField::new(
            div()
                .w(px(200.0))
                .p_2()
                .bg(rgb(t.paper()))
                .border_1()
                .border_color(rgb(t.frame()))
                .rounded(px(3.0))
                .child(box_body),
            self.search_focus.clone(),
            cx.entity(),
        );

        let btn_go = small_button("检索", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.run_search(cx)),
        );
        let btn_prev = small_button("‹ 上一处", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.search_prev(cx)),
        );
        let btn_next = small_button("下一处 ›", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.search_next(cx)),
        );
        let btn_close = small_button("关闭", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.show_search = false;
                this.clear_search();
                cx.notify();
            }),
        );

        div()
            .absolute()
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(42.0))
            .flex()
            .flex_row()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .bg(rgb(t.chrome()))
                    .border_1()
                    .border_color(rgb(t.frame()))
                    .rounded(px(4.0))
                    .child(input)
                    .child(btn_go)
                    .child(btn_prev)
                    .child(btn_next)
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from(label)),
                    )
                    .child(btn_close),
            )
    }

    /// 点字查字浮层：所点字用大字立在朱印旁，下附 charIndex、选中文本、
    /// 释义、繁体、通假，底部一枚「在线查阅」（汉典）。
    fn render_dict_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let ch = self.lookup.clone().unwrap_or_default();
        // 「在线查阅」的按钮闭包是 `move`，需要自己持有一份字串。
        let ch_for_online = ch.clone();
        let c = ch.chars().next().unwrap_or(' ');
        let book = self.book.as_ref();
        let trad = trad::traditional_of(c);
        // 词库键为繁体：先按原字查，未命中再按繁 / 简变体兜底——
        // 无论书稿是简体还是繁体、繁简开关怎么切，点字都能查到释义。
        let def = crate::dict::define(&ch)
            .or_else(|| trad.and_then(|t| crate::dict::define(&t.to_string())))
            .or_else(|| {
                trad::simplified_of(c).and_then(|s| crate::dict::define(&s.to_string()))
            });
        let tong = book
            .and_then(|b| trad::tongjia_of(c, &b.tongjia))
            .map(|t| t.to_string());

        let mut body = div().flex().flex_col().gap_2();
        // 字头：大字 + 朱框，像一枚「字印」。
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(56.0))
                        .h(px(56.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(rgb(t.accent()))
                        .rounded(px(2.0))
                        .text_size(px(36.0))
                        .text_color(rgb(t.ink()))
                        .child(SharedString::from(ch.clone())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(t.accent()))
                                .child(SharedString::from("字書")),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(t.ink_soft()))
                                .child(SharedString::from("点字查义 · 通假对照")),
                        ),
                ),
        );
        // charIndex：点「查询」时一并记下的 (章, 段, 字) 与全书统一字符下标。
        if let Some((ci, pi, oi)) = self.lookup_key {
            body = body.child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from(format!(
                        "第 {} 章 · 第 {} 段 · 第 {} 字",
                        ci + 1,
                        pi + 1,
                        oi + 1
                    ))),
            );
            if let Some(f) = self.lookup_flat_index() {
                body = body.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(t.accent()))
                        .child(SharedString::from(format!("全书第 {f} 字"))),
                );
            }
        }
        // 选中文本：拖选后点「查询」时，把选的哪一段也标出来。
        if self.lookup_text.chars().count() > 1 {
            body = body.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(t.indigo()))
                    .child(SharedString::from(format!("选中：{}", self.lookup_text))),
            );
        }
        if let Some(d) = def {
            // 释义按行拆开逐行渲染（GPUI 的 SharedString 不解析 \n）；
            // 整部《古漢語常用字字典》一条释义可能上千字，放进可滚动区，
            // 免把查字面板撑出屏幕。
            let mut col = div().flex().flex_col().gap_1();
            for line in d.split('\n') {
                if line.is_empty() {
                    continue;
                }
                col = col.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(t.ink_soft()))
                        .child(SharedString::from(line.to_string())),
                );
            }
            let mut holder = div()
                .max_h(px((self.viewport_h * 0.58).max(220.0)))
                .child(col);
            holder.style().overflow.y = Some(Overflow::Scroll);
            body = body.child(holder);
        } else {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("（字典未收此字，换个字点点看）")),
            );
        }
        if let Some(tr) = trad {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.accent()))
                    .child(SharedString::from(format!("繁体：{tr}"))),
            );
        }
        if let Some(tj) = tong {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.indigo()))
                    .child(SharedString::from(format!("通假：{tj}"))),
            );
        }

        div()
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .w(px(272.0))
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.frame()))
            .rounded(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from("字书 · 点字查义")),
                    )
                    .child(small_button("关闭", false, t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_lookup();
                            cx.notify();
                        }),
                    )),
            )
            .child(body)
            // 在线查阅：拼汉典地址交系统浏览器，与长按浮签同一个动作。
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(small_button("在线查阅", true, t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let url = crate::dict::zdic_url(&ch_for_online);
                            let ch2 = ch_for_online.clone();
                            if crate::app::open_external(&url) {
                                this.notice = format!("已用浏览器打开汉典：{ch2}");
                            } else {
                                this.notice = "未能调用系统浏览器".to_string();
                            }
                            cx.notify();
                        }),
                    )),
            )
    }

    /// 样式浮层：纸色、字级、版式、注文排法。
    /// 笔记 / 批注浮层：列出全书笔记，并可在当前页写笔记。
    /// 竖排工具框：查询 / 划线 / 笔记 / 复制 / 分享（参照 03 图）。
    /// 点字弹出的工具条：在点击位置附近浮起一行 查询 / 划线 / 笔记 / 复制 / 分享。
    /// 不再常驻右上角，只有点了正文某个字才出现；点别处即收起。
    /// 每个工具都作用于被点击的那个字。
    fn render_glyph_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let Some(hit) = self.popup.clone() else {
            return div();
        };
        let vw = self.viewport_w;
        let vh = self.viewport_h;

        // 工具条尺寸估算，用于把浮条夹在视口内。按钮数必须与下面实际排布
        // 一致（查询 / 划线 / 人名 / 地名 / 书名 / 夹注 / 笔记 / 复制 / 分享，
        // 共 9 个）：早先按 5 个估算，宽度少算 150+px，靠右时右半截（含
        // 「笔记 / 复制 / 分享」）被裁到屏幕外——既显示不全，也点不到。
        let n: f32 = 9.0;
        let bw = 46.0;
        let gap = 6.0;
        let pad = 8.0;
        let menu_w = pad * 2.0 + bw * n + gap * (n - 1.0); // ≈ 426
        let menu_h = 38.0;
        let avail = (vw - 16.0).max(120.0);
        // 窄窗放不下一整排时折行，保证每个按钮都留在屏内。
        let wraps = menu_w > avail;
        let est_h = if wraps { menu_h * 2.0 + 4.0 } else { menu_h };

        // 落在选区「左下方」：先按右缘对齐命中点向左展开，再夹进视口四边，
        // 保证整条工具条完整可见。
        let mut left = hit.x - menu_w;
        if left < 8.0 {
            left = 8.0;
        }
        let max_left = (vw - menu_w.min(avail) - 8.0).max(8.0);
        if left > max_left {
            left = max_left;
        }
        let mut top = hit.y + 14.0; // 选区正下方
        if top + est_h > vh - 8.0 {
            top = (hit.y - est_h - 14.0).max(8.0); // 下方放不下则翻到上方
        }

        let key = hit.key;
        let ch = hit.ch;

        let btn_query = small_button("查询", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                // 一并记下 charIndex 与选中文本：字书面板上要显示「第几章第几段
                // 第几字」，并在底部给出「在线查阅」。
                let text = if this.has_selection() {
                    this.selected_text()
                } else {
                    ch.to_string()
                };
                this.open_lookup(key, ch, text, None, None);
                this.popup = None;
                this.clear_selection();
                cx.notify();
            }),
        );
        // 划线：拖选了整段则对选区全部字划线（再点取消）；单击单字则只划该字。
        let btn_mark = small_button("划线", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if this.has_selection() {
                    this.underline_selection();
                } else if this.underlines.contains(&key) {
                    this.underlines.remove(&key);
                    this.save_annotations();
                } else {
                    this.underlines.insert(key);
                    this.save_annotations();
                }
                this.popup = None;
                cx.notify();
            }),
        );
        // 标记：拖选整段则对选区全部字加专名线（人名 / 地名）或书名线
        // （波浪线）；单击单字则只标该字。标记写入书稿并回写 JSON，下次
        // 打开仍在（与数据里预置的标记走同一通道，mark_kind 实时读 self.book）。
        let btn_person = small_button("人名", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.mark_selection(NoteKind::Person);
                this.popup = None;
                cx.notify();
            }),
        );
        let btn_place = small_button("地名", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.mark_selection(NoteKind::Place);
                this.popup = None;
                cx.notify();
            }),
        );
        let btn_bookm = small_button("书名", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.mark_selection(NoteKind::Book);
                this.popup = None;
                cx.notify();
            }),
        );
        // 夹注：在**这个字**上加一条双行小字夹注（深朱砂红、可右键改删）。
        // 与「笔记」不同：笔记是页眉批注（整页一条），夹注是嵌在正文列里
        // 的双行小字，且**挂在字形上、不进正文流**——注入后只从当前半叶
        // 起点局部重排（`relayout_from_halfleaf`），绝不重排全书。
        let existing_note = self
            .book
            .as_ref()
            .and_then(|b| {
                b.chapters
                    .get(key.0)?
                    .paragraphs
                    .get(key.1)?
                    .notes
                    .iter()
                    .find(|n| n.at == key.2 && n.kind == NoteKind::Note)
            })
            .map(|n| n.text.clone())
            .unwrap_or_default();
        let btn_gloss = small_button("夹注", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                // 已有夹注则预填（算改写），没有则从空白起。
                this.start_note_inject(key, &existing_note, Some((hit.x, hit.y)));
                cx.notify();
            }),
        );
        // 笔记：在点击处直接浮出笔记框，盖在文本上输入；不另开面板、
        // 不推开正文，Esc 取消 / ⏎ 保存 / 点别处自动保存。
        let btn_note = small_button("笔记", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.start_note_edit_at(Some((hit.x, hit.y)));
                cx.notify();
            }),
        );
        let btn_copy = small_button("复制", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.copy_page_text();
                this.popup = None;
                this.clear_selection();
                cx.notify();
            }),
        );
        let btn_share = small_button("分享", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.show_share_panel = true;
                this.popup = None;
                this.clear_selection();
                cx.notify();
            }),
        );

        div()
            .absolute()
            .top(px(top))
            .left(px(left))
            .flex()
            .flex_row()
            .flex_wrap()
            .max_w(px(avail))
            .items_center()
            .gap(px(gap))
            .p(px(pad))
            .rounded(px(9.0))
            .bg(rgb(t.chrome()))
            .border_1()
            .border_color(rgb(t.frame()))
            .child(btn_query)
            .child(btn_mark)
            .child(btn_person)
            .child(btn_place)
            .child(btn_bookm)
            .child(btn_gloss)
            .child(btn_note)
            .child(btn_copy)
            .child(btn_share)
    }

    /// 注文放大悬浮卡（交互层）：点行内夹注小字后弹出——现代风格半透明
    /// 卡片，把注文放大横排显示，解决手机 / 窄屏上小字看不清的问题。
    /// 定位在点击处附近并夹进视口；点卡片外任意处收起（root 级监听），
    /// 卡片自身 `stop_propagation` 保住内容不被秒关。
    fn render_note_zoom(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let Some(zoom) = self.note_zoom.clone() else {
            return div();
        };
        let vw = self.viewport_w;
        let vh = self.viewport_h;

        // 卡片宽度不超视口留边；正文字号 17px，每行字数按卡宽折。
        let card_w = (vw - 40.0).clamp(220.0, 380.0);
        let pad = 14.0;
        let body_fs = 17.0;
        let line_h = body_fs * 1.75;
        let text_w = card_w - pad * 2.0;
        let per_line = ((text_w / body_fs).floor() as usize).max(8);
        let lines = wrap_lines(&zoom.text, per_line);
        // 注音浮签（`pinyin` 有值）：卡片内容换成「大字拼音 + 本字」，
        // 高度按固定行算（39 拼音 + 36 本字 + 24 释义行）。
        let is_ruby = zoom.pinyin.is_some();
        let head_h = 20.0;
        let foot_h = 16.0;
        let card_h = if is_ruby {
            pad * 2.0 + head_h + 8.0 + 39.0 + 36.0 + 24.0 + foot_h
        } else {
            pad * 2.0 + head_h + 8.0 + lines.len() as f32 * line_h + 6.0 + foot_h
        };

        // 锚点右侧下方优先，贴边时向左 / 向上翻，始终夹在视口内。
        let mut left = zoom.x + 14.0;
        if left + card_w > vw - 8.0 {
            left = (zoom.x - card_w - 14.0).max(8.0);
        }
        let mut top = zoom.y + 18.0;
        if top + card_h > vh - 8.0 {
            top = (zoom.y - card_h - 12.0).max(8.0);
        }
        left = left.min((vw - card_w - 8.0).max(8.0));
        top = top.min((vh - card_h - 8.0).max(8.0));

        let close = div()
            .id("note-zoom-close")
            .w(px(22.0))
            .h(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(11.0))
            .text_size(px(13.0))
            .text_color(rgb(0xFFFFFFB0))
            .cursor_pointer()
            .child(SharedString::from("✕"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.note_zoom = None;
                    cx.notify();
                }),
            );

        // 「重录」＝改写这条夹注：卡片只带文本、不带定位键，
        // 用 `note_key_for_text` 反查它属于书稿里哪条注文，
        // 预填进浮层编辑栏，保存时写回书稿 JSON（下次打开仍在）。
        // 跨列流转的注文在卡里只是整条的一段，反查不到 → 提示不支持。
        let key = self.note_key_for_text(&zoom.text);
        let ztext = zoom.text.clone();
        let zpos = (zoom.x, zoom.y);
        let edit = div()
            .id("note-zoom-edit")
            .w(px(46.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(10.0))
            .text_size(px(11.0))
            .text_color(rgb(0xFFFFFFC0))
            .border_1()
            .border_color(rgba(0xFFFFFF40))
            .cursor_pointer()
            .child(SharedString::from("重录"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    match key {
                        Some(k) => this.start_annotation_edit(k, &ztext, Some(zpos)),
                        None => {
                            this.notice = "该注文跨列流转，暂不支持直接改写".to_string()
                        }
                    }
                    cx.notify();
                }),
            );

        let head = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(3.0))
                            .h(px(12.0))
                            .rounded(px(1.5))
                            .bg(rgb(t.accent())),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(0xFFFFFFC0))
                            .child(SharedString::from(if is_ruby {
                                "注音浮签"
                            } else {
                                "夹注重录"
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    // 注音浮签没有「重录」——拼音来自生僻字表，不可改写。
                    .when(!is_ruby, |d| d.child(edit))
                    .child(close),
            );

        let mut body = div().flex().flex_col().gap(px(2.0));
        if is_ruby {
            // 注音浮签：拼音 26px 大字 + 本字 22px + 释义行（横排，卡片里读横最清楚）。
            let py = zoom.pinyin.clone().unwrap_or_default();
            body = body
                .child(
                    div()
                        .h(px(39.0))
                        .flex()
                        .items_center()
                        .text_size(px(26.0))
                        .text_color(rgb(0xFFFFFFFF))
                        .child(SharedString::from(py)),
                )
                .child(
                    div()
                        .h(px(36.0))
                        .flex()
                        .items_center()
                        .text_size(px(22.0))
                        .text_color(rgb(0xFFFFFFB0))
                        .child(SharedString::from(zoom.text.clone())),
                )
                .child(
                    div()
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .text_size(px(12.0))
                        .text_color(rgb(0xFFFFFF90))
                        .child(SharedString::from("生僻字 · 竖排注音")),
                );
        } else {
            for line in &lines {
                body = body.child(
                    div()
                        .h(px(line_h))
                        .flex()
                        .items_center()
                        .text_size(px(body_fs))
                        .text_color(rgb(0xFFFFFFFF))
                        .child(SharedString::from(line.clone())),
                );
            }
        }

        let foot = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0xFFFFFF90))
                    .child(SharedString::from(if is_ruby {
                        format!("第 {} 葉 · 生字注音", zoom.page + 1)
                    } else {
                        format!("第 {} 葉 · 原注双行小字，此处已放大", zoom.page + 1)
                    })),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0xFFFFFF90))
                    .child(SharedString::from(format!("{} 字", zoom.text.chars().count()))),
            );

        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(card_w))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(pad))
            .rounded(px(14.0))
            // 现代风格半透明：深色玻璃底 + 细亮边 + 投影。
            .bg(rgba(0x121216E8))
            .border_1()
            .border_color(rgba(0xFFFFFF24))
            .shadow_lg()
            // 放大注卡同样走**批注字体**（楷体）：卡片里读的是注文，
            // 与行内夹注小字同一副面孔，放大后仍是「注文的字」。
            .font_family(crate::fonts::note_family(self.note_font()))
            // 点卡片内容不冒泡到 root（否则会被当「点别处」秒关）。
            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                cx.stop_propagation();
            }))
            .child(head)
            .child(body)
            .child(foot)
    }

    /// 右键 / 长按**用户自加夹注**弹出的「修改 / 删除」菜单。
    ///
    /// 只有 `NoteSource::User` 的注文会走到这里（见 `ancient_gloss`）：
    /// 古籍原本的夹注是底本的一部分，不可改删。
    ///   * 修改 → 复用既有浮层编辑栏，预填原文本，保存时写回书稿 JSON；
    ///   * 删除 → 从书稿移除该条注文并回写 JSON；引擎下一帧按新注文集重排，
    ///     释放的半格让后续字上移回流（只重排当前半叶之后，绝不重排全书）。
    fn render_note_menu(&self, cx: &mut Context<Self>) -> Div {
        let Some(menu) = self.note_menu.clone() else {
            return div();
        };
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let card_w = 188.0;
        let row_h = 34.0;
        let card_h = 30.0 + row_h * 2.0 + 8.0;

        // 锚点右下方优先，贴边时向左 / 向上翻，始终夹在视口内。
        let mut left = menu.x + 6.0;
        if left + card_w > vw - 8.0 {
            left = (menu.x - card_w - 6.0).max(8.0);
        }
        let mut top = menu.y + 6.0;
        if top + card_h > vh - 8.0 {
            top = (menu.y - card_h - 6.0).max(8.0);
        }
        left = left.min((vw - card_w - 8.0).max(8.0));
        top = top.min((vh - card_h - 8.0).max(8.0));

        let key = menu.key;
        let mtext = menu.text.clone();
        let mpos = (menu.x, menu.y);

        let mut card = div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(card_w))
            .flex()
            .flex_col()
            .p(px(6.0))
            .gap(px(4.0))
            .rounded(px(10.0))
            .bg(rgba(0x121216F0))
            .border_1()
            .border_color(rgba(0xFFFFFF26))
            .shadow_lg()
            // 点菜单不冒泡到 root（否则会被当「点别处」秒关）。
            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                cx.stop_propagation();
            }))
            .child(
                div()
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .pl(px(4.0))
                    .text_size(px(11.0))
                    .text_color(rgba(0xFFFFFF80))
                    .child(SharedString::from("自加夹注")),
            );

        // 修改：预填原文本进入浮层编辑栏，保存时写回书稿。
        let edit_text = mtext.clone();
        card = card.child(
            div()
                .id("note-menu-edit")
                .w_full()
                .h(px(row_h))
                .flex()
                .items_center()
                .pl(px(10.0))
                .rounded(px(7.0))
                .text_size(px(13.0))
                .text_color(rgb(USER_NOTE_INK))
                .cursor_pointer()
                .child(SharedString::from("修改"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.note_menu = None;
                        this.start_annotation_edit(key, &edit_text, Some(mpos));
                        cx.notify();
                    }),
                ),
        );

        // 删除：数据移除 + 回写 JSON；下一帧按新注文集重排（局部）。
        card = card.child(
            div()
                .id("note-menu-delete")
                .w_full()
                .h(px(row_h))
                .flex()
                .items_center()
                .pl(px(10.0))
                .rounded(px(7.0))
                .text_size(px(13.0))
                .text_color(rgb(USER_NOTE_INK))
                .cursor_pointer()
                .child(SharedString::from("删除"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.note_menu = None;
                        // 删掉的是书稿里那条注文；正文流不变，后续字上移回流。
                        this.delete_user_note_at(key);
                        cx.notify();
                    }),
                ),
        );
        card
    }

    /// 复制当前页正文（或拖选选区的文字）到系统剪贴板。
    fn copy_page_text(&mut self) {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let pages = self.paginate();
        let text: String = if self.has_selection() {
            // 有选区：收集选区内（含双面对开的左右两页）的字。
            let (a, b) = self.selection_range().unwrap_or(((0, 0, 0), (0, 0, 0)));
            let mut out = String::new();
            for pg in [self.page, self.page + 1] {
                if let Some(glyphs) = pages.get(pg) {
                    for g in glyphs {
                        if g.is_indent() {
                            continue;
                        }
                        let k = (g.chapter, g.para, g.offset);
                        if k >= a && k <= b {
                            out.push(self.shown(g.ch));
                        }
                    }
                }
            }
            out
        } else {
            let Some(glyphs) = pages.get(self.page) else {
                self.notice = "没有可复制的内容".to_string();
                return;
            };
            glyphs
                .iter()
                .filter(|g| !g.is_indent())
                .map(|g| self.shown(g.ch))
                .collect()
        };
        if text.is_empty() {
            self.notice = "没有可复制的内容".to_string();
            return;
        }
        match Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
                self.notice = format!("已复制 {} 字", text.chars().count());
            }
            Err(_) => self.notice = "复制失败：找不到 pbcopy".to_string(),
        }
    }

    /// 对拖选选区划线：区间内全部字未划则全划，已全划则全消（支持双面对开跨页）。
    fn underline_selection(&mut self) {
        let Some((a, b)) = self.selection_range() else {
            return;
        };
        let pages = self.paginate();
        let mut keys: Vec<(usize, usize, usize)> = Vec::new();
        for pg in [self.page, self.page + 1] {
            if let Some(glyphs) = pages.get(pg) {
                for g in glyphs {
                    if g.kind != GlyphKind::Char || g.is_indent() {
                        continue;
                    }
                    let k = (g.chapter, g.para, g.offset);
                    if k >= a && k <= b {
                        keys.push(k);
                    }
                }
            }
        }
        if keys.is_empty() {
            self.notice = "选区内没有可划线的字".to_string();
            return;
        }
        let all_marked = keys.iter().all(|k| self.underlines.contains(k));
        for k in keys {
            if all_marked {
                self.underlines.remove(&k);
            } else {
                self.underlines.insert(k);
            }
        }
        self.save_annotations();
        self.clear_selection();
        self.notice = if all_marked {
            "已取消选区划线".to_string()
        } else {
            "已给选中文字划线".to_string()
        };
    }

    /// 分享卡片：深底竖排正文 + 落款朱印（参照 02 图），可存为 PNG。
    /// 晒本占位卡：无书 / 空页 / 切片校验失败时的一块素纸一句提示
    /// （面积固定，控制条照常可用）。
    fn sunben_placeholder(t: Theme) -> Div {
        div()
            .w(px(400.0))
            .h(px(300.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.frame_ink()))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("打开一本书，再来晒本")),
            )
    }

    /// 晒本长图：把**当前叶**的内容（含夹注）按真实版式重排成一张竖长纸。
    ///
    /// 与分享卡其它版式（纯文字卡）根本不同：这里走 `render_ancient`
    /// 全量渲染——版框双边、乌丝栏、竖排正文、双行小字夹注（用户注深朱砂）、
    /// 章首页的章题列与卷首题署、书耳、地脚叶码，一应俱全。
    ///
    /// 三步（内容与阅读视图严格同源，只改几何）：
    ///   1. 从全书字形流里切出当前叶那一段（首字 → 末字，逐字校验）；
    ///   2. `layout_book_from` 以「减一列 × 多行」续排这段——避头尾 /
    ///      夹注跨列流转都由真引擎重算，行数从 4 试到刚好一面收完，
    ///      长图因此比原叶更竖长；
    ///   3. 按卡片可用面积解字格（同时受卡宽 / 卡高约束，地脚随字格变，
    ///      一并代入），以 `LeafGeo` 覆盖几何渲染；预览不可交互、
    ///      夹注强制行内（晒本一律带注）。
    ///
    /// 返回 `(纸面 Div, 纸宽, 纸高)`；无书 / 空页 / 切片校验失败返回 `None`。
    fn sunben_sheet(
        &self,
        box_w: f32,
        box_h: f32,
        cx: &mut Context<Self>,
    ) -> Option<(Div, f32, f32)> {
        let book = self.book.as_ref()?;
        let pages = self.layout_pages();
        let cur = pages.get(self.page)?;
        let first = cur.glyphs.first()?;
        let last = cur.glyphs.last()?;
        let flow = book.to_flow(self.show_punct);
        // ① 切片：当前叶首字在流里的位置（整段逐字校验——段首缩进占位
        // 两两全同，必须连 kind 一起比对才能定准）。
        let i0 = flow.iter().position(|g| same_glyph(g, first))?;
        let n = cur.glyphs.len();
        if i0 + n > flow.len() {
            return None;
        }
        if !cur
            .glyphs
            .iter()
            .enumerate()
            .all(|(k, g)| same_glyph(&flow[i0 + k], g))
        {
            return None;
        }
        let slice = &flow[i0..i0 + n];

        // ② 续排：列数减一（长图更竖）；信笺保持 14 列（中缝列照旧）。
        //    夹注经 gloss 闭包行内化——晒本一律带注，与 notes_inline 无关，
        //    但尊重「显示注释」总开关（note_chars_with_source 内部已判）。
        let cfg = self.current_layout();
        let cols2 = if cfg.is_letter() {
            self.grid.columns
        } else {
            self.grid.columns.saturating_sub(1).max(3)
        };
        let gloss = |g: &Glyph| -> Option<crate::gloss::GlossPayload> {
            self.note_chars_with_source(*g)
                .map(|(s, src)| (s.to_string(), src, *g))
        };
        let mut rows2 = 4usize;
        let laid = loop {
            let reserve = if cur.start_chapter {
                crate::gloss::inscription_reserve(self.chapter_inscription_need(), rows2)
            } else {
                0
            };
            let ps = crate::gloss::layout_book_from(slice, &gloss, cols2, rows2, reserve, cur.start_chapter);
            if ps.len() <= 1 || rows2 >= 60 {
                break ps.into_iter().next();
            }
            rows2 += 1;
        }?;

        // ③ 几何：纸宽 = 纸边 46（16×2 留白 + 框 3×4 + 内框 1×2）+ 列总宽；
        //    纸高 = 46 + 行数×行高 + 地脚。字格同时受卡宽 / 卡高约束取小，
        //    地脚里随字号伸缩的那部分（0.36×字号比）一并代入高度约束。
        let ratio = crate::grid::GRID_ROW_RATIO; // 1.1
        let fs_ratio = crate::grid::GRID_FS_RATIO; // 0.75
        // 信笺多一列中缝；非信笺章首页的章题列已含在 cols2 里（limit+1）。
        let cols_total = if cfg.is_letter() { cols2 + 1 } else { cols2 } as f32;
        let by_w = (box_w - 46.0) / cols_total;
        let by_h = (box_h - 56.0) / (rows2 as f32 * ratio + 0.36 * fs_ratio);
        let mut cell_w = by_w.min(by_h).max(8.0);
        // 最小字号保护：字被压到下限时放大字格（与 compute_grid 同策）。
        if cell_w * fs_ratio < crate::grid::GRID_FS_MIN {
            cell_w = crate::grid::GRID_FS_MIN / fs_ratio;
        }
        let fs = (cell_w * fs_ratio).clamp(crate::grid::GRID_FS_MIN, crate::grid::GRID_FS_MAX);
        let cell_h = cell_w * ratio;
        let foot_h = (fs * 0.36).max(9.0) + 10.0;
        let sheet_w = 46.0 + cols_total * cell_w;
        let sheet_h = 46.0 + rows2 as f32 * cell_h + foot_h;

        // ④ 渲染：单叶（四边全封闭，如单独一叶刻纸）；叶码用当前叶号；
        //    章末统计表按「切片之后是否紧跟新章」判（与原页的
        //    chapters_ended_here 同果——切片末尾即原叶末尾）。
        let ends_chapter = match flow.get(i0 + n) {
            Some(g) => g.chapter != last.chapter,
            None => true,
        };
        let stats: Vec<ChapStat> = if ends_chapter {
            self.chapter_stat(last.chapter).into_iter().collect()
        } else {
            Vec::new()
        };
        let geo = LeafGeo {
            cell_w,
            cell_h,
            rows: rows2,
            fs,
            border_style: cfg.border_style,
            show_fish_tail: cfg.show_fish_tail,
            notes_inline: true,
            interactive: false,
        };
        let sheet = self.render_ancient(
            &laid.glyphs,
            &laid.columns,
            false,
            laid.start_chapter,
            self.page,
            LeafEdge::Alone,
            &stats,
            geo,
            cx,
        );
        Some((sheet, sheet_w, sheet_h))
    }

    fn render_share_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let pages = self.paginate();
        let glyphs: Vec<Glyph> = pages.get(self.page).cloned().unwrap_or_default();
        let chapter = glyphs.first().map(|g| g.chapter).unwrap_or(0);
        let chap_title = self.chapter_title(chapter);
        let book_title = self
            .book
            .as_ref()
            .map(|b| b.title.clone())
            .unwrap_or_default();
        let author = self
            .book
            .as_ref()
            .map(|b| b.author.clone())
            .unwrap_or_default();
        let body: Vec<char> = glyphs
            .iter()
            .filter(|g| !g.is_indent())
            .map(|g| self.shown(g.ch))
            .collect();

        // 晒本：卡片 = 当前叶（含夹注）按真实版式重排的竖长纸，尺寸由
        // 内容与可用面积定（下面文字卡的固定比例不适用）。
        let scroll = self.share_layout == CardLayout::Scroll;
        let sunben: Option<(Div, f32, f32)> = if scroll {
            let box_w = (vw - 60.0).max(280.0);
            let box_h = (vh - 200.0).max(260.0);
            Some(
                self.sunben_sheet(box_w, box_h, cx)
                    .unwrap_or_else(|| (Self::sunben_placeholder(t), 400.0, 300.0)),
            )
        } else {
            None
        };

        let (bg, ink, mute) = SHARE_BGS[self.share_bg.min(SHARE_BGS.len() - 1)];
        // 卡片高度按视口比例取定值（「高度」滑杆已删），并给控制面板留位。
        let card_h = sunben
            .as_ref()
            .map(|s| s.2)
            .unwrap_or_else(|| (vh * SHARE_CARD_H_FRAC).min(vh - 250.0).max(280.0));
        // 笺是竖长书衣比例；其余版式沿用横长比例。
        let card_w = sunben.as_ref().map(|s| s.1).unwrap_or_else(|| match self.share_layout {
            CardLayout::Note => (card_h * 0.72).clamp(260.0, 460.0),
            _ => (card_h * 0.66).clamp(300.0, 600.0),
        });
        let fs = (card_h / 40.0).clamp(12.0, 24.0);
        // 单字行高：GPUI 的默认行高约 1.4×字号，再加上 gap_1(4px)。
        // 少算行高会让最后几行溢出卡片（分享卡底部被裁掉半行字）。
        let line_h = fs * 1.42 + 4.0;

        // ---- 题识列 / 日期列 / 朱印：版式共用（笺也共用同一组装）----
        // 题识：章节 / 书名 / 作者 按开关拼接（书名仍夹 12 字）。
        let mut sign_text = String::new();
        if self.share_show_chapter && !chap_title.is_empty() {
            sign_text.push_str(&format!("{} · ", chap_title));
        }
        if self.share_show_title {
            sign_text.push_str(&clip_chars(&book_title, 12));
        }
        if self.share_show_author && !author.is_empty() {
            sign_text.push_str(&format!(" · {}", clip_chars(&author, 8)));
        }
        let sign_cap = ((card_h - 56.0) / 15.0).floor().max(8.0) as usize;
        let sign_text = clip_chars(sign_text.trim_matches([' ', '·']), sign_cap);
        let sign = share_sign(&sign_text, mute);

        // 日期角标：竖排中文日期立于最右（可关）。
        let date_text = cn_date_from_str(&today_ymd());
        let date_col = share_date_col(&date_text, self.share_show_date, mute);

        // 朱印：题识列底部那枚 30×30 小印。
        let seal = share_seal(t);

        // ---- 正文：按版式排（笺 / 竖排分列 / 单列竖排；只要竖排）----
        // 晒本的「正文」是真实版面长图（sunben），不在此列。
        let grid = match self.share_layout {
            CardLayout::Scroll => div(),
            CardLayout::Columns => {
                // 先按高度算「一列最多几个字」，再按宽度收紧列数：
                // 卡片宽度有限，题识列与日期列又必须留位置，否则 flex 会把
                // 两侧的落款挤成 0 宽（分享卡上书名、日期直接消失）。
                let per_col_h = ((card_h - 44.0) / line_h).floor().max(4.0) as usize;
                let avail_w = (card_w - 32.0 - 64.0).max(fs * 4.0);
                let max_cols = (avail_w / (fs + 8.0)).floor().max(2.0);
                let need_cols = (body.len() as f32 / per_col_h as f32).ceil().max(1.0);
                let per_col = if need_cols > max_cols {
                    (body.len() as f32 / max_cols).ceil().max(4.0) as usize
                } else {
                    per_col_h
                };
                let cols: Vec<Vec<char>> = body.chunks(per_col).map(|c| c.to_vec()).collect();
                let mut g = div().flex().flex_row().items_start().gap_2();
                for col in cols.iter().rev() {
                    let mut c = div().flex().flex_col().items_center().gap_1();
                    for ch in col {
                        c = c.child(
                            div()
                                .text_size(px(fs))
                                .text_color(rgb(ink))
                                .child(SharedString::from(ch.to_string())),
                        );
                    }
                    g = g.child(c);
                }
                g
            }
            CardLayout::Single => {
                let cap = ((card_h - 44.0) / line_h).floor().max(4.0) as usize;
                let mut c = div().flex().flex_col().items_center().gap_1();
                for ch in body.iter().take(cap) {
                    c = c.child(
                        div()
                            .text_size(px(fs))
                            .text_color(rgb(ink))
                            .child(SharedString::from(ch.to_string())),
                    );
                }
                c
            }
            CardLayout::Note => {
                // 笺：竖丝栏 8/10/12/14 列自适应（列数按分享内容挑最小值）。
                // 题识列 / 日期列 / 朱印走下方通用组装，不单独成卡。
                note_paper(card_w, card_h, &body, ink, mute)
            }
        };

        // 晒本：纸面即卡片（自带纸色 / 版框 / 界行 / 夹注，不套文字卡外壳）。
        let card = if let Some((sheet, _, _)) = sunben {
            sheet
        } else {
            div()
                .w(px(card_w))
                .h(px(card_h))
                .flex()
                .flex_row()
                .justify_center()
                .items_start()
                .gap_4()
                .p_4()
                .bg(rgb(bg))
                .border_1()
                .border_color(rgb(0x5A4A38))
                .rounded(px(3.0))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_between()
                        .flex_shrink_0()
                        .h_full()
                        .child(div())
                        .child(sign)
                        .child(seal),
                )
                .child(grid)
                .child(date_col)
        };

        // ---- 控制面板：极简留白（无底色、无边框、无方框）----
        //
        // 书名 / 作者 / 章节 / 日期不再是按钮，而是**一排极小的浅灰文字**：
        // 文字本身就是开关（点一下收起该元素），选中稍亮、未选淡到几乎
        // 融进底色——信息还在，但不再有「现代后台系统」那种控件感。
        let metas: Vec<(String, bool, ShareAct)> = [
            (
                clip_chars(&book_title, 14).to_string(),
                self.share_show_title,
                ShareAct::Title,
            ),
            (
                clip_chars(&author, 10).to_string(),
                self.share_show_author,
                ShareAct::Author,
            ),
            (
                clip_chars(&chap_title, 12).to_string(),
                self.share_show_chapter,
                ShareAct::Chapter,
            ),
            (date_text.clone(), self.share_show_date, ShareAct::Date),
        ]
        .into_iter()
        .filter(|(s, _, _)| !s.is_empty())
        .collect();
        let mut meta = div().flex().flex_row().items_center().gap(px(10.0)).flex_1();
        for (i, (text, on, act)) in metas.into_iter().enumerate() {
            if i > 0 {
                meta = meta.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgba(0xB9AE9C40))
                        .child(SharedString::from("·")),
                );
            }
            meta = meta.child(
                div()
                    .id(SharedString::from(format!("share-meta-{i}")))
                    .cursor_pointer()
                    .text_size(px(10.5))
                    .text_color(rgba(if on { 0xD8CFC0D9 } else { 0x8A837866 }))
                    .hover(|s| s.text_color(rgba(0xF2EAD6FF)))
                    .child(SharedString::from(text))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            match act {
                                ShareAct::Title => this.share_show_title = !this.share_show_title,
                                ShareAct::Author => {
                                    this.share_show_author = !this.share_show_author
                                }
                                ShareAct::Chapter => {
                                    this.share_show_chapter = !this.share_show_chapter
                                }
                                ShareAct::Date => this.share_show_date = !this.share_show_date,
                            }
                            cx.notify();
                        }),
                    ),
            );
        }

        // 版式：**只有线条图标**——没有方框、没有底色，选中靠线的明暗。
        let mut layouts = div().flex().flex_row().items_center().gap(px(16.0));
        for (lay, label) in [
            (CardLayout::Note, "八行笺"),
            (CardLayout::Columns, "竖排分列"),
            (CardLayout::Single, "单列竖排"),
            (CardLayout::Scroll, "晒本"),
        ] {
            let on = self.share_layout == lay;
            // 选中 = 近白的亮线；未选中 = 淡到快融进底色的灰线。
            // 不靠方框与底色区分——那正是要丢掉的「现代控件感」。
            let fg = if on { 0xF2EAD6 } else { 0x6E6659 };
            let icon = match lay {
                CardLayout::Note => div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(1.0))
                    .child(bar(11.0, 2.0, fg))
                    .child(bar(11.0, 2.0, fg))
                    .child(bar(11.0, 2.0, fg))
                    .child(bar(11.0, 2.0, fg)),
                CardLayout::Columns => div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap_1()
                    .child(bar(2.0, 13.0, fg))
                    .child(bar(2.0, 13.0, fg))
                    .child(bar(2.0, 13.0, fg)),
                CardLayout::Single => div()
                    .w(px(13.0))
                    .h(px(13.0))
                    .border_2()
                    .border_color(rgb(fg))
                    .rounded(px(1.0)),
                // 晒本：一张竖长纸，纸上两道字列。
                CardLayout::Scroll => div()
                    .w(px(11.0))
                    .h(px(14.0))
                    .border_2()
                    .border_color(rgb(fg))
                    .rounded(px(1.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(1.5))
                    .child(bar(1.5, 8.0, fg))
                    .child(bar(1.5, 8.0, fg)),
            };
            layouts = layouts.child(
                div()
                    .id(SharedString::from(format!("share-lay-{label}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(26.0))
                    .h(px(26.0))
                    .rounded(px(13.0))
                    .cursor_pointer()
                    // 悬停只给一圈几乎看不见的晕（8% 白），不是按钮底色。
                    .hover(|s| s.bg(rgba(0xF2EAD614)))
                    .child(icon)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.share_layout = lay;
                            cx.notify();
                        }),
                    ),
            );
        }

        // 葉号：极小号浅灰，与元信息同一档（不再做成标签 / 色块）。
        let leaf_tag = div()
            .text_size(px(10.5))
            .text_color(rgba(0x8A83788C))
            .child(SharedString::from(format!("第 {} 葉", self.page + 1)));

        // 底色：**四枚 20px 的圆**，圆心一色、边上极细白描边；选中时外面
        // 多一圈朱砂细环作「光晕」。原先的方块 + 粗边是后台系统的样子。
        let mut swatches = div().flex().flex_row().items_center().gap(px(13.0));
        for (i, (bgc, _, _)) in SHARE_BGS.iter().enumerate() {
            let on = i == self.share_bg;
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("share-bg-{i}")))
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    // 选中：外面那圈光晕（朱砂细环）。
                    .when(on, |d| {
                        d.border_1().border_color(rgba(0xC3272DCC))
                    })
                    .hover(|s| s.border_1().border_color(rgba(0xF2EAD64D)))
                    .child(
                        div()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(10.0))
                            .bg(rgb(*bgc))
                            .border_1()
                            .border_color(rgba(0xFFFFFF59)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.share_bg = i;
                            cx.notify();
                        }),
                    ),
            );
        }

        // 控制条：**一行**。左四枚圆、中一行极小浅灰字、右线条图标。
        // 没有容器底色、没有边框、没有内边距——控件直接浮在磨砂底上。
        let controls_h: f32 = 30.0;
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(18.0))
            .h(px(controls_h))
            .child(swatches)
            .child(meta)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(14.0))
                    .child(leaf_tag)
                    .child(layouts),
            );

        // ---- 动作：**两枚朱砂印章**，落在卡片右下角 ----
        // 主印朱底白字（保存图片），副印白线框（分享图片）。悬停微微放大。
        // 「分享文字」撤了：导出界面里它就是个无关按钮。
        let save = self.seal_button("保存", true).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.save_share_image(window, cx);
            }),
        );
        let share_img = self.seal_button("分享", false).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.share_image_to_clipboard(window, cx);
            }),
        );
        let seals = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(share_img)
            .child(save);

        // 卡片与印章用绝对坐标定位（而不是 flex 居中）：保存 / 分享图片
        // 要按 `share_card_rect` 这块矩形裁图，渲染位置必须与算式严格一致。
        //
        // 印章**必须落在卡片矩形之外**（贴着右下角外侧）——若浮在卡片上，
        // 截出来的图会带一枚朱印，导出就废了。
        let seal_box_w = 100.0;
        let total_h = card_h + 20.0 + SEAL_BOX;
        let top = ((vh - total_h) * 0.5).max(0.0);
        let left = ((vw - card_w) * 0.5).max(0.0);
        // 控制条：左侧与卡片对齐，右侧给印章让出位置。
        let controls_w = (card_w - seal_box_w - 16.0).max(200.0);
        let seal_left = left + card_w - seal_box_w + 4.0;
        self.share_card_rect = Some((left, top, card_w, card_h));

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(vw))
            .h(px(vh))
            // 磨砂玻璃：GPUI 0.2.2 没有 backdrop-filter，用 80% 不透明的
            // 近黑（rgba(20,20,20,.8)）压住背景——肉眼看就是一层深色磨砂。
            .bg(rgba(0x141414CC))
            // 右上角 ✕：面板是全屏覆盖且点空白不收起（防误触丢失选择），
            // 所以关闭必须有个常驻的明确出口；Esc 也有同样效果。
            // 同样去边框去底色，只留一个淡淡的 ✕。
            .child(
                div()
                    .absolute()
                    .top(px(16.0))
                    .right(px(20.0))
                    .w(px(26.0))
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(13.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(0xF2EAD614)))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(rgba(0xB9AE9C99))
                            .child(SharedString::from("✕")),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.show_share_panel = false;
                            cx.notify();
                        }),
                    ),
            )
            // 卡片：深色柔和阴影把它从磨砂底上「浮」起来。
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .child(card.shadow_lg()),
            )
            .child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top + card_h + 22.0))
                    .w(px(controls_w))
                    .child(controls),
            )
            .child(
                div()
                    .absolute()
                    .left(px(seal_left))
                    .top(px(top + card_h + 14.0))
                    .child(seals),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _, _cx| {
                    // 点面板空白处不关闭分享面板（避免误触丢失选择）。
                }),
            )
    }

    /// 帮助面板：快捷键与操作说明一览（点工具栏「帮助」或按 ⌘/ 打开）。
    fn render_help_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let vw = self.viewport_w;
        let vh = self.viewport_h;

        let mut list = div().flex().flex_col().px_4().pb_3();

        list = list.child(help_sec("翻页", t));
        list = list.child(help_row("→ / ↓ / 点屏幕右半", "下一页", t));
        list = list.child(help_row("← / ↑ / 点屏幕左半", "上一页", t));

        list = list.child(help_sec("阅读", t));
        list = list.child(help_row("⌘T", "简体 / 繁体互转", t));
        list = list.child(help_row("⌘1 – ⌘5", "墨色（五套）", t));
        list = list.child(help_row("⌘F", "切换字体", t));
        list = list.child(help_row("⌘P", "切换纸色", t));
        list = list.child(help_row("⌘N", "注释显隐", t));
        list = list.child(help_row("⌘J", "通假显隐", t));
        list = list.child(help_row("⌘B", "标点显隐", t));
        list = list.child(help_row("⌘R", "生僻字显隐", t));

        list = list.child(help_sec("视图", t));
        list = list.child(help_row("⌘E", "样式面板", t));
        list = list.child(help_row("⌘L", "目录", t));
        list = list.child(help_row("⌘Z", "专注模式（Esc 退出）", t));
        list = list.child(help_row("⌘K", "笔记面板", t));
        list = list.child(help_row("⌘/", "帮助（本面板）", t));
        list = list.child(help_row("Esc", "退出专注 / 图片·文字分享 / 收起浮层", t));

        list = list.child(help_sec("文字", t));
        list = list.child(help_row("点正文文字", "查询 / 划线 / 笔记 / 复制 / 分享", t));
        list = list.child(help_row("拖选文字", "连续划线（竖排同样支持）", t));
        list = list.child(help_row("⌘C", "复制（有选区复制选区，否则整页）", t));
        list = list.child(help_row("笔记框", "⏎ 保存 · Esc 取消 · ← → 移光标 · ⌘V 粘贴", t));

        let sheet = div()
            .w(px((vw * 0.52).clamp(380.0, 560.0)))
            .max_h(px(vh * 0.82))
            .flex()
            .flex_col()
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.frame()))
            .rounded(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(t.chrome_line()))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(t.accent()))
                            .child(SharedString::from("帮 助")),
                    )
                    .child(small_button("关闭", false, t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.show_help = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(list);

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(vw))
            .h(px(vh))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100ACC))
            .child(sheet)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.show_help = false;
                cx.notify();
            }))
    }

    fn render_notes_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let page = self.page;
        let existing = self.notes.get(&page).cloned().unwrap_or_default();

        let shown: String = if !existing.is_empty() {
            existing.clone()
        } else {
            "（这一葉还没有笔记）".to_string()
        };
        // 笔记内容按当前繁简开关转繁／简，再竖排显示（仿古人批注）。
        let existing_trad: String = existing.chars().map(|c| self.shown(c)).collect();

        // 「第 N 葉的笔记」区：静态展示已有笔记；写笔记统一走正文上方的
        // 浮层编辑框（直接盖在文本上输入，不占用面板、不遮正文）。
        let note_box = if existing.is_empty() {
            div()
                .w(px(328.0))
                .p_2()
                .bg(rgb(t.paper()))
                .border_1()
                .border_color(rgb(t.rule()))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .text_color(rgb(t.ink_soft()))
                .child(SharedString::from(shown))
        } else {
            // 竖排繁体：按字身折列，长笔记自动成多列，overflow_hidden 防止撑破面板。
            div()
                .w(px(328.0))
                .p_2()
                .overflow_hidden()
                .bg(rgb(t.paper()))
                .border_1()
                .border_color(rgb(t.rule()))
                .rounded(px(4.0))
                .child(crate::shelf::vertical_block(
                    &existing_trad,
                    300.0,
                    132.0,
                    13.0,
                    t.ink(),
                    FontWeight::NORMAL,
                ))
        };

        let mut page_btns = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(small_button("✎ 写笔记", false, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.start_note_edit_at(None);
                    cx.notify();
                }),
            ));
        if !existing.is_empty() {
            page_btns = page_btns.child(small_button("清除", false, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_note(this.page, String::new());
                    cx.notify();
                }),
            ));
        }

        let mut list = div().flex().flex_col().gap_1();
        let mut notes: Vec<(usize, String)> =
            self.notes.iter().map(|(k, v)| (*k, v.clone())).collect();
        notes.sort_by_key(|(k, _)| *k);
        if notes.is_empty() {
            list = list.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("还没有笔记，写下第一笔吧。")),
            );
        }
        for (p, txt) in notes {
            let label: String = txt.chars().take(20).map(|c| self.shown(c)).collect();
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        small_button(&format!("第{}葉", p + 1), false, t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.page = p;
                                this.show_notes_panel = false;
                                this.persist();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .text_color(rgb(t.ink()))
                            .child(SharedString::from(label)),
                    ),
            );
        }

        let panel = div()
            .w(px(360.0))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(t.chrome()))
            .border_1()
            .border_color(rgb(t.frame()))
            .rounded(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(t.accent()))
                            .child(SharedString::from("筆 記")),
                    )
                    .child(small_button("关闭", false, t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.show_notes_panel = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from(format!("第 {} 葉的笔记", page + 1))),
                    )
                    .child(note_box),
            )
            .child(page_btns)
            .child(div().h(px(1.0)).bg(rgb(t.chrome_line())))
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("全部笔记")),
            )
            .child(list);

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(vw))
            .h(px(vh))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100ACC))
            .child(panel)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.show_notes_panel = false;
                cx.notify();
            }))
    }

    /// 版式控制面板：古籍竖笺的列数（8/10/12/14）与四周留白随时可切。
    /// 单页 / 双面对开由屏幕方向自动决定（横屏对开、竖屏单页），不在此调整。
    fn render_grid_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let margin = self.grid.margin;

        let mut panel = div()
            .w(px(360.0))
            .p_4()
            .bg(rgb(t.chrome()))
            .border_1()
            .border_color(rgb(t.chrome_line()))
            .rounded(px(6.0))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(rgb(t.accent()))
                            .child(SharedString::from("版式 · 古籍竖笺")),
                    )
                    .child(small_button("收起", true, t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.show_grid_panel = false;
                            cx.notify();
                        }),
                    )),
            );

        // 版式档位：直接遍历配置注册表自动生成按钮——新增档位（如 16 列）
        // 只需在 `LAYOUT_CONFIGS` 加一行，此处零改动。按钮文案读 `cfg.label`，
        // 点亮态比对 `self.layout_key`，点击统一走 `set_layout`（保持阅读进度重排）。
        let mut layout_row = div().flex().flex_row().items_center().gap_2().flex_wrap();
        for cfg in crate::grid::LAYOUT_CONFIGS {
            let key = cfg.key;
            let on = self.layout_key == key;
            layout_row = layout_row.child(
                small_button(cfg.label, on, t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_layout(key);
                        cx.notify();
                    }),
                ),
            );
        }
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(t.ink_soft()))
                        .child(SharedString::from("版式")),
                )
                .child(layout_row),
        );

        // 留白行：窄 / 中 / 宽（四周同比例留白）。
        let mut m_row = div().flex().flex_row().items_center().gap_2();
        for (label, val) in [("窄", 0.02f32), ("中", 0.06), ("宽", 0.12)] {
            let on = (margin - val).abs() < 0.005;
            m_row = m_row.child(small_button(label, on, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_margin(val);
                    cx.notify();
                }),
            ));
        }
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(t.ink_soft()))
                        .child(SharedString::from("四周留白")),
                )
                .child(m_row),
        );

        // 实时反馈：当前引擎算出的列宽 / 字号 / 每面行数。
        panel.child(
            div()
                .text_size(px(10.5))
                .text_color(rgb(t.ink_soft()))
                .child(SharedString::from(format!(
                    "列宽 {:.0} · 字号 {:.0} · 每面 {} 行 · {}",
                    self.col_w,
                    self.size_ancient,
                    self.grid_rows,
                    if self.duplex == Duplex::Facing {
                        "对开（横屏）"
                    } else {
                        "单页（竖屏）"
                    }
                ))),
        )
    }

    fn render_style_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let tab = self.style_tab;
        // 样式面板宽度与「单张页面」严格等宽：直接取 fit_sizes 算出的实际页宽。
        let style_w = self.page_w;

        // ---- 标签行：样式 / 字体（参照古籍阅读器底部样式面板）----
        let tab_style = small_button("样式", tab == StyleTab::Style, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.style_tab = StyleTab::Style;
                cx.notify();
            }),
        );
        let tab_font = small_button("字体", tab == StyleTab::Font, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.style_tab = StyleTab::Font;
                cx.notify();
            }),
        );
        let head = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(tab_style)
                    .child(tab_font),
            )
            .child(small_button("关闭", false, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.show_style = false;
                    cx.notify();
                }),
            ));

        let mut sheet = div()
            .w(px(style_w))
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgb(t.chrome()))
            .border_1()
            .border_color(rgb(t.frame()))
            .rounded(px(8.0))
            // 界面字体（思源黑体）：面板 / 工具条这类 UI 文字用无衬线，
            // 与纸面上的宋体正文一眼分得开。字体名不写死——走 `FontRole::Ui`。
            .font_family(crate::fonts::family_for_role(crate::fonts::FontRole::Ui))
            .child(head);

        match tab {
            StyleTab::Style => {
                // 墨色：五套正文 / 小字注色（深色按钮，如古籍 App 的墨色行）。
                sheet = sheet.child(style_row("墨色", self.palette_chips(cx), t));
                // 简繁：正文字体全部统一切换。列数 / 留白在「版式」面板调，
                // 单页 / 对开由屏幕方向自动决定，此处不再做版式开关。
                let ver = div().flex().flex_row().items_center().gap_2().child(
                    self.style_chips(
                        vec![(
                            if self.show_traditional { "繁体" } else { "简体" },
                            self.show_traditional,
                            StyleAct::Trad,
                        )],
                        cx,
                    ),
                );
                sheet = sheet.child(style_row("简繁", ver, t));
                // 纸色：五套纸底大色板，选中描边。
                let mut swatches = div().flex().flex_row().items_end().gap_2();
                for th in Theme::ALL {
                    let on = th == t;
                    swatches = swatches.child(
                        div()
                            .id(SharedString::from(format!("theme-{:?}", th)))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .bg(rgb(th.paper()))
                                    .border_2()
                                    .border_color(rgb(if on { t.accent() } else { t.rule() }))
                                    .rounded(px(3.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(15.0))
                                    .text_color(rgb(th.ink()))
                                    .child(SharedString::from("文")),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(if on { t.accent() } else { t.ink_soft() }))
                                    .child(SharedString::from(th.label())),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.theme = th;
                                    cx.notify();
                                }),
                            ),
                    );
                }
                sheet = sheet.child(style_row("纸色", swatches, t));
                // 丝栏：乌 / 金 / 朱 三色界行（竖排列间那道细线）。
                // 色块里立三道竖线，直接预览界行颜色与粗细。
                let mut rules = div().flex().flex_row().items_end().gap_2();
                for &rc in crate::types::RuleColor::ALL.iter() {
                    let on = rc == self.rule_color;
                    let rule_c = t.rule_by(rc);
                    rules = rules.child(
                        div()
                            .id(SharedString::from(format!("rule-{:?}", rc)))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .child(
                                div()
                                    .w(px(34.0))
                                    .h(px(28.0))
                                    .bg(rgb(t.paper()))
                                    .border_2()
                                    .border_color(rgb(if on { t.accent() } else { t.rule() }))
                                    .rounded(px(3.0))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(4.0))
                                    .child(
                                        div().w(px(1.5)).h(px(15.0)).bg(rgb(rule_c)),
                                    )
                                    .child(
                                        div().w(px(1.5)).h(px(15.0)).bg(rgb(rule_c)),
                                    )
                                    .child(
                                        div().w(px(1.5)).h(px(15.0)).bg(rgb(rule_c)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(if on { t.accent() } else { t.ink_soft() }))
                                    .child(SharedString::from(rc.label())),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.rule_color = rc;
                                    cx.notify();
                                }),
                            ),
                    );
                }
                sheet = sheet.child(style_row("丝栏", rules, t));
                // 纸色：**遍历主题配置表自动生成**预览按钮——新增一套配色
                // 只需在 `PAPER_THEMES` 加一行，此处零改动。
                // 每颗按钮就是一张小纸：底色=纸色、边框=界栏色、
                // 例字=正文色，点中即用该套配色重绘（fit_sizes 后 notify）。
                let mut themes_row = div().flex().flex_row().items_center().gap_2().flex_wrap();
                for (i, pt) in crate::theme::PAPER_THEMES.iter().enumerate() {
                    let on = self.theme.index() == i;
                    let name = pt.name;
                    themes_row = themes_row.child(
                        div()
                            .w(px(46.0))
                            .h(px(28.0))
                            .rounded(px(5.0))
                            .bg(rgb(pt.paper))
                            .border_1()
                            .border_color(rgb(if on { t.accent() } else { pt.border }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pt.text))
                                    .child(SharedString::from(name)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_theme(i);
                                    cx.notify();
                                }),
                            ),
                    );
                }
                sheet = sheet.child(style_row("纸色", themes_row, t));
                // 标注：通假 / 标点 / 生僻字 / 注释（「红色繁体小注」已删除，
                // 繁简改由版式行的「简体 / 繁体」统一切换正文字体）。
                let marks = self.style_chips(
                    vec![
                        ("通假", self.show_tongjia, StyleAct::Tong),
                        ("标点", self.show_punct, StyleAct::Punct),
                        ("生僻字", self.show_rare, StyleAct::Rare),
                        ("数字汉字", self.han_digits, StyleAct::Han),
                        ("自动开合", self.auto_duplex, StyleAct::AutoDuplex),
                        ("注释", self.show_notes, StyleAct::Notes),
                    ],
                    cx,
                );
                sheet = sheet.child(style_row("标注", marks, t));
            }
            StyleTab::Font => {
                // **排版字体**：刻本宋体 ↔ 手写楷体（一键换一对，见
                // `Root::set_font_preset`——换完连行高与分页一起重算）。
                sheet = sheet.child(style_row("排版字体", self.font_preset_row(cx), t));
                // 字体：按当前模式给出可选字体（正文宋体 / 批注楷体 / 黑体 / 西文）。
                sheet = sheet.child(style_row("字体", self.font_choices_row(cx), t));
                // 批注字体：楷体二选一，与正文宋体分开——
                // 正文匠体、注文手写体，正是刻本的层次。
                sheet = sheet.child(style_row("批注", self.note_font_row(cx), t));
                // 字级：小 / 中 / 大 / 特大 四档（原先那条七档圆点滑块已撤销——
                // 档位密到肉眼分不出，反而要来回试）。选中档用朱砂实底，
                // 一眼看清自己在哪一档；右侧实时报出当前字号。
                let cur_level = crate::types::ZoomLevel::from_zoom(self.zoom);
                let mut seg = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .p(px(2.0))
                    .rounded(px(5.0))
                    .bg(rgb(t.chip()))
                    .border_1()
                    .border_color(rgb(t.chrome_line()));
                for lvl in crate::types::ZoomLevel::ALL {
                    let on = lvl == cur_level;
                    seg = seg.child(
                        div()
                            .id(SharedString::from(format!("zoom-lvl-{}", lvl.label())))
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .bg(rgb(if on { t.accent() } else { 0x00000000 }))
                            .text_size(px(11.0))
                            .text_color(rgb(if on { 0xFFF6E8 } else { t.ink() }))
                            .child(SharedString::from(lvl.label()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_zoom_level(lvl);
                                    cx.notify();
                                }),
                            ),
                    );
                }
                seg = seg.child(
                    div()
                        .ml_1()
                        .text_size(px(10.5))
                        .text_color(rgb(t.ink_soft()))
                        .child(SharedString::from(format!(
                            "{}pt",
                            self.current_size() as i32
                        ))),
                );
                sheet = sheet.child(style_row("字级", seg, t));
            }
        }

        // ---- 底部两个大按钮：分享图片 / 分享文字 ----
        let share_img = self.action_button("分享图片", t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.show_share_panel = true;
                cx.notify();
            }),
        );
        let share_txt = self.action_button("分享文字", t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.copy_page_text();
                cx.notify();
            }),
        );
        sheet = sheet.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .mt_1()
                .child(share_img)
                .child(share_txt),
        );

        // 作为正文正下方的行内面板（像照片说明），宽度等于单张页面，不遮挡正文。
        sheet
    }


    /// 把分享卡截成 PNG：按渲染时记下的卡片矩形裁切，只截卡片本身。
    ///
    /// `screencapture -R x,y,w,h` 用屏幕坐标（左上原点）；窗口 frame 含标题栏，
    /// 内容区从标题栏下沿开始，所以 y 要加 `窗口高 - 视口高`。
    /// 截分享卡。返回 `Ok(路径)` / `Err(可读原因)`。
    ///
    /// 早先只返回 `Option`，失败时界面上一句「需要屏幕录制权限」把所有情况
    /// 都归到权限头上——坐标算错、桌面目录不存在、临时文件写不进去都长一个样，
    /// 没法排查。现在把 `screencapture` 的 stderr 一并带出来。
    pub(crate) fn capture_card_png(&self, window: &Window, prefix: &str) -> Result<String, String> {
        let b = window.bounds();
        let unit = px(1.0);
        let wx = b.origin.x / unit;
        let wy = b.origin.y / unit;
        let wh = b.size.height / unit;
        let title_h = (wh - self.viewport_h).max(0.0);
        let (rx, ry, rw, rh) = self.share_card_rect.unwrap_or((
            0.0,
            0.0,
            self.viewport_w,
            self.viewport_h,
        ));
        if rw < 40.0 || rh < 40.0 {
            return Err(format!(
                "卡片区域过小（{rw:.0}×{rh:.0}），无法截图；请重开分享面板"
            ));
        }
        let x = (wx + rx).max(0.0);
        let y = (wy + title_h + ry).max(0.0);
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        // 桌面目录不一定存在（沙箱 / 自定义 HOME），落不下去就退回家目录。
        let dir = {
            let d = format!("{home}/Desktop");
            if std::path::Path::new(&d).is_dir() {
                d
            } else {
                home.clone()
            }
        };
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = format!("{dir}/{prefix}-{secs}.png");
        let rect = format!("{x:.0},{y:.0},{rw:.0},{rh:.0}");
        if std::env::var("JIANDU_CAPTURE_DEBUG").is_ok() {
            eprintln!(
                "bounds=({wx:.0},{wy:.0},{:.0},{wh:.0}) viewport=({:.0},{:.0}) title={title_h:.0} card=({rx:.0},{ry:.0},{rw:.0},{rh:.0}) -> {rect}",
                b.size.width / unit, self.viewport_w, self.viewport_h
            );
        }
        let out = std::process::Command::new("screencapture")
            .args(["-x", "-R", &rect, &path])
            .output();
        match out {
            Ok(o) if o.status.success() && std::path::Path::new(&path).exists() => Ok(path),
            Ok(o) => {
                let why = String::from_utf8_lossy(&o.stderr).trim().to_string();
                let why = if why.is_empty() {
                    format!("退出码 {:?}", o.status.code())
                } else {
                    why
                };
                // macOS 上最常见的失败就是没给屏幕录制权限。
                let hint = if why.to_lowercase().contains("permission")
                    || why.to_lowercase().contains("not allowed")
                    || why.to_lowercase().contains("denied")
                {
                    "请到「系统设置 → 隐私与安全性 → 屏幕录制」勾选笺读后重试"
                } else {
                    "（若提示权限不足：系统设置 → 隐私与安全性 → 屏幕录制）"
                };
                Err(format!("截图失败：{why} {hint}"))
            }
            Err(e) => Err(format!("无法调用 screencapture：{e}")),
        }
    }

    /// 保存分享卡 / 晒本：截卡片到桌面 PNG，并收起面板。
    fn save_share_image(&mut self, window: &Window, cx: &mut Context<Self>) {
        // 先截再收面板：截屏是同步的，先关面板会让画面在下一帧才更新，
        // 虽然大概率仍截到卡片，但顺序反了容易截到「已消失」的界面。
        let sunben = self.share_layout == CardLayout::Scroll;
        let prefix = if sunben { "jiandu-sunben" } else { "jiandu-card" };
        let shot = self.capture_card_png(window, prefix);
        self.show_share_panel = false;
        self.notice = match shot {
            Ok(p) => {
                if sunben {
                    format!("已保存晒本：{p}")
                } else {
                    format!("已保存分享卡：{p}")
                }
            }
            Err(e) => e,
        };
        cx.notify();
    }

    /// 分享图片：把截下的卡片直接放进系统剪贴板，可在微信 / 备忘录里 ⌘V 粘贴。
    fn share_image_to_clipboard(&mut self, window: &Window, cx: &mut Context<Self>) {
        let sunben = self.share_layout == CardLayout::Scroll;
        let prefix = if sunben { "jiandu-sunben" } else { "jiandu-share" };
        let shot = self.capture_card_png(window, prefix);
        self.show_share_panel = false;
        match shot {
            Ok(p) => {
                let script = format!(
                    "set the clipboard to (read (POSIX file \"{p}\") as TIFF picture)"
                );
                let ok = std::process::Command::new("osascript")
                    .args(["-e", &script])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                self.notice = if ok {
                    if sunben {
                        "晒本已复制到剪贴板，可直接粘贴".to_string()
                    } else {
                        "分享图已复制到剪贴板，可直接粘贴".to_string()
                    }
                } else {
                    format!("图片已保存到 {p}（剪贴板不可用）")
                };
            }
            Err(e) => {
                self.notice = e;
            }
        }
        cx.notify();
    }

    /// 样式面板底部的大按钮（等宽、居中文字）。
    fn action_button(&self, label: &'static str, t: Theme) -> Div {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .py_2()
            .rounded(px(5.0))
            .bg(rgb(t.chip()))
            .border_1()
            .border_color(rgb(t.chrome_line()))
            .cursor_pointer()
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(t.chrome_text()))
                    .child(SharedString::from(label)),
            )
    }

    /// 分享面板的**朱砂印章**按钮：一枚方印，`filled = true` 是朱底白字
    /// （主动作：保存），`false` 是白线框（次动作：分享）。
    ///
    /// 印面上字**竖排**（走正文族名），不做圆角胶囊、不做图标——印就是印。
    /// 悬停时印面 `+4px`，从中心向外微微放大（GPUI 0.2.2 没有元素级
    /// transform，只能改尺寸；外框固定，故放大不挤旁边的控件）。
    fn seal_button(&self, label: &'static str, filled: bool) -> Div {
        let family = self.main_family();
        let mut face = div()
            .w(px(SEAL_SIZE))
            .h(px(SEAL_SIZE))
            .rounded(px(3.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(1.0))
            .cursor_pointer();
        face = if filled {
            face.bg(rgb(CINNABAR))
        } else {
            face.border_1().border_color(rgba(0xF2EAD659))
        };
        for c in label.chars() {
            face = face.child(
                div()
                    .font_family(family.clone())
                    .text_size(px(13.0))
                    .text_color(rgb(if filled { 0xFFF8EC } else { 0xF2EAD6 }))
                    .child(SharedString::from(c.to_string())),
            );
        }
        div()
            .w(px(SEAL_BOX))
            .h(px(SEAL_BOX))
            .flex()
            .items_center()
            .justify_center()
            .child(face.hover(|s| s.w(px(SEAL_SIZE + 4.0)).h(px(SEAL_SIZE + 4.0))))
    }

    /// 样式面板里的一组小按钮。
    fn style_chips(
        &self,
        items: Vec<(&'static str, bool, StyleAct)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        let mut row = div().flex().flex_row().items_center().gap_2();
        for (label, on, act) in items {
            row = row.child(
                small_button(label, on, t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        match act {
                            StyleAct::Single => this.duplex = Duplex::Single,
                            // 双面对开仅现代横笺可用（古文竖笺恒定单面）。
                            StyleAct::Facing => {
                                if this.script != Script::Ancient {
                                    this.duplex = Duplex::Facing;
                                }
                            }
                            StyleAct::Inline => this.set_notes_inline(true),
                            StyleAct::Footnote => this.set_notes_inline(false),
                            StyleAct::Trad => this.show_traditional = !this.show_traditional,
                            StyleAct::Tong => this.show_tongjia = !this.show_tongjia,
                            StyleAct::Notes => this.show_notes = !this.show_notes,
                            StyleAct::Punct => this.show_punct = !this.show_punct,
                            StyleAct::Rare => this.show_rare = !this.show_rare,
                            // 数字转汉字：改它要重排（格数账变了），
                            // 走 `set_layout` 同级路径保持阅读进度。
                            StyleAct::Han => {
                                this.han_digits = !this.han_digits;
                                this.fit_sizes();
                            }
                            // 响应式单 / 双叶：宽屏对开、窄屏单叶贴右；
                            // 关掉则沿用手动「对开 / 单页」。
                            StyleAct::AutoDuplex => {
                                this.auto_duplex = !this.auto_duplex;
                                this.fit_sizes();
                            }
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        row
    }

    /// 样式面板里的「字体」选择行（字体、字号已收进「样式」面板）。
    /// 样式面板的「墨色」行：五套正文 / 小字注色，深色圆角按钮（仿古籍 App 墨色行）。
    fn palette_chips(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let mut row = div().flex().flex_row().flex_wrap().items_center().gap_2();
        for p in InkPalette::ALL {
            let on = self.palette == p;
            let chip = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(px(5.0))
                .border_1()
                .border_color(rgb(if on { t.accent() } else { 0x2A2520 }))
                .bg(rgb(if on { 0x4A4038 } else { 0x2A2520 }))
                .cursor_pointer()
                .child(div().w(px(11.0)).h(px(11.0)).rounded(px(2.0)).bg(rgb(p.ink(t))))
                .child(div().w(px(11.0)).h(px(11.0)).rounded(px(2.0)).bg(rgb(p.note(t))))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(0xE9E2D2))
                        .child(SharedString::from(p.label())),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.palette = p;
                        cx.notify();
                    }),
                );
            row = row.child(chip);
        }
        row
    }

    /// 字体页的**批注字体**行：楷体二选一（`FontChoice::NOTE`）。
    ///
    /// 与正文宋体分行陈列：正文要「生僻字全覆盖 + 竖排标点正确」，
    /// 批注要「手写味、与正文一眼可分」，两件事的最优解不是同一个字体。
    /// 字体页的**排版字体**行：刻本宋体 ↔ 手写楷体。
    ///
    /// 这是「一键换一对」的开关：选中即同时换正文与批注（两者永远不同体），
    /// 并连带重算行高与分页（见 `Root::set_font_preset`）。字牌用该预设的
    /// **正文字体**渲染自己的名字——选前就能看见「宋体什么样、楷体什么样」。
    fn font_preset_row(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let cur = self.font_preset;
        let mut row = div().flex().flex_row().items_center().gap_2().py_1();
        for p in crate::types::FontPreset::ALL {
            let on = cur == p;
            let label = p.label();
            row = row.child(
                div()
                    .id(SharedString::from(format!("font-preset-{}", p.index())))
                    .flex()
                    .flex_col()
                    .items_center()
                    .px_3()
                    .py_1()
                    .bg(rgb(if on { t.ink() } else { t.chip() }))
                    .border_1()
                    .border_color(rgb(if on { t.accent() } else { t.chrome_line() }))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    // 字牌用这个预设的正文款字体：所见即所得。
                    .font_family(crate::fonts::family_for(
                        p.main_choice(),
                        crate::types::Script::Ancient,
                    ))
                    .text_size(px(13.0))
                    .text_color(rgb(if on { 0xF3EADA } else { t.ink() }))
                    .child(SharedString::from(label))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(rgb(if on { 0xC9BFAE } else { t.ink_soft() }))
                            .child(SharedString::from(p.hint())),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            // 换字体 → 换行高 → 重算尺寸 → 重排（四步在
                            // `set_font_preset` 里一次走完）。
                            this.set_font_preset(p);
                            this.notice = format!("排版字体：{}（{}）", p.label(), p.hint());
                            cx.notify();
                        }),
                    ),
            );
        }
        row
    }

    fn note_font_row(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let cur = self.note_font();
        let mut row = div().flex().flex_row().items_center().gap_2().py_1();
        for choice in FontChoice::NOTE {
            let on = cur == choice;
            let label = choice.label();
            // 字牌直接用该字体渲染自己的名字：选前就能看见楷体长什么样。
            row = row.child(
                div()
                    .id(SharedString::from(format!("note-font-{}", label)))
                    .px_3()
                    .py_1()
                    .bg(rgb(if on { t.ink() } else { t.chip() }))
                    .border_1()
                    .border_color(rgb(if on { t.accent() } else { t.chrome_line() }))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .font_family(crate::fonts::note_family(choice))
                    .text_size(px(13.0))
                    .text_color(rgb(if on { 0xF3EADA } else { t.ink() }))
                    .child(SharedString::from(label))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_note_font(choice);
                            cx.notify();
                        }),
                    ),
            );
        }
        row
    }

    /// 字体页的字体行：竖排字牌（参照图 2 的「朱雀仿宋 / 京华老宋 …」）。
    /// 每块字牌把字体名逐字竖排，选中的用墨底白字；未安装的字体仍可选，
    /// 实际渲染会沿回退链落到最接近的系统字体。
    fn font_choices_row(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let cur = self.current_font();
        let mut opts: Vec<FontChoice> = Vec::new();
        if self.script == Script::Modern {
            opts.extend_from_slice(&FontChoice::MODERN);
        }
        opts.extend_from_slice(&FontChoice::CLASSIC);

        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_start()
            .gap_2()
            .py_1();
        for choice in opts {
            let on = cur == choice;
            let label = choice.label();
            let mut card = div()
                .id(SharedString::from(format!("font-{}", label)))
                .flex()
                .flex_col()
                .items_center()
                .px_2()
                .py_2()
                .min_w(px(24.0))
                .bg(rgb(if on { 0x2B2622 } else { t.chip() }))
                .border_1()
                .border_color(rgb(if on { t.accent() } else { t.chrome_line() }))
                .rounded(px(4.0))
                .cursor_pointer();
            for c in label.chars() {
                card = card.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(if on { 0xF3EADA } else { t.ink() }))
                        .child(SharedString::from(c.to_string())),
                );
            }
            row = row.child(card.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_font(choice);
                    this.persist();
                    cx.notify();
                }),
            ));
        }
        row
    }

    // ---- 样式条（文本正下方，常驻，像照片说明）与快捷键辅助 ----

    /// 选中第 idx 套配色（越界忽略）。
    fn set_palette(&mut self, idx: usize) {
        if let Some(p) = InkPalette::ALL.get(idx) {
            self.palette = *p;
            self.persist();
        }
    }

    /// 循环切换当前模式的字体（字体页的竖排字牌是点选，这里是快捷键 F）。
    fn cycle_font(&mut self) {
        let mut opts: Vec<FontChoice> = Vec::new();
        if self.script == Script::Modern {
            opts.extend_from_slice(&FontChoice::MODERN);
        }
        opts.extend_from_slice(&FontChoice::CLASSIC);
        let idx = opts
            .iter()
            .position(|c| *c == self.current_font())
            .unwrap_or(0);
        let next = opts[(idx + 1) % opts.len()];
        self.set_font(next);
        self.persist();
    }

    /// 循环切换纸色主题。
    fn cycle_theme(&mut self) {
        let idx = Theme::ALL.iter().position(|x| *x == self.theme).unwrap_or(0);
        self.theme = Theme::ALL[(idx + 1) % Theme::ALL.len()];
        self.persist();
    }

    /// 切到第 `i` 套纸面主题（配置表 `PAPER_THEMES` 下标）。
    /// 换纸即重排（纸色变了、夹注兑淡基点也变），随后由调用方 notify 重绘。
    fn set_theme(&mut self, i: usize) {
        let Some(next) = Theme::ALL.get(i).copied() else {
            return;
        };
        self.theme = next;
        self.fit_sizes();
        self.persist();
    }
}

/// 供未来扩展：字体权重占位，避免未使用告警。
#[allow(dead_code)]
fn _unused() {
    let _ = FontWeight::MEDIUM;
}

/// 分享面板里「书名 / 作者 / 章节 / 日期」四个开关。
#[derive(Clone, Copy)]
enum ShareAct {
    Title,
    Author,
    Chapter,
    Date,
}

/// 版式图标里的一根线条（分享面板的三个版式按钮用）。
fn bar(w: f32, h: f32, color: u32) -> Div {
    div().w(px(w)).h(px(h)).rounded(px(1.0)).bg(rgb(color))
}

/// 样式面板里的动作。
#[derive(Clone, Copy)]
enum StyleAct {
    Single,
    Facing,
    Inline,
    Footnote,
    Trad,
    Tong,
    Notes,
    Punct,
    Rare,
    /// 数字转汉字（`1628` → `一六二八`）。
    Han,
    /// 响应式单 / 双叶：按屏幕宽高比自动决定对开 / 单叶。
    AutoDuplex,
}

/// 样式面板的一行：左侧标签，右侧一组控件。
fn style_row(label: &'static str, chips: Div, t: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(58.0))
                .text_size(px(11.5))
                .text_color(rgb(t.ink_soft()))
                .child(SharedString::from(label)),
        )
        .child(chips)
}

/// 帮助面板的一行快捷键：左侧键名（朱色小签），右侧功能说明。
fn help_row(key: &str, desc: &str, t: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(168.0))
                .flex_shrink_0()
                .text_size(px(11.5))
                .text_color(rgb(t.accent()))
                .child(SharedString::from(key.to_string())),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(t.ink()))
                .child(SharedString::from(desc.to_string())),
        )
}

/// 帮助面板的分组标题。
fn help_sec(title: &str, t: Theme) -> Div {
    div()
        .pt_2()
        .pb_1()
        .text_size(px(10.5))
        .text_color(rgb(t.ink_soft()))
        .child(SharedString::from(title.to_string()))
}

/// 分享卡题识列：竖排一列小字（题识 = 章节 · 书名 · 作者，调用方已拼接裁好）。
/// 题识里的【】显作「」——与阅读版面同一套字。
fn share_sign(sign_text: &str, mute: u32) -> Div {
    let mut sign = div().flex().flex_col().items_center().gap_1();
    for c in sign_text.chars() {
        sign = sign.child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(mute))
                .child(SharedString::from(
                    crate::types::bracket_to_quote(c).to_string(),
                )),
        );
    }
    sign
}

/// 分享卡日期列：竖排中文日期立于最右（关时留一枚零宽占位，把正文推离右缘）。
fn share_date_col(date_text: &str, show: bool, mute: u32) -> Div {
    if !show {
        return div().flex_shrink_0();
    }
    let mut date_chars = div().flex().flex_col().items_center().gap_1();
    for c in date_text.chars() {
        date_chars = date_chars.child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(mute))
                .child(SharedString::from(c.to_string())),
        );
    }
    div()
        .flex()
        .flex_col()
        .items_center()
        .flex_shrink_0()
        .h_full()
        .child(date_chars)
}

/// 分享卡朱印：30×30 朱砂（accent）圆角小印，内「笺讀」二字，
/// 坐在题识列底部——题跋图同款构成：题识在左、印在其下。
fn share_seal(t: Theme) -> Div {
    div()
        .w(px(30.0))
        .h(px(30.0))
        .bg(rgb(t.accent()))
        .rounded(px(2.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(10.5))
                .text_color(rgb(0xF5EFE2))
                .child(SharedString::from("笺讀")),
        )
}

/// 笺面网格规划：给定笺面内尺寸与正文字数，返回 `(字号, 列数, 每列字数)`。
/// **列数按内容自适应**：在 8/10/12/14 列里取能装下的最小列数——内容越少
/// 列越少、字越大；内容越多列越多、字越小。都装不下时只缩字号（列数封顶
/// 14，字号下限 8pt）。
fn note_grid_plan(inner_w: f32, inner_h: f32, len: usize) -> (f32, usize, usize) {
    let mut fs = 11.0f32;
    let mut n_cols = 8usize;
    let mut per_col = 1usize;
    for &c in &[8usize, 10, 12, 14] {
        // 列宽 = 笺宽 ÷ 列数；字号跟列宽走（字近方，略收一成）。
        let f = (inner_w / c as f32 * 0.92).clamp(11.0, 30.0);
        // 每列容量 = 笺高 ÷ 行距（GPUI 行高约 1.4×字号）。
        let cap = ((inner_h / (f * 1.42)).floor() as usize).max(1);
        fs = f;
        n_cols = c;
        per_col = cap;
        if len <= c * cap {
            break;
        }
    }
    // 14 列仍装不下：只缩字号（列数封顶 14），字号不低于 8pt。
    while n_cols == 14 && len > n_cols * per_col && fs - 0.5 >= 8.0 {
        fs -= 0.5;
        per_col = ((inner_h / (fs * 1.42)).floor() as usize).max(1);
    }
    (fs, n_cols, per_col.max(1))
}

/// 分享笺面（笔记 / 图片分享的笺纸样式）：仿「八行笺 / 十三行」——
/// **竖丝栏**（乌丝栏是竖线，界在列与列之间）把笺面分成 8/10/12/14 列，
/// 列数按分享内容自适应（`note_grid_plan`），正文竖排从右到左。
/// 只有竖排：横排版式与右侧高度标识均已按要求删除。
///
/// 题识列 / 日期列 / 朱印由调用方（`render_share_sheet`）在笺面两侧组装，
/// 三种竖排版式共用同一组装。
fn note_paper(card_w: f32, card_h: f32, body: &[char], ink: u32, mute: u32) -> Div {
    // 笺面在卡片内的可用尺寸：卡片内边距 32 + 题识列 ~30 + 日期列 ~13 + 列距 32。
    let avail_w = (card_w - 108.0).max(140.0);
    let avail_h = (card_h - 32.0).max(120.0);
    let pad_x = 8.0;
    let inner_w = (avail_w - pad_x * 2.0).max(56.0);
    let inner_h = avail_h;

    // ---- 网格：列数 8/10/12/14 按内容挑，字号随列宽，每列 per_col 字 ----
    let (fs, n_cols, per_col) = note_grid_plan(inner_w, inner_h, body.len());
    let line_pitch = fs * 1.42;
    let col_w = inner_w / n_cols as f32;
    let cols: Vec<Vec<char>> = body.chunks(per_col).map(|c| c.to_vec()).collect();

    // ---- 竖丝栏（乌丝栏）：列界细线，通笺高（外框由卡片自带）----
    let rule_c = (mute & 0x00FFFFFF) | 0x00000033u32;
    let mut rules = div()
        .absolute()
        .top(px(0.0))
        .left(px(pad_x))
        .w(px(inner_w))
        .h(px(inner_h));
    for i in 1..n_cols {
        rules = rules.child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(i as f32 * col_w))
                .w(px(1.0))
                .h(px(inner_h))
                .bg(rgb(rule_c)),
        );
    }

    // ---- 正文：竖排，每列定宽 col_w（字居中），首列在最右 ----
    let mut text = div()
        .absolute()
        .top(px(0.0))
        .left(px(pad_x))
        .w(px(inner_w))
        .h(px(inner_h))
        .flex()
        .flex_row()
        .items_start()
        .justify_end();
    for col in cols.iter().rev() {
        let mut c = div().w(px(col_w)).flex().flex_col().items_center();
        for ch in col {
            c = c.child(
                div()
                    .h(px(line_pitch))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(fs))
                            .text_color(rgb(ink))
                            .child(SharedString::from(ch.to_string())),
                    ),
            );
        }
        text = text.child(c);
    }

    div()
        .w(px(avail_w))
        .h(px(avail_h))
        .relative()
        .child(rules)
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::{chapters_ended_at, cn_date_from_str, cn_small, first_empty_column, note_grid_plan};
    use crate::gloss::{Page, Piece};

    /// 「篇 / 章 / 字」只收在此叶收尾的章：跨叶未完的章不赶它，
    /// 两章同叶收尾时两章都列（一行一篇）。
    #[test]
    fn stats_only_list_chapters_that_end_on_this_leaf() {
        let page = |chapters: &[usize]| -> Page {
            Page {
                glyphs: chapters
                    .iter()
                    .map(|&c| {
                        crate::types::Glyph {
                            chapter: c,
                            para: 0,
                            offset: 0,
                            ch: '甲',
                            kind: crate::types::GlyphKind::Char,
                        }
                    })
                    .collect(),
                start_chapter: false,
                columns: Vec::new(),
            }
        };
        // 第 0 叶：章 0 未完（下一叶还是它） → 不收。
        let pages = vec![page(&[0, 0]), page(&[0, 1])];
        assert_eq!(chapters_ended_at(&pages, 0), Vec::<usize>::new());
        // 第 1 叶：章 0 在此收尾，章 1 刚开始 → 只收章 0。
        let pages = vec![page(&[0, 0]), page(&[0, 1]), page(&[1, 1])];
        assert_eq!(chapters_ended_at(&pages, 1), vec![0]);
        // 末叶：两章都收（短章同叶）。
        let pages = vec![page(&[0]), page(&[1, 2])];
        assert_eq!(chapters_ended_at(&pages, 1), vec![1, 2]);
        // 越界叶：不收。
        assert_eq!(chapters_ended_at(&pages, 9), Vec::<usize>::new());
    }

    /// 章末统计表要落在「正文之后的第一片空列」：
    /// 整页排满时返回 None（无处可落，宁可不画），否则给下一列的下标。
    #[test]
    fn stats_table_lands_right_after_the_last_text_column() {
        let text = |s: &str| -> Vec<Piece> {
            s.chars()
                .map(|ch| {
                    Piece::Main(crate::types::Glyph {
                        chapter: 0,
                        para: 0,
                        offset: 0,
                        ch,
                        kind: crate::types::GlyphKind::Char,
                    })
                })
                .collect()
        };
        // 前两列有字，第三列起空着 → 表落在第 2 列。
        let cols = vec![text("甲乙"), text("丙丁"), Vec::new(), Vec::new()];
        assert_eq!(first_empty_column(&cols, 4), Some(2));
        // 空页面（如章首尚未落字）→ 落在第 0 列。
        let cols = vec![Vec::new(), Vec::new()];
        assert_eq!(first_empty_column(&cols, 2), Some(0));
        // 排满 → 无处可落。
        let cols = vec![text("甲"), text("乙")];
        assert_eq!(first_empty_column(&cols, 2), None);
        // 只有一列空位时仍返回它（宽度够不够由渲染层按可用宽再判一次）。
        let cols = vec![text("甲"), text("乙"), Vec::new()];
        assert_eq!(first_empty_column(&cols, 3), Some(2));
    }

    #[test]
    fn cn_small_numbers() {
        assert_eq!(cn_small(0), "〇");
        assert_eq!(cn_small(9), "九");
        assert_eq!(cn_small(10), "十");
        assert_eq!(cn_small(11), "十一");
        assert_eq!(cn_small(18), "十八");
        assert_eq!(cn_small(20), "二十");
        assert_eq!(cn_small(21), "二十一");
        assert_eq!(cn_small(30), "三十");
        assert_eq!(cn_small(31), "三十一");
    }

    #[test]
    fn cn_date_parsing() {
        assert_eq!(cn_date_from_str("2026-10-18"), "二〇二六年十月十八日");
        assert_eq!(cn_date_from_str("2026-01-05"), "二〇二六年一月五日");
        assert_eq!(cn_date_from_str("2000-12-31"), "二〇〇〇年十二月三十一日");
        assert_eq!(cn_date_from_str(""), "");
        assert_eq!(cn_date_from_str("bad"), "");
        assert_eq!(cn_date_from_str("20x6-1a-2"), "");
    }

    /// 笺面列数按分享内容自适应：短内容 8 列（字大），越长列越多
    /// （10 → 12 → 14，字随之缩小），14 列仍装不下只缩字号不加列。
    #[test]
    fn note_grid_columns_adapt_to_content_length() {
        // 近似真实卡片：笺面约 256×482（380×530 的八行笺卡扣掉题识 / 日期列）。
        let (iw, ih) = (256.0, 482.0);
        // 20 字：8 列就装得下（每列 11 字），字号顶到列宽上限（≈29.4pt）。
        let (fs8, cols8, per8) = note_grid_plan(iw, ih, 20);
        assert_eq!(cols8, 8);
        assert!((fs8 - iw / 8.0 * 0.92).abs() < 0.01);
        assert!(per8 >= 11);
        // 100 字：8 列（88 字）装不下 → 10 列，字号降到 24pt 上下。
        let (_fs10, cols10, per10) = note_grid_plan(iw, ih, 100);
        assert_eq!(cols10, 10);
        assert!(cols10 * per10 >= 100);
        // 300 字：一路加到 14 列（约 280 字）仍差一点 → 封顶 14 列并缩字号。
        let (fs14, cols14, per14) = note_grid_plan(iw, ih, 300);
        assert_eq!(cols14, 14);
        assert!(fs14 < 24.0, "长内容应缩字号，实得 {fs14}");
        assert!(cols14 * per14 >= 300 || fs14 <= 8.0);
        // 列数随内容单调不减；空内容最少列数起步。
        let cols = |n: usize| note_grid_plan(iw, ih, n).1;
        assert_eq!(cols(0), 8);
        assert!(cols(40) <= cols(120));
        assert!(cols(120) <= cols(400));
        assert!(cols(400) <= 14);
        // 极端长文：列数绝不越过 14，字号有下限，per_col 至少 1。
        let (fs, c, pc) = note_grid_plan(iw, ih, 100_000);
        assert_eq!(c, 14);
        assert!(fs >= 8.0);
        assert!(pc >= 1);
    }
}
