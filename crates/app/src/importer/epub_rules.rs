//! EPUB 脏数据清洗规则表（**外置 JSON，用户可改**）。
//!
//! # 为什么外置
//!
//! 古籍 EPUB 的脏**没有标准**：这本把句读做成 91 万张 `<img>`，那本用
//! `<span class="zhushi">〔…〕</span>` 包夹注、用 `【章：…】` 塞校勘记，
//! 第三本可能又有自己的私有标签。把规则硬编码进 Rust，等于每来一本书就
//! 改一次代码、发一次版——**不可能穷举**。
//!
//! 所以规则表放 JSON：内置的编进二进制（`assets/img_punct_map.json`），
//! 首次导入时**落到用户目录** `com.jiandu.app/rules/img_punct_map.json`，
//! 之后以用户那份为准。用户照着手上那本书的图片名 / class 名改完重启即生效。
//!
//! # 漏斗的三阶段
//!
//! ```text
//!   ① 探针  Rules::select(抽样 XHTML)   → 选中一个 Profile（认不出就用 generic）
//!   ② 结构  html_parser 按 Profile 改写 DOM 行为（句读图 → 字符、drop_class）
//!   ③ 文本  strip_* 系列（校勘记 / 竖排兼容字符 / 段首缩进 / 装饰符）
//! ```
//!
//! 认不出门派时 **generic 方案只做通用净化、绝不动结构**——宁可留着脏，
//! 也不能把不认识的书的正文洗坏。

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// 内置默认规则表（编进二进制，离线可用）。
const BUILTIN_JSON: &str = include_str!("../../assets/img_punct_map.json");

/// 用户可编辑的那一份的文件名。
const FILE_NAME: &str = "img_punct_map.json";

// ----------------------------------------------------------------------
// 数据结构（与 JSON 一一对应）
// ----------------------------------------------------------------------

/// `【章：…】` / `『章：…』` / `〔章：…〕` 这类**校勘记壳**：整条剔除。
///
/// 判据是双重的，缺一不可：
///   1. 开壳处（或开壳前一个 `标签：`）的标签在 `labels` 白名单里；
///   2. 内容主体是校勘记体例。
///
/// 第 1 条是必须的：`〔〕` 在中华书局本里**既是夹注壳又是校勘记壳**——
/// `〔師古曰：…〕` 是胡注正文的一部分，`〔章：十二行本…〕` 才是该杀的校勘记。
/// 只看「有没有冒号」会把 8 万条真夹注一起杀光。
///
/// ⚠️ 标签与冒号可能落在**壳外**（转换件自己就是坏的）：
/// `齊章：<span class="zhushi">〔十二行本齊上有…〕</span>`。
/// 故 `label_before_open = true` 时，额外接受「开壳前紧邻 `标签：`」。
#[derive(Deserialize, Clone, Debug)]
pub struct JunkBracket {
    pub open: char,
    pub close: char,
    #[serde(default = "dflt_label_max")]
    pub label_max_chars: usize,
    #[serde(default = "dflt_true")]
    pub require_colon: bool,
    /// 校勘记标签白名单（章鈺 / 嚴衍 / 張敦仁 / …）。空 = 不校验标签。
    #[serde(default)]
    pub labels: Vec<String>,
    /// 也接受「`标签：` 在开壳**之前**」这种被转换件写坏的形态。
    #[serde(default = "dflt_true")]
    pub label_before_open: bool,
    /// 内容里命中其中任一词才认（空 = 不校验内容）。
    #[serde(default)]
    pub body_must_contain: Vec<String>,
    /// 校勘记体的**最小长度**：太短的 `章：…` 多半是别的什么东西。
    #[serde(default = "dflt_min_len")]
    pub min_len: usize,
}

fn dflt_min_len() -> usize {
    8
}

fn dflt_toc_block() -> usize {
    30
}

fn dflt_label_max() -> usize {
    6
}
fn dflt_true() -> bool {
    true
}

/// 门派探针：在抽样 XHTML 里数 `raw_contains` 的命中次数。
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Detect {
    #[serde(default)]
    pub raw_contains: Vec<String>,
    #[serde(default)]
    pub min_hits: usize,
}

/// 一套清洗方案。
#[derive(Deserialize, Clone, Debug)]
pub struct Profile {
    pub id: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub detect: Detect,
    /// 只在这些 `class` 里的 `<img>` 才查句读映射表（空 = 所有图都查）。
    #[serde(default)]
    pub img_class: Vec<String>,
    /// 竖排兼容字符（U+FE10–FE4F）归一成横向标点。
    #[serde(default)]
    pub vertical_forms: bool,
    /// 段首全角缩进 `　　` 剥掉（笺读绝对顶格）。
    #[serde(default)]
    pub strip_leading_indent: bool,
    /// 这些 `class` 的 `<span>` **整块丢弃**（如中华书局的段号 `class="number"`）。
    #[serde(default)]
    pub drop_class: Vec<String>,
    /// 这些 `class` 的 `<span>` 是**夹注**，转成 core 的 `[注]…[/注]`（双行小字）。
    #[serde(default)]
    pub note_class: Vec<String>,
    /// 是否合并断裂段落（劣质件把一句话切成好几个 `<p>`）。
    #[serde(default = "dflt_true")]
    pub merge_broken: bool,
}

/// 规则表全文。
#[derive(Deserialize, Clone, Debug, Default)]
struct RawRules {
    /// 表结构版本。**低于内置表就整体换新**（旧表结构可能缺字段）：
    /// 升级时把用户那份改名留底，再落一份新的——用户照着新表重改一遍即可。
    #[serde(default)]
    version: u32,
    #[serde(default)]
    img_punct: HashMap<String, String>,
    #[serde(default)]
    drop_symbols: Vec<String>,
    /// 目录块剔除阈值（默认 30 行；填 0 表示不剔除目录）。
    #[serde(default = "dflt_toc_block")]
    toc_block_min_lines: usize,
    #[serde(default)]
    junk_brackets: Vec<JunkBracket>,
    #[serde(default)]
    profiles: Vec<Profile>,
}

/// 解析后的规则表（对外只暴露这个）。
#[derive(Clone, Debug)]
pub struct Rules {
    /// 图片文件名（去路径去扩展名）→ 标点字符。
    img: HashMap<String, char>,
    /// 正文里见一个删一个的装饰符。
    pub drop_symbols: Vec<char>,
    /// 连续多少行「极短无句读」算目录块（0 = 不剔除目录）。
    pub toc_block_min_lines: usize,
    /// 校勘记壳。
    pub junk: Vec<JunkBracket>,
    /// 门派方案，按顺序探；最后一条必是 generic。
    pub profiles: Vec<Profile>,
}

impl Rules {
    /// 探针：按抽样文本选中一套方案。**永远有返回值**（末条 generic 兜底）。
    pub fn select(&self, sample: &str) -> &Profile {
        for p in &self.profiles {
            let hits: usize = p
                .detect
                .raw_contains
                .iter()
                .map(|k| count_hits(sample, k))
                .sum();
            if hits >= p.detect.min_hits {
                return p;
            }
        }
        // 兜底：`profiles` 为空等极端情况，绝不 panic。
        self.profiles.last().expect("规则表至少有一条 generic 方案")
    }

    /// 按方案组装出本次导入要用的句读图片映射。
    pub fn img_punct_for(&self, p: &Profile) -> ImgPunct {
        ImgPunct {
            map: self.img.clone(),
            classes: p.img_class.clone(),
        }
    }
}

/// 数 `needle` 在 `s` 里出现几次（简单计数，够探针用）。
fn count_hits(s: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    s.match_indices(needle).count()
}

// ----------------------------------------------------------------------
// 句读图片 → 标点
// ----------------------------------------------------------------------

/// 一次导入中生效的「句读图片 → 字符」查表器。
#[derive(Clone, Debug, Default)]
pub struct ImgPunct {
    map: HashMap<String, char>,
    /// 空 = 所有 `<img>` 都查表；否则只查 `class` 命中其一的。
    classes: Vec<String>,
}

impl ImgPunct {
    /// 不查任何表（html / md / txt 等非 EPUB 入口，以及未选中方案时）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 这张图该变成什么字？`None` = 当普通插图丢掉。
    ///
    /// `src` 形如 `../Images/image19703.gif`，取**去路径去扩展名**的主名查表。
    pub fn punct_for(&self, src: &str, class: &str) -> Option<char> {
        if !self.classes.is_empty() {
            let c = class.to_ascii_lowercase();
            if !self.classes.iter().any(|k| c.contains(&k.to_ascii_lowercase())) {
                return None;
            }
        }
        self.map.get(&file_stem(src)).copied()
    }
}

/// `../Images/image19703.gif` → `image19703`（小写）。
fn file_stem(src: &str) -> String {
    let after = src.rsplit('/').next().unwrap_or(src);
    let stem = match after.rsplit_once('.') {
        Some((a, _)) => a,
        None => after,
    };
    stem.to_ascii_lowercase()
}

// ----------------------------------------------------------------------
// 文本级净化
// ----------------------------------------------------------------------

/// 竖排兼容字符（U+FE10–FE4F）归一成**横向标点**。
///
/// 竖排本里 `︰ ﹁ ﹂ ︵ ︶` 是「同一个标点的竖排字形」。笺读自己会摆竖排，
/// 拿到竖排专用码位反而画不出正确的句读（它们是兼容性字符，不该进正文流）。
pub fn normalize_vertical_forms(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '︰' => '：', // U+FE30
            '︐' => '，', // U+FE10
            '︑' => '、', // U+FE11
            '︒' => '。', // U+FE12
            '︓' => '：', // U+FE13
            '︔' => '；', // U+FE14
            '︕' => '！', // U+FE15
            '︖' => '？', // U+FE16
            '︗' => '【', // U+FE17
            '︘' => '】', // U+FE18
            '︙' => '…', // U+FE19
            '︵' => '（',
            '︶' => '）',
            '︷' => '｛',
            '︸' => '｝',
            '︹' => '〔',
            '︺' => '〕',
            '︻' => '【',
            '︼' => '】',
            '︽' => '《',
            '︾' => '》',
            '︿' => '〈',
            '﹀' => '〉',
            '﹁' => '「',
            '﹂' => '」',
            '﹃' => '『',
            '﹄' => '』',
            _ => c,
        })
        .collect()
}

/// 校勘记壳（如 `【章：乙十一行本…】`）整条剔除，**允许跨行**（劣质件常
/// 把一条校勘记用 `<br>` 切成两段）。
///
/// 找不到配对闭括号就原样放过——宁可留一条校勘记，也不能吃掉后面的正文。
const JUNK_SCAN_LIMIT: usize = 400;

pub fn strip_junk_brackets(s: &str, junk: &[JunkBracket]) -> String {
    if junk.is_empty() {
        return s.to_string();
    }
    let cs: Vec<char> = s.chars().collect();
    // 闭壳符**不认牌子**：劣质件常把壳写残，`【章：…〕` 这种开方头、闭六角
    // 的畸形壳满地都是。只要撞上任意一种闭壳符就算数——反正还有标签白名单
    // 和体例词两道闸，
    let closes: Vec<char> = junk.iter().map(|j| j.close).collect();
    // 先扫出所有该删的区间，**再**重建。不能边扫边输出：壳前的 `标签：`
    // 落在开壳之前，等扫到开壳时那几个字已经吐出去了，收不回来。
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < cs.len() {
        let Some(jb) = junk.iter().find(|j| j.open == cs[i]) else {
            i += 1;
            continue;
        };
        let lim = (i + JUNK_SCAN_LIMIT).min(cs.len());
        // 闭壳符按**牌子优先**：`『` 先配 `』`，配不上才认别的牌子（`【…〕`
        // 这种畸形壳）。最后实在没有，退到「删到校勘记的句号收尾」——
        // 转换件常常只丢了闭壳符，校勘记本身的句点还在。
        let same = (i + 1..lim).find(|&k| cs[k] == jb.close);
        let any = (i + 1..lim).find(|&k| closes.contains(&cs[k]));
        let period = (i + 1..lim).find(|&k| cs[k] == '。');
        let (j, closed) = match (same, any, period) {
            (Some(j), _, _) => (j, true),
            (None, Some(j), _) => (j, true),
            (None, None, Some(j)) => (j, false),
            (None, None, None) => {
                i += 1; // 连个句号都没有 → 原样放过，绝不硬吃正文
                continue;
            }
        };
        match junk_span(jb, &cs, i, j, closed) {
            Some(span) => {
                spans.push(span);
                i = span.1;
            }
            None => i += 1,
        }
    }
    if spans.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut cur = 0usize;
    for (a, b) in spans {
        out.extend(cs[cur..a].iter());
        cur = b;
    }
    out.extend(cs[cur..].iter());
    out
}

/// 这条壳里有没有校勘记？有就返回**该删的区间** `[start, end)`。
///
/// 三种形态，一种比一种脏：
///
/// | 形态 | 怎么处理 |
/// |---|---|
/// | ① `〔章：…〕` 标签紧贴开壳 | 整条连壳丢掉 |
/// | ①b `〔諱胡亥…也。章：十二行本…〕` 夹注与校勘记**挤在同一壳** | 只砍校勘记，**留 `〕` 把夹注收好** |
/// | ③ `齊章：〔十二行本…〕` 标签被写到壳**外** | 起点提前到标签那个字，正文的 `齊` 一个字都不许带走 |
fn junk_span(
    jb: &JunkBracket,
    cs: &[char],
    i: usize,
    j: usize,
    closed: bool,
) -> Option<(usize, usize)> {
    let whole = |f: &dyn Fn() -> bool| (j - i - 1) >= jb.min_len && f();
    if jb.labels.is_empty() {
        return whole(&|| body_hits(jb, cs, i + 1, j)).then_some((i, j + 1));
    }
    // ① / ①b 壳内：冒号要**扫全壳**，不能只看开壳后几个字——
    // 夹注在前、校勘记在后时，标签藏在壳中间。
    for k in (i + 1)..j {
        if cs[k] != '：' {
            continue;
        }
        let from = k.saturating_sub(jb.label_max_chars).max(i + 1);
        let label: String = cs[from..k].iter().collect();
        let Some(off) = suffix_label_off(jb, &label) else {
            continue;
        };
        let start = from + off;
        if (j - start) < jb.min_len || !body_hits(jb, cs, start, j) {
            continue;
        }
        return Some(if start == i + 1 {
            (i, j + 1) // ① 紧贴开壳 → 连壳整条丢
        } else if closed {
            (start, j) // ①b 夹注之后 → 只砍校勘记，留下闭括号
        } else {
            (start, j + 1) // 壳丢了 → 连收尾那个句号一起砍
        });
    }
    // ② 不强制要冒号时，开壳后前 `label_max_chars` 字本身就是标签。
    if !jb.require_colon {
        let e = (i + 1 + jb.label_max_chars).min(j);
        let label: String = cs[i + 1..e].iter().collect();
        if jb.labels.iter().any(|x| *x == label)
            && whole(&|| body_hits(jb, cs, i + 1, j))
        {
            return Some((i, j + 1));
        }
    }
    // ③ 壳前：`…是強弱之常也。齊章：<span class="zhushi">〔十二行本…〕`
    //    `齊` 是正文、`章：` 才是校勘记标签，故按**后缀**认标签。
    if jb.label_before_open {
        let back = i.saturating_sub(jb.label_max_chars + 2);
        if let Some(k) = (back..i).find(|&k| cs[k] == '：') {
            let from = back.max(k.saturating_sub(jb.label_max_chars));
            let label: String = cs[from..k].iter().collect();
            if let Some(off) = suffix_label_off(jb, &label) {
                if whole(&|| body_hits(jb, cs, i + 1, j)) {
                    return Some((from + off, j + 1));
                }
            }
        }
    }
    None
}

/// `label` 的**后缀**命中白名单，就返回它在串里的字符下标。
///
/// 只用于 ③ 壳前那条分支：正文紧贴着标签（`齊章：`），必须允许前缀有杂字。
fn suffix_label_off(jb: &JunkBracket, label: &str) -> Option<usize> {
    let lc: Vec<char> = label.chars().collect();
    (0..lc.len()).rev().find(|&off| {
        let tail: String = lc[off..].iter().collect();
        jb.labels.iter().any(|x| *x == tail)
    })
}

/// 内容像不像校勘记（`body_must_contain` 里命中任一词即可）。
fn body_hits(jb: &JunkBracket, cs: &[char], from: usize, to: usize) -> bool {
    if jb.body_must_contain.is_empty() {
        return true;
    }
    let body: String = cs[from..to].iter().collect();
    jb.body_must_contain.iter().any(|k| body.contains(k))
}

/// 段首全角缩进 `　　` 剥掉：笺读**绝对顶格**，段首空格会各占一个字格。
pub fn strip_leading_indent(s: &str) -> String {
    s.lines()
        .map(|l| l.trim_start_matches(['\u{3000}', ' ', '\t']).to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 目录块剔除：连续 `min_lines` 行以上的「极短 + 无句读」行，判为**目录页**整块丢掉。
///
/// 判据故意做到很笨、很保守——宁可漏掉一段目录，也绝不碰正文：
///   * 一行 ≤ 14 字；
///   * 整行不含任何句读标点（`，。；：！？、`）；
///   * 且要**连续**成块（`min_lines`，默认 30）。
///
/// 正文段落必然带句读、且远不止 14 字；就算碰上「周紀一」这种短卷标题，
/// 也只是孤零零一两行，成不了块。
pub fn strip_toc_blocks(s: &str, min_lines: usize) -> String {
    if min_lines == 0 {
        return s.to_string();
    }
    let lines: Vec<&str> = s.lines().collect();
    let mut drop = vec![false; lines.len()];
    let mut run_start = 0usize;
    let mut run = 0usize;
    for (i, l) in lines.iter().enumerate() {
        if is_toc_line(l) {
            if run == 0 {
                run_start = i;
            }
            run += 1;
        } else {
            if run >= min_lines {
                drop[run_start..i].fill(true);
            }
            run = 0;
        }
    }
    if run >= min_lines {
        drop[run_start..lines.len()].fill(true);
    }
    if !drop.iter().any(|d| *d) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (l, d) in lines.iter().zip(drop) {
        if !d {
            out.push_str(l);
            out.push('\n');
        }
    }
    // 末尾那一刀：原本 `lines()` 丢掉的收尾换行，别让行数凭空多一个空行出来。
    if !s.ends_with('\n') {
        let _ = out.pop();
    }
    out
}

const TOC_LINE_MAX_CHARS: usize = 14;

fn is_toc_line(l: &str) -> bool {
    let t = l.trim_ascii();
    if t.is_empty() || t.chars().count() > TOC_LINE_MAX_CHARS {
        return false;
    }
    !t.contains(['，', '。', '；', '：', '！', '？', '、'])
}

/// 行首**孤立句读串**删掉：图片句读本常有 `、、，。，，。二十三年…` 这种
/// 版心里的散装符号（原书那几格只有句读没有字），留在正文里就是一坨噪声。
///
/// 只砍**行首连续 2 个以上**的句读——正文绝不会以两个标点开头，所以这条
/// 规则再笨也不会碰到正文。
pub fn strip_leading_punct_runs(s: &str) -> String {
    s.lines()
        .map(|l| {
            let t = l.trim_ascii();
            // 行首的夹注开壳 `[注]` 是标记不是正文，跳过它再数句读
            let (head, body) = match t.strip_prefix("[注]") {
                Some(r) => ("[注]", r),
                None => ("", t),
            };
            let run = body.chars().take_while(|c| is_punct(*c)).count();
            // 0 个或 1 个：不动。正文里行首单一个标点不算噪声。
            if run < 2 {
                return l.to_string();
            }
            format!("{}{}", head, body.chars().skip(run).collect::<String>())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 删掉**空夹注壳** `[注][/注]`。
///
/// 校勘记常常就住在 `<span class="note">` 里；校勘记整条剔除后，那个壳里
/// 什么都不剩，留着就是正文里一串没意义的记号。
pub fn strip_empty_zhu(s: &str) -> String {
    s.replace("[注][/注]", "")
}

fn is_punct(c: char) -> bool {
    matches!(c, '、' | '，' | '。' | '；' | '：' | '！' | '？' | '︐' | '︑' | '︒' | '︓' | '︔' | '︕' | '︖' | '︰')
}

/// 装饰符（◇ ◆ □ …）见一个删一个。
pub fn strip_symbols(s: &str, syms: &[char]) -> String {
    if syms.is_empty() {
        return s.to_string();
    }
    s.chars().filter(|c| !syms.contains(c)).collect()
}

// ----------------------------------------------------------------------
// 加载：内置表 + 用户外置覆盖
// ----------------------------------------------------------------------

static RULES: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();

/// 生效中的规则表（进程内只解析一次）。
pub fn rules() -> &'static Rules {
    RULES.get_or_init(load)
}

/// 用户那份的路径（`com.jiandu.app/rules/img_punct_map.json`）。
pub fn user_path() -> PathBuf {
    jiandu_core::library::Library::rules_dir().join(FILE_NAME)
}

fn load() -> Rules {
    let builtin: RawRules = match serde_json::from_str(BUILTIN_JSON) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[jiandu] 内置 img_punct_map.json 解析失败，规则全部停用：{e}");
            RawRules::default()
        }
    };
    let path = user_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<RawRules>(&s) {
            Ok(u) if u.version >= builtin.version => u,
            Ok(u) => {
                // 用户那份是旧版：留底后换新，免得字段缺失导致规则静默失效。
                eprintln!(
                    "[jiandu] 外置规则表是 v{}，内置已是 v{}，已换新（旧表留底为 .bak）",
                    u.version, builtin.version
                );
                materialize(&path);
                builtin
            }
            Err(e) => {
                eprintln!(
                    "[jiandu] 外置规则表 {} 解析失败，回退内置表：{e}",
                    path.display()
                );
                builtin
            }
        },
        Err(_) => {
            // 还没有就**落一份**到用户目录，让用户有东西可改。
            materialize(&path);
            builtin
        }
    };
    build(raw)
}

/// 把内置表写到用户目录（失败静默：没有外置表只是不能改，不影响导入）。
///
/// 已有旧表就先改名留底——用户在旧表上花的功夫不该无声蒸发。
fn materialize(path: &PathBuf) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if path.exists() {
        let bak = path.with_extension("json.bak");
        let _ = std::fs::rename(path, bak);
    }
    let _ = std::fs::write(path, BUILTIN_JSON);
}

/// 校正成可用的 `Rules`：
/// * 保证末尾一定有一条「什么都匹配得上」的 generic 方案（探针绝不空手而归）；
/// * 图片映射只收单字符值（填错了就跳过那条，不整表失效）。
fn build(mut raw: RawRules) -> Rules {
    let mut img = HashMap::with_capacity(raw.img_punct.len());
    for (k, v) in raw.img_punct.drain() {
        let mut it = v.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => {
                img.insert(file_stem(&k), c);
            }
            _ => eprintln!("[jiandu] img_punct 里 {k} 的值不是单个字符，已跳过"),
        }
    }
    let has_generic = raw
        .profiles
        .iter()
        .any(|p| p.detect.raw_contains.is_empty() && p.detect.min_hits == 0);
    if !has_generic {
        raw.profiles.push(Profile {
            id: "generic".to_string(),
            note: "认不出门派：只做通用净化，不动结构".to_string(),
            detect: Detect::default(),
            img_class: Vec::new(),
            vertical_forms: false,
            strip_leading_indent: false,
            drop_class: Vec::new(),
            note_class: Vec::new(),
            merge_broken: true,
        });
    }
    Rules {
        img,
        drop_symbols: raw
            .drop_symbols
            .iter()
            .filter_map(|s| s.chars().next())
            .collect(),
        toc_block_min_lines: raw.toc_block_min_lines,
        junk: raw.junk_brackets,
        profiles: raw.profiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        build(serde_json::from_str(BUILTIN_JSON).unwrap())
    }

    #[test]
    fn builtin_json_parses_and_has_a_generic_profile() {
        let r = rules();
        assert!(!r.img.is_empty(), "内置表不该为空");
        assert!(
            r.profiles
                .last()
                .map(|p| p.detect.raw_contains.is_empty())
                .unwrap_or(false),
            "最后一条必须是 generic：{:?}",
            r.profiles.iter().map(|p| &p.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn probe_picks_the_image_punct_profile() {
        let r = rules();
        let a = r#"<img alt="" class="ytz" src="../Images/image19703.gif"/>"#.repeat(10);
        assert_eq!(r.select(&a).id, "ytz-img-punct");
        let b = r#"<span class="zhushi">〔陘，音刑。〕</span>"#.repeat(10);
        assert_eq!(r.select(&b).id, "zhonghua-jiaokan");
        // 谁都不像 → generic（绝不空手而归）
        assert_eq!(r.select("無何有之鄉").id, "generic");
    }

    #[test]
    fn img_punct_lookup_matches_stem_and_class() {
        let r = rules();
        let p = r.select(&r#"<img class="ytz" src="x.gif"/>"#.repeat(10));
        assert_eq!(p.id, "ytz-img-punct", "探针没选中图片句读方案");
        let ip = r.img_punct_for(p);
        assert_eq!(
            ip.punct_for("../Images/image19703.gif", "ytz"),
            Some('，'),
            "逗号图没映射上"
        );
        assert_eq!(ip.punct_for("../Images/image19802.gif", "ytz"), Some('。'));
        assert_eq!(ip.punct_for("../Images/image19805.gif", "ytz"), Some('？'));
        // class 不对 → 不查表（当插图丢掉）
        assert_eq!(ip.punct_for("../Images/image19703.gif", "cover"), None);
        // 表里没有的图 → 丢掉
        assert_eq!(ip.punct_for("../Images/image99999.gif", "ytz"), None);
    }

    #[test]
    fn junk_brackets_are_removed_whole_and_across_lines() {
        let junk = rules().junk;
        let s = "莫【章：乙十一行本「莫」下有「敢」字；孔本同。】不奔走而服役者";
        assert_eq!(
            strip_junk_brackets(s, &junk),
            "莫不奔走而服役者",
            "校勘记没整条剔除"
        );
        // 跨行（被 <br> 切成两段）
        let s2 = "乘作承；孔本【章：乙十一行本\n同；傳校同。】為弱也";
        let out = strip_junk_brackets(s2, &junk);
        assert!(!out.contains("【章"), "跨行校勘记没剔除：{out:?}");
        assert!(out.starts_with("乘作承；孔本"), "正文被误删：{out:?}");
        assert!(out.ends_with("為弱也"), "正文被误删：{out:?}");
    }

    #[test]
    fn real_quotes_are_never_treated_as_junk() {
        let junk = rules().junk;
        let s = "臣聞『千里饋糧，士有饑色；樵蘇後爨，師不宿飽。』今井陘之道";
        assert_eq!(strip_junk_brackets(s, &junk), s, "真引文被误杀");
        // 带冒号但不是「标签：」形态的短引文也不该杀
        let s2 = "信曰：『此在兵法，顧諸君不察耳！』";
        assert_eq!(strip_junk_brackets(s2, &junk), s2, "真引文被误杀");
    }

    /// **最关键的一条**：`〔〕` 在中华书局本里既是夹注壳又是校勘记壳。
    /// `〔師古曰：…〕` 是真夹注（胡注），`〔章：…〕` 才是校勘记。
    /// 只靠「有没有冒号」判别会把全书 8 万条夹注杀光。
    #[test]
    fn annotation_shell_with_a_scholar_label_survives() {
        let junk = rules().junk;
        let s = "韓信、張耳以兵數萬東擊趙〔章：十二行本二字互乙；乙十一行本同。〕不可諱";
        assert_eq!(
            strip_junk_brackets(s, &junk),
            "韓信、張耳以兵數萬東擊趙不可諱",
            "〔章：…〕校勘记没剔除"
        );
        // 真夹注：标签是注家名，不在白名单里 → 一个字都不许动
        let keep = "聚兵井陘口〔陘，音刑。杜佑曰：井陘口在鎮州鹿泉縣。〕號二十萬";
        assert_eq!(strip_junk_brackets(keep, &junk), keep, "真夹注被误杀");
        let keep2 = "〔師古曰：殊，絶也；言決意必死。〕不可敗";
        assert_eq!(strip_junk_brackets(keep2, &junk), keep2, "真夹注被误杀");
        let keep3 = "〔考異曰：天文志作「乙未」。今從帝紀及長曆。〕";
        assert_eq!(strip_junk_brackets(keep3, &junk), keep3, "真夹注被误杀");
    }

    /// 转换件把标签写到了壳**外面**，且紧贴着正文：
    /// `…常也。齊章：〔十二行本齊上有好士者…〕` —— 只删 `章：〔…〕`，`齊` 是正文。
    #[test]
    fn junk_label_written_before_the_open_bracket() {
        let junk = rules().junk;
        let s = "弱，是強弱之常也。齊章：〔十二行本齊上有好士者，下有賢士也；孔本同。〕而齊不用";
        assert_eq!(
            strip_junk_brackets(s, &junk),
            "弱，是強弱之常也。齊而齊不用",
            "壳外标签没处理对：{:?}",
            strip_junk_brackets(s, &junk)
        );
        // 同理，注家名在壳外时不能被当成校勘记
        let keep = "聚兵井陘口。師古曰：〔陘，音刑。〕號二十萬";
        assert_eq!(strip_junk_brackets(keep, &junk), keep, "真夹注被误杀");
    }

    /// 夹注与校勘记**挤在同一个壳**里：`〔諱胡亥，始皇少子也。章：十二行本無此八字。〕`
    /// 只砍后半条校勘记，闭括号留着把夹注收好 —— 删整条会把真注文一起吃掉。
    #[test]
    fn collation_note_sharing_a_shell_with_a_real_annotation() {
        let junk = rules().junk;
        let s = "二世皇帝上〔諱胡亥，始皇少子也。章：十二行本無此八字；乙十一行本同。〕元年";
        assert_eq!(
            strip_junk_brackets(s, &junk),
            "二世皇帝上〔諱胡亥，始皇少子也。〕元年",
            "共壳校勘记没处理对：{:?}",
            strip_junk_brackets(s, &junk)
        );
    }

    #[test]
    fn unpaired_junk_bracket_is_left_alone() {
        let junk = rules().junk;
        let s = "莫【章：十二行本有脫文，後文佚失";
        assert_eq!(strip_junk_brackets(s, &junk), s, "没配对的壳不该吃掉正文");
    }

    #[test]
    fn vertical_forms_become_horizontal_punctuation() {
        assert_eq!(normalize_vertical_forms("爾雅︰太歲在甲曰閼逢"), "爾雅：太歲在甲曰閼逢");
        assert_eq!(normalize_vertical_forms("﹁盡玄黓困敦﹂"), "「盡玄黓困敦」");
        assert_eq!(normalize_vertical_forms("︵戊寅︶"), "（戊寅）");
    }

    #[test]
    fn leading_full_width_indent_is_stripped() {
        assert_eq!(strip_leading_indent("　　漢紀二　起強圉作噩"), "漢紀二　起強圉作噩");
        // 段中的全角空格是「抬头」记号，不动
        assert_eq!(strip_leading_indent("甲　乙"), "甲　乙");
    }

    #[test]
    fn toc_blocks_go_but_real_prose_stays() {
        // 目录：连续 6 行极短无句读 → 整块丢（阈值取 5）
        let toc = "周紀一\n資治通鑑卷第二\n周紀二\n資治通鑑卷第三\n周紀三\n資治通鑑卷第四\n";
        assert_eq!(strip_toc_blocks(toc, 5), "");
        // 只有 2 行 → 不成块，原样留（卷标题页本来就这么短）
        let heads = "周紀一\n資治通鑑卷第二\n";
        assert_eq!(strip_toc_blocks(heads, 5), heads);
        // 正文里夹一两个短标题，前后都是带句读的长段 → 一个字都不许动
        let prose = "秦孝公據崤函之固，擁雍州之地。\n周紀一\n孝公既沒，惠文、武、昭襄蒙故業。\n";
        assert_eq!(strip_toc_blocks(prose, 5), prose, "正文被当目录杀了");
    }

    #[test]
    fn file_stem_strips_path_and_extension() {
        assert_eq!(file_stem("../Images/image19703.gif"), "image19703");
        assert_eq!(file_stem("image19802.GIF"), "image19802");
        assert_eq!(file_stem(""), "");
    }
}
