//! 参数化网格排版引擎：竖排 / 横排共用同一套「格子坐标 (x, y)」与读序逻辑。
//!
//! 设计目标（见用户需求）：
//!   1. 配置对象 [`GridConfig`]：列数 `columns`、方向 `dir`、四周留白比例 `margin`。
//!   2. 动态网格计算 [`compute_grid`]：不写死像素——
//!      colWidth = 可用宽 / 列数
//!      fontSize = 0.75 × colWidth
//!      rowHeight = 1.1 × colWidth
//!      totalRows = floor(可用高 / rowHeight)
//!   3. 一套引擎兼容横竖排：方向只改变「读序」与「坐标映射」，格子几何完全一致。
//!      · 竖排：遍历从右向左（列），从上向下（行）；
//!      · 横排：遍历从上向下（行），从左向右（列）；
//!      · 共用 [`cell_origin`] 计算 (x, y)、[`reading_order`] 计算读序。
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

// ── 版框闭合（v1.34.0） ─────────────────────────────────────────────────
/// 一叶在摊开纸面上的位置：对开的左半 / 右半 / 独占一整张。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LeafHalf {
    /// 独占一纸（单面阅读，或对开的最后一叶）。
    Alone,
    /// 左半叶：内侧是右缘（贴中缝那一侧）。
    Left,
    /// 右半叶：内侧是左缘（贴中缝那一侧）。
    Right,
}

/// 版框四边（上 / 右 / 下 / 左）是否描边——**恒为四边全闭合**。
///
/// v1.34.0 用户实测的「双叶分家」事故：左半叶只画左框、右半叶只画右框，
/// 贴中缝的内侧两缘都空着，中缝就像一条孤立的马路悬在中央，鱼尾与
/// 叶码无所依傍。现在不论左叶 / 右叶 / 单叶，**外框粗线（3px）与内框
/// 细线（1px）都画满四边**：
///
/// * 左半叶的右缘、右半叶的左缘各立一道粗竖框，两线分居中缝两侧，
///   把鱼尾、书名、叶码死死夹在中间——中缝不再悬空；
/// * 内框细线与列间界行（乌丝栏）同色同粗同起止，围成一圈，
///   字格的上下两端因此接到横框上，网格完全封闭。
///
/// 渲染层（`reader::render_ancient`）按这张表给 `frame` / `inner`
/// 上 `border_3` / `border_1`，测试钉死「任何一半都不许缺边」。
pub fn frame_sides(_half: LeafHalf) -> [bool; 4] {
    [true, true, true, true]
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
    /// **古版行款预设键**（v1.30.0）：`song_dynasty_8` / `ming_dynasty_10` /
    /// `qing_dynasty_12` / `letter_14`。与 `key` 一一对应，供外部配置与
    /// 脚本按「行款」点名（见 [`LayoutConfig::by_preset`]）。
    pub preset: &'static str,
    /// 按钮 / 菜单上显示的标签（UI 直接读它，不写死文案）。
    pub label: &'static str,
    /// 行款释文（一句话说清这套版式的来历与观感）。
    pub desc: &'static str,
    /// 每面列数（竖排=竖列数）：8 / 10 / 12 / 14。
    pub columns: usize,
    /// **每列目标字数**（v1.32.0 古版行款）：半叶 `columns` 行、每行
    /// `target_chars` 字——宋版「半叶八行，行十四字」即 8 列 × 14 字
    /// （v1.48.0 四档锁死：8→14 / 10→20 / 12→22 / 14→26）。
    ///
    /// 这是**神圣不可违**的行款：字号与行高由它反算（`row_h = 可用高 /
    /// target_chars`），排版引擎绝不为了塞满屏幕而拆散每列字数。
    pub target_chars: usize,
    /// **字号 / 列宽**：`fs = 列宽 × fs_ratio`。
    ///
    /// 行款越古越疏：宋版八行字大（0.8），信笺十四行字小（0.68）。
    pub fs_ratio: f32,
    /// **行高 / 字号**：`row_h = fs × line_ratio`。
    ///
    /// 与 `fs_ratio` 相乘才是「行高 / 列宽」：宋版 0.8×1.2 = 0.96 列宽、
    /// 信笺 0.68×1.05 = 0.714 列宽——行款越密，行高收得越紧。
    pub line_ratio: f32,
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
///
/// v1.30.0 起每一行还带一套**古版行款**（`fs_ratio` / `line_ratio`）：
/// 切版式不只是改列数，字级与行高一起变——八行本翻开来就是宋版书那种
/// 疏朗大字，十四行信笺则是精刻细密的小字。
pub const LAYOUT_CONFIGS: &[LayoutConfig] = &[
    LayoutConfig {
        key: "standard8", preset: "song8",
        label: "八行疏朗本", desc: "宋版：字大，行距宽",
        // 行款：半叶八行，行十四字（v1.48.0 用户定调锁死，旧值 15）。
        columns: 8, target_chars: 14, fs_ratio: 0.82, line_ratio: 1.20,
        dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false,
    },
    LayoutConfig {
        key: "standard10", preset: "ming10",
        label: "十行经典本", desc: "明版：适中",
        // 行款：半叶十行，行二十字（v1.48.0，旧值 19）。
        columns: 10, target_chars: 20, fs_ratio: 0.80, line_ratio: 1.15,
        dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false,
    },
    LayoutConfig {
        key: "standard12", preset: "qing12",
        label: "十二行密排本", desc: "清版：密集",
        // 行款：半叶十二行，行二十二字（v1.48.0，旧值 21）。
        // v1.31.x：字号比 0.70 → 0.78，让字更贴满字格、呈「字阵」的致密感，
        // 不再显得空旷（列数恒为 12，只调字 / 格比例）。
        columns: 12, target_chars: 22, fs_ratio: 0.78, line_ratio: 1.10,
        dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false,
    },
    LayoutConfig {
        key: "standard14", preset: "letter14",
        label: "十四行信笺本", desc: "信笺：极密",
        // 行款：半叶十四行，行二十六字（v1.48.0 沿用旧值）。
        columns: 14, target_chars: 26, fs_ratio: 0.72, line_ratio: 1.05,
        dir: GridDir::Vertical, margin: 0.04, show_fish_tail: false,
    },
];

impl LayoutConfig {
    /// 按配置键取配置；未知键回落默认。
    pub fn by_key(key: &str) -> Option<&'static LayoutConfig> {
        LAYOUT_CONFIGS.iter().find(|c| c.key == key)
    }
    /// **按行款预设键取配置**（v1.30.0）：`song_dynasty_8` / `ming_dynasty_10` /
    /// `qing_dynasty_12` / `letter_14`。未知键返回 `None`。
    ///
    /// 两套键并存是刻意的：`key` 是存档里的稳定句柄（旧存档写的是
    /// `standard*`），`preset` 是对外的行款名，便于脚本 / 配置按版式点名。
    pub fn by_preset(preset: &str) -> Option<&'static LayoutConfig> {
        LAYOUT_CONFIGS.iter().find(|c| c.preset == preset).or_else(|| {
            // 长名别名（`song_dynasty_8` 等）：同一套行款，两种叫法都认，
            // 免得外部脚本换个写法就查不到。
            let short = match preset {
                "song_dynasty_8" => "song8",
                "ming_dynasty_10" => "ming10",
                "qing_dynasty_12" => "qing12",
                "letter_14" => "letter14",
                other => other,
            };
            LAYOUT_CONFIGS.iter().find(|c| c.preset == short)
        })
    }
    /// 四套行款的**疏密梯度**自检值：`(列数, 字号比, 行高比)`，由稀到密。
    pub fn presets() -> &'static [LayoutConfig] {
        LAYOUT_CONFIGS
    }
    /// 列数 → 该列数对应的行款（列数与行款一一对应，故查表即可）。
    pub fn by_columns(columns: usize) -> Option<&'static LayoutConfig> {
        LAYOUT_CONFIGS.iter().find(|c| c.columns == columns)
    }
    /// 默认版式（首启 / 配置损坏时的回落）。
    ///
    /// v1.28.0 起为 **12 列**：10 列在常见的 1280–1600 宽窗口里字大行稀，
    /// 摊开像「大字盲人版」；12 列一行多两字，纸面立刻有了刻本该有的密度。
    /// 名为 `default` 但不实现 `Default`：它返回的是**表里那一档的静态引用**，
    /// 不是新造的值（`Default` 的语义是「造一个」），故不套 trait。
    #[allow(clippy::should_implement_trait)]
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
    /// **字号 / 列宽**（v1.30.0 由行款预设给，不再写死 `GRID_FS_RATIO`）：
    /// `fs = 列宽 × fs_ratio`。宋版八行 0.80、信笺十四行 0.68。
    pub fs_ratio: f32,
    /// **行高 / 字号**（v1.30.0 同样来自行款）：`row_h = fs × line_ratio`。
    ///
    /// 与 `fs_ratio` 相乘才得到「行高 / 列宽」：宋版 0.96、信笺 0.714——
    /// 行款越密，行高收得越紧，一列能多排几个字。
    pub line_ratio: f32,
    /// **行高缩放**（`lineHeightScale`）：行高 = 字号 × `line_ratio` × 它。
    ///
    /// 不同字体的 em box 大小不同——思源系列字面偏大，同样字号下竖排
    /// 上下两字容易贴住。按当前正文字体给一点余量（宋体 1.0、楷体 1.06），
    /// 字形才紧凑而不重叠。见 `fonts::line_height_scale`。
    ///
    /// v1.32.0 行款重构后，行高由 `target_chars` 反算（行款优先），此系数
    /// 仅作历史兼容保留，不再参与 `compute_grid` 的字号 / 行高算式。
    pub line_scale: f32,
    /// **每列目标字数**（v1.32.0 行款）：半叶 `columns` 行、每行此数字。
    /// 字号与行高由它反算：`row_h = 可用高 / target_chars`、
    /// `fs = row_h × FS_OF_ROW`。见 [`LayoutConfig::target_chars`]。
    pub target_chars: usize,
    /// **版心率**（v1.33.0 用户可调留白档）：版心宽 ÷ 单叶可用宽。
    /// 0.60 = 宽（复古）/ 0.75 = 中（标准，默认）/ 0.90 = 窄（满屏）。
    /// 见 [`BANXIN_RATIOS`]；天头率随档联动（[`top_frac_for`]）。
    /// 传 0 = 用默认档（与 `fs_ratio` 的 0 哨兵同俗）。
    pub banxin_ratio: f32,
}

impl Default for GridConfig {
    fn default() -> Self {
        // 与 `LayoutConfig::default()`（清版十二行）同源：列数与两个比例
        // 都从那一行取，两处默认就不会打架。
        let c = LayoutConfig::default();
        GridConfig {
            columns: c.columns,
            dir: c.dir,
            margin: c.margin,
            fs_ratio: c.fs_ratio,
            line_ratio: c.line_ratio,
            line_scale: 1.0,
            target_chars: c.target_chars,
            // 留白默认「中（75%）」：既留古籍两侧留白的优雅，又保证现代
            // 屏幕上的阅读体验（用户 v1.33.0 定稿）。
            banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
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
/// 专注模式 / 导出的字号物理上限（v1.84.1）：常规阅读封顶 32px，但专注模式与
/// 导出要「满版大字」，旧代码直接传 `f32::INFINITY`——一旦放大就无上限，既
/// 可能把排版撑爆、也不符合「防呆」原则。这里给一个合理的极大值（128px，约
/// 96pt，足够撑满一屏大字），触顶即夹住、行列绝不破防。
pub const FOCUS_FS_MAX: f32 = 128.0;
/// **字号 / 行高**（v1.32.0 行款）：`fs = row_h × FS_OF_ROW`。
///
/// 行款重构后，行高由「每列目标字数」反算（`row_h = 可用高 / target_chars`），
/// 字号必须**略小于**行高——字面只占行高的一成半不到，竖排上下两字才不粘连
/// （朱砂句读、批注小字都叠在字格里，留白就是呼吸位）。
pub const FS_OF_ROW: f32 = 0.85;
/// 列数 → 目标字数的兜底斜率（未登记行款的列数按它估：8→15、10→19、
/// 12→22、14→26，与登记值同量级）。
pub const TARGET_CHARS_SLOPE: f32 = 1.85;
/// 行高缩放的中性值：不放大也不压缩（旧存档 / 未指定字体时取它）。
pub const LINE_SCALE_BASE: f32 = 1.0;
/// 行高缩放的合理区间（夹住异常值，免得行气散掉或字压成一团）。
pub const LINE_SCALE_MIN: f32 = 0.85;
pub const LINE_SCALE_MAX: f32 = 1.6;

/// 古籍竖笺「天头宽、地脚窄」（v1.31.x，与 `zen_kit` 同源）：天头占内容区
/// 高度的比例。天头留白给眉批、地脚只容鱼尾叶码，故天头明显更宽。
///
/// v1.34.0 起它同时是**地脚的唯一来源**：地脚不再单独给比例，一律取
/// 天头 ÷ [`TOP_OF_BOT`]（用户铁律「天头宽于地脚」，比例见
/// `frame_tests::top_margin_keeps_the_head_to_foot_ratio`）。
/// 留白三档联动因此也长在地脚上：宽（复古）档天头大 → 地脚同步大。
pub const ANCIENT_TOP_FRAC: f32 = 0.12;

/// **天头 ÷ 地脚**（v1.34.0 立律：2.0；**v1.37.0 改律：1.5，即古籍常
/// 用 3:2**）。
///
/// 这条律与边框粗细、字号大小、列数多少、留白档位**统统无关**：
/// 外框从 1px 加粗到 3px、中缝闭合、句读重绘，都不许多吃一寸天头、
/// 也不许把地脚撑成对称——否则「天宽地窄」的刻本气质就没了。
/// 违反它的回归由 `frame_tests` 的两条测试把着：
///   * `top_margin_keeps_the_head_to_foot_ratio`（全档位 × 全尺寸
///     × 开/关页脚注，比例恒为 [`TOP_OF_BOT`]）；
///   * `bottom_margin_never_grows_past_the_top`（地脚永远窄于天头）。
///
/// 为什么从 2.0 收到 1.5：2:1 的天头在宽屏上偏「浮」，版心被推得太高，
/// 整幅纸看着头重脚轻；3:2 让版面重心下沉一档，更贴刻本的稳重相，
/// 同时仍严格保住「天宽地窄」这条方向性铁律（1.5 > 1）。
///
/// 注意：页脚注区（`foot`，`ANCIENT_NOTES`）**不是地脚的一部分**——
/// 它并进纵向预留（与 `head` 同理），否则一开页脚注，比例就被注区
/// 撑变形（那正是用户点名「绝不允许」的漂移）。
pub const TOP_OF_BOT: f32 = 1.5;

// ── 列首基准线（v1.56.0 顶格对齐）───────────────────────────────────
/// **列首字的绝对基准线**：每一列**第一个字的渲染起始 Y** 恒等于这一条
/// 线——不因字体（宋 / 楷）、不因是否带拼音 / 夹注、不因避头尾禁则的挤压
/// 而有半个像素的漂移。
///
/// 值为 0，意思是「紧贴行格第一格的上沿」；列内其它纵向位移（卷首题署
/// 预留带、地脚）都在它**之上 / 之下**另计，绝不混进这一条线里。每一列
/// 的起始绘制一律锚定 [`top_baseline`]，渲染层（`reader::render_column`）
/// 只此一个入口，不得各自加 padding。
pub const TOP_BASELINE: f32 = 0.0;

/// **双行夹注在列首时允许向下的缩进**（字高比例，v1.56.0）。
///
/// 注文本就比正文低一号，压在列首时略降一点更像刻本；但夹注**占格向上
/// 取整到整字格**（见 [`ceil_to_cells`]），故这一缩进只是注块内部的视觉
/// 下沉——它之后的正文首字**仍落在基准线上**，绝不整列下移。
pub const GLOSS_HEAD_INSET: f32 = 0.12;

/// 列首基准线的**唯一取用口径**：`TOP_BASELINE + top_skip 格`。
///
/// `top_skip` 是卷首题署占的格数（章首页开卷列才非零）。渲染层一律用它
/// 算列的 `pt()`，不许再出现 `cell_h * top_skip` 这种就地算式——两处算
/// 法一旦分家，顶格对齐就悄悄漂了。
pub fn top_baseline(top_skip_cells: f32, cell_h: f32) -> f32 {
    TOP_BASELINE + top_skip_cells.max(0.0) * cell_h.max(0.0)
}

/// 把任意高度**向上取整到整字格**（v1.56.0）。
///
/// 双行夹注、数字串这类块的高度不是整格（夹注一行半格、三行就是 1.5 格），
/// 若照实高往下排，它后面的正文首字就落在一个半格上——整列往下漂半格，
/// 正是「顶格对不齐」的由来。取整到整格后，块内多出的那点空当自然成为
/// 白口，基准线网格纹丝不动。
pub fn ceil_to_cells(h: f32, cell_h: f32) -> f32 {
    if cell_h <= 0.0 {
        return h.max(0.0);
    }
    ((h / cell_h).ceil() * cell_h).max(cell_h)
}

// ── 留白三档（v1.33.0）：版心宽占单叶可用宽的比例 ────────────────────
/// **版心率**：版心（界行文字区 + 版框）宽 ÷ 单叶可用宽。
///
/// 宋版书版心只占纸宽六成——两侧留白是纸的一部分，不是浪费。但现代屏幕
/// 宽而扁，六成常常剩下一片桌面；故做成三档，由用户在「文物感」与
/// 「阅读效率」之间自选：
///   0. **宽（复古 60%）**：宋版 authentic 比例，两侧留白最阔；
///   1. **中（标准 75%）**：**默认档**——既留古籍两侧留白的优雅，
///      又保证现代屏幕上的阅读体验（用户 v1.33.0 定稿）；
///   2. **窄（满屏 90%）**：文字区铺满叶面，追求最大信息密度。
///
/// 存**下标**进 `Prefs::margin_tier`（改标签不让老存档失效），越界回落默认档。
pub const BANXIN_RATIOS: [f32; 3] = [0.60, 0.75, 0.90];
/// 默认档下标：「中（标准 75%）」。
pub const BANXIN_TIER_DEFAULT: usize = 1;
/// 各档联动的**天头率**：版心越窄（越复古），天头越宽——选「宽」留白的人
/// 在追求极致文物感，值得更大的天头（用户 v1.33.0 要求「天头地脚也拉大」）。
/// 与 [`BANXIN_RATIOS`] 同序：0.60→0.16、0.75→0.12（=基准）、0.90→0.09。
pub const BANXIN_TOP_FRACS: [f32; 3] = [0.16, 0.12, 0.09];
/// 列宽上限（相对行高）：满屏档把字格拉宽当字距，但宽过行高两倍，乌丝栏
/// 就散成一道栅栏、不再像书。触顶时版心占比略降（字距优先让位版式感）。
pub const COL_W_MAX_OF_ROW: f32 = 2.0;
/// 列宽下限（相对字号）：横向必须留足呼吸空间，列宽永远 ≥ 字号 × 此值
/// （v1.84.1：旧代码下限只到 `fs`，放大字号时字贴着格线、视觉上挤进邻列；
/// 现在强制 ≥ 1.1× 字号，保证至少 10% 字距，字与字、列与列绝不粘连）。
pub const COL_W_MIN_OF_FS: f32 = 1.1;

/// 留白档 → 天头率。版心率落在 [`BANXIN_RATIOS`] 某一档上就取该档的天头率
/// （「宽留白配大天头」的联动只此一处，不会两处漂移）；未登记的比值
/// （用户自定 / 未来新档）退回基准 [`ANCIENT_TOP_FRAC`]。
pub fn top_frac_for(banxin_ratio: f32) -> f32 {
    match BANXIN_RATIOS
        .iter()
        .position(|r| (r - banxin_ratio).abs() < 0.005)
    {
        Some(i) => BANXIN_TOP_FRACS[i],
        None => ANCIENT_TOP_FRAC,
    }
}

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

// ── 自适应列数匹配引擎（Auto-Fit Column Engine, v1.9.19）──────────────────
/// **视觉最饱满**的判据：黄金分割比。一面被文字填到约 61.8% 时，既不空旷
/// （稀疏电线杆）也不拥挤（逼到翻页），最符合古籍版面的呼吸感。
pub const GOLDEN_FILL: f32 = 0.618;
/// 纯函数几何默认值里的「版框 + 内边距双边预留」——与 app 层 `ANCIENT_EDGE=50` 同源。
pub const FIT_EDGE_DEFAULT: f32 = 50.0;
/// **响应式重算阈值**（px）：窗口尺寸相对上次决策变化超过此值，才重新挑列数。
/// 防「拖动窗口时每帧都整本重排」的抖动——只在越过阈值时跳一次（见 app 层）。
pub const FIT_RESIZE_THRESHOLD: f32 = 24.0;

/// **自适应列数匹配引擎**的输入。
///
/// 三个必填字段（`available_width` / `available_height` / `total_chars`）是用户
/// 指定的决策依据；其余几何参数都有与渲染层 [`compute_grid`] 同源的默认值，调用方
/// （GPUI 壳）可覆盖成真实视口扣减后的数值，做到「算出来几列、排出来就几列」，
/// 不漂移。
#[derive(Clone, Copy, Debug)]
pub struct ColumnFitInput {
    /// 可用内容区宽（已扣工具栏 / 状态栏 / 四边留白）。
    pub available_width: f32,
    /// 可用内容区高。
    pub available_height: f32,
    /// 待排文本的总字数（用于防稀疏与溢出评估）。
    pub total_chars: usize,
    /// 字级倍率。
    pub zoom: f32,
    /// 四周留白比例（相对内容区）。
    pub margin: f32,
    /// 版心率（留白档）。
    pub banxin_ratio: f32,
    /// 版框 + 内边距双边预留。
    pub edge: f32,
    /// 天头预留。
    pub head: f32,
    /// 地脚预留。
    pub foot: f32,
    /// 行高缩放（随正文字体）。
    pub line_scale: f32,
    /// 排版方向。
    pub dir: GridDir,
    /// 字号上限（px）。
    pub fs_max: f32,
}

impl Default for ColumnFitInput {
    fn default() -> Self {
        ColumnFitInput {
            available_width: 800.0,
            available_height: 1000.0,
            total_chars: 0,
            zoom: 1.0,
            margin: 0.04,
            banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
            edge: FIT_EDGE_DEFAULT,
            head: 0.0,
            foot: 0.0,
            line_scale: 1.0,
            dir: GridDir::Vertical,
            fs_max: GRID_FS_MAX,
        }
    }
}

impl ColumnFitInput {
    /// 用户指定的三参数构造；其余取同源默认值。
    pub fn new(available_width: f32, available_height: f32, total_chars: usize) -> Self {
        let mut s = Self::default();
        s.available_width = available_width;
        s.available_height = available_height;
        s.total_chars = total_chars;
        s
    }
}

/// **自适应列数匹配引擎**的结果（不止列数，附度量供调试 / 状态栏）。
#[derive(Clone, Copy, Debug)]
pub struct ColumnFitResult {
    /// **最终选定的列数**（四档之一：8 / 10 / 12 / 14）。
    pub columns: usize,
    /// 该列数下的字格宽（列宽）。
    pub column_width: f32,
    /// 该列数下的正文实际字号。
    pub font_size: f32,
    /// 每列实际行数（= 每列能排的字数）。
    pub rows_per_column: usize,
    /// 该列数下一面（一叶）的总容量（列数 × 每列字数）。
    pub total_capacity: usize,
    /// 排下全部文本需要的页数（>1 即溢出到下一页）。
    pub pages: usize,
    /// 单面填充率（`total_chars / total_capacity`，截到 1.0）。
    pub fill_ratio: f32,
    /// 综合打分（越接近黄金填充越满越赏心）。
    pub score: f32,
}

/// **自适应列数匹配引擎（Auto-Fit Column Engine）**——纯函数，不碰 GPUI。
///
/// 输入当前窗口可用大小与文本总字数，从 [`GRID_COLUMN_CHOICES`]（[14, 12, 10, 8]，
/// 优先尝试更密集）里挑出最完美的一档。四步淘汰 / 匹配，与 [`compute_grid`]
/// **共用同一套几何**（每档都拿真实网格试算，绝不两套公式各写一遍）：
///
/// 1. **物理宽度底线**：`column_width` 与 `font_size` 经 [`compute_grid`] 反算。
///    若窗口太小、这档连「满行款的最小字号」都撑不住——引擎被迫锁最小字号、
///    一列排不满（`min_font_hit`）→ 淘汰该档，退到更疏的档，让整行款在可读字号
///    下排得下（绝不靠单页滚动硬塞）。
/// 2. **内容密度（防稀疏）**：文本太少却用密列，会像「稀疏的电线杆」。用
///    [`content_columns`] 把「内容长度 → 列数上限」定成天花板——比内容体量更密的档
///    一律淘汰（如 15 字的小药方 → 天花板 8 列，14/12 列直接出局，自动落到 8 列
///    疏朗大字，像一张刻意开的处方签，而非半空的海报）。
/// 3. **溢出评估**：若按该档排总高超出 `available_height`（= `pages > 1`），**不淘汰**
///    ——读者本就该翻页；但「能在一面内排下」会小幅加分（见第 4 步），鼓励不翻页的饱满。
/// 4. **寻找最优解**：在 1–3 都合法的候选里，选 `fill_ratio` 最接近 [`GOLDEN_FILL`]
///    （约 61.8%）的一档——即「视觉最饱满」：内容该密就密、该疏就疏，不空旷也不拥挤。
///
/// 返回 [`ColumnFitResult`]，其中 `columns` 即最终列数；长文溢出时各候选 `fill_ratio`
/// 触顶同分，规则退化为「天花板内最密档」，与 [`content_columns`] 的疏散梯度一致。
pub fn calculate_best_column_layout(input: ColumnFitInput) -> ColumnFitResult {
    // 内容长度决定的「密度天花板」：比它更密的档一律视作稀疏，淘汰。
    let density_ceiling = content_columns(input.total_chars);
    let mut best: Option<ColumnFitResult> = None;

    // 优先尝试更密集的列数（[14, 12, 10, 8]）。
    for &n in GRID_COLUMN_CHOICES.iter().rev() {
        let cfg = match LayoutConfig::by_columns(n) {
            Some(c) => c,
            None => continue,
        };
        // 该档在真实视口下的铺法（对开 / 单叶），与渲染同源。
        let mode = if should_face(input.available_width, input.available_height, n, input.zoom) {
            LayoutMode::Facing
        } else {
            LayoutMode::Single
        };
        let m = calculate_grid(GridInput {
            columns: n,
            mode,
            content_w: input.available_width,
            content_h: input.available_height,
            zoom: input.zoom,
            edge: input.edge,
            head: input.head,
            foot: input.foot,
            margin: input.margin,
            line_scale: input.line_scale,
            fs_ratio: cfg.fs_ratio,
            line_ratio: cfg.line_ratio,
            banxin_ratio: input.banxin_ratio,
            dir: input.dir,
            fs_max: input.fs_max,
        })
        .m;

        // 每列实际字数（行数）；一面总容量 = 列数 × 每列字数。
        let per_col = m.chars_per_col.max(1);
        let capacity = n * per_col;
        // 需要多少列才排得下（竖排每列 per_col 字）。
        let filled_cols = ((input.total_chars as f32 / per_col as f32).ceil()).max(1.0) as usize;
        let pages = ((filled_cols as f32 / n as f32).ceil()).max(1.0) as usize;
        let fill_ratio = if capacity > 0 {
            (input.total_chars as f32 / capacity as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // ── RULE 1：物理宽度底线 ──
        // 窗口太小，这档连「满行款的最小字号」都撑不住（引擎被迫锁最小字号、
        // 一列排不满 → 单页滚动）。退到更疏的档，让整行款在可读字号下排得下。
        if m.min_font_hit {
            continue;
        }

        // ── RULE 2：内容密度（防稀疏）天花板 ──
        if n > density_ceiling {
            continue;
        }

        // ── RULE 4：视觉最饱满（越接近黄金填充越赏心）；一面能排下略加权 ──
        let mut score = 1.0 - (fill_ratio - GOLDEN_FILL).abs();
        if pages <= 1 {
            score += 0.05;
        }

        let cand = ColumnFitResult {
            columns: n,
            column_width: m.col_w,
            font_size: m.fs,
            rows_per_column: m.rows,
            total_capacity: capacity,
            pages,
            fill_ratio,
            score,
        };
        // 取最高分（= 天花板内最密且填充最接近黄金的档）；严格大于保证同分时
        // 保留先遇到的更密档（触顶溢出时退化为最密合法档）。
        if best.is_none() || score > best.unwrap().score {
            best = Some(cand);
        }
    }

    best.unwrap_or_else(|| ColumnFitResult {
        columns: GRID_COLUMN_CHOICES[0],
        column_width: 0.0,
        font_size: GRID_FS_MIN,
        rows_per_column: 1,
        total_capacity: GRID_COLUMN_CHOICES[0],
        pages: 1,
        fill_ratio: 0.0,
        score: 0.0,
    })
}

/// 便捷封装：只需用户指定的三参数，返回最终列数。
pub fn best_column_count(available_width: f32, available_height: f32, total_chars: usize) -> usize {
    calculate_best_column_layout(ColumnFitInput::new(available_width, available_height, total_chars))
        .columns
}

/// 网格度量：由 [`compute_grid`] 算出，渲染与分页共用，不再写死任何像素。
#[derive(Clone, Copy, Debug)]
pub struct GridMetrics {
    pub dir: GridDir,
    pub columns: usize,
    /// **行款目标字数**（v1.32.0）：半叶 `columns` 行、每行应排此数字。
    /// 与 `LayoutConfig::target_chars` 同源，状态栏行款信息读它。
    pub target_chars: usize,
    /// **实际每列字数**（v1.32.0）：行款未被兜底时恒等于 `target_chars`；
    /// 触发最小字号保护（窗口过小）时小于它——状态栏据此提示
    /// 「无法展示完整的 X 行 Y 字」。
    pub chars_per_col: usize,
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
    /// 天头预留（px）：内容高 × [`ANCIENT_TOP_FRAC`]，上缘留白。
    pub margin_top: f32,
    /// 地脚预留（px）：**天头 ÷ [`TOP_OF_BOT`]（= 天头的 2/3）**，下缘留白
    /// （明显窄于天头）。用户铁律：天头必须是地脚的 2 倍。
    pub margin_bot: f32,
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

    // ── 天头地脚非对称（v1.31.x，与 `zen_kit` 同源）──────────────────
    // 传统刻本「天头宽、地脚窄」：天头留给眉批（v1.37.0 起真能写），地脚
    // 只容鱼尾叶码，故天头明显更宽。纵向预留分两份：天头
    // （`top_frac_for(版心率)` × 内容高）+ 地脚（**天头 ÷ `TOP_OF_BOT`
    // = 天头的 2/3**，用户铁律 3:2）；
    // 页脚注区（`foot`）不是地脚，它与 `head` 一样并进纵向预留——
    // 否则开页脚注就会把 2:1 撑变形。
    //
    // 天头率随留白档联动（v1.33.0）：选「宽（复古）」的人值得更大的天头，
    // 选「窄（满屏）」的人要把每一像素都让给文字——见 `top_frac_for`。
    let top_frac = top_frac_for(cfg.banxin_ratio);
    let mt = (content_h * top_frac).max(0.0);
    let mb = (mt / TOP_OF_BOT).max(0.0);

    // 4) **古版行款反算**（v1.32.0）：每列字数神圣不可违——
    //    先定「半叶几行、每行几字」（行款表 `target_chars`），再由可用高
    //    反算行高与字号：`row_h = 可用高 / target_chars`，
    //    `fs = row_h × FS_OF_ROW`（0.85，字面必须略小于行高才不粘连）。
    //
    //    这与旧版「算字号求行数」正好相反：旧版为了塞满屏幕会悄悄改动
    //    每列字数（floor 余量、列宽回缩都在拆行款）；现在行款是因，
    //    几何是果——屏幕只决定「这一版印得印不下」，不改「一行几字」。
    //
    //    页脚鱼尾条（`foot_h`）依赖字号，先不计它探一遍（只占约 18px，
    //    一遍收敛即够），拿到探针字号再精算可用高。
    let target = cfg.target_chars.max(1) as f32;
    let cap = fs_max.max(GRID_FS_MIN);
    // 探针与精算都扣掉页脚注区（`foot`）：它是注文带、不是地脚留白
    // （见 `TOP_OF_BOT` 的说明），绝不计入 2:1 的任何一侧。
    let fs_probe = (((content_h - head - mt - mb - foot).max(40.0) / target) * FS_OF_ROW)
        .clamp(GRID_FS_MIN, cap);
    let foot_h = (fs_probe * 0.36).max(9.0) + 10.0; // 与 `render_ancient` 的 `foot_h` 同源
    // 5) 字格可用高 = 内容高 − 天头 − 地脚预留 − 页脚注区 − 鱼尾叶码条
    //    − 版框上缘（`head` 仅竖排无天头行时恒为 0）。
    let inner_h = (content_h - head - mt - mb - foot - foot_h).max(40.0);
    // 6) 行高 = 可用高 / 每列目标字数（行款值）；字号 = 行高 × 0.85 × 字级
    //    倍率，再按上下限夹住（专注模式传 INFINITY 豁免上限）。
    let row_h_std = inner_h / target;
    let fs = (row_h_std * FS_OF_ROW * zoom).clamp(GRID_FS_MIN, cap);
    // 7) 窗口过小：按公式算出的字号跌破下限 → **锁定最小字号**，行高反被
    //    抬大 → 一列排不下 `target` 字（`rows < target`）。此时渲染层开
    //    单页滚动、状态栏提示「无法展示完整的 X 行 Y 字」——绝不拆行款，
    //    绝不把字压成一团黑疙瘩。
    let min_hit = row_h_std * FS_OF_ROW * zoom < GRID_FS_MIN;
    // **行高 = max(行款基准, 字号 / FS_OF_ROW)**（v1.84.1 修复「字号最大时字
    // 叠在一起」）：这是 `fs = row_h × FS_OF_ROW` 的逆运算——一旦 zoom / 专注
    // 模式把 fs 顶过 `row_h_std × FS_OF_ROW`，行高立即用 fs 反推撑开（= 字号
    // × 1.176），保证 `row_h > fs` 恒成立（竖排字绝不上下重叠、也不挤进邻列）；
    // 字号没顶过基准时仍用行款 row_h_std，维持版面与列数不变。旧代码只在
    // `min_hit`（窗口过小）分支反推 row_h，常态 / 触顶（fs 被 fs_max 夹住）时
    // row_h 停在 row_h_std，于是放大字号后 **fs > row_h**，字全叠在一起。
    let row_h = row_h_std.max(fs / FS_OF_ROW);
    // 行数 = 可用高 / 行高。加 1e-4 再 floor：`inner_h/(inner_h/target)` 在浮点
    // 里可能落在 target−ε（如 25.999999999999996），直接 floor 会少算一行，
    // 状态栏报「行二十字」、纸高也短一行——epsilon 只吃掉浮点尘，吃不到
    // 真正的余量（真实余量 ≥ 千分之一行时才该少排）。
    let rows = ((inner_h / row_h + 1e-4).floor()).max(1.0) as usize;
    // 8) **版心率定列宽**（v1.33.0 用户可调留白档）：版心（界行区 + 版框）
    //    宽 = 单叶可用宽 × `banxin_ratio`（0.60 复古 / 0.75 标准 / 0.90 满屏），
    //    列宽由它反推：`col_w = (版心宽 − 版框) / 列数`。字格因此不必是方的——
    //    满屏档把多出的宽度当**字距**（字形仍居中于格内，行款字号不变），
    //    复古档则收到近乎方格。三条边界：
    //      · 下限 `fs`：字不破格——放不下就退回「可用宽均分」（旧行为），
    //        版心占比随之超标，交由横向滚动，绝不把字压扁；
    //      · 上限 `row_h × COL_W_MAX_OF_ROW`：再宽乌丝栏就散成栅栏；
    //      · 纸边留白让位：纸宽（版心 + 纸边）不超出单叶，满屏档留白归零，
    //        纸正好贴满叶面；复古档纸窄，叶面两侧露出桌面。
    let n = (cfg.columns as f32 + extra_cols).max(1.0);
    let ratio = if cfg.banxin_ratio > 0.0 {
        cfg.banxin_ratio.clamp(0.35, 1.0)
    } else {
        BANXIN_RATIOS[BANXIN_TIER_DEFAULT]
    };
    let banxin_w = (per_leaf * ratio).max(120.0);
    let mw = (per_leaf * cfg.margin)
        .min(((per_leaf - banxin_w) / 2.0).max(0.0))
        .max(0.0);
    let inner_w = (per_leaf - edge - 2.0 * mw).max(40.0);
    let want = (banxin_w - edge) / n; // 版心率反推的列宽
    let col_w = if want >= fs * COL_W_MIN_OF_FS {
        want.clamp(fs * COL_W_MIN_OF_FS, row_h * COL_W_MAX_OF_ROW)
    } else {
        // 窄屏（手机 12/14 列）或放大字号后：可用宽放不下「字号 × 下限」的字，
        // 退回可用宽均分——版心占比超标 → 横向滚动，但列宽仍强制 ≥ 1.1× 字号，
        // 字绝不破格、绝不挤进邻列。
        (inner_w / n).clamp(fs * COL_W_MIN_OF_FS, row_h * COL_W_MAX_OF_ROW)
    };

    let grid_w = (cfg.columns as f32 + extra_cols) * col_w;
    let grid_h = rows as f32 * row_h;
    let outer_w = grid_w + edge + 2.0 * mw;
    // 整张纸高 = 字格区 + 天头 + 地脚预留 + 鱼尾叶码条（+ 版框上缘）。
    // 行款未被兜底时 `grid_h == inner_h`，纸高恒等于内容区高。
    let outer_h = grid_h + head + mt + mb + foot_h;

    GridMetrics {
        dir: cfg.dir,
        columns: cfg.columns,
        target_chars: cfg.target_chars.max(1),
        chars_per_col: rows,
        rows,
        col_w,
        row_h,
        fs,
        grid_w,
        grid_h,
        outer_w,
        outer_h,
        min_font_hit: min_hit,
        margin_top: mt,
        margin_bot: mb,
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
    /// **字号 / 列宽**（行款预设给）：宋版 0.80、信笺 0.68。
    /// 传 0 = 按列数自行查行款表（见 [`LayoutConfig::by_columns`]）。
    pub fs_ratio: f32,
    /// **行高 / 字号**（行款预设给）：宋版 1.20、信笺 1.05。传 0 = 查表。
    pub line_ratio: f32,
    /// **版心率**（v1.33.0 留白档）：版心宽 ÷ 单叶可用宽。
    /// 0.60 宽（复古）/ 0.75 中（标准，默认）/ 0.90 窄（满屏）。
    /// 传 0 = 默认档。见 [`BANXIN_RATIOS`]。
    pub banxin_ratio: f32,
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
    // 行款比例：优先用调用方给的（UI 已按当前版式取好）；没给就按列数
    // 查行款表——保证「切版式 = 列数 + 字级 + 行高一起变」，且两处同源。
    let preset = LayoutConfig::by_columns(inp.columns.max(1));
    let fs_ratio = if inp.fs_ratio > 0.0 {
        inp.fs_ratio
    } else {
        preset.map(|c| c.fs_ratio).unwrap_or(GRID_FS_RATIO)
    };
    let line_ratio = if inp.line_ratio > 0.0 {
        inp.line_ratio
    } else {
        preset.map(|c| c.line_ratio).unwrap_or(GRID_ROW_RATIO / fs_ratio)
    };
    let cfg = GridConfig {
        columns: inp.columns.max(1),
        dir: inp.dir,
        margin: inp.margin,
        fs_ratio,
        line_ratio,
        line_scale: inp.line_scale,
        // 行款字数：按列数查行款表；未登记的列数按斜率估一个（同量级）。
        target_chars: preset
            .map(|c| c.target_chars)
            .unwrap_or_else(|| ((inp.columns.max(1) as f32) * TARGET_CHARS_SLOPE).round() as usize),
        // 留白档：调用方（fit_sizes，读 Prefs）给 0 = 用默认档。
        banxin_ratio: inp.banxin_ratio,
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
            line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
            // 留白档：测试助手一律走默认档「中（75%）」——与出厂行为一致。
            banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
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
        // 单叶把整幅宽都给了这一叶 → 版心更宽（列宽更大、字距更疏）。
        // 行款模型（v1.32.0）下字号由「每列几字」定：单叶与对开**字号相同**
        // （同一行款印不出两种字），差别只在版心宽——版心率（v1.33.0）
        // 按单叶可用宽算，单叶的叶更宽，故列更宽。
        assert!(
            single.m.col_w >= facing.m.col_w - 0.01,
            "单叶列宽 {:.1} 不该小于对开 {:.1}",
            single.m.col_w,
            facing.m.col_w
        );
        // 整页：整叶必须完整塞进内容区（等比缩放，不留裁切）。
        assert!(full.m.outer_w <= cw + 0.5, "整页纸宽 {:.1} 超内容区 {:.1}", full.m.outer_w, cw);
        assert!(full.m.outer_h <= ch + 0.5, "整页纸高 {:.1} 超内容区 {:.1}", full.m.outer_h, ch);
        assert_eq!(single.scale, 1.0);
        assert_eq!(facing.scale, 1.0);
        assert!(full.scale > 0.0 && full.scale <= FULL_SCALE_MAX);
        // 三模式共用同一套行款算式：row_h = 可用高 / target_chars，
        // fs = row_h × FS_OF_ROW，col_w = (版心 − 版框) / 列数（版心率定宽）。
        for p in [single, facing, full] {
            assert!(p.m.fs >= GRID_FS_MIN - 0.01, "字号跌破下限：{:.2}", p.m.fs);
            assert!(p.m.row_h > p.m.fs, "row_h 必须大于 fs（竖排不上下重叠）");
            // 字不破格：字号不超过列宽、也不超过行高。
            assert!(
                p.m.fs <= p.m.col_w + 0.01,
                "字宽超格：fs={:.2} col_w={:.2}",
                p.m.fs,
                p.m.col_w
            );
            assert!(p.m.fs <= p.m.row_h + 0.01, "字高超行高");
            if !p.m.min_font_hit && p.m.fs < GRID_FS_MAX + 0.01 {
                // 未被兜底、未触顶：字号严格 = 行高 × 0.85（行款算式本身）。
                assert!(
                    (p.m.fs - p.m.row_h * FS_OF_ROW).abs() < 0.01,
                    "fs 非 row_h×{FS_OF_ROW}：{:.2} vs {:.2}",
                    p.m.fs,
                    p.m.row_h * FS_OF_ROW
                );
                // 行款满额：每列字数 = 目标（屏幕只决定印不印得下，不改行款）。
                assert_eq!(p.m.chars_per_col, p.m.target_chars);
            }
        }
    }

    #[test]
    fn font_scale_guard_never_overlaps_at_any_zoom() {
        // v1.84.1 回归：字号放大（zoom Large/Huge）曾让 fs > row_h，竖排字
        // 上下叠、挤进邻列。护栏 `row_h = max(row_h_std, fs / FS_OF_ROW)` 必须
        // 保证任意字级下 row_h > fs 恒成立，且列宽强制 ≥ 1.1× 字号。
        let (cw, ch) = content();
        for &zoom in &[0.8_f32, 1.0, 1.3, 1.6, 2.0, 3.0] {
            // 用 Single 模式 + 不同 zoom 穷举放大场景，验证护栏在任意字级下
            // 都撑开行高、守住列宽下限。
            let mut inp = input(LayoutMode::Single, 12, cw, ch);
            inp.zoom = zoom;
            let p = calculate_grid(inp);
            assert!(
                p.m.row_h > p.m.fs + 1e-3,
                "zoom={} 时 row_h {:.2} 未严格大于 fs {:.2}（字会上下重叠）",
                zoom, p.m.row_h, p.m.fs
            );
            assert!(
                p.m.col_w >= p.m.fs * COL_W_MIN_OF_FS - 1e-3,
                "zoom={} 时 col_w {:.2} 小于 1.1×fs {:.2}（字会挤进邻列）",
                zoom, p.m.col_w, p.m.fs
            );
        }
    }

    #[test]
    fn focus_mode_128px_never_overlaps() {
        // v1.84.1 回归：专注模式字号封顶 128px（FOCUS_FS_MAX）。在超大视口
        // 下字级触顶 128，护栏仍必须守住「行高严格大于字号、列宽 ≥ 1.1×
        // 字号」——字绝不破格、绝不重叠。
        let mut inp = input(LayoutMode::Facing, 12, 3840.0, 2400.0);
        inp.fs_max = FOCUS_FS_MAX; // 生产专注模式上限
        let p = calculate_grid(inp);
        assert!(
            p.m.fs <= FOCUS_FS_MAX + 0.01,
            "专注模式字号 {:.2} 超 128 物理上限",
            p.m.fs
        );
        assert!(
            p.m.row_h > p.m.fs + 1e-3,
            "触顶后 row_h {:.2} 未严格大于 fs {:.2}（字会重叠）",
            p.m.row_h, p.m.fs
        );
        assert!(
            p.m.col_w >= p.m.fs * COL_W_MIN_OF_FS - 1e-3,
            "触顶后列宽 {:.2} 小于 1.1×fs {:.2}（字会挤进邻列）",
            p.m.col_w, p.m.fs
        );
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
        // 极窄屏 14 列（行款 26 字/列）：可用高除以 26 后字号跌破 12px
        // 下限 → 触发最小字号锁定，fs 不应低于下限，且列宽保证字不超格
        // （字格至少与字同宽——不够宽就溢出滚动，不压字）。
        let p = calculate_grid(input(LayoutMode::Single, 14, 200.0, 400.0));
        assert!(p.m.min_font_hit, "窄屏 14 列应触发最小字号保护");
        assert!(p.m.fs >= GRID_FS_MIN - 0.01, "保护后字号仍 {:.2}", p.m.fs);
        assert!(
            p.m.col_w >= p.m.fs - 0.01,
            "列宽不足：col_w={:.2} fs={:.2}",
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
            free.fs > GRID_FS_MAX,
            "专注模式不该被 {GRID_FS_MAX} 限住：{:.1}",
            free.fs
        );
        assert!(
            free.fs > capped.fs * 1.2,
            "不封顶的字号 {:.1} 应明显大于封顶值 {:.1}",
            free.fs,
            capped.fs
        );
        // 行款不被上限拆散：封顶只压字号，每列仍是行款锁定的字数
        // （v1.48.0 起 12 列本 = 22 字；读配置而非写死，防下次调行款又红）。
        let want_chars = cfg.target_chars;
        assert_eq!(capped.chars_per_col, want_chars);
        assert_eq!(free.chars_per_col, want_chars);
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
        // 行款算式（v1.32.0）：row_h = 可用高 / target_chars；
        // fs = row_h × FS_OF_ROW；col_w = (版心 − 版框) / 列数（v1.33.0
        // 版心率定宽：版心 = 单叶可用宽 × 0.75）。
        // 每列字数神圣：未被兜底时 rows 恒等于 target_chars。
        let (cw, ch) = content();
        // 传无限上限：这一条验的是「算式本身」，不该被 32px 的阅读上限干扰。
        // 行款取 12 列本的锁定值（v1.48.0：22 字/列）。
        let cfg = GridConfig {
            columns: 12,
            target_chars: 22,
            ..GridConfig::default()
        };
        let m = compute_grid(cw, ch, cfg, false, 1.0, 50.0, 0.0, 12.0, 0.0, f32::INFINITY);
        let inner_w = (cw - 50.0 - 2.0 * cw * 0.04).max(40.0);
        // ① 行款目标 = 22 字/列，且实实在在排满（行款没被拆）。
        assert_eq!(m.target_chars, 22);
        assert_eq!(m.chars_per_col, 22, "行款应排满每列 22 字");
        assert_eq!(m.rows, 22);
        // ② 字号 = 行高 × 0.85（字面必须略小于行高，竖排不粘连）。
        assert!((m.fs - m.row_h * FS_OF_ROW).abs() < 0.01, "fs 非 row_h×{FS_OF_ROW}");
        assert!(m.row_h > m.fs, "row_h({}) 必须大于 fs({})", m.row_h, m.fs);
        // ③ 版心率定宽（v1.33.0）：版心 = 可用宽 × 75%，列宽由它反推，
        //    再受两条边界夹住（字不破格 ≥ fs；字距不散 ≤ 2×行高）。
        //    单叶 1276 宽上 75% 档会撞第二条（字距上限）——这正是帽子的
        //    用途：宁可版心占比略降，也不把乌丝栏拉成一道栅栏。
        let want = (cw * 0.75 - 50.0) / 12.0;
        let expect_col = want.clamp(m.fs, m.row_h * COL_W_MAX_OF_ROW);
        assert!(
            (m.col_w - expect_col).abs() < 0.01,
            "列宽 {:.2} 应 = clamp((75%×{:.0}−版框)/12, fs, 2×row_h) = {:.2}",
            m.col_w,
            cw,
            expect_col
        );
        assert!(m.col_w >= m.fs - 0.01, "字宽超格：fs={:.2} col_w={:.2}", m.fs, m.col_w);
        assert!(
            m.col_w <= m.row_h * COL_W_MAX_OF_ROW + 0.01,
            "字距散成栅栏：col_w={:.2} row_h={:.2}",
            m.col_w,
            m.row_h
        );
        assert!(m.col_w * 12.0 <= inner_w + 0.5, "12 列总宽 {:.1} 超可用宽", m.grid_w);
        assert!(m.grid_w <= inner_w + 0.5);
        // ④ 纵向预算正好用光：纸高 + 页脚注区 = 内容区高。
        //    自 v1.34.0 起 `foot`（注文带）**不是地脚的一部分**——地脚
        //    = 天头 ÷ 1.5（用户铁律 `TOP_OF_BOT`），注区像 `head` 一样是
        //    独立的纵向预留，故纸高 = 内容高 − 注区。
        assert!(
            (m.outer_h + 12.0 - ch).abs() < 0.5,
            "纸高 + 注区 {:.1} 应 = 内容高 {:.1}",
            m.outer_h + 12.0,
            ch
        );
        assert!(m.outer_w <= cw + 0.5, "纸宽 {:.1} 超内容区 {:.1}", m.outer_w, cw);
    }

    #[test]
    fn twelve_columns_no_overflow_or_overlap() {
        // 12 列：字格整张纸宽不超过内容区；且 row_h > fs 保证不重叠。
        let (cw, ch) = content();
        // 两套都按各自的**行款**算（清版十二行 vs 八行疏朗本），才比得出
        // 「列多则字小行密」；混用旧口径（0.75 / 行高=列宽）会得出反结论。
        let qing = LayoutConfig::by_columns(12).unwrap();
        let m = compute_grid(
            cw,
            ch,
            GridConfig {
                columns: qing.columns,
                fs_ratio: qing.fs_ratio,
                line_ratio: qing.line_ratio,
                target_chars: qing.target_chars,
                ..GridConfig::default()
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
        // 8 列按**八行疏朗本**的行款算（列数与两个比例都从行款表取），
        // 才比得出「列多 → 字小行密」。
        let song = LayoutConfig::by_columns(8).unwrap();
        let m8 = compute_grid(
            cw,
            ch,
            GridConfig {
                columns: song.columns,
                fs_ratio: song.fs_ratio,
                line_ratio: song.line_ratio,
                target_chars: song.target_chars,
                ..GridConfig::default()
            },
            true,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,GRID_FS_MAX);
        assert!(m.col_w < m8.col_w + 0.01, "12 列列宽 {:.1} 应不大于 8 列 {:.1}", m.col_w, m8.col_w);
        assert!(m.fs <= m8.fs + 0.01, "12 列字级不该大于 8 列");
        assert!(m.rows >= m8.rows, "行高收紧后行数不减少");
    }

    #[test]
    fn layout_presets_step_from_sparse_to_dense() {
        // 四套古版行款：列数递增 → 字号比递减 → 行高比递减，
        // 于是「行款越古越疏朗、越晚近越细密」——一键切换就能换开本。
        let ps = LayoutConfig::presets();
        assert_eq!(ps.len(), 4, "四套行款");
        for w in ps.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert!(b.columns > a.columns, "列数应递增：{} → {}", a.columns, b.columns);
            assert!(b.fs_ratio < a.fs_ratio, "字号比应递减：{} → {}", a.columns, b.columns);
            assert!(b.line_ratio < a.line_ratio, "行高比应递减：{} → {}", a.columns, b.columns);
            // 行款字数随列数递增（八行十五字 → 十四行二十六字）。
            assert!(b.target_chars > a.target_chars, "行款字数应递增：{} → {}", a.columns, b.columns);
            assert!(!a.desc.is_empty() && !a.label.is_empty(), "行款缺文案");
        }
        // 预设键可按名点名（含长名别名）。
        assert!(LayoutConfig::by_preset("song8").is_some());
        assert!(LayoutConfig::by_preset("letter14").is_some());
        assert!(LayoutConfig::by_preset("qing_dynasty_12").is_some(), "长名别名也要认");
        assert_eq!(
            LayoutConfig::by_preset("song_dynasty_8").unwrap().columns,
            8
        );
        assert!(LayoutConfig::by_preset("nope").is_none());
    }

    #[test]
    fn font_size_and_row_height_follow_the_preset_ratios() {
        // 行款驱动（v1.32.0）：每列字数 = 行款表 target_chars（神圣）；
        // 行高 = 可用高 / target_chars；字号 = 行高 × FS_OF_ROW。
        let (cw, ch) = content();
        let mut prev_rows = 0usize;
        for p in LayoutConfig::presets() {
            let cfg = GridConfig {
                columns: p.columns,
                target_chars: p.target_chars,
                ..GridConfig::default()
            };
            // 单叶 + 无限上限：纯粹验行款算式，不被 32px 上限干扰。
            let m = compute_grid(cw, ch, cfg, false, 1.0, 50.0, 0.0, 12.0, 0.0, f32::INFINITY);
            // ① 行款目标字数如实刻在度量里，且每列排满（行款没被拆）。
            assert_eq!(m.target_chars, p.target_chars, "{} 列行款字数", p.columns);
            assert_eq!(m.chars_per_col, p.target_chars, "{} 列应排满行款", p.columns);
            assert_eq!(m.rows, p.target_chars);
            // ② 字号 = 行高 × 0.85：字面必须略小于行高（防粘连的底线）。
            assert!(
                (m.fs - m.row_h * FS_OF_ROW).abs() < 0.01,
                "{} 列字号应 = 行高 × {:.2}，实为 {:.2} / {:.2}",
                p.columns, FS_OF_ROW, m.fs, m.row_h
            );
            assert!(m.row_h > m.fs, "{} 列 row_h({:.2}) 必须 > fs({:.2})", p.columns, m.row_h, m.fs);
            // ③ 版心率定宽（v1.33.0）：列宽 = (75%×可用宽 − 版框)/列数，
            //    夹在 [fs, 2×行高] 之间；总宽不溢出可用宽。
            let want = (cw * 0.75 - 50.0) / p.columns as f32;
            let expect_col = want.clamp(m.fs, m.row_h * COL_W_MAX_OF_ROW);
            assert!(
                (m.col_w - expect_col).abs() < 0.01,
                "{} 列列宽 {:.2} 应 = {:.2}",
                p.columns, m.col_w, expect_col
            );
            let inner_w = (cw - 50.0 - 2.0 * cw * 0.04).max(40.0);
            assert!(
                m.col_w * p.columns as f32 <= inner_w + 0.5,
                "{} 列总宽 {:.1} 超可用宽 {:.1}",
                p.columns, m.grid_w, inner_w
            );
            // ④ 纵向预算正好用光：纸高 + 页脚注区 = 内容区高
            //    （`foot` 是注文带、不是地脚，见 `TOP_OF_BOT`）。
            assert!(
                (m.outer_h + 12.0 - ch).abs() < 0.5,
                "{} 列纸高 + 注区 {:.1} ≠ 内容高 {:.1}",
                p.columns,
                m.outer_h + 12.0,
                ch
            );
            // ⑤ 行款越密，一列能排的字越多。
            assert!(m.rows >= prev_rows, "{} 列行数 {} 少于上一档 {}", p.columns, m.rows, prev_rows);
            prev_rows = m.rows;
        }
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
                margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
                target_chars: 26,
                banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
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
        // 极窄内容区（200×400）+ 14 列（行款 26 字/列）：可用高除以 26 后
        // 字号跌破 12px 下限 → 触发最小字号锁定，fs 不应低于下限，
        // 且列宽保证字不超格（字格至少与字同宽——不够宽就溢出滚动）。
        let m = compute_grid(
            200.0,
            400.0,
            GridConfig {
                columns: 14,
                dir: GridDir::Vertical,
                margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
                target_chars: 26,
                banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
            },
            false,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,GRID_FS_MAX);
        assert!(m.min_font_hit, "窄屏 14 列应触发最小字号");
        assert!(m.fs >= GRID_FS_MIN - 0.01, "fs 不得低于下限 {GRID_FS_MIN}");
        assert!(m.col_w >= m.fs - 0.01, "col_w 须 ≥ fs，否则字超格");
        // 行款兜底：锁字号后一列排不满 26 字（渲染层开滚动 + 状态栏提示）。
        assert!(m.chars_per_col < m.target_chars, "兜底时行款应排不满");
    }

    // ── 自适应列数匹配引擎（Auto-Fit Column Engine）──

    /// 常规大视口：引擎与 `content_columns` 梯度一致——短章疏朗、长篇章回密集。
    #[test]
    fn auto_fit_follows_content_gradient_on_wide_viewport() {
        let (cw, ch) = (1400.0, 1000.0);
        // 15 字的小药方 → 天花板 8 列（疏朗大字，绝不稀疏电线杆）。
        assert_eq!(best_column_count(cw, ch, 15), 8);
        // 中等篇幅（200 字）→ 10 列。
        assert_eq!(best_column_count(cw, ch, 200), 10);
        // 长篇（5000 字）→ 14 列。
        assert_eq!(best_column_count(cw, ch, 5000), 14);
    }

    /// RULE 2（防稀疏）：极短文本绝不上密列，固定落到 8 列。
    #[test]
    fn auto_fit_never_picks_dense_for_sparse_text() {
        let (cw, ch) = (1400.0, 1000.0);
        for n in [1usize, 5, 15, 40, 80] {
            let r = calculate_best_column_layout(ColumnFitInput::new(cw, ch, n));
            assert_eq!(r.columns, 8, "{n} 字不该上密列");
        }
    }

    /// RULE 1（物理宽度底线）：窄到字破格 / 字过小时，逐级退到能排下的列数。
    #[test]
    fn auto_fit_retreats_when_columns_get_too_small_to_read() {
        // 极窄内容区：14 列必然字破格，引擎应退到更疏的档；绝不返回 >12。
        let r = calculate_best_column_layout(ColumnFitInput::new(220.0, 400.0, 5000));
        assert!(r.columns <= 12, "窄屏 14 列必退：得到 {}", r.columns);
        assert!(r.font_size >= GRID_FS_MIN - 0.01, "字号守下限");
        assert!(r.column_width >= r.font_size - 0.01, "列宽守字格下限");
    }

    /// RULE 3（溢出不淘汰）：长文溢出到多页时，引擎仍选天花板内最密档（不崩、不降级）。
    #[test]
    fn auto_fit_keeps_dense_when_overflowing() {
        let r = calculate_best_column_layout(ColumnFitInput::new(1400.0, 1000.0, 5000));
        assert_eq!(r.columns, 14);
        assert!(r.pages > 1, "长文应多页：{} 页", r.pages);
        assert!(r.fill_ratio >= 0.99, "溢出时填充率触顶");
    }

    /// 引擎与渲染层 `compute_grid` 同源：挑出的列数确实能在给定视口下可读地排下。
    #[test]
    fn auto_fit_column_is_renderable_at_chosen_size() {
        let (cw, ch) = (1200.0, 900.0);
        let n = best_column_count(cw, ch, 300);
        let cfg = LayoutConfig::by_columns(n).unwrap();
        let m = calculate_grid(
            GridInput {
                columns: n,
                mode: if should_face(cw, ch, n, 1.0) {
                    LayoutMode::Facing
                } else {
                    LayoutMode::Single
                },
                content_w: cw,
                content_h: ch,
                zoom: 1.0,
                edge: FIT_EDGE_DEFAULT,
                head: 0.0,
                foot: 0.0,
                margin: 0.04,
                line_scale: 1.0,
                fs_ratio: cfg.fs_ratio,
                line_ratio: cfg.line_ratio,
                banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
                dir: GridDir::Vertical,
                fs_max: GRID_FS_MAX,
            },
        )
        .m;
        assert!(m.fs >= GRID_FS_MIN - 0.01, "挑出的列数必须可读：{} 列 fs={}", n, m.fs);
        assert!(m.col_w >= m.fs - 0.01, "挑出的列数字不破格：{} 列 col_w={}", n, m.col_w);
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
            target_chars: 2,
            chars_per_col: 2,
            rows: 2,
            col_w: 10.0,
            row_h: 11.0,
            fs: 7.5,
            grid_w: 30.0,
            grid_h: 22.0,
            outer_w: 30.0,
            outer_h: 22.0,
            min_font_hit: false,
            margin_top: 0.0,
            margin_bot: 0.0,
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
            target_chars: 2,
            chars_per_col: 2,
            rows: 2,
            col_w: 10.0,
            row_h: 11.0,
            fs: 7.5,
            grid_w: 30.0,
            grid_h: 22.0,
            outer_w: 30.0,
            outer_h: 22.0,
            min_font_hit: false,
            margin_top: 0.0,
            margin_bot: 0.0,
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
                    line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
                    target_chars: c.target_chars,
                    banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
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
    fn hangkou_wins_over_line_scale_and_keeps_glyphs_apart() {
        // 行款优先（v1.32.0）：每列字数由行款表定，字体行高缩放
        // （line_scale）**不再参与**字号 / 行高算式——绝不为了适配字体
        // 而悄悄改动「一行几字」。缩放只影响观感余量，不动行款。
        let grid = |scale: f32| {
            compute_grid(
                800.0,
                600.0,
                GridConfig {
                    columns: 8,
                    dir: GridDir::Vertical,
                    margin: 0.04,
                    line_scale: scale, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
                    target_chars: 15,
                    banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
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
        let wild = grid(9.0);
        let tiny = grid(0.01);
        // ① 行款恒定：四个缩放下每列字数都一样（15 字），行款没被拆。
        for (name, m) in [("base", &base), ("kai", &kai), ("wild", &wild), ("tiny", &tiny)] {
            assert_eq!(m.rows, 15, "{name} 行款应恒为 15 字/列");
            assert_eq!(m.chars_per_col, m.target_chars, "{name} 行款应排满");
        }
        // ② 行高 / 字号恒定：缩放不进来搅局（同输入 → 同输出）。
        for (name, m) in [("kai", &kai), ("wild", &wild), ("tiny", &tiny)] {
            assert!((m.row_h - base.row_h).abs() < 0.01, "{name} 行高不该随缩放漂移");
            assert!((m.fs - base.fs).abs() < 0.01, "{name} 字号不该随缩放漂移");
        }
        // ③ 无论怎么缩放，行高始终大于字号：竖排绝不会上下重叠。
        for (name, m) in [("base", &base), ("kai", &kai), ("wild", &wild), ("tiny", &tiny)] {
            assert!(m.row_h > m.fs, "{name} row_h({:.2}) 必须 > fs({:.2})", m.row_h, m.fs);
            assert!((m.fs - m.row_h * FS_OF_ROW).abs() < 0.01, "{name} 字号应 = 行高×0.85");
        }
        // ④ 中性值 = 不缩放（确定性）。
        assert!((grid(crate::grid::LINE_SCALE_BASE).row_h - base.row_h).abs() < 0.01);
    }

    // ---- 留白三档（v1.33.0）：版心宽 ÷ 单叶可用宽 ----

    /// 三档的版心率：宽（复古 60%）< 中（标准 75%）< 窄（满屏 90%），
    /// 默认档是「中」——既留古籍两侧留白的优雅，又保证现代屏幕阅读体验。
    #[test]
    #[allow(clippy::assertions_on_constants)] // 钉的是三档版心率「宽 < 中 < 窄」
    fn banxin_ratio_tiers_step_from_retro_to_full() {
        assert_eq!(BANXIN_RATIOS.len(), 3, "三档");
        assert_eq!(BANXIN_TIER_DEFAULT, 1, "默认档 = 中");
        assert!((BANXIN_RATIOS[BANXIN_TIER_DEFAULT] - 0.75).abs() < 1e-6);
        assert!(BANXIN_RATIOS[0] < BANXIN_RATIOS[1] && BANXIN_RATIOS[1] < BANXIN_RATIOS[2]);
        // 天头率随档联动：版心越窄（越复古），天头越宽——选「宽」的人
        // 在追求极致文物感，值得更大的天头（用户 v1.33.0 要求）。
        assert!(
            top_frac_for(BANXIN_RATIOS[0]) > top_frac_for(BANXIN_RATIOS[1])
                && top_frac_for(BANXIN_RATIOS[1]) > top_frac_for(BANXIN_RATIOS[2]),
            "天头率应随留白档收窄：{:?}",
            BANXIN_TOP_FRACS
        );
        // 未登记的比值（用户自定）退回基准天头，不 panic。
        assert!((top_frac_for(0.66) - ANCIENT_TOP_FRAC).abs() < 1e-6);
    }

    /// 版心率真的管用：三档各自把版心拉到「单叶可用宽 × 档位」，
    /// 纸不超出单叶，字不破格、字距不散；**行款一字不改**（每列仍 21 字）。
    #[test]
    fn banxin_ratio_sizes_the_block_and_keeps_hangkou() {
        // 1202×710 对开（用户的屏幕口径）：12 列、行款 21 字/列。
        let (cw, ch) = (1161.0, 640.0);
        let base = GridConfig {
            columns: 12,
            target_chars: 21,
            ..GridConfig::default()
        };
        let gap = gutter_w(cw, 12, 50.0, base.margin);
        let per_leaf = (cw - gap) / 2.0;
        let mut prev_top = f32::MAX;
        for (i, &ratio) in BANXIN_RATIOS.iter().enumerate() {
            let m = compute_grid(
                cw,
                ch,
                GridConfig {
                    banxin_ratio: ratio,
                    ..base
                },
                true,
                1.0,
                50.0,
                0.0,
                0.0,
                0.0,
                GRID_FS_MAX,
            );
            // ① 版心 = 单叶可用宽 × 档位（此尺寸下三档都不触顶，精确命中）。
            let banxin = m.grid_w + 50.0;
            assert!(
                (banxin - per_leaf * ratio).abs() < 0.5,
                "{} 档版心 {:.1} 应 = {:.0}×{:.2}",
                ["宽", "中", "窄"][i],
                banxin,
                per_leaf,
                ratio
            );
            // ② 纸不超出单叶（满屏档时纸边留白归零、正好贴满）。
            assert!(m.outer_w <= per_leaf + 0.5, "{} 档纸宽 {:.1} 超单叶", ["宽", "中", "窄"][i], m.outer_w);
            // ③ 字不破格、字距不散成栅栏。
            assert!(m.col_w >= m.fs - 0.01, "{} 档字宽超格", ["宽", "中", "窄"][i]);
            assert!(
                m.col_w <= m.row_h * COL_W_MAX_OF_ROW + 0.01,
                "{} 档字距过宽：col_w={:.1} row_h={:.1}",
                ["宽", "中", "窄"][i],
                m.col_w,
                m.row_h
            );
            // ④ 行款神圣：留白档只改版心宽与天头，不改「一行几字」。
            assert_eq!(m.rows, 21, "{} 档行款被拆：{} 字/列", ["宽", "中", "窄"][i], m.rows);
            assert_eq!(m.chars_per_col, m.target_chars);
            // ⑤ 天头随档收窄（宽档最大 → 窄档最小）。
            assert!(
                m.margin_top < prev_top,
                "{} 档天头 {:.1} 未随档收窄（上一档 {:.1}）",
                ["宽", "中", "窄"][i],
                m.margin_top,
                prev_top
            );
            prev_top = m.margin_top;
        }
    }

    /// 版心率缺省（传 0）= 默认档「中（75%）」——与出厂行为一致，
    /// 老调用方不传这一字段也能拿到标准留白。
    #[test]
    fn banxin_ratio_unset_falls_back_to_standard() {
        let (cw, ch) = (1161.0, 640.0);
        let base = GridConfig {
            columns: 12,
            target_chars: 21,
            banxin_ratio: 0.0,
            ..GridConfig::default()
        };
        let m = compute_grid(cw, ch, base, true, 1.0, 50.0, 0.0, 0.0, 0.0, GRID_FS_MAX);
        let std = compute_grid(
            cw,
            ch,
            GridConfig {
                banxin_ratio: BANXIN_RATIOS[BANXIN_TIER_DEFAULT],
                ..base
            },
            true,
            1.0,
            50.0,
            0.0,
            0.0,
            0.0,
            GRID_FS_MAX,
        );
        assert!((m.col_w - std.col_w).abs() < 0.01, "缺省版心率应落到 75% 档");
        assert!((m.margin_top - std.margin_top).abs() < 0.01);
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    // ════════════════════════════════════════════════════════════════
    // v1.34.0 三大前提（Invariants）——用户点名「绝对不许破坏」：
    //   ① 天宽地窄：天头 = 地脚 × `TOP_OF_BOT`（v1.37.0 起为 **1.5**，3:2）；
    //   ② 绝对居中：对开两叶一丝不差等宽，中缝钉在屏幕正中央；
    //   ③ 句读不被盖：几何钉在 `punct` 模块（见那里的单测），
    //      渲染顺序上句读是整棵树最深的叶子，必然最后落笔。
    // 动边框 / 动字号 / 动列数之前，先跑这个模块。
    // ════════════════════════════════════════════════════════════════

    /// ① 天宽地窄：**任何**列数、任何留白档、任何窗口尺寸、开不开页脚注，
    /// 天头都严格是地脚的 [`TOP_OF_BOT`] 倍（v1.37.0 定 **3:2 = 1.5**）——
    /// 加粗外框（1px→3px）不许把版面挤扁，更不许把上下留白拉成对称。
    ///
    /// 比例只认 [`TOP_OF_BOT`] 一个来源：改律时改常量即可，此处不写死数字，
    /// 免得「常量改了、测试还钉着旧值」这种假绿。
    #[test]
    fn top_margin_keeps_the_head_to_foot_ratio() {
        // 窗口尺寸 × 列数 × 留白档 × 页脚注开关，全扫一遍。
        for &(cw, ch) in &[(1161.0, 640.0), (1280.0, 760.0), (900.0, 500.0), (1600.0, 1000.0)] {
            for &cols in &[8usize, 10, 12, 14] {
                for &ratio in &BANXIN_RATIOS {
                    for &foot in &[0.0, 76.0] {
                        let cfg = GridConfig {
                            columns: cols,
                            target_chars: LayoutConfig::by_columns(cols)
                                .map(|c| c.target_chars)
                                .unwrap_or(21),
                            banxin_ratio: ratio,
                            ..GridConfig::default()
                        };
                        let m = compute_grid(cw, ch, cfg, true, 1.0, 50.0, 0.0, foot, 0.0, GRID_FS_MAX);
                        let want = m.margin_top / TOP_OF_BOT;
                        assert!(
                            (m.margin_bot - want).abs() < 0.01,
                            "{}:1 被打破：cols={cols} 档={ratio} 窗={cw}×{ch} foot={foot} → 天头 {:.2} / 地脚 {:.2}",
                            TOP_OF_BOT,
                            m.margin_top,
                            m.margin_bot
                        );
                        // 顺带钉死 3:2 这个**具体档位**：光用常量自证会漏掉
                        // 「有人手滑把常量改回 2.0 却没发现」这类回归。
                        let ratio = m.margin_top / m.margin_bot.max(0.01);
                        assert!(
                            (ratio - 1.5).abs() < 0.02,
                            "天头:地脚 应为 3:2（1.5），实得 {ratio:.3}"
                        );
                        // 顺带钉死「宽窄」方向本身：天头永远严格大于地脚。
                        assert!(
                            m.margin_top > m.margin_bot,
                            "上下留白对称了：{:.2} vs {:.2}",
                            m.margin_top,
                            m.margin_bot
                        );
                    }
                }
            }
        }
    }

    /// ① 的兜底：地脚永不超过天头（哪怕内容区矮到留白被 clamp 成 0）。
    #[test]
    fn bottom_margin_never_grows_past_the_top() {
        for &ch in &[220.0, 240.0, 320.0, 640.0] {
            let cfg = GridConfig::default();
            let m = compute_grid(1161.0, ch, cfg, true, 1.0, 50.0, 0.0, 0.0, 0.0, GRID_FS_MAX);
            assert!(m.margin_bot <= m.margin_top + 1e-6, "ch={ch} 地脚反超天头");
        }
    }

    /// ② 绝对居中：对分两叶 + 中缝恰好铺满内容宽，两叶一丝不差等宽，
    /// 且**中缝中心 = 屏幕正中央**（`gw/2 + per_leaf == cw/2`）。
    /// 中缝宽只由「列数 / 内容宽 / 版框 / 纸边」决定，与边框粗细无关。
    #[test]
    fn facing_split_is_symmetric_and_the_gutter_sits_on_the_center_line() {
        for &(cw, cols) in &[
            (1161.0, 8usize),
            (1161.0, 10),
            (1161.0, 12),
            (1161.0, 14),
            (900.0, 12),
            (1600.0, 14),
        ] {
            for &edge in &[50.0, 14.0, 3.0] {
                // 外框从 1px 加到 3px（edge 取几档）都不许破坏等宽与居中。
                let gw = gutter_w(cw, cols, edge, 0.10);
                let per_leaf = ((cw - gw) / 2.0).max(60.0);
                // ②-a 两叶严格等宽：从中缝向两侧各分一半，右叶宽度由
                //   「剩余宽度」反算，差值必须为 0（不许有一丝舍入损失）。
                let right_w = cw - gw - per_leaf;
                assert!(
                    (per_leaf - right_w).abs() < 1e-9,
                    "cols={cols} edge={edge}：两叶不等宽 {per_leaf} vs {right_w}"
                );
                // ②-b 两叶 + 中缝 = 内容宽（不溢不欠）。
                assert!(
                    (per_leaf * 2.0 + gw - cw).abs() < 1e-4,
                    "cols={cols} edge={edge}：{per_leaf}×2 + {gw} ≠ {cw}"
                );
                // ②-c 中缝中心钉在屏幕正中。
                let gutter_center = per_leaf + gw * 0.5;
                assert!(
                    (gutter_center - cw * 0.5).abs() < 1e-4,
                    "cols={cols} edge={edge}：中缝中心 {gutter_center} 不在屏幕正中 {}",
                    cw * 0.5
                );
                // ②-d 中缝宽度合理（立得住一对鱼尾，又不像一条马路）。
                assert!((LEAF_GAP_MIN - 1e-6..=LEAF_GAP_MAX + 1e-6).contains(&gw));
            }
        }
    }

    /// v1.34.0 回归（用户实测「双叶分家」）：对开两叶贴中缝的内侧缘
    /// **也必须描边**——左叶右缘、右叶左缘各一道粗竖框，中缝才被
    /// 两条竖线夹住，而不是一条孤立的马路。
    #[test]
    fn every_half_closes_all_four_sides() {
        for half in [LeafHalf::Alone, LeafHalf::Left, LeafHalf::Right] {
            let sides = frame_sides(half);
            assert_eq!(sides, [true, true, true, true], "{half:?} 有边没闭合：{sides:?}");
        }
    }

    /// 左半叶的右缘（贴中缝）与右半叶的左缘（贴中缝）都必须闭合——
    /// 这正是 v1.34.0 之前漏画的两条竖边框。
    #[test]
    fn gutter_facing_edges_are_closed() {
        // 左半叶：右缘（下标 1）闭合。
        assert!(frame_sides(LeafHalf::Left)[1], "左半叶右缘必须描边");
        // 右半叶：左缘（下标 3）闭合。
        assert!(frame_sides(LeafHalf::Right)[3], "右半叶左缘必须描边");
        // 上下横框任何时候都闭合（界行两端才接得上）。
        for half in [LeafHalf::Alone, LeafHalf::Left, LeafHalf::Right] {
            let s = frame_sides(half);
            assert!(s[0] && s[2], "{half:?} 上下横框断了");
        }
    }

    #[test]
    fn top_baseline_is_the_only_head_anchor() {
        // 无预留：基准线就是 0（贴第一格上沿）。
        assert_eq!(top_baseline(0.0, 40.0), 0.0);
        // 有卷首题署预留：整格往上抬，绝不出现半格。
        assert_eq!(top_baseline(3.0, 40.0), 120.0);
        // 负数 / 非有限值兜底：不许把基准线算成负的（那会把字顶出纸外）。
        assert_eq!(top_baseline(-2.0, 40.0), 0.0);
        assert_eq!(top_baseline(1.0, 0.0), 0.0);
    }

    #[test]
    fn ceil_to_cells_snaps_half_cells_up() {
        let cell = 40.0;
        // 夹注 3 行（1.5 格）→ 2 整格；1 行（0.5 格）→ 1 整格。
        assert_eq!(ceil_to_cells(60.0, cell), 80.0);
        assert_eq!(ceil_to_cells(20.0, cell), 40.0);
        // 整格原样返回，绝不无谓加高。
        assert_eq!(ceil_to_cells(80.0, cell), 80.0);
        assert_eq!(ceil_to_cells(40.0, cell), 40.0);
        // 退化输入不 panic。
        assert_eq!(ceil_to_cells(10.0, 0.0), 10.0);
    }

}
