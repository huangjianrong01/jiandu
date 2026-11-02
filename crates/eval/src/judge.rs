//! AI 笺注质量评估——LLM-as-judge 主观层。
//!
//! 笺注没有唯一标准答案，正是 LLM 裁判的用武之地。但 LLM 裁判有三个已知偏差
//! （位置偏差 / 冗长偏差 / 自我偏好），所以：
//!   - 默认走 [`heuristic_score`]：离线、确定性、可复现，作为 nightly 基线；
//!   - 真实打分用 [`judge_annotation`] 传入 `llm` 回调（CLI 里可接到 jiandu_core::ai 或任意模型），
//!     回调失败/缺席时自动回落启发式并标 `method="heuristic"`；
//!   - 永远配人工抽检校准（见 crate 根说明），定期看裁判与人对不拢就调 prompt。

use serde::{Deserialize, Serialize};

/// 笺注裁判得分。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JudgeScore {
    /// 0..1。
    pub score: f64,
    /// 评分依据。
    pub rationale: String,
    /// `llm` 或 `heuristic`。
    pub method: String,
}

fn clamp01(x: f64) -> f64 {
    x.max(0.0).min(1.0)
}

/// 离线启发式：覆盖提问关键词 + 长度达标 + 非拒答。确定性、可复现，作基线。
pub fn heuristic_score(query: &str, answer: &str) -> JudgeScore {
    let ans = answer.trim();
    if ans.is_empty() || ans.contains("无法") && ans.len() < 12 {
        return JudgeScore {
            score: 0.0,
            rationale: "空答或明确拒答".into(),
            method: "heuristic".into(),
        };
    }
    // 提问里的实词（≥2 字、非停用）被回答覆盖的比例。
    let joined: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    // 中文无词边界：用 2 字滑窗截词；纯拉丁按空白截词。
    let has_cjk = joined.chars().any(|c| (c as u32) > 0x2E80);
    let q_terms: Vec<String> = if has_cjk {
        if joined.chars().count() < 2 {
            Vec::new()
        } else {
            joined
                .chars()
                .collect::<Vec<_>>()
                .windows(2)
                .map(|w| w.iter().collect::<String>())
                .collect()
        }
    } else {
        joined
            .split_whitespace()
            .filter(|t| t.chars().count() >= 2)
            .map(|s| s.to_string())
            .collect()
    };
    let covered = if q_terms.is_empty() {
        1.0
    } else {
        q_terms
            .iter()
            .filter(|t| ans.contains(t.as_str()))
            .count() as f64
            / q_terms.len() as f64
    };
    // 长度 adequacy：太少像没答，过多像灌水（这里取 20..400 字为甜区）。
    let len = ans.chars().count();
    let len_ok = if len < 20 {
        len as f64 / 20.0
    } else if len > 400 {
        400.0 / len as f64
    } else {
        1.0
    };
    let score = clamp01(0.6 * covered + 0.4 * len_ok);
    JudgeScore {
        score,
        rationale: format!("关键词覆盖 {:.0}% · 长度 {} 字", covered * 100.0, len),
        method: "heuristic".into(),
    }
}

/// 用外部 LLM 裁判打分；`llm` 返回对 (score_text, rationale) 的原始文本。
/// 解析失败或 `llm` 为 None 时回落 [`heuristic_score`]。
pub fn judge_annotation(
    query: &str,
    answer: &str,
    llm: Option<&dyn Fn(&str) -> String>,
) -> JudgeScore {
    let Some(llm) = llm else {
        return heuristic_score(query, answer);
    };
    let prompt = format!(
        "你是古籍笺注质量评委。给下面这条笺注打分（0-1，只回数字）并给一句理由。\n\
         提问：{}\n笺注：{}\n只输出形如 `0.85 | 释义准确，举例恰当` 的一行。",
        query, answer
    );
    let raw = llm(&prompt);
    if let Some((s, r)) = raw.split_once('|') {
        if let Ok(v) = s.trim().parse::<f64>() {
            return JudgeScore {
                score: clamp01(v),
                rationale: r.trim().to_string(),
                method: "llm".into(),
            };
        }
    }
    heuristic_score(query, answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_rewards_coverage_and_penalizes_empty() {
        let good = heuristic_score("老子 万物", "道是老子哲学的最高范畴，指万物本源。");
        assert!(good.score > 0.5, "覆盖提问词且长度达标");
        let bad = heuristic_score("老子 万物", "");
        assert_eq!(bad.score, 0.0);
    }

    #[test]
    fn llm_fallback_to_heuristic_on_garbage() {
        let j = judge_annotation("道", "道者万物之奥。", Some(&|_| "不是数字".into()));
        assert_eq!(j.method, "heuristic");
    }

    #[test]
    fn llm_parses_score() {
        let j = judge_annotation(
            "道",
            "道者万物之奥。",
            Some(&|_| "0.9 | 释义准确".into()),
        );
        assert_eq!(j.method, "llm");
        assert!((j.score - 0.9).abs() < 1e-9);
    }
}
