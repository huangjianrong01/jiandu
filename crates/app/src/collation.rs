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
use crate::share_layout::{
    clean_md, layout, Binding, Imposition, LayoutSpec, Paper, ShareFont,
};
use crate::share_paint::{render_pdf, render_pdf_spread, render_png, render_spread_png};
use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, rgba, Bounds, BoxShadow, Context, Div,
    EntityInputHandler, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, StatefulInteractiveElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ParentElement, Render,
    RenderImage, ScrollHandle, ScrollWheelEvent, SharedString, Stateful, Styled, Window,
    AnimationExt,
};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;
use std::path::{Path, PathBuf};

/// Pixels → f32：gpui 的 `Pixels` 字段在 jiandu crate 不可见，用「除 1px」取数值。
fn px_f32(p: Pixels) -> f32 {
    p / px(1.0)
}

/// 当前本地时间戳（紧凑 `YYYY-MM-DD HH:MM`），纯标准库实现，不引入额外依赖。
fn ts_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    };
    // 简单公历换算（2038 之后仍正确，足够本工具使用）。以 1970-01-01 00:00 UTC 为锚。
    const DAY: i64 = 86400;
    let days = secs / DAY;
    let rem = secs - days * DAY;
    // 已知 1970-01-01 是星期四，累计天数求年月日（蔡勒式无需，直接递推）。
    let mut y = 1970i64;
    let mut d = days;
    loop {
        let leap = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
            366
        } else {
            365
        };
        if d < leap {
            break;
        }
        d -= leap;
        y += 1;
    }
    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mut m = 0usize;
    let mut dd = d;
    loop {
        let md = month_days[m] + if m == 1 && leap { 1 } else { 0 };
        if dd < md {
            break;
        }
        dd -= md;
        m += 1;
    }
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m + 1,
        dd + 1,
        hh,
        mm
    )
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
/// **快捷标点栏**的高度（v1.9.25）。
///
/// 它是**布局常量**不是视觉常量：标点栏夹在状态条与正文之间，所以
/// `editor_origin()` 的纵向原点必须把它算进去——少算 44px，「点哪落哪」
/// 就整体上移一行。改这个数，两处（本行、`editor_origin`）自动同变。
const PUNCT_BAR_H: f32 = 44.0;
const PAD: f32 = 14.0;
const FONT_SIZE: f32 = 17.0;
/// 行高（v1.9.24）：17 × 1.8 ≈ 30.6。
///
/// 1.7 时四行字还是一块「面」，1.8 才散成一列「行」。这是纯样式的量，
/// 但有一个**隐性耦合**必须记住：`caret_from_point` / `render_line` 的
/// 选区带与光标高度全部由它推导，改这一个数，三处一起变——这正是要的。
const LINE_H: f32 = FONT_SIZE * 1.8;
/// **右侧编辑区的内边距**（v1.9.24）：28 → **40**。
///
/// 用户口径：「四周 Padding 至少 32」。取 40 是有原因的——它同时是
/// `editor_origin()` 里的横纵原点偏移，字摆得越靠里，「点哪落哪」的余量
/// 越大；而上下留白一旦宽过行距，一整屏文字才真正像「铺在纸上」。
/// ⚠️ 两处用的是**同一个常量**，所以改这里不需要再去改 `editor_origin`。
const ED_PAD: f32 = 40.0;
/// 近似字宽：原图转录以 CJK 为主，按字号取值做编辑器内点击定位（仅用于右文
/// 落光标 / 选区估算，不影响左图高亮精度——左图高亮走 `SourceRegion` 真实映射）。
const CHAR_W: f32 = FONT_SIZE * 0.95;

/// 估算一段文本在「每行约 `cpl` 字」下的视觉行数（对照模式段落对齐用）。
///
/// 中文近等宽，`whitespace_normal` 几乎处处可断，故按「字数 ÷ 每行字数」上取整即可；
/// 对 CJK 近似足够准（亚行级误差不影响段落锚定）。空段按 1 行计，与 GPUI 占位一致。
fn est_wrapped_lines(s: &str, cpl: usize) -> usize {
    let cpl = cpl.max(1);
    let n = s.chars().count();
    if n == 0 {
        1
    } else {
        n.div_ceil(cpl)
    }
}

/// **点 → 字符下标**（纯函数，v1.9.67）：屏幕点击与单测共用同一口径。
///
/// 正文是**软换行**的：一个逻辑行（`\n` 分隔）在屏幕上可能占 2~4 个视觉行。
/// 旧实现把「鼠标 y ÷ 行高」直接当**逻辑**行号用——只要有一段折行，后面所有
/// 行号就整体错位：点屏幕第 3 屏会被算成第 12 个逻辑行，光标瞬移到几十行外
/// （真机表现就是「点不出光标」，因为光标落在屏幕外），拖拽终点同样飞走
/// （「稍微一动就整段巨大色块」）。这里三步走：
///   ① y → **视觉行号**；② 逐逻辑行扣掉它占的视觉行数，落进哪一行就是哪一行；
///   ③ 行内列 = 该逻辑行内的第几条视觉行 × 每行字数 + x ÷ 字宽。
/// 越界一律夹紧到文末，绝不返回越界下标。
///
/// `adv` 是**实测单字进距**（v1.9.68）：由 `TextSystem` 量出的真值，不再用
/// `CHAR_W`（17 × 0.95）这个近似值。近似值每字差 5%，一行 60 字就累计 51px
/// ≈ 3 个字——这正是「点两个字之间、光标却插到旁边」的真因。
///
/// 列位取**最近的字缝**（`+0.5` 再取整）：点在某个字的左半 → 插到它前面，
/// 右半 → 插到它后面。用 `floor` 的话点在右半边也会跳到该字左侧，手感上
/// 总觉得「慢半拍、差一格」。
fn caret_at(text: &str, cpl: usize, adv: f32, local_x: f32, local_y: f32) -> usize {
    let cpl = cpl.max(1);
    let adv = if adv > 0.0 { adv } else { CHAR_W };
    let visual = (local_y.max(0.0) / LINE_H).floor() as usize;
    let col_in_visual = ((local_x.max(0.0) / adv) + 0.5).floor() as usize;
    let mut start = 0usize;
    let mut remain = visual;
    for line in text.split('\n') {
        let len = line.chars().count();
        let lines = est_wrapped_lines(line, cpl);
        if remain < lines {
            return start + (remain * cpl + col_in_visual).min(len);
        }
        remain -= lines;
        start += len + 1;
    }
    // 点在最后一行下方：落到文末（而不是行首，更不是越界）。
    text.chars().count()
}

/// **实测 CJK 单字进距**（v1.9.68）：让 `TextSystem` 真正排一行字，取平均宽度。
///
/// 为什么必须量：`CHAR_W` 是「字号 × 0.95」的估算，而中文全角字的真实进距就是
/// 1.0em（17px）。5% 的误差单看无感，但一行的列位是 `x ÷ 字宽` 累计出来的——
/// 60 字一行就差 51px（≈3 个字），点击时看着就是「插不进两个字之间」。
/// 用 32 个字取平均，把逐字的亚像素舍入误差摊平。
///
/// 量不到（字体未就绪 / 平台返回空）时退回 `CHAR_W`，绝不返回 0——
/// 0 会让 `chars_per_line` 除零、`caret_at` 全落到行首。
fn measure_char_adv(ts: &gpui::WindowTextSystem, family: &SharedString) -> f32 {
    const PROBE_N: usize = 32;
    let probe: String = "國".repeat(PROBE_N);
    let run = gpui::TextRun {
        len: probe.len(),
        font: gpui::font(family.clone()),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = ts.shape_line(probe.into(), px(FONT_SIZE), &[run], None);
    let w = px_f32(shaped.width);
    if w.is_finite() && w > 1.0 {
        w / PROBE_N as f32
    } else {
        CHAR_W
    }
}

/// **气泡锚点**（纯函数，v1.9.69）：内容原点 + 「选区末行的下一行」的行距。
///
/// 抽出来只为一件事——**可测**。v1.9.68 那次回归正是坐标写错（`- scroll_y`
/// 把负偏移又取反了一次），而错在 `render_*` 里的算式只能靠开窗口点鼠标验。
/// 现在的契约是：`origin` 是 `editor_origin()`（**已含滚动偏移**），所以
/// 滚动只该通过 `origin.y` 生效，这里绝不再自己加减滚动量。
fn bar_anchor(origin: (f32, f32), visual_idx: usize, col_in_visual: usize, adv: f32) -> (f32, f32) {
    let x = origin.0 + col_in_visual as f32 * adv + 4.0;
    let y = origin.1 + (visual_idx + 1) as f32 * LINE_H + 4.0;
    (x, y)
}

/// **把气泡夹进右栏矩形**（纯函数，v1.9.69）：任何输入都不许算到屏幕外。
///
/// 三种必须兜住的情形：选区在视口之上（y 会是负的）、选区在视口之下
/// （y 会远超面板）、窄窗口（x 顶到右缘）。夹紧策略是「贴边」而不是「隐藏」——
/// 用户刚拖完选，气泡跑到天边比贴边更让人以为坏了。
fn clamp_bar_pos(
    x: f32,
    y: f32,
    vw: f32,
    vh: f32,
    panel_l: f32,
    panel_t: f32,
    bar_w: f32,
    bar_h: f32,
) -> (f32, f32) {
    let min_x = panel_l + 8.0;
    let max_x = (vw - bar_w - 10.0).max(min_x);
    let min_y = panel_t + 4.0;
    let max_y = (vh - bar_h - 10.0).max(min_y);
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

fn est_wrapped_lines_n(n: usize, cpl: usize) -> usize {
    let cpl = cpl.max(1);
    if n == 0 { 1 } else { n.div_ceil(cpl) }
}

/// **版式行索引**（v1.9.69）——百万字正文的唯一几何真相。
///
/// ## 为什么非有它不可
/// 旧实现每一帧都做三件 O(全文) 的事：
///   ① `self.text.split('\n').collect::<Vec<_>>()`（几万行 → 每帧一次堆分配）；
///   ② 逐行 `line.chars().count()` 算视觉行（百万字 → 每帧扫一遍全本）；
///   ③ `line_ranges()` 再建一个 `Vec<(usize,usize)>`（又一次几万条分配）。
/// 静止时勉强能撑，一滚动 / 一拖选就掉帧——因为滚动是**每帧**都重排。
///
/// 现在把这三件事压成**一次**：文本或版心列数变了才重建（`gen` / `cpl` 做键），
/// 之后每帧只做二分查找，O(log n)。虚拟列表的「只画可见几十行」也因此才真正成立——
/// 没有前缀和，你根本不知道第 8000 行该落在哪个像素上。
///
/// 字段全是行级前缀和，`len = 行数 + 1`（末尾那格是总量，省一次 if）：
/// ・`char_start` / `byte_start`：每行在全文里的字符 / 字节起点（`text = …` 用得上）；
/// ・`vis`：累加**视觉行**数（软换行口径，与 `render_line` / `caret_at` 同源）；
/// ・`aligned`：对照模式的**段落对齐**口径（每段取 `max(清洗行数, 毛坯行数)`），
///   非对照模式与 `vis` 完全相同。
///
/// ⚠️ 行数用 `usize` 而不是 `u32`：百万字 ≈ 几万行，内存 4 个 Vec 合起来不到 1MB，
///    换掉一整类溢出边界，值得。
#[derive(Default, Clone)]
struct LineIndex {
    /// 建索引时的文本代际（`CollationWorkspace::text_gen`）。0 = 空索引。
    gen: u64,
    /// 建索引时的每行字数（列数变了，视觉行前缀和就全废）。
    cpl: usize,
    /// 建索引时是否处于对照模式（`aligned` 是否等于 `vis`）。
    diff: bool,
    /// 每行字符起点。
    char_start: Vec<usize>,
    /// 每行字节起点。
    byte_start: Vec<usize>,
    /// 视觉行前缀和（len = 行数 + 1）。
    vis: Vec<usize>,
    /// 段落对齐前缀和（len = 行数 + 1）。
    aligned: Vec<usize>,
    /// 毛坯（左栏 / 对照）每行的字节起点；非对照模式为空。
    ///
    /// 对照模式右栏要逐段比对 `raw_seg != clean_seg`（决定是否打朱砂底纹），
    /// 没有这张表就只能 `raw.split('\n').nth(i)`——那是 O(n) 的，一屏 60 行
    /// 就是 60 次全量扫描，比整帧预算还贵。
    raw_byte_start: Vec<usize>,
    /// 全文总字数（`chars().count()` 的结果，省掉每帧整本扫描）。
    total_chars: usize,
}

impl LineIndex {
    fn build(text: &str, raw: Option<&str>, cpl: usize, gen: u64, diff: bool) -> Self {
        let cpl = cpl.max(1);
        let mut idx = LineIndex {
            gen,
            cpl,
            diff,
            ..Default::default()
        };
        // 预留容量：按「平均 80 字一行」估（汉字 3 字节 → 240 字节一行），避开多次
        // 扩容。**用字节长估，不 `chars().count()`**——后者是白扫一遍全文，百万字
        // 每次重建多花两三毫秒，而这里只是个容量提示，估歪了也只是多占一点。
        let est_lines = (text.len() / 240).max(8);
        idx.char_start.reserve(est_lines);
        idx.byte_start.reserve(est_lines);
        idx.vis.reserve(est_lines + 1);
        idx.aligned.reserve(est_lines + 1);
        idx.vis.push(0);
        idx.aligned.push(0);

        let mut raw_iter = raw.map(|r| r.split('\n'));
        let mut cstart = 0usize;
        let mut bstart = 0usize;
        let mut rstart = 0usize;
        let (mut vis, mut al) = (0usize, 0usize);
        for line in text.split('\n') {
            let n = line.chars().count();
            idx.char_start.push(cstart);
            idx.byte_start.push(bstart);
            cstart += n + 1;
            bstart += line.len() + 1;
            let v = est_wrapped_lines_n(n, cpl);
            vis += v;
            // 对照模式：本段高度取两栏的较大者（与 `render_editor` 的填充口径一致）。
            let raw_line = raw_iter.as_mut().and_then(|it| it.next());
            if raw.is_some() {
                idx.raw_byte_start.push(rstart);
                rstart += raw_line.map(|r| r.len() + 1).unwrap_or(1);
            }
            let rv = raw_line
                .map(|r| est_wrapped_lines(r, cpl))
                .unwrap_or(v);
            al += v.max(rv);
            idx.vis.push(vis);
            idx.aligned.push(al);
        }
        // 每行都多算了一个 '\n'，末行没有 → 减 1 即总字数。
        idx.total_chars = cstart.saturating_sub(1);
        idx
    }

    fn valid(&self, gen: u64, cpl: usize, diff: bool) -> bool {
        self.gen != 0 && self.gen == gen && self.cpl == cpl && self.diff == diff
    }

    fn lines(&self) -> usize {
        self.char_start.len()
    }

    fn total_visual(&self) -> usize {
        *self.vis.last().unwrap_or(&0)
    }

    fn total_aligned(&self) -> usize {
        *self.aligned.last().unwrap_or(&0)
    }

    /// 字符下标 → 行号（二分，O(log n)）。
    ///
    /// 行尾那个「换行符位」算前一行（与 `line_ranges` 的 `s <= i <= e` 同口径），
    /// 否则光标停在行尾时会跳到下一行行首，`⇧→` 的手感立刻不对。
    fn line_of(&self, char_idx: usize) -> Option<usize> {
        if self.char_start.is_empty() {
            return None;
        }
        let li = self.char_start.partition_point(|&s| s <= char_idx);
        let li = li.saturating_sub(1).min(self.lines() - 1);
        let end = self.char_end(li);
        (char_idx <= end).then_some(li)
    }

    /// 第 `li` 行的字符终点（不含换行）。
    fn char_end(&self, li: usize) -> usize {
        match self.char_start.get(li + 1) {
            Some(&n) => n - 1,
            None => self.total_chars,
        }
    }

    /// 第 `li` 行的字符区间。
    fn char_range(&self, li: usize) -> (usize, usize) {
        (self.char_start[li], self.char_end(li))
    }

    /// 第 `li` 行的字节区间（`\n` 不在内）。
    ///
    /// 末行没有后续行起点可参照，只能拿 `text.len()` 收尾——`text` 必须是建索引的
    /// 那一份（调用点都传 `&self.text`）。
    fn byte_range(&self, text: &str, li: usize) -> (usize, usize) {
        let a = self.byte_start[li];
        let b = match self.byte_start.get(li + 1) {
            Some(&n) => n - 1,
            None => text.len(),
        };
        (a, b.max(a))
    }

    /// 第 `li` 行的 &str。
    fn line_str<'a>(&self, text: &'a str, li: usize) -> &'a str {
        let (a, b) = self.byte_range(text, li);
        text.get(a..b).unwrap_or("")
    }

    /// 对照模式：第 `li` 段对应的**毛坯**原文（越界返回空串）。
    fn raw_line<'a>(&self, raw: &'a str, li: usize) -> &'a str {
        let Some(&a) = self.raw_byte_start.get(li) else {
            return "";
        };
        let b = match self.raw_byte_start.get(li + 1) {
            Some(&n) => n.saturating_sub(1),
            None => raw.len(),
        };
        raw.get(a..b.max(a)).unwrap_or("")
    }

    /// 视觉行号 → 行号（二分，O(log n)）：虚拟列表的入口。
    fn line_at_visual(&self, v: usize) -> usize {
        Self::line_at_table(&self.vis, self.lines(), v)
    }

    /// 段落对齐视觉行号 → 行号（对照模式用）。
    fn line_at_aligned(&self, v: usize) -> usize {
        Self::line_at_table(&self.aligned, self.lines(), v)
    }

    fn line_at_table(table: &[usize], n: usize, v: usize) -> usize {
        if n == 0 || table.is_empty() {
            return 0;
        }
        let i = table.partition_point(|&x| x <= v);
        i.saturating_sub(1).min(n - 1)
    }
}

/// 把「一行内的列区间 `[a, b)`」按软换行切成若干**视觉行**片段，
/// 返回 `(视觉行序号, 起始列, 结束列)`（列号仍以逻辑行行首为 0）。
///
/// 选区底色必须**逐视觉行**画：整行画一块的话，一行文字只要折了行，色块就会
/// 盖住折行后的空白、压到下一段的行距上——真机看着就是「把段落 Padding 和
/// 行距都变成了丑陋的大色块」。切段之后每一块只覆盖真正有字的那一段。
fn visual_spans(a: usize, b: usize, cpl: usize) -> Vec<(usize, usize, usize)> {
    let cpl = cpl.max(1);
    let (a, b) = (a.min(b), a.max(b));
    let mut out = Vec::new();
    let mut p = a;
    while p < b {
        let k = p / cpl;
        let end = b.min((k + 1) * cpl);
        out.push((k, p, end));
        p = end;
    }
    out
}


// ── 「一张木桌，两张纸」（v1.9.24 视觉定稿）─────────────────────────
//
// 分屏不靠线，靠**明度差 + 一道柔影**：左纸（原图）压深一档，右纸（稿纸）最亮，
// 右纸左缘再挂一片极柔的外阴影——于是它看起来是**叠在**左纸上面的一张纸，
// 而不是被一条黑线劈开的两个窗格。三个色值只写在这里，别在别处再抄。
/// 右纸：极淡暖纸，正文的底。
const PAPER_R: u32 = 0xF7F0DC;
/// 左纸：同族压深一档。只用来和右纸拉开层次，不做任何装饰。
const PAPER_L: u32 = 0xDDD2B4;
/// 顶栏：与右纸同色但**不带边框**，靠一道 6% 墨的发丝线落地。
const CHROME: u32 = 0xF3EBD4;
/// 竖排预览各列数档位下的整页容量（由 `share_layout` 版式几何精算得出，
/// 预览固定 720×1280、不落款不批注：8→50 / 10→79 / 12→110 / 14→152）。
/// 每页喂入 ≤ 此值可保证 `layout` 不触发自动缩字号（share_layout.rs:346 的
/// `cols_max * rows < n` 永不成立），整本文稿字号恒定、版面一致，绝不前大后小。
/// 列数经 `norm_columns` 吸附到这四档，非档位值不会命中。
fn preview_page_cap(columns: usize) -> usize {
    match crate::share::norm_columns(columns) {
        8 => 50,
        10 => 79,
        12 => 110,
        14 => 152,
        _ => 110, // 不应命中：norm_columns 只产出 8/10/12/14
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
/// **PDF 导出的页数上限**（后台任务，故比预览宽松得多）。
///
/// 与预览的 6 页不是一回事：导出是「整本印出来」，只防手滑把整部丛书一次排进去。
const EXPORT_MAX_PAGES: usize = 300;
/// ③ 时间预算（毫秒）：累计渲染耗时超预算立即停手，留最后一张给 UI。
const PREVIEW_TIME_BUDGET_MS: u128 = 2500;
/// 预览图在滚动容器里的最大显示高度（px）。
///
/// ⚠️ 绝不能用 `h_full()`：`h_full` = 100% 父容器高，而这里父容器是
/// `overflow_y_scroll()`（高度由内容决定）→ 「容器高依赖内容、内容高依赖容器」
/// 的**布局反馈环**，会让 GPUI 反复求解尺寸直至卡死。给显式上限即可解耦。
const PREVIEW_IMG_MAX_H: f32 = 880.0;

/// 右侧编辑器可载入的**最大字数**（导入 .md / .txt 时生效）。
///
/// v1.9.69 起从 8 万字提到 **500 万字**：《資治通鑑》全本 ≈ 300 万字、《四庫全書》
/// 单种多在 100 万字以内，这个门限足够装下「一整部书」而不用按卷拆。
///
/// 敢提这么高，是因为渲染已经不是「一行一个 div 全铺」了：`render_editor` 现在
/// 只建**视口内那几十行**（行索引 + 前缀和，见 [`LineIndex`]），文本本身是
/// 一份 `String`，滚动是 `ScrollHandle` 的像素偏移——几百万字和几千字在渲染
/// 上没有任何区别。真正的成本只剩内存（500 万字 ≈ 15MB UTF-8）与一次 O(n)
/// 的建索引。
///
/// 超过此数仍然**截断并明确告知**，绝不静默吞掉。再往上就该按卷导入了：
/// 一亿字的东西不是「一本书」，而是「一个数据库」。
const TEXT_MAX_CHARS: usize = 5_000_000;
/// 导入文本的**硬上限**：超过直接拒绝，绝不把用户内存撑爆。
///
/// 产品边界（用户 2026-11-01 定，v1.9.69 复核）：
/// ・**不做「大于 8MB 就不让进」**——大部头古籍扫描件转出来的纯文本动辄十几
///   MB，一刀切拒绝等于「我的书导入不了」。判据永远是字数，不是字节。
/// ・真正的闸门放在这里：64MB。再大就不是「一本书」了（多半是选错了文件，
///   或者是数据库 / 日志），读进来只会把内存和界面一起拖死。
/// ・读盘按 `take(HARD + 1)` **流式**读，绝不先信 `metadata` 再整读——网络盘 /
///   iCloud 的 size 可能是 0 或说谎，信它就会一次性 read 出一个 2GB 的 Buffer。
const TEXT_IMPORT_HARD_BYTES: u64 = 64 * 1024 * 1024;

// 历史记录：这里曾定过一个「最低字面对合度」阈值（覆盖率 0.5），低于它就不画框
// 不滚动。真机上它造成的是最坏的一种失败——「右文和影印件明明是同一段话，却提示
// 不匹配，一个字的高亮都不出来」：覆盖率量的是「右文有多少被左文包含」，而 PDF
// 文本层常把一句话切成几截、OCR 又常漏字，比值天然偏低。
//
// v1.9.23 起改成「最长公共子串 ≥ LCS_MIN」判定（见 `locate_region`）：只问
// 「有没有一段连续 6 个字对得上」，不问「百分比够不够」。`bigram_overlap` 作为
// 校对的**诊断量**保留，但不再参与放行决策。

/// **康熙部首 → 通用汉字** 折叠表（214 字，按 `U+2F00` 起的码点顺序逐位对齐）。
///
/// ## 为什么需要它
///
/// PDF 文本层里经常混进**康熙部首**而不是正体汉字：真机抓到的那一页 PDFKit 抽出来
/// 的是 `⾐`（U+2F90）、`⼥`（U+2F25）、`⼰`（U+2F70），而右文写的是 `衣` `女` `己`
/// （U+8863 / U+5973 / U+5DF1）。两个码点长得一模一样，`==` 却为假——于是**字面
/// 完全相同的两句话，判成不匹配**，高亮自然出不来。
///
/// 成因是字体子集反查：这批 PDF 没有 ToUnicode，PDFKit 只能拿字形反查字体 cmap，
/// 反查表里同一枚字形对应多个码点时，它可能撞上部首码点。
///
/// 表来自 Unicode 的兼容分解（`NFKC` 会把 U+2F00–U+2FD5 逐个映射到对应汉字），
/// 生成方式是脚本而不是手抄，故与标准逐字一致；这里只存结果，**不引依赖**。
const KANGXI_RADICALS: &str = "一丨丶丿乙亅二亠人儿入八冂冖冫几凵刀力勹匕匚匸十卜卩厂厶又口囗土士夂夊夕大女子宀寸小尢尸屮山巛工己巾干幺广廴廾弋弓彐彡彳心戈戶手支攴文斗斤方无日曰月木欠止歹殳毋比毛氏气水火爪父爻爿片牙牛犬玄玉瓜瓦甘生用田疋疒癶白皮皿目矛矢石示禸禾穴立竹米糸缶网羊羽老而耒耳聿肉臣自至臼舌舛舟艮色艸虍虫血行衣襾見角言谷豆豕豸貝赤走足身車辛辰辵邑酉釆里金長門阜隶隹雨靑非面革韋韭音頁風飛食首香馬骨高髟鬥鬯鬲鬼魚鳥鹵鹿麥麻黃黍黑黹黽鼎鼓鼠鼻齊齒龍龜龠";

/// 表按码点顺序排，取下标用。
static KANGXI_FOLD: once_cell::sync::Lazy<Vec<char>> =
    once_cell::sync::Lazy::new(|| KANGXI_RADICALS.chars().collect());

/// 把一个可能是**康熙部首**的字折叠成通用汉字；其余字符原样返回。
///
/// `U+2E80–U+2EFF`（CJK 部首补充）里只有两个字有标准兼容分解，一并收进来；
/// 折叠是幂等的（结果都是通用汉字），所以两边都折一遍也不会出错。
fn fold_radicals(c: char) -> char {
    let cp = c as u32;
    if (0x2F00..=0x2FD5).contains(&cp) {
        return KANGXI_FOLD
            .get((cp - 0x2F00) as usize)
            .copied()
            .unwrap_or(c);
    }
    match cp {
        0x2E9F => '母', // ⺟ → 母
        0x2EF3 => '龟', // ⻳ → 龟
        _ => c,
    }
}

/// 计算右文段落在原图区域文字里的**连续二字对覆盖率**（0.0–1.0）。
///
/// 不用单字重合：中文常见字（之 / 不 / 而）单字命中率虚高，会把不相干的文本判定
/// 成匹配；二字对（bigram）要求相邻两字连续出现，抗巧合能力强得多。
/// 比对用「净文本」：先按 Markdown 洗一遍，再只留汉字 / 字母 / 数字。
///
/// 校对的常态是「左 PDF 文本层（或 OCR）纯文本 ↔ 右转录」，而右转录可能是
/// Markdown：`#`、`**`、`>`、`---`、表格的 `|` 全在。拿带记号的串去和纯文本做
/// 连续二字比对，命中率会被这些噪声直接拉到阈值以下 —— 表现就是「明明同一段话，
/// 却提示不匹配、高亮不出来」。
///
/// 所以两道洗：① `share_layout::clean_md` 剥掉全部 Markdown 记号（与竖排预览
/// 用的是**同一套**清洗，绝不让「预览里干净、匹配时装了记号」两套标准并存）；
/// ② 再滤掉标点 / 空白 / 符号 —— PDF 文本层常常没有标点，OCR 也常错标点，
/// 留着它们只会制造假阴性。汉字本身就是 `is_alphanumeric()`，故 CJK 全保留。
///
/// 比对用的「净文本」：洗 Markdown 记号、折康熙部首、只留汉字/字母/数字。
fn match_text(raw: &str) -> String {
    crate::share_layout::clean_md(raw)
        .text()
        .chars()
        .map(fold_radicals)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn bigram_overlap(seg: &str, region: &str) -> f32 {
    let s: Vec<char> = match_text(seg).chars().collect();
    let r = match_text(region);
    if s.len() < 2 || r.is_empty() {
        return 0.0;
    }
    let mut hit = 0usize;
    for w in s.windows(2) {
        let pair: String = w.iter().collect();
        if r.contains(&pair) {
            hit += 1;
        }
    }
    hit as f32 / (s.len() - 1) as f32
}

/// 判定「这段右文就是原图那一处」所需的最短**连续重合字数**。
///
/// 为什么不用覆盖率阈值：OCR / 文本层会把一段话切成若干短行，右文一行可能只
/// 对应某区域的前半句。覆盖率（`bigram_overlap`）在这种「包含但不相等」的情形
/// 下天然偏低，于是同一段话被判成不匹配——正是真机反馈的「明明一模一样却高亮
/// 不出来」。改成「最长公共子串 ≥ 6 字」：既承认包含关系，又不会因为「之乎者
/// 也」这类高频字偶然连上两三个字就误判。
const LCS_MIN: usize = 6;

/// 求最长公共子串时的字符上限（一页文本层正常几百字；封顶防 O(n·m) 退化）。
const LCS_CAP: usize = 600;

/// 最长公共**子串**（连续）长度——经典 DP，两行滚动，不分配 n×m 矩阵。
///
/// ⚠️ 这里要的是**子串**不是**子序列**。子序列允许跳字，`甲乙丙` 三字若散落在
/// 一整页里就算「连续重合 3 字」，判据立刻失效（中文页面上「之乎者也」之类的
/// 高频字散落重合太容易了）。故递推式在不等时**归零**：
///
/// ```text
/// cur[j+1] = if a[i] == b[j] { prev[j] + 1 } else { 0 }
/// ```
///
/// 真正的定位需要「原图里也有一段一模一样连着的字」，归零才是这个意思。
fn lcs_len(a: &[char], b: &[char]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut prev = vec![0usize; b.len() + 1];
    let mut cur = vec![0usize; b.len() + 1];
    let mut best = 0usize;
    for &ca in a {
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = if ca == cb { prev[j] + 1 } else { 0 };
            best = best.max(cur[j + 1]);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    best
}

/// 在 `regions` 里找与本行文字**最像**的那一处。返回 `(区域下标, 连续重合字数)`。
///
/// 这是「点哪高亮哪」的地基，**不再假设索引对齐**。原先按 `regions[行号]` 取框，
/// 隐含前提是「原图文本层切出来的段数 ≈ 右文行数」——真实 PDF 一页几十行、
/// 右文几百行，这个前提从第一页起就不成立：取不到 → 点了毫无反应；偶尔取到
/// 也是错位的一处 → 框落在不相干的字上（真机反馈的「高亮框飘了」）。
///
/// 改成按文字找之后，段数多少、顺序如何都不影响；跨页找不到时由调用方给话。
fn locate_region(line_text: &str, regions: &[SourceRegion]) -> Option<(usize, usize)> {
    let needle: Vec<char> = match_text(line_text).chars().take(LCS_CAP).collect();
    // 一个字（或纯标点）没法定位：这不是「未匹配」，是「这行没内容」，
    // 由调用方区分，免得空行点一下就弹一条红字。
    if needle.len() < 2 {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    for (i, r) in regions.iter().enumerate() {
        let hay: Vec<char> = match_text(&r.text).chars().take(LCS_CAP).collect();
        if hay.is_empty() {
            continue;
        }
        let n = lcs_len(&needle, &hay);
        if best.map(|(_, b)| n > b).unwrap_or(true) {
            best = Some((i, n));
        }
    }
    best.filter(|&(_, n)| n >= LCS_MIN)
}


/// 文本区域结构定义在 `pdf_text::SourceRegion`（PDF 文本层抽取的数据源）。
///
/// 真实接入时由带文本层的 PDF 填充（每个识别片段一条）；无文本层（扫描图）时
/// 退回 `mock_regions()` 假数据演示机制。右文第 i 段 ↔ `regions[i]`
/// （转录逐行对齐原图行）。坐标恒为原图自然像素（PDF 点），显示时按
/// `source_size()` 的缩放比换算，绝不写死像素。
use crate::pdf_text::SourceRegion;

/// 一次编辑的**最小可逆描述**（v1.9.66 · 撤销/重做）。
///
/// 为什么不是整本快照：这是一本「几百万字」的校对工具。整本快照一步就是
/// 1~6 MB（UTF-8 中文三字节），存 50 步就上百 MB，用户按 ⌘Z 三下内存就翻倍——
/// 那是玩具的做法。这里只存**这次编辑真正动过的那几个字**：
/// 「在 pos 处，把 removed 换成了 inserted」。一次按键通常几十字节，
/// 500 步也就几十 KB，与正文长度无关。
#[derive(Clone, Debug)]
struct EditOp {
    /// 起点（**字符**下标，不是字节——中文一个字三字节，用字节索引必错位）。
    pos: usize,
    /// 被这次编辑**移除**的原文（撤销时原样插回）。
    removed: String,
    /// 被这次编辑**插入**的新文（撤销时按字符数删掉）。
    inserted: String,
    /// 合并类别：同类的连续编辑合成一步（连打字 = 一步，不是一字一步）。
    kind: EditKind,
    /// 记录时刻，仅用于「合并时间窗」判定。
    at: std::time::Instant,
}

impl EditOp {
    /// 尝试把「紧接着的又一次编辑」吸收进本步，成功返回 `true`。
    ///
    /// 这是撤销栈好用的关键：逐字键入若一字一步，按一次 ⌘Z 只退一个字，
    /// 用户会以为撤销坏了。吸收的判据是**几何**的——新编辑必须落在本步
    /// 已插入的那段文字之内 / 紧邻其后（情形 A），或者两边都是纯删除且
    /// 首尾相接（情形 B/C）。光标一旦被方向键或点选挪走，几何条件自然
    /// 不成立，于是自动分步，不需要任何「编辑会话」状态机。
    fn try_absorb(&mut self, pos: usize, removed: &str, inserted: &str) -> bool {
        let ins: Vec<char> = self.inserted.chars().collect();
        let rem: Vec<char> = self.removed.chars().collect();
        let r = removed.chars().count();
        let i = inserted.chars().count();

        // 情形 A：新编辑发生在「本步插入的文字」区间内（连续打字 / 输入法逐帧组字）。
        if pos >= self.pos {
            let k = pos - self.pos;
            if k <= ins.len() && k + r <= ins.len() {
                let mut merged: String = ins[..k].iter().collect();
                merged.push_str(inserted);
                merged.extend(ins[k + r..].iter());
                self.inserted = merged;
                return true;
            }
        }
        // 情形 B：两边都是纯删除，且新删的正好紧贴在本步之前（连按退格）。
        if ins.is_empty() && i == 0 && pos + r == self.pos {
            let mut merged: String = removed.chars().collect();
            merged.extend(rem.iter());
            self.removed = merged;
            self.pos = pos;
            return true;
        }
        // 情形 C：纯删除且起点相同（连按 Delete 向后删）。
        if ins.is_empty() && i == 0 && pos == self.pos {
            let mut merged = self.removed.clone();
            merged.push_str(removed);
            self.removed = merged;
            return true;
        }
        false
    }
}

/// 撤销步的类别（v1.9.66）：只有同类别才可能合并。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditKind {
    /// 逐字键入（连打合并成一步）。
    Insert,
    /// 退格（向前删，连按合并成一步）。
    Backspace,
    /// Delete 键（向后删，连按合并成一步）。
    Delete,
    /// 一次性动作：删选区 / 包裹符号 / 整段粘贴 / 整段 AI 补全——永不合并。
    Once,
    /// 输入法组字：逐帧 marked text 会合并成「这一次输入」。
    Ime,
}

/// **清洗审计的一次算完结果**（v1.9.70）。
///
/// 为什么要有这个缓存：
///  ① **绝不在 `render` 里清洗全文**——旧实现每帧 `clean(&raw)`，三百万字就是
///     每帧全量清洗一遍，面板一开界面就躺平；
///  ② **字数只算正文**——`[注]` / `[/注]` 是结构标记（每处多吃 7 个字符），
///     把它们算进字数，就会出现「删了 3 万字却显示净增 31829」这种假账。
///     真正的判据是 [`jiandu_core::cleaner::body_char_count`]。
#[derive(Clone, Copy, Debug, Default)]
struct SanitizeCard {
    /// 毛坯**正文**字数（已剔标签 / 空白 / 不可见字符）。
    raw_body: usize,
    /// 清洗后**正文**字数（同上口径）。
    clean_body: usize,
    ghost: usize,
    variant: usize,
    notes: usize,
    /// 被剔除、不计入字数的夹注标签字符数（`[注]` 3 + `[/注]` 4，按实际出现次数）。
    tag_chars: usize,
}

impl SanitizeCard {
    fn delta_label(&self) -> String {
        let d = self.clean_body as i64 - self.raw_body as i64;
        if d >= 0 {
            format!("净增 {d}")
        } else {
            format!("净减 {}", -d)
        }
    }
}

/// 连续编辑合并的时间窗（毫秒）。超过就认为是「另一件事」，另起一步。
const UNDO_COALESCE_MS: u64 = 700;/// 撤销栈上限。500 步覆盖任何一次校对会话，内存恒定在几十 KB 量级。
const UNDO_MAX_STEPS: usize = 500;

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
    /// 对照模式（show_diff）左右**共用**的滚动柄：左栏毛坯与右栏清洗正文双向同步。
    /// 与 `scroll`（编辑器常态柄）分离——编辑器在常态/纯文本模式仍走 `scroll`，
    /// 那条路径已验证可编辑；对照模式单独挂 `diff_scroll`，互不干扰。
    diff_scroll: ScrollHandle,
    /// 鼠标拖拽选择中的锚点（字符下标）。
    anchor: usize,
    /// 是否正在拖拽选择。
    dragging: bool,
    /// **按下点**（窗口坐标，v1.9.67）：拖选只认「在本编辑器里按下过的这一只手」。
    ///
    /// 之前 `extend_selection_from_drag` 只判「左键按着 + 鼠标在动」，于是**任何**
    /// 左键拖动（哪怕按下点在标点栏 / 空白 / 别的栏）都会拿旧的 `anchor` 去扩展——
    /// 锚点若是 0，一拖就是「从文首到鼠标」的整段大色块，这就是真机说的
    /// 「稍微一动就整段全选」。现在按下时记原点、离开原点超过阈值才允许起选，
    /// 抬起即清空：没有按下点就绝不改选区。
    drag_origin: Option<(f32, f32)>,
    /// 竖排预览开关（骨架阶段只切占位浮层；真实栅格化在下一步）。
    show_preview: bool,
    /// 位图原图（真正的 PNG / JPG）路径：由 `gpui::img` 自己解码，不预栅格化。
    /// 与 `image_mem` 二选一，谁有值就画谁。
    image_path: Option<PathBuf>,
    /// PDF 页的**内存位图**（v1.9.30）。
    ///
    /// 这是左栏手感的关键：原先 PDF 页要先编成 PNG 落盘、再让 `gpui::img` 异步
    /// 读盘解码贴图——一次翻页就是「CG 画 → 转 RGBA → PNG 编码 → 写盘 → 读盘 →
    /// PNG 解码」六趟，其中编解码两趟是纯浪费。`gpui` 本来就支持直接吃
    /// `Arc<RenderImage>`（`ImageSource::Render` 分支零开销），于是把 CG 画好的
    /// 像素直接留在内存：翻页只剩「画 + 上传纹理」。
    ///
    /// 顺带解决了清晰度问题：没有了 PNG 的体积顾虑，栅格化就敢按**显示尺寸**
    /// 出图（见 [`raster_px`]），而不必把 300dpi 的影印本压到 72dpi。
    image_mem: Option<Arc<RenderImage>>,
    /// 待回收的内存位图（v1.9.30）：换页时把旧页挪进来，**下一帧** render 开头
    /// 统一 `window.drop_image(...)` 释放它的 GPU 纹理。
    ///
    /// ⚠️ 这一条不能省。gpui 把 `RenderImage` 按 `ImageId` 上传到 sprite atlas，
    /// 而 `ImageSource::Render` 的 `remove_asset` 是**空实现**——不显式
    /// `drop_image`，翻 100 页就留下 100 张 1120×1679 的纹理常驻显存（每张约
    /// 7.5MB，翻几百页就是几个 G）。隔一帧再删是照 `temp_gc` 的老规矩：本帧的
    /// 场景可能还引用着旧图，等元素树换过再删才安全。
    image_gc: Vec<Arc<RenderImage>>,
    /// 上一张临时栅格化 PNG（PDF 转图产生）；载入新图时删除以免堆积。
    last_temp: Option<PathBuf>,
    /// 导入失败信息（红色提示）。
    image_error: Option<String>,
    /// 非错误的温和提示（如多页 PDF 仅渲首页）。
    image_note: Option<String>,
    /// 竖排预览（离屏栅格化）各页位图路径（按页序；均为临时 PNG，重生成时清空）。
    preview_paths: Vec<PathBuf>,
    /// 待删临时 PNG：关闭预览 / 重排时把旧路径挪进来，**下一帧** render 开头统一删盘。
    ///
    /// 不当场删是因为本帧的 `img` 可能还在异步读该文件；隔一帧元素树已换，删了安全。
    temp_gc: Vec<PathBuf>,
    /// 当前这批预览图对应的版式指纹（`preview_sig`）；相同请求直接跳过不重排。
    preview_sig: Option<(u64, usize, u8, u8, u8, u8, u8, u8)>,
    /// 当前预览页码（0-based）；`preview_paths` 为空时无意义。
    preview_page: usize,
    /// 预览栅格化失败信息（红色提示，如系统缺中文字体）。
    preview_error: Option<String>,
    /// 预览的温和提示（非错误）：如「长稿仅预览前 900 字 / 前 6 页」。
    preview_note: Option<String>,
    /// 预览是否正在后台排版（true 时不叠加任务，只挂 pending）。
    preview_busy: bool,
    /// 排版期间又有新请求（连点 / 连改样式）→ 当前任务回来后补跑一次。
    preview_pending: bool,
    /// 预览代际令牌：每次发起请求 +1，旧代际的结果回来即丢弃（含临时文件）。
    preview_gen: usize,
    /// 竖排预览样式：底色（宣纸黄 / 暗金黑 / 极简白 / 仿刻本）。
    preview_bg: ShareBg,
    /// 竖排预览样式：版框（红线框 / 黑线框 / 无边框 / 双边）。
    preview_frame: ShareFrame,
    /// 竖排预览样式：列数（8 / 10 / 12 / 14，经 norm_columns 锁定）。
    preview_columns: usize,
    /// **纸张口径**（v1.21.0）：预览与 PDF 导出共用同一个选择。
    ///
    /// `Card` = 9:16 的屏幕取景卡（沿用老观感，不可导出）；`A4 / A3` 是真实纸张，
    /// 尺寸一律经 `Paper::geom()` 由毫米算出——本文件里不写任何画布魔数。
    preview_paper: Paper,
    /// **拼版方式**（v1.22.0）：`Single` 一页一张纸；`Spread` 双页拼版（书籍折页）。
    ///
    /// 与 `preview_paper` 是两个正交的旋钮：纸张说的是「用多大的纸」，拼版说的
    /// 是「这张纸上摊几页」。A3 + Spread 才是印厂要的折页；A3 + Single 是放大
    /// 单页（用户点名的「大字海报」，只在明确要单页大图时才用）。
    imposition: Imposition,
    /// **装订模式**（v1.22.0）：右开本（古籍 · 奇左偶右）/ 左开本（現代 · 偶左奇右）。
    ///
    /// 只在双页拼版时起作用：决定奇数页落在左半边还是右半边。默认右开本
    /// （用户 2026-11-01 明令，古籍是右开本）。
    binding: Binding,
    /// PDF 导出是否在途（后台排版 + 矢量落盘，禁止重入）。
    export_busy: bool,
    /// 导出结果 / 错误（浮层底部状态条，与预览状态条同一套视觉语言）。
    export_note: Option<String>,
    /// 样式选择面板是否展开。
    show_style_panel: bool,
    /// **原图高亮框显隐**（v1.9.25）——顶栏「注」那一枚说了算，默认开。
    ///
    /// 关掉时左图只剩底图：区域参考框与联动高亮矩形一概不画。它是「校对
    /// 时的注释层」，读图看图时嫌它挡字就一键收掉——与阅读器里那枚「注」
    /// 同名义、同手感（只不过那边关的是纸上的夹注与眉批）。
    ///
    /// 纯渲染开关：`active_para_idx` / `regions` 一个不动，框随时能再长回来。
    show_region_box: bool,
    /// 当前联动的**右文行号**（0-based，行 = `text` 里 `\n` 分隔的第几行）。    ///
    /// 与下面的 `active_para_idx` 是一对「源 → 靶」：这里是**用户点的哪一行**，
    /// 那里是**左图命中的哪一处区域**。分开记的原因：命中不了时（跨页 / 扫描版 /
    /// 稿子对不上）仍需如实显示「你点的是第几行」，否则用户点了没反应，连
    /// 「系统知道我在点哪」都无从判断。
    active_line: Option<usize>,
    /// 当前联动命中的原图区域下标（0-based）。`None` = 这一行没命中。
    ///
    /// ⚠️ 它**不再等于行号**（v1.9.23）：由 `locate_region` 按文字查出来，故右文
    /// 行数与原图文本层段数不同、顺序不同都无所谓。
    active_para_idx: Option<usize>,
    /// 左图原图自然像素宽（由栅格化结果填充；无原图时由区域包围盒推导）。
    src_nat_w: f32,
    /// 左图原图自然像素高（由栅格化结果填充；无原图时由区域包围盒推导）。
    src_nat_h: f32,
    /// 左栏**实际显示宽度的设备像素**（`vw * 0.4 * 屏幕缩放`，v1.9.30）。
    ///
    /// 由 `render` 每帧刷新（那里才拿得到 `window.scale_factor()` 与 `vw`），
    /// 栅格化时读它决定画布多大：取「够用就好」的像素数，既不为 Retina 欠采样
    /// 发糊，也不为 4K 白烧内存。0 = 还不知道（首帧），此时退回 1:1。
    left_px: f32,
    /// 联动提示（非错误）：如「该段与原图此处文字不匹配，已跳过定位」。
    match_note: Option<String>,
    /// 排障用帧计数（每 120 帧打一次日志，狂刷 = 存在无限重绘循环）。
    frame_count: usize,
    /// 原始 PDF 路径（多页翻页的唯一凭据）。`None` = 当前左图不是 PDF（位图原图 /
    /// 尚未导入），翻页时左侧不动。
    pdf_path: Option<PathBuf>,
    /// 左侧 PDF 当前**渲染中 / 已渲染**的页码（1-based）。
    ///
    /// 与 `preview_page`（右侧文本预览的页）是**两个独立状态**，`flip_page` 负责
    /// 让它们一起走——用户按一次 →，左右各进一页，这才是「双屏校对的灵魂」。
    pdf_page: usize,
    /// 左侧 PDF 总页数（≤1 时翻页键对左侧无效）。
    pdf_pages: usize,
    /// 左图是否正在后台重栅格化（翻页时显示 Loading，不阻塞任何键）。
    page_busy: bool,
    /// 左图栅格化的代际令牌：连按方向键时，旧页结果回来即丢弃（绝不闪回）。
    page_gen: usize,
    /// PDF 页缓存：页码（1-based）→ 位图 + 该页文本层坐标（v1.9.31）。
    ///
    /// 翻页的丝滑感全在这里。单次栅格化约 12ms、文本层 10–40ms，单看都不长，
    /// 但按住 → 连翻时它们是**串行**压在每一次换页上，手会明显「追不上」。
    /// 缓存 4 页后，来回对照阅读（校对最常干的事）几乎零等待。
    ///
    /// 与 `image_mem` 的分工：`image_mem` 是**此刻正在显示**的那一页（决定纹理
    /// 生命周期），这里存**最近看过的**几页的 CPU 位图。命中时只是换个 `Arc`，
    /// 不重新栅格化、不重新抽文本层。
    pdf_cache: std::collections::HashMap<usize, CachedPage>,
    /// 是否有预取在途（同时只允许一个）。
    ///
    /// 连翻十页时若每页都起一个预取，后台线程池会被十份栅格化塞满，反而拖慢
    /// 用户真正在等的那一页。一个在途就够：预取 12ms 就完成，手再快也追不上。
    prefetch_busy: bool,
    /// 下一次 render 要不要**强制重建输入焦点**（true 时先 blur 再 focus）。
    ///
    /// 为什么必须有这个开关：`rfd` 是原生 NSOpenPanel，它关掉之后窗口的 first
    /// responder 已经被系统收走；而 GPUI 的 `Window::focus()` 头一句就是
    /// `if self.focus == Some(handle.id) { return }`——它以为还聚焦着，于是永远
    /// 不再把输入上下文交还给平台。结果就是「刚导入完的文件，右侧编辑器看着好好的，
    /// 却一个字都打不进去」。唯一解法是显式 `blur()` + `focus()`，让焦点 id 变一次，
    /// 从而重走一遍「注册 first responder / 输入法上下文」的路径。
    refocus: bool,
    /// 原图文本层 / OCR 识别出的文本区域（自然像素坐标）。真实接入时由 PDF 文本层
    /// 或 OCR 引擎填充；MVP 由 `mock_regions()` 提供假数据跑通映射数学。
    regions: Vec<SourceRegion>,
    /// 右侧参与校对的 **Markdown / 纯文本源码文件**（导入后记录，仅作状态显示）。
    ///
    /// ⚠️ 编辑器里显示的是**源码原件**（`#`、`**`、`---` 都在）——用户校的是
    /// 原文，改的也是原文；把记号偷偷洗掉会让「所见即所改」失真。清洗只发生在
    /// 竖排预览出图那一刻（`share_layout::clean_md`）。
    text_file: Option<PathBuf>,
    /// 文本导入的提示 / 错误（工具栏下方一行；`text_err` 决定红字还是朱字）。
    text_note: Option<String>,
    /// `text_note` 是否是错误（红色）。
    text_err: bool,
    /// **批量识别队列**（v1.9.25）：一次选中的多个 PDF / 图片。
    ///
    /// 只排队、不消费——产品目前不内置 OCR（带文本层的 PDF 走 `pdf_text`
    /// 直接抽坐标，扫描件要等外部识别）。`jiandu_core::ocr::OcrQueue` 是纯
    /// 状态机，接 API 时写一个消费循环即可，这块 UI 一行都不用改。
    ocr_queue: jiandu_core::ocr::OcrQueue,
    /// 队列面板是否展开（队列为空时恒不画）。
    ocr_panel: bool,
    /// 队列的一次性提示（如「3 件已入队 · 2 件不支持，已跳过」）。
    ocr_note: Option<String>,
    /// 左图**正在拖选**的起点（原图自然坐标）。`None` = 没在拖。
    pick_from: Option<(f32, f32)>,
    /// 左图拖选出来的矩形（自然坐标，已归一）。它同时是「选区框」的绘制依据。
    pick_rect: Option<crate::pdf_text::PickRect>,
    /// 拖选命中区域拼出来的**待补文字**；`None` = 没有可补的东西。
    ///
    /// 存下来而不是每次重算：一次拖选要遍历整页区域（几百条），每帧都算一遍
    /// 会让「拖动中」掉帧；而且它也是「补入」按钮的唯一依据——没有它就说明
    /// 这次框选没选中字，按钮不该出现。
    pick_text: Option<String>,
    /// 本次拖选命中的区域数（只用于状态提示「共 N 处」）。
    pick_hits: usize,
    /// **问小笺面板**（v1.9.54）：与阅读器**共用同一个 Entity**——
    /// 会话、配置、流式输出全在同一处，用户在哪儿问都是同一位先生。
    ai_panel: gpui::Entity<crate::ai::AiPanel>,
    /// 问小笺浮卡是否展开。
    ai_open: bool,
    /// **掩注开关**（v1.9.54，Cmd+N）：竖排预览 / 导出时剥掉括注（夹注）小字，
    /// 只留正文。纯派生状态——`text` 一个字都不动，故随时能切回来。
    hide_gloss: bool,
    /// **纯文本模式**（v1.9.59，一键笺校）：本书没有可栅格化的原图（PDF/PNG），
    /// 左栏原图区隐藏、右栏正文全屏，顶栏提示「纯文本模式」。
    pure_text_mode: bool,
    /// **替换确认**（v1.9.59）：点【替换当前书籍】若已有校对内容，先弹此确认框。
    replace_confirm: bool,
    /// 清洗审计（v1.9.60）：导入时的「毛坯文本」（已提取段落、未清洗），
    /// 供【原文对照】逐字比对，防误杀。无源文件可解析时为 `None`。
    raw_text: Option<String>,
    /// 是否开启【原文对照】（左栏毛坯 / 右栏清洗后）。
    show_diff: bool,
    /// 是否弹出【清洗日志】内嵌面板（极简统计浮卡，不离开 App）。
    show_sanitize_panel: bool,
    /// 持久化锚点（v1.9.64）：本书在 `chapters.db` 的 `store_id`。
    /// `None` = 临时样张（演示用千字文，没有 `store_id`），不落 `collation_state`。
    store_id: Option<String>,
    /// 防抖落盘代际令牌（v1.9.64）：每次 `touch` +1，定时器触发时校验是否过期，
    /// 过期即丢弃——保证 1.5 秒窗口内**只写最后一次**状态（停手才落盘）。
    save_gen: usize,
    /// **撤销栈**（v1.9.66）：操作级 diff（见 [`EditOp`]），绝不存整本快照。
    /// 顺序即时间序，栈顶 = 最近一步。
    undo_stack: Vec<EditOp>,
    /// **重做栈**：撤销时把弹出的步压进来；任何**新的**编辑都会清空它（分叉即弃）。
    redo_stack: Vec<EditOp>,
    /// 是否已注册「退出前强存」钩子（v1.9.66）。注册是幂等的，但仍加闸门
    /// 避免每帧重复 subscribe（Duplicated 回调 = 退出时同一行写十几次）。
    quit_hooked: bool,
    /// **合并断点**（v1.9.66）：光标被方向键 / 点选 / 拖选挪过之后置位一次。
    ///
    /// 不置位的话，「打两个字 → 点回开头 → 再打两个字」会被几何判据并成一步，
    /// 一次 ⌘Z 把两处不相干的编辑一起退掉——这在百万字校对里是灾难。
    /// 置位后 `record_edit` 会强制另起一步，然后自动复位。
    undo_barrier: bool,
    /// 帮助浮窗开关（v1.9.66）：顶栏【？】唤出的毛玻璃说明卡。
    show_help: bool,
    /// **实测单字进距**（v1.9.68，单位 px）。由 `TextSystem::shape_line` 真量一次，
    /// 之后全程复用。`0.0` = 还没量到（此时退回 `CHAR_W` 近似值）。
    ///
    /// 近似值 `CHAR_W = 字号 × 0.95` 与中文真实的 1.0em 进距差 5%：一行 60 字
    /// 就累计 51px ≈ 3 个字——这正是「点两个字之间、光标却插到旁边」的真因。
    char_adv: f32,
    /// **快捷标点栏是否滑出**（v1.9.68）：鼠标靠近右栏顶部才显形，移开即隐。
    /// 纯文本模式下一整条 44px 的色带常年占着版面，会把「沉浸阅读」的纸感毁掉。
    punct_bar_shown: bool,
    /// 标点栏滑出动画的代际：每次「隐 → 显」+1。
    /// `with_animation` 只认 id，id 不变就只播一次（记忆里的老坑），
    /// 故用代际拼进元素 id，让每次唤出都重播那 140ms 的滑入。
    punct_bar_gen: usize,
    /// 上一帧的视口高度（v1.9.68）：`ensure_caret_visible` 要拿它算可视范围。
    /// 渲染期顺手记下来，比在键盘回调里再去问窗口便宜。
    viewport_h: f32,
    /// 上一帧的视口宽度（v1.9.68）：光标跟随要用它算「每行几字」。
    viewport_w: f32,
    /// **查找条**（v1.9.68，⌘F）：是否展开。
    find_open: bool,
    /// 查找词。
    find_query: String,
    /// 命中位置（**字节**偏移，扫一次就存下来；只在跳转时换算成字符下标）。
    find_hits: Vec<usize>,
    /// 当前命中序号（0 起）。
    find_idx: usize,
    /// 当前命中的**字符**区间（只给渲染高亮用，避免每行都做字节→字符换算）。
    find_sel: Option<Range<usize>>,
    /// 输入法正在组字的那一截（尚未上屏）：查找词 = `find_query + find_marked`。
    ///
    /// 拆成两段是必须的：中文查找要打拼音，而组字是**逐帧替换**的
    ///（`replace_and_mark_text_in_range` 每次给的是整段新的候选串，不是增量）。
    /// 只有把「已上屏」与「正在组」分开存，才能一边保留已打的字、一边跟着候选走。
    find_marked: String,
    /// **版式行索引**（v1.9.69）：百万字的虚拟列表就靠它。
    ///
    /// 只在「文本变了」或「版心列数变了」时重建（见 [`Self::ensure_line_index`]），
    /// 之后每帧都是二分查找。没有它，滚动一帧就要扫一遍全本 + 建几万个 Vec 条目。
    lines: LineIndex,
    /// 文本代际：每次正文被替换 +1。行索引的失效键，也是「有没有真的改过字」的判据。
    text_gen: u64,
    /// 上一次落盘时的文本代际（v1.9.69）：**内容没变就绝不重写 15MB**。
    saved_gen: u64,
    /// 上一次落盘时的光标（同上，配合 `saved_gen` 判断「这次真的需要写吗」）。
    saved_cursor: usize,
    /// **清洗审计卡片的缓存**（v1.9.70）：`None` = 还没算（或换了书）。
    /// 只随 `raw_text` 失效，**不随打字失效**——它比的是「导入毛坯 vs 清洗后」。
    sanitize_card: Option<SanitizeCard>,
}

/// **快捷标点栏**的一枚动作（v1.9.25）。
///
/// 做成枚举而不是 `String`，是为了让「插一个字符」与「包住选区」这两件事
/// **在类型上就分开**——否则每个按钮回调里都要自己判一次「有没有选区」，
/// 八处逻辑八种写法，迟早有一处忘了处理选区。
#[derive(Clone, Copy)]
enum PunctKey {
    /// 光标处插入（已有选区则替换）。
    Plain(&'static str),
    /// 用一对符号包裹选区；无选区时插一对并把光标放中间。
    Wrap(&'static str, &'static str),
    Backspace,
    Newline,
}

/// **包裹的纯计算**：给定正文 / 选区 / 光标，算出包裹后的新正文与新光标位。
///
/// 抽成自由函数只有一个理由——**可单测**。这段逻辑一旦写进
/// `wrap_selection(&mut self)`，就只能开窗口点鼠标才验得了；而它恰恰是最容易
/// 出错的地方（选区越界、CJK 字节边界、光标落在闭合符号外）。
///
/// 返回 `(新正文, 新光标)`。光标规则见 [`CollationWorkspace::wrap_selection`]。
fn wrap_text(
    text: &str,
    sel: Option<Range<usize>>,
    cursor: usize,
    open: &str,
    close: &str,
) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let cursor = cursor.min(total);
    let (a, b) = match sel {
        // 空选区（start == end）按「无选区」处理。
        Some(r) if r.start != r.end => (r.start.min(r.end).min(total), r.end.max(r.start).min(total)),
        _ => (cursor, cursor),
    };
    let inner: String = chars[a..b].iter().collect();
    let open_n = open.chars().count();
    let inner_n = inner.chars().count();
    let close_n = close.chars().count();
    let mut out = chars;
    out.splice(a..b, format!("{open}{inner}{close}").chars());
    let cursor = if a == b {
        a + open_n
    } else {
        a + open_n + inner_n + close_n
    };
    (out.into_iter().collect(), cursor)
}

/// **把一步编辑压进撤销栈**（v1.9.66）：同类别 + 时间窗内 + 几何相邻就合并。
///
/// 抽成自由函数（而不是写成 `CollationWorkspace` 的方法）只为可单测——
/// 合并逻辑是撤销功能最容易出 bug 的地方（连打十个字要退一步、不是十步），
/// 而它本身是纯数据操作，不该被「必须开窗口才能验」拖累。
fn push_edit(
    stack: &mut Vec<EditOp>,
    pos: usize,
    removed: String,
    inserted: String,
    kind: EditKind,
    now: std::time::Instant,
    allow_merge: bool,
) {
    // 空操作不记（例如光标已在文末还按 Delete）。
    if removed.is_empty() && inserted.is_empty() {
        return;
    }
    if allow_merge {
        if let Some(last) = stack.last_mut() {
            if last.kind == kind
                && now.duration_since(last.at) < Duration::from_millis(UNDO_COALESCE_MS)
                && last.try_absorb(pos, &removed, &inserted)
            {
                last.at = now;
                return;
            }
        }
    }
    stack.push(EditOp {
        pos,
        removed,
        inserted,
        kind,
        at: now,
    });
    if stack.len() > UNDO_MAX_STEPS {
        stack.remove(0);
    }
}

/// **字符级替换**（v1.9.66）：把 `[pos, pos+from_len)` 这段字符换成 `to`，返回新文本与光标。
///
/// 撤销与重做是**同一条数学**的两次调用——撤销是「把插入的换回删掉的」，
/// 重做是「把删掉的换回插入的」。抽成一个纯函数的好处：不用开窗口就能单测，
/// 而且保证两条路径永远不会各自漂移（这类「撤销对了、重做错一个字」的 bug，
/// 一旦分成两份实现就必然会出）。
fn splice_chars(text: &str, pos: usize, from_len: usize, to: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let a = pos.min(chars.len());
    let b = (a + from_len).min(chars.len());
    let mut next: Vec<char> = chars[..a].to_vec();
    next.extend(to.chars());
    next.extend(chars[b..].iter().copied());
    let cursor = (a + to.chars().count()).min(next.len());
    (next.into_iter().collect(), cursor)
}

/// **掩注**（⌘N）的纯计算：剥掉括注 / 夹注，只留正文。
///
/// 认两种写法，都是古稿里真在用的：
///  ① 全角 / 半角圆括号括注：`（注文）`、`(gloss)`——可嵌套，整段一起剥；
///  ② 方头注记：`[注:…]`、`[註:…]`（Markdown 稿里最常见的一种）。
///
/// 只删注文、**不动正文字序**；括号本身也不留（掩注就是「当它没写过」）。
/// 抽成自由函数是为了可单测——它是纯字符串活，不该只能开窗口按键才验得到。
fn strip_gloss(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        // ② `[注:…]` / `[註:…]`：整段连同方括号一起剥。
        if c == '[' {
            let is_note = matches!(chars.get(i + 1), Some('注') | Some('註'))
                && matches!(chars.get(i + 2), Some(':') | Some('：'));
            if is_note {
                let mut j = i + 3;
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                i = (j + 1).min(chars.len());
                continue;
            }
        }
        // ① 圆括号括注：支持嵌套，深度归零才继续抄正文。
        if c == '（' || c == '(' {
            let mut depth = 0i32;
            let mut j = i;
            while j < chars.len() {
                match chars[j] {
                    '（' | '(' => depth += 1,
                    '）' | ')' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            i = j.min(chars.len());
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

impl CollationWorkspace {
    pub fn new(cx: &mut Context<Self>, ai_panel: gpui::Entity<crate::ai::AiPanel>) -> Self {
        let mut me = Self {
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
            diff_scroll: ScrollHandle::new(),
            anchor: 0,
            dragging: false,
            drag_origin: None,
            show_preview: false,
            image_path: None,
            image_mem: None,
            image_gc: Vec::new(),
            last_temp: None,
            image_error: None,
            image_note: None,
            preview_paths: Vec::new(),
            temp_gc: Vec::new(),
            preview_sig: None,
            preview_paper: Paper::Card,
            imposition: Imposition::Single,
            binding: Binding::Right,
            export_busy: false,
            export_note: None,
            preview_page: 0,
            preview_error: None,
            preview_note: None,
            preview_busy: false,
            preview_pending: false,
            preview_gen: 0,
            preview_bg: ShareBg::Xuan,
            preview_frame: ShareFrame::Red,
            preview_columns: 12,
            show_style_panel: false,
            // 出厂显示高亮框：校对时它是「这一段落在原图哪一处」的唯一凭据。
            show_region_box: true,
            active_line: None,
            active_para_idx: None,
            src_nat_w: 0.0,
            src_nat_h: 0.0,
            left_px: 0.0,
            match_note: None,
            frame_count: 0,
            // 启动时**没有任何原图**，故区域表为空：绝不用假坐标占位。
            // 没有图就没有框、也不做任何匹配（`link_to_paragraph` 会先说
            // 「未加载原图」），这样「点了弹出重合度 4%」这类莫名其妙的
            // 内部指标再也不会出现。
            regions: Vec::new(),
            pdf_path: None,
            pdf_page: 1,
            pdf_pages: 1,
            page_busy: false,
            page_gen: 0,
            pdf_cache: std::collections::HashMap::new(),
            prefetch_busy: false,
            refocus: false,
            text_file: None,
            text_note: None,
            text_err: false,
            ocr_queue: jiandu_core::ocr::OcrQueue::new(),
            ocr_panel: false,
            ocr_note: None,
            pick_from: None,
            pick_rect: None,
            pick_text: None,
            pick_hits: 0,
            ai_panel,
            ai_open: false,
            hide_gloss: false,
            pure_text_mode: false,
            replace_confirm: false,
            // 清洗审计（v1.9.60）：导入时的「毛坯文本」与对照开关。
            raw_text: None,
            show_diff: false,
            show_sanitize_panel: false,
            // v1.9.64：持久化锚点未定（首启动是演示样张，没有 store_id）。
            store_id: None,
            save_gen: 0,
            // v1.9.66：撤销/重做栈从空开始（演示样张没有「上一版」可退）。
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            quit_hooked: false,
            undo_barrier: false,
            show_help: false,
            // v1.9.68：字宽度量留到首帧（那时字体一定已注册），先记 0 走近似值。
            char_adv: 0.0,
            punct_bar_shown: false,
            punct_bar_gen: 0,
            viewport_h: 800.0,
            viewport_w: 1200.0,
            find_open: false,
            find_query: String::new(),
            find_hits: Vec::new(),
            find_idx: 0,
            find_sel: None,
            find_marked: String::new(),
            lines: LineIndex::default(),
            // 从 1 起：0 是 `LineIndex::default()` 的哨兵值（表示「还没建过」）。
            text_gen: 1,
            saved_gen: 0,
            saved_cursor: usize::MAX,
            sanitize_card: None,
        };
        // ── v1.9.66：退出前强存（Exit Flush）────────────────────────────
        //
        // `touch` 的 1.5 秒防抖保证「平时别丢」，但 ⌘Q / 关窗 / 系统注销时那个
        // 定时器还没到点，最后这 1.5 秒的编辑就全没了。这里在 App 退出前把最新
        // 状态**同步**写一次盘，兜住最后一步——用户要测的「强杀进程恢复进度」
        // 靠的就是这条底线。
        //
        // ⚠️ 两个雷，都躺在 gpui 0.2.2 源码里：
        //  ① `Subscription` 的 `Drop` 会**取消**回调（subscription.rs:197-203），
        //     所以必须 `.detach()`——否则这钩子注册完下一行就失效，flush 永远
        //     不触发（与前轮 `cx.spawn` 必须 detach 是同一类雷）。
        //  ② 回调跑在 AppKit 的退出栈上，**绝不能**在这里更新别的 GPUI 实体
        //    （panic 穿过 ObjC 栈 = `panic_cannot_unwind` 直接 abort）。本回调
        //     只碰自己的字段 + 一条 sqlite REPLACE，安全。
        cx.on_app_quit(|this, _cx| {
            this.flush_now();
            async {}
        })
        .detach();
        me.quit_hooked = true;
        // 启动即清扫上一进程遗留的临时图（崩溃 / 强退时 `Drop` 跑不到，只能等下次
        // 启动补扫）。只认本应用自己的命名前缀，且只删「够旧」的，绝不误伤另开的
        // 一个笺读实例正在用的图。
        sweep_stale_temp_pngs();
        me
    }

    // ── 选区 / 光标 / 编辑 ───────────────────────────────────────────────

    /// **换正文的唯一入口**（v1.9.69）：写正文的同时把代际 +1，行索引随之失效。
    ///
    /// 为什么要一个入口而不是十处直接赋值：行索引一旦漏掉一次失效，屏幕上就会拿
    /// **上一版文本**的几何去画这一版的字——表现为「打一个字，光标位置跳一下」，
    /// 而且只在长文里出现，极难复查。收成一个函数，漏不掉。
    fn set_text(&mut self, t: String) {
        self.text = t;
        self.text_gen = self.text_gen.wrapping_add(1);
    }

    /// 全文总字数（读索引里的缓存，O(1)）。
    ///
    /// 直接写 `self.text.chars().count()` 是百万字里最贵的一行——一帧里被调五六次
    /// 就是几千万次字符解码，全花在「数一遍」上。
    fn total_chars(&self) -> usize {
        if self.index_valid() {
            self.lines.total_chars
        } else {
            self.text.chars().count()
        }
    }

    /// 行索引当前是否可用（文本代际 / 列数 / 模式三者全对）。
    fn index_valid(&self) -> bool {
        self.lines
            .valid(self.text_gen, self.chars_per_line(self.viewport_w), self.show_diff)
    }

    /// **按需重建行索引**（v1.9.69）。只在 `render` 里调一次，任何读几何的函数
    /// 都假定它已经就位。
    fn ensure_line_index(&mut self) {
        let cpl = self.chars_per_line(self.viewport_w);
        if self.lines.valid(self.text_gen, cpl, self.show_diff) {
            return;
        }
        let raw = if self.show_diff {
            self.raw_text.as_deref()
        } else {
            None
        };
        self.lines = LineIndex::build(&self.text, raw, cpl, self.text_gen, self.show_diff);
    }

    /// **可见行窗口**（v1.9.69）：虚拟列表的核心一步，返回闭区间 `[first, last]`。
    ///
    /// 口径与 `render_line` / `caret_at` 完全同源（`ED_PAD` + `LINE_H` + 滚动偏移，
    /// 向下滚为负），否则「画到屏幕上的行」和「光标该在的行」会错开，表现为
    /// 页面一片空白或行号错位。上下各留两屏余量：快滚时先画上，不至于闪白。
    fn visible_line_range(&self, row_h: f32) -> (usize, usize) {
        let n = self.lines.lines();
        if n == 0 {
            return (0, 0);
        }
        let scroll = -px_f32(if self.show_diff {
            self.diff_scroll.offset().y
        } else {
            self.scroll.offset().y
        });
        let vis_h = (row_h - self.head_band_h()).max(120.0);
        let overscan = vis_h * 2.0;
        let first_px = (scroll - ED_PAD - overscan).max(0.0);
        let last_px = (scroll + vis_h + overscan).max(0.0);
        let total = if self.show_diff {
            self.lines.total_aligned()
        } else {
            self.lines.total_visual()
        };
        if total == 0 {
            return (0, n - 1);
        }
        let last_v = ((last_px / LINE_H).ceil() as usize).min(total - 1);
        let first_v = ((first_px / LINE_H).floor() as usize).min(last_v);
        let (li0, li1) = if self.show_diff {
            (
                self.lines.line_at_aligned(first_v),
                self.lines.line_at_aligned(last_v),
            )
        } else {
            (
                self.lines.line_at_visual(first_v),
                self.lines.line_at_visual(last_v),
            )
        };
        (li0.min(n - 1), li1.min(n - 1).max(li0.min(n - 1)))
    }

    /// 第 `li` 行的字符区间（索引可用走 O(1)，否则退回单趟扫）。
    fn line_range(&self, li: usize) -> Option<(usize, usize)> {
        if self.index_valid() {
            return (li < self.lines.lines()).then(|| self.lines.char_range(li));
        }
        self.line_ranges().get(li).copied()
    }

    /// `(start, end)` 每行字符区间（不含换行）。
    ///
    /// ⚠️ 冷路径专用（导入 / 兜底）。热路径一律走 [`Self::line_range`] 或
    /// `self.lines`——这里每次调用都要 `split` + 建 Vec，几万行的稿子上就是几毫秒。
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
        if self.index_valid() {
            return self.lines.line_of(char_idx);
        }
        // 兜底：单趟扫，不建 Vec（旧实现这里会先 `line_ranges()` 再线性找）。
        let mut start = 0usize;
        for (i, line) in self.text.split('\n').enumerate() {
            let len = line.chars().count();
            if char_idx >= start && char_idx <= start + len {
                return Some(i);
            }
            start += len + 1;
        }
        None
    }

    /// 字符下标 → **字节偏移**（v1.9.69）：让「插入一个字」不必再把全本倒进
    /// `Vec<char>`（百万字 = 每次按键 20MB 分配 + 三次全量拷贝）。
    ///
    /// 走行索引：先落到行，再只扫这一行（≤ 版心列数个字），O(词长)。
    fn byte_of_char(&self, char_idx: usize) -> usize {
        if let Some(li) = self.line_of(char_idx) {
            if self.index_valid() {
                let (cs, _) = self.lines.char_range(li);
                let (bs, be) = self.lines.byte_range(&self.text, li);
                let off = char_idx.saturating_sub(cs);
                let line = &self.text[bs..be];
                return bs
                    + line
                        .char_indices()
                        .nth(off)
                        .map(|(b, _)| b)
                        .unwrap_or(line.len());
            }
        }
        // 兜底（索引未就绪 / 越界）：扫一遍。
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    fn char_up(&self) -> usize {
        let idx = match self.line_of(self.cursor) {
            Some(i) => i,
            None => return self.cursor,
        };
        if idx == 0 {
            return self.cursor;
        }
        let Some((cs, _)) = self.line_range(idx) else {
            return self.cursor;
        };
        let col = self.cursor - cs;
        match self.line_range(idx - 1) {
            Some((ps, pe)) => (ps + col.min(pe - ps)).min(pe),
            None => self.cursor,
        }
    }

    fn char_down(&self) -> usize {
        let idx = match self.line_of(self.cursor) {
            Some(i) => i,
            None => return self.cursor,
        };
        let Some((cs, _)) = self.line_range(idx) else {
            return self.cursor;
        };
        let col = self.cursor - cs;
        match self.line_range(idx + 1) {
            Some((ns, ne)) => (ns + col.min(ne - ns)).min(ne),
            None => self.cursor,
        }
    }

    fn line_home(&mut self, shift: bool, cx: &mut Context<Self>) {
        if let Some(idx) = self.line_of(self.cursor) {
            if let Some((s, _)) = self.line_range(idx) {
                self.move_cursor_to(s, shift, cx);
            }
        }
    }

    fn line_end(&mut self, shift: bool, cx: &mut Context<Self>) {
        if let Some(idx) = self.line_of(self.cursor) {
            if let Some((_, e)) = self.line_range(idx) {
                self.move_cursor_to(e, shift, cx);
            }
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
        // 光标挪过 → 后续键入另起一步（不许与挪动前那次打字合并）。
        self.undo_barrier = true;
        // 上下键常把光标顶出视口：方向键也要跟随滚动（v1.9.68）。
        self.ensure_caret_visible(cx);
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

    /// **记一步可撤销的编辑**（v1.9.66）。
    ///
    /// 三条约定：
    ///   ① **先清空重做栈**——任何新编辑都让「原来那条重做线」作废（分叉即弃），
    ///      这是所有编辑器的通行语义，不清的话 ⌘Z ⌘Z ⌘⇧Z 会拼出乱序；
    ///   ② 同类别 + 700ms 内 + 几何相邻 → 合并进上一步（连打字算一步、连退格算一步）；
    ///   ③ 栈满 500 步从头丢，内存恒定（与正文长度无关）。
    fn record_edit(&mut self, pos: usize, removed: String, inserted: String, kind: EditKind) {
        // ① **先清空重做栈**——任何新编辑都让「原来那条重做线」作废（分叉即弃），
        //    这是所有编辑器的通行语义；不清的话 ⌘Z ⌘Z ⌘⇧Z 会拼出乱序。
        self.redo_stack.clear();
        // 光标刚被挪过 → 这一步必须自成一档（见 `undo_barrier`）。
        let allow_merge = !self.undo_barrier;
        self.undo_barrier = false;
        push_edit(
            &mut self.undo_stack,
            pos,
            removed,
            inserted,
            kind,
            std::time::Instant::now(),
            allow_merge,
        );
    }

    /// **⌘Z 撤销**：把栈顶那一步反向应用（删掉它插入的、插回它删掉的）。
    ///
    /// 返回是否真的撤销了一步——没历史时返回 `false`，调用方可据此给提示
    /// （而不是让用户按了没反应还以为键盘坏了）。
    fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(op) = self.undo_stack.pop() else {
            return false;
        };
        let (text, cursor) =
            splice_chars(&self.text, op.pos, op.inserted.chars().count(), &op.removed);
        self.set_text(text);
        // 光标落在这段恢复文字之后（按下 ⌘Z 后接着打字的位置就是那里）。
        self.cursor = cursor;
        self.selection = None;
        self.marked = None;
        self.redo_stack.push(op);
        // 撤销后接着打字：必须是**新的一步**，绝不能并进刚撤掉的那步去。
        self.undo_barrier = true;
        cx.notify();
        self.touch(cx);
        true
    }

    /// **⌘⇧Z 重做**：把撤销掉的那一步再做一遍。
    fn redo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(op) = self.redo_stack.pop() else {
            return false;
        };
        let (text, cursor) =
            splice_chars(&self.text, op.pos, op.removed.chars().count(), &op.inserted);
        self.set_text(text);
        self.cursor = cursor;
        self.selection = None;
        self.marked = None;
        // 直接压回撤销栈（**不能**走 `record_edit`——那会清空重做栈，重做一次就断链）。
        self.undo_stack.push(op);
        self.undo_barrier = true;
        cx.notify();
        self.touch(cx);
        true
    }

    fn delete_char(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        // ⚠️ 全部走**字节区间**（v1.9.69）：旧实现每次按键都 `self.text.chars()
        //    .collect::<Vec<char>>()`，百万字就是每按一次分配 20MB 再拷贝三遍——
        //    打一句话界面就卡成幻灯片。`replace_range` 只做一次 memmove。
        if let Some(sel) = self.selection.as_ref() {
            let (s, e) = (sel.start.min(sel.end), sel.start.max(sel.end));
            let total = self.total_chars();
            let s = s.min(total);
            let e = e.min(total).max(s);
            let (bs, be) = (self.byte_of_char(s), self.byte_of_char(e));
            let removed: String = self.text[bs..be].to_string();
            self.text.replace_range(bs..be, "");
            self.text_gen = self.text_gen.wrapping_add(1);
            self.cursor = s;
            self.selection = None;
            // 删选区是「一次性动作」：不参与合并，按一次 ⌘Z 整段回来。
            self.record_edit(s, removed, String::new(), EditKind::Once);
            cx.notify();
            self.touch(cx);
            return;
        }
        let total = self.total_chars();
        if forward {
            if self.cursor < total {
                let (b0, b1) = (
                    self.byte_of_char(self.cursor),
                    self.byte_of_char(self.cursor + 1),
                );
                let removed: String = self.text[b0..b1].to_string();
                self.text.replace_range(b0..b1, "");
                self.text_gen = self.text_gen.wrapping_add(1);
                self.record_edit(self.cursor, removed, String::new(), EditKind::Delete);
                cx.notify();
                self.touch(cx);
            }
        } else if self.cursor > 0 {
            let (b0, b1) = (
                self.byte_of_char(self.cursor - 1),
                self.byte_of_char(self.cursor),
            );
            let removed: String = self.text[b0..b1].to_string();
            self.text.replace_range(b0..b1, "");
            self.text_gen = self.text_gen.wrapping_add(1);
            self.cursor -= 1;
            self.record_edit(self.cursor, removed, String::new(), EditKind::Backspace);
            cx.notify();
            self.touch(cx);
        }
    }

    /// 在光标处（或替换选区）插入文本。
    fn insert_text(&mut self, s: &str, cx: &mut Context<Self>) {
        let (a, b) = match self.selection.as_ref() {
            Some(sel) => (sel.start.min(sel.end), sel.start.max(sel.end)),
            None => (self.cursor, self.cursor),
        };
        let total = self.total_chars();
        let a = a.min(total);
        let b = b.min(total).max(a);
        // 字节区间 + `replace_range`（v1.9.69）：见 `delete_char` 的注释。
        let (bs, be) = (self.byte_of_char(a), self.byte_of_char(b));
        let replaced: String = self.text[bs..be].to_string();
        let s_n = s.chars().count();
        self.text.replace_range(bs..be, s);
        self.text_gen = self.text_gen.wrapping_add(1);
        self.cursor = a + s_n;
        self.selection = None;
        // 逐字键入（含回车）走 `Insert`，可与前后连打合并成一步；
        // 整段粘贴 / AI 补全一次进一大块，走 `Once` 独立成步——否则「粘一页、
        // 再打两个字」会被并成一步，⌘Z 一下把整页都吃了。
        let kind = if s.chars().count() == 1 {
            EditKind::Insert
        } else {
            EditKind::Once
        };
        self.record_edit(a, replaced, s.to_string(), kind);
        cx.notify();
        self.touch(cx);
    }

    /// **用一对符号包裹选区**（快捷标点栏的「夹注括号」「方括号」两枚）。
    ///
    /// 两条分支，光标落点**故意不同**：
    ///   ① 有选区 → 包裹后光标停在**闭合符号之后**（一口气把这段注完，接着往下写）；
    ///   ② 无选区 → 插入一对空符号，光标落在**两个符号中间**（下一步就是写注文，
    ///      不是跳过它）。「（）」插完把光标丢到括号外面，等于每次都要再按一次
    ///      ←，校对时那一秒最恼人。
    fn wrap_selection(&mut self, open: &str, close: &str, cx: &mut Context<Self>) {
        // 先把这次替换的「旧串 / 新串」算出来（撤销栈只认这一对），再交给
        // `wrap_text` 做真活——两边口径必须同源，否则撤销会错位一个字。
        let total = self.total_chars();
        let (a, b) = match self.selection.clone() {
            Some(r) if r.start != r.end => (
                r.start.min(r.end).min(total),
                r.start.max(r.end).min(total),
            ),
            _ => {
                let c = self.cursor.min(total);
                (c, c)
            }
        };
        let removed: String = self.text[self.byte_of_char(a)..self.byte_of_char(b)].to_string();
        let inserted = format!("{open}{removed}{close}");
        let (text, cursor) = wrap_text(&self.text, self.selection.clone(), self.cursor, open, close);
        self.set_text(text);
        self.cursor = cursor;
        self.selection = None;
        self.record_edit(a, removed, inserted, EditKind::Once);
        cx.notify();
        self.touch(cx);
    }

    /// **⌘N：掩注**（v1.9.54）——把括注 / 夹注小字从**预览与导出**里隐去。
    ///
    /// 「无痕重排」的三条口径：
    ///   ① `text` 一个字都不改（掩注是**派生视图**，随时切回来还是原稿）；
    ///   ② 版式指纹显式作废——不然内容变了还被短路成「同一版式」，看着像没反应；
    ///   ③ `preview_page` 原样保留：用户在第三页按的 ⌘N，重排完仍停在第三页。
    fn toggle_gloss(&mut self, cx: &mut Context<Self>) {
        self.hide_gloss = !self.hide_gloss;
        self.preview_sig = None;
        if self.show_preview {
            self.request_preview(cx);
        }
        cx.notify();
    }

    /// 切换【原文对照】：左栏在「原图 / 毛坯文本」之间切换。无关图时左栏本就空，
    /// 开对照即把毛坯铺上去与右栏清洗后正文并排。无毛坯直接返回（按钮已禁用）。
    fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        if self.raw_text.is_none() {
            return;
        }
        self.show_diff = !self.show_diff;
        // v1.9.63：进对照即把共用 `diff_scroll` 归零，左右从顶部对齐起步。
        if self.show_diff {
            self.diff_scroll.set_offset(Point::new(px(0.0), px(0.0)));
        }
        cx.notify();
        self.touch(cx);
    }

    /// 切换【清洗日志】内嵌面板的显隐（极简浮卡，不离开 App）。
    fn toggle_sanitize_panel(&mut self, cx: &mut Context<Self>) {
        if self.raw_text.is_none() {
            return;
        }
        self.show_sanitize_panel = !self.show_sanitize_panel;
        // 打开时**一次性**算好（`render` 里也会兜一次），之后每帧只读缓存。
        if self.show_sanitize_panel && self.sanitize_card.is_none() {
            self.sanitize_card = Some(self.compute_sanitize_card());
        }
        cx.notify();
    }

    /// 算一次清洗审计（读毛坯、跑一遍清洗、按**正文**数字数）。只在开面板时调用。
    fn compute_sanitize_card(&self) -> SanitizeCard {
        let Some(raw) = self.raw_text.as_deref() else {
            return SanitizeCard::default();
        };
        let stats = jiandu_core::cleaner::sanitize_stats(raw);
        let cleaned = crate::importer::cleaner::clean(raw);
        SanitizeCard {
            // ⚠️ 全部走 `body_char_count`：剔除 `[注]` / `[/注]`、空白、零宽字符。
            // 用 `chars().count()` 就是把标签算成字，几千处夹注足以把「净减」
            // 变成「净增三万」——这正是用户截图里那条吓人的假账。
            raw_body: jiandu_core::cleaner::body_char_count(raw),
            clean_body: jiandu_core::cleaner::body_char_count(&cleaned),
            ghost: stats.ghost,
            variant: stats.variant,
            notes: stats.notes,
            tag_chars: cleaned.matches("[注]").count() * 3
                + cleaned.matches("[/注]").count() * 4,
        }
    }

    /// 生成 `sanitize_report.md` 并在访达中打开（macOS `open`）。
    ///
    /// 报告以 Raw 毛坯为基准，如实罗列清洗引擎会做的三类改动：
    /// 清除幽灵字符 X 个、折叠异体/康熙部首 Y 处、转换异文/括号夹注 Z 处——
    /// 让用户一眼确认「没有把原书真字吞掉」。落盘到与本书同源的临时目录，绝不
    /// 污染用户书库。供【清洗日志】面板的「导出明细」按钮调用。
    fn open_sanitize_log(&mut self, cx: &mut Context<Self>) {
        let Some(raw) = self.raw_text.clone() else {
            return;
        };
        let stats = jiandu_core::cleaner::sanitize_stats(&raw);
        let cleaned = crate::importer::cleaner::clean(&raw);
        // **正文口径**（v1.9.70）：与面板同一个函数，导出与屏幕绝不能两套账。
        let cleaned_len = jiandu_core::cleaner::body_char_count(&cleaned);
        let raw_len = jiandu_core::cleaner::body_char_count(&raw);
        let tag_chars =
            cleaned.matches("[注]").count() * 3 + cleaned.matches("[/注]").count() * 4;
        let now = ts_now();
        let report = format!(
            "# 清洗审计日志 · sanitize_report\n\n\
             生成时间：{now}\n\n\
             ## 一、统计（以导入毛坯为基准）\n\n\
             | 项目 | 数量 | 说明 |\n\
             | --- | --- | --- |\n\
             | 清除幽灵字符 | {ghost} | 零宽 / BOM / 软连字符 / 控制符（竖排里全是空白格，删除零风险） |\n\
             | 折叠异体 / 康熙部首 | {variant} | 只改 VARIANT_MAP 与兼容区里有通式的字；生僻字一律保留 |\n\
             | 转换异文 / 括号夹注 | {notes} | `〔…〕` `（…）` 升格为 `[注]…[/注]` |\n\n\
             ## 二、体量变化（只计正文）\n\n\
             - 毛坯正文字数：**{raw_len}**\n\
             - 清洗后正文字数：**{cleaned_len}**（{delta_label}）\n\
             - 不计入字数的夹注标签字符：{tag_chars}（`[注]` / `[/注]` 是结构标记，不是书里的字）\n\
             - 说明：空白、换行、零宽 / BOM 等不可见字符一律不计。\n\n\
             ## 三、防误杀声明\n\n\
             清洗层对**可见可读的正文字符一律不动**：幽灵字符删除清单只含零宽 / 控制符，\
             异体字归一只动有通式的字、生僻字保留，段落级噪声删除（广告 / 页码 / 版权页）\
             全部带关键词 + 长度双阈值，绝不在不确定的情况下直接吞字。\n",
            ghost = stats.ghost,
            variant = stats.variant,
            notes = stats.notes,
            raw_len = raw_len,
            cleaned_len = cleaned_len,
            tag_chars = tag_chars,
            delta_label = if cleaned_len as i64 >= raw_len as i64 {
                format!("净增 {}", cleaned_len as i64 - raw_len as i64)
            } else {
                format!("净减 {}", raw_len as i64 - cleaned_len as i64)
            },
        );
        // 落盘到临时目录（与书库隔离），再交给访达打开。
        let dir = std::env::temp_dir().join("jiandu-sanitize");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sanitize_report.md");
        if let Err(e) = std::fs::write(&path, report) {
            eprintln!("[jiandu] 清洗日志写入失败：{e}");
            cx.notify();
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&path).status();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = std::process::Command::new("xdg-open").arg(&path).status();
        }
        self.image_note = Some(format!("清洗日志已生成：{}", path.display()));
        cx.notify();
    }

    // ── 问小笺（v1.9.54）───────────────────────────────────────────────

    /// 当前选区的原文（无选区 = 空串）。
    fn selected_text(&self) -> String {
        let Some(r) = self.selection.as_ref() else {
            return String::new();
        };
        let chars: Vec<char> = self.text.chars().collect();
        let a = r.start.min(r.end).min(chars.len());
        let b = r.end.max(r.start).min(chars.len());
        chars[a..b].iter().collect()
    }

    /// **框一句原文来问**：把选区压成 [`Ask`]，交给与阅读器共用的问小笺面板。
    ///
    /// 与阅读器同一条口径：引文只进不出（随这一轮请求发出，不落盘、不进存档），
    /// 前后文各带 80 字，让先生知道这句在说什么。面板没开就顺手开——用户的意图
    /// 是「问这一句」，不是「开个面板看看」。
    fn ask_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let quote = self.selected_text();
        if quote.trim().is_empty() {
            return;
        }
        let chars: Vec<char> = self.text.chars().collect();
        let (a, b) = match self.selection.as_ref() {
            Some(r) => (
                r.start.min(r.end).min(chars.len()),
                r.end.max(r.start).min(chars.len()),
            ),
            None => (self.cursor.min(chars.len()), self.cursor.min(chars.len())),
        };
        let before: String = chars[a.saturating_sub(80)..a].iter().collect();
        let after: String = chars[b..(b + 80).min(chars.len())].iter().collect();
        let ask = jiandu_core::ai::Ask {
            book: self
                .text_file
                .as_ref()
                .map(|p| file_display_name(p))
                .unwrap_or_else(|| "笺校稿".to_string()),
            chapter: String::new(),
            quote,
            before,
            after,
            question: String::new(),
            refs: Vec::new(),
        };
        self.ai_open = true;
        // 面板与阅读器共用，纸墨可能是阅读那边同步过的（深色 / 别的底色）——
        // 工作台永远是浅宣纸，故开卡时按出厂纸墨校一次，免得深底浮在浅纸上。
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        self.ai_panel.update(cx, |p, cx| {
            p.sync_look(jiandu_core::theme::Theme::default(), song);
            p.take_context(ask, cx);
            p.ask_quote(cx);
        });
        // 焦点递给提问框：否则输入法没有落点，追问打不进中文。
        let f = self.ai_panel.read(cx).ask_focus();
        window.focus(&f);
        cx.notify();
    }

    /// 收起问小笺浮卡（焦点交还编辑器）。
    fn close_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ai_open = false;
        window.focus(&self.focus);
        cx.notify();
    }

    /// 问小笺浮卡：右下角宣纸卡，内嵌**共用**的 `AiPanel` 实体。
    ///
    /// 与阅读器同款视觉（圆角纸卡 + 淡朱边 + 柔影），但不做滑入动画——
    /// 工作台是「一屏做一件事」的工具，浮卡随点随到即可。
    fn render_ai_card(&self, vw: f32, vh: f32, cx: &mut Context<Self>) -> gpui::Div {
        let w = 560.0_f32.min((vw - 56.0).max(320.0));
        let h = (vh * 0.66).clamp(320.0, 560.0);
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        div()
            .absolute()
            .right(px(18.0))
            .bottom(px(18.0))
            .w(px(w))
            .h(px(h))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(0x2B26202E))
            .shadow(vec![BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.18),
                offset: gpui::point(px(0.0), px(4.0)),
                blur_radius: px(14.0),
                spread_radius: px(0.0),
            }])
            // 阻断穿透：否则点面板会把「抬起」漏给正文（同阅读器浮层的老坑）。
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_c, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(34.0))
                    .px(px(12.0))
                    .bg(rgb(CHROME))
                    .border_b_1()
                    .border_color(rgba(0x2B26200D))
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(12.0))
                            .text_color(rgb(0x2B2620))
                            .child(SharedString::from("问小笺")),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgba(0x2B26208C))
                            .hover(|s| s.text_color(rgb(0x8c2b22)))
                            .child(SharedString::from("收起"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|c, _, window, cx| {
                                    cx.stop_propagation();
                                    c.close_ai(window, cx);
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.ai_panel.clone()),
            )
    }

    /// 编辑器选区旁的**悬浮钮**（v1.9.54）：框住一段字才出现，点「问小笺」即发问。
    ///
    /// 坐标口径与 `render_line` 完全一致（同一个 `CHAR_W` / `LINE_H` / `ED_PAD`，
    /// 再减编辑器滚动偏移），否则气泡会飘在半空——这是「点哪亮哪」那条链的
    /// 同一个坑，故这里只算一次、不再另立一套。
    /// **查找条**（v1.9.68，⌘F）：右上角一条窄卡，不占布局、不抢焦点。
    ///
    /// 为什么不抢焦点：如果给它单独一个 `FocusHandle`，输入法就要重建一次
    /// 输入上下文（`rfd` 之后那个老坑），而且「在查找条里打字」与「在正文里
    /// 打字」会走两条不同的 IME 路径。这里让**编辑器保持聚焦**，按键与上屏
    /// 事件在 `handle_key` / `replace_text_in_range` 里分流——一条路径，
    /// 中英文都通。
    fn render_find_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        let q = self.find_text();
        let total = self.find_hits.len();
        let count = if total == 0 {
            if q.is_empty() { String::new() } else { "无匹配".to_string() }
        } else {
            format!("{} / {}", self.find_idx + 1, total)
        };
        let btn = |label: &'static str, cx: &mut Context<Self>, act: fn(&mut Self, &mut Context<Self>)| {
            div()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(rgba(0x2B2620AA))
                .hover(|s| s.bg(rgba(0x2B26200F)).text_color(rgb(0x8c2b22)))
                .child(SharedString::from(label))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |c, _, _, cx| {
                        cx.stop_propagation();
                        act(c, cx);
                    }),
                )
        };
        div()
            .absolute()
            .top(px(HEADER_H + 8.0))
            .right(px(22.0))
            .h(px(36.0))
            .w(px(348.0))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px(px(11.0))
            .rounded(px(8.0))
            .bg(rgb(CHROME))
            .border_1()
            .border_color(rgba(0x2B26202E))
            .shadow(vec![BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.16),
                offset: gpui::point(px(0.0), px(4.0)),
                blur_radius: px(14.0),
                spread_radius: px(-2.0),
            }])
            // 阻断穿透：点查找条不该把光标挪到下面的正文里。
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_c, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .font_family(song.clone())
                    .text_size(px(11.5))
                    .text_color(rgba(0x2B262080))
                    .child(SharedString::from("查找")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .font_family(song.clone())
                    .text_size(px(13.0))
                    .text_color(if q.is_empty() {
                        rgba(0x2B262066)
                    } else {
                        rgb(0x2B2622)
                    })
                    .child(SharedString::from(if q.is_empty() {
                        // 组字中的那一截用「…」标出来，用户知道字还没上屏。
                        "点这里打字 · ⌘G 下一个".to_string()
                    } else if self.find_marked.is_empty() {
                        q.clone()
                    } else {
                        format!("{}{}", self.find_query, "…")
                    })),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgba(0x2B262080))
                    .child(SharedString::from(count)),
            )
            .child(btn("‹", cx, |c, cx| c.find_step(-1, cx)))
            .child(btn("›", cx, |c, cx| c.find_step(1, cx)))
            .child(btn("✕", cx, |c, cx| c.close_find(cx)))
    }

    /// **清洗日志内嵌面板**（v1.9.60）：极简浮卡，如实展示本次清洗的三类统计——
    /// 折叠异体/康熙部首 X 处、清除幽灵字符 Y 个、转换异文/括号夹注 Z 处，让
    /// 用户一眼确认「没有把原书真字吞掉」。不离开 App，点「导出明细」才落盘报告。
    fn render_sanitize_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        // **只读缓存，绝不在渲染里清洗全文**（v1.9.70）：旧实现每帧都跑一遍
        // `clean(&raw)`——三百万字的稿子，面板开着的时候等于每帧全量清洗一次，
        // 界面直接躺平。缓存在 `render()` 开头算一次（见 `sanitize_card`）。
        let card = self.sanitize_card.unwrap_or_default();
        // 单项条目：朱砂数字 + 说明，左对齐。
        let item = |label: &'static str, n: usize, note: &'static str| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .mb(px(12.0))
                .child(
                    div()
                        .w(px(46.0))
                        .text_size(px(22.0))
                        .font_family(song.clone())
                        .text_color(rgb(0x8c2b22))
                        .child(SharedString::from(format!("{n}"))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(0x2b2620))
                                .child(SharedString::from(label)),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(0x9A9184))
                                .child(SharedString::from(note)),
                        ),
                )
        };
        // 抽屉宽：右栏够宽就 360，窄窗口收到右栏以内——**永不越过左栏**。
        let panel_w = (self.viewport_w - self.left_col_w(self.viewport_w) - 24.0).clamp(240.0, 360.0);
        div()
            .absolute()
            // 必须 `.id()` 使 `Div` 变 `Stateful<Div>` —— `overflow_y_scroll` 等方法
            // 只在 `StatefulInteractiveElement`（仅对 `Stateful<E>` 实现）上可用。
            .id(SharedString::from("collation-sanitize-panel"))
            // **右侧抽屉**（v1.9.70，用户口径）：从右缘贴边滑出，上沿让开顶栏，
            // 下沿留 8px。左栏（原文 / 原图）**一个像素都不压**。
            .top(px(HEADER_H + 8.0))
            .bottom(px(8.0))
            .right(px(8.0))
            .w(px(panel_w))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p(px(18.0))
            // 毛玻璃感：GPUI 0.2.2 没有 backdrop-filter，用「半透明宣纸 + 大范围柔影」
            // 近似——底下的字会隐隐透出来，但读不清，正是毛玻璃的观感。绝不用死白。
            .bg(rgba(0xF3ECD9EB))
            .border_1()
            .border_color(rgba(0x9A6B4F66))
            .rounded(px(12.0))
            .font_family(song.clone())
            .shadow(vec![gpui::BoxShadow {
                color: gpui::hsla(0.09, 0.30, 0.20, 0.20),
                offset: gpui::point(px(-6.0), px(0.0)),
                blur_radius: px(26.0),
                spread_radius: px(-2.0),
            }])
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .mb(px(2.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(15.0))
                            .font_family(song.clone())
                            .text_color(rgb(0x2b2620))
                            .child(SharedString::from("清洗审计日志")),
                    )
                    .child(
                        div()
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(11.0))
                            .cursor_pointer()
                            .text_size(px(13.0))
                            .text_color(rgb(0x9A9184))
                            .hover(|s| s.bg(rgba(0x2B26200F)).text_color(rgb(0x8c2b22)))
                            .child(SharedString::from("✕"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|c, _, _, cx| {
                                    cx.stop_propagation();
                                    c.show_sanitize_panel = false;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0x9A9184))
                    .mb(px(16.0))
                    .child(SharedString::from(
                        "以导入毛坯为基准，如实汇报清洗引擎动了哪些字。",
                    )),
            )
            .child(item(
                "折叠异体 / 康熙部首",
                card.variant,
                "只改有通式的字，生僻字一律保留",
            ))
            .child(item(
                "清除幽灵字符",
                card.ghost,
                "零宽 / BOM / 软连 / 控制符，删除零风险",
            ))
            .child(item(
                "转换异文 / 括号夹注",
                card.notes,
                "〔…〕（…）升格为 [注]…[/注]",
            ))
            .child(
                div()
                    .mt(px(4.0))
                    .pt(px(12.0))
                    .border_t_1()
                    .border_color(rgba(0x2B26201A))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    // **字数只算正文**（v1.9.70）：`[注]` / `[/注]` 是给渲染器的结构
                    // 标记，不是书里的字。按整串 `chars().count()` 比，几千处夹注就会
                    // 让「删了 3 万字」显示成「净增 3 万」——这是上一版最吓人的假账。
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(0x4a4335))
                            .child(SharedString::from(format!(
                                "正文 {} 字 → {} 字（{}）",
                                card.raw_body,
                                card.clean_body,
                                card.delta_label()
                            ))),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(0x9A9184))
                            .child(SharedString::from(format!(
                                "未计入 {} 个夹注标签字符（[注] {} 处）与空白",
                                card.tag_chars, card.notes
                            ))),
                    )
                    .child(
                        div()
                            .mt(px(6.0))
                            .flex()
                            .flex_row()
                            .justify_end()
                            .child(self.button("导出明细", |c, _, _, cx| {
                                c.open_sanitize_log(cx)
                            }, cx)),
                    ),
            )
    }

    /// **选区旁的悬浮钮**（v1.9.54 立项，v1.9.69 重定位）。
    ///
    /// ## 回归事故记录（必读，别再把坐标写回「相对编辑器」）
    /// v1.9.68 给编辑器面板加 `.overflow_hidden()` 之后，这枚气泡**整颗消失**。
    /// 根因是坐标写法本身错了两处，只是以前面板不裁切、错位也还看得见：
    ///   ① `- scroll_y` 是**双重否定**——`scroll.offset().y` 向下滚已经是负数，
    ///      再取反等于「滚 300px、气泡往下跑 600px」。一滚就飘出屏幕，裁切一来即消失。
    ///   ② 高度只数了 `est_wrapped_lines`，**没算对照模式每段的等高填充**；
    ///      改过几段之后，气泡就浮到选区上面几十行去。
    /// 现在统一走 `editor_origin()`（全应用唯一的内容原点）+ 行索引前缀和：
    /// 气泡位置 = 原点 + (对齐后的视觉行 + 1) × 行高，滚动偏移由原点自带，
    /// 不再手工加减。最后再夹进右栏矩形——**任何情况下都不会算到屏幕外**。
    ///
    /// 它现在挂在**根节点**（[`Self::render`]），是全屏最后一层：编辑器的
    /// `overflow_hidden`、标点栏、识别队列都盖不住它。模态浮层（预览 / 问小笺卡 /
    /// 覆盖确认）仍压在它上面，那是刻意的。
    fn render_selection_bar(&self, vw: f32, vh: f32, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let sel = self.selection.as_ref()?;
        if sel.start == sel.end {
            return None;
        }
        // 预览 / 覆盖确认开着时不浮（那两层是模态，气泡没有意义还挡字）。
        if self.show_preview || self.replace_confirm {
            return None;
        }
        let end = sel.end.max(sel.start).min(self.total_chars());
        let li = self.line_of(end)?;
        let (line_start, _) = self.line_range(li)?;
        let col = end.saturating_sub(line_start);
        let cpl = self.chars_per_line(vw).max(1);
        // 视觉行 = 该行之前累计的视觉行数 + 行内第几折行。对照模式用**对齐后**
        // 的前缀和（每段取两栏行数的较大者），否则填充会把气泡顶到选区上方。
        let visual_before = if self.show_diff {
            self.lines.aligned.get(li).copied().unwrap_or(0)
        } else {
            self.lines.vis.get(li).copied().unwrap_or(0)
        };
        let visual_idx = visual_before + col / cpl;
        let col_in_visual = col % cpl;
        // 唯一原点（含 left_col_w / 顶栏 / 状态条 / 滚动偏移）。
        let (ox, oy) = self.editor_origin(vw);
        const BAR_W: f32 = 66.0;
        const BAR_H: f32 = 24.0;
        let panel_l = self.left_col_w(vw);
        let panel_t = HEADER_H + self.head_band_h();
        // 贴选区末行的**下一行**；越界一律夹进右栏矩形（见 `clamp_bar_pos`）。
        let (bx, by) = bar_anchor((ox, oy), visual_idx, col_in_visual, self.char_adv());
        let (x, y) = clamp_bar_pos(bx, by, vw, vh, panel_l, panel_t, BAR_W, BAR_H);
        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                // `occlude`：这一小块必须挡住下面的编辑器，否则点「问小笺」
                // 会先落一次光标、把选区清掉，随后的 ask_ai 就拿到空选区。
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_c, _, _, cx| cx.stop_propagation()),
                )
                .child(
                    div()
                        .px(px(11.0))
                        .py(px(5.0))
                        .rounded(px(14.0))
                        .bg(rgb(CHROME))
                        .border_1()
                        .border_color(rgba(0x8C2B2255))
                        .shadow(vec![BoxShadow {
                            color: gpui::hsla(0.0, 0.0, 0.0, 0.10),
                            offset: gpui::point(px(0.0), px(2.0)),
                            blur_radius: px(6.0),
                            spread_radius: px(0.0),
                        }])
                        .cursor_pointer()
                        .text_size(px(11.5))
                        .text_color(rgb(0x8C2B22))
                        .hover(|s| s.bg(rgba(0xF3EBD4FF)))
                        .child(SharedString::from("问小笺"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|c, _, window, cx| {
                                cx.stop_propagation();
                                c.ask_ai(window, cx);
                            }),
                        ),
                ),
        )
    }

    /// 快捷标点栏落子：三类动作统一走这里，**只有一处**决定「插还是包」。
    fn punct_apply(&mut self, key: PunctKey, cx: &mut Context<Self>) {
        // 组字中（拼音还没上屏）不许插：此时选区与光标都还是旧的，插进去会叠字。
        if self.marked.is_some() {
            return;
        }
        match key {
            PunctKey::Plain(s) => self.insert_text(s, cx),
            PunctKey::Wrap(open, close) => self.wrap_selection(open, close, cx),
            PunctKey::Backspace => self.delete_char(false, cx),
            PunctKey::Newline => self.insert_text("\n", cx),
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            // 组字中：交给输入法，回填由 `EntityInputHandler` 负责。
            return;
        }
        let k = event.keystroke.key.as_str();
        let shift = event.keystroke.modifiers.shift;
        let cmd = event.keystroke.modifiers.platform || event.keystroke.modifiers.control;
        // 查找条开着：键先归它（Esc 关、⏎ 下一个、⌫ 删字、再按 ⌘F 收起）。
        if self.find_open && self.find_key(k, shift, cmd, cx) {
            return;
        }
        // ── 平台键（⌘ / Ctrl）───────────────────────────────────────────
        // GPUI 0.2.2 不会替自绘编辑器接管剪贴板：⌘V 必须自己读系统剪贴板再落字
        // （与阅读器草稿框同一条路，见 `reader` 的 note 输入）。否则用户在
        // 工作台里按 ⌘V 是**一点反应都没有**——这就是「粘贴不通」的真因。
        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
            match k {
                // ⌘Z / ⌘⇧Z：**撤销 / 重做**（v1.9.66）。几百万字的稿子没有撤销，
                // 用户根本不敢改一个字——这是「可发售」与「玩具」的分界线。
                // macOS 的 keystroke.key 在 ⇧Z 时仍可能是 "z"（大写由 modifiers
                // 表达），也可能给 "Z"，两种都收，判定只认 `shift` 标志。
                "z" | "Z" => {
                    if shift {
                        self.redo(cx);
                    } else {
                        self.undo(cx);
                    }
                    return;
                }
                "v" | "V" => {
                    // 正文要保留换行（稿子是多段的），故不走 `paste_text`
                    //（那个口子把 \n 压成空格，是给单行草稿用的）。
                    if let Some(s) = crate::clipboard::read_text(cx) {
                        if !s.is_empty() {
                            self.insert_text(&s, cx);
                        }
                    }
                    return;
                }
                // ⌘C / ⌘X / ⌘A：自绘编辑器必须自己接剪贴板与全选——GPUI 不会替
                // 任何非内置输入框做这件事（v1.9.68）。
                "c" | "C" => {
                    self.copy_selection(cx);
                    return;
                }
                "x" | "X" => {
                    self.cut_selection(cx);
                    return;
                }
                "a" | "A" => {
                    self.select_all(cx);
                    return;
                }
                // ⌘F 查找；⌘G / ⌘⇧G 下一个 / 上一个（与系统「查找」同一套手感）。
                "f" | "F" => {
                    self.open_find(cx);
                    return;
                }
                "g" | "G" => {
                    if self.find_open {
                        self.find_step(if shift { -1 } else { 1 }, cx);
                    }
                    return;
                }
                // ⌘← / ⌘→ 行首行尾、⌘↑ / ⌘↓ 全文首尾（v1.9.68）。
                // 三百万字的稿子里，用方向键一行行挪到段首是折磨；这两组是
                // macOS 文本控件的通用约定，缺了就会「手感不像原生」。
                "left" => {
                    self.line_home(shift, cx);
                    return;
                }
                "right" => {
                    self.line_end(shift, cx);
                    return;
                }
                "up" => {
                    self.move_cursor_to(0, shift, cx);
                    return;
                }
                "down" => {
                    let total = self.text.chars().count();
                    self.move_cursor_to(total, shift, cx);
                    return;
                }
                // ⌘N：**掩注**（隐藏夹注小字）——竖排预览就地静默重排。
                "n" | "N" => {
                    self.toggle_gloss(cx);
                    return;
                }
                _ => {}
            }
        }
        match k {
            "left" | "right" | "up" | "down" => self.move_cursor(k, shift, cx),
            "backspace" => self.delete_char(false, cx),
            "delete" => self.delete_char(true, cx),
            "home" => self.line_home(shift, cx),
            "end" => self.line_end(shift, cx),
            "enter" | "return" => self.insert_text("\n", cx),
            "escape" => {
                // 帮助卡优先收：用户按 Esc 想关的第一顺位永远是「刚弹出来的那层」。
                if self.show_help {
                    self.show_help = false;
                } else {
                    self.selection = None;
                }
                cx.notify();
            }
            _ => {}
        }
        self.touch(cx);
    }

    // ── 剪贴板 / 全选 / 查找（v1.9.68）────────────────────────────────────
    //
    // 自绘编辑器意味着这些**全都要自己接**：GPUI 只为它内置的输入框绑定
    // ⌘C/V/X/A。上一版只做了 ⌘V（粘贴），于是「复制一段去别处比对」这件事
    // 在整个工作台里做不到——对一个校对工具来说这是硬伤。

    /// 当前选中的文字（无选区 / 空选区返回 `None`）。
    fn selection_string(&self) -> Option<String> {
        let r = self.selection.as_ref()?;
        let (a, b) = (r.start.min(r.end), r.end.max(r.start));
        if a == b {
            return None;
        }
        let chars: Vec<char> = self.text.chars().collect();
        let b = b.min(chars.len());
        let a = a.min(b);
        Some(chars[a..b].iter().collect())
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.selection_string() {
            crate::clipboard::write_text(cx, s);
        }
    }

    /// 剪切 = 复制 + 删（走 `delete_char`，撤销栈记得住这一步）。
    fn cut_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.selection_string() {
            crate::clipboard::write_text(cx, s);
            self.delete_char(false, cx);
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let total = self.text.chars().count();
        self.anchor = 0;
        self.cursor = total;
        self.selection = if total > 0 { Some(0..total) } else { None };
        self.dragging = false;
        self.undo_barrier = true;
        cx.notify();
    }

    /// 查找词 = 已上屏 + 正在组字（中文查找要打拼音，组字是逐帧替换的）。
    fn find_text(&self) -> String {
        format!("{}{}", self.find_query, self.find_marked)
    }

    fn open_find(&mut self, cx: &mut Context<Self>) {
        if self.find_open {
            // 再按一次 ⌘F：收起（与系统查找条同手感）。
            self.close_find(cx);
            return;
        }
        self.find_open = true;
        self.find_query.clear();
        self.find_marked.clear();
        self.find_hits.clear();
        self.find_idx = 0;
        self.find_sel = None;
        cx.notify();
    }

    fn close_find(&mut self, cx: &mut Context<Self>) {
        self.find_open = false;
        self.find_query.clear();
        self.find_marked.clear();
        self.find_hits.clear();
        self.find_sel = None;
        cx.notify();
    }

    /// 查找条打开时的按键分流。返回 `true` = 已消费，不再进编辑逻辑。
    fn find_key(&mut self, k: &str, shift: bool, cmd: bool, cx: &mut Context<Self>) -> bool {
        match k {
            "escape" if !cmd => {
                self.close_find(cx);
                true
            }
            "enter" | "return" if !cmd => {
                self.find_step(if shift { -1 } else { 1 }, cx);
                true
            }
            "backspace" if !cmd => {
                self.find_query.pop();
                self.find_marked.clear();
                self.find_refresh(cx);
                true
            }
            "delete" if !cmd => {
                self.find_query.clear();
                self.find_marked.clear();
                self.find_refresh(cx);
                true
            }
            // ⌘F 再按收起；⌘G / ⌘⇧G 上/下一个。
            "f" | "F" if cmd => {
                self.close_find(cx);
                true
            }
            "g" | "G" if cmd => {
                self.find_step(if shift { -1 } else { 1 }, cx);
                true
            }
            // ⌘A 在查找条里没有「全选查找词」的必要，吃掉它，别让文档被全选。
            "a" | "A" if cmd => true,
            _ => false,
        }
    }

    /// 重算命中并跳到「离光标最近的那一处」。
    ///
    /// 命中存**字节偏移**（`str::find` 的原生单位），只在跳转时换算成字符下标：
    /// 三百万字的稿子里，若每个命中都做一次字节→字符遍历，光换算就是几十毫秒。
    fn find_refresh(&mut self, cx: &mut Context<Self>) {
        let q = self.find_text();
        self.find_hits.clear();
        self.find_sel = None;
        self.find_idx = 0;
        if q.is_empty() {
            cx.notify();
            return;
        }
        let mut from = 0usize;
        while from <= self.text.len() {
            let Some(off) = self.text[from..].find(&q) else {
                break;
            };
            let abs = from + off;
            self.find_hits.push(abs);
            // 上限防病态输入（查找 "的" 这种字在古籍里能命中几万处）。
            if self.find_hits.len() >= 5000 {
                break;
            }
            from = abs + q.len().max(1);
        }
        let cursor_byte: usize = self
            .text
            .chars()
            .take(self.cursor)
            .map(char::len_utf8)
            .sum();
        let idx = self
            .find_hits
            .iter()
            .position(|&o| o >= cursor_byte)
            .unwrap_or(0);
        self.find_goto(idx, cx);
    }

    /// 跳到第 `idx` 处命中：高亮 + 光标落段尾 + 滚到可见。
    fn find_goto(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(&off) = self.find_hits.get(idx) else {
            cx.notify();
            return;
        };
        let q = self.find_text();
        let a = self.text[..off].chars().count();
        let b = a + q.chars().count();
        self.find_idx = idx;
        self.find_sel = Some(a..b);
        // 光标落在命中末尾：用户看完可以直接接着改。
        self.cursor = b;
        self.anchor = b;
        self.selection = None;
        self.undo_barrier = true;
        self.ensure_caret_visible(cx);
        cx.notify();
    }

    fn find_step(&mut self, dir: isize, cx: &mut Context<Self>) {
        let n = self.find_hits.len();
        if n == 0 {
            return;
        }
        let idx = if dir >= 0 {
            (self.find_idx + 1) % n
        } else {
            (self.find_idx + n - 1) % n
        };
        self.find_goto(idx, cx);
    }

    /// **光标跟随**（v1.9.68）：打完字 / 回车 / 方向键之后，光标若跑出视口，
    /// 就把它滚回来——长文校对时「打着打着光标不见了」是最劝退的体验之一。
    ///
    /// 口径与 `caret_at` 完全对称：光标在第几条**视觉行** → 该行 y 区间 →
    /// 和视口区间比。留一行余量，免得每敲一下屏幕都跟着抖。
    fn ensure_caret_visible(&mut self, cx: &mut Context<Self>) {
        // 对照模式左右共用 `diff_scroll`，强行滚动会打乱段落对齐，故不动。
        if self.show_diff || self.viewport_w <= 0.0 {
            return;
        }
        let vw = self.viewport_w;
        let cpl = self.chars_per_line(vw);
        let total = self.total_chars();
        let cursor = self.cursor.min(total);
        // 视觉行号走行索引（v1.9.69）：旧实现每敲一个字都 `split('\n')` 全本扫一遍，
        // 百万字就是「每打一个字扫五百万次」——光标跟随还没到，键盘已经卡了。
        let visual_line = if self.index_valid() {
            match self.lines.line_of(cursor) {
                Some(li) => {
                    let (cs, _) = self.lines.char_range(li);
                    self.lines.vis[li] + cursor.saturating_sub(cs) / cpl
                }
                None => 0,
            }
        } else {
            let mut visual_before = 0usize;
            let mut start = 0usize;
            for line in self.text.split('\n') {
                let len = line.chars().count();
                if cursor <= start + len {
                    break;
                }
                visual_before += est_wrapped_lines(line, cpl);
                start += len + 1;
            }
            visual_before + cursor.saturating_sub(start) / cpl
        };
        let top = ED_PAD + visual_line as f32 * LINE_H;
        let bottom = top + LINE_H;
        let view_h = self.editor_view_h();
        let off = -px_f32(self.scroll.offset().y);
        let margin = LINE_H;
        let mut target = off;
        if top - margin < off {
            target = (top - margin).max(0.0);
        } else if bottom + margin > off + view_h {
            target = (bottom + margin - view_h).max(0.0);
        }
        if (target - off).abs() > 0.5 {
            let max = px_f32(self.scroll.max_offset().height).max(0.0);
            let y = (-target).clamp(-max, 0.0);
            self.scroll.set_offset(Point::new(px(0.0), px(y)));
            cx.notify();
        }
    }

    /// **滚轮强接**（v1.9.65，用户「绝对强制」铁律 ③）。
    ///
    /// `render_editor` 里 `InputField` 外面那层透明容器在 Bubble 阶段**先于**
    /// 滚动容器的内建滚轮处理收到事件（gpui 冒泡按叶子优先），这里手动把
    /// 增量写进 `ScrollHandle` 再掐断传播——事件被谁吞都吞不掉，内建处理
    /// 也不会跟着再滚一次（双滚）。公式与 gpui 内建（div.rs `paint_scroll_
    /// listener` / `clamp_scroll_position`）**逐字相同**：`offset += delta`
    /// 然后 clamp 到 `[-handle.max_offset(), 0]`——`max_offset` 正是 gpui
    /// 自己从真实布局算出的滚动余量（div.rs:1762 存进 handle），零估算。
    ///
    /// 已审计 `ime::InputField`：它只注册焦点 / IME（`window.handle_input`），
    /// **不碰滚轮**——这层是双保险，不是补丁。
    fn forward_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        // 对照模式：左右两栏共用 `diff_scroll`，由 GPUI 内建统一滚（左栏毛坯
        // 视口一起动、段落对齐不漂）。此处若接管会绕开左栏口径 → 放行且
        // **不掐传播**，让内建处理照常收尾。
        if self.show_diff {
            return;
        }
        let dy = px_f32(ev.delta.pixel_delta(px(LINE_H)).y);
        if dy == 0.0 {
            return;
        }
        let cur = self.scroll.offset();
        let max = px_f32(self.scroll.max_offset().height).max(0.0);
        if max <= 0.0 {
            // 排障口供（真机若再出现「滚不动」，看这行就分得清责任）：
            // 事件到了、公式也跑了，但 gpui 算出的滚动余量是 0 —— 那不是事件
            // 被吞，是**滚动视口的高度又塌了**（视口 == 内容高），去查
            // render_editor 的高度链，别再改滚轮。
            eprintln!(
                "[collation] 滚轮已接管但 max_offset=0：视口高度塌了（内容没高出视口），\
                 这是布局问题不是事件问题"
            );
        }
        let next = (px_f32(cur.y) + dy).clamp(-max, 0.0);
        if next != px_f32(cur.y) {
            self.scroll.set_offset(Point::new(cur.x, px(next)));
            cx.notify();
        }
        // 掐断传播：滚动容器的内建滚轮处理晚于本层收到同一事件，不掐就双滚。
        cx.stop_propagation();
    }

    /// 左栏实际宽度（CSS 像素）：决定右栏左缘，进而决定编辑器内容的 X 原点。
    ///
    /// 三种情形：
    ///   * 原文对照开（`show_diff`）：左栏是 Raw 毛坯，占整屏 **50%**；
    ///   * 纯文本模式（无图）：左栏为 0（右栏占满整屏）；
    ///   * 正常有图模式：左栏原图占 **40%**。
    /// `render_right` 的右栏宽 = `vw - left_col_w(vw)`，二者必须同源。
    fn left_col_w(&self, vw: f32) -> f32 {
        // 原文对照开启时，无论是否纯文本模式，左栏都占 50% 给 Raw 毛坯、右栏 50%
        // 给清洗后正文——纯文本模式（无原图）同样需要这份「逐字比对」视图。
        if self.show_diff {
            vw * 0.5
        } else if self.pure_text_mode {
            0.0
        } else {
            vw * 0.4
        }
    }

    /// 右侧编辑器的**内容原点**（正文第一行第一字的窗口坐标）。
    ///
    /// 四个必须一起算的量，漏任何一个都会让「点第 N 行」落到别的行上：
    ///  ① 全局工具条 `HEADER_H`；
    ///  ② 编辑区上面那两条状态条（文本导入提示 24 / 联动提示 26，没有就是 0）；
    ///  ②′ 快捷标点栏 `PUNCT_BAR_H`（v1.9.25 起夹在状态条与正文之间）；
    ///  ③ 编辑器自身的 `ED_PAD`；
    ///  ④ 滚动偏移（`scroll.offset().y`，向下滚为**负**）。
    ///
    /// 真机「点哪高亮哪失效」的第二层死因就在这儿：上一版只减了 `HEADER_H + PAD`，
    /// 既没算两条状态条也没算滚动，于是点第 10 行常被算成第 8 行，滚过一屏之后
    /// 更是整段错位——拿错的**行号**去查原图区域，自然一处都对不上。
    ///
    /// ⚠️ 行内点击已**不再依赖**这个估算：`render_line` 给每一行挂了直达
    /// `link_to_paragraph(行号)` 的点击（见 [`Self::click_line`]），行号由结构给出、
    /// 不是量出来的。这里只服务「点在正文之外的空白处」这一种兜底情形。
    fn editor_origin(&self, vw: f32) -> (f32, f32) {
        // 纯文本模式（一键笺校进无图的书）：没有左栏原图，右栏占满整屏，
        // 编辑器内容左缘就是 `ED_PAD`，**不是** `vw*0.4 + ED_PAD`——这正是真机
        // 「点哪光标歪到行首」的 X 轴死因：原点偏右了 40% 屏宽，列位被夹成 0。
        // 原文对照开启时左栏是 50% 的 Raw，原点随之右移。
        let origin_x = self.left_col_w(vw) + ED_PAD;
        // v1.9.63：对照模式编辑器滚在 `diff_scroll` 上，坐标口径要跟着走。
        let scroll_y = px_f32(if self.show_diff {
            self.diff_scroll.offset().y
        } else {
            self.scroll.offset().y
        });
        // v1.9.68：快捷标点栏**不再占布局**（改成顶部悬停浮层），
        // 纯文本提示条也已删除，故 Y 原点只剩「顶栏 + 两条状态条 + 内边距」。
        (
            origin_x,
            HEADER_H + self.head_band_h() + ED_PAD + scroll_y,
        )
    }

    /// 编辑器上方常驻的**状态条总高**（文本导入提示 24 + 联动提示 26）。
    ///
    /// 三处共用：`editor_origin` 的 Y 原点、`ensure_caret_visible` 的可视高度、
    /// 标点栏悬停热区的起点。分头写就会出现「点哪偏一行」这类鬼账。
    fn head_band_h(&self) -> f32 {
        let status = if self.text_note.is_some() { 24.0 } else { 0.0 };
        let link = if self.match_note.is_some() || self.active_para_idx.is_some() {
            26.0
        } else {
            0.0
        };
        status + link
    }

    /// 编辑器滚动视口的高度（px）：右栏高 − 顶部状态条。
    fn editor_view_h(&self) -> f32 {
        (self.viewport_h - HEADER_H - self.head_band_h()).max(120.0)
    }

    /// 当前字体的**单字进距**：实测值优先，未量到退回 `CHAR_W`。
    fn char_adv(&self) -> f32 {
        if self.char_adv > 0.0 {
            self.char_adv
        } else {
            CHAR_W
        }
    }

    /// 每行可容纳的字符数（软换行口径）。
    ///
    /// `render_line` 画选区、`caret_at` 落光标、`render_editor` 估折行**必须同源**：
    /// 差一个字，色块就会比字长一截或短一截，点第 N 个视觉行也会落错列。
    fn chars_per_line(&self, vw: f32) -> usize {
        let adv = self.char_adv();
        let content_w = (vw - self.left_col_w(vw) - 2.0 * ED_PAD).max(adv);
        (content_w / adv).floor().max(1.0) as usize
    }

    /// 由点击/拖拽位置（窗口坐标）推出字符下标。
    ///
    /// 右文编辑器内用「固定行高 + **实测**字宽」换算（仅用于编辑器内落光标 /
    /// 选区）；左图高亮坐标则来自 `SourceRegion` 的真实自然像素映射，二者互不影响。
    /// 换算本身在 [`caret_at`]（纯函数，已覆盖折行 / 越界 / 空文单测）。
    fn caret_from_point(&self, x: f32, y: f32, vw: f32) -> usize {
        let (origin_x, origin_y) = self.editor_origin(vw);
        let local_x = (x - origin_x).max(0.0);
        let local_y = (y - origin_y).max(0.0);
        let cpl = self.chars_per_line(vw);
        let adv = self.char_adv();
        // 快路径（v1.9.69）：行索引在就二分定位，O(log n) + O(行内)。
        // 慢路径（索引未就绪）退回 `caret_at`，与它逐字同源，绝不两套算法。
        if self.index_valid() {
            let visual = (local_y / LINE_H).floor() as usize;
            let col_in_visual = ((local_x / adv) + 0.5).floor() as usize;
            // 对照模式的段落填充会凭空多出空行，故视觉行坐标系要换成 `aligned`。
            let (total, li) = if self.show_diff {
                (
                    self.lines.total_aligned(),
                    self.lines.line_at_aligned(visual),
                )
            } else {
                (
                    self.lines.total_visual(),
                    self.lines.line_at_visual(visual),
                )
            };
            if visual >= total {
                return self.lines.total_chars;
            }
            let (cs, ce) = self.lines.char_range(li);
            let base = if self.show_diff {
                self.lines.aligned[li]
            } else {
                self.lines.vis[li]
            };
            let in_line = visual.saturating_sub(base);
            return cs + (in_line * cpl + col_in_visual).min(ce - cs);
        }
        caret_at(&self.text, cpl, adv, local_x, local_y)
    }

    /// 双击选「句」：以 `pos` 为中心，向前后扩张到最近的句读 / 空白边界，
    /// 返回 `[start, end)` 字符区间。CJK 无空格分词，故以常见句读、括号、引号、
    /// 换行及空白作为句界——双击一次即选中光标所在的整句（而非整段）。
    fn sentence_bounds(&self, pos: usize) -> (usize, usize) {
        let total = self.total_chars();
        let p = pos.min(total);
        let delim = |c: char| -> bool {
            c.is_whitespace()
                || matches!(
                    c,
                    '。' | '！'
                        | '？'
                        | '；'
                        | '，'
                        | '、'
                        | '：'
                        | '（'
                        | '）'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '《'
                        | '》'
                        | '“'
                        | '”'
                        | '\n'
                )
        };
        // **只扫本行**（v1.9.69）：句界里含 `\n`，所以「句」永远落在一个逻辑行内。
        // 旧实现把**整本**倒进 `Vec<char>` 再前后走——百万字双击一次就是 20MB
        // 分配 + 五百万次解码，双击能明显顿一下。现在最多扫一整行（几十字）。
        let (ls, le) = match self.line_of(p) {
            Some(li) => self.line_range(li).unwrap_or((0, total)),
            None => (0, total),
        };
        let local_text = &self.text[self.byte_of_char(ls)..self.byte_of_char(le)];
        let chars: Vec<char> = local_text.chars().collect();
        let n = chars.len();
        let lp = p.saturating_sub(ls).min(n);
        let mut a = lp;
        while a > 0 && !delim(chars[a - 1]) {
            a -= 1;
        }
        let mut b = lp;
        while b < n && !delim(chars[b]) {
            b += 1;
        }
        // 落在标点 / 空白上（区间为空）时，退而选中该单字，避免「双击无反应」。
        if a == b && lp < n {
            b = lp + 1;
        }
        (ls + a, ls + b)
    }

    /// 右文**某一行的行身**被点中：落光标 + 直连左图联动。
    ///
    /// 这是「点哪高亮哪」的**主路径**（v1.9.23）。上一版全靠父容器
    /// `place_caret_from_click` 用「鼠标 y ÷ 行高」反推行号，那是个估算：状态条、
    /// 内边距、滚动偏移任何一项变动都会让它错位，而错一行就意味着**查错了原图
    /// 区域**——表现就是「点了没反应」或「框落在别处」。现在行号由
    /// `render_line` 在渲染时就捕获好传进来，量都不用量。
    ///
    /// 同一个事件里把三件事做完，**不做任何节流**（用户明确要求：点击必须立刻
    /// 触发坐标搜索）：
    ///  ① 落光标 / 清选区（并请求焦点，让随后的中文能直接打进来）；
    ///  ② 按这一行的文字去原图找区域（`link_to_paragraph`）；
    ///  ③ `stop_propagation`，免得父容器的估算路径再算一遍、把行号覆盖成估算值。
    fn click_line(
        &mut self,
        li: usize,
        line_start: usize,
        line_len: usize,
        x: f32,
        y: f32,
        vw: f32,
        vh: f32,
        shift: bool,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 行号**不估**（由 render_line 在渲染时绑好），列位按真实点算：走
        // `caret_from_point` 才能认出「本行折行后的第几条视觉行」；再用本行的
        // 字符区间夹一次，保证绝不落到别的段落去（预估值抖动也不出这一行）。
        let to = self.caret_from_point(x, y, vw);
        let prev_anchor = self.anchor;
        self.cursor = to.clamp(line_start, line_start + line_len);
        // **⇧ 点选扩选**（v1.9.68）：锚点保持在上一次落点，只把光标挪过去——
        // 这是所有原生文本框的标准行为，校对时「从段首 ⇧ 点到段尾」最常用。
        if shift {
            self.anchor = prev_anchor;
            let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
            self.selection = if a == b { None } else { Some(a..b) };
            self.drag_origin = Some((x, y));
            self.dragging = false;
            Self::focus_for_typing(window, &self.focus);
            self.link_to_paragraph(li, vw, vh, cx);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        self.anchor = self.cursor;
        if click_count >= 3 {
            // 三击：选中**整段**（本逻辑行，含行内所有折行）——v1.9.68。
            // 校对时最常见的动作是「整段删/整段改写」，三击是最顺手的入口。
            self.selection = if line_len > 0 {
                Some(line_start..line_start + line_len)
            } else {
                None
            };
        } else if click_count >= 2 {
            // 双击：选中光标所在的「句」（到前后最近标点 / 空白边界）。
            let (a, b) = self.sentence_bounds(self.cursor);
            self.selection = if a < b { Some(a..b) } else { None };
        } else {
            // 单击：只落光标，**绝不当拖拽**——否则轻微移动即触发跨行大选区，
            // 连带把原生 macOS 的蓝色选区招出来、吃掉光标焦点。
            self.selection = None;
        }
        // 记下按下点：拖选必须先有「在正文里按下」这一手（v1.9.67）。
        self.drag_origin = Some((x, y));
        self.dragging = false;
        Self::focus_for_typing(window, &self.focus);
        self.link_to_paragraph(li, vw, vh, cx);
        // 无论联动是否命中都必须重绘：光标位置变了，屏幕上要立刻见到。
        cx.notify();
        cx.stop_propagation();
    }

    fn place_caret_from_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vw = px_f32(window.viewport_size().width);
        let vh = px_f32(window.viewport_size().height);
        let x = px_f32(event.position.x);
        let y = px_f32(event.position.y);
        let prev_anchor = self.anchor;
        self.cursor = self.caret_from_point(x, y, vw);
        self.anchor = self.cursor;
        // 点选定位 → 后续键入另起一步（撤销不许跨过这次点击）。
        self.undo_barrier = true;
        if event.modifiers.shift {
            // ⇧ 点选扩选（v1.9.68）：锚点不跟着走，只把光标挪过去。
            self.anchor = prev_anchor;
            let (a, b) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
            self.selection = if a == b { None } else { Some(a..b) };
        } else if event.click_count >= 3 {
            // 三击选整段（v1.9.68）：与 `click_line` 同一口径，两条点击路径
            // 不能一个有三击、一个没有——那会变成「有时候选得上、有时候选不上」。
            self.selection = self
                .line_of(self.cursor)
                .and_then(|li| self.line_ranges().get(li).copied())
                .filter(|&(s, e)| e > s)
                .map(|(s, e)| s..e);
        } else if event.click_count >= 2 {
            let (a, b) = self.sentence_bounds(self.cursor);
            self.selection = if a < b { Some(a..b) } else { None };
        } else {
            // 单击只落光标，绝不当拖拽。
            self.selection = None;
        }
        self.drag_origin = Some((x, y));
        self.dragging = false;
        // 点进编辑器 = 用户想打字。**用轻量聚焦**而不是 `hard_refocus`：后者为了
        // 救「原生对话框回来后平台丢了输入上下文」，会先 `blur()` 再 `focus()`；
        // 放在每次点击上就是每次点都拆一次输入上下文（闪烁 + 可能丢 first responder）。
        // 点击只需要「确保聚焦」，重建上下文留给 `refocus` 那条专门的路径。
        Self::focus_for_typing(window, &self.focus);
        // 兜底路径：没落在任何一行上（点的是正文下方的空白）时，仍按估算行号联动，
        // 绝不让「点了没反应」出现。
        if let Some(li) = self.line_of(self.cursor) {
            self.link_to_paragraph(li, vw, vh, cx);
        }
        cx.notify();
        self.touch(cx);
        cx.stop_propagation();
    }

    fn extend_selection_from_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 只在左键**按住**时扩展选区。
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        // ① **必须**先在正文里按下过（`drag_origin` 由 click_line / place_caret 置位）。
        //    没有按下点就改选区 = 拿旧的 `anchor` 瞎扩，锚点若是 0 就是「一拖全选」。
        let Some((ox, oy)) = self.drag_origin else {
            return;
        };
        let x = px_f32(event.position.x);
        let y = px_f32(event.position.y);
        // ② 3px 死区：手抖不算拖选（用户口径「稍微一动就全选」的最后一层保险）。
        if !self.dragging && (x - ox).abs() < 3.0 && (y - oy).abs() < 3.0 {
            return;
        }
        self.dragging = true;
        let vw = px_f32(window.viewport_size().width);
        let to = self.caret_from_point(x, y, vw);
        let (s, e) = (self.anchor.min(to), self.anchor.max(to));
        self.cursor = to;
        self.selection = if s == e { None } else { Some(s..e) };
        // 拖选也是「挪过光标」，同样断开合并链。
        self.undo_barrier = true;
        // ⚠️ 这里**不**落盘（v1.9.68）：拖选时 mousemove 是 60~120Hz，若每次都
        // 起一个 1.5 秒的防抖任务，一秒钟就是上百个 task 在抢 executor——
        // 拖起来会明显发涩。落盘统一放在 mouse-up（一次就够）。
        // 光标跟随也不在这里做：拖选是用户主动在找位置，自动滚会打架。
        cx.notify();
    }

    // ── 渲染 ─────────────────────────────────────────────────────────────

    /// **强制重建输入焦点**——原生模态（rfd 文件对话框 / 菜单）返回后的唯一解法。
    ///
    /// 为什么必须是「blur + focus」而不是只 focus：`Window::focus()` 头一句是
    /// `if self.focus == Some(handle.id) { return }`，它以为还聚焦着就直接返回；
    /// 而平台侧的 first responder / 输入法上下文早被 NSOpenPanel 收走了。
    /// 于是「看着在编辑器里，一个字都打不进去」。先 blur 把焦点 id 清空，
    /// 再 focus 一次，平台才会重新把输入上下文交回来。
    ///
    /// `activate_window()` 兜住另一种情况：对话框关掉后本进程未必是 key window，
    /// 若无 key 状态，键盘事件根本不会进到 GPUI。
    fn hard_refocus(window: &mut Window, focus: &FocusHandle) {
        if std::env::var("JIANDU_IME_DEBUG").is_ok() {
            eprintln!("[collation] hard_refocus（重建输入上下文）");
        }
        window.activate_window();
        window.blur();
        window.focus(focus);
    }

    /// **点击时的轻量聚焦**（v1.9.67）：只确保「键盘归编辑器」，**不拆输入上下文**。
    ///
    /// 与 [`Self::hard_refocus`] 的分工：
    ///   * 这里 = 用户拿鼠标点正文。焦点本来就常在编辑器上，`Window::focus()`
    ///     自身会短路；窗口不是 key 时补一次 `activate_window()` 即可。
    ///   * `hard_refocus` = 原生对话框 / 菜单刚关掉，平台侧 first responder 被
    ///     系统收走过，**必须** blur 再 focus 才能重建 IME。那条路只在
    ///     `refocus` 标记为真时走一次，绝不能挂在每次点击上——否则每点一下
    ///     就拆一次输入法上下文，中文输入会时断时续。
    fn focus_for_typing(window: &mut Window, focus: &FocusHandle) {
        window.activate_window();
        window.focus(focus);
    }

    /// 原生对话框关掉之后，**分几次**把输入焦点重新交回编辑器。
    ///
    /// 只补一次是不够的。`activate_window()` 在 macOS 上是**异步投递**的
    /// （`makeKeyAndOrderFront` 走平台 executor），紧跟其后同步执行的
    /// `blur + focus` 很有可能发生在「本窗口还不是 key window」的那一瞬间：
    /// 此时 `window.focus()` 只改了 GPUI 自己的账本，平台侧的 first responder
    /// 与输入法上下文并没有真的交回来 —— 表现就是「文字看得见、光标不给、
    /// 一个字打不进」（用户 2026-11-01 真机第二次复现）。
    ///
    /// 故按 80 / 240 / 520ms 补三次：窗口成为 key 是个几十毫秒级的过场，
    /// 三次里总有一次落在它之后。补焦只是几个状态赋值 + 一次重绘，代价可忽略。
    fn schedule_refocus(&mut self, cx: &mut Context<Self>) {
        self.refocus = true;
        let exec = cx.background_executor().clone();
        cx.spawn(
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    for ms in [80u64, 240, 520] {
                        exec.timer(Duration::from_millis(ms)).await;
                        let Some(entity) = this.upgrade() else { return };
                        // 预览浮层开着时不抢：那会儿焦点该在 `preview_focus` 上。
                        let _ = entity.update(&mut app, |this, cx| {
                            if !this.show_preview {
                                this.refocus = true;
                                cx.notify();
                            }
                        });
                    }
                }
            },
        )
        .detach();
    }

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

    /// **主按钮**（v1.9.24）：实底朱砂 + **全圆角胶囊**，一条工具条上只该有一枚。
    ///
    /// v1.9.22 的 6px 圆角读起来还是「一个按钮」；`rounded_full()` 之后它变成
    /// 一枚**印章边**——朱砂底色 + 胶囊轮廓，是这张桌上唯一的实心色块，
    /// 视线天然落在这里。左右内边距也给足（16），不然胶囊会被文字顶扁。
    fn primary_button(
        &self,
        label: &'static str,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .px(px(16.0))
            .py(px(6.0))
            .bg(rgb(0x8c2b22))
            .rounded_full()
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(0xf7f0dc))
            .shadow_sm()
            .hover(|s| s.bg(rgb(0x7a2419)))
            .child(SharedString::from(label))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, event, window, cx| on_click(c, event, window, cx)),
            )
    }

    /// 「注」开关（v1.9.25）：形制与 [`Self::ghost_button`] 同一档（无边框、
    /// 纸灰底），只有**字色**表达开关态——朱砂 = 框开着，墨 = 已收起。
    /// 单字、不换宽，翻转时顶栏一行不会因为字多字少跳半格。
    fn region_box_chip(&self, cx: &mut Context<Self>) -> gpui::Div {
        let on = self.show_region_box;
        let fg = if on { 0x8c2b22 } else { 0x4a4335 };
        div()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(fg))
            .bg(rgba(0x2B26200A))
            .hover(|s| s.bg(rgba(0x2B26201A)))
            .child(SharedString::from("注"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|c, _, _, cx| {
                    cx.stop_propagation();
                    c.show_region_box = !c.show_region_box;
                    cx.notify();
                }),
            )
    }

    /// **掩注**开关（v1.9.54，同 ⌘N）：朱砂 = 已掩（预览里只剩正文）。
    ///
    /// 为什么要有一枚看得见的钮：⌘N 是给熟手用的，第一次用的人不可能猜得到；
    /// 而「夹注遮住版心」恰恰是竖排预览里最常被抱怨的一件。钮与键同源，都走
    /// [`Self::toggle_gloss`]，不存在两套状态。
    fn gloss_chip(&self, cx: &mut Context<Self>) -> gpui::Div {
        let on = self.hide_gloss;
        let fg = if on { 0x8c2b22 } else { 0x4a4335 };
        div()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(fg))
            .bg(if on {
                rgba(0x8C2B221A)
            } else {
                rgba(0x2B26200A)
            })
            .hover(|s| s.bg(rgba(0x2B26201A)))
            .child(SharedString::from("掩注"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|c, _, _, cx| {
                    cx.stop_propagation();
                    c.toggle_gloss(cx);
                }),
            )
    }

    /// 「原文对照」开关（v1.9.60）：左栏切到导入毛坯、右栏保留清洗后正文。
    /// 无毛坯（如「打开文件」手选、无源文件可解析）时按钮变灰禁用，绝不闪。
    fn diff_chip(&self, cx: &mut Context<Self>) -> gpui::Div {
        let has = self.raw_text.is_some();
        let on = self.show_diff && has;
        let fg = if !has {
            0x9A9184
        } else if on {
            0x8c2b22
        } else {
            0x4a4335
        };
        div()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(fg))
            .bg(if on {
                rgba(0x8C2B221A)
            } else {
                rgba(0x2B26200A)
            })
            .hover(|s| s.bg(if has { rgba(0x2B26201A) } else { rgba(0x2B26200A) }))
            .child(SharedString::from("原文对照"))
            .when(has, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|c, _, _, cx| {
                        cx.stop_propagation();
                        c.toggle_diff(cx);
                    }),
                )
            })
    }

    /// 「清洗日志」入口（v1.9.60）：弹出内嵌极简面板，展示本次清洗的三类统计。
    /// 无毛坯时禁用——日志必以 Raw 为基准。
    fn clean_log_chip(&self, cx: &mut Context<Self>) -> gpui::Div {
        let has = self.raw_text.is_some();
        let on = self.show_sanitize_panel && has;
        let fg = if !has {
            0x9A9184
        } else if on {
            0x8c2b22
        } else {
            0x4a4335
        };
        div()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(fg))
            .bg(if on {
                rgba(0x8C2B221A)
            } else {
                rgba(0x2B26200A)
            })
            .hover(|s| s.bg(if has { rgba(0x2B26201A) } else { rgba(0x2B26200A) }))
            .child(SharedString::from("清洗日志"))
            .when(has, |d| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|c, _, _, cx| {
                        cx.stop_propagation();
                        c.toggle_sanitize_panel(cx);
                    }),
                )
            })
    }

    /// **次级按钮**（v1.9.24）：无边框，只留一层几乎看不见的纸灰底。
    ///
    /// 给【打开文件】【导入 MD】这类「入口」用——它们是渠道，不是动作的高潮。
    /// 底色的 alpha 只有 4%，灰到几乎等于没有；悬停抬到 10%，**动的是底、不是线**，
    /// 所以永远不会出现「便利贴」那种硬边。
    fn ghost_button(
        &self,
        label: &'static str,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(0x4a4335))
            .bg(rgba(0x2B26200A))
            .hover(|s| s.bg(rgba(0x2B26201A)))
            .child(SharedString::from(label))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, event, window, cx| on_click(c, event, window, cx)),
            )
    }

    /// **快捷标点栏的一枚小方块**（v1.9.25）。
    ///
    /// 刻意不做成「灰色按钮」：28×28 的薄片、7px 圆角、4% 墨的底，悬停才浮到
    /// 10%——它得看起来像**贴在纸上的一枚小签**，而不是工具栏里的一颗控件。
    /// 朱砂一个都不用：这条栏是笔，不是印章，抢了色就会跟右上的【竖排预览】打架。
    fn punct_chip(
        &self,
        label: &'static str,
        fs: f32,
        w: f32,
        opt: (f32, f32),
        key: PunctKey,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        // `opt` = **(视觉居中的光学位移)**，只给 `、` 与 `。` 用。
        //
        // 这两个字的墨迹长在全角字身的**左下角**（这是 CJK 标点的规矩，不是
        // 字体缺陷）：几何居中之后，读者看到的是一个「沉在方块左下角的小点」，
        // 真机放大到 4 倍时我一度以为它根本没画出来。位移量按 28px 方块 / 19px
        // 字身算出（字身左下 → 方块正中约 +6.5 / −5），写死在这里，别处别抄。
        let glyph = if opt == (0.0, 0.0) {
            div().child(SharedString::from(label))
        } else {
            div()
                .absolute()
                .left(px((w - fs) / 2.0 + opt.0))
                .top(px((28.0 - fs) / 2.0 + opt.1))
                .child(SharedString::from(label))
        };
        div()
            .relative()
            .w(px(w))
            .h(px(28.0))
            .mr(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.0))
            .bg(rgba(0x2B26200D))
            .hover(|s| s.bg(rgba(0x2B26201F)))
            .cursor_pointer()
            .text_size(px(fs))
            .text_color(rgb(0x4A4335))
            .child(glyph)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, _e: &MouseDownEvent, window, cx| {
                    c.punct_apply(key, cx);
                    // **点完必须把焦点还给编辑器**：GPUI 里点击一个非 focusable
                    // 的 div 不会自动转移焦点，而原生对话框 / 工具栏点击之后
                    // first responder 已经不在输入框上——不还焦点，接下来打的
                    // 中文一个都进不去（正是「点了符号再打字就没反应」的死因）。
                    Self::hard_refocus(window, &c.focus);
                    cx.notify();
                }),
            )
    }

    /// **古籍校对快捷标点栏**（v1.9.25）。
    ///
    /// 挂在编辑器**上方**（状态条之下）。注意它是**布局件**：`editor_origin()`
    /// 的纵向原点已经把 `PUNCT_BAR_H` 算进去了——这一条少不得，否则「点哪落哪」
    /// 会整体上移一整条栏的高度。下边距取 0、内外留白靠 `px(ED_PAD)`，
    /// 让七枚小方块和正文首字**同一条左基线**。
    fn render_punct_bar(&self, cx: &mut Context<Self>) -> gpui::Div {
        // **标点符号走黑体链**（v1.9.25 真机结论）：`、`(U+3001) 与 `。`(U+3002)
        // 在正文那款宋体里根本不出墨——不是太淡，是**一个字都没画**（真机放大
        // 到 4 倍看是纯空的）。同一枚方块里换成无衬线就正常显示，所以这条栏
        // 整体切到 `SourceHanSans → PingFang`：它是**控件**不是正文，不受
        // 「正文必用宋体」那条规矩约束，而标点认不出就等于这条栏白做。
        let chip_font = crate::fonts::family_for(
            crate::types::FontChoice::SourceHanSans,
            crate::types::Script::Modern,
        );
        div()
            .w_full()
            .h(px(PUNCT_BAR_H))
            .flex()
            .flex_row()
            .items_center()
            .px(px(ED_PAD))
            .bg(rgb(PAPER_R))
            .font_family(chip_font)
            // 一道 5% 的发丝线：它是「纸上的另一行」，不是一条分割线。
            .border_b_1()
            .border_color(rgba(0x2B26200D))
            // 五枚符号 + 两枚动作。
            //
            // 字号是**逐枚定的**，不是随手写的：`、` `。` 的墨迹只占全角字身
            // 左下角的一小块，13px 时落成 2px 的点，在浅底上等于没画——所以
            // 这两枚压到 19px；括号与方括号是成对的窄笔画，14px 刚好撑住；
            // 圈号本身接近全角，15px 就够。
            //
            // `⌫` / `⏎` 这两个符号宋体里没有码位（真机上渲染成豆腐块），
            // 所以退格与换行改用**汉字**——这不是妥协，标签比符号更好认。
            .child(self.punct_chip("、", 19.0, 28.0, (9.0, -11.0), PunctKey::Plain("、"), cx))
            .child(self.punct_chip("。", 19.0, 28.0, (9.0, -11.0), PunctKey::Plain("。"), cx))
            // 古籍句读的空心圈：朱砂是渲染层画的，这里只负责把 **○** 这个
            // 字符送进正文（U+25CB，与 `gloss::jodu_of` 认的圈同形）。
            .child(self.punct_chip("○", 15.0, 28.0, (0.0, 0.0), PunctKey::Plain("○"), cx))
            .child(self.punct_chip("（）", 14.0, 34.0, (0.0, 0.0), PunctKey::Wrap("（", "）"), cx))
            .child(self.punct_chip("[]", 14.0, 30.0, (0.0, 0.0), PunctKey::Wrap("[", "]"), cx))
            // 退格 / 换行与编辑键同源，走 `delete_char(false)` / 插 `\n`，
            // 不另写一套删除逻辑（另写必与键盘删字符的行为分岔）。
            .child(self.punct_chip("退格", 10.5, 36.0, (0.0, 0.0), PunctKey::Backspace, cx))
            .child(self.punct_chip("换行", 10.5, 36.0, (0.0, 0.0), PunctKey::Newline, cx))
    }

    /// 顶部返回栏。
    fn render_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        div()
            .h(px(HEADER_H))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            // **去边框化**（v1.9.24）：顶栏底色只比稿纸深半个色阶，下沿那道线压到
            // 6% 墨——它已经不像「一条分割线」，而像两张纸叠在一起时的那道缝。
            // 工具条上三枚控件的边界全部由**自身底色**给出，不再由线给出；这也是
            // GPUI 0.2.2 没有 `backdrop-filter` 时的正解：别假装毛玻璃，把线撤掉。
            .bg(rgb(CHROME))
            .border_b_1()
            .border_color(rgba(0x2B26200F))
            .px(px(18.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    // 一枚「笺」小印：这一屏的名字。压到 18px，只当标点用。
                    .child(crate::shelf::seal_chip("笺", 18.0))
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(13.5))
                            .text_color(rgb(0x2b2620))
                            .child(SharedString::from("笺校")),
                    ),
            )
            .child(div().w(px(18.0)))
            // 两个「渠道」并排在最左（用户口径：打开文件与导入 MD 合并到左侧）。
            // 「替换当前书籍」：已有校对内容时先弹确认框，避免误覆盖（见 `replace_book`）。
            .child(self.ghost_button("替换当前书籍", |c, _, _, cx| c.replace_book(cx), cx))
            .child(self.ghost_button("导入 MD", |c, _, _, cx| c.open_text_picker(cx), cx))
            // 【批量导入】已移除（v1.9.70，用户口径）：工作台是**单书校对台**，
            // 一次只校一本；批量排队只属于书架（书架自己有【批量导入】）。
            // 这个按钮在单书工作台上纯属干扰，还容易被误当成「再开一本」。
            // `open_batch_picker` 与识别队列本身保留——书架的入口还在用它。
            // 「注」：一键显隐**原图高亮框**（v1.9.25）。与阅读器顶栏那枚同名
            // 同义——那边关的是纸上的夹注 / 眉批，这边关的是原图上的联动框，
            // 两处一起关才是「只剩字、只剩图」的安静状态。朱砂 = 框开着。
            .child(self.region_box_chip(cx))
            // 「掩注」（v1.9.54）：隐藏括注 / 夹注，只看正文。与 ⌘N 同源。
            .child(self.gloss_chip(cx))
            // 「原文对照」（v1.9.60）：左栏切到导入毛坯、右栏保留清洗后正文，
            // 逐字比对防误杀。无毛坯（如「打开文件」手选）时按钮变灰禁用。
            .child(self.diff_chip(cx))
            // 「清洗日志」（v1.9.60）：生成 sanitize_report.md 并在访达中打开。
            .child(self.clean_log_chip(cx))
            .child(div().flex_1())
            .child(div().w(px(6.0)))
            // 【？】帮助（v1.9.66）：一枚极简的圆形问号，点开是毛玻璃说明卡。
            //
            // 刻意不做成「打开外部 md」：1) 沙盒里起 `open` 子进程是合规雷；
            // 2) 用户正在校对时被抢到浏览器，回来还得重新落点。内容内嵌在
            // `render_help_overlay`，离线、零依赖、Esc 即回。
            .child(
                div()
                    .size(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(11.0))
                    .border_1()
                    .border_color(rgba(0x2B262026))
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .font_family(song.clone())
                    .text_color(rgba(0x2B2620A6))
                    .hover(|s| s.border_color(rgb(0x8c2b22)).text_color(rgb(0x8c2b22)))
                    .child(SharedString::from("？"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|c, _, _, cx| {
                            c.show_help = true;
                            cx.notify();
                        }),
                    ),
            )
            .child(div().w(px(10.0)))
            // 唯一的实底主按钮。
            .child(self.primary_button(
                "竖排预览",
                |c, _, _, cx| {
                    if c.show_preview {
                        c.close_preview();
                    } else {
                        c.show_preview = true;
                        c.request_preview(cx);
                    }
                    cx.notify();
                },
                cx,
            ))
            .child(div().w(px(6.0)))
            // 【返回阅读】弱化到最右：小字 + 左箭头，无底无框，鼠标上去才转朱砂。
            // 字号从 12 压到 11.5、墨色压到 55%——它是「退路」，不该和【竖排预览】
            // 争同一档视觉重量。
            .child(
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .font_family(song.clone())
                    .text_color(rgba(0x2B26208C))
                    .hover(|s| s.text_color(rgb(0x8c2b22)))
                    .child(SharedString::from("← 返回阅读"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_c, _, _, cx| {
                            cx.emit(CollationEvent::Exit);
                        }),
                    ),
            )
    }

    /// **帮助浮卡**（v1.9.66）：顶栏【？】唤出的毛玻璃说明卡。
    ///
    /// 内容刻意内嵌（不走外部 `USER_GUIDE.md`）：
    ///   ① 沙盒合规——不在 App 里起 `open` 子进程；
    ///   ② 体验——用户正在校对，被抢到浏览器再回来，光标和滚动位置全丢；
    ///   ③ 离线可用——随包 md 是「完整说明」，这里只留**当下最需要的四段**。
    ///
    /// 「毛玻璃」在 GPUI 0.2.2 里没有 `backdrop-filter`，做法与顶栏同一套：
    /// 用 92% 不透明的纸色 + 一道 1px 淡墨边 + 大圆角，靠**层次**而不是模糊
    /// 骗过眼睛（半透明叠在正文上会脏，反而更廉价）。
    fn render_help_overlay(&self, _vw: f32, vh: f32, cx: &mut Context<Self>) -> gpui::Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        // 一行「键 → 说明」。左列窄、右列撑满，读起来像笺注而不是说明文档。
        let line = |k: &'static str, v: &'static str| {
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(72.0))
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(rgba(0x8C2B22CC))
                        .child(SharedString::from(k)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.5))
                        .text_color(rgb(0x3A342C))
                        .child(SharedString::from(v)),
                )
        };
        let head = |t: &'static str| {
            div()
                .w_full()
                .pt(px(6.0))
                .text_size(px(11.0))
                .text_color(rgba(0x2B26208C))
                .child(SharedString::from(t))
        };

        let card = div()
            .w(px(600.0))
            .max_h(px((vh - 90.0).max(240.0)))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(22.0))
            .rounded(px(12.0))
            .bg(rgba(0xF7F2E6F7))
            .border_1()
            .border_color(rgba(0x2B26201F))
            .shadow_lg()
            .font_family(song)
            .on_mouse_down(
                MouseButton::Left,
                // 卡片内部点一下不能关（否则改不了里面的文字）。
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.0))
                            .text_color(rgb(0x2B2622))
                            .child(SharedString::from("笺读 · 使用帮助")),
                    )
                    .child(
                        div()
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(10.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgba(0x2B26208C))
                            .hover(|s| s.text_color(rgb(0x8c2b22)))
                            .child(SharedString::from("✕"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|c, _, _, cx| {
                                    cx.stop_propagation();
                                    c.show_help = false;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            // ① 核心工作流
            .child(head("核心工作流"))
            .child(line("① 导入", "书架【导入书籍】选 PDF / TXT / MD / EPUB；多本批量导入在书架。"))
            .child(line("② 阅读", "书架点封面进阅读器；← → 翻页，Esc 退专注模式。"))
            .child(line("③ 笺校", "书架卡片右下【一键笺校】直达工作台，自动带上原图与正文。"))
            // ② 工作台操作
            .child(head("工作台操作"))
            .child(line("定位", "单击落光标；双击选句；三击选整段；⇧ 点击从上一落点扩选。"))
            .child(line("框选补字", "在左图上拖一个框，命中的字会拼成待补文字，点【补入】插到光标处。"))
            .child(line("撤销", "⌘Z 撤销、⌘⇧Z 重做。连打字算一步，不会一个字退一次。"))
            .child(line("查找", "⌘F 呼出查找条，⌘G / ⌘⇧G 上下一个；再按 ⌘F 或 Esc 收起。"))
            // ③ 特色功能
            .child(head("特色功能"))
            .child(line("原文对照", "左栏导入毛坯 / 右栏清洗后正文，逐字比对，防清洗误杀。"))
            .child(line("清洗日志", "本次导入删了什么、改了什么，一页看完。"))
            .child(line("掩注 ⌘N", "竖排预览与导出时隐去夹注小字；正文一个字都不动。"))
            .child(line("问小笺", "选中一段 → 悬浮钮 → 就地提问，与阅读器同一位先生。"))
            .child(line("竖排预览", "把正文排成笺读风书页，可切单/双叶、底色与界栏。"))
            .child(line("AI 句读", "【更多】→ AI 句读，自动给全文加句读并写回。"))
            // ④ 快捷键
            .child(head("快捷键"))
            .child(line("⌘Z / ⌘⇧Z", "撤销 / 重做"))
            .child(line("⌘C / ⌘X / ⌘V / ⌘A", "复制 / 剪切 / 粘贴 / 全选"))
            .child(line("⌘F / ⌘G", "查找 / 下一个（⌘⇧G 上一个）"))
            .child(line("⌘V / ⌘N", "粘贴 / 掩注"))
            .child(line("← → ↑ ↓", "移动光标（按住 ⇧ 扩选）；⌘← → 行首行尾；⌘↑ ↓ 全文首尾"))
            .child(line("Esc", "关闭当前浮层（查找 / 帮助 / 预览 / 问小笺）"))
            .child(line("⌘Q", "退出（退出前会自动存盘一次，不会丢最后一笔）"))
            .child(
                div()
                    .w_full()
                    .pt(px(8.0))
                    .text_size(px(10.5))
                    .text_color(rgba(0x2B262066))
                    .child(SharedString::from("完整说明见随包 USER_GUIDE.md")),
            );

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100A4D))
            // 点遮罩即关（与清洗日志 / 覆盖确认同一套手势）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|c, _, _, cx| {
                    c.show_help = false;
                    cx.notify();
                }),
            )
            .child(card)
    }

    /// 原文对照左栏：把导入时的「毛坯文本」（未清洗）逐行铺开，只读、可滚。
    /// 与右栏清洗后正文并排，让用户一眼看出清洗引擎删了哪些字、改了哪些字（防误杀）。
    fn render_raw_pane(&self, vw: f32, _cx: &mut Context<Self>) -> gpui::Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        // ⚠️ 不再 `self.raw_text.clone()`（v1.9.69）：几十万字的稿子，每帧复制一份
        //    就是每帧几 MB 的 memcpy，纯浪费。借引用即可。
        let raw: &str = self.raw_text.as_deref().unwrap_or("");
        let mut content = div().w_full().flex().flex_col();
        // 顶部一条标签：明确这是「未清洗」的毛坯，与右栏的清洗后正文区分。
        content = content.child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .py(px(4.0))
                .bg(rgba(0x2B26200D))
                .text_size(px(11.0))
                .text_color(rgb(0x9A9184))
                .child(SharedString::from("原文（导入毛坯 · 未清洗）")),
        );
        // v1.9.63：段落对齐（防错位，极重要）。清洗会增减字数，若左右各滚各的、
        // 只按「总高」同步，段落必然越滚越错。做法：把每一段（按 \n 切）当对齐单元，
        // 两侧各段都填充到「同一段在两栏里较高的那个行数」→ 第 i 段永远停在同一个 y，
        // 零漂移。两侧等宽（各 vw*0.5）、同字体同字号同行距。
        //
        // **虚拟列表**（v1.9.69）：左栏与右栏用**同一份**行索引、同一套对齐前缀和，
        // 只画视口内的段，其余用等高空白撑住。百万字时「左栏全铺」和「右栏全铺」
        // 一样致命——而对照模式恰恰是大部头校对最常用的视图。
        const VIRTUALIZE_ABOVE: usize = 160;
        let n_lines = self.lines.lines();
        if !self.index_valid() || n_lines == 0 {
            // 兜底（索引未就绪，理论上进不来）：只铺前 200 段，绝不整本铺。
            // 宁可少显示，也不能在这一条冷路径上把主线程拖死。
            for (li, seg) in raw.split('\n').take(200).enumerate() {
                let band = if li % 2 == 1 {
                    rgba(0x2B26200A)
                } else {
                    rgba(0x2B262004)
                };
                content = content.child(
                    div()
                        .w_full()
                        .bg(band)
                        .child(
                            div()
                                .w_full()
                                .min_w(px(0.0))
                                .whitespace_normal()
                                .text_size(px(FONT_SIZE))
                                .text_color(rgb(0x333333))
                                .font_family(song.clone())
                                .line_height(px(LINE_H))
                                .child(SharedString::from(seg.to_string())),
                        ),
                );
            }
            return self.render_raw_pane_frame(vw, content);
        }
        let panel_h = (self.viewport_h - HEADER_H).max(120.0);
        let (li0, li1) = if n_lines <= VIRTUALIZE_ABOVE {
            (0, n_lines - 1)
        } else {
            self.visible_line_range(panel_h)
        };
        let top_lines = self.lines.aligned[li0];
        let total_lines = self.lines.total_aligned();
        let bot_lines = total_lines.saturating_sub(
            self.lines
                .aligned
                .get(li1 + 1)
                .copied()
                .unwrap_or(total_lines),
        );
        if top_lines > 0 {
            content = content.child(div().w_full().h(px(top_lines as f32 * LINE_H)));
        }
        for li in li0..=li1 {
            let seg = self.lines.raw_line(raw, li);
            let seg_lines = est_wrapped_lines(seg, self.lines.cpl);
            let max_lines = self.lines.aligned[li + 1] - self.lines.aligned[li];
            // 极淡浅色底纹：逐段勾出左右对应（隔段略深一档，便于眼睛定位）。
            let band = if li % 2 == 1 {
                rgba(0x2B26200A)
            } else {
                rgba(0x2B262004)
            };
            let pad = max_lines.saturating_sub(seg_lines) as f32 * LINE_H;
            content = content.child(
                div()
                    .w_full()
                    .bg(band)
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .whitespace_normal()
                            .text_size(px(FONT_SIZE))
                            .text_color(rgb(0x333333))
                            .font_family(song.clone())
                            .line_height(px(LINE_H))
                            .child(SharedString::from(seg.to_string())),
                    )
                    .child(div().h(px(pad))), // 填充到与右栏同段等高 → 段落对齐
            );
        }
        if bot_lines > 0 {
            content = content.child(div().w_full().h(px(bot_lines as f32 * LINE_H)));
        }
        self.render_raw_pane_frame(vw, content)
    }

    /// 左栏（毛坯）的骨架：宽度、底色、滚动柄、内边距。内容由调用方给。
    fn render_raw_pane_frame(&self, vw: f32, content: gpui::Div) -> gpui::Div {
        div()
            .w(px(self.left_col_w(vw)))
            .h_full()
            .flex()
            .flex_col()
            // 左「纸」压深一档（与正常左栏同款），两张纸靠明度差分开。
            .bg(rgb(PAPER_L))
            // 与右栏同样的滚动容器：长文可滚，命中区才不会错位。
            // ⚠️ `min_h(px(0.0))`：本滚动容器是左栏 flex 列的 `flex_1` 子项，
            // 不设它会被正文撑开、滚轮失效；与右栏编辑器同理（见 `render_editor`）。
            .child(
                div()
                    .id(SharedString::from("collation-raw-scroll"))
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .w_full()
                    .h_full()
                    .overflow_y_scroll()
                    // v1.9.63：对照模式左右**共用**专用 `diff_scroll` 句柄 → 双向像素级同步
                    // （滚轮 / 触控板 / PageUp·Down / 拖滚动条一律同步）。与 `scroll`（编辑器
                    // 常态柄）分离，互不干扰，编辑器交互（焦点 / 光标 / 选区 / IME）不受影响。
                    .track_scroll(&self.diff_scroll)
                    .px(px(ED_PAD))
                    .py(px(ED_PAD))
                    .child(content),
            )
    }

    /// 左侧 40%：原图区。顶部悬浮【打开文件】按钮；下方滚动容器挂 `left_scroll`，
    /// 内容（栅格化位图或 mock 行带）内部用 `relative()` 承载绝对定位的高亮叠加层；
    /// 底部状态条显示导入错误或温和提示（如多页 PDF 仅渲首页）。
    fn render_left(&self, vw: f32, cx: &mut Context<Self>) -> gpui::Div {
        // 原文对照开启：左栏不再是原图，而是导入时的「毛坯文本」（未清洗），
        // 供用户逐字比对清洗引擎删了哪些字、改了哪些字（防误杀）。
        if self.show_diff {
            return self.render_raw_pane(vw, cx);
        }
        // **空状态**（v1.9.22）：一张原图都没有时，绝不把「占位页 + 假区域框」
        // 摆出来——那看起来像一个渲染 Bug（真机反馈原话：「两个灰色线框太突兀」）。
        // 改成一张安静的空纸 + 一句提示，它是「还没开始」，不是「坏了」。
        let empty = self.image_path.is_none() && self.image_mem.is_none();
        let body = if empty {
            self.render_left_empty()
        } else {
            // 外面这层 `div` 只为把 `Stateful<Div>`（滚动容器带 id）拉回普通
            // `Div`，好与空状态同型（两种分支同一个返回类型）。
            div().size_full().child(
                div()
                    .id(SharedString::from("collation-left-scroll"))
                    .w_full()
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.left_scroll)
                    .child(self.render_left_content(vw, cx)),
            )
        };
        div()
            .w(px(self.left_col_w(vw)))
            .h_full()
            .flex()
            .flex_col()
            // 左「纸」压深一档：两张纸摆在一张桌上，靠明度差分开，不靠硬线。
            .bg(rgb(PAPER_L))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(body)
                    // PDF 多页时浮出页码条（左下角）。
                    .child(
                        div()
                            .absolute()
                            .bottom(px(8.0))
                            .left(px(8.0))
                            .child(self.render_pdf_pager(cx)),
                    )
                    // 框选后的确认条（底部居中）。放底部不放顶部：竖排是从右上
                    // 起读的，顶栏一浮就正好压住正文的第一列；左下角已被页码条
                    // 占了，所以居中——离图近，又不遮字。
                    .child(match self.render_pick_bar(cx) {
                        Some(bar) => div()
                            .absolute()
                            .bottom(px(10.0))
                            .left(px(0.0))
                            .right(px(0.0))
                            .flex()
                            .justify_center()
                            .child(bar),
                        None => div().h(px(0.0)),
                    }),
            )
            .child(match self.render_image_status() {
                Some(s) => s,
                None => div().h(px(0.0)),
            })
    }

    /// **左栏空状态**（v1.9.24 定稿）：一张还没有贴原图的纸。
    ///
    /// 五条设计纪律，都是为了让它是「空状态」而不是「错误提示」：
    ///   ① **框要极轻**——一圈 4% 纸灰的发丝边 + 16px 大圆角 + 比左纸亮半档的底，
    ///      读起来是「一张待填的纸」，不是「一个灰色的占位方块」。
    ///      （GPUI 0.2.2 没有 `dashed`，真虚线要靠一串 1px 短横拼，那是结构改动；
    ///        本轮口径是「只改样式」，故用**灰度**换**虚线感**：4% 的线在任何
    ///        屏幕上都已经「虚线化」了。）
    ///   ② 图标是**线稿**不是字形：一本线装书，全是 1px 直边——不用放大镜那种
    ///      斜柄图形，因为 GPUI 0.2.2 的元素不能旋转，斜线只能靠多边形硬画。
    ///   ③ 字号三层递进：15 / 11.5 / 11，含一次**字体切换**（宋体 → 黑体），
    ///      第三行是「格式说明」，用无衬线更轻、更像注脚。
    ///   ④ 朱砂**一处都不用**。空的时候不该有任何"喊人"的颜色。
    ///   ⑤ 提示必须**指向具体的按钮位置**（左上角「打开文件」）。只说「请导入
    ///      原图」等于把找路的活儿又丢回给用户。
    fn render_left_empty(&self) -> gpui::Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        // 第三行用无衬线：它讲的是「支持什么格式」，是注脚不是正文。
        let sans = crate::fonts::family_for(
            crate::types::FontChoice::SourceHanSans,
            crate::types::Script::Modern,
        );
        // 线稿的墨色分三档：外框最实（40%）、书脊次之（30%）、栏线最淡（12%）。
        // 同一张图里三档灰度，是「线稿」而不是「几个灰方块」的关键。
        let line_strong = rgba(0x2B262040);
        let line_mid = rgba(0x2B262033);
        let line_faint = rgba(0x2B26201A);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(330.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .py(px(46.0))
                    .px(px(30.0))
                    // 比左纸亮半档的暖白：像一张**压在上面**的干净稿纸。
                    .bg(rgba(0xFBF6E6D9))
                    .border_1()
                    .border_color(rgba(0x2B262014))
                    .rounded(px(16.0))
                    .child(
                        // ── 线稿：一本摊开的线装书 ──
                        div()
                            .relative()
                            .w(px(54.0))
                            .h(px(42.0))
                            .border_1()
                            .border_color(line_strong)
                            .rounded(px(3.0))
                            // 书脊折线
                            .child(
                                div()
                                    .absolute()
                                    .top(px(5.0))
                                    .left(px(13.0))
                                    .w(px(1.0))
                                    .h(px(32.0))
                                    .bg(line_mid),
                            )
                            // 三针缝线（穿书脊的短横）——线装书的记号
                            .child(
                                div()
                                    .absolute()
                                    .top(px(8.0))
                                    .left(px(8.0))
                                    .w(px(11.0))
                                    .h(px(1.0))
                                    .bg(line_strong),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(20.0))
                                    .left(px(8.0))
                                    .w(px(11.0))
                                    .h(px(1.0))
                                    .bg(line_strong),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(32.0))
                                    .left(px(8.0))
                                    .w(px(11.0))
                                    .h(px(1.0))
                                    .bg(line_strong),
                            )
                            // 右侧三道**横**栏线：示意「页面上有字」。
                            // 早先这里画的是两道竖线，真机上读成了「表格的两列」——
                            // 横线才是「书页有字」的通用符号，竖线只在版心语义下成立。
                            .child(
                                div()
                                    .absolute()
                                    .top(px(12.0))
                                    .left(px(24.0))
                                    .w(px(20.0))
                                    .h(px(1.0))
                                    .bg(line_faint),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(21.0))
                                    .left(px(24.0))
                                    .w(px(20.0))
                                    .h(px(1.0))
                                    .bg(line_faint),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(30.0))
                                    .left(px(24.0))
                                    .w(px(14.0))
                                    .h(px(1.0))
                                    .bg(line_faint),
                            ),
                    )
                    .child(div().h(px(20.0)))
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(15.0))
                            .text_color(rgb(0x4B453A))
                            .child(SharedString::from("等待导入古籍原图")),
                    )
                    .child(div().h(px(7.0)))
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(11.5))
                            .text_color(rgba(0x5A5346B5))
                            .child(SharedString::from("请点左上角「打开文件」导入")),
                    )
                    .child(div().h(px(5.0)))
                    .child(
                        div()
                            .font_family(sans)
                            .text_size(px(11.0))
                            .text_color(rgba(0x7E7666C0))
                            .child(SharedString::from("支持 PDF / PNG · PDF 需带文本层")),
                    ),
            )
    }

    /// 左图内容：已载入原图则贴 `img`（按宽自适应高度），否则画占位页（按
    /// `source_size()` 自然尺寸等比缩放），两种情况都叠加 `regions` 区域参考框与
    /// 当前联动段的高亮矩形——高亮坐标全部来自 `SourceRegion` 的真实 (x,y,w,h)
    /// 自然像素，按 `src_nat` 缩放后落到显示坐标（彻底告别 N*BAND_H Mock）。
    fn render_left_content(&self, vw: f32, cx: &mut Context<Self>) -> gpui::Div {
        let (_nw, nh) = self.source_size();
        // 缩放比与 `scroll_region_to_center` **同源**（都走 `left_scale`）。
        // 一旦两处各算各的，框就会整体偏移几十像素——「框飘了」的经典死因。
        let scale = self.left_scale(vw);
        let disp_h = nh * scale;
        let mut page = div().relative().w_full().h(px(disp_h)).bg(rgb(0xf1e8cf));

        // 两种源二选一：PDF 页走**内存位图**（`ImageSource::Render`，零解码零 IO），
        // 位图原图走文件路径（`gpui::img` 自带解码器）。绝不同时画两张。
        let has_source = self.image_path.is_some() || self.image_mem.is_some();
        if let Some(m) = &self.image_mem {
            page = page.child(img(m.clone()).w_full());
        } else if let Some(p) = &self.image_path {
            page = page.child(img(p.clone()).w_full());
        }
        // 区域参考框：无原图时画出全部 regions（验证映射数学），有原图时仅画
        // 当前联动段（避免遮挡整图）；真实文本层接入后这些框即原图真实文本位置。
        //
        // 「注」关掉（v1.9.25）时整层不画：只剩底图，读图时不被框打扰。
        // 这是纯渲染开关——`regions` 与 `active_para_idx` 一个都不动，
        // 再点一次框原样回到原来的位置。
        if self.show_region_box {
            for (i, r) in self.regions.iter().enumerate() {
                let active = self.active_para_idx == Some(i);
                if has_source && !active {
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
        }

        // 拖选产区（v1.9.33）：按住拖出一块，松手就把里面的字提出来，等一句
        // 「补入」。框本身只画**边界**（1px 朱砂）+ 极淡的底，让底下的字仍然
        // 读得清——这一层是「圈一个范围」，不是「盖住一片」。
        if let Some(r) = self.pick_rect {
            page = page.child(
                div()
                    .absolute()
                    .top(px(r.y0 * scale))
                    .left(px(r.x0 * scale))
                    .w(px((r.width() * scale).max(1.0)))
                    .h(px((r.height() * scale).max(1.0)))
                    .border_1()
                    .border_color(rgba(0x8c2b22aa))
                    .bg(rgba(0x8c2b2214)),
            );
        }

        // 鼠标三件事，全部换算到**原图自然坐标**再存——屏幕坐标会被窗口大小、
        // 滚动位置、缩放比三重影响，存下来一挪窗口就全废。
        page = page
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, e: &MouseDownEvent, _w, cx| {
                    let (nx, ny) =
                        c.natural_from_window(px_f32(e.position.x), px_f32(e.position.y), vw);
                    c.pick_from = Some((nx, ny));
                    c.pick_rect = Some(crate::pdf_text::PickRect::new(nx, ny, nx, ny));
                    // 重新开框 = 上一次的待补文字作废（否则会补进上一次的段落）。
                    c.pick_text = None;
                    c.pick_hits = 0;
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |c, e: &MouseMoveEvent, _w, cx| {
                    // 只有「按着左键拖」才画框；光是把鼠标移过去不算。
                    if e.pressed_button != Some(MouseButton::Left) {
                        return;
                    }
                    let Some((sx, sy)) = c.pick_from else {
                        return;
                    };
                    let (nx, ny) =
                        c.natural_from_window(px_f32(e.position.x), px_f32(e.position.y), vw);
                    c.pick_rect = Some(crate::pdf_text::PickRect::new(sx, sy, nx, ny));
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |c, _e, _w, cx| c.pick_finish(cx)),
            );
        page
    }

    // ── 左图框选 → 补入右文（v1.9.33）─────────────────────────────────────

    /// 窗口坐标 → **原图自然坐标**。
    ///
    /// 三件事必须一起算，漏任何一个框都会整体偏一截：
    ///  ① 左栏起点：窗口最左（x = 0）、全局天头之下（y = `HEADER_H`）；
    ///  ② 滚动偏移（`left_scroll`，向下滚为**负**）；
    ///  ③ `left_scale`——与画框、与滚动定位**同一个**缩放比。
    ///
    /// 这和 `editor_origin` 是同一类坑：量出来的原点和画出来的原点一旦分叉，
    /// 高亮就会落在错的字上，而且只在某些窗口尺寸下才错，极难复现。
    fn natural_from_window(&self, x: f32, y: f32, vw: f32) -> (f32, f32) {
        let scale = self.left_scale(vw).max(1e-6);
        let scroll_y = px_f32(self.left_scroll.offset().y);
        (
            (x / scale).max(0.0),
            ((y - HEADER_H - scroll_y) / scale).max(0.0),
        )
    }

    /// 松手：把框里的字提出来，等用户点【补入】。
    ///
    /// 三种结局的交代**分轻重**（这条是「不吵人」的关键）：
    ///   * 提到字 → 浮出确认条（预览前 14 字 + 字数）；
    ///   * 拖了一框但没字（图版 / 印章 / 空白）→ 状态条明说换一处再框；
    ///   * 只是点了一下 / 整页无文本层 → **静默**。点一下空白就弹一条提示，
    ///     读图的时候会一直被打断；而无文本层的「需 OCR」早已常驻在左栏状态条，
    ///     再重复一遍纯属噪声。
    fn pick_finish(&mut self, cx: &mut Context<Self>) {
        self.pick_from = None;
        let Some(rect) = self.pick_rect else {
            return;
        };
        let idx = crate::pdf_text::pick_indices(&self.regions, rect);
        let text = crate::pdf_text::join_picked(&self.regions, &idx);
        if !text.is_empty() {
            self.pick_hits = idx.len();
            self.pick_text = Some(text);
            cx.notify();
            return;
        }
        self.pick_text = None;
        self.pick_hits = 0;
        self.pick_rect = None;
        if !rect.is_tap() && !self.regions.is_empty() {
            self.match_note =
                Some("这一框里没有文字（可能是图版 / 印章 / 空白），换一处再框".to_string());
        }
        cx.notify();
    }

    /// 确认条：【补入右文 · N 字】+ 前 14 字预览 + 【✕】。没框到字时不存在。
    fn render_pick_bar(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let text = self.pick_text.as_ref()?;
        let n = text.chars().count();
        let lines = text.lines().count();
        // 预览只取**首段**前 14 字：跨行截断会让人以为原文就是断的。
        // 多段时补一句「共 N 段」——一框下去补进来几段，用户必须能提前看见。
        let first = text.lines().next().unwrap_or("");
        let head: String = first.chars().take(14).collect();
        let more = if first.chars().count() > 14 { "…" } else { "" };
        let preview = if lines > 1 {
            format!("{head}{more} · 共 {lines} 段")
        } else {
            format!("{head}{more}")
        };
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(rgba(0xFBF6E6F2))
                .border_1()
                .border_color(rgba(0x8c2b2240))
                .shadow(vec![BoxShadow {
                    color: rgba(0x2B262026).into(),
                    blur_radius: px(10.0),
                    spread_radius: px(0.0),
                    offset: Point::new(px(0.0), px(2.0)),
                }])
                .child(
                    div()
                        .font_family(song.clone())
                        .text_size(px(11.5))
                        .text_color(rgba(0x2B2620A6))
                        .max_w(px(150.0))
                        .child(SharedString::from(preview)),
                )
                .child(
                    // 主动作：胶囊实底朱砂（与主按钮同一套语言，但小一号——
                    // 它是一次「插入」，不是一屏的主入口）。
                    div()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded_full()
                        .cursor_pointer()
                        .text_size(px(11.5))
                        .text_color(rgb(0xFBF6E6))
                        .bg(rgb(0x8c2b22))
                        .hover(|s| s.bg(rgb(0x9d352a)))
                        .child(SharedString::from(format!("补入 {n} 字")))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|c, _, window, cx| {
                                // 关键：不让这次点击冒泡到左图画布的 on_mouse_down，
                                // 否则点按钮会顺手清掉选区（然后就补不进去了）。
                                cx.stop_propagation();
                                c.pick_commit(window, cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(rgba(0x2B26208C))
                        .hover(|s| s.text_color(rgb(0x8c2b22)))
                        .child(SharedString::from("✕"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|c, _, _, cx| {
                                cx.stop_propagation();
                                c.pick_text = None;
                                c.pick_rect = None;
                                c.pick_hits = 0;
                                cx.notify();
                            }),
                        ),
                ),
        )
    }

    /// 把待补文字插进右文，落点规则与手打一致（有选区就替换选区）。
    fn pick_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(raw) = self.pick_text.take() else {
            return;
        };
        let hits = self.pick_hits;
        self.pick_hits = 0;
        self.pick_rect = None;

        // 插入点不在行首且前面还有字 → 先断一行，免得把新段黏在上一段尾巴上。
        // 有选区时不断行：那是「用底本覆盖这一段」，覆盖完接着原样往下。
        let replaced = self
            .selection
            .as_ref()
            .map(|r| r.end.saturating_sub(r.start))
            .unwrap_or(0);
        let at_line_start = self.cursor == 0
            || self.text.chars().nth(self.cursor.saturating_sub(1)) == Some('\n');
        let added = raw.chars().count();
        let payload = if replaced == 0 && !at_line_start {
            format!("\n{raw}")
        } else {
            raw
        };
        let from = match self.selection.as_ref() {
            Some(sel) => sel.start.min(sel.end),
            None => self.cursor,
        };
        self.insert_text(&payload, cx);
        // 补完**把刚补进来的这段选中**。三个作用：
        //  ① 用户一眼看见补进来的是什么（不用在一屏字里找）；
        //  ② 编辑器没有 undo，选中就是「软撤销」——按一下 Delete 就回到原样；
        //  ③ 若补错了，直接打字覆盖，不必先删一段再写。
        let to = self.cursor;
        if to > from {
            self.selection = Some(from..to);
        }
        // 插完把焦点还给编辑器：不然接着打字会打进虚空（左图画布没有输入上下文）。
        Self::hard_refocus(window, &self.focus);
        self.match_note = Some(if replaced > 0 {
            format!("已用原图文字替换 {replaced} 字（取自 {hits} 处）")
        } else {
            format!("已从原图补入 {added} 字（取自 {hits} 处）")
        });
        cx.notify();
    }

    /// 导入原图（PDF / PNG）：弹出系统文件选择框（rfd **前台直调**），
    /// 选中后在异步块里把 PDF 首页 / PNG 预栅格化为位图路径，存入 `image_path`。
    fn open_image_picker(&mut self, cx: &mut Context<Self>) {
        // rfd 同步对话框必须跑在主线程（见 IME 笔记 / reader.rs）：用 cx.spawn 前台
        // executor，块内同步调用 pick_file；绝不过 background_executor（会静默 None）。
        cx.spawn(
            |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let picked = rfd::FileDialog::new()
                        .set_title("选择原图（PDF / PNG）")
                        .add_filter("原图 (PDF / PNG)", &["pdf", "png"])
                        .add_filter("全部文件", &["*"])
                        .pick_file();
                    let Some(path) = picked else { return };
                    // 「吃原图」这一套抽到 `ingest_source`，一键笺校的自动装配
                    // （`load_book_auto`）也走同一份，绝不两套逻辑。
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            this.ingest_source(path, cx);
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// **替换当前书籍**（v1.9.59 立项，v1.9.70 修「点了没反应」）。
    ///
    /// 旧实现只在「空工作台」时直接进 `open_image_picker`（**只收 PDF/PNG**），
    /// 有内容时弹确认框、确认后仍然只收原图——用户想换一本 TXT 就永远换不了，
    /// 看着就是「这个按钮是死的」。现在：
    ///   ① 空工作台直接进选择器，有内容先弹确认（防误覆盖）；
    ///   ② 选择器**同时收原图与文本**（PDF / 图片 / TXT / MD），按扩展名分流；
    ///   ③ 选中后先 `detach_old_book()` **切断旧书绑定**（store_id / 撤销栈 /
    ///      选区 / 原图 / 毛坯全清），再把新内容载入当前工作台——
    ///      绝不让新书顶替旧书之后还往旧书的 `collation_state` 里写。
    fn replace_book(&mut self, cx: &mut Context<Self>) {
        if !self.text.is_empty() || self.image_path.is_some() || self.image_mem.is_some() {
            self.replace_confirm = true;
            cx.notify();
        } else {
            self.replace_book_picker(cx);
        }
    }

    /// **切断旧书的一切绑定**（v1.9.70）：换书前必须先做，顺序不能颠倒。
    fn detach_old_book(&mut self) {
        // ① 持久化锚点：不断掉的话，新书的内容会写进**上一本**的 collation_state，
        //    下次打开旧书看到的是别人的正文——这是最难查的一类数据事故。
        self.store_id = None;
        // ② 撤销 / 重做：那些 diff 的字符坐标只对旧文本成立，留着必错位。
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_barrier = true;
        // ③ 光标 / 选区 / 拖拽态 / 组字态。
        self.cursor = 0;
        self.anchor = 0;
        self.selection = None;
        self.marked = None;
        self.drag_origin = None;
        self.dragging = false;
        // ④ 联动与审计缓存。
        self.active_para_idx = None;
        self.active_line = None;
        self.match_note = None;
        self.sanitize_card = None;
        self.show_sanitize_panel = false;
        self.text_file = None;
        self.raw_text = None;
        self.text_note = None;
        self.text_err = false;
        // ⑤ 原图 / PDF / 坐标。
        if let Some(old) = self.image_mem.take() {
            self.image_gc.push(old);
        }
        self.image_path = None;
        self.pdf_path = None;
        self.pdf_cache.clear();
        self.regions.clear();
        self.image_error = None;
        self.image_note = None;
        self.pick_from = None;
        self.pick_rect = None;
        self.pick_text = None;
        self.pick_hits = 0;
        // ⑥ 正文与滚动。
        self.set_text(String::new());
        self.scroll.set_offset(Point::new(px(0.0), px(0.0)));
        self.diff_scroll.set_offset(Point::new(px(0.0), px(0.0)));
        self.left_scroll.set_offset(Point::new(px(0.0), px(0.0)));
        // 索引等下一次 render 重建（`text_gen` 已经 +1）。
    }

    /// 替换书籍的**统一选择器**：原图与文本都收，按扩展名分流。
    ///
    /// 与 `open_image_picker` / `open_text_picker` 的分工：那两个是「在当前书上
    /// 换个图 / 换份稿」，本方法负责「换一本书」——所以一定要先 `detach_old_book`。
    fn replace_book_picker(&mut self, cx: &mut Context<Self>) {
        // rfd 同步对话框必须在主线程前台直调（见 `open_image_picker` 的注释）。
        cx.spawn(
            |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let picked = rfd::FileDialog::new()
                        .set_title("替换当前书籍（原图 / 文本）")
                        .add_filter(
                            "全部支持的格式",
                            &[
                                "pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp",
                                "heic", "txt", "md", "markdown",
                            ],
                        )
                        .add_filter(
                            "原图（PDF / 图片）",
                            &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "heic"],
                        )
                        .add_filter("文本（TXT / Markdown）", &["txt", "md", "markdown"])
                        .add_filter("全部文件", &["*"])
                        .pick_file();
                    let Some(path) = picked else { return };
                    let is_text = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| {
                            let e = e.to_ascii_lowercase();
                            matches!(e.as_str(), "txt" | "md" | "markdown")
                        })
                        .unwrap_or(false);
                    if !is_text {
                        // 原图：走既有的一条完整链路（抽文本层 + 后台栅格化）。
                        if let Some(entity) = this.upgrade() {
                            let _ = entity.update(&mut app, |this, cx| {
                                this.detach_old_book();
                                this.pure_text_mode = false;
                                let label = file_display_name(&path);
                                this.ingest_source(path, cx);
                                this.text_note = Some(format!("已替换原图：{label}"));
                                this.text_err = false;
                                cx.notify();
                            });
                        }
                        return;
                    }
                    // 文本：读盘 / 解码在后台，绝不在主线程读一个大文件。
                    let bg = app.background_executor().clone();
                    let rd = path.clone();
                    let res = bg.spawn(async move { read_text_file(&rd) }).await;
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| match res {
                            Ok((body, note)) => {
                                // **先断旧、再上新**：顺序反了会把旧书的撤销栈 / 光标
                                // 带进新文本，一按 ⌘Z 就跳回别人的段落。
                                this.detach_old_book();
                                this.set_text(body.clone());
                                this.raw_text = Some(body);
                                this.show_diff = false;
                                this.pure_text_mode = true;
                                this.text_file = Some(path.clone());
                                this.text_note = Some(note);
                                this.text_err = false;
                                this.sanitize_card = None;
                                this.schedule_refocus(cx);
                                cx.notify();
                            }
                            Err(e) => {
                                this.text_note = Some(e);
                                this.text_err = true;
                                cx.notify();
                            }
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// **吃原图的核心**（v1.9.59）：给定一份原图（PDF / PNG / 位图）路径，
    /// ① 主线程同步抽取 PDF 文本层坐标（无文本层则清空 regions 并提示需 OCR）；
    /// ② 后台栅格化（CG 绘制该页），完成后回主线程写入内存位图等字段。
    ///
    /// `open_image_picker`（手动选文件）与 `load_book_auto`（一键笺校自动装配）
    /// 共用这一份，保证两条入口行为完全一致。
    fn ingest_source(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // ① 文本层坐标抽取（主线程同步）：带文本层的 PDF 用真实坐标替换 Mock，
        //    无文本层（扫描图 / 位图）extract 返回空 → 清空 Mock，避免高亮落
        //    在错误位置，由状态栏提示需 OCR。
        //
        //    ⚠️ 产品边界（用户 2026-11-01 定）：扫描版 PDF **只渲染、不报错、
        //    不崩溃**——`regions` 清空、`image_error` 显式置 None（**不是**错误），
        //    底部状态栏挂一条「需 OCR」的提示，底图照常渲染。
        let real = crate::pdf_text::extract_pdf_text_regions(&path);
        // 换原图 = 上一次框选的待补文字作废（它属于旧图的坐标与文字）。
        self.pick_from = None;
        self.pick_rect = None;
        self.pick_text = None;
        self.pick_hits = 0;
        if !real.is_empty() {
            self.regions = real;
            self.image_error = None;
            self.image_note = None;
        } else {
            self.regions.clear();
            self.image_error = None; // 绝不把「无文本层」当错误报
            self.image_note = Some(
                "该原图无文本层（扫描图），只作底图显示，无法坐标映射——需 OCR"
                    .to_string(),
            );
        }
        cx.notify();
        // ② 最耗时的栅格化（CG 绘制该页）放到后台 executor，避免阻塞 UI；
        //    完成后回主线程写入内存位图等字段。
        let target_px = self.left_px;
        cx.spawn(
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let bg = app.background_executor().clone();
                    let raster_path = path.clone();
                    let result =
                        bg.spawn(async move { rasterize_source(&raster_path, 1, target_px) })
                            .await;
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            match result {
                                Ok(r) => this.apply_raster(r, &path, cx),
                                Err(e) => {
                                    this.image_error = Some(e);
                                    this.image_note = None;
                                }
                            }
                            cx.notify();
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// 把栅格化结果写进工作台字段（左图各状态），并热进第 2 页缓存。
    fn apply_raster(&mut self, r: Rasterized, path: &Path, cx: &mut Context<Self>) {
        // 删除上一张临时栅格化图（绝不碰用户原文件）。
        if let Some(old) = self.last_temp.take() {
            let _ = std::fs::remove_file(&old);
        }
        // PDF 页走内存位图，位图原图走文件路径——两者互斥。
        // 旧位图挪进 `image_gc` 等下一帧释放纹理。
        if let Some(old) = self.image_mem.take() {
            self.image_gc.push(old);
        }
        self.image_mem = r.image;
        self.image_path = r.path;
        self.last_temp = r.temp;
        // 记录原图自然像素尺寸（PDF 点），供真实坐标映射缩放使用。
        self.src_nat_w = r.nat_w as f32;
        self.src_nat_h = r.nat_h as f32;
        // 多页翻页的凭据：只有真 PDF 才记路径（位图原图翻不动）。
        let is_pdf = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false);
        self.pdf_path = if is_pdf { Some(path.to_path_buf()) } else { None };
        // 换书 → 旧书的页缓存全部作废：页码在另一本书里毫无意义。
        self.pdf_cache.clear();
        self.prefetch_busy = false;
        self.pdf_pages = r.total.max(1);
        self.pdf_page = r.page;
        // 换原图 = 之前联动的段 / 框全部作废（属于旧图坐标）。
        self.active_para_idx = None;
        self.active_line = None;
        self.match_note = None;
        self.left_scroll.set_offset(Point::new(px(0.0), px(0.0)));
        self.diff_scroll.set_offset(Point::new(px(0.0), px(0.0)));
        self.image_error = None;
        // 若此前因无文本层已设 OCR 提示则保留；否则用栅格化 note。
        if self.image_note.is_none() {
            self.image_note = r.note;
        }
        // 原生对话框刚关 → 强制重建输入焦点（见 `refocus`）。
        self.refocus = true;
        // 第 1 页已经在屏幕上：顺手把第 2 页热进缓存，用户第一次按 → 就是零等待。
        self.prefetch_pdf_page(2, cx);
    }

    /// **一键笺校自动装配**（v1.9.59）：把书架那本书的正文与原图**焊死**灌进工作台，
    /// 绝不要求用户再点一次导入。
    ///
    /// `has_image` 为真且 `source_path` 是 PDF / PNG 时左图右文；否则纯文本全屏模式。
    pub fn load_book_auto(
        &mut self,
        source_path: Option<PathBuf>,
        text: String,
        has_image: bool,
        raw_text: Option<String>,
        store_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // 重置与「上一本书」相关的全部状态（文本 / 图像 / 联动 / 翻页缓存）。
        self.set_text(text);
        // 换书 → 清洗审计卡片作废（它比的是「这一本的毛坯 vs 清洗后」）。
        self.sanitize_card = None;
        self.show_sanitize_panel = false;
        // 毛坯文本：供【原文对照】展示。重新进书时清空旧值，避免上一本的
        // 脏数据残留到这一本（没图的书 raw 来自重新解析源文件）。
        // ⚠️ 纯文本模式（无图、`source_path` 非 PDF/PNG 或为空）时上游传来的
        // `raw` 是 `None`——若不兜底，【原文对照】/【清洗日志】按钮会 `has=false`
        // 被禁用、点击直接 early-return（用户实测「点了没反应」）。这里退化为
        // 「与正文同源」，至少让按钮可点、面板有内容；有真实 Raw 时（PDF / 手选
        // MD）上游给的 `raw` 优先，做真·Raw↔Clean 对照。
        self.raw_text = raw_text.or_else(|| Some(self.text.clone()));
        self.show_diff = false;
        self.cursor = 0;
        self.selection = None;
        self.marked = None;
        // v1.9.66：换书即清空撤销历史——上一本书的 ⌘Z 绝不能退进这一本里。
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.active_line = None;
        self.active_para_idx = None;
        self.match_note = None;
        if let Some(old) = self.last_temp.take() {
            let _ = std::fs::remove_file(&old);
        }
        if let Some(old) = self.image_mem.take() {
            self.image_gc.push(old);
        }
        self.image_mem = None;
        self.image_path = None;
        self.pdf_path = None;
        self.pdf_cache.clear();
        self.regions.clear();
        self.image_error = None;
        self.left_scroll.set_offset(Point::new(px(0.0), px(0.0)));
        self.diff_scroll.set_offset(Point::new(px(0.0), px(0.0)));
        // v1.9.64：记住本书 store_id（临时样张为 None，不落库）。
        self.store_id = store_id.clone();
        // v1.9.64：从 `collation_state` 恢复进度（若有且是真书）。
        // 只在「导入文本 + 重置状态」之后做——用快照里的**编辑后全文**覆盖刚
        // 导入的原文，并恢复对照模式 / 光标 / 滚动。毛坯 `raw_text` 不动（仍取
        // 自源文件），这样【原文对照】永远比的是「原稿 vs 你校过的稿」。
        if let Some(sid) = store_id {
            if let Some(saved) = jiandu_core::store::get_collation_state(&sid) {
                if !saved.full_text.is_empty() {
                    self.set_text(saved.full_text);
                }
                self.show_diff = saved.show_diff;
                self.cursor = saved.cursor.min(self.text.chars().count());
                let y = px(saved.scroll_y as f32);
                self.scroll.set_offset(Point::new(px(0.0), y));
                self.diff_scroll.set_offset(Point::new(px(0.0), y));
            }
        }
        self.preview_page = 0;
        if has_image {
            self.pure_text_mode = false;
            if let Some(p) = source_path {
                self.ingest_source(p, cx);
            }
        } else {
            self.pure_text_mode = true;
            self.image_note = Some("纯文本模式：本书无原图，仅显示正文".to_string());
        }
        cx.notify();
    }

    /// v1.9.64：**标脏 + 1.5 秒防抖落盘**。任何编辑 / 光标 / 模式切换后调用。
    ///
    /// 三条铁律（与用户确认）：
    ///  ① **绝不碰 `chapters.db`**——写的是独立的 `collation_state` 一行；
    ///  ② **1.5 秒防抖**——`on_change` 高频触发时只 +1 代际令牌、不写盘；停手 1.5 秒
    ///     后**只写最后一次**状态（`save_gen` 校验，过期即丢弃）；
    ///  ③ **主线程零卡顿**——落盘在后台 executor 跑，百万字（≈ 1MB 串）也不占主线程；
    ///     且 `INSERT OR REPLACE` 单行，绝不全量覆写。
    ///
    /// 临时样张（`store_id` 为 `None`）直接返回，不污染数据库。
    /// v1.9.66：**退出前同步强存**（Exit Flush）。
    ///
    /// 与 [`Self::touch`] 的分工：`touch` 管「平时别丢」（1.5 秒防抖，扔后台线程，
    /// 不卡手），本方法管「合眼前最后一口气」——⌘Q / 关窗 / 系统注销时防抖计时器
    /// 还没到点，那最后一两秒的编辑只在内存里。
    ///
    /// 三条纪律：
    ///   ① **同步**（不 spawn）：退出窗口是 `SHUTDOWN_TIMEOUT`，起线程/排队的
    ///      任务未必来得及跑；一条 REPLACE 是毫秒级，直接写完最稳；
    ///   ② **只碰自己的字段 + sqlite**：本方法由 AppKit 退出栈上的回调调用，
    ///      在里面更新别的 GPUI 实体 = panic 穿 ObjC 栈 = 直接 abort；
    ///   ③ 代际 +1：把可能还在飞的那轮防抖判定作废，避免它稍后写回旧值。
    pub fn flush_now(&mut self) {
        let Some(sid) = self.store_id.clone() else {
            // 临时样张（演示千字文）没有 store_id，本来就不落库。
            return;
        };
        self.save_gen = self.save_gen.wrapping_add(1);
        let Ok(conn) = jiandu_core::store::connect() else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let state = jiandu_core::store::CollationState {
            store_id: sid,
            full_text: self.text.clone(),
            show_diff: self.show_diff,
            cursor: self.cursor,
            scroll_y: px_f32(self.scroll.offset().y) as f64,
            updated_at: now,
        };
        let _ = jiandu_core::store::put_collation_state(&conn, &state);
        // 强存也算「已落盘」，否则退出前刚写过的内容会被防抖任务再写一遍。
        self.saved_gen = self.text_gen;
        self.saved_cursor = self.cursor;
    }

    fn touch(&mut self, cx: &mut Context<Self>) {
        // **光标跟随**（v1.9.68）放在这里：所有「改文本 / 落光标」的路径最后都
        // 会调 `touch`，在这一处兜住，比在每个编辑函数里各写一遍可靠得多。
        // 必须在 `store_id` 判空**之前**——演示样张没有 store_id，同样要能跟随。
        self.ensure_caret_visible(cx);
        let Some(sid) = self.store_id.clone() else {
            return;
        };
        // 代际 +1：本轮定时器触发时若已不等于此值，说明又有了新的编辑，丢弃旧轮。
        self.save_gen = self.save_gen.wrapping_add(1);
        let gen = self.save_gen;
        let exec = cx.background_executor().clone();
        cx.spawn(
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    // 停手 1.5 秒才落盘（高频 on_change 只会不断把上面的 gen 顶掉）。
                    exec.timer(std::time::Duration::from_millis(1500)).await;
                    // 校验代际 + 实时读最新状态（含最新滚动偏移，避免在 touch 时刻抓旧值）。
                    let snapshot = match this.upgrade() {
                        Some(e) => e.update(&mut app, |ws, _cx| {
                            if gen != ws.save_gen {
                                return None;
                            }
                            // **内容没变就不写**（v1.9.69）：`collation_state` 是
                            // 单行 `INSERT OR REPLACE`，几百万字一次就是 15MB
                            // 序列化 + 写盘。拖选收尾、点一下、滚动一下都会走
                            // `touch`——只为挪个光标就把整本书重写一遍毫无意义，
                            // 而且主线程上还要先 `clone` 出那 15MB。
                            if ws.text_gen == ws.saved_gen && ws.cursor == ws.saved_cursor {
                                return None;
                            }
                            // 记账要在**同一帧**里做：写盘在后台，等它回来再记
                            // 就可能被下一轮 touch 抢先，重复写第二次。
                            ws.saved_gen = ws.text_gen;
                            ws.saved_cursor = ws.cursor;
                            Some((
                                ws.text.clone(),
                                ws.show_diff,
                                ws.cursor,
                                px_f32(ws.scroll.offset().y),
                            ))
                        }),
                        None => Ok(None),
                    };
                    let Ok(Some((text, show_diff, cursor, scroll_y))) = snapshot else {
                        return;
                    };
                    // 落盘扔后台线程：一条 REPLACE 最多写 ≈15MB（500 万字），别卡主线程。
                    let _ = exec
                        .spawn(async move {
                            if let Ok(conn) = jiandu_core::store::connect() {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs() as i64)
                                    .unwrap_or(0);
                                let state = jiandu_core::store::CollationState {
                                    store_id: sid,
                                    full_text: text,
                                    show_diff,
                                    cursor,
                                    scroll_y: scroll_y as f64,
                                    updated_at: now,
                                };
                                let _ = jiandu_core::store::put_collation_state(&conn, &state);
                            }
                        })
                        .await;
                }
            },
        )
        .detach();
    }

    /// 导入 Markdown / 纯文本（.md / .markdown / .txt）到右侧编辑器。
    ///
    /// **「源码编辑 + 预览解析」策略的前半步**：这里一个字都不清洗——`#`、
    /// `**`、`---` 原样进编辑器，用户校的是原文、改的也是原文（校对精度靠
    /// 「所见即所改」保住）。清洗放在竖排预览出图前（`share_layout::clean_md`）。
    ///
    /// 不卡死的三道做法：
    ///  ① rfd 同步对话框**前台直调**（走 background 会静默 None，见 reader.rs 笔记）；
    ///  ② 读盘 / 解码扔 background executor——再大的稿子也不压在主线程；
    ///  ③ 超过 `TEXT_MAX_CHARS` 按字截断并明确提示（编辑器逐行自绘，超长会拖）。
    fn open_text_picker(&mut self, cx: &mut Context<Self>) {
        cx.spawn(
            |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let picked = rfd::FileDialog::new()
                        .set_title("选择校对稿（Markdown / 纯文本）")
                        .add_filter("Markdown / 纯文本", &["md", "markdown", "txt"])
                        .add_filter("全部文件", &["*"])
                        .pick_file();
                    let Some(path) = picked else { return };

                    // 读盘在后台：纯 IO，不碰 UI，也不占主线程。
                    let bg = app.background_executor().clone();
                    let rd = path.clone();
                    let res = bg.spawn(async move { read_text_file(&rd) }).await;

                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            match res {
                                Ok((body, note)) => {
                                    this.set_text(body.clone());
                                    // 手选文件：Raw 与清洗后同源（导入即未清洗），
                                    // 让【原文对照】仍然可用且一致；同时清掉旧稿残留。
                                    this.raw_text = Some(body);
                                    // 换稿 → 审计卡片作废（见 `SanitizeCard`）。
                                    this.sanitize_card = None;
                                    this.show_diff = false;
                                    this.cursor = 0;
                                    this.selection = None;
                                    this.marked = None;
                                    // 联动状态属于旧稿，随新稿一起清空，绝不带着
                                    // 上一本书的高亮框进下一本。
                                    this.active_para_idx = None;
                                    this.active_line = None;
                                    this.match_note = None;
                                    // **换稿即清场**（v1.9.24）：竖排预览的版式指纹、
                                    // 旧页图、当前页码全部作废。不清就会「上一份稿的
                                    // 竖排版式还挂在新稿上」——用户看到的正是那种
                                    // 「右侧排版乱套了」的假象：新稿横排、旧预览竖排
                                    // 叠在一起。旧 PNG 挪进待删队列，下一帧统一删盘。
                                    this.preview_sig = None;
                                    this.preview_page = 0;
                                    this.temp_gc.append(&mut this.preview_paths);
                                    this.text_file = Some(path.clone());
                                    this.text_note = Some(note);
                                    this.text_err = false;
                                    // 原生对话框刚关 → **分几次**把焦点交回来。
                                    this.schedule_refocus(cx);
                                }
                                Err(e) => {
                                    this.text_note = Some(e);
                                    this.text_err = true;
                                }
                            }
                            cx.notify();
                            // v1.9.64：换稿 / 改稿后标脏，1.5 秒防抖落盘（覆盖全文快照）。
                            this.touch(cx);
                        });
                    }
                }
            },
        )
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
    /// 预览栅格化的**纯计算核心**（关联函数，不碰 `self`）。
    ///
    /// 可在后台线程安全执行：`share_layout` / `share_paint` 只依赖 CoreGraphics +
    /// CoreText，不涉及 AppKit / 主线程 API。返回 (各页临时 PNG 路径, 温和提示)。
    /// 三重防卡死闸门（字数 / 页数 / 时间预算）依旧生效，但在后台里它们只限制
    /// **一页能装多少字**——分页的唯一口径。
    ///
    /// * `Card`：沿用 v1.9.x 的经验表（[`preview_page_cap`]）。它是「一屏一页」
    ///   的取景卡、不是真实纸张，换算法会让已验证过的观感整体走样，故不动。
    /// * `A4 / A3`：**问版式自己**——拿 `page_pdf_geom` 尺寸排一个字，把「列数 × 行数」
    ///   原样问出来（`CardLayout::capacity`）。绝不手写第二张表：纸张一改、
    ///   列数一改，容量自己就跟着变。
    ///
    /// ⚠️ 尺寸取的是**单页**（`page_pdf_geom`），不是整张纸。双页拼版的 A3
    /// 必须按「半边」算——用整宽算出来的就是那 300 字一页的大字海报。
    fn page_cap(
        paper: Paper,
        imp: Imposition,
        columns: usize,
        bg: ShareBg,
        frame: ShareFrame,
    ) -> usize {
        if paper == Paper::Card {
            return preview_page_cap(columns);
        }
        let g = paper.page_pdf_geom(imp);
        let body = ['道'];
        let red = [false];
        let spec = LayoutSpec {
            body: &body,
            red: &red,
            md: &[],
            note: "",
            show_notes: false,
            sign: "",
            show_sign: false,
            date: "",
            show_date: false,
            columns,
            bg,
            frame,
            seal: ShareSeal::Twin,
            seal_text: "",
            font: ShareFont::Song,
            stamp: "",
            w: g.w,
            h: g.h,
        };
        layout(&spec).capacity().max(1)
    }

    /// **导出 PDF 的纯计算核心**（关联函数，可安全跑在后台线程）。
    ///
    /// 与预览共用同一套「清洗 → 分页 → 排版」，区别只有两点：尺寸用
    /// `Paper::page_pdf_geom()`（72dpi＝pt，矢量），产物是 PDF 而不是 PNG。
    ///
    /// 拼版（v1.22.0）在这里分叉：
    /// * `Single` → 一版一页，媒体框 = 单页（`render_pdf`）；
    /// * `Spread` → 每两个单页并到一张**横放的大纸**上（`render_pdf_spread`），
    ///   媒体框 = 整张纸。**单页尺寸仍是半边**，绝不因为要拼版就去拿整宽排版。
    ///
    /// 返回 `(写出的张数, 落字数, 是否被页数闸门截断)`——`Spread` 时「张数」
    /// 是纸数，一纸两页。
    fn render_pdf_blocking(
        text: &str,
        columns: usize,
        bg: ShareBg,
        frame: ShareFrame,
        paper: Paper,
        imp: Imposition,
        binding: Binding,
        hide_gloss: bool,
        path: &Path,
    ) -> Result<(usize, usize, bool), String> {
        let src = if hide_gloss { strip_gloss(text) } else { text.to_string() };
        let cleaned = clean_md(&src);
        if cleaned.body.is_empty() {
            return Err("正文为空（清洗后一个字都没有）".to_string());
        }
        let body = cleaned.body;
        let md = cleaned.md;
        let cap = Self::page_cap(paper, imp, columns, bg, frame);
        let g = paper.page_pdf_geom(imp);
        let red = vec![false; body.len()];
        let mut sheets = Vec::new();
        let mut start = 0usize;
        while start < body.len() {
            if sheets.len() >= EXPORT_MAX_PAGES {
                break;
            }
            let end = (start + cap).min(body.len());
            let spec = LayoutSpec {
                body: &body[start..end],
                red: &red[start..end],
                md: &md[start..end],
                note: "",
                show_notes: false,
                sign: "",
                show_sign: false,
                date: "",
                show_date: false,
                columns,
                bg,
                frame,
                seal: ShareSeal::Twin,
                seal_text: "笺校",
                font: ShareFont::Song,
                stamp: "",
                w: g.w,
                h: g.h,
            };
            let card = layout(&spec);
            let laid = card.chars.len();
            if laid == 0 {
                break;
            }
            start += laid;
            sheets.push(card);
        }
        if sheets.is_empty() {
            return Err("没有可导出的页面".to_string());
        }
        let chars: usize = sheets.iter().map(|c| c.chars.len()).sum();
        let n = if imp.is_spread() {
            // 一纸两页：媒体框是**整张纸**（横的），单页只是其中的一半。
            let sheet = paper.sheet_pdf_geom(imp);
            render_pdf_spread(path, &sheets, sheet.w, sheet.h, binding)?
        } else {
            render_pdf(path, &sheets)?
        };
        Ok((n, chars, start < body.len()))
    }

    /// 【导出 PDF】：选保存位置 → 后台排版 → 矢量落盘。
    ///
    /// 三条纪律与预览一致：① rfd 同步对话框必须**前台直调**（包进 background
    /// 会静默返回 None）；② 排版 / 落盘全在 background executor，主线程只更新
    /// 状态文字；③ `export_busy` 防重入，连点不会写坏同一个文件。
    fn export_pdf(&mut self, cx: &mut Context<Self>) {
        if self.export_busy {
            return;
        }
        let paper = self.preview_paper;
        let imp = self.eff_imposition();
        let binding = self.binding;
        if paper == Paper::Card {
            self.export_note =
                Some("「預覽卡」是屏幕取景、不是印刷幅面：请先选 A4 或 A3".to_string());
            cx.notify();
            return;
        }
        if self.text.trim().is_empty() {
            self.export_note = Some("正文为空，没有可导出的内容".to_string());
            cx.notify();
            return;
        }
        let default_name = if imp.is_spread() {
            format!("笺校-{}-雙頁.pdf", paper.label())
        } else {
            format!("笺校-{}.pdf", paper.label())
        };
        // 闸门先落下：面板还没弹出来就再点一次，也不会起两份排版任务。
        self.export_busy = true;
        self.export_note = Some(format!("选择保存位置…（{} {}）", paper.label(), imp.label()));
        cx.notify();

        let text = self.text.clone();
        // 掩注（⌘N）在导出侧同样生效：屏幕上掩掉的字，纸上也不该冒出来
        // ——「所见即所印」是这条链的一致口径。
        let hide_gloss = self.hide_gloss;
        let columns = self.preview_columns;
        let bg = self.preview_bg;
        let frame = self.preview_frame;

        cx.spawn(
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    // ① **原生保存面板必须在这里调**（`cx.spawn` 的前台任务），
                    //    绝不能像早先那样在点击回调里直接 `save_file()`：
                    //    鼠标回调跑在 `update_entity` 里，App 的 RefCell 正被
                    //    借出；rfd 的模态会转起 NSApp runloop，期间键盘布局通知
                    //    再进 `App::new_app` 取一次 borrow_mut → 「RefCell already
                    //    borrowed」直接 abort（真机点两次就崩，栈见 2026-11-01）。
                    //    放到 spawn 里，调用点已脱离借用，面板才弹得出来。
                    let picked = rfd::FileDialog::new()
                        .set_file_name(&default_name)
                        .add_filter("PDF", &["pdf"])
                        .save_file();
                    let Some(path) = picked else {
                        // 取消：复位闸门 + 清提示（绝不留下「选择保存位置…」的残影）。
                        if let Some(entity) = this.upgrade() {
                            let _ = entity.update(&mut app, |this, cx| {
                                this.export_busy = false;
                                this.export_note = None;
                                cx.notify();
                            });
                        }
                        return;
                    };

                    // ② 排版 / 落盘全在 background executor，主线程只更新状态文字。
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            this.export_note = Some(if imp.is_spread() {
                                format!(
                                    "正在双页拼版 {}（{}，矢量 PDF）…",
                                    paper.label(),
                                    imp.label()
                                )
                            } else {
                                format!("正在排版 {}（矢量 PDF）…", paper.label())
                            });
                            cx.notify();
                        });
                    }
                    let exec = app.background_executor().clone();
                    let path_bg = path.clone();
                    let out = exec
                        .spawn(async move {
                            CollationWorkspace::render_pdf_blocking(
                                &text,
                                columns,
                                bg,
                                frame,
                                paper,
                                imp,
                                binding,
                                hide_gloss,
                                &path_bg,
                            )
                        })
                        .await;
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            this.export_busy = false;
                            this.export_note = Some(match out {
                                Ok((n, chars, cut)) => {
                                    let tail = if cut {
                                        format!("；已到 {} 页上限，余稿未导出", EXPORT_MAX_PAGES)
                                    } else {
                                        String::new()
                                    };
                                    // 拼版时 `n` 是**纸数**（一纸两页），必须说清，
                                    // 否则用户数页数会发现少了一半。
                                    let unit = if imp.is_spread() {
                                        format!("{n} 张（{} 頁，雙頁拼版）", n * 2)
                                    } else {
                                        format!("{n} 页")
                                    };
                                    format!(
                                        "已导出 {unit} / {chars} 字（{}）→ {}{tail}",
                                        paper.label(),
                                        path.display()
                                    )
                                }
                                Err(e) => format!("导出失败：{e}"),
                            });
                            cx.notify();
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// 单批任务时长，不再阻塞 UI 一分一毫。
    fn render_preview_pages(
        text: &str,
        preview_columns: usize,
        preview_bg: ShareBg,
        preview_frame: ShareFrame,
        paper: Paper,
        imp: Imposition,
        binding: Binding,
        hide_gloss: bool,
    ) -> Result<(Vec<PathBuf>, Option<String>), String> {
        // **掩注**（⌘N，v1.9.54）：先把括注 / 夹注剥掉再进排版——它在 clean_md
        // 之前做，因为括号里的字也是正文流的一部分，必须整段一起消失。
        let src = if hide_gloss { strip_gloss(text) } else { text.to_string() };
        let t0 = std::time::Instant::now();
        // **Markdown 清洗**（预览解析）：编辑器里存的是源码（`#`、`**`、`---`
        // 原样可见），出图前才洗成「干净文字 + 标题/加粗装饰位」。清洗器在
        // `share_layout`（纯函数、零 macOS API），故本步同样可在后台线程跑。
        // 它同时吃掉换行与空行：竖排是连续流，`\n` 只会白占一格。
        let cleaned = clean_md(&src);
        if cleaned.body.is_empty() {
            return Ok((Vec::new(), None)); // 空稿（或全是标记）：浮层显示占位提示。
        }
        let mut body = cleaned.body;
        let mut md = cleaned.md;

        // —— 防卡死闸门 ①：字数上限 ——
        let orig_len = body.len();
        let mut note: Option<String> = None;
        if body.len() > PREVIEW_MAX_CHARS {
            note = Some(format!(
                "长稿仅预览前 {} 字（共 {} 字）",
                PREVIEW_MAX_CHARS, orig_len
            ));
            body.truncate(PREVIEW_MAX_CHARS);
            // 装饰位与正文**必须同步截断**：两边一旦错位，标题放大就会落到
            // 别的字身上（版式错乱，且极难从成图上看出原因）。
            md.truncate(PREVIEW_MAX_CHARS);
        }
        let red = vec![false; body.len()];

        // 按当前**纸张 + 拼版**取整页容量，喂入 ≤ 此值保证不缩字；再用实际落字数推进。
        // 容量是版式实测出来的（见 `page_cap`），不是手写经验表——A4/A3 因此
        // 天然各有一套正确容量，代码里没有第二张魔数表。双页拼版取的也是
        // **半边**的容量：两张 A4 大小的单页，不是一张 A3 的海报。
        let cap = Self::page_cap(paper, imp, preview_columns, preview_bg, preview_frame);
        // 屏幕取景尺寸：单页保竖排形状；整张纸（拼版）是横的。
        // 长边一律封顶 1280（A3 绝不按 300dpi 贴图）。
        let page_geom = paper.preview_page_geom(imp);
        let sheet_geom = paper.preview_sheet_geom(imp);

        // —— 第一步：只排版，攒 CardLayout（不落盘）——
        //
        // 拼版要「两页并一张纸」，所以必须先有整批页面才能决定怎么出图。
        // 一版一张 `CardLayout` 只是几千个坐标，攒 6 张也不占什么内存。
        let mut cards: Vec<_> = Vec::new();
        let mut start = 0usize;
        let mut cut = false; // 是否被闸门 ②/③ 截断
        while start < body.len() {
            // —— 防卡死闸门 ②：页数上限（数的是**单页**，不是纸）——
            if cards.len() >= PREVIEW_MAX_PAGES {
                cut = true;
                break;
            }
            // —— 防卡死闸门 ③：时间预算 ——
            if t0.elapsed().as_millis() > PREVIEW_TIME_BUDGET_MS {
                cut = true;
                break;
            }
            let end = (start + cap).min(body.len());
            let spec = LayoutSpec {
                body: &body[start..end],
                red: &red[start..end],
                // 装饰位随正文同区间切片（与 `body` 严格等长）。
                md: &md[start..end],
                note: "",
                show_notes: false,
                sign: "",
                show_sign: false,
                date: "",
                show_date: false,
                columns: preview_columns,
                bg: preview_bg,
                frame: preview_frame,
                seal: ShareSeal::Twin,
                seal_text: "笺校",
                font: ShareFont::Song,
                stamp: "",
                // 尺寸一律来自 `Paper`：`Card` 仍是 720×1280（老观感，PNG 体积
                // 小、贴图压力低），A4/A3 按比例取样但同一张纸的形状。
                w: page_geom.w,
                h: page_geom.h,
            };
            // 让 layout 自己按整页容量落字；取实际落字数当页边界（双保险，绝不丢字）。
            let card = layout(&spec);
            let laid = card.chars.len();
            if laid == 0 {
                break; // 异常：一字未落，避免死循环。
            }
            cards.push(card);
            start += laid;
        }

        // —— 第二步：出图 ——
        //
        // 单页拼版：一版一张 PNG；双页拼版：两张并一张**横的** PNG（与 PDF 同源：
        // 同一份 `CardLayout`、同一个 `spread_slots` 落点）。屏幕上看的就是印出来的。
        let mut pages: Vec<PathBuf> = Vec::new();
        let write_one = |blob: Result<Vec<u8>, String>,
                             pages: &mut Vec<PathBuf>|
         -> Result<(), String> {
            let png = blob?;
            let out = temp_png_path(); // 纳秒唯一命名，绕开 img 缓存。
            if std::fs::write(&out, &png).is_ok() {
                pages.push(out);
                Ok(())
            } else {
                Err("预览图写入临时文件失败".to_string())
            }
        };
        let rendered: Result<(), String> = if imp.is_spread() {
            let mut r = Ok(());
            for pair in cards.chunks(2) {
                r = write_one(
                    render_spread_png(pair, sheet_geom.w, sheet_geom.h, binding),
                    &mut pages,
                );
                if r.is_err() {
                    break;
                }
            }
            r
        } else {
            let mut r = Ok(());
            for card in &cards {
                r = write_one(render_png(card), &mut pages);
                if r.is_err() {
                    break;
                }
            }
            r
        };
        if let Err(e) = rendered {
            for p in pages.drain(..) {
                let _ = std::fs::remove_file(&p);
            }
            return Err(e);
        }

        if cut {
            note = Some(if imp.is_spread() {
                format!(
                    "长稿仅预览前 {} 张（{} 頁，共 {} 字；余稿未渲以防界面卡死）",
                    pages.len(),
                    cards.len(),
                    orig_len
                )
            } else {
                format!(
                    "长稿仅预览前 {} 页（共 {} 字，余稿未渲以防界面卡死）",
                    pages.len(),
                    orig_len
                )
            });
        }
        // 防卡死排障日志：字数 / 单页数 / 出图数 / 耗时一目了然。
        eprintln!(
            "[preview] chars={}/{} pages={} sheets={} imp={:?} elapsed={}ms",
            body.len(),
            orig_len,
            cards.len(),
            pages.len(),
            imp,
            t0.elapsed().as_millis()
        );
        Ok((pages, note))
    }

    /// 彻底关闭预览：**立即卸载预览图 + 作废在途后台任务**。
    ///
    /// 这是「关闭 / Esc 拥有最高响应优先级」的落点：
    /// ・`preview_gen += 1` → 在途任务的代际立即过期，结果回来即丢弃，绝不回写 UI；
    /// ・`preview_paths.clear()` → `img` 元素随本帧 render 消失，与预览图相关的
    ///   任何解码 / 纹理开销立刻停止。故无论后台排到哪一步，关闭都能当场生效。
    /// 旧页路径挪进 `temp_gc`，**下一帧**才真的删盘（本帧 img 可能还在异步读它，
    /// 当帧删除会让图片加载报错；隔一帧则元素树已换，删了也无人引用）。
    fn close_preview(&mut self) {
        self.show_preview = false;
        self.preview_gen += 1;
        self.preview_pending = false;
        self.temp_gc.append(&mut self.preview_paths);
        self.preview_sig = None;
        self.preview_page = 0;
    }

    /// 后台结果回来时，是否已经**过期**（预览已关 / 又发起了新的一批）。
    ///
    /// 这就是「关闭按钮随时能点」的全部机制：`close_preview` 只做几件 O(1) 的
    /// 状态赋值（不碰磁盘、不等后台），并把 `preview_gen` 加一；在途任务回来
    /// 时用本函数一判，必然 `true` → 结果连临时文件一起丢掉，绝不回写 UI。
    /// 主线程因此从不被后台排版阻塞，无论它排到第几页。
    ///
    /// 抽成纯函数只为能被单测钉死——「关掉以后会不会又闪回来」是用户
    /// 2026-11-01 明确点名要保证的事。
    fn preview_result_stale(show_preview: bool, gen_at_start: usize, gen_now: usize) -> bool {
        !show_preview || gen_at_start != gen_now
    }


    /// **翻一页（← / →）——左右双屏同步走。**
    ///
    /// 校对工具的灵魂就在这里：右侧竖排预览往后翻一页，左侧 PDF 原图也必须
    /// 翻到下一页。否则用户看着第 5 页的原图，对的却是第 2 页的转录，越校越乱。
    ///
    /// 三条纪律：
    ///  ① **两个状态各自钳在自己的边界内**：预览可能只有 6 页（防卡死闸门），
    ///    PDF 却有 300 页；任一侧到底了就只停那一侧，绝不把另一侧也拽住。
    ///  ② **PDF 只有 1 页 / 不是 PDF → 左侧纹丝不动**（`pdf_pages <= 1` 直接跳过），
    ///    连后台任务都不起。
    ///  ③ **左边重栅格化走后台 + 代际令牌**：连按 → 不会排队堵住，旧页结果回来
    ///    时发现代际已过期，连临时文件一起丢掉，绝不「闪回上一页」。
    fn flip_page(&mut self, dir: i32, cx: &mut Context<Self>) {
        if dir == 0 {
            return;
        }
        let mut moved = false;

        // —— ① 右侧：竖排预览文本页（仅在浮层打开时才有意义）——
        if self.show_preview {
            let n = self.preview_paths.len();
            if n > 1 {
                let cur = self.preview_page.min(n - 1);
                let next = if dir < 0 {
                    cur.saturating_sub(1)
                } else {
                    (cur + 1).min(n - 1)
                };
                if next != cur {
                    self.preview_page = next;
                    moved = true;
                }
            }
        }

        // —— ② 左侧：PDF 原图页 ——
        if self.pdf_pages > 1 && self.pdf_path.is_some() {
            let cur = self.pdf_page.max(1);
            let next = if dir < 0 {
                cur.saturating_sub(1).max(1)
            } else {
                (cur + 1).min(self.pdf_pages)
            };
            if next != cur {
                self.pdf_page = next;
                moved = true;
                self.request_pdf_page(next, cx);
            }
        }

        if moved {
            cx.notify();
        }
    }

    /// 后台重栅格化左侧 PDF 的某一页（1-based），成功后换图 + 复位滚动。
    ///
    /// 三条纪律（v1.9.31 重写）：
    ///  ① **缓存优先**：最近看过的几页直接换 `Arc`，零等待（`pdf_cache`）。
    ///     校对最常干的事是「两页之间来回对」——这条才是丝滑的来源。
    ///  ② **图与文本层同一趟后台产**：不再让主线程为 `regions` 多等 10–40ms。
    ///  ③ **代际令牌**：连翻时旧页结果绝不显示（绝不闪回），但**照样塞缓存**——
    ///     用户手滑翻过头再翻回来，那页已经热着了。
    fn request_pdf_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(path) = self.pdf_path.clone() else {
            return;
        };
        self.page_gen += 1;
        let gen = self.page_gen;
        // 换页即作废旧页的文本层坐标与联动状态：右边的段落还没点，左边不该
        // 留着上一页的框。
        self.active_para_idx = None;
        self.active_line = None;
        self.match_note = None;

        // —— ① 缓存命中：连栅格化都不用等，只换个指针 ——
        if let Some(c) = self.pdf_cache.get(&page).cloned() {
            self.page_busy = false;
            self.show_pdf_page(page, &c);
            cx.notify();
            self.prefetch_pdf_page(page + 1, cx);
            return;
        }

        // —— ② 未命中：后台一趟搞定位图 + 文本层 ——
        // 画布目标宽：左栏设备像素（render 每帧刷新；首帧前为 0，退回 1:1）。
        let target_px = self.left_px;
        self.page_busy = true;
        cx.spawn(
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let exec = app.background_executor().clone();
                    let (out, regs) = render_pdf_page(exec, path, page, target_px).await;
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            this.page_busy = false;
                            match out {
                                Ok(r) => {
                                    // 走内存位图后恒为 None；留着是给非 PDF 兜底路径。
                                    if let Some(t) = &r.temp {
                                        let _ = std::fs::remove_file(t);
                                    }
                                    match r.image {
                                        Some(image) => {
                                            let c = CachedPage {
                                                image,
                                                nat_w: r.nat_w,
                                                nat_h: r.nat_h,
                                                regions: regs,
                                            };
                                            let cur = this.pdf_page;
                                            this.cache_page(page, c.clone(), cur);
                                            // 代际过期：只入缓存，绝不动画面。
                                            if gen == this.page_gen {
                                                this.show_pdf_page(page, &c);
                                            }
                                        }
                                        None => {
                                            // 非 PDF（正常进不来）：沿用路径显示。
                                            if gen == this.page_gen {
                                                if let Some(old) = this.image_mem.take() {
                                                    this.image_gc.push(old);
                                                }
                                                this.image_path = r.path;
                                                this.last_temp = r.temp;
                                                this.src_nat_w = r.nat_w as f32;
                                                this.src_nat_h = r.nat_h as f32;
                                                this.regions = regs;
                                                this.image_error = None;
                                                this.image_note = r.note;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    if gen == this.page_gen {
                                        this.image_error = Some(e);
                                    }
                                }
                            }
                            // —— ③ 预取下一页 ——
                            // 只有「这一页确实被显示出来了」才值得预取它的下一页；
                            // 已被翻走的请求不该顺手把算力花在错的方向上。
                            if this.pdf_page == page {
                                this.prefetch_pdf_page(page + 1, cx);
                            }
                            cx.notify();
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// 左栏 PDF 的页码提示（只有多页才值得占一行字）。
    fn pdf_note(&self, page: usize) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if self.pdf_pages > 1 {
            parts.push(format!("第 {} / {} 頁（← → 翻页）", page, self.pdf_pages));
        }
        // 只有**这一页真能取到字**才提示框选。扫描件上写这句等于骗人去框一块
        // 什么也框不出来的图。
        if !self.regions.is_empty() {
            parts.push("框选原图可补入右文".to_string());
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// 把一页成品挂到左栏。
    ///
    /// 缓存命中与后台完成**必须走同一段代码**：否则「第一次看」和「翻回来看」
    /// 会在滚动位置、页码提示、联动框上呈现两种样子——这种不一致最难查。
    fn show_pdf_page(&mut self, page: usize, c: &CachedPage) {
        // 旧的临时图（若有）删掉；PDF 页本就不落盘。
        if let Some(old) = self.last_temp.take() {
            let _ = std::fs::remove_file(&old);
        }
        // 旧的显示位图挪进 `image_gc`：纹理要**下一帧**才 drop（本帧场景可能
        // 仍引用着它，当帧就删会让 gpui 画到一半丢图）。
        if let Some(old) = self.image_mem.take() {
            self.image_gc.push(old);
        }
        self.image_mem = Some(c.image.clone());
        self.image_path = None;
        self.src_nat_w = c.nat_w as f32;
        self.src_nat_h = c.nat_h as f32;
        self.pdf_page = page;
        self.regions = c.regions.clone();
        // 换页 = 上一页的框选作废。这不是洁癖：`pick_text` 是**旧页**的文字，
        // 留着它，用户在新页上直接点【补入】就会把上一页的段落插进正文——
        // 校对比对最怕的就是「补错了页还没人发现」。
        self.pick_rect = None;
        self.pick_text = None;
        self.pick_hits = 0;
        self.pick_from = None;
        // 滚动复位：新页从顶上看起（旧的偏移对不上新页）。
        self.left_scroll
            .set_offset(Point::new(px(0.0), px(0.0)));
        self.image_error = None;
        // 提示**必须在这里算**：`pdf_note` 要读 `regions`（决定要不要提示「可框选
        // 补入」），而 `regions` 正是本函数刚刚换上的。放在调用方算就会拿到上一
        // 页的文本层——扫描页上凭空冒出「可框选」，点了却什么也框不出来。
        self.image_note = self.pdf_note(page);
    }

    /// 把一页塞进缓存，并把「离当前页最远」的旧页挤出去。
    ///
    /// 按距离淘汰而不是 LRU：来回对读时 LRU 会把「刚离开的那页」先踢掉，而它
    /// 恰恰是最可能被翻回来的。距离最远的那页才是真的冷。
    fn cache_page(&mut self, page: usize, c: CachedPage, cur: usize) {
        self.pdf_cache.insert(page, c);
        while self.pdf_cache.len() > PDF_CACHE_CAP {
            let Some(far) = victim_of(self.pdf_cache.keys().copied(), cur) else {
                break;
            };
            self.pdf_cache.remove(&far);
        }
    }

    /// 提前把 `page` 备好塞进缓存（翻过去时零等待）。
    ///
    /// 三条纪律：
    ///  ① 已缓存 / 越界 / 已有预取在途 → 什么都不做；
    ///     （一个在途就够：预取 12ms 完成，手再快也追不上；十个并发反而把后台
    ///     线程池塞满，拖慢用户真正在等的那一页。）
    ///  ② 失败静默：预取是锦上添花，失败就让翻页走正常路径，绝不弹错打扰校对；
    ///  ③ **只进缓存，不动 `image_mem`**：那页还没显示，抢纹理没有意义。
    fn prefetch_pdf_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.prefetch_busy || page == 0 || page > self.pdf_pages {
            return;
        }
        if self.pdf_cache.contains_key(&page) {
            return;
        }
        let Some(path) = self.pdf_path.clone() else {
            return;
        };
        let target_px = self.left_px;
        self.prefetch_busy = true;
        cx.spawn(
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let exec = app.background_executor().clone();
                    let (out, regs) = render_pdf_page(exec, path, page, target_px).await;
                    if let Some(entity) = this.upgrade() {
                        // 预取不改画面，所以**不 notify**：省一次无谓重绘。
                        let _ = entity.update(&mut app, |this, _cx| {
                            this.prefetch_busy = false;
                            let Ok(r) = out else { return };
                            if let Some(t) = &r.temp {
                                let _ = std::fs::remove_file(t);
                            }
                            if let Some(image) = r.image {
                                let cur = this.pdf_page;
                                this.cache_page(
                                    page,
                                    CachedPage {
                                        image,
                                        nat_w: r.nat_w,
                                        nat_h: r.nat_h,
                                        regions: regs,
                                    },
                                    cur,
                                );
                            }
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// 版式指纹（内容 / 列数 / 底色 / 版框）。四个输入完全相同 → 排出来的图必然一模一样。
    ///
    /// 用 `DefaultHasher` 而不是存整份文本：比对比拷贝快，且不受文本长度影响。
    fn preview_sig(
        text: &str,
        columns: usize,
        bg: ShareBg,
        frame: ShareFrame,
        paper: Paper,
        imp: Imposition,
        binding: Binding,
        hide_gloss: bool,
    ) -> (u64, usize, u8, u8, u8, u8, u8, u8) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        // 拼版 / 装订也进指纹：换拼版就是换版式，绝不能拿旧图充数。
        // 掩注同理——它是**派生正文**，必须参与指纹，否则 ⌘N 会被短路成「没反应」。
        (
            h.finish(),
            columns,
            bg as u8,
            frame as u8,
            paper as u8,
            imp as u8,
            binding as u8,
            hide_gloss as u8,
        )
    }

    /// 请求重排预览（**绝不阻塞 UI**）：节流 + 代际令牌 + 后台栅格化。
    ///
    /// ・**节流**：1s 内不重复起任务；连点只挂 `preview_pending`，当前任务回来后
    ///   自动补跑一次（既不堆积任务，也不会「点了没反应」）。
    /// ・**代际令牌** `preview_gen`：连改样式时旧结果回来即丢弃（含临时文件），
    ///   绝不让过期的版面覆盖新样式。
    /// ・**后台执行**：`share_*` 是纯 CG/CT 离屏计算，无主线程依赖，故可安全
    ///   扔到 background_executor，主线程只管 UI。
    /// ・**关闭优先**：结果回来时若预览已关闭，直接丢弃结果 —— 关闭 / Esc 因此
    ///   永远畅通，不受任何后台排版任务影响。
    /// 强制重排（【刷新预览】专用）：先作废版式指纹，再照常走后台。
    ///
    /// 与 `request_preview` 的唯一区别就是「不许被版式指纹短路」——其余纪律
    /// （在途合并、代际令牌、关闭优先）全部照旧，故刷一下也不会卡死。
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_sig = None;
        self.request_preview(cx);
    }

    fn request_preview(&mut self, cx: &mut Context<Self>) {
        // —— 版式指纹：内容 / 列数 / 底色 / 版框都没变就不重排 ——
        //
        // 真机日志实测：同一版式 `[preview] chars=398 pages=4` 成对连打（每次
        // 80–110ms），全是白烧——点样式面板、连点刷新、连按翻页都会重复请求同
        // 一张图。竖排排一次 4 页就要一百毫秒，白烧几十次就是白白占据后台线程、
        // 拖慢其它操作。何况预览图已经渲好，重排只会产出**像素级相同**的新文件。
        //
        // 只在「已经有一批可用的预览图」时短路：预览关掉后 `preview_paths` 为空，
        // 再次打开必须重排（旧文件已进 `temp_gc`）。
        let key = Self::preview_sig(
            &self.text,
            self.preview_columns,
            self.preview_bg,
            self.preview_frame,
            self.preview_paper,
            self.eff_imposition(),
            self.binding,
            self.hide_gloss,
        );
        if !self.preview_paths.is_empty() && self.preview_sig == Some(key) {
            self.preview_pending = false;
            return;
        }
        // ⚠️ `pending` 只能在**真有任务在飞**时置位。
        //
        // 补跑的唯一触发点是「在途任务回来」的那一刻（见下方回调末尾），所以
        // 一旦在没有任务时把 pending 立起来，就再没人来接它 —— 表现是
        // 「预览区一直写着『正文为空』，正文明明有 4928 字」（真机 2026-11-01
        // 截图抓到的就是这个）。
        //
        // 原来这里还叠了一道「1 秒内不重排」的时间节流，正好制造这种死角：
        // 关掉预览（paths 清空、无任务）→ 立刻重开 → 落进时间窗口 → pending
        // 立起来 → 永远没人补跑。而它本来要治的「同一版式成对连打」，上一行
        // 的版式指纹短路已经治好了（同 sig + 有图 → 直接返回），故这个时间
        // 节流纯属多余，删掉。
        if self.preview_busy {
            self.preview_pending = true;
            return;
        }
        self.preview_busy = true;
        self.preview_gen += 1;
        let gen = self.preview_gen;
        let key_at_start = key;
        // 进后台前先把输入快照出来（绝不把 self 带进后台任务）。
        let text = self.text.clone();
        let columns = self.preview_columns;
        let bg = self.preview_bg;
        let frame = self.preview_frame;
        let paper = self.preview_paper;
        let imp = self.eff_imposition();
        let binding = self.binding;
        let hide_gloss = self.hide_gloss;

        cx.spawn(
            // ⚠️ 必须 move：把快照（text/columns/bg/frame/paper/gen）的所有权交给异步块，
            // 否则 async move 捕获的是对局部变量的借用，过不了 'static。
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let exec = app.background_executor().clone();
                    let out = exec
                        .spawn(async move {
                            CollationWorkspace::render_preview_pages(
                                &text,
                                columns,
                                bg,
                                frame,
                                paper,
                                imp,
                                binding,
                                hide_gloss,
                            )
                        })
                        .await;
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            this.preview_busy = false;
                            // 预览已关闭 / 代际过期 → 丢弃这批结果（连临时文件一起清）。
                            //
                            // ⚠️ 这里**绝不能提前 return**：`preview_pending` 的补跑写在
                            // 函数末尾，一旦 return 就把排队中的请求一起吞了 —— 表现是
                            // 「后台明明排好了 6 页，浮层却一直显示『正文为空』」，而且
                            // 再点刷新也没用（那一批图刚被删干净）。真机就踩了这个坑。
                            let stale = Self::preview_result_stale(
                                this.show_preview,
                                gen,
                                this.preview_gen,
                            );
                            if stale {
                                if let Ok((pages, _)) = &out {
                                    for p in pages {
                                        let _ = std::fs::remove_file(p);
                                    }
                                }
                            }
                            if !stale {
                                match out {
                                    Ok((pages, note)) => {
                                        // 先清旧页（绝不碰用户原文件）：挪进待删队列，
                                        // 下一帧 render 统一删盘（见 `temp_gc`）。
                                        this.temp_gc.append(&mut this.preview_paths);
                                        this.preview_paths = pages;
                                        this.preview_note = note;
                                        this.preview_error = None;
                                        // 记住这批图对应的版式指纹：同一请求再来直接跳过。
                                        this.preview_sig = Some(key_at_start);
                                        // 列数 / 样式变动可能改变页数，钳制当前页到有效范围。
                                        let max_idx = this.preview_paths.len().saturating_sub(1);
                                        if this.preview_page > max_idx {
                                            this.preview_page = max_idx;
                                        }
                                    }
                                    Err(e) => {
                                        this.preview_error = Some(e);
                                        this.preview_note = None;
                                    }
                                }
                            }
                            cx.notify();
                            // 期间又有新请求（连点 / 连改样式 / 关掉又打开）→ 立刻补跑一次。
                            // 预览已关就不补跑：那是纯浪费（结果回来还是会被丢弃）。
                            if this.preview_pending {
                                this.preview_pending = false;
                                if this.show_preview {
                                    this.request_preview(cx);
                                }
                            }
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// 右文某一行 → 左图联动：**按文字找**对应区域，高亮 + 滚动居中。
    ///
    /// v1.9.23 重写。上一版按 `regions[行号]` 取区域，隐含「原图文本层切出的
    /// 段数 ≈ 右文行数」这个前提——真实 PDF 一页几十处、右文几百行，前提从第一
    /// 页起就不成立，于是：取不到 → 点了毫无反应；偶尔取到也是错位那一处 →
    /// 高亮框落在不相干的字上。现在改为 [`locate_region`] 按最长公共子串找，
    /// 段数 / 顺序都不再重要。
    ///
    /// 三种结局都要有可见的交代（绝不「点了没反应」）：
    ///   * 找到 → 画框 + 滚动居中 + 状态条报「第几段 → 第几处，连续重合几字」；
    ///   * 没找到 → 状态条明写「未匹配到原图区域」；
    ///   * 无文本层 → 状态条明写需 OCR（扫描版的正常形态，不是错误）。
    fn link_to_paragraph(&mut self, line_idx: usize, vw: f32, vh: f32, cx: &mut Context<Self>) {
        self.active_line = Some(line_idx);

        // ⓪ **纯文本模式：什么联动都不做**（v1.9.68）。
        //
        //    这一模式的定义就是「本书没有原图」，点正文时去算「未加载原图」
        //    只会甩出一条永远消不掉的红色横幅，既占版面又像程序在报错。
        //    真机截图里那句红字压在正文顶上，一屏最贵的位置就这么被浪费了。
        //    静默返回：光标该落哪落哪，版面干干净净。
        if self.pure_text_mode {
            self.active_para_idx = None;
            self.match_note = None;
            return;
        }

        // ① **前置判断：左侧到底有没有原图**（用户 2026-11-01 明确要求）。
        //
        //    顺序绝不能颠倒。上一版先看 `regions` 空不空，而没有把「压根没导入原图」
        //    挡在最前面 —— 于是左侧明明是空的时，程序照样拿右文去和占位数据算一遍
        //    重合度，再把「重合度 4% < 50%」这种**内部指标**甩给用户看。既答非所问，
        //    又像是程序在报错。这里改成三级判断，每一级只说人话：
        //    没图 → 「未加载原图」；有图无文本层 → 「需 OCR」；有文本层 → 才去比对。
        if self.image_path.is_none() && self.image_mem.is_none() {
            self.active_para_idx = None;
            self.match_note = Some(
                "未加载原图：请先点左上角「打开文件」导入 PDF / 图片，再点右文定位"
                    .to_string(),
            );
            cx.notify();
            return;
        }
        if self.regions.is_empty() {
            self.active_para_idx = None;
            self.match_note = Some(
                "该原图无文本层（扫描图），需先 OCR 才能自动定位——现在只作底图显示"
                    .to_string(),
            );
            cx.notify();
            return;
        }

        // ② 这一行没字（空行 / 只有一个标点）：静默清掉上一次的联动，
        //    不弹红字——空行点一下就报「未匹配」是噪声，不是信息。
        let text = self.paragraph_text(line_idx);
        if match_text(&text).chars().count() < 2 {
            self.active_para_idx = None;
            self.match_note = None;
            cx.notify();
            return;
        }

        // ③ 按文字找。
        match locate_region(&text, &self.regions) {
            Some((ri, n)) => {
                // 覆盖率只作**诊断**报给用户看（「这段对得有多齐」），不参与放行：
                // 放行判据是上面那句「连续 ≥ LCS_MIN 字重合」。
                self.active_para_idx = Some(ri);
                // 只报「连续几字重合」。**不报覆盖率**：右文一行常常对不上原图
                // 一列（一行 130 字 ↔ 一列 16 字），覆盖率天然只有个位数，摆出来
                // 只会让人以为没对上——数字在这里是噪声，不是信息。
                self.match_note = Some(format!(
                    "已匹配：第 {} 行 → 原图第 {} 处（连续 {} 字重合）",
                    line_idx + 1,
                    ri + 1,
                    n
                ));
                self.scroll_region_to_center(ri, vw, vh);
            }
            None => {
                self.active_para_idx = None;
                self.match_note = Some(
                    "未匹配到原图区域：本行文字在当前页的文本层里找不到——可翻页再试，\
                     或核对稿件与影印件是否为同一段"
                        .to_string(),
                );
            }
        }
        cx.notify();
    }

    /// 把 `regions[ri]` 滚到左侧视野**正中**。
    ///
    /// 上一次是「顶部对齐」（`-(r.y*scale)`）：短区域会贴到屏幕上沿，长区域
    /// 一半落在视野外，看着像「框没对准」。居中之后，无论区域多高，它都在
    /// 视线中央。
    ///
    /// ⚠️ 缩放比必须与 `render_left_content` **同源**：左栏宽 `vw*0.4`、
    /// 原图自然宽 `source_size().0`。两处一旦分叉，框就会整体偏移——这正是
    /// 「框飘了」的经典死因，所以这里只写一次公式、两边都调它。
    fn scroll_region_to_center(&mut self, ri: usize, vw: f32, vh: f32) {
        let Some(r) = self.regions.get(ri) else {
            return;
        };
        let scale = self.left_scale(vw);
        let top = r.y * scale;
        let h = r.h * scale;
        // 左侧可视高度：视口减去全局天头与底部状态条（有状态条时会矮一截，
        // 这里按最常见的一条 24px 估；偏几像素不影响「居中」这件事）。
        let view_h = (vh - HEADER_H - 24.0).max(160.0);
        let target = (top + h * 0.5 - view_h * 0.5).max(0.0);
        self.left_scroll
            .set_offset(Point::new(px(0.0), px(-target)));
    }

    /// 左栏「原图自然坐标 → 屏幕像素」的唯一缩放比。
    ///
    /// 高亮矩形的绘制（`render_left_content`）与滚动定位（上一函数）都调它。
    /// 分开算过一次，两处差一个 `vw*0.4` 的取整就会让框整体偏几十像素。
    fn left_scale(&self, vw: f32) -> f32 {
        let (nw, _nh) = self.source_size();
        if nw > 0.0 {
            (vw * 0.4) / nw
        } else {
            1.0
        }
    }


    /// 取右文第 `idx` 段的纯文本（用于与原图区域文字做字面相似度校验）。
    fn paragraph_text(&self, idx: usize) -> String {
        self.line_ranges()
            .get(idx)
            .map(|&(s, e)| {
                self.text
                    .chars()
                    .skip(s)
                    .take(e.saturating_sub(s))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 左图原图自然尺寸（PDF 点 / 位图像素）。**只认真实尺寸**。
    ///
    /// 未载入原图时返回一个中性占位（800×1100）：左栏那会儿显示的是空状态卡片，
    /// 这个数字根本不会被用来画框——绝不拿 `regions` 的包围盒倒推一个「假页面」，
    /// 那会在空状态里凭空画出一页纸和几枚空框（真机反馈「两个灰框太突兀」）。
    fn source_size(&self) -> (f32, f32) {
        if self.src_nat_w > 0.0 && self.src_nat_h > 0.0 {
            return (self.src_nat_w, self.src_nat_h);
        }
        (800.0, 1100.0)
    }

    /// 文本导入状态条：显示「已载入哪份稿、多少字」或错误原因。
    ///
    /// 只占 24px 一行，无提示时不占高度（返回 None 由调用方补零高 div）——
    /// 校对区一寸版面都不许浪费在常驻标签上。
    fn render_text_status(&self) -> Option<gpui::Div> {
        let n = self.text_note.as_ref()?;
        let (bg, fg) = if self.text_err {
            (rgba(0xb0342c33), rgb(0xb0342c))
        } else {
            (rgba(0x8c2b2210), rgb(0x8c2b22))
        };
        Some(
            div()
                .h(px(24.0))
                .flex()
                .items_center()
                // 与编辑区同一左基线（`ED_PAD`）：状态条的文字和正文首列对齐，
                // 眼睛才不会在换行处「错一次位」。
                .px(px(ED_PAD))
                .bg(bg)
                .text_color(fg)
                .text_size(px(13.0))
                .child(SharedString::from(n.clone())),
        )
    }

    /// 右侧联动状态条：显示当前已联动段对应的原图区域坐标（自然像素）。
    fn render_link_status(&self) -> gpui::Div {
        // 不匹配提示优先（此时 active_para_idx 为 None，否则会显示空条）。
        if let Some(n) = &self.match_note {
            return div()
                .h(px(26.0))
                .flex()
                .items_center()
                .px(px(ED_PAD))
                .bg(rgba(0xb0342c1f))
                .text_color(rgb(0xb0342c))
                .text_size(px(13.0))
                .child(SharedString::from(n.clone()));
        }
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
                    .px(px(ED_PAD))
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
    fn render_right(&self, vw: f32, row_h: f32, cx: &mut Context<Self>) -> gpui::Div {
        let editor = self.render_editor(vw, row_h, cx);
        // 右栏宽 = 整屏 − 左栏宽（原文对照 50% / 纯文本 0 / 正常 40%）。必须与
        // `left_col_w` 同源，否则右栏溢出或缩水都会让编辑器原点失准。
        let right_w = vw - self.left_col_w(vw);
        // 标点栏改成**顶部悬停浮层**（v1.9.68）：它不再占布局，只在鼠标靠近
        // 右栏顶部时滑出。于是正文第一行直接从状态条下面开始——正是用户要的
        // 「把满屏空间全留给文字」。
        let bar_top = self.head_band_h();
        // 原先这里还有一条**自己的**小工具条（导入 MD / 竖排预览）。v1.9.22
        // 把它并进全局那条——两栏各顶一条色带，看起来像两套软件拼在一起。
        // 现在整屏只有一条工具条，右栏从第一行字就是稿纸。
        div()
            .relative()
            .w(px(right_w))
            // ⚠️ v1.9.65：不再用 `h_full()`（百分比高）——它解析到的父级
            // （分栏行）高度如今是**写死的绝对像素**，但为了整条高度链
            // 「零百分比」，这里也直接绑绝对像素：`row_h` 由 render() 从
            // 窗口真实高度算出（vh − 顶栏），一处算、处处用。
            .h(px(row_h))
            .flex()
            .flex_col()
            .overflow_hidden()
            // 稿纸：最亮的一张纸。
            .bg(rgb(PAPER_R))
            // **柔影向左**（v1.9.24）：右纸压左纸，靠这片外阴影。偏移取负 x，
            // 影子就落到左侧那张纸上，而不是落到自己身上（那样只会脏了稿纸边）。
            // 18px 模糊 + 10% 黑，刚够看出「这里叠了一层」，绝不至于变成一条暗带。
            .shadow(vec![BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.10),
                offset: gpui::point(px(-4.0), px(0.0)),
                blur_radius: px(18.0),
                spread_radius: px(-2.0),
            }])
            // **悬停唤出标点栏**（v1.9.68）：鼠标进入右栏顶部那一条（状态条 +
            // 标点栏高度）就滑出，离开即隐。只在**状态真的变**时 notify——
            // 否则每一次 mousemove 都触发整屏重绘，那就把刚做的窗口化白干了。
            .on_mouse_move(cx.listener(|c, ev: &MouseMoveEvent, _window, cx| {
                let y = px_f32(ev.position.y);
                let show = y <= HEADER_H + c.head_band_h() + PUNCT_BAR_H;
                if show != c.punct_bar_shown {
                    c.punct_bar_shown = show;
                    if show {
                        c.punct_bar_gen = c.punct_bar_gen.wrapping_add(1);
                    }
                    cx.notify();
                }
            }))
            .child(match self.render_text_status() {
                Some(s) => s,
                None => div().h(px(0.0)),
            })
            .child(self.render_link_status())
            .child(editor)
            // 快捷标点栏（浮层）：压在最上，滑出时盖住正文顶部几行；
            // 收起时**一个像素都不占**。
            .child(if self.punct_bar_shown {
                let gen = self.punct_bar_gen;
                self.render_punct_bar(cx)
                    .absolute()
                    .top(px(bar_top))
                    .left(px(0.0))
                    .right(px(0.0))
                    .shadow(vec![BoxShadow {
                        color: gpui::hsla(0.0, 0.0, 0.0, 0.14),
                        offset: gpui::point(px(0.0), px(3.0)),
                        blur_radius: px(10.0),
                        spread_radius: px(-2.0),
                    }])
                    .with_animation(
                        gpui::ElementId::NamedInteger("punct-bar-slide".into(), gen as u64),
                        gpui::Animation::new(Duration::from_millis(140)),
                        move |el, t| el.top(px(bar_top + (t - 1.0) * PUNCT_BAR_H)),
                    )
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            // 识别队列浮层：有活才浮出来，压在标点栏上方，不占任何布局。
            .child(if self.ocr_panel && !self.ocr_queue.is_empty() {
                self.render_ocr_panel(cx)
            } else {
                div()
            })
    }

    /// **批量导入**（v1.9.25）：一次选多个 PDF / 图片，全部排进识别队列。
    ///
    /// 与 `open_image_picker` 的分工：那个是「打开一份来校」，选完立刻栅格化、
    /// 立刻可点可联动；这个是「先把活接下来」——本版没有 OCR 引擎，选中即入队，
    /// 由 `OcrQueue` 记住待识别件数。接 API 后只需在后台起一个消费循环
    /// （`take_next` → 外呼 → `mark_done/mark_failed`），这个入口一个字不用改。
    fn open_batch_picker(&mut self, cx: &mut Context<Self>) {
        cx.spawn(
            |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    // rfd 同步对话框必须在主线程前台直调（见 IME / reader 的笔记）。
                    let picked = rfd::FileDialog::new()
                        .set_title("批量导入原图（PDF / 图片，可多选）")
                        .add_filter(
                            "PDF / 图片",
                            &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp", "heic"],
                        )
                        .add_filter("全部文件", &["*"])
                        .pick_files();
                    let Some(paths) = picked else { return };

                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            let (ok, bad) = this.ocr_queue.push_many(paths);
                            this.ocr_panel = true;
                            this.ocr_note = Some(if ok == 0 {
                                "没接到可识别的文件（只收 PDF / PNG / JPG / TIFF）".to_string()
                            } else if bad == 0 {
                                format!("已入队 {ok} 件 · 待识别（本版未内置 OCR）")
                            } else {
                                format!("已入队 {ok} 件 · {bad} 件不支持或重复，已跳过")
                            });
                            cx.notify();
                        });
                    }
                }
            },
        )
        .detach();
    }

    /// **识别队列浮层**（v1.9.25）：右下角一张小纸，列出队首几件与状态。
    ///
    /// 三条克制：① 最多显示 4 行，多的折成一行「另有 N 件」（校对时不该被
    /// 一长串文件名压住正文）；② 状态用**文字**不用色块，朱砂只留给「失败」
    /// 一档；③ 底部诚实写明「未内置 OCR」——用户点了批量导入，必须知道现在
    /// 只是排队，不是已经在识别。
    fn render_ocr_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        let jobs = self.ocr_queue.jobs();
        const SHOW: usize = 4;
        let shown = jobs.len().min(SHOW);
        let mut card = div()
            .absolute()
            .right(px(ED_PAD))
            .bottom(px(56.0))
            .w(px(300.0))
            .flex()
            .flex_col()
            .p(px(14.0))
            .rounded(px(12.0))
            .bg(rgba(0xFBF6E6F5))
            .border_1()
            .border_color(rgba(0x2B262014))
            .shadow_md()
            // 抬头：标题 + 计数 + 一枚关闭。
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_family(song.clone())
                            .text_size(px(12.5))
                            .text_color(rgb(0x4B453A))
                            .child(SharedString::from(format!(
                                "识别队列 · {} 件",
                                jobs.len()
                            ))),
                    )
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgba(0x2B26208C))
                            .hover(|s| s.text_color(rgb(0x8c2b22)))
                            .child(SharedString::from("收起"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|c, _, _, cx| {
                                    c.ocr_panel = false;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(div().h(px(8.0)));
        for j in jobs.iter().take(shown) {
            let (fg, tag) = match j.state {
                jiandu_core::ocr::OcrState::Failed => (rgb(0xb0342c), "失败"),
                jiandu_core::ocr::OcrState::Done => (rgba(0x4A4335AA), "完成"),
                _ => (rgb(0x4A4335), j.state.label()),
            };
            card = card.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .mb(px(5.0))
                    // 类型小签：PDF / 图
                    .child(
                        div()
                            .w(px(26.0))
                            .mr(px(6.0))
                            .text_size(px(10.5))
                            .text_color(rgba(0x8C2B22AA))
                            .child(SharedString::from(j.kind.label())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_size(px(11.5))
                            .text_color(rgb(0x4A4335))
                            .child(SharedString::from(j.name.clone())),
                    )
                    .child(
                        div()
                            .ml(px(6.0))
                            .text_size(px(10.5))
                            .text_color(fg)
                            .child(SharedString::from(tag)),
                    ),
            );
        }
        if jobs.len() > shown {
            card = card.child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgba(0x7E7666C0))
                    .mb(px(4.0))
                    .child(SharedString::from(format!(
                        "另有 {} 件在队列中",
                        jobs.len() - shown
                    ))),
            );
        }
        card.child(div().h(px(6.0)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(10.5))
                            .text_color(rgba(0x7E7666C0))
                            .child(SharedString::from(
                                self.ocr_note
                                    .clone()
                                    .unwrap_or_else(|| "本版未内置 OCR · 队列已暂存".to_string()),
                            )),
                    )
                    .child(
                        div()
                            .ml(px(8.0))
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .text_color(rgba(0x2B26208C))
                            .hover(|s| s.text_color(rgb(0x8c2b22)))
                            .child(SharedString::from("清空"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|c, _, _, cx| {
                                    c.ocr_queue.clear();
                                    c.ocr_panel = false;
                                    c.ocr_note = None;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
    }

    /// 渲染单行（**一个文本节点 + 两层绝对定位覆盖物**）。
    ///
    /// v1.9.24 重写。上一版把一行**按光标切成 before / caret / after 三段**，各段都是
    /// 一个 `flex_1()` 的 flex item —— 这在 GPUI 里必然出事：`flex: 1` 让首段撑满整行
    /// 剩余宽度，于是「光标之后那半句」被推到行尾，一行字在一屏里被撕成两处；一旦
    /// 有选区，更是变成三个深浅不一的色块并排。真机反馈的「前几行横排、中间孤立方块、
    /// 下面多栏」正是这个：**行内被拆成多个 flex item**，读起来就像多栏。
    ///
    /// 现在的做法把三者彻底分开，谁也不参与谁的布局：
    ///  ① **文本**：整行一个 `SharedString` 节点，`w_full + whitespace_normal()`，
    ///     单列、自上而下、超宽自动软换行 —— 这是唯一的「流」；
    ///  ② **选区底色**：绝对定位的一条带子，画在字**下面**（先 add）；
    ///  ③ **光标**：绝对定位的一根 2px 竖线，画在字**上面**（最后 add）。
    ///
    /// 覆盖物只吃「列号 × 字宽」这一个估算量，**绝不向文本流插入任何元素**，所以
    /// 无论有没有光标、有没有选区，文字的排版结果都完全一样。
    fn render_line(
        &self,
        line: &str,
        line_start: usize,
        li: usize,
        vw: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let line_len = line.chars().count();
        let line_end = line_start + line_len;

        let cpl = self.chars_per_line(vw);
        // 实测字宽（v1.9.68）：色带宽度 / 光标列位都必须用它，和 `caret_at`
        // 用同一个数——两处一旦分家，色块就会比字长一截或短一截。
        let adv = self.char_adv();
        // 选区在本行内的列区间 `[a, b)`（列号从行首算）。
        let (sel_a, sel_b) = match self.selection.as_ref() {
            Some(r) if r.start != r.end => {
                let a = r.start.max(line_start).min(line_end) - line_start;
                let b = r.end.max(line_start).min(line_end) - line_start;
                (a.min(b), a.max(b))
            }
            _ => (0, 0),
        };
        // 光标是否落在本行（含行尾）。
        let caret_col = if self.cursor >= line_start && self.cursor <= line_end {
            Some(self.cursor - line_start)
        } else {
            None
        };
        // **字符行盒**（v1.9.67）：选区色带与光标只占「字身」这一段高度，在行盒里
        // 垂直居中。以前色带和光标都取整行 `LINE_H`（= 1.8 倍字高），一行紧贴一行时
        // 看着就成了「把行距和段落留白一起涂成色块」——用户点名要 Inline 就是这个。
        let box_h = (FONT_SIZE * 1.42).min(LINE_H);
        let box_top = (LINE_H - box_h) * 0.5;

        let mut row = div()
            .relative()
            .min_h(px(LINE_H))
            .w_full()
            .min_w(px(0.0))
            .cursor_text()
            // 行身点击直达联动（v1.9.23）：行号在渲染时就绑好，
            // 不再靠「鼠标 y ÷ 行高」反推——那正是「点哪高亮哪」错行的死因。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, event: &MouseDownEvent, window, cx| {
                    let vw = px_f32(window.viewport_size().width);
                    let vh = px_f32(window.viewport_size().height);
                    c.click_line(
                        li,
                        line_start,
                        line_len,
                        px_f32(event.position.x),
                        px_f32(event.position.y),
                        vw,
                        vh,
                        event.modifiers.shift,
                        event.click_count,
                        window,
                        cx,
                    );
                }),
            );

        // ② 选区底色：绝对定位，**在文本之前** add（GPUI 按子元素顺序作画，
        //    后加的盖在上面），因此永远压在字下面，不遮墨色。
        //
        // ⚠️ **逐视觉行**画（v1.9.67）：一行只要折了行，整行一块就会盖住折行后的
        // 空白、压到下一段行距上。`visual_spans` 按每行字数切开，每块只覆盖真正
        // 有字的那一段——这才是「贴在字上」的选区，而不是一块段落色卡。
        if sel_b > sel_a {
            for (k, a, b) in visual_spans(sel_a, sel_b, cpl) {
                row = row.child(
                    div()
                        .absolute()
                        .top(px(box_top + k as f32 * LINE_H))
                        .left(px((a - k * cpl) as f32 * adv))
                        .w(px((b - a) as f32 * adv))
                        .h(px(box_h))
                        .bg(rgba(0x8c2b2233)),
                );
            }
        }

        // ②′ 查找命中（v1.9.68，⌘F）：整段泛黄，与朱砂选区区分开——
        //    查找要「一眼看到在哪」，不能和用户亲手拖出来的选区同色，
        //    否则按 ⌘G 跳下一个时根本分不清哪一处是刚跳到的。
        if self.find_open {
            if let Some(r) = self.find_sel.as_ref() {
                let a = r.start.max(line_start).min(line_end) - line_start;
                let b = r.end.max(line_start).min(line_end) - line_start;
                if b > a {
                    for (k, sa, sb) in visual_spans(a, b, cpl) {
                        row = row.child(
                            div()
                                .absolute()
                                .top(px(box_top + k as f32 * LINE_H))
                                .left(px((sa - k * cpl) as f32 * adv))
                                .w(px((sb - sa) as f32 * adv))
                                .h(px(box_h))
                                .bg(rgba(0xD9A44155)),
                        );
                    }
                }
            }
        }

        // ① 文本：整行一个节点。单列、软换行，不受任何覆盖物影响。
        row = row.child(
            div()
                .w_full()
                .min_w(px(0.0))
                .whitespace_normal()
                .child(SharedString::from(line.to_string())),
        );

        // ③ 光标：最后一根竖线，绝对定位压在字上（同样只占字身高）。
        if let Some(col) = caret_col {
            // 行尾恰好落在折行边界时（col 是 cpl 的整数倍且等于行长），光标属于
            // **上一视觉行的右缘**，不能掉到下一段去。
            let (k, vcol) = if col > 0 && col % cpl == 0 && col == line_len {
                (col / cpl - 1, cpl)
            } else {
                (col / cpl, col % cpl)
            };
            row = row.child(
                div()
                    .absolute()
                    .top(px(box_top + k as f32 * LINE_H))
                    .left(px(vcol as f32 * adv))
                    .w(px(2.0))
                    .h(px(box_h))
                    .bg(rgb(0x8c2b22)),
            );
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
    fn render_editor(&self, vw: f32, row_h: f32, cx: &mut Context<Self>) -> gpui::Div {
        // 稿纸的三件套（v1.9.22）：**宋体**、**#333 的墨**、**1.7 倍行距**。
        // 纯黑（0x000/0x2b2620）在亮纸上是刺的，深灰（#333）才像墨晕开的样子。
        let song = crate::fonts::family_for(
            crate::types::FontChoice::WenJinMincho,
            crate::types::Script::Ancient,
        );
        // 右侧是**纯文本流式布局**：一列、自上而下、超宽自动软换行。
        //
        // 三条硬约束（用户 2026-11-01 明确要求：这里不许出现任何分栏 / 网格 /
        // 竖排计算）：
        //  ① `content` 撑满宽度（`w_full`）：不撑满时子元素的 `w_full` 会退化成
        //     「按最长一行撑宽」，于是长行不折行、短行各自成块——真机上看起来
        //     就像「孤立方块 + 忽宽忽窄」；
        //  ② 每一行 `w_full + min_w(0)`：既占满整行宽，又允许被压缩到比内容更窄，
        //     从而把超长中文交给 `whitespace_normal()` 折行；
        //  ③ 文本段 `flex_1 + min_w(0)`：flex item 默认 `min-width:auto` 会拒绝
        //     收缩，是「中文长行不肯折、直接冲出右边」的经典死因。
        let mut content = div().w_full().flex().flex_col();
        // ── **虚拟列表**（v1.9.69，百万字的主命门）──────────────────────────
        //
        // 三条铁律（改这里之前先读完）：
        //  ① **只建可见行**：一帧最多建「视口高 ÷ 行高 + 4 屏余量」个行元素。
        //     百万字 ≈ 几万行，全建的话光 hitbox 就是几万个，点一下鼠标要重排
        //     整套树——那不是慢，是死。
        //  ② **总高必须一分不差**：视口外的行用等高空白（skip / tail）撑住，
        //     于是 `scroll_max`、滚动条长度、`caret_at` 的全局坐标全都不变。
        //     少一像素，滚动条就会「跳」。
        //  ③ **几何只问行索引**：`self.lines` 里的前缀和是唯一真相。绝不在渲染里
        //     再 `split('\n')` 一遍——那是 O(全文) × 每帧。
        //
        // 少于此数就直接全画：稿子只有一两屏时，窗口化省不到什么，反而多一层风险。
        const VIRTUALIZE_ABOVE: usize = 160;
        let n_lines = self.lines.lines();
        if self.index_valid() && n_lines > 0 {
            // ① 定位可见窗口（二分）。
            let (li0, li1) = if n_lines <= VIRTUALIZE_ABOVE {
                (0, n_lines - 1)
            } else {
                self.visible_line_range(row_h)
            };
            // ② 顶部 / 底部占位（`aligned` 在非对照模式下等于 `vis`）。
            let top_lines = if self.show_diff {
                self.lines.aligned[li0]
            } else {
                self.lines.vis[li0]
            };
            let total_lines = if self.show_diff {
                self.lines.total_aligned()
            } else {
                self.lines.total_visual()
            };
            let bot_lines = total_lines.saturating_sub(if li1 + 1 < self.lines.aligned.len() {
                if self.show_diff {
                    self.lines.aligned[li1 + 1]
                } else {
                    self.lines.vis[li1 + 1]
                }
            } else {
                total_lines
            });
            if top_lines > 0 {
                content = content.child(div().w_full().h(px(top_lines as f32 * LINE_H)));
            }
            // ③ 只画窗口内的行。
            let raw_owned;
            let raw: &str = if self.show_diff {
                raw_owned = self.raw_text.as_deref().unwrap_or("");
                raw_owned
            } else {
                ""
            };
            for li in li0..=li1 {
                let seg = self.lines.line_str(&self.text, li);
                let cs = self.lines.char_start[li];
                if self.show_diff {
                    // v1.9.63：段落对齐 + 差异高亮。段内仍走 `render_line`（光标 /
                    // 选区 / 打字全部保留），只是外面包一层带底色的块 + 尾部填充。
                    let raw_seg = self.lines.raw_line(raw, li);
                    let seg_lines = self.lines.vis[li + 1] - self.lines.vis[li];
                    let max_lines = self.lines.aligned[li + 1] - self.lines.aligned[li];
                    let band = if raw_seg != seg {
                        rgba(0x8c2b221A) // 本段被清洗引擎改过 → 极弱朱砂底
                    } else if li % 2 == 1 {
                        rgba(0x2B26200A)
                    } else {
                        rgba(0x2B262004)
                    };
                    let pad = max_lines.saturating_sub(seg_lines) as f32 * LINE_H;
                    content = content.child(
                        div()
                            .w_full()
                            .bg(band)
                            .child(self.render_line(seg, cs, li, vw, cx))
                            .child(div().h(px(pad))),
                    );
                } else {
                    content = content.child(self.render_line(seg, cs, li, vw, cx));
                }
            }
            if bot_lines > 0 {
                content = content.child(div().w_full().h(px(bot_lines as f32 * LINE_H)));
            }
        }
        // 用 `InputField` 包裹，使其在获得焦点时把 IME 回传中文接住。
        // ⚠️ **这一层是「内容层」，不给高度**——它的高度由正文行数决定，正是
        //   滚动视口要「滚动」的那个内容高度。任何 `h_full` 都不能挂在这里。
        let editor = crate::ime::InputField::new(
            div()
                .w_full()
                // **GPUI 原生「点一下就聚焦」**（v1.22.1）。
                //
                // 真机反馈：导入 MD 之后点进右侧编辑器，连光标都不给，更别说打字。
                // 手写的 `hard_refocus`（blur + focus）在「窗口还不是 key window」
                // 时只会改 GPUI 的内部账本，平台侧 first responder 根本没交回来。
                // `track_focus` 走的是 GPUI 自己的鼠标派发：命中即 `window.focus()`，
                // 与平台侧「点谁谁成为 first responder」同一时刻发生，不依赖任何
                // 手写补偿。两条路并存，任一条通就能打字。
                .track_focus(&self.focus)
                .bg(rgb(PAPER_R))
                // 内边距 14 → 28 → **40**：起首空两格、四边留白宽过行距，字才是
                // 「摆」在纸上，而不是被框住。40 同时是 `editor_origin()` 的
                // 原点偏移——同一个常量，改一处即两处同变。
                .px(px(ED_PAD))
                .py(px(ED_PAD))
                .text_size(px(FONT_SIZE))
                // #333 而不是纯黑：亮纸上纯黑会「刺」，深灰才像墨晕开的样子。
                .text_color(rgb(0x333333))
                .font_family(song.clone())
                .line_height(px(LINE_H))
                // 文本光标（I 形）——「这里能打字」的第一眼线索。
                .cursor_text()
                // **滚轮强接**（v1.9.65 铁律 ③，v1.9.67 并回本层）：
                // 原先它在 `InputField` 外面套了一层**只有滚轮监听**的透明 div。
                // 那层会额外插一个 hitbox、并在焦点路径上多插一级——真机表现就是
                // 点不进、打不出（只读）。现在滚轮 / 焦点 / 鼠标 / 键盘**挂在同一个
                // 元素**上：一个 hitbox、一条焦点路径，零竞争。
                .on_scroll_wheel(cx.listener(|c, ev: &ScrollWheelEvent, _window, cx| {
                    c.forward_wheel(ev, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|c, event, window, cx| c.place_caret_from_click(event, window, cx)),
                )
                .child(content),
            self.focus.clone(),
            cx.entity(),
        );
        // **滚动视口**（v1.9.65 终极修法，用户「绝对强制」铁律 ② + ③）：
        //
        // 旧链的真根：`scroll_view.h_full()` 是**百分比高度**，挂在高度由
        // `flex_1` 解析出来的 `wrap` 上——taffy 对「百分比 vs flex 解析高」的
        // 父级可能判为**不定** → h_full 退化成内容高 → 视口 == 内容 → gpui
        // 的 `clamp_scroll_position`（div.rs:1747）算出 `scroll_max = 0` →
        // 滚轮事件来了 offset 也被钉死在 0（div.rs:1758）——**滚轮没坏、
        // 事件也没被吞，是视口高度塌了**。这就是真机「彻底滑不动」的真根，
        // 也是前两轮「min_h(0) 修复」只治了 wrap 一层、没治 scroll_view 这层
        // 百分比的原因。修法：
        //   ① render()/render_right() 把外层行与右栏全改**绝对像素高**；
        //   ② `wrap` 改 flex 列、`scroll_view` 用 **flex_1 + min_h(0)**——
        //      flex 主轴分配永远给出确定高，整条链零百分比；
        //   ③ 滚轮接管层**并进编辑器那一层**（v1.9.67）：v1.9.65 曾在
        //      `InputField` 外单独套一个只挂 `on_scroll_wheel` 的 div，结果多出
        //      一个 hitbox / 一级焦点路径，真机上编辑器直接变**只读**（点不进、
        //      打不出）。滚轮监听现已挂回编辑器自身，见上。
        let scroll_view = div()
            .id(SharedString::from(if self.pure_text_mode {
                // 用户点名的专用 id（铁律 ②）：纯文本模式的独立滚动容器。
                "pure-text-scroll-container"
            } else {
                "collation-editor-scroll"
            }))
            // ⚠️ 不再用 h_full()（百分比高度）——见上，flex 主轴分配才是确定高。
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_y_scroll()
            // v1.9.63：对照模式挂专用 `diff_scroll`（与左栏毛坯共用 → 同步滚动）；
            // 常态 / 纯文本模式仍走 `scroll`（已验证可编辑），两柄分离互不干扰。
            .track_scroll(if self.show_diff {
                &self.diff_scroll
            } else {
                &self.scroll
            })
            // **兜底点击**（v1.9.67）：正文比视口短时，最后一行之下是一大片空白，
            // 那里不属于编辑器内容盒（它的高度 = 内容高），点了没反应最像「坏了」。
            // 视口这一层接住它：落光标 + 聚焦（落点由 `caret_at` 夹到文末）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|c, event, window, cx| c.place_caret_from_click(event, window, cx)),
            )
            // **键盘挂在这一层**（v1.9.67）：`track_focus` 的句柄由 `InputField`
            // 与内层 div 两处登记，焦点路径最终落在哪一级取决于登记顺序；挂在
            // **共同祖先**（视口）上，则无论哪一级拿到焦点，按键都必达编辑逻辑。
            // ⚠️ 只能挂这一处：内层若也挂，退格一次会删两个字。
            .on_key_down(cx.listener(|c, event, _window, cx| c.handle_key(event, cx)))
            .child(editor);
        // `min_h(px(0.0))`：wrap 是右栏 flex 列里的 `flex_1` 子项，不带它就
        // 会被正文撑到「内容高度」。**v1.9.65 起右栏本身已是绝对像素高**，
        // flex 分配必出确定值；wrap 同时改 flex 列，让 scroll_view 的
        // `flex_1` 有主轴可分——从 wrap 到视口，全程零百分比高度。
        //
        // **拖选监听挂在面板层**（v1.9.67）：拖到正文内容盒之外（短稿的下方空白、
        // 或横向拖出一点）仍要继续扩选。挂在内容盒上会因为「鼠标离开了该 hitbox」
        // 而中断；挂在铺满面板的这一层，手感才连续。
        //
        // ⚠️ **选区悬浮钮不在这里**（v1.9.69）：它已提到根节点（见 `render`）。
        //    留在这一层会被本层的 `overflow_hidden` 裁掉——v1.9.68 起「问小笺
        //    气泡整颗消失」的直接原因。气泡的显示条件只看 `self.selection`，
        //    与它挂在哪一层无关，故搬走不影响触发。
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .w_full()
            .on_mouse_move(
                cx.listener(|c, event, window, cx| c.extend_selection_from_drag(event, window, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|c, _, _, cx| {
                    // **拖选收尾**（v1.9.69）：清拖拽态 → 落盘一次 → 重绘。
                    // 重绘是必须的：气泡的显示条件虽然是 `selection`，但拖拽过程中
                    // 的最后一帧可能还没画到（mousemove 被 3px 死区吃掉），
                    // 抬起时补一次 notify，保证「松手即见气泡」。
                    c.dragging = false;
                    c.drag_origin = None;
                    c.touch(cx);
                    cx.notify();
                }),
            )
            .child(scroll_view)
    }

    /// 竖排预览浮层：把转录文本离屏栅格化成「笺读风」竖排书页，`gpui::img` 贴图。
    ///
    /// 全屏遮罩 + 顶部工具条（翻页器 / 刷新 / 样式 / 关闭）+ 居中整页图（随窗口
    /// 高度自适应，比例保持竖排书页）+ 样式面板（底色 / 版框 / 列数，点按即重栅格
    /// 化）+ 底部状态条（仅栅格化错误，如缺中文字体）。长文自动按页切块，用顶部
    /// 「← 第 X / Y 页 →」翻页（见 [`regenerate_preview`]）。
    fn render_preview_overlay(&self, vh: f32, cx: &mut Context<Self>) -> gpui::Div {
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
                        // 标题独占「按钮以外的全部宽度」，且**允许裁切**：工具栏
                        // 现在有纸张 / 拼版 / 装订三组胶囊 + 四颗钮，窄窗口下
                        // 若让标题按内容撑开，右边那排按钮会被挤出可视区。
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_color(rgb(0xf4ecd8))
                            .text_size(px(15.0))
                            .child(SharedString::from(self.preview_title())),
                    )
                    // Loading：后台排版期间给出明确反馈（用户点了不会「没反应」）。
                    .child(if self.preview_busy {
                        div()
                            .px(px(8.0))
                            .text_color(rgb(0xd8c9a8))
                            .text_size(px(13.0))
                            .child(SharedString::from("排版中…"))
                    } else {
                        div()
                    })
                    .child(self.preview_pager(cx))
                    .child(self.paper_selector(cx))
                    .child(self.imposition_selector(cx))
                    .child(self.binding_selector(cx))
                    // **掩注**（v1.9.54，⌘N）：隐藏括注 / 夹注小字，只看正文。
                    // 放在导出旁边，是因为它的最终目的就是「印一版干净的正文」。
                    .child(self.button(
                        if self.hide_gloss {
                            "掩注：开"
                        } else {
                            "掩注：关"
                        },
                        |c, _, _, cx| {
                            c.toggle_gloss(cx);
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(self.button(
                        "导出 PDF",
                        |c, _, _, cx| c.export_pdf(cx),
                        cx,
                    ))
                    .child(self.button(
                        "刷新预览",
                        |c, _, _, cx| {
                            // 强制重排：清掉版式指纹，绕过「同版式直接跳过」那道短路。
                            // （用户点刷新就是「我怀疑这图不对，再排一遍」，绝不能说
                            //   什么都没发生——那是他明确讨厌过的「点了没反应」。）
                            c.refresh_preview(cx);
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
                            // 彻底关闭：卸载预览图 + 作废在途后台任务（见 close_preview）。
                            c.close_preview();
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
                            Some(p) => {
                                // 显式宽高等比缩放：预览位图恒为 1080×1920（9:16），
                                // 不显式给尺寸时 img 会按原始像素铺开 → 被容器裁切
                                // （表现为「文字被切掉 / 版面错乱」）；而 h_full()
                                // 又会让「容器高↔内容高」循环依赖 → GPUI 反复求解
                                // 尺寸直至卡死。故按视口算一个确定的等比尺寸。
                                let img_h = (vh - HEADER_H * 2.0 - 64.0)
                                    .clamp(240.0, PREVIEW_IMG_MAX_H);
                                // 宽高比来自当前纸张：`Card` 仍是 0.5625（9:16，
                                // 与 1080×1920 的老取景逐像素一致），A4/A3 是 1:√2。
                                // 拼版时预览图是**整张纸**（横的），故宽高比也取整张纸；
                                // 单页拼版取单页。两者都只是同一条版式数据的贴图。
                                let img_w = img_h
                                    * self
                                        .preview_paper
                                        .preview_sheet_geom(self.eff_imposition())
                                        .aspect();
                                // ⚠️ 不要给 img 套 with_animation：动画期间每帧重绘，
                                // 1080×1920 级的位图反复解码 / 重上传会把主线程拖垮。
                                // 翻页已由「← 第 X / Y 頁 →」明确指示，不需要淡入。
                                div()
                                    .child(img(p.clone()).w(px(img_w)).h(px(img_h)))
                                    .into_any_element()
                            }
                            None => {
                                div()
                                .px(px(20.0))
                                .py(px(16.0))
                                .bg(rgb(0xf4ecd8))
                                .rounded(px(8.0))
                                .text_color(rgb(0x2b2620))
                                .text_size(px(14.0))
                                .child(SharedString::from(if self.preview_busy {
                                    // 排版中（后台还没回）：绝不能说「转录为空」——
                                    // 用户会以为稿子白写了，其实是还在排。
                                    "排版中…（请稍候，Esc 可随时关闭）"
                                } else if !self.text.trim().is_empty() {
                                    // 有字、没图、也没在排：只可能是还没排过（刚打开
                                    // 浮层 / 刚清过临时图）。绝不说「正文为空」——
                                    // 真机上正文明明有 4928 字，写「为空」等于冤枉用户。
                                    "尚未生成预览，点「刷新预览」可重新排版"
                                } else {
                                    "（正文为空：写入文字或【导入 MD】后，点「刷新预览」）"
                                }))
                                .into_any_element()
                            }
                        }
                    }),
            )
            .child(self.render_style_panel(cx))
            .child(match self.render_preview_status() {
                Some(s) => s,
                None => div().h(px(0.0)),
            })
            .child(match self.render_export_status() {
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
                    c.request_preview(cx);
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
                    c.request_preview(cx);
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
                    c.request_preview(cx);
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
    ///
    /// 三个按钮都走 [`CollationWorkspace::flip_page`]——点箭头与按方向键必须是
    /// **同一条路**，否则左右双屏迟早会不同步。
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
                |c, _, _, cx| c.flip_page(-1, cx),
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
                |c, _, _, cx| c.flip_page(1, cx),
                cx,
            ))
    }

    /// 左图页码条（浮在左栏底部）：显示 PDF 当前页 / 总页，可点击翻页。
    ///
    /// 键盘的 ←/→ 只在预览浮层打开时才归翻页（关着的时候要留给编辑器移光标），
    /// 而校对时用户常常是「关掉预览、只看原图逐页核」——所以左侧必须有自己的
    /// 翻页入口，不能只靠键盘。
    fn render_pdf_pager(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.pdf_pages <= 1 {
            return div();
        }
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(rgba(0xf4ecd8e0))
            .child(self.button("‹", |c, _, _, cx| c.flip_page(-1, cx), cx))
            .child(
                div()
                    .text_color(rgb(0x8c2b22))
                    .text_size(px(13.0))
                    .child(SharedString::from(if self.page_busy {
                        format!("第 {} / {} 頁 · 载入中…", self.pdf_page, self.pdf_pages)
                    } else {
                        format!("第 {} / {} 頁", self.pdf_page, self.pdf_pages)
                    })),
            )
            .child(self.button("›", |c, _, _, cx| c.flip_page(1, cx), cx))
    }

    /// 底部状态条：仅显示预览栅格化错误（红）。多页提示由顶部翻页器承载。
    /// 纸张选择胶囊（浮层顶部）：`預覽卡 / A4 / A3`。
    ///
    /// 换纸 = 换版式的物理尺寸，**不是**换个滤镜：预览的分页容量、字号、天头
    /// 地脚、以及「导出 PDF」出来的纸幅，全由这一个选择决定（版式层只认 `w/h`）。
    fn paper_selector(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut row = div().flex().flex_row().items_center().mr(px(6.0));
        for paper in Paper::ALL {
            let selected = self.preview_paper == paper;
            row = row.child(
                div()
                    .px(px(9.0))
                    .py(px(3.0))
                    .mr(px(4.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .bg(if selected {
                        rgb(0xd8c9a8)
                    } else {
                        rgba(0xffffff1a)
                    })
                    .text_color(if selected {
                        rgb(0x2b2620)
                    } else {
                        rgb(0xd8c9a8)
                    })
                    .child(SharedString::from(paper.label()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |c, _, _, cx| {
                            if c.preview_paper != paper {
                                c.preview_paper = paper;
                                // 預覽卡不是纸：拼版对它无意义，切回去就复位成单页，
                                // 免得留下「一张 9:16 卡上并排两半」的怪相。
                                if paper == Paper::Card {
                                    c.imposition = Imposition::Single;
                                }
                                // 指纹里含纸张 → 换纸必然重排；这里走 request 而非
                                // refresh，连点时仍受 1s 节流保护。
                                c.request_preview(cx);
                            }
                            cx.notify();
                        }),
                    ),
            );
        }
        row
    }

    /// 工具栏胶囊（纸张 / 拼版 / 装订三处共用同一套视觉）。
    ///
    /// 只负责「长什么样」，点按行为由调用方挂 `on_mouse_down`——三处的回调
    /// 语义不同（换纸要重算容量、换拼版要重排），视觉却必须一致。
    fn chip(&self, label: impl Into<SharedString>, selected: bool) -> gpui::Div {
        div()
            .px(px(9.0))
            .py(px(3.0))
            .mr(px(4.0))
            .rounded(px(5.0))
            .cursor_pointer()
            .text_size(px(12.5))
            .bg(if selected {
                rgb(0xd8c9a8)
            } else {
                rgba(0xffffff1a)
            })
            .text_color(if selected {
                rgb(0x2b2620)
            } else {
                rgb(0xd8c9a8)
            })
            .child(label.into())
    }

    /// **拼版选择胶囊**：`單頁 / 雙頁拼版`（v1.22.0）。
    ///
    /// 这是「大字海报」与「书」的那个开关：`單頁` 一版一张纸；`雙頁拼版` 把
    /// 两个 A4 大小的单页并到一张横放的大纸上（书籍折页）。`預覽卡` 不是真实
    /// 纸张，没有拼版可言，故那时整组不显示。
    fn imposition_selector(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.preview_paper.mm().is_none() {
            return div();
        }
        let mut row = div().flex().flex_row().items_center().mr(px(6.0));
        for imp in Imposition::ALL {
            let selected = self.imposition == imp;
            row = row.child(self.chip(imp.label(), selected).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, _, _, cx| {
                    if c.imposition != imp {
                        c.imposition = imp;
                        // 指纹里含拼版 → 换拼版必然重排；走 request 而非 refresh，
                        // 连点时仍受 1s 节流保护。
                        c.request_preview(cx);
                    }
                    cx.notify();
                }),
            ));
        }
        row
    }

    /// **装订模式胶囊**：`右開本 / 左開本`（v1.22.0）。
    ///
    /// 只在双页拼版时才有意义（单页拼版没有「谁在左」这回事），故其余时候整组
    /// 不显示——工具栏上少两颗无用的钮，比多两颗更清楚。
    fn binding_selector(&self, cx: &mut Context<Self>) -> gpui::Div {
        if !self.imposition.is_spread() || self.preview_paper.mm().is_none() {
            return div();
        }
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .mr(px(6.0))
            // 把「奇偶怎么摆」直说在胶囊旁边：这是最容易搞反的一条，
            // 光靠「右開本」三个字不够，得让用户一眼看到 奇左偶右。
            .child(
                div()
                    .mr(px(5.0))
                    .text_size(px(11.5))
                    .text_color(rgb(0x8a8272))
                    .child(SharedString::from(self.binding.hint())),
            );
        for b in Binding::ALL {
            let selected = self.binding == b;
            row = row.child(self.chip(b.label(), selected).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |c, _, _, cx| {
                    if c.binding != b {
                        c.binding = b;
                        c.request_preview(cx);
                    }
                    cx.notify();
                }),
            ));
        }
        row
    }

    /// **当前真正生效的拼版**：`預覽卡` 不是纸张，一律按单页取景。
    ///
    /// 所有「读」拼版的地方都走这里，别直接读 `self.imposition`——那是个可能
    /// 与当前纸张不匹配的残留值（用户先选 A3 拼版、再切回取景卡）。
    fn eff_imposition(&self) -> Imposition {
        if self.preview_paper == Paper::Card {
            Imposition::Single
        } else {
            self.imposition
        }
    }

    /// 浮层标题：把「多大的纸 / 怎么拼 / 单页多大 / 奇偶怎么摆」一次说清。
    ///
    /// 这几个数是印厂对话的全部内容（420×297 是上机尺寸，202×297 是排版尺寸），
    /// 少一个用户就得回来问一遍。所以宁可字小一点，也要都摆出来。
    fn preview_title(&self) -> String {
        let Some(_) = self.preview_paper.mm() else {
            return "竖排预览 · 預覽卡（9:16 取景）".to_string();
        };
        let imp = self.eff_imposition();
        let sheet = self.preview_paper.sheet_mm(imp).unwrap_or((0.0, 0.0));
        let base = format!(
            "竖排预览 · {} {} {}×{}mm",
            self.preview_paper.label(),
            imp.label(),
            sheet.0.round() as i32,
            sheet.1.round() as i32
        );
        if !imp.is_spread() {
            return base;
        }
        let page = self.preview_paper.page_mm(imp).unwrap_or((0.0, 0.0));
        format!(
            "{base} · 單頁 {}×{}mm",
            page.0.round() as i32,
            page.1.round() as i32
        )
    }

    /// 导出状态条：**在途 / 成功 / 失败**三态都用一行字说清（绝不静默）。
    fn render_export_status(&self) -> Option<gpui::Div> {
        let note = self.export_note.as_ref()?;
        let busy = self.export_busy;
        Some(
            div()
                .h(px(24.0))
                .flex()
                .items_center()
                .px(px(PAD))
                .bg(rgba(0x2b262099))
                .text_color(if busy {
                    rgb(0xd8c9a8)
                } else {
                    rgb(0xf4ecd8)
                })
                .text_size(px(13.0))
                .child(SharedString::from(note.clone())),
        )
    }

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
    /// 键盘总入口（挂在根 div 上）。
    ///
    /// 派发顺序就是优先级，**不能乱**：
    ///   ① 问小笺开着 → 键先归面板（⏎ 发送 / ⇧⏎ 换行 / 输入法组字 / Esc 清框）；
    ///   ② 预览开着 → ←/→ 翻页、Esc 关闭、⌘N 掩注；
    ///   ③ 其余键一概不拦（编辑器自己的 `handle_key` 收）。
    fn ws_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // ① 面板优先：它自己会说「这一键我要不要」（false = 不要）。
        if self.ai_open && crate::ai::forward_key(&self.ai_panel, event, window, cx) {
            return;
        }
        let k = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        // ② 掩注：预览没开也要能切（切了下次开预览即生效）。
        if (mods.platform || mods.control) && (k == "n" || k == "N") {
            self.toggle_gloss(cx);
            return;
        }
        // ②′ 原文对照：⌘⇧D / Ctrl+Shift+D。
        if (mods.platform || mods.control) && mods.shift && (k == "d" || k == "D") {
            self.toggle_diff(cx);
            return;
        }
        // ③ 浮卡开着时 Esc 先收卡，再轮到预览 / 选区。
        if self.ai_open && k == "escape" {
            self.close_ai(window, cx);
            return;
        }
        self.preview_key(event, window, cx);
    }

    /// 预览浮层的键盘（仅在 `show_preview` 时生效）。
    ///
    /// ←/→ 翻页（**左右双屏一起翻**，见 [`CollationWorkspace::flip_page`]）；
    /// Esc 关闭浮层。预览关闭时直接返回，绝不干扰编辑器自身的方向键光标移动。
    fn preview_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_preview {
            return;
        }
        match event.keystroke.key.as_str() {
            "left" => self.flip_page(-1, cx),
            "right" => self.flip_page(1, cx),
            "escape" => {
                self.close_preview();
                window.focus(&self.focus);
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for CollationWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 上一帧起就不再显示的临时 PNG，此刻删盘最安全（元素树已换，无人引用）。
        // 量很小（每轮预览 ≤6 张），同步删几个文件远不到一帧的预算。
        for p in self.temp_gc.drain(..) {
            let _ = std::fs::remove_file(&p);
        }
        // 同理回收上一页的 GPU 纹理（`drop_image` 只减 atlas 引用计数，整块纹理
        // 在无人引用时才真正释放）。不回收 = 翻一页漏一张 7.5MB 的纹理。
        for m in self.image_gc.drain(..) {
            let _ = window.drop_image(m);
        }
        // 刚关过原生文件对话框（rfd）→ 强制重建输入焦点。
        // `Window::focus()` 在「已是焦点」时直接 return，故必须先 blur 一次把
        // 焦点 id 清掉，平台才会重新把 first responder / 输入法上下文交回来。
        // 不这么做，导入完的文件在右侧「看得见、打不进字」（只读假象）。
        if self.refocus {
            self.refocus = false;
            Self::hard_refocus(window, &self.focus);
        }
        let vw = px_f32(window.viewport_size().width);
        let vh = px_f32(window.viewport_size().height);
        // **首帧实测字宽**（v1.9.68）：`CHAR_W` 是 17 × 0.95 的估算，中文真实进距
        // 是 1.0em。5% 的误差按行累计，到 60 字一行就差 51px（≈3 个字）——点击
        // 时看着就是「插不进两个字之间」。这里只量一次，之后全程用真值。
        // 放在 render 里量（而不是 `new`）：那时字体注册早已完成，量到的一定是真字形。
        self.viewport_h = vh;
        self.viewport_w = vw;
        if self.char_adv <= 0.0 {
            let fam = crate::fonts::family_for(
                crate::types::FontChoice::WenJinMincho,
                crate::types::Script::Ancient,
            );
            self.char_adv = measure_char_adv(&window.text_system(), &fam);
        }
        // 左栏实际显示宽度的**设备像素**：栅格化按它出图（`raster_px`）。
        // 取不到缩放（未接显示器 / 测试环境）时按 1x，绝不会因为 0 而算出空画布。
        let backing = {
            let s = window.scale_factor();
            if s.is_finite() && s > 0.0 { s } else { 1.0 }
        };
        // 纯文本模式（一键笺校进无图的书）：没有左栏原图，栅格化目标宽度归零。
        let left_px = if self.pure_text_mode { 0.0 } else { vw * 0.4 * backing };
        // 左栏宽度变了（拉窗口 / 进全屏 / 挪到另一块屏）→ 缓存里那几页是按**旧**
        // 像素栅格化的，拉宽后照样贴上去就是糊的。这是缓存带来的新风险，必须显式
        // 兜住：超过 10% 才算「变宽」，像素级抖动不值得重栅格化。
        let resized = self.left_px > 0.0 && (left_px - self.left_px).abs() > self.left_px * 0.1;
        self.left_px = left_px;
        if resized && self.pdf_path.is_some() {
            self.pdf_cache.clear();
            // 当前这一页也按新像素重出一张（不然屏幕上就是那张旧的低清图）。
            let page = self.pdf_page.max(1);
            self.request_pdf_page(page, cx);
        }
        // 排障：帧计数。静止时也狂刷 → 说明有无限重绘循环（脏标记漏了）。
        self.frame_count += 1;
        if self.frame_count % 30 == 0 {
            eprintln!(
                "[render] frames={} preview={} busy={}",
                self.frame_count, self.show_preview, self.preview_busy
            );
        }
        // **行索引就位**（v1.9.69）：必须在任何读几何 / 建行的代码之前。
        // 它只在「文本变了 / 版心列数变了 / 对照开关变了」时真重建，其余帧是
        // 一次整数比较——百万字的稿子，这一行把每帧 O(全文) 变成 O(1)。
        self.ensure_line_index();
        // **清洗审计卡片**（v1.9.70）：面板开着且没算过，就在这一帧算一次。
        // 绝不在 `render_sanitize_panel` 里算——那等于每帧清洗一遍全文。
        if self.show_sanitize_panel && self.sanitize_card.is_none() {
            self.sanitize_card = Some(self.compute_sanitize_card());
        }
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
            .relative()
            .size_full()
            .bg(rgb(PAPER_R))
            .track_focus(&self.preview_focus)
            .on_key_down(cx.listener(|c, event, window, cx| c.ws_key(event, window, cx)))
            .child(self.render_header(cx));
        // **纯文本模式不再有常驻提示条**（v1.9.68）：以前这里有一条 20px 的
        // 「纯文本模式 · 本书无原图，仅显示正文」静默横条，既占版面又反复提醒
        // 用户「你没有图」——可这一屏本来就没有左栏，说这句话是噪声。
        // 直接删掉，把 20px 还给正文；模式信息由顶栏的书名与「无原图」图标承担。
        //
        // **绝对强制物理高度**（v1.9.65，用户铁律 ①）：分栏行不再用 `flex_1`
        // （高度靠 flex 链逐级解析，链上任何一环把百分比判成「不定」都会塌成
        // 内容高 → 视口 == 内容 → 永远滚不动）。直接绑**窗口绝对高度**：
        // vh − 顶栏 44，写死的像素，不依赖任何父级解析结果。
        let row_h = (vh - HEADER_H).max(120.0);
        root = root.child(
            // **视觉分割**（v1.9.24）：一条黑线都不要。
            // 现在是「柔影（右纸的 shadow，见 render_right）+ 一根 5% 墨的
            // 发丝线」：影子负责体积，发丝线只负责在 1x 屏上兜底——
            // 没有那道线时，两块同族浅色在低对比屏上会糊成一片。
            // 用绝对定位画线（不占布局），两栏仍是 40/60，不会因为多出
            // 这 1px 而把右栏挤出去。
            div()
                .relative()
                .flex()
                .flex_row()
                .h(px(row_h))
                .overflow_hidden()
                .when(self.left_col_w(vw) > 0.0, |d| {
                    d.child(self.render_left(vw, cx))
                        .child(
                            div()
                                .absolute()
                                .top(px(0.0))
                                .bottom(px(0.0))
                                .left(px(self.left_col_w(vw)))
                                .w(px(1.0))
                                .bg(rgba(0x2B26200D)),
                        )
                })
                .child(self.render_right(vw, row_h, cx)),
        );
        // **选区悬浮钮（问小笺）挂在这里**（v1.9.69）：
        // 根节点是全屏最后一层的坐标系，气泡不会再被右栏的 `overflow_hidden`
        // 裁掉，也不会被标点栏 / 识别队列浮层盖住。它用 `editor_origin()` 算
        // 窗口绝对坐标，所以对照模式（左栏占一半）同样落得准。
        //
        // 位置刻意排在预览 / 问小笺浮卡 / 覆盖确认**之前**：那三层是真模态，
        // 该压住它；气泡只跟编辑器争层级。
        if let Some(bar) = self.render_selection_bar(vw, vh, cx) {
            root = root.child(bar);
        }
        if self.show_preview {
            root = root.child(self.render_preview_overlay(vh, cx));
        }
        // 问小笺浮卡：压在预览之上（预览里也能接着问），点「收起」或 Esc 收。
        if self.ai_open {
            root = root.child(self.render_ai_card(vw, vh, cx));
        }
        // 替换当前书籍的「覆盖确认」浮层：压在最上，阻断误覆盖。
        if self.replace_confirm {
            root = root.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x14100A66))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|c, _, _, cx| {
                            cx.stop_propagation();
                            c.replace_confirm = false;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(14.0))
                            .p(px(24.0))
                            .bg(rgb(0xF3ECD9))
                            .border_1()
                            .border_color(rgb(0x9A6B4F))
                            .rounded(px(4.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(0x2B2622))
                                    .child(SharedString::from("替换当前书籍？")),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(0x9A9184))
                                    .child(SharedString::from(
                                        "将覆盖当前正在校对的内容（原图与已标注的联动）。",
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .px(px(16.0))
                                            .py(px(6.0))
                                            .rounded(px(6.0))
                                            .bg(rgb(0xA63A2B))
                                            .cursor_pointer()
                                            .text_size(px(12.5))
                                            .text_color(rgb(0xFFFFFF))
                                            .child(SharedString::from("确认替换"))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|c, _, _, cx| {
                                                    cx.stop_propagation();
                                                    c.replace_confirm = false;
                                                    c.replace_book_picker(cx);
                                                    cx.notify();
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .px(px(16.0))
                                            .py(px(6.0))
                                            .rounded(px(6.0))
                                            .border_1()
                                            .border_color(rgb(0x9A6B4F))
                                            .cursor_pointer()
                                            .text_size(px(12.5))
                                            .text_color(rgb(0x2B2622))
                                            .child(SharedString::from("取消"))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|c, _, _, cx| {
                                                    cx.stop_propagation();
                                                    c.replace_confirm = false;
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            ),
                    ),
            );
        }
        // 清洗日志内嵌面板：压在最上（与覆盖确认同级），点遮罩或 ✕ 收。
        if self.show_sanitize_panel {
            root = root.child(self.render_sanitize_panel(cx));
        }
        // 查找条（v1.9.68）：压在正文之上、帮助卡之下。
        if self.find_open {
            root = root.child(self.render_find_bar(cx));
        }
        // 帮助浮卡（v1.9.66）：也压在最上——它是「随时呼出、随时收起」的说明。
        if self.show_help {
            root = root.child(self.render_help_overlay(vw, vh, cx));
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
//   ③ 多页 PDF：不整本入内存，**每次只渲调用方点名的那一页**，页数>1 时给温和
//      提示，不报错。
//
// ④ 大文件走后台（产品边界，用户 2026-11-01 定）：本文件的栅格化恒在
//    `bg.spawn(...)` 里跑（见 `open_image_picker` / `flip_page`），主线程只负责
//    「换个路径给 img」。所以 500 页的扫描件不会在导入时把界面冻住——它压根没被
//    整本渲染过，只有当前页落成一张 PNG；翻页时才渲下一页，进度由状态栏页码承担
//    （「第 3 / 500 頁（← → 翻页）」）。这正是 8 MB / 8 万字那条线在**原图侧**的
//    等价做法：不设「只渲前 N 页」的硬截断，因为逐页渲染本身就等价于「只渲 1 页」。
//    竖排预览（`render_preview_pages`）才是「一次出多页」的那条路，它另有
//    `PREVIEW_MAX_PAGES` / 时间预算两道闸门，并明确提示「仅预览前 N 页」。

// ── 文本导入：rfd 选 .md / .txt → 读成纯文本源码（不清洗）──

/// 文件名（给用户看的）：取不到就用占位，绝不 `unwrap`（无文件名 = 路径以 `..` 结尾）。
fn file_display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "（未命名）".to_string())
}

/// 流式读盘 + 硬上限：最多读 `hard + 1` 字节，多读的那一字节就是「越界」的证据。
///
/// 为什么不用 `std::fs::read`：它按 `metadata().len()` 一次性预分配。网络盘 /
/// iCloud 占位文件 / 正在被别的进程写入的文件，size 可能是 0 也可能是几 GB，
/// 信它就会一次性 read 出一个 2 GB 的 Buffer——用户内存当场爆掉。`Read::take`
/// 在**读取侧**截断，内存占用恒定封顶在 `hard`，判据只看「多读到一个字节没有」。
///
/// `hard` 是参数而非常量，纯为了可测：单测拿 1 KB 当闸门去撞一个 6 KB 的文件，
/// 不必真造一个 64 MB 的样本。
fn read_bytes_capped(path: &Path, name: &str, hard: u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut bytes: Vec<u8> = Vec::new();
    let f = std::fs::File::open(path).map_err(|e| format!("读取「{}」失败：{}", name, e))?;
    f.take(hard + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读取「{}」失败：{}", name, e))?;
    if bytes.len() as u64 > hard {
        return Err(format!(
            "「{}」过大（超过 {} MB）：请先拆分，或改用 EPUB / 按章导入",
            name,
            hard / 1024 / 1024
        ));
    }
    Ok(bytes)
}

/// 读入校对稿，返回 `(源码文本, 给用户看的提示)`。
///
/// 三条纪律：
///  ① **原样读**：不清洗、不转繁简、不删空行——编辑器要显示的就是原文
///    （`.md` 的 `#` / `**` / `---` 必须看得见，抄校才有依据）；
///  ② 编码宽松：`from_utf8_lossy` 兜住 GBK 老稿的零星坏字节，绝不为一个
///    怪字节报错退出（古籍稿常见扫描后转码的残留）；
///  ③ 超长截断并**说清楚**：编辑器逐行自绘，字数是唯一会拖慢首帧的量；
///  ④ **本函数恒在后台 executor 上跑**（调用点见 `open_text_picker` 的
///    `bg.spawn`）——读盘 + 解码 + 数字数三步都是 IO/CPU，一步都不许压主线程。
///    产品边界（用户 2026-11-01 定）：>8 MB / >8 万字的稿子照样能进来，只是
///    读盘在后台、超出 8 万字的部分按字截断并在状态栏挑明「只载入了前 N 字」。
fn read_text_file(path: &Path) -> Result<(String, String), String> {
    let name = file_display_name(path);
    let bytes = read_bytes_capped(path, &name, TEXT_IMPORT_HARD_BYTES)?;
    // BOM 与 CRLF 归一：否则首字会是个看不见的 U+FEFF，行尾多一个 '\r'。
    let text = String::from_utf8_lossy(&bytes)
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let total = text.chars().count();
    if total == 0 {
        return Err(format!("「{}」是空文件", name));
    }
    if total > TEXT_MAX_CHARS {
        let cut: String = text.chars().take(TEXT_MAX_CHARS).collect();
        return Ok((
            cut,
            format!(
                "「{}」共 {} 万字，超过单册上限 {} 万字：已载入前 {} 万字，其余部分请按卷导入",
                name,
                total / 10_000,
                TEXT_MAX_CHARS / 10_000,
                TEXT_MAX_CHARS / 10_000
            ),
        ));
    }
    Ok((text, format!("已载入「{}」　{} 字（源码原样显示）", name, total)))
}

/// 临时 PNG 输出路径（系统 temp 目录，按纳秒时间戳唯一命名，避免缓存串图）。
fn temp_png_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{}{}.png", TEMP_PNG_PREFIX, nanos))
}

/// 本模块产出的临时 PNG 统一前缀（清扫时只认这个前缀，绝不碰别人的文件）。
const TEMP_PNG_PREFIX: &str = "jiandu_collation_";

/// 清扫**上一进程**遗留的临时图：只删「文件名合前缀 + 修改时间够旧」的。
///
/// 为什么必须有这一刀：预览图 / 翻页栅格化每翻一页产一张临时 PNG，正常路径上
/// 由「重生成时清旧页」「代际过期丢弃」「`Drop` 收尾」三处回收；但崩溃、强退、
/// 活动监视器 kill 这些路径一行 `Drop` 都跑不到，临时目录就会一点点堆起来
/// （实测一次调试积了 61 张）。启动时补扫是唯一不依赖退出路径的兜底。
///
/// 只认 1 小时前的文件：另开的一个笺读实例可能正在用它刚生成的图，够旧的才删。
fn sweep_stale_temp_pngs() {
    const STALE_AFTER: Duration = Duration::from_secs(3600);
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        let path = e.path();
        let is_ours = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with(TEMP_PNG_PREFIX) && n.ends_with(".png"))
            .unwrap_or(false);
        if !is_ours {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|age| age > STALE_AFTER)
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 工作台销毁（切走 / 关窗 / 退出）时回收本进程产出的临时图，不留垃圾。
///
/// ⚠️ 只删 `preview_paths` 与 `last_temp` —— 这两个字段里**只会**是本模块自己
/// 栅格化出来的临时 PNG；用户导入的原图路径（`image_path` 可能等于它）绝不在此
/// 之列，故不存在误删用户文件的风险。
impl Drop for CollationWorkspace {
    fn drop(&mut self) {
        for p in self.preview_paths.drain(..) {
            let _ = std::fs::remove_file(&p);
        }
        for p in self.temp_gc.drain(..) {
            let _ = std::fs::remove_file(&p);
        }
        if let Some(t) = self.last_temp.take() {
            let _ = std::fs::remove_file(t);
        }
    }
}

/// 栅格化结果：显示路径、临时路径、原图自然像素尺寸、页码信息、温和提示。
/// `nat_w` / `nat_h` 供真实坐标映射（`source_size`）把 `SourceRegion` 自然像素
/// 缩放到显示坐标；无原图时回退到区域包围盒。
/// PDF 页缓存的容量（页）。
///
/// 1120×1679 RGBA ≈ 7.5MB/页 → 4 页约 30MB。再往上加就是拿内存换「翻得再远
/// 也秒开」，性价比开始变差（校对通常只在相邻两三页之间来回）。缓存的是 CPU
/// 位图，不占 GPU 纹理。
const PDF_CACHE_CAP: usize = 4;

/// 缓存溢出时挑牺牲者：离当前页**最远**的那一页。
///
/// 为什么不用 LRU：校对的动作就是「两页之间来回对」。刚离开的那一页恰恰最
/// 可能马上被翻回来，LRU 会第一个把它踢掉——越是来回看，越是每次都重新栅格化。
/// 距离最远的那页才是真的冷。当前页自己永不牺牲（回 `None`，调用方就此停手）。
fn victim_of(keys: impl Iterator<Item = usize>, cur: usize) -> Option<usize> {
    keys.max_by_key(|&p| p.abs_diff(cur)).filter(|&p| p != cur)
}

/// 一页 PDF 的成品：位图 + 该页文本层坐标。
///
/// 两者**必须绑在一起缓存**：翻页后「点右文 → 亮左图」依赖 `regions` 与新页
/// 对齐；只缓存图、坐标每次重抽，等于把省下的 10–40ms 又还回去了。
#[derive(Clone)]
struct CachedPage {
    /// 已栅格化的整页位图（就是交给 `img()` 的那一份）。
    image: Arc<RenderImage>,
    nat_w: f64,
    nat_h: f64,
    regions: Vec<crate::pdf_text::SourceRegion>,
}

/// 后台产出一页 PDF 的「位图 + 文本层坐标」。
///
/// 两件事必须**同一趟**做完：都要开 PDF 文档，分开就是两次 IO + 两次
/// `PDFDocument` 初始化。更关键的是——文本层抽取以前排在主线程，翻页时那
/// 10–40ms 直接压在输入响应上。
///
/// 线程安全：`extract_pdf_text_regions_on` 每次 `alloc/init` 自己的
/// `PDFDocument`，不共享可变状态，见其文档注释里的前提。
async fn render_pdf_page(
    exec: gpui::BackgroundExecutor,
    path: PathBuf,
    page: usize,
    target_px: f32,
) -> (
    Result<Rasterized, String>,
    Vec<crate::pdf_text::SourceRegion>,
) {
    exec.spawn(async move {
        let raster = rasterize_source(&path, page, target_px);
        let regions = crate::pdf_text::extract_pdf_text_regions_on(&path, page.saturating_sub(1));
        (raster, regions)
    })
    .await
}

struct Rasterized {
    /// 位图原图直接复用原路径（`gpui::img` 自带解码）；PDF 页为 `None`。
    path: Option<PathBuf>,
    /// 临时 PNG 路径（仅位图原图之外的兜底路径会产出；PDF 走内存后恒为 None）。
    temp: Option<PathBuf>,
    /// PDF 页的**内存位图**：`img(Arc<RenderImage>)` 直接上传纹理，不落盘。
    image: Option<Arc<RenderImage>>,
    nat_w: f64,
    nat_h: f64,
    /// 本次渲出的是第几页（1-based；位图原图恒为 1）。
    page: usize,
    /// 总共多少页（位图原图恒为 1）。
    total: usize,
    note: Option<String>,
}

/// 栅格化长边上限（像素）。CGBitmapContext 分配失败是**直接闪退**，不是返回 nil。
#[cfg(target_os = "macos")]
const MAX_RASTER_DIM: f64 = 4096.0;

/// 栅格化位图字节上限（64 MiB）。与 [`MAX_RASTER_DIM`] 一起兜住内存。
#[cfg(target_os = "macos")]
const MAX_RASTER_BYTES: f64 = 64.0 * 1024.0 * 1024.0;

/// 由「页面点尺寸 + 显示像素宽」算出栅格化画布的像素尺寸（**只升不降**）。
///
/// # 为什么不能按 MediaBox 的「点」直接当像素
///
/// 这是影印本发糊的根，也是最容易被误判成「渲染不行」的一处。MediaBox 的单位
/// 是**点**（1/72 英寸），而影印本内嵌的是 **300dpi 二值扫描**（JBIG2，一页约
/// 1670×2500 像素）压在 595×892 点的页面上。旧实现拿 `nat_w × nat_h` 当像素
/// 建位图上下文 —— 等于把 300dpi 的扫描**先降到 72dpi**（丢掉约 4 倍细节），
/// 再由 `img().w_full()` 在 Retina 屏上放大回去（再插值一次）。一降一升，糊
/// 是必然的，而且和字号、字库、抗锯齿都无关。
///
/// # 三条规则
///
/// ① **只升不降**：`target_px` 小于 MediaBox 宽（窄窗口 / 非 Retina）时仍按
///    1:1，绝不把大尺寸扫描件主动缩下来——缩小省的那点内存，赔的是清晰度。
/// ② **两条硬上限**：长边 ≤ [`MAX_RASTER_DIM`]、`宽×高×4 ≤ MAX_RASTER_BYTES`。
///    上限优先于 ①（超大海报页必须降，否则分配失败直接闪退）。
/// ③ `target_px <= 0`（调用方还不知道显示尺寸）时退回 1:1。
#[cfg(target_os = "macos")]
fn raster_px(nat_w: f64, nat_h: f64, target_px: f32) -> (usize, usize) {
    let want = if target_px.is_finite() && target_px > 0.0 {
        (target_px as f64 / nat_w.max(1.0)).max(1.0)
    } else {
        1.0
    };
    // 两条上限都是**比例**（不是 1.0）：页面比上限小时它是 >1 的数，不能反过来
    // 把升采样也挡掉——这正是 `scan_is_rasterized_at_display_pixels_not_pdf_points`
    // 抓到的那个 bug（写成 `if long > MAX { MAX/long } else { 1.0 }` 时，
    // 1120 的目标被 1.0 掐回 595，等于白改）。
    let long = nat_w.max(nat_h);
    let dim_cap = if long > 0.0 {
        MAX_RASTER_DIM / long
    } else {
        1.0
    };
    let area_cap = {
        let bytes = nat_w * nat_h * 4.0;
        if bytes > 0.0 {
            (MAX_RASTER_BYTES / bytes).sqrt()
        } else {
            1.0
        }
    };
    let scale = want.min(dim_cap).min(area_cap).max(0.01);
    // MediaBox 宽高是浮点数：一律 ceil() 向上取整，绝不截断——截断会让画布比
    // 实际内容窄 1px，行尾像素写到缓冲区外面去。
    let w = (nat_w * scale).max(1.0).ceil() as usize;
    let h = (nat_h * scale).max(1.0).ceil() as usize;
    (w, h)
}

#[cfg(target_os = "macos")]
fn rasterize_source(path: &Path, page_no: usize, target_px: f32) -> Result<Rasterized, String> {
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
            path: Some(path.to_path_buf()),
            temp: None,
            image: None,
            nat_w: nw,
            nat_h: nh,
            page: 1,
            total: 1,
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
    // 页号：外部传 1-based；这里钳到 [1, total]（越界绝不 panic，退回末页 / 首页）。
    let page_no = page_no.clamp(1, total.max(1));
    let page = unsafe { CGPDFDocumentGetPage(doc, page_no) };
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
        return Err(format!("PDF 第 {} 页不存在", page_no));
    }
    // 媒体框（PDF 点）：坐标映射的基准尺寸，与文本层坐标同单位。
    let mb = unsafe { CGPDFPageGetBoxRect(page, 0) }; // kCGPDFMediaBox = 0
    // ⚠️ 页面可能被要求**旋转显示**（/Rotate 90 / 270）：扫描件里横放的插页、
    //    为省纸而旋转排版的内页都很常见。旋转 90/270 时显示宽高是**互换**的——
    //    照 MediaBox 建画布、再让 GetDrawingTransform 等比塞进去，只会得到
    //    「内容缩成一条横带、两侧大片留白」，文本层坐标也全对不上。
    //    这里的 nat_w/nat_h 是**旋转后的显示尺寸**，与 `pdf_text` 里旋转过的
    //    regions 同一坐标系，两边必须严格一致。
    let rotation = unsafe { CGPDFPageGetRotationAngle(page) };
    let quarter_turn = rotation.rem_euclid(180) == 90;
    let (nat_w, nat_h) = if quarter_turn {
        (mb.h, mb.w)
    } else {
        (mb.w, mb.h)
    };
    if nat_w <= 0.0 || nat_h <= 0.0 {
        unsafe { CGPDFPageRelease(page) };
        return Err("PDF 页面尺寸为 0".to_string());
    }

    // ① 画布像素尺寸：由**显示尺寸**反推，不是把 MediaBox 的点当像素。
    //    影印本内嵌 300dpi 扫描，按点 1:1 建位图 = 先降到 72dpi 再被 Retina
    //    放大回去，一降一升必然糊（详见 `raster_px` 的说明）。
    let (w, h) = raster_px(nat_w, nat_h, target_px);

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
            // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big
            //
            // 必须显式给 `ByteOrder32Big`（v1.9.30）：只给 alpha 位时内存序随
            // 主机端序走，实际是 **BGRA**，于是每帧都要在 Rust 里逐像素倒成 RGBA
            // （1120×1678 就是 188 万次搬运）。指定 32Big 后 CG 直接按 R,G,B,A
            // 落字节，这一整趟转换消失。alpha 恒为 255（书页不透明），故
            // premultiplied 与 straight 等价。
            1 | (4 << 12),
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
    //
    //    `rotation` 已在上面取过（画布尺寸要用），这里直接复用：传 0 的话
    //    GetDrawingTransform 会忽略 /Rotate，位图就与 regions 差一次旋转。
    let target = CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(w as f64, h as f64),
    );
    let transform =
        unsafe { CGPDFPageGetDrawingTransform(page, 0, target, rotation, true) };
    unsafe {
        CGContextConcatCTM(ctx.as_ptr(), transform);
        CGContextDrawPDFPage(ctx.as_ptr(), page);
        CGPDFPageRelease(page);
    }

    // CG 已按 R,G,B,A 落字节（见上面 `ByteOrder32Big`），这里**不再倒色序**，
    // 只做「行距对齐 → 紧凑」的搬运：行距正好等于 宽×4 时直接整块拷走。
    // ⚠️ 绝不直接下标切片：行距用系统返回的真实值（CG 可能按 16 字节对齐放大），
    //    区间一律 .get() 取；一旦越界优雅报错返回，绝不 panic / 越界写。
    let bpr_actual = ctx.bytes_per_row();
    let row_bytes = w * 4;
    if bpr_actual < row_bytes {
        return Err(format!(
            "位图行距异常（系统 bytes_per_row={} < 宽×4={}），放弃转换",
            bpr_actual, row_bytes
        ));
    }
    let data = ctx.data();
    let tight: usize = row_bytes * h;
    let pixels: Vec<u8> = if bpr_actual == row_bytes {
        // 快路径：行距正好紧凑，一次整块拷走（绝大多数情况走这里）。
        data.get(..tight)
            .ok_or_else(|| {
                format!(
                    "位图缓冲区偏短（需 {} 字节，实得 {}）",
                    tight,
                    data.len()
                )
            })?
            .to_vec()
    } else {
        let mut v: Vec<u8> = Vec::with_capacity(tight);
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
            v.extend_from_slice(src);
        }
        v
    };
    drop(ctx);

    // **不落盘**（v1.9.30）：直接包成 gpui 的内存位图。
    // 原先这里要 PNG 编码 + 写盘 + 由 `gpui::img` 读盘解码，一页两次编解码 +
    // 两次 IO；现在 `ImageSource::Render` 分支直接拿去上传纹理。
    let buffer = image::RgbaImage::from_raw(w as u32, h as u32, pixels).ok_or_else(|| {
        format!(
            "位图尺寸与缓冲区不符（{}×{} 需要 {} 字节）",
            w,
            h,
            tight
        )
    })?;
    // `RenderImage::new` 收 `SmallVec<[Frame; 1]>`，用 `Vec` 走 smallvec 的
    // `From<Vec<_>>`（单帧图，多一步 move 而已）。
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));

    // ③ 多页：现在**能翻**了（见 `flip_page`），不再只渲首页；提示只报页码与总数。
    //    文案里明说「仅渲当前页」——大文件的页数由翻页承担，不整本进内存。
    let note = if total > 1 {
        Some(format!(
            "第 {} / {} 頁（← → 翻页；仅渲当前页，不占整本内存）",
            page_no, total
        ))
    } else {
        None
    };
    Ok(Rasterized {
        path: None,
        temp: None,
        image: Some(image),
        nat_w,
        nat_h,
        page: page_no,
        total,
        note,
    })
}

#[cfg(not(target_os = "macos"))]
fn rasterize_source(_path: &Path, _page_no: usize, _target_px: f32) -> Result<Rasterized, String> {
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
        // **永远返回零宽选区**：平台（macOS NSTextInputClient）会按 `selected_text_range`
        // 绘制**原生选中高亮**——未聚焦时是灰蓝、聚焦时是蓝，正好盖住我们自绘的朱砂
        // `rgba(0x8c2b2233)`（`render_line` 在字下画的底色）。自绘层只画我们自己的
        // 颜色、且永远在字下面，所以这里绝不能把真实选区交给平台，否则朱砂被原生蓝
        // 「反客为主」。选区高亮**完全由 `render_line` 负责**；复制 / 拖拽扩展 / IME
        // 合成都走我们自己的逻辑，不依赖平台选区范围。
        let a = char_to_utf16(&self.text, self.cursor);
        Some(gpui::UTF16Selection {
            range: a..a,
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
        // 查找条开着：上屏的字进查找词，**绝不写进正文**（v1.9.68）。
        // 不拦这一步，中文用户按 ⌘F 打一个拼音就会把候选字打进稿子里。
        if self.find_open {
            self.find_query.push_str(text);
            self.find_marked.clear();
            self.find_refresh(cx);
            return;
        }
        let eff = effective_replace_range(range, &self.marked);
        self.marked = None;
        self.apply_replace_utf16(eff, text);
        self.selection = None;
        cx.notify();
        self.touch(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 查找条开着：组字只更新「正在组」的那一截，等上屏（`replace_text_in_range`）
        // 才真正并入查找词。组字是逐帧替换的，每帧都全库扫一遍会卡住输入法。
        if self.find_open {
            self.find_marked = new_text.to_string();
            self.marked = None;
            cx.notify();
            return;
        }
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
        self.touch(cx);
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
        let total = self.total_chars();
        let a = a.min(total);
        let b = b.clamp(a, total);
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        // 字节区间替换（v1.9.69）：组字每出一帧候选都会走这里，百万字若还
        // `chars().collect()` 就是把 20MB 反复搬来搬去——拼音打到第三个字母就明显卡。
        let (bs, be) = (self.byte_of_char(a), self.byte_of_char(b));
        let replaced: String = self.text[bs..be].to_string();
        let n = clean.chars().count();
        self.text.replace_range(bs..be, &clean);
        self.text_gen = self.text_gen.wrapping_add(1);
        self.cursor = a + n;
        // 输入法这条路的撤销：组字过程中 `replace_and_mark_text_in_range` 会被
        // 逐帧调用（n → ni → nih → niha…），逐帧记步会让 ⌘Z 退成「一个字退一下」。
        // 统一标 `Ime`，由 `try_absorb` 把连续组字合成**一次输入**。
        self.record_edit(a, replaced, clean, EditKind::Ime);
        char_to_utf16(&self.text, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 右文带 Markdown 记号时，**剥记号再比**必须仍然命中同一段原图文字。
    ///
    /// 这就是真机上「明明同一段话，却提示不匹配、高亮不出来」的复现与回归：
    /// 拿 `## 第一節 四時` / `**寒來暑往**` 这种带记号的串直接去和 PDF 纯文本做
    /// 连续二字比对，命中率会被 `#`、`*` 拉垮到阈值以下。
    #[test]
    fn markdown_markers_do_not_break_matching() {
        let region = "昃，辰宿列張。第一節 四時寒來暑往，秋收冬藏。閏餘成歲";
        let md = "## 第一節 四時\n\n**寒來暑往**，秋收冬藏。";
        assert!(
            bigram_overlap(md, region) > 0.85,
            "MD 剥记号后应高度命中：{}",
            bigram_overlap(md, region)
        );
        // 表格行（`|`）与引用（`>`）同理。
        assert!(bigram_overlap("> | 第一節 | 四時 |", region) > 0.3);
    }

    /// OCR / 纯文本（.txt）这条路**优先级更高**：不带任何记号时必须原样命中。
    #[test]
    fn plain_ocr_text_matches_best() {
        let region = "資治通鑑卷第一 周紀一 起著雍攝提格";
        assert!(bigram_overlap("資治通鑑卷第一，周紀一，起著雍攝提格", region) > 0.95);
        // 标点不同 / PDF 无标点也不该扣分（比对前统一滤标点）。
        assert!(bigram_overlap("資治通鑑卷第一周紀一起著雍攝提格", region) > 0.95);
    }

    // ── 「点哪高亮哪」的定位器（v1.9.23）───────────────────────────────

    fn region(text: &str, y: f32) -> SourceRegion {
        SourceRegion {
            x: 72.0,
            y,
            w: 360.0,
            h: 24.0,
            text: text.to_string(),
        }
    }

    /// 最长公共子串的 DP 得数要对（这是定位器的度量本体）。
    #[test]
    fn lcs_len_measures_the_longest_run_not_the_overlap() {
        let a: Vec<char> = "甲乙丙丁戊".chars().collect();
        let b: Vec<char> = "子丑甲乙丙丁寅卯".chars().collect();
        assert_eq!(lcs_len(&a, &b), 4, "「甲乙丙丁」是连续 4 字");
        // 交错重合**不算**：单字都出现过，但没有连续段（最长只到 1 字）。
        let c: Vec<char> = "甲子乙丑丙寅".chars().collect();
        assert_eq!(lcs_len(&c, &a), 1, "交错出现的单字不构成连续重合");
        // 空侧不 panic。
        assert_eq!(lcs_len(&[], &b), 0);
        assert_eq!(lcs_len(&a, &[]), 0);
    }

    /// **顺序无关**：右文某一段必须命中「最像的那一处」，而不是「第几行就配第几处」。
    ///
    /// 这就是真机「高亮框飘了」的回归——原先按 `regions[行号]` 取，段数一错就全错。
    #[test]
    fn locate_region_finds_the_text_wherever_it_sits() {
        let regions = vec![
            region("天地玄黃，宇宙洪荒。", 100.0),
            region("寒來暑往，秋收冬藏。閏餘成歲，律呂調陽。", 200.0),
            region("金生麗水，玉出崑岡。", 300.0),
        ];
        // 右文第 0 行对应的其实是**第 3 处**。
        let hit = locate_region("金生麗水，玉出崑岡。", &regions).expect("应命中第 3 处");
        assert_eq!(hit.0, 2, "必须命中下标 2，而不是按行号取到的 0");
        assert!(hit.1 >= 6, "连续重合字数应 ≥ LCS_MIN：{}", hit.1);
    }

    /// Markdown 记号 + 标点 + 空格全部忽略，只比纯中文（用户第 2 条要求）。
    #[test]
    fn locate_region_ignores_markdown_punctuation_and_spaces() {
        let regions = vec![
            region("第一節 四時寒來暑往秋收冬藏", 100.0),
            region("閏餘成歲律呂調陽", 200.0),
        ];
        // `##`、`**`、全角标点、半角空格一个不少 —— 仍须稳准命中第 1 处。
        let hit = locate_region("## 第一節 四時\n\n**寒來暑往**，秋收冬藏。", &regions)
            .expect("洗掉记号后应命中");
        assert_eq!(hit.0, 0);
    }

    /// 不相干的稿子**必须拒绝**：宁可报「未匹配」，也绝不把框画到随机一处。
    #[test]
    fn locate_region_refuses_unrelated_text() {
        let regions = vec![region("資治通鑑卷第一 周紀一 起著雍攝提格", 100.0)];
        assert!(
            locate_region("天地玄黃，宇宙洪荒。", &regions).is_none(),
            "两段话毫不相干，不许硬凑出一个「最像」的区域"
        );
        // 只有一两个字相同时也不行（`LCS_MIN` 的那道坎）。
        assert!(locate_region("之", &regions).is_none(), "单字不构成定位");
    }

    // ── 产品边界：大文件不许撑爆内存，也不许糊里糊涂被拒 ──────────────────

    /// 硬上限是**读取侧**的闸门：先整读再判大小正是撑爆内存的写法。这里拿 1 KB
    /// 当闸门去撞一个 6 KB 的中文文件——必须当场报「过大」，而不是读完之后才说。
    #[test]
    fn oversized_text_file_is_rejected_at_the_read_side() {
        let dir = std::env::temp_dir().join(format!(
            "jiandu_cap_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("大部頭.txt");
        std::fs::write(&p, "甲".repeat(2000)).unwrap(); // 3 字节/字 = 6000 字节
        let name = file_display_name(&p);
        assert_eq!(name, "大部頭.txt");

        let err = read_bytes_capped(&p, &name, 1024).unwrap_err();
        assert!(err.contains("过大"), "越界必须明确报「过大」：{}", err);
        assert!(err.contains("大部頭.txt"), "提示里要带文件名：{}", err);

        // 闸门放到 64 KB：同一个文件必须原样读完（证明不是「一律拒绝」）。
        let ok = read_bytes_capped(&p, &name, 64 * 1024).unwrap();
        assert_eq!(ok.len(), 6000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 两条线各自的位置钉死：**64 MB 硬闸门**（不是 8 MB 一刀切）+ 500 万字截断线。
    ///
    /// 500 万字按汉字 3 字节算 15 MB，仍在 64 MB 硬闸门之内（`* 4` 兜住最坏编码）——
    /// 否则整部《資治通鑑》连「被截断」的机会都没有就被拒了，等于「我的书导入不了」。
    #[test]
    fn product_limits_are_the_agreed_ones() {
        assert_eq!(TEXT_IMPORT_HARD_BYTES, 64 * 1024 * 1024);
        assert_eq!(TEXT_MAX_CHARS, 5_000_000);
        assert!(
            TEXT_IMPORT_HARD_BYTES > (TEXT_MAX_CHARS as u64) * 4,
            "硬闸门必须高于截断线（按最坏 4 字节/字算），否则截断逻辑永远跑不到"
        );
    }

    // ── 行索引（v1.9.69）：虚拟列表的地基 ─────────────────────────────────

    /// 前缀和必须与「逐行实实在在数一遍」完全一致——差一行，滚动条就会跳。
    #[test]
    fn line_index_prefix_sums_match_a_hand_count() {
        let text = "第一行\n\n第三行很长很长……\nabc\nde";
        let idx = LineIndex::build(text, None, 5, 7, false);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(idx.lines(), lines.len());
        // 字符起点 / 字节起点逐行对齐。
        let (mut cs, mut bs) = (0usize, 0usize);
        for (li, l) in lines.iter().enumerate() {
            assert_eq!(idx.char_start[li], cs, "第 {li} 行字符起点错");
            assert_eq!(idx.byte_start[li], bs, "第 {li} 行字节起点错");
            cs += l.chars().count() + 1;
            bs += l.len() + 1;
        }
        assert_eq!(idx.total_chars, text.chars().count());
        // 视觉行前缀和 = 逐行上取整累加。
        let mut acc = 0usize;
        for (li, l) in lines.iter().enumerate() {
            acc += est_wrapped_lines(l, 5);
            assert_eq!(idx.vis[li + 1], acc, "第 {li} 行视觉行前缀错");
        }
        assert_eq!(idx.total_visual(), acc);
        // 非对照模式：aligned 与 vis 同值。
        assert_eq!(idx.aligned, idx.vis);
        // 行尾换行符位归**前一行**（⇧→ 的手感就靠这条）。
        assert_eq!(idx.line_of(0), Some(0));
        assert_eq!(idx.line_of(3), Some(0), "第 0 行末尾的换行位属于第 0 行");
        assert_eq!(idx.line_of(4), Some(1), "空行独占一个位");
        assert!(idx.line_of(text.chars().count()).is_some());
        // 字节区间取出来的正是该行原文。
        for (li, l) in lines.iter().enumerate() {
            assert_eq!(idx.line_str(text, li), *l);
        }
    }

    /// 对照模式：每段高度取两栏的**较大者**（段落对齐的数学依据）。
    #[test]
    fn diff_prefix_takes_the_taller_side() {
        // 清洗后 2 个视觉行（cpl=6，12 字），毛坯只有 1 行 → 取 2。
        let clean2 = "一二三四五六七八九十一二";
        let raw2 = "一二三";
        let idx2 = LineIndex::build(clean2, Some(raw2), 6, 1, true);
        assert_eq!(idx2.vis[1], 2);
        assert_eq!(idx2.aligned[1], 2, "取较大者：2 > 1");
        // 反过来：毛坯更长 → 对齐后仍取 2，右栏空的这一行由填充撑住。
        let idx3 = LineIndex::build("一二三", Some(clean2), 6, 1, true);
        assert_eq!(idx3.vis[1], 1);
        assert_eq!(idx3.aligned[1], 2, "取较大者：1 < 2");
        // 毛坯行数比清洗后少（清洗可能增段）→ 越界的毛坯行返回空串，绝不 panic。
        assert_eq!(idx3.raw_line(clean2, 1), "");
        // 非对照模式（diff=false）时两者必须完全一致：右栏没有段落填充。
        let idx4 = LineIndex::build(clean2, None, 6, 1, false);
        assert_eq!(idx4.aligned, idx4.vis);
    }

    /// 视觉行 → 行号的二分必须与「逐行累加找」一致，且永不越界。
    #[test]
    fn visual_to_line_lookup_never_overshoots() {
        let mut text = String::new();
        for i in 0..500 {
            // 行长度在 0..30 之间摆动，制造 1~5 个视觉行不等。
            let n = (i * 7) % 30;
            for _ in 0..n {
                text.push('字');
            }
            if i < 499 {
                text.push('\n');
            }
        }
        let cpl = 7;
        let idx = LineIndex::build(&text, None, cpl, 3, false);
        let lines: Vec<&str> = text.split('\n').collect();
        // 逐视觉行问一遍，答案必须落在「该视觉行确实属于这一行」的区间里。
        for v in 0..idx.total_visual() {
            let li = idx.line_at_visual(v);
            assert!(li < lines.len(), "越界：v={v} → li={li}");
            assert!(
                idx.vis[li] <= v && v < idx.vis[li + 1],
                "v={v} 落在 li={li} 之外（区间 {}..{}）",
                idx.vis[li],
                idx.vis[li + 1]
            );
        }
        // 超出总量 → 夹到末行，不 panic。
        assert_eq!(idx.line_at_visual(idx.total_visual() + 999), lines.len() - 1);
    }

    /// **性能守门**：百万字建索引只跑一次（文本 / 列数不变就不重建）。
    ///
    /// 这里只钉「建得出来且不算完」，不断言毫秒数——CI 机器差异太大。
    /// 真正防的是「退回每帧 O(全文)」：那会让 `render` 里每帧都走一遍，
    /// 这个测试跑 100 万字符若卡住，肉眼立刻能看出来。
    #[test]
    fn million_char_index_builds_and_answers_in_log_time() {
        let text = "資治通鑑卷第一周紀一起著雍攝提格盡玄黓困敦凡三十五年".repeat(50_000);
        let idx = LineIndex::build(&text, None, 37, 9, false);
        assert_eq!(idx.total_chars, text.chars().count());
        assert_eq!(idx.lines(), 1, "整本没有换行 → 一个逻辑行");
        // 二分定位：无论问哪一条视觉行，都是一次查找的事。
        let mid = idx.total_visual() / 2;
        let li = idx.line_at_visual(mid);
        assert_eq!(li, 0);
        assert!(idx.line_of(idx.total_chars / 2).is_some());
    }

    /// **虚拟列表的核心不变量**：无论稿子多长、滚到哪里，一帧只画「一屏 + 四屏余量」
    /// 那么多的行。这条断言就是「百万元字不会一次性全渲染」的机器化表述。
    ///
    /// 注意：它不测 `render_editor`（那要开窗口），而是测它依赖的两个事实——
    /// 前缀和能把「像素」翻译成「行号」，且窗口宽度与文本总量无关。
    #[test]
    fn visible_window_stays_bounded_no_matter_how_long_the_book_is() {
        // 造一本 ~100 万字的「书」，每 40 字一段。
        let para = "資治通鑑卷第一周紀一起著雍攝提格盡玄黓困敦凡三十五年";
        let text = para.repeat(40_000); // 26 字 × 4 万 ≈ 104 万字
        let cpl = 37; // 1440 宽窗口、左右各半、17px 字 → 约 37 字一行
        let idx = LineIndex::build(&text, None, cpl, 11, false);
        assert!(idx.total_chars > 900_000, "样本够大：{}", idx.total_chars);
        let total_v = idx.total_visual();
        let (vis_h, overscan) = (900.0f32, 900.0f32 * 2.0);
        let mut max_rows = 0usize;
        for scroll in [0.0f32, 500.0, 5_000.0, 60_000.0, 120_000.0] {
            let first_v = ((scroll - ED_PAD - overscan).max(0.0) / LINE_H).floor() as usize;
            let last_v = (((scroll + vis_h + overscan) / LINE_H).ceil() as usize).min(total_v - 1);
            let li0 = idx.line_at_visual(first_v);
            let li1 = idx.line_at_visual(last_v);
            let rows = li1 - li0 + 1;
            max_rows = max_rows.max(rows);
            assert!(li0 <= li1);
        }
        // 一屏 ≈ 30 行，加四屏余量 ≈ 150 行；给到 200 已经是很宽松的上界。
        assert!(
            max_rows <= 200,
            "窗口化失效：一帧竟然要建 {max_rows} 行（100 万字的稿子）"
        );
    }

    // ── 选区气泡坐标（v1.9.69 回归）────────────────────────────────────
    /// **回归钉子**：气泡跟着**内容**走，不跟着屏幕走。
    ///
    /// v1.9.68 的真 bug 是 `- scroll_y`：偏移本身是负数，再取反等于「向下滚
    /// 300px，气泡往下跑 600px」。这里用两个原点（未滚 / 已滚 300）验证：
    /// 同一个选区在滚动后 y 必须**同样上移 300**，而不是下移。
    #[test]
    fn bar_follows_content_when_scrolled() {
        let adv = 17.0;
        let (x0, y0) = bar_anchor((100.0, 44.0 + 40.0), 10, 6, adv);
        let (x1, y1) = bar_anchor((100.0, 44.0 + 40.0 - 300.0), 10, 6, adv);
        assert_eq!(x0, x1, "滚动只影响 y");
        assert!(
            (y0 - y1 - 300.0).abs() < 0.01,
            "滚动 300 后气泡必须上移 300，实际 Δ={}",
            y0 - y1
        );
        // 锚点本身贴在「末行下一行」：比末行顶多一整行 + 4px。
        let expect_y = 44.0 + 40.0 + (10 + 1) as f32 * LINE_H + 4.0;
        assert!((y0 - expect_y).abs() < 0.01, "锚点公式被改了？y0={y0}");
        // X 跟着列位走（第 6 个字缝）。
        assert!((x0 - (100.0 + 6.0 * adv + 4.0)).abs() < 0.01);
    }

    /// 任何输入都不许把气泡算到面板外——这是「拖完选却看不见气泡」的第二半答案。
    #[test]
    fn bar_is_always_clamped_inside_the_panel() {
        let (vw, vh) = (1440.0, 900.0);
        let (pl, pt) = (720.0, 44.0 + 50.0);
        let cases = [
            (-500.0, -500.0),
            (0.0, 0.0),
            (5000.0, 5000.0),
            (f32::MAX, f32::MAX),
            (f32::MIN, f32::MIN),
        ];
        for (bx, by) in cases {
            let (x, y) = clamp_bar_pos(bx, by, vw, vh, pl, pt, 66.0, 24.0);
            assert!(x >= pl, "x={x} 跑到左栏里去了（panel_l={pl}）");
            assert!(x + 66.0 <= vw, "x={x} 右侧出窗口（vw={vw}）");
            assert!(y >= pt, "y={y} 压到状态条上去了（panel_t={pt}）");
            assert!(y + 24.0 <= vh, "y={y} 底部出窗口（vh={vh}）");
        }
    }
    // ── 纸张口径：容量从版式实测，绝不写第二张魔数表 ─────────────────────
    /// 换纸必须换容量；`預覽卡` 的老经验表一个字都不许变。
    #[test]
    fn page_capacity_follows_the_paper() {
        // 取景卡沿用 v1.9.x 的经验表（12 列 → 110 字）。
        assert_eq!(
            CollationWorkspace::page_cap(
                Paper::Card,
                Imposition::Single,
                12,
                ShareBg::Xuan,
                ShareFrame::Red
            ),
            110
        );
        // 真实纸张问版式自己。A4 / A3 同比例（1:√2），故同列数下容量应当接近
        // ——只差像素取整带来的±1 行（A4 的相对留白比 A3 略多一点点）。
        let a4 = CollationWorkspace::page_cap(
            Paper::A4,
            Imposition::Single,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
        );
        let a3 = CollationWorkspace::page_cap(
            Paper::A3,
            Imposition::Single,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
        );
        let diff = (a4 as f32 - a3 as f32).abs() / a3 as f32;
        assert!(diff < 0.10, "同比例纸张容量差太多：A4={a4} A3={a3}");
        assert!(a3 > 120, "A4/A3 一页只装得下 {a3} 字，明显不对");
    }

    /// **拼版单页 = A4 那一档**：A3 雙頁的每页容量必须与 A4 单页几乎一样。
    ///
    /// 这就是「书」与「大字海报」的分界在容量上的体现：拼版单页排的是
    /// 202×297mm（≈A4）的版心，绝不能用整张 A3 的宽度去算列数。
    #[test]
    fn spread_page_holds_a4_worth_of_text_not_a3_worth() {
        let a4 = CollationWorkspace::page_cap(
            Paper::A4,
            Imposition::Single,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
        );
        let book = CollationWorkspace::page_cap(
            Paper::A3,
            Imposition::Spread,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
        );
        let diff = (a4 as f32 - book as f32).abs() / a4 as f32;
        assert!(
            diff < 0.12,
            "拼版单页的容量没回到 A4 档：A4={a4} 拼版单页={book}"
        );
        // 取景卡不受拼版影响（它不是纸）。
        assert_eq!(
            CollationWorkspace::page_cap(
                Paper::Card,
                Imposition::Spread,
                12,
                ShareBg::Xuan,
                ShareFrame::Red
            ),
            110
        );
    }

    /// 从 PDF 字节里读**媒体框**（宽, 高）——纸张方向唯一的硬证据。
    fn media_box(bytes: &[u8]) -> (f32, f32) {
        let hay = String::from_utf8_lossy(bytes);
        let i = hay.find("MediaBox").expect("PDF 里找不到 MediaBox");
        let seg = &hay[i..(i + 90).min(hay.len())];
        let nums: Vec<f32> = seg
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter_map(|t| t.parse::<f32>().ok())
            .collect();
        assert!(nums.len() >= 4, "MediaBox 解不出四个数：{seg}");
        (nums[2] - nums[0], nums[3] - nums[1])
    }

    /// **双页拼版落盘**：媒体框是横的，张数 = 单页数 ÷ 2 上取整。
    ///
    /// 这条把三件事一次钉死：① 纸是横的（不是那版竖着放大铺满的 A3）；
    /// ② 一张纸装两页；③ 单页拼版照旧一页一张纸（老出口没被改动）。
    #[test]
    fn spread_export_makes_landscape_sheets_of_two_pages() {
        // 约 5 页的字量：足够排满两张纸，又还能验证「末张单页」这条边界。
        let text = "天地玄黃宇宙洪荒日月盈昃辰宿列張寒來暑往秋收冬藏閏餘成歲律呂調陽".repeat(8);
        let dir = std::env::temp_dir();
        let single_out = dir.join("jiandu_pdf_single.pdf");
        let spread_out = dir.join("jiandu_pdf_spread.pdf");

        let (one_per_sheet, _, _) = CollationWorkspace::render_pdf_blocking(
            &text,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
            Paper::A3,
            Imposition::Single,
            Binding::Right,
            false,
            &single_out,
        )
        .expect("单页导出应能跑通");
        let (sheets, chars, _) = CollationWorkspace::render_pdf_blocking(
            &text,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
            Paper::A3,
            Imposition::Spread,
            Binding::Right,
            false,
            &spread_out,
        )
        .expect("双页拼版导出应能跑通");

        assert!(one_per_sheet >= 2, "样张字量不够：{one_per_sheet} 页");
        // 一个字都不许丢：落字数必须等于清洗后的字数（切字 / 丢字都会露馅）。
        assert_eq!(chars, 256, "拼版导出掉字了（应为 256 字）：{chars}");
        assert_eq!(
            sheets,
            (one_per_sheet + 1) / 2,
            "拼版张数应为单页数的一半个（上取整）：{sheets} vs {one_per_sheet}"
        );
        assert!(chars > 0);

        let single_bytes = std::fs::read(&single_out).expect("单页 PDF 应已落盘");
        let spread_bytes = std::fs::read(&spread_out).expect("拼版 PDF 应已落盘");
        let (sw, sh) = media_box(&spread_bytes);
        assert!(
            sw > sh,
            "拼版媒体框必须是横的（宽 > 高）：{sw}×{sh}"
        );
        assert!(
            (sw - Paper::A3.sheet_pdf_geom(Imposition::Spread).w).abs() < 2.0,
            "拼版媒体框宽度不是整张 A3：{sw}"
        );
        let (pw, ph) = media_box(&single_bytes);
        assert!(ph > pw, "單頁 A3 仍是竖放：{pw}×{ph}");

        let _ = std::fs::remove_file(&single_out);
        let _ = std::fs::remove_file(&spread_out);
    }

    /// 生成 A3 样张（人工目检用，默认跳过）：单页与拼版各一份。
    ///
    /// 跑法：`JIANDU_GEN_A3_SAMPLE=1 cargo test -p jiandu --bins collation -- --ignored`。
    /// 产物路径写在日志里，用「预览」或 `sips` 打开即可看版式（标题 / 正文 / 版框 /
    /// 中缝）。**拼版那份要亲眼确认**：奇左偶右、两页各自成版、中间中缝。
    #[test]
    #[ignore]
    fn dump_a3_sample_pdf() {
        if std::env::var("JIANDU_GEN_A3_SAMPLE").is_err() {
            return;
        }
        // 字数要够排满两页，才能看出「两页并一张纸」与中缝。
        let body = "# 千字文\n\n**天地玄黃**，宇宙洪荒。日月盈昃，辰宿列張。寒來暑往，秋收冬藏。\n\n## 第一節 四時\n\n閏餘成歲，律呂調陽。雲騰致雨，露結為霜。金生麗水，玉出崑岡。劍號巨闕，珠稱夜光。果珍李柰，菜重芥薑。海鹹河淡，鱗潛羽翔。龍師火帝，鳥官人皇。始制文字，乃服衣裳。推位讓國，有虞陶唐。弔民伐罪，周發殷湯。坐朝問道，垂拱平章。愛育黎首，臣伏戎羌。遐邇一體，率賓歸王。鳴鳳在竹，白駒食場。化被草木，賴及萬方。";
        let text = format!("{body}\n\n{body}");
        for (imp, name) in [
            (Imposition::Single, "jiandu_a3_sample.pdf"),
            (Imposition::Spread, "jiandu_a3_spread.pdf"),
        ] {
            let out = std::env::temp_dir().join(name);
            let (n, chars, _) = CollationWorkspace::render_pdf_blocking(
                &text,
                12,
                ShareBg::Xuan,
                ShareFrame::Red,
                Paper::A3,
                imp,
                Binding::Right,
                false,
                &out,
            )
            .expect("样张生成失败");
            eprintln!("[a3-sample/{imp:?}] {n} 张 / {chars} 字 → {}", out.display());
        }
    }

    /// 【样张】把 **UI 预览那条路**（`render_preview_pages`）真跑一遍并出图。
    ///
    /// 与 `dump_a3_sample_pdf` 互补：那条验 PDF，这条验屏幕。两者共用
    /// `draw_card` / `spread_slots`，所以这条同时也是「预览会不会和印出来
    /// 不一样」的回归。开关：`JIANDU_GEN_PREVIEW_SAMPLE=1 … -- --ignored`。
    #[test]
    fn dump_spread_preview_png() {
        if std::env::var("JIANDU_GEN_PREVIEW_SAMPLE").is_err() {
            return;
        }
        let body = "# 千字文\n\n**天地玄黃**，宇宙洪荒。日月盈昃，辰宿列張。寒來暑往，秋收冬藏。\n\n## 第一節 四時\n\n閏餘成歲，律呂調陽。雲騰致雨，露結為霜。金生麗水，玉出崑岡。劍號巨闕，珠稱夜光。果珍李柰，菜重芥薑。海鹹河淡，鱗潛羽翔。龍師火帝，鳥官人皇。始制文字，乃服衣裳。推位讓國，有虞陶唐。弔民伐罪，周發殷湯。坐朝問道，垂拱平章。愛育黎首，臣伏戎羌。遐邇一體，率賓歸王。鳴鳳在竹，白駒食場。化被草木，賴及萬方。";
        let text = format!("{body}\n\n{body}");
        let (pages, note) = CollationWorkspace::render_preview_pages(
            &text,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
            Paper::A3,
            Imposition::Spread,
            Binding::Right,
            false,
        )
        .expect("预览出图失败");
        eprintln!("[preview-sample] {} 张 {note:?}", pages.len());
        for p in pages {
            eprintln!("  {}", p.display());
        }
    }

    /// **端到端**：Markdown 源码 → 清洗 → A3 排版 → 矢量 PDF 落盘。
    ///
    /// 这是三件事缝在一起的验收：① 源码里的 `#` / `**` 不进纸面；
    /// ② 排的是 A3（尺寸由 `Paper` 算）；③ 产物是真 PDF。
    #[test]
    fn markdown_goes_all_the_way_to_an_a3_pdf() {
        let text = "# 千字文\n\n**天地玄黃**，宇宙洪荒。日月盈昃，辰宿列張。";
        let out = std::env::temp_dir().join("jiandu_collation_export_test.pdf");
        let r = CollationWorkspace::render_pdf_blocking(
            text,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
            Paper::A3,
            Imposition::Single,
            Binding::Right,
            false,
            &out,
        );
        let (pages, chars, cut) = r.expect("MD → A3 PDF 应能跑通");
        assert_eq!(pages, 1, "这么短的稿子不该分成多页");
        assert!(!cut, "一页的稿子不该被页数闸门截断");
        assert!(chars >= 16, "落字太少（清洗把正文吃掉了？）：{chars}");

        let bytes = std::fs::read(&out).expect("PDF 应已落盘");
        assert!(bytes.starts_with(b"%PDF"), "产物不是 PDF");
        assert!(bytes.len() > 2000, "PDF 过小：{}", bytes.len());
        let _ = std::fs::remove_file(&out);
    }

    /// **关闭优先**：预览关掉以后，在途的后台排版结果一律丢弃，绝不回写。
    ///
    /// 用户 2026-11-01 第 5 条：「先把 UI 卡死问题解决，确保【关闭】按钮随时
    /// 能点」。机制就三件事，这条钉住最关键的一件——
    /// ・排版全在 `background_executor`（主线程只 set 状态文字）；
    /// ・`close_preview` 是几个 O(1) 赋值（不删盘、不等后台）；
    /// ・结果回来先过本判据，过期即连临时文件一起丢。
    #[test]
    fn closing_the_preview_always_discards_in_flight_results() {
        // 在途任务代际 7、预览开着 → 结果是有效的。
        assert!(!CollationWorkspace::preview_result_stale(true, 7, 7));
        // 用户按了关闭：show_preview=false 且 gen 7→8 → 必丢。
        assert!(CollationWorkspace::preview_result_stale(false, 7, 8));
        // 只关不加代际也必须丢（口径不依赖 gen 是否变）。
        assert!(CollationWorkspace::preview_result_stale(false, 7, 7));
        // 关掉又重开、但代际已变 → 旧的那批仍然必须丢（绝不「闪回上一版」）。
        assert!(CollationWorkspace::preview_result_stale(true, 7, 8));
    }

    // ── v1.9.65 · 纯文本模式滚动（用户「绝对强制」铁律 ④）────────────────
    //
    // 背景：真机纯文本模式「彻底滑不动」。根因不在滚轮事件（事件链已逐行
    // 审计过 gpui 0.2.2 源码：div.rs `paint_scroll_listener` 正常收 Bubble
    // 事件、`ime::InputField` 只注册焦点 / IME 不碰滚轮），而在**视口高度
    // 塌陷**——`scroll_view.h_full()` 是百分比高度，父级高度由 flex 解析时
    // taffy 可能判「不定」→ 视口 == 内容高 → `clamp_scroll_position` 算出
    // `scroll_max = 0` → offset 永远钉死 0。修法是外层绑绝对像素高、内部
    // 全 flex_1 + min_h(0)、零百分比。GPUI 0.2.2 无法在无窗口的 cargo test
    // 里真开一扇窗验证渲染，故把滚动几何抽成纯函数，按用户铁律单测：
    // **10000 行数字若撑不破视口 / 无滚动余量，测试不过。**

    /// 纯文本模式滚动几何（纯函数）：给定正文与视口宽高，返回
    /// （内容高， 视口高， 最大滚动距离）。
    ///
    /// 与 `render_editor` 同一套常量（`CHAR_W` 近似字宽 / `LINE_H` 行高 /
    /// `ED_PAD` 上下内边距），编码两条铁律：
    ///   ① **视口高恒等于传入值**——视口绝不被内容自适应撑大；
    ///   ② 内容高出视口的部分就是滚动余量，必须 > 0 才滚得动。
    fn pure_scroll_geometry(text: &str, view_w: f32, view_h: f32) -> (f32, f32, f32) {
        let cpl = ((view_w - 2.0 * ED_PAD) / CHAR_W).max(1.0).floor() as usize;
        let lines: usize = text.split('\n').map(|s| est_wrapped_lines(s, cpl)).sum();
        let content_h = lines as f32 * LINE_H + 2.0 * ED_PAD;
        let scroll_max = (content_h - view_h).max(0.0);
        (content_h, view_h, scroll_max)
    }

    /// **用户铁律 ④**：10000 行数字，撑不破视口 / 不出滚动余量 → 测试不过。
    #[test]
    fn ten_thousand_lines_must_overflow_any_screen() {
        // 纯数字循环生成 10000 行（用户点名的口径：极简、纯数字）。
        let text = (0..10_000)
            .map(|i| format!("{:05} 0123456789", i))
            .collect::<Vec<_>>()
            .join("\n");
        // 从笔记本（1200×800）到 5K 大屏（4480×2400）的右栏视口，全部必须被撑破。
        for &(w, h) in &[(1200.0, 800.0), (2560.0, 1440.0), (4480.0, 2400.0)] {
            let (content_h, view_h, scroll_max) = pure_scroll_geometry(&text, w, h);
            assert!(
                content_h > view_h,
                "{w}×{h}：10000 行内容高 {content_h:.0}px 必须大于视口高 {view_h:.0}px"
            );
            assert!(
                scroll_max > 0.0,
                "{w}×{h}：必须存在滚动余量（现为 {scroll_max:.0}px）——没有余量 = 滚不动"
            );
            // 铁律 ① 的编码：视口高恒等于传入值，绝不被内容撑大。
            assert_eq!(view_h, h);
        }
    }

    /// 铁律 ③ 的公式自检：与 gpui 内建（div.rs:2446 `offset += delta`，
    /// div.rs:1754-1758 clamp 到 `[-max, 0]`）逐字同一条数学。
    #[test]
    fn wheel_forward_math_mirrors_gpui_builtin() {
        let max = 3000.0f32;
        let clamp = |y: f32| y.clamp(-max, 0.0);
        let mut y = 0.0f32;
        // 向下滚 10 次 × 52px：内容上移 520px，没到下界。
        for _ in 0..10 {
            y = clamp(y + (-52.0));
        }
        assert_eq!(y, -520.0);
        // 滚到底再滚：钉死在下界，绝不过冲。
        for _ in 0..100 {
            y = clamp(y + (-52.0));
        }
        assert_eq!(y, -max);
        // 向上滚回顶：钉死在 0，绝不越过。
        for _ in 0..100 {
            y = clamp(y + 52.0);
        }
        assert_eq!(y, 0.0);
    }

    // ── v1.9.67 · 点击落点 / 拖选 / 选区行盒（真机「只读 + 一整段大色块」）──
    //
    // 这三条都是纯函数级的：命中换算、折行切段、行盒几何。屏幕上的点击与单测
    // 走**同一个** `caret_at`，所以这些断言就是真机行为的合同。

    /// 折行后点第 2、3 个**视觉行**，必须落到对应逻辑行的对应列。
    ///
    /// 旧实现把 y ÷ 行高当逻辑行号：`abcdefghij`（10 字 / 每行 5 字）占 2 个视觉行，
    /// 点它的第二行会被算成「第 2 个逻辑行」= `klm` 的段首——错 5 个字起步；
    /// 长文里就是「点一下，光标跑到几十行外」。
    #[test]
    fn caret_at_counts_wrapped_visual_lines() {
        let text = "abcdefghij\nklm";
        let cpl = 5;
        // 第 1 视觉行第 2 列 → 2。
        assert_eq!(caret_at(text, cpl, CHAR_W, CHAR_W * 2.0 + 1.0, 1.0), 2);
        // 第 2 视觉行第 1 列 → 5 + 1 = 6（仍是第一段，不是第二段）。
        assert_eq!(caret_at(text, cpl, CHAR_W, CHAR_W * 1.0 + 1.0, LINE_H + 1.0), 6);
        // 第 3 视觉行（第二段第一行）第 0 列 → 段首 11。
        assert_eq!(caret_at(text, cpl, CHAR_W, 1.0, LINE_H * 2.0 + 1.0), 11);
        // 更下方 → 文末，绝不越界。
        assert_eq!(
            caret_at(text, cpl, CHAR_W, 1.0, LINE_H * 9.0),
            text.chars().count()
        );
    }

    /// **点击落点取最近的字缝**（v1.9.68）：左半 → 前一个字缝，右半 → 后一个字缝。
    ///
    /// 这是「无法精准插入到两个字之间」的直接合同：旧实现用 `floor`，
    /// 点在某个字的右半边也会插到它**前面**，手感上总是差一格。
    #[test]
    fn caret_at_snaps_to_nearest_gap() {
        let adv = 17.0f32;
        // 第 0 字左半（x = 3）→ 缝 0；右半（x = 14）→ 缝 1。
        assert_eq!(caret_at("一二三四五六七八", 8, adv, 3.0, 1.0), 0);
        assert_eq!(caret_at("一二三四五六七八", 8, adv, 14.0, 1.0), 1);
        // 第 3 个字的中线两侧：字身占 [51, 68)，中线 59.5。
        assert_eq!(caret_at("一二三四五六七八", 8, adv, 3.0 * adv - 1.0, 1.0), 3);
        assert_eq!(caret_at("一二三四五六七八", 8, adv, 3.0 * adv + 9.0, 1.0), 4);
        // 实测进距与近似值不同时，**以传入的 adv 为准**（这正是精度来源）。
        assert_eq!(caret_at("一二三四五六七八", 8, 20.0, 19.0, 1.0), 1);
        assert_eq!(caret_at("一二三四五六七八", 8, 20.0, 21.0, 1.0), 1);
        assert_eq!(caret_at("一二三四五六七八", 8, 20.0, 11.0, 1.0), 1);
        assert_eq!(caret_at("一二三四五六七八", 8, 20.0, 9.0, 1.0), 0);
    }

    /// 点在行尾右侧的空白：夹到**本行**行尾，不许串到下一行。
    #[test]
    fn caret_at_clamps_to_line_end() {
        let text = "abc\ndefg";
        assert_eq!(caret_at(text, 5, CHAR_W, CHAR_W * 20.0, 1.0), 3);
        assert_eq!(caret_at(text, 5, CHAR_W, CHAR_W * 20.0, LINE_H + 1.0), 8);
    }

    /// 空文 / 空行不 panic，落点恒为 0。
    #[test]
    fn caret_at_handles_empty_text() {
        assert_eq!(caret_at("", 8, CHAR_W, 100.0, 100.0), 0);
        assert_eq!(caret_at("\n\n", 8, CHAR_W, 100.0, LINE_H * 1.5), 1);
    }

    /// 选区按软换行切段：12 个字、每行 5 字 → 三段，最后一段只有 2 个字。
    ///
    /// 整行画一块就会把折行后的空白也涂上，真机看着就是「段落 Padding 变成大色块」；
    /// 切段后每块宽度严格等于「这一段真正有字的字数 × 字宽」。
    #[test]
    fn selection_spans_split_on_wrap() {
        assert_eq!(visual_spans(0, 12, 5), vec![(0, 0, 5), (1, 5, 10), (2, 10, 12)]);
        // 空选区不画块（否则每个空行都变成一条色带）。
        assert!(visual_spans(3, 3, 5).is_empty());
        // 区间反着给也不出错。
        assert_eq!(visual_spans(6, 1, 5), vec![(0, 1, 5), (1, 5, 6)]);
    }

    /// 选区 / 光标的**行盒**必须窄于行高，且完整落在行内。
    ///
    /// 这是用户「Inline」那条口径的编码：色带 = 字身高，上下各留一点行距，
    /// 绝不把行距与段落留白一起涂掉。
    #[test]
    fn selection_box_stays_inline() {
        let box_h = (FONT_SIZE * 1.42).min(LINE_H);
        let box_top = (LINE_H - box_h) * 0.5;
        assert!(box_h < LINE_H, "行盒 {box_h} 必须窄于行高 {LINE_H}");
        assert!(box_top > 0.0, "行盒必须上下留出行距，不能贴边");
        assert!(box_top + box_h <= LINE_H + 0.001, "行盒不许溢出本行");
    }

    // ── v1.9.66 · 撤销 / 重做（用户点名的「保命底线」）──────────────────────
    //
    // 这里全部按**纯函数**验：`push_edit` 管「记与合并」，`splice_chars` 管
    // 「反向应用」。两者合起来就是 ⌘Z / ⌘⇧Z 的全部数学，不必开窗口也能钉死行为。

    /// 连打字必须合并成**一步**：否则按一次 ⌘Z 只退一个字，用户会以为撤销坏了。
    #[test]
    fn consecutive_typing_merges_into_one_undo_step() {
        let mut stack: Vec<EditOp> = Vec::new();
        let now = std::time::Instant::now();
        let mut text = String::new();
        let mut cursor = 0usize;
        for ch in "天地玄黄".chars() {
            push_edit(
                &mut stack,
                cursor,
                String::new(),
                ch.to_string(),
                EditKind::Insert,
                now,
                true,
            );
            text.push(ch);
            cursor += 1;
        }
        assert_eq!(stack.len(), 1, "连打四个字应只记一步");
        let op = stack.pop().unwrap();
        assert_eq!(op.inserted, "天地玄黄");
        let (after, caret) = splice_chars(&text, op.pos, op.inserted.chars().count(), &op.removed);
        assert_eq!(after, "", "一步 ⌘Z 应把四个字全部退掉");
        assert_eq!(caret, 0);
    }

    /// 连按退格的合并方向与打字相反（新删的在**前**），且一步退干净。
    #[test]
    fn consecutive_backspace_merges_into_one_undo_step() {
        let mut stack: Vec<EditOp> = Vec::new();
        let now = std::time::Instant::now();
        let mut text = "天地玄黄".to_string();
        let mut cursor = 4usize;
        while cursor > 0 {
            let chars: Vec<char> = text.chars().collect();
            let removed = chars[cursor - 1].to_string();
            let (t, c) = splice_chars(&text, cursor - 1, 1, "");
            text = t;
            cursor = c;
            push_edit(
                &mut stack,
                cursor,
                removed,
                String::new(),
                EditKind::Backspace,
                now,
                true,
            );
        }
        assert_eq!(stack.len(), 1, "连按四次退格应只记一步");
        let op = stack.pop().unwrap();
        assert_eq!(op.pos, 0);
        assert_eq!(op.removed, "天地玄黄");
        let (after, caret) = splice_chars(&text, op.pos, op.inserted.chars().count(), &op.removed);
        assert_eq!(after, "天地玄黄", "一步 ⌘Z 应把四个字整段插回");
        assert_eq!(caret, 4);
    }

    /// 撤销 → 重做必须回到原样（重做用的是同一条 `splice_chars`，只是方向相反）。
    #[test]
    fn undo_then_redo_restores_exact_text() {
        let original = "资治通鉴卷第一";
        let mut stack: Vec<EditOp> = Vec::new();
        let now = std::time::Instant::now();
        // 在第 2 个字后插入「周纪」。
        push_edit(
            &mut stack,
            2,
            String::new(),
            "周纪".to_string(),
            EditKind::Once,
            now,
            true,
        );
        let edited = splice_chars(original, 2, 0, "周纪").0;
        assert_eq!(edited, "资治周纪通鉴卷第一");

        // ⌘Z
        let op = stack.pop().unwrap();
        let (back, _) = splice_chars(&edited, op.pos, op.inserted.chars().count(), &op.removed);
        assert_eq!(back, original);

        // ⌘⇧Z
        let (again, caret) =
            splice_chars(&back, op.pos, op.removed.chars().count(), &op.inserted);
        assert_eq!(again, edited);
        assert_eq!(caret, 4);
    }

    /// 输入法逐帧组字（n → ni → nih → niha → nihao）只算**一次输入**。
    #[test]
    fn ime_composition_merges_into_one_step() {
        let mut stack: Vec<EditOp> = Vec::new();
        let now = std::time::Instant::now();
        let mut composed = String::new();
        for s in ["n", "ni", "nih", "niha", "nihao"] {
            let removed = std::mem::take(&mut composed);
            push_edit(
                &mut stack,
                0,
                removed,
                s.to_string(),
                EditKind::Ime,
                now,
                true,
            );
            composed = s.to_string();
        }
        assert_eq!(stack.len(), 1, "逐帧组字应合并成一次输入");
        assert_eq!(stack[0].inserted, "nihao");
        assert_eq!(stack[0].removed, "", "组字前的原文是空的，撤销不该删到别的字");
        let (after, _) = splice_chars(&composed, 0, 5, "");
        assert_eq!(after, "");
    }

    /// 位置越界（历史栈与当前文本不同步时的极端情况）绝不许 panic——越界只夹紧。
    #[test]
    fn splice_chars_clamps_out_of_range_positions() {
        assert_eq!(splice_chars("天地", 99, 3, "玄").0, "天地玄");
        assert_eq!(splice_chars("", 5, 2, "黄").0, "黄");
        assert_eq!(splice_chars("天地玄黄", 1, 99, "宇").0, "天宇");
    }

    /// 不同类别的相邻编辑**不许**合并（否则「打字 + 删除」会被并成一步）。
    #[test]
    fn different_kinds_never_merge() {
        let mut stack: Vec<EditOp> = Vec::new();
        let now = std::time::Instant::now();
        push_edit(&mut stack, 0, String::new(), "天".into(), EditKind::Insert, now, true);
        push_edit(&mut stack, 1, "天".into(), String::new(), EditKind::Backspace, now, true);
        assert_eq!(stack.len(), 2);
    }

    /// 光标挪过之后（`undo_barrier`）即使几何位置正好接上，也必须另起一步。
    #[test]
    fn barrier_forces_a_new_step_despite_geometry() {
        let mut stack: Vec<EditOp> = Vec::new();
        let now = std::time::Instant::now();
        push_edit(&mut stack, 0, String::new(), "天".into(), EditKind::Insert, now, true);
        // 挪回开头再打「地」：位置上正好落在上一段的起点，纯几何判据会吸收它。
        push_edit(&mut stack, 0, String::new(), "地".into(), EditKind::Insert, now, false);
        assert_eq!(stack.len(), 2, "断点之后必须是独立的一步");
    }
}


/// **快捷标点栏**的纯计算单测（v1.9.25）。
///
/// 这些用例把「点一枚符号到底改了什么」钉死成可执行的断言——没有它们，
/// 这套交互只能靠开窗口点鼠标验，而**光标落点差一格**正是用户当场就会骂的
/// 那种缺陷。三条分支各覆盖：无选区、有选区、越界与 CJK 边界。
#[cfg(test)]
mod punct_bar_tests {
    use super::*;

    // ── 掩注（⌘N）────────────────────────────────────────────────────
    #[test]
    fn strip_gloss_removes_full_and_half_width_parentheses() {
        assert_eq!(strip_gloss("天地（玄黃）宇宙"), "天地宇宙");
        assert_eq!(strip_gloss("天地(玄黃)宇宙"), "天地宇宙");
    }

    #[test]
    fn strip_gloss_handles_nested_and_unclosed() {
        // 嵌套：一剥到底
        assert_eq!(strip_gloss("甲（乙（丙）丁）戊"), "甲戊");
        // 未闭合：到文末为止，绝不把后文吞成乱码
        assert_eq!(strip_gloss("甲（乙丙"), "甲");
        // 空串 / 无注：原样
        assert_eq!(strip_gloss(""), "");
        assert_eq!(strip_gloss("天地玄黃"), "天地玄黃");
    }

    #[test]
    fn strip_gloss_removes_bracket_notes() {
        assert_eq!(strip_gloss("天地[注:玄黃]宇宙"), "天地宇宙");
        assert_eq!(strip_gloss("天地[註：玄黃]宇宙"), "天地宇宙");
        // 普通方括号不动（快捷标点栏就靠它包注文）
        assert_eq!(strip_gloss("天地[玄黃]"), "天地[玄黃]");
    }

    #[test]
    fn strip_gloss_counts_cjk_by_char_not_byte() {
        // 「（玄）」是多字节字符：按字节切会切进字中间（panic 或出乱码）。
        let out = strip_gloss("猛（měng）虎");
        assert_eq!(out, "猛虎");
        assert_eq!(out.chars().count(), 2);
    }

    #[test]
    fn wrap_without_selection_puts_caret_between_symbols() {
        // 「甲乙」光标在 1（甲 | 乙）→ 插一对空括号，光标落在括号中间。
        let (text, cursor) = wrap_text("甲乙", None, 1, "（", "）");
        assert_eq!(text, "甲（）乙");
        assert_eq!(cursor, 2, "光标必须落在两个符号之间，下一步才写得了注文");
    }

    #[test]
    fn wrap_with_selection_puts_caret_after_closing_symbol() {
        // 选中「乙丙」，用方括号包住 → 光标停在 `]` 之后。
        let (text, cursor) = wrap_text("甲乙丙丁", Some(1..3), 3, "[", "]");
        assert_eq!(text, "甲[乙丙]丁");
        assert_eq!(cursor, 5, "包裹后光标应落在闭合符号之后");
    }

    #[test]
    fn wrap_ignores_empty_or_reversed_selection() {
        // 空选区（start == end）等同无选区：插一对，光标在中间。
        let (text, cursor) = wrap_text("甲乙", Some(1..1), 1, "（", "）");
        assert_eq!(text, "甲（）乙");
        assert_eq!(cursor, 2);
        // 反向拖拽（start > end）也必须正确包裹，不能 panic。
        let (text, cursor) = wrap_text("甲乙丙", Some(2..0), 2, "[", "]");
        assert_eq!(text, "[甲乙]丙");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn wrap_clamps_out_of_range_cursor() {
        // 光标越界（滚动 / 删字后残留的旧下标）不许 panic，按文末处理。
        let (text, cursor) = wrap_text("甲乙", None, 99, "（", "）");
        assert_eq!(text, "甲乙（）");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn wrap_selection_counts_cjk_by_char_not_byte() {
        // 「天地玄黃」四字 12 字节：按下标 1..3 必须切出「地玄」两个字，
        // 而不是切进某个字的第 2 个字节（v1.9.18 那次 panic 的同款坑）。
        let (text, cursor) = wrap_text("天地玄黃", Some(1..3), 3, "（", "）");
        assert_eq!(text, "天（地玄）黃");
        assert_eq!(cursor, 5);
        assert_eq!(text.chars().count(), 6);
    }
}

/// **康熙部首折叠**的回归（v1.9.23）。
#[cfg(test)]
mod radical_fold_tests {
    use super::*;

    /// 折叠表必须是**标准 214 字**，且逐个落在通用汉字区（不是自己瞎编的）。
    #[test]
    fn kangxi_table_is_the_standard_214() {
        assert_eq!(KANGXI_RADICALS.chars().count(), 214);
        assert_eq!(KANGXI_FOLD.len(), 214);
        for c in KANGXI_FOLD.iter() {
            let cp = *c as u32;
            assert!(
                (0x4E00..=0x9FFF).contains(&cp),
                "{:?} (U+{:04X}) 不是通用汉字",
                c,
                cp
            );
        }
        // 首尾对齐：U+2F00 一、U+2FD5 龠。
        assert_eq!(fold_radicals('\u{2F00}'), '一');
        assert_eq!(fold_radicals('\u{2FD5}'), '龠');
    }

    /// 真机抓到的那三个字：⾐(U+2F90) ⼥(U+2F25) ⼰(U+2F30) 必须折成 衣 女 己。
    #[test]
    fn observed_radicals_fold_to_their_hanzi() {
        assert_eq!(fold_radicals('\u{2F90}'), '衣');
        assert_eq!(fold_radicals('\u{2F25}'), '女');
        assert_eq!(fold_radicals('\u{2F30}'), '己');
        assert_eq!(fold_radicals('\u{2E9F}'), '母');
        assert_eq!(fold_radicals('\u{2EF3}'), '龟');
        // 本来就是汉字的、标点的、拉丁的，一律原样。
        for c in ['衣', '女', '己', '一', '，', 'a', '龠'] {
            assert_eq!(fold_radicals(c), c);
        }
    }

    /// 两侧文本一个用部首、一个用汉字，`match_text` 之后必须**一模一样**。
    #[test]
    fn match_text_folds_both_sides() {
        assert_eq!(match_text("乃服⾐裳"), match_text("乃服衣裳"));
        assert_eq!(match_text("⼥慕贞洁"), match_text("女慕贞洁"));
        // 幂等：已经是汉字再折一次不变。
        let once = match_text("衣裳");
        assert_eq!(match_text(&once), once);
    }

    /// 原图区域整条都是部首字形时，右文（正常汉字）仍然要能定位到它。
    #[test]
    fn locate_region_survives_kangxi_region_text() {
        let regions = vec![
            SourceRegion {
                x: 0.0,
                y: 0.0,
                w: 24.0,
                h: 400.0,
                text: "⼥慕贞洁男效才良知过必改得能莫忘".to_string(),
            },
            SourceRegion {
                x: 30.0,
                y: 0.0,
                w: 24.0,
                h: 400.0,
                text: "罔谈彼短靡恃⼰长信使可覆器欲难量".to_string(),
            },
        ];
        let (ri, n) = locate_region("靡恃己长，信使可覆", &regions).expect("应当命中部首字形");
        assert_eq!(ri, 1, "该落在第二列");
        assert!(n >= 6, "连续重合 {n} 字");
    }
}

/// **栅格化画布尺寸**的护栏（v1.9.30）。
///
/// 这四条断言盯的是一件事：**别再把 MediaBox 的「点」当像素**。影印本内嵌
/// 300dpi 扫描，按点建画布等于先降到 72dpi 再让 Retina 放大回去——糊。
#[cfg(all(test, target_os = "macos"))]
mod raster_px_tests {
    use super::*;

    /// 影印本的典型尺寸：595×892 点、内嵌扫描。
    const SCAN: (f64, f64) = (595.0, 892.0);

    /// 左栏在 MacBook 上约 1120 设备像素宽 → 画布就该是 1120 宽（**升采样**）。
    ///
    /// 旧实现这里给的是 595，一比一放大到 1120 就糊了。
    #[test]
    fn scan_is_rasterized_at_display_pixels_not_pdf_points() {
        let (w, h) = raster_px(SCAN.0, SCAN.1, 1120.0);
        assert_eq!(w, 1120, "画布宽应等于显示设备像素宽");
        // 高按 MediaBox 比例走：892/595 ≈ 1.499
        let expect_h = (1120.0 * SCAN.1 / SCAN.0).ceil() as usize;
        assert!((h as i64 - expect_h as i64).abs() <= 1, "画布高 {h} 应约 {expect_h}");
    }

    /// **只升不降**：目标宽比页面点还窄（窄窗口 / 1x 屏）时，仍按 1:1。
    ///
    /// 缩小省的那点内存，赔的是「大图看着锐」——而且下次窗口一拉宽还得重渲一遍。
    #[test]
    fn narrow_target_never_downscales_below_native_points() {
        let (w, h) = raster_px(SCAN.0, SCAN.1, 300.0);
        assert_eq!((w, h), (595, 892), "1:1 是下限，不是上限");
    }

    /// 还不知道显示尺寸（首帧 `left_px = 0`）时退回 1:1，绝不产出空画布。
    #[test]
    fn unknown_target_falls_back_to_one_to_one() {
        assert_eq!(raster_px(SCAN.0, SCAN.1, 0.0), (595, 892));
        assert_eq!(raster_px(SCAN.0, SCAN.1, f32::NAN), (595, 892));
    }

    /// 超大海报页必须被两条硬上限压住——分配失败是**闪退**，不是返回 nil。
    #[test]
    fn huge_page_is_held_by_hard_caps() {
        let (w, h) = raster_px(20000.0, 30000.0, 4000.0);
        assert!(
            w.max(h) as f64 <= MAX_RASTER_DIM,
            "长边 {} 不得超过 {}",
            w.max(h),
            MAX_RASTER_DIM
        );
        assert!(
            (w * h * 4) as f64 <= MAX_RASTER_BYTES,
            "{}×{} 的内存 {} 字节超过上限",
            w,
            h,
            w * h * 4
        );
    }
}

/// **颜色通道序**的护栏（v1.9.30）。
///
/// 这一条盯的是「肉眼很难发现、但一定错」的那类 bug：`CGBitmapContextCreate` 的
/// `bitmap_info` 少写 `kCGBitmapByteOrder32Big` 时，内存里是 **BGRA**，而我们把
/// 它当 RGBA 用——纸会变成偏蓝、朱砂会变成青色，但因为整张图「色调整体一致地
/// 错了」，在书页上极不容易被看出来。用纯红整页把它钉死。
#[cfg(all(test, target_os = "macos"))]
mod raster_color_tests {
    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};

    extern "C" {
        // ⚠️ 签名必须与 `share_paint.rs` 里那四条**逐字一致**：同一枚 C 符号
        // 在两处写成两套类型，编译器会报「redeclared with a different
        // signature」，链接器却照旧按一个符号走——真出事是最难查的那种账。
        fn CGPDFContextCreateWithURL(
            url: core_foundation::url::CFURLRef,
            media: *const CGRect,
            aux: core_foundation::base::CFTypeRef,
        ) -> core_graphics::sys::CGContextRef;
        fn CGPDFContextBeginPage(
            ctx: core_graphics::sys::CGContextRef,
            info: core_foundation::base::CFTypeRef,
        );
        fn CGPDFContextEndPage(ctx: core_graphics::sys::CGContextRef);
        fn CGPDFContextClose(ctx: core_graphics::sys::CGContextRef);
    }

    /// 纯红整页 → 栅格化后第一个像素必须是 (255, 0, 0, 255)。
    #[test]
    fn rasterized_page_keeps_channel_order() {
        let path = std::env::temp_dir().join("jiandu_raster_red.pdf");
        let _ = std::fs::remove_file(&path);
        let url = CFURL::from_path(&path, false).expect("路径");
        let media = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(200.0, 200.0));
        let raw = unsafe {
            CGPDFContextCreateWithURL(url.as_concrete_TypeRef(), &media, std::ptr::null())
        };
        assert!(!raw.is_null(), "建 PDF 上下文失败");
        let ctx = unsafe { CGContext::from_existing_context_ptr(raw) };
        unsafe { CGPDFContextBeginPage(raw, std::ptr::null()) };
        ctx.set_rgb_fill_color(1.0, 0.0, 0.0, 1.0);
        ctx.fill_rect(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(200.0, 200.0),
        ));
        unsafe { CGPDFContextEndPage(raw) };
        unsafe { CGPDFContextClose(raw) };
        drop(ctx);

        // 目标 256px → 升采样到 256×256（顺带验了「只升不降」在真 PDF 上生效）。
        let r = rasterize_source(&path, 1, 256.0).expect("栅格化失败");
        let _ = std::fs::remove_file(&path);
        let img = r.image.expect("PDF 页应走内存位图，不再产出 PNG");
        assert_eq!(img.size(0).width.0, 256, "画布宽应等于目标像素");
        let bytes = img.as_bytes(0).expect("应当有像素");
        // 断**中心**像素：`CGPDFPageGetDrawingTransform` 会按比例居中，画布首行
        // 偶尔落在留白上（0,0,0,0），拿首像素断会误报。
        let mid = (256 * 128 + 128) * 4;
        assert_eq!(
            &bytes[mid..mid + 4],
            &[255, 0, 0, 255],
            "颜色通道序被调换（期望 RGBA 纯红，实得 {:?}）——检查 bitmap_info 的 ByteOrder32Big",
            &bytes[mid..mid + 4]
        );
        // 通道调换的签名：红变蓝。整页不该出现一枚纯蓝像素。
        let blue = bytes
            .chunks_exact(4)
            .filter(|p| p[0] == 0 && p[1] == 0 && p[2] == 255 && p[3] == 255)
            .count();
        assert_eq!(blue, 0, "出现 {blue} 枚纯蓝像素：R/B 被调换了");
    }
}

/// **真实影印本栅格化代价**探针（默认跳过，只在做性能决策时手动跑）。
///
/// 跑法：
/// ```text
/// JIANDU_PROBE_PDF=~/Downloads/xx.pdf \
///   cargo test -p jiandu --bins probe_real_scan_raster_cost -- --ignored --nocapture
/// ```
/// 它会打印两种口径（`target=0` 按 MediaBox 点 1:1＝老行为；`target=1120`
/// 按左栏设备像素＝新行为）的耗时与画布尺寸，并单独量一次 PNG 编码耗时——
/// 那个数就是 v1.9.30 砍掉的那一趟（编 PNG + 写盘 + 由 img 读盘解码）。
#[cfg(all(test, target_os = "macos"))]
mod raster_probe {
    use super::*;
    use std::time::Instant;

    /// `/Rotate 90` 的页，栅格化出来的「自然尺寸」必须是**宽高互换**后的 400×300，
    /// 位图本身也得横过来（宽 > 高），比例约 4:3。
    ///
    /// 为什么这条非有不可：`rasterize_source` 的 `nat_w/nat_h` 同时是**坐标映射
    /// 的基准**（`source_size()` → 框的位置 = region / nat × 显示宽）。若还按
    /// MediaBox 的 300×400 算，`GetDrawingTransform` 会把页面等比缩成一条横带
    /// 塞进竖画布（上下大片留白），而且与 `pdf_text` 里旋转过的 regions 差一次
    /// 旋转——用户看到的就是「图歪着、框飞到页边」。
    #[test]
    fn rotated_page_rasterizes_into_a_swapped_canvas() {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("jiandu_rot_raster_{}.pdf", std::process::id()));
        std::fs::write(&p, crate::pdf_text::tiny_pdf(90)).expect("写样张失败");
        let r = rasterize_source(&p, 1, 1200.0).expect("栅格化失败");
        let _ = std::fs::remove_file(&p);
        assert!(
            (r.nat_w - 400.0).abs() < 0.5,
            "自然宽应为 400（= MediaBox 的高），实得 {}",
            r.nat_w
        );
        assert!(
            (r.nat_h - 300.0).abs() < 0.5,
            "自然高应为 300（= MediaBox 的宽），实得 {}",
            r.nat_h
        );
        let img = r.image.expect("PDF 页应是内存位图");
        let (pw, ph) = (
            img.size(0).width.0 as f64,
            img.size(0).height.0 as f64,
        );
        assert!(pw > ph, "位图应横过来（宽>高），实得 {pw}×{ph}");
        let ratio = pw / ph;
        assert!(
            (ratio - 4.0 / 3.0).abs() < 0.05,
            "宽高比应约 1.333（400:300），实得 {ratio:.3}——画布可能还在按未旋转尺寸建"
        );
    }

    #[test]
    #[ignore]
    fn probe_real_scan_raster_cost() {
        let Ok(p) = std::env::var("JIANDU_PROBE_PDF") else {
            return;
        };
        let path = PathBuf::from(p);
        for target in [0.0_f32, 1120.0_f32] {
            let mut best = f64::MAX;
            let mut dims = (0usize, 0usize);
            let mut raw: Vec<u8> = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                let r = rasterize_source(&path, 1, target).expect("栅格化失败");
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let img = r.image.expect("应是内存位图");
                dims = (
                    img.size(0).width.0 as usize,
                    img.size(0).height.0 as usize,
                );
                raw = img.as_bytes(0).unwrap_or(&[]).to_vec();
                if ms < best {
                    best = ms;
                }
            }
            // 旧路径还要把它编成 PNG 落盘、再由 gpui 读盘解码；两趟都单独量，
            // 那个和就是 v1.9.30 真正砍掉的代价。
            let out = std::env::temp_dir().join("jiandu_probe_raster.png");
            let t = Instant::now();
            let _ = image::save_buffer(
                &out,
                &raw,
                dims.0 as u32,
                dims.1 as u32,
                image::ColorType::Rgba8,
            );
            let enc = t.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            let _ = image::open(&out);
            let dec = t.elapsed().as_secs_f64() * 1000.0;
            let _ = std::fs::remove_file(&out);
            println!(
                "target={target:>6.0}  画布={}×{}  栅格化={best:>6.1}ms  已砍编解码={:.1}ms  现在总={best:.1}ms  原总={:.1}ms",
                dims.0,
                dims.1,
                enc + dec,
                best + enc + dec
            );
        }
        // 文本层抽取：v1.9.31 从主线程挪到后台。它是翻页路上的第二笔固定开销，
        // 以前那 10–40ms 是直接压在输入响应上的（翻页时按 → 会「粘手」）。
        let mut best_scan = f64::MAX;
        let mut n_regions = 0usize;
        for _ in 0..3 {
            let t = Instant::now();
            let r = crate::pdf_text::extract_pdf_text_regions_on(&path, 0);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            n_regions = r.len();
            if ms < best_scan {
                best_scan = ms;
            }
        }
        println!(
            "文本层抽取={best_scan:>6.1}ms  regions={n_regions}（v1.9.31 起在后台，与栅格化同一趟）"
        );
    }
}

#[cfg(test)]
mod pdf_cache_rules {
    use super::*;

    /// 淘汰要挑「离当前页最远」的，不是最久没用的。
    ///
    /// 这条口径直接决定手感：LRU 会在「第 1 页 ↔ 第 2 页来回对读」时反复把
    /// 第 1 页踢掉，每一次回翻都重新栅格化；按距离淘汰则两页都稳稳热着。
    #[test]
    fn cache_evicts_the_farthest_page_not_the_oldest() {
        let v = victim_of([1usize, 2, 3, 4, 5].into_iter(), 2).expect("5 个条目必然有牺牲者");
        assert_eq!(v, 5, "离第 2 页最远的是第 5 页");
        assert_ne!(v, 1, "刚离开的第 1 页最可能被翻回来，不能先踢");
        assert_ne!(v, 3, "相邻页是来回对照的另一半，不能踢");
    }

    /// 只剩当前页自己时不许删它——删了左图会直接空掉。
    #[test]
    fn cache_never_evicts_the_current_page() {
        assert_eq!(victim_of([7usize].into_iter(), 7), None);
        assert_eq!(victim_of(std::iter::empty(), 7), None);
    }

    /// 当前页不管在哪个位置都不牺牲（含它恰好最远这种构造场景）。
    #[test]
    fn current_page_survives_from_any_position() {
        for cur in 1..=5 {
            let v = victim_of([1usize, 2, 3, 4, 5].into_iter(), cur)
                .expect("5 个条目必然有牺牲者");
            assert_ne!(v, cur, "当前页第 {cur} 页被淘汰了");
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod vector_pdf_regression {
    //! **端到端回归**：我们自己导出的矢量 PDF → 文本层 → 「点哪高亮哪」。
    //!
    //! 这条是用户明确要求的「拿真实样张跑一遍」。它同时钉住三件事：
    //! ① 自产 PDF 的文本层能抽全（`PDFSelection` 路径，不是半页空矩形）；
    //! ② 抽出来的框是**一列字**（细长），不是 6×6 的退化小方块；
    //! ③ 右文一行能在这些区域里被 `locate_region` 找到，且找到的那一列
    //!    确实是这一行的一部分（框不会落在不相干的字上）。

    use super::*;

    /// 千字文一段，单段不换行——避免一列跨两段、「子串」判据失真。
    const SAMPLE: &str = "天地玄黄 宇宙洪荒 日月盈昃 辰宿列张 寒来暑往 秋收冬藏 闰余成岁 律吕调阳 云腾致雨 露结为霜 金生丽水 玉出昆冈 剑号巨阙 珠称夜光 果珍李柰 菜重芥姜 海咸河淡 鳞潜羽翔 龙师火帝 鸟官人皇 始制文字 乃服衣裳 推位让国 有虞陶唐";

    fn sample_pdf() -> Vec<SourceRegion> {
        // ⚠️ 每个调用必须用**自己的**临时文件：cargo test 默认多线程，几条用例
        // 曾共用一个固定文件名，于是 A 写→B 抽→A 删 三者交错，抽到空 regions，
        // 表现为「偶发失败、单跑不挂」。v1.9.31 给 PDFKit 加了串行锁后撞得更
        // 频繁，才把这个老问题顶出来。pid + 自增序号保证跨进程、跨线程都唯一。
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jiandu_collation_roundtrip_{}_{}.pdf",
            std::process::id(),
            n
        ));
        CollationWorkspace::render_pdf_blocking(
            SAMPLE,
            12,
            ShareBg::Xuan,
            ShareFrame::Red,
            Paper::A4,
            Imposition::Single,
            Binding::Right,
            false,
            &path,
        )
        .expect("样张 PDF 生成失败");
        let regions = crate::pdf_text::extract_pdf_text_regions(&path);
        let _ = std::fs::remove_file(&path);
        regions
    }

    #[test]
    fn own_vector_pdf_yields_column_shaped_regions() {
        let regions = sample_pdf();
        assert!(!regions.is_empty(), "自产矢量 PDF 必须抽得出文本层");
        // 一列至少十几个字 → 高度几十上百点；宽度 ≈ 一个字。
        let tall = regions.iter().filter(|r| r.h > 40.0).count();
        assert!(
            tall >= 2,
            "抽出 {} 个区域，其中只有 {} 个是「一列」（其余可能是退化小方块）",
            regions.len(),
            tall
        );
        for r in regions.iter().filter(|r| r.h > 40.0) {
            assert!(r.w < 60.0, "列宽 {} 太胖：把相邻两列并进来了", r.w);
        }
    }

    #[test]
    fn own_vector_pdf_can_be_highlighted_by_right_text() {
        let regions = sample_pdf();
        // 排障：自产 PDF 的文本层到底抽出了什么字（ToUnicode 一旦串码，
        // 这里就是证据）。留一行，回归时一眼看出是「坐标错」还是「字错」。
        for r in regions.iter().take(6) {
            println!("[区域 {:.0}×{:.0}] {:?}", r.w, r.h, r.text);
        }
        // 右文第一段的开头（正常汉字，不带标点）。
        let line = "天地玄黄宇宙洪荒日月盈昃辰宿列张";
        let (ri, n) = locate_region(line, &regions).expect("在我们自己的 PDF 上应当定位得到");
        assert!(n >= 6, "连续重合 {n} 字，太少（判据是 ≥ {LCS_MIN}）");
        let hay = match_text(&regions[ri].text);
        let needle = match_text(line);
        assert!(!hay.is_empty(), "命中的区域文本为空");
        assert!(
            needle.contains(&hay),
            "命中的那一列 {:?} 不是这一行的子串——框会落在不相干的字上",
            hay
        );
    }

    /// **端到端：框住整页 → 拼出来的文字必须与版式源文本一致，且顺序正确。**
    ///
    /// 这是「左图框选 → 补入右文」的最后一道验收：前面几条守的是坐标与匹配，
    /// 这条守的是**读出来的字本身**。自产 PDF 的源文本（`SAMPLE`）是已知的，
    /// 于是可以断言「拼出来的就是它」。
    ///
    /// 特别守「方向」这一条：若列序写成左→右，拼出来会以最后一列（`推位让国`）
    /// 开头，`starts_with("天地")` 立刻红。
    #[test]
    fn picking_the_whole_page_reproduces_the_source_text() {
        let regions = sample_pdf();
        assert!(!regions.is_empty(), "自产矢量 PDF 必须抽得出文本层");
        let whole = crate::pdf_text::PickRect::new(-10.0, -10.0, 10_000.0, 10_000.0);
        let idx = crate::pdf_text::pick_indices(&regions, whole);
        assert_eq!(idx.len(), regions.len(), "整页框选应当命中全部区域");
        let picked = crate::pdf_text::join_picked(&regions, &idx);

        let src: String = SAMPLE.chars().filter(|c| !c.is_whitespace()).collect();
        println!("[框选拼出 {} 字] {picked}", picked.chars().count());
        assert!(
            picked.starts_with("天地"),
            "竖排应从右上第一列读起（「天地玄黄…」），实得 {:?}",
            picked.chars().take(12).collect::<String>()
        );
        // 正文必须**逐字一致**（顺序、丢字、多字、错字都靠这句守）。
        //
        // 末尾允许多出极短的尾巴：样张自己在版框外落了一处「笺校」（书口 / 落款），
        // 它在文本层里是真字，框住整页时理应被框进来——真实影印本的叶码、卷次
        // 也一样。**刻意不做「版框外就丢」的过滤**：眉批、夹注正是生在版框外的，
        // 那才是校对要补的东西，为一行落款把它们一起滤掉是拿原稿换整洁。
        assert!(
            picked.starts_with(&src),
            "框选拼出来的正文与版式源文本不一致（顺序 / 丢字 / 多字）\n实得：{picked}"
        );
        let tail: String = picked.chars().skip(src.chars().count()).collect();
        assert!(
            tail.trim_start_matches('\n').chars().count() <= 4,
            "正文之后多出太多字（{tail:?}）——有区域被读重了"
        );
    }
}
