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
/// **批量导入**（v1.77.0）：点检 → 串行刻印 → 可取消 → 错误日志。
pub mod batch;
/// **问小笺**（v1.76.0 · V2.0 模块二）：AI 伴读的纯逻辑层——服务商预设 /
/// 提示词组装 / SSE 增量解析。零网络依赖（HTTP 在 UI 层 `src/ai.rs`）。
pub mod ai;
/// **卡片匣**（v1.74.0）：跨书摘录卡的落盘层（`store` 同库的一张表）。
pub mod cards;
/// **全文检索索引**（v1.75.0）：SQLite FTS5 按需建索引，毫秒级定位候选章。
pub mod search;
/// **清洗器**（v1.54.0）：两遍扫描——去噪 + 重建语义，产出 IR。
pub mod cleaner;
pub mod content;
/// **常用字白名单**（v1.81.0）：自动注音的判据（GB2312 一级字 + 其繁体形）。
/// 文件由 `scripts/gen_common_chars.py` 生成，勿手改。
pub mod common_chars;
pub mod dict;
/// **内测邀请码**（v1.0.0-beta）：纯本地闸门，无服务器。
pub mod gate;
pub mod grid;
/// **命中测试**（v1.0.0-beta 自 app 迁入）：`Page` → 字/片包围盒，
/// 供画布点选、卡片定位、注音落点复用。纯几何，零 UI。
pub mod hit;
/// **双行夹注排版引擎**（v1.0.0-beta 自 app 迁入）：一次排出「面 → 列 → 片」
/// 整棵树，分页与渲染同源。只依赖 `types`，零 UI——跨平台壳子拿到的是
/// 同一份 `Page`，画法可以完全不同。
pub mod gloss;
pub mod importer;
/// **统一书稿模型 IR**（v1.54.0）：解析与排版之间的中间表示。
/// 各格式先「断头」剥壳 → 清洗 → 落进 IR，引擎只认这一份数据。
pub mod ir;
pub mod library;
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
/// **C ABI 出口**（v1.0.0-beta）：给 Tauri / Flutter / Swift / JNI 等
/// 非 Rust 壳子调 `cdylib` 用。见 `README_cross_platform.md`。
pub mod ffi;
pub mod prefs;
pub mod ruby;
/// 章节仓库（`store`）与流式导入（`stream`）：v1.50.0 为百万字丛书加的
/// 「边解析边落盘、按章取回」通道。
pub mod store;
pub mod stream;
pub mod trad;
pub mod theme;
pub mod types;
pub mod vclean;
pub mod west;
