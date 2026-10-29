import Foundation
import JianduCore

// 无头验证：导入 → 摘要 → 落库 → 取条目 → 开卷 → 记进度 → 移除。
// 用真内核跑（链接 libjiandu_core.a），不依赖 Xcode / UIKit。
//
//   swiftc -I. -L <target/debug> -ljiandu_core \
//          JianduCore.swift JianDuImporter.swift import_test.swift -o /tmp/import_test
//
// 素材：/tmp/jiandu_import_fixtures（千字文.txt / 千字文_gbk.txt / sample.epub）

@main struct ImportTest {
    static func main() {
        let fx = "/tmp/jiandu_import_fixtures"
        let pid = ProcessInfo.processInfo.processIdentifier
        let home = "/tmp/jiandu_ios_home_\(pid)"
        try? FileManager.default.removeItem(atPath: home)
        try? FileManager.default.createDirectory(atPath: home, withIntermediateDirectories: true)
        JianDu.setHome(home) // iOS 上换成 Documents / Application Support

        print("core 版本: \(JianDu.version())")
        print("支持扩展名: \(JianDuImporter.supportedExtensions().joined(separator: ", "))")
        print("")

        let samples = ["千字文.txt", "千字文_gbk.txt", "sample.epub", "随便.pdf"]
        for name in samples {
            let path = "\(fx)/\(name)"
            guard FileManager.default.fileExists(atPath: path) || name.hasSuffix(".pdf") else {
                print("[\(name)] 素材缺失，跳过")
                continue
            }
            guard JianDuImporter.supports(path: path) else {
                print("[\(name)] 内核不认得这个扩展名 —— UI 应在此灰显，实际: \(JianDu.lastError())")
                continue
            }
            do {
                let (info, json) = try JianDuImporter.inspect(path: path)
                print("[\(name)]")
                print("  书名=\(info.title)  作者=\(info.author)  章数=\(info.chapterCount)  字数=\(info.charCount)  JSON=\(json.utf8.count)B")
                print("  首章=\(info.firstChapterTitle)")

                let idx = try JianDuShelf.importFile(atPath: path)
                let entry = try JianDuShelf.entry(at: idx)
                print("  落库 idx=\(idx) 条目书名=\(entry.title) 主色=0x\(String(entry.accentHex, radix: 16)) 进度=\(entry.progress)")

                // 落库后再取回整本 JSON 开卷（iOS 的真实路径）
                let book = JianDuBook()
                try book.open(bookJSON: JianDuShelf.bookJSON(at: idx))
                let m = try book.meta()
                print("  开卷: 书名=\(m.title) 页数=\(m.pageCount) 章数=\(m.chapterCount) 字数=\(m.totalChars)")
                book.close()

                JianDuShelf.touch(index: idx, page: 1, total: max(m.pageCount, 1))
            } catch {
                print("[\(name)] 失败: \(error)")
            }
        }

        print("")
        print("书架共 \(JianDuShelf.count()) 本")
        for (i, e) in JianDuShelf.all().enumerated() {
            print("  #\(i) \(e.title)（\(e.author)）章=\(e.script) 进度=\(String(format: "%.0f%%", e.progress * 100))")
        }

        // 清理：把刚落的书移除，验证 remove 也通
        let n = JianDuShelf.count()
        if n > 0 {
            let ok = JianDuShelf.remove(at: n - 1)
            print("移除末本: \(ok ? "成功" : "失败") → 剩 \(JianDuShelf.count()) 本")
        }
        print("IMPORT_OK")
    }
}
