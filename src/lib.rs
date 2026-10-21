//! 笺读 JianDu · 纯逻辑库目标。
//!
//! 只暴露不依赖 GPUI 的纯模块（网格引擎、双行夹注排版、数据模型），
//! 供单元测试与 `examples/` 下的控制台验证程序使用；
//! 上层渲染（`app` / `reader`）仍在二进制目标 `src/main.rs`。
#![allow(dead_code)]

pub mod content;
pub mod dict;
pub mod gloss;
pub mod grid;
pub mod hit;
pub mod importer;
pub mod library;
pub mod ruby;
pub mod trad;
pub mod types;
pub mod west;
