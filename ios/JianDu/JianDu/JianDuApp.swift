// 笺读 · iPhone 端
// 书架（书封墙）→ 封面页 → 正文（古文竖笺 / 现代横笺）。

import SwiftUI

@main
struct JianDuApp: App {
    @StateObject private var store = BookStore()

    var body: some Scene {
        WindowGroup {
            root
                .environmentObject(store)
        }
    }

    @ViewBuilder
    private var root: some View {
        if store.current == nil {
            ShelfView()
                .transition(.opacity)
        } else if !store.reading {
            CoverView()
                .transition(.move(edge: .trailing))
        } else {
            ReaderView()
                .transition(.move(edge: .trailing))
        }
    }
}
