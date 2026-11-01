import SwiftUI

/// 阅读器占位（第 3 步会替换为真正的 PDFKit 渲染）。
/// 先保证书架 → 阅读器的导航链路通畅，再在下一步接入 PDF/PNG 真实渲染。
struct ReaderView: View {
    let item: DocumentItem

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: item.kind.systemImage)
                .font(.system(size: 48))
                .foregroundStyle(.secondary)
            Text(item.name)
                .font(.headline)
            Text("阅读器将在下一步接入 PDFKit 渲染")
                .font(.caption)
                .foregroundStyle(.tertiary)
        }
        .navigationTitle(item.name)
        .navigationBarTitleDisplayMode(.inline)
    }
}
