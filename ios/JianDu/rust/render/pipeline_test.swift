// pipeline_test.swift
//
// 笺读 iOS 端口 · P4 全链路无头验证（macOS 命令行）
//
// 这条链路是 P4 的**全部证据**：在没有 Xcode / 没有 iOS 模拟器时，
// 用宿主静态库把「真 Rust 内核 → FFI → JSON → Swift 模型 → 落点 →
// CoreGraphics 出图」整条跑一遍：
//
//   Book JSON ──jiandu_open_book──▶ 会话句柄
//              ──jiandu_get_page──▶ Page JSON（core 排好的页树）
//              ──JSONDecoder──────▶ JianDuPage（Swift 模型）
//              ──JianDuLayouter───▶ [JianDuLaidGlyph]（半格账落点）
//              ──JianDuRenderer───▶ PNG
//
// 与 iOS 的区别只有「链接的库切片不同」（宿主 .a vs xcframework），
// 业务代码一字不改。
//
// 运行（在 ios/JianDu/rust 目录下）：
//   LIB=$(cd ../../../target/debug && pwd)
//   swiftc -I. -L "$LIB" -ljiandu_core \
//          JianduCore.swift \
//          render/JianDuTypography.swift render/JianDuFont.swift render/JianDuRenderer.swift \
//          render/JianDuPageModel.swift render/JianDuLayouter.swift render/pipeline_test.swift \
//          -o /tmp/pipeline_test \
//          -framework CoreGraphics -framework CoreText -framework ImageIO
//   /tmp/pipeline_test

import CoreGraphics
import CoreText
import ImageIO
import Foundation
import JianduCore

@main
struct PipelineTest {
    static func main() {
        // 0) 注册齐伋体
        let qijiPath = ProcessInfo.processInfo.environment["JIANDU_QIJI_PATH"]
            ?? "\(NSHomeDirectory())/Downloads/jiandu/crates/app/assets/fonts/qiji.ttf"
        print("qiji 注册: \(JianDuFont.registerQiji(at: qijiPath) ? "成功" : "失败→系统楷体回退")")
        print("core 版本: \(JianDu.version())")

        // 1) 一本带句读的古籍（GB 级标点进流，show_punct=true 保留）
        let bookJSON = """
        {
          "title": "千字文",
          "author": "周兴嗣",
          "chapters": [
            {"title": "卷一", "paragraphs": [
              {"text": "天地玄黄，宇宙洪荒。日月盈昃，辰宿列张。寒来暑往，秋收冬藏。闰余成岁，律吕调阳。云腾致雨，露结为霜。金生丽水，玉出昆冈。剑号巨阙，珠称夜光。果珍李柰，菜重芥姜。海咸河淡，鳞潜羽翔。"},
              {"text": "龙师火帝，鸟官人皇。始制文字，乃服衣裳。推位让国，有虞陶唐。吊民伐罪，周发殷汤。坐朝问道，垂拱平章。爱育黎首，臣伏戎羌。遐迩一体，率宾归王。鸣凤在竹，白驹食场。化被草木，赖及万方。"}
            ]},
            {"title": "卷二", "paragraphs": [
              {"text": "盖此身发，四大五常。恭惟鞠养，岂敢毁伤。女慕贞洁，男效才良。知过必改，得能莫忘。罔谈彼短，靡恃己长。信使可覆，器欲难量。墨悲丝染，诗赞羔羊。景行维贤，克念作圣。德建名立，形端表正。"},
              {"text": "空谷传声，虚堂习听。祸因恶积，福缘善庆。尺璧非宝，寸阴是竞。资父事君，曰严与敬。孝当竭力，忠则尽命。临深履薄，夙兴温凊。似兰斯馨，如松之盛。川流不息，渊澄取映。容止若思，言辞安定。"}
            ]}
          ]
        }
        """
        // 与 Swift 侧网格同款：12 列 × 21 行，保留句读。
        let optsJSON = #"{"cols":12,"rows":21,"show_punct":true,"west_mode":true,"start_as_chapter":true}"#

        // 2) 开书（真 Rust 内核排版）
        let book = JianDuBook()
        do {
            try book.open(bookJSON: bookJSON, optsJSON: optsJSON)
        } catch {
            fputs("PIPELINE_FAIL 开书失败: \(error) / \(JianDu.lastError())\n", stderr)
            exit(1)
        }
        let meta: JianDuBook.Meta
        do { meta = try book.meta() } catch {
            fputs("PIPELINE_FAIL meta 失败: \(error)\n", stderr); exit(1)
        }
        print("书名: \(meta.title) / 作者: \(meta.author)")
        print("章数: \(meta.chapterCount) / 页数: \(meta.pageCount) / 总字数: \(meta.totalChars)")
        guard meta.pageCount > 0 else { fputs("PIPELINE_FAIL 页数为 0\n", stderr); exit(1) }

        // 3) 取第 0 页并解码成 Swift 模型
        let page: JianDuPage
        do { page = try book.page(at: 0, as: JianDuPage.self) } catch {
            fputs("PIPELINE_FAIL 取页/解码失败: \(error)\n", stderr); exit(1)
        }
        print("第 0 页: 列数=\(page.columns.count) 正文字数=\(page.glyphs.count) 章首=\(page.startChapter)")
        var pieceCount = 0
        var mainCount = 0
        var hangCount = 0
        var gapCount = 0
        var glossCount = 0
        var hangChars: [Character] = []
        for col in page.columns {
            for p in col {
                pieceCount += 1
                switch p {
                case .main(let g):
                    if g.isHang { hangCount += 1; hangChars.append(g.char) } else { mainCount += 1 }
                case .gap: gapCount += 1
                case .gloss: glossCount += 1
                }
            }
        }
        if !hangChars.isEmpty {
            print("  挤压标点字符: \(hangChars.map { "\\u{\(String($0.unicodeScalars.first!.value, radix: 16))} \($0)" }.joined(separator: ", "))")
        }
        print("  片: 共 \(pieceCount)（正文 \(mainCount) / 挤压 \(hangCount) / 空白 \(gapCount) / 夹注 \(glossCount)）")

        // 4) 几何（与 core 同源的 compute_grid 移植）
        let cfg = JianDuLayoutConfig(columns: 12, vertical: true, margin: 0.04,
                                     fsRatio: 0.78, lineRatio: 1.10, lineScale: 1.0,
                                     targetChars: 21, banxinRatio: 0.75)
        let g = JianDuTypography.compute(contentW: 720, contentH: 1040, cfg: cfg, facing: false)
        print("网格: 列=\(g.columns) 行=\(g.rows) 字号=\(String(format: "%.1f", g.fs)) " +
              "字格=\(String(format: "%.1f", g.colW))×\(String(format: "%.1f", g.rowH))")

        // 5) 落点（半格账）+ 出图
        let laid = JianDuLayouter.laidGlyphs(page: page, grid: g, mode: .classic)
        let punctCount = laid.filter { $0.punct != nil }.count
        print("落点: \(laid.count) 枚（其中句读 \(punctCount) 枚，叠画 \(laid.filter { $0.overlay }.count) 枚）")
        guard !laid.isEmpty else { fputs("PIPELINE_FAIL 落点为空\n", stderr); exit(1) }
        // 越界保护：落点不得超出行列范围
        let bad = laid.filter { $0.col >= g.columns || $0.row >= g.rows }
        if !bad.isEmpty { fputs("PIPELINE_FAIL 落点越界 \(bad.count) 枚\n", stderr); exit(1) }

        let W = Int(g.outerW.rounded())
        let H = Int(g.outerH.rounded())
        guard let cs = CGColorSpaceCreateDeviceRGB() as CGColorSpace?,
              let ctx = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            fputs("PIPELINE_FAIL 无法创建 bitmap context\n", stderr); exit(1)
        }
        ctx.clear(CGRect(x: 0, y: 0, width: CGFloat(W), height: CGFloat(H)))
        // 坐标系：渲染器按 **CG 默认（原点左下、y 向上）** 工作——bitmap 上下文
        // 本就是这个坐标系，直接画即可，导出 PNG 即为正立（见 orient 实测：
        // 「上」「L」的最长横都落在图像下半 = 正立）。
        // ⚠️ iOS 的 UIView 是 y 向下的，JianDuPageUIView 里必须先翻回 y 向上再画。
        let box = JianDuRenderer.leafBox(g, leafX: 0, leafY: 0)
        JianDuRenderer.drawPage(ctx, box, g, page: page, mode: .classic, palette: .keben)

        guard let img = ctx.makeImage() else { fputs("PIPELINE_FAIL makeImage 失败\n", stderr); exit(1) }
        let url = URL(fileURLWithPath: "/tmp/jiandu_pipeline.png")
        guard let dest = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil),
              ({
                  CGImageDestinationAddImage(dest, img, nil)
                  return CGImageDestinationFinalize(dest)
              })() else {
            fputs("PIPELINE_FAIL PNG 导出失败\n", stderr); exit(1)
        }
        print("导出 PNG: /tmp/jiandu_pipeline.png (\(W)×\(H))")

        // 6) 逐页扫一遍：确保整本每一页都能解码 + 落点（不崩、不越界）
        var total = 0
        for i in 0..<meta.pageCount {
            do {
                let p = try book.page(at: i, as: JianDuPage.self)
                let l = JianDuLayouter.laidGlyphs(page: p, grid: g, mode: .classic)
                total += l.count
            } catch {
                fputs("PIPELINE_FAIL 第 \(i) 页失败: \(error)\n", stderr); exit(1)
            }
        }
        print("整本 \(meta.pageCount) 页全部解码+落点成功，累计 \(total) 枚")

        // 7) 越界保护：应抛错，不崩
        do {
            _ = try book.page(at: meta.pageCount, as: JianDuPage.self)
            fputs("PIPELINE_FAIL 越界取页应失败\n", stderr); exit(1)
        } catch {
            print("越界取页正确拒绝: \(error)")
        }

        // 8) 业务层门面 JianDuBookSession：同样的书再走一遍（带页缓存）
        do {
            var opts = JianDuLayoutOptions(cols: 12, rows: 21)
            opts.showPunct = true
            let session = try JianDuBookSession(bookJSON: bookJSON, options: opts)
            let p0 = try session.page(at: 0)
            let laid2 = try session.laidGlyphs(pageIndex: 0, grid: g, mode: .classic)
            _ = try session.page(at: 0)   // 二次取页应命中缓存
            print("会话门面: 书名=\(session.title) 页数=\(session.pageCount) " +
                  "第0页列数=\(p0.columns.count) 落点=\(laid2.count)（二次取页命中缓存）")
            guard laid2.count == laid.count else {
                fputs("PIPELINE_FAIL 门面落点数与直连不一致\n", stderr); exit(1)
            }
        } catch {
            fputs("PIPELINE_FAIL 会话门面失败: \(error)\n", stderr); exit(1)
        }

        book.close()
        print("PIPELINE_OK")
    }
}
