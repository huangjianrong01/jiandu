#!/usr/bin/env bash
# ============================================================
# 笺读 JianDu · macOS App 打包脚本
#
# 用法：
#   ./scripts/make-mac-app.sh            # 编译 release 并打包（本机架构）
#   ARCH=universal ./scripts/make-mac-app.sh  # 打通用包（Apple Silicon + Intel）
#   SKIP_BUILD=1 ./scripts/make-mac-app.sh   # 用已编译好的二进制打包
#   SANDBOX=1 ./scripts/make-mac-app.sh      # ad-hoc 签名也带上沙盒 entitlements
#                                            # → 本地即可真跑沙盒，验证导入/导出路径
#
# 产出：dist/笺读.app（可直接拖到「应用程序」文件夹）
#
# v1.71.0：ARCH=universal 会各编一份 aarch64 / x86_64 再用 lipo 合成，
#          一份 .app 同时支持 Apple Silicon 与 Intel Mac。
#          前置：rustup target add x86_64-apple-darwin
# ============================================================
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
APP_NAME="笺读"
BUNDLE_ID="com.jiandu.app"
VERSION="1.9.9"
BUILD_NUM="220"
MIN_MACOS="12.0"
ARCH="${ARCH:-native}"

DIST="$ROOT/dist"
APP="$DIST/${APP_NAME}.app"

echo "==============================================="
echo "  笺读 · macOS App 打包"
echo "==============================================="

# ---- 1) 编译 release 二进制 ----
if [ "${SKIP_BUILD:-0}" != "1" ]; then
    if [ "$ARCH" = "universal" ]; then
        # 本机那份走默认 target 目录（能复用已编好的产物，省一次全量编译）；
        # 另一份才开独立 target 子目录。
        echo "→ 编译 release（本机 $(uname -m)，首次需要几分钟）..."
        cargo build --release -p jiandu --bin jiandu
        if [ "$(uname -m)" = "arm64" ]; then
            echo "→ 编译 release（intel x86_64）..."
            cargo build --release -p jiandu --bin jiandu --target x86_64-apple-darwin
        else
            echo "→ 编译 release（apple silicon arm64）..."
            cargo build --release -p jiandu --bin jiandu --target aarch64-apple-darwin
        fi
    else
        echo "→ 编译 release（首次需要几分钟）..."
        cargo build --release -p jiandu --bin jiandu
    fi
fi

# 定位二进制（尊重 CARGO_TARGET_DIR，默认 ./target）
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
if [ "$ARCH" = "universal" ]; then
    if [ "$(uname -m)" = "arm64" ]; then
        ARM_BIN="$TARGET_DIR/release/jiandu"
        X86_BIN="$TARGET_DIR/x86_64-apple-darwin/release/jiandu"
    else
        ARM_BIN="$TARGET_DIR/aarch64-apple-darwin/release/jiandu"
        X86_BIN="$TARGET_DIR/release/jiandu"
    fi
    for b in "$ARM_BIN" "$X86_BIN"; do
        if [ ! -f "$b" ]; then
            echo "✗ 找不到二进制：$b（先跑 rustup target add x86_64-apple-darwin）"
            exit 1
        fi
    done
    # 注意：全角括号会被非 UTF-8 locale 的 bash 误认作变量名的一部分
    # （$BIN（ → 报 unbound variable），所以这里一律用 ${BIN} 显式定界。
    BIN="$DIST/jiandu-universal"
    mkdir -p "$DIST"
    lipo -create -output "${BIN}" "$ARM_BIN" "$X86_BIN"
    echo "✓ 通用二进制：${BIN}（架构：$(lipo -archs "${BIN}")）"
else
    BIN="$TARGET_DIR/release/jiandu"
    if [ ! -f "$BIN" ]; then
        echo "✗ 找不到二进制：$BIN"
        echo "  先跑 cargo build --release，或用 SKIP_BUILD=1 配合已有二进制。"
        exit 1
    fi
    echo "✓ 二进制：$BIN"
fi

# ---- 2) 清理旧 bundle ----
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# ---- 3) 放入可执行文件 ----
cp "$BIN" "$APP/Contents/MacOS/jiandu"
chmod +x "$APP/Contents/MacOS/jiandu"

# ---- 4) 随包资源：字体（可选） ----
if [ -d "$ROOT/crates/core/resources/fonts" ] && ls "$ROOT/crates/core/resources/fonts"/*.otf "$ROOT/crates/core/resources/fonts"/*.ttf "$ROOT/crates/core/resources/fonts"/*.ttc >/dev/null 2>&1; then
    mkdir -p "$APP/Contents/Resources/fonts"
    cp "$ROOT/crates/core/resources/fonts"/*.otf "$ROOT/crates/core/resources/fonts"/*.ttf "$ROOT/crates/core/resources/fonts"/*.ttc "$APP/Contents/Resources/fonts/" 2>/dev/null || true
    echo "✓ 已打包随包字体：$(ls "$APP/Contents/Resources/fonts" | tr '\n' ' ')"
else
    echo "· 无随包字体（将回退到系统宋体 / 思源，可在 crates/core/resources/fonts/ 放入 .otf 后重新打包）"
fi

# ---- 4.5) 随包资源：内置书目（运行时读 Resources/books，方便热更新） ----
if [ -d "$ROOT/crates/core/resources/books" ] && ls "$ROOT/crates/core/resources/books"/*.json >/dev/null 2>&1; then
    mkdir -p "$APP/Contents/Resources/books"
    cp "$ROOT/crates/core/resources/books"/*.json "$APP/Contents/Resources/books/" 2>/dev/null || true
    echo "✓ 已打包内置书目：$(ls "$APP/Contents/Resources/books" | tr '\n' ' ')"
else
    echo "· 无 crates/core/resources/books/（内置书目将缺失，首启书架会空）"
fi

# ---- 5) App 图标：PNG → icns ----
# v1.82.0：v1.0.0-beta 起改成了 Cargo Workspace，资产整体挪进 crates/app/assets，
# 而脚本里这条路径还指着旧的 `$ROOT/assets/`——打出来的包会**没有图标**，
# 甚至在这里直接中断。故按新位置优先、旧位置兜底。
ICON_SRC="$ROOT/crates/app/assets/icon_1024.png"
if [ ! -f "$ICON_SRC" ] && [ -f "$ROOT/assets/icon_1024.png" ]; then
    ICON_SRC="$ROOT/assets/icon_1024.png"
fi
if [ ! -f "$ICON_SRC" ]; then
    echo "→ 生成图标..."
    mkdir -p "$(dirname "$ICON_SRC")"
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

# ---- 5.5) App Store 上传用的 1024 图标 ----
# bundle 里的 icns 允许带圆角与透明（Dock / 启动台看着才对）；
# 但 App Store Connect 另要单独上传一张 1024×1024 的 PNG，**不许有 alpha**——
# 有透明通道会被直接拒收。做法：把圆角外的空白用图标**自己的主色**填满
# （合成到白底会露出四个白角，很明显）。
python3 - "$ICON_SRC" "$DIST/appstore-icon-1024.png" <<'ICONPY'
from PIL import Image
import sys, collections

src = Image.open(sys.argv[1]).convert("RGBA").resize((1024, 1024), Image.LANCZOS)
# 底色 = 非透明像素里的主流颜色（图标自身的纸色），方角填它，天衣无缝。
cnt = collections.Counter()
for px in src.resize((64, 64), Image.LANCZOS).getdata():
    if px[3] > 200:
        cnt[px[:3]] += 1
base = cnt.most_common(1)[0][0] if cnt else (255, 255, 255)
bg = Image.new("RGBA", src.size, base + (255,))
bg.alpha_composite(src)
bg.convert("RGB").save(sys.argv[2])
ICONPY
echo "✓ App Store 上传用图标：dist/appstore-icon-1024.png（1024×1024，无 alpha）"

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
    <key>NSHumanReadableCopyright</key>
    <string>Copyright © 2026 Jiandu. All rights reserved.</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.books</string>
    <!-- 不用任何非豁免加密（无网络、无 HTTPS、无加密存档）：声明后
         App Store Connect 不再索要出口合规文件。 -->
    <key>ITSAppUsesNonExemptEncryption</key>
    <false/>
    <!-- v1.71.0：全仓已无 screencapture——「导出当前页图片」改走 CoreGraphics
         离屏重绘（见 reader::render_leaf_png），不再申请「屏幕录制」权限。 -->
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
            <key>LSItemContentTypes</key>
            <array>
                <string>net.daringfireball.markdown</string>
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
            <key>LSItemContentTypes</key>
            <array>
                <string>public.plain-text</string>
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
            <key>LSItemContentTypes</key>
            <array>
                <string>org.idpf.epub-container</string>
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
            <key>LSItemContentTypes</key>
            <array>
                <string>public.json</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
        </dict>
    </array>
    <!-- md / epub 不是系统内置 UTI，要在包内自行声明才能被 LaunchServices 认。 -->
    <key>UTImportedTypeDeclarations</key>
    <array>
        <dict>
            <key>UTTypeIdentifier</key>
            <string>net.daringfireball.markdown</string>
            <key>UTTypeDescription</key>
            <string>Markdown 书籍</string>
            <key>UTTypeConformsTo</key>
            <array>
                <string>public.plain-text</string>
            </array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key>
                <array>
                    <string>md</string>
                    <string>markdown</string>
                </array>
            </dict>
        </dict>
        <dict>
            <key>UTTypeIdentifier</key>
            <string>org.idpf.epub-container</string>
            <key>UTTypeDescription</key>
            <string>EPUB 电子书</string>
            <key>UTTypeConformsTo</key>
            <array>
                <string>public.data</string>
                <string>public.composite-content</string>
            </array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key>
                <array>
                    <string>epub</string>
                </array>
                <key>public.mime-type</key>
                <string>application/epub+zip</string>
            </dict>
        </dict>
    </array>
</dict>
</plist>
PLIST
echo "✓ Info.plist 已写入"

# ---- 7) 签名 ----
if [ "${APPSTORE:-0}" = "1" ]; then
    # —— App Store 分发：用正式的「3rd Party Mac Developer」证书 ——
    APP_CERT="${APP_CERT:?请先设置 APP_CERT，例如 '3rd Party Mac Developer Application: Your Name (TEAMID)'}"
    INSTALLER_CERT="${INSTALLER_CERT:?请先设置 INSTALLER_CERT，例如 '3rd Party Mac Developer Installer: Your Name (TEAMID)'}"
    PROFILE="${PROVISIONING_PROFILE:-$ROOT/scripts/embedded.provisionprofile}"
    if [ ! -f "$PROFILE" ]; then
        echo "✗ 找不到 provisioning profile：$PROFILE"
        echo "  在 Developer Portal 下载「Mac App Store」类型的 profile 放到此处，或设 PROVISIONING_PROFILE=路径"
        exit 1
    fi
    cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"
    echo "✓ 已嵌入 provisioning profile"

    # 用 entitlements + 强化运行时（App Store 必须）签名应用
    codesign --force --deep --sign "$APP_CERT" \
        --entitlements "$ROOT/jiandu.entitlements" \
        --options runtime "$APP"
    if codesign --verify --deep --strict "$APP" 2>/dev/null; then
        echo "✓ App Store 应用签名校验通过"
    else
        echo "✗ 应用签名校验失败"
        exit 1
    fi

    # 打包成 .pkg（用 Installer 证书），供上传 App Store Connect
    PKG="$DIST/${APP_NAME}-${VERSION}-${BUILD_NUM}.pkg"
    productbuild --sign "$INSTALLER_CERT" \
        --component "$APP" /Applications \
        "$PKG"
    echo "✓ 已生成安装包：$PKG"
    echo ""
    echo "   下一步：用 Transporter（或 Xcode Organizer）上传 $PKG 到 App Store Connect。"
else
    # —— 本地 / 直接分发：ad-hoc 签名 ——
    #
    # SANDBOX=1：ad-hoc 签名时**也**带上 entitlements（v1.73.0）。
    # 这样打出来的 .app 在本机双击就直接跑在 App Sandbox 里——上传 App Store
    # 之前，导入 / 导出 / 数据目录这些路径问题可以在本地**真跑一遍**验出来，
    # 而不是等审核被打回。ad-hoc 身份签的沙盒 App 本机可正常运行。
    # 沙盒下数据目录会被重定向到
    # ~/Library/Containers/com.jiandu.app/Data/Library/Application Support/com.jiandu.app，
    # 书架为空是正常的（旧书在容器外，沙盒读不到）——验收时重新导入一本即可。
    if [ "${SANDBOX:-0}" = "1" ]; then
        codesign --force --deep --sign - \
            --entitlements "$ROOT/jiandu.entitlements" \
            "$APP" 2>/dev/null && echo "✓ ad-hoc 签名完成（已启用 App Sandbox）" \
            || echo "· 跳过签名（不影响本地运行）"
    else
        codesign --force --deep --sign - "$APP" 2>/dev/null && echo "✓ ad-hoc 签名完成" || echo "· 跳过签名（不影响本地运行）"
    fi
    if codesign --verify --deep "$APP" 2>/dev/null; then
        echo "✓ 签名校验通过"
    else
        echo "· 签名校验未通过（本地运行通常无碍；如报 sealed resource，重新执行本脚本即可）"
    fi
fi

# ---- 7.5) 可选：DMG 安装镜像（DMG=1）----
#
# 为什么还要 dmg：内测同学拿到的应该是一个**能双击挂载、把图标拖进
# 「应用程序」**的东西，而不是一个 zip——zip 解压出来的 app 少了那层
# 「用户主动拖拽」的授权语境，Gatekeeper 更容易直接拒绝。
#
# 布局：卷里左「笺读.app」右「Applications」软链，经典拖拽式。
# 用 `ditto` 而不是 `cp -R`：前者保留资源叉、xattr 与符号链接本身，
# 后者会把签名相关的扩展属性丢掉（装完打不开就是这么来的）。
if [ "${DMG:-0}" = "1" ]; then
    DMG_OUT="$DIST/${APP_NAME}-${VERSION}.dmg"
    STAGE="$(mktemp -d)/dmg"
    mkdir -p "$STAGE"
    ditto "$APP" "$STAGE/${APP_NAME}.app"
    ln -s /Applications "$STAGE/Applications"
    rm -f "$DMG_OUT"
    if hdiutil create -volname "${APP_NAME} ${VERSION}" -srcfolder "$STAGE" \
        -ov -format UDZO -fs HFS+ "$DMG_OUT" >/dev/null; then
        echo "✓ 已生成 DMG：${DMG_OUT}（$(du -sh "$DMG_OUT" | cut -f1)）"
    else
        echo "✗ DMG 生成失败"
        exit 1
    fi
    rm -rf "$(dirname "$STAGE")"
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
echo "  用户数据在 ~/Library/Application Support/com.jiandu.app/"
echo "  （书架索引、章节库、阅读进度、自备字库都在里，卸载后可留可删）；"
echo "  v1.69 及更早版本的 ~/.jiandu/ 旧库会被**只读**挂载取回，不会删除。"
