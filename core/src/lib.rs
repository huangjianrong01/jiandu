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

pub mod banxin;
pub mod content;
pub mod dict;
pub mod grid;
pub mod importer;
pub mod library;
pub mod numcn;
pub mod parsers;
pub mod pdf;
pub mod pipeline;
pub mod prefs;
pub mod ruby;
pub mod trad;
pub mod theme;
pub mod types;
pub mod vclean;
pub mod west;
