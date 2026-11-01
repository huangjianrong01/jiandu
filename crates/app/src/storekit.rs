//! **App Store 购买凭据**（v1.9.23 · 内购链路真正接线）。
//!
//! # 为什么单开一个模块
//!
//! 离线 key（`jiandu_core::license`）与 App Store 内购是**两条不相干的
//! 凭据**：前者是「我卖给你一串字符」，后者是「Apple 替我收钱并给我一张
//! 收据」。App Review 2.4.5(vi) 禁止前者出现在商店版里，所以商店版只能
//! 认后者。把它们分在两个模块、`licensed()` 里取「或」，将来官网版与商店版
//! 的差异就只有 `licensed()` 那一个分支，不必动其余代码。
//!
//! # 三层结构（这是本模块唯一需要看懂的地方）
//!
//! ```text
//!   ┌─ 公开 API ────────────────────────────────┐
//!   │ prepare / purchased / phase / price /     │  ← UI 只认这一层
//!   │ purchase / restore / last_error           │
//!   ├─ 纯状态机 ────────────────────────────────┤
//!   │ Phase 迁移 + 原子量 + 一行诊断文案        │  ← 单测在这里跑，不碰 Apple
//!   ├─ 后端（macOS + appstore）─────────────────┤
//!   │ StoreKit 1：SKPaymentQueue + 观察者 +     │  ← 只有它认识 objc
//!   │ SKProductsRequest 取本地化价格            │
//!   └──────────────────────────────────────────┘
//! ```
//!
//! 把状态机与 StoreKit 切开，是为了**买得起的东西能测**：从「点购买」到
//! 「已解锁」中间要经过 6 个状态（含失败、待恢复），真机走一遍得有一个沙盒
//! 账号；而状态机本身是纯的，`cargo test` 就能把每条路走干净。
//!
//! # 为什么是 StoreKit **1**（`SKPaymentQueue`）而不是 StoreKit 2
//!
//! StoreKit 2 的 API 是 Swift 的 `async/await`（`Product.purchase()`、
//! `Transaction.updates`），**没有 Objective-C 入口**。本项目是 Rust + `objc`
//! crate 直调，没有 Swift 编译单元（也没有 Xcode 工程），引入 StoreKit 2 等于
//! 先引入一条 Swift 构建链。StoreKit 1 全是 ObjC 类（`SKPaymentQueue` /
//! `SKMutablePayment` / `SKProductsRequest`），用现有手法就能打通。
//!
//! 代价：StoreKit 1 自 macOS 15 起标记为 deprecated（仍可用、仍能过审）。
//! 将来若要迁到 StoreKit 2，**只需替换本文件的 `backend`**，上面两层一行不动。
//!
//! # 凭据落在哪
//!
//! 买断型（non-consumable）本地缓存一个布尔即可，不必每次联网：落在
//! `NSUserDefaults`（键 [`DEFAULTS_KEY`]）。这是沙盒安全的（自动写进
//! `~/Library/Containers/<bundle-id>/Data/Library/Preferences/`），也是平台
//! 认可的「权益缓存」位置。
//!
//! ⚠️ **这不是防破解**：`defaults write com.jiandu.app` 能把它改成 true。
//! 买断价的产品不值得为它做反破解（真做了也会被绕过），但要**知道**这条：
//! 将来若要收紧，正路是启动时读 `appStoreReceiptURL` 那张收据、本地校验其中
//! 的 IAP 记录（PKCS#7，需引 `openssl` 或手写 ASN.1）。列入上架后的硬化项，
//! 见 `docs/appstore-iap.md`。
//!
//! # 两个非显然的 API 事实
//!
//! * `SKPaymentQueue` 的观察者必须在**启动早期**挂上（`prepare`）。挂上之后
//!   StoreKit 会重投所有未 finish 的交易——包括上次买到一半被杀掉的那笔。
//!   做完必须 `finishTransaction:`，否则它会**每次启动都重投**。
//! * `SKRequest.delegate` 是 `assign`（裸指针，不是 weak），而
//!   `SKProductsRequest` 一旦没人持有就会 dealloc。所以观察者实例与请求实例
//!   都**故意泄漏**（各存一个 `AtomicUsize`）——它们活到进程结束，泄漏量是
//!   两个对象，比替它们做一套引用计数便宜得多。

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Mutex;

/// 买断型内购的 product id。
///
/// 必须与 App Store Connect 后台配置**逐字一致**——写错一个字符，已付费用户
/// 会被判成未购买，那是最难缠的一类工单。
pub const UNLOCK_PRODUCT_ID: &str = "cn.jiandu.unlock.pro";

/// 本地权益缓存键（`NSUserDefaults`）。
pub const DEFAULTS_KEY: &str = "cn.jiandu.purchased";

/// 购买链路的相位。UI 只根据它决定按钮与文案，不自己猜。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// 还没问过 StoreKit（启动极早期）。
    Idle,
    /// 正在向 StoreKit 要商品（含价格）。
    Loading,
    /// 商品已就绪，可以买。
    Ready,
    /// 查不到商品：后台没配 / bundle id 不符 / 未签名 / 无账号。
    NoProduct,
    /// 已发起支付，等 StoreKit 回调。
    Purchasing,
    /// 正在恢复购买（换机器重装后）。
    Restoring,
    /// 已买断。
    Owned,
    /// 上一次失败了（文案在 `last_error`）。
    Failed,
}

impl Phase {
    fn code(self) -> u8 {
        match self {
            Phase::Idle => 0,
            Phase::Loading => 1,
            Phase::Ready => 2,
            Phase::NoProduct => 3,
            Phase::Purchasing => 4,
            Phase::Restoring => 5,
            Phase::Owned => 6,
            Phase::Failed => 7,
        }
    }

    fn from_code(c: u8) -> Phase {
        match c {
            1 => Phase::Loading,
            2 => Phase::Ready,
            3 => Phase::NoProduct,
            4 => Phase::Purchasing,
            5 => Phase::Restoring,
            6 => Phase::Owned,
            7 => Phase::Failed,
            _ => Phase::Idle,
        }
    }
}

/// 状态机里那些**非布尔**的东西：价格与最后一条错误。只在解锁那张纸打开时读。
#[derive(Default, Clone)]
struct Note {
    price: Option<String>,
    err: Option<String>,
}

static PHASE: AtomicU8 = AtomicU8::new(0);
static OWNED: AtomicBool = AtomicBool::new(false);
static NOTE: Mutex<Note> = Mutex::new(Note {
    price: None,
    err: None,
});

fn set_phase(p: Phase) {
    PHASE.store(p.code(), Ordering::Relaxed);
}

/// 标记「已买断」。
fn mark_owned() {
    OWNED.store(true, Ordering::SeqCst);
    set_phase(Phase::Owned);
}

fn set_price(p: String) {
    if let Ok(mut n) = NOTE.lock() {
        n.price = Some(p);
    }
}

fn set_err(e: impl Into<String>) {
    if let Ok(mut n) = NOTE.lock() {
        n.err = Some(e.into());
    }
}

/// 是不是已经买断。**每帧都会被问**（`licensed()`），所以走原子量、不碰锁定。
pub fn purchased() -> bool {
    // 开发口子：只认 debug 构建。release 里这个分支恒 false，
    // 所以在商店包里塞不进一条「白拿」的路。
    if cfg!(debug_assertions)
        && std::env::var("JIANDU_STORE_UNLOCK")
            .map(|v| v == "1")
            .unwrap_or(false)
    {
        return true;
    }
    OWNED.load(Ordering::Relaxed)
}

/// 当前相位。
pub fn phase() -> Phase {
    Phase::from_code(PHASE.load(Ordering::Relaxed))
}

/// 相位的原始码（`u8`）。
///
/// 给 UI 做「变了没有」的比较用：`Phase` 每次都要从原子量解一次，而
/// 每帧比较两次枚举值不如比较一个 `u8` 直接。**顺序与 `Phase::code` 同源**，
/// 不要另立一套编号。
pub fn phase_id() -> u8 {
    PHASE.load(Ordering::Relaxed)
}

/// StoreKit 给的本地化价格（`¥198.00` 这种）。拿不到为 `None`。
pub fn price() -> Option<String> {
    NOTE.lock().ok().and_then(|n| n.price.clone())
}

/// 最后一条错误文案（已经被人话化，可直接上屏）。
pub fn last_error() -> Option<String> {
    NOTE.lock().ok().and_then(|n| n.err.clone())
}

/// 清掉上一条错误（重试前调）。
pub fn clear_error() {
    if let Ok(mut n) = NOTE.lock() {
        n.err = None;
    }
}

/// **启动时调一次**：载入本地权益 + 挂交易观察者 + 拉商品信息。
///
/// 必须在主线程、在建窗之前调（观察者要尽早挂，才能接住上次未完成的交易）。
pub fn prepare() {
    backend::prepare();
}

/// 发起购买。返回 `false` = 现在买不了（商品没就绪 / 已有一次在跑）。
///
/// 结果**不在这里返回**：StoreKit 走回调（可能弹系统登录框、可能要家长批准），
/// 由 `phase()` 的变化体现。UI 侧见 `unlock::poll`。
pub fn purchase() -> bool {
    if matches!(phase(), Phase::Purchasing | Phase::Restoring | Phase::Owned) {
        return false;
    }
    backend::purchase()
}

/// 恢复购买（换机器 / 重装后）。买断型**必须有**这个入口，
/// 否则用户换台 Mac 就以为钱白付了——这是差评高发区。
pub fn restore() -> bool {
    if matches!(phase(), Phase::Purchasing | Phase::Restoring) {
        return false;
    }
    backend::restore()
}

// ======================================================================
// 后端 ①：macOS + appstore feature —— 真 StoreKit
// ======================================================================
#[cfg(all(target_os = "macos", feature = "appstore"))]
mod backend {
    use super::*;
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Protocol, Sel, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;
    use std::sync::Once;

    #[link(name = "StoreKit", kind = "framework")]
    extern "C" {}

    extern "C" {
        fn dlopen(filename: *const c_char, flag: i32) -> *mut std::os::raw::c_void;
    }
    /// `RTLD_LAZY`（`<dlfcn.h>`）。
    const RTLD_LAZY: i32 = 0x1;

    const OBS_CLASS: &str = "JianduStoreKitObserver";

    /// 观察者单例（故意泄漏，见文件头）。
    static OBSERVER: AtomicUsize = AtomicUsize::new(0);
    /// 进行中的 `SKProductsRequest`（delegate 是 assign，必须自己攥住）。
    static REQUEST: AtomicUsize = AtomicUsize::new(0);
    /// 拿到的 `SKProduct`（`SKMutablePayment` 要它）。
    static PRODUCT: AtomicUsize = AtomicUsize::new(0);
    static ONCE: Once = Once::new();

    fn dbg_on() -> bool {
        std::env::var("JIANDU_STORE_DEBUG")
            .map(|v| v == "1")
            .unwrap_or(false)
    }

    fn log(msg: &str) {
        if dbg_on() {
            eprintln!("[storekit] {msg}");
        }
    }

    // ---- ObjC 小工具（每个都自带 autoreleasepool）----

    fn ns_string(s: &str) -> *mut Object {
        let Ok(c) = CString::new(s) else {
            return std::ptr::null_mut();
        };
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    fn ns_to_string(p: *mut Object) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe {
            let utf8: *const c_char = msg_send![p, UTF8String];
            if utf8.is_null() {
                return String::new();
            }
            CStr::from_ptr(utf8).to_string_lossy().into_owned()
        }
    }

    fn load_flag() -> bool {
        objc::rc::autoreleasepool(|| unsafe {
            let d: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
            let k = ns_string(DEFAULTS_KEY);
            let yes: BOOL = msg_send![d, boolForKey: k];
            yes == YES
        })
    }

    fn store_flag(on: bool) {
        objc::rc::autoreleasepool(|| unsafe {
            let d: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
            let k = ns_string(DEFAULTS_KEY);
            let _: () = msg_send![d, setBool: (on as BOOL) forKey: k];
            // 立刻落盘：用户买完可能直接 ⌘Q，不能指望系统的延迟写。
            let _: BOOL = msg_send![d, synchronize];
        });
        log(&format!("persist purchased={on}"));
    }

    fn queue() -> *mut Object {
        unsafe { msg_send![class!(SKPaymentQueue), defaultQueue] }
    }

    /// **把 StoreKit 显式拉进进程**。
    ///
    /// 这一步曾漏掉，代价是真机日志里两行
    /// `!! 找不到 SKPaymentTransactionObserver 协议`：协议表是**运行时**注册
    /// 的东西，`objc_getProtocol` 只看「已加载的 image」，而现代 dyld 的惰性
    /// 绑定让我们第一次查协议时 StoreKit 的 image 还没进场（`-framework`
    /// 只保证了「迟早会加载」，不保证「此刻已加载」）。协议缺失的后果是
    /// `add_protocol` 静默跳过——交易回调能不能来全看 StoreKit 认不认
    /// `respondsToSelector`，那是**赌**，不是工程。
    ///
    /// 显式 `dlopen` 一次，把「迟早」变成「现在」。找不到框架（极老系统）也
    /// 不崩，只是后面的相位会落 `NoProduct`。
    fn ensure_framework_loaded() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let path = b"/System/Library/Frameworks/StoreKit.framework/StoreKit\0";
            let h = dlopen(path.as_ptr() as *const c_char, RTLD_LAZY);
            log(&format!("dlopen StoreKit -> {:p}", h));
        });
    }

    /// 这个包有没有 App Store 收据？本地开发包 / 官网版都没有。
    ///
    /// 用来把 StoreKit 那句干巴巴的 `An unknown error occurred` 翻译成人话：
    /// 没从 App Store 装下来，本来就取不到商品——那不是故障，是常态。
    fn has_receipt() -> bool {
        objc::rc::autoreleasepool(|| unsafe {
            let b: *mut Object = msg_send![class!(NSBundle), mainBundle];
            if b.is_null() {
                return false;
            }
            let url: *mut Object = msg_send![b, appStoreReceiptURL];
            !url.is_null()
        })
    }

    // ---- 观察者：交易 + 恢复 + 商品查询，三件事共用一个类 ----

    extern "C" fn updated_transactions(
        _this: &Object,
        _cmd: Sel,
        queue: *mut Object,
        transactions: *mut Object,
    ) {
        unsafe {
            let n: usize = msg_send![transactions, count];
            for i in 0..n {
                let t: *mut Object = msg_send![transactions, objectAtIndex: i];
                let state: i64 = msg_send![t, transactionState];
                // SKPaymentTransactionState: 0 Purchasing / 1 Purchased /
                // 2 Failed / 3 Restored / 4 Deferred
                log(&format!("transaction state={state}"));
                match state {
                    1 | 3 => {
                        // 买到 / 恢复到：落盘 + 解锁 + finish（不 finish 会反复重投）。
                        store_flag(true);
                        mark_owned();
                        let _: () = msg_send![queue, finishTransaction: t];
                    }
                    2 => {
                        let err: *mut Object = msg_send![t, error];
                        let desc = if err.is_null() {
                            String::new()
                        } else {
                            let d: *mut Object = msg_send![err, localizedDescription];
                            ns_to_string(d)
                        };
                        let code: i64 = if err.is_null() {
                            0
                        } else {
                            msg_send![err, code]
                        };
                        // 用户自己取消（SKErrorPaymentCancelled = 2）不算错误：
                        // 报「购买失败」会让人以为钱被扣了。
                        if code == 2 {
                            set_phase(Phase::Ready);
                            log("cancelled by user");
                        } else {
                            set_err(format!(
                                "{}（错误码 {code}）",
                                if desc.is_empty() {
                                    "购买未完成"
                                } else {
                                    &desc
                                }
                            ));
                            set_phase(Phase::Failed);
                        }
                        let _: () = msg_send![queue, finishTransaction: t];
                    }
                    4 => {
                        // 待批准（家长 / 「先买后付」）：保持「进行中」，别当失败。
                        set_err("这笔购买正在等待批准，批准后会自动解锁");
                        set_phase(Phase::Purchasing);
                    }
                    _ => {}
                }
            }
        }
    }

    extern "C" fn restore_finished(_this: &Object, _cmd: Sel, _queue: *mut Object) {
        // 恢复跑完但一笔都没恢复（= 这个账号确实没买过）：必须给话，
        // 否则按钮会永远停在「正在恢复」。
        if phase() != Phase::Owned {
            set_err("这个 Apple 账号没有查到此应用的购买记录");
            set_phase(Phase::Ready);
        }
        log("restore finished");
    }

    extern "C" fn restore_failed(
        _this: &Object,
        _cmd: Sel,
        _queue: *mut Object,
        err: *mut Object,
    ) {
        let desc = if err.is_null() {
            String::new()
        } else {
            unsafe {
                let d: *mut Object = msg_send![err, localizedDescription];
                ns_to_string(d)
            }
        };
        set_err(format!(
            "恢复购买失败：{}",
            if desc.is_empty() { "未知原因" } else { &desc }
        ));
        set_phase(Phase::Failed);
    }

    extern "C" fn products_response(
        _this: &Object,
        _cmd: Sel,
        _request: *mut Object,
        response: *mut Object,
    ) {
        unsafe {
            let arr: *mut Object = msg_send![response, products];
            let n: usize = if arr.is_null() {
                0
            } else {
                msg_send![arr, count]
            };
            log(&format!("products count={n}"));
            if n == 0 {
                // 这是**最常见的配置故障**：后台没建商品 / product id 打错 /
                // bundle id 不是 com.jiandu.app / 包没签名。把排查方向直接写上屏。
                set_err(format!(
                    "查不到商品 {UNLOCK_PRODUCT_ID}——请检查 App Store Connect \
                     里是否已建该内购、Bundle ID 是否与安装包一致"
                ));
                set_phase(Phase::NoProduct);
                return;
            }
            let p: *mut Object = msg_send![arr, objectAtIndex: 0];
            // 攥住 product：SKMutablePayment 要用它，而 autorelease 池随时会收走。
            let _: *mut Object = msg_send![p, retain];
            PRODUCT.store(p as usize, Ordering::SeqCst);

            // 价格必须说真话（Guideline 3.1.1：展示真实价）。
            // `localizedPrice` 是较新的便捷属性；老系统上没有就退回 `price`
            // 的原始十进制串（不含货币符号，但不撒谎比好看重要）。
            let has: BOOL = msg_send![p, respondsToSelector: sel!(localizedPrice)];
            let price = if has == YES {
                ns_to_string(msg_send![p, localizedPrice])
            } else {
                let d: *mut Object = msg_send![p, price];
                let s: *mut Object = msg_send![d, description];
                ns_to_string(s)
            };
            if !price.is_empty() {
                set_price(price);
            }
            clear_error();
            set_phase(Phase::Ready);
        }
    }

    extern "C" fn request_failed(
        _this: &Object,
        _cmd: Sel,
        _request: *mut Object,
        err: *mut Object,
    ) {
        let desc = if err.is_null() {
            String::new()
        } else {
            unsafe {
                let d: *mut Object = msg_send![err, localizedDescription];
                ns_to_string(d)
            }
        };
        set_err(format!(
            "取商品信息失败：{}{}",
            if desc.is_empty() {
                "网络或账号问题"
            } else {
                &desc
            },
            if has_receipt() {
                ""
            } else {
                "——这个包没有 App Store 收据（本地开发包 / TestFlight 之外装的都如此），取不到商品属正常；要看真实价格请用 TestFlight 或商店版"
            }
        ));
        set_phase(Phase::NoProduct);
    }

    fn observer_class() -> Option<&'static Class> {
        unsafe {
            if let Some(c) = Class::get(OBS_CLASS) {
                return Some(c);
            }
            let mut decl = ClassDecl::new(OBS_CLASS, class!(NSObject))?;
            // 少挂一个协议，StoreKit 就不会回调对应的方法——静默不生效，
            // 所以这里两个都必须挂上（前者管交易与恢复，后者管商品查询）。
            if let Some(p) = Protocol::get("SKPaymentTransactionObserver") {
                decl.add_protocol(p);
            } else {
                log("!! 找不到 SKPaymentTransactionObserver 协议");
            }
            if let Some(p) = Protocol::get("SKProductsRequestDelegate") {
                decl.add_protocol(p);
            } else {
                log("!! 找不到 SKProductsRequestDelegate 协议");
            }
            decl.add_method(
                sel!(paymentQueue:updatedTransactions:),
                updated_transactions as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(paymentQueueRestoreCompletedTransactionsFinished:),
                restore_finished as extern "C" fn(&Object, Sel, *mut Object),
            );
            decl.add_method(
                sel!(paymentQueue:restoreCompletedTransactionsFailedWithError:),
                restore_failed as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(productsRequest:didReceiveResponse:),
                products_response as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            decl.add_method(
                sel!(request:didFailWithError:),
                request_failed as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            Some(decl.register())
        }
    }

    fn observer() -> *mut Object {
        let p = OBSERVER.load(Ordering::SeqCst);
        if p != 0 {
            return p as *mut Object;
        }
        let Some(cls) = observer_class() else {
            return std::ptr::null_mut();
        };
        unsafe {
            let obj: *mut Object = msg_send![cls, new];
            if obj.is_null() {
                return std::ptr::null_mut();
            }
            // 故意不释放：见文件头。`new` 已 +1，存起来就够。
            OBSERVER.store(obj as usize, Ordering::SeqCst);
            obj
        }
    }

    pub fn prepare() {
        ONCE.call_once(|| {
            objc::rc::autoreleasepool(|| {
                // ⓪ 先把框架拉进进程，否则协议表里查不到观察者协议（见
                //    `ensure_framework_loaded` 的说明）。
                ensure_framework_loaded();
                // 顺手把「类在不在」记一笔：真机排障时这一行能直接分清
                // 「系统没有 StoreKit 1」与「我们没链上」。
                log(&format!(
                    "SKPaymentQueue class = {:?}, hasReceipt = {}",
                    Class::get("SKPaymentQueue").is_some(),
                    has_receipt()
                ));

                // ① 本地权益先载入——它要在第一帧之前就位，否则已付费用户
                //    开机会看到一次「未购买」的闪动。
                if load_flag() {
                    log("cached flag = purchased");
                    mark_owned();
                } else {
                    set_phase(Phase::Loading);
                }

                // ② 挂观察者（会重投上次未 finish 的交易）。
                let obs = observer();
                if obs.is_null() {
                    set_err("系统 StoreKit 不可用（观察者注册失败）");
                    set_phase(Phase::Failed);
                    return;
                }
                unsafe {
                    let _: () = msg_send![queue(), addTransactionObserver: obs];
                }

                // ③ 查商品（拿价格 + 顺便诊断后台配置）。
                unsafe {
                    let ids: *mut Object =
                        msg_send![class!(NSSet), setWithObject: ns_string(UNLOCK_PRODUCT_ID)];
                    let req: *mut Object = msg_send![class!(SKProductsRequest), alloc];
                    if req.is_null() {
                        set_err("系统 StoreKit 不可用（SKProductsRequest 不可用）");
                        set_phase(Phase::Failed);
                        return;
                    }
                    let req: *mut Object = msg_send![req, initWithProductIdentifiers: ids];
                    let _: () = msg_send![req, setDelegate: obs];
                    // 攥住 request：delegate 是 assign，request 自己被池收走
                    // 就永远等不到回调。
                    REQUEST.store(req as usize, Ordering::SeqCst);
                    let _: () = msg_send![req, start];
                }
                log("prepared");
            });
        });
    }

    pub fn purchase() -> bool {
        let p = PRODUCT.load(Ordering::SeqCst);
        if p == 0 {
            // 商品还没就绪：不能空手 addPayment（会立刻失败且错误码难懂）。
            set_err("商品信息还没取到，请稍候再试（或点「恢复购买」）");
            set_phase(Phase::Failed);
            return false;
        }
        objc::rc::autoreleasepool(|| unsafe {
            let payment: *mut Object =
                msg_send![class!(SKMutablePayment), paymentWithProduct: p as *mut Object];
            if payment.is_null() {
                set_err("无法创建支付请求（StoreKit 未就绪）");
                set_phase(Phase::Failed);
                return;
            }
            clear_error();
            set_phase(Phase::Purchasing);
            let _: () = msg_send![queue(), addPayment: payment];
            log("payment added");
        });
        true
    }

    pub fn restore() -> bool {
        objc::rc::autoreleasepool(|| unsafe {
            clear_error();
            set_phase(Phase::Restoring);
            let _: () = msg_send![queue(), restoreCompletedTransactions];
            log("restore started");
        });
        true
    }
}

// ======================================================================
// 后端 ②：其余一切（官网版 / 非 macOS / 测试）
// ======================================================================
#[cfg(not(all(target_os = "macos", feature = "appstore")))]
mod backend {
    use super::*;

    /// 官网分发版没有 App Store 这回事：把相位钉在 `NoProduct`，
    /// 让任何误接进来的 UI 都显示「此版本不走内购」而不是转圈。
    pub fn prepare() {
        set_err("此版本不经 App Store 购买");
        set_phase(Phase::NoProduct);
    }

    pub fn purchase() -> bool {
        false
    }

    pub fn restore() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 相位编码必须可逆——UI 与后端隔着一条原子量传它，
    /// 表错一位就会显示成另一个状态（曾把「恢复中」显示成「失败」）。
    #[test]
    fn phase_codes_round_trip() {
        for p in [
            Phase::Idle,
            Phase::Loading,
            Phase::Ready,
            Phase::NoProduct,
            Phase::Purchasing,
            Phase::Restoring,
            Phase::Owned,
            Phase::Failed,
        ] {
            assert_eq!(Phase::from_code(p.code()), p, "{p:?} 编码不可逆");
        }
        // 未知码一律退 Idle，不 panic（前向兼容：将来加相位不会炸老代码）。
        assert_eq!(Phase::from_code(200), Phase::Idle);
    }

    /// 走一遍从「查商品」到「已解锁」的整条路，含取消与恢复两个岔口。
    ///
    /// 串在一个测试里跑是**故意的**：状态机是进程级单例（原子量 + Mutex），
    /// 拆成多个 `#[test]` 会并行互踩，红一次要查半天。
    #[test]
    fn state_machine_walks_the_whole_purchase() {
        // ① 启动：本地没有权益 → Loading。
        set_phase(Phase::Loading);
        assert!(!purchased());
        assert_eq!(phase(), Phase::Loading);

        // ② 商品查到 → Ready + 有价格。
        set_price("¥198.00".to_string());
        clear_error();
        set_phase(Phase::Ready);
        assert_eq!(phase(), Phase::Ready);
        assert_eq!(price().as_deref(), Some("¥198.00"));

        // ③ 点购买 → Purchasing。
        set_phase(Phase::Purchasing);
        assert_eq!(phase(), Phase::Purchasing);

        // ④ 用户取消（StoreKit 报错误码 2）→ 回到 Ready，**不写错误文案**：
        //    自己点取消却看到「购买失败」，人会以为钱被扣了。
        clear_error();
        set_phase(Phase::Ready);
        assert_eq!(phase(), Phase::Ready);
        assert!(last_error().is_none());

        // ⑤ 再买一次，这次成了 → Owned。
        mark_owned();
        assert_eq!(phase(), Phase::Owned);
        assert!(purchased(), "买断后 purchased() 必须为真");

        // ⑥ 已买断后再走「恢复」也不该退回未购买。
        set_phase(Phase::Restoring);
        mark_owned();
        assert!(purchased());
    }

    /// 恢复购买一笔都没恢复：必须落回 Ready 并给话，不能永远转圈。
    #[test]
    fn failed_restore_still_lands_somewhere_usable() {
        set_err("这个 Apple 账号没有查到此应用的购买记录");
        set_phase(Phase::Ready);
        assert_eq!(phase(), Phase::Ready);
        assert!(last_error().is_some());
        clear_error();
        assert!(last_error().is_none());
    }

    #[test]
    fn product_id_is_stable() {
        // 改了它，老用户的恢复购买会认不出自己买过什么。
        assert_eq!(UNLOCK_PRODUCT_ID, "cn.jiandu.unlock.pro");
        assert!(UNLOCK_PRODUCT_ID.starts_with("cn.jiandu."));
    }
}
