//! Dock 复活：把「点 Dock 图标回不到主页面」这条断掉的线接上。
//!
//! ## 断在哪
//!
//! macOS 点 Dock 图标的标准路径是：先 `[NSApp activate]`，再问 delegate 的
//! `applicationShouldHandleReopen:hasVisibleWindows:`，**按返回值**决定要不要
//! 恢复窗口（取消最小化 / 重新前置）。
//!
//! GPUI 0.2.2 把这个方法注册成返回 `()`（`platform.rs:85` 的 `add_method` 与
//! `:1409` 的 `should_handle_reopen`），而 AppKit 的签名是 `- (BOOL)…`。返回
//! void 的函数交回去的那一个布尔寄存器是**残留垃圾**，于是 AppKit 时常读到
//! 「不要恢复」——窗口被最小化、或者藏在别的 app 后面时，点 Dock 只有菜单栏
//! 换了名字，主窗口死活不回来。
//!
//! 同一个方法只在 `!has_open_windows` 时才调 gpui 的 `reopen` 回调；窗口被红叉
//! 关掉后（进程还在跑）那条回调**根本没注册**，所以连「无窗口」这条路也是空的。
//!
//! ## 怎么接
//!
//! 不去改 gpui 的私有实现（那是别人家的代码，换版本就散架），而是另挂一条与
//! 返回值无关的线：监听 `NSApplicationDidBecomeActiveNotification`。app 被激活
//! **一定**会发这条通知，与 delegate 返回什么无关。收到就把主窗口重新做成 key
//! 并前置，最小化的先取消最小化。
//!
//! 「窗口已经被关掉」由 `main.rs` 里的 `cx.on_reopen` 重建——那是 gpui 唯一
//! 一条会给 `&mut App` 的复活入口。
//!
//! 两条线互不重叠：有窗口走通知，没窗口走 reopen。宁可两条都装上，也不要
//! 让用户对着一个点不动的图标猜是哪儿坏了。

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::CString;
    use std::ptr;
    use std::sync::OnceLock;

    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    use objc::{class, sel, sel_impl};

    /// 观察者类只注册一次（`ClassDecl::register` 对同名类第二次会失败）。
    static WAKE_CLASS: OnceLock<Option<&'static Class>> = OnceLock::new();

    /// 装上「激活即前置」的观察者。**主线程调**（`NSNotificationCenter` 不是
    /// 线程安全的）；开窗前后来都行——窗口还没建时它只是遍历到空列表。
    pub fn install() {
        let Some(cls) = *WAKE_CLASS.get_or_init(register_class) else {
            return;
        };
        unsafe {
            let observer: *mut Object = objc::msg_send![cls, new];
            if observer.is_null() {
                return;
            }
            let center: *mut Object = objc::msg_send![class!(NSNotificationCenter), defaultCenter];
            if center.is_null() {
                return;
            }
            let Ok(name_c) = CString::new("NSApplicationDidBecomeActiveNotification") else {
                return;
            };
            let name: *mut Object = objc::msg_send![
                class!(NSString),
                stringWithUTF8String: name_c.as_ptr()
            ];
            // 观察者不主动 retain：我们用 `new`（+1）且一辈子不 release，活到进程
            // 结束——一个几十字节的对象，泄漏它比提前释放安全一万倍。
            let _: () = objc::msg_send![
                center,
                addObserver: observer
                selector: objc::sel!(appDidBecomeActive:)
                name: name
                object: ptr::null_mut::<Object>()
            ];
        }
    }

    fn register_class() -> Option<&'static Class> {
        // 重名（理论上不会）就干脆不装，绝不在启动路上 panic。
        let mut decl = ClassDecl::new("JianDuWakeObserver", class!(NSObject))?;
        unsafe {
            decl.add_method(
                objc::sel!(appDidBecomeActive:),
                on_became_active as extern "C" fn(&Object, Sel, *mut Object),
            );
        }
        Some(decl.register())
    }

    extern "C" fn on_became_active(_this: &Object, _: Sel, _note: *mut Object) {
        bring_windows_front();
    }

    /// 把属于本进程的普通窗口全部前置，最小化的先取消最小化。
    ///
    /// 只处理 `canBecomeMainWindow == true` 的窗口：查找面板、保存框这类
    /// `NSPanel` 不该因为「切回 app」就抢焦——它们有各自的打开时机。
    ///
    /// ⚠️ 这段在 AppKit 的通知回调里跑，处在 **ObjC 调用栈**上，**绝不许 panic**：
    ///    一旦 panic 跨过 FFI 边界就是 `panic_cannot_unwind` 直接 abort，表现是
    ///    「点一下 Dock 整个 app 没了」。所以这里只做判空与 msg_send，不 unwrap、
    ///    不索引、不做可能失败的分配。
    fn bring_windows_front() {
        unsafe {
            let app: *mut Object = objc::msg_send![class!(NSApplication), sharedApplication];
            if app.is_null() {
                return;
            }
            let windows: *mut Object = objc::msg_send![app, windows];
            if windows.is_null() {
                return;
            }
            let count: usize = objc::msg_send![windows, count];
            for i in 0..count {
                let w: *mut Object = objc::msg_send![windows, objectAtIndex: i];
                if w.is_null() {
                    continue;
                }
                let can_be_main: bool = objc::msg_send![w, canBecomeMainWindow];
                if !can_be_main {
                    continue;
                }
                let minimized: bool = objc::msg_send![w, isMiniaturized];
                if minimized {
                    // sender 给 nil：我们不是某个按钮触发的。
                    let _: () = objc::msg_send![w, deminiaturize: ptr::null_mut::<Object>()];
                }
                let _: () = objc::msg_send![w, makeKeyAndOrderFront: ptr::null_mut::<Object>()];
            }
            // Dock 点击本来就意味着「我要用这个 app」，显式压过别的 app 符合预期。
            // `activateIgnoringOtherApps:` 虽已废弃，仍是 macOS 12 起唯一可靠的一招。
            let _: () = objc::msg_send![app, activateIgnoringOtherApps: true];
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::install;

/// 非 macOS：什么都不做（GPUI 的 Windows / Linux 后端本就有自己的 reopen 路径）。
#[cfg(not(target_os = "macos"))]
pub fn install() {}
