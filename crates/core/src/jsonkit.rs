//! **智能 JSON 兼容层**（v1.9.77）：市面上的古籍整理 JSON，键名各家用各家的。
//!
//! 这一层只有一个职责：把「任意结构 + 任意键名」的 JSON 归一成笺读的**最小
//! 公共形状**——一串段落字符串，其中注疏已经就地包成 `[注]…[/注]`。之后断章、
//! 夹注吸附、排版全交给既有管线：`parse_json_book` 里的 `ingest_inline_notes`
//! 会把标签吸成真正的**双行小字夹注**（标签本身一个字都不进正文格）。
//!
//! # 四条规矩
//!
//! 1. **模糊认键**：正文认 `content` / `text` / `body` / `paragraphs` …，注疏认
//!    `note` / `annotation` / `注释` / `集解` …（大小写不敏感）。
//! 2. **递归到底**：`chapters[].sections[].paragraphs[].text` 这种深嵌套逐层往下
//!    走，叶子上的 `text` 与同级 `note` 一起收——绝不只是认第一层。
//! 3. **绝不报错**：结构认不出来就退回「把见过的字符串都捞一遍」，并置
//!    [`JsonReport::downgraded`]，上层据此提示「已降级为纯文本导入」。
//!    `null` 一律跳过（`Option` / `is_empty` 双保险），绝不 panic。
//! 4. **一律洗 HTML**：`<p>` / `<br>` / `&nbsp;` 在**进段落之前**就过
//!    [`crate::dclean::strip_html_residue`]，绝不让标签流到排版层。
//!
//! 递归不必自己设上限：serde_json 解析时默认就有 128 层深度限制，能走到这一层
//! 的树一定比调用栈浅。

use serde_json::Value;

/// 正文字段（大小写不敏感）。
///
/// **宁漏不滥**：这里不用 `ends_with("text")` 这类模糊后缀——`context` 会躺枪，
/// 而认错一个键的代价是把整份配置塞进正文。
const BODY_KEYS: &[&str] = &[
    "content",
    "text",
    "body",
    "paragraphs",
    "paragraph",
    "para",
    "passage",
    "sentences",
    "sentence",
    "lines",
    "line",
    "chapter_text",
    "chaptertext",
    "chapter_content",
    "chaptercontent",
    "fulltext",
    "full_text",
    "plaintext",
    "plain_text",
    "rawtext",
    "raw_text",
    "txt",
    "raw",
    // 中文 schema
    "原文",
    "正文",
    "内容",
    "章节内容",
    "段落",
    "文本",
    "本文",
];

/// 注疏字段（大小写不敏感）。
///
/// 只收**确实是注**的：`translation` / `译文` / `meaning` 是整段今译，压进双行
/// 小字就是灾难，它们仍留在元数据里（丢弃）。
///
/// `集解` / `索隐` / `正义` 三家注常被拆成三个平级字段，
/// 每条会带上自己的注家名（见 [`note_label`]）。
const NOTE_KEYS: &[&str] = &[
    "note",
    "notes",
    "annotation",
    "annotations",
    "comment",
    "comments",
    "commentary",
    "gloss",
    "glosses",
    "scholia",
    "footnote",
    "marginalia",
    // 中文 schema
    "注",
    "注疏",
    "注释",
    "笺注",
    "校注",
    "夹注",
    "疏",
    "集解",
    "索隐",
    "索隱",
    "正义",
    "正義",
    // 音释 / 注音：古籍 JSON 常把反切注音放在 `zhushu`（「注疏」拼音）字段，
    // 与 `paragraph`（被注单字）平级。旧版没认这个键，导致音释被整枚吞掉、
    // 只剩孤零零的单字段落，断章器再把它们错拆成垃圾章题。补上后音释会就地
    // 包成 `[注]…[/注]`，书籍主路 `ingest_inline_notes` 吸成双行小字夹注。
    "zhushu",
    "yinzhu",
    "音释",
    "注音",
];

/// 题名字段（先出题名再出正文，章名才不会掉到章尾）。
const TITLE_KEYS: &[&str] = &["title", "name", "标题", "题名", "篇", "章", "章题"];

/// 锚句字段：注文该挂在哪一句之后。
const ANCHOR_KEYS: &[&str] = &["anchor", "quote", "target", "text_before", "句"];

/// 一趟解析的体检报告。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct JsonReport {
    /// 结构认不出来、退化成「把字符串都捞一遍」。
    ///
    /// 上层拿它弹一句「此 JSON 格式过于复杂，已降级为纯文本导入」——
    /// **事成了，但用户该知道代价**。
    pub downgraded: bool,
    /// 转成 `[注]…[/注]` 的注疏条数（诊断用，不参与任何判据）。
    pub glosses: usize,
}

/// 一个 JSON 值 → 段落流（注疏已就地包成 `[注]…[/注]`）。
///
/// 这是 core 导入路径唯一要调的函数：章节的题名由调用方另取（`Chapter.title`），
/// 所以**本章自己的题名不重复进段落**。
pub fn paragraphs_of(ch: &Value, rep: &mut JsonReport) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut saw_body = false;
    collect(ch, &mut out, rep, false, &mut saw_body);
    if out.is_empty() {
        // ③ 走到了这一步 = 结构完全认不出来。**不报错**：把见过的字符串都捞一遍，
        //    强行当纯文本读；`downgraded` 让 UI 去解释代价。
        collect_lenient(ch, &mut out);
        if !out.is_empty() {
            rep.downgraded = true;
        }
    } else if !saw_body {
        // 一个字都不是从「正文字段」里取的——是顺着数组 / 对象一路捡字符串捡来的。
        // 同样算降级：用户那份 schema 我们其实没读懂。
        rep.downgraded = true;
    }
    out
}

/// 一整个 JSON 值 → 纯文本（段落间 `\n`），给「只要一段字」的调用方用
/// （原文对照的毛坯视图、多格式漏斗的兜底导入）。
pub fn flatten_text(v: &Value) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut rep = JsonReport::default();
    let mut saw_body = false;
    collect(v, &mut out, &mut rep, true, &mut saw_body);
    if out.is_empty() {
        collect_lenient(v, &mut out);
    }
    out.join("\n")
}

/// 这个章节值是不是**本工程自己的 `Paragraph` 数组**形状
/// （`{paragraphs:[{text, notes:[{at:…, text:…}]}]}`）。
///
/// 是的话调用方应当直接反序列化，**别走兼容层**——兼容层只能把注文压成段末的
/// `[注]`，而结构化 `Note` 带着精确的 `at` 锚点，压平就丢了位置。
pub fn is_structured_paragraphs(ch: &Value) -> bool {
    let Some(ps) = ch.get("paragraphs").and_then(|x| x.as_array()) else {
        return false;
    };
    if ps.is_empty() {
        return false;
    }
    ps.iter().all(|p| match p {
        Value::Object(m) => {
            if !matches!(m.get("text"), Some(Value::String(_))) {
                return false;
            }
            // 平级注疏字段（`note` / `annotation` / `注释`…）一出现，就说明这是
            // 「正文 + 注文」的外来格式，必须走兼容层（否则 `Paragraph`
            // 反序列化会**静默吞掉**这些注文）。
            let has_flat_note = NOTE_KEYS
                .iter()
                .filter(|k| **k != "notes")
                .any(|k| get_ci(m, k).is_some());
            if has_flat_note {
                return false;
            }
            match m.get("notes") {
                None => true,
                Some(Value::Array(ns)) => ns.iter().all(|n| {
                    matches!(n, Value::Object(nm) if nm.get("at").is_some() || nm.get("anchor").is_some())
                }),
                _ => false,
            }
        }
        Value::String(_) => false,
        _ => false,
    })
}

// ---------------------------------------------------------------------------
// 递归抽取
// ---------------------------------------------------------------------------

/// 逐层往下走：认得出正文键就收正文（顺带收同级的注疏），认不出就把数组 / 对象
/// 继续往下翻。`emit_title` 只管**当前这一层**——子层永远按「有题名就出」处理。
///
/// `saw_body` 记账：「这一段字到底是从正文字段里取的，还是顺手捡来的」。
/// 一次都没认到正文字段 = 用户那份 schema 我们没读懂 → 上层据此判「降级」。
fn collect(
    v: &Value,
    out: &mut Vec<String>,
    rep: &mut JsonReport,
    emit_title: bool,
    saw_body: &mut bool,
) {
    match v {
        // 走到这里的字符串只可能来自正文键（顶层裸串同理）；元数据键在上面已经拦掉。
        Value::String(s) => push_paragraphs(out, s),
        Value::Array(a) => {
            for x in a {
                collect(x, out, rep, true, saw_body);
            }
        }
        Value::Object(m) => {
            // 书眉级元数据行（「音釋」节题 / 译者·作者署名）绝不进正文流——
            // 否则断章器会把无标点短行误判成章题，炸出垃圾章、竖排错乱。
            // ⚠️ 绝不基于 `type` 字段一刀切：`type` 语义因书而异——
            //   《楞嚴經》的 `type=2` 是**正文经文**（占全书 96% 内容），
            //   《漢書注》的 `type=2` 是注释者小传，两者都带标点、都该保留；
            //   只有「音釋」节题与译者署名行是无标点的纯元数据。这里只认
            //   「内容形态」，对所有书通用、零误杀。
            let body = find_body(m);
            if let Some(Value::String(s)) = body {
                if is_metadata_line(s) {
                    return;
                }
            }
            if let Some(body) = body {
                *saw_body = true;
                if emit_title {
                    if let Some(Value::String(t)) = find_title(m) {
                        let t = t.trim();
                        if !t.is_empty() {
                            out.push(t.to_string());
                        }
                    }
                }
                // 本层正文从 `mark` 开始：注疏只能挂在**本层**正文的末段上，
                // 绝不能挂到上一章尾巴上（那是错章）。
                let mark = out.len();
                collect(body, out, rep, true, saw_body);
                attach_notes(out, mark, m, rep);
                return;
            }
            // 没认出正文键：只往下翻**数组 / 对象**，不认识的键名下的字符串一律
            // 丢弃（`pinyin` / `author` / `卷三` 正是从这里混进正文的）。
            for (k, val) in m {
                if is_meta_key(k) {
                    continue;
                }
                if matches!(val, Value::Array(_) | Value::Object(_)) {
                    collect(val, out, rep, true, saw_body);
                }
            }
        }
        // null / 数字 / 布尔：一律跳过（空值兜底，绝不 panic）。
        _ => {}
    }
}

/// 兜底遍历：认不出键名就把所有字符串捞一遍。宁可脏一点，也不能整本导入成空白。
fn collect_lenient(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => push_paragraphs(out, s),
        Value::Array(a) => a.iter().for_each(|x| collect_lenient(x, out)),
        Value::Object(m) => {
            for (k, val) in m {
                if is_meta_key(k) {
                    continue;
                }
                collect_lenient(val, out);
            }
        }
        _ => {}
    }
}

/// 收一段：先洗 HTML 残留（`<p>` / `<br>` → 换行，`&nbsp;` → 空格），
/// 再按换行拆成段落，压掉纯空白段。
///
/// **这是正文进段落流的唯一闸口**——凡是往 `out` 里放的正文都必须过它，
/// 否则 `<p>` 会一路流到排版层，在竖排里排出四个「正文字」。
fn push_paragraphs(out: &mut Vec<String>, s: &str) {
    let washed = crate::dclean::strip_html_residue(s);
    for line in washed.split('\n') {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        out.push(l.to_string());
    }
}

// ---------------------------------------------------------------------------
// 注疏
// ---------------------------------------------------------------------------

/// 一枚待挂的注文。
struct NotePiece {
    /// 注家名（`集解` / `索隐` / `正义`），来自字段名；普通 `note` 为 `None`。
    label: Option<&'static str>,
    text: String,
    /// 该挂在哪一句之后；找不到锚句就挂到本段末尾。
    anchor: Option<String>,
}

/// 把本层对象的注疏挂到 `out[mark..]` 上。
fn attach_notes(
    out: &mut Vec<String>,
    mark: usize,
    m: &serde_json::Map<String, Value>,
    rep: &mut JsonReport,
) {
    for piece in note_pieces_of(m) {
        let tag = wrap_note(piece.label, &piece.text);
        if tag.is_empty() {
            continue;
        }
        attach_tag(out, mark, &tag, piece.anchor.as_deref());
        rep.glosses += 1;
    }
}

/// 按 [`NOTE_KEYS`] 的固定顺序取注疏。
///
/// 为什么要按固定顺序：`serde_json::Map` 默认是 BTreeMap（按键名排序），
/// 直接遍历 `m` 会让「正义」排在「集解」前面——三家注的顺序是学问，不是字符序。
fn note_pieces_of(m: &serde_json::Map<String, Value>) -> Vec<NotePiece> {
    let mut pieces: Vec<NotePiece> = Vec::new();
    // 锚句常常和注文**平级**（`{"text":…,"note":…,"anchor":…}`），
    // 所以本级对象上的锚要往下传给那些「本身就是字符串」的注文。
    let inherited = anchor_of(m);
    for key in NOTE_KEYS {
        let Some(v) = get_ci(m, key) else { continue };
        collect_pieces(v, note_label(key), inherited.as_deref(), &mut pieces);
    }
    // 同一枚注文被 `note` 与 `notes` 各认一次时（值一模一样）只留一条。
    pieces.dedup_by(|a, b| a.text == b.text);
    pieces
}

/// 取本级对象的锚句（`anchor` / `quote` / `target` / `text_before`）。
fn anchor_of(m: &serde_json::Map<String, Value>) -> Option<String> {
    ANCHOR_KEYS
        .iter()
        .find_map(|k| get_ci(m, k).and_then(|x| x.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 把一个注疏值拆成若干枚注文（字符串 / 数组 / 对象都认）。
fn collect_pieces(
    v: &Value,
    label: Option<&'static str>,
    inherited: Option<&str>,
    out: &mut Vec<NotePiece>,
) {
    match v {
        Value::String(s) => out.push(NotePiece {
            label,
            text: s.clone(),
            anchor: inherited.map(|a| a.to_string()),
        }),
        Value::Array(a) => a
            .iter()
            .for_each(|x| collect_pieces(x, label, inherited, out)),
        Value::Object(m) => {
            let anchor = anchor_of(m).or_else(|| inherited.map(|s| s.to_string()));
            // `{"text": "…", "anchor": "…"}`：正文键优先，其次 `note`。
            if let Some(b) = find_body(m).or_else(|| get_ci(m, "note")) {
                match b {
                    Value::String(s) => out.push(NotePiece {
                        label,
                        text: s.clone(),
                        anchor,
                    }),
                    other => collect_pieces(other, label, anchor.as_deref(), out),
                }
            } else {
                for (_, val) in m {
                    if let Value::String(s) = val {
                        out.push(NotePiece {
                            label,
                            text: s.clone(),
                            anchor: anchor.clone(),
                        });
                    }
                }
            }
        }
        // null / 数字 / 布尔：跳过（空值兜底）。
        _ => {}
    }
}

/// 三家注的字段名 → 注家名。其余注疏字段没有「谁说的」，返回 `None`。
fn note_label(key: &str) -> Option<&'static str> {
    match key {
        "集解" => Some("集解"),
        "索隐" | "索隱" => Some("索隐"),
        "正义" | "正義" => Some("正义"),
        _ => None,
    }
}

/// 注文 → `[注]…[/注]`。
///
/// * 空注文返回空串（调用方据此跳过，绝不留一对空壳 `[注][/注]`）；
/// * 源里本身就带着 `[注]…[/注]` 的，剥壳重包，**绝不叠两层**；
/// * 三家注补上注家名前缀（「谁说的」是三家注一半的价值），已有则不重复。
fn wrap_note(label: Option<&str>, raw: &str) -> String {
    let mut body = normalize_note_text(raw);
    if body.is_empty() {
        return String::new();
    }
    if let Some(inner) = body
        .strip_prefix("[注]")
        .and_then(|r| r.strip_suffix("[/注]"))
    {
        body = inner.trim().to_string();
        if body.is_empty() {
            return String::new();
        }
    }
    if let Some(l) = label {
        if !body.starts_with(l) {
            body = format!("{l}：{body}");
        }
    }
    let mut tag = String::with_capacity(body.len() + 10);
    tag.push_str("[注]");
    tag.push_str(&body);
    tag.push_str("[/注]");
    tag
}

/// 注文正文归一：洗 HTML 残留 + 折掉硬换行。
///
/// 硬换行必须折：双行小字里出现 `\n` 会凭空断行（和 `mark_three_jia_notes`
/// 同一条规矩）。折的时候看两侧字符——中文直接相接，ASCII 词之间补一个空格，
/// 否则英文注疏会连成一个词。
fn normalize_note_text(raw: &str) -> String {
    let washed = crate::dclean::strip_html_residue(raw);
    let mut out = String::with_capacity(washed.len());
    let mut prev: Option<char> = None;
    for line in washed.split('\n') {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let (Some(p), Some(c)) = (prev, t.chars().next()) {
            if p.is_ascii_alphanumeric() && c.is_ascii_alphanumeric() {
                out.push(' ');
            }
        }
        out.push_str(t);
        prev = t.chars().last();
    }
    out.trim().to_string()
}

/// 把 `tag` 挂进 `out[mark..]`：优先挂到锚句之后，其次挂到本层末段末尾。
fn attach_tag(out: &mut Vec<String>, mark: usize, tag: &str, anchor: Option<&str>) {
    if let Some(a) = anchor {
        if let Some(pos) = out[mark..].iter().position(|s| s.contains(a)) {
            let idx = mark + pos;
            if let Some(at) = out[idx].find(a) {
                let cut = at + a.len();
                out[idx].insert_str(cut, tag);
                return;
            }
        }
    }
    if let Some(last) = out[mark..].last_mut() {
        last.push_str(tag);
    } else if let Some(prev) = out.last_mut() {
        // 本层没有正文（只有注）：挂到上一段末尾，总比丢了好。
        prev.push_str(tag);
    } else {
        // 全书第一段就是一枚孤注：只能自己成段（极小概率，但不许丢字）。
        out.push(tag.to_string());
    }
}

// ---------------------------------------------------------------------------
// 键名判定
// ---------------------------------------------------------------------------

fn find_body(m: &serde_json::Map<String, Value>) -> Option<&Value> {
    BODY_KEYS.iter().find_map(|k| get_ci(m, k))
}

fn find_title(m: &serde_json::Map<String, Value>) -> Option<&Value> {
    TITLE_KEYS.iter().find_map(|k| get_ci(m, k))
}

/// 大小写不敏感取键（先精确、再忽略大小写扫一遍）。
fn get_ci<'a>(m: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    if let Some(v) = m.get(key) {
        return Some(v);
    }
    m.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// 古籍 JSON 里两类**书眉级元数据行**绝不进正文流，否则断章器会把无标点短行
/// 误判成章题，炸出垃圾章、竖排错乱：
///   * 「音釋」「音释」节题（固定 2 字）；
///   * 译者 / 作者署名行（**无标点** + 短 + 以 譯 / 譯語 / 筆受 结尾）。
///
/// ⚠️ 不基于 `type` 字段一刀切：`type` 语义因书而异（经史子集丛各有各的约定——
///   《楞嚴經》的 `type=2` 是正文经文，《漢書注》的 `type=2` 是注释者小传，其余书
///   根本没有 `type=2`）。两类书里真正的正文 / 小传都**带标点**，只有纯元数据
///   是无标点的署名行——所以靠「无标点 + 译/筆受结尾」这一内容形态来甄别，
///   对所有经史子集丛通用、**零误杀**。
fn is_metadata_line(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    // 「音釋」「音释」节题。
    if t == "音釋" || t == "音释" {
        return true;
    }
    // 译者 / 作者署名行：无标点 + 短 + 以译家动词结尾。
    // 限定「无标点」是关键区分——佛经译者署名行（唐天竺沙門般剌密帝譯）无标点，
    // 而真正的经文（如是我聞：一時…）与史书注的注释者小传（刘德，北海人。）
    // 都带标点，绝不会被误剔。
    let has_punct = t.chars().any(|c| {
        "。，、：；！？…—（）〔〕【】「」『』《》〈〉．：；！？".contains(c)
    });
    if !has_punct && t.chars().count() <= 60 {
        if t.ends_with("譯") || t.ends_with("譯語") || t.ends_with("筆受") {
            return true;
        }
    }
    false
}

/// 元信息字段：**绝不进正文流**。
///
/// 古籍 JSON 的元数据多到离谱（`pinyin` / `book` / `volume` / `author` …），
/// 认漏一个就是「拼音字母竖着排」「佚名占一列」「卷三混在正文中间」。
///
/// ⚠️ `note` / `annotation` / `注释` **不在这里**：它们是注疏，由
/// [`note_pieces_of`] 收去包成 `[注]`（v1.9.77 之前被当元数据丢掉，注文整本消失）。
fn is_meta_key(k: &str) -> bool {
    matches!(
        k.to_ascii_lowercase().as_str(),
        "id" | "uuid" | "isbn" | "author" | "authors" | "meta" | "metadata" | "toc" | "cover"
            | "language" | "lang" | "date" | "publisher" | "version" | "created" | "updated"
            | "url" | "source" | "site" | "tags" | "icon" | "css" | "style" | "type" | "format"
            | "kind" | "book_id" | "chapter_id" | "section_id"
            // —— 古籍 JSON 的元数据四件套 ——
            | "pinyin" | "py" | "pron" | "pronunciation" | "phonetic" | "zhuyin" | "注音"
            | "拼音" | "romaji" | "tone" | "tones"
            | "book" | "books" | "书名" | "volume" | "vol" | "juan" | "卷" | "卷名"
            | "chapter_no" | "chapter_num" | "num" | "index" | "idx" | "order" | "序号"
            | "translator" | "dynasty" | "editor" | "annotator" | "series" | "category"
            | "keywords" | "译文" | "translation" | "meaning" | "explain" | "shiyi" | "english"
            | "en" | "simplified" | "traditional"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(j: &str) -> (Vec<String>, JsonReport) {
        let v: Value = serde_json::from_str(j).expect("测试 JSON 应合法");
        let mut rep = JsonReport::default();
        let p = paragraphs_of(&v, &mut rep);
        (p, rep)
    }

    /// **递归到底**：`chapters[].sections[].paragraphs[].text` 四层，叶子都要收到。
    #[test]
    fn deep_nesting_reaches_leaf_text() {
        let j = r#"{"book":"史記","chapters":[
            {"title":"本紀","sections":[
                {"paragraphs":[{"text":"黃帝者，少典之子。"},{"text":"生而神靈。"}]},
                {"paragraphs":[{"text":"弱而能言。"}]}
            ]}
        ]}"#;
        let (p, rep) = paras(j);
        assert_eq!(p, vec!["黃帝者，少典之子。", "生而神靈。", "弱而能言。"], "{p:?}");
        assert!(!rep.downgraded, "认得出结构，不该算降级");
    }

    /// 深嵌套里的 `note` 必须变成 `[注]…[/注]`，且挂在**同一段**末尾。
    #[test]
    fn deep_nesting_carries_notes_as_inline_gloss() {
        let j = r#"{"chapters":{"sections":[{"paragraphs":[
            {"text":"黃帝者，少典之子。","note":"索隱：少典，諸侯國號。"},
            {"text":"姓公孫，名曰軒轅。","annotation":null}
        ]}]}}"#;
        let (p, rep) = paras(j);
        assert_eq!(p[0], "黃帝者，少典之子。[注]索隱：少典，諸侯國號。[/注]", "{p:?}");
        // null 注疏跳过，不产出空壳。
        assert_eq!(p[1], "姓公孫，名曰軒轅。", "null 注疏不该留壳：{p:?}");
        assert_eq!(rep.glosses, 1);
    }

    /// 章级注疏（正文与注在同一个对象上）挂到该章末段。
    #[test]
    fn chapter_level_note_lands_on_last_paragraph() {
        let j = r#"{"title":"學而","content":"子曰：學而時習之。\n有朋自遠方來。","note":"疏：習，鷹乃學習。"}"#;
        let (p, rep) = paras(j);
        assert_eq!(p.len(), 2, "题名不该进段落：{p:?}");
        assert!(p[1].ends_with("[/注]"), "注应挂在末段：{p:?}");
        assert_eq!(rep.glosses, 1);
    }

    /// 三家注三平级字段：顺序按「集解→索隐→正义」，各带注家名前缀。
    #[test]
    fn three_jia_fields_keep_their_labels_and_order() {
        let j = r#"{"text":"黃帝者。","正义":"正義：甲。","集解":"集解：乙。","索隐":"丙。"}"#;
        let (p, _) = paras(j);
        assert_eq!(
            p[0], "黃帝者。[注]集解：乙。[/注][注]索隐：丙。[/注][注]正义：正義：甲。[/注]",
            "{p:?}"
        );
    }

    /// 带锚句的注文：插在锚句之后，而不是一律堆到段末。
    #[test]
    fn anchored_note_goes_after_the_quoted_sentence() {
        let j = r#"{"paragraphs":[
            {"text":"黃帝者，少典之子。姓公孫。","note":"案：此句疑衍。","anchor":"少典之子。"}
        ]}"#;
        let (p, _) = paras(j);
        assert_eq!(
            p[0], "黃帝者，少典之子。[注]案：此句疑衍。[/注]姓公孫。",
            "{p:?}"
        );
    }

    /// HTML 实体与标签绝不能流到排版层。
    #[test]
    fn html_tags_never_reach_the_paragraph() {
        let j = r#"{"content":"<p>道可道，<b>非常</b>道。</p><p>名可名。<br/>非常名。</p>","note":"注：<i>案</i>&nbsp;此句。"}"#;
        let (p, _) = paras(j);
        let joined = p.join("|");
        assert!(!joined.contains('<'), "标签漏进正文：{joined}");
        assert!(!joined.contains("&nbsp;"), "实体漏进正文：{joined}");
        assert!(joined.contains("道可道，非常道。"));
        assert!(joined.contains("名可名。"), "{joined}");
        assert!(joined.contains("非常名。"), "{joined}");
    }

    /// **静默降级**：认不出结构也绝不报错，把字符串捞出来当纯文本，并置标记。
    #[test]
    fn unknown_shape_degrades_to_plain_text_and_says_so() {
        let j = r#"{"data":{"weird_deep":{"unknown_key":["道可道，非常道。", null, 42]}}}"#;
        let (p, rep) = paras(j);
        assert!(rep.downgraded, "应标记为降级");
        assert_eq!(p, vec!["道可道，非常道。"], "空值 / 数字不该进段落：{p:?}");
    }

    /// 空对象 / 空数组 / `null` 根：不 panic、不出段落。
    #[test]
    fn empty_and_null_inputs_are_harmless() {
        for j in ["{}", "[]", "null", r#"{"a":null,"b":[],"note":null}"#] {
            let (p, rep) = paras(j);
            assert!(p.is_empty(), "{j} 不该出段落：{p:?}");
            assert!(!rep.downgraded);
        }
    }

    /// 已经带着 `[注]…[/注]` 的正文：不叠第二层壳。
    #[test]
    fn pre_tagged_note_is_not_double_wrapped() {
        let j = r#"{"text":"黃帝者。","note":"[注]案：舊注已標。[/注]"}"#;
        let (p, _) = paras(j);
        assert_eq!(p[0], "黃帝者。[注]案：舊注已標。[/注]", "{p:?}");
        assert_eq!(p[0].matches("[注]").count(), 1);
    }

    /// `flatten_text` 保留「题名先于正文」的顺序（章名不掉到章尾）。
    #[test]
    fn flatten_puts_titles_before_their_body() {
        let v: Value = serde_json::from_str(
            r#"{"chapters":[{"title":"學而","content":"子曰。"},{"title":"為政","content":"為政以德。"}]}"#,
        )
        .unwrap();
        assert_eq!(flatten_text(&v), "學而\n子曰。\n為政\n為政以德。");
    }

    /// 本工程自己的 `Paragraph` 形状必须被认出来（好让调用方保住锚点）。
    #[test]
    fn structured_paragraphs_are_recognised() {
        let v: Value = serde_json::from_str(
            r#"{"paragraphs":[{"text":"甲","notes":[{"at":0,"text":"注"}]}]}"#,
        )
        .unwrap();
        assert!(is_structured_paragraphs(&v));
        // 平级 `note` = 外来格式，绝不能当结构化段落吞掉注文。
        let flat: Value =
            serde_json::from_str(r#"{"paragraphs":[{"text":"甲","note":"注"}]}"#).unwrap();
        assert!(!is_structured_paragraphs(&flat));
    }

    /// 音释 / 注音字段（`zhushu` 等）必须就地包成 `[注]…[/注]`，绝不整枚吞掉。
    ///
    /// v1.9.89 之前 `zhushu` 既非正文字段也非注疏键，音释被静默丢弃——
    /// 《楞嚴經》音释字表整批消失、只剩孤零零的单字段落，断章器再把它们
    /// 错拆成垃圾章。补 `zhushu` 进 `NOTE_KEYS` 后，音释必须成为行内夹注。
    #[test]
    fn zhushu_becomes_inline_note() {
        let v: Value = serde_json::from_str(
            r#"{"chapters":[{"name":"楞嚴經卷第一","paragraphs":[
                {"paragraph":"挃","zhushu":"陟栗切，撞也。"},
                {"paragraph":"如是我聞。","zhushu":""}
            ]}]}"#,
        )
        .unwrap();
        let t = flatten_text(&v);
        assert!(t.contains("[注]陟栗切，撞也。[/注]"), "音释必须包成夹注：{t}");
        assert!(t.contains("挃"), "被注单字必须保留：{t}");
        // 空音释不得生成空壳 `[注][/注]`。
        assert_eq!(t.matches("[注][/注]").count(), 0, "空音释不得留空壳：{t}");
    }

    /// 书眉级元数据（「音釋」节题 / 无标点的译者署名行）绝不进正文流，但正文经文
    /// （哪怕 `type=2` 且很长）必须保留——否则《楞嚴經》96% 的经文会被误杀。
    #[test]
    fn metadata_lines_never_leak_but_body_is_kept() {
        let v: Value = serde_json::from_str(
            r#"{"chapters":[{"name":"楞嚴經卷第一","paragraphs":[
                {"paragraph":"楞嚴經卷第一","type":1},
                {"paragraph":"唐天竺沙門般剌密帝譯","type":2},
                {"paragraph":"音釋","type":2},
                {"paragraph":"如是我聞：一時，佛在室羅筏城祇桓精舍。","type":2},
                {"paragraph":"挃","zhushu":"陟栗切，撞也。"}
            ]}]}"#,
        )
        .unwrap();
        let t = flatten_text(&v);
        assert!(t.contains("楞嚴經卷第一"), "卷题文本(type=1)应保留：{t}");
        assert!(
            t.contains("如是我聞"),
            "正文经文(type=2 带标点)必须保留——否则误杀 96% 正文：{t}"
        );
        assert!(t.contains("挃"), "音释单字必须保留：{t}");
        assert!(
            !t.contains("般剌密帝"),
            "译者署名(无标点·譯结尾)不得进正文：{t}"
        );
        assert!(!t.contains("音釋"), "音释节题不得进正文：{t}");

        let mut rep = JsonReport::default();
        let ps = paragraphs_of(&v, &mut rep);
        let joined = ps.concat();
        assert!(joined.contains("如是我聞"), "正文经文必须进段落流：{joined}");
        assert!(
            !joined.contains("般剌密帝"),
            "译者署名不得进段落流：{joined}"
        );
    }
}
