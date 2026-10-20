// 视觉规范：与 macOS 端（src/app.rs 的配色常量）完全一致。
// 纸色 PAPER #F7F1E3 / 纸背 PAPER_BACK #EAE0C9 / 墨 INK #2B2622 /
// 朱 CINNABAR #A63A2B / 靛 INDIGO #3E5A8A / 框 FRAME #9A6B4F / 丝栏 RULE #B98A63

import SwiftUI

enum Paper {
    static let paper = Color(argb: 0xF7F1E3)
    static let paperBack = Color(argb: 0xEAE0C9)
    static let ink = Color(argb: 0x2B2622)
    static let cinnabar = Color(argb: 0xA63A2B)
    static let indigo = Color(argb: 0x3E5A8A)
    static let frame = Color(argb: 0x9A6B4F)
    static let rule = Color(argb: 0xB98A63)

    /// 版式参数（与 macOS 端一致）。
    static let ancientCols = 8     // 每叶八行（八列）
    static let ancientRows = 11    // 每行十一字
    static let modernLines = 12
    static let modernPerLine = 24
}

/// 竖排文字：逐字成列（用于书封 / 封面页 / 朱印）。
struct VerticalText: View {
    let text: String
    var size: CGFloat
    var color: Color = Paper.ink
    var weight: Font.Weight = .regular
    var spacing: CGFloat = 2

    var body: some View {
        VStack(spacing: spacing) {
            ForEach(Array(text.prefix(8).enumerated()), id: \.offset) { _, ch in
                Text(String(ch))
            }
        }
        .font(.system(size: size, weight: weight))
        .foregroundColor(color)
    }
}

/// 朱印：圆角红底白字。
struct SealBadge: View {
    let text: String
    var size: CGFloat = 44

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.12)
                .fill(Paper.cinnabar)
            RoundedRectangle(cornerRadius: size * 0.12)
                .strokeBorder(Color.white.opacity(0.85), lineWidth: max(1, size * 0.014))
                .padding(size * 0.07)
            VerticalText(text: text, size: size * 0.42, color: .white, weight: .bold, spacing: 0)
        }
        .frame(width: size, height: size)
    }
}

/// 宣纸底：纸色 + 淡淡纸纹（细线）。
struct PaperBackground: View {
    var body: some View {
        ZStack {
            Paper.paper
            Canvas { ctx, size in
                var y: CGFloat = 0
                while y < size.height {
                    ctx.fill(Path(CGRect(x: 0, y: y, width: size.width, height: 0.5)), with: .color(Paper.rule.opacity(0.10)))
                    y += 7
                }
            }
        }
    }
}

/// 书封卡片：宣纸 + 竖排书名 + 朱印 + 进度。
struct BookCoverCard: View {
    let entry: ShelfBook

    var body: some View {
        ZStack {
            PaperBackground()
            VStack(spacing: 0) {
                VerticalText(text: entry.cover.title.isEmpty ? entry.title : entry.cover.title,
                             size: 26, color: Paper.ink, weight: .medium, spacing: 3)
                    .padding(.top, 22)
                if !entry.cover.subtitle.isEmpty {
                    Text(entry.cover.subtitle)
                        .font(.system(size: 11))
                        .foregroundColor(Paper.frame)
                        .padding(.top, 10)
                }
                Spacer()
                SealBadge(text: entry.cover.sealOrDefault, size: 40)
                    .padding(.bottom, 16)
            }
            .padding(.horizontal, 10)
        }
        .aspectRatio(0.72, contentMode: .fit)
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .strokeBorder(Paper.frame.opacity(0.55), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .shadow(color: .black.opacity(0.12), radius: 3, x: 0, y: 2)
    }
}
