import SwiftUI

/// 阅读器：按文档类型分流渲染。
/// - PDF  → 原生 PDFKitView（系统 PDFKit 渲染，支持滚动/缩放/分页）
/// - PNG  → SwiftUI Image（铺满查看）
/// 数据取自 BookStore.url(for:)，core(Rust) 不参与阅读渲染。
struct ReaderView: View {
    let item: DocumentItem
    @EnvironmentObject var store: BookStore

    var body: some View {
        Group {
            if item.kind == .png {
                pngView
            } else {
                PDFKitView(url: store.url(for: item))
            }
        }
        .navigationTitle(item.name)
        .navigationBarTitleDisplayMode(.inline)
        .ignoresSafeArea(.all, edges: .bottom)
    }

    @ViewBuilder
    private var pngView: some View {
        let url = store.url(for: item)
        if let img = UIImage(contentsOfFile: url.path) {
            Image(uiImage: img)
                .resizable()
                .scaledToFit()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color.black.opacity(0.04))
        } else {
            ContentUnavailableView("无法读取图片", systemImage: "exclamationmark.triangle")
        }
    }
}
