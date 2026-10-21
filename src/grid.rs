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

/// 版框样式。渲染层只按这个值分支，不写死「信笺才文武」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BorderStyle {
    /// 单线：一根细框，普通古籍竖笺。
    Single,
    /// 文武边栏：外粗内细，信笺 / 讲究的刻本用。
    Double,
}

impl BorderStyle {
    /// 边框样式标签（UI / 调试用）。
    pub fn label(self) -> &'static str {
        match self {
            BorderStyle::Single => "单线",
            BorderStyle::Double => "文武",
        }
    }
}

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
    /// 版框样式：单线（standard）/ 文武（信笺）。
    pub border_style: BorderStyle,
    /// 是否在版心（中缝）绘制上下相对的「对鱼尾」。
    pub show_fish_tail: bool,
}

/// 版式配置注册表：有序数组即「单一数据源」。新增版式（如 16 列）只需在此
/// 追加一行，UI / 渲染 / 几何自动适配——零 if-else 扩散。
pub const LAYOUT_CONFIGS: &[LayoutConfig] = &[
    LayoutConfig { key: "standard8",  label: "标准 8 列",  columns: 8,  dir: GridDir::Vertical, margin: 0.04, border_style: BorderStyle::Single, show_fish_tail: false },
    LayoutConfig { key: "standard10", label: "标准 10 列", columns: 10, dir: GridDir::Vertical, margin: 0.04, border_style: BorderStyle::Single, show_fish_tail: false },
    LayoutConfig { key: "standard12", label: "标准 12 列", columns: 12, dir: GridDir::Vertical, margin: 0.04, border_style: BorderStyle::Single, show_fish_tail: false },
    // 信笺：四周文武边栏 + 中缝对鱼尾，独占 2:3 平铺稿纸几何（见 `fit_letter`）。
    LayoutConfig { key: "letter14",   label: "信笺 14 列", columns: 14, dir: GridDir::Vertical, margin: 0.04, border_style: BorderStyle::Double, show_fish_tail: true },
];

impl LayoutConfig {
    /// 按配置键取配置；未知键回落默认。
    pub fn by_key(key: &str) -> Option<&'static LayoutConfig> {
        LAYOUT_CONFIGS.iter().find(|c| c.key == key)
    }
    /// 默认版式（首启 / 配置损坏时的回落）。
    pub fn default() -> &'static LayoutConfig {
        Self::by_key("standard10").unwrap_or(&LAYOUT_CONFIGS[1])
    }
    /// 默认配置键。
    pub fn default_key() -> &'static str {
        Self::default().key
    }
    /// 是否「信笺」模板：独占 2:3 平铺几何与对鱼尾，与普通对开 / 单页是两条路径。
    pub fn is_letter(&self) -> bool {
        self.key == "letter14"
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
            columns: 8,
            dir: GridDir::Vertical,
            margin: 0.04, line_scale: 1.0,
        }
    }
}

/// 字号 / 行高算式系数与上下限（均来自用户给定的公式与边界要求）。
pub const GRID_FS_RATIO: f32 = 0.75; // fontSize = colWidth × 0.75
pub const GRID_ROW_RATIO: f32 = 1.1; // rowHeight = colWidth × 1.1
/// 最小字号保护阈值（用户指定 12px）：窄屏 12/14 列也不至于小到看不清；
/// 触发后几何按「字号优先」重算，可能超出可用宽 → 由渲染层开启单页
/// 上下（及左右）滚动，绝不把字挤成一团黑疙瘩。
pub const GRID_FS_MIN: f32 = 12.0;
/// 字号上限：专注模式撑满整屏时不被无限制放大。
pub const GRID_FS_MAX: f32 = 220.0;
/// 行高缩放的中性值：不放大也不压缩（旧存档 / 未指定字体时取它）。
pub const LINE_SCALE_BASE: f32 = 1.0;
/// 行高缩放的合理区间（夹住异常值，免得行气散掉或字压成一团）。
pub const LINE_SCALE_MIN: f32 = 0.85;
pub const LINE_SCALE_MAX: f32 = 1.6;

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
    // 5) 字号 = 0.75 × 列宽；下限保护窄屏小字。
    let want = col_w_raw * GRID_FS_RATIO * zoom;
    let min_hit = want < GRID_FS_MIN;
    let fs = want.clamp(GRID_FS_MIN, GRID_FS_MAX);
    // 触发下限时，反推列宽使字号不小于下限（宁可超出可用宽 → 渲染层滚动），
    // 避免字比字格还宽而横向重叠。
    let col_w = if min_hit { fs / GRID_FS_RATIO } else { col_w_raw };
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

    #[test]
    fn formulas_match_spec() {
        // 竖排 8 列：col_w = 可用宽/8，fs = 0.75*col_w，row_h = 1.1*col_w。
        // extra_cols = 0：章题列在格内，不占额外宽。
        let (cw, ch) = content();
        let m = compute_grid(cw, ch, GridConfig::default(), false, 1.0, 50.0, 0.0, 12.0, 0.0);
        let inner_w = (cw - 50.0 - 2.0 * cw * 0.04).max(40.0);
        let expect_col = inner_w / 8.0;
        assert!((m.col_w - expect_col).abs() < 0.01, "col_w {:.2} ≠ {:.2}", m.col_w, expect_col);
        assert!((m.fs - expect_col * 0.75).abs() < 0.01, "fs 非 0.75*col_w");
        assert!((m.row_h - expect_col * 1.1).abs() < 0.01, "row_h 非 1.1*col_w");
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
            false,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,
        );
        assert!(m.outer_w <= cw + 0.5, "12 列纸宽 {:.1} 超内容区 {:.1}", m.outer_w, cw);
        assert!(m.outer_h <= ch + 0.5, "12 列纸高 {:.1} 超内容区 {:.1}", m.outer_h, ch);
        assert!(m.row_h > m.fs, "12 列 row_h 必须 > fs");
        // 列越多字越小，12 列 fs < 8 列 fs。
        let m8 = compute_grid(cw, ch, GridConfig::default(), false, 1.0, 50.0, 0.0, 12.0, 0.0);
        assert!(m.fs < m8.fs, "12 列应比 8 列字小");
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
            0.0,
        );
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
            0.0,
        );
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
        // 注册表恰好覆盖用户指定的四个档位，键 / 标签 / 列数一一对应。
        assert_eq!(LAYOUT_CONFIGS.len(), 4);
        let keys: Vec<&str> = LAYOUT_CONFIGS.iter().map(|c| c.key).collect();
        assert_eq!(keys, vec!["standard8", "standard10", "standard12", "letter14"]);
        // 标准三档是单线、无鱼尾；信笺是文武边栏 + 对鱼尾。
        for c in LAYOUT_CONFIGS.iter().filter(|c| !c.is_letter()) {
            assert_eq!(c.border_style, BorderStyle::Single, "{} 应为单线", c.key);
            assert!(!c.show_fish_tail, "{} 不应有鱼尾", c.key);
        }
        let letter = LayoutConfig::by_key("letter14").unwrap();
        assert_eq!(letter.border_style, BorderStyle::Double);
        assert!(letter.show_fish_tail);
        assert_eq!(letter.columns, 14);
        // 默认档是 standard10。
        assert_eq!(LayoutConfig::default().key, "standard10");
        // 注册表是精确查表：未知键查不到（`None`），回落由调用方
        // （`current_layout` / `set_layout`）统一处理，从而 `set_layout`
        // 能在解析失败时安全返回，不污染状态。
        assert!(LayoutConfig::by_key("nope").is_none());
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
                0.0,
            );
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
                0.0,
            )
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
