//! **古籍净化漏斗 · 文本层**（v1.80.0）
//!
//! 现代出版物的 EPUB 里，古籍正文只占一小半：封面题名页、版权页、丛书名、
//! 译注者、目录、前言、序言、索引……全都被转换器当成「正文」排进了竖排。
//! 一册《尔雅》里，「管锡华◎译注」「全本全注全译丛书」和「初、哉、首、基」
//! 挤在同一页上——既毁了版面，也污染了 FTS5 索引。
//!
//! 本模块只管**纯文本层**的清洗（结构层 / DOM 层在各格式的 parser 里，见
//! `app::importer::epub_parser`）。三条判据，从硬到软：
//!
//! ```text
//!   ① 铁证词（ISBN / 版权所有 / 图书在版编目…）→ 整行死，不看行长
//!   ② 软词（丛书 / 译注 / 主编 / 出版社…）    → 只在「短行」上生效
//!   ③ 章节名（前言 / 目录 / 索引 / 后记…）    → 整节（front matter）死
//! ```
//!
//! # 为什么②只打短行
//!
//! 「出版社」这类词在**长句子**里往往是正文的一部分——前言里 citing 一句
//! 「中华书局1984年版」是学术行文，不是版权页。硬删会连正文一起吃掉。
//! 真版权页的词永远落在**短行**上（「管锡华◎译注」8 字、「全本全注全译丛书」
//! 8 字），所以短行才杀，长行放过——**宁可漏判，不可误杀**（与 `cleaner`
//! 清洗口径一致）。
//!
//! # 第三层：启发式正文起点
//!
//! 没有目录 / 目录不可信时的兜底：[`find_body_start`] 逐段给「文言度」打分
//! （文言虚词加分、现代虚词与拉丁字母减分），找到**连续 [`BODY_RUN`] 段都像
//! 文言**的第一处，把它之前的全部扔掉。定位不到就返回 0——**认不出就不动**，
//! 绝不拿半吊子把握去砍真正文。

/// **铁证词**：出现即判定为出版噪声，与行长无关。
///
/// 这些词在正常古籍正文里几乎不可能出现（「字数」「印张」在训诂里也另有说法，
/// 但真撞上了也是出版方的话），所以敢无条件杀。
pub const PUB_HARD: &[&str] = &[
    "ISBN",
    "ISSN",
    "图书在版编目",
    "CIP",
    "版权所有",
    "翻印必究",
    "著作权所有",
    "责任编辑",
    "责任校对",
    "责任印制",
    "封面设计",
    "版式设计",
    "装帧设计",
    "插页",
    "字数",
    "印张",
    "开本",
    "定价",
    "印次",
    "版次",
    "书号",
    "经销",
    "发行所",
    "印刷厂",
    "新华书店",
    "图书出版",
    "出版社发行",
    "z-library",
    "z-lib",
    "zlib",
    "1lib",
    "annas-archive",
    "libgen",
    "www.",
    "http://",
    "https://",
    "mailto:",
    "©",
    "®",
];

/// **软词**：只在短行（≤ [`PUB_SOFT_MAX_CHARS`] 字）上生效。
///
/// 长句里出现「出版社」多半是征引版本，不是版权页。
pub const PUB_SOFT: &[&str] = &[
    "出版社",
    "出版发行",
    "出版集团",
    "出版",
    "发行",
    "印行",
    "刊行",
    "丛书",
    "译注",
    "注译",
    "译著",
    "编译",
    "主编",
    "编著",
    "编注",
    "校注",
    "校点",
    "整理",
    "编辑部",
    "编委会",
    "编辑委员会",
    "装帧",
    "书店",
    "书局",
    "印社",
    "官网",
    "扫码",
    "电子书",
    "电子版",
];

/// 软词生效的行长上限（字）：24。
///
/// 「管锡华◎译注」8 字、「全本全注全译丛书」8 字、「中华书局」4 字都在这头；
/// 前言里那句 200 字的学术长句在这头之外。
pub const PUB_SOFT_MAX_CHARS: usize = 24;

/// **front / back matter 的章节名**：目录里见了这些名字，整节不收。
///
/// 「序」「跋」本身是古文文体，但它们几乎总带着整理者的现代白话，且不属于
/// 经文——用户口径是「剔除首尾的现代版权声明页、前言、序言」，照办。
/// 判据是**整行等于 / 以之开头**，不是「含」：「释诂第一」不含「序」字，安全。
pub const FRONT_MATTER_TITLES: &[&str] = &[
    "封面",
    "扉页",
    "书名页",
    "版权页",
    "版权",
    "出版说明",
    "说明",
    "凡例",
    "例言",
    "编例",
    "前言",
    "序言",
    "序",
    "叙",
    "引言",
    "导读",
    "导言",
    "编者的话",
    "写在前面",
    "内容简介",
    "内容提要",
    "作者简介",
    "作者介绍",
    "译者简介",
    "目录",
    "目次",
    "目録",
    "总目",
    "索引",
    "笔画索引",
    "附录",
    "附録",
    "附",
    "后记",
    "後記",
    "跋",
    "校后记",
    "参考文献",
    "引用书目",
    "鸣谢",
    "致谢",
];

/// 文言虚词（强）：出现一个记 1 分。
const WENYAN_STRONG: &[char] = &[
    '也', '矣', '焉', '哉', '乎', '耶', '曰', '者', '谓', '斯', '兹', '奚', '孰', '咸', '悉',
    '蓋', '盖', '惟', '唯', '爾', '尔', '汝', '吾', '予', '弗', '勿', '皆', '俱', '既', '遂',
    '乃', '故', '耳', '兮', '夫',
];

/// 文言虚词（弱）：出现一个记 0.4 分——这些字现代文里也常用，不敢给满。
const WENYAN_WEAK: &[char] = &['之', '其', '所', '於', '于', '以', '而', '則', '则', '若', '如'];

/// 现代汉语标记：出现一个扣 1 分。
const MODERN_MARKS: &[char] = &['的', '了', '着', '吗', '呢', '吧'];

/// 现代汉语双字词：出现一个扣 1.5 分（比单字更铁证）。
const MODERN_WORDS: &[&str] = &[
    "我们", "他们", "它们", "这种", "那种", "这个", "那个", "因为", "所以", "而且", "但是",
    "然后", "如果", "虽然", "对于", "关于", "以及", "并且", "进行", "通过", "世纪", "年代",
    "社会", "文化", "研究", "认为", "可以", "已经", "问题", "方面", "情况", "现代", "汉语",
    "词语", "年版", "公司", "大学", "教授", "先生", "女士", "电话", "邮箱", "网站", "电子",
];

/// 现代排版符号：出现一个扣 0.6 分（古籍整理本也用，故轻罚）。
const MODERN_PUNCT: &[char] = &['！', '？', '；', '：', '“', '”', '—', '%'];

/// 启发式起点认定需要的**连续文言段数**：3。
///
/// 一段像文言可能是目录里的一行；连着三段都像，才是正文开始了。
pub const BODY_RUN: usize = 3;

/// 一段被认作「文言」的最短字数：6。
pub const BODY_MIN_CHARS: usize = 6;

/// 一行 / 一段文本是不是出版噪声。
///
/// * 铁证词（[`PUB_HARD`]）→ 直接 true，不看行长（"ISBN 978-7-101" 多长都死）；
/// * 软词（[`PUB_SOFT`]）→ 仅当行长 ≤ [`PUB_SOFT_MAX_CHARS`] 才 true；
/// * 其余 → false。
pub fn is_pub_noise(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    // 铁证词：ASCII 词（ISBN / CIP / www. …）统一按大写比，大小写都逃不掉。
    let upper = t.to_ascii_uppercase();
    for w in PUB_HARD {
        let wu = w.to_ascii_uppercase();
        if upper.contains(&wu) {
            return true;
        }
    }
    if t.chars().count() <= PUB_SOFT_MAX_CHARS {
        for w in PUB_SOFT {
            if t.contains(w) {
                return true;
            }
        }
    }
    false
}

/// 章节名是不是 front / back matter（前言 / 目录 / 索引 / 后记…）。
///
/// 判据：去掉空白与常见装饰后，**以**表中某词开头，或整名**等于**某词。
/// 「释诂第一」「卷之上」之类正经篇名绝不误中。
pub fn is_front_matter_title(title: &str) -> bool {
    let t = title
        .trim()
        .trim_start_matches(|c: char| c.is_whitespace() || c == '【' || c == '[' || c == '《')
        .trim_end_matches(|c: char| c.is_whitespace() || c == '】' || c == ']' || c == '》');
    if t.is_empty() {
        return false;
    }
    // 「以之开头」兜住「出版说明（二）」，「以之结尾」兜住
    // 「《尔雅》词语笔画索引」——只判开头会漏掉书名在前的那类。
    FRONT_MATTER_TITLES
        .iter()
        .any(|w| t == *w || t.starts_with(w) || t.ends_with(w))
}

/// 一段文本的**文言度**：> 0 判文言，< 0 判现代白话。
///
/// 计分（按字数归一到每百字）：文言虚词加、现代虚词 / 双字词 / 拉丁字母 /
/// 阿拉伯数字 / 现代标点减。零分（既不文也不白，如「释诂第一」「1.001」）
/// 归到「不像正文」，交给调用方按场景处理。
pub fn classical_score(line: &str) -> f32 {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    if n == 0 {
        return 0.0;
    }
    let s = line;
    let mut score = 0.0f32;
    for c in &chars {
        if WENYAN_STRONG.contains(c) {
            score += 1.0;
        } else if WENYAN_WEAK.contains(c) {
            score += 0.4;
        } else if MODERN_MARKS.contains(c) {
            score -= 1.0;
        } else if MODERN_PUNCT.contains(c) {
            score -= 0.6;
        } else if c.is_ascii_alphanumeric() {
            score -= 0.3;
        }
    }
    for w in MODERN_WORDS {
        if s.contains(w) {
            score -= 1.5;
        }
    }
    score / n as f32 * 100.0
}

/// **第三层兜底：启发式正文起点**。
///
/// 逐行扫描，遇到连续 [`BODY_RUN`] 行「像文言」（字数 ≥ [`BODY_MIN_CHARS`]、
/// 非出版噪声、文言度 > 0）时，返回**这一段开头的行号**；之前全部丢弃。
/// 找不到就返回 0（**不动**，宁可留着也不砍错）。
pub fn find_body_start(lines: &[&str]) -> usize {
    let mut run = 0usize;
    for (i, raw) in lines.iter().enumerate() {
        let l = raw.trim();
        // 「≥ 0」而不是「> 0」：无虚词的经文（「道可道，非常道」）打 0 分，
        // 那也是正文；认作「不像正文」的是**扣成负分**的现代白话。
        let ok = l.chars().count() >= BODY_MIN_CHARS
            && !is_pub_noise(l)
            && classical_score(l) >= 0.0
            && !is_front_matter_title(l);
        if ok {
            run += 1;
            if run >= BODY_RUN {
                return i + 1 - BODY_RUN;
            }
        } else {
            run = 0;
        }
    }
    0
}

/// 逐行滤掉出版噪声，返回清洗后的文本（行序不变，空行不补）。
pub fn strip_pub_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for l in text.lines() {
        if is_pub_noise(l) {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(l);
    }
    out
}

/// 一段文本里「出版噪声行」的行数（节级判定用）。
pub fn noise_line_count(text: &str) -> usize {
    text.lines().filter(|l| is_pub_noise(l)).count()
}

/// 非空白字符数（节级「字数极少」判据用）。
///
/// 不用 `chars().count()`：版权页靠大量空格 / 缩进排版，按可见字算才准。
pub fn visible_chars(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// 整节是不是**垃圾节**：字数极少（< `min_chars`）且噪声行 ≥ `min_noise`。
///
/// 对应「某个 HTML 文件字数极少且全是出版标签 → 判定为垃圾文件跳过」。
/// 两个条件都满足才杀：字数多的一律放行（哪怕夹着两句出版信息，那也是正文里
/// 的征引，不是版权页）。
pub fn is_junk_section(text: &str, min_chars: usize, min_noise: usize) -> bool {
    visible_chars(text) < min_chars && noise_line_count(text) >= min_noise
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_words_die_at_any_length() {
        for l in [
            "ISBN 978-7-101-10123-4",
            "图书在版编目（CIP）数据",
            "版权所有·翻印必究",
            "责任编辑：张三　封面设计：李四",
            "定价：48.00元",
            "印张：12.5　开本：880×1230 1/32",
            "本书由 z-library 提供下载 www.example.com",
        ] {
            assert!(is_pub_noise(l), "铁证词漏判：{l}");
        }
    }

    #[test]
    fn soft_words_only_die_on_short_lines() {
        // 短行：题名页 / 版权页的排法
        assert!(is_pub_noise("管锡华◎译注"), "译注者未杀");
        assert!(is_pub_noise("全本全注全译丛书"), "丛书名未杀");
        assert!(is_pub_noise("中华书局"), "出版社未杀");
        assert!(is_pub_noise("陈桐生　译注"), "译注者未杀");
        // 长行：正文里征引版本，必须放过
        let long = "（清王先谦《释名疏证补》第314页，上海古籍出版社1984年版）此即所谓的近正说，这是后世大多数学者所接受的关于《尔雅》名义的解释。";
        assert!(!is_pub_noise(long), "学术长句被误杀");
    }

    #[test]
    fn real_classical_text_is_never_noise() {
        for l in [
            "初、哉、首、基、肈、祖、元、胎、俶、落、权舆，始也。",
            "道可道，非常道。名可名，非常名。",
            "釋詁第一",
            "林、烝、天、帝、皇、王、后、辟、公、侯，君也。",
        ] {
            assert!(!is_pub_noise(l), "正文被误杀：{l}");
        }
    }

    #[test]
    fn front_matter_titles_are_recognized() {
        for t in [
            "前言", "序言", "序", "目录", "目次", "《尔雅》词语笔画索引", "索引", "后记", "跋",
            "出版说明", "内容简介", "附录", "参考文献", "卷末索引", "出版说明（二）",
        ] {
            assert!(is_front_matter_title(t), "front matter 漏判：{t}");
        }
        for t in [
            "释诂第一",
            "释言第二",
            "卷之上",
            "道德经",
            "第一篇　逍遥游",
            "尔雅",
        ] {
            assert!(!is_front_matter_title(t), "正经篇名被误判：{t}");
        }
    }

    #[test]
    fn wenyan_scores_high_and_baihua_scores_low() {
        assert!(classical_score("初、哉、首、基、肈、祖、元、胎、俶、落、权舆，始也。") > 0.0);
        // 无虚词的经文打 0 分——那也是正文（≥ 0 即放行，见 find_body_start）。
        assert!(classical_score("道可道，非常道。名可名，非常名。") >= 0.0);
        assert!(
            classical_score("《尔雅》是第一部内容和体例都比较完备的汉语词典，在历史上它被列入儒家经典。") < 0.0
        );
        assert!(classical_score("本书的整理工作得到了许多专家的帮助。") < 0.0);
    }

    #[test]
    fn body_start_skips_the_modern_head() {
        let text = concat!(
            "尔雅\n",
            "全本全注全译丛书\n",
            "管锡华◎译注\n",
            "前言\n",
            "《尔雅》是第一部内容和体例都比较完备的汉语词典，对后世多个学科都产生过重要的影响。\n",
            "本书的整理工作得到了许多专家的帮助，在此一并致谢。\n",
            "释诂第一\n",
            "初、哉、首、基、肈、祖、元、胎、俶、落、权舆，始也。\n",
            "林、烝、天、帝、皇、王、后、辟、公、侯，君也。\n",
            "如、適、之、嫁、徂、逝，往也。\n",
        );
        let lines: Vec<&str> = text.lines().collect();
        let start = find_body_start(&lines);
        assert!(
            lines[start].contains("初、哉"),
            "起点没落在经文上：{}",
            lines[start]
        );
    }

    #[test]
    fn body_start_returns_zero_when_nothing_looks_classical() {
        // 全书都是白话：认不出就不动，绝不乱砍。
        let lines = vec!["本书由某某出版社出版", "第一章　导论", "我们讨论了这个问题"];
        assert_eq!(find_body_start(&lines), 0);
    }

    #[test]
    fn strip_pub_lines_keeps_the_rest() {
        let text = "尔雅\n全本全注全译丛书\n管锡华◎译注\n释诂第一\n初、哉，始也。";
        let out = strip_pub_lines(text);
        assert!(!out.contains("译注"), "译注未删：{out}");
        assert!(!out.contains("丛书"), "丛书名未删：{out}");
        assert!(out.contains("释诂第一"), "正文被误删：{out}");
        assert!(out.contains("初、哉，始也。"), "正文被误删：{out}");
    }

    #[test]
    fn junk_section_needs_both_conditions() {
        let junk = "尔雅\n全本全注全译丛书\n管锡华◎译注";
        assert!(is_junk_section(junk, 200, 2), "垃圾节漏判");
        // 字数够多 → 放行（正文里夹着征引不算垃圾）
        let body = "初、哉、首、基、肈、祖、元、胎、俶、落、权舆，始也。\n".repeat(30);
        assert!(!is_junk_section(&body, 200, 2), "正文被当垃圾");
        // 噪声不足 → 放行
        assert!(!is_junk_section("尔雅\n释诂第一", 200, 2), "噪声不足也杀了");
    }

    #[test]
    fn visible_chars_ignores_whitespace() {
        assert_eq!(visible_chars("尔　雅\n　译注 "), 4);
    }
}
