//! 笺读 JianDu · 库目标（v1.47.0 起为 `jiandu-core` 的门面）。
//!
//! v1.47.0 架构解耦：排版网格（grid）、解析器（parsers）、字典查询（dict）
//! 等 17 个纯逻辑模块已独立成 `jiandu-core` crate（零 GPUI 依赖，可移植
//! iOS）；本库透传它的全部模块，`jiandu::grid` 等老路径继续可用，
//! `examples/` 下的控制台验证程序不受影响。
//!
//! 与绘制耦合的三个排版辅助（punct 的句读画笔需要 gpui）仍留在本库内。
#![allow(dead_code)]
// 排版函数按几何逐项传值（列宽 / 行高 / 天头 / 地脚…），硬拆成 struct
// 反而掩盖每一项的物理意义——与 bin 的 crate 级政策同一口径。
#![allow(clippy::too_many_arguments)]

pub use jiandu_core::*;

pub mod gloss;
pub mod hit;
pub mod punct;
