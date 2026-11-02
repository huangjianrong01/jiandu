#!/usr/bin/env bash
# 笺读评测台 · nightly 跑分。
# 1) run  —— 样例金标演示三道防线（标点保真 F1 / 文本保真 / 注音正确率）+ 笺注启发式基线，
#            并刻意埋了一处回归（掉逗号+错拼音）证明守卫能抓问题。
# 2) real —— 真实集成：从 resources/books/*.json 抽金标，用真实 jiandu_core::classicize
#            清洗链做预测。既跑真实代码，又当回归守卫（清洗吞标点 / 移位 → F1 立刻掉）。
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="metrics-$(date +%Y%m%d).json"
echo "== [1/2] 样例基线 =="
cargo run -p jiandu-eval --quiet -- run \
  --gold crates/eval/data/gold.sample.jsonl \
  --pred crates/eval/data/pred.sample.jsonl \
  --out "$OUT"
echo "指标已落盘：$OUT"
echo ""
echo "== [2/2] 真实书目集成（classicize 幂等守卫）=="
cargo run -p jiandu-eval --quiet -- real --out "real-${OUT}"
echo "真实指标已落盘：real-${OUT}"
