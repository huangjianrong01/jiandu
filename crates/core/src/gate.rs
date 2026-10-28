//! **内测邀请码**（v1.0.0-beta）：极简闸门，防的是「随手转发」而不是破解。
//!
//! 设计口径：
//!   * **没有服务器**：码是编进二进制的常量，本机只记「这台机器已经验过」，
//!     不联网、不收集任何信息（与「笺读不联网」的定位一致）。
//!   * **一次录入、永久放行**：验过就写一枚 `beta_code.txt` 进应用数据目录，
//!     之后启动直接进书架，不再打扰。
//!   * **比对前归一**：去空格、转大写、全角转半角——测试同学手抄码时多打
//!     一个空格或小写，不该判错。
//!   * 想换码：只改下面 `CODES` 一处，重打包即可；老码立即失效（但已验过的
//!     机器仍放行——毕竟人家已经在用了，把老用户关在门外没有意义）。

use crate::library::Library;
use std::path::PathBuf;

/// 内测码（3 组固定码）。改这里就够了。
pub const CODES: [&str; 3] = ["SHUYE2026", "MOJUAN88", "QINGJIAN7"];

/// **内测期天数**（v1.82.0）：自首次放行起 7 天内视同已激活，逾期须用授权 key。
///
/// 为什么要限期：固定邀请码一旦被转发到群里，就是一个永久免费的入口；
/// 但把码换成一次性的又会把「重装系统」变成客服工单。7 天是个折中——
/// 够内测同学把功能摸一遍，也够我们收回这批包。到期**不锁阅读**，
/// 只锁导出与印制（见 `Root::export_begin`）：读不了书才是最坏的结果。
pub const BETA_DAYS: i64 = 7;

/// 归一化：去首尾空白、去内部空格与连接符、全角转半角、统一大写。
pub fn normalize(raw: &str) -> String {
    raw.chars()
        .filter_map(|c| {
            // 全角英数（Ａ-Ｚ ０-９）与全角连字符各回半角。
            let c = match c {
                '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
                '\u{3000}' => ' ',
                _ => c,
            };
            if c.is_whitespace() || c == '-' || c == '_' {
                return None;
            }
            Some(c.to_ascii_uppercase())
        })
        .collect()
}

/// 这串码对不对。
pub fn valid(raw: &str) -> bool {
    let n = normalize(raw);
    !n.is_empty() && CODES.iter().any(|c| normalize(c) == n)
}

/// 已经验过的凭据落盘位置（应用数据目录；沙盒下自动落进容器）。
pub fn ticket_path() -> PathBuf {
    Library::home().join("beta_code.txt")
}

/// 判定某个凭据文件是否放行（**纯 I/O**：单测直接喂临时路径，
/// 不去动 `JIANDU_HOME` 那个全局口子——并行测试下会互相踩）。
pub fn redeemed_at(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path).map(|s| valid(&s)).unwrap_or(false)
}

/// 写凭据（同上，纯 I/O）。
pub fn remember_at(path: &std::path::Path, raw: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, normalize(raw));
}

/// 这台机器是否已验过（凭据内容仍须是有效码）。
pub fn redeemed() -> bool {
    redeemed_at(&ticket_path())
}

/// 内测起算日的落盘位置（与凭据**分两只文件**：一个管「验过没有」，
/// 一个管「从哪天起算」，删一个不会把另一个弄坏）。
pub fn since_path() -> PathBuf {
    Library::home().join("beta_since.txt")
}

/// 判定内测还剩几天（**纯 I/O**，单测直接喂临时路径，不碰 `JIANDU_HOME`）。
///
/// 返回：
///   * `None` —— 压根没有有效内测凭据（或文件读不了）；
///   * `Some(n)`，`n > 0` —— 还在期内，`n` 天；
///   * `Some(0)` —— 内测期已满，须用授权 key。
///
/// 起算日文件缺失时**当场补写**：老用户（v1.81 及更早发的包）手里的凭据
/// 没有起算日，从今天算起——不能让人家刚升级就凭空到期。
pub fn beta_days_left_at(ticket: &std::path::Path, since: &std::path::Path, now: i64) -> Option<i64> {
    if !redeemed_at(ticket) {
        return None;
    }
    let today = today_of(now);
    let start = match std::fs::read_to_string(since) {
        Ok(s) => s.trim().parse::<i64>().ok().filter(|d| *d > 0 && *d <= today),
        Err(_) => None,
    };
    let start = match start {
        Some(d) => d,
        None => {
            if let Some(dir) = since.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(since, today.to_string());
            today
        }
    };
    Some((BETA_DAYS - (today - start)).max(0))
}

/// 本机内测剩余天数（见 [`beta_days_left_at`]）。
pub fn beta_days_left() -> Option<i64> {
    beta_days_left_at(&ticket_path(), &since_path(), now_secs())
}

/// 当前 Unix 秒；系统时钟不可信时返回 0（起算日会因此写成 0，
/// 下次读到 0 会被 `filter` 挡掉并按「今天」重写——不会永久锁死）。
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Unix 秒 → epoch 天（水印只要日期，不要时分秒）。
fn today_of(now: i64) -> i64 {
    now.div_euclid(86_400)
}

/// 记住这次录入（覆盖写，失败静默——大不了下次再敲一遍）。
pub fn remember(raw: &str) {
    remember_at(&ticket_path(), raw);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tolerates_typing_noise() {
        assert_eq!(normalize(" jiandu-2026 "), "JIANDU2026");
        assert_eq!(normalize("JIAN DU 2026"), "JIANDU2026");
        assert_eq!(normalize("ｊｉａｎｄｕ２０２６"), "JIANDU2026");
        assert_eq!(normalize("Jiandu_2026"), "JIANDU2026");
    }

    #[test]
    fn only_listed_codes_pass() {
        for c in CODES {
            assert!(valid(c), "「{c}」是该放行的码");
            assert!(valid(&c.to_lowercase()), "小写也该放行");
        }
        assert!(!valid("JIANDU2027"));
        assert!(!valid(""));
        assert!(!valid("   "));
        assert!(!valid("JIANDU"));
    }

    #[test]
    fn ticket_round_trip() {        let dir = std::env::temp_dir().join(format!("jiandu-gate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("beta_code.txt");
        assert!(!redeemed_at(&p), "文件不存在不该算已验过");
        remember_at(&p, "shuye2026");
        assert!(redeemed_at(&p), "记下之后应放行");
        std::fs::write(&p, "乱写的东西").unwrap();
        assert!(!redeemed_at(&p), "凭据内容不是有效码就不放行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 临时目录里一套「凭据 + 起算日」。
    fn fixture(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("jiandu-beta-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("beta_code.txt"), dir.join("beta_since.txt"))
    }

    #[test]
    fn beta_window_is_seven_days_then_locks() {
        let (ticket, since) = fixture("window");
        let day = 86_400i64;
        let now = 1_800_000_000i64; // 只为算天数，具体是哪天不重要
        // 没有凭据 → 压根不算内测（None，而不是「过期」）。
        assert_eq!(beta_days_left_at(&ticket, &since, now), None);

        remember_at(&ticket, "SHUYE2026");
        // 首次判定：起算日当场落盘，剩满 7 天。
        assert_eq!(beta_days_left_at(&ticket, &since, now), Some(BETA_DAYS));
        assert!(since.exists(), "起算日该被补写");
        // 同一天再问，天数不变（起算日不会被刷新成「今天」）。
        assert_eq!(beta_days_left_at(&ticket, &since, now + 3600), Some(BETA_DAYS));
        // 第 6 天还剩 1 天；第 7 天满；第 8 天起一直是 0（不出现负数）。
        assert_eq!(beta_days_left_at(&ticket, &since, now + 6 * day), Some(1));
        assert_eq!(beta_days_left_at(&ticket, &since, now + 7 * day), Some(0));
        assert_eq!(beta_days_left_at(&ticket, &since, now + 30 * day), Some(0));
        let _ = std::fs::remove_dir_all(ticket.parent().unwrap());
    }

    #[test]
    fn old_tickets_without_since_start_counting_today() {
        // v1.81 及更早发的包只写了凭据、没写起算日：升级上来的人不该
        // 「凭空到期」——首次读到就在今天起算。
        let (ticket, since) = fixture("legacy");
        let now = 1_800_000_000i64;
        remember_at(&ticket, "QINGJIAN7");
        assert!(!since.exists());
        assert_eq!(beta_days_left_at(&ticket, &since, now), Some(BETA_DAYS));
        assert_eq!(
            std::fs::read_to_string(&since).unwrap().trim(),
            today_of(now).to_string()
        );
        let _ = std::fs::remove_dir_all(ticket.parent().unwrap());
    }

    #[test]
    fn bad_since_file_falls_back_to_today_not_dead() {
        // 起算日文件写坏（乱码 / 0 / 未来日期）不该把人锁死，一律按今天重算。
        let (ticket, since) = fixture("bad");
        let now = 1_800_000_000i64;
        remember_at(&ticket, "MOJUAN88");
        for junk in ["乱写", "0", ""] {
            std::fs::write(&since, junk).unwrap();
            assert_eq!(
                beta_days_left_at(&ticket, &since, now),
                Some(BETA_DAYS),
                "起算日「{junk}」应退回今天"
            );
        }
        // 未来日期（时钟拨回去过）同样不认。
        std::fs::write(&since, (today_of(now) + 99).to_string()).unwrap();
        assert_eq!(beta_days_left_at(&ticket, &since, now), Some(BETA_DAYS));
        let _ = std::fs::remove_dir_all(ticket.parent().unwrap());
    }
}
