//! 书架视图：书封墙 + 封面页。
//!
//! 每本书按书名散列出一个稳定主色，画成宣纸书封：竖排书名、朱印、进度条。
//! 点封面进「封面页」，再点「翻开」进正文。

use crate::app::{Root, Screen, CINNABAR, FRAME, INK, PAPER};
use crate::library::ShelfEntry;
use gpui::{
    div, prelude::*, px, rgb, rgba, Context, Div, FontWeight, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled,
};

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
        let per_col = ((box_h - GAP) / (fs + GAP)).floor().max(1.0) as usize;
        let max_cols = (box_w / (fs + GAP)).floor().max(1.0) as usize;
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
    let mut row = div().flex().flex_row().items_start().gap_1();
    // 首列在最右：倒序添加，最后一列落在最左。
    for col in cols.iter().rev() {
        let mut c = div().flex().flex_col().items_center().gap_1();
        for ch in col.chars() {
            c = c.child(
                div()
                    .text_size(px(fs))
                    .font_weight(weight)
                    .text_color(rgb(color))
                    .child(SharedString::from(ch.to_string())),
            );
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

    /// 书架：书封墙。
    pub(crate) fn render_shelf(&mut self, cx: &mut Context<Self>) -> Div {
        let mut head = div()
            .flex()
            .items_center()
            .gap_3()
            .p_3()
            .bg(rgb(0xFEFDFC))
            .border_b_1()
            .border_color(rgb(0xE6E1D6))
            .child(self.render_brand())
            .child(div().flex_1())
            .child(small_button("📚 导入书籍").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.import_dialog(cx)),
            ))
            .child(small_button("📖 书库").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_library = !this.show_library;
                    cx.notify();
                }),
            ));

        // 空书架提示
        if self.library.books.is_empty() {
            head = head.child(
                div()
                    .flex_1()
                    .p_1()
                    .text_size(px(12.0))
                    .text_color(rgb(CINNABAR))
                    .child(SharedString::from(&self.notice)),
            );
        }

        let entries: Vec<(usize, ShelfEntry)> = self
            .library
            .books
            .iter()
            .cloned()
            .enumerate()
            .collect();

        let mut wall = div().flex().flex_wrap().gap_6().p_6().items_start();
        if entries.is_empty() {
            wall = wall.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .p_8()
                    .bg(rgb(PAPER))
                    .border_1()
                    .border_color(rgb(FRAME))
                    .rounded(px(4.0))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .text_color(rgb(INK))
                            .child(SharedString::from("书架还空着")),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(0x8A8175))
                            .child(SharedString::from("支持 md / txt / epub / json")),
                    )
                    .child(small_button("导入第一本书").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.import_dialog(cx)),
                    )),
            );
        }

        for (idx, entry) in entries {
            wall = wall.child(self.render_book_card(idx, &entry, cx));
        }

        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .bg(rgb(0xF4F4F6));
        // 键盘入口统一挂在窗口根元素（见 app.rs 的 Render::render）→ Root::dispatch_key，
        // 书架态路由到 handle_shelf_key：重命名键入 / Esc 收起书库 / 取消删除确认。
        root = root.child(head);
        // 「正在阅读」：读过的书按最近阅读排序横排，一键回到上次读的地方。
        if let Some(strip) = self.render_reading_strip(cx) {
            root = root.child(strip);
        }
        root = root.child(div().flex_1().overflow_hidden().child(wall));
        root = root.child(self.render_shelf_status());
        // 书库浮层：内置可下载的书。
        if self.show_library {
            root = root.child(self.render_library_panel(cx));
        }
        // 重命名对话框。
        if self.rename_idx.is_some() {
            root = root.child(self.render_rename_dialog(cx));
        }
        root
    }

    /// 「正在阅读」横条：读过（有进度或有阅读时间）的书，按最近阅读排序，
    /// 最多 6 本；没有则整条不显示。每本带迷你封面、进度与「继续」按钮。
    fn render_reading_strip(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let mut reading: Vec<(usize, ShelfEntry)> = self
            .library
            .books
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, e)| e.last_read > 0 || e.last_page > 0)
            .collect();
        // 最近读过的排最前；没时间戳的老档案按进度兜底。
        reading.sort_by(|a, b| {
            b.1.last_read
                .cmp(&a.1.last_read)
                .then(b.1.last_page.cmp(&a.1.last_page))
        });
        reading.truncate(6);
        if reading.is_empty() {
            return None;
        }

        let mut row = div()
            .id("reading-strip")
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .p_3()
            .overflow_x_scroll();
        for (idx, e) in &reading {
            let idx = *idx;
            let accent = e.accent;
            let prog = e.progress();
            let pct = (prog * 100.0).round() as i32;
            // 迷你封面：主色块 + 竖排书名（自适应 40×60 小盒）。
            let mini = div()
                .w(px(46.0))
                .h(px(64.0))
                .flex_shrink_0()
                .bg(rgb(accent))
                .rounded(px(2.0))
                .flex()
                .items_center()
                .justify_center()
                .child(vertical_block(&e.title, 40.0, 60.0, 13.0, PAPER, FontWeight::MEDIUM));
            let info = div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(INK))
                        .child(SharedString::from(clip_title_w(&e.title, 132.0, 12.5))),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(0x8A8175))
                        .child(SharedString::from(format!(
                            "第 {} 面 · {}%",
                            e.last_page + 1,
                            pct
                        ))),
                )
                .child(
                    div()
                        .w(px(72.0))
                        .h(px(3.0))
                        .bg(rgb(0x00000014))
                        .child(
                            div()
                                .h(px(3.0))
                                .w(px(72.0 * prog))
                                .bg(rgb(accent)),
                        ),
                );
            let resume = small_button("继续 ▸").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_book(idx);
                    cx.notify();
                }),
            );
            let item = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w(px(252.0))
                .flex_shrink_0()
                .p_2()
                .bg(rgb(0xFEFDFC))
                .border_1()
                .border_color(rgb(0xE6E1D6))
                .rounded(px(4.0))
                .child(mini)
                .child(info)
                .child(resume);
            row = row.child(item);
        }

        Some(
            div()
                .flex()
                .flex_col()
                .bg(rgb(0xFEFDFC))
                .border_b_1()
                .border_color(rgb(0xE6E1D6))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px_4()
                        .pt_3()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(rgb(CINNABAR))
                                        .child(SharedString::from("正在阅读")),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(rgb(0x8A8175))
                                        .child(SharedString::from(format!(
                                            "{} 本 · 点「继续」回到上次读的地方",
                                            reading.len()
                                        ))),
                                ),
                        ),
                )
                .child(row),
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
        //    Home End 首尾 / ⌘V 粘贴 / 可打印字符键入。
        if self.rename_idx.is_some() {
            match k {
                "escape" | "Escape" | "esc" => self.cancel_rename(),
                "enter" | "Enter" => self.commit_rename(),
                "backspace" | "Backspace" => {
                    crate::app::draft_backspace(&mut self.rename_draft, &mut self.rename_cursor)
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
                _ => {
                    if !mods.platform && !mods.control && !mods.alt {
                        if let Some(s) = event.keystroke.key_char.as_deref() {
                            if let Some(c) = s.chars().next() {
                                // 书名上限 12 字（与导入时的自动识别一致）。
                                if !c.is_control() && self.rename_draft.chars().count() < 12 {
                                    crate::app::draft_insert(
                                        &mut self.rename_draft,
                                        &mut self.rename_cursor,
                                        c,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            cx.notify();
            return;
        }

        // ② Esc：收起书库浮层 / 取消删除确认。
        if k == "escape" || k == "Escape" || k == "esc" {
            self.show_library = false;
            self.delete_confirm = None;
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

    /// 一本书的书封。
    fn render_book_card(
        &mut self,
        idx: usize,
        entry: &ShelfEntry,
        cx: &mut Context<Self>,
    ) -> Div {
        let accent = entry.accent;
        let prog = entry.progress();
        let seal = if entry.seal.is_empty() {
            "笺读".to_string()
        } else {
            // 印章只容两字，超出截断（印框 26px，避免长印章溢出）。
            entry.seal.chars().take(2).collect()
        };
        let pct = (prog * 100.0).round() as i32;

        let card = div()
            .flex()
            .flex_col()
            .w(px(160.0))
            .h(px(224.0))
            .p_3()
            .bg(rgb(PAPER))
            .border_1()
            .border_color(rgb(accent))
            .rounded(px(2.0))
            // 书脊线
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(accent))
                            .child(SharedString::from("笺读藏本")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_center()
                            // 竖排书名自适应卡片中部可用区域（136×148）：
                            // 长名自动分列，从右到左，绝不溢出书封。
                            .child(vertical_block(
                                &entry.title,
                                136.0,
                                148.0,
                                20.0,
                                INK,
                                FontWeight::MEDIUM,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_end()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(9.0))
                                            .text_color(rgb(0x8A8175))
                                            .child(SharedString::from(format!(
                                                "{pct}% · 第{}面",
                                                entry.last_page + 1
                                            ))),
                                    )
                                    .child(
                                        div()
                                            .w(px(70.0))
                                            .h(px(3.0))
                                            .bg(rgb(0x00000014))
                                            .child(
                                                div()
                                                    .h(px(3.0))
                                                    .w(px(70.0 * prog))
                                                    .bg(rgb(accent)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(26.0))
                                    .h(px(26.0))
                                    .bg(rgb(accent))
                                    .rounded(px(2.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(PAPER))
                                            .child(SharedString::from(seal)),
                                    ),
                            ),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_book(idx);
                    cx.notify();
                }),
            );

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(card)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w(px(160.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(INK))
                            .child(SharedString::from(clip_title_w(&entry.title, 158.0, 12.0))),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(0x8A8175))
                            .child(SharedString::from(if entry.author.is_empty() {
                                "未署名".to_string()
                            } else {
                                format!("〔{}〕", clip_title_w(&entry.author, 150.0, 10.0))
                            })),
                    ),
            )
            // 管理行：重命名 / 删除（删除需二次确认，防误触）。
            .child(self.render_card_manage(idx, cx))
    }

    /// 书封下方的管理行：重命名 / 删除；点过「删除」后变为二次确认态。
    fn render_card_manage(&mut self, idx: usize, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(160.0));
        if self.delete_confirm == Some(idx) {
            let confirm = div()
                .px_2()
                .py_1()
                .rounded(px(7.0))
                .bg(rgb(CINNABAR))
                .text_size(px(10.5))
                .text_color(rgb(PAPER))
                .cursor_pointer()
                .child(SharedString::from("确认删除"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.delete_book(idx);
                        cx.notify();
                    }),
                );
            row = row.child(confirm).child(small_button("点错了").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.delete_confirm = None;
                    cx.notify();
                }),
            ));
        } else {
            row = row
                .child(small_button("重命名").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.start_rename(idx);
                        cx.notify();
                    }),
                ))
                .child(small_button("删除").on_mouse_down(
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
                "点书封开始阅读 · 封面下可重命名 / 删除 · 进度自动记住",
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
        let title = if book.cover.title.is_empty() {
            book.title.clone()
        } else {
            book.cover.title.clone()
        };
        let subtitle = book.cover.subtitle.clone();
        let seal = book.cover.seal_or_default();
        let author = book.author.clone();
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
                    .child(vertical_block(
                        &title,
                        42.0,
                        196.0,
                        23.0,
                        0x2A241E,
                        FontWeight::MEDIUM,
                    )),
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
            // 竖排大书名自适应封面中部可用区域（312×318）：长名自动分列，
            // 从右到左，保持古籍版式且不溢出封面。
            .child(vertical_block(&title, 312.0, 318.0, 52.0, 0xEDE3CC, FontWeight::MEDIUM))
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
        let per_col = ((CARD_BOX.1 - 4.0) / (fs + 4.0)).floor() as usize;
        assert!(cols.len() >= 2, "长名应分列，实际 {:?}", cols);
        assert!(cols.iter().all(|c| c.chars().count() <= per_col));
        assert!(cols.len() as f32 * (fs + 4.0) <= CARD_BOX.0 + 0.01);

        // 超长名：缩到最小字号仍装不下时截断，且结果依然放得进盒子。
        let long: String = "长".repeat(100);
        let (fs, cols) = fit_vertical(&long, CARD_BOX.0, CARD_BOX.1, 20.0);
        let per_col = ((CARD_BOX.1 - 4.0) / (fs + 4.0)).floor() as usize;
        let max_cols = (CARD_BOX.0 / (fs + 4.0)).floor() as usize;
        let total: usize = cols.iter().map(|c| c.chars().count()).sum();
        assert!(total < long.chars().count(), "超长名应被截断");
        assert!(total <= per_col * max_cols);
        assert!(cols.iter().all(|c| c.chars().count() <= per_col));
        assert!(cols.len() as f32 * (fs + 4.0) <= CARD_BOX.0 + 0.01);
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
}
