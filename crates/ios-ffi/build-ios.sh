#!/usr/bin/env bash
# 笺读 · iOS 构建脚本：交叉编译 core+FFI → 生成 Swift 绑定 → 打包 XCFramework。
#
# 前置：Xcode（含 iOS SDK）+ Rust iOS target。
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
#
# 用法（在仓库根或本目录执行均可）：
#   ./crates/ios-ffi/build-ios.sh
#
# 产物：
#   bindings/jiandu_ios_ffi.swift + module.modulemap   （Swift 绑定，拖进 Xcode 作源码）
#   JianDuCore.xcframework                            （喂给 Xcode，device+sim 双 slice）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "==> [1/5] 加 iOS target"
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

echo "==> [2/5] 交叉编译 device + simulator 静态库"
cargo build --release --target aarch64-apple-ios
cargo build --release --target aarch64-apple-ios-sim

DEVICE_A="target/aarch64-apple-ios/release/libjiandu_ios_ffi.a"
SIM_A="target/aarch64-apple-ios-sim/release/libjiandu_ios_ffi.a"

echo "==> [3/5] 生成 Swift 绑定（从 device 静态库读取 uniffi 元数据）"
cargo run --bin uniffi-bindgen -- generate \
  --library "$DEVICE_A" \
  --language swift --out-dir bindings
# Xcode 只认 module.modulemap
if [ -f "bindings/jiandu_ios_ffiFFI.modulemap" ]; then
  mv bindings/jiandu_ios_ffiFFI.modulemap bindings/module.modulemap
fi

# 仅把 C 桥接头（.h + module.modulemap）抽出做 xcframework 的 Headers，
# 不要把生成的 .swift 也塞进 Headers（否则 Xcode 消费时会混淆：.swift 必须
# 由 App 工程单独编译，xcframework 只提供原生符号 + C module）。
mkdir -p ffi-headers
cp bindings/jiandu_ios_ffiFFI.h ffi-headers/
cp bindings/module.modulemap ffi-headers/

echo "==> [4/5] 打包 XCFramework（device/sim 两套 arm64 不能同 fat.a，必须用 xcframework）"
rm -rf JianDuCore.xcframework
xcodebuild -create-xcframework \
  -library "$DEVICE_A" -headers ffi-headers \
  -library "$SIM_A"   -headers ffi-headers \
  -output JianDuCore.xcframework

# 清理临时头文件目录（构建产物，不应留在源码树）
rm -rf ffi-headers

echo "==> [5/5] 校验：XCFramework 内含两份真静态库"
count=$(find JianDuCore.xcframework -name '*.a' | wc -l | tr -d ' ')
if [ "$count" -ge 2 ]; then
  echo "✅ JianDuCore.xcframework 已生成（含 $count 份静态库切片：device + simulator）"
else
  echo "❌ 未找到足够的静态库切片（找到 $count 份）"
  exit 1
fi
