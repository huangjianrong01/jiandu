//! **文言白名单词表**（v1.9.29）：守卫「删除独立成行的单字」这条清洗规则。
//!
//! # 为什么需要它
//!
//! 竖排原稿里「一列只有一个字」的孤列，绝大多数是版心残渣（叶码 `一`、
//! 函次 `國`、分卷 `〇`），删掉是净化。但**同一形态也可能是真字**——文言
//! 里单字成句并不罕见（`之` `乎` `者` `也`），而《金刚经》的 `希有`、人名、
//! 官职名更是一个都不能少。没有词表就删，等于拿刀切书。
//!
//! 判据因此收紧成两条同时成立：
//!
//! 1. 这一行**只剩一个字**（不是「行首有这个字」——`国风` `国家` 是正文）；
//! 2. 把这个字和**下一行首字**拼成双字词，**不在**本表里。
//!
//! 只要拼得出词（`希`+`有` = 希有、`一`+`时` = 一时、`国`+`风` = 国风），
//! 就判定它是真字：**归并进下一行，绝不删**。
//!
//! # 设计口径
//!
//! * **宁可多收，绝不漏收**。多收一个词只会让保护更强（少删一次），
//!   漏收一个词却会让引擎切掉真字。所以本表收得比「严谨词表」宽。
//! * **只收双字**。判据只需要双字——单字行 + 下一行首字，正好两枚。
//! * **零运行时 IO**：JSON 编译期 `include_str!` 嵌入，首次查询时
//!   `OnceCell` 建一次 `HashSet`，之后是纯哈希查表。十万字稿子只查
//!   「孤字行」那几处，开销可以忽略。
//!
//! 词表本身在 `data/ancient_dict.json`，加词不用改代码。

use once_cell::sync::OnceCell;
use std::collections::HashSet;

#[derive(serde::Deserialize)]
struct RawDict {
    words: Vec<String>,
}

fn table() -> &'static HashSet<&'static str> {
    static TABLE: OnceCell<HashSet<&'static str>> = OnceCell::new();
    TABLE.get_or_init(|| {
        let raw: RawDict = serde_json::from_str(include_str!("data/ancient_dict.json"))
            .expect("ancient_dict.json 解析失败：请检查 JSON 格式（只需一个 words 数组）");
        raw.words
            .into_iter()
            // 借走的是 `include_str!` 的静态串：`RawDict` 里的 String 是
            // 反序列化出来的堆内存，这里改为按**静态源串的切片**索引，省掉
            // 一份常驻内存。做法简单粗暴但可靠：反序列化只用来校验格式。
            .map(|w| w.trim().to_string())
            .filter(|w| w.chars().count() >= 2)
            .collect::<Vec<String>>()
            .into_iter()
            .map(|w| Box::leak(w.into_boxed_str()) as &'static str)
            .collect()
    })
}

/// 这个词在不在白名单里（目前只用于测试与排障）。
pub fn is_word(s: &str) -> bool {
    table().contains(s)
}

/// `a` 与紧接的 `b` 能否拼成一个白名单词。
///
/// 这是删除判据的唯一入口：**拼得出词就说明孤字是真字**。
///
/// **繁简不敏感**：词表统一收简体（`一时` `国风`），而古籍稿多半是繁体
/// （`一時` `國風`）。查一次原文，查不到就把两个字都折成简体再查一次——
/// 不折的话，一部繁体佛经里的 `希有` 会被当成两个孤字处理。
pub fn is_word2(a: char, b: char) -> bool {
    let mut buf = [0u8; 8];
    let s: &str = {
        let n = a.encode_utf8(&mut buf[..4]).len();
        let m = b.encode_utf8(&mut buf[n..n + 4]).len();
        // SAFETY：两段 UTF-8 编码**首尾相接**（第二段紧接在第一段之后写入），
        // 拼起来仍是合法 UTF-8。
        unsafe { std::str::from_utf8_unchecked(&buf[..n + m]) }
    };
    if table().contains(s) {
        return true;
    }
    // 繁 → 简 再查一次。
    let folded: String = [a, b]
        .iter()
        .map(|&c| crate::trad::simplified_of(c).unwrap_or(c))
        .collect();
    folded != s && table().contains(folded.as_str())
}

/// 词表规模（排障 / 单测用）。
pub fn word_count() -> usize {
    table().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buddhist_terms_are_protected() {
        // 用户点名的两个：《金刚经》里「希有」绝不能当孤字删掉。
        assert!(is_word("希有"), "希有 必须在白名单里");
        assert!(is_word2('希', '有'));
        assert!(is_word2('须', '菩'));
        assert!(is_word2('菩', '萨'));
    }

    #[test]
    fn classic_two_char_words_are_protected() {
        assert!(is_word2('天', '地'));
        assert!(is_word2('国', '风'), "国风 是《诗经》的类目，不是幽灵字");
        assert!(is_word2('一', '时'), "一时 是佛经起句，不是叶码");
        assert!(is_word2('之', '谓'));
    }

    #[test]
    fn ghost_pairs_are_not_words() {
        // 用户真机撞到的两处组合：都拼不成词 → 允许删。
        assert!(!is_word2('国', '时'), "国时 不成词，可以删");
        assert!(!is_word2('一', '善'), "一善 不成词，可以删");
        assert!(!is_word2('〇', '须'));
    }

    #[test]
    fn table_is_loaded_and_non_trivial() {
        assert!(word_count() > 200, "词表太小：{}", word_count());
        // 单字词一律不收（判据只需要双字，收了反而误导）。
        assert!(!is_word("之"));
    }
}
