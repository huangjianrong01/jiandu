//! **统一清洗层**：全格式唯一的出口（漏斗的第二个口）。
//!
//! 任何一种格式解析出来的纯文本，都必须从这里过一遍再进 `pipeline`。
//! 这一层只做「文本卫生」，**不做**断章、不做标点竖排化、不改字——那些是
//! core 管线的活。越界就会二次加工，把本来干净的稿子改脏。
//!
//! 脏数据在真实电子书里长什么样（都是实测撞出来的）：
//!
//! ```text
//!   零宽字符   \u{200B} 夹在「道\u{200B}可道」里 → 排版时字格里空一格
//!   BOM        \u{FEFF} 在文件头 → 首章标题识别不到
//!   软换行     一句诗被硬折成三行 → 断章时变成三段
//!   全角半角   "1,2" 与 "１，２" 混排 → 同一本书两种标点
//!   简繁混排   正文繁体、夹注简体 → 字书查不到字
//!   注疏不闭合 「（案：此句疑衍」→ 竖排里括号一直挂到段末
//! ```

use jiandu_core::trad;

/// 清洗开关。默认档刻意保守：只做**不改变语义**的卫生处理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanOptions {
    /// 过滤零宽 / 方向控制 / 不可见分隔符（默认开）。
    pub zero_width: bool,
    /// ASCII 数字 → 全角数字（`１`）。
    ///
    /// **默认关**，原因写在 [`fullwidth_digits`] 的文档里：core 的「数字汉字化」
    /// 与「回目解析」只认 ASCII 数字，开了会把「第12回」变成「第１２回」，
    /// 回目识别当场失灵、汉字化也失效。竖排古籍确要全角数字时再单独打开。
    pub fullwidth_digits: bool,
    /// 半角标点 → 全角（仅当左右紧邻汉字时，默认开）。
    pub cjk_punct: bool,
    /// 简繁混排归一（方向只朝简体，默认开）。
    pub unify_script: bool,
    /// 闭合括号 / 注疏标签（默认开）。
    pub close_notes: bool,
}

impl Default for CleanOptions {
    fn default() -> Self {
        Self {
            zero_width: true,
            fullwidth_digits: false,
            cjk_punct: true,
            unify_script: true,
            close_notes: true,
        }
    }
}

impl CleanOptions {
    /// 竖排古籍档：连数字也要全角（正文好看），但**回目里的数字仍然保留
    /// 半角**——否则章节切不开。
    pub fn for_vertical() -> Self {
        Self {
            fullwidth_digits: true,
            ..Self::default()
        }
    }
}

/// 清洗主入口（默认档）。
pub fn clean(raw: &str) -> String {
    clean_with(raw, &CleanOptions::default())
}

/// 带开关的清洗。顺序有讲究，别乱调：
///
/// ```text
///   换行归一 → 去不可见字符 → 空白规整 → 标点 → 简繁 → 闭合注疏
///   （简繁必须在标点之后：否则全角化会把刚折好的字再判一次）
/// ```
pub fn clean_with(raw: &str, opt: &CleanOptions) -> String {
    let mut s = normalize_newlines(raw);

    if opt.zero_width {
        s = strip_invisible(&s);
    }
    s = tidy_whitespace(&s);

    if opt.cjk_punct {
        s = cjk_punct(&s);
    }
    if opt.fullwidth_digits {
        s = fullwidth_digits(&s);
    }
    if opt.unify_script {
        s = unify_script(&s);
    }
    if opt.close_notes {
        s = close_notes(&s);
    }
    s = fix_common_typos(&s);

    tidy_blocks(&s)
}

// ----------------------------------------------------------------------
// 一、逐项清洗
// ----------------------------------------------------------------------

/// `\r\n` / `\r` → `\n`（老 TXT、Windows 导出的 EPUB 都有）。
fn normalize_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                out.push('\n');
            }
            _ => out.push(c),
        }
    }
    out
}

/// 去掉零宽字符、方向控制符、孤立控制字符。
///
/// 这些字符**看不见但占位**：一个 `\u{200B}` 就能让「道可道」在竖排里多出一
/// 个空字格，把整列顶歪。BOM 还会让首行标题认不出来（比对时永远差一个字节）。
fn strip_invisible(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                *c,
                '\u{200B}'..='\u{200F}'   // 零宽空格 / 连接符 / 方向标记
                | '\u{202A}'..='\u{202E}' // 方向嵌入 / 覆盖
                | '\u{2060}'..='\u{2064}' // word joiner / 不可见运算符
                | '\u{FEFF}'              // BOM
                | '\u{00AD}'              // 软连字符
                | '\u{180E}'              // 蒙古文元音分隔符
                | '\u{0000}'..='\u{0008}'
                | '\u{000B}'
                | '\u{000C}'
                | '\u{000E}'..='\u{001F}'
            )
        })
        .collect()
}

/// 空白规整：NBSP / 窄空格 → 普通空格；行尾空白剪掉。
///
/// **全角空格 `\u{3000}` 必须留着**：它是古籍稿里唯一的段首缩进记号，
/// 变成半角空格后「段首缩进」这条信息就没了。
fn tidy_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut l = String::with_capacity(line.len());
        for c in line.chars() {
            match c {
                '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\u{2000}'..='\u{200A}' | '\u{2028}'
                | '\u{2029}' => l.push(' '),
                '\t' => l.push_str("　　"), // 制表位 = 两个全角空格（段首缩进等价）
                _ => l.push(c),
            }
        }
        // 行尾空白（含全角空格）剪掉：竖排里行尾空格会顶出一个空格。
        while l.ends_with(' ') || l.ends_with('　') {
            l.pop();
        }
        out.push_str(&l);
    }
    out
}

/// 半角标点 → 全角，**只在中日韩汉字旁边动手**。
///
/// 判据用「左或右邻是汉字」：`3.14`、`a,b`、`http://` 这类西文串绝不能被
/// 改成「３．１４」「ａ，ｂ」——那才是真的把书改脏。
fn cjk_punct(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        let left_cjk = i.checked_sub(1).and_then(|j| chars.get(j)).is_some_and(|c| is_cjk(*c));
        let right_cjk = chars.get(i + 1).is_some_and(|c| is_cjk(*c));
        // 紧邻汉字，且不是数字旁边（`第1,2回` 里的逗号也算标点，但 `1,000` 不算）。
        let near_num = i
            .checked_sub(1)
            .and_then(|j| chars.get(j))
            .is_some_and(|c| c.is_ascii_digit() || *c == '１')
            || chars.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == '１');
        let mapped = match c {
            ',' if (left_cjk || right_cjk) && !near_num => '，',
            '.' if (left_cjk || right_cjk) && !near_num => '。',
            ';' if left_cjk || right_cjk => '；',
            ':' if left_cjk || right_cjk => '：',
            '!' if left_cjk || right_cjk => '！',
            '?' if left_cjk || right_cjk => '？',
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push(mapped);
    }
    out
}

/// ASCII 数字 → 全角数字（`0`..`9` → `０`..`９`），**跳过回目里的数字**。
///
/// # 为什么默认关闭（重要）
///
/// core 的两处关键逻辑只认 ASCII 数字：
///   * `pipeline` 的「数字汉字化」（`第 3 章` → `第三章`）；
///   * 回目解析（`第12回` 的名字提取，`take_while(is_ascii_digit)`）。
///
/// 全角化一旦跑在它们前面，「第12回」变「第１２回」——回目切不开、汉字化也
/// 失灵，整本书会退化成一个大章。所以本函数只做「正文数字」的全角化，
/// **`第 N 回 / 卷 N / 章 N` 这类结构标记里的数字原样保留**，且默认不启用。
/// 竖排真要全角数字时，用 [`CleanOptions::for_vertical`] 显式打开。
fn fullwidth_digits(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    // 结构位判定：从这一位往前跳过连续数字，撞上的第一个字若是「第/卷/回…」
    // 就说明这串数字是回目编号，必须留半角。
    let structural_at = |i: usize| -> bool {
        let mut j = i;
        while j > 0 {
            j -= 1;
            if chars[j].is_ascii_digit() {
                continue;
            }
            return matches!(
                chars[j],
                '第' | '卷' | '回' | '章' | '节' | '篇' | '部' | '集' | '册' | '本'
            );
        }
        false
    };
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_digit() || structural_at(i) {
            out.push(c);
        } else {
            out.push(char::from_u32(0xFF10 + (c as u32 - '0' as u32)).unwrap_or(c));
        }
    }
    out
}

/// 简繁混排归一：**只朝简体方向折**。
///
/// 为什么只朝一个方向——汉字简化是「多对一」（乾/幹/干 → 干），反着走是
/// 「一对多」，机器没有上下文可分：把「干戈」折成「乾戈」、「干部」折成
/// 「乾部」，等于把书改错。故：
///   * 简体为主（现代书） → 夹在里面的繁体字一律折成简体（安全）；
///   * 繁体为主（古籍）   → **一个字都不动**（避免拍脑袋造错字），
///     只在极端混排时保持原样交给人工。
fn unify_script(s: &str) -> String {
    let mut cjk = 0usize;
    let mut simp = 0usize; // 有繁体对应的简体字
    let mut trad = 0usize; // 有简体对应的繁体字
    for c in s.chars() {
        if !is_cjk(c) {
            continue;
        }
        cjk += 1;
        if trad::traditional_of(c).is_some() {
            simp += 1;
        }
        if trad::simplified_of(c).is_some() {
            trad += 1;
        }
    }
    // 样本太小（一节诗、一条批注）不做判断，免得把繁体书名折坏。
    if cjk < 240 || simp == 0 || trad == 0 {
        return s.to_string();
    }
    // 繁体占优 → 不动。
    if simp < trad {
        return s.to_string();
    }
    // 混排比例低于 2% 视为本来就干净，不做无谓改写。
    if trad * 50 <= simp {
        return s.to_string();
    }
    s.chars()
        .map(|c| trad::simplified_of(c).unwrap_or(c))
        .collect()
}

/// 闭合括号与注疏标签：**只补不删**。
///
/// 竖排里一个没闭合的「（」会一路挂到段末，把后面整段字都圈成夹注；
/// `[注]` 不闭合则会把余下全文吞进夹注。补在行末（不是文末）——按段收口，
/// 才不会被读者当成跨段的长注。
fn close_notes(s: &str) -> String {
    const PAIRS: [(char, char); 8] = [
        ('（', '）'),
        ('〔', '〕'),
        ('［', '］'),
        ('【', '】'),
        ('《', '》'),
        ('〈', '〉'),
        ('「', '」'),
        ('『', '』'),
    ];
    let mut out = String::with_capacity(s.len() + 8);
    for (n, line) in s.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let mut depth: Vec<char> = Vec::new();
        for c in line.chars() {
            if let Some(&(_, close)) = PAIRS.iter().find(|(o, _)| *o == c) {
                depth.push(close);
            } else if let Some(pos) = depth.iter().rposition(|x| *x == c) {
                depth.remove(pos);
            }
            out.push(c);
        }
        // 未闭合的按「后开先闭」补在行末。
        while let Some(close) = depth.pop() {
            out.push(close);
        }
        // `[注]` / `[高亮]` 这类开关标签：开着的补一个 `[/注]`。
        let opens = line.matches("[注]").count();
        let closes = line.matches("[/注]").count();
        for _ in closes..opens {
            out.push_str("[/注]");
        }
    }
    out
}

/// 常见误字表（**只收「另一个写法一定是错」的**，宁缺毋滥）。
///
/// 古籍电子文本最常见的错不是「字不认识」，而是「同音字手滑」与「简繁
/// 混着敲」。这张表只放**改动方向唯一**的几条；凡是需要看上下文的
/// （如「己 / 已 / 巳」）一律不碰——不知道该改哪个，就别改。
const TYPO_TABLE: &[(&str, &str)] = &[
    ("己經", "已經"),
    ("以經", "已經"),
    ("因該", "應該"),
    ("既然己", "既然已"),
    ("按排", "安排"),
    ("另人", "令人"),
];

fn fix_common_typos(s: &str) -> String {
    let mut out = s.to_string();
    for (wrong, right) in TYPO_TABLE {
        if out.contains(wrong) {
            out = out.replace(wrong, right);
        }
    }
    out
}

// ----------------------------------------------------------------------
// 二、块级收尾
// ----------------------------------------------------------------------

/// 段间空白收尾：连续空行压成一个、全文去首尾空行、末尾补一个换行。
fn tidy_blocks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            // 段与段之间恒一个空行：core 的断章器把空行当段落边界。
            out.push_str("\n\n");
        }
        out.push_str(line);
    }
    if out.is_empty() {
        return String::new();
    }
    out.push('\n');
    out
}

/// 汉字（含扩展区）判定。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF     // 扩展 A
        | 0x4E00..=0x9FFF   // 基本区
        | 0xF900..=0xFAFF   // 兼容汉字
        | 0x20000..=0x2FA1F // 扩展 B~F
    )
}
