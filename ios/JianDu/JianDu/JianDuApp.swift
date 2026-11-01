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
        // PoC 临时入口：直出探针页，验证 Rust 桥接与排版。
        // 还原原 App：git checkout -- ios/JianDu/JianDu/JianDuApp.swift
        ProbeView()
    }
}
