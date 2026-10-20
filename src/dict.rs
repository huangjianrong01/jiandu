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
}
