// 封面页：竖排书名、副题、朱印、「翻开书页」。

import SwiftUI

struct CoverView: View {
    @EnvironmentObject var store: BookStore
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ZStack {
            PaperBackground()
                .ignoresSafeArea()

            VStack(spacing: 0) {
                Spacer()

                // 竖排书名（最多八字，多了换小字号）
                let title = store.current?.coverTitle ?? ""
                VerticalText(text: title,
                             size: title.count > 6 ? 34 : 42,
                             color: Paper.ink,
                             weight: .medium,
                             spacing: 6)

                if let sub = store.current?.cover.subtitle, !sub.isEmpty {
                    Text(sub)
                        .font(.system(size: 14))
                        .foregroundColor(Paper.frame)
                        .padding(.top, 16)
                }

                if let author = store.current?.author, !author.isEmpty {
                    Text(author)
                        .font(.system(size: 12))
                        .foregroundColor(Paper.frame.opacity(0.8))
                        .padding(.top, 8)
                }

                Spacer()

                SealBadge(text: store.current?.cover.sealOrDefault ?? "笺读", size: 56)
                    .padding(.bottom, 40)

                Button {
                    withAnimation(.easeInOut(duration: 0.25)) {
                        store.beginReading()
                    }
                } label: {
                    Text("翻开书页 ▸")
                        .font(.system(size: 16, weight: .medium))
                        .foregroundColor(.white)
                        .padding(.horizontal, 28)
                        .padding(.vertical, 12)
                        .background(Paper.cinnabar)
                        .clipShape(Capsule())
                }
                .padding(.bottom, 36)
            }
            .padding(.horizontal, 24)
        }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    store.closeBook()
                } label: {
                    Label("书架", systemImage: "chevron.left")
                }
            }
        }
    }
}
