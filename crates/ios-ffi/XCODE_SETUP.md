# 笺读 · iOS Xcode 接入指南（傻瓜式，一步一步来）

本指南假设你已经在本地克隆了 `jiandu` 仓库，且 `feature/ipados-poc` 分支已就位。
需要的核心产物（都在 `crates/ios-ffi/` 下）：

| 文件 | 作用 | 放哪里 |
|---|---|---|
| `JianDuCore.xcframework` | 真静态库（device + simulator 双切片）+ C 桥接头 | 拖进工程，**Embed & Sign** |
| `bindings/jiandu_ios_ffi.swift` | uniffi 生成的 Swift 绑定（你调用的 API 都在这） | 拖进工程，**作为源码编译** |
| `bindings/jiandu_ios_ffiFFI.h` + `module.modulemap` | C 桥接头，**已在 xcframework 内部** | ❌ **不要单独拖**，否则重复 |

> ⚠️ 核心理念：xcframework 提供「原生符号 + C 模块 `jiandu_ios_ffiFFI`」；`.swift` 由你的 App 工程单独编译，二者合起来才完整。两套都加、且只加一次，就不会报错。

---

## 第 0 步：前置条件

- Xcode 15+（你有 27.1，足够）。
- 一个 Apple ID（免费即可跑**模拟器**；**真机** iPad 调试需付费开发者 ¥688/年，或临时用个人团队 7 天签名）。
- 终端确认产物在：`ls crates/ios-ffi/JianDuCore.xcframework` 应能看到。

---

## 第 1 步：新建 SwiftUI 工程

1. 打开 Xcode → **Create New Project** → 选 **iOS** 标签 → 选 **App** → Next。
2. Product Name：`JianDuPoC`（随便取）。
3. **Interface：SwiftUI**、**Language：Swift**、Storage：None。
4. 选个保存位置 → Create。

> 此时 Xcode 左侧「Project Navigator」（⌘+1）能看到你的 `JianDuPoC` 工程结构。

---

## 第 2 步：把 JianDuCore.xcframework 拖进工程（回答问题 1）

1. 打开 **Finder**，进入 `crates/ios-ffi/`，找到 `JianDuCore.xcframework`。
2. 把它**拖到 Xcode 左侧 Project Navigator** 里（拖到你的工程根节点，和 `JianDuPoC` 文件夹同级或之内都行）。
3. 弹出对话框，**两个勾都打上**：
   - ✅ Copy items if needed
   - ✅ Add to targets：勾选你的 App（如 `JianDuPoC`）
   - 点 **Finish**。
4. 选中左侧蓝色工程图标 → **TARGETS** → 你的 App → 切到 **Frameworks, Libraries, and Embedded Content** 这个标签页。
5. 你会看到 `JianDuCore.xcframework` 已在列表里。把它右侧的 **Embed** 下拉设为 **Embed & Sign**。

✅ 到此，C 模块 `jiandu_ios_ffiFFI` 已可用。`.h` / `module.modulemap` 已包含在 xcframework 内部的 `Headers/`，**不要再单独拖这两个文件**。

---

## 第 3 步：把 jiandu_ios_ffi.swift 加进工程（回答问题 2）

1. 回到 Finder 的 `crates/ios-ffi/bindings/`，找到 `jiandu_ios_ffi.swift`。
2. 把它**拖到 Xcode 左侧 Project Navigator** 里（拖到你的源码组，比如 `JianDuPoC` 组下）。
3. 对话框同样：**✅ Copy items if needed**、**✅ Add to targets：你的 App** → Finish。

⚠️ 只加这一个 `.swift`。**不要**加 `jiandu_ios_ffiFFI.h`、`module.modulemap`（它们已在 xcframework 里，加了反而重复）。

### 如果报错了，怎么救

| 报错 | 原因 | 解决 |
|---|---|---|
| `Cannot find module 'jiandu_ios_ffiFFI'` | xcframework 没加进 target / 没 Embed | 回第 2 步第 4–5 点，确认在 Frameworks 列表且 Embed & Sign |
| 一堆 `Cannot find 'layoutProbe'` / 符号未定义 | `.swift` 没加进 target | 回第 3 步，确认 `jiandu_ios_ffi.swift` 在 Compile Sources |
| `Building for iOS Simulator, but linked framework was built for iOS` | 只拿了真机切片 | 用我们给的 xcframework（双切片）；或重跑 `bash build-ios.sh` |
| 真机签名报错 | 免费账号不能永久签真机 | Signing & Capabilities → Team 选你的 Apple ID（个人团队，7 天有效）或付费团队 |

---

## 第 4 步：最小测试代码（回答问题 3）

> 说明：Swift 这边**没有直接叫 `calculate_grid` 的函数**——它是 Rust 内部函数。我们暴露的 `layoutProbe(text:columns:)` 内部**就是调用真实的 `calculate_grid`**（竖排网格度量），返回每字的 `GlyphBoxFfi`。所以「调用 Rust 的 calculate_grid」= Swift 里调 `layoutProbe`。

1. 在 Project Navigator 里打开 `ContentView.swift`。
2. **整文件替换为下面这段代码**（含一个 `TextEditor` + 一个 `Button`，点按钮调 `layoutProbe` 并打印到控制台）。

```swift
import SwiftUI

struct ContentView: View {
    @State private var inputText: String = "天地玄黄宇宙洪荒日月盈昃辰宿列张"
    @State private var status: String = "输入文本后点按钮"

    var body: some View {
        VStack(spacing: 16) {
            // 显示 Rust 版本，验证桥接通了
            Text("笺读版本：\(jianduVersion())")
                .font(.headline)

            // 文本输入框
            TextEditor(text: $inputText)
                .border(Color.gray)
                .frame(height: 120)

            // 按钮：调用 Rust 排版（layoutProbe → calculate_grid）
            Button("调用 Rust 排版 (calculate_grid)") {
                let boxes = layoutProbe(text: inputText, columns: 12)
                // boxes 是 [GlyphBoxFfi]，字段：ch, x, y, w, h
                print("=== GlyphBoxFfi 共 \(boxes.count) 个 ===")
                for b in boxes {
                    print("字「\(b.ch)」 x=\(b.x) y=\(b.y) w=\(b.w) h=\(b.h)")
                }
                status = "已在控制台打印 \(boxes.count) 个 GlyphBoxFfi"
            }
            .padding()
            .background(Color.blue)
            .foregroundColor(.white)
            .cornerRadius(8)

            Text(status)
                .font(.caption)
        }
        .padding()
    }
}
```

---

## 第 5 步：运行 + 看控制台

1. 顶部运行目标栏选一个 **模拟器**（如 iPad Pro 13-inch）；首次可能提示 **Signing**：去 **Signing & Capabilities** → Team → 选/添加你的 Apple ID（免费即可模拟器）。
2. 点 **▶ Run**（⌘R）。
3. App 起来后，在文本框里改几个字，点蓝色按钮 **「调用 Rust 排版」**。
4. 打开 Xcode **控制台**（⌘⇧C，或底部调试区），应看到类似：

```
=== GlyphBoxFfi 共 20 个 ===
字「天」 x=... y=... w=... h=...
字「地」 x=... y=... w=... h=...
...
```

看到这串输出 = **uniffi 桥接 + Rust 真实排版 + iOS 构建全链路打通** 🎉

---

## 下一步

- 把 `GlyphBoxFfi` 的 `x/y/w/h` 画到 Canvas（见 `README.md` §2 的竖排网格渲染）。
- Apple Pencil 红线对齐测试（见 `README.md` §3）。
- 接 PDF 导入（PDFKit 同源，复用 `pdf_text.rs` 的 `walk_column` 几何）。

重新生成产物（若改了 Rust）：在 `crates/ios-ffi/` 跑 `bash build-ios.sh`，再回 Xcode ⌘R。
