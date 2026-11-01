//! 笺读 · iOS FFI 层入口。
//!
//! 本 crate 是 core 的「iOS 外壳」：
//! * `ffi` 模块暴露 uniffi 接口（版本号 / 排版探针 / PDF 导出入口），SwiftUI 直接消费；
//! * `pdf_export` 模块是 iOS 专属（`#[cfg(target_os = "ios")]`），用 raw CoreGraphics +
//!   CoreText C FFI 真正生成矢量 PDF，**只在 iOS 编译**，Mac / 其他平台不碰。
//!
//! core 保持纯算法、零平台依赖；这里只做「桥接」，排版几何全部复用 core。

// uniffi 脚手架：生成 UniFfiTag 等元数据（跨平台都需要，Mac / iOS 都编进）。
uniffi::setup_scaffolding!();

mod ffi;

// iOS 专属 PDF 导出桥接：仅在 iOS 目标编译，Mac 端构建时整块剔除。
#[cfg(target_os = "ios")]
mod pdf_export;
