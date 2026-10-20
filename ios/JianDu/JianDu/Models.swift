// 数据模型：与 macOS 端（Rust）的 JSON 结构完全对齐。
//
// 笺读 JSON 书籍格式：
// {
//   "title": "道德经", "author": "老子",
//   "cover":  { "title", "subtitle", "seal", "accent" },
//   "tongjia": { "见": "现" },                 // 本书专属通假表（简体 -> 本字）
//   "toc":    [ { "title", "chapter", "level" } ],
//   "chapters": [ { "title", "paragraphs": [ "纯字符串" 或 { "text", "notes": [...] } ] } ]
// }
// 段落里的注释：{ "at": 段内偏移, "kind": "note" | "tongjia", "text": "注文" }

import Foundation
import SwiftUI

/// 排版方向：古文（竖笺）或现代（横笺）。
enum Script: String, Codable, CaseIterable {
    case ancient
    case modern

    var label: String {
        switch self {
        case .ancient: return "古文竖笺"
        case .modern: return "现代横笺"
        }
    }
}

/// 注释类型。
enum NoteKind: String, Codable {
    case note
    case tongjia
}

/// 一条注释。`at` 是段内字符偏移（从 0 起）。
struct Note: Codable, Equatable {
    var at: Int
    var kind: NoteKind
    var text: String

    init(at: Int, kind: NoteKind, text: String) {
        self.at = at
        self.kind = kind
        self.text = text
    }
}

/// 一个自然段：正文 + 段上注释。
/// 反序列化兼容两种形态：纯字符串，或 `{ "text": ..., "notes": [...] }`。
struct Paragraph: Codable, Equatable {
    var text: String
    var notes: [Note]

    init(text: String, notes: [Note] = []) {
        self.text = text
        self.notes = notes
    }

    init(from decoder: Decoder) throws {
        if let s = try? decoder.singleValueContainer().decode(String.self) {
            self.text = s
            self.notes = []
            return
        }
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.text = try c.decode(String.self, forKey: .text)
        self.notes = try c.decodeIfPresent([Note].self, forKey: .notes) ?? []
    }

    /// 段内指定偏移上的普通注。
    func note(at offset: Int) -> Note? {
        notes.first { $0.at == offset && $0.kind == .note }
    }

    /// 段内指定偏移上的通假注。
    func tongjia(at offset: Int) -> Note? {
        notes.first { $0.at == offset && $0.kind == .tongjia }
    }
}

/// 封面信息：书名、副题、印章文字、主色（0xRRGGBB）。
struct Cover: Codable, Equatable {
    var title: String = ""
    var subtitle: String = ""
    var seal: String = ""
    var accent: UInt32 = 0

    enum CodingKeys: String, CodingKey {
        case title, subtitle, seal, accent
    }

    init() {}

    var sealOrDefault: String {
        seal.isEmpty ? "笺读" : seal
    }

    /// 主色（没给就由书名散列出稳定色，与 macOS 端一致）。
    var accentColor: Color {
        if accent != 0 {
            return Color(argb: accent)
        }
        var hash: UInt32 = 0
        for u in (title.isEmpty ? "笺读" : title).unicodeScalars {
            hash = hash &* 31 &+ u.value
        }
        let palette: [UInt32] = [
            0x8C3B2E, 0x2F4B7C, 0x4A5D23, 0x6B3FA0, 0x1F6F6B, 0x8A6D1F, 0x7C2E4A, 0x3B4A6B,
        ]
        return Color(argb: palette[Int(hash) % palette.count])
    }
}

/// 一章。
struct Chapter: Codable, Equatable {
    var title: String
    var paragraphs: [Paragraph]

    var charCount: Int {
        paragraphs.reduce(0) { $0 + $1.text.count }
    }
}

/// 目录条目。
struct TocEntry: Codable, Equatable, Identifiable {
    /// 同一章可能有多级条目（卷/章/节），id 必须唯一，否则 SwiftUI 会复用错视图。
    var id: String { "\(chapter)-\(level)-\(title)" }
    var title: String
    var chapter: Int
    var level: Int = 1

    enum CodingKeys: String, CodingKey {
        case title, chapter, level
    }

    init(title: String, chapter: Int, level: Int = 1) {
        self.title = title
        self.chapter = chapter
        self.level = max(1, level)
    }
}

/// 一本书。
struct Book: Codable, Equatable, Identifiable {
    var title: String
    var author: String = ""
    var cover: Cover = Cover()
    var tongjia: [String: String] = [:]
    var toc: [TocEntry] = []
    var chapters: [Chapter]

    enum CodingKeys: String, CodingKey {
        case title, author, cover, tongjia, toc, chapters
    }

    init(title: String, author: String = "", chapters: [Chapter] = []) {
        self.title = title
        self.author = author
        self.chapters = chapters
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        title = try c.decode(String.self, forKey: .title)
        author = try c.decodeIfPresent(String.self, forKey: .author) ?? ""
        cover = try c.decodeIfPresent(Cover.self, forKey: .cover) ?? Cover()
        tongjia = try c.decodeIfPresent([String: String].self, forKey: .tongjia) ?? [:]
        toc = try c.decodeIfPresent([TocEntry].self, forKey: .toc) ?? []
        chapters = try c.decode([Chapter].self, forKey: .chapters)
    }

    var totalChars: Int {
        chapters.reduce(0) { $0 + $1.charCount }
    }

    var id: String { title }

    var coverTitle: String {
        cover.title.isEmpty ? title : cover.title
    }

    /// 没有目录时按章节补一份（与 macOS 端 `ensure_toc` 同语义）。
    mutating func ensureToc() {
        if !toc.isEmpty {
            toc.removeAll { $0.chapter >= chapters.count }
            if !toc.isEmpty { return }
        }
        toc = chapters.indices.map { TocEntry(title: chapters[$0].title, chapter: $0) }
    }

    /// 某章的第一面在全书页码中的位置（目录跳转用）。
    func firstPage(ofChapter chapter: Int, capacity: Int) -> Int {
        guard capacity > 0 else { return 0 }
        var slots = 0
        for ci in 0..<chapter where ci < chapters.count {
            for p in chapters[ci].paragraphs {
                slots += 2 + p.text.count // 段首两格缩进 + 正文字数
            }
        }
        return slots / capacity
    }
}

// MARK: - Color(0xRRGGBB / 0xAARRGGBB)

extension Color {
    init(argb: UInt32) {
        let r = Double((argb >> 16) & 0xFF) / 255.0
        let g = Double((argb >> 8) & 0xFF) / 255.0
        let b = Double(argb & 0xFF) / 255.0
        self.init(red: r, green: g, blue: b)
    }
}
