import SwiftUI

/// PoC 探针页：验证 uniffi 桥接 + Rust 真实排版（calculate_grid）+ iOS 模拟器全链路。
/// 点击按钮调用 layoutProbe（内部即 core 的 calculate_grid），在控制台打印 GlyphBoxFfi。
struct ProbeView: View {
    @State private var inputText: String = "天地玄黄宇宙洪荒日月盈昃辰宿列张"
    @State private var status: String = "输入文本后点按钮"

    var body: some View {
        VStack(spacing: 16) {
            Text("笺读版本：\(jianduVersion())")   // 验证桥接通了
                .font(.headline)

            TextEditor(text: $inputText)
                .border(Color.gray)
                .frame(height: 120)

            Button("调用 Rust 排版 (layoutProbe → calculate_grid)") {
                // Swift 这边没有直接暴露 calculate_grid；
                // layoutProbe 内部就是调它，返回每字的页空间包围盒。
                let boxes = layoutProbe(text: inputText, columns: 12)
                print("=== GlyphBoxFfi 共 \(boxes.count) 个 ===")
                for b in boxes {
                    print("字「\(b.ch)」 x=\(b.x) y=\(b.y) w=\(b.w) h=\(b.h)")
                }
                status = "已在控制台打印 \(boxes.count) 个 GlyphBoxFfi"
            }
            .padding()
            .background(Color.blue)
            .foregroundColor(.white)
            .cornerRadius(8)

            Text(status).font(.caption)
        }
        .padding()
    }
}
