#!/usr/bin/env bash
#
# **core 纯洁性守卫**（v1.0.0-beta）
#
# 干什么：检查 `crates/core` 里有没有混进任何「跟某一台机器 / 某一个 UI
# 框架」绑定的东西。它是整个跨平台计划的**地基**——core 一旦沾上 GPUI 或
# CoreGraphics，Windows / iOS / Android 壳子就得把这堆东西一起拖走，
# 那时候再说「复用」就是空话。
#
# 判据（够用就好，宁可漏报也不误报）：
#   1. 源码里出现 `use gpui` / `use cocoa` / `use objc` / `use core_graphics`
#      / `use core_text` / `use core_foundation` / `use rfd` / `use image`；
#   2. `extern crate` 同上；
#   3. `#[link(name = "objc"|"cocoa")]`（直连 Objective-C 运行时）；
#   4. `Cargo.toml` 里声明了这些依赖。
#
# 只匹配**真正的代码行**（行首可选的空白 + `use `），所以文档注释里写
# 「不许出现 `use gpui`」不会把自己误伤。
#
# 用法：
#   ./scripts/check_core_purity.sh          # 违规即 exit 1
#
# 建议挂到 pre-commit：
#   ln -s ../../scripts/check_core_purity.sh .git/hooks/pre-commit

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORE="$ROOT/crates/core"

if [ ! -d "$CORE" ]; then
    echo "✗ 找不到 $CORE（是不是在别的目录跑的？）"
    exit 1
fi

# 违禁的 crate 名（Rust 标识符形式，下划线与连字符都要覆盖）
BANNED='(gpui|cocoa|objc|core_graphics|core-graphics|core_text|core-text|core_foundation|core-foundation|rfd|image|foreign_types|foreign-types|block2|objc2)'

FAIL=0
report() {
    echo "✗ core 纯洁性检查未通过："
    echo "$1"
    FAIL=1
}

# ── 1) 源码 ────────────────────────────────────────────────────────────────
PAT="^[[:space:]]*(use[[:space:]]+${BANNED}|extern[[:space:]]+crate[[:space:]]+${BANNED})|^[[:space:]]*#\[link\(name[[:space:]]*=[[:space:]]*\"(objc|cocoa|Cocoa|CoreGraphics|CoreText)\"\)\]"
HITS="$(grep -rEn "$PAT" "$CORE/src" --include='*.rs' 2>/dev/null || true)"
if [ -n "$HITS" ]; then
    report "$HITS"
fi

# ── 2) Cargo.toml ──────────────────────────────────────────────────────────
TOML_HITS="$(grep -En "^[[:space:]]*${BANNED}[[:space:]]*=" "$CORE/Cargo.toml" 2>/dev/null || true)"
if [ -n "$TOML_HITS" ]; then
    report "$TOML_HITS"
fi

# ── 3) 平台特判：源码里不该出现 macOS / Windows 的 cfg 分支 ────────────────
# core 要么跨平台，要么把差异做成 trait；在 core 里写
# `#[cfg(target_os = "macos")]` 就是把这个决定往后推，早晚要还。
CFG_HITS="$(grep -rEn '#\[cfg\(target_os' "$CORE/src" --include='*.rs' 2>/dev/null || true)"
if [ -n "$CFG_HITS" ]; then
    report "$CFG_HITS
   → core 里不该按平台分叉：请把差异抽成 trait（见 crates/core/src/platform.rs），
     由壳子注入实现。"
fi

if [ "$FAIL" -ne 0 ]; then
    exit 1
fi

echo "✓ core 纯洁：$(find "$CORE/src" -name '*.rs' | wc -l | tr -d ' ') 个文件、$(find "$CORE/src" -name '*.rs' -exec cat {} + | wc -l | tr -d ' ') 行，零 UI / 零平台依赖。"
exit 0
