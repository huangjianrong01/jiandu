//! **授权（v1.81.0 · 模块六）的那一张纸**：居中宣纸浮层，敲一枚离线 key。
//!
//! 与内测闸门（`beta.rs`）的分工：那张纸管「能不能进这个包」，这张管
//! 「能不能出活」——导出与印制。纯逻辑（校验位 / 归一 / 凭据）在
//! `jiandu_core::license`，这里只管画与收键。
//!
//! 三条自律：
//!   * **不绑机，所以纸上一个字都不问机器**。key 只校验自身，不问 MAC、
//!     硬盘号、主板号——换电脑重装照样能用，这是拍过板的。
//!   * **key 是英数，故只走 keydown 一条路**（与邀请码同款：挂 IME 会
//!     `abc123` → `ABCABC112233`）。⌘V 单独开一道，方便从邮件里粘。
//!   * **激活后不提供「注销」**。注销只会把人锁在门外，出了问题还得我
//!     手工补凭据，收益是负的。
//!
//! 未激活时**只拦导出**：`export_begin` 一处收口，阅读 / 批注 / 卡片匣 /
//! 文本分析照旧——没付钱也该能读完一本书。

use crate::app::Root;
use crate::theme::Theme;
use gpui::*;

/// 草稿里允许出现的字形：`license::ALPHABET`（数字去 0 1，字母去 I O）。
///
/// 不在表上的字符一律丢掉，而不是「先收进来再报错」——字符表就是告诉用户
/// 「这枚 key 里不会出现的字」，敲了也白敲。
fn glyph_ok(c: char) -> bool {
    let c = c.to_ascii_uppercase();
    jiandu_core::license::ALPHABET.contains(&(c as u8))
}

/// 16 位草稿 → 分组展示 `XXXX-XXXX-XXXX-XXXX`。
///
/// 人一次只核对得住 4 个字符，16 位连成一串没法看。归一时会把连字符去掉，
/// 所以展示怎么分组都不影响比对。
fn grouped(code: &str) -> String {
    code.chars()
        .collect::<Vec<char>>()
        .chunks(4)
        .map(|g| g.iter().collect::<String>())
        .collect::<Vec<String>>()
        .join("-")
}

impl Root {
    /// **打开激活纸**（更多菜单「激活」）。
    ///
    /// 与其余顶层面板互斥：它是覆盖层，压在版字 / 检索之上会打架。
    pub fn open_license(&mut self, cx: &mut Context<Self>) {
        self.show_license = true;
        self.show_style = false;
        self.show_search = false;
        self.show_help = false;
        self.show_keys = false;
        self.show_analysis = false;
        self.analysis = None;
        self.more_menu = false;
        self.close_side();
        self.close_transient_ui();
        cx.notify();
    }

    /// 收下几枚字符（已归一：大写 + 只留字符表内的字形）。
    pub fn license_push(&mut self, text: &str) {
        for c in text.chars() {
            if self.license_code.chars().count() >= jiandu_core::license::KEY_LEN {
                break;
            }
            if !glyph_ok(c) {
                continue;
            }
            self.license_code.push(c.to_ascii_uppercase());
        }
        self.license_cursor = self.license_code.chars().count();
        self.license_err = false;
    }

    /// 退格：删末一位（key 只用得到末尾编辑，不值得做光标中间插入）。
    pub fn license_backspace(&mut self) {
        self.license_code.pop();
        self.license_cursor = self.license_code.chars().count();
        self.license_err = false;
    }

    /// 校验并（成功时）落凭据。返回是否通过。
    pub fn license_submit(&mut self) -> bool {
        if jiandu_core::license::activate(&self.license_code) {
            self.license_ok = true;
            self.license_err = false;
            self.notice = "已激活：导出与印制已解锁（此授权不绑机器）".to_string();
            true
        } else {
            self.license_err = true;
            false
        }
    }

    /// 这张纸上的按键：只认「编辑键 + 字符表内的字形 + ⌘V」。
    ///
    /// 与邀请码同口径：`stop_propagation` 那一行不能删——不吞掉这个键，
    /// 平台会把同一个可打印键再送进输入法上下文，实测 `abc123` 得到
    /// `ABCABC112233`。
    pub fn handle_license_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        // ⌘V：从邮件 / 微信里粘一枚 key。
        if mods.platform && k == "v" {
            if let Some(t) = crate::clipboard::paste_text(cx) {
                let t = t.split_whitespace().next().unwrap_or("").to_string();
                self.license_push(&t);
            }
            cx.notify();
            return;
        }
        if mods.platform || mods.control {
            return;
        }
        match k {
            "enter" | "return" => {
                self.license_submit();
            }
            "backspace" | "delete" => self.license_backspace(),
            // Esc 关门：这道门**有**取消（与邀请码那张纸不同，那张是全屏
            // 唯一的门，没地方可退）。
            "escape" | "esc" => self.show_license = false,
            "left" | "right" | "up" | "down" | "tab" => {}
            _ => {
                if let Some(c) = event.keystroke.key_char.as_ref() {
                    self.license_push(c);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// 画这张纸。
    ///
    /// **不管焦点**：`Render::render` 里那一段「焦点归属」统一把它交给
    /// `license_focus`（与检索同款）。这里不自己抢，是因为 render 阶段抢焦点
    /// 会与中央那套互斥判断打架（每帧互相夺，中文输入法一个字都进不来）。
    pub fn render_license_sheet(&mut self, cx: &mut Context<Self>) -> Div {
        let t: Theme = self.theme;
        let vw = self.viewport_w;
        let vh = self.viewport_h;
        let zhu = crate::reader::sheet_zhu(t);
        let ink = rgb(t.ink());
        let soft = rgb(t.ink_soft());
        let family = self.main_family();
        let licensed = self.licensed();

        // ① 标题行。
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
                            .child(SharedString::from("激活")),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(t.ink_soft()))
                            .child(SharedString::from(if licensed {
                                "授权".to_string()
                            } else {
                                "未激活".to_string()
                            })),
                    ),
            )
            .child(crate::reader::small_button("关闭", false, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.show_license = false;
                    cx.notify();
                }),
            ));

        // ② 状态行：**三态说清楚**——有正式 key / 只有内测码 / 两样都没有。
        //
        // 内测期「持码视同已激活」，但纸上必须说明白是哪一种：用户看着
        // 「已激活」却找不到自己的 key，比没激活更让人心里没底。
        let (state, state_color) = if self.license_ok {
            // 回执只露头尾各 4 位：整串打出来等于把 key 贴在屏幕上给人拍照；
            // 全遮住又没法核对「是不是这一枚」。
            let raw = std::fs::read_to_string(jiandu_core::license::ticket_path())
                .unwrap_or_default();
            let n = jiandu_core::license::normalize(&raw);
            let cs: Vec<char> = n.chars().collect();
            let masked = if cs.len() == jiandu_core::license::KEY_LEN {
                format!(
                    "{}····{}",
                    cs[..4].iter().collect::<String>(),
                    cs[12..].iter().collect::<String>()
                )
            } else {
                "····".to_string()
            };
            (format!("已激活 · 凭据 {masked}"), rgb(t.accent()))
        } else if self.beta_left > 0 {
            (
                format!(
                    "内测邀请码已放行 · 剩 {} 天（到期后须用授权 key）",
                    self.beta_left
                ),
                rgb(t.ink_soft()),
            )
        } else if self.beta_left == 0 {
            (
                "内测期已满 · 导出与印制已锁（阅读不受影响）".to_string(),
                rgb(zhu),
            )
        } else {
            (
                "未激活 · 导出与印制已锁".to_string(),
                rgb(zhu),
            )
        };
        let state_row = div()
            .text_size(px(11.5))
            .font_family(family.clone())
            .text_color(state_color)
            .child(SharedString::from(state));

        // ③ 输入框：**任何状态都给**。已激活的人要换一枚 key、或把内测码
        //    换成正式 key，都得有地方敲。
        let draft = grouped(&self.license_code);
        let n = self.license_code.chars().count();
        let field = div()
            .w_full()
            .h(px(30.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(1.0))
            .text_size(px(15.0))
            .font_family(family.clone())
            .child(if draft.is_empty() {
                div()
                    .text_color(soft)
                    .child(SharedString::from("XXXX-XXXX-XXXX-XXXX"))
            } else {
                // 分组展示，便于逐位核对（与手抄号码同款读法）。
                div()
                    .text_color(ink)
                    .child(SharedString::from(draft))
            })
            .child(div().w(px(1.0)).h(px(15.0)).bg(rgb(zhu)).ml(px(1.0)));

        let body = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(state_row)
            .child(
                div()
                    .border_b_1()
                    .border_color(rgba(((t.rule() & 0xFFFFFF) << 8) | 0x66))
                    .pb(px(2.0))
                    .child(field),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .font_family(family.clone())
                    .text_color(soft)
                    .child(SharedString::from(format!(
                        "已輸入 {}/{} 位　·　回車激活　·　⌘V 粘貼",
                        n,
                        jiandu_core::license::KEY_LEN
                    ))),
            )
            .child(if self.license_err {
                div()
                    .text_size(px(10.5))
                    .font_family(family.clone())
                    .text_color(rgb(zhu))
                    .child(SharedString::from(
                        "這枚 key 不對——請核對後再試（校驗位會擋住抄錯的字）",
                    ))
            } else {
                div()
                    .text_size(px(10.5))
                    .font_family(family.clone())
                    .text_color(soft)
                    .child(SharedString::from(
                        "此授權不綁機器——換電腦、重裝系統，這枚 key 照舊可用。",
                    ))
            });

        let sheet = div()
            .w(px((vw * 0.46).clamp(360.0, 460.0)))
            .max_h(px(vh * 0.82))
            .flex()
            .flex_col()
            .occlude()
            .bg(rgb(t.paper()))
            .border_1()
            .border_color(rgb(t.frame()))
            .rounded(px(8.0))
            .track_focus(&self.license_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                this.handle_license_key(event, cx);
            }))
            .child(head)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .px_4()
                    .py_4()
                    .child(body),
            );

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
                    this.show_license = false;
                    cx.notify();
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    // 只点名要测的两个，不用 `use super::*`——`gpui::*` 里带进来的东西会
    // 让 `#[test]` 展开时撞上递归上限（rustc 老毛病，换显式导入即可）。
    use super::{glyph_ok, grouped};

    #[test]
    fn glyph_ok_rejects_the_four_confusable_characters() {
        // 字符表就是「这枚 key 里不会出现的字」：0 / 1 / I / O 一律不收，
        // 敲了也不进草稿，用户不会攒出一枚注定无效的 key。
        for ok in ['2', '9', 'A', 'Z', 'a', 'z'] {
            assert!(glyph_ok(ok), "「{ok}」该收");
        }
        for bad in ['0', '1', 'I', 'O', 'i', 'o', '-', ' ', '中'] {
            assert!(!glyph_ok(bad), "「{bad}」不该收");
        }
    }

    #[test]
    fn grouped_shows_four_per_chunk() {
        assert_eq!(grouped(""), "");
        assert_eq!(grouped("ABCD"), "ABCD");
        assert_eq!(grouped("ABCDE"), "ABCD-E");
        assert_eq!(grouped("23456789ABCD2345"), "2345-6789-ABCD-2345");
    }
}
