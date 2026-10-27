//! **清洗器**：所有格式剥壳之后的共同一段路——两遍扫描，落到 IR。
//!
//! 为什么是两遍，不是一堆正则：
//!   * **第一遍 · 去噪**：只做「减法」——删 script/style 残留、URL、页码、
//!     翻页导航、版权广告、连续重复段、零宽字符。这一步不产生任何结构。
//!   * **第二遍 · 重建语义**：把剩下的碎片「升格」成 IR 的语义节点——
//!     标题层级（卷 / 章 / 小节）、夹注、异体字归一。**加法在这一步**。
//!
//! 分开的意义：减法可以无条件对所有格式生效（谁都不想看到广告），
//! 加法必须看来源——EPUB / MD / HTML 自带结构，再启发式切章只会把人家
//! 好好的目录切碎；只有 TXT / DOCX 这类「没有结构」的才需要重建。
//! 判据见 [`IrSource::has_structure`]。
//!
//! ⚠️ 铁律：本模块**不得**引入 GPUI / 平台类型；清洗只在**导入时**跑一次，
//! 不擅自覆盖用户已经改过的书稿。

use crate::ir::{IrBlock, IrBook, IrJuan, IrSource, IrSpan};
use crate::pipeline::RawChapter;
use crate::types::Book;

// ------------------------------------------------------------------
// 零、开关
// ------------------------------------------------------------------

/// 清洗开关（默认全开）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanOpts {
    /// 第一遍：去噪。
    pub denoise: bool,
    /// 第二遍：重建标题层级（卷 / 章 / 小节）。
    pub structure: bool,
    /// 第二遍：括号夹注识别。
    pub notes: bool,
    /// 第二遍：异体字 / 私造字归一。
    pub variants: bool,
}

impl Default for CleanOpts {
    fn default() -> Self {
        Self {
            denoise: true,
            structure: true,
            notes: true,
            variants: true,
        }
    }
}

impl CleanOpts {
    /// 只去噪、不动结构（给「用户已经手工整理好的书稿」用）。
    pub fn denoise_only() -> Self {
        Self {
            denoise: true,
            structure: false,
            notes: false,
            variants: false,
        }
    }
}

// ------------------------------------------------------------------
// 一、第一遍：去噪
// ------------------------------------------------------------------

/// 噪声关键词（整段命中且够短才算——长文里提到「版权所有」是正文）。
const NOISE_HINTS: &[&str] = &[
    "版权所有",
    "ISBN",
    "ISBN号",
    "All rights reserved",
    "all rights reserved",
    "©",
    "广告",
    "Advertisement",
    "advertisement",
    "扫码",
    "关注微信",
    "微信公众号",
    "手机阅读",
    "请记住本站",
    "免费阅读",
    "本章未完",
    "未完待续",
    "请点击",
    "點擊",
    "更多精彩",
    "本章免费",
    "加入书签",
    "加入書籤",
    "推荐票",
    "推薦票",
];

/// **版权页 / 封面废话**（v1.69.0，整段命中即删）。
///
/// 中华书局「全本全注全译」这类丛书，每卷卷首都会塞一页
/// `中华经典名著全本全注全译丛书`、`管锡华译注` 这种废话；盗版 epub
/// 更狠，连 `z-library`、`1lib` 的站名都印进正文。判据是「**整段包含**
/// 且够短」——长文里提到丛书名那是正文，不是废话。
const COLOPHON_HINTS: &[&str] = &[
    "全本全注全译",
    "全本全譯全注",
    "中华经典名著",
    "中華經典名著",
    "中华经典藏书",
    "中華經典藏書",
    "中国古典名著",
    "中國古典名著",
    "国学经典",
    "國學經典",
    "国学典藏",
    "國學典藏",
    "丛书编委会",
    "叢書編委會",
    "图书在版编目",
    "圖書在版編目",
    "责任编辑",
    "責任編輯",
    "封面设计",
    "封面設計",
    "版式设计",
    "排版制作",
    "侵权必究",
    "侵權必究",
    "版权合同",
    "著作權",
    "著作权",
    "出版社",
    "出版发行",
    "出版發行",
    "印刷",
    "字数：",
    "字數：",
    "开本：",
    "開本：",
    "定价：",
    "定價：",
    "印张",
    "印張",
    "z-library",
    "z-lib",
    "1lib",
    "zlibrary",
    "libgen",
];

/// **凡例 / 图例里的并列标签词组**（v1.69.0）：`书名、地名、人名、夹注`。
///
/// 古籍 pdf/epub 转出来的本子，卷首常有一页「凡例」：
/// `书名　地名　人名　夹注　校记`——既无句读也不是正文，排进竖排就是
/// 一列莫名其妙的词。
///
/// ⚠️ **宁可漏判、绝不可误杀**：古籍整理本的正文里也有顿号列举
/// （`仁、义、礼、智、信`），一刀切会连正文一起吃掉。故判据是五条
/// **同时**成立，且必须出现「标签味」的词尾 / 关键词：
///   ① 全长 ≤ 24 字；② 通篇无句读（。，！？；：）；
///   ③ 按顿号 / 空白切成 **≥3 段**；④ 每段 **1~4 字**；
///   ⑤ 至少一段以「名 注 记 号 例 文 字 词 符 标 点 线」收尾，
///      或整行含 `夹注` / `书名` / `括号` 这类明显的标签词。
pub fn is_tag_list_line(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > 24 {
        return false;
    }
    // ⑤-a 一眼就是标签词的，直接判（这是用户点名的那一类）。
    for kw in ["夹注", "夾註", "书名号", "書名號", "着重号", "專名號", "专名号", "校记", "校記"] {
        if t.contains(kw) {
            return true;
        }
    }
    // ② 有句读就是正文（哪怕是短句）。
    if t.chars()
        .any(|c| matches!(c, '。' | '，' | '！' | '？' | '；' | '：' | '.' | ',' | '!' | '?'))
    {
        return false;
    }
    // ③ 按顿号 / 空白切段。
    let parts: Vec<&str> = t
        .split(|c: char| c == '、' || c == ',' || c.is_whitespace() || c == '　')
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 3 {
        return false;
    }
    // ④ 每段都得是短词。
    if !parts
        .iter()
        .all(|p| (1..=4).contains(&p.chars().count()))
    {
        return false;
    }
    // ⑤-b 至少一段带「标签味」的词尾。
    parts.iter().any(|p| {
        p.chars()
            .next_back()
            .map(|c| {
                matches!(
                    c,
                    '名' | '注' | '記' | '记' | '號' | '号' | '例'
                        | '文' | '字' | '詞' | '词' | '語' | '语'
                        | '符' | '標' | '标' | '點' | '点' | '線' | '线'
                )
            })
            .unwrap_or(false)
    })
}

/// 翻页 / 导航残留（**整段等于**这些词才删，避免误伤正文里的「目录」二字）。
const NAV_HINTS: &[&str] = &[
    "上一页",
    "下一页",
    "上一章",
    "下一章",
    "返回目录",
    "返回顶部",
    "回到顶部",
    "首页",
    "首頁",
    "上一節",
    "下一節",
    "加载中",
    "載入中",
];

/// 这一段是不是噪声（页码 / 网址 / 广告 / 翻页导航）。
pub fn is_noise_para(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return true;
    }
    let n = t.chars().count();
    // ① 页码：纯数字 / 罗马数字 / 「- 12 -」
    let mut digits = 0usize;
    let mut others = 0usize;
    for c in t.chars() {
        if c.is_ascii_digit() || ('０'..='９').contains(&c) {
            digits += 1;
        } else if matches!(c, '-' | '—' | '·' | '.' | ' ') {
            // 页码常写成 `- 12 -`
        } else {
            others += 1;
        }
    }
    if digits > 0 && others == 0 && n <= 8 {
        return true;
    }
    if n <= 7 && t.chars().all(|c| matches!(c, 'i' | 'I' | 'v' | 'V' | 'x' | 'X' | 'l' | 'L')) {
        return true;
    }
    // ② 网址 / 邮箱
    if ["http://", "https://"].iter().any(|s| t.contains(s)) || t.starts_with("www.") {
        return true;
    }
    let tld = [".com", ".cn", ".net", ".org"];
    if tld.iter().any(|s| t.contains(s)) && t.chars().any(|c| c == '/' || c == '.') && n <= 60 {
        return true;
    }
    if n <= 40 && t.contains('@') && !t.contains(' ') && t.contains('.') {
        return true;
    }
    // ③ 版权 / 广告
    if n <= 40 {
        for kw in NOISE_HINTS {
            if t.contains(kw) {
                return true;
            }
        }
    }
    // ④ 翻页导航（整段等于）
    for kw in NAV_HINTS {
        if t == *kw {
            return true;
        }
    }
    // ⑤ 版权页 / 封面废话（v1.69.0）：「中华经典名著全本全注全译丛书」
    // 「管锡华译注」这类印在卷首的出版信息，绝不是正文。
    if n <= 30 {
        for kw in COLOPHON_HINTS {
            if t.contains(kw) {
                return true;
            }
        }
        // 「管锡华译注」「王晓玮 译注」「陈某 著」这种署名行——短、无句读、
        // 以译注 / 著 / 撰 / 点校收尾。单字的「注」「译」不算（正文里常见）。
        let tail = ["译注", "譯註", "譯注", "译註", "点校", "點校", "校点", "編譯", "编译", "著", "撰"];
        if n <= 12 && tail.iter().any(|w| t.ends_with(w)) && !t.contains('。') {
            return true;
        }
    }
    // ⑥ 并列标签词组（v1.69.0）：「书名、地名、人名、夹注」这种凡例行。
    if is_tag_list_line(t) {
        return true;
    }
    // ⑦ 汉字叶码（v1.68.1）：「十三」「六」这种漏网的汉字页码，绝不能独占一列。
    is_lone_number_para(t)
}

/// 单段文本去噪：零宽字符、HTML 残留、实体、空白压缩，以及
/// **PUA 私用区 / 增补私用区 → 「□」空心方块**的防崩溃兜底。
///
/// **不动** 笺读自己的 `[注]` / `[拼音:]` 记号——它们不含 `<`，不会被
/// 剥标签那一步碰到。也**不替换**生僻但可渲染的字（CJK 扩展 A/B、康熙部首等）：
/// 那些交给字体回退（主字缺字时系统大字库 / 用户沙盒字库兜底），且只进
/// `rare_chars` 的扫描清单提醒配字典——改一个字就等于篡改古籍，绝不越界。
///
/// 唯独 **PUA 私用区**（BMP `E000–F8FF` + 增补私用区 `F0000+` / `100000+`）
/// 任何字库都不会有字形、也无从安装字体补齐：它们几乎全是坏转换留下的私造
/// 占位，漏进排版引擎只会吐一串大小不一的系统豆腐块，甚至有触发文本 shaping
/// 崩溃的险。故一律换成古风占位的「□」（U+25A1，通用字库都有，绝不会再缺字），
/// 连续多个乱码压成一个方块，**绝对不许 Panic**。
/// **删掉连续拉丁字母（拼音）与多余英文空格**（v1.69.0）。
///
/// 古籍正文里不该出现成串的拼音字母（`dào kě dào`）——竖排引擎会把它们
/// **一个字母一格**竖着排下去，读者看到的是一列天书。规则：
///
/// * **连续 ≥2 个拉丁字母** → 整段删（拼音必然连成串）；
/// * **单个字母** → 保留（真的西文单词里它是词的一部分，占一格，不会
///   被拆成单字母竖排）；
/// * 连续的英文空格 → 折成一个；删完字母留下的空洞空格也一并收干净；
/// * `[拼音:dào]` / `[注]…` 这类自家标记**整段原样放行**（标记不是正文，
///   而且注音还得靠它）。
/// 是不是**拉丁字母**（含带调符号的拼音字母）。
///
/// 拼音几乎都带调号（`dào kě`、`lǘ`、`ḿ`），只认 ASCII 字母会漏掉
/// 整串——`is_ascii_alphabetic()` 对 `à` 返回 false，一串拼音就被拆成
/// 一个个「单字母」全放过去了。这里把 Latin-1 补充 / Latin 扩展 A·B /
/// Latin 扩展附加区一并算作字母。
fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
        || ('\u{00C0}'..='\u{024F}').contains(&c)
        || ('\u{1E00}'..='\u{1EFF}').contains(&c)
}

pub fn strip_pinyin_runs(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let n = cs.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    let mut space = false;
    while i < n {
        // 自家标记整段放行（`[…]` 里的内容一个字母不动）。
        if cs[i] == '[' {
            if let Some(end) = (i + 1..n).find(|&j| cs[j] == ']') {
                for &c in &cs[i..=end] {
                    out.push(c);
                }
                i = end + 1;
                space = false;
                continue;
            }
        }
        let c = cs[i];
        if is_latin_letter(c) {
            // 数一数这串字母有多长。
            let mut j = i;
            while j < n && is_latin_letter(cs[j]) {
                j += 1;
            }
            // 只留「单字母」：那是一个词里的一格，不是拼音串。
            if j - i == 1 {
                out.push(c);
                space = false;
            }
            // 拼音串（≥2）：整段丢；它前面若留着空格，交给下面的空格折叠。
            i = j;
            // 一串拼音后面可能跟着数字（声调 `dào3`）：一并吃掉。
            while i < n && cs[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        }
        if c == ' ' || c == '\u{00A0}' {
            if !space {
                out.push(' ');
                space = true;
            }
            i += 1;
            continue;
        }
        out.push(c);
        space = false;
        i += 1;
    }
    out.trim().to_string()
}

pub fn denoise_text(s: &str) -> String {
    let mut s = s.to_string();
    // HTML 残留（清洗漏网的 `<span>`、`&nbsp;`）：先剥标签再解实体。
    if s.contains('<') && s.contains('>') {
        s = crate::pipeline::strip_tags(&s);
    }
    if s.contains('&') {
        s = crate::pipeline::decode_entities(&s);
    }
    // v1.69.0：**拼音（连续拉丁字母）一律删**。开源 JSON 的 `pinyin` 字段
    // 一旦混进正文，竖排引擎会把它**一个字母一格**竖着排成一串天书。
    // 单个字母保留（真有西文单词时它是一个完整词，占一格）。
    // 自家标记 `[拼音:dào]` / `[注]…` 里的内容受保护，一个字母不少。
    s = strip_pinyin_runs(&s);
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    let mut tile = false; // 上一个输出已是「□」：连续私用/乱码只留一个方块
    for c in s.chars() {
        match c {
            // 零宽 / BOM / 软连字符 / 方向控制符：一律删（竖排里全是空白格）。
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{0000}'..='\u{0008}'
            | '\u{000E}'..='\u{001F}' => {}
            // PUA 私用区（BMP + 增补 A/B）：任何字库都无此字形 → 「□」占位，
            // 连续多个压成一个，绝不把无法渲染的私造字漏进排版引擎。
            '\u{E000}'..='\u{F8FF}'
            | '\u{F0000}'..='\u{FFFFD}'
            | '\u{100000}'..='\u{10FFFD}' => {
                if !tile {
                    out.push('\u{25A1}');
                    tile = true;
                }
                space = false;
            }
            // 各种空白统一成一个半角空格（段内换行先并成空格）。
            '\u{00A0}' | '\u{3000}' | '\t' | '\r' | '\n' | ' ' => {
                if !space {
                    out.push(' ');
                    space = true;
                }
                tile = false;
            }
            _ => {
                out.push(c);
                space = false;
                tile = false;
            }
        }
    }
    out.trim().to_string()
}

// ── 终极头部清洗（v1.68.1）────────────────────────────────────────────
/// **段首缩进清零**：把一段开头的所有「看不见 / 占位」字符全部剔除——
/// 全角空格 `　`、半角空格、制表符、不换行空格、零宽字符、BOM。
///
/// 竖排排版里这些字符**每一个都占一个正文字格**（`　` 尤其，它是全角，
/// 会画成一整格的空白），于是「段首缩进两格」在版面上就是第一列顶部凭空
/// 多出两格空当——正是用户实测到的「幽灵空行」。段首一律清零，第一个字
/// 直接贴住版框上沿。
pub fn trim_para_head(s: &str) -> String {
    let invisible = |c: char| {
        matches!(c, '\u{3000}' | '\u{00A0}' | '\t')
            || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{FEFF}')
    };
    // 头部一路吃掉（可能是一串 `　　` 或 `　 ` 混排）。
    let t = s.trim_start_matches(|c: char| invisible(c) || c == '\r' || c == '\n');
    // **半角空格只在「一眼就是缩进」时才吃**（连续 ≥4 个）。
    //
    // 行首一两个半角空格多半不是缩进，而是西文 / 数字的**换行续行**
    // （`1.003␣004` 折成两行时第二行带一个空格）——吃掉它就是把两个词黏成
    // 一个，那是**在改内容**，绝不能干。何况真正的段首空白在下游本来就会被
    // `line.trim()` 吃掉（txt / epub 两条链都是按行 trim 的），这里再狠一点
    // 只会伤到「续行中间的那个空格」。
    let sp = t.chars().take_while(|&c| c == ' ').count();
    let t = if sp >= 4 { &t[sp..] } else { t };
    // 尾部只清空白与不可见，不动标点。
    t.trim_end_matches(|c: char| invisible(c) || c == ' ' || c == '\r' || c == '\n')
        .to_string()
}

/// **折叠幽灵空行**：连续 3 个以上换行（中间夹着空格 / 全角空格 / 制表符
/// 也算）一律折成 2 个——也就是「至多一个空行」。
///
/// 排版侧绝不会因此看到空段：空段在 [`pass1_juan`] 里就被 `retain` 掉了，
/// 更不可能被当成一列排在最前面。
fn collapse_blank_runs(s: &str) -> String {
    // 按行走，只裁决「空行」：**行内容一个字都不动**——
    // 续行前导的那个空格是内容（`1.003␣004` 折行时第二行的空格），
    // 顺手吃掉就把两个词黏成一个了。
    let mut out = String::with_capacity(s.len());
    let mut blank = 0usize; // 当前连续空行数
    for line in s.split('\n') {
        if line.trim().is_empty() {
            blank += 1;
            // 至多留一个空行（= 至多两个换行）。
            if blank == 1 {
                out.push('\n');
            }
            continue;
        }
        blank = 0;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

/// **剥离 YAML / TOML 式元数据头**（v1.68.1）。
///
///  notion / Obsidian / Jekyll 导出的 md / txt 常以
/// ```text
/// ---
/// title: 庄子
/// author: 某人
/// ---
/// ```
/// 开头。这整块一旦流进正文，第一列顶部就是一串 `title:` `author:`，
/// 更是把真正的首字挤下去。这里整块连壳剥掉，一个字都不留。
///
/// 判据保守：开头（略过不可见字符后）必须是 `---`，且在随后的
/// [`YAML_SCAN_LINES`] 行内找得到收口的 `---` / `...`；找不到就**不动**
/// （免得把正文里的分隔线当元数据）。
const YAML_SCAN_LINES: usize = 60;
pub fn strip_yaml_front_matter(s: &str) -> String {
    let head = s.trim_start_matches(|c: char| {
        matches!(c, '\u{FEFF}' | '\u{200B}'..='\u{200F}' | '\u{2060}') || c.is_whitespace()
    });
    if !head.starts_with("---") {
        return s.to_string();
    }
    let mut lines = head.lines();
    lines.next(); // 开闸那一行
    let mut seen = 0usize;
    for l in lines {
        seen += 1;
        let t = l.trim();
        if t == "---" || t == "..." {
            // 收口：整块（含收口行）剥掉，返回其后内容。
            let rest = &head[..];
            // 用「行号」定位开销大，这里按字节数直接切：head 里从第
            // (seen+1) 行起就是正文。
            return rest
                .lines()
                .skip(seen + 1)
                .collect::<Vec<_>>()
                .join("\n");
        }
        if seen >= YAML_SCAN_LINES {
            break;
        }
    }
    s.to_string()
}

/// **终极头部清洗**：原文进解析器之前的一刀（v1.68.1，用户四条红线）。
///
/// 1. 剥 YAML / TOML 元数据头；
/// 2. 折叠连续 3+ 空行为至多 1 个空行；
/// 3. 逐行做 **段首缩进清零**（全角空格 / 空格 / 零宽 / BOM 全剔）；
/// 4. 全文再强力 `trim()`——开头的 BOM、零宽、全角空格、多余换行一律
///    滚蛋，保证交给排版引擎的第一个字符就是**真字**。
///
/// 只碰「文本 → 章」这一段：不改分页、不改锚点、不改字格账。
pub fn strip_head_junk(text: &str) -> String {
    let s = strip_yaml_front_matter(text);
    let s = collapse_blank_runs(&s);
    let s: String = s.lines().map(trim_para_head).collect::<Vec<_>>().join("\n");
    s.trim_start_matches(|c: char| {
        matches!(c, '\u{FEFF}' | '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{3000}' | '\u{00A0}')
            || c == '\n'
            || c == '\r'
            || c == ' '
            || c == '\t'
    })
    .to_string()
}

// ── 注疏标签：只去壳，留肉身（v1.68.1）─────────────────────────────────
/// **只去壳、留肉身**：把形态各异的注疏标签**规范化**成笺读自己的
/// `[注] … [/注]` 夹注壳，**注疏正文一个字都不删**。
///
/// 处理的形态（大小写 / 空格 / 全角一律认）：
///   * `[注]` `[ 注 ]` `[　注　]` `[注 ]` → `[注]`
///   * `[/注]` `[ /注 ]` `[／注]` → `[/注]`
///   * `[疏]` `[ 疏 ]` → `[注]`；`[/疏]` → `[/注]`（疏也是注）
///   * `[高亮]` `[ 高亮 ]` → `[高亮]`（着重号同理）
///   * **空壳**（`[注][/注]`、只有壳没有肉）→ 连壳删掉，不留空标记
///   * **不认识的方括号**（`[12]`、`[一]`、网页残渣）→ **只削掉方括号**，
///     里头的字原样留下（这就是「只去壳」的边界：绝不连内容一起吞）
///   * 落单的 `[` / `]` → 直接删
///
/// 反过来，**合法壳之间的内容 100% 保留**：
/// `正文[注]注疏内容[/注]正文` → `正文[注]注疏内容[/注]正文`（原样），
/// 交给排版层落成 `InlineNote`（双行小字夹注），正文里不出现方括号。
pub fn normalize_note_shells(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '[' {
            if chars[i] != ']' {
                out.push(chars[i]);
            }
            i += 1;
            continue;
        }
        // 找收口的 `]`（最多向前看 40 字：标签不会更长）。
        let close = (i + 1..chars.len().min(i + 41)).find(|&j| chars[j] == ']');
        let Some(c) = close else {
            i += 1; // 没有收口：这个 `[` 是垃圾，丢掉（内容照常往后走）
            continue;
        };
        let inner: String = chars[i + 1..c].iter().collect();
        // 归一化：去空格 / 全角空格 / 全角斜杠 → 半角，再判是不是我们的壳。
        let key: String = inner
            .chars()
            .filter(|ch| !matches!(ch, ' ' | '\t' | '\u{3000}' | '\u{00A0}'))
            .map(|ch| if ch == '／' { '/' } else { ch })
            .collect();
        let low = key.to_lowercase();
        match low.as_str() {
            "注" | "疏" => out.push_str("[注]"),
            "/注" | "/疏" => out.push_str("[/注]"),
            "高亮" => out.push_str("[高亮]"),
            "/高亮" => out.push_str("[/高亮]"),
            // 带参数的自家标记（`[拼音:xxx]` / `[数字:xxx]`）连壳原样放行：
            // 它们的内部是**语法**（冒号是分隔符），下游 `paragraph_of` /
            // `strip_digit_marks` 还要靠这对壳认出它，削了壳就断成正文了。
            _ if low.starts_with("拼音:") || low.starts_with("拼音：") => {
                out.push_str(&format!("[{}]", key));
            }
            _ if low.starts_with("数字:") || low.starts_with("数字：") => {
                out.push_str(&format!("[{}]", key));
            }
            _ => {
                // 不认识的壳：**只削括号，留肉身**。
                out.push_str(inner.trim());
            }
        }
        i = c + 1;
    }
    // 空壳连壳删：`[注][/注]` 这种没有肉身的直接消失。
    let out = out.replace("[注][/注]", "");
    out.replace("[高亮][/高亮]", "")
}

/// **取「纯正文」**：把所有注疏 / 着重 / 注音壳连同方括号一起抹掉，
/// **壳里的字一个不少地留下**。
///
/// 用途：导出纯文本、以及单元测试钉「只去壳不留壳、肉身 100% 在」这条边界。
/// 导入管线本身**不**调用它——管线要保留 `[注]…[/注]` 好让排版层落成
/// `InlineNote`（双行小字夹注），抹了就没有夹注了。
pub fn note_content_only(s: &str) -> String {
    let mut cur = normalize_note_shells(s);
    // 反复抹壳（抹掉一对之后可能又挨上一对）。
    for tag in ["[注]", "[/注]", "[高亮]", "[/高亮]"] {
        loop {
            match cur.find(tag) {
                Some(at) => cur = format!("{}{}", &cur[..at], &cur[at + tag.len()..]),
                None => break,
            }
        }
    }
    // `[拼音:xxx]`：壳删、拼音留。
    loop {
        let Some(at) = cur.find("[拼音") else { break };
        let Some(end) = cur[at..].find(']') else { break };
        let inner = cur[at + "[拼音".len()..at + end]
            .trim_start_matches(|c| c == ':' || c == '：')
            .to_string();
        cur = format!("{}{}{}", &cur[..at], inner, &cur[at + end + 1..]);
    }
    cur.replace(['[', ']'], "")
}

/// **剥离落单的书名号**（v1.68.1）：跨行 / 跨段被截断的《》直接删掉。
///
/// 正常成对出现的《…》不动（它们会被 [`crate::vclean`] 转成竖排的 ︽︾，
/// 渲染时作悬挂标点，不占字格中轴）；只有**找不到另一半**的才删——
/// 段首落单的 `》`、段尾落单的 `《`，以及全段里多出来的那半个。
pub fn strip_orphan_title_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut open = 0i32;
    for c in s.chars() {
        match c {
            '《' => {
                open += 1;
                out.push(c);
            }
            '》' => {
                if open > 0 {
                    open -= 1;
                    out.push(c);
                }
                // 没有待闭的《：这是个落单的 》，丢掉。
            }
            _ => out.push(c),
        }
    }
    if open > 0 {
        // 段尾有多余的《：从后往前删掉多出来的那几个。
        let mut extra = open;
        let mut keep: Vec<char> = Vec::with_capacity(out.len());
        for c in out.chars().rev() {
            if c == '《' && extra > 0 {
                extra -= 1;
                continue;
            }
            keep.push(c);
        }
        out = keep.into_iter().rev().collect();
    }
    out
}

/// **孤立的页码 / 页眉残留**（v1.68.1）：整段只由「汉字数字 / 阿拉伯数字 /
/// 〇零廿卅」构成、且不带任何句读 → 这是漏网的叶码，绝不能独占一列。
///
/// 例：`十三`、`六`、`四十二`、`１２`。带上下文的（「卷十三」「十三年」）
/// 有其它汉字，不会被判成页码，安全。
pub fn is_lone_number_para(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > 8 {
        return false;
    }
    let is_num = |c: char| {
        c.is_ascii_digit()
            || ('０'..='９').contains(&c)
            || matches!(
                c,
                '〇' | '零' | '一' | '二' | '三' | '四' | '五'
                    | '六' | '七' | '八' | '九' | '十' | '百' | '千' | '廿' | '卅' | '两'
            )
    };
    t.chars().all(is_num)
}

/// **页眉式章节元数据**（v1.68.1）：`尔雅 [ 注 ] 二`、`庄子 卷三` 这种
/// 原书页眉——短、无句读、且带着书名或「注 / 疏」字样。
///
/// 判据三条同时成立才算：**≤ 16 字** + **通篇无句读** + **含书名，或含
/// 「注 / 疏」标记**。识别出来就从正文流里摘掉（它是 TOC 用的元数据，不是
/// 正文），免得「尔雅 注 二」这种串混进第一列。
///
/// ⚠️ **宁可漏判，不可误杀**：`卷 / 篇 / 章 / 回` 这几枚**不算**证据——
/// 正文里「卷三」「篇上」的短行太多了，凭它们删段是真删内容。只有「书名
/// 出现在无句读的短行里」（`庄子 卷三`）或「带着注疏标记」（`尔雅[注]二`）
/// 才动手——这两种要么本来就是页眉，要么注疏内容已由 `[注]` 壳保住了。
pub fn is_running_header(s: &str, book_title: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > 16 {
        return false;
    }
    if t.chars().any(|c| matches!(c, '。' | '，' | '！' | '？' | '；' | '：' | '、')) {
        return false;
    }
    let has_mark = t.chars().any(|c| matches!(c, '注' | '疏'));
    let has_book = !book_title.is_empty()
        && t.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .contains(book_title.trim());
    has_mark || has_book
}

/// 第一遍：一巻内的去噪（文本净化 + 丢噪声段 + 丢页眉 / 叶码 + 合并连续重复段
/// + 砍导航短段堆）。
///
/// `book_title` 用来认页眉（`庄子 卷三`）：书名出现在无句读短行里，那行就是
/// 元数据不是正文。传空串即关闭这条判据（只留「注 / 疏」那条）。
fn pass1_juan(j: &mut IrJuan, titles: &std::collections::HashSet<String>, book_title: &str) {
    for b in j.blocks.iter_mut() {
        match b {
            IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
                for sp in spans.iter_mut() {
                    match sp {
                        IrSpan::Text(t) => *t = denoise_text(t),
                        IrSpan::Ruby { base, rt } => {
                            *base = denoise_text(base);
                            *rt = denoise_text(rt);
                        }
                        IrSpan::Note { text, .. } => *text = denoise_text(text),
                    }
                }
            }
            IrBlock::HeadNote(t) => *t = denoise_text(t),
            IrBlock::SubHead { text, .. } => *text = denoise_text(text),
        }
    }
    // 丢噪声段（含净化后变空的段）+ 丢页眉 / 汉字叶码（v1.68.1）
    j.blocks.retain(|b| match b {
        IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
            let t = spans_line(spans);
            !t.trim().is_empty() && !is_noise_para(&t) && !is_running_header(&t, book_title)
        }
        IrBlock::HeadNote(t) => !t.trim().is_empty(),
        IrBlock::SubHead { text, .. } => !text.trim().is_empty(),
    });
    // 连续完全相同的段：只留一段（网页把同一段复制两遍是常态）
    let mut dedup: Vec<IrBlock> = Vec::with_capacity(j.blocks.len());
    for b in std::mem::take(&mut j.blocks) {
        let same = dedup
            .last()
            .map(|prev: &IrBlock| prev.plain_text() == b.plain_text())
            .unwrap_or(false);
        if !same {
            dedup.push(b);
        }
    }
    j.blocks = dedup;
    // 导航短段堆：连续 ≥3 段、每段 ≤6 字、无句读，且这些段在别处是标题
    // （典型的「目录残页」）。诗歌短句不会被误伤——它们不会连续出现在
    // 卷的标题表里的。
    drop_nav_runs(&mut j.blocks, titles);
}

fn spans_line(spans: &[IrSpan]) -> String {
    let mut s = String::new();
    for sp in spans {
        match sp {
            IrSpan::Text(t) => s.push_str(t),
            IrSpan::Ruby { base, .. } => s.push_str(base),
            IrSpan::Note { kind, text } => {
                if kind.is_span_mark() {
                    s.push_str(text);
                }
            }
        }
    }
    s
}

/// 一刀砍掉「目录残页」式的短段堆。
fn drop_nav_runs(blocks: &mut Vec<IrBlock>, titles: &std::collections::HashSet<String>) {
    if blocks.len() < 3 {
        return;
    }
    let nav_like = |b: &IrBlock| -> bool {
        let t = b.plain_text();
        let n = t.chars().count();
        n > 0 && n <= 6 && !t.chars().any(is_sentence_punct) && titles.contains(&t)
    };
    let mut out: Vec<IrBlock> = Vec::with_capacity(blocks.len());
    let mut run: Vec<IrBlock> = Vec::new();
    let flush = |run: &mut Vec<IrBlock>, out: &mut Vec<IrBlock>| {
        if run.len() >= 3 {
            run.clear(); // 连续三段以上是导航，整堆丢
        } else {
            out.append(run);
        }
    };
    for b in blocks.drain(..) {
        if nav_like(&b) {
            run.push(b);
        } else {
            flush(&mut run, &mut out);
            out.push(b);
        }
    }
    flush(&mut run, &mut out);
    *blocks = out;
}

/// 句读（**句内停顿 / 句末**那几个）：注文里可以有冒号、引号、书名号，
/// 但有一读（，、。）就说明它是正文的一句话，不是注。
fn is_sentence_punct(c: char) -> bool {
    matches!(c, '。' | '，' | '、' | '！' | '？' | '；')
}

// ------------------------------------------------------------------
// 二、第二遍：重建语义
// ------------------------------------------------------------------

/// 标题层级判定：返回 `1` = 卷（开新巻），`2` = 章（开新巻，目录二级），
/// `3` = 章内小节。`None` = 不是标题。
///
/// 全靠字符扫描（core 不引正则 crate），认的模式：
///   * `第X章/回/卷/节/篇/折/出/幕/集/部/编/則/则`
///   * `卷X` `巻X` `卷之X` `卷第X` `卷上/中/下` `上篇/中篇/下篇`
///   * `Chapter 12` `CHAPTER IV` `PART I`
///   * 序跋类：`序 / 叙 / 自序 / 前言 / 引 / 跋 / 后记 / 附录 / 凡例 / 楔子 / 例言 / 题辞`
///   * 编号小节：`一、` `（一）` `1.1` `第一节`（已含于第X节）
pub fn heading_level(line: &str) -> Option<u8> {
    let t = line.trim();
    let n = t.chars().count();
    if n == 0 || n > 40 {
        return None;
    }
    // ① 第 X 章 / 回 / …
    if crate::pipeline::is_chapter_head(t) {
        return Some(2);
    }
    // ② 卷：卷一 / 卷之三 / 卷第三 / 卷上 / 巻二
    if let Some(level) = juan_level(t) {
        return Some(level);
    }
    // ③ 西文：Chapter 12 / CHAPTER IV / Part One
    let up = t.to_ascii_uppercase();
    for kw in ["CHAPTER", "PART", "BOOK", "VOLUME", "SECTION"] {
        if let Some(rest) = up.strip_prefix(kw) {
            let rest = rest.trim_start_matches([' ', '.', ':']);
            if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return Some(2);
            }
            // Chapter One / Part Two 之类：后面是英文数词
            if rest.chars().count() <= 12 && rest.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
            {
                return Some(2);
            }
        }
    }
    // ④ 序跋 / 附录类：卷级
    if n <= 12 {
        for kw in [
            "序", "叙", "自序", "序文", "前言", "引言", "弁言", "例言", "凡例", "题辞", "題辭",
            "跋", "後記", "后记", "附录", "附錄", "楔子", "缘起", "緣起", "总序", "總序", "译序",
            "譯序",
        ] {
            if t == kw {
                return Some(1);
            }
        }
    }
    // ⑤ 编号小节：一、 / （一） / 1.1 / 1、
    if let Some(level) = numbered_head(t) {
        return Some(level);
    }
    None
}

/// `卷X` 系列 → 卷级（1）。
fn juan_level(t: &str) -> Option<u8> {
    let cs: Vec<char> = t.chars().collect();
    let mut i = if cs[0] == '卷' || cs[0] == '巻' {
        1usize
    } else {
        return None;
    };
    // 「卷之三」的「之」跳过
    if cs.get(i) == Some(&'之') || cs.get(i) == Some(&'第') {
        i += 1;
    }
    let mut digits = 0usize;
    while i < cs.len() && (is_cn_numeral(cs[i]) || cs[i].is_ascii_digit() || ('０'..='９').contains(&cs[i])) {
        digits += 1;
        i += 1;
    }
    if digits > 0 {
        // 「卷一 某某」：后面还能跟标题名，但总长已限 40
        return Some(1);
    }
    // 卷上 / 卷中 / 卷下 / 卷首 / 卷末
    if i < cs.len() && matches!(cs[i], '上' | '中' | '下' | '首' | '末' | '甲' | '乙' | '丙' | '丁') {
        return Some(1);
    }
    None
}

fn is_cn_numeral(c: char) -> bool {
    matches!(
        c,
        '零' | '〇'
            | '一'
            | '二'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '万'
            | '萬'
            | '两'
            | '兩'
            | '廿'
            | '卅'
    )
}

/// `一、` / `（一）` / `1.1 xxx` → 小节（3）。
fn numbered_head(t: &str) -> Option<u8> {
    let cs: Vec<char> = t.chars().collect();
    // 一、二、三、（位于行首，后面紧跟顿号）
    if cs.len() >= 2 && is_cn_numeral(cs[0]) && matches!(cs[1], '、' | '．' | '.') {
        return Some(3);
    }
    // （一） / (1)
    if cs.len() >= 3 && matches!(cs[0], '（' | '(') {
        let inner = &cs[1..];
        let close = inner.iter().position(|c| matches!(c, '）' | ')'));
        if let Some(k) = close {
            if (1..=3).contains(&k) && inner[..k].iter().all(|c| is_cn_numeral(*c) || c.is_ascii_digit()) {
                return Some(3);
            }
        }
    }
    // 1.1 / 2.3.4（章号.节号）
    if cs.len() >= 3 && cs[0].is_ascii_digit() && cs[1] == '.' && cs[2].is_ascii_digit() {
        return Some(3);
    }
    None
}

/// 第二遍之一：把正文里被认出是标题的段**升格**成巻 / 小节。
///
/// 卷级（1）与章级（2）都开新巻（目录要能跳），层级写在 `IrJuan::level`；
/// 小节（3）留在巻内当 `SubHead`。
fn rebuild_headings(juans: Vec<IrJuan>) -> Vec<IrJuan> {
    let mut out: Vec<IrJuan> = Vec::new();
    let mut cur: Option<IrJuan> = None;
    for j in juans.iter() {
        // 原巻标题本身也是一条目录项，别丢：先开一个同名巻。
        let mut blocks = j.blocks.clone();
        let mut opened = false;
        for b in blocks.drain(..) {
            let t = b.plain_text();
            let level = match &b {
                IrBlock::Para(_) => heading_level(&t),
                _ => None,
            };
            match level {
                Some(1) | Some(2) => {
                    if let Some(prev) = cur.take() {
                        if !prev.is_empty() || !prev.title.trim().is_empty() {
                            out.push(prev);
                        }
                    }
                    cur = Some(IrJuan {
                        title: t.trim().to_string(),
                        level: level.unwrap_or(1),
                        blocks: Vec::new(),
                    });
                    opened = true;
                }
                Some(_) => {
                    let j = cur.get_or_insert_with(|| IrJuan {
                        title: j.title.clone(),
                        level: 1,
                        blocks: Vec::new(),
                    });
                    j.blocks.push(IrBlock::SubHead {
                        level: 3,
                        text: t.trim().to_string(),
                    });
                }
                None => {
                    let j = cur.get_or_insert_with(|| IrJuan {
                        title: j.title.clone(),
                        level: 1,
                        blocks: Vec::new(),
                    });
                    j.blocks.push(b);
                }
            }
        }
        // 这一巻没被切成新巻（通篇没有标题行）：原样留下。
        if let Some(prev) = cur.take() {
            if !opened && prev.title == j.title && !j.title.trim().is_empty() {
                out.push(j.clone());
            } else if !prev.is_empty() || !prev.title.trim().is_empty() {
                out.push(prev);
            }
        }
    }
    if out.is_empty() {
        return juans.clone();
    }
    out
}

/// 第二遍之二：括号里的短串**升格**成夹注。
///
/// 判据保守：只认全角 `（）`（现代编辑加的注释都这么写），内容 1–30 字、
/// 不含句读（含句读的说明是正文本身，不是注）、不含嵌套括号。
pub fn paren_to_note(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    let mut buf = String::new();
    for c in s.chars() {
        match c {
            '（' | '(' => {
                if depth == 0 {
                    buf.clear();
                    depth = 1;
                } else {
                    depth += 1;
                    buf.push(c);
                }
            }
            '）' | ')' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        let body = buf.trim();
                        let n = body.chars().count();
                        let ok = (1..=30).contains(&n)
                            && !body.chars().any(is_sentence_punct)
                            && !body.contains(['（', '）', '(', ')']);
                        if ok {
                            out.push_str(&format!("[注]{body}[/注]"));
                        } else {
                            out.push_str(&format!("（{body}）"));
                        }
                        buf.clear();
                        continue;
                    }
                    buf.push(c);
                } else {
                    out.push(c);
                }
            }
            _ => {
                if depth > 0 {
                    buf.push(c);
                } else {
                    out.push(c);
                }
            }
        }
    }
    // 没闭合的括号：原样吐出来，绝不吞字
    if depth > 0 {
        out.push('（');
        out.push_str(&buf);
    }
    out
}

/// 把一段正文里的括号注转成 IR 的夹注节点。
fn rebuild_notes_juan(j: &mut IrJuan) {
    for b in j.blocks.iter_mut() {
        if let IrBlock::Para(spans) = b {
            let line = spans_line(spans);
            if !line.contains('（') && !line.contains('(') {
                continue;
            }
            let marked = paren_to_note(&line);
            if marked == line {
                continue;
            }
            let mut new = crate::ir::line_to_spans(&marked);
            if !new.is_empty() {
                std::mem::swap(spans, &mut new);
            }
        }
    }
}

// ------------------------------------------------------------------
// 三、异体字 / 私造字归一
// ------------------------------------------------------------------

/// 异体字 → 正字。只收**无争议**的一批（两岸字书都认的异写），
/// 繁简转换不在这里做（那是 `trad` / `west` 的活，且由用户开关决定）。
const VARIANT_MAP: &[(char, char)] = &[
    ('峯', '峰'),
    ('峩', '峨'),
    ('羣', '群'),
    ('敎', '教'),
    ('効', '效'),
    ('卽', '即'),
    ('皁', '皂'),
    ('旣', '既'),
    ('卻', '却'),
    ('畧', '略'),
    ('冦', '寇'),
    ('厯', '历'),
    ('喆', '哲'),
    ('敍', '叙'),
    ('敡', '扬'),
    ('煑', '煮'),
    ('稾', '稿'),
    ('覩', '睹'),
    ('劒', '剑'),
    ('虵', '蛇'),
    ('壻', '婿'),
    ('聦', '聪'),
    ('茲', '兹'),
    ('卹', '恤'),
    ('袐', '秘'),
    ('甯', '宁'),
    ('氷', '冰'),
];

/// 单个字的异体归一（不在表里就原样返回）。
pub fn normalize_variant_char(c: char) -> char {
    for (from, to) in VARIANT_MAP {
        if *from == c {
            return *to;
        }
    }
    // CJK **兼容表意文字**（U+F900–U+FAFF）与私用区（U+E000–U+F8FF，私造字）
    // 没有通式可映射（要查整张 Unicode 大表）：**一律保留原字**——宁可多一个
    // 生僻字查字典落空，也绝不许把字改错。它们只进 `rare_chars` 的扫描清单。
    c
}

/// 全文本异体字归一。
pub fn normalize_variants(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        out.push(normalize_variant_char(c));
    }
    out
}

fn normalize_variants_book(ir: &mut IrBook) {
    ir.meta.title = normalize_variants(&ir.meta.title);
    ir.meta.author = normalize_variants(&ir.meta.author);
    for j in ir.juans.iter_mut() {
        j.title = normalize_variants(&j.title);
        for b in j.blocks.iter_mut() {
            match b {
                IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
                    for sp in spans.iter_mut() {
                        match sp {
                            IrSpan::Text(t) => *t = normalize_variants(t),
                            // 拼音是拉丁字母，不参与异体归一。
                            IrSpan::Ruby { base, .. } => *base = normalize_variants(base),
                            IrSpan::Note { text, .. } => *text = normalize_variants(text),
                        }
                    }
                }
                IrBlock::HeadNote(t) => *t = normalize_variants(t),
                IrBlock::SubHead { text, .. } => *text = normalize_variants(text),
            }
        }
    }
}

/// 这个字算不算「生僻 / 私造」（供后台建索引时扫一遍，好提醒用户配字典）。
///
/// 判据：CJK 扩展 A 区（U+3400+）、扩展 B 区及以上（U+20000+）、
/// 康熙部首区、兼容表意文字区、PUA 私用区。
pub fn is_rare_char(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x3400..=0x4DBF      // CJK 扩展 A
        | 0x20000..=0x323AF  // CJK 扩展 B–F
        | 0x2E80..=0x2FDF    // 部首扩展 / 康熙部首
        | 0xF900..=0xFAFF    // 兼容表意文字
        | 0xE000..=0xF8FF    // 私用区（私造字）
    )
}

/// 扫一本书的生僻字，返回（字, 出现次数），按次数降序。
///
/// 这是「导入后后台慢慢做」的那件慢事之一：不阻塞进阅读器，
/// 结果拿去提醒「这些字字典里没有，要不要配一版字体 / 注音」。
pub fn rare_chars(book: &Book, limit: usize) -> Vec<(char, usize)> {
    let mut map: std::collections::BTreeMap<char, usize> = std::collections::BTreeMap::new();
    for c in &book.chapters {
        for p in &c.paragraphs {
            for ch in p.text.chars() {
                if is_rare_char(ch) {
                    *map.entry(ch).or_insert(0) += 1;
                }
            }
        }
    }
    let mut v: Vec<(char, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(limit);
    v
}

// ------------------------------------------------------------------
// 四、主入口
// ------------------------------------------------------------------

/// **导入清洗总入口**：`Book` → IR → 两遍扫描 → 写回 `Book`。
///
/// 只换 `chapters`（+ 目录重建），封面 / 通假表 / 眉批 / 仓库 id 一律不动。
/// 洗完如果一章不剩（判据误伤），**原样退回**，绝不把书洗没了。
pub fn clean_book(book: &mut Book, source: IrSource) {
    clean_book_with(book, source, &CleanOpts::default());
}

pub fn clean_book_with(book: &mut Book, source: IrSource, opts: &CleanOpts) {
    if book.chapters.is_empty() {
        return;
    }
    let before = book.chapters.len();
    let mut ir = IrBook::from_book(book, source);
    // 题名表做成哈希集合：`drop_nav_runs` 对每个「短段」都要问一句
    // 「这段是不是别处的题名」，用 Vec 是 O(段数 × 章数) 的线性查找——
    // 《诸子百家》那类大书上，光这一句就要一秒多（v1.67.2 实测）。
    let titles: std::collections::HashSet<String> =
        ir.juans.iter().map(|j| j.title.clone()).collect();

    // ---- 第一遍：去噪（所有格式都做，减法最安全） ----
    if opts.denoise {
        // 页眉认「书名出现在无句读短行里」，书名在 `clean_book_with` 之后可能
        // 被截短（12 字），故这里取当前书名即可——长书名也只会让判据更严。
        let book_title = book.title.clone();
        for j in ir.juans.iter_mut() {
            pass1_juan(j, &titles, &book_title);
        }
        ir.juans.retain(|j| !j.is_empty() || !j.title.trim().is_empty());
    }

    // ---- 第二遍：重建语义（只在「没有结构」的来源上做加法） ----
    let unstructured = !source.has_structure();
    if opts.structure && unstructured {
        ir.juans = rebuild_headings(std::mem::take(&mut ir.juans));
    }
    if opts.notes && unstructured {
        for j in ir.juans.iter_mut() {
            rebuild_notes_juan(j);
        }
    }
    if opts.variants {
        normalize_variants_book(&mut ir);
    }

    ir.juans.retain(|j| !j.is_empty() || !j.title.trim().is_empty());
    if ir.juans.is_empty() || ir.char_count() == 0 {
        return; // 洗没了：退回原书，宁可不洗
    }
    ir.write_chapters_into(book);
    // 分章变了，旧目录下标不作数——`write_chapters_into` 已重建；
    // 这里只补一个「章数没变但内容变了」时的保险。
    if book.toc.len() != book.chapters.len() && before == book.chapters.len() {
        book.toc.clear();
        book.rebuild_toc_from_chapters();
    }
}

/// 解析器的 `RawChapter` → 清洗 → `RawChapter`（给还没建 `Book` 的链路用）。
pub fn clean_chapters(chapters: &[RawChapter], source: IrSource) -> Vec<RawChapter> {
    let ir = IrBook::from_chapters("", "", source, chapters);
    let mut book = ir.to_book();
    clean_book(&mut book, source);
    book.chapters
        .iter()
        .map(|c| {
            RawChapter::new(
                c.title.as_str(),
                c.paragraphs
                    .iter()
                    .map(crate::ir::para_to_line)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })
        .collect()
}

/// 单段文本的完整清洗（去噪 + 括号夹注 + 异体归一），给导入向导 / 命令行用。
pub fn clean_line(s: &str) -> String {
    let t = denoise_text(s);
    if t.is_empty() || is_noise_para(&t) {
        return String::new();
    }
    normalize_variants(&paren_to_note(&t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Chapter, Paragraph};

    fn book_of(chapters: Vec<(&str, Vec<&str>)>) -> Book {
        let mut b = Book::new("測試", "");
        for (t, paras) in chapters {
            b.chapters.push(Chapter {
                title: t.to_string(),
                paragraphs: paras.iter().map(|p| Paragraph::plain(*p)).collect(),
            });
        }
        b
    }

    #[test]
    fn denoise_kills_urls_pages_and_zero_width() {
        assert!(is_noise_para("123"));
        assert!(is_noise_para("- 12 -"));
        assert!(is_noise_para("https://example.com/book/1"));
        assert!(is_noise_para("版权所有，翻印必究"));
        assert!(is_noise_para("下一页"));
        assert!(!is_noise_para("道可道，非常道。"));
        assert_eq!(denoise_text("道\u{200B}可\u{200B}道"), "道可道");
        assert_eq!(denoise_text("&amp;道"), "&道");
    }

    #[test]
    fn headings_recognized() {
        assert_eq!(heading_level("卷一"), Some(1));
        assert_eq!(heading_level("卷之三"), Some(1));
        assert_eq!(heading_level("卷上"), Some(1));
        assert_eq!(heading_level("第一章 宴桃园"), Some(2));
        assert_eq!(heading_level("第十五回"), Some(2));
        assert_eq!(heading_level("Chapter 12"), Some(2));
        assert_eq!(heading_level("序"), Some(1));
        assert_eq!(heading_level("一、"), Some(3));
        assert_eq!(heading_level("（一）"), Some(3));
        assert_eq!(heading_level("1.1 小引"), Some(3));
        assert_eq!(heading_level("道可道非常道"), None);
    }

    #[test]
    fn paren_notes_become_annotations() {
        assert_eq!(paren_to_note("名可名（河上公曰：有名）非常名"), "名可名[注]河上公曰：有名[/注]非常名");
        // 含句读的不是注——那是正文自己带的括号
        assert_eq!(paren_to_note("他说（不对，其实是错的）。"), "他说（不对，其实是错的）。");
        // 没闭合：绝不吞字
        assert_eq!(paren_to_note("道（可道"), "道（可道");
    }

    #[test]
    fn txt_gets_rebuilt_into_juans() {
        let mut b = book_of(vec![(
            "正文",
            vec![
                "卷一",
                "道可道非常道名可名非常名",
                "第一章",
                "道生一一生二二生三",
                "第二章",
                "上善若水水善利万物而不争",
            ],
        )]);
        clean_book(&mut b, IrSource::Txt);
        let titles: Vec<&str> = b.chapters.iter().map(|c| c.title.as_str()).collect();
        assert!(titles.contains(&"卷一"), "卷级没切开：{titles:?}");
        assert!(titles.contains(&"第一章") && titles.contains(&"第二章"), "章级没切开：{titles:?}");
    }

    #[test]
    fn structured_sources_are_left_alone() {
        // EPUB / MD 自带结构：清洗器只做减法，不许再切章。
        let mut b = book_of(vec![("卷一", vec!["第一章", "道可道非常道"])]);
        let n = b.chapters.len();
        clean_book(&mut b, IrSource::Epub);
        assert_eq!(b.chapters.len(), n, "EPUB 不该被重建结构");
        assert_eq!(b.chapters[0].title, "卷一");
    }

    #[test]
    fn variants_normalized_once() {
        assert_eq!(normalize_variants("山峯羣敎"), "山峰群教");
        assert_eq!(normalize_variants("道德經"), "道德經");
    }

    #[test]
    fn nav_runs_dropped_but_poetry_kept() {
        // 三段短行、且都是别处的卷名 → 目录残页，砍
        let mut b = book_of(vec![
            ("卷一", vec!["道可道非常道名可名非常名"]),
            ("正文", vec!["卷一", "卷二", "卷三", "正文开始了，这一句很长很长。"]),
        ]);
        clean_book(&mut b, IrSource::Txt);
        let body: String = b
            .chapters
            .iter()
            .flat_map(|c| c.paragraphs.iter())
            .map(|p| p.text.as_str())
            .collect();
        assert!(!body.contains("卷二"), "目录残页没砍掉：{body}");
        assert!(body.contains("正文开始了"), "正文被误砍：{body}");
    }

    #[test]
    fn cleaning_never_empties_a_book() {
        let mut b = book_of(vec![("正文", vec!["123", "https://a.com", "下一页"])]);
        clean_book(&mut b, IrSource::Txt);
        assert!(!b.chapters.is_empty(), "没东西可留时也不许把书洗没");
    }

    #[test]
    fn rare_char_scan_reports_privates() {
        let b = book_of(vec![("正文", vec!["道\u{E000}可道"])]);
        let rare = rare_chars(&b, 10);
        assert!(
            rare.iter().any(|(c, _)| *c == '\u{E000}'),
            "PUA 私造字没扫出来：{rare:?}"
        );
    }

    #[test]
    fn pua_becomes_a_single_tile_but_rare_renderable_chars_survive() {
        // 单个私用区字 → 一个「□」古风占位，绝不漏进排版引擎。
        assert_eq!(denoise_text("\u{E000}"), "\u{25A1}");
        assert_eq!(denoise_text("道\u{E000}可道"), "道\u{25A1}可道");
        // 连续多个私用/乱码字压成一个方块，别在版心吐「□□□□」。
        assert_eq!(denoise_text("\u{E000}\u{E001}\u{E002}"), "\u{25A1}");
        assert_eq!(denoise_text("甲\u{E000}\u{E000}乙"), "甲\u{25A1}乙");
        // 增补私用区 A/B 同样兜底（Rust char 覆盖到 U+10FFFF）。
        assert_eq!(denoise_text("\u{F0000}"), "\u{25A1}");
        assert_eq!(denoise_text("\u{100000}"), "\u{25A1}");
        // 关键分界：生僻但**可渲染**的字（CJK 扩展 B、康熙部首）绝不替换——
        // 交给字体回退去显示真字形，改了就是篡改古籍。
        assert_eq!(denoise_text("道\u{20000}可道"), "道\u{20000}可道");
        assert_eq!(denoise_text("\u{2F00}"), "\u{2F00}"); // 康熙部首
        // 占位方块本身不是空白：段首段尾的 trim 不该吃掉它。
        assert_eq!(denoise_text("  \u{E000}  "), "\u{25A1}");
    }

    #[test]
    fn clean_line_is_idempotent() {
        let once = clean_line("道（本原）可道");
        let twice = clean_line(&once);
        assert_eq!(once, twice, "清洗不幂等：{once} -> {twice}");
    }

    // ── v1.68.1：注疏「只去壳、留肉身」的防误杀边界 ──────────────────

    /// **头号红线**：`正文[注]注疏内容[/注]正文` 洗完必须**注疏内容一字不少**，
    /// 且正文里**一个方括号都不留**。这是用户亲手指定的防误杀用例。
    #[test]
    fn note_shells_lose_only_the_shell_never_the_meat() {
        let raw = "正文[注]注疏内容[/注]正文";
        let kept = note_content_only(raw);
        assert_eq!(kept, "正文注疏内容正文", "注疏肉身被动了：{kept}");
        assert!(kept.contains("注疏内容"), "注疏内容丢了：{kept}");
        assert!(!kept.contains("[注]"), "壳没去干净：{kept}");
        assert!(!kept.contains('[') && !kept.contains(']'), "方括号进正文了：{kept}");
        // 规范化那一道同样只换壳、不动肉身：壳必须成对留着，好让排版层落成夹注。
        let norm = normalize_note_shells(raw);
        assert_eq!(norm, raw, "规范化的壳不该改变合法输入：{norm}");
        assert!(norm.contains("[注]注疏内容[/注]"), "夹注壳被破坏：{norm}");
    }

    /// 带空格 / 全角空格 / 疏体 / 全角斜杠的壳，一律归一成 `[注]…[/注]`，
    /// **肉身照旧**。
    #[test]
    fn spaced_and_shu_shells_are_normalized_not_deleted() {
        let s = normalize_note_shells("正文[ 注 ]某某曰：某，某也。[/注]正文");
        assert!(s.contains("[注]某某曰：某，某也。[/注]"), "疏/空格壳没归一：{s}");
        assert!(s.contains("某某曰"), "注疏肉身丢了：{s}");

        let s = normalize_note_shells("甲[疏]某某曰：某也。[/疏]乙");
        assert!(s.contains("[注]某某曰：某也。[/注]"), "疏体壳没转注：{s}");
        assert!(!s.contains("[疏]"), "疏体壳残留：{s}");

        // 全角斜杠 `／` 的收口壳（`[／注]`）同样归一成 `[/注]`。
        assert_eq!(normalize_note_shells("甲[注]某某[／注]乙"), "甲[注]某某[/注]乙");
        assert_eq!(normalize_note_shells("甲[　注　]乙[/注]丙"), "甲[注]乙[/注]丙");

        // 空壳（只有壳没有肉）：连壳消失，不许在版心留一枚孤零零的标记。
        assert_eq!(normalize_note_shells("甲[注][/注]乙"), "甲乙");
        // 不认识的方括号：只削括号，**里头的字一个不少**（[12] → 12）。
        assert_eq!(normalize_note_shells("甲[12]乙"), "甲12乙");
        assert_eq!(normalize_note_shells("甲[一]乙"), "甲一乙");
        // 落单的半截括号直接删。
        assert_eq!(normalize_note_shells("甲[乙"), "甲乙");
        assert_eq!(normalize_note_shells("甲]乙"), "甲乙");
        // 带参数的自家标记：连壳原样放行（下游还要靠它认注音 / 数字）。
        assert_eq!(normalize_note_shells("长[拼音:cháng]江"), "长[拼音:cháng]江");
        assert_eq!(normalize_note_shells("号[数字:978-7]"), "号[数字:978-7]");
    }

    /// HTML 同理：`<span class="note">注疏内容</span>` **只剥标签，留内容**。
    #[test]
    fn html_note_span_keeps_its_text() {
        let s = denoise_text("<span class=\"note\">注疏内容</span>");
        assert_eq!(s, "注疏内容", "标签没剥 / 内容被吞：{s}");
        assert!(!s.contains('<') && !s.contains('>'), "尖括号漏进正文：{s}");
        // 混在正文里的同样只去壳：前后正文 + 注疏内容一个字都不能少。
        let s = denoise_text("正文<span class=\"note\">某某曰</span>正文");
        assert_eq!(s, "正文某某曰正文", "内容被动了：{s}");
        // 走完整清洗链（`clean_line`）也是同一个结论。
        assert_eq!(clean_line("正文<span class=\"note\">某某曰</span>正文"), "正文某某曰正文");
    }

    /// 头部幽灵（BOM / YAML 元数据 / 全角空格 / 连续空行）：第一列第一个字
    /// 必须是**真字**，不许有任何前导空白。
    #[test]
    fn head_junk_never_reaches_the_first_column() {
        let raw = "\u{FEFF}---\ntitle: 庄子\nauthor: 某人\n---\n\u{3000}\u{3000}　道可道，非常道。\n\n\n\n\n名可名，非常名。";
        let s = strip_head_junk(raw);
        assert!(s.starts_with("道可道"), "首字没顶格：{s:?}");
        assert!(!s.contains("title:"), "YAML 头没剥：{s:?}");
        assert!(!s.contains('\u{FEFF}'), "BOM 残留：{s:?}");
        assert!(!s.starts_with('\u{3000}') && !s.starts_with(' '), "段首缩进没清零：{s:?}");
        // 连续 3 个以上空行折成至多一个空行：段与段之间不会插进幽灵空段。
        assert!(!s.contains("\n\n\n"), "幽灵空行没折叠：{s:?}");
        // 全文也没被洗没。
        assert!(s.contains("名可名，非常名。"), "正文被误删：{s:?}");
    }

    /// 汉字叶码（`十三` / `六`）与页眉（`尔雅 [ 注 ] 二`）不进正文流；
    /// 带上下文的（`十三年`）绝不误杀。
    #[test]
    fn lone_numbers_and_running_headers_are_filtered() {
        assert!(is_lone_number_para("十三"));
        assert!(is_lone_number_para("六"));
        assert!(is_lone_number_para("四十二"));
        assert!(is_lone_number_para("１２"));
        assert!(!is_lone_number_para("十三年"), "带上下文的数字被误判：");
        assert!(!is_lone_number_para("卷十三"));

        assert!(is_running_header("尔雅 [ 注 ] 二", "尔雅"), "页眉没认出来");
        assert!(is_running_header("庄子 卷三", "庄子"), "带书名的页眉没认出来");
        assert!(!is_running_header("道可道，非常道。", "尔雅"), "正文被当页眉：");
        assert!(!is_running_header("卷三", "尔雅"), "无书名的短行不许凭『卷』删：");

        // 端到端：这两段都得从正文流里消失。
        let mut b = book_of(vec![(
            "卷一",
            vec!["道可道，非常道。", "十三", "尔雅 [ 注 ] 二", "名可名，非常名。"],
        )]);
        b.title = "尔雅".to_string();
        clean_book(&mut b, crate::ir::IrSource::Txt);
        let body: String = b
            .chapters
            .iter()
            .flat_map(|c| c.paragraphs.iter())
            .map(|p| p.text.as_str())
            .collect();
        assert!(!body.contains("十三"), "汉字叶码没滤掉：{body}");
        assert!(!body.contains("尔雅"), "页眉没滤掉：{body}");
        assert!(body.contains("道可道，非常道。"), "正文被误杀：{body}");
    }

    /// 落单的书名号删掉，成对的原样留着（下游转 ︽︾ 作悬挂标点）。
    #[test]
    fn orphan_title_marks_stripped_but_pairs_kept() {
        assert_eq!(strip_orphan_title_marks("甲》乙"), "甲乙");
        assert_eq!(strip_orphan_title_marks("读《庄子》"), "读《庄子》");
        assert_eq!(strip_orphan_title_marks("读《庄子"), "读庄子");
    }

    /// v1.69.0：凡例里的并列标签词组判杂质。
    #[test]
    fn tag_list_lines_are_noise_but_prose_is_not() {
        assert!(is_tag_list_line("书名、地名、人名、夹注"), "凡例标签行没判成杂质");
        assert!(is_tag_list_line("书名 地名 人名 校记"), "空格并排的标签行没判成杂质");
        assert!(is_tag_list_line("夹注 校记 专名号"), "含「夹注」的短行没判成杂质");
        // 正文里带顿号的短列举：没有「标签味」词尾，绝不许误杀。
        assert!(!is_tag_list_line("仁、义、礼、智、信"), "正文列举被误杀");
        // 有句读的一律是正文。
        assert!(!is_tag_list_line("道可道，非常道。"), "正文被误杀");
        // 只有两段不算（太短，容易是正文）。
        assert!(!is_tag_list_line("道 德"), "过短的并列不算杂质");
    }

    /// v1.69.0：版权页废话与署名行不进正文流。
    #[test]
    fn colophon_boilerplate_is_noise() {
        assert!(is_noise_para("中华经典名著全本全注全译丛书"), "丛书名没判成杂质");
        assert!(is_noise_para("管锡华译注"), "署名行没判成杂质");
        assert!(is_noise_para("王晓玮 译注"), "署名行没判成杂质");
        assert!(is_noise_para("图书在版编目（CIP）数据"), "版权页没判成杂质");
        assert!(is_noise_para("z-library.sk"), "盗版站名没判成杂质");
        // 正文里提到丛书名（长句）不许误杀。
        assert!(
            !is_noise_para("本书属中华经典名著全本全注全译丛书之一种，点校细致，注释妥帖，翻译精确，读者可依此入门。"),
            "长句被误杀"
        );
    }

    /// v1.69.0：连续拉丁字母（拼音）删掉，单字母保留；自家标记不受影响。
    #[test]
    fn pinyin_runs_are_stripped_but_markers_survive() {
        assert_eq!(strip_pinyin_runs("道可道 dào kě dào 非常道"), "道可道 非常道");
        assert_eq!(strip_pinyin_runs("lùn yǔ"), "");
        assert_eq!(strip_pinyin_runs("子曰"), "子曰");
        // 单字母是一个词里的一格，不是拼音串，保留。
        assert_eq!(strip_pinyin_runs("甲 A 乙"), "甲 A 乙");
        // 声调数字一并吃掉。
        assert_eq!(strip_pinyin_runs("道 dao3 可"), "道 可");
        // 自家标记整段放行：`[拼音:]` 里的注音一个字母不少。
        assert_eq!(strip_pinyin_runs("长[拼音:cháng]江"), "长[拼音:cháng]江");
        // 多余空格折叠。
        assert_eq!(strip_pinyin_runs("甲  乙"), "甲 乙");
    }
}
