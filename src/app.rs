//! 应用根视图：书架 / 封面 / 阅读 三态。
//!
//! 启动先进「书架」，点书进「封面」，再进「阅读」。
//! 每本书的阅读档案（第几面、模式、字体、标注开关）会即时写回 library.json。

use crate::fonts;
use crate::grid::{compute_grid, GridConfig, GridDir, GRID_COLUMN_CHOICES};
use crate::library::Library;
use crate::theme::{InkPalette, Theme};
use crate::types::{
    Book, CardLayout, Duplex, FontChoice, Glyph, GlyphHit, GlyphKey, GlyphKind, MarkKind, Note,
    NoteKind, RuleColor, Script, StyleTab,
};
use gpui::{
    div, px, rgb, AsyncApp, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, Styled, WeakEntity, Window,
};
use std::path::PathBuf;

/// 当前所在界面。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    /// 书架（书封墙）。
    Shelf,
    /// 单本书的封面。
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
    /// 版式控制面板（列数 / 方向 / 留白）是否展开。
    pub show_grid_panel: bool,
    pub duplex: Duplex,
    pub font_ancient: FontChoice,
    pub font_modern: FontChoice,
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
    /// 目录面板滑入 / 滑出动画进度（0 = 完全收起，1 = 完全展开）。
    pub toc_slide: f32,
    /// 目录动画的目标态（true = 展开）。
    pub toc_target: bool,
    /// 是否正在播放目录滑入 / 滑出动画。
    pub toc_animating: bool,
    /// 样式面板是否展开。
    pub show_style: bool,
    /// 专注模式：隐藏工具栏、状态栏与目录，沉浸阅读。
    pub focus_mode: bool,
    /// 字号缩放系数（A-/A+ 调节；配合视口铺满使用）。
    pub zoom: f32,
    /// 点字查字面板当前查询的字（None 表示未打开）。
    pub lookup: Option<String>,
    /// 当前书的笔记：页序号 -> 正文。
    pub notes: std::collections::HashMap<usize, String>,
    /// 笔记面板是否展开。
    pub show_notes_panel: bool,
    /// 笔记编辑态：true 时正文区弹出浮层编辑栏，键盘全部路由给编辑。
    pub note_editing: bool,
    /// 编辑栏正在改的是**哪条注文**：`Some((chapter, para, offset))` 表示
    /// 改的是书稿里的夹注 / 注释（保存时写回书稿 JSON）；
    /// `None` 表示改的是页眉批注（`notes[page]`，既有行为）。
    pub note_edit_note: Option<(usize, usize, usize)>,
    /// 笔记编辑栏的当前草稿（未保存的输入缓冲）。
    pub note_draft: String,
    /// 笔记草稿的光标位置（字符下标，0..=len）。
    pub note_cursor: usize,
    /// 笔记框的焦点句柄：中文 IME 只把组好的字送给「有焦点且注册了
    /// handle_input」的元素，没有它笔记框就打不进汉字（见 ime.rs）。
    pub note_focus: FocusHandle,
    /// 笔记框 IME 组字中的标记区间（UTF-16 下标）。
    pub note_marked: Option<std::ops::Range<usize>>,
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
    /// 分享卡文字版式（八行笺 / 竖排分列 / 单列竖排；只要竖排，横排已删）。
    pub share_layout: CardLayout,
    /// 分享卡底色索引（见 reader::SHARE_BGS）。
    pub share_bg: usize,
    /// 丝栏（竖排列间界行）颜色：乌丝栏 / 金丝栏 / 朱丝栏，样式面板可选。
    pub rule_color: RuleColor,
    /// 分享卡在窗口内容区里的矩形 (x, y, w, h)，渲染时写入，
    /// 保存 / 分享图片时据此只截卡片，而不是整块屏幕。
    pub share_card_rect: Option<(f32, f32, f32, f32)>,
    /// 开发用：自动存卡的帧计数（到第 3 帧才截）。
    pub share_shot_frames: u8,
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
    /// 样式面板当前标签页：样式 / 字体。
    pub style_tab: StyleTab,
    /// 标点显隐：true 显示句读（朱色小字），false 隐藏（同时影响分页）。
    pub show_punct: bool,
    /// 生僻字显示：true 时给生僻字标注读音。
    pub show_rare: bool,
    /// 书库面板是否展开（书架上下载更多书）。
    pub show_library: bool,
    /// 窗口内容区尺寸（每帧由 render 写入，供铺满屏计算）。
    pub viewport_w: f32,
    pub viewport_h: f32,
    /// 单张页面的实际渲染宽度（由 fit_sizes 按最终字号算出）。
    /// 样式面板用它取宽，保证与单页纸面严格等宽。
    pub page_w: f32,
    /// 单张页面的实际渲染高度（网格引擎 outer_h；`leaf_origin` 定位注文用）。
    pub page_h: f32,
    /// 可见面上所有注文块的包围盒（每帧刷新，Canvas 命中测试用）。
    pub note_boxes: Vec<crate::gloss::NoteBox>,
    /// 点注文弹出的放大卡片（None 表示未弹出）。
    pub note_zoom: Option<crate::gloss::NoteZoom>,
    /// 现代横笺的行高（由 fit_sizes 按可用高度均分 12 行算出，用来纵向铺满）。
    pub line_modern: f32,
    pub notice: String,
    /// 开发用：一次性自检标记（见 render_shelf 的 JIANDU_RENAME_KEYS）。
    pub rename_test_done: bool,

    // ---- 全文检索 ----
    /// 检索面板是否展开。
    pub show_search: bool,
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

    /// 全书统计（章数, 字数）：开卷时算一次，供章末「篇·章·字」小表的
    /// 「全書」一行使用（逐帧扫全书太贵）。
    pub book_stat: (usize, usize),

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
}

impl Root {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let mut library = Library::load();
        if library.books.is_empty() {
            let _ = library.seed_sample();
        }
        ensure_assets();
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
            show_grid_panel: false,
            duplex: Duplex::Single,
            font_ancient: FontChoice::SongTi,
            font_modern: FontChoice::SourceHanSans,
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
            toc_slide: 0.0,
            toc_target: false,
            toc_animating: false,
            show_style: false,
            focus_mode: false,
            zoom: 1.0,
            lookup: None,
            notes: std::collections::HashMap::new(),
            show_notes_panel: false,
            note_editing: false,
            note_edit_note: None,
            note_draft: String::new(),
            note_cursor: 0,
            note_focus: _cx.focus_handle(),
            note_marked: None,
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
            share_layout: CardLayout::Note,
            share_bg: 0,
            rule_color: RuleColor::Wu,
            share_card_rect: None,
            share_shot_frames: 0,
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
            style_tab: StyleTab::Style,
            show_punct: true,
            show_rare: false,
            show_library: false,
            viewport_w: 1280.0,
            viewport_h: 860.0,
            page_w: 640.0,
            page_h: 480.0,
            note_boxes: Vec::new(),
            note_zoom: None,
            line_modern: 26.0,
            rename_test_done: false,
            show_search: false,
            search_draft: String::new(),
            search_cursor: 0,
            search_focus: _cx.focus_handle(),
            search_marked: None,
            search_hits: Vec::new(),
            search_idx: 0,
            search_keys: std::collections::HashSet::new(),
            search_cur: None,
            book_stat: (0, 0),
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
            notice: if count == 0 {
                "书架空空，点「导入书籍」放一本进来".into()
            } else {
                format!("书架共 {count} 本")
            },
        };

        // 开发用：JIANDU_AUTO_READ=1 直接进入正文，方便截图核对版式
        // （JIANDU_BOOK_IDX 可指定打开书架第几本）。
        // 开发用：JIANDU_SHELF=list 以书脊列表开局、JIANDU_SHELF_Q=词 预填搜索、
        // JIANDU_SHELF_CAT=史 预选分类，便于逐态截图核对书架。
        if let Ok(v) = std::env::var("JIANDU_SHELF") {
            if v == "list" {
                root.shelf_view = ShelfView::Spine;
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
        if std::env::var("JIANDU_SHARE_OPEN").is_ok() {
            root.show_share_panel = true;
        }
        // 开发用：指定分享卡版式 / 底色，便于逐项核对。
        if let Ok(v) = std::env::var("JIANDU_SHARE_LAYOUT") {
            root.share_layout = match v.as_str() {
                "single" => CardLayout::Single,
                "note" => CardLayout::Note,
                _ => CardLayout::Columns,
            };
        }
        if let Ok(v) = std::env::var("JIANDU_SHARE_BG") {
            root.share_bg = v.parse().unwrap_or(0);
        }
        // 开发用：指定丝栏色（0 乌 / 1 金 / 2 朱），便于逐色截图核对。
        if let Ok(v) = std::env::var("JIANDU_RULE") {
            if let Ok(i) = v.parse::<usize>() {
                if let Some(rc) = crate::types::RuleColor::ALL.get(i) {
                    root.rule_color = *rc;
                }
            }
        }
        // 开发用：展开样式面板 / 切到字体页。
        if std::env::var("JIANDU_STYLE_OPEN").is_ok() {
            root.show_style = true;
        }
        // 开发用：直接展开版式面板（列数 / 留白），便于截图核对。
        if std::env::var("JIANDU_GRID_OPEN").is_ok() {
            root.show_grid_panel = true;
        }
        // 开发用：直接进入专注模式。
        if std::env::var("JIANDU_FOCUS").is_ok() {
            root.focus_mode = true;
        }
        // 开发用：直接切双面对开（古文竖笺也要能对开）。
        if std::env::var("JIANDU_FACING").is_ok() {
            root.duplex = Duplex::Facing;
        }
        if std::env::var("JIANDU_FONT_TAB").is_ok() {
            root.style_tab = StyleTab::Font;
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

    pub fn set_font(&mut self, c: FontChoice) {
        match self.script {
            Script::Ancient => self.font_ancient = c,
            Script::Modern => self.font_modern = c,
        }
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
    /// 少算任何一项，纸面就会超出视口被裁掉边框（双面对开尤其明显：
    /// 两叶各超宽一截，右叶的版框直接飞出屏幕）。
    pub fn fit_sizes(&mut self) {
        // 内容区：视口减去工具栏(≈44)与状态栏(≈34)及四边留白(≈44)。
        // 专注模式：工具栏 / 状态栏隐藏，且去掉四边留白——单面、双面
        // （对开两叶）都真正撑满整屏，如官网宣传图般只见纸面不见界面。
        let (chrome_v, pad) = if self.focus_mode {
            (0.0, 0.0)
        } else {
            (44.0 + 34.0, 44.0)
        };
        // 样式面板为浮层（可盖住正文下部），不为它预留高度，正文始终铺满。
        let content_h = (self.viewport_h - chrome_v - pad).max(220.0);
        // 目录改为浮层，不占水平空间。
        let content_w = (self.viewport_w - pad).max(220.0);
        self.content_w = content_w;
        self.content_h = content_h;
        let notes_foot = self.show_notes && !self.notes_inline;
        // 双面对开 / 单页由屏幕方向驱动：横屏（宽 > 高，平板 / iPad 横持）
        // 走双面对开；竖屏走单页。两者都渲染古籍竖笺。
        let facing = self.viewport_w > self.viewport_h;
        self.duplex = if facing { Duplex::Facing } else { Duplex::Single };
        if std::env::var("JIANDU_FIT_DEBUG").is_ok() {
            eprintln!("[fit] vw={:.0} vh={:.0} content={:.0}x{:.0} cols={} facing={} focus={}", self.viewport_w, self.viewport_h, content_w, content_h, self.grid.columns, facing, self.focus_mode);
        }
        // 古籍竖笺几何预留：版框双边 50、无天头（篇题进版框）、地脚 12 +
        // 页脚注区（脚注模式且开注释时）。列宽 / 字号 / 行高 / 总行数由网格
        // 引擎按列数动态算出（8 / 10 / 12 / 14 列可切）。
        // 章题列不占额外宽度：它是「列数」里的最后一列（章首页少排一列正文
        // 给章题），整面总宽恒为 列数×col_w，因此 extra = 0。
        let edge = ANCIENT_EDGE;
        let head = ANCIENT_HEAD;
        let foot = ANCIENT_FOOT + if notes_foot { ANCIENT_NOTES } else { 0.0 };
        let extra = 0.0;
        let m = compute_grid(
            content_w,
            content_h,
            self.grid,
            facing,
            self.zoom,
            edge,
            head,
            foot,
            extra,
        );
        self.size_ancient = m.fs;
        self.size_modern = m.fs;
        self.line_modern = m.row_h;
        self.page_w = m.outer_w;
        self.page_h = m.outer_h;
        self.col_w = m.col_w;
        self.row_h = m.row_h;
        self.grid_rows = m.rows;
        self.grid_min_font = m.min_font_hit;
    }

    /// 设列数（8 / 10 / 12 / 14）；越界值忽略。改列数会重排，回到首页。
    pub fn set_columns(&mut self, n: usize) {
        if GRID_COLUMN_CHOICES.contains(&n) && self.grid.columns != n {
            self.grid.columns = n;
            self.page = 0;
            self.sync_current_chapter();
        }
    }

    /// 设四周留白比例（0..1，近似四边同留白）。
    pub fn set_margin(&mut self, m: f32) {
        self.grid.margin = m.clamp(0.0, 0.2);
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

    /// 打开书架上第 idx 本书：直接进正文上次读的那一面，并恢复进度与笔记。
    pub fn open_book(&mut self, idx: usize) {
        let Some(entry) = self.library.books.get(idx).cloned() else {
            return;
        };
        let nt = entry.notes.clone();
        let ul = entry.underlines.clone();
        match crate::importer::import_from_path(&entry.path) {
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
                self.book = Some(book);
                self.notes = nt;
                self.underlines = ul.into_iter().collect();
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
                // 想看书衣仍可从阅读页工具栏「封面」进。
                self.screen = Screen::Reading;
                self.notice.clear();
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

    /// 把文件导入书架。
    pub fn add_import(&mut self, path: PathBuf) {
        match self.library.import_path(&path) {
            Ok(i) => {
                let title = self
                    .library
                    .books
                    .get(i)
                    .map(|b| b.title.clone())
                    .unwrap_or_default();
                self.notice = format!("已上架：{title}");
            }
            Err(e) => {
                self.notice = format!("导入失败：{e}");
            }
        }
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
    pub fn import_dialog(&mut self, cx: &mut Context<Self>) {        cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // AsyncApp 是 Clone：把「借用的上下文」换成「owned 句柄」，
            // async move 块才能安全持有它跨 await（借用不能进 'static future）。
            let mut cx = cx.clone();
            async move {
                let picked = cx
                    .background_executor()
                    .spawn(async {
                        rfd::FileDialog::new()
                            .set_title("导入书籍到书架")
                            .add_filter(
                                "书籍 (md / txt / epub / json)",
                                &["md", "markdown", "txt", "epub", "json"],
                            )
                            .add_filter("全部文件", &["*"])
                            .pick_files()
                    })
                    .await;

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

    /// 某章的第一面页码；找不到就回 0。
    pub fn page_of_chapter(&self, chapter: usize) -> usize {
        let pages = self.paginate();
        pages
            .iter()
            .position(|p| p.first().map(|g| g.chapter >= chapter).unwrap_or(false))
            .unwrap_or(0)
    }

    /// 设字级档位（样式面板的「小 / 中 / 大 / 特大」）。
    ///
    /// `zoom` 存的是倍率，档位由 `ZoomLevel::from_zoom` 反推，
    /// 所以这里只需把倍率落到该档的 `factor()` 上，排版下一帧自动重算。
    pub fn set_zoom_level(&mut self, lvl: crate::types::ZoomLevel) {
        self.zoom = lvl.factor();
        self.notice = format!("字级：{}", lvl.label());
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
    pub fn open_toc(&mut self) {
        if self.show_toc && self.toc_target {
            return;
        }
        self.show_toc = true;
        self.toc_target = true;
        self.toc_animating = true;
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
        // 极端情况：侧栏可见但 slide 已为 0（不应发生），直接收。
        if self.toc_slide <= 0.0 {
            self.show_toc = false;
            self.toc_animating = false;
        }
    }

    /// 分页（缓存由每次渲染即时计算，文本量级足够）。
    pub fn page_cap(&self) -> usize {
        // 每面容量（粗略上限）：列数 × 行数。
        self.grid.columns * self.grid_rows
    }

    /// 取某个字上挂的注文（夹注只认 `Note` 一类，通假另走旁注）。
    pub fn note_chars(&self, g: Glyph) -> Option<&str> {
        if !self.show_notes || g.kind != GlyphKind::Char {
            return None;
        }
        let book = self.book.as_ref()?;
        let p = book.chapters.get(g.chapter)?.paragraphs.get(g.para)?;
        p.notes
            .iter()
            .find(|n| n.at == g.offset && n.kind == NoteKind::Note)
            .map(|n| n.text.as_str())
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

    /// 一列 `rows` 字 = `rows×2` 格、一面 `cols` 列；注文过长会跨列流转，
    /// 排版结果与阅读器里的列布局完全一致（两边同走 `gloss::layout_book`）。
    /// `cols` / `rows` 全部来自网格引擎——同一套分页逻辑适配任意列数。
    pub fn paginate(&self) -> Vec<Vec<Glyph>> {
        self.layout_pages()
            .into_iter()
            .map(|p| p.glyphs)
            .collect()
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
        crate::gloss::layout_book(
            &flow,
            |g| {
                if self.notes_inline {
                    self.note_chars(*g).map(|s| s.to_string())
                } else {
                    None
                }
            },
            cols,
            rows,
            reserve,
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
    fn chapter_inscription_need(&self) -> usize {
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
    pub fn refresh_note_boxes(&mut self, pages: &[crate::gloss::Page]) {
        self.note_boxes.clear();
        if !self.notes_inline || !self.show_notes {
            return;
        }
        let mut leaves: Vec<(usize, bool)> = Vec::new();
        if self.duplex == Duplex::Facing {
            if self.page + 1 < pages.len() {
                leaves.push((self.page + 1, true));
            }
            if self.page < pages.len() {
                leaves.push((self.page, false));
            }
        } else if self.page < pages.len() {
            leaves.push((self.page, false));
        }
        for (idx, back) in leaves {
            let Some(pg) = pages.get(idx) else { continue };
            let tc = if pg.start_chapter {
                self.grid.columns.saturating_sub(1)
            } else {
                self.grid.columns
            };
            for (i, col) in pg.columns.iter().enumerate() {
                // 竖排：读序列号 i=0 在最右；flex_row 从左数的位置 li = tc-1-i。
                let li = tc.saturating_sub(1).saturating_sub(i);
                for mut b in
                    crate::gloss::note_boxes_in_column(col, li, self.col_w, self.row_h)
                {
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
        self.note_suppress_commit = true; // 忽略「打开这一下」点击的自动提交
        self.popup = None;
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
        self.note_suppress_commit = true;
        self.popup = None;
        self.note_zoom = None;
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
            if let Some(path) = self.library.books.get(self.current).map(|e| e.path.clone()) {
                let _ = crate::importer::write_book_json(self.book.as_ref().unwrap(), &path);
            }
        }
    }

    /// 保存草稿并退出编辑态。
    pub fn commit_note_edit(&mut self) {
        let text = std::mem::take(&mut self.note_draft);
        let target = self.note_edit_note.take();
        self.note_editing = false;
        self.note_suppress_commit = false;
        self.note_cursor = 0;
        self.note_pos = None;
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
                if self.notes.get(&self.page).is_some() {
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

    /// 编辑栏粘贴：读系统剪贴板（`pbpaste`）插入到光标处——中文笔记的录入方式。
    pub fn note_paste(&mut self) {
        if let Ok(o) = std::process::Command::new("pbpaste").output() {
            let s = String::from_utf8_lossy(&o.stdout).replace(['\n', '\r'], " ");
            draft_insert_str(&mut self.note_draft, &mut self.note_cursor, &s);
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
        self.dragging = false;
        self.touched_glyph = false;
        self.sel_char = None;
    }

    /// 顶部品牌条（三态共用）。
    pub fn render_brand(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .text_size(px(16.0))
                    .text_color(rgb(CINNABAR))
                    .child(SharedString::from("笺读")),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0x8A8175))
                    .child(SharedString::from("JianDu · 为阅读创造环境")),
            )
    }
}

// ---- 行内草稿编辑（浮层笔记框 / 重命名框共用：GPUI 无文本输入元素，
//      以「草稿串 + 字符光标」自绘，键入走 keystroke.key_char，⌘V 走 pbpaste）----
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

/// 古文竖笺纸面 chrome（横 / 纵共用）：sheet p_4(16×2=32) + 版框双边
/// （外 3×2 + 中缝 3×2 + 内 1×2 = 14）+ 余量 ≈ 50。
///
/// 版框内不再留横白：界行（乌丝栏）上下直接顶到内框，所以这个值比旧版
/// （112，按「天头一行 + 版框三层内边距」估）小得多——竖笺才真正铺满视口，
/// 与影印本「框内即是行格」的观感一致。
pub const ANCIENT_EDGE: f32 = 50.0;
/// 古文竖笺天头：篇题与书名已移进版框（章题列 + 地脚），天头不再单占一行。
pub const ANCIENT_HEAD: f32 = 0.0;
/// 古文竖笺地脚：鱼尾 + 葉码 一行（小字按 0.36×字号伸缩，这里只留固定部分）。
pub const ANCIENT_FOOT: f32 = 12.0;
/// 古文竖笺页脚注区预留（脚注模式且开启注释时）。
pub const ANCIENT_NOTES: f32 = 76.0;
/// 章题列宽度系数：章题列就是「列数」里的格内一列，与正文列同宽。
/// 章首页 = (列数-1) 个正文列 + 1 个章题列，整面总宽恒为 列数×col_w。
pub const ANCIENT_TITLE_COL: f32 = 1.0;

// 主题色
pub const PAPER: u32 = 0xF7F1E3;
pub const PAPER_BACK: u32 = 0xEAE0C9;
pub const INK: u32 = 0x2B2622;
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
    // 版心鱼尾：完整照搬用户参考图「横版」中缝里那两枚——**横长黑矩形**
    // （骑跨版心、上下各一枚：上鱼尾在篇题之上，下鱼尾在叶码之下），
    // 上边平直满宽、下缘两角微收成浅 V；下鱼尾上下翻转同形。
    // viewBox 24×9（宽高比约 33:12，与参考图一致）。GPUI 只取 SVG 的
    // alpha 通道再用元素 text_color 上色，故 fill 必须为不透明实色。
    // 见 reader::banxin_tail。
    let bx_top = dir.join("banxin_top.svg");
    let bx_bot = dir.join("banxin_bottom.svg");
    let bx_top_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 9\"><path d=\"M0,0 L24,0 L24,6.4 L13.5,9 L10.5,9 L0,6.4 Z\" fill=\"#1A150F\"/></svg>";
    let bx_bot_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 9\"><path d=\"M0,9 L24,9 L24,2.6 L13.5,0 L10.5,0 L0,2.6 Z\" fill=\"#1A150F\"/></svg>";
    let _ = std::fs::write(&bx_top, bx_top_svg);
    let _ = std::fs::write(&bx_bot, bx_bot_svg);
    let bot_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 13\"><path d=\"M0,13 L11,13 L12,0 Z M24,13 L13,13 L12,0 Z\" fill=\"#1A150F\"/></svg>";
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
        let screen = self.render_screen(cx);
        // 开发用：JIANDU_SAVE_SHARE=1 时，分享卡渲染到第 3 帧后自动存一张，
        // 便于核对裁剪区域（前两帧还没上屏，截不到）。
        if std::env::var("JIANDU_SAVE_SHARE").is_ok()
            && self.show_share_panel
            && self.share_shot_frames < 3
        {
            self.share_shot_frames += 1;
            if self.share_shot_frames == 3 {
                let p = self.capture_card_png(window, "jiandu-card-test");
                self.notice = format!("调试存卡：{p:?}");
            }
        }
        let key_focus = self.key_focus.clone();
        div()
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
            })
            .child(screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            margin: 0.04,
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
            0.0,
        )
    }

    #[test]
    fn ancient_focus_mode_fills_screen() {
        // 专注单面：竖笺横向、纵向一起撑满（focus: 无 chrome/pad → content=VW×VH）。
        let cfg = GridConfig {
            columns: 8,
            dir: GridDir::Vertical,
            margin: 0.04,
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
            0.0,
        );
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
            margin: 0.04,
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
        );
        assert!(2.0 * m.outer_w + gap <= VW + 0.5);
        assert!(2.0 * m.outer_w + gap > VW * 0.95);
        assert!(m.outer_h <= VH + 0.5, "对开纸高 {:.1} 超 {VH}", m.outer_h);
        // 中缝宽约等于一行字宽（用户参考图「古风3」的要求）。
        assert!(
            (gap - m.col_w).abs() <= m.col_w * 0.12,
            "中缝宽 {gap:.1} 应与一行字宽 {:.1} 相当",
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
            margin: 0.04,
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
            0.0,
        );
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
                // 章首页的第一个字必须是该章的段首缩进占位。
                let first = pg.glyphs[0];
                assert!(first.is_indent() && first.offset == 0);
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
            |g| (g.kind == GlyphKind::Char && g.offset % 2 == 0).then(|| "夹注四字".to_string()),
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
                crate::gloss::Piece::Gloss(s) => Some(s.as_str()),
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
                        (g.offset % 7 == 0).then(|| "注".to_string())
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
        for cols in GRID_COLUMN_CHOICES {
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
}
