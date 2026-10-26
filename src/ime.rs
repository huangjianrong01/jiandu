//! IME（输入法）支持：让自绘输入框能打中文。
//!
//! GPUI 0.2.2 没有内置文本框元素。中文输入法**不走 keydown**——它先把
//! 拼音组字，再经 `InputHandler`（macOS 的 NSTextInputClient）把组好的
//! 字整段送回应用。没有 InputHandler 时，组好的字无处可去：表现就是
//! 「重命名时打不进中文，退格也像失灵」。
//!
//! 本模块做三件事：
//!   ① `InputField`：把「草稿串 + 字符光标」的自绘输入框包成可聚焦元素，
//!      paint 阶段注册 `handle_input`，焦点落在本元素时平台才把 IME 送进来；
//!   ② `impl EntityInputHandler for Root`：把输入法回传的文字写进各草稿；
//!   ③ 组字区间的正解（`replace_and_mark_text_in_range` /
//!      `replace_text_in_range`）：输入法传 NSNotFound（GPUI 转成 `None`）时，
//!      要替换的是**当前标记区间**，没有标记区间才落在光标处。
//!
//! 关于文字输入的总架构——**可打印字符一律不 keydown 直插**：
//! GPUI 0.2.2 的派发顺序是「先 keydown 回调、后送输入法上下文」，而
//! inputContext 对**所有**输入源（ABC / 拼音 / 双拼 / Rime）都会回调
//! `insertText:` / `setMarkedText:`（实测 ABC 下每个可打印键都触发
//! insertText）。所以 keydown 直插与 InputHandler 回落必叠字——
//! 修复前实测：Squirrel 小鹤双拼敲 `nihao`，草稿得到
//! `nnnnnn n i h a o haoh`（21 字而非「你好」）；ABC 敲 `qwe`
//! 得 `qqwwee`。修法：文字只走 InputHandler 一条路（与 NSTextField
//! 架构一致），keydown 只处理编辑键（Esc / ⏎ / ← → / Home / End /
//! ⌘V / 未组字时的退格）——见 `reader::handle_reading_key` 与
//! `shelf::handle_shelf_key`。

use crate::app::Root;
use gpui::{
    div, App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Point,
    SharedString, UTF16Selection, Window,
};
use std::ops::Range;

/// 书名上限（与导入时自动识别的 12 字一致）。
const TITLE_MAX: usize = 12;

// ---- UTF-16 ↔ 字符下标换算（IME 接口一律按 UTF-16 计位）----

/// 字符下标 → UTF-16 下标。
pub fn char_to_utf16(s: &str, char_idx: usize) -> usize {
    s.chars().take(char_idx).map(|c| c.len_utf16()).sum()
}

/// UTF-16 下标 → 字符下标（越界夹到串尾，避免切进代理对中间）。
pub fn utf16_to_char(s: &str, u16_idx: usize) -> usize {
    let mut acc = 0usize;
    for (i, c) in s.chars().enumerate() {
        if acc >= u16_idx {
            return i;
        }
        acc += c.len_utf16();
    }
    s.chars().count()
}

/// 取 [start, end) 这段 UTF-16 区间对应的子串。
pub fn utf16_slice(s: &str, range: Range<usize>) -> String {
    let a = utf16_to_char(s, range.start);
    let b = utf16_to_char(s, range.end);
    s.chars().skip(a).take(b.saturating_sub(a)).collect()
}

/// 一段文字的 UTF-16 长度。
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

/// 检索框字数上限：够写下「长安」这类词，也够一句短语。
pub const SEARCH_MAX: usize = 64;

/// 笔记字数上限：一段批注的合理长度（与 keydown 路径的 500 字口径一致）。
pub const NOTE_MAX: usize = 500;

/// 输入法替换区间的正解：`range` 为 `None`（macOS 传 NSNotFound，
/// GPUI 转成 None）时，替换目标是**当前标记区间**；没有标记区间
/// 才落在光标处。
///
/// 这条语义是叠字事故的另一半：组字期间输入法每步都传 NSNotFound，
/// 若按「插到光标」处理，第二步就会把新拼音叠在旧拼音后面——
/// 实测 Squirrel 双拼敲 `nihao`，草稿滚成 `nninih…`。
pub fn effective_replace_range(
    range: Option<Range<usize>>,
    marked: &Option<Range<usize>>,
) -> Option<Range<usize>> {
    range.or_else(|| marked.clone())
}

/// 把草稿 `draft` 里 `range`（UTF-16 区间；None = 当前光标处，零长度）
/// 替换成 `text`，并把光标 `cursor` 落到插入内容之后；返回替换处的
/// UTF-16 下标。
///
/// 书名按 `TITLE_MAX` 截断，超出的部分丢弃——与 keydown 路径的
/// 「12 字上限」保持同一口径。抽成纯函数是为了能直接单测
/// （构造整个 `Root` 需要 App 上下文）。
pub fn replace_utf16(
    draft: &mut String,
    cursor: &mut usize,
    range: Option<Range<usize>>,
    text: &str,
) -> usize {
    replace_utf16_limited(draft, cursor, range, text, TITLE_MAX)
}

/// `replace_utf16` 的通用版：上限 `max` 由调用方定
/// （书名 12 字、检索框 `SEARCH_MAX` 字）。
pub fn replace_utf16_limited(
    draft: &mut String,
    cursor: &mut usize,
    range: Option<Range<usize>>,
    text: &str,
    max: usize,
) -> usize {
    let (a, b) = match range {
        Some(r) => (utf16_to_char(draft, r.start), utf16_to_char(draft, r.end)),
        None => {
            let c = (*cursor).min(draft.chars().count());
            (c, c)
        }
    };
    let total = draft.chars().count();
    let a = a.min(total);
    let b = b.clamp(a, total);
    // 滤掉全部控制字符（IME 偶尔会带 \r / \n / \t 之类），只留可排版的字。
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    // 删掉 [a,b) 后剩余的长度，据此算还能插几个字。
    let remain = total - (b - a);
    let room = max.saturating_sub(remain);
    let ins: Vec<char> = clean.chars().take(room).collect();
    let mut chars: Vec<char> = draft.chars().collect();
    chars.splice(a..b, ins.iter().copied());
    *draft = chars.into_iter().collect();
    *cursor = (a + ins.len()).min(draft.chars().count());
    char_to_utf16(draft, a)
}

/// 当前正在接字的输入框。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputTarget {
    /// 书架上的书名重命名框。
    Rename,
    /// 阅读页的全文检索框。
    Search,
    /// 阅读页的浮层笔记框（划线 / 点字后直接就着正文写的那段批注）。
    Note,
    /// 书架的搜索框（只筛书名 / 作者，用的是 `shelf_query`）。
    ShelfSearch,
}

impl Root {
    /// 把重命名草稿里 `range`（UTF-16 区间；None = 当前光标处）替换成 `text`；
    /// 返回替换处的 UTF-16 下标。
    pub fn rename_replace_utf16(&mut self, range: Option<Range<usize>>, text: &str) -> usize {
        replace_utf16(
            &mut self.rename_draft,
            &mut self.rename_cursor,
            range,
            text,
        )
    }

    /// 检索框：同上，走 `SEARCH_MAX` 上限。
    pub fn search_replace_utf16(&mut self, range: Option<Range<usize>>, text: &str) -> usize {
        replace_utf16_limited(
            &mut self.search_draft,
            &mut self.search_cursor,
            range,
            text,
            SEARCH_MAX,
        )
    }

    /// 笔记框：同上，走 `NOTE_MAX` 上限。
    ///
    /// 笔记框原先**完全没接 IME**——只有重命名框和检索框接了，于是
    /// 「点笔记 → 切中文输入法 → 打不出汉字，退格也像失灵」。中文不走
    /// keydown，全靠 InputHandler 回传，漏一路就是一整个框不可用。
    pub fn note_replace_utf16(&mut self, range: Option<Range<usize>>, text: &str) -> usize {
        replace_utf16_limited(
            &mut self.note_draft,
            &mut self.note_cursor,
            range,
            text,
            NOTE_MAX,
        )
    }

    /// 此刻哪个输入框该接字：重命名框 → 笔记框 → 书架搜索 → 阅读页检索；
    /// 都没有则 None（IME 接口收到 None 即不接管，键盘走普通 keydown）。
    pub fn input_target(&self) -> Option<InputTarget> {
        if self.rename_idx.is_some() {
            Some(InputTarget::Rename)
        } else if self.note_editing {
            Some(InputTarget::Note)
        } else if self.shelf_searching && self.screen == crate::app::Screen::Shelf {
            Some(InputTarget::ShelfSearch)
        } else if self.show_search {
            Some(InputTarget::Search)
        } else {
            None
        }
    }

    /// 书架搜索框：同上，走 `SEARCH_MAX` 上限。
    pub fn shelf_query_replace_utf16(&mut self, range: Option<Range<usize>>, text: &str) -> usize {
        replace_utf16_limited(
            &mut self.shelf_query,
            &mut self.shelf_query_cursor,
            range,
            text,
            SEARCH_MAX,
        )
    }
}

// ---- 自绘输入框外壳 ----

/// 把任意子元素包成「可聚焦、可接 IME」的输入元素。
///
/// prepared 阶段登记焦点句柄（焦点才有落点，IME 才认这块区域），
/// paint 阶段注册 `handle_input`（平台把组好的字经它送回 `Root`）。
pub struct InputField {
    child: Option<gpui::AnyElement>,
    focus: FocusHandle,
    view: Entity<Root>,
}

impl InputField {
    /// `child` 为输入框的视觉内容（草稿 + 光标自绘），`focus` 为该框的焦点句柄。
    pub fn new(child: impl IntoElement, focus: FocusHandle, view: Entity<Root>) -> Self {
        Self {
            child: Some(child.into_any_element()),
            focus,
            view,
        }
    }
}

impl IntoElement for InputField {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for InputField {
    type RequestLayoutState = gpui::AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name(SharedString::from("jiandu-input-field")))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 布局缓存命中时本帧不会再走这里，兜一个空元素避免 panic。
        let mut child = self.child.take().unwrap_or_else(|| div().into_any_element());
        let layout_id = child.request_layout(window, cx);
        (layout_id, child)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // 登记焦点句柄：window.focus 之后，派发树上才有本元素的节点。
        window.set_focus_handle(&self.focus, cx);
        child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // 只有焦点落在本元素上，这个 handler 才会注册到平台——
        // IME 因此能把组好的中文送到 `Root` 的 replace_text_in_range。
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        child.paint(window, cx);
    }
}

// ---- Root 接成 InputHandler：中文经这里落进草稿 ----

impl EntityInputHandler for Root {
    /// 取指定 UTF-16 区间的文字（输入法用来读回上下文）。
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let target = self.input_target()?;
        // 整段都给：区间不再裁剪，回写原区间即可。
        *adjusted_range = Some(range.clone());
        Some(match target {
            InputTarget::Rename => utf16_slice(&self.rename_draft, range),
            InputTarget::Search => utf16_slice(&self.search_draft, range),
            InputTarget::Note => utf16_slice(&self.note_draft, range),
            InputTarget::ShelfSearch => utf16_slice(&self.shelf_query, range),
        })
    }

    /// 光标位置：一个零长度选区（本输入框不做选区，只插字）。
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let target = self.input_target()?;
        let at = match target {
            InputTarget::Rename => char_to_utf16(&self.rename_draft, self.rename_cursor),
            InputTarget::Search => char_to_utf16(&self.search_draft, self.search_cursor),
            InputTarget::Note => char_to_utf16(&self.note_draft, self.note_cursor),
            InputTarget::ShelfSearch => char_to_utf16(&self.shelf_query, self.shelf_query_cursor),
        };
        Some(UTF16Selection {
            range: at..at,
            reversed: false,
        })
    }

    /// 正在组字（未上屏）的那段文字；没有就 None。
    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let target = self.input_target()?;
        match target {
            InputTarget::Rename => self.rename_marked.clone(),
            InputTarget::Search => self.search_marked.clone(),
            InputTarget::Note => self.note_marked.clone(),
            InputTarget::ShelfSearch => self.shelf_query_marked.clone(),
        }
    }

    /// 组字结束 / 放弃组字：清掉标记区间。
    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        match self.input_target() {
            Some(InputTarget::Rename) => self.rename_marked = None,
            Some(InputTarget::Search) => self.search_marked = None,
            Some(InputTarget::Note) => self.note_marked = None,
            Some(InputTarget::ShelfSearch) => self.shelf_query_marked = None,
            None => {}
        }
    }

    /// 输入法把字「上屏」：替换区间（或光标处）并落定光标。
    ///
    /// 同 `replace_and_mark_text_in_range`：`range` 为 None（NSNotFound）时
    /// 替换当前标记区间——上屏的汉字应该顶掉未上屏的拼音串，
    /// 而不是插在它后面（否则草稿里拼音汉字并存）。
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.input_target() else {
            return;
        };
        if std::env::var("JIANDU_IME_DEBUG").is_ok() {
            eprintln!(
                "[ime-commit] target={:?} range={:?} text={:?}",
                target, range, text
            );
        }
        match target {
            InputTarget::Rename => {
                let eff = effective_replace_range(range, &self.rename_marked);
                self.rename_marked = None;
                self.rename_replace_utf16(eff, text);
            }
            InputTarget::Search => {
                let eff = effective_replace_range(range, &self.search_marked);
                self.search_marked = None;
                self.search_replace_utf16(eff, text);
            }
            InputTarget::Note => {
                let eff = effective_replace_range(range, &self.note_marked);
                self.note_marked = None;
                self.note_replace_utf16(eff, text);
            }
            InputTarget::ShelfSearch => {
                let eff = effective_replace_range(range, &self.shelf_query_marked);
                self.shelf_query_marked = None;
                self.shelf_query_replace_utf16(eff, text);
            }
        }
        cx.notify();
    }

    /// 输入法「组字中」：先把候选拼音写进草稿并标记区间，
    /// 用户确认后由 `replace_text_in_range` 替换成最终汉字。
    ///
    /// NSTextInputClient 语义：`range` 为 None 时（输入法传 NSNotFound），
    /// 要替换的是**当前标记区间**，没有标记区间才落在光标处——
    /// 否则第二次组字会把新拼音插在旧拼音后面，草稿滚成
    /// "nninih…"（实测 Squirrel 双拼敲 nihao，29 字而非「你好」）。
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.input_target() else {
            return;
        };
        if std::env::var("JIANDU_IME_DEBUG").is_ok() {
            eprintln!(
                "[ime-mark] target={:?} range={:?} marked={:?} text={:?}",
                target,
                range,
                match target {
                    InputTarget::Rename => &self.rename_marked,
                    InputTarget::Search => &self.search_marked,
                    InputTarget::Note => &self.note_marked,
                    InputTarget::ShelfSearch => &self.shelf_query_marked,
                },
                new_text
            );
        }
        match target {
            InputTarget::Rename => {
                let eff = effective_replace_range(range, &self.rename_marked);
                let start = self.rename_replace_utf16(eff, new_text);
                let end = start + utf16_len(new_text);
                self.rename_marked = Some(start..end);
                // 光标跟着输入法给的选区走；没给就落在标记文字末尾。
                self.rename_cursor = match new_selected_range {
                    Some(sel) => utf16_to_char(&self.rename_draft, sel.end),
                    None => utf16_to_char(&self.rename_draft, end),
                };
            }
            InputTarget::Search => {
                let eff = effective_replace_range(range, &self.search_marked);
                let start = self.search_replace_utf16(eff, new_text);
                let end = start + utf16_len(new_text);
                self.search_marked = Some(start..end);
                self.search_cursor = match new_selected_range {
                    Some(sel) => utf16_to_char(&self.search_draft, sel.end),
                    None => utf16_to_char(&self.search_draft, end),
                };
            }
            InputTarget::Note => {
                let eff = effective_replace_range(range, &self.note_marked);
                let start = self.note_replace_utf16(eff, new_text);
                let end = start + utf16_len(new_text);
                self.note_marked = Some(start..end);
                self.note_cursor = match new_selected_range {
                    Some(sel) => utf16_to_char(&self.note_draft, sel.end),
                    None => utf16_to_char(&self.note_draft, end),
                };
            }
            InputTarget::ShelfSearch => {
                let eff = effective_replace_range(range, &self.shelf_query_marked);
                let start = self.shelf_query_replace_utf16(eff, new_text);
                let end = start + utf16_len(new_text);
                self.shelf_query_marked = Some(start..end);
                self.shelf_query_cursor = match new_selected_range {
                    Some(sel) => utf16_to_char(&self.shelf_query, sel.end),
                    None => utf16_to_char(&self.shelf_query, end),
                };
            }
        }
        cx.notify();
    }

    /// 候选词面板的锚定位置：书名很短，锚整个输入框即可。
    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    /// 点选定位：本输入框不支持鼠标定位光标（用 ← → 键移光标）。
    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip_on_cjk() {
        let s = "道德经·上篇";
        // 中文 / 间隔号都在 BMP：字符数与 UTF-16 长度一致。
        assert_eq!(utf16_len(s), s.chars().count());
        for i in 0..=s.chars().count() {
            assert_eq!(utf16_to_char(s, char_to_utf16(s, i)), i);
        }
        assert_eq!(utf16_slice(s, 0..3), "道德经");
        assert_eq!(utf16_slice(s, 4..6), "上篇");
    }

    #[test]
    fn utf16_handles_astral_chars() {
        // 𠀀 是增补平面汉字：占 2 个 UTF-16 单元。
        let s = "a𠀀b";
        assert_eq!(utf16_len(s), 4);
        assert_eq!(char_to_utf16(s, 1), 1);
        assert_eq!(char_to_utf16(s, 2), 3);
        assert_eq!(utf16_to_char(s, 3), 2);
        // 越界夹到串尾，不会切进代理对。
        assert_eq!(utf16_to_char(s, 99), 3);
        assert_eq!(utf16_slice(s, 1..3), "𠀀");
    }

    #[test]
    fn replace_inserts_at_cursor() {
        let mut draft = "道德经".to_string();
        let mut cursor = 3;
        // 光标在末尾：追加。
        replace_utf16(&mut draft, &mut cursor, None, "新编");
        assert_eq!(draft, "道德经新编");
        assert_eq!(cursor, 5);

        // 光标在中间：插在光标处，后面的字后移。
        let mut draft = "道德经".to_string();
        let mut cursor = 1;
        replace_utf16(&mut draft, &mut cursor, None, "X");
        assert_eq!(draft, "道X德经");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn replace_respects_title_cap() {
        let mut draft = "道德经".to_string();
        let mut cursor = 3;
        // 上限 12 字：多余的丢掉。
        replace_utf16(&mut draft, &mut cursor, None, "这是一段很长的书名超过十二个字");
        assert_eq!(draft.chars().count(), 12);
    }

    #[test]
    fn replace_overwrites_marked_range() {
        // 模拟输入法上屏：把开头的拼音替换成汉字。
        let mut draft = "nihao道".to_string();
        let mut cursor = 0;
        let start = replace_utf16(&mut draft, &mut cursor, Some(0..5), "你好");
        assert_eq!(draft, "你好道");
        assert_eq!(start, 0);
        assert_eq!(cursor, 2);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)] // 钉的是常量档位关系（笔记框 > 检索框）
    fn note_cap_is_generous_but_bounded() {
        // 笔记框走 NOTE_MAX（500），比分名（12）/ 检索（64）宽得多：
        // 一段批注写得下，但也不会无限增长。
        assert!(NOTE_MAX > SEARCH_MAX);
        let mut draft = String::new();
        let mut cursor = 0;
        let long: String = "批".repeat(NOTE_MAX + 40);
        replace_utf16_limited(&mut draft, &mut cursor, None, &long, NOTE_MAX);
        assert_eq!(draft.chars().count(), NOTE_MAX);
        assert_eq!(cursor, NOTE_MAX);
        // 到达上限后继续输入不再增长。
        replace_utf16_limited(&mut draft, &mut cursor, None, "再写", NOTE_MAX);
        assert_eq!(draft.chars().count(), NOTE_MAX);
    }

    #[test]
    fn note_ime_inserts_chinese_at_cursor() {
        // 模拟中文输入法上屏：笔记框光标处插入汉字（这是原先缺失的那条路）。
        let mut draft = "长安".to_string();
        let mut cursor = 1;
        let start = replace_utf16_limited(&mut draft, &mut cursor, None, "城", NOTE_MAX);
        assert_eq!(draft, "长城安");
        assert_eq!(start, 1);
        assert_eq!(cursor, 2);
    }

    #[test]
    fn replace_drops_control_chars() {
        let mut draft = String::new();
        let mut cursor = 0;
        // IME 偶尔会夹带换行 / 回车，书名里应当被滤掉。
        replace_utf16(&mut draft, &mut cursor, None, "道德\r\n经");
        assert_eq!(draft, "道德经");
    }

    #[test]
    fn effective_range_falls_back_to_marked() {
        // 输入法显式给了区间：以它为准。
        assert_eq!(effective_replace_range(Some(3..7), &Some(0..2)), Some(3..7));
        // NSNotFound（None）：替换当前标记区间——这是叠字事故的另一半。
        assert_eq!(effective_replace_range(None, &Some(0..2)), Some(0..2));
        // 没有标记区间：才落在光标处（None = 插到光标）。
        assert_eq!(effective_replace_range(None, &None), None);
    }

    #[test]
    fn ime_composition_replaces_marked_each_step() {
        // 复现并锁死「Squirrel 双拼敲 nihao」的修复：
        // 输入法每步都传 NSNotFound（None），草稿必须逐级替换标记区间，
        // 而不是把新拼音叠在旧拼音后面（修复前得到 21 字而非「你好」）。
        let mut draft = String::new();
        let mut cursor = 0usize;
        let mut marked: Option<Range<usize>> = None;
        for preedit in ["n", "ni", "nih", "niha", "nihao"] {
            let eff = effective_replace_range(None, &marked);
            let start = replace_utf16_limited(&mut draft, &mut cursor, eff, preedit, NOTE_MAX);
            marked = Some(start..start + utf16_len(preedit));
        }
        assert_eq!(draft, "nihao");
        assert_eq!(marked, Some(0..5));
        // 上屏：汉字顶掉整段拼音，而不是插在它后面。
        let eff = effective_replace_range(None, &marked);
        replace_utf16_limited(&mut draft, &mut cursor, eff, "你好", NOTE_MAX);
        assert_eq!(draft, "你好");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn ime_second_word_appends_after_first() {
        // 第一个词上屏后标记清零，第二个词从光标处接续——
        // 不会把第二个词的拼音叠到第一个词后面。
        let mut draft = String::new();
        let mut cursor = 0usize;
        let mut marked: Option<Range<usize>> = None;
        let eff = effective_replace_range(None, &marked);
        let start = replace_utf16_limited(&mut draft, &mut cursor, eff, "nihao", NOTE_MAX);
        marked = Some(start..start + utf16_len("nihao"));
        let eff = effective_replace_range(None, &marked);
        replace_utf16_limited(&mut draft, &mut cursor, eff, "你好", NOTE_MAX);
        marked = None;
        // 第二个词
        for preedit in ["shi", "shi'jie"] {
            let eff = effective_replace_range(None, &marked);
            let start = replace_utf16_limited(&mut draft, &mut cursor, eff, preedit, NOTE_MAX);
            marked = Some(start..start + utf16_len(preedit));
        }
        assert_eq!(draft, "你好shi'jie");
        let eff = effective_replace_range(None, &marked);
        replace_utf16_limited(&mut draft, &mut cursor, eff, "世界", NOTE_MAX);
        assert_eq!(draft, "你好世界");
    }
}
