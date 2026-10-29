# 笺读 · Android 跨平台技术评估与破冰报告（POC）

> 分支：`feature/android-poc`（隔离分支，不并回 `main`）
> 路线：Tauri 2.0 + core 纯内核复用（与 Windows 版同构）
> 状态：**底层可行性验证**，未写任何 UI 代码

---

## 0. 一句话结论

core 在**源码结构层面已经是 Android-ready** 的：零平台 `cfg` 代码、`crate-type` 已含 `staticlib`（安卓静态链接进 App 正需要它）、唯一带 C 依赖的是 `rusqlite` 的 bundled SQLite——而这是 `cargo-ndk + NDK` 的**已知良好场景**，预期能干净编过。真正要补的是 Android 壳子的三块适配（文件导入权限、TTS、PDF 存储），以及先把 NDK 装上把 POC 跑绿。

---

## 1. 环境盘点（实测）

| 组件 | 状态 | 说明 |
|------|------|------|
| Android Studio | ❌ 未装 | 可选。自带 SDK Manager GUI + 模拟器，最省心 |
| Android SDK | ✅ 已存在 | `~/Library/Android/sdk`，且 `cmdline-tools/latest` 已装 |
| Android NDK | ❌ 未装 | `~/Library/Android/sdk/ndk` 不存在——**Rust 交叉编译 C 依赖的硬前置** |
| platform-tools (adb) | ❌ 未装 | 真机调试/装包需要 |
| Android 构建目标 (rust) | ❌→✅ | 已 `rustup target add aarch64-linux-android` |
| cargo-ndk | ❌ 未装 | 脚本会在 NDK 就绪后自动 `cargo install` |
| cmake / ninja | ❌ 未装 | NDK 自带其工具链，一般不需额外装 |

### 必须去官网/SDK Manager 下载的核心包

1. **Android NDK（side-by-side，建议 27.x）**——最关键。Rust 交叉编译 SQLite C 全靠它的 `clang/ar + sysroot`。
   装完务必：`export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/<version>`
2. **Android SDK Platform + Build-Tools（API 34/35）**——Tauri/Gradle 打包 APK 需要。
   `sdkmanager "platforms;android-34" "build-tools;34.0.0"`
3. **JDK 17**——Gradle / Tauri Android 构建链需要（macOS：`brew install openjdk@17`）。
4. **platform-tools**（含 adb）——真机装包/调试：`sdkmanager "platform-tools"`。
5. **Android Studio（可选）**——想用模拟器或图形化 SDK Manager 就装；纯命令行用上面的 `sdkmanager` 即可。

> 注：本机 `cmdline-tools` 已就绪，所以第 2/4 项可直接用 `sdkmanager` 命令行装，不必先装 Studio。

---

## 2. 交叉编译 POC 结论（rusqlite / fontdue 会不会报错？）

**先纠正一个前提**：整个 workspace 的 `Cargo.toml` 里**没有任何 `fontdue` 依赖**——它既不在 core，也不在渲染层。即便有，fontdue 也是**纯 Rust 字体光栅化引擎、零 C 依赖**。所以"fontdue 带 C 依赖"这个担心**不成立**。

core 的真实 C 依赖面只有一处：
- `rusqlite = { version = "0.32.1", features = ["bundled"] }` → 编译期把 sqlite3 的 C 源码编进库。

POC 脚本 `scripts/android-poc.sh` 已写好：装好 NDK 后执行 `cargo ndk -t aarch64-linux-android build -p jiandu-core`，由 `cargo-ndk` 注入 NDK 的 `clang/ar` 与 sysroot，`rusqlite` 的 build.rs 会用 `cc` crate 调 NDK clang 把 SQLite C 编过。这是 Rust-Android 生态里**最成熟的路径之一**，大量生产应用这么干，**预期零报错**，产物为 `target/aarch64-linux-android/debug/libjiandu_core.a`。

**POC 实测结果（2026-10-30，PASS）**：装好 NDK 26.0.10792818 + cargo-ndk 4.1.2 后重跑脚本，**全程零报错**：
- `libsqlite3-sys v0.30.1`（rusqlite 的 bundled SQLite C）由 NDK 的 clang **干净编过**；
- `rusqlite v0.32.1`、`jiandu-core v1.9.7` 均编译通过；
- 产物：`target/aarch64-linux-android/debug/libjiandu_core.a`（168MB，为含 debuginfo 的 debug 构建，release 会显著变小），`file` 确认为 `current ar archive`，即标准安卓静态库。
- ⇒ **实证结论：core 交叉编译安卓零报错**。你担心的 rusqlite C 依赖在 NDK 下完全可控；fontdue 根本不是项目依赖（前提不成立）。

**结论**：core 交叉编译的技术风险**已实证为极低**——唯一变量（NDK 是否就位）已解决，不涉及任何代码改动。下一步是 Tauri Android 壳子初始化（尚未动手，按你的要求不急着建完整 App）。

---

## 3. 安卓端三大痛点评估

### 3.1 文件导入权限（如何读取手机里的 EPUB）

- **作用域存储（Scoped Storage，Android 10+）**：App 默认**不能** `std::fs::read` 任意路径。
- Tauri Android 的 dialog 插件在安卓上走**系统 Document Picker（SAF）**，返回的是 `content://` URI 或经 SAF 解析后的受限路径，**不是桌面那种裸文件系统路径**。
- **痛点 / 改造点**：core 当前 `import_from_path` 用 `std::fs::read(path)`，安卓上 content URI 不能直接这么读。两条出路：
  - **(a) 简单版**：壳子把用户选中的文件先拷进 App 私有目录（`getFilesDir` / Tauri app data dir），再把真实路径交给 core——每次导入多一次拷贝。
  - **(b) 更优（推荐）**：core 暴露 `import_from_bytes(Vec<u8>)` / 流式 `import_from_reader`，壳子用 Tauri fs/api 读出字节直接喂 core，**绕开路径问题**，且 iOS / Web 也能统一走字节入口。
- **权限**：Android 13+ 读媒体用 `READ_MEDIA_*`（细粒度），老版本用 `READ_EXTERNAL_STORAGE`；而 SAF 本身**不需要**存储权限（用户显式挑选即授权，可 `takePersistableUriPermission` 持久化）。Tauri 的 Android 权限系统会在首次需要时弹授权。
- **结论**：技术可行。**建议把 core 改造成"字节优先"导入接口**——这是安卓/iOS/Web 三端统一的刚需，不是安卓独有问题，优先级应提前。

### 3.2 TTS 语音引擎（安卓调用什么）

- 安卓系统级 TTS = **`android.speech.tts.TextToSpeech`**（系统引擎 + 各语种语音包）。
- ⚠️ **不能直接复用现有 `window.speechSynthesis`**：那是 WebView 的 Web Speech API，在安卓 Chromium WebView 里支持参差、很多设备不可用。
- **正确路线**：写一个极薄的 **Kotlin / Tauri Android 插件**封装 `TextToSpeech`，通过 Tauri 命令暴露给前端（Rust command → 调 Kotlin → 回调进度）。异步回调映射成前端现有的朗读状态机（idle/playing/paused），与 Windows 版的 `speechSynthesis` 状态机**同构**，前端几乎不改。
- **痛点（真实风险）**：
  1. **中文语音包缺失**：设备若没装中文 TTS 语音包，《静夜思》会静默或报错——POC 阶段就该在真机验证，并做"引导用户去系统设置装包"的兜底。
  2. TTS 是**异步事件驱动**，前端高亮跟随要处理回调延迟。
  3. 首次初始化有几百 ms 延迟，需 loading 态。
- **结论**：TTS 走原生 `TextToSpeech` 封装，工作量中等（一个小插件）；**中文语音包可用性**是真风险点，必须真机先验。

### 3.3 PDF 导出与存储路径（存到手机下载目录）

- **双重难题，难度天差地别**：
  - **(a) 生成 PDF（真正的硬骨头）**：core 的 PDF 导出目前是 **macOS-only**（`CGPDFContext` / Core Graphics），安卓根本没这套 API。要跨平台必须先做**纯 Rust PDF 写入器**——也就是记忆里那条未完成的 `纯 Rust PDF (Win/Linux, CJK 内嵌最难)`。**最难的是 CJK 字体子集化 + CID 字体嵌入**，否则中文变方块。这块 Windows 端也只破了"矢量文字"、完整 PDF 仍悬而未决。
  - **(b) 存到 Downloads（反而好解决）**：作用域存储下不能直接 `std::fs::write` 到 `/Download`。标准做法：① SAF 让用户挑保存位置（Tauri save dialog 在安卓走系统保存器），拿到 URI 后写；② 或用 **`MediaStore.Downloads`** 插入条目，系统落到 Downloads，**Android 10+ 无需权限**。
- **结论**：存储路径不是问题，**真正的拦路虎是"纯 Rust 跨平台 PDF + CJK 嵌入"**，建议作为**独立里程碑**评估，不要和首版安卓绑定。

---

## 4. 总体结论与下一步

**可行性：成立。** Tauri 2.0 官方支持 Android；core 纯内核 + `cargo-ndk + NDK` 路线干净，C 风险面仅 `rusqlite`（已知良好）；Windows 版积累的 dialog / fs / 权限机制在安卓上同构可复用。

**建议优先级：**
1. **装 NDK + 跑绿 POC**（本分支 `scripts/android-poc.sh`，半天级）——把"core 能编过安卓"从推测变实证。
2. **core 改造为"字节优先"导入接口**（`import_from_bytes` / reader）——顺带利好 iOS / Web，优先级最高。
3. **Tauri Android 壳子初始化**（`cargo tauri android init`）+ 真机跑通"导入 EPUB / TTS 朗读"。
4. **PDF 导出单列里程碑**（纯 Rust CJK）——独立于首版安卓。

**真实风险点（按风险排序）：**
- 🔴 纯 Rust PDF + CJK 嵌入（工程量最大）
- 🟠 安卓中文 TTS 语音包可用性（需真机先验）
- 🟡 SAF / 存储权限适配（标准套路，可控）
- 🟢 core 交叉编译（几乎无风险，仅差 NDK）

---

## 附：本分支已落地的 POC 产物

- `scripts/android-poc.sh`：NDK 闸门 + cargo-ndk 自动安装 + `cargo ndk -t aarch64-linux-android build -p jiandu-core` 的极简脚本。
- 本文件：评估与破冰报告。
- （未做：任何 UI 代码、未改 core 源码、未并回 main）
