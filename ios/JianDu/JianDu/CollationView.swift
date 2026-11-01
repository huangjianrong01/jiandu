import SwiftUI

/// 笺校工作台骨架：左右分屏。
///
/// 左：文本输入 + 列数调节 + 书籍来源选择（演示数据从 BookStore 传递到工作台）
///      + 「运行 Rust 排版」按钮（内部即 core.calculate_grid，经 FFI 的 layoutProbe）。
/// 右：Canvas 把 Rust 返回的 GlyphBoxFfi 渲染成红色字格竖排网格（左上原点，
///     与 SwiftUI / Apple Pencil 同系），并支持手指 / Pencil 拖动读数，为后续
///     「高亮联动」预留坐标落点。
///
/// ⚠️ 仅做 UI 与数据传递；core(Rust) 逻辑零改动，layoutProbe 即 core.calculate_grid。
struct CollationView: View {
    @EnvironmentObject var store: BookStore

    @State private var inputText: String = "天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏"
    @State private var columns: UInt32 = 12
    @State private var boxes: [GlyphBoxFfi] = []
    @State private var status: String = "点「运行 Rust 排版」生成竖排版式"
    /// Pencil / 触摸轨迹（右 pane 本地视图坐标，左上原点）
    @State private var penTrail: [CGPoint] = []
    @State private var penReadout: String = "在右侧网格上拖动手指 / Apple Pencil 验证对齐（零 Y 翻转）"
    /// 来源选择（数据传递演示；书籍原文提取为后续步骤）
    @State private var selectedDocID: DocumentItem.ID?

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            HStack(spacing: 0) {
                leftPane.frame(maxWidth: .infinity)
                Divider()
                rightPane.frame(maxWidth: .infinity)
            }
        }
    }

    private var header: some View {
        HStack {
            Text("笺校工作台").font(.headline)
            Spacer()
            Text("core.calculate_grid · 红框=字格").font(.caption).foregroundStyle(.secondary)
        }
        .padding(.horizontal).padding(.vertical, 8)
    }

    // ── 左：输入与控制 ──
    private var leftPane: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !store.documents.isEmpty {
                Picker("来源", selection: $selectedDocID) {
                    Text("手动输入").tag(DocumentItem.ID?.none)
                    ForEach(store.documents) { doc in
                        Text(doc.name).tag(DocumentItem.ID?.some(doc.id))
                    }
                }
                .pickerStyle(.menu)
                if selectedDocID != nil {
                    Text("（书籍原文提取待接入；下方仍可手动排版验证）")
                        .font(.caption2).foregroundStyle(.tertiary)
                }
            }

            Text("排版文本").font(.subheadline).foregroundStyle(.secondary)
            TextEditor(text: $inputText)
                .border(Color.gray.opacity(0.4), width: 1)
                .frame(minHeight: 120)

            Stepper("列数：\(columns)", value: $columns, in: 4...20)

            Button("运行 Rust 排版 (calculate_grid)") {
                runLayout()
            }
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity)
            .background(Color.blue)
            .foregroundColor(.white)
            .cornerRadius(8)

            Text(status).font(.caption)
            Text(penReadout).font(.caption2).foregroundStyle(.gray)
            Spacer()
        }
        .padding()
    }

    // ── 右：版式渲染 + Pencil 落点 ──
    private var rightPane: some View {
        GeometryReader { geo in
            let t = layoutTransform(size: geo.size)
            Canvas { ctx, size in
                // 灰底
                ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(.gray.opacity(0.12)))
                guard let t else { return }
                for b in boxes {
                    let rx = t.offsetX + (CGFloat(b.x) - t.minX) * t.scale
                    let ry = t.offsetY + (CGFloat(b.y) - t.minY) * t.scale
                    let rw = CGFloat(b.w) * t.scale
                    let rh = CGFloat(b.h) * t.scale
                    ctx.stroke(
                        Path(roundedRect: CGRect(x: rx, y: ry, width: rw, height: rh), cornerRadius: 1),
                        with: .color(.red), lineWidth: 1
                    )
                    ctx.draw(
                        Text(b.ch).font(.system(size: min(rw, rh) * 0.8)),
                        at: CGPoint(x: rx + rw / 2, y: ry + rh / 2),
                        anchor: .center
                    )
                }
                // Pencil / 触摸蓝线（同一 Canvas、同一左上原点 → 验证未来高亮联动零 Y 翻转）
                if penTrail.count > 1 {
                    var path = Path()
                    path.move(to: penTrail[0])
                    for p in penTrail.dropFirst() { path.addLine(to: p) }
                    ctx.stroke(path, with: .color(.blue), lineWidth: 2)
                }
            }
            .border(Color.gray.opacity(0.4))
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0).onChanged { v in
                    penTrail.append(v.location)
                    if let t {
                        let rx = (v.location.x - t.offsetX) / t.scale + t.minX
                        let ry = (v.location.y - t.offsetY) / t.scale + t.minY
                        let hit = boxes.first {
                            rx >= CGFloat($0.x) && rx <= CGFloat($0.x + $0.w) &&
                            ry >= CGFloat($0.y) && ry <= CGFloat($0.y + $0.h)
                        }
                        penReadout = hit.map { "Pencil→命中「\($0.ch)」" }
                            ?? "Pencil→页坐标 (x:\(String(format: "%.1f", rx)), y:\(String(format: "%.1f", ry)))"
                    }
                }
            )
        }
        .padding()
    }

    // ── 调用 Rust 引擎（后台线程，主线程不卡死）──
    private func runLayout() {
        let text = inputText
        let cols = columns
        status = "Rust 排版中…（后台线程）"
        Task.detached(priority: .userInitiated) {
            let result = layoutProbe(text: text, columns: cols)
            await MainActor.run {
                boxes = result
                penTrail = []
                status = "已渲染 \(result.count) 个字格（Rust 真实坐标）"
            }
        }
    }

    // 把 Rust 页空间包围盒 → 屏幕空间的等比缩放 + 居中参数（与右 pane Canvas 共用）
    private func layoutTransform(size: CGSize) -> (minX: CGFloat, minY: CGFloat, scale: CGFloat, offsetX: CGFloat, offsetY: CGFloat)? {
        guard !boxes.isEmpty else { return nil }
        let minX = boxes.map { CGFloat($0.x) }.min()!
        let minY = boxes.map { CGFloat($0.y) }.min()!
        let maxX = boxes.map { CGFloat($0.x + $0.w) }.max()!
        let maxY = boxes.map { CGFloat($0.y + $0.h) }.max()!
        let contentW = max(maxX - minX, 1)
        let contentH = max(maxY - minY, 1)
        let scale = min(size.width / contentW, size.height / contentH) * 0.92
        let offsetX = (size.width - contentW * scale) / 2
        let offsetY = (size.height - contentH * scale) / 2
        return (minX, minY, scale, offsetX, offsetY)
    }
}
