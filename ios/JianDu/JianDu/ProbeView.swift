import SwiftUI

/// 笺读 iPadOS PoC · 探针页
/// 验证 uniffi 桥接 + Rust 真实排版（calculate_grid）+ iOS 模拟器全链路。
/// 点按钮调用 layoutProbe（内部即 core 的 calculate_grid），
/// 控制台打印每个字的 GlyphBoxFfi，并在屏幕上把竖排网格 + 包围盒画出来。
struct ProbeView: View {
    @State private var inputText: String = "天地玄黄宇宙洪荒日月盈昃辰宿列张"
    @State private var status: String = "输入文本后点按钮"
    @State private var boxes: [GlyphBoxFfi] = []

    var body: some View {
        VStack(spacing: 16) {
            Text("笺读版本：\(jianduVersion())")   // 验证桥接通了
                .font(.headline)

            TextEditor(text: $inputText)
                .border(Color.gray)
                .frame(height: 100)

            Button("调用 Rust 排版 (layoutProbe → calculate_grid)") {
                let result = layoutProbe(text: inputText, columns: 12)
                boxes = result
                print("=== GlyphBoxFfi 共 \(result.count) 个 ===")
                for b in result {
                    print("字「\(b.ch)」 x=\(b.x) y=\(b.y) w=\(b.w) h=\(b.h)")
                }
                status = "已在控制台打印 \(result.count) 个 GlyphBoxFfi"
            }
            .padding()
            .background(Color.blue)
            .foregroundColor(.white)
            .cornerRadius(8)

            // 屏幕可视化：把 Rust 算出的包围盒画成竖排网格（相对缩放，自动居中）
            GeometryReader { _ in
                Canvas { ctx, size in
                    guard !boxes.isEmpty else { return }
                    let minX = boxes.map { $0.x }.min()!
                    let minY = boxes.map { $0.y }.min()!
                    let maxX = boxes.map { $0.x + $0.w }.max()!
                    let maxY = boxes.map { $0.y + $0.h }.max()!
                    let contentW = max(maxX - minX, 1)
                    let contentH = max(maxY - minY, 1)
                    let scale = min(size.width / contentW, size.height / contentH) * 0.92
                    let offsetX = (size.width - contentW * scale) / 2
                    let offsetY = (size.height - contentH * scale) / 2
                    for b in boxes {
                        let rx = offsetX + (b.x - minX) * scale
                        let ry = offsetY + (b.y - minY) * scale
                        let rw = b.w * scale
                        let rh = b.h * scale
                        // 字格（红框）
                        ctx.stroke(
                            Path(roundedRect: CGRect(x: rx, y: ry, width: rw, height: rh), cornerRadius: 1),
                            with: .color(.red),
                            lineWidth: 0.5
                        )
                        // 字（居中落在格内）
                        ctx.draw(
                            Text(b.ch).font(.system(size: min(rw, rh) * 0.8)),
                            at: CGPoint(x: rx + rw / 2, y: ry + rh / 2),
                            anchor: .center
                        )
                    }
                }
                .border(Color.gray)
            }
            .frame(maxHeight: .infinity)

            Text(status).font(.caption)
        }
        .padding()
    }
}
