//! 笺读 JianDu · 纯逻辑核心库（v1.47.0 起独立成 crate）。
//!
//! **零 GPUI 依赖**：
//!   * 网格排版引擎（`grid`：天宽地窄、文武边栏、鱼尾黑口的全部几何演算）
//!     与双行夹注排版（`gloss`：面 → 列 → 片）；
//!   * 双行夹注之外的版面数据模型（`types` / `content` / `ruby`）；
//!   * EPUB / TXT / MD / HTML 解析与清洗（`parsers` / `vclean` / `pipeline` /
//!     `importer` / `library`：通假、脚注、前言智能跳过、上标幽灵字清剿）；
//!   * 离线双引擎字典（`dict`：古汉语常用字 + 康熙/說文合并库）；
//!   * 简繁对照（`trad` / `west`）、数字转汉字（`numcn`）、版心几何（`banxin`）、
//!     纸墨主题数据（`theme`）、偏好（`prefs`）。
//!
//! UI 层（仓库根的 `jiandu` bin：`app` / `reader` / `shelf`）只消费这里的
//! 数据与计算结果，为将来移植 iOS（核心库原样复用、仅重写渲染层）做准备。
//!
//! ⚠️ 铁律：本 crate 里**不得**引入任何 GPUI / 平台 UI 类型。
#![allow(dead_code)]

// 排版函数按几何逐项传值（列宽 / 行高 / 天头 / 地脚…），硬拆成 struct
// 反而掩盖每一项的物理意义——与 bin 的 crate 级政策同一口径。
#![allow(clippy::too_many_arguments)]

pub mod banxin;
/// **古籍文本量化分析**（v1.81.0 · 模块五）：字词 / 句读 / 文风三栏的纯
/// 计算层。只数数、不分词、不改原文；高频词走 n-gram，文风走虚词密度。
pub mod analysis;
/// **质检 · 待办清单**（V2.1）：把清洗 / 异体 / 繁简那几套判据**再问一遍**，
/// 把「还能改动的地方」列成可点击条目（残留 / 符号 / 繁简 / 标点 / 重复 / 罕用）。
/// 零猜测、零 UI，纯函数，可直接序列化成书稿质检报告。
pub mod qc;
/// **批量导入**（v1.77.0）：点检 → 串行刻印 → 可取消 → 错误日志。
pub mod batch;
/// **批量识别队列**（v1.9.25）：一次选中的多个 PDF / 图片排队等 OCR。
/// 纯队列状态机，零引擎依赖——接 API 时只写消费循环，UI 不动。
pub mod ocr;
/// **L4 对齐层 · OCR 提供者契约**（V2.0）：`OcrProvider` trait + `OcrResult` /
/// `OcrGlyph` + `MockOcrProvider` + `AlignmentStore`（把 OCR 结果翻译成与
/// `app.rs::load_compare_json` 逐字节兼容的对照 JSON，并支持正 / 反向坐标查询）。
/// 纯内核、零平台绑定，真实引擎（Vision / PaddleOCR）放独立 crate `impl` 即可。
pub mod ocr_provider;
/// **问小笺**（v1.76.0 · V2.0 模块二）：AI 伴读的纯逻辑层——服务商预设 /
/// 提示词组装 / SSE 增量解析。零网络依赖（HTTP 在 UI 层 `src/ai.rs`）。
pub mod ai;
/// **卡片匣**（v1.74.0）：跨书摘录卡的落盘层（`store` 同库的一张表）。
pub mod cards;
/// **全文检索索引**（v1.75.0）：SQLite FTS5 按需建索引，毫秒级定位候选章。
pub mod search;
/// **知识图谱**（V2.0 · MVP）：实体抽取 + 溯源落盘层（kg_entities / kg_relations /
/// kg_mentions / kg_meta）。纯逻辑，零 UI 依赖；Ollama 主路径 + 词典回退。
pub mod kg;
/// **清洗器**（v1.54.0）：两遍扫描——去噪 + 重建语义，产出 IR。
pub mod cleaner;
/// **加强版深度清洗漏斗**（v1.9.9）：TXT 广告行 / 智能断行 / 编码嗅探，
/// EPUB 内联样式 / 图片假标点 / 专名号，JSON 递归寻址 / HTML 残留，
/// DOCX 脚注 / 软回车 / 缩进垃圾。全是纯函数，先过单测才准接进漏斗。
pub mod dclean;
/// **外部格式残留清洗**（v1.9.26）：原格式的脚手架混进正文流——汉字间
/// 伪空格 / 页码行 / HTML 实体 / 点线目录 / Markdown 标记 / 上标注号 /
/// 破折号字形。判据一律「宁漏不误」，全部 `Option` 早退（没命中零分配）。
pub mod residue;
/// **文言白名单词表**（v1.9.29）：守卫 [`residue::strip_orphan_lines`] 的
/// 删字判据——孤字 + 下一行首字拼得上词，就说明它是真字（`希有` `一时`
/// `国风`），只归并不删。词表在 `data/ancient_dict.json`，编译期嵌入。
pub mod lexicon;
pub mod content;
/// **常用字白名单**（v1.81.0）：自动注音的判据（GB2312 一级字 + 其繁体形）。
/// 文件由 `scripts/gen_common_chars.py` 生成，勿手改。
pub mod common_chars;
pub mod dict;
/// **DOCX 导入**（v1.9.5）：zip + quick-xml 流式抽 `<w:t>`，还原成纯文本后
/// 喂 TXT 漏斗（切章 / 夹注归一 / 全角化全复用），带进度心跳、可边解析边落盘。
pub mod docx;
/// **内测邀请码**（v1.0.0-beta）：纯本地闸门，无服务器。
pub mod gate;
/// **字体字形覆盖**（v1.9.78）：自带极小 `cmap` 解析器，只回答一件事——
/// 这个字这款字体画不画得出。「导入古籍不出豆腐块」只能靠它验证，任何
/// 映射表都替不了缺字形。零平台依赖，纯字节解析。
pub mod glyph;
pub mod grid;
/// **命中测试**（v1.0.0-beta 自 app 迁入）：`Page` → 字/片包围盒，
/// 供画布点选、卡片定位、注音落点复用。纯几何，零 UI。
pub mod hit;
/// **双行夹注排版引擎**（v1.0.0-beta 自 app 迁入）：一次排出「面 → 列 → 片」
/// 整棵树，分页与渲染同源。只依赖 `types`，零 UI——跨平台壳子拿到的是
/// 同一份 `Page`，画法可以完全不同。
pub mod gloss;
pub mod importer;
/// **智能 JSON 兼容层**（v1.9.77）：把市面上各家古籍整理 JSON 的键名 / 嵌套
/// 归一成「段落串 + `[注]…[/注]`」。注疏字段（`note` / `annotation` / `注释` /
/// 三家注）不再被当元数据丢掉；结构认不出来就静默降级为纯文本，绝不报错。
pub mod jsonkit;
/// **统一书稿模型 IR**（v1.54.0）：解析与排版之间的中间表示。
/// 各格式先「断头」剥壳 → 清洗 → 落进 IR，引擎只认这一份数据。
pub mod ir;
/// **结构化数据导出（TEI / XML）**（V2.0 · B 端商业化模块）：把 IR 序列化成
/// 标准 TEI P5 —— 正文 `<p>` / 夹注 `<note type="gloss">` / 批注 `<note type="annotation">`
/// / 实体 `<persName>` `<placeName>` `<title>`，三路严格分离。零 UI 依赖，详见 `exporter.rs`。
pub mod exporter;
pub mod library;
/// **授权**（v1.81.0 · 模块六）：离线 key，自带校验位，**不绑机器**。
/// 只防随手转发与手抄错字，不做加密对抗——真要对抗得靠服务器或 App Store
/// 收据。铸 key 的脚本在 `scripts/gen_license.py`（算法必须与本模块同式）。
pub mod license;
pub mod numcn;
pub mod parsers;
pub mod pipeline;
/// **平台能力抽象层**（v1.0.0-beta）：`PdfExporter` / `InputSink` /
/// `PinyinSource` 三个 trait —— core 说「要什么」，壳子说「怎么画」。
/// 跨平台时换壳子不换内核，全靠这一层。
pub mod platform;
/// **古籍净化漏斗 · 文本层**（v1.80.0）：出版噪声词（ISBN / 丛书 / 译注 /
/// 版权页）的行级屏蔽、front matter 章节名判据、启发式正文起点定位。
/// 结构层（DOM / TOC / spine）在 `app::importer::epub_parser`。
pub mod purge;
/// **全局快捷键表**（v1.81.0）：动作 / 键位 / 映射的唯一数据源。
/// 壳子只做「框架按键 → `Chord`」的翻译与执行，键位本身不许在壳里另写一份。
pub mod keys;
/// **内存护栏**（v1.81.0）：后台重活（FTS5 建索引 / 批量导入）在块与块之间
/// 看一眼常驻内存，超限就在**后台线程**里退让——主线程一帧都不许等。
/// 读 RSS 由壳子注入探针（core 不许 `#[cfg(target_os)]`）。
pub mod mem;
/// **C ABI 出口**（v1.0.0-beta）：给 Tauri / Flutter / Swift / JNI 等
/// 非 Rust 壳子调 `cdylib` 用。见 `README_cross_platform.md`。
pub mod ffi;
pub mod prefs;
/// **样式预设**（v1.9.10）：把一整套版式存成一句话（「灯下」/「雨窗」/
/// 「仿刻」/「素笺」+ 三个用户槽位）。只管观感，不动字体与字级。
pub mod preset;
pub mod ruby;
/// 章节仓库（`store`）与流式导入（`stream`）：v1.50.0 为百万字丛书加的
/// 「边解析边落盘、按章取回」通道。
pub mod store;
pub mod stream;
pub mod trad;
/// **异体字 / 繁简映射层**（v1.9.78）：外部字典（三元组 / 二元组 / 分隔符 /
/// JSON）导入 → 三张泾渭分明的映射表（繁体正字 / 简体正字 / 可写回的异体归一），
/// 外加「字体缺字才折」的豆腐块抢救。基线 5164 条由
/// `scripts/gen_variant_table.py` 生成。详见 `variants.rs`。
pub mod variants;
/// **中医方剂本地化规则字典**（v1.9.19）：剂量量词 / 数量单位 / 异体 OCR 别名 /
/// 炮制短语全部外置到 `src/data/tcm_rules.json`（编译期 `include_str!` 嵌入），
/// 解析器只委托、不硬编码。详见 `tcm_rules.rs`。
pub mod tcm_rules;
pub mod theme;
pub mod types;
pub mod vclean;
pub mod west;
