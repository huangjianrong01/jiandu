import PDFKit
import Foundation

/// 从 PDF 文本层提取出的一个片段（一行 / 一段）。
/// ⚠️ 坐标系：统一「左上原点、y 向下」（与 Rust core 的 GlyphBoxFfi 完全一致），
///    由 PDFKit 原生「左下原点、y 向上」的页坐标做 y 翻转得到
///    (x, pageHeight - y - height, width, height)。
///    这样整本 App 的「模型坐标」都和 Rust 严丝合缝，便于后续高亮联动 / 混排。
struct PdfSegment: Identifiable, Hashable {
    let id: Int
    let text: String
    /// 左上原点页坐标（与 Rust 同系）
    let rect: CGRect
    let pageIndex: Int
    /// 该页原始高度（做 y 翻转 / 反向命中时还原左下原点需要）
    let pageHeight: CGFloat
}

enum PdfTextExtractor {
    /// 提取整本文档文本层：按页、按视觉行切分，返回 (文本, 左上原点包围盒)。
    ///
    /// 设计说明（架构对齐）：
    /// - iOS 上 Rust core 不具备 PDF 文本层解析能力（core 零 PDF 解析依赖，
    ///   Mac 端文本层提取同样依赖 PDFKit），故此处用 iOS 原生 PDFKit 读真实原图坐标。
    /// - 提取出的坐标立即翻转为左手上原点，与 Rust calculate_grid 输出的 GlyphBoxFfi
    ///   坐标系一致；纯血 Rust 引擎负责「重排」，保证 Mac / iPad 排版结果 100% 一致。
    static func extract(from url: URL) -> [PdfSegment] {
        guard let doc = PDFDocument(url: url) else { return [] }
        var out: [PdfSegment] = []
        var sid = 0
        for i in 0..<doc.pageCount {
            guard let page = doc.page(at: i) else { continue }
            let mb = page.bounds(for: .mediaBox)
            let pageH = mb.height
            guard let full = page.selection(for: mb) else { continue }
            // 按视觉行切分；竖排 PDF 也会被聚成视觉行，足以驱动首版高亮联动。
            for line in full.selectionsByLine() {
                let t = (line.string ?? "")
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                guard !t.isEmpty else { continue }
                // PDFSelection.bounds(for:) 接收的是「页对象」（不是 box），
                // 返回「页内容坐标系（左下原点、y 向上）」包围盒，与 mediaBox 同系；
                // 翻转成左上原点（与 Rust 同系）。
                let r = line.bounds(for: page)   // 左下原点、y 向上
                // 翻转成左上原点（与 Rust 同系）
                let rTL = CGRect(x: r.origin.x,
                                 y: pageH - r.origin.y - r.height,
                                 width: r.width, height: r.height)
                out.append(PdfSegment(id: sid, text: t,
                                      rect: rTL, pageIndex: i,
                                      pageHeight: pageH))
                sid += 1
            }
        }
        return out
    }
}
