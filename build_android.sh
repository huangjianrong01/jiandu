#!/usr/bin/env bash
# ==============================================================================
# build_android.sh — 笺读 (JianDu) Android 签名 Release 包 一键构建脚本
# ==============================================================================
# 用法:
#   ./build_android.sh              # arm64-only  (推荐, ~9.3 MiB) 荣耀/OPPO/vivo 新机
#   ./build_android.sh aarch64      # 同上，显式指定
#   ./build_android.sh all          # 全架构 universal (~32 MiB)
#
# 产物: ~/Desktop/jiandu-android-<arch>-release.apk  (已签名, 可直接装机)
#
# 原理要点(踩过的坑, 改动前请先读):
#   1. 必须走 `cargo tauri android build` 编排器。Gradle 的 rust 插件靠编排器生成的
#      gen/android/.tauri/cli-options-server.json 驱动, 直接 ./gradlew 会报
#      "the tauri android dev or build command must be running"。
#   2. 移动端 Tauri 不把 frontendDist 嵌进 .so (tauri-build 移动端 assets=None),
#      前端靠 app/build.gradle.kts 里的 copyFrontendAssets 任务灌进 frontend_assets/
#      再声明为额外 assets 源目录。删掉那个任务 → APK 裸奔、界面不更新。
#   3. CLI 的 TAURI_ANDROID_KEY_* 自动签名在本工程不生效, 产物恒为 -unsigned,
#      因此本脚本用 apksigner 手动签名。
# ==============================================================================
set -euo pipefail

TARGET_ARG="${1:-aarch64}"

PROJ_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TAURI_DIR="$PROJ_ROOT/jiandu-tauri"
GEN_DIR="$TAURI_DIR/src-tauri/gen/android"
KEYSTORE="$GEN_DIR/jiandu.keystore"
KS_ALIAS="jiandu"
KS_PASS="123456"

OUT_DIR="$GEN_DIR/app/build/outputs/apk/universal/release"
UNSIGNED="$OUT_DIR/app-universal-release-unsigned.apk"

log() { printf '\033[1;36m==> %s\033[0m\n' "$*"; }
ok()  { printf '\033[1;32m    ✓ %s\033[0m\n' "$*"; }
err() { printf '\033[1;31m    ✗ %s\033[0m\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- 1. 环境检查
log "1/6 检查环境"

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
[ -d "$ANDROID_HOME" ] || err "未找到 Android SDK: $ANDROID_HOME (请设置 ANDROID_HOME)"

# JDK 17: 优先沿用已有 JAVA_HOME, 否则依次试 java_home / Android Studio 自带 jbr
if [ -z "${JAVA_HOME:-}" ] || [ ! -x "$JAVA_HOME/bin/java" ]; then
  if [ -x /usr/libexec/java_home ]; then
    JAVA_HOME="$(/usr/libexec/java_home -v 17 2>/dev/null || true)"
  fi
fi
if [ -z "${JAVA_HOME:-}" ] || [ ! -x "$JAVA_HOME/bin/java" ]; then
  for j in "/Applications/Android Studio.app/Contents/jbr/Contents/Home"; do
    [ -x "$j/bin/java" ] && JAVA_HOME="$j" && break
  done
fi
[ -n "${JAVA_HOME:-}" ] && [ -x "$JAVA_HOME/bin/java" ] || err "未找到 JDK 17, 请设置 JAVA_HOME"
export JAVA_HOME
export PATH="$JAVA_HOME/bin:$PATH"
ok "ANDROID_HOME=$ANDROID_HOME"
ok "JAVA_HOME=$JAVA_HOME"

# apksigner: 取 build-tools 下可用的最新一个
APKSIGNER=""
for d in "$ANDROID_HOME"/build-tools/*/apksigner; do
  [ -x "$d" ] && APKSIGNER="$d"
done
[ -n "$APKSIGNER" ] || err "未找到 apksigner, 请在 Android SDK 里安装 build-tools"
ok "apksigner=$APKSIGNER"

# ------------------------------------------------------------ 2. 签名密钥准备
log "2/6 签名密钥"
if [ ! -f "$KEYSTORE" ]; then
  log "    keystore 不存在, 自动生成 (别名 $KS_ALIAS)"
  keytool -genkey -v -keystore "$KEYSTORE" -alias "$KS_ALIAS" \
    -keyalg RSA -keysize 2048 -validity 10000 \
    -storepass "$KS_PASS" -keypass "$KS_PASS" \
    -dname "CN=Jiandu, OU=Dev, O=Jiandu, L=, S=, C=CN" >/dev/null
fi
ok "keystore=$KEYSTORE"
ok "注意: 这是开发用密钥, 已被 .gitignore 排除, 切勿提交或外传"

# ------------------------------------------------------------------ 3. 构建
log "3/6 构建 (cargo tauri android build)"
cd "$TAURI_DIR"
if [ "$TARGET_ARG" = "all" ]; then
  OUT_NAME="jiandu-android-universal-release.apk"
  cargo tauri android build --apk
else
  # Rust triple -> 产物里的架构标签(arm64 比 aarch64 更直观)
  case "$TARGET_ARG" in
    aarch64|arm64) OUT_TAG="arm64"; RUST_TARGET="aarch64" ;;
    armv7|arm)     OUT_TAG="arm";   RUST_TARGET="armv7"   ;;
    i686|x86)      OUT_TAG="x86";   RUST_TARGET="i686"    ;;
    *)             OUT_TAG="$TARGET_ARG"; RUST_TARGET="$TARGET_ARG" ;;
  esac
  OUT_NAME="jiandu-android-${OUT_TAG}-release.apk"
  cargo tauri android build --apk --target "$RUST_TARGET"
fi
[ -f "$UNSIGNED" ] || err "未找到构建产物: $UNSIGNED"
ok "未签名包: $(wc -c < "$UNSIGNED" | tr -d ' ') bytes"

# -------------------------------------------------------- 4. 校验前端资源进包
log "4/6 校验包内前端资源 (防 APK 裸奔)"
tmp_css="$(mktemp)"
trap 'rm -f "$tmp_css"' EXIT
unzip -p "$UNSIGNED" assets/styles.css > "$tmp_css" 2>/dev/null || true
grep -q "Songti" "$tmp_css" \
  && ok "styles.css 含宋体回退链 Songti" \
  || err "styles.css 缺 Songti —— 前端没进包! 检查 copyFrontendAssets 任务"
unzip -p "$UNSIGNED" assets/index.html | grep -q "testCoreBtn" \
  && err "index.html 仍含旧 testCoreBtn —— 打进的是旧前端!" \
  || ok "index.html 无旧按钮残留"

# ------------------------------------------------------------------ 5. 签名
log "5/6 apksigner 签名 + 校验"
cp "$UNSIGNED" "$OUT_DIR/$OUT_NAME"
"$APKSIGNER" sign --ks "$KEYSTORE" --ks-key-alias "$KS_ALIAS" \
  --ks-pass "pass:$KS_PASS" --key-pass "pass:$KS_PASS" \
  "$OUT_DIR/$OUT_NAME"
"$APKSIGNER" verify "$OUT_DIR/$OUT_NAME" >/dev/null \
  && ok "签名校验通过" \
  || err "签名校验失败"

# ------------------------------------------------------------------ 6. 交付
log "6/6 交付到桌面"
cp "$OUT_DIR/$OUT_NAME" "$HOME/Desktop/$OUT_NAME"
SIZE="$(wc -c < "$HOME/Desktop/$OUT_NAME" | tr -d ' ')"
MB="$(awk "BEGIN{printf \"%.2f\", $SIZE/1048576}")"
ABIS="$(unzip -l "$HOME/Desktop/$OUT_NAME" | grep -oE 'lib/[a-z0-9_-]+/' | sort -u | tr '\n' ' ')"
ok "成品: $HOME/Desktop/$OUT_NAME"
printf '\n\033[1;33m    大小: %s bytes (%.2f MiB)\n' "$SIZE" "$(awk "BEGIN{printf \"%.2f\", $SIZE/1048576}")"
printf '    架构: %s\033[0m\n\n' "$ABIS"
echo "    传给手机安装即可。祝你验收顺利!"
