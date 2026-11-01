import SwiftUI
import PDFKit

/// 笺校工作台（核心业务闭环 v2a）。
///
/// 左：原生 PDFKit 渲染原图（通过 holder 暴露 PDFView 实例）。
/// 右：PDF 文本层提取出的分段列表（iOS PDFKit 提取真实坐标，已翻为左上原点）。
///
/// 本步（C2）完成：文档选择 → 后台提取文本层 → 右侧分段列表展示。
/// 后续步骤叠加：C3 红色高亮叠加 + 自动居中；C4 反向联动 + Rust 重排面板。
///
/// ⚠️ 仅做 UI 与数据传递：PDF 渲染走系统 PDFKit，提取坐标走 PDFKit，
///    纯血 Rust core 不参与本步（重排预览在 C4 接入 calculate_grid）。
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

    // ── 左：原生 PDFKit 原图 ──
    private var leftPane: some View {
        Group {
            if let id = selectedDocID,
               let doc = store.documents.first(where: { $0.id == id }),
               doc.kind == .pdf {
                PDFKitView(url: store.url(for: doc), holder: holder)
                    .ignoresSafeArea(.all, edges: .bottom)
            } else {
                ContentUnavailableView("选择 PDF 以载入原图", systemImage: "doc.fill")
            }
        }
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
            // C3 在此接入高亮叠加 + 自动居中；C4 接入 Rust 重排预览。
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
}
