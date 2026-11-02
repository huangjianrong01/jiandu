//! **外部格式残留清洗**（v1.9.26）。
//!
//! [`crate::cleaner`] 管的是「文本卫生」——零宽字符、简繁混排、注疏不闭合，
//! 那些是**写稿人自己的疏忽**。本模块管的是另一类脏：**原格式的脚手架**
//! 没被剥干净，混进了正文流。它们的共同点是「字一个都没错，但根本不是书」。
//!
//! 实测从真实电子书里抠出来的样子（每一条都撞过）：
//!
//! ```text
//!   汉字间空格   `道 可 道 ， 非 常 道 。`      ← PDF / OCR / 扒站的头号产物
//!   页码行       `- 12 -` `· 13 ·` `第 14 页` `Page 15 of 300`
//!   HTML 实体    `&nbsp;&ldquo;玄之又玄&rdquo;`  ← 而且 `;` 会被全角化成 `；`
//!   点线目录     `第一章　導論………………1`
//!   Markdown     `**此句為全書綱領**` `> 案：此句疑衍。`
//!   上标注号     `天下皆知美之為美[1]`
//!   破折号       `絕學無憂――此之謂也。`         ← U+2015 ×2，看着对、量宽不对
//! ```
//!
//! # 三条设计铁律
//!
//! 1. **宁漏不误**。每一个判据都往「只有这一种解释」上收：页码行必须整行
//!    剥完只剩数字；点线目录必须「有 ≥3 个点线**且**行尾是数字」；汉字间
//!    空格必须两侧都是汉字或中文标点。判不准的一律不做（见 [`should`] 的
//!    说明）。
//! 2. **零开销早退**。全部返回 `Option<String>`：`None` 表示「这一条根本没
//!    命中」，调用方连一次 clone 都不用付。古籍正文里 `&`、`*`、`` ` ``、
//!    `#` 出现的概率极低，所以绝大多数函数在真实书稿上是**一次 `memchr`
//!    都不到**就返回。
//! 3. **单遍扫描**。[`sweep_lines`] 把四条行级规则（页码 / 目录 / Markdown /
//!    上标）收进**同一次** `split('\n')`，不是四遍。十万字稿子是毫秒级。
//!
//! # 为什么有些「明显的脏」反而故意不做
//!
//! * **相邻重复行**：分不清「OCR 双栏各扫一遍」与「重章叠句」（《诗经》
//!   通篇都是刻意重复）。删对了没人夸，删错了是把书改残——不做。
//! * **`①` `②` 圈号**：方书里「①人参 ②白朮」是**内容**不是标号，剥掉就
//!   丢了分条语义。笺读的批注圈号是渲染层画出来的，跟文本里的 `①` 无关。
//! * **重复页眉**：要靠全书频次判断，小样本（单章 txt）上必然误杀。

/// 汉字（含扩展 A/B、兼容区）判据。
///
/// 扩展 B 必须留：生僻字（`𠀀` 类）在古籍里是常态，判成非汉字会让
/// 汉字间空格被保留下来。
fn is_han(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF       // 扩展 A
        | 0x4E00..=0x9FFF     // 基本区
        | 0xF900..=0xFAFF     // 兼容汉字
        | 0x20000..=0x2FA1F   // 扩展 B–F + 兼容补充
    )
}

/// 中文标点（全角 + 竖排专用形 + 兼容形）。
///
/// 汉字间空格的判据要把标点也算作「中文侧」：`道可道 ， 非常道` 里空格
/// 紧邻的是标点不是汉字，只认汉字会漏掉一半。
fn is_cn_punct(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F       // 　、。〃〈〉《》「」『』【】〔〕〖〗…
        | 0xFE10..=0xFE1F     // 竖排专用形 ︐︑︒︓︔︕︖︗︘︙
        | 0xFE30..=0xFE4F     // 兼容形 ︰︱︲︳︴︵︶︷︸︹︺︻︼︽︾︿﹀﹁﹂﹃﹄
        | 0xFF01..=0xFF0F     // ！＂＃＄％＆＇（）＊＋，－．／
        | 0xFF1A..=0xFF20     // ：；＜＝＞？＠
        | 0xFF3B..=0xFF40     // ［＼］＾＿｀
        | 0xFF5B..=0xFF65     // ｛｜｝～｡｢｣､･
    ) || matches!(c, '—' | '–' | '…' | '‥' | '·' | '・' | '－')
}

/// 「中文侧」：汉字或中文标点。空格两侧都满足才删。
fn cn_side(c: char) -> bool {
    is_han(c) || is_cn_punct(c)
}

// ---------------------------------------------------------------------------
// ① 换页符 → 段落边界
// ---------------------------------------------------------------------------

/// **`\x0C`（换页符）变成换行，不是删掉**。
///
/// 这是 PDF / DOCX 抽取最常见的一处**静默粘段**：`strip_invisible` 原来把
/// `\u{000C}` 直接从流里滤掉，于是「名可名，非常名。」与下一页的「無名天地
/// 之始。」被拼成一句——字没丢，句子错了，而且错得看不出来。
///
/// 只换 `\x0C`：`\x0B`（纵向制表）在古籍稿里几乎不出现，出现也多是人手
/// 误触，删掉比断段安全。
pub fn formfeed_to_break(s: &str) -> Option<String> {
    if !s.contains('\u{000C}') {
        return None;
    }
    Some(s.replace('\u{000C}', "\n"))
}

// ---------------------------------------------------------------------------
// ② HTML 实体解码
// ---------------------------------------------------------------------------

/// **HTML 标签残留剥离**（网页复制 / 扒站 EPUB 的尸体）。
///
/// 与 [`crate::dclean::strip_html_residue`] 的分工：那是给 JSON 内容字段
/// 用的（单段、脏得有限）；这一层面对的是整本书的正文流，必须更狠——
///
///   * `<script>…</script>` / `<style>…</style>` **连同内容整段删**。只剥
///     标签会把 `p{font-family:宋体;line-height:1.8}` 留成正文第一段，
///     这是扒站稿最常见的开场白。
///   * `<!-- 注释 -->` 整段删。
///   * 块级标签（`div` `p` `br` `li` `tr` `h1`…）换成换行，保住段落边界；
///     行内标签（`span` `font` `b` `i` `a` `sup`…）直接抹掉。
///   * `<` 后面**必须跟字母或 `/`** 才算标签。`a < b`、`3<5` 一律原样留——
///     古籍正文里 `<` 极少，但注文里的算式不该被吃掉。
///
/// # 为什么不用正则
///
/// 用户的原话是「强制正则过滤」，但这里刻意**不用**：HTML 标签的形态
/// （引号里的 `>`、嵌套注释、大小写混排）足以让简单正则出现病态回溯，
/// 一本几十万字的书卡在一次 `replace_all` 上是不可接受的。逐个字符走
/// 状态机是**严格 O(n)**、只看一遍，且不需要给 core 引入 regex 依赖
/// （core 的纯净守卫会拦）。效果与正则等价，代价是零。
pub fn strip_tags(s: &str) -> Option<String> {
    if !s.contains('<') {
        return None;
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    let mut hit = false;
    while i < b.len() {
        if b[i] != b'<' {
            // 整段拷贝到下一个 `<`（比逐字节 push 快一个量级）。
            let next = s[i..].find('<').map_or(b.len(), |d| i + d);
            out.push_str(&s[i..next]);
            i = next;
            continue;
        }
        // `<!-- … -->`
        if s[i..].starts_with("<!--") {
            match s[i + 4..].find("-->") {
                Some(d) => {
                    i = i + 4 + d + 3;
                    hit = true;
                    continue;
                }
                None => {
                    out.push_str(&s[i..]);
                    break;
                }
            }
        }
        // 解析标签名：`<name` / `</name`，名字只收 ASCII 字母数字。
        let mut j = i + 1;
        let closing = j < b.len() && b[j] == b'/';
        if closing {
            j += 1;
        }
        let name_start = j;
        while j < b.len() && (b[j].is_ascii_alphanumeric()) {
            j += 1;
        }
        let name = &s[name_start..j];
        if name.is_empty() {
            // `<` 后面不是标签（`a < b`、`3<5`）：原样留。
            out.push('<');
            i += 1;
            continue;
        }
        let Some(end) = s[j..].find('>') else {
            // 半截标签（文件被截断）：余下原样保留，绝不吞字。
            out.push_str(&s[i..]);
            break;
        };
        let after = j + end + 1;
        let lower = name.to_ascii_lowercase();
        // script / style：连内容一起删。
        if !closing && (lower == "script" || lower == "style") {
            let close = format!("</{lower}");
            match find_ci(&s[after..], &close) {
                Some(d) => {
                    let tail = after + d;
                    i = s[tail..].find('>').map_or(b.len(), |e| tail + e + 1);
                }
                None => i = b.len(), // 没闭合就删到文末（这一整块都是脚本）
            }
            hit = true;
            continue;
        }
        if BLOCK_TAGS.contains(&lower.as_str()) {
            out.push('\n');
        }
        // 行内标签：什么都不留（文字本身在标签外面）。
        i = after;
        hit = true;
    }
    if hit {
        Some(out)
    } else {
        None
    }
}

/// ASCII 大小写不敏感地找 `needle`（只用于 `</script` / `</style`）。
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// 换成换行的块级标签（段落边界靠这些保住）。
const BLOCK_TAGS: &[&str] = &[
    "p", "div", "br", "li", "tr", "td", "th", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote",
    "section", "article", "ul", "ol", "table", "hr", "pre", "figure", "figcaption", "dd", "dt",
    "dl", "body", "html", "head", "title", "nav", "header", "footer", "aside", "main",
];

/// 具名实体表。**只收真的会出现在中文电子书里的**——Unicode 有 2000+ 个
/// 具名实体，全表搬进来是给编译器和运行时白交税，剩下的交给 `&#N;` 数字
/// 形式兜底。
const NAMED: &[(&str, &str)] = &[
    // 空白
    ("nbsp", " "),
    ("ensp", " "),
    ("emsp", "\u{3000}"),
    ("thinsp", ""),
    ("zwnj", ""),
    ("zwj", ""),
    // 转义
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    // 中西引号与破折号（扒站的 EPUB / HTML 里到处都是）
    ("ldquo", "“"),
    ("rdquo", "”"),
    ("lsquo", "‘"),
    ("rsquo", "’"),
    ("laquo", "«"),
    ("raquo", "»"),
    ("hellip", "…"),
    ("mdash", "—"),
    ("ndash", "–"),
    ("middot", "·"),
    ("bull", "·"),
    ("prime", "′"),
    ("Prime", "″"),
    // 符号
    ("times", "×"),
    ("divide", "÷"),
    ("deg", "°"),
    ("plusmn", "±"),
    ("sect", "§"),
    ("para", "¶"),
    ("copy", "©"),
    ("reg", "®"),
    ("trade", "™"),
    ("sup2", "²"),
    ("sup3", "³"),
    ("frac12", "½"),
    ("larr", "←"),
    ("rarr", "→"),
    ("harr", "↔"),
];

/// 一个实体名字 → 替换文本（`None` = 不认识，原样保留 `&`）。
fn entity_of(name: &str) -> Option<String> {
    if name.is_empty() || name.len() > 8 {
        return None;
    }
    // 数字形式：`#12288` / `#x3000` / `#X3000`
    if let Some(num) = name.strip_prefix('#') {
        let cp = if let Some(hex) = num.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            num.parse::<u32>().ok()?
        };
        // 拒绝代理区与 0：`&#55296;` 是坏 HTML 的常见产物，没法变成 char。
        if cp == 0 || (0xD800..=0xDFFF).contains(&cp) {
            return None;
        }
        return char::from_u32(cp).map(|c| c.to_string());
    }
    NAMED
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| (*v).to_string())
}

/// **HTML 实体解码**。必须在半角标点全角化**之前**跑。
///
/// 顺序错了会出人命：`&ldquo;` 尾部的 `;` 被 [`crate::cleaner`] 的全角化
/// 判成中文标点 → 变成 `&ldquo；`，从此再也解不出来，读者看到的是一串
/// `&ldquo；玄之又玄&rdquo；`。
///
/// 兼容已经全角化的残留：分隔符 `;` 与 `；` 都认（老版本洗坏的稿子还能救）。
pub fn decode_entities(s: &str) -> Option<String> {
    if !s.contains('&') {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut hit = false;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        // 实体名最长 8 字符（`#x10FFFF` 也就 8），超出就不是实体。
        let term = after
            .char_indices()
            .take(10)
            .find(|(_, c)| *c == ';' || *c == '；');
        if let Some((e, sep)) = term {
            if let Some(rep) = entity_of(&after[..e]) {
                out.push_str(&rep);
                // 分隔符本身（无论是半角还是被洗坏的全角）一并吃掉。
                // ⚠️ 必须用 `len_utf8()` 步进：全角 `；` 是 3 字节，按 1 字节
                // 跳会切在字符中间，`not a char boundary` 当场 panic。
                rest = &after[e + sep.len_utf8()..];
                hit = true;
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    if hit {
        Some(out)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// ③ 汉字之间的伪空格
// ---------------------------------------------------------------------------

/// **删掉汉字 / 中文标点之间的空格**。
///
/// 中文排版不用空格分词。这句话在竖排里尤其硬：**任何**一个多余空格都会
/// 在网格里顶出一个空字格，整列往下错位——这正是 PDF / OCR 抽取稿在笺读
/// 里「看起来字距忽大忽小」的根因。
///
/// 判据要「两侧都算中文」才删，于是这些**安全保留**：
///
/// ```text
///   共 300 页        数字侧不是中文 → 留（后面由页码行规则整行删）
///   Page 15 of 300   拉丁 → 留（同上）
///   　　道可道        行首缩进左邻为空 → 留（这是段首记号，唯一该留的空格）
///   《 道德經 》      两侧是书名号（中文标点）→ 删
/// ```
///
/// 连续多个空格整段丢或整段留，不拆开——半个缩进比没有缩进更难看。
pub fn despace_cjk(s: &str) -> Option<String> {
    if !s.contains(' ') && !s.contains('\u{3000}') {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    // 待定空格串：遇上非空格再决定去留（要看右邻是谁）。
    let mut gap = String::new();
    let mut hit = false;
    for c in s.chars() {
        if c == ' ' || c == '\u{3000}' {
            gap.push(c);
            continue;
        }
        if !gap.is_empty() {
            let left = out.chars().next_back();
            if left.is_some_and(cn_side) && cn_side(c) {
                hit = true; // 中文夹空格：整段丢弃
            } else {
                out.push_str(&gap);
            }
            gap.clear();
        }
        out.push(c);
    }
    // 文末空格（`tidy_whitespace` 本该剪掉，这里只兜底不判）。
    out.push_str(&gap);
    if hit {
        Some(out)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// ④ 行级残渣：页码 / 点线目录 / Markdown / 上标
// ---------------------------------------------------------------------------

/// 页码行常见的包装符（`- 12 -`、`· 13 ·`、`(14)`、`[15]`、`| 16 |`…）。
fn is_page_wrap(c: char) -> bool {
    matches!(
        c,
        '-' | '–' | '—' | '―' | '·' | '•' | '*' | '|' | '=' | '~'
            | '[' | ']' | '(' | ')' | '（' | '）' | '【' | '】' | '「' | '」'
            | ' ' | '\u{3000}'
    )
}

/// **整行是不是一个页码 / 页眉页脚**。
///
/// 判据严到近乎保守：把包装符全剥掉之后，剩下的必须**只能是**
///
/// * 1–4 位数字（`12`、`１３`）——阿拉伯数字在古籍正文里不会独立成行；
/// * `第 N 页 / 第 N 頁`；
/// * `N / M`、`N-M`、`N—M`（页码范围）；
/// * `Page N`、`Page N of M`、`p. N`、`p N`；
/// * `共 N 页 / 全 N 页 / 總 N 頁`。
///
/// 任何带正文汉字的行（`咸亨元年`、`卷十三`、`第一`）都不在其列——
/// 「汉字数字孤行」这条**故意不做**，交给 core 的 IR 层
/// [`crate::cleaner::is_lone_number_para`]，那里有整卷上下文可参考，
/// 在清洗层单看一行删「十三」会误伤诗行。
fn is_page_line(line: &str) -> bool {
    let t = line.trim();
    // 页码行不可能长；长行直接放过（省一次两遍扫描）。
    if t.is_empty() || t.chars().count() > 24 {
        return false;
    }
    // 必须含数字，否则连候选都不是。
    if !t.chars().any(|c| c.is_ascii_digit() || ('０'..='９').contains(&c)) {
        return false;
    }
    let core: String = t.chars().filter(|c| !is_page_wrap(*c)).collect();
    if core.is_empty() {
        return false;
    }
    let digits: String = core
        .chars()
        .map(|c| {
            if ('０'..='９').contains(&c) {
                char::from_u32(c as u32 - 0xFF10 + '0' as u32).unwrap_or(c)
            } else {
                c
            }
        })
        .collect();
    let d = digits.as_str();

    // ① 纯数字
    if d.len() <= 4 && d.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // ② 第 N 页 / 第 N 頁
    if let Some(rest) = d.strip_prefix('第') {
        if let Some(n) = rest.strip_suffix(['页', '頁']) {
            return !n.is_empty() && n.len() <= 4 && n.chars().all(|c| c.is_ascii_digit());
        }
        // ②b 共 N 页（剥掉「共/全/總」后再看一次）
        return false;
    }
    for pre in ["共", "全", "總", "总"] {
        if let Some(rest) = d.strip_prefix(pre) {
            if let Some(n) = rest.strip_suffix(['页', '頁']) {
                return !n.is_empty() && n.len() <= 4 && n.chars().all(|c| c.is_ascii_digit());
            }
        }
    }
    // ③ N / M、N-M、N—M
    for sep in ['/', '-', '—', '–'] {
        if let Some((a, b)) = d.split_once(sep) {
            if !a.is_empty()
                && !b.is_empty()
                && a.len() <= 4
                && b.len() <= 4
                && a.chars().all(|c| c.is_ascii_digit())
                && b.chars().all(|c| c.is_ascii_digit())
            {
                return true;
            }
        }
    }
    // ④ Page N / Page N of M / p. N / p N
    let low = d.to_ascii_lowercase();
    if let Some(rest) = low.strip_prefix("page") {
        let rest = rest.trim();
        if let Some((a, _)) = rest.split_once("of") {
            return a.trim().len() <= 4 && a.trim().chars().all(|c| c.is_ascii_digit());
        }
        return rest.len() <= 4 && !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
    }
    if let Some(rest) = low.strip_prefix("p.") {
        let rest = rest.trim();
        return rest.len() <= 4 && !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
    }
    // `p 12`：只在原文里真有空格分隔时才认（`p12` 可能是正文页码引用）。
    if low.len() <= 5 && low.starts_with('p') {
        let rest = &low[1..];
        if rest.len() <= 4 && !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return t.chars().nth(1).is_some_and(|c| c == '.' || c.is_whitespace());
        }
    }
    false
}

/// 点线字符（目录行里连接题名与页码的那一串）。
fn is_leader(c: char) -> bool {
    matches!(c, '…' | '‥' | '·' | '・' | '.' | '．' | '‧' | '—' | '–' | '_')
}

/// **点线目录行**：`第一章　導論………………1`。
///
/// 判据只有一条组合，但很硬：**≥3 个连续点线字符** 且 **行尾是 1–4 位数字**。
/// 两个条件缺一不可——
///
/// * 只认点线不认行尾数字 → 正文里的 `……所由來久矣` 会被误删；
/// * 只认行尾数字不认点线 → `乾隆十五年` 这类正文会被误删。
///
/// 所以「靠对齐空格排的目录」（`第一章　　導論　　1`）**不删**：它跟正文
/// 里「某某　　某某」的对仗句长得一模一样，机器没有可靠判据。
fn is_toc_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 60 {
        return false;
    }
    // 行尾必须是数字（允许一层包裹：`1`、`（1）`、` 1 `）。
    let tail: String = t
        .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '）' | ')' | '】' | '」'))
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || ('０'..='９').contains(c))
        .collect();
    if tail.is_empty() || tail.chars().count() > 4 {
        return false;
    }
    // 点线：≥3 个连续。
    let mut run = 0usize;
    let mut best = 0usize;
    for c in t.chars() {
        if is_leader(c) {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best >= 3
}

/// **上标注号** `[1]` `【12】` → 剥掉。
///
/// 只认「方括号里纯数字」，因为这是唯一无歧义的一种：`[注]` / `[/注]`
/// 是夹注壳（绝不能碰），`[高亮]` 是批注壳，`[案：…]` 是内容。
///
/// 圆括号 `(1)` **不做**：古籍里「（1）」更可能是原文自带的层次序号，
/// 剥掉就把分条结构吃掉了。
pub fn strip_sup_marks(s: &str) -> Option<String> {
    if !s.contains('[') && !s.contains('【') {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut hit = false;
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        let close = match c {
            '[' => ']',
            '【' => '】',
            _ => {
                out.push(c);
                i += 1;
                continue;
            }
        };
        // 向后找闭括号，最多 4 个字符（数字 1–3 位 + 闭括号）。
        let mut j = i + 1;
        let mut digits = 0usize;
        while j < cs.len() && j - i <= 4 && cs[j] != close {
            if cs[j].is_ascii_digit() || ('０'..='９').contains(&cs[j]) {
                digits += 1;
            } else {
                break;
            }
            j += 1;
        }
        if digits > 0 && j < cs.len() && cs[j] == close {
            hit = true;
            i = j + 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    if hit {
        Some(out)
    } else {
        None
    }
}

/// Markdown 行首标记 → 剥掉，只留文字。
///
/// `## 標題` → `標題`；`> 案：` → `案：`；`- 其一` → `其一`。
/// 只剥**一个层级**（重复调用会剥第二层，但一层已经够——稿子不会嵌套五层）。
fn md_strip_head(t: &str) -> Option<String> {
    let body = t.trim_start();
    if body.is_empty() {
        return None;
    }
    // `#{1,6}` + 空格
    if body.starts_with('#') {
        let n = body.chars().take_while(|c| *c == '#').count();
        if n <= 6 {
            let rest = &body[n..];
            if rest.is_empty() {
                return Some(String::new());
            }
            if rest.starts_with(' ') || rest.starts_with('\u{3000}') {
                return Some(rest.trim_start().to_string());
            }
        }
    }
    // `>` 引用
    if let Some(rest) = body.strip_prefix('>') {
        return Some(rest.trim_start().to_string());
    }
    // `- ` / `* ` / `+ ` 列表项
    for m in ['-', '*', '+'] {
        if let Some(rest) = body.strip_prefix(m) {
            if rest.starts_with(' ') || rest.starts_with('\u{3000}') {
                let rest = rest.trim_start();
                // 纯 `- ---` 分隔线不在这里处理（走下面 md_strip_line）
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

/// 独立分隔线：`---`、`***`、`___`、`===`（≥3 个同字符）。
fn is_md_rule(t: &str) -> bool {
    let t = t.trim();
    let n = t.chars().count();
    if n < 3 {
        return false;
    }
    let first = t.chars().next().unwrap_or(' ');
    matches!(first, '-' | '*' | '_' | '=')
        && t.chars().all(|c| c == first || c == ' ')
        && t.chars().filter(|c| *c == first).count() >= 3
}

/// Markdown 行内标记 → 剥掉。
///
/// 只处理**成对**的、且两侧都有内容的：孤零零一个 `*` 原样留着（可能是
/// 缺字记号）。
fn md_strip_inline(t: &str) -> Option<String> {
    let mut s = t.to_string();
    let mut hit = false;

    // `![alt](url)` 整枚删（图片在笺读里没有位置）。
    while let Some(a) = s.find("![") {
        let Some(b) = s[a..].find("](") else { break };
        let Some(c) = s[a + b..].find(')') else { break };
        s.replace_range(a..a + b + c + 1, "");
        hit = true;
    }
    // `[text](url)` → `text`（链接文字是正文，URL 不是）。
    while let Some(a) = s.find('[') {
        let Some(b) = s[a..].find("](") else { break };
        let Some(c) = s[a + b..].find(')') else { break };
        let text = s[a + 1..a + b].to_string();
        // `[注]` 后面不会跟 `(`，天然避开；但 `[案](…)` 这种要保内容。
        if text.contains('[') {
            break;
        }
        s.replace_range(a..a + b + c + 1, &text);
        hit = true;
    }
    // `**粗**` / `__粗__` → `粗`
    for m in ["**", "__"] {
        while let Some(a) = s.find(m) {
            let Some(rel) = s[a + m.len()..].find(m) else { break };
            let b = a + m.len();
            let e = b + rel;
            if e == b {
                // `****` 空标记：只摘一对，避免死循环。
                s.replace_range(a..b, "");
                hit = true;
                continue;
            }
            let inner = s[b..e].to_string();
            s.replace_range(a..e + m.len(), &inner);
            hit = true;
            if s == inner {
                break;
            }
        }
    }
    // `` `代码` `` → `代码`
    while let Some(a) = s.find('`') {
        let Some(rel) = s[a + 1..].find('`') else { break };
        let e = a + 1 + rel;
        let inner = s[a + 1..e].to_string();
        s.replace_range(a..=e, &inner);
        hit = true;
    }
    if hit {
        Some(s)
    } else {
        None
    }
}

/// **一条脏行 → 干净行（或整行不要）**。
///
/// 返回 `None` = 这行原样保留；`Some("")` = 整行删；`Some(s)` = 换成 `s`。
fn clean_one_line(line: &str) -> Option<String> {
    // 顺序：先判整行删（页码 / 目录 / 分隔线），再剥行首标记，最后剥行内。
    if is_page_line(line) || is_toc_line(line) || is_md_rule(line) {
        return Some(String::new());
    }
    let mut cur = line.to_string();
    let mut hit = false;
    if let Some(x) = md_strip_head(&cur) {
        cur = x;
        hit = true;
    }
    // 行首剥完可能暴露出新的行首标记（`> **案**`），再来一轮。
    if let Some(x) = md_strip_head(&cur) {
        cur = x;
        hit = true;
    }
    if let Some(x) = md_strip_inline(&cur) {
        cur = x;
        hit = true;
    }
    if let Some(x) = strip_sup_marks(&cur) {
        cur = x;
        hit = true;
    }
    if hit {
        Some(cur)
    } else {
        None
    }
}

/// **行级残渣总扫**：页码行、点线目录、Markdown 标记、上标注号。
///
/// 四件事收进**同一次** `split('\n')`。分成四个函数各扫一遍也能写，但
/// 十万字稿子就是四倍的字符串分配开销——导入要「黄油般丝滑」，这类地方
/// 一遍都不能多。
///
/// 被删掉的行会留下空行，交给下游 `tidy_blocks` 收（它本来就要压空行）。
pub fn sweep_lines(s: &str) -> Option<String> {
    // 一个候选都没有就整段跳过：这四个特征字符在古籍正文里出现率极低。
    let maybe = s.contains('&')
        || s.contains('#')
        || s.contains('>')
        || s.contains('*')
        || s.contains('`')
        || s.contains('_')
        || s.contains('[')
        || s.contains('【')
        || s.contains('…')
        || s.contains('·')
        || s.contains('.');
    // 页码是纯数字行，没上面那些字符却仍是页码，所以这里不能用 `maybe`
    // 一刀切——只在有数字时才做完整扫描。
    if !maybe && !s.chars().any(|c| c.is_ascii_digit() || ('０'..='９').contains(&c)) {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut hit = false;
    let mut first = true;
    for line in s.split('\n') {
        match clean_one_line(line) {
            Some(x) => {
                hit = true;
                if x.is_empty() {
                    continue; // 整行删
                }
                if !first {
                    out.push('\n');
                }
                out.push_str(&x);
                first = false;
            }
            None => {
                if !first {
                    out.push('\n');
                }
                out.push_str(line);
                first = false;
            }
        }
    }
    if hit {
        Some(out)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// ⑤ 破折号字形归一
// ---------------------------------------------------------------------------

/// **横线字形归一**：`―`(U+2015) / `－`(U+FF0D) → `—`(U+2014)。
///
/// 长度不变，只换字形。这三种在屏幕上肉眼几乎分不出，但**度量不一样**：
/// U+2015 在多数中文字体里是半角宽，两个拼起来比一个汉字窄，竖排的
/// `――` 就比 `——` 短一截，一列字都跟着歪。古籍稿里三种混着来是常态
/// （不同 OCR / 不同输入法各写各的）。
pub fn unify_dashes(s: &str) -> Option<String> {
    if !s.contains('―') && !s.contains('－') {
        return None;
    }
    Some(
        s.chars()
            .map(|c| match c {
                '―' | '－' => '—',
                _ => c,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formfeed_breaks_instead_of_gluing() {
        // 这是真 bug 的回归：删掉换页符会把两段粘成一句。
        assert_eq!(
            formfeed_to_break("名可名，非常名。\u{000C}無名天地之始。").as_deref(),
            Some("名可名，非常名。\n無名天地之始。")
        );
        assert_eq!(formfeed_to_break("干净的一行"), None);
    }

    #[test]
    fn tags_are_stripped_but_text_kept() {
        // 行内标签抹掉，文字留下。
        assert_eq!(
            strip_tags("道<span class=\"x\">可</span>道").as_deref(),
            Some("道可道")
        );
        // 块级标签换成换行（保住段落边界）。
        assert_eq!(
            strip_tags("<div>名可名</div><div>非常名</div>").as_deref(),
            Some("\n名可名\n\n非常名\n")
        );
        // script / style 连内容整段删——不然 CSS 会变成正文第一段。
        assert_eq!(
            strip_tags("<style>p{font-family:宋体}</style>道可道").as_deref(),
            Some("道可道")
        );
        assert_eq!(
            strip_tags("道<script>var a=1;</script>可道").as_deref(),
            Some("道可道")
        );
        // 大小写混排也要认（扒站稿常见 `<DIV>`）。
        assert_eq!(strip_tags("<DIV>甲</DIV>").as_deref(), Some("\n甲\n"));
        // 注释整段删。
        assert_eq!(strip_tags("甲<!-- 注释 -->乙").as_deref(), Some("甲乙"));
        // `<` 后不是字母就不是标签：算式必须原样留。
        assert_eq!(strip_tags("三 < 五"), None);
        assert_eq!(strip_tags("a<b"), None);
        // 半截标签（文件被截断）绝不吞字。
        assert_eq!(strip_tags("道可道<div class="), None);
    }

    #[test]
    fn entities_are_decoded_and_unknown_kept() {
        assert_eq!(
            decode_entities("&ldquo;玄之又玄&rdquo;").as_deref(),
            Some("“玄之又玄”")
        );
        assert_eq!(decode_entities("&nbsp;&#12288;眾妙").as_deref(), Some(" 　眾妙"));
        // 全角化的分隔符（老版本洗坏的）也要能救回来。
        assert_eq!(decode_entities("&ldquo；玄").as_deref(), Some("“玄"));
        // 不认识的实体原样留着，绝不吞字。
        assert_eq!(decode_entities("&unknownthing;"), None);
        assert_eq!(decode_entities("a & b"), None);
        // 代理区码位不能变成替换符。
        assert_eq!(decode_entities("&#55296;"), None);
    }

    #[test]
    fn spaces_between_han_are_dropped() {
        assert_eq!(
            despace_cjk("道 可 道 ， 非 常 道 。").as_deref(),
            Some("道可道，非常道。")
        );
        // 连续空格也整段丢。
        assert_eq!(despace_cjk("道　　可道").as_deref(), Some("道可道"));
    }

    #[test]
    fn spaces_next_to_non_han_are_kept() {
        // 数字 / 拉丁旁的空格必须留：`共 300 页` 里的空格是分隔不是脏。
        assert_eq!(despace_cjk("共 300 页"), None);
        assert_eq!(despace_cjk("Page 15 of 300"), None);
        // 行首缩进（左邻为空）保留——这是古籍稿唯一的段首记号。
        assert_eq!(despace_cjk("　　道可道"), None);
        // 纯拉丁文本一个字都不改。
        assert_eq!(despace_cjk("hello world"), None);
    }

    #[test]
    fn page_numbers_are_recognized() {
        for s in [
            "12",
            "- 12 -",
            "· 13 ·",
            "第 14 页",
            "第15頁",
            "Page 15 of 300",
            "p. 16",
            "17 / 300",
            "共 200 页",
            "１８",
            "(19)",
            "[20]",
        ] {
            assert!(is_page_line(s), "页码行没认出来：{s}");
        }
    }

    #[test]
    fn body_lines_are_not_page_numbers() {
        for s in [
            "咸亨元年",
            "卷十三",
            "第一",
            "十三年春",
            "道可道，非常道。",
            "——",          // 破折号独立行不能当页码删
            "第二章　天下皆知",
            "共 300 人",    // 量词不是「页」
            "p12",          // 无分隔的 p12 可能是正文引用
        ] {
            assert!(!is_page_line(s), "正文被当成页码了：{s}");
        }
    }

    #[test]
    fn toc_needs_both_leaders_and_tail_number() {
        assert!(is_toc_line("第一章　導論………………1"));
        assert!(is_toc_line("第二章 本論 ...... 25"));
        assert!(is_toc_line("第三章　結論·······３"));
        // 只有点线没有尾数字 → 正文里的省略号，不能删。
        assert!(!is_toc_line("……所由來久矣。"));
        // 只有尾数字没有点线 → 可能是正文（对齐空格排的目录认不出，宁可漏）。
        assert!(!is_toc_line("第一章　　導論　　1"));
    }

    #[test]
    fn sup_marks_are_stripped_but_note_shells_survive() {
        assert_eq!(
            strip_sup_marks("天下皆知美之為美[1]").as_deref(),
            Some("天下皆知美之為美")
        );
        assert_eq!(strip_sup_marks("其政悶悶【12】").as_deref(), Some("其政悶悶"));
        // 夹注壳与批注壳一个字都不能碰。
        assert_eq!(strip_sup_marks("[注]河上公曰[/注]"), None);
        assert_eq!(strip_sup_marks("[高亮]此句[/高亮]"), None);
        // 非数字方括号是内容。
        assert_eq!(strip_sup_marks("案[详见下文]"), None);
    }

    #[test]
    fn markdown_residue_is_stripped() {
        assert_eq!(md_strip_head("## 標題"), Some("標題".to_string()));
        assert_eq!(md_strip_head("> 案：此句疑衍。"), Some("案：此句疑衍。".to_string()));
        assert_eq!(md_strip_head("- 其一"), Some("其一".to_string()));
        assert_eq!(md_strip_head("正文一行"), None);
        assert_eq!(
            md_strip_inline("**此句為全書綱領**").as_deref(),
            Some("此句為全書綱領")
        );
        assert_eq!(
            md_strip_inline("見[下文](https://x.y/z)可知").as_deref(),
            Some("見下文可知")
        );
        assert_eq!(md_strip_inline("![圖](a.png)說明").as_deref(), Some("說明"));
        // 孤星号是缺字记号，不能动。
        assert_eq!(md_strip_inline("其政＊悶悶"), None);
    }

    #[test]
    fn sweep_removes_residue_lines_in_one_pass() {
        let dirty = "道可道，非常道。\n- 12 -\n第一章　導論………………1\n**案**\n名可名[1]，非常名。";
        let out = sweep_lines(dirty).expect("应当命中");
        assert_eq!(out, "道可道，非常道。\n案\n名可名，非常名。");
        // 干净文本零改动、零分配。
        assert_eq!(sweep_lines("道可道，非常道。"), None);
    }

    #[test]
    fn dashes_are_unified_in_place() {
        assert_eq!(unify_dashes("絕學無憂――此之謂也。").as_deref(), Some("絕學無憂——此之謂也。"));
        // 长度必须不变：三个还是三个。
        assert_eq!(unify_dashes("―――").as_deref(), Some("———"));
        assert_eq!(unify_dashes("本来就没有横线"), None);
    }
}
