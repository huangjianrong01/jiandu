//! 阅读视图：古文竖笺（八行、从右到左、朱丝栏）与现代横笺（仅横线）。
//!
//! 标注体系：
//!   * 繁体标注 —— 朱红小字，竖文标字左、横文标字上；
//!   * 通假标注 —— 靛蓝小字，位置同繁体；
//!   * 注释     —— 朱红圈号 ①②③ 标字旁，页脚注释区列出注文；
//!   * 段落分隔 —— 段首空两格（首行缩进），不再放置任何段落标记符。

use crate::app::{
    AppUIState, Root, Screen, ANCIENT_EDGE, ANCIENT_TITLE_COL,
};
use crate::dict;
use crate::gloss::{Page, Piece};
use crate::grid::LeafHalf;
use crate::lines;
use crate::theme::{InkPalette, Theme};
use crate::trad;
use crate::types::{
    is_punct, underline_run_start, Book, CardLayout, Duplex, FontChoice, Glyph, GlyphHit,
    GlyphKind, MarkKind, NoteKind, NoteSource, Script, TocEntry,
};
use gpui::{
    canvas, div, point, prelude::*, px, rgb, rgba, svg, Animation, AnimationExt, AnyElement, App,
    BoxShadow, Bounds, Context, Div, ElementId, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Overflow, ParentElement, PathBuilder, Pixels,
    SharedString, Styled, Window,
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

/// 字书浮签的纸色透明度（v1.43.0）：**0.92**。
///
/// 原先是 0.98，几乎不透光，压在正文上像贴了块白板。降到 0.92 之后
/// 背后的字能**若隐若现**——一张压在书页上的笺本来就该透出底下的字，
/// 但再低释义就发灰了，故九二为界。
const WASHI_ALPHA: u32 = 0xEB;

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
    /// 叶内中缝（对鱼尾）开关，由当前 `LayoutConfig` 决定。
    pub show_fish_tail: bool,
    /// 夹注行内显示（双行小字夹注）；false = 页脚注横排列表。
    pub notes_inline: bool,
    /// 字格 / 注块是否挂鼠标交互（晒本预览关掉：纯展示，不弹工具条）。
    pub interactive: bool,
}

impl LeafGeo {
    /// 当前阅读视图的几何（与 `fit_sizes` 算出的同源）。
    /// 鱼尾开关直接从当前 `LayoutConfig` 取，不再读 `letter_mode` 布尔。
    pub(crate) fn current(r: &Root) -> Self {
        let cfg = r.current_layout();
        Self {
            cell_w: r.col_w,
            cell_h: r.row_h,
            rows: r.grid_rows,
            fs: r.size_ancient,
            show_fish_tail: cfg.show_fish_tail,
            notes_inline: r.notes_inline,
            interactive: true,
        }
    }
}

/// 长按阈值：按住正文超过这么久再抬起，算「长按」——弹气泡菜单
/// （复制 / 检索 / 字典），而不是普通的点字 / 拖选。
const LONG_PRESS: std::time::Duration = std::time::Duration::from_millis(500);
/// **悬停查词阈值**：鼠标在一个字上停这么久，自动浮出字书浮签
/// （与长按同值——两条路手感一致，见 `Root::probe_hover`）。
pub const HOVER_LOOKUP: std::time::Duration = std::time::Duration::from_millis(500);

// ── 天头眉批（v1.37.0）────────────────────────────────────────────
/// 眉批字号 = 正文字号 × 此比例。
///
/// v1.52.0 由 0.40 提到 **0.65**：40% 在 Retina 上只有十来个像素高，笔画
/// 糊成一团像蚂蚁；65% 落在正文的六到七成之间——看得清，又仍比正文矮一档，
/// 不至于压过版框里的正文。
pub const HEAD_NOTE_FS_RATIO: f32 = 0.65;
/// 眉批字号的**硬下限**（px）。
///
/// 窗口缩到很窄时正文会被压小，眉批若等比跟着缩就又变回蚂蚁了——
/// 故此处兜一个 14px：无论正文缩到多小，眉批至少 14px，永远看得清。
pub const HEAD_NOTE_FS_MIN: f32 = 14.0;
/// 眉批的**行间距**倍率（字身 × 此值 = 一格高）。
///
/// 1.06 是「字挨字」，放大后笔画容易糊在一起；1.30 给字间留出一线空隙，
/// 长批语逐字读下来不粘连。
pub const HEAD_NOTE_LH_RATIO: f32 = 1.30;
/// 眉批朱砂的**不透明度**（0x00~0xFF）。
///
/// v1.52.0 由 0xCC 提到 0xF2：字一小，半透明就显得发虚发粉；字放大后
/// 更要压得住，得像真正的朱砂墨迹，而不是淡红影子。留 0xF2 而非全不透明，
/// 是为了让纸纹隐约透上来，仍带一点「写在纸上」的意思。
pub const HEAD_NOTE_INK_ALPHA: u32 = 0xF2;
/// 天头窄于此（px）就不开写：专注模式几乎不留天头，免得点一下就误进输入态。
pub const HEAD_BAND_MIN: f32 = 10.0;
/// 眉批排布的另两个常量（`HEAD_NOTE_STEP` / `HEAD_NOTE_V_TOL`）放在
/// `types.rs` 的 `HeadNote` 旁边——`types` 是 lib 的一部分，而 `reader`
/// 只在 bin 里，放那儿 types 反而够不着。

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

/// 康熙/說文原文是一整条**无换行**的长串（`【唐韻】…【集韻】…　又…），
/// 直接灌进一个 div 就成了「一坨砖」。这里按**书证起手**切段：
///
/// 1. 遇 `　`（原书的段间记号 U+3000）即断；
/// 2. 在【唐韻】【集韻】【說文】等「字书/韵书」起手前断；
/// 3. 随文的引书（【爾雅】【詩·秦風】之类）**不**单独立段——它们本就是
///    在解释上一条义项，断开反而碎；
/// 4. 连续的纯标签孤儿段（`【集韻】` 还没接上正文）并入下一段，免得
///    `【唐韻】【集韻】` 被拆成两截。
fn split_zishu_paragraphs(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    const HEADS: [&str; 11] = [
        "唐韻", "廣韻", "集韻", "韻會", "正韻", "說文", "玉篇", "類篇", "字彙", "正字通",
        "韻補",
    ];
    let chars: Vec<char> = s.chars().collect();
    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{3000}' {
            if !cur.trim().is_empty() {
                segs.push(cur.trim().to_string());
            }
            cur.clear();
            i += 1;
            continue;
        }
        if c == '【' {
            let end = chars[i..].iter().position(|&x| x == '】').map(|p| i + p);
            if let Some(e) = end {
                let name: String = chars[i + 1..e].iter().collect();
                if HEADS.iter().any(|h| name.starts_with(h)) && !cur.trim().is_empty() {
                    segs.push(cur.trim().to_string());
                    cur.clear();
                }
            }
            cur.push(c);
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        segs.push(cur.trim().to_string());
    }
    // 合并纯标签孤儿段（并入紧随其后的那一段）。
    let mut merged: Vec<String> = Vec::new();
    let mut pending = String::new();
    for seg in segs {
        let orphan = seg.starts_with('【')
            && !seg.contains('。')
            && !seg.contains('，')
            && seg.chars().count() <= 10;
        if orphan {
            pending.push_str(&seg);
        } else {
            let mut s = if pending.is_empty() {
                seg
            } else {
                format!("{pending}{seg}")
            };
            pending.clear();
            merged.push(std::mem::take(&mut s));
        }
    }
    if !pending.is_empty() {
        if let Some(last) = merged.last_mut() {
            last.push_str(&pending);
        } else {
            merged.push(pending);
        }
    }
    merged
}

/// **竖排一列**：把一串字自上而下叠成一列（古籍竖排的通用写法）。
///
/// GPUI 0.2.2 没有元素级旋转，故「横排内容竖过来」一律走这里：一字一格
/// 往下叠。视觉上等价于把横排那一行整体旋转 90°——注音、按钮、释文
/// 全用它，签上就不会出现任何一个横排的字。
///
/// 字号由 `fs` 定，格高取 `fs * 1.12`（行距略松，小字也不粘）；返回值
/// 仍需调用方补 `.w()` ——列宽由各家自定（字头宽、注音窄）。
fn vertical_text(s: &str, fs: f32, family: SharedString) -> Div {
    let mut col = div()
        .flex()
        .flex_col()
        .items_center()
        .justify_start()
        .font_family(family)
        .text_size(px(fs));
    for c in s.chars() {
        col = col.child(
            div()
                .h(px(fs * 1.12))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(SharedString::from(c.to_string())),
        );
    }
    col
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

// ---- 版字面板的古风控件（v1.21.0 起）----
//
// 面板原先是一整套「灰底圆角胶囊 + 深墨实底」的现代控件：方块按钮、粗边
// 字牌、42pt 这种现代单位。它贴在古籍正文下方，像一张打印纸压在宋版书页上。
// 现在整块面板改用水墨语汇——**无底、无框、无圆角**，只留一行字与一道
// 极细的线；选中态只做两件事：文字转朱砂、字下压一道 1px 墨线。

/// 面板选中的朱砂（用户指定 #B23A30）：比落款印泥 `CINNABAR` 略沉一档，
/// 落在宣纸上像一枚旧朱印，不刺眼。
const ZHU_BASE: u32 = 0xB23A30;

/// 章题列的字号 / 正文字号：**1.2**（v1.28.0 用户口径「标题字号略大于
/// 正文」，v1.36 再抬一档）。
///
/// 刻本里章题只比正文高一头——再大就成了现代书里的「章标题」；但只放大
/// 一成二又与正文难分，卷首要有一点「尊」。格宽是字号的 1.33 倍，1.2 的
/// 字仍稳稳落在列内，不会探到邻列。
const TITLE_COL_SCALE: f32 = 1.2;

/// **大题（书名）**最多排几字：版心只有一格宽，书名再长也得让位给篇题。
const BIG_TITLE_MAX: usize = 4;
/// 大题的字号 / 小题的字号：**0.92**（大题次尊一档，仍高于正文）。
const BIG_TITLE_SCALE: f32 = 0.92;
/// 小题（篇题）最多排几字。
const SUB_TITLE_MAX: usize = 7;

/// **卷首章题上空几格**（v1.36）：两格。
///
/// 刻本的卷首题名不顶着上鱼尾排，而是低两格起——「空两格另起一列」那份
/// 尊。章题列本就独占一列、通高到版框，空出的两格只是把篇题往下压，
/// 既不占正文的地，也不会挤掉正文（能排几字按扣除后的余高算）。
const TITLE_HEAD_CELLS: f32 = 2.0;

/// 顶栏高度：一处定义——动画位移、触发带回差、命中判定全用它。
pub(crate) const BAR_H: f32 = 44.0;
/// 天头触发带：鼠标进到屏幕顶端这 30px 里，顶栏滑出。
pub(crate) const BAR_HOT: f32 = 30.0;
/// 退出回差：鼠标越过「栏底 + 26px」才收回，免得贴着栏底来回抖。
pub(crate) const BAR_GAP: f32 = 26.0;

/// 顶栏显隐的判据（**纯函数**，只有它决定收 / 放）。
///
/// `y` 是鼠标的窗口纵坐标，`shown` 是当前是否已滑出，`pinned` 表示有面板开着。
/// 关键在于**回差**：收着时门槛是 30px（须探到天头才出），打开时门槛放宽到
/// 「栏底 + 26px」（贴在栏下沿也不算离开）——否则鼠标一离开字面就闪回收起，
/// 点第二下都点不着。
pub(crate) fn bar_wants_show(y: f32, shown: bool, pinned: bool) -> bool {
    pinned
        || if shown {
            y <= BAR_H + BAR_GAP
        } else {
            y <= BAR_HOT
        }
}

/// 面板选中色的**实际取值**：深底主题（夜读）上这枚朱砂会沉进深纸看不见，
/// 故向宣纸白提亮一档——界面色与纸面永远反色，不会出现「暗纸上的暗朱」。
fn sheet_zhu(t: Theme) -> u32 {
    if t.is_dark() {
        crate::theme::mix(ZHU_BASE, 0xF2EAD6, 0.38)
    } else {
        ZHU_BASE
    }
}

/// 极细横线（1px）：选中下划线、行间界、符号线框都用它。
fn hairline(w: f32, color: u32) -> Div {
    div().w(px(w)).h(px(1.0)).bg(rgb(color))
}

/// 极细朱砂**虚线**：GPUI 0.2.2 没有 dashed 边框，用「2 实 2 虚」的短横排出来
/// （与书架页签的 `hair_dashed` 同一手法，这里是朱色版本）。
fn zhu_dashed(w: f32, color: u32) -> Div {
    const DASH: f32 = 2.0;
    const GAP: f32 = 2.0;
    let n = (w / (DASH + GAP)).floor().max(1.0) as usize;
    let mut row = div().flex().flex_row().items_center();
    for i in 0..n {
        row = row.child(div().w(px(DASH)).h(px(1.0)).bg(rgb(color)));
        if i + 1 < n {
            row = row.child(div().w(px(GAP)).h(px(1.0)));
        }
    }
    row
}

/// 面板的「收起」：两字，无底无框，悬停转朱砂。
/// （原来那枚灰底「关闭」按钮，是面板里最像网页控件的一颗。）
fn panel_close(t: Theme) -> Div {
    div()
        .px(px(5.0))
        .py(px(2.0))
        .cursor_pointer()
        .text_size(px(14.0))
        .text_color(rgb(t.ink_soft()))
        .hover(|s| s.text_color(rgb(sheet_zhu(t))))
        .child(SharedString::from("✕"))
}

/// 面板的**选项文字**：整块面板的基本单位。
///
/// `on` 时朱砂字、字下压一道 1px 墨线；否则淡墨字、无线。悬停时淡墨转深墨。
/// 没有任何背景色——点选靠的是「墨色与朱色」的差别，不是色块。
fn ink_option(label: &'static str, on: bool, t: Theme) -> Div {
    let fg = if on { sheet_zhu(t) } else { t.chrome_text() };
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.0))
        .cursor_pointer()
        .text_color(rgb(fg))
        .hover(|s| if on { s } else { s.text_color(rgb(t.ink())) })
        .child(div().text_size(px(12.0)).child(SharedString::from(label)))
        .child(
            div()
                .h(px(4.0))
                .flex()
                .justify_center()
                .child(if on {
                    div()
                        .w(px(4.0))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(rgb(sheet_zhu(t)))
                } else {
                    div().w(px(4.0)).h(px(4.0))
                }),
        )
}

/// 纯文字动作旁的小图标：一律**线框**，不填色。
#[derive(Clone, Copy)]
enum ActionIcon {
    /// 分享图片：一个画框，框底一道地平线。
    Frame,
    /// 分享文字：一枚简册（框内两道竖简）。
    Slips,
}

/// 版字面板底部的**纯文字动作**：一行字 + 一枚 12px 线框符号。
/// 没有背景、没有边框、没有内边距堆出来的「按钮形状」；悬停只加深字色。
fn text_action(label: &'static str, icon: ActionIcon, t: Theme) -> Div {
    let line = ((t.ink() & 0xFFFFFF) << 8) | 0x8C;
    let mut glyph = div()
        .w(px(12.0))
        .h(px(12.0))
        .border_1()
        .border_color(rgba(line))
        .flex()
        .items_end()
        .justify_center()
        .pb(px(1.5));
    match icon {
        ActionIcon::Frame => {
            glyph = glyph.child(div().w(px(5.0)).h(px(1.0)).bg(rgba(line)));
        }
        ActionIcon::Slips => {
            glyph = glyph.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(1.5))
                    .child(div().w(px(1.0)).h(px(5.0)).bg(rgba(line)))
                    .child(div().w(px(1.0)).h(px(5.0)).bg(rgba(line))),
            );
        }
    }
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        .text_size(px(11.5))
        .text_color(rgb(t.ink_soft()))
        .hover(|s| s.text_color(rgb(t.ink())))
        .child(glyph)
        .child(div().child(SharedString::from(label)))
}

/// 注释全局编号：(章, 段, 偏移) -> 序号。
type NoteIndex = HashMap<(usize, usize, usize), usize>;
/// 版心「对鱼尾」（黑鱼尾）：**直线几何多边形**——顶边一刀平直、两侧
/// 竖直下垂成尾翼、中央劈出 V 形凹口，上下两枚**开口相对**。
///
/// 早先是两枚 `<rect>` 实心黑块（33×12），既不合刻本形制，视觉上也极
/// 重；中间一度改用贝塞尔画「燕尾」，画成了圆滚滚的黑桃 / 蝙蝠翅膀。
/// 现在整枚只有 `M` / `L` / `Z`（`banxin::fishtail_svg`），尺寸由
/// `banxin::banxin_tail_size` 算好传进来（v1.48.0：宽 = 列宽 × 0.46、
/// 高 = 0.22 字，凹口 0.76 深——更小更锐）。墨色渲染层给 `#333333`
/// 深灰——黑鱼尾不取纯黑。见 app::ensure_assets。
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

/// 版心折痕：纸**对折**的那道物理褶——0.5px、`#999999`、**实线**，
/// 比界行还淡一档，只作「纸的痕迹」，不抢鱼尾与书口文字。
///
/// GPUI 0.2.2 没有元素级 dashed 边框，故用一串短横拼出来：段长固定，
/// 段距由 `banxin::fold_dashes` 按可用高反算，头尾严丝合缝不留残段。
/// 中缝那道「纸折痕」：**极细的灰色实线**（v1.28.0）。
///
/// 旧版是一串短横拼的虚线（GPUI 0.2.2 没有 dashed 边框）——放大看就是一排
/// 断续的墨点，在中缝里比鱼尾还吵。用户口径改为「极细灰色实线」：0.5px、
/// `#999999`、一通到底，它只是纸的痕迹，不是又一根界栏。
fn fold_crease(x: f32, top: f32, len: f32, color: u32) -> Div {
    div()
        .absolute()
        .top(px(top))
        .left(px(x))
        .w(px(crate::banxin::FOLD_W))
        .h(px(len.max(0.0)))
        .bg(rgb(color))
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

/// 一枚藏印：朱砂红「箋讀」小方印。
///
/// v1.28.0 起不再嵌在版心里（用户：版心只留鱼尾、书口字与叶码），改由
/// `Root::render` 盖在**纸页左下角外侧**——像一册书摊在案上、角落落的
/// 一方藏印，既认得出是谁的本子，又不隔在书口中缝里。
/// 篆字是 `app::ensure_assets()` 写出的两枚 SVG（`seal_jian.svg` /
/// `seal_du.svg`，等粗圆头线稿），这里只用 `text_color` 着朱砂色，
/// 印面自留白（朱文）。
pub(crate) fn banxin_seal(w: f32, h: f32) -> Div {
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

/// 书耳：刻本贴在版框上角的一小片「耳朵」（题签小耳，线装 / 包背装
/// 常见）。样式参照用户给的「天 / 地 / 鱼尾 / 书脑 / 耳子」图：一片竖长
/// 小牌，外侧带线、内侧贴住版框，紧贴左上角不放空。
///
/// **只挂左上角一处**（v1.27.0）：原先对开时右半叶还会镜像出一枚右耳，
/// 两耳说的是同一件事（本章章名），纯属重复——留一枚贴左上即够。
///
/// 耳内竖排题签（篇章名 / 卷名 / 提要，见 `types::ear_label`），
/// 单列自适应收缩——耳片极窄，读的是个「形」与位置。
fn book_ear(t: Theme, w: f32, h: f32, x: f32, top: f32, label: &str) -> Div {
    // 耳内小字：高出一成给行距，宽不超出耳片。
    let fs = (w * 0.62).max(6.5).min(10.0);
    // 耳片只有一列宽：单列自适应（字号随耳高收缩，装不下才截断缀「…」）。
    let (efs, ecol, _cut) = crate::shelf::fit_single_column(label, h - 6.0, fs);
    div()
        .absolute()
        .top(px(top))
        .left(px(x))
        .w(px(w))
        .h(px(h))
        .bg(rgb(t.paper()))
        .border_t_1()
        .border_b_1()
        .border_l_1()
        .border_color(rgb(t.frame_ink()))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .child(crate::shelf::vertical_cols(
            &[ecol],
            efs,
            t.ink_soft(),
            FontWeight::NORMAL,
        ))
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
/// 行款信息（状态栏「半叶八行，行十四字」）与兜底提示也借它转汉字。
pub(crate) fn han_num(n: usize) -> String {
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

/// 叶码小字「第27637葉」：极小**宋体**、纯文本，无边框、无墨签。
///
/// v1.47.0：早先这枚叶码带一圈淡边框 + 起首短竖，做成了一枚「第N葉」小
/// 方块——目录列表里那种黑白方块正是被用户判了死刑的「控制台感」。现在
/// 只留一行小字（仅朱批区的页定位还在用，目录列表已彻底不显示叶码）。
fn leaf_no(page_no: usize, fg: u32, song: &SharedString) -> Div {
    div()
        .font_family(song.clone())
        .text_size(px(9.5))
        .text_color(rgb(fg))
        .child(SharedString::from(format!("第{page_no}葉")))
}

/// 朱批录里的一条：带跳转锚点（`key`）或兜底页码（`page`）。
struct NoteEntry {
    /// 章 · 段 · 偏移三件套锚点；`None` = 页笔记（无 charIndex）。
    key: Option<(usize, usize, usize)>,
    /// 锚点跨度（个字数），闪高时点亮这一串。
    len: usize,
    /// 兜底页码（页笔记用）。
    page: Option<usize>,
    /// 品类标签：眉批 / 夹注 / 通假 / 专名 / 地名 / 书名 / 着重 / 划线 / 页记。
    kind: &'static str,
    /// 原文引用（朱砂细线旁的那一句）。
    quote: String,
    /// 批注内容（眉批 / 夹注正文）。
    content: String,
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

/// 对开两叶的纸面**左右**留白（Invariant 2：两叶必须一丝不差等宽）。
///
/// 内侧（贴中缝那一侧）收掉、外侧留 `pad`——左右叶严格镜像：
/// `Left = (pad, 0)`、`Right = (0, pad)`，故两叶纸面总宽恒等，
/// 中缝因此钉在整张纸的正中央（引擎侧的等分证明见
/// `grid::frame_tests::facing_split_is_symmetric_and_the_gutter_sits_on_the_center_line`；
/// 渲染侧的镜像由 `tests::facing_sheets_are_mirrored_so_the_two_leaves_are_equal_width`
/// 把着）。单叶（`Alone`）两侧对称留白，居中独摆。
pub(crate) fn sheet_side_pads(half: LeafHalf, pad: f32) -> (f32, f32) {
    match half {
        LeafHalf::Right => (0.0, pad),
        LeafHalf::Left => (pad, 0.0),
        LeafHalf::Alone => (pad, pad),
    }
}

/// **列数的唯一数据源**（v1.35.0）：UI 只认这张表，底层只收整数 / `None`。
///
/// 刻本按「一叶几行」称呼开本，故文案是「八行」这类古称，值才是引擎要的
/// 列数；`None` = 自动（按这本书平均一章多少字 + 当前可用宽每帧换挡，见
/// `grid::auto_columns`）。
///
/// 去重记录：原先「版式」浮层遍历 `grid::LAYOUT_CONFIGS` 出四枚档位按钮、
/// 版字面板又写死一份 `COL_OPTS`，两处同时改 `columns_auto` / `grid.columns`
/// ——同一件事两副面孔。现在只剩这一张表：读写都过
/// `Root::columns_pref` / `Root::set_columns_pref`。
pub(crate) const COLUMN_OPTS: [(&str, Option<usize>); 5] = [
    ("八行", Some(8)),
    ("十行", Some(10)),
    ("十二行", Some(12)),
    ("十四行", Some(14)),
    ("自动", None),
];

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
            false, // 信笺模板已删除：不再有「叶内宽中缝」这一种版心
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
        // 自动列数刚换过档（拉窗口跨了档）：几何已更新，此刻才能按新排版
        // 把阅读位置找回来，免得「拉一下窗口就跳到别处」。
        if let Some(anchor) = self.pending_anchor.take() {
            self.restore_anchor(Some(anchor));
            self.sync_current_chapter();
        }
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
                row = row.child(self.render_leaf(&pages, next, false, LeafHalf::Left, cx));
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
                let edge = if next < total { LeafHalf::Right } else { LeafHalf::Alone };
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
                .child(self.render_leaf(&pages, self.page, false, LeafHalf::Alone, cx))
        };

        let toolbar = self.render_reading_toolbar(total, cx);

        // 导出探针（v1.35.0）：一枚**什么都不画**的隐形 canvas，尺寸与纸面
        // 严丝合缝，只在 prepaint 时把「当帧纸面在窗口内容区里的矩形」记下来，
        // 供「印制笺卡（PNG）」按此范围截图。
        //
        // 它是渲染的**输出**而不是输入——不绘制像素、不读回任何值、不改
        // 任何排版坐标；父层只是多包一层 `relative`（尺寸由纸面撑出，与原来
        // 的 stage 完全同尺寸），布局看不出差别。
        let probe = self.page_rect.clone();
        let mut stage = div()
            .relative()
            .child(stage)
            .child(
                canvas(
                    move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| {
                        let unit = px(1.0);
                        probe.set(Some((
                            bounds.origin.x / unit,
                            bounds.origin.y / unit,
                            bounds.size.width / unit,
                            bounds.size.height / unit,
                        )));
                    },
                    |_bounds, _state, _window, _cx| {},
                )
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0)),
            );
        // ★ 天头眉批：铺在纸面天头带里的一层（绝对定位，不参与任何排版）。
        //   必须挂在**纸面这一层**上：它的原点就是 `page_rect` 的左上角，
        //   眉批的 `u/v` × 当帧天头带 = 屏幕坐标，拖窗口 / 换留白档自动跟随。
        stage = stage.child(self.render_head_notes(cx));

        // 正文区：单页 / 对开舞台居中铺满。版字面板改为浮层（见下方 root.overlay），
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
        let mut body = div().flex_1().flex().flex_col().items_center().justify_start();
        if self.focus_mode {
            body = body.p_0();
        } else {
            body = body.p_4();
        }
        let body = body
            .capture_any_mouse_down(cx.listener(
                move |this, event: &MouseDownEvent, _window, _cx| {
                    let (px_, py_) = (
                        event.position.x.to_f64() as f32,
                        event.position.y.to_f64() as f32,
                    );
                    // ★ 天头眉批：这一下按在版框上方那段留白里吗？
                    //   命中就记下归一化 (u, v)，抬起时**就地开写**而不翻页。
                    //   记在 capture 阶段（先于字格处理器），落点不会有遗漏。
                    this.head_press = this.head_band_hit(px_, py_);
                    this.press_at = Some((px_, py_));
                    // 记下按下时刻：抬起时据此判定「长按」（弹气泡菜单）。
                    this.press_time = Some(std::time::Instant::now());
                    this.touched_glyph = false;
                },
            ))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseUpEvent, _window, cx| {
                        let (ux, uy) = (
                            event.position.x.to_f64() as f32,
                            event.position.y.to_f64() as f32,
                        );
                        // ★ 天头眉批（v1.37.0）：按下落在天头 → 抬起就地开写。
                        //   必须排在最前：天头那一下不能翻页、不能弹工具条。
                        //   已在写另一条时，先落笔存好手上这条，再在新位置起一条。
                        if let Some((u, v)) = this.head_press.take() {
                            if this.note_editing && this.head_anchor.is_some() {
                                this.commit_note_edit();
                            }
                            this.open_head_note(u, v);
                            cx.notify();
                            return;
                        }
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
                            // （v1.49.1）长按不再自动查词——与拖选 / 点字一致，只
                            // 呼出「选词工具条」（横排竖字那一枚），字典交由工具条
                            // 「查典」按钮呼出。本字或拖选段都认：取当前选区首字。
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
                            // 同一时刻只留一层浮签：长按不再开字典，旧字典签先收掉。
                            this.bubble = None;
                            this.close_lookup();
                            this.dragging = false;
                            cx.notify();
                            return;
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
        // 顶栏「触天头即出」——**由鼠标位置驱动，不铺一条 30px 的命中带**。
        //
        // 为什么不在天头摆一张透明的触发条：那会在纸面上开一条 30px 的死区，
        // 顶格的字点不动、翻页也失灵。改成在根元素上听 `mousemove`，只算 y：
        //   · 收着时：进到 `BAR_HOT` 以内 → 滑出；
        //   · 打开时：越过「栏底 + `BAR_GAP`」→ 收回（回差，防贴边抖动）；
        //   · 任一面板开着（`bar_pinned`）→ 一律留住。
        // root 铺满整窗、又是冒泡链末端，鼠标在纸面任何一处都会到这里。
        root = root.on_mouse_move(cx.listener(
            |this, event: &MouseMoveEvent, _window, cx| {
                let y = event.position.y.to_f64() as f32;
                let want = bar_wants_show(y, this.bar_shown, this.bar_pinned());
                if want != this.bar_shown {
                    this.bar_shown = want;
                    cx.notify();
                }
                // 悬停查词：鼠标停在一个字上满 500ms 就浮出字书浮签。
                // 只在**非专注模式**生效（专注模式连顶栏都收了，不该冒签），
                // 且排在顶栏显隐之后——两者都吃这一次 mousemove，互不干扰。
                if !this.focus_mode {
                    let x = event.position.x.to_f64() as f32;
                    this.probe_hover(x, y, cx);
                }
            },
        ));
        // 键盘总入口不挂在这里：GPUI 只把按键派发给「焦点节点 → 根元素」这条
        // 路径上的监听器，而本函数是根元素的子层（见 app.rs 的 Render::render）。

        if !self.focus_mode {
            // 顶栏是**浮层**（绝对定位、贴顶、默认收到天头外），所以排在正文
            // 之后画、盖在纸面之上；正文因此能一直铺到屏幕顶端。
            root = root
                .child(body)
                .child(self.render_reading_status(total, cx))
                .child(toolbar);
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
        // 版字面板浮层：点工具栏「样式」展开，宽度与单张页面一致，
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
        // （原「版式控制面板」浮层 v1.35.0 已删除：那四枚 8/10/12/14 档位与
        // 版字面板的「列数」是同一件事的两副面孔，现统一并入「版字」页的
        // `COLUMN_OPTS`；留白两行也一并搬过去了。）
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
        // 左上角那枚独立的书名签条（v1.27.0 删除）：它离书页有一段距离，
        // 且与版框上的书耳、顶栏的「目录」重复——书名与卷目入口都不缺，
        // 只剩一块飘在留白里的牌子碍眼。书耳仍在（左半叶左上角）。

        // 开发用：`JIANDU_LOOKUP=1` 时开卷自动对叶面上第一个命中的字开一次
        // 查词浮签——与 `JIANDU_HEAD_NOTE` 同款调试开关，便于核对浮签版式
        // 与避让，不影响正常交互（只在 `bubble` 为空时生效一次）。
        self.dev_lookup_once();
        self.dev_menu_once();

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
        // 天头眉批复用同一套编辑态，但它**就地竖排**显示在天头上
        // （`render_head_notes`），不能再压一张浮层卡片上去。
        if self.note_editing && self.head_anchor.is_none() {
            root = root.child(self.render_note_editor(cx));
        }

        // 选词工具条（横排竖字那一枚）：必须画在**所有阅读浮层之后**——
        // 即最高 z-index，绝不被字典卡 / 注文卡 / 笔记框盖住（v1.49.1 修复
        // 「拖选后字典把工具条吃掉、工具条出不来」的根因）。互斥：任一顶层面板
        // 开着就强制隐藏，防止事件穿透误触。
        if self.popup.is_some() && !self.any_overlay_open() {
            root = root.child(self.render_glyph_menu(cx));
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
                            // 全文检索：⌘F（与顶部「檢索」同款，走状态机开合）。
                            "f" | "F" => {
                                self.set_ui_state(AppUIState::Search);
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
                            // 版字面板：⌘E（走全局状态机：开版字即关其它面板与浮层）。
                            "e" | "E" => {
                                self.set_ui_state(AppUIState::Style);
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
                            // 专注模式：⌘Z（收起样式 / 目录 / 帮助，进入时清底层浮层）。
                            "z" | "Z" => {
                                self.focus_mode = !self.focus_mode;
                                self.show_style = false;
                                self.close_toc();
                                self.show_help = false;
                                if self.focus_mode {
                                    self.close_transient_ui();
                                }
                                cx.notify();
                            }
                            // 笔记面板（侧栏）：⌘K（走全局状态机）。
                            "k" | "K" => {
                                self.set_ui_state(AppUIState::Notes);
                                cx.notify();
                            }
                            // 帮助：⌘/（走全局状态机）。
                            "/" => {
                                self.set_ui_state(AppUIState::Help);
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

    /// 目录「竖排卷帘」（v1.51.1）：从底部升起的**半屏竖排抽屉**。
    ///
    /// 全程只用 GPUI 原生 `div()` / `flex()` / `absolute()` / `px()`，
    /// 一条边框都不描、一枚假鱼尾都不画——上一版给每卷套了左右界行和上下
    /// 三角，294 卷并排就是一片条形码，正是要彻底删掉的东西。
    ///   * **抽屉**：宣纸底、固定 400 高、贴屏底，顶上只留一条极细淡墨分割线；
    ///   * **卷名**：逐字一格竖排（`flex_col`），从右往左一枚枚排开，列距 24；
    ///   * **朱砂**：当前在读的那一卷字色转朱砂，其余保持墨色；
    ///   * **顶栏**：正中一行极小字「共 N 卷」，右上角一枚手绘的极简 X。
    #[allow(non_snake_case)]
    fn render_toc_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let Some(book) = self.book.as_ref() else {
            return div();
        };
        let t = self.theme;
        // 抽屉底：宣纸 + alpha 0xE6（约 90%）——磨砂半透的宣纸色，不做死白网页。
        let DRAWER_BG: u32 = (t.paper() << 8) | 0xE6;
        // 顶上那条分割线：淡墨兑到很浅，只起「抽屉是另一张纸」的提示作用。
        let EDGE: u32 = (t.rule() << 8) | 0x5A;
        let INK: u32 = t.ink();
        let INK_SOFT: u32 = t.ink_soft();
        // 朱砂：rgb(178, 58, 48)——当前卷专用，其余卷一律墨色。
        const VERMILION: u32 = 0xB23A30;

        let song = crate::fonts::family_for(FontChoice::SongTi, Script::Ancient);

        // 目录条目先克隆出来：后面要用 &mut self 构建抽屉（跳页要改 self），
        // 不能一直借着 self.book。
        let toc: Vec<TocEntry> = book
            .toc
            .iter()
            .filter(|e| e.level <= 3)
            .cloned()
            .collect();
        // 当前章：用后台悄悄对齐好的 current_chapter，朱砂落在准确卷目上。
        let cur_chapter = self.current_chapter;

        // 固定高 400；slide 1 时底边齐屏底，0 时整块落在屏外。
        const H: f32 = 400.0;
        const HEAD_H: f32 = 34.0;
        let slide = self.toc_slide.clamp(0.0, 1.0);
        let off = -(1.0 - slide) * H;
        // 上半屏遮罩随 slide 淡入：最重也只有 42% 黑，别把书页糊死。
        let mask: u32 = (0x000000 << 8) | ((0.42 * slide * 255.0) as u32).min(255);

        const FS: f32 = 15.0; // 卷名字号
        const PITCH: f32 = 22.0; // 一格高（字身 + 行气）
        // 一列最多排几个字：抽屉高扣掉顶栏、上留白 12、下留白 28 再按格取整。
        // 上一版扣得太松（只留 40），字数算多一格，末字正好被窗口底边切掉。
        let per_col = (((H - HEAD_H - 56.0) / PITCH).floor() as usize).clamp(3, 20);

        // ── 顶栏：正中一行极小字「共 N 卷」，右上角一枚手绘 X ──
        // X 用 canvas 画两笔，不用 ✕ 字符（U+2715 在中文回退字体里零宽消失）。
        let close_x = canvas(
            |_b: Bounds<Pixels>, _w: &mut Window, _c: &mut App| (),
            move |bounds: Bounds<Pixels>, _s, window: &mut Window, _c: &mut App| {
                let o = bounds.origin;
                let side = px(11.0);
                let x0 = o.x + px(2.5);
                let y0 = o.y + px(2.5);
                let mut p = PathBuilder::stroke(px(1.0));
                p.move_to(point(x0, y0));
                p.line_to(point(x0 + side, y0 + side));
                p.move_to(point(x0 + side, y0));
                p.line_to(point(x0, y0 + side));
                if let Ok(path) = p.build() {
                    window.paint_path(path, rgb(INK_SOFT));
                }
            },
        )
        .size_full();

        let head = div()
            .relative()
            .w_full()
            .h(px(HEAD_H))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .font_family(song.clone())
                    .text_size(px(9.5))
                    .text_color(rgb(INK_SOFT))
                    .child(SharedString::from(format!("共 {} 卷", toc.len()))),
            )
            .child(
                div()
                    .absolute()
                    .right(px(14.0))
                    .top(px((HEAD_H - 16.0) / 2.0))
                    .w(px(16.0))
                    .h(px(16.0))
                    .cursor_pointer()
                    .child(close_x)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_toc();
                            cx.notify();
                        }),
                    ),
            );

        // ── 卷名行：纯竖排文字，从右往左，横向滚动 ──
        let mut row = div()
            .id(SharedString::from("toc-drawer-row"))
            .flex()
            // **row-reverse**：第一卷落在最右，往后一枚枚往左排——古籍读序。
            .flex_row_reverse()
            .items_start()
            .gap(px(24.0))
            .px(px(20.0))
            .pt(px(12.0))
            .pb(px(20.0))
            .flex_1()
            .min_h(px(0.0))
            .overflow_x_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.toc_scroll);

        if toc.is_empty() {
            row = row.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .pt(px(18.0))
                    .font_family(song.clone())
                    .text_size(px(12.0))
                    .text_color(rgb(INK_SOFT))
                    .child(SharedString::from("这本书没有目录")),
            );
        }

        for e in toc.iter() {
            let cur = e.chapter == cur_chapter;
            let jump = e.start_char_index.unwrap_or(0);
            let fg = if cur { VERMILION } else { INK };
            let shown = Self::toc_label(&e.title, per_col);

            // 一列就是一个卷名：逐字一格，自上而下，**没有任何边框**。
            let mut vcol = div().flex().flex_col().items_center();
            for c in shown.chars() {
                // 竖排标点（直角引号 / 书名号 / 居中省略号）随目录字号缩一档。
                let mark = matches!(c, '「' | '」' | '︽' | '︾' | '︙');
                let size = if mark { FS * 0.8 } else { FS };
                vcol = vcol.child(
                    div()
                        .w(px(FS + 4.0))
                        .h(px(PITCH))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(song.clone())
                        .text_size(px(size))
                        .when(cur && !mark, |d| d.font_weight(FontWeight::BOLD))
                        .text_color(rgb(fg))
                        .child(SharedString::from(c.to_string())),
                );
            }
            vcol = vcol
                .flex_shrink_0()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.goto_char_index(jump);
                        this.close_toc();
                        cx.notify();
                    }),
                );
            row = row.child(vcol);
        }

        // 自动定位：开目录时滚到**当前在读的那卷**——294 卷的目录若永远从
        // 第一卷起手，读到后半本的人每次都得横滚半天才找得到自己。
        // `scroll_to_item` 只置一次「活动项」，真正落地在下一帧 prepaint
        // （子项 bounds 出来之后），故开一次目录只需置一次。
        if self.toc_need_scroll {
            let pos = toc
                .iter()
                .position(|e| e.chapter == cur_chapter)
                .unwrap_or(0);
            self.toc_scroll.scroll_to_item(pos);
            self.toc_need_scroll = false;
        }

        // 竹简目录：宽度锁定屏幕 61.8% 黄金比、水平居中。窗口拉伸时随视口
        // 每帧重算，永不变形（见下方 `panel_w` / `panel_left`）。
        let panel_w = vw * 0.618;
        let panel_left = (vw - panel_w) / 2.0;
        let drawer = div()
            .absolute()
            .left(px(panel_left))
            .w(px(panel_w))
            .bottom(px(off))
            .h(px(H))
            .flex()
            .flex_col()
            .bg(rgba(DRAWER_BG))
            // 悬浮质感：8px 圆角 + 极弱黑色投影，像一张浮在屏底的纸条。
            .rounded(px(8.0))
            .shadow(vec![gpui::BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.12),
                offset: point(px(0.0), px(3.0)),
                blur_radius: px(12.0),
                spread_radius: px(0.0),
            }])
            .border_t_1()
            .border_color(rgba(EDGE))
            .overflow_hidden()
            // 拦住冒泡：点抽屉内不该顺下去触发外层遮罩的「关闭」或翻页。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(head)
            .child(row);

        // 上半屏遮罩：点空白处平滑收起抽屉（透明度随 slide 淡入）。
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
            .child(drawer)
    }

    /// 目录题名的**竖排净化**（v1.49.3）：先剥书名号空壳、砍掉括注，再把
    /// 现代标点换成竖排形（黑括号→直角引号、书名号→︽︾、省略号→︙），
    /// 最后按简条能容纳的字数截断——宁可尾巴断字，也不让长题名撑破简条。
    fn toc_label(title: &str, max: usize) -> String {
        let raw = crate::types::strip_empty_title_marks(title);
        // 括注（卷次 / 篇名）一律砍掉：竖排简条只有一格宽，塞不下。
        let cut = raw
            .find(|c| matches!(c, '（' | '(' | '〔'))
            .unwrap_or(raw.len());
        let raw = &raw[..cut];
        let mut out = String::new();
        for c in raw.chars() {
            match c {
                '【' | '“' | '“' => out.push('「'),
                '】' | '”' | '”' => out.push('」'),
                '《' | '〈' => out.push('︽'),
                '》' | '〉' => out.push('︾'),
                '…' | '⋯' => out.push('︙'),
                _ => out.push(c),
            }
        }
        let s = out.trim();
        if s.chars().count() > max {
            let mut r: String = s.chars().take(max.saturating_sub(1)).collect();
            r.push('︙');
            r
        } else {
            s.to_string()
        }
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


    /// 任一面板开着 → 顶栏必须留住。
    ///
    /// 面板都从底边浮起，鼠标一移下去就把顶栏收走，用户会以为「点了没反应」；
    /// 所以「有门开着」时显隐不受鼠标位置支配。分享卡是整屏深底导出界面，
    /// 不算「开着的门」——那时顶栏该退场。
    fn bar_pinned(&self) -> bool {
        self.show_toc
            || self.show_style
            || self.show_search
            || self.show_notes_panel
            || self.show_help
    }

    /// 顶栏（天头那张薄纸）的渲染。
    ///
    /// v1.26.0「沉浸式古风化」以后，它不再是布局里的一条横栏，而是**浮在
    /// 纸面上的一张半透明宣纸**：默认收到天头之外（`top = -BAR_H`），
    /// 鼠标触到天头 30px 才滑下来，读完即退——把整幅纸面还给正文。
    ///
    /// 三组纯文字：书阁 · 目录 ｜ 版字 · 专注 ｜ 检索 · 朱批 · 帮助。
    ///
    /// v1.37.0「UI 瘦身」撤掉了「封面」一枚：书衣看点在书架点书那一刻，
    /// 读着读着没人想回封面，它只是白占天头一条横向空间。
    /// 没有图标、没有底色块、没有底线：字本身就是按钮。
    fn render_reading_toolbar(&mut self, total: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let zhu = sheet_zhu(t);
        let family = self.main_family();
        // 半透明宣纸底：GPUI 0.2.2 没有 `backdrop-filter`，磨砂只能用
        // 「纸色 90% 不透明」近似——书页的墨从底下隐约透出一点，字仍看得清。
        //（深底主题自动取深纸，不必另写一套。）
        let veil = ((t.paper() & 0xFFFFFF) << 8) | 0xE6;
        // 组间界行：墨 10%——`rgba(0,0,0,.1)` 落在纸面上的等价物。
        let sep_ink = ((t.ink() & 0xFFFFFF) << 8) | 0x1A;

        // 一枚纯文字：无底、无框、无图标。悬停转朱砂；`on`（面板开着）时
        // 常驻朱砂——这是「此门已开」的唯一提示，不另加下划线或色块。
        // GPUI 0.2.2 无 letter-spacing，字距只能靠 12px 的内边距撑开。
        let item = |label: &'static str, on: bool| -> Div {
            let fg = if on { zhu } else { t.ink() };
            div()
                .h(px(BAR_H))
                .flex()
                .items_center()
                .px(px(12.0))
                .cursor_pointer()
                .text_size(px(13.0))
                .text_color(rgb(fg))
                .hover(move |s| s.text_color(rgb(zhu)))
                .child(SharedString::from(label))
        };
        // 组间竖线：13px 高的一道极细界，不贯通到顶底（贯通就成了表格）。
        let sep = || -> Div {
            div()
                .w(px(1.0))
                .h(px(13.0))
                .ml(px(17.0))
                .mr(px(17.0))
                .bg(rgba(sep_ink))
        };

        // ---- 左组：书阁（回书架） / 目录（卷目） ----
        let mut left = div().h(px(BAR_H)).flex().items_center().pl(px(26.0));
        left = left.child(item("书阁", false).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.back_to_shelf();
                cx.notify();
            }),
        ));
        left = left.child(item("目录", self.show_toc).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                // 展开 / 收起侧栏（带滑入滑出动画）。
                cx.stop_propagation();
                if this.show_toc {
                    this.close_toc();
                } else {
                    this.open_toc();
                }
                cx.notify();
            }),
        ));
        // （v1.37.0：此处原有一枚「封面」，已删——见本函数头注释。）
        left = left.child(sep());

        // ---- 中组：版字（列数 / 字体 / 字级 / 留白 / 纸墨 / 标注） / 专注 ----
        //
        // v1.35.0：原先的「版式」浮层、「字体」页、「样式」页三处合成
        // **一张纸、一枚入口**——点「版字」开合，不再有两处各开一个浮层、
        // 也不再有「当前停在哪页」这个状态。
        let mut mid = div().h(px(BAR_H)).flex().items_center();
        mid = mid.child(item("版字", self.show_style).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.set_ui_state(AppUIState::Style);
                cx.notify();
            }),
        ));
        mid = mid.child(item("专注", false).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.focus_mode = !this.focus_mode;
                if this.focus_mode {
                    this.close_transient_ui();
                }
                cx.notify();
            }),
        ));
        mid = mid.child(sep());

        // ---- 右组：检索 / 朱批（笔记） / 帮助 ----
        let mut right = div().h(px(BAR_H)).flex().items_center().pr(px(26.0));
        right = right.child(item("检索", self.show_search).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                // 走全局状态机：开检索即关其它面板与底层浮层；再点一次收起。
                this.set_ui_state(AppUIState::Search);
                if this.show_search {
                    // 焦点交给检索框，否则输入法没有落点、打不进中文。
                    window.focus(&this.search_focus);
                }
                cx.notify();
            }),
        ));
        right = right.child(item("朱批", self.show_notes_panel).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.set_ui_state(AppUIState::Notes);
                cx.notify();
            }),
        ));
        right = right.child(item("帮助", self.show_help).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.set_ui_state(AppUIState::Help);
                cx.notify();
            }),
        ));
        // v1.52.0：**导出**下拉。三件事（印制笺卡 PNG / 全书 PDF / 导出 MD）
        // 原先散在底部状态栏，天头一收就找不着了；这里收成一枚「导出」，
        // 菜单里逐条列出，点哪条都直接弹系统保存对话框。
        right = right.child(item("导出", self.export_menu).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.export_menu = !this.export_menu;
                cx.notify();
            }),
        ));

        let mut bar = div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .h(px(BAR_H))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .bg(rgba(veil))
            .font_family(family)
            .text_color(rgb(t.ink()));

        // 三列等分：左组贴左、中组绝对居中、右组贴右——中组不会被左右两组的
        // 宽度推歪（三格 `flex_1` 而不是 `space-between`）。
        bar = bar
            .child(
                div()
                    .flex_1()
                    .h(px(BAR_H))
                    .flex()
                    .items_center()
                    .justify_start()
                    .child(left),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(BAR_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(mid),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(BAR_H))
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(right),
            );

        // 滑入 / 滑出：GPUI 0.2.2 只有元素级动画（没有 CSS transition），且
        // 动画状态按元素 id 缓存——所以显隐各用一个 id，id 一变动画就从头播。
        let shown = self.bar_shown;
        let anim = Animation::new(std::time::Duration::from_millis(if shown { 220 } else { 160 }))
            .with_easing(move |x: f32| {
                if shown {
                    1.0 - (1.0 - x).powi(3) // 出场：先快后慢，像纸片落上案头
                } else {
                    x * x // 收回：先慢后快，收起时不拖泥带水
                }
            });
        let anim_id =
            ElementId::NamedInteger(SharedString::from("jd-topbar"), if shown { 1 } else { 0 });

        let _ = total;
        bar.with_animation(anim_id, anim, |d: Div, v: f32| {
            // v: 0 → 1。shown 时由 -BAR_H 落回 0；hidden 时由 0 收进 -BAR_H。
            d.top(px(-BAR_H * (1.0 - v)))
        })
        .into_any_element()
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
        edge: LeafHalf,
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
        edge: LeafHalf,
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
            // 最左一列（i == text_cols-1）贴版框左缘：它的左界行由 inner
            // 的闭合细线（`border_l_1`）充当，自己不再画——否则两条 1px
            // 并成 2px，乌丝栏就不再根根一样粗（v1.34.0 闭合版框的收尾）。
            let first_col = i == text_cols - 1;
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
                first_col,
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

        // ════════════════════════════════════════════════════════════
        // v1.34.0 三大前提（Invariants）——用户点名「绝对不许破坏」。
        // 动下面这段边框代码之前，先看一遍；跑测试先跑
        // `grid::frame_tests` 与 `punct` 两个模块：
        //
        //   ① 天宽地窄：纸面的 `pt` / `pb` 就是引擎的 `margin_top` /
        //      `margin_bot`，而 `margin_bot = margin_top ÷ 1.5`（用户铁律，
        //      `grid::TOP_OF_BOT`）。**加粗外框（1→3px）只长在框上，
        //      不许多吃一寸天头、不许把上下拉成对称。**
        //   ② 绝对居中：左右叶的左右留白走 `sheet_side_pads`（严格镜像），
        //      两叶纸面总宽恒等；中缝宽 = `grid::gutter_w`（引擎同一算式），
        //      故中缝钉在屏幕正中。
        //   ③ 句读不被盖：`punct_overlay` 是字格的子元素，而字格 ⊂ 列 ⊂
        //      grid ⊂ inner ⊂ frame ⊂ sheet——GPUI 先画祖先后画子孙，
        //      边框是祖先自身的一部分，**在渲染顺序上不可能盖住句读**；
        //      下面的边框循环只碰 `border_*`，绝不重建 `grid` 的子元素树。
        // ════════════════════════════════════════════════════════════
        //
        // 版框：由配置驱动——single = 单线；double = 外粗内细（文武边栏）。
        // 文武的内层细线只在 double 时画；single 只需最外一根。
        // **四边一律闭合**（v1.34.0）：对开两叶贴中缝的内侧缘（左叶右缘 /
        // 右叶左缘）也设框线——左右两叶拼成一整张纸时，外框围住整页外缘、
        // 内侧各一道粗竖框把中缝夹住；单叶（Alone）同样四边全封闭。
        // 内层 `relative`：地脚 / 卷首题署都用绝对定位叠在行格之上。
        let mut inner = div()
            .relative()
            .border_color(rgb(t.frame_ink()))
            .child(grid)
            .child(foot);
        // **内框线（文武边栏的内层细线）**：与列间界行（乌丝栏）同一条——
        // 同色、同粗（1px）、同起止，看上去就是「界行围了一圈」。
        // **四边一律闭合**（v1.34.0，`grid::frame_sides`）：对开两叶贴中缝
        // 的内侧缘（左叶右缘 / 右叶左缘）也必须画——此前内侧留空，中缝像
        // 一条孤立的马路悬在中央（用户实测）。界行的上下两端因此接到
        // 横框上，字格围成完全封闭的网格。
        let sides = crate::grid::frame_sides(edge);
        if sides[0] {
            inner = inner.border_t_1();
        }
        if sides[1] {
            inner = inner.border_r_1();
        }
        if sides[2] {
            inner = inner.border_b_1();
        }
        if sides[3] {
            inner = inner.border_l_1();
        }
        inner = inner.border_color(rgb(t.rule_by(self.rule_color)));
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
        // **外层框线一律加粗（3px）**：8 / 10 / 12 / 14 列都是「外粗内细」
        // 的文武边栏——外框压得住整版，内框与界行同粗，层次才立得住。
        // **四边一律闭合**（v1.34.0）：左半叶的右缘、右半叶的左缘各立一道
        // 粗竖框，两线分居中缝两侧，把鱼尾、书名、叶码死死夹在中间——
        // 中缝不再是一条悬空的「马路」。
        if sides[0] {
            frame = frame.border_t_3();
        }
        if sides[1] {
            frame = frame.border_r_3();
        }
        if sides[2] {
            frame = frame.border_b_3();
        }
        if sides[3] {
            frame = frame.border_l_3();
        }

        // 书耳：贴版框上角的一小片「耳朵」（刻本书脑侧的题签小耳）。
        //
        // **只贴在左上角**（v1.27.0 用户口径）：对开时右半叶原先会镜像出一枚
        // 右耳，两耳同说「本章章名」，是重复的功能，故右半叶不再挂耳——
        // 一叶摊开只在书页左上角留一枚。
        //
        // v1.32.0 行款重构后，版心（界行区）窄于纸面、在叶内居中——书耳
        // 若是纸面的子元素就会漂在留白里，与版框脱节。故改挂到 **frame**
        // （版框自身）上：left/top 均为 0，即「版框左上角」，框宽耳随。
        if edge != LeafHalf::Right {
            let ear_w = (cell_w * 0.16).clamp(11.0, 14.0);
            // 耳高约一格（刻本书耳只有一两字高，不是一整条边）。
            let ear_h = (cell_h * 1.25).clamp(34.0, 150.0);
            // 耳内题签按内容自适应：篇章名 / 卷名 / 提要（types::ear_label）。
            let ear_label = self.ear_label(chapter);
            frame = frame.child(book_ear(t, ear_w, ear_h, 0.0, 0.0, &ear_label));
        }

        // 纸面留白：对开时内侧（贴纸心那一侧）收掉，两半才能拼成一张纸。
        // `relative`：书耳 / 地脚牌记用绝对定位挂在纸面角上。
        //
        // 天头地脚非对称（v1.31.x）：阅读视图用网格引擎算出的 `margin_top`
        // （天头，宽）/ `margin_bot`（地脚，窄）作上下内边距，并把纸张高度
        // 锁死为 `content_h`，强制顶部对齐——无论屏幕多大，天头永远是地脚
        // 的 ~2 倍，绝不再靠垂直居中把非对称留白藏起来。分享 / 长图(非交互)
        // 维持对称 `pad` 与自适应高度，避免破坏导出尺寸。
        let pad = 16.0;
        let top_margin = if geo.interactive { self.margin_top } else { pad };
        let bot_margin = if geo.interactive { self.margin_bot } else { pad };
        // 左右留白走 `sheet_side_pads`（Invariant 2：左右叶镜像 → 等宽）。
        let (pl, pr) = sheet_side_pads(edge, pad);
        let mut sheet = div()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(top_margin))
            .pb(px(bot_margin))
            .pl(px(pl))
            .pr(px(pr))
            .bg(rgb(paper))
            .child(frame);
        // 纸张高度锁死为内容区高：顶对齐后整张纸撑满正文区，天头 / 地脚就是
        // 引擎算出的 `margin_top` / `margin_bot`（非交互的导出图保持自适应）。
        if geo.interactive {
            sheet = sheet.h(px(self.content_h.max(120.0)));
        }
        // 地脚牌记：天头宽、地脚窄，地脚那点留白缀一行极小宋体版权页
        // （「笺读数字刻本」），优雅地把底部空间利用起来，而不是靠居中去遮挡。
        if geo.interactive {
            sheet = sheet.child(
                div()
                    .absolute()
                    .bottom(px(4.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from("笺读数字刻本")),
                    ),
            );
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
    /// `first_col` = 是否是**最左一列**（贴版框左缘）：它的左界行由
    /// `inner` 的 `border_l_1` 充当（v1.34.0 四边闭合后那条线必然存在），
    /// 自己再画一道就并成 2px——乌丝栏必须根根一样粗。
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
        // 是否最左一列（贴版框左缘）：为真则不画左界行（由 inner 的
        // 闭合细线充当），免得两条 1px 并成 2px。
        first_col: bool,
        note_index: &NoteIndex,
        // 字格 / 注块是否挂鼠标交互（晒本预览传 false：纯展示）。
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        // 列外框（含乌丝栏界行）高度固定，与邻列对齐；顶部预留带用内边距把
        // 正文内容压到带下，预留带内仍是空白纸面，留给卷首题署叠绘。
        let mut col = div()
            .w(px(cell_w))
            .h(px(cell_h * rows as f32 + foot_h))
            .flex()
            .flex_col()
            .items_center();
        // 丝栏：列间界行按版字面板所选（乌 / 金 / 朱）取色。
        // 最左一列不画（见 `first_col`）：版框内缘那道闭合细线就是它。
        if !first_col {
            col = col.border_l_1().border_color(rgb(t.rule_by(self.rule_color)));
        }
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
                // 数字串（防呆）：整串成组、每字半格，`223` 只占 1.5 格高，
                // 绝不一字一格竖着摞成三截。
                Piece::Main(g) if west_mode && crate::west::is_digit(g.ch) => {
                    let mut j = idx;
                    while j < items.len() {
                        match &items[j] {
                            Piece::Main(g2) if crate::west::in_digit_run(g2.ch, true) => j += 1,
                            _ => break,
                        }
                    }
                    content = content.child(self.ancient_digit_run(
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
                // 书名号（`Attach`）：半格账记 0，**不占独立格**。
                // 开号 `︽` 由后一个字 look-behind 画、闭号 `︾` 由前一个字
                // look-ahead 画，故正常情况下在下面的 `Main` 分支里就被消费
                // 掉了；走到这里说明它**没有宿主**（列尾 / 段尾落单），
                // 兜底单独画一小格，总比丢字强。
                Piece::Main(g) if g.kind == GlyphKind::Attach => {
                    let has_host = if crate::types::is_open_title(g.ch) {
                        items[idx + 1..]
                            .iter()
                            .any(|p| matches!(p, Piece::Main(m) if m.kind != GlyphKind::Attach))
                    } else {
                        idx > 0
                            && matches!(items[idx - 1], Piece::Main(m) if m.kind != GlyphKind::Attach)
                    };
                    if !has_host {
                        // 落单：单独占一格，但仍是 0.7 倍小字、不撑满格。
                        content = content.child(
                            div()
                                .w(px(cell_w))
                                .h(px(cell_h))
                                .relative()
                                .child(self.title_mark_overlay(g.ch, cell_w, cell_h, fs, back)),
                        );
                    }
                    idx += 1;
                }
                Piece::Main(g) => {
                    // 挤压标点：紧随其后的 `Hang` 字与**本字同格**——列排满
                    // 时排不下的句读挂在末字右下角，不另起一列占列首格。
                    // 渲染必须与分页同源：分页把它排在本字之后，这里就把
                    // 它叠进本字那一格（`idx` 一次吃掉两片）。
                    let hang = match items.get(idx + 1) {
                        Some(Piece::Main(h)) if h.kind == GlyphKind::Hang => Some(*h),
                        _ => None,
                    };
                    // 书名号：开号在本字的**前一格位**（它领起的就是本字，
                    // 故画在本字格右上角）；闭号紧跟本字（书名末字），同 Hang
                    // 一路被本字吃掉。
                    let open_mk = match items.get(idx.wrapping_sub(1)) {
                        Some(Piece::Main(m))
                            if m.kind == GlyphKind::Attach
                                && crate::types::is_open_title(m.ch) =>
                        {
                            Some(m.ch)
                        }
                        _ => None,
                    };
                    let close_mk = match items.get(idx + 1) {
                        Some(Piece::Main(m))
                            if m.kind == GlyphKind::Attach
                                && crate::types::is_close_title(m.ch) =>
                        {
                            Some(m.ch)
                        }
                        _ => None,
                    };
                    idx += 1 + if hang.is_some() { 1 } else { 0 }
                        + if close_mk.is_some() { 1 } else { 0 };
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
                        hang,
                        (open_mk, close_mk),
                        cx,
                    ));
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

    /// 一串连续数字（`223` / `1980`）的竖排渲染：**整串成组、每字半格**。
    ///
    /// 刻本的处理是把整串顺时针转 90° 横躺顺着列往下读，纵向占高
    /// `字数 × 半格`（3 个数字 = 1.5 个汉字高）。旋转坐标由
    /// [`crate::west::digit_slots`] 给出（中心 `cx/cy` + 90°）；GPUI 0.2.2
    /// 没有元素级变换，故按**同一批槽位直立排布**——槽位与分页的半格账逐字
    /// 对齐，将来支持变换时改用 `(cx, cy, 90°)` 绘制即可，账目无需改动。
    ///
    /// 字号取 [`crate::west::digit_font_size`]（上限半个格高），保证相邻
    /// 数字不叠、不溢出格宽。
    fn ancient_digit_run(
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
                Piece::Main(g) => Some(g.ch),
                _ => None,
            })
            .collect();
        // 半格账与分页同源：`223` → 3 个半格 = 1.5 格高。
        // 旋转槽位（若要真横躺）由 west::digit_slots 给出：中心
        // `cy = j * 半格 + 半格/2`、`cx = cell_w/2`、角 90°。
        let units = chars.len().max(1);
        let half = cell_h / 2.0;
        let height = units as f32 * half;
        let dfs = crate::west::digit_font_size(cell_w, cell_h, fs);
        let ink = if back { t.ink_soft() } else { self.palette.ink(t) };
        // 数字走西文字体（Computer Modern 衬线数字），不写死字体名。
        let latin = crate::fonts::family_for_role(crate::fonts::FontRole::Latin);

        let mut block = div()
            .w(px(cell_w))
            .h(px(height))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .font_family(latin.clone());
        if keys.iter().any(|k| self.in_selection(*k)) {
            block = block.relative().child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(cell_w))
                    .h(px(height))
                    .bg(rgba(0xA63A2B2E)),
            );
        }
        for ch in chars.iter() {
            if !crate::west::is_digit(*ch) {
                // 串里的零宽字符：占位但不落墨（账上仍记半格）。
                block = block.child(div().w(px(cell_w)).h(px(half)));
                continue;
            }
            block = block.child(
                div()
                    .w(px(cell_w))
                    .h(px(half))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .text_size(px(dfs))
                    .text_color(rgb(ink))
                    .child(SharedString::from(ch.to_string())),
            );
        }
        if interactive {
            block = block.on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                    // 数字串不可逐字选，按下只标记「碰过正文」，绝不翻页。
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
        // **双行小字**：两个半宽小列，**各占 50%**（v1.25.0 用户指定）——
        // 右列读在前、左列续后，合起来正好铺满一个正文列的宽度，注块与
        // 上下正文同宽同界，行气才接得上（原先左缘留 1/6 空白、两列偏右）。
        let pad = cell_w * crate::gloss::NOTE_PAD_RATIO;
        // 两侧各让出极窄一道（见 `NOTE_EDGE_INSET`）：小字不蹭界栏，
        // 两个小列仍各占余下宽度的一半，注块中线不变。
        let inset = cell_w * crate::gloss::NOTE_EDGE_INSET;
        let sub_w = (cell_w - pad - inset * 2.0) * 0.5;
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
            // 左隙：小字与界栏之间的一线之隔。
            line = line.child(div().w(px(inset)).h(px(row_h)));
            for c in pair {
                line = line.child(match c {
                    // 注文里的【】同样显作「」（夹注不过 shown，这里单独过一道）。
                    Some(ch) => {
                        let ch = crate::types::bracket_to_quote(ch);
                        // 小字里的句读再收一档（v1.28.0）：全角句号与注文同号时
                        // 一个点顶半颗字，整块夹注糊成黑团——见 NOTE_PUNCT_RATIO。
                        let char_fs = if crate::gloss::is_dot_punct(ch)
                            || crate::types::is_punct(ch)
                        {
                            nfs * crate::gloss::NOTE_PUNCT_RATIO
                        } else {
                            nfs
                        };
                        div()
                            .w(px(sub_w))
                            .h(px(row_h))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(char_fs))
                            .text_color(rgb(note_color))
                            .child(SharedString::from(ch.to_string()))
                    }
                    None => div().w(px(sub_w)).h(px(row_h)),
                });
            }
            // 右隙：与左隙对称。
            line = line.child(div().w(px(inset)).h(px(row_h)));
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
        // 篇题字号**略大于正文**（v1.28.0 用户口径「识别章节标题，其字号
        // 可以略大于正文」）：1.12 倍——刻本里章题比正文高一头，但不翻成
        // 「标题字」；格宽仍是 1.33×字号，1.12 不会探出列外。
        // 章题自成**独占的一列**，其后的正文从左边一列另起（正文列数在
        // 分页时已按 total-1 算过），故标题永远不会与正文挤在同一列。
        let tfs = fs * TITLE_COL_SCALE;
        let tcell = cell_h * TITLE_COL_SCALE;
        // 丝栏：列间界行按版字面板所选（乌 / 金 / 朱）取色。
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
        let (tw, th) = crate::banxin::banxin_tail_size(col_w, cell_h, col_w);
        col = col.child(banxin_tail(tw, th, true, t.fish_ink()));
        col = col.child(div().h(px(cell_h * 0.18)));
        // 篇题能排几字：总高扣天头、两枚鱼尾、篇题数与几段留白，
        // 余下全给篇题——鱼尾取了 1.5 字高后，短叶也不会溢出。
        let nfs = (fs * 0.42).max(9.0);
        let num_h = (nfs + 2.0) * han_num(chapter + 1).chars().count() as f32;
        // 卷首空两格：题名不顶着上鱼尾起，低 [`TITLE_HEAD_CELLS`] 格再落字。
        let lead = tcell * TITLE_HEAD_CELLS;
        let room = cell_h * rows as f32 + foot_h
            - head
            - th * 2.0
            - num_h
            - lead
            - cell_h * (0.18 + 0.1 + 0.38);
        // ── 大题在上、小题在下（古籍卷首常例）────────────────────────
        // **大题 = 书名**（《史记》），**小题 = 篇题 / 章题**（五帝本纪第一）。
        // 正文起首书名与篇题各占一行、前后呼应，上下位序取「大题在上、
        // 小题在下」的那种。小题是本篇之名，最尊（1.2×）；大题为全书之名，
        // 次之一档（1.1×），仍高于正文。
        let big: Vec<char> = self.book_title().chars().take(BIG_TITLE_MAX).collect();
        let mut big_n = big.len();
        let gap_h = cell_h * 0.3;
        // 一字一格，格高随放大后的字号（tcell），能排几字按它算。
        let sub_n_of = |n: usize| -> usize {
            let g = if n > 0 { gap_h } else { 0.0 };
            ((room - n as f32 * tcell - g) / tcell)
                .floor()
                .clamp(0.0, SUB_TITLE_MAX as f32) as usize
        };
        let mut take_n = sub_n_of(big_n);
        // 小题优先：地方不够就先砍大题，绝不把篇题挤没（保底 3 字）。
        while take_n < 3 && big_n > 0 {
            big_n -= 1;
            take_n = sub_n_of(big_n);
        }
        // 空两格：两枚空白字格（与正文格同高的纯留白，不画线、不排字）。
        if take_n > 0 || big_n > 0 {
            col = col.child(div().h(px(lead)));
        }
        // 大题：书名，一字一格，自上面下（次尊一档）。
        for c in big.iter().take(big_n) {
            col = col.child(
                div()
                    .w(px(col_w))
                    .h(px(tcell))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(tfs * BIG_TITLE_SCALE))
                    .text_color(rgb(t.ink()))
                    .child(SharedString::from(c.to_string())),
            );
        }
        if big_n > 0 {
            col = col.child(div().h(px(gap_h)));
        }
        // 小题：篇题，一字一格，自上面下（最尊）。
        for c in title.chars().take(take_n) {
            col = col.child(
                div()
                    .w(px(col_w))
                    .h(px(tcell))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(tfs))
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
        // 折痕：纸对折的物理褶，落在中缝正中（极细实线，见 `fold_crease`）。
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

    /// 中缝（版心）：对开时立在纸心的一道竖栏——鱼尾、书口字、叶码
    /// 都立于此。带内自上而下排布：
    ///
    /// ```text
    ///   上黑口（粗墨一道）
    ///   上鱼尾（∧ 口朝下）        ← v1.48.0：中心落在版心高 1/4 处
    ///   书名   宋词三百首     ← 极小号宋体，竖排
    ///   卷次   卷五十一
    ///   篇题   （余量有多少排多少）
    ///          ⋯⋯
    ///   叶码   五十
    ///   下鱼尾（∨ 口朝上）        ← v1.48.0：中心落在版心高 3/4 处
    ///   下黑口（粗墨一道）
    /// ```
    ///
    /// 正中另有一道**极细实线**（0.5px、`#999999`）作纸对折的折痕。
    /// 缝宽约等于一行字宽（`grid::gutter_w`）：刻本版心本就是一列字的
    /// 宽度；文字的高度账由 `banxin::layout` 按优先级分（叶码 → 卷次 →
    /// 书名 → 篇题），分不够就截字，绝不溢出、绝不压字。
    /// 左半叶是章首时不用它——那一叶自带的「章题列」本身就是版心。
    ///
    /// ── v1.34.0：中缝与两叶是**同一张纸**（用户「把中缝两端与外边框
    /// 完美闭合」）────────────────────────────────────────
    /// 旧版中缝是一根独立竖栏：自己算高（605pt vs 纸面 640pt）、自己
    /// 用绝对定位钉框线（结果飘到两叶版框上方 83px）、左右各画一条
    /// 竖线——摊开来中缝像一条孤立的马路悬在中央，两叶「分家」。
    /// 现在它与 `render_ancient` 的纸面**逐层同构**：
    ///
    /// ```text
    ///   sheet   : h(content_h) + pt(margin_top) + pb(margin_bot)   ← 同值
    ///   frame   : p(3) + border_*_3                                ← 同层同位
    ///   inner   : border_*_1                                       ← 同层同位
    ///   内容    : 两叶是 grid+foot，中缝是版心栏（同高 core_h）
    /// ```
    ///
    /// 三条好处，条条冲着用户的三大前提：
    ///   ① **上下闭合**：中缝的上下外框 / 内框与左右叶的上下框在同一
    ///      高度、同一粗细，摊开是一整张平铺的纸——「两端完美闭合」；
    ///   ② **不碰 Invariant 1**：中缝的上下留白直接读引擎的
    ///      `margin_top` / `margin_bot`（3:2 铁律见 `grid::TOP_OF_BOT`），
    ///      不自己算数，天头地脚因此与两叶一丝不差；
    ///   ③ **不碰 Invariant 2**：中缝宽仍是引擎的 `gutter_w`，左右两叶
    ///      等宽（`sheet_side_pads` 镜像），中缝钉在屏幕正中。
    /// 左右竖框则**不画**：两叶的版框已四边闭合（`grid::frame_sides`），
    /// 左叶右缘与右叶左缘各一道粗竖框 + 一道细竖框，正好把中缝夹住——
    /// 中缝再画自己的竖线，缝上就成并排的两条。
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
        // ── 与两叶同源的纵向几何（Invariant 1 / ②）──────────────────
        // 纸面锁 `content_h`、上下留白用引擎的天头 / 地脚——与
        // `render_ancient` 的 sheet 逐值相同，中缝因此与两叶严格对齐。
        let mt = self.margin_top;
        let mb = self.margin_bot;
        let h = self.content_h.max(120.0);
        // 地脚高与 `render_ancient` 同式（叶码条跟着字号走）。
        let foot_h = (fs * 0.36).max(9.0) + 10.0;
        // 版心内芯高 = 行格 + 地脚叶码条，与两叶的 `grid + foot` 同高
        // （两叶的列高本来就含 foot_h，界行才通到版框下沿）。
        let core_h = cell_h * self.grid_rows as f32 + foot_h;
        // 版心**内芯**宽：gw 扣掉左右两叶各 7pt 的框层（3 粗框 + 3 框内
        // 留边 + 1 内细线）。鱼尾、黑口、书口字都排在这两条细线之间，
        // 绝不探到框上——中缝因此被两叶的竖框「夹」住。
        let core_w = (gw - 14.0).max(9.0);
        // 黑口：版心天头 / 地脚各一道粗黑块（大黑口本子），紧贴内框细线
        // 之内——它就是纸对折处露出的那道墨，翻书时一眼定位。墨色与鱼尾
        // 同源（`fish_ink`），深底主题下会自动提亮。
        let heikou = crate::banxin::heikou_h(cell_h);
        // 书口文字：书名（去括号）、卷次「卷五十一」、叶码「五十」。
        let book = self.book_title();
        let juan = format!("卷{}", han_num(chapter + 1));
        let page = han_num(page_idx + 1);
        let lay = crate::banxin::layout(crate::banxin::BanxinOpts {
            // 鱼尾与书口字都按**内芯宽**排：不得探出两条细线。
            gutter_w: core_w,
            // v1.48.0：可用高 = 两枚鱼尾**中心之间**的那段版心（1/4 ↔ 3/4，
            // 即半幅内芯高 + 一枚鱼尾）——鱼尾挪位后，书口文字的预算跟着
            // 换成「夹在两枚鱼尾之间」的量，`banxin::layout` 再扣鱼尾与
            // 段间留白，剩下的才是文字。
            band_h: (core_h * 0.5 + crate::banxin::banxin_tail_size(core_w, cell_h, self.col_w).1)
                .max(40.0),
            cell_h,
            // v1.30.0：鱼尾宽按「列宽 × 0.7」取（见 `banxin_tail_size`）。
            col_w: self.col_w,
            fs,
            book: &book,
            juan: &juan,
            page: &page,
            chapter_title: &self.chapter_title(chapter),
            detailed,
            // v1.28.0：朱印从中缝挪到纸页左下角外侧（见 `render_page_seal`）——
            // 版心只留「鱼尾 + 书口字 + 叶码」，不再被一枚红印隔着。
            seal: false,
        });
        // 书口小字**极小号宋体**：走正文字体（默认文津宋体）的回退链，
        // 横笺里也不会退化成黑体。
        let family = crate::fonts::family_for(self.current_font(), Script::Ancient);
        // 版心内芯：全部走**流式布局**（旧版用绝对定位钉线，纸一伸缩就漂）。
        // ── v1.48.0：鱼尾不再死贴上下黑口 ──────────────────────────
        // 收官指令「中心点移至版心高度的 1/4 和 3/4 处」：上鱼尾中心落在
        // `0.25·core_h`、下鱼尾中心落在 `0.75·core_h`（上下镜像）。算法：
        // 上黑口 + 定位垫 + 半枚鱼尾 = 0.25·core_h，故垫高 =
        // `0.25·core_h − heikou − tail_h/2`；下侧同一数值，黑口留在原位
        // （纸口那道墨不动，动的只是鱼尾）。
        let pad = (core_h * 0.25 - heikou - lay.tail_h * 0.5).max(2.0);
        let mut col = div()
            .w(px(core_w))
            .h(px(core_h))
            .flex()
            .flex_col()
            .items_center();
        // 上黑口：紧贴内框细线之内的一道粗墨。
        col = col.child(div().w_full().h(px(heikou)).bg(rgb(t.fish_ink())));
        // 上鱼尾定位垫：把上鱼尾中心推到版心 1/4 处（不再是口下一丁点）。
        col = col.child(div().h(px(pad)));
        // 上鱼尾：凹口朝下（直线多边形，深灰 `#333333`，见 `banxin_tail`）。
        // 尺寸由 `banxin::layout` 按「宽不越界栏」算好（v1.48.0 已缩小）。
        col = col.child(banxin_tail(lay.tail_w, lay.tail_h, true, t.fish_ink()));
        col = col.child(div().h(px(lay.gap)));
        // 书名 → 卷次 → 篇题（极小号宋体，一字一格竖排）。每段之间留一道
        // 小气口，绝不连成一串（v1.31.x：原先三段直接相接，看着像「挤成一坨」）。
        for (i, line) in lay.above.iter().enumerate() {
            if i > 0 {
                col = col.child(div().h(px(3.0)));
            }
            col = col.child(banxin_vtext(line, core_w, &family, t));
        }
        // 余量全给中间：书名卷次贴着上鱼尾、叶码贴着下鱼尾，两枚鱼尾
        // 「夹」住书口文字，正是刻本版心的样子。
        col = col.child(div().flex_1());
        // 叶码。
        for line in &lay.below {
            col = col.child(banxin_vtext(line, core_w, &family, t));
        }
        col = col.child(div().h(px(lay.gap)));
        // 下鱼尾：∨ 口朝上，与上鱼尾开口相对。
        col = col.child(banxin_tail(lay.tail_w, lay.tail_h, false, t.fish_ink()));
        // 下鱼尾定位垫：与上侧镜像，下鱼尾中心因此落在版心 3/4 处。
        col = col.child(div().h(px(pad)));
        // 下黑口：与上黑口对称，压在内框细线之上。
        col = col.child(div().w_full().h(px(heikou)).bg(rgb(t.fish_ink())));

        // ── 纸面 / 版框 / 内框：与 `render_ancient` 的 sheet 逐层同构 ──
        // `relative`：折痕用绝对定位锚在本栏内（GPUI 的 absolute 子元素
        // 以最近的 positioned 祖先为定位上下文，缺了它线会飘到根节点上）。
        // **不设 `overflow_hidden`**：两叶的纸面也没设——版框自然高
        // （core_h + 14）略超内容盒时两边一样溢出，框线才仍对齐；
        // 单边裁一刀，中缝的下框反比两叶短一截。
        let mut paper = div()
            .relative()
            .w(px(gw))
            .h(px(h))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(mt))
            .pb(px(mb))
            .bg(rgb(t.paper()));
        // 版框（与两叶的 frame 同层）：外粗 3px + 框内留边 3px，
        // **只画上下横框**——左右竖框由两叶闭合的版框充当。
        let mut frame = div()
            .w(px(gw))
            .p(px(3.0))
            .flex()
            .flex_col()
            .items_center()
            .border_color(rgb(t.frame_ink()))
            .border_t_3()
            .border_b_3();
        // 内框细线（与两叶的 inner 同层）：1px、与界行同色同起止，
        // 只画上下——中缝的上下两端因此与左右叶的上下框**逐层闭合**，
        // 界行（乌丝栏）也顺着这两条线接到横框上，字格围成封闭网格。
        let inner = div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .border_color(rgb(t.rule_by(self.rule_color)))
            .border_t_1()
            .border_b_1()
            .child(col);
        // 折痕：纸**对折**的那一道物理褶，落在版心正中——0.5px、`#999999`、
        // 实线（v1.28.0 由虚线改来），只作「纸的痕迹」，比界行还淡。
        // 起止夹在两条内框细线之间（mt+6 ⇄ h-mb-6），与两叶的内框同界。
        paper = paper.child(fold_crease(
            gw * 0.5 - crate::banxin::FOLD_W * 0.5,
            mt + 6.0,
            (h - mt - mb - 12.0).max(0.0),
            t.fold_ink(),
        ));
        frame = frame.child(inner);
        paper = paper.child(frame);
        paper
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

    /// 一枚句读的落位：**字号 × 0.4、起点偏移到字格右下角、色取淡朱**。
    ///
    /// 竖排里「右」是上一列那一侧、「下」是读序前进的方向，故右下角正是
    /// 刻本句读的位置——它跟着读序贴在字的行进前方，既不挡字也不占位。
    ///
    /// 几何全在 `punct::place`（纯函数，字号与偏移的唯一来源）：
    /// `fs = 正文 × 0.4`、`dx = 格宽 × 0.3`、`dy = 格高 × 0.3`。
    /// 颜色是「朱砂兑纸色」的淡朱（比正文轻，却仍是句读的朱意），两套
    /// 深浅主题都稳——用兑色而非 opacity，不依赖底色透出来。
    /// 书名号叠加层（v1.30.0）：缩到正文 **0.7 倍**，贴在字格**右上角**，
    /// 不撑满格子、不与正文争格心。
    ///
    /// 书名号绝不单独占一格——半格账记 0（见 `gloss::piece_units`），
    /// 开号 `︽` 叠在书名首字那一格、闭号 `︾` 叠在书名末字那一格，
    /// 一对角标正好把书名「括」住，行气不断（旧版它独占一格，读起来一断一断）。
    fn title_mark_overlay(&self, ch: char, cell_w: f32, cell_h: f32, fs: f32, back: bool) -> Div {
        let t = self.theme;
        let c = crate::types::title_mark(ch);
        let pfs = (fs * crate::punct::TITLE_FS_RATIO)
            .max(6.0)
            .min(cell_h.min(cell_w));
        let ink = if back { t.ink_soft() } else { t.ink() };
        div()
            .absolute()
            // 右上角：贴格上缘与格右缘（竖排右缘是上一列那一侧，正是
            // 刻本旁标的位置）。只占一个小方盒，正文字仍在格心。
            .top(px(0.0))
            .right(px(0.0))
            .w(px(pfs))
            .h(px(pfs))
            .flex()
            .flex_row()
            .items_start()
            .justify_end()
            .text_size(px(pfs))
            .text_color(rgb(ink))
            .child(SharedString::from(c.to_string()))
    }

    /// 句读浮层：**绝不用字体字形**，也绝不用 SVG 贴图——一枚 `canvas`
    /// 上 `paint_path` 纯手绘的朱砂小记号（几何全在 `punct::place`）。
    ///
    /// ── Invariant 3：句读永不被边框盖住 / 重置 ──────────────────────
    /// 本浮层是 `cell`（字格）的子元素，而 `cell` ⊂ `ancient_column` ⊂
    /// `grid` ⊂ `inner` ⊂ `frame` ⊂ `sheet`：GPUI  immediate 模式下
    /// **先画祖先、后画子孙**，版框（`border_*_3` / `border_*_1`）是
    /// 祖先元素自身的一部分，必然先落笔；句读是整棵树最深处的叶子，
    /// 必然最后落笔。故加粗外框、闭合中缝这类改动**在渲染顺序上
    /// 不可能盖住句读**——只有一种情况会弄丢它：改边框时顺手把
    /// `grid` 的子元素树重建 / 替换掉。本函数与 `ancient_column`
    /// 的字格渲染因此只增不减，边框循环（`frame_sides` 驱动的四个
    /// `if`）也只碰 `border_*`，不碰 children。
    ///
    /// 另外记号本身被 `punct::place` 夹在字格内（有单测钉死），
    /// 版框在字格之外，二者物理上不相交；中缝栏虽有
    /// `overflow_hidden`，但句读在两叶里、不在中缝里，淋不到。
    fn punct_overlay(&self, ch: char, cell_w: f32, cell_h: f32, fs: f32, back: bool) -> Div {
        // 几何（纯函数、有单测）：直径 = 正文 × 0.35、中心钉在格内
        // 70% × 70% 的右下象限，绝不居中、绝不溢出。
        let m = crate::punct::place(fs, cell_w, cell_h, crate::punct::kind_of(ch));
        let ink = self.punct_ink(back);
        // 浮层铺满字格：canvas 的 paint 闭包收到的是**窗口坐标**的
        // bounds，把字格原点（ox/oy）加回记号中心即可。
        div()
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .w(px(cell_w))
            .h(px(cell_h))
            .child(
                canvas(
                    // prepaint：纯布局阶段，只读 bounds，返回空状态。
                    |_bounds: Bounds<gpui::Pixels>, _window: &mut Window, _cx: &mut App| (),
                    // paint：只读几何与颜色，绝不修改 State、绝不 notify
                    // （闭包签名收 `&mut App` 也从类型上保证了这一点）。
                    move |bounds: Bounds<gpui::Pixels>, _state: (), window: &mut Window, _cx: &mut App| {
                        let ox = f32::from(bounds.origin.x);
                        let oy = f32::from(bounds.origin.y);
                        crate::punct::paint_mark(window, ox + m.cx, oy + m.cy, m, ink);
                    },
                )
                .w(px(cell_w))
                .h(px(cell_h)),
            )
    }

    /// 句读墨色（Invariant 3 的颜色一侧）：**强制用 `palette.note`**——
    /// 句读标点与夹注、眉批同属「批注语汇」，必须跟正文（`palette.ink`）严格分色，
    /// 绝不能正文注文一个色（任务三·生死攸关）。`note` 自身已按纸色明暗
    /// 自动切换（深底提亮、浅纸压深），无需再手算。
    fn punct_ink(&self, _back: bool) -> u32 {
        self.palette.note(self.theme)
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
        // 挤进本格右下角的句读（列排满时排不下的那枚），`None` 为常规字格。
        hang: Option<Glyph>,
        // 叠到本格右上角的书名号（v1.30.0）：`(开号, 闭号)`。
        // 开号 `︽` 落在书名**首字**格、闭号 `︾` 落在书名**末字**格，
        // 两者都不占独立格——书名号因此永不落列首 / 列尾。
        title_marks: (Option<char>, Option<char>),
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
        // 注音：**书里的优先，字典兜底**。
        //   * `NoteKind::Ruby` —— EPUB `<ruby><rt>` 导入时挂在该字上的读音
        //     （`[拼音:…]` 标记 → 注音注），作者标什么就读什么；
        //   * 没有则查内置生僻字表（`rare_pinyin`），按「生僻字注音」开关走。
        let ruby_py: Option<String> = if self.show_rare {
            self.ruby_pinyin(g)
                .or_else(|| (!is_punct(g.ch)).then(|| self.rare_pinyin(g.ch)).flatten().map(str::to_string))
        } else {
            None
        };
        let rare_py = ruby_py;
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
        // 着重（导入管线里 `**加粗**` / `<strong>` 转来的 `[高亮]…[/高亮]`）：
        // 刻本不用西式「加粗」，而是在字旁点一枚**朱砂圈点**（着重号）。
        // 竖排里点落在字右侧，与左侧的专名线分列两边，互不干扰。
        let emph = !is_punct(g.ch) && self.emph_at(g);
        // 检索命中：朱砂红衬底，当前那一处更重、其余较淡（用 `mix` 兑出
        // 半透感，不依赖 opacity —— 深色主题下也稳）。
        let hit = if g.kind == GlyphKind::Char {
            // 抵达高亮优先：朱批录 / 搜索跳转后那一下，比检索命中更重一档。
            if self.flash_state_at(g.chapter, g.para, g.offset) == Some(true) {
                Some(true)
            } else {
                self.search_state_at(g.chapter, g.para, g.offset)
            }
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
        if marked || rare_py.is_some() || mark_line.is_some() || hit.is_some() || emph {
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
        if emph {
            // 着重圈点：一枚实心朱砂小圆，点径随字号但保底可见（2.2~5px）。
            let d = (fs * 0.13).clamp(2.2, 5.0);
            cell = cell.child(
                div()
                    .absolute()
                    .right(px(2.0))
                    .top(px(cell_h * 0.5 - d * 0.5))
                    .w(px(d))
                    .h(px(d))
                    .rounded(px(d / 2.0))
                    .bg(rgb(t.accent())),
            );
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
        if let Some(py) = rare_py.as_deref() {
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
        } else if crate::types::is_title_mark(g.ch) {
            // **书名号**（兜底路径）：正常流程里 `to_flow` 已把它换成 `︽ ︾`
            // 并标成 `Attach`，由 `title_mark_overlay` 叠到邻字格的右上角、
            // 半格账记 0、不占格。走到这里说明它是**孤立**的（前后都没有
            // 可依附的正文），只能单独一格——字号仍是 0.7 倍正文。
            let tf = (fs * crate::punct::TITLE_FS_RATIO).max(9.0).min(cell_h);
            cell = cell.child(
                div()
                    .text_size(px(tf))
                    .text_color(rgb(ink))
                    .child(SharedString::from(
                        crate::types::title_mark(g.ch).to_string(),
                    )),
            );
        } else if crate::types::is_quote_mark(g.ch) {
            // **直角引号**：竖排里要旋转 90°（开口朝下）。GPUI 0.2.2 没有
            // 元素级旋转变换，直接用 Unicode 竖排呈现形 `﹁ ﹂ ﹃ ﹄`
            // ——字库里存的就是转好的那一枚。
            cell = cell.child(
                div()
                    .text_size(px(fs.min(cell_h)))
                    .text_color(rgb(ink))
                    .child(SharedString::from(
                        crate::types::vertical_quote(g.ch).to_string(),
                    )),
            );
        } else if is_punct(g.ch) {
            // 句读（标点）：**正文字号 × 0.35** 的朱砂小手绘（空心圈 / 小弧），
            // 中心钉在字格右下角（x + 格宽×0.7、y + 格高×0.7），格心整个
            // 让给正文。几何与绘制全在 `punct`（纯函数 + `paint_path`，
            // 有单测），这里只照抄——**严禁**在这里回落字体字形或 SVG。
            cell = cell
                .relative()
                .child(self.punct_overlay(g.ch, cell_w, cell_h, fs, back));
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
        // 挤压标点（列排满时排不下的那枚句读）：挂在**本格**右下角，与末字
        // 同格、共用一格。它半格账记 0，故绝不会另起一列占列首格。
        if let Some(h) = hang {
            cell = cell
                .relative()
                .child(self.punct_overlay(h.ch, cell_w, cell_h, fs, back));
        }
        // 书名号：贴**本格右上角**、0.7 倍正文。开号在本格（书名首字）、
        // 闭号也在本格（书名末字）——一格至多各一枚，绝不单独占列。
        let (open_mk, close_mk) = title_marks;
        if open_mk.is_some() || close_mk.is_some() {
            cell = cell.relative();
            if let Some(c) = open_mk {
                cell = cell.child(self.title_mark_overlay(c, cell_w, cell_h, fs, back));
            }
            if let Some(c) = close_mk {
                cell = cell.child(self.title_mark_overlay(c, cell_w, cell_h, fs, back));
            }
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
                            // ★ 长按（v1.40.0）：这一下**不能**被字格吞掉。
                            //
                            // 字格在这里弹「工具条」并 `stop_propagation()`，
                            // 冒泡到此为止——正文层（`body`）的抬起处理器
                            // 永远收不到，于是 `held >= LONG_PRESS` 那条长按
                            // 查词的路整个不成立（真机表现：长按毫无反应，
                            // 只冒出一条工具条）。故长按时直接放行，把这一下
                            // 让给正文层的长按分支去弹字书浮签。
                            if this
                                .press_time
                                .map(|t| t.elapsed() >= LONG_PRESS)
                                .unwrap_or(false)
                            {
                                return;
                            }
                            if this.sel_anchor.is_some() {
                                this.dragging = false;
                                this.sel_focus = Some(key);
                                // 单击（未拖选）带注音的字：直接呼出「注音浮签」，
                                // 大字显示拼音与本字；拖选仍走原来的选区工具条。
                                if this.sel_anchor == this.sel_focus {
                                    if let Some(py) = rare_py.clone() {
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

    /// 书里的注音（EPUB `<ruby><rt>` 导入的 `NoteKind::Ruby`）。
    ///
    /// 与 `rare_pinyin` 的分工：这个是**作者标了什么就读什么**（`长[拼音:cháng]`
    /// 的 cháng 不会被通用字表念成 zhǎng），字表只在没标音时兜底。
    fn ruby_pinyin(&self, g: Glyph) -> Option<String> {
        let p = self.book.as_ref()?.chapters.get(g.chapter)?.paragraphs.get(g.para)?;
        p.notes
            .iter()
            .find(|n| n.kind == NoteKind::Ruby && n.at == g.offset)
            .map(|n| n.text.clone())
    }

    fn render_reading_status(&mut self, total: usize, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let unit = match self.script {
            Script::Ancient => "叶",
            Script::Modern => "页",
        };
        // 行款信息（v1.32.0）：版本学口径「半叶几行、行几字」——版式名 +
        // 行款表的目标字数；触发最小字号保护（窗口过小）时改报实际排下的
        // 字数，与「无法展示完整的 X 行 Y 字」提示互为印证。
        // 列数不在行款表（用户自定档）时退回引擎实测行数，照样有行款可报。
        let hangkou = crate::grid::LayoutConfig::by_columns(self.grid.columns)
            .map(|c| {
                if self.grid_min_font {
                    format!(
                        "{} · 半叶{}行，行{}字",
                        c.label,
                        han_num(self.grid.columns),
                        han_num(self.grid_rows)
                    )
                } else {
                    format!(
                        "{} · 半叶{}行，行{}字",
                        c.label,
                        han_num(self.grid.columns),
                        han_num(c.target_chars)
                    )
                }
            })
            .unwrap_or_else(|| {
                format!(
                    "半叶{}行，行{}字",
                    han_num(self.grid.columns),
                    han_num(self.grid_rows)
                )
            });
        let mode = match self.script {
            Script::Ancient => {
                if self.notes_inline && self.show_notes {
                    format!("古文竖笺 · {hangkou} · 双行小字夹注 · 从右到左")
                } else {
                    format!("古文竖笺 · {hangkou} · 四周双边 · 从右到左")
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
        // 页码处**双击**＝一键回到卷首（v1.38.0）。
        //
        // 状态栏那一行「第 X 叶 / 共 Y 叶」本就是全书坐标，双击它回卷首
        // 是最自然的手势——不必先开目录、再拖到最右一根竹简。
        // GPUI 0.2.2 没有 `on_double_click`，只有 `MouseDownEvent::click_count`
        // （双击时系统会连发两次 mousedown，第二次的 click_count 为 2），
        // 故这里判 `>= 2`；单击（click_count == 1）一律放行，不吞事件。
        let page_chip = div()
            .cursor_pointer()
            .text_color(rgb(t.chrome_text()))
            .hover(|s| s.text_color(rgb(sheet_zhu(t))))
            .child(SharedString::from(left.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    if event.click_count < 2 {
                        return;
                    }
                    cx.stop_propagation();
                    this.jump_to_start();
                    cx.notify();
                }),
            );
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
        // 导出：**极简文字按钮**（无底无框，悬停转朱砂），落在状态栏中段。
        // 状态栏是常驻的一条，`justify_between` 三格即「页码 ｜ 导出 ｜ 书信息」。
        let action = |label: &'static str| -> Div {
            div()
                .px(px(8.0))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(rgb(t.chrome_text()))
                .hover(|s| s.text_color(rgb(sheet_zhu(t))))
                .child(SharedString::from(label))
        };
        let png_btn = action("印制笺卡（PNG）").on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                // 截当前纸面 → 弹系统保存对话框 → 落盘。
                this.export_page_png(window, cx);
            }),
        );
        let md_btn = action("导出纯文本（MD）").on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _window, cx| {
                cx.stop_propagation();
                // 全书正文 + `[^n]` 脚注 → 弹保存对话框写成 .md。
                this.export_markdown_file(cx);
            }),
        );
        let pdf_page_btn = action("本叶为 PDF").on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                // 截当前纸面 → 包成单页 PDF → 弹系统保存框。
                this.export_pdf_current(window, cx);
            }),
        );
        let pdf_book_btn = action("全书为 PDF").on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                // 逐页翻面截屏 → 合成多页 PDF → 弹系统保存框。
                this.export_pdf_full(window, cx);
            }),
        );

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
            .child(page_chip)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(png_btn)
                    .child(md_btn)
                    .child(pdf_page_btn)
                    .child(pdf_book_btn),
            )
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
    /// **天头眉批**（v1.37.0）：把读者写在版框上方那段天头里的小字竖排画出来。
    ///
    /// 画两样东西：
    ///   * 已存的一列列朱砂小字（`types::HeadNote`）——坐标一律由 `u/v` 比例
    ///     × **当帧**天头带算出，故窗口缩放、换留白档、改列数都自动等比跟随，
    ///     存储值一个字节都不用改（用户点名的「必须跟着天头等比缩放」）；
    ///   * 正在写的那条——就地竖排 + 一道朱色光标，外面套 `ime::InputField`
    ///     接中文输入法（可打印字符一律不走 keydown，见 `ime.rs` 架构说明）。
    ///
    /// 无背景、无边框：朱砂小字直接落在纸的留白上，像毛笔写上去的。
    /// 整层是绝对定位，**不参与任何排版**，故绝不会碰动三大铁律的几何。
    fn render_head_notes(&mut self, cx: &mut Context<Self>) -> Div {
        let dbg = std::env::var("JIANDU_HEAD_NOTE").is_ok();
        if dbg {
            eprintln!(
                "[眉批] page_rect={:?} margin_top={} viewport={}x{}",
                self.page_rect.get(),
                self.margin_top,
                self.viewport_w,
                self.viewport_h
            );
        }
        let Some((_, _, bw, bh)) = self.head_band() else {
            if dbg {
                eprintln!("[眉批] 天头不可用（band=None，margin_top 太窄）");
            }
            return div();
        };
        // 只画钉在**当前这一面**上的眉批：判据是该面首字的字键
        // （与 `page_anchor()` 同源），改列数重新分页后仍能找回去。
        let Some(key) = self.page_anchor() else {
            if dbg {
                eprintln!("[眉批] 无页锚点");
            }
            return div();
        };
        // 开发用：JIANDU_HEAD_NOTE=1 自动题两条**同位置**的眉批（第二条
        // 应向左让开一列），便于核对天头渲染与防重叠。只动内存，不写 JSON。
        if dbg && !self.head_notes.iter().any(|n| n.key == key) {
            let (u, v) = (0.70f32, 0.18f32);
            self.head_notes.push(crate::types::HeadNote::new(
                key,
                u,
                v,
                "批曰：此本与《索隐》异",
            ));
            let u2 = crate::types::HeadNote::free_u(&self.head_notes, key, u, v);
            self.head_notes
                .push(crate::types::HeadNote::new(key, u2, v, "再批：让开一列"));
        }
        // 字号：正文的 65%，但**不低于 14px**（正文缩得再小也看得清）。
        let fs = (self.size_ancient * HEAD_NOTE_FS_RATIO).max(HEAD_NOTE_FS_MIN);
        let lh = fs * HEAD_NOTE_LH_RATIO;
        // 恒定楷体（不看夹注选了什么字体）：小字放大后宋体笔画发木，楷体才像墨迹。
        let family = crate::fonts::head_family();
        // T3：眉批（天头批注）强制走 note_color，与正文套印分离。
        let head_ink = (self.palette.note(self.theme) << 8) | HEAD_NOTE_INK_ALPHA;

        let mut layer = div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(bw))
            .h(px(bh))
            .font_family(&family)
            .text_size(px(fs))
            .text_color(rgba(head_ink));

        for (idx, n) in self.head_notes.iter().enumerate() {
            if n.key != key {
                continue;
            }
            // v1.48.0：正在改写的那条暂时收起——草稿就地竖排占它的位，
            // 原文与草稿绝不叠在同一处（`head_edit` 由 commit / cancel 消费）。
            if self.head_edit == Some(idx) {
                continue;
            }
            let n_chars = n.text.chars().count();
            let (x, y) = Self::head_note_pos(n.u, n.v, n_chars, bw, bh, fs, lh);
            // 命中区比字柱略宽：天头批注是小字，给个宽松好点的框。
            // 点它 = 唤醒管理态（再点一次 = 就地改写），**不是**新建——
            // 与「点空白起新批」的分野就在这一层（防误触）。
            // v1.52.0：倍率由 3.6 降到 2.2 —— 字号涨了 1.6 倍，倍率不改的话
            // 命中框会跟着胀大 60%，空白处起新批就被它抢走了。降下来正好
            // 让命中框的**绝对宽度**与放大前持平（0.40×3.6 ≈ 0.65×2.2）。
            let mut note_box = div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(fs * 2.2))
                .cursor_pointer()
                .child(Self::head_note_column(&n.text, fs, lh, None, head_ink));
            // v1.48.0 唤醒态：字柱下方挂一枚**极小古风菜单**——纯文字
            // 「改 / 删」，无底无框（与现代白框菜单划清界限）；删 朱砂。
            if self.head_active == Some(idx) {
                let menu = div()
                    .w(px(fs * 2.1))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .pt(px(2.0))
                    .child(
                        div()
                            .font_family(family.clone())
                            // v1.52.0：菜单字跟着长一档（10.5 → 12.0），
                            // 免得字柱放大后「改 / 删」显得像两粒芝麻。
                            .text_size(px(12.0))
                            .text_color(rgba((self.palette.note(self.theme) << 8) | 0xCC))
                            .cursor_pointer()
                            .child(SharedString::from("改"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    // 清掉 capture 阶段记下的天头按下，免得
                                    // 随后那次 mouse-up 又在原地起一条新批。
                                    this.head_press = None;
                                    this.press_at = None;
                                    this.open_head_note_edit(idx);
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .font_family(family.clone())
                            .text_size(px(12.5))
                            .text_color(rgba((self.palette.note(self.theme) << 8) | HEAD_NOTE_INK_ALPHA))
                            .cursor_pointer()
                            // 「删」不用 ✕（U+2715）：楷书字体多缺此字形，
                            // 渲染成零宽——按钮看着像丢了（v1.48.0 实测）。
                            .child(SharedString::from("删"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.head_press = None;
                                    this.press_at = None;
                                    this.delete_head_note(idx);
                                    cx.notify();
                                }),
                            ),
                    );
                note_box = note_box.child(menu);
            }
            note_box = note_box.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    // 吞掉天头「新建」的按下记录：这一下点的是已有眉批。
                    this.head_press = None;
                    this.press_at = None;
                    if this.head_active == Some(idx) {
                        // 已唤醒 → 再点字柱本身 = 就地改写。
                        this.open_head_note_edit(idx);
                    } else {
                        this.head_active = Some(idx);
                    }
                    cx.notify();
                }),
            );
            layer = layer.child(note_box);
        }
        if dbg {
            let drawn = self.head_notes.iter().filter(|n| n.key == key).count();
            eprintln!("[眉批] band={bw}x{bh} fs={fs} lh={lh} 画出={drawn} 共={}", self.head_notes.len());
        }

        // 正在写的那条：就地竖排 + 光标，并套 IME 输入层接中文。
        if self.note_editing {
            if let Some((k, u, v)) = self.head_anchor {
                if k == key {
                    let n = self.note_draft.chars().count();
                    let (x, y) = Self::head_note_pos(u, v, n.max(1), bw, bh, fs, lh);
                    let draft = self.note_draft.clone();
                    let cur = self.note_cursor;
                    layer = layer.child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            // 输入层：焦点落在此元素上，`InputHandler` 才把
                            // 输入法组好的汉字送回 `note_draft`（见 `ime.rs`）。
                            .child(crate::ime::InputField::new(
                                Self::head_note_column(&draft, fs, lh, Some(cur), head_ink),
                                self.note_focus.clone(),
                                cx.entity(),
                            )),
                    );
                }
            }
        }
        layer
    }

    /// 眉批字柱的**左上角**：由归一化 (u, v) 换算，并向内夹紧——
    /// 一列写到底不许溢出天头、也不许被纸缘切掉半个字。
    fn head_note_pos(u: f32, v: f32, chars: usize, bw: f32, bh: f32, fs: f32, lh: f32) -> (f32, f32) {
        let w = fs * 1.2;
        let x = (u * bw).min((bw - w).max(0.0)).max(0.0);
        let y = (v * bh).min((bh - chars as f32 * lh).max(0.0)).max(0.0);
        (x, y)
    }

    /// 一列竖排小字（眉批的「字柱」）：逐字换行成列，`cursor` 为 Some 时在
    /// 第 cur 字前插一道朱色细横标（竖排里光标就是字上方的一道横线）。
    fn head_note_column(text: &str, fs: f32, lh: f32, cursor: Option<usize>, ink: u32) -> Div {
        let mut col = div().flex().flex_col().items_center();
        let n = text.chars().count();
        for (i, ch) in text.chars().enumerate() {
            if cursor == Some(i) {
                // 光标横线随字号加粗（原来死的 1.5px 在大字旁边细得看不见）。
                col = col.child(div().w(px(fs)).h(px((fs * 0.10).max(2.0))).bg(rgba(ink)));
            }
            col = col.child(
                div()
                    .h(px(lh))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(SharedString::from(ch.to_string())),
            );
        }
        // 光标在串尾：补一道横线，否则末尾看不见落点。
        if cursor == Some(n) {
            col = col.child(div().w(px(fs)).h(px((fs * 0.10).max(2.0))).bg(rgba(ink)));
        }
        col
    }

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
    /// 开发用：扫一遍叶面找一个字，开一次查词浮签（见调用处的说明）。
    ///
    /// 不写死坐标——列数 / 留白 / 窗口尺寸一变，固定点就落到界栏上了；
    /// 这里用 `glyph_hit_at` 从右向左、自上而下试，命中第一个字就收工。
    fn dev_lookup_once(&mut self) {
        if std::env::var("JIANDU_LOOKUP").is_err() || self.bubble.is_some() {
            return;
        }
        let (vw, vh) = (self.viewport_w, self.viewport_h);
        let mut hit = None;
        let mut y = vh * 0.22;
        while y < vh * 0.8 && hit.is_none() {
            let mut x = vw * 0.92;
            while x > vw * 0.45 {
                if let Some(gb) = self.glyph_hit_at(x, y) {
                    if !crate::types::is_punct(gb.glyph.ch) {
                        hit = Some(gb);
                        break;
                    }
                }
                x -= self.col_w.max(8.0) * 0.5;
            }
            y += self.row_h.max(8.0) * 0.5;
        }
        let Some(gb) = hit else {
            return;
        };
        let key = gb.key();
        let ch = gb.glyph.ch;
        let mut text = self.text_from(key, 4);
        if text.is_empty() {
            text = ch.to_string();
        }
        let (x, y) = (gb.x + gb.w * 0.5, gb.y + gb.h * 0.5);
        // 开发用：`JIANDU_LOOKUP_CHAR=某字` 时刻意查该字（不问叶面首字），
        // 便于核对短释义的卡高自适应与长释义的截断分栏。
        let (ch, text) = match std::env::var("JIANDU_LOOKUP_CHAR") {
            Ok(s) if !s.trim().is_empty() => {
                let c = s.trim().chars().next().unwrap();
                (c, c.to_string())
            }
            _ => (ch, text.clone()),
        };
        self.bubble = Some(crate::types::GlyphHit {
            key,
            ch,
            text: text.clone(),
            x,
            y,
        });
        self.open_lookup(key, ch, text, Some((x, y)), Some((gb.x, gb.y, gb.w, gb.h)));
    }

    /// 开发用：`JIANDU_MENU=1` 时开卷就在叶面上点一个字，把「竖排书签」
    /// 摊出来——与 `JIANDU_LOOKUP` 同款开关，便于截图核对这枚签的版式。
    fn dev_menu_once(&mut self) {
        if std::env::var("JIANDU_MENU").is_err() || self.popup.is_some() {
            return;
        }
        let (vw, vh) = (self.viewport_w, self.viewport_h);
        let mut y = vh * 0.28;
        while y < vh * 0.7 {
            let mut x = vw * 0.9;
            while x > vw * 0.5 {
                if let Some(gb) = self.glyph_hit_at(x, y) {
                    if gb.glyph.kind == crate::types::GlyphKind::Char {
                        let key = gb.key();
                        let ch = gb.glyph.ch;
                        self.popup = Some(crate::types::GlyphHit {
                            key,
                            ch,
                            text: ch.to_string(),
                            x: gb.x + gb.w * 0.5,
                            y: gb.y + gb.h * 0.5,
                        });
                        return;
                    }
                }
                x -= self.col_w.max(8.0) * 0.5;
            }
            y += self.row_h.max(8.0) * 0.5;
        }
    }

    fn render_lookup_card(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let Some(hit) = self.bubble.clone() else {
            return div();
        };
        let vw = self.viewport_w;
        let vh = self.viewport_h;

        // ── 词条：**双引擎字书优先**（康熙 + 說文），不成再退旧字书 ──
        //
        // `query_dict` 走嵌在二进制里的 dict.db：一字同时给出康熙的历代
        // 韵书引文与說文的小篆本义；它收不到的（标点、词组、极生僻字）
        // 才退回 `guhanyu.json` 那部古汉语字典。
        let zishu = crate::dict::query_dict(hit.ch);
        let entry = crate::dict::lookup(&hit.text).or_else(|| crate::dict::entry_of(hit.ch));
        let word = zishu
            .as_ref()
            .map(|z| z.word.clone())
            .or_else(|| entry.as_ref().map(|e| e.word.clone()))
            .unwrap_or_else(|| hit.ch.to_string());
        let pinyin = {
            let raw = zishu
                .as_ref()
                .filter(|z| !z.pinyin.is_empty())
                .map(|z| z.pinyin.clone())
                .or_else(|| entry.as_ref().map(|e| e.pinyin.clone()))
                .unwrap_or_default();
            // 清掉首尾的占位横线/空白：旧字书兜底没拼音时会落下一根孤立的
            // 「—」破折号，既丑又像拼音的一部分，去掉。
            let t = raw
                .trim()
                .trim_matches(|c| matches!(c, '—' | '–' | '-' | '_' | '|' | '/' | '／' | '　'))
                .trim();
            if t.is_empty() {
                String::new()
            } else {
                t.to_string()
            }
        };

        // ── 横排便签的度量（v1.42.0）────────────────────────────────────
        //
        // 尺寸账的来历（v1.41.0 立、v1.42.0 收紧）：
        //   · 定宽 400 → **336**：一次只摊一部字书（分页之后），卡片就不必
        //     为「康熙 + 說文」两段并排留那么宽；窄一档，正文少糊掉一片。
        //   · 纸色 α=0.90 → **0.92**（`WASHI_ALPHA`）：背后那行字要更**若隐
        //     若现**，才像一张压在书页上的笺，不像一块白板。
        //   · 按钮行 22 → **18**、字号 11.5 → **11**、钮间距 12 → **9**。
        //   · 三截硬账不变：字头条（含朱丝线）与按钮行固定高，中间正文区
        //     `flex_1 + overflow_y_scroll` 自滚，按钮永在卡内。
        let pad = 11.0;
        let max_w = 336.0f32.min(vw - 24.0);
        let inner_w = max_w - pad * 2.0 - 2.0; // 扣掉内边距与边框
        let fs = 13.5f32; // 正文（释义）字号
        // 行距：v1.42 起是 1.62，本轮再 +20% → 1.944，长串释义不再挤成一坨砖。
        let line_h = fs * 1.944;
        let para_gap = 6.5; // 段间留白：在【唐韻】等书证起手前断开，肉眼能分段
        let head_fs = 30.0; // 字头大字
        let py_fs = 13.0; // 拼音（无衬线，独成一列不跟汉字抢宽度）
        let tab_fs = 12.5; // 页签「康熙 / 說文」
        let tab_h = tab_fs * 1.9;
        let head_row_h = head_fs * 1.25;
        let head_block_h = head_row_h + 4.0; // 字头条（已去掉压在拼音下的朱丝线）
        let btn_row_h = 18.0;
        let sec_gap = 8.0;
        let max_h = (vh * 0.82).clamp(220.0, 520.0);
        // 一行能排多少字：宋体汉字是**全角**，一个字正好一个字号宽，故直取
        // `inner_w / fs`。留余量（×1.06）会把每行少算一个字，十几行累计下来
        // 卡高多出近一行，按钮上方就空出一条白带。
        let per_line = ((inner_w / fs).floor() as usize).clamp(8, 60);

        // ── 页签：一次只摊一部（v1.42.0）────────────────────────────────
        //
        // 选中的那部若是空的（字书未收《說文》，或本地字书压根没收这个字），
        // 自动落到另一部——页签不空转，用户不会点出一个空白页。
        let kx_src = match zishu.as_ref() {
            Some(z) => crate::dict::tidy_zishu(&z.kangxi),
            None => String::new(),
        };
        let sw_src = match zishu.as_ref() {
            Some(z) => crate::dict::tidy_zishu(&z.shuowen),
            None => entry.as_ref().map(|e| e.gloss.clone()).unwrap_or_default(),
        };
        let mut tab = self.dict_tab;
        let body_src = match tab {
            crate::app::DictTab::Kangxi => {
                if kx_src.is_empty() && !sw_src.is_empty() {
                    tab = crate::app::DictTab::Shuowen;
                    sw_src.clone()
                } else {
                    kx_src.clone()
                }
            }
            crate::app::DictTab::Shuowen => {
                if sw_src.is_empty() && !kx_src.is_empty() {
                    tab = crate::app::DictTab::Kangxi;
                    kx_src.clone()
                } else {
                    sw_src.clone()
                }
            }
        };
        // 把一整条长串切成「书证」小段（【唐韻】…【集韻】…　又…），段前留白，
        // 不再糊成一坨砖。单页上限四千字，再长也不值得在浮签里读。
        let paras_all: Vec<String> = if body_src.is_empty() {
            vec!["（本地字書未收，可点「考據原典」上漢典）".to_string()]
        } else {
            split_zishu_paragraphs(&clip_chars(&body_src, 4000))
        };

        let lines_of = |s: &str| -> usize {
            if s.is_empty() {
                0
            } else {
                (s.chars().count() + per_line - 1) / per_line
            }
        };
        // 段间留白折算成「行」计入行预算，避免段多了把卡撑过上限。
        let gap_lines = (para_gap / line_h).ceil();
        // 正文区在「卡封顶」时能容下多少行（页签与字头条占的地方先扣掉）。
        let body_h_at = |h: f32| -> f32 {
            (h - pad * 2.0 - tab_h - sec_gap - head_block_h - sec_gap * 2.0 - btn_row_h - 2.0)
                .max(48.0)
        };
        // 贪心选段：能整段塞进预算就整段塞，塞不下的最后一段按需截断。
        let avail = ((body_h_at(max_h) / line_h).floor() as f32).max(3.0);
        let mut chosen: Vec<String> = Vec::new();
        let mut lines_total: f32 = 0.0;
        let mut first = true;
        for p in &paras_all {
            let l = lines_of(p).max(1) as f32;
            let add = if first { l } else { l + gap_lines };
            if lines_total + add <= avail {
                lines_total += add;
                chosen.push(p.clone());
                first = false;
            } else {
                let room = (avail - lines_total - if first { 0.0 } else { gap_lines }).floor()
                    as usize;
                if room >= 1 {
                    let seg = clip_chars(p, room * per_line);
                    if !seg.is_empty() {
                        chosen.push(seg);
                        lines_total += room as f32 + if first { 0.0 } else { gap_lines };
                    }
                }
                break;
            }
        }
        if chosen.is_empty() {
            chosen.push(clip_chars(&body_src, (avail as usize).max(1) * per_line));
        }
        // 定稿高度：按**实际选段**的行预算收（含段间折算），不要硬撑到上限，
        // 否则按钮上方空出一块死白，像卡片被截了一刀。
        let h = {
            let l = lines_total.max(1.0);
            (pad * 2.0 + tab_h + sec_gap + head_block_h + sec_gap * 2.0 + l * line_h
                + btn_row_h
                + 2.0)
                .min(max_h)
        };
        let w = max_w;

        // ── 卡面配色：宣纸 + 朱砂 + 暖墨 ────────────────────────────────
        //
        // 纸色固定取宣纸白（#F5F0E6）；深底主题下向纸面兑一档，免得夜里弹出
        // 一块刺眼的亮板。透明度取 `WASHI_ALPHA`（0.92）：**要能看见底下的
        // 正文**——压在书页上的笺本来就该透出后面的字，太实就成了白板。
        let paper_hex = if t.is_dark() {
            crate::theme::mix(0xF5F0E6, t.paper(), 0.28)
        } else {
            0xF5F0E6
        };
        let zhu = 0xB23A30;
        let washi = rgba((paper_hex << 8) | WASHI_ALPHA);
        let lum = (((paper_hex >> 16) & 0xFF) * 30
            + ((paper_hex >> 8) & 0xFF) * 59
            + (paper_hex & 0xFF) * 11)
            / 100;
        let (ink_main, ink_body) = if lum > 128 {
            (
                crate::theme::mix(0x2A2620, paper_hex, 0.06),
                crate::theme::mix(0x2A2620, paper_hex, 0.16),
            )
        } else {
            (
                crate::theme::mix(0xF2ECE0, paper_hex, 0.06),
                crate::theme::mix(0xF2ECE0, paper_hex, 0.16),
            )
        };
        // 拼音用深灰（不再用朱砂红）：读音是辅助信息，朱砂太跳会盖过释义。
        let py_ink = if lum > 128 {
            crate::theme::mix(0x55514B, paper_hex, 0.08)
        } else {
            crate::theme::mix(0xB8B2A8, paper_hex, 0.12)
        };

        // ── 智能避让：绕开被查的那个字（有选区则绕开整段选区）─────────
        let anchor = match self.lookup_rect {
            Some((x, y, w, h)) => crate::hit::Anchor { x, y, w, h },
            None => self.selected_anchor().unwrap_or(crate::hit::Anchor {
                x: hit.x - self.col_w * 0.5,
                y: hit.y - self.row_h * 0.5,
                w: self.col_w,
                h: self.row_h,
            }),
        };
        let place = crate::hit::place_card(
            &anchor,
            w,
            h,
            vw,
            vh,
            crate::hit::CARD_PAD,
            crate::hit::CARD_GAP,
        );

        // ── 字体：正文 / 字头宋体，拼音无衬线 ─────────────────────────
        let main_family = self.main_family();
        let latin_family = crate::fonts::family_for_role(crate::fonts::FontRole::Latin);

        // ── 顶部页签：康熙 / 說文（一次一签，选中者朱砂带下划线）──────
        let tab_row = {
            let mut row = div()
                .w_full()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .justify_start()
                .gap(px(14.0));
            for which in [crate::app::DictTab::Kangxi, crate::app::DictTab::Shuowen] {
                let src_empty = match which {
                    crate::app::DictTab::Kangxi => kx_src.is_empty(),
                    crate::app::DictTab::Shuowen => sw_src.is_empty(),
                };
                let on = which == tab;
                // 选中的那一签下面压一根朱丝线（GPUI 0.2.2 无 `border_b_1`，
                // 故用一枚 1px 的细条；未选中的那签让条透明，高度才对齐）。
                // 选中态：朱砂下划线「强化」——加粗到 1.5px、alpha 提到 0xDC，
                // 仍属「极细」，但比上一版那种半透明 1px 清楚得多。
                let rule = if on {
                    rgba((zhu << 8) | 0xDC)
                } else {
                    rgba((zhu << 8) | 0x00)
                };
                let text = div()
                    .font_family(main_family.clone())
                    .text_size(px(tab_fs))
                    .line_height(px(tab_fs * 1.3))
                    .flex_none()
                    .whitespace_nowrap()
                    .child(SharedString::from(which.label()));
                let text = if src_empty {
                    // 这一部是空的：灰掉——点了也是空页，不如不点。
                    text.text_color(rgb(crate::theme::mix(ink_main, paper_hex, 0.62)))
                } else if on {
                    text.text_color(rgb(zhu))
                } else {
                    text.text_color(rgb(crate::theme::mix(ink_main, paper_hex, 0.34)))
                        .hover(move |s| s.text_color(rgb(zhu)))
                };
                let mut slip = div()
                    .id(SharedString::from(format!("jiandu-dict-tab-{:?}", which)))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.0))
                    .child(text)
                    .child(div().w_full().h(px(1.5)).bg(rule));
                if !src_empty && !on {
                    slip = slip
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.dict_tab = which;
                                cx.notify();
                            }),
                        );
                }
                row = row.child(slip);
            }
            row
        };

        // ── 字头条：左大字 + 拼音**独立成列**（下压一根朱丝线）──────────
        //
        // 拼音原先跟大字挤在同一行、宽度又没写死，字头一宽它就被压没了
        // （「游」那张签上拼音整栏空白，一半是数据、一半是这么挤掉的）。
        // 现在：大字 `flex_none`，拼音也 `flex_none + whitespace_nowrap`，
        // 长读音（如「lè yuè」）可自动落到大字下面另起一行。
        let word4: String = word.chars().take(4).collect();
        let head_glyph = div()
            .flex_none()
            .font_family(main_family.clone())
            .text_size(px(head_fs))
            .line_height(px(head_fs * 1.2))
            .text_color(rgb(ink_main))
            .child(SharedString::from(word4));
        let py_row = {
            let long = pinyin.chars().count() > 10;
            let py = div()
                .flex_none()
                .font_family(latin_family.clone())
                .text_size(px(py_fs))
                .line_height(px(py_fs * 1.35))
                .text_color(rgb(py_ink))
                .whitespace_nowrap()
                .child(SharedString::from(pinyin.clone()));
            if long {
                // 读音太长（多音字并列）：竖着接在大字下面，绝不横向挤压。
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_start()
                    .child(head_glyph)
                    .child(py)
            } else if pinyin.is_empty() {
                div().flex_none().flex().flex_row().child(head_glyph)
            } else {
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(head_glyph)
                    .child(py)
            }
        };
        // 字头条：纯字头 + 拼音，**不再压一根朱丝线**——那根线正好落在
        // 拼音下方，看着像「拼音后面多了一条横线符号」，用户要求去掉。
        let head_block = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(py_row);

        // 正文：一段段宋体释义，段间留白；行高写死（与预算同数），超长自滚。
        let mut body_col = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(para_gap));
        for p in &chosen {
            body_col = body_col.child(
                div()
                    .w(px(inner_w))
                    .font_family(main_family.clone())
                    .text_size(px(fs))
                    .line_height(px(line_h))
                    .text_color(rgb(ink_body))
                    .child(SharedString::from(p.clone())),
            );
        }
        let body_area = div()
            .id("jiandu-lookup-body")
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .child(body_col);

        // ── 底部一行：考據原典 / 拓印 / 檢索 / 收（小字，宋体，紧凑）────
        let zdic = crate::dict::zdic_url(&word);
        let link = |label: &'static str| -> Div {
            div()
                .flex_none()
                .font_family(main_family.clone())
                .text_size(px(11.0))
                .text_color(rgb(zhu))
                .cursor_pointer()
                .hover(move |s| s.text_color(rgb(ink_main)))
                .child(SharedString::from(label))
        };
        let zdic_u = zdic.clone();
        let zdic_word = word.clone();
        let btn_zdic = link("考據原典").on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if crate::app::open_external(&zdic_u) {
                    this.notice = format!("已用浏览器打开汉典：{zdic_word}");
                } else {
                    this.notice = "未能调用系统浏览器".to_string();
                }
                cx.notify();
            }),
        );
        let copy_key = hit.key;
        let btn_copy = link("拓印").on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
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
        let search_text = hit.text.clone();
        let btn_search = link("檢索").on_mouse_down(
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
        let btn_close = link("收").on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.bubble = None;
                this.close_lookup();
                cx.notify();
            }),
        );
        let bar = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(9.0))
            .child(btn_zdic)
            .child(btn_copy)
            .child(btn_search)
            .child(btn_close);

        // ── 便签本体：宣纸半透明 + 一根极细朱砂边 + 微弱阴影 ──────────
        div()
            .absolute()
            .left(px(place.left))
            .top(px(place.top))
            .w(px(w))
            .h(px(h))
            .bg(washi)
            .border_1()
            .border_color(rgb(zhu))
            .rounded(px(2.0))
            .shadow_sm()
            .p(px(pad))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(sec_gap))
            .child(tab_row)
            .child(head_block)
            .child(body_area)
            .child(bar)
            // 点浮签内部不让它冒到根容器的「点别处收起」监听上去。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
    }

    /// 样式浮层：纸色、字级、版式、注文排法。
    /// 笔记 / 批注浮层：列出全书笔记，并可在当前页写笔记。
    /// 竖排工具框：查询 / 划线 / 笔记 / 复制 / 分享（参照 03 图）。
    /// 点字弹出的工具条：在点击位置附近浮起一行 查询 / 划线 / 笔记 / 复制 / 分享。
    /// 不再常驻右上角，只有点了正文某个字才出现；点别处即收起。
    /// 每个工具都作用于被点击的那个字。
    /// 全文检索浮层：输入一个词，全书瞬时查找，跳到那一面并用朱砂红高亮。
    ///
    /// 输入框套 `ime::InputField` —— 中文输入法不走 keydown，
    /// 只有挂了 InputHandler 才能把组好的「长安」送回草稿。
    /// **全文检索**：右滑侧栏，古风单行输入（宣纸底 + 朱砂下划线），下列
    /// 结果列表——每条「前文摘录」，命中词以朱砂标出；点一条即瞬跳到该处
    /// 并点亮「抵达」高亮（复用检索命中的朱砂衬底模式）。
    fn render_search_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let vw = self.viewport_w;
        let slide = self.search_slide.clamp(0.0, 1.0);
        let panel_w = (vw * 0.34).clamp(280.0, 420.0);
        let off = -(1.0 - slide) * panel_w;
        let zhu = sheet_zhu(t);
        let ink = t.ink();
        let ink_soft = t.ink_soft();
        let paper_veil = ((t.paper() & 0xFFFFFF) << 8) | 0xF0;
        let sept = crate::theme::mix(t.paper(), t.ink(), 0.16);

        let q = self.search_draft.trim().to_string();
        let qlen = q.chars().count();
        let n = self.search_hits.len();
        let hint = if q.is_empty() {
            "輸入字詞後回車檢索".to_string()
        } else if n == 0 {
            format!("未找到「{q}」")
        } else {
            format!("「{q}」共 {n} 處 · 第 {}/{n}", self.search_idx + 1)
        };

        // 输入框：草稿按光标切成两段，中间立一根朱砂光标；外层只用一道
        // 朱砂下划线（无白框、无圆角、无底色块）——古风单行输入。
        let draft = self.search_draft.clone();
        let cursor = self.search_cursor.min(draft.chars().count());
        let before: String = draft.chars().take(cursor).collect();
        let after: String = draft.chars().skip(cursor).collect();
        let box_body = div()
            .flex()
            .flex_row()
            .items_center()
            .text_size(px(14.0))
            .text_color(rgb(ink))
            .child(SharedString::from(before))
            .child(div().w(px(1.5)).h(px(16.0)).bg(rgb(zhu)))
            .child(SharedString::from(after));
        let input = crate::ime::InputField::new(
            div()
                .w(px(panel_w - 48.0))
                .px_1()
                .py_2()
                .bg(rgb(t.paper()))
                .border_b_1()
                .border_color(rgb(zhu))
                .child(box_body),
            self.search_focus.clone(),
            cx.entity(),
        );

        let btn_prev = self
            .search_link("‹ 上一處", t)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.search_prev(cx)));
        let btn_next = self
            .search_link("下一處 ›", t)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.search_next(cx)));
        let btn_close = self
            .search_link("收", t)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.set_ui_state(AppUIState::None);
                cx.notify();
            }));

        // 结果列表：每条从书稿正文摘一段「前文 + 命中 + 后文」，命中词朱砂标出。
        let mut list = div()
            .id(SharedString::from("search-result-list"))
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll();
        if n == 0 && !q.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_size(px(11.0))
                    .text_color(rgb(ink_soft))
                    .child(SharedString::from("全文未檢得此詞。")),
            );
        } else if n == 0 {
            list = list.child(
                div()
                    .p_4()
                    .text_size(px(11.0))
                    .text_color(rgb(ink_soft))
                    .child(SharedString::from("輸入字詞，即檢即得。")),
            );
        }
        for i in 0..n {
            let (c, p, o) = self.search_hits[i];
            // 从书稿摘上下文（无论该处是否在当前页都摘得出来）。
            let ctx: Option<(String, String, String)> = self.book.as_ref().and_then(|b| {
                let para = b.chapters.get(c)?.paragraphs.get(p)?;
                let chars: Vec<char> = para.text.chars().collect();
                let len = chars.len();
                if o >= len {
                    return None;
                }
                let qs = qlen.min(len - o).max(1);
                let s = o.saturating_sub(6);
                let e = ((o + qs) + 6).min(len);
                let shown = |r: std::ops::Range<usize>| -> String {
                    chars[r].iter().map(|ch| self.shown(*ch)).collect()
                };
                Some((shown(s..o), shown(o..o + qs), shown(o + qs..e)))
            });
            let ctx = match ctx {
                Some(c) => c,
                None => ("".into(), q.clone(), "".into()),
            };
            let cur = i == self.search_idx;
            let mut row = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p_3()
                .cursor_pointer()
                .border_b_1()
                .border_color(rgb(sept))
                .when(cur, |d| d.bg(rgb(crate::theme::mix(t.paper(), zhu, 0.10))))
                .hover(|d| d.bg(rgb(crate::theme::mix(t.paper(), zhu, 0.06))))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.goto_hit(i, cx);
                }));
            // 一行：前文（淡墨）+ 命中（朱砂）+ 后文（淡墨）。
            let mut line = div()
                .flex()
                .flex_row()
                .items_center()
                .text_size(px(12.0))
                .child(div().text_color(rgb(ink_soft)).child(SharedString::from(ctx.0)))
                .child(
                    div()
                        .text_color(rgb(zhu))
                        .child(SharedString::from(ctx.1)),
                )
                .child(div().text_color(rgb(ink_soft)).child(SharedString::from(ctx.2)));
            row = row.child(line);
            if cur {
                row = row.child(
                    div()
                        .text_size(px(9.5))
                        .text_color(rgb(zhu))
                        .child(SharedString::from("▸ 當前")),
                );
            }
            list = list.child(row);
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(sept))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(zhu))
                    .child(SharedString::from("檢 索")),
            )
            .child(
                div()
                    .text_size(px(9.5))
                    .text_color(rgb(ink_soft))
                    .child(SharedString::from(hint)),
            );

        div()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .right(px(off))
            .w(px(panel_w))
            .flex()
            .flex_col()
            .bg(rgba(paper_veil))
            .border_l_1()
            .border_color(rgb(sept))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(sept))
                    .child(input)
                    .child(btn_prev)
                    .child(btn_next)
                    .child(btn_close),
            )
            .child(list)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
    }

    /// 检索侧栏里的极简文字链接（无底无框，悬停转朱砂）。
    fn search_link(&self, label: &'static str, t: Theme) -> Div {
        div()
            .px(px(4.0))
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(rgb(t.chrome_text()))
            .hover(|s| s.text_color(rgb(sheet_zhu(t))))
            .child(SharedString::from(label))
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

    /// 点字弹出的**竖排书签**（v1.40.0 重写）。
    ///
    /// 旧版是一条横排白底长条：九个现代按钮、圆角、chrome 底——像网页
    /// 右键菜单，横在书叶上既出戏又挡字。这里改成一枚真正的书签：
    ///   * **竖排**：每枚钮两个字自上而下叠成一列，整签窄窄一条；
    ///   * **古风**：半透明宣纸底、一根极细朱砂边、楷体、无任何现代边框
    ///     与图标，只在悬停时把墨色转成朱砂；
    ///   * **精简**：撤掉「划线」（已有朱批）与「分享」，改名——查询→查典、
    ///     笔记→朱批、复制→拓印，专名线（人名 / 地名 / 书名）照旧；
    ///   * **避让**：落位走 `hit::place_card`，锚点是**整段选区的包围盒**
    ///     （`Root::selected_anchor`），不是按下那一点——文字靠右则签在左、
    ///     文字靠下则签向上展开，绝不压住选中的字。
    fn render_glyph_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let Some(hit) = self.popup.clone() else {
            return div();
        };
        let vw = self.viewport_w;
        let vh = self.viewport_h;

        // ── 锚点：让开的是「整段选中的文字」 ──────────────────────────
        let anchor = self
            .selected_anchor()
            .or_else(|| {
                self.glyph_hit_at(hit.x, hit.y).map(|gb| crate::hit::Anchor {
                    x: gb.x,
                    y: gb.y,
                    w: gb.w,
                    h: gb.h,
                })
            })
            .unwrap_or(crate::hit::Anchor {
                x: hit.x - self.col_w * 0.5,
                y: hit.y - self.row_h * 0.5,
                w: self.col_w,
                h: self.row_h,
            });
        let anchor_rect = Some((anchor.x, anchor.y, anchor.w, anchor.h));
        let hit_xy = (hit.x, hit.y);

        // ── 尺寸：一条横排圆角面板（钮内文字竖排）────────────────────
        let fs = 14.5f32;
        let item_w = fs * 1.5;
        let item_h = fs * 2.5; // 两字一钮（竖排）
        let gap = 8.0; // 钮间距
        let pad = 6.0;
        let n: f32 = 7.0; // 查典 / 朱批 / 夹注 / 拓印 / 人名 / 地名 / 书名
        let w = pad * 2.0 + item_w * n + gap * (n - 1.0) + 2.0;
        let h = pad * 2.0 + item_h + 2.0;
        let place = crate::hit::place_card(
            &anchor,
            w,
            h,
            vw,
            vh,
            crate::hit::CARD_PAD,
            crate::hit::CARD_GAP,
        );

        // 楷体：毛笔质感，比黑体更像写在签上的字。
        let kai = crate::fonts::family_for_role(crate::fonts::FontRole::Note);
        let key = hit.key;
        let ch = hit.ch;

        // 一枚竖排钮：**无边框、无底色**，只有字；悬停转朱砂。
        let mut items: Vec<Div> = Vec::new();
        let push_btn = |label: &'static str, items: &mut Vec<Div>| {
            items.push(
                vertical_text(label, fs, kai.clone())
                    .w(px(item_w))
                    .text_color(rgb(t.ink()))
                    .cursor_pointer()
                    .hover(move |s| s.text_color(rgb(sheet_zhu(t)))),
            );
        };
        push_btn("查典", &mut items);
        push_btn("朱批", &mut items);
        push_btn("夹注", &mut items);
        push_btn("拓印", &mut items);
        push_btn("人名", &mut items);
        push_btn("地名", &mut items);
        push_btn("书名", &mut items);

        // 夹注：先看看这个字上有没有旧注，有就预填（算改写）。
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

        // ── 逐枚绑上动作（与上面 push 的次序一一对应）────────────────
        let mut items = items.into_iter();
        let b_query = items
            .next()
            .unwrap()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    // ★ 与离线字书**无缝衔接**：就地呼出康熙 + 說文浮签
                    // （`render_lookup_card` 走 `dict::query_dict`），锚点
                    // 用选区包围盒，浮签照旧不压字。
                    let text = if this.has_selection() {
                        this.selected_text()
                    } else {
                        ch.to_string()
                    };
                    this.bubble = Some(GlyphHit {
                        key,
                        ch,
                        text: text.clone(),
                        x: hit_xy.0,
                        y: hit_xy.1,
                    });
                    this.open_lookup(key, ch, text, Some(hit_xy), anchor_rect);
                    this.hover_card = false;
                    this.popup = None;
                    this.clear_selection();
                    cx.notify();
                }),
            );
        let b_note = items.next().unwrap().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.start_note_edit_at(Some(hit_xy));
                this.popup = None;
                cx.notify();
            }),
        );
        let b_gloss = items.next().unwrap().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                // 已有夹注则预填（算改写），没有则从空白起。
                this.start_note_inject(key, &existing_note, Some(hit_xy));
                this.popup = None;
                cx.notify();
            }),
        );
        let b_copy = items.next().unwrap().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if !this.has_selection() {
                    this.sel_anchor = Some(key);
                    this.sel_focus = Some(key);
                }
                this.copy_page_text();
                this.popup = None;
                this.clear_selection();
                cx.notify();
            }),
        );
        let b_person = items.next().unwrap().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.mark_selection(NoteKind::Person);
                this.popup = None;
                cx.notify();
            }),
        );
        let b_place = items.next().unwrap().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.mark_selection(NoteKind::Place);
                this.popup = None;
                cx.notify();
            }),
        );
        let b_book = items.next().unwrap().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.mark_selection(NoteKind::Book);
                this.popup = None;
                cx.notify();
            }),
        );

        // ── 面板本体：横排圆角面板，钮内文字竖排 ──────────────────────
        let paper_washi = rgba((t.paper() << 8) | 0xFA); // 半透明宣纸底（≈0.98）
        // 钮与钮之间**只用留白**分档（不画线、不加框）：一枚钮就是面板上的
        // 一列字，靠呼吸感隔开；面板整体横排，文字逐列竖读。
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(gap))
            .child(b_query);
        for b in [b_note, b_gloss, b_copy, b_person, b_place, b_book] {
            row = row.child(b);
        }

        div()
            .absolute()
            .left(px(place.left))
            .top(px(place.top))
            .w(px(w))
            .h(px(h))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .bg(paper_washi)
            .border_1()
            .border_color(rgb(sheet_zhu(t)))
            .rounded(px(7.0))
            .shadow_sm()
            .p(px(pad))
            .child(row)
            // 面板内不冒泡：否则一点钮，根容器的「点别处收起」就把面板收了。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
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

        // ② 续排：列数减一（长图更竖）。
        //    夹注经 gloss 闭包行内化——晒本一律带注，与 notes_inline 无关，
        //    但尊重「显示注释」总开关（note_chars_with_source 内部已判）。
        let cols2 = self.grid.columns.saturating_sub(1).max(3);
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
        // 章首页的章题列已含在 cols2 里（limit+1），不再额外加中缝列。
        let cols_total = cols2 as f32;
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
            // 晒本是单叶长图，无叶内中缝，恒不画鱼尾。
            show_fish_tail: false,
            notes_inline: true,
            interactive: false,
        };
        let sheet = self.render_ancient(
            &laid.glyphs,
            &laid.columns,
            false,
            laid.start_chapter,
            self.page,
            LeafHalf::Alone,
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
        list = list.child(help_row("⌘E", "版字面板", t));
        list = list.child(help_row("⌘L", "目录", t));
        list = list.child(help_row("⌘Z", "专注模式（Esc 退出）", t));
        list = list.child(help_row("⌘K", "笔记面板", t));
        list = list.child(help_row("⌘/", "帮助（本面板）", t));
        list = list.child(help_row("Esc", "退出专注 / 图片·文字分享 / 收起浮层", t));

        list = list.child(help_sec("文字", t));
        // v1.37.0：天头（版框上方那段留白）点一下就能写朱砂小字。
        list = list.child(help_row("点天头留白", "题「眉批」：⏎ 落笔 · Esc 弃", t));
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

    /// 朱批录：从书稿里收齐四类批注（眉批 / 夹注·通假 / 专名·书名线·着重 / 划线），
    /// 外加「页笔记」（无 charIndex，只按页码跳），每条都带上跳转锚点。
    fn note_entries(&self) -> Vec<NoteEntry> {
        let mut out = Vec::new();
        if let Some(b) = &self.book {
            for h in &b.head_notes {
                out.push(NoteEntry {
                    key: Some(h.key),
                    len: 1,
                    page: None,
                    kind: "眉批",
                    quote: self.quote_at(h.key, 10),
                    content: h.text.clone(),
                });
            }
            for (ci, ch) in b.chapters.iter().enumerate() {
                for (pi, p) in ch.paragraphs.iter().enumerate() {
                    for n in &p.notes {
                        if n.kind == NoteKind::Ruby {
                            continue; // 注音不是批注
                        }
                        let len = n.len.max(1);
                        let key = n.char_index(ci, pi);
                        let quote = self.quote_at(key, len);
                        let (kind, content) = match n.kind {
                            NoteKind::Note => ("夹注", n.text.clone()),
                            NoteKind::Tongjia => ("通假", n.text.clone()),
                            NoteKind::Person => ("专名", "人名专名线".into()),
                            NoteKind::Place => ("地名", "地名专名线".into()),
                            NoteKind::Book => ("书名", "书名波浪线".into()),
                            NoteKind::Emph => ("着重", "朱砂圈点".into()),
                            NoteKind::Ruby => ("", String::new()),
                        };
                        out.push(NoteEntry {
                            key: Some(key),
                            len,
                            page: None,
                            kind,
                            quote,
                            content,
                        });
                    }
                }
            }
        }
        for k in &self.underlines {
            out.push(NoteEntry {
                key: Some(*k),
                len: 1,
                page: None,
                kind: "划线",
                quote: self.quote_at(*k, 6),
                content: String::new(),
            });
        }
        let mut pages: Vec<(usize, String)> =
            self.notes.iter().map(|(k, v)| (*k, v.clone())).collect();
        pages.sort_by_key(|(k, _)| *k);
        for (pg, txt) in pages {
            out.push(NoteEntry {
                key: None,
                len: 0,
                page: Some(pg),
                kind: "页记",
                quote: String::new(),
                content: txt,
            });
        }
        out
    }

    /// 按锚点从书稿正文里摘一段原文——无论该字是否在当前页都摘得出来。
    fn quote_at(&self, key: (usize, usize, usize), len: usize) -> String {
        if let Some(b) = &self.book {
            if let Some(ch) = b.chapters.get(key.0) {
                if let Some(p) = ch.paragraphs.get(key.1) {
                    return p.text.chars().skip(key.2).take(len.max(1)).collect();
                }
            }
        }
        String::new()
    }

    /// **朱批录**：右滑侧栏，半透明宣纸底，朱砂细线列出全部批注；点一条即
    /// 收栏、瞬跳到该 charIndex 并点亮「抵达」高亮。
    fn render_notes_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let slide = self.notes_slide.clamp(0.0, 1.0);
        let panel_w = (vw * 0.36).clamp(300.0, 440.0);
        let off = -(1.0 - slide) * panel_w;
        let zhu = sheet_zhu(t);
        let ink = t.ink();
        let ink_soft = t.ink_soft();
        // 半透明宣纸底：纸色 94% 不透明（GPUI 无 backdrop-filter，用纸色近似磨砂）。
        let paper_veil = ((t.paper() & 0xFFFFFF) << 8) | 0xF0;
        // 极细墨线（条目分隔）：纸 + 墨 16%。
        let sept = crate::theme::mix(t.paper(), t.ink(), 0.16);
        let song = self.main_family();

        let entries = self.note_entries();
        let count = entries.len();

        let mut list = div()
            .id(SharedString::from("notes-list"))
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll();
        if entries.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_size(px(11.0))
                    .text_color(rgb(ink_soft))
                    .child(SharedString::from(
                        "尚無批注。讀中點字即興批點，或框選正文落墨。",
                    )),
            );
        }
        for (i, e) in entries.into_iter().enumerate() {
            let kind = e.kind;
            let quote = e.quote;
            let content = e.content;
            let key = e.key;
            let len = e.len;
            let page = e.page;
            let row = div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .p_3()
                .cursor_pointer()
                .border_b_1()
                .border_color(rgb(sept))
                .when(i == 0, |s| s.border_t_1().border_color(rgb(sept)))
                .hover(|s| s.bg(rgb(crate::theme::mix(t.paper(), zhu, 0.06))))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(k) = key {
                        this.jump_to_key(k, len, cx);
                    } else if let Some(pg) = page {
                        this.jump_to_page(pg, cx);
                    }
                }));
            let mut head = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(zhu))
                        .child(SharedString::from(kind.to_string())),
                );
            if !quote.is_empty() {
                head = head.child(
                    div()
                        .flex_1()
                        .text_size(px(11.5))
                        .text_color(rgb(ink_soft))
                        .child(SharedString::from(quote)),
                );
            }
            let mut row = row.child(head);
            if !content.is_empty() {
                row = row.child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(ink))
                        .child(SharedString::from(content)),
                );
            } else if kind == "划线" {
                row = row.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(zhu))
                        .child(SharedString::from("（朱線）")),
                );
            }
            list = list.child(row);
        }

        // 表头：朱批录 + 条数（朱砂宋体），另一侧一句极小提示「點選可躍」。
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(sept))
            .child(
                div()
                    .font_family(song.clone())
                    .text_size(px(12.5))
                    .text_color(rgb(zhu))
                    .child(SharedString::from(format!("朱批錄 {count}"))),
            )
            .child(
                div()
                    .font_family(song.clone())
                    .text_size(px(9.5))
                    .text_color(rgb(ink_soft))
                    .child(SharedString::from("點選可躍")),
            );

        div()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .right(px(off))
            .w(px(panel_w))
            .flex()
            .flex_col()
            .bg(rgba(paper_veil))
            .border_l_1()
            .border_color(rgb(sept))
            .child(header)
            .child(list)
            // 拦住冒泡：点面板内不触发外层（页面）的翻页 / 关闭。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
    }

    /// **「版字」面板**：列数 · 字级 · 字体 · 批注 · 留白 · 纸边 · 页面 ·
    /// 纸色 · 墨色 · 丝栏 · 标注——**一张纸、一页**，没有第二处入口。
    ///
    /// v1.35.0 终局合并：原先「版式」浮层（8/10/12/14 四档）、版字面板的
    /// 「字体」页与「样式」页是三副面孔——列数两处写、字体三行写，`set_font`
    /// 与 `set_font_preset` 各改各的，UI 上还互相打脸。现在全部并到这一页，
    /// 每一项只认一个数据源、只留一行控件：
    ///   列数 → 模块级 [`COLUMN_OPTS`]（值 `Option<usize>`，None = 自动）；
    ///   字级 → `ZoomLevel::ALL` → `set_zoom_level`；
    ///   字体 → `FontChoice::CLASSIC` → `Root::set_font`（正文 + 批注 +
    ///          行高一次换完，见 `set_font` 的注释）；
    ///   批注 → `FontChoice::NOTE` → `set_note_font`（字体行的二次微调）；
    ///   留白 → `banxin_ratio`；纸边 → `grid.margin`；页面 → `LayoutMode`；
    ///   纸色 → `theme::PAPER_THEMES`；墨色 → `InkPalette::ALL`；
    ///   丝栏 → `RuleColor::ALL`；标注 → `StyleAct`。
    fn render_style_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        // 面板宽度：比「单张页面」略收一档，紧凑不撑满（v1.46.0 瘦身）。
        let style_w = (self.page_w * 0.80).max(360.0);

        // ---- 抬头：一个名字 + 一枚关闭 ----
        //
        // 合并后只剩一页，原「样式 / 版字」两枚页签连 `StyleTab` 枚举一起删去：
        // 页签是「两页各管一套」时代的东西，现在没有第二页可切。
        let head = div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .pb(px(2.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.ink()))
                    .child(SharedString::from("版字")),
            )
            .child(panel_close(t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.show_style = false;
                    cx.notify();
                }),
            ));

        // 面板像一张**薄纸**：半透明宣纸底（纸色 + 95% 不透明）+ 一道极淡墨边，
        // 悬浮在古籍之上。GPUI 0.2.2 没有 backdrop-filter，纸底本色已足够
        // 「透」，再用一层柔和投影把它托起来（影子只向下，不四散）。
        // 纸色更透（0xF6）：磨砂比 v1.44 再弱一档，更贴近「一张素纸」。
        let paper_veil = ((t.paper() & 0xFFFFFF) << 8) | 0xF6;
        let mut sheet = div()
            .relative()
            .w(px(style_w))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .px(px(14.0))
            .py(px(10.0))
            .bg(rgba(paper_veil))
            .border_1()
            .border_color(rgba(((t.rule() & 0xFFFFFF) << 8) | 0x33))
            .rounded(px(2.0))
            .shadow(vec![BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.16),
                offset: gpui::point(px(0.0), px(6.0)),
                blur_radius: px(16.0),
                spread_radius: px(-4.0),
            }])
            // 面板文字一律走**正文宋体**（原来用思源黑体，正是「生硬」的根源）：
            // 面板贴在宋版书页下方，字要与书页同一种笔意。
            .font_family(self.main_family())
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.0))
                    .bg(rgba(0xFFFFFF33)),
            )
            .child(head);

        // ---- 列数：八行 / 十行 / 十二行 / 十四行 / 自动 ----
        //
        // 唯一数据源 `COLUMN_OPTS`：文案是古称，值是引擎要的
        // `Option<usize>`（None = 自动）。读写只过 `columns_pref` /
        // `set_columns_pref`——同一件事不再有第二处入口。
        // 切档不只是列变：列宽随之变，字号与行高跟着变，八行本翻开
        // 来就是宋版书那种疏朗大字（行款比例见 `grid::LAYOUT_CONFIGS`）。
        let cur = self.columns_pref();
        let mut cols_row = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(16.0))
            .flex_wrap();
        for (label, want) in COLUMN_OPTS {
            cols_row = cols_row.child(
                ink_option(label, cur == want, t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_columns_pref(want);
                        cx.notify();
                    }),
                ),
            );
        }
        sheet = sheet.child(style_row("列数", cols_row, t));

        // ---- 字级：蝇头 / 中楷 / 大字 / 榜书 四档 ----
        //
        // 不再是四颗方块按钮 + 「42pt」：方块是网页控件语汇，pt 是排版
        // 软件的单位，两者都不该出现在一张纸下面。现在只有四个词，
        // 选中的那个转朱砂、字下压一道 1px 墨线——像在书眉上圈了一个字。
        let cur_level = crate::types::ZoomLevel::from_zoom(self.zoom);
        let mut seg = div().flex().flex_row().items_start().gap(px(18.0)).py_1();
        for lvl in crate::types::ZoomLevel::ALL {
            let on = lvl == cur_level;
            seg = seg.child(
                div()
                    .id(SharedString::from(format!("zoom-lvl-{}", lvl.ancient_name())))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.0))
                    .cursor_pointer()
                    .text_color(rgb(if on { sheet_zhu(t) } else { t.chrome_text() }))
                    .hover(|s| if on { s } else { s.text_color(rgb(t.ink())) })
                    .child(
                        div()
                            .text_size(px(13.0))
                            .child(SharedString::from(lvl.ancient_name())),
                    )
                    .child(
                        div()
                            .w(px(26.0))
                            .h(px(1.0))
                            .flex()
                            .justify_center()
                            .child(if on {
                                hairline(26.0, t.ink())
                            } else {
                                div().w(px(0.0)).h(px(1.0))
                            }),
                    )
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
        sheet = sheet.child(style_row("字级", seg, t));

        // ---- 字体：竖排字牌，一牌一款 ----
        //
        // v1.35.0：原先「排版字体」（预设对）与「字体」（单换正文）两行写同一
        // 个 `font_ancient`，各写各的。现在只留这一行，选一款即走
        // `set_font`——正文 + 批注 + 行高缩放 + 重算尺寸一次做完。
        sheet = sheet.child(style_row("字体", self.font_choices_row(cx), t));
        // 批注字体：楷体二选一，与正文宋体分开——
        // 正文匠体、注文手写体，正是刻本的层次。字体行定完对子后，
        // 这里还能单独挑一款批注。
        sheet = sheet.child(style_row("批注", self.note_font_row(cx), t));

        // ---- 留白（左右）：版心宽 ÷ 单叶可用宽，三档 ----
        //
        // 「版心宽」就是左右留白比例的另一面：版心占得越窄，两侧留白
        // 越阔。宋版书版心只占纸宽六成（宽 = 复古），满屏档让文字区
        // 铺满叶面。天头率随档联动（`grid::top_frac_for`），
        // 且**天头恒为地脚的 1.5 倍**（`grid::TOP_OF_BOT`）——这一档只
        // 动左右，不动那条铁律。
        let mut bx_row = div().flex().flex_row().items_start().gap(px(16.0)).flex_wrap();
        for (label, val) in [
            ("宽 60%", crate::grid::BANXIN_RATIOS[0]),
            ("中 75%", crate::grid::BANXIN_RATIOS[1]),
            ("窄 90%", crate::grid::BANXIN_RATIOS[2]),
        ] {
            let on = (self.banxin_ratio - val).abs() < 0.005;
            bx_row = bx_row.child(
                ink_option(label, on, t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_banxin_ratio(val);
                        this.fit_sizes();
                        cx.notify();
                    }),
                ),
            );
        }
        sheet = sheet.child(style_row("留白", bx_row, t));

        // ---- 纸边：纸面四周留白（窄 / 中 / 宽）----
        let margin = self.grid.margin;
        let mut m_row = div().flex().flex_row().items_start().gap(px(16.0)).flex_wrap();
        for (label, val) in [("窄", 0.02f32), ("中", 0.06), ("宽", 0.12)] {
            let on = (margin - val).abs() < 0.005;
            m_row = m_row.child(
                ink_option(label, on, t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_margin(val);
                        this.fit_sizes();
                        cx.notify();
                    }),
                ),
            );
        }
        sheet = sheet.child(style_row("纸边", m_row, t));

        // ---- 页面：对开 / 单叶 / 整页（三模式）+ 「自动」开关 ----
        //
        // 自动开着时由屏幕宽高比在对开 ⇄ 单叶之间自动响应；手动选任一
        // 档即退出自动（见 `Root::set_layout_mode`）。整页是「看整体
        // 版式」的手动档，缩放到整叶塞进屏幕。
        // 这一排管的是**怎么铺**（对开 / 单叶 / 整页），与上面管字多大的
        // 列数是两回事，故不再都叫「版式」。
        let cur_mode = self.layout_mode;
        let mut modes = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(16.0))
            .flex_wrap();
        for m in crate::grid::LayoutMode::ALL {
            let on = cur_mode == m;
            modes = modes.child(
                ink_option(m.label(), on, t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_layout_mode(m);
                        cx.notify();
                    }),
                ),
            );
        }
        modes = modes.child(
            ink_option("自动", self.auto_duplex, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.auto_duplex = !this.auto_duplex;
                    this.fit_sizes();
                    cx.notify();
                }),
            ),
        );
        sheet = sheet.child(style_row("页面", modes, t));

        // ---- 纸色：**遍历主题配置表自动生成**——新增一套配色只需在
        // `PAPER_THEMES` 加一行，此处零改动 ----
        //
        // 每套配色是一颗**圆形小点**（不是方块）：点面=纸色，
        // 细边=界栏色，点名=极小宋体；选中的那颗套一圈朱砂细环，
        // 点名转朱砂。八颗点排列在一起，像一排印泥调色盘。
        let mut themes_row = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(13.0))
            .flex_wrap();
        for (i, pt) in crate::theme::PAPER_THEMES.iter().enumerate() {
            let on = self.theme.index() == i;
            let name = pt.name;
            let dot = div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(pt.paper))
                .border_1()
                .border_color(rgb(t.chrome_line()));
            themes_row = themes_row.child(
                div()
                    .id(SharedString::from(format!("theme-{}-{}", pt.id, i)))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(3.0))
                    .cursor_pointer()
                    .child(
                        // 选中的外面再套一圈极淡朱晕（环的余韵），
                        // 未选的只是干净一颗点。
                        dot.when(on, |d| d.border_2().border_color(rgb(sheet_zhu(t)))),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(rgb(if on { sheet_zhu(t) } else { t.ink_soft() }))
                            .child(SharedString::from(name)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            // 换纸要走 `set_theme`（fit_sizes + 持久），
                            // 直接赋值 `this.theme` 只换背景，夹注兑淡
                            // 与界栏不会跟着变。
                            this.set_theme(i);
                            cx.notify();
                        }),
                    ),
            );
        }
        sheet = sheet.child(style_row("纸色", themes_row, t));

        // ---- 墨色：五套正文 / 小字注色（两枚小色点 + 一行墨字）----
        sheet = sheet.child(style_row("墨色", self.palette_chips(cx), t));

        // ---- 丝栏：乌 / 金 / 朱 三色界行（竖排列间那道细线）----
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
                            .w(px(32.0))
                            .h(px(26.0))
                            .bg(rgb(t.paper()))
                            .border_1()
                            .border_color(rgb(if on { sheet_zhu(t) } else { t.rule() }))
                            .rounded(px(2.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .gap(px(4.0))
                            .child(div().w(px(1.5)).h(px(14.0)).bg(rgb(rule_c)))
                            .child(div().w(px(1.5)).h(px(14.0)).bg(rgb(rule_c)))
                            .child(div().w(px(1.5)).h(px(14.0)).bg(rgb(rule_c))),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(if on { sheet_zhu(t) } else { t.ink_soft() }))
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

        // ---- 标注：通假 / 标点 / 生僻字 / 数字汉字 / 注释 / 繁体 ----
        //
        // v1.35.0：「简繁」原是单独一行、只有一枚开关（占一整行放一个字），
        // 并入标注这一排——它本来就是同一种「显示什么」的开关。
        let marks = self.style_chips(
            vec![
                ("通假", self.show_tongjia, StyleAct::Tong),
                ("标点", self.show_punct, StyleAct::Punct),
                ("生僻字", self.show_rare, StyleAct::Rare),
                ("数字汉字", self.han_digits, StyleAct::Han),
                ("注释", self.show_notes, StyleAct::Notes),
                (
                    if self.show_traditional { "繁体" } else { "简体" },
                    self.show_traditional,
                    StyleAct::Trad,
                ),
            ],
            cx,
        );
        sheet = sheet.child(style_row("标注", marks, t));

        // ---- 实时反馈：引擎当下算出的列数 / 版心 / 列宽 / 字号 / 每面行款 ----
        // 自动档每帧换挡，这一行是「当前到底排成什么样」的唯一凭据。
        // 层次化：**版心% 与 天头:地脚** 用半透明朱砂微微高亮（核心参数），
        // 其余（列数 / 列宽 / 字号 / 行数）用淡墨，让这行字有主次感。
        let cols_auto = if self.columns_auto { "（自动）" } else { "" };
        let banxin = self.banxin_ratio * 100.0;
        let ratio = self.margin_top / self.margin_bot.max(0.01);
        let zhu = rgba((sheet_zhu(t) << 8) | 0xCC);
        let soft = rgb(t.ink_soft());
        let seg = |s: String, z: bool| {
            div()
                .text_color(if z { zhu } else { soft })
                .whitespace_nowrap()
                .child(SharedString::from(s))
        };
        sheet = sheet.child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .text_size(px(10.5))
                .child(seg(format!("{} 列{}", self.grid.columns, cols_auto), false))
                .child(seg(format!("版心 {:.0}%", banxin), true))
                .child(seg(format!("列宽 {:.0}", self.col_w), false))
                .child(seg(format!("字号 {:.0}", self.size_ancient), false))
                .child(seg(format!("每面 {} 行", self.grid_rows), false))
                .child(seg(format!("天头:地脚 {:.1}:1", ratio), true)),
        );
        // ---- 底行：两枚**纯文字**动作（分享图片 / 分享文字）----
        // 原来那两个灰色大方框是面板里最重的色块；现在只剩一行字，字旁一枚
        // 极小的线框符号（画框=图片，简册=文字），悬停时字色加深。
        let share_img = text_action("分享图片", ActionIcon::Frame, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.show_share_panel = true;
                cx.notify();
            }),
        );
        let share_txt = text_action("分享文字", ActionIcon::Slips, t).on_mouse_down(
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
                .items_center()
                .gap(px(24.0))
                .pt(px(6.0))
                // 与上一行之间一道极淡的墨界，像纸上的折痕。
                .border_t_1()
                .border_color(rgba(((t.rule() & 0xFFFFFF) << 8) | 0x1F))
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

    /// 版字面板底部的**纯文字动作**（分享图片 / 分享文字）已改为自由函数
    /// `text_action`：没有灰底方框，只有一行字与一枚极小线框符号。
    ///
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

    /// 版字面板里的一组小选项（标注 / 简繁等开关）。
    /// 一律无底无框：只有字与字下那道线。
    fn style_chips(
        &self,
        items: Vec<(&'static str, bool, StyleAct)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        let mut row = div().flex().flex_row().flex_wrap().items_start().gap(px(16.0));
        for (label, on, act) in items {
            row = row.child(
                ink_option(label, on, t).on_mouse_down(
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

    /// 版字面板的「墨色」行：两枚小色点（正文 / 注文）+ 一行墨字。
    /// 原来是「深墨实底胶囊」，在纸面上像贴了两颗黑钮；现在只剩字与色点。
    fn palette_chips(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let mut row = div().flex().flex_row().flex_wrap().items_start().gap(px(16.0));
        for p in InkPalette::ALL {
            let on = self.palette == p;
            // 墨色图标：**两枚等大的圆并排**（左=正文色、右=注文色），
            // 中间 2px 缝隙；选中时外围套一圈极细朱砂框（v1.46.0）。
            let dot = |c: u32| div().w(px(16.0)).h(px(16.0)).rounded_full().bg(rgb(c));
            let icon = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .p(px(2.0))
                .rounded_full()
                .border_1()
                .border_color(rgba(0x00000000))
                .when(on, |d| d.border_2().border_color(rgb(sheet_zhu(t))))
                .child(dot(p.ink(t)))
                .child(dot(p.note(t)));
            let chip = div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.0))
                .cursor_pointer()
                .text_color(rgb(if on { sheet_zhu(t) } else { t.chrome_text() }))
                .hover(|s| if on { s } else { s.text_color(rgb(p.ink(t))) })
                .child(icon)
                .child(div().text_size(px(10.5)).child(SharedString::from(p.label())))
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

    fn note_font_row(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let cur = self.note_font();
        let mut row = div().flex().flex_row().items_start().gap(px(18.0)).py_1();
        for choice in FontChoice::NOTE {
            let on = cur == choice;
            let label = choice.label();
            // 字牌直接用该字体渲染自己的名字：选前就能看见楷体长什么样。
            row = row.child(
                div()
                    .id(SharedString::from(format!("note-font-{}", label)))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.0))
                    .cursor_pointer()
                    .font_family(crate::fonts::note_family(choice))
                    .text_color(rgb(if on { sheet_zhu(t) } else { t.chrome_text() }))
                    .hover(|s| if on { s } else { s.text_color(rgb(t.ink())) })
                    .child(div().text_size(px(13.0)).child(SharedString::from(label)))
                    .child(
                        div()
                            .h(px(4.0))
                            .flex()
                            .justify_center()
                            .child(if on {
                                div()
                                    .w(px(4.0))
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(rgb(sheet_zhu(t)))
                            } else {
                                div().w(px(4.0)).h(px(4.0))
                            }),
                    )
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

    /// 版字页的字体行：竖排字牌（参照图 2 的「朱雀仿宋 / 京华老宋 …」）。
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
            .gap(px(16.0))
            .py_1();
        for choice in opts {
            let on = cur == choice;
            let label = choice.label();
            // 竖排字牌：**没有底、没有框**。选中的字转朱砂，字列右侧压一道
            // 1px 墨线——竖排书里标重点，线就在字右，不在字下。
            let mut face = div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(4.0))
                .cursor_pointer();
            let mut col = div().flex().flex_col().items_center().gap(px(1.0));
            for c in label.chars() {
                col = col.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(if on { sheet_zhu(t) } else { t.chrome_text() }))
                        .child(SharedString::from(c.to_string())),
                );
            }
            face = face.child(col);
            let card = div()
                .id(SharedString::from(format!("font-{}", label)))
                .child(face);
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

    /// 循环切换当前模式的字体（版字页的竖排字牌是点选，这里是快捷键 F）。
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

    /// 循环切换纸色主题（同时联动默认墨色）。
    fn cycle_theme(&mut self) {
        let idx = Theme::ALL.iter().position(|x| *x == self.theme).unwrap_or(0);
        self.theme = Theme::ALL[(idx + 1) % Theme::ALL.len()];
        self.palette = self.theme.linked_palette();
        self.persist();
    }

    /// 切到第 `i` 套纸面主题（配置表 `PAPER_THEMES` 下标）。
    /// 换纸即重排（纸色变了、夹注兑淡基点也变）；并智能联动墨色——
    /// 每套纸都有默认墨色配对（宣纸→墨朱 / 米黄→赭朱 / 青简→松墨 /
    /// 夜读→暗金），一键换肤；用户随后仍可手动把墨色切到任意一套。
    fn set_theme(&mut self, i: usize) {
        let Some(next) = Theme::ALL.get(i).copied() else {
            return;
        };
        self.theme = next;
        self.palette = next.linked_palette();
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

/// 版字面板里的动作。
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

/// 版字面板的一行：左侧极小号标签（像书页边的批字），右侧一组控件。
/// 标签不做粗体、不加底色，只比正文淡一档。
fn style_row(label: &'static str, chips: Div, t: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(14.0))
        .child(
            div()
                .w(px(52.0))
                .pt(px(2.0))
                .text_size(px(10.0))
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
    use super::{
        bar_wants_show, chapters_ended_at, cn_date_from_str, cn_small, first_empty_column,
        note_grid_plan, sheet_side_pads, COLUMN_OPTS, BAR_GAP, BAR_H, BAR_HOT,
    };
    use crate::gloss::{Page, Piece};
    use crate::grid::LeafHalf;
    use crate::types::{FontChoice, FontPreset};

    /// 顶栏：贴着天头出、栏下沿附近赖着不走、深入纸面才收；有面板开着则永远不放。
    #[test]
    fn topbar_has_hysteresis_and_stays_while_a_panel_is_open() {
        // 收着时：只有探进天头 30px 才出来（鼠标停在 y=80 不动）。
        assert!(bar_wants_show(0.0, false, false));
        assert!(bar_wants_show(BAR_HOT, false, false));
        assert!(!bar_wants_show(BAR_HOT + 1.0, false, false));
        assert!(!bar_wants_show(400.0, false, false));
        // 打开后：门槛放宽到「栏底 + 回差」，紧贴栏下沿也不会立刻收。
        assert!(bar_wants_show(BAR_H + BAR_GAP, true, false));
        assert!(bar_wants_show(BAR_HOT + 1.0, true, false));
        assert!(!bar_wants_show(BAR_H + BAR_GAP + 1.0, true, false));
        // 回差确实存在：同一个 y，收着时不弹、开着时不收。
        let y = BAR_H + 10.0;
        assert!(!bar_wants_show(y, false, false));
        assert!(bar_wants_show(y, true, false));
        // 任一面板开着：鼠标在屏幕最底下也要留住。
        assert!(bar_wants_show(2000.0, false, true));
        assert!(bar_wants_show(2000.0, true, true));
    }

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

    /// Invariant 2（绝对居中对称）的**渲染**一侧：对开两叶的左右留白
    /// 严格镜像，纸面总宽恒等——左叶右缘与右叶左缘在同一条竖线上相接，
    /// 中缝因此不偏分毫地钉在整张纸正中（引擎侧的等分证明在
    /// `grid::frame_tests::facing_split_is_symmetric_and_the_gutter_sits_on_the_center_line`）。
    ///
    /// 这条测试守着「加粗外框 / 闭合中缝不许把两叶改胖改瘦」：
    /// 只要左右留白还镜像，两叶等宽就不依赖任何一边的具体数值。
    #[test]
    fn facing_sheets_are_mirrored_so_the_two_leaves_are_equal_width() {
        for pad in [0.0, 16.0, 44.0] {
            let (lpl, lpr) = sheet_side_pads(LeafHalf::Left, pad);
            let (rpl, rpr) = sheet_side_pads(LeafHalf::Right, pad);
            // 镜像：左叶的左边距 == 右叶的右边距，反之亦然。
            assert_eq!(lpl, rpr, "pad={pad} 左右叶不留白不镜像");
            assert_eq!(lpr, rpl, "pad={pad} 左右叶内侧留白不镜像");
            // 总宽恒等——「两叶一丝不差等宽」的直接来源。
            assert_eq!(lpl + lpr, rpl + rpr, "pad={pad} 两叶纸面总宽不等");
            // 内侧（贴中缝那一侧）必须收掉：两叶才能拼成一张纸。
            assert_eq!(lpr, 0.0, "左叶右缘（贴中缝）必须收掉留白");
            assert_eq!(rpl, 0.0, "右叶左缘（贴中缝）必须收掉留白");
            // 单叶两侧对称（居中独摆，不与任何叶拼幅）。
            assert_eq!(sheet_side_pads(LeafHalf::Alone, pad), (pad, pad));
        }
    }

    /// v1.35.0 去重：列数**只有一张表**（`COLUMN_OPTS`）。
    ///
    /// 原先「版式」浮层遍历 `LAYOUT_CONFIGS` 出四枚档位、版字面板又写死一份
    /// 四档数组，两处同时改 `columns_auto` / `grid.columns`。现在 UI 只认这
    /// 一张表，且**值是引擎要的整数或 None**（文案「八行」只是古称）。
    #[test]
    fn columns_has_one_single_source_of_truth() {
        // 恰好五档：八行 / 十行 / 十二行 / 十四行 / 自动。
        assert_eq!(COLUMN_OPTS.len(), 5, "列数档位必须是 5 档");
        let labels: Vec<&str> = COLUMN_OPTS.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            labels,
            vec!["八行", "十行", "十二行", "十四行", "自动"],
            "文案必须按刻本「一叶几行」的古称排序"
        );
        // 传给底层引擎的是整数或 None——不是文案、不是配置键。
        let values: Vec<Option<usize>> = COLUMN_OPTS.iter().map(|(_, v)| *v).collect();
        assert_eq!(
            values,
            vec![Some(8), Some(10), Some(12), Some(14), None],
            "八行→8、十行→10、十二行→12、十四行→14、自动→None"
        );
        // 四档整数必须都能在行款表里查到（否则点了没反应）。
        for (label, want) in COLUMN_OPTS {
            if let Some(n) = want {
                assert!(
                    crate::grid::LayoutConfig::by_columns(n).is_some(),
                    "{label}（{n} 列）在行款表里查不到"
                );
            }
        }
        // 无重复项：两处去重后不许再长出第二张表。
        let mut seen = std::collections::BTreeSet::new();
        for (_, want) in COLUMN_OPTS {
            assert!(seen.insert(want), "列数档位重复：{want:?}");
        }
    }

    /// v1.35.0：面板只剩**一页**「版字」——样式（纸墨标注）与版字（列数
    /// 字体字级留白）已合并，独立的「版式」浮层与「样式 / 版字」页签枚举
    /// 一并删去（`StyleTab` 已不存在，故这里只能钉死它的「不在」）。
    ///
    /// 字体同理只剩一行：正文字体 → 排版预设的换算唯一公式
    /// `FontPreset::for_main` 在，预设行就不会与字体行打架。
    #[test]
    fn font_preset_follows_the_body_font_so_the_two_rows_never_disagree() {
        // 匠体：正文宋体 / 思源宋体一路 → 刻本宋体（注用霞鹜文楷）。
        assert_eq!(FontPreset::for_main(FontChoice::WenJinMincho), FontPreset::Song);
        assert_eq!(FontPreset::for_main(FontChoice::MengYuanSerif), FontPreset::Song);
        // 写体：任何一款楷体 → 手写楷体（注用玄冬楷书）。
        assert_eq!(FontPreset::for_main(FontChoice::LXGWWenKai), FontPreset::Kai);
        assert_eq!(FontPreset::for_main(FontChoice::XuandongKaishu), FontPreset::Kai);
        // 正文与批注永远不同体——换哪一款都不至于「正文批注一个样」。
        for p in FontPreset::ALL {
            assert_ne!(p.main_choice(), p.note_choice(), "{p:?} 正文与批注同体了");
        }
    }
}
