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

use std::borrow::Cow;
use std::cell::RefCell;

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

/// 拼音串剥离（写入 `out`，零 `Vec<char>` 拷贝）。
///
/// 与 [`strip_pinyin_runs`] 行为完全一致，但不再把整段 `collect` 成
/// `Vec<char>`（一段 5MB 正文会白白吃掉 20MB 堆）——改用字节游标 + 前瞻，
/// 直接把结果写进调用方复用的缓冲区。
pub fn strip_pinyin_runs_into(s: &str, out: &mut String) {
    out.clear();
    let mut i = 0usize;
    let mut space = false;
    while i < s.len() {
        let rest = &s[i..];
        let Some(c) = rest.chars().next() else { break };
        let c_len = c.len_utf8();
        // 自家标记整段放行（`[…]` 里的内容一个字母不动）。
        if c == '[' {
            if let Some(rel) = s[i + 1..].find(']') {
                let end = i + 1 + rel; // `]` 的字节下标
                out.push_str(&s[i..=end]);
                i = end + 1;
                space = false;
                continue;
            }
            // 没有配对的 `]`：当普通字符处理。
            out.push('[');
            i += c_len;
            space = false;
            continue;
        }
        if is_latin_letter(c) {
            // 数一数这串拉丁字母有多长。
            let mut j = i + c_len;
            while j < s.len() {
                let Some(nc) = s[j..].chars().next() else { break };
                if !is_latin_letter(nc) {
                    break;
                }
                j += nc.len_utf8();
            }
            // 只留「单字母」：那是一个词里的一格，不是拼音串。
            if j - i == c_len {
                out.push(c);
                space = false;
            }
            // 拼音串（≥2）：整段丢；后面跟着的数字（声调 `dào3`）一并吃掉。
            i = j;
            while i < s.len() {
                let Some(nc) = s[i..].chars().next() else { break };
                if !nc.is_ascii_digit() {
                    break;
                }
                i += nc.len_utf8();
            }
            continue;
        }
        if c == ' ' || c == '\u{00A0}' {
            if !space {
                out.push(' ');
                space = true;
            }
            i += c_len;
            continue;
        }
        out.push(c);
        space = false;
        i += c_len;
    }
    // 与旧实现一致：末尾 trim 后返回。
    let trimmed: String = out.trim().to_string();
    *out = trimmed;
}

/// 拼音串剥离：连续拉丁字母（≥2）整段删除，单个字母保留；
/// 自家标记 `[…]` 内原样透传。返回新 `String`。
pub fn strip_pinyin_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    strip_pinyin_runs_into(s, &mut out);
    out
}

/// 单字符去噪落笔：零宽 / BOM / 控制符删；私用区压成一个方块；
/// 各类空白归一成一个半角空格；其余原样。
fn denoise_push(out: &mut String, c: char, space: &mut bool, tile: &mut bool) {
    match c {
        '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{061C}'
        | '\u{180E}'
        | '\u{FEFF}'
        | '\u{00AD}'
        | '\u{0000}'..='\u{0008}'
        | '\u{000E}'..='\u{001F}'
        | '\u{007F}'..='\u{009F}' => {
            *space = false;
        }
        '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}' => {
            if !*tile {
                out.push('\u{25A1}');
                *tile = true;
            }
            *space = false;
        }
        // 换页 / 垂直制表当空格，绝不能当控制符删掉——删了上下两页会粘成一句。
        // 其余是 Unicode 空白家族（窄空格、行/段分隔符、蒙古文元音分隔符…），
        // 与 app 链 tidy_whitespace 的口径对齐。
        '\u{000B}' | '\u{000C}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200A}'
        | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}'
        | '\u{00A0}' | '\u{3000}' | '\t' | '\r' | '\n' | ' ' => {
            if !*space {
                out.push(' ');
                *space = true;
            }
            *tile = false;
        }
        _ => {
            out.push(c);
            *space = false;
            *tile = false;
        }
    }
}

/// 去噪（写入 `out`，零冗余拷贝）。
///
/// 把「拼音剥离 + 不可见字符 / 私用区折叠 + 空白归一」合并成**单趟扫描**直接
/// 写进 `out`——旧实现先 `to_string` 拷一份、再过 `strip_pinyin_runs`（内部
/// 又 `collect` 成 `Vec<char>`）、最后 `.trim().to_string()` 再拷一份，一段
/// 正文要付 3~4 次全量分配。新版只在「含 `<>` 标签或 `&` 实体」这种罕见路径上
/// 才分配临时串，常态路径零分配。
pub fn denoise_text_into(s: &str, out: &mut String) {
    out.clear();
    // 罕见路径：标签 / HTML 实体——按需才分配。
    let work: Cow<'_, str> = if (s.contains('<') && s.contains('>')) || s.contains('&') {
        let mut t = String::with_capacity(s.len());
        if s.contains('<') && s.contains('>') {
            t.push_str(&crate::pipeline::strip_tags(s));
        } else {
            t.push_str(s);
        }
        if t.contains('&') {
            t = crate::pipeline::decode_entities(&t);
        }
        Cow::Owned(t)
    } else {
        Cow::Borrowed(s)
    };

    let mut space = false;
    let mut tile = false; // 上一个输出已是「□」：连续私用/乱码只留一个方块
    let mut i = 0usize;
    while i < work.len() {
        let rest = &work[i..];
        let Some(c) = rest.chars().next() else { break };
        let c_len = c.len_utf8();

        // 自家标记整段放行（`[…]` 里的内容原样透传，不做任何归一）。
        if c == '[' {
            if let Some(rel) = work[i + 1..].find(']') {
                let end = i + 1 + rel;
                out.push_str(&work[i..=end]);
                i = end + 1;
                space = false;
                tile = false;
                continue;
            }
        }

        // 拼音串（连续拉丁字母 ≥2）整段丢；单字母保留并照常归一。
        if is_latin_letter(c) {
            let mut j = i + c_len;
            while j < work.len() {
                let Some(nc) = work[j..].chars().next() else { break };
                if !is_latin_letter(nc) {
                    break;
                }
                j += nc.len_utf8();
            }
            if j - i == c_len {
                denoise_push(out, c, &mut space, &mut tile);
            }
            i = j;
            while i < work.len() {
                let Some(nc) = work[i..].chars().next() else { break };
                if !nc.is_ascii_digit() {
                    break;
                }
                i += nc.len_utf8();
            }
            continue;
        }

        denoise_push(out, c, &mut space, &mut tile);
        i += c_len;
    }
    let trimmed: String = out.trim().to_string();
    *out = trimmed;
}

/// 去噪：剥离 HTML 残留、解码实体、删拼音串、折叠不可见字符与空白。
/// 返回新 `String`。
pub fn denoise_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    denoise_text_into(s, &mut out);
    out
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

/// **自家标记壳的位置掩码**（v1.9.56）：`[注]` `[/注]` `[疏]` `[/疏]`
/// `[高亮]` `[/高亮]` `[note]` `[/note]` `[拼音:…]` `[数字:…]` 的**每一个
/// 字符**都标 `true`，其余 `false`；返回的向量与 `s.chars()` **逐位对齐**。
///
/// 这是「标签绝不许进排版网格」的最后一道闸。此前只有导入管线负责把壳收
/// 干净，一旦某本书稿是**旧版本**导入的（`Paragraph.text` 里还留着
/// `[/注]`），`Book::to_flow` 就会把 `[` `/` `注` `]` 当正文逐字塞进列里
/// ——真机上是四枚大字竖着排下来，整页书就废了。分页引擎在**造字流**时不
/// 再信任上游，照这张掩码跳字：正文干净与否是导入的事，**能不能上纸是排版
/// 的事**，两者必须各守一条线。
///
/// 掩码只盖**壳**、不盖壳里的字：`[注]周纪三[/注]` → 只挡 7 个字符，
/// `周纪三` 原样留在正文里（剥壳 ≠ 吞字）。
///
/// `s` 中没有 `[` 时返回空表（热路径：99.9% 的段落走这一支，零分配）。
pub fn note_tag_mask(s: &str) -> Vec<bool> {
    if !s.contains('[') {
        return Vec::new();
    }
    let cs: Vec<char> = s.chars().collect();
    let mut mask = vec![false; cs.len()];
    let mut i = 0usize;
    while i < cs.len() {
        if cs[i] != '[' {
            i += 1;
            continue;
        }
        // 收口最多向前看 12 字：最长的 `[拼音:zhōng]` 也不过这个量级。
        let close = (i + 1..(i + 13).min(cs.len())).find(|&j| cs[j] == ']');
        let Some(j) = close else {
            i += 1;
            continue;
        };
        // 归一化判据与 `normalize_note_shells` 同口径：去空白、全角斜杠折半角。
        let key: String = cs[i + 1..j]
            .iter()
            .filter(|c| !matches!(c, ' ' | '\t' | '\u{3000}' | '\u{00A0}'))
            .map(|c| if *c == '／' { '/' } else { *c })
            .collect();
        let low = key.to_lowercase();
        let is_tag = matches!(
            low.as_str(),
            "注" | "疏" | "/注" | "/疏" | "高亮" | "/高亮" | "note" | "/note"
        ) || low.starts_with("拼音:")
            || low.starts_with("拼音：")
            || low.starts_with("数字:")
            || low.starts_with("数字：");
        if is_tag {
            for m in &mut mask[i..=j] {
                *m = true;
            }
            i = j + 1;
        } else {
            // 不是自家壳（`[1]` 年份注、正文里的方括号）：一个字符都不许挡。
            i += 1;
        }
    }
    mask
}

/// **剥掉自家标记壳、留下壳内的字**：`[注]周纪三[/注]` → `周纪三`。
///
/// 与 [`note_content_only`] 的区别：那个还要拆 `[拼音:x]` 取内容、清残余
/// 括号；这个只做一件事——按 [`note_tag_mask`] 把壳的字符删掉，一个正文
/// 字都不动。用于章题、注文、目录这些**已经取出内容**、只剩壳的地方。
pub fn strip_note_tags(s: &str) -> String {
    let mask = note_tag_mask(s);
    if mask.is_empty() {
        return s.to_string();
    }
    s.chars()
        .enumerate()
        .filter(|(i, _)| !mask.get(*i).copied().unwrap_or(false))
        .map(|(_, c)| c)
        .collect()
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

// ───────────────────── 书眉：跨章 / 跨页「变尾码 + 整行重复」双路剔除 ─────────────────────
//
// 竖排影印 / OCR 古籍每叶都印同一串书眉（`資治通鑑卷一 周紀一・威烈王・安王・烈王　三`
// / `…　四` / `…　五`）：只在最末缀一个**页码**，整行因此**每页都不完全相同**。
// 只靠「整行完全重复」认书眉会漏掉它们；这里把行尾页码剥掉、按**前缀**归并，
// 让同一书眉的所有变体一起被认出来。
//
// 与 [`is_running_header`]（单行：无句读 + 短 + 书名 / 注疏）互补：那个看**一行**，
// 这个看**整书分布**，能抓跨章 / 跨页重复与「仅页码不同」的变体。
// 也和 `vclean::purge_running_headers`（EPUB）、`pdf_text::strip_running_lines`（PDF）
// 共用同一把尺，保证各格式口径一致。

/// 书眉判定用的归一键：去空白（含全角 / 半角空格与换行），**保留 `・` 等间隔号**
/// —— 它们是前缀身份的一部分，剥了会把不同书眉混为一谈。
pub fn header_norm_key(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 书眉「骨架」归一（清洗底层共用，跨格式同一把尺）：在去空白之上，再剥掉所有
/// 页码数字（`rh_is_tail_char` 涵盖汉字 `〇零一二…廿卅` / 阿拉伯 / 罗马）、`第`、
/// 与各类括号（`〔〕【】（）()「」『』<>`）及注疏壳（`[注]`/`[/注]` 的 `[` `]` `注`）。
///
/// 用途：把同一条书眉的全部数字 / 中缀变体坍缩成同一骨架——
/// `資治通鑑卷一` / `資治通鑑卷第一・周紀一・威烈王・安王・烈王` /
/// `資治通鑑卷一周紀一・威烈王・安王・烈王二` 全部化成
/// `資治通鑑卷・紀・威烈王・安王・烈王`，再用频率（≥ [`RH_MIN_REPEAT`]）一网打尽。
///
/// ⚠️ 它**只用于匹配键**，绝不改动正文文本——正文清洗由 `denoise_text` 负责。
pub fn skeleton_key(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !rh_is_tail_char(*c)
                && !matches!(
                    *c,
                    '第' | '〔' | '〕' | '【' | '】' | '（' | '）' | '(' | ')'
                        | '「' | '」' | '『' | '』' | '<' | '>' | '[' | ']' | '注'
                )
        })
        .collect()
}

/// 尾码字符：阿拉伯数字 / 汉字数字 / 罗马数字。
fn rh_is_tail_char(c: char) -> bool {
    c.is_ascii_digit()
        || ('０'..='９').contains(&c)
        || matches!(
            c,
            '〇' | '零' | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十'
                | '百' | '千' | '万' | '萬' | '两' | '兩' | '廿' | '卅'
        )
        || matches!(c, 'i' | 'I' | 'v' | 'V' | 'x' | 'X' | 'l' | 'L')
}

/// 一整串是否纯尾码字符（这种串不是书眉，是页码本身）。
fn rh_prefix_only_numerals(s: &str) -> bool {
    !s.is_empty() && s.chars().all(rh_is_tail_char)
}

/// 一行像不像「古籍书眉 / 卷题」：短（≤ [`RH_MAX_CHARS`]），且含结构标记
/// `卷 / 紀 / 纪 / 册 / 冊 / 集 / 部`。
///
/// 影印古籍每叶都印同一串卷题、又不是正文——`周紀一・威烈王・安王・烈王`、
/// `資治通鑑卷二周紀二・顯王`、`卷` 都是这类。不含 `題 / 章 / 年` 等，避免误杀
/// 正文里偶提的「某章」与一般年份；正文第一句「凡三十五年」之类的数字句不含
/// 任何结构标记，绝不会被这一路认走。
fn rh_looks_like_header(k: &str) -> bool {
    let n = k.chars().count();
    if n == 0 || n > RH_MAX_CHARS {
        return false;
    }
    k.contains('卷')
        || k.contains('紀')
        || k.contains('纪')
        || k.contains('册')
        || k.contains('冊')
        || k.contains('集')
        || k.contains('部')
}

/// 剥掉行尾的「页码尾码」，返回 `(前缀, 尾码)`。
///
/// 尾码 = 行尾连续的数字（阿拉伯 / 汉字数字 / 罗马数字）；其前若是
/// `第` / `页` / `葉` / `頁` / `p` / `P` / `No` / `no` 这类页码前缀字一并剥掉。
pub fn strip_page_tail(s: &str) -> (String, String) {
    let cs: Vec<char> = s.trim().chars().collect();
    if cs.is_empty() {
        return (String::new(), String::new());
    }
    let mut end = cs.len();
    while end > 0 && rh_is_tail_char(cs[end - 1]) {
        end -= 1;
    }
    // 尾码前的页码前缀字（`第N` / `N页` / `No.N`）。
    if end < cs.len() && end > 0 {
        let c = cs[end - 1];
        if matches!(c, '第' | '页' | '葉' | '叶' | '頁' | 'p' | 'P') {
            end -= 1;
        } else if end > 1 && cs[end - 2] == '第' {
            end -= 2;
        } else if end > 1
            && (cs[end - 1] == 'o' || cs[end - 1] == 'O')
            && (cs[end - 2] == 'n' || cs[end - 2] == 'N')
        {
            end -= 2;
        }
    }
    let prefix: String = cs[..end].iter().collect();
    let tail: String = cs[end..].iter().collect();
    (prefix, tail)
}

/// 书眉短行的长度上限（字）：再长的「重复行」是正文（诗里的叠句），不动。
const RH_MAX_CHARS: usize = 30;
/// 同一前缀出现多少次才算书眉：影印书眉几乎是每叶一条。
const RH_MIN_REPEAT: usize = 3;

/// 全书的「书眉」归一键集合（跨格式共用同一把尺，两路都认）。
///
/// 输入是一本书的**所有行**（逐行文本，含标点也无妨）。返回应当删除的
/// 归一键（[`header_norm_key`] 之后）。
///
/// **（A）同前缀·仅页码不同**：剥掉尾码后的前缀 ≥ 4 字、非纯数字，且同一前缀
/// 在全书出现 ≥ [`RH_MIN_REPEAT`] 次、至少 **2 个不同尾码**——覆盖
/// `資治通鑑卷一…三 / 四 / 五` 这类汉字页码尾码（整行完全重复认不出）。
///
/// **（B）整行完全重复的书眉**：短（≤ [`RH_MAX_CHARS`]）、含结构标记
/// `卷 / 紀 / 纪 / 册 / 冊 / 集 / 部`、且整行**原样重复 ≥ [`RH_MIN_REPEAT`]
/// 次**——覆盖影印古籍「每卷/纪只跨几页」的卷题（`周紀一・威烈王・安王・烈王`
/// 每页印一遍、整本只出现十几回）。这类既低于 PDF `strip_running_lines` 的
/// 1/3 页门槛，又没有变尾码可认，单列一行重复判定也漏，故按「书眉形状」补刀。
///
/// 两路都走「严格、宁可漏判不可误杀」：正文第一句「凡三十五年」这类数字句不含
/// 任何结构标记，绝不会被（B）认走；纯数字串（`一二三`）被（A）的结构守卫挡掉。
/// 全书的「书眉」骨架删除集（跨格式共用同一把尺，两路都认）。
///
/// 返回 `(结构标记命中, 纯书名命中)` 两个骨架集：
/// * **结构集**——骨架含 `卷/紀/纪/册/冊/集/部` 且频率 ≥ [`RH_MIN_REPEAT`]，
///   直接可删（有结构标记 + 重复，几乎不可能是正文）；
/// * **书名集**——骨架恰好等于书名骨架（影印书口字 / 书眉头），需由调用方
///   结合「页面最前 / 最后几行」的位置闸门再放行，避免误杀正文中偶提的书名。
///
/// 输入是一本书的所有行（逐行文本）；返回应当删除的**骨架键**
/// （[`skeleton_key`] 之后）。骨架归一会把同一条书眉的`資治通鑑卷一` /
/// `資治通鑑卷第一・周紀一…` / `資治通鑑卷一周紀一…二` 等全部数字 / 中缀变体
/// 坍缩成同一个骨架，再用频率一网打尽。
pub fn running_header_prefix_keys(
    lines: &[String],
    title: &str,
) -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
) {
    let title_sk = skeleton_key(title);
    let mut groups: std::collections::HashMap<
        String,
        (usize, std::collections::HashSet<String>),
    > = std::collections::HashMap::new();
    let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for l in lines {
        let k = skeleton_key(l);
        if k.is_empty() || k.chars().count() > RH_MAX_CHARS {
            continue;
        }
        *freq.entry(k.clone()).or_insert(0) += 1;
        // （A）同前缀·仅页码不同：骨架化后数字已先剥，前缀归并更稳。
        let (prefix, tail) = strip_page_tail(&k);
        if prefix.chars().count() >= 4 && !rh_prefix_only_numerals(&prefix) {
            let g = groups
                .entry(prefix)
                .or_insert((0, std::collections::HashSet::new()));
            g.0 += 1;
            if !tail.is_empty() {
                g.1.insert(tail);
            }
        }
    }
    let mut structure: std::collections::HashSet<String> = std::collections::HashSet::new();
    // （A）同前缀·仅页码不同。
    for (prefix, (count, tails)) in &groups {
        if *count >= RH_MIN_REPEAT && tails.len() >= 2 {
            for l in lines {
                let k = skeleton_key(l);
                if k.chars().count() > RH_MAX_CHARS {
                    continue;
                }
                let (p, _) = strip_page_tail(&k);
                if p == *prefix {
                    structure.insert(k);
                }
            }
        }
    }
    // （B）整行重复 / 数字中缀变体坍缩后的书眉骨架。
    let mut by_title: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (k, n) in &freq {
        if *n >= RH_MIN_REPEAT {
            if rh_looks_like_header(k) {
                structure.insert(k.clone());
            } else if !title_sk.is_empty() && k == &title_sk {
                by_title.insert(k.clone());
            }
        }
    }
    (structure, by_title)
}

/// 在清洗主流程的去噪阶段，把书眉（变尾码 / 整行重复两路）从 IR 里赶出去。
///
/// 跨所有巻 / 章收集块文本，复用 [`running_header_prefix_keys`]：md / txt /
/// html / json / docx 这些走 `clean_book` 的格式因此与 EPUB（`purge_running_headers`）、
/// PDF（`strip_running_lines`）共用同一把尺。只删 `Para` / `Annotation`（章题、
/// 眉批不在其列），且绝不把空巻判没（下游 `retain` 另有兜底）。
fn purge_running_header_variants(ir: &mut IrBook, title: &str) {
    let mut lines: Vec<String> = Vec::new();
    for j in ir.juans.iter() {
        for b in j.blocks.iter() {
            let t = b.plain_text();
            if !t.trim().is_empty() {
                lines.push(t);
            }
        }
    }
    if lines.len() < RH_MIN_REPEAT {
        return;
    }
    let (structure, by_title) = running_header_prefix_keys(&lines, title);
    let drop: std::collections::HashSet<String> =
        structure.into_iter().chain(by_title).collect();
    if drop.is_empty() {
        return;
    }
    for j in ir.juans.iter_mut() {
        j.blocks.retain(|b| match b {
            IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
                !drop.contains(&skeleton_key(&spans_line(spans)))
            }
            _ => true,
        });
    }
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
/// 判据保守：只认圆括号 `（）` / `()`（现代编辑加的注释都这么写），内容 1–30 字、
/// 不含句读（含句读的说明是正文本身，不是注）、不含嵌套括号。
/// 括号→夹注（写入 `out`）。与 [`paren_to_note`] 行为一致，写进复用缓冲区，
/// 且把 `format!("[注]{body}[/注]")` 的临时串也省掉（直接三次 `push_str`）。
pub fn paren_to_note_into(s: &str, out: &mut String) {
    out.clear();
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
                            out.push_str("[注]");
                            out.push_str(body);
                            out.push_str("[/注]");
                        } else {
                            out.push('（');
                            out.push_str(body);
                            out.push('）');
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
}

/// 括号→夹注：把 `（）` / `()` 保守升格成系统一等公民的 `[注]…[/注]` 标记。
/// 返回新 `String`。
pub fn paren_to_note(s: &str) -> String {
    let mut out = String::new();
    paren_to_note_into(s, &mut out);
    out
}

/// 这个位置是否在**行首**（只允许前面是空白）。
///
/// 只往回看到上一个非空白字符为止——行首缩进撑死几个空格，所以这一步
/// 是 O(1)，不会因为正文很长而退化。
pub(crate) fn at_line_head(cs: &[char], i: usize) -> bool {
    let mut j = i;
    while j > 0 {
        j -= 1;
        match cs[j] {
            '\n' => return true,
            ' ' | '\u{3000}' | '\t' => continue,
            _ => return false,
        }
    }
    true // 文件开头也算行首
}

/// 行首短标签的**闭括号位置**；不是标签返回 `None`。
///
/// 判据（缺一不可）：
///   * 内容 **1–8 字**——`【题解】【注釋】【譯文】【按】` 都在这个量级，
///     真正的注疏内容远超 8 字；
///   * 内容**无句读**——有 `。，；：` 就是一段话，不是标签；
///   * 内容**无嵌套**——`〔外（內）尾〕` 是注，不是标签；
///   * 中间不许换行（标签不会跨行）。
fn bracket_label_end(cs: &[char], i: usize, open: char) -> Option<usize> {
    const LABEL_MAX: usize = 8;
    let close = if open == '〔' { '〕' } else { '】' };
    let mut j = i + 1;
    let mut n = 0usize;
    while j < cs.len() {
        let c = cs[j];
        if c == close {
            return if n >= 1 { Some(j) } else { None };
        }
        if n >= LABEL_MAX
            || matches!(c, '〔' | '【' | '〕' | '】' | '\n' | '（' | '(' | '）' | ')')
            || is_sentence_punct(c)
        {
            return None;
        }
        n += 1;
        j += 1;
    }
    None
}

/// **统一「括号→双行夹注」入口**：把 `〔…〕` / `【…】` 与圆括号 `（）` / `()`
/// 都归一成系统一等公民的 `[注]…[/注]` 标记。
///
/// 这样 TXT / DOCX（走 `pipeline::parse_txt`）与 EPUB（走 `vclean::clean_epub`）
/// 三条导入路共享同一套夹注识别——这才是真正的「全格式」。
///
/// 设计要点：
/// - `〔…〕` / `【…】` 升格（古籍注疏、校勘记的正规写法，不限额长）。
/// - **行首的短标签不升格**（v1.9.26）：`【题解】` `【注釋】` `〔按〕` 这类
///   章节体例标记**原样保留**。判据是「行首 + ≤8 字 + 无句读 + 无嵌套」，
///   夹注永远嵌在正文中间，过不了「行首」这一关。
/// - `（）` / `()` 用**保守判据**升格：内容 **2–30 字**（单字是标记不是注文，
///   见下方注释）、不含句读（含句读说明是正文）、不含嵌套括号、且不是已存在
///   的 `[注]` 壳；否则原样保留，绝不吞字。
/// - `（）` 若嵌在 `〔…〕` / `【…】` 注内，原样透传，绝不套出双层 `[注]`。
/// - 残留未闭合的括号一律兜底成对，绝不产生不配对的 `[注]` 标记。
/// - **严格标记 `[note]…[/note]`** 直接映射成 `[注]…[/注]`，给用户一个绝不会
///   被标点误触发的安全入口。
///
/// ⚠️ **PDF 链不经过这里**。PDF 抽取文本走 `parsers::classicize` →
/// `parsers::purge_bracket_marks`，那条链上的 `【题解】` 绝不能升格成夹注
/// （尔雅式的章节标签，升格就等于正文缺一段，v1.9.26 已定死）。两条链的
/// 口径**故意不同**：这里面对的是「括号就是注释协议」的裸文本 / EPUB，
/// 那边面对的是版式已定的印刷品。改任何一边之前，先确认动的是哪条链。
pub fn brackets_to_notes(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    // 栈记「注壳类型」：0 = 〔，1 = 【。遇到对应闭壳才收口。
    let mut stack: Vec<u8> = Vec::new();
    // 圆括号注缓冲：仅当不在 〔〕/【】 注内时收集 `（）` 内容。
    let mut paren: Option<String> = None;
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        match c {
            // —— 行首短标签：**原样保留，绝不升格** ——
            //
            // `【题解】此章言道之體。` 里的「题解」是**章节体例标签**（尔雅、
            // 诗经、四库提要都是这个写法），不是夹注。升格成 `[注]题解[/注]`
            // 会被下游 `split_body_marks` 当双行小字吃掉，正文直接少一段。
            //
            // 判据四条同时成立才算标签（见 `bracket_label_end`）：**行首** +
            // **≤8 字** + **无句读** + **无嵌套**。夹注永远嵌在正文中间，
            // 天然不满足「行首」这一条——这是最硬的分界线。
            '〔' | '【' => {
                if at_line_head(&cs, i) {
                    if let Some(end) = bracket_label_end(&cs, i, c) {
                        out.extend(&cs[i..=end]);
                        i = end + 1;
                        continue;
                    }
                }
                stack.push(if c == '〔' { 0 } else { 1 });
                out.push_str("[注]");
            }
            '〕' => {
                // 与栈顶 〔 配对才闭；落单右括号（正文里的噪声）丢弃，
                // 与旧 `txt_bracket_notes` 行为一致。
                if let Some(&0) = stack.last() {
                    stack.pop();
                    out.push_str("[/注]");
                }
            }
            '】' => {
                if let Some(&1) = stack.last() {
                    stack.pop();
                    out.push_str("[/注]");
                }
            }
            // —— 严格标记 [note] / [/note] ——
            //
            // ⚠️ 步进必须**吃掉整个标记**（含 `]`）：`continue` 会跳过循环末尾的
            // `i += 1`，少跳一格就把右方括号当正文留在原地，输出 `[注]]`。
            '[' => {
                // `[/note]` = 7 个字符
                if i + 7 <= cs.len()
                    && cs[i + 1] == '/'
                    && &cs[i + 2..i + 7] == ['n', 'o', 't', 'e', ']']
                {
                    out.push_str("[/注]");
                    i += 7;
                    continue;
                }
                // `[note]` = 6 个字符
                if i + 6 <= cs.len() && &cs[i + 1..i + 6] == ['n', 'o', 't', 'e', ']'] {
                    out.push_str("[注]");
                    i += 6;
                    continue;
                }
                out.push(c);
            }
            '（' | '(' => {
                if paren.is_some() {
                    paren.as_mut().unwrap().push(c);
                } else if stack.is_empty() {
                    paren = Some(String::new());
                } else {
                    // 处在 〔〕/【】 注内：原样透传，绝不嵌套。
                    out.push(c);
                }
            }
            '）' | ')' => {
                if let Some(mut b) = paren.take() {
                    b.push(c);
                    let body = b
                        .trim()
                        .trim_matches(|x| matches!(x, '（' | '(' | '）' | ')'))
                        .to_string();
                    let n = body.chars().count();
                    // **单字不升格**（v1.9.26）：`（注）` `（案）` 里的那个字
                    // 是**标记本身**，不是注文。升格成 `[注]注[/注]` 之后，
                    // 下游会把内容恰好是「注」的夹注当壳吞掉——正文里
                    // 「其間有（注）者」会变成「其間有者」，凭空少两个字。
                    let ok = (2..=30).contains(&n)
                        && !body.chars().any(is_sentence_punct)
                        && !body.contains(['（', '）', '(', ')'])
                        && !body.contains("[注]");
                    if ok {
                        out.push_str(&format!("[注]{body}[/注]"));
                    } else {
                        // 不升格：把开闭括号原样补回，绝不吞字。
                        out.push('（');
                        out.push_str(&body);
                        out.push('）');
                    }
                } else {
                    out.push(c);
                }
            }
            _ => {
                if let Some(b) = paren.as_mut() {
                    b.push(c);
                } else {
                    out.push(c);
                }
            }
        }
        i += 1;
    }
    if let Some(b) = paren.take() {
        // 没闭合的括号：把内容原样补回（含开括号），绝不吞字。
        out.push('(');
        out.push_str(&b);
    }
    // 残留未闭合的 〔 / 【：一律兜底补收壳，保证 `[注]` 与 `[/注]` 成对——
    // 下游 `split_body_marks` 见到悬空开壳会把它当正文吐出来（整段变夹注）。
    while stack.pop().is_some() {
        out.push_str("[/注]");
    }
    out
}

/// 把一段正文里的括号注转成 IR 的夹注节点。
fn rebuild_notes_juan(j: &mut IrJuan) {
    for b in j.blocks.iter_mut() {
        if let IrBlock::Para(spans) = b {
            let line = spans_line(spans);
            // 圆括号（全角 / 半角）才可能触发夹注。
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

/// 单个字的异体归一（不在表里就原样返回）。
///
/// v1.9.78 起委托 [`crate::variants::VariantTable`]：不再逐条线性扫 27 项，
/// 而是查哈希表，且表里有 500+ 条（外部 CJK 表里「繁简同形」那一类，
/// 折过去是字形归一、不是简繁转换，见 `variants` 模块文档）。
///
/// **仍然只折异体，绝不折繁体正字**——`道德經` 进来还是 `道德經`。
pub fn normalize_variant_char(c: char) -> char {
    crate::variants::active().fold(c)
}

/// 全文本异体字归一。
///
/// 一次取激活表快照，再逐字查表——**绝不每字读一次锁 / 磁盘**。
/// 百万字丛书这条路会跑一整遍，性能不能靠「反正表小」糊过去。
/// 异体字归一（写入 `out`）。一次性取激活表快照，逐字查表——绝不每字读一次锁。
pub fn normalize_variants_into(s: &str, out: &mut String) {
    let table = crate::variants::active();
    out.clear();
    out.reserve(s.len());
    for c in s.chars() {
        out.push(table.fold(c));
    }
}

/// 全文本异体字归一。返回新 `String`。
pub fn normalize_variants(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    normalize_variants_into(s, &mut out);
    out
}

/// **正文字数**（v1.9.70）：剔除 `[注]` / `[/注]` 标记与所有空白后的字数。
///
/// ## 为什么必须单独有一个函数
/// 清洗把 `〔…〕` 升格成 `[注]…[/注]`——每处**凭空多出 7 个字符**（`[注]` 三个、
/// `[/注]` 四个）。这些是给渲染器看的**结构标记**，不是书里的字。用
/// `str::chars().count()` 直接比毛坯与清洗后，几千处夹注就能让「删了 3 万字」
/// 显示成「净增 31829 字」——用户看到的第一反应是「这引擎在往我的书里塞字」。
///
/// 所以字数一律按**正文**算：去掉标记、去掉换行 / 空格 / 制表符，剩下的才算。
/// 标点计入（`。，` 也是原书的一个字位），零宽与 BOM 这类不可见字符不算。
pub fn body_char_count(s: &str) -> usize {
    const OPEN: &str = "[注]";
    const CLOSE: &str = "[/注]";
    let mut n = 0usize;
    let mut i = 0usize;
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with(OPEN) {
            i += OPEN.len();
            continue;
        }
        if rest.starts_with(CLOSE) {
            i += CLOSE.len();
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        if !c.is_whitespace() && !is_invisible_char(c) {
            n += 1;
        }
        i += c.len_utf8();
    }
    n
}

/// **只删落单的收标记**（v1.9.71）：`[/注]` / `[/高亮]` 前面若没有配对的
/// 开标记，连壳删掉；成对的、以及**没闭合的开标记**一律原样保留。
///
/// ## 为什么需要它（真机现场）
/// 一本《资治通鉴》txt 里 `〔胡三省注：…〕` 跨了硬折行：`brackets_to_notes`
/// 照规矩把整对升格成 `[注]…\n…[/注]`（换行在注内原样透传），可段落是**按行
/// 收口**的——上一行补了一个收口，这一行真正的收标记就成了孤儿，
/// `split_body_marks` 配不上对，四个标签字符原样印进正文：
/// 用户截图里那句「且俟知者。**[/注]**」就是它。
///
/// ## 为什么不用 `types::strip_dangling_note_marks`
/// 那个是 EPUB 链的「配对通吃」：落单的**开**标记也删。可核心管线的既有契约是
/// 「未闭合的 `[注]` 把后文吞进夹注、绝不丢字」（见
/// `pipeline::tests::note_after_trailing_space_keeps_its_body`）——直接换过去
/// 会把注文变成正文，改动面太大。这里只做**收标记**这一个方向，精准消掉泄露。
///
/// 不改变任何非标签字符：壳外的正文一个字不动。
pub fn drop_lone_note_closes(s: &str) -> String {
    if !s.contains('[') {
        return s.to_string();
    }
    // (开标记, 收标记)：正文里可能出现的三族壳。
    const PAIRS: [(&str, &str); 4] = [
        ("[注]", "[/注]"),
        ("[疏]", "[/疏]"),
        ("[note]", "[/note]"),
        ("[高亮]", "[/高亮]"),
    ];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // 各族壳各自记深度：`[高亮][注]…[/注][/高亮]` 这种嵌套也能正确配对。
    let mut depth = [0usize; 4];
    while !rest.is_empty() {
        let mut hit = false;
        for (k, (open, close)) in PAIRS.iter().enumerate() {
            if rest.starts_with(close) {
                if depth[k] > 0 {
                    depth[k] -= 1;
                    out.push_str(close);
                }
                // 落单的收标记：直接吞掉（连壳删）。
                rest = &rest[close.len()..];
                hit = true;
                break;
            }
            if rest.starts_with(open) {
                depth[k] += 1;
                out.push_str(open);
                rest = &rest[open.len()..];
                hit = true;
                break;
            }
        }
        if hit {
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// **三家注智能剥离**（v1.9.72）：`【集解】` / `【索隐】` / `【正义】` 这类
/// 注文标签一出现，就从这里开始**进入注文模式**，一路吃到下一个注文标签、
/// 空行（段落结束）或章题为止，整段包成 `[注]…[/注]`（双行小字夹注）。
///
/// ## 为什么必须有这一步
/// 《史记》三家注的电子文本长这样：
///
/// ```text
/// 黄帝者，少典之子，姓公孙，名曰轩辕。【集解】徐广曰：号有熊。
/// 【索隐】案：有熊国君，少典之子，姓公孙。
/// ```
///
/// `【集解】` 恰好**过不了** v1.9.26 那条「行首 + ≤8 字 + 无句读 → 判为章节
/// 体例标签、原样保留」的闸（它确实长得像 `【题解】`），于是整段注文被当
/// **正文大字**排了下去——用户在《史记》里看到的就是「【集解】谯周曰……」
/// 顶着正文字号铺满一列，专业排版上是灾难。
///
/// ## 口径
///   * 只认**明确的注家标签**（集解 / 索隐 / 索隱 / 正义 / 正義），`【题解】`
///     `【注釋】` 这类章节标签一个字都不动——它们本来就不是注文。
///   * 注文续行（下一行没有标签）**并进同一枚夹注**，中间不留换行（古籍竖排
///     折行的位置不是语义边界，留 `\n` 会在小字里凭空断行）。
///   * 遇到空行 / 章题立刻收口：绝不让一枚夹注把下一段正文吞成小字。
///   * 标签本身保留在注文里（`集解：…`）——三家注的价值一半在于「这是谁说的」。
pub fn mark_three_jia_notes(s: &str) -> String {
    // 热路径：全文里连注家名都没有就一个字都不用看（大书导入时 99% 的行走这支）。
    if !THREE_JIA_LABELS.iter().any(|l| s.contains(l)) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 32);
    let mut in_note = false;
    for (i, line) in s.split('\n').enumerate() {
        let blank = line.trim().is_empty();
        let head = !blank && crate::pipeline::is_chapter_head(line);
        // 空行 / 章题 = 段落边界 → 先收口。收标记必须紧贴上一行注文的末尾，
        // 绝不能自己占了下一行的行首（那会让夹注把空行也吞进小字）。
        if in_note && (blank || head) {
            out.push_str("[/注]");
            in_note = false;
        }
        // 注文续行：直接相接，**不吐换行**。古籍硬折行的位置不是语义边界，
        // 留着 `\n` 会让同一段注文在小字里凭空断开。
        if i > 0 && !(in_note && !blank) {
            out.push('\n');
        }
        // 空行原样吐出去（上面的收口已经做完了）。
        if blank {
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        while !rest.is_empty() {
            match find_note_label(rest) {
                Some((at, len, label)) => {
                    // 标签前面的字：在注里就是注文的续段，不在注里就是正文。
                    if at > 0 {
                        out.push_str(&rest[..at]);
                    }
                    // 换注：先把上一枚收掉（`[注]…[注]` 叠着会让下标错位）。
                    if in_note {
                        out.push_str("[/注]");
                    }
                    out.push_str("[注]");
                    out.push_str(label);
                    out.push('：');
                    in_note = true;
                    rest = &rest[at + len..];
                }
                None => {
                    out.push_str(rest);
                    break;
                }
            }
        }
    }
    if in_note {
        out.push_str("[/注]");
    }
    out
}

/// 三家注的标签字（正体 / 繁体都认）。
///
/// 刻意**只列注家名**：`题解` / `注釋` / `按` 这些是章节体例或行文用语，
/// 把它们也转成小字会把正文吃掉（v1.9.26 那条闸就是为它们立的）。
const THREE_JIA_LABELS: [&str; 5] = ["集解", "索隐", "索隱", "正义", "正義"];

/// 在 `s` 里找下一个注文标签，返回 `(起始字节, 整枚标签字节长, 标签字)`。
///
/// 括号四套都认（`【集解】` / `〔集解〕` / `[集解]` / `［集解］`）——不同排印本
/// 与电子本各用一套，用户实报的那本书里就混着用。只认括号里**恰好是注家名**
/// 的，`[注]` / `[1]` / `[案]` 一律不算（它们是自家壳或正文）。
fn find_note_label(s: &str) -> Option<(usize, usize, &'static str)> {
    const SHELLS: [(char, char); 4] = [('【', '】'), ('〔', '〕'), ('[', ']'), ('［', '］')];
    let mut from = 0usize;
    while from < s.len() {
        let open = s[from..]
            .char_indices()
            .find(|(_, c)| SHELLS.iter().any(|(o, _)| o == c))
            .map(|(i, c)| (from + i, c))?;
        let (start, oc) = open;
        let close = SHELLS
            .iter()
            .find(|(o, _)| *o == oc)
            .map(|(_, c)| *c)
            .unwrap_or('】');
        let after = &s[start + oc.len_utf8()..];
        let Some(end) = after.find(close) else {
            // 没有收括号：这个开括号不可能是标签，往后挪一格继续。
            from = start + oc.len_utf8();
            continue;
        };
        let inner = after[..end].trim();
        if let Some(label) = THREE_JIA_LABELS.iter().find(|l| **l == inner) {
            return Some((start, oc.len_utf8() + end + close.len_utf8(), label));
        }
        from = start + oc.len_utf8();
    }
    None
}

/// **纯数字标号整枚删**（v1.9.72）：`[1]` `[２]` `［12］` → 什么都不留。
///
/// 三家注的电子文本里到处是 `[1]` `[2]` 这种上标注号（脚注锚点被压平后的
/// 产物）。旧链路把它们交给 `normalize_note_shells` 的「不认识的方括号只削
/// 括号、留肉身」分支——结果是正文里留下一串孤零零的阿拉伯数字，紧贴在
/// 句读旁边（「道可道1，非常名2。」），比标签还难看出是脏数据。
///
/// 只认**纯数字**：`[一]` `[注]` `[案]` 一律不碰（可能是正文里的真字）。
pub fn drop_reference_marks(s: &str) -> String {
    if !s.contains('[') && !s.contains('［') {
        return s.to_string();
    }
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        if c == '[' || c == '［' {
            let close = if c == '[' { ']' } else { '］' };
            // 标号不会长：最多向前看 6 字（`[12345]`）。
            let end = (i + 1..(i + 7).min(cs.len())).find(|&j| cs[j] == close);
            if let Some(j) = end {
                let pure_num = j > i + 1
                    && cs[i + 1..j]
                        .iter()
                        .all(|ch| ch.is_ascii_digit() || ('０'..='９').contains(ch));
                if pure_num {
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 不可见字符（零宽 / BOM / 软连 / 方向控制 / 控制符）——与 `denoise_text`
/// 的删除清单同源。它们本来就会被洗掉，不该在任一侧的字数里占位。
pub fn is_invisible_char(c: char) -> bool {
    // 与 denoise_push 的**删除清单**同源。U+000B / U+000C 不在此列：
    // 它们被归一成空格，由 is_whitespace 在字数账里排除，不算「被删」。
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{0000}'..='\u{0008}'
            | '\u{000E}'..='\u{001F}'
            | '\u{007F}'..='\u{009F}'
    )
}

/// 清洗审计统计（供笺校台【原文对照】/ 清洗日志使用）。
///
/// 三项分别对应用户最关心的「有没有误杀」：
///   * `ghost`   —— 清除的**幽灵字符**数（零宽 / BOM / 软连字符 / 控制符。
///                  它们在竖排里全是空白格或豆腐块，删除不会丢任何可读字）；
///   * `variant` —— 折叠的**异体 / 康熙部首 / 兼容表意字**（只改 `VARIANT_MAP`
///                  与兼容区里有通式的字；生僻字一律保留，绝不篡改古籍）；
///   * `notes`   —— 转换的**异文 / 括号夹注**数（`〔…〕` `（…）` 升格为
///                  `[注]…[/注]` 的次数）。
///
/// ⚠️ 这些统计**只读不写**：枚举的是「若经清洗会改动的字符」，绝不为了计数
/// 而多删一个字。这是「防误杀」的透明凭证。
#[derive(Clone, Copy, Debug, Default)]
pub struct SanitizeStats {
    pub ghost: usize,
    pub variant: usize,
    pub notes: usize,
}

/// 在**不修改**任何文本的前提下，统计 `raw`（解析后的毛坯文本）若经清洗会产生的
/// 三类改动次数。供「清洗日志」如实汇报，让用户一眼看出清洗引擎动了哪些字、
/// 确保没把原书真字吞掉。
pub fn sanitize_stats(raw: &str) -> SanitizeStats {
    let mut ghost = 0usize;
    let mut variant = 0usize;
    for c in raw.chars() {
        match c {
            // 与 `denoise_text` 删除清单严格一致：零宽 / BOM / 软连 / 方向控制 /
            // 控制符——这些在竖排里全是空白格或豆腐块，删掉零风险。
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{0000}'..='\u{0008}'
            | '\u{000E}'..='\u{001F}' => ghost += 1,
            _ => {}
        }
        // 异体 / 康熙部首 / 兼容表意字归一：只动有通式的字，生僻字保留。
        if normalize_variant_char(c) != c {
            variant += 1;
        }
    }
    // 异文 / 括号注：先〔〕【】再（）（与管线同序），数 `[注]` 标记即升格次数。
    let noted = brackets_to_notes(raw);
    let noted = paren_to_note(&noted);
    let notes = noted.matches("[注]").count();
    SanitizeStats { ghost, variant, notes }
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
        // 跨章 / 跨页书眉（影印 / OCR 古籍每叶都印同一串卷题）：① 变尾码
        // （資治通鑑卷一…三/四/五）② 整行完全重复但每卷只跨几页（周紀一・威烈王・
        // 安王・烈王）单列「1/3 页重复」或「同前缀·仅页码不同」都认不出，这里两路
        // 并认。`purge_running_header_variants` 复用 `running_header_prefix_keys`，与
        // EPUB / PDF 共用同一把尺。md / txt / html / json / docx 走这一条。
        purge_running_header_variants(&mut ir, &book_title);
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

// 单段文本清洗的复用缓冲区：两块 `String` 乒乓，避免每段都重新分配。
// 分两个独立的线程局部单元，规避 `RefMut` 同帧多重可变借用的限制。
thread_local! {
    static CLEAN_A: RefCell<String> = RefCell::new(String::new());
    static CLEAN_B: RefCell<String> = RefCell::new(String::new());
}

/// 单段文本的完整清洗（去噪 + 括号夹注 + 异体归一），给导入向导 / 命令行用。
///
/// 通过线程局部复用缓冲区，把旧实现「三段各返回新 `String`」的 ~4 次全量分配
/// 压到只剩返回值的 1 次拷贝——百万字的丛书导入能少分配上百万次堆块。
pub fn clean_line(s: &str) -> String {
    CLEAN_A.with(|a| {
        let mut a = a.borrow_mut();
        denoise_text_into(s, &mut *a);
        if a.is_empty() || is_noise_para(&*a) {
            return String::new();
        }
        CLEAN_B.with(|b| {
            let mut b = b.borrow_mut();
            paren_to_note_into(&*a, &mut *b);
            normalize_variants_into(&*b, &mut *a);
            a.clone() // 线程局部缓冲区不能外借，复制出返回值（1 次分配）。
        })
    })
}

// ------------------------------------------------------------------
// 品 / 章 行内断行（v1.75.0）
// ------------------------------------------------------------------

/// 章节单位字：`第X__`。
///
/// 传统六种之外，v1.75.0 补了 **品 / 颂 / 偈 / 门 / 赞**——佛经与子书的分节
/// 单位。《金刚经》三十二品、《法华经》二十八品此前一个都不认，整段品题连
/// 着正文挤在同一段里，竖排出来就是「标题正文糊成一团」。
///
/// ⚠️ 没敢放「分」「经」「论」「传」：这四个在正文里太常见（「一分为二」
/// 「论语」），放进来必然误杀。宁可漏判，不可误杀。
pub const HEAD_UNITS: &[char] = &[
    '章', '回', '卷', '节', '篇', '折', '出', '品', '颂', '偈', '门', '赞',
];

/// 品题的收尾字：`法会因由分` / `威仪寂静分` / `序品` / `方便品` / `偈颂`。
///
/// 与 [`HEAD_UNITS`] 有交集（「品」既是单位也是品名尾）是**故意的**：
/// 「序品第一」这类后置序数里，「品」正是品名的一部分。
const NAME_TAILS: &[char] = &['分', '品', '颂', '赞', '偈', '门', '论', '篇'];

/// 断行前一格不许是这些「收尾的半个」——`《论语》第二十章说…` 是**引用**，
/// 不是品题，切一刀就把一句话劈成两半。
fn closes_quote(c: char) -> bool {
    matches!(c, '》' | '」' | '』' | '）' | '】' | '”' | '’' | '〉')
}

pub(crate) fn is_han(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{f900}'..='\u{faff}').contains(&c)
}

/// **行内品题断行**：把黏在正文里的「第X品 / 第X章…」连它的品名一起切出来，
/// 独占一行。
///
/// 典型症状（网上下载的《金刚经》_txt 常是一整坨）：
///
/// ```text
/// …是名一合相。第二十九品 威仪寂静分须菩提。若有人言。如来若来若去…
/// ```
///
/// 切完：
///
/// ```text
/// …是名一合相。
/// 第二十九品 威仪寂静分
/// 须菩提。若有人言。如来若来若去…
/// ```
///
/// 判据（**三条全中才切**，宁可漏判不可误杀）：
/// 1. 行内（或行首）出现 `第` + 数字 + 单位字；
/// 2. 单位字之后 12 字内有品名收尾字（见 [`NAME_TAILS`]），且收尾字后还
///    粘着 ≥20 字的内容——说明后面是整品正文，而不是「第二十章分析如下」
///    这种正文里的引用；
/// 3. 前一格不是收尾引号（挡掉 `《论语》第二十章…`）。
///
/// 只切**第一处**命中：一行里连着两个品号的概率极低，两刀叠着切会把行切碎。
pub fn split_inline_heads(text: &str) -> String {
    if !text.contains('第') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 64);
    for line in text.split('\n') {
        out.push_str(&split_one_head(line));
        out.push('\n');
    }
    // `split('\n')` 会多补一个换行（原串结尾那个），退回去保持行结构不变。
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn split_one_head(line: &str) -> String {
    if !line.contains('第') {
        return line.to_string();
    }
    let cs: Vec<char> = line.chars().collect();
    let n = cs.len();
    let mut i = 0usize;
    while i < n {
        if cs[i] != '第' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && crate::pipeline::is_numeral(cs[j]) {
            j += 1;
        }
        if j == i + 1 || j >= n || !HEAD_UNITS.contains(&cs[j]) {
            i += 1;
            continue;
        }
        let m = j + 1; // 单位字之后 = 品名起点
        if let Some(t) = name_tail_at(&cs, m) {
            // 前一格是收尾引号 → 多半是「《论语》第二十章」这类引用，不切。
            if i > 0 && closes_quote(cs[i - 1]) {
                i += 1;
                continue;
            }
            let head: String = cs[i..=t].iter().collect();
            let tail: String = cs[t + 1..].iter().collect();
            // 行首本来就是标题行：只补后刀，前面不加空段。
            return if i == 0 {
                format!("{head}\n{tail}")
            } else {
                let before: String = cs[..i].iter().collect();
                format!("{before}\n{head}\n{tail}")
            };
        }
        i += 1;
    }
    line.to_string()
}

/// 品名收尾字的下标：单位字后 12 格内、全是汉字 / 空格，遇标点即停。
fn name_tail_at(cs: &[char], m: usize) -> Option<usize> {
    let n = cs.len();
    // 后面得黏着一大段正文才值得切：只跟着三五个字的话，多半是正文里的引用。
    if n.saturating_sub(m) < 20 {
        return None;
    }
    let end = (m + 12).min(n);
    for k in m..end {
        let c = cs[k];
        if c == ' ' || c == '\u{3000}' || is_han(c) {
            if NAME_TAILS.contains(&c) && k + 1 < n {
                return Some(k);
            }
            continue;
        }
        break; // 标点 / 其它字符：品名到此为止
    }
    None
}


// ------------------------------------------------------------------
// 二之三、章题去重（v1.9.40）
//
// 真机截图里出现过「資治通鑑 資治通鑑 ◇ 卷第一」——病根有两处：
//   ① 源 txt 的卷题行就是「書名 ◇ 卷次」，`◇` 是分节装饰符；
//   ② 卷首题名那一列**已经**把书名当「大题」排在篇题之上，篇题再把书名
//      带一遍，就成了复读机。
// 这里先把章题洗成「只留篇题」（`clean_title`），渲染层不再自己拼书名；
// 旧书稿（已入库、没重洗过的 JSON）在 `Root::chapter_title` 再兜一道。
// ------------------------------------------------------------------

/// 章题里要剔除的**分节装饰符**：古籍版刻里表示「一节之终」，竖排里只会
/// 变成一枚孤零零的菱形，既不是字也不是句读。`○` **不在此列**——它可能
/// 是分节圈点，也是句号画法，交给句读层处理。
fn is_title_junk(c: char) -> bool {
    matches!(c, '◇' | '◆' | '※' | '·' | '•' | '・' | '＊' | '*')
}

/// **章题去重**：滤掉分节装饰符，并剥掉重复的书名前缀。
///
/// `資治通鑑 ◇ 卷第一` → `卷第一`；`資治通鑑資治通鑑卷第一` → `卷第一`；
/// `五帝本紀第一`（书名不在题内）→ 原样。
///
/// 只剥「题首恰好是书名」的情况，绝不按子串删——正文里的篇题若碰巧含
/// 书名中的字（《通鑑》里的「通」），按子串删会把篇题啃出窟窿。
pub fn clean_title(book_title: &str, raw: &str) -> String {
    // ① 装饰符一律剔除（含全角空格，它会在格子里留一块空白）。
    let mut t: String = raw
        .chars()
        .filter(|c| !is_title_junk(*c) && *c != '\u{3000}')
        .collect();
    t = collapse_blank_runs(&t).trim().to_string();
    let stripped: String = t.chars().filter(|c| !c.is_whitespace()).collect();

    // ② 剥重复书名（最多四道：`書名書名書名卷X` 这种复制粘贴的也收拾掉）。
    let bt: String = book_title
        .chars()
        .filter(|c| !c.is_whitespace() && !is_title_junk(*c))
        .collect();
    let bt_n = bt.chars().count();
    if bt_n >= 2 {
        // 题就是书名本身（整章标题 = 书名）：**数据层原样留着**——它仍是这一章
        // 的题（目录 / 中缝要用）；「别在纸面上写两遍」是渲染层的事
        // （`ancient_title_column` 见小题 == 大题就不画小题）。
        if stripped == bt {
            return stripped;
        }
        let mut cur = stripped.clone();
        for _ in 0..4 {
            if cur.chars().count() <= bt_n || !cur.starts_with(&bt) {
                break;
            }
            let rest: String = cur.chars().skip(bt_n).collect();
            let rest = rest
                .trim_start_matches(|c: char| matches!(c, '·' | '•' | '：' | ':' | '—' | '-' | ',' | '，' | '、'))
                .to_string();
            if rest.is_empty() {
                break;
            }
            cur = rest;
        }
        if cur.chars().count() < stripped.chars().count() {
            // ③ 剥过之后只剩标点 → 篇题本来就是空的。
            //    题内若**只有书名**（整章就是书名，如卷首序），剥完也是空：
            //    返回空串——卷首那一列的大题已经把书名排了一次，小题再写一遍
            //    就是复读机。调用方（章题列 / 中缝 / 目录）对空串都是「不写」。
            if cur.chars().any(|c| !c.is_whitespace() && !crate::types::is_punct(c)) {
                return cur;
            }
            // 剥完只剩标点：篇题本来就没有字，把书名还给调用方兜底。
            return stripped;
        }
        return stripped;
    }
    stripped
}

// ------------------------------------------------------------------
// 三、史论断段（v1.9.40）
//
// 刻本的论赞（「臣光曰」「太史公曰」「贊曰」）是**另起一列、低两格**的独立
// 段落。排版引擎（`gloss.rs`）只认「段首」，所以原文若把
// 「……为诸侯。臣光曰：……」挤在同一段里，那条规矩就永远不触发。
//
// OCR / 盗版 txt 里这种挤压极常见（整章一段、标点乱飞）。这里在导入时补
// 一刀：**句末之后紧跟论赞起手**才切，切点落在论赞首字上，前一段连句末
// 标点（含收尾引号）一起收尾。
//
// ⚠️ 只切段、不改字：切完 word 数、标点、注文偏移必须一一对得上——
// 下面 `split_praise_paragraph` 会同步重映射 `Note::at`，否则夹注会整体
// 错位到隔壁字上去。
// ------------------------------------------------------------------

// 句末标点 / 收尾符号的判据**只有一份**（`types::is_sentence_end` /
// `types::is_closer`）：断段、论赞另起、导出断句三处同取，免得改一处
// 忘另一处，切出两个口径。
use crate::types::{is_closer, is_sentence_end};

/// 找出「史论另起」的断点，返回**段内 char 下标**列表（新段首字的位置）。
///
/// 判据：句末标点 → （可选的收尾引号 / 空白）→ [`crate::types::is_praise_head`]。
/// 只取后面 8 个字做判据（`is_praise_head` 最长认到「臣安石曰」）。
pub fn praise_cut_points(text: &str) -> Vec<usize> {
    let cs: Vec<char> = text.chars().collect();
    let n = cs.len();
    let mut cuts: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < n {
        if !is_sentence_end(cs[i]) {
            i += 1;
            continue;
        }
        // 跳过收尾引号与空白：它们留在前一段。
        let mut j = i + 1;
        while j < n && (cs[j].is_whitespace() || is_closer(cs[j])) {
            j += 1;
        }
        if j >= n {
            break;
        }
        let head: String = cs[j..(j + 8).min(n)].iter().collect();
        if crate::types::is_praise_head(&head) && cuts.last() != Some(&j) {
            cuts.push(j);
        }
        // 无论切不切，都从 j 继续（跳过的一串已在 j 之前，不必重扫）。
        i = j.max(i + 1);
    }
    cuts
}

/// 把一段里「挤压在一起」的史论切成独立段落；没有断点就原样返回（零拷贝
/// 之外只 clone 一次）。
///
/// 注文偏移按段重映射：落在本段之外的一律丢弃（那是上一段 / 下一段的注），
/// 跨度越过段尾的截到段尾——绝不留下一个指向段外的 `at`。
pub fn split_praise_paragraph(p: &crate::types::Paragraph) -> Vec<crate::types::Paragraph> {
    let cuts = praise_cut_points(&p.text);
    if cuts.is_empty() {
        return vec![p.clone()];
    }
    let cs: Vec<char> = p.text.chars().collect();
    let mut bounds: Vec<usize> = Vec::with_capacity(cuts.len() + 2);
    bounds.push(0);
    for c in cuts {
        if c > *bounds.last().unwrap() && c < cs.len() {
            bounds.push(c);
        }
    }
    bounds.push(cs.len());

    let mut out: Vec<crate::types::Paragraph> = Vec::with_capacity(bounds.len() - 1);
    for w in bounds.windows(2) {
        let (mut s, e) = (w[0], w[1]);
        while s < e && cs[s].is_whitespace() {
            s += 1; // 段首空白不占格；偏移随之下移
        }
        if s >= e {
            continue;
        }
        let raw: String = cs[s..e].iter().collect();
        let text = raw.trim_end().to_string();
        let len = text.chars().count();
        if len == 0 {
            continue;
        }
        let mut notes = Vec::new();
        for nt in &p.notes {
            if nt.at < s || nt.at >= e {
                continue;
            }
            let mut nn = nt.clone();
            nn.at -= s;
            if nn.at >= len {
                continue; // 落在被裁掉的尾部空白里
            }
            if nn.at + nn.len > len {
                nn.len = len - nn.at;
            }
            notes.push(nn);
        }
        out.push(crate::types::Paragraph { text, notes });
    }
    if out.is_empty() {
        vec![p.clone()]
    } else {
        out
    }
}

/// 整本过一遍史论断段（导入收口调用；只动段落边界，一个字都不改）。
pub fn split_praise_in_book(book: &mut Book) {
    for ch in &mut book.chapters {
        // 绝大多数段落没有断点，`praise_cut_points` 一趟线性扫描 + 无分配，
        // 先探再决定要不要真切，避免给整本书每段都 clone 一份。
        if !ch
            .paragraphs
            .iter()
            .any(|p| !praise_cut_points(&p.text).is_empty())
        {
            continue;
        }
        let mut next: Vec<crate::types::Paragraph> = Vec::with_capacity(ch.paragraphs.len() + 8);
        for p in ch.paragraphs.drain(..) {
            next.extend(split_praise_paragraph(&p));
        }
        ch.paragraphs = next;
    }
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

    /// **正文字数不吃标签**（v1.9.70）：这是「314 万 → 317 万 净增 3 万」那个
    /// Bug 的回归钉子。夹注升格会凭空多出 `[注]` / `[/注]` 共 7 个字符，
    /// 但它们不是书里的字——按正文算，删了字就必须显示「净减」。
    #[test]
    fn lone_note_close_is_dropped_but_pairs_survive() {
        // 用户真机截图：跨行夹注被切开后，后半截只剩收标记。
        assert_eq!(drop_lone_note_closes("且俟知者。[/注]"), "且俟知者。");
        // 成对的壳原样留（双行夹注的开关，删了就丢注）。
        assert_eq!(
            drop_lone_note_closes("道[注]注文[/注]可"),
            "道[注]注文[/注]可"
        );
        // 未闭合的开标记**不动**（既有契约：吞后文、绝不丢字）。
        assert_eq!(drop_lone_note_closes("道[注]未尽"), "道[注]未尽");
        // 嵌套 / 多族壳各自记账。
        assert_eq!(
            drop_lone_note_closes("[/高亮]道[高亮]非[/高亮]常[/注]"),
            "道[高亮]非[/高亮]常"
        );
        // 不是自家壳的方括号（正文里的 `[1]`）一个字符都不许碰。
        assert_eq!(drop_lone_note_closes("卷[12]之末"), "卷[12]之末");
    }

    /// **三家注剥离**（v1.9.72）：`【集解】` 这类注家标签整段转双行夹注，
    /// 续行并入同一枚，遇到空行 / 章题立刻收口。
    #[test]
    fn three_jia_labels_become_inline_notes() {
        let raw = "黃帝者，少典之子。【集解】徐廣曰：號有熊。\n【索隱】案：有熊國君。\n\n下一段正文。";
        let s = mark_three_jia_notes(raw);
        assert!(!s.contains('【'), "注家标签没吃掉：{s}");
        assert!(s.contains("[注]集解：徐廣曰：號有熊。[/注]"), "{s}");
        assert!(s.contains("[注]索隱：案：有熊國君。[/注]"), "{s}");
        assert_eq!(
            s.matches("[注]").count(),
            s.matches("[/注]").count(),
            "标签必须成对：{s}"
        );
        // 空行收口：收标记必须紧贴注文末尾，下一段正文一个字都不进小字。
        assert!(s.ends_with("[/注]\n\n下一段正文。"), "正文被吞进夹注：{s}");
    }

    /// 续行（没有标签的下一行）并进同一枚夹注，且**不留换行**——
    /// 古籍硬折行的位置不是语义边界，留 `\n` 会在小字里凭空断行。
    #[test]
    fn note_continuation_joins_without_a_break() {
        let s = mark_three_jia_notes("【正義】此句疑衍，\n且俟知者。");
        assert_eq!(s, "[注]正義：此句疑衍，且俟知者。[/注]");
    }

    /// 章节体例标签不是注文：`【題解】` 一个字都不许动。
    #[test]
    fn chapter_label_is_not_a_note() {
        assert_eq!(
            mark_three_jia_notes("【題解】此章言道之體。"),
            "【題解】此章言道之體。"
        );
    }

    /// 纯数字标号整枚删；`[一]`、正文里真写着的方括号不碰。
    #[test]
    fn numeric_reference_marks_are_dropped() {
        assert_eq!(drop_reference_marks("道可道[1]，非常名[12]。"), "道可道，非常名。");
        assert_eq!(drop_reference_marks("［２］道"), "道");
        assert_eq!(drop_reference_marks("卷[一]之末"), "卷[一]之末");
        assert_eq!(drop_reference_marks("无标号"), "无标号");
    }

    #[test]
    fn body_count_ignores_note_tags_and_whitespace() {
        // 纯标签不计。
        assert_eq!(body_char_count("[注]注文[/注]"), 2);
        assert_eq!(body_char_count("[注][/注]"), 0);
        // 空白 / 换行 / 制表符不计；标点计（它是原书的一个字位）。
        assert_eq!(body_char_count("道 可\t道\n，"), 4);
        // 零宽与 BOM 不计。
        assert_eq!(body_char_count("道\u{200B}可\u{FEFF}道"), 3);
        // **真账**：毛坯 3 字夹注 → 升格后正文不变，绝不能算出「增字」。
        let raw = "天〔注〕地";
        let cleaned = "[注]天[/注]地";
        assert!(
            body_char_count(cleaned) <= body_char_count(raw),
            "升格夹注不许让正文字数变多：raw={} clean={}",
            body_char_count(raw),
            body_char_count(cleaned)
        );
    }

    #[test]
    fn denoise_kills_urls_pages_and_zero_width() {        assert!(is_noise_para("123"));
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
        // （）保守升格（内容不含句读、不超过 30 字）
        assert_eq!(
            paren_to_note("名可名（河上公曰：有名）非常名"),
            "名可名[注]河上公曰：有名[/注]非常名"
        );
        // 半角 () 同一口径
        assert_eq!(
            paren_to_note("名可名(河上公注)非常名"),
            "名可名[注]河上公注[/注]非常名"
        );
        // 含句读的不是注——那是正文自己带的括号
        assert_eq!(
            paren_to_note("他说（不对，其实是错的）。"),
            "他说（不对，其实是错的）。"
        );
        // 没闭合：绝不吞字
        assert_eq!(paren_to_note("道（可道"), "道（可道");
    }

    #[test]
    fn brackets_to_notes_unifies_all_bracket_kinds() {
        // 〔〕 / 【】 在**行内**一律升格——裸文本里这就是注疏 / 校勘记的正规写法
        assert_eq!(brackets_to_notes("甲〔注曰〕乙"), "甲[注]注曰[/注]乙");
        assert_eq!(brackets_to_notes("甲【注曰】乙"), "甲[注]注曰[/注]乙");
        // （）保守升格（无句读）
        assert_eq!(
            brackets_to_notes("名可名（河上公注）非常名"),
            "名可名[注]河上公注[/注]非常名"
        );
        // （）含句读不升格——那是正文自己带的括号
        assert_eq!(brackets_to_notes("他说（不对，错的）。"), "他说（不对，错的）。");
        // （）嵌在〔〕注内不嵌套，原样透传
        assert_eq!(brackets_to_notes("〔外（內）尾〕"), "[注]外（內）尾[/注]");
        // 半角 () 同一口径
        assert_eq!(
            brackets_to_notes("名可名(河上公注)非常名"),
            "名可名[注]河上公注[/注]非常名"
        );
        // 含句读不升格——那是正文自己带的括号（开闭括号统一归一到全角）
        assert_eq!(
            brackets_to_notes("他说(不对，错的)。"),
            "他说（不对，错的）。"
        );
        // 严格标记 [note] / [/note] → [注] / [/注]（整段吃掉，不留残 `]`）
        assert_eq!(
            brackets_to_notes("道[note]a note[/note]名"),
            "道[注]a note[/注]名"
        );
        // 行首的短标签是**章节体例**，原样保留；后面的半角 () 照旧升格。
        assert_eq!(
            brackets_to_notes("【题解】(注曰)乙"),
            "【题解】[注]注曰[/注]乙"
        );
        // 行首但内容带句读 → 是注疏不是标签，照旧升格。
        assert_eq!(
            brackets_to_notes("〔名午，考王之子〕是為威烈王"),
            "[注]名午，考王之子[/注]是為威烈王"
        );
        // 行内的方头括注照样升格：`【乙丙】` 是括注不是标签。
        assert_eq!(brackets_to_notes("甲【乙丙】丁"), "甲[注]乙丙[/注]丁");
        // 行首但超长（>8 字）→ 是注疏不是标签。
        assert_eq!(
            brackets_to_notes("【此章言道之體開卷之總綱】乙"),
            "[注]此章言道之體開卷之總綱[/注]乙"
        );
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

    // ---- v1.9.40：章题去重（书名复读机）----------------------------------

    #[test]
    fn chapter_title_drops_repeated_book_name_and_section_mark() {
        // 真机截图那一例：`資治通鑑 ◇ 卷第一`（◇ 是分节符，空格是半角）。
        assert_eq!(clean_title("資治通鑑", "資治通鑑 ◇ 卷第一"), "卷第一");
        // 复制粘贴造成的三连书名。
        assert_eq!(
            clean_title("資治通鑑", "資治通鑑資治通鑑資治通鑑卷第一"),
            "卷第一"
        );
        // 全角空格 + 中点。
        assert_eq!(clean_title("資治通鑑", "資治通鑑　·　卷第二"), "卷第二");
        // 书名不在题内：一个字都不许动。
        assert_eq!(clean_title("史記", "五帝本紀第一"), "五帝本紀第一");
        // 题内只有书名（整章就是书名）：数据层原样留着，渲染层才不会把
        // 目录 / 中缝的条目也一并抹成空。
        assert_eq!(clean_title("史記", "史記"), "史記");
        // 空书名：只清装饰符。
        assert_eq!(clean_title("", "◇ 卷第一"), "卷第一");
        // 长书名（四字以上）同样只剥前缀、不按子串删。
        assert_eq!(
            clean_title("資治通鑑", "資治通鑑外紀卷第一"),
            "外紀卷第一"
        );
    }

    #[test]
    fn dedup_titles_is_idempotent() {
        let mut b = Book::new("資治通鑑", "司馬光");
        b.chapters.push(Chapter {
            title: "資治通鑑 ◇ 卷第一".into(),
            paragraphs: vec![Paragraph::plain("周紀一")],
        });
        b.chapters.push(Chapter {
            title: "卷第二".into(),
            paragraphs: vec![Paragraph::plain("周紀二")],
        });
        b.dedup_titles();
        let first: Vec<String> = b.chapters.iter().map(|c| c.title.clone()).collect();
        assert_eq!(first, vec!["卷第一".to_string(), "卷第二".to_string()]);
        b.dedup_titles();
        let second: Vec<String> = b.chapters.iter().map(|c| c.title.clone()).collect();
        assert_eq!(first, second, "洗两遍结果必须一样");
    }

    // ---- v1.9.40：史论断段 ----------------------------------------------

    #[test]
    fn praise_runs_are_split_into_their_own_paragraphs() {
        let p = Paragraph::plain("齊威王薨。臣光曰：臣聞天子之職莫大於禮。");
        let out = split_praise_paragraph(&p);
        assert_eq!(out.len(), 2, "史论没有断开：{out:?}");
        assert_eq!(out[0].text, "齊威王薨。");
        assert!(out[1].text.starts_with("臣光曰"), "第二段：{}", out[1].text);
        // 逗号后面是句中转引，绝不断。
        let q = Paragraph::plain("王曰，臣光曰不可，遂止。");
        assert_eq!(split_praise_paragraph(&q).len(), 1, "逗号处被误断");
        // 没有史论的段落原样返回（只有一份，不是零份）。
        let r = Paragraph::plain("道可道，非常道。");
        let out = split_praise_paragraph(&r);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, r.text);
        // 闭引号归前段，不甩到下一段开头。
        let s2 = Paragraph::plain("子曰「學而時習之。」贊曰蓋聞有虞氏之德。");
        let out = split_praise_paragraph(&s2);
        assert_eq!(out.len(), 2);
        assert!(out[0].text.ends_with('」'), "闭引号被甩到下一段：{}", out[0].text);
    }

    #[test]
    fn splitting_paragraphs_remaps_note_offsets() {
        let mut p = Paragraph::plain("甲乙。臣光曰丙丁");
        p.notes.push(crate::types::Note::new(1, crate::types::NoteKind::Note, "乙注"));
        p.notes.push(crate::types::Note::new(3, crate::types::NoteKind::Note, "臣注"));
        let out = split_praise_paragraph(&p);
        assert_eq!(out.len(), 2);
        // 第一段「甲乙。」：只剩指向「乙」的那条，偏移仍是 1。
        assert_eq!(out[0].notes.len(), 1, "注没跟着段走");
        assert_eq!(out[0].notes[0].at, 1);
        assert_eq!(out[0].notes[0].text, "乙注");
        // 第二段「臣光曰丙丁」：原偏移 3 是「臣」，段内应变成 0。
        assert_eq!(out[1].notes.len(), 1);
        assert_eq!(out[1].notes[0].at, 0, "注偏移没重映射");
        assert_eq!(out[1].notes[0].text, "臣注");
        // 字一个不少（拼回去等于原文）。
        let rejoined: String = out.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(rejoined, "甲乙。臣光曰丙丁");
    }

    #[test]
    fn praise_split_touches_only_book_that_needs_it() {
        let mut b = Book::new("資治通鑑", "司馬光");
        b.chapters.push(Chapter {
            title: "卷第一".into(),
            paragraphs: vec![
                Paragraph::plain("周紀一。臣光曰：臣聞天子之職莫大於禮。"),
                Paragraph::plain("道可道，非常道。"),
            ],
        });
        split_praise_in_book(&mut b);
        assert_eq!(b.chapters[0].paragraphs.len(), 3);
        assert_eq!(b.chapters[0].paragraphs[0].text, "周紀一。");
        assert!(b.chapters[0].paragraphs[1].text.starts_with("臣光曰"));
        assert_eq!(b.chapters[0].paragraphs[2].text, "道可道，非常道。");
    }

    // ── 标记壳闸（v1.9.56）──────────────────────────────────────────

    /// 壳剥干净、壳里的字一个不少——这是「剥壳 ≠ 吞字」的边界。
    #[test]
    fn strip_note_tags_keeps_the_words_inside() {
        assert_eq!(
            strip_note_tags("卷第三[注]周纪三[/注]起重光赤奋若"),
            "卷第三周纪三起重光赤奋若"
        );
        assert_eq!(strip_note_tags("甲[疏]疏曰[/疏]乙"), "甲疏曰乙");
        assert_eq!(strip_note_tags("[高亮]道[/高亮]可道"), "道可道");
        // 空壳：什么都不剩，也绝不吐括号。
        assert_eq!(strip_note_tags("道[注][/注]可道"), "道可道");
        // 没有壳的段落原样返回（热路径不复制内容也不改一字）。
        assert_eq!(strip_note_tags("道可道，非常道。"), "道可道，非常道。");
    }

    /// **不是自家壳的方括号一个都不许动**：`[1]` 是年份注、`[[…]]` 是
    /// 文献里常见的引文标记，都是正文的一部分。
    #[test]
    fn strip_note_tags_never_touches_foreign_brackets() {
        assert_eq!(strip_note_tags("見[1]注"), "見[1]注");
        assert_eq!(strip_note_tags("案[案]云"), "案[案]云");
        // 唯一的例外是自家那三对壳，它们藏在别处也必须被剥。
        assert_eq!(strip_note_tags("見[1][注]注[/注]"), "見[1]注");
    }

    /// 掩码与字符**逐位对齐**：只盖壳、不盖壳里的字，且长度恒等于字数。
    #[test]
    fn note_tag_mask_is_char_aligned() {
        let s = "甲[注]乙[/注]丙";
        let m = note_tag_mask(s);
        assert_eq!(m.len(), s.chars().count(), "掩码长度必须等于字数");
        let masked: String = s
            .chars()
            .enumerate()
            .filter(|(i, _)| m[*i])
            .map(|(_, c)| c)
            .collect();
        assert_eq!(masked, "[注][/注]");
        // 无 `[` 的段落返回空表（零分配快路）。
        assert!(note_tag_mask("道可道").is_empty());
    }

    /// 空格 / 全角斜杠 / 大小写都认：`[ / 注 ]` `[／疏]` `[/NOTE]`。
    #[test]
    fn note_tag_mask_tolerates_spaces_and_full_width_slash() {
        assert_eq!(strip_note_tags("甲[ / 注 ]乙[／注]丙"), "甲乙丙");
        assert_eq!(strip_note_tags("甲[/NOTE]乙"), "甲乙");
        // `[拼音:…]` / `[数字:…]` 是**带参标记**：壳与参数一起下纸，
        // 参数绝不能当正文排（`dào` 三个拉丁字母竖着摞成三格是灾难）。
        assert_eq!(strip_note_tags("道[拼音:dào]可道"), "道可道");
        assert_eq!(strip_note_tags("凡[数字:35]葉"), "凡葉");
    }

    // ---- v1.9.88：书眉剔除（变尾码 + 整行重复，跨格式共用）-------------------

    #[test]
    fn strip_page_tail_peels_trailing_page_numbers() {
        // 汉字页码尾码。
        assert_eq!(strip_page_tail("資治通鑑卷一威烈王三"), ("資治通鑑卷一威烈王".into(), "三".into()));
        // 阿拉伯数字尾码。
        assert_eq!(strip_page_tail("資治通鑑卷一威烈王3"), ("資治通鑑卷一威烈王".into(), "3".into()));
        // 罗马数字尾码。
        assert_eq!(strip_page_tail("資治通鑑卷一威烈王iv"), ("資治通鑑卷一威烈王".into(), "iv".into()));
        // 无尾码：整串都是前缀。
        assert_eq!(strip_page_tail("周威烈王二十三年"), ("周威烈王二十三年".into(), String::new()));
        // 纯页码串：前缀为空，整串当尾码（这种不是书眉）。
        assert_eq!(strip_page_tail("一二三"), (String::new(), "一二三".into()));
    }

    #[test]
    fn header_norm_key_drops_whitespace_only() {
        assert_eq!(header_norm_key("資 治 通 鑑"), "資治通鑑");
        assert_eq!(header_norm_key("資治通鑑卷一　三"), "資治通鑑卷一三");
    }

    #[test]
    fn running_header_prefix_keys_catches_variant_page_tails() {
        // 竖排影印《資治通鑑》：书眉每叶同串、只末位页码不同。
        let lines = vec![
            "資治通鑑卷一周紀一威烈王安王烈王三".to_string(),
            "資治通鑑卷一周紀一威烈王安王烈王三".to_string(),
            "資治通鑑卷一周紀一威烈王安王烈王三".to_string(),
            "資治通鑑卷一周紀一威烈王安王烈王四".to_string(),
            "資治通鑑卷一周紀一威烈王安王烈王四".to_string(),
            "資治通鑑卷一周紀一威烈王安王烈王五".to_string(),
            // 正文短句：只出现一次，绝不误杀。
            "周威烈王二十三年初命晉大夫".to_string(),
        ];
        let (structure, _title) = running_header_prefix_keys(&lines, "");
        let drop: std::collections::HashSet<String> = structure.into_iter().collect();
        assert!(drop.contains(&skeleton_key("資治通鑑卷一周紀一威烈王安王烈王三")));
        assert!(drop.contains(&skeleton_key("資治通鑑卷一周紀一威烈王安王烈王四")));
        assert!(drop.contains(&skeleton_key("資治通鑑卷一周紀一威烈王安王烈王五")));
        assert!(!drop.contains(&skeleton_key("周威烈王二十三年初命晉大夫")));
    }

    #[test]
    fn running_header_prefix_keys_does_not_kill_plain_repeats() {
        // 同尾码重复（没有页码变化）不该被这把尺删：那是「整行完全重复」的活，
        // 由各自的 strip_running_lines / purge_running_headers 处理，互不越界。
        let lines = vec![
            "章題一".to_string(),
            "章題一".to_string(),
            "章題一".to_string(),
        ];
        let (s, t) = running_header_prefix_keys(&lines, "");
        assert!(s.is_empty() && t.is_empty(), "同尾码重复不误杀");
        // 样本太小（<3 条）也不动。
        let few = vec!["資治通鑑卷一三".to_string(), "資治通鑑卷一四".to_string()];
        let (s2, t2) = running_header_prefix_keys(&few, "");
        assert!(s2.is_empty() && t2.is_empty(), "尾码不足不误杀");
        // 纯数字串不是书眉。
        let numerals = vec![
            "一二三".to_string(),
            "一二三".to_string(),
            "一二四".to_string(),
            "一二五".to_string(),
        ];
        let (s3, t3) = running_header_prefix_keys(&numerals, "");
        assert!(s3.is_empty() && t3.is_empty(), "纯数字串不误杀");
    }

    /// 真实《資治通鑑》影印本暴露的形态：书眉是**整行完全重复、但每卷只跨几页**
    /// 的卷题（无变尾码），旧 `strip_running_lines` 的 1/3 页门槛与「同前缀·仅页码不同」
    /// 都认不出；新函数按「书眉形状」补刀，必须删掉。
    #[test]
    fn running_header_prefix_keys_drops_exact_repeat_volume_headers() {
        // 取自真实 PDF 探针：这些串每叶印一遍、整本只出现十几回。
        let lines = vec![
            "周紀一・威烈王・安王・烈王".to_string(),
            "周紀一・威烈王・安王・烈王".to_string(),
            "周紀一・威烈王・安王・烈王".to_string(),
            "周紀一・威烈王・安王・烈王".to_string(),
            "周紀二・顯王".to_string(),
            "周紀二・顯王".to_string(),
            "周紀二・顯王".to_string(),
            "周紀二・顯王".to_string(),
            "周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "資治通鑑卷二周紀二・顯王".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            "卷".to_string(),
            // 正文短句（含「王」但不含结构标记、且只出现一次）：绝不可误杀。
            "周威烈王二十三年初命晉大夫".to_string(),
        ];
        let (structure, _t) = running_header_prefix_keys(&lines, "");
        let drop: std::collections::HashSet<String> = structure.into_iter().collect();
        assert!(drop.contains(&skeleton_key("周紀一・威烈王・安王・烈王")));
        assert!(drop.contains(&skeleton_key("周紀二・顯王")));
        assert!(drop.contains(&skeleton_key("資治通鑑卷二周紀二・顯王")));
        assert!(drop.contains(&skeleton_key("卷")));
        assert!(!drop.contains(&skeleton_key("周威烈王二十三年初命晉大夫")));
    }

    /// 骨架归一的核心价值：把 `資治通鑑卷一` / `資治通鑑卷第一・周紀一…` /
    /// `資治通鑑卷一周紀一…二` 全部坍缩到同一骨架，再用频率（≥3）一网打尽——
    /// 真实《資治通鑑》影印本里「第X」中缀、页码尾码都是同一书眉的变体。
    #[test]
    fn skeleton_normalization_collapses_numeral_variants() {
        let lines = vec![
            "資治通鑑卷一".to_string(),
            "資治通鑑卷一".to_string(),
            "資治通鑑卷一".to_string(),
            "資治通鑑卷第一・周紀一・威烈王・安王・烈王".to_string(),
            "資治通鑑卷第一・周紀一・威烈王・安王・烈王".to_string(),
            "資治通鑑卷第一・周紀一・威烈王・安王・烈王".to_string(),
            "資治通鑑卷一周紀一・威烈王・安王・烈王二".to_string(),
            "資治通鑑卷一周紀一・威烈王・安王・烈王二".to_string(),
            "資治通鑑卷一周紀一・威烈王・安王・烈王二".to_string(),
            "資治通鑑卷一周紀一・威烈王・安王・烈王三".to_string(),
            "資治通鑑卷一周紀一・威烈王・安王・烈王三".to_string(),
            "資治通鑑卷一周紀一・威烈王・安王・烈王三".to_string(),
        ];
        let (structure, _t) = running_header_prefix_keys(&lines, "資治通鑑");
        assert!(structure.contains(&skeleton_key("資治通鑑卷一")), "卷一变体必删");
        assert!(
            structure.contains(&skeleton_key("資治通鑑卷第一・周紀一・威烈王・安王・烈王")),
            "第X形必删"
        );
        assert!(
            structure.contains(&skeleton_key("資治通鑑卷一周紀一・威烈王・安王・烈王二")),
            "尾码形必删"
        );
    }

    /// 防御性（用户原话「资治通鉴」）：正文里三次提到书名（嵌入句、非独立成行）
    /// 必须保留；但独立成行、重复三遍的书名（影印书口字）必须删。
    #[test]
    fn body_mentioning_title_thrice_is_kept_but_standalone_header_dropped() {
        let title = "資治通鑑";
        // ① 正文嵌入书名 ×3：绝不误杀。
        let body = vec![
            "前文已述資治通鑑乃編年巨著。".to_string(),
            "前文已述資治通鑑乃編年巨著。".to_string(),
            "前文已述資治通鑑乃編年巨著。".to_string(),
        ];
        let (s, t) = running_header_prefix_keys(&body, title);
        assert!(s.is_empty() && t.is_empty(), "正文嵌入书名不误杀");
        // ② 独立成行、重复三遍的书名（影印书口字）→ 必须删（书名集命中）。
        let header = vec![
            "資治通鑑".to_string(),
            "資治通鑑".to_string(),
            "資治通鑑".to_string(),
        ];
        let (_s, t2) = running_header_prefix_keys(&header, title);
        assert!(t2.contains(&skeleton_key("資治通鑑")), "独立成行的书名书眉必须删");
    }

    /// 防御性（用户原话「凡三十五年」）：正文第一句就是带数字的，绝不能误杀——
    /// 它不含任何书眉结构标记（卷 / 纪 / 册 / 集 / 部），再怎么重复都不该被书眉尺删掉。
    #[test]
    fn body_with_leading_numerals_is_never_killed() {
        // ① 整句重复出现多次：仍必须保留。
        let repeated = vec![
            "凡三十五年，天下大旱".to_string(),
            "凡三十五年，天下大旱".to_string(),
            "凡三十五年，天下大旱".to_string(),
            "凡三十五年，天下大旱".to_string(),
        ];
        let (s, t) = running_header_prefix_keys(&repeated, "");
        assert!(
            s.is_empty() && t.is_empty(),
            "『凡三十五年』整句重复也绝不可误杀"
        );
        // ② 孤行一次：保留。
        let once = vec!["起著雍攝提格盡玄黓困敦凡三十五年".to_string()];
        let (s2, t2) = running_header_prefix_keys(&once, "");
        assert!(
            s2.is_empty() && t2.is_empty(),
            "《資治通鑑》开篇纪年句含『凡三十五年』不可误杀"
        );
        // ③ 干支纪年重复：不含结构标记，保留。
        let gan = vec![
            "著雍攝提格".to_string(),
            "著雍攝提格".to_string(),
            "著雍攝提格".to_string(),
        ];
        let (s3, t3) = running_header_prefix_keys(&gan, "");
        assert!(
            s3.is_empty() && t3.is_empty(),
            "干支纪年重复也不可误杀"
        );
    }

    /// 边界（长度闸门）：一条很长的「正文」即便含『卷』也绝不因（B）路被当书眉删——
    /// 结构标记只在短行（≤ RH_MAX_CHARS）才生效，长正文豁免。
    #[test]
    fn long_body_line_with_volume_char_survives() {
        let long = vec![
            "昔者周室既衰諸侯力政強弱相劫君臣易位上下失序而孔子修春秋以正名分其文雖簡而義甚嚴學者不可不察焉且著於卷末以告後世".to_string(),
            "昔者周室既衰諸侯力政強弱相劫君臣易位上下失序而孔子修春秋以正名分其文雖簡而義甚嚴學者不可不察焉且著於卷末以告後世".to_string(),
            "昔者周室既衰諸侯力政強弱相劫君臣易位上下失序而孔子修春秋以正名分其文雖簡而義甚嚴學者不可不察焉且著於卷末以告後世".to_string(),
        ];
        let (s, t) = running_header_prefix_keys(&long, "");
        assert!(
            s.is_empty() && t.is_empty(),
            "超长正文含『卷』也不该被删"
        );
    }

}
