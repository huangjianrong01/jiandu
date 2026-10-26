//! 笺读 JianDu · 纯逻辑核心库（v1.47.0 起独立成 crate）。
//!
//! **零 GPUI 依赖**：
//!   * 网格排版引擎（`grid`：天宽地窄、文武边栏、鱼尾黑口的全部几何演算）；
//!   * 双行夹注之外的版面数据模型（`types` / `content` / `ruby`）；
//!   * EPUB / TXT / MD / HTML 解析与清洗（`parsers` / `vclean` / `pipeline` /
//!     `importer` / `library`：通假、脚注、前言智能跳过、上标幽灵字清剿）；
//!   * 离线双引擎字典（`dict`：古汉语常用字 + 康熙/說文合并库）；
//!   * 简繁对照（`trad` / `west`）、数字转汉字（`numcn`）、版心几何（`banxin`）、
//!     纸墨主题数据（`theme`）、偏好（`prefs`）、PDF 导出（`pdf`）。
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
/// **清洗器**（v1.54.0）：两遍扫描——去噪 + 重建语义，产出 IR。
pub mod cleaner;
pub mod content;
pub mod dict;
pub mod grid;
pub mod importer;
/// **统一书稿模型 IR**（v1.54.0）：解析与排版之间的中间表示。
/// 各格式先「断头」剥壳 → 清洗 → 落进 IR，引擎只认这一份数据。
pub mod ir;
pub mod library;
pub mod numcn;
pub mod parsers;
pub mod pdf;
pub mod pipeline;
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
