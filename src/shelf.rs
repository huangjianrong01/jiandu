//! 书架视图：题签网格 / 书脊列表 + 封面页。
//!
//! 默认「题签网格」：一格一签，竖排书名最多两列，下面四行小字
//! （作者/注者、版本或朝代、当前卷册、阅读进度）。可切「书脊列表」。
//! 顶栏有继续阅读 / 搜索 / 排序 / 视图切换，下面一排四部 + 自定页签。
//! 视觉：宣纸底、墨色字、朱印点缀、细线留白——不做木纹、不加纹理与动画。

use crate::app::{
    Root, Screen, ShelfSort, ShelfView, CINNABAR, FRAME, INK, PAPER,
};
use crate::library::{ellipsis, ShelfEntry};
use gpui::{
    div, prelude::*, px, rgb, rgba, Context, Div, FontWeight, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled,
};

// ---- 书架配色（宣纸 / 墨 / 朱 / 细线）----
/// 桌面（托纸的底色，比纸面深半档）。
const SHELF_BG: u32 = 0xF1EADA;
/// 顶栏与页签条底色。
const SHELF_PAPER: u32 = 0xFBF7EC;
/// 题签纸：比桌面浅，像贴在书衣上的小签。
const SLIP_PAPER: u32 = 0xFDFAF2;
/// 细线（界栏 / 分隔）。
const HAIR: u32 = 0xE0D5BE;
/// 次级墨（作者行）。
const INK_SOFT: u32 = 0x6B6357;
/// 三级墨（版本 / 卷册等旁注小字）。
const INK_FAINT: u32 = 0x9A9184;

/// 题签里书名竖排可用的盒（宽 × 高）：宽只容两列。
const SLIP_TITLE_W: f32 = 68.0;
const SLIP_TITLE_H: f32 = 132.0;

/// 一枚小按钮：`primary` 为朱底白字，否则纸底细框。
fn shelf_button(label: &str, primary: bool) -> Div {
    div()
        .px(px(11.0))
        .py(px(5.0))
        .rounded(px(3.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgb(if primary { PAPER } else { INK_SOFT }))
        .bg(rgb(if primary { CINNABAR } else { SLIP_PAPER }))
        .border_1()
        .border_color(rgb(if primary { CINNABAR } else { HAIR }))
        .child(SharedString::from(label.to_string()))
}

/// 一枚「文字链」式的管理按钮（很轻，不抢题签的视线）。
fn link_button(label: &str, color: u32) -> Div {
    div()
        .px(px(4.0))
        .py(px(2.0))
        .rounded(px(2.0))
        .cursor_pointer()
        .text_size(px(10.0))
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

/// 进度条 + 百分比：细线留白，只有一道朱/册色。
fn progress_row(prog: f32, pct: i32, accent: u32) -> Div {
    div()
        .mt(px(8.0))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .w_full()
                .h(px(2.0))
                .bg(rgb(HAIR))
                .child(div().h(px(2.0)).w(px(136.0 * prog.clamp(0.0, 1.0))).bg(rgb(accent))),
        )
        .child(text(&format!("{pct}%"), 9.5, INK_FAINT))
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
        let counts = self.shelf_counts();

        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .bg(rgb(SHELF_BG));

        root = root.child(self.render_shelf_top(cx));
        root = root.child(self.render_shelf_tabs(&counts, cx));

        let body = if entries.is_empty() {
            self.render_shelf_empty(cx)
        } else {
            match self.shelf_view {
                ShelfView::TitleSlip => self.render_slip_grid(&entries, cx),
                ShelfView::Spine => self.render_spine_list(&entries, cx),
            }
        };
        root = root.child(div().flex_1().overflow_hidden().child(body));
        root = root.child(self.render_shelf_status());

        // 浮层：书库 / 重命名 / 改归类 / 展开书名。
        if self.show_library {
            root = root.child(self.render_library_panel(cx));
        }
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

    /// 各页签的本数：全部 / 经 / 史 / 子 / 集 / 自定。
    fn shelf_counts(&self) -> Vec<(&'static str, usize)> {
        let mut v: Vec<(&'static str, usize)> = vec![
            ("全部", self.library.books.len()),
            ("经", 0),
            ("史", 0),
            ("子", 0),
            ("集", 0),
            ("自定", 0),
        ];
        for e in &self.library.books {
            let k = e.cat_key();
            if let Some(slot) = v.iter_mut().find(|(name, _)| *name == k) {
                slot.1 += 1;
            }
        }
        v
    }

    /// 顶栏：品牌 · 继续阅读 · 搜索 · 排序 · 视图 · 导入 · 书库。
    fn render_shelf_top(&mut self, cx: &mut Context<Self>) -> Div {
        let sort = self.shelf_sort;
        let view = self.shelf_view;
        let has_any = self
            .library
            .books
            .iter()
            .any(|e| e.last_read > 0 || e.last_page > 0);

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .px(px(18.0))
            .py(px(9.0))
            .bg(rgb(SHELF_PAPER))
            .border_b_1()
            .border_color(rgb(HAIR))
            .child(self.render_brand())
            .child(div().flex_1());

        if has_any {
            bar = bar.child(
                shelf_button("继续阅读", true).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_continue();
                        cx.notify();
                    }),
                ),
            );
        }

        bar = bar
            .child(self.render_shelf_search(cx))
            .child(
                shelf_button(&format!("排序 · {}", sort.label()), false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_sort = this.shelf_sort.next();
                        cx.notify();
                    }),
                ),
            )
            .child(
                shelf_button(&format!("视图 · {}", view.label()), false).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_view = this.shelf_view.toggled();
                        this.shelf_expand = None;
                        cx.notify();
                    }),
                ),
            )
            .child(shelf_button("导入", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.import_dialog(cx)
                }),
            ))
            .child(shelf_button("书库", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.show_library = !this.show_library;
                    cx.notify();
                }),
            ));
        bar
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
        let mut boxx = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(4.0))
            .w(px(178.0))
            .h(px(27.0))
            .px(px(8.0))
            .bg(rgb(0xFFFFFF))
            .border_1()
            .border_color(rgb(if active { CINNABAR } else { HAIR }))
            .rounded(px(3.0))
            .cursor_pointer()
            .child(body)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.shelf_searching = true;
                    cx.notify();
                }),
            );

        if has_text {
            boxx = boxx.child(
                div()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .bg(rgb(HAIR))
                    .text_size(px(9.0))
                    .text_color(rgb(INK_SOFT))
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
                    ),
            );
        }
        boxx
    }

    /// 分类页签：全部 / 经 / 史 / 子 / 集 / 自定（带本数）。
    fn render_shelf_tabs(
        &mut self,
        counts: &[(&'static str, usize)],
        cx: &mut Context<Self>,
    ) -> Div {
        let cur = self.shelf_cat.clone();
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(22.0))
            .px(px(18.0))
            .pt(px(8.0))
            .bg(rgb(SHELF_PAPER))
            .border_b_1()
            .border_color(rgb(HAIR));

        for (name, n) in counts.iter() {
            let name = *name;
            let selected = match (&cur, name) {
                (None, "全部") => true,
                (Some(c), _) => c == name,
                _ => false,
            };
            let tab = div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .pb(px(6.0))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_end()
                        .gap(px(4.0))
                        .child(text(name, 13.0, if selected { CINNABAR } else { INK_SOFT }))
                        .child(text(&n.to_string(), 9.5, INK_FAINT)),
                )
                .child(div().w_full().h(px(2.0)).bg(rgb(if selected {
                    CINNABAR
                } else {
                    SHELF_PAPER
                })))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_cat = if name == "全部" {
                            None
                        } else {
                            Some(name.to_string())
                        };
                        cx.notify();
                    }),
                );
            row = row.child(tab);
        }
        row
    }

    /// 题签网格：一格一签。
    fn render_slip_grid(
        &mut self,
        entries: &[(usize, ShelfEntry)],
        cx: &mut Context<Self>,
    ) -> Div {
        let mut grid = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_start()
            .gap(px(18.0))
            .p(px(18.0));
        for (idx, e) in entries {
            grid = grid.child(self.render_slip_card(*idx, e, cx));
        }
        div().size_full().overflow_hidden().child(grid)
    }

    /// 一枚题签：竖排书名（最多两列）+ 作者/注者、版本/朝代、当前卷册、进度。
    fn render_slip_card(&mut self, idx: usize, e: &ShelfEntry, cx: &mut Context<Self>) -> Div {
        // 书衣题签：竖排单列，字号随书名长度自适应（不再折成多列）。
        let (fs, title_col, truncated) = fit_single_column(&e.title, SLIP_TITLE_H, 17.0);
        let prog = e.progress();
        let pct = (prog * 100.0).round() as i32;
        let accent = e.accent;
        let author = e.author_line();
        let edition = e.edition_line();
        let volume = e.volume_line();

        let mut title_area = div()
            .w_full()
            .h(px(SLIP_TITLE_H))
            .flex()
            .justify_center()
            .items_start()
            .child(vertical_cols(&[title_col], fs, INK, FontWeight::MEDIUM));
        if truncated {
            // 书名放不下：点书名展开全名（点别处仍是打开书）。
            title_area = title_area.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.shelf_expand = Some(idx);
                    cx.notify();
                }),
            );
        }

        let card = div()
            .w(px(156.0))
            .h(px(244.0))
            .p(px(10.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(SLIP_PAPER))
            .border_1()
            .border_color(rgb(HAIR))
            .rounded(px(2.0))
            .cursor_pointer()
            .child(title_area)
            .child(div().w_full().h(px(1.0)).bg(rgb(HAIR)).mt(px(8.0)).mb(px(6.0)))
            .child(text_fit(&author, 11.5, INK, 11))
            .child(text_fit(&edition, 10.5, INK_FAINT, 12))
            .child(text_fit(&volume, 10.5, INK_FAINT, 12))
            .child(progress_row(prog, pct, accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_book(idx);
                    cx.notify();
                }),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(card)
            .child(self.render_card_manage(idx, cx))
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
        div().size_full().overflow_hidden().child(col)
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

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(rgb(HAIR))
            .cursor_pointer()
            // 书脊色条：一眼可辨的册色，不做木纹也不做纹理。
            .child(div().w(px(4.0)).h(px(88.0)).bg(rgb(accent)).rounded(px(1.0)))
            .child(title_area)
            .child(info)
            .child(seat)
            .child(self.render_card_manage(idx, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_book(idx);
                    cx.notify();
                }),
            )
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
    pub(crate) fn handle_shelf_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
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
                    if let Ok(o) = std::process::Command::new("pbpaste").output() {
                        let s = String::from_utf8_lossy(&o.stdout).replace(['\n', '\r'], " ");
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
                    if let Ok(o) = std::process::Command::new("pbpaste").output() {
                        let s = String::from_utf8_lossy(&o.stdout).replace(['\n', '\r'], " ");
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
            self.show_library = false;
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

    /// 书库浮层：列出内置书库，点「下载」写入书架。
    fn render_library_panel(&mut self, cx: &mut Context<Self>) -> Div {
        let items = crate::content::catalog();
        let mut list = div().flex().flex_col().gap_3().p_4();
        for it in &items {
            let key = it.key;
            let file = format!("{key}.json");
            let owned = self
                .library
                .books
                .iter()
                .any(|b| b.path.file_name().and_then(|s| s.to_str()) == Some(file.as_str()));
            let info = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(rgb(INK))
                        .child(SharedString::from(clip_title_w(&it.title, 300.0, 14.0))),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(0x8A8175))
                        .child(SharedString::from(clip_title_w(
                            &format!("{} · {}", it.author, it.subtitle),
                            320.0,
                            10.5,
                        ))),
                );
            let action = if owned {
                div()
                    .px_3()
                    .py_1p5()
                    .rounded(px(8.0))
                    .bg(rgb(0xEFEDE7))
                    .text_size(px(12.0))
                    .text_color(rgb(0x8A8175))
                    .child(SharedString::from("已在书架"))
            } else {
                small_button("⬇ 下载").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.download_book(key);
                        cx.notify();
                    }),
                )
            };
            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .p_3()
                .bg(rgb(PAPER))
                .border_1()
                .border_color(rgb(FRAME))
                .rounded(px(4.0))
                .child(info)
                .child(action);
            list = list.child(row);
        }

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100A99))
            .child(
                div()
                    .w(px(460.0))
                    .max_h(px(520.0))
                    .flex()
                    .flex_col()
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
                            .p_4()
                            .border_b_1()
                            .border_color(rgb(0xE6E1D6))
                            .child(
                                div()
                                    .text_size(px(15.0))
                                    .text_color(rgb(CINNABAR))
                                    .child(SharedString::from("书 库")),
                            )
                            .child(small_button("关闭").on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.show_library = false;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(div().flex_1().overflow_hidden().child(list)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_library = false;
                    cx.notify();
                }),
            )
    }

    /// 卡片下方的管理行：归类 / 改名 / 删除；点过「删除」后变为二次确认态。
    /// 管理动作放在卡片**外面**，卡片里永远只有那五项。
    fn render_card_manage(&mut self, idx: usize, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .w(px(156.0));
        if self.delete_confirm == Some(idx) {
            row = row
                .child(link_button("确认删除", CINNABAR).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_book(idx);
                        cx.notify();
                    }),
                ))
                .child(link_button("点错了", INK_FAINT).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_confirm = None;
                        cx.notify();
                    }),
                ));
        } else {
            row = row
                .child(link_button("归类", INK_FAINT).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.shelf_recat = Some(idx);
                        this.delete_confirm = None;
                        cx.notify();
                    }),
                ))
                .child(link_button("改名", INK_FAINT).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.start_rename(idx);
                        cx.notify();
                    }),
                ))
                .child(link_button("删除", INK_FAINT).on_mouse_down(
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

    fn render_shelf_status(&self) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_3()
            .bg(rgb(0xFEFDFC))
            .border_t_1()
            .border_color(rgb(0xE6E1D6))
            .text_size(px(12.0))
            .text_color(rgb(0x6B6357))
            .child(SharedString::from(
                "点题签开卷 · 书名过长点书名可展开 · 归类 / 改名 / 删除在签下 · 进度自动记住",
            ))
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
                    .child(self.render_brand()),
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
    const SLIP: (f32, f32) = (SLIP_TITLE_W, SLIP_TITLE_H);

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
