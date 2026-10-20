@echo off
chcp 65001 >nul
setlocal

cd /d "%~dp0"

echo ===============================================
echo   笺读 JianDu . 中文阅读器
echo ===============================================
echo.

where cargo >nul 2>nul
if errorlevel 1 (
    if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
        set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    )
)

where cargo >nul 2>nul
if errorlevel 1 (
    echo [X] 没有找到 Rust 工具链 cargo
    echo.
    echo 请先安装 Rust，只需一次:
    echo   1. 打开 https://rustup.rs
    echo   2. 下载 rustup-init.exe 并双击安装
    echo   3. 安装时选择 "Quick install via the Visual Studio Community"
    echo   4. 装好后重开本窗口，再运行本脚本
    echo.
    echo 详情见「开始使用.md」。
    echo.
    pause
    exit /b 1
)

for /f "tokens=*" %%v in ('cargo --version') do set CARGO_VER=%%v
echo [OK] %CARGO_VER%
echo.
echo [!] 注意: GPUI 框架对 Windows 支持较弱，编译可能失败。
echo     如失败，建议在 macOS 或 Linux 上运行。
echo.
echo -^> 开始编译，首次需要几分钟...
echo.
if "%~1"=="" (
    cargo run --release --bin jiandu
) else (
    echo -^> 命令行模式: %*
    echo.
    cargo run --release --bin jiandu -- %*
)

if errorlevel 1 (
    echo.
    echo [X] 编译或运行失败。请把上面的错误信息保存下来排查。
    pause
)

endlocal
