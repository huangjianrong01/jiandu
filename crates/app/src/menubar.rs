//! macOS 顶部状态栏常驻（Menu Bar Extra，v1.9.44）。
//!
//! 参考「写匠」等现代写作软件：在系统状态栏注册一枚极简图标，点击弹出
//! 原生下拉菜单（NSMenu，自带 vibrancy 毛玻璃感），提供：
//!   · 顶部信息：今日阅卷：X 字（复用 [`Root::poll_menu_intents`] 刷进来的数据）
//!   · 打开工作台（唤起主窗口并切到笺校工作台）
//!   · 打开书架（唤起主窗口的书架界面）
//!   · 最近项目（二级子菜单，列最近阅读的 5 本）
//!   · 检查更新 / 退出笺读
//!
//! # 纪律（同 dropzone / revive）
//!
//! AppKit 的菜单回调跑在 ObjC 调用栈上，**绝不直碰 GPUI 实体**——只把意图塞进
//! 共享队列，由 [`Root::poll_menu_intents`] 每帧在主线程取空、执行。回调里动实体
//! = 必崩（`panic_cannot_unwind` 直接 abort）。
//!
//! # 图标
//!
//! 用菜单栏**文字「笺」**而非图片：系统自动按深浅色适配（黑底变白、白底变黑），
//! 既不引新依赖，又绕开「模板图」的 FFI 绘制坑（AppKit 绘文本只需 id 级消息，
//! 不碰任何 `NSRect`/`NSSize` 之类需要手动 `Encode` 的结构体）。

use std::ffi::CString;
use std::ptr;
use std::sync::{Mutex, OnceLock};

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, sel, sel_impl};

/// 菜单点出来的意图（与 [`Root::poll_menu_intents`] 的匹配一致）。
#[derive(Clone, Copy, Debug)]
pub enum MenuIntent {
    OpenShelf,
    Collation,
    OpenRecent(usize),
    CheckUpdate,
    Quit,
}

/// 菜单需要的动态数据：GPUI 侧每帧刷进来，AppKit 侧开菜单时读。
pub struct MenuData {
    /// 今日读过的字数（标点不计）。
    pub today_chars: usize,
    /// (书名, 书架序号) —— 序号交给 `Root::open_book`。
    pub recents: Vec<(String, usize)>,
}

/// 菜单意图队列（AppKit 回调写、GPUI 主线程读）。
static INTENTS: Mutex<Vec<MenuIntent>> = Mutex::new(Vec::new());

/// 菜单动态数据（GPUI 主线程写、AppKit 开菜单时读）。
static MENU_DATA: Mutex<MenuData> = Mutex::new(MenuData {
    today_chars: 0,
    recents: Vec::new(),
});

/// 取走这一帧攒下的菜单意图（只会成功一次）。
pub fn take_intents() -> Vec<MenuIntent> {
    INTENTS
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// GPUI 侧每帧刷入「今日字数 / 最近打开」，供下次开菜单时读。
pub fn refresh_menu_data(data: MenuData) {
    if let Ok(mut g) = MENU_DATA.lock() {
        *g = data;
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    // 菜单项 tag 编码：与 `intent_from_tag` 对应。
    const TAG_SHELF: i64 = 1;
    const TAG_COLLATION: i64 = 2;
    const TAG_UPDATE: i64 = 3;
    const TAG_QUIT: i64 = 4;
    const TAG_RECENT_BASE: i64 = 100; // 100 + book_idx

    static STATUS_ITEM: OnceLock<OpaquePtr> = OnceLock::new();
    static ITEM_TARGET: OnceLock<OpaquePtr> = OnceLock::new();
    static MENU_TARGET_CLASS: OnceLock<&'static Class> = OnceLock::new();
    static ITEM_TARGET_CLASS: OnceLock<&'static Class> = OnceLock::new();

    /// 裸 ObjC 指针不是 `Sync`，不能直进 `static`。包一层新类型并手动保证
    /// 「只主线程碰」语义（与 `STATUS_ITEM` 等活到进程结束、不跨线程共享的纪律一致）。
    struct OpaquePtr(*mut Object);
    unsafe impl Send for OpaquePtr {}
    unsafe impl Sync for OpaquePtr {}

    /// 注册两个回调类（只一次）。`ClassDecl::new` 在类名已存在时返回 `None`，
    /// 那种极端情况下直接取回已注册的类——绝不 panic（`OnceLock::get_or_init`
    /// 要求闭包返回 `&'static Class`，不能是 `Option`）。
    fn register_menu_class() -> &'static Class {
        match ClassDecl::new("JianDuMenuShowTarget", class!(NSObject)) {
            Some(mut decl) => {
                unsafe {
                    decl.add_method(
                        sel!(showMenu:),
                        show_menu as extern "C" fn(&Object, Sel, *mut Object),
                    );
                }
                decl.register()
            }
            None => class!(JianDuMenuShowTarget),
        }
    }

    fn register_item_class() -> &'static Class {
        match ClassDecl::new("JianDuMenuItemTarget", class!(NSObject)) {
            Some(mut decl) => {
                unsafe {
                    decl.add_method(
                        sel!(menuAction:),
                        item_action as extern "C" fn(&Object, Sel, *mut Object),
                    );
                }
                decl.register()
            }
            None => class!(JianDuMenuItemTarget),
        }
    }

    unsafe fn nsstring(s: &str) -> *mut Object {
        let c = match CString::new(s) {
            Ok(c) => c,
            Err(_) => return ptr::null_mut(),
        };
        objc::msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
    }

    /// 把意图塞进队列（永远安全：只碰锁与枚举）。
    unsafe fn push_intent(intent: MenuIntent) {
        if let Ok(mut g) = INTENTS.lock() {
            g.push(intent);
        }
    }

    fn intent_from_tag(tag: i64) -> Option<MenuIntent> {
        match tag {
            TAG_SHELF => Some(MenuIntent::OpenShelf),
            TAG_COLLATION => Some(MenuIntent::Collation),
            TAG_UPDATE => Some(MenuIntent::CheckUpdate),
            TAG_QUIT => Some(MenuIntent::Quit),
            t if t >= TAG_RECENT_BASE => {
                Some(MenuIntent::OpenRecent((t - TAG_RECENT_BASE) as usize))
            }
            _ => None,
        }
    }

    // ── 菜单项的 action 目标 ─────────────────────────────────────────────
    extern "C" fn item_action(_this: &Object, _cmd: Sel, sender: *mut Object) {
        unsafe {
            if sender.is_null() {
                return;
            }
            let tag: i64 = objc::msg_send![sender, tag];
            if let Some(intent) = intent_from_tag(tag) {
                push_intent(intent);
            }
        }
    }

    // ── 状态栏按钮的 action 目标：弹出菜单 ───────────────────────────────
    extern "C" fn show_menu(_this: &Object, _cmd: Sel, _sender: *mut Object) {
        unsafe {
            let item = match STATUS_ITEM.get() {
                Some(&OpaquePtr(p)) if !p.is_null() => p,
                _ => return,
            };
            let menu = build_menu();
            if menu.is_null() {
                return;
            }
            // 弹出（模态，返回即关闭）；关闭后释放这一棵菜单树，避免每次点击都泄漏。
            let _: () = objc::msg_send![item, popUpStatusItemMenu: menu];
            let _: () = objc::msg_send![menu, release];
        }
    }

    /// 构造当次菜单（带最新数据）。调用方负责 release。
    unsafe fn build_menu() -> *mut Object {
        let menu: *mut Object = objc::msg_send![class!(NSMenu), new];
        if menu.is_null() {
            return ptr::null_mut();
        }
        // 关掉自动启用判断：启用状态全由我们 `setEnabled:` 决定。
        let _: () = objc::msg_send![menu, setAutoenablesItems: false];

        // 顶部信息：今日阅卷 X 字（极小灰字，禁用项，不可点）。
        let today = MENU_DATA
            .lock()
            .map(|g| g.today_chars)
            .unwrap_or(0);
        let header_label = if today > 0 {
            format!("今日阅卷：{} 字", today)
        } else {
            "今日尚未阅卷".to_string()
        };
        let header = make_item(&header_label, true);
        if !header.is_null() {
            let attr = attributed_gray(&header_label, 10.0);
            if !attr.is_null() {
                let _: () = objc::msg_send![header, setAttributedTitle: attr];
            }
            let _: () = objc::msg_send![menu, addItem: header];
        }

        // 打开工作台（v1.9.73 正名：以前叫「快速笺校」，用户口语里一律叫「工作台」）。
        let collation = make_item("打开工作台", false);
        if !collation.is_null() {
            let _: () = objc::msg_send![menu, addItem: collation];
        }
        // 打开书架。
        let shelf = make_item("打开书架", false);
        if !shelf.is_null() {
            let _: () = objc::msg_send![menu, addItem: shelf];
        }

        // 最近项目（二级子菜单；v1.9.73 由「最近打开」正名）。
        let recent_parent = make_item("最近项目", false);
        if !recent_parent.is_null() {
            let sub = build_recents_submenu();
            if !sub.is_null() {
                let _: () = objc::msg_send![recent_parent, setSubmenu: sub];
            }
            let _: () = objc::msg_send![menu, addItem: recent_parent];
        }

        // 分割线。
        let sep: *mut Object = objc::msg_send![class!(NSMenuItem), separatorItem];
        if !sep.is_null() {
            let _: () = objc::msg_send![menu, addItem: sep];
        }

        // 检查更新 / 退出。
        let update = make_item("检查更新", false);
        if !update.is_null() {
            let _: () = objc::msg_send![menu, addItem: update];
        }
        let quit = make_item("退出笺读", false);
        if !quit.is_null() {
            let _: () = objc::msg_send![menu, addItem: quit];
        }

        menu
    }

    /// 造一枚菜单项。禁用的项（如顶部信息）置 `disabled = true`。
    /// 所有项都挂上 `menuAction:` 目标；禁用项点不动，不会发意图。
    unsafe fn make_item(title: &str, disabled: bool) -> *mut Object {
        let title_obj = nsstring(title);
        if title_obj.is_null() {
            return ptr::null_mut();
        }
        let empty = nsstring("");
        if empty.is_null() {
            return ptr::null_mut();
        }
        let item: *mut Object = objc::msg_send![class!(NSMenuItem), alloc];
        let action: Sel = sel!(menuAction:);
        let item: *mut Object = objc::msg_send![
            item,
            initWithTitle: title_obj
            action: action
            keyEquivalent: empty
        ];
        if item.is_null() {
            return ptr::null_mut();
        }
        if let Some(&OpaquePtr(target)) = ITEM_TARGET.get() {
            if !target.is_null() {
                let _: () = objc::msg_send![item, setTarget: target];
            }
        }
        // tag 编码：禁用项(顶部信息)用 0（映射不到任何意图）；其余由各调用点 setTag。
        let _: () = objc::msg_send![item, setTag: 0i64];
        if disabled {
            let _: () = objc::msg_send![item, setEnabled: false];
        }
        item
    }

    /// 给「最近打开」造二级子菜单。
    unsafe fn build_recents_submenu() -> *mut Object {
        let sub: *mut Object = objc::msg_send![class!(NSMenu), new];
        if sub.is_null() {
            return ptr::null_mut();
        }
        let _: () = objc::msg_send![sub, setAutoenablesItems: false];
        let recents: Vec<(String, usize)> = MENU_DATA
            .lock()
            .map(|g| g.recents.clone())
            .unwrap_or_default();
        let recents: Vec<_> = recents.into_iter().take(5).collect();
        if recents.is_empty() {
            let none = make_item("（暂无最近阅读）", true);
            if !none.is_null() {
                let _: () = objc::msg_send![sub, addItem: none];
            }
            return sub;
        }
        for (title, idx) in recents {
            let item = make_item(&title, false);
            if !item.is_null() {
                let _: () = objc::msg_send![item, setTag: TAG_RECENT_BASE + idx as i64];
                let _: () = objc::msg_send![sub, addItem: item];
            }
        }
        sub
    }

    /// 灰色小字属性串（NSFont + 半透明灰），用于顶部信息那种「极小灰字」。
    unsafe fn attributed_gray(title: &str, size: f64) -> *mut Object {
        let title_obj = nsstring(title);
        if title_obj.is_null() {
            return ptr::null_mut();
        }
        let dict: *mut Object = objc::msg_send![class!(NSMutableDictionary), dictionary];
        if dict.is_null() {
            return ptr::null_mut();
        }
        let font: *mut Object = objc::msg_send![class!(NSFont), systemFontOfSize: size];
        let font_key = nsstring("NSFontAttributeName");
        if !font.is_null() && !font_key.is_null() {
            let _: () = objc::msg_send![dict, setObject: font forKey: font_key];
        }
        let color: *mut Object =
            objc::msg_send![class!(NSColor), colorWithCalibratedWhite: 0.5f64 alpha: 1.0f64];
        let color_key = nsstring("NSForegroundColorAttributeName");
        if !color.is_null() && !color_key.is_null() {
            let _: () = objc::msg_send![dict, setObject: color forKey: color_key];
        }
        let attr: *mut Object = objc::msg_send![class!(NSAttributedString), alloc];
        let attr: *mut Object = objc::msg_send![attr, initWithString: title_obj attributes: dict];
        attr
    }

    /// 装上状态栏图标 + 下拉菜单。**主线程调**（NSStatusBar 不是线程安全的）。
    pub fn install() {
        let menu_cls = *MENU_TARGET_CLASS.get_or_init(register_menu_class);
        let item_cls = *ITEM_TARGET_CLASS.get_or_init(register_item_class);
        let menu_target: *mut Object = unsafe { objc::msg_send![menu_cls, new] };
        let item_target: *mut Object = unsafe { objc::msg_send![item_cls, new] };
        if menu_target.is_null() || item_target.is_null() {
            return;
        }
        ITEM_TARGET.set(OpaquePtr(item_target)).ok();
        unsafe {
            let status_bar: *mut Object = objc::msg_send![class!(NSStatusBar), systemStatusBar];
            if status_bar.is_null() {
                return;
            }
            // NSVariableStatusItemLength = -1.0（CGFloat）。
            let item: *mut Object = objc::msg_send![status_bar, statusItemWithLength: -1.0f64];
            if item.is_null() {
                return;
            }
            let button: *mut Object = objc::msg_send![item, button];
            if button.is_null() {
                return;
            }
            // 极简图标：菜单栏文字「笺」，系统自动深浅色适配（黑底变白 / 白底变黑）。
            let title = nsstring("笺");
            if !title.is_null() {
                let _: () = objc::msg_send![button, setTitle: title];
            }
            // 点按钮 → 弹菜单（不把菜单直接绑在 item 上，以便每次都重建带最新数据）。
            let _: () = objc::msg_send![button, setTarget: menu_target];
            let _: () = objc::msg_send![button, setAction: sel!(showMenu:)];
            // 泄漏 item：活到进程结束，比提前释放安全一万倍。
            STATUS_ITEM.set(OpaquePtr(item)).ok();
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::install;

/// 非 macOS：什么都不做（GPUI 的 Windows / Linux 后端无状态栏概念）。
///
/// 注意：`take_intents` / `refresh_menu_data` 在文件顶层（无 `#[cfg]`）已有
/// 全平台实现——它们只读写 `INTENTS` / `MENU_DATA` 两个 `Mutex` 静态，非
/// macOS 下无人 `push_intent`，`take_intents` 自然空返、`refresh_menu_data`
/// 写入也无副作用，无需再重复定义（否则非 mac 构建会触发重复定义 E0428）。
#[cfg(not(target_os = "macos"))]
pub fn install() {}
