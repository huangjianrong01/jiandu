# 云端打包教程：如何在 GitHub 上自动产出 Windows 安装包

本项目的 Windows 安装包（`.msi` / `.exe`）**不在 Mac/Linux 本机交叉编译**，
而是交给 **GitHub Actions 的 `windows-latest` 云端 runner** 构建。
你只需要在 Mac 上写代码、推送，云端会自动出包。

> 为什么不在本机交叉编译？macOS 缺少 Windows 专属的 MSVC 工具链与
> WiX/NSIS 打包器，且 Tauri 安装包只能在目标平台上构建。云端 Windows 环境
> 一站式解决。详见仓库根目录 `.github/workflows/release.yml`。

---

## 一、把项目推送到 GitHub

如果你还没建远程仓库：

```bash
cd jiandu                      # 仓库根目录
git remote add origin https://github.com/<你的用户名>/jiandu.git
git push -u origin feature/windows-tauri     # 推当前分支
# 或者把成果合并回 main 后再推 main
```

如果已建好远程，直接：

```bash
git add -A
git commit -m "feat: Tauri 壳 + 云端打包流水线 + 系统支持检查"
git push
```

> 流水线文件位于 `.github/workflows/release.yml`，必须随代码一起推到 GitHub 才会生效。

---

## 二、触发打包的三种方式

| 触发方式 | 做法 | 产物去处 |
| --- | --- | --- |
| **推送到 `main`** | `git push origin main` | 仅上传 **Artifact**（Actions 页面可下载） |
| **打 `v*` tag** | `git tag v1.0.0 && git push origin v1.0.0` | 自动发到 **GitHub Release**（含 `.msi`/`.exe`） |
| **手动触发** | 进仓库 *Actions → Build Windows Installer → Run workflow* | 同"推送到 main" |

### 下载产物

- **Artifact 方式**：仓库 *Actions* 页 → 对应运行 → 底部 *Artifacts* 区 → `jiandu-windows-installer.zip` 下载解压即得 `.msi` 与 `.exe`。
- **Release 方式**：仓库 *Releases* 页 → 对应版本 → 直接下载 `.msi` / `.exe`。

---

## 三、流水线在做什么（release.yml 精要）

1. `actions/checkout` 检出代码（包含 `crates/core` 内核，因为是相对路径依赖）。
2. `actions/setup-node` 装 Node.js 20。
3. `dtolnay/rust-toolchain` 装 Rust，并显式带 `x86_64-pc-windows-msvc` 标准库。
4. `Swatinem/rust-cache` 缓存 Rust 编译，加速后续构建。
5. `cargo install tauri-cli` 装 Tauri v2 CLI。
6. `cargo tauri build` 在子目录 `jiandu-tauri/` 编译并打包 → 产出
   `jiandu-tauri/src-tauri/target/release/bundle/{msi,nsis}`。
7. `actions/upload-artifact` 上传安装包；若为 tag 则再用 `softprops/action-gh-release` 发到 Release。

> 等价更快的替代：把第 5 步换成 `npm install`（已有 `@tauri-apps/cli` 依赖），
> 然后 `npx tauri build`，省去 `cargo install` 的源码编译时间。

---

## 四、注意事项

### 1. 系统支持范围
- **仅支持 Windows 10（1803 及以上，build ≥ 17134）与 Windows 11**。
- 老旧系统（Windows 7 / 8 等）因 WebView2 不可用而无法运行；程序首屏会通过
  `os_support` 命令检测并弹出友好引导页，指向「云端代排服务」。

### 2. 代码签名（建议，但非必须）
当前流水线产出的是**未签名**安装包。Windows SmartScreen 会对未签名程序
弹安全警告（用户点"仍要运行"即可）。正式发布前建议购买代码签名证书，
在 workflow 里加 `tauri.conf.json` 的 `bundle.windows` 签名配置（或
`tauri-action` 的 `signingIdentity`）。

### 3. 图标
打包需要 `src-tauri/icons/` 下的 `.ico` / `.icns` / `.png`（模板已默认生成），
请勿删除。

### 4. 首次构建较慢
`cargo tauri build` 首次会编译整个 Tauri + wry + 内核依赖（含 bundled SQLite 的
C 编译），云端约 10–20 分钟属正常；有 Rust 缓存后显著变快。

---

## 五、本地校验（可选）

本机（macOS）只验证开发态即可，无需出 Windows 包：

```bash
cd jiandu-tauri
cargo tauri dev      # 弹原生窗口，点「导入书籍」/「朗读」验证功能
```
