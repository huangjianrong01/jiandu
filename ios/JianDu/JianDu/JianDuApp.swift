// 笺读 · iPadOS App 入口
// 正式工程骨架：TabView 组合「书架」(ShelfView) 与「笺校」(CollationView)，
// BookStore 作为 environmentObject 注入，两个标签页及其下钻视图共享同一份数据。
// （PoC 探针页保留在书架右上角「调试」入口，可随时回看 Rust 引擎坐标。）

import SwiftUI

@main
struct JianDuApp: App {
    @StateObject private var store = BookStore()

    var body: some Scene {
        WindowGroup {
            TabView {
                ShelfView()
                    .tabItem { Label("书架", systemImage: "books.vertical") }

                CollationView()
                    .tabItem { Label("笺校", systemImage: "rectangle.split.2x2") }
            }
            .environmentObject(store)
        }
    }
}
