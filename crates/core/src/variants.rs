//! **异体字 / 繁简映射层**（v1.9.78）：外部字表导入 + 归一 + 缺字抢救。
//!
//! # 为什么要单独一层
//!
//! `cleaner::normalize_variants` 原来只有 27 对手写异体字（`峯→峰` `羣→群`），
//! 而市面上的古籍整理项目（如 `chinese-philosophy` 的 `cjk/`）自带一份
//! **九万八千字的繁简 / 异体三元组表**。把它接进来，清洗库的词条量从 27
//! 涨到 570，检索归一的词条量从 0 涨到 5600。
//!
//! # 外部表的真实形态（务必先读懂再改）
//!
//! `cjk.txt` 每行**恰好三个汉字**：
//!
//! ```text
//! 与與与     异体=与  繁体=與  简体=与
//! 叡睿睿     异体=叡  繁体=睿  简体=睿
//! 說説说     异体=說  繁体=説  简体=说
//! 經經经     异体=經  繁体=經  简体=经
//! ```
//!
//! 三列位置**恒定**：`col1` 永远是繁体正字，`col2` 永远是简体正字。
//! 98682 行里有 93518 行三字相同——那只是「这个字存在」的收录清单，
//! **不含任何映射信息**，生成基线时已剔除。
//!
//! # 三种映射，三种用途，绝不能混
//!
//! | 映射 | 内容 | 用在哪 | 会不会改正文 |
//! |---|---|---|---|
//! | [`VariantTable::to_trad`] | 任意写法 → 繁体正字 | 检索 / 跨版本比对 | **不会** |
//! | [`VariantTable::to_simp`] | 任意写法 → 简体正字 | 检索 / 跨版本比对 | **不会** |
//! | [`VariantTable::safe_fold`] | 异体 → 正字（同体系无歧义） | 清洗写回正文 | 会 |
//!
//! `safe_fold` 只收「`繁体==简体`」那一类（表里 550 条）——此时正字没有
//! 繁简分歧，`叡→睿` `卽→即` 折过去是**字形归一**，不是简繁转换。凡是
//! `經→经` 这种真·简化，一律不进 `safe_fold`：用户读繁体书，正文就得是繁体。
//!
//! # 一简对多繁：宁可不动，绝不猜
//!
//! 表里有 9 个字是「一个简体对应多个繁体」——`发→發/髮`、`钟→鍾/鐘`、
//! `苏→蘇/囌`、`舍→捨/舍`。这种情况**任何自动转换都是错的**，因为选哪个
//! 要看上下文（「头发」不是「头發」）。本模块一律**丢弃并上报**
//! （[`ImportReport::conflicts`]），不做启发式猜测。少一条映射只是少一次
//! 归一，猜错一个就是错字。
//!
//! # 外部字典导入
//!
//! [`VariantTable::parse`] 自动嗅探五种格式，见 [`DictFormat`]：
//! 三元组、二元组、分隔符对、JSON 对象、JSON 数组。用户把自己的表丢进来
//! 即可，不需要转格式。

use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::glyph::GlyphSet;

// ------------------------------------------------------------------
// 一、手写白的异体字（v1.54.0 起的既有资产，保留并优先）
// ------------------------------------------------------------------

/// 手工校过的异体字对（`异体 → 简体正字`）。
///
/// 这批是**人眼过了一遍**的，即便外部表里有冲突也以这里为准。外部表虽然
/// 量大，但它是机器生成的；27 对手写条目是古籍样张实测出来的。
pub const HAND_PICKED: &[(char, char)] = &[
    ('峯', '峰'),
    ('峩', '峨'),
    ('羣', '群'),
    ('敎', '教'),
    ('効', '效'),
    ('卽', '即'),
    ('皁', '皂'),
    ('旣', '既'),
    ('卻', '却'),
    ('畧', '略'),
    ('冦', '寇'),
    ('厯', '历'),
    ('喆', '哲'),
    ('敍', '叙'),
    ('敡', '扬'),
    ('煑', '煮'),
    ('稾', '稿'),
    ('覩', '睹'),
    ('劒', '剑'),
    ('虵', '蛇'),
    ('壻', '婿'),
    ('聦', '聪'),
    ('茲', '兹'),
    ('卹', '恤'),
    ('袐', '秘'),
    ('甯', '宁'),
    ('氷', '冰'),
];

// ------------------------------------------------------------------
// 二、外部表解析
// ------------------------------------------------------------------

/// 嗅探出来的外部表格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DictFormat {
    /// 每行恰好三个汉字：异体 / 繁体 / 简体（`chinese-philosophy` 风格）。
    Triple,
    /// 每行恰好两个汉字：异体 / 正字。
    Pair,
    /// `A<TAB|,|;|=|→|:>B`，每行一对。
    Delimited,
    /// JSON 对象：`{"A":"B"}`（也认 `{"variants":{...}}` 这层壳）。
    JsonObject,
    /// JSON 数组：`[["A","B"], ...]`。
    JsonArray,
    /// 一个字都认不出来。
    #[default]
    Unknown,
}

impl DictFormat {
    pub fn label(self) -> &'static str {
        match self {
            DictFormat::Triple => "三元组（异/繁/简）",
            DictFormat::Pair => "二元组（异/正）",
            DictFormat::Delimited => "分隔符对",
            DictFormat::JsonObject => "JSON 对象",
            DictFormat::JsonArray => "JSON 数组",
            DictFormat::Unknown => "无法识别",
        }
    }
}

/// 一条原始映射。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// 异体 / 繁体 / 简体。
    Triple(char, char, char),
    /// 异体 / 正字。
    Pair(char, char),
}

/// 解析结果。**永不 panic、永不返回 `Err`**——脏数据只记账，不中断导入。
#[derive(Clone, Debug, Default)]
pub struct ParseOutcome {
    pub rows: Vec<Row>,
    pub format: Option<DictFormat>,
    /// 认不出来被跳过的行数。
    pub skipped: usize,
    /// 前几条被跳过的原文，供 UI 提示「你的表第几行长这样」。
    pub samples: Vec<String>,
}

impl ParseOutcome {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn format(&self) -> DictFormat {
        self.format.unwrap_or(DictFormat::Unknown)
    }
}

/// 是不是汉字（含扩展区 / 兼容区 / 私用区——**私造字也要能进表**）。
pub fn is_han(c: char) -> bool {
    let u = c as u32;
    (0x3400..=0x4DBF).contains(&u)          // 扩展 A
        || (0x4E00..=0x9FFF).contains(&u)   // 基本区
        || (0xF900..=0xFAFF).contains(&u)   // 兼容表意文字
        || (0x20000..=0x3FFFF).contains(&u) // 扩展 B–H
        || (0xE000..=0xF8FF).contains(&u)   // 私用区（私造字）
        || (0xF0000..=0x10FFFD).contains(&u) // 私用区补充平面（厂商私造字）
}

/// 单个「字」：允许带一层括号 / 书名的装饰，取其中唯一的汉字。
fn one_han(tok: &str) -> Option<char> {
    let mut it = tok.chars().filter(|c| is_han(*c));
    let f = it.next()?;
    if it.next().is_some() {
        return None; // 不止一个字，判不了
    }
    Some(f)
}

/// 一行里的候选分隔符。**顺序即优先级**：制表符最不可能出现在字里。
const SEPARATORS: &[char] = &['\t', ',', '，', ';', '；', '=', '→', ':', '：', '|'];

fn strip_comment(line: &str) -> &str {
    // 只认行首注释，避免把 `A#B` 这种脏数据截断成 `A`。
    let t = line.trim_start();
    if t.starts_with('#') || t.starts_with("//") {
        return "";
    }
    line
}

/// 逐行嗅探。空行 / 注释 / 表头一律跳过。
fn parse_lines(text: &str) -> ParseOutcome {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text); // BOM
    let mut out = ParseOutcome::default();
    let mut fmt_votes: HashMap<&'static str, usize> = HashMap::new();

    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        // 表头：`异体 繁体 简体` / `from,to` 之类——含非汉字又认不出分隔就丢。
        if let Some(row) = parse_one_line(line) {
            match row {
                Row::Triple(..) => *fmt_votes.entry("t").or_insert(0) += 1,
                Row::Pair(..) => *fmt_votes.entry("p").or_insert(0) += 1,
            }
            out.rows.push(row);
        } else {
            out.skipped += 1;
            if out.samples.len() < 5 {
                out.samples.push(raw.trim().chars().take(40).collect());
            }
        }
    }

    // 分隔符对 vs 纯二元组：只要过半的行带分隔符，就报 Delimited。
    let delimited = text
        .lines()
        .filter(|l| {
            let l = strip_comment(l).trim();
            !l.is_empty() && SEPARATORS.iter().any(|s| l.contains(*s))
        })
        .count();
    let total = text.lines().filter(|l| !strip_comment(l).trim().is_empty()).count().max(1);

    out.format = if out.rows.is_empty() {
        None
    } else if delimited * 2 > total {
        Some(DictFormat::Delimited)
    } else if fmt_votes.get("t").copied().unwrap_or(0) > fmt_votes.get("p").copied().unwrap_or(0) {
        Some(DictFormat::Triple)
    } else {
        Some(DictFormat::Pair)
    };
    out
}

fn parse_one_line(line: &str) -> Option<Row> {
    // ---- 1) 带分隔符 ----
    if let Some(sep) = SEPARATORS.iter().find(|s| line.contains(**s)) {
        let parts: Vec<&str> = line.split(*sep).map(str::trim).filter(|p| !p.is_empty()).collect();
        return match parts.len() {
            0 | 1 => None,
            2 => {
                let (a, b) = (one_han(parts[0])?, one_han(parts[1])?);
                Some(Row::Pair(a, b))
            }
            _ => {
                // 三列以上：按「异/繁/简」取前三列，多余列（备注 / 读音）忽略。
                let (a, b, c) = (one_han(parts[0])?, one_han(parts[1])?, one_han(parts[2])?);
                Some(Row::Triple(a, b, c))
            }
        };
    }

    // ---- 2) 纯汉字行：2 字 = 对，3 字 = 三元组 ----
    let hans: Vec<char> = line.chars().filter(|c| is_han(*c)).collect();
    let non_han = line.chars().filter(|c| !is_han(*c) && !c.is_whitespace()).count();
    if non_han > 0 {
        return None; // 「說明：見上」这种正文行，不是映射
    }
    match hans.len() {
        2 => Some(Row::Pair(hans[0], hans[1])),
        // 三字相同不是「认不出来」，是「这个字存在但没有映射信息」——
        // 收进 rows 交给 `absorb` 记成 identity，否则导入一份 9 万字表
        // 会报「跳过 93518 行」，把用户吓一跳。
        3 => Some(Row::Triple(hans[0], hans[1], hans[2])),
        _ => None,
    }
}

/// JSON 导入：对象 / 数组两种都吃，嵌套一层壳（`{"variants": …}`）也认。
fn parse_json(text: &str) -> Option<ParseOutcome> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut out = ParseOutcome::default();

    fn collect(v: &serde_json::Value, out: &mut ParseOutcome, depth: usize) {
        if depth > 3 {
            return;
        }
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    if let Some(s) = val.as_str() {
                        if let (Some(a), Some(b)) = (one_han(k), one_han(s)) {
                            if a != b {
                                out.rows.push(Row::Pair(a, b));
                            }
                            continue;
                        }
                    }
                    // 值不是字符串 → 当作一层壳继续往下翻（`{"variants":{...}}`）。
                    collect(val, out, depth + 1);
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    match item {
                        serde_json::Value::Array(pair) if pair.len() >= 2 => {
                            let a = pair[0].as_str().and_then(one_han);
                            let b = pair[1].as_str().and_then(one_han);
                            if let (Some(a), Some(b)) = (a, b) {
                                if a != b {
                                    out.rows.push(Row::Pair(a, b));
                                }
                            }
                        }
                        other => collect(other, out, depth + 1),
                    }
                }
            }
            _ => {}
        }
    }
    collect(&v, &mut out, 0);
    if out.rows.is_empty() {
        return None;
    }
    out.format = Some(if v.is_array() { DictFormat::JsonArray } else { DictFormat::JsonObject });
    Some(out)
}

// ------------------------------------------------------------------
// 三、映射表
// ------------------------------------------------------------------

/// 三元映射表。三张表的用途严格分工，见模块文档的表格。
#[derive(Clone, Debug, Default)]
pub struct VariantTable {
    /// 任意写法 → **繁体**正字。只用于检索 / 比对，**不写回正文**。
    pub to_trad: HashMap<char, char>,
    /// 任意写法 → **简体**正字。只用于检索 / 比对，**不写回正文**。
    pub to_simp: HashMap<char, char>,
    /// 异体 → 正字，**同体系、无繁简分歧**。这一张才允许写回正文。
    pub safe_fold: HashMap<char, char>,
    /// 一简对多繁这类**拒绝自动判定**的字，供 UI 解释「为什么这个字没折」。
    pub conflicts: Vec<(char, char, char)>,
    /// 歧义字黑名单。**表里三张表都不许再收录它**——
    /// 否则「发→發」和「发→髮」谁排在后面谁赢，等于偷偷替用户猜了一个
    /// 上下文。猜错一个字，比不折这个字严重得多。
    pub ambiguous: HashSet<char>,
}

/// 导入账本：让用户看见「进了多少、丢了多少、为什么丢」。
#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub format: DictFormat,
    /// 读进来的原始行数。
    pub parsed: usize,
    /// 真正进了某张表的映射条数（一条可能进两张表）。
    pub accepted: usize,
    /// 一简对多繁 —— 拒绝判定。
    pub conflicts: Vec<(char, char, char)>,
    /// 无信息（`A→A`）直接丢掉的条数。
    pub identity: usize,
    /// 认不出的行数。
    pub skipped: usize,
    /// 认不出的原文样例。
    pub samples: Vec<String>,
}

impl ImportReport {
    /// 一句话说清结果，直接能给 UI 用。
    pub fn summary(&self) -> String {
        let mut s = format!("{}：收录 {} 条", self.format.label(), self.accepted);
        if !self.conflicts.is_empty() {
            s.push_str(&format!("，一简对多繁 {} 字未折算", self.conflicts.len()));
        }
        if self.skipped > 0 {
            s.push_str(&format!("，跳过 {} 行", self.skipped));
        }
        s
    }
}

impl VariantTable {
    /// 内核基线：`data/variant_cjk.txt` 三元组 + [`HAND_PICKED`]。
    pub fn builtin() -> &'static VariantTable {
        static TABLE: Lazy<VariantTable> = Lazy::new(|| {
            let mut t = VariantTable::default();
            t.absorb(&parse_lines(include_str!("data/variant_cjk.txt")), true);
            // 手写表最后灌、且**覆盖**外部表：人校过的优先。
            // 但歧义黑名单优先于一切——不许因为手滑把「发」又放回来。
            for &(a, b) in HAND_PICKED {
                if !t.ambiguous.contains(&a) {
                    t.safe_fold.insert(a, b);
                }
            }
            t
        });
        &TABLE
    }

    /// 空表（测试 / 用户自建）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 表的规模，供状态栏 / 报告展示。
    pub fn len(&self) -> usize {
        self.to_trad.len() + self.to_simp.len() + self.safe_fold.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 把解析结果灌进三张表。
    ///
    /// `overwrite=false` 时先到先得（同一文件内冲突丢弃并记账）；
    /// `overwrite=true` 时后到覆盖（用户显式导入 = 他说了算）。
    pub fn absorb(&mut self, outcome: &ParseOutcome, overwrite: bool) -> ImportReport {
        let mut rep = ImportReport {
            format: outcome.format(),
            parsed: outcome.rows.len(),
            skipped: outcome.skipped,
            samples: outcome.samples.clone(),
            ..Default::default()
        };
        for row in &outcome.rows {
            match *row {
                Row::Triple(a, b, c) => {
                    // 三列位置恒定：b=繁体正字，c=简体正字。
                    if a == b && b == c {
                        rep.identity += 1;
                        continue;
                    }
                    put_map(&mut self.to_trad, &mut self.ambiguous, a, b, overwrite, &mut rep);
                    put_map(&mut self.to_trad, &mut self.ambiguous, c, b, overwrite, &mut rep);
                    put_map(&mut self.to_simp, &mut self.ambiguous, a, c, overwrite, &mut rep);
                    put_map(&mut self.to_simp, &mut self.ambiguous, b, c, overwrite, &mut rep);
                    // 只有「繁简同形」才是无歧义字形归一 —— 唯一可写回正文的一类。
                    if b == c {
                        put_map(&mut self.safe_fold, &mut self.ambiguous, a, b, overwrite, &mut rep);
                    }
                }
                Row::Pair(a, b) => {
                    if a == b {
                        rep.identity += 1;
                        continue;
                    }
                    // 二元组语义是「异体 → 正字」，用户自己的表，直接进清洗层。
                    put_map(&mut self.safe_fold, &mut self.ambiguous, a, b, overwrite, &mut rep);
                }
            }
        }
        // 冲突要留在表上，否则 `is_ambiguous` 查不到「为什么这个字没折」。
        for c in &rep.conflicts {
            if !self.conflicts.contains(c) {
                self.conflicts.push(*c);
            }
        }
        rep
    }

    /// **纯函数**：嗅探并解析外部字典文本。JSON 先试，失败再按行解析。
    pub fn parse(text: &str) -> ParseOutcome {
        let t = text.trim_start_matches('\u{feff}').trim_start();
        if t.starts_with('{') || t.starts_with('[') {
            if let Some(o) = parse_json(t) {
                return o;
            }
        }
        parse_lines(text)
    }

    /// 从外部文本建一张**独立的**表（不碰内置基线），再决定要不要 [`merge`]。
    ///
    /// [`merge`]: VariantTable::merge
    pub fn from_text(text: &str) -> (VariantTable, ImportReport) {
        let outcome = Self::parse(text);
        let mut t = VariantTable::default();
        let rep = t.absorb(&outcome, true);
        (t, rep)
    }

    /// 并进当前表（用户导入路径：`builtin().clone().merge(&user)`）。
    pub fn merge(&mut self, other: &VariantTable) -> ImportReport {
        let mut rep = ImportReport { format: DictFormat::JsonObject, ..Default::default() };
        rep.conflicts = other.conflicts.clone();
        // 用户表的歧义黑名单必须一并继承，否则合并后又把禁用字放回来。
        self.ambiguous.extend(other.ambiguous.iter().copied());
        for (k, v) in &other.to_trad {
            if !self.ambiguous.contains(k) {
                self.to_trad.insert(*k, *v);
                rep.accepted += 1;
            }
        }
        for (k, v) in &other.to_simp {
            if !self.ambiguous.contains(k) {
                self.to_simp.insert(*k, *v);
                rep.accepted += 1;
            }
        }
        for (k, v) in &other.safe_fold {
            if !self.ambiguous.contains(k) {
                self.safe_fold.insert(*k, *v);
                rep.accepted += 1;
            }
        }
        for k in &self.ambiguous {
            self.to_trad.remove(k);
            self.to_simp.remove(k);
            self.safe_fold.remove(k);
        }
        rep
    }

    // ---- 查询 ----

    /// 异体 → 正字（**唯一可以写回正文的折叠**）。链式最多走 4 跳，防环。
    pub fn fold(&self, c: char) -> char {
        walk(&self.safe_fold, c)
    }

    /// 任意写法 → 繁体正字（检索用）。
    pub fn trad(&self, c: char) -> char {
        walk(&self.to_trad, c)
    }

    /// 任意写法 → 简体正字（检索用）。
    pub fn simp(&self, c: char) -> char {
        walk(&self.to_simp, c)
    }

    /// 全串异体归一（清洗写回用的就是它）。
    pub fn fold_str(&self, s: &str) -> String {
        s.chars().map(|c| self.fold(c)).collect()
    }

    /// 全串 → 简体正字（检索 / 比对键）。
    pub fn simp_str(&self, s: &str) -> String {
        s.chars().map(|c| self.simp(c)).collect()
    }

    /// 全串 → 繁体正字（检索 / 比对键）。
    pub fn trad_str(&self, s: &str) -> String {
        s.chars().map(|c| self.trad(c)).collect()
    }

    /// 检索键：异体先归一，再落到简体正字。两本书一繁一简、一篇用异体，
    /// 走完它就能对上。
    pub fn search_key(&self, s: &str) -> String {
        s.chars()
            .map(|c| {
                let c = self.fold(c);
                self.simp(c)
            })
            .collect()
    }

    /// 这个字是否属于「一简对多繁」的歧义字（**整条禁用**，不许自动折算）。
    pub fn is_ambiguous(&self, c: char) -> bool {
        self.ambiguous.contains(&c)
    }
}

fn walk(map: &HashMap<char, char>, c: char) -> char {
    let mut cur = c;
    for _ in 0..4 {
        match map.get(&cur) {
            Some(&n) if n != cur => cur = n,
            _ => break,
        }
    }
    cur
}

/// 往一张映射表里放一条映射。**歧义即整条禁用**。
///
/// `发→發` 与 `发→髮` 撞上时，不是「先到先得」也不是「后到先得」——
/// 两种都是在替用户猜上下文，而上下文只有读者知道。这里的处理是
/// 把 `发` 拉进黑名单、从表里删掉，谁都不折。
fn put_map(
    map: &mut HashMap<char, char>,
    banned: &mut HashSet<char>,
    k: char,
    v: char,
    _overwrite: bool,
    rep: &mut ImportReport,
) {
    if k == v || banned.contains(&k) {
        return;
    }
    match map.get(&k) {
        Some(&old) if old != v => {
            if !rep.conflicts.iter().any(|(x, y, z)| *x == k && (*y == v || *z == v)) {
                rep.conflicts.push((k, v, old));
            }
            banned.insert(k);
            map.remove(&k);
            rep.accepted = rep.accepted.saturating_sub(1);
        }
        Some(_) => {}
        None => {
            map.insert(k, v);
            rep.accepted += 1;
        }
    }
}

// ------------------------------------------------------------------
// 四、缺字抢救（豆腐块）
// ------------------------------------------------------------------

/// 缺字抢救的结果。
#[derive(Clone, Debug, Default)]
pub struct RescueReport {
    /// 成功折回、字体里有字形的字数。
    pub rescued: Vec<(char, char)>,
    /// 折了也没有字形的字（映射表救不了，只有换字体能救）。
    pub hopeless: Vec<char>,
    /// 根本没进表、字体也没有的字（真·生僻字，需要人工）。
    pub unknown: Vec<char>,
}

impl RescueReport {
    pub fn has_any(&self) -> bool {
        !self.rescued.is_empty() || !self.hopeless.is_empty() || !self.unknown.is_empty()
    }
}

/// **只在字体确实缺字形时**才把字折成正字——不碰任何能正常显示的字。
///
/// 这是「映射表能减少豆腐块」唯一正确的用法：`叡` 字体没有、`睿` 有，就显示
/// `睿`；`與` 字体有，就老老实实显示 `與`，绝不因为它是繁体就换成 `与`。
///
/// **默认严格档：只走 `safe_fold`（异体归一），绝不走繁简映射。**
/// [`rescue_missing_display`] 才允许繁简降级，且只准用于屏幕渲染，
/// 绝不能写回书稿——古籍里把 `衆` 悄悄改成 `众` 属于数据损坏。
///
/// 折不了的字**原样保留**并记进 [`RescueReport`]，绝不瞎换成别的字——
/// 一个错字比一个豆腐块严重得多。
pub fn rescue_missing(s: &str, glyphs: &GlyphSet, table: &VariantTable) -> (String, RescueReport) {
    rescue_missing_with(s, glyphs, table, false)
}

/// **显示层专用**的宽松抢救：额外允许 `繁→简` / `简→繁` 折算。
///
/// ⚠️ 返回的字符串**只能喂给渲染器做临时替换**，绝不可以存回 sqlite、
/// 写进导出 PDF 的正文，或参与字数 / 校勘比对。它改变的是字形，不是异写。
pub fn rescue_missing_display(
    s: &str,
    glyphs: &GlyphSet,
    table: &VariantTable,
) -> (String, RescueReport) {
    rescue_missing_with(s, glyphs, table, true)
}

fn rescue_missing_with(
    s: &str,
    glyphs: &GlyphSet,
    table: &VariantTable,
    allow_script_fold: bool,
) -> (String, RescueReport) {
    let mut out = String::with_capacity(s.len());
    let mut rep = RescueReport::default();
    let mut seen: HashSet<char> = HashSet::new();
    for c in s.chars() {
        if c.is_whitespace() || !is_han(c) || glyphs.contains(c) {
            out.push(c);
            continue;
        }
        // 字体没有这个字：先找一个「同体系异体归一」后有字形的正字。
        let mut cand = {
            let f = table.fold(c);
            (f != c && glyphs.contains(f)).then_some(f)
        };
        if cand.is_none() && allow_script_fold {
            let s2 = table.simp(c);
            cand = (s2 != c && glyphs.contains(s2)).then_some(s2);
            if cand.is_none() {
                let t2 = table.trad(c);
                cand = (t2 != c && glyphs.contains(t2)).then_some(t2);
            }
        }
        let in_table = cand.is_some()
            || table.fold(c) != c
            || (allow_script_fold && (table.simp(c) != c || table.trad(c) != c));
        match cand {
            Some(t) => {
                if seen.insert(c) {
                    rep.rescued.push((c, t));
                }
                out.push(t);
            }
            None => {
                if seen.insert(c) {
                    if in_table {
                        rep.hopeless.push(c);
                    } else {
                        rep.unknown.push(c);
                    }
                }
                out.push(c);
            }
        }
    }
    (out, rep)
}

/// 扫出一段文本里**字体画不出来**的字（去重、保序）。给「导入前体检」用。
pub fn tofu_chars(s: &str, glyphs: &GlyphSet) -> Vec<char> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in s.chars() {
        if is_han(c) && !glyphs.contains(c) && seen.insert(c) {
            out.push(c);
        }
    }
    out
}

// ------------------------------------------------------------------
// 五、用户字典：落盘 + 激活表
// ------------------------------------------------------------------

/// 外部字典按字节读入时的编码还原。
///
/// 中文古籍字表有一大半是 GB18030 / Big5 老文件，直接按 UTF-8 读会整片变
/// 问号。判据跟 `txt_parser::decode_bytes` 同口径：UTF-8 能通过且无替换符
/// 就用 UTF-8，否则按错误数挑一个多字节编码。
pub fn decode_bytes(bytes: &[u8]) -> (String, &'static str) {
    let b = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(&bytes[2..]);
        return (s.into_owned(), "utf-16le");
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(&bytes[2..]);
        return (s.into_owned(), "utf-16be");
    }
    if let Ok(s) = std::str::from_utf8(b) {
        return (s.to_string(), "utf-8");
    }
    let cands: [(&'static encoding_rs::Encoding, &'static str); 4] = [
        (encoding_rs::GB18030, "gb18030"),
        (encoding_rs::BIG5, "big5"),
        (encoding_rs::SHIFT_JIS, "shift_jis"),
        (encoding_rs::EUC_KR, "euc-kr"),
    ];
    let mut best: Option<(String, &'static str, usize)> = None;
    for (enc, name) in cands {
        let (s, _, bad) = enc.decode(b);
        let bad_n = if bad { 1 } else { 0 };
        // 只数替换符 U+FFFD，坏字节越多排名越后。
        let score = s.chars().filter(|c| *c == '\u{fffd}').count() + bad_n * 256;
        if best.as_ref().is_none_or(|(_, _, n)| score < *n) {
            best = Some((s.into_owned(), name, score));
        }
    }
    best.map(|(s, n, _)| (s, n)).unwrap_or_else(|| {
        (String::from_utf8_lossy(b).into_owned(), "lossy")
    })
}

/// 用户自己的异体字表。文件放在 `Library::home()`，与书架索引同目录，
/// App Sandbox 下由系统透明重定向，不需要判环境。
pub mod user_dict {
    use super::*;
    use std::path::PathBuf;

    /// 用户表文件名。**保留用户原文**（不存解析后的二进制），这样他随时能
    /// 拿文本编辑器打开改，也方便回滚。
    pub fn path() -> PathBuf {
        crate::library::Library::home().join("variants.txt")
    }

    /// 读回用户表原文；没导入过就返回 `None`。
    pub fn text() -> Option<String> {
        let bytes = std::fs::read(path()).ok()?;
        Some(decode_bytes(&bytes).0)
    }

    /// 导入一份外部字典字节：**先解析验一遍，验不过绝不覆盖已有表**。
    ///
    /// 这是防止用户手滑拖错文件把好表冲掉的关键——一行字都认不出来就
    /// 返回 `Err`，磁盘上的旧表原封不动。
    pub fn import_bytes(bytes: &[u8]) -> Result<ImportReport, String> {
        let src = decode_bytes(bytes).0;
        let outcome = VariantTable::parse(&src);
        if outcome.rows.is_empty() {
            return Err(format!(
                "没认出任何映射（{}）。支持的格式：三字一行「异繁简」、两字一行、A<Tab>B、A=B、JSON。\n样例：{}",
                outcome.format().label(),
                outcome.samples.first().cloned().unwrap_or_default()
            ));
        }
        let (_probe, rep) = VariantTable::from_text(&src);
        let dir = crate::library::Library::home();
        std::fs::create_dir_all(&dir).map_err(|e| format!("建目录失败：{e}"))?;
        // 先写临时文件再改名，避免写一半掉电把表写坏。
        let tmp = dir.join("variants.txt.tmp");
        std::fs::write(&tmp, src.as_bytes()).map_err(|e| format!("写表失败：{e}"))?;
        std::fs::rename(&tmp, path()).map_err(|e| format!("落盘失败：{e}"))?;
        reload();
        Ok(rep)
    }

    /// 删掉用户表（恢复纯内置基线）。
    pub fn clear() -> bool {
        let ok = std::fs::remove_file(path()).is_ok();
        reload();
        ok
    }

    /// 用户表当前状态（供设置面板显示）。
    pub fn status() -> String {
        match text() {
            None => "未导入外部字表（使用内置基线）".to_string(),
            Some(t) => {
                let o = VariantTable::parse(&t);
                format!(
                    "外部字表：{}，{} 行｜{}",
                    path().file_name().unwrap_or_default().to_string_lossy(),
                    o.rows.len(),
                    o.format().label()
                )
            }
        }
    }
}

/// 运行时激活表 = 内置基线 ∪ 用户导入表。
static ACTIVE: Lazy<RwLock<Arc<VariantTable>>> = Lazy::new(|| {
    let mut t = VariantTable::builtin().clone();
    if let Some(text) = user_dict::text() {
        let (ext, _) = VariantTable::from_text(&text);
        t.merge(&ext);
    }
    RwLock::new(Arc::new(t))
});

/// 取一份当前激活表的快照（`Arc`，O(1)）。
///
/// 清洗一整本书时**只在开头取一次**，然后逐字查表——绝不每字都读一次锁，
/// 更绝不每字都读一次磁盘。
pub fn active() -> Arc<VariantTable> {
    ACTIVE.read().map(|g| g.clone()).unwrap_or_else(|_| {
        // 锁被投毒（极端情况）也不能让导入流程崩：退回纯内置基线。
        Arc::new(VariantTable::builtin().clone())
    })
}

/// 用户导入 / 删除表之后重建激活表。
pub fn reload() {
    let mut t = VariantTable::builtin().clone();
    if let Some(text) = user_dict::text() {
        let (ext, _) = VariantTable::from_text(&text);
        t.merge(&ext);
    }
    if let Ok(mut g) = ACTIVE.write() {
        *g = Arc::new(t);
    }
}

/// 当前激活表的描述（设置面板 / 报告用）。
pub fn active_summary() -> String {
    let t = active();
    format!(
        "异体归一 {} 条｜繁体正字 {} 条｜简体正字 {} 条｜歧义拒判 {} 字",
        t.safe_fold.len(),
        t.to_trad.len(),
        t.to_simp.len(),
        t.ambiguous.len()
    )
}

// ------------------------------------------------------------------
// 六、测试
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_is_not_empty_and_hand_picked_survive() {
        let t = VariantTable::builtin();
        assert!(t.safe_fold.len() > 400, "safe_fold 太小：{}", t.safe_fold.len());
        assert!(t.to_trad.len() > 3000);
        assert!(t.to_simp.len() > 3000);
        // 手写表必须压过外部表。
        assert_eq!(t.fold('峯'), '峰');
        assert_eq!(t.fold('羣'), '群');
    }

    #[test]
    fn safe_fold_never_does_simplification() {
        let t = VariantTable::builtin();
        // 正体繁体一个都不许动：读繁体书，正文就得是繁体。
        for c in "道德經與學問為無觀".chars() {
            assert_eq!(t.fold(c), c, "safe_fold 不该折繁体正字 {c}");
        }
        // 但检索键要能落成简体。
        assert_eq!(t.search_key("道德經"), "道德经");
    }

    #[test]
    fn one_to_many_trad_is_refused_not_guessed() {
        let t = VariantTable::builtin();
        // 发 → 發 / 髮 两个都合法，选哪个看上下文，因此都不许自动折。
        assert!(t.is_ambiguous('发') || t.to_trad.get(&'发').is_none());
        // 钟 → 鍾 / 鐘 同理。危险的是「随便挑一个」。
        let mut t2 = VariantTable::empty();
        let o = VariantTable::parse("发\t發\n发\t髮\n");
        let rep = t2.absorb(&o, false);
        assert_eq!(t2.safe_fold.get(&'发'), None, "歧义字必须整条禁用，不许猜");
        assert!(!rep.conflicts.is_empty(), "冲突必须上报");
        assert!(t2.is_ambiguous('发'), "冲突必须留在表上供 UI 解释");
        assert_eq!(t2.fold('发'), '发', "正文里的「发」原样保留");
    }

    #[test]
    fn parses_this_projects_triple_format() {
        let text = "# 注释行\n与與与\n叡睿睿\n說説说\n\n###\n";
        let o = VariantTable::parse(text);
        assert_eq!(o.format(), DictFormat::Triple);
        assert_eq!(o.rows.len(), 3);
        assert_eq!(o.rows[0], Row::Triple('与', '與', '与'));
    }

    #[test]
    fn parses_pair_tsv_csv_and_equals() {
        for (txt, fmt) in [
            ("說\t説\n車\t车\n", DictFormat::Delimited),
            ("說,説\n車,车\n", DictFormat::Delimited),
            ("說=説\n車=车\n", DictFormat::Delimited),
            ("說→説\n車→车\n", DictFormat::Delimited),
            ("說：説\n車：车\n", DictFormat::Delimited),
            ("說説\n車车\n", DictFormat::Pair),
        ] {
            let o = VariantTable::parse(txt);
            assert_eq!(o.format(), fmt, "格式判错：{txt:?}");
            assert_eq!(o.rows.len(), 2, "行数不对：{txt:?}");
            assert_eq!(o.rows[0], Row::Pair('說', '説'));
        }
    }

    #[test]
    fn parses_json_object_and_array_and_nested() {
        let o = VariantTable::parse(r#"{"說":"説","車":"车"}"#);
        assert_eq!(o.format(), DictFormat::JsonObject);
        assert_eq!(o.rows.len(), 2);

        let o = VariantTable::parse(r#"[["說","説"],["車","车"]]"#);
        assert_eq!(o.format(), DictFormat::JsonArray);
        assert_eq!(o.rows.len(), 2);

        // 一层壳也要能翻进去。
        let o = VariantTable::parse(r#"{"meta":{"v":1},"variants":{"說":"説"}}"#);
        assert_eq!(o.rows.len(), 1);
        assert_eq!(o.rows[0], Row::Pair('說', '説'));
    }

    #[test]
    fn dirty_lines_never_panic_and_are_counted() {
        let junk = "这不是映射表\n说明：见上\n\n\u{feff}\n、、、\n😀😀\n";
        let o = VariantTable::parse(junk);
        assert!(o.rows.is_empty());
        assert!(o.skipped > 0);
    }

    #[test]
    fn external_table_extension_works() {
        let mut t = VariantTable::builtin().clone();
        let before = t.safe_fold.len();
        let (ext, rep) = VariantTable::from_text("𠀀\t一\n");
        assert_eq!(rep.accepted, 1, "{}", rep.summary());
        t.merge(&ext);
        assert_eq!(t.safe_fold.len(), before + 1);
        assert_eq!(t.fold('𠀀'), '一');
    }

    #[test]
    fn fold_chain_stops_and_does_not_loop() {
        let mut t = VariantTable::empty();
        t.safe_fold.insert('甲', '乙');
        t.safe_fold.insert('乙', '丙');
        assert_eq!(t.fold('甲'), '丙');
        t.safe_fold.insert('丙', '甲'); // 人为造环
        let _ = t.fold('甲'); // 不许死循环
    }

    #[test]
    fn is_han_covers_extension_b_and_private_use() {
        assert!(is_han('一'));
        assert!(is_han('㐀'));       // 扩展 A
        assert!(is_han('𠀀'));       // 扩展 B
        assert!(is_han('\u{f0000}')); // 私用区补充
        assert!(!is_han('a'));
        assert!(!is_han('。'));
    }

    #[test]
    fn decode_bytes_recognizes_bom_and_gb18030() {
        assert_eq!(decode_bytes("\u{feff}說説".as_bytes()).0, "說説");
        let (gb, _, _) = encoding_rs::GB18030.encode("說説说\n叡睿睿\n");
        let t = decode_bytes(gb.as_ref());
        assert_eq!(t.1, "gb18030", "老字表的编码没认出来");
        assert!(t.0.contains("說説说"));
    }

    #[test]
    fn decode_bytes_never_panics_on_garbage() {
        for n in [1usize, 3, 17, 4096] {
            let v: Vec<u8> = (0..n).map(|i| (i * 91 % 256) as u8).collect();
            let _ = decode_bytes(&v);
        }
    }

    /// 拿用户手上那份真实 `cjk/cjk.txt` 走一遍完整导入链。
    /// 设 `JIANDU_CJK_TABLE=/path/to/cjk.txt` 才会跑，别的机器自动跳过。
    #[test]
    fn imports_the_real_cjk_table_end_to_end() {
        let Ok(p) = std::env::var("JIANDU_CJK_TABLE") else {
            eprintln!("跳过：未设置 JIANDU_CJK_TABLE");
            return;
        };
        let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("读不到 {p}：{e}"));
        let (text, _enc) = decode_bytes(&bytes);
        let outcome = VariantTable::parse(&text);
        assert_eq!(outcome.format(), DictFormat::Triple);
        // 98682 行全是合法三元组；其中 93518 行三字相同 = 纯收录清单，
        // 记成 identity（不是「认不出来」），5164 行才有真映射。
        assert_eq!(outcome.rows.len(), 98682, "原始行数对不上");
        let (t, rep) = VariantTable::from_text(&text);
        assert_eq!(rep.skipped, 0, "不该有认不出的行：{}", rep.summary());
        assert_eq!(rep.identity, 93518, "纯收录行应记成无信息，不是跳过");
        assert!(t.safe_fold.len() >= 550, "无歧义异体归一太少");
        assert!(t.to_trad.len() > 3000);
        assert!(!t.conflicts.is_empty(), "一简对多繁必须被记账");
        assert_eq!(t.trad('经'), '經');
        assert_eq!(t.simp('經'), '经');
        assert_eq!(t.fold('叡'), '睿');
        println!("真实字表导入：{}｜{}", rep.summary(), active_summary());
    }

    #[test]
    fn rescue_only_touches_chars_the_font_cannot_draw() {
        // 真的拼一个最小 TrueType，只含「睿」「與」两个字，走完整 cmap 解析器。
        let fake = GlyphSet::from_bytes(&fake_font_with(&['睿', '與'])).unwrap();
        assert!(fake.contains('睿'));
        assert!(fake.contains('與'));
        assert!(!fake.contains('叡'));
        let t = VariantTable::builtin();
        let (out, rep) = rescue_missing("叡睿與", &fake, &t);
        assert_eq!(rep.rescued, vec![('叡', '睿')]);
        assert_eq!(out, "睿睿與");
        assert!(!out.contains('与'), "有字形的繁体正字绝不能被简化");
    }

    #[test]
    fn strict_rescue_never_does_simplification_but_display_mode_can() {
        // 字体只有「众」没有「衆」：严格档不许把繁体正字改成简体，
        // 显示层专用档才允许，而且调用方必须自己保证「绝不落库」。
        let fake = GlyphSet::from_bytes(&fake_font_with(&['众'])).unwrap();
        let t = VariantTable::builtin();
        let (strict, srep) = rescue_missing("衆", &fake, &t);
        assert_eq!(strict, "衆", "严格档绝不改繁体正字");
        assert!(srep.rescued.is_empty());
        let (display, drep) = rescue_missing_display("衆", &fake, &t);
        assert_eq!(display, "众");
        assert_eq!(drep.rescued, vec![('衆', '众')]);
    }

    #[test]
    fn tofu_chars_reports_what_a_font_cannot_draw() {
        let fake = GlyphSet::from_bytes(&fake_font_with(&['一', '二'])).unwrap();
        let missing = tofu_chars("一丁丁三𠀀", &fake);
        assert_eq!(missing, vec!['丁', '三', '𠀀']);
    }

    /// 拼一个只含 format 12 分组的最小 sfnt，让抢救测试**真的解析字体**。
    fn fake_font_with(chars: &[char]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0, 1, 0, 0]); // sfntVersion
        v.extend_from_slice(&1u16.to_be_bytes()); // numTables
        v.extend_from_slice(&0u16.to_be_bytes()); // searchRange
        v.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        v.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        v.extend_from_slice(b"cmap"); // table tag
        v.extend_from_slice(&0u32.to_be_bytes()); // checksum
        let cmap_off = 12 + 16;
        v.extend_from_slice(&(cmap_off as u32).to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes()); // length（解析器不校验）
        // ---- cmap ----
        v.extend_from_slice(&0u16.to_be_bytes()); // version
        v.extend_from_slice(&1u16.to_be_bytes()); // numTables
        v.extend_from_slice(&3u16.to_be_bytes()); // platformID = Windows
        v.extend_from_slice(&10u16.to_be_bytes()); // encodingID = UCS-4
        v.extend_from_slice(&12u32.to_be_bytes()); // subtable offset = 12
        // ---- format 12 ----
        let n = chars.len();
        v.extend_from_slice(&12u16.to_be_bytes()); // format
        v.extend_from_slice(&0u16.to_be_bytes()); // reserved
        v.extend_from_slice(&((16 + 12 * n) as u32).to_be_bytes()); // length
        v.extend_from_slice(&0u32.to_be_bytes()); // language
        v.extend_from_slice(&(n as u32).to_be_bytes()); // numGroups
        for ch in chars {
            let cp = *ch as u32;
            v.extend_from_slice(&cp.to_be_bytes()); // startCharCode
            v.extend_from_slice(&cp.to_be_bytes()); // endCharCode
            v.extend_from_slice(&1u32.to_be_bytes()); // startGlyphID
        }
        v
    }
}
