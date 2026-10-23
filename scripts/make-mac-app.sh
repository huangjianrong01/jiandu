#!/usr/bin/env bash
# ============================================================
# 笺读 JianDu · macOS App 打包脚本
#
# 用法：
#   ./scripts/make-mac-app.sh            # 编译 release 并打包
#   SKIP_BUILD=1 ./scripts/make-mac-app.sh   # 用已编译好的二进制打包
#
# 产出：dist/笺读.app（可直接拖到「应用程序」文件夹）
# ============================================================
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
APP_NAME="笺读"
BUNDLE_ID="com.jiandu.app"
VERSION="1.30.0"
BUILD_NUM="27"
MIN_MACOS="12.0"

DIST="$ROOT/dist"
APP="$DIST/${APP_NAME}.app"

echo "==============================================="
echo "  笺读 · macOS App 打包"
echo "==============================================="

# ---- 1) 编译 release 二进制 ----
if [ "${SKIP_BUILD:-0}" != "1" ]; then
    echo "→ 编译 release（首次需要几分钟）..."
    cargo build --release --bin jiandu
fi

# 定位二进制（尊重 CARGO_TARGET_DIR，默认 ./target）
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/release/jiandu"
if [ ! -f "$BIN" ]; then
    echo "✗ 找不到二进制：$BIN"
    echo "  先跑 cargo build --release，或用 SKIP_BUILD=1 配合已有二进制。"
    exit 1
fi
echo "✓ 二进制：$BIN"

# ---- 2) 清理旧 bundle ----
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# ---- 3) 放入可执行文件 ----
cp "$BIN" "$APP/Contents/MacOS/jiandu"
chmod +x "$APP/Contents/MacOS/jiandu"

# ---- 4) 随包资源：字体（可选） ----
if [ -d "$ROOT/resources/fonts" ] && ls "$ROOT/resources/fonts"/*.otf "$ROOT/resources/fonts"/*.ttf "$ROOT/resources/fonts"/*.ttc >/dev/null 2>&1; then
    mkdir -p "$APP/Contents/Resources/fonts"
    cp "$ROOT/resources/fonts"/*.otf "$ROOT/resources/fonts"/*.ttf "$ROOT/resources/fonts"/*.ttc "$APP/Contents/Resources/fonts/" 2>/dev/null || true
    echo "✓ 已打包随包字体：$(ls "$APP/Contents/Resources/fonts" | tr '\n' ' ')"
else
    echo "· 无随包字体（将回退到系统宋体 / 思源，可在 resources/fonts/ 放入 .otf 后重新打包）"
fi

# ---- 5) App 图标：PNG → icns ----
ICON_SRC="$ROOT/assets/icon_1024.png"
if [ ! -f "$ICON_SRC" ]; then
    echo "→ 生成图标..."
    swift "$ROOT/scripts/gen-icon.swift" "$ICON_SRC"
fi
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
# 标准 iconset 命名：base 尺寸 + @2x 视网膜尺寸（注意不能用 out 当文件名，否则同名覆盖）
for spec in "16 16" "16 32" "32 32" "32 64" "128 128" "128 256" "256 256" "256 512" "512 512" "512 1024"; do
    set -- $spec
    base=$1; px=$2
    if [ "$base" = "$px" ]; then
        name="icon_${base}x${base}.png"
    else
        name="icon_${base}x${base}@2x.png"
    fi
    sips -z "$px" "$px" "$ICON_SRC" --out "$ICONSET/$name" >/dev/null
done
if iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null; then
    echo "✓ 图标已生成（iconutil）：AppIcon.icns"
else
    echo "· iconutil 不可用，改用 Python PIL 生成..."
    python3 - "$ICON_SRC" "$APP/Contents/Resources/AppIcon.icns" <<'PY'
from PIL import Image, os
src = Image.open(os.sys.argv[1]).convert("RGBA")
src.save(os.sys.argv[2], format="ICNS")
PY
    echo "✓ 图标已生成（PIL）：AppIcon.icns"
fi
rm -rf "$ICONSET"
echo "✓ 图标已就绪"

# ---- 6) Info.plist ----
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>zh_CN</string>
    <key>CFBundleDisplayName</key>
    <string>笺读</string>
    <key>CFBundleExecutable</key>
    <string>jiandu</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>笺读</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${BUILD_NUM}</string>
    <key>CFBundleSupportedPlatforms</key>
    <array>
        <string>MacOSX</string>
    </array>
    <key>LSMinimumSystemVersion</key>
    <string>${MIN_MACOS}</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Markdown 书籍</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>md</string>
                <string>markdown</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>纯文本书籍</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>txt</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>EPUB 电子书</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>epub</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>笺读书籍 JSON</string>
            <key>CFBundleTypeExtensions</key>
            <array>
                <string>json</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
        </dict>
    </array>
</dict>
</plist>
PLIST
echo "✓ Info.plist 已写入"

# ---- 7) ad-hoc 签名（本地运行更顺；联网分发需 Apple Developer 证书） ----
codesign --force --deep --sign - "$APP" 2>/dev/null && echo "✓ ad-hoc 签名完成" || echo "· 跳过签名（不影响本地运行）"
if codesign --verify --deep "$APP" 2>/dev/null; then
    echo "✓ 签名校验通过"
else
    echo "· 签名校验未通过（本地运行通常无碍；如报 sealed resource，重新执行本脚本即可）"
fi

# ---- 8) 收尾 ----
SIZE="$(du -sh "$APP" | cut -f1)"
echo ""
echo "==============================================="
# 注意：变量名后用花括号括起来（${APP}${SIZE}），避免非 UTF-8 locale 下
# bash 把全角括号的首字节误认作变量名一部分而报 unbound variable。
echo "  ✅ 打包完成：${APP}（${SIZE}）"
echo "==============================================="
echo ""
echo "  使用方法："
echo "    1) 双击打开；若首次被 Gatekeeper 拦住，"
echo "       在图标上右键 → 打开 → 再点「打开」即可；"
echo "    2) 或拖到「应用程序」文件夹；"
echo "    3) 之后就能像普通 App 一样从启动台 / Dock 打开。"
echo ""
echo "  卸载：把「笺读.app」拖进废纸篓即可；"
echo "  用户数据在 ~/.jiandu/（书架索引与阅读进度），可留可删。"
