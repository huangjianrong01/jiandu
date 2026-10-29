# 笺读 · Tauri 跨平台壳（jiandu-tauri）

笺读的桌面端外壳：用 [Tauri 2.0](https://tauri.app/) 承载纯 Rust 内核
[`jiandu-core`](../crates/core)，把古籍导入 / 阅读 / 朗读跑在原生窗口里。

- 前端：纯原生 HTML / CSS / JS（无框架、无 Vite、无需 `npm install`）
- 后端：Rust 直接 `path` 依赖 `../../crates/core`，通过 `#[tauri::command]` 暴露能力
- 已接入能力：`import_and_parse`（全格式导入：epub/txt/docx/json/md）、`test_core`、`os_support`、TTS 朗读（`window.speechSynthesis`）

## 本地开发

```bash
cd jiandu-tauri
cargo tauri dev      # 启动原生窗口 + 热重载
```

> 首次编译会拉取并编译 Tauri / wry / WebView 依赖，耗时较长，之后很快。

## 系统支持范围（重要）

**Windows 版本仅支持：**

- **Windows 10（版本 1803 及以上，即 build ≥ 17134）**
- **Windows 11**

Tauri 2 依赖系统内置的 **WebView2** 运行时，而 WebView2 在 Windows 7 / 8
等老旧系统上不可用，因此这些系统无法运行本桌面程序。

### 老旧系统的友好引导

对于不支持的系统（如 Windows 7），程序会在**首屏**通过 `os_support` 命令
做系统版本检查，若判定不支持则弹出引导页，提示用户：

- 升级到 Windows 10（1803+）/ Windows 11；或
- 直接使用我们的 **「云端代排服务」**——在浏览器里即可完成古籍排版与导出。

> 云端代排服务的跳转地址在 `src/main.js` 的 `CLOUD_URL` 常量中配置（当前为占位地址，
> 上线前请替换为真实域名）。

## 云端打包（Windows 安装包）

我们不在本机交叉编译，而是用 **GitHub Actions 云端 Windows runner** 出包：
推送代码 / 打 tag 即可在云端自动产出 `.msi` / `.exe`。

详细步骤见 **[WINDOWS_BUILD_GUIDE.md](./WINDOWS_BUILD_GUIDE.md)**，
流水线定义见 [`.github/workflows/release.yml`](../.github/workflows/release.yml)。

## 目录结构

```
jiandu-tauri/
├── src/                 # 前端（纯静态，被 Tauri 直接打包）
├── src-tauri/           # Rust 端（独立 [workspace]，依赖 ../../crates/core）
│   ├── src/lib.rs       # 命令：import_and_parse / test_core / os_support
│   ├── tauri.conf.json  # Tauri 配置（productName / identifier / bundle）
│   └── Cargo.toml
└── WINDOWS_BUILD_GUIDE.md
```
