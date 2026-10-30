//! **App Store 购买凭据**（v1.9.9 · 上架准备）。
//!
//! # 为什么单开一个模块
//!
//! 离线 key（`jiandu_core::license`）与 App Store 内购是**两条不相干的
//! 凭据**：前者是「我卖给你一串字符」，后者是「Apple 替我收钱并给我一张
//! 收据」。App Review 2.4.5(vi) 禁止前者出现在商店版里，所以商店版只能
//! 认后者。把它们分在两个模块、`licensed()` 里取「或」，将来官网版与商店版
//! 的差异就只有 `licensed()` 那一个分支，不必动其余代码。
//!
//! # ⚠️ 当前是占位实现
//!
//! `purchased()` 现在恒返回 `true`——这是**故意的**：先让商店版能完整跑通
//! 导出链路（沙盒验证、TestFlight 内测都要用），等真接上 StoreKit 再换成
//! 读收据。
//!
//! **上架前必须把 TODO 消掉**，否则等于全功能免费送。接法见 [`purchased`]。
//!
//! # 接真收据时的形状
//!
//! StoreKit 是 Objective-C / Swift API，Rust 侧要么走 `objc` crate 直接发
//! 消息，要么编一个 `.m` 桥接文件（推荐，AppKit 那套已经在用了）。要拿的
//! 是 App Store 收据里那一条「买断型内购」的 `product_id`：
//!   * `SKPaymentQueue.default()` → `restoreCompletedTransactions` 恢复；
//!   * `Bundle.main.appStoreReceiptURL` 读收据 → 本地校验或交给
//!     `/verifyReceipt`（后者要网络，离线时得退回本地缓存的购买标记）。
//! 买断型（non-consumable）可以本地缓存一个布尔，不必每次联网。

/// 买断型内购的 product id（上架前在 App Store Connect 建好后填进来）。
///
/// 占位值，接真 StoreKit 时必须与后台配置**逐字一致**——写错一个字符，
/// 已付费用户会被判成未购买，那是最难缠的一类工单。
pub const UNLOCK_PRODUCT_ID: &str = "cn.jiandu.unlock.pro";

/// **用户是否已买断**。
///
/// ⚠️ TODO(上架)：当前恒 `true`，接真 StoreKit 后改成读收据 / 缓存标记。
///
/// 见模块文档里「接真收据时的形状」。返回值**必须缓存**（收据解析不便宜），
/// 别放在每帧都会跑的路径上——`licensed()` 是每帧问的，`Root` 里落一个
/// `purchased: bool` 字段、启动时与恢复购买后各刷一次即可。
pub fn purchased() -> bool {
    true
}

/// **发起购买**（买断解锁）。
///
/// ⚠️ TODO(上架)：接 `SKPaymentQueue` 加一个 `SKPayment`。
///
/// 现在什么都不做并返回 `false`，调用方据此提示「购买不可用」。
pub fn purchase() -> bool {
    false
}

/// **恢复购买**（换机器 / 重装后）。
///
/// ⚠️ TODO(上架)：接 `restoreCompletedTransactions`。
///
/// 买断型内购**必须**提供恢复入口，否则用户换台 Mac 就以为钱白付了——
/// 这是 3.1.1 之外的合规常识，也是差评高发区。
pub fn restore() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 占位期：恒 true（保证商店版导出链路可测）。
    ///
    /// 接真收据后这条要改成「未购买时返回 false」——到时候它会先红，
    /// 提醒你把 TODO 清掉。
    #[test]
    fn placeholder_always_unlocked() {
        assert!(purchased(), "占位期应恒为已购买");
    }

    #[test]
    fn product_id_is_stable() {
        // 改了它，老用户的恢复购买会认不出自己买过什么。
        assert!(UNLOCK_PRODUCT_ID.starts_with("cn.jiandu."));
        assert!(!UNLOCK_PRODUCT_ID.is_empty());
    }
}
