//! **中医方剂本地化规则字典**（v1.9.19）。
//!
//! 把原 `dclean.rs` 里硬编码的剂量量词 / 炮制词 / 异体字映射，迁移到一份
//! **可离线维护的 JSON 字典**（`src/data/tcm_rules.json`），编译期经
//! `include_str!` 嵌入内核，零运行时 IO、零网络依赖。
//!
//! **设计口径**：
//! * 字典是旧硬编码集的**超集**——`weight_units` 收全了旧 `is_dose_unit` 的
//!   全部字符（錢/钱/分/兩/两/両/克/斤/寸/升/斗/合/匕/撮/丸/劑/剂/綫/線/
//!   厘/釐/條/条/枚/粒/杯/盃/匙/羹/支/燋）+ 新增 `铢`；`count_units` 收
//!   `個/个/只/具`；`variant_map` 收 `朮→术`、`薑→姜`、`蠹→釐`、`釐→厘`；
//!   `processing_words` 收炮制 / 修治短语。重构后只委托、不替换，**行为不退化**。
//! * 区分**重量单位**（`weight_units`）与**数量单位**（`count_units`）：
//!   `個/个` 当归数量、**绝不**当重量（保「一個人」零误包），但两者都参与
//!   方名升格的「铁壁」——任何药行都带量词（重或数），含之必非方名。
//! * `variant_map` 统一繁简 / 异体 / OCR 别名：`蠹`（蛀虫，OCR 误识）归一为
//!   `釐`（厘），`朮`→`术`、`薑`→`姜`，剂量解析时先归一再判单位。

use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// JSON 原始结构（仅反序列化用）。
#[derive(Deserialize)]
struct TcmRulesRaw {
    weight_units: Vec<String>,
    count_units: Vec<String>,
    variant_map: HashMap<String, String>,
    processing_words: Vec<String>,
}

/// 归一化后的内存结构：字符级集合 + 短语表，O(1) 查询。
struct TcmRules {
    /// 重量 / 古量 / 丸劑单位（含繁简异体全收）。
    weight: HashSet<char>,
    /// 数量单位（個/个/只/具）。
    count: HashSet<char>,
    /// 异体 / OCR 别名 → 规范字。
    variant: HashMap<char, char>,
    /// 炮制 / 修治短语（去頭足 / 炙 / 炒 / 去皮尖 / 研末 …）。
    processing: Vec<String>,
}

/// **编译期嵌入**的字典单例。panic 仅可能在字典 JSON 写错时发生
/// （开发期即可暴露），运行期永不失败。
static RULES: Lazy<TcmRules> = Lazy::new(|| {
    let raw: TcmRulesRaw = serde_json::from_str(include_str!("data/tcm_rules.json"))
        .expect("tcm_rules.json 解析失败：请检查 JSON 格式");
    let mut weight = HashSet::new();
    for s in &raw.weight_units {
        // 每个单位 entry 必须是单字（多字 entry 视为非法，直接取首字并告警式忽略）。
        if let Some(c) = s.chars().next() {
            weight.insert(c);
        }
    }
    let mut count = HashSet::new();
    for s in &raw.count_units {
        if let Some(c) = s.chars().next() {
            count.insert(c);
        }
    }
    let mut variant = HashMap::new();
    for (k, v) in &raw.variant_map {
        if let (Some(kc), Some(vc)) = (k.chars().next(), v.chars().next()) {
            variant.insert(kc, vc);
        }
    }
    TcmRules {
        weight,
        count,
        variant,
        processing: raw.processing_words,
    }
});

/// **异体 / 繁简 / OCR 别名归一**：`朮→术`、`薑→姜`、`蠹→釐`、`釐→厘`。
///
/// 不在映射表里的字符原样返回。剂量解析前先归一，再判单位 / 查炮制。
pub fn normalize_variants(c: char) -> char {
    *RULES.variant.get(&c).unwrap_or(&c)
}

/// **重量 / 古量 / 丸劑单位**判据（含繁简异体全收 + OCR 别名归一）。
///
/// `蠹`（OCR 误识）经 `normalize_variants` 归一成 `釐` → 命中重量集；
/// `釐` 归一为 `厘`（也在重量集）→ 同样命中。两者都安全。
pub fn is_weight_unit(c: char) -> bool {
    RULES.weight.contains(&c) || RULES.weight.contains(&normalize_variants(c))
}

/// **数量单位**判据（個/个/只/具）。
///
/// ⚠️ 数量与重量**互斥**：`個/个` 绝不进 `weight_units`（保「一個人」），
/// 但两者都进方名升格「铁壁」。
pub fn is_count_unit(c: char) -> bool {
    RULES.count.contains(&c) || RULES.count.contains(&normalize_variants(c))
}

/// **剂量单位**（重量 ∪ 数量）——`dclean::is_dose_unit` 的权威实现。
///
/// 含数字配对要求的「是否真的当剂量」由 `split_dosage_to_gloss` 把关，
/// 本函数只回答「这个字**能不能**当量词」。
pub fn is_dose_unit(c: char) -> bool {
    is_weight_unit(c) || is_count_unit(c)
}

/// **炮制 / 修治短语**判据：归一后整串是否含任一炮制词。
///
/// 用于方名升格「铁壁」——含 `去頭足 / 炙 / 炒 / 去皮尖 / 研末` 等炮制提示的
/// 行必是药行 / 用法说明，绝不可能是方名标题。
pub fn is_processing_phrase(s: &str) -> bool {
    let norm: String = s.chars().map(normalize_variants).collect();
    RULES.processing.iter().any(|w| norm.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_loads_and_exposes_superset() {
        // 旧硬编码集全部字符必须仍在重量集（向后兼容，不退化）。
        for c in [
            '錢', '钱', '分', '兩', '两', '両', '克', '斤', '寸', '升', '斗', '合', '匕',
            '撮', '丸', '劑', '剂', '綫', '線', '厘', '釐', '條', '条', '枚', '粒', '杯',
            '盃', '匙', '羹', '支', '燋',
        ] {
            assert!(is_weight_unit(c), "旧重量单位漏网：{c}");
        }
        // 新增铢进重量集。
        assert!(is_weight_unit('铢'));
        // 数量单位。
        for c in ['個', '个', '只', '具'] {
            assert!(is_count_unit(c));
            assert!(!is_weight_unit(c), "数量单位被误判为重量：{c}");
        }
    }

    #[test]
    fn variant_normalization_works() {
        assert_eq!(normalize_variants('朮'), '术');
        assert_eq!(normalize_variants('薑'), '姜');
        // OCR 别名：蠹 → 釐 → 命中重量集。
        assert_eq!(normalize_variants('蠹'), '釐');
        assert!(is_dose_unit('蠹'));
        assert!(is_weight_unit('蠹'));
        // 釐 归一为 厘，仍命中。
        assert_eq!(normalize_variants('釐'), '厘');
        assert!(is_dose_unit('釐'));
    }

    #[test]
    fn count_units_are_not_weight() {
        // 铁律：個/个 绝不进重量集（保「一個人」）。
        assert!(!is_weight_unit('個'));
        assert!(!is_weight_unit('个'));
    }

    #[test]
    fn processing_phrase_detects_tcm_terms() {
        assert!(is_processing_phrase("斑蝥去頭足十個"));
        assert!(is_processing_phrase("當歸炙"));
        assert!(is_processing_phrase("川芎炒"));
        assert!(is_processing_phrase("杏仁去皮尖"));
        assert!(is_processing_phrase("雄黃研末"));
        // 普通方名不含炮制词。
        assert!(!is_processing_phrase("走馬牙疳"));
    }
}
