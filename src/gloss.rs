//! 双行夹注排版模块（古籍竖笺）。
//!
//! 一列古籍同时排正文与小字夹注，规则照刻本：
//!   * 计量单位是「半格」：正文一字占 2 格；夹注排成左右两个半宽小列
//!     （先右列后左列，合竖排读序），每小行占 1 格，字号取正文一半；
//!   * 一列容量 `rows*2` 格（`rows` 是网格引擎算出的每面行数）；
//!   * 注文过长、本列放不下时**自动跨列流转**：注尾接着排在下一列列首，
//!     正文随后跟上；一面排满时注尾随下一面首列继续；
//!   * 章首另起一面，章首页少一列正文（第 `cols` 列是章题列）。
//!
//! 分页（`Root::paginate`）与渲染切列（阅读器）都走本模块的
//! `layout_book`：一次排出「面 → 列 → 片」整棵树，两边结构同源，
//! 不会再出现「分页算得下、渲染放不下」的丢字 / 错位。

use crate::types::Glyph;

/// 一列里的一段内容：一个正文字，或一段双行小字夹注（跨列流转时是注尾）。
#[derive(Clone, Debug)]
pub enum Piece {
    Main(Glyph),
    Gloss(String),
}

/// 一面（一叶）的排版结果。
#[derive(Clone, Debug)]
pub struct Page {
    /// 本面正文（翻页导航 / 章首判定 / 页脚注用）。
    pub glyphs: Vec<Glyph>,
    /// 本面是否某章的第一面（要排章题列）。
    pub start_chapter: bool,
    /// 切好的列：每列自上而下是正文与夹注片段的序列；
    /// 跨列流转的注尾排在下一列列首。
    pub columns: Vec<Vec<Piece>>,
}

/// 列填充状态机：只记「半格」账。
///
/// `push_main` / `push_note` 的返回值都是「本列放不放得下」的信号，
/// 调用方据此开新列、计列数（一面排满就收面）。
#[derive(Clone, Copy, Debug)]
pub struct Filler {
    cap: usize,
    units: usize,
}

impl Filler {
    /// `cap` = 一列的半格容量（`rows*2`）。
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(2),
            units: 0,
        }
    }

    /// 剩余半格数。
    pub fn room(&self) -> usize {
        self.cap - self.units
    }

    /// 放入一个正文格（2 格）。当前列放不下时先弃旧列、开新列，
    /// 返回 `true` 表示此字另起一列。
    pub fn push_main(&mut self) -> bool {
        let roll = self.units + 2 > self.cap && self.units > 0;
        if roll {
            self.units = 0;
        }
        self.units += 2;
        roll
    }

    /// 放入一段 `chars` 个注文字的夹注。返回本列能容纳的注文字数
    /// （每小行 2 字占 1 格）；放不下的部分由调用方流转到下一列。
    pub fn push_note(&mut self, chars: usize) -> usize {
        if chars == 0 {
            return 0;
        }
        let want = (chars + 1) / 2;
        let room = self.room();
        if want <= room {
            self.units += want;
            chars
        } else {
            self.units = self.cap;
            (room * 2).min(chars)
        }
    }

    /// 开新列（账目清零）。不一定由 `push_main` 触发——夹注跨列也会用到。
    pub fn begin_column(&mut self) {
        self.units = 0;
    }

    /// 本列是否已满。
    pub fn is_full(&self) -> bool {
        self.units >= self.cap
    }

    /// 预留顶部若干「行」给卷首题署（书名·卷次 + 署名）独占，
    /// 正文从预留带之下接着排。只扣「半格」账，不改列容量；
    /// 调用方已把预留行数收敛到 ≤ 列容量的一半以内，不会把整列吃空。
    pub fn reserve_rows(&mut self, rows: usize) {
        self.units = (self.units + rows * 2).min(self.cap);
    }
}

/// 卷首题署预留行数：把「书名·卷次 + 1 行间隔 + 署名」的字符数收敛成
/// 列内预留行数，上限受每列行数（`rows`）与可读上限（14 行）双重约束。
///
/// 渲染层据此在章首页开卷列顶部叠一卷端题名；正文从预留带之下接着排。
/// 若章名 + 署名过长超出上限，渲染层按预留带裁切（不会顶穿版框）。
pub fn inscription_reserve(need: usize, rows: usize) -> usize {
    if rows <= 2 {
        return 0;
    }
    need.min(rows - 2).min(14)
}

/// 把整卷字形流排成一面面叶子：分页（章首另起、每面列数上限）与
/// 切列（正文 + 夹注跨列流转）一次完成。
///
/// 规则（与渲染侧严格一致）：
///   * 按「半格」计量：正文一字占 2 格、夹注每行小字占 1 格；
///   * 夹注本列放不下 → 余下部分排下一列**列首**（注尾在前、正文随后）；
///   * 本面列尽 → 收面，注尾随下一面首列继续；
///   * 新章另起一面，且章首页只排 `cols-1` 列正文（第 `cols` 列是章题列）。
pub fn layout_book(
    flow: &[Glyph],
    gloss: impl Fn(&Glyph) -> Option<String>,
    cols: usize,
    rows: usize,
    // 章首页开卷列顶部预留给卷首题署的行数（0 = 不预留，按普通列排）。
    reserve_rows: usize,
) -> Vec<Page> {
    let col_units = rows * 2;
    let mut pages: Vec<Page> = Vec::new();
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut columns: Vec<Vec<Piece>> = vec![Vec::new()];
    let mut filler = Filler::new(col_units);
    let mut cols_filled = 0usize;
    // 当前面的正文列上限——它是「面」的属性：章首页 `cols-1`（章题列），
    // 续面 `cols`。不能按当前字算：章首页最后一列边界落在普通字上时
    // 才取 limit 会退化成 `cols` 列，多出的字不会被渲染。
    let mut page_limit = cols.saturating_sub(1).max(1);
    let mut start_chapter = true;

    // 收面：并入手上的列，重开一面。手里没有正文时只重置列状态、
    // 不产出空面（夹注注尾恰好跨到章首那种极端情况）。
    macro_rules! seal {
        () => {{
            if !glyphs.is_empty() {
                pages.push(Page {
                    glyphs: std::mem::take(&mut glyphs),
                    start_chapter,
                    columns: std::mem::take(&mut columns),
                });
                columns = vec![Vec::new()];
            }
            filler.begin_column();
            cols_filled = 0;
        }};
    }

    for (i, g) in flow.iter().enumerate() {
        let chapter_start = i == 0 || flow[i - 1].chapter != g.chapter;
        if chapter_start {
            if !glyphs.is_empty() {
                seal!();
            }
            page_limit = cols.saturating_sub(1).max(1);
            start_chapter = true;
        }
        // 卷首题署：章首页的「开卷列」（正文第一列）顶部留出一段空白带，
        // 渲染层在那里叠一卷端题名（书名·卷次 + 署名），正文从带下接着排。
        // 仅章首页（start_chapter 为真）的开卷列（cols_filled==0）预留一次。
        if start_chapter && cols_filled == 0 && reserve_rows > 0 {
            filler.reserve_rows(reserve_rows);
        }
        if filler.push_main() {
            // 此字另起一列。**先判面尽、后开新列**——顺序反了会把刚开好的
            // 空列封进上一面（章首页多出一列的 bug 就这么来的）。
            cols_filled += 1;
            if cols_filled >= page_limit {
                seal!();
                page_limit = cols;
                start_chapter = false;
            } else if columns.last().map(|c| !c.is_empty()).unwrap_or(false) {
                columns.push(Vec::new());
            }
        }
        columns.last_mut().unwrap().push(Piece::Main(*g));
        glyphs.push(*g);
        // 夹注：本列能放几行放几行，放不下的排下一列列首。
        let Some(txt) = gloss(g) else { continue };
        let all: Vec<char> = txt.chars().collect();
        let mut rest: &[char] = &all;
        while !rest.is_empty() {
            let take = filler.push_note(rest.len());
            if take > 0 {
                let piece: String = rest[..take].iter().collect();
                columns.last_mut().unwrap().push(Piece::Gloss(piece));
                rest = &rest[take..];
            }
            if !rest.is_empty() {
                cols_filled += 1;
                if cols_filled >= page_limit {
                    // 本面列尽（含注文列占的预算）：收面，注尾进下一面首列。
                    // 判据不能只靠「有正文」——注尾收面时手上必有宿主字，
                    // 但列里可能只剩注；columns 非空也要收。
                    seal!();
                    page_limit = cols;
                    start_chapter = false;
                } else if columns.last().map(|c| !c.is_empty()).unwrap_or(false) {
                    columns.push(Vec::new());
                }
                filler.begin_column();
            }
        }
    }
    if !glyphs.is_empty()
        || !columns.iter().all(|c| c.is_empty())
        || pages.is_empty()
    {
        pages.push(Page {
            glyphs,
            start_chapter,
            columns,
        });
    }
    pages
}

/// 双行夹注块的左侧留白比（占宿主字格宽的比例）。
///
/// 刻本里的夹注小字嵌在正文列里、不顶满列宽：左缘留一道空白，
/// 双小列偏右安居，视觉上才「夹」在正文之间，而不是与界行齐平。
/// 1/6 ≈ 两道小列各让半分——右侧仍贴列右缘，读序不受影响。
pub const NOTE_PAD_RATIO: f32 = 1.0 / 6.0;

/// 一列内所有夹注块的纸面坐标。
///
/// `li` 是该列在 flex_row 里**从左数**的位置（左缘 `li*cell_w`）：
/// 竖排正文列从左数是反的——渲染侧 `i`（读序列号，0 在最右）的
/// `li = text_cols - 1 - i`。
/// 列内高度按「正文每字 `cell_h`、夹注块 `ceil(n/2) * cell_h/2`」
/// 自上而下累计（与分页同源的半格账）。
/// bbox 同步左侧留白（见 `NOTE_PAD_RATIO`）：命中区只覆盖真正
/// 排了小字的部分，空白纸面不误触。
pub fn note_boxes_in_column(col: &[Piece], li: usize, cell_w: f32, cell_h: f32) -> Vec<NoteBox> {
    let row_h = cell_h / 2.0;
    let pad = cell_w * NOTE_PAD_RATIO;
    let mut y = 0.0f32;
    let mut out = Vec::new();
    for p in col {
        match p {
            Piece::Main(_) => y += cell_h,
            Piece::Gloss(text) => {
                let rows = (text.chars().count() + 1) / 2;
                let h = rows as f32 * row_h;
                out.push(NoteBox {
                    x: li as f32 * cell_w + pad,
                    y,
                    w: cell_w - pad,
                    h,
                    text: text.clone(),
                    page: 0,
                    back: false,
                });
                y += h;
            }
        }
    }
    out
}

/// 一叶纸在窗口中的原点（左上角，像素）。
///
/// 与 `render_reading` 的舞台排布一致：
///   * 单页：水平居中，垂直方向在正文区（工具栏 44 + 状态栏 34 之下、
///     四边留白 44 之中）居中；
///   * 对开（横屏）：两叶并排、中间隔 `LEAF_GAP`，整体居中——
///     `back`（下一面）在左、当前面在右，读序自右向左；
///   * 专注模式：工具栏 / 状态栏 / 留白皆 0，纸面贴屏。
/// `paper_w` / `paper_h` 是纸面尺寸（网格引擎 `outer_w` / `outer_h`）。
pub fn leaf_origin(
    facing: bool,
    viewport_w: f32,
    viewport_h: f32,
    paper_w: f32,
    paper_h: f32,
    back: bool,
    focus: bool,
) -> (f32, f32) {
    // 正文区高度 = 视口 - 工具栏(44) - 状态栏(34)；叶在其中垂直居中。
    let chrome_h = if focus { 0.0 } else { 44.0 + 34.0 };
    let stage_w = if facing {
        paper_w * 2.0 + crate::grid::LEAF_GAP
    } else {
        paper_w
    };
    let x = if facing {
        let left = ((viewport_w - stage_w) / 2.0).max(0.0);
        if back {
            left
        } else {
            left + paper_w + crate::grid::LEAF_GAP
        }
    } else {
        ((viewport_w - stage_w) / 2.0).max(0.0)
    };
    let body_h = viewport_h - chrome_h;
    let y = if focus {
        0.0
    } else {
        chrome_h + ((body_h - paper_h) / 2.0).max(0.0)
    };
    (x, y)
}

/// 纸面上一个注文块的包围盒（版框内坐标系：原点 = 内框左上）。
///
/// 与渲染严格同源：列左缘与列内高度都用同一套网格量（列宽 `cell_w`、
/// 行高 `cell_h`）推算，因此 bbox 与实际渲染矩形一致，可直接用于
/// 命中测试与定位（点击注文呼出放大卡片，见 reader.rs）。
#[derive(Clone, Debug)]
pub struct NoteBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 本块注文文本（跨列流转时是整条注文的一段）。
    pub text: String,
    /// 所属面（叶）序号，0 基。
    pub page: usize,
    /// 是否对开里的左叶（屏幕左侧那叶）。
    pub back: bool,
}

/// 点注文弹出的放大卡片（悬浮窗）状态。
#[derive(Clone, Debug)]
pub struct NoteZoom {
    /// 被放大的注文文本。
    pub text: String,
    /// 卡片锚点（点击处的窗口坐标）。
    pub x: f32,
    pub y: f32,
    /// 注文所在的叶（0 基，卡片「来源」行用）。
    pub page: usize,
}

// ------------------------------------------------------------------
// 双行夹注（双列小字）· 标记解析与 Z 字型填充
// ------------------------------------------------------------------

/// 带标记文本的一段：正文或注文。注文由 `[注]…[/注]` 包裹。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Text(String),
    Note(String),
}

/// 解析 `[注]…[/注]` 标记。
///
/// 规则：`[注]` 与 `[/注]` 之间的文本是一条注文；其余为正文。
/// 未闭合的 `[注]` 到文本末尾视为一条注文（容错，不丢字）。
pub fn parse_annotated(s: &str) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let mut buf = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("[注]") {
            if !buf.is_empty() {
                out.push(Seg::Text(std::mem::take(&mut buf)));
            }
            // 找到闭合标签；没有就吞到结尾。
            let (note, after) = match tail.find("[/注]") {
                Some(i) => (tail[..i].to_string(), &tail[i + "[/注]".len()..]),
                None => (tail.to_string(), ""),
            };
            out.push(Seg::Note(note));
            rest = after;
        } else {
            // 吃到下一个 `[注]` 或结尾。按字符推进，`[` 开头但不成标签的
            // 是普通正文（古籍方括号句读如「〔 」」不在本标记体系内）。
            let ch = rest.chars().next().unwrap();
            buf.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    if !buf.is_empty() {
        out.push(Seg::Text(buf));
    }
    out
}

/// Z 字型布局里的一个字（正文大字或注文小字）。
#[derive(Clone, Debug, PartialEq)]
pub struct ZCell {
    pub ch: char,
    /// 格内 x（列内坐标；竖排一列 x 恒 0，注文右小列为 col_w/2）。
    pub x: f32,
    /// 格内 y（自注文宿主字的行首起算，含此前各行累计）。
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub fs: f32,
    /// 是否注文小字。
    pub note: bool,
    /// 在整段文本里的出现序（打印 / 调试用）。
    pub seq: usize,
}

/// 双行夹注的布局（纯函数，坐标可打印、可断言）。
///
/// 规格（与用户约定一致）：
///   * 正文格宽 = `col_w`、高 = `row_h`（`row_h = 1.1×col_w`，末行含行间距）；
///   * 遇注文（`[注]…[/注]`）时，宿主字的 `col_w` **一分为二**：
///     左小列宽 `col_w/2`、右小列宽 `col_w/2`；
///   * 注文字号 = 正文的 50%（`fs*0.5`）；
///   * **竖排传统读序「右先左后」**：注文按列优先填充——前 `ceil(n/2)`
///     个字自上而下排**右小列**，剩余字接排**左小列**（右列读在前）；
///   * **行高对齐**：小行高 = `row_h/2`，两个上下小字的总高**严格等于**
///     一行正文的大字高加行间距（`2 × (row_h/2) == row_h`），不错位。
///
/// 竖排单列：正文与左小列 x 均为 0，右小列 x = `col_w/2`；
/// y 游标自 0 起，正文每字推 `row_h`，注文块推 `ceil(n/2) × row_h/2`。
pub fn z_layout(text: &str, col_w: f32, row_h: f32, fs: f32) -> Vec<ZCell> {
    let sub_w = col_w / 2.0;
    let sub_row = row_h / 2.0;
    let nfs = fs * 0.5;
    let mut out: Vec<ZCell> = Vec::new();
    let mut y = 0.0f32; // 列内游标
    let mut seq = 0usize;
    for seg in parse_annotated(text) {
        match seg {
            Seg::Text(t) => {
                for ch in t.chars() {
                    out.push(ZCell {
                        ch,
                        x: 0.0,
                        y,
                        w: col_w,
                        h: row_h,
                        fs,
                        note: false,
                        seq,
                    });
                    y += row_h;
                    seq += 1;
                }
            }
            Seg::Note(t) => {
                let chars: Vec<char> = t.chars().collect();
                let half = (chars.len() + 1) / 2; // 右小列（读在前）的字数
                let anchor = y; // 注块紧跟宿主字之后起排
                for (i, ch) in chars.iter().enumerate() {
                    let right = i < half; // 竖排右先左后
                    out.push(ZCell {
                        ch: *ch,
                        // 右小列 x 靠右（次列宽处）；左小列贴格左缘。
                        x: if right { sub_w } else { 0.0 },
                        y: anchor + (if right { i } else { i - half }) as f32 * sub_row,
                        w: sub_w,
                        h: sub_row,
                        fs: nfs,
                        note: true,
                        seq,
                    });
                    seq += 1;
                }
                // 块高 = half 个小行（两列各 half 行；奇数时左列末行留空）。
                y += half as f32 * sub_row;
            }
        }
    }
    out
}

// ------------------------------------------------------------------
// 双行夹注 · 溢出与流转（同页流转 + 跨页 pendingNote）
// ------------------------------------------------------------------

/// 一列里的一个「半格行」：一个正文字（占满格宽、2 半格），
/// 或一对注文小字（左 / 右各半宽、1 半格）。
#[derive(Clone, Debug, PartialEq)]
pub enum RowSlot {
    Main(char),
    /// 竖排右先左后：任一列可为 None（块尾奇数时左列少一字）。
    Note(Option<char>, Option<char>),
}

/// 流转引擎排出的一个字（页内绝对坐标）。
#[derive(Clone, Debug, PartialEq)]
pub struct ZFlowCell {
    pub ch: char,
    /// 页内 x（列偏移 + 小列偏移；竖排右小列 x = col*col_w + col_w/2）。
    pub x: f32,
    /// 列内 y（半格累计，渲染层加列在页内的纵向位置）。
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub fs: f32,
    /// 是否注文小字。
    pub note: bool,
    /// 第几面（叶），0 基。
    pub page: usize,
    /// 面内列序（DOM 从左数；渲染层负责竖排右→左的镜像）。
    pub col: usize,
    /// 注文小列：0 = 左小列，1 = 右小列；正文为 0。
    pub sub: u8,
    /// 该注字在整条注文块内的序号（跨列 / 跨页连续，Z 读序的依据）。
    pub note_idx: usize,
}

/// 未排完的注文缓存（跨页 / 跨叶）：剩余字符 + 块内全局序号 + 右列字数。
#[derive(Clone, Debug, PartialEq)]
pub struct PendingNote {
    pub rest: Vec<char>,
    /// 下一个待排注字的块内全局序号（右先左后：<half 进右小列）。
    pub next_idx: usize,
    pub half: usize,
}

/// 双行夹注流转引擎（纯函数，坐标可打印、可断言）。
///
/// 规格（与用户约定一致）：
///   1. **列底不截断**：注文排到列的底部（`rows*2` 半格用完）还没结束，
///      绝不截断；
///   2. **同页流转**：剩余注文流到本面下一列，该列是 **note_only 列**
///      （只排小字、不再排正文），继续双小列右先左后排；
///   3. **跨页流转**：本面所有列（正文列 + 注文列）都排满后仍有剩余，
///      余下的注文进 `pending_note` 缓存；新的一面（含对开的左半叶）
///      首列**优先**排缓存注文，排完才排正文；
///   4. **主进度指针 `ci`**：单一原文字符游标（单调前进），标记
///      `[注]` / `[/注]` 只切换状态、不产字；正文与注文都从 `ci` 取字，
///      交替读取时游标绝不回退、不错位。
///
/// 竖排读序「右先左后」：块内前 `half = ceil(n/2)` 字进右小列、剩余进
/// 左小列；跨列 / 跨页后由 `note_idx` 续推，读序保持连续。
pub fn layout_z_flow(text: &str, cols: usize, rows: usize, col_w: f32) -> Vec<ZFlowCell> {
    let mut b = ZFlowBuilder::new(cols, rows, col_w);
    b.run(text)
}

/// 分页版：返回逐面的字表（`Vec<页, Vec<字>>`），供控制台验证按面打印。
pub fn layout_z_flow_pages(
    text: &str,
    cols: usize,
    rows: usize,
    col_w: f32,
) -> Vec<Vec<ZFlowCell>> {
    let mut b = ZFlowBuilder::new(cols, rows, col_w);
    b.run(text);
    b.pages
}

/// 主进度指针终值（验证用）：跑一遍流转引擎，返回 `ci` 消费的原文字符
/// 数——应恒等于去标记文本的全字计数（正文与注文交替也不乱、不回退）。
pub fn layout_z_flow_ci(text: &str, cols: usize, rows: usize, col_w: f32) -> usize {
    let mut b = ZFlowBuilder::new(cols, rows, col_w);
    b.run(text);
    b.ci_consumed()
}

/// 流转引擎状态机：把「正文 + 夹注」排成一面面叶子。
struct ZFlowBuilder {
    cols: usize,
    cap: usize,
    col_w: f32,
    row_h: f32,
    fs: f32,
    nfs: f32,
    sub_w: f32,
    sub_row: f32,
    /// 第几面（0 基）。
    page_no: usize,
    /// 面内当前列序（DOM 从左数；渲染层负责竖排右→左的镜像）。
    col: usize,
    /// 当前列已用半格。
    col_used: usize,
    /// 当前列已排高度。
    col_y: f32,
    /// 当前列的半格行栈。
    col_slots: Vec<RowSlot>,
    /// 当前列是否纯注文列（同页流转列：只排小字，不再排正文）。
    note_only: bool,
    /// 新面首列是否尚未动（pendingNote 优先权的判据）。
    page_fresh: bool,
    /// 跨页 / 跨叶缓存：本面排不下的注文，新面首列优先续排。
    pending: Option<PendingNote>,
    cells: Vec<ZFlowCell>,
    pages: Vec<Vec<ZFlowCell>>,
    /// 主进度指针：原文字符游标（单调前进，正文与注文都从它取字）。
    ci: usize,
}

impl ZFlowBuilder {
    fn new(cols: usize, rows: usize, col_w: f32) -> Self {
        let row_h = 1.1 * col_w;
        let fs = 0.75 * col_w;
        let cap = (rows * 2).max(2); // 极端参数下限 2 半格
        Self {
            cols: cols.max(1),
            cap,
            col_w,
            row_h,
            fs,
            nfs: fs * 0.5,
            sub_w: col_w / 2.0,
            sub_row: row_h / 2.0,
            page_no: 0,
            col: 0,
            col_used: 0,
            col_y: 0.0,
            col_slots: Vec::new(),
            note_only: false,
            page_fresh: true,
            pending: None,
            cells: Vec::new(),
            pages: Vec::new(),
            ci: 0,
        }
    }

    /// 主循环：正文与注文交替读取。
    ///
    /// 结构：
    ///   ① 逐 Seg（预拆 `parse_annotated`，整块注文长度已知 → 右列字数
    ///      `half` 可预先算出）；
    ///   ② Seg 内逐字符；每个动作前先 `flush_pending`（新面首列优先还注）；
    ///   ③ 正文字占 2 半格；注文字占 1 半格、右先左后并入半格行；
    ///   ④ 本面列尽 → 注文余文入 `pending`，立即在新面首列续排。
    /// `ci` 每消费一个原文字符 +1（标记切换时跳过、不计版面），单调
    /// 前进——正文与注文交替也不回退、不错位（单测断言）。
    ///
    /// 取 `&mut self`：跑完后调用方可校验 `ci_consumed()`（主指针终值，
    /// 只前进不回退）与面数，供单测 / 控制台验证使用。
    fn run(&mut self, text: &str) -> Vec<ZFlowCell> {
        for seg in parse_annotated(text) {
            match seg {
                Seg::Text(t) => {
                    for ch in t.chars() {
                        // 新面首列优先：缓存注文未排完前，正文让位。
                        self.flush_pending();
                        self.place_main(ch);
                        self.ci += 1;
                        self.page_fresh = self.pending.is_some();
                    }
                }
                Seg::Note(t) => {
                    let all: Vec<char> = t.chars().collect();
                    let half = (all.len() + 1) / 2; // 右小列（读在前）字数
                    for k in 0..all.len() {
                        let ch = all[k]; // 当前注文字（主指针内容）
                        self.flush_pending();
                        if self.place_note(ch, k, half).is_err() {
                            // 本面列尽（连同页流转列也没有）：收面，
                            // 余文（含当前字）入缓存，立刻在新面首列续排。
                            // 续排可能跨多列 / 多面，全部排完才回到本块。
                            self.seal_page();
                            self.pending = Some(PendingNote {
                                rest: all[k..].to_vec(),
                                next_idx: k,
                                half,
                            });
                            self.flush_pending();
                            // 本块余文已整体交托缓存：立即跳出本块循环。
                            // 剩余字符由 flush_pending / 收尾循环排完
                            //（ci 也由 drain_pending 记账）——若继续迭代
                            // k+1..，会把已入缓存的字重复排一遍。
                            break;
                        }
                        self.ci += 1;
                        self.page_fresh = self.pending.is_some();
                    }
                }
            }
        }
        // 正文耗尽但缓存还有（最后一块注文跨多面）：继续翻面续排。
        let mut guard = 0usize;
        while self.pending.is_some() && guard < 100_000 {
            guard += 1;
            if !self.drain_pending() {
                // 刚收过面：缓存仍在，新面首列必空，下一轮必落字。
                continue;
            }
        }
        if !self.cells.is_empty() || self.pages.is_empty() {
            self.pages.push(std::mem::take(&mut self.cells));
        }
        // 单测通过 `ci_consumed` 校验主指针；这里按面展开为字表。
        // 注意：不能 take 走 `pages`——`layout_z_flow_pages` 还要按面读。
        self.pages.iter().flatten().cloned().collect()
    }

    /// 主指针最终值（单测用：断言它只前进、不回退）。
    fn ci_consumed(&self) -> usize {
        self.ci
    }

    /// 新面首列优先权：只要新面首列未动且缓存非空，就先把缓存排掉。
    /// drain 到本面列尽时会自行收面开新面（新首列必空、`page_fresh`
    /// 保持 true、缓存原样保留），此时继续循环即可在新面首列续排——
    /// 绝不能 break 后让正文插到缓存注文前面。
    fn flush_pending(&mut self) {
        let mut guard = 0usize;
        while self.page_fresh && self.pending.is_some() && guard < 100_000 {
            guard += 1;
            // 返回 false = 刚收过面（page_fresh 仍 true、缓存仍在），
            // 新面首列必然空（cap >= 2），下一轮迭代一定能落字。
            let _ = self.drain_pending();
        }
    }

    /// 开下一列；`want_note_only` = 是否纯注文列。false = 本面列尽。
    fn next_col(&mut self, want_note_only: bool) -> bool {
        self.col += 1;
        if self.col >= self.cols {
            return false;
        }
        self.col_used = 0;
        self.col_y = 0.0;
        self.col_slots = Vec::new();
        self.note_only = want_note_only;
        true
    }

    /// 收面：当前 cells 入 pages。收面后 `page_fresh = true`——
    /// 新面首列若有缓存注文，由 `flush_pending` 优先续排。
    fn seal_page(&mut self) {
        self.pages.push(std::mem::take(&mut self.cells));
        self.page_no += 1;
        self.col = 0;
        self.col_used = 0;
        self.col_y = 0.0;
        self.col_slots = Vec::new();
        self.note_only = false;
        self.page_fresh = true;
    }

    /// 放一个正文字（占 2 半格）。本列放不下就换列；本面列尽就收面，
    /// 收面后新面首列必空（`cap >= 2`），继续排此字——不丢、不截断。
    fn place_main(&mut self, ch: char) {
        if self.note_only || self.col_used + 2 > self.cap {
            if !self.next_col(false) {
                self.seal_page();
            }
        }
        self.col_slots.push(RowSlot::Main(ch));
        self.col_used += 2;
        let y = self.col_y;
        self.col_y += self.row_h;
        self.cells.push(ZFlowCell {
            ch,
            x: self.col as f32 * self.col_w,
            y,
            w: self.col_w,
            h: self.row_h,
            fs: self.fs,
            note: false,
            page: self.page_no,
            col: self.col,
            sub: 0,
            note_idx: 0,
        });
    }

    /// 放一个注文字（占 1 半格，右先左后）。Err(()) = 本面列尽，
    /// 连同页流转列也没有了——调用方收面并余文入缓存。
    fn place_note(&mut self, ch: char, idx: usize, half: usize) -> Result<(), ()> {
        let right = idx < half; // 竖排右先左后
        // ① 并入列尾半格行（尾 slot 恰缺同侧字）——不占新半格。
        if let Some(RowSlot::Note(l, r)) = self.col_slots.last_mut() {
            if right && r.is_none() {
                *r = Some(ch);
                let y = self.col_y - self.sub_row;
                self.emit_note(ch, right, y, idx);
                return Ok(());
            } else if !right && l.is_none() {
                *l = Some(ch);
                let y = self.col_y - self.sub_row;
                self.emit_note(ch, right, y, idx);
                return Ok(());
            }
        }
        // ② 需要新半格行：本列满 → 同页流转（纯注文列）；仍满 → 本面列尽。
        if self.col_used + 1 > self.cap && !self.next_col(true) {
            return Err(());
        }
        self.push_note_slot(ch, right, idx);
        Ok(())
    }

    fn push_note_slot(&mut self, ch: char, right: bool, idx: usize) {
        self.col_slots.push(RowSlot::Note(
            if right { None } else { Some(ch) },
            if right { Some(ch) } else { None },
        ));
        self.col_used += 1;
        let y = self.col_y;
        self.col_y += self.sub_row;
        self.emit_note(ch, right, y, idx);
    }

    fn emit_note(&mut self, ch: char, right: bool, y: f32, idx: usize) {
        self.cells.push(ZFlowCell {
            ch,
            x: self.col as f32 * self.col_w + if right { self.sub_w } else { 0.0 },
            y,
            w: self.sub_w,
            h: self.sub_row,
            fs: self.nfs,
            note: true,
            page: self.page_no,
            col: self.col,
            sub: if right { 1 } else { 0 },
            note_idx: idx,
        });
    }

    /// 排缓存注文的下一个字。返回是否有进展（false = 本面列尽已收面，
    /// 缓存仍在，调用方应再试新面首列）。
    fn drain_pending(&mut self) -> bool {
        let Some(p) = self.pending.take() else {
            return true; // 空缓存：视为有进展（外层循环终止条件）
        };
        let ch = p.rest[0];
        match self.place_note(ch, p.next_idx, p.half) {
            Ok(()) => {
                // 缓存注字同样是主指针消费的一个原文字符（ci 守恒）。
                self.ci += 1;
                if p.rest.len() == 1 {
                    // 缓存清空。
                    self.page_fresh = false;
                    true
                } else {
                    self.pending = Some(PendingNote {
                        rest: p.rest[1..].to_vec(),
                        next_idx: p.next_idx + 1,
                        half: p.half,
                    });
                    true
                }
            }
            Err(()) => {
                // 本面列尽：收面开新面，缓存原样保留（idx 连续）。
                self.seal_page();
                self.pending = Some(p);
                false
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Book, Chapter, GlyphKind, Paragraph};

    fn flow_of(book: &Book) -> Vec<Glyph> {
        book.to_flow(true)
    }

    // ---- 注文包围盒（Canvas 命中测试 / 卡片定位）----

    fn main_glyph() -> Piece {
        Piece::Main(Glyph {
            chapter: 0,
            para: 0,
            offset: 0,
            ch: '一',
            kind: GlyphKind::Char,
        })
    }

    #[test]
    fn note_boxes_stack_top_down_in_column() {
        // 列内：正文每字一个 cell_h，注块 = ceil(n/2) 个半格，自上而下累计。
        let col = vec![
            main_glyph(),                 // 占 0..cell_h
            Piece::Gloss("甲乙".into()),   // 4 字 → 1 行 → cell_h/2
            main_glyph(),
            Piece::Gloss("丙丁戊己".into()), // 4 字 → 2 行 → cell_h
        ];
        let boxes = note_boxes_in_column(&col, 2, 46.0, 51.0);
        assert_eq!(boxes.len(), 2);
        // 第一个注块：正文一字之后。
        assert!((boxes[0].y - 51.0).abs() < 0.01);
        assert!((boxes[0].h - 25.5).abs() < 0.01);
        // 第二个注块：又排了一个正文字 + 半个注块。
        assert!((boxes[1].y - (51.0 + 25.5 + 51.0)).abs() < 0.01);
        assert!((boxes[1].h - 51.0).abs() < 0.01);
        // 宽 = 列宽 − 左侧留白（注块左缘空一道，见 NOTE_PAD_RATIO）；
        // x = 列序 li × 列宽 + 同一留白。
        let pad = 46.0 * NOTE_PAD_RATIO;
        assert!((boxes[0].w - (46.0 - pad)).abs() < 0.01);
        assert!((boxes[0].x - (2.0 * 46.0 + pad)).abs() < 0.01);
        assert_eq!(boxes[0].text, "甲乙");
        assert_eq!(boxes[1].text, "丙丁戊己");
    }

    #[test]
    fn note_boxes_reverse_order_for_vertical_text() {
        // 竖排读序列号 i=0 在最右 → flex_row 从左数的 li = text_cols-1-i，
        // 渲染侧传进来的 li 越大 x 越大：最右列 x = (text_cols-1)*cell_w
        //（再往后偏一个左侧留白）。
        let col = vec![Piece::Gloss("注".into())];
        let (cell_w, cell_h) = (50.0, 55.0);
        let text_cols = 4;
        let rightmost = note_boxes_in_column(&col, text_cols - 1 - 0, cell_w, cell_h);
        let leftmost = note_boxes_in_column(&col, text_cols - 1 - 3, cell_w, cell_h);
        let pad = cell_w * NOTE_PAD_RATIO;
        assert!((rightmost[0].x - (3.0 * cell_w + pad)).abs() < 0.01);
        assert!((leftmost[0].x - pad).abs() < 0.01);
        assert!(rightmost[0].x > leftmost[0].x);
    }

    #[test]
    fn note_boxes_empty_when_no_gloss() {
        let col = vec![main_glyph(), main_glyph()];
        assert!(note_boxes_in_column(&col, 0, 40.0, 44.0).is_empty());
    }

    #[test]
    fn leaf_origin_single_page_centers_and_clears_chrome() {
        // 单页：水平居中；垂直在正文区（工具栏 44 + 状态栏 34 之下）居中。
        let (x, y) = leaf_origin(false, 700.0, 780.0, 620.0, 640.0, false, false);
        assert!((x - (700.0 - 620.0) / 2.0).abs() < 0.01);
        let body_h = 780.0 - 78.0;
        assert!((y - (78.0 + (body_h - 640.0) / 2.0)).abs() < 0.01);
        // 专注模式：贴屏，无 chrome 偏移。
        let (fx, fy) = leaf_origin(false, 700.0, 780.0, 700.0, 780.0, false, true);
        assert!((fx).abs() < 0.01);
        assert!((fy).abs() < 0.01);
    }

    #[test]
    fn leaf_origin_facing_puts_back_on_left() {
        // 对开：两叶并排隔 LEAF_GAP，整体居中；back（下一面）在左。
        let (pw, ph) = (560.0, 640.0);
        let (lx, ly) = leaf_origin(true, 1200.0, 800.0, pw, ph, true, false);
        let (rx, ry) = leaf_origin(true, 1200.0, 800.0, pw, ph, false, false);
        let stage = pw * 2.0 + crate::grid::LEAF_GAP;
        assert!((lx - (1200.0 - stage) / 2.0).abs() < 0.01);
        assert!((rx - (lx + pw + crate::grid::LEAF_GAP)).abs() < 0.01);
        assert_eq!(ly, ry);
        assert!(rx > lx);
    }

    // ---- 双行夹注（双列小字）· Z 字型填充 ----

    #[test]
    fn parse_annotated_splits_text_and_notes() {
        use super::{parse_annotated, Seg};
        assert_eq!(
            parse_annotated("道可道"),
            vec![Seg::Text("道可道".to_string())]
        );
        assert_eq!(
            parse_annotated("道[注]注一[/注]可"),
            vec![
                Seg::Text("道".to_string()),
                Seg::Note("注一".to_string()),
                Seg::Text("可".to_string()),
            ]
        );
        // 未闭合的 [注] 吞到结尾（不丢字）。
        assert_eq!(
            parse_annotated("道[注]注一"),
            vec![Seg::Text("道".to_string()), Seg::Note("注一".to_string())]
        );
        // 空注文也成块（块高 0，游标不推进）。
        assert_eq!(
            parse_annotated("道[注][/注]可"),
            vec![
                Seg::Text("道".to_string()),
                Seg::Note(String::new()),
                Seg::Text("可".to_string()),
            ]
        );
    }

    #[test]
    fn z_layout_folds_column_and_alternates() {
        use super::{z_layout, Seg};
        let (col_w, row_h, fs) = (46.0, 1.1 * 46.0, 0.75 * 46.0);
        let (sub_w, sub_row, nfs) = (col_w / 2.0, row_h / 2.0, fs * 0.5);
        // 样文：3 个正文字 + 5 字注文（奇数）+ 1 个正文字。
        let text = "甲乙丙[注]一二三四五[/注]丁";
        assert_eq!(
            super::parse_annotated(text)
                .iter()
                .filter(|s| matches!(s, Seg::Note(_)))
                .count(),
            1
        );
        let cells = z_layout(text, col_w, row_h, fs);

        let mains: Vec<&_> = cells.iter().filter(|c| !c.note).collect();
        let notes: Vec<&_> = cells.iter().filter(|c| c.note).collect();
        assert_eq!(mains.len(), 4, "正文一字不多不少");
        assert_eq!(notes.len(), 5, "注文一字不多不少");

        // 正文：满格宽高、字号不变；y 每字推进一个 row_h。
        for (i, m) in mains.iter().enumerate() {
            assert!((m.x).abs() < 1e-3);
            assert!((m.w - col_w).abs() < 1e-3);
            assert!((m.h - row_h).abs() < 1e-3);
            assert!((m.fs - fs).abs() < 1e-3);
            if i == 0 {
                assert!((m.y).abs() < 1e-3);
            }
        }

        // 注文：半格宽高 + 50% 字号；块锚点 = 宿主字之后（3 个正文 = 3*row_h）。
        let anchor = 3.0 * row_h;
        for (_i, n) in notes.iter().enumerate() {
            assert!((n.w - sub_w).abs() < 1e-3, "小列宽必须是 colWidth/2");
            assert!((n.h - sub_row).abs() < 1e-3, "小行高必须是 rowHeight/2");
            assert!((n.fs - nfs).abs() < 1e-3, "注文字号必须是正文 50%");
        }
        // 竖排右先左后：块内前 half=ceil(5/2)=3 字进右小列（读序在前），
        // 余下 2 字进左小列；两小列各自自上而下，构成 Z 字型填充。
        let half = (5 + 1) / 2;
        for (i, n) in notes.iter().enumerate() {
            let (want_x, want_row) = if i < half {
                (sub_w, i) // 右小列第 i 行
            } else {
                (0.0, i - half) // 左小列第 (i-half) 行
            };
            assert!(
                (n.x - want_x).abs() < 1e-3,
                "第 {i} 个注字 x 错位（右先左后期望右小列 x=colWidth/2）"
            );
            assert!(
                (n.y - (anchor + want_row as f32 * sub_row)).abs() < 1e-3,
                "第 {i} 个注字 y 错位"
            );
        }

        // 行高对齐：两个上下小字的总高严格等于一行正文（大字 + 行间距）。
        assert!(
            (2.0 * sub_row - row_h).abs() < 1e-3,
            "2×小行高必须严格等于正文行高"
        );
        // 同小列内相邻行距恰为一个 sub_row（不错位）：右列 0→1→2、左列 0→1。
        assert!((notes[1].y - notes[0].y - sub_row).abs() < 1e-3);
        assert!((notes[2].y - notes[1].y - sub_row).abs() < 1e-3);
        assert!((notes[4].y - notes[3].y - sub_row).abs() < 1e-3);
        // 左右小列首行对齐（Z 字型的横向带必须同高）。
        assert!((notes[3].y - notes[0].y).abs() < 1e-3);

        // 收尾：注块占 ceil(5/2)=3 小行，最后一个正文字紧跟其后。
        let last = cells.last().unwrap();
        assert!(!last.note);
        assert!((last.y - (anchor + 3.0 * sub_row)).abs() < 1e-3);
    }

    #[test]
    fn filler_accounts_main_and_note_half_units() {
        // cap = 4：两列各 2 个正文格，或 1 正文 + 2 行注。
        let mut f = Filler::new(4);
        assert!(!f.push_main()); // 1 字（2 格）
        assert!(!f.push_main()); // 2 字（满）
        assert_eq!(f.room(), 0);
        assert!(f.push_main()); // 第 3 字起新列
        assert_eq!(f.room(), 2);
        assert_eq!(f.push_note(4), 4); // 4 字注（2 行）正好放下
        assert!(f.is_full());
        assert_eq!(f.push_note(2), 0); // 满列再放注：本列一字不收
        f.begin_column();
        assert!(!f.is_full());
    }

    #[test]
    fn note_splits_across_columns() {
        // 3 列 × cap 4（rows=2）。第 2 字挂 10 字夹注（5 行 > 本列余量 1 行），
        // 应在本列排 2 字、注尾流转到下一列列首。
        let mut b = Book::new("夹注流转", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![Paragraph::plain("一二三四五六七八九十")],
        });
        let flow = flow_of(&b);
        let pages = layout_book(
            &flow,
            |g| (g.offset == 1).then(|| "甲乙丙丁戊己庚辛壬癸".to_string()),
            3,
            2,
            0,
        );
        // 全文不丢字。
        let flat: Vec<&Glyph> = pages.iter().flat_map(|p| p.glyphs.iter()).collect();
        assert_eq!(flat.len(), flow.len());
        // 注文 10 字全部在（可跨列）。
        let joined: String = pages
            .iter()
            .flat_map(|p| p.columns.iter())
            .flatten()
            .filter_map(|p| match p {
                Piece::Gloss(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(joined, "甲乙丙丁戊己庚辛壬癸");
        // 注至少占两列：存在「列首即是注尾」的列。
        let gloss_first = pages
            .iter()
            .flat_map(|p| p.columns.iter())
            .any(|c| matches!(c.first(), Some(Piece::Gloss(_))));
        assert!(gloss_first, "长注应跨列流转：下一列列首应是注尾");
    }

    #[test]
    fn note_tail_can_cross_page_boundary() {
        // 2 列 × cap 4（rows=2）：一面最多 4 个正文格。第 1 字挂 30 字夹注
        // （15 行 > 一面容量 8 格），注尾必须进入下一面首列，且一字不丢。
        let mut b = Book::new("跨叶注", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![Paragraph::plain("一二三四五六七八九十十一十二")],
        });
        let flow = flow_of(&b);
        let pages = layout_book(
            &flow,
            |g| (g.kind == GlyphKind::Char && g.offset == 0).then(|| "注".repeat(30)),
            2,
            2,
            0,
        );        let joined: String = pages
            .iter()
            .flat_map(|p| p.columns.iter())
            .flatten()
            .filter_map(|p| match p {
                Piece::Gloss(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(joined.chars().count(), 30, "跨叶注文丢失");
        assert!(pages.len() >= 3, "长注应推着正文跨面：{} 面", pages.len());
        let flat: Vec<&Glyph> = pages.iter().flat_map(|p| p.glyphs.iter()).collect();
        assert_eq!(flat.len(), flow.len(), "跨叶注推挤下正文丢字");
    }

    #[test]
    fn chapter_inscription_reserve_keeps_all_glyphs() {
        // 卷首题署会在章首页开卷列顶部预留若干行；无论列数 / 行数 / 预留量，
        // 都不能丢字——预留只减少开卷列正文容量，正文顺延到后续列/面。
        let mut b = Book::new("書名四字", "撰者五字");
        for ci in 0..2 {
            b.chapters.push(Chapter {
                title: format!("第{ci}章"),
                paragraphs: vec![Paragraph::plain(
                    "天地玄黄宇宙洪荒日月盈昃辰宿列张".repeat(3),
                )],
            });
        }
        let flow = flow_of(&b);
        for cols in [8usize, 10, 12, 14] {
            for rows in [10usize, 14, 18] {
                // 复刻 app 的预留量计算（书名 + 卷次(~2) + 间隔1 + 署名）。
                let need = b.title.chars().count() + b.author.chars().count() + 3;
                let reserve = crate::gloss::inscription_reserve(need, rows);
                let pages = layout_book(&flow, |_| None, cols, rows, reserve);
                let flat: Vec<&Glyph> = pages.iter().flat_map(|p| p.glyphs.iter()).collect();
                assert_eq!(flat.len(), flow.len(), "{cols}x{rows} 预留带丢字");
                assert!(pages[0].start_chapter, "首面须为章首");
            }
        }
    }

    #[test]
    fn chapter_start_page_holds_fewer_columns() {
        // 每章段长远小于一面：章首页应 ≤ cols-1 列，续面 ≤ cols 列。
        let mut b = Book::new("章首列数", "佚名");
        for ci in 0..2 {
            b.chapters.push(Chapter {
                title: format!("第{ci}章"),
                paragraphs: vec![Paragraph::plain("一二三四五六七八九十十一十二")],
            });
        }
        let flow = flow_of(&b);
        let (cols, rows) = (4usize, 2usize);
        let pages = layout_book(&flow, |_| None, cols, rows, 0);
        assert!(pages.len() >= 2);
        for pg in &pages {
            let cap = if pg.start_chapter { cols - 1 } else { cols };
            assert!(
                pg.columns.len() <= cap,
                "章首页 {:?} / 续面 {:?} 列数 {} 超上限 {}",
                pg.start_chapter,
                !pg.start_chapter,
                pg.columns.len(),
                cap
            );
        }
        assert!(pages[0].start_chapter, "第一面必须是章首");
    }

    #[test]
    fn any_columns_drop_no_glyph() {
        // 8 / 10 / 12 / 14 列 + 半数奇字挂夹注：不丢字、每面不超列上限。
        let mut b = Book::new("任意列", "佚名");
        for ci in 0..2 {
            b.chapters.push(Chapter {
                title: format!("第{ci}章"),
                paragraphs: vec![Paragraph::plain("天地玄黄宇宙洪荒日月盈昃辰宿列张".repeat(4))],
            });
        }
        let flow = flow_of(&b);
        for cols in crate::grid::GRID_COLUMN_CHOICES {
        let pages = layout_book(
            &flow,
            |g| (g.offset % 2 == 0 && g.kind == GlyphKind::Char).then(|| "夹注夹注".to_string()),
            cols,
            11,
            0,
        );
            let flat: Vec<&Glyph> = pages.iter().flat_map(|p| p.glyphs.iter()).collect();
            assert_eq!(flat.len(), flow.len(), "{cols} 列丢了字");
            for (a, b) in flat.iter().zip(flow.iter()) {
                assert_eq!((a.chapter, a.para, a.offset, a.ch), (b.chapter, b.para, b.offset, b.ch));
            }
            for pg in &pages {
                let cap = if pg.start_chapter { cols - 1 } else { cols };
                assert!(pg.columns.len() <= cap, "{cols} 列某面超限");
                // 每列容量不超：列内半格总数 ≤ rows*2。
                let units: usize = pg
                    .columns
                    .iter()
                    .map(|c| {
                        c.iter()
                            .map(|p| match p {
                                Piece::Main(_) => 2,
                                Piece::Gloss(s) => (s.chars().count() + 1) / 2,
                            })
                            .sum::<usize>()
                    })
                    .sum();
                assert!(units <= cols * 22, "{cols} 列某面半格超容");
            }
        }
    }

    // ---- Z 字型流转引擎：溢出 / 同页流转 / 跨页 pendingNote ----

    fn stripped_len(text: &str) -> usize {
        parse_annotated(text)
            .iter()
            .map(|s| match s {
                Seg::Text(t) => t.chars().count(),
                Seg::Note(t) => t.chars().count(),
            })
            .sum()
    }

    /// 列底不截断 + 同页流转：注文排到列底还没完，剩余流到本面下一列，
    /// 该列是纯注文列（不再排正文）；后续正文字另起新列。一字不丢。
    #[test]
    fn z_flow_overflows_to_same_page_note_only_column() {
        // 3 列 × rows 2 → cap = 4 半格/列。
        let pages = layout_z_flow_pages("甲[注]一二三四五六[/注]乙", 3, 2, 46.0);
        let flat: Vec<&ZFlowCell> = pages.iter().flat_map(|p| p.iter()).collect();
        assert_eq!(flat.len(), 8, "正文 2 字 + 注文 6 字");
        let main: String = flat.iter().filter(|c| !c.note).map(|c| c.ch).collect();
        let note: String = flat.iter().filter(|c| c.note).map(|c| c.ch).collect();
        assert_eq!(main, "甲乙");
        assert_eq!(note, "一二三四五六", "列底不截断：注文一字不能少");
        // 没跨页：整面排完。
        assert_eq!(pages.len(), 1);
        // 注文跨到了第 2 列（col 1）。
        assert!(flat.iter().any(|c| c.note && c.col == 1));
        // 同页流转列只有小字——「这一列不需要再排正文」。
        assert!(
            flat.iter().all(|c| c.col != 1 || c.note),
            "同页流转列必须是纯注文列"
        );
        // 后续正文字另起新列（col 2），不挤回流注列。
        let last_main = flat.iter().filter(|c| !c.note).last().unwrap();
        assert_eq!(last_main.ch, '乙');
        assert_eq!(last_main.col, 2);
        // 列容量不超：每列已用高度 ≤ cap 半格。
        let (cap, sub_row) = (4usize, 1.1 * 46.0 / 2.0);
        for pg in &pages {
            for col in 0..3 {
                let used = flat
                    .iter()
                    .filter(|c| c.page == pg[0].page && c.col == col)
                    .map(|c| c.y + c.h)
                    .fold(0.0f32, f32::max);
                assert!(used <= cap as f32 * sub_row + 1e-3, "第 {col} 列超容");
            }
        }
    }

    /// 跨页流转：一面排满后剩余注文入 pendingNote，新面首列优先续排
    /// （排在后续正文之前），读序靠 note_idx 跨页连续。
    #[test]
    fn z_flow_pending_note_leads_next_page() {
        // 2 列 × rows 2 → cap 4，一面最多 8 半格。
        // 注文 12 字（6 小行）超过一面余量 → 注尾必须进下一面首列。
        let text = "甲[注]一二三四五六七八九十甲乙[/注]乙";
        let pages = layout_z_flow_pages(text, 2, 2, 46.0);
        let flat: Vec<&ZFlowCell> = pages.iter().flat_map(|p| p.iter()).collect();
        assert_eq!(flat.len(), 14, "正文 2 字 + 注文 12 字");
        assert!(pages.len() >= 2, "长注应跨页：{} 面", pages.len());
        let main: String = flat.iter().filter(|c| !c.note).map(|c| c.ch).collect();
        let note: String = flat.iter().filter(|c| c.note).map(|c| c.ch).collect();
        assert_eq!(main, "甲乙");
        assert_eq!(note, "一二三四五六七八九十甲乙", "跨页注文一字不能少");
        // 新面首列优先：第 1 面列 0 最前面的字是缓存注文续排（八，idx 7），
        // 而不是新正文「乙」。
        let first_of_page1 = flat
            .iter()
            .filter(|c| c.page == 1)
            .min_by(|a, b| {
                (a.col, a.y)
                    .partial_cmp(&(b.col, b.y))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        assert!(first_of_page1.note, "新面首列必须是缓存注文优先");
        assert_eq!(first_of_page1.ch, '八');
        assert_eq!(first_of_page1.note_idx, 7, "读序跨页连续");
        // 正文字「乙」排在缓存注文全部落完之后（本块 5 个续排字把第 1 面
        // 两列排满，新正文只能落到第 2 面）。
        let last_main = flat.iter().filter(|c| !c.note).last().unwrap();
        assert!(
            last_main.page == 2 && last_main.col == 0,
            "新正文应排在缓存注文之后（实际第 {} 面第 {} 列）",
            last_main.page,
            last_main.col
        );
        // 右先左后：块内前 half=6 字进右小列（sub=1），余下进左小列。
        let half = 6usize;
        for c in flat.iter().filter(|c| c.note) {
            assert_eq!(c.sub, u8::from(c.note_idx < half));
        }
        // 行高对齐：小行高恰为正文行高一半。
        let mh = flat.iter().find(|c| !c.note).unwrap().h;
        let nh = flat.iter().find(|c| c.note).unwrap().h;
        assert!((2.0 * nh - mh).abs() < 1e-3);
    }

    /// 主进度指针：正文与注文交替读取时，ci 每消费一个原文字符 +1，
    /// 终值恒等于全字计数；每块注文的 note_idx 恰为 0..len-1。
    #[test]
    fn z_flow_ci_advances_one_per_char() {
        let text = "道可道[注]非常道[/注]非常道[注]道可道，非常道[/注]名可名";
        let stripped = stripped_len(text);
        for cols in [1usize, 2, 3, 5] {
            for rows in [1usize, 2, 3] {
                let mut b = ZFlowBuilder::new(cols, rows, 46.0);
                let cells = b.run(text);
                assert_eq!(b.ci_consumed(), stripped, "{cols}x{rows} 主指针乱掉");
                assert_eq!(cells.len(), stripped, "{cols}x{rows} 主指针内容错位");
                // 每块注文的块内序号 0..len-1 连续（右先左后读序的依据）。
                let lens: Vec<usize> = parse_annotated(text)
                    .iter()
                    .filter_map(|s| match s {
                        Seg::Note(t) => Some(t.chars().count()),
                        _ => None,
                    })
                    .collect();
                let idxs: Vec<usize> =
                    cells.iter().filter(|c| c.note).map(|c| c.note_idx).collect();
                let want: Vec<usize> = lens.iter().flat_map(|&l| 0..l).collect();
                assert_eq!(idxs, want, "{cols}x{rows} 块内读序断档");
                // 列容量不超（任意列数 / 行数都不丢不超）。
                let cap = (rows * 2).max(2);
                let sub_row = 1.1 * 46.0 / 2.0;
                for pg_col in 0..cols {
                    for pg in 0..=cells.iter().map(|c| c.page).max().unwrap_or(0) {
                        let used = cells
                            .iter()
                            .filter(|c| c.page == pg && c.col == pg_col)
                            .map(|c| c.y + c.h)
                            .fold(0.0f32, f32::max);
                        assert!(
                            used <= cap as f32 * sub_row + 1e-3,
                            "{cols}x{rows} 第 {pg} 面第 {pg_col} 列超容"
                        );
                    }
                }
            }
        }
    }

    /// 任意几何（单列 / 多列、单行 / 多行）全字守恒、双流各自保序。
    #[test]
    fn z_flow_any_geometry_loses_no_char() {
        let text = "天地玄黄宇宙洪荒[注]日月盈昃辰宿列张寒来暑往秋收冬藏[/注]闰余成岁律吕调阳";
        for cols in [1usize, 2, 3, 4, 7] {
            for rows in [1usize, 2, 3, 5] {
                let pages = layout_z_flow_pages(text, cols, rows, 46.0);
                let flat: Vec<&ZFlowCell> = pages.iter().flat_map(|p| p.iter()).collect();
                assert_eq!(flat.len(), stripped_len(text), "{cols}x{rows} 丢字");
                let main: String =
                    flat.iter().filter(|c| !c.note).map(|c| c.ch).collect();
                let note: String =
                    flat.iter().filter(|c| c.note).map(|c| c.ch).collect();
                assert_eq!(main, "天地玄黄宇宙洪荒闰余成岁律吕调阳", "{cols}x{rows}");
                assert_eq!(note, "日月盈昃辰宿列张寒来暑往秋收冬藏", "{cols}x{rows}");
                assert_eq!(
                    layout_z_flow_ci(text, cols, rows, 46.0),
                    stripped_len(text),
                    "{cols}x{rows} ci 不守恒"
                );
            }
        }
    }
}
