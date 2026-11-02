//! 评测核心指标。
//!
//! 笺读没有「从无标点生成句读」的引擎——原文（道德经/唐诗 JSON）自带标准标点，
//! 句读是**渲染与保真**；真正的 AI 输出是 RAG 笺注。所以评测台守三道防线：
//!   1. 清洗/导入管线对标点的保真（回归守卫）—— [`punctuation_f1`]
//!   2. 文本字符保真（清洗不能吞字/乱插）—— [`text_fidelity`]
//!   3. 注音正确率—— [`pinyin_accuracy`]
//! AI 笺注质量走 [`crate::judge`]，用 LLM 裁判 + 人工校准（主观层）。

use serde::{Deserialize, Serialize};

/// 一个标点：位于第 `after` 个**内容字符**之后（0-based，只数非标点的字符）。
///
/// 这样定义对空白/全半角差异不敏感；若清洗把内容字移位，after 跟着错位，
/// 标点在 F1 里就匹配不上——正好把「吞字导致标点漂移」这种回归暴露出来。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PunctSpan {
    pub after: usize,
    pub kind: char,
}

/// 哪些字符算「标点」—— CJK 句读符号 + 常见 ASCII 标点。
pub fn is_punct(c: char) -> bool {
    match c {
        '。' | '．' | '，' | '、' | '；' | '：' | '？' | '！' | '…' | '—' | '·' => true,
        '「' | '」' | '『' | '』' | '“' | '”' | '‘' | '’' => true,
        '（' | '）' | '《' | '》' | '〈' | '〉' | '【' | '】' | '〔' | '〕' => true,
        '.' | ',' | ';' | ':' | '?' | '!' | '"' | '\'' | '(' | ')' | '-' => true,
        _ => false,
    }
}

/// 从一段文本里抽出标点跨度（金标 / 预测共用同一抽取逻辑，保证口径一致）。
pub fn extract_puncts(text: &str) -> Vec<PunctSpan> {
    let mut out = Vec::new();
    let mut after = 0usize;
    for c in text.chars() {
        if is_punct(c) {
            out.push(PunctSpan { after, kind: c });
        } else {
            after += 1;
        }
    }
    out
}

/// 标点评测得分。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PunctScore {
    pub gold: usize,
    pub pred: usize,
    pub tp: usize,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

impl PunctScore {
    fn ratio(a: usize, b: usize) -> f64 {
        if b == 0 {
            if a == 0 {
                1.0
            } else {
                0.0
            }
        } else {
            a as f64 / b as f64
        }
    }
}

/// 计算标点 F1。
///
/// `by_type = true` 时要求 `kind` 也一致（位置 + 类型）；`false` 时只看位置。
/// 同一 `after` 上可能叠多个标点（如 `。」`），用排序双指针做多重集匹配。
pub fn punctuation_f1(gold: &[PunctSpan], pred: &[PunctSpan], by_type: bool) -> PunctScore {
    let norm = |s: &PunctSpan| -> (usize, char) {
        if by_type {
            (s.after, s.kind)
        } else {
            (s.after, '\u{0000}')
        }
    };
    let mut g: Vec<(usize, char)> = gold.iter().map(norm).collect();
    let mut p: Vec<(usize, char)> = pred.iter().map(norm).collect();
    g.sort();
    p.sort();
    let (mut i, mut j) = (0usize, 0usize);
    let mut tp = 0usize;
    while i < g.len() && j < p.len() {
        if g[i] < p[j] {
            i += 1;
        } else if g[i] > p[j] {
            j += 1;
        } else {
            tp += 1;
            i += 1;
            j += 1;
        }
    }
    let precision = PunctScore::ratio(tp, pred.len());
    let recall = PunctScore::ratio(tp, gold.len());
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    PunctScore {
        gold: gold.len(),
        pred: pred.len(),
        tp,
        precision,
        recall,
        f1,
    }
}

/// 文本字符保真度。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextFidelity {
    /// 金标内容字数（已剔除标点和空白）。
    pub gold_chars: usize,
    /// 预测内容字数。
    pub pred_chars: usize,
    /// 最长公共子序列长度（字符级保真上限）。
    pub lcs: usize,
    /// 字符留存率 = lcs / gold_chars。
    pub preservation: f64,
    /// 预测覆盖率 = lcs / pred_chars。
    pub coverage: f64,
    /// 编辑距离（只数内容字）。
    pub lev: usize,
}

fn han_only(s: &str) -> Vec<char> {
    s.chars().filter(|c| !is_punct(*c) && !c.is_whitespace()).collect()
}

fn lcs(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return 0;
    }
    let mut dp = vec![0usize; m + 1];
    for i in 1..=n {
        let mut prev = 0usize;
        for j in 1..=m {
            let tmp = dp[j];
            dp[j] = if a[i - 1] == b[j - 1] {
                prev + 1
            } else {
                dp[j].max(dp[j - 1])
            };
            prev = tmp;
        }
    }
    dp[m]
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev = (0..=m).collect::<Vec<_>>();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

/// 文本字符保真度（清洗/导入不能吞字、乱插、错字）。
pub fn text_fidelity(gold: &str, pred: &str) -> TextFidelity {
    let g = han_only(gold);
    let p = han_only(pred);
    let l = lcs(&g, &p);
    let lev = levenshtein(&g, &p);
    TextFidelity {
        gold_chars: g.len(),
        pred_chars: p.len(),
        lcs: l,
        preservation: PunctScore::ratio(l, g.len()),
        coverage: PunctScore::ratio(l, p.len()),
        lev,
    }
}

/// 注音正确率。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinyinScore {
    pub total: usize,
    pub correct: usize,
    pub accuracy: f64,
    /// 错掉的索引（用于报告里点名）。
    pub wrong: Vec<usize>,
}

fn fold_diacritic(c: char) -> char {
    match c {
        'ā' | 'á' | 'ǎ' | 'à' => 'a',
        'ē' | 'é' | 'ě' | 'è' => 'e',
        'ī' | 'í' | 'ǐ' | 'ì' => 'i',
        'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
        'ū' | 'ú' | 'ǔ' | 'ù' => 'u',
        'ü' | 'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'v',
        'ń' | 'ň' => 'n',
        'ḿ' => 'm',
        _ => c,
    }
}

fn normalize_py(s: &str, tone_insensitive: bool) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if tone_insensitive {
            let c = fold_diacritic(c).to_ascii_lowercase();
            if c.is_ascii_alphanumeric() {
                out.push(c);
            }
        } else {
            // 严格模式保留声调符号，逐字精确比对。
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// 逐字注音正确率。`tone_insensitive = true` 时只比声韵、不计调（dào≈dao）。
pub fn pinyin_accuracy(gold: &[String], pred: &[String], tone_insensitive: bool) -> PinyinScore {
    let n = gold.len().min(pred.len());
    let mut correct = 0usize;
    let mut wrong = Vec::new();
    for i in 0..n {
        let g = normalize_py(&gold[i], tone_insensitive);
        let p = normalize_py(&pred[i], tone_insensitive);
        if g == p {
            correct += 1;
        } else {
            wrong.push(i);
        }
    }
    PinyinScore {
        total: n,
        correct,
        accuracy: PunctScore::ratio(correct, n),
        wrong,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_puncts_matches_hand_count() {
        // 道可道，非常道。名可名，非常名。
        let t = "道可道，非常道。名可名，非常名。";
        let ps = extract_puncts(t);
        assert_eq!(ps.len(), 4);
        assert_eq!(ps[0], PunctSpan { after: 3, kind: '，' });
        assert_eq!(ps[1], PunctSpan { after: 6, kind: '。' });
        assert_eq!(ps[2], PunctSpan { after: 9, kind: '，' });
        assert_eq!(ps[3], PunctSpan { after: 12, kind: '。' });
    }

    #[test]
    fn perfect_match_is_f1_one() {
        let g = extract_puncts("道可道，非常道。");
        let p = extract_puncts("道可道，非常道。");
        let s = punctuation_f1(&g, &p, true);
        assert_eq!(s.f1, 1.0);
        assert_eq!(s.precision, 1.0);
        assert_eq!(s.recall, 1.0);
    }

    #[test]
    fn dropped_comma_lowers_recall() {
        // 金标有 6 个标点；预测丢了一个逗号（after=18）。
        let gold = extract_puncts("道可道，非常道。名可名，非常名。無名天地之始，有名萬物之母。");
        let pred = extract_puncts("道可道，非常道。名可名，非常名。無名天地之始有名萬物之母。");
        let s = punctuation_f1(&gold, &pred, true);
        assert_eq!(s.gold, 6);
        assert_eq!(s.pred, 5);
        assert_eq!(s.tp, 5);
        assert!((s.recall - 5.0 / 6.0).abs() < 1e-9);
        assert_eq!(s.precision, 1.0);
        assert!((s.f1 - 10.0 / 11.0).abs() < 1e-9, "f1={}", s.f1);
    }

    #[test]
    fn wrong_kind_drops_type_f1_but_not_position() {
        // 把句号写成逗号：位置 F1 仍满，类型 F1 掉到 0.5（2 标点里 1 个类型错）。
        let gold = extract_puncts("道可道，非常道。");
        let pred = extract_puncts("道可道，非常道，");
        let pos = punctuation_f1(&gold, &pred, false);
        let typ = punctuation_f1(&gold, &pred, true);
        assert_eq!(pos.f1, 1.0);
        assert!((typ.f1 - 0.5).abs() < 1e-9, "type f1={}", typ.f1);
    }

    #[test]
    fn text_fidelity_catches_dropped_char() {
        let f = text_fidelity("道可道非常道", "道可非常道");
        assert_eq!(f.gold_chars, 6);
        assert_eq!(f.pred_chars, 5);
        assert_eq!(f.lcs, 5);
        assert!((f.preservation - 5.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn pinyin_accuracy_and_tone_insensitive() {
        // 严格：第 1 字 dào vs dao 仅声调不同也判错。
        let gold = vec!["dào".into(), "kě".into(), "dào".into()];
        let pred = vec!["dao".into(), "kě".into(), "dào".into()];
        let strict = pinyin_accuracy(&gold, &pred, false);
        assert_eq!(strict.correct, 2);
        assert_eq!(strict.wrong, vec![0]);
        // 不计调：dào≈dao，三者全中。
        let loose = pinyin_accuracy(&gold, &pred, true);
        assert_eq!(loose.correct, 3, "不计调时 dào/dao 视为同");
    }
}
