//! **解锁那张纸**（v1.9.23 · 内购链路的前台）。
//!
//! 只有 `--features appstore` 的构建才会画它：官网版走离线 key（`license.rs`
//! 那张激活纸），商店版按 App Review 2.4.5(vi) 不许出现 licence key，只能有
//! 「买断」这一条路。
//!
//! # 它为什么必须存在
//!
//! `storekit.rs` 把购买链路接通了，但没有这张纸就等于「有门无门框」：用户
//! 点导出被拦下，提示说「需先解锁」，然后**没有任何地方能解锁**。所以这张
//! 纸和 `storekit` 是同一件事的两半，缺一半比不做更糟。
//!
//! # 三个状态都要说人话
//!
//! StoreKit 的回调是异步的（可能要弹系统登录框、可能要等家长批准），
//! 所以这里不假设「点了就成」，而是把 `Phase` 八态摊开：
//!
//! * 取价中 / 已就绪 → 显示**真实价格**（Guideline 3.1.1 不许写死价）；
//! * 支付中 / 恢复中 → 明说「等 App Store 确认」，别让人以为卡死；
//! * 查不到商品 → 直接把排查方向写上屏（后台没建 IAP / Bundle ID 不符）；
//! * 已解锁 → 关门，别再让人点第二次。
//!
//! 轮询用 `poll_unlock`（每帧看一眼原子量），与 `poll_import` 同一套路：
//! 状态机是进程级原子量，UI 只需要知道「变了没有」。

use crate::app::Root;
use crate::storekit::{self, Phase};
use gpui::*;

impl Root {
    /// 打开解锁纸（导出被拦时、或「更多 → 工具 → 解锁」）。
    pub fn open_unlock(&mut self, cx: &mut Context<Self>) {
        self.show_unlock = true;
        // 与其余浮层互斥：叠在版字 / 检索之上会打架。
        self.show_license = false;
        self.show_about = false;
        self.show_help = false;
        self.show_keys = false;
        self.show_analysis = false;
        self.analysis = None;
        self.more_menu = false;
        self.more_group = None;
        self.close_side();
        self.close_transient_ui();
        // 记下当前相位：下一帧 `poll_unlock` 才有「变没变」的基准。
        self.unlock_phase = storekit::phase_id();
        cx.notify();
    }

    /// 每帧看一眼相位；**变了才 `notify`**，否则会自激成死循环。
    pub fn poll_unlock(&mut self, cx: &mut Context<Self>) {
        if !self.show_unlock {
            return;
        }
        let now = storekit::phase_id();
        if now == self.unlock_phase {
            return;
        }
        let before = self.unlock_phase;
        self.unlock_phase = now;
        // 买成了：给一句 toast 并把纸收掉——用户的下一个动作是去点那个刚被
        // 拦下的导出，不该还挡着一张「已解锁」的纸让他自己找关闭。
        if storekit::purchased() {
            self.notice = "已解锁：导出与印制可以用了".to_string();
            self.show_unlock = false;
        } else if before != now && storekit::phase() == Phase::Failed {
            // 失败不关纸：错误文案就在纸上，关掉等于把话吞回去。
        }
        cx.notify();
    }

    /// 点「解锁全部功能」。
    pub fn buy_unlock(&mut self, cx: &mut Context<Self>) {
        if storekit::purchase() {
            self.unlock_phase = storekit::phase_id();
        } else {
            // 买不了的原因已经在 `last_error` 里，纸上会显示。
            self.unlock_phase = storekit::phase_id();
        }
        cx.notify();
    }

    /// 点「恢复购买」。
    pub fn restore_unlock(&mut self, cx: &mut Context<Self>) {
        storekit::restore();
        self.unlock_phase = storekit::phase_id();
        cx.notify();
    }

    fn unlock_button(&self, label: &str, primary: bool, dim: bool) -> Div {
        let t = self.theme;
        let zhu = crate::reader::sheet_zhu(t);
        if dim {
            // 「正在等…」态：画成按钮但不可点，眼神上要看出「它自己在动」。
            return div()
                .px(px(22.0))
                .py(px(9.0))
                .rounded(px(4.0))
                .bg(rgba(((t.ink() & 0xFFFFFF) << 8) | 0x0F))
                .text_size(px(12.5))
                .text_color(rgb(t.ink_soft()))
                .child(SharedString::from(label.to_string()));
        }
        if primary {
            div()
                .px(px(22.0))
                .py(px(9.0))
                .rounded(px(4.0))
                .bg(rgb(zhu))
                .text_size(px(12.5))
                .text_color(rgb(t.paper()))
                .cursor_pointer()
                .hover(|s| s.opacity(0.9))
                .child(SharedString::from(label.to_string()))
        } else {
            div()
                .px(px(14.0))
                .py(px(8.0))
                .text_size(px(11.0))
                .text_color(rgb(t.ink_soft()))
                .cursor_pointer()
                .hover(move |s| s.text_color(rgb(zhu)))
                .child(SharedString::from(label.to_string()))
        }
    }

    pub fn render_unlock_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let zhu = crate::reader::sheet_zhu(t);
        let family = crate::fonts::family_for(crate::types::FontChoice::WenJinMincho, crate::types::Script::Ancient);
        let phase = storekit::phase();
        let owned = storekit::purchased();
        let price = storekit::price();
        let err = storekit::last_error();

        // ---- 抬头：解锁 / 状态 ----
        let state_word = if owned {
            "已解鎖"
        } else {
            match phase {
                Phase::Loading => "取價中",
                Phase::Purchasing => "等待確認",
                Phase::Restoring => "恢復中",
                Phase::Ready => "待解鎖",
                _ => "待解鎖",
            }
        };
        let head = div()
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
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgb(t.accent()))
                            .child(SharedString::from("解鎖")),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from(state_word)),
                    ),
            )
            .child(crate::reader::small_button("关闭", false, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.show_unlock = false;
                    cx.notify();
                }),
            ));

        // ---- 价格 / 状态大字 ----
        // 取不到商品时**不编一个假价格**：Guideline 3.1.1 要求展示真实价，
        // 而写死一个「¥198」在后台改价后就成了欺骗。宁可显示「待取價」，
        // 把原因交给下面那行错误说明。
        let big = if owned {
            "已解鎖".to_string()
        } else {
            match &price {
                Some(p) => p.clone(),
                None => "待取價".to_string(),
            }
        };
        let price_row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_baseline()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(if price.is_some() || owned { 30.0 } else { 18.0 }))
                    .font_family(family.clone())
                    .text_color(rgb(if owned || price.is_some() {
                        t.ink()
                    } else {
                        t.ink_soft()
                    }))
                    .child(SharedString::from(big)),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(t.ink_soft()))
                    .child(SharedString::from("一次買斷 · 不訂閱")),
            );

        // ---- 说清楚「解锁的是什么」----
        // 拆成两行短句，不用一整行长串：GPUI 0.2.2 的换行只在容器有确定宽度时
        // 才可靠，手工断句比赌它自动折行稳。
        let scope = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .text_size(px(11.0))
            .font_family(family.clone())
            .text_color(rgb(t.ink_soft()))
            .child(SharedString::from(
                "解鎖後：出圖 · 印製 PDF · Markdown · 朱批錄",
            ))
            .child(SharedString::from("　　　　以及 Anki / Obsidian 卡片導出"))
            .child(SharedString::from(
                "閱讀、批註、卡片匣一直免費——沒解鎖也能讀完一本書。",
            ));

        // ---- 主按钮 ----
        let busy = matches!(phase, Phase::Purchasing | Phase::Restoring);
        let can_buy = !owned && !busy && matches!(phase, Phase::Ready);
        let main_btn = if owned {
            self.unlock_button("已解鎖", false, true)
        } else if busy {
            self.unlock_button(
                if phase == Phase::Restoring { "正在恢復…" } else { "正在等待 App Store 確認…" },
                true,
                true,
            )
        } else if can_buy {
            self.unlock_button("解鎖全部功能", true, false)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.buy_unlock(cx);
                    }),
                )
        } else {
            // 取价中 / 查不到商品：主按钮画成灰的，但「恢复购买」始终可点。
            self.unlock_button("解鎖全部功能", true, true)
        };

        let restore = if owned {
            div()
        } else {
            self.unlock_button("恢復購買（換機器 / 重裝後）", false, busy)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.restore_unlock(cx);
                    }),
                )
        };

        // ---- 错误行（含诊断）----
        let err_row = match (&err, phase) {
            (Some(e), _) if !owned => div()
                .text_size(px(10.5))
                .font_family(family.clone())
                .text_color(rgb(zhu))
                .child(SharedString::from(e.clone())),
            _ => div(),
        };

        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(price_row)
            .child(scope)
            .child(div().h(px(2.0)))
            .child(div().flex().flex_row().items_center().gap(px(10.0)).child(main_btn).child(restore))
            .child(err_row);

        let sheet = div()
            .w(px((vw * 0.52).clamp(400.0, 520.0)))
            .max_h(px(vh * 0.82))
            .flex()
            .flex_col()
            .occlude()
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.frame()))
            .rounded(px(8.0))
            .shadow_lg()
            .child(head)
            .child(div().flex().flex_col().gap(px(6.0)).px_4().py_4().child(body));

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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_unlock = false;
                    cx.notify();
                }),
            )
    }
}
