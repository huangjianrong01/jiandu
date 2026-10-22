//! 参数化网格排版引擎：竖排 / 横排共用同一套「格子坐标 (x, y)」与读序逻辑。
//!
//! 设计目标（见用户需求）：
//!   1. 配置对象 [`GridConfig`]：列数 `columns`、方向 `dir`、四周留白比例 `margin`。
//!   2. 动态网格计算 [`compute_grid`]：不写死像素——
//!        colWidth = 可用宽 / 列数
//!        fontSize = 0.75 × colWidth
//!        rowHeight = 1.1 × colWidth
//!        totalRows = floor(可用高 / rowHeight)
//!   3. 一套引擎兼容横竖排：方向只改变「读序」与「坐标映射」，格子几何完全一致。
//!        · 竖排：遍历从右向左（列），从上向下（行）；
//!        · 横排：遍历从上向下（行），从左向右（列）；
//!        · 共用 [`cell_origin`] 计算 (x, y)、[`reading_order`] 计算读序。
//!   4. 边界处理：最小字号 [`GRID_FS_MIN`]（窄屏 12 列也不至于小到看不清，
//!      触发时 `min_font_hit` 为真，由渲染层开启单页滚动）。

use serde::{Deserialize, Serialize};

use crate::types::Script;

/// 双面对开两叶之间的「版心（中缝）」宽：**约等于一行字宽**。
/// 刻本版心本就是一格字的宽度——鱼尾、篇题、叶码、朱砂小印都立在这一格
/// 里。这里按 `compute_grid` 的同一套算式反解：令中缝宽恰等于单叶字格宽
/// （`(content_w×k/2 - edge) / (columns + k/2)`，`k = 1 - 2×margin`），
/// 两叶各占 `columns` 格、缝里再立一格；列数越多缝越窄，始终跟随
/// 「一行字宽」进退。
pub fn gutter_w(content_w: f32, columns: usize, edge: f32, margin: f32) -> f32 {
    let c = columns.max(1) as f32;
    let k = (1.0 - 2.0 * margin.clamp(0.0, 0.45)).max(0.1);
    let num = (content_w * k / 2.0 - edge).max(1.0);
    (num / (c + k / 2.0)).clamp(LEAF_GAP_MIN, LEAF_GAP_MAX)
}

/// 中缝宽下限（窄屏也立得住一对鱼尾）/ 上限（14 列时不至于宽得离谱）。
pub const LEAF_GAP_MIN: f32 = 22.0;
pub const LEAF_GAP_MAX: f32 = 110.0;

// ── 响应式单 / 双叶 ─────────────────────────────────────────────────
/// 双叶对开所需的最小宽高比：低于此值按「竖屏 / 近方屏」处理 → 单叶。
pub const FACING_MIN_ASPECT: f32 = 1.15;
/// 双叶时单叶的最小可读宽：再窄两叶就成两条细缝，不如单叶。
pub const FACING_MIN_LEAF_W: f32 = 300.0;
/// 双叶时单叶字号的可读下限：挤到这个字号以下宁可退回单叶——
/// 「保证字号足够大」优先于「铺两叶」。
pub const FACING_MIN_FS: f32 = 14.0;
/// 响应式估算用的四边留白与两叶间中缝（与渲染同源的量级）。
pub const RESP_MARGIN: f32 = 16.0;
pub const RESP_GUTTER: f32 = 24.0;

/// 是否窄屏（竖屏 / 近方屏）：单叶模式，版心贴屏幕右边缘。
pub fn is_narrow(viewport_w: f32, viewport_h: f32) -> bool {
    if viewport_w <= 1.0 || viewport_h <= 1.0 {
        return true;
    }
    viewport_w / viewport_h < FACING_MIN_ASPECT
}

/// 响应式单 / 双叶判断。
///
/// * 宽屏（平板横持 / 电脑）：**双叶对开**，两叶等宽、中缝落在屏幕正中；
/// * 窄屏（手机竖持）：**单叶**，只排当前叶，纸面贴右边缘，
///   整幅宽都给这一叶 —— 字号自然翻倍，不挤占阅读空间。
///
/// 判据不只看宽高比：双叶还要保证单叶宽与字号都过得去，
/// 否则（超多列 + 小字级）宽屏也退回单叶。
pub fn should_face(viewport_w: f32, viewport_h: f32, columns: usize, zoom: f32) -> bool {
    if is_narrow(viewport_w, viewport_h) {
        return false;
    }
    let content_w = (viewport_w - 2.0 * RESP_MARGIN).max(1.0);
    let per_leaf = (content_w - RESP_GUTTER) / 2.0;
    if per_leaf < FACING_MIN_LEAF_W {
        return false;
    }
    let fs = (per_leaf / columns.max(1) as f32) * GRID_FS_RATIO * zoom;
    fs >= FACING_MIN_FS
}

/// 排版方向。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum GridDir {
    /// 竖排：从右向左分列，每列自上而下（古籍式）。
    #[default]
    Vertical,
    /// 横排：从上向下分行，每行自左向右（现代信笺式）。
    Horizontal,
}

impl GridDir {
    pub fn label(self) -> &'static str {
        match self {
            GridDir::Vertical => "竖排",
            GridDir::Horizontal => "横排",
        }
    }

    /// 方向 → 字体族（竖排用楷/宋，横排用黑体）。
    pub fn script(self) -> Script {
        match self {
            GridDir::Vertical => Script::Ancient,
            GridDir::Horizontal => Script::Modern,
        }
    }

    /// 该方向下的「主轴线」分隔线：竖排画竖线（乌丝栏），横排画横线（横丝栏）。
    pub fn is_vertical(self) -> bool {
        matches!(self, GridDir::Vertical)
    }
}

/// 列数档位：古籍新增 14 列，连同原有的 8 / 10 / 12。
/// （保留以供分页一致性测试遍历；UI 与状态切换已改走 [`LAYOUT_CONFIGS`]。）
pub const GRID_COLUMN_CHOICES: [usize; 4] = [8, 10, 12, 14];

/// 版式配置：数据驱动排版引擎的**唯一数据源**。
///
/// 取代原先散落的 `letter_mode` 布尔、`GRID_COLUMN_CHOICES` 硬编码与
/// 「信笺（14列）」特例——所有版式（标准 8 / 10 / 12 列、信笺 14 列）都只是
/// 这张表里的一条记录。列数 / 边框样式 / 鱼尾开关 / 方向 / 留白全部声明于此，
/// 渲染与几何只消费配置，不再写 `if cfg.columns == 14` 这类判定。
#[derive(Clone, Copy, Debug)]
pub struct LayoutConfig {
    /// 配置键（数据驱动切换的句柄，见 [`LAYOUT_CONFIGS`]）。
    pub key: &'static str,
    /// 按钮 / 菜单上显示的标签（UI 直接读它，不写死文案）。
    pub label: &'static str,
    /// 每面列数（竖排=竖列数）：8 / 10 / 12 / 14。
    pub columns: usize,
    /// 排版方向（沿用既有方向枚举）。
    pub dir: GridDir,
    /// 四周留白比例。
    pub margin: f32,
    /// 是否在版心（中缝）绘制上下相对的「对鱼尾」。
    pub show_fish_tail: bool,
}

/// 版式配置注册表：有序数组即「单一数据源」。新增版式（如 16 列）只需在此
/// 追加一行，UI / 渲染 / 几何自动适配——零 if-else 扩散。
///
/// v1.24.0 起四档列数（8 / 10 / 12 / 14）走**同一条刻本几何**：原先的
/// 「信笺 14 列」独占一条 2:3 平铺稿纸路径（`fit_letter`），已被移除——
/// 那套模板只有 14 列能用，且与列数选择器冲突（选 14 得到的是信笺而不是
/// 标准刻本）。版框统一为「外粗内细」的文武边栏（见 `render_ancient`）。
pub const LAYOUT_CONFIGS: &[LayoutConfig] = &[
    LayoutConfig { key: "standard8",  label: "标准 8 列",  columns: 8,  dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false },
    LayoutConfig { key: "standard10", label: "标准 10 列", columns: 10, dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false },
    LayoutConfig { key: "standard12", label: "标准 12 列", columns: 12, dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false },
    LayoutConfig { key: "standard14", label: "标准 14 列", columns: 14, dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false },
];

impl LayoutConfig {
    /// 按配置键取配置；未知键回落默认。
    pub fn by_key(key: &str) -> Option<&'static LayoutConfig> {
        LAYOUT_CONFIGS.iter().find(|c| c.key == key)
    }
    /// 默认版式（首启 / 配置损坏时的回落）。
    ///
    /// v1.28.0 起为 **12 列**：10 列在常见的 1280–1600 宽窗口里字大行稀，
    /// 摊开像「大字盲人版」；12 列一行多两字，纸面立刻有了刻本该有的密度。
    pub fn default() -> &'static LayoutConfig {
        Self::by_key("standard12").unwrap_or(&LAYOUT_CONFIGS[2])
    }
    /// 默认配置键。
    pub fn default_key() -> &'static str {
        Self::default().key
    }
    /// 旧存档里可能还写着已删除的「信笺 14 列」键：一律回落到同列数的标准版。
    /// （列数不变、几何不变，用户察觉不到，但不会再走那条已删除的模板路径。）
    pub fn migrate_key(key: &str) -> &'static str {
        match LayoutConfig::by_key(key) {
            Some(c) => c.key,
            None if key == "letter14" => "standard14",
            None => Self::default_key(),
        }
    }
}

/// 网格配置：参数化排版引擎的唯一输入。
#[derive(Clone, Copy, Debug)]
pub struct GridConfig {
    /// 列数（竖排=竖列数；横排=每行字数）：8 / 10 / 12 / 14。
    pub columns: usize,
    /// 排版方向。
    pub dir: GridDir,
    /// 四周留白比例（相对内容区，0..1）。
    pub margin: f32,
    /// **行高缩放**（`lineHeightScale`）：行高 = 列宽 × `GRID_ROW_RATIO` × 它。
    ///
    /// 不同字体的 em box 大小不同——思源系列字面偏大，同样字号下竖排
    /// 上下两字容易贴住。按当前正文字体给一点余量（宋体 1.05、楷体 1.12），
    /// 字形才紧凑而不重叠。见 `fonts::line_height_scale`。
    pub line_scale: f32,
}

impl Default for GridConfig {
    fn default() -> Self {
        GridConfig {
            // 与 `LayoutConfig::default()` 同为 12 列：两处默认必须一致，
            // 否则「首启版式」与「首启列数」会打架（一个说 12、一个排 8）。
            columns: 12,
            dir: GridDir::Vertical,
            margin: 0.04, line_scale: 1.0,
        }
    }
}

/// 字号 / 行高算式系数与上下限（均来自用户给定的公式与边界要求）。
pub const GRID_FS_RATIO: f32 = 0.75; // fontSize = colWidth × 0.75
/// 行高 / 列宽：v1.28.0 由 1.1 收到 **1.0**。
///
/// 竖排的字身本身就是列宽（字号 = 0.75 × 列宽），行距留到 1.1 时两字之间
/// 有近三成空隙，整页看着「稀」；收到 1.0 后字与字之间只剩字面本身的
/// 一点余白（约 5%），行气才接得上刻本。
pub const GRID_ROW_RATIO: f32 = 1.0; // rowHeight = colWidth × 1.0
/// 最小字号保护阈值（用户指定 12px）：窄屏 12/14 列也不至于小到看不清；
/// 触发后几何按「字号优先」重算，可能超出可用宽 → 由渲染层开启单页
/// 上下（及左右）滚动，绝不把字挤成一团黑疙瘩。
pub const GRID_FS_MIN: f32 = 12.0;
/// 字号上限（px）：**32**（v1.28.0 用户指定 `MaxFontSize = 32px`）。
///
/// 旧上限 220 只是「防无限制放大」，实际等于不设限——大屏一对开就能排出
/// 40 多磅的正文，一列没几个字，整页空旷得像童书大字本。收到 32 后：
/// 触顶时**列宽跟着回缩**（见 `compute_grid` 第 5 步），字仍贴着格，
/// 密度不会被大屏稀释。
pub const GRID_FS_MAX: f32 = 32.0;
/// 行高缩放的中性值：不放大也不压缩（旧存档 / 未指定字体时取它）。
pub const LINE_SCALE_BASE: f32 = 1.0;
/// 行高缩放的合理区间（夹住异常值，免得行气散掉或字压成一团）。
pub const LINE_SCALE_MIN: f32 = 0.85;
pub const LINE_SCALE_MAX: f32 = 1.6;

// ── 自动列数（v1.29.0）：按「书的内容」选档，而不是一刀切一个默认值 ────
/// 短章 / 次短章的分界（平均一章字数）：低于此按「语录、短札」处理。
pub const AUTO_SHORT_CHARS: usize = 150;
/// 次短章 / 常篇的分界。
pub const AUTO_MID_CHARS: usize = 500;
/// 常篇 / 长篇的分界：高于此按「长篇章回」处理。
pub const AUTO_LONG_CHARS: usize = 1200;

/// **内容定档**：平均一章多少字 → 列数。**与屏幕无关**。
///
/// 刻本的实际做法就是这样——短章语录（如《道德经》）字大行疏、开本窄；
/// 长篇话本字小行密、开本宽。篇幅决定疏密，而不是所有书都排一个样。
///
/// 四档正好落在 [`GRID_COLUMN_CHOICES`] 上：8 / 10 / 12 / 14。
pub fn content_columns(avg_chapter_chars: usize) -> usize {
    if avg_chapter_chars < AUTO_SHORT_CHARS {
        8
    } else if avg_chapter_chars < AUTO_MID_CHARS {
        10
    } else if avg_chapter_chars < AUTO_LONG_CHARS {
        12
    } else {
        14
    }
}

/// **自动档：选列数**。纯函数，不碰 GPUI，可单测。
///
/// 两条规则，主次分明：
///   1. **内容定档**（[`content_columns`]）——篇幅是主因，屏幕管不着。
///   2. **屏幕兜底**：只有当该档的字号跌破可读下限 [`GRID_FS_MIN`] 时，
///      才逐级往少列退（列少 → 列宽大 → 字大）。
///
/// 关键：`fs_of` 只在第 2 条里当**兜底判据**用，不参与选档。
/// 早期版本拿「字号接近某个目标值」打分，结果在宽屏上 8/10/12 列的字号
/// 全被 `GRID_FS_MAX = 32` 压成同一个数，判据失去区分度，任何书都只能
/// 选最密档——自适应名存实亡。**字号触顶不是「该加列」的信号**：触顶时
/// `compute_grid` 会让列宽回缩、纸面变窄居中（见其第 5 步），那正是
/// 「疏朗而不空旷」，是短章该有的样子。
///
/// `fs_of` 求「排成 c 列时正文的实际字号」，是**唯一的口径来源**：
/// 运行时传真实的 `calculate_grid` 试算（含字号上下限与列宽回缩），
/// 单测里传一句简算即可——两边共用同一套判据，不会各写一套而漂移。
pub fn auto_columns(fs_of: impl Fn(usize) -> f32, avg_chapter_chars: usize) -> usize {
    let mut cols = content_columns(avg_chapter_chars);
    // 屏幕兜底：字太小就读不动了，逐级往疏处退。
    // 触顶（字过大）不在这里处理——那是列宽回缩的事，不是加列的理由。
    while cols > GRID_COLUMN_CHOICES[0] && fs_of(cols) < GRID_FS_MIN - 0.01 {
        let prev = GRID_COLUMN_CHOICES
            .iter()
            .rev()
            .find(|&&c| c < cols)
            .copied()
            .unwrap_or(GRID_COLUMN_CHOICES[0]);
        if prev == cols {
            break;
        }
        cols = prev;
    }
    cols
}

/// 网格度量：由 [`compute_grid`] 算出，渲染与分页共用，不再写死任何像素。
#[derive(Clone, Copy, Debug)]
pub struct GridMetrics {
    pub dir: GridDir,
    pub columns: usize,
    pub rows: usize,
    /// 字格宽（竖排=列宽；横排=单字格宽）。
    pub col_w: f32,
    /// 字格高（竖排=单字竖高；横排=行高）。
    pub row_h: f32,
    /// 字号。
    pub fs: f32,
    /// 纯字格区宽（不含版框 / 留白）。
    pub grid_w: f32,
    /// 纯字格区高。
    pub grid_h: f32,
    /// 含版框 + 留白后的整张纸面宽。
    pub outer_w: f32,
    /// 含版框 + 留白后的整张纸面高。
    pub outer_h: f32,
    /// 是否触发了最小字号（需要单页滚动）。
    pub min_font_hit: bool,
}

/// 计算网格。
///
/// * `content_w` / `content_h`：内容区（已扣工具栏 / 状态栏 / 四边留白）。
/// * `cfg`：列数 / 方向 / 留白。
/// * `facing`：是否双面对开（对开时单叶可用宽对半）。
/// * `edge`：该方向版框 + 内边距的「双边」总预留（竖排≈`ANCIENT_EDGE`，横排≈`MODERN_EDGE`）。
/// * `head` / `foot`：天头 / 地脚预留（竖排地脚有鱼尾叶码，横排有天头标题）。
/// * `extra_cols`：`columns` **之外**的附加列宽（以 col_w 为单位计）。
///   古籍竖笺的章题列是格内一列（章首页少排一列正文给它），已在
///   `columns` 内，故传 0；仅有「格外再加竖条」这类需求时才传正值。
/// * `fs_max`：本次排版的字号上限（px）。常规阅读传 [`GRID_FS_MAX`]；
///   专注模式这类「我要整个屏幕都用来承载更大的字」的场合传
///   `f32::INFINITY`，否则大屏上会被 32 限成小字。
pub fn compute_grid(
    content_w: f32,
    content_h: f32,
    cfg: GridConfig,
    facing: bool,
    zoom: f32,
    edge: f32,
    head: f32,
    foot: f32,
    extra_cols: f32,
    fs_max: f32,
) -> GridMetrics {
    // 1) 双面对开：单叶可用宽对半——先扣两叶之间的版心（中缝），
    //    中缝宽约等于一行字宽（见 `gutter_w`）。
    let per_leaf = if facing {
        ((content_w - gutter_w(content_w, cfg.columns, edge, cfg.margin)) / 2.0).max(60.0)
    } else {
        content_w
    };
    // 2) 四周留白（相对内容区比例），上下 / 左右同比例。
    let mw = (per_leaf * cfg.margin).max(0.0);
    let mh = (content_h * cfg.margin).max(0.0);
    // 3) 字格可用区（再扣版框 + 留白）。
    let inner_w = (per_leaf - edge - 2.0 * mw).max(40.0);
    let inner_h = (content_h - head - foot - 2.0 * mh).max(40.0);

    // 4) 用户公式：列宽 = 可用宽 / 列数。
    let col_w_raw = inner_w / cfg.columns as f32;
    // 5) 字号 = 0.75 × 列宽；上下限双向兜底。
    //
    //    触**下限**（窄屏 14 列）：字号优先，反推列宽放大（宁可溢出滚动）。
    //    触**上限**（大屏对开）：列宽随之回缩到「字刚好贴格」，字与格的
    //    0.75 比例得以保住——只压字号不回缩列宽的话，字会缩在格心，纸面
    //    反而更空（这正是「大字本」的反面）。但最多只缩到原列宽的 70%：
    //    再窄就成了一条纸带，两侧空白比书还宽。
    let cap = fs_max.max(GRID_FS_MIN);
    let want = col_w_raw * GRID_FS_RATIO * zoom;
    let min_hit = want < GRID_FS_MIN;
    let capped = want > cap;
    let fs = want.clamp(GRID_FS_MIN, cap);
    let col_w = if min_hit {
        fs / GRID_FS_RATIO
    } else if capped {
        (fs / GRID_FS_RATIO).max(col_w_raw * 0.70)
    } else {
        col_w_raw
    };
    // 6) 行高 = 1.1 × 列宽 × 行高缩放（与字号比例固定，天然保证
    //    rowHeight > fontSize，竖排不上下重叠）。缩放由当前正文字体定
    //    （思源系字面大 → 松开一点余量），见 `fonts::line_height_scale`。
    let scale = if cfg.line_scale > 0.0 {
        cfg.line_scale.clamp(LINE_SCALE_MIN, LINE_SCALE_MAX)
    } else {
        LINE_SCALE_BASE
    };
    let row_h = col_w * GRID_ROW_RATIO * scale;
    // 7) 总行数 = floor(可用高 / 行高)。
    let rows = ((inner_h / row_h).floor()).max(1.0) as usize;

    let grid_w = (cfg.columns as f32 + extra_cols) * col_w;
    let grid_h = rows as f32 * row_h;
    let outer_w = grid_w + edge + 2.0 * mw;
    let outer_h = grid_h + head + foot + 2.0 * mh;

    GridMetrics {
        dir: cfg.dir,
        columns: cfg.columns,
        rows,
        col_w,
        row_h,
        fs,
        grid_w,
        grid_h,
        outer_w,
        outer_h,
        min_font_hit: min_hit,
    }
}

// ── 三种渲染模式（v1.23.0：全平台响应式多版式引擎）──────────────────
/// 渲染模式：同一套网格引擎，三种铺法。
///
/// 模式只决定「内容区怎么给」与「铺几叶」，**几何算式完全一致**
/// （col_w = 可用宽 / 列数、fs = 0.75 × col_w、row_h = 1.1 × col_w）——
/// 差别全在 [`calculate_grid`] 里那一次分派，渲染层不再写模式判定。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum LayoutMode {
    /// **对开翻阅（Spread）**：左右两半叶 + 中缝（版心含鱼尾 / 叶码），
    /// 视线自右向左扫视。适合电脑端与平板横屏。
    #[default]
    Facing,
    /// **手机竖屏（Mobile Portrait）**：屏幕撑不住双叶，强制单叶；
    /// 版心贴屏幕右缘，正文网格尽量放大，保证阅读不费眼。
    Single,
    /// **标准刻本整页（Full Page）**：把一整叶（两叶 + 中缝）当作一个整体
    /// 等比缩放塞进屏幕。内部网格比例不变，只对外层做 Scale。
    Full,
}

impl LayoutMode {
    pub const ALL: [LayoutMode; 3] = [
        LayoutMode::Facing,
        LayoutMode::Single,
        LayoutMode::Full,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayoutMode::Facing => "对开",
            LayoutMode::Single => "单叶",
            LayoutMode::Full => "整页",
        }
    }

    /// 面板上的注释（一句话说清适合什么场景）。
    pub fn hint(self) -> &'static str {
        match self {
            LayoutMode::Facing => "两半叶对开，中缝居中",
            LayoutMode::Single => "只排一叶，字更大",
            LayoutMode::Full => "整叶等比缩放，看版式",
        }
    }

    pub fn index(self) -> usize {
        match self {
            LayoutMode::Facing => 0,
            LayoutMode::Single => 1,
            LayoutMode::Full => 2,
        }
    }

    pub fn from_index(i: usize) -> LayoutMode {
        Self::ALL.get(i).copied().unwrap_or(LayoutMode::Facing)
    }

    /// 是否按「两叶 + 中缝」铺排：整页模式铺的也是**一整叶**（两叶 + 缝），
    /// 故与对开同路；只有手机竖屏是单叶。
    pub fn spreads(self) -> bool {
        matches!(self, LayoutMode::Facing | LayoutMode::Full)
    }
}

/// 整页模式的**基准版式尺寸**（一整叶 = 两叶 + 中缝），比例取刻本开本
/// （≈1.44:1）。几何恒定按这个基准算，再由 [`full_scale`] 等比缩放——
/// 内部网格比例永不改变，变的只是外层 Scale。
pub const FULL_BASE_W: f32 = 1180.0;
pub const FULL_BASE_H: f32 = 820.0;
/// 缩放上下限：太小字不可读，太大一叶撑爆屏幕（专注模式可放大看版式）。
pub const FULL_SCALE_MIN: f32 = 0.25;
pub const FULL_SCALE_MAX: f32 = 3.0;

/// 整页缩放系数：取「宽能塞下」与「高能塞下」里更小的那个——
/// 与 CSS `object-fit: contain` 同一套思路，保证整叶完整可见。
pub fn full_scale(content_w: f32, content_h: f32, zoom: f32) -> f32 {
    let k = (content_w / FULL_BASE_W).min(content_h / FULL_BASE_H);
    (k * zoom.max(0.1)).clamp(FULL_SCALE_MIN, FULL_SCALE_MAX)
}

/// [`calculate_grid`] 的输入：模式 + 列数 + 视口 + 预留。
#[derive(Clone, Copy, Debug)]
pub struct GridInput {
    /// 列数（8 / 10 / 12 / 14）。
    pub columns: usize,
    /// 渲染模式。
    pub mode: LayoutMode,
    /// 可用内容区宽（已扣工具栏 / 状态栏 / 四边留白）。
    pub content_w: f32,
    /// 可用内容区高。
    pub content_h: f32,
    /// 字级倍率（整页模式下并入缩放系数）。
    pub zoom: f32,
    /// 版框 + 内边距双边预留。
    pub edge: f32,
    /// 天头预留。
    pub head: f32,
    /// 地脚预留。
    pub foot: f32,
    /// 四周留白比例。
    pub margin: f32,
    /// 行高缩放（随正文字体）。
    pub line_scale: f32,
    pub dir: GridDir,
    /// 字号上限（px）：常规阅读 = [`GRID_FS_MAX`]；专注模式传
    /// `f32::INFINITY`（整屏只排一叶，就是要大字）。
    pub fs_max: f32,
}

/// [`calculate_grid`] 的输出：网格度量 + 该模式的元信息。
#[derive(Clone, Copy, Debug)]
pub struct GridPlan {
    /// 网格度量（列宽 / 字号 / 行高 / 行数 / 纸面尺寸）。
    pub m: GridMetrics,
    /// 实际参与计算的内容区（整页模式是**缩放后**的虚拟尺寸）。
    pub box_w: f32,
    pub box_h: f32,
    /// 是否按「两叶 + 中缝」铺排。
    pub facing: bool,
    /// 整页模式的缩放系数（其余模式恒为 1.0）。
    pub scale: f32,
}

/// **统一入口**：`calculateGrid(columns, mode)`。
///
/// 三种模式在这里分派一次，之后共用同一套算式：
///   * 对开：`content / 2`（扣中缝）给单叶 → 两叶 + 缝；
///   * 单叶：整幅 `content` 都给这一叶 → 字最大；
///   * 整页：按 [`full_scale`] 把基准版式等比缩放，再走「两叶 + 缝」。
///
/// 字号始终 = 0.75 × 列宽，并有 [`GRID_FS_MIN`] 下限兜底（14 列窄屏
/// 也不会小如蚂蚁；触发时反推列宽、由渲染层开启滚动）。
pub fn calculate_grid(inp: GridInput) -> GridPlan {
    let scale = match inp.mode {
        LayoutMode::Full => full_scale(inp.content_w, inp.content_h, inp.zoom),
        _ => 1.0,
    };
    let (bw, bh, facing, zoom) = match inp.mode {
        LayoutMode::Facing => (inp.content_w, inp.content_h, true, inp.zoom),
        LayoutMode::Single => (inp.content_w, inp.content_h, false, inp.zoom),
        LayoutMode::Full => (FULL_BASE_W * scale, FULL_BASE_H * scale, true, 1.0),
    };
    let cfg = GridConfig {
        columns: inp.columns.max(1),
        dir: inp.dir,
        margin: inp.margin,
        line_scale: inp.line_scale,
    };
    // 整页：版框 / 天头 / 地脚也乘同一系数——否则「边框不缩放」会让内部
    // 比例随屏幕漂移，那就不是「等比缩放」而是「换个版式」了。
    // 整页模式是「把基准版式等比缩小看全貌」，字号随缩放走，**不能**套
    // 阅读用的 32px 上限——那会把整页排成大格小字，比例全毁。
    let fs_max = match inp.mode {
        LayoutMode::Full => f32::INFINITY,
        _ => inp.fs_max,
    };
    let m = compute_grid(
        bw,
        bh,
        cfg,
        facing,
        zoom,
        inp.edge * scale,
        inp.head * scale,
        inp.foot * scale,
        0.0,
        fs_max,
    );
    GridPlan {
        m,
        box_w: bw,
        box_h: bh,
        facing,
        scale,
    }
}

/// 给定网格坐标 `(col, row)`，返回该格左上角的 `(x, y)`。
///
/// 这是横竖排共用的「格子坐标」逻辑——方向只改变 x 的映射：
/// · 竖排：`col = 0` 落在最右，越往左 `col` 越大（中文从右读起）；
/// · 横排：`col = 0` 落在最左，向右递增。
///
/// `y` 两个方向一致（从上往下）。
pub fn cell_origin(m: &GridMetrics, col: usize, row: usize) -> (f32, f32) {
    let x = match m.dir {
        GridDir::Vertical => (m.columns - 1 - col) as f32 * m.col_w,
        GridDir::Horizontal => col as f32 * m.col_w,
    };
    let y = row as f32 * m.row_h;
    (x, y)
}

/// 读序：返回一个按阅读顺序排列的 `(col, row)` 序列（横竖排共用，方向决定顺序）。
///
/// · 竖排：先排最右一列，列内自上而下；再向左一列，直到最左（从右→左、上→下）。
/// · 横排：先排最上一行，行内自左向右；再向下一行，直到最下（上→下、左→右）。
pub fn reading_order(cols: usize, rows: usize, dir: GridDir) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(cols * rows);
    match dir {
        GridDir::Vertical => {
            for col in (0..cols).rev() {
                for row in 0..rows {
                    out.push((col, row));
                }
            }
        }
        GridDir::Horizontal => {
            for row in 0..rows {
                for col in 0..cols {
                    out.push((col, row));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> (f32, f32) {
        (1320.0 - 44.0, 880.0 - 78.0 - 44.0)
    }

    // ---- 三种渲染模式（v1.23.0）----

    fn input(mode: LayoutMode, columns: usize, cw: f32, ch: f32) -> GridInput {
        GridInput {
            columns,
            mode,
            content_w: cw,
            content_h: ch,
            zoom: 1.0,
            edge: 50.0,
            head: 0.0,
            foot: 12.0,
            margin: 0.04,
            line_scale: 1.0,
            dir: GridDir::Vertical,
            fs_max: GRID_FS_MAX,
        }
    }

    #[test]
    fn three_modes_share_one_grid_formula() {
        let (cw, ch) = content();
        let single = calculate_grid(input(LayoutMode::Single, 10, cw, ch));
        let facing = calculate_grid(input(LayoutMode::Facing, 10, cw, ch));
        let full = calculate_grid(input(LayoutMode::Full, 10, cw, ch));
        // 铺几叶：对开与整页铺两叶 + 中缝，手机竖屏只铺一叶。
        assert!(facing.facing, "对开 = 两叶");
        assert!(full.facing, "整页 = 一整叶（两叶 + 中缝）");
        assert!(!single.facing, "手机竖屏 = 单叶");
        // 单叶把整幅宽都给了这一叶 → 字更大。
        assert!(
            single.m.col_w > facing.m.col_w,
            "单叶列宽 {:.1} 应大于对开 {:.1}",
            single.m.col_w,
            facing.m.col_w
        );
        // 整页：整叶必须完整塞进内容区（等比缩放，不留裁切）。
        assert!(full.m.outer_w <= cw + 0.5, "整页纸宽 {:.1} 超内容区 {:.1}", full.m.outer_w, cw);
        assert!(full.m.outer_h <= ch + 0.5, "整页纸高 {:.1} 超内容区 {:.1}", full.m.outer_h, ch);
        assert_eq!(single.scale, 1.0);
        assert_eq!(facing.scale, 1.0);
        assert!(full.scale > 0.0 && full.scale <= FULL_SCALE_MAX);
        // 三模式共用同一套算式：fs = 0.75 × col_w，row_h = 1.0 × col_w。
        for p in [single, facing, full] {
            assert!(p.m.fs >= GRID_FS_MIN - 0.01, "字号跌破下限：{:.2}", p.m.fs);
            assert!(p.m.row_h > p.m.fs, "row_h 必须大于 fs（竖排不上下重叠）");
            if !p.m.min_font_hit {
                // 触上限时列宽不再等于 fs/0.75（大屏上会保底 70%），
                // 但字绝不会比「贴着格」还大。
                assert!(
                    p.m.fs <= p.m.col_w * GRID_FS_RATIO + 0.01,
                    "fs 超过 0.75×col_w：{:.2} > {:.2}",
                    p.m.fs,
                    p.m.col_w * GRID_FS_RATIO
                );
                if p.m.fs < GRID_FS_MAX - 0.01 {
                    assert!((p.m.fs - p.m.col_w * GRID_FS_RATIO).abs() < 0.01, "fs 非 0.75×col_w");
                }
            }
        }
    }

    #[test]
    fn full_mode_scales_without_changing_inner_proportions() {
        let a = calculate_grid(input(LayoutMode::Full, 10, 1200.0, 800.0));
        let b = calculate_grid(input(LayoutMode::Full, 10, 600.0, 400.0));
        // 屏幕减半 → 缩放系数减半（取宽高里更紧的一边）。
        assert!((b.scale / a.scale - 0.5).abs() < 0.02, "缩放未等比：{:.3} vs {:.3}", a.scale, b.scale);
        // 内部网格同比例缩放：列宽 / 字号 / 行高都恰好减半。
        assert!((b.m.col_w / a.m.col_w - 0.5).abs() < 0.02, "列宽未等比缩放");
        assert!((b.m.fs / a.m.fs - 0.5).abs() < 0.02, "字号未等比缩放");
        assert!((b.m.row_h / a.m.row_h - 0.5).abs() < 0.02, "行高未等比缩放");
        // 列数与行数不变——「只对外层缩放，不改内部网格比例」。
        assert_eq!(a.m.columns, b.m.columns);
        assert!((a.m.rows as i32 - b.m.rows as i32).abs() <= 1, "行数不该随屏幕变：{} vs {}", a.m.rows, b.m.rows);
    }

    #[test]
    fn column_switch_scales_linearly_and_guards_min_font() {
        let (cw, ch) = content();
        let mut prev = f32::MAX;
        for n in GRID_COLUMN_CHOICES {
            let p = calculate_grid(input(LayoutMode::Single, n, cw, ch));
            assert_eq!(p.m.columns, n);
            // 列数越多列宽越小（线性反比）。
            assert!(p.m.col_w <= prev + 0.01, "{n} 列列宽未随列数收窄");
            prev = p.m.col_w;
            assert!(p.m.fs >= GRID_FS_MIN - 0.01, "{n} 列字号跌破 {GRID_FS_MIN}");
        }
        // 极窄屏 14 列：触发最小字号保护 → 反推列宽，字不小于 12px。
        let p = calculate_grid(input(LayoutMode::Single, 14, 240.0, 480.0));
        assert!(p.m.min_font_hit, "窄屏 14 列应触发最小字号保护");
        assert!(p.m.fs >= GRID_FS_MIN - 0.01, "保护后字号仍 {:.2}", p.m.fs);
        assert!(
            p.m.col_w >= p.m.fs / GRID_FS_RATIO - 0.01,
            "反推列宽不足：col_w={:.2} fs={:.2}",
            p.m.col_w,
            p.m.fs
        );
    }

    #[test]
    fn font_size_is_capped_at_32_on_wide_screens() {
        // 大屏单叶 12 列：按算式本该排出上百磅的字（「大字本」的根源），
        // 封顶 32 后列宽跟着回缩，字仍不小于「贴格」所需。
        let cfg = GridConfig { columns: 12, ..GridConfig::default() };
        let capped = compute_grid(2560.0, 1400.0, cfg, false, 1.0, 50.0, 0.0, 12.0, 0.0, GRID_FS_MAX);
        assert!(
            (capped.fs - GRID_FS_MAX).abs() < 0.01,
            "大屏字号应封顶 {GRID_FS_MAX}：{:.2}",
            capped.fs
        );
        assert!(
            capped.col_w >= capped.fs / GRID_FS_RATIO - 0.01,
            "触顶后列宽收得比字还窄：col_w={:.2} fs={:.2}",
            capped.col_w,
            capped.fs
        );
        // 传无限上限（专注模式）：不封顶，整屏就该是大字。
        let free = compute_grid(2560.0, 1400.0, cfg, false, 1.0, 50.0, 0.0, 12.0, 0.0, f32::INFINITY);
        assert!(
            free.fs > GRID_FS_MAX * 1.5,
            "专注模式不该被 {GRID_FS_MAX} 限住：{:.1}",
            free.fs
        );
    }

    #[test]
    fn layout_mode_labels_and_spreads() {
        assert_eq!(LayoutMode::ALL.len(), 3);
        for (i, m) in LayoutMode::ALL.iter().copied().enumerate() {
            assert_eq!(m.index(), i);
            assert_eq!(LayoutMode::from_index(i), m);
            assert!(!m.label().is_empty());
            assert!(!m.hint().is_empty());
        }
        assert!(LayoutMode::Facing.spreads());
        assert!(LayoutMode::Full.spreads());
        assert!(!LayoutMode::Single.spreads());
        // 8/10/12/14 四档都有对应版式，且列数选择器取到的是标准版。
        for n in GRID_COLUMN_CHOICES {
            let cfg = LAYOUT_CONFIGS.iter().find(|c| c.columns == n).unwrap();
            assert_eq!(cfg.columns, n);
            assert!(cfg.key.starts_with("standard"), "{n} 列应命中标准版式");
        }
        // 已删除的「信笺」键要能平滑迁移到标准 14 列。
        assert_eq!(LayoutConfig::migrate_key("letter14"), "standard14");
        assert_eq!(LayoutConfig::migrate_key("nope"), LayoutConfig::default_key());
    }

    #[test]
    fn formulas_match_spec() {
        // 竖排 12 列：col_w = 可用宽/12，fs = 0.75*col_w，row_h = 1.0*col_w。
        // extra_cols = 0：章题列在格内，不占额外宽。
        let (cw, ch) = content();
        // 传无限上限：这一条验的是「算式本身」，不该被 32px 的阅读上限干扰
        // （1276 宽单叶的字号本来会上百，触顶后列宽回缩，验不出公式）。
        let cfg = GridConfig { columns: 12, ..GridConfig::default() };
        let m = compute_grid(cw, ch, cfg, false, 1.0, 50.0, 0.0, 12.0, 0.0, f32::INFINITY);
        let inner_w = (cw - 50.0 - 2.0 * cw * 0.04).max(40.0);
        let expect_col = inner_w / 12.0;
        assert!((m.col_w - expect_col).abs() < 0.01, "col_w {:.2} ≠ {:.2}", m.col_w, expect_col);
        assert!((m.fs - expect_col * 0.75).abs() < 0.01, "fs 非 0.75*col_w");
        assert!((m.row_h - expect_col * GRID_ROW_RATIO).abs() < 0.01, "row_h 非 1.0*col_w");
        // row_h 必大于 fs（竖排不上下重叠）。
        assert!(m.row_h > m.fs, "row_h({}) 必须大于 fs({})", m.row_h, m.fs);
    }

    #[test]
    fn twelve_columns_no_overflow_or_overlap() {
        // 12 列：字格整张纸宽不超过内容区；且 row_h > fs 保证不重叠。
        let (cw, ch) = content();
        let m = compute_grid(
            cw,
            ch,
            GridConfig {
                columns: 12,
                dir: GridDir::Vertical,
                margin: 0.04, line_scale: 1.0,
            },
            true,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,GRID_FS_MAX);
        assert!(m.outer_w <= cw + 0.5, "12 列纸宽 {:.1} 超内容区 {:.1}", m.outer_w, cw);
        assert!(m.outer_h <= ch + 0.5, "12 列纸高 {:.1} 超内容区 {:.1}", m.outer_h, ch);
        assert!(m.row_h > m.fs, "12 列 row_h 必须 > fs");
        // 列越多越密：12 列的列宽必然小于 8 列。字号在两者都触 32px 上限时
        // 会相等——这正是上限的意思（不再随列数变大的那种「大字本」）。
        let m8 = compute_grid(cw, ch, GridConfig::default(), true, 1.0, 50.0, 0.0, 12.0, 0.0, GRID_FS_MAX);
        assert!(m.col_w < m8.col_w + 0.01, "12 列列宽 {:.1} 应不大于 8 列 {:.1}", m.col_w, m8.col_w);
        assert!(m.fs <= m8.fs + 0.01, "12 列字级不该大于 8 列");
        assert!(m.rows >= m8.rows, "行高收紧后行数不减少");
    }

    #[test]
    fn fourteen_columns_still_fits() {
        let (cw, ch) = content();
        let m = compute_grid(
            cw,
            ch,
            GridConfig {
                columns: 14,
                dir: GridDir::Vertical,
                margin: 0.04, line_scale: 1.0,
            },
            false,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,GRID_FS_MAX);
        assert!(m.outer_w <= cw + 0.5);
        assert!(m.outer_h <= ch + 0.5);
        assert!(m.row_h > m.fs);
    }

    #[test]
    fn min_font_prevents_tiny_glyphs_on_narrow_screen() {
        // 极窄内容区（200×640）+ 14 列：触发最小字号，fs 不应低于下限，
        // 且 col_w 反推保证字不超格。
        let m = compute_grid(
            200.0,
            640.0,
            GridConfig {
                columns: 14,
                dir: GridDir::Vertical,
                margin: 0.04, line_scale: 1.0,
            },
            false,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,GRID_FS_MAX);
        assert!(m.min_font_hit, "窄屏 14 列应触发最小字号");
        assert!(m.fs >= GRID_FS_MIN - 0.01, "fs 不得低于下限 {GRID_FS_MIN}");
        assert!(m.col_w >= m.fs / GRID_FS_RATIO - 0.01, "col_w 须 ≥ fs/0.75，否则字超格");
    }

    #[test]
    fn vertical_reading_order_right_to_left() {
        // 竖排 3 列 × 2 行：读序应为 (2,0)(2,1)(1,0)(1,1)(0,0)(0,1)。
        let order = reading_order(3, 2, GridDir::Vertical);
        assert_eq!(
            order,
            vec![(2, 0), (2, 1), (1, 0), (1, 1), (0, 0), (0, 1)]
        );
        // 坐标：竖排 col=2 在最右（x 最小），col=0 在最左（x 最大）。
        let m = GridMetrics {
            dir: GridDir::Vertical,
            columns: 3,
            rows: 2,
            col_w: 10.0,
            row_h: 11.0,
            fs: 7.5,
            grid_w: 30.0,
            grid_h: 22.0,
            outer_w: 30.0,
            outer_h: 22.0,
            min_font_hit: false,
        };
        assert_eq!(cell_origin(&m, 2, 0), (0.0, 0.0));
        assert_eq!(cell_origin(&m, 0, 0), (20.0, 0.0));
    }

    #[test]
    fn horizontal_reading_order_top_to_bottom() {
        // 横排 3 列 × 2 行：读序应为 (0,0)(1,0)(2,0)(0,1)(1,1)(2,1)。
        let order = reading_order(3, 2, GridDir::Horizontal);
        assert_eq!(
            order,
            vec![(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)]
        );
        let m = GridMetrics {
            dir: GridDir::Horizontal,
            columns: 3,
            rows: 2,
            col_w: 10.0,
            row_h: 11.0,
            fs: 7.5,
            grid_w: 30.0,
            grid_h: 22.0,
            outer_w: 30.0,
            outer_h: 22.0,
            min_font_hit: false,
        };
        assert_eq!(cell_origin(&m, 0, 0), (0.0, 0.0));
        assert_eq!(cell_origin(&m, 2, 0), (20.0, 0.0));
    }

    #[test]
    fn portrait_phone_renders_single_leaf() {
        // 手机竖屏 390×844：宽高比 0.46 → 单叶（版心贴右边缘）。
        assert!(is_narrow(390.0, 844.0));
        assert!(!should_face(390.0, 844.0, 10, 1.0));
    }

    #[test]
    fn landscape_tablet_renders_facing_pair() {
        // 平板横屏 1194×834：宽高比 1.43，单叶宽 569 → 双叶对开。
        assert!(!is_narrow(1194.0, 834.0));
        assert!(should_face(1194.0, 834.0, 10, 1.0));
        assert!(should_face(1440.0, 900.0, 12, 1.0));
    }

    #[test]
    fn facing_falls_back_when_font_would_be_too_small() {
        // 宽屏但「14 列 + 极小字级」：双叶每叶字号 < 14px → 退回单叶，
        // 宁可只排一叶也要保证字号够大。
        assert!(should_face(1194.0, 834.0, 14, 1.0));
        assert!(!should_face(1194.0, 834.0, 14, 0.3));
    }

    #[test]
    fn narrow_single_leaf_keeps_the_whole_width() {
        // 窄屏单叶：整幅宽都给这一叶，列宽约为双叶时的两倍。
        let vw = 420.0;
        let single_w = vw - 2.0 * RESP_MARGIN;
        let facing_w = (vw - 2.0 * RESP_MARGIN - RESP_GUTTER) / 2.0;
        assert!(single_w > facing_w * 1.9);
        assert!(!should_face(vw, 900.0, 10, 1.0));
    }

    #[test]
    fn layout_configs_are_the_single_source_of_truth() {
        // 注册表恰好是用户指定的四档列数（8/10/12/14），键 / 标签 / 列数
        // 一一对应；「信笺」模板已删除，不再出现在这张表里。
        assert_eq!(LAYOUT_CONFIGS.len(), 4);
        let keys: Vec<&str> = LAYOUT_CONFIGS.iter().map(|c| c.key).collect();
        assert_eq!(keys, vec!["standard8", "standard10", "standard12", "standard14"]);
        for c in LAYOUT_CONFIGS {
            assert_eq!(c.dir, GridDir::Vertical, "{} 应为竖排", c.key);
            assert!(
                !c.key.contains("letter"),
                "{} 信笺模板已删除，不该还在注册表里",
                c.key
            );
        }
        // 默认档是 standard12（v1.28.0 起提高密度）。
        assert_eq!(LayoutConfig::default().key, "standard12");
        assert_eq!(LayoutConfig::default().columns, 12);
        // 注册表是精确查表：未知键查不到（`None`），回落由调用方
        // （`current_layout` / `set_layout`）统一处理，从而 `set_layout`
        // 能在解析失败时安全返回，不污染状态。
        assert!(LayoutConfig::by_key("nope").is_none());
    }

    #[test]
    fn content_columns_steps_up_as_chapters_grow() {
        // 篇幅越长 → 列数越多（越密）。四档正好落在可选列数上。
        assert_eq!(content_columns(60), 8, "语录短章（《道德经》体量）");
        assert_eq!(content_columns(300), 10);
        assert_eq!(content_columns(800), 12);
        assert_eq!(content_columns(5000), 14, "长篇章回");
        // 单调递增：不会「更长的书反而排得更疏」。
        let mut prev = 0;
        for avg in [10, 150, 500, 1200, 99999] {
            let c = content_columns(avg);
            assert!(c >= prev, "篇幅增长时列数回退：{avg} → {c}");
            prev = c;
        }
        // 每档都在可选列数里（面板上点得到）。
        for avg in [0, 149, 150, 499, 500, 1199, 1200, 100000] {
            let c = content_columns(avg);
            assert!(
                GRID_COLUMN_CHOICES.contains(&c),
                "内容档 {c} 不在可选列数里"
            );
        }
    }

    #[test]
    fn auto_columns_follows_the_book_not_a_fixed_default() {
        // 同一块屏幕（半叶 560），书不同 → 列数不同。
        // 这正是「自适应文章内容」：不是所有书都套一个默认档。
        let fs_of = |c: usize| 560.0 / c as f32 * GRID_FS_RATIO;
        let quote = auto_columns(fs_of, 60); // 语录短章
        let essay = auto_columns(fs_of, 300);
        let misc = auto_columns(fs_of, 800); // 常篇
        let novel = auto_columns(fs_of, 5000); // 长篇章回
        assert!(quote < misc && misc < novel, "篇幅越长越密：{quote} {misc} {novel}");
        assert_eq!(quote, 8, "短章在宽屏上仍排 8 列（字大、纸窄、疏朗）");
        assert_eq!(novel, 14, "长篇排最密档");
        assert_eq!(essay, 10);
        assert_eq!(misc, 12);
    }

    #[test]
    fn auto_columns_only_retreats_when_the_type_gets_too_small() {
        // 屏幕只在「字跌破下限」时兜底，不会替书做主——
        // 这是修掉的老毛病：早先用「字号接近目标值」打分，宽屏上 8/10/12
        // 列的字号全被 32px 上限压成同一个数，判据失去区分度，任何书都
        // 只能选最密档，自适应名存实亡。
        let wide = |c: usize| 560.0 / c as f32 * GRID_FS_RATIO;
        assert_eq!(auto_columns(wide, 60), 8, "宽屏不改变内容档");
        // 极窄屏（半叶 300）：14 列字只有 16px、12 列 18.75px，还过得去；
        // 换成半叶 150 才逼得动 14 列往回退。
        let tiny = |c: usize| 150.0 / c as f32 * GRID_FS_RATIO;
        let c = auto_columns(tiny, 5000);
        assert!(c < 14, "极窄屏上长篇也得往疏处退：{c}");
        assert!(tiny(c) >= GRID_FS_MIN - 0.01, "退到 {c} 列字号仍 {:.1}", tiny(c));
    }

    #[test]
    fn auto_columns_never_lands_below_the_comfort_floor() {
        // 屏幕兜底的底线：任何宽度下都不会停在「字小到读不动」的那一档
        // （除非已经退到最疏档——那时由 GRID_FS_MIN 抬字号、溢出滚动）。
        for w in [260.0, 320.0, 420.0, 560.0, 700.0, 900.0] {
            let c = auto_columns(|c| w / c as f32 * GRID_FS_RATIO, 500);
            let fs = w / c as f32 * GRID_FS_RATIO;
            assert!(
                fs >= crate::grid::GRID_FS_MIN || c == GRID_COLUMN_CHOICES[0],
                "宽 {w} 选了 {c} 列，字号 {fs} 跌破下限"
            );
        }
    }

    #[test]
    fn columns_derive_from_config_not_hardcode() {
        // 列数全部来自配置，引擎不写死 8/10/12/14 的 if-else。
        for c in LAYOUT_CONFIGS {
            let m = compute_grid(
                1320.0 - 44.0,
                880.0 - 78.0 - 44.0,
                GridConfig {
                    columns: c.columns,
                    dir: c.dir,
                    margin: c.margin,
                    line_scale: 1.0,
                },
                false,
                1.0,
                50.0,
                0.0,
                12.0,
                0.0,GRID_FS_MAX);
            // 四档都不溢出内容区，且不重叠（比例约束恒成立）。
            assert!(m.outer_w <= 1320.0 - 44.0 + 0.5, "{} 纸宽溢出", c.key);
            assert!(m.outer_h <= 880.0 - 78.0 - 44.0 + 0.5, "{} 纸高溢出", c.key);
            assert!(m.row_h > m.fs, "{} row_h 必须 > fs", c.key);
        }
    }

    #[test]
    fn line_height_scale_loosens_rows_and_keeps_glyphs_apart() {
        let grid = |scale: f32| {
            compute_grid(
                800.0,
                600.0,
                GridConfig {
                    columns: 8,
                    dir: GridDir::Vertical,
                    margin: 0.04,
                    line_scale: scale,
                },
                false,
                1.0,
                50.0,
                0.0,
                12.0,
                0.0,GRID_FS_MAX)
        };
        let base = grid(1.0);
        let kai = grid(1.12);
        // 行高随缩放放大 → 每列能排的行数相应减少（分页与渲染同源，不溢出）。
        assert!(kai.row_h > base.row_h, "{} vs {}", kai.row_h, base.row_h);
        assert!(kai.rows <= base.rows);
        // 无论怎么缩放，行高始终大于字号：竖排绝不会上下重叠。
        assert!(kai.row_h > kai.fs);
        assert!(base.row_h > base.fs);
        // 中性值 = 不缩放。
        assert!((grid(crate::grid::LINE_SCALE_BASE).row_h - base.row_h).abs() < 0.01);
        // 异常值被夹住，行气不至于散掉或压成一团。
        let wild = grid(9.0);
        assert!(wild.row_h <= base.row_h * crate::grid::LINE_SCALE_MAX + 0.01);
        let tiny = grid(0.01);
        assert!(tiny.row_h >= base.row_h * crate::grid::LINE_SCALE_MIN - 0.01);
    }
}
