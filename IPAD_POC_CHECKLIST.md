# 笺读 · iPadOS 移植验证（PoC）技术 CheckList

> 目标：用最快速度验证「纯 Rust core → iOS 静态库 → SwiftUI 调用 + PDF 复用 + Pencil 坐标对齐」三条主线的可行性。
> 方向已定：iPadOS 与 macOS 同属 Apple 生态，PDFKit/CoreGraphics 同源，本战役风险远低于 Windows 移植。
> 配套文档：`README_cross_platform.md`（总体路线）、`ANDROID_POC_REPORT.md`、`docs/print-cmyk-assessment.md`。

---

## 0. 一页纸前置结论（先读这栏）

| 项 | 现状 | 风险 |
|---|---|---|
| core 编译为 iOS 静态库 | ✅ 已 `crate-type=["lib","cdylib","staticlib"]`；纯 Rust 零平台依赖（CI 守卫 `check_core_purity.sh` 禁止 cocoa/objc/gpui） | 极低 |
| SQLite | ✅ `rusqlite` 已 `bundled`（自编译，无系统库依赖） | 无 |
| uniffi 暗雷 | ✅ workspace `[profile.release]` 无 `panic="abort"`（uniffi 异常捕获不被废） | 无 |
| PDFKit/CoreGraphics 框架同源 | ✅ iOS 11+ / CoreGraphics 全平台可用 | — |
| **PDFKit「生成」PDF** | ❌ **iOS 上 PDFKit 无绘制 surface，不能生成 PDF**（只能观看/标注） | 🔴 导出路径必须改 |
| **Rust 绑定层编译** | ❌ 现有 `app/src/pdf.rs`/`pdf_text.rs` 用 `cocoa`/`appkit`（macOS-only）封装 → iOS 上**不能 100% 编译** | 🔴 绑定层要换 `objc2`/`icrate` 或搬 Swift |
| Pencil ↔ Rust 坐标对齐 | ⚠️ 坐标系不同（Y 翻转 + scale + 原点），需统一变换 | 🟡 可构造性保证，但别漏 Y 翻转 |

**一句话**：core 这边几乎「开箱即编 iOS」；真正的工程量在「壳」——uniffi 桥接、PDF 绑定层重写、坐标对齐。PoC 约 **5–8 人日**。

---

## 1. 问题一：Rust ↔ SwiftUI 桥接（uniffi 路线）

**选型结论：用 uniffi，不用 swift-bridge。**
- uniffi（Mozilla，Firefox 移动端生产在用）proc-macro 路线最快：`#[uniffi::export]` 标函数/结构体/枚举 → 自动生成 Swift 绑定 + C ABI。
- swift-bridge 要手写 `#[swift_bridge::bridge]` glue，更细但更慢、生态更薄。我们的 core 类型清晰，uniffi 的「描述接口→自动生成」最省力。

### 最小验证步骤（7 步，半天内可出绿）
- [ ] **0. 加 iOS 目标**
  ```bash
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim
  ```
- [ ] **1. core 加 uniffi（用 `ffi` feature 隔离，避免污染 macOS/Tauri 路径）**
  - `crates/core/Cargo.toml`：`uniffi = { version = "0.25", features = ["cli"] }`（dev/build-dep），并加 `[features] ffi = ["uniffi"]`。
  - `crates/core/src/lib.rs` 末尾：`#[cfg(feature="ffi")] mod ffi;`
  - 新建 `crates/core/src/ffi.rs`：
    ```rust
    uniffi::setup_scaffolding!();
    #[uniffi::export]
    pub fn jiandu_version() -> String { env!("CARGO_PKG_VERSION").to_string() }
    #[uniffi::export]
    pub fn layout_probe(text: String) -> Vec<f64> { /* 调 calculate_grid 最小切面 */ Vec::new() }
    ```
- [ ] **2. 编 iOS 静态库（device + simulator 两套）**
  ```bash
  cargo build --release --target aarch64-apple-ios     --features ffi
  cargo build --release --target aarch64-apple-ios-sim --features ffi
  ```
- [ ] **3. 生成 Swift 绑定**
  ```bash
  cargo run --bin uniffi-bindgen generate \
    --library target/aarch64-apple-ios/release/libjiandu_core.a \
    --language swift --out-dir bindings
  ```
- [ ] **4. 打 XCFramework（必须用 xcframework，不能 fat .a）**
  > device-arm64 与 sim-arm64 **不能共存于同一个 lipo 档案**，这是 XCFramework 而非单 fat 静态库的根本原因。
  ```bash
  xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libjiandu_core.a     -headers bindings \
    -library target/aarch64-apple-ios-sim/release/libjiandu_core.a -headers bindings \
    -output JianDuCore.xcframework
  ```
- [ ] **5. Xcode 新建 SwiftUI App**，拖入 `JianDuCore.xcframework` + `JianDuCore.swift`，`ContentView` 里 `Text(jianduVersion())`。
- [ ] **6. Simulator（arm64）跑通，打印版本号 = 绿**。真机再验一次（签名用临时团队证书即可，PoC 不用 ¥688 开发者账号）。
- [ ] **7. 审计 core 依赖**：确认无 macOS-only crate（CI 守卫已堵 cocoa/objc/gpui；`zip/encoding_rs/regex/serde/pinyin/once_cell/anyhow` 均纯 Rust/跨平台，`rusqlite=bundled` 已无系统 sqlite 依赖）。

---

## 2. 问题二：底层 PDF 复用 —— 能否 100% 编译通过？

**结论：框架同源，但现有 Rust 代码不能 100% 编译通过。分两层看：**

### 2.1 Apple 框架本身（✅ 同源可用）
- `PDFDocument` / `PDFPage` / `PDFSelection` / `PDFAnnotation` / `CoreGraphics` 在 iOS 11+/iPadOS 全平台可用，类定义与 macOS 基本一致。
- 导入/文本抽取用到的 **`PDFSelection.boundsForPage:`、`characterBoundsAtIndex:`、`selectionsByLine`** 全是 Document/Support 类（非 View），iOS 100% 具备 → 我们 `pdf_text.rs` 里那套**竖排聚类几何（`walk_column`）算法可原样复用**，只换最外层的 objc 调用封装。

### 2.2 我们的 Rust 绑定层（❌ macOS-only，编译不过）
- [ ] **`cocoa` / `appkit` crate 是 macOS 专属**，iOS 上不存在 → `app/src/pdf.rs`/`pdf_text.rs` 里 `msg_send![class!(PDFDocument), ...]` 那层若基于 `cocoa` 封装，iOS 编不过。
- [ ] **改法二选一（推荐 A）**：
  - [ ] **A. 绑定的封装迁到 Swift 侧**：core 只产出「矢量绘制指令列表」或复用 `core/platform.rs` 的 `PdfExporter` trait（已有 `UnavailablePdfExporter` 兜底结构）；iOS 壳用 `PDFKit` 做 import/display、`UIGraphicsPDFRenderer`（UIKit）做 export。Rust 继续当纯算法，零 Apple 依赖。
  - [ ] **B. 留在 Rust 侧**：把 `cocoa` 换成 iOS 兼容的 `objc2`/`icrate`（支持 iOS），CoreGraphics context 用 `core-graphics` 的 iOS 可用模块。工作量更大、坑更隐蔽，**不推荐 PoC 走这条**。
- [ ] **🔴 iOS 的 PDFKit 不能「生成」PDF**（无绘制 surface）——现有 `CGPDFContext` 矢量导出逻辑在 iOS 上**无对应 API**。必须改走 `UIGraphicsPDFRenderer`（UIKit 提供 CG PDF 上下文）做一叶一页矢量落笔，或直接用 CoreGraphics C 函数 `CGPDFContextCreate`。**这是 Q2 唯一真正的重写点**，排版几何（`layout_book_from_opts` / `Piece`）一字不改。
- [ ] **验证（PoC 内）**：在 SwiftUI App 里用 `PDFKit.PDFDocument` 打开一本竖排古籍，调 `pdf_text.rs` 同款 `selectionsByLine`+`boundsForPage` 抽坐标，与 macOS 端抽出的坐标逐字比对，误差 < 1pt = 绿。

---

## 3. 问题三：Apple Pencil 坐标 ↔ Rust 高亮坐标对齐

**结论：可完美对齐，但不是自动的——需要一个「统一坐标变换」作为唯一真相源。**

### 3.1 两套坐标系差异
| 维度 | Pencil / SwiftUI | Rust 高亮（`calculate_grid`/`walk_column`） |
|---|---|---|
| 单位 | point（UIKit point，iOS 用 @2x/@3x scale 仅影响像素，逻辑坐标仍 point） | PDF 页用户坐标（72 pt/inch） |
| 原点 | 视图左上，Y **向下** | PDF 页左下，Y **向上** |
| 内容 | 屏幕视图空间（含 zoom/offset） | 纸张空间（无 zoom） |

### 3.2 对齐三要素（缺一不可）
- [ ] **zoom 比例**：当前视图每 PDF-point 对应多少屏幕 point（`pdfView.scaleFactor` 或自绘 canvas 的 zoom）。
- [ ] **页原点偏移**：页面在视图中的左上角位置。
- [ ] **🔴 Y 轴翻转**：SwiftUI/UIKit 的 Y 向下 vs PDF 的 Y 向上，漏掉这一项红线就画到镜像位置。
- [ ] （次要）PDF 页旋转 / MediaBox 原点偏移：`PDFPage.transform` 或 `rotation` 要纳入。

### 3.3 最稳做法（二选一）
- [ ] **A. 用 PDFKit 的 `pdfView.convert(_:to:page)`** 自动把视图 point 变换到 PDF 页坐标——Apple 已处理 zoom/Y 翻转/旋转，零手工公式。前提是显示层用 `PDFView`。
- [ ] **B. 自绘 canvas（我们自己的 Rust 排版渲染）**：变换由我们自己定义，**构造上就保证对齐**——Pencil 落点按同一变换反推回 PDF 空间即可。适合未来脱离 PDFKit 的纯自绘路线。

### 3.4 验证（PoC 内，决定性测试）
- [ ] 在 SwiftUI 上加载一页已排好的古籍，用 Pencil 在**某个已知 glyph cell** 上画一条红线。
- [ ] 把红线采样点经统一变换反推到 PDF 空间，与 Rust `calculate_grid` 输出的**同一 cell 坐标**比对。
- [ ] 误差 < 1pt（≈ 屏幕 1–2 px）即判定对齐通过；否则回查 Y 翻转 / scale。

---

## 4. PoC 落地顺序（单一 SwiftUI App 串起三条主线）

1. [ ] **Day 1**：按 §1 的 0–6 步，core → `JianDuCore.xcframework` → SwiftUI 打印 `jianduVersion()` 绿。
2. [ ] **Day 2**：SwiftUI 用 `PDFKit.PDFDocument` 打开古籍；`import` 路径接 core 文本抽取，验证竖排坐标（§2.2）。
3. [ ] **Day 3**：`export` 路径用 `UIGraphicsPDFRenderer` 复刻 `layout_book_from_opts` 的矢量输出（§2.2 🔴 项）。
4. [ ] **Day 4**：叠加 `DragGesture` 收 Pencil 轨迹，经 §3.3 变换反推，做 §3.4 的对齐决定性测试。
5. [ ] **Day 5**：补 `core/platform.rs` 的 `PdfExporter` iOS 实现桩 + 把 FFI 从「版本号」扩到「打开书 / 排版一页」最小业务切面。

---

## 5. Go / No-Go 与投入

- **可行性：高**。core 已 iOS-ready（staticlib 产物 + bundled sqlite + 无 panic=abort + 零平台依赖）。三条主线均有成熟路径。
- **PoC 工期**：约 **5–8 人日**（1 人；若 Rust + Swift 双人并行约 3–4 天）。
- **完整 iPad 版**（非 PoC）：在 Windows 移植（Tauri 壳 40–60 人日）之外**另算**，但复用同一份 core，边际成本低。
- **经费**：
  - PoC 阶段：**¥0 现金**（Simulator 即可验证桥接/PDF/坐标；真机用免费临时团队证书）。
  - 上架才需：**Apple 开发者账号 ¥688/年** + 一台 iPad 真机（约 ¥2k–6k，验证 Pencil 手感必备）。
  - 无第三方库授权费（uniffi MIT；PDFKit/CoreGraphics 系统自带）。

---

## 6. 风险登记（Red / Yellow）

| # | 风险 | 等级 | 缓解 |
|---|---|---|---|
| R1 | iOS PDFKit 不能生成 PDF，导出须重写 | 🔴 | 改 `UIGraphicsPDFRenderer` / CoreGraphics context，几何复用 |
| R2 | `cocoa`/`appkit` 绑定层 macOS-only | 🔴 | 迁 Swift 侧（推荐）或换 `objc2`/`icrate` |
| R3 | Pencil 坐标 Y 翻转 / scale 漏算 | 🟡 | 单一统一变换模块；§3.4 决定性测试拦截 |
| R4 | uniffi 大数据结构跨 FFI 拷贝开销 | 🟡 | 用 `Arc` 接口类型 + 流式 API，避免热循环传 `Vec` |
| R5 | core 隐式依赖某 macOS-only crate | 🟢 | CI 守卫已堵；§1 第 7 步审计兜底 |

---

_生成于 2026-11-01。配套：`README_cross_platform.md`、`ANDROID_POC_REPORT.md`。下一步：确认走 §1 的 uniffi 路线 + §2.2 的「绑定迁 Swift」策略后，我可立即动手补 `ffi.rs` 与 `JianDuCore.xcframework` 构建脚本。_
