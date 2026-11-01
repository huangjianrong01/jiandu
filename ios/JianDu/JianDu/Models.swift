import Foundation
import SwiftUI
import UniformTypeIdentifiers

/// 文档类型：当前书架支持导入 PDF 与 PNG（后续可扩展为 txt / 扫描图等）。
enum DocKind: String, Codable, Identifiable {
    case pdf
    case png
    var id: String { rawValue }

    var utType: UTType {
        switch self {
        case .pdf: return .pdf
        case .png: return .png
        }
    }

    var systemImage: String {
        switch self {
        case .pdf: return "doc.fill"
        case .png: return "photo.fill"
        }
    }
}

/// 书架里的一本书 / 一份文档。
/// 文件本体存放在 App 沙盒 Documents/jiandu/ 下，这里只保存「索引」。
struct DocumentItem: Identifiable, Codable, Hashable {
    var id: UUID
    /// 用户可见书名（取自原文件名，去扩展名）。
    var name: String
    /// 落地后的文件名（UUID + 原扩展名），用于在 storeDir 里定位真实文件。
    var fileName: String
    var kind: DocKind
    var addedAt: Date
}

/// 一本书的校对档案：用户校改过的原文 + 上次选中的文本片段。
/// 与文件本体分离，单独存于 Documents/jiandu/collation/<书ID>.json，关 App 不丢。
struct CollationRecord: Codable {
    var sourceText: String
    var selectedID: Int?
    var updatedAt: Date
}

/// 书架数据层（ObservableObject）。
/// 职责：落盘导入的文件 + 维护 JSON 索引 + 给 UI 提供 `@Published` 列表。
/// ⚠️ 纯 SwiftUI 数据层，不触碰 core（Rust）任何逻辑；core 只通过 FFI 在
///    阅读 / 笺校等视图里被调用。
@MainActor
final class BookStore: ObservableObject {
    @Published private(set) var documents: [DocumentItem] = []
    /// 最近一次导入 / 删除失败的原因（UI 可弹 alert）。
    @Published var lastError: String?

    private let fm = FileManager.default
    /// 真实文件落地目录：Documents/jiandu/
    private let storeDir: URL
    /// 索引文件：Documents/jiandu/index.json
    private let indexURL: URL
    /// 校对档案目录：Documents/jiandu/collation/
    private let collationDir: URL

    init() {
        let docs = fm.urls(for: .documentDirectory, in: .userDomainMask)[0]
        storeDir = docs.appendingPathComponent("jiandu", isDirectory: true)
        indexURL = storeDir.appendingPathComponent("index.json")
        collationDir = storeDir.appendingPathComponent("collation", isDirectory: true)
        try? fm.createDirectory(at: storeDir, withIntermediateDirectories: true)
        try? fm.createDirectory(at: collationDir, withIntermediateDirectories: true)
        load()
    }

    /// 取某文档的真实文件 URL（阅读器 / 导出用）。
    func url(for item: DocumentItem) -> URL {
        storeDir.appendingPathComponent(item.fileName)
    }

    /// 导出笺读 PDF 的落盘路径：Documents/jiandu/exports/<书名>_笺读.pdf，
    /// 并确保 exports 目录存在。
    func exportURL(for item: DocumentItem) -> URL {
        let dir = storeDir.appendingPathComponent("exports", isDirectory: true)
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("\(item.name)_笺读.pdf")
    }

    /// 某书的校对档案路径：Documents/jiandu/collation/<书ID>.json
    func collationURL(for item: DocumentItem) -> URL {
        collationDir.appendingPathComponent("\(item.id.uuidString).json")
    }

    /// 读取已存的校对档案（校改稿 + 选中行）；无则返回 nil（首读需从 PDF 现提）。
    func loadCollation(for item: DocumentItem) -> CollationRecord? {
        guard let data = try? Data(contentsOf: collationURL(for: item)) else { return nil }
        return try? JSONDecoder().decode(CollationRecord.self, from: data)
    }

    /// 异步落盘校对档案（后台写文件，不阻塞主线程；写盘失败仅记日志）。
    func saveCollation(_ record: CollationRecord, for item: DocumentItem) {
        let url = collationURL(for: item)
        let dir = collationDir   // 按值捕获，避免在非隔离的 detached 闭包里引用 self
        Task.detached(priority: .background) {
            do {
                try FileManager.default.createDirectory(at: dir,
                                                       withIntermediateDirectories: true)
                let data = try JSONEncoder().encode(record)
                try data.write(to: url, options: .atomic)
            } catch {
                await MainActor.run { self.lastError = "校改存档失败：\(error.localizedDescription)" }
            }
        }
    }

    /// 导入：把用户选中的文件拷贝进沙盒，并登记索引。
    /// 文件拷贝放在后台线程，避免大 PDF 阻塞主线程；写完后回主线程更新列表。
    func importFile(from sourceURL: URL) {
        let id = UUID()
        let ext = sourceURL.pathExtension.lowercased()
        let kind: DocKind = (ext == "png") ? .png : .pdf
        let fileName = "\(id.uuidString).\(ext)"
        let dest = storeDir.appendingPathComponent(fileName)
        // 这些按值捕获进 detached，避免跨 actor 持有 self。
        let source = sourceURL
        let baseName = sourceURL.deletingPathExtension().lastPathComponent

        Task.detached(priority: .userInitiated) {
            do {
                let fm = FileManager.default
                if fm.fileExists(atPath: dest.path) { try fm.removeItem(at: dest) }
                // 安全作用域外拷贝（fileImporter 已授权该 URL 的读权限）。
                try fm.copyItem(at: source, to: dest)
                let item = DocumentItem(
                    id: id, name: baseName, fileName: fileName, kind: kind, addedAt: Date()
                )
                await MainActor.run {
                    self.documents.insert(item, at: 0)
                    self.persist()
                }
            } catch {
                await MainActor.run { self.lastError = error.localizedDescription }
            }
        }
    }

    /// 删除：同时移除索引条目与沙盒里的真实文件。
    func delete(_ item: DocumentItem) {
        documents.removeAll { $0.id == item.id }
        let file = url(for: item)
        try? fm.removeItem(at: file)
        persist()
    }

    // ── 持久化 ──
    private func persist() {
        do {
            let data = try JSONEncoder().encode(documents)
            try data.write(to: indexURL, options: .atomic)
        } catch {
            lastError = "索引写入失败：\(error.localizedDescription)"
        }
    }

    private func load() {
        guard let data = try? Data(contentsOf: indexURL) else { return }
        guard let list = try? JSONDecoder().decode([DocumentItem].self, from: data) else { return }
        // 过滤掉索引还在、但文件被外部删掉的脏条目。
        documents = list.filter { fm.fileExists(atPath: url(for: $0).path) }
    }
}
