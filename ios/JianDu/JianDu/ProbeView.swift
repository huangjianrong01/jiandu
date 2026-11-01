import SwiftUI

/// 笺读 iPadOS PoC · 探针页（最终视觉验证版）
/// 验证 uniffi 桥接 + Rust 真实排版（calculate_grid）+ iOS 模拟器/真机全链路。
///
/// 三件事：
/// ① 主线程防卡死：layoutProbe（内部即 core 的 calculate_grid）放进 Task.detached 后台线程，
///    算完再 MainActor 回主线程更新 UI。
/// ② 渲染红框：把 Rust 返回的 GlyphBoxFfi 坐标数组，用 Canvas 画在灰色底视图上——
///    每个字一个红色边框矩形（左上原点，与 SwiftUI/Pencil 同系）。
/// ③ 额外：在网格上拖手指 / Apple Pencil 画蓝线，实时读数验证坐标零 Y 翻转（对齐用）。
struct ProbeView: View {
    @State private var inputText: String = "天地玄黄宇宙洪荒日月盈昃辰宿列张"
    @State private var status: String = "输入文本后点按钮"
    @State private var boxes: [GlyphBoxFfi] = []
    /// Pencil / 触摸轨迹（屏幕坐标，SwiftUI 视图本地空间，左上原点）
    @State private var penTrail: [CGPoint] = []
    @State private var penReadout: String = "在网格上拖动手指 / Apple Pencil 验证对齐"

    // 把 Rust 页空间包围盒 -> 屏幕空间的变换参数（与下方 Canvas 共用）
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

    var body: some View {
        VStack(spacing: 16) {
            Text("笺读版本：\(jianduVersion())")   // 验证桥接通了
                .font(.headline)

            TextEditor(text: $inputText)
                .border(Color.gray)
                .frame(height: 100)

            Button("调用 Rust 排版 (layoutProbe → calculate_grid)") {
                let text = inputText   // 值捕获，避免跨 actor 引用 @State
                status = "Rust 排版中…（后台线程，主线程不卡死）"
                // ① 主线程防卡死：放进后台线程跑同步 FFI
                Task.detached(priority: .userInitiated) {
                    let result = layoutProbe(text: text, columns: 12)
                    // 算完回主线程刷新 UI
                    await MainActor.run {
                        boxes = result
                        penTrail = []
                        status = "已在后台算完，屏幕渲染 \(result.count) 个红框（Rust 真实坐标）"
                    }
                }
            }
            .padding()
            .background(Color.blue)
            .foregroundColor(.white)
            .cornerRadius(8)

            // ② 渲染红框：灰色底视图 + 红色边框矩形（覆盖在灰底上）
            GeometryReader { geo in
                let t = layoutTransform(size: geo.size)
                Canvas { ctx, size in
                    // 灰色底
                    ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(.gray.opacity(0.12)))
                    guard let t else { return }
                    for b in boxes {
                        let rx = t.offsetX + (CGFloat(b.x) - t.minX) * t.scale
                        let ry = t.offsetY + (CGFloat(b.y) - t.minY) * t.scale
                        let rw = CGFloat(b.w) * t.scale
                        let rh = CGFloat(b.h) * t.scale
                        // 红色边框
                        ctx.stroke(
                            Path(roundedRect: CGRect(x: rx, y: ry, width: rw, height: rh), cornerRadius: 1),
                            with: .color(.red),
                            lineWidth: 1
                        )
                        // 字（居中落在格内）
                        ctx.draw(
                            Text(b.ch).font(.system(size: min(rw, rh) * 0.8)),
                            at: CGPoint(x: rx + rw / 2, y: ry + rh / 2),
                            anchor: .center
                        )
                    }
                    // ③ Pencil / 触摸蓝线（同一 Canvas，同一左上原点，验证零 Y 翻转）
                    if penTrail.count > 1 {
                        var path = Path()
                        path.move(to: penTrail[0])
                        for p in penTrail.dropFirst() { path.addLine(to: p) }
                        ctx.stroke(path, with: .color(.blue), lineWidth: 2)
                    }
                }
                .border(Color.gray)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { v in
                            penTrail.append(v.location)
                            if let t {
                                let rx = (v.location.x - t.offsetX) / t.scale + t.minX
                                let ry = (v.location.y - t.offsetY) / t.scale + t.minY
                                let hit = boxes.first {
                                    rx >= CGFloat($0.x) && rx <= CGFloat($0.x + $0.w) &&
                                    ry >= CGFloat($0.y) && ry <= CGFloat($0.y + $0.h)
                                }
                                penReadout = hit.map { "Pencil→Rust 页坐标 命中「\($0.ch)」" }
                                    ?? "Pencil→Rust 页坐标 (x:\(String(format: "%.1f", rx)), y:\(String(format: "%.1f", ry)))"
                            }
                        }
                )
            }
            .frame(maxHeight: .infinity)

            Text(status).font(.caption)
            Text(penReadout).font(.caption2).foregroundColor(.gray)
        }
        .padding()
    }
}
