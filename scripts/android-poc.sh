#!/usr/bin/env bash
# 笺读 · Android 跨平台破冰 POC
# 目的：仅验证 core 能否用 cargo-ndk 交叉编译到 aarch64-linux-android，
#       重点观察唯一带 C 依赖的 rusqlite(bundled) 会不会卡在 C 编译。
# 不涉及任何 UI 代码。
#
# 前置（见 ANDROID_POC_REPORT.md）：
#   1. 安装 Android NDK，并 export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/<ver>
#   2. 本脚本会自动装 cargo-ndk、加 rust 目标（若缺失）
# 用法：bash scripts/android-poc.sh
set -euo pipefail

TARGET=aarch64-linux-android

echo ">> [1/4] 检查 Android NDK ..."
NDK="${ANDROID_NDK_HOME:-${NDK_HOME:-}}"
if [ -z "$NDK" ] && [ -d "$HOME/Library/Android/sdk/ndk" ]; then
  NDK=$(ls -d "$HOME/Library/Android/sdk/ndk"/* 2>/dev/null | head -1)
fi
if [ -z "$NDK" ] || [ ! -d "$NDK" ]; then
  echo "❌ 未找到 Android NDK（Rust 交叉编译 C 依赖的硬前置）。"
  echo "   请先安装 NDK，然后："
  echo "     export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/<version>"
  echo "   详见 ANDROID_POC_REPORT.md 的『环境清单』。"
  exit 1
fi
echo "   NDK = $NDK"

echo ">> [2/4] 检查 cargo-ndk ..."
if ! command -v cargo-ndk >/dev/null 2>&1; then
  echo "   cargo-ndk 未安装，尝试 cargo install cargo-ndk --locked ..."
  cargo install cargo-ndk --locked
fi
cargo ndk --version

echo ">> [3/4] 检查 Rust 目标 $TARGET ..."
rustup target add "$TARGET" >/dev/null 2>&1 || true

echo ">> [4/4] 交叉编译 jiandu-core -> $TARGET (staticlib) ..."
# 只编 core（不含 GPUI 壳）。cargo-ndk 会注入 NDK 的 clang/ar + sysroot，
# rusqlite(bundled) 的 SQLite C 源码即由它跨平台编过。
cargo ndk -t "$TARGET" build -p jiandu-core
echo "✅ 成功！产物：target/$TARGET/debug/libjiandu_core.a"
