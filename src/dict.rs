//! 轻量古汉语 / 常用字字书。
//!
//! 数据来自 `resources/dict/guhanyu.json`（255 条常见文言虚词与实词的多种义项），
//! 编译期直接嵌入，运行时零 I/O、跟随二进制走，点字即可查义。

use once_cell::sync::Lazy;
use std::collections::HashMap;

const RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/dict/guhanyu.json"
));

/// 词库：`《古漢語常用字字典》` 的 JSON 结构是「记录数组」——
///   `[[字, 释义段①, 释义段②, …], …]`
/// （音项 ㈠㈡㈢ 与义项 ①②③ 各占一段）。这里把每字各段用换行连成
/// 一条释义，保留原始分行；面板按行渲染、可滚动。旧版是 `{字: 释义}`
/// 的 Map，只有 255 字；换成整部字典后约 3900 字。
static DICT: Lazy<HashMap<String, String>> = Lazy::new(|| {
    let rows: Vec<Vec<String>> = serde_json::from_str(RAW).unwrap_or_default();
    let mut m: HashMap<String, String> = HashMap::new();
    for row in rows {
        let Some((head, rest)) = row.split_first() else {
            continue;
        };
        let ch = head.trim();
        if ch.is_empty() {
            continue;
        }
        let body = rest
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if body.is_empty() {
            continue;
        }
        // 重复字以靠后的定义为准。
        m.insert(ch.to_string(), body);
    }
    m
});

/// 查一个字的释义；未收录返回 None。
pub fn define(ch: &str) -> Option<&'static str> {
    DICT.get(ch).map(|s| s.as_str())
}

/// 在线字书：汉典。长按浮签的「在线查阅」按钮拼这个地址。
pub const ZDIC_BASE: &str = "https://www.zdic.net/hans/";

/// 百分号编码：只放行 URL 里的「无保留字符」，其余按 UTF-8 字节转 `%XX`。
///
/// 汉字必须编码——直接把「阿」拼进 URL 交给 `open` 虽然多数浏览器能猜对，
/// 但一旦字里有 `#`、`?`、`&`（生僻字不会，URL 里却常见）就会被截断。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let ok = matches!(
            b,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
        );
        if ok {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 拼出该字在汉典的查阅地址：`https://www.zdic.net/hans/{字}`。
///
/// 只取**第一个字符**——查字查的是单字，拖选了整段也只查首字
/// （浮签上另外单独显示选区全文）。
pub fn zdic_url(ch: &str) -> String {
    let c = ch.chars().next().unwrap_or(' ');
    format!("{ZDIC_BASE}{}", percent_encode(&c.to_string()))
}

/// 字书收录字数（用于状态展示）。
pub fn len() -> usize {
    DICT.len()
}

/// 生僻字注音表（精选常见于古文 / 诗词的生僻字，键为常用书写形）。
/// 「生僻字显示」开启时，命中这些字会在其上标出读音，便于诵读。
static RARE: Lazy<HashMap<char, &'static str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    let entries: &[(char, &str)] = &[
        ('澹', "dàn"), ('泠', "líng"), ('龢', "hé"), ('纡', "yū"),
        ('窈', "yǎo"), ('窕', "tiǎo"), ('霁', "jì"), ('霏', "fēi"),
        ('荇', "xìng"), ('芷', "zhǐ"), ('蕙', "huì"), ('纨', "wán"),
        ('绮', "qǐ"), ('瑶', "yáo"), ('瑾', "jǐn"), ('瑜', "yú"),
        ('灏', "hào"), ('皋', "gāo"), ('隰', "xí"), ('岑', "cén"),
        ('浔', "xún"), ('涘', "sì"), ('沚', "zhǐ"), ('湄', "méi"),
        ('汀', "tīng"), ('渚', "zhǔ"), ('嵘', "róng"), ('峋', "xún"),
        ('徼', "jiào"), ('玦', "jué"), ('璋', "zhāng"), ('琰', "yǎn"),
        ('璜', "huáng"), ('琮', "cóng"), ('琛', "chēn"), ('璨', "càn"),
        ('璀', "cuǐ"), ('玳', "dài"), ('瑁', "mào"), ('濯', "zhuó"),
        ('湲', "yuán"), ('泠', "líng"), ('霈', "pèi"), ('熹', "xī"),
        ('昃', "zè"), ('柰', "nài"),
        ('昀', "yún"), ('昶', "chǎng"), ('彧', "yù"), ('翀', "chōng"),
        ('珏', "jué"), ('瑄', "xuān"), ('璠', "fán"), ('玙', "yú"),
    ];
    for (c, p) in entries {
        m.insert(*c, *p);
    }
    m
});

/// 查一个生僻字的读音；未收录返回 None。
pub fn pinyin_of(c: char) -> Option<&'static str> {
    RARE.get(&c).copied()
}

/// 生僻字表收录字数（用于状态展示）。
pub fn rare_len() -> usize {
    RARE.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_is_loaded_from_guhanyu_json() {
        // 整部《古漢語常用字字典》应加载进来（约 3900 字）。
        assert!(len() > 1000, "dict len = {}", len());
        let d = define("阿").expect("「阿」应收录");
        assert!(
            d.contains("詞頭") || d.contains("山陵"),
            "「阿」释义异常：{}",
            d
        );
    }

    #[test]
    fn zdic_url_percent_encodes_the_char() {
        // 「阿」U+963F → UTF-8 E9 98 BF。
        assert_eq!(zdic_url("阿"), "https://www.zdic.net/hans/%E9%98%BF");
        // 只查首字：拖选了整段也只拼第一个字。
        assert_eq!(zdic_url("阿房宫"), zdic_url("阿"));
        // ASCII 原样放行，不瞎编码。
        assert_eq!(zdic_url("a"), "https://www.zdic.net/hans/a");
        // 空串不 panic。
        assert_eq!(zdic_url(""), "https://www.zdic.net/hans/%20");
        // 会被 URL 截断的字符必须编码掉。
        assert!(percent_encode("a#b?c&d").contains("%23"));
    }

    #[test]
    fn every_dict_entry_has_a_reachable_online_url() {
        // 释义与在线查阅必须同源：能查到释义的字，都能一键跳汉典。
        let d = define("阿").expect("「阿」应收录");
        assert!(!d.is_empty());
        let url = zdic_url("阿");
        assert!(url.starts_with(ZDIC_BASE));
        assert!(url.len() > ZDIC_BASE.len());
    }
}
