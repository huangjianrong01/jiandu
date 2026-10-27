//! **繁简转换引擎（macOS · ICU）**：core 只定档位与口径，真正转字在这里。
//!
//! 用 `-[NSString stringByApplyingTransform:reverse:]`——它是 Foundation 对
//! ICU 变换的一层薄封装，`Hans-Hant` / `Hant-Hans` 是**词组级**规则：
//! 「头发 → 頭髮」转得对，「皇后」也不会被误转成「皇後」。自己维护一张逐字
//! 对照表做不到这一点（一字多繁：后/後、发/髮、干/乾/幹）。
//!
//! 三个约束：
//!   1. **不加依赖**：libobjc 三个符号 + Foundation 的类，`extern "C"` 直调。
//!   2. **必须带自动释放池**：`stringByApplyingTransform:` 返回的是
//!      autoreleased 对象，一页要转十几段，没有池子会一直涨。
//!   3. **失败返 `None`**：core 会退回逐字表——宁可质量差一档，也不吐乱码。
//!
//! # ⚠️ 为什么 `objc_msgSend` 声明成无参函数再 transmute
//!
//! 这是本次踩出来的坑，改这里的人务必先读：
//!
//! C 头文件里 `objc_msgSend` 是**变参**函数（`id objc_msgSend(id, SEL, ...)`）。
//! 在 Rust 里照着写成 `fn objc_msgSend(r, s, ...)` 在 **x86_64 上碰巧能跑**
//! （整数实参走寄存器，变参也在寄存器），但在 **arm64 上必崩**：AAPCS64 规定
//! 变参实参走**栈**，而 `objc_msgSend` 的实现是按**寄存器**（x2 / x3 …）读的
//! ——它从 x2 拿到的是垃圾，第一次调用就 SIGSEGV。
//!
//! 正确做法：把它当成一个**地址**，每次按真实签名 transmute 成具体函数指针
//! （`objc` crate 内部也是这么干的）。下面 `msg0` / `msg1s` / `msg2i` 就是
//! 本模块用到的三种签名。

use jiandu_core::trad::Convert;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};

/// ICU 变换名（Foundation 直接认 ICU 的 transform id）。
const HANS_TO_HANT: &str = "Hans-Hant";
const HANT_TO_HANS: &str = "Hant-Hans";

struct IcuConvert;

impl Convert for IcuConvert {
    fn convert(&self, text: &str, to_trad: bool) -> Option<String> {
        // SAFETY: 只在主线程 / 渲染线程调用；内部自带自动释放池；指针为空即返 None。
        unsafe { icu_convert(text, to_trad) }
    }
}

/// 装引擎（`main` 里调一次）。
pub fn install() {
    jiandu_core::trad::install(Box::new(IcuConvert));
}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    /// **只当符号地址用**，绝不按变参调（见文件头）。
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

/// `id f(id, SEL)`
unsafe fn msg0(r: *mut c_void, s: *mut c_void) -> *mut c_void {
    let f: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const ());
    f(r, s)
}

/// `id f(id, SEL, const char *)` —— `+stringWithUTF8String:`
unsafe fn msg1s(r: *mut c_void, s: *mut c_void, a: *const c_char) -> *mut c_void {
    let f: extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const ());
    f(r, s, a)
}

/// `id f(id, SEL, id, BOOL)` —— `-stringByApplyingTransform:reverse:`
unsafe fn msg2i(r: *mut c_void, s: *mut c_void, a: *mut c_void, b: i32) -> *mut c_void {
    let f: extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const ());
    f(r, s, a, b)
}

/// 一次转换的完整生命周期（池 → 建串 → 变换 → 取回 → 收池）。
unsafe fn icu_convert(text: &str, to_trad: bool) -> Option<String> {
    let sel = |n: &[u8]| -> *mut c_void { sel_registerName(n.as_ptr() as *const c_char) };

    let pool = objc_autoreleasePoolPush();
    let out = (|| -> Option<String> {
        let ns_string = objc_getClass(b"NSString\0".as_ptr() as *const c_char);
        if ns_string.is_null() {
            return None;
        }
        let src_c = CString::new(text).ok()?;
        let src = msg1s(ns_string, sel(b"stringWithUTF8String:\0"), src_c.as_ptr());
        if src.is_null() {
            return None;
        }
        let name = if to_trad { HANS_TO_HANT } else { HANT_TO_HANS };
        let name_c = CString::new(name).ok()?;
        let tn = msg1s(ns_string, sel(b"stringWithUTF8String:\0"), name_c.as_ptr());
        if tn.is_null() {
            return None;
        }
        let conv = msg2i(src, sel(b"stringByApplyingTransform:reverse:\0"), tn, 0);
        if conv.is_null() {
            return None;
        }
        let utf8 = msg0(conv, sel(b"UTF8String\0"));
        if utf8.is_null() {
            return None;
        }
        Some(
            CStr::from_ptr(utf8 as *const c_char)
                .to_string_lossy()
                .into_owned(),
        )
    })();
    objc_autoreleasePoolPop(pool);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试里统一包一层：生产路径上 `IcuConvert` 已经替我们挂在 unsafe 里了。
    fn conv(s: &str, to_trad: bool) -> Option<String> {
        unsafe { icu_convert(s, to_trad) }
    }

    #[test]
    fn icu_converts_phrases_not_just_characters() {
        // 「头发」要整词转成「頭髮」——逐字表只会得「頭發」。
        let t = conv("头发", true).expect("ICU 变换失败");
        assert_eq!(t, "頭髮");

        // 反方向：繁 → 简。
        let s = conv("頭髮", false).expect("ICU 变换失败");
        assert_eq!(s, "头发");
    }

    #[test]
    fn ambiguous_char_keeps_context() {
        // 「皇后」的「后」在繁体里仍是「后」（不是「後」）——这正是词组级的价值。
        let t = conv("皇后", true).expect("ICU 变换失败");
        assert_eq!(t, "皇后");
        // 而「以后」的「后」该转「後」。
        let t2 = conv("以后", true).expect("ICU 变换失败");
        assert_eq!(t2, "以後");
    }

    #[test]
    fn plain_ascii_and_empty_survive() {
        assert_eq!(conv("", true).as_deref(), Some(""));
        assert_eq!(conv("hello 123", true).as_deref(), Some("hello 123"));
    }
}
