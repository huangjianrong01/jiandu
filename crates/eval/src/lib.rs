//! 笺读评测台 · 库入口。
//!
//! 三道防线：标点保真 F1 / 文本字符保真 / 注音正确率；AI 笺注走 [`judge`]（LLM 裁判 + 人工校准）。
//! 用户纠正经 [`gold::fold_corrections`] 沉淀为金标与已知坏样本，形成反馈飞轮。
//!
//! 用法见 `main.rs`（CLI）：`jiandu-eval run --gold G --pred P`。

pub mod gold;
pub mod judge;
pub mod score;

use gold::{GoldCase, Prediction};
use judge::{judge_annotation, JudgeScore};
use score::{punctuation_f1, pinyin_accuracy, text_fidelity, PunctScore, PinyinScore, TextFidelity};
use serde::Serialize;
use std::collections::HashMap;

/// 单用例评测报告。
#[derive(Serialize)]
pub struct CaseReport {
    pub id: String,
    pub punct_pos: PunctScore,
    pub punct_type: PunctScore,
    pub text: TextFidelity,
    pub pinyin: Option<PinyinScore>,
    pub annotation: Option<JudgeScore>,
}

/// 聚合指标（各维度在用例上的均值）。
#[derive(Serialize, Default)]
pub struct Aggregate {
    pub cases: usize,
    pub punct_pos_f1: f64,
    pub punct_type_f1: f64,
    pub text_preservation: f64,
    pub pinyin_acc: f64,
    pub annotation_score: f64,
}

/// 对金标集跑一遍预测，返回逐条报告与聚合。
///
/// `llm` 为 None 时笺注走启发式基线；传回调即接入真实 LLM 裁判。
pub fn evaluate(
    gold: &[GoldCase],
    pred: &[Prediction],
    llm: Option<&dyn Fn(&str) -> String>,
) -> (Vec<CaseReport>, Aggregate) {
    let mut reports = Vec::with_capacity(gold.len());
    let mut s_pos = 0.0;
    let mut s_type = 0.0;
    let mut s_text = 0.0;
    let mut s_py = 0.0;
    let mut py_cnt = 0usize;
    let mut s_an = 0.0;
    let mut an_cnt = 0usize;

    let pred_by_id: HashMap<&str, &Prediction> =
        pred.iter().map(|p| (p.id.as_str(), p)).collect();

    for g in gold {
        let p = pred_by_id.get(g.id.as_str());

        let g_punct = g.puncts();
        let p_punct = p
            .and_then(|p| p.puncts.clone())
            .or_else(|| p.and_then(|p| p.text.as_ref()).map(|t| score::extract_puncts(t)))
            .unwrap_or_default();
        let punct_pos = punctuation_f1(&g_punct, &p_punct, false);
        let punct_type = punctuation_f1(&g_punct, &p_punct, true);

        let p_text = p.and_then(|p| p.text.clone()).unwrap_or_default();
        let text = text_fidelity(&g.text, &p_text);

        let pinyin = p
            .and_then(|p| p.pinyin.clone())
            .map(|pv| pinyin_accuracy(&g.pinyin, &pv, true));

        let annotation = match (g.annotation_query.as_ref(), p.and_then(|p| p.annotation.clone())) {
            (Some(q), Some(a)) => Some(judge_annotation(q, &a, llm)),
            _ => None,
        };

        // 累加须在 move 进报告之前进行。
        s_pos += punct_pos.f1;
        s_type += punct_type.f1;
        s_text += text.preservation;
        if let Some(py) = &pinyin {
            s_py += py.accuracy;
            py_cnt += 1;
        }
        if let Some(an) = &annotation {
            s_an += an.score;
            an_cnt += 1;
        }

        reports.push(CaseReport {
            id: g.id.clone(),
            punct_pos,
            punct_type,
            text,
            pinyin,
            annotation,
        });
    }

    let n = gold.len().max(1) as f64;
    let py_n = py_cnt.max(1) as f64;
    let an_n = an_cnt.max(1) as f64;
    let agg = Aggregate {
        cases: gold.len(),
        punct_pos_f1: s_pos / n,
        punct_type_f1: s_type / n,
        text_preservation: s_text / n,
        pinyin_acc: if py_cnt > 0 { s_py / py_n } else { 1.0 },
        annotation_score: if an_cnt > 0 { s_an / an_n } else { 1.0 },
    };
    (reports, agg)
}
