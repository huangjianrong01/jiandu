#!/usr/bin/env bash
# 笺读评测台 · nightly 跑分（基线版）。
# 当前用样例金标演示三道防线（标点保真 F1 / 文本保真 / 注音正确率）+ 笺注启发式基线。
# 接真实数据时：把金标换成从 resources/books/*.json 抽出的标准标点，
# 预测换成引擎/管线在该书上的实际输出（按 id 对齐，写同一 JSONL 格式即可）。
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="metrics-$(date +%Y%m%d).json"
cargo run -p jiandu-eval --quiet -- run \
  --gold crates/eval/data/gold.sample.jsonl \
  --pred crates/eval/data/pred.sample.jsonl \
  --out "$OUT"
echo "指标已落盘：$OUT"
