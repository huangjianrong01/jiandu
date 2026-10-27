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
    fn ticket_round_trip() {
        let dir = std::env::temp_dir().join(format!("jiandu-gate-{}", std::process::id()));
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
}
