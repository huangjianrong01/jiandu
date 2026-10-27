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
        // 上标幽灵字与上标注号同理先过一遍 [`strip_sup_marks`]。
        let html = strip_sup_marks(&drop_ad_nodes(raw));
        Ok(drop_front_matter(parse_html(&html)))
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
        // 上标幽灵字先吃掉再进五阶段——`<sup>` 一旦被当成「注」，
        // 后面再想认出它是杂质就晚了（注文与正文已经混成一团）。
        Ok(drop_front_matter(clean_epub(&strip_sup_marks(raw))))
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
/// 七道工序，顺序有讲究：
///
/// 0. **头部幽灵**（v1.68.1）：BOM / 零宽 / 全角空格 / 多余空行 / YAML 元数据
///    块一律切除——它们是「第一列顶部幽灵空行」的元凶；
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
    // 0) 头部幽灵（BOM / 零宽 / 全角空格 / 连续空行 / YAML 头）：必须在最前面
    //    ——后面的工序都按「行」走，行首那几个看不见的字符一留下来就会占字格。
    let s = crate::cleaner::strip_head_junk(text);
    // 1) 页码孤行（在任何数字转汉字之前——否则会被当成数字转成汉字）。
    let s = strip_page_lines(&s);
    // 2) 先卸掉历史 `[数字:…]` 壳，让里面的数字也能被汉字化。
    let s = crate::pipeline::strip_digit_marks(&s);
    // 3) 阿拉伯数字 → 汉字（纪年逐字读、一般按数值读，最长 6 位，字母/# 粘连跳过）。
    let s = crate::numcn::normalize_numbers(&s);
    // 4) HTML 残片：`<br>` → 换行、实体还原、去标签。
    let s = strip_html_residue(&s);
    // 4.5) **注疏标签只去壳、留肉身**（v1.68.1）：`[ 注 ]` / `[疏]` / `[/疏]`
    //    一律规范化成自家的 `[注]` / `[/注]`；不认识的方括号**只削括号**，
    //    里头的字一个不少地留下。放在标点化之前——规范好壳，下一步的
    //    「标记保护」才认得出它们，注疏正文才不会被当正文标点化。
    let s = crate::cleaner::normalize_note_shells(&s);
    // 4.6) 落单的书名号：跨段截断的《》删掉（成对的转 ︽︾，见工序 5）。
    let s = crate::cleaner::strip_orphan_title_marks(&s);
    // 5) 标点竖排化 + 书名号 / 省略号：只对标记之外的文本跑（标记保护）。
    let s = crate::vclean::map_outside_markers(&s, cleanup_punct_inner);
    // 5.5) 现代括号（`〔…〕` / `【…】` / `[…]`）一律不许进正文流：短的转
    // 夹注、长的剥壳（见 `purge_bracket_marks`）。放在标点化之后——先让
    // 正文的标点定形，再把括注整段摘成夹注。
    let s = purge_bracket_marks(&s);
    // 6) 单字杂质夹注（`[注]已[/注]` 这类上标残渣）连壳删。
    let s = scrub_ghost_notes(&s);
    // 7) 孤立的 `[注]` 壳（不成对的）连壳删，保留完整 `[注]…[/注]` 夹注。
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
    // 上标先裁决（吃掉注号 / 幽灵字，长上标转夹注）——必须在**去标签之前**，
    // 否则 `<sup>已</sup>` 剥完壳就变成一个和正文平起平坐的「已」字。
    let s = strip_sup_marks(s);
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

/// **铲掉现代括号**（v1.40.0）：`〔 〕`、`【 】`、`[ ]` 一律不许进正文流。
///
/// 现代整理本爱用方括号括注（「威烈王〔名午，考王之子〕」）。竖排里这四个
/// 括号会各占**一个完整的正文字格**——一排方括号横在版心，比错字还刺眼。
///
/// 处理规则（括号符号本身绝不留下）：
/// * `〔…〕` / `【…】`：内容 ≤ [`BRACKET_NOTE_MAX`] 字 → 转 `[注]…[/注]`
///   （落成双行小字夹注，正合古籍体例）；更长的一律剥壳、内容留作正文——
///   整段注疏塞进半格会把版面撑破。
/// * 半角 `[…]`：除自家标记（`[注]` / `[/注]` / `[高亮]` / `[拼音:…]`）
///   原样放行，其余一律剥壳留内容。
/// * 落单的半个括号（无配对的开 / 闭）直接删字符。
pub fn purge_bracket_marks(s: &str) -> String {
    const OPEN: [char; 3] = ['〔', '【', '['];
    const CLOSE: [char; 3] = ['〕', '】', ']'];
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        // 自家标记原样放行（[注] / [/注] / [高亮] / [拼音:…]）。
        if c == '[' && starts_own_marker(&cs, i) {
            let (body, next) = take_own_marker(&cs, i);
            out.push_str(&body);
            i = next;
            continue;
        }
        if let Some(k) = OPEN.iter().position(|&o| o == c) {
            // 找配对的闭括号（同一副）。
            if let Some(j) = (i + 1..cs.len()).find(|&j| cs[j] == CLOSE[k]) {
                let inner: String = cs[i + 1..j].iter().collect();
                let n = inner.chars().count();
                if n == 0 {
                    // 空壳：连括号一起删。
                } else if k < 2 && n <= BRACKET_NOTE_MAX {
                    out.push_str("[注]");
                    out.push_str(&inner);
                    out.push_str("[/注]");
                } else {
                    // 半角括号 / 超长括注：只剥壳，内容照进正文。
                    out.push_str(&inner);
                }
                i = j + 1;
                continue;
            }
            // 没有配对：这个开括号是杂质，删字符、正文照常。
            i += 1;
            continue;
        }
        if CLOSE.contains(&c) {
            // 落单的闭括号：删。
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 转成夹注的括注长度上限（超过就只剥壳，不当夹注排）。
const BRACKET_NOTE_MAX: usize = 80;

/// `i` 处是不是自家标记（保护它们不被当杂质剥掉）。
fn starts_own_marker(cs: &[char], i: usize) -> bool {
    let rest: String = cs[i..].iter().take(8).collect();
    rest.starts_with("[注]")
        || rest.starts_with("[/注]")
        || rest.starts_with("[高亮]")
        || rest.starts_with("[/高亮]")
        || rest.starts_with("[拼音:")
        || rest.starts_with("[数字:")
}

/// 取出 `i` 处那条自家标记，返回 (标记文本, 下一个下标)。
fn take_own_marker(cs: &[char], i: usize) -> (String, usize) {
    // 带参数的标记（`[拼音:xxx]` / `[数字:xxx]`）收在第一个 `]`；
    // 其余是定长标记（注意是**字符数**不是字节数）。
    let head4: String = cs[i..].iter().take(3).collect();
    if head4 == "[拼音" || head4 == "[数字" {
        for j in i..cs.len() {
            if cs[j] == ']' {
                return (cs[i..=j].iter().collect(), j + 1);
            }
        }
        return (cs[i..].iter().collect(), cs.len());
    }
    let head4: String = cs[i..].iter().take(4).collect();
    let len = if head4 == "[/注]" || head4 == "[高亮]" {
        4
    } else if cs[i..].iter().take(5).collect::<String>() == "[/高亮]" {
        5
    } else {
        3 // [注]
    };
    let end = (i + len).min(cs.len());
    (cs[i..end].iter().collect(), end)
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
    let s = s.replace("……", "︙").replace('…', "︙");
    // 破折号 → 竖排竖线（与 `vclean::classic_punct` 同一道，两条链路同源）。
    crate::vclean::dash_to_vertical(&s)
}

// ------------------------------------------------------------------
// 一、智能跳过前言：正文之前的「附件章」一律不进书架
// ------------------------------------------------------------------

/// 章题里出现这些词 → 它是出版社的附件，不是作者写的正文。
///
/// 现代 EPUB（尤其 PDF / 扫描转制本）在正文前塞了一堆东西：总目录、版权页、
/// 书名页书影、前言、导读、目录…… 竖排里它们会被当成「第一章」排出来，
/// 读者翻开《史记》先读到「图书在版编目（CIP）数据」——荒唐。
///
/// 匹配分两级（都只作用在**正文起手之前**的章，绝不误伤书中的《太史公自序》）：
/// * 整题等于该词（`前 言` 去了空格后相等）；
/// * 或**短题**（≤ [`FRONT_TITLE_MAX`] 字）里含有该词（`版权页` / `书名页书影`）。
const FRONT_MATTER_HINTS: &[&str] = &[
    // 序 / 前言 / 导读
    "前言", "序言", "自序", "原序", "总序", "译序", "译者序", "推荐序", "代序", "新序", "序",
    "导读", "导言", "引言", "弁言", "卷首语", "写在前面", "开篇寄语",
    // 编者 / 出版方的话
    "编者", "出版说明", "编例", "凡例", "例言", "体例", "使用说明", "排版说明",
    // 版权 / 元数据
    "版权", "图书在版编目", "CIP", "Cip", "cip", "版权页", "著作权", "ISBN", "isbn",
    // 目录
    "目录", "总目录", "目次", "Contents", "contents", "CONTENTS",
    // 图版 / 书影（扫描本的插图页，标题常是「未知」「书名页书影」）
    "书影", "插图", "图版", "扉页", "封面", "版权图",
    // 作者简介 / 致谢
    "作者简介", "作者小传", "编委", "鸣谢", "致谢", "内容简介", "内容提要", "提要", "摘要",
    // 整理者的导读类文字（v1.37.0 补）：《资治通鉴》卷首的「胡三省简介」
    // 「胡注史学意义」就靠这两枚认出来——它们不是史文，是今人加的话。
    "简介", "评介", "概说", "概略", "解题", "导论", "绪言", "意义",
    // 点校 / 整理说明
    "校点", "点校", "整理说明", "校勘", "校记",
];

/// 能被 [`FRONT_MATTER_HINTS`] 按「含有」判定的章题长度上限。
///
/// 长题几乎都是正文（`太史公自序第七十` 十一个字，含「自序」却是《史记》
/// 最后一章的正经内容）——只让短题走「含有」这条宽松路。
const FRONT_TITLE_MAX: usize = 10;

/// 「薄章」的内容字数上限：书影 / 插图页常只有一句图注，正文不会这么短。
const THIN_CHAPTER_CHARS: usize = 120;
/// 「薄章」的段落数上限。
const THIN_CHAPTER_PARAS: usize = 2;

/// 从正文起手处往前，最多允许砍掉多少章（前言不会比这更长；超了说明判据失灵）。
const FRONT_CUT_MAX: usize = 40;

/// **章题是不是「正文起手」**：`卷一 五帝本纪第一` / `第一百二十回` /
/// `五帝本纪第一` / `卷十三　三代世表第一` 都算。
///
/// 判据宁窄勿宽——误判成正文起手会让前言漏进来，比多丢一章严重得多。
/// 故只认「卷 / 第 + 数词」开头，以及史书专属的 本纪 / 世家 / 列传。
pub fn is_body_start_title(title: &str) -> bool {
    let t = title.trim();
    if t.is_empty() || is_front_matter_title(t) {
        return false;
    }
    let cs: Vec<char> = t.chars().collect();
    // 「卷 + 数词」：`卷一`、`卷一百三十`、`卷十三　三代世表第一`；
    // 也认 `卷第一`——《资治通鉴》通篇这么写（v1.37.0 补的这一「第」，
    // 少了它，通鉴全书找不到一个正文起手，前言一章都砍不动）。
    if cs[0] == '卷' {
        let rest = if cs.get(1) == Some(&'第') { &cs[2..] } else { &cs[1..] };
        if starts_with_numeral(rest) {
            return true;
        }
    }
    // 「第 + 数词 + 量词」：`第一回`、`卷第一百二十回` 之外的 `第一百二十回`。
    if cs[0] == '第' {
        let mut i = 1usize;
        while i < cs.len() && (is_numeral_char(cs[i]) || cs[i] == '零') {
            i += 1;
        }
        if i > 1 && i < cs.len() {
            return matches!(
                cs[i],
                '卷' | '回' | '章' | '节' | '篇' | '折' | '出' | '部' | '集' | '册' | '話' | '话'
            );
        }
    }
    // 史书专属：`五帝本纪第一`、`陈涉世家`…（前面没有卷次时也认）。
    t.contains("本纪") || t.contains("世家") || t.contains("列传")
}

/// `cs` 的开头是不是一串数词（汉字 / 阿拉伯数字 / 〇）。
fn starts_with_numeral(cs: &[char]) -> bool {
    let mut i = 0usize;
    while i < cs.len() && (is_numeral_char(cs[i]) || cs[i] == '之') {
        i += 1;
    }
    i > 0
}

/// 汉字数词 / 阿拉伯数字（供 [`is_body_start_title`] 数「卷几」）。
fn is_numeral_char(c: char) -> bool {
    matches!(
        c,
        '〇' | '零'
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
            | '两'
            | '壹'
            | '贰'
            | '叁'
            | '肆'
            | '伍'
            | '陆'
            | '柒'
            | '捌'
            | '玖'
            | '拾'
    ) || c.is_ascii_digit()
}

/// **章题是不是前言 / 版权 / 目录这类附件**（见 [`FRONT_MATTER_HINTS`]）。
pub fn is_front_matter_title(title: &str) -> bool {
    let t = title.trim();
    if t.is_empty() {
        return false;
    }
    // 去空白后再比对（`前　言` 与 `前言` 同罪）。
    let tight: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if tight.is_empty() {
        return false;
    }
    let n = tight.chars().count();
    FRONT_MATTER_HINTS
        .iter()
        .any(|h| tight == *h || (n <= FRONT_TITLE_MAX && tight.contains(h)))
}

/// 这一章是不是「薄得不像正文」：图注页 / 空章 / 转场页。
///
/// 三条同时成立才算：段数极少、字数极少、**且没有句读**。
/// 第三条是关键——`《陈太史评阅史记》　明崇祯元年刻本　书名页书影` 这种图注
/// 一行到底没有句号，而 `读史须知：先读本纪。` 虽短却有句读，是真正文。
pub fn is_thin_chapter(title: &str, body: &str) -> bool {
    let _ = title; // 判据只看正文（题名另有 [`is_front_matter_title`] 把关）。
    let body = body.trim();
    if body.is_empty() {
        return true;
    }
    let paras = body.lines().filter(|l| !l.trim().is_empty()).count();
    if paras > THIN_CHAPTER_PARAS || body.chars().count() > THIN_CHAPTER_CHARS {
        return false;
    }
    !body.chars().any(|ch| matches!(ch, '。' | '！' | '？' | '；'))
}

/// 夹在**正文中间**的散页：书商常把「目录」「书影」插在卷与卷之间。
///
/// `front_matter_cut` 只砍卷首那一串前缀——它必须先认出「正文起手」才动刀，
/// 所以当第一章本来就是正文时它返回 0，夹在后面的目录页就漏网了（实测
/// 《史记》第 14 章「目录」：11 段 119 字、通篇无句读，就是一页卷名清单）。
///
/// 与 [`is_thin_chapter`] 的差别：字数放宽到 [`STRAY_CHAPTER_CHARS`]
/// （目录页要列十几行卷名），但**句读这条一点不放宽**——目录页通篇没有
/// 句号，而真正的短章再短也有一两个句号。段数也不再设限（目录页段数多）。
pub fn is_stray_front_chapter(title: &str, body: &str) -> bool {
    if !is_front_matter_title(title) {
        return false;
    }
    let body = body.trim();
    if body.is_empty() {
        return true;
    }
    if body.chars().count() > STRAY_CHAPTER_CHARS {
        return false;
    }
    !body.chars().any(|ch| matches!(ch, '。' | '！' | '？' | '；'))
}

/// 散页判据的字数上限：比 `THIN_CHAPTER_CHARS` 宽——目录页要列十几行卷名。
const STRAY_CHAPTER_CHARS: usize = 600;

/// 全书扫一遍，返回**应当删掉的**那些夹带散页的章号（升序）。
///
/// 只认「题名像附件 + 正文不像正文」这两条同时成立，正文章绝不会被误伤：
/// 真章的题名是「卷X / 第X回 / 某某列传」，压根进不了附件词表。
pub fn stray_front_indices(pairs: &[(&str, &str)]) -> Vec<usize> {
    pairs
        .iter()
        .enumerate()
        .filter(|(_, (t, b))| is_stray_front_chapter(t, b))
        .map(|(i, _)| i)
        .collect()
}

/// 正文开头出现这些词 → 这一章是版权页 / 出版信息页。
///
/// 版权页的题名常是 `未知`（扫描本没写 `<title>`），只能从内容认：
/// 「图书在版编目（CIP）数据」「责任编辑」「开本 / 印张 / 定价」这些词
/// 绝不会出现在真正文里。只看开头 [`FRONT_BODY_SCAN`] 字——元数据总在最前。
const FRONT_BODY_HINTS: &[&str] = &[
    "图书在版编目",
    "CIP",
    "ISBN",
    "版权",
    "著作权",
    "责任编辑",
    "策划编辑",
    "封面设计",
    "出版发行",
    "出版社",
    "印刷",
    "开本",
    "印张",
    "字数",
    "定价",
    "书号",
    "版次",
    "翻印必究",
];
/// [`FRONT_BODY_HINTS`] 只扫正文开头这么多字。
const FRONT_BODY_SCAN: usize = 240;

/// 正文开头是不是出版社的元数据（见 [`FRONT_BODY_HINTS`]）。
pub fn is_front_matter_body(body: &str) -> bool {
    let head: String = body.chars().take(FRONT_BODY_SCAN).collect();
    FRONT_BODY_HINTS.iter().any(|h| head.contains(h))
}

/// 这一章能不能安全地丢掉（前言 / 目录 / 版权 / 图注薄章）。
fn is_droppable_leading(pair: &(&str, &str)) -> bool {
    let (title, body) = *pair;
    is_front_matter_title(title) || is_thin_chapter(title, body) || is_front_matter_body(body)
}

/// **该砍掉开头多少章**（0 = 一章都不砍）。
///
/// 入参是 `(章题, 正文)` 对——与 `RawChapter` / `types::Chapter` 都解耦，
/// 故管线里（逐章）与导入时（整本书）能共用同一把刀，判据不会分叉。
pub fn front_matter_cut(pairs: &[(&str, &str)]) -> usize {
    if pairs.len() <= 1 {
        return 0;
    }
    // 主判据：正文起手之前全砍。
    if let Some(idx) = pairs.iter().position(|(t, _)| is_body_start_title(t)) {
        if idx > 0 && idx <= FRONT_CUT_MAX && pairs[..idx].iter().all(is_droppable_leading) {
            return idx;
        }
        return 0;
    }
    // 兜底：砍掉开头连续的前言章（按题名判，不按薄判——见 `drop_front_matter`）。
    let lead = pairs
        .iter()
        .take_while(|(t, _)| is_front_matter_title(t))
        .count();
    if lead == 0 || lead == pairs.len() {
        return 0;
    }
    lead
}

/// **智能跳过前言**：从「总目录 / 版权页 / 书影 / 前言 / 目录」一路砍到第一个
/// 正文章（`卷一`、`本纪` …）为止。
///
/// 两道判据，都要成立才动刀（宁可漏删，绝不把正文当前言丢）：
///
/// 1. 找得到**正文起手**（[`is_body_start_title`]），且它在前 [`FRONT_CUT_MAX`] 章内；
/// 2. 起手之前的每一章都确实是附件（[`is_droppable_leading`]）。
///
/// 找不到正文起手时的兜底：把开头**连续**的附件章砍掉（同样要求砍完还剩正文）。
/// 砍空了就原样奉还——「解析不出章节」比「解析出空书」体面得多。
pub fn drop_front_matter(chapters: Vec<RawChapter>) -> Vec<RawChapter> {
    let pairs: Vec<(&str, &str)> = chapters
        .iter()
        .map(|c| (c.title.as_str(), c.content.as_str()))
        .collect();
    let cut = front_matter_cut(&pairs);
    if cut == 0 {
        return chapters;
    }
    chapters.into_iter().skip(cut).collect()
}

// ------------------------------------------------------------------
// 二、清剿上标幽灵字：<sup> 要么吃掉、要么转成注，绝不裸进正文
// ------------------------------------------------------------------

/// 上标里的内容够这么长，才算一条「真注文」；短于此的一律是杂质。
///
/// 现代 EPUB 的 `<sup>` 九成是纯注号（`1`、`①`、`[3]`），剩下一成是转制时
/// 漏出来的单字残渣（`<sup>已</sup>` 之类）——它们在竖排里是钉在字旁的一粒
/// 小红字，把行气戳出一个洞。真正有价值的注文不会只有一两个字。
const SUP_NOTE_MIN_CHARS: usize = 4;

/// 上标的处理结论。
enum SupVerdict {
    /// 吃掉（空 / 注号 / 单字残渣）。
    Drop,
    /// 转成夹注（够长，像一条真注文）。
    Note,
}

/// **吃掉 / 转注**：把所有 `<sup>` `<sub>` 就地裁决，正文里再没有上标。
///
/// * 空标签、纯注号（`1` `①` `[3]` `*`）→ 连内容一起删；
/// * 单字 / 双字残渣（`已`、`注1`）→ 删（这就是「红色幽灵字」的源头）；
/// * ≥ [`SUP_NOTE_MIN_CHARS`] 字 → 改写成 `<span class="note">`，由既有
///   标签行走器转成 `[注]…[/注]` 双行夹注（上标里真有注文时不能白扔）。
///
/// 嵌套 `<sup>` 按深度配对找闭合；找不到闭合就把开标签丢掉，绝不把
/// 后半本书吞进上标里。
pub fn strip_sup_marks(html: &str) -> String {
    let cs: Vec<char> = html.chars().collect();
    let n = cs.len();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < n {
        if cs[i] != '<' {
            out.push(cs[i]);
            i += 1;
            continue;
        }
        let Some(gt) = (i..n).find(|k| cs[*k] == '>') else {
            out.extend(&cs[i..]);
            break;
        };
        let raw: String = cs[i + 1..gt].iter().collect();
        let closing = raw.trim_start().starts_with('/');
        let name: String = raw
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        // 不是上标：原样放过（其余标签自有行走器处理）。
        if name != "sup" && name != "sub" {
            out.extend(&cs[i..gt + 1]);
            i = gt + 1;
            continue;
        }
        // 孤儿闭合标签：丢。
        if closing {
            i = gt + 1;
            continue;
        }
        let (from, to, next) = match matching_tag_close(&cs, gt + 1, &name) {
            Some(t) => t,
            // 没闭合：只丢这一枚开标签，后面的内容照常收（不能因坏标签吞掉半本书）。
            None => {
                i = gt + 1;
                continue;
            }
        };
        let inner: String = cs[from..to].iter().collect();
        match sup_verdict(&plain_text_of(&inner)) {
            SupVerdict::Drop => {}
            SupVerdict::Note => {
                let text = plain_text_of(&inner);
                out.push_str(&format!("<span class=\"note\">{text}</span>"));
            }
        }
        i = next;
    }
    out
}

/// 上标内容的裁决（见 [`strip_sup_marks`]）。
fn sup_verdict(text: &str) -> SupVerdict {
    if text.is_empty() || is_note_marker(text) {
        return SupVerdict::Drop;
    }
    if text.chars().count() >= SUP_NOTE_MIN_CHARS {
        return SupVerdict::Note;
    }
    SupVerdict::Drop
}

/// 是不是「纯注号」：只有数字 / 圈号 / 星号 / 括号，没有任何实义文字。
///
/// `1`、`12`、`①`、`[3]`、`*`、`†` 都是；`注1` 不是（带了个汉字）。
fn is_note_marker(text: &str) -> bool {
    let cs: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if cs.is_empty() {
        return false;
    }
    cs.iter().all(|c| {
        c.is_ascii_digit()
            || matches!(
                *c,
                '①' | '②' | '③' | '④' | '⑤' | '⑥' | '⑦' | '⑧' | '⑨' | '⑩' | '⑪' | '⑫' | '⑬'
                    | '⑭' | '⑮' | '⑯' | '⑰' | '⑱' | '⑲' | '⑳' | '⓪' | '*' | '†' | '‡'
                    | '§' | '※' | '[' | ']' | '(' | ')' | '〔' | '〕' | '【' | '】' | '·'
                    | '.' | '-'
            )
    })
}

/// 从 `pos` 起找 `name` 的配对闭合标签，返回 `(内容起, 内容止, 闭合后位置)`。
fn matching_tag_close(cs: &[char], pos: usize, name: &str) -> Option<(usize, usize, usize)> {
    let mut depth = 1i32;
    let mut i = pos;
    let from = pos;
    while i < cs.len() {
        if cs[i] != '<' {
            i += 1;
            continue;
        }
        let gt = (i..cs.len()).find(|k| cs[*k] == '>')?;
        let raw: String = cs[i + 1..gt].iter().collect();
        let closing = raw.trim_start().starts_with('/');
        let self_close = raw.trim_end().ends_with('/');
        let tn: String = raw
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let next = gt + 1;
        if tn != name {
            i = next;
            continue;
        }
        if closing {
            depth -= 1;
            if depth == 0 {
                return Some((from, i, next));
            }
        } else if !self_close {
            depth += 1;
        }
        i = next;
    }
    None
}

/// 上标内部的纯文本（去标签 + 还原实体 + 压空白）。
fn plain_text_of(inner: &str) -> String {
    let s = crate::pipeline::decode_entities(&crate::pipeline::strip_tags(inner));
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// **清剿单字杂质**：把 `[注]已[/注]` 这类独字夹注从正文流里摘掉。
///
/// 上标幽灵字的第二道闸——有些书的上标早在转制阶段就被压成了标记，进到
/// 这里时已经是 `[注]X[/注]`。真注文不会只有一个字（`锡：赐。` 四个字起），
/// 独字且不带任何标点的，一律是残渣。
pub fn scrub_ghost_notes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("[注]") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + "[注]".len()..];
        match tail.find("[/注]") {
            Some(i) => {
                let body = &tail[..i];
                let after = &tail[i + "[/注]".len()..];
                let t = body.trim();
                let ghost = !t.is_empty() && t.chars().count() <= 1;
                if ghost {
                    // 吃掉：连壳带字一起丢，不留空壳。
                } else {
                    out.push_str(&format!("[注]{body}[/注]"));
                }
                rest = after;
            }
            // 没有收口：剩下的原样吐出（交给后面的落单壳清理）。
            None => {
                out.push_str(&rest[at..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

// ------------------------------------------------------------------
// P0-1：诗词 / 硬回车「防碎裂」合并
// ------------------------------------------------------------------

/// 诗词与短句的「硬回车」防碎裂合并（P0-1，v1.52.0 新增）。
///
/// 真实出版社（中华书局等）的 EPUB 常用无数个 `<br>` / `<p>` 强行断行来排
/// 诗词，于是「床前明月光」「疑是地上霜」各占一段；更极端的是逐字断行，
/// 一个字一段——送进竖排引擎就成了「一列只有一个字」的灾难。
///
/// 算法：扫描章节正文段落，把**连续**的「可并段」合并成一整段。
/// 一段「可并」的判据（三条必须同时成立）：
///   1. 字数 `< MAX_MERGE_CHARS`（默认 10，用户给定「少于 10 个字」）；
///   2. **不以**终结标点收尾（。？！；… 及半角 !?; 都算收尾）；
///   3. **不以**冒号 `：`/`：` 收尾，也**不**以开引号 / 开括号起头
///      （「『（【《< 这些是「新一句 / 新一节」的起手，不能当成上一句的续行）。
///
/// 第 3 条专门防误并：像「子曰：」「（按）」「「子」曰」这类断行，若被并到
/// 上一句会破坏句读；诗词的逐句 / 逐字断行都没有这些符号，照并不误。
///
/// 合并后的长段交给引擎：竖排里它作为**一个逻辑段**按字格铺排，过长时自然
/// 折成下一列（与正常长段落同口径），不会再出现「一字一列」。
///
/// 注意：只动**正文段落**，章题在调用方单独处理，不在此合并。
pub fn merge_poetry_lines(lines: &mut Vec<String>) {
    const MAX_MERGE_CHARS: usize = 10;
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut buf = String::new();
    for line in lines.drain(..) {
        if is_poetry_mergeable(&line, MAX_MERGE_CHARS) {
            if buf.is_empty() {
                buf = line;
            } else {
                buf.push_str(&line);
            }
        } else {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
            out.push(line);
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    *lines = out;
}

/// 一段是否可并入相邻短段（见 [`merge_poetry_lines`] 的三条判据）。
fn is_poetry_mergeable(line: &str, max_chars: usize) -> bool {
    let n = line.chars().count();
    if n == 0 || n >= max_chars {
        return false;
    }
    // 末字符若是终结标点，说明这一句已收束，不可并入上一句。
    let ends_terminal = line
        .chars()
        .next_back()
        .map(is_terminal_punct)
        .unwrap_or(false);
    // 首字符若是开引号 / 开括号，视为新一句起手，不可并入上一句。
    let starts_open = line.chars().next().map(is_open_marker).unwrap_or(false);
    !ends_terminal && !starts_open
}

/// 终结标点：句读收尾（诗词整句应以它收束，故不算「断行待并」）。
fn is_terminal_punct(c: char) -> bool {
    matches!(
        c,
        '。' | '？' | '！' | '；' | '…' | '!' | '?' | ';' | '︰'
    )
}

/// 开引号 / 开括号：新一句 / 新一节的起手，不能当作上一句的续行并进去。
fn is_open_marker(c: char) -> bool {
    matches!(
        c,
        '「' | '『' | '（' | '【' | '《' | '<' | '“' | '‘' | '(' | '[' | '{'
    )
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
                        "p" | "ab" | "lg" | "seg" | "div" | "body"
                            if skip == 0 => {
                                mode = Mode::Body;
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
        // v1.40.0：现代括号绝不进正文流
        assert_eq!(
            purge_bracket_marks("威烈王〔名午，考王之子〕元年"),
            "威烈王[注]名午，考王之子[/注]元年"
        );
        assert_eq!(purge_bracket_marks("甲【乙丙】丁"), "甲[注]乙丙[/注]丁");
        // 半角方括号：只剥壳，内容照进正文
        assert_eq!(purge_bracket_marks("甲[乙丙]丁"), "甲乙丙丁");
        // 自家标记原样放行
        assert_eq!(
            purge_bracket_marks("前[注]成对[/注]后"),
            "前[注]成对[/注]后"
        );
        assert_eq!(purge_bracket_marks("长[拼音:cháng]江"), "长[拼音:cháng]江");
        // 空壳 / 落单的半个括号：直接删
        assert_eq!(purge_bracket_marks("甲〔〕乙〔丙"), "甲乙丙");
        assert_eq!(purge_bracket_marks("甲〕乙"), "甲乙");
        // 超长括注：只剥壳，不撑破版面
        let long: String = std::iter::repeat_n('注', 100).collect();
        assert!(purge_bracket_marks(&format!("甲〔{long}〕乙")).contains(&long));
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

    // ── 智能跳过前言 ────────────────────────────────────────────────

    /// 《史记》实况：总目录 / 版权页 / 书影 / 前言 / 目录 全部砍掉，从卷一开始。
    #[test]
    fn front_matter_is_skipped_before_juan_one() {
        let chapters = vec![
            RawChapter::new("总目录", "史记（精注全译）（第一册）"),
            RawChapter::new("版权页", "图书在版编目（CIP）数据 史记"),
            RawChapter::new("未知", "《陈太史评阅史记》　明崇祯元年刻本　书名页书影"),
            RawChapter::new("未知", "《陈太史评阅史记》　明崇祯元年刻本　序书影"),
            RawChapter::new("前言", "作为有着五千年悠久文明历史的国度"),
            RawChapter::new("目录", "卷一 五帝本纪第一 卷二 夏本纪第二"),
            RawChapter::new("卷一　五帝本纪第一", "黄帝者，少典之子，姓公孙，名曰轩辕。"),
            RawChapter::new("卷二　夏本纪第二", "夏禹，名曰文命。"),
        ];
        let out = drop_front_matter(chapters);
        assert_eq!(out.len(), 2, "前言未跳过：{out:?}");
        assert_eq!(out[0].title, "卷一　五帝本纪第一");
        assert!(!out.iter().any(|c| c.title.contains("前言")), "前言漏网：{out:?}");
        assert!(!out.iter().any(|c| c.content.contains("CIP")), "版权页漏网：{out:?}");
    }

    /// 书中的《太史公自序》是正经正文（最后一章），绝不能被当成前言丢掉。
    #[test]
    fn self_preface_inside_the_book_survives() {
        let chapters = vec![
            RawChapter::new("前言", "前言正文若干。"),
            RawChapter::new("卷一　五帝本纪第一", "黄帝者，少典之子。"),
            RawChapter::new("卷一百三十　太史公自序第七十", "太史公曰：先人有言。"),
        ];
        let out = drop_front_matter(chapters);
        assert_eq!(out.len(), 2, "不该动书里的自序：{out:?}");
        assert!(out.iter().any(|c| c.title.contains("太史公自序")));
    }

    /// 判据失灵时原样奉还：正文起手之前若夹着不像附件的章，一刀都不砍。
    #[test]
    fn ambiguous_leading_chapter_blocks_the_cut() {
        let chapters = vec![
            RawChapter::new("出版说明", "本书由某某整理。"),
            RawChapter::new("读史须知", "读史须知：先读本纪，再读世家。"),
            RawChapter::new("卷一　五帝本纪第一", "黄帝者。"),
        ];
        let out = drop_front_matter(chapters);
        assert_eq!(out.len(), 3, "不该砍：{out:?}");
    }

    /// 短章集（道德经八十一章）不会被「薄章」判据误杀。
    #[test]
    fn short_chapter_collections_survive() {
        let chapters: Vec<RawChapter> = (1..=8)
            .map(|i| RawChapter::new(format!("第{i}章"), "道可道，非常道。"))
            .collect();
        let out = drop_front_matter(chapters);
        assert_eq!(out.len(), 8, "短章被误删：{out:?}");
    }

    // ── 夹在正文中间的散页（《史记》第 14 章「目录」） ────────────────

    /// 卷与卷之间插的目录页：题名是「目录」、通篇无句读 → 抓出来。
    #[test]
    fn stray_toc_page_inside_the_body_is_caught() {
        let toc_body = "卷一 五帝本纪第一\n卷二 夏本纪第二\n卷三 殷本纪第三\n\
                        卷四 周本纪第四\n卷五 秦本纪第五";
        let pairs = [
            ("卷一 五帝本纪第一", "黄帝者，少典之子。"),
            ("目录", toc_body),
            ("卷二 夏本纪第二", "夏禹，名曰文命。"),
        ];
        assert_eq!(stray_front_indices(&pairs), vec![1]);
    }

    /// 同题名的**真正文**不会被误删：只要有一个句号就得留下。
    #[test]
    fn chapter_titled_toc_but_with_punct_is_kept() {
        let pairs = [
            ("卷一 五帝本纪第一", "黄帝者，少典之子。"),
            ("目录", "本篇目录如下，凡三卷。"),
            ("卷二 夏本纪第二", "夏禹，名曰文命。"),
        ];
        assert!(stray_front_indices(&pairs).is_empty());
    }

    /// 正文章题名（卷X / 列传）压根进不了附件词表，全书扫也不误伤。
    #[test]
    fn stray_scan_never_touches_real_chapters() {
        let pairs: Vec<(&str, &str)> = (1..=6)
            .map(|i| {
                (
                    match i {
                        1 => "卷一 五帝本纪第一",
                        2 => "卷二 夏本纪第二",
                        3 => "卷三 殷本纪第三",
                        4 => "卷四 周本纪第四",
                        5 => "卷五 秦本纪第五",
                        _ => "卷六 秦始皇本纪第六",
                    },
                    "黄帝者，少典之子。",
                )
            })
            .collect();
        assert!(stray_front_indices(&pairs).is_empty());
    }

    // ── 清剿上标幽灵字 ──────────────────────────────────────────────

    /// 单字上标（`司马迁<sup>已</sup>`）连字带壳吃掉，也不许变成夹注。
    #[test]
    fn sup_ghost_char_is_eaten() {
        let s = strip_sup_marks("<p>司马迁<sup>已</sup>曰：某事。</p>");
        assert!(!s.contains('已'), "幽灵字未清：{s}");
        assert!(!s.contains("note"), "不该转成注：{s}");
        assert!(s.contains("司马迁"), "正文被误伤：{s}");
    }

    /// 纯注号上标（`1` / `①` / `[3]`）同样吃掉。
    #[test]
    fn sup_note_markers_are_eaten() {
        for mark in ["1", "12", "①", "[3]", "*"] {
            let s = strip_sup_marks(&format!("<p>正文<sup>{mark}</sup>续。</p>"));
            assert!(!s.contains(mark), "注号未清：{s}");
        }
    }

    /// 够长的上标是真注文：转成 `[注]…[/注]` 夹注，不白扔。
    #[test]
    fn sup_real_note_becomes_gloss() {
        let s = strip_sup_marks("<p>某事<sup>司马贞索隐曰：此事可疑</sup>。</p>");
        assert!(s.contains(r#"<span class="note">司马贞索隐曰：此事可疑</span>"#), "未转注：{s}");
        assert!(!s.contains("<sup"), "残留上标：{s}");
    }

    /// 上标不闭合（坏标签）时只丢开标签，绝不吞掉后面的正文。
    #[test]
    fn unclosed_sup_does_not_swallow_the_page() {
        let s = strip_sup_marks("<p>前<sup>已</p><p>后段正文仍在。</p>");
        assert!(s.contains("后段正文仍在"), "后半页被吞：{s}");
    }

    /// 正文流里的独字夹注（`[注]已[/注]`）也要被刮掉。
    #[test]
    fn ghost_single_char_gloss_is_scrubbed() {
        let s = scrub_ghost_notes("司马迁[注]已[/注]曰：某事。");
        assert_eq!(s, "司马迁曰：某事。", "独字夹注未清：{s}");
        // 真注文（哪怕只有四个字、带标点）必须留下。
        let s = scrub_ghost_notes("锡[注]锡：赐。[/注]玄圭。");
        assert!(s.contains("[注]锡：赐。[/注]"), "真注文被误删：{s}");
    }

    /// 端到端：`<sup>` 幽灵字经全管线后既不裸奔也不成注。
    #[test]
    fn sup_ghost_vanishes_through_the_whole_cleanup() {
        let s = cleanup_ancient_text("司马迁<sup>已</sup>曰：迁生龙门。");
        assert!(!s.contains('已'), "幽灵字残留：{s}");
        assert!(!s.contains("[注]"), "不该成注：{s}");
        assert!(s.contains("司马迁"), "正文被误伤：{s}");
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

