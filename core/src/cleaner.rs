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
    false
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
pub fn denoise_text(s: &str) -> String {
    let mut s = s.to_string();
    // HTML 残留（清洗漏网的 `<span>`、`&nbsp;`）：先剥标签再解实体。
    if s.contains('<') && s.contains('>') {
        s = crate::pipeline::strip_tags(&s);
    }
    if s.contains('&') {
        s = crate::pipeline::decode_entities(&s);
    }
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

/// 第一遍：一巻内的去噪（文本净化 + 丢噪声段 + 合并连续重复段 + 砍导航短段堆）。
fn pass1_juan(j: &mut IrJuan, titles: &[String]) {
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
    // 丢噪声段（含净化后变空的段）
    j.blocks.retain(|b| match b {
        IrBlock::Para(spans) | IrBlock::Annotation { spans, .. } => {
            let t = spans_line(spans);
            !t.trim().is_empty() && !is_noise_para(&t)
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
fn drop_nav_runs(blocks: &mut Vec<IrBlock>, titles: &[String]) {
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
    let titles: Vec<String> = ir.juans.iter().map(|j| j.title.clone()).collect();

    // ---- 第一遍：去噪（所有格式都做，减法最安全） ----
    if opts.denoise {
        for j in ir.juans.iter_mut() {
            pass1_juan(j, &titles);
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
}
