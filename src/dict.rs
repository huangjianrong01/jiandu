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

static DICT: Lazy<HashMap<String, String>> = Lazy::new(|| {
    serde_json::from_str(RAW).unwrap_or_default()
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
