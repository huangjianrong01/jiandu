//! **内测闸门的那一张纸**（v1.0.0-beta）。
//!
//! `Render::render` 在 `beta_ok == false` 时**只**画这里的东西：竖排的「箋讀」
//! 二字、一行「內測邀請碼」、一只下划线输入框、一行小字提示。书架 / 正文 /
//! 所有浮层都不进场，也没有「点空白处关闭」——这是唯一的门。
//!
//! 三件事都极简：
//!   * 码在 `jiandu_core::gate::CODES`（3 组固定码，改一处即可）；
//!   * 验过就写一枚凭据进应用数据目录，之后启动不再问；
//!   * 中文输入法在这张纸上用不上（码是英数），但**照样挂 IME 通路**——
//!     万一有人用「全角」或候选词敲，`InputTarget::Beta` 也接得住。

use crate::app::Root;
use crate::theme::Theme;
use gpui::*;

/// 启动时定一次：内测闸门是否已过。
///
/// v1.9.9：商店版**恒 true**。App Store 没有内测码这回事，用户从商店装下来
/// 就该直接进应用。若沿用 `gate::redeemed()`（读本机那枚凭据文件），商店版
/// 永远读不到，`beta_ok` 恒 false，后果是两层：
///   ① `Render::render` 只画邀请码那一屏（见本文件顶部）——应用等于打不开；
///   ② `ime::input_target()` 开头直接 `return None`，**所有输入框都不接
///      输入法**：眉批 / 夹注 / 检索一个汉字都打不进去。中文不走 keydown，
///      全靠 `EntityInputHandler` 回传，这一条断了就是整个输入框废掉。
pub fn gate_passed() -> bool {
    #[cfg(feature = "appstore")]
    {
        true
    }
    #[cfg(not(feature = "appstore"))]
    {
        crate::gate::redeemed()
    }
}

impl Root {
    /// 把一串字符追加进邀请码草稿（已归一：只留 ASCII 英数，转大写）。
    pub fn beta_push(&mut self, text: &str) {
        for c in text.chars() {
            let c = c.to_ascii_uppercase();
            if !(c.is_ascii_alphanumeric()) {
                continue;
            }
            if self.beta_code.chars().count() >= 24 {
                break;
            }
            let at = self.beta_cursor.min(self.beta_code.chars().count());
            let byte = self
                .beta_code
                .char_indices()
                .nth(at)
                .map(|(i, _)| i)
                .unwrap_or(self.beta_code.len());
            self.beta_code.insert(byte, c);
            self.beta_cursor = at + 1;
        }
        self.beta_err = false;
    }

    /// 退格：删光标前一个字。
    pub fn beta_backspace(&mut self) {
        let n = self.beta_code.chars().count();
        if self.beta_cursor == 0 || n == 0 {
            return;
        }
        let at = (self.beta_cursor - 1).min(n - 1);
        let start = self.beta_code.char_indices().nth(at).map(|(i, _)| i);
        if let Some(start) = start {
            let end = self
                .beta_code
                .char_indices()
                .nth(at + 1)
                .map(|(i, _)| i)
                .unwrap_or(self.beta_code.len());
            self.beta_code.replace_range(start..end, "");
            self.beta_cursor = at;
        }
        self.beta_err = false;
    }

    /// 校验并（成功时）落凭据。返回是否放行。
    pub fn beta_submit(&mut self) -> bool {
        if crate::gate::valid(&self.beta_code) {
            crate::gate::remember(&self.beta_code);
            self.beta_ok = true;
            // 当场落起算日并算一次剩余天数：不然刚敲完码还要重启一次才认得
            // 内测期（`licensed()` 读的是这个缓存字段）。
            self.beta_left = crate::gate::beta_days_left().unwrap_or(0);
            self.beta_err = false;
            // 进门后把焦点交回阅读键位（否则方向键不翻页）。
            true
        } else {
            self.beta_err = true;
            false
        }
    }

    /// 这张纸上的按键：只认「编辑键 + 可打印英数 + ⌘V」。
    ///
    /// 与阅读视图同一口径：**可打印字符不在 keydown 里直插**（IME 也会送来
    /// 一遍，直插必叠字），这里之所以例外地收 `key_char`，是因为码是纯英数、
    /// 输入法不介入；万一有 IME 送来，`InputTarget::Beta` 那条路会把字归一后
    /// 再插一次 —— 两条路都过 `beta_push` 归一，最多是同一个字进两次，
    /// 但 IME 只在真的组字时才会送，实际不会同时发生。
    pub fn handle_beta_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        // ⌘V：从剪贴板粘一段码（测试同学常这么干）。
        if mods.platform && k == "v" {
            if let Some(t) = crate::clipboard::paste_text(cx) {
                let t = t.split_whitespace().next().unwrap_or("").to_string();
                self.beta_push(&t);
            }
            cx.notify();
            return;
        }
        // ⌘ 组合键一律不接（⌘Q 该退出就退出）。
        if mods.platform || mods.control {
            return;
        }
        match k {
            "enter" | "return" => {
                self.beta_submit();
            }
            "backspace" | "delete" => self.beta_backspace(),
            // Esc 不关门：这道闸没有「取消」。
            "escape" | "esc" => {}
            _ => {
                if let Some(c) = event.keystroke.key_char.as_ref() {
                    self.beta_push(c);
                }
            }
        }
        // ★ 关键一行：吃掉这个键。
        //
        // GPUI（macOS）的 keyDown 顺序是「先跑 keydown 回调，未被消费才转给
        // `[inputContext handleEvent:]`」。这里不 `stop_propagation`，平台就会
        // 再把同一个可打印键送进输入法上下文 → `insertText:` 又上屏一次，
        // 实测 `abc123` 得到 `ABCABC112233`。吃掉它，输入法对这张纸就是哑的，
        // 也不会因为一次误触把拼音组字条吊在屏幕角落。
        cx.stop_propagation();
        cx.notify();
    }

    /// 画这张纸。整幅铺满窗口（不透明纸色），别处没有任何可点区域。
    pub fn render_beta_gate(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t: Theme = self.theme;
        let zhu = crate::reader::sheet_zhu(t);
        // 焦点在这张纸上，键才收得到（`track_focus` 让派发树里有这一节点）。
        if !self.beta_focus.is_focused(window) {
            window.focus(&self.beta_focus);
        }
        let ink = rgb(t.ink());
        let soft = rgb(t.ink_soft());
        let family = self.main_family();

        // 竖排「箋讀」：一字一格往下叠（GPUI 0.2.2 没有竖排文字，全仓通用写法）。
        let mut title = div().flex().flex_col().items_center();
        for c in "箋讀".chars() {
            title = title.child(
                div()
                    .h(px(46.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(34.0))
                    .font_family(family.clone())
                    .text_color(ink)
                    .child(SharedString::from(c.to_string())),
            );
        }

        // 输入框正文：空白时显示灰色的「請輸入邀請碼」。
        let shown = if self.beta_code.is_empty() {
            div()
                .text_color(soft)
                .child(SharedString::from("請輸入邀請碼"))
        } else {
            div()
                .text_color(ink)
                .child(SharedString::from(self.beta_code.clone()))
        };
        let caret = div()
            .w(px(1.0))
            .h(px(15.0))
            .bg(rgb(zhu))
            .ml(px(1.0));
        let field_box = div()
            .w_full()
            .h(px(36.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(1.0))
            .text_size(px(17.0))
            .font_family(family.clone())
            // 字距拉开一点，像手抄的号码，也便于逐字核对。
            .child(shown)
            .child(caret);

        // 输入框**不挂 IME**：邀请码是英数，走 keydown 一条路就够；
        // 挂了输入法反而两条路都来一遍（`abc123` → `ABCABC112233`，实测）。
        // 见 `handle_beta_key` 里那句 `stop_propagation`——它同时挡住了
        // 「输入法把字母吃掉组字」这条死路。
        let field = field_box;

        let mut sheet = div()
            .w(px(320.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.0))
            .pt(px(28.0))
            .pb(px(18.0))
            .px(px(18.0))
            // 极淡的墨界：像一张笺纸的四周。
            .border_1()
            .border_color(rgba(((t.rule() & 0xFFFFFF) << 8) | 0x44))
            .child(title)
            .child(div().h(px(6.0)))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_family(family.clone())
                    .text_color(soft)
                    .child(SharedString::from("內　測　邀　請　碼")),
            )
            .child(
                div()
                    .w(px(240.0))
                    .border_b_1()
                    .border_color(rgba(((t.rule() & 0xFFFFFF) << 8) | 0x66))
                    .pb(px(2.0))
                    .child(field),
            );

        // 提示 / 报错：同一行的位置，二选一，不做抖动（抖动要动画，且廉价）。
        sheet = sheet.child(if self.beta_err {
            div()
                .text_size(px(11.0))
                .font_family(family.clone())
                .text_color(rgb(zhu))
                .child(SharedString::from("邀請碼不正確，請核對後再試"))
        } else {
            div()
                .text_size(px(11.0))
                .font_family(family.clone())
                .text_color(soft)
                .child(SharedString::from("回車進入　·　邀請碼見內測群公告"))
        });

        let ver = div()
            .absolute()
            .bottom(px(16.0))
            .right(px(18.0))
            .text_size(px(10.0))
            .font_family(family.clone())
            .text_color(rgba(((t.ink_soft() & 0xFFFFFF) << 8) | 0x99))
            .child(SharedString::from(format!(
                "v{} · 內測",
                env!("CARGO_PKG_VERSION")
            )));

        div()
            .relative()
            .size_full()
            .bg(rgb(t.paper()))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.beta_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                this.handle_beta_key(event, cx);
            }))
            .child(sheet)
            .child(ver)
    }
}
