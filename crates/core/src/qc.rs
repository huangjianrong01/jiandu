//! **质检 · 待办清单**（V2.1）：把已经写好的清洗 / 异体 / 繁简判据，从
//! 「默默改掉」变成「摆在用户眼前的一列可点击条目」。
//!
//! 设计原则（与整个内核一脉相承）：
//!   * **零猜测**——每一条都能说出触发它的客观条件（哪个字符、哪条 Unicode
//!     区间、哪一对符号），绝不跑模型、绝不「疑似」；
//!   * **零误伤**——宁漏不误。宁可放过一处，也不让用户点开发现是虚警；
//!   * **零 UI**——纯函数，[`QcReport`] 可直接序列化给 B 端做书稿质检报告；
//!   * **不复用 `analysis::is_variant_char`**：那把尺子是为「阅读时提示生僻」
//!     校准的，它把「凡是能归一到一个常用字的写法」都算异体——于是繁体书里
//!     每一个「說」「國」都会被点出来。质检要的是**同一部书内部的不一致**，
//!     不是「这字简繁哪一版」，故此处另立判据（见 [`is_uncommon_form`]）。
//!
//! 六类判据，全部可解释：
//!
//! | 类别 | 判据 | 级别 |
//! |------|------|------|
//! | [`QcKind::Residue`] | 把 `residue` 那套清洗函数**再跑一遍**，还能改动的行 | 错误 |
//! | [`QcKind::Bracket`] | 引号 / 书名号 / 括号不配对、不闭合 | 错误 |
//! | [`QcKind::TradMix`] | 同一个字，全书出现繁简（或正异）两种写法 | 提醒 |
//! | [`QcKind::Punct`] | 两个句读连用（`，，` `。，`） | 提醒 |
//! | [`QcKind::DupLine`] | 相邻两行完全相同（多为书名 / 页眉残留） | 提醒 |
//! | [`QcKind::Rare`] | 兼容区 / 扩展 B 区以后的罕用形 | 提示 |

use crate::residue;
use crate::variants::VariantTable;
use std::collections::HashMap;

/// 单类最多收多少条：清单是给人看的，不是给机器扫的；超了只记 `truncated`。
pub const MAX_PER_KIND: usize = 120;
/// 全表上限。超过后不再追加，避免百万字稿子一次生成几万条把界面拖死。
pub const MAX_TOTAL: usize = 500;

/// 质检类别。`ALL` 的顺序 = 界面上的显示顺序（错误在前）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum QcKind {
    Residue,
    Bracket,
    TradMix,
    Punct,
    DupLine,
    Rare,
}

impl QcKind {
    pub const ALL: [QcKind; 6] = [
        QcKind::Residue,
        QcKind::Bracket,
        QcKind::TradMix,
        QcKind::Punct,
        QcKind::DupLine,
        QcKind::Rare,
    ];

    /// 在 `ALL` / `counts` 里的下标。
    pub const fn index(self) -> usize {
        match self {
            QcKind::Residue => 0,
            QcKind::Bracket => 1,
            QcKind::TradMix => 2,
            QcKind::Punct => 3,
            QcKind::DupLine => 4,
            QcKind::Rare => 5,
        }
    }

    /// 界面上的短名（面板条目左侧那枚小标签）。
    pub const fn label(self) -> &'static str {
        match self {
            QcKind::Residue => "残留",
            QcKind::Bracket => "符号",
            QcKind::TradMix => "繁简",
            QcKind::Punct => "标点",
            QcKind::DupLine => "重复",
            QcKind::Rare => "罕用",
        }
    }

    /// 判定依据（悬停提示 / 报告脚注用）。
    pub const fn hint(self) -> &'static str {
        match self {
            QcKind::Residue => "清洗函数仍能改动此行：可能还留着标签、实体、伪空格或表格线",
            QcKind::Bracket => "引号 / 括号 / 书名号未成对",
            QcKind::TradMix => "同一个字在全书出现两种写法，宜统一",
            QcKind::Punct => "两个句读连用，多为误录",
            QcKind::DupLine => "与上一行完全相同，多为书名或页眉残留",
            QcKind::Rare => "兼容区或扩展区罕用形，不是通行正字",
        }
    }

    /// 是否属于「必须改」的一类（面板里点朱砂色）。
    pub const fn is_error(self) -> bool {
        matches!(self, QcKind::Residue | QcKind::Bracket)
    }
}

/// 一条待办。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QcIssue {
    pub kind: QcKind,
    /// 0-based 行号（按 `\n` 切分的第几行）。
    pub line: usize,
    /// **字符下标**（不是字节偏移）——与编辑器 `cursor` 同一套口径，
    /// 面板点一下就能把光标落到这里。
    pub ch: usize,
    /// 命中长度（字符数），用于就地高亮。
    pub len: usize,
    /// 命中处的上下文（已裁到 16 字以内，供清单显示）。
    pub excerpt: String,
    /// 一句话说明：为什么它被点出来。
    pub detail: String,
}

/// 一次扫描的结果。
#[derive(Clone, Debug, Default)]
pub struct QcReport {
    /// 按**文档顺序**排好的条目（先看到前面的问题）。
    pub issues: Vec<QcIssue>,
    /// 分类计数（下标见 [`QcKind::index`]）。
    pub counts: [usize; 6],
    pub scanned_chars: usize,
    pub scanned_lines: usize,
    /// 有条目因为超出上限被丢弃。
    pub truncated: bool,
}

impl QcReport {
    pub fn count(&self, k: QcKind) -> usize {
        self.counts[k.index()]
    }
    pub fn total(&self) -> usize {
        self.issues.len()
    }
    /// 必须处理的条目数（残留 + 符号）。
    pub fn errors(&self) -> usize {
        self.count(QcKind::Residue) + self.count(QcKind::Bracket)
    }
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }
}

/// 句读集合：只有这七个算「句读」，连用才有意义。
const PUNCT: [char; 7] = ['，', '。', '、', '；', '：', '？', '！'];

/// 配对符号：开 → 闭。
const PAIRS: [(char, char); 9] = [
    ('「', '」'),
    ('『', '』'),
    ('【', '】'),
    ('（', '）'),
    ('〈', '〉'),
    ('《', '》'),
    ('〔', '〕'),
    ('“', '”'),
    ('‘', '’'),
];

/// 清洗残留探针：把 `residue` 那套**已经在管线里跑过**的函数再跑一遍。
///
/// 还能改动就说明「这一行没洗干净」。这是最诚实的一条判据——它不重新定义
/// 什么算残留，只是把管线自己的答案问第二遍。
const RESIDUE_CHECKS: [(&str, fn(&str) -> Option<String>); 5] = [
    ("HTML 标签", residue::strip_tags),
    ("HTML 实体", residue::decode_entities),
    ("汉字间伪空格", residue::despace_cjk),
    ("Markdown 表格线", residue::clean_md_table_line),
    ("破折号字形", residue::unify_dashes),
];

/// 兼容区 / 扩展 B 区以后：Unicode 自己写明「这是为兼容旧字符集而单列的写法」，
/// 或压根就是罕用形。这一条不含任何简繁判断，故繁体书不会被误报。
fn is_uncommon_form(c: char) -> bool {
    let u = c as u32;
    (0xF900..=0xFAFF).contains(&u)      // 兼容汉字
        || (0x2F800..=0x2FA1F).contains(&u) // 兼容补充
        || (0x20000..=0x323AF).contains(&u) // 扩展 B 及以后
}

/// 两个字符串第一个不同的字符下标（字符计，不是字节）。完全相同 → `None`。
fn first_diff_char(a: &str, b: &str) -> Option<usize> {
    let mut i = 0usize;
    let mut ai = a.chars();
    let mut bi = b.chars();
    loop {
        match (ai.next(), bi.next()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return Some(i);
                }
            }
            (Some(_), None) | (None, Some(_)) => return Some(i),
            (None, None) => return None,
        }
        i += 1;
    }
}

/// 取 `ch` 附近的一段上下文（最多 `width` 个字），供清单显示。
fn excerpt_at(text: &str, ch: usize, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    let at = ch.min(n - 1);
    let half = width / 2;
    let mut start = at.saturating_sub(half);
    let mut end = (start + width).min(n);
    if end - start < width {
        start = end.saturating_sub(width);
    }
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < n {
        s.push('…');
    }
    s
}

/// **扫一遍全文，出一份待办清单。**
///
/// 单趟 `O(n)`（外加一次按字的 `O(unique)` 聚合），百万字量级也在几十毫秒内。
pub fn scan(text: &str) -> QcReport {
    let mut rep = QcReport::default();
    // 六万字的繁简 / 正异三元表（`data/variant_cjk.txt`）：只当**分组键**用，
    // 一个字都不改。注意不能用 `core::trad`——那张手写表只有百来对，只覆盖
    // 《道德经》样张，「说 / 說」都不在里头，用它做判据会永远静默。
    let table = VariantTable::builtin();
    // 逐字频次：字 → (出现次数, 首次字符下标, 首次所在行)。
    let mut freq: HashMap<char, (usize, usize, usize)> = HashMap::new();
    // 未闭合符号栈：(开符号, 字符下标, 行号)。
    let mut stack: Vec<(char, usize, usize)> = Vec::new();

    let mut base = 0usize; // 当前行首的字符下标
    let mut line_no = 0usize;
    let mut prev_line: Option<&str> = None;
    let mut push = |rep: &mut QcReport, issue: QcIssue| {
        if rep.counts[issue.kind.index()] >= MAX_PER_KIND || rep.issues.len() >= MAX_TOTAL {
            rep.truncated = true;
            return;
        }
        rep.counts[issue.kind.index()] += 1;
        rep.issues.push(issue);
    };

    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let n = chars.len();
        rep.scanned_lines += 1;
        rep.scanned_chars += n;

        // ── 1. 清洗残留 ─────────────────────────────────────────────────
        if n > 0 {
            for (name, f) in RESIDUE_CHECKS.iter() {
                let Some(cleaned) = f(line) else { continue };
                let Some(mut off) = first_diff_char(line, &cleaned) else {
                    continue;
                };
                // 实体这条要回退到 `&`：`天&amp;地` 的第一个差异落在 `a` 上，
                // 但用户想看到的是那个不该出现的 `&`。逐字回退到最近的 `&`。
                if *name == "HTML 实体" {
                    let head: Vec<char> = line.chars().take(off + 1).collect();
                    if let Some(p) = head.iter().rposition(|&c| c == '&') {
                        off = p;
                    }
                }
                push(
                    &mut rep,
                    QcIssue {
                        kind: QcKind::Residue,
                        line: line_no,
                        ch: base + off,
                        len: 1,
                        excerpt: excerpt_at(text, base + off, 16),
                        detail: format!("{} · 清洗未净", name),
                    },
                );
            }
        }

        // ── 2. 符号配对 ─────────────────────────────────────────────────
        for (i, &c) in chars.iter().enumerate() {
            if let Some(&(_, close)) = PAIRS.iter().find(|(o, _)| *o == c) {
                stack.push((close, base + i, line_no));
            } else if let Some(&(open, _)) = PAIRS.iter().find(|(_, cl)| *cl == c) {
                match stack.last() {
                    Some(&(expect, _, _)) if expect == c => {
                        stack.pop();
                    }
                    _ => {
                        push(
                            &mut rep,
                            QcIssue {
                                kind: QcKind::Bracket,
                                line: line_no,
                                ch: base + i,
                                len: 1,
                                excerpt: excerpt_at(text, base + i, 16),
                                detail: format!("「{}」多余：前面没有对应的「{}」", c, open),
                            },
                        );
                    }
                }
            }
        }

        // ── 3. 句读连用 ─────────────────────────────────────────────────
        for i in 1..n {
            let (a, b) = (chars[i - 1], chars[i]);
            if !(PUNCT.contains(&a) && PUNCT.contains(&b)) {
                continue;
            }
            // 「？！」「！？」是正当的强调连写，放过。
            if matches!((a, b), ('？', '！') | ('！', '？')) {
                continue;
            }
            push(
                &mut rep,
                QcIssue {
                    kind: QcKind::Punct,
                    line: line_no,
                    ch: base + i,
                    len: 1,
                    excerpt: excerpt_at(text, base + i, 16),
                    detail: format!("「{}{}」连用", a, b),
                },
            );
        }

        // ── 4. 重复行 ───────────────────────────────────────────────────
        if n >= 2 {
            if let Some(prev) = prev_line {
                if prev == line {
                    push(
                        &mut rep,
                        QcIssue {
                            kind: QcKind::DupLine,
                            line: line_no,
                            ch: base,
                            len: n,
                            excerpt: excerpt_at(text, base, 16),
                            detail: "与上一行完全相同".to_string(),
                        },
                    );
                }
            }
        }
        prev_line = Some(line);

        // ── 5. 逐字：罕用形 + 频次 ──────────────────────────────────────
        for (i, &c) in chars.iter().enumerate() {
            if is_uncommon_form(c) {
                let simp = table.simp(c);
                let detail = if simp != c {
                    format!("罕用形 · 通行作「{}」", simp)
                } else {
                    "扩展区罕用字".to_string()
                };
                push(
                    &mut rep,
                    QcIssue {
                        kind: QcKind::Rare,
                        line: line_no,
                        ch: base + i,
                        len: 1,
                        excerpt: excerpt_at(text, base + i, 16),
                        detail,
                    },
                );
            }
            freq.entry(c)
                .and_modify(|e| e.0 += 1)
                .or_insert((1usize, base + i, line_no));
        }

        base += n + 1; // `\n` 也占一个字符
        line_no += 1;
    }

    // 收尾：栈里剩的都是没闭合的开符号。
    for (close, ch, line) in stack {
        push(
            &mut rep,
            QcIssue {
                kind: QcKind::Bracket,
                line,
                ch,
                len: 1,
                excerpt: excerpt_at(text, ch, 16),
                detail: format!("「{}」未闭合", close),
            },
        );
    }

    // ── 6. 繁简 / 正异混用（全书口径）──────────────────────────────────
    //
    // 分组键 = 该字归一后的通行简体形；组内出现≥2 种写法，且其中既有通行形
    // 又有它的繁体/异体形 → 全书不一致。取**少数派**那一形作为落点：
    // 繁体书里偶冒出来的简体字，正好就是校对时要抓的那一处。
    let mut groups: HashMap<char, Vec<(char, usize, usize, usize)>> = HashMap::new();
    for (&c, &(cnt, first, line)) in freq.iter() {
        let key = table.simp(c);
        groups.entry(key).or_default().push((c, cnt, first, line));
    }
    for (&key, surfaces) in groups.iter() {
        if surfaces.len() < 2 {
            continue;
        }
        // 「宁漏不误」：只有当**通行形本身**在场时才判「繁简混用」。
        // 若组里只有两个异体（如「說」「説」都不等于「说」），那属于字形差异，
        // 不是简繁之争——放过，交给别的判据。
        if !surfaces.iter().any(|s| s.0 == key) {
            continue;
        }
        // 少数派 = 全书出现次数最少的那个写法（平手取后出现的）：
        // 繁体书里偶冒出来的那个简体字，正是校对时要抓的那一处。
        let Some(minor) = surfaces.iter().min_by_key(|s| (s.1, usize::MAX - s.2)) else {
            continue;
        };
        let Some(major) = surfaces.iter().max_by_key(|s| (s.1, s.2)) else {
            continue;
        };
        if minor.0 == major.0 {
            continue;
        }
        let (&(form, cnt, first, line), &(major_form, major_cnt, _, _)) = (minor, major);
        push(
            &mut rep,
            QcIssue {
                kind: QcKind::TradMix,
                line,
                ch: first,
                len: 1,
                excerpt: excerpt_at(text, first, 16),
                detail: format!(
                    "「{}」×{} 与「{}」×{} 并存，宜统一",
                    form, cnt, major_form, major_cnt
                ),
            },
        );
    }

    // 按文档顺序排：用户是从上往下校的，清单也该这么读。
    rep.issues.sort_by_key(|i| (i.ch, i.kind.index()));
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(r: &QcReport) -> Vec<QcKind> {
        r.issues.iter().map(|i| i.kind).collect()
    }

    fn find<'a>(r: &'a QcReport, k: QcKind) -> Option<&'a QcIssue> {
        r.issues.iter().find(|i| i.kind == k)
    }

    #[test]
    fn clean_text_has_no_issues() {
        let r = scan("天地玄黃，宇宙洪荒。\n日月盈昃，辰宿列張。");
        assert!(r.is_empty(), "干净文本不该报任何条目：{:?}", r.issues);
        assert_eq!(r.scanned_chars, 20);
        assert_eq!(r.scanned_lines, 2);
    }

    #[test]
    fn residue_space_points_at_the_space() {
        // 「天地 玄黃」中间那个空格：管线里的 despace_cjk 仍能删掉它
        let r = scan("天地 玄黃");
        let i = find(&r, QcKind::Residue).expect("应检出伪空格");
        assert_eq!(i.ch, 2, "落点必须正好在空格上");
        assert!(i.detail.contains("伪空格"));
    }

    #[test]
    fn residue_tag_points_at_the_bracket() {
        let r = scan("<p>天地玄黃");
        let i = find(&r, QcKind::Residue).expect("应检出 HTML 标签");
        assert_eq!(i.ch, 0);
        assert!(i.detail.contains("HTML 标签"));
    }

    #[test]
    fn residue_entity_points_at_the_ampersand() {
        let r = scan("天&amp;地");
        let i = find(&r, QcKind::Residue).expect("应检出 HTML 实体");
        assert_eq!(i.ch, 1);
    }

    #[test]
    fn bracket_unclosed_and_stray() {
        let r = scan("「天地玄黃");
        let i = find(&r, QcKind::Bracket).expect("应检出未闭合");
        assert_eq!(i.ch, 0);
        assert!(i.detail.contains("未闭合"));

        let r2 = scan("天地玄黃」");
        let j = find(&r2, QcKind::Bracket).expect("应检出多余右引号");
        assert_eq!(j.ch, 4);
        assert!(j.detail.contains("多余"));
    }

    #[test]
    fn bracket_paired_is_clean() {
        let r = scan("「天地」玄黃（宇宙）");
        assert!(find(&r, QcKind::Bracket).is_none(), "{:?}", r.issues);
    }

    #[test]
    fn punct_double_is_flagged_but_emphasis_pair_is_not() {
        let r = scan("天地玄黃，，宇宙洪荒");
        let i = find(&r, QcKind::Punct).expect("应检出连用");
        assert_eq!(i.ch, 5, "落点在第二个逗号");

        let ok = scan("天地玄黃？！宇宙洪荒");
        assert!(find(&ok, QcKind::Punct).is_none(), "？！ 是正当连写");
    }

    #[test]
    fn duplicate_line_is_flagged() {
        let r = scan("資治通鑑\n天地玄黃\n天地玄黃");
        let i = find(&r, QcKind::DupLine).expect("应检出重复行");
        assert_eq!(i.line, 2);
    }

    #[test]
    fn trad_mix_reports_the_minority_form() {
        // 繁體書裡冒出一個簡體「说」，少數派就是它
        let text = "說說說說說說說說說说";
        let r = scan(text);
        let i = find(&r, QcKind::TradMix).expect("应检出繁简混用");
        assert_eq!(i.ch, 9, "落点在那个少数派简体字上");
        assert!(i.detail.contains('说') && i.detail.contains('說'));
    }

    #[test]
    fn uniform_traditional_is_not_flagged() {
        // 全繁体不该被当成「繁简混用」——这是 is_variant_char 会误报的地方
        let r = scan("說國學禮義廉恥");
        assert!(find(&r, QcKind::TradMix).is_none(), "{:?}", r.issues);
    }

    #[test]
    fn uncommon_form_is_flagged() {
        // U+F900 是「兼容汉字」区的 豈，与通行形 U+8C48 不是同一个码位
        let text = format!("\u{F900}敢");
        let r = scan(&text);
        let i = find(&r, QcKind::Rare).expect("应检出兼容区字形");
        assert_eq!(i.ch, 0);
    }

    #[test]
    fn issues_are_in_document_order() {
        let r = scan("「天地 玄黃\n宇宙洪荒，，");
        let chs: Vec<usize> = r.issues.iter().map(|i| i.ch).collect();
        let mut sorted = chs.clone();
        sorted.sort();
        assert_eq!(chs, sorted, "清单必须按文档顺序");
    }

    #[test]
    fn counts_match_issues() {
        let r = scan("「天地 玄黃，，\n天地 玄黃，，");
        for k in QcKind::ALL {
            assert_eq!(r.count(k), r.issues.iter().filter(|i| i.kind == k).count());
        }
        assert_eq!(r.total(), r.issues.len());
        assert!(r.errors() >= 2);
    }

    #[test]
    fn empty_text_is_safe() {
        let r = scan("");
        assert!(r.is_empty());
        assert_eq!(r.scanned_lines, 1); // split('\n') 对空串给一个空行
    }
}
