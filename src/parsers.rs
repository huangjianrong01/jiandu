//! 通用数据清洗与解析管线（Strategy Pattern 重构版）。
//!
//! 一句话：`UniversalParser::parse(kind, raw) -> Result<Vec<RawChapter>>`。
//! 不管进来的是 txt / md / html / xml / json / epub，出去的都是同一份干净结构：
//!
//! ```text
//! [ { title: "卷一", content: "正文[注]夹注[/注]正文[高亮]着重[/高亮]" } ]
//! ```
//!
//! ## 策略模式（Strategy Pattern）
//!
//! 每种格式是一个独立的 `AncientParser` 实现，只负责「把该格式的原始文本
//! 解析成 `Vec<RawChapter>`」这一件事——降噪、抽章节、转标记各写各的，互
//! 不耦合。`UniversalParser` 只做两件事：
//!
//! 1. 按 [`SrcKind`] 选出对应的策略（解析阶段）；
//! 2. 把解析结果统一送进 **`cleanup_ancient_text`**（古籍化后处理，绝对红线）。
//!
//! 后处理与格式无关——数字强制汉字化、现代标点竖排化、残留标签清理、
//! 孤立页码过滤，六条格式走的是**同一道闸**。新增格式只需再写一个
//! `AncientParser` 并到 `UniversalParser::parse` 里加一行分支，后处理零改动。
//!
//! ## 六格式职责边界
//!
//! * **HTML**（网页剪藏）：去 `<script>/<style>/<nav>/<footer>/<header>` 与
//!   广告 / 版权 class，抽 `<h1>~<h6>` 作章题、`<p>/<article>/<div>` 作正文，
//!   内联 `<b>/<strong>` → `[高亮]`、`<sup>/<span class="note">` → `[注]`。
//! * **EPUB**（标准电子书）：容器，逐份 xhtml 走 `vclean` 五阶段
//!   （OPF spine 顺序、编码嗅探、页码孤行清理、`<ruby><rt>` → `[拼音:]`）。
//! * **TXT**（纯文本兜底）：编码 / BOM 探测、去缩进空行、半角标点转全角、
//!   `第X章回卷节` 切章、超长无章按 3000 字强切。
//! * **Markdown**：`#` 标题、引用 `>` → `[注]`、`**加粗**` → `[高亮]`、
//!   剔除图片与表格只留文字。
//! * **XML**（古籍 TEI / DocBook 等）：白名单标签抽 textContent
//!   （`<title>/<head>` 作章题、`<p>/<div>` 作正文、`<note>` → `[注]`），
//!   剥离所有属性与 `<teiHeader>/<front>/<back>` 等元数据区。
//! * **JSON**：容错——缺 `chapters` 数组自动包成单章；只留 `title`/`content`。
//!
//! 最后 `chapters_to_book` 把 `[高亮]` 摘成着重 `Note`、把 `[注]` 留给排版
//! 引擎展开成双行夹注（见 `crate::pipeline`）。

use crate::pipeline::{RawChapter, SrcKind, parse_html, parse_jsonish, parse_markdown, parse_txt};
use crate::vclean::clean_epub;
use anyhow::Result;

/// 单格式解析策略：只负责「原始文本 → 标准章数组」，不做古籍化后处理。
///
/// 后处理（数字汉字化、标点竖排化、标签清理、页码过滤）统一由
/// [`cleanup_ancient_text`] 兜底，故这里产出的文本可以「脏」一点——
/// 残留的 `&nbsp;`、英文标点、阿拉伯数字都会在后处理里被收干净。
pub trait AncientParser {
    /// 该策略对应的源头格式（用于日志 / 调试）。
    fn kind(&self) -> SrcKind;
    /// 解析原始文本；失败返回可读的错误（绝不 panic / 白屏）。
    fn parse(&self, raw: &str) -> Result<Vec<RawChapter>>;
}

/// HTML 网页剪藏（见模块文档）。
pub struct HtmlParser;
impl AncientParser for HtmlParser {
    fn kind(&self) -> SrcKind {
        SrcKind::Html
    }
    fn parse(&self, raw: &str) -> Result<Vec<RawChapter>> {
        // 预处理：整块丢弃 class 含广告 / 版权关键字的节点（见 [`drop_ad_nodes`]）。
        // `DROP_BLOCKS` 只按标签名（nav/footer/…）丢，class 层面的噪声靠这一步补。
        Ok(parse_html(&drop_ad_nodes(raw)))
    }
}

/// 广告 / 版权 / 推广类节点的 class 关键字（子串匹配，大小写不敏感）。
///
/// `<div class="ad-banner">`、`<aside class="copyright">` 这类「带广告味的 class」
/// 在网页剪藏里极常见，但正文一个字都不是——整棵子树（含其内的图片、链接、
/// 脚本）一并删掉，绝不留进排版引擎。
/// 直接整块丢弃的标签（结构性 / 非正文 / 脚本样式）。
const DROP_TAGS: &[&str] = &[
    "nav", "header", "footer", "aside", "script", "style", "noscript", "svg", "form",
    "iframe", "button", "canvas",
];

/// `class` / `id` 命中以下任一关键字的整段子树也整块丢弃（广告 / 版权 / 导航 /
/// 播放器 / 工具条 / 登录区 等现代网页 chrome 噪声）。
///
/// 注意：**只放侧栏 / 顶栏 / 播放器等明确非正文的词**，绝不放 `left` 这类
/// 常被用来包裹正文的布局类（古诗文网的诗歌正文就躺在 `<div class="left">` 里）。
const NOISE_HINTS: &[&str] = &[
    "ad", "advert", "banner", "promo", "sponsor", "copyright", "版权", "recommend", "nav",
    "menu", "header", "footer", "sidebar", "aside", "tool", "toolbar", "player", "audio",
    "play", "speed", "login", "登录", "register", "注册", "share", "分享", "weixin", "微信",
    "qr", "二维码", "maintop", "contyishang", "unlogin", "icon", "app", "right",
];

/// 删除所有整块噪声子树：标签命中 [`DROP_TAGS`] 或 `class`/`id` 含 [`NOISE_HINTS`]
/// 关键字的整段（按同名标签嵌套计数找配对闭合标签，避免只删开标签导致后续文本全丢）。
fn drop_ad_nodes(html: &str) -> String {
    let chars: Vec<char> = html.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < n {
        if chars[i] == '<' {
            let mut j = i + 1;
            while j < n && chars[j] != '>' {
                j += 1;
            }
            let tag: String = if j < n {
                chars[i + 1..j].iter().collect()
            } else {
                chars[i + 1..].iter().collect()
            };
            let is_close = tag.trim_start().starts_with('/');
            let self_close = tag.trim_end().ends_with('/');
            let tagname: String = tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let tag_lower = tag.to_ascii_lowercase();
            let is_drop_tag = DROP_TAGS.contains(&tagname.as_str());
            let is_hidden = tag_lower.contains("display:none") || tag_lower.contains("visibility:hidden");
            let hit = is_drop_tag
                || is_hidden
                || NOISE_HINTS.iter().any(|h| tag_lower.contains(h));
            // 开标签且其 class 命中：跳到同名配对的闭合标签（含自身这一层）。
            if !is_close && hit {
                if self_close {
                    // 自闭合（少见）：直接跳过这一枚标签。
                    i = if j < n { j + 1 } else { n };
                    continue;
                }
                let mut depth = 1usize;
                let mut k = j + 1;
                while k < n && depth > 0 {
                    if chars[k] == '<' {
                        let mut e = k + 1;
                        while e < n && chars[e] != '>' {
                            e += 1;
                        }
                        let t: String = if e < n {
                            chars[k + 1..e].iter().collect()
                        } else {
                            chars[k + 1..].iter().collect()
                        };
                        let c_close = t.trim_start().starts_with('/');
                        let c_self = t.trim_end().ends_with('/');
                        let tn: String = t
                            .trim_start_matches('/')
                            .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                            .next()
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if !c_close && !c_self && tn == tagname {
                            depth += 1;
                        } else if c_close && tn == tagname {
                            depth -= 1;
                        }
                        k = if e < n { e + 1 } else { n };
                    } else {
                        k += 1;
                    }
                }
                i = k;
                continue;
            }
            // 普通标签：原样输出。
            out.extend(&chars[i..if j < n { j + 1 } else { n }]);
            i = if j < n { j + 1 } else { n };
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 标准 EPUB 电子书（走 `vclean` 五阶段）。
pub struct EpubParser;
impl AncientParser for EpubParser {
    fn kind(&self) -> SrcKind {
        SrcKind::Epub
    }
    fn parse(&self, raw: &str) -> Result<Vec<RawChapter>> {
        Ok(clean_epub(raw))
    }
}

/// 纯文本兜底（编码 / BOM / 切章 / 全角标点）。
pub struct TxtParser;
impl AncientParser for TxtParser {
    fn kind(&self) -> SrcKind {
        SrcKind::Txt
    }
    fn parse(&self, raw: &str) -> Result<Vec<RawChapter>> {
        Ok(parse_txt(raw))
    }
}

/// Markdown（标题 / 引用 / 加粗 / 去图去表）。
pub struct MarkdownParser;
impl AncientParser for MarkdownParser {
    fn kind(&self) -> SrcKind {
        SrcKind::Markdown
    }
    fn parse(&self, raw: &str) -> Result<Vec<RawChapter>> {
        Ok(parse_markdown(raw))
    }
}

/// 古籍 TEI / 通用 XML（白名单抽 textContent）。
pub struct XmlParser;
impl AncientParser for XmlParser {
    fn kind(&self) -> SrcKind {
        SrcKind::Xml
    }
    fn parse(&self, raw: &str) -> Result<Vec<RawChapter>> {
        Ok(parse_xml(raw))
    }
}

/// JSON（容错：缺 chapters 自动包单章）。
pub struct JsonParser;
impl AncientParser for JsonParser {
    fn kind(&self) -> SrcKind {
        SrcKind::Json
    }
    fn parse(&self, raw: &str) -> Result<Vec<RawChapter>> {
        parse_jsonish(raw)
    }
}

/// 统一调度入口（策略选择 + 古籍化后处理）。
///
/// ```text
/// 解析阶段（按格式选策略）  →  cleanup_ancient_text（格式无关的绝对红线）
/// ```
///
/// 所有格式在这里汇流，后处理只写一遍、六格式共用——这正是策略模式相对
/// 于「一个巨型 match 里又解析又清洗」的最大收益。
pub struct UniversalParser;

impl UniversalParser {
    /// 已知格式时直接解析字符串；解析完对每一章的 `title` 与 `content` 过一遍
    /// 古籍化后处理再返回。
    pub fn parse(kind: SrcKind, raw: &str) -> Result<Vec<RawChapter>> {
        let chapters = match kind {
            SrcKind::Html => HtmlParser.parse(raw)?,
            SrcKind::Epub => EpubParser.parse(raw)?,
            SrcKind::Txt => TxtParser.parse(raw)?,
            SrcKind::Markdown => MarkdownParser.parse(raw)?,
            SrcKind::Xml => XmlParser.parse(raw)?,
            SrcKind::Json => JsonParser.parse(raw)?,
        };
        Ok(chapters
            .into_iter()
            .map(|mut c| {
                c.title = cleanup_ancient_text(&c.title);
                c.content = cleanup_ancient_text(&c.content);
                c
            })
            .collect())
    }
}

// ------------------------------------------------------------------
// 统一古籍化后处理（绝对红线）
// ------------------------------------------------------------------

/// **古籍化后处理**：所有格式解析出的文本最终都必须流经此函数。
///
/// 四道工序，顺序有讲究：
///
/// 1. **页码孤行**：纯数字（≥4 位）独占一行的，按漏网 EPUB 页码整行删掉
///    （绝不混入正文）；
/// 2. **数字强制汉字化**：所有连续阿拉伯数字（含全角）转汉字——竖排里
///    `223` 会被拆成三格竖摞，源头就写成「二二三／二百二十三」根治；
/// 3. **HTML 残片**：实体还原（`&nbsp;`/`&lt;`…）→ 去标签（`<br>` 先转换行）；
/// 4. **现代标点竖排化 + 残留标签清理**：只对「标记之外」的文本跑——
///    《》→︽︾、……→︙、半角→全角、孤立 `[注]` 壳连壳删。
///
/// 标记（`[注]…[/注]`、`[高亮]…[/高亮]`、`[拼音:…]`）受 [`crate::vclean::map_outside_markers`]
/// 保护——它们的内部绝不被标点 / 实体清洗碰到，否则 `[拼音:hàn]` 的冒号
/// 会被转全角、断掉注音解析。
pub fn cleanup_ancient_text(text: &str) -> String {
    // 1) 页码孤行（在任何数字转汉字之前——否则会被当成数字转成汉字）。
    let s = strip_page_lines(text);
    // 2) 先卸掉历史 `[数字:…]` 壳，让里面的数字也能被汉字化。
    let s = crate::pipeline::strip_digit_marks(&s);
    // 3) 阿拉伯数字 → 汉字（纪年逐字读、一般按数值读，最长 6 位，字母/# 粘连跳过）。
    let s = crate::numcn::normalize_numbers(&s);
    // 4) HTML 残片：`<br>` → 换行、实体还原、去标签。
    let s = strip_html_residue(&s);
    // 5) 标点竖排化 + 书名号 / 省略号：只对标记之外的文本跑（标记保护）。
    let s = crate::vclean::map_outside_markers(&s, cleanup_punct_inner);
    // 6) 孤立的 `[注]` 壳（不成对的）连壳删，保留完整 `[注]…[/注]` 夹注。
    crate::types::strip_dangling_note_marks(&s)
}

/// 工序 1：删掉「纯数字（≥4 位）独占一行」的页码孤行。
///
/// 判据：该行去掉空白后**全部是 ASCII 数字**、且数字位数 ≥ 4。带 `第/回/年`
/// 等汉字的数字串（如 `第1234回`、`2024年`）不是纯数字行 → 保留、交给
/// 数字汉字化去处理。这是最稳妥的「漏网页码」识别——EPUB 页码几乎都独占一行。
fn strip_page_lines(text: &str) -> String {
    text.lines()
        .map(|line| {
            let t = line.trim();
            let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
            let is_page_number = !t.is_empty()
                && digits.len() >= 4
                && t.chars().all(|c| c.is_ascii_digit() || c == ' ' || c == '　');
            if is_page_number {
                "" // 页码孤行：整行清空（下方 join 自然变成空行，再被章节逻辑忽略）
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 工序 4：HTML 残片清理（`<br>` 转换行 → 实体还原 → 去标签）。
///
/// 解析阶段已经剥过标签，这里只是兜底——漏网的 `&nbsp;`、`</p>` 残留尖括号
/// 绝不能流进排版引擎（否则会被画成一串方括号 / 字面 `&amp;`）。
fn strip_html_residue(s: &str) -> String {
    // `<br>` 先转成换行，避免前后正文黏成一团。
    let s = s
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    // 实体还原（`&nbsp;` → 不换行空格、`&lt;` → `<` …）。
    let s = crate::pipeline::decode_entities(&s);
    // 去标签（任何残留的 `<…>`）。
    crate::pipeline::strip_tags(&s)
}

/// 工序 5（标记之外的那一段）：半角 / 全角现代标点竖排化 + 书名号 / 省略号。
fn cleanup_punct_inner(seg: &str) -> String {
    // 半角 → 全角（`，。？！；：（）`；数字里的 `.`/`,` 内部已在 numcn 处理过，
    // fullwidth_punct 自身也会跳过数字串里的标点，不会把 `3.14` 掰断）。
    let s = crate::pipeline::fullwidth_punct(seg);
    // 书名号 → 竖排专用形（刻本不用横排的《》）。
    let s = s.replace('《', "︽").replace('》', "︾");
    // 现代括号 → 直角引号（竖排里（）开口朝错方向，且这本就是「引用 / 括注」的语义）。
    let s = s.replace('（', "「").replace('）', "」");
    // 省略号 → 竖排省略号（…… 是横排躺倒的两点，竖排该是居中的竖点）。
    s.replace("……", "︙").replace('…', "︙")
}

// ------------------------------------------------------------------
// XML 解析（手写标签白名单抽取，与项目「不引重型 DOM 库」的取向一致）
// ------------------------------------------------------------------

/// 古籍 TEI / 通用 XML 解析：按白名单标签抽 `textContent`，属性一律丢弃。
///
/// 规则（见模块文档）：
/// * `<title>` / `<head>` → 章题（出现即开新章，上一章先落盘）；
/// * `<p>` / `<ab>` / `<lg>` / `<seg>` / `<div>` / `<body>` → 正文段落；
/// * `<note>` → 包成 `[注]…[/注]`（古籍夹注）；
/// * `<teiHeader>` / `<header>` / `<front>` / `<back>` / `<fig>` / `<facsimile>`
///   整段跳过（元数据 / 图版区，不是正文）；
/// * 其它未知标签（`<hi>`、`<q>` …）不改当前语义，其文本按外层 mode 收集；
/// * 属性（id / class / style）天然被丢——`textContent` 本就不含属性。
pub fn parse_xml(raw: &str) -> Vec<RawChapter> {
    #[derive(Clone, Copy)]
    enum Mode {
        None,
        Title,
        Body,
        Note,
    }

    // 把当前 `tb` 按 mode 落进对应缓冲（段间空白不记）。作为内部自由函数，
    // 每次调用临时借用、调用即释放，避免与循环里其它可变借用冲突。
    fn flush_text(
        tb: &mut String,
        mode: Mode,
        title_buf: &mut String,
        body_buf: &mut String,
        note_buf: &mut String,
    ) {
        let t = tb.trim().to_string();
        tb.clear();
        if t.is_empty() {
            return;
        }
        match mode {
            Mode::Title => title_buf.push_str(&t),
            Mode::Body => body_buf.push_str(&t),
            Mode::Note => note_buf.push_str(&t),
            Mode::None => {}
        }
    }

    fn flush_para(body_buf: &mut String, paras: &mut Vec<String>) {
        let p = body_buf.trim().to_string();
        body_buf.clear();
        if !p.is_empty() {
            paras.push(p);
        }
    }

    fn flush_chapter(title: &mut String, paras: &mut Vec<String>, out: &mut Vec<RawChapter>) {
        let body = paras
            .iter()
            .filter(|p| !p.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if body.trim().is_empty() {
            return;
        }
        let t = std::mem::take(title);
        let title_str = if t.trim().is_empty() {
            "正文".to_string()
        } else {
            t.trim().to_string()
        };
        out.push(RawChapter::new(title_str, body));
        *title = String::from("正文");
        paras.clear();
    }

    let mut out: Vec<RawChapter> = Vec::new();
    let mut title = String::from("正文");
    let mut paras: Vec<String> = Vec::new();
    let mut title_buf = String::new();
    let mut body_buf = String::new();
    let mut note_buf = String::new();
    let mut mode = Mode::None;
    let mut stack: Vec<Mode> = Vec::new();
    let mut skip = 0usize;

    let cs: Vec<char> = raw.chars().collect();
    let mut i = 0usize;
    let mut tb = String::new(); // 当前 mode 下累积的纯文本

    while i < cs.len() {
        if cs[i] == '<' {
            // 先把手头累积的文本按当前 mode 落盘，再解析这个标签。
            flush_text(&mut tb, mode, &mut title_buf, &mut body_buf, &mut note_buf);
            // 找标签尾 '>'（XML 注释 / CDATA / 声明一并当标签吞掉，内部文本不收集）。
            let mut j = i + 1;
            while j < cs.len() && cs[j] != '>' {
                j += 1;
            }
            let tag = if j < cs.len() {
                cs[i + 1..j].iter().collect::<String>()
            } else {
                cs[i + 1..].iter().collect::<String>()
            };
            let closed = tag.starts_with('/');
            let self_close = tag.trim_end().ends_with('/');
            let inner = if closed { tag[1..].trim() } else { tag.trim() };
            // 标签名（不含属性、不含 `/`）。
            let name: String = inner
                .split(|c: char| c.is_whitespace() || c == '/' || c == '>' || c == '<')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let is_skip_tag = matches!(
                name.as_str(),
                "teiheader" | "header" | "front" | "back" | "fig" | "facsimile" | "graphic"
            );

            if closed {
                match name.as_str() {
                    "title" | "head" => {
                        if skip == 0 {
                            let t = std::mem::take(&mut title_buf).trim().to_string();
                            if !t.is_empty() {
                                title = t;
                            }
                            mode = stack.pop().unwrap_or(Mode::None);
                        }
                    }
                    "note" => {
                        if skip == 0 {
                            let n = std::mem::take(&mut note_buf).trim().to_string();
                            if !n.is_empty() {
                                body_buf.push_str(&format!("[注]{n}[/注]"));
                            }
                            mode = stack.pop().unwrap_or(Mode::None);
                        }
                    }
                    "p" | "ab" | "lg" | "seg" => {
                        if skip == 0 {
                            flush_para(&mut body_buf, &mut paras);
                            mode = Mode::Body;
                        }
                    }
                    "div" | "body" => {
                        if skip == 0 {
                            flush_para(&mut body_buf, &mut paras);
                            flush_chapter(&mut title, &mut paras, &mut out);
                            mode = Mode::None;
                        }
                    }
                    _ => {
                        if is_skip_tag && skip > 0 {
                            skip -= 1;
                        }
                    }
                }
            } else if !self_close {
                if is_skip_tag {
                    skip += 1;
                } else {
                    match name.as_str() {
                        "title" | "head" => {
                            if skip == 0 {
                                // 新章题出现：先把上一章落盘，再开新章。
                                flush_para(&mut body_buf, &mut paras);
                                flush_chapter(&mut title, &mut paras, &mut out);
                                title_buf.clear();
                                stack.push(std::mem::replace(&mut mode, Mode::Title));
                            }
                        }
                        "note" => {
                            if skip == 0 {
                                note_buf.clear();
                                stack.push(std::mem::replace(&mut mode, Mode::Note));
                            }
                        }
                        "p" | "ab" | "lg" | "seg" | "div" | "body" => {
                            if skip == 0 {
                                mode = Mode::Body;
                            }
                        }
                        _ => {
                            // 其它未知开放标签（hi/q/ref…）：不改语义，文本仍按外层 mode 收。
                        }
                    }
                }
            }
            i = if j < cs.len() { j + 1 } else { cs.len() };
            continue;
        }
        tb.push(cs[i]);
        i += 1;
    }
    // 文件末尾：把残留文本、段落、章节落盘。
    flush_text(&mut tb, mode, &mut title_buf, &mut body_buf, &mut note_buf);
    flush_para(&mut body_buf, &mut paras);
    flush_chapter(&mut title, &mut paras, &mut out);
    if out.is_empty() {
        out.push(RawChapter::new("正文", String::new()));
    }
    out
}

// ------------------------------------------------------------------
// 测试
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::decode_bytes;

    /// 数字强制汉字化：年份、页码、数量都转，绝不残留阿拉伯数字被竖排拆散。
    #[test]
    fn digits_become_chinese_not_vertical_runs() {
        let s = cleanup_ancient_text("贞观223年，卷十五第3回，共1980页");
        assert!(!s.contains('2'), "数字未转：{s}");
        assert!(!s.contains('3'), "数字未转：{s}");
        assert!(s.contains("贞观二二三年") || s.contains("贞观二百二十三年"), "年份应汉字化：{s}");
        assert!(s.contains("卷十五") || s.contains("卷一十五"), "章序号应汉字化：{s}");
    }

    /// 现代标点竖排化：书名号、省略号转专用形；（）→「」。
    #[test]
    fn modern_punct_becomes_vertical() {
        let s = cleanup_ancient_text("读《红楼梦》……他说（注：原书作）");
        assert!(s.contains("︽"), "书名号未转：{s}");
        assert!(s.contains("︾"), "书名号未转：{s}");
        assert!(s.contains("︙"), "省略号未转：{s}");
        assert!(!s.contains("《"), "残留横排书名号：{s}");
    }

    /// 残留标签清理：HTML 实体还原、完整 [注]…[/注] 保留、落单壳删除。
    #[test]
    fn stray_markup_is_stripped() {
        let s = cleanup_ancient_text("正文&lt;br&gt;&nbsp;前[注]成对[/注]后。落单[注]壳尾");
        assert!(!s.contains("&lt;"), "实体未还原：{s}");
        assert!(!s.contains("&nbsp;"), "实体未还原：{s}");
        assert!(s.contains("[注]成对[/注]"), "完整夹注被误删：{s}");
        assert!(!s.contains("落单[注]"), "孤立 [注] 壳未删：{s}");
    }

    /// 落单的收标记 [/注]（前方无配对开）也要连壳删。
    #[test]
    fn dangling_close_mark_dropped() {
        let s = cleanup_ancient_text("尾[/注]落单");
        assert!(!s.contains("[/注]"), "孤立收标记未删：{s}");
        assert_eq!(s, "尾落单");
    }

    /// 孤立页码过滤：整行纯数字（≥4 位）应被删掉。
    #[test]
    fn isolated_page_numbers_dropped() {
        let raw = "第一章\n正文若干。\n1234\n第二章\n又有正文。";
        let s = cleanup_ancient_text(raw);
        assert!(!s.contains("1234"), "页码孤行未删：{s}");
        assert!(s.contains("第一章") && s.contains("第二章"));
    }

    /// 标记保护：清洗绝不能碰 `[拼音:…]` 的内部（冒号是注音语法）。
    #[test]
    fn pinyin_marker_survives_cleanup() {
        let s = cleanup_ancient_text("长[拼音:cháng]江");
        assert!(s.contains("[拼音:cháng]"), "注音标记被破坏：{s}");
        assert!(!s.contains("[拼音：cháng]"), "冒号被转全角：{s}");
    }

    /// XML：TEI 片段能抽出章题与正文，并剥掉 teiHeader 与属性。
    #[test]
    fn tei_xml_extracts_title_and_body() {
        let xml = r#"<?xml version="1.0"?>
<TEI>
  <teiHeader><titleStmt><title>原书题名</title></titleStmt></teiHeader>
  <text><body>
    <div type="chapter">
      <head>卷一 天地</head>
      <p>天地玄黄，<hi>宇宙</hi>洪荒。</p>
      <p>寒来暑往，<note>四时也</note>秋收冬藏。</p>
    </div>
  </body></TEI>"#;
        let chs = parse_xml(xml);
        // teiHeader 里的「原书题名」不可进正文。
        assert!(!chs.iter().any(|c| c.content.contains("原书题名")), "元数据泄漏：{chs:?}");
        let first = chs.iter().find(|c| c.title.contains("卷一")).expect("应有卷一");
        assert!(first.content.contains("天地玄黄"), "正文缺失：{first:?}");
        assert!(first.content.contains("[注]四时也[/注]"), "note 未转夹注：{first:?}");
        // 属性 type="chapter" 不应进正文。
        assert!(!first.content.contains("type="), "属性残留：{first:?}");
    }

    /// 端到端：HTML 网页经 UniversalParser 后，输出既干净又结构化。
    #[test]
    fn universal_parser_html_is_clean() {
        let html = r#"<html><head><style>.a{color:red}</style></head>
<body><nav>菜单</nav>
<h1>赤壁赋</h1>
<div class="ad">广告</div>
<article><p>壬戌之秋，七月既望。清风徐来，水波不兴。&#20013;华。</p>
<p>举酒属客，诵明月之诗。</p></article>
<script>var x=1;</script>
</body></html>"#;
        let chs = UniversalParser::parse(SrcKind::Html, html).unwrap();
        let c = &chs[0];
        assert_eq!(c.title, "赤壁赋", "章题应为 h1：{c:?}");
        assert!(!c.content.contains("广告"), "噪声未滤：{}", c.content);
        assert!(!c.content.contains("var x"), "script 未滤：{}", c.content);
        assert!(!c.content.contains("<"), "残留尖括号：{}", c.content);
        assert!(!c.content.contains("style"), "style 未剥：{}", c.content);
    }

    /// 网页实弹：若本地有真实抓取的网页，跑一遍并断言干净（无文件则跳过）。
    ///
    /// 跑法（任取一个古典文本网页）：
    /// ```text
    /// curl -L 'https://zh.wikisource.org/wiki/...' -o /tmp/jiandu_web_test.html
    /// cargo test --lib parsers::tests::webpage_smoke -- --nocapture
    /// ```
    #[test]
    fn webpage_smoke() {
        let path = "/tmp/jiandu_web_test.html";
        if !std::path::Path::new(path).exists() {
            return; // 没有真实网页就跳过，不阻塞 CI。
        }
        let bytes = std::fs::read(path).unwrap();
        let raw = decode_bytes(&bytes);
        let chs = UniversalParser::parse(SrcKind::Html, &raw).unwrap();
        let mut dirty = false;
        for c in &chs {
            if c.content.contains('<') || c.content.contains("&nbsp;") || c.content.contains("[注]不成对") {
                dirty = true;
            }
        }
        println!("章节数 = {}，首章标题 = {:?}", chs.len(), chs.first().map(|c| &c.title));
        assert!(!dirty, "真实网页清洗不干净");
    }
}
