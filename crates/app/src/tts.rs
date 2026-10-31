//! TTS 语音朗读（MVP · 分支 feature/tts-voice）
//!
//! 目标：验证能否用**系统原生**语音合成把选中的古文读出来——**零第三方依赖**、
//! **零体积增加**。优先 `AVSpeechSynthesizer`（AVFoundation，用户点名），取不到
//! 该类的环境兜底 `NSSpeechSynthesizer`（AppKit，与 `sound.rs` 的 `NSSound`
//! 同框架，必定可用）。两者都是 macOS 系统自带的语音引擎。
//!
//! FFI 完全照搬 `sound.rs` 的 libobjc 直调套路：只声明 `objc_getClass` /
//! `sel_registerName` / `objc_msgSend` 三个符号（它们都来自 libobjc，gpui 已链接，
//! 不必拖 `objc`/`cocoa` crate）；所有变参调用一律 `transmute` 成具体签名后再调
//! （v1.81.0 修正：arm64 上变参实参走寄存器，照 C 可变参声明调必崩）。
//!
//! 所有失败路径一律静默返回 `false`——没声音不影响读书，绝不 panic。

use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
        /// 自动释放池（libobjc 自带，无需任何新依赖）。`cargo test` 等非主线程
        /// 环境没有默认释放池，`stringWithUTF8String:` 等返回的临时对象会被立刻
        /// 回收，导致 `initWithString:` 读到悬垂指针 → SIGBUS。用 push/pop 把整段
        /// FFI 兜进一个池里，任何线程都安全（主线程上等价于无操作）。
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    // —— 主线程判定 + 抛回主队列（libSystem，系统自带，零新依赖）——
    #[link(name = "System")]
    extern "C" {
        /// 当前是否主线程（macOS / iOS 上 AVFoundation 必须在主线程）。
        fn pthread_main_np() -> i32;
        /// 异步把 `work(context)` 派发到 `queue`。
        fn dispatch_async_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
        /// `dispatch_get_main_queue()` 在本机 SDK 是内联宏，直接返回 `&_dispatch_main_q`；
        /// 这里取这个全局符号的地址即得主队列指针（避免 `_dispatch_get_main_queue`
        /// 内联符号在链接期不存在）。
        static _dispatch_main_q: c_void;
    }

    // —— objc_msgSend 的具体签名包装（transmute，绝不按变参调）——
    unsafe fn msg0(r: *mut c_void, s: *mut c_void) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s)
    }
    unsafe fn msg1_id(r: *mut c_void, s: *mut c_void, a: *mut c_void) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s, a)
    }
    unsafe fn msg1_cstr(r: *mut c_void, s: *mut c_void, a: *const c_char) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s, a)
    }

    unsafe fn sel(n: &[u8]) -> *mut c_void {
        sel_registerName(n.as_ptr() as *const c_char)
    }
    unsafe fn ns_str(s: *const c_char) -> *mut c_void {
        let cls = objc_getClass(b"NSString\0".as_ptr() as *const c_char);
        if cls.is_null() {
            return std::ptr::null_mut();
        }
        msg1_cstr(cls, sel(b"stringWithUTF8String:\0"), s)
    }

    /// 设置朗读语速（倍率 → 引擎原生值）。
    ///
    /// `setRate:` 是**定元**方法、第三参是 `float`——arm64 上 float 走 s0 寄存器，
    /// 照 `objc_msgSend` 的可变参声明（全走通用寄存器）调必崩。故 transmute 成
    /// `(*mut c_void, *mut c_void, f32)` 的具体签名再调（与 v1.81.0 修 `msg1_*` 同思路）。
    unsafe fn set_rate(target: *mut c_void, rate: f32) {
        let f: extern "C" fn(*mut c_void, *mut c_void, f32) =
            std::mem::transmute(objc_msgSend as *const ());
        f(target, sel(b"setRate:\0"), rate);
    }

    /// 缓存的合成器对象（裸指针按 usize 存以过 Send/Sync）。
    static SYNTH: OnceLock<usize> = OnceLock::new();
    /// 1 = AVSpeechSynthesizer，2 = NSSpeechSynthesizer。
    static WHICH: AtomicU8 = AtomicU8::new(0);

    pub fn speak(text: &str, rate: f32) -> bool {
        let Ok(cstr) = CString::new(text) else {
            return false;
        };
        // AVFoundation 的合成器 / 朗读必须在**主线程**跑——非主线程上
        // `stringWithUTF8String:` 的临时对象没有释放池兜底 → 悬垂指针 → SIGBUS。
        // 主线程直跑；非主线程用 `dispatch_async` 把任务抛回主队列，由 App 主运行
        // 循环执行（真实 App 里 [听] 一定在主线程触发，这条分支只是兜底保险）。
        unsafe {
            if pthread_main_np() != 0 {
                speak_on_main(cstr.as_ptr(), rate)
            } else {
                let boxed = Box::into_raw(Box::new((cstr, rate)));
                let main_q = std::ptr::addr_of!(_dispatch_main_q) as *mut c_void;
                dispatch_async_f(main_q, boxed as *mut c_void, speak_trampoline);
                true // 已抛回主线程，乐观返回；真正发声在主线程完成后发生。
            }
        }
    }

    /// 主线程内的实际合成 + 朗读（调用方保证在主线程）。释放池兜底，任何线程都安全。
    ///
    /// `rate` 是倍率（0.8 / 1.0 / 1.25）——
    ///   * AVSpeechUtterance 的 `rate` 是 0–1 的相对值（默认 0.5），故 `0.5 * rate`；
    ///   * NSSpeechSynthesizer 的 `rate` 是每分钟字数（默认 200），故 `200 * rate`。
    /// 两个引擎各自缩放，调用方只传一个人类可读的倍率。
    ///
    /// 合成器只建一次（存进 `SYNTH` 后复用），既防止每次点击都泄漏一个
    /// Objective-C 对象，也避免旧合成器还在播就被反复新建带来的状态混乱。
    unsafe fn speak_on_main(cstr: *const c_char, rate: f32) -> bool {
        let pool = objc_autoreleasePoolPush();
        // 已选定引擎：复用已建好的合成器，只新建 utterance 朗读。
        let r = match WHICH.load(Ordering::Relaxed) {
            1 => match SYNTH.get() {
                Some(p) => speak_av(*p as *mut c_void, cstr, rate),
                None => false,
            },
            2 => match SYNTH.get() {
                Some(p) => speak_ns(*p as *mut c_void, cstr, rate),
                None => false,
            },
            _ => {
                // 首次：优先 AV，取不到回退 NS。
                if let Some(s) = try_av(cstr, rate) {
                    WHICH.store(1, Ordering::Relaxed);
                    SYNTH.get_or_init(|| s as usize);
                    true
                } else if let Some(s) = try_ns(cstr, rate) {
                    WHICH.store(2, Ordering::Relaxed);
                    SYNTH.get_or_init(|| s as usize);
                    true
                } else {
                    false
                }
            }
        };
        objc_autoreleasePoolPop(pool);
        r
    }

    /// `dispatch_async_f` 的回调：在主线程重建 CString 并朗读，随后释放。
    extern "C" fn speak_trampoline(ctx: *mut c_void) {
        unsafe {
            let boxed = Box::from_raw(ctx as *mut (CString, f32));
            speak_on_main(boxed.0.as_ptr(), boxed.1);
        }
    }

    /// AVSpeechSynthesizer 路径（用户点名；若 AVFoundation 未加载则类为 null，返回 None）。
    ///
    /// 这里**只建合成器**（首次调用），朗读交给 `speak_av`。合成器建好后由
    /// `SYNTH` 长期持有（裸指针 leaked，语音播完前绝不被释放——解决「播完前被
    /// Drop 导致回调访问悬垂指针」这一类崩溃）。
    unsafe fn try_av(cstr: *const c_char, rate: f32) -> Option<*mut c_void> {
        let cls = objc_getClass(b"AVSpeechSynthesizer\0".as_ptr() as *const c_char);
        if cls.is_null() {
            return None;
        }
        let synth = msg0(msg0(cls, sel(b"alloc\0")), sel(b"init\0"));
        if synth.is_null() {
            return None;
        }
        speak_av(synth, cstr, rate);
        Some(synth)
    }

    /// 在已建好的 AVSpeechSynthesizer 上朗读一段文字（每次点击都新建 utterance）。
    unsafe fn speak_av(synth: *mut c_void, cstr: *const c_char, rate: f32) -> bool {
        let ucls = objc_getClass(b"AVSpeechUtterance\0".as_ptr() as *const c_char);
        if ucls.is_null() {
            return false;
        }
        let ustr = ns_str(cstr);
        let utt = msg1_id(msg0(ucls, sel(b"alloc\0")), sel(b"initWithString:\0"), ustr);
        if utt.is_null() {
            return false;
        }
        // 中文语音：优先普通话（大陆 Ting-Ting / 台湾 Mei-Jia 皆可），取不到用默认。
        // ⚠️ `voiceWithLanguage:` 收的是 **NSString\***，绝不是 C 字符串——
        // 把裸 C 字符串指针当对象传进去，框架会把它当成 NSString 发消息 → 立刻
        // EXC_BAD_ACCESS 闪退（v1.9.9 热修前就是这个 bug）。必须用 `ns_str` 包一层。
        let vcls = objc_getClass(b"AVSpeechSynthesisVoice\0".as_ptr() as *const c_char);
        if !vcls.is_null() {
            let lang = ns_str(b"zh-CN\0".as_ptr() as *const c_char);
            let v = msg1_id(vcls, sel(b"voiceWithLanguage:\0"), lang);
            if !v.is_null() {
                msg1_id(utt, sel(b"setVoice:\0"), v);
            }
        }
        // 语速：AVSpeechUtterance 的相对速率默认 0.5，倍率直接乘上去。
        let av_rate = (0.5 * rate).clamp(0.1, 0.9);
        set_rate(utt, av_rate);
        let _ = msg1_id(synth, sel(b"speakUtterance:\0"), utt);
        true
    }

    /// NSSpeechSynthesizer 兜底（AppKit，必在；默认语音可能偏英文，但一定能发声）。
    /// 同样只建合成器，朗读交给 `speak_ns`。
    unsafe fn try_ns(cstr: *const c_char, rate: f32) -> Option<*mut c_void> {
        let cls = objc_getClass(b"NSSpeechSynthesizer\0".as_ptr() as *const c_char);
        if cls.is_null() {
            return None;
        }
        let synth = msg0(msg0(cls, sel(b"alloc\0")), sel(b"init\0"));
        if synth.is_null() {
            return None;
        }
        speak_ns(synth, cstr, rate);
        Some(synth)
    }

    /// 在已建好的 NSSpeechSynthesizer 上朗读一段文字。
    unsafe fn speak_ns(synth: *mut c_void, cstr: *const c_char, rate: f32) -> bool {
        // 尝试设中文语音；不同系统 identifier 不同，失败就沿用默认语音。
        let v = ns_str(
            b"com.apple.speech.synthesis.voice.Ting-Ting\0".as_ptr() as *const c_char,
        );
        if !v.is_null() {
            msg1_id(synth, sel(b"setVoice:\0"), v);
        }
        // 语速：NSSpeechSynthesizer 以「每分钟字数」计，默认 200；倍率直乘。
        let ns_rate = (200.0 * rate).clamp(80.0, 400.0);
        set_rate(synth, ns_rate);
        let _ = msg1_id(synth, sel(b"startSpeakingString:\0"), ns_str(cstr));
        true
    }

    pub fn stop() -> bool {
        let p = match SYNTH.get() {
            Some(p) => *p,
            None => return false,
        };
        let which = WHICH.load(Ordering::Relaxed);
        unsafe {
            let synth = p as *mut c_void;
            if which == 1 {
                // AVSpeechSynthesizer: stopSpeakingAtBoundary: 0 (Immediate)
                msg1_id(synth, sel(b"stopSpeakingAtBoundary:\0"), 0 as *mut c_void);
            } else if which == 2 {
                // NSSpeechSynthesizer: stopSpeaking
                msg0(synth, sel(b"stopSpeaking\0"));
            }
        }
        true
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn speak(_text: &str, _rate: f32) -> bool {
        false
    }
    pub fn stop() -> bool {
        false
    }
}

pub use imp::*;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    // AVFoundation 的合成器必须在主线程跑；`cargo test` 把用例放到 worker 线程，
    // 真机验证不便。这条用例仅作编译期存在性校验，运行时跳过——
    // 真正的「能发声」以真实 App（主线程点 [听]）为准。
    #[test]
    #[ignore = "AVFoundation 必须在主线程；cargo test 跑在 worker 线程，跳过。真机点 [听] 验证。"]
    fn speak_zh_cn_returns_true_and_no_panic() {
        // 验证 Rust FFI 在真实 macOS 上能解析 AVSpeechSynthesizer 类、构造
        // utterance、设 zh-CN 语音并调用 speakUtterance 而不崩溃（与 app 内
        // [听] 按钮走的是同一段代码）。
        let ok = speak("床前明月光，疑是地上霜。举头望明月，低头思故乡。", 1.0);
        assert!(ok, "speak() 应返回 true：AVSpeechSynthesizer 路径打通");
        std::thread::sleep(std::time::Duration::from_millis(800));
        let stopped = stop();
        assert!(stopped, "stop() 应返回 true：合成器对象已缓存");
    }
}

