//! 笺校工作台（Collation Tool）—— 与阅读器完全解耦的全屏模态视图。
//!
//! 设计原则（见《技术可行性与商业化评估报告》）：
//!   ① `enum AppMode { Reader, Collation }` 顶层状态机：进入 Collation 时，
//!      `Root::render` 直接返回 `Entity<CollationWorkspace>`，**根本不构建**
//!      阅读器视图 → 对阅读界面零影响。
//!   ② 本模块只依赖 `jiandu-core` 与现有 `ime` 基础设施，**绝不 import reader 模块**。
//!   ③ 右侧编辑器用 GPUI 原生轻量元素 + 复用 `crate::ime::InputField` /
//!      `EntityInputHandler` 做中文输入（GPUI 0.2.2 没有内置 Editor）。
//!   ④ 竖排预览（已落地）走「离屏栅格化 + `img` 贴图」：复用阅读引擎的
//!      `share_layout::layout` + `share_paint::render_png`，不在每帧重绘朱砂点，
//!      且**不与 reader 耦合**（本模块绝不 import reader）。
//!
//! 本文件是 MVP 的** UI 骨架**：AppMode 切换 + 40/60 分屏 + 可编辑/可选择的
//! 原生编辑器 + 左图右文联动（真实坐标映射：右文第 N 段 → `SourceRegion` 的
//! (x,y,w,h) 自然像素坐标 → 缩放后高亮原图区域 + `ScrollHandle` 滚动）。
//! `SourceRegion` 同时承载「真实 PDF 文本层 / OCR」坐标与「Mock 假数据」，
//! 二者共用同一套映射数学，仅数据源不同（见 `mock_regions` 与后续接入点）。
//!
//! —— 已落地 ——
//! ・方向①（真实图片）：左侧【打开文件】按钮 → rfd 选 PDF / PNG → CoreGraphics
//!   把 PDF 首页（或复用 PNG）预栅格化为位图 → `gpui::img` 贴图，带超大尺寸 /
//!   空指针 / 多页三道防护（见文件末尾 `rasterize_source`）。
//! ・竖排预览：右侧【竖排预览】按钮 → 把转录文本离屏栅格化成「笺读风」竖排书页
//!   （宣纸黄 + 朱丝栏 + 双鱼尾印 + 宋体 12 列），`gpui::img` 整页贴图
//!   （见 `regenerate_preview` + `render_preview_overlay`，纯 `share_*` 链路）。
//! ・真实坐标映射：删除 N*BAND_H 行带 Mock，新增 `SourceRegion`{x,y,w,h,text}
//!   数据源结构；右文点击段落 → 查 `regions[i]` → 自然像素坐标按 `source_size()`
//!   缩放后高亮原图区域并滚动。载入带文本层 PDF 时由 `pdf_text` 抽真实坐标
//!   替换 `mock_regions()`；扫描图（无文本层）退回 Mock 演示，需 OCR 才是下一步。

use crate::ime::{
    char_to_utf16, effective_replace_range, utf16_len, utf16_slice, utf16_to_char,
};
// 竖排预览：复用阅读引擎的「纯数据版式 + CoreGraphics 离屏光栅」链路，与 reader
// 解耦（本模块绝不 import reader）。这三条都是 crate 内模块，reader 同款用法。
use crate::share::{ShareBg, ShareFrame, ShareSeal};
use crate::share_layout::{layout, LayoutSpec, ShareFont};
use crate::share_paint::render_png;
use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, rgba, Animation, AnimationExt, Bounds, Context,
    EntityInputHandler, EventEmitter, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ParentElement, Render,
    ScrollHandle, SharedString, Styled, Window,
};
use std::ops::Range;
use std::time::Duration;
use std::path::{Path, PathBuf};

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
/// 近似字宽：原图转录以 CJK 为主，按字号取值做编辑器内点击定位（仅用于右文
/// 落光标 / 选区估算，不影响左图高亮精度——左图高亮走 `SourceRegion` 真实映射）。
const CHAR_W: f32 = FONT_SIZE * 0.95;
/// 竖排预览各列数档位下的整页容量（由 `share_layout` 版式几何精算得出，
/// 预览固定 1080×1920、不落款不批注：8→120 / 10→190 / 12→264 / 14→364）。
/// 每页喂入 ≤ 此值可保证 `layout` 不触发自动缩字号（share_layout.rs:346 的
/// `cols_max * rows < n` 永不成立），整本文稿字号恒定、版面一致，绝不前大后小。
/// 列数经 `norm_columns` 吸附到这四档，非档位值不会命中。
fn preview_page_cap(columns: usize) -> usize {
    match crate::share::norm_columns(columns) {
        8 => 120,
        10 => 190,
        12 => 264,
        14 => 364,
        _ => 264, // 不应命中：norm_columns 只产出 8/10/12/14
    }
}

/// 预览栅格化的**防卡死三重上限**（见 `regenerate_preview`）。
///
/// 栅格化是主线程同步的 CG 离屏绘制（1080×1920）+ PNG 编码，单页百毫秒级。稿件
/// 一长、页数一多就会把主线程堵死——表现为点「竖排预览」后鼠标转圈、关闭按钮与
/// Esc 全部无反应（不是渲染慢，是主线程根本没有机会处理事件）。三重闸门把最坏
/// 耗时封死在秒级，宁可少预览几页，也绝不让 UI 失去响应。
/// ① 字数上限：超过就截断，只预览前 N 字。
const PREVIEW_MAX_CHARS: usize = 900;
/// ② 页数上限：达到即停止分页（哪怕还有余稿）。
const PREVIEW_MAX_PAGES: usize = 6;
/// ③ 时间预算（毫秒）：累计渲染耗时超预算立即停手，留最后一张给 UI。
const PREVIEW_TIME_BUDGET_MS: u128 = 2500;
/// 预览图在滚动容器里的最大显示高度（px）。
///
/// ⚠️ 绝不能用 `h_full()`：`h_full` = 100% 父容器高，而这里父容器是
/// `overflow_y_scroll()`（高度由内容决定）→ 「容器高依赖内容、内容高依赖容器」
/// 的**布局反馈环**，会让 GPUI 反复求解尺寸直至卡死。给显式上限即可解耦。
const PREVIEW_IMG_MAX_H: f32 = 880.0;

/// 文本区域结构定义在 `pdf_text::SourceRegion`（PDF 文本层抽取的数据源）。
///
/// 真实接入时由带文本层的 PDF 填充（每个识别片段一条）；无文本层（扫描图）时
/// 退回 `mock_regions()` 假数据演示机制。右文第 i 段 ↔ `regions[i]`
/// （转录逐行对齐原图行）。坐标恒为原图自然像素（PDF 点），显示时按
/// `source_size()` 的缩放比换算，绝不写死像素。
use crate::pdf_text::SourceRegion;

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
    /// 预览浮层焦点句柄：预览开启时焦点引到它，使根 div 上的 `on_key_down`
    /// 能收到 ←/→/Esc（GPUI 0.2.2 只派发「焦点节点→根」路径上的键）。
    preview_focus: FocusHandle,
    /// 预览浮层焦点句柄：预览开启时焦点引到它，使根 div 上的 `on_key_down`
    /// 能收到 ←/→/Esc（GPUI 0.2.2 只派发「焦点节点→根」路径上的键）。
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
    /// 上一张临时栅格化 PNG（PDF 转图产生）；载入新图时删除以免堆积。
    last_temp: Option<PathBuf>,
    /// 导入失败信息（红色提示）。
    image_error: Option<String>,
    /// 非错误的温和提示（如多页 PDF 仅渲首页）。
    image_note: Option<String>,
    /// 竖排预览（离屏栅格化）各页位图路径（按页序；均为临时 PNG，重生成时清空）。
    preview_paths: Vec<PathBuf>,
    /// 当前预览页码（0-based）；`preview_paths` 为空时无意义。
    preview_page: usize,
    /// 预览栅格化失败信息（红色提示，如系统缺中文字体）。
    preview_error: Option<String>,
    /// 预览的温和提示（非错误）：如「长稿仅预览前 900 字 / 前 6 页」。
    preview_note: Option<String>,
    /// 竖排预览样式：底色（宣纸黄 / 暗金黑 / 极简白 / 仿刻本）。
    preview_bg: ShareBg,
    /// 竖排预览样式：版框（红线框 / 黑线框 / 无边框 / 双边）。
    preview_frame: ShareFrame,
    /// 竖排预览样式：列数（8 / 10 / 12 / 14，经 norm_columns 锁定）。
    preview_columns: usize,
    /// 样式选择面板是否展开。
    show_style_panel: bool,
    /// 当前联动的右文段索引（0-based）。None = 尚未联动。右文第 i 段 ↔
    /// `regions[i]`（转录逐行对齐原图行）。
    active_para_idx: Option<usize>,
    /// 左图原图自然像素宽（由栅格化结果填充；无原图时由区域包围盒推导）。
    src_nat_w: f32,
    /// 左图原图自然像素高（由栅格化结果填充；无原图时由区域包围盒推导）。
    src_nat_h: f32,
    /// 原图文本层 / OCR 识别出的文本区域（自然像素坐标）。真实接入时由 PDF 文本层
    /// 或 OCR 引擎填充；MVP 由 `mock_regions()` 提供假数据跑通映射数学。
    regions: Vec<SourceRegion>,
}

impl CollationWorkspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            // 示例转录：一段较长的古文（千字文节选），让用户一进来就能看到
            // 编辑器、立即尝试选择/输入，并直接打开「竖排预览」看到多页分页。
            text: "天地玄黄 宇宙洪荒 日月盈昃 辰宿列张 寒来暑往 秋收冬藏 闰余成岁 律吕调阳 云腾致雨 露结为霜 金生丽水 玉出昆冈 剑号巨阙 珠称夜光 果珍李柰 菜重芥姜 海咸河淡 鳞潜羽翔 龙师火帝 鸟官人皇 始制文字 乃服衣裳 推位让国 有虞陶唐 吊民伐罪 周发殷汤 坐朝问道 垂拱平章 爱育黎首 臣伏戎羌 遐迩一体 率宾归王 鸣凤在竹 白驹食场 化被草木 赖及万方\n盖此身发 四大五常 恭惟鞠养 岂敢毁伤 女慕贞洁 男效才良 知过必改 得能莫忘 罔谈彼短 靡恃己长 信使可覆 器欲难量 墨悲丝染 诗赞羔羊 景行维贤 克念作圣 德建名立 形端表正 空谷传声 虚堂习听 祸因恶积 福缘善庆 尺璧非宝 寸阴是竞 资父事君 曰严与敬 孝当竭力 忠则尽命 临深履薄 夙兴温凊 似兰斯馨 如松之盛 川流不息 渊澄取映 容止若思 言辞安定 笃初诚美 慎终宜令 荣业所基 藉甚无竟 学优登仕 摄职从政 存以甘棠 去而益咏"
                .to_string(),
            cursor: 0,
            selection: None,
            marked: None,
            focus: cx.focus_handle(),
            preview_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            left_scroll: ScrollHandle::new(),
            anchor: 0,
            dragging: false,
            show_preview: false,
            image_path: None,
            last_temp: None,
            image_error: None,
            image_note: None,
            preview_paths: Vec::new(),
            preview_page: 0,
            preview_error: None,
            preview_note: None,
            preview_bg: ShareBg::Xuan,
            preview_frame: ShareFrame::Red,
            preview_columns: 12,
            show_style_panel: false,
            active_para_idx: None,
            src_nat_w: 0.0,
            src_nat_h: 0.0,
            regions: Self::mock_regions(),
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
    /// 右文编辑器内用固定布局常量 + 平均字宽估算（仅用于编辑器内落光标 / 选区）；
    /// 左图高亮坐标则来自 `SourceRegion` 的真实自然像素映射，二者互不影响。
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
            self.link_to_paragraph(li, vw, cx);
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

    /// 样式面板里的单选项胶囊：选中态朱底白字，未选中浅朱底。点下即触发 `on_click`。
    fn style_option(
        &self,
        selected: bool,
        label: &'static str,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .px(px(10.0))
            .py(px(4.0))
            .mr(px(6.0))
            .mb(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .bg(if selected { rgb(0x8c2b22) } else { rgba(0x8c2b221f) })
            .text_color(if selected { rgb(0xf4ecd8) } else { rgb(0x8c2b22) })
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

    /// 左侧 40%：原图区。顶部悬浮【打开文件】按钮；下方滚动容器挂 `left_scroll`，
    /// 内容（栅格化位图或 mock 行带）内部用 `relative()` 承载绝对定位的高亮叠加层；
    /// 底部状态条显示导入错误或温和提示（如多页 PDF 仅渲首页）。
    fn render_left(&self, vw: f32, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .w(px(vw * 0.4))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0xddd2b4))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .child(
                        div()
                            .id(SharedString::from("collation-left-scroll"))
                            .w_full()
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.left_scroll)
                            .child(self.render_left_content(vw)),
                    )
                    .child(
                        self.button("打开文件", |c, _, _, cx| c.open_image_picker(cx), cx)
                            .absolute()
                            .top(px(8.0))
                            .right(px(8.0)),
                    ),
            )
            .child(match self.render_image_status() {
                Some(s) => s,
                None => div().h(px(0.0)),
            })
    }

    /// 左图内容：已载入原图则贴 `img`（按宽自适应高度），否则画占位页（按
    /// `source_size()` 自然尺寸等比缩放），两种情况都叠加 `regions` 区域参考框与
    /// 当前联动段的高亮矩形——高亮坐标全部来自 `SourceRegion` 的真实 (x,y,w,h)
    /// 自然像素，按 `src_nat` 缩放后落到显示坐标（彻底告别 N*BAND_H Mock）。
    fn render_left_content(&self, vw: f32) -> gpui::Div {
        let (nw, nh) = self.source_size();
        let disp_w = vw * 0.4;
        let scale = if nw > 0.0 { disp_w / nw } else { 1.0 };
        let disp_h = nh * scale;
        let mut page = div().relative().w_full().h(px(disp_h)).bg(rgb(0xf1e8cf));

        if let Some(p) = &self.image_path {
            page = page.child(img(p.clone()).w_full());
        }
        // 区域参考框：无原图时画出全部 regions（验证映射数学），有原图时仅画
        // 当前联动段（避免遮挡整图）；真实文本层接入后这些框即原图真实文本位置。
        for (i, r) in self.regions.iter().enumerate() {
            let active = self.active_para_idx == Some(i);
            if self.image_path.is_some() && !active {
                continue;
            }
            page = page.child(
                div()
                    .absolute()
                    .top(px(r.y * scale))
                    .left(px(r.x * scale))
                    .w(px(r.w * scale))
                    .h(px(r.h * scale))
                    .border_1()
                    .border_color(if active {
                        rgba(0x8c2b22cc)
                    } else {
                        rgba(0x8c2b2255)
                    })
                    .bg(if active {
                        rgba(0x8c2b2222)
                    } else {
                        rgba(0x00000000)
                    }),
            );
        }
        page
    }

    /// 导入原图（PDF / PNG）：弹出系统文件选择框（rfd **前台直调**），
    /// 选中后在异步块里把 PDF 首页 / PNG 预栅格化为位图路径，存入 `image_path`。
    fn open_image_picker(&mut self, cx: &mut Context<Self>) {
        // rfd 同步对话框必须跑在主线程（见 IME 笔记 / reader.rs）：用 cx.spawn 前台
        // executor，块内同步调用 pick_file；绝不过 background_executor（会静默 None）。
        cx.spawn(
            |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
            // AsyncApp 是 Clone：把「借用的上下文」换成「owned 句柄」，
            // async move 块才能安全持有它跨 await（借用不能进 'static future）。
            let mut app = app.clone();
            async move {
            let picked = rfd::FileDialog::new()
                .set_title("选择原图（PDF / PNG）")
                .add_filter("原图 (PDF / PNG)", &["pdf", "png"])
                .add_filter("全部文件", &["*"])
                .pick_file();
            let Some(path) = picked else { return };

            // 1) 文本层坐标抽取：**主线程同步执行**。PDFKit / Foundation 的 objc 调用
            //    必须在主线程（且 autorelease pool 已在本函数内部包裹，见 pdf_text.rs）。
            //    与栅格化解耦：先点亮左侧真实区域框，不阻塞 UI。
            if let Some(entity) = this.upgrade() {
                let _ = entity.update(&mut app, |this, cx| {
                    // 真实坐标映射：带文本层的 PDF 用 PDFKit 抽出的文本层坐标替换 Mock；
                    // 无文本层（扫描图 / 位图）extract 返回空 → 清空 Mock，避免高亮落
                    // 在错误位置，由状态栏提示需 OCR。
                    let real = crate::pdf_text::extract_pdf_text_regions(&path);
                    if !real.is_empty() {
                        this.regions = real;
                        this.image_error = None;
                        this.image_note = None;
                    } else {
                        this.regions.clear();
                        this.image_note = Some(
                            "该原图无文本层（扫描图），无法坐标映射，需 OCR".to_string(),
                        );
                    }
                    cx.notify();
                });
            }

            // 2) 最耗时的栅格化（CG 绘制首页 / 读 PNG 尺寸）放到后台 executor，
            //    避免阻塞 UI；完成后回主线程写入 image_path 等字段。
            let bg = app.background_executor().clone();
            let raster_path = path.clone();
            let result = bg.spawn(async move { rasterize_source(&raster_path) }).await;

            if let Some(entity) = this.upgrade() {
                let _ = entity.update(&mut app, |this, cx| {
                    match result {
                        Ok(r) => {
                            // 删除上一张临时栅格化图（绝不碰用户原文件）。
                            if let Some(old) = this.last_temp.take() {
                                let _ = std::fs::remove_file(&old);
                            }
                            this.image_path = Some(r.path);
                            this.last_temp = r.temp;
                            // 记录原图自然像素尺寸（PDF 点），供真实坐标映射缩放使用。
                            this.src_nat_w = r.nat_w as f32;
                            this.src_nat_h = r.nat_h as f32;
                            this.image_error = None;
                            // 若此前因无文本层已设 OCR 提示则保留；否则用栅格化 note。
                            if this.image_note.is_none() {
                                this.image_note = r.note;
                            }
                        }
                        Err(e) => {
                            this.image_error = Some(e);
                            this.image_note = None;
                        }
                    }
                    cx.notify();
                });
            }
        }
        })
        .detach();
    }

    /// 重新生成竖排预览：把当前编辑器文本离屏栅格化成「笺读风」竖排书页 PNG。
    ///
    /// 复用阅读引擎的纯数据版式（`share_layout::LayoutSpec` + `layout`）与
    /// CoreGraphics 离屏光栅（`share_paint::render_png`），与 reader 完全解耦；
    /// 输出 PNG 字节写临时文件，交给 `gpui::img` 异步解码贴图（与 PDF 导入同款
    /// 字节 → 路径 → 贴图 路径）。栅格化在主线程同步完成（单页毫秒级，导出亦同）。
    ///
    /// 设计取舍（MVP）：只渲「首页」——`layout` 按版心容量自动截断超长文本；
    /// 翻页（多页卡预览）留待后续。文本改动后需点浮层「刷新预览」重生成。
    /// 把转录文本离屏栅格化成「笺读风」竖排书页（**多页**）。
    ///
    /// 长文按 [`preview_page_cap`]（随当前列数）逐块喂入 `layout`：每块都 ≤ 整页
    /// 容量，故 `layout` 不会触发自动缩字号（share_layout.rs:346），整本字号恒定、
    /// 版面一致，绝不前大后小。用 `layout` 返回的实际落字数 `CardLayout.chars.len()`
    /// 当页边界推进——零猜测、绝不丢字，列数 / 底色 / 版框怎么变都安全。
    /// 任意一页失败则整体报错并清空已有页（绝不留下半截图）。
    fn regenerate_preview(&mut self) {
        let t0 = std::time::Instant::now();
        // 先清旧页（绝不碰用户原文件）。
        for p in self.preview_paths.drain(..) {
            let _ = std::fs::remove_file(&p);
        }
        self.preview_error = None;
        self.preview_note = None;

        // 预览是一条连续竖排流：去掉编辑器里的换行（段落概念不在简单预览里
        // 体现），否则 '\n' 会占一格变成竖排里的空白格。
        let mut body: Vec<char> = self
            .text
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();
        if body.is_empty() {
            self.preview_page = 0;
            return; // 空稿：无图，浮层显示占位提示。
        }

        // —— 防卡死闸门 ①：字数上限 ——
        // 栅格化是主线程同步的 CG 绘制 + PNG 编码，不做上限时长稿会把主线程堵死
        // （鼠标转圈、关闭/Esc 全无响应）。宁可只预览前 N 字，也要保住 UI 响应。
        let orig_len = body.len();
        let mut note: Option<String> = None;
        if body.len() > PREVIEW_MAX_CHARS {
            note = Some(format!(
                "长稿仅预览前 {} 字（共 {} 字）",
                PREVIEW_MAX_CHARS, orig_len
            ));
            body.truncate(PREVIEW_MAX_CHARS);
        }
        let red = vec![false; body.len()];

        // 按当前列数取整页容量，喂入 ≤ 此值保证不缩字；再用实际落字数推进。
        let cap = preview_page_cap(self.preview_columns);
        let mut pages: Vec<PathBuf> = Vec::new();
        let mut start = 0usize;
        let mut cut = false; // 是否被闸门 ②/③ 截断
        while start < body.len() {
            // —— 防卡死闸门 ②：页数上限 ——
            if pages.len() >= PREVIEW_MAX_PAGES {
                cut = true;
                break;
            }
            // —— 防卡死闸门 ③：时间预算 ——
            // 每页开渲前先看累计耗时，超预算立即收手（已渲的页照常显示）。
            if t0.elapsed().as_millis() > PREVIEW_TIME_BUDGET_MS {
                cut = true;
                break;
            }
            let end = (start + cap).min(body.len());
            let spec = LayoutSpec {
                body: &body[start..end],
                red: &red[start..end],
                note: "",
                show_notes: false,
                sign: "",
                show_sign: false,
                date: "",
                show_date: false,
                columns: self.preview_columns,
                bg: self.preview_bg,
                frame: self.preview_frame,
                seal: ShareSeal::Twin,
                seal_text: "笺校",
                font: ShareFont::Song,
                stamp: "",
                w: 1080.0,
                h: 1920.0,
            };
            // 让 layout 自己按整页容量落字；取实际落字数当页边界（双保险，绝不丢字）。
            let card = layout(&spec);
            let laid = card.chars.len();
            if laid == 0 {
                break; // 异常：一字未落，避免死循环。
            }
            match render_png(&card) {
                Ok(png) => {
                    let out = temp_png_path(); // 纳秒唯一命名，绕开 img 缓存。
                    if std::fs::write(&out, &png).is_ok() {
                        pages.push(out);
                    } else {
                        self.preview_error = Some("预览图写入临时文件失败".to_string());
                        for p in pages.drain(..) {
                            let _ = std::fs::remove_file(&p);
                        }
                        return;
                    }
                }
                Err(e) => {
                    self.preview_error = Some(e);
                    for p in pages.drain(..) {
                        let _ = std::fs::remove_file(&p);
                    }
                    return;
                }
            }
            start += laid;
        }

        if cut {
            note = Some(format!(
                "长稿仅预览前 {} 页（共 {} 字，余稿未渲以防界面卡死）",
                pages.len(),
                orig_len
            ));
        }
        self.preview_paths = pages;
        // 列数 / 样式变动可能改变页数，钳制当前页到有效范围。
        let max_idx = self.preview_paths.len().saturating_sub(1);
        if self.preview_page > max_idx {
            self.preview_page = max_idx;
        }
        self.preview_note = note;
        // 防卡死排障日志：字数 / 页数 / 耗时一目了然，便于真机判断是否还需要收紧。
        eprintln!(
            "[preview] chars={}/{} pages={} elapsed={}ms",
            body.len(),
            orig_len,
            self.preview_paths.len(),
            t0.elapsed().as_millis()
        );
    }

    /// 右文某段 → 左图联动：记录段索引 + 左侧滚动到对应区域。
    ///
    /// 真实坐标映射（替换 N*BAND_H Mock）：右文第 `para_idx` 段 ↔ `regions[para_idx]`
    /// （转录逐行对齐原图行）。区域 (x,y,w,h) 为原图自然像素，按 `src_nat` 缩放后
    /// 既是高亮矩形（由 `render_left_content` 渲染）也是滚动目标。无对应区域
    /// （段落数 ≠ 区域数）则不高亮也不滚动。
    fn link_to_paragraph(&mut self, para_idx: usize, vw: f32, cx: &mut Context<Self>) {
        // 无对应区域（段落数 ≠ 区域数 / 原图无文本层）→ 不高亮、不滚动、不标记。
        let Some(r) = self.regions.get(para_idx) else {
            return;
        };
        self.active_para_idx = Some(para_idx);
        let (nw, _nh) = self.source_size();
        let disp_w = vw * 0.4;
        let scale = if nw > 0.0 { disp_w / nw } else { 1.0 };
        // 滚动使区域顶部进入视野（向下偏移取负）。
        self.left_scroll
            .set_offset(Point::new(px(0.0), px(-(r.y * scale))));
        cx.notify();
    }

    /// 左图原图自然像素尺寸：已载入原图用栅格化得到的真实尺寸；无原图时由
    /// `regions` 包围盒推导（Mock 演示用，保证占位页能框住全部区域）。
    fn source_size(&self) -> (f32, f32) {
        if self.src_nat_w > 0.0 && self.src_nat_h > 0.0 {
            return (self.src_nat_w, self.src_nat_h);
        }
        let mut max_x: f32 = 800.0;
        let mut max_y: f32 = 1100.0;
        for r in &self.regions {
            max_x = max_x.max(r.x + r.w);
            max_y = max_y.max(r.y + r.h);
        }
        (max_x, max_y)
    }

    /// Mock 坐标数组：替代被删除的 N*BAND_H 行带，提供一套假的原图文本层坐标，
    /// 用于在无真实扫描件时跑通「右文段落 → 左图真实 X/Y 高亮 + 滚动」的底层数学。
    /// 每个 `SourceRegion` 对应右文一段（按索引对齐），坐标为假设原图 900×1300
    /// 自然像素空间；真实接入时整段替换为 PDF 文本层 / OCR 输出即可（结构不变）。
    fn mock_regions() -> Vec<SourceRegion> {
        // 与 `new()` 中的示例转录两段对应：第 1 段落在原图上方偏左，第 2 段落在
        // 下方偏右——两个框的 x/y/w/h 都不同，便于直观验证映射同时用到了四项坐标。
        vec![
            SourceRegion {
                x: 90.0,
                y: 120.0,
                w: 720.0,
                h: 480.0,
                text: "天地玄黄 宇宙洪荒 …（示例转录第一段）".to_string(),
            },
            SourceRegion {
                x: 300.0,
                y: 700.0,
                w: 500.0,
                h: 460.0,
                text: "盖此身发 四大五常 …（示例转录第二段）".to_string(),
            },
        ]
    }

    /// 右侧联动状态条：显示当前已联动段对应的原图区域坐标（自然像素）。
    fn render_link_status(&self) -> gpui::Div {
        match self.active_para_idx {
            Some(idx) => {
                let coord = self
                    .regions
                    .get(idx)
                    .map(|r| format!("x={:.0} y={:.0} w={:.0} h={:.0}", r.x, r.y, r.w, r.h))
                    .unwrap_or_else(|| "（无对应区域）".to_string());
                div()
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .px(px(PAD))
                    .bg(rgba(0x8c2b2214))
                    .text_color(rgb(0x8c2b22))
                    .child(SharedString::from(format!(
                        "已联动：右文第 {} 段 → 原图区域 {}（自然像素）",
                        idx + 1,
                        coord
                    )))
            }
            None => div().h(px(0.0)),
        }
    }

    /// 底部状态条：优先显示导入错误（红），否则显示温和提示（如多页 PDF 仅渲首页）。
    fn render_image_status(&self) -> Option<gpui::Div> {
        if let Some(e) = &self.image_error {
            return Some(
                div()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .px(px(PAD))
                    .bg(rgba(0xb0342c33))
                    .text_color(rgb(0xb0342c))
                    .text_size(px(13.0))
                    .child(SharedString::from(e.clone())),
            );
        }
        if let Some(n) = &self.image_note {
            return Some(
                div()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .px(px(PAD))
                    .bg(rgba(0x8c2b2210))
                    .text_color(rgb(0x8c2b22))
                    .text_size(px(13.0))
                    .child(SharedString::from(n.clone())),
            );
        }
        None
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
                        if c.show_preview {
                            c.regenerate_preview();
                        }
                        cx.notify();
                    }, cx)),
            )
            .child(self.render_link_status())
            .child(editor)
    }

    /// 渲染单行（含选区背景与光标）。
    ///
    /// ⚠️ 软折行：长文本（尤其无换行符的连续中文，如「天地玄黄……」）必须能
    /// 在 60% 宽右栏内折行，否则会撑破右边缘被截断。做法：文本段用
    /// `flex_1() + min_w(0)` 受行宽约束（破除 flex item 默认 min-width 撑开陷阱），
    /// 再 `whitespace_normal()` 允许在字符间断行；行容器高度由固定 `h(LINE_H)`
    /// 改为 `min_h`，折成多行后自然增高。
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

        // 文本段：受行宽约束 + 软折行（含 CJK 字符间断行）。
        let text_seg = |seg: String| -> gpui::Div {
            div()
                .flex_1()
                .min_w(px(0.0))
                .whitespace_normal()
                .child(SharedString::from(seg))
        };

        let mut row = div()
            .min_h(px(LINE_H))
            .w_full()
            .flex()
            .flex_row()
            .items_center();
        if overlaps {
            let before: String = self
                .text
                .chars()
                .skip(line_start)
                .take(seg_start.saturating_sub(line_start))
                .collect();
            if !before.is_empty() {
                row = row.child(text_seg(before));
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
                    .flex_1()
                    .min_w(px(0.0))
                    .whitespace_normal()
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
                row = row.child(text_seg(after));
            }
        } else if caret_here {
            let before: String = self
                .text
                .chars()
                .skip(line_start)
                .take(self.cursor.saturating_sub(line_start))
                .collect();
            if !before.is_empty() {
                row = row.child(text_seg(before));
            }
            row = row.child(self.caret());
            let after: String = self
                .text
                .chars()
                .skip(self.cursor)
                .take(line_end.saturating_sub(self.cursor))
                .collect();
            if !after.is_empty() {
                row = row.child(text_seg(after));
            }
        } else {
            row = row.child(text_seg(line.to_string()));
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

    /// 竖排预览浮层：把转录文本离屏栅格化成「笺读风」竖排书页，`gpui::img` 贴图。
    ///
    /// 全屏遮罩 + 顶部工具条（翻页器 / 刷新 / 样式 / 关闭）+ 居中整页图（随窗口
    /// 高度自适应，比例保持竖排书页）+ 样式面板（底色 / 版框 / 列数，点按即重栅格
    /// 化）+ 底部状态条（仅栅格化错误，如缺中文字体）。长文自动按页切块，用顶部
    /// 「← 第 X / Y 页 →」翻页（见 [`regenerate_preview`]）。
    fn render_preview_overlay(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .absolute()
            .top(px(HEADER_H))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .bg(rgba(0x00000059))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(HEADER_H))
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(PAD))
                    .bg(rgb(0x2b2620))
                    .child(
                        div()
                            .text_color(rgb(0xf4ecd8))
                            .text_size(px(15.0))
                            .child(SharedString::from("竖排预览")),
                    )
                    .child(div().flex_1())
                    .child(self.preview_pager(cx))
                    .child(self.button(
                        "刷新预览",
                        |c, _, _, cx| {
                            c.regenerate_preview();
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(self.button(
                        "样式",
                        |c, _, _, cx| {
                            c.show_style_panel = !c.show_style_panel;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(self.button(
                        "✕ 关闭",
                        |c, _, _, cx| {
                            c.show_preview = false;
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .id(SharedString::from("collation-preview-scroll"))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_y_scroll()
                    .child({
                        let idx = self.preview_page.min(self.preview_paths.len().saturating_sub(1));
                        match self.preview_paths.get(idx) {
                            Some(p) => div()
                                // ⚠️ 绝不用 h_full()：父容器是 overflow_y_scroll
                                // （高度由内容定），h_full 会形成「容器高↔内容高」
                                // 的布局反馈环 → GPUI 反复求解尺寸直至卡死。显式
                                // 限高解耦，超高则由滚动容器接管。
                                .child(img(p.clone()).max_h(px(PREVIEW_IMG_MAX_H)))
                                // 翻页淡入：动画 id 含页下标，翻到新页 id 变 → 重播，
                                // 同页重绘（hover 等）id 不变 → 停在 opacity 1 不闪。
                                .with_animation(
                                    SharedString::from(format!("preview-page-{}", idx)),
                                    Animation::new(Duration::from_millis(180)),
                                    |page, delta| page.opacity(delta),
                                )
                                .into_any_element(),
                            None => div()
                                .px(px(20.0))
                                .py(px(16.0))
                                .bg(rgb(0xf4ecd8))
                                .rounded(px(8.0))
                                .text_color(rgb(0x2b2620))
                                .text_size(px(14.0))
                                .child(SharedString::from(
                                    "（转录为空：写入文字后点「竖排预览」或「刷新预览」）",
                                ))
                                .into_any_element(),
                        }
                    }),
            )
            .child(self.render_style_panel(cx))
            .child(match self.render_preview_status() {
                Some(s) => s,
                None => div().h(px(0.0)),
            })
    }

    /// 样式选择面板（浮在预览图之上）：底色 / 版框 / 列数三组切换。
    /// 每个选项点下即设置对应样式并 [`regenerate_preview`] 重栅格化；选中态
    /// 高亮（朱底白字），当前列数随 `norm_columns` 吸附到 8/10/12/14 四档。
    fn render_style_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        if !self.show_style_panel {
            return div();
        }

        // —— 底色行 ——
        let mut bg_row = div()
            .flex()
            .flex_wrap()
            .child(
                div()
                    .w_full()
                    .mb(px(4.0))
                    .text_color(rgb(0x8c2b22))
                    .text_size(px(13.0))
                    .child(SharedString::from("底色")),
            );
        for b in ShareBg::ALL {
            bg_row = bg_row.child(self.style_option(
                self.preview_bg == b,
                b.label(),
                move |c, _, _, cx| {
                    c.preview_bg = b;
                    c.regenerate_preview();
                    cx.notify();
                },
                cx,
            ));
        }

        // —— 版框行 ——
        let mut frame_row = div()
            .flex()
            .flex_wrap()
            .child(
                div()
                    .w_full()
                    .mb(px(4.0))
                    .text_color(rgb(0x8c2b22))
                    .text_size(px(13.0))
                    .child(SharedString::from("版框")),
            );
        for f in ShareFrame::ALL {
            frame_row = frame_row.child(self.style_option(
                self.preview_frame == f,
                f.label(),
                move |c, _, _, cx| {
                    c.preview_frame = f;
                    c.regenerate_preview();
                    cx.notify();
                },
                cx,
            ));
        }

        // —— 列数行（8/10/12/14，经 norm_columns 锁定）——
        let mut col_row = div()
            .flex()
            .flex_wrap()
            .child(
                div()
                    .w_full()
                    .mb(px(4.0))
                    .text_color(rgb(0x8c2b22))
                    .text_size(px(13.0))
                    .child(SharedString::from("列数")),
            );
        for cols in [8usize, 10, 12, 14] {
            col_row = col_row.child(self.style_option(
                crate::share::norm_columns(self.preview_columns) == cols,
                match cols {
                    8 => "八列",
                    10 => "十列",
                    12 => "十二列",
                    14 => "十四列",
                    _ => "列",
                },
                move |c, _, _, cx| {
                    c.preview_columns = cols;
                    c.regenerate_preview();
                    cx.notify();
                },
                cx,
            ));
        }

        div()
            .absolute()
            .top(px(HEADER_H + 6.0))
            .left(px(PAD))
            .w(px(252.0))
            .bg(rgb(0xf4ecd8))
            .rounded(px(8.0))
            .p(px(12.0))
            .child(bg_row)
            .child(frame_row)
            .child(col_row)
    }

    /// 翻页器：多页时显示「← 第 X / Y 页 →」，单页（或无图）时返回空 div。
    fn preview_pager(&self, cx: &mut Context<Self>) -> gpui::Div {
        let n = self.preview_paths.len();
        if n <= 1 {
            // 单页也给出键盘提示：Esc 关闭浮层（←/→ 多页时才翻得动）。
            return div()
                .text_color(rgba(0xf4ecd8aa))
                .text_size(px(12.0))
                .child(SharedString::from("← → 翻页 · Esc 关闭"));
        }
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(self.button(
                "←",
                |c, _, _, cx| {
                    if c.preview_page > 0 {
                        c.preview_page -= 1;
                    }
                    cx.notify();
                },
                cx,
            ))
            .child(
                div()
                    .text_color(rgb(0xf4ecd8))
                    .text_size(px(14.0))
                    .child(SharedString::from(format!(
                        "第 {} / {} 頁",
                        self.preview_page + 1,
                        n
                    ))),
            )
            .child(self.button(
                "→",
                |c, _, _, cx| {
                    if c.preview_page + 1 < c.preview_paths.len() {
                        c.preview_page += 1;
                    }
                    cx.notify();
                },
                cx,
            ))
    }

    /// 底部状态条：仅显示预览栅格化错误（红）。多页提示由顶部翻页器承载。
    fn render_preview_status(&self) -> Option<gpui::Div> {
        if let Some(e) = self.preview_error.as_ref() {
            return Some(
                div()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .px(px(PAD))
                    .bg(rgba(0xb0342c33))
                    .text_color(rgb(0xb0342c))
                    .text_size(px(13.0))
                    .child(SharedString::from(e.clone())),
            );
        }
        // 非错误提示：长稿被防卡死闸门截断（字数 / 页数 / 时间预算）。
        let n = self.preview_note.as_ref()?;
        Some(
            div()
                .h(px(24.0))
                .flex()
                .items_center()
                .px(px(PAD))
                .bg(rgba(0x8c2b2218))
                .text_color(rgb(0x8c2b22))
                .text_size(px(13.0))
                .child(SharedString::from(n.clone())),
        )
    }
}

impl EventEmitter<CollationEvent> for CollationWorkspace {}

impl CollationWorkspace {
    /// 预览浮层的键盘总入口（挂在根 div 上，仅在 `show_preview` 时生效）。
    ///
    /// ←/→ 翻页（到边界钳制）；Esc 关闭浮层。预览关闭时直接返回，绝不干扰
    /// 编辑器自身的方向键光标移动。
    fn preview_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.show_preview {
            return;
        }
        let pages = self.preview_paths.len();
        match event.keystroke.key.as_str() {
            "left" => {
                if self.preview_page > 0 {
                    self.preview_page -= 1;
                    cx.notify();
                }
            }
            "right" => {
                if pages > 0 && self.preview_page + 1 < pages {
                    self.preview_page += 1;
                    cx.notify();
                }
            }
            "escape" => {
                self.show_preview = false;
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for CollationWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vw = px_f32(window.viewport_size().width);
        // 焦点归属：预览开启时把焦点引到 `preview_focus`，使其落在「根元素」派发
        // 路径上 → 根 div 的 `on_key_down` 能收到 ←/→/Esc；同时编辑器（IME 焦点在
        // `focus`）自然失焦，不再抢方向键去移动光标。预览关闭则焦点还给编辑器，
        // 中文输入照常。
        // ⚠️ 必须判 is_focused 再 focus：render() 每帧都会走这里，若无条件调
        //    window.focus()，焦点事件会触发 notify → 重绘 → 再 focus → **无限重绘
        //    死循环**，主线程被彻底占满（表现为进入预览 / 开样式面板后整个界面卡死，
        //    点【返回阅读】、按 Esc 全无反应）。两个分支一律先判再设。
        if self.show_preview {
            if !self.preview_focus.is_focused(window) {
                window.focus(&self.preview_focus);
            }
        } else if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0xf4ecd8))
            .track_focus(&self.preview_focus)
            .on_key_down(cx.listener(|c, event, _window, cx| c.preview_key(event, cx)))
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
            root = root.child(self.render_preview_overlay(cx));
        }
        root
    }
}

// ── 原图导入：rfd 选文件 → 预栅格化为位图（PDF 首页 / PNG 直接复用）──
// 策略（见评估报告）：预栅格化位图 + gpui::img 贴图。PDF 用 CoreGraphics 把首页
// 绘到 CGBitmapContext，再编码为 PNG 临时文件交给 img 异步解码。
//
// 三道安全防线（防超大 / 超长卷轴 OOM 或闪退）：
//   ① 计算防护：建位图前先算 宽*高*4，超限则按比例缩到 最大边长≤4096 且 ≤64MB；
//   ② 空指针：直接用 CGBitmapContextCreate 并判空，绕开 create_bitmap_context
//      内部的 assert（其遇分配失败会直接 panic）；
//   ③ 多页 PDF：只渲首页，页数>1 时给温和提示，不报错。

/// 临时 PNG 输出路径（系统 temp 目录，按纳秒时间戳唯一命名，避免缓存串图）。
fn temp_png_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("jiandu_collation_{}.png", nanos))
}

/// 栅格化结果：显示路径、临时路径、原图自然像素尺寸、温和提示。
/// `nat_w` / `nat_h` 供真实坐标映射（`source_size`）把 `SourceRegion` 自然像素
/// 缩放到显示坐标；无原图时回退到区域包围盒。
struct Rasterized {
    path: PathBuf,
    temp: Option<PathBuf>,
    nat_w: f64,
    nat_h: f64,
    note: Option<String>,
}

#[cfg(target_os = "macos")]
fn rasterize_source(path: &Path) -> Result<Rasterized, String> {
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGAffineTransform, CGPoint, CGRect, CGSize};
    use core_graphics::sys;
    use foreign_types::ForeignType;
    use std::os::raw::c_void;
    use std::ptr;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());

    // PNG / 常见位图：gpui::img 自带解码器，无需预栅格化，直接复用原路径。
    if matches!(
        ext.as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("tif") | Some("tiff") | Some("bmp")
            | Some("gif") | Some("webp")
    ) {
        // 位图原图：gpui::img 自带解码器，无需预栅格化；读其自然尺寸填坐标。
        let (nw, nh) = image::image_dimensions(path)
            .map(|(w, h)| (w as f64, h as f64))
            .unwrap_or((800.0, 1100.0));
        return Ok(Rasterized {
            path: path.to_path_buf(),
            temp: None,
            nat_w: nw,
            nat_h: nh,
            note: None,
        });
    }
    // 首版仅支持 PDF 栅格化。
    if ext.as_deref() != Some("pdf") {
        return Err(format!(
            "不支持的文件类型：{:?}（首版仅支持 PDF / 常见图片）",
            path
        ));
    }

    extern "C" {
        fn CGPDFDocumentCreateWithURL(url: core_foundation::url::CFURLRef) -> *mut c_void;
        fn CGPDFDocumentGetNumberOfPages(doc: *mut c_void) -> usize;
        fn CGPDFDocumentGetPage(doc: *mut c_void, page_number: usize) -> *mut c_void;
        fn CGPDFPageRetain(page: *mut c_void) -> *mut c_void;
        fn CGPDFPageGetBoxRect(page: *mut c_void, box_: u32) -> CGBox;
        fn CGPDFPageGetRotationAngle(page: *mut c_void) -> i32;
        fn CGPDFPageGetDrawingTransform(
            page: *mut c_void,
            box_: u32,
            rect: CGRect,
            rotate: i32,
            preserve_aspect_ratio: bool,
        ) -> CGAffineTransform;
        fn CGContextConcatCTM(ctx: sys::CGContextRef, t: CGAffineTransform);
        fn CGContextTranslateCTM(ctx: sys::CGContextRef, tx: f64, ty: f64);
        fn CGContextScaleCTM(ctx: sys::CGContextRef, sx: f64, sy: f64);
        fn CGContextDrawPDFPage(ctx: sys::CGContextRef, page: *mut c_void);
        fn CGPDFDocumentRelease(doc: *mut c_void);
        fn CGPDFPageRelease(page: *mut c_void);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
        ) -> sys::CGContextRef;
    }

    // 媒体框矩形（PDF 点，72/inch）：与文本层坐标同单位，供坐标映射缩放。
    #[repr(C)]
    struct CGBox {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    let url = CFURL::from_path(path, false).ok_or_else(|| format!("无法解析路径：{:?}", path))?;
    let doc = unsafe { CGPDFDocumentCreateWithURL(url.as_concrete_TypeRef()) };
    if doc.is_null() {
        return Err(format!("无法打开 PDF：{:?}", path));
    }
    let total = unsafe { CGPDFDocumentGetNumberOfPages(doc) };
    let page = unsafe { CGPDFDocumentGetPage(doc, 1) };
    // ⚠️ 必须先显式 retain 住 page，再释放文档：CGPDFDocumentGetPage **不保证**给
    //    调用方 +1（page 所有权可能仍归文档）。若直接释放文档，page 会随之释放，
    //    后续 CGPDFPageGetBoxRect / CGContextDrawPDFPage 即 use-after-free
    //    → EXC_BAD_ACCESS 闪退（这正是「导入即闪退」的头号嫌疑）。
    //    显式 Retain + 末尾一次 Release：在 +0/+1 两种约定下都不会越放，最稳。
    if !page.is_null() {
        unsafe { CGPDFPageRetain(page) };
    }
    unsafe { CGPDFDocumentRelease(doc) };
    if page.is_null() {
        return Err("PDF 第一页不存在".to_string());
    }
    // 媒体框（PDF 点）：坐标映射的基准尺寸，与文本层坐标同单位。
    let mb = unsafe { CGPDFPageGetBoxRect(page, 0) }; // kCGPDFMediaBox = 0
    let nat_w = mb.w;
    let nat_h = mb.h;
    if nat_w <= 0.0 || nat_h <= 0.0 {
        unsafe { CGPDFPageRelease(page) };
        return Err("PDF 页面尺寸为 0".to_string());
    }

    // ① 计算防护：先把超大尺寸按比例缩小，确保单页栅格化后
    //    最大边长 ≤ 4096px 且 宽*高*4 ≤ 64MB，避免 CGBitmapContext OOM / 闪退。
    const MAX_DIM: f64 = 4096.0;
    const MAX_BYTES: f64 = 64.0 * 1024.0 * 1024.0;
    let long = nat_w.max(nat_h);
    let dim_scale = if long > MAX_DIM { MAX_DIM / long } else { 1.0 };
    let area_scale = {
        let bytes = nat_w * nat_h * 4.0;
        if bytes > MAX_BYTES {
            (MAX_BYTES / bytes).sqrt()
        } else {
            1.0
        }
    };
    let scale = dim_scale.min(area_scale);
    // MediaBox 宽高是浮点数：一律 ceil() 向上取整，绝不截断——截断会让画布比
    // 实际内容窄 1px，行尾像素写到缓冲区外面去。
    let w = (nat_w * scale).max(1.0).ceil() as usize;
    let h = (nat_h * scale).max(1.0).ceil() as usize;

    let space = CGColorSpace::create_device_rgb();
    // 行距严格按 16 字节对齐（CoreGraphics 对位图行距有对齐要求）。若给非对齐值，
    // CG 会自行放大行距，与我们的换算不一致 → 极易越界。这里主动补齐，再在
    // 读取时用系统真实值（bytes_per_row）二次校验。
    let bpr = ((4 * w + 15) / 16) * 16;
    // ② 空指针处理：直接用 CGBitmapContextCreate 并判空，绕开 create_bitmap_context
    //    内部的 assert（其遇分配失败会直接 panic）。
    let raw = unsafe {
        CGBitmapContextCreate(
            ptr::null_mut(),
            w,
            h,
            8,
            bpr,
            space.as_ptr() as *mut c_void,
            1, // kCGImageAlphaPremultipliedLast
        )
    };
    if raw.is_null() {
        return Err(format!(
            "位图上下文分配失败（原图约 {}×{}，已缩到 {}×{} 仍失败，可能内存不足）",
            nat_w as u32, nat_h as u32, w, h
        ));
    }
    let mut ctx = unsafe { CGContext::from_ptr(raw) };
    // ⚠️ 绝不能再手动 Translate + Scale(1,-1) 翻转：CGContextDrawPDFPage 在位图
    //    上下文里本就以**正向**落笔，额外的 Y 翻转会把整页画成上下颠倒（沙箱探针
    //    实测：手动翻转 → 倒置；不翻转 / DrawingTransform → 正向）。
    //    改用官方 CGPDFPageGetDrawingTransform：把媒体框映射到目标矩形，并自动
    //    处理页面 /Rotate（0/90/180/270），缩放也一并交给它。
    let target = CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(w as f64, h as f64),
    );
    let rotation = unsafe { CGPDFPageGetRotationAngle(page) };
    let transform =
        unsafe { CGPDFPageGetDrawingTransform(page, 0, target, rotation, true) };
    unsafe {
        CGContextConcatCTM(ctx.as_ptr(), transform);
        CGContextDrawPDFPage(ctx.as_ptr(), page);
        CGPDFPageRelease(page);
    }

    // CoreGraphics 位图内存序为 BGRA，转成 img / PNG 需要的 RGBA。
    // ⚠️ 绝不直接下标切片：行距用系统返回的真实值（CGBitmapContextGetBytesPerRow，
    //    CG 可能按 16 字节对齐放大），区间一律 .get() 取；一旦越界优雅报错返回，
    //    绝不 panic / 越界写。
    let bpr_actual = ctx.bytes_per_row();
    let row_bytes = w * 4;
    if bpr_actual < row_bytes {
        return Err(format!(
            "位图行距异常（系统 bytes_per_row={} < 宽×4={}），放弃转换",
            bpr_actual, row_bytes
        ));
    }
    let data = ctx.data();
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let start = row * bpr_actual;
        let end = start + row_bytes;
        let Some(src) = data.get(start..end) else {
            return Err(format!(
                "位图数据越界（第 {} 行，区间 {}..{}，缓冲区长度 {}）",
                row,
                start,
                end,
                data.len()
            ));
        };
        for px in src.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]); // BGRA → RGBA
        }
    }
    drop(ctx);

    let out = temp_png_path();
    image::save_buffer(&out, &rgba, w as u32, h as u32, image::ColorType::Rgba8)
        .map_err(|e| format!("写入 PNG 失败：{}", e))?;

    // ③ 多页提示：首版仅渲染首页，页数 >1 时给温和提示（不报错）。
    let note = if total > 1 {
        Some(format!(
            "已载入第 1 页（共 {} 页），多页 PDF 当前仅渲染首页",
            total
        ))
    } else {
        None
    };
    Ok(Rasterized {
        path: out.clone(),
        temp: Some(out),
        nat_w,
        nat_h,
        note,
    })
}

#[cfg(not(target_os = "macos"))]
fn rasterize_source(_path: &Path) -> Result<Rasterized, String> {
    Err("原图导入仅在 macOS 支持".to_string())
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
