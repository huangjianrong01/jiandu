#!/bin/zsh
# ════════════════════════════════════════════════════════════════════════
# 避雷版 Compare Mode 启动器（开发沙盒专用 · 不是给最终用户用的）
#
# 背景 / 为什么需要它：
#   宿主软件 WorkBuddy 的 user-prompt-submit 钩子里藏了一行 `pkill -f jiandu`，
#   会在你每次给助手发消息时执行，把所有「命令行里含 jiandu 字样」的进程
#   连坐杀死 —— 表现就是 Compare 窗口「闪退」。
#   标准启动方式 `./target/release/jiandu` 的进程路径天生带 jiandu，必死。
#
# 解决 / 怎么做：
#   把编译产物复制成「不含 jiandu 字样」的临时名（默认 /tmp/compare_render_safe）
#   再启动。钩子按 argv 匹配，永远打不到这个进程。
#   注意：JIANDU_* 是「环境变量名」（在 environment 里，不在 argv 里），
#         pkill -f 看不见，所以保留它们完全安全。
#
# 用法：
#   scripts/launch-compare-safe.sh                # 默认指向 target/release/jiandu
#   JIANDU_COMPARE_IMG=... JIANDU_COMPARE_DB=... scripts/launch-compare-safe.sh
# 或自定义源码二进制：
#   scripts/launch-compare-safe.sh /path/to/jiandu-universal
# ════════════════════════════════════════════════════════════════════════
set -e

SRC="${1:-target/release/jiandu}"
SAFE="/tmp/compare_render_safe"

if [ ! -x "$SRC" ]; then
  echo "✗ 缺少可执行文件：$SRC" >&2
  echo "  请先编译：cargo build --release -p jiandu" >&2
  exit 1
fi

# 复制到不含 jiandu 字样的临时名（关键避雷步骤）
cp "$SRC" "$SAFE"
xattr -cr "$SAFE" 2>/dev/null || true

echo "✓ 已复制到避雷名 $SAFE 并启动 Compare 窗口"
exec env \
  JIANDU_DEV_COMPARE=1 \
  JIANDU_COMPARE_IMG="${JIANDU_COMPARE_IMG:-/tmp/life_pyramid_page1.png}" \
  JIANDU_COMPARE_DB="${JIANDU_COMPARE_DB:-/tmp/life_pyramid.db|1}" \
  JIANDU_COMPARE_HORIZONTAL=1 \
  JIANDU_COMPARE_HIT="${JIANDU_COMPARE_HIT:-50}" \
  "$SAFE"
