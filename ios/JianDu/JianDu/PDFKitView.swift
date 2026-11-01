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
    /// 反向联动：PDFView 上的点击 / Apple Pencil 落点（PDFView 本地坐标），
    /// 由父视图设置，用于把触摸坐标换回页坐标、反向选中右侧文本。
    /// 用 UIKit 手势（cancelsTouchesInView=false）挂载，绝不抢占 PDFKit 自身的缩放/平移。
    var onTap: ((CGPoint) -> Void)?
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
        // 反向联动：挂一个「不拦截」的单击手势到 PDFView 自身，
        // cancelsTouchesInView=false 保证双指缩放 / 单指平移等原生手势照常工作，
        // 仅把落点（PDFView 本地坐标）回调给父视图做反向命中。
        let tap = UITapGestureRecognizer(target: context.coordinator,
                                         action: #selector(Coordinator.handleTap(_:)))
        tap.cancelsTouchesInView = false
        view.addGestureRecognizer(tap)
        return view
    }

    func updateUIView(_ uiView: PDFView, context: Context) {
        if uiView.document?.documentURL != url {
            uiView.document = PDFDocument(url: url)
        }
        // iOS 17 之前 PDFView 包裹 UIScrollView，可经 enclosingScrollView 监听
        // contentOffset；但 iOS 26/27 SDK 已移除 enclosingScrollView（PDFView 重构，
        // 不再包裹 UIScrollView）。改为监听 PDFView 自身的 bounds 与 scaleFactor：
        // 滚动使 bounds.origin 变化、缩放使 bounds.size 与 scaleFactor 变化，KVO
        // 即驱动高亮层重绘，且绝不抢占 PDFView 自身 delegate。
        if context.coordinator.observedView == nil {
            context.coordinator.observedView = uiView
            uiView.addObserver(context.coordinator,
                               forKeyPath: "bounds",
                               options: [], context: nil)
            uiView.addObserver(context.coordinator,
                               forKeyPath: "scaleFactor",
                               options: [], context: nil)
        }
    }

    final class Coordinator: NSObject {
        let holder: PDFViewHolder
        weak var observedView: PDFView?

        init(holder: PDFViewHolder) { self.holder = holder }

        deinit {
            observedView?.removeObserver(self, forKeyPath: "bounds")
            observedView?.removeObserver(self, forKeyPath: "scaleFactor")
        }

        /// 反向联动：落点已是 PDFView 本地坐标，直接回调父视图做页坐标换换算 + 命中。
        @objc func handleTap(_ recognizer: UITapGestureRecognizer) {
            let loc = recognizer.location(in: recognizer.view)
            holder.onTap?(loc)
        }

        override func observeValue(forKeyPath keyPath: String?,
                                   of object: Any?,
                                   change: [NSKeyValueChangeKey: Any]?,
                                   context: UnsafeMutableRawPointer?) {
            if keyPath == "bounds" || keyPath == "scaleFactor" {
                holder.scrollNonce += 1
            } else {
                super.observeValue(forKeyPath: keyPath,
                                   of: object, change: change, context: context)
            }
        }
    }
}
