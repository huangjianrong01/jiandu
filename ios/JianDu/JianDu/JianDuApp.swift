// 笺读 · iPadOS PoC 入口
// 仅用于验证 uniffi 桥接 + Rust 真实排版（calculate_grid）+ iOS 构建全链路。
// 后续接回原 App（书架 / 封面 / 正文竖笺）时，在此恢复导航逻辑。

import SwiftUI

@main
struct JianDuApp: App {
    var body: some Scene {
        WindowGroup {
            ProbeView()
        }
    }
}
