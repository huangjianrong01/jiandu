// 书架数据源：内置样张 + 用户导入 + 阅读进度记忆。
// 与 macOS 端的 ~/.jiandu 对应：iOS 上用 App 的 Documents 目录。

import Foundation
import SwiftUI

/// 书架上的一本书（含来源信息与阅读进度）。
struct ShelfBook: Identifiable, Codable, Equatable {
    let id: String          // 稳定标识：文件名
    var title: String
    var author: String
    var cover: Cover
    var addedAt: Date
    /// 读到的「面」（古文竖笺的一叶 / 现代模式的一段滚动位置由 ReaderView 另行处理）。
    var page: Int
    var script: Script

    init(book: Book, id: String, addedAt: Date = Date(), page: Int = 0, script: Script = .ancient) {
        self.id = id
        self.title = book.title
        self.author = book.author
        self.cover = book.cover
        self.addedAt = addedAt
        self.page = page
        self.script = script
    }
}

/// 书架索引（持久化到 Documents/library.json）。
struct LibraryIndex: Codable {
    var books: [ShelfBook]
}

@MainActor
final class BookStore: ObservableObject {
    @Published var books: [ShelfBook] = []
    /// 当前打开的书（完整内容）。
    @Published var current: Book?
    @Published var currentID: String?
    @Published var notice: String = ""
    /// 是否已进入正文（false = 停在封面页）。
    @Published var reading: Bool = false
    /// 当前书的版式（古文 / 横笺）。
    @Published var currentScript: Script = .ancient

    private let fm = FileManager.default
    private var booksDir: URL { docs.appendingPathComponent("books", isDirectory: true) }
    private var indexURL: URL { docs.appendingPathComponent("library.json") }
    private var docs: URL { fm.urls(for: .documentDirectory, in: .userDomainMask)[0] }

    init() {
        try? fm.createDirectory(at: booksDir, withIntermediateDirectories: true)
        loadIndex()
        if books.isEmpty {
            seedSample()
        }
        #if DEBUG
        // 仅调试构建：供自动化截图/联调用环境变量直接翻到正文，正式包不包含。
        if ProcessInfo.processInfo.environment["JIANDU_AUTO_READ"] == "1",
           let first = books.first {
            open(first)
            beginReading()
        }
        #endif
    }

    // MARK: - 书架索引

    private func loadIndex() {
        guard let data = try? Data(contentsOf: indexURL),
              let idx = try? JSONDecoder().decode(LibraryIndex.self, from: data) else {
            return
        }
        // 只保留文件仍在的书。
        books = idx.books.filter { fm.fileExists(atPath: booksDir.appendingPathComponent($0.id).path) }
    }

    private func saveIndex() {
        if let data = try? JSONEncoder().encode(LibraryIndex(books: books)) {
            try? data.write(to: indexURL, options: .atomic)
        }
    }

    /// 首次启动：把内置《道德经》样张复制进书架。
    private func seedSample() {
        guard let url = Bundle.main.url(forResource: "daodejing", withExtension: "json"),
              var book = try? Book.from(url: url) else {
            notice = "未找到内置样张"
            return
        }
        book.ensureToc()
        let id = "daodejing.json"
        if let data = try? JSONEncoder().encode(book) {
            try? data.write(to: booksDir.appendingPathComponent(id), options: .atomic)
            books.append(ShelfBook(book: book, id: id))
            saveIndex()
            notice = "书架共 \(books.count) 本"
        }
    }

    // MARK: - 打开 / 进度

    func open(_ entry: ShelfBook) {
        let url = booksDir.appendingPathComponent(entry.id)
        guard var book = try? Book.from(url: url) else {
            notice = "打不开：\(entry.title)"
            return
        }
        book.ensureToc()
        current = book
        currentID = entry.id
        currentScript = entry.script
        reading = false
        notice = ""
    }

    /// 从封面页进入正文。
    func beginReading() {
        reading = true
    }

    /// 从正文退回封面页。
    func backToCover() {
        reading = false
    }

    /// 合上书回书架。
    func closeBook() {
        current = nil
        currentID = nil
        reading = false
    }

    func setScript(_ script: Script) {
        currentScript = script
        saveScript(script)
    }

    /// 记住当前书读到的面。
    func savePage(_ page: Int) {
        guard let id = currentID,
              let i = books.firstIndex(where: { $0.id == id }) else { return }
        if books[i].page != page {
            books[i].page = page
            saveIndex()
        }
    }

    func saveScript(_ script: Script) {
        guard let id = currentID,
              let i = books.firstIndex(where: { $0.id == id }) else { return }
        if books[i].script != script {
            books[i].script = script
            saveIndex()
        }
    }

    // MARK: - 导入 / 删除

    /// 导入用户在「文件」App 里选的书（.json / .md / .txt / .epub）。
    func importFile(_ url: URL) {
        let ext = url.pathExtension.lowercased()
        guard ["json", "md", "markdown", "txt", "epub"].contains(ext) else {
            notice = "不支持的格式：.\(ext)（支持 json / md / txt / epub）"
            return
        }
        // 安全作用域：用户通过 fileImporter 选的文件需要显式访问。
        let needsScope = url.startAccessingSecurityScopedResource()
        defer { if needsScope { url.stopAccessingSecurityScopedResource() } }

        do {
            let data = try Data(contentsOf: url)
            var book: Book
            switch ext {
            case "json":
                book = try JSONDecoder().decode(Book.self, from: data)
            case "md", "markdown", "txt":
                book = try TextBookParser.parse(text: String(decoding: data, as: UTF8.self), isMarkdown: ext != "txt")
            case "epub":
                book = try EpubBookParser.parse(data: data)
            default:
                return
            }
            book.ensureToc()
            let id = uniqueID(for: url.lastPathComponent)
            let encoded = try JSONEncoder().encode(book)
            try encoded.write(to: booksDir.appendingPathComponent(id), options: .atomic)
            books.insert(ShelfBook(book: book, id: id), at: 0)
            saveIndex()
            notice = "已上架：\(book.title)"
        } catch {
            notice = "导入失败：\(error.localizedDescription)"
        }
    }

    func remove(_ entry: ShelfBook) {
        try? fm.removeItem(at: booksDir.appendingPathComponent(entry.id))
        books.removeAll { $0.id == entry.id }
        if currentID == entry.id {
            current = nil
            currentID = nil
        }
        saveIndex()
    }

    private func uniqueID(for name: String) -> String {
        var id = name
        var n = 2
        while books.contains(where: { $0.id == id }) || fm.fileExists(atPath: booksDir.appendingPathComponent(id).path) {
            let base = (name as NSString).deletingPathExtension
            id = "\(base)-\(n).\((name as NSString).pathExtension)"
            n += 1
        }
        return id
    }
}

// MARK: - Book 从文件加载

extension Book {
    static func from(url: URL) throws -> Book {
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(Book.self, from: data)
    }
}

// MARK: - txt / md 简易解析（与 macOS 端 importer 的分章规则对齐）

enum TextBookParser {
    /// 极简 epub 解析：取 OPF spine 里的 xhtml 正文，忽略图片与样式。
    static func parse(text: String, isMarkdown: Bool) throws -> Book {
        var title = ""
        var chapters: [Chapter] = []
        var currentTitle: String?
        var buffer: [String] = []

        func flush() {
            guard !buffer.isEmpty else { return }
            let paras = splitParagraphs(buffer.joined(separator: "\n"))
            guard !paras.isEmpty else { return }
            chapters.append(Chapter(title: currentTitle ?? "正文", paragraphs: paras))
            buffer.removeAll()
            currentTitle = nil
        }

        for rawLine in text.components(separatedBy: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if isMarkdown, line.hasPrefix("#") {
                flush()
                currentTitle = String(line.drop(while: { $0 == "#" }).trimmingCharacters(in: .whitespaces))
                if title.isEmpty { title = currentTitle ?? "" }
                continue
            }
            if !isMarkdown, isChapterHeading(line) {
                flush()
                currentTitle = line
                if title.isEmpty { title = line }
                continue
            }
            if !isMarkdown, title.isEmpty, !line.isEmpty {
                // txt 第一行非空且不是章节标题时，暂当作者/书名提示不用，直接进正文。
            }
            buffer.append(rawLine)
        }
        flush()

        if title.isEmpty {
            title = chapters.first?.title ?? "未命名"
        }
        let book = Book(title: title, author: "", chapters: chapters.isEmpty ? [Chapter(title: "正文", paragraphs: [Paragraph(text: text.trimmingCharacters(in: .whitespacesAndNewlines))])] : chapters)
        return book
    }

    /// 「第 X 章 / 卷 X / Chapter X」这类行算章节标题。
    static func isChapterHeading(_ line: String) -> Bool {
        let patterns = ["^第.{1,6}[章节卷篇回]", "^卷.{1,6}", "^Chapter\\s+\\d+", "^CHAPTER\\s+\\d+"]
        for p in patterns {
            if line.range(of: p, options: .regularExpression) != nil { return true }
        }
        return false
    }

    /// 空行分段；没有空行时按句号/分号切。
    static func splitParagraphs(_ text: String) -> [Paragraph] {
        let blocks = text
            .components(separatedBy: "\n\n")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        if blocks.count > 1 {
            return blocks.map { Paragraph(text: $0) }
        }
        // 单块：按句末标点切段
        var paras: [Paragraph] = []
        var cur = ""
        for ch in text {
            cur.append(ch)
            if "。！？；!?;".contains(ch) {
                let t = cur.trimmingCharacters(in: .whitespacesAndNewlines)
                if !t.isEmpty { paras.append(Paragraph(text: t)) }
                cur = ""
            }
        }
        let tail = cur.trimmingCharacters(in: .whitespacesAndNewlines)
        if !tail.isEmpty { paras.append(Paragraph(text: tail)) }
        return paras.isEmpty ? [Paragraph(text: text.trimmingCharacters(in: .whitespacesAndNewlines))] : paras
    }
}

// MARK: - epub 解析（zip 解压 + OPF spine 顺序取正文）

enum EpubBookParser {
    /// 解析 epub：按 OPF 的 spine 顺序拼接各 xhtml 正文。
    static func parse(data: Data) throws -> Book {
        let entries = try MiniZip.entries(data: data)
        guard let opfEntry = entries.first(where: { $0.name.hasSuffix(".opf") }) else {
            throw NSError(domain: "EpubBookParser", code: 1, userInfo: [NSLocalizedDescriptionKey: "epub 里找不到 .opf"])
        }
        let opf = opfEntry.text
        let opfDir = (opfEntry.name as NSString).deletingLastPathComponent

        // 书名
        var title = "EPUB"
        if let r = opf.range(of: #"<dc:title[^>]*>([^<]+)</dc:title>"#, options: .regularExpression) {
            let stripped = String(opf[r]).replacingOccurrences(of: #"</?dc:title[^>]*>"#, with: "", options: .regularExpression)
            let t = stripped.trimmingCharacters(in: .whitespacesAndNewlines)
            if !t.isEmpty { title = t }
        }

        // manifest：id -> href
        var manifest: [String: String] = [:]
        for m in opf.matches(of: #"<item\b[^>]*/?>"#) {
            let tag = String(opf[m])
            if let id = tag.capture(#"id="([^"]+)""#), let href = tag.capture(#"href="([^"]+)""#) {
                manifest[id] = href
            }
        }
        // spine：idref 顺序
        var order: [String] = []
        for m in opf.matches(of: #"<itemref\b[^>]*/?>"#) {
            if let idref = String(opf[m]).capture(#"idref="([^"]+)""#) { order.append(idref) }
        }

        // 按 spine 顺序拼正文；解析失败就退化为「所有 xhtml 按名排序」。
        var bodyParts: [String] = []
        for idref in order {
            guard let href = manifest[idref] else { continue }
            let full = opfDir.isEmpty ? href : "\(opfDir)/\(href)"
            let decoded = full.removingPercentEncoding ?? full
            if let e = entries.first(where: { $0.name == decoded }) {
                bodyParts.append(stripHTML(e.text))
            }
        }
        if bodyParts.isEmpty {
            bodyParts = entries
                .filter { $0.name.hasSuffix(".xhtml") || $0.name.hasSuffix(".html") }
                .sorted { $0.name < $1.name }
                .map { stripHTML($0.text) }
        }
        let body = bodyParts.joined(separator: "\n\n")

        var book = try TextBookParser.parse(text: body, isMarkdown: false)
        if book.title == "未命名" || book.title == "正文" { book.title = title }
        if book.author.isEmpty,
           let r = opf.range(of: #"<dc:creator[^>]*>([^<]+)</dc:creator>"#, options: .regularExpression) {
            book.author = String(opf[r])
                .replacingOccurrences(of: #"</?dc:creator[^>]*>"#, with: "", options: .regularExpression)
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return book
    }

    static func stripHTML(_ html: String) -> String {
        var s = html
        s = s.replacingOccurrences(of: "<script[\\s\\S]*?</script>", with: "", options: .regularExpression)
        s = s.replacingOccurrences(of: "<style[\\s\\S]*?</style>", with: "", options: .regularExpression)
        // 块级标签换成换行，行内标签直接删掉
        s = s.replacingOccurrences(of: #"</(p|div|h[1-6]|li|section|article)[^>]*>"#, with: "\n", options: .regularExpression)
        s = s.replacingOccurrences(of: #"<br\b[^>]*>"#, with: "\n", options: .regularExpression)
        s = s.replacingOccurrences(of: "<[^>]+>", with: "", options: .regularExpression)
        for (k, v) in [("&nbsp;", " "), ("&lt;", "<"), ("&gt;", ">"), ("&amp;", "&"), ("&quot;", "\""), ("&#39;", "'"), ("&ldquo;", "“"), ("&rdquo;", "”")] {
            s = s.replacingOccurrences(of: k, with: v)
        }
        return s
    }
}

/// 极简 ZIP 读取器（stored / deflate 两种，覆盖绝大多数 epub）。
/// iOS 没有暴露解压 API，为避免引入第三方依赖，DEFLATE 用纯 Swift 解码（见 Deflate）。
enum MiniZip {
    struct Entry {
        let name: String
        let data: Data
        var text: String { String(decoding: data, as: UTF8.self) }
    }

    static func entries(data: Data) throws -> [Entry] {
        let bytes = [UInt8](data)
        // 找 EOCD（End Of Central Directory）：签名 0x06054b50
        guard let eocd = lastIndex(bytes, of: [0x50, 0x4b, 0x05, 0x06]) else {
            throw NSError(domain: "MiniZip", code: 2, userInfo: [NSLocalizedDescriptionKey: "不是合法 zip"])
        }
        let count = Int(bytes[eocd + 10]) | Int(bytes[eocd + 11]) << 8
        var offset = Int(bytes[eocd + 16]) | Int(bytes[eocd + 17]) << 8 | Int(bytes[eocd + 18]) << 16 | Int(bytes[eocd + 19]) << 24
        var out: [Entry] = []
        for _ in 0..<count {
            guard bytes[offset] == 0x50, bytes[offset + 1] == 0x4b, bytes[offset + 2] == 0x01, bytes[offset + 3] == 0x02 else { break }
            let method = Int(bytes[offset + 10]) | Int(bytes[offset + 11]) << 8
            let compSize = Int(bytes[offset + 20]) | Int(bytes[offset + 21]) << 8 | Int(bytes[offset + 22]) << 16 | Int(bytes[offset + 23]) << 24
            let nameLen = Int(bytes[offset + 28]) | Int(bytes[offset + 29]) << 8
            let extraLen = Int(bytes[offset + 30]) | Int(bytes[offset + 31]) << 8
            let commentLen = Int(bytes[offset + 32]) | Int(bytes[offset + 33]) << 8
            let localOffset = Int(bytes[offset + 42]) | Int(bytes[offset + 43]) << 8 | Int(bytes[offset + 44]) << 16 | Int(bytes[offset + 45]) << 24
            let nameStart = offset + 46
            let name = String(bytes: bytes[nameStart..<nameStart + nameLen], encoding: .utf8) ?? ""

            // 跳到本地文件头拿数据起点
            let lh = localOffset
            let lNameLen = Int(bytes[lh + 26]) | Int(bytes[lh + 27]) << 8
            let lExtraLen = Int(bytes[lh + 28]) | Int(bytes[lh + 29]) << 8
            let dataStart = lh + 30 + lNameLen + lExtraLen
            let raw = Data(bytes[dataStart..<dataStart + compSize])
            let content: Data
            if method == 0 {
                content = raw
            } else {
                content = try inflate(raw)
            }
            out.append(Entry(name: name, data: content))
            offset += 46 + nameLen + extraLen + commentLen
        }
        return out
    }

    private static func inflate(_ data: Data) throws -> Data {
        try Deflate.inflate(data)
    }

    private static func lastIndex(_ bytes: [UInt8], of sig: [UInt8]) -> Int? {
        if bytes.count < sig.count { return nil }
        var i = bytes.count - sig.count
        while true {
            if Array(bytes[i..<i + sig.count]) == sig { return i }
            if i == 0 { break }
            i -= 1
        }
        return nil
    }
}

// MARK: - 正则小工具

extension String {
    /// 取第一个匹配的捕获组（组 1）。
    func capture(_ pattern: String) -> String? {
        guard let re = try? NSRegularExpression(pattern: pattern),
              let m = re.firstMatch(in: self, range: NSRange(startIndex..., in: self)),
              m.numberOfRanges > 1,
              let r = Range(m.range(at: 1), in: self) else { return nil }
        return String(self[r])
    }

    /// 全部匹配的 Range 列表。
    func matches(of pattern: String) -> [Range<String.Index>] {
        guard let re = try? NSRegularExpression(pattern: pattern) else { return [] }
        return re.matches(in: self, range: NSRange(startIndex..., in: self)).compactMap { Range($0.range, in: self) }
    }
}

// MARK: - 纯 Swift DEFLATE 解码（RFC 1951：stored / fixed / dynamic Huffman）

enum Deflate {
    private static let lengthBase = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258]
    private static let lengthExtra = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0]
    private static let distBase = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577]
    private static let distExtra = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13]

    private static var corrupt: NSError {
        NSError(domain: "Deflate", code: 3, userInfo: [NSLocalizedDescriptionKey: "deflate 数据损坏"])
    }

    static func inflate(_ data: Data) throws -> Data {
        var reader = BitReader(data)
        var out = Data()
        out.reserveCapacity(max(data.count * 4, 64 * 1024))
        while true {
            let bfinal = reader.readBit()
            let btype = reader.readBits(2)
            switch btype {
            case 0: // stored：跳过块头，原样拷贝
                reader.align()
                let p = reader.pos / 8
                guard p + 4 <= reader.bytes.count else { throw corrupt }
                let len = Int(reader.bytes[p]) | Int(reader.bytes[p + 1]) << 8
                guard p + 4 + len <= reader.bytes.count else { throw corrupt }
                out.append(contentsOf: reader.bytes[(p + 4)..<(p + 4 + len)])
                reader.pos = (p + 4 + len) * 8
            case 1: // fixed Huffman
                var litLengths = [Int](repeating: 0, count: 288)
                for i in 0...143 { litLengths[i] = 8 }
                for i in 144...255 { litLengths[i] = 9 }
                for i in 256...279 { litLengths[i] = 7 }
                for i in 280...287 { litLengths[i] = 8 }
                let litTree = Huffman(lengths: litLengths)
                let distTree = Huffman(lengths: [Int](repeating: 5, count: 30))
                try decodeBlock(reader: &reader, out: &out, litTree: litTree, distTree: distTree)
            case 2: // dynamic Huffman
                let (litTree, distTree) = try readDynamicTables(reader: &reader)
                try decodeBlock(reader: &reader, out: &out, litTree: litTree, distTree: distTree)
            default:
                throw corrupt
            }
            if bfinal == 1 { break }
        }
        return out
    }

    private static func decodeBlock(reader: inout BitReader, out: inout Data, litTree: Huffman, distTree: Huffman) throws {
        while true {
            guard let sym = litTree.decode(&reader) else { throw corrupt }
            if sym < 256 {
                out.append(UInt8(sym))
            } else if sym == 256 {
                return
            } else {
                let li = sym - 257
                guard li < lengthBase.count else { throw corrupt }
                let length = lengthBase[li] + (lengthExtra[li] > 0 ? reader.readBits(lengthExtra[li]) : 0)
                guard let dsym = distTree.decode(&reader) else { throw corrupt }
                guard dsym < distBase.count else { throw corrupt }
                let dist = distBase[dsym] + (distExtra[dsym] > 0 ? reader.readBits(distExtra[dsym]) : 0)
                guard dist > 0, out.count >= dist else { throw corrupt }
                let start = out.count - dist
                for i in 0..<length { out.append(out[start + i]) }
            }
        }
    }

    private static func readDynamicTables(reader: inout BitReader) throws -> (Huffman, Huffman) {
        let hlit = reader.readBits(5) + 257
        let hdist = reader.readBits(5) + 1
        let hclen = reader.readBits(4) + 4
        let order = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15]
        var clLengths = [Int](repeating: 0, count: 19)
        for i in 0..<hclen { clLengths[order[i]] = reader.readBits(3) }
        let clTree = Huffman(lengths: clLengths)

        var lengths: [Int] = []
        lengths.reserveCapacity(hlit + hdist)
        while lengths.count < hlit + hdist {
            guard let sym = clTree.decode(&reader) else { throw corrupt }
            switch sym {
            case 0...15:
                lengths.append(sym)
            case 16: // 重复前一个码长 3-6 次
                guard let prev = lengths.last else { throw corrupt }
                for _ in 0..<(3 + reader.readBits(2)) { lengths.append(prev) }
            case 17: // 重复 0，3-10 次
                for _ in 0..<(3 + reader.readBits(3)) { lengths.append(0) }
            case 18: // 重复 0，11-138 次
                for _ in 0..<(11 + reader.readBits(7)) { lengths.append(0) }
            default:
                throw corrupt
            }
        }
        guard lengths.count >= hlit + hdist else { throw corrupt }
        return (Huffman(lengths: Array(lengths[0..<hlit])), Huffman(lengths: Array(lengths[hlit..<(hlit + hdist)])))
    }

    /// 位读取器（DEFLATE 的位是 LSB-first）。
    private struct BitReader {
        let bytes: [UInt8]
        var pos = 0

        init(_ data: Data) { bytes = [UInt8](data) }

        mutating func readBit() -> Int {
            guard pos / 8 < bytes.count else { return 0 }
            let bit = Int((bytes[pos / 8] >> UInt8(pos % 8)) & 1)
            pos += 1
            return bit
        }

        mutating func readBits(_ n: Int) -> Int {
            var v = 0
            for i in 0..<n { v |= readBit() << i }
            return v
        }

        mutating func align() {
            if pos % 8 != 0 { pos = (pos / 8 + 1) * 8 }
        }
    }

    /// 规范 Huffman 解码表。
    private struct Huffman {
        private let counts: [Int]
        private let symbols: [Int]

        init(lengths: [Int]) {
            var counts = [Int](repeating: 0, count: 16)
            for l in lengths where l > 0 { counts[l] += 1 }
            counts[0] = 0
            var offsets = [Int](repeating: 0, count: 16)
            for i in 1..<15 { offsets[i + 1] = offsets[i] + counts[i] }
            var symbols = [Int](repeating: 0, count: lengths.count)
            for (sym, l) in lengths.enumerated() where l > 0 {
                symbols[offsets[l]] = sym
                offsets[l] += 1
            }
            self.counts = counts
            self.symbols = symbols
        }

        func decode(_ reader: inout BitReader) -> Int? {
            var code = 0, first = 0, index = 0
            for len in 1...15 {
                code |= reader.readBit()
                let count = counts[len]
                if code - first < count { return symbols[index + (code - first)] }
                index += count
                first = (first + count) << 1
                code <<= 1
            }
            return nil
        }
    }
}
