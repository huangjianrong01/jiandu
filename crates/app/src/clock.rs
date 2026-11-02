//! 本地日历日（「今天」）的唯一真源。
//!
//! 凡是要说「今日」的地方都从这里取：书架的**今日读叶账**、分享图落款、
//! PDF 时间戳。
//!
//! 为什么不拿 `SystemTime` 自己除 86400：那是 **UTC** 的日子。东八区在
//! 00:00–08:00 之间会被算成昨天——偏偏那一段正是「夜读」的正经时候，
//! 落款写错日子、读叶记到昨天，都挺煞风景。系统时区只有平台知道，所以
//! 这一处落到 `localtime_r`（libc，macOS / Linux 同一套签名）。
//!
//! ⚠️ 时区**只在壳子里问**：`jiandu-core` 的 `library::day_epoch` /
//! `day_key_of` 是纯整数日期算术，不碰时区。

/// 本地日历日 `YYYY-MM-DD`；系统时钟或时区取不到时回落到 **UTC** 那天
/// （宁可差 8 小时，也不空着手）。
pub fn today_key() -> String {
    match today_ymd() {
        Some((y, m, d)) => format!("{y:04}-{m:02}-{d:02}"),
        None => {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            jiandu_core::library::day_key_of((secs / 86_400) as i64)
        }
    }
}

/// 本地（年, 月, 日）。系统时钟不可信时返回 `None`。
pub fn today_ymd() -> Option<(i64, u32, u32)> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // `localtime_r` 是线程安全的（`localtime` 不是，返回的是同一块静态区）。
    let ok = unsafe { libc::localtime_r(&secs, &mut tm) };
    if ok.is_null() {
        return None;
    }
    // `tm_year` 从 1900 起算、`tm_mon` 从 0 起算，各加回去。
    let y = i64::from(tm.tm_year) + 1900;
    let m = u32::try_from(tm.tm_mon + 1).ok()?;
    let d = u32::try_from(tm.tm_mday).ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 日期键必须是**这个形状**，且与 core 的日期算术对得上
    /// （对不上 = 读叶账分桶会错日）。
    #[test]
    fn today_key_is_a_well_formed_local_day() {
        let k = today_key();
        assert_eq!(k.len(), 10, "{k}");
        assert_eq!(&k[4..5], "-");
        assert_eq!(&k[7..8], "-");
        assert!(jiandu_core::library::day_epoch(&k).is_some(), "{k} 应能解析");
        let (y, m, d) = today_ymd().expect("系统时钟应可信");
        assert!((2020..2200).contains(&y), "年份异常：{y}");
        assert_eq!(format!("{y:04}-{m:02}-{d:02}"), k, "两条路必须同一天");
    }
}
