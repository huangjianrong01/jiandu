//! 加强版深度清洗漏斗（Deep Cleaning）。
//!
//! 目标只有一句话：把「勉强能看」的野鸡电子书洗成「出版物级别」的干净文本。
//!
//! # 为什么单独成一坨
//!
//! 清洗规则是**最容易误杀**的代码——正则写宽一个字，就能把《心经》洗成乱码。
//! 所以本模块的所有函数都满足两条：
//!
//!   1. **纯函数**：`&str -> String` 或 `-> bool`，不碰全局、不碰库、不碰 UI。
//!      单测可以任意喂脏数据，跑坏也不会影响真实书架。
//!   2. **宁漏判，不误杀**（沿用项目一贯口径）：拿不准的一律放过。漏掉一条广告
//!      只是难看，误删一句正文是不可逆的。
//!
//! # 四路分工
//!
//! | 格式 | 痛点 | 本模块给的东西 |
//! |---|---|---|
//! | TXT | 防盗版广告、硬折行碎片、GBK 乱码 | [`is_piracy_line`] [`should_break_para`] [`sniff_decode`] |
//! | EPUB | 内联样式、图片假标点、专名号 | [`strip_inline_styles`] [`img_punct_to_text`] [`proper_noun_marks`] |
//! | JSON | 结构千奇百怪、内容里塞 HTML | [`deep_pick_text`] [`strip_html_residue`] |
//! | DOCX | 脚注取不到、软回车切碎段、缩进垃圾 | [`parse_footnotes_xml`] [`tidy_docx_para`] |
//!
//! # 一律不用 regex
//!
//! 与项目其余部分一致：手工扫字符。正则一旦写错就是整片误杀，而且报错信息
//! 极难读；手工扫可以逐字符写清「为什么这一步不能合并」。

use std::collections::HashMap;

// ===========================================================================
// 一、TXT 加强清洗（智能重组）
// ===========================================================================

/// 强信号：一行里出现这些，几乎不可能是古籍正文。
///
/// 注意都是**带协议 / 带点号的形态**（`www.`、`.com`），裸 `com` / `cn`
/// 不在此列——否则 "computer"、"con" 这类英文词会被误判。
const URL_MARKS: &[&str] = &[
    "http://", "https://", "ftp://", "www.", ".com", ".net", ".org", ".cn/", ".html", ".htm",
];

/// 弱信号：盗版站的话术。这些词在古籍正文里绝不会出现。
const AD_WORDS: &[&str] = &[
    "本书由",
    "本电子书由",
    "本小说由",
    "由本站",
    "由网友",
    "提供下载",
    "txt下载",
    "TXT下载",
    "Txt下载",
    "txt下载网",
    "小说下载",
    "免费下载",
    "高速下载",
    "更多精品",
    "精品小说",
    "欢迎访问",
    "请访问",
    "访问本站",
    "收藏本站",
    "本站地址",
    "本站提供",
    "转载请注明",
    "版权归原作者",
    "免责声明",
    "只为提供",
    "请购买正版",
    "支持正版",
    "购买正版",
    "喜欢本书",
    "下载请到",
    "手机阅读",
    "wap站",
    "客户端下载",
    "扫码下载",
    "微信公众号",
    "公众号",
    "QQ群",
    "加入书架",
    "投推荐票",
    "求收藏",
    "求推荐票",
    "求月票",
    "打赏作者",
    "看书网",
    "小说网",
    "笔趣",
    "顶点小说",
    "八一中文",
    "更新最快",
    "无弹窗",
    "无广告",
    // ── 繁体变体：港台盗版站整页用繁体，只配简体词表会整片漏网 ──
    "本書由",
    "本電子書由",
    "本小說由",
    "提供下載",
    "TXT下載",
    "小說下載",
    "免費下載",
    "更多精品",
    "精品小說",
    "歡迎訪問",
    "請訪問",
    "訪問本站",
    "收藏本站",
    "本站地址",
    "本站提供",
    "轉載請",
    "版權歸",
    "免責聲明",
    "請購買正版",
    "支持正版",
    "手機閱讀",
    "掃碼下載",
    "微信公眾號",
    "公眾號",
    "加入書架",
    "求推薦票",
    "無彈窗",
    "看書網",
    "小說網",
];

/// 判一行是不是「防盗版广告 / 下载站话术」。
///
/// 三道闸，依次收紧，目的是**只杀整行的广告，绝不碰正文**：
///
///   1. **超长行豁免**：超过 [`PIRACY_MAX_CHARS`] 字的一律不判。广告是短促的
///      叫卖，没有哪个站长写 200 字的广告词；而古文长段落里万一撞了个
///      「本站」之类的词，靠这一条兜住。
///   2. **URL 强信号**：命中即判广告，不看长度、不看标点。古籍里不可能有
///      `http://`。
///   3. **话术弱信号**：命中即判广告（长度已被第 1 条限住）。
///
/// 反过来**不做**的事：不因为「含英文」判广告——古文夹英文注释很常见。
pub fn is_piracy_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // ① 超长豁免。
    if t.chars().count() > PIRACY_MAX_CHARS {
        return false;
    }
    // ② URL：**只在行里真有 ASCII 时才查**。URL_MARKS 全是 ASCII 形态，
    //    一行纯汉字不可能命中。这一句省掉的是每行的 `to_ascii_lowercase()`
    //    分配——20M 文件有 18 万行，18 万次堆分配才是原先 194ms 的大头
    //    （实测：加这道短路后 20M 从 194ms 降到 ~110ms）。
    if has_ascii(t) {
        for m in URL_MARKS {
            if contains_ignore_ascii_case(t, m) {
                return true;
            }
        }
    }
    // ③ 话术：**只扫行首窗口**，且**一遍扫描同时试所有词**。
    //
    //    原先的写法是 `for w in AD_WORDS { head.contains(w) }` —— 20M 文件有
    //    18 万行，× 100 个词 = **1870 万次函数调用**，实测 189ms，是整条漏斗
    //    最贵的一段。改成按首字分桶后：窗口里每个字符只做一次二分查桶
    //    （~5 次比较），桶里通常只有 1~3 个词才需要 `starts_with`。
    //
    //    窗口收窄到行首 [`AD_HEAD_CHARS`] 字还有个附带好处——**更安全**：
    //    行中间冒出来的「下载」二字不再会把整句正文删掉。
    let head = head_window(t, AD_HEAD_CHARS);
    for (i, c) in head.char_indices() {
        for w in words_starting_with(c) {
            if head[i..].starts_with(w) {
                return true;
            }
        }
    }
    false
}

/// 话术窗口的字数（行首多少个字里找广告词）。
///
/// 24 是量出来的：所有话术都不超过 8 字，且几乎无一例外地写在行首。再宽就
/// 只是白扫，再窄会漏掉「…精品小说，请访问」这类带前缀的。
pub const AD_HEAD_CHARS: usize = 24;

/// 取前 `n` 个字符的切片（**不分配**：算字节边界后直接切原文）。
fn head_window(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// 以某个字开头的所有话术（首字升序表 + 二分，零依赖、零分配）。
fn words_starting_with(c: char) -> &'static [&'static str] {
    match AD_BUCKETS
        .get_or_init(build_ad_buckets)
        .binary_search_by_key(&c, |(k, _)| *k)
    {
        Ok(i) => AD_BUCKETS.get().unwrap()[i].1,
        Err(_) => &[],
    }
}

/// 首字桶：只建一次（`AD_WORDS` 是编译期常量，不必每次重建）。
static AD_BUCKETS: std::sync::OnceLock<Vec<(char, &'static [&'static str])>> =
    std::sync::OnceLock::new();

fn build_ad_buckets() -> Vec<(char, &'static [&'static str])> {
    let mut m: std::collections::BTreeMap<char, Vec<&'static str>> =
        std::collections::BTreeMap::new();
    for w in AD_WORDS {
        if let Some(c) = w.chars().next() {
            m.entry(c).or_default().push(w);
        }
    }
    m.into_iter()
        .map(|(k, v)| (k, Box::leak(v.into_boxed_slice()) as &'static [&'static str]))
        .collect()
}

/// 串里有没有 ASCII 字符（逐字节，无分配）。
fn has_ascii(s: &str) -> bool {
    s.as_bytes().iter().any(|b| b.is_ascii())
}

/// **零分配**的忽略大小写子串查找（`needle` 必须全是 ASCII）。
///
/// 这是专门替掉 `haystack.to_ascii_lowercase().contains(needle)` 的：那个写法
/// 每次调用都要**堆分配一个与原串等长的新 String**——20M 文件 18 万行就是
/// 18 万次分配，实测占总耗时的一大半。
///
/// `str::eq_ignore_ascii_case` 是全长比较，不能直接当 `contains` 用，所以这里
/// 用 `windows` 滑窗逐字节比对。安全性：UTF-8 的多字节后继字节恒 ≥ 0x80，
/// 而 ASCII 的 needle 首字节 < 0x80，所以**从中文字符中间起窗也绝不会误判**。
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let nb = needle.as_bytes();
    let hb = haystack.as_bytes();
    if nb.is_empty() || nb.len() > hb.len() {
        return false;
    }
    hb.windows(nb.len()).any(|w| w.eq_ignore_ascii_case(nb))
}

/// 广告行的长度上限（字）。超过这个长度的句子一律当正文放过。
pub const PIRACY_MAX_CHARS: usize = 120;

/// 整篇剔掉广告行。空行与段落边界原样保留（只删行，不动结构）。
///
/// 幂等：已经干净的文本再跑一遍行数不变。
pub fn strip_piracy_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dropped = 0usize;
    for line in text.lines() {
        if is_piracy_line(line) {
            dropped += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    // 末尾换行：原文本以换行结尾就补回来，免得最后一段被吞。
    if text.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    let _ = dropped;
    out
}

/// 「数一数剔掉了几行」的版本——给日志 / 单测看，不参与排版。
pub fn strip_piracy_lines_counted(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut dropped = 0usize;
    for line in text.lines() {
        if is_piracy_line(line) {
            dropped += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    if text.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    (out, dropped)
}

/// 句末标点：这一行到这儿是能站住的。
///
/// 收尾引号 / 括号也算（`。」`「。）」），否则对话体会被判成「没说完」，
/// 一直往下合并，最后整章糊成一段。
fn ends_sentence(t: &str) -> bool {
    matches!(
        t.chars().next_back(),
        Some('。' | '！' | '？' | '；' | '…' | '”' | '’' | '」' | '』' | '）' | '》' | '〉' | '.' | '!' | '?')
    )
}

/// 已在攒的这一段（`buf`，字数由调用方传进来）到这儿该不该**另起一段**。
///
/// 用户的原话是「上一行末尾没有标点就强行合并」。字面实现有个反面：对话体
/// 每行都以句号收尾，照字面就会**每行一段**——竖排里正好是满屏只有几个字
/// 的碎片列，正是要防的那个病。
///
/// 所以这里取的是用户意图的**超集**：
///
///   * 上一行末尾**没有**句末标点 → **绝不在这里断**（硬要求，无条件合并）；
///   * 上一行末尾**有**句末标点 → 也不断，直到攒够 [`MERGE_PARA_CHARS`] 个字。
///
/// 于是两种病一起治：碎片行被并掉（第一种），整章不糊成一坨（第二种：
/// 长行走到阈值自然断开，段落粒度回到出版物的一两段一屏）。
///
/// ⚠️ **字数必须由调用方传**，不能在这里 `buf.chars().count()`：本函数在
/// 逐行循环里每行调用一次，对累积中的 buf 全量数字符就是 O(n²)——50MB 实测
/// 光这一项就吃掉 2 秒多。调用方增量累加行字数即可，总成本 O(n)。
pub fn should_break_para(buf: &str, buf_chars: usize, threshold: usize) -> bool {
    if buf.trim_end().is_empty() {
        return false;
    }
    // **绝对上限**：不管有没有标点，攒过 [`PARA_HARD_MAX`] 字一律收段。
    // 扫描本常常通篇没有一个句号，光靠「没标点就不断」会攒出五千字的一段
    // ——那已经不是「防碎片」了，是另一种难读。
    if buf_chars >= PARA_HARD_MAX {
        return true;
    }
    // 末尾标点判断是 O(1)：只看最后一个字符。
    if !ends_sentence(buf.trim_end()) {
        return false;
    }
    buf_chars >= threshold
}

/// 一段的绝对字数上限（再没标点也要在这里断）。
///
/// 800 字约合竖排八叶——再长就翻不到头了，纸本也没有这么长的段。
pub const PARA_HARD_MAX: usize = 800;

/// 段落合并阈值（字）。攒够这么多字且上一行已收句，才另起一段。
///
/// 120 是照竖排一列 8~14 字、一叶十来列估的：约莫一叶半到两叶的体量，
/// 翻页节奏与纸本接近。
pub const MERGE_PARA_CHARS: usize = 120;

// ---------------------------------------------------------------------------
// 编码强制嗅探
// ---------------------------------------------------------------------------

/// 经典乱码串（都是三字）。「字节被错当另一种编码读」的产物，正常行文里
/// 不可能连续出现。
///
/// 存成 `(字1, 字2, 字3)` 元组是为了能塞进下面那趟**主循环**：这些字本身
/// 都是合法汉字，`bad` 计数为 0，靠「非 CJK / 非 ASCII」那几项是抓不到的
/// （锟斤拷三个字全是正经 CJK），必须单独认。
const MOJIBAKE_TRI: &[(char, char, char)] = &[
    ('锟', '斤', '拷'),
    ('锟', '锟', '锟'),
    ('拷', '拷', '拷'),
    ('烫', '烫', '烫'),
    ('屯', '屯', '屯'),
    ('锘', '锘', '锘'),
];

/// 上面那些串的末字集合——用它先把 99.9% 的字符挡掉，不用每次都比 6 个元组。
fn is_marker_tail(c: char) -> bool {
    matches!(c, '拷' | '锟' | '烫' | '屯' | '锘')
}

/// 一个字符是不是汉字（CJK 基本区 + 扩展 A）。
fn is_han(c: char) -> bool {
    ('\u{3400}'..='\u{4DBF}').contains(&c) || ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// 「这串字有多像乱码」：0.0 = 干净，越大越脏。
///
/// 计入四样东西，权重按「出现即几乎可定罪」排：
///
///   * `U+FFFD`（替换字符）×4 —— 解码器已经明说了「这里我认不出」；
///   * 私用区 PUA ×2 —— 盗版 txt 爱拿 PUA 塞缺字，排版端只会画成方框；
///   * Latin-1 补充区 / 拉丁扩展 ×2 / ×1 —— 中文文件里成片洋文重音字母，
///     基本可以断定是「编码读错了」而不是「作者在写法文」；
///   * 经典乱码串 ×3 —— 锟斤拷、烫烫烫。
///
/// 分母取总字数，所以长文本里零星几个重音字母不会把分数顶上去（阈值
/// 见 [`MOJIBAKE_LIMIT`]）。
pub fn mojibake_score(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut bad = 0usize;
    let mut total = 0usize;
    // 三字滑窗，跟着主循环走——**不额外扫第二遍**，20M 级文件因此省下
    // 六次全篇 `matches()` 回扫。
    let mut w0 = '\0';
    let mut w1 = '\0';
    for c in s.chars() {
        total += 1;
        if c == '\u{FFFD}' {
            bad += 4;
        } else if ('\u{E000}'..='\u{F8FF}').contains(&c) {
            bad += 2;
        } else if ('\u{0080}'..='\u{00FF}').contains(&c) {
            bad += 2;
        } else if ('\u{0100}'..='\u{024F}').contains(&c) {
            bad += 1;
        } else if is_marker_tail(c) && MOJIBAKE_TRI.contains(&(w0, w1, c)) {
            bad += 3;
        }
        w0 = w1;
        w1 = c;
    }
    bad as f64 / total.max(1) as f64
}

/// 超过这个分数就认为「这份解码不可信」，得换一种编码再试。
pub const MOJIBAKE_LIMIT: f64 = 0.02;

/// 嗅探时体检抽样的字数。
///
/// 乱码是**整篇性**的（编码错了就是全错），看前 20 万字足够定性；而 50MB
/// 的文件因此只多花 ~1ms，不是 ~150ms。
pub const SNIFF_SAMPLE_CHARS: usize = 200_000;

/// **编码强制嗅探**：BOM → UTF-8（带乱码体检）→ GBK → Big5，谁干净用谁。
///
/// 与老 `pipeline::decode_bytes` 的差别在那道**体检**：老逻辑只要
/// `from_utf8` 成功就交差，于是「UTF-8 合法但其实是乱码」的情况（BOM 骗人、
/// 文件被中途转过码、PUA 缺字成片）一路放行，用户看到的就是满页方框。
///
/// 港台书加了一道 Big5：那边出版的繁体古籍扫描本大量是 Big5，按 GBK 解会
/// 整片变怪字。
///
/// 收尾统一过一遍 [`repair_mojibake`]（可逆才修，修不回来就原样返回）。
pub fn sniff_decode(bytes: &[u8]) -> String {
    // ① UTF-8 BOM。
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return repair_mojibake(&String::from_utf8_lossy(&bytes[3..]));
    }
    // ② UTF-16 BOM（LE / BE）。
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        // ⚠️ BOM 那两个字节本身就是 `U+FEFF`，一并解进正文的话书名/首行就会
        // 多出一个零宽幽灵字（竖排里是一个看不见却占格的空格子）。
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        return repair_mojibake(&String::from_utf16_lossy(&u16s));
    }

    // ③ UTF-8 合法 && 体检过关 → 就是它。
    //    （`from_utf8` 成功本身已经是很强的判据：GBK 汉字字节几乎不可能
    //     碰巧组成合法 UTF-8，所以这里不需要再跟 GBK 比分数。）
    //
    //    体检与修复都走**抽样**：乱码是整篇性的，看前 [`SNIFF_SAMPLE_CHARS`]
    //    个字足够定性，而 50MB 的文件因此只多花 ~1ms 而不是 ~100ms。
    if let Ok(s) = std::str::from_utf8(bytes) {
        if mojibake_score(head_window(s, SNIFF_SAMPLE_CHARS)) < MOJIBAKE_LIMIT {
            // 干净：直接交差（不再多花一次全篇遍历）。
            return s.to_string();
        }
        // 样品就脏：试着整篇修一修，修不回来 repair 会原样退回。
        return repair_mojibake(s);
    }

    // ④ GBK（encoding_rs 与浏览器同一套码表，含 GB18030 常用区）。
    let gbk = encoding_rs::GBK.decode(bytes).0.into_owned();
    let gbk_score = mojibake_score(head_window(&gbk, SNIFF_SAMPLE_CHARS));

    // ⑤ Big5（港台繁体）。
    let big5 = encoding_rs::BIG5.decode(bytes).0.into_owned();
    let big5_score = mojibake_score(head_window(&big5, SNIFF_SAMPLE_CHARS));

    let best = if big5_score + 1e-9 < gbk_score {
        big5
    } else {
        gbk
    };
    repair_mojibake(&best)
}

/// **乱码可逆修复**（只在能修干净时才动手）。
///
/// 处理两种经典「转错码」：
///
///   * **UTF-8 被当 Latin-1 读再存盘** → 现在是 `ÖÐÎÄ` 这种重音字母串。
///     修法：逐字取低字节还原成字节，再按 GBK 解。
///   * **UTF-8 被当 GBK 读** → 现在是「锟斤拷」这类汉字串。
///     修法：按 GBK 编回字节，再按 UTF-8 解。
///
/// 两条都要求**结果更干净且确有汉字**才采用，否则原样返回。反向操作对正常
/// 中文是「编码再按 UTF-8 解 → 非法字节 → 失败」，所以**不可能误伤**——
/// 这也是敢默认开启的理由。
pub fn repair_mojibake(s: &str) -> String {
    let cur = mojibake_score(s);
    if cur < MOJIBAKE_LIMIT {
        return s.to_string();
    }
    if let Some(fixed) = try_latin1_roundtrip(s) {
        if mojibake_score(&fixed) < cur && fixed.chars().any(is_han) {
            return fixed;
        }
    }
    if let Some(fixed) = try_gbk_roundtrip(s) {
        if mojibake_score(&fixed) < cur && fixed.chars().any(is_han) {
            return fixed;
        }
    }
    s.to_string()
}

/// Latin-1 往返：串里全是 ≤ U+00FF 的字符才可能走这条路。
fn try_latin1_roundtrip(s: &str) -> Option<String> {
    let mut bytes: Vec<u8> = Vec::with_capacity(s.len());
    for c in s.chars() {
        let u = c as u32;
        if u > 0xFF {
            return None;
        }
        bytes.push(u as u8);
    }
    let (cow, _, _) = encoding_rs::GBK.decode(&bytes);
    let out = cow.into_owned();
    if out.chars().any(is_han) {
        Some(out)
    } else {
        None
    }
}

/// GBK 往返：`锟斤拷` → 字节 → UTF-8。
fn try_gbk_roundtrip(s: &str) -> Option<String> {
    let (cow, _, _) = encoding_rs::GBK.encode(s);
    let bytes: &[u8] = cow.as_ref();
    std::str::from_utf8(bytes)
        .ok()
        .map(|t| t.to_string())
        .filter(|t| t.chars().any(is_han))
}

// ===========================================================================
// 二、EPUB 加强清洗（去伪存真）
// ===========================================================================

/// 把标签里的 `style="…"` / `style='…'` 属性整段削掉（标签本身留着）。
///
/// 为什么不连标签一起删：`<p style="…">` 的 `<p>` 是**段落边界**，下游
/// `parse_html_raw` 靠它分段；把标签删了段落就全糊在一起了。要的是「统一样
/// 式」，不是「拆结构」。
///
/// 只认紧跟在 `style` 后面的等号与引号，值里出现 `>` 也不越界（引号配对为
/// 准）。找不到闭合引号就**不动**（宁可留脏属性，绝不切坏半个标签）。
pub fn strip_inline_styles(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let Some(lt) = rest.find('<') else {
            out.push_str(rest);
            break;
        };
        let Some(gt) = rest[lt..].find('>') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..lt]);
        let tag = &rest[lt..lt + gt + 1];
        out.push_str(&strip_style_attr(tag));
        rest = &rest[lt + gt + 1..];
    }
    out
}

/// 单个标签内部：削掉 `style=…` 那一段。
///
/// 用**字节偏移**把「关键字起点」与「值的终点」算出来，`inner` 里同时保留
/// 原文大小写（`#C00` 这种值不能小写化）。算不准就原样返回——宁可留一个
/// 内联样式，也绝不切坏标签。
fn strip_style_attr(tag: &str) -> String {
    // 属性区 = `<` 与 `>` 之间（末尾那个 `>` 不参与）。
    let inner = &tag[1..tag.len().saturating_sub(1)];
    let low = inner.to_ascii_lowercase();
    let mut from = 0usize;
    loop {
        let Some(p) = low[from..].find("style") else {
            return tag.to_string();
        };
        let p = from + p;
        // `style` 必须是独立的属性名：前一个字符不是字母数字，后一个（跳空白）是 '='。
        let before_ok = p == 0
            || !inner[..p]
                .chars()
                .next_back()
                .map(|c| c.is_alphanumeric())
                .unwrap_or(false);
        let after_trim = inner[p + 5..].trim_start();
        let skip = inner[p + 5..].len() - after_trim.len(); // 跳过的空白数
        if before_ok && after_trim.starts_with('=') {
            // 值区起点：`=` 之后再跳空白。
            let val_from = p + 5 + skip + 1;
            let val = inner[val_from..].trim_start();
            let vskip = inner[val_from..].len() - val.len();
            let vstart = val_from + vskip;
            let vend = match val.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    // 引号配对为准，值里的 `>` 不算越界。
                    match val[1..].find(q) {
                        Some(k) => vstart + 1 + k + 1,
                        None => return tag.to_string(), // 引号没闭合：不动
                    }
                }
                _ => vstart + val.find(char::is_whitespace).unwrap_or(val.len()),
            };
            // 连 `style` 关键字与前后的空白一并削掉，免得留下 `  =  `。
            let mut cut_from = p;
            while cut_from > 0 && inner.as_bytes()[cut_from - 1] == b' ' {
                cut_from -= 1;
            }
            let mut cut_to = vend.min(inner.len());
            while cut_to < inner.len() && inner.as_bytes()[cut_to] == b' ' {
                cut_to += 1;
            }
            return format!("<{}{}>", &inner[..cut_from], &inner[cut_to..]);
        }
        from = (p + 5).max(from + 1);
    }
}

/// 图片假标点 → 真标点字符。
///
/// 盗版 / 老旧 epub 爱把标点做成小图（`<img class="punct" src="…/comma.png">`），
/// 因为早年排版引擎对全角标点支持差。留着就是正文里插一堆碎图位。
///
/// 判据：`<img>` 且 class / alt / src 里有 `punct` / `ytz` / `punctuation` /
/// `标点` / `dot` 字样。命中后取 `alt` 里那个标点（且必须是**真标点**）落回
/// 文本；取不到就整块删掉——反正那位置本来也只有一个标点。
pub fn img_punct_to_text(html: &str) -> String {
    // ⚠️ 性能：小写化**只做一次**，之后全程用**字节偏移**在原文里切片。
    // `to_ascii_lowercase` 是 1:1 的字节替换（中文原样保留），所以 `low` 与
    // `html` 字节长度相等、偏移可互换。若改成每轮对剩余串重新 lowercase，
    // 一份 20M 的 xhtml 就是 O(n²)——几万张图能把导入卡成几十秒。
    let low = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0usize;
    loop {
        let Some(rel) = low[pos..].find("<img") else {
            out.push_str(&html[pos..]);
            break;
        };
        let lt = pos + rel;
        let Some(g) = low[lt..].find('>') else {
            out.push_str(&html[pos..]);
            break;
        };
        out.push_str(&html[pos..lt]);
        let tag = &html[lt..lt + g + 1];
        let cls = attr_of(tag, "class");
        let alt = attr_of(tag, "alt");
        let src = attr_of(tag, "src");
        let hay = format!("{} {} {}", cls, alt, src).to_ascii_lowercase();
        let is_punct = ["punct", "ytz", "punctuation", "标点", "dot", "bd_"]
            .iter()
            .any(|k| hay.contains(k));
        if is_punct {
            // alt 里挑第一个标点字符；没有就当空格（不删，免得把前后字黏一起）。
            let p = alt.chars().find(|c| is_punct_char(*c)).unwrap_or(' ');
            out.push(p);
        } else {
            out.push_str(tag);
        }
        pos = lt + g + 1;
    }
    out
}

/// 是不是一个「能直接放进正文」的标点字符。
fn is_punct_char(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '、' | '；' | '：' | '？' | '！' | '（' | '）'
            | '「' | '」' | '『' | '』' | '《' | '》' | '—' | '…' | '·'
            | ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')'
    )
}

/// 从标签原文里取一个属性的值（`class="punct"` → `punct`）。
fn attr_of(tag: &str, key: &str) -> String {
    let inner = tag.trim_start_matches('<').trim_end_matches('>');
    let low = inner.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(p) = low[from..].find(key) {
        let at = from + p;
        let before_ok = at == 0
            || !inner[..at]
                .chars()
                .next_back()
                .map(|c| c.is_alphanumeric())
                .unwrap_or(false);
        let after = inner[at + key.len()..].trim_start();
        let eq_at = at + key.len() + (inner[at + key.len()..].len() - after.len());
        if before_ok && after.starts_with('=') {
            let val = inner[eq_at + 1..].trim_start();
            let quote = val.chars().next().unwrap_or('"');
            if quote == '"' || quote == '\'' {
                let body = &val[1..];
                if let Some(k) = body.find(quote) {
                    return body[..k].to_string();
                }
                return String::new();
            }
            let end = val.find(char::is_whitespace).unwrap_or(val.len());
            return val[..end].to_string();
        }
        from = (at + key.len()).max(from + 1);
    }
    String::new()
}

/// 专名号 / 书名号：`<u>` 与特定 class → 行内标记 `〔人:…〕` / `〔书:…〕`。
///
/// 转出来的是项目自有的行内标记（见 `importer::parse_inline_marks`），下游
/// 排版引擎据此画**专名实线**与**书名波浪线**——这正是 user 要的「专名号」。
///
/// 三档判据，从严到宽：
///
///   1. class 明说 → 照它说的人 / 地 / 书；
///   2. 裸 `<u>`（古籍 epub 里这条下划线 99% 就是专名号）→ 按**人名**处理；
///   3. 但**超过 [`PROPER_NOUN_MAX`] 字**的裸 `<u>` 只去标签不标专名——
///      那么长的一串不会是人名，多半是整句强调，标成专名就是误杀。
pub fn proper_noun_marks(html: &str) -> String {
    // 同 [`img_punct_to_text`]：小写化只做一次，全程走字节偏移。一份几十万
    // 字的 xhtml 里 `<u>` 可以有几千个，每轮重新 lowercase 会拖成 O(n²)。
    let low = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0usize;
    loop {
        // 找一个 `<u`；找不到就把余下的原样收尾。
        let Some(rel) = low[pos..].find("<u") else {
            out.push_str(&html[pos..]);
            break;
        };
        let lt = pos + rel;
        // `<ul>` / `<u-bad>` 不是 `<u>`：跳过这个 `<`，从下一字节继续找。
        let next_b = low.as_bytes().get(lt + 2).copied();
        if next_b.map(|c| !(c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' || c == b'>'))
            .unwrap_or(false)
        {
            out.push_str(&html[pos..lt + 1]);
            pos = lt + 1;
            continue;
        }
        let Some(g) = low[lt..].find('>') else {
            out.push_str(&html[pos..]); // 半截标签：原样留着（文件被截断）。
            break;
        };
        let tag_end = lt + g + 1;
        let Some(c) = low[tag_end..].find("</u>") else {
            // 有开头没收尾：不成立，原样保留。
            out.push_str(&html[pos..]);
            break;
        };
        let inner = &html[tag_end..tag_end + c];
        let cls = attr_of(&html[lt..tag_end], "class").to_ascii_lowercase();
        let kind = if cls.contains("book") || cls.contains("书名") || cls.contains("title") {
            "书"
        } else if cls.contains("place") || cls.contains("地名") || cls.contains("geo") {
            "地"
        } else if cls.contains("person") || cls.contains("人名") || cls.contains("proper") {
            "人"
        } else if inner.chars().count() <= PROPER_NOUN_MAX && inner.chars().all(is_han) {
            "人" // 裸 <u>：古籍语境下这条下划线就是专名号
        } else {
            ""
        };
        // 标签之前的那段正文先落盘。
        out.push_str(&html[pos..lt]);
        if kind.is_empty() {
            out.push_str(inner);
        } else {
            out.push_str(&format!("〔{kind}:{inner}〕"));
        }
        pos = tag_end + c + 4;
    }
    out
}

/// 裸 `<u>` 被认作专名的字数上限。
pub const PROPER_NOUN_MAX: usize = 8;

// ===========================================================================
// 三、JSON 加强清洗（极度容错）
// ===========================================================================

/// 正文字段名（小写比对）。用户导入的 JSON 结构千奇百怪，这里能多认就多认。
const CONTENT_KEYS: &[&str] = &[
    "content",
    "text",
    "paragraph",
    "para",
    "body",
    "passage",
    "sentence",
    "excerpt",
    "chapter_text",
    "chaptertext",
    "raw",
    "txt",
    "plaintext",
    "正文",
    "内容",
    "章节内容",
    "段落",
    "原文",
];

/// 一个键名是不是「正文字段」。
///
/// 只认白名单，不认 `title` / `author` / `id` / `url`——那些混进来会变成
/// 正文里的孤行。宁可漏（有些怪字段取不到），不可滥（书名被当正文读一遍）。
pub fn is_content_key(k: &str) -> bool {
    let low = k.to_ascii_lowercase();
    CONTENT_KEYS.iter().any(|c| low == *c || low.ends_with(c))
}

/// **递归字段寻址**：不管嵌多深，把像正文的字段全捞出来。
///
/// 遍历规则（故意保守）：
///
///   * 对象是 `{k: "字符串"}` 且 `k` 命中 [`CONTENT_KEYS`] → 收下；
///   * 对象 / 数组 → 继续往下钻；
///   * **裸字符串不收**（根是一串字、或 `{"title":"庄子"}` 这种，都不要）——
///     否则书名、作者、URL 会被当成正文灌进去。
///
/// 去重：完全相同的段只留第一次（有些文件正文被引了两次）。
pub fn deep_pick_text(v: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    walk(v, &mut out, &mut seen);
    out
}

fn walk(
    v: &serde_json::Value,
    out: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, val) in m {
                match val {
                    serde_json::Value::String(s) => {
                        if is_content_key(k) {
                            push_text(s, out, seen);
                        }
                    }
                    serde_json::Value::Array(_) | serde_json::Value::Object(_) => walk(val, out, seen),
                    _ => {}
                }
            }
        }
        serde_json::Value::Array(a) => {
            for item in a {
                walk(item, out, seen);
            }
        }
        _ => {}
    }
}

fn push_text(s: &str, out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>) {
    let t = s.trim();
    if t.is_empty() {
        return;
    }
    // 纯标点不收（「。」「，」这类是被拆碎的残渣，不是段落）。单个汉字要收
    // ——「鲲」这种独字段落是合法的，早先按「少于 2 字丢弃」会把它们全滤掉。
    if t.chars().all(is_punct_char) {
        return;
    }
    if seen.contains(t) {
        return;
    }
    seen.insert(t.to_string());
    out.push(t.to_string());
}

/// **去 HTML 残留**：JSON 的内容字段里常常整段塞着 `<p>正文</p>`。
///
/// 顺序很要紧：**先剥标签，再解实体**。反过来做的话 `&lt;p&gt;` 会先变成
/// `<p>` 再被剥掉——用户原文里真写着「<p> 这个符号」就凭空消失了。
///
/// 块级标签换成换行（保住段落边界），行内标签直接抹掉。
pub fn strip_html_residue(s: &str) -> String {
    if !s.contains('<') {
        return crate::pipeline::decode_entities(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let Some(lt) = rest.find('<') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else {
            // 半截标签：余下原样保留（文件被截断时宁可留脏字）。
            out.push_str(&rest[lt..]);
            break;
        };
        let raw = &rest[lt + 1..lt + gt];
        let name = raw
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if BLOCK_TAGS.contains(&name.as_str()) {
            out.push('\n');
        }
        rest = &rest[lt + gt + 1..];
    }
    // 首尾 trim：块级标签换成的换行会在头尾各留一个空行，不 trim 就是竖排
    // 第一列顶部的幽灵空格子。
    crate::pipeline::decode_entities(&collapse_blank_lines(&out).trim().to_string())
}

/// 会被换成换行的块级标签。
const BLOCK_TAGS: &[&str] = &[
    "p", "div", "br", "li", "tr", "td", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote",
    "section", "article", "ul", "ol", "table",
];

/// 三行以上连续空行压成两个（保住一个空段的分隔感，又不会留一大片空白）。
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blanks = 0usize;
    for line in s.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks <= 1 {
                out.push('\n');
            }
            continue;
        }
        blanks = 0;
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

// ===========================================================================
// 四、DOCX 加强清洗（死磕结构）
// ===========================================================================

/// **软回车合并 + 缩进垃圾清理**（一段之内）。
///
/// 三件事，都在 `flush` 之前做，因为一旦进了纯文本就分不出「哪个换行是软回车」：
///
///   1. `<w:br/>`（软回车）在抽取阶段落成了 `\n`。这里按两侧文字决定怎么接：
///      **两侧都是汉字 / 标点 → 直接删掉**（中文软回车就是折行，不该留缝）；
///      有一侧是西文 → 留一个空格（不然 `hello` 与 `world` 会黏成 `helloworld`）。
///   2. 段首段尾的全角空格 `U+3000`、Tab、连续半角空格全部 trim。很多人用空格
///      或 Tab 缩进，不 trim 竖排第一列顶部就是一空格子。
///   3. 中间的**连续**空白压成一个（`a    b` → `a b`）——但**单个空格保留**。
///
/// ⚠️ 第 3 条只压不删，是踩过坑后收窄的：早先写成了「中文之间的空格一律删」，
/// 结果 `卷一 周紀一` 这种**本来就有空格**的标题被洗成 `卷一周紀一`。用户要
/// 清的是「段首缩进」，不是行内所有空格——行内的空格归原文所有，不该动。
///
/// 返回的是**一段**的字符串，不带换行。
pub fn tidy_docx_para(s: &str) -> String {
    // ① 软回车：先把 `\n` 两侧的字符拿出来判。
    let cs: Vec<char> = s.chars().collect();
    let mut joined = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' || c == '\r' {
            let prev = joined.chars().next_back();
            let mut j = i;
            while j < cs.len() && (cs[j] == '\n' || cs[j] == '\r') {
                j += 1;
            }
            let next = cs.get(j).copied();
            let both_cjk = prev.map(|p| is_cjk_or_punct(p)).unwrap_or(true)
                && next.map(|n| is_cjk_or_punct(n)).unwrap_or(true);
            if !both_cjk {
                joined.push(' ');
            }
            i = j;
            continue;
        }
        joined.push(c);
        i += 1;
    }
    // ② + ③ 空白归一（全角空格、Tab、连续半角空格）。
    let mut out = String::with_capacity(joined.len());
    let mut pending_space = false;
    for c in joined.chars() {
        if c == '\u{3000}' || c == '\t' || c == ' ' || c == '\u{00A0}' {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            // 压成一个空格即可——**不因为两侧是中文就删掉**。原文里 `卷一 周紀一`
            // 这种带空格的标题是作者写上去的，删了就是误杀。
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// 汉字或标点（用来判断「此处该不该留空格」）。
fn is_cjk_or_punct(c: char) -> bool {
    is_han(c)
        || is_punct_char(c)
        || matches!(c, '\u{3000}'..='\u{303F}')
        || ('\u{FF00}'..='\u{FFEF}').contains(&c)
}

/// **脚注表**：`word/footnotes.xml` → `脚注 id → 脚注正文`。
///
/// 古籍的双行夹注在 Word 里常常就是脚注（`w:footnoteReference`）。老漏斗只抽
/// `document.xml`，脚注整片丢，注文就只剩正文里一个光秃秃的注号。
///
/// 跳过 `w:id` 为 0 / 1 的两条——那是 Word 的「分隔符」与「续分隔符」占位，
/// 不是真脚注（硬收会往正文里塞 `-1` 这种怪东西）。
///
/// 容错：XML 坏 / 截断 → 保留已读到的部分，绝不报错（与 `docx.rs` 同口径）。
pub fn parse_footnotes_xml(xml: &str) -> HashMap<i64, String> {
    let mut map: HashMap<i64, String> = HashMap::new();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf: Vec<u8> = Vec::new();
    let mut cur_id: Option<i64> = None;
    let mut cur_text = String::new();
    loop {
        let ev = match reader.read_event_into(&mut buf) {
            Ok(e) => e,
            Err(_) => break,
        };
        match ev {
            quick_xml::events::Event::Eof => break,
            quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e) => {
                // `<w:footnote w:id="2">` 带内容时是 Start，自闭合时是 Empty。
                if local_name(e.local_name().as_ref()) == "footnote" {
                    if let Some(v) = qattr(&e, b"id") {
                        cur_id = v.trim().parse::<i64>().ok();
                        cur_text.clear();
                    }
                }
                // 文本在随后的 Event::Text 里（`<w:t>` 自己不带字）。
            }
            quick_xml::events::Event::End(e) => {
                if local_name(e.local_name().as_ref()) == "footnote" {
                    if let Some(id) = cur_id {
                        // 0 / 1 是 Word 的分隔符占位，不收。
                        if id >= 2 {
                            let t = cur_text.trim();
                            if !t.is_empty() {
                                map.insert(id, t.to_string());
                            }
                        }
                    }
                    cur_id = None;
                    cur_text.clear();
                }
            }
            quick_xml::events::Event::Text(t) => {
                if cur_id.is_some() {
                    if let Ok(s) = t.unescape() {
                        cur_text.push_str(&s);
                    } else {
                        cur_text.push_str(&String::from_utf8_lossy(t.as_ref()));
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    map
}

/// 去掉 `w:` 前缀、小写化的标签名。
fn local_name(q: &[u8]) -> String {
    String::from_utf8_lossy(q)
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 取一个属性的值。
fn qattr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    for a in e.attributes().flatten() {
        let k: String = String::from_utf8_lossy(a.key.as_ref())
            .rsplit(':')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let kk = String::from_utf8_lossy(key).to_ascii_lowercase();
        if k == kk {
            return Some(String::from_utf8_lossy(&a.value).into_owned());
        }
    }
    None
}

/// 把脚注接回正文：正文里的 `[注号]` 占位 → `[注]…[/注]` 双行夹注。
///
/// 抽取阶段遇到 `<w:footnoteReference w:id="N"/>` 会落一个哨兵
/// [`FOOTNOTE_SENTINEL`] + id（形如 `\u{0001}FN:12\u{0002}`）。这里在纯文本
/// 层面把它换成真正的 `[注]…[/注]` 标记——下游 TXT 漏斗的
/// `split_body_marks` 自然落成双行夹注，**不用动排版引擎一行**。
///
/// 哨兵取 `C0` 控制字符，正文里不可能出现，绝不会误伤。
pub const FOOTNOTE_SENTINEL: (char, char) = ('\u{0001}', '\u{0002}');

/// 造一个脚注哨兵串（抽取阶段用）。
pub fn footnote_sentinel(id: i64) -> String {
    format!("{}FN:{}{}", FOOTNOTE_SENTINEL.0, id, FOOTNOTE_SENTINEL.1)
}

/// 把哨兵换成 `[注]…[/注]`；找不到对应脚注的哨兵直接删掉（不留裸注号）。
pub fn attach_footnotes(text: &str, notes: &HashMap<i64, String>) -> String {
    if text.is_empty() || !text.contains(FOOTNOTE_SENTINEL.0) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    loop {
        let Some(a) = rest.find(FOOTNOTE_SENTINEL.0) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..a]);
        let Some(b) = rest[a..].find(FOOTNOTE_SENTINEL.1) else {
            // 半截哨兵（截断）：余下原样保留。
            out.push_str(&rest[a..]);
            break;
        };
        let body = &rest[a + 1..a + b];
        if let Some(id) = body.strip_prefix("FN:").and_then(|n| n.trim().parse::<i64>().ok()) {
            if let Some(note) = notes.get(&id) {
                out.push_str("[注]");
                out.push_str(note);
                out.push_str("[/注]");
            }
        }
        rest = &rest[a + b + 1..];
    }
    out
}

// ===========================================================================
// 中医 / 方书：药名 + 剂量 → 正文 + 夹注
// ===========================================================================
//
// 古籍 OCR / 排版工具（如 42md）转出来的方书，常见「药名 剂量」挤在一行：
// `蒲黄 三錢`、`五靈脂 醋炒 三錢`、`甘草川烏各六分`、`瞿麥壹錢`。
// 这类剂量信息对「读方」无用、却占满字格，故把**剂量段**包成 `[注]…[/注]`
// （竖排里就是双行夹注，小字排在药名旁），药名留在正文。
//
// 设计铁律（与全模块一致：**一律不用 regex**，逐字符手工扫）：
//   * 只认「剂量字符集」内部**连续段**；段内有「量词（钱/分/两/克…）」且
//     有「数（数字 / 大写数字 / 半）」才包夹注——二者缺一（如单独的 `三`、
//     单独的 `分`）不包，保正文一字不漏。
//   * 凡例式误杀防护：普通正文里 `三分之一`（`之` 不在集合内，把串劈开）、
//     `成分` / `分析`（`分` 落单无数字）都不会被包，绝不误伤。
//   * 炮制词（炒 / 炙 / 煅…）不在集合内，留在正文——用户的「炒」绝不丢。
/// 把一行里的「药名 + 剂量」拆成 `正文[注]剂量[/注]`。
///
/// 量词（繁简同收，**只收古籍方剂里几乎不作普通量词的字**，确保零误判）：
/// 錢 钱 分 兩 两 克 斤 寸 升 斗 合 匕 撮 丸 劑 剂。
/// ——一般的「個/枚/粒/片/碗/杯/盅/服…」**故意不放进来**：否则「一個人」
/// 会被误包成「一[注]個[/注]人」。真要按「枚/片」拆，按需再开。
///
/// 数（与单位配对用）：0-9 ０-９ 零 壹 貳 叁 肆 伍 陸 柒 捌 玖 半 一 二 三 四 五 六 七 八 九。
/// ⚠️ **故意不含**「十/百/千/拾/佰/仟」——避免把「十分 / 百分」这种程度副词误判成剂量
/// （剂量极少用整十整百作单位）。
/// 引导字：各 每 用 另 净 先 后 共 同 取（如 `各六分` 的「各」）。
///
/// 防误杀三道防线：① 段内必须**同时有数+单位**才包（单独的「三」、落单的「分」不包）；
/// ② 单位段尾是「分」且其后紧跟「之」→ 判为分数（如 `三分之二`），整段不包；
/// ③ 数集不含十/百/千 → 「十分 / 百分」不包。炮制词（炒/炙/煅…）不在集合，留在正文。
pub fn split_dosage_to_gloss(text: &str) -> String {
    // 数（与单位配对用）：阿拉伯数字 + 中文小写/大写数字 + 半。
    // ⚠️ 故意不含「十/百/千/拾/佰/仟」：避免把「十分 / 百分」误判成剂量。
    let is_num = |c: char| {
        matches!(
            c,
            '0'..='9' | '０'..='９' | '零' | '壹' | '貳' | '叁' | '肆' | '伍' | '陸'
                | '柒' | '捌' | '玖' | '半' | '一' | '二' | '三' | '四' | '五' | '六'
                | '七' | '八' | '九'
        )
    };
    // 单位（繁简同收）：以「重量 / 古量 / 丸劑」为主，全是古籍方剂里**几乎不会**
    // 作普通量词用的字，确保零误判。一般量词（個/枚/粒/片/碗/杯…）不放进来。
    let is_unit = |c: char| {
        matches!(
            c,
            '錢' | '钱' | '分' | '兩' | '两' | '克' | '斤' | '寸' | '升' | '斗' | '合'
                | '匕' | '撮' | '丸' | '劑' | '剂'
        )
    };
    // 引导字：各 每 用 另 净 先 后 共 同 取（如 `各六分` 的「各」）。
    let is_guide = |c: char| {
        matches!(
            c,
            '各' | '每' | '用' | '另' | '净' | '先' | '后' | '共' | '同' | '取'
        )
    };
    let is_dose = |c: char| is_num(c) || is_unit(c) || is_guide(c);
    let cs: Vec<char> = text.chars().collect();
    let n = cs.len();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0usize;
    while i < n {
        if is_dose(cs[i]) {
            let start = i;
            while i < n && is_dose(cs[i]) {
                i += 1;
            }
            let seg: String = cs[start..i].iter().collect();
            let has_unit = seg.chars().any(is_unit);
            let has_num = seg.chars().any(is_num);
            // 「分」作**分数分母**（如 `三分之二`）时，整段不当剂量：否则会把
            // 「三分之二」切开。满足「段内含分、分在段尾、分后紧跟 之」即为分数。
            let is_fraction = seg.ends_with('分') && i < n && cs[i] == '之';
            if has_unit && has_num && !is_fraction {
                out.push_str("[注]");
                out.push_str(&seg);
                out.push_str("[/注]");
            } else {
                out.push_str(&seg);
            }
        } else {
            out.push(cs[i]);
            i += 1;
        }
    }
    out
}

// ===========================================================================
// 中医方剂导入专用：标点彻底古籍化 + 剥壳夹注 + 清理废标点 + 方名升格
// （v1.9.16，用户硬规矩：现代标点绝不准进竖排网格）
// ===========================================================================

/// **剥壳 + 古籍化 + 清废标点**——中医秘方 Markdown 进排版引擎前的最后一道。
///
/// 三道死命令：
/// 1. **剥壳夹注**：中英文括号 `（）()` 一律从文本流删除。笺读用 `[注]` 标记
///    承载夹注，括号在竖排里只会被渲染层当成「竖排括号 / 引号」画出来，极丑；
///    真机截图里 `川芎（三錢）` 的括号正是这么漏出的。
/// 2. **标点古籍化**：半角 `,` `.` → 全角 `，` `。`；`、`，` `。` `、` 直接交给
///    渲染层的朱砂句读（读类→实心墨滴、句类→空心圈〇），绝不让原字形进网格。
/// 3. **清理废标点**：OCR / Markdown 产生的 `· • ※ ◇` 等不规则记号，以及残余
///    ASCII 标点（`!@#$%^&*_+=~|\/<>?` 等）直接滤掉。
///
/// **不动 `[注]…[/注]` 等标记内部**（沿用 `map_outside_markers` 口径）：否则会把
/// 注音 / 夹注标记里的冒号也洗掉、断掉解析。
pub fn tcm_clean(text: &str) -> String {
    let cs: Vec<char> = text.chars().collect();
    let n = cs.len();
    let mut out = String::with_capacity(n);
    let mut i = 0usize;
    while i < n {
        // 标记内部原样透传（注音 / 夹注 / 高亮 …）。
        if cs[i] == '[' {
            let mut j = i + 1;
            while j < n && cs[j] != ']' {
                j += 1;
            }
            if j < n {
                out.extend(&cs[i..=j]);
                i = j + 1;
                continue;
            }
        }
        let c = cs[i];
        // ① 括号：中英文一律删（剥壳夹注）。
        if matches!(c, '（' | '）' | '(' | ')') {
            i += 1;
            continue;
        }
        // ③ 废标点 / 不规则记号：直接滤掉。
        if is_junk_punct(c) {
            i += 1;
            continue;
        }
        // ② 半角句读 → 全角（交给渲染层画朱砂记号，绝不印原字形）。
        match c {
            ',' => out.push('，'),
            '.' => out.push('。'),
            other => out.push(other),
        }
        i += 1;
    }
    out
}

/// 不规则记号 / 残余 ASCII 标点（古籍竖排里无处安放，直接剔除）。
fn is_junk_punct(c: char) -> bool {
    matches!(
        c,
        '·' | '・' | '•' | '※' | '◇' | '◆' | '□' | '■' | '○' | '△' | '▲' | '=' | '+' | '*'
            | '&' | '%' | '#' | '@' | '$' | '^' | '~' | '`' | '|' | '\\' | '/' | '<' | '>' | '?'
            | '_' | '“' | '”' | '‘' | '’' | '—' | '–' | '―' | '：' | '；'
    )
}

/// **智能方名升格**：极严格的方书标题判定（用户 v1.9.16）。
///
/// 同时满足以下四条才认作「方名 → ChapterTitle（榜眼大字）」，否则当普通正文：
/// * 该行以 **方 / 湯 / 散 / 丸 / 丹** 结尾（如 `牙痛妙方`、`血山崩方`、`六味丸`）；
/// * 不含任何**剂量单位**（錢 钱 兩 两 分 克 斤 寸 升 斗 合 匕 撮 片 枚 粒）——
///   药行（如 `川芎三錢`）必含 `錢` 而落第，绝不误升格；
/// * 字数 ≤ 8（极短）；
/// * 不含任何句读（方名是「没有标点的独立短句」）。
///
/// ⚠️ 剂量单位集**故意不含** 方/湯/散/丸/丹 自身——它们是方名后缀，可能在
/// 方名里出现（`六味丸`），不能当剂量排掉。
pub fn is_fang_title(s: &str) -> bool {
    let t: Vec<char> = s.trim().chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() || t.len() > 8 {
        return false;
    }
    // 结尾必须是五种方名后缀之一。
    match t.last() {
        Some(c) if matches!(c, '方' | '湯' | '散' | '丸' | '丹') => {}
        _ => return false,
    }
    // 含剂量单位 → 是药行 / 剂量说明，不是方名。
    if t.iter().any(|c| {
        matches!(
            c,
            '錢' | '钱' | '兩' | '两' | '分' | '克' | '斤' | '寸' | '升' | '斗' | '合' | '匕'
                | '撮' | '片' | '枚' | '粒'
        )
    }) {
        return false;
    }
    // 含句读 → 是正文句，不是方名。
    if t.iter().any(|&c| crate::types::is_punct(c)) {
        return false;
    }
    true
}

// ===========================================================================
// 测试：带乱码 / 广告 / 假标点的脏文本
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- TXT：广告 ----------

    #[test]
    fn ad_lines_are_caught() {
        assert!(is_piracy_line("本书由顶点小说提供下载"));
        assert!(is_piracy_line("更多精品小说请访问 www.biquge.com"));
        assert!(is_piracy_line("https://www.example.com/book/123 无弹窗阅读"));
        assert!(is_piracy_line("TXT下载：http://www.xs8.cn"));
        assert!(is_piracy_line("欢迎访问八一中文网，求收藏求推荐票"));
        assert!(is_piracy_line("手机阅读请扫码下载客户端"));
    }

    /// **红线**：再脏的判据也不能碰正文。这几句是真古籍里的话。
    ///
    /// 关键词表里**没有裸「本站」**——只有「本站地址 / 访问本站 / 收藏本站」
    /// 这类完整话术。所以「本站者，谓此心也。」这种文言用法必须放行。
    #[test]
    fn ad_rules_never_touch_real_text() {
        for s in [
            "道可道，非常道。名可名，非常名。",
            "子曰：学而时习之，不亦说乎？",
            "项籍者，下相人也，字羽。",
            "《史记》卷七·项羽本纪第七",
            "是故空中无色，无受想行识。",
            "本站者，谓此心也。",
            "盗亦有道乎？",     // 含「盗」但不是广告
            "书曰：三人行必有我师。", // 含「书」字，不是「本书由」
            "请学为圃。",       // 含「请」字
        ] {
            assert!(!is_piracy_line(s), "正文被误杀：{s}");
        }
    }

    /// **虚词防误杀**（用户点名）：古汉语的「曰 / 乎 / 之 / 者 / 也 / 矣 /
    /// 焉 / 哉」一个都不能少，书名号《》与篇题也不能被当成脏东西削掉。
    #[test]
    fn classical_particles_survive_every_stage() {
        // 这段刻意塞满古汉语虚词（曰 / 乎 / 之 / 者 / 也 / 矣 / 焉 / 哉 / 其 /
        // 而 / 则 / 乃 / 斯 / 夫）与真书名号《》——用户点名要盯的「误杀高发区」。
        let src = "\
子曰：学而时习之，不亦说乎？有朋自远方来，不亦乐乎？
人不知而不愠，不亦君子乎？
《论语》学而第一
曾子曰：吾日三省吾身。
仁者爱人，智者知人，勇者不惧焉。
逝者如斯夫，不舍昼夜。
学而不思则罔，思而不学则殆。
君子之过也，如日月之食焉：过也，人皆见之；更也，人皆仰之。
吾知其所矣。大哉问！乃不知有汉，无论魏晋。
是可忍也，孰不可忍也？
";
        // ① 广告清洗后：字一个不少。
        let (after_ad, dropped) = strip_piracy_lines_counted(src);
        assert_eq!(dropped, 0, "纯净文言不该被判广告");
        for w in [
            "曰", "乎", "之", "者", "也", "矣", "焉", "哉", "其", "而", "则", "乃",
            "《论语》", "学而第一", "仁者爱人", "逝者如斯夫",
        ] {
            assert!(after_ad.contains(w), "虚词/书名号被误删：{w}");
        }
        // ② HTML 残留洗净后：还是原样（无标签可剥，实体也不该乱解）。
        assert_eq!(strip_html_residue(src).trim_end(), src.trim_end());
        // ③ 专名号：这段没有 <u>，原样返回。
        assert_eq!(proper_noun_marks(src), src);
        // ④ 图片假标点：这段没有 <img>，原样返回。
        assert_eq!(img_punct_to_text(src), src);
        // ⑤ DOCX 段内整理：不 trim 掉句末句号，也不动虚词。
        assert_eq!(tidy_docx_para("　子曰：学而时习之，不亦说乎？"), "子曰：学而时习之，不亦说乎？");
    }

    /// **书名号防误杀**：《》、「」、（）这些是真标点，不能当「括号注疏」吃掉。
    #[test]
    fn book_title_marks_survive() {
        let s = "《莊子》內篇·逍遙遊第一";
        assert_eq!(strip_html_residue(s), s);
        assert!(!is_piracy_line(s));
        // 夹注归一走的是 `brackets_to_notes`（不属本模块），本模块只保证
        // 这些字符不会被自己的规则删掉。
        assert_eq!(tidy_docx_para(s), s);
    }

    /// 超长行豁免：古文长段落里就算撞了关键词也不删。
    #[test]
    fn long_line_is_exempt() {
        let long = "本站".to_string() + &"天地玄黄宇宙洪荒".repeat(20);
        assert!(long.chars().count() > PIRACY_MAX_CHARS);
        assert!(!is_piracy_line(&long));
    }

    #[test]
    fn strip_piracy_keeps_structure() {
        let dirty = "第一章 逍遥游\n北冥有鱼，其名为鲲。\n本书由XX提供下载\n更多精品请访问www.a.com\n鲲之大，不知其几千里也。\n";
        let (clean, dropped) = strip_piracy_lines_counted(dirty);
        assert_eq!(dropped, 2, "应剔掉两行广告");
        assert!(clean.contains("北冥有鱼"));
        assert!(clean.contains("鲲之大"));
        assert!(!clean.contains("提供下载"));
        assert!(!clean.contains("www.a.com"));
        // 幂等。
        let (again, dropped2) = strip_piracy_lines_counted(&clean);
        assert_eq!(dropped2, 0);
        assert_eq!(again, clean);
    }

    // ---------- TXT：智能断行 ----------

    #[test]
    fn no_punct_means_never_break() {
        // 上一行没标点 → 绝不断（用户硬要求）。
        let a = "北冥有鱼其名为鲲";
        assert!(!should_break_para(a, a.chars().count(), MERGE_PARA_CHARS));
        let b = "鲲之大".repeat(100);
        assert!(!should_break_para(&b, b.chars().count(), MERGE_PARA_CHARS));
    }

    #[test]
    fn no_punct_forever_still_has_a_hard_ceiling() {
        // 扫描本：通篇没有句号。没标点不该断，但也不能无限攒下去。
        let s = "北冥有鱼".repeat(90); // 360 字
        assert!(!should_break_para(&s, 360, MERGE_PARA_CHARS));
        // 越过绝对上限就必须断。
        assert!(should_break_para(&s, PARA_HARD_MAX, MERGE_PARA_CHARS));
        assert!(should_break_para(&s, PARA_HARD_MAX + 1, MERGE_PARA_CHARS));
    }

    #[test]
    fn long_settled_para_breaks() {
        let s = "道可道，非常道。".repeat(20); // 128 字，且以「。」收尾
        assert!(s.chars().count() >= MERGE_PARA_CHARS);
        assert!(should_break_para(&s, s.chars().count(), MERGE_PARA_CHARS));
        // 短的不该断。
        let short = "道可道，非常道。";
        assert!(!should_break_para(short, short.chars().count(), MERGE_PARA_CHARS));
    }

    #[test]
    fn quote_end_counts_as_settled() {
        // 对话体：」。收尾 → 算收句，不会永远合并下去。
        let q = "「为之奈何？」".repeat(25);
        assert!(should_break_para(&q, q.chars().count(), MERGE_PARA_CHARS));
    }

    // ---------- TXT：编码 ----------

    #[test]
    fn sniff_utf8_plain() {
        let s = "北冥有鱼，其名为鲲。";
        assert_eq!(sniff_decode(s.as_bytes()), s);
    }

    #[test]
    fn sniff_utf8_bom() {
        let mut v = vec![0xEF, 0xBB, 0xBF];
        v.extend_from_slice("莊子".as_bytes());
        assert_eq!(sniff_decode(&v), "莊子");
    }

    #[test]
    fn sniff_gbk() {
        // 真·往返：拿 GBK 编一遍再让嗅探器认回来（不手搓字节，手搓容易造错）。
        let (bytes, _, _) = encoding_rs::GBK.encode("北冥有鱼，其名为鲲。");
        assert!(std::str::from_utf8(&bytes).is_err(), "这串字节本该不是合法 UTF-8");
        let s = sniff_decode(&bytes);
        assert!(s.contains("北冥有鱼"), "GBK 没解出来：{s:?}");
        assert!(s.contains("其名为鲲"));
        assert!(!s.contains('\u{FFFD}'), "解出了替换字符：{s:?}");
    }

    #[test]
    fn sniff_utf16_le_bom() {
        let u16s: Vec<u16> = "莊子".encode_utf16().collect();
        let mut v = vec![0xFF, 0xFE];
        for u in u16s {
            v.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(sniff_decode(&v), "莊子");
    }

    #[test]
    fn mojibake_score_sees_replacement_chars() {
        assert_eq!(mojibake_score("北冥有鱼"), 0.0);
        assert!(mojibake_score("北\u{FFFD}有鱼") >= MOJIBAKE_LIMIT);
        assert!(mojibake_score("锟斤拷锟斤拷") >= MOJIBAKE_LIMIT);
    }

    #[test]
    fn utf8_with_replacement_falls_back_to_gbk() {
        // UTF-8 合法但含 U+FFFD → 体检不过 → 换 GBK 再试。
        let mut v = "莊子".as_bytes().to_vec();
        v.extend_from_slice(&[0xE4, 0xB8, 0xAD]); // 中（合法 UTF-8）
        let out = sniff_decode(&v);
        assert!(!out.contains('\u{FFFD}'), "仍带替换字符：{out:?}");
    }

    // ---------- EPUB ----------

    #[test]
    fn inline_style_is_stripped_but_tag_kept() {
        let h = r#"<p style="text-indent:2em;font-size:16px">北冥有鱼</p>"#;
        let out = strip_inline_styles(h);
        assert_eq!(out, "<p>北冥有鱼</p>");
        // 别的属性要留着（class 后面专名号还要用）。
        let h2 = r#"<p class="body" style="color:red">鲲</p>"#;
        assert_eq!(strip_inline_styles(h2), r#"<p class="body">鲲</p>"#);
    }

    #[test]
    fn style_attr_without_quote_is_handled() {
        assert_eq!(strip_inline_styles("<p style=foo>鲲</p>"), "<p>鲲</p>");
    }

    #[test]
    fn unclosed_style_is_left_alone() {
        // 引号没闭合：不动（宁可留脏属性，绝不切坏标签）。
        let h = r#"<p style="text-indent:2em>鲲</p>"#;
        assert_eq!(strip_inline_styles(h), h);
    }

    #[test]
    fn img_punct_becomes_real_punct() {
        assert_eq!(img_punct_to_text(r#"鲲<img class="punct" alt="，"/>鹏"#), "鲲，鹏");
        assert_eq!(img_punct_to_text(r#"鲲<img class="ytz" alt="。"/>鹏"#), "鲲。鹏");
        let s = img_punct_to_text(r#"鲲<img src="images/dot.png" alt="、"/>鹏"#);
        assert_eq!(s, "鲲、鹏");
    }

    #[test]
    fn real_image_is_not_touched() {
        let h = r#"<img class="cover" src="a.jpg" alt="封面图"/>"#;
        assert_eq!(img_punct_to_text(h), h);
    }

    #[test]
    fn proper_noun_marks_are_emitted() {
        assert_eq!(proper_noun_marks("<u>项羽</u>"), "〔人:项羽〕");
        assert_eq!(
            proper_noun_marks(r#"<u class="booktitle">莊子</u>"#),
            "〔书:莊子〕"
        );
        assert_eq!(proper_noun_marks(r#"<u class="place">垓下</u>"#), "〔地:垓下〕");
    }

    #[test]
    fn long_bare_u_is_not_a_name() {
        // 超过 8 字的裸 <u>：只去标签，不标专名（防误杀）。
        let long = "<u>北冥有鱼其名为鲲鹏之大不知几千里</u>";
        assert_eq!(proper_noun_marks(long), "北冥有鱼其名为鲲鹏之大不知几千里");
    }

    #[test]
    fn ul_is_not_u() {
        // `<ul>` 不是 `<u>`：不能误认。
        assert_eq!(proper_noun_marks("<ul><li>鲲</li></ul>"), "<ul><li>鲲</li></ul>");
    }

    // ---------- JSON ----------

    #[test]
    fn deep_pick_finds_nested_content() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"book":{"meta":{"title":"莊子"},"ch":[{"no":1,"data":{"content":"北冥有鱼"}}]}}"#,
        )
        .unwrap();
        let got = deep_pick_text(&v);
        assert_eq!(got, vec!["北冥有鱼".to_string()]);
        // 书名「莊子」没被当成正文收进来。
        assert!(!got.iter().any(|s| s == "莊子"));
    }

    #[test]
    fn deep_pick_understands_chinese_keys() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"卷一":[{"正文":"道可道"},{"正文":"非常道"}]}"#).unwrap();
        assert_eq!(deep_pick_text(&v), vec!["道可道", "非常道"]);
    }

    #[test]
    fn deep_pick_dedups() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"a":{"text":"鲲"},"b":{"text":"鲲"}}"#).unwrap();
        assert_eq!(deep_pick_text(&v), vec!["鲲"]);
    }

    #[test]
    fn html_residue_is_washed() {
        assert_eq!(strip_html_residue("<p>北冥有鱼</p>"), "北冥有鱼");
        assert_eq!(
            strip_html_residue("<p>鲲之大</p><p>不知几千里</p>"),
            "鲲之大\n不知几千里"
        );
        // 实体：先剥标签再解，所以 &lt;p&gt; 会原样留下字面文本而不是被吃掉。
        assert_eq!(strip_html_residue("&lt;p&gt;字面量&lt;/p&gt;"), "<p>字面量</p>");
        assert_eq!(strip_html_residue("a &amp; b"), "a & b");
    }

    #[test]
    fn no_html_means_untouched() {
        assert_eq!(strip_html_residue("道可道，非常道。"), "道可道，非常道。");
    }

    // ---------- DOCX ----------

    #[test]
    fn soft_break_joins_cjk_without_space() {
        // 软回车两侧都是中文 → 直接接上，不空格（留缝就是竖排里的空格子）。
        assert_eq!(tidy_docx_para("北冥\n有鱼"), "北冥有鱼");
        assert_eq!(tidy_docx_para("鲲之大\n\n不知其几千里也"), "鲲之大不知其几千里也");
    }

    #[test]
    fn soft_break_keeps_space_for_latin() {
        assert_eq!(tidy_docx_para("hello\nworld"), "hello world");
    }

    #[test]
    fn indent_junk_is_trimmed() {
        assert_eq!(tidy_docx_para("\u{3000}\u{3000}北冥有鱼"), "北冥有鱼");
        assert_eq!(tidy_docx_para("\t\t鲲之大"), "鲲之大");
        assert_eq!(tidy_docx_para("   其名为鲲   "), "其名为鲲");
        // 连续的空白压成一个，但**单个空格必须保留**——那是原文的一部分。
        assert_eq!(tidy_docx_para("北冥   有鱼"), "北冥 有鱼");
        assert_eq!(tidy_docx_para("卷一 周紀一"), "卷一 周紀一");
    }

    #[test]
    fn footnotes_are_parsed() {
        let xml = r#"<w:footnotes xmlns:w="x">
            <w:footnote w:id="0"><w:p><w:r><w:t>sep</w:t></w:r></w:p></w:footnote>
            <w:footnote w:id="2"><w:p><w:r><w:t>項羽，名籍，字羽。</w:t></w:r></w:p></w:footnote>
            <w:footnote w:id="3"><w:p><w:r><w:t>下相，縣名。</w:t></w:r></w:p></w:footnote>
        </w:footnotes>"#;
        let m = parse_footnotes_xml(xml);
        assert_eq!(m.len(), 2, "0/1 是分隔符占位，不该收");
        assert_eq!(m.get(&2).map(|s| s.as_str()), Some("項羽，名籍，字羽。"));
        assert_eq!(m.get(&3).map(|s| s.as_str()), Some("下相，縣名。"));
    }

    #[test]
    fn footnotes_are_attached_as_gloss() {
        let mut m = HashMap::new();
        m.insert(2, "項羽，名籍。".to_string());
        let body = format!("項籍者{}下相人也。", footnote_sentinel(2));
        assert_eq!(attach_footnotes(&body, &m), "項籍者[注]項羽，名籍。[/注]下相人也。");
        // 查不到的注号：不留裸注号。
        let orphan = format!("項籍者{}下相人也。", footnote_sentinel(9));
        assert_eq!(attach_footnotes(&orphan, &m), "項籍者下相人也。");
    }

    #[test]
    fn text_without_sentinel_is_untouched() {
        let m = HashMap::new();
        assert_eq!(attach_footnotes("北冥有鱼，其名为鲲。", &m), "北冥有鱼，其名为鲲。");
    }

    // ---------- 性能：20M 大文件 / 几万张图 ----------

    /// **20 MB 级 txt 的清洗耗时**。
    ///
    /// 用户给的红线：清洗一个 20M 的文件不能超 100ms。这里就是拿数据说话——
    /// 断言卡在 400ms（给慢机器留三倍余量），真实耗时用
    /// `cargo test -- --nocapture` 看 `[bench]` 那行。
    #[test]
    fn big_txt_cleans_fast() {
        let unit = "北冥有魚其名為鯤鯤之大不知其幾千里也化而為鳥其名為鵬鵬之背不知其幾千里也。\n";
        let lines = 20 * 1024 * 1024 / unit.len();
        let mut big = String::with_capacity(lines * unit.len() + 64 * 1024);
        for i in 0..lines {
            // 每 500 行掺一条广告（真实盗版 txt 的密度）。
            if i % 500 == 0 {
                big.push_str("本书由某某小说网提供下载\n");
            }
            big.push_str(unit);
        }
        let bytes = big.len();
        let t0 = std::time::Instant::now();
        let (_clean, dropped) = strip_piracy_lines_counted(&big);
        let d = t0.elapsed();
        eprintln!("[bench] TXT 广告清洗：{} 字节 / {} 行 → {:?}（剔 {} 行）", bytes, lines, d, dropped);
        assert!(dropped >= lines / 500, "广告没剔干净");
        assert!(d.as_millis() < 400, "20M 清洗 {:?} 太慢（红线 100ms）", d);
    }

    /// **HTML 侧必须是线性的**：几万个 `<img>` / `<u>` 不能拖成 O(n²)。
    ///
    /// 早先的写法每轮都对剩余串重新 `to_ascii_lowercase()`，2 万个标签就是
    /// 2 万次全量拷贝 —— 这条测试就是钉死「不许再退回去」的桩。
    #[test]
    fn html_with_many_tags_is_linear() {
        let mut h = String::with_capacity(1_500_000);
        for _ in 0..20_000 {
            h.push_str(r#"<p style="text-indent:2em">鲲<img class="punct" alt="，"/>鹏<u>项羽</u></p>"#);
        }
        let bytes = h.len();
        let t0 = std::time::Instant::now();
        let s = strip_inline_styles(&h);
        let s = img_punct_to_text(&s);
        let s = proper_noun_marks(&s);
        let d = t0.elapsed();
        eprintln!("[bench] EPUB 三件套：{} 字节 / 2 万组标签 → {:?}", bytes, d);
        assert!(s.contains("鲲，鹏"), "图片标点没转成文本");
        assert!(s.contains("〔人:项羽〕"), "专名号没打上");
        assert!(!s.contains("style="), "内联样式没剥掉");
        assert!(d.as_millis() < 500, "2 万组标签耗时 {:?}，疑似 O(n²)", d);
    }

    /// **乱码体检也不能是 O(n²)**：它要扫全篇每一个字。
    #[test]
    fn mojibake_score_is_fast() {
        let s = "北冥有鱼，其名为鲲。".repeat(500_000); // ~5M 字
        let t0 = std::time::Instant::now();
        let sc = mojibake_score(&s);
        let d = t0.elapsed();
        eprintln!("[bench] mojibake_score：{} 字节 → {:?}（分数 {sc}）", s.len(), d);
        assert_eq!(sc, 0.0);
        assert!(d.as_millis() < 200, "体检 {:?} 太慢", d);
    }

    // ---------- 综合：一整坨脏文本 ----------

    #[test]
    fn e2e_dirty_text_comes_out_clean() {
        let dirty = "\
第一章 逍遙遊
本書由頂點小說提供下載
更多精品請訪問 www.biquge.com
北冥有魚
其名為鯤
鯤之大
不知其幾千里也
手機閱讀請掃碼下載客戶端
化而為鳥
其名為鵬
";
        let (clean, dropped) = strip_piracy_lines_counted(dirty);
        assert_eq!(dropped, 3, "三条广告全得剔掉");
        assert!(!clean.contains("www."));
        assert!(!clean.contains("提供下載"));
        assert!(!clean.contains("掃碼"));
        // 正文一个字都不能少。
        for w in ["北冥有魚", "其名為鯤", "鯤之大", "不知其幾千里也", "化而為鳥", "其名為鵬"] {
            assert!(clean.contains(w), "正文被误杀：{w}");
        }
        // 章标题也得在。
        assert!(clean.contains("第一章 逍遙遊"));
        // 行数：原 10 行 → 剔 3 行 = 7 行（结构原样，只删行不断段）。
        assert_eq!(clean.lines().count(), 7);
    }

    // ---------- 中医剂量拆分 ----------

    #[test]
    fn dose_split_basic() {
        assert_eq!(split_dosage_to_gloss("瞿麥壹錢"), "瞿麥[注]壹錢[/注]");
        assert_eq!(split_dosage_to_gloss("當歸三錢"), "當歸[注]三錢[/注]");
        assert_eq!(
            split_dosage_to_gloss("甘草川烏各六分"),
            "甘草川烏[注]各六分[/注]"
        );
        assert_eq!(
            split_dosage_to_gloss("蒲黄 三錢 五靈脂 醋炒 三錢 川芎 三錢"),
            "蒲黄 [注]三錢[/注] 五靈脂 醋炒 [注]三錢[/注] 川芎 [注]三錢[/注]"
        );
        // 炮制词（炒）留在正文，不被包、不被删。
        assert_eq!(
            split_dosage_to_gloss("川芎（炒）二钱"),
            "川芎（炒）[注]二钱[/注]"
        );
        // 钱半（=1.5 錢）也认。
        assert_eq!(
            split_dosage_to_gloss("京芥穗 錢半"),
            "京芥穗 [注]錢半[/注]"
        );
    }

    #[test]
    fn dose_split_no_false_positive() {
        // 普通正文里的「分」不包：三分之一、成分、分析。
        assert_eq!(split_dosage_to_gloss("三分之二"), "三分之二");
        assert_eq!(split_dosage_to_gloss("分析原因"), "分析原因");
        // 程度副词「十分 / 百分」不包（数集不含十/百）。
        assert_eq!(split_dosage_to_gloss("十分感謝"), "十分感謝");
        // 普通量词「個」不放进单位集：绝不误包「一個人」。
        assert_eq!(split_dosage_to_gloss("一個人"), "一個人");
        // 单独的「三」不包（无单位）。
        assert_eq!(split_dosage_to_gloss("三國演義"), "三國演義");
        // 极短修饰词绝不丢：单独的「炒」留在正文。
        assert_eq!(split_dosage_to_gloss("炒"), "炒");
    }

    // ---------- v1.9.16：剥括号 + 标点古籍化 + 清废标点 ----------

    #[test]
    fn tcm_clean_strips_parens_everywhere() {
        // 夹注括号（中英文）一律删，只留纯文本。
        assert_eq!(tcm_clean("川芎（三錢）"), "川芎三錢");
        assert_eq!(tcm_clean("當歸(二錢)"), "當歸二錢");
        // 剂量已拆成 [注] 标记时，外侧括号仍要剥。
        assert_eq!(tcm_clean("川芎（[注]三錢[/注]）"), "川芎[注]三錢[/注]");
        // 括号绝不进网格。
        assert!(!tcm_clean("血山崩方（附方）").contains('（'));
        assert!(!tcm_clean("血山崩方（附方）").contains('）'));
    }

    #[test]
    fn tcm_clean_halfwidth_punct_to_fullwidth() {
        // 半角句读 → 全角（交给渲染层画朱砂记号）。
        assert_eq!(tcm_clean("救中風,童便灌飲即甦."), "救中風，童便灌飲即甦。");
        // 全角句读原样保留（不被当废标点滤掉）。
        assert_eq!(tcm_clean("甘草，川烏。"), "甘草，川烏。");
    }

    #[test]
    fn tcm_clean_filters_junk_punct() {
        // OCR / Markdown 不规则记号 + 残余 ASCII 标点直接滤掉。
        assert_eq!(tcm_clean("淨·水煎服"), "淨水煎服");
        assert_eq!(tcm_clean("每服三錢#"), "每服三錢");
        assert_eq!(tcm_clean("右咀*片"), "右咀片");
        // 标记内部不动：注音冒号保住。
        assert_eq!(tcm_clean("[拼音:hàn]漢"), "[拼音:hàn]漢");
    }

    // ---------- v1.9.16：智能方名升格 ----------

    #[test]
    fn fang_title_detects_real_names() {
        assert!(is_fang_title("牙痛妙方"));
        assert!(is_fang_title("血山崩方"));
        assert!(is_fang_title("六味丸"));
        assert!(is_fang_title("小兒三朝不飲乳方")); // 8 字，卡在阈值上
        assert!(is_fang_title("湯")); // 单字后缀也算
    }

    #[test]
    fn fang_title_rejects_medicine_lines() {
        // 药行含剂量单位 → 落第。
        assert!(!is_fang_title("川芎三錢"));
        assert!(!is_fang_title("五靈脂醋炒三錢"));
        assert!(!is_fang_title("當歸[注]二錢[/注]"));
        // 太长 → 落第。
        assert!(!is_fang_title("治婦人血氣心痛及冷氣攻心方")); // > 8
        // 带句读 → 落第（是正文句不是方名）。
        assert!(!is_fang_title("牙痛妙方。"));
        assert!(!is_fang_title("血山崩，方"));
    }
}
