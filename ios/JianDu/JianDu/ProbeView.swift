import SwiftUI

/// 笺读 iPadOS PoC · 探针页
/// 验证 uniffi 桥接 + Rust 真实排版（calculate_grid）+ iOS 模拟器全链路。
///
/// 两层验证：
/// 1. 点按钮调用 layoutProbe（内部即 core 的 calculate_grid），控制台打印 GlyphBoxFfi；
///    屏幕用 Canvas 把 Rust 算出的竖排网格 + 包围盒画出来（红框 + 汉字）。
/// 2. 在网格上拖动手指 / Apple Pencil 画蓝线（红线叠加在红框上），并实时回显
///    "Pencil 落点对应的 Rust 页坐标 + 命中哪个字"——直接证明两套坐标系同系
///    （都是左上原点，y 向下），对齐时零 Y 翻转，这是当初故意保留的设计。
struct ProbeView: View {
    @State private var inputText: String = "天地玄黄宇宙洪荒日月盈昃辰宿列张"
    @State private var status: String = "输入文本后点按钮"
    @State private var boxes: [GlyphBoxFfi] = []
    /// Pencil / 触摸轨迹（屏幕坐标，SwiftUI 视图本地空间，左上原点）
    @State private var penTrail: [CGPoint] = []
    /// 落点反算回 Rust 页坐标的实时读数（验证对齐用）
    @State private var penReadout: String = "在网格上拖动手指 / Apple Pencil"

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
                let result = layoutProbe(text: inputText, columns: 12)
                boxes = result
                penTrail = []
                penReadout = "在网格上拖动手指 / Apple Pencil"
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

            // 屏幕可视化 + Pencil 红线对齐验证
            GeometryReader { geo in
                let t = layoutTransform(size: geo.size)
                Canvas { ctx, size in
                    guard let t else { return }
                    // 1) Rust 排版红框 + 汉字
                    for b in boxes {
                        let rx = t.offsetX + (CGFloat(b.x) - t.minX) * t.scale
                        let ry = t.offsetY + (CGFloat(b.y) - t.minY) * t.scale
                        let rw = CGFloat(b.w) * t.scale
                        let rh = CGFloat(b.h) * t.scale
                        ctx.stroke(
                            Path(roundedRect: CGRect(x: rx, y: ry, width: rw, height: rh), cornerRadius: 1),
                            with: .color(.red),
                            lineWidth: 0.5
                        )
                        ctx.draw(
                            Text(b.ch).font(.system(size: min(rw, rh) * 0.8)),
                            at: CGPoint(x: rx + rw / 2, y: ry + rh / 2),
                            anchor: .center
                        )
                    }
                    // 2) Pencil / 触摸蓝线（同一 Canvas，同一左上原点）
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
                            // 实时反算：屏幕点 -> Rust 页坐标，找命中的字
                            if let t {
                                let rx = (v.location.x - t.offsetX) / t.scale + t.minX
                                let ry = (v.location.y - t.offsetY) / t.scale + t.minY
                                let hit = boxes.first {
                                    rx >= CGFloat($0.x) && rx <= CGFloat($0.x + $0.w) &&
                                    ry >= CGFloat($0.y) && ry <= CGFloat($0.y + $0.h)
                                }
                                if let h = hit {
                                    penReadout = "Pencil→Rust 页坐标 (x:\(String(format: "%.1f", rx)), y:\(String(format: "%.1f", ry)))  命中「\(h.ch)」"
                                } else {
                                    penReadout = "Pencil→Rust 页坐标 (x:\(String(format: "%.1f", rx)), y:\(String(format: "%.1f", ry)))  未命中字格"
                                }
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
