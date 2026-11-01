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
    /// 当前已载入（其状态在内存中）的书 ID；切换前先存旧书，避免校改丢失。
    @State private var loadedDocID: DocumentItem.ID?
    /// 防抖落盘任务句柄。
    @State private var saveTask: Task<Void, Never>?
    /// 导出笺读 PDF 的落盘 URL（成功后弹出系统分享面板用）。
    @State private var shareURL: URL?
    @State private var showShare: Bool = false
    /// 分段在原文（sourceText）中的字符区间表，供「光标落点 → 高亮分段」实时映射。
    @State private var segRanges: [(range: Range<String.Index>, id: PdfSegment.ID)] = []
    /// 编辑器光标在原文中的 UTF-16 偏移（由 SourceTextView 实时回传），驱动左侧高亮。
    @State private var caretLocation: Int = 0

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
        // 原文编辑（校改）防抖落盘：关 App、切书都不丢。
        .onChange(of: sourceText) { _ in scheduleSave() }
        .sheet(isPresented: $showShare) {
            if let url = shareURL {
                ShareSheet(items: [url])
            }
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Text("笺校").font(.headline).foregroundStyle(Color.inkColor)
            Spacer()
            docPicker
            Button {
                exportLayout()
            } label: {
                Image(systemName: "square.and.arrow.up")
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .tint(Color.inkColor)
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
                        .opacity(selectedID == nil ? 0 : 1)
                        .animation(.easeInOut(duration: 0.18), value: selectedID)
                }
                .onAppear {
                    // 反向联动：PDFView 落点（本地坐标）回传，不挡 PDFKit 原生手势。
                    // CollationView 是 struct（值类型），不可 weak；直接捕获 self 即可
                    // （值类型不产生引用循环，且本视图在 TabView 中长期存活，无泄漏风险）。
                    holder.onTap = { point in
                        self.reverseHitTest(at: point)
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
            ctx.fill(Path(vRect), with: .color(Color.zhuSha.opacity(0.22)))
            ctx.stroke(Path(vRect), with: .color(Color.zhuSha), lineWidth: 2)
        }
        .id(holder.scrollNonce)
    }

    // ── 右：原文编辑区（宣纸黄 + 古籍宋体，光标实时驱动左侧高亮） + Rust 重排预览 ──
    private var rightPane: some View {
        VStack(spacing: 0) {
            // 可编辑原文：加载 PDF 后自动填入（由分段精确构造，光标落点 ↔ 高亮框 1:1 对应）。
            // 背景宣纸黄、字体宋体；光标移动经 SourceTextView 回传 → caretToSegment → 左侧朱砂红框实时出现并平滑居中。
            SourceTextView(text: $sourceText, caretLocation: $caretLocation, family: "Songti SC")
                .background(Color.paperYellow)
                .cornerRadius(12)
                .padding(8)
                .frame(maxHeight: .infinity)
                .overlay(alignment: .topLeading) {
                    if sourceText.isEmpty {
                        Text("加载 PDF 后自动填入原文…")
                            .foregroundStyle(.tertiary)
                            .padding(14)
                            .allowsHitTesting(false)
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
            // ③ 记忆上次选中行（持久化）
            scheduleSave()
        }
        .onChange(of: caretLocation) { _ in
            // 光标移动 → 命中所在分段 → 驱动左侧高亮（仅当选中分段改变才滚动居中，避免逐字跳动）。
            caretToSegment(location: caretLocation)
        }
    }

    /// 底部「笺读 Rust 重排」预览：把选中文段交给 core.calculate_grid，
    /// 用 Rust 真实排版坐标画字格，证明 Mac/iPad 排版与坐标 100% 一致。
    private var rustPanel: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("笺读 · Rust 重排预览（calculate_grid · 左上原点）")
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
        }
        .padding(8)
        .background(Color.paperWarm.opacity(0.45))
        .cornerRadius(10)
    }

    // ── 后台提取 PDF 文本层 + 整本原文（不阻塞主线程）──
    private func loadDocument() {
        // 切换前，先把当前已载入书的校改落盘，避免一切换就丢。
        if let prev = loadedDocID, prev != selectedDocID {
            saveCurrentCollation(for: prev)
        }
        guard let id = selectedDocID,
              let doc = store.documents.first(where: { $0.id == id }),
              doc.kind == .pdf else {
            segments = []
            selectedID = nil
            sourceText = ""
            loadedDocID = nil
            status = "请选择 PDF 文档"
            return
        }
        let url = store.url(for: doc)
        status = "提取 PDF 文本层…"
        Task.detached(priority: .userInitiated) {
            // 同一本 PDF 只开一次：分段（高亮联动）与整本原文（编辑区）一起拿到。
            let (segs, _) = PdfTextExtractor.extractAll(from: url)
            await MainActor.run {
                segments = segs
                // 优先用已存校改稿；无则取新鲜提取（由分段精确构造）并立即落盘建档。
                if let rec = store.loadCollation(for: doc) {
                    sourceText = rec.sourceText
                    // 校改稿可能已被改动：尽力按分段顺序重建区间映射（改动处可能失配，不崩溃）。
                    segRanges = CollationView.rebuildRanges(segs, in: rec.sourceText)
                    selectedID = rec.selectedID
                    status = "已载入校改稿 · \(segs.count) 片段"
                } else {
                    // 由分段精确构造可编辑原文 + 每段字符区间（光标 → 高亮 1:1 对应）。
                    let built = CollationView.buildSource(segs)
                    sourceText = built.text
                    segRanges = built.ranges
                    selectedID = segs.first?.id
                    store.saveCollation(CollationRecord(sourceText: built.text,
                                                       selectedID: segs.first?.id,
                                                       updatedAt: Date()), for: doc)
                    status = "已提取 \(segs.count) 片段 · 原文 \(built.text.count) 字"
                }
                loadedDocID = id
            }
        }
    }

    /// 把当前内存里的校改（原文 + 选中行）落盘到指定书。
    private func saveCurrentCollation(for docID: DocumentItem.ID) {
        guard let doc = store.documents.first(where: { $0.id == docID }) else { return }
        store.saveCollation(CollationRecord(sourceText: sourceText,
                                            selectedID: selectedID,
                                            updatedAt: Date()), for: doc)
    }

    /// 编辑原文 / 切换选中后防抖落盘（避免每次击键都写盘）。
    private func scheduleSave() {
        saveTask?.cancel()
        let docID = loadedDocID
        guard docID != nil else { return }
        saveTask = Task {
            try? await Task.sleep(nanoseconds: 500_000_000)   // 0.5s 防抖
            guard !Task.isCancelled, let docID = docID else { return }
            await MainActor.run { saveCurrentCollation(for: docID) }
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
        // go(to:) 自带平滑滚动动画；不再调用 layoutDocumentView()，避免打断居中动画。
        pv.go(to: dest)
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

    // ── 原文构造与「光标 → 分段」实时映射（去按钮化联动核心）──

    /// 由分段精确构造可编辑原文：逐页插入「——— 第 N 页 ———」分隔，每段一行；
    /// 同时记录每段在原文中的字符区间，使光标落点与高亮框 1:1 对应、零搜索歧义。
    private static func buildSource(_ segs: [PdfSegment])
        -> (text: String, ranges: [(range: Range<String.Index>, id: PdfSegment.ID)]) {
        var text = ""
        var ranges: [(Range<String.Index>, PdfSegment.ID)] = []
        var lastPage = -1
        for seg in segs {
            if seg.pageIndex != lastPage {
                if !text.isEmpty { text += "\n" }
                text += "——— 第 \(seg.pageIndex + 1) 页 ———\n"
                lastPage = seg.pageIndex
            }
            let start = text.endIndex
            text += seg.text + "\n"
            ranges.append((start..<text.index(before: text.endIndex), seg.id))
        }
        return (text, ranges)
    }

    /// 在（可能已被校改的）原文中尽力重建分段区间：按分段顺序顺序查找 seg.text。
    /// 校改过的段落若不再匹配则跳过该段映射（高亮在该处不更新，不崩溃）。
    private static func rebuildRanges(_ segs: [PdfSegment], in text: String)
        -> [(range: Range<String.Index>, id: PdfSegment.ID)] {
        var ranges: [(Range<String.Index>, PdfSegment.ID)] = []
        var from = text.startIndex
        for seg in segs {
            if let r = text.range(of: seg.text, range: from..<text.endIndex) {
                ranges.append((r, seg.id))
                from = r.upperBound
            }
        }
        return ranges
    }

    /// 编辑器光标（UITextView 的 UTF-16 偏移）→ 命中所在分段 → 驱动左侧朱砂红框 + 居中。
    /// 仅当选中分段真正改变时才写 selectedID（进而触发滚动居中），避免逐字跳动。
    private func caretToSegment(location: Int) {
        guard !segRanges.isEmpty else { return }
        let utf16 = sourceText.utf16
        guard let idx = utf16.index(utf16.startIndex, offsetBy: location,
                                    limitedBy: utf16.endIndex) else { return }
        guard let sIdx = String.Index(idx, within: sourceText) else { return }
        if let hit = segRanges.first(where: { $0.range.contains(sIdx) }),
           selectedID != hit.id {
            selectedID = hit.id
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

// MARK: - 配色（对齐 Mac 端笺读视觉语言）

extension Color {
    /// 右栏宣纸黄（与 Mac 端 PAPER_R 0xF7F0DC 同色）。
    static let paperYellow = Color(red: 0xF7 / 255, green: 0xF0 / 255, blue: 0xDC / 255)
    /// 左栏 / 面板暖纸底（与 Mac 端 PAPER_L 0xDDD2B4 同系）。
    static let paperWarm = Color(red: 0xDD / 255, green: 0xD2 / 255, blue: 0xB4 / 255)
    /// 墨色（正文 / 标题）。
    static let inkColor = Color(red: 0x33 / 255, green: 0x2B / 255, blue: 0x22 / 255)
    /// 朱砂红（高亮框，与 Mac 端 keben zhu 0xB0342C 同色系）。
    static let zhuSha = Color(red: 0xB0 / 255, green: 0x34 / 255, blue: 0x2C / 255)
}

// MARK: - 可编辑原文视图（暴露光标位置，驱动实时联动）

/// 包 UITextView 为 SwiftUI 视图，用于右侧可校原文编辑区。
/// 相比 SwiftUI 原生 `TextEditor`，它额外暴露光标位置（selectedRange.location，
/// UTF-16 偏移），从而把「光标落在某句 → 左侧朱砂红框实时出现并平滑居中」做成
/// 去按钮化的实时联动。背景透明，由外层容器铺宣纸黄；字体用古籍宋体（Songti SC）。
struct SourceTextView: UIViewRepresentable {
    @Binding var text: String
    /// 光标在原文中的 UTF-16 偏移，实时回传给父视图驱动高亮。
    @Binding var caretLocation: Int
    /// 字体家族（传入 "Songti SC" 即古籍宋体）。
    var family: String

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> UITextView {
        let tv = UITextView()
        tv.delegate = context.coordinator
        let desc = UIFontDescriptor(fontAttributes: [.family: family])
        tv.font = UIFont(descriptor: desc, size: 18)
        tv.backgroundColor = .clear
        tv.textColor = UIColor(Color.inkColor)
        tv.isEditable = true
        tv.isSelectable = true
        tv.textContainerInset = UIEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
        tv.autocorrectionType = .no
        tv.smartQuotesType = .no
        tv.smartDashesType = .no
        tv.text = text
        context.coordinator.parent = self
        return tv
    }

    func updateUIView(_ uiView: UITextView, context: Context) {
        context.coordinator.parent = self
        // 仅在文本确实不同步时回填（如新建 / 切书），不干扰用户正常输入与光标。
        if uiView.text != text {
            let sel = uiView.selectedRange
            uiView.text = text
            uiView.selectedRange = sel   // 尽量保留光标（越界由系统自动夹取）
        }
    }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: SourceTextView

        override init() { parent = SourceTextView(text: .constant(""), caretLocation: .constant(0), family: "Songti SC") }

        func textViewDidChange(_ tv: UITextView) {
            parent.text = tv.text ?? ""
        }

        func textViewDidChangeSelection(_ tv: UITextView) {
            parent.caretLocation = tv.selectedRange.location
        }
    }
}
