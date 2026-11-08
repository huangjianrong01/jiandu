//! L5 落库 · SQLite 持久化适配（re-export from shared `align-db` crate）。
//!
//! 原 `db.rs` 实现已抽成独立 `align-db` crate，供 `ocr-cli` 与 `pdf-cli` 共用，
//! 保持 `jiandu-core` 零 DB 依赖（纯洁红线）。这里只做透明转发，避免改动
//! `ocr-cli/src/main.rs` 里现有的 `db::*` 调用。

pub use align_db::*;
