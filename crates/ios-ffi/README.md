# 笺读 · iOS / iPadOS 集成指南（PoC 阶段）

本目录产出已可直接喂给 Xcode：

- `JianDuCore.xcframework` —— 真静态库，含 `ios-arm64`（真机）+ `ios-arm64-simulator`（模拟器）两套切片。
- `bindings/jiandu_ios_ffi.swift` —— uniffi 生成的 Swift 绑定（拖进工程当源码）。
- `bindings/module.modulemap` —— C 桥接头（xcframework 已自带，无需单独处理）。

> core 零改动，这些全是 `crates/ios-ffi` 的产物；macOS 端构建与 905 项测试不受影响。

---

## 1. 把 xcframework 加进 Xcode 工程

1. 新建一个 **App**（Interface: SwiftUI，Language: Swift）。
2. 把 `JianDuCore.xcframework` 拖进工程导航器：
   - 勾选 **Add to target**；
   - 在 Target → **Frameworks, Libraries, and Embedded Content** 里把它设为 **Embed & Sign**。
3. 把 `bindings/jiandu_ios_ffi.swift` 也拖进工程（作为普通 Swift 源文件，不要嵌到 Frameworks）。
4. 签名：模拟器用免费「个人团队」即可；真机需要付费 Apple Developer（¥688/年）。

> 若 Xcode 报 `module.modulemap not found`，确认 xcframework 内 `Headers/` 目录含 `module.modulemap`，且 Target → Build Settings → **Framework Search Paths** 指向 xcframework 所在目录。

---

## 2. 最小 SwiftUI App：调用版号 + 渲染竖排网格

`layoutProbe(text:columns:)` 返回的 `GlyphBoxFfi` 坐标**原点=纸面左上、Y 向下**，与 SwiftUI 坐标系同系——所以画出来不用翻 Y，直接当 `CGRect` 用。

```swift
import SwiftUI
// jiandu_ios_ffi.swift 已把下列顶层函数/类型加入工程：
//   jianduVersion() -> String
//   layoutProbe(text:columns:) -> [GlyphBoxFfi]   // GlyphBoxFfi: ch, x, y, w, h
//   exportLayoutToPdf(outPath:text:columns:) throws
//   enum PdfError

struct ContentView: View {
    let sample = "天地玄黄宇宙洪荒日月盈昃辰宿列张"
    @State private var boxes: [GlyphBoxFfi] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("笺读版本：\(jianduVersion())")
                .font(.headline)

            // 竖排网格预览（右上起、逐列向左）
            GeometryReader { geo in
                Canvas { ctx, size in
                    // 纸面白底
                    ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(.white))
                    // 用 layoutProbe 的真实坐标画每格 + 单字
                    for b in boxes {
                        let rect = CGRect(x: b.x, y: b.y, width: b.w, height: b.h)
                        ctx.stroke(Path(rect), with: .color(.gray), lineWidth: 0.5)
                        // 字居中（SwiftUI 文字原点在左下，这里简单偏移）
                        let p = CGPoint(x: b.x + b.w/2, y: b.y + b.h/2)
                        ctx.draw(Text(b.ch).font(.system(size: min(b.w, b.h) * 0.8)),
                                 at: p, anchor: .center)
                    }
                }
                .onAppear {
                    boxes = layoutProbe(text: sample, columns: 12)
                }
            }
            .frame(width: 360, height: 480)
            .border(Color.black.opacity(0.2))
        }
        .padding()
    }
}
```

跑模拟器，应看到一页竖排网格（右起顶格、列向向左），证明 **uniffi 桥接 + core 真实排版** 在 iOS 上通了。

---

## 3. Apple Pencil 坐标对齐（决定性测试，对应 CheckList §3.4）

设计要点：我们的 FFI 坐标与 SwiftUI/UIKit **同系（左上原点、Y 向下）**，而 PDF 内部空间是 **Y 向上**。因此：

- **Pencil 红线 vs `layoutProbe` 格子**：直接比对（同系，零翻转歧义）。
- **Pencil 红线 ↔ PDF 空间**：经 `pdfView.convert(_:to:)` 交给 PDFKit 自动变换，或从 FFI 坐标反推。

下面的示例演示「在已知字格上画红线 → 反推命中哪个字 → 误差应 < 1pt」：

```swift
import SwiftUI
import PencilKit   // 若用 PKCanvasView；或直接用 DragGesture 取坐标

extension CGPoint {
    /// 命中 layoutProbe 返回的哪个字格（左上原点、Y 向下，同系直比）。
    func hitGlyph(in boxes: [GlyphBoxFfi]) -> Int? {
        for (i, b) in boxes.enumerated() {
            let rect = CGRect(x: b.x, y: b.y, width: b.w, height: b.h)
            if rect.contains(self) { return i }
        }
        return nil
    }
}

// 在 Canvas / PKCanvasView 的 .onChange(of: canvasView.drawing) 里：
func validatePencil(redDot: CGPoint, boxes: [GlyphBoxFfi]) -> String {
    guard let idx = redDot.hitGlyph(in: boxes) else { return "未命中任何字格" }
    let b = boxes[idx]
    // 与字格中心的距离，应远小于一格宽高（PoC 验收：< 1pt 视为完美对齐）
    let center = CGPoint(x: b.x + b.w/2, y: b.y + b.h/2)
    let dx = redDot.x - center.x, dy = redDot.y - center.y
    let dist = sqrt(dx*dx + dy*dy)
    return "命中第 \(idx) 字「\(b.ch)」，偏离中心 \(dist)pt（<1pt 即完美）"
}
```

**完整 PDF 空间往返（若已加载真 PDF 到 `PDFView`）：**

```swift
import PDFKit
// Pencil 落在 PDFView 的视图坐标 point：
if let page = pdfView.page(for: point, nearest: true) {
    let pdfPoint = pdfView.convert(point, to: page) // Apple 自动做 Y 翻转/缩放
    // pdfPoint 即 PDF 72dpi 空间坐标，可直接喂给 Rust 高亮存盘逻辑
}
```

> 验收标准（CheckList R4）：Pencil 在 `layoutProbe` 某字格中心画红线 → `hitGlyph` 应返回该字索引，且 `dist < 1pt`。若偏差大，几乎一定是缩放因子（PDF 72dpi vs 屏幕 point）或 `convert` 漏调用，而非坐标系设计问题。

---

## 4. 调用 PDF 导出

```swift
// outPath 需是 App 沙盒内可写路径（如 FileManager.temporaryDirectory）
let url = FileManager.default.temporaryDirectory
    .appendingPathComponent("jiandu_export.pdf").path
do {
    try exportLayoutToPdf(outPath: url, text: sample, columns: 12)
    print("已导出：\(url)")
} catch {
    print("导出失败：\(error)")  // 非 iOS target 会抛 PdfError.unsupportedPlatform
}
```

`exportLayoutToPdf` 在 iOS 上走 `crates/ios-ffi/src/pdf_export.rs` 的 raw CoreGraphics + CoreText 真矢量生成；在 Mac/模拟器 host 校验构建上返回 `UnsupportedPlatform`（保证 Swift 绑定签名一致）。

---

## 5. 已知限制 / 下一步

- ❌ 尚未接 **PDF 导入**（PDFKit 抽取文本层 / 栅格化给 OCR）——iOS 上 PDFKit 同源可用，需在 `crates/ios-ffi` 再补一个 `#[cfg(target_os="ios")]` 模块（参照 macOS `pdf_text.rs` 的 `walk_column` 几何，逐字可复用）。
- ❌ 尚未在**真机**跑 Pencil 事件（模拟器无 Pencil，需 iPad 真机 + 个人/付费签名）。
- ❌ 未做 SwiftUI 完整阅读/校对 UI（竖排、双行夹注、注音、卡片匣）——这是 iPad 版的「Tauri 壳」那一层工作量。
- ✅ 已验证：uniffi 桥接、core 真实排版、iOS 双 target 编译、xcframework 打包、Swift 绑定生成。

重新生成产物：`./build-ios.sh`（自动加 target、双编、生成绑定、打 xcframework）。
