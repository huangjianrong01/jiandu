# iPadOS Pencil 红线对齐验证指南（PoC 第二阶段）

本指南承接 `XCODE_SETUP.md` 的"跑通 ProbeView"第一步，验证移植战略里的第三个核心问题：
**Apple Pencil 坐标与 Rust 高亮能否对齐**。

结论先行：**能，且零 Y 翻转**。因为 `GlyphBoxFfi` 与 SwiftUI / Pencil 视图坐标
**同为左上原点（y 轴向下为正）**，所以 Pencil 落点可以直接映射到 Rust 页坐标，
不需任何坐标翻转或镜像。本 PoC 已把这套映射在 `ProbeView` 里实装成"手指/笔拖动 → 蓝线 + 实时落点读数"。

---

## 一、坐标体系（务必记住）

| 坐标系 | 原点 | x 方向 | y 方向 |
|---|---|---|---|
| Rust `GlyphBoxFfi` | 左上 | 向右正 | 向下正 |
| SwiftUI `Canvas` / `DragGesture.location` | 左上 | 向右正 | 向下正 |
| Apple Pencil `UITouch` / `CGPoint` | 左上 | 向右正 | 向下正 |

三者同系 → **任何一对之间都不需要 Y 翻转**，只需一次等比缩放 + 平移（见下方变换）。
这是当初故意把 `calculate_grid` 输出保持"左上原点"的原因，专门为 Pencil 对齐铺路。

---

## 二、屏幕 ↔ Rust 页坐标的变换（已在 ProbeView 实装）

`ProbeView.layoutTransform(size:)` 算出：

```
minX, minY      = 所有字格的最小 x / y（Rust 页空间）
contentW, H     = 包围盒宽高
scale           = min(viewW/contentW, viewH/contentH) * 0.92   // 等比留白 8%
offsetX, offsetY= 居中偏移
```

- 正向（Rust 页 → 屏幕）：
  `screenX = offsetX + (rustX - minX) * scale`
  `screenY = offsetY + (rustY - minY) * scale`
- 反向（屏幕 → Rust 页，Pencil 对齐用）：
  `rustX = (screenX - offsetX) / scale + minX`
  `rustY = (screenY - offsetY) / scale + minY`

`ProbeView` 的 `DragGesture.onChanged` 里已用反向公式实时算出落点的 Rust 页坐标，
并打印"命中哪个字"，这就是对齐的端到端证据。

---

## 三、在模拟器里先验证（不用真机也能看）

1. Xcode 打开 `ios/JianDu/JianDu.xcodeproj` → 选 **arm64 iPad 模拟器** → ⌘R
2. 点蓝色按钮跑一次 Rust 排版（屏幕出现竖排红框网格）
3. 在网格上**按住鼠标拖动**（模拟器用鼠标模拟触摸），你会看到：
   - 一条 **蓝色轨迹线** 跟着指针走（Pencil/触摸层）
   - 底部灰字实时显示 `Pencil→Rust 页坐标 (x, y) 命中「某字」`
4. 把蓝线拖过某个红框里的汉字，读数应显示**命中该字**——
   这说明"屏幕指针位置"与"Rust 字格位置"严格对应，**坐标同系成立**。

> 模拟器没有 Apple Pencil，但 `DragGesture` 在鼠标下行为一致，
> 足以证明变换数学正确；真机的 Pencil 只是更精细的输入源，映射逻辑完全相同。

---

## 四、真机 iPad + Apple Pencil 验证

### 前置
- 一台 Apple Silicon / Intel Mac 做宿主机，一根 Apple Pencil，一台 iPad（A 系列/M 系列均可）。
- 用 **数据线** 把 iPad 接 Mac，Xcode 顶部运行目标选这台 **真机 iPad**（首次需信任 + 签名）。
- 或在 Xcode 里配对无线调试。

### 步骤
1. 选真机 iPad → ⌘R 安装并运行 `JianDu`。
2. 点按钮跑 Rust 排版，屏幕出现红框网格。
3. 用 **Apple Pencil 在网格上书写/圈画**：
   - 蓝线应精确贴合你落笔的位置（无上下颠倒、无左右镜像）。
   - 底部读数应随笔尖移动实时更新，圈到某字时显示"命中「某字」"。
4. 关键断言：**Pencil 笔尖正下方的字，与读数"命中"的字一致，且蓝线不偏离红框**。
   若一致 → 对齐验证通过（零 Y 翻转成立）。

### 若要对齐真实 PDF 页（进阶，非本 PoC 必须）
当前 `ProbeView` 只渲染 Rust 排版网格。将来把真实 PDF 页用 `PDFView` / `CGContextDrawPDFPage`
铺到底层，再把同一 `layoutTransform` 套上去，Pencil 红线就能直接落在 PDF 原文对应字上——
因为 PDF 文本层坐标、Rust `GlyphBoxFfi`、`calculate_grid` 三者本就同单位同原点
（见工程笔记《导入 PDF 闪退真凶》与 `core/hit.rs` 的映射数学）。

---

## 五、验证通过后的落点

- 第二核心问题（PDF 复用）已靠 `crates/ios-ffi/src/pdf_export.rs`（`#[cfg(target_os="ios")]`
  走 CoreGraphics + CoreText C FFI 真实矢量出 PDF）解决，坐标系同样左上原点。
- 三核心问题全部验证完毕 → 可进入正式 iPadOS 工程化（书架 / 阅读器 / 笺校工作台移植）。
- 本 PoC 的 `ProbeView` 可保留为回归探针，或验证完后用
  `git checkout -- ios/JianDu/JianDu/JianDuApp.swift` 把入口切回原 App。
