import Foundation
import SwiftUI
import UniformTypeIdentifiers
import UIKit

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
    /// 用户可见书名（取自原文件名，去扩展名；内置书则为书名）。
    var name: String
    /// 落地后的文件名（UUID 或内置 key + 原扩展名），用于在 storeDir 里定位真实文件。
    var fileName: String
    var kind: DocKind
    var addedAt: Date
    /// 是否 App 内置公版书（区别于用户导入）。默认 false，兼容旧索引。
    var isBuiltin: Bool = false
    /// 内置书在清单里的稳定 key（如 "daodejing"）；用户导入书为 nil。
    var builtinKey: String? = nil
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
    /// 内置书清单索引（key → 条目），用于书架取封面 / 简介 / 作者。
    private var builtinByKey: [String: BuiltinBook] = [:]

    init() {
        let docs = fm.urls(for: .documentDirectory, in: .userDomainMask)[0]
        storeDir = docs.appendingPathComponent("jiandu", isDirectory: true)
        indexURL = storeDir.appendingPathComponent("index.json")
        collationDir = storeDir.appendingPathComponent("collation", isDirectory: true)
        try? fm.createDirectory(at: storeDir, withIntermediateDirectories: true)
        try? fm.createDirectory(at: collationDir, withIntermediateDirectories: true)
        // 先载入清单（无论是否已初始化，书架都需要它来取封面/简介）。
        if let manifest = BuiltinBooks.loadManifest() {
            for book in manifest.books { builtinByKey[book.key] = book }
        }
        load()
        // 首次启动把内置书复制到沙盒；已初始化则跳过（尊重用户删除）。
        ensureBuiltinBooks()
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

    // ── 内置公版书：首次启动从只读 Bundle 复制到可写沙盒 ──

    /// 首次启动把内置书从只读 Bundle 复制到用户可写沙盒，并登记进索引。
    ///
    /// 防呆核心：`UserDefaults.isBuiltinBooksInitialized` 一旦为 true 就整段跳过——
    /// 即使用户手动删掉了某本内置书，下次启动也**绝不**强行回塞，尊重用户操作。
    /// 若清单缺失（异常态），也标记初始化，避免每次启动都失败重试。
    func ensureBuiltinBooks() {
        guard !BuiltinBooks.isInitialized else { return }
        guard let manifest = BuiltinBooks.loadManifest() else {
            BuiltinBooks.markInitialized()
            return
        }
        for book in manifest.books {
            let ext = URL(fileURLWithPath: book.resource).pathExtension.lowercased()
            let destName = "\(book.key).\(ext)"
            let dest = storeDir.appendingPathComponent(destName)
            // Bundle 内资源（只读）。
            guard let src = Bundle.main.url(forResource: book.resource, withExtension: nil) else {
                continue
            }
            if !fm.fileExists(atPath: dest.path) {
                try? fm.copyItem(at: src, to: dest)
            }
            // 用稳定 key 作文件名前缀，便于跨启动定位与关联校对档案。
            let item = DocumentItem(
                id: UUID(),
                name: book.title,
                fileName: destName,
                kind: (ext == "png") ? .png : .pdf,
                addedAt: Date(),
                isBuiltin: true,
                builtinKey: book.key
            )
            documents.append(item)
        }
        persist()
        BuiltinBooks.markInitialized()
    }

    /// 内置书封面图（Bundle 内 PNG）；非内置或缺失返回 nil。
    func builtinCover(for item: DocumentItem) -> UIImage? {
        guard let key = item.builtinKey, let book = builtinByKey[key] else { return nil }
        return UIImage(named: book.cover)
    }

    /// 内置书作者；非内置返回 nil。
    func builtinAuthor(for item: DocumentItem) -> String? {
        guard let key = item.builtinKey else { return nil }
        return builtinByKey[key]?.author
    }

    /// 内置书简介；非内置返回 nil。
    func builtinIntro(for item: DocumentItem) -> String? {
        guard item.isBuiltin, let key = item.builtinKey else { return nil }
        return builtinByKey[key]?.intro
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
