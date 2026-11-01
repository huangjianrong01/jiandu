import PDFKit
import SwiftUI
import UIKit

/// 持有 PDFView 实例与滚动标记，供父视图做坐标换算 / 滚动居中 / 反向命中。
/// - pdfView：原生 PDFView，用于 convert(_:from:) / convert(_:to:) / go(to:)。
/// - scrollNonce：每当 PDF 滚动 / 缩放时自增，驱动叠加层（高亮框）重新绘制，
///   使高亮始终贴合页面，不随滚动漂移。
/// ⚠️ 纯 SwiftUI/UIKit 桥接层，零改动 core(Rust)；core 只在「导出」与「重排」时介入。
final class PDFViewHolder: ObservableObject {
    @Published var pdfView: PDFView?
    @Published var scrollNonce: Int = 0
}

/// 把 iOS 原生 PDFKit 的 `PDFView` 包成 SwiftUI 视图。
/// 直接复用系统 PDF 渲染（滚动 / 缩放 / 分页 / 触摸），无需手搓渲染，
/// 也完全不触碰 core(Rust) —— core 只在「导出」与「重排」时介入。
/// 通过 `holder` 把底层 PDFView 暴露给父视图，是高亮叠加与自动居中的基础。
struct PDFKitView: UIViewRepresentable {
    let url: URL
    /// 默认空 holder，保证既有的 ReaderView（PDFKitView(url:)）无需改动仍可编译。
    var holder: PDFViewHolder = PDFViewHolder()
    var displayMode: PDFDisplayMode = .singlePageContinuous
    var backgroundColor: UIColor = .systemBackground

    func makeCoordinator() -> Coordinator { Coordinator(holder: holder) }

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = displayMode
        view.displayDirection = .vertical
        view.backgroundColor = backgroundColor
        view.usePageViewController(false, withViewOptions: nil)
        view.document = PDFDocument(url: url)
        // 把 PDFView 实例交给 holder，供父视图做坐标换算 / 居中 / 反向命中。
        holder.pdfView = view
        return view
    }

    func updateUIView(_ uiView: PDFView, context: Context) {
        if uiView.document?.documentURL != url {
            uiView.document = PDFDocument(url: url)
        }
        // PDFView 的 scrollView 在首次加入视图层级后才就绪，这里稳妥挂 KVO。
        // 用 KVO 监听 contentOffset（不抢占 PDFView 自身 delegate），滚动/缩放时
        // 让叠加层重绘，高亮框始终对齐页面。
        if context.coordinator.scrollView == nil,
           let sv = uiView.enclosingScrollView {
            context.coordinator.scrollView = sv
            sv.addObserver(context.coordinator,
                           forKeyPath: "contentOffset",
                           options: [], context: nil)
        }
    }

    final class Coordinator: NSObject {
        let holder: PDFViewHolder
        weak var scrollView: UIScrollView?

        init(holder: PDFViewHolder) { self.holder = holder }

        deinit {
            scrollView?.removeObserver(self, forKeyPath: "contentOffset")
        }

        override func observeValue(forKeyPath keyPath: String?,
                                   of object: Any?,
                                   change: [NSKeyValueChangeKey: Any]?,
                                   context: UnsafeMutableRawPointer?) {
            if keyPath == "contentOffset" {
                holder.scrollNonce += 1
            } else {
                super.observeValue(forKeyPath: keyPath,
                                   of: object, change: change, context: context)
            }
        }
    }
}
