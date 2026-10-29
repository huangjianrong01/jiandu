//! 书架视图：书衣网格 / 书脊列表 + 封面页。
//!
//! 默认「书衣网格」：一格一书，书是**瓷青布面**的线装本——右下角一道
//! 微小投影、右侧一条书脊线；左上角贴一枚宣纸**题签**，书名在签内竖排；
//! 作者与进度是极小的竖排小字，分踞左下角与右下角。可切「书脊列表」。
//! 顶栏有朱砂印章式的「继续阅读」、毛笔下划线的搜索框，下面一排四部
//! + 自定页签，选中的那道是极细墨线、未选的是虚墨线。
//!
//! 归类 / 改名 / 删除默认不露面，鼠标悬停到书衣上才在底部淡入一条
//! 半透明操作栏（极小楷体）。
//!
//! 视觉：宣纸米黄的桌面 + 暗青的书衣，墨线与朱印点缀，不加纹理。

use crate::app::{
    Root, Screen, ShelfSort, ShelfView, CINNABAR, FRAME, INK, PAPER,
};
use crate::fonts::FontRole;
use crate::library::{ellipsis, ShelfEntry};
use gpui::{
    canvas, div, point, prelude::*, px, rgb, rgba, Animation, AnimationExt, AnyElement, App, Bounds,
    BoxShadow, Context, Div, FontWeight, KeyDownEvent, MouseButton, MouseMoveEvent, ParentElement,
    PathBuilder, Pixels, SharedString, Stateful, Styled, Window,
};
use std::time::Duration;

// ---- 书架配色（宣纸米黄的桌面 + 暗青的书衣）----
/// 桌面：宣纸米黄，比纸面深半档，托住一本本暗青书衣。
const SHELF_BG: u32 = 0xEDE4D0;
/// 顶栏与页签条：宣纸本色。
const SHELF_PAPER: u32 = 0xF7F1E3;
/// 书衣：瓷青布面（暗青）——线装书衣的本色，与题签的米黄成对。
const CLOTH: u32 = 0x2F4858;
/// 书脊线：比书衣再深一档的灰蓝。竖排在右，书脊就在右。
const CLOTH_SPINE: u32 = 0x1B2B36;
/// 布面压出的棱线（书衣边框）。
const CLOTH_EDGE: u32 = 0x44607A;
/// 题签纸：宣纸米黄，贴在书衣左上角。
const SLIP_PAPER: u32 = 0xF3ECD9;
/// 题签的边框：旧纸边上那道浅褐。
const SLIP_EDGE: u32 = 0x8A6A52;
/// 题签上的墨字。
const SLIP_INK: u32 = 0x2A241E;
/// 书衣上的浅字（作者 / 卷册 / 进度）。
const CLOTH_FAINT: u32 = 0xA9B6BD;
/// 细线（界栏 / 分隔 / 下划线）。
const HAIR: u32 = 0xC9BDA6;
/// 次级墨（页签名）。
const INK_SOFT: u32 = 0x6B6357;
/// 三级墨（本数等旁注小字）。
const INK_FAINT: u32 = 0x9A9184;

// ---- 书衣几何 ----
/// 书衣（一张卡片）的宽 × 高。
pub(crate) const CARD_W: f32 = 156.0;
pub(crate) const CARD_H: f32 = 244.0;
/// 书脊线宽：右侧那一条。
const SPINE_W: f32 = 4.0;
/// 题签：左上角，宽 58、内里书名竖排可用高 132。
const SLIP_W: f32 = 58.0;
const SLIP_TITLE_H: f32 = 132.0;
/// 角上竖排小字（作者 / 进度）的字号与可用高。
const MINI_FS: f32 = 9.5;
const MINI_H: f32 = 52.0;

/// v1.49.3 极简书衣：宣纸底 + 淡墨线框，内里只一枚竖排书名。
const CARD_BG: u32 = 0xF7F1E3;
/// 淡墨描边（线框书衣的那道边）。
const CARD_EDGE: u32 = 0x8C8474;
/// 封面上的书名墨色。
const CARD_INK: u32 = 0x2A241E;

/// 书衣的右下角微小投影：只往右下偏一点点、模糊很小——「微弱立体感」，
/// 不是现代 UI 那种大团弥散阴影。`spread` 取负让影子收在书衣里面。
fn book_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: gpui::hsla(0.0, 0.0, 0.0, 0.28),
        offset: gpui::point(px(3.0), px(4.0)),
        blur_radius: px(9.0),
        spread_radius: px(-2.0),
    }]
}

/// 虚墨线：GPUI 0.2.2 没有 dashed 边框，用「若干 1px 短横」排出来。
/// 步长 4（2 实 2 虚），比实线轻，用来标**未选中**的页签。
fn hair_dashed(w: f32, color: u32) -> Div {
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

/// 顶栏的一枚「墨字 + 下划线」按钮：没有底色、没有边框、没有圆角——
/// 只是一行小字压在一道极细墨线上（水墨下划线），像书签上的题名。
/// `strong` 为朱色字（用于主要动作），否则次级墨色。
fn shelf_button(label: &str, strong: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(3.0))
        .cursor_pointer()
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(if strong { CINNABAR } else { INK_SOFT }))
                .child(SharedString::from(label.to_string())),
        )
        .child(hair_line_full(if strong { CINNABAR } else { HAIR }))
}

/// 撑满父宽的一根极细线（按钮下划线用；父级定宽后它跟着走）。
fn hair_line_full(color: u32) -> Div {
    div().w_full().h(px(1.0)).bg(rgb(color))
}

/// **朱砂印章**：一枚方印，白字（纸色）压在朱底上，字竖排。
/// 古印是方的，不做胶囊；`34×34` 在顶栏里正好一行高。
/// 悬停时整枚轻轻加深（GPUI 0.2.2 无元素级 transform，改不了尺寸缩放，
/// 只能换色表达「按下去」的意思）。
pub(crate) fn seal_chip(words: &str, size: f32) -> Div {
    // 每个字占一枚固定高的字格（size 的 46%），两字加间隙刚好落在印面里；
    // 定高才不会随字体行高浮动、把字挤出印面。
    let cell = size * 0.46;
    let mut inner = div().flex().flex_col().items_center().gap(px(1.0));
    for ch in words.chars() {
        inner = inner.child(
            div()
                .w(px(cell))
                .h(px(cell))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .text_size(px(size * 0.40))
                .text_color(rgb(PAPER))
                .font_family(crate::fonts::family_for_role(FontRole::Main))
                .child(SharedString::from(ch.to_string())),
        );
    }
    div()
        .w(px(size))
        .h(px(size))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .bg(rgb(CINNABAR))
        .rounded(px(2.0))
        .cursor_pointer()
        .child(inner)
}

/// 一枚「文字链」式的管理按钮（很轻，不抢题签的视线）。
/// 走**批注字体**（楷体）且字号极小——现代黑体的管理按钮一出现，
/// 古籍的气质就散了。
fn link_button(label: &str, color: u32) -> Div {
    div()
        .px(px(5.0))
        .py(px(2.0))
        .cursor_pointer()
        .text_size(px(9.0))
        .font_family(crate::fonts::family_for_role(FontRole::Note))
        .text_color(rgb(color))
        .child(SharedString::from(label.to_string()))
}

/// 一行字。
fn text(s: &str, size: f32, color: u32) -> Div {
    div()
        .text_size(px(size))
        .text_color(rgb(color))
        .child(SharedString::from(s.to_string()))
}

/// 一行字（按字数截断，保证不换行、不撑破卡片）。
fn text_fit(s: &str, size: f32, color: u32, max_chars: usize) -> Div {
    div()
        .h(px(size + 3.0))
        .overflow_hidden()
        .text_size(px(size))
        .text_color(rgb(color))
        .child(SharedString::from(ellipsis(s, max_chars)))
}

/// 一个字格：定宽定高、字居中。
///
/// 竖排的每列是「一列 flex_col，每字一格」。GPUI 的文本行高随字体度量浮动
/// （通常比字号大 20%~40%），不给定高时列会被撑高、溢出盒子——封面大书名、
/// 阅读页书名签、书架题签都因此被裁过。定成 `fs + 2` 后行距可精确计算，
/// `fit_*` 的容量估算才和实际渲染一致。
fn glyph_box(ch: char, fs: f32, color: u32, weight: FontWeight) -> Div {
    div()
        .w(px(fs + 2.0))
        .h(px(fs + 2.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(fs))
        .font_weight(weight)
        .text_color(rgb(color))
        .child(SharedString::from(ch.to_string()))
}

/// 竖排的行距：一格（fs+2）+ 字间留白 4。
fn row_pitch(fs: f32) -> f32 {
    fs + 6.0
}

/// 把已经分好列的字排成竖排块（首列在最右，与古籍一致）。
/// 字间距 1px：书衣题签排得紧，长书名才塞得进一列（与 `fit_single_column`
/// 的 `fs + 3` 行距一致）。
pub(crate) fn vertical_cols(cols: &[String], fs: f32, color: u32, weight: FontWeight) -> Div {
    let mut row = div().flex().flex_row().items_start().gap(px(4.0));
    for col in cols.iter().rev() {
        let mut c = div().flex().flex_col().items_center().gap(px(1.0));
        for ch in col.chars() {
            c = c.child(glyph_box(ch, fs, color, weight));
        }
        row = row.child(c);
    }
    row
}

/// 竖排分列，并**限定最多几列**：题签最多两列，超出即缩字号，
/// 缩到最小仍放不下就截断缀省略号（调用方据此给出「展开」入口）。
/// 返回 (字号, 各列文字[首列在最右])。
fn fit_cols(text: &str, box_w: f32, box_h: f32, base_fs: f32, max_cols: usize) -> (f32, Vec<String>) {
    const GAP: f32 = 4.0;
    const MIN_FS: f32 = 12.0;
    // 竖排里字距由字格体现，空格只会排出一个空洞；先滤掉。
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut fs = base_fs.max(MIN_FS);
    loop {
        let per_col = ((box_h - GAP) / row_pitch(fs)).floor().max(1.0) as usize;
        // 每列占 (fs+2) 宽 + 4 留白 = row_pitch，与竖排列宽一致。
        let cols_by_w = (box_w / row_pitch(fs)).floor().max(1.0) as usize;
        let cap = per_col * cols_by_w.min(max_cols.max(1));
        if chars.len() <= cap || fs <= MIN_FS {
            let fits = chars.len() <= cap;
            let take_n = if fits { chars.len() } else { cap.saturating_sub(1) };
            let mut s: String = chars.iter().take(take_n).collect();
            if !fits {
                s.push('…');
            }
            let clipped: Vec<char> = s.chars().collect();
            let cols = clipped
                .chunks(per_col)
                .map(|c| c.iter().collect::<String>())
                .collect();
            return (fs, cols);
        }
        fs = (fs - 1.0).max(MIN_FS);
    }
}

/// 单行输入框的取景窗：把光标附近 `max` 个字取出来，
/// 返回 (是否省略了前文, 光标前文字, 光标后文字)。
fn caret_window(draft: &str, cursor: usize, max: usize) -> (bool, String, String) {
    let chars: Vec<char> = draft.chars().collect();
    let cursor = cursor.min(chars.len());
    let max = max.max(1);
    let start = if chars.len() > max {
        cursor.saturating_sub(max / 2).min(chars.len() - max)
    } else {
        0
    };
    let end = (start + max).min(chars.len());
    let cut = cursor.clamp(start, end);
    let head: String = chars[start..cut].iter().collect();
    let tail: String = chars[cut..end].iter().collect();
    (start > 0, head, tail)
}

/// 横排书名按可用宽度截断：中文一字约一个字身宽、拉丁字符约半个，
/// 排不下就缀省略号，保证不撑破卡片列宽（作者名另起一行小字号列出）。
fn clip_title_w(s: &str, width: f32, fs: f32) -> String {
    let total: usize = s.chars().count();
    let mut used = 0.0f32;
    let mut out = String::new();
    for c in s.chars() {
        let w = if c.is_ascii() { fs * 0.55 } else { fs };
        if used + w > width - fs {
            break; // 给省略号留一个字身位
        }
        used += w;
        out.push(c);
    }
    if out.chars().count() == total {
        s.to_string()
    } else {
        out.push('…');
        out
    }
}

/// 把书名排成「适应盒子的竖排块」：先按盒高盒宽选出能放下的字号，
/// 一行（一列）排不下就自动分成多列——真实古籍正是从右往左成列，
/// 首列在最右；字号缩到最小仍装不下时，末列缀省略号截断。
/// 返回 (字号, 各列文字[首列在最右])。
fn fit_vertical(text: &str, box_w: f32, box_h: f32, base_fs: f32) -> (f32, Vec<String>) {
    const GAP: f32 = 4.0;
    const MIN_FS: f32 = 13.0;
    let chars: Vec<char> = text.chars().collect();
    let mut fs = base_fs.max(MIN_FS);
    loop {
        let per_col = ((box_h - GAP) / row_pitch(fs)).floor().max(1.0) as usize;
        let max_cols = (box_w / row_pitch(fs)).floor().max(1.0) as usize;
        let cap = per_col * max_cols;
        let fits = chars.len() <= cap;
        if fits || fs <= MIN_FS {
            // 放得下：全量分列；放不下且已最小字号：按容量截断缀省略号。
            let take_n = if fits { chars.len() } else { cap.saturating_sub(1) };
            let mut s: String = chars.iter().take(take_n).collect();
            if !fits {
                s.push('…');
            }
            let clipped: Vec<char> = s.chars().collect();
            let cols = clipped
                .chunks(per_col)
                .map(|c| c.iter().collect::<String>())
                .collect();
            return (fs, cols);
        }
        fs = (fs - 2.0).max(MIN_FS);
    }
}

/// 竖排书名「单列自适应」：永远只排成一列，字号随书名长度自适应收缩，
/// 缩到最小（8pt）仍放不下才截断缀省略号（调用方据此给「展开」入口）。
/// 返回 (字号, 这一列的文字, 是否被截断)。用于书衣（题签 / 书脊 / 封面）
/// ——线装书衣上的题签本就是一竖条，长书名靠缩字号而不是折成多列。
pub(crate) fn fit_single_column(text: &str, box_h: f32, base_fs: f32) -> (f32, String, bool) {
    const GAP: f32 = 2.0;
    const MIN_FS: f32 = 6.0;
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut fs = base_fs.max(MIN_FS);
    loop {
        // 行距与 `vertical_cols` 一致：字格 fs+2、字间 1px → fs+3。
        let per_col = ((box_h - GAP) / (fs + 3.0)).floor().max(1.0) as usize;
        let fits = chars.len() <= per_col;
        if fits || fs <= MIN_FS {
            let take_n = if fits { chars.len() } else { per_col.saturating_sub(1) };
            let mut s: String = chars.iter().take(take_n).collect();
            if !fits {
                s.push('…');
            }
            return (fs, s, !fits);
        }
        fs = (fs - 1.0).max(MIN_FS);
    }
}

/// 西文题名判定：拉丁字母（含数字）过半即按西文排。
///
/// 竖排里一个拉丁字母要占一格，`Poetry` 六个字母就是六格高——把英文
/// 书名强行竖排，一列排下去能拉出整张卡片那么长，正是要禁止的样子。
pub(crate) fn is_latin_title(s: &str) -> bool {
    let all: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if all.is_empty() {
        return false;
    }
    let latin = all.iter().filter(|c| c.is_ascii_alphanumeric()).count();
    latin * 2 >= all.len()
}

/// 断词：ASCII 连成一个词（中间的 `' - . , &` 也算词内），
/// 其余（中日韩文字与标点）一字一token——中英混排也能断得开。
fn latin_tokens(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let flush = |out: &mut Vec<String>, buf: &mut String| {
        if !buf.is_empty() {
            out.push(std::mem::take(buf));
        }
    };
    for c in s.chars() {
        if c.is_whitespace() {
            flush(&mut out, &mut buf);
        } else if c.is_ascii_alphanumeric() || matches!(c, '\'' | '-' | '.' | ',' | '&') {
            buf.push(c);
        } else {
            flush(&mut out, &mut buf);
            out.push(c.to_string());
        }
    }
    flush(&mut out, &mut buf);
    out
}

/// 一个字的排宽：拉丁约 0.55 字身，其余一个字身（与 `clip_title_w` 同口径，
/// 三处估算必须一致，否则「算得下、排不下」）。
fn char_w(c: char) -> f32 {
    if c.is_ascii() {
        0.55
    } else {
        1.0
    }
}

/// 一个词（token）的排宽。
fn token_w(tok: &str, fs: f32) -> f32 {
    tok.chars().map(|c| char_w(c) * fs).sum()
}

/// 按给定宽度把西文（或中西混排）题名**横排折行**。
/// 词间补一个窄空格，汉字之间不补——中文连排才不散。
/// 单词本身比一行还宽时按字硬断（只在字号已缩到最小仍放不下时发生，
/// `fit_latin_block` 会先试着缩字号让整词落下）。
fn wrap_latin(text: &str, box_w: f32, fs: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut used = 0.0f32;
    for tok in latin_tokens(text) {
        let tw = token_w(&tok, fs);
        // 整词就超出一行：按字硬断。
        if tw > box_w {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                used = 0.0;
            }
            for c in tok.chars() {
                let cw = char_w(c) * fs;
                if used + cw > box_w && !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                    used = 0.0;
                }
                cur.push(c);
                used += cw;
            }
            continue;
        }
        // 只有「两边都是拉丁」才补空格；汉字连排不补。
        let join = !cur.is_empty()
            && cur.chars().last().map(char_w).unwrap_or(1.0) < 1.0
            && tok.chars().next().map(char_w).unwrap_or(1.0) < 1.0;
        let gap = if join { fs * 0.30 } else { 0.0 };
        if !cur.is_empty() && used + gap + tw > box_w {
            lines.push(std::mem::take(&mut cur));
            used = 0.0;
        } else if join {
            cur.push(' ');
            used += gap;
        }
        cur.push_str(&tok);
        used += tw;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// 西文题名：**横排**，字号按盒宽自适应收缩、排满一行就折行。
///
/// 用户给的「整体旋转 90° 横躺」在 GPUI 0.2.2 里做不到——没有元素级
/// 旋转变换（`TransformationMatrix` 只在内部 sprite 用）。故取其第二个
/// 方案：**不竖排拉长，改横排 + 更小字号**，视觉上就是洋装书横排的题签。
/// 返回 (字号, 各行文字, 是否被截断)。
pub(crate) fn fit_latin_block(
    text: &str,
    box_w: f32,
    box_h: f32,
    base_fs: f32,
) -> (f32, Vec<String>, bool) {
    const MIN_FS: f32 = 8.0;
    const LINE_RATIO: f32 = 1.34; // 行距：西文小字排紧一点
    let mut fs = base_fs.max(MIN_FS);
    loop {
        let lines = wrap_latin(text, box_w, fs);
        // 两道约束：① 最长的那个词要能整词落在一行里（否则先缩字号，
        //    不急着把单词切断）；② 折行后的总高不超盒高。
        let word_fits = latin_tokens(text)
            .iter()
            .all(|t| token_w(t, fs) <= box_w + 0.01);
        let height_fits = lines.len() as f32 * fs * LINE_RATIO <= box_h;
        if (word_fits && height_fits) || fs <= MIN_FS {
            let max_lines = (box_h / (fs * LINE_RATIO)).floor().max(1.0) as usize;
            let fits = lines.len() <= max_lines;
            let mut out = lines;
            if !fits {
                out.truncate(max_lines.max(1));
                // 末行缀省略号，告知「名字没排完」。
                if let Some(last) = out.last_mut() {
                    last.push('…');
                }
            }
            return (fs, out, !fits);
        }
        fs = (fs - 0.5).max(MIN_FS);
    }
}

/// 题签里的书名：中文竖排（单列自适应缩字号），**西文一律横排**。
/// `truncated` 为真时调用方给出「点开看全名」的入口。
pub(crate) fn slip_title(
    title: &str,
    box_w: f32,
    box_h: f32,
    base_fs: f32,
    color: u32,
) -> (Div, bool) {
    if is_latin_title(title) {
        let (fs, lines, cut) = fit_latin_block(title, box_w, box_h, base_fs.min(13.0));
        let mut col = div().flex().flex_col().items_center().gap(px(1.0));
        for ln in &lines {
            col = col.child(
                div()
                    .text_size(px(fs))
                    .text_color(rgb(color))
                    .child(SharedString::from(ln.clone())),
            );
        }
        (col, cut)
    } else {
        let (fs, s, cut) = fit_single_column(title, box_h, base_fs);
        (vertical_cols(&[s], fs, color, FontWeight::MEDIUM), cut)
    }
}

/// 极小号的**竖排**小字（书衣左下角的作者、右下角的卷册与进度）。
/// 横排的现代小字一出现就破功，故一律竖排，字号 9.5 上下。
fn vertical_mini(text: &str, box_h: f32, fs: f32, color: u32) -> Div {
    let (fs, s, _) = fit_single_column(text, box_h, fs);
    vertical_cols(&[s], fs, color, FontWeight::NORMAL)
}

/// 竖排文字块：逐字换行成列，用于书封与封面页；
/// 长书名自动分列（从右到左），不再溢出卡片。
pub(crate) fn vertical_block(
    text: &str,
    box_w: f32,
    box_h: f32,
    base_fs: f32,
    color: u32,
    weight: FontWeight,
) -> Div {
    let (fs, cols) = fit_vertical(text, box_w, box_h, base_fs);
    let mut row = div().flex().flex_row().items_start().gap(px(4.0));
    // 首列在最右：倒序添加，最后一列落在最左。
    for col in cols.iter().rev() {
        let mut c = div().flex().flex_col().items_center().gap(px(4.0));
        for ch in col.chars() {
            c = c.child(glyph_box(ch, fs, color, weight));
        }
        row = row.child(c);
    }
    row
}

fn small_button(label: &str) -> Div {
    div()
        .px_3()
        .py_1p5()
        .rounded(px(8.0))
        .bg(rgb(0xF3F0E9))
        .text_size(px(12.0))
        .text_color(rgb(0x4A443C))
        .child(SharedString::from(label.to_string()))
}

/// **每日一句**的数据源：本地 `assets/quotes.json`，按日期取模轮换。
///
/// v1.52.0：句库由「源码里写死 8 条」换成 **194 条的本地 JSON**
/// （`assets/quotes.json`，字段 `text / author / source`），编译期
/// `include_str!` 编进二进制——不必管 app bundle 里资源在哪，也不怕
/// 用户挪动工作目录。
///
/// 轮换：**今天距 Unix 元日的天数 % 总条数**，同一天开多少次都是同一句，
/// 过零点自然换下一句。
///
/// 两处消费：app.rs `render_brand`（顶栏品牌标语）与书架顶栏标题右侧
/// （`render_shelf_top`）。
pub(crate) fn daily_quote() -> Option<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Q {
        text: String,
        #[serde(default)]
        author: String,
        #[serde(default)]
        source: String,
    }
    // 编译期编入：改句库只需动 assets/quotes.json 重新编译。
    const QUOTES_JSON: &str = include_str!("../assets/quotes.json");
    let list: Vec<Q> = serde_json::from_str(QUOTES_JSON).unwrap_or_default();
    if list.is_empty() {
        return None;
    }
    let day = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        / 86_400) as usize;
    let q = &list[day % list.len()];
    // 出处：优先书名（加书名号），没有书名就退作者。
    let src = if q.source.trim().is_empty() {
        q.author.clone()
    } else {
        format!("《{}》", q.source)
    };
    Some((q.text.clone(), src))
}

impl Root {
    pub fn render_screen(&mut self, cx: &mut Context<Self>) -> Div {
        match self.screen {
            Screen::Shelf => self.render_shelf(cx),
            Screen::Cover => self.render_cover(cx),
            Screen::Reading => self.render_reading(cx),
        }
    }

    /// 书架：题签网格（默认）/ 书脊列表，配分类页签、搜索、排序。
    pub(crate) fn render_shelf(&mut self, cx: &mut Context<Self>) -> Div {
        let entries = self.shelf_filtered();

        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .bg(rgb(SHELF_BG));

        // v1.49.3：页签已并进顶栏这一条，不再单起第二行。
        root = root.child(self.render_shelf_top(cx));
        // v1.47.0：此处原有的「每日一句」旧组件已删除——顶栏品牌标语
        // （app.rs render_brand）已含同一句，书架中部再悬浮一份即重复。

        // 网格自己带滚动（`overflow_y_scroll` 会把 Div 变 Stateful<Div>），
        // 书脊列表不带，故统一收成 AnyElement 再挂上去。
        let body: AnyElement = if entries.is_empty() {
            self.render_shelf_empty(cx).into_any_element()
        } else {
            match self.shelf_view {
                ShelfView::TitleSlip => self.render_slip_grid(&entries, cx).into_any_element(),
                ShelfView::Spine => self.render_spine_list(&entries, cx).into_any_element(),
            }
        };
        // 滚动交回网格自己（v1.49.4）：顶栏与页签是**固定不动**的兄弟节点，
        // 只有下面这堵书墙会滚；`min_h(0)` 是 flex 子项能滚起来的前提，
        // 缺了它子项会被内容撑高、滚动条永不出现。
        root = root.child(div().flex_1().min_h(px(0.0)).child(body));
        // 批量管理：底部浮出一条操作栏（全选 / 删除所选 / 退出）。
        if self.shelf_manage {
            root = root.child(self.render_batch_bar(cx));
        }
        root = root.child(self.render_shelf_status());

        // 浮层：重命名 / 改归类 / 展开书名（「书库」浮层 v1.49.3 已整体删除）。
        if self.rename_idx.is_some() {
            root = root.child(self.render_rename_dialog(cx));
        }
        if self.shelf_recat.is_some() {
            root = root.child(self.render_recat_sheet(cx));
        }
        if self.shelf_expand.is_some() {
            root = root.child(self.render_title_expand(cx));
        }

        // 点纸面空白：收起搜索框与浮层（浮层内部各自 stop_propagation）。
        root.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                if this.shelf_searching || this.shelf_expand.is_some() || this.shelf_recat.is_some() {
                    this.shelf_searching = false;
                    this.shelf_expand = None;
                    this.shelf_recat = None;
                    cx.notify();
                }
            }),
        )
    }

    /// 当前页签 + 搜索词 + 排序之后的书目；下标仍是书架里的原下标。
    fn shelf_filtered(&self) -> Vec<(usize, ShelfEntry)> {
        let q = self.shelf_query.trim().to_lowercase();
        let mut v: Vec<(usize, ShelfEntry)> = self
            .library
            .books
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, e)| match &self.shelf_cat {
                None => true,
                Some(c) => e.cat_key() == c.as_str(),
            })
            .filter(|(_, e)| {
                q.is_empty()
                    || e.title.to_lowercase().contains(&q)
                    || e.author.to_lowercase().contains(&q)
                    || e.annotator.to_lowercase().contains(&q)
            })
            .collect();

        match self.shelf_sort {
            // 最近读过的在前；没读过的（last_read=0）自然落到后面，按入架顺序。
            ShelfSort::Recent => v.sort_by(|a, b| b.1.last_read.cmp(&a.1.last_read).then(a.0.cmp(&b.0))),
            ShelfSort::Title => v.sort_by(|a, b| a.1.title.cmp(&b.1.title)),
            ShelfSort::Progress => v.sort_by(|a, b| {
                b.1.progress()
                    .partial_cmp(&a.1.progress())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(&b.0))
            }),
            ShelfSort::Added => {}
        }
        v
    }

    /// 顶栏（v1.49.4 全新）：**就三样**——左边一枚大标题「书架」，右边
    /// 一枚放大镜 + 一枚「编辑」；下面一行才是胶囊页签。
    ///
    /// 排序 / 视图 / 题签 / 整理 / 导入 那一串文字钮全部撤掉（整理＝「编辑」），
    /// 天头这才空得下来。搜索框不常驻：点放大镜才在标题右侧展开下划线
    /// 输入框，写完再点一次收起。
    ///
    /// 顶栏与页签**固定在顶部**：它们是 `flex_col` 里独立的兄弟节点，
    /// 只有下面那层网格会滚，滚多远天头都不动。
    fn render_shelf_top(&mut self, cx: &mut Context<Self>) -> Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::SongTi,
            crate::types::Script::Ancient,
        );
        let managing = self.shelf_manage;
        let searching = self.shelf_searching;

        // 大标题：一枚「书架」，宋体、墨色、19px——不是 logo，就是一行字。
        let title = div()
            .font_family(song.clone())
            .text_size(px(19.0))
            .text_color(rgb(INK))
            .child(SharedString::from("书架"));

        // 「编辑 / 完成」：极小号宋体，无底无框，进整理态转朱。
        let edit = div()
            .font_family(song.clone())
            .text_size(px(12.0))
            .text_color(rgb(if managing { CINNABAR } else { INK_SOFT }))
            .cursor_pointer()
            .hover(|s| s.text_color(rgb(CINNABAR)))
            .child(SharedString::from(if managing { "完成" } else { "编辑" }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_shelf_manage();
                    cx.notify();
                }),
            );

        // 每日一句：`daily_quote()` 按日期取模，格式「一蓑烟雨任平生。
        // ——《定风波》」。极小号淡墨，紧挨大标题右侧——它只是今日的一点
        // 意思，绝不能抢「书架」二字的风头。
        let quote = div()
            .font_family(song.clone())
            .text_size(px(11.0))
            .text_color(rgb(INK_FAINT))
            .child(SharedString::from(match crate::shelf::daily_quote() {
                Some((text, src)) => format!("{text} ——{src}"),
                None => "为阅读创造环境".to_string(),
            }));

        // 「导入」：右上角搜索图标旁的一枚极简文字钮，点开系统文件选择器。
        // 走的是 `Root::import_dialog` → 后台线程 `import_streaming`（流式
        // 清洗管线，不是旧版整本读进内存那套），详见 app.rs。
        let import_btn = div()
            .font_family(song.clone())
            .text_size(px(12.0))
            .text_color(rgb(INK_SOFT))
            .cursor_pointer()
            .hover(|s| s.text_color(rgb(CINNABAR)))
            .child(SharedString::from("导入"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.import_dialog(cx);
                    cx.notify();
                }),
            );

        // 「整批导入」（v1.77.0）：选一个文件夹，里头的书挨个点检、后台
        // 一本一本地刻。与「导入」并排——一个是挑几本，一个是整批搬进来。
        let batch_btn = div()
            .font_family(song.clone())
            .text_size(px(12.0))
            .text_color(rgb(INK_SOFT))
            .cursor_pointer()
            .hover(|s| s.text_color(rgb(CINNABAR)))
            .child(SharedString::from("整批导入"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.import_folder_dialog(cx);
                    cx.notify();
                }),
            );

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.0))
            // 左上留白 92px：v1.52.0 恢复系统红绿灯后，那三枚圆点就压在这
            // 条天头的左端，标题必须让开，否则「书」字被红点盖住半个。
            .pl(px(92.0))
            .pr(px(40.0))
            .pt(px(18.0))
            .pb(px(12.0))
            .bg(rgb(SHELF_BG))
            .child(title)
            .child(quote);

        // 搜索：未点开时只是一枚放大镜；点开后原地展开下划线输入框。
        if searching {
            bar = bar.child(self.render_shelf_search(cx));
        }
        bar = bar.child(div().flex_1());
        // 右侧四件：**导入**、**整批导入**、**放大镜**、**编辑**。
        bar = bar.child(import_btn);
        bar = bar.child(batch_btn);
        bar = bar.child(self.render_search_icon(searching, cx));
        bar = bar.child(edit);

        // 第二行：胶囊页签（全部 / 经 / 史 / 子 / 集）。
        let tabs = div()
            .pl(px(92.0))
            .pr(px(40.0))
            .pb(px(16.0))
            .bg(rgb(SHELF_BG))
            .child(self.render_shelf_tabs(cx));

        div().flex().flex_col().child(bar).child(tabs)
    }
    /// 放大镜（搜索）：GPUI 0.2.2 没有图标集，这枚是 canvas 画的——
    /// 一圈细边 + 一根斜柄，16px，淡墨、悬停朱砂。
    fn render_search_icon(&self, on: bool, cx: &mut Context<Self>) -> Div {
        let ink = if on { CINNABAR } else { INK_SOFT };
        div()
            .w(px(20.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.opacity(0.65))
            .child(
                canvas(
                    |_b: Bounds<Pixels>, _w: &mut Window, _c: &mut App| (),
                    move |bounds: Bounds<Pixels>, _s: (), window: &mut Window, _c: &mut App| {
                        let ox = f32::from(bounds.origin.x);
                        let oy = f32::from(bounds.origin.y);
                        let bw = f32::from(bounds.size.width);
                        let bh = f32::from(bounds.size.height);
                        let r = bw * 0.30;
                        let cx0 = ox + bw * 0.42;
                        let cy0 = oy + bh * 0.42;
                        // 镜圈：四段二次曲线凑圆（与句读小圈同一套系数）。
                        let k = r * 0.9142;
                        let mut b = PathBuilder::stroke(px(1.2));
                        b.move_to(point(px(cx0 + r), px(cy0)));
                        b.curve_to(point(px(cx0), px(cy0 + r)), point(px(cx0 + k), px(cy0 + k)));
                        b.curve_to(point(px(cx0 - r), px(cy0)), point(px(cx0 - k), px(cy0 + k)));
                        b.curve_to(point(px(cx0), px(cy0 - r)), point(px(cx0 - k), px(cy0 - k)));
                        b.curve_to(point(px(cx0 + r), px(cy0)), point(px(cx0 + k), px(cy0 - k)));
                        b.close();
                        if let Ok(path) = b.build() {
                            window.paint_path(path, rgb(ink));
                        }
                        // 柄：从圈的右下斜出去。
                        let mut h = PathBuilder::stroke(px(1.2));
                        h.move_to(point(px(cx0 + r * 0.72), px(cy0 + r * 0.72)));
                        h.line_to(point(px(ox + bw * 0.86), px(oy + bh * 0.86)));
                        if let Ok(path) = h.build() {
                            window.paint_path(path, rgb(ink));
                        }
                    },
                )
                .w(px(20.0))
                .h(px(20.0)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.shelf_searching = !this.shelf_searching;
                    if !this.shelf_searching {
                        this.shelf_query.clear();
                        this.shelf_query_cursor = 0;
                    }
                    cx.notify();
                }),
            )
    }

    /// 书架搜索框：未激活时是一枚「搜索」按钮，点开后变成可接中文的输入框。
    fn render_shelf_search(&mut self, cx: &mut Context<Self>) -> Div {
        let active = self.shelf_searching;
        let draft = self.shelf_query.clone();
        let has_text = !draft.is_empty();

        let mut line = div().flex().flex_row().items_center().gap(px(2.0));
        if draft.is_empty() && !active {
            line = line.child(text("搜书名 / 作者", 11.5, INK_FAINT));
        } else {
            // 单行显示：长词只留光标附近的一段，前面缀省略号。
            let (skip, head, tail) = caret_window(&draft, self.shelf_query_cursor, 14);
            if skip {
                line = line.child(text("…", 12.0, INK_FAINT));
            }
            if !head.is_empty() {
                line = line.child(text(&head, 12.0, INK));
            }
            if active {
                line = line.child(div().w(px(1.5)).h(px(13.0)).bg(rgb(CINNABAR)));
            }
            if !tail.is_empty() {
                line = line.child(text(&tail, 12.0, INK));
            }
        }

        let body = crate::ime::InputField::new(line, self.shelf_query_focus.clone(), cx.entity());

        // 清除：一枚极小的墨色「✕」，不做圆形灰底（那也是现代控件）。
        let clear = div()
            .text_size(px(10.0))
            .text_color(rgb(INK_FAINT))
            .cursor_pointer()
            .child(SharedString::from("✕"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.shelf_query.clear();
                    this.shelf_query_cursor = 0;
                    cx.notify();
                }),
            );

        // **毛笔下划线输入框**：没有灰底、没有边框、没有圆角，只有底下
        // 一道极细墨线（落笔时转朱砂），像在纸上写下一行。
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .w(px(156.0))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .h(px(22.0))
                    .child(body)
                    .when(has_text, |d| d.child(clear)),
            )
            .child(hair_line_full(if active { CINNABAR } else { HAIR }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.shelf_searching = true;
                    cx.notify();
                }),
            )
    }

    /// 分类页签（v1.49.4 全新）：**胶囊**——全部 / 经 / 史 / 子 / 集。
    ///
    /// 选中：朱砂实底 + 白字；未选：只有一圈淡墨细边。不再挂本数，
    /// 也不再有下划线 tab 那套现代后台的做派。
    fn render_shelf_tabs(&mut self, cx: &mut Context<Self>) -> Div {
        let cur = self.shelf_cat.clone();
        let song = crate::fonts::family_for(
            crate::types::FontChoice::SongTi,
            crate::types::Script::Ancient,
        );
        let mut row = div().flex().flex_row().items_center().gap(px(10.0));
        for (key, label) in [
            ("全部", "全部"),
            ("经", "经"),
            ("史", "史"),
            ("子", "子"),
            ("集", "集"),
        ] {
            let selected = match (&cur, key) {
                (None, "全部") => true,
                (Some(c), _) => c == key,
                _ => false,
            };
            let pill = div()
                .px(px(14.0))
                .py(px(5.0))
                .rounded_full()
                .cursor_pointer()
                // 选中：朱砂实底 + 白字；未选：素纸底 + 淡墨边。
                .bg(rgb(if selected { CINNABAR } else { SHELF_PAPER }))
                .border_1()
                .border_color(rgb(if selected { CINNABAR } else { HAIR }))
                .font_family(song.clone())
                .text_size(px(12.0))
                .text_color(rgb(if selected { 0xFFFFFF } else { INK_SOFT }))
                .child(SharedString::from(label))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_cat = if key == "全部" {
                            None
                        } else {
                            Some(key.to_string())
                        };
                        this.shelf_expand = None;
                        cx.notify();
                    }),
                );
            row = row.child(pill);
        }
        row
    }
    /// 整理（批量管理）的底部操作栏：已选册数 · 全选 / 反选 / 清选 · 删除 · 退出。
    ///
    /// 控件一律走「极小字 + 朱砂」，不做灰底胶囊——与书衣同一套古风语汇。
    fn render_batch_bar(&mut self, cx: &mut Context<Self>) -> Div {
        let n = self.shelf_picked.len();
        let total = self.shelf_filtered().len();
        let confirm = self.shelf_batch_confirm;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(16.0))
            .px(px(18.0))
            .py(px(7.0))
            .bg(rgb(SHELF_PAPER))
            .border_t_1()
            .border_color(rgb(HAIR));

        bar = bar.child(text(
            &format!("已选 {n} 册 · 共 {total} 册"),
            12.0,
            if n > 0 { CINNABAR } else { INK_FAINT },
        ));
        bar = bar.child(link_button("全选", INK_FAINT).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                let entries = this.shelf_filtered();
                this.pick_all(&entries);
                cx.notify();
            }),
        ));
        bar = bar.child(link_button("反选", INK_FAINT).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                let entries = this.shelf_filtered();
                this.invert_pick(&entries);
                cx.notify();
            }),
        ));
        bar = bar.child(link_button("清选", INK_FAINT).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.clear_pick();
                cx.notify();
            }),
        ));
        bar = bar.child(div().flex_1());

        if confirm {
            // 删多本不可逆：二次确认，朱色字，旁边留一枚「点错了」。
            bar = bar
                .child(link_button(&format!("确认删除 {n} 册"), CINNABAR).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_picked();
                        cx.notify();
                    }),
                ))
                .child(link_button("点错了", INK_FAINT).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_batch_confirm = false;
                        cx.notify();
                    }),
                ));
        } else {
            bar = bar.child(
                link_button("删除所选", CINNABAR).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        if this.shelf_picked.is_empty() {
                            this.notice = "请先勾选要删除的书".to_string();
                        } else {
                            this.shelf_batch_confirm = true;
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        bar = bar.child(link_button("退出整理", INK_FAINT).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_shelf_manage();
                cx.notify();
            }),
        ));
        bar
    }

    /// 勾选框：一枚方形小印——未选是淡框，已选是朱砂底 + 宣纸白心。
    /// 不用「✓」：那枚符号是系统字形的西式控件语汇，在书衣上很跳。
    fn pick_box(picked: bool) -> Div {
        let mut b = div()
            .w(px(16.0))
            .h(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(1.0))
            .border_1()
            .border_color(rgb(if picked { CINNABAR } else { SLIP_EDGE }))
            .bg(rgba(if picked { 0xC0503CEE } else { 0x14100A66 }));
        if picked {
            b = b.child(div().w(px(7.0)).h(px(7.0)).bg(rgb(SLIP_PAPER)));
        }
        b
    }

    /// 书架**网格**（v1.51.1 全新）：卡片尺寸**写死**，靠 `flex-wrap` 折行。
    ///
    /// 卡片固定 **160 × 213**（严格 3:4），外层 `flex().flex_row().flex_wrap()
    /// .gap(px(24))` 自己算一排排得下几本：宽屏一排 6 本，窗口缩窄自动降到
    /// 5 / 4 / 3 本，卡片**绝不拉伸变形**。
    ///
    /// 旧版「按可用宽度把余头摊回卡片」是把 CSS 思路搬进 GPUI——摊完之后
    /// 卡片宽变成小数、子元素的绝对定位基准跟着飘，边界就散了。这一版一律
    /// 用 GPUI 原生 `px()` 常量，一行代码都不算宽度。
    ///
    /// 外面再套一层**垂直滚动**：书目过百也不挤，滚轮 / 触控板上下丝滑；
    /// 滚动条压成 2px 的极细一线，不做现代那条宽滑轨，免得污了书架。
    fn render_slip_grid(
        &mut self,
        entries: &[(usize, ShelfEntry)],
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        // 列距 24：书与书之间留一道白，一排书目才有呼吸。
        const GAP: f32 = 24.0;
        const SIDE: f32 = 40.0;
        let mut grid = div()
            // 满宽必写：滚动容器里的子项若宽度塌成「内容宽」，折行就按内容
            // 宽算、卡片会挤成一坨；钉成 100% 才按屏宽折行。
            .w_full()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_start()
            .justify_center()
            .gap(px(GAP))
            .px(px(SIDE))
            .py(px(24.0));
        for (idx, e) in entries {
            grid = grid.child(self.render_slip_card(*idx, e, cx));
        }
        // v1.77.0：批量导入时，还没刻完的书先以「待刻书衣」占个位——
        // 点检是毫秒级的，书名早就知道了，不必让用户盯着空书架等。
        for c in self.batch_pending_cards() {
            grid = grid.child(self.render_pending_card(&c));
        }
        div()
            .id(SharedString::from("shelf-grid-scroll"))
            .size_full()
            .overflow_y_scroll()
            // 2px：若隐若现的一条细线，不抢书衣的戏。
            .scrollbar_width(px(0.0))
            .child(grid)
            // 鼠标落在书与书之间的空当：撤掉悬停。书衣自己会
            // stop_propagation，故悬在书上时这一条不会执行。
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| {
                if this.shelf_hover.is_some() {
                    this.shelf_hover = None;
                    this.shelf_hover_seq = this.shelf_hover_seq.wrapping_add(1);
                    cx.notify();
                }
            }))
    }

    /// 一枚**书衣**（v1.51.1 全新）：纯 GPUI 原生布局，尺寸全部写死。
    ///
    /// 三件事，都靠 `px()` 钉死，不再有任何「按父容器算出来」的宽高：
    ///   * **封面**：`w(px(160)).h(px(213))` + `.relative()` + `.rounded(px(8))`
    ///     + 素色底（五档古纸色轮转）。`relative()` 是里头三样绝对定位的基准；
    ///   * **题签**：`absolute().top(10).left(10)`，白底小矩形，书名竖排；
    ///   * **作者**：`absolute().top(10).right(10)`，极小字竖排；
    ///   * **进度**：`absolute().bottom(10).left(10)`，未读的书不写。
    ///
    /// 封面**下方**（外层 `flex_col`）只有一行居中的横排书名——作者与其余
    /// 标签一概不再重复。
    fn render_slip_card(&mut self, idx: usize, e: &ShelfEntry, cx: &mut Context<Self>) -> Div {
        // 卡片尺寸：写死。3:4 的比例、160 的宽度，一排排几本交给 flex-wrap。
        const W: f32 = 160.0;
        const H: f32 = 213.0;
        // 题签：白底小矩形，贴着封面的左上角。
        const SLIP_W: f32 = 52.0;
        const SLIP_H: f32 = 96.0;

        let prog = e.progress();
        let started = prog > 0.0005;
        let pct = (prog * 100.0).round() as i32;
        let manage = self.shelf_manage;
        let picked = manage && self.is_picked(idx);
        let song = crate::fonts::family_for(
            crate::types::FontChoice::SongTi,
            crate::types::Script::Ancient,
        );
        // 封面色：五档**深青布面**轮转。上一版用「古纸色」当底，与书架底色
        // （0xEDE4D0）几乎同色，卡片边界整个消失、字像浮在空中——这是被骂
        // 的根因。深青压得住白题签，边界一眼就框得死。
        const CLOTH: [u32; 5] = [0x2F4858, 0x35494E, 0x3A4A44, 0x2E4348, 0x394451];
        let cloth = CLOTH[idx % CLOTH.len()];

        let title = vertical_block(
            &e.title,
            SLIP_W - 8.0,
            SLIP_H - 8.0,
            15.0,
            0x241F1A,
            FontWeight::MEDIUM,
        );

        let card = div()
            // 容器基准：里头三样东西的 absolute 全部相对这一枚封面定位。
            .relative()
            .w(px(W))
            .h(px(H))
            .rounded(px(8.0))
            .bg(rgb(cloth))
            .overflow_hidden()
            .cursor_pointer()
            // 整理态勾中：收一圈朱边（常态一条线都不画）。
            .when(picked, |d| d.border_1().border_color(rgb(CINNABAR)))
            // ① 左上题签：上、左各 10，白底小矩形，里面竖排书名。
            .child(
                div()
                    .absolute()
                    .top(px(10.0))
                    .left(px(10.0))
                    .w(px(SLIP_W))
                    .h(px(SLIP_H))
                    .p(px(4.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .bg(rgb(0xFFFFFF))
                    .child(title),
            )
            // ② 右缘作者：右 10、上 10，极小号浅灰竖排——像书脊那行小注。
            .child(
                div()
                    .absolute()
                    .right(px(10.0))
                    .top(px(10.0))
                    .max_h(px(H * 0.52))
                    .overflow_hidden()
                    .child(vertical_mini(&e.author_line(), H * 0.52, 8.5, 0xBCB19C)),
            )
            // ③ 左下进度：下、左各 10，极小号；未读的书不写「已读 0%」。
            .when(started, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(10.0))
                        .bottom(px(10.0))
                        .font_family(song.clone())
                        .text_size(px(9.0))
                        .text_color(rgb(0xC8BCA4))
                        .child(SharedString::from(format!("已读 {pct}%"))),
                )
            })
            // 整理态：右上角一枚勾选印。
            .when(manage, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(8.0))
                        .right(px(8.0))
                        .child(Self::pick_box(picked)),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if manage {
                        this.toggle_pick(idx);
                    } else {
                        this.open_book(idx);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.enter_manage_at(idx);
                    cx.notify();
                }),
            );

        // 封面下：**只有书名**——居中、横排、常规字号。作者与其余标签全删。
        let name_line = div()
            .w(px(W))
            .flex()
            .justify_center()
            .pt(px(10.0))
            .child(
                div()
                    .font_family(song)
                    .text_size(px(13.0))
                    .text_color(rgb(INK))
                    .child(SharedString::from(clip_title_w(&e.title, W - 4.0, 13.0))),
            );

        let hovered = self.shelf_hover == Some(idx);
        let confirming = self.delete_confirm == Some(idx);
        // v1.9.3：新书入架的 150ms 内整体渐显（封面 + 书名一起浮现），
        // 不再「啪」地蹦出。老书没记过出生，透明度恒为 1。
        let birth_key = e
            .store_id
            .clone()
            .unwrap_or_else(|| format!("whole:{}", e.title));
        let birth_op = self.shelf_card_opacity(&birth_key);
        let mut outer = div()
            .relative()
            .w(px(W))
            // **不许压**：flex 行里默认 `flex_shrink: 1`，一排塞不下 6 本时
            // GPUI 会把 160 的卡片压成 130，边界与比例全废。钉死「不收缩」，
            // 塞不下就让 `flex_wrap` 折行。
            .flex_shrink_0()
            .opacity(birth_op)
            .flex()
            .flex_col()
            .items_center()
            .child(card)
            .child(name_line);
        if (hovered || confirming) && !manage {
            let bar = self.render_manage_bar(idx, W, cx);
            let seq = self.shelf_hover_seq;
            outer = outer.child(bar.with_animation(
                SharedString::from(format!("shelf-manage-{idx}-{seq}")),
                Animation::new(Duration::from_millis(180)),
                |bar, delta| bar.opacity(delta),
            ));
        }
        outer.on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
            if this.shelf_hover != Some(idx) {
                this.shelf_hover = Some(idx);
                this.shelf_hover_seq = this.shelf_hover_seq.wrapping_add(1);
                cx.notify();
            }
            cx.stop_propagation();
        }))
    }

    /// 批量导入里**还没刻完**的书（点检已出真书名，正文还在后台刻）。
    fn batch_pending_cards(&self) -> Vec<jiandu_core::batch::Candidate> {
        let from = self.batch_consumed.min(self.batch_cards.len());
        self.batch_cards[from..].to_vec()
    }

    /// **待刻书衣**（v1.77.0）：与真书衣同形，只是布面褪成一档灰青、
    /// 右下角盖一枚「待刻」小印，**点不开**——它还没有正文。
    ///
    /// 为什么不做成能点开的半本书：半成的书点开是一片白纸，那比「还没
    /// 出现」更像崩了。占位卡不可点，刻完那一格自然换成真书衣。
    fn render_pending_card(&mut self, c: &jiandu_core::batch::Candidate) -> Div {
        const W: f32 = 160.0;
        const H: f32 = 213.0;
        const SLIP_W: f32 = 52.0;
        const SLIP_H: f32 = 96.0;
        // 褪色的布面：比真书衣（0x2F4858）浅一档、灰一档，一眼看出「未成」。
        const PENDING_CLOTH: u32 = 0x5C6B72;

        let song = crate::fonts::family_for(
            crate::types::FontChoice::SongTi,
            crate::types::Script::Ancient,
        );
        let title = vertical_block(
            c.display_name(),
            SLIP_W - 8.0,
            SLIP_H - 8.0,
            15.0,
            0x241F1A,
            FontWeight::MEDIUM,
        );

        let card = div()
            .relative()
            .w(px(W))
            .h(px(H))
            .rounded(px(8.0))
            .bg(rgb(PENDING_CLOTH))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top(px(10.0))
                    .left(px(10.0))
                    .w(px(SLIP_W))
                    .h(px(SLIP_H))
                    .p(px(4.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .bg(rgb(0xFFFFFF))
                    .child(title),
            )
            // 右下角「待刻」：朱砂小字，像一枚压在书衣上的印。
            .child(
                div()
                    .absolute()
                    .right(px(10.0))
                    .bottom(px(10.0))
                    .font_family(song.clone())
                    .text_size(px(10.0))
                    .text_color(rgb(CINNABAR))
                    .child(SharedString::from(if c.toc.is_empty() {
                        "待刻".to_string()
                    } else {
                        format!("待刻 · {} 章", c.toc.len())
                    })),
            );

        let name_line = div()
            .w(px(W))
            .flex()
            .justify_center()
            .pt(px(10.0))
            .child(
                div()
                    .font_family(song)
                    .text_size(px(13.0))
                    .text_color(rgb(INK_FAINT))
                    .child(SharedString::from(clip_title_w(c.display_name(), W - 4.0, 13.0))),
            );

        div()
            .relative()
            .w(px(W))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .child(card)
            .child(name_line)
            // 不挂 on_mouse_down：点它要落到网格空白处（撤悬停），
            // 绝不能当成「翻一本没有正文的书」。
    }

    /// 书脊列表：一行一册。
    fn render_spine_list(
        &mut self,
        entries: &[(usize, ShelfEntry)],
        cx: &mut Context<Self>,
    ) -> Div {
        let mut col = div().flex().flex_col().px(px(18.0)).py(px(6.0));
        for (idx, e) in entries {
            col = col.child(self.render_spine_row(*idx, e, cx));
        }
        div()
            .size_full()
            .overflow_hidden()
            .child(col)
            // 落在行外的空当：撤掉悬停（行自己会 stop_propagation）。
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| {
                if this.shelf_hover.is_some() {
                    this.shelf_hover = None;
                    this.shelf_hover_seq = this.shelf_hover_seq.wrapping_add(1);
                    cx.notify();
                }
            }))
    }

    fn render_spine_row(&mut self, idx: usize, e: &ShelfEntry, cx: &mut Context<Self>) -> Div {
        // 书脊题名：竖排单列，字号随书名长度自适应。
        let (fs, title_col, truncated) = fit_single_column(&e.title, 88.0, 13.0);
        let prog = e.progress();
        let pct = (prog * 100.0).round() as i32;
        let accent = e.accent;
        let author = e.author_line();
        let edition = e.edition_line();
        let volume = e.volume_line();

        let mut title_area = div()
            .w(px(46.0))
            .h(px(88.0))
            .flex()
            .justify_center()
            .items_start()
            .child(vertical_cols(&[title_col], fs, INK, FontWeight::MEDIUM));
        if truncated {
            title_area = title_area.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.shelf_expand = Some(idx);
                    cx.notify();
                }),
            );
        }

        let info = div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .flex_1()
            .child(text_fit(&author, 13.0, INK, 22))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(text_fit(&edition, 10.5, INK_FAINT, 14))
                    .child(text_fit(&volume, 10.5, INK_FAINT, 14)),
            );

        let seat = div()
            .w(px(120.0))
            .flex()
            .flex_col()
            .items_end()
            .gap(px(4.0))
            .child(text(&format!("{pct}%"), 11.0, CINNABAR))
            .child(
                div()
                    .w(px(120.0))
                    .h(px(2.0))
                    .bg(rgb(HAIR))
                    .child(div().h(px(2.0)).w(px(120.0 * prog)).bg(rgb(accent))),
            );

        let hovered = self.shelf_hover == Some(idx);
        let confirming = self.delete_confirm == Some(idx);
        let manage = self.manage_row(idx, cx, INK_FAINT, CINNABAR);
        // 批量管理态：行首一枚方形小印，点行即勾选。
        let batch = self.shelf_manage;
        let picked = batch && self.is_picked(idx);

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(HAIR))
            .cursor_pointer()
            .when(batch, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .child(Self::pick_box(picked)),
                )
            })
            // 书脊色条：一眼可辨的册色，不做木纹也不做纹理。
            .child(div().w(px(4.0)).h(px(88.0)).bg(rgb(accent)).rounded(px(1.0)))
            .child(title_area)
            .child(info)
            .child(seat)
            // 管理动作默认不露面，悬停（或等确认删除）时才出现；批量态不露。
            .when((hovered || confirming) && !batch, |d| d.child(manage))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if batch {
                        this.toggle_pick(idx);
                    } else {
                        this.open_book(idx);
                    }
                    cx.notify();
                }),
            )
            // 右键：直接进整理态并勾上这一本。
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.enter_manage_at(idx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                if this.shelf_hover != Some(idx) {
                    this.shelf_hover = Some(idx);
                    this.shelf_hover_seq = this.shelf_hover_seq.wrapping_add(1);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
    }

    /// 空书架：纸面上一句话 + 两枚按钮。
    fn render_shelf_empty(&mut self, cx: &mut Context<Self>) -> Div {
        let hint = if self.shelf_query.trim().is_empty() {
            "书架还空着"
        } else {
            "没有找到这本书"
        };
        let sub = if self.library.books.is_empty() {
            "支持 md / txt / epub / json"
        } else {
            "换个词，或把搜索清掉看看全部"
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(30.0))
                    .bg(rgb(SLIP_PAPER))
                    .border_1()
                    .border_color(rgb(HAIR))
                    .rounded(px(2.0))
                    .child(text(hint, 15.0, INK))
                    .child(text(sub, 11.5, INK_FAINT))
                    .child(
                        shelf_button("导入第一本书", true).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.import_dialog(cx)
                            }),
                        ),
                    ),
            )
    }

    /// 「改归类」浮层：经 / 史 / 子 / 集 / 自定书单 / 恢复自动。
    fn render_recat_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let idx = self.shelf_recat.unwrap_or(0);
        let cur = self
            .library
            .books
            .get(idx)
            .map(|e| e.category.clone())
            .unwrap_or_default();
        let title = self
            .library
            .books
            .get(idx)
            .map(|e| e.title.clone())
            .unwrap_or_default();

        let options: [(&str, &str); 6] = [
            ("", "按书名自动归类"),
            ("经", "经部"),
            ("史", "史部"),
            ("子", "子部"),
            ("集", "集部"),
            ("我的书单", "自定书单"),
        ];
        let mut col = div().flex().flex_col().gap(px(4.0));
        for (key, label) in options {
            let k = key.to_string();
            let selected = cur == k;
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(3.0))
                    .bg(rgb(if selected { 0xF1E7D3 } else { SLIP_PAPER }))
                    .cursor_pointer()
                    .child(text(label, 12.5, if selected { CINNABAR } else { INK }))
                    .child(text(if selected { "今在此" } else { "" }, 10.0, CINNABAR))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.recat_book(idx, &k);
                            cx.notify();
                        }),
                    ),
            );
        }

        let card = div()
            .w(px(260.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(16.0))
            .bg(rgb(SLIP_PAPER))
            .border_1()
            .border_color(rgb(FRAME))
            .rounded(px(4.0))
            .child(text(&format!("归类 · {}", ellipsis(&title, 12)), 13.0, CINNABAR))
            .child(col)
            .child(text("归到自定书单后，可在「自定」页签里找到", 10.0, INK_FAINT))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }));

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100A66))
            .child(card)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.shelf_recat = None;
                    cx.notify();
                }),
            )
    }

    /// 展开书名：题签放不下的长名，整幅竖排看全。
    fn render_title_expand(&mut self, cx: &mut Context<Self>) -> Div {
        let idx = self.shelf_expand.unwrap_or(0);
        let Some(e) = self.library.books.get(idx).cloned() else {
            return div();
        };
        let (fs, cols) = fit_cols(&e.title, 300.0, 280.0, 22.0, 6);
        let prog = e.progress();
        let pct = (prog * 100.0).round() as i32;

        let card = div()
            .w(px(380.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.0))
            .p(px(20.0))
            .bg(rgb(SLIP_PAPER))
            .border_1()
            .border_color(rgb(FRAME))
            .rounded(px(4.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(vertical_cols(&cols, fs, INK, FontWeight::MEDIUM)),
            )
            .child(div().w_full().h(px(1.0)).bg(rgb(HAIR)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(text(&e.author_line(), 12.5, INK))
                    .child(text(&e.edition_line(), 11.0, INK_FAINT))
                    .child(text(&e.volume_line(), 11.0, INK_FAINT))
                    .child(text(&format!("已读 {pct}%"), 11.0, CINNABAR)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.0))
                    .child(
                        shelf_button("翻开", true).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.shelf_expand = None;
                                this.open_book(idx);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        shelf_button("收起", false).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.shelf_expand = None;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }));

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100A66))
            .child(card)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.shelf_expand = None;
                    cx.notify();
                }),
            )
    }

    /// 书库键盘处理：重命名对话框开着时，键盘全部路由给重命名框。
    pub(crate) fn handle_shelf_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        if std::env::var("JIANDU_KEY_DEBUG").is_ok() {
            eprintln!(
                "[shelf-key] key={:?} char={:?} cmd={} ctrl={} alt={} rename={:?} draft_len={}",
                k,
                event.keystroke.key_char,
                mods.platform,
                mods.control,
                mods.alt,
                self.rename_idx.is_some(),
                self.rename_draft.chars().count()
            );
        }

        // ① 重命名对话框：Esc 取消 / ⏎ 保存 / 退格 / ← → 移光标 /
        //    Home End 首尾 / ⌘V 粘贴。
        //    可打印字符**不在此插入**——中文与英文都经 InputHandler
        //    回落（insertText / setMarkedText），keydown 直插会与回落
        //    叠字（见 ime.rs 架构说明）。
        if self.rename_idx.is_some() {
            match k {
                "escape" | "Escape" | "esc" => self.cancel_rename(),
                "enter" | "Enter" => self.commit_rename(),
                // 组字期间退格由输入法消化（删的是未上屏的拼音串）。
                "backspace" | "Backspace" => {
                    if self.rename_marked.is_none() {
                        crate::app::draft_backspace(&mut self.rename_draft, &mut self.rename_cursor)
                    }
                }
                "left" | "Left" | "ArrowLeft" => {
                    crate::app::draft_move(&self.rename_draft, &mut self.rename_cursor, true)
                }
                "right" | "Right" | "ArrowRight" => {
                    crate::app::draft_move(&self.rename_draft, &mut self.rename_cursor, false)
                }
                "home" | "Home" => self.rename_cursor = 0,
                "end" | "End" => self.rename_cursor = self.rename_draft.chars().count(),
                "v" | "V" if mods.platform || mods.control => {
                    // 原生剪贴板（NSPasteboard），不再 fork `pbpaste`。
                    if let Some(s) = crate::clipboard::paste_text(cx) {
                        crate::app::draft_insert_str(
                            &mut self.rename_draft,
                            &mut self.rename_cursor,
                            &s,
                        );
                    }
                }
                // 可打印键交给输入法回落，不直插。
                _ => {}
            }
            cx.notify();
            return;
        }

        // ①-b 书架搜索框：Esc 收起并清空 / ⏎ 收起但保留筛选 /
        //      退格 · ← → · Home End · ⌘V。
        //      可打印字符不在此插入——经 InputHandler 回落，keydown
        //      直插会与回落叠字（见 ime.rs 架构说明）。
        if self.shelf_searching {
            match k {
                "escape" | "Escape" | "esc" => {
                    self.shelf_searching = false;
                    self.shelf_query.clear();
                    self.shelf_query_cursor = 0;
                }
                "enter" | "Enter" => self.shelf_searching = false,
                "backspace" | "Backspace" => {
                    if self.shelf_query_marked.is_none() {
                        crate::app::draft_backspace(&mut self.shelf_query, &mut self.shelf_query_cursor)
                    }
                }
                "left" | "Left" | "ArrowLeft" => {
                    crate::app::draft_move(&self.shelf_query, &mut self.shelf_query_cursor, true)
                }
                "right" | "Right" | "ArrowRight" => {
                    crate::app::draft_move(&self.shelf_query, &mut self.shelf_query_cursor, false)
                }
                "home" | "Home" => self.shelf_query_cursor = 0,
                "end" | "End" => self.shelf_query_cursor = self.shelf_query.chars().count(),
                "v" | "V" if mods.platform || mods.control => {
                    // 原生剪贴板（NSPasteboard），不再 fork `pbpaste`。
                    if let Some(s) = crate::clipboard::paste_text(cx) {
                        if self.shelf_query.chars().count() < crate::ime::SEARCH_MAX {
                            crate::app::draft_insert_str(
                                &mut self.shelf_query,
                                &mut self.shelf_query_cursor,
                                &s,
                            );
                        }
                    }
                }
                // 可打印键交给输入法回落，不直插。
                _ => {}
            }
            cx.notify();
            return;
        }

        // ② Esc：收起浮层 / 清掉展开态 / 取消删除确认。
        if k == "escape" || k == "Escape" || k == "esc" {
            self.delete_confirm = None;
            self.shelf_expand = None;
            self.shelf_recat = None;
            cx.notify();
        }
    }

    /// 重命名对话框：书名输入框（草稿 + 光标），⏎ 保存 / Esc 取消。
    fn render_rename_dialog(&mut self, cx: &mut Context<Self>) -> Div {
        let idx = self.rename_idx.unwrap_or(0);
        let old = self
            .library
            .books
            .get(idx)
            .map(|b| b.title.clone())
            .unwrap_or_default();
        let draft = self.rename_draft.clone();
        let cursor = self.rename_cursor;

        // 输入框：草稿折行显示，光标处插一道朱色光标块。
        let mut box_body = div().flex().flex_col().gap_1().min_h(px(48.0));
        if draft.is_empty() {
            box_body = box_body.child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(0xB0A896))
                    .child(SharedString::from("输入新书名…")),
            );
        }
        for line in crate::reader::wrap_lines(&draft, 14) {
            let mut row = div().flex().flex_row().items_center().h(px(21.0));
            let head: String = line.chars().take(cursor).collect();
            let tail: String = line.chars().skip(cursor).collect();
            if !head.is_empty() {
                row = row.child(
                    div()
                        .text_size(px(14.0))
                        .text_color(rgb(INK))
                        .child(SharedString::from(head)),
                );
            }
            row = row.child(div().w(px(1.5)).h(px(15.0)).mx(px(0.5)).bg(rgb(CINNABAR)));
            if !tail.is_empty() {
                row = row.child(
                    div()
                        .text_size(px(14.0))
                        .text_color(rgb(INK))
                        .child(SharedString::from(tail)),
                );
            }
            box_body = box_body.child(row);
        }

        let save = div()
            .px_4()
            .py_1p5()
            .rounded(px(8.0))
            .bg(rgb(CINNABAR))
            .text_size(px(12.0))
            .text_color(rgb(PAPER))
            .cursor_pointer()
            .child(SharedString::from("保存"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.commit_rename();
                    cx.notify();
                }),
            );
        let cancel = small_button("取消").on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.cancel_rename();
                cx.notify();
            }),
        );

        let card = div()
            .w(px(380.0))
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .bg(rgb(0xFEFDFC))
            .border_1()
            .border_color(rgb(FRAME))
            .rounded(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.0))
                            .text_color(rgb(CINNABAR))
                            .child(SharedString::from("重命名")),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(0x8A8175))
                            .child(SharedString::from("最多 12 字")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(0x8A8175))
                            .child(SharedString::from(format!(
                                "当前书名：{}",
                                clip_title_w(&old, 300.0, 10.5)
                            ))),
                    )
                    .child(
                        // 输入框外套一层「可聚焦 + 可接 IME」的元素：
                        // 中文输入法不走 keydown，全靠它把组好的字送回草稿。
                        crate::ime::InputField::new(
                            div()
                                .w_full()
                                .p_2()
                                .bg(rgb(0xFFFFFF))
                                .border_2()
                                .border_color(rgb(CINNABAR))
                                .rounded(px(4.0))
                                .child(box_body),
                            self.rename_focus.clone(),
                            cx.entity(),
                        ),
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
                            .text_size(px(10.5))
                            .text_color(rgb(0x8A8175))
                            .child(SharedString::from(
                                "⏎ 保存 · Esc 取消 · ⌘V 粘贴 · 可直接打中文",
                            )),
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
            // 对话框内任何点击都不外传（不会点到书架上的书）。
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }));

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100A99))
            .child(card)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    // 点对话框外＝取消重命名。
                    this.cancel_rename();
                    cx.notify();
                }),
            )
    }

    /// 管理动作：归类 / 改名 / 删除；点过「删除」后变为二次确认态。
    /// 极小**楷体**（`FontRole::Note`），默认不露面——见 `render_manage_bar`。
    /// `ink` 是常态字色、`warn` 是「确认删除」的朱色——书衣上的管理栏压在
    /// 深色半透明底上，得用浅字；书脊列表行在纸上，得用深字。
    fn manage_row(&mut self, idx: usize, cx: &mut Context<Self>, ink: u32, warn: u32) -> Div {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0));
        if self.delete_confirm == Some(idx) {
            row = row
                .child(link_button("确认删除", warn).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_book(idx);
                        cx.notify();
                    }),
                ))
                .child(link_button("点错了", ink).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_confirm = None;
                        cx.notify();
                    }),
                ));
        } else {
            row = row
                .child(link_button("归类", ink).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_recat = Some(idx);
                        this.delete_confirm = None;
                        cx.notify();
                    }),
                ))
                .child(link_button("改名", ink).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.start_rename(idx);
                        cx.notify();
                    }),
                ))
                .child(link_button("删除", ink).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_confirm = Some(idx);
                        cx.notify();
                    }),
                ));
        }
        row
    }

    /// 悬停时**浮现在书衣底部**的半透明管理栏：内嵌 `manage_row`，
    /// 底是半透明的近黑（宣纸上的书衣本来就是暗的，压一层影子即可），
    /// 淡入动画由调用方的 `with_animation` 给。
    fn render_manage_bar(&mut self, idx: usize, w: f32, cx: &mut Context<Self>) -> Div {
        let row = self.manage_row(idx, cx, 0xD8CFC0, 0xE0867A);
        div()
            .absolute()
            .left(px(4.0))
            .bottom(px(4.0))
            .w(px(w - 8.0))
            .h(px(26.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(2.0))
            .bg(rgba(0x14100ACC))
            .child(row)
            // 栏内点空白不该顺下去把书翻开。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|_, _: &MouseMoveEvent, _, cx| {
                cx.stop_propagation();
            }))
    }

    /// 底栏（v1.49.3）：那条长长的新手提示**删了**——只在鼠标悬停到某本书
    /// 上时才浮出一句；其余时间底栏只是一道留白 + 通知（notice）。
    fn render_shelf_status(&self) -> Div {
        let hint = if self.shelf_manage {
            "点书衣勾选 · 右键任一本直接勾选 · 「⋯」里可退出整理"
        } else {
            "悬停书衣可归类 / 改名 / 删除 · 右键任一本进整理"
        };
        let show_hint = self.shelf_hover.is_some() || self.shelf_manage;
        div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(26.0))
            .py(px(8.0))
            .bg(rgb(SHELF_PAPER))
            .border_t_1()
            .border_color(rgb(HAIR))
            .text_size(px(11.0))
            .text_color(rgb(INK_FAINT))
            .child(SharedString::from(if show_hint {
                hint.to_string()
            } else {
                String::new()
            }))
            .child(SharedString::from(if self.notice.is_empty() {
                String::new()
            } else {
                self.notice.clone()
            }))
    }

    /// 封面页：竖排书名、副题、朱印、翻开按钮。
    pub fn render_cover(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(book) = &self.book else {
            return self.render_shelf(cx);
        };
        // 封面文字：书名 / 副题 / 作者的【】都显作「」（与阅读版面一致）。
        let mapq = |s: &str| -> String {
            s.chars().map(crate::types::bracket_to_quote).collect()
        };
        let raw = if book.cover.title.is_empty() {
            &book.title
        } else {
            &book.cover.title
        };
        let title = mapq(raw);
        let subtitle = mapq(&book.cover.subtitle);
        let seal = book.cover.seal_or_default();
        let author = mapq(&book.author);
        let accent = self
            .library
            .books
            .get(self.current)
            .map(|b| b.accent)
            .unwrap_or(CINNABAR);
        let chapters = book.chapters.len();
        let chars = book.total_chars();

        let cover = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .relative()
            .w(px(360.0))
            .h(px(500.0))
            .p_6()
            .bg(rgb(0x2F4858)) // 书衣：瓷青布面
            .border_1()
            .border_color(rgb(0x1E2E3A))
            .rounded(px(3.0))
            // 题签：左上角竖条，写书名（线装书衣的招牌元素）。
            // 顶端一道朱丝短线代替藏印，纸签感更足，也不抢中部大书名。
            .child(
                div()
                    .absolute()
                    .top(px(30.0))
                    .left(px(30.0))
                    .w(px(56.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .pt(px(8.0))
                    .pb(px(9.0))
                    .px(px(5.0))
                    .gap(px(6.0))
                    .bg(rgb(0xF3ECD9))
                    .border_1()
                    .border_color(rgb(0x8A6A52))
                    .rounded(px(1.0))
                    .child(div().w(px(20.0)).h(px(2.0)).bg(rgb(0xB4453C)))
                    .child({
                        let (fs_t, tcol, _) = fit_single_column(&title, 196.0, 23.0);
                        vertical_cols(&[tcol], fs_t, 0x2A241E, FontWeight::MEDIUM)
                    }),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0xC9BBA6))
                    .child(SharedString::from(format!(
                        "{} · 笺读藏本",
                        clip_title_w(&title, 190.0, 11.0)
                    ))),
            )
            // 竖排大书名：书衣题名永远一列，字号随书名长度自适应收缩，
            // 长名自动变小而不折列，保持古籍书衣版式且不溢出封面。
            .child({
                let (fs_b, bcol, _) = fit_single_column(&title, 318.0, 52.0);
                vertical_cols(&[bcol], fs_b, 0xEDE3CC, FontWeight::MEDIUM)
            })
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0xC9BBA6))
                    .child(SharedString::from(clip_title_w(&subtitle, 300.0, 11.0))),
            )
            .child(
                div()
                    .w(px(44.0))
                    .h(px(44.0))
                    .bg(rgb(accent))
                    .rounded(px(3.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(15.0))
                            .text_color(rgb(PAPER))
                            .child(SharedString::from(seal)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(0xC9BBA6))
                            .child(SharedString::from(clip_title_w(
                                &format!(
                                    "{} · {chapters} 章 · 约 {chars} 字",
                                    if author.is_empty() {
                                        "未署名".to_string()
                                    } else {
                                        author.clone()
                                    }
                                ),
                                300.0,
                                11.0,
                            ))),
                    )
            )
            // 点书衣即进正文：原先这里是一枚「翻开书页 ▸」小按钮，
            // 现在点书架的书已直达上次读的位置，那行小字纯属多余；
            // 改成整张书衣可点，既去掉小字，也不会出现「进了封面却无处可点」。
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.screen = Screen::Reading;
                    this.persist();
                    cx.notify();
                }),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xF4F4F6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_3()
                    .bg(rgb(0xFEFDFC))
                    .border_b_1()
                    .border_color(rgb(0xE6E1D6))
                    .child(
                        small_button("‹ 书架").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.back_to_shelf();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(self.render_brand(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                     .justify_center()
                    .child(cover),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 书封盒子：136×148（卡片 160×224 去掉边距、天头地脚后的中部区域）。
    const CARD_BOX: (f32, f32) = (136.0, 148.0);

    #[test]
    fn vertical_title_fits_card_box() {
        // 短名单列、不缩字号。
        let (fs, cols) = fit_vertical("道德经", CARD_BOX.0, CARD_BOX.1, 20.0);
        assert_eq!(fs, 20.0);
        assert_eq!(cols, vec!["道德经".to_string()]);

        // 长名自动分列：每列高度不超盒高，总宽不超盒宽。
        let (fs, cols) = fit_vertical("西游记（图文珍藏版）", CARD_BOX.0, CARD_BOX.1, 20.0);
        let per_col = ((CARD_BOX.1 - 4.0) / row_pitch(fs)).floor() as usize;
        assert!(cols.len() >= 2, "长名应分列，实际 {:?}", cols);
        assert!(cols.iter().all(|c| c.chars().count() <= per_col));
        assert!(cols.len() as f32 * (fs + 6.0) <= CARD_BOX.0 + 0.01);

        // 超长名：缩到最小字号仍装不下时截断，且结果依然放得进盒子。
        let long: String = "长".repeat(100);
        let (fs, cols) = fit_vertical(&long, CARD_BOX.0, CARD_BOX.1, 20.0);
        let per_col = ((CARD_BOX.1 - 4.0) / row_pitch(fs)).floor() as usize;
        let max_cols = (CARD_BOX.0 / (fs + 6.0)).floor() as usize;
        let total: usize = cols.iter().map(|c| c.chars().count()).sum();
        assert!(total < long.chars().count(), "超长名应被截断");
        assert!(total <= per_col * max_cols);
        assert!(cols.iter().all(|c| c.chars().count() <= per_col));
        assert!(cols.len() as f32 * (fs + 6.0) <= CARD_BOX.0 + 0.01);
        assert!(cols.last().map(|c| c.ends_with('…')).unwrap_or(false));
    }

    #[test]
    fn clip_title_w_respects_width() {
        // 短名原样返回。
        assert_eq!(clip_title_w("道德经", 158.0, 12.0), "道德经");
        // 长名按字宽截断并缀省略号：中文 12px/字，(158-12)/12 = 12 字 + 省略号。
        let clipped = clip_title_w("一二三四五六七八九十十一十二十三", 158.0, 12.0);
        assert_eq!(clipped.chars().count(), 13);
        assert!(clipped.ends_with('…'));
        // 拉丁字符按半宽估算，同宽度下能排更多字符。
        let latin = clip_title_w("The Great Gatsby", 158.0, 12.0);
        assert_eq!(latin, "The Great Gatsby");
    }

    /// 题签盒：68×132，最多两列。
    const SLIP: (f32, f32) = (68.0, SLIP_TITLE_H);

    #[test]
    fn slip_title_never_exceeds_two_columns() {
        // 短名：一列，不缩字号。
        let (fs, cols) = fit_cols("道德经", SLIP.0, SLIP.1, 17.0, 2);
        assert_eq!(fs, 17.0);
        assert_eq!(cols, vec!["道德经".to_string()]);

        // 十个字的长名：仍限两列。
        let (fs, cols) = fit_cols("西游记图文珍藏版全", SLIP.0, SLIP.1, 17.0, 2);
        let per_col = ((SLIP.1 - 4.0) / row_pitch(fs)).floor() as usize;
        assert!(cols.len() <= 2, "题签最多两列，实际 {:?}", cols);
        assert!(cols.iter().all(|c| c.chars().count() <= per_col));
        assert!(cols.iter().all(|c| !c.ends_with('…')), "十个字应放得下");

        // 超长名：截断缀省略号，列数依旧不超两列。
        let long: String = "长".repeat(80);
        let (fs, cols) = fit_cols(&long, SLIP.0, SLIP.1, 17.0, 2);
        assert!(cols.len() <= 2);
        assert!(cols.last().map(|c| c.ends_with('…')).unwrap_or(false));
        let total: usize = cols.iter().map(|c| c.chars().count()).sum();
        let per_col = ((SLIP.1 - 4.0) / row_pitch(fs)).floor() as usize;
        assert!(total <= per_col * 2);
    }

    #[test]
    fn fit_cols_keeps_rightmost_column_first_character_order() {
        // 分列后按「先填满第一列」切分：首列是开头几个字。
        let (_, cols) = fit_cols("一二三四五六七八九十", SLIP.0, SLIP.1, 17.0, 2);
        assert_eq!(cols.concat(), "一二三四五六七八九十");
    }

    /// 一排字的宽度（与 `char_w` 同口径）。
    fn line_w(s: &str, fs: f32) -> f32 {
        s.chars().map(|c| char_w(c) * fs).sum()
    }

    #[test]
    fn latin_titles_are_not_forced_into_a_vertical_column() {
        // 纯西文：按西文排（横排，绝不竖排拉长）。
        assert!(is_latin_title("Poetry"));
        assert!(is_latin_title("Selected Poems"));
        assert!(is_latin_title("The Great Gatsby"));
        // 纯中文：照旧竖排。
        assert!(!is_latin_title("道德经"));
        assert!(!is_latin_title("王右丞集"));
        // 空串不当西文（走竖排，渲染出来是空的，不会误判成横排空行）。
        assert!(!is_latin_title(""));
        // 拉丁不过半仍按中文竖排（四个汉字夹两个字母）。
        assert!(!is_latin_title("王右丞集 v2"));
    }

    #[test]
    fn latin_block_wraps_and_stays_inside_the_slip() {
        // 题签内宽 46（SLIP_W 58 减左右内边距与边框），可用高 132。
        let (fs, lines, cut) = fit_latin_block("Selected Poems of Wang Wei", 46.0, 132.0, 13.0);
        assert!(fs < 13.0, "「Selected」一行放不下，应缩字号：{fs}");
        assert!(lines.len() >= 2, "该折行，实际 {lines:?}");
        assert!(!cut, "这个长度应排得下：{lines:?}");
        for ln in &lines {
            assert!(line_w(ln, fs) <= 46.0 + 0.01, "行宽溢出：{ln:?} @ {fs}");
        }
        assert!(
            lines.len() as f32 * fs * 1.34 <= 132.0 + 0.01,
            "折行后总高溢出：{lines:?} @ {fs}"
        );
    }

    #[test]
    fn long_latin_title_shrinks_instead_of_overflowing() {
        let long = "The Complete Anthology of Classical Chinese Poetry and Prose";
        let (fs, lines, _) = fit_latin_block(long, 46.0, 132.0, 13.0);
        assert!(fs <= 13.0, "长名只能缩字号：{fs}");
        for ln in &lines {
            assert!(line_w(ln, fs) <= 46.0 + 0.01, "行宽溢出：{ln:?} @ {fs}");
        }
        assert!(
            lines.len() as f32 * fs * 1.34 <= 132.0 + 0.01,
            "总高溢出：{lines:?} @ {fs}"
        );
    }

    #[test]
    fn absurd_latin_title_ellipsizes_at_the_floor_size() {
        // 长到字号缩到底仍排不完：截断并缀省略号。
        let absurd: String = std::iter::repeat_n("Anthology ", 40).collect();
        let (fs, lines, cut) = fit_latin_block(&absurd, 46.0, 132.0, 13.0);
        assert_eq!(fs, 8.0, "应缩到最小字号：{fs}");
        assert!(cut, "这么长必然排不下");
        assert!(lines.last().map(|l| l.ends_with('…')).unwrap_or(false));
        assert!(lines.len() as f32 * fs * 1.34 <= 132.0 + 0.01);
    }

    #[test]
    fn long_cjk_title_shrinks_font_size_instead_of_overflowing() {
        // 长中文书名：缩字号，绝不放任溢出题签。
        let (fs, col, _) = fit_single_column("一二三四五六七八九十十一十二十三", SLIP_TITLE_H, 17.0);
        assert!(fs < 17.0, "应缩字号：{fs}");
        let need = col.chars().count() as f32 * (fs + 3.0);
        assert!(need <= SLIP_TITLE_H, "竖排总高溢出：{need} > {SLIP_TITLE_H}");
    }

    #[test]
    fn book_shadow_falls_to_the_bottom_right() {
        let s = book_shadow();
        assert_eq!(s.len(), 1, "只一道投影，多了就是现代 UI 的大团阴影");
        let sh = &s[0];
        // 右下角：两个分量都为正。
        assert!(sh.offset.x > px(0.0), "投影应偏右");
        assert!(sh.offset.y > px(0.0), "投影应偏下");
        // 微小：偏移不超 5、模糊不超 12、扩散为负（收在书衣里面）。
        assert!(sh.offset.x <= px(5.0) && sh.offset.y <= px(5.0));
        assert!(sh.blur_radius <= px(12.0));
        assert!(sh.spread_radius < px(0.0), "spread 取负才收得住");
    }

    #[test]
    fn wrap_latin_breaks_between_words_not_inside_them() {
        // 盒宽 60 容得下整个 "Selected"（57.2），故不该把词切断。
        let lines = wrap_latin("Selected Poems", 60.0, 13.0);
        assert_eq!(lines.join(" "), "Selected Poems");
        assert_eq!(lines.len(), 2, "应折成两行：{lines:?}");
        for ln in &lines {
            for tok in ln.split_whitespace() {
                assert!(tok == "Selected" || tok == "Poems", "词被切断了：{lines:?}");
            }
        }
        // 汉字之间不补空格。
        let cjk = wrap_latin("王右丞集", 60.0, 13.0);
        assert_eq!(cjk.concat(), "王右丞集");
        assert!(!cjk[0].contains(' '));
        // 单词比盒还宽时按字硬断，且断出来的每行都不超盒宽。
        let narrow = wrap_latin("Anthology", 20.0, 8.0);
        assert!(narrow.len() >= 2, "窄盒应硬断：{narrow:?}");
        for ln in &narrow {
            assert!(line_w(ln, 8.0) <= 20.0 + 0.01, "硬断后仍溢出：{ln:?}");
        }
        assert_eq!(narrow.concat(), "Anthology");
    }

    #[test]
    fn caret_window_follows_cursor_and_reports_ellipsis() {
        // 短串：整段显示，不省略。
        let (skip, head, tail) = caret_window("论语", 1, 14);
        assert!(!skip);
        assert_eq!((head.as_str(), tail.as_str()), ("论", "语"));
        // 长串且光标在末尾：窗口贴着尾部，前面省略。
        let long: String = "一".repeat(40);
        let (skip, head, tail) = caret_window(&long, 40, 14);
        assert!(skip);
        assert!(tail.is_empty());
        assert_eq!(head.chars().count(), 14);
        // 光标越界不 panic。
        let (_, _, _) = caret_window("短", 99, 14);
    }
}
