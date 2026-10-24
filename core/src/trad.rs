//! 简繁对照、通假字表与标注查询。

use once_cell::sync::Lazy;
use std::collections::{BTreeMap, HashMap};

/// 简体 -> 繁体 对照（覆盖《道德经》样张及常用典籍用字）。
pub static S2T: Lazy<HashMap<char, char>> = Lazy::new(|| {
    let pairs: &[(&str, &str)] = &[
        ("经", "經"), ("无", "無"), ("为", "為"), ("万", "萬"), ("观", "觀"),
        ("众", "眾"), ("门", "門"), ("谓", "謂"), ("异", "異"), ("两", "兩"),
        ("恶", "惡"), ("难", "難"), ("长", "長"), ("较", "較"), ("倾", "傾"),
        ("声", "聲"), ("后", "後"), ("随", "隨"), ("圣", "聖"), ("处", "處"),
        ("辞", "辭"), ("贤", "賢"), ("争", "爭"), ("贵", "貴"), ("货", "貨"),
        ("盗", "盜"), ("见", "見"), ("乱", "亂"), ("虚", "虛"), ("实", "實"),
        ("强", "強"), ("则", "則"), ("与", "與"), ("学", "學"), ("习", "習"),
        ("书", "書"), ("读", "讀"), ("乐", "樂"), ("爱", "愛"), ("国", "國"),
        ("义", "義"), ("礼", "禮"), ("传", "傳"), ("体", "體"), ("时", "時"),
        ("风", "風"), ("云", "雲"), ("电", "電"), ("车", "車"), ("马", "馬"),
        ("鸟", "鳥"), ("鱼", "魚"), ("龙", "龍"), ("华", "華"), ("汉", "漢"),
        ("语", "語"), ("词", "詞"), ("汇", "匯"), ("译", "譯"), ("讲", "講"),
        ("议", "議"), ("论", "論"), ("证", "證"), ("试", "試"), ("题", "題"),
        ("问", "問"), ("记", "記"), ("忆", "憶"), ("温", "溫"), ("练", "練"),
        ("复", "複"), ("关", "關"), ("于", "於"), ("还", "還"), ("这", "這"),
        ("么", "麼"), ("样", "樣"), ("个", "個"), ("们", "們"), ("来", "來"),
        ("过", "過"), ("对", "對"), ("开", "開"), ("东", "東"), ("动", "動"),
        ("进", "進"), ("远", "遠"), ("边", "邊"), ("宝", "寶"), ("虚", "虛"),
    ];
    let mut m = HashMap::new();
    for (s, t) in pairs {
        if let (Some(sc), Some(tc)) = (s.chars().next(), t.chars().next()) {
            m.insert(sc, tc);
        }
    }
    m
});

/// 内置通假字表（典籍常见：借字 -> 本字）。
pub static TONGJIA: Lazy<HashMap<char, char>> = Lazy::new(|| {
    let pairs: &[(&str, &str)] = &[
        ("见", "现"), // 见(xiàn)：显露
        ("知", "智"), // 知(zhì)：智慧
        ("说", "悦"), // 说(yuè)：喜悦
        ("蚤", "早"),
        ("有", "又"),
        ("反", "返"),
        ("辩", "辨"),
        ("亡", "无"),
        ("以", "已"),
        ("锡", "赐"),
        ("陈", "阵"),
        ("共", "供"),
        ("干", "岸"),
        ("而", "能"),
    ];
    let mut m = HashMap::new();
    for (a, b) in pairs {
        if let (Some(x), Some(y)) = (a.chars().next(), b.chars().next()) {
            m.insert(x, y);
        }
    }
    m
});

/// 查一个字的繁体形（没有则返回 None）。
pub fn traditional_of(c: char) -> Option<char> {
    S2T.get(&c).copied()
}

/// 繁体 -> 简体 对照（由 `S2T` 反向生成），用于「全部转简体」。
pub static T2S: Lazy<HashMap<char, char>> = Lazy::new(|| {
    let mut m = HashMap::new();
    for (s, t) in S2T.iter() {
        m.insert(*t, *s);
    }
    m
});

/// 查一个字的简体形（没有则返回 None）。
pub fn simplified_of(c: char) -> Option<char> {
    T2S.get(&c).copied()
}

/// 查一个字的通假本字：先查书内自定义表，再查内置表。
pub fn tongjia_of(c: char, book_table: &BTreeMap<String, String>) -> Option<char> {
    if let Some(v) = book_table.get(&c.to_string()) {
        return v.chars().next();
    }
    TONGJIA.get(&c).copied()
}

/// 某段文本中是否含可标注的简体字（用于 UI 提示）。
pub fn has_annotatable(text: &str) -> bool {
    text.chars().any(|c| S2T.contains_key(&c))
}
