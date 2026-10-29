//! **拖拽入书**：把 Finder 里拖进来的文件接住（v1.9.5）。
//!
//! # 为什么得手写 FFI
//!
//! GPUI 0.2.2 没有文件拖放 API（没有 `on_drop` / `ExternalPaths`，源码里
//! 连 `NSDragging` 都不出现），所以这事只能落到 AppKit 上。纪律与
//! [`crate::sound`] 那条 NSSound 通道一致：**不加任何新依赖**，`libobjc`
//! 里的四个符号直接 `extern "C"` 声明（gpui 本身也链 libobjc）。
//!
//! # 做法
//!
//! 给 GPUI 窗口的 NSView **补上三个方法**（`class_addMethod` 只加方法，
//! 不改类结构、不替换类）：
//!
//! | 选择器 | 作用 |
//! |---|---|
//! | `draggingEntered:` | 返回 `NSDragOperationCopy`，光标才显示「可放开」 |
//! | `prepareForDragOperation:` | 返回 YES，AppKit 才往下走 |
//! | `performDragOperation:` | 取拖拽粘贴板里的文件路径，塞进全局队列 |
//!
//! 再对那个 view 调 `registerForDraggedTypes:` 登记 `NSFilenamesPboardType`
//! （Finder 多选拖拽的兼容类型）与 `public.file-url`（单文件兜底）。
//!
//! # 三条铁律
//!
//! 1. **AppKit 回调里绝不碰 GPUI**：只把路径塞进 `Mutex<Vec<PathBuf>>`，
//!    由 UI 每帧 `take()` 一次（与 [`ImportJob`](crate::app::ImportJob) 的
//!    `take_done` 同一节奏）。跨线程回调里动实体 = 必崩。
//! 2. **任何一步失败都静默降级**：没窗口、类没有这个方法、粘贴板为空——
//!    一律返回，绝不 panic、绝不弹错。拖不进来是少个功能，崩了是事故。
//! 3. **只认文件**：拖进来的若是文字 / 图片，队列为空，自然什么也不发生。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// 待处理的拖入文件（AppKit 线程写，UI 线程取）。
static DROPPED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// 是否已经挂上过（仅用于调试日志，不再作为「只挂一次」的闸门）。
static ARMED: AtomicBool = AtomicBool::new(false);

/// 渲染帧计数（每 30 帧补挂一次）。
static FRAMES: AtomicU32 = AtomicU32::new(0);

/// 取走这一帧收到的拖入文件（只会成功一次）。
pub fn take() -> Vec<PathBuf> {
    DROPPED
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

fn push_dropped(p: PathBuf) {
    // 同一个文件连拖两次是常态，但一次拖拽事件里不该重复。
    if let Ok(mut g) = DROPPED.lock() {
        if !g.contains(&p) {
            g.push(p);
        }
    }
}

/// 每帧调一次：每 30 帧扫一遍所有窗口，把新窗口 / 新建的视图也挂上。
///
/// 为什么不是「挂一次就完事」：GPUI 起窗、换窗、加子视图的时刻都不确定，
/// 而 AppKit 的拖放判定落在**最深层那个被击中的视图**上——只挂 `contentView`
/// 会漏（子视图继承的是 NSView 的默认实现，一律返回「不接受」）。
pub fn arm_once() {
    let n = FRAMES.fetch_add(1, Ordering::Relaxed);
    if n % 30 != 0 {
        return;
    }
    #[cfg(target_os = "macos")]
    unsafe {
        arm_all_windows();
    }
}

// ---------------------------------------------------------------------------
// macOS：libobjc / AppKit 直调
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod ffi {
    use std::os::raw::{c_char, c_void};

    #[link(name = "objc")]
    extern "C" {
        pub fn objc_getClass(name: *const c_char) -> *mut c_void;
        pub fn sel_registerName(name: *const c_char) -> *mut c_void;
        pub fn object_getClass(obj: *mut c_void) -> *mut c_void;
        pub fn class_addMethod(
            cls: *mut c_void,
            name: *mut c_void,
            imp: *const c_void,
            types: *const c_char,
        ) -> i8;
        /// **只当符号地址用，绝不按变参调**：C 头里是 `id objc_msgSend(id, SEL, ...)`，
        /// 照着写成变参会调用崩溃（arm64 变参走栈、它按寄存器读）。下面统一
        /// `transmute` 成具体签名再调——与 `sound.rs` 同一套修法。
        pub fn objc_msgSend();
    }

    /// `id f(id, SEL)`
    pub unsafe fn msg0(r: *mut c_void, s: *mut c_void) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s)
    }

    /// `id f(id, SEL, id)`
    pub unsafe fn msg1(r: *mut c_void, s: *mut c_void, a: *mut c_void) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s, a)
    }

    /// `NSUInteger f(id, SEL)`
    pub unsafe fn msg_len(r: *mut c_void, s: *mut c_void) -> usize {
        let f: extern "C" fn(*mut c_void, *mut c_void) -> usize =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s)
    }

    /// `id f(id, SEL, NSUInteger)`
    pub unsafe fn msg_at(r: *mut c_void, s: *mut c_void, i: usize) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s, i)
    }

    /// `id f(id, SEL, const char *)`
    pub unsafe fn msg_str(
        r: *mut c_void,
        s: *mut c_void,
        a: *const c_char,
    ) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s, a)
    }

    /// `id f(id, SEL, const id *, NSUInteger)`（`arrayWithObjects:count:`）
    pub unsafe fn msg_arr(
        r: *mut c_void,
        s: *mut c_void,
        a: *const *mut c_void,
        n: usize,
    ) -> *mut c_void {
        let f: extern "C" fn(*mut c_void, *mut c_void, *const *mut c_void, usize) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s, a, n)
    }

    /// `void f(id, SEL, id)`
    pub unsafe fn msg_void1(r: *mut c_void, s: *mut c_void, a: *mut c_void) {
        let f: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
            std::mem::transmute(objc_msgSend as *const ());
        f(r, s, a);
    }

    /// 注册选择器（调用方保证以 `\0` 结尾）。
    pub unsafe fn sel(z: &'static [u8]) -> *mut c_void {
        sel_registerName(z.as_ptr() as *const c_char)
    }

    /// `[NSString stringWithUTF8String:"..."]`
    pub unsafe fn nsstr(z: &'static [u8]) -> *mut c_void {
        let cls = objc_getClass(b"NSString\0".as_ptr() as *const c_char);
        if cls.is_null() {
            return std::ptr::null_mut();
        }
        msg_str(cls, sel(b"stringWithUTF8String:\0"), z.as_ptr() as *const c_char)
    }

    /// `id` → `String`（走 `UTF8String`，不认就丢）。
    pub unsafe fn ns_to_string(s: *mut c_void) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let p = msg0(s, sel(b"UTF8String\0")) as *const c_char;
        if p.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(p).to_str().ok().map(|s| s.to_string())
    }
}

#[cfg(target_os = "macos")]
unsafe fn arm_all_windows() {
    use ffi::*;
    use std::os::raw::c_char;

    let app_cls = objc_getClass(b"NSApplication\0".as_ptr() as *const c_char);
    if app_cls.is_null() {
        return;
    }
    let app = msg0(app_cls, sel(b"sharedApplication\0"));
    if app.is_null() {
        return;
    }
    let wins = msg0(app, sel(b"windows\0"));
    if wins.is_null() {
        return;
    }
    let n = msg_len(wins, sel(b"count\0")).min(12);
    let mut armed = 0u32;
    for i in 0..n {
        let win = msg_at(wins, sel(b"objectAtIndex:\0"), i);
        if win.is_null() {
            continue;
        }
        let view = msg0(win, sel(b"contentView\0"));
        armed += arm_view(view, 0);
    }
    if armed > 0 {
        ARMED.store(true, Ordering::Relaxed);
    }
    if std::env::var_os("JIANDU_DEBUG_DROP").is_some() && armed > 0 {
        eprintln!("[dropzone] armed {armed} views across {n} windows");
    }
}

/// 给这个视图的类补上拖放方法，并递归挂到子视图（返回挂成功的个数）。
///
/// AppKit 的拖放判定落在**光标下最深的那个视图**上，而子视图继承的是
/// NSView 的默认实现（一律「不接受」），所以光挂 `contentView` 不够——
/// GPUI 的视图常常嵌在内容视图里面。
#[cfg(target_os = "macos")]
unsafe fn arm_view(view: *mut std::os::raw::c_void, depth: usize) -> u32 {
    use ffi::*;
    use std::os::raw::{c_char, c_void};
    if view.is_null() || depth > 4 {
        return 0;
    }

    extern "C" fn imp_entered(
        _this: *mut c_void,
        _cmd: *mut c_void,
        _sender: *mut c_void,
    ) -> usize {
        if std::env::var_os("JIANDU_DEBUG_DROP").is_some() {
            eprintln!("[dropzone] draggingEntered");
        }
        1 // NSDragOperationCopy：光标显示「＋」
    }
    extern "C" fn imp_prepare(_this: *mut c_void, _cmd: *mut c_void, _sender: *mut c_void) -> i8 {
        1
    }
    extern "C" fn imp_perform(_this: *mut c_void, _cmd: *mut c_void, sender: *mut c_void) -> i8 {
        unsafe { collect_paths(sender) };
        if std::env::var_os("JIANDU_DEBUG_DROP").is_some() {
            eprintln!("[dropzone] performDragOperation queued={}", DROPPED.lock().map(|g| g.len()).unwrap_or(0));
        }
        1
    }

    let mut n = 0u32;
    let cls = object_getClass(view);
    if !cls.is_null() {
        // 类型编码：返回值 + self(@) + _cmd(:) + 实参(@)。
        //   Q = NSUInteger（NSDragOperation），B = BOOL。
        // 已存在的实现会返回 0（那是好事：说明类里本来就有）。
        class_addMethod(
            cls,
            sel(b"draggingEntered:\0"),
            imp_entered as *const c_void,
            b"Q@:@\0".as_ptr() as *const c_char,
        );
        class_addMethod(
            cls,
            sel(b"draggingUpdated:\0"),
            imp_entered as *const c_void,
            b"Q@:@\0".as_ptr() as *const c_char,
        );
        class_addMethod(
            cls,
            sel(b"prepareForDragOperation:\0"),
            imp_prepare as *const c_void,
            b"B@:@\0".as_ptr() as *const c_char,
        );
        class_addMethod(
            cls,
            sel(b"performDragOperation:\0"),
            imp_perform as *const c_void,
            b"B@:@\0".as_ptr() as *const c_char,
        );

        // 登记可接收的拖拽类型（每个视图各登记一次，幂等）。
        let s1 = nsstr(b"NSFilenamesPboardType\0");
        let s2 = nsstr(b"public.file-url\0");
        let nsarray = objc_getClass(b"NSArray\0".as_ptr() as *const c_char);
        if !s1.is_null() && !s2.is_null() && !nsarray.is_null() {
            let items: [*mut c_void; 2] = [s1, s2];
            let arr = msg_arr(nsarray, sel(b"arrayWithObjects:count:\0"), items.as_ptr(), 2);
            if !arr.is_null() {
                msg_void1(view, sel(b"registerForDraggedTypes:\0"), arr);
                if std::env::var_os("JIANDU_DEBUG_DROP").is_some() {
                    let cls_name = msg0(cls, sel(b"description\0"));
                    let nm = ns_to_string(cls_name).unwrap_or_default();
                    eprintln!("[dropzone] registered on {nm} (view={view:p})");
                }
            }
        }
        n += 1;
    }

    // 递归子视图。
    let subs = msg0(view, sel(b"subviews\0"));
    if !subs.is_null() {
        let cnt = msg_len(subs, sel(b"count\0")).min(16);
        for i in 0..cnt {
            let sv = msg_at(subs, sel(b"objectAtIndex:\0"), i);
            n += arm_view(sv, depth + 1);
        }
    }
    n
}

/// 从 `NSDraggingInfo` 里刮出文件路径（AppKit 主线程调用）。
///
/// 先走 `NSFilenamesPboardType`（Finder 多选拖拽都带这个兼容类型），
/// 拿不到再退 `public.file-url`（单文件 / 别的 App 拖出来时常见）。
#[cfg(target_os = "macos")]
unsafe fn collect_paths(sender: *mut std::os::raw::c_void) {
    use ffi::*;
    if sender.is_null() {
        return;
    }
    let pb = msg0(sender, sel(b"draggingPasteboard\0"));
    if pb.is_null() {
        return;
    }

    let key = nsstr(b"NSFilenamesPboardType\0");
    if !key.is_null() {
        let plist = msg1(pb, sel(b"propertyListForType:\0"), key);
        if !plist.is_null() {
            let n = msg_len(plist, sel(b"count\0"));
            for i in 0..n.min(64) {
                let item = msg_at(plist, sel(b"objectAtIndex:\0"), i);
                if let Some(s) = ns_to_string(item) {
                    let p = PathBuf::from(s);
                    if p.is_file() {
                        push_dropped(p);
                    }
                }
            }
        }
    }

    // 兜底：单文件 URL（`file:///Users/…`，要解百分号转义）。
    if DROPPED.lock().map(|g| g.is_empty()).unwrap_or(true) {
        let key = nsstr(b"public.file-url\0");
        if !key.is_null() {
            let s = msg1(pb, sel(b"stringForType:\0"), key);
            if let Some(s) = ns_to_string(s) {
                if let Some(p) = file_url_to_path(&s) {
                    if p.is_file() {
                        push_dropped(p);
                    }
                }
            }
        }
    }
}

/// `file:///Users/a%20b/x.docx` → `/Users/a b/x.docx`（只做百分号解码）。
fn file_url_to_path(s: &str) -> Option<PathBuf> {
    let rest = s.strip_prefix("file://")?;
    // `file:///Users/…` 三斜杠：主机名为空，路径从第一个 `/` 算起。
    // `file://localhost/…` 这种极少见，直接按第一个 `/` 之后算。
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => rest,
    };
    let bytes = path.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_url_decodes_percent_escapes() {
        assert_eq!(
            file_url_to_path("file:///Users/hj/Desktop/%E8%B5%84%E6%B2%BB.docx")
                .unwrap()
                .to_string_lossy(),
            "/Users/hj/Desktop/资治.docx"
        );
        assert_eq!(
            file_url_to_path("file:///Users/a%20b/x.docx")
                .unwrap()
                .to_string_lossy(),
            "/Users/a b/x.docx"
        );
        assert!(file_url_to_path("https://example.com/x").is_none());
    }

    #[test]
    fn queue_dedups_within_one_drop() {
        push_dropped(PathBuf::from("/tmp/a.docx"));
        push_dropped(PathBuf::from("/tmp/a.docx"));
        let got = take();
        assert_eq!(got.len(), 1);
        assert!(take().is_empty(), "take 必须把队列掏空");
    }
}
