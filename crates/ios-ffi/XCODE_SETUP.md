# 笺读 · iOS Xcode 接入指南

> **状态：工程 `ios/JianDu` 已预接线完成**（xcframework + Swift 绑定 + 系统框架都已写进 `project.pbxproj`）。
> 你**不需要再拖任何文件**，直接打开 Xcode → 选 iPad 模拟器 → ⌘R 即可。
> 下方 §A 是「贴脸」运行步骤；§B 是接线细节（备查）；§C 是之前写的从零拖拽教程（已废弃，留作参考）。

---

## §A. 你只需要做这几步（傻瓜式）

1. **打开工程**：双击 `ios/JianDu/JianDu.xcodeproj`。
2. **选运行目标**：顶部运行目标栏 → 选一个 **iPad 模拟器**（如 *iPad Pro 13-inch (M5)* 或任意 arm64 iPad）。
   - ⚠️ 我们的 `JianDuCore.xcframework` 只含 **arm64 切片**（真机 device + arm64 模拟器），**Apple Silicon Mac 的 iPad 模拟器就是 arm64，没问题**；
     老 Intel Mac 的 x86_64 模拟器会报「missing architecture」——那种机器请用真机。
3. **签名**：首次若弹 Signing，去 **Signing & Capabilities → Team** 选/加你的 Apple ID（免费即可跑模拟器）。
4. **运行**：⌘R。
5. **看结果**：App 起来后，文本框里改几个字 → 点蓝色按钮 **「调用 Rust 排版」** → 打开控制台（⌘⇧C），应看到：

```
笺读版本：1.9.25
=== GlyphBoxFfi 共 N 个 ===
字「天」 x=… y=… w=… h=…
字「地」 x=… y=… w=… h=…
…
```

看到这串 = **uniffi 桥接 + Rust 真实排版（calculate_grid）+ iOS 构建全链路打通** 🎉

---

## §B. 接线细节（已自动完成，备查）

`project.pbxproj` 里已加好：

| 项 | 说明 |
|---|---|
| `JianDuCore.xcframework` | 已加进 **Frameworks** 阶段 + **Embed & Sign**（路径相对仓库根 `../../../crates/ios-ffi/...`） |
| `jiandu_ios_ffi.swift` | 已拷进 `ios/JianDu/JianDu/` 并加入 **Compile Sources**（纯 Swift 绑定，不参与宏） |
| `CoreGraphics / CoreText / CoreFoundation.framework` | **系统框架，已加进 Frameworks 阶段，Do Not Embed**——这是 iOS PDF 导出（`pdf_export.rs` 用 CG/CT/CF 的 C FFI）链接所必须，缺了会 `Undefined symbols` |
| `ProbeView.swift` | 已新建的探针页（TextEditor + Button 调 `layoutProbe`） |
| `JianDuApp.swift` 的 `root` | 已临时切到 `ProbeView()` 以便直接验证 |

> 还原原 App 入口：`git checkout -- ios/JianDu/JianDu/JianDuApp.swift`

### 暴露的 API（都在 `jiandu_ios_ffi.swift`）

- `jianduVersion() -> String`
- `layoutProbe(text:columns:) -> [GlyphBoxFfi]`（内部即调 core 的 `calculate_grid`）
- `GlyphBoxFfi { ch:String, x:Double, y:Double, w:Double, h:Double }`（页空间包围盒，左上原点，与 Pencil 对齐同系）
- `exportLayoutToPdf(outPath:text:columns:) throws`（iOS 专属，走 CoreGraphics/CoreText 矢量导出）

---

## §C. 从零拖拽教程（旧，已废弃，留底）

<details>
<summary>点击展开：如果你想在「全新工程」里手动接线（当前 ios/JianDu 已自动完成，不必看）</summary>

1. 新建 iOS → App → SwiftUI 工程。
2. 把 `crates/ios-ffi/JianDuCore.xcframework` 拖进工程 → **Copy items + Add to target** → 在 **Frameworks, Libraries, and Embedded Content** 设为 **Embed & Sign**。
3. 把 `crates/ios-ffi/bindings/jiandu_ios_ffi.swift` 拖进工程源码组 → **Copy + Add to target**（只加这一个 .swift，`.h`/`module.modulemap` 已在 xcframework 内，别单独加）。
4. 在 **Build Phases → Link Binary With Libraries** 额外加 **CoreGraphics.framework / CoreText.framework / CoreFoundation.framework**（Do Not Embed）。
5. 把 §A 的 `ProbeView` 代码粘进工程运行。

</details>

---

## 已知限制 / 下一步

- **Swift 宏**：你原有的 `ShelfView.swift`/`ReaderView.swift` 用了 `@State` 等 Swift 宏；在**正常 Xcode GUI 里编译没问题**。注意本仓库在「无头 `xcodebuild`」环境里偶发 `swift-plugin-server produced malformed response`（Xcode 27 已知问题，与本工程无关，GUI 不受影响）。
- 把 `GlyphBoxFfi` 的 `x/y/w/h` 画到 Canvas（竖排网格渲染，见 `README.md` §2）。
- Apple Pencil 红线对齐测试（见 `README.md` §3，需真机 iPad）。
- 接 PDF 导入（PDFKit 同源，复用 `pdf_text.rs` 的 `walk_column` 几何）。

重新生成产物（若改了 Rust）：在 `crates/ios-ffi/` 跑 `bash build-ios.sh`，再回 Xcode ⌘R。
