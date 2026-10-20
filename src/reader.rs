//! 阅读视图：古文竖笺（八行、从右到左、朱丝栏）与现代横笺（仅横线）。
//!
//! 标注体系：
//!   * 繁体标注 —— 朱红小字，竖文标字左、横文标字上；
//!   * 通假标注 —— 靛蓝小字，位置同繁体；
//!   * 注释     —— 朱红圈号 ①②③ 标字旁，页脚注释区列出注文；
//!   * 段落分隔 —— 段首空两格（首行缩进），不再放置任何段落标记符。

use crate::app::{
    Root, Screen, ANCIENT_TITLE_COL,
};
use crate::dict;
use crate::gloss::{Page, Piece};
use crate::grid::GRID_COLUMN_CHOICES;
use crate::lines;
use crate::theme::{InkPalette, Theme};
use crate::trad;
use crate::types::{
    is_punct, underline_run_start, Book, CardLayout, Duplex, FontChoice, Glyph, GlyphHit,
    GlyphKind, MarkKind, NoteKind, Script, StyleTab, TocEntry,
};
use gpui::{
    div, prelude::*, px, rgb, rgba, svg, Context, Div, FontWeight, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Overflow, ParentElement, SharedString,
    Styled, Window,
};
use std::collections::HashMap;

/// 分享卡底色：(底色, 正文字色, 落款 / 日期色)。对应分享面板的五个色块。
pub(crate) const SHARE_BGS: [(u32, u32, u32); 5] = [
    (0x161310, 0xE9E2D2, 0xA8957A), // 墨夜
    (0x2A1512, 0xF0DFD2, 0xC49A83), // 朱漆
    (0x121A24, 0xDCE6F0, 0x8FA3B8), // 靛青
    (0x14211B, 0xDDEAE0, 0x93AE9E), // 松烟
    (0xF2EAD6, 0x2B2622, 0x8A8175), // 宣纸
];

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

/// 版心鱼尾：比目录里那枚更竖长（viewBox 24×17），贴篇题上下各一枚。
fn banxin_tail(width: f32, top: bool, color: u32) -> Div {
    let h = width * 17.0 / 24.0;
    let path = if top {
        "banxin_top.svg"
    } else {
        "banxin_bottom.svg"
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
    pub fn render_reading(&mut self, cx: &mut Context<Self>) -> Div {
        if self.book.is_none() {
            return self.render_shelf(cx);
        }
        // 按当前窗口视口把字格缩放到铺满屏。
        self.fit_sizes();
        let t = self.theme;
        // 「面 → 列 → 片」一次排好：分页与切列同源（gloss::layout_book），
        // 双行夹注的跨列流转也在这里定型，渲染侧不再重算。
        let pages = self.layout_pages();
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
            let cur = self.page; // 当前面
            let next = self.page + 1; // 下一面
            let mut row = div().flex().flex_row().gap_4().items_start();
            // 古籍对开自右向左读：当前面（cur）在右、下一面（next）在左，读序 N → N+1。
            // flex_row 首个子元素落在最左，故先排下一面（左）、后排当前面（右）；
            // 左叶用纸背色（paper_back）、右叶用正面色，拟一帧真实对开。
            if next < total {
                row = row.child(self.render_leaf(&pages, next, true, cx));
            }
            if cur < total {
                row = row.child(self.render_leaf(&pages, cur, false, cx));
            }
            row
        } else {
            div()
                .flex()
                .flex_row()
                .justify_center()
                .child(self.render_leaf(&pages, self.page, false, cx))
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
                        if this.note_editing {
                            this.commit_note_edit();
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
                        if this.touched_glyph && this.sel_char.is_some() && held >= LONG_PRESS {
                            let key = this.sel_focus.or(this.sel_anchor).unwrap_or((0, 0, 0));
                            let ch = this.sel_char.unwrap_or(' ');
                            this.bubble = Some(GlyphHit {
                                key,
                                ch,
                                x: ux,
                                y: uy,
                            });
                            this.dragging = false;
                            this.popup = None;
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
                                this.popup = Some(GlyphHit {
                                    key,
                                    ch: this.sel_char.unwrap_or(' '),
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
                            });
                            cx.notify();
                        } else if let Some(fwd) = fwd {
                            // 全程在纸面且是单击：按落点左右半屏翻页——
                            // 但「屏幕中间区域」视为呼出目录（而非翻页），
                            // 避免与左右半屏翻页抢手势。
                            let mid_lo = this.viewport_w * 0.34;
                            let mid_hi = this.viewport_w * 0.66;
                            if ux > mid_lo && ux < mid_hi {
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
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(t.chrome()))
            // 点字弹出的工具条：点正文某字才出现；点其它地方（正文空白处 / 工具栏 / 浮层）即收起，
            // 同时清除拖选选区。
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                // 笔记浮层开着时，点浮层以外的任何地方＝自动保存笔记；
                // 同时「吃掉」本次按下记录，避免松手时误翻页。
                if this.note_editing {
                    this.commit_note_edit();
                    this.press_at = None;
                }
                if this.popup.is_some() || this.has_selection() {
                    this.popup = None;
                    this.clear_selection();
                }
                // 点气泡菜单以外的任何地方：收起气泡菜单。
                this.bubble = None;
                // 点注文卡片之外：收起放大卡（卡片自身 stop_propagation，
                // 点卡片内容不会到达这里）。
                this.note_zoom = None;
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

        // 点字查字浮层。
        if self.lookup.is_some() {
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
        if self.show_search && !self.focus_mode {
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

        // 长按正文弹出的气泡菜单（复制 / 检索 / 字典）。
        if self.bubble.is_some() {
            root = root.child(self.render_bubble_menu(cx));
        }

        // 注文放大悬浮卡：点行内夹注小字弹出，点卡片外任意处收起。
        if self.note_zoom.is_some() {
            root = root.child(self.render_note_zoom(cx));
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
                    //    ⌘V 粘贴中文 / 可打印字符直接键入（英文 / 数字 / 标点）。
                    if self.note_editing {
                        match k {
                            "escape" | "Escape" | "esc" => self.cancel_note_edit(),
                            "enter" | "Enter" => self.commit_note_edit(),
                            "backspace" | "Backspace" => self.note_backspace(),
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
                            _ => {
                                if !mods.platform && !mods.control && !mods.alt {
                                    if let Some(s) = event.keystroke.key_char.as_deref() {
                                        if let Some(c) = s.chars().next() {
                                            if !c.is_control() {
                                                self.note_input_char(c);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        cx.notify();
                        return;
                    }

                    // ①′ 检索框：面板开着时键盘先给它。
                    //     中文走 IME（`InputTarget::Search`，见 ime.rs），
                    //     这里只管英文 / 数字的直接键入与几个编辑键。
                    if self.show_search {
                        match k {
                            "enter" | "Enter" => self.run_search(cx),
                            "escape" | "Escape" | "esc" => {
                                self.show_search = false;
                                self.clear_search();
                            }
                            "backspace" | "Backspace" => self.search_backspace(),
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
                            _ => {
                                if !mods.platform && !mods.control && !mods.alt {
                                    if let Some(s) = event.keystroke.key_char.as_deref() {
                                        if let Some(c) = s.chars().next() {
                                            self.search_input_char(c);
                                        }
                                    }
                                }
                            }
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
                        self.lookup = None;
                        self.clear_selection();
                        self.popup = None;
                        self.bubble = None;
                        self.note_zoom = None;
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
                                    .or_else(|| self.popup.map(|h| h.ch))
                                    .or_else(|| {
                                        self.paginate().get(self.page).and_then(|p| {
                                            p.iter().find(|g| !g.is_indent()).map(|g| g.ch)
                                        })
                                    });
                                if let Some(c) = c {
                                    self.lookup = Some(c.to_string());
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
        let title = if book.cover.title.is_empty() {
            book.title.clone()
        } else {
            book.cover.title.clone()
        };
        if title.is_empty() {
            return div();
        }
        let t = self.theme;
        div()
            .absolute()
            .top(px(10.0))
            .left(px(10.0))
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
            .child(crate::shelf::vertical_block(
                &title,
                22.0,
                176.0,
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

        // 生成照片：把当前窗口截存为 PNG（macOS 下用 screencapture）。
        bar = bar.child(small_button("生成照片", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _window, _, cx| {
                this.save_photo(cx);
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
        cx: &mut Context<Self>,
    ) -> Div {
        let page = pages.get(idx);
        let glyphs: Vec<Glyph> = page.map(|p| p.glyphs.clone()).unwrap_or_default();
        let start = page.map(|p| p.start_chapter).unwrap_or(false);
        let columns: Vec<Vec<Piece>> = page.map(|p| p.columns.clone()).unwrap_or_default();
        // 现只保留古籍竖笺（现代横笺已移除）：一律走竖排渲染。
        let inner = self.render_ancient(&glyphs, &columns, back, start, idx, cx);
        // 最小字号保护：窄屏 + 高列数（12/14）触发最小字号后，纸面可能超出
        // 视口高度——这里把该面包进一个限高（=内容区高）的滚动容器，允许
        // 单页内上下滑动查看，而不是把字压到更小或裁掉。
        let inner = if self.grid_min_font {
            let leaf_w = if self.duplex == Duplex::Facing {
                ((self.content_w - crate::grid::LEAF_GAP) / 2.0).max(80.0)
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
        if idx + 1 >= pages.len() && pages.len() > 0 {
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
                            note,
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

    fn chapter_title(&self, chapter: usize) -> String {
        self.book
            .as_ref()
            .and_then(|b| b.chapters.get(chapter))
            .map(|c| c.title.clone())
            .unwrap_or_default()
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
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
        let fs = self.size_ancient; // == 网格引擎字号（self.grid_settings 驱动）
        // 字格宽 / 高来自网格引擎：竖排列宽 = col_w，单字竖高 = row_h。
        let cell_w = self.col_w;
        let cell_h = self.row_h;
        let paper = if back { t.paper_back() } else { t.paper() };
        let chapter = glyphs.first().map(|g| g.chapter).unwrap_or(0);
        let note_index = if !self.notes_inline && self.show_notes {
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
        let rows = self.grid_rows;
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
            };

        // 界行（乌丝栏）：从右往左先排正文——`i = text_cols-1` 落在最左，
        // `i = 0` 落在最右，正是竖排的读序。每列左缘一道通高细线，
        // 空列也照画（刻本里字未排满，栏线一样到底）。
        let mut grid = div().flex().flex_row().items_start();
        for i in (0..text_cols).rev() {
            let items: &[Piece] = columns.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
            // 章首页的开卷列（i==0，紧邻版心）：顶部预留一段空白带给卷首题署，
            // 正文从带下接着排（带高 = chapter_inscription_reserve_rows）。
            let top_skip =
                if start_chapter && i == 0 { self.chapter_inscription_reserve_rows() } else { 0 };
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
                idx,
            ));
        }

        // 地脚：章首页的叶码已并入版心（章题列），此处只给正文页（无版心）
        // 居中补一枚小字叶码，免得地脚整条空着；不再另缀鱼尾（鱼尾成对
        // 只出现在版心里，地脚单独再放一枚会显得重复、位置也对不齐）。
        // 用绝对定位而非另起一行：地脚不占行格，界行才能贯通到版框底。
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
            .child(if start_chapter {
                div()
            } else {
                div()
                    .text_size(px(small))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from(han_num(idx + 1)))
            });

        // 版框：四周双边——外粗内细，中间留一线空隙（刻本双边的样子）。
        // 内层 `relative`：地脚 / 卷首题署都用绝对定位叠在行格之上。
        let mut inner = div()
            .relative()
            .border_1()
            .border_color(rgb(t.frame_ink()))
            .child(grid)
            .child(foot);
        // 卷首题署（卷端）：章首页开卷列顶部叠一卷端题名——
        // 第一行（顶格）写书名·卷次，第二行（低几格）写署名；
        // 落在 layout_book 预留的空白带上，与正文互不遮挡。
        if start_chapter && self.chapter_inscription_reserve_rows() > 0 {
            inner = inner.child(self.render_inscription(chapter, cell_w, cell_h, fs, cx));
        }
        let frame = div()
            .border_3()
            .border_color(rgb(t.frame_ink()))
            .p(px(3.0))
            .child(inner);

        let mut sheet = div()
            .flex()
            .flex_col()
            .items_center()
            .p(px(16.0))
            .bg(rgb(paper))
            .child(frame);

        // 注文不进正文时，退回页脚注。
        if !self.notes_inline {
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
            // 乌丝栏：列间界行取墨色细线，与刻本一致（朱线只留给句读 / 划线）。
            .border_l_1()
            .border_color(rgb(t.rule_ink()));
        let mut content = div().w_full().flex().flex_col().items_center();
        if top_skip > 0 {
            content = content.pt(px(cell_h * top_skip as f32));
        }
        for p in items {
            match p {
                Piece::Main(g) => {
                    let mark = self.mark_kind(*g);
                    content = content.child(self.ancient_cell(*g, cell_w, cell_h, fs, back, mark, note_index, cx))
                }
                Piece::Gloss(txt) => content = content.child(self.ancient_gloss(txt, cell_w, cell_h, fs, columns, back, cx)),
            }
        }
        col.child(content)
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
        cell_w: f32,
        cell_h: f32,
        fs: f32,
        columns: usize,
        back: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme;
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
        let note_color = self.palette.z_note(t, back);
        // 注文全体（跨列流转的一段）——点它时放大显示的就是这段。
        let note_text = text.to_string();
        let mut block = div()
            .w(px(cell_w))
            .h(px(rows as f32 * row_h))
            .flex()
            .flex_col()
            .items_center()
            .overflow_hidden()
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
                        text: note_text.clone(),
                        x: event.position.x.to_f64() as f32,
                        y: event.position.y.to_f64() as f32,
                        page: this.page,
                    });
                    cx.notify();
                }),
            );
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
                    Some(ch) => div()
                        .w(px(sub_w))
                        .h(px(row_h))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(nfs))
                        .text_color(rgb(note_color))
                        .child(SharedString::from(ch.to_string())),
                    None => div().w(px(sub_w)).h(px(row_h)),
                });
            }
            block = block.child(line);
        }
        block
    }

    /// 章题列：照刻本「版心」的样子——天头留白、上鱼尾、竖排篇题、下鱼尾，
    /// 一列通高（与八列界行同高，栏线上下都顶到版框）。
    fn ancient_title_column(
        &self,
        chapter: usize,
        cell_w: f32,
        cell_h: f32,
        rows: usize,
        foot_h: f32,
        fs: f32,
        idx: usize,
    ) -> Div {
        let t = self.theme;
        let title = self.chapter_title(chapter);
        let col_w = cell_w * ANCIENT_TITLE_COL;
        // 篇题字号与正文齐（刻本不把篇题放大，靠独占一列区分）。
        let mut col = div()
            .w(px(col_w))
            .h(px(cell_h * rows as f32 + foot_h))
            .flex()
            .flex_col()
            .items_center()
            .border_l_1()
            .border_color(rgb(t.rule_ink()));
        // 天头：留约一字高的空白，鱼尾才落在版心上部而不是贴框。
        col = col.child(div().h(px(cell_h * 1.0)));
        // 上鱼尾：尾朝下，宽度略窄于列（影印本鱼尾不顶格）。
        col = col.child(banxin_tail(col_w * 0.72, true, t.frame_ink()));
        col = col.child(div().h(px(cell_h * 0.18)));
        // 篇题：一字一格，自上面下。
        for c in title.chars().take(7) {
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
        // 叶码：小字，竖排压在下鱼尾之上（宋版版心「篇题—叶码—下鱼尾」的次序）。
        col = col.child(div().flex_1());
        let nfs = (fs * 0.42).max(9.0);
        for c in han_num(idx + 1).chars() {
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
        // 下鱼尾：尾朝上，与上鱼尾对合，夹住篇题与叶码。
        col = col.child(banxin_tail(col_w * 0.72, false, t.frame_ink()));
        col.child(div().h(px(cell_h * 0.38)))
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
        if let Some(py) = rare_py {
            cell = cell.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(cell_w))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .text_size(px((fs * 0.28).max(6.0)))
                    .text_color(rgb(self.palette.note(t)))
                    .child(SharedString::from(py)),
            );
        }
        if let Some((txt, color)) = self.annotation_of(g) {
            cell = cell.child(
                div()
                    .text_size(px((fs * 0.34).max(7.0)))
                    .text_color(rgb(if back { t.ink_soft() } else { color }))
                    .child(SharedString::from(txt)),
            );
        }
        if is_punct(g.ch) {
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
        } else {
            cell = cell.child(
                div()
                    .text_size(px(fs))
                    .text_color(rgb(if back {
                        t.ink_soft()
                    } else {
                        self.palette.ink(t)
                    }))
                    .child(SharedString::from(self.shown(g.ch).to_string())),
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
                            this.popup = Some(GlyphHit {
                                key,
                                ch: g.ch,
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

    /// 长按正文弹出的气泡菜单：复制 / 检索 / 字典。
    ///
    /// 三件事都复用既有能力，不新造一套：
    ///   * 复制 —— `copy_page_text`（有选区复制选区，无选区则临时选中该字）；
    ///   * 检索 —— `search_for` 直接把选中的词喂给全文检索并跳面高亮；
    ///   * 字典 —— 写入 `lookup`，由既有的点字查字浮层显示释义。
    fn render_bubble_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let Some(hit) = self.bubble.clone() else {
            return div();
        };
        let h1 = hit.clone();
        let h2 = hit.clone();
        let h3 = hit.clone();
        let btn_copy = small_button("复制", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                // 长按单字时没有选区：临时把这一个字设为选区再复制，
                // 免得退化成「复制整页」。
                if !this.has_selection() {
                    this.sel_anchor = Some(h1.key);
                    this.sel_focus = Some(h1.key);
                }
                this.copy_page_text();
                this.clear_selection();
                this.bubble = None;
                cx.notify();
            }),
        );
        let btn_search = small_button("检索", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                let text = if this.has_selection() {
                    this.selected_text()
                } else {
                    h2.ch.to_string()
                };
                this.search_for(&text, cx);
                this.bubble = None;
                cx.notify();
            }),
        );
        let btn_dict = small_button("字典", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.lookup = Some(h3.ch.to_string());
                this.bubble = None;
                cx.notify();
            }),
        );

        div()
            .absolute()
            .left(px(hit.x))
            .top(px(hit.y))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .p_1()
            .bg(rgb(t.chrome()))
            .border_1()
            .border_color(rgb(t.frame()))
            .rounded(px(4.0))
            .child(btn_copy)
            .child(btn_search)
            .child(btn_dict)
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

    /// 点字查字浮层：所点字用大字立在朱印旁，下附释义、繁体、通假。
    fn render_dict_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let ch = self.lookup.clone().unwrap_or_default();
        let c = ch.chars().next().unwrap_or(' ');
        let book = self.book.as_ref();
        let def = crate::dict::define(&ch);
        let trad = trad::traditional_of(c);
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
        if let Some(d) = def {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from(d)),
            );
        } else {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("（字书未收此字，换个字点点看）")),
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
                            this.lookup = None;
                            cx.notify();
                        }),
                    )),
            )
            .child(body)
    }

    /// 样式浮层：纸色、字级、版式、注文排法。
    /// 笔记 / 批注浮层：列出全书笔记，并可在当前页写笔记。
    /// 竖排工具框：查询 / 划线 / 笔记 / 复制 / 分享（参照 03 图）。
    /// 点字弹出的工具条：在点击位置附近浮起一行 查询 / 划线 / 笔记 / 复制 / 分享。
    /// 不再常驻右上角，只有点了正文某个字才出现；点别处即收起。
    /// 每个工具都作用于被点击的那个字。
    fn render_glyph_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let Some(hit) = self.popup else {
            return div();
        };
        let vw = self.viewport_w;
        let vh = self.viewport_h;

        // 工具条尺寸估算，用于把浮条夹在视口内。
        let n: f32 = 5.0;
        let bw = 46.0;
        let gap = 6.0;
        let pad = 8.0;
        let menu_w = pad * 2.0 + bw * n + gap * (n - 1.0);
        let menu_h = 38.0;

        let mut left = hit.x + 10.0;
        let mut top = hit.y + 20.0;
        if left + menu_w > vw {
            left = (vw - menu_w - 10.0).max(0.0);
        }
        if top + menu_h > vh {
            top = (hit.y - menu_h - 10.0).max(0.0);
        }

        let key = hit.key;
        let ch = hit.ch;

        let btn_query = small_button("查询", false, t).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.lookup = Some(ch.to_string());
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
        let head_h = 20.0;
        let foot_h = 16.0;
        let card_h =
            pad * 2.0 + head_h + 8.0 + lines.len() as f32 * line_h + 6.0 + foot_h;

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
                            .child(SharedString::from("夹注重录")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(edit)
                    .child(close),
            );

        let mut body = div().flex().flex_col().gap(px(2.0));
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

        let foot = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0xFFFFFF90))
                    .child(SharedString::from(format!(
                        "第 {} 葉 · 原注双行小字，此处已放大",
                        zoom.page + 1
                    ))),
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
            // 点卡片内容不冒泡到 root（否则会被当「点别处」秒关）。
            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                cx.stop_propagation();
            }))
            .child(head)
            .child(body)
            .child(foot)
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

        let (bg, ink, mute) = SHARE_BGS[self.share_bg.min(SHARE_BGS.len() - 1)];
        // 卡片高度由「高度」滑杆控制，同时给下方控制面板留出空间。
        let card_h = (vh * self.share_h).min(vh - 250.0).max(280.0);
        let card_w = (card_h * 0.66).clamp(300.0, 600.0);
        let fs = (card_h / 40.0).clamp(12.0, 24.0);
        // 单字行高：GPUI 的默认行高约 1.4×字号，再加上 gap_1(4px)。
        // 少算行高会让最后几行溢出卡片（分享卡底部被裁掉半行字）。
        let line_h = fs * 1.42 + 4.0;

        // ---- 正文：按版式排（竖排分列从右到左 / 横排分行 / 单列竖排）----
        let grid = match self.share_layout {
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
            CardLayout::Rows => {
                let per_line = ((card_w - 76.0) / (fs + 2.0)).floor().max(6.0) as usize;
                let mut g = div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_2()
                    .w(px(card_w - 150.0));
                for line in body.chunks(per_line) {
                    let s: String = line.iter().collect();
                    g = g.child(
                        div()
                            .text_size(px(fs))
                            .text_color(rgb(ink))
                            .child(SharedString::from(s)),
                    );
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
        };

        // ---- 题识列：章节 / 书名 / 作者 按开关拼接（书名仍夹 12 字）----
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
        let mut sign = div().flex().flex_col().items_center().gap_1();
        for c in sign_text.chars() {
            sign = sign.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(mute))
                    .child(SharedString::from(c.to_string())),
            );
        }

        // ---- 日期角标：竖排中文日期立于右上（可关）----
        let date_col = if self.share_show_date {
            let date_text = cn_date_from_str(&today_ymd());
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
        } else {
            div().flex_shrink_0()
        };

        let seal = div()
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
            );

        let card = div()
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
            .child(date_col);

        // ---- 高度滑杆（竖排五档，仿图 1 右侧的「高度」条）----
        let steps = [0.62f32, 0.69, 0.76, 0.84, 0.92];
        let cur_step = steps
            .iter()
            .position(|s| (s - self.share_h).abs() < 0.01)
            .unwrap_or(3);
        let mut ticks = div().flex().flex_col().items_center().gap_2();
        ticks = ticks.child(
            div()
                .text_size(px(9.5))
                .text_color(rgb(0xB9AE9C))
                .child(SharedString::from("高")),
        );
        for (i, s) in steps.iter().enumerate() {
            let on = i == cur_step;
            let v = *s;
            ticks = ticks.child(
                div()
                    .id(SharedString::from(format!("share-h-{i}")))
                    .w(px(if on { 12.0 } else { 7.0 }))
                    .h(px(if on { 12.0 } else { 7.0 }))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .bg(rgb(if on { t.accent() } else { 0x6B6152 }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.share_h = v;
                            cx.notify();
                        }),
                    ),
            );
        }
        ticks = ticks.child(
            div()
                .text_size(px(9.5))
                .text_color(rgb(0xB9AE9C))
                .child(SharedString::from("度")),
        );
        let slider = div()
            .flex()
            .flex_col()
            .items_center()
            .h(px(card_h))
            .child(div().flex_1())
            .child(ticks)
            .child(div().flex_1());

        // ---- 控制面板：元素开关 / 版式 / 底色 ----
        let mut toggles = div().flex().flex_row().items_center().gap_2();
        for (label, on, act) in [
            ("书名", self.share_show_title, ShareAct::Title),
            ("作者", self.share_show_author, ShareAct::Author),
            ("章节", self.share_show_chapter, ShareAct::Chapter),
            ("日期", self.share_show_date, ShareAct::Date),
        ] {
            toggles = toggles.child(small_button(label, on, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    match act {
                        ShareAct::Title => this.share_show_title = !this.share_show_title,
                        ShareAct::Author => this.share_show_author = !this.share_show_author,
                        ShareAct::Chapter => {
                            this.share_show_chapter = !this.share_show_chapter
                        }
                        ShareAct::Date => this.share_show_date = !this.share_show_date,
                    }
                    cx.notify();
                }),
            ));
        }

        let icon_ink = 0x2B2622;
        let mut layouts = div().flex().flex_row().items_center().gap_2();
        for (lay, label) in [
            (CardLayout::Columns, "竖排分列"),
            (CardLayout::Rows, "横排分行"),
            (CardLayout::Single, "单列竖排"),
        ] {
            let on = self.share_layout == lay;
            let fg = if on { 0xE9E2D2 } else { icon_ink };
            let icon = match lay {
                CardLayout::Columns => div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap_1()
                    .child(bar(2.0, 13.0, fg))
                    .child(bar(2.0, 13.0, fg))
                    .child(bar(2.0, 13.0, fg)),
                CardLayout::Rows => div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .child(bar(14.0, 2.0, fg))
                    .child(bar(14.0, 2.0, fg))
                    .child(bar(14.0, 2.0, fg)),
                CardLayout::Single => div()
                    .w(px(13.0))
                    .h(px(13.0))
                    .border_2()
                    .border_color(rgb(fg))
                    .rounded(px(1.0)),
            };
            layouts = layouts.child(
                div()
                    .id(SharedString::from(format!("share-lay-{label}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(40.0))
                    .h(px(26.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .bg(rgb(if on { 0x2B2622 } else { t.chip() }))
                    .border_1()
                    .border_color(rgb(if on { t.accent() } else { t.chrome_line() }))
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

        let mut swatches = div().flex().flex_row().items_center().gap_2();
        for (i, (bgc, _, _)) in SHARE_BGS.iter().enumerate() {
            let on = i == self.share_bg;
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("share-bg-{i}")))
                    .w(px(30.0))
                    .h(px(22.0))
                    .rounded(px(3.0))
                    .cursor_pointer()
                    .bg(rgb(*bgc))
                    .border_2()
                    .border_color(rgb(if on { t.accent() } else { 0x8A8175 }))
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

        let controls = div()
            .flex()
            .flex_col()
            .gap_2()
            .h(px(88.0))
            .w(px((card_w + 92.0).min(vw * 0.92)))
            .p_3()
            .rounded(px(8.0))
            .bg(rgb(0x1C1915))
            .border_1()
            .border_color(rgb(0x3A332B))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(toggles)
                    .child(layouts),
            )
            .child(
                div()
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
                                    .text_size(px(10.0))
                                    .text_color(rgb(0xB9AE9C))
                                    .child(SharedString::from("底色")),
                            )
                            .child(swatches),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(0xB9AE9C))
                            .child(SharedString::from(format!(
                                "第 {} 葉 · {}",
                                self.page + 1,
                                self.share_layout.label()
                            ))),
                    ),
            );

        // ---- 三个动作：保存图片 / 分享图片 / 分享文字（与样式面板同一排）----
        let save = self.action_button("保存图片", t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.save_share_image(window, cx);
            }),
        );
        let share_img = self.action_button("分享图片", t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.share_image_to_clipboard(window, cx);
            }),
        );
        let share_txt = self.action_button("分享文字", t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.copy_page_text();
                this.notice = "已复制本页文字".to_string();
                cx.notify();
            }),
        );
        let panel_w = (card_w + 92.0).min(vw * 0.92);
        let actions = div()
            .flex()
            .flex_row()
            .gap_2()
            .h(px(36.0))
            .w(px(panel_w))
            .child(save)
            .child(share_img)
            .child(share_txt);

        // 卡片与面板用绝对坐标定位（而不是 flex 居中）：保存 / 分享图片
        // 要按这块矩形裁图，渲染位置必须与算式严格一致，否则截出来会偏。
        // 版式：卡片 + 12 + 控制条 88 + 12 + 按钮 36，整体垂直居中。
        let total_h = card_h + 12.0 + 88.0 + 12.0 + 36.0;
        let top = ((vh - total_h) * 0.5).max(0.0);
        let row_w = card_w + 12.0 + 42.0; // 卡片 + 间距 + 高度滑杆
        let left = ((vw - row_w) * 0.5).max(0.0);
        let panel_left = (left - (panel_w - row_w) * 0.5).max(0.0);
        self.share_card_rect = Some((left, top, card_w, card_h));

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(vw))
            .h(px(vh))
            .bg(rgb(0x0E0D0B))
            // 右上角 ✕：面板是全屏覆盖且点空白不收起（防误触丢失选择），
            // 所以关闭必须有个常驻的明确出口；Esc 也有同样效果。
            .child(
                div()
                    .absolute()
                    .top(px(14.0))
                    .right(px(16.0))
                    .w(px(30.0))
                    .h(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(15.0))
                    .border_1()
                    .border_color(rgb(0x5A5248))
                    .bg(rgb(0x1A1714))
                    .cursor_pointer()
                    .child(
                        div()
                            .text_size(px(15.0))
                            .text_color(rgb(0xD8CFC0))
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
            .child(div().absolute().left(px(left)).top(px(top)).child(card))
            .child(
                div()
                    .absolute()
                    .left(px(left + card_w + 12.0))
                    .top(px(top))
                    .child(slider),
            )
            .child(
                div()
                    .absolute()
                    .left(px(panel_left))
                    .top(px(top + card_h + 12.0))
                    .child(controls),
            )
            .child(
                div()
                    .absolute()
                    .left(px(panel_left))
                    .top(px(top + card_h + 12.0 + 88.0 + 12.0))
                    .child(actions),
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

        // 「第 N 葉的笔记」区：静态展示已有笔记；写笔记统一走正文上方的
        // 浮层编辑框（直接盖在文本上输入，不占用面板、不遮正文）。
        let note_box = div()
            .w(px(328.0))
            .p_2()
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.rule()))
            .rounded(px(4.0))
            .text_size(px(12.0))
            .text_color(rgb(if existing.is_empty() { t.ink_soft() } else { t.ink() }))
            .child(SharedString::from(shown));

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
            let label: String = txt.chars().take(20).collect();
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
        let cols = self.grid.columns;
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

        // 列数行：8 / 10 / 12 / 14（引擎按列数动态算列宽 / 字号 / 行数）。
        let mut col_row = div().flex().flex_row().items_center().gap_2().flex_wrap();
        for n in GRID_COLUMN_CHOICES {
            let s = format!("{n} 列");
            let on = cols == n;
            col_row = col_row.child(small_button(s.as_str(), on, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_columns(n);
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
                        .child(SharedString::from("列数")),
                )
                .child(col_row),
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
                // 标注：通假 / 标点 / 生僻字 / 注释（「红色繁体小注」已删除，
                // 繁简改由版式行的「简体 / 繁体」统一切换正文字体）。
                let marks = self.style_chips(
                    vec![
                        ("通假", self.show_tongjia, StyleAct::Tong),
                        ("标点", self.show_punct, StyleAct::Punct),
                        ("生僻字", self.show_rare, StyleAct::Rare),
                        ("注释", self.show_notes, StyleAct::Notes),
                    ],
                    cx,
                );
                sheet = sheet.child(style_row("标注", marks, t));
            }
            StyleTab::Font => {
                // 字体：按当前模式给出可选字体。
                sheet = sheet.child(style_row("字体", self.font_choices_row(cx), t));
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

        // ---- 底部三个大按钮：保存图片 / 分享图片 / 分享文字 ----
        let save = self.action_button("保存图片", t).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.save_photo(cx);
            }),
        );
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
                .child(save)
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

    /// 保存分享卡：截卡片到桌面 PNG，并收起面板。
    fn save_share_image(&mut self, window: &Window, cx: &mut Context<Self>) {
        // 先截再收面板：截屏是同步的，先关面板会让画面在下一帧才更新，
        // 虽然大概率仍截到卡片，但顺序反了容易截到「已消失」的界面。
        let shot = self.capture_card_png(window, "jiandu-card");
        self.show_share_panel = false;
        self.notice = match shot {
            Ok(p) => format!("已保存分享卡：{p}"),
            Err(e) => e,
        };
        cx.notify();
    }

    /// 分享图片：把截下的卡片直接放进系统剪贴板，可在微信 / 备忘录里 ⌘V 粘贴。
    fn share_image_to_clipboard(&mut self, window: &Window, cx: &mut Context<Self>) {
        let shot = self.capture_card_png(window, "jiandu-share");
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
                    "分享图已复制到剪贴板，可直接粘贴".to_string()
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

    /// 保存当前页为照片（macOS 下用 screencapture 截存桌面 PNG）。
    pub fn save_photo(&mut self, cx: &mut Context<Self>) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = format!("{home}/Desktop/jiandu-{secs}.png");
        let ok = std::process::Command::new("screencapture")
            .args(["-x", &path])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        self.notice = if ok {
            format!("已生成照片：{path}")
        } else {
            "生成照片失败：需 macOS 桌面环境".to_string()
        };
        cx.notify();
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

#[cfg(test)]
mod tests {
    use super::{cn_date_from_str, cn_small};

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
}
