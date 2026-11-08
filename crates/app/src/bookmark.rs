//! 右缘**竹简书签**（v2.0 门面件，独立小模块）。
//!
//! 只有一件事：在沉浸阅读态，右缘挂一根书签——
//!   · 宽度**严格等于正文竖排网格的单列宽** `col_w`，不设任何固定像素；
//!   · 朱砂书签头 + 竖排章节名，纯黑墨色，不叠透明度；
//!   · 默认半隐藏，鼠标悬停 240ms 平滑滑出（位移 + 透明度渐变）。
//!
//! ⚠️ 几何要点（这里踩过坑，别再改回去）：静止态**露出**的宽度是 `peek`，
//! 所以文字层宽度必须也是 `peek`。早先文字层按整条宽度 `w` 居中，半隐藏时
//! 每个字都被窗口右缘切掉一半，看起来像「半截字」。
//!
//! ⚠️ 外层容器**绝不能** `.occlude()`：GPUI 的 occlude = `BlockMouse` 命中盒，
//! 全屏容器一旦 occlude，正文的点击 / 选字 / 双击编辑会被整片吞掉。

use crate::app::Root;
use crate::app::SideTab;
use gpui::{
    div, prelude::*, px, rgb, rgba, Animation, AnimationExt, BoxShadow, Context, Div, MouseButton,
    MouseDownEvent, SharedString,
};
use std::time::Duration;

/// 竖排行高系数：与阅读器字格同源（1.18em）。
pub const LINE_FACTOR: f32 = 1.18;

const INK: u32 = 0x1A1A1A;
const ZHU: u32 = 0xB0342C;
const HAIR_INK: u32 = 0x0000001A;

/// 竹简书签宽度：**严格等于**正文竖排网格的单列宽度，不设固定像素。
pub fn bookmark_width(body_col_w: f32) -> f32 {
    body_col_w.max(1.0)
}

impl Root {
    /// **右缘竹简书签**：宽度严格 = 正文单列宽 `col_w`，朱砂点缀，
    /// 默认半隐藏在右侧边缘，悬停平滑滑出（240ms 位移 + 透明度渐变）。
    pub(crate) fn render_bamboo_bookmark(&mut self, cx: &mut Context<Self>) -> Div {
        let vh = self.viewport_h.max(320.0);
        let w = bookmark_width(if self.col_w.is_finite() && self.col_w > 2.0 {
            self.col_w
        } else {
            28.0
        });
        // 半隐藏的比例：露出 62%，且至少 15px —— 露出太窄会把字切掉半个，
        // 反而像「没画完」。字号跟着露出宽度收，保证静止态就是一列完整的字。
        let peek = (w * 0.62).max(15.0).min(w);
        let h = (vh * 0.42).clamp(180.0, 340.0);
        let top = (vh - h) * 0.5;
        let hover = self.bookmark_hover;
        let hidden = (w - peek).max(0.0);
        let label = self
            .book
            .as_ref()
            .and_then(|b| b.chapters.get(self.current_chapter))
            .map(|c| c.title.clone())
            .unwrap_or_else(|| "書籤".to_string());
        let chars: Vec<char> = label.chars().take(8).collect();

        let mut strip = div()
            .id("bamboo-bookmark")
            .absolute()
            .top(px(top))
            .right(px(0.0))
            .w(px(w))
            .h(px(h))
            .rounded(px(6.0))
            .bg(rgba(0xF2F1EDFA))
            .border_l_1()
            .border_color(rgba(HAIR_INK))
            .shadow(vec![BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.12),
                offset: gpui::point(px(0.0), px(4.0)),
                blur_radius: px(16.0),
                spread_radius: px(0.0),
            }])
            .occlude()
            .cursor_pointer()
            .on_hover(cx.listener(|this, on: &bool, _window, cx| {
                if this.bookmark_hover != *on {
                    this.bookmark_hover = *on;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.side_tab = SideTab::Bookmark;
                    this.toggle_side();
                    cx.notify();
                }),
            )
            // 朱砂书签头：压在顶端那道朱线。
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(w))
                    .h(px(3.0))
                    .bg(rgb(ZHU)),
            );
        // 文字层宽度 = 露出宽度：静止态这一列字完整落在可见区域里，
        // 滑出后也不会突然「换位置」，只是右边多露出一段竹面。
        let text_w = peek;
        let fs = (text_w * 0.78).clamp(10.0, 18.0);
        for (i, c) in chars.iter().enumerate() {
            strip = strip.child(
                div()
                    .absolute()
                    .top(px(14.0 + i as f32 * fs * LINE_FACTOR))
                    .left(px(0.0))
                    .w(px(text_w))
                    .h(px(fs * LINE_FACTOR))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(fs))
                    .text_color(rgb(INK))
                    .child(SharedString::from(c.to_string())),
            );
        }
        strip = strip.child(
            div()
                .absolute()
                .bottom(px(10.0))
                .left(px(0.0))
                .w(px(text_w))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .text_color(rgb(ZHU))
                .child(SharedString::from("●")),
        );

        // 显隐两态用两个不同 id：GPUI 0.2.2 的 with_animation 只认 id 变化。
        let anim_id = SharedString::from(if hover {
            "bamboo-bookmark-in"
        } else {
            "bamboo-bookmark-out"
        });
        let animated = strip.with_animation(
            anim_id,
            Animation::new(Duration::from_millis(240)),
            move |el, delta| {
                let v = if hover { delta } else { 1.0 - delta };
                el.right(px(-hidden * (1.0 - v)))
                    .opacity(0.55 + 0.45 * v)
            },
        );
        // ⚠️ 外层容器绝不 occlude：全屏命中盒会吞掉正文的点击与选字。
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .child(animated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_width_is_exactly_the_body_column_width() {
        for w in [18.0, 28.0, 37.5, 64.0] {
            assert_eq!(bookmark_width(w), w);
        }
    }
}
