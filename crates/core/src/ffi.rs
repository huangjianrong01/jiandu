//! **C ABI 出口**（给 Tauri / Flutter / Swift / Kotlin 等**非 Rust 壳子**用）。
//!
//! 为什么要有这一层：跨平台壳子不一定能直接 link Rust 的 `rlib`。
//! 把 core 编成 `cdylib` 后，Windows 得到 `jiandu_core.dll`、
//! Linux 得到 `libjiandu_core.so`、macOS 得到 `libjiandu_core.dylib`，
//! 任何能调 C 函数的语言（Tauri 的 Rust 端、Dart FFI、JNI、Swift）都能用。
//!
//! ```text
//!   cargo build -p jiandu-core --release
//!   → target/release/libjiandu_core.dylib      (macOS)
//!   → target/release/jiandu_core.dll           (Windows)
//!   → target/release/libjiandu_core.so         (Linux)
//! ```
//!
//! ── 约定（写错就是内存泄漏或野指针，记牢）─────────────────────────
//! 1. **返回 `*mut c_char` 的字符串，一律由调用方用 [`jiandu_string_free`] 释放**
//!    —— 是本 crate 用 `CString::into_raw` 交出去的，别人 free 不了。
//! 2. 返回 `null` 表示「无此结果」（不是错误）。真正的错误走
//!    [`jiandu_last_error`]（线程局部，下次调用会覆盖）。
//! 3. 本模块**不做** UI、不弹窗、不读剪贴板；纯计算，可放心在后台线程调。
//! 4. 需要整本书 / 检索 / 排版这类**大数据**能力时，别走 FFI 逐个搬结构体——
//!    走 Tauri 的 IPC（Rust 端直接依赖 `jiandu-core`，见 `README_cross_platform.md`）。
//!    FFI 只适合「小进小出」的查询。
//!
//! ⚠️ 全部函数都是 `unsafe`（跨语言边界），但**内部不做任何 unsafe 之外的
//! 假设**：指针为 null 就返回 null，绝不 deref。

use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::c_char;

// 上一次错误（线程局部）。跨语言传错误码很啰嗦，留一句人话在这里更实用。
// （`thread_local!` 是宏，用 `//!` / `///` 会报 unused_doc_comments，故用行注。）
thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_error(msg: impl AsRef<str>) {
    let s = CString::new(msg.as_ref()).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(s));
}

/// 把 Rust 字符串交出去（调用方负责 `jiandu_string_free`）。
fn give(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        // 字符串里嵌了 `\0`（理论上只有恶意输入才会），宁可交个空串
        Err(_) => CString::new("").unwrap().into_raw(),
    }
}

/// 从 C 侧安全地读一个 UTF-8 字符串（null → None）。
unsafe fn take<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        return None;
    }
    std::ffi::CStr::from_ptr(s).to_str().ok()
}

// ── 生命周期 ────────────────────────────────────────────────────────────────

/// 释放本 crate 交出去的字符串。**只能**用于这些函数返回的指针。
///
/// # Safety
/// `s` 必须是本 crate 返回的 `*mut c_char`，且未被释放过；传 null 是安全的空操作。
#[no_mangle]
pub unsafe extern "C" fn jiandu_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// core 的版本号（与 `Cargo.toml` 同步），用于排查「壳子与内核版本对不上」。
///
/// # Safety
/// 无参数，安全。返回值须用 [`jiandu_string_free`] 释放。
#[no_mangle]
pub extern "C" fn jiandu_version() -> *mut c_char {
    give(env!("CARGO_PKG_VERSION").to_string())
}

/// 上一次错误的文本；没有则空串。
///
/// # Safety
/// 无参数，安全。返回值须用 [`jiandu_string_free`] 释放。
#[no_mangle]
pub extern "C" fn jiandu_last_error() -> *mut c_char {
    LAST_ERROR.with(|e| match e.borrow().as_ref() {
        Some(c) => {
            // 复制一份交出去（调用方会 free，不能把内部那份交出去）
            let bytes = c.as_bytes_with_nul().to_vec();
            unsafe { CString::from_vec_with_nul_unchecked(bytes) }.into_raw()
        }
        None => give(String::new()),
    })
}

// ── 注音（跨平台壳子里最常问的一件事）─────────────────────────────────────

/// 这个字该注什么音（含声调，如 `duǒ`）；不用注或查不到则返回 `null`。
///
/// # Safety
/// 无指针参数，安全。返回值须用 [`jiandu_string_free`] 释放。
#[no_mangle]
pub extern "C" fn jiandu_auto_pinyin(codepoint: u32) -> *mut c_char {
    match std::char::from_u32(codepoint) {
        Some(c) => match crate::dict::auto_pinyin(c) {
            Some(p) => {
                LAST_ERROR.with(|e| *e.borrow_mut() = None);
                give(p)
            }
            None => std::ptr::null_mut(),
        },
        None => {
            set_error(format!("U+{codepoint:04X} 不是合法码位"));
            std::ptr::null_mut()
        }
    }
}

/// 这个字算不算常用（常用的不注音）：1 = 常用，0 = 生僻或非法码位。
///
/// # Safety
/// 无指针参数，安全。
#[no_mangle]
pub extern "C" fn jiandu_is_common_char(codepoint: u32) -> u8 {
    match std::char::from_u32(codepoint) {
        Some(c) => u8::from(crate::dict::is_common_char(c)),
        None => 0,
    }
}

// ── 字书 / 词典 ─────────────────────────────────────────────────────────────

/// 查一个字的简明释义（王力《古汉语常用字字典》首义）。无则 `null`。
///
/// # Safety
/// `word` 必须是指向合法 UTF-8 的 C 字符串，或 null。返回值须用
/// [`jiandu_string_free`] 释放。
#[no_mangle]
pub unsafe extern "C" fn jiandu_define(word: *const c_char) -> *mut c_char {
    let Some(w) = take(word) else {
        set_error("word 为 null");
        return std::ptr::null_mut();
    };
    match crate::dict::define(w) {
        Some(s) => give(s.to_string()),
        None => std::ptr::null_mut(),
    }
}

/// 内测邀请码校验：1 = 通过，0 = 不通过。
///
/// 纯本地比对（无服务器），语义与 `gate::valid` 完全一致——跨平台壳子
/// 若要同一道闸门，走这个入口，别自己再抄一份码表。
///
/// # Safety
/// `code` 必须是指向合法 UTF-8 的 C 字符串，或 null。
#[no_mangle]
pub unsafe extern "C" fn jiandu_gate_check(code: *const c_char) -> u8 {
    let Some(c) = take(code) else {
        set_error("code 为 null");
        return 0;
    };
    u8::from(crate::gate::valid(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        let p = jiandu_version();
        assert!(!p.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_str().unwrap();
        assert!(!s.is_empty());
        unsafe { jiandu_string_free(p) };
    }

    #[test]
    fn auto_pinyin_round_trip() {
        // 埵 U+57F5：生僻，应有音
        let p = jiandu_auto_pinyin(0x57F5);
        assert!(!p.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_str().unwrap().to_string();
        unsafe { jiandu_string_free(p) };
        assert!(!s.is_empty(), "「埵」应有读音");
        // 般 U+822C：常用，不注音
        assert_eq!(jiandu_is_common_char(0x822C), 1);
        // 非法码位不崩
        assert!(jiandu_auto_pinyin(0x110000).is_null());
    }

    #[test]
    fn gate_check_accepts_normalized_code() {
        let c = CString::new("shuye2026").unwrap();
        assert_eq!(unsafe { jiandu_gate_check(c.as_ptr()) }, 1);
        let bad = CString::new("不是码").unwrap();
        assert_eq!(unsafe { jiandu_gate_check(bad.as_ptr()) }, 0);
        assert_eq!(unsafe { jiandu_gate_check(std::ptr::null()) }, 0);
    }
}
