//! HTML / XHTML：抽正文（EPUB 的每一节也走这里）。
//!
//! 只做一件事：**把标签剥掉、把块级边界还原成段落**。
//! * `script` / `style` / `nav` / `header` / `footer` / `aside` / `head` 整块丢弃
//!   （脚本与导航进正文是电子书里最刺眼的一类脏）；
//! * `<p>` `<div>` `<h1..h6>` `<li>` `<blockquote>` `<tr>` 视为块，块与块之间断行；
//! * `<br>` → 换行；`&nbsp;` → 空格；其余实体按表还原（数字实体直接认码位）。
//!
//! # 为什么不用 scraper
//!
//! 本机 `crates.io` 不通（403），`scraper` / `html5ever` 拉不下来；而 `quick-xml`
//! 本来就在依赖里（EPUB 的 OPF 要用）。HTML 的容错性确实不如 HTML5 解析器，
//! 但电子书里的 XHTML 基本是良构的；真遇到野 HTML，`quick-xml` 会报错，本模块
//! **保留已读到的部分**而不是整本导入失败——丢几个畸形段好过整本书打不开。

use std::io::BufRead;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::traits::FormatParser;
use super::txt_parser::decode_bytes;
use super::{read_streamed, ImportError};

/// HTML 解析器（`.html` / `.htm` / `.xhtml` / `.xml`）。
pub struct HtmlParser;

impl FormatParser for HtmlParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let bytes = read_streamed(path)?;
        let (src, _enc) = decode_bytes(&bytes);
        let (text, err) = extract_html_str(&src);
        if text.trim().is_empty() {
            if let Some(e) = err {
                return Err(ImportError::Xml(e));
            }
        }
        Ok(text)
    }

    fn extensions() -> &'static [&'static str] {
        &["html", "htm", "xhtml", "xml"]
    }
}

/// 从字符串抽正文（EPUB 的每一节都用它）。
pub fn extract_html_str(src: &str) -> (String, Option<String>) {
    extract_html_reader(std::io::Cursor::new(src.as_bytes()))
}

/// 流式抽正文：`BufRead` 进、纯文本出。**默认整块丢弃 `<table>`**（见
/// [`extract_html_reader_keep_tables`] 取回表格内容的退路）。
///
/// 返回（正文, 解析错误）。**即使出错也返回已读到的部分**：一本 300 节的 EPUB
/// 里有 1 节畸形，不该让整本书打不开。
pub fn extract_html_reader<R: BufRead>(reader: R) -> (String, Option<String>) {
    extract_html_with(reader, true)
}

/// 同上，但**保留表格文字**（v1.69.0 的退路）。
///
/// 少数电子书整本都用 `<table>` 排版（多见于劣质转换件）。全本弃表后会
/// 一个字都不剩，那时再跑一遍本函数把表格文字当正文救回来——**有字胜过
/// 无字**，表格排版再丑也比空书强。
pub fn extract_html_reader_keep_tables<R: BufRead>(reader: R) -> (String, Option<String>) {
    extract_html_with(reader, false)
}

/// 单次导入里允许读的**最大 XML 事件数**（v1.69.0）。
///
/// 防盗版脏文件把解析器拖进死循环：非标准 XHTML（`<<<<<`、未闭合的
/// 实体、几 MB 的单行注释）在某些 `quick-xml` 版本上会反复产出事件而不
/// 推进游标。设了这道闸，最坏情况是**读到上限就停、返回已读到的部分**
/// 并给出报错——绝不会「进度条卡死在 0%」。
const MAX_EVENTS: usize = 4_000_000;

/// 专名号 / 书名号的**最大跨度**（字）：超过就当它是排版用的强调，不标。
///
/// 古籍里一个专名至多四五个字，书名长些也就十字上下；一段几十字被 `<u>`
/// 包住，多半是「整句下划线」这种现代排版习惯，标成专名线反而是错的。
const MARK_MAX_CHARS: usize = 12;

/// 标签 + `class` → 该画哪种线：`"书"` 波浪线，`"人"` 实线；`None` 不标。
///
/// 判据顺序：**先看书名类**（`class` 里带 book / title 的一律是书名），
/// 再认 `<u>`：中文 EPUB 里 `<u>` 几乎就是**专名号**（竖排的专名线在
/// HTML 里只能靠下划线表达），故直接归「人」。
fn mark_kind_for(name: &str, class: &str) -> Option<&'static str> {
    let c = class.to_ascii_lowercase();
    if ["book", "title", "shuming", "shu-ming"].iter().any(|k| c.contains(k)) {
        return Some("书");
    }
    if name == "u" {
        return Some("人");
    }
    if ["proper", "person", "people", "zhuanming", "zhuan-ming"]
        .iter()
        .any(|k| c.contains(k))
    {
        return Some("人");
    }
    None
}

/// 取 `class` 属性的值（没有就是空串）。
fn class_of(e: &quick_xml::events::BytesStart) -> String {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == b"class" {
            return String::from_utf8_lossy(a.value.as_ref()).to_string();
        }
    }
    String::new()
}

fn extract_html_with<R: BufRead>(reader: R, drop_tables: bool) -> (String, Option<String>) {
    let mut xml = Reader::from_reader(reader);
    // 不 trim：行内空格是词边界（`<b>道</b> 可道` 里的空格不能丢）。
    xml.config_mut().trim_text(false);

    let mut buf: Vec<u8> = Vec::new();
    let mut out = String::new();
    let mut para = String::new();
    let mut skip = 0usize; // >0 表示正处在丢弃块里（可嵌套）
    let mut err: Option<String> = None;
    let mut events = 0usize;
    // 正在采集的**划线标记**：`(标签名, 线种, 已收文本)`。
    //
    // 三层漏斗的第一层就在这里：`<u>孔子</u>` 不剥成一串没头没尾的字，
    // 而是转成 core 认识的 `〔人:孔子〕孔子`——那条注会变成字侧的
    // **专名线**，而不是正文里多出来的几个字。只采一层（嵌套不叠），
    // 因为古籍里的专名不会套着书名再套着专名。
    let mut mark_cap: Option<(String, &'static str, String)> = None;

    // 块级标签：结束时要断段。
    let is_block = |n: &str| -> bool {
        matches!(
            n,
            "p" | "div" | "section" | "article" | "blockquote" | "li" | "ul" | "ol"
                | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "pre"
                | "figure" | "figcaption" | "dl" | "dt" | "dd" | "body" | "html"
        ) || (!drop_tables && matches!(n, "tr" | "td" | "th" | "table"))
    };
    // 丢弃块：整块内容都不进正文。
    //
    // v1.69.0：**表格整块丢弃**（用户死命令）。盗版 epub 里的表格多是
    // 「篇章字」表、生僻字表、出版信息表——排进竖排就是一堆错位方块。
    // `tbody/thead/tfoot/tr/td/th/caption` 一并丢：它们只在表格里出现，
    // 留着只会剩下散落的单元格文字。
    let is_skip = |n: &str| -> bool {
        matches!(
            n,
            "script" | "style" | "nav" | "header" | "footer" | "aside" | "head" | "noscript"
                | "svg" | "math" | "template"
        ) || (drop_tables
            && matches!(
                n,
                "table" | "tbody" | "thead" | "tfoot" | "tr" | "td" | "th" | "caption"
                    | "colgroup" | "col"
            ))
    };

    loop {
        events += 1;
        if events > MAX_EVENTS {
            err = Some(format!(
                "这一节的 XHTML 结构异常（事件数超过 {MAX_EVENTS}），已跳过余下内容"
            ));
            break;
        }
        match xml.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                if is_skip(&name) {
                    skip += 1;
                } else if skip == 0 {
                    // 划线标记开壳：只认一层，且块级标签先断段（`<u>` 不会
                    // 是块，但 `<span class=book>` 里可能有）——标记绝不跨段。
                    if mark_cap.is_none() {
                        if let Some(kind) = mark_kind_for(&name, &class_of(&e)) {
                            push_break(&mut para, &mut out);
                            mark_cap = Some((name, kind, String::new()));
                            buf.clear();
                            continue;
                        }
                    }
                    if is_block(&name) {
                        push_break(&mut para, &mut out);
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref());
                if skip == 0 && matches!(name.as_str(), "br" | "hr") {
                    push_break(&mut para, &mut out);
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                if is_skip(&name) {
                    skip = skip.saturating_sub(1);
                } else if skip == 0 {
                    // 收壳：跨度合法就转成 core 的划线标记，否则原样留字
                    // （宁可少画一条线，也不能把「整句下划线」标成人名）。
                    let closing = matches!(&mark_cap, Some((t, _, _)) if *t == name);
                    if closing {
                        let (_, kind, text) = mark_cap.take().unwrap();
                        let t = collapse_inline_space(&text);
                        let n = t.chars().count();
                        if n > 0 && n <= MARK_MAX_CHARS && !t.chars().any(crate::types::is_punct) {
                            para.push_str(&format!("〔{kind}:{t}〕{t}"));
                        } else if !t.is_empty() {
                            para.push_str(&t);
                        }
                    } else if is_block(&name) {
                        push_break(&mut para, &mut out);
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if skip == 0 {
                    let raw = String::from_utf8_lossy(t.as_ref());
                    let txt = unescape_html(&raw);
                    match mark_cap.as_mut() {
                        Some((_, _, cap)) => cap.push_str(&txt),
                        None => para.push_str(&txt),
                    }
                }
            }
            Ok(Event::CData(c)) => {
                if skip == 0 {
                    para.push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            Ok(_) => {}
            Err(e) => {
                err = Some(e.to_string());
                break; // 保已读到的部分
            }
        }
        buf.clear();
    }
    push_break(&mut para, &mut out);
    (out, err)
}

/// 段收口：把当前累积的文本压成一段（行内空白归一）后挂到输出。
fn push_break(para: &mut String, out: &mut String) {
    let t = collapse_inline_space(para);
    para.clear();
    if t.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&t);
}

/// 段内空白折叠：连续空格 / 换行 → 一个空格；句首句尾不留白。
///
/// 注意**不动全角空格**：`　　` 是段首缩进记号，折成一个空格就丢了。
fn collapse_inline_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true; // 段首当作已有空格，天然去头
    for c in s.chars() {
        match c {
            ' ' | '\t' | '\n' | '\u{00A0}' | '\u{200B}' => {
                if !prev_space {
                    out.push(' ');
                    prev_space = true;
                }
            }
            _ => {
                out.push(c);
                prev_space = c == '\u{3000}';
            }
        }
    }
    out.trim_end().to_string()
}

/// `ns:tag` → `tag`（XHTML 常带 `xhtml:` 前缀）。
fn local_name(q: &[u8]) -> String {
    let s = String::from_utf8_lossy(q);
    match s.rsplit(':').next() {
        Some(t) => t.to_ascii_lowercase(),
        None => s.to_ascii_lowercase(),
    }
}

/// HTML 实体还原：数字实体按码位、常见命名实体查表，**不认识的原样保留**。
///
/// 不认识就保留而不是删掉：`&foo;` 出现在正文里，读者看得见就知道原文有东西；
/// 删掉则永久丢字。
pub fn unescape_html(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let b: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != '&' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        // 往前找分号（最多 12 个字符，避免把长段落当实体）。
        let end = (i + 1..b.len().min(i + 12)).find(|&j| b[j] == ';');
        let Some(end) = end else {
            out.push('&');
            i += 1;
            continue;
        };
        let name: String = b[i + 1..end].iter().collect();
        let rep = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = name.strip_prefix('#') {
            dec.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            named_entity(&name)
        };
        match rep {
            Some(c) => {
                out.push(c);
                i = end + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// 常见命名实体（够用即可，未知的交给调用方原样保留）。
fn named_entity(n: &str) -> Option<char> {
    Some(match n {
        "nbsp" => '\u{00A0}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{3000}',
        "thinsp" => '\u{2009}',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "laquo" => '«',
        "raquo" => '»',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "middot" => '·',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "deg" => '°',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "sect" => '§',
        "para" => '¶',
        "dagger" => '†',
        "bull" => '•',
        "prime" => '′',
        "Prime" => '″',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "zwsp" => '\u{200B}',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_scripts_nav_and_keeps_paragraphs() {
        let src = r#"<html><head><title>書名</title><style>p{color:red}</style></head>
            <body><nav><a href="/">首页</a></nav>
            <h1>道德經</h1>
            <p>道可道，<b>非常</b>道。</p>
            <script>alert(1)</script>
            <header>页眉</header>
            <p>名可名<br/>非常名&nbsp;也。</p>
            </body></html>"#;
        let (t, e) = extract_html_str(src);
        assert!(e.is_none(), "不该报错：{e:?}");
        assert!(!t.contains("alert"), "脚本漏进正文");
        assert!(!t.contains("页眉"), "页眉漏进正文");
        assert!(!t.contains("color:red"), "CSS 漏进正文");
        assert!(t.contains("道可道，非常道。"), "行内标签没剥干净：{t}");
        // `<br/>` 按规矩转成换行（诗行由此分行），故第 2 段被切成两行。
        assert_eq!(t.lines().count(), 4, "段落/换行数不对：{t:?}");
        assert!(t.contains("非常名 也。"), "&nbsp; 没还原成空格：{t:?}");
    }

    /// 专名号：`<u>` / `class` 带 book 的 span 转成 core 的划线标记，
    /// **不**在正文里留下划线标签本身，也不把长句硬标成人名。
    #[test]
    fn underline_becomes_a_proper_name_mark() {
        let src = r#"<html><body>
            <p><u>孔子</u>曰：<span class="book-name">春秋</span>者，天子之事也。</p>
            <p><u>这一整句都被画了下划线的现代排版句子不该被标成专名</u></p>
            <p><span class="quiet">普通 span 不动</span></p>
            </body></html>"#;
        let (t, e) = extract_html_str(src);
        assert!(e.is_none(), "不该报错：{e:?}");
        assert!(t.contains("〔人:孔子〕孔子"), "`<u>` 没转成专名：{t}");
        assert!(t.contains("〔书:春秋〕春秋"), "书名 span 没转成书名线：{t}");
        // 超过跨度上限的（现代「整句下划线」）不标，但字一个都不许丢。
        assert!(!t.contains("〔人:这一整句"), "长跨度不该标成专名：{t}");
        assert!(t.contains("这一整句"), "长跨度的字被吞了：{t}");
        // 普通 span 不带线。
        assert!(!t.contains("〔人:普通"), "{t}");
        assert!(t.contains("普通 span 不动"), "{t}");
        // 标记壳不能残留成正文。
        assert!(!t.contains("<u>") && !t.contains("class="), "{t}");
    }

    /// 插图：`<img>` 与「只有图的 `<div>`」一律不进正文，**连 alt 都不取**
    /// （alt 常是「印章」「藏书票」这类出版杂字，取了就是往正文里塞垃圾）。
    #[test]
    fn images_are_dropped_and_alt_never_leaks() {
        let src = r#"<html><body>
            <p>道可道，非常道。</p>
            <div class="illus"><img src="seal.png" alt="藏書票"/></div>
            <div><img src="blank.gif"/></div>
            <p><img src="inline.png" alt="印章"/>名可名，非常名。</p>
            <svg><text>矢量水印</text></svg>
            </body></html>"#;
        let (t, e) = extract_html_str(src);
        assert!(e.is_none(), "不该报错：{e:?}");
        assert!(!t.contains("藏書票"), "alt 漏进正文：{t:?}");
        assert!(!t.contains("印章"), "alt 漏进正文：{t:?}");
        assert!(!t.contains("seal.png") && !t.contains("src="), "标签壳漏进正文：{t:?}");
        assert!(!t.contains("矢量水印"), "svg 漏进正文：{t:?}");
        // 纯图容器不该留下空行（空段被 push_break 吞掉）
        assert_eq!(
            t.lines().filter(|l| !l.trim().is_empty()).count(),
            2,
            "纯图 div 不该留下空段：{t:?}"
        );
        assert!(t.contains("道可道，非常道。"), "正文被误删：{t:?}");
        assert!(t.contains("名可名，非常名。"), "正文被误删：{t:?}");
    }

    /// 实体还原：`&nbsp;` 变空格、`&ldquo;/&rdquo;` 变中文弯引号、
    /// `&mdash;` 变破折号——还原成**汉字标点**而不是留着 `&xxx;` 的字面量。
    #[test]
    fn named_entities_become_real_punctuation() {
        let (t, _) = extract_html_str(
            r#"<html><body><p>&ldquo;道&rdquo;&mdash;&nbsp;可道也&hellip;</p></body></html>"#,
        );
        assert!(t.contains("“道”—"), "弯引号/破折号没还原：{t:?}");
        assert!(t.contains("可道也…"), "省略号没还原：{t:?}");
        assert!(!t.contains("&ldquo;") && !t.contains("&mdash;"), "实体没还原：{t:?}");
        assert!(!t.contains('\u{00A0}'), "&nbsp; 该还原成普通空格：{t:?}");
    }

    #[test]
    fn unknown_entities_are_kept_verbatim() {
        assert_eq!(unescape_html("&foo;道"), "&foo;道");
        assert_eq!(unescape_html("&#36947;"), "道");
        assert_eq!(unescape_html("&#x9053;"), "道");
    }
}
