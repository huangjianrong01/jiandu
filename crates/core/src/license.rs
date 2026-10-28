//! **授权（v1.81.0 · 模块六）：一枚离线 key，不绑机器。**
//!
//! # 拍板过的三条
//! 1. **不绑机**：同一枚 key 在所有机器上都能用。理由很实在——买书的人换
//!    电脑、重装系统、家里一台公司一台，是常态；绑机只会把付费用户变成
//!    客服工单。故这里**绝不读** MAC / 硬盘序列号 / 主板号，一个字节都不碰。
//! 2. **最轻量**：不联网、不引第三方授权 SDK、不做 RSA。key 自带校验位，
//!    本机只记「这枚 key 验过」。
//! 3. **IAP 以后再说**：Mac App Store 的内购走另一条路（收据校验），本模块
//!    只负责「离线 key」这一种凭据；两条路将来在
//!    [`crate::gate`] 那样的入口处取「或」即可，互不干扰。
//!
//! # 防的是什么
//! 与内测码同款口径：**防随手转发，不防破解**。校验位算法就编在二进制里，
//! 真要逆向总能造出 key——那不是这个模块要解决的问题（真要解决得靠服务器
//! 或 App Store 收据）。这里的收益是两件实在事：
//!   * **挡错字**：手抄 key 漏一个字符，校验位立刻不认，用户当场知道抄错了，
//!     而不是「输进去说无效，反复试到怀疑人生」；
//!   * **挡瞎猜**：16 位里 4 位是校验，随手编一串能过检的概率约百万分之一。
//!
//! # key 长相
//! `XXXX-XXXX-XXXX-XXXX`：前 12 位是**载荷**（随机），后 4 位是**校验位**
//! （由载荷 + 固定盐算出）。字符表去掉了 `0/O`、`1/I` 这类手抄必混的字形。
//! 归一化后是 16 位无连字符的大写串——落盘也存这个形态。

use crate::library::Library;
use std::path::PathBuf;

/// 固定盐：换掉它就等于换了一整套 key（旧 key 全部失效）。
pub const SALT: &str = "JIANDU-LIC-1";

/// 字符表（32 个）：数字去 `0` `1`，字母去 `I` `O`——手抄时最容易混的四枚。
///
/// 32 个是有意的：正好 5 位一字符，校验位按 5 比特切，不必做除法。
pub const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// 载荷长度（字符数）。
pub const PAYLOAD_LEN: usize = 12;
/// 校验位长度（字符数）：4 × 5bit = 20bit，瞎猜命中率约百万分之一。
pub const CHECK_LEN: usize = 4;
/// 归一化后的总长度。
pub const KEY_LEN: usize = PAYLOAD_LEN + CHECK_LEN;

/// 归一化：去空白与连字符、全角转半角、统一大写。
///
/// 用户是从邮件 / 微信里复制 key 的，前后带空格、带全角、小写，全都可能——
/// 这些都不该判成无效。
pub fn normalize(raw: &str) -> String {
    raw.chars()
        .filter_map(|c| {
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

/// FNV-1a 64 + 一次雪崩搅动。
///
/// 只用 FNV-1a 的话，改载荷末尾一个字节，哈希低位只线性挪一点，太"乖"；
/// 末尾再走一遍 murmur3 的 finalizer，让改动彻底扩散。
fn digest(payload: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in SALT.bytes().chain(std::iter::once(b':')).chain(payload.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// 由载荷算出 4 位校验串。
pub fn check_of(payload: &str) -> String {
    let mut x = digest(payload);
    let mut out = String::new();
    for _ in 0..CHECK_LEN {
        out.push(ALPHABET[(x & 0x1f) as usize] as char);
        x >>= 5;
    }
    out
}

/// **铸一枚 key**：给 12 位载荷，补上校验位返回 16 位串。
///
/// 载荷里不在字符表上的字符会被丢掉——铸 key 的人（脚本）不该被信任到
/// 「直接拿它当哈希输入」的程度，宁可静默丢也不能产出表里没有的字形。
pub fn mint(payload: &str) -> String {
    let payload: String = payload
        .chars()
        .filter(|c| ALPHABET.contains(&(*c as u8)))
        .take(PAYLOAD_LEN)
        .collect();
    format!("{}{}", payload, check_of(&payload))
}

/// 这枚 key 对不对。
pub fn valid(raw: &str) -> bool {
    let n = normalize(raw);
    if n.chars().count() != KEY_LEN {
        return false;
    }
    if !n.bytes().all(|b| ALPHABET.contains(&b)) {
        return false;
    }
    let payload = &n[..PAYLOAD_LEN];
    &n[PAYLOAD_LEN..] == check_of(payload)
}

/// 展示形态：`XXXX-XXXX-XXXX-XXXX`。
///
/// 16 位连成一串是没法核对的——人一次只能记 4 个字符。分组是给人眼用的，
/// 归一化时会把连字符去掉，来回无损。
pub fn pretty(raw: &str) -> String {
    let n = normalize(raw);
    n.chars()
        .collect::<Vec<char>>()
        .chunks(4)
        .map(|g| g.iter().collect::<String>())
        .collect::<Vec<String>>()
        .join("-")
}

/// 凭据落盘位置（应用数据目录；沙盒下自动落进容器）。
///
/// 与内测码那张凭据**分开两只文件**：内测码是「能不能进这个包」，授权是
/// 「能不能出活」。两件事的生命期不同（内测码发完就废，授权要留着），
/// 混在一只文件里将来没法单独吊销。
pub fn ticket_path() -> PathBuf {
    Library::home().join("license.txt")
}

/// 某个凭据文件是否已激活（**纯 I/O**：单测直接喂临时路径，不去动
/// `JIANDU_HOME` 那个全局口子——并行测试下会互相踩）。
pub fn activated_at(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| valid(&s))
        .unwrap_or(false)
}

/// 记下这枚 key。**只有有效的才写**——写一枚无效凭据等于把下次的报错
/// 藏起来（启动读回来是「已激活」，用户却不知道那串是错的）。
///
/// 返回是否真的写成了（即 key 是否有效）。
pub fn activate_at(path: &std::path::Path, raw: &str) -> bool {
    if !valid(raw) {
        return false;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, normalize(raw)).is_ok()
}

/// 本机是否已激活。
pub fn activated() -> bool {
    activated_at(&ticket_path())
}

/// 记下这枚 key（本机），返回是否激活成功。
pub fn activate(raw: &str) -> bool {
    activate_at(&ticket_path(), raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_has_32_unambiguous_glyphs() {
        assert_eq!(ALPHABET.len(), 32, "32 个才够每字符 5 比特");
        for bad in ['0', '1', 'I', 'O'] {
            assert!(
                !ALPHABET.contains(&(bad as u8)),
                "「{bad}」手抄必混，不该进字符表"
            );
        }
    }

    #[test]
    fn normalize_tolerates_copy_paste_noise() {
        // 从微信里复制出来常带空格 / 全角 / 小写。
        assert_eq!(normalize(" abcd-efgh "), "ABCDEFGH");
        assert_eq!(normalize("ａｂｃｄ"), "ABCD");
        assert_eq!(normalize("ab_cd"), "ABCD");
    }

    #[test]
    fn minted_keys_always_pass() {
        // 载荷随机：抽几组不同形态的，铸出来必须自己认。
        for payload in [
            "23456789ABCD",
            "ZZZZZZZZZZZZ",
            "AAAAAAAAAAAA",
            "JKLMNPQRSTUV",
            "WXYZ23456789",
        ] {
            let k = mint(payload);
            assert_eq!(k.chars().count(), KEY_LEN, "{payload} → {k}");
            assert!(valid(&k), "铸出来的 key 自己得认：{k}");
            assert!(valid(&pretty(&k)), "分组展示后仍要认：{}", pretty(&k));
        }
    }

    #[test]
    fn checksum_rejects_single_character_typos() {
        // 手抄错一个字就该立刻不认——这是校验位存在的全部意义。
        let k = mint("23456789ABCD");
        let cs: Vec<char> = k.chars().collect();
        for i in 0..cs.len() {
            for c in ALPHABET.iter().map(|b| *b as char) {
                if c == cs[i] {
                    continue;
                }
                let mut wrong: Vec<char> = cs.clone();
                wrong[i] = c;
                let wrong: String = wrong.into_iter().collect();
                assert!(
                    !valid(&wrong),
                    "改一位「{k}」→「{wrong}」居然还认，校验位白算了"
                );
            }
        }
    }

    #[test]
    fn wrong_length_and_foreign_glyphs_are_rejected() {
        assert!(!valid(""));
        assert!(!valid("23456789ABC")); // 少一位
        assert!(!valid("23456789ABCDE")); // 多一位
        // 字符表外的字形（0 / 1 / I / O）：
        let mut k = mint("23456789ABCD");
        k.push('0');
        assert!(!valid(&k));
        assert!(!valid("0000000000000000"));
    }

    #[test]
    fn invalid_keys_are_never_written_to_disk() {
        let dir = std::env::temp_dir().join(format!("jiandu-lic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("license.txt");
        assert!(!activated_at(&p), "文件不存在不该算已激活");
        assert!(!activate_at(&p, "乱写的"), "无效 key 不该落盘");
        assert!(
            !p.exists(),
            "无效 key 连文件都不该建——否则下次读回来是「已激活」"
        );
        let k = mint("JKLMNPQRSTUV");
        assert!(activate_at(&p, &k), "有效 key 该写成功");
        assert!(activated_at(&p));
        std::fs::write(&p, "23456789ABCD0000").unwrap();
        assert!(!activated_at(&p), "凭据内容是坏的就不算激活");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_minted_by_the_python_script_are_accepted() {
        // 与 `scripts/gen_license.py` 对表：脚本铸的 key 必须被这里认。
        // 两边算法一旦走偏（改了盐或搅动步骤），这条会先红。
        // 这三枚是 `python3 scripts/gen_license.py -n 3 --raw` 铸出来的。
        for k in [
            "FJSJ83HG32QZ7RXU",
            "U5XFE264ZY8UV65Y",
            "7VLYJM44JY9V796U",
        ] {
            assert!(valid(k), "脚本铸的 key 不认：{k}");
            assert!(valid(&pretty(k)));
        }
    }
}
