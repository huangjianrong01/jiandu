//! 阿拉伯数字 → 汉字（导入期「源头治理」的数据清洗）。
//!
//! 竖排古籍里 `223年` 若原样入库，排版引擎会把它当成三个独立西文字符，
//! 一格一个地竖着摞成「2 / 2 / 3」，读起来断成三截。根治办法是在**导入
//! 阶段**就把它写成汉字——`二二三年`——从源头断绝被拆散的可能。
//!
//! 本模块只做纯字符串计算，不依赖 GPUI，可单测：
//!
//! * [`num_to_cn`]：数值读法——`15 → 十五`、`223 → 二百二十三`、`1980 → 一千九百八十`；
//! * [`num_to_cn_digits`]：逐字读——`223 → 二二三`（年份 / 编号用，刻本纪年就是这么念的）；
//! * [`is_year_context`]：后接「年 / 年代 / 年间 / 年度 / 年份」且不超过四位 → 纪年；
//! * [`normalize_numbers`]：扫全文的 `/[0-9]+/g`（含全角数字），按上下文择读法替换。
//!
//! **不该转的要放过**：`ISBN 978-7`、`No.15`、`#2024` 这类编号与字母粘连，
//! 转成汉字反而更难认，故相邻 ASCII 字母 / `#` 时整段跳过（[`should_skip`]）。

/// 一个数字串允许转换的最大位数：更长的（ISBN、身份证、时间戳）视为编号，不转。
///
/// 六位已足以覆盖页码、年份、数量；再长就是流水号，转成汉字反而读不断句。
pub const MAX_DIGITS: usize = 6;

/// 逐字读的数字表（零位用「〇」，刻本计数习惯）。
const DIGIT_CN: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];

/// 数字读法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumStyle {
    /// 数值读法：`15 → 十五`、`223 → 二百二十三`、`1980 → 一千九百八十`。
    Value,
    /// 逐字读：`223 → 二二三`（纪年 / 编号）。
    Digit,
}

/// 转换策略：纪年与一般数字可以各用一种读法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumPolicy {
    /// 纪年（后接「年」等）的读法。
    pub year: NumStyle,
    /// 一般数字（页码、数量、岁数……）的读法。
    pub general: NumStyle,
}

impl Default for NumPolicy {
    fn default() -> Self {
        Self {
            // 纪年逐字读：刻本里「二二三年」就是这么写的，读序与字序一致；
            // 一般数字按数值读：十五、二百二十三，才是汉语的念法。
            year: NumStyle::Digit,
            general: NumStyle::Value,
        }
    }
}

/// 全角数字 `０-９` → 半角（它们在竖排里同样会被逐字拆格）。
pub fn ascii_digit(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(c as u32 - '0' as u32),
        '０'..='９' => Some(c as u32 - '０' as u32),
        _ => None,
    }
}

/// `c` 是不是「数字」（半角 / 全角）。
pub fn is_digit(c: char) -> bool {
    ascii_digit(c).is_some()
}

/// 零宽 / 不可见字符：数字串中间夹了它们（EPUB 常见 `&#8203;`）也不能算断开。
pub fn is_transparent(c: char) -> bool {
    matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}' | '\u{2060}')
}

/// 小数点（半角 / 全角）：夹在数字中间时并入同一串。
pub fn is_decimal_point(c: char) -> bool {
    matches!(c, '.' | '．' | '·')
}

/// 千分位 / 连字符：夹在数字中间时并入同一串（`978-7`、`1,280`）。
fn is_digit_sep(c: char) -> bool {
    matches!(c, ',' | '，' | '-' | '－' | '‐' | '–' | ' ')
}

/// 后接上下文是不是「纪年」：`223年` / `1980年代` / `2024年度`。
///
/// 只看紧跟着的一两个字符，且数字不超过四位——`19980203` 这种流水号不是纪年。
pub fn is_year_context(tail: &str, digits: usize) -> bool {
    if digits == 0 || digits > 4 {
        return false;
    }
    let t: Vec<char> = tail.chars().take(3).collect();
    if t.is_empty() {
        return false;
    }
    matches!(t[0], '年' | '載' | '载')
        || (t.len() >= 2 && t[0] == '年' && matches!(t[1], '代' | '間' | '间' | '度' | '份' | '末' | '初'))
}

/// 这一段数字该不该**放过不转**（编号 / 与字母粘连 / 过长）。
///
/// `prev` / `next` 取**最近的非空白字符**（`ISBN 978` 中间隔了空格也要认出
/// 前面的字母）：
///
/// * 前或后是 ASCII 字母 → 型号 / ISBN 一类（`ISBN 978`、`A4`）；
/// * 前是 `#`、`/`、`@`、`+`、`*`、`=` → 编号 / 标记；
/// * 位数超过 [`MAX_DIGITS`] → 时间戳 / 身份证，转成汉字反而读不了。
pub fn should_skip(prev: Option<char>, next: Option<char>, digits: usize) -> bool {
    if digits == 0 || digits > MAX_DIGITS {
        return true;
    }
    let letter = |c: char| c.is_ascii_alphabetic();
    if prev.map(letter).unwrap_or(false) || next.map(letter).unwrap_or(false) {
        return true;
    }
    if let Some(p) = prev {
        if matches!(p, '#' | '/' | '\\' | '@' | '_' | '+' | '*' | '=') {
            return true;
        }
    }
    false
}

/// 四位数一节的读法（`0..=9999`），`lowest` = 是否最低节（决定 `10~19` 念「十」还是「一十」）。
fn four_digits(n: u32, lowest: bool) -> String {
    const D: [char; 10] = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    let (th, h, t, o) = (n / 1000, n % 1000 / 100, n % 100 / 10, n % 10);
    let mut s = String::new();
    let mut zero = false; // 待补的「零」
    if th > 0 {
        s.push(D[th as usize]);
        s.push('千');
    }
    if h > 0 {
        s.push(D[h as usize]);
        s.push('百');
    } else if th > 0 && (t > 0 || o > 0) {
        zero = true;
    }
    if t > 0 {
        if zero {
            s.push('零');
            zero = false;
        }
        // 最低节的 `10~19` 念「十 / 十五」，不念「一十」；高位节照念「一千一百一十」。
        if !(t == 1 && s.is_empty() && lowest) {
            s.push(D[t as usize]);
        }
        s.push('十');
    } else if (th > 0 || h > 0) && o > 0 {
        zero = true;
    }
    if o > 0 {
        if zero {
            s.push('零');
        }
        s.push(D[o as usize]);
    }
    s
}

/// 数值读法：`15 → 十五`、`223 → 二百二十三`、`1980 → 一千九百八十`、`0 → 零`。
///
/// 按「亿 / 万 / 个」三节处理，节内不足四位时补一个「零」（`100000003 → 一亿零三`）。
pub fn num_to_cn(n: u64) -> String {
    if n == 0 {
        return "零".to_string();
    }
    const YI: u64 = 100_000_000;
    const WAN: u64 = 10_000;
    let yi = n / YI;
    let rest = n % YI;
    let wan = rest / WAN;
    let low = rest % WAN;
    let mut out = String::new();
    if yi > 0 {
        out.push_str(&four_digits(yi as u32, false));
        out.push('亿');
    }
    if wan > 0 {
        if yi > 0 && wan < 1000 {
            out.push('零');
        }
        out.push_str(&four_digits(wan as u32, false));
        out.push('万');
    }
    if low > 0 {
        if (yi > 0 || wan > 0) && low < 1000 {
            out.push('零');
        }
        out.push_str(&four_digits(low as u32, true));
    }
    if out.is_empty() {
        out.push('零');
    }
    out
}

/// 逐字读：`223 → 二二三`、`1980 → 一九八〇`（零位读「〇」，合刻本计数习惯）。
pub fn num_to_cn_digits(digits: &str) -> String {
    digits
        .chars()
        .filter_map(ascii_digit)
        .map(|d| DIGIT_CN[d as usize])
        .collect()
}

/// 一个数字串的替换结果（供 [`normalize_numbers`] 与测试直接取用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumRun {
    /// 原文（含全角数字、小数点、千分位）。
    pub raw: String,
    /// 替换后的汉字。
    pub cn: String,
    /// 采用的读法。
    pub style: NumStyle,
    /// 是否判为纪年。
    pub year: bool,
}

/// 从 `chars[i]` 起取出一个完整数字串，返回 `(串长字符数, 纯数字位数)`。
///
/// 规则：`i` 处必须是数字；其后允许插入零宽字符；小数点 / 千分位 / 连字符
/// 只有在**后面还跟着数字**时才并入（`3.14` 是小数，`14.` 的句点不吞）。
pub fn scan_run(chars: &[char], i: usize) -> (usize, usize) {
    if i >= chars.len() || !is_digit(chars[i]) {
        return (0, 0);
    }
    let mut end = i;
    let mut digits = 0usize;
    while end < chars.len() {
        let c = chars[end];
        if is_digit(c) {
            digits += 1;
            end += 1;
        } else if is_transparent(c) {
            end += 1; // 零宽：直接吞掉，不算位数
        } else if (is_decimal_point(c) || is_digit_sep(c))
            && chars[end..].iter().skip(1).any(|&x| is_digit(x))
        {
            end += 1; // 小数点的「点」位数另计
        } else {
            break;
        }
    }
    // 收尾拖着的分隔符不算进串里（`223. ` 不该吞掉句点）。
    while end > i {
        let last = chars[end - 1];
        if is_digit(last) {
            break;
        }
        if is_transparent(last) {
            end -= 1;
            continue;
        }
        if is_decimal_point(last) || is_digit_sep(last) {
            end -= 1;
            continue;
        }
        break;
    }
    (end - i, digits)
}

/// 把一段数字串（原文）按上下文转成汉字。
pub fn run_to_cn(raw: &str, tail: &str, policy: &NumPolicy) -> NumRun {
    let digits: String = raw
        .chars()
        .filter_map(ascii_digit)
        .map(|d| (b'0' + d as u8) as char)
        .collect();
    let n_digits = digits.chars().count();
    let year = is_year_context(tail, n_digits);
    // 含小数点 → 一律逐字读（三点一四），数值读法说不清。
    let has_point = raw.chars().any(is_decimal_point);
    // 超过四位的长串（页码 27637）也逐字读：写成「二万七千六百三十七」会把
    // 版心撑开一大截，逐字则与刻本叶码一致。
    let long = n_digits > 4;
    let style = if year || has_point || long {
        NumStyle::Digit
    } else {
        policy.general
    };
    let mut cn = match style {
        NumStyle::Digit => {
            // 小数：整数部分逐字，「点」接小数部分。
            if has_point {
                let mut out = String::new();
                let mut dot_done = false;
                for c in raw.chars() {
                    if let Some(d) = ascii_digit(c) {
                        out.push(DIGIT_CN[d as usize]);
                    } else if is_decimal_point(c) && !out.is_empty() && !dot_done {
                        out.push('点');
                        dot_done = true;
                    }
                }
                out
            } else {
                num_to_cn_digits(&digits)
            }
        }
        NumStyle::Value => {
            match digits.parse::<u64>() {
                Ok(v) => num_to_cn(v),
                Err(_) => num_to_cn_digits(&digits), // 溢出：退回逐字读
            }
        }
    };
    // 纪年但策略要求数值读法时，`223年` 仍念「二百二十三年」。
    if year && policy.year == NumStyle::Value && !has_point {
        if let Ok(v) = digits.parse::<u64>() {
            cn = num_to_cn(v);
        }
    }
    NumRun {
        raw: raw.to_string(),
        cn,
        style,
        year,
    }
}

/// 全文清洗：把所有数字串换成汉字（默认策略见 [`NumPolicy::default`]）。
pub fn normalize_numbers(text: &str) -> String {
    normalize_numbers_with(text, &NumPolicy::default())
}

/// 全文清洗（指定策略）。
///
/// 逐字扫描，遇到数字就用 [`scan_run`] 取出整串，按 [`should_skip`] 决定
/// 放过还是替换；其余字符原样输出，段落与标点一律不动。
pub fn normalize_numbers_with(text: &str, policy: &NumPolicy) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        if !is_digit(chars[i]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let (len, _digits) = scan_run(&chars, i);
        if len == 0 {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let raw: String = chars[i..i + len].iter().collect();
        let tail: String = chars[i + len..].iter().take(3).collect();
        // 最近的非空白字符：`ISBN 978` 隔着空格也要认出前面的字母。
        let prev = chars[..i]
            .iter()
            .rev()
            .find(|c| !c.is_whitespace() && !is_transparent(**c))
            .copied();
        let next = chars[i + len..]
            .iter()
            .find(|c| !c.is_whitespace() && !is_transparent(**c))
            .copied();
        let digit_count = raw.chars().filter_map(ascii_digit).count();
        if should_skip(prev, next, digit_count) {
            out.push_str(&raw);
        } else {
            out.push_str(&run_to_cn(&raw, &tail, policy).cn);
        }
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_reading() {
        assert_eq!(num_to_cn(0), "零");
        assert_eq!(num_to_cn(1), "一");
        assert_eq!(num_to_cn(10), "十");
        assert_eq!(num_to_cn(11), "十一");
        assert_eq!(num_to_cn(15), "十五");
        assert_eq!(num_to_cn(20), "二十");
        assert_eq!(num_to_cn(100), "一百");
        assert_eq!(num_to_cn(105), "一百零五");
        assert_eq!(num_to_cn(110), "一百一十");
        assert_eq!(num_to_cn(223), "二百二十三");
        assert_eq!(num_to_cn(1980), "一千九百八十");
        assert_eq!(num_to_cn(10_000), "一万");
        assert_eq!(num_to_cn(27_637), "二万七千六百三十七");
        assert_eq!(num_to_cn(100_000_003), "一亿零三");
    }

    #[test]
    fn digit_reading() {
        assert_eq!(num_to_cn_digits("223"), "二二三");
        assert_eq!(num_to_cn_digits("1980"), "一九八〇");
        assert_eq!(num_to_cn_digits("0"), "〇");
    }

    // ── 用户给的两个用例 ────────────────────────────────────────────
    #[test]
    fn case_year_223() {
        // 纪年逐字读：二二三年。
        assert_eq!(normalize_numbers("建安二十二年，公元223年，岁在癸卯。"), "建安二十二年，公元二二三年，岁在癸卯。");
        // 策略换成数值读法则是「二百二十三年」。
        let p = NumPolicy { year: NumStyle::Value, general: NumStyle::Value };
        assert_eq!(
            normalize_numbers_with("公元223年", &p),
            "公元二百二十三年"
        );
    }

    #[test]
    fn case_1980() {
        // 一般数字按数值读法：一千九百八十。
        assert_eq!(normalize_numbers("全书1980页"), "全书一千九百八十页");
        assert_eq!(normalize_numbers("共15卷"), "共十五卷");
        // 后接「年代」也算纪年 → 逐字读。
        assert_eq!(normalize_numbers("1980年代的排印本"), "一九八〇年代的排印本");
    }

    #[test]
    fn decimals_and_fullwidth() {
        assert_eq!(normalize_numbers("圆周率3.14"), "圆周率三点一四");
        assert_eq!(normalize_numbers("第２７６３７葉"), "第二七六三七葉");
    }

    #[test]
    fn ids_are_left_alone() {
        assert_eq!(normalize_numbers("ISBN 978-7-02"), "ISBN 978-7-02");
        assert_eq!(normalize_numbers("型号A4纸"), "型号A4纸");
        assert_eq!(normalize_numbers("编号#20240101"), "编号#20240101");
        // 超长流水号不转。
        assert_eq!(normalize_numbers("19980203"), "19980203");
    }

    #[test]
    fn zero_width_does_not_split_run() {
        // EPUB 常见的零宽空格：串仍应整体识别。
        let s = "公元2\u{200B}2\u{200B}3年";
        assert_eq!(normalize_numbers(s), "公元二二三年");
        let (len, digits) = scan_run(&s.chars().collect::<Vec<_>>(), 2);
        assert_eq!(digits, 3);
        assert!(len >= 5);
    }

    #[test]
    fn scan_run_edges() {
        let c: Vec<char> = "12.34页".chars().collect();
        let (len, digits) = scan_run(&c, 0);
        assert_eq!(digits, 4);
        assert_eq!(len, 5); // "12.34"
        let c2: Vec<char> = "14。".chars().collect();
        let (len2, digits2) = scan_run(&c2, 0);
        assert_eq!((len2, digits2), (2, 2)); // 句号不吞
    }
}
