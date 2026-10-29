// render_test.swift
//
// 笺读 iOS 端口 · P3 无头渲染验证（macOS 命令行）
//
// 目的：在**没有 Xcode / 没有 iOS 模拟器**的前提下，用一张真实的 bitmap
// CGContext 把 JianDuRenderer 的所有绘制函数跑一遍，证明：
//   1. 三个渲染源（Typography / Font / Renderer）能一起编译通过；
//   2. 句读墨滴 / 空心圈 / 古风〇 / 版框 / 乌丝栏 / 字 / 中缝鱼尾 / 象鼻 /
//      折痕 / 书口文字 都能在 CGContext 上绘制且不崩；
//   3. 能导出成 PNG（视觉正确与否用 ViewImage 看）。
//
// 这是 P3 能交付的「最强无头证据」——Xcode 集成只是把同样的代码放进去。
//
// 运行：
//   swiftc -I. JianDuTypography.swift JianDuFont.swift JianDuRenderer.swift \
//          render_test.swift -o /tmp/render_test \
//          -framework CoreGraphics -framework CoreText -framework ImageIO
//   /tmp/render_test

import CoreGraphics
import CoreText
import ImageIO
import Foundation

@main
struct RenderTest {
    static func main() {
        // 1) 注册启戟字体（无头用绝对路径；走 JianDuFont.registerQiji(at:)）
        let qijiDefault = "\(NSHomeDirectory())/Downloads/jiandu/crates/app/assets/fonts/qiji.ttf"
        let qijiPath = ProcessInfo.processInfo.environment["JIANDU_QIJI_PATH"] ?? qijiDefault
        let qijiOK = JianDuFont.registerQiji(at: qijiPath)
        print("qiji 注册: \(qijiOK ? "成功" : "失败→走系统楷体回退")")

        // 2) 几何：清版十二行竖排，模拟一叶内容区
        let cfg = JianDuLayoutConfig.standard12
        let contentW: CGFloat = 720
        let contentH: CGFloat = 1040
        let g = JianDuTypography.compute(contentW: contentW, contentH: contentH,
                                         cfg: cfg, facing: false)
        print("网格: 列=\(g.columns) 行=\(g.rows) 字号=\(String(format: "%.1f", g.fs)) " +
              "字格=\(String(format: "%.1f", g.colW))×\(String(format: "%.1f", g.rowH)) " +
              "整纸=\(String(format: "%.0f", g.outerW))×\(String(format: "%.0f", g.outerH))")

        // 3) 构造一页字（竖排：列0在最右；句读挂在若干字上）
        let text: [Character] = Array("天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏闰余成岁律吕调阳云腾致雨露结为霜金生丽水玉出昆冈剑号巨阙珠称夜光果珍李柰菜重芥姜海咸河淡鳞潜羽翔龙师火帝鸟官人皇始制文字乃服衣裳推位让国有虞陶唐吊民伐罪周发殷汤坐朝问道垂拱平章爱育黎首臣伏戎羌遐迩一体率宾归王鸣凤在竹白驹食场化被草木赖及万方盖此身发四大五常恭惟鞠养岂敢毁伤女慕贞洁男效才良知过必改得能莫忘罔谈彼短靡恃己长信使可覆器欲难量墨悲丝染诗赞羔羊景行维贤克念作圣德建名立形端表正空谷传声虚堂习听祸因恶积福缘善庆尺璧非宝寸阴是竞资父事君曰严与敬孝当竭力忠则尽命临深履薄夙兴温凊似兰斯馨如松之盛川流不息渊澄取映容止若思言辞安定笃初诚美慎终宜令荣业所基籍甚无竟学优登仕摄职从政存以甘棠去而益咏乐殊贵贱礼别尊卑上和下睦夫唱妇随外受傅训入奉母仪诸姑伯叔犹子比儿孔怀兄弟同气连枝交友投分切磨箴规仁慈隐恻造次弗离节义廉退颠沛匪亏性静情逸心动神疲守真志满逐物意移坚持雅操好爵自縻都邑华夏东西二京背邙面洛浮渭据泾宫殿盘郁楼观飞惊图写禽兽画彩仙灵丙舍傍启甲帐对楹肆筵设席鼓瑟吹笙升阶纳陛弁转疑星右通广内左达承明既集坟典亦聚群英杜稿钟隶漆书壁经府罗将相路侠槐卿户封八县家给千兵高冠陪辇驱毂振缨世禄侈富车驾肥轻策功茂实勒碑刻铭")
        var glyphs: [JianDuLaidGlyph] = []
        let perCol = g.rows
        for col in 0..<cfg.columns {
            for row in 0..<perCol {
                let idx = col * perCol + row
                guard idx < text.count else { break }
                let ch = text[idx]
                var punct: JianDuPunctKind? = nil
                switch ch {
                case "黄", "荒", "藏", "阳": punct = .seed
                case "霜", "冈", "皇": punct = .ring
                case "光", "翔": punct = .bigSeed
                case "张", "姜": punct = .dot
                default: break
                }
                glyphs.append(JianDuLaidGlyph(ch: ch, col: col, row: row, punct: punct))
            }
        }
        print("本页字形数: \(glyphs.count)")

        // 4) 画布 + 画单叶
        let W = Int(g.outerW.rounded())
        let H = Int(g.outerH.rounded())
        guard let cs = CGColorSpaceCreateDeviceRGB() as CGColorSpace? else { fatalError("colorspace") }
        guard let ctx = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            fatalError("无法创建 bitmap context")
        }
        ctx.clear(CGRect(x: 0, y: 0, width: CGFloat(W), height: CGFloat(H)))
        // 坐标系：渲染器按 **CG 默认（原点左下、y 向上）** 工作，bitmap 上下文
        // 本就是它，直接画即可（iOS 的 UIView 是 y 向下的，那里要先翻回 y 向上）。

        let box = JianDuRenderer.leafBox(g, leafX: 0, leafY: 0)
        JianDuRenderer.drawLeaf(ctx, box, g, glyphs: glyphs, palette: .keben)

        func exportPNG(_ context: CGContext, _ w: Int, _ h: Int, _ path: String) {
            guard let img = context.makeImage() else { fatalError("makeImage 失败: \(path)") }
            let url = URL(fileURLWithPath: path)
            guard let dest = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else {
                fatalError("CGImageDestination 失败: \(path)")
            }
            CGImageDestinationAddImage(dest, img, nil)
            guard CGImageDestinationFinalize(dest) else { fatalError("finalize 失败: \(path)") }
            print("导出 PNG: \(path) (\(w)×\(h))")
        }

        exportPNG(ctx, W, H, "/tmp/jiandu_page.png")

        // 5) 单独验证中缝：鱼尾 + 象鼻 + 折痕 + 书口文字（竖排对开模式下才有）
        let gw = g.gutterW
        let gW = Int((gw + 60).rounded())
        let gH = 420
        guard let gctx = CGContext(data: nil, width: gW, height: gH, bitsPerComponent: 8,
                                   bytesPerRow: 0, space: cs,
                                   bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            fatalError("无法创建 gutter context")
        }
        gctx.clear(CGRect(x: 0, y: 0, width: CGFloat(gW), height: CGFloat(gH)))
        gctx.saveGState()
        gctx.setFillColor(JianDuPalette.keben.paper)
        gctx.fill(CGRect(x: 0, y: 0, width: CGFloat(gW), height: CGFloat(gH)))
        gctx.restoreGState()
        JianDuRenderer.drawGutter(gctx, cx: CGFloat(gW) / 2.0, top: 24, bottom: CGFloat(gH) - 24,
                                  g: g, bookMouth: "笺读", leafNum: "三", palette: .keben)
        exportPNG(gctx, gW, gH, "/tmp/jiandu_gutter.png")

        print("RENDER_OK")
    }
}
