//! 端到端：用样例金标/预测跑一遍 `evaluate`，断言能抓到故意埋的回归。

use jiandu_eval::gold::{load_gold, load_predictions};
use jiandu_eval::evaluate;

fn data(p: &str) -> String {
    format!("{}/data/{}", env!("CARGO_MANIFEST_DIR"), p)
}

#[test]
fn catches_dropped_comma_and_wrong_pinyin() {
    let gold = load_gold(&data("gold.sample.jsonl")).unwrap();
    let pred = load_predictions(&data("pred.sample.jsonl")).unwrap();
    let (reports, agg) = evaluate(&gold, &pred, None);

    let ddj = reports.iter().find(|r| r.id == "ddj-1").unwrap();
    // 金标 6 标点，预测丢 1 逗号 ⇒ 位置/类型 F1 = 10/11。
    assert!((ddj.punct_type.f1 - 10.0 / 11.0).abs() < 1e-9, "{}", ddj.punct_type.f1);
    assert!((ddj.punct_pos.f1 - 10.0 / 11.0).abs() < 1e-9);
    // 24 字拼音里错 1 个（kǎ≠kě）⇒ 23/24。
    let py = ddj.pinyin.as_ref().unwrap();
    assert_eq!(py.correct, 23);
    assert_eq!(py.wrong, vec![1]);

    let ts = reports.iter().find(|r| r.id == "ts-1").unwrap();
    assert_eq!(ts.punct_type.f1, 1.0, "完美用例应为满分");

    // 聚合：两句平均，标点 F1 应低于 1（有回归），ts 拉回一半。
    assert!(agg.punct_type_f1 < 1.0);
    assert!(agg.punct_type_f1 > 0.95);
    assert!((agg.pinyin_acc - 23.0 / 24.0).abs() < 1e-9);
    assert!(agg.annotation_score > 0.0, "笺注启发式应给出基线分");
}
