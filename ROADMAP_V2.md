# ROADMAP_V2 — 向「看典古籍」借鉴，但不抄它的臃肿

> 评估稿 · 2026-11-02
> 状态：**仅技术评估 + 规划，未动手实现**。等拍板后按优先级落地。
> 风格红线：宣纸底色、毛玻璃、极简单字 —— 任何新界面都不得退化成竞品的「老气排版 + 一排按钮」。

---

## 0. 总原则：吸取什么 / 不抄什么

| 竞品核心能力（值得吸收） | 竞品臃肿处（坚决不抄） |
|---|---|
| 让古籍「零门槛可入库」：扫图即识别、即校对 | 顶部一排功能按钮（转简/转繁/校对/导出…）塞满工具栏 |
| 繁简一键切换，降低阅读门槛 | 弹窗套弹窗的设置面板，层层叠叠 |
| 多版本文本可比对、可校勘 | 仿 Windows 95 的灰边框 + 立体按钮 + 密排列表 |

我们的差异化护城河仍是「句读 + 排版 + 笺校联动」，**这三件新功能只是把「古籍入门」的摩擦削平，绝不新增视觉噪音**。

---

## 1. 一键 OCR 接入（通用云端 OCR 接口）

### 1.1 竞品借鉴 / 不抄
- **吸收**：用户选中一个纯图片 PDF → 后台自动拉起 OCR 跑完全书 → 右侧转录区直接被填满。把「OCR 工程」藏在「扫描识别」这一个动作背后。
- **不抄**：竞品把 OCR 设置、引擎选择、语言包做成三页对话框。我们要的是「选文件 → 喝茶 → 全文到手」。

### 1.2 技术评估（基于现有代码）
`core/src/ocr.rs` **已经把骨架搭好了**，只是个桩：
- `OcrQueue` / `OcrJob { id, path, name, kind, state, note }` / `OcrState { Pending, Running, Done, Failed }` 完整。
- `push_many` / `take_next`（取首个 Pending 置 Running）/ `mark_done` / `mark_failed` / `remove` / `clear` / `summary` 齐备。
- `OcrKind::{Pdf, Image}` 由扩展名判定（`kind_of`）。
- **缺口**：① 没有 `OcrProvider` 抽象，没有任何识别实现；② 消费循环（`take_next`→识别→`mark_done`）还没写；③ **`OcrJob` 目前只存 `note`，不存识别结果文本** —— 必须给 `OcrJob` 加 `result: Option<String>`（或另开 `HashMap<id,String>`）才能回填右侧。

### 1.3 接口设计：通用云端 OCR Provider
新增一个 provider trait，百度/腾讯/未来任何云都实现它即可插拔：

```rust
// core/src/ocr.rs（扩）
pub trait OcrProvider: Send + Sync {
    fn name(&self) -> &'static str;
    /// 识别一张位图（PNG/JPEG 字节），返回按阅读顺序拼接的文本。
    /// 在 bg.spawn 里跑，绝不在主线程阻塞。
    fn recognize(&self, image_png: &[u8]) -> anyhow::Result<String>;
}

pub struct BaiduOcr   { api_key: String, secret_key: String, token: Mutex<Option<(String, Instant)>> }
pub struct TencentOcr { secret_id: String, secret_key: String }
```

- 密钥走 `Prefs`（落盘）+ macOS Keychain（存 secret），不进 git。
- **隐私闸门（必做）**：首次调用云端前弹一张「图像将发送至 XX 云 OCR，是否继续？」的同意卡（宣纸风格，非系统灰窗）。古籍扫描件属用户私产，默认不静默上传。
- **限速/并发**：每页一次请求，并发 1–3，按 `OcrState::Running` 推进；失败件 `mark_failed` 不重试（UI 可手动重跑）。

### 1.4 实现路径（文件级）
1. `core/src/ocr.rs`：`OcrJob` 加 `result`；新增 `OcrProvider` trait + `BaiduOcr`/`TencentOcr` 两个实现（HTTP 调各自通用印刷体识别 API）。
2. 新增消费循环 `consume_ocr(cx)`：`take_next` → 用现有 CoreGraphics 栅格化路径（`render_pdf_blocking` / `request_pdf_page`）把 PDF 每一页出 PNG → `provider.recognize(page_png)` → 累加 → 全书完成把拼接文本写回 `collation.text`。
3. `app/src/collation.rs`：顶栏在「打开文件 / 导入 MD / 批量导入」后加一枚**弱化的「扫描识别」**入口（沿用 `ghost_button` 视觉，不抢「竖排预览」的主权重）；`ocr_panel` 浮层已存在，直接复用显示进度。
4. **自动填充右侧**：OCR 全书完成 → `self.text = joined` → 重置 `cursor/selection/scroll` → `cx.notify()`。左侧仍是原图 PDF，可继续「左图右文」校勘。

### 1.5 工作量
中。骨架已省掉 60% 工作量；剩下是「provider HTTP 适配 + 消费循环 + 回填」。百度/腾讯通用印刷体 API 都是标准 REST，单家约 1–2 天。

### 1.6 风险与约束
- **费用/账号模型要你拍板**：用户自带 key？还是我们代付走套餐？这决定 UI 上要不要有「购买 OCR 额度」。
- **离线兜底（可选）**：可绑 `tesseract` crate（chi_sim）做本地识别，质量差但零隐私顾虑、零费用；作为云端失败时的 fallback，不强制。
- **长书 OOM**：逐页栅格化 + 逐页识别（已有「只渲当前页」机制），不会整本入内存。

### 1.7 风格兼容
「扫描识别」沿用现有 `ghost_button`（无底无框、hover 转朱）；进度复用现有右下角 `ocr_panel` 毛玻璃小卡。**不**新增设置弹窗。

---

## 2. 极简繁简转换与校对面板

### 2.1 现状（关键结论：引擎已完成，纯 UI 缺口）
`core/src/trad.rs` **已经做完**转换引擎，不用重写：
- `TradMode { Original, Simplified, Traditional }` 三档。
- `trad::convert(text, mode)`：macOS 走 ICU `Hans-Hant`/`Hant-Hans` **词组级**变换（能正确把「皇后」留在「后」、不误转「後」）；引擎不在时退回内置逐字表，且**一简对多繁的歧义字（发/干/后/复…）一律跳过**，绝不改错方向。
- 所以这件不是「造引擎」，是「把引擎露出来」。

### 2.2 设计：不铺按钮排，做【字】面板 + 右键菜单
- **【字】面板（主入口）**：顶栏右上角一枚极简单字印（如篆/楷「字」），点开是一张**毛玻璃小卡**，里面只有 3 行极简单字：**原文 / 简 / 繁**。这是文档级「显示档」——非破坏、只做渲染滤镜（底本一个字不动，网格/分页/注音/检索全不受影响）。
- **右键菜单（选区级，加分项）**：在右侧编辑器选中一段 → 右键 → 「选区转简 / 选区转繁」，**就地改写 `self.text`**（把转换 bake 进工作文本，导出即是简/繁版）。复用 `trad::convert` 对选区内子串处理。
- 坚决**不**做竞品那种「横排一排转换按钮」。

### 2.3 实现路径
1. `TradMode` 已是 `Prefs` 字段，UI 只需把「原文/简/繁」三选一接到 `self.trad_mode` / 全局 prefs。
2. `collation.rs` 顶栏加一枚 `.seal_chip("字", …)` 风格的小印，展开毛玻璃卡（复用 `render_style_panel` 的卡片视觉语言，但内容压到 ≤3 项）。
3. 编辑器 `on_mouse_down` 选区成立后，挂右键（`on_mouse_down` Right 或平台右键）→ 弹毛玻璃菜单 → 调 `trad::convert` 改写选区。

### 2.4 工作量
低（最快落地的一项）。引擎零改动，UI 约 0.5–1 天。

### 2.5 台繁细节
`TradMode::Traditional` 现走 ICU `Hans→Hant`，约等于臺灣正體覆盖 ~95%（裡/裏、台/臺、著/着 等少数例外）。若你要的是**严格台灣正體標準**，可把引擎换成 OpenCC `s2tw`/`t2tw`（加一个 `OpenccEngine` 实现 `trad::Convert` trait 即可，不动 UI）。**默认先上 `Hans→Hant`，台繁作为可选引擎追加**，不阻塞。

### 2.6 风格兼容
【字】面板 = 现有毛玻璃卡 + 极简单字；右键菜单 = 同族小卡。**零边框、零灰底、零长标签**。

---

## 3. 文本 Diff（差异比对）

### 3.1 可行性结论
**完全可行，且对校勘价值高。** 当前是「左图右文」比对原图；导入两段纯文本（如《道德经》甲乙本）做**文本级 Diff**，是天然延伸。CJK 用**字符级** Diff（不是行级，因为段落会重排）。

### 3.2 技术方案
- 新模块 `core/src/diff.rs`：
  ```rust
  pub enum DiffOp { Equal(char), Insert(char), Delete(char) }
  pub fn diff_chars(base: &[char], other: &[char]) -> Vec<DiffOp>;
  ```
- **两阶段，控复杂度**：先按 `\n` 把两段切成段落对齐（结构相同，按下标配对；结构不同则模糊配对），再对**每个变动段落**做字符级 Myers（建议直接引 `similar` crate 的 Myers，成熟且快）。Equal 段落整段跳过，避免 8 万字全量 O(N·M)。
- 输出 ops：`Insert`（他本有、底本无）→ 绿；`Delete`（底本有、他本无）→ 红删除线；`Equal` → 正常墨。

### 3.3 UI：双文比对模式（沿用左/右隐喻）
- 在顶栏新增一个弱化的「双文比对」入口（与「竖排预览」平级，不抢主权重）。
- 开启后工作台从「左图右文」切到「左文(A 底本) 右文(B 他本)」，**沿用现有 40/60 左右分栏**，只是左栏从位图换成文本。
- 渲染：左栏把 `Delete` 段以**红删除线**标出；右栏把 `Insert` 段以**绿**标出；两栏**滚动同步**（长文不脱节）。
- 导入：一个「双文比对」动作连续取两份文本（MD/粘贴），A 为底本、B 为他本。

### 3.4 实现路径
1. `core/src/diff.rs`：两阶段 Diff + `DiffOp`。
2. `collation.rs`：新增一个 mode（`Compare`），`render_left` 在 Compare 模式下改渲文本（复用现有 `render_line` / 编辑器组件，只是去掉 IME 输入与句读联动）；Diff 高亮在 `render_line` 里按 ops 上色。
3. 滚动同步：监听任一栏 `scroll`，镜像到另一栏（按行比例）。

### 3.5 工作量
中。Diff 算法引 `similar` 约 1 天；UI 复用现有左右分栏与 `render_line`，约 1–2 天。

### 3.6 性能约束
字符级 Myers 在最坏情况（两段完全不同）是 O(N·M)。两阶段（段落先对齐）把每段压到小窗口，`similar` 的 Myers 实测对单段几千字游刃有余。**8 万字全书**靠「段落对齐 + 段内 Diff」即可不卡。

### 3.7 风格兼容
绿/红按你指定的语义（新增绿、删除红删除线）。**其余一律宣纸 + 朱砂批注语言**，不引入竞品的彩色高亮风暴——只此两色，且删除线用细红、新增用沉绿，不刺眼。

---

## 4. 优先级与拍板建议

| 顺位 | 功能 | 风险 | 工作量 | 建议 |
|---|---|---|---|---|
| 1 | **繁简面板** | 极低（引擎已就绪） | 低 | **先行**：纯 UI，直接碾压竞品按钮排 |
| 2 | **文本 Diff** | 低（算法现成） | 中 | 第二落，校勘刚需 |
| 3 | **一键 OCR** | 高（外部依赖/隐私/费用） | 中 | **需你定**：用哪家、key 谁出、是否代付 |

**待你拍板的三件事**：
1. OCR 提供商与计费模型（用户自带 key / 我们代付 / 仅离线 tesseract）。
2. 繁简是否要「严格台繁」（OpenCC）还是先用 ICU `Hans→Hant` 够用。
3. Diff 默认「双文并排同步」还是「单栏内联高亮」。

> 本稿仅评估与规划，未改动任何功能代码。
