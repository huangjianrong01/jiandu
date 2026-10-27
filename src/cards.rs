//! **卡片匣**（v1.74.0）：跨书摘录卡的 UI 层。
//!
//! 与「朱批录」是两码事，别混：
//!   * 朱批**长在书里**——跟着 `Book` 一起序列化进 `books.meta_json`，
//!     换台机器重导这本书，批注跟着走；但**换一本书就看不见了**。
//!   * 卡片**汇在匣里**——读《庄子》摘一句、读《史记》摘一句，全进同一个
//!     匣子；书名 / 章名 / 原文都存了快照，**书从架上删掉卡片也还在**
//!     （只是跳不回原文了）。
//!
//! 落盘见 `jiandu_core::cards`（与章节库同一个 `chapters.db`）。定位用
//! `(章, 段, 段内字偏移)`——与眉批 / 朱批**同一把尺子**，点一条卡片直接
//! 复用现成的 `jump_to_key` 跳回原文。

use crate::app::Root;
use crate::reader::sheet_zhu;
use gpui::{
    div, prelude::*, px, rgb, Context, Div, MouseButton, SharedString, Stateful,
};
use jiandu_core::cards::{self, Card, CardKind};

/// 列表一次最多取这么多条。
///
/// 攒到几千张时全量塞进 UI 会把列表拖死；真要翻旧卡是模块一（FTS5 全文
/// 检索）的活儿，那时改成「检索 + 分页」，不是在这里硬撑。
const LIST_LIMIT: usize = 500;

/// 卡片条目上原文最多显示多少字（超出以「…」收尾）。
const QUOTE_SNIP: usize = 60;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Root {
    /// 从库里重读卡片（开栏 / 增删后调用）。
    ///
    /// 失败一律静默：卡片匣读不出来**不该打断阅读**，顶多是空匣子。
    /// 不做每帧查询——侧栏是低频面板，开一次读一次就够。
    pub(crate) fn reload_cards(&mut self) {
        self.cards = match jiandu_core::store::connect() {
            Ok(c) => cards::list_cards(&c, LIST_LIMIT).unwrap_or_default(),
            Err(_) => Vec::new(),
        };
    }

    /// **选中即入匣**：把当前选区的原文做成一张卡片。
    ///
    /// 选区优先；没有选区（只是点了某个字弹了工具条）就退化为那**一个字**。
    /// 手滑连点两次不会在匣里躺两张一样的卡（`cards::same_place` 挡一道）。
    pub fn save_selection_card(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            self.notice = "尚未打开书籍，无法入匣".to_string();
            cx.notify();
            return;
        };
        // 原文：拖选优先，否则退回工具条上那一个字。
        let quote = self.selected_text().trim().to_string();
        // 锚点：拖选取选区首字；只是点了个字弹工具条，就用那个字的位置。
        let key = self
            .selection_range()
            .map(|(a, _)| a)
            .or_else(|| self.popup.as_ref().map(|h| h.key));
        let (Some(key), false) = (key, quote.is_empty()) else {
            self.notice = "请先选中要入匣的文字".to_string();
            cx.notify();
            return;
        };

        let new = Card {
            id: 0,
            book: book.title.clone(),
            chapter: self.chapter_title(key.0),
            store_id: book.store_id.clone().unwrap_or_default(),
            key,
            len: quote.chars().count(),
            quote: quote.clone(),
            note: String::new(),
            kind: CardKind::Quote,
            created_ms: now_ms(),
        };

        // 去重要拿**库里**的最新状态比：开栏后一直在加卡，`self.cards`
        // 未必是最新（比如刚删过）。
        self.reload_cards();
        if self.cards.iter().any(|c| cards::same_place(c, &new)) {
            self.notice = "匣中已有此句".to_string();
            self.toast = Some("匣中已有此句".to_string());
            cx.notify();
            return;
        }

        let res = jiandu_core::store::connect().and_then(|c| cards::put_card(&c, &new));
        match res {
            Ok(_) => {
                self.reload_cards();
                let tip = format!("已入匣\n{}", snip(&quote));
                self.notice = format!("已入匣：{}", new.source());
                // 阅读视图里 `notice` 看不见（只在书架底栏渲染），
                // 所以回执走底部那枚宣纸提示条。
                self.toast = Some(tip);
                // 动作做完就收工：工具条与选区都清掉，免得挡着继续读。
                self.popup = None;
                self.clear_selection();
            }
            Err(e) => {
                let m = format!("入匣失败：{e}");
                self.notice = m.clone();
                self.toast = Some(m);
            }
        }
        cx.notify();
    }

    /// 丢弃一枚卡片（匣里的「刪」钮）。
    pub fn drop_card(&mut self, id: i64, cx: &mut Context<Self>) {
        let res = jiandu_core::store::connect().and_then(|c| cards::delete_card(&c, id));
        match res {
            Ok(()) => {
                self.reload_cards();
                self.notice = "已丢弃这张卡片".to_string();
                self.toast = Some("已丢弃".to_string());
            }
            Err(e) => {
                let m = format!("删除失败：{e}");
                self.notice = m.clone();
                self.toast = Some(m);
            }
        }
        cx.notify();
    }

    /// **卡片匣侧栏**：右滑宣纸栏，逐条列卡片；点一条即收栏并跳回原文。
    ///
    /// 形态照朱批录（`render_notes_panel`）来，读作同一套东西的两个抽屉：
    /// 一个装「批在这本书上的字」，一个装「从各本书里摘出来的句」。
    /// **笔记抽屉 · 卡片匣页**的条目列表（v1.78.0）：跨书摘录卡，点一条即
    /// 收栏并跳回原文。
    ///
    /// **只有列表，不含外壳**——外壳与「朱批錄 / 卡片匣」两枚页签在
    /// `Root::render_notes_panel`。原先卡片匣是一整个独立抽屉（自己的外壳、
    /// 自己的滑入动画、自己的顶栏钮），与朱批录互斥；现在两页共用一个抽屉，
    /// 切页签只换这一块内容。
    pub(crate) fn render_cards_list(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme;
        let zhu = sheet_zhu(t);
        let ink = t.ink();
        let ink_soft = t.ink_soft();
        // 极细墨线（条目分隔）：纸 + 墨 16%。
        let sept = crate::theme::mix(t.paper(), t.ink(), 0.16);

        let rows: Vec<(i64, String, String, (usize, usize, usize), usize, CardKind, String)> =
            self.cards
                .iter()
                .map(|c| {
                    (
                        c.id,
                        c.source(),
                        c.quote.clone(),
                        c.key,
                        c.len,
                        c.kind,
                        c.note.clone(),
                    )
                })
                .collect();

        let mut list = div()
            .id(SharedString::from("cards-list"))
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll();
        if rows.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_size(px(11.0))
                    .text_color(rgb(ink_soft))
                    .child(SharedString::from(
                        "匣中空空。框選正文，於浮簽點「存卡」即入匣。",
                    )),
            );
        }
        for (i, (id, source, quote, key, len, kind, note)) in rows.into_iter().enumerate() {
            let head = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(zhu))
                        .child(SharedString::from(kind.label())),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.5))
                        .text_color(rgb(ink_soft))
                        .child(SharedString::from(source)),
                );
            let mut body = div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .flex_1()
                .child(head)
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(ink))
                        .child(SharedString::from(snip(&quote))),
                );
            if !note.is_empty() {
                body = body.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(crate::theme::mix(ink, zhu, 0.35)))
                        .child(SharedString::from(snip(&note))),
                );
            }

            let row = div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(6.0))
                .p_3()
                .cursor_pointer()
                .border_b_1()
                .border_color(rgb(sept))
                .when(i == 0, |s| s.border_t_1().border_color(rgb(sept)))
                .hover(|s| s.bg(rgb(crate::theme::mix(t.paper(), zhu, 0.06))))
                .child(body)
                .child(
                    // 「刪」：单字 CJK（✕ 那类拉丁符号在 GPUI 里是零宽，画不出来）。
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(ink_soft))
                        .cursor_pointer()
                        .hover(move |s| s.text_color(rgb(zhu)))
                        .child(SharedString::from("刪"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                // 拦住冒泡：删卡不该顺带把人送回原文。
                                cx.stop_propagation();
                                this.drop_card(id, cx);
                            }),
                        ),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.show_cards_panel = false;
                        this.jump_to_key(key, len, cx);
                    }),
                );
            list = list.child(row);
        }

        list
    }
}

/// 截断长文（列表里一张卡不该占半屏）。
fn snip(s: &str) -> String {
    let n = s.chars().count();
    if n <= QUOTE_SNIP {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(QUOTE_SNIP).collect::<String>())
    }
}
