import PDFKit
import SwiftUI

/// 把 iOS 原生 PDFKit 的 `PDFView` 包成 SwiftUI 视图。
/// 直接复用系统 PDF 渲染能力（滚动 / 缩放 / 分页），无需手搓渲染，
/// 也完全不触碰 core(Rust) —— core 只在「导出 PDF」(exportLayoutToPdf) 时介入。
struct PDFKitView: UIViewRepresentable {
    let url: URL
    var displayMode: PDFDisplayMode = .singlePageContinuous
    var backgroundColor: UIColor = .systemBackground

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = displayMode
        view.displayDirection = .vertical
        view.backgroundColor = backgroundColor
        view.usePageViewController(false, withViewOptions: nil)
        view.document = PDFDocument(url: url)
        return view
    }

    func updateUIView(_ uiView: PDFView, context: Context) {
        // 列表/导航重用 cell 时，若 URL 变了就重载文档。
        if uiView.document?.documentURL != url {
            uiView.document = PDFDocument(url: url)
        }
    }
}
