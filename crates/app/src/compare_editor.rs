//! Compare Mode 右栏「笺校编辑器」的**纯逻辑层**（Task 1）。
//!
//! 这里只放不依赖 GPUI / SQLite 的计算，目的有二：
//!   ① 五条硬指标里最难的两条——**坐标重映射**与**撤销栈**——能脱离窗口直接单测；
//!   ② 与 `collation.rs` 的笺校工作台保持同一套口径，不另造一份「差不多」的实现。
//!
//! 与工作台的口径对照：
//!   · 夹注识别：`collation::strip_gloss` 认 `（…）` / `(…)` / `[注:…]` /
//!     `[注]…[/注]`；本模块 [`classify_spans`] 认同一批写法，只是把「剥掉」
//!     换成「标记成小字」（双屏右栏要看得见、改得着，即工作台的源码视图原则）。
//!   · 撤销：`collation` 用 `EditOp` + 700ms 同类别合并 + 500 步上限；本模块
//!     用整页快照（一页几百字，代价可忽略），保留同样的合并 / barrier / 分叉即弃。
//!   · 组字：IME 中间态不进撤销栈、不落库，上屏时以「组字前快照」记一步。
//!
//! ⚠️ 本模块**不碰** `crates/core`：core 纯洁守卫一条不动。

use std::ops::Range;
use std::time::Instant;

// ─────────────────────────────────────────────────────────────────────────────
// ① 排版层级：大字（正文）/ 小字（夹注）
// ─────────────────────────────────────────────────────────────────────────────

/// 单个字符的排版层级。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpanKind {
    /// 正文大字。
    Body,
    /// 夹注小字（`（…）` / `[注:…]` / `[注]…[/注]` 内的注文，含括号本身）。
    Gloss,
    /// 夹注结构标签（`[注]` `[/注]` `[注:` 这类「不是书里的字」）。
    GlossTag,
}

fn starts_at(chars: &[char], i: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    i + p.len() <= chars.len() && chars[i..i + p.len()] == p[..]
}

/// 逐字标出排版层级，返回长度 = `text.chars().count()` 的表。
///
/// 认与 `collation::strip_gloss` 相同的三种夹注写法，另加古籍常用的 `〔…〕`
/// 与 `【…】`（工作台里 `〔…〕` 会升格成 `[注]…[/注]`，这里同样按夹注看）。
/// 括号可嵌套；半壳 / 孤收壳绝不吞掉后文——宁可少标几个字，也不能把正文全染小。
pub fn classify_spans(text: &str) -> Vec<SpanKind> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut kinds = vec![SpanKind::Body; n];

    // (a) 笺读自家标签 `[注]…[/注]`：壳标 GlossTag，注文标 Gloss。
    let mut i = 0usize;
    while i < n {
        if starts_at(&chars, i, "[注]") || starts_at(&chars, i, "[註]") {
            for k in i..(i + 3).min(n) {
                kinds[k] = SpanKind::GlossTag;
            }
            let mut j = i + 3;
            let mut close = None;
            while j + 3 < n {
                if starts_at(&chars, j, "[/注]") || starts_at(&chars, j, "[/註]") {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(c) = close {
                // 完整壳：注文小字，收壳也是标签。
                for k in (i + 3)..c {
                    kinds[k] = SpanKind::Gloss;
                }
                for k in c..(c + 4).min(n) {
                    kinds[k] = SpanKind::GlossTag;
                }
                i = c + 4;
            } else {
                // ⚠️ 只有开壳时**绝不把余下正文染成小字**：半壳只是个
                // 残缺标签，工作台 `strip_gloss` 同款「半壳不吞字」。
                // 早先写成 `close.unwrap_or(n)` 会让「甲[注]乙丙」的
                // 「乙丙」整段被误判成夹注（单测抓到）。
                i += 3;
            }
            continue;
        }
        // (b) `[注:…]` / `[註：…]`：整段连方括号一起当夹注，前 3 字是标签。
        let is_note = chars[i] == '['
            && matches!(chars.get(i + 1), Some('注') | Some('註'))
            && matches!(chars.get(i + 2), Some(':') | Some('：'));
        if is_note {
            let mut j = i + 3;
            while j < n && chars[j] != ']' {
                j += 1;
            }
            let end = (j + 1).min(n);
            for k in i..(i + 3).min(end) {
                kinds[k] = SpanKind::GlossTag;
            }
            for k in (i + 3)..end {
                kinds[k] = SpanKind::Gloss;
            }
            i = end;
            continue;
        }
        i += 1;
    }

    // (c) 括注：可嵌套，括号本身也算夹注。GlossTag 不被覆盖。
    let mut depth = 0usize;
    for i in 0..n {
        let c = chars[i];
        let open = matches!(c, '（' | '(' | '〔' | '【');
        let close = matches!(c, '）' | ')' | '〕' | '】');
        if open {
            depth += 1;
            if kinds[i] == SpanKind::Body {
                kinds[i] = SpanKind::Gloss;
            }
        } else if close {
            if kinds[i] == SpanKind::Body {
                kinds[i] = SpanKind::Gloss;
            }
            depth = depth.saturating_sub(1);
        } else if depth > 0 && kinds[i] == SpanKind::Body {
            kinds[i] = SpanKind::Gloss;
        }
    }
    kinds
}

// ─────────────────────────────────────────────────────────────────────────────
// ①b 注本体例切分：无字高双峰时的降级识别
// ─────────────────────────────────────────────────────────────────────────────

/// 章标题的尾字（「第一章」「卷之二」「第三則」…）。
fn is_chapter_tail(c: char) -> bool {
    matches!(c, '章' | '節' | '节' | '篇' | '卷' | '回' | '則' | '则')
}
fn is_han_num(c: char) -> bool {
    c.is_ascii_digit() || "一二三四五六七八九十百千零〇廿卅兩两".contains(c)
}

/// 从「第/卷」起扫一个章标题，返回 `(起, 止含)`。
fn chapter_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if !matches!(chars.get(i).copied(), Some('第') | Some('卷')) {
        return None;
    }
    let mut j = i + 1;
    if chars.get(j) == Some(&'之') {
        j += 1;
    }
    let num_start = j;
    while j < chars.len() && j - i <= 10 && is_han_num(chars[j]) {
        j += 1;
    }
    if j == num_start || j >= chars.len() || !is_chapter_tail(chars[j]) {
        return None;
    }
    Some((i, j))
}

/// 给一个句子定层级：**短句 = 经文大字，长解释句 = 注文小字**。
///
/// 这是注本（如王弼《老子注》）「经文简、注文繁」的体例特征。扫描件正注
/// 同字号、OCR 又不带 `[注]` 标记时，这是唯一可解释的自动分层依据；它明确
/// 是启发式，用户随时可以在编辑器里敲 `[注]…[/注]` 精确覆盖。
fn mark_sentence(kinds: &mut [SpanKind], chars: &[char], a: usize, b: usize) {
    if a >= b {
        return;
    }
    let seg = &chars[a..b];
    let is_punct = |c: char| {
        matches!(c, '：' | '，' | ',' | '、' | '；' | ';' | '。' | '！' | '？' | '\n' | '\r')
            || "「」『』“”\"'（）()〔〕【】".contains(c)
    };
    let body_len = seg.iter().filter(|c| !c.is_whitespace() && !is_punct(**c)).count();
    let commas = seg
        .iter()
        .filter(|c| matches!(c, '，' | ',' | '、'))
        .count();
    if body_len > 12 || commas >= 3 {
        for k in a..b {
            // 已被标成结构标签（章标题）的字绝不被句子规则改写。
            if kinds[k] == SpanKind::Body {
                kinds[k] = SpanKind::Gloss;
            }
        }
    }
}

/// 纯文本注本切分：书名 / 章标题 → 结构标签；短句 → 正文大字；长解释句 → 夹注小字。
///
/// 只在「整页既没有字高聚类结果、也没有显式夹注标记」时启用（见
/// [`resolve_spans`]），绝不覆盖真实数据。
pub fn infer_gloss_spans(text: &str) -> Vec<SpanKind> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut kinds = vec![SpanKind::Body; n];

    // (a) 全篇章标题 → GlossTag；首个标题之前的短书名一并归为标签。
    let mut first_end = None;
    let mut i = 0usize;
    while i < n {
        if let Some((s, e)) = chapter_at(&chars, i) {
            if first_end.is_none() && s <= 20 {
                for k in 0..s {
                    kinds[k] = SpanKind::GlossTag;
                }
                first_end = Some(e);
            }
            for k in s..=e {
                kinds[k] = SpanKind::GlossTag;
            }
            i = e + 1;
        } else {
            i += 1;
        }
    }
    let start = first_end.map(|e| e + 1).unwrap_or(0);

    // (b) 从正文起点按句读切句，短句留大字、长句降小字。
    let mut s = start;
    for i in start..n {
        if matches!(chars[i], '。' | '！' | '？' | '；' | ';' | '\n' | '\r') {
            mark_sentence(&mut kinds, &chars, s, i + 1);
            s = i + 1;
        }
    }
    if s < n {
        mark_sentence(&mut kinds, &chars, s, n);
    }
    kinds
}

/// **渲染 / 导出唯一入口**：合并三路层级，优先级从高到低
///   ① 显式夹注标记 `[注]…[/注]` / `（…）`（用户可写，最可信）；
///   ② SQLite `glyphs.span_kind` 的字高双峰聚类（真实刻本双行小字）；
///   ③ 整页无任何层级时，启用注本体例切分（现代排印本兜底）。
///
/// 返回长度恒等于 `text.chars().count()`，绝不越界。
pub fn resolve_spans(text: &str, db_kinds: &[u8]) -> Vec<SpanKind> {
    let mut kinds = classify_spans(text);
    for (i, k) in kinds.iter_mut().enumerate() {
        if *k == SpanKind::Body {
            *k = match db_kinds.get(i).copied().unwrap_or(0) {
                1 => SpanKind::Gloss,
                2 => SpanKind::GlossTag,
                _ => SpanKind::Body,
            };
        }
    }
    if kinds.iter().all(|&k| k == SpanKind::Body) {
        return infer_gloss_spans(text);
    }
    kinds
}

// ─────────────────────────────────────────────────────────────────────────────
// ② 字符网格布局：让「点击定位」「光标」「联动」共用同一套下标
// ─────────────────────────────────────────────────────────────────────────────

/// 一个可视字格。`idx` 是**全文绝对字符下标**（与 `glyph.target` 同一坐标系）。
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub idx: usize,
    pub ch: char,
    pub kind: SpanKind,
    /// 视觉行 / 列（横排=行号/列号，竖排=列号/列内字序）。
    pub row: usize,
    pub col: usize,
    /// 换行符在编辑态渲染成的可见占位（不是正文字符，用 `↵` 表示）。
    pub newline: bool,
}

/// 把全文切成「一行 / 一列」的字格。
///
/// 三条不变量（联动精度的根）：
///   ① `idx` 严格等于 `text.chars().enumerate()` 的下标，**换行符也占一位**
///      ——旧实现跳过 `\n` 却继续累加渲染 idx，段落一多右侧红框就整体前移；
///   ② `per` 是横排每行字数 / 竖排每列字数，两者共用同一套切分；
///   ③ `editing = true` 时换行符生成一个可见占位字格，保证**每一个字符
///      下标都有可点击区域**，光标才能点哪落哪。
pub fn layout_cells(text: &str, per: usize, editing: bool) -> Vec<Vec<Cell>> {
    let kinds = classify_spans(text);
    layout_cells_with_kinds(text, per, editing, &kinds)
}

/// 与 [`layout_cells`] 完全一致，但字格层级由调用方传入（SQLite 字高聚类 +
/// 显式标记 + 注本体例合并后的结果）。右栏渲染走这条，保证界面与导出同源。
pub fn layout_cells_with_kinds(
    text: &str,
    per: usize,
    editing: bool,
    kinds: &[SpanKind],
) -> Vec<Vec<Cell>> {
    let per = per.max(1);
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut row = 0usize;
    let mut col = 0usize;
    for (idx, ch) in text.chars().enumerate() {
        let kind = kinds.get(idx).copied().unwrap_or(SpanKind::Body);
        if ch == '\n' || ch == '\r' {
            if editing && ch == '\n' {
                cur.push(Cell {
                    idx,
                    ch: '↵',
                    kind: SpanKind::GlossTag,
                    row,
                    col,
                    newline: true,
                });
            }
            rows.push(std::mem::take(&mut cur));
            row += 1;
            col = 0;
            continue;
        }
        if col >= per {
            rows.push(std::mem::take(&mut cur));
            row += 1;
            col = 0;
        }
        cur.push(Cell {
            idx,
            ch,
            kind,
            row,
            col,
            newline: false,
        });
        col += 1;
    }
    if !cur.is_empty() || rows.is_empty() {
        rows.push(cur);
    }
    rows
}

/// 光标落在哪一视觉行 / 列（用于方向键、Home/End、IME 候选框锚点）。
pub fn caret_xy(rows: &[Vec<Cell>], cursor: usize) -> (usize, usize) {
    for (r, line) in rows.iter().enumerate() {
        if let Some(c) = line.first() {
            if cursor < c.idx {
                return (r, 0);
            }
        }
        if let Some(c) = line.last() {
            if cursor <= c.idx || (c.newline && cursor == c.idx) {
                let col = line
                    .iter()
                    .position(|x| x.idx >= cursor)
                    .unwrap_or(line.len());
                return (r, col);
            }
        }
    }
    let r = rows.len().saturating_sub(1);
    (r, rows.get(r).map(|l| l.len()).unwrap_or(0))
}

/// 按视觉行列反查字符下标（↑↓ 保持列、行首 / 行尾）。
pub fn index_at(rows: &[Vec<Cell>], row: usize, col: usize, total: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let r = row.min(rows.len() - 1);
    let line = &rows[r];
    if line.is_empty() {
        return total;
    }
    let c = col.min(line.len());
    if c < line.len() {
        line[c].idx
    } else {
        line.last().map(|x| x.idx + 1).unwrap_or(total).min(total)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ②-b 响应式排版：每行字数 / 容器宽度（纯函数，可脱离窗口单测）
// ─────────────────────────────────────────────────────────────────────────────

/// 右栏面板的实际宽度（逻辑像素）。
/// 内容区 = 视口 − 左右 padding(12×2) − 面板间距(12) − 左栏宽。
pub fn right_pane_width(vw: f32, left_w: f32) -> f32 {
    (vw - 24.0 - 12.0 - left_w).max(220.0)
}

/// 按右栏**当帧可用宽度**算每行字数，绝不硬编码。
///
/// 保证 `per × (字号 + 字距)` 不超过「容器宽 − 两侧留白 − 8px 安全余量」，
/// 因此窗口拉大拉小都只会在行尾自然折行，绝不在右缘裁字；稿纸格线的
/// 条数与长度也由同一个值派生，不会越界。
pub fn responsive_per_line(vw: f32, left_w: f32, fs: f32, pad: f32, char_gap: f32) -> usize {
    let right_w = right_pane_width(vw, left_w);
    let usable = (right_w - pad * 2.0 - 8.0).max(fs * 2.0);
    ((usable + char_gap) / (fs + char_gap)).floor().max(1.0) as usize
}

// ─────────────────────────────────────────────────────────────────────────────
// ③ 编辑操作 / 撤销栈
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Insert,
    Delete,
    Replace,
    Other,
}

impl EditKind {
    fn mergeable(self) -> bool {
        matches!(self, EditKind::Insert | EditKind::Delete)
    }
}

/// 一页的编辑快照。字形不进快照：任何时候都能由「基准原文 + 基准字框」
/// 经 [`align_texts`] 重算，存两份反而容易不一致。
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub text: String,
    pub cursor: usize,
}

/// 撤销 / 重做栈（工作台 `undo_stack` / `redo_stack` 的轻量同款）。
pub struct UndoStack {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    barrier: bool,
    last: Option<(EditKind, Instant)>,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            barrier: false,
            last: None,
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.barrier = false;
        self.last = None;
    }

    /// 光标刚挪过 / 刚撤销过：下一步必须自成一档，不许并进上一档。
    pub fn barrier(&mut self) {
        self.barrier = true;
        self.last = None;
    }

    /// 记一步可撤销的编辑。`before` 必须是**改动前**的快照。
    pub fn record(&mut self, before: Snapshot, kind: EditKind) {
        // 分叉即弃：任何新编辑都让原重做线作废。
        self.redo.clear();
        let now = Instant::now();
        let merge = !self.barrier
            && kind.mergeable()
            && self
                .last
                .map(|(k, t)| k == kind && now.saturating_duration_since(t).as_millis() < 700)
                .unwrap_or(false);
        self.barrier = false;
        self.last = Some((kind, now));
        if merge {
            return; // 连打字 / 连退格算一步，栈顶留着更早那份快照。
        }
        self.undo.push(before);
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
    }

    pub fn undo(&mut self, cur: Snapshot) -> Option<Snapshot> {
        let prev = self.undo.pop()?;
        self.redo.push(cur);
        self.barrier();
        Some(prev)
    }

    pub fn redo(&mut self, cur: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push(cur);
        self.barrier();
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    #[allow(dead_code)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    #[allow(dead_code)]
    pub fn depth(&self) -> usize {
        self.undo.len()
    }
}

/// 字符区间替换（**保留 `\n`**）。返回新文本与插入后的光标。
/// 与工作台 `splice_chars` 同一口径：越界一律夹紧，绝不 panic。
pub fn splice_chars(text: &str, start: usize, end: usize, ins: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let a = start.min(total);
    let b = end.clamp(a, total);
    let mut out = String::with_capacity(text.len() + ins.len());
    out.extend(chars[..a].iter());
    out.push_str(ins);
    out.extend(chars[b..].iter());
    (out, a + ins.chars().count())
}

/// IME 上屏文本清洗：滤掉控制字符但**留下换行**（Enter 是笺校的硬需求）。
/// 组字串里夹 `\r` / `\t` 是输入法常见脏数据，留着会把字格排歪。
pub fn sanitize_ime(text: &str, allow_newline: bool) -> String {
    text.chars()
        .filter(|c| {
            if *c == '\n' {
                allow_newline
            } else {
                !c.is_control()
            }
        })
        .collect()
}

/// UTF-16 区间 → 字符区间（IME 接口一律按 UTF-16 计位）。
pub fn range_to_chars(text: &str, range: Option<Range<usize>>, cursor: usize) -> (usize, usize) {
    let to_char = |u: usize| -> usize {
        let mut acc = 0usize;
        for (i, c) in text.chars().enumerate() {
            if acc >= u {
                return i;
            }
            acc += c.len_utf16();
        }
        text.chars().count()
    };
    match range {
        Some(r) => {
            let a = to_char(r.start);
            let b = to_char(r.end);
            (a.min(b), a.max(b))
        }
        None => (cursor, cursor),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ④ 坐标重映射（清单第 4 条，最核心）
// ─────────────────────────────────────────────────────────────────────────────

/// 旧文本 → 新文本的字符下标对齐结果。
pub struct Alignment {
    /// `map[i]`：旧文本第 i 个字在新文本里的下标；`None` = 这个旧字框停用。
    pub map: Vec<Option<usize>>,
    /// 降级复用的字框数（旧字被替换成一串新字时，把原框留给新串首字）。
    pub degraded: usize,
}

/// 计算字形框的降级映射。
///
/// 三条铁律：
///   ① **同字数替换（改错别字）→ 恒等映射，红框一个像素都不动**，连 LCS 都不跑；
///   ② 字数变化 → LCS 对齐，仍能对上的字继续用原 bbox，坐标零漂移；
///   ③ 被替换掉的那一段，把旧框降级给新串首字（`degraded` 计数），
///      绝不把框乱挪到别人身上；删掉的字直接停用，新增的字不画框。
///
/// 超大文本（> 4M 单元格）退回公共前后缀对齐，保证内存与耗时有上界。
pub fn align_texts(old: &str, new: &str) -> Alignment {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let n = a.len();
    let m = b.len();
    let identity: Vec<Option<usize>> = (0..n).map(|i| (i < m).then_some(i)).collect();
    if a == b || n == m {
        // ① 同字数：改的是字，不是位置。红框全部留在原处。
        return Alignment {
            map: identity,
            degraded: 0,
        };
    }

    if n.saturating_mul(m) > 4_000_000 {
        return prefix_suffix_align(&a, &b);
    }

    // ② LCS（u32 DP，4M 格 = 16MB，一页 OCR 文本远远用不到）。
    let w = m + 1;
    let mut dp = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * w + j] = if a[i] == b[j] {
                dp[(i + 1) * w + j + 1] + 1
            } else {
                dp[(i + 1) * w + j].max(dp[i * w + j + 1])
            };
        }
    }
    let mut matches: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut map = vec![None; n];
    for &(oi, nj) in &matches {
        map[oi] = Some(nj);
    }
    let mut degraded = 0usize;
    let mut taken: Vec<bool> = vec![false; m];
    for &(_, nj) in &matches {
        taken[nj] = true;
    }

    // ③ 相邻匹配之间的「替换块」：旧块首字 → 新块首字，标为降级复用。
    let mut pi = 0usize;
    let mut pj = 0usize;
    let mut seq = matches;
    seq.push((n, m));
    for (oi, nj) in seq {
        if pi < oi && pj < nj {
            // 旧 [pi,oi) 被换成新 [pj,nj)：原框让给新串第一个字。
            if !taken[pj] {
                if let Some(slot) = map.get_mut(pi) {
                    if slot.is_none() {
                        *slot = Some(pj);
                        taken[pj] = true;
                        degraded += 1;
                    }
                }
            }
        }
        pi = oi.saturating_add(1);
        pj = nj.saturating_add(1);
    }
    Alignment { map, degraded }
}

fn prefix_suffix_align(a: &[char], b: &[char]) -> Alignment {
    let n = a.len();
    let m = b.len();
    let mut map = vec![None; n];
    let mut p = 0usize;
    while p < n.min(m) && a[p] == b[p] {
        map[p] = Some(p);
        p += 1;
    }
    let mut s = 0usize;
    while s < (n - p).min(m - p) && a[n - 1 - s] == b[m - 1 - s] {
        map[n - 1 - s] = Some(m - 1 - s);
        s += 1;
    }
    Alignment { map, degraded: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ① 大字 / 小字层级 ──
    #[test]
    fn gloss_in_parentheses_is_small() {
        let k = classify_spans("天地（玄黃）宇宙");
        assert_eq!(k[0], SpanKind::Body);
        assert_eq!(k[1], SpanKind::Body);
        for x in &k[2..6] {
            assert_eq!(*x, SpanKind::Gloss, "括号内（含括号）应为夹注小字");
        }
        assert_eq!(k[6], SpanKind::Body);
        assert_eq!(k[7], SpanKind::Body);
    }

    #[test]
    fn note_tags_are_smallest_and_body_untouched() {
        // 下标：0子 1曰 | 2[ 3注 4] | 5習 6， 7鷹 8乃 9學 10習 |
        //       11[ 12/ 13注 14] | 15。
        let s = "子曰[注]習，鷹乃學習[/注]。";
        let k = classify_spans(s);
        assert_eq!(k[0], SpanKind::Body);
        assert_eq!(k[1], SpanKind::Body);
        for i in 2..=4 {
            assert_eq!(k[i], SpanKind::GlossTag, "开壳 [{i}] 是结构标签");
        }
        for i in 5..=10 {
            assert_eq!(k[i], SpanKind::Gloss, "注文 [{i}] 是小字");
        }
        for i in 11..=14 {
            assert_eq!(k[i], SpanKind::GlossTag, "收壳 [{i}] 是结构标签");
        }
        assert_eq!(k[15], SpanKind::Body, "收壳之后必须回到正文大字");
        assert_eq!(k.len(), s.chars().count());
    }

    #[test]
    fn bracket_note_and_nested_gloss() {
        let k = classify_spans("甲[注:乙丙]丁（戊（己）庚）辛");
        assert_eq!(k[0], SpanKind::Body);
        assert_eq!(k[1], SpanKind::GlossTag);
        assert_eq!(k[4], SpanKind::Gloss);
        // `[注:…]` 整段连闭合方括号都算夹注，故 6 号 `]` 也是小字。
        assert_eq!(k[6], SpanKind::Gloss);
        assert_eq!(k[7], SpanKind::Body, "] 之后立刻回到正文大字");
        assert_eq!(k[8], SpanKind::Gloss, "嵌套括注仍按小字");
        assert_eq!(k[k.len() - 1], SpanKind::Body);
    }

    #[test]
    fn half_open_note_never_swallows_body() {
        let s = "甲[注]乙丙";
        let k = classify_spans(s);
        assert_eq!(k.len(), s.chars().count());
        assert_eq!(k[0], SpanKind::Body);
        for i in 1..=3 {
            assert_eq!(k[i], SpanKind::GlossTag);
        }
        assert_eq!(k[4], SpanKind::Body, "半壳不得吞掉后面的正文");
        assert_eq!(k[5], SpanKind::Body, "半壳不得吞掉后面的正文");
    }

    // ── ② 网格下标不漂移 ──
    #[test]
    fn cell_indices_stay_absolute_across_newlines() {
        let text = "甲乙丙\n丁戊";
        let rows = layout_cells(text, 30, false);
        let flat: Vec<(usize, char)> = rows
            .iter()
            .flatten()
            .map(|c| (c.idx, c.ch))
            .collect();
        // '\n' 占下标 3，但不渲染 cell；'丁' 必须是 4 而不是 3。
        assert_eq!(flat, vec![(0, '甲'), (1, '乙'), (2, '丙'), (4, '丁'), (5, '戊')]);
    }

    #[test]
    fn editing_makes_newline_clickable() {
        let rows = layout_cells("甲\n乙", 30, true);
        assert!(rows[0].iter().any(|c| c.newline && c.idx == 1));
        assert_eq!(rows[1][0].idx, 2);
    }

    #[test]
    fn caret_xy_and_index_at_round_trip() {
        let rows = layout_cells("甲乙丙丁\n戊己", 2, true);
        let (r, c) = caret_xy(&rows, 5);
        assert_eq!(index_at(&rows, r, c, 7), 5);
        assert_eq!(caret_xy(&rows, 0), (0, 0));
    }

    // ── ②-b 响应式：拉大拉小都不裁字、不越界 ──
    #[test]
    fn responsive_per_line_never_overflows_any_window() {
        let (fs, pad, gap) = (22.0f32, 28.0f32, 3.0f32);
        let cases = [
            (760.0f32, 380.0f32),  // 极窄
            (900.0, 450.0),        // 窄
            (1184.0, 592.0),       // 默认
            (1400.0, 700.0),       // 宽
            (1920.0, 960.0),       // 超宽
        ];
        let mut prev = 0usize;
        for (vw, left_w) in cases {
            let per = responsive_per_line(vw, left_w, fs, pad, gap);
            let right_w = right_pane_width(vw, left_w);
            // 每行的最大占位必须落在文字区净宽内（留 8px 安全余量）。
            assert!(
                per as f32 * (fs + gap) <= right_w - pad * 2.0,
                "vw={vw} per={per} 会溢出右栏"
            );
            // 格线长度 = 净宽，恒为正且等于派生值。
            let line_w = (right_w - pad * 2.0).max(1.0);
            assert!(line_w > 0.0 && line_w <= right_w);
            assert!(per >= prev, "窗口变宽时每行字数不应变少");
            prev = per;
        }
        assert!(
            responsive_per_line(1400.0, 700.0, fs, pad, gap)
                > responsive_per_line(900.0, 450.0, fs, pad, gap),
            "宽窗口必须比窄窗口排更多字（证明 reflow 真在变）"
        );
    }

    #[test]
    fn zoom_shrinks_chars_per_line_but_still_fits() {
        // 放大字号 = 每行字数减少，但仍不越界（缩放与 resize 同源）。
        let a = responsive_per_line(1184.0, 592.0, 22.0, 28.0, 3.0);
        let b = responsive_per_line(1184.0, 592.0, 44.0, 28.0, 3.0);
        assert!(b < a);
        let usable = right_pane_width(1184.0, 592.0) - 56.0;
        assert!(b as f32 * (44.0 + 3.0) <= usable);
    }

    // ── ③ 编辑 / 撤销 ──
    #[test]
    fn splice_keeps_newline_and_clamps() {
        let (t, c) = splice_chars("甲乙", 1, 1, "\n丙");
        assert_eq!(t, "甲\n丙乙");
        assert_eq!(c, 3);
        let (t, _) = splice_chars("甲乙", 99, 999, "X");
        assert_eq!(t, "甲乙X");
    }

    #[test]
    fn sanitize_ime_keeps_enter_only_when_allowed() {
        assert_eq!(sanitize_ime("你\r好\n\t", true), "你好\n");
        assert_eq!(sanitize_ime("你\r好\n\t", false), "你好");
    }

    #[test]
    fn undo_restores_exact_text_and_cursor() {
        let mut u = UndoStack::new();
        let before = Snapshot { text: "甲".into(), cursor: 1 };
        u.record(before, EditKind::Insert);
        let cur = Snapshot { text: "甲乙".into(), cursor: 2 };
        let prev = u.undo(cur.clone()).unwrap();
        assert_eq!(prev.text, "甲");
        assert_eq!(prev.cursor, 1);
        let again = u.redo(prev).unwrap();
        assert_eq!(again.text, "甲乙");
    }

    #[test]
    fn consecutive_typing_merges_but_barrier_splits() {
        let mut u = UndoStack::new();
        u.record(Snapshot { text: "".into(), cursor: 0 }, EditKind::Insert);
        u.record(Snapshot { text: "甲".into(), cursor: 1 }, EditKind::Insert);
        assert_eq!(u.depth(), 1, "连打字应合并成一步");
        u.barrier();
        u.record(Snapshot { text: "甲乙".into(), cursor: 2 }, EditKind::Insert);
        assert_eq!(u.depth(), 2, "光标移动后的编辑必须自成一档");
    }

    // ── ④ 坐标重映射 ──
    #[test]
    fn same_length_correction_keeps_every_box() {
        // 改错别字：一个字换一个字，红框一个都不许动。
        let al = align_texts("天地玄黃宇宙", "天地玄黄宇宙");
        assert_eq!(al.degraded, 0);
        for (i, x) in al.map.iter().enumerate() {
            assert_eq!(*x, Some(i), "第 {i} 个框必须原位");
        }
    }

    #[test]
    fn one_char_becomes_two_degrades_gracefully() {
        // 「甲」改成「乙丙」：原框留在原位置给「乙」，不崩、不飘。
        let al = align_texts("甲丁", "乙丙丁");
        assert_eq!(al.map.len(), 2);
        assert_eq!(al.map[0], Some(0), "原框降级给新串首字");
        assert_eq!(al.map[1], Some(2), "后面的「丁」仍然精准");
        assert_eq!(al.degraded, 1);
    }

    #[test]
    fn insertion_shifts_following_boxes_exactly() {
        let al = align_texts("甲乙丙", "甲乙X丙");
        assert_eq!(al.map[0], Some(0));
        assert_eq!(al.map[1], Some(1));
        assert_eq!(al.map[2], Some(3));
        assert_eq!(al.degraded, 0);
    }

    #[test]
    fn deletion_disables_only_removed_boxes() {
        let al = align_texts("甲乙丙丁", "甲丁");
        assert_eq!(al.map[0], Some(0));
        assert_eq!(al.map[1], None);
        assert_eq!(al.map[2], None);
        assert_eq!(al.map[3], Some(1));
    }

    #[test]
    fn never_maps_past_new_text_end() {
        let al = align_texts("甲乙丙丁戊", "甲");
        for x in &al.map {
            assert!(x.map(|j| j < 1).unwrap_or(true), "映射不得越界：{x:?}");
        }
    }

    #[test]
    fn empty_texts_never_panic() {
        let al = align_texts("", "甲乙");
        assert!(al.map.is_empty());
        let al = align_texts("甲乙", "");
        assert_eq!(al.map, vec![None, None]);
    }

    // ── L9 · 正注分离 ──
    #[test]
    fn infer_gloss_marks_chapter_title_and_long_sentence() {
        // 真实《道德经王弼注》第 2 页开头：书名 + 章标题 + 经文 + 注文连排。
        let s = "老子道德经王弼注第一章道可道，非常道。名可名，非常名。可道之道，可名之名，指事造形，非其常也。";
        let k = infer_gloss_spans(s);
        assert_eq!(k.len(), s.chars().count());
        assert_eq!(k[0], SpanKind::GlossTag, "书名按结构标签小字");
        let chars: Vec<char> = s.chars().collect();
        let find = |pat: &str| {
            let p: Vec<char> = pat.chars().collect();
            chars.windows(p.len()).position(|w| w == p.as_slice()).unwrap()
        };
        let ti = find("第一章");
        assert_eq!(k[ti], SpanKind::GlossTag, "章标题小字");
        let jing = find("道可道");
        assert_eq!(k[jing], SpanKind::Body, "短经句保持大字正文");
        let zhu = find("指事造形");
        assert_eq!(k[zhu], SpanKind::Gloss, "长解释句降为夹注小字");
    }

    #[test]
    fn resolve_spans_db_clustering_wins_over_inference() {
        let s = "道可道，非常道。名可名，非常名。可道之道，可名之名，指事造形，非其常也。";
        let db: Vec<u8> = vec![1; s.chars().count()];
        let k = resolve_spans(s, &db);
        assert!(k.iter().all(|&x| x == SpanKind::Gloss),
            "有真实字高聚类结果时必须整页采信，不启用体例猜测");
    }

    #[test]
    fn resolve_spans_explicit_tag_beats_db() {
        let s = "甲[注]乙丙[/注]丁";
        let db: Vec<u8> = vec![1; s.chars().count()];
        let k = resolve_spans(s, &db);
        assert_eq!(k[1], SpanKind::GlossTag, "开壳 [注] 是结构标签");
        assert_eq!(k[4], SpanKind::Gloss, "壳内『乙』是夹注小字");
        // 显式标签区不被 DB 改写；Body 区则接受 DB 聚类结果（设计如此）。
        assert_eq!(k[10], SpanKind::Gloss, "壳外 Body 字接受 DB 聚类");
    }

    #[test]
    fn resolve_spans_falls_back_for_plain_text() {
        let k = resolve_spans("道可道，非常道。", &[]);
        // 短句无长解释句时宁可全大字，也不伪造小字。
        assert!(k.iter().all(|&x| x == SpanKind::Body));
    }

    #[test]
    fn layout_cells_with_kinds_keeps_absolute_indices() {
        // 完整壳：半壳不吞字（正文保持大字）是既有红线，这里用完整壳验夹注。
        let s = "甲乙[注]丙丁[/注]";
        let k = resolve_spans(s, &[]);
        let rows = layout_cells_with_kinds(s, 3, true, &k);
        let flat: Vec<usize> = rows.iter().flat_map(|r| r.iter().map(|c| c.idx)).collect();
        assert_eq!(flat, (0..s.chars().count()).collect::<Vec<_>>());
        let c = rows.iter().flatten().find(|c| c.ch == '丙').unwrap();
        assert_eq!(c.kind, SpanKind::Gloss);
    }
}
