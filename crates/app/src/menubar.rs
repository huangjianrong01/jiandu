//! macOS 顶部状态栏常驻（Menu Bar Extra，v1.9.44；v1.9.83 改版）。
//!
//! 参考「写匠」等现代写作软件：在系统状态栏注册一枚极简图标，点击弹出
//! 原生下拉菜单（NSMenu，自带 vibrancy 毛玻璃、圆角、投影），提供：
//!   · 顶部信息：今日阅读 X 分钟（灰字，不可点）
//!   · 继续阅读：《最近那本》
//!   · 开始 / 停止听书（接 TTS，见 `crate::tts`）
//!   · 高对比护眼模式（勾选态，黑底黄字）
//!   · 打开工作台 / 导入异体字表…
//!   · 最近阅读（二级子菜单，列最近读的 5 本 + 全部书架）
//!   · 重启并更新 / 退出笺读
//!
//! # 为什么不用 `tray-icon` / `muda`
//!
//! 本项目是 **GPUI 0.2.2**（Zed 的框架），不是 Tauri / Egui。GPUI 0.2.2
//! 里没有任何托盘 API（`tray` / `status item` 全仓零命中），所以「原生
//! 支持吗」的答案是**不支持**。
//!
//! 但引入 crate 也不划算：`tray-icon` + `muda` 在 macOS 上就是
//! `NSStatusBar` + `NSMenu` 的一层薄包装（它们自己不实现任何东西），
//! 而这两样本文件已经手写完了，跑了一年多。引进来要付三笔账：
//!   ① 两个新依赖 + 一串平台相关传递依赖；
//!   ② `muda` 的 `MenuEvent::receiver()` 是 crossbeam channel，还得自己
//!      桥回 GPUI 主线程；
//!   ③ 与现有 GPUI 窗口的菜单（⌘Q / ⌘W 那一套）抢同一份 NSApp 菜单所有权。
//! 故：不引，扩展现有。
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuIntent {
    OpenShelf,
    Collation,
    /// 打开书架上第 `n` 本。
    OpenRecent(usize),
    /// 继续阅读：打开最近读的那本，并直接进正文。
    ///
    /// 与 [`Self::OpenRecent`] 分开不是多此一举：这一项**必须**落到阅读
    /// 界面（菜单栏的首要用途就是「接着上次读」），而 `OpenRecent` 只保证
    /// 「打开这本书」，落在哪一屏由 `open_book` 自己决定。
    ContinueReading,
    /// 开始 / 停止听书（复用 `Root::set_tts_active`）。
    ToggleTts,
    /// 高对比护眼模式（黑底黄字，复用 `Root::toggle_huyan`）。
    ToggleHuyan,
    /// 导入外部异体字 / 繁简映射表（v1.9.78）。
    ImportVariants,
    CheckUpdate,
    /// 重启并更新（v1.9.83）：落盘 → 重新拉起 .app → 退出本进程。
    RestartUpdate,
    Quit,
}

/// 菜单需要的动态数据：GPUI 侧每帧刷进来，AppKit 侧开菜单时读。
#[derive(Clone)]
pub struct MenuData {
    /// 今日累计阅读秒数（`library::ReadDay::secs`）。
    pub today_secs: u64,
    /// 听书是否正在念（决定那行写「开始」还是「停止」）。
    pub tts_on: bool,
    /// 当前是不是护眼档（决定那一项打不打勾）。
    pub huyan_on: bool,
    /// 最近读的那本（「继续阅读」用）：(书名, 书架序号)。
    pub recent: Option<(String, usize)>,
    /// 全部最近读过，按 `last_read` 降序（子菜单只取前 5）。
    pub recents: Vec<(String, usize)>,
}

impl Default for MenuData {
    fn default() -> Self {
        Self {
            today_secs: 0,
            tts_on: false,
            huyan_on: false,
            recent: None,
            recents: Vec::new(),
        }
    }
}

/// 菜单意图队列（AppKit 回调写、GPUI 主线程读）。
static INTENTS: Mutex<Vec<MenuIntent>> = Mutex::new(Vec::new());

/// 队列里有没有东西（原子镜像）。
///
/// 存在的唯一理由：GPUI 的帧源不能每 100ms 去抢一次 `INTENTS` 的锁，也不该
/// 在没意图时无谓重绘。一个 `AtomicBool` 足够回答「要不要去看一眼」。
/// [`take_intents`] 取空时置回 false——注意是**取完之后**置，不是取之前：
/// 中间要是又有一个入队，宁可多跑一拍也不能漏。
static PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 菜单动态数据（GPUI 主线程写、AppKit 开菜单时读）。
static MENU_DATA: Mutex<Option<MenuData>> = Mutex::new(None);

/// 取走这一帧攒下的菜单意图（只会成功一次）。
pub fn take_intents() -> Vec<MenuIntent> {
    // 顺序很讲究：**先清标记、再取队列**。
    //   先清后取：中间若有一次入队，它落在队列里，我们这一趟顺手取走——不丢。
    //   先取后清：中间那次入队被取走了，标记却被清成 false —— 那一拍里
    //   队列是空的、标记是 false，可它明明发生过；若之后没有别的帧源，
    //   这个意图就永远没人处理（这正是「点了没反应」最难查的一种）。
    PENDING.store(false, std::sync::atomic::Ordering::SeqCst);
    INTENTS
        .lock()
        .map(|mut g| std::mem::take(&mut *g))
        .unwrap_or_default()
}

/// 队列里有没有待处理的意图（给 `Root::ensure_menu_pump` 用）。
pub fn has_pending() -> bool {
    PENDING.load(std::sync::atomic::Ordering::Relaxed)
}

/// GPUI 侧每帧刷入菜单数据，供下次开菜单时读。
pub fn refresh_menu_data(data: MenuData) {
    if let Ok(mut g) = MENU_DATA.lock() {
        *g = Some(data);
    }
}

/// 顶部那行灰字：「今日阅读 X 分钟」。
///
/// 三档，都在 [`tests`] 里钉死：
///   * `0` → 「今日尚未阅读」——写「今日阅读 0 分钟」像在数落人；
///   * `<60` → 「今日阅读不足 1 分钟」——**不四舍五入成 0**，否则刚坐下
///     读了两叶的人看到的和没读一样；
///   * 满一小时改说「H 小时 M 分钟」，四位数分钟数在菜单里太蠢。
pub fn fmt_today(secs: u64) -> String {
    if secs == 0 {
        return "今日尚未阅读".to_string();
    }
    if secs < 60 {
        return "今日阅读不足 1 分钟".to_string();
    }
    let mins = secs / 60;
    if mins >= 60 {
        let h = mins / 60;
        let m = mins % 60;
        if m == 0 {
            format!("今日阅读 {h} 小时")
        } else {
            format!("今日阅读 {h} 小时 {m} 分钟")
        }
    } else {
        format!("今日阅读 {mins} 分钟")
    }
}

/// 「继续阅读」那一行的标题。没有最近阅读时给纯「继续阅读」（并置灰）。
pub fn continue_label(title: Option<&str>) -> String {
    match title {
        Some(t) if !t.is_empty() => format!("继续阅读：《{t}》"),
        _ => "继续阅读".to_string(),
    }
}

/// 听书那一行的标题——**用动词而不是勾**：这一项点一下是「做一件事」，
/// 不是「开一个状态」，用户不该靠去看有没有勾来判断点下去会怎样。
pub fn tts_label(on: bool) -> &'static str {
    if on {
        "停止听书"
    } else {
        "开始听书"
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    // 菜单项 tag 编码：与 `intent_from_tag` 对应。
    //
    // ⚠️ v1.9.78 的血泪：造完菜单项**必须 `setTag`**，否则 tag 全是 0，
    // `intent_from_tag(0)` 返回 None ——菜单看得见、点了没反应。
    // 现在的办法是**把 tag 变成 `make_item` 的必填参数**，从结构上让人
    // 忘不掉（而不是靠注释提醒）。
    const TAG_SHELF: i64 = 1;
    const TAG_COLLATION: i64 = 2;
    const TAG_UPDATE: i64 = 3;
    const TAG_QUIT: i64 = 4;
    const TAG_VARIANTS: i64 = 5;
    const TAG_CONTINUE: i64 = 6;
    const TAG_TTS: i64 = 7;
    const TAG_HUYAN: i64 = 8;
    const TAG_RESTART: i64 = 9;
    const TAG_RECENT_BASE: i64 = 100; // 100 + book_idx

    /// `NSControlStateValueOn`。
    const STATE_ON: isize = 1;
    /// `NSControlStateValueOff`。
    const STATE_OFF: isize = 0;

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
                    // 开发钩子用：按 tag 模拟「点这一项」（见 `fire_tag`）。
                    decl.add_method(
                        sel!(fireTag:),
                        fire_tag as extern "C" fn(&Object, Sel, *mut Object),
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
            // 入队之后再抬标记：先抬标记再入队的话，中间那一拍被取空，
            // 就会「标记为真、队列为空」，白跑一趟（不丢数据，但没意义）。
            PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn intent_from_tag(tag: i64) -> Option<MenuIntent> {
        match tag {
            TAG_SHELF => Some(MenuIntent::OpenShelf),
            TAG_COLLATION => Some(MenuIntent::Collation),
            TAG_UPDATE => Some(MenuIntent::CheckUpdate),
            TAG_QUIT => Some(MenuIntent::Quit),
            TAG_VARIANTS => Some(MenuIntent::ImportVariants),
            TAG_CONTINUE => Some(MenuIntent::ContinueReading),
            TAG_TTS => Some(MenuIntent::ToggleTts),
            TAG_HUYAN => Some(MenuIntent::ToggleHuyan),
            TAG_RESTART => Some(MenuIntent::RestartUpdate),
            t if t >= TAG_RECENT_BASE => {
                Some(MenuIntent::OpenRecent((t - TAG_RECENT_BASE) as usize))
            }
            _ => None,
        }
    }

    /// 只给 `mod tests` 用：把 tag → 意图的映射露出来断言。
    ///
    /// 不直接 `pub(crate) intent_from_tag` 是因为运行期只有 AppKit 调它，
    /// 平时不该有第二个调用者——留一个 test 专用口子，比放开可见性干净。
    #[cfg(test)]
    pub(super) fn intent_from_tag_for_test(tag: i64) -> Option<MenuIntent> {
        intent_from_tag(tag)
    }

    /// 当次菜单要用的数据（缺省值 = 冷启动，书架空）。
    fn data() -> MenuData {
        MENU_DATA
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_default()
    }

    // ── 菜单项的 action 目标 ─────────────────────────────────────────────
    extern "C" fn item_action(_this: &Object, _cmd: Sel, sender: *mut Object) {
        unsafe {
            if sender.is_null() {
                return;
            }
            let tag: i64 = objc::msg_send![sender, tag];
            let dbg = std::env::var("JIANDU_MENU_DEBUG").is_ok();
            if dbg {
                eprintln!("[menu] action 回调 tag={tag}");
            }
            if let Some(intent) = intent_from_tag(tag) {
                push_intent(intent);
                if dbg {
                    eprintln!("[menu] 入队 {intent:?}");
                }
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

    /// 造一条分割线（`NSMenuItem::separatorItem`）。
    unsafe fn add_sep(menu: *mut Object) {
        let sep: *mut Object = objc::msg_send![class!(NSMenuItem), separatorItem];
        if !sep.is_null() {
            let _: () = objc::msg_send![menu, addItem: sep];
        }
    }

    /// **开发钩子：按 tag 模拟「点这一项」**（`JIANDU_MENU_FIRE=<tag>`）。
    ///
    /// 为什么需要它：菜单项「点下去有没有用」恰恰是这类代码最容易坏、又最难
    /// 自查的地方（v1.9.78 那次 tag 全是 0，菜单看得见、点了没反应，不崩不报错）。
    /// 而真实点击要靠合成鼠标事件，过不了 TCC。
    ///
    /// 这里走的是 **`NSApplication sendAction:to:from:`**——菜单跟踪循环内部
    /// 也是这么发的，所以除了「鼠标按下」那一下，**整条链路都是真的**：
    /// 真菜单项、真 target/action、真回调、真意图队列、真 `poll_menu_intents`。
    unsafe fn dispatch_tag(menu: *mut Object, want: i64, dbg: bool) -> bool {
        let arr: *mut Object = objc::msg_send![menu, itemArray];
        if arr.is_null() {
            return false;
        }
        let n: usize = objc::msg_send![arr, count];
        for i in 0..n {
            let it: *mut Object = objc::msg_send![arr, objectAtIndex: i];
            if it.is_null() {
                continue;
            }
            let tag: i64 = objc::msg_send![it, tag];
            if tag == want && want != 0 {
                let action: Sel = objc::msg_send![it, action];
                let target: *mut Object = objc::msg_send![it, target];
                let app: *mut Object =
                    objc::msg_send![class!(NSApplication), sharedApplication];
                let ok: bool = objc::msg_send![app, sendAction: action to: target from: it];
                if dbg {
                    eprintln!("[menu] fire tag={want} ok={ok} target={target:p}");
                }
                return ok;
            }
            // 下一层（「最近阅读」的子项：tag 100+）。
            let sub: *mut Object = objc::msg_send![it, submenu];
            if !sub.is_null() && dispatch_tag(sub, want, dbg) {
                return true;
            }
        }
        if dbg {
            eprintln!("[menu] fire tag={want} 找不到这一项");
        }
        false
    }

    /// 延迟到点后被调：建菜单 → 派发指定 tag 的动作 → 释放。
    extern "C" fn fire_tag(_this: &Object, _cmd: Sel, num: *mut Object) {
        unsafe {
            if num.is_null() {
                return;
            }
            let want: i64 = objc::msg_send![num, longLongValue];
            let menu = build_menu();
            if menu.is_null() {
                return;
            }
            let _ = dispatch_tag(menu, want, std::env::var("JIANDU_MENU_DEBUG").is_ok());
            let _: () = objc::msg_send![menu, release];
        }
    }

    /// 构造当次菜单（带最新数据）。调用方负责 release。
    ///
    /// 顺序严格按产品给的结构；`打开工作台` / `导入异体字表` 是原有功能，
    /// 结构里没有但删了就没入口，故并进中段（仍是一条分割线之内）。
    unsafe fn build_menu() -> *mut Object {
        let menu: *mut Object = objc::msg_send![class!(NSMenu), new];
        if menu.is_null() {
            return ptr::null_mut();
        }
        // 关掉自动启用判断：启用状态全由我们 `setEnabled:` 决定
        // （灰字表头 + 「书架空时禁用继续阅读」都靠它）。
        let _: () = objc::msg_send![menu, setAutoenablesItems: false];

        let d = data();

        // ① 顶部状态：今日阅读 X 分钟（灰字，禁用，不可点）。
        let head = fmt_today(d.today_secs);
        let header = make_item(&head, 0, true);
        if !header.is_null() {
            let attr = attributed_gray(&head, 10.0);
            if !attr.is_null() {
                let _: () = objc::msg_send![header, setAttributedTitle: attr];
            }
            let _: () = objc::msg_send![menu, addItem: header];
        }

        add_sep(menu);

        // ② 继续阅读：《最近那本》——没有最近阅读则置灰（点不动，不发意图）。
        let cont_title = continue_label(d.recent.as_ref().map(|(t, _)| t.as_str()));
        let cont = make_item(&cont_title, TAG_CONTINUE, d.recent.is_none());
        if !cont.is_null() {
            let _: () = objc::msg_send![menu, addItem: cont];
        }

        // ③ 开始 / 停止听书。没有书时置灰——没正文可念。
        let tts = make_item(tts_label(d.tts_on), TAG_TTS, d.recent.is_none());
        if !tts.is_null() {
            let _: () = objc::msg_send![menu, addItem: tts];
        }

        // ④ 高对比护眼模式（黑底黄字）。勾选态由 `setState:` 表达——
        //    这是 AppKit 的原生约定，比分一个「退出护眼」的子项更省地方。
        let huyan = make_item("高对比护眼模式", TAG_HUYAN, false);
        if !huyan.is_null() {
            let _: () =
                objc::msg_send![huyan, setState: if d.huyan_on { STATE_ON } else { STATE_OFF }];
            let _: () = objc::msg_send![menu, addItem: huyan];
        }

        add_sep(menu);

        // ⑤ 原有入口（结构里没有，但不能丢）：工作台 / 导入异体字表。
        let collation = make_item("打开工作台", TAG_COLLATION, false);
        if !collation.is_null() {
            let _: () = objc::msg_send![menu, addItem: collation];
        }
        let variants = make_item("导入异体字表…", TAG_VARIANTS, false);
        if !variants.is_null() {
            let _: () = objc::msg_send![menu, addItem: variants];
        }

        // ⑥ 最近阅读（二级子菜单，系统会自动画「>」并在悬停时展开）。
        let recent_parent = make_item("最近阅读", 0, false);
        if !recent_parent.is_null() {
            let sub = build_recents_submenu(&d);
            if !sub.is_null() {
                let _: () = objc::msg_send![recent_parent, setSubmenu: sub];
            }
            let _: () = objc::msg_send![menu, addItem: recent_parent];
        }

        add_sep(menu);

        // ⑦ 重启并更新 / 退出。
        let restart = make_item("重启并更新", TAG_RESTART, false);
        if !restart.is_null() {
            let _: () = objc::msg_send![menu, addItem: restart];
        }
        let quit = make_item("退出笺读", TAG_QUIT, false);
        if !quit.is_null() {
            let _: () = objc::msg_send![menu, addItem: quit];
        }

        menu
    }

    /// 造一枚菜单项。
    ///
    /// **`tag` 是必填参数**（v1.9.83）：v1.9.78 那次「四项全忘 setTag、
    /// 点了没反应」的根因是 tag 靠调用点自己补，漏一个就是死的。现在
    /// 造即带 tag，`0` 只给「永远不发意图」的项（灰表头、带子菜单的父项）。
    ///
    /// 所有项都挂上 `menuAction:` 目标；`disabled` 的项点不动，不发意图。
    unsafe fn make_item(title: &str, tag: i64, disabled: bool) -> *mut Object {
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
        let _: () = objc::msg_send![item, setTag: tag];
        if disabled {
            let _: () = objc::msg_send![item, setEnabled: false];
        }
        item
    }

    /// 给「最近阅读」造二级子菜单：最近 5 本 + 一条分割线 + 全部书架。
    unsafe fn build_recents_submenu(d: &MenuData) -> *mut Object {
        let sub: *mut Object = objc::msg_send![class!(NSMenu), new];
        if sub.is_null() {
            return ptr::null_mut();
        }
        let _: () = objc::msg_send![sub, setAutoenablesItems: false];
        if d.recents.is_empty() {
            let none = make_item("（暂无最近阅读）", 0, true);
            if !none.is_null() {
                let _: () = objc::msg_send![sub, addItem: none];
            }
            return sub;
        }
        for (title, idx) in d.recents.iter().take(5) {
            let item = make_item(title, TAG_RECENT_BASE + *idx as i64, false);
            if !item.is_null() {
                let _: () = objc::msg_send![sub, addItem: item];
            }
        }
        // 「全部书架」：子菜单里最后一格。它不进「最近」，是「去书架」，
        // 故用分割线隔开——不然会被读成第 6 本最近的书。
        add_sep(sub);
        let all = make_item("全部书架…", TAG_SHELF, false);
        if !all.is_null() {
            let _: () = objc::msg_send![sub, addItem: all];
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
        // 诊断开关：`JIANDU_MENU_DEBUG=1` 打每一步的去向。
        //
        // 状态栏项「装没装上」没有返回值、不抛异常、失败也不崩——不打开这个
        // 开关，只能靠盯着屏幕猜。四个早退分支各打一行，谁掉的立刻看得出来。
        let dbg = std::env::var("JIANDU_MENU_DEBUG").is_ok();
        let menu_cls = *MENU_TARGET_CLASS.get_or_init(register_menu_class);
        let item_cls = *ITEM_TARGET_CLASS.get_or_init(register_item_class);
        let menu_target: *mut Object = unsafe { objc::msg_send![menu_cls, new] };
        let item_target: *mut Object = unsafe { objc::msg_send![item_cls, new] };
        if dbg {
            eprintln!("[menu] targets menu={menu_target:p} item={item_target:p}");
        }
        if menu_target.is_null() || item_target.is_null() {
            return;
        }
        ITEM_TARGET.set(OpaquePtr(item_target)).ok();
        unsafe {
            let status_bar: *mut Object = objc::msg_send![class!(NSStatusBar), systemStatusBar];
            if dbg {
                eprintln!("[menu] status_bar={status_bar:p}");
            }
            if status_bar.is_null() {
                return;
            }
            // NSVariableStatusItemLength = -1.0（CGFloat）。
            let item: *mut Object = objc::msg_send![status_bar, statusItemWithLength: -1.0f64];
            if dbg {
                eprintln!("[menu] item={item:p}");
            }
            if item.is_null() {
                return;
            }
            // **显式 retain 一次**：`NSStatusBar` 本来就会持有放上去的 item，
            // 这一步是防御性的——我们把裸指针存进 `static`，生命周期就不该只
            // 依赖「系统一定会持有到我退出」这个假设。多持一份不会泄漏到有害
            // （进程结束即回收），少持一份可能导致「图标装上又消失、不崩不报错」。
            let _: *mut Object = objc::msg_send![item, retain];
            let button: *mut Object = objc::msg_send![item, button];
            if dbg {
                eprintln!("[menu] button={button:p}");
            }
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
            // 宽度：默认 `-1`（`NSVariableStatusItemLength`，按内容自动量宽）。
            //
            // 实测两种都画得出来（`JIANDU_MENU_W=26` 走定宽、`-1` 走可变，各截图
            // 核对过「笺」一样清晰）。取可变是因为它跟随内容——将来若改成
            // 「图标 + 字数」，定宽就会裁字，而可变宽度不会。
            let want_len: f64 = std::env::var("JIANDU_MENU_W")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(-1.0);
            let _: () = objc::msg_send![item, setLength: want_len];
            if dbg {
                let vis: bool = objc::msg_send![item, isVisible];
                let len: f64 = objc::msg_send![item, length];
                let btitle: *mut Object = objc::msg_send![button, title];
                let clen: usize = if btitle.is_null() {
                    0
                } else {
                    objc::msg_send![btitle, length]
                };
                eprintln!("[menu] visible={vis} length={len:.1} title_len={clen}");
            }
            // 泄漏 item：活到进程结束，比提前释放安全一万倍。
            STATUS_ITEM.set(OpaquePtr(item)).ok();

            // 开发钩子 `JIANDU_MENU_POP=<秒>`：延迟若干秒后**程序化弹出菜单**。
            //
            // 为什么非它不可：菜单栏按钮在屏幕顶端的系统区域，合成点击（CGEvent
            // / AppleScript）会被 TCC 拦掉，`cliclick` 这类工具又不在依赖里。
            // 没有这个钩子，「点开是不是这样」只能靠人肉看——而菜单结构、灰字、
            // 勾选态、子菜单恰恰是最容易改错又最难自查的地方（v1.9.78 那次
            // 「点了没反应」就是因为看不见）。
            //
            // `popUpStatusItemMenu:` 会进模态跟踪循环（进程不退出，只是不再
            // 重绘），所以截图要由**外部进程**做：`screencapture` 不受影响。
            if let Ok(secs) = std::env::var("JIANDU_MENU_POP") {
                let delay: f64 = secs.parse().unwrap_or(3.0);
                let _: () = objc::msg_send![
                    menu_target,
                    performSelector: sel!(showMenu:)
                    withObject: ptr::null_mut::<Object>()
                    afterDelay: delay
                ];
            }

            // 开发钩子 `JIANDU_MENU_FIRE=<tag>`：延迟若干秒后**派发这一项的
            // 动作**（不弹菜单）。tag 见 `TAG_*` 常量：6=继续阅读、7=听书、
            // 8=护眼、9=重启、100+n=最近第 n 本。
            //
            // 与 `JIANDU_MENU_POP` 分开两个开关：验证「菜单长得对不对」和
            // 「点下去对不对」是两件事，混在一起会互相干扰（弹着菜单时
            // GPUI 主循环被模态挡住，改了状态也看不见）。
            if let Ok(v) = std::env::var("JIANDU_MENU_FIRE") {
                if let Ok(tag) = v.parse::<i64>() {
                    let delay: f64 = std::env::var("JIANDU_MENU_DELAY")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(3.0);
                    let num: *mut Object =
                        objc::msg_send![class!(NSNumber), numberWithLongLong: tag];
                    if dbg {
                        eprintln!("[menu] 排程 fireTag tag={tag} delay={delay}");
                    }
                    let _: () = objc::msg_send![
                        menu_target,
                        performSelector: sel!(fireTag:)
                        withObject: num
                        afterDelay: delay
                    ];
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 顶部灰字的三档：0 / 不足一分钟 / 满分钟 / 满小时。
    ///
    /// 这几档看着琐碎，但它是**菜单里唯一一个会说话的数字**——写成
    /// 「今日阅读 0 分钟」像在数落人，把 40 秒四舍五入成 0 又让人白读。
    #[test]
    fn fmt_today_never_insults_or_rounds_down_to_zero() {
        assert_eq!(fmt_today(0), "今日尚未阅读");
        assert_eq!(fmt_today(1), "今日阅读不足 1 分钟");
        assert_eq!(fmt_today(59), "今日阅读不足 1 分钟");
        assert_eq!(fmt_today(60), "今日阅读 1 分钟");
        assert_eq!(fmt_today(59 * 60), "今日阅读 59 分钟");
        assert_eq!(fmt_today(3600), "今日阅读 1 小时");
        assert_eq!(fmt_today(3600 + 60 * 23), "今日阅读 1 小时 23 分钟");
    }

    /// 「继续阅读」的空书名不许拼出「《》」这种两个空括号。
    #[test]
    fn continue_label_handles_missing_title() {
        assert_eq!(continue_label(Some("史記")), "继续阅读：《史記》");
        assert_eq!(continue_label(None), "继续阅读");
        assert_eq!(continue_label(Some("")), "继续阅读");
    }

    /// 听书项用动词，不用勾——勾表示「状态」，而这一项点下去是「做事」。
    #[test]
    fn tts_label_is_a_verb_pair() {
        assert_eq!(tts_label(false), "开始听书");
        assert_eq!(tts_label(true), "停止听书");
    }

    /// 每个意图都得能从 tag 走回来——这是「点了没反应」的第一道闸。
    ///
    /// v1.9.78 的 bug 正是 tag 全是 0：菜单看得见、点了没反应，且**不会崩**，
    /// 只能靠人肉发现。这条测试把「有 tag 必须有回程」钉死。
    #[cfg(target_os = "macos")]
    #[test]
    fn every_action_tag_maps_back_to_an_intent() {
        use super::mac::intent_from_tag_for_test as f;
        assert_eq!(f(1), Some(MenuIntent::OpenShelf));
        assert_eq!(f(2), Some(MenuIntent::Collation));
        assert_eq!(f(3), Some(MenuIntent::CheckUpdate));
        assert_eq!(f(4), Some(MenuIntent::Quit));
        assert_eq!(f(5), Some(MenuIntent::ImportVariants));
        assert_eq!(f(6), Some(MenuIntent::ContinueReading));
        assert_eq!(f(7), Some(MenuIntent::ToggleTts));
        assert_eq!(f(8), Some(MenuIntent::ToggleHuyan));
        assert_eq!(f(9), Some(MenuIntent::RestartUpdate));
        assert_eq!(f(100 + 7), Some(MenuIntent::OpenRecent(7)));
        // 0 是「灰表头 / 带子菜单的父项」专用：必须映射不到任何东西。
        assert_eq!(f(0), None);
    }
}
