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
    div, img, px, rgb, rgba, Bounds, Context,
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
/// 编辑器是「一行一个 GPUI 元素」的自绘文本框，几十万字一次性铺开会让首帧
/// 布局与滚动全部变慢（不是崩，是拖）。故按字数截断并明确告知——宁可只校对
/// 前 8 万字，也不让点一下导入就把界面拖住。
const TEXT_MAX_CHARS: usize = 80_000;
/// 导入文本的**硬上限**：超过直接拒绝，绝不把用户内存撑爆。
///
/// 产品边界（用户 2026-11-01 定）：
/// ・**不做「大于 8MB 就不让进」**——大部头古籍扫描件转出来的纯文本动辄十几
///   MB，一刀切拒绝等于「我的书导入不了」。8MB 只是**经验上的提示量级**，真正的
///   判据是字数：超过 [`TEXT_MAX_CHARS`] 就截断，并明确告诉用户「只载入了前 N 字」。
/// ・真正的闸门放在这里：64MB。再大就不是「一本书」了（多半是选错了文件，
///   或者是数据库 / 日志），读进来只会把内存和界面一起拖死。
/// ・读盘按 `take(HARD + 1)` **流式**读，绝不先信 `metadata` 再整读——网络盘 /
///   iCloud 的 size 可能是 0 或说谎，信它就会一次性 read 出一个 2GB 的 Buffer。
const TEXT_IMPORT_HARD_BYTES: u64 = 64 * 1024 * 1024;

/// 右文段 ↔ 原图区域文字的**最低字面对合度**（0.0–1.0）。
/// 段落索引对齐的前提是「右侧转录 ≈ 原图该区域文字」。用户实测过一类典型误用：
/// 导入《資治通鑑》却在右侧贴《千字文》——两者毫不相干，索引对齐失去意义，画出的
/// 框必然是错的。故联动前先做字面校验，低于此值就**绝不画框、绝不滚动**。
const MATCH_MIN_SIM: f32 = 0.5;

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
fn match_text(raw: &str) -> String {
    crate::share_layout::clean_md(raw)
        .text()
        .chars()
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
    /// 待删临时 PNG：关闭预览 / 重排时把旧路径挪进来，**下一帧** render 开头统一删盘。
    ///
    /// 不当场删是因为本帧的 `img` 可能还在异步读该文件；隔一帧元素树已换，删了安全。
    temp_gc: Vec<PathBuf>,
    /// 当前这批预览图对应的版式指纹（`preview_sig`）；相同请求直接跳过不重排。
    preview_sig: Option<(u64, usize, u8, u8, u8, u8, u8)>,
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
    /// 当前联动的右文段索引（0-based）。None = 尚未联动。右文第 i 段 ↔
    /// `regions[i]`（转录逐行对齐原图行）。
    active_para_idx: Option<usize>,
    /// 左图原图自然像素宽（由栅格化结果填充；无原图时由区域包围盒推导）。
    src_nat_w: f32,
    /// 左图原图自然像素高（由栅格化结果填充；无原图时由区域包围盒推导）。
    src_nat_h: f32,
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
}

impl CollationWorkspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let me = Self {
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
            active_para_idx: None,
            src_nat_w: 0.0,
            src_nat_h: 0.0,
            match_note: None,
            frame_count: 0,
            regions: Self::mock_regions(),
            pdf_path: None,
            pdf_page: 1,
            pdf_pages: 1,
            page_busy: false,
            page_gen: 0,
            refocus: false,
            text_file: None,
            text_note: None,
            text_err: false,
        };
        // 启动即清扫上一进程遗留的临时图（崩溃 / 强退时 `Drop` 跑不到，只能等下次
        // 启动补扫）。只认本应用自己的命名前缀，且只删「够旧」的，绝不误伤另开的
        // 一个笺读实例正在用的图。
        sweep_stale_temp_pngs();
        me
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
        // 点进编辑器 = 用户想打字。这里走硬重建（而不是裸 focus）：原生对话框
        // 关掉后平台侧可能已经丢了输入上下文，而 GPUI 仍以为聚焦（见 hard_refocus）。
        Self::hard_refocus(window, &self.focus);
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
                    )
                    // PDF 多页时浮出页码条（左下角，绝不遮住右上角的【打开文件】）
                    .child(
                        div()
                            .absolute()
                            .bottom(px(8.0))
                            .left(px(8.0))
                            .child(self.render_pdf_pager(cx)),
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
                    //
                    // ⚠️ 产品边界（用户 2026-11-01 定）：扫描版 PDF **只渲染、不报错、
                    //    不崩溃**。这里走的就是这条路——`regions` 清空（没有任何坐标），
                    //    `image_error` 显式置 None（**不是**错误），底部状态栏挂一条
                    //    「需 OCR」的提示（见 `image_note` 的渲染处），底图照常栅格化
                    //    显示、照常翻页、照常导出。用户看到的是「这本书能看，只是没法
                    //    自动定位」，而不是一个红叉。
                    let real = crate::pdf_text::extract_pdf_text_regions(&path);
                    if !real.is_empty() {
                        this.regions = real;
                        this.image_error = None;
                        this.image_note = None;
                    } else {
                        this.regions.clear();
                        this.image_error = None; // 绝不把「无文本层」当错误报
                        this.image_note = Some(
                            "该原图无文本层（扫描图），只作底图显示，无法坐标映射——需 OCR"
                                .to_string(),
                        );
                    }
                    cx.notify();
                });
            }

            // 2) 最耗时的栅格化（CG 绘制该页 / 读 PNG 尺寸）放到后台 executor，
            //    避免阻塞 UI；完成后回主线程写入 image_path 等字段。
            let bg = app.background_executor().clone();
            let raster_path = path.clone();
            let result = bg.spawn(async move { rasterize_source(&raster_path, 1) }).await;

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
                            // 多页翻页的凭据：只有真 PDF 才记路径（位图原图翻不动）。
                            let is_pdf = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|s| s.eq_ignore_ascii_case("pdf"))
                                .unwrap_or(false);
                            this.pdf_path = if is_pdf { Some(path.clone()) } else { None };
                            this.pdf_pages = r.total.max(1);
                            this.pdf_page = r.page;
                            // 换原图 = 之前联动的段 / 框全部作废（属于旧图坐标）。
                            this.active_para_idx = None;
                            this.match_note = None;
                            this.left_scroll
                                .set_offset(Point::new(px(0.0), px(0.0)));
                            this.image_error = None;
                            // 若此前因无文本层已设 OCR 提示则保留；否则用栅格化 note。
                            if this.image_note.is_none() {
                                this.image_note = r.note;
                            }
                            // 原生对话框刚关 → 强制重建输入焦点（见 `refocus`）。
                            this.refocus = true;
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
                                    this.text = body;
                                    this.cursor = 0;
                                    this.selection = None;
                                    this.marked = None;
                                    // 联动状态属于旧稿，随新稿一起清空，绝不带着
                                    // 上一本书的高亮框进下一本。
                                    this.active_para_idx = None;
                                    this.match_note = None;
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
        path: &Path,
    ) -> Result<(usize, usize, bool), String> {
        let cleaned = clean_md(text);
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
                                &text, columns, bg, frame, paper, imp, binding, &path_bg,
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
    ) -> Result<(Vec<PathBuf>, Option<String>), String> {
        let t0 = std::time::Instant::now();
        // **Markdown 清洗**（预览解析）：编辑器里存的是源码（`#`、`**`、`---`
        // 原样可见），出图前才洗成「干净文字 + 标题/加粗装饰位」。清洗器在
        // `share_layout`（纯函数、零 macOS API），故本步同样可在后台线程跑。
        // 它同时吃掉换行与空行：竖排是连续流，`\n` 只会白占一格。
        let cleaned = clean_md(text);
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
    /// 与 `open_image_picker` 里首次导入走同一条 `rasterize_source`，区别只在
    /// 「页号」与「旧图回收」：翻页必须删掉上一页的临时 PNG，否则长翻几百页
    /// 会在临时目录里堆出几百张 1080p 图。
    fn request_pdf_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(path) = self.pdf_path.clone() else {
            return;
        };
        self.page_busy = true;
        self.page_gen += 1;
        let gen = self.page_gen;
        // 换页即作废旧页的文本层坐标与联动状态：右边的段落还没点，左边不该
        // 留着上一页的框。
        self.active_para_idx = None;
        self.match_note = None;

        cx.spawn(
            move |this: gpui::WeakEntity<CollationWorkspace>, app: &mut gpui::AsyncApp| {
                let mut app = app.clone();
                async move {
                    let exec = app.background_executor().clone();
                    let out = exec
                        .spawn(async move { rasterize_source(&path, page) })
                        .await;
                    if let Some(entity) = this.upgrade() {
                        let _ = entity.update(&mut app, |this, cx| {
                            this.page_busy = false;
                            // 代际过期（用户又翻了几页）→ 丢弃结果与临时图。
                            if gen != this.page_gen {
                                if let Ok(r) = &out {
                                    if let Some(t) = &r.temp {
                                        let _ = std::fs::remove_file(t);
                                    }
                                }
                                cx.notify();
                                return;
                            }
                            match out {
                                Ok(r) => {
                                    if let Some(old) = this.last_temp.take() {
                                        let _ = std::fs::remove_file(&old);
                                    }
                                    this.image_path = Some(r.path);
                                    this.last_temp = r.temp;
                                    this.src_nat_w = r.nat_w as f32;
                                    this.src_nat_h = r.nat_h as f32;
                                    this.pdf_page = r.page;
                                    this.pdf_pages = r.total.max(1);
                                    // 新页的文本层坐标：翻页后仍要能「点右文 → 亮左图」，
                                    // 若沿用上一页的框，高亮会落到完全不相干的位置。
                                    let real =
                                        crate::pdf_text::extract_pdf_text_regions_on(
                                            this.pdf_path.as_deref().unwrap_or(Path::new("")),
                                            r.page.saturating_sub(1),
                                        );
                                    this.regions = real;
                                    // 滚动复位：新页从顶上看起（旧的偏移对不上新页）。
                                    this.left_scroll
                                        .set_offset(Point::new(px(0.0), px(0.0)));
                                    this.image_error = None;
                                    this.image_note = r.note;
                                }
                                Err(e) => {
                                    this.image_error = Some(e);
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
    ) -> (u64, usize, u8, u8, u8, u8, u8) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        // 拼版 / 装订也进指纹：换拼版就是换版式，绝不能拿旧图充数。
        (h.finish(), columns, bg as u8, frame as u8, paper as u8, imp as u8, binding as u8)
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
                                &text, columns, bg, frame, paper, imp, binding,
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

    /// 右文某段 → 左图联动：记录段索引 + 左侧滚动到对应区域。
    ///
    /// 真实坐标映射（替换 N*BAND_H Mock）：右文第 `para_idx` 段 ↔ `regions[para_idx]`
    /// （转录逐行对齐原图行）。区域 (x,y,w,h) 为原图自然像素，按 `src_nat` 缩放后
    /// 既是高亮矩形（由 `render_left_content` 渲染）也是滚动目标。无对应区域
    /// （段落数 ≠ 区域数）则不高亮也不滚动。
    fn link_to_paragraph(&mut self, para_idx: usize, vw: f32, cx: &mut Context<Self>) {
        // 无对应区域（段落数 ≠ 区域数 / 原图无文本层）→ 不高亮、不滚动、不标记。
        let Some(r) = self.regions.get(para_idx) else {
            self.active_para_idx = None;
            // 一条区域都没有 = 扫描版（无文本层）。这类原图**只渲染、不报错**，但也
            // 确实没有坐标可映射——必须把「点了为什么没反应」当面讲清楚（产品边界 ①）。
            self.match_note = if self.regions.is_empty() && self.image_path.is_some() {
                Some("该原图无文本层（扫描图）：无法定位到原图，需先 OCR 出文本层".to_string())
            } else {
                None
            };
            cx.notify();
            return;
        };
        // —— 兜底容错：文本对合度校验 ——
        // 索引对齐只在「右侧转录 ≈ 原图该区域文字」时才有意义。内容不相干（如导入
        // 《資治通鑑》却贴《千字文》）时，按索引取到的区域必然对不上，画框只会误导
        // 校对。故先校验字面重合，过低就跳过定位（不高亮、不滚动），并明确提示。
        let sim = bigram_overlap(&self.paragraph_text(para_idx), &r.text);
        if sim < MATCH_MIN_SIM {
            self.active_para_idx = None;
            self.match_note = Some(format!(
                "该段与原图此处文字不匹配（字对重合 {:.0}% < {}%），已跳过定位",
                sim * 100.0,
                (MATCH_MIN_SIM * 100.0) as u32
            ));
            cx.notify();
            return;
        }
        self.active_para_idx = Some(para_idx);
        self.match_note = None;
        let (nw, _nh) = self.source_size();
        let disp_w = vw * 0.4;
        let scale = if nw > 0.0 { disp_w / nw } else { 1.0 };
        // 滚动使区域顶部进入视野（向下偏移取负）。
        self.left_scroll
            .set_offset(Point::new(px(0.0), px(-(r.y * scale))));
        cx.notify();
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
                .px(px(PAD))
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
                .px(px(PAD))
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
                    // 【导入 MD】：把 .md / .txt 的**源码**灌进右侧编辑器。
                    // 与【打开文件】一样是 rfd 同步对话框，但读盘走后台（见
                    // `open_text_picker`），点下去界面立刻回来，不会假死。
                    .child(self.button("导入 MD", |c, _, _, cx| c.open_text_picker(cx), cx)
                        .mr(px(8.0)))
                    .child(self.button("竖排预览", |c, _, _, cx| {
                        if c.show_preview {
                            c.close_preview();
                        } else {
                            c.show_preview = true;
                            c.request_preview(cx);
                        }
                        cx.notify();
                    }, cx)),
            )
            .child(match self.render_text_status() {
                Some(s) => s,
                None => div().h(px(0.0)),
            })
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
                // **GPUI 原生「点一下就聚焦」**（v1.22.1）。
                //
                // 真机反馈：导入 MD 之后点进右侧编辑器，连光标都不给，更别说打字。
                // 手写的 `hard_refocus`（blur + focus）在「窗口还不是 key window」
                // 时只会改 GPUI 的内部账本，平台侧 first responder 根本没交回来。
                // `track_focus` 走的是 GPUI 自己的鼠标派发：命中即 `window.focus()`，
                // 与平台侧「点谁谁成为 first responder」同一时刻发生，不依赖任何
                // 手写补偿。两条路并存，任一条通就能打字。
                .track_focus(&self.focus)
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
    /// 预览浮层的键盘总入口（挂在根 div 上，仅在 `show_preview` 时生效）。
    ///
    /// ←/→ 翻页（**左右双屏一起翻**，见 [`CollationWorkspace::flip_page`]）；
    /// Esc 关闭浮层。预览关闭时直接返回，绝不干扰编辑器自身的方向键光标移动。
    fn preview_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.show_preview {
            return;
        }
        match event.keystroke.key.as_str() {
            "left" => self.flip_page(-1, cx),
            "right" => self.flip_page(1, cx),
            "escape" => {
                self.close_preview();
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
        // 排障：帧计数。静止时也狂刷 → 说明有无限重绘循环（脏标记漏了）。
        self.frame_count += 1;
        if self.frame_count % 30 == 0 {
            eprintln!(
                "[render] frames={} preview={} busy={}",
                self.frame_count, self.show_preview, self.preview_busy
            );
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
            root = root.child(self.render_preview_overlay(vh, cx));
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
                "已载入「{}」前 {} 字（原文 {} 字）：编辑器逐行渲染，超长会拖慢界面",
                name, TEXT_MAX_CHARS, total
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
struct Rasterized {
    path: PathBuf,
    temp: Option<PathBuf>,
    nat_w: f64,
    nat_h: f64,
    /// 本次渲出的是第几页（1-based；位图原图恒为 1）。
    page: usize,
    /// 总共多少页（位图原图恒为 1）。
    total: usize,
    note: Option<String>,
}

#[cfg(target_os = "macos")]
fn rasterize_source(path: &Path, page_no: usize) -> Result<Rasterized, String> {
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
        path: out.clone(),
        temp: Some(out),
        nat_w,
        nat_h,
        page: page_no,
        total,
        note,
    })
}

#[cfg(not(target_os = "macos"))]
fn rasterize_source(_path: &Path, _page_no: usize) -> Result<Rasterized, String> {
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

    /// 两条线各自的位置钉死：**64 MB 硬闸门**（不是 8 MB 一刀切）+ 8 万字截断线。
    ///
    /// 8 万字按汉字 3 字节算只有 240 KB，硬闸门必须远高于截断线——否则十几 MB 的
    /// 大部头连「截断到 8 万字」的机会都没有就被拒了，等于「我的书导入不了」。
    #[test]
    fn product_limits_are_the_agreed_ones() {
        assert_eq!(TEXT_IMPORT_HARD_BYTES, 64 * 1024 * 1024);
        assert_eq!(TEXT_MAX_CHARS, 80_000);
        assert!(
            TEXT_IMPORT_HARD_BYTES > (TEXT_MAX_CHARS as u64) * 3 * 10,
            "硬闸门必须远高于截断线，否则截断逻辑永远跑不到"
        );
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
}
