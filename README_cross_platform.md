# 笺读 · 跨平台架构指南

> v1.0.0-beta 起，笺读从「一个 macOS App」改成「一个内核 + 若干壳子」。
> 这篇讲清楚：**内核在哪、壳子怎么接、Windows 与移动端最快怎么跑起来**。
>
> 读者假设：你已经能把 macOS 版跑起来（`cargo run -p jiandu`）。

---

## 0. 三句话版

1. **内核 = `crates/core`**：纯 Rust，零 UI。排版（`gloss` / `grid`）、解析、SQLite、FTS5、注音、AI 提示词组装全在里面。
2. **壳子 = 谁会画**：现在是 `crates/app`（GPUI + CoreGraphics，仅 macOS）。将来加 `crates/desktop`（Tauri，Win/Linux）与移动端。
3. **两者之间靠 trait 说话**：`PdfExporter` / `InputSink` / `PinyinSource`（都在 `crates/core/src/platform.rs`）。core 说「要什么」，壳子说「怎么画」。

一句话原则：**换壳子不换内核**。排出来的 `Page` 是全平台同一份。

---

## 1. 现在长什么样

```text
jiandu/
├── Cargo.toml              ← 虚拟 workspace（只管成员 / 版本 / profile）
├── crates/
│   ├── core/               ← jiandu-core：纯 Rust 内核
│   │   ├── Cargo.toml      crate-type = [lib, cdylib, staticlib]
│   │   └── src/
│   │       ├── gloss.rs    双行夹注排版引擎（面→列→片）   ← v1.0.0-beta 从 app 迁入
│   │       ├── grid.rs     网格几何（天头地脚 3:2、行款矩阵）
│   │       ├── hit.rs      命中测试（字/片包围盒）         ← v1.0.0-beta 从 app 迁入
│   │       ├── parsers.rs / vclean.rs / cleaner.rs / pipeline.rs / importer.rs
│   │       ├── store.rs / stream.rs / search.rs(FTS5)
│   │       ├── dict.rs / common_chars.rs / trad.rs / ruby.rs
│   │       ├── ai.rs（提示词组装 / SSE 解析，不发网络请求）
│   │       ├── gate.rs     内测邀请码
│   │       ├── platform.rs ← **跨平台接缝**（三个 trait）
│   │       ├── ffi.rs      ← **C ABI 出口**
│   │       └── resources/  dict.db / *.json / books
│   └── app/                ← jiandu（bin + lib）：macOS 原生壳子
│       └── src/  gpui / pdf.rs(CoreGraphics) / ime.rs / clipboard.rs / sound.rs …
├── scripts/
│   ├── make-mac-app.sh     打包（ARCH=universal DMG=1）
│   └── check_core_purity.sh ← **core 纯洁性守卫**
└── README_cross_platform.md ← 本文件
```

依赖方向**只能单向**：

```text
   crates/app   ──依赖──▶   crates/core
   crates/desktop(将来) ──依赖──▶   crates/core
   crates/core  ──依赖──▶   （只依赖纯 Rust：serde / rusqlite / zip / pinyin…）
```

---

## 2. core 的规矩

### 里面有什么

| 能力 | 模块 | 备注 |
| --- | --- | --- |
| 双行夹注排版 | `gloss` | 产出 `Page { glyphs, columns: Vec<Vec<Piece>> }`，壳子只管画 |
| 网格几何 | `grid` | `calculate_grid(GridInput)` 唯一入口 |
| 命中测试 | `hit` | `Page` → 包围盒，点选 / 卡片定位 / 注音落点共用 |
| 解析与清洗 | `parsers` / `vclean` / `cleaner` / `pipeline` / `importer` | EPUB / TXT / MD / HTML / JSON |
| 落盘与检索 | `store` / `stream` / `search` | SQLite + FTS5(trigram) |
| 字书 / 注音 | `dict` / `common_chars` / `ruby` / `trad` | 含自动注音判据 |
| AI 纯逻辑 | `ai` | 提示词组装 / SSE 增量解析；**HTTP 在壳子** |
| 内测闸门 | `gate` | 纯本地 |
| 平台接缝 | `platform` | 三个 trait |
| C ABI | `ffi` | 给非 Rust 壳子 |

### 里面**不许**有什么

- 任何 `use gpui` / `use cocoa` / `use objc` / `use core_graphics` / `use core_text` / `use rfd` / `use image`
- 任何 `#[cfg(target_os = …)]` 分支——要平台差异就抽 trait
- 任何「画面」概念（颜色是 `u32` 数据，字体是**语义类** `PdfFontKind`，不是具体字体）

有机器守着：

```bash
./scripts/check_core_purity.sh     # 违规 exit 1
# 建议挂 pre-commit：
ln -s ../../scripts/check_core_purity.sh .git/hooks/pre-commit
```

---

## 3. 三条接缝（trait）

都在 `crates/core/src/platform.rs`。

### 3.1 `PdfExporter` —— 印 PDF

```rust
pub trait PdfExporter {
    fn name(&self) -> &'static str;
    fn render_leaves(&self, leaves: &[LeafSpec<'_>], geo: &PdfGeometry)
        -> anyhow::Result<Vec<u8>>;
}
```

- `LeafSpec` = 一叶（持 `&gloss::Page`、章题、叶码、注音表）；`PdfGeometry` = 版式与配色（纯数据）。
- **实现者不许自己分页**：`Page` 已经排好了，抢过来排就会出现「屏幕上一套、纸上一套」。
- macOS 实现：`crates/app/src/pdf.rs` 的 `CoreGraphicsPdfExporter`（`#[cfg(target_os = "macos")]`），矢量、`CGPDFContext`、真字内嵌。
- 分派函数 `crate::pdf::exporter()`：非 macOS 时退回 `UnavailablePdfExporter`——**明确报错**，而不是给用户一张空白 PDF。

**接一个纯 Rust 后端（Windows / Linux）**：

```toml
# crates/desktop/Cargo.toml
[dependencies]
printpdf = "0.7"     # 或 pdf-writer = "0.10"
```

```rust
// crates/desktop/src/pdf_printpdf.rs
pub struct PrintPdfExporter;

impl jiandu_core::platform::PdfExporter for PrintPdfExporter {
    fn name(&self) -> &'static str { "printpdf" }

    fn render_leaves(
        &self,
        leaves: &[jiandu_core::platform::LeafSpec<'_>],
        geo: &jiandu_core::platform::PdfGeometry,
    ) -> anyhow::Result<Vec<u8>> {
        // 1. 纸：A4 竖放（与 macOS 版同一尺寸口径）
        // 2. 逐叶 → 逐列 → 逐 `Piece`：
        //      Piece::Main(g)  → 一个整字格
        //      Piece::Gloss(s, src, host) → 双行小字，各占半格（先右列后左列）
        //      Piece::Gap      → 空一格
        //    坐标换算见 crates/app/src/pdf.rs 的 walk_column（那份是 CG 版，
        //    几何一致，只换落笔 API）。
        // 3. ⚠️ 中文必须**嵌入 CJK 字体子集**（printpdf 支持内嵌 TTF），
        //    否则用户打开是一堆空白或方框——这是纯 Rust 路线唯一的硬骨头。
        // 4. 注音：`jiandu_core::ruby::layout(py, fs, cell_w, cell_h)`
        //    给的是同一套度量，照抄即可与屏幕逐字同位。
        todo!()
    }
}
```

> rome 没建，路通了。真要落地，主要工作量在**字体内嵌**与**竖排字形定位**，
> 排版那半个字都不用重写——那才是这套抽象值钱的地方。

### 3.2 `InputSink` —— 输入法

```rust
pub trait InputSink {
    fn insert_text(&mut self, text: &str);   // 已上屏
    fn set_marked(&mut self, text: &str);    // 组字中（拼音/候选）
    fn clear_marked(&mut self);              // 收工
}
```

中文输入法是各家 OS 最深的方言（macOS `NSTextInputClient`、Windows IMM32/TSF、
安卓 `InputConnection`），但**最后都归结成这三件事**。core 侧只要会喊这三句。

macOS 壳子已在 `crates/app/src/ime.rs` 实现（`impl InputSink for Root`），
覆盖重命名框 / 笔记框 / 书架搜索 / 阅读页检索四路。问小笺的草稿住在另一个
Entity 里（动它要 `Window`/`Context`），仍走 GPUI 的 `EntityInputHandler`。

### 3.3 `PinyinSource` —— 注音

```rust
pub trait PinyinSource {
    fn pinyin_of(&self, c: char) -> Option<String>;
    fn is_common(&self, c: char) -> bool;
}
```

`BuiltinPinyin` 是现在的默认实现（内置字书 → 常用字不注 → `pinyin` crate 兜底）。
将来要接多音字消歧模型或用户自建读音表，换一个实现即可，渲染层不动。

---

## 4. 路线 A：Tauri 2.0 Web 壳（**推荐**，Windows / Linux / 移动端的快路）

### 4.1 为什么是 Tauri

- Rust 端直接 `use jiandu_core::…`，**没有 FFI 编组成本**，也没有「两套数据模型」；
- WebView2（Win）/ WKWebView（iOS）/ WebView（Android）都是系统自带，包体只有十几 MB；
- 前端画竖排有现成的 CSS：`writing-mode: vertical-rl`，比手写 Canvas 省事得多。

### 4.2 建第三个成员

```bash
cd jiandu
npm create tauri-app@latest            # 或手动建 crates/desktop
```

把根 `Cargo.toml` 的成员加上：

```toml
[workspace]
members = ["crates/core", "crates/app", "crates/desktop"]
resolver = "2"
```

`crates/desktop/Cargo.toml`（Tauri 端）：

```toml
[package]
name = "jiandu-desktop"
version.workspace = true
edition.workspace = true

[dependencies]
jiandu-core = { path = "../core" }
tauri = { version = "2", features = [] }
tauri-plugin-dialog = "2"
tauri-plugin-fs = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

> ⚠️ 别把 `crates/app` 和 `crates/desktop` 放进同一个二进制：
> `app` 依赖 GPUI 与 CoreGraphics，在 Windows 上编不过。它们是**两个壳子**，
> 共用同一个 `core`。

建议加一条 feature 门，避免 macOS 机器上误编 Windows 壳子：

```toml
[features]
default = []
desktop = []
```

### 4.3 Rust 端命令（IPC）

`crates/desktop/src/lib.rs`：

```rust
use jiandu_core::{gloss, grid, parsers, pipeline, store};
use serde::Serialize;

/// 一叶的「绘制指令」：前端拿它直接摆 DOM，不必懂排版。
#[derive(Serialize)]
struct CellDto {
    ch: String,
    col: usize,
    row: usize,
    kind: &'static str,      // "main" | "gloss" | "gap"
    /// 注音（贴该字右上角），没有则 null
    ruby: Option<String>,
}

#[derive(Serialize)]
struct LeafDto {
    cols: usize,
    rows: usize,
    cell_w: f32,
    cell_h: f32,
    fs: f32,
    cells: Vec<CellDto>,
}

/// 排一叶：把 `store_id` 这本书第 `ci` 章的第 `leaf` 叶排出来。
#[tauri::command]
fn layout_leaf(store_id: String, ci: usize, leaf: usize, cols: usize) -> Result<LeafDto, String> {
    // ① 取章（按需加载，百万字丛书也不会整本进内存）
    let ch = store::chapter_at(&store_id, ci).map_err(|e| e.to_string())?;

    // ② 正文字流（`Glyph` 序列）：与 macOS 版同一条管线
    let flow = ch.to_flow(true);

    // ③ 试算网格：A4（或窗口）尺寸 → 列宽 / 行高 / 字号
    let m = grid::calculate_grid(grid::GridInput {
        content_w: 900.0, content_h: 1400.0,
        cols,
        ..Default::default()
    });

    // ④ 排版：**与 macOS 版同一个函数**
    let pages = gloss::layout_book_from_opts(
        &flow,
        |_| None,                       // 夹注源
        m.columns, m.rows, 0,           // 列数 / 行数 / 章首预留
        true,                           // west_mode
        true,
    );
    let page = pages.get(leaf).ok_or("没有这一叶")?;

    // ⑤ 摊平成绘制指令（列 → 片 → 字）
    let mut cells = Vec::new();
    for (col_i, col) in page.columns.iter().enumerate() {
        let mut row = 0usize;
        for piece in col {
            match piece {
                gloss::Piece::Main(g) => {
                    cells.push(CellDto {
                        ch: g.ch.to_string(), col: col_i, row,
                        kind: "main",
                        ruby: jiandu_core::dict::auto_pinyin(g.ch),
                    });
                    row += 1;
                }
                gloss::Piece::Gloss(s, _src, _host) => {
                    for c in s.chars() {
                        cells.push(CellDto {
                            ch: c.to_string(), col: col_i, row,
                            kind: "gloss", ruby: None,
                        });
                    }
                    row += 1;
                }
                gloss::Piece::Gap => { row += 1; }
            }
        }
    }
    Ok(LeafDto { cols: m.columns, rows: m.rows, cell_w: m.col_w, cell_h: m.row_h, fs: m.fs, cells })
}

/// 导入一本书（EPUB / TXT / MD / HTML / JSON 全走同一条漏斗）。
#[tauri::command]
fn import_book(path: String) -> Result<String, String> {
    let txt = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let book = pipeline::parse_str(&txt, &path).map_err(|e| e.to_string())?;
    let id = store::insert_book(&book).map_err(|e| e.to_string())?;
    Ok(id)
}

/// 全文检索（FTS5，按需建索引）。
#[tauri::command]
fn search(store_id: String, q: String) -> Result<Vec<(usize, String)>, String> {
    jiandu_core::search::run_search(&store_id, &q).map_err(|e| e.to_string())
}

/// 印 PDF：本平台有后端就印，没有就明确报错。
#[tauri::command]
fn export_pdf(store_id: String, ci: usize) -> Result<Vec<u8>, String> {
    // 见 3.1：Windows 上把这里换成 `PrintPdfExporter`
    let exporter: Box<dyn jiandu_core::platform::PdfExporter> =
        Box::new(PrintPdfExporter);
    // … 组好 `LeafSpec` / `PdfGeometry` 后调 `render_leaves`
    Err("尚未接线".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            layout_leaf, import_book, search, export_pdf
        ])
        .run(tauri::generate_context!())
        .expect("启动失败");
}
```

### 4.4 前端怎么调

```ts
import { invoke } from '@tauri-apps/api/core';

const leaf = await invoke<LeafDto>('layout_leaf', {
  storeId, ci: 0, leaf: 0, cols: 12,
});
```

### 4.5 前端怎么画（竖排的关键四行 CSS）

```css
.page {
  writing-mode: vertical-rl;   /* 竖排、从右往左 */
  text-orientation: upright;   /* 拉丁字母也立起来（注音需要） */
  font-family: "Songti SC", "SimSun", serif;
  display: grid;
  grid-template-rows: repeat(var(--rows), var(--cell-h));
  grid-auto-flow: column;
}
.gloss { font-size: 50%; }     /* 双行夹注：正文一半 */
```

注音用绝对定位摆到字格右上角（坐标由 `jiandu_core::ruby::layout` 给，
与 macOS 版同一套度量）：

```css
.ruby {
  position: absolute;
  right: 0; top: 0;
  writing-mode: vertical-rl;
  text-orientation: upright;
  font-size: 30%;
  color: var(--note-ink);
}
```

### 4.6 移动端额外注意

- **数据目录**：`jiandu_core::library::Library::home()` 走 `dirs`，iOS/Android 上会落到各自沙盒，不用改代码。
- **字体**：CJK 字体几十 MB，别打进包。学 macOS 版的做法——留一个「拖字体进来」的目录（`Library::fonts_dir()`），主字体缺字时回退。
- **PDF**：iOS 可以直接用 `UIGraphicsPDFRenderer`（Swift 侧）或纯 Rust 后端；Android 用 `PdfDocument`。都实现 `PdfExporter` 即可。
- **IME**：安卓的 `InputConnection` 天然就是 `InputSink` 的三件事，接起来比 macOS 那套 NSTextInputClient 省心。

---

## 5. 路线 B：FFI（编译成动态库）

### 5.1 什么时候走 FFI，什么时候走 IPC

| 场景 | 建议 |
| --- | --- |
| 壳子是 Rust（Tauri） | **直接 `use jiandu_core`，走 IPC**。零编组成本，最快 |
| 壳子是 Flutter / SwiftUI / Compose / Electron(非 Rust) | 走 **FFI**（`cdylib`） |
| 要搬的是整本书 / 检索结果 / 排版树 | **别用 FFI 逐个搬结构体**，做成「JSON 进 JSON 出」的一条命令 |
| 只是查一个字的拼音 / 释义 | FFI 很合适（小进小出） |

### 5.2 编译

```bash
cargo build -p jiandu-core --release
```

| 平台 | 产物 |
| --- | --- |
| macOS | `target/release/libjiandu_core.dylib` |
| Windows | `target/release/jiandu_core.dll` |
| Linux | `target/release/libjiandu_core.so` |
| iOS/Android | `cargo build --target aarch64-apple-ios` → 静态库（`staticlib`）链进宿主 App |

### 5.3 现成的 C ABI（`crates/core/src/ffi.rs`）

```c
/* 版本 / 错误 */
char*  jiandu_version(void);
char*  jiandu_last_error(void);
void   jiandu_string_free(char* s);

/* 注音 */
char*  jiandu_auto_pinyin(uint32_t codepoint);   /* 不注或查不到 → NULL */
uint8_t jiandu_is_common_char(uint32_t codepoint);

/* 字书 */
char*  jiandu_define(const char* word);          /* 无 → NULL */

/* 内测闸门 */
uint8_t jiandu_gate_check(const char* code);
```

### 5.4 调用示例

**Dart（Flutter）**

```dart
typedef _Pinyin = Pointer<Utf8> Function(int cp);
final pinyin = dylib
    .lookupFunction<_Pinyin, Pointer<Utf8> Function(int)>('jiandu_auto_pinyin');
final p = pinyin(0x57F5);            // 埵
if (p != nullptr) {
  print(p.toDartString());           // duǒ
  jianduStringFree(p);               // ★ 必须释放
}
```

**Swift**

```swift
if let p = jiandu_auto_pinyin(0x57F5) {
    let s = String(cString: p)       // "duǒ"
    jiandu_string_free(p)            // ★ 必须释放
}
```

**Node（koffi / ffi-napi）**

```js
const lib = koffi.load('target/release/libjiandu_core.dylib');
const pinyin = lib.func('char* jiandu_auto_pinyin(uint32_t cp)');
const free   = lib.func('void jiandu_string_free(char* s)');
const p = pinyin(0x57F5);
if (p) { console.log(p); free(p); }
```

### 5.5 内存约定（写错就是泄漏或崩溃）

1. 所有返回 `char*` 的字符串**必须**用 `jiandu_string_free` 释放——是本 crate 用 `CString::into_raw` 交出去的，别人 free 不了。
2. 返回 `NULL` = 「无此结果」，不是错误；真错误看 `jiandu_last_error()`。
3. 传入指针为 `NULL` 是安全的（不会 deref，只记一条错误）。
4. `ffi.rs` 里**不做** UI、不弹窗、不读剪贴板，可放心在后台线程调。

---

## 6. 迁移清单

### Windows（Tauri 壳）

- [ ] 建 `crates/desktop`，加进 workspace members
- [ ] 实现 `PdfExporter`（printpdf / pdf-writer），重点是**CJK 字体内嵌**
- [ ] WebView2 运行时（Tauri 会带；离线安装包需单独引导）
- [ ] 字体：不要打包大字库，用 `Library::fonts_dir()` 做用户自备入口
- [ ] 数据目录：`dirs` 已覆盖（`%APPDATA%`）

### iOS / Android

- [ ] `crate-type = ["staticlib"]` 已就位；用 `cargo build --target aarch64-apple-ios` / `aarch64-linux-android`
- [ ] Tauri mobile（`tauri android init` / `tauri ios init`）或各自原生壳
- [ ] 触摸交互：点选 / 长按选区要走 `hit::column_spans`（core 已提供）
- [ ] 沙盒：SQLite 落 `Library::home()`（已走 `dirs`，无需改）

### 通用

- [ ] 每次改动后跑 `./scripts/check_core_purity.sh`
- [ ] 排版相关改动后跑 `cargo test --workspace`（`gloss` / `hit` / `grid` 有几百条几何单测钉着）

---

## 7. 已知限制

1. **`crates/app` 仍然是 macOS-only**：它依赖 GPUI 与 CoreGraphics。这不是没做完，是设计如此——GPUI 壳就是 macOS 原生壳。跨平台走**新壳子**，不是把老壳子改到能跑 Windows。
2. **纯 Rust PDF 后端还没接**：trait 与分派都在，实现留白。主要工作量在 CJK 字体内嵌。
3. **问小笺的 HTTP 在壳子侧**：core 只做提示词组装与 SSE 解析（`core/src/ai.rs`）。跨平台壳子要自带一个 HTTP 客户端（`reqwest` 或 Tauri 的 `fetch`）。
4. **core 里没有 `#[cfg(target_os)]`**：这是有意的（纯洁性脚本会拦）。真需要平台差异就加 trait。
5. **macOS 版的中文输入法仍有一处未修**（检索框 / 问小笺），与本次重构无关，见 `docs/` 里的排查记录：根因在 GPUI 平台层那个 view 不是 firstResponder，应用侧 `InputHandler` 本身是好的。
