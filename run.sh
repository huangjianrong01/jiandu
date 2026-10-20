#!/usr/bin/env bash
# 笺读 JianDu 一键启动脚本（macOS / Linux）
set -e

cd "$(dirname "$0")"

echo "==============================================="
echo "  笺读 JianDu · 中文阅读器"
echo "  不带参数：打开书架 ｜ 带参数：格式转换"
echo "==============================================="

# 1) 检查 cargo
if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1090
        . "$HOME/.cargo/env"
    fi
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo ""
    echo "✗ 没有找到 Rust 工具链（cargo）"
    echo ""
    echo "请先安装 Rust，只需一次："
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo ""
    echo "安装后重开终端，或执行："
    echo "  source \"\$HOME/.cargo/env\""
    echo ""
    echo "然后重新运行本脚本。详情见「开始使用.md」。"
    exit 1
fi

echo "✓ Rust: $(cargo --version)"

# 2) 检查系统
OS="$(uname -s)"
if [ "$OS" = "Linux" ]; then
    if ! command -v pkg-config >/dev/null 2>&1; then
        echo ""
        echo "⚠ 检测到 Linux，建议先安装系统依赖："
        echo "  sudo apt install -y build-essential pkg-config libssl-dev \\"
        echo "    libx11-dev libxkbcommon-dev libwayland-dev \\"
        echo "    libfontconfig1-dev libfreetype6-dev"
        echo ""
    fi
fi

# 3) 编译运行
echo "→ 开始编译（首次需要几分钟，请耐心等待）..."
echo ""
# 4) 有参数就原样透传（格式转换等 CLI 用法），没参数就直接开窗口
if [ "$#" -gt 0 ]; then
    echo "→ 命令行模式：$*"
    echo ""
    cargo run --release --bin jiandu -- "$@"
else
    cargo run --release --bin jiandu
fi
