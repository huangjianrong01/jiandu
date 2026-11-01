//! uniffi-bindgen CLI 入口：从编译产物生成 Swift / Kotlin 等绑定。
//!
//! 用法（见 build-ios.sh）：
//!   cargo run --bin uniffi-bindgen -- generate \
//!     --library target/aarch64-apple-ios/release/libjiandu_ios_ffi.a \
//!     --language swift --out-dir bindings

fn main() {
    uniffi::uniffi_bindgen_main()
}
