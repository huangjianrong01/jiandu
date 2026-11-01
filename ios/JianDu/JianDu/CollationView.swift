import SwiftUI
import PDFKit
import UIKit

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
    /// Rust core（calculate_grid / layoutProbe）对选中行文本的重排结果，
    /// 用于右侧「笺读 Rust 重排」预览——保证 Mac/iPad 排版与坐标 100% 一致。
    @State private var rustBoxes: [GlyphBoxFfi] = []
    @State private var status: String = "从书架选一本 PDF，开始笺校"
    /// 整本原文（逐页 page.string 提取，带页码分隔），自动填入右侧编辑区，可直接校改。
    @State private var sourceText: String = ""
    /// 导出笺读 PDF 的落盘 URL（成功后弹出系统分享面板用）。
    @State private var shareURL: URL?
    @State private var showShare: Bool = false

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
        .sheet(isPresented: $showShare) {
            if let url = shareURL {
                ShareSheet(items: [url])
            }
        }
    }

    private var header: some View {
        HStack {
            Text("笺校工作台").font(.headline)
            Spacer()
            docPicker
            Button("导出笺读") { exportLayout() }
                .disabled(selectedDocID == nil || sourceText.isEmpty)
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
                .onAppear {
                    // 反向联动：PDFView 落点（本地坐标）回传，不挡 PDFKit 原生手势。
                    holder.onTap = { [weak self] point in
                        self?.reverseHitTest(at: point)
                    }
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

    // ── 右：原文编辑区（可校） + 文本层片段列表 + Rust 重排预览 ──
    private var rightPane: some View {
        VStack(spacing: 0) {
            // 原文编辑区：加载 PDF 后自动填入整本原文（逐页提取、带页码分隔），可直接校改。
            VStack(alignment: .leading, spacing: 4) {
                Text("原文（已自动提取 · 可校）")
                    .font(.caption2).foregroundStyle(.secondary)
                TextEditor(text: $sourceText)
                    .font(.system(size: 13))
                    .overlay(alignment: .topLeading) {
                        if sourceText.isEmpty {
                            Text("加载 PDF 后自动填入原文…")
                                .foregroundStyle(.tertiary)
                                .padding(6)
                                .allowsHitTesting(false)
                        }
                    }
                    .border(Color.gray.opacity(0.3), width: 1)
            }
            .padding(8)
            .frame(maxHeight: .infinity)

            Divider()
            // 文本层片段（点击 → PDF 红框高亮 + 自动居中 + Rust 重排）
            List(selection: $selectedID) {
                ForEach(segments) { seg in
                    Text(seg.text)
                        .font(.system(size: 14))
                        .tag(seg.id as PdfSegment.ID?)
                }
            }
            .frame(height: 170)
            .overlay {
                if segments.isEmpty {
                    Text("暂无文本片段").foregroundStyle(.tertiary)
                }
            }
            Divider()
            rustPanel
        }
        .onChange(of: selectedID) { _ in
            // 选中分段：① 高亮叠加 + 自动居中（selectedID 驱动 Canvas，centerOnSelected 居中）
            centerOnSelected()
            // ② 把选中行文本喂给纯血 Rust 引擎（calculate_grid / layoutProbe）重排预览
            if let seg = segments.first(where: { $0.id == selectedID }) {
                runRust(seg.text)
            }
        }
    }

    /// 底部「笺读 Rust 重排」预览：把选中文段交给 core.calculate_grid，
    /// 用 Rust 真实排版坐标画字格，证明 Mac/iPad 排版与坐标 100% 一致。
    private var rustPanel: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("笺读 Rust 重排（calculate_grid · 左上原点）")
                .font(.caption2).foregroundStyle(.secondary)
            GeometryReader { _ in
                Canvas { ctx, size in
                    ctx.fill(Path(CGRect(origin: .zero, size: size)),
                             with: .color(.gray.opacity(0.10)))
                    guard !rustBoxes.isEmpty else { return }
                    let minX = rustBoxes.map { CGFloat($0.x) }.min()!
                    let minY = rustBoxes.map { CGFloat($0.y) }.min()!
                    let maxX = rustBoxes.map { CGFloat($0.x + $0.w) }.max()!
                    let maxY = rustBoxes.map { CGFloat($0.y + $0.h) }.max()!
                    let contentW = max(maxX - minX, 1)
                    let contentH = max(maxY - minY, 1)
                    let scale = min(size.width / contentW, size.height / contentH) * 0.92
                    let offX = (size.width - contentW * scale) / 2
                    let offY = (size.height - contentH * scale) / 2
                    for b in rustBoxes {
                        let rx = offX + (CGFloat(b.x) - minX) * scale
                        let ry = offY + (CGFloat(b.y) - minY) * scale
                        let rw = CGFloat(b.w) * scale
                        let rh = CGFloat(b.h) * scale
                        ctx.stroke(Path(roundedRect: CGRect(x: rx, y: ry,
                                                            width: rw, height: rh),
                                        cornerRadius: 1),
                                   with: .color(.red), lineWidth: 0.5)
                        ctx.draw(Text(b.ch).font(.system(size: min(rw, rh) * 0.8)),
                                 at: CGPoint(x: rx + rw / 2, y: ry + rh / 2),
                                 anchor: .center)
                    }
                }
            }
            .frame(height: 150)
            .border(Color.gray.opacity(0.3))
        }
        .padding(8)
    }

    // ── 后台提取 PDF 文本层 + 整本原文（不阻塞主线程）──
    private func loadDocument() {
        guard let id = selectedDocID,
              let doc = store.documents.first(where: { $0.id == id }),
              doc.kind == .pdf else {
            segments = []
            selectedID = nil
            sourceText = ""
            status = "请选择 PDF 文档"
            return
        }
        let url = store.url(for: doc)
        status = "提取 PDF 文本层…"
        Task.detached(priority: .userInitiated) {
            // 同一本 PDF 只开一次：分段（高亮联动）与整本原文（编辑区）一起拿到。
            let (segs, full) = PdfTextExtractor.extractAll(from: url)
            await MainActor.run {
                segments = segs
                selectedID = segs.first?.id
                sourceText = full
                status = "已提取 \(segs.count) 片段 · 原文 \(full.count) 字"
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

    // ── 反向联动：PDF 上点击 / Pencil → 命中右侧分段 ──
    private func reverseHitTest(at point: CGPoint) {
        guard let pv = holder.pdfView,
              let page = pv.currentPage,
              let doc = pv.document else { return }
        let pageIndex = doc.index(for: page)
        // 视图坐标 → PDFKit 左下原点页坐标 → 翻回左上原点（与模型同系）。
        let pagePtYUp = pv.convert(point, to: page)
        let mb = page.bounds(for: .mediaBox)
        let pagePtTL = CGPoint(x: pagePtYUp.x, y: mb.height - pagePtYUp.y)
        if let hit = segments.first(where: {
            $0.pageIndex == pageIndex && $0.rect.contains(pagePtTL)
        }) {
            selectedID = hit.id
        }
    }

    // ── 调用纯血 Rust 引擎重排选中行（后台线程，主线程不卡死）──
    private func runRust(_ text: String) {
        Task.detached(priority: .userInitiated) {
            // layoutProbe 即 core.calculate_grid；单段以 1 列竖排示意。
            let boxes = layoutProbe(text: text, columns: 1)
            await MainActor.run { rustBoxes = boxes }
        }
    }

    // ── 导出笺读 PDF（纯血 Rust 排版引擎，后台线程写盘，不阻塞主线程）──
    private func exportLayout() {
        guard let id = selectedDocID,
              let doc = store.documents.first(where: { $0.id == id }) else {
            status = "请先选择 PDF"
            return
        }
        // 去掉「——— 第 N 页 ———」页码分隔标记（避免被当成正文排版），保留纯原文。
        let cleaned = sourceText
            .split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("———") }
            .joined(separator: "\n")
        guard !cleaned.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            status = "无原文可导出"
            return
        }
        let outURL = store.exportURL(for: doc)
        status = "正在导出笺读 PDF…"
        Task.detached(priority: .userInitiated) {
            do {
                // exportLayoutToPdf(outPath:text:columns:) 由 Rust core 排版并写盘；
                // columns=1 与右侧「笺读 Rust 重排」预览同口径（竖排单栏示意）。
                try exportLayoutToPdf(outPath: outURL.path, text: cleaned, columns: 1)
                await MainActor.run {
                    shareURL = outURL
                    showShare = true
                    status = "已导出：\(outURL.lastPathComponent)"
                }
            } catch {
                await MainActor.run {
                    status = "导出失败：\(error.localizedDescription)"
                }
            }
        }
    }
}

/// 系统分享面板（AirDrop / 存储到文件 / 打印…），把导出的笺读 PDF 交给用户。
/// 用 UIViewControllerRepresentable 包 UIActivityViewController，标准 SwiftUI 桥接。
struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }
    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
