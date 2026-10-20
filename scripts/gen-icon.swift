// 笺读 App 图标生成器（macOS 命令运行，无需 Xcode 工程）
// 用法：swift scripts/gen-icon.swift <输出.png>
// 设计：宣纸底 + 纸纹纤维 + 双线墨框 + 竖排「笺读」 + 朱印。
import AppKit

let S: CGFloat = 1024
guard CommandLine.arguments.count > 1 else {
    FileHandle.standardError.write("用法：swift gen-icon.swift <输出.png>\n".data(using: .utf8)!)
    exit(64)
}
let outPath = CommandLine.arguments[1]

// ---- 画布 ----
let img = NSImage(size: NSSize(width: S, height: S))
img.lockFocus()

// AppKit 默认原点在左下，这里统一用「左上角坐标」思考，封装一个换算。
func Y(_ top: CGFloat) -> CGFloat { S - top }

// 纸色底
NSColor(calibratedRed: 0.929, green: 0.906, blue: 0.855, alpha: 1.0).setFill()
NSBezierPath(rect: NSRect(x: 0, y: 0, width: S, height: S)).fill()

// 纸纹：细纤维（LCG 伪随机，保证每次生成一致）
var seed: UInt64 = 20261017
func rnd() -> Double {
    seed = seed &* 6364136223846793005 &+ 1442695040888963407
    return Double((seed >> 33) & 0xFFFFFF) / Double(0xFFFFFF)
}
for _ in 0..<420 {
    let x = rnd() * S
    let yTop = rnd() * S
    let len = 20 + rnd() * 90
    let horiz = rnd() < 0.72
    let alpha = 0.03 + rnd() * 0.05
    NSColor(calibratedRed: 0.55, green: 0.47, blue: 0.36, alpha: alpha).setStroke()
    let p = NSBezierPath()
    p.lineWidth = 0.6 + rnd() * 0.8
    p.move(to: NSPoint(x: x, y: Y(yTop)))
    if horiz {
        p.line(to: NSPoint(x: min(x + len, S), y: Y(yTop)))
    } else {
        p.line(to: NSPoint(x: x, y: Y(min(yTop + len, S))))
    }
    p.stroke()
}

// 四角淡墨晕染
for (cx, cy) in [(S * 0.12, S * 0.12), (S * 0.88, S * 0.12), (S * 0.12, S * 0.88), (S * 0.88, S * 0.88)] {
    let r = NSRect(x: cx - 90, y: Y(cy) - 90, width: 180, height: 180)
    NSColor(calibratedRed: 0.72, green: 0.64, blue: 0.50, alpha: 0.05).setFill()
    NSBezierPath(ovalIn: r).fill()
}

// ---- 双线墨框 ----
func frame(inset: CGFloat, width: CGFloat, alpha: CGFloat) {
    NSColor(calibratedRed: 0.36, green: 0.30, blue: 0.22, alpha: alpha).setStroke()
    let p = NSBezierPath(rect: NSRect(x: inset, y: inset, width: S - inset * 2, height: S - inset * 2))
    p.lineWidth = width
    p.stroke()
}
frame(inset: 34, width: 4, alpha: 0.92)
frame(inset: 54, width: 1.2, alpha: 0.55)

// ---- 竖排「笺读」 ----
let titleChars = Array("笺读")
let titleFont = NSFont(name: "Songti SC", size: 300)
    ?? NSFont(name: "STSong", size: 300)
    ?? NSFont.systemFont(ofSize: 300, weight: .regular)
let ink = NSColor(calibratedRed: 0.15, green: 0.13, blue: 0.10, alpha: 1.0)
let attrs: [NSAttributedString.Key: Any] = [.font: titleFont, .foregroundColor: ink]

// 两字垂直排布，整体偏上，给底部朱印留位。
let charH: CGFloat = 306
let totalH = charH * 2
var yTop = (S - totalH) / 2 - 92
for ch in titleChars {
    let s = String(ch) as NSString
    let size = s.size(withAttributes: attrs)
    let x = (S - size.width) / 2
    s.draw(at: NSPoint(x: x, y: Y(yTop) - size.height), withAttributes: attrs)
    yTop += charH
}

// ---- 朱印：白文「笺读」 ----
let sealSize: CGFloat = 172
let sealX = (S - sealSize) / 2
let sealYTop = S - 128 - sealSize
let sealRect = NSRect(x: sealX, y: Y(sealYTop + sealSize), width: sealSize, height: sealSize)
let sealRed = NSColor(calibratedRed: 0.70, green: 0.22, blue: 0.17, alpha: 0.96)
sealRed.setFill()
NSBezierPath(roundedRect: sealRect, xRadius: 18, yRadius: 18).fill()
// 印面内白框
NSColor(calibratedWhite: 1.0, alpha: 0.85).setStroke()
let inner = sealRect.insetBy(dx: 12, dy: 12)
let ip = NSBezierPath(roundedRect: inner, xRadius: 10, yRadius: 10)
ip.lineWidth = 2.5
ip.stroke()

let sealFont = NSFont(name: "Songti SC", size: 74)
    ?? NSFont(name: "STSong", size: 74)
    ?? NSFont.systemFont(ofSize: 74, weight: .bold)
let sealAttrs: [NSAttributedString.Key: Any] = [.font: sealFont, .foregroundColor: NSColor.white]
for (i, ch) in titleChars.enumerated() {
    let s = String(ch) as NSString
    let size = s.size(withAttributes: sealAttrs)
    let x = sealX + (sealSize - size.width) / 2
    let charTop = sealYTop + 20 + CGFloat(i) * 80
    s.draw(at: NSPoint(x: x, y: Y(charTop) - size.height), withAttributes: sealAttrs)
}

img.unlockFocus()

// ---- 导出 PNG ----
guard let tiff = img.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:]) else {
    FileHandle.standardError.write("PNG 编码失败\n".data(using: .utf8)!)
    exit(1)
}
do {
    try png.write(to: URL(fileURLWithPath: outPath))
    print("图标已生成：\(outPath)（1024×1024）")
} catch {
    FileHandle.standardError.write("写入失败：\(error)\n".data(using: .utf8)!)
    exit(1)
}
