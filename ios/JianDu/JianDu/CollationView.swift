import SwiftUI
import PDFKit

/// 笺校工作台（核心业务闭环）。
///
/// 左：原生 PDFKit 渲染原图（通过 holder 暴露 PDFView 实例）+ 选中分段红色高亮叠加层。
/// 右：PDF 文本层提取出的分段列表（iOS PDFKit 提取真实坐标，已翻为左上原点）。
///
/// 已完成：C2 文档选择→后台提取文本层→列表；C3 点击分段→PDF 红色高亮 + 自动居中。
/// 进行中：C4 反向联动（PDF 点击/Pencil→右侧文本）+ Rust 重排预览面板。
///
/// ⚠️ 仅做 UI 与数据传递：PDF 渲染走系统 PDFKit，提取坐标走 PDFKit，
///    纯血 Rust core 不参与提取（重排预览在 C4 接入 calculate_grid）。
struct CollationView: View {
    @EnvironmentObject var store: BookStore
    @StateObject private var holder = PDFViewHolder()

    @State private var selectedDocID: DocumentItem.ID?
    @State private var segments: [PdfSegment] = []
    @State private var selectedID: PdfSegment.ID?
    @State private var status: String = "从书架选一本 PDF，开始笺校"

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
            docPicker
            Text(status).font(.caption).foregroundStyle(.secondary)
        }
        .padding(.horizontal).padding(.vertical, 8)
    }

    private var docPicker: some View {
        Picker("书籍", selection: $selectedDocID) {
            Text("未选择").tag(DocumentItem.ID?.none)
            ForEach(store.documents) { doc in
                Text(doc.name).tag(DocumentItem.ID?.some(doc.id))
            }
        }
        .pickerStyle(.menu)
        .onChange(of: selectedDocID) { _ in loadDocument() }
    }

    // ── 左：原生 PDFKit 原图 + 高亮叠加层 ──
    private var leftPane: some View {
        Group {
            if let id = selectedDocID,
               let doc = store.documents.first(where: { $0.id == id }),
               doc.kind == .pdf {
                ZStack {
                    PDFKitView(url: store.url(for: doc), holder: holder)
                        .ignoresSafeArea(.all, edges: .bottom)
                    // 高亮叠加层：不拦截触摸（allowsHitTesting false），
                    // .id(scrollNonce) 让 PDF 滚动/缩放时重绘，高亮始终贴合页面。
                    highlightOverlay
                        .allowsHitTesting(false)
                }
            } else {
                ContentUnavailableView("选择 PDF 以载入原图", systemImage: "doc.fill")
            }
        }
    }

    /// 半透明红色高亮框：把选中分段的「左上原点页坐标」转回 PDFKit 原生
    /// 「左下原点页坐标」后，用 pdfView.convert 换到视图坐标再绘制，
    /// 与 SwiftUI/Rust 同一套左上原点屏幕空间严丝合缝。
    private var highlightOverlay: some View {
        Canvas { ctx, _ in
            guard let pv = holder.pdfView,
                  let seg = segments.first(where: { $0.id == selectedID }),
                  let page = pv.document?.page(at: seg.pageIndex) else { return }
            let rYUp = CGRect(x: seg.rect.origin.x,
                              y: seg.pageHeight - seg.rect.origin.y - seg.rect.height,
                              width: seg.rect.width, height: seg.rect.height)
            let vRect = pv.convert(rYUp, from: page)
            ctx.fill(Path(vRect), with: .color(.red.opacity(0.28)))
            ctx.stroke(Path(vRect), with: .color(.red), lineWidth: 2)
        }
        .id(holder.scrollNonce)
    }

    // ── 右：文本层分段列表 ──
    private var rightPane: some View {
        List(selection: $selectedID) {
            ForEach(segments) { seg in
                Text(seg.text)
                    .font(.system(size: 14))
                    .tag(seg.id as PdfSegment.ID?)
            }
        }
        .overlay {
            if segments.isEmpty {
                Text("暂无文本片段").foregroundStyle(.tertiary)
            }
        }
        .onChange(of: selectedID) { _ in
            // C3：选中分段 → 高亮叠加（由 selectedID 驱动 Canvas）+ 自动滚动居中；
            // C4 将在此接入 Rust 重排预览。
            centerOnSelected()
        }
    }

    // ── 后台提取 PDF 文本层（不阻塞主线程）──
    private func loadDocument() {
        guard let id = selectedDocID,
              let doc = store.documents.first(where: { $0.id == id }),
              doc.kind == .pdf else {
            segments = []
            selectedID = nil
            status = "请选择 PDF 文档"
            return
        }
        let url = store.url(for: doc)
        status = "提取 PDF 文本层…"
        Task.detached(priority: .userInitiated) {
            let segs = PdfTextExtractor.extract(from: url)
            await MainActor.run {
                segments = segs
                selectedID = segs.first?.id
                status = "已提取 \(segs.count) 个文本片段"
            }
        }
    }

    // ── 选中分段 → 自动滚动居中（PDFView.go(to: PDFDestination)）──
    private func centerOnSelected() {
        guard let pv = holder.pdfView,
              let seg = segments.first(where: { $0.id == selectedID }),
              let page = pv.document?.page(at: seg.pageIndex) else { return }
        // 左上原点页坐标 → PDFKit 左下原点页坐标 → 居中目标点。
        let rYUp = CGRect(x: seg.rect.origin.x,
                          y: seg.pageHeight - seg.rect.origin.y - seg.rect.height,
                          width: seg.rect.width, height: seg.rect.height)
        let dest = PDFDestination(page: page,
                                  at: CGPoint(x: rYUp.midX, y: rYUp.midY))
        pv.go(to: dest)
        pv.layoutDocumentView()
    }
}
