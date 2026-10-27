//! 应用根视图：书架 / 封面 / 阅读 三态。
//!
//! 启动先进「书架」，点书进「封面」，再进「阅读」。
//! 每本书的阅读档案（第几面、模式、字体、标注开关）会即时写回 library.json。

use crate::fonts;
use crate::grid::{GridConfig, GridDir, LayoutConfig};
use crate::library::{Bookmark, Library};
use crate::theme::{InkPalette, Theme};
use crate::types::{
    Book, Duplex, FontChoice, FontPreset, Glyph, GlyphHit, GlyphKey, GlyphKind,
    MarkKind, Note, NoteKind, NoteSource, RuleColor, Script,
};
use gpui::{
    div, px, rgb, AsyncApp, Context, Div, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, ScrollHandle, SharedString, Styled,
    WeakEntity, Window,
};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

/// **渲染帧 → 事件层的一次性矩形探针**（x, y, w, h）。
///
/// GPUI 的 `canvas` 不是交互元素，画完就走；凡是要「拿这一帧画在哪」去做
/// 命中判断的（天头眉批带 `margin_bands`、纸面 `page_rect`），都在 prepaint
/// 里往这枚 `Cell` 回填一次。渲染是输出、事件是输入，中间靠它过河。
pub type RectProbe = Rc<Cell<Option<(f32, f32, f32, f32)>>>;

/// 字书浮签的**来源页签**（v1.42.0）：康熙 / 說文，一次只摊一部。
///
/// 浮签里不再两段并列——一次全摊会占掉半屏正文，且康熙动辄数百字，
/// 说到文那段就被挤成一行半。改成分页：顶部两枚小签点一下换一部。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DictTab {
    /// 《康熙字典》：历代韵书引文、反切、考證。
    Kangxi,
    /// 《說文解字》：小篆本义。
    Shuowen,
    /// 简明释义：高频常用字的现代语体一句话概括（之乎者也为首的手写小表）。
    #[default]
    Jianming,
}

impl DictTab {
    /// 页签上的两个字（繁体，与字书一致）。
    pub fn label(self) -> &'static str {
        match self {
            DictTab::Kangxi => "康熙",
            DictTab::Shuowen => "說文",
            DictTab::Jianming => "簡明",
        }
    }

}

/// 当前所在界面。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    /// 书架（书封墙）。
    Shelf,
    /// 单本书的封面（书衣）。
    ///
    /// v1.37.0 UI 瘦身把阅读页工具栏那枚「封面」撤了，故当前**没有入口**
    /// 走到这一屏；`shelf::render_cover` 仍在（书架侧随时可接回一个入口），
    /// 所以保留变体并豁免 dead_code，而不是把整页书衣删掉。
    #[allow(dead_code)]
    Cover,
    /// 正文阅读。
    Reading,
}

/// 书架视图：题签网格（默认）/ 书脊列表。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShelfView {
    /// 一格一签：竖排书名居纸中，下缀四项小字。
    TitleSlip,
    /// 一行一册：左侧书脊色条 + 竖排书名，右侧四项横排。
    Spine,
}

impl ShelfView {
    pub fn label(self) -> &'static str {
        match self {
            Self::TitleSlip => "题签",
            Self::Spine => "书脊",
        }
    }
    pub fn toggled(self) -> Self {
        match self {
            Self::TitleSlip => Self::Spine,
            Self::Spine => Self::TitleSlip,
        }
    }
}

/// 书架排序。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShelfSort {
    /// 最近读过（没读过的按入架顺序垫底）。
    Recent,
    /// 书名（按字序）。
    Title,
    /// 阅读进度（读得多的在前）。
    Progress,
    /// 入架顺序。
    Added,
}

impl ShelfSort {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "最近阅读",
            Self::Title => "书名",
            Self::Progress => "进度",
            Self::Added => "入架顺序",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Recent => Self::Title,
            Self::Title => Self::Progress,
            Self::Progress => Self::Added,
            Self::Added => Self::Recent,
        }
    }
}

/// 顶层 UI 状态机（v1.45.0 交互重构 · 任务一）。
///
/// 同一时刻只允许一个「顶层面板」打开（版字 / 检索 / 朱批 / 帮助 / 目录 /
/// 分享），打开任一面板会强制关闭其它面板与所有底层瞬态浮层（选词工具条、
/// 字义浮签、天头输入框、选区等）。`None` 表示纯阅读态。`ui_state` 是这些
/// 分散布尔开关的权威镜像，所有开合都走 [`Root::set_ui_state`]。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppUIState {
    /// 纯阅读（无面板）。
    None,
    /// 版字（列数 / 字体 / 字级 / 留白 / 纸墨 / 标注）。
    Style,
    /// 全文检索。
    Search,
    /// 朱批（笔记侧栏）。
    Notes,
    /// 帮助。
    Help,
    /// 目录（卷目）侧栏。
    Toc,
    /// 分享卡片浮层。
    Share,
    /// 卡片匣（v1.74.0）：跨书摘录卡。
    Cards,
    /// 书签抽屉。
    Bookmarks,
}

/// PNG 本叶导出的两段式阶段（v1.52.3 起，见 `Root::png_export`）。
///
/// v1.71.0：出图已改**离屏重绘**，不再依赖「屏幕上一帧干净纸面」，但两段式
/// 保留——Warm 帧先把面板收干净并给用户一句「正在印制本叶…」，下一帧才出图
/// 弹保存框，节拍与旧版一致（不抢在事件处理器里做重活）。
///
/// `screencapture` 截的是**屏幕**而不是渲染树：事件处理器里当场截，截到的
/// 还是带着目录 / 浮层的旧帧。故先收面板、登记 `Warm`，等「无面板」纸面
/// 真正合成上屏后的下一帧（`Shoot`）才截——与 PDF 的 `pdf_warm` 同序。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PngStage {
    /// 面板刚收：本帧只负责把干净纸面画出来，不截。
    Warm,
    /// 干净纸面已上屏：本帧截屏 + 弹系统保存框。
    Shoot,
}


/// 把 `catch_unwind` 捕获的 panic 负载转成可读字符串（`&str` / `String` 优先，
/// 其它类型回退为「未知错误」）。导出路径在事件处理器与异步任务两处 FFI 边界
/// 都用它把 panic 降级为界面提示，避免 `panic_cannot_unwind` 强杀进程。
pub(crate) fn panic_payload_to_string(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知错误".to_string()
    }
}

pub struct Root {
    pub library: Library,
    pub screen: Screen,
    /// 当前书在书架中的下标。
    pub current: usize,
    pub book: Option<Book>,
    pub script: Script,
    /// 参数化网格排版配置：列数 / 方向 / 留白（驱动竖排与横排两套渲染）。
    pub grid: GridConfig,
    /// 上次算出的内容区尺寸（渲染层据此决定是否开启单页滚动）。
    pub content_w: f32,
    pub content_h: f32,
    /// 字格宽 / 高（来自网格引擎，渲染与分页共用，不再写死像素）。
    pub col_w: f32,
    pub row_h: f32,
    /// 当前网格总行数（竖排=每列字数；横排=行数）。
    pub grid_rows: usize,
    /// 是否触发了最小字号（需要单页滚动）。
    pub grid_min_font: bool,
    /// 天头预留（内容高的比例推导，单位 px）：竖笺「天头宽、地脚窄」之上缘，
    /// 由网格引擎按 [`crate::grid::ANCIENT_TOP_FRAC`] 算出，渲染层据以铺纸。
    pub margin_top: f32,
    /// 地脚预留（内容高比例 + 页脚注区）：下缘留白，明显窄于天头。
    pub margin_bot: f32,
    /// **版心率**（v1.33.0 留白档）：版心宽 ÷ 单叶可用宽。
    /// 0.60 宽（复古）/ 0.75 中（标准，默认）/ 0.90 窄（满屏）。
    /// 由 `Prefs::margin_tier` 落下，`set_banxin_ratio` 改写并持久化；
    /// 网格引擎据此反推列宽，天头率随档联动（`grid::top_frac_for`）。
    pub banxin_ratio: f32,
    /// 当前版式配置键：数据驱动排版引擎的句柄（见 [`crate::grid::LAYOUT_CONFIGS`]）。
    ///
    /// v1.35.0：它**不是**列数的来源，只是「行款（字级 / 行高比例）」的句柄——
    /// 列数的唯一数据源是 [`Root::columns_pref`]，UI 只写那里、只读那里。
    /// 取代原先散落的 `letter_mode` 布尔——所有版式只是注册表里的一条记录。
    pub layout_key: &'static str,
    pub duplex: Duplex,
    pub font_ancient: FontChoice,
    pub font_modern: FontChoice,
    /// **批注字体**（夹注小字 / 放大注卡专用）：楷体二选一，见
    /// [`FontChoice::NOTE`]。与正文宋体分开，正是刻本「正文匠体、注文手写体」
    /// 的层次；默认玄冬楷书。
    pub font_note: FontChoice,
    /// **排版字体**（设置面板那一行）：刻本宋体 ↔ 手写楷体。
    ///
    /// 它是一对字体的**组合句柄**——切一次同时换正文与批注（见
    /// `set_font_preset`）。字体名本身不在这儿：`fonts::FONT_CONFIGS`
    /// 才是唯一数据源，UI 与渲染都只认角色。
    pub font_preset: FontPreset,
    pub show_traditional: bool,
    pub show_tongjia: bool,
    pub show_notes: bool,
    /// 注文是否以「双行小字夹注」排进正文（false 则退为页脚注）。
    pub notes_inline: bool,
    /// 纸色主题。
    pub theme: Theme,
    pub size_ancient: f32,
    pub size_modern: f32,
    pub page: usize,
    /// 当前页首字所属的章（翻页 / 跳章 / 恢复进度时悄悄对齐，供目录朱砂红标当前卷）。
    pub current_chapter: usize,
    /// 目录面板是否展开（true = 该渲染目录层）。
    pub show_toc: bool,
    /// 底部竖排目录的滚动柄：开目录时用它滚到当前在读的那一卷。
    pub toc_scroll: ScrollHandle,
    /// 是否需要「滚到当前卷」（每次开目录置一次，滚完即清）。
    pub toc_need_scroll: bool,
    /// **后台导入任务**（v1.50.0）：不为空表示「正在刻印」。
    ///
    /// 百万字的丛书解析要跑几秒，绝不能堵在主线程上；进度与结果都放在
    /// `Arc<Mutex<…>>` 里，主线程每帧读一次（见 `poll_import`）。
    pub import_job: Option<ImportJob>,
    /// 目录面板滑入 / 滑出动画进度（0 = 完全收起，1 = 完全展开）。
    pub toc_slide: f32,
    /// 目录动画的目标态（true = 展开）。
    pub toc_target: bool,
    /// 是否正在播放目录滑入 / 滑出动画。
    pub toc_animating: bool,
    /// 书签抽屉是否展开（true = 该渲染书签层）。
    pub show_bookmarks: bool,
    /// 当前这本书的书签（插入 / 跳转 / 删除，落档 `library.json`）。
    pub bookmarks: Vec<Bookmark>,
    /// 书签抽屉滑入 / 滑出动画进度（0 = 完全收起，1 = 完全展开）。
    pub bookmarks_slide: f32,
    /// 书签动画的目标态（true = 展开）。
    pub bookmarks_target: bool,
    /// 是否正在播放书签滑入 / 滑出动画。
    pub bookmarks_animating: bool,
    /// 书签列表滚动柄。
    pub bookmarks_scroll: ScrollHandle,
    /// 「版字」面板是否展开（v1.35.0：样式与版式两页合并为一页，
    /// 面板只剩一张纸，故不再有「当前页签」这个状态）。
    pub show_style: bool,
    /// 专注模式：隐藏工具栏、状态栏与目录，沉浸阅读。
    pub focus_mode: bool,
    /// 顶栏是否已滑出（v1.26.0「沉浸式古风化」）。
    ///
    /// v1.52.1 起顶栏**常驻显示**：用户反馈「找不到工具」，故撤掉「触天头才
    /// 滑出、离开即退」的自动隐藏——`bar_shown` 开局置 `true` 且不再被鼠标
    /// 位置改写（`bar_wants_show` / `BAR_HOT` / `BAR_GAP` 已删）。只有专注
    /// 模式会在渲染层把整块顶栏撤下。
    pub bar_shown: bool,
    /// 字号缩放系数（A-/A+ 调节；配合视口铺满使用）。
    pub zoom: f32,
    /// 点字查字面板当前查询的字（None 表示未打开）。
    pub lookup: Option<String>,
    /// 查字浮签的 **charIndex**：被查那个字的 `(chapter, para, offset)`。
    ///
    /// 长按 / 点字查义时一并记下——浮签上要显示「第几章第几段第几字」，
    /// 以及它在全书流里的统一字符下标（`Book::flat_index`）。
    pub lookup_key: Option<GlyphKey>,
    /// 查字浮签上的**选中文本**：有拖选区取选区全文，单字就取那一个字。
    /// 与 `lookup`（单字）配套——浮签要同时展示「查的是哪个字」和「选了哪段」。
    pub lookup_text: String,
    /// 查字**浮签**的锚点（长按处窗口坐标）。
    ///
    /// `Some` 表示浮签贴着长按点浮起；`None` 表示退化为右上角常驻面板
    /// （工具条「查询」按钮走的就是这条路）。
    pub lookup_at: Option<(f32, f32)>,
    /// 被查那个字**在屏幕上的矩形**（窗口坐标）：浮签据此避让——
    /// 展开方向要让开它，绝不能把查的字压在浮签底下。
    ///
    /// 由长按的**坐标反查**（`glyph_hit_at`）量出来，和 `lookup_at`
    /// （手指落点）不是一回事：落点可能偏在字格边缘，而要让开的是整个字。
    pub lookup_rect: Option<(f32, f32, f32, f32)>,
    /// **悬停查词**（v1.38.0）：鼠标停住 500ms 就浮出字书浮签。
    ///
    /// `hover_key` 记「当前悬停在哪个字上」（charIndex 三元组），
    /// `hover_seq` 是**一次悬停的代号**：鼠标一动就 +1，之前排的那枚定时器
    /// 醒来发现代号对不上便自行作废——这样只有真正停住 500ms 才弹签，
    /// 掠过一排字不会连甩十几张。
    pub hover_key: Option<(usize, usize, usize)>,
    pub hover_seq: usize,
    /// 当前这张字书浮签是**悬停**呼出来的吗？
    ///
    /// 悬停呼出的那张要随指针走：移到别的字就换、离开正文就收；
    /// 长按 / 点字呼出的那张则是「钉住」的，只能点掉。两条路共用一个
    /// `bubble`，故必须记一笔来源，否则悬停会把长按的签顶掉、或长按的签
    /// 把悬停永远堵死（v1.40.0 修）。
    pub hover_card: bool,
    /// 字书浮签停在**哪一源**（v1.42.0）：康熙 / 說文二选一，顶部两枚小页签
    /// 左右切换。
    ///
    /// 一次只显示一部，卡片就窄一半、短一截——长释义不再糊住整片正文。
    /// 命中的那部若是空的（字书未收說文），自动落到另一部，页签不空转。
    ///
    /// 只在会话内记（不落 prefs）：切过的那一部会留着，连查几个字不必
    /// 每次再点一次《說文》。
    pub dict_tab: DictTab,
    /// 当前书的笔记：页序号 -> 正文。
    pub notes: std::collections::HashMap<usize, String>,
    /// 笔记面板是否展开。
    pub show_notes_panel: bool,
    /// 卡片匣侧栏（v1.74.0）：跨书摘录卡的汇集处，与朱批录同为右滑侧栏。
    pub show_cards: bool,
    /// 匣中卡片（开栏时从库里读一次，增删后重读；不每帧查询）。
    pub cards: Vec<jiandu_core::cards::Card>,
    /// 笔记编辑态：true 时正文区弹出浮层编辑栏，键盘全部路由给编辑。
    pub note_editing: bool,
    /// 编辑栏正在改的是**哪条注文**：`Some((chapter, para, offset))` 表示
    /// 改的是书稿里的夹注 / 注释（保存时写回书稿 JSON）；
    /// `None` 表示改的是页眉批注（`notes[page]`，既有行为）。
    pub note_edit_note: Option<(usize, usize, usize)>,
    /// 编辑栏正在**新建**一条用户自加夹注（宿主字键）。
    ///
    /// 与 `note_edit_note` 的区别：那个是「改已有注文」（找不到就不写），
    /// 这个是「在该字上加一条夹注」（没有就注入一条 `NoteSource::User` 的）。
    /// 保存时走 `inject_user_note_at`，注入后正文流长度与所有字的全局下标
    /// 都不变，只需从当前半叶起点局部重排（`relayout_from_halfleaf`）。
    pub note_inject_key: Option<(usize, usize, usize)>,
    /// 笔记编辑栏的当前草稿（未保存的输入缓冲）。
    pub note_draft: String,
    /// 笔记草稿的光标位置（字符下标，0..=len）。
    pub note_cursor: usize,
    /// 笔记框的焦点句柄：中文 IME 只把组好的字送给「有焦点且注册了
    /// handle_input」的元素，没有它笔记框就打不进汉字（见 ime.rs）。
    pub note_focus: FocusHandle,
    /// 笔记框 IME 组字中的标记区间（UTF-16 下标）。
    pub note_marked: Option<std::ops::Range<usize>>,
    /// **天头眉批**：读者点在版框上方那段天头里手写的一列朱砂小字。
    ///
    /// 与 `notes`（页眉批注，按页号存）是两回事：眉批钉在**字**上
    /// （`HeadNote::key` = 该面首字字键），改列数 / 改字号重新分页后仍
    /// 跟着那一面；位置存的是归一化比例，窗口缩放自动跟随。
    pub head_notes: Vec<crate::types::HeadNote>,
    /// **v1.55.0 新制天头眉批**：`anchor_char_index` 绝对锚定（旧 `head_notes`
    /// 的 u/v 坐标系已废弃，仅在读档时被清空，不再参与渲染）。
    pub margin_notes: Vec<crate::types::MarginNote>,
    /// **悬停联动**（v1.55.0）：鼠标停在哪一条眉批上（`MarginNote::id`）。
    ///
    /// 只记 id，不记坐标——坐标每帧由 `anchor_char_index` 反算，故悬停时
    /// 哪怕正在翻页 / 改字号，高亮也钉在被批的那个字上。
    pub margin_hover: Option<u64>,
    /// [`Self::margin_hover`] 那条眉批所锚定的**字形键**。
    ///
    /// 与 `margin_hover` 一并置位：字格渲染只认键（它拿不到眉批表），
    /// 这样「谁被高亮」的判断就是一次元组比较，不必逐帧查书稿。
    pub margin_hover_key: Option<(usize, usize, usize)>,
    /// **正在写的那条眉批锚在全书第几个字**（v1.55.0 新制）。
    ///
    /// 非空即表示「草稿是天头眉批」：⏎ 存进 `margin_notes`
    /// （`anchor_char_index` = 此值），而不是旧 `head_notes` 的 `u/v`。
    /// 判据只认这一枚开关，与旧制互不干扰——旧制走 `head_anchor`。
    pub margin_draft_at: Option<usize>,
    /// 正在**改写**的那条眉批的 id（新写为 `None`）。
    ///
    /// 与 `margin_draft_at` 配对：⏎ 时据此决定是追加一条，还是原位换文
    /// （草稿清空即删，与旧制「改写写空即抹」同一口径）。
    pub margin_edit_id: Option<u64>,
    /// 天头带在窗口里的矩形（**由画布 prepaint 回填**）：右叶 / 左叶各一份。
    ///
    /// 画布本身不收鼠标事件（GPUI 的 `canvas` 不是交互元素），悬停命中要靠
    /// 这一枚探针把「天头带这一帧落在窗口何处」记下来——与 `page_rect`
    /// 同一个套路：渲染是输出，事件是输入，中间用一次绘制把矩形带出来。
    pub margin_bands: [RectProbe; 2],
    /// 正在写的眉批落点 `(key, u, v)`。
    ///
    /// `note_editing` 为 true 且此项为 Some 时，草稿**就地竖排**显示在天头
    /// （而不是弹浮层卡片），⏎ 存成眉批而不是夹注。
    pub head_anchor: Option<((usize, usize, usize), f32, f32)>,
    /// 按下时是否落在天头里（capture 阶段记下归一化 (u, v)，抬起时开写）。
    pub head_press: Option<(f32, f32)>,
    /// v1.48.0 眉批管理：当前**唤醒**的眉批下标——点在天头某条朱砂眉批上
    /// 即置位，该条字柱下方随即挂出一枚极小古风菜单「改 / ✕」；
    /// 点天头空白处起新批时清零。与「新建」互不冲突（防误触的关键）。
    pub head_active: Option<usize>,
    /// v1.48.0 眉批管理：正在**改写**的那条眉批下标（`None` = 新建）。
    /// 非空时 `commit_note_edit` 走「原位换文」而不是 push 新条；
    /// 渲染侧据此把原条暂时收起，避免原文与草稿叠在同一处。
    pub head_edit: Option<usize>,
    /// 浮层笔记框的锚点（窗口坐标）；None 时按视口居中偏上放置。
    pub note_pos: Option<(f32, f32)>,
    /// 刚打开笔记框的这一次点击：抬起时不要立刻「自动保存」把它关掉。
    /// 打开笔记的那一下，mouseup 常会重新命中正文并触发 `on_mouse_up` 里的
    /// 自动提交——空草稿一提交，浮层就「闪开即关」，看起来像做不了笔记。
    /// 置位后由紧随的一次自动提交消费掉（跳过），随后即恢复正常。
    pub note_suppress_commit: bool,
    /// 重命名对话框：正在重命名书架第几本书（None 表示未打开）。
    pub rename_idx: Option<usize>,
    /// 重命名草稿与光标。
    pub rename_draft: String,
    pub rename_cursor: usize,
    /// 重命名输入框的焦点句柄：挂 InputHandler 用。
    /// 中文输入法（IME）不会走 keydown，而是把组好的字经 InputHandler
    /// 送回应用——没有它，重命名框就打不出中文（见 `EntityInputHandler` 实现）。
    pub rename_focus: FocusHandle,
    /// 窗口根元素的焦点句柄。
    ///
    /// **快捷键能不能用全靠它**：GPUI 只把按键派发给「焦点节点 → 根元素」这条
    /// 派发路径上的监听器；`Window::focus_path()` 在窗口无焦点时返回空数组，
    /// 挂在根 div 上的 `on_key_down` 就永远收不到键——整组快捷键（← → 翻页、
    /// ⌘T 繁简、Esc 收浮层……）会静默失效。所以平时必须让焦点落在这个句柄上，
    /// 只在重命名 / 检索这类要接 IME 的输入态才让给对应的输入框。
    pub key_focus: FocusHandle,
    /// IME 组字中的标记区间（**UTF-16 下标**，与 selected/marked 接口一致）；
    /// None 表示当前没有正在组字的文字。
    pub rename_marked: Option<std::ops::Range<usize>>,
    /// 删除二次确认：点过「删除」的书（再点「确认删除」才真删）。
    pub delete_confirm: Option<usize>,
    /// 帮助面板是否展开（快捷键一览）。
    pub show_help: bool,
    /// 已划线的字：(chapter, para, offset)。
    pub underlines: std::collections::HashSet<(usize, usize, usize)>,
    /// 分享卡片浮层是否展开。
    pub show_share_panel: bool,
    /// 分享卡：书名 / 作者 / 章节 / 日期 四个元素是否显示。
    pub share_show_title: bool,
    pub share_show_author: bool,
    pub share_show_chapter: bool,
    pub share_show_date: bool,
    /// 分享卡模板下标（旧制四套配色，v1.56.0 起不再参与渲染）。
    pub share_tpl: usize,
    /// **底色**（v1.56.0）：下标见 [`crate::share::ShareBg::ALL`]。
    pub share_bg: usize,
    /// **版框**（v1.56.0）：下标见 [`crate::share::ShareFrame::ALL`]。
    pub share_frame: usize,
    /// **印章**（v1.56.0）：下标见 [`crate::share::ShareSeal::ALL`]。
    pub share_seal: usize,
    /// 是否连同**朱批 / 眉批**一起分享（关 = 只分享正文）。
    pub share_show_notes: bool,
    /// 快照：正文中每个字是否被划了红线（与 `share_body` 等长）。
    pub share_red: Vec<bool>,
    /// 快照：该段范围内用户写的眉批（已裁切合并）。
    pub share_notes: Vec<String>,
    /// 快照：印文（书名首二字，见 `crate::share::seal_chars`）。
    pub share_seal_text: String,
    /// 分享卡文字方向：竖排右起（复古）/ 横排居中（现代）。
    pub share_dir: crate::share::ShareDir,
    /// 用户是否手动指定过方向：手动后不再随模板的建议方向变。
    pub share_dir_touched: bool,
    /// 分享面板页签：样式 / 字体。
    pub share_tab: crate::share::ShareTab,
    /// 分享卡正文字体：None = 跟随阅读字体（见 `crate::share::SHARE_FONTS`）。
    pub share_font: Option<usize>,
    /// 打开面板时**快照**的正文：有选区取选区，无选区取本叶。
    /// 之后点任何模板 / 页签都只改分享态——阅读界面一动不动。
    pub share_body: Vec<char>,
    /// 快照内容是否来自选区（false = 本叶正文）。
    pub share_from_sel: bool,
    /// **待分享选区**（v1.64.0）：顶层面板（版字 / 目录 / 朱批 …）一开，
    /// `close_transient_ui` 就要清 `sel_anchor` / `sel_focus`——选区随之蒸发，
    /// 「选中 → 版字 → 分享」就只能退化成分享本叶。故在清之前把范围定格
    /// 一份存这里，供分享抽屉继续取用；点纸面 / 翻页 / 换书即作废
    /// （见 [`Root::clear_selection`]）。
    pub share_sel: Option<(GlyphKey, GlyphKey)>,
    /// 快照的元信息：书名 / 作者 / 章节 / 叶号 / 中文日期。
    pub share_book: String,
    pub share_author: String,
    pub share_chap: String,
    pub share_leaf: usize,
    pub share_date: String,
    /// 分享卡在窗口内容区里的矩形 (x, y, w, h)，渲染时写入，
    /// 保存 / 分享图片时据此只截卡片，而不是整块屏幕。
    pub share_card_rect: Option<(f32, f32, f32, f32)>,
    /// 丝栏（竖排列间界行）颜色：乌丝栏 / 金丝栏 / 朱丝栏，版字面板可选。
    pub rule_color: RuleColor,
    /// **当前纸面（整幅双叶）在窗口内容区里的矩形** (x, y, w, h)。
    ///
    /// 由阅读视图里一枚**不绘制任何东西**的测量探针在 prepaint 时写入
    /// （见 `reader::render_screen` 的 `stage` 包装层）。导出 PNG 据此把
    /// 截图范围收得与纸面严丝合缝——不带上状态栏，也不裁掉半个版框。
    /// 它是渲染帧的**输出**而非输入，故不影响任何排版坐标。
    pub page_rect: RectProbe,
    /// 点字弹出的工具条所锚定的字（None 表示未弹出）。
    pub popup: Option<GlyphHit>,
    /// 拖拽选区的起点 / 焦点（(chapter, para, offset)）。
    pub sel_anchor: Option<(usize, usize, usize)>,
    pub sel_focus: Option<(usize, usize, usize)>,
    /// 是否正在拖拽选字（按下未松）。
    pub dragging: bool,
    /// 最近一次「按下」在正文区的位置（capture 阶段记录，用于翻页的单击判定）。
    pub press_at: Option<(f32, f32)>,
    /// 最近一次「按下」的时刻：与 `press_at` 配套，用来判定长按。
    pub press_time: Option<std::time::Instant>,
    /// 长按正文弹出的气泡菜单（复制 / 检索 / 字典）；None 表示未弹出。
    pub bubble: Option<GlyphHit>,
    /// 本次按下-抬起过程中是否碰过正文文字（点纸面翻页 vs 选字的分界）。
    pub touched_glyph: bool,
    /// 最近碰过的正文文字（拖选松手不在字格上时，工具条回退单字用）。
    pub sel_char: Option<char>,
    /// 正文 / 小字注释的配色方案。
    pub palette: InkPalette,
    /// 标点显隐：true 显示句读（朱色小字），false 隐藏（同时影响分页）。
    pub show_punct: bool,
    /// 生僻字显示：true 时给生僻字标注读音。
    pub show_rare: bool,
    /// 数字转汉字：true 时阿拉伯数字在竖排里显示为汉字（一六二八），
    /// 每个汉字独占一格（不再按「两字一格」压缩）。默认关闭——保留原貌。
    pub han_digits: bool,
    /// 响应式单 / 双叶：true 时按屏幕宽高比自动决定对开 / 单叶
    /// （宽屏双叶对开、窄屏单叶贴右），false 时沿用手动 `duplex`。
    pub auto_duplex: bool,
    /// **列数是否自动**（v1.29.0）：true = 按「这本书平均一章多少字」加上
    /// 当前可用宽算列数，false = 锁定用户手动选的那一档。
    ///
    /// 默认开。老机器升级后不会因为曾经选过 8 / 10 列就一直停在旧密度上——
    /// 自动档每帧重算；想锁死就在「版式」面板点具体列数（那会把这里置 false），
    /// 点回「自动」再放开。
    pub columns_auto: bool,
    /// 窄屏小字提示是否已弹过（避免 `fit_sizes` 每帧重设 `notice` 把提示刷没）。
    ///
    /// 窄屏硬选 14 列时字号会跌破 `GRID_FS_MIN`（12px）——引擎已自动把它
    /// 抬回 12px 并改为溢出滚动，但用户该知道「这一屏撑不下这套行款」。
    pub min_font_noticed: bool,
    /// 自动换挡时**待恢复的阅读锚点**（章, 段, 偏移）。
    ///
    /// 拉窗口可能让自动列数跨档（8 → 12），一换档就是整本重排、页码全变。
    /// `fit_sizes` 里不能在算完网格**之前**去动分页（那时几何还是旧的），
    /// 故只把锚点挂起，等 `render_reading` 排完新页再回找位置。
    pub pending_anchor: Option<(usize, usize, usize)>,
    /// **渲染模式**（v1.23.0 三种版式）：对开翻阅 / 手机竖屏单叶 / 整页等比缩放。
    ///
    /// 与 `duplex` 的分工：`layout_mode` 是**用户 / 屏幕选的意图**，
    /// `duplex` 是它推导出的**当帧铺法**（整页也铺两叶 + 中缝，故 = Facing）。
    /// 自动模式下 `auto_duplex` 只在对开 ⇄ 单叶之间来回，不碰整页。
    pub layout_mode: crate::grid::LayoutMode,
    /// 整页模式的当帧缩放系数（其余模式恒为 1.0）：供 UI 与调试查看
    /// 「整叶被缩到了几成」。
    pub full_scale: f32,
    /// 窗口内容区尺寸（每帧由 render 写入，供铺满屏计算）。
    pub viewport_w: f32,
    pub viewport_h: f32,
    /// 单张页面的实际渲染宽度（由 fit_sizes 按最终字号算出）。
    /// 版字面板用它取宽，保证与单页纸面严格等宽。
    pub page_w: f32,
    /// 单张页面的实际渲染高度（网格引擎 outer_h；`leaf_origin` 定位注文用）。
    pub page_h: f32,
    /// 可见面上所有注文块的包围盒（每帧刷新，Canvas 命中测试用）。
    pub note_boxes: Vec<crate::gloss::NoteBox>,
    /// 点注文弹出的放大卡片（None 表示未弹出）。
    pub note_zoom: Option<crate::gloss::NoteZoom>,
    /// 右键 / 长按**用户自加夹注**弹出的「修改 / 删除」菜单（None 表示未弹出）。
    pub note_menu: Option<crate::gloss::NoteMenu>,
    /// 现代横笺的行高（由 fit_sizes 按可用高度均分 12 行算出，用来纵向铺满）。
    pub line_modern: f32,
    pub notice: String,
    /// 开发用：一次性自检标记（见 render_shelf 的 JIANDU_RENAME_KEYS）。
    pub rename_test_done: bool,

    // ---- 全文检索 ----
    /// 检索面板是否展开。
    pub show_search: bool,
    /// 顶层 UI 状态机镜像（见 [`AppUIState`]）。
    pub ui_state: AppUIState,
    /// 检索框草稿与光标（字符下标）。
    pub search_draft: String,
    pub search_cursor: usize,
    /// 检索框焦点句柄（挂 InputHandler 用，见 ime.rs —— 中文要走 IME）。
    pub search_focus: FocusHandle,
    /// 检索框 IME 组字中的标记区间（UTF-16 下标）。
    pub search_marked: Option<std::ops::Range<usize>>,
    /// 命中位置：按「章 → 段 → 字」阅读顺序排列。
    pub search_hits: Vec<(usize, usize, usize)>,
    /// 当前命中下标。
    pub search_idx: usize,
    /// 所有命中字的键集合（整个匹配跨度都入集），供渲染层高亮。
    pub search_keys: std::collections::HashSet<(usize, usize, usize)>,
    /// 当前这一处命中的跨度 `(chapter, para, start, len)`：高亮比其它命中重一档。
    pub search_cur: Option<(usize, usize, usize, usize)>,

    /// 朱批录 / 搜索跳转后的「抵达」高亮：短暂朱砂衬底，提示用户已跳到该处。
    /// 口径与 `search_cur` 一致 `(chapter, para, offset, len)`。
    pub note_flash: Option<(usize, usize, usize, usize)>,
    /// 朱批录侧栏滑入 / 滑出动画（与 `toc_slide` 同理）。
    pub notes_slide: f32,
    pub notes_target: bool,
    pub notes_animating: bool,
    /// 卡片匣侧栏滑入 / 滑出动画（v1.74.0，与朱批录同款逐帧推进）。
    pub cards_slide: f32,
    pub cards_target: bool,
    pub cards_animating: bool,
    /// 检索侧栏滑入 / 滑出动画。
    pub search_slide: f32,
    pub search_target: bool,
    pub search_animating: bool,


    /// **导出闸门**（v1.73.0）：任一导出作业（出图 → 弹保存框 → 落盘）进行中
    /// 为 true，**所有出口**（成功 / 失败 / 取消）都必须复位。
    ///
    /// 修的是「连点两次导出 → 弹两个保存框、第二次把第一次的状态冲掉」。
    /// 保存框是**主线程模态**，但 `cx.spawn` 的 async 体就在同一个主线程上
    /// 被 poll：面板浮起前那几十毫秒里，第二次点击照样能进事件处理器。
    /// PNG 那条原本靠 `png_export.is_some()` 挡着，MD / 卡片 ×2 / 批注
    /// 四条**全裸奔**。统一交给这个闸门管。
    pub export_busy: bool,

    /// PNG 笺卡导出的两段式作业（v1.52.3）：`Some(Warm)` = 面板已收、等一帧
    /// 干净纸面；`Some(Shoot)` = 下一帧截屏 + 弹保存框。`None` = 空闲。
    /// 修的是「开着目录点导出 PNG，图里带着目录」——`screencapture` 截屏幕，
    /// 不等合成器翻页就会把浮层一起截进去（12 用户模拟报告 P2#2）。
    pub png_export: Option<PngStage>,

    /// **瞬时提示条**（v1.52.3 起，v1.74.0 由「导出专用」泛化为通用）：
    /// 导出成功路径 / 失败原因 / 用户取消、入匣回执……都送到底部正中间那行
    /// 宣纸纸签——`notice` 只在书架底栏渲染，**阅读视图里看不到**（P2#3）。
    /// 收起三选一：点「关」/ Esc / 点纸面；下一次交互即清，不常驻。
    pub toast: Option<String>,

    /// 窄屏首次提示的显示态（v1.52.3，P3#8）：窄屏（手机竖持 / 窗口拉窄）
    /// 单叶阅读时，纸面贴右缘、竖排自右向左——冷启动用户完全找不到正文，
    /// 故第一次进窄屏在底部正中亮一句「竖排自右向左，纸面贴右缘」，
    /// 点一下纸面即收。`narrow_hint_seen` 是持久档（只说一次）。
    pub narrow_hint: bool,
    /// 窄屏提示是否已看过（跨会话，落 `prefs.json` 的 `narrow_hint_seen`）。
    pub narrow_hint_seen: bool,

    /// 全书统计（章数, 字数）：开卷时算一次，供章末「篇·章·字」小表的
    /// 「全書」一行使用（逐帧扫全书太贵）。
    pub book_stat: (usize, usize),
    /// v1.60.0 **惰性分页器**：按章分块 + 前缀和索引 + LRU 窗口。
    ///
    /// 渲染循环只向它要「当前这一叶」，**绝不再每帧排全书**——旧架构每帧
    /// `to_flow`(13ms) + `layout_book`(94ms) ≈ 107ms/帧，是「导入完卡死」的
    /// 真凶。换书 / 换版式时它会整体作废重来。
    pub pager: Option<crate::pager::Pager>,
    /// 卷首题署预留行数的缓存：算它要遍历全书章题，别每帧来一遍。
    pub inscription_need: Option<usize>,

    // ---- 书架（题签网格 / 书脊列表 / 分类 / 搜索 / 排序）----
    /// 书架视图：题签网格（默认）或书脊列表。
    pub shelf_view: ShelfView,
    /// 书架排序方式。
    pub shelf_sort: ShelfSort,
    /// 分类页签：None = 全部；Some("经"/"史"/"子"/"集"/"自定") 只显示该类。
    pub shelf_cat: Option<String>,
    /// 书架搜索词（空 = 不筛）。与阅读页的全文检索是两套，互不干扰。
    pub shelf_query: String,
    pub shelf_query_cursor: usize,
    /// 书架搜索框焦点句柄（中文要走 IME，见 ime.rs）。
    pub shelf_query_focus: FocusHandle,
    /// 书架搜索框 IME 组字区间（UTF-16）。
    pub shelf_query_marked: Option<std::ops::Range<usize>>,
    /// 书架搜索框是否激活（决定焦点归属与 IME 目标）。
    pub shelf_searching: bool,
    /// 题签 / 书脊里书名被截断的那本：展开看全名（None = 未展开）。
    pub shelf_expand: Option<usize>,
    /// 「改归类」浮层正在改第几本（None = 未打开）。
    pub shelf_recat: Option<usize>,
    /// 鼠标正悬停在第几本上（决定卡片底部的管理栏是否浮现）。
    /// 纯交互态，**不进 prefs**——重开程序不该还记得鼠标在哪。
    pub shelf_hover: Option<usize>,
    /// 悬停序号：每换一次悬停目标就 +1，用作淡入动画的 ElementId 后缀。
    /// 换 id 才能让 `with_animation` 重新起一段动画（否则同一 id 的动画
    /// 已跑完，再浮现就是「啪」地出现）。
    pub shelf_hover_seq: u32,
    /// **批量管理**态：开启后点书衣是「勾选」而不是「翻书」。
    /// 纯交互态，不进 prefs——重开程序不该还停在勾选态。
    pub shelf_manage: bool,
    /// 已勾选的书（书架里的原下标，升序）。
    pub shelf_picked: Vec<usize>,
    /// 批量删除的二次确认（删多本是不可逆的，必须再问一句）。
    pub shelf_batch_confirm: bool,
    /// 关窗钩子是否已挂（v1.52.0：改回系统红绿灯后，关闭前要落盘一次）。
    pub close_hook_set: bool,
    /// 顶栏「导出」下拉是否展开（v1.52.2）：底部四枚导出按钮已撤，全部
    /// 收进这一枚菜单——当前页图片 / 当前章 Anki 卡片 / MD，逐条直调
    /// 原生保存对话框。渲染见 `reader::render_reading_toolbar`。
    pub export_menu: bool,
}

/// **后台导入任务**（v1.50.0 流式架构）。
///
/// 百万字的丛书（如《资治通鉴》全套）解析 + 清洗 + 落盘要跑好几秒，
/// 绝不能堵在主线程上——那会连进度条都画不出来。所以真正干活的是
/// `std::thread::spawn` 出来的那条线程，主线程每帧只来 `take_done()`
/// 探头看一眼；进度与结果都藏在 `Arc<Mutex<…>>` 里。
/// 导入线程的最终产物：成功则（书稿, 书名, 落盘路径），失败则一句人话。
type ImportOutcome = std::result::Result<(Book, String, PathBuf), String>;

pub struct ImportJob {
    /// 正在刻印的书名（文件名兜底，此时还不知道真书名）。
    pub name: String,
    pct: std::sync::Arc<std::sync::Mutex<f32>>,
    label: std::sync::Arc<std::sync::Mutex<String>>,
    done: std::sync::Arc<std::sync::Mutex<Option<ImportOutcome>>>,
    /// 起刻时刻（v1.69.0 看门狗用）。
    started: std::time::Instant,
    /// 上一次**观测到进度有推进**的时刻。
    moved: std::time::Instant,
    /// 上一帧看到的进度签名（pct 量化 + 文案）：变了才算「有推进」。
    sig: String,
}

/// 看门狗：起刻后**这么久还没任何推进**就判定卡死（v1.69.0）。
///
/// 正常的书 3 秒内一定已经解出第一节、进度条会动；卡在 0% 一动不动的，
/// 只可能是解析器陷在脏文件里出不来。这时必须**报错并收掉浮层**，
/// 绝不能让用户对着 0% 干等（用户原话：「绝对不能假死」）。
const IMPORT_STALL_START: std::time::Duration = std::time::Duration::from_secs(3);
/// 已经动过之后的宽容阈值：大书单节可能真要跑十几秒（尔雅 9.5MB、
/// 诸子百家 18MB），给足 45 秒，避免误杀。
const IMPORT_STALL_LATER: std::time::Duration = std::time::Duration::from_secs(45);

/// **渐进铺索引**每帧啃几章（v1.70.0）。
///
/// 一章的成本是「SQLite 读一章 + 排一章」≈ 0.14ms，8 章 ≈ 1.1ms——塞进
/// 一帧里 60 FPS 毫无感觉；799 章的书约 100 帧（1.7 秒）铺满。给太大
/// 会在铺索引的那几帧掉帧（正是要避免的），给太小则铺得太慢。
const INDEX_WARM_CHAPTERS: usize = 8;

impl ImportJob {
    /// 起一条后台线程：流式解析 → 逐章落盘 → 结果塞进 `done`。
    pub fn start(path: PathBuf) -> Self {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("书")
            .to_string();
        let pct = std::sync::Arc::new(std::sync::Mutex::new(0.0f32));
        let label = std::sync::Arc::new(std::sync::Mutex::new("雕版刻印中".to_string()));
        let done = std::sync::Arc::new(std::sync::Mutex::new(None));

        let p = path.clone();
        let pct2 = pct.clone();
        let lab2 = label.clone();
        let done2 = done.clone();
        std::thread::spawn(move || {
            let cb = move |v: f32, s: String| {
                if let Ok(mut g) = pct2.lock() {
                    *g = v;
                }
                if let Ok(mut g) = lab2.lock() {
                    *g = s;
                }
            };
            // ── v1.69.0：**导入线程绝不许 panic 到消失** ──────────────
            // 后台线程一 panic，`done` 永远不会被填上，主线程就一直以为
            // 「还在刻印」——进度条永远停在 0%，App 看着就是假死，而用户
            // 连一句报错都看不到。这里用 `catch_unwind` 把 panic 收成一条
            // 正常的失败结果，交给下面的错误分支去提示。
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // 主路：core 的流式导入（逐章落 chapters.db，书架只留骨架）。
                // 兜底：v1.62.0 的多格式漏斗——脏编码 TXT / 畸形 EPUB /
                // 半截 JSON 这类「主路读不动」的稿子，换漏斗再试一次
                // （整本入库，store_id 留空）。
                match crate::stream::import_streaming(&p, Some(Box::new(cb))) {
                    Ok((book, id)) => Ok((book, id, p.clone())),
                    Err(main_err) => crate::importer::import_file(&p)
                        .map(|ir| (ir.to_book(), String::new(), p.clone()))
                        .map_err(|e| format!("{main_err}；兜底也失败：{e}")),
                }
            }))
            .unwrap_or_else(|_| {
                Err("解析器在这本电子书上崩溃了（多半是文件损坏或非标准结构）".to_string())
            });
            if let Ok(mut g) = done2.lock() {
                *g = Some(r);
            }
        });

        let now = std::time::Instant::now();
        Self {
            name,
            pct,
            label,
            done,
            started: now,
            moved: now,
            // 初始签名 = 「0% + 初始文案」：这样第一帧比下来是「没推进」，
            // 看门狗走 3 秒的严档；一旦真有推进，立刻放宽到 45 秒。
            sig: "0.0000|雕版刻印中".to_string(),
        }
    }

    /// 当前进度（0.0~1.0）。
    pub fn pct(&self) -> f32 {
        self.pct.lock().map(|g| *g).unwrap_or(0.0)
    }

    /// 当前进度文案（「雕版刻印中」/「刻版」……）。
    pub fn label(&self) -> String {
        self.label
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "雕版刻印中".to_string())
    }

    /// 取走结果（只可能成功一次；没完成返回 `None`）。
    pub fn take_done(&self) -> Option<std::result::Result<(Book, String, PathBuf), String>> {
        self.done.lock().ok().and_then(|mut g| g.take())
    }
}

impl Root {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let mut library = Library::load();
        if library.books.is_empty() {
            let _ = library.seed_sample();
        }
        ensure_assets();
        // 阅读偏好（跨书持久化）：留白档从这里落下——文件缺失 / 损坏一律
        // 回落默认值（不让坏存档挡住启动），见 `prefs::Prefs::load`。
        let prefs = crate::prefs::Prefs::load();
        let count = library.books.len();
        let mut root = Self {
            library,
            screen: Screen::Shelf,
            current: 0,
            book: None,
            script: Script::Ancient,
            grid: GridConfig::default(),
            content_w: 0.0,
            content_h: 0.0,
            col_w: 10.0,
            row_h: 11.0,
            grid_rows: 11,
            grid_min_font: false,
            margin_top: 0.0,
            margin_bot: 0.0,
            // 留白默认「中（75%）」：既留古籍两侧留白的优雅，又保证现代
            // 屏幕上的阅读体验（用户 v1.33.0 定稿）。启动时由 prefs 落值。
            banxin_ratio: prefs.banxin_ratio(),
            layout_key: LayoutConfig::default_key(),
            duplex: Duplex::Single,
            layout_mode: crate::grid::LayoutMode::Facing,
            full_scale: 1.0,
            // 正文首选文津宋体：大字符集、生僻字全覆盖、竖排标点挤压正确。
            font_ancient: FontChoice::WenJinMincho,
            font_modern: FontChoice::SourceHanSans,
            font_note: FontChoice::XuandongKaishu,
            font_preset: FontPreset::Song,
            show_traditional: true,
            show_tongjia: true,
            show_notes: true,
            notes_inline: true,
            theme: Theme::Xuan,
            size_ancient: 26.0,
            size_modern: 18.0,
            page: 0,
            current_chapter: 0,
            show_toc: false,
            toc_scroll: ScrollHandle::new(),
            toc_need_scroll: false,
            import_job: None,
            toc_slide: 0.0,
            toc_target: false,
            toc_animating: false,
            show_bookmarks: false,
            bookmarks: Vec::new(),
            bookmarks_slide: 0.0,
            bookmarks_target: false,
            bookmarks_animating: false,
            bookmarks_scroll: ScrollHandle::new(),
            show_style: false,
            focus_mode: false,
            bar_shown: true,
            zoom: 1.0,
            lookup: None,
            lookup_key: None,
            lookup_text: String::new(),
            lookup_at: None,
            lookup_rect: None,
            hover_key: None,
            hover_seq: 0,
            hover_card: false,
            dict_tab: DictTab::default(),
            notes: std::collections::HashMap::new(),
            show_notes_panel: false,
            note_editing: false,
            note_edit_note: None,
            note_inject_key: None,
            note_draft: String::new(),
            note_cursor: 0,
            note_focus: _cx.focus_handle(),
            note_marked: None,
            head_notes: Vec::new(),
            margin_notes: Vec::new(),
            margin_hover: None,
            margin_hover_key: None,
            margin_draft_at: None,
            margin_edit_id: None,
            margin_bands: [Rc::new(Cell::new(None)), Rc::new(Cell::new(None))],
            head_anchor: None,
            head_press: None,
            head_active: None,
            head_edit: None,
            note_pos: None,
            note_suppress_commit: false,
            rename_idx: None,
            rename_draft: String::new(),
            rename_cursor: 0,
            rename_focus: _cx.focus_handle(),
            key_focus: _cx.focus_handle(),
            rename_marked: None,
            delete_confirm: None,
            show_help: false,
            underlines: std::collections::HashSet::new(),
            show_share_panel: false,
            share_show_title: true,
            share_show_author: true,
            share_show_chapter: true,
            share_show_date: true,
            share_tpl: 0,
            share_bg: 0,
            share_frame: 0,
            share_seal: 0,
            share_show_notes: true,
            share_red: Vec::new(),
            share_notes: Vec::new(),
            share_seal_text: crate::share::seal_chars(""),
            share_dir: crate::share::ShareDir::Vertical,
            share_dir_touched: false,
            share_tab: crate::share::ShareTab::Style,
            share_font: None,
            share_body: Vec::new(),
            share_from_sel: false,
            share_sel: None,
            share_book: String::new(),
            share_author: String::new(),
            share_chap: String::new(),
            share_leaf: 0,
            share_date: String::new(),
            rule_color: RuleColor::Wu,
            share_card_rect: None,
            page_rect: Rc::new(Cell::new(None)),
            popup: None,
            sel_anchor: None,
            sel_focus: None,
            dragging: false,
            press_at: None,
            press_time: None,
            bubble: None,
            touched_glyph: false,
            sel_char: None,
            palette: InkPalette::Ink,
            show_punct: true,
            show_rare: false,
            han_digits: false,
            auto_duplex: true,
            // 列数默认自动：按书的内容与可用宽算，不写死某一档。
            columns_auto: true,
            pending_anchor: None,
            min_font_noticed: false,
            viewport_w: 1280.0,
            viewport_h: 860.0,
            page_w: 640.0,
            page_h: 480.0,
            note_boxes: Vec::new(),
            note_zoom: None,
            note_menu: None,
            line_modern: 26.0,
            rename_test_done: false,
            show_search: false,
            ui_state: AppUIState::None,
            search_draft: String::new(),
            search_cursor: 0,
            search_focus: _cx.focus_handle(),
            search_marked: None,
            search_hits: Vec::new(),
            search_idx: 0,
            search_keys: std::collections::HashSet::new(),
            search_cur: None,
            note_flash: None,
            notes_slide: 0.0,
            notes_target: false,
            notes_animating: false,
            cards_slide: 0.0,
            cards_target: false,
            cards_animating: false,
            show_cards: false,
            cards: Vec::new(),
            search_slide: 0.0,
            search_target: false,
            search_animating: false,
            png_export: None,
            export_busy: false,
            toast: None,
            narrow_hint: false,
            narrow_hint_seen: prefs.narrow_hint_seen,
            book_stat: (0, 0),
            pager: None,
            inscription_need: None,
            shelf_view: ShelfView::TitleSlip,
            shelf_sort: ShelfSort::Recent,
            shelf_cat: None,
            shelf_query: String::new(),
            shelf_query_cursor: 0,
            shelf_query_focus: _cx.focus_handle(),
            shelf_query_marked: None,
            shelf_searching: false,
            shelf_expand: None,
            shelf_recat: None,
            shelf_hover: None,
            shelf_hover_seq: 0,
            shelf_manage: false,
            shelf_picked: Vec::new(),
            shelf_batch_confirm: false,
            close_hook_set: false,
            export_menu: false,
            notice: if count == 0 {
                "书架空空，点「导入书籍」放一本进来".into()
            } else {
                format!("书架共 {count} 本")
            },
        };

        // 开发用：JIANDU_AUTO_READ=1 直接进入正文，方便截图核对版式
        // （JIANDU_BOOK_IDX 可指定打开书架第几本）。
        // 行高缩放随**当前正文字体**定：默认「刻本宋体」（文津宋体）取 1.05。
        // 不设的话是中性值 1.0，切一次排版字体后行高会突然变——一开局就对齐。
        root.grid.line_scale = crate::fonts::preset_line_scale(root.font_preset);
        // 开发用：JIANDU_SHELF=list 以书脊列表开局、JIANDU_SHELF_Q=词 预填搜索、
        // JIANDU_SHELF_CAT=史 预选分类，便于逐态截图核对书架。
        if let Ok(v) = std::env::var("JIANDU_SHELF") {
            if v == "list" {
                root.shelf_view = ShelfView::Spine;
            }
        }
        // 开发用：JIANDU_PANEL=style|banzi|toc|notes|help 直接把某个面板摊开，
        // 便于逐面板截图核对（顶栏默认隐身，脚本点不到）。
        // v1.35.0：样式与版式已合并为唯一一张「版字」纸，三个档名同义。
        if let Ok(v) = std::env::var("JIANDU_PANEL") {
            match v.as_str() {
                "grid" | "banzi" | "style" => root.show_style = true,
                "toc" => root.open_toc(),
                "notes" => root.show_notes_panel = true,
                "help" => root.show_help = true,
                _ => {}
            }
        }
        if let Ok(v) = std::env::var("JIANDU_SHELF_Q") {
            root.shelf_query = v;
            root.shelf_query_cursor = root.shelf_query.chars().count();
        }
        if let Ok(v) = std::env::var("JIANDU_SHELF_CAT") {
            if !v.is_empty() {
                root.shelf_cat = Some(v);
            }
        }
        if std::env::var("JIANDU_AUTO_READ").is_ok() && !root.library.books.is_empty() {
            let idx = std::env::var("JIANDU_BOOK_IDX")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0)
                .min(root.library.books.len() - 1);
            root.open_book(idx);
            root.screen = Screen::Reading;
        }
        // 开发用：JIANDU_COLUMNS=n 指定初始列数（8/10/12/14），便于逐列截图核对。
        if let Ok(v) = std::env::var("JIANDU_COLUMNS") {
            if let Ok(n) = v.parse::<usize>() {
                root.set_columns(n);
            }
        }
        // 开发用：JIANDU_PAGE=3 直接翻到第 3 叶（0 基），便于核对章首版心。
        if let Ok(v) = std::env::var("JIANDU_PAGE") {
            if let Ok(p) = v.parse::<usize>() {
                root.page = p;
            }
        }
        // 开发用：JIANDU_PALETTE=n 指定初始墨色组（0..InkPalette::ALL.len()），
        // 便于截图核对各套墨色（如 5 = 暗金 · 米白夹注）。
        if let Ok(v) = std::env::var("JIANDU_PALETTE") {
            if let Ok(i) = v.parse::<usize>() {
                if let Some(p) = InkPalette::ALL.get(i) {
                    root.palette = *p;
                }
            }
        }
        // 开发用：默认展开浮层，便于截图核对（不影响正常逻辑）。
        if std::env::var("JIANDU_TOC_OPEN").is_ok() {
            root.show_toc = true;
            root.toc_target = true;
            root.toc_slide = 1.0;
            root.toc_need_scroll = true;
        }
        if std::env::var("JIANDU_NOTES_OPEN").is_ok() {
            root.show_notes_panel = true;
        }
        // 开发用：直接浮出笔记编辑框，便于截图核对浮层版式。
        if std::env::var("JIANDU_NOTE_EDIT").is_ok() {
            root.start_note_edit_at(None);
        }
        // 开发用：直接打开重命名对话框，便于截图核对。
        if std::env::var("JIANDU_RENAME_OPEN").is_ok() {
            root.start_rename(0);
        }
        // 开发用：给本页开头几个字划线，便于截图核对「朱点 + 朱线」。
        if std::env::var("JIANDU_UNDERLINE").is_ok() {
            let keys: Vec<(usize, usize, usize)> = root
                .book
                .as_ref()
                .map(|b| {
                    b.to_flow(true)
                        .iter()
                        .filter(|g| !g.is_indent() && g.kind == GlyphKind::Char)
                        .take(4)
                        .map(|g| (g.chapter, g.para, g.offset))
                        .collect()
                })
                .unwrap_or_default();
            for k in keys {
                root.underlines.insert(k);
            }
        }
    // 开发用：直接打开分享面板，便于核对版式 / 底色 / 开关。
    // v1.64.0：改为走**真实快照**（旧版只置 `show_share_panel = true`，
    // 从没抓过正文，卡片必然是空的——这正是「分享没实现」的现场）。
    // 再加 `JIANDU_SHARE_SEL=1` 时先伪造一段选区，用于核对「分享所选」。
    if std::env::var("JIANDU_SHARE_OPEN").is_ok() {
        if std::env::var("JIANDU_SHARE_SEL").is_ok() {
            // 取「第一处真有正文的段落」——不少书的章首段是空的（只有章题）。
            let mut pick: Option<(usize, usize, usize)> = None;
            if let Some(b) = root.book.as_ref() {
                'outer: for (ci, ch) in b.chapters.iter().enumerate() {
                    for (pi, p) in ch.paragraphs.iter().enumerate() {
                        let n = p.text.chars().count();
                        if n > 4 {
                            pick = Some((ci, pi, (n - 1).min(24)));
                            break 'outer;
                        }
                    }
                }
            }
            if let Some((ci, pi, end)) = pick {
                root.sel_anchor = Some((ci, pi, 1));
                root.sel_focus = Some((ci, pi, end));
            }
        }
        if let Err(e) = root.snapshot_share() {
            root.notice = e;
        }
        root.show_share_panel = true;
    }
    // 开发用：注入两条**纯内存**眉批并唤醒管理菜单（改 / ✕），便于截图
    // 核对眉批管理交互（v1.48.0）。不回写书稿 JSON；若真的删了它们，
    // 回写时以内存集合为准——演示数据可随手删净。
    if std::env::var("JIANDU_HEAD_NOTE").is_ok() {
        if let Some(key) = root.page_anchor() {
            root.head_notes
                .push(crate::types::HeadNote::new(key, 0.30, 0.08, "眉批：與《索隱》異"));
            root.head_notes
                .push(crate::types::HeadNote::new(key, 0.30, 0.32, "再批：疑有衍文"));
            root.head_active = Some(0);
            root.notice = "眉批演示：点字柱唤醒，再点『改 / ✕』".to_string();
        }
    }
        // 开发用：指定分享卡模板 / 方向，便于逐项截图核对。
        if let Ok(v) = std::env::var("JIANDU_SHARE_TPL") {
            if let Ok(i) = v.parse::<usize>() {
                if i < crate::share::TEMPLATES.len() {
                    root.share_tpl = i;
                }
            }
        }
        if let Ok(v) = std::env::var("JIANDU_SHARE_DIR") {
            root.share_dir = match v.as_str() {
                "h" | "horizontal" => crate::share::ShareDir::Horizontal,
                _ => crate::share::ShareDir::Vertical,
            };
            root.share_dir_touched = true;
        }
        // 开发用：指定丝栏色（0 乌 / 1 金 / 2 朱），便于逐色截图核对。
        if let Ok(v) = std::env::var("JIANDU_RULE") {
            if let Ok(i) = v.parse::<usize>() {
                if let Some(rc) = crate::types::RuleColor::ALL.get(i) {
                    root.rule_color = *rc;
                }
            }
        }
        // 开发用：展开「版字」面板（JIANDU_BANZI_OPEN 取代旧的
        // JIANDU_GRID_OPEN——独立版式浮层已并入这一张纸）。
        if std::env::var("JIANDU_STYLE_OPEN").is_ok()
            || std::env::var("JIANDU_BANZI_OPEN").is_ok()
        {
            root.show_style = true;
        }
        // 开发用：直接进入专注模式。
        if std::env::var("JIANDU_FOCUS").is_ok() {
            root.focus_mode = true;
        }
        // 开发用：直接切双面对开（古文竖笺也要能对开）。
        if std::env::var("JIANDU_FACING").is_ok() {
            root.duplex = Duplex::Facing;
        }
        // 开发用：直接切到 14 列（信笺模板已删除，14 列就是标准刻本版式）。
        if std::env::var("JIANDU_LETTER").is_ok() {
            root.layout_key = "standard14";
            root.grid.columns = 14;
            root.duplex = Duplex::Single;
        }
        // 开发用：JIANDU_NOTE_ZOOM=x,y 在给定窗口坐标直接弹出注文放大卡
        // （模拟点击注文块；本机合成鼠标点击被 macOS TCC 拦，无法端到端
        // 验证真实点击，用此钩子核对卡片渲染版式）。文本用道德经长注。
        if let Ok(v) = std::env::var("JIANDU_NOTE_ZOOM") {
            if let Some((xs, ys)) = v.split_once(',') {
                if let (Ok(x), Ok(y)) = (xs.trim().parse::<f32>(), ys.trim().parse::<f32>()) {
                    root.note_zoom = Some(crate::gloss::NoteZoom {
                        text: "名：称谓、概念。凡有称谓者，皆非恒常之名；老子虑人执名以求道，故破名相，而欲其实践无为之玄览。名起于分别，分别起于对待，对待既立而道隐矣。".into(),
                        x,
                        y,
                        page: root.page,
                        pinyin: None,
                    });
                }
            }
        }

        // 开发用：JIANDU_USER_NOTE=offset 在首章首段第 offset 字挂一条
        // 「用户自加夹注」，便于截图核对深朱砂红 + 淡红底框渲染（以及
        // 右键改删菜单的命中演示）。文本用道德经风格短注。
        if let Ok(v) = std::env::var("JIANDU_USER_NOTE") {
            if let Ok(off) = v.trim().parse::<usize>() {
                crate::note_inject::inject_inline_note_at_key(
                    root.book.as_mut().unwrap(),
                    0,
                    0,
                    off,
                    "道可道，非常道。此为用户自加夹注，色作深朱砂，可与原注相辨。",
                    crate::types::NoteSource::User,
                );
            }
        }

        // 开发用：JIANDU_NOTE_MENU=x,y 在给定窗口坐标直接弹出「自加夹注」
        // 改删菜单（模拟右键命中注文块；合成鼠标右键同样被 TCC 拦）。
        // 宿主字取首章首段第一个正文字，与 JIANDU_USER_NOTE 配合可端到端
        // 核对「朱砂注 → 右键菜单 → 改/删」链路。
        if let Ok(v) = std::env::var("JIANDU_NOTE_MENU") {
            if let Some((xs, ys)) = v.split_once(',') {
                if let (Ok(x), Ok(y)) = (xs.trim().parse::<f32>(), ys.trim().parse::<f32>()) {
                    let key = root
                        .book
                        .as_ref()
                        .and_then(|b| {
                            b.to_flow(true)
                                .iter()
                                .find(|g| g.kind == GlyphKind::Char)
                                .map(|g| (g.chapter, g.para, g.offset))
                        })
                        .unwrap_or((0, 0, 0));
                    root.note_menu = Some(crate::gloss::NoteMenu {
                        key,
                        text: "道可道，非常道。此为用户自加夹注。".into(),
                        x,
                        y,
                    });
                }
            }
        }

        root
    }

    pub fn current_font(&self) -> FontChoice {
        match self.script {
            Script::Ancient => self.font_ancient,
            Script::Modern => self.font_modern,
        }
    }

    pub fn current_size(&self) -> f32 {
        match self.script {
            Script::Ancient => self.size_ancient,
            Script::Modern => self.size_modern,
        }
    }

    /// **切换正文字体**——面板里「字体」这一行的唯一入口。
    ///
    /// v1.35.0 去重：原先「排版字体」（预设，一键换一对）与「字体」（单换
    /// 正文）两行都写 `font_ancient`，各写各的、互相打脸——在字体行挑了
    /// 楷体，预设行还亮着「刻本宋体」。现在只留「字体」一行，`set_font`
    /// 一个入口把四件事一次做完（与 `set_font_preset` 同源）：
    ///   1. 换**正文**字体；
    ///   2. 归位**排版预设**（楷体 → 写体，其余 → 匠体），行高缩放随之；
    ///   3. 换**批注**字体（永远与正文不同体，见 `FontPreset::note_choice`）；
    ///   4. **重算尺寸**（`fit_sizes`）——行高 / 行数 / 字号全部重算。
    pub fn set_font(&mut self, c: FontChoice) {
        match self.script {
            Script::Ancient => self.font_ancient = c,
            Script::Modern => self.font_modern = c,
        }
        // 楷体是「写体」，行间要更松（`preset_line_scale`）；其余按匠体排。
        self.font_preset = FontPreset::for_main(c);
        let preset = self.font_preset;
        self.font_note = preset.note_choice();
        self.grid.line_scale = crate::fonts::preset_line_scale(preset);
        self.fit_sizes();
        self.persist();
    }

    /// 批注字体（楷体），与正文宋体分开。
    pub fn note_font(&self) -> FontChoice {
        self.font_note
    }

    pub fn set_note_font(&mut self, c: FontChoice) {
        self.font_note = c;
        self.persist();
    }

    /// 当前正文字体族名（渲染层唯一入口）。
    pub fn main_family(&self) -> gpui::SharedString {
        crate::fonts::family_for(self.font_ancient, self.script)
    }

    /// 当前批注（夹注 / 放大注卡）字体族名。
    pub fn note_family(&self) -> gpui::SharedString {
        crate::fonts::note_family(self.font_note)
    }

    /// 打开「字义浮签」：记下被查那个字的 **charIndex**、选中文本与锚点。
    ///
    /// `at = Some((x, y))` 时浮签贴着长按点浮起（长按查字走这条路）；
    /// `None` 时退化为右上角常驻面板（工具条「查询」按钮走这条路）。
    pub fn open_lookup(
        &mut self,
        key: GlyphKey,
        ch: char,
        text: String,
        at: Option<(f32, f32)>,
        rect: Option<(f32, f32, f32, f32)>,
    ) {
        self.lookup = Some(ch.to_string());
        self.lookup_key = Some(key);
        self.lookup_text = text;
        self.lookup_at = at;
        self.lookup_rect = rect;
        // 智能默认页签：常用字（简明库有数据）开签先亮「简明」，生僻字
        // （简明库无数据）退回「康熙」。用户之后仍可手动切换其余页签。
        self.dict_tab = if crate::dict::concise_of(ch).is_some() {
            DictTab::Jianming
        } else {
            DictTab::Kangxi
        };
    }

    /// **悬停查词**（v1.38.0，v1.40.0 重做状态机）：鼠标在一个字上停满
    /// [`crate::reader::HOVER_LOOKUP`] 就自动浮出字书浮签，与长按互为两条路——
    /// 手不离笔时悬停更快，触屏没有 hover 就仍走长按。
    ///
    /// 只排定时器、不立刻弹签：鼠标每动一格就把 `hover_seq` 加一，先前那枚
    /// 定时器醒来发现代号对不上便自行作废。于是「掠过一排字」不会连甩
    /// 十几张签，只有真正停住的那一个字才弹。
    ///
    /// v1.40.0 三处修正（此前会造成「悬停完全没反应」）：
    ///   * 悬停呼出的签**随指针走**：移到别的字就换签、离开正文就收签。
    ///     旧版一见 `bubble.is_some()` 就整体退出，于是弹过一张之后永远不再弹。
    ///   * 工具条（`popup`）不再堵死悬停：停稳后收掉工具条让位给字书签。
    ///   * 长按 / 点字呼出的签仍「钉住」，只有悬停那张才自动收。
    pub fn probe_hover(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        // 这些状态下悬停一律不弹：写批注 / 拖选 / 专注 / 面板展开。
        if self.note_editing
            || self.dragging
            || self.focus_mode
            || self.show_toc
            || self.show_style
            || self.book.is_none()
        {
            self.hover_key = None;
            return;
        }
        // 长按 / 点字呼出的签是「钉住」的：不去打扰，等用户点掉。
        if self.bubble.is_some() && !self.hover_card {
            self.hover_key = None;
            return;
        }
        let Some(gb) = self.glyph_hit_at(x, y) else {
            self.hover_key = None;
            // 指针离开正文：把悬停那张收掉（钉住的不动）。
            if self.hover_card {
                self.dismiss_hover_card(cx);
            }
            return;
        };
        let key = gb.key();
        // 已经在这个字上摆着签：别重启计时。
        if self.hover_card && self.lookup_key == Some(key) {
            return;
        }
        // 还停在这个字上（签还没弹出来）：同理别重启。
        if self.hover_key == Some(key) && !self.hover_card {
            return;
        }
        self.hover_key = Some(key);
        self.hover_seq = self.hover_seq.wrapping_add(1);
        let seq = self.hover_seq;
        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                cx.background_executor()
                    .timer(crate::reader::HOVER_LOOKUP)
                    .await;
                let _ = this.update(&mut cx, |this, cx| {
                    if this.hover_seq != seq || this.hover_key != Some(key) {
                        return;
                    }
                    // 醒来时再反查一次：确认指针还压在那个字上（其间可能
                    // 翻了页、换了列数，坐标底下的字已经不是原来那个了）。
                    let Some(g2) = this.glyph_hit_at(x, y) else {
                        return;
                    };
                    if g2.key() != key {
                        return;
                    }
                    // ── v1.52.10：恢复悬停查词 ────────────────────────
                    // v1.49.1 曾把这条路整体禁用（怕字典签顶掉选词工具条）。
                    // 现在的护栏改在**入口**：只要选词工具条（popup）在屏、
                    // 或正在拖选 / 写字，就不抢——工具条该出来时一定出来；
                    // 指针单纯在字上停稳（没点过）才浮出字书浮签。
                    if this.note_editing || this.dragging || this.popup.is_some() {
                        return;
                    }
                    let (ch, gx, gy, gw, gh) = (
                        g2.glyph.ch,
                        g2.x,
                        g2.y,
                        g2.w,
                        g2.h,
                    );
                    // 词组：从命中字起取 4 字交给词典做最长匹配
                    // （「长安」成一条词，而不是「长」+「安」）。
                    let mut text = this.text_from(key, 4);
                    if text.is_empty() {
                        text = ch.to_string();
                    }
                    this.bubble = Some(GlyphHit {
                        key,
                        ch,
                        text: text.clone(),
                        x,
                        y,
                    });
                    this.open_lookup(key, ch, text, Some((x, y)), Some((gx, gy, gw, gh)));
                    this.hover_card = true;
                    cx.notify();
                });
            }
        })
        // 后台等 500ms 的那枚定时器不必持有：窗口关了它自然被丢。
        .detach();
    }

    /// 收起「悬停」那张字书浮签（钉住的不动）。
    fn dismiss_hover_card(&mut self, cx: &mut Context<Self>) {
        self.hover_card = false;
        self.hover_key = None;
        self.bubble = None;
        self.close_lookup();
        cx.notify();
    }

    /// 关闭字义浮签：三件套一起清，免得下次点字还残留上次的 charIndex。
    pub fn close_lookup(&mut self) {
        self.lookup = None;
        self.lookup_key = None;
        self.lookup_text.clear();
        self.lookup_at = None;
        self.lookup_rect = None;
        // 签都没了，自然也不是「悬停那张」了。
        self.hover_card = false;
    }

    // ------------------------------------------------------------------
    // 坐标反查：只用 (x, y) 反推「查的是哪个字」
    // ------------------------------------------------------------------
    //
    // 元素级事件（每个字格挂 `on_mouse_down`）能回答「点了哪个字」，但
    // 落在格子缝隙、夹注块上、或拖选结束那一下，元素不一定命中。这里走
    // 几何：窗口坐标 → 叶 → 列 → 列内半格账 → 字。与渲染同源（列序、
    // 夹注半格、西文成组、卷首题署预留带都对齐），见 `crate::hit`。

    /// 选区在屏幕上的**包围盒**（窗口坐标，v1.40.0）。
    ///
    /// 浮签要绕开的是**整段被选中的字**，不是按下那一点——否则拖选一排
    /// 字时，浮签会稳稳压在选区上。做法与 `glyph_hit_at` 同源：逐叶逐列
    /// 取格，凡字形键落在选区内的就并进包围盒。纯读布局，不改任何排版。
    pub fn selected_anchor(&self) -> Option<crate::hit::Anchor> {
        let a = self.sel_anchor?;
        let b = self.sel_focus?;
        if a == b {
            return None;
        }
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let pages = self.layout_pages();
        if pages.is_empty() {
            return None;
        }
        let facing = self.duplex == Duplex::Facing;
        let mut leaves: Vec<(usize, bool)> = Vec::new();
        if facing {
            if self.page < pages.len() {
                leaves.push((self.page, false)); // 右叶
            }
            if self.page + 1 < pages.len() {
                leaves.push((self.page + 1, true)); // 左叶
            }
        } else if self.page < pages.len() {
            leaves.push((self.page, false));
        }
        let gap = crate::grid::gutter_w(
            self.content_w,
            self.grid.columns,
            ANCIENT_EDGE,
            self.grid.margin,
        );
        let right_align = !facing && crate::grid::is_narrow(self.viewport_w, self.viewport_h);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (idx, back) in leaves {
            let Some(pg) = pages.get(idx) else { continue };
            let (ox, oy) = crate::gloss::leaf_origin(
                facing,
                self.viewport_w,
                self.viewport_h,
                self.page_w,
                self.page_h,
                gap,
                back,
                self.focus_mode,
                right_align,
            );
            let f = self.leaf_frame(pg);
            for i in 0..f.text_cols {
                let cx = crate::hit::col_x(&f, i);
                let skip = if f.start_chapter && i == 0 {
                    f.reserve as f32 * f.cell_h
                } else {
                    0.0
                };
                let items: &[crate::gloss::Piece] =
                    pg.columns.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
                for sp in crate::hit::column_spans(items, f.cell_h, f.west_mode) {
                    let in_sel = match &sp.kind {
                        crate::hit::SpanKind::Main(g) => {
                            let k = (g.chapter, g.para, g.offset);
                            k >= lo && k <= hi
                        }
                        crate::hit::SpanKind::West(chars) => chars.iter().any(|g| {
                            let k = (g.chapter, g.para, g.offset);
                            k >= lo && k <= hi
                        }),
                        _ => false,
                    };
                    if !in_sel {
                        continue;
                    }
                    let gx = ox + cx;
                    let gy = oy + sp.y + skip;
                    x0 = x0.min(gx);
                    y0 = y0.min(gy);
                    x1 = x1.max(gx + f.cell_w);
                    y1 = y1.max(gy + sp.h);
                }
            }
        }
        if x0 == f32::MAX {
            None
        } else {
            Some(crate::hit::Anchor {
                x: x0,
                y: y0,
                w: (x1 - x0).max(1.0),
                h: (y1 - y0).max(1.0),
            })
        }
    }

    /// 从 `key` 起向后取 `n` 个字符（查词用：词组最长匹配）。
    ///
    /// 长按一个字，光知道「这是哪个字」不够——「长安」要当一条词来查。
    /// 这里从命中字的 `offset` 起取几个字交给 `dict::lookup` 做最长匹配，
    /// 命中就出词条，不命中自然退回单字。
    pub fn text_from(&self, key: GlyphKey, n: usize) -> String {
        let Some(b) = &self.book else {
            return String::new();
        };
        let (ci, pi, off) = key;
        let Some(ch) = b.chapters.get(ci) else {
            return String::new();
        };
        let Some(p) = ch.paragraphs.get(pi) else {
            return String::new();
        };
        p.text.chars().skip(off).take(n).collect()
    }

    /// 当前叶的几何（反查用，与 `render_ancient` 同源）。
    pub fn leaf_frame(&self, pg: &crate::gloss::Page) -> crate::hit::LeafFrame {
        let total = self.grid.columns;
        // 章首页最右一列让给章题，正文列数少一列（与渲染同源）。
        let text_cols = if pg.start_chapter {
            total.saturating_sub(1)
        } else {
            total
        };
        let cfg = self.current_layout();
        crate::hit::LeafFrame {
            cell_w: self.col_w,
            cell_h: self.row_h,
            rows: self.grid_rows,
            text_cols,
            start_chapter: pg.start_chapter,
            title_cols: ANCIENT_TITLE_COL,
            show_fish_tail: cfg.show_fish_tail,
            // 信笺中缝插在第 7、8 个正文列之间；章首页多一个章题列，
            // 分割点随之前移一列（与渲染的 `split_i` 同一条规则）。
            gutter_at: if pg.start_chapter { 5 } else { 6 },
            reserve: self.chapter_inscription_reserve_rows(),
            west_mode: !self.han_digits,
        }
    }

    /// 窗口坐标 → 字（含它在屏幕上的矩形）。
    ///
    /// 命中的是**当前可见的叶**：对开先试右叶（当前面）、再试左叶（下一面），
    /// 与 `render_reading` 的舞台排布一致。返回的字用**窗口坐标**，浮签
    /// 避让可直接拿去算。
    pub fn glyph_hit_at(&self, x: f32, y: f32) -> Option<crate::hit::GlyphBox> {
        let pages = self.layout_pages();
        self.glyph_hit_in(&pages, x, y)
    }

    /// `glyph_hit_at` 的实段：排版由调用方给（一次排版多探几个点，见
    /// `glyph_hit_near`——整书排版不便宜，不能每个候选点排一遍）。
    fn glyph_hit_in(&self, pages: &[crate::gloss::Page], x: f32, y: f32) -> Option<crate::hit::GlyphBox> {
        if pages.is_empty() {
            return None;
        }
        let facing = self.duplex == Duplex::Facing;
        let mut leaves: Vec<(usize, bool)> = Vec::new();
        if facing {
            if self.page < pages.len() {
                leaves.push((self.page, false)); // 右叶 = 当前面
            }
            if self.page + 1 < pages.len() {
                leaves.push((self.page + 1, true)); // 左叶 = 下一面
            }
        } else if self.page < pages.len() {
            leaves.push((self.page, false));
        }
        let gap = crate::grid::gutter_w(
            self.content_w,
            self.grid.columns,
            ANCIENT_EDGE,
            self.grid.margin,
        );
        let right_align = !facing && crate::grid::is_narrow(self.viewport_w, self.viewport_h);
        for (idx, back) in leaves {
            let Some(pg) = pages.get(idx) else { continue };
            let (ox, oy) = crate::gloss::leaf_origin(
                facing,
                self.viewport_w,
                self.viewport_h,
                self.page_w,
                self.page_h,
                gap,
                back,
                self.focus_mode,
                right_align,
            );
            if let Some(mut gb) = crate::hit::glyph_at(pg, x - ox, y - oy, &self.leaf_frame(pg)) {
                // 纸面坐标 → 窗口坐标。
                gb.x += ox;
                gb.y += oy;
                return Some(gb);
            }
            // 这一叶没命中（点在别的叶上）：继续试下一叶。
        }
        None
    }

    /// 按下点**就近**找一个字（v1.52.3 拖选起手容差，P3#6）：先原坐标，
    /// 再按「半格四邻 → 一格四邻 → 半格对角 → 一格对角」由近及远探。
    ///
    /// 解决的是「按下落在字间留白 / 地脚 / 标点上，拖动划不选」：字格只在
    /// 按下命中字时才起锚，落在缝里那一按 historically 什么都不发生，
    /// 松手还可能被算成翻页（12 用户模拟报告：通勤族第一天丢过 3 次位置）。
    /// 整书排版只做一次，八个候选点共用。
    pub fn glyph_hit_near(&self, x: f32, y: f32) -> Option<crate::hit::GlyphBox> {
        let pages = self.layout_pages();
        if let Some(gb) = self.glyph_hit_in(&pages, x, y) {
            return Some(gb);
        }
        let cw = self.col_w.max(1.0);
        let ch = self.row_h.max(1.0);
        // 由近及远：先四邻（半格 / 一格），再对角（半格 / 一格）。
        let probes: [(f32, f32); 16] = [
            (0.0, -0.5),
            (0.0, 0.5),
            (-0.5, 0.0),
            (0.5, 0.0),
            (0.0, -1.0),
            (0.0, 1.0),
            (-1.0, 0.0),
            (1.0, 0.0),
            (-0.5, -0.5),
            (0.5, -0.5),
            (-0.5, 0.5),
            (0.5, 0.5),
            (-1.0, -1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
        ];
        for (dx, dy) in probes {
            if let Some(gb) = self.glyph_hit_in(&pages, x + dx * cw, y + dy * ch) {
                return Some(gb);
            }
        }
        None
    }

    /// 该字在全书流里的统一字符下标（`Book::flat_index`）。
    /// 没有书或没有记录时返回 None（浮签上就不显示这一行）。
    pub fn lookup_flat_index(&self) -> Option<usize> {
        let key = self.lookup_key?;
        self.book.as_ref().map(|b| b.flat_index(key))
    }

    pub fn bump_font(&mut self, up: bool) {
        let d = if up { 0.1 } else { -0.1 };
        self.zoom = (self.zoom + d).clamp(0.7, 1.6);
    }

    /// 根据窗口视口与网格配置，用引擎算出「铺满屏幕」的字格尺寸。
    ///
    /// 版式由 [`crate::grid::compute_grid`] 动态计算（列数 8 / 10 / 12 / 14
    /// 可切）：col_w = 可用宽 / 列数，字号 = 0.75 × col_w，行高 = 1.1 × col_w，
    /// 总行数 = floor(可用高 / 行高)。单页 / 双页都自适应铺满视口。
    ///
    /// 关键：可用区域必须扣掉纸面自身的「 chrome 」——
    ///   * 古籍竖笺：sheet p_4(16×2) + border_3+border_1(8×2) + 版框双边
    ///     （`ANCIENT_EDGE` 50）、地脚鱼尾叶码（`ANCIENT_FOOT` 12，脚注模式
    ///     再加 `ANCIENT_NOTES`）；双面对开两叶之间还有一道版心中缝
    ///     （宽约一行字，见 `crate::grid::gutter_w`）。
    ///   * 章题列占「格内一列」：章首页 = 列数-1 个正文列 + 1 个章题列，
    ///     总宽仍是 列数×col_w，不额外外扩。
    ///
    /// 少算任何一项，纸面就会超出视口被裁掉边框（双面对开尤其明显：
    /// 两叶各超宽一截，右叶的版框直接飞出屏幕）。
    pub fn fit_sizes(&mut self) {
        // 内容区：视口减去工具栏(≈44)与状态栏(≈34)及四边留白(≈44)。
        // 专注模式：工具栏 / 状态栏隐藏，且去掉四边留白——单面、双面
        // （对开两叶）都真正撑满整屏，如官网宣传图般只见纸面不见界面。
        // 响应式留白：窄屏（手机竖持）只留一丝边，把宽度全让给字；
        // 宽屏照旧留 44（纸面四周要有「天地」才像刻本）。
        let narrow = crate::grid::is_narrow(self.viewport_w, self.viewport_h);
        // 横向留白（纸面左右天地）：专注模式 0、窄屏一丝边、宽屏留 44。
        let pad = if self.focus_mode {
            0.0
        } else if narrow {
            crate::grid::RESP_MARGIN
        } else {
            44.0
        };
        // 纵向预留：状态栏(~34) + 正文区上下内白(p_4=32) + 安全余量(~4)，
        // 让算出的纸高(content_h)正好等于正文区可用高——这样纸张撑满、不再
        // 有「居中才能遮丑」的残余留白（见 render_ancient 的天头地脚非对称）。
        let v_reserve = if self.focus_mode { 0.0 } else { 70.0 };
        // 版字面板为浮层（可盖住正文下部），不为它预留高度，正文始终铺满。
        let content_h = (self.viewport_h - v_reserve).max(220.0);
        // 目录改为浮层，不占水平空间。
        let content_w = (self.viewport_w - pad).max(220.0);
        self.content_w = content_w;
        self.content_h = content_h;
        let notes_foot = self.show_notes && !self.notes_inline;
        // 古籍竖笺几何预留：版框双边 50、无天头（篇题进版框）、地脚 12 +
        // 页脚注区（脚注模式且开注释时）。列宽 / 字号 / 行高 / 总行数由网格
        // 引擎按列数动态算出（8 / 10 / 12 / 14 列可切）。
        // 章题列不占额外宽度：它是「列数」里的最后一列（章首页少排一列正文
        // 给章题），整面总宽恒为 列数×col_w，因此 extra = 0。
        let edge = ANCIENT_EDGE;
        let head = ANCIENT_HEAD;
        let foot = ANCIENT_FOOT + if notes_foot { ANCIENT_NOTES } else { 0.0 };
        let margin = self.grid.margin;
        let line_scale = self.grid.line_scale;
        let dir = self.grid.dir;
        let zoom = self.zoom;
        // 专注模式：整屏只排一叶，字号不设上限（要的就是满屏大字）；
        // 常规阅读封顶 32px，免得大屏上一列没几个字、页面空旷。
        let fs_max = if self.focus_mode {
            f32::INFINITY
        } else {
            crate::grid::GRID_FS_MAX
        };
        // ---- 渲染模式分派（对开 / 单叶 / 整页）----
        // 自动模式下只在对开 ⇄ 单叶之间来回（整页是「看整体版式」的手动档，
        // 自动切进去会打断阅读）。判据 `grid::should_face` 除宽高比外还要求
        // 单叶宽与字号都过得去——宁可单叶也不牺牲字号。
        let mut mode = if self.auto_duplex && self.layout_mode != crate::grid::LayoutMode::Full {
            if crate::grid::should_face(self.viewport_w, self.viewport_h, self.grid.columns, zoom) {
                crate::grid::LayoutMode::Facing
            } else {
                crate::grid::LayoutMode::Single
            }
        } else {
            self.layout_mode
        };
        // ---- 自动列数（v1.29.0）：按「书的内容」选档，不是一刀切一个默认值 ----
        //
        // 打分用的字号取自**真实的网格试算**（含 32px 上限与列宽回缩），与
        // 最终渲染同一口径——不会出现「算出来 12 列、排出来却是另一码事」。
        // 铺法取决于列数、列数又取决于铺法给的可用宽，故至多两轮收敛。
        if self.columns_auto {
            let avg = self
                .book
                .as_ref()
                .map(|b| b.avg_chapter_chars())
                .unwrap_or(0);
            let mut cols = self.grid.columns;
            for _ in 0..2 {
                let probe = mode;
                let next = crate::grid::auto_columns(
                    |c| {
                        crate::grid::calculate_grid(crate::grid::GridInput {
                            columns: c,
                            mode: probe,
                            content_w,
                            content_h,
                            zoom,
                            edge,
                            head,
                            foot,
                            margin,
                            line_scale,
                            // 试算每一档时按**该档的行款**取比例：八行本字大
                            // 行宽、十四行信笺字小行密——打分才判得准。
                            fs_ratio: crate::grid::LayoutConfig::by_columns(c)
                                .map(|x| x.fs_ratio)
                                .unwrap_or(crate::grid::GRID_FS_RATIO),
                            line_ratio: crate::grid::LayoutConfig::by_columns(c)
                                .map(|x| x.line_ratio)
                                .unwrap_or(0.0),
                            // 留白档照当前偏好走：自动选列数不该把用户选的
                            // 版心率甩在一边（试算与终版同口径）。
                            banxin_ratio: self.banxin_ratio,
                            dir,
                            fs_max,
                        })
                        .m
                        .fs
                    },
                    avg,
                );
                if next == cols {
                    break;
                }
                cols = next;
                // 列数变了可能改变铺法（单叶宽 / 字号门槛），重判一次。
                if self.auto_duplex && self.layout_mode != crate::grid::LayoutMode::Full {
                    mode = if crate::grid::should_face(self.viewport_w, self.viewport_h, cols, zoom)
                    {
                        crate::grid::LayoutMode::Facing
                    } else {
                        crate::grid::LayoutMode::Single
                    };
                }
            }
            if cols != self.grid.columns {
                // 换档 = 整本重排、页码全变。记下当前页首字，等新页排好再回找
                // （此处几何还是旧的，`layout_pages` 会按旧列数算，锚点才准）。
                if self.pending_anchor.is_none() {
                    self.pending_anchor = self.page_anchor();
                }
                self.grid.columns = cols;
                // 版式键跟着走，面板上那一档才会亮。
                if let Some(c) = crate::grid::LAYOUT_CONFIGS.iter().find(|c| c.columns == cols) {
                    self.layout_key = c.key;
                }
            }
        }
        self.layout_mode = mode;
        let facing = mode.spreads();
        // 当帧铺法：整页铺的也是一整叶（两叶 + 中缝），故与对开同为 Facing。
        self.duplex = if facing { Duplex::Facing } else { Duplex::Single };
        // 当前版式（列数 + 该行款的字级 / 行高比例）。
        let cur_cfg = self.current_layout();
        let fit_debug = std::env::var("JIANDU_FIT_DEBUG").is_ok();
        if fit_debug {
            eprintln!("[fit] vw={:.0} vh={:.0} content={:.0}x{:.0} cols={} auto={} avg={} facing={} focus={}", self.viewport_w, self.viewport_h, content_w, content_h, self.grid.columns, self.columns_auto, self.book.as_ref().map(|b| b.avg_chapter_chars()).unwrap_or(0), facing, self.focus_mode);
        }
        // 统一入口 `calculateGrid(columns, mode)`：模式只决定内容区怎么给
        // （对开对半 / 单叶整幅 / 整页按基准版式等比缩放），列宽与字号
        // 的算式三模式完全一致。
        let plan = crate::grid::calculate_grid(crate::grid::GridInput {
            columns: self.grid.columns,
            mode,
            content_w,
            content_h,
            zoom,
            edge,
            head,
            foot,
            margin,
            line_scale,
            // 行款比例（v1.30.0）：与列数一起决定字级与行高——切到八行本
            // 不只是列变少，字也跟着大、行也跟着宽。
            fs_ratio: cur_cfg.fs_ratio,
            line_ratio: cur_cfg.line_ratio,
            // 留白档（v1.33.0）：版心宽 ÷ 单叶可用宽（0.60 复古 / 0.75 标准
            // / 0.90 满屏），天头率随档联动。读自 `Prefs::margin_tier`。
            banxin_ratio: self.banxin_ratio,
            dir,
            fs_max,
        });
        self.full_scale = plan.scale;
        let m = plan.m;
        self.margin_top = m.margin_top;
        self.margin_bot = m.margin_bot;
        self.size_ancient = m.fs;
        if std::env::var("JIANDU_FIT_DEBUG").is_ok() {
            eprintln!(
                "[fit] content_h={:.0} 天头 margin_top={:.1}  地脚 margin_bot={:.1}  比例 {:.2}:1",
                content_h, m.margin_top, m.margin_bot, m.margin_top / m.margin_bot.max(0.01)
            );
        }
        self.size_modern = m.fs;
        self.line_modern = m.row_h;
        self.page_w = m.outer_w;
        self.page_h = m.outer_h;
        self.col_w = m.col_w;
        self.row_h = m.row_h;
        self.grid_rows = m.rows;
        self.grid_min_font = m.min_font_hit;
        // 窄屏硬选多列（典型是手机竖屏选十四行信笺）：字号已被抬到 12px 下限，
        // 纸面会溢出滚动——提示一句，别让用户以为排版坏了。
        if m.min_font_hit && !self.min_font_noticed {
            // 行款兜底（v1.32.0）：字号已锁在 12px 下限，一列排不满行款
            // 字数——明说「无法展示完整的 X 行 Y 字」，绝不悄悄拆行款。
            self.notice = format!(
                "当前窗口过小，无法展示完整的{}行{}字（已锁 {:.0}px 最小字号，可滚动查看）",
                crate::reader::han_num(m.columns),
                crate::reader::han_num(m.target_chars),
                m.fs
            );
            self.min_font_noticed = true;
        } else if !m.min_font_hit {
            self.min_font_noticed = false;
        }
        if fit_debug {
            // 行款 + 双叶几何（v1.32.0）：行款字数、每列实际字数、单叶纸宽、
            // 中缝宽，以及「两叶 + 中缝」相对内容区的占比——中缝是否居中、
            // 左右是否均分，一眼可查。
            let gap = crate::grid::gutter_w(
                content_w,
                self.grid.columns,
                ANCIENT_EDGE,
                self.grid.margin,
            );
            let row_w = if facing { 2.0 * m.outer_w + gap } else { m.outer_w };
            eprintln!(
                "[fit] → fs={:.1} col_w={:.1} row_h={:.1} rows={} (行款 {}字/列, 实际 {}字) paper={:.0}x{:.0} leaf={:.0} gap={:.1} row_w={:.0}/content_w={:.0} ({:.0}%)",
                m.fs, m.col_w, m.row_h, m.rows, m.target_chars, m.chars_per_col, m.outer_w, m.outer_h,
                if facing { (content_w - gap) / 2.0 } else { m.outer_w },
                gap, row_w, content_w, row_w / content_w * 100.0
            );
        }
    }

    /// 当前版式配置：数据驱动的唯一数据源（见 [`crate::grid::LAYOUT_CONFIGS`]）。
    ///
    /// 走 `migrate_key`：旧存档里可能还写着已删除的「letter14（信笺）」，
    /// 一律迁到同列数的 standard14，几何不变、用户察觉不到。
    pub fn current_layout(&self) -> &'static crate::grid::LayoutConfig {
        let key = crate::grid::LayoutConfig::migrate_key(self.layout_key);
        crate::grid::LayoutConfig::by_key(key)
            .unwrap_or_else(crate::grid::LayoutConfig::default)
    }

    /// 统一的版式切换入口（数据驱动）：按配置键换版式，**保持当前阅读进度**
    /// （首字锚点 = charIndex）不丢失，切换瞬间重排当前页。
    ///
    /// 取代原先散落的 `set_letter_mode` / `set_columns` 两套分支——它们都收敛
    /// 到这里，UI 也只调这一处。新增版式（如 16 列）只需在注册表加一行，
    /// 这里与 UI 零改动。
    pub fn set_layout(&mut self, key: &str) {
        let cfg = match crate::grid::LayoutConfig::by_key(key) {
            Some(c) => c,
            None => return,
        };
        // 手动选了具体的一档 = 锁定：自动档让位，否则下一帧 `fit_sizes`
        // 又按内容把它算回去，用户会觉得「我点了 8 列怎么自己跳走了」。
        // （自动档内部是直接改 `grid.columns` / `layout_key`，不走这里。）
        self.columns_auto = false;
        if self.layout_key == cfg.key {
            return; // 同一档，跳过无谓重排
        }
        // 锚点：当前页第一个字（段首缩进占位也算），切版前后都用它定位。
        let anchor = self.page_anchor();
        self.layout_key = cfg.key;
        self.grid.columns = cfg.columns;
        self.grid.dir = cfg.dir;
        self.grid.margin = cfg.margin;
        self.fit_sizes();
        self.restore_anchor(anchor);
        self.sync_current_chapter();
    }

    /// 当前页首字的 **charIndex 锚点**（章, 段, 偏移）。
    ///
    /// 切列数 / 切模式 / 改注文排法都会**整本重排**，页码随之变化；
    /// 记下锚点再按它回找新页码，才不会「换个版式就跳回卷首」。
    pub fn page_anchor(&self) -> Option<(usize, usize, usize)> {
        self.layout_pages()
            .get(self.page)
            .and_then(|p| p.glyphs.first())
            .map(|g| (g.chapter, g.para, g.offset))
    }

    /// 按锚点把 `self.page` 重定位到「含这个字」的那一面（找不到则原位不动）。
    pub(crate) fn restore_anchor(&mut self, anchor: Option<(usize, usize, usize)>) {
        let Some((c, pa, o)) = anchor else { return };
        if let Some(pi) = self.layout_pages().iter().position(|pg| {
            pg.glyphs
                .iter()
                .any(|g| (g.chapter, g.para, g.offset) == (c, pa, o))
        }) {
            self.page = pi;
        }
    }

    // ------------------------------------------------------------------
    // 天头眉批（v1.37.0）
    // ------------------------------------------------------------------

    /// **天头带**在窗口内容区里的矩形 (x, y, w, h)。
    ///
    /// 天头 = 纸面顶缘往下 `margin_top` 那一条（版框以上的留白）。
    /// `x/y/w` 取当帧纸面矩形（`page_rect`，由 `render_screen` 的探针写入），
    /// 高就是引擎算出的天头——换留白档、拖窗口都会立刻变，所以眉批**只存
    /// 比例不存像素**（见 `types::HeadNote`），缩放时自动等比跟随。
    pub fn head_band(&self) -> Option<(f32, f32, f32, f32)> {
        let (x, y, w, h) = self.page_rect.get()?;
        let bh = self.margin_top.min(h);
        // 天头太窄（专注模式几乎不留白）：不开写，免得点一下就误进输入态。
        if bh < crate::reader::HEAD_BAND_MIN || w <= 0.0 {
            return None;
        }
        Some((x, y, w, bh))
    }

    /// 窗口坐标落进天头带 → 归一化 `(u, v)`；不在天头里返回 `None`。
    pub fn head_band_hit(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (bx, by, bw, bh) = self.head_band()?;
        if x < bx || x > bx + bw || y < by || y > by + bh {
            return None;
        }
        Some(((x - bx) / bw, (y - by) / bh))
    }

    /// 在天头 `(u, v)` 处**起一条眉批**：进入编辑态，草稿就地竖排显示。
    ///
    /// 复用夹注那套输入通道（`note_draft` / `note_cursor` / `note_focus`），
    /// 故中文输入法、退格、⏎ 保存、Esc 取消全都是现成的——只有「画在哪、
    /// 存到哪」不同（见 `commit_note_edit` 的天头分支）。
    pub fn open_head_note(&mut self, u: f32, v: f32) {
        let Some(key) = self.page_anchor() else {
            self.notice = "此页无法题眉批".to_string();
            return;
        };
        self.cancel_note_edit();
        self.head_anchor = Some((key, u, v));
        // v1.48.0：起新批即退出「改写」与「唤醒」态——点空白是新建，
        // 点已有眉批才是管理（防误触，见 `head_active`）。
        self.head_edit = None;
        self.head_active = None;
        self.note_draft.clear();
        self.note_cursor = 0;
        self.note_editing = true;
        // 开写那一下的 mouseup 会重新命中正文，别让它「点别处自动保存」把
        // 刚开的小框立刻关掉——与浮层笔记框同一个坑（见 `note_suppress_commit`）。
        self.note_suppress_commit = true;
        self.popup = None;
        self.notice = "眉批：键入后 ⏎ 落笔，Esc 弃".to_string();
    }

    /// v1.48.0 眉批管理：**就地改写**一条已有眉批（预填草稿，⏎ 原位换文）。
    ///
    /// 位置（u/v）原样不动——改写不重新避让，否则 `free_u` 会把自己撞开；
    /// 原条在编辑期间暂时收起（`head_edit`），草稿就地竖排显示。
    pub fn open_head_note_edit(&mut self, idx: usize) {
        let Some(n) = self.head_notes.get(idx).cloned() else {
            return;
        };
        self.cancel_note_edit();
        self.head_anchor = Some((n.key, n.u, n.v));
        self.head_edit = Some(idx);
        self.head_active = None;
        self.note_draft = n.text.clone();
        self.note_cursor = n.text.chars().count();
        self.note_editing = true;
        self.note_suppress_commit = true;
        self.popup = None;
        self.notice = "改眉批：⏎ 落笔，Esc 弃".to_string();
    }

    /// v1.48.0 眉批管理：**抹掉**一条眉批——数据、书稿 JSON、画布三处同时
    /// 消失（`persist_book_json` 回写，下次开卷不再有它）。
    pub fn delete_head_note(&mut self, idx: usize) {
        if idx >= self.head_notes.len() {
            return;
        }
        self.head_notes.remove(idx);
        if let Some(book) = self.book.as_mut() {
            book.head_notes = self.head_notes.clone();
        }
        self.persist_book_json();
        self.head_active = None;
        self.head_edit = None;
        self.notice = format!("已抹眉批 · 尚余 {} 条", self.head_notes.len());
    }

    /// 存一条天头眉批：同位置已有就**向左挪一列**，挪出左缘绕回右缘；
    /// 写完立刻回写书稿 JSON（下次开卷还在）。
    /// **从当前选区起一条天头眉批**（v1.55.0 新制）。
    ///
    /// 链路就是「选区 → `flat_index` → `anchor_char_index`」：
    /// 选区起点那个字的字形键经 `Book::flat_index` 换成全书扁平字符下标，
    /// 存进 `MarginNote`。之后每一帧都由这个下标反算出那个字的屏幕坐标，
    /// 翻页 / 改字号 / 换列数都不丢锚（见 `reader::render_margin_layer`）。
    ///
    /// 只进编辑态、不弹窗：草稿就地竖排显示在天头（由画布层排位）。
    pub fn start_margin_note(&mut self) {
        let Some(book) = self.book.as_ref() else {
            self.notice = "尚未开卷".to_string();
            return;
        };
        // 有拖选取选区起点，否则取当前悬停 / 点击的那一个字。
        let key = self
            .selection_range()
            .map(|(a, _)| a)
            .or(self.hover_key)
            .or_else(|| self.page_anchor());
        let Some(key) = key else {
            self.notice = "先选中要批的文字".to_string();
            return;
        };
        let at = book.flat_index(key);
        self.cancel_note_edit();
        self.margin_draft_at = Some(at);
        self.note_draft.clear();
        self.note_cursor = 0;
        self.note_editing = true;
        // 开写那一下的 mouseup 会重新命中正文，别让它「点别处自动保存」
        // 把刚开的小框立刻关掉（与浮层笔记框同一个坑）。
        self.note_suppress_commit = true;
        self.popup = None;
        self.notice = "眉批：⏎ 存 · Esc 弃".to_string();
    }

    /// **改写一条已有眉批**（按 id 找到它，草稿预填原文）。
    ///
    /// 锚点照旧由 `anchor_char_index` 决定，改的只是批语文字——故位置不动，
    /// 换文后天头上的字柱与引线原样在那儿。
    pub fn start_margin_note_edit(&mut self, id: u64) {
        let Some(n) = self.margin_notes.iter().find(|m| m.id == id) else {
            return;
        };
        let at = n.anchor_char_index;
        let text = n.content.clone();
        self.cancel_note_edit();
        self.margin_draft_at = Some(at);
        self.margin_edit_id = Some(id);
        self.note_draft = text.replace(['\n', '\r'], " ");
        self.note_cursor = self.note_draft.chars().count();
        self.note_editing = true;
        self.note_suppress_commit = true;
        self.popup = None;
        self.notice = "改眉批：⏎ 存 · 清空即删 · Esc 弃".to_string();
    }

    /// 存一条天头眉批（新制）：`anchor_char_index` 绝对锚定。
    pub fn push_margin_note(&mut self, anchor: usize, text: String) {
        let text: String = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        if text.trim().is_empty() {
            return;
        }
        let id = self
            .book
            .as_ref()
            .map(|b| b.next_margin_id())
            .unwrap_or_else(|| self.margin_notes.iter().map(|m| m.id).max().unwrap_or(0) + 1);
        self.margin_notes
            .push(crate::types::MarginNote::new(id, anchor, text));
        if let Some(book) = self.book.as_mut() {
            book.margin_notes = self.margin_notes.clone();
        }
        self.persist_book_json();
        self.notice = format!("已题眉批 · 共 {} 条", self.margin_notes.len());
    }

    /// 删一条天头眉批（按 id）。
    pub fn delete_margin_note(&mut self, id: u64) {
        let before = self.margin_notes.len();
        self.margin_notes.retain(|m| m.id != id);
        if self.margin_notes.len() == before {
            return;
        }
        if self.margin_hover == Some(id) {
            self.margin_hover = None;
            self.margin_hover_key = None;
        }
        if let Some(book) = self.book.as_mut() {
            book.margin_notes = self.margin_notes.clone();
        }
        self.persist_book_json();
        self.notice = format!("眉批已删 · 余 {} 条", self.margin_notes.len());
    }

    pub fn push_head_note(&mut self, key: (usize, usize, usize), u: f32, v: f32, text: String) {
        // 同一处已有 → 向左让开一列（判据见 `HeadNote::free_u`）。
        let u = crate::types::HeadNote::free_u(&self.head_notes, key, u, v);
        let text: String = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        self.head_notes
            .push(crate::types::HeadNote::new(key, u, v, text));
        if let Some(book) = self.book.as_mut() {
            book.head_notes = self.head_notes.clone();
        }
        self.persist_book_json();
        self.notice = format!("已题眉批 · 共 {} 条", self.head_notes.len());
    }

    /// **切渲染模式**（对开 / 单叶 / 整页）：同样保住阅读进度。
    ///
    /// 与 `set_layout` 走同一套锚点机制——先记当前页首字（charIndex），
    /// 换模式重排后按这个字回找新页码，绝不跳到第一页。
    pub fn set_layout_mode(&mut self, mode: crate::grid::LayoutMode) {
        if self.layout_mode == mode {
            return;
        }
        let anchor = self.page_anchor();
        self.layout_mode = mode;
        // 手动选模式即视为「不再自动响应屏幕」——否则下一帧 `fit_sizes`
        // 又按宽高比把它改回去，面板上的选择看着像没生效。
        // 想回到自动，重新打开「自动版式」开关即可。
        self.auto_duplex = false;
        self.fit_sizes();
        self.restore_anchor(anchor);
        self.sync_current_chapter();
        self.notice = format!(
            "版式：{}（{}）· {} 列",
            mode.label(),
            mode.hint(),
            self.grid.columns
        );
        self.persist();
    }

    /// **列数的唯一读口**（v1.35.0）：`None` = 自动（按书与可用宽每帧算），
    /// `Some(n)` = 锁死 n 列。
    ///
    /// 原先「版式」浮层读 `layout_key`、版字面板读 `grid.columns` +
    /// `columns_auto`，两处各自判断、同一件事两副面孔；现在 UI 一律只读这
    /// 一处，写入只走 [`set_columns_pref`]。底层引擎仍照旧吃
    /// `grid.columns` + `columns_auto`，换算在这里做一次，引擎零改动。
    pub fn columns_pref(&self) -> Option<usize> {
        if self.columns_auto {
            None
        } else {
            Some(self.grid.columns)
        }
    }

    /// **列数的唯一写入口**（v1.35.0）：接 UI 的 `Option<usize>`，
    /// 落到引擎的 `columns_auto` + `set_columns`。
    ///
    /// `None`（自动）只是放开锁，让下一帧 `fit_sizes` 按内容重算，不必
    /// 保锚重排；`Some(n)` 走 `set_layout`，保住 charIndex 锚点不跳页。
    pub fn set_columns_pref(&mut self, want: Option<usize>) {
        match want {
            None => {
                if self.columns_auto {
                    return;
                }
                self.columns_auto = true;
                self.fit_sizes();
            }
            Some(n) => {
                if !self.columns_auto && self.grid.columns == n {
                    return;
                }
                self.set_columns(n);
            }
        }
    }

    /// 设列数（8 / 10 / 12 / 14）：列数档位全部来自配置，统一走 [`set_layout`]，
    /// 因此同样保持阅读进度、不跳回卷首。
    pub fn set_columns(&mut self, n: usize) {
        let key = crate::grid::LAYOUT_CONFIGS
            .iter()
            .find(|c| c.columns == n)
            .map(|c| c.key)
            .unwrap_or_else(crate::grid::LayoutConfig::default_key);
        self.set_layout(key);
    }

    /// 设四周留白比例（0..1，近似四边同留白）。
    pub fn set_margin(&mut self, m: f32) {
        self.grid.margin = m.clamp(0.0, 0.2);
    }

    /// 设**留白档**（v1.33.0）：版心宽 ÷ 单叶可用宽。
    ///
    /// 三档（[`crate::grid::BANXIN_RATIOS`]）：0.60 宽（复古）/ 0.75 中
    /// （标准，默认）/ 0.90 窄（满屏）。列宽由版心率反推、天头率随档联动
    /// （见 `crate::grid::top_frac_for`），行款字数不受影响——这一档只管
    /// 「文字区占多宽」，不管「一行几字」。
    ///
    /// 几何在下一帧 `fit_sizes` 重算（分页只数字数，不随宽度变，故不必
    /// 保锚重排）；偏好**读-改-写**落盘，老存档其余字段不受影响。
    pub fn set_banxin_ratio(&mut self, ratio: f32) {
        self.banxin_ratio = ratio.clamp(0.35, 1.0);
        let mut prefs = crate::prefs::Prefs::load();
        prefs.margin_tier = crate::grid::BANXIN_RATIOS
            .iter()
            .position(|r| (r - self.banxin_ratio).abs() < 0.005)
            .unwrap_or(crate::grid::BANXIN_TIER_DEFAULT);
        prefs.save();
    }

    /// 切换「双行夹注 ⇄ 页脚注」时保持阅读位置：记下当前页首字锚点，
    /// 切换并重分页后定位到含该字的新页码，避免一开关夹注就跳回别处。
    pub fn set_notes_inline(&mut self, inline: bool) {
        if self.notes_inline == inline {
            return;
        }
        // 锚点：当前页第一个字（章首缩进占位也算）。
        let anchor = self
            .layout_pages()
            .get(self.page)
            .and_then(|p| p.glyphs.first())
            .map(|g| (g.chapter, g.para, g.offset));
        self.notes_inline = inline;
        if let Some((c, pa, o)) = anchor {
            if let Some(pi) = self.layout_pages().iter().position(|pg| {
                pg.glyphs
                    .iter()
                    .any(|g| (g.chapter, g.para, g.offset) == (c, pa, o))
            }) {
                self.page = pi;
            }
        }
        self.sync_current_chapter();
    }

    /// 取一本书的完整 `Book`（v1.50.0）。
    ///
    /// * **虚拟化条目**（`store_id` 有值）：读骨架 json（只有题名与目录），
    ///   再 `stream::hydrate_all` 按章把正文取回来——正文平时根本不在内存；
    /// * **老条目**：走旧的一次性导入（json / txt 整本读入）。
    fn load_book_for(entry: &crate::library::ShelfEntry) -> anyhow::Result<Book> {
        if let Some(sid) = &entry.store_id {
            let text = std::fs::read_to_string(&entry.path)?;
            let mut book: Book = serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("书稿残损：{e}"))?;
            book.store_id = Some(sid.clone());
            book.ensure_toc();
            crate::stream::hydrate_all(&mut book);
            return Ok(book);
        }
        crate::importer::import_from_path(&entry.path)
    }

    /// 打开书架上第 idx 本书：直接进正文上次读的那一面，并恢复进度与笔记。
    pub fn open_book(&mut self, idx: usize) {
        let Some(entry) = self.library.books.get(idx).cloned() else {
            return;
        };
        let nt = entry.notes.clone();
        let ul = entry.underlines.clone();
        match Self::load_book_for(&entry) {
            Ok(mut book) => {
                book.ensure_toc();
                self.current = idx;
                // 只保留古籍竖笺（现代横笺已移除）；列数 / 留白沿用用户当前 grid 配置。
                // 单页 / 对开不再读档案，由屏幕方向在 fit_sizes 里决定。
                self.script = Script::Ancient;
                self.grid.dir = GridDir::Vertical;
                // 恢复上次阅读进度。
                self.page = entry.last_page;
                // 全书统计（章数 / 字数）开卷时算一次：章末那张「篇·章·字」
                // 小表要用到「全書」一行，放渲染里逐帧扫全书太贵。
                self.book_stat = (
                    book.chapters.len(),
                    book.chapters
                        .iter()
                        .flat_map(|c| c.paragraphs.iter())
                        .map(|p| {
                            p.text
                                .chars()
                                .filter(|ch| !ch.is_whitespace() && !crate::types::is_punct(*ch))
                                .count()
                        })
                        .sum(),
                );
                // v1.55.0：读档先规整——旧格式（u/v 眉批）直接清空、版本号
                // 升到当前。用户明示旧数据是测试乱写的，不做迁移。
                book.normalize_after_load();
                // 天头眉批开卷时从书稿里取一次（之后增删改都回写同一份）。
                self.head_notes = book.head_notes.clone();
                self.margin_notes = book.margin_notes.clone();
                self.head_anchor = None;
                self.head_press = None;
                // v1.48.0：换书即退出眉批管理 / 改写态（下标是属于上一本书的）。
                self.head_active = None;
                self.head_edit = None;
                // 开发用：JIANDU_HEAD_NOTE=1 会在**第一帧渲染时**自动题两条
                // 同位置的眉批（第二条自动向左让开一列）——注入点放在
                // `render_head_notes` 里，因为此刻才有页锚点（开卷那一刻
                // 版面还没排，`page_anchor()` 是 None）。只动内存，不写 JSON。
                self.book = Some(book);
                // v1.60.0：换书必须作废分页器与题署缓存——两本章数恰好
                // 相同时若不清，会拿上一本的面数索引去查这一本。
                self.pager = None;
                self.inscription_need = None;
                self.notes = nt;
                self.underlines = ul.into_iter().collect();
                self.bookmarks = entry.bookmarks.clone();
                self.show_share_panel = false;
                self.show_notes_panel = false;
                self.note_editing = false;
                self.note_draft.clear();
                self.note_cursor = 0;
                self.note_pos = None;
                self.rename_idx = None;
                self.rename_draft.clear();
                self.rename_cursor = 0;
                self.delete_confirm = None;
                self.shelf_expand = None;
                self.shelf_recat = None;
                self.shelf_searching = false;
                self.popup = None;
                self.sel_anchor = None;
                self.sel_focus = None;
                self.dragging = false;
                // 单页 / 双面对开交由 fit_sizes 依屏幕方向决定（横屏对开、竖屏单页）。
                // 点书架的书**直接进正文上次读的那一面**（`self.page` 已在上面从
                // 档案恢复），不再先落书衣封面——翻书就该回到原处。
                // （v1.37.0 UI 瘦身撤掉了阅读页工具栏那枚「封面」，
                //  书衣只在书架这一侧看了。）
                self.screen = Screen::Reading;
                self.notice.clear();
                // 换书即换场景：上一本的导出收尾话不留（v1.52.3）。
                self.toast = None;
                // 恢复进度后立刻对齐当前章，目录朱砂红点才不会错位。
                self.sync_current_chapter();
            }
            Err(e) => {
                self.notice = format!("打开失败：{e}");
            }
        }
    }

    /// 把当前进度写回书架档案。
    pub fn persist(&mut self) {
        let total = self.page_count();
        let idx = self.current;
        let page = self.page;
        let script = self.script.as_str().to_string();
        let duplex = match self.duplex {
            Duplex::Facing => "facing",
            Duplex::Single => "single",
        };
        // 当前卷册：书架「当前卷册」一栏直接显示它，不必再开书。
        let chapter = self
            .book
            .as_ref()
            .and_then(|b| b.chapters.get(self.current_chapter))
            .map(|c| c.title.clone())
            .unwrap_or_default();
        self.library
            .touch(idx, page, total, &script, duplex, &chapter);
    }

    /// 回到书架。
    pub fn back_to_shelf(&mut self) {
        self.persist();
        self.screen = Screen::Shelf;
        self.book = None;
        // 回书架时收起书的临时态；搜索词与视图/排序保留（用户偏好）。
        self.shelf_searching = false;
        self.shelf_expand = None;
        self.shelf_recat = None;
        self.delete_confirm = None;
    }

    /// 「继续阅读」：直接打开最近读过的那本；一本书都没读过则打开第一本。
    pub fn shelf_continue(&mut self) {
        let idx = self
            .library
            .books
            .iter()
            .enumerate()
            .filter(|(_, e)| e.last_read > 0 || e.last_page > 0)
            .max_by_key(|(_, e)| e.last_read)
            .map(|(i, _)| i)
            .or(if self.library.books.is_empty() {
                None
            } else {
                Some(0)
            });
        if let Some(i) = idx {
            self.open_book(i);
        } else {
            self.notice = "书架还是空的，先导入一本".into();
        }
    }

    /// 改一本书的归类（书架「改归类」浮层用）。
    pub fn recat_book(&mut self, idx: usize, cat: &str) {
        self.library.set_category(idx, cat);
        self.shelf_recat = None;
        self.notice = if cat.is_empty() {
            "已恢复按书名自动归类".to_string()
        } else {
            format!("已归入「{cat}」")
        };
    }

    /// 把文件导入书架（v1.50.0：**后台流式导入**）。
    ///
    /// 这里**不解析**——解析交给 `ImportJob` 那条线程（逐章落盘），
    /// 主线程立刻返回，用户看到的是「雕版刻印中…」。落盘完成后由
    /// `poll_import` 登记入架。
    ///
    /// md / html / json 这些通常不大的格式也走同一条路（流式管线内部对
    /// 它们仍是「整篇解析 + 逐章落盘」），口径统一，不必维护两套入口。
    pub fn add_import(&mut self, path: PathBuf) {
        if self.import_job.is_some() {
            self.notice = "还有一部书正在刻印，请稍候".into();
            return;
        }
        self.notice.clear();
        self.import_job = Some(ImportJob::start(path));
    }

    /// 漏斗兜底入架（v1.62.0）：整本书写成一个书稿 json，再走旧的非流式入架。
    ///
    /// 为什么不做流式：流式入库（`admit_streamed`）要求正文已经在 `chapters.db`
    /// 里、书架只留骨架；漏斗是「一次性解析整本」，没有落库步骤。整本入库对
    /// 脏文件足够——这类书本来就不大，且是少数派路径，不占用常规导入的性能。
    fn admit_whole(&mut self, src: &std::path::Path, book: &Book) -> anyhow::Result<usize> {
        let dir = crate::library::Library::books_dir();
        std::fs::create_dir_all(&dir)?;
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("book")
            .to_string();
        let mut dst = dir.join(format!("{stem}.json"));
        let mut n = 2;
        while dst.exists() {
            let cand = dir.join(format!("{stem}-{n}.json"));
            if !cand.exists() {
                dst = cand;
                break;
            }
            n += 1;
        }
        crate::importer::write_book_json(book, &dst)?;
        self.library.import_path(&dst)
    }

    /// 每帧看一眼后台导入：**没完**就继续画进度，**完了**就登记入架。
    pub fn poll_import(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.import_job.take() else {
            return;
        };
        let Some(res) = job.take_done() else {
            // 没完成：先看一眼**看门狗**（v1.69.0）——进度一直不动就判定
            // 卡死，报错并收掉浮层，绝不让用户对着 0% 干等到天亮。
            let now = std::time::Instant::now();
            let sig = format!("{:.4}|{}", (job.pct() * 1000.0).round(), job.label());
            let mut job = job;
            if sig != job.sig {
                job.sig = sig;
                job.moved = now;
            }
            let budget = if job.moved == job.started {
                IMPORT_STALL_START
            } else {
                IMPORT_STALL_LATER
            };
            if now.duration_since(job.moved) >= budget {
                // 卡死了：撤掉任务（浮层随之消失）+ 弹一句人话。
                self.import_job = None;
                self.notice = if job.moved == job.started {
                    "解析失败，文件可能损坏或非标准电子书（进度一直没动，已中止）".to_string()
                } else {
                    format!(
                        "解析超时：{} 秒没有任何进展，已中止（文件可能损坏）",
                        now.duration_since(job.moved).as_secs()
                    )
                };
                cx.notify();
                return;
            }
            // 还在跑（且有推进）：任务放回去，请求下一帧继续刷进度条。
            self.import_job = Some(job);
            cx.notify();
            return;
        };
        match res {
            Ok((book, sid, src)) => {
                let title = book.title.clone();
                // `sid` 为空 = 走的是漏斗兜底（整本入库），不是流式骨架。
                let admitted = if sid.is_empty() {
                    self.admit_whole(&src, &book)
                } else {
                    self.library.admit_streamed(&src, &book, &sid)
                };
                match admitted {
                    Ok(_) => self.notice = format!("已上架：{title}"),
                    Err(e) => self.notice = format!("导入失败：{e}"),
                }
            }
            Err(e) => {
                // JSON 格式错误已是古籍风浮窗文案，原样弹出；其余补「导入失败：」。
                self.notice = if e.contains("竹简散乱") {
                    e
                } else {
                    format!("导入失败：{e}")
                };
            }
        }
        cx.notify();
    }

    /// 导入进度浮层（v1.50.0）：极简三行——一句「雕版刻印中… 34%」、
    /// 文件名、一道朱砂细线。不做转圈动画，不做现代进度对话框。
    fn import_overlay(name: &str, label: &str, pct: f32) -> Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::SongTi,
            crate::types::Script::Ancient,
        );
        let pct_i = (pct * 100.0).round() as i32;
        let bar_w = 210.0;
        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x14100A55))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(9.0))
                    .px(px(34.0))
                    .py(px(24.0))
                    .rounded(px(10.0))
                    .bg(rgb(0xF5F0E6))
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(15.0))
                            .text_color(rgb(0x2A241E))
                            .child(SharedString::from(format!("{label}… {pct_i}%"))),
                    )
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(11.0))
                            .text_color(rgb(0x9A9184))
                            .child(SharedString::from(name.to_string())),
                    )
                    .child(
                        div()
                            .w(px(bar_w))
                            .h(px(2.0))
                            .bg(rgb(0xC9BDA6))
                            .child(
                                div()
                                    .h_full()
                                    .w(px(bar_w * pct.clamp(0.0, 1.0)))
                                    .bg(rgb(0xC3272D)),
                            ),
                    ),
            )
    }

    /// 从内置书库下载一本书到书架（离线可用：书库随二进制内置）。
    pub fn download_book(&mut self, key: &str) {
        match self.library.download(key) {
            Ok(i) => {
                let title = self
                    .library
                    .books
                    .get(i)
                    .map(|b| b.title.clone())
                    .unwrap_or_default();
                self.notice = format!("已下载到书架：{title}");
            }
            Err(e) => {
                self.notice = format!("下载失败：{e}");
            }
        }
    }

    /// 弹出系统文件选择器导入书籍。
    pub fn import_dialog(&mut self, cx: &mut Context<Self>) {
        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // AsyncApp 是 Clone：把「借用的上下文」换成「owned 句柄」，
            // async move 块才能安全持有它跨 await（借用不能进 'static future）。
            let mut cx = cx.clone();
            async move {
                // v1.52.4：**前台直调**（与导出保存框同一修复）——rfd 同步
                // pick_files 必须跑在主线程，过 background_executor 会被
                // GPUI 主循环挤掉、静默返回 None（点了「导入」没反应）。
                let picked = rfd::FileDialog::new()
                    .set_title("导入书籍到书架")
                    // 统一管线支持的所有格式都在这里列上：html / xml
                    // 走同一条清洗链（剥 script/nav、em 转夹注）。
                    .add_filter(
                        "书籍 (md / txt / html / xml / epub / docx / json)",
                        &[
                            "md", "markdown", "txt", "html", "htm", "xhtml", "xml",
                            "epub", "docx", "json",
                        ],
                    )
                    .add_filter("全部文件", &["*"])
                    .pick_files();

                if let Some(paths) = picked {
                    this.update(&mut cx, |this, cx| {
                        for p in paths {
                            this.add_import(p);
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // 导出：印制笺卡（PNG） / 导出纯文本（MD）
    // ------------------------------------------------------------------

    // v1.71.0：旧 `page_screen_rect` + `capture_page_png`（走 `screencapture`
    // 子进程、要屏幕录制权限、会把顶栏与页码一起截进去）已整体删除。
    // 现在「导出当前页图片」改由 `reader::Root::render_leaf_png` **离屏重绘**
    // （CoreGraphics 位图，长边 2160 的 2× 高清，画面只有纸与字），
    // 见 `export_page_png_shoot`。纸面矩形探针 `page_rect` 仍由渲染帧写入，
    // 只用于取本叶的长宽比（`leaf_png_size`）。
    /// 『印制笺卡（PNG）』按钮：截当前纸面 → 弹系统保存对话框 → 落盘。
    ///
    /// v1.52.3：**先收面板再截**（与 PDF 导出同序）。`screencapture` 截的是
    /// 屏幕而非渲染树，事件处理器里当场截，截到的还是带着目录 / 浮层的旧帧
    /// （12 用户模拟报告 P2#2）。故这里只收面板 + 登记两段式作业
    /// （`png_export = Some(Warm)`），真正截屏在 `render` 循环的 `Shoot`
    /// 分支——那时「无面板」纸面已合成上屏。
    ///
    /// 保存顺序**不能反**：先截后弹。保存面板一浮起，再截就截到面板自己了。
    /// 用户取消 = 正常结果：清掉临时文件、提示一句，不 panic、不报错红。
    /// 导出作业的**统一入口闸门**：已在跑就拒绝并提示一句，否则占位。
    ///
    /// 返回 `false` = 已有导出在跑，调用方应当立刻 return。
    /// 每个对应出口都必须 `export_busy = false`，否则导出会**永久锁死**
    /// （此后点任何导出都只回一句「已有导出在进行中」）。
    fn export_begin(&mut self, cx: &mut Context<Self>) -> bool {
        if self.export_busy {
            self.notice = "已有导出在进行中，请先完成或取消".to_string();
            cx.notify();
            return false;
        }
        self.export_busy = true;
        true
    }

    pub fn export_page_png(&mut self, _window: &Window, cx: &mut Context<Self>) {
        // 导出已在推进中：忽略重复点击（两段式作业会自行收尾）。
        // `export_begin` 顺手把统一闸门占住——两段式跨帧，Shoot 帧才收摊。
        if self.png_export.is_some() || !self.export_begin(cx) {
            return;
        }
        // 导出前收起所有面板 / 浮层，纸面只留正文（与 begin_pdf_export 同序）。
        self.set_ui_state(AppUIState::None);
        self.close_transient_ui();
        self.png_export = Some(PngStage::Warm);
        cx.notify();
        // ⚠️ 这里**不能**调 `window.request_animation_frame()`（v1.53.0 修崩）：
        // GPUI 0.2.2 的 `request_animation_frame` 内部走
        // `current_view().unwrap()`——`rendered_entity_stack` 只在 paint /
        // prepaint 阶段非空；事件处理器（on_mouse_down → 本函数）里调用时
        // 栈为空，`unwrap()` 直接 panic。panic 穿过 GPUI↔ObjC 的
        // `handle_view_event` FFI 边界会被 `panic_cannot_unwind` 强杀
        // （表现为「点导出就闪退」，见 ~/.jiandu/panic.log 的栈：
        // begin_pdf_export → request_animation_frame → current_view）。
        // 首帧改由 `cx.notify()` 触发重绘；`render` 里的 pdf/png 分支
        // 身处绘制阶段，那时再 `request_animation_frame()` 才安全
        // （目录滑入动画一直都是这么驱动的）。
    }

    /// PNG 笺卡导出的实段（`PngStage::Shoot` 帧由 `render` 循环调用）：
    /// **离屏重绘**本叶 → 弹系统保存框 → 把 PNG 字节直接写到用户选的路径。
    ///
    /// v1.71.0：不再 `screencapture`（要屏幕录制权限、分辨率随窗口、会把顶栏
    /// 与页码截进去），改走 `render_leaf_png()` 的 CoreGraphics 离屏光栅——
    /// 于是既不需要「等一帧干净纸面」，也不再有临时文件要清理。
    /// 用户取消 = 正常结果：提示一句，不 panic、不报错红。
    fn export_page_png_shoot(&mut self, _window: &Window, cx: &mut Context<Self>) {
        // 两段式作业到这一帧就结束了：**无论成败都先收摊**。不复位的话
        // `png_export` 会永远停在 Some，此后再点「导出当前页图片」无事发生。
        self.png_export = None;
        let png = match self.render_leaf_png() {
            Ok(p) => p,
            Err(e) => {
                self.export_busy = false;
                self.notice = e.clone();
                self.toast = Some(e);
                cx.notify();
                return;
            }
        };
        let title = self
            .book
            .as_ref()
            .map(|b| b.title.clone())
            .unwrap_or_else(|| "笺读".to_string());
        let default = format!(
            "笺读_{}_第{}叶.png",
            crate::exporter::safe_file_stem(&title),
            self.page + 1
        );
        cx.spawn(
            move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    // v1.52.4：**前台直调**，不再过 background_executor。
                    // rfd 同步 save_file 靠 dispatch_sync(main) 跑模态面板，
                    // 推后台线程会被 GPUI 主循环挤掉 → 静默返回 None（看似
                    // 「点了导出没反应」）。cx.spawn 的 async 体由 GPUI 前台
                    // executor 在主线程 poll，is_main_thread() 为 true，
                    // 面板即原生模态显示。详见 exporter::pick_save_path 注释。
                    let picked = crate::exporter::pick_save_path(
                        "印制本叶（高清 PNG）",
                        &default,
                        "PNG 图片",
                        &["png"],
                    );
                    // 三种结局（用户取消 / 写盘成功 / 写盘失败）**在同一处收口**：
                    // 提示语先算成值，最后只做一次 update——闸门复位与提示
                    // 落地绑在一起，杜绝「某个分支忘了复位 → 导出永久锁死」。
                    let (msg, toast) = match picked {
                        // 用户点了取消：这不是错误。
                        None => (
                            "已取消保存".to_string(),
                            "已取消保存".to_string(),
                        ),
                        // v1.73.0：走 `save_bytes`——补后缀 + 建父目录 +
                        // 写字节 + 记住这个目录（下次保存框直接从这里开）。
                        // 不再「先截临时文件再拷一份」，少一次失败点。
                        Some(dest) => match crate::exporter::save_bytes(dest, "png", &png) {
                            Ok(p) => {
                                let kb = png.len() / 1024;
                                let name = p
                                    .file_name()
                                    .map(|f| f.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "本叶.png".to_string());
                                // 用户点名要的提示语：一句「本叶已印制」。
                                // 副行只带文件名（不含目录），保持极简；
                                // 完整路径放 `notice`，Toast 上不堆信息。
                                (
                                    format!("已保存到 {}（{} KB）", p.display(), kb),
                                    format!("本叶已印制\n{name}"),
                                )
                            }
                            Err(e) => {
                                let m = format!("保存失败：{e}");
                                (m.clone(), m)
                            }
                        },
                    };
                    this.update(&mut cx, |this, cx| {
                        this.export_busy = false;
                        this.notice = msg;
                        this.toast = Some(toast);
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    /// **『导出全书批注』**（v1.63.0）：把全部 `MarginNote`（天头朱批）按章
    /// 节顺序排成一份 Markdown——章节名 + 原文摘录 + 批语，弹保存框落盘。
    ///
    /// 摘录不是整段抄：版式层只记「锚在第几个字」，导出层回正文取那一句
    /// （`exporter::to_annotations_md`），读稿时才知道当时批的是哪句。
    pub fn export_margin_notes(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            self.notice = "尚未打开书籍，无法导出批注".to_string();
            cx.notify();
            return;
        };
        let n = book
            .margin_notes
            .iter()
            .filter(|m| !m.content.trim().is_empty())
            .count();
        let file_name = format!(
            "{}_朱批录.md",
            crate::exporter::safe_file_stem(&book.title)
        );
        // v1.73.0：连点两次不该弹两个保存框（async 体在主线程 poll，
        // 面板浮起前第二次点击照样能进来）。
        if !self.export_begin(cx) {
            return;
        }
        cx.spawn(
            move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    // v1.52.4：rfd 同步保存框必须前台直调（走 background
                    // executor 会被主循环挤掉、静默返回 None）。
                    let res = crate::exporter::export_annotations_picked(&book, &file_name);
                    this.update(&mut cx, |this, cx| {
                        this.export_busy = false;
                        this.notice = match res {
                            Ok(Some(p)) if n == 0 => {
                                format!("此书尚无朱批，已导出空稿：{}", p.display())
                            }
                            Ok(Some(p)) => format!("已导出 {n} 条批注：{}", p.display()),
                            Ok(None) => "已取消导出".to_string(),
                            Err(e) => format!("导出失败：{e}"),
                        };
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    /// **『导出选中文字为 Anki 卡片』**（v1.63.0，极简 CSV）。
    ///
    /// 与「导出当前章为卡片」（`export_anki_chapter`，整章逐句切句）是两条
    /// 路：这里**只出一张卡**——正面就是用户拖选的原文，背面是 书名·章名
    /// 加这段范围内的朱批（`NoteKind::Note`）与眉批（`MarginNote`）。
    /// 不弹面板、不做预览，算好一行 CSV 直接走系统保存框。
    ///
    /// 转义一律交给 `exporter::csv_line`（RFC 4180）：古籍里逗号、引号成片，
    /// 不转义 Anki 导入会当场错列。
    pub fn export_anki_selection(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            self.notice = "尚未打开书籍，无法导出卡片".to_string();
            cx.notify();
            return;
        };
        // 正面：选中即原文（无拖选时 `selected_text` 退回单个 `sel_char`）。
        let front = self.selected_text().trim().to_string();
        if front.is_empty() {
            self.notice = "请先选中要制卡的文字".to_string();
            cx.notify();
            return;
        }
        // v1.73.0：连点两次不该弹两个保存框（async 体在主线程 poll，保存
        // 面板浮起前那几十毫秒里，第二次点击照样能进事件处理器）。
        if !self.export_begin(cx) {
            return;
        }

        let ci = self.current_chapter.min(book.chapters.len().saturating_sub(1));
        let chap = self.chapter_title(ci);
        let source = if chap.is_empty() {
            book.title.clone()
        } else {
            format!("{}·{}", book.title, chap)
        };

        // 背面：出处 + 选区范围内的批注。朱批按「章/段/字」比（与选区同一
        // 把尺子），眉批另按全书扁平下标比（它的锚点就是扁平下标）。
        let mut back = format!("【出处】{source}");
        let mut marks: Vec<String> = Vec::new();
        if let Some((a, b)) = self.selection_range() {
            for c in a.0..=b.0 {
                let Some(ch) = book.chapters.get(c) else {
                    continue;
                };
                let lo = if c == a.0 { a.1 } else { 0 };
                let hi = if c == b.0 {
                    b.1
                } else {
                    ch.paragraphs.len().saturating_sub(1)
                };
                for pi in lo..=hi {
                    let Some(p) = ch.paragraphs.get(pi) else {
                        continue;
                    };
                    let chars: Vec<char> = p.text.chars().collect();
                    let len = chars.len();
                    if len == 0 {
                        continue;
                    }
                    let from = if c == a.0 && pi == a.1 { a.2 } else { 0 };
                    let to = if c == b.0 && pi == b.1 {
                        b.2.min(len - 1)
                    } else {
                        len - 1
                    };
                    if from > to {
                        continue;
                    }
                    // 朱批：用户写在这一段字上的批注
                    for nt in p.notes.iter() {
                        let is_gloss = matches!(
                            nt.kind,
                            crate::types::NoteKind::Note | crate::types::NoteKind::Tongjia
                        );
                        if is_gloss && nt.at >= from && nt.at <= to {
                            let t = nt.text.trim();
                            if !t.is_empty() {
                                marks.push(t.to_string());
                            }
                        }
                    }
                    // 朱批：被划红线的字（拼起来读）
                    let red: String = (from..=to)
                        .filter(|o| self.underlines.contains(&(c, pi, *o)))
                        .filter_map(|o| chars.get(o).copied())
                        .collect();
                    if !red.is_empty() {
                        marks.push(format!("划线：{red}"));
                    }
                }
            }
            let base = book.flat_index(a);
            let end = book.flat_index(b);
            for m in book.margin_notes.iter() {
                if m.anchor_char_index >= base
                    && m.anchor_char_index <= end
                    && !m.content.trim().is_empty()
                {
                    marks.push(m.content.trim().to_string());
                }
            }
        }
        if !marks.is_empty() {
            // `<br>` 而非裸换行：Anki 导入后直接按 HTML 分行，不必再设样式。
            back.push_str("<br>");
            back.push_str(&marks.join("；"));
        }

        // 两列恒加双引号：古籍句子里逗号、引号成片，包起来才不会错列。
        let csv = crate::exporter::csv_two_quoted(&front, &back);
        let stem = crate::exporter::safe_file_stem(&book.title);
        let default = format!("{stem}_卡片.csv");
        cx.spawn(
            move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    // rfd 保存框必须前台直调（v1.52.4 铁律）：走 background
                    // executor 会被 GPUI 主循环挤掉、静默返回 None。
                    let picked = crate::exporter::pick_save_path(
                        "导出 Anki 卡片（CSV）",
                        &default,
                        "CSV 表格",
                        &["csv"],
                    );
                    // v1.73.0：取消 / 成功 / 失败三态在**同一处收口**：提示
                    // 先算成值，最后只做一次 update——闸门复位与提示落地绑在
                    // 一起，杜绝「某个分支忘了复位 → 导出永久锁死」。
                    // 写盘走 `save_bytes`：补后缀 + 建父目录 + 记住该目录。
                    let msg = match picked {
                        None => "已取消导出".to_string(),
                        Some(dest) => match crate::exporter::save_bytes(dest, "csv", csv.as_bytes()) {
                            Ok(_) => "已导出 1 张卡片，可在 Anki 中导入 CSV".to_string(),
                            Err(e) => format!("写入失败：{e}"),
                        },
                    };
                    this.update(&mut cx, |this, cx| {
                        this.export_busy = false;
                        this.notice = msg;
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    /// 『导出纯文本（MD）』按钮：全书正文 + `[^n]` 脚注，弹系统保存对话框落盘。
    /// **导出当前章为 Anki 卡片**（v1.56.0）。
    ///
    /// 逐句切出卡片：正面 = 句子正文；背面 = 【出处】书名·章名 + 【注释】
    /// 该句范围内的双行夹注 + 【朱批】该句范围内的眉批与被划红线的字。
    /// 落盘 `.csv`（Front / Back 两列，RFC 4180 转义），Anki 直接导入。
    pub fn export_anki_chapter(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            self.notice = "尚未打开书籍，无法导出卡片".to_string();
            cx.notify();
            return;
        };
        let ci = self.current_chapter.min(book.chapters.len().saturating_sub(1));
        let chap = self.chapter_title(ci);
        let source = if chap.is_empty() {
            book.title.clone()
        } else {
            format!("{}·{}", book.title, chap)
        };
        let paras = match book.chapters.get(ci) {
            Some(c) => c.paragraphs.clone(),
            None => Vec::new(),
        };
        let mut csv = String::new();
        let mut n = 0usize;
        for (pi, p) in paras.iter().enumerate() {
            let base = book.flat_index((ci, pi, 0));
            let cs: Vec<char> = p.text.chars().collect();
            let mut start = 0usize;
            for (i, c) in cs.iter().enumerate() {
                let brk = matches!(c, '。' | '！' | '？' | '；' | '!' | '?' | ';' | '\n');
                if !brk {
                    continue;
                }
                Self::anki_push_card(
                    &mut csv,
                    &mut n,
                    &book,
                    &source,
                    &cs,
                    start,
                    i + 1,
                    base,
                    ci,
                    pi,
                    &p.notes,
                    &self.underlines,
                );
                start = i + 1;
            }
            if start < cs.len() {
                Self::anki_push_card(
                    &mut csv,
                    &mut n,
                    &book,
                    &source,
                    &cs,
                    start,
                    cs.len(),
                    base,
                    ci,
                    pi,
                    &p.notes,
                    &self.underlines,
                );
            }
        }
        if n == 0 {
            self.notice = "本章没有可制卡片的句子".to_string();
            cx.notify();
            return;
        }
        // v1.73.0：连点两次不该弹两个保存框。
        if !self.export_begin(cx) {
            return;
        }
        let stem = crate::exporter::safe_file_stem(&book.title);
        let cstem = crate::exporter::safe_file_stem(&chap);
        let default = if cstem.is_empty() {
            format!("{stem}_卡片.csv")
        } else {
            format!("{stem}_{cstem}_卡片.csv")
        };
        let total = n;
        cx.spawn(
            move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    // rfd 保存框必须前台直调（v1.52.4 铁律）：走 background
                    // executor 会被 GPUI 主循环挤掉、静默返回 None。
                    let picked = crate::exporter::pick_save_path(
                        "导出 Anki 卡片（CSV）",
                        &default,
                        "CSV 表格",
                        &["csv"],
                    );
                    // v1.73.0：取消 / 成功 / 失败三态在**同一处收口**：提示
                    // 先算成值，最后只做一次 update——闸门复位与提示落地绑在
                    // 一起，杜绝「某个分支忘了复位 → 导出永久锁死」。
                    // 写盘走 `save_bytes`：补后缀 + 建父目录 + 记住该目录。
                    let msg = match picked {
                        None => "已取消导出".to_string(),
                        Some(dest) => match crate::exporter::save_bytes(dest, "csv", csv.as_bytes()) {
                            Ok(_) => format!("已导出 {total} 张卡片，请在 Anki 中导入 CSV"),
                            Err(e) => format!("写入失败：{e}"),
                        },
                    };
                    this.update(&mut cx, |this, cx| {
                        this.export_busy = false;
                        this.notice = msg;
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

/// 往 CSV 里追加一张卡片（`[from, to)` 是段内字符区间）。
///
/// 短于两字的碎片（孤立的标点、空行）不成卡。注释 / 朱批都按**字符区间**
/// 取：夹注看 `Note::at`，眉批看 `MarginNote::anchor_char_index`（全书扁平
/// 下标，`base` 是本段首字的扁平下标），划线看 `(章, 段, 偏移)` 键。
#[allow(clippy::too_many_arguments)]
fn anki_push_card(
    csv: &mut String,
    n: &mut usize,
    book: &crate::types::Book,
    source: &str,
    cs: &[char],
    from: usize,
    to: usize,
    base: usize,
    ci: usize,
    pi: usize,
    notes: &[crate::types::Note],
    underlines: &std::collections::HashSet<(usize, usize, usize)>,
) {
    if to <= from || to > cs.len() {
        return;
    }
    let front: String = cs[from..to].iter().collect();
    let front = front.trim().to_string();
    if front.chars().count() < 2 {
        return;
    }
    let mut back = format!("【出处】{source}");

    // 注释：夹注（Note）与通假（Tongjia）都算注文。
    let mut gloss: Vec<String> = Vec::new();
    for nt in notes.iter() {
        let is_gloss = matches!(
            nt.kind,
            crate::types::NoteKind::Note | crate::types::NoteKind::Tongjia
        );
        if !is_gloss || nt.at < from || nt.at >= to {
            continue;
        }
        if !nt.text.trim().is_empty() {
            gloss.push(nt.text.trim().to_string());
        }
    }
    if !gloss.is_empty() {
        back.push_str("<br>【注释】");
        back.push_str(&gloss.join("；"));
    }

    // 朱批：眉批（锚在字上）+ 被划红线的字。
    let mut marks: Vec<String> = Vec::new();
    for m in book.margin_notes.iter() {
        if m.anchor_char_index >= base + from && m.anchor_char_index < base + to {
            let t = m.content.trim();
            if !t.is_empty() {
                marks.push(t.to_string());
            }
        }
    }
    let red: String = (from..to)
        .filter(|o| underlines.contains(&(ci, pi, *o)))
        .map(|o| cs[o])
        .collect();
    if !red.is_empty() {
        marks.push(format!("划线：{red}"));
    }
    if !marks.is_empty() {
        back.push_str("<br>【朱批】");
        back.push_str(&marks.join("；"));
    }

    csv.push_str(&crate::exporter::csv_line(&[&front, &back]));
    *n += 1;
}

    pub fn export_markdown_file(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            self.notice = "尚未打开书籍，无法导出".to_string();
            cx.notify();
            return;
        };
        let file_name = format!("{}.md", crate::exporter::safe_file_stem(&book.title));
        // v1.73.0：连点两次不该弹两个保存框。
        if !self.export_begin(cx) {
            return;
        }
        cx.spawn(
            |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    // v1.52.4：**前台直调**（与 PNG/PDF 同一修复）——rfd
                    // 同步 save_file 必须跑在主线程；过 background_executor
                    // 会被 GPUI 主循环挤掉、静默返回 None，表现为「点了
                    // 导出什么都没发生」。to_markdown 是毫秒级 CPU 活儿，
                    // 放前台同步做不卡 UI。
                    let res = crate::exporter::export_markdown_picked(
                        &book,
                        "导出纯文本（MD）",
                        &file_name,
                    );
                    this.update(&mut cx, |this, cx| {
                        this.export_busy = false;
                        this.notice = match res {
                            Ok(Some(p)) => format!("已导出 Markdown：{}", p.display()),
                            // 取消不是失败：只说一声，不甩错误。
                            Ok(None) => "已取消导出".to_string(),
                            Err(e) => format!("导出失败：{e}"),
                        };
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }


    /// 某章的第一面页码；找不到就回 0。
    pub fn page_of_chapter(&self, chapter: usize) -> usize {
        let pages = self.paginate();
        pages
            .iter()
            .position(|p| p.first().map(|g| g.chapter >= chapter).unwrap_or(false))
            .unwrap_or(0)
    }

    /// 设字级档位（版字面板的「蝇头 / 中楷 / 大字 / 榜书」）。
    ///
    /// `zoom` 存的是倍率，档位由 `ZoomLevel::from_zoom` 反推，
    /// 所以这里只需把倍率落到该档的 `factor()` 上，排版下一帧自动重算。
    pub fn set_zoom_level(&mut self, lvl: crate::types::ZoomLevel) {
        self.zoom = lvl.factor();
        self.notice = format!("字级：{}（{}）", lvl.ancient_name(), lvl.label());
        self.persist();
    }

    /// 跳到指定章的首面（目录跳转用）。
    ///
    /// 走 `goto_page` 而不是直接赋值：双面对开时页码必须落在偶数上，
    /// 否则章首那一面会被排到**左半叶**，与上一叶错开半叶、读序颠倒。
    pub fn goto_chapter(&mut self, chapter: usize) {
        let page = self.page_of_chapter(chapter);
        self.goto_page(page);
        self.notice = self
            .book
            .as_ref()
            .and_then(|b| b.chapters.get(chapter))
            .map(|c| format!("已跳到「{}」", c.title))
            .unwrap_or_default();
        self.persist();
    }

    /// 跳到某一叶（目录划线条目用）。
    ///
    /// 双面对开时先页在右、后页在左，所以页码必须落在「偶数」上，
    /// 否则目标叶会跑到左半边、与上一叶错开一页。
    pub fn goto_page(&mut self, page: usize) {
        let pages = self.paginate();
        let last = pages.len().saturating_sub(1);
        let mut p = page.min(last);
        if self.duplex == Duplex::Facing && p % 2 == 1 {
            p = p.saturating_sub(1);
        }
        self.page = p;
        // 悄悄对齐当前章：目录朱砂红标记据此落在准确的卷目上。
        if let Some(g) = pages.get(p).and_then(|pg| pg.first()) {
            self.current_chapter = g.chapter;
        }
        self.notice = format!("已跳到第 {} 葉", p + 1);
        self.persist();
    }

    /// **一键回到卷首**（v1.38.0）：状态栏页码处双击、目录面板顶部
    /// 「⟲ 回到卷首」都走它。
    ///
    /// 与翻页共用 `goto_page`，因此双面对开时同样自动落回偶数叶（否则
    /// 卷首会跑到左半叶、与下一叶错开半叶、读序颠倒）。**不改任何排版
    /// 参数**——列数 / 留白 / 字级 / 版式一概不动，只是把游标拨回第一叶，
    /// 重绘由下一帧的 `paginate` 自然完成。
    pub fn jump_to_start(&mut self) {
        let already = self.page == 0;
        self.goto_page(0);
        // `goto_page` 会写「已跳到第 1 葉」；回到卷首给一句更贴题的提示。
        self.notice = if already {
            "已在卷首".to_string()
        } else {
            "已回到卷首".to_string()
        };
        self.persist();
    }

    /// 把 `current_chapter` 对齐到「当前页首字」所属章（翻页 / 跳章 / 恢复进度时调用）。
    ///
    /// 用户翻页时后台悄悄算好，打开目录时朱砂红点就落在准确的章节上，
    /// 不必等目录渲染时再临时推算。
    pub fn sync_current_chapter(&mut self) {
        let pages = self.paginate();
        if let Some(g) = pages.get(self.page).and_then(|pg| pg.first()) {
            self.current_chapter = g.chapter;
        }
    }

    /// 按全局字符下标跳转（目录卷目导航用）：
    ///
    /// 把 `idx` 当作版面流里的字符位置，找到它所在的首面，再走 `goto_page`
    /// 跳过去——相当于「把引擎的当前位置设到该章开头，清空当前画布，
    /// 重新从新位置开始计算网格排版，渲染第一叶」。双面对开同样自动落偶数页。
    pub fn goto_char_index(&mut self, idx: usize) {
        let pages = self.paginate();
        let mut acc = 0usize;
        let mut target = 0usize;
        for (pi, pg) in pages.iter().enumerate() {
            let n = pg.len();
            if acc <= idx && idx < acc + n {
                target = pi;
                break;
            }
            acc += n;
            target = pi;
        }
        self.goto_page(target);
        self.notice = self
            .book
            .as_ref()
            .and_then(|b| {
                b.toc
                    .iter()
                    .find(|e| e.start_char_index == Some(idx))
                    .map(|e| format!("已跳到「{}」", e.title))
            })
            .unwrap_or_else(|| format!("已跳到第 {} 葉", target + 1));
        self.persist();
    }

    /// 呼出目录侧栏（从右侧滑入）。
    ///
    /// 只设状态、不播动画本身——真正的逐帧推进在 `RootView::render` 里借
    /// `window.request_animation_frame` 完成，所以这里不需要 `window`，
    /// 键盘快捷键、鼠标回调都能直接调用。已展开 / 正在展开则幂等返回。
    /// 强制收起所有「底层瞬态浮层」：选词工具条、字义浮签、长按气泡、天头
    /// 输入框、文本选区、夹注菜单、注文放大卡。打开任一顶层面板前调用，杜绝
    /// 叠层乱弹（v1.45.0 交互重构 · 任务一）。
    pub fn close_transient_ui(&mut self) {
        self.popup = None;
        self.bubble = None;
        self.lookup_at = None;
        self.note_editing = false;
        self.head_anchor = None;
        // v1.48.0：眉批管理态随浮层一起收（改写态更甚——草稿不跨面板存活）。
        self.head_edit = None;
        self.head_active = None;
        // v1.64.0：清选区**之前**先定格一份「待分享选区」。分享抽屉是唯一
        // 需要跨面板拿到选区的地方——版字面板一开就清选区，否则用户
        // 「选中 → 版字 → 分享」永远只能分享本叶。
        if self.share_sel.is_none() && self.has_selection() {
            self.share_sel = self.selection_range();
        }
        self.sel_anchor = None;
        self.sel_focus = None;
        self.note_zoom = None;
        self.note_menu = None;
        self.dragging = false;
        self.hover_card = false;
        // v1.52.0：顶栏「导出」下拉也是瞬态的，随其余浮层一起收。
        self.export_menu = false;
    }

    /// 全局 UI 状态机：同一时刻只开一个顶层面板，并强制关闭所有底层瞬态浮层。
    /// 再点同一个已开面板 = 收起（`None`）。所有顶栏按钮 / 快捷键的开合都应
    /// 走这里，保证互斥一致（v1.45.0 交互重构 · 任务一）。
    pub fn set_ui_state(&mut self, s: AppUIState) {
        // 以「当前该面板是否真的开着」决定切换：避免其它路径（点面板内关闭
        // 钮、快捷键等）关掉面板后 `ui_state` 失同步，导致再点同一钮开不了。
        let on = match s {
            AppUIState::Style => self.show_style,
            AppUIState::Search => self.show_search,
            AppUIState::Notes => self.show_notes_panel,
            AppUIState::Help => self.show_help,
            AppUIState::Toc => self.show_toc,
            AppUIState::Share => self.show_share_panel,
            AppUIState::Bookmarks => self.show_bookmarks,
            AppUIState::Cards => self.show_cards,
            AppUIState::None => false,
        };
        let s = if on { AppUIState::None } else { s };
        self.ui_state = s;
        self.show_style = matches!(s, AppUIState::Style);
        self.show_search = matches!(s, AppUIState::Search);
        self.show_notes_panel = matches!(s, AppUIState::Notes);
        self.show_help = matches!(s, AppUIState::Help);
        self.show_toc = matches!(s, AppUIState::Toc);
        self.show_share_panel = matches!(s, AppUIState::Share);
        self.show_bookmarks = matches!(s, AppUIState::Bookmarks);
        self.show_cards = matches!(s, AppUIState::Cards);
        if !self.show_search {
            self.clear_search();
        }
        // 侧栏滑入 / 滑出：开 → 目标态拉满并播动画；关 → 目标态归零、
        // 由 `render` 逐帧把 slide 推回 0 后再卸载面板（与 `toc_slide` 同款）。
        self.notes_target = self.show_notes_panel;
        self.notes_animating = self.show_notes_panel || self.notes_slide > 0.0;
        self.search_target = self.show_search;
        self.search_animating = self.show_search || self.search_slide > 0.0;
        self.bookmarks_target = self.show_bookmarks;
        self.bookmarks_animating = self.show_bookmarks || self.bookmarks_slide > 0.0;
        self.cards_target = self.show_cards;
        self.cards_animating = self.show_cards || self.cards_slide > 0.0;
        // 开栏那一刻才去库里取一次（侧栏是低频面板，没必要每帧查）。
        if self.show_cards {
            self.reload_cards();
        }
        self.close_transient_ui();
    }

    /// 是否有任一顶层面板 / 浮层开着——用于选词工具条等瞬态浮签的**互斥隐藏**
    /// （打开『版字』/『目录』等顶部面板时，强制收起选词工具条，防事件穿透误触）。
    pub fn any_overlay_open(&self) -> bool {
        self.show_style
            || self.show_toc
            || self.show_search
            || self.show_notes_panel
            || self.show_help
            || self.show_share_panel
            || self.show_bookmarks
            || self.show_cards
    }

    pub fn open_toc(&mut self) {
        if self.show_toc && self.toc_target {
            return;
        }
        // 互斥：开目录前收起其它顶层面板与底层瞬态浮层。
        self.show_style = false;
        self.show_search = false;
        self.show_notes_panel = false;
        self.show_help = false;
        self.show_share_panel = false;
        self.ui_state = AppUIState::Toc;
        self.close_transient_ui();
        self.show_toc = true;
        self.toc_target = true;
        self.toc_animating = true;
        // v1.49.3：每次开目录都重新定位——滚到当前在读的那一卷，
        // 而不是永远从第一卷起手。
        self.toc_need_scroll = true;
        if self.toc_slide <= 0.0 {
            self.toc_slide = 0.0;
        }
    }

    /// 收起目录侧栏（向右侧滑出）。
    ///
    /// 若侧栏已经完全收起则原地返回；否则把目标态翻成「收起」，
    /// 由 `render` 的逐帧循环把 `toc_slide` 推回 0 后再真正卸载。
    pub fn close_toc(&mut self) {
        if !self.show_toc && !self.toc_animating {
            return;
        }
        if !self.show_toc {
            return;
        }
        self.toc_target = false;
        self.toc_animating = true;
        self.ui_state = AppUIState::None;
        // 极端情况：侧栏可见但 slide 已为 0（不应发生），直接收。
        if self.toc_slide <= 0.0 {
            self.show_toc = false;
            self.toc_animating = false;
        }
    }

    /// 呼出书签抽屉（从底部滑入）；与目录栏互斥（同一时刻只开一个顶层）。
    pub fn open_bookmarks(&mut self) {
        if self.show_bookmarks && self.bookmarks_target {
            return;
        }
        self.show_style = false;
        self.show_search = false;
        self.show_notes_panel = false;
        self.show_help = false;
        self.show_toc = false;
        self.show_share_panel = false;
        self.ui_state = AppUIState::Bookmarks;
        self.close_transient_ui();
        self.show_bookmarks = true;
        self.bookmarks_target = true;
        self.bookmarks_animating = true;
    }

    /// 收起书签抽屉（向底部滑出）。
    pub fn close_bookmarks(&mut self) {
        if !self.show_bookmarks && !self.bookmarks_animating {
            return;
        }
        if !self.show_bookmarks {
            return;
        }
        self.bookmarks_target = false;
        self.bookmarks_animating = true;
        self.ui_state = AppUIState::None;
        if self.bookmarks_slide <= 0.0 {
            self.show_bookmarks = false;
            self.bookmarks_animating = false;
        }
    }

    /// 顶栏「书签」钮：抽屉开着就收起；没开则先收藏当前页（去重）再展开。
    pub fn toggle_bookmarks(&mut self) {
        if self.show_bookmarks {
            self.close_bookmarks();
        } else {
            self.add_bookmark_current();
            self.open_bookmarks();
        }
    }

    /// 收藏当前页：以 `page_anchor` 三元组定位，记下章节名与一句摘录。
    /// 同一字已收藏则跳过（去重），避免反复点同一个钮刷出一串重复书签。
    pub fn add_bookmark_current(&mut self) {
        let Some((chapter, para, offset)) = self.page_anchor() else {
            return;
        };
        if self
            .bookmarks
            .iter()
            .any(|b| (b.chapter, b.para, b.offset) == (chapter, para, offset))
        {
            return;
        }
        let chapter_title = self.chapter_title(chapter);
        let excerpt = self.excerpt_at(chapter, para, offset);
        let bm = Bookmark {
            id: Self::new_bookmark_id(),
            chapter,
            para,
            offset,
            chapter_title,
            excerpt,
            created: crate::library::now_secs(),
        };
        self.bookmarks.push(bm);
        self.save_bookmarks();
    }

    /// 删除一枚书签（按 id）。
    pub fn remove_bookmark(&mut self, id: &str) {
        self.bookmarks.retain(|b| b.id != id);
        self.save_bookmarks();
    }

    /// 跳到某书签：按三元组找回它所在的那一面，再走 `goto_page`（自动对齐当前章）。
    pub fn jump_to_bookmark(&mut self, bm: &Bookmark) {
        if let Some(pi) = self.layout_pages().iter().position(|pg| {
            pg.glyphs.iter().any(|g| {
                (g.chapter, g.para, g.offset) == (bm.chapter, bm.para, bm.offset)
            })
        }) {
            self.goto_page(pi);
        }
    }

    /// 把当前书签落档到书架。
    pub fn save_bookmarks(&mut self) {
        let bms = self.bookmarks.clone();
        self.library.save_bookmarks(self.current, &bms);
    }

    /// 当前段落里围绕锚点的一句摘录（去空白、截断），落档用。
    fn excerpt_at(&self, chapter: usize, para: usize, offset: usize) -> String {
        let Some(text) = self
            .book
            .as_ref()
            .and_then(|b| b.chapters.get(chapter))
            .and_then(|c| c.paragraphs.get(para))
            .map(|p| p.text.clone())
        else {
            return String::new();
        };
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let start = offset.saturating_sub(8);
        let end = (offset + 16).min(chars.len());
        let slice: String = chars[start..end].iter().collect();
        slice
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 书签 id：纳秒时间戳，几乎不会撞。
    fn new_bookmark_id() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("bm-{nanos}")
    }

    /// 分页（缓存由每次渲染即时计算，文本量级足够）。
    pub fn page_cap(&self) -> usize {
        // 每面容量（粗略上限）：列数 × 行数。
        self.grid.columns * self.grid_rows
    }

    /// 取某个字上挂的注文（夹注只认 `Note` 一类，通假另走旁注）。
    /// 某字上的夹注文本 **与来源**（`NoteSource::User` = 用户自加，深朱砂红）。
    /// 供 `layout_book` 的 `gloss` 闭包取用，来源随注文一路带到渲染层做视觉区分。
    pub fn note_chars_with_source(&self, g: Glyph) -> Option<(&str, NoteSource)> {
        if !self.show_notes || g.kind != GlyphKind::Char {
            return None;
        }
        let book = self.book.as_ref()?;
        let p = book.chapters.get(g.chapter)?.paragraphs.get(g.para)?;
        p.notes
            .iter()
            .find(|n| n.at == g.offset && n.kind == NoteKind::Note)
            .map(|n| (n.text.as_str(), n.source))
    }

    /// 只要夹注文本（不关心来源）。
    pub fn note_chars(&self, g: Glyph) -> Option<&str> {
        self.note_chars_with_source(g).map(|(s, _)| s)
    }

    /// 某字是否落在划线标记（人名 / 地名 / 书名）的跨度内，
    /// 返回线的种类（实线 / 波浪线）。
    pub fn mark_kind(&self, g: Glyph) -> Option<MarkKind> {
        if g.kind != GlyphKind::Char {
            return None;
        }
        let book = self.book.as_ref()?;
        let p = book.chapters.get(g.chapter)?.paragraphs.get(g.para)?;
        p.mark_at(g.offset)
    }

    /// 某字是否落在**着重**（导入管线由 `**加粗**` / `<strong>` 转来的
    /// `[高亮]…[/高亮]`）的跨度内。渲染侧据此在字旁点一枚朱砂圈点。
    pub fn emph_at(&self, g: Glyph) -> bool {
        if g.kind != GlyphKind::Char {
            return false;
        }
        let Some(book) = self.book.as_ref() else {
            return false;
        };
        let Some(p) = book
            .chapters
            .get(g.chapter)
            .and_then(|c| c.paragraphs.get(g.para))
        else {
            return false;
        };
        p.emph_at(g.offset)
    }

    /// 把当前拖选区间标记为人名 / 地名 / 书名：写入书稿段落并回写 JSON，
    /// 标记即时生效（`mark_kind` 实时读 `self.book`），下次打开仍在。
    /// 与数据里预置的标记走同一通道——渲染侧 `ancient_cell` 只认 `mark_kind`。
    pub fn mark_selection(&mut self, kind: NoteKind) {
        if !matches!(kind, NoteKind::Person | NoteKind::Place | NoteKind::Book) {
            return;
        }
        let Some((a, b)) = self.selection_range() else {
            self.notice = "请先拖选要标记的文字".to_string();
            return;
        };
        let pages = self.paginate();
        // 收集区间内与本段同段的字键，并拼出选区文本（兼顾双面对开左右两页）。
        let mut keys: Vec<(usize, usize, usize)> = Vec::new();
        let mut text = String::new();
        for pg in [self.page, self.page + 1] {
            if let Some(glyphs) = pages.get(pg) {
                for g in glyphs {
                    if g.kind != GlyphKind::Char || g.is_indent() {
                        continue;
                    }
                    let k = (g.chapter, g.para, g.offset);
                    if k >= a && k <= b && k.1 == a.1 {
                        keys.push(k);
                        text.push(g.ch);
                    }
                }
            }
        }
        if keys.is_empty() {
            self.notice = "选区内没有可标记的字".to_string();
            return;
        }
        let (c, pa, start) = (a.0, a.1, a.2);
        let end = keys.iter().map(|k| k.2).max().unwrap_or(start);
        let len = end - start + 1;
        // 编辑书稿：在对应段落追加一条跨度标记，并清掉区间内已有的专名标记
        // （避免新旧叠加错位）。
        let edited = if let Some(book) = self.book.as_mut() {
            if let Some(p) = book
                .chapters
                .get_mut(c)
                .and_then(|ch| ch.paragraphs.get_mut(pa))
            {
                p.notes.retain(|n| {
                    !(n.kind.is_mark()
                        && n.at <= end
                        && n.at + n.len.saturating_sub(1) >= start)
                });
                p.notes.push(Note::span(start, len, kind, text));
                true
            } else {
                false
            }
        } else {
            false
        };
        if edited {
            if let Some(path) = self.library.books.get(self.current).map(|e| e.path.clone()) {
                let _ = crate::importer::write_book_json(self.book.as_ref().unwrap(), &path);
            }
        }
        self.clear_selection();
        let label = match kind {
            NoteKind::Book => "书名",
            _ => "专名",
        };
        self.notice = format!("已标记为{label}");
    }

    /// 分页（只保留古籍竖笺）。
    ///
    /// 竖笺按「半格」计量：正文一字占 2 格、夹注每行小字占 1 格，
    /// 当前选区的文字（按阅读顺序拼出）。
    ///
    /// 气泡菜单的「复制 / 检索」都以此为准：有选区取选区，
    /// 没有选区（长按单字）就退回那一个字。
    pub fn selected_text(&self) -> String {
        let Some((a, b)) = self.selection_range() else {
            return self.sel_char.map(|c| c.to_string()).unwrap_or_default();
        };
        let pages = self.paginate();
        let mut text = String::new();
        for pg in [self.page, self.page + 1] {
            let Some(glyphs) = pages.get(pg) else {
                continue;
            };
            for g in glyphs {
                if g.kind != GlyphKind::Char || g.is_indent() {
                    continue;
                }
                let k = (g.chapter, g.para, g.offset);
                if k >= a && k <= b {
                    text.push(g.ch);
                }
            }
        }
        if text.is_empty() {
            self.sel_char.map(|c| c.to_string()).unwrap_or_default()
        } else {
            text
        }
    }

    // ---- 全文检索 ----

    /// 在书稿正文里找出 `q` 的全部出现位置，按「章 → 段 → 字」阅读顺序排列。
    ///
    /// 只搜正文（`Paragraph::text`），不搜注文——注文是后加的注解，
    /// 混进结果会让「跳到那一页」失去意义。偏移用**字符下标**，
    /// 与 `Glyph::offset`、`Note::at` 同一口径。
    pub fn search_all(&self, q: &str) -> Vec<(usize, usize, usize)> {
        self.book.as_ref().map(|b| b.search(q)).unwrap_or_default()
    }

    /// 跑一次检索：填命中表、整段匹配跨度入高亮集、跳到第一处。
    pub fn run_search(&mut self, cx: &mut Context<Self>) {
        let q = self.search_draft.trim().to_string();
        if q.is_empty() {
            self.clear_search();
            self.notice = "请输入要检索的字词".to_string();
            cx.notify();
            return;
        }
        let hits = self.search_all(&q);
        let qlen = q.chars().count();
        // 命中跨度整段入集：不只亮首字，「长安」两字一起亮。
        self.search_keys = hits
            .iter()
            .flat_map(|&(c, p, o)| (0..qlen).map(move |k| (c, p, o + k)))
            .collect();
        let n = hits.len();
        self.search_hits = hits;
        self.search_idx = 0;
        if n == 0 {
            self.search_cur = None;
            self.notice = format!("全文未找到「{q}」");
            cx.notify();
        } else {
            self.notice = format!("「{q}」共 {n} 处");
            self.goto_hit(0, cx);
        }
    }

    /// 用给定的词直接检索（气泡菜单「检索」用：选中即搜，不必手打）。
    pub fn search_for(&mut self, q: &str, cx: &mut Context<Self>) {
        self.search_draft = q.chars().take(crate::ime::SEARCH_MAX).collect();
        self.search_cursor = self.search_draft.chars().count();
        self.show_search = true;
        self.run_search(cx);
    }

    /// 跳到第 `i` 处命中所在的面，并把它设为「当前命中」。
    pub fn goto_hit(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(&(c, p, o)) = self.search_hits.get(i) else {
            return;
        };
        self.search_idx = i;
        let qlen = self.search_draft.trim().chars().count().max(1);
        self.search_cur = Some((c, p, o, qlen));
        if let Some(pg) = self.page_of_key((c, p, o)) {
            self.page = pg;
        }
        self.clear_selection();
        self.popup = None;
        cx.notify();
    }

    pub fn search_next(&mut self, cx: &mut Context<Self>) {
        let n = self.search_hits.len();
        if n == 0 {
            return;
        }
        let i = (self.search_idx + 1) % n;
        self.goto_hit(i, cx);
    }

    pub fn search_prev(&mut self, cx: &mut Context<Self>) {
        let n = self.search_hits.len();
        if n == 0 {
            return;
        }
        let i = (self.search_idx + n - 1) % n;
        self.goto_hit(i, cx);
    }

    /// 清掉检索结果（保留草稿，方便改词再搜）。
    pub fn clear_search(&mut self) {
        self.search_hits.clear();
        self.search_keys.clear();
        self.search_idx = 0;
        self.search_cur = None;
    }

    /// 检索框：退格删掉光标前一个字。
    pub fn search_backspace(&mut self) {
        let mut chars: Vec<char> = self.search_draft.chars().collect();
        let at = self.search_cursor.min(chars.len());
        if at == 0 {
            return;
        }
        chars.remove(at - 1);
        self.search_draft = chars.into_iter().collect();
        self.search_cursor = at - 1;
    }

    /// 检索框：光标处插入一个字（英文 / 数字走 keydown；中文走 IME）。
    pub fn search_input_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        if self.search_draft.chars().count() >= crate::ime::SEARCH_MAX {
            return;
        }
        let mut chars: Vec<char> = self.search_draft.chars().collect();
        let at = self.search_cursor.min(chars.len());
        chars.insert(at, c);
        self.search_draft = chars.into_iter().collect();
        self.search_cursor = at + 1;
    }

    /// 命中所在面：分页里第一个含该字的面。
    pub fn page_of_key(&self, key: (usize, usize, usize)) -> Option<usize> {
        self.paginate()
            .iter()
            .position(|pg| pg.iter().any(|g| (g.chapter, g.para, g.offset) == key))
    }

    /// 渲染层查询：该字是否命中检索。
    ///   `Some(true)`  = 落在**当前**这一处命中的跨度里（朱砂更重）
    ///   `Some(false)` = 命中但非当前（朱砂较淡）
    ///   `None`        = 未命中
    pub fn search_state_at(&self, chapter: usize, para: usize, offset: usize) -> Option<bool> {
        if let Some((c, p, s, len)) = self.search_cur {
            if c == chapter && p == para && offset >= s && offset < s + len {
                return Some(true);
            }
        }
        if self.search_keys.contains(&(chapter, para, offset)) {
            Some(false)
        } else {
            None
        }
    }

    /// 渲染层查询：该字是否落在「抵达」高亮里（朱批录 / 搜索跳转后那一下）。
    /// 口径与 `search_state_at` 同，但优先级更高——它是「刚跳到这」的强提示。
    pub fn flash_state_at(&self, chapter: usize, para: usize, offset: usize) -> Option<bool> {
        if let Some((c, p, s, len)) = self.note_flash {
            if c == chapter && p == para && offset >= s && offset < s + len {
                return Some(true);
            }
        }
        None
    }

    /// 点亮一枚「抵达」高亮，约 1.4s 后自动熄灭（提示用户已跳到该处）。
    ///
    /// 用 `target` 快照防「快速连点两条」时后一条的定时器把前一条的
    /// 高亮误熄——只熄灭与自己同款的那个。
    pub fn flash_note(&mut self, key: (usize, usize, usize), len: usize, cx: &mut Context<Self>) {
        self.note_flash = Some((key.0, key.1, key.2, len.max(1)));
        let target = self.note_flash;
        cx.notify();
        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1400))
                    .await;
                let _ = this.update(&mut cx, |this, cx| {
                    if this.note_flash == target {
                        this.note_flash = None;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// 朱批录 / 搜索结果的一键跳转：瞬移该锚点所在面、收起侧栏、点亮抵达高亮。
    ///
    /// `key` 是「章 · 段 · 偏移」三件套（`HeadNote::key` / `Note::char_index`
    /// 都按此口径），与分页锚点同源，故换列数也能找回去。页笔记没有 charIndex，
    /// 走 `jump_to_page` 只翻面不闪高。
    pub fn jump_to_key(&mut self, key: (usize, usize, usize), len: usize, cx: &mut Context<Self>) {
        if let Some(pg) = self.page_of_key(key) {
            self.page = pg;
        }
        self.show_notes_panel = false;
        self.show_search = false;
        self.clear_selection();
        self.popup = None;
        self.flash_note(key, len, cx);
        cx.notify();
    }

    /// 只翻面（页笔记用：没有 charIndex，无法闪高具体字）。
    pub fn jump_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.page = page;
        self.show_notes_panel = false;
        self.popup = None;
        cx.notify();
    }

    /// 当前版式指纹：它变了的任何一项都会改变面数，故整份分页缓存作废。
    fn pager_key(&mut self) -> crate::pager::LayoutKey {
        // 卷首题署预留行数要遍历全书章题，缓存住（换书时清）。
        let need = match self.inscription_need {
            Some(v) => v,
            None => {
                let v = self.chapter_inscription_need();
                self.inscription_need = Some(v);
                v
            }
        };
        crate::pager::LayoutKey {
            cols: self.grid.columns,
            rows: self.grid_rows,
            reserve: crate::gloss::inscription_reserve(need, self.grid_rows),
            show_punct: self.show_punct,
            inline_notes: self.notes_inline && self.show_notes,
            west_mode: !self.han_digits,
        }
    }

    /// 让分页器与「当前这本书 + 当前这套版式」对齐。
    ///
    /// 换书（章数不同）→ 重建；换列数 / 留白 / 标点显隐 / 夹注开关 → 作废
    /// 重排（`rekey`）。两者都不变时是空操作，故每帧调它也零成本。
    fn sync_pager(&mut self) {
        let n = match &self.book {
            Some(b) => b.chapters.len(),
            None => {
                self.pager = None;
                return;
            }
        };
        let key = self.pager_key();
        let store = self.book.as_ref().and_then(|b| b.store_id.clone());
        let rebuild = match self.pager.as_ref() {
            Some(p) => p.chapters() != n,
            None => true,
        };
        if rebuild {
            self.pager = Some(crate::pager::Pager::new(n, key, store));
        } else if let Some(p) = self.pager.as_mut() {
            p.rekey(key);
            p.set_store(store);
        }
    }

    /// **取第 `i` 叶**（渲染 / 命中用）：命中缓存就是 O(log n) 的一次查表，
    /// 没排过才排它所在的那一章。
    ///
    /// 旧架构在这里排的是**全书**——现在只排一章（实测最慢一章 0.8ms）。
    pub fn page_at(&mut self, i: usize) -> Option<crate::gloss::Page> {
        self.sync_pager();
        let book = self.book.as_mut()?;
        let pg = self.pager.as_mut()?;
        pg.page(book, i)
    }

    /// 总叶数。第一次调用会把索引探明（hydrate 沿途各章 + 排各章取面数），
    /// 之后纯查表。开卷首帧一次性付出（308 万字约 0.36s），换来之后每帧
    /// 60 FPS。
    pub fn total_pages_lazy(&mut self) -> usize {
        self.sync_pager();
        let Some(book) = self.book.as_mut() else {
            return 0;
        };
        let Some(pg) = self.pager.as_mut() else {
            return 0;
        };
        if !pg.is_complete() {
            pg.scan_all(book);
        }
        pg.known_total()
    }

    /// 一列 `rows` 字 = `rows×2` 格、一面 `cols` 列；注文过长会跨列流转，
    /// 排版结果与阅读器里的列布局完全一致（两边同走 `gloss::layout_book`）。
    /// `cols` / `rows` 全部来自网格引擎——同一套分页逻辑适配任意列数。
    pub fn paginate(&self) -> Vec<Vec<Glyph>> {
        self.layout_pages()
            .into_iter()
            .map(|p| p.glyphs)            .collect()
    }

    /// 排出「面 → 列 → 片」的完整树（阅读器渲染用）。
    pub fn layout_pages(&self) -> Vec<crate::gloss::Page> {
        let Some(book) = &self.book else {
            return vec![crate::gloss::Page {
                glyphs: Vec::new(),
                start_chapter: false,
                columns: Vec::new(),
            }];
        };
        let flow = book.to_flow(self.show_punct);
        let cols = self.grid.columns;
        let rows = self.grid_rows;
        // 卷首题署：章首页开卷列顶部预留的空白带行数（0 = 不预留）。
        let reserve = crate::gloss::inscription_reserve(self.chapter_inscription_need(), rows);
        // 只保留古籍竖笺：一律按竖排分页。
        // `west_mode`：数字转汉字开启时，每个数字是独立汉字格，不能再按
        // 「两字一格」记账，否则分页与渲染的列高对不上（会溢出一格）。
        crate::gloss::layout_book_from_opts(
            &flow,
            |g| {
                if self.notes_inline {
                    // 带上来源与宿主字形：来源决定渲染配色（用户注深朱砂红），
                    // 宿主字形用于右键/长按定位书稿里那条注文（改 / 删）。
                    self.note_chars_with_source(*g)
                        .map(|(s, src)| (s.to_string(), src, *g))
                } else {
                    None
                }
            },
            cols,
            rows,
            reserve,
            true,              // start_as_chapter：整书从首面排起
            !self.han_digits,  // west_mode：西文成组压缩
        )
    }

    /// 排版耗时探针：`JIANDU_TIMING=1` 时每次分页打一行到 stderr。
    /// 全书逐帧重排是当前引擎的**规模上限所在**，量出来才知道能装多厚的书。
    pub fn layout_pages_timed(&self) -> Vec<crate::gloss::Page> {
        let t0 = std::time::Instant::now();
        let pages = self.layout_pages();
        if std::env::var("JIANDU_TIMING").is_ok() {
            eprintln!(
                "[layout] {} 章 / {} 页 / {} ms",
                self.book.as_ref().map(|b| b.chapters.len()).unwrap_or(0),
                pages.len(),
                t0.elapsed().as_millis()
            );
        }
        pages
    }

    /// 卷首题署所需字符数上限：对每章取「书名 + 卷次 + 1 行间隔 + 署名」的字符数，
    /// 取全书最大值。这个值只决定章首页开卷列顶部预留多少行，渲染层按预留带裁切。
    pub fn chapter_inscription_need(&self) -> usize {
        let Some(b) = &self.book else {
            return 0;
        };
        let author = b.author.chars().count().max(1);
        let mut need = 0usize;
        for (ci, ch) in b.chapters.iter().enumerate() {
            let vol = crate::types::chapter_volume(ci, &ch.title).chars().count();
            need = need.max(b.title.chars().count() + vol + 1 + author);
        }
        need
    }

    /// 卷首题署预留行数（与 `layout_book` 收到的 `reserve_rows` 同源，渲染层据此定位）。
    pub fn chapter_inscription_reserve_rows(&self) -> usize {
        crate::gloss::inscription_reserve(self.chapter_inscription_need(), self.grid_rows)
    }

    /// 每帧重算可见面上的注文包围盒（Canvas 命中测试 / 卡片定位用）。
    ///
    /// 只覆盖当前视野：单页取当前面；对开取右叶（当前面）+ 左叶（下一面），
    /// 与 `render_reading` 的舞台排布一致。坐标是**版框内纸面坐标**，
    /// 换算窗口坐标走 `gloss::leaf_origin`（见 `note_box_at`）。
    /// 脚注模式（`notes_inline=false`）注文在页脚，不在行内，没有包围盒。
    /// v1.60.0：只吃**当前叶 + 下一叶**（由分页器按需取来），不再要全量切片
    /// ——旧签名逼着调用方每帧排全书。
    pub fn refresh_note_boxes(
        &mut self,
        cur: Option<&crate::gloss::Page>,
        next: Option<&crate::gloss::Page>,
    ) {
        self.note_boxes.clear();
        if !self.notes_inline || !self.show_notes {
            return;
        }
        let mut leaves: Vec<(usize, bool, &crate::gloss::Page)> = Vec::new();
        if self.duplex == Duplex::Facing {
            if let Some(p) = next {
                leaves.push((self.page + 1, true, p));
            }
            if let Some(p) = cur {
                leaves.push((self.page, false, p));
            }
        } else if let Some(p) = cur {
            leaves.push((self.page, false, p));
        }
        for (idx, back, pg) in leaves {
            let tc = if pg.start_chapter {
                self.grid.columns.saturating_sub(1)
            } else {
                self.grid.columns
            };
            for (i, col) in pg.columns.iter().enumerate() {
                // 竖排：读序列号 i=0 在最右；flex_row 从左数的位置 li = tc-1-i。
                let li = tc.saturating_sub(1).saturating_sub(i);
                for mut b in crate::gloss::note_boxes_in_column(
                    col,
                    li,
                    self.col_w,
                    self.row_h,
                    !self.han_digits,
                ) {
                    b.page = idx;
                    b.back = back;
                    self.note_boxes.push(b);
                }
            }
        }
    }

    /// 点击坐标（窗口坐标）命中哪个注文块：命中返回该块（窗口坐标系矩形，
    /// 便于直接定位卡片）。`scroll` 是最小字号保护下滚动容器的纵向偏移
    /// （注块随纸面一起滚，命中矩形同步平移）。
    pub fn note_box_at(&self, x: f32, y: f32) -> Option<&crate::gloss::NoteBox> {
        let facing = self.duplex == Duplex::Facing;
        // 中缝宽与渲染层同源（grid::gutter_w）：对开时两叶之间隔的就是它。
        let gap = crate::grid::gutter_w(self.content_w, self.grid.columns, ANCIENT_EDGE, self.grid.margin);
        self.note_boxes.iter().find(|b| {
            let (ox, oy) = crate::gloss::leaf_origin(
                facing,
                self.viewport_w,
                self.viewport_h,
                self.page_w,
                self.page_h,
                gap,
                b.back,
                self.focus_mode,
                // 窄屏单叶：纸面贴右边缘（与渲染同源）。
                !facing && crate::grid::is_narrow(self.viewport_w, self.viewport_h),
            );
            x >= ox + b.x
                && x <= ox + b.x + b.w
                && y >= oy + b.y
                && y <= oy + b.y + b.h
        })
    }

    pub fn page_count(&self) -> usize {
        self.paginate().len()
    }
    pub fn max_page(&self) -> usize {
        self.page_count().saturating_sub(1)
    }

    /// 翻面（双面对开时一次两步）；翻页即换页，不再有翻页动画。
    pub fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let delta = if self.duplex == Duplex::Facing { 2 } else { 1 };
        let last = self.max_page();
        let old = self.page;
        let new = if forward {
            (self.page + delta).min(last)
        } else {
            self.page.saturating_sub(delta)
        };
        if new == old {
            return; // 已在首/末页，无需翻动。
        }
        self.page = new;
        self.clear_selection();
        self.popup = None;
        // 翻页也是「下一次交互」：导出收尾话随翻页清掉（v1.52.3）。
        self.toast = None;
        // 窄屏引导也跟着走：用户已经在读书了，不用再教（v1.52.3）。
        self.narrow_hint = false;
        self.persist();
        cx.notify();
    }

    /// 保存 / 删除某页笔记（空文字即删除）。
    pub fn set_note(&mut self, page: usize, text: String) {
        if text.trim().is_empty() {
            self.notes.remove(&page);
        } else {
            self.notes.insert(page, text);
        }
        self.save_annotations();
    }

    // ---- 浮层笔记编辑栏（直接盖在文本上，不推开正文）----

    /// 打开当前页的浮层笔记编辑栏（草稿预填已有笔记；没有则为空）。
    /// 编辑栏以浮层形式盖在正文上方，不弹系统对话框、不挤压正文排版；
    /// 点正文其它处即自动保存，Esc 取消，⏎ 保存。
    pub fn start_note_edit(&mut self) {
        self.start_note_edit_at(None);
    }

    /// `at` 为浮层锚点（窗口坐标）；None 时按视口居中偏上放置。
    pub fn start_note_edit_at(&mut self, at: Option<(f32, f32)>) {
        self.note_draft = self
            .notes
            .get(&self.page)
            .cloned()
            .unwrap_or_default()
            .replace(['\n', '\r'], " ");
        self.note_cursor = self.note_draft.chars().count();
        self.note_pos = at;
        self.note_editing = true;
        self.note_edit_note = None; // 页眉批注
        self.note_inject_key = None;
        self.note_suppress_commit = true; // 忽略「打开这一下」点击的自动提交
        self.popup = None;
    }

    /// 在宿主字 `key` 上**新建**一条用户自加夹注：打开浮层编辑栏，
    /// 预填该字已有的夹注（有则算改写），保存时走 `inject_user_note_at`。
    ///
    /// 这是「点字 → 加夹注」的入口，配合 `note_inject.rs` 的局部重排：
    /// 夹注挂在字形上、不进正文流，所以注入后**不用重排全书**。
    pub fn start_note_inject(
        &mut self,
        key: (usize, usize, usize),
        existing: &str,
        at: Option<(f32, f32)>,
    ) {
        self.note_draft = existing.replace(['\n', '\r'], " ");
        self.note_cursor = self.note_draft.chars().count();
        self.note_pos = at;
        self.note_editing = true;
        self.note_edit_note = None;
        self.note_inject_key = Some(key);
        self.note_suppress_commit = true;
        self.popup = None;
        self.note_zoom = None;
        self.note_menu = None;
    }

    /// 按文本反查注文位置：全书里第一条正文与 `text` 相同的注释。
    ///
    /// 放大卡只有注文文本（`NoteZoom.text`），不带定位键
    /// （`Piece::Gloss` 只传文本），据此把「改的是哪条注文」找回来。
    /// 跨列流转的注文在卡里只是整条的一段，可能查不到 —— 返回 None。
    pub fn note_key_for_text(&self, text: &str) -> Option<(usize, usize, usize)> {
        if text.is_empty() {
            return None;
        }
        let book = self.book.as_ref()?;
        for (ci, ch) in book.chapters.iter().enumerate() {
            for (pi, p) in ch.paragraphs.iter().enumerate() {
                if let Some(n) = p
                    .notes
                    .iter()
                    .find(|n| n.kind == NoteKind::Note && n.text == text)
                {
                    return Some((ci, pi, n.at));
                }
            }
        }
        None
    }

    /// 编辑某条注文（夹注 / 注释）：草稿预填原文本，保存时写回书稿。
    pub fn start_annotation_edit(
        &mut self,
        key: (usize, usize, usize),
        text: &str,
        at: Option<(f32, f32)>,
    ) {
        self.note_draft = text.replace(['\n', '\r'], " ");
        self.note_cursor = self.note_draft.chars().count();
        self.note_pos = at;
        self.note_editing = true;
        self.note_edit_note = Some(key);
        self.note_inject_key = None;
        self.note_suppress_commit = true;
        self.popup = None;
        self.note_zoom = None;
    }

    /// 在宿主字 `key` 上注入一条**用户自加夹注**，并回写书稿 JSON。
    ///
    /// 与古籍原本夹注的区别只在 `NoteSource::User`：渲染侧据此染深朱砂红、
    /// 垫极淡朱砂底衬，并允许右键 / 长按改删。
    /// 夹注是挂在字形上的、不进正文流，所以注入**不改变任何字的全局下标**，
    /// 后续只需从当前半叶起点局部重排（`relayout_from_halfleaf`），绝不重排全书。
    pub fn inject_user_note_at(&mut self, key: (usize, usize, usize), text: &str) {
        let (c, p, o) = key;
        let done = if let Some(book) = self.book.as_mut() {
            crate::note_inject::inject_inline_note_at_key(
                book,
                c,
                p,
                o,
                text,
                NoteSource::User,
            );
            true
        } else {
            false
        };
        if done {
            self.persist_book_json();
            // 只从宿主字所在半叶续排，把视图锚回原来在读的那一叶。
            self.reanchor_after_note_edit(key);
            self.notice = if text.trim().is_empty() {
                "夹注已清空".to_string()
            } else {
                format!("已加夹注「{}」", text.chars().take(10).collect::<String>())
            };
        }
    }

    /// 删除宿主字 `key` 上的「用户自加夹注」并回写 JSON。
    ///
    /// 古籍原本的夹注**删不掉**（`remove_user_note_at_key` 只匹配
    /// `NoteSource::User`）——那是底本的一部分，动了就失真。
    /// 删除后释放该注所占的半格，后续字上移回流（局部重排由调用方触发）。
    pub fn delete_user_note_at(&mut self, key: (usize, usize, usize)) -> bool {
        let (c, p, o) = key;
        let removed = match self.book.as_mut() {
            Some(book) => crate::note_inject::remove_user_note_at_key(book, c, p, o),
            None => false,
        };
        if removed {
            self.persist_book_json();
            // 释放的半格让后续字上移回流：同样只续排后缀、把视图锚回去。
            self.reanchor_after_note_edit(key);
            self.notice = "夹注已删除".to_string();
        } else {
            self.notice = "此处没有可删除的自加夹注".to_string();
        }
        removed
    }

    /// 宿主字键 `(chapter, para, offset)` → 全局流下标。
    fn flow_index_of_key(&self, key: (usize, usize, usize)) -> Option<usize> {
        let book = self.book.as_ref()?;
        let flow = book.to_flow(self.show_punct);
        flow.iter()
            .position(|g| g.chapter == key.0 && g.para == key.1 && g.offset == key.2)
    }

    /// 夹注增删后把视图锚回宿主字所在的半叶——**只从该半叶起点续排**。
    ///
    /// 夹注挂在字形上、不进正文流，所以正文的下标一个都没变；变的是
    /// 半格账（注文占半格，会把后续字往后面的叶推）。这里用
    /// `relayout_from_halfleaf`：前 N−1 面**原样复用、零重算**，只把第 N 面
    /// 之后续排一遍（已由单测证明逐面 == 全书重排，无漂移），据此把
    /// `self.page` 锚到宿主字现在所在的叶，避免增删注文后视图漂走。
    fn reanchor_after_note_edit(&mut self, key: (usize, usize, usize)) {
        let Some(idx) = self.flow_index_of_key(key) else {
            return;
        };
        let Some(book) = self.book.as_ref() else {
            return;
        };
        let flow = book.to_flow(self.show_punct);
        let cols = self.grid.columns;
        let rows = self.grid_rows;
        let reserve = crate::gloss::inscription_reserve(self.chapter_inscription_need(), rows);
        // 改动前的分页：定位半叶 N 与其起点、并原样复用前 N−1 面。
        let old = self.layout_pages();
        let new_pages = if self.notes_inline {
            crate::note_inject::relayout_from_halfleaf(&old, &flow, |g| {
                self.note_chars_with_source(*g)
                    .map(|(s, src)| (s.to_string(), src, *g))
            }, cols, rows, reserve, idx)
        } else {
            crate::note_inject::relayout_from_halfleaf(&old, &flow, |_| None, cols, rows, reserve, idx)
        };
        if new_pages.is_empty() {
            return;
        }
        // 宿主字现在落在第几叶 → 视图锚过去（防止增删注文后页码漂移）。
        let mut acc = 0usize;
        let mut target = 0usize;
        for (i, p) in new_pages.iter().enumerate() {
            let n = p.glyphs.len();
            if acc <= idx && idx < acc + n {
                target = i;
                break;
            }
            acc += n;
            target = i;
        }
        self.page = target.min(new_pages.len() - 1);
        self.sync_current_chapter();
    }

    /// 把当前书稿回写进 JSON（下次打开仍在）。抽出来供注入 / 删除共用。
    fn persist_book_json(&mut self) {
        let Some(path) = self.library.books.get(self.current).map(|e| e.path.clone()) else {
            return;
        };
        if let Some(book) = self.book.as_ref() {
            let _ = crate::importer::write_book_json(book, &path);
        }
    }

    /// 把草稿写进书稿里那条注文（并按原通道回写 JSON，下次打开仍在）。
    fn write_annotation(&mut self, key: (usize, usize, usize), text: String) {
        let (c, p, o) = key;
        let edited = if let Some(book) = self.book.as_mut() {
            if let Some(para) = book
                .chapters
                .get_mut(c)
                .and_then(|ch| ch.paragraphs.get_mut(p))
            {
                if let Some(n) = para
                    .notes
                    .iter_mut()
                    .find(|n| n.kind == NoteKind::Note && n.at == o)
                {
                    n.text = text;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if edited {
            self.persist_book_json();
            // 注文变长 / 变短 → 半格账变 → 后续字推走或回流；
            // 只续排后缀并把视图锚回原来在读的那一叶。
            self.reanchor_after_note_edit(key);
        }
    }

    /// 保存草稿并退出编辑态。
    pub fn commit_note_edit(&mut self) {
        // ★ v1.55.0 新制天头眉批**最先判**：草稿锚在 `anchor_char_index` 上，
        //   ⏎ 存进 `margin_notes`（旧制 `head_anchor` 的 u/v 分支在其后，
        //   两者互斥，互不干扰）。
        if let Some(at) = self.margin_draft_at.take() {
            let text = std::mem::take(&mut self.note_draft);
            let edit_id = self.margin_edit_id.take();
            self.note_editing = false;
            self.note_suppress_commit = false;
            self.note_cursor = 0;
            self.note_pos = None;
            match (text.trim().is_empty(), edit_id) {
                // 改写写空 = 抹掉这条（不留看不见的空条）。
                (true, Some(id)) => self.delete_margin_note(id),
                (true, None) => self.notice = "已取消眉批".to_string(),
                (false, Some(id)) => {
                    if let Some(n) = self.margin_notes.iter_mut().find(|m| m.id == id) {
                        n.content = text;
                    }
                    if let Some(book) = self.book.as_mut() {
                        book.margin_notes = self.margin_notes.clone();
                    }
                    self.persist_book_json();
                    self.notice = format!("眉批已改 · 共 {} 条", self.margin_notes.len());
                }
                (false, None) => self.push_margin_note(at, text),
            }
            return;
        }
        // 天头眉批（旧制 u/v）：草稿是就地写在天头上的，⏎ 存进 `head_notes`
        //   而不是夹注 / 页眉批注（判断只认 `head_anchor` 一枚开关）。
        //   v1.48.0：`head_edit` 非空 = 改写已有眉批（原位换文 / 清空即抹）。
        if let Some((key, u, v)) = self.head_anchor.take() {
            let text = std::mem::take(&mut self.note_draft);
            let edit_idx = self.head_edit.take();
            self.note_editing = false;
            self.note_suppress_commit = false;
            self.note_cursor = 0;
            self.note_pos = None;
            if text.trim().is_empty() {
                // 清空原文 = 抹掉这条（改写写空即删，不留看不见的空条）。
                if let Some(idx) = edit_idx {
                    self.delete_head_note(idx);
                } else {
                    self.notice = "已取消眉批".to_string();
                }
            } else if let Some(idx) = edit_idx.filter(|i| *i < self.head_notes.len()) {
                if let Some(n) = self.head_notes.get_mut(idx) {
                    n.text = text;
                }
                if let Some(book) = self.book.as_mut() {
                    book.head_notes = self.head_notes.clone();
                }
                self.persist_book_json();
                self.notice = format!("眉批已改 · 共 {} 条", self.head_notes.len());
            } else {
                self.push_head_note(key, u, v, text);
            }
            return;
        }
        let text = std::mem::take(&mut self.note_draft);
        let target = self.note_edit_note.take();
        let inject = self.note_inject_key.take();
        self.note_editing = false;
        self.note_suppress_commit = false;
        self.note_cursor = 0;
        self.note_pos = None;
        // 「新建夹注」优先：在该字上挂一条 `NoteSource::User` 的注文。
        // 夹注挂在字形上、不进正文流，所以只从当前半叶起点局部重排即可。
        // 空输入视为放弃（不注入空注，避免留一条看不见的夹注占位）。
        if let Some(key) = inject {
            if text.trim().is_empty() {
                self.notice = "已取消加夹注".to_string();
            } else {
                self.inject_user_note_at(key, &text);
            }
            return;
        }
        match target {
            Some(key) => {
                // 改的是书稿里的注文：写回段落并回写 JSON。
                self.write_annotation(key, text.clone());
                self.notice = if text.trim().is_empty() {
                    "注文已清空".to_string()
                } else {
                    "注文已保存".to_string()
                };
            }
            None => {
                self.set_note(self.page, text);
                if self.notes.contains_key(&self.page) {
                    self.notice = format!("已保存第 {} 葉笔记", self.page + 1);
                } else {
                    self.notice = "笔记已清空".to_string();
                }
            }
        }
    }

    /// 取消编辑（丢弃草稿）。
    pub fn cancel_note_edit(&mut self) {
        self.note_editing = false;
        self.note_suppress_commit = false;
        self.note_edit_note = None;
        self.note_inject_key = None;
        self.head_anchor = None;
        self.margin_draft_at = None;
        self.margin_edit_id = None;
        // v1.48.0：放弃改写也退出改写态（否则下一条新批会被当成改写）。
        self.head_edit = None;
        self.note_draft.clear();
        self.note_cursor = 0;
        self.note_pos = None;
    }

    /// 编辑栏键入：可打印字符插入到光标处。
    pub fn note_input_char(&mut self, c: char) {
        if self.note_draft.chars().count() < 500 {
            draft_insert(&mut self.note_draft, &mut self.note_cursor, c);
        }
    }

    /// 编辑栏退格：删光标前一字。
    pub fn note_backspace(&mut self) {
        draft_backspace(&mut self.note_draft, &mut self.note_cursor);
    }

    /// 编辑栏粘贴：把一段文本插入到光标处——中文笔记的录入方式。
    ///
    /// 剪贴板由调用方读（v1.71.0 起走原生 `crate::clipboard`，不再是
    /// `pbpaste` 子进程）；这里只管落字，保持「草稿串 + 字符光标」的纯粹。
    pub fn note_paste(&mut self, text: &str) {
        if !text.is_empty() {
            draft_insert_str(&mut self.note_draft, &mut self.note_cursor, text);
        }
    }

    // ---- 书库管理：重命名 / 删除 ----

    /// 打开书架第 idx 本书的重命名对话框（草稿预填当前书名）。
    pub fn start_rename(&mut self, idx: usize) {
        let Some(entry) = self.library.books.get(idx) else {
            return;
        };
        self.rename_draft = entry.title.clone();
        self.rename_cursor = self.rename_draft.chars().count();
        self.rename_idx = Some(idx);
        self.delete_confirm = None;
    }

    /// 取消重命名（丢弃草稿）。
    pub fn cancel_rename(&mut self) {
        self.rename_idx = None;
        self.rename_draft.clear();
        self.rename_cursor = 0;
    }

    /// 提交重命名：更新书架条目，json 书籍同步改写文件内的书名。
    pub fn commit_rename(&mut self) {
        let Some(idx) = self.rename_idx.take() else {
            return;
        };
        let title = self.rename_draft.trim().to_string();
        self.rename_draft.clear();
        self.rename_cursor = 0;
        if title.is_empty() {
            self.notice = "书名不能为空".to_string();
            return;
        }
        match self.library.rename(idx, &title) {
            Ok(()) => {
                // 正在读的就是这本书：同步阅读页里的书名。
                if self.current == idx {
                    if let Some(b) = self.book.as_mut() {
                        b.title = title.clone();
                    }
                }
                self.notice = format!("已重命名为：{title}");
            }
            Err(e) => self.notice = format!("重命名失败：{e}"),
        }
    }

    // ---- 批量管理（勾选 → 全选 / 反选 / 批量删除）----

    /// 进出批量管理态：进去时清掉旧勾选与确认态，出来时一并清空。
    pub fn toggle_shelf_manage(&mut self) {
        self.shelf_manage = !self.shelf_manage;
        self.shelf_picked.clear();
        self.shelf_batch_confirm = false;
        self.delete_confirm = None;
        self.notice = if self.shelf_manage {
            "整理：点书衣勾选，底部可一次清架（右键任一本也能直接勾选）".to_string()
        } else {
            String::new()
        };
    }

    /// 勾选 / 取消勾选第 idx 本（保持升序，便于「已选 N 册」与删除时的下标修正）。
    pub fn toggle_pick(&mut self, idx: usize) {
        match self.shelf_picked.iter().position(|i| *i == idx) {
            Some(pos) => {
                self.shelf_picked.remove(pos);
            }
            None => {
                self.shelf_picked.push(idx);
                self.shelf_picked.sort_unstable();
            }
        }
        // 勾选变了，旧的「确认删除」不该继续有效。
        self.shelf_batch_confirm = false;
    }

    pub fn is_picked(&self, idx: usize) -> bool {
        self.shelf_picked.contains(&idx)
    }

    /// 右键（或长按）书衣：直接进整理态并勾上这一本。
    ///
    /// 「整理」按钮在顶栏最右，容易看漏；书衣上右键是更就手的入口——
    /// 想删哪本就在哪本上直接动手。
    pub fn enter_manage_at(&mut self, idx: usize) {
        if !self.shelf_manage {
            self.shelf_manage = true;
            self.shelf_picked.clear();
        }
        if !self.shelf_picked.contains(&idx) {
            self.shelf_picked.push(idx);
            self.shelf_picked.sort_unstable();
        }
        self.shelf_batch_confirm = false;
        self.notice = "整理：已勾选此书，可继续点选或批量删除".to_string();
    }

    /// 反选（只作用于当前筛选出来的书目）：选中的撤掉、没选的勾上。
    pub fn invert_pick(&mut self, entries: &[(usize, crate::library::ShelfEntry)]) {
        for (i, _) in entries {
            match self.shelf_picked.iter().position(|x| x == i) {
                Some(pos) => {
                    self.shelf_picked.remove(pos);
                }
                None => self.shelf_picked.push(*i),
            }
        }
        self.shelf_picked.sort_unstable();
        self.shelf_batch_confirm = false;
    }

    /// 清空勾选（底部栏的「清选」）。
    pub fn clear_pick(&mut self) {
        self.shelf_picked.clear();
        self.shelf_batch_confirm = false;
    }

    /// 全选：按**当前筛选后的书目**勾选（看得见的才选）。
    /// 「取消全选 / 反选」各有独立入口（`clear_pick` / `invert_pick`），
    /// 不在一个按钮里做三态切换——按钮写什么就做什么。
    pub fn pick_all(&mut self, entries: &[(usize, crate::library::ShelfEntry)]) {
        for (i, _) in entries {
            if !self.shelf_picked.contains(i) {
                self.shelf_picked.push(*i);
            }
        }
        self.shelf_picked.sort_unstable();
        self.shelf_batch_confirm = false;
    }

    /// 批量删除：从大到小删，下标才不会被前面的删除带偏。
    pub fn delete_picked(&mut self) {
        let mut picked = std::mem::take(&mut self.shelf_picked);
        if picked.is_empty() {
            self.notice = "还没勾选要删除的书".to_string();
            return;
        }
        picked.sort_unstable();
        picked.dedup();
        let n = picked.len();
        let mut titles: Vec<String> = Vec::new();
        let mut failed = 0usize;
        for idx in picked.iter().rev() {
            let title = self
                .library
                .books
                .get(*idx)
                .map(|b| b.title.clone())
                .unwrap_or_default();
            match self.library.remove(*idx) {
                Ok(()) => {
                    if titles.len() < 3 {
                        titles.push(title);
                    }
                    // 修正「当前书」下标：删的是当前书就回书架。
                    if self.current == *idx {
                        self.book = None;
                        self.screen = Screen::Shelf;
                        self.current = 0;
                    } else if self.current > *idx {
                        self.current -= 1;
                    }
                }
                Err(e) => {
                    failed += 1;
                    self.notice = format!("删除失败：{e}");
                }
            }
        }
        self.shelf_batch_confirm = false;
        self.delete_confirm = None;
        if failed == 0 {
            let who = titles.join("》《");
            self.notice = if n > 3 {
                format!("已删除 {n} 本（《{who}》等）")
            } else {
                format!("已删除 {n} 本：《{who}》")
            };
            if self.library.books.is_empty() {
                self.notice = "书架已空，去「书库」挑一本吧".to_string();
            }
        }
    }

    /// 删除书架第 idx 本书（条目 + books/ 里的文件）。
    pub fn delete_book(&mut self, idx: usize) {
        let title = self
            .library
            .books
            .get(idx)
            .map(|b| b.title.clone())
            .unwrap_or_default();
        if let Err(e) = self.library.remove(idx) {
            self.notice = format!("删除失败：{e}");
            return;
        }
        self.delete_confirm = None;
        // 修正「当前书」下标：删的正是当前书就回书架。
        if self.current == idx {
            self.book = None;
            self.screen = Screen::Shelf;
            self.current = 0;
        } else if self.current > idx {
            self.current -= 1;
        }
        self.notice = if self.library.books.is_empty() {
            "书架已空，去「书库」挑一本吧".to_string()
        } else {
            format!("已删除：《{title}》")
        };
    }

    /// 把笔记与划线写回书架档案。
    pub fn save_annotations(&mut self) {
        let nts = self.notes.clone();
        let uls: Vec<(usize, usize, usize)> = self.underlines.iter().copied().collect();
        self.library.save_annotations(self.current, &nts, &uls);
    }

    // ---- 拖拽选字（竖排 / 横排连续划线）----

    /// 归一化后的选区范围（按 (chapter, para, offset) 字典序 = 阅读顺序）。
    pub fn selection_range(&self) -> Option<(GlyphKey, GlyphKey)> {
        let a = self.sel_anchor?;
        let f = self.sel_focus?;
        Some(crate::types::normalize_selection(a, f))
    }

    /// 某个字是否落在当前选区内。
    pub fn in_selection(&self, key: GlyphKey) -> bool {
        match self.selection_range() {
            Some((a, b)) => crate::types::key_in_range(key, a, b),
            None => false,
        }
    }

    /// 是否存在真正的拖选（起止不同）。
    pub fn has_selection(&self) -> bool {
        self.sel_anchor.is_some()
            && self.sel_focus.is_some()
            && self.sel_anchor != self.sel_focus
    }

    /// 清除选区（翻页 / 打开书本 / 收起工具条时调用）。
    pub fn clear_selection(&mut self) {
        self.sel_anchor = None;
        self.sel_focus = None;
        // v1.64.0：连「待分享选区」一并作废——点纸面 / 翻页 / 换书都走到这里，
        // 语义就是「这次选中的东西不要了」，分享入口不该再捞回来。
        self.share_sel = None;
        self.dragging = false;
        self.touched_glyph = false;
        self.sel_char = None;
    }

    /// 顶部品牌条（三态共用）：**笺读 · JianDu · 每日一句**。
    ///
    /// v1.47.0 立：品牌之后接**每日一句**（淡墨小字 + 出处），替掉静态标语
    /// 「为阅读创造环境」；诗句限宽 400px、超长截断缀省略号，绝不挤走右侧
    /// 内容。
    ///
    /// v1.52.0：**撤下自绘的窗口三钮**（✕ / — / □）——macOS 红绿灯已恢复
    /// （见 `main.rs` 的 `zen_window_options`），再留一套自绘钮就是「两个
    /// 关闭键」，既冗余又违背系统手感。关窗落盘改由 `render` 里挂的
    /// `on_window_should_close` 负责。
    pub fn render_brand(&mut self, _cx: &mut Context<Self>) -> Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::SongTi,
            crate::types::Script::Ancient,
        );
        let mut bar = div()
            .flex()
            .items_center()
            .gap(px(16.0))
            .child(
                div()
                    .text_size(px(16.0))
                    .font_family(song.clone())
                    .text_color(rgb(CINNABAR))
                    .child(SharedString::from("笺读")),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_family(song.clone())
                    .text_color(rgb(INK_SOFT))
                    .child(SharedString::from("JianDu")),
            )
            // 极细墨点：品牌与诗句之间的那枚「·」。
            .child(
                div()
                    .w(px(3.0))
                    .h(px(3.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(rgb(INK_FAINT)),
            );
        // 每日一句：诗句淡墨、出处更淡；取不到数据就退回静态标语。
        match crate::shelf::daily_quote() {
            Some((text, src)) => {
                bar = bar
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_family(song.clone())
                            .text_color(rgb(INK_SOFT))
                            .child(SharedString::from(clip_to_width(&text, 11.0, 400.0))),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_family(song.clone())
                            .text_color(rgb(INK_FAINT))
                            .child(SharedString::from(format!("—— {}", src))),
                    );
            }
            None => {
                bar = bar.child(
                    div()
                        .text_size(px(11.0))
                        .font_family(song.clone())
                        .text_color(rgb(INK_SOFT))
                        .child(SharedString::from("为阅读创造环境")),
                );
            }
        }
        bar
    }
}

/// 按**视觉宽度**截断（CJK 全角 ≈ 1em、西文 ≈ 0.55em），超宽则截断并缀
/// 省略号——顶栏标语专用：每日一句再长也不能挤走右侧的「续读」等钮。
fn clip_to_width(s: &str, fs: f32, max_w: f32) -> String {
    let mut w = 0.0f32;
    let mut out = String::new();
    for c in s.chars() {
        let cw = if (c as u32) < 0x2E80 { fs * 0.55 } else { fs };
        // 给省略号留一个字身。
        if w + cw > max_w - fs {
            out.push('…');
            return out;
        }
        w += cw;
        out.push(c);
    }
    out
}

// ---- 行内草稿编辑（浮层笔记框 / 重命名框共用：GPUI 无文本输入元素，
//      以「草稿串 + 字符光标」自绘，键入走 keystroke.key_char，
//      ⌘V 走 crate::clipboard 原生粘贴板）----
//
// 注：网格的列数 / 行数不再在这里写死——一律来自 `crate::grid::GridConfig`
// 与 `compute_grid`（8 / 10 / 12 / 14 列可切）。旧的固定常量（ANCIENT_ROWS /
// MODERN_LINES 等）已随网格引擎参数化一并移除。

/// 取草稿前 `cursor` 个字符组成的串（按字符边界切分）。
fn draft_head(draft: &str, cursor: usize) -> String {
    draft.chars().take(cursor).collect()
}

/// 在光标处插入一个字符。
pub fn draft_insert(draft: &mut String, cursor: &mut usize, c: char) {
    let head = draft_head(draft, *cursor);
    let tail: String = draft.chars().skip(*cursor).collect();
    draft.clear();
    draft.push_str(&head);
    draft.push(c);
    draft.push_str(&tail);
    *cursor += 1;
}

/// 在光标处插入一段文字。
pub fn draft_insert_str(draft: &mut String, cursor: &mut usize, s: &str) {
    for c in s.chars() {
        draft_insert(draft, cursor, c);
    }
}

/// 退格：删光标前一个字符；光标在 0 时无操作。
pub fn draft_backspace(draft: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let head = draft_head(draft, *cursor - 1);
    let tail: String = draft.chars().skip(*cursor).collect();
    draft.clear();
    draft.push_str(&head);
    draft.push_str(&tail);
    *cursor -= 1;
}

/// 光标左移 / 右移一个字符（不越界）。
pub fn draft_move(draft: &str, cursor: &mut usize, left: bool) {
    let len = draft.chars().count();
    if left {
        *cursor = cursor.saturating_sub(1);
    } else {
        *cursor = (*cursor + 1).min(len);
    }
}

// ---- 纸面 chrome 常量（与 reader.rs 的实际渲染严格对应，改渲染须同步改这里）----
// （现代横笺 chrome 已随「现代横笺」模式移除，只剩古籍竖笺。）
/// 双面对开两叶之间的「版心（中缝）」宽：约等于一行字宽，随列数走，
/// 由 `grid::gutter_w(content_w, columns)` 统一算出——渲染层与引擎同源，
/// 本文件不再另存副本（旧常量 `LEAF_GAP = 32` 已并入该函数）。
///
/// 古文竖笺纸面 chrome（横 / 纵共用）：sheet p_4(16×2=32) + 版框双边
/// （外 3×2 + 中缝 3×2 + 内 1×2 = 14）+ 余量 ≈ 50。
///
/// 版框内不再留横白：界行（乌丝栏）上下直接顶到内框，所以这个值比旧版
/// （112，按「天头一行 + 版框三层内边距」估）小得多——竖笺才真正铺满视口，
/// 与影印本「框内即是行格」的观感一致。
pub const ANCIENT_EDGE: f32 = 50.0;
/// 古文竖笺天头：篇题与书名已移进版框（章题列 + 地脚），天头不再单占一行。
pub const ANCIENT_HEAD: f32 = 0.0;
/// 古文竖笺地脚预留：鱼尾 + 葉码条由渲染层的 `foot_h` 单独承担，纵向留白
/// 完全交给网格引擎的 `margin_bot`（= 天头 ÷ 1.5，用户铁律「天头是地脚
/// 的 2 倍」，见 `grid::TOP_OF_BOT`），故此处归零。
pub const ANCIENT_FOOT: f32 = 0.0;
/// 古文竖笺页脚注区预留（脚注模式且开启注释时）。
pub const ANCIENT_NOTES: f32 = 76.0;
/// 章题列宽度系数：章题列就是「列数」里的格内一列，与正文列同宽。
/// 章首页 = (列数-1) 个正文列 + 1 个章题列，整面总宽恒为 列数×col_w。
pub const ANCIENT_TITLE_COL: f32 = 1.0;

// 主题色
pub const PAPER: u32 = 0xF7F1E3;
pub const PAPER_BACK: u32 = 0xEAE0C9;
pub const INK: u32 = 0x2B2622;
/// 次级墨（顶栏标语 / 每日一句 / 窗口钮的淡墨线条）。
pub const INK_SOFT: u32 = 0x6B6357;
/// 三级墨（出处等更淡一档的小字）。
pub const INK_FAINT: u32 = 0x9A9184;
pub const CINNABAR: u32 = 0xA63A2B;
pub const INDIGO: u32 = 0x3E5A8A;
pub const FRAME: u32 = 0x9A6B4F;
pub const RULE: u32 = 0xB98A63;

/// 首次运行时把「鱼尾」SVG 资源写到书架主目录，供目录浮层引用。
fn ensure_assets() {
    let dir = Library::home();
    let _ = std::fs::create_dir_all(&dir);
    let top = dir.join("fishtail_top.svg");
    let bot = dir.join("fishtail_bottom.svg");
    // 鱼尾纹：一枚朝下的实心三角，从顶边正中劈开一道窄缝，形似鱼尾（上尾朝下、下尾朝上）。
    // 注意 GPUI 只取 SVG 的 alpha 通道再用元素 text_color 上色，故 fill 必须为不透明实色。
    let top_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 13\"><path d=\"M0,0 L11,0 L12,13 Z M24,0 L13,0 L12,13 Z\" fill=\"#1A150F\"/></svg>";
    // 版心「对鱼尾」（黑鱼尾）：**直线五边形**，既不用 `<rect>` 画方块，
    // 也不用贝塞尔（那会画成圆滚滚的黑桃 / 蝙蝠翅膀）。
    // 上鱼尾：顶边平直 → 右翼竖直下垂 → 中轴凹口（锐角）→ 左翼竖直下垂；
    // 下鱼尾是它的垂直镜像，两枚开口相对。比例竖长（版心窄而高），
    // 高宽比见 `banxin::FISHTAIL_ASPECT`；墨色由渲染层给 `#333333` 深灰
    // （黑鱼尾不取纯黑）。见 reader::banxin_tail。
    let bx_top = dir.join("banxin_top.svg");
    let bx_bot = dir.join("banxin_bottom.svg");
    let _ = std::fs::write(
        &bx_top,
        crate::banxin::fishtail_svg(true, crate::banxin::FISHTAIL_ASPECT),
    );
    let _ = std::fs::write(
        &bx_bot,
        crate::banxin::fishtail_svg(false, crate::banxin::FISHTAIL_ASPECT),
    );
    // 信笺（14 列）模板的「对鱼尾」：同一条直线五边形路径，比例取横展
    // （照用户「信笺」参考图的 47×34 量级），见 reader::letter_fishtail。
    let yu_top = dir.join("yu_top.svg");
    let yu_bot = dir.join("yu_bottom.svg");
    let _ = std::fs::write(
        &yu_top,
        crate::banxin::fishtail_svg(true, crate::banxin::LETTER_TAIL_ASPECT),
    );
    let _ = std::fs::write(
        &yu_bot,
        crate::banxin::fishtail_svg(false, crate::banxin::LETTER_TAIL_ASPECT),
    );
    let bot_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 13\"><path d=\"M0,13 L11,13 L12,0 Z M24,13 L13,13 L12,0 Z\" fill=\"#1A150F\"/></svg>";
    // 句读「读」号：一枚小弧线（逗号 / 顿号 / 冒号类的记号），替代字体字形——
    // 极小字号下 `，。` 会缩成一枚「奇怪的黑痣」。GPUI 只取 alpha 再用 text_color
    // 着朱砂色，故 stroke 为不透明实色、fill 为 none。见 reader::punct_overlay。
    let punct_comma = dir.join("punct_comma.svg");
    let _ = std::fs::write(
        &punct_comma,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><path d=\"M5,19 A14,14 0 0 1 19,5\" fill=\"none\" stroke=\"#1A150F\" stroke-width=\"3.6\" stroke-linecap=\"round\"/></svg>",
    );
    // 朱砂小印「箋讀」的小篆字稿：两枚 100×100 的等粗圆头线篆（墨色由
    // 渲染层 text_color 换成朱砂红）。随系统没有篆体字库，索性照小篆结构
    // （左右对称、笔势圆匀、收笔不出锋）手写成路径，缩到 20px 见方也读得出
    // 是一方印。见 reader::banxin_seal。
    let seal_jian = dir.join("seal_jian.svg");
    let seal_du = dir.join("seal_du.svg");
    // 箋：上「竹」两枚圆头个形，下「戔」两戈相叠（下戈钩向左）。
    let jian_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\"><g fill=\"none\" stroke=\"#1A150F\" stroke-width=\"7\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\
<path d=\"M46,10 C39,18 34,26 31,32\"/><path d=\"M46,10 C47,20 45,32 43,44\"/>\
<path d=\"M54,10 C61,18 66,26 69,32\"/><path d=\"M54,10 C53,20 55,32 57,44\"/>\
<path d=\"M20,50 L84,50\"/><path d=\"M82,28 L80,66\"/><path d=\"M80,66 C80,74 74,78 66,78\"/><path d=\"M82,28 C74,32 66,36 58,40\"/>\
<path d=\"M16,84 L80,84\"/><path d=\"M21,60 L23,94\"/><path d=\"M23,94 C23,98 27,99 35,99\"/><path d=\"M21,60 C29,64 37,68 45,72\"/></g></svg>";
    // 讀：左「言」（横画 + 一口 + 中间两短横，中竖直贯），右「賣」
    // （上士、中网目、下貝）。
    let du_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\"><g fill=\"none\" stroke=\"#1A150F\" stroke-width=\"7\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\
<path d=\"M6,18 L40,14\"/><path d=\"M34,16 L28,92\"/>\
<path d=\"M8,34 L40,34\"/><path d=\"M8,60 L40,60\"/><path d=\"M8,34 L8,60\"/><path d=\"M40,34 L40,60\"/><path d=\"M14,47 L34,47\"/>\
<path d=\"M54,16 L98,16\"/><path d=\"M60,32 L92,32\"/><path d=\"M76,4 L76,32\"/>\
<path d=\"M56,44 L96,44\"/><path d=\"M56,72 L96,72\"/><path d=\"M56,44 L56,72\"/><path d=\"M96,44 L96,72\"/><path d=\"M62,56 L90,56\"/><path d=\"M66,63 L86,63\"/>\
<path d=\"M56,84 L96,84\"/><path d=\"M56,96 L96,96\"/><path d=\"M56,84 L56,96\"/><path d=\"M96,84 L96,96\"/><path d=\"M62,90 L90,90\"/></g></svg>";
    let _ = std::fs::write(&seal_jian, jian_svg);
    let _ = std::fs::write(&seal_du, du_svg);
    // 资源很小，每次启动覆盖写入，保证形状随版本更新。
    let _ = std::fs::write(&top, top_svg);
    let _ = std::fs::write(&bot, bot_svg);
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 记录窗口内容区尺寸，供阅读视图按视口铺满屏计算字号。
        // `Pixels` 字段对外部 crate 私有，借助 `Pixels / Pixels -> f32` 取出数值。
        let vp = window.viewport_size();
        let unit = px(1.0);
        let w = vp.width / unit;
        let h = vp.height / unit;
        self.viewport_w = if w > 0.0 { w } else { 1280.0 };
        self.viewport_h = if h > 0.0 { h } else { 860.0 };
        // v1.52.0：改回**系统红绿灯**后，「关闭」走的是 AppKit 的红叉，
        // 不再经过自绘那枚钮（那里原本会先 persist 再退进程）。故在此挂一次
        // 关窗钩子：红叉 / ⌘W 关闭前把阅读进度落盘，返回 true 放行关窗。
        // 只需挂一次（on_window_should_close 是覆盖式注册，反复挂只是浪费）。
        if !self.close_hook_set {
            self.close_hook_set = true;
            // on_window_should_close 的回调签名为 `Fn(&mut Window, &mut App) -> bool + 'static`，
            // 因此必须捕获一个**拥有的** `Entity<Root>`（'static），而不能用 `cx.listener`
            // 那种借用 `&mut Self` 的四参闭包。回调里通过 entity.update 再拿到 `this`。
            let ent = cx.entity();
            window.on_window_should_close(cx, move |_window, cx| {
                ent.update(cx, |this, _cx| {
                    this.persist();
                });
                true
            });
        }
        // 目录侧栏滑入 / 滑出：逐帧推进 toc_slide 直到到位，到位后
        // 若是「收起」则卸载侧栏。这里只改状态、不读窗口输入，
        // 真正的「下一帧」由 request_animation_frame 驱动，形成动画循环。
        if self.toc_animating {
            let dir = if self.toc_target { 1.0 } else { -1.0 };
            // 每帧约 12%：260ms 量级滑完，手感顺而不拖。
            let step = 0.12;
            let next = (self.toc_slide + dir * step).clamp(0.0, 1.0);
            self.toc_slide = next;
            if (self.toc_target && next >= 1.0) || (!self.toc_target && next <= 0.0) {
                self.toc_animating = false;
                if !self.toc_target {
                    self.show_toc = false; // 滑出完成，真正卸载侧栏
                }
            } else {
                window.request_animation_frame();
            }
        }
        // 朱批录侧栏滑入 / 滑出（与 toc 同款逐帧推进）。
        if self.notes_animating {
            let dir = if self.notes_target { 1.0 } else { -1.0 };
            let step = 0.12;
            let next = (self.notes_slide + dir * step).clamp(0.0, 1.0);
            self.notes_slide = next;
            if (self.notes_target && next >= 1.0) || (!self.notes_target && next <= 0.0) {
                self.notes_animating = false;
                if !self.notes_target {
                    self.show_notes_panel = false;
                }
            } else {
                window.request_animation_frame();
            }
        }
        // 书签抽屉滑入 / 滑出（与 toc 同款逐帧推进）。
        if self.bookmarks_animating {
            let dir = if self.bookmarks_target { 1.0 } else { -1.0 };
            let step = 0.12;
            let next = (self.bookmarks_slide + dir * step).clamp(0.0, 1.0);
            self.bookmarks_slide = next;
            if (self.bookmarks_target && next >= 1.0) || (!self.bookmarks_target && next <= 0.0) {
                self.bookmarks_animating = false;
                if !self.bookmarks_target {
                    self.show_bookmarks = false;
                }
            } else {
                window.request_animation_frame();
            }
        }
        // 卡片匣侧栏滑入 / 滑出（v1.74.0，与书签同款逐帧推进）。
        if self.cards_animating {
            let dir = if self.cards_target { 1.0 } else { -1.0 };
            let step = 0.12;
            let next = (self.cards_slide + dir * step).clamp(0.0, 1.0);
            self.cards_slide = next;
            if (self.cards_target && next >= 1.0) || (!self.cards_target && next <= 0.0) {
                self.cards_animating = false;
                if !self.cards_target {
                    self.show_cards = false;
                }
            } else {
                window.request_animation_frame();
            }
        }
        // 检索侧栏滑入 / 滑出。
        if self.search_animating {
            let dir = if self.search_target { 1.0 } else { -1.0 };
            let step = 0.12;
            let next = (self.search_slide + dir * step).clamp(0.0, 1.0);
            self.search_slide = next;
            if (self.search_target && next >= 1.0) || (!self.search_target && next <= 0.0) {
                self.search_animating = false;
                if !self.search_target {
                    self.show_search = false;
                }
            } else {
                window.request_animation_frame();
            }
        }
        // PNG 本叶导出：两段式——v1.52.3。
        // Warm 帧：面板已收、纸上只剩正文，并显示「正在印制本叶…」；
        // Shoot 帧：离屏重绘出图 + 弹系统保存框。
        // （v1.71.0 起出图走 CoreGraphics 离屏光栅，不再读屏幕。）
        if let Some(stage) = self.png_export {
            match stage {
                PngStage::Warm => {
                    self.png_export = Some(PngStage::Shoot);
                    window.request_animation_frame();
                }
                PngStage::Shoot => {
                    self.png_export = None;
                    self.export_page_png_shoot(window, cx);
                }
            }
        }
        let family = fonts::family_for(self.current_font(), self.script);
        let ent = cx.entity().downgrade();
        // 重命名框：把焦点交给它（中文输入法只在「有焦点 + 该元素注册了
        // handle_input」时才把组好的字送回来，见 ime::InputField）；
        // 对话框关掉后收回焦点，免得焦点悬在已不存在的输入框上，
        // 影响笔记框等其它输入。
        // 焦点归属：要接中文 IME 的输入态（重命名 / 检索）把焦点让给对应输入框；
        // 其余时候一律落在根元素的 key_focus 上——否则焦点为空，派发路径为空，
        // 挂在根 div 上的 on_key_down 收不到任何键，整组快捷键全部失效。
        let search_open = self.show_search && !self.focus_mode;
        if self.rename_idx.is_some() {
            window.focus(&self.rename_focus);
        } else if self.shelf_searching && self.screen == Screen::Shelf {
            // 书架搜索框同理：焦点在它身上，中文才送得进来。
            window.focus(&self.shelf_query_focus);
        } else if search_open {
            window.focus(&self.search_focus);
        } else if self.note_editing {
            // 笔记框要接中文：焦点必须落在它自己的句柄上，IME 才认这块区域。
            window.focus(&self.note_focus);
        } else if !self.key_focus.is_focused(window) {
            window.focus(&self.key_focus);
        }
        // 藏印只在阅读正文时出现：书架 / 封面是另一套书衣视觉，专注模式要
        // 把整幅纸面还给正文（一枚红印也嫌吵）。
        // 目录抽屉展开时收起纸角那枚朱印：印是画在 screen **之后**的，
        // 若不收，它会盖在抽屉左首列的末字上（看起来像脏点）。
        let show_seal = self.screen == Screen::Reading && !self.focus_mode && !self.show_toc;
        // v1.50.0：每帧探一眼后台导入（完成就登记入架，没完成就刷新进度）。
        self.poll_import(cx);
        // v1.70.0：**空闲帧渐进铺索引**。开卷后每帧只啃 8 章（≈1ms，60 FPS
        // 纹丝不动），一两秒内把全书的「第几叶在第几章」探明；之后点目录
        // 跳任何一章都是 O(log n) 查表（130µs），根本轮不到「冷跳到末叶」
        // 那一百多毫秒的卡顿。只在阅读屏做，导入中不去抢它的时间。
        if self.screen == Screen::Reading && self.import_job.is_none() {
            let more = match (self.pager.as_mut(), self.book.as_mut()) {
                (Some(pg), Some(book)) if !pg.is_complete() => {
                    pg.scan_more(book, INDEX_WARM_CHAPTERS)
                }
                _ => false,
            };
            if more {
                cx.notify();
            }
        }
        let screen = self.render_screen(cx);
        let key_focus = self.key_focus.clone();
        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .font_family(family)
            // 把根元素登记进派发树：只有 track_focus 之后，上面 window.focus(&key_focus)
            // 才有一条「焦点节点 → 根元素」的路径，按键才送得到根元素上的监听器。
            .track_focus(&key_focus)
            // 键盘总入口必须挂在**窗口根元素**上。
            // GPUI 0.2.2 只把按键派发给「焦点节点 → 根元素」这条路径上的监听器；
            // 本应用没有任何可聚焦元素（没有输入框），焦点为空时派发路径只有根元素
            // 一个节点——挂在 render_screen 返回的子层上，整组快捷键（含重命名键入、
            // ← → 翻页、笔记输入）会静默失效。
            .on_key_down(move |event: &KeyDownEvent, _window, app| {
                let _ = ent.update(app, |this, cx| this.dispatch_key(event, cx));
            });
        // 藏印「箋讀」：盖在**纸页左下角外侧**（v1.28.0 由中缝移来）。
        // 必须画在 `screen` **之后**：正文屏是整幅不透明纸面，先画的元素会
        // 被它整块盖住（第一版就是这样，印根本看不见）。印章只在左下角
        // 一小块，压不到居中的浮层。
        root = root.child(screen);
        // 「雕版刻印中… 34%」：盖在最上层，导入期间挡住操作，免得重复点导入。
        let import_ui = self
            .import_job
            .as_ref()
            .map(|j| (j.name.clone(), j.label(), j.pct()));
        if let Some((name, label, pct)) = import_ui {
            root = root.child(Self::import_overlay(&name, &label, pct));
        }
        if show_seal {
            root = root.child(
                div()
                    .absolute()
                    .left(px(12.0))
                    .bottom(px(30.0))
                    .child(crate::reader::banxin_seal(22.0, 40.0)),
            );
        }
        root
    }
}

/// 用**系统默认浏览器**打开一个网址（「在线查阅」用）。
///
/// 不引 webbrowser 之类依赖：三个平台各一条系统命令就够了，
/// 失败也只回个 false，由调用方在状态栏提示，绝不 panic。
pub fn open_external(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "linux")]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let r: std::io::Result<std::process::Child> =
        Err(std::io::Error::other("unsupported platform"));
    r.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    // 单测验的是「算式本身」，直接调 `compute_grid` 绕开 `calculate_grid`
    // 的三模式分派（非测试代码一律走 `calculate_grid` 这个唯一入口）。
    use crate::grid::compute_grid;

    /// 默认窗口 1320×880、普通模式（工具栏 + 状态栏 + 四边留白）。
    const VW: f32 = 1320.0;
    const VH: f32 = 880.0;

    fn content() -> (f32, f32) {
        ((VW - 44.0).max(220.0), (VH - 78.0 - 44.0).max(220.0))
    }

    /// 用网格引擎算一张纸面（普通模式、缩放 1.0；脚注区默认不预留）。
    /// 「去掉现代」后只剩古籍竖笺，横排分支与本方常量一律不参与。
    fn grid_metrics(columns: usize, facing: bool) -> crate::grid::GridMetrics {
        let (cw, ch) = content();
        let cfg = GridConfig {
            columns,
            dir: GridDir::Vertical,
            margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
            // 行款字数：按列数查行款表（v1.32.0）。
            target_chars: crate::grid::LayoutConfig::by_columns(columns)
                .map(|c| c.target_chars)
                .unwrap_or(22),
            // 留白默认档「中（75%）」（v1.33.0）。
            banxin_ratio: crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT],
        };
        compute_grid(
            cw,
            ch,
            cfg,
            facing,
            1.0,
            ANCIENT_EDGE,
            ANCIENT_HEAD,
            ANCIENT_FOOT,
            0.0, crate::grid::GRID_FS_MAX)
    }

    #[test]
    fn ancient_focus_mode_fills_screen() {
        // 专注单面：竖笺横向、纵向一起撑满（focus: 无 chrome/pad → content=VW×VH）。
        let cfg = GridConfig {
            columns: 8,
            dir: GridDir::Vertical,
            margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
            target_chars: 14,
            banxin_ratio: crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT],
        };
        let m = compute_grid(
            VW,
            VH,
            cfg,
            false,
            1.0,
            ANCIENT_EDGE,
            ANCIENT_HEAD,
            ANCIENT_FOOT,
            0.0, crate::grid::GRID_FS_MAX);
        assert!(m.outer_w <= VW + 0.5, "专注单面纸宽 {:.1} 超 {VW}", m.outer_w);
        assert!(
            m.fs > 24.0,
            "专注单面字级偏小：{:.1}（需 > 24.0）",
            m.fs
        );
        assert!(m.rows >= 1);
    }

    #[test]
    fn ancient_facing_focus_fills_screen() {
        // 专注对开（模拟横屏 iPad）：两叶 + 中缝撑满整宽，高度不溢出。
        let cfg = GridConfig {
            columns: 8,
            dir: GridDir::Vertical,
            margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
            target_chars: 14,
            banxin_ratio: crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT],
        };
        let gap = crate::grid::gutter_w(VW, 8, ANCIENT_EDGE, cfg.margin);
        let m = compute_grid(
            VW,
            VH,
            cfg,
            true,
            1.0,
            ANCIENT_EDGE,
            ANCIENT_HEAD,
            ANCIENT_FOOT,
            0.0,
            // 专注模式不设字号上限（`fs_max = INFINITY`）：整屏只排一叶，
            // 就是要大字撑满，否则触上限后纸会缩到七成、四周留白。
            f32::INFINITY);
        assert!(2.0 * m.outer_w + gap <= VW + 0.5);
        // 行款版式（v1.32.0）：字格取方，版心（界行区）窄于纸面——宋版书
        // 版心本就只占纸宽六成，两侧留白归纸面，故不再要求铺到 95%。
        // 底线：两叶 + 中缝仍占得下大部分宽度（不是两条细缝）。
        assert!(2.0 * m.outer_w + gap > VW * 0.4);
        assert!(m.outer_h <= VH + 0.5, "对开纸高 {:.1} 超 {VH}", m.outer_h);
        // 中缝容得下一对鱼尾：与字格同量级（0.5×~2×），不窄于半格、
        // 不宽过两格（宽过两格就成了两片孤叶）。
        assert!(
            gap >= m.col_w * 0.5 && gap <= m.col_w * 2.0,
            "中缝宽 {gap:.1} 与字格宽 {:.1} 不成比例",
            m.col_w
        );
    }

    #[test]
    fn ancient_single_fits_with_title_column() {
        let (cw, ch) = content();
        let m = grid_metrics(8, false);
        // 章首页（7 正文列 + 1 章题列）也放得下。
        assert!(m.outer_w <= cw + 0.5, "古文纸宽 {:.1} 超 {cw}", m.outer_w);
        assert!(m.outer_h <= ch + 0.5, "古文纸高 {:.1} 超 {ch}", m.outer_h);
        assert!(m.rows >= 1);
    }

    #[test]
    fn ancient_facing_fits_two_leaves() {
        // 古文竖笺双面对开：两叶并排 + 中缝不得超出内容宽度。
        let (cw, _ch) = content();
        let m = grid_metrics(8, true);
        let gap = crate::grid::gutter_w(cw, 8, ANCIENT_EDGE, 0.04);
        assert!(
            2.0 * m.outer_w + gap <= cw + 0.5,
            "古文双面纸宽 {:.1}×2 + 中缝 超 {cw}",
            m.outer_w
        );
        // 字号不低于最小字号（不会被折半压到看不清）。
        assert!(m.fs >= 14.0 - 0.01, "古文双面字级低于最小字号：{}", m.fs);
    }

    #[test]
    fn zoom_respects_viewport_when_smaller() {
        // 缩小字号只让纸更小，不会反向溢出。
        let (cw, ch) = content();
        let cfg = GridConfig {
            columns: 8,
            dir: GridDir::Vertical,
            margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
            target_chars: 14,
            banxin_ratio: crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT],
        };
        let m = compute_grid(
            cw,
            ch,
            cfg,
            true,
            0.7,
            ANCIENT_EDGE,
            ANCIENT_HEAD,
            ANCIENT_FOOT,
            0.0, crate::grid::GRID_FS_MAX);
        assert!(2.0 * m.outer_w + crate::grid::gutter_w(cw, 8, ANCIENT_EDGE, 0.04) <= cw + 0.5);
    }

    #[test]
    fn ancient_paginate_drops_no_glyph() {
        use crate::types::{Book, Chapter, Paragraph};
        // 三章、每章两段，段长足以铺满多面。
        let mut b = Book::new("分页测试", "佚名");
        for ci in 0..3 {
            let mut ch = Chapter {
                title: format!("第{ci}章"),
                paragraphs: Vec::new(),
            };
            for _ in 0..2 {
                ch.paragraphs
                    .push(Paragraph::plain("天地玄黄宇宙洪荒日月盈昃辰宿列张".repeat(6)));
            }
            b.chapters.push(ch);
        }
        let flow = b.to_flow(true);
        // 列数 / 行数现在由网格引擎决定；这里按默认 8 列 × 11 行验证。
        let (cols, rows) = (8usize, 11usize);
        let pages = crate::gloss::layout_book(&flow, |_| None, cols, rows, 0);
        // ① 一个字都不能丢：拍平后与字流逐一对应。
        let flat: Vec<&Glyph> = pages.iter().flat_map(|p| p.glyphs.iter()).collect();
        assert_eq!(flat.len(), flow.len(), "分页丢了字");
        for (a, b) in flat.iter().zip(flow.iter()) {
            assert_eq!((a.chapter, a.para, a.offset, a.ch), (b.chapter, b.para, b.offset, b.ch));
        }
        // ② 每面列数不超渲染侧 limit：章首页 ≤ 7 列，普通页 ≤ 8 列。
        for pg in &pages {
            let cap = if pg.start_chapter { cols - 1 } else { cols };
            assert!(pg.columns.len() <= cap, "某面超限：{} 列", pg.columns.len());
            if pg.start_chapter {
                // 章首页的第一个字必须是该章首字（顶格起排，不再有段首缩进占位）。
                let first = pg.glyphs[0];
                assert!(!first.is_indent() && first.offset == 0);
            }
        }
        // ③ 至少跨了多面（否则测不出东西）。
        assert!(pages.len() >= 4, "面数太少：{}", pages.len());
    }

    #[test]
    fn ancient_paginate_with_gloss_respects_columns() {
        use crate::types::{Book, Chapter, Paragraph};
        let mut b = Book::new("夹注分页", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![Paragraph::plain("一二三四五六七八九十".repeat(30))],
        });
        let flow = b.to_flow(true);
        let (cols, rows) = (8usize, 11usize);
        // 奇数字挂 4 字夹注：成本变化也不得丢字、不得超 8 列。
        let pages = crate::gloss::layout_book(
            &flow,
            |g| (g.kind == GlyphKind::Char && g.offset % 2 == 0)
                .then(|| ("夹注四字".to_string(), NoteSource::Original, *g)),
            cols,
            rows,
            0,
        );
        let flat: Vec<&Glyph> = pages.iter().flat_map(|p| p.glyphs.iter()).collect();
        assert_eq!(flat.len(), flow.len());
        for pg in &pages {
            let cap = if pg.start_chapter { cols - 1 } else { cols };
            assert!(pg.columns.len() <= cap);
        }
        // 夹注总量守恒：4 字注一条不少。
        let joined: String = pages
            .iter()
            .flat_map(|p| p.columns.iter())
            .flatten()
            .filter_map(|p| match p {
                crate::gloss::Piece::Gloss(s, _, _) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        let want = flow
            .iter()
            .filter(|g| g.kind == GlyphKind::Char && g.offset % 2 == 0)
            .count()
            * 4;
        assert_eq!(joined.chars().count(), want, "夹注丢失");
    }

    #[test]
    fn notes_inline_toggle_keeps_glyph_keys_identical() {
        // 切换「双行夹注 ⇄ 页脚注」只改变列内装排（夹注是否挤进正文），
        // 不应增删任何字——这是 set_notes_inline 用「首字锚点」重定位页码
        // 能永远命中的前提。
        use crate::types::{Book, Chapter, Note, NoteKind, Paragraph};
        let mut b = Book::new("锚点保持", "佚名");
        let mut p = Paragraph::plain("天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往".repeat(6));
        p.notes.push(Note::span(0, 4, NoteKind::Person, "人名"));
        p.notes.push(Note::new(10, NoteKind::Note, "这是一条夹注".to_string()));
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![p],
        });
        let flow = b.to_flow(true);
        let (cols, rows) = (8usize, 11usize);
        let keys_of = |inline: bool| -> Vec<(usize, usize, usize, char)> {
            let pages = crate::gloss::layout_book(
                &flow,
                |g| {
                    if inline {
                        (g.offset % 7 == 0)
                            .then(|| ("注".to_string(), NoteSource::Original, *g))
                    } else {
                        None
                    }
                },
                cols,
                rows,
                0,
            );
            pages
                .iter()
                .flat_map(|p| p.glyphs.iter())
                .map(|g| (g.chapter, g.para, g.offset, g.ch))
                .collect()
        };
        assert_eq!(
            keys_of(true),
            keys_of(false),
            "切换夹注不应增删任何字，锚点才能重定位"
        );
    }

    #[test]
    fn ancient_paginate_any_columns_drops_no_glyph() {
        use crate::types::{Book, Chapter, Paragraph};
        let mut b = Book::new("分页任意列", "佚名");
        for ci in 0..2 {
            b.chapters.push(Chapter {
                title: format!("第{ci}章"),
                paragraphs: vec![Paragraph::plain("天地玄黄宇宙洪荒日月盈昃".repeat(8))],
            });
        }
        let flow = b.to_flow(true);
        // 任意列数（8/10/12/14）都不丢字、每面不超渲染侧列上限。
        for cols in crate::grid::GRID_COLUMN_CHOICES {
            let rows = 11;
            let pages = crate::gloss::layout_book(&flow, |_| None, cols, rows, 0);
            let flat: Vec<&Glyph> = pages.iter().flat_map(|p| p.glyphs.iter()).collect();
            assert_eq!(flat.len(), flow.len(), "{cols} 列分页丢了字");
            for pg in &pages {
                let cap = if pg.start_chapter { cols - 1 } else { cols };
                assert!(pg.columns.len() <= cap, "{cols} 列某面超限：{}", pg.columns.len());
            }
        }
    }

    #[test]
    fn draft_editing_with_cursor() {        let mut d = String::from("笔记");
        let mut c = 2;
        draft_insert(&mut d, &mut c, '好');
        assert_eq!(d, "笔记好");
        assert_eq!(c, 3);
        // 光标回到 1 处插入。
        draft_move(&d, &mut c, true);
        draft_move(&d, &mut c, true);
        assert_eq!(c, 1);
        draft_insert(&mut d, &mut c, '一');
        assert_eq!(d, "笔一记好");
        // 退格删「一」。
        draft_backspace(&mut d, &mut c);
        assert_eq!(d, "笔记好");
        assert_eq!(c, 1);
        // 粘贴到光标处。
        draft_insert_str(&mut d, &mut c, "AB");
        assert_eq!(d, "笔AB记好");
        assert_eq!(c, 3);
        // 边界：光标 0 退格无操作；右移不越界。
        draft_move(&d, &mut c, true);
        draft_move(&d, &mut c, true);
        draft_move(&d, &mut c, true);
        assert_eq!(c, 0);
        draft_backspace(&mut d, &mut c);
        assert_eq!(d, "笔AB记好");
        draft_move(&d, &mut c, false);
        for _ in 0..20 {
            draft_move(&d, &mut c, false);
        }
        assert_eq!(c, d.chars().count());
    }

    /// **排版耗时实测 + 「按章分页 ≡ 全书分页」等价性验证**（v1.60.0）。
    ///
    /// 平时 `#[ignore]`（要读真书稿、跑满全书），只在架构决策时手动跑：
    /// `cargo test --release -- --ignored --nocapture bench_layout`
    ///
    /// 它回答两个问题：① 百万字全书排版到底多贵（决定要不要惰性化）；
    /// ② 逐章排完串起来，与全书一次排是否逐字同面（决定能不能惰性化）。
    #[test]
    #[ignore]
    fn bench_layout_and_chapterwise_equivalence() {
        use std::time::Instant;
        let p = jiandu_core::library::Library::home().join("books/船山遗书.json");
        if !p.exists() {
            eprintln!("[bench] 书稿不在，跳过");
            return;
        }
        let t0 = Instant::now();
        let book = crate::importer::import_json(&p).expect("读稿失败");
        let chars: usize = book
            .chapters
            .iter()
            .flat_map(|c| c.paragraphs.iter())
            .map(|q| q.text.chars().count())
            .sum();
        println!(
            "[bench] import {:?}　章 {}　字 {}",
            t0.elapsed(),
            book.chapters.len(),
            chars
        );

        let cols = 12usize;
        let rows = 22usize;

        let t1 = Instant::now();
        let flow = book.to_flow(true);
        println!("[bench] to_flow {} 字　{:?}", flow.len(), t1.elapsed());

        let t2 = Instant::now();
        let pages =
            crate::gloss::layout_book_from_opts(&flow, |_| None, cols, rows, 0, true, false);
        println!(
            "[bench] 全书排版 {} 面　{:?}",
            pages.len(),
            t2.elapsed()
        );

        // ---- 按章排：等价性 + 单块耗时 ----
        let t3 = Instant::now();
        let mut total2 = 0usize;
        let mut flat2: Vec<(usize, usize, usize)> = Vec::new();
        let mut worst = std::time::Duration::ZERO;
        for ci in 0..book.chapters.len() {
            let f = book.flow_of_chapter(ci, true);
            let tc = Instant::now();
            let ps =
                crate::gloss::layout_book_from_opts(&f, |_| None, cols, rows, 0, true, false);
            let el = tc.elapsed();
            if el > worst {
                worst = el;
            }
            for pg in &ps {
                for g in &pg.glyphs {
                    flat2.push((g.chapter, g.para, g.offset));
                }
            }
            total2 += ps.len();
        }
        println!(
            "[bench] 按章排 {} 面　{:?}　最慢一章 {:?}",
            total2,
            t3.elapsed(),
            worst
        );

        // 只排 3 章（秒开场景：当前章 + 前后各一章）的耗时。
        let t4 = Instant::now();
        for ci in 0..book.chapters.len().min(3) {
            let f = book.flow_of_chapter(ci, true);
            let _ = crate::gloss::layout_book_from_opts(&f, |_| None, cols, rows, 0, true, false);
        }
        println!("[bench] 只排 3 章　{:?}", t4.elapsed());

        let flat1: Vec<(usize, usize, usize)> = pages
            .iter()
            .flat_map(|pg| pg.glyphs.iter().map(|g| (g.chapter, g.para, g.offset)))
            .collect();
        println!(
            "[bench] 面数 {} vs {}　字序逐字一致：{}",
            pages.len(),
            total2,
            flat1 == flat2
        );
        assert_eq!(pages.len(), total2, "按章排与全书排的面数不一致");
        assert!(flat1 == flat2, "按章排与全书排的字序不一致");
    }

    /// **开卷代价实测**（v1.60.0）：`hydrate_all`（从 SQLite 把全书正文读回
    /// 内存）与全书排版的耗时对比，外加「只 hydrate 当前章」的秒开耗时。
    ///
    /// 用**真实的《资治通鉴》**（799 章 / 12072 叶）跑，平时 `#[ignore]`。
    #[test]
    #[ignore]
    fn bench_open_cost_on_real_book() {
        use std::time::Instant;
        let db = jiandu_core::store::db_path();
        if !db.exists() {
            eprintln!("[bench] 没有 chapters.db，跳过");
            return;
        }
        let Ok(conn) = crate::store::connect() else {
            eprintln!("[bench] 连不上库");
            return;
        };
        // 库里第一本书（真机上是《资治通鉴》）。
        let Ok(id) = conn.query_row("select store_id from books limit 1", [], |r| r.get::<_, String>(0)) else {
            eprintln!("[bench] 库里没书");
            return;
        };
        let Ok(meta) = conn.query_row("select meta_json from books where store_id=?1", [&id], |r| r.get::<_, String>(0)) else {
            eprintln!("[bench] 取不到 meta");
            return;
        };
        let mut book: crate::types::Book = match serde_json::from_str(&meta) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[bench] meta 解析失败：{e}");
                return;
            }
        };
        let n = crate::store::chapter_count(&conn, &id);
        book.store_id = Some(id.clone());
        book.chapters = (0..n)
            .map(|_| crate::types::Chapter {
                title: String::new(),
                paragraphs: Vec::new(),
            })
            .collect();

        println!("[bench] 书：{}　章 {}", book.title, n);

        // ① 单章 hydrate（秒开场景：只取当前章）
        let t0 = Instant::now();
        let one = crate::stream::hydrate_chapter(&id, n / 2);
        println!(
            "[bench] 单章 hydrate {:?}　{} 字",
            t0.elapsed(),
            one.as_ref().map(|c| c.char_count()).unwrap_or(0)
        );

        // ② 全书 hydrate
        let t1 = Instant::now();
        crate::stream::hydrate_all(&mut book);
        let chars: usize = book
            .chapters
            .iter()
            .flat_map(|c| c.paragraphs.iter())
            .map(|q| q.text.chars().count())
            .sum();
        println!("[bench] 全书 hydrate {:?}　{} 字", t1.elapsed(), chars);

        // ③ 全书排版
        let t2 = Instant::now();
        let flow = book.to_flow(true);
        println!("[bench] to_flow {:?}", t2.elapsed());
        let t3 = Instant::now();
        let pages =
            crate::gloss::layout_book_from_opts(&flow, |_| None, 12, 22, 0, true, false);
        println!("[bench] 全书排版 {} 面　{:?}", pages.len(), t3.elapsed());
    }

    /// **狂滚实测**（v1.60.0）：用真实的《资治通鉴》模拟「连翻 300 叶」，
    /// 量每一叶的取页成本（首帧建索引另计）。
    #[test]
    #[ignore]
    fn bench_pager_scroll_300_leaves() {
        use std::time::Instant;
        let db = jiandu_core::store::db_path();
        if !db.exists() {
            eprintln!("[bench] 没有 chapters.db，跳过");
            return;
        }
        let Ok(conn) = crate::store::connect() else { return };
        let Ok(id) = conn.query_row("select store_id from books limit 1", [], |r| r.get::<_, String>(0)) else { return };
        let Ok(meta) = conn.query_row("select meta_json from books where store_id=?1", [&id], |r| r.get::<_, String>(0)) else { return };
        let mut book: crate::types::Book = match serde_json::from_str(&meta) {
            Ok(b) => b,
            Err(_) => return,
        };
        let n = crate::store::chapter_count(&conn, &id);
        book.store_id = Some(id.clone());
        book.chapters = (0..n)
            .map(|_| crate::types::Chapter { title: String::new(), paragraphs: Vec::new() })
            .collect();

        let key = crate::pager::LayoutKey {
            cols: 12,
            rows: 22,
            reserve: 0,
            show_punct: true,
            inline_notes: false,
            west_mode: false,
        };
        let mut pg = crate::pager::Pager::new(n, key, Some(id.clone()));

        // ① 开卷首帧：建索引（hydrate 沿途各章 + 逐章取面数）
        let t0 = Instant::now();
        let total = {
            crate::stream::hydrate_all(&mut book);
            pg.scan_all(&mut book);
            pg.known_total()
        };
        println!("[bench] 首帧建索引 {:?}　共 {} 叶", t0.elapsed(), total);

        // ② 狂滚 300 叶（每叶都要真的取出来：模拟连续翻页 / 惯性滚动）
        let t1 = Instant::now();
        let mut worst = std::time::Duration::ZERO;
        let mut got = 0usize;
        for k in 0..300 {
            let i = (k * 37) % total.max(1); // 跳着取，逼 LRU 换章
            let t = Instant::now();
            if pg.page(&mut book, i).is_some() {
                got += 1;
            }
            let el = t.elapsed();
            if el > worst {
                worst = el;
            }
        }
        let per = t1.elapsed();
        println!(
            "[bench] 连取 300 叶 {:?}（平均 {:?} / 叶，最慢 {:?}）　取到 {} 叶",
            per,
            per / 300,
            worst,
            got
        );
        println!("[bench] LRU 窗口内常驻章数：{}", pg.cache_len());

        // ③ 冷启动跳到最后一叶：模拟「从目录点最后一章 / 恢复书签」。
        //    索引是空的（`scanned == 0`），`page()` 必须一路扫到末章才拿得到
        //    那一叶——这一趟全是同步的 SQLite 读 + 逐章排版，若不限流就会
        //    整帧卡在这里（v1.70.0 要修的正是它）。
        let fresh_book = || crate::types::Book {
            store_id: Some(id.clone()),
            chapters: (0..n)
                .map(|_| crate::types::Chapter { title: String::new(), paragraphs: Vec::new() })
                .collect(),
            ..book.clone()
        };
        let mut b2 = fresh_book();
        let mut pg2 = crate::pager::Pager::new(n, key, Some(id.clone()));
        let t2 = Instant::now();
        let target = total.saturating_sub(1);
        let hit = pg2.page(&mut b2, target).is_some();
        let el = t2.elapsed();
        println!(
            "[bench] 冷跳到末叶 #{} {:?}　命中={}　（>16ms 即掉帧，>100ms 就是用户能感觉到的卡）",
            target, el, hit
        );
        // 半程跳：只扫一半，成本应当约为全程的一半（线性 = 没有隐藏的 O(n²)）。
        let mut b3 = fresh_book();
        let mut pg3 = crate::pager::Pager::new(n, key, Some(id.clone()));
        let t3 = Instant::now();
        let half = total / 2;
        let hit3 = pg3.page(&mut b3, half).is_some();
        println!(
            "[bench] 冷跳到半程 #{} {:?}　命中={}",
            half, t3.elapsed(), hit3
        );
    }
}
