# 技术备忘录（Tech Note）｜ macOS Vision 竖排 CJK 盲区 与 PaddleOCR V2.5 独立插件架构

**编号**：TN-2026-11-08-Vision-Vertical-CJK
**日期**：2026-11-08
**作者**：笺读工程
**状态**：已确认（Decision Made）
**分支**：v2.0-dev ｜ 关联 L4 提交 `1520caa` / `f5c160d`

---

## 0. 结论先行（TL;DR）
1. **底层坐标管道已验证健康** —— 真实横排现代中文 98 字、oob=0、阅读顺序正确、左上角原点正确。工程能力"毕业"。
2. **macOS Vision 的 accurate 识别器对「竖排 CJK」是彻头彻尾的瞎子**：4 张真实古籍竖排繁体书页（0/90/180/270° 全向）均返回 0 字。模型只认横排行向特征，整图旋转无法同时让「字正 + 行横」，死胡同。
3. **决策**：立即停止在 Vision 竖排方向投入。**提前启动 V2.5 PaddleOCR 独立插件计划**，专攻竖排古籍/繁体，保持主工程零模型、轻量。
4. PaddleOCR 作为**独立按需下载插件**（不并入主工程），通过既有的 `OcrProvider` trait + `OcrResult` JSON 契约接入，主程序零改动即可透明切换引擎。

---

## 1. 坑位记录：Vision 竖排 CJK 盲区

### 1.1 现象
- 输入：4 张 WeChat 文件助手传的书页 JPEG（~1.3MB/张，EXIF orientation=nil），内容为**传统竖排繁体中文**（纵列、右起左行）。
- 结果：Vision `VNRecognizeTextRequest`（accurate 模式）返回 **0 字形**，text 为空，oob=0。
- 排除法：同一二进制对合成横排图稳定出 19 字、对真实横排现代中文出 98 字 → **引擎未坏，是模型「看不懂」竖排**。

### 1.2 穷举验证（证明是死胡同）
| 朝向 | 结果 |
|---|---|
| 原图（0°） | 0 字 |
| 旋 90°（CW） | 0 字 |
| 旋 180° | 0 字 |
| 旋 270°（CCW） | 0 字 |

根本原因：Vision accurate recognizer 的特征是**横排行向**训练的。对竖排书页：旋到「字正」→ 行变竖，模型不认；旋到「行横」→ 字倒，模型也不认。**整图旋转永远无法同时得到「字正 + 行横」**，故四种朝向全 0。

### 1.3 老系统限制说明
在老版本 macOS SDK 下，Vision **没有竖排/纵向文本识别模式**，也没有面向 CJK 竖排的版面分析。纵列文字即便靠「按列切片→旋转→识别→坐标反变换」的 workaround，切片后列内仍是竖排语义，模型依旧不认。故：**在老系统上，Vision 对竖排古籍「不可用」，不是调参能解决的。**

> 注：即便升级到较新 macOS，Vision 也仅提供有限的文本方向分类（±90° 纠偏），并非真正的竖排多列版面 OCR。对「笺读」核心业务（古籍/繁体竖排）而言，Vision 始终不是答案。

---

## 2. 正向验证：横排现代中文（工程能力毕业）

### 2.1 测试样本
- 生成真实横排现代中文段落（STHeiti Medium，1000×520，4 行，含中文/拉丁/数字混排）：
  `笺读（JianDu）是一款面向古籍与繁体竖排文献的校对工具。` 等。
- 命令：`./target/release/jiandu-ocr vision /tmp/real_han.png -o real_han.json --overlay real_han._overlay.png`

### 2.2 结果
| 指标 | 值 | 判定 |
|---|---|---|
| 字形数 | 98 | ✓ 与可见字符数一致 |
| oob（越界） | 0 | ✓ Y 翻转正确、坐标在帧内 |
| 阅读顺序 | 正确（首行 y=52 在上，末行 y=330 在下） | ✓ 左上角原点正确 |
| text 还原 | 完整（仅空格/标点归一化，符合 AlignmentStore 契约） | ✓ |
| 拉丁/数字 | 识别并定位（"JianDu"/"Version"/"2026" 均命中） | ✓ |

### 2.3 已知质量细节（非阻塞，记入待办）
- **per-glyph 框粒度**：本机 `elements()` 可用；单字元素给紧框，但 mixed 行中部分元素跨多字（token 级），此时按字符数均分宽度 → 框偏松/邻框重叠。属质量细节，不破正确性（顺序、oob 均正确）。
- **confidence 字段**：Vision accurate 模式只给候选/行级置信，`msg_send![cand, confidence]` 在本机恒返 0.5，已逐字复制。该值仅作占位；后续 PaddleOCR 插件将给出真实逐字置信。

---

## 3. 战略决策
- **放弃**在 Vision 上继续攻克竖排（ROI 为负，且违反「笺读核心业务 = 古籍竖排」）。
- **保留** Vision 作为 macOS 横排/现代中文的零成本离线引擎（走 Neural Engine，无模型文件）。
- **启动** V2.5 PaddleOCR 独立插件，专攻竖排古籍/繁体，并补齐跨平台（Windows/Linux 经同一插件）。

---

## 4. PaddleOCR 作为独立插件接入 Rust —— 架构方案

### 4.1 设计原则（硬约束）
1. **主工程轻量、零模型**：400MB+ 模型与推理运行时**绝不进 App 包**。
2. **按需下载**：首次用到竖排能力时，从 CDN 拉取插件包到用户缓存目录（如 `~/Library/Application Support/jiandu/plugins/paddleocr/`，不在 app bundle 内）。
3. **契约复用**：插件输入输出严格遵守既有 `OcrProvider` trait 与 `OcrResult` JSON（`image_width/height, text, glyphs[{x,y,w,h,target,confidence}]`），主程序**零侵入**即可换引擎。
4. **故障隔离**：插件崩溃/缺模型不影响主程序；走超时 + 降级链。
5. **跨平台**：插件本身可编译/运行于 macOS/Windows/Linux，主程序不因插件引入平台耦合（契合「Windows 不碰主工程」——插件是外挂，非主工程代码）。

### 4.2 接入形态对比
| 方案 | 做法 | 优点 | 缺点 | 结论 |
|---|---|---|---|---|
| A. 子进程 + JSON IPC | 插件是独立可执行（或 Python 脚本），主程序 spawn 之，stdin/stdout 走 `OcrInput`/`OcrResult` JSON（或本地 Unix socket） | 隔离最好、零 Rust FFI 复杂度、下载即用、跨平台 | 进程启动开销（~百 ms，批量可长连接复用） | **推荐（V2.5 首选）** |
| B. FFI 直链 C++ lib | 编 PaddleOCR C++ 推理库 + cbindgen 头，Rust `extern "C"` 调 | 零进程开销、延迟最低 | 400MB 运行时需随包/随插件、构建极重、UB 风险高 | 备选（仅当延迟敏感且愿承担构建重量） |
| C. ONNX Runtime + Rust(`ort`) | 把 PP-OCR 模型转 ONNX，用 `ort`/`tract` 纯 Rust 推理 | 无 Python、纯 Rust、跨平台 | 仍需模型文件；转换/后处理（NMS、CTC 解码、版面）工作量不小 | 长期优选（去 Python 依赖） |

**推荐组合**：V2.5 先用 **方案 A（子进程 + JSON IPC）** 快速落地（直接复用官方 PaddleOCR Python 或编译好的 `paddleocr_runner`）；长期演进到 **方案 C（ONNX + Rust）** 去除 Python 运行时依赖，提升端到端稳定性。

### 4.3 推荐架构（方案 A 落地图）
```
┌─────────────────────────────────────────────────────────────┐
│ 主程序 jiandu (GPUI)  —— 仅依赖 core::OcrProvider trait       │
│   compare / collation 模块 通过 trait 调 OCR，不知背后引擎     │
└───────────────┬─────────────────────────────────────────────┘
                │  dyn OcrProvider
┌───────────────▼─────────────────────────────────────────────┐
│ core::ocr_provider (纯 Rust 契约，已存在)                     │
│   OcrProvider trait · OcrInput · OcrResult · AlignmentStore   │
└───────────────┬─────────────────────────────────────────────┘
        ┌───────┴────────┬───────────────────────┐
        ▼                ▼                        ▼
 VisionOcrProvider  SubprocessOcrProvider   (未来) CloudProvider
 (ocr-cli, macOS)   (host 端，V2.5 新增)      (可选兜底)
        │                │
        │                └──► spawn 插件可执行
        │                     ▼
        │                 ~/Library/.../plugins/paddleocr/
        │                   ├─ paddleocr.plugin.json (manifest: 版本/模型清单/能力/下载URL/校验和)
        │                   ├─ paddleocr_runner  (编译二进制 或 python -m paddleocr_server)
        │                   └─ models/  (det + rec + cls 模型，首次按需下载)
        │                     ▲
        │                     │ stdin: OcrInput JSON
        │                     │ stdout: OcrResult JSON
        └─────────────────────┴───────────────────────
```

### 4.4 插件接口契约（复用现有，仅追加 `options`）
请求（`OcrInput` 扩展，向后兼容）：
```json
{ "image": "<path|base64>",
  "options": { "direction": "vertical", "lang": ["ch_tra","en"], "detail": true } }
```
响应（严格 `OcrResult` / `AlignmentStore::to_compare_json` 形状）：
```json
{ "image_width":W,"image_height":H,"text":"…",
  "glyphs":[{"x":..,"y":..,"w":..,"h":..,"target":i,"confidence":0.98}, …] }
```
- 坐标原点：**左上角像素**（与 Vision 路径输出一致；若插件给归一化/左下角，由 host 端统一翻转到左上角像素，复用现有 Y 翻转逻辑）。
- `direction=vertical` 时，插件内部负责：列检测 → 逐列旋转 90° → 识别 → 坐标反变换回原图空间（**这正是 Vision 做不到、却必须由引擎侧完成的关键**）。

### 4.5 Host 端实现要点（`SubprocessOcrProvider`）
- 位置：新增 `crates/ocr-plugin`（独立 crate，不进 core、不进 app 平台依赖），或扩 `ocr-cli`。
- 行为：
  - 懒加载：首次调用检查 manifest + 模型是否存在，缺失则触发下载器（带进度、校验和、超时）。
  - 通信：批量场景保持**长连接**（插件进程常驻，多帧复用），避免每页 spawn 开销。
  - 超时与降级：单帧 >N 秒或进程退出码异常 → 返回错误，主程序降级到 Vision（若可用）或提示「竖排插件未就绪」。
  - 安全：下载包校验 SHA-256 + 签名；模型与二进制分离，可独立更新。

### 4.6 竖排古籍专项（PaddleOCR 能力映射）
| 笺读需求 | PaddleOCR 能力 | 备注 |
|---|---|---|
| 竖排繁体识别 | PP-OCRv5 / `ch_tra` 识别 + 方向分类 | 原生支持竖排/繁体，免手工切片 |
| 双列/多列版面 | PP-Structure / 版面分析 | 可一并输出栏框，喂 `AlignmentStore` |
| 夹注/批注分离 | 版面分析 + 位置聚类 | 复用 L1 三路分离思路 |
| 跨平台 | C++/ONNX 推理 | macOS/Windows/Linux 同一插件 |

### 4.7 演进路线（V2.5 里程碑）
1. **M1 插件骨架**：manifest + 下载器 + `SubprocessOcrProvider` + JSON IPC（先 Python runner 跑通竖排样例）。
2. **M2 竖排打通**：`direction=vertical` 端到端（真实古籍扫描 → 红框 overlay 验坐标）。
3. **M3 质量对齐**：逐字置信、版面分栏、与 Vision 横向引擎并存于降级链。
4. **M4 去 Python**：转 ONNX + Rust `ort`，去掉运行时依赖，固化跨平台交付。
5. **M5（可选）** B 端：插件市场/按需计费。

---

## 5. 对 L5 落库的影响
- **暂停 L5（AlignmentStore 持久化）**，直至竖排坐标质量经 PaddleOCR 插件验证达标（用户既定红线：底层坐标可靠前不落库）。
- 当前 L4 交付物（Vision 横排引擎 + 测试工具）保留为「横排/现代中文」通道，与 V2.5 竖排插件形成双引擎。

---

## 6. 风险与待确认
- 插件体积/下载体验（首启需拉模型，需 CDN 与进度 UI）。
- Python runner 的端用户环境（无 Python 时需编译二进制版 → 指向方案 C）。
- 竖排坐标反变换的精度（列检测误差会线性传导到逐字框，需在 M2 用 overlay 严格目检）。

---

## 7. 高优需求跟踪（PaddleOCR 竖排支持）

> **状态：高优先级 / P0（用户明确指定，2026-11-08）** —— 竖排古籍是「笺读」核心业务，而 Vision 已被证明对 CJK 竖排**系统级盲区**（见 §1），且会幻觉出错误字符（如日文假名 `まべ`），**绝不可用于竖排古籍**。因此 PaddleOCR 竖排支持不是「锦上添花」，而是 **V2.5 的硬门槛**。

- **为什么高优**：核心业务（古籍 / 繁体竖排）无法依赖 Vision；L4 盲测（竖排《世說新語》4 张全 0、1 张误识出日文假名）已彻底坐实这一点（用户刻意盲测验证）。
- **归属**：V2.5 **独立插件**（见 §4），按需下载、绝不并入主工程，保持主包零模型、轻量；跨平台（macOS/Windows/Linux 同插件）。
- **验收门槛**：真实古籍竖排扫描 → 红框 overlay 严格贴合逐字、oob=0、阅读顺序（右起左行、上→下）正确，且不再出现跨语种幻觉。
- **关联 TODO**：在 `TODO_V2.md` / `ROADMAP_V2.md` 中标为 P0；L5 落库（§5）完成后接入，避免在未验证引擎上盖仓库。

---

*本备忘录取代任何「继续在 Vision 上攻克竖排」的尝试；相关代码改动仅限 `ocr-cli`（独立 crate），core/app 纯洁红线未破。*
*测试证据见：`/tmp/jiandu_test_run/`（竖排 0 字）、`/tmp/real_han.json` + `/tmp/real_han._overlay.png`（横排 98 字 oob=0）。*
