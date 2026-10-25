//! 轻量古汉语 / 常用字字书。
//!
//! 数据来自 `resources/dict/guhanyu.json`（255 条常见文言虚词与实词的多种义项），
//! 编译期直接嵌入，运行时零 I/O、跟随二进制走，点字即可查义。

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;

const RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../resources/dict/guhanyu.json"
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

// ----------------------------------------------------------------------
// 简明释义：高频常用字的一行现代语体概括
// ----------------------------------------------------------------------
//
// 浮签新增的「简明」页签就吃这张表：给文言高频字一句现代人读得懂的概括
// （「之」＝ 往 / 到；代词：这、他；助词），比康熙的反切、說文的篆本部首
// 更省事。这是一份**离线手写小表**（首期 20+ 高频字，之乎者也为首），
// 不依赖大文件；后续换成整部《古汉语常用字字典》的「首义」字段即可无缝替换。
///
/// 查询优先级（`concise_of`）：原字 → 繁体 → 简体，与字书同一套兜底口径。
static CONCISE: Lazy<HashMap<char, &'static str>> = Lazy::new(|| {
    let entries: &[(char, &str)] = &[
        ('之', "往、到；代词：这、他；助词：的、取消句子独立性"),
        ('乎', "介词「于」；语气词：吗、呢、呀"),
        ('者', "代词：…的人 / 事 / 物；助词，表提顿"),
        ('也', "语气词：表判断、解释、肯定"),
        ('其', "代词：他 / 它 / 那；副词：大概、难道"),
        ('而', "连词：又、而且、却、如果"),
        ('以', "介词：用、把、凭；连词：来、因为"),
        ('于', "介词：在、向、对、到"),
        ('是', "这、此；判断词：对、正确"),
        ('不', "否定副词：不、没"),
        ('人', "人类；别人；人才"),
        ('天', "天空；天然；天命"),
        ('下', "下面；降下；下属"),
        ('子', "子女；对人的尊称；种子"),
        ('君', "君主；对男子的尊称"),
        ('长', "cháng 短之长；zhǎng 生长、尊长"),
        ('安', "安稳；怎么、哪里"),
        ('山', "地面高耸的部分"),
        ('水', "无色液体；河流；水灾"),
        ('风', "空气流动；教化；风气"),
        ('月', "月球；月份；月光"),
        ('日', "太阳；日子；每天"),
        ('年', "地球绕日周期；岁数；年成"),
        ('心', "心脏；内心；思想"),
        ('生', "生长；活着；生计"),
        ('死', "生命终止；坚决"),
        ('道', "路；道理；学说；说"),
        ('德', "道德；恩惠；德性"),
        ('仁', "仁爱；仁德"),
        ('义', "道义；合理；意义"),
        ('礼', "礼节；礼制"),
        ('智', "智慧；智谋"),
        ('信', "诚信；相信；信息"),
        ('学', "学习；学问"),
        ('行', "行走；品行；做"),
        ('言', "言语；说"),
        ('文', "花纹；文字；文章"),
        ('书', "书写；书籍；文书"),
        ('诗', "诗歌"),
        ('国', "国家；国都"),
        ('家', "家庭；住所；学派"),
        ('春', "春季"),
        ('秋', "秋季；年；时候"),
        ('江', "长江；大江"),
        ('河', "黄河；河流"),
        ('花', "花朵；开花"),
        ('草', "草本植物"),
        ('木', "树木；木材"),
        ('金', "金属；黄金；钱财"),
        ('玉', "美石；玉佩"),
        ('王', "君主；一类之首"),
        ('侯', "诸侯；爵位"),
        ('将', "jiāng 将要；jiàng 将领"),
        ('士', "士人；士兵；男子"),
        ('女', "女性；女儿"),
        ('夫', "成年男子；丈夫；发语词"),
        ('母', "母亲；女性长辈"),
        ('父', "父亲；男性长辈"),
    ];
    let mut m = HashMap::new();
    for (c, s) in entries {
        m.insert(*c, *s);
    }
    m
});

/// 简明释义：原字 → 繁体 → 简体三级兜底（与字书同口径）。
///
/// 浮签「简明」页签显示的就是它；查不到返回 `None`（该字走康熙 / 說文，或
/// 三库皆空时退化到汉典在线考据）。
pub fn concise_of(ch: char) -> Option<&'static str> {
    CONCISE
        .get(&ch)
        .copied()
        .or_else(|| crate::trad::traditional_of(ch).and_then(|t| CONCISE.get(&t).copied()))
        .or_else(|| crate::trad::simplified_of(ch).and_then(|s| CONCISE.get(&s).copied()))
}

// ----------------------------------------------------------------------
// 词条：拼音 + 词性 + 释义（浮签要按「字 / 拼音 / 释义」三列竖排）
// ----------------------------------------------------------------------
//
// 字书那部 `guhanyu.json` 只有释义，没有拼音与词性；浮签的第二列（拼音）
// 与「词性」标签得有专门的表。这里放一份**轻量本地词条**：词组取自
// `resources/dict/phrases.json`（长安、曰、天下…），单字的拼音另有一张
// 常用字表，两者拼成同一枚 `Entry`，浮签不必关心它从哪来。

/// 一枚词条：浮签的三列就是它。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 词（单字或词组）。
    pub word: String,
    /// 拼音（词组带空格分隔）。
    pub pinyin: String,
    /// 词性（名 / 动 / 形 / 连 ……）；单字无可考时给「字」。
    pub pos: String,
    /// 释义。
    pub gloss: String,
}

#[derive(serde::Deserialize)]
struct RawPhrase {
    word: String,
    pinyin: String,
    pos: String,
    gloss: String,
}

const PHRASE_RAW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../resources/dict/phrases.json"
));

/// 词组表（编译期嵌入，运行时零 I/O）。
static PHRASES: Lazy<Vec<RawPhrase>> =
    Lazy::new(|| serde_json::from_str(PHRASE_RAW).unwrap_or_default());

/// 常用字拼音表（文言高频字；生僻字另见 `RARE`）。
static PINYIN: Lazy<HashMap<char, &'static str>> = Lazy::new(|| {
    let entries: &[(char, &str)] = &[
        ('曰', "yuē"), ('之', "zhī"), ('乎', "hū"), ('者', "zhě"), ('也', "yě"),
        ('其', "qí"), ('而', "ér"), ('以', "yǐ"), ('于', "yú"), ('是', "shì"),
        ('不', "bù"), ('人', "rén"), ('天', "tiān"), ('下', "xià"), ('子', "zǐ"),
        ('君', "jūn"), ('长', "cháng"), ('安', "ān"), ('山', "shān"), ('水', "shuǐ"),
        ('风', "fēng"), ('月', "yuè"), ('日', "rì"), ('年', "nián"), ('心', "xīn"),
        ('生', "shēng"), ('死', "sǐ"), ('道', "dào"), ('德', "dé"), ('仁', "rén"),
        ('义', "yì"), ('礼', "lǐ"), ('智', "zhì"), ('信', "xìn"), ('学', "xué"),
        ('行', "xíng"), ('言', "yán"), ('文', "wén"), ('书', "shū"), ('诗', "shī"),
        ('國', "guó"), ('国', "guó"), ('家', "jiā"), ('春', "chūn"), ('秋', "qiū"),
        ('江', "jiāng"), ('河', "hé"), ('花', "huā"), ('草', "cǎo"), ('木', "mù"),
        ('金', "jīn"), ('玉', "yù"), ('王', "wáng"), ('侯', "hóu"), ('将', "jiàng"),
        ('士', "shì"), ('女', "nǚ"), ('夫', "fū"), ('母', "mǔ"), ('父', "fù"),
    ];
    let mut m = HashMap::new();
    for (c, p) in entries {
        m.insert(*c, *p);
    }
    m
});

/// 查一个字的拼音：常用字表 → 生僻字表。
pub fn pinyin_for(ch: char) -> Option<&'static str> {
    PINYIN.get(&ch).copied().or_else(|| pinyin_of(ch))
}

/// 查词组（**最长匹配**）：在词条表里找 `text` 开头的词。
///
/// 浮签拖选了「长安」就出「长安」这一条，而不是「长」+「安」两条——
/// 查词嘛，自然是成词优先。
pub fn phrase(text: &str) -> Option<Entry> {
    let chars: Vec<char> = text.chars().collect();
    let max = chars.len().min(4);
    if max < 2 {
        return None;
    }
    for n in (2..=max).rev() {
        let head: String = chars[..n].iter().collect();
        if let Some(p) = PHRASES.iter().find(|p| p.word == head) {
            return Some(Entry {
                word: p.word.clone(),
                pinyin: p.pinyin.clone(),
                pos: p.pos.clone(),
                gloss: p.gloss.clone(),
            });
        }
    }
    None
}

/// 查单字：释义取字书（原字 → 繁体 → 简体），拼音取拼音表。
pub fn entry_of(ch: char) -> Option<Entry> {
    let s = ch.to_string();
    let gloss = crate::dict::define(&s)
        .or_else(|| {
            crate::trad::traditional_of(ch).and_then(|t| crate::dict::define(&t.to_string()))
        })
        .or_else(|| {
            crate::trad::simplified_of(ch).and_then(|t| crate::dict::define(&t.to_string()))
        })?;
    Some(Entry {
        word: s,
        pinyin: pinyin_for(ch).unwrap_or("—").to_string(),
        // 单字不标词性：古汉语一字兼类太常见，标了反而误导。
        pos: "字".to_string(),
        gloss: gloss.to_string(),
    })
}

/// **浮签的统一入口**：先试成词，再退到单字。
///
/// `text` 是长按 / 拖选得到的文本（可能一整段）：只取开头去匹配，
/// 匹配不上就查首字。查不到返回 `None`（浮签上给出「可点考据原典」）。
pub fn lookup(text: &str) -> Option<Entry> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    phrase(t).or_else(|| t.chars().next().and_then(entry_of))
}

/// 词条表收录条数（状态展示用）。
pub fn phrase_len() -> usize {
    PHRASES.len()
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

// ----------------------------------------------------------------------
// 双引擎离线字书：《康熙字典》 × 《說文解字》
// ----------------------------------------------------------------------
//
// `guhanyu.json` 那部是「古汉语常用字字典」——**释义现代汉语化、收字 3900**，
// 读《史记》够用，但遇上「𠀤」「㫃」这类字就哑了。这两部才是查字的正主：
//   * 《康熙字典》48709 条：反切、叶音、历代韵书引文、部首卷次；
//   * 《說文解字》11037 字头：小篆本义与徐铉 / 段注系统的原文。
//
// 两部由 `scripts/merge_dict.py` 在构建期合成**一个** `dict.db`（说文本来是
// 9833 个碎 JSON，逐个打包既不体面也慢），再由 `include_bytes!` 嵌进二进制：
// 运行时零 I/O、断网可查、跟随 .app 走。
//
// SQLite 只能读文件、读不了内存里的 `&[u8]`，故首次查询时把嵌入的字节落到
// 一个缓存路径再以只读方式打开；长度对不上就重写（升版后嵌的是新库）。

/// 合并后的字书库（康熙 + 說文）。
const ZISHU_DB: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../resources/dict/dict.db"
));

/// **一条字书记录**：康熙释义与說文原文分开存，浮签分栏各列一列。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zishu {
    /// 字头（正条，多为繁体 / 传统字形）。
    pub word: String,
    /// 拼音（带调，来自說文条目）。
    pub pinyin: String,
    /// 部首。
    pub radical: String,
    /// 反切（“於悉切”）。
    pub fanqie: String,
    /// 康熙部首卷次（“【寅集上】【子字部】”）。
    pub volume: String,
    /// 《康熙字典》释义原文。
    pub kangxi: String,
    /// 《說文解字》原文。
    pub shuowen: String,
}

impl Zishu {
    /// 两部都没收到内容就算没查到（只有空壳字头不值得浮一张签）。
    pub fn is_empty(&self) -> bool {
        self.kangxi.is_empty() && self.shuowen.is_empty()
    }
}

/// 字书缓存落盘路径：`~/.jiandu/zishu.db`（拿不到 HOME 就退回临时目录）。
fn zishu_cache_path() -> std::path::PathBuf {
    let dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|h| h.join(".jiandu"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    dir.join("zishu.db")
}

/// 落盘并打开只读连接；任何一步失败都返回 `None`（查词降级，绝不崩）。
fn open_zishu() -> Option<rusqlite::Connection> {
    let path = zishu_cache_path();
    let stale = std::fs::metadata(&path)
        .map(|m| m.len() != ZISHU_DB.len() as u64)
        .unwrap_or(true);
    if stale {
        std::fs::write(&path, ZISHU_DB).ok()?;
    }
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

/// 字书连接（懒开：第一次查词才落盘，启动不付这份代价）。
static ZISHU: Lazy<Mutex<Option<rusqlite::Connection>>> =
    Lazy::new(|| Mutex::new(open_zishu()));

/// 字书可用吗（状态栏 / 兜底判断用）。
pub fn zishu_ready() -> bool {
    matches!(ZISHU.lock(), Ok(g) if g.is_some())
}

/// 字书原文的**显示层**净化：只洗网页残渣，不动一个字的正文。
///
/// 库里的康熙 / 說文正文是从网页版字书灌下来的，段落之间夹着 `<hr>`、`<br>`
/// 这类标签，还有 `&nbsp;` 之类的实体。竖排时看不出来（标签会被竖排引擎
/// 当西文挤掉），改横排之后它们就成了宣纸便签上的一行尖括号，比释义本身
/// 还抢眼。这里做三件事：剥标签（块级标签换成全角空格断句）、还原几个最
/// 常见的实体、把连写的全角空格收成一枚。**不做**任何改写、删节、繁简转换。
pub fn tidy_zishu(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(j) = chars[i + 1..].iter().position(|&c| c == '>') {
                let tag: String = chars[i + 1..i + 1 + j].iter().collect();
                let name: String = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                if matches!(name.as_str(), "hr" | "br" | "p" | "div" | "li" | "tr") {
                    out.push('　');
                }
                i += j + 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    let out = out
        .replace("&nbsp;", "　")
        .replace("&lt;", "〈")
        .replace("&gt;", "〉")
        .replace("&quot;", "「")
        .replace("&amp;", "&");
    // 连写的全角空格（网页拿它做段首缩进）收成一枚，免得行首空一大片。
    let mut tidy = String::with_capacity(out.len());
    let mut prev_space = false;
    for c in out.chars() {
        let sp = matches!(c, '　' | ' ' | '\n' | '\r' | '\t');
        if sp {
            if !prev_space {
                tidy.push('　');
            }
        } else {
            tidy.push(c);
        }
        prev_space = sp;
    }
    tidy.trim_matches('　').to_string()
}

/// 按一个字头查一条记录（`alias` 表已含繁简两种写法，故简繁都能命中）。
///
/// **同字兄弟回填**（v1.42.0）：排序第一优先是「繁体字头」——简写「学」要
/// 落到「學」。但库里同一个字常有几条（「游」与「遊」），繁体那条的
/// `pinyin` / `shuowen` 常常是 NULL，于是浮签上拼音整栏空白、《說文》那一
/// 页也是空的（用户报的「游」没拼音，就是命中了缺字段的「遊」）。故在主行
/// 选定之后，这两列再从**同一输入字**的其它字头里各补一次——补的是同一个
/// 字的异体写法，不算串字。主行本身有值就用主行的。
fn query_word(conn: &rusqlite::Connection, w: &str) -> Option<Zishu> {
    let mut st = conn
        .prepare(
            "SELECT d.word,
                    ifnull(d.content, ''),
                    ifnull(nullif(d.pinyin, ''),
                           (SELECT d2.pinyin FROM dict d2 JOIN alias a2 ON d2.word = a2.head
                            WHERE a2.word = ?1 AND nullif(d2.pinyin, '') IS NOT NULL
                            LIMIT 1)),
                    ifnull(d.radical, ''),
                    ifnull(d.fanqie, ''),
                    ifnull(d.volume, ''),
                    ifnull(nullif(d.shuowen, ''),
                           (SELECT d3.shuowen FROM dict d3 JOIN alias a3 ON d3.word = a3.head
                            WHERE a3.word = ?1 AND nullif(d3.shuowen, '') IS NOT NULL
                            LIMIT 1))
             FROM alias a JOIN dict d ON d.word = a.head
             WHERE a.word = ?1
             ORDER BY (d.word = ifnull((SELECT trad FROM variant
                                        WHERE word = ?1 LIMIT 1), ?1)) DESC,
                      (ifnull(d.shuowen, '') <> '') DESC,
                      length(d.content) DESC
             LIMIT 1",
        )
        .ok()?;
    let mut rows = st.query(rusqlite::params![w]).ok()?;
    let r = rows.next().ok()??;
    Some(Zishu {
        word: r.get(0).unwrap_or_default(),
        kangxi: tidy_zishu(&r.get::<_, String>(1).unwrap_or_default()),
        // 回填的子查询可能仍是 NULL（整字都没有拼音），`unwrap_or_default`
        // 拿到 NULL 会退化成空串——浮签那一行就不画，不留空壳。
        pinyin: r.get::<_, String>(2).unwrap_or_default(),
        radical: r.get(3).unwrap_or_default(),
        fanqie: r.get(4).unwrap_or_default(),
        volume: r.get(5).unwrap_or_default(),
        shuowen: tidy_zishu(&r.get::<_, String>(6).unwrap_or_default()),
    })
}

/// **查词总入口**：原字 → 繁体 → 简体，三级兜底。
///
/// 兜底的两条路：
///   1. 库里的 `alias` 表——构建期用 macOS ICU 把字书里出现过的每个字批量
///      转成简体而建（2898 组），简写输入直接命中繁体字头；
///   2. `crate::trad` 的简繁对照——跨不出 ICU 那张表时再补一道。
/// 两道都不成（生僻字、标点、拉丁字母）就返回 `None`，浮签退回旧字书。
pub fn query_dict(ch: char) -> Option<Zishu> {
    let guard = ZISHU.lock().ok()?;
    let conn = guard.as_ref()?;
    let own = ch.to_string();
    query_word(conn, &own)
        .or_else(|| {
            crate::trad::traditional_of(ch)
                .and_then(|t| query_word(conn, &t.to_string()))
        })
        .or_else(|| {
            crate::trad::simplified_of(ch).and_then(|s| query_word(conn, &s.to_string()))
        })
        .filter(|z| !z.is_empty())
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

    /// 拼音不能因为命中了繁体字头而丢：同一个字的兄弟字头要能回填。
    ///
    /// 「游」在库里有两条（游 / 遊），排序先取繁体字头「遊」，而「遊」那行
    /// 的 `pinyin` 与 `shuowen` 都是 NULL——不回填的话浮签上「游」就没有
    /// 拼音，切到《說文》那一页也是空的。
    #[test]
    fn zishu_backfills_pinyin_from_sibling_head() {
        let z = query_dict('游').expect("字书应收录「游」");
        assert_eq!(z.pinyin, "yóu", "拼音没回填：{:?}", z.pinyin);
        assert!(!z.shuowen.is_empty(), "說文没回填：{:?}", z.shuowen);
        // 主行有值时不能被兄弟行顶掉。
        let du = query_dict('讀').expect("「讀」应收录");
        assert_eq!(du.pinyin, "dú");
        assert!(du.shuowen.contains("誦書也"));
    }

    /// 字书原文的网页残渣必须洗掉：`<hr>` 之类的标签绝不进浮签。
    #[test]
    fn tidy_zishu_strips_html_residue() {
        let raw = "反其旄倪。<hr>考證：〔【說文】孩子也。象形。〕　謹照原文。<br />又　&nbsp;云。";
        let s = tidy_zishu(raw);
        assert!(!s.contains('<'), "标签没剥干净：{s}");
        assert!(!s.contains("&nbsp;"), "实体没还原：{s}");
        assert!(s.contains("考證"), "正文被误删：{s}");
        assert!(s.contains("孩子也"), "正文被误删：{s}");
        // 连写的全角空格收成一枚，且首尾不留空。
        assert!(!s.contains("　　　"), "空格没收：{s}");
        assert_eq!(s.trim(), s);
        // 空串 / 纯标签都退化成空。
        assert_eq!(tidy_zishu(""), "");
        assert_eq!(tidy_zishu("<hr>"), "");
    }

    /// 字书嵌进来了吗——没嵌（脚本没跑 / 资源缺失）就谈不上后面几条。
    #[test]
    fn zishu_db_is_embedded() {
        assert!(
            ZISHU_DB.len() > 1_000_000,
            "dict.db 只有 {} 字节，像是没合并",
            ZISHU_DB.len()
        );
        assert!(zishu_ready(), "字书没能打开（落盘失败？）");
    }

    /// 双引擎：同一字要同时给出康熙释义与說文原文。
    #[test]
    fn zishu_merges_kangxi_and_shuowen() {
        let z = query_dict('讀').expect("字书应收录「讀」");
        assert!(z.kangxi.contains("徒谷切"), "康熙缺反切：{}", z.kangxi);
        assert!(z.shuowen.contains("誦書也"), "說文缺原文：{}", z.shuowen);
        assert_eq!(z.pinyin, "dú");
        assert_eq!(z.radical, "言");
    }

    /// 繁简兜底：拿简体字查，要能落到繁体字头上（ICU 对照表建的 alias）。
    #[test]
    fn zishu_falls_back_from_simplified() {
        let z = query_dict('学').expect("简体「学」应兜底到繁体字头");
        assert_eq!(z.word, "學");
        assert!(!z.kangxi.is_empty());
    }

    /// 库里没收的东西（拉丁字母）不该拿空壳出来撑一张浮签。
    #[test]
    fn zishu_returns_none_for_unknown() {
        assert!(query_dict('A').is_none());
        assert!(query_dict('，').is_none());
    }

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
    fn phrase_lookup_prefers_the_longest_word() {
        // 「长安」成词：拖选它不能拆成「长」+「安」。
        let e = lookup("长安").expect("「长安」应收录");
        assert_eq!(e.word, "长安");
        assert_eq!(e.pinyin, "cháng ān");
        assert!(!e.pos.is_empty());
        // 「长安道」仍先命中「长安」（最长匹配，但不越过 4 字）。
        assert_eq!(lookup("长安道上").map(|e| e.word), Some("长安".to_string()));
        // 单字「曰」：拼音与词性齐全。
        let y = lookup("曰").expect("「曰」应收录");
        assert_eq!(y.pinyin, "yuē");
        assert_eq!(y.pos, "字");
        assert!(!y.gloss.is_empty());
    }

    #[test]
    fn single_char_falls_back_to_the_character_book() {
        // 词组表里没有的字，退到字书（拼音表没有就给「—」）。
        let e = lookup("阿").expect("「阿」字书有收");
        assert_eq!(e.word, "阿");
        assert_eq!(e.pos, "字");
        // 拼音表收了「天」。
        assert_eq!(pinyin_for('天'), Some("tiān"));
        // 生僻字表兜底。
        assert_eq!(pinyin_for('窈'), Some("yǎo"));
        // 空串 / 空白不 panic，也不出条目。
        assert!(lookup("").is_none());
        assert!(lookup("   ").is_none());
    }

    #[test]
    fn phrase_table_is_loaded_and_well_formed() {
        assert!(phrase_len() >= 10, "词条太少：{}", phrase_len());
        for p in PHRASES.iter() {
            // 单字（「曰」）与词组（「长安」）都收：古汉语里一字即一词。
            assert!(!p.word.is_empty(), "词条缺词");
            assert!(!p.pinyin.is_empty(), "{} 缺拼音", p.word);
            assert!(!p.pos.is_empty(), "{} 缺词性", p.word);
            assert!(!p.gloss.is_empty(), "{} 缺释义", p.word);
        }
        // 用户点名的两条测试数据必须在。
        assert!(PHRASES.iter().any(|p| p.word == "长安"));
        assert!(PHRASES.iter().any(|p| p.word == "曰"));
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
