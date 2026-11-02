//! 金标与预测的数据模型 + 加载 + 用户纠正沉淀。
//!
//! 金标 JSONL 一行一条 [`GoldCase`]，预测 JSONL 一行一条 [`Prediction`]，按 `id` 对齐。
//! 用户每次手动改拼音/标点，产一条 [`CorrectionLog`]；`fold_corrections` 把它并回金标，
//! 同时把「引擎旧输出」记成已知坏样本——这就是「改了=负例」自动沉淀的飞轮起点。

use crate::score::PunctSpan;
use serde::{Deserialize, Serialize};

/// 单条金标用例。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoldCase {
    pub id: String,
    /// 原文（带标准标点）。若未显式给 `puncts`，评测时用 [`crate::score::extract_puncts`] 从它抽。
    pub text: String,
    #[serde(default)]
    pub puncts: Vec<PunctSpan>,
    /// 逐字金标拼音（可选）。
    #[serde(default)]
    pub pinyin: Vec<String>,
    /// 若评 AI 笺注，给一个提问。
    #[serde(default)]
    pub annotation_query: Option<String>,
    /// 笺注金标（可选，deterministic 比对用）。
    #[serde(default)]
    pub annotation_gold: Option<String>,
    /// 已知坏样本：引擎历史上错过的输出（用户纠正前的 `before`）。
    #[serde(default)]
    pub known_bad: Vec<String>,
}

impl GoldCase {
    /// 取标点跨度：显式给就用显式的，否则从 text 抽。
    pub fn puncts(&self) -> Vec<PunctSpan> {
        if self.puncts.is_empty() {
            crate::score::extract_puncts(&self.text)
        } else {
            self.puncts.clone()
        }
    }
}

/// 引擎/管线的一次输出，与金标按 `id` 对齐。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prediction {
    pub id: String,
    /// 引擎输出的正文（用于文本保真比对）。
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub puncts: Option<Vec<PunctSpan>>,
    #[serde(default)]
    pub pinyin: Option<Vec<String>>,
    /// AI 笺注输出。
    #[serde(default)]
    pub annotation: Option<String>,
}

/// 用户纠正日志——飞轮燃料。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CorrectionLog {
    pub case_id: String,
    /// 被改的字段。
    #[serde(default)]
    pub field: CorrectionField,
    /// 引擎旧输出（负例来源）。
    pub before: String,
    /// 用户改后的值（成为新金标）。
    pub after: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum CorrectionField {
    #[default]
    Pinyin,
    Punct,
    Annotation,
    Text,
}

fn load_jsonl<T: for<'de> Deserialize<'de>>(path: &str) -> anyhow::Result<Vec<T>> {
    let txt = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for (i, line) in txt.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: T = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("{} 第 {} 行解析失败: {}", path, i + 1, e))?;
        out.push(v);
    }
    Ok(out)
}

pub fn load_gold(path: &str) -> anyhow::Result<Vec<GoldCase>> {
    load_jsonl(path)
}

pub fn load_predictions(path: &str) -> anyhow::Result<Vec<Prediction>> {
    load_jsonl(path)
}

pub fn load_corrections(path: &str) -> anyhow::Result<Vec<CorrectionLog>> {
    load_jsonl(path)
}

/// 把用户纠正并回金标集。
///
/// 语义：用户把 `before` 改成 `after` ⇒ 金标应取 `after`；`before` 记进 `known_bad`，
/// 日后 nightly 若引擎又产出 `before`，即为已知回归，单独告警。
pub fn fold_corrections(gold: &mut [GoldCase], corrections: &[CorrectionLog]) {
    for c in corrections {
        let Some(g) = gold.iter_mut().find(|g| g.id == c.case_id) else {
            continue;
        };
        match c.field {
            CorrectionField::Pinyin => {
                // 拼音按整串替换（样例里一条纠正对应整词/整句拼音）。
                g.pinyin = c.after.split_whitespace().map(|s| s.to_string()).collect();
            }
            CorrectionField::Annotation => {
                g.annotation_gold = Some(c.after.clone());
            }
            CorrectionField::Text => {
                g.text = c.after.clone();
                g.puncts.clear(); // text 变了，旧显式标点作废，下次从 text 重抽。
            }
            CorrectionField::Punct => {
                g.puncts = crate::score::extract_puncts(&c.after);
            }
        }
        if !g.known_bad.contains(&c.before) {
            g.known_bad.push(c.before.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_pinyin_updates_gold_and_records_bad() {
        let mut gold = vec![GoldCase {
            id: "c1".into(),
            text: "道可道".into(),
            puncts: vec![],
            pinyin: vec!["dào".into(), "kǎ".into(), "dào".into()],
            annotation_query: None,
            annotation_gold: None,
            known_bad: vec![],
        }];
        let corr = vec![CorrectionLog {
            case_id: "c1".into(),
            field: CorrectionField::Pinyin,
            before: "dào kǎ dào".into(),
            after: "dào kě dào".into(),
        }];
        fold_corrections(&mut gold, &corr);
        assert_eq!(gold[0].pinyin, vec!["dào", "kě", "dào"]);
        assert_eq!(gold[0].known_bad, vec!["dào kǎ dào"]);
    }

    #[test]
    fn load_gold_from_jsonl_roundtrip() {
        let tmp = std::env::temp_dir().join("jiandu_eval_gold_test.jsonl");
        let line = r#"{"id":"x","text":"道可道，非常道。","pinyin":["dào","kě","dào"]}"#;
        std::fs::write(&tmp, line).unwrap();
        let g = load_gold(tmp.to_str().unwrap()).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].puncts().len(), 2);
        let _ = std::fs::remove_file(&tmp);
    }
}
