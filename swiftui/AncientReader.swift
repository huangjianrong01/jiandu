//
//  AncientReader.swift
//  笺读 JianDu —— 纯 SwiftUI + CoreGraphics 古籍排版（Mac 原生重写方向）
//
//  任务一：核心排版引擎（天头地脚非对称计算 + Canvas 绘图）
//  任务二：设置面板原生化（TabView / Picker.segmented，三 Tab）
//  任务三：原生状态响应与重绘（@State / @AppStorage 驱动 Canvas 自动重排）
//
//  严禁 HTML/CSS/JS 思维：无 div、无 requestAnimationFrame、无手动重绘调用。
//

import SwiftUI
import CoreGraphics
import AppKit

// MARK: - 可枚举的设置项（原始值直接落地 @AppStorage）

/// 留白预设：窄 / 中 / 宽。决定基础 padding，进而带动天头地脚非对称重算。
enum PaddingPreset: String, CaseIterable, Identifiable {
    case narrow, medium, wide
    var id: String { rawValue }
    var label: String {
        switch self { case .narrow: "窄"; case .medium: "中"; case .wide: "宽" }
    }
    /// 基础留白（点）。天头 = padding * 1.5，地脚 = padding * 1.0。
    var padding: CGFloat {
        switch self { case .narrow: 24; case .medium: 40; case .wide: 64 }
    }
}

/// 列数（行款）。
enum ColumnCount: Int, CaseIterable, Identifiable {
    case c8 = 8, c10 = 10, c12 = 12, c14 = 14
    var id: Int { rawValue }
    var label: String { "\(rawValue) 行" }
}

/// 阅读模式。
enum ReadingMode: String, CaseIterable, Identifiable {
    case single, facing, full
    var id: String { rawValue }
    var label: String {
        switch self { case .single: "单页"; case .facing: "对开"; case .full: "整页" }
    }
}

/// 丝栏（界栏样式）：无 / 单丝 / 双丝。
enum BorderStyle: String, CaseIterable, Identifiable {
    case none, single, double
    var id: String { rawValue }
    var label: String {
        switch self { case .none: "无"; case .single: "单丝"; case .double: "双丝" }
    }
}

/// 字体（正文）。
enum FontChoice: String, CaseIterable, Identifiable {
    case song, kai, yuan, li
    var id: String { rawValue }
    var label: String {
        switch self { case .song: "文津宋"; case .kai: "玄冬楷"; case .yuan: "梦源宋"; case .li: "霞鹜楷" }
    }
    /// 字体族名（系统缺失时回退系统宋体）。
    var family: String {
        switch self { case .song: "Songti SC"; case .kai: "Kaiti SC"; case .yuan: "Source Han Serif SC"; case .li: "Xiaolu Yolng" }
    }
}

/// 墨色。
enum InkColor: String, CaseIterable, Identifiable {
    case ink, zhu, dai, qing
    var id: String { rawValue }
    var label: String {
        switch self { case .ink: "墨黑"; case .zhu: "朱砂"; case .dai: "黛蓝"; case .qing: "青简" }
    }
    var color: Color {
        switch self {
        case .ink:  .primary
        case .zhu:  Color(red: 0.70, green: 0.23, blue: 0.19)
        case .dai:  Color(red: 0.18, green: 0.22, blue: 0.30)
        case .qing: Color(red: 0.20, green: 0.36, blue: 0.30)
        }
    }
}

/// 纸色。
enum PaperColor: String, CaseIterable, Identifiable {
    case xuan, mi, qing, ye
    var id: String { rawValue }
    var label: String {
        switch self { case .xuan: "宣纸"; case .mi: "米黄"; case .qing: "青简"; case .ye: "夜读" }
    }
    var color: Color {
        switch self {
        case .xuan: Color(red: 0.95, green: 0.92, blue: 0.84)
        case .mi:   Color(red: 0.93, green: 0.88, blue: 0.74)
        case .qing: Color(red: 0.86, green: 0.90, blue: 0.83)
        case .ye:   Color(red: 0.13, green: 0.13, blue: 0.15)
        }
    }
}

/// 设置面板三个 Tab。
enum SettingsTab: String, CaseIterable, Identifiable {
    case layout, typography, assist
    var id: String { rawValue }
    var label: String {
        switch self { case .layout: "版式"; case .typography: "字体与墨色"; case .assist: "阅读辅助" }
    }
}

// MARK: - 任务一：核心排版引擎（天头地脚非对称计算）

/// 网格计算结果（一切绘制都依据它，且严格受 marginTop / marginBottom 约束）。
struct GridResult {
    let marginTop: CGFloat
    let marginBottom: CGFloat
    let availableHeight: CGFloat
    let colWidth: CGFloat
    let rowHeight: CGFloat
    let fontSize: CGFloat
    let rowsPerColumn: Int
    let columns: Int
    /// 文字块矩形：所有正文 / 界栏 / 夹注的绘制都必须落在它内部。
    let block: CGRect
}

/// 非对称边距 + 动态网格计算。
/// - 天头 = padding * 1.5，地脚 = padding * 1.0（古籍美学：天宽地窄）。
/// - availableHeight = height - marginTop - marginBottom。
/// - colWidth = width / columns；fontSize 由列宽与可用高度共同决定；rowHeight = fontSize * 1.12。
func computeGrid(size: CGSize, columns: Int, padding: CGFloat, fontScale: CGFloat) -> GridResult {
    let marginTop = padding * 1.5
    let marginBottom = padding
    let availableHeight = size.height - marginTop - marginBottom

    let colWidth = size.width / CGFloat(columns)
    // 字号：主要跟随列宽，并受可用高度上限约束（避免高屏把字撑爆）。
    let byWidth = colWidth * 0.82
    let byHeight = availableHeight / 26.0
    let fontSize = min(byWidth, byHeight) * fontScale
    let rowHeight = fontSize * 1.12
    let rowsPerColumn = max(1, Int((availableHeight / rowHeight).rounded(.down)))

    let block = CGRect(x: 0,
                       y: marginTop,
                       width: size.width,
                       height: max(0, availableHeight))
    return GridResult(marginTop: marginTop,
                      marginBottom: marginBottom,
                      availableHeight: availableHeight,
                      colWidth: colWidth,
                      rowHeight: rowHeight,
                      fontSize: fontSize,
                      rowsPerColumn: rowsPerColumn,
                      columns: columns,
                      block: block)
}

/// 解析字体为 SwiftUI Font（缺失回退）。
func resolveFont(_ choice: FontChoice, size: CGFloat) -> Font {
    if NSFont(name: choice.family, size: size) != nil {
        return Font.custom(choice.family, size: size)
    }
    return Font.system(size: size)
}

// MARK: - 任务一：Canvas 绘图上下文（几何由 GeometryReader 提供）

/// 古籍阅读画布：竖排右起、列向左排，全部绘制约束在 [marginTop, height - marginBottom]。
struct ReadingCanvas: View {
    // —— 任务三：所有排版相关的状态都来自 @AppStorage，Canvas 闭包直接依赖它们 ——
    @AppStorage("columns")   private var columnsRaw: Int = 12
    @AppStorage("padding")   private var paddingRaw: Double = 40
    @AppStorage("fontScale") private var fontScaleRaw: Double = 1.0
    @AppStorage("border")    private var borderRaw: String = BorderStyle.single.rawValue
    @AppStorage("font")      private var fontRaw: String = FontChoice.song.rawValue
    @AppStorage("ink")       private var inkRaw: String = InkColor.ink.rawValue
    @AppStorage("paper")     private var paperRaw: String = PaperColor.xuan.rawValue
    @AppStorage("punctOn")   private var punctOn: Bool = true

    /// 竖排样张（演示用，真实数据来自书籍模型）。
    private let sampleText =
        "天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏闰余成岁律吕调阳云腾致雨露结为霜" +
        "君子之道忠恕而已矣吾日三省吾身为人谋而不忠乎与朋友交而不信乎传不习乎温故而知新" +
        "高山仰止景行行止虽不能至心向往之川流不息渊深鱼跃登楼之赋极目千里"

    /// 双行夹注样张（演示：贴在某一列，小字双列）。
    private let glossText = "兰之猗猗扬扬其香不采而佩于兰何伤竹解虚心是我师焉采薇采薇亦采其棘"

    var body: some View {
        GeometryReader { geo in
            // 任务一·1：GeometryReader 取得当前画布可用尺寸。
            let size = geo.size
            let grid = computeGrid(size: size,
                                   columns: max(4, columnsRaw),
                                   padding: CGFloat(paddingRaw),
                                   fontScale: CGFloat(fontScaleRaw))
            let border = BorderStyle(rawValue: borderRaw) ?? .single
            let font = FontChoice(rawValue: fontRaw) ?? .song
            let ink = (InkColor(rawValue: inkRaw) ?? .ink).color
            let paper = (PaperColor(rawValue: paperRaw) ?? .xuan).color

            Canvas { context, _ in
                // 1) 先铺满全屏纸面（在任何裁剪之前），保证天头地脚留白区域也有底色。
                context.fill(Path(CGRect(origin: .zero, size: size)), with: .color(paper))

                // 版框（文武边栏外框）：贴在文字块外缘，须在被裁剪前描绘（它在 block 之外）。
                let block = grid.block
                let frameW: CGFloat = (border == .double) ? 3 : 1
                context.stroke(Path(roundedRect: block.insetBy(dx: -6, dy: 0), cornerRadius: 0),
                               with: .color(ink.opacity(0.85)), lineWidth: frameW)
                if border == .double { // 双丝：再描一道更靠内的细线
                    context.stroke(Path(roundedRect: block.insetBy(dx: -10, dy: 0), cornerRadius: 0),
                                   with: .color(ink.opacity(0.55)), lineWidth: 1)
                }

                // 任务一·5：此后所有文字 / 界栏 / 夹注绘制都被裁剪进文字块，
                // Y 起点 marginTop、终点严格封顶在 height - marginBottom。
                context.clip(to: block)

                // 界栏（乌丝栏）：每个列边界一道竖线，严格在 [marginTop, bottom] 之间。
                if border != .none {
                    for i in 0...grid.columns {
                        let x = size.width - grid.colWidth * CGFloat(i)
                        var line = Path()
                        line.move(to: CGPoint(x: x, y: block.minY))
                        line.addLine(to: CGPoint(x: x, y: block.maxY))
                        context.stroke(line, with: .color(ink.opacity(0.35)), lineWidth: 0.6)
                    }
                }

                // 正文：竖排，列从右向左（column 0 在最右），行从上向下。
                let chars = Array(sampleText)
                var idx = 0
                let noteColumn = grid.columns > 6 ? 2 : 0   // 选定一列放双行夹注
                for c in 0..<grid.columns where idx < chars.count {
                    let xCenter = size.width - grid.colWidth * (CGFloat(c) + 0.5)
                    if c == noteColumn {
                        drawGloss(context, into: CGRect(x: xCenter - grid.colWidth/2,
                                                        y: block.minY,
                                                        width: grid.colWidth,
                                                        height: block.height),
                                  text: glossText, grid: grid, ink: ink, font: font)
                        idx += glossText.count
                        continue
                    }
                    for r in 0..<grid.rowsPerColumn where idx < chars.count {
                        let yCenter = block.minY + grid.rowHeight * (CGFloat(r) + 0.5)
                        let ch = String(chars[idx]); idx += 1
                        let isPunct = ch.unicodeScalars.first
                            .map({ CharacterSet.punctuationCharacters.contains($0) }) ?? false
                        let disp = (punctOn || !isPunct) ? ch : " "
                        let t = Text(verbatim: disp)
                                  .font(resolveFont(font, size: grid.fontSize))
                                  .foregroundColor(ink)
                        context.draw(t, at: CGPoint(x: xCenter, y: yCenter), anchor: .center)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(paper)
        }
    }

    /// 双行夹注绘制：把注文折成两道窄列（左右各半），字号缩小，仍严格裁剪在传入 rect 内。
    private func drawGloss(_ context: GraphicsContext,
                          into rect: CGRect,
                          text: String,
                          grid: GridResult,
                          ink: Color,
                          font: FontChoice) {
        context.saveGState()                 // 别污染全局裁剪（否则后续正文被裁掉）
        context.clip(to: rect)               // 任务一·5：夹注路径同样不越界
        let noteFont = grid.fontSize * 0.5
        let subW = rect.width / 2
        let subH = grid.rowHeight
        let chars = Array(text)
        let half = (chars.count + 1) / 2
        // 右列（side 0）读前半，左列（side 1）续后半（古籍双行夹注惯例）。
        for side in 0..<2 {
            let xCenter = rect.minX + subW * (CGFloat(side) + 0.5)
            let start = side == 0 ? 0 : half
            let end   = side == 0 ? half : chars.count
            var r = 0
            for i in start..<end {
                let yCenter = rect.minY + subH * (CGFloat(r) + 0.5)
                guard yCenter <= rect.maxY else { break }
                let t = Text(verbatim: String(chars[i])).font(resolveFont(font, size: noteFont))
                          .foregroundColor(ink.opacity(0.8))
                context.draw(t, at: CGPoint(x: xCenter, y: yCenter), anchor: .center)
                r += 1
            }
        }
        context.restoreGState()              // 恢复全局裁剪
    }
}

// MARK: - 任务二：设置面板（原生 TabView / Picker.segmented，三 Tab）

struct SettingsPanel: View {
    @State private var tab: SettingsTab = .layout

    var body: some View {
        VStack(spacing: 0) {
            // 三 Tab：用 Picker 的 .segmented 样式（Mac 设置最地道），等价于 TabView 分区。
            Picker("设置", selection: $tab) {
                ForEach(SettingsTab.allCases) { t in
                    Text(t.label).tag(t)
                }
            }
            .pickerStyle(.segmented)
            .padding(14)

            Divider()

            ScrollView {
                switch tab {
                case .layout:     LayoutTab()
                case .typography: TypographyTab()
                case .assist:     AssistTab()
                }
            }
        }
        .frame(width: 320)
        .background(Color(NSColor.windowBackgroundColor))
    }
}

/// Tab 1 · 版式：留白 / 阅读模式 / 列数 / 丝栏。
struct LayoutTab: View {
    @AppStorage("padding")  private var paddingRaw: Double = 40
    @AppStorage("columns")  private var columnsRaw: Int = 12
    @AppStorage("mode")     private var modeRaw: String = ReadingMode.facing.rawValue
    @AppStorage("border")   private var borderRaw: String = BorderStyle.single.rawValue

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            GroupBox("留白（带动天头地脚非对称重算）") {
                Picker("留白", selection: $paddingRaw) {
                    ForEach(PaddingPreset.allCases) { p in
                        Text(p.label).tag(Double(p.padding))
                    }
                }
                .pickerStyle(.segmented)
                Text("天头 = 留白 × 1.5，地脚 = 留白 × 1.0")
                    .font(.caption).foregroundStyle(.secondary)
            }
            GroupBox("阅读模式") {
                Picker("模式", selection: $modeRaw) {
                    ForEach(ReadingMode.allCases) { m in
                        Text(m.label).tag(m.rawValue)
                    }
                }
                .pickerStyle(.segmented)
            }
            GroupBox("列数（行款）") {
                Picker("列数", selection: $columnsRaw) {
                    ForEach(ColumnCount.allCases) { c in
                        Text(c.label).tag(c.rawValue)
                    }
                }
                .pickerStyle(.segmented)
            }
            GroupBox("丝栏（界栏）") {
                Picker("丝栏", selection: $borderRaw) {
                    ForEach(BorderStyle.allCases) { b in
                        Text(b.label).tag(b.rawValue)
                    }
                }
                .pickerStyle(.segmented)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Tab 2 · 字体与墨色：字体 / 墨色（带预览按钮组）+ 字号 Slider。
struct TypographyTab: View {
    @AppStorage("font")      private var fontRaw: String = FontChoice.song.rawValue
    @AppStorage("ink")       private var inkRaw: String = InkColor.ink.rawValue
    @AppStorage("fontScale") private var fontScaleRaw: Double = 1.0

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            GroupBox("字体") {
                // 带预览的自定义按钮组（原生 Button + 选中态）。
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 72))], spacing: 8) {
                    ForEach(FontChoice.allCases) { f in
                        Button {
                            fontRaw = f.rawValue
                        } label: {
                            Text(f.label)
                                .font(resolveFont(f, size: 16))
                                .frame(maxWidth: .infinity)
                                .padding(8)
                                .background(fontRaw == f.rawValue ? Color.accentColor.opacity(0.2)
                                                              : Color.clear)
                                .overlay(RoundedRectangle(cornerRadius: 6)
                                            .stroke(fontRaw == f.rawValue ? Color.accentColor : .clear))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            GroupBox("墨色") {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 72))], spacing: 8) {
                    ForEach(InkColor.allCases) { c in
                        Button {
                            inkRaw = c.rawValue
                        } label: {
                            HStack(spacing: 6) {
                                Circle().fill(c.color).frame(width: 14, height: 14)
                                Text(c.label)
                            }
                            .frame(maxWidth: .infinity)
                            .padding(8)
                            .background(inkRaw == c.rawValue ? Color.accentColor.opacity(0.2) : Color.clear)
                            .overlay(RoundedRectangle(cornerRadius: 6)
                                        .stroke(inkRaw == c.rawValue ? Color.accentColor : .clear))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            GroupBox("字号大小") {
                Slider(value: $fontScaleRaw, in: 0.8...1.2, step: 0.02) {
                    Text("缩放")
                } minimumValueLabel: { Text("小") } maximumValueLabel: { Text("大") }
                Text("当前缩放：\(fontScaleRaw, specifier: "%.2f")×")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Tab 3 · 阅读辅助：纸色 / 简繁 / 通假 / 标点。
struct AssistTab: View {
    @AppStorage("paper")      private var paperRaw: String = PaperColor.xuan.rawValue
    @AppStorage("traditional") private var traditional: Bool = true
    @AppStorage("tongjia")    private var tongjia: Bool = false
    @AppStorage("punctOn")    private var punctOn: Bool = true

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            GroupBox("纸色") {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 72))], spacing: 8) {
                    ForEach(PaperColor.allCases) { p in
                        Button {
                            paperRaw = p.rawValue
                        } label: {
                            HStack(spacing: 6) {
                                RoundedRectangle(cornerRadius: 3).fill(p.color).frame(width: 14, height: 14)
                                Text(p.label)
                            }
                            .frame(maxWidth: .infinity)
                            .padding(8)
                            .background(paperRaw == p.rawValue ? Color.accentColor.opacity(0.2) : Color.clear)
                            .overlay(RoundedRectangle(cornerRadius: 6)
                                        .stroke(paperRaw == p.rawValue ? Color.accentColor : .clear))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            GroupBox("文字与标点") {
                Toggle("繁体显示", isOn: $traditional)
                Toggle("通假字提示", isOn: $tongjia)
                Toggle("显示句读标点", isOn: $punctOn)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - 任务三：声明式装配（状态变化 → Canvas 自动重排，无需手动重绘）

@main
struct AncientReaderApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        .windowStyle(.hiddenTitleBar)
    }
}

struct ContentView: View {
    @State private var showSettings = true

    var body: some View {
        HSplitView {
            ReadingCanvas()
                .frame(minWidth: 360, maxWidth: .infinity, maxHeight: .infinity)
            if showSettings {
                SettingsPanel()
            }
        }
        .toolbar {
            ToolbarItem {
                Button(showSettings ? "隐藏设置" : "设置") {
                    showSettings.toggle()
                }
            }
        }
    }
}
