//! 笺校工作台（Collation Tool）—— 与阅读器完全解耦的全屏模态视图。
//!
//! 设计原则（见《技术可行性与商业化评估报告》）：
//!   ① `enum AppMode { Reader, Collation }` 顶层状态机：进入 Collation 时，
//!      `Root::render` 直接返回 `Entity<CollationWorkspace>`，**根本不构建**
//!      阅读器视图 → 对阅读界面零影响。
//!   ② 本模块只依赖 `jiandu-core` 与现有 `ime` 基础设施，**绝不 import reader 模块**。
//!   ③ 右侧编辑器用 GPUI 原生轻量元素 + 复用 `crate::ime::InputField` /
//!      `EntityInputHandler` 做中文输入（GPUI 0.2.2 没有内置 Editor）。
//!   ④ 竖排预览（下一步）会走「离屏栅格化 + `img` 贴图」，不在每帧重绘朱砂点。
//!
//! 本文件是 MVP 的** UI 骨架**：AppMode 切换 + 40/60 分屏 + 可编辑/可选择的
//! 原生编辑器 + 左图右文联动（骨架阶段：左图 mock 行带 + 右文点击段 → 左图
//! `ScrollHandle` 滚动到 N*BAND_H 并叠加半透明高亮矩形）。原图预栅格化位图
//! 真实接入（PDF 文本层 / OCR 坐标匹配）、竖排预览栅格化在后续提交中落地。

use crate::ime::{
    char_to_utf16, effective_replace_range, utf16_len, utf16_slice, utf16_to_char,
};
use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, rgba, Bounds, Context, EntityInputHandler, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, ParentElement, Render, ScrollHandle, SharedString, Styled, Window,
};
use std::ops::Range;
use std::path::PathBuf;

/// Pixels → f32：gpui 的 `Pixels` 字段在 jiandu crate 不可见，用「除 1px」取数值。
fn px_f32(p: Pixels) -> f32 {
    p / px(1.0)
}

/// 顶层应用模式：阅读器 / 笺校工作台。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppMode {
    Reader,
    Collation,
}

/// 笺校工作台发回 Root 的事件（用于「返回阅读」）。
#[derive(Clone, Copy)]
pub enum CollationEvent {
    Exit,
}

// ── 视觉常量（骨架阶段，先写死；接入主题系统前不改）──
const HEADER_H: f32 = 44.0;
const PAD: f32 = 14.0;
const FONT_SIZE: f32 = 17.0;
const LINE_H: f32 = FONT_SIZE * 1.7;
/// 近似字宽：原图转录以 CJK 为主，按字号取值做点击定位（精确映射在下一步替换）。
const CHAR_W: f32 = FONT_SIZE * 0.95;
/// 左图 mock 行高（骨架阶段：右文第 N 段 → 左图 Y = N * BAND_H）。
const BAND_H: f32 = 100.0;

/// 左侧高亮矩形（内容坐标系，随原图一起滚动）。
#[derive(Clone, Copy)]
struct Highlight {
    y: f32,
    h: f32,
}

/// 笺校工作台视图：拥有自己的编辑缓冲、焦点、滚动柄，**不引用 Reader 任何字段**。
pub struct CollationWorkspace {
    /// 转录文本（横排，逐字对应左侧原图的一行/一列）。
    text: String,
    /// 字符下标光标（插入点）。
    cursor: usize,
    /// 选区（字符下标；start==end 表示无选区）。
    selection: Option<Range<usize>>,
    /// 输入法组字中的标记区间（UTF-16，与 `ime` 约定一致）。
    marked: Option<Range<usize>>,
    /// 编辑器焦点句柄：输入框靠它把中文送回 `EntityInputHandler`。
    focus: FocusHandle,
    /// 编辑器滚动柄。
    scroll: ScrollHandle,
    /// 左侧原图滚动柄（用于右文点击联动滚动）。
    left_scroll: ScrollHandle,
    /// 鼠标拖拽选择中的锚点（字符下标）。
    anchor: usize,
    /// 是否正在拖拽选择。
    dragging: bool,
    /// 竖排预览开关（骨架阶段只切占位浮层；真实栅格化在下一步）。
    show_preview: bool,
    /// 预栅格化的原图位图（PDF 页 / PNG）；None 时左图用 mock 行带占位。
    image_path: Option<PathBuf>,
    /// 左图高亮矩形（内容坐标，随原图滚动）。None = 无高亮。
    highlight: Option<Highlight>,
    /// 当前联动的右文段索引（0-based）。None = 尚未联动。
    active_para_idx: Option<usize>,
    /// 当前联动段提取出的纯文本（为后续真实坐标匹配预留）。
    active_para_text: String,
}

impl CollationWorkspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            // 示例转录：让用户一进来就能看到编辑器并立即尝试选择/输入。
            text: "天地玄黃 宇宙洪荒\n日月盈昃 辰宿列張\n寒來暑往 秋收冬藏\n閏餘成歲 律呂調陽\n雲騰致雨 露結為霜"
                .to_string(),
            cursor: 0,
            selection: None,
            marked: None,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            left_scroll: ScrollHandle::new(),
            anchor: 0,
            dragging: false,
            show_preview: false,
            image_path: None,
            highlight: None,
            active_para_idx: None,
            active_para_text: String::new(),
        }
    }

    // ── 选区 / 光标 / 编辑 ───────────────────────────────────────────────

    /// `(start, end)` 每行字符区间（不含换行）。
    fn line_ranges(&self) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        let mut start = 0usize;
        for line in self.text.split('\n') {
            let c = line.chars().count();
            v.push((start, start + c));
            start += c + 1;
        }
        v
    }

    fn line_of(&self, char_idx: usize) -> Option<usize> {
        self.line_ranges()
            .iter()
            .position(|&(s, e)| char_idx >= s && char_idx <= e)
    }

    fn char_up(&self) -> usize {
        let ranges = self.line_ranges();
        let idx = match self.line_of(self.cursor) {
            Some(i) => i,
            None => return self.cursor,
        };
        if idx == 0 {
            return self.cursor;
        }
        let col = self.cursor - ranges[idx].0;
        let (ps, pe) = ranges[idx - 1];
        (ps + col.min(pe - ps)).min(pe)
    }

    fn char_down(&self) -> usize {
        let ranges = self.line_ranges();
        let idx = match self.line_of(self.cursor) {
            Some(i) => i,
            None => return self.cursor,
        };
        if idx + 1 >= ranges.len() {
            return self.cursor;
        }
        let col = self.cursor - ranges[idx].0;
        let (ns, ne) = ranges[idx + 1];
        (ns + col.min(ne - ns)).min(ne)
    }

    fn line_home(&mut self, shift: bool, cx: &mut Context<Self>) {
        if let Some(idx) = self.line_of(self.cursor) {
            let (s, _) = self.line_ranges()[idx];
            self.move_cursor_to(s, shift, cx);
        }
    }

    fn line_end(&mut self, shift: bool, cx: &mut Context<Self>) {
        if let Some(idx) = self.line_of(self.cursor) {
            let (_, e) = self.line_ranges()[idx];
            self.move_cursor_to(e, shift, cx);
        }
    }

    /// 把光标移到 `to`；`shift` 时以锚点扩展选区。
    fn move_cursor_to(&mut self, to: usize, shift: bool, cx: &mut Context<Self>) {
        if shift {
            let anchor = match self.selection.as_ref() {
                Some(sel) => {
                    let (s, e) = (sel.start.min(sel.end), sel.end.max(sel.end));
                    if self.cursor <= s {
                        e
                    } else {
                        s
                    }
                }
                None => self.cursor,
            };
            let (s, e) = (anchor.min(to), anchor.max(to));
            self.selection = if s == e { None } else { Some(s..e) };
        } else if self.selection.is_some() {
            // 无 shift 时先收拢选区到光标，不移动（与系统文本框一致）。
            self.selection = None;
            cx.notify();
            return;
        }
        self.cursor = to;
        cx.notify();
    }

    fn move_cursor(&mut self, dir: &str, shift: bool, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            // 组字中：方向键交给输入法，不要打断组字。
            return;
        }
        let new = match dir {
            "left" => self.cursor.saturating_sub(1),
            "right" => (self.cursor + 1).min(self.text.chars().count()),
            "up" => self.char_up(),
            "down" => self.char_down(),
            _ => self.cursor,
        };
        self.move_cursor_to(new, shift, cx);
    }

    fn delete_char(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        if let Some(sel) = self.selection.as_ref() {
            let (s, e) = (sel.start.min(sel.end), sel.end.max(sel.end));
            let chars: Vec<char> = self.text.chars().collect();
            let kept: String = chars[..s].iter().chain(&chars[e..]).collect();
            self.text = kept;
            self.cursor = s;
            self.selection = None;
            cx.notify();
            return;
        }
        let total = self.text.chars().count();
        if forward {
            if self.cursor < total {
                let chars: Vec<char> = self.text.chars().collect();
                self.text = chars[..self.cursor].iter().chain(&chars[self.cursor + 1..]).collect();
                cx.notify();
            }
        } else if self.cursor > 0 {
            let chars: Vec<char> = self.text.chars().collect();
            self.text = chars[..self.cursor - 1].iter().chain(&chars[self.cursor..]).collect();
            self.cursor -= 1;
            cx.notify();
        }
    }

    /// 在光标处（或替换选区）插入文本。
    fn insert_text(&mut self, s: &str, cx: &mut Context<Self>) {
        let (a, b) = match self.selection.as_ref() {
            Some(sel) => (sel.start.min(sel.end), sel.end.max(sel.end)),
            None => (self.cursor, self.cursor),
        };
        let mut chars: Vec<char> = self.text.chars().collect();
        chars.splice(a..b, s.chars());
        self.text = chars.into_iter().collect();
        self.cursor = (a + s.chars().count()).min(self.text.chars().count());
        self.selection = None;
        cx.notify();
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            // 组字中：交给输入法，回填由 `EntityInputHandler` 负责。
            return;
        }
        let k = event.keystroke.key.as_str();
        let shift = event.keystroke.modifiers.shift;
        match k {
            "left" | "right" | "up" | "down" => self.move_cursor(k, shift, cx),
            "backspace" => self.delete_char(false, cx),
            "delete" => self.delete_char(true, cx),
            "home" => self.line_home(shift, cx),
            "end" => self.line_end(shift, cx),
            "enter" | "return" => self.insert_text("\n", cx),
            "escape" => {
                self.selection = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// 由点击/拖拽位置（窗口坐标）近似推出字符下标。
    ///
    /// 骨架阶段用固定布局常量 + 平均字宽估算；**精确像素映射（对齐原图几何）
    /// 在「选中联动高亮」提交中替换**。
    fn caret_from_point(&self, x: f32, y: f32, vw: f32) -> usize {
        let editor_left = vw * 0.4 + PAD;
        let editor_top = HEADER_H + PAD;
        let local_x = (x - editor_left).max(0.0);
        let local_y = (y - editor_top).max(0.0);
        let line_idx = (local_y / LINE_H).floor() as usize;
        let ranges = self.line_ranges();
        let idx = line_idx.min(ranges.len().saturating_sub(1));
        let (s, e) = match ranges.get(idx) {
            Some(&(s, e)) => (s, e),
            None => (0, self.text.chars().count()),
        };
        let in_line = (local_x / CHAR_W).floor() as usize;
        s + in_line.min(e - s)
    }

    fn place_caret_from_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vw = px_f32(window.viewport_size().width);
        self.cursor = self.caret_from_point(px_f32(event.position.x), px_f32(event.position.y), vw);
        self.selection = None;
        self.anchor = self.cursor;
        self.dragging = true;
        window.focus(&self.focus);
        // 落光标即联动：右文点击第 N 段 → 左图滚动 + 高亮。
        if let Some(li) = self.line_of(self.cursor) {
            self.link_to_paragraph(li, cx);
        } else {
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn extend_selection_from_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging {
            return;
        }
        let vw = px_f32(window.viewport_size().width);
        let to = self.caret_from_point(px_f32(event.position.x), px_f32(event.position.y), vw);
        let (s, e) = (self.anchor.min(to), self.anchor.max(to));
        self.cursor = to;
        self.selection = if s == e { None } else { Some(s..e) };
        cx.notify();
    }

    // ── 渲染 ─────────────────────────────────────────────────────────────

    /// 通用小按钮：承载 `on_click`（点下即触发 + 阻止冒泡）。
    fn button(
        &self,
        label: &'static str,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .px(px(12.0))
            .py(px(5.0))
            .bg(rgba(0x8c2b221f))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_color(rgb(0x8c2b22))
            .child(SharedString::from(label))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, event, window, cx| on_click(c, event, window, cx)),
            )
    }

    /// 顶部返回栏。
    fn render_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .h(px(HEADER_H))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .bg(rgb(0xece2c8))
            .px(px(PAD))
            .child(
                div()
                    .text_color(rgb(0x2b2620))
                    .child(SharedString::from("笺校工作台")),
            )
            .child(div().flex_1())
            .child(self.button("返回阅读", |_c, _, _, cx| {
                cx.emit(CollationEvent::Exit);
            }, cx))
    }

    /// 左侧 40%：原图区。滚动容器挂 `left_scroll`，内容随窗口伸缩；
    /// 内容（栅格化位图或 mock 行带）内部用 `relative()` 承载绝对定位的高亮叠加层。
    fn render_left(&self, vw: f32, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .w(px(vw * 0.4))
            .h_full()
            .relative()
            .bg(rgb(0xddd2b4))
            .child(
                div()
                    .id(SharedString::from("collation-left-scroll"))
                    .w_full()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.left_scroll)
                    .child(self.render_left_content()),
            )
            .child(
                self.button("导入原图", |c, _, _, cx| c.open_image_picker(cx), cx)
                    .absolute()
                    .top(px(8.0))
                    .right(px(8.0)),
            )
    }

    /// 左图内容：已载入位图则贴 `img`，否则画 mock 行带（每段一行，便于演示联动）。
    /// 高亮矩形作为绝对定位子节点叠加在 `page` 上，随内容一起滚动。
    fn render_left_content(&self) -> gpui::Div {
        let h = self.page_height();
        let mut page = div().relative().w_full().h(px(h)).bg(rgb(0xf1e8cf));
        if let Some(p) = &self.image_path {
            page = page.child(img(p.clone()).w_full().h(px(h)));
        } else {
            let n = self.line_ranges().len().max(1);
            for i in 0..n {
                page = page.child(self.render_band(i));
            }
        }
        if let Some(hl) = &self.highlight {
            page = page.child(
                div()
                    .absolute()
                    .top(px(hl.y))
                    .left(px(0.0))
                    .right(px(0.0))
                    .h(px(hl.h))
                    .bg(rgba(0x8c2b2244))
                    .border_1()
                    .border_color(rgba(0x8c2b22cc)),
            );
        }
        page
    }

    /// mock 行带：左图第 i 行，直接显示右侧第 i 段的真实文本（自证式演示联动）。
    /// 真实接入预栅格化位图后，本行带被 `img` 取代，段文本转为坐标匹配依据。
    fn render_band(&self, i: usize) -> gpui::Div {
        let bg = if i % 2 == 0 { rgb(0xe9dfc4) } else { rgb(0xf1e8cf) };
        let text = self
            .line_ranges()
            .get(i)
            .map(|&(s, e)| {
                self.text
                    .chars()
                    .skip(s)
                    .take(e.saturating_sub(s))
                    .collect::<String>()
            })
            .unwrap_or_default();
        div()
            .h(px(BAND_H))
            .w_full()
            .flex()
            .items_center()
            .px(px(PAD))
            .bg(bg)
            .child(SharedString::from(format!(
                "左图第 {} 行 ｜ 右文：{}",
                i + 1,
                text
            )))
    }

    /// 左图内容高度（骨架 mock = 段数 * BAND_H，至少 12 行保证可滚动）。
    fn page_height(&self) -> f32 {
        (self.line_ranges().len().max(12)) as f32 * BAND_H
    }

    /// 导入原图（PDF/PNG）：下一步接入 rfd 文件选择 + 预栅格化位图；
    /// 这里仅占位，避免死按钮。
    fn open_image_picker(&mut self, cx: &mut Context<Self>) {
        // TODO(collation): 用 rfd 打开文件 → 预栅格化 PDF 页为 RenderImage → 存入 image_path。
        cx.notify();
    }

    /// 右文某段 → 左图联动：记录段文本 + 左侧滚动到对应 Y + 高亮。
    ///
    /// 骨架阶段用 mock 映射：右文第 `para_idx` 段 → 左图 Y = para_idx * BAND_H。
    /// 真实坐标（PDF 文本层 / OCR）在后续提交替换此函数体。
    fn link_to_paragraph(&mut self, para_idx: usize, cx: &mut Context<Self>) {
        let y = para_idx as f32 * BAND_H;
        self.highlight = Some(Highlight { y, h: BAND_H });
        self.active_para_idx = Some(para_idx);
        // 提取该段纯文本（为后续真实坐标匹配预留数据）。
        if let Some(&(s, e)) = self.line_ranges().get(para_idx) {
            self.active_para_text = self.text.chars().skip(s).take(e.saturating_sub(s)).collect();
        }
        // 驱动左侧 ScrollHandle 滚动（向下偏移取负）。
        self.left_scroll.set_offset(Point::new(px(0.0), px(-y)));
        cx.notify();
    }

    /// 右侧联动状态条：显示当前已联动的段与左图目标 Y。
    fn render_link_status(&self) -> gpui::Div {
        match self.active_para_idx {
            Some(idx) => div()
                .h(px(26.0))
                .flex()
                .items_center()
                .px(px(PAD))
                .bg(rgba(0x8c2b2214))
                .text_color(rgb(0x8c2b22))
                .child(SharedString::from(format!(
                    "已联动：右文第 {} 段 → 左侧 Y={:.0}px 高亮",
                    idx + 1,
                    idx as f32 * BAND_H
                ))),
            None => div().h(px(0.0)),
        }
    }

    /// 右侧 60%：竖排预览工具条 + 原生编辑器。
    fn render_right(&self, vw: f32, cx: &mut Context<Self>) -> gpui::Div {
        let editor = self.render_editor(cx);
        div()
            .w(px(vw * 0.6))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0xf4ecd8))
            .child(
                div()
                    .h(px(HEADER_H))
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(PAD))
                    .bg(rgb(0xece2c8))
                    .child(div().flex_1())
                    .child(self.button("竖排预览", |c, _, _, cx| {
                        c.show_preview = !c.show_preview;
                        cx.notify();
                    }, cx)),
            )
            .child(self.render_link_status())
            .child(editor)
    }

    /// 渲染单行（含选区背景与光标）。
    fn render_line(&self, line: &str, line_start: usize) -> gpui::Div {
        let line_end = line_start + line.chars().count();
        let caret_here = self.cursor >= line_start && self.cursor <= line_end;
        let (s, e) = match self.selection.as_ref() {
            Some(r) => (r.start.min(r.end), r.end.max(r.end)),
            None => (self.cursor, self.cursor),
        };
        let sel_active = self.selection.as_ref().map_or(false, |r| r.start != r.end);
        let seg_start = line_start.max(s);
        let seg_end = line_end.min(e);
        let overlaps = sel_active && seg_end > seg_start;

        let mut row = div().h(px(LINE_H)).flex().flex_row().items_center();
        if overlaps {
            let before: String = self
                .text
                .chars()
                .skip(line_start)
                .take(seg_start.saturating_sub(line_start))
                .collect();
            if !before.is_empty() {
                row = row.child(div().child(SharedString::from(before)));
            }
            if self.cursor == seg_start {
                row = row.child(self.caret());
            }
            let mid: String = self
                .text
                .chars()
                .skip(seg_start)
                .take(seg_end - seg_start)
                .collect();
            row = row.child(
                div()
                    .bg(rgba(0x8c2b2238))
                    .px(px(1.0))
                    .child(SharedString::from(mid)),
            );
            if self.cursor == seg_end {
                row = row.child(self.caret());
            }
            let after: String = self
                .text
                .chars()
                .skip(seg_end)
                .take(line_end - seg_end)
                .collect();
            if !after.is_empty() {
                row = row.child(div().child(SharedString::from(after)));
            }
        } else if caret_here {
            let before: String = self
                .text
                .chars()
                .skip(line_start)
                .take(self.cursor.saturating_sub(line_start))
                .collect();
            if !before.is_empty() {
                row = row.child(div().child(SharedString::from(before)));
            }
            row = row.child(self.caret());
            let after: String = self
                .text
                .chars()
                .skip(self.cursor)
                .take(line_end.saturating_sub(self.cursor))
                .collect();
            if !after.is_empty() {
                row = row.child(div().child(SharedString::from(after)));
            }
        } else {
            row = row.child(div().child(SharedString::from(line.to_string())));
        }
        row
    }

    fn caret(&self) -> gpui::Div {
        div().w(px(2.0)).h(px(FONT_SIZE * 1.1)).bg(rgb(0x8c2b22))
    }

    /// 原生编辑器：滚动容器 + `InputField`（注册焦点 / IME）+ 逐行文本。
    ///
    /// 注：GPUI 0.2.2 无内置 Editor，这里用自绘文本 + `crate::ime::InputField`
    /// 注册焦点，中文输入经 `EntityInputHandler` 落回 `self.text`。
    fn render_editor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut content = div().flex().flex_col();
        let mut char_start = 0usize;
        for line in self.text.split('\n') {
            let lc = line.chars().count();
            content = content.child(self.render_line(line, char_start));
            char_start += lc + 1;
        }
        // 用 `InputField` 包裹，使其在获得焦点时把 IME 回传中文接住。
        let editor = crate::ime::InputField::new(
            div()
                .id(SharedString::from("collation-editor-scroll"))
                .flex_1()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .bg(rgb(0xf4ecd8))
                .px(px(PAD))
                .py(px(PAD))
                .text_size(px(FONT_SIZE))
                .text_color(rgb(0x2b2620))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|c, event, window, cx| c.place_caret_from_click(event, window, cx)),
                )
                .on_mouse_move(
                    cx.listener(|c, event, window, cx| c.extend_selection_from_drag(event, window, cx)),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|c, _, _, cx| {
                        c.dragging = false;
                        cx.notify();
                    }),
                )
                .on_key_down(cx.listener(|c, event, _window, cx| c.handle_key(event, cx)))
                .child(content),
            self.focus.clone(),
            cx.entity(),
        );
        div().flex_1().child(editor)
    }

    /// 竖排预览占位浮层（真实栅格化在下一步提交）。
    fn render_preview_overlay(&self) -> gpui::Div {
        div()
            .absolute()
            .top(px(HEADER_H))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgba(0x00000059))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .px(px(20.0))
                    .py(px(16.0))
                    .bg(rgb(0xf4ecd8))
                    .rounded(px(8.0))
                    .text_color(rgb(0x2b2620))
                    .child(SharedString::from(
                        "竖排预览（42° 朱砂点 · 离屏栅格化贴图）— 下一步接入",
                    )),
            )
    }
}

impl EventEmitter<CollationEvent> for CollationWorkspace {}

impl Render for CollationWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vw = px_f32(window.viewport_size().width);
        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf4ecd8))
            .child(self.render_header(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(self.render_left(vw, cx))
                    .child(self.render_right(vw, cx)),
            );
        if self.show_preview {
            root = root.child(self.render_preview_overlay());
        }
        root
    }
}

// ── 中文输入：复用现有 `ime` 的 UTF-16 约定，绝不叠字 ──────────────────────

impl EntityInputHandler for CollationWorkspace {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        *adjusted_range = Some(range.clone());
        Some(utf16_slice(&self.text, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        let (s, e) = match self.selection.as_ref() {
            Some(r) => (r.start.min(r.end), r.end.max(r.end)),
            None => {
                let a = char_to_utf16(&self.text, self.cursor);
                (a, a)
            }
        };
        Some(gpui::UTF16Selection {
            range: s..e,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    /// 候选词面板的锚定位置：本编辑器全区域，直接返回元素边界即可。
    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    /// 点选定位：本编辑器用鼠标点击落光标（见 `place_caret_from_click`），
    /// 不依赖输入法按点反查字符下标，故返回 None。
    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let eff = effective_replace_range(range, &self.marked);
        self.marked = None;
        self.apply_replace_utf16(eff, text);
        self.selection = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let eff = effective_replace_range(range, &self.marked);
        self.marked = None;
        let start = self.apply_replace_utf16(eff, new_text);
        let end = start + utf16_len(new_text);
        self.marked = Some(start..end);
        self.cursor = match new_selected_range {
            Some(sel) => utf16_to_char(&self.text, sel.end),
            None => utf16_to_char(&self.text, end),
        };
        self.selection = None;
        cx.notify();
    }
}

impl CollationWorkspace {
    /// 在 `range`（UTF-16；None = 光标处）替换为 `text`，返回替换处的 UTF-16 起点。
    fn apply_replace_utf16(&mut self, range: Option<Range<usize>>, text: &str) -> usize {
        let (a, b) = match range {
            Some(r) => (
                utf16_to_char(&self.text, r.start),
                utf16_to_char(&self.text, r.end),
            ),
            None => {
                let c = self.cursor.min(self.text.chars().count());
                (c, c)
            }
        };
        let total = self.text.chars().count();
        let a = a.min(total);
        let b = b.clamp(a, total);
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        let mut chars: Vec<char> = self.text.chars().collect();
        chars.splice(a..b, clean.chars());
        self.text = chars.into_iter().collect();
        self.cursor = (a + clean.chars().count()).min(self.text.chars().count());
        char_to_utf16(&self.text, a)
    }
}
