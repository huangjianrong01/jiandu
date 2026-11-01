// 笺读 · iPadOS App 入口
// 第 2 步起：根视图切换为「书架」(ShelfView)，由 BookStore 提供数据。
// （PoC 探针页已改为书架右上角「调试」入口，可随时回看 Rust 引擎坐标。）

import SwiftUI

@main
struct JianDuApp: App {
    @StateObject private var store = BookStore()

    var body: some Scene {
        WindowGroup {
            ShelfView()
                .environmentObject(store)
        }
    }
}
