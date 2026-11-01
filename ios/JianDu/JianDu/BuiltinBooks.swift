import Foundation

/// 内置公版古籍清单条目，镜像 `builtin_books.json`。
/// Bundle 是只读的，清单随 App 一同发布；真正的书文件在首次启动时
/// 由 `BookStore.ensureBuiltinBooks()` 复制到用户可写的沙盒。
struct BuiltinBook: Codable, Identifiable {
    var id: String { key }
    let key: String
    let title: String
    let author: String
    let intro: String
    /// Bundle 内 PDF 文件名（如 "daodejing.pdf"）。
    let resource: String
    /// Bundle 内封面图文件名（如 "builtin_daodejing_cover.png"）。
    let cover: String
    /// 资源类型："pdf" / "png"。
    let kind: String
}

/// 清单根对象。
struct BuiltinManifest: Codable {
    let version: Int
    let books: [BuiltinBook]
}

/// 内置书的只读访问 + 首次初始化守卫。
enum BuiltinBooks {
    /// UserDefaults 守卫键：首次初始化后置 true，之后永不再自动复制。
    /// 这是「防呆」核心——尊重用户删除，绝不流氓回塞。
    static let initializedKey = "isBuiltinBooksInitialized"

    /// 从 App Bundle（只读）读取内置书清单。
    static func loadManifest() -> BuiltinManifest? {
        guard let url = Bundle.main.url(forResource: "builtin_books", withExtension: "json") else {
            return nil
        }
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(BuiltinManifest.self, from: data)
    }

    /// 是否已执行过首次初始化。
    static var isInitialized: Bool {
        UserDefaults.standard.bool(forKey: initializedKey)
    }

    /// 标记首次初始化完成（写 UserDefaults）。
    static func markInitialized() {
        UserDefaults.standard.set(true, forKey: initializedKey)
    }
}
