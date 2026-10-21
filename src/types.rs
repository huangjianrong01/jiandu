//! 数据模型：书、章、段、注释、封面、目录、排版选项。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 排版方向：古文（竖笺）或现代（横笺）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Script {
    /// 古文：竖排，从右到左，每叶八行。
    Ancient,
    /// 现代：横排，仅横线（信笺式）。
    Modern,
}

impl Script {
    pub fn label(&self) -> &'static str {
        match self {
            Script::Ancient => "古文竖笺",
            Script::Modern => "现代横笺",
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Script::Ancient => "ancient",
            Script::Modern => "modern",
        }
    }
    pub fn from_str(s: &str) -> Self {
        if s == "modern" {
            Script::Modern
        } else {
            Script::Ancient
        }
    }
}

/// 单面 / 双面（对开）阅读。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Duplex {
    Single,
    Facing,
}

/// 纸面「点击翻页」判定（点左半屏上一页 / 右半屏下一页）。
/// `pressed` 为 capture 阶段记下的按下点，`up` 为抬起点，`half` 为屏幕半宽。
/// 返回 `Some(forward)` 表示应当翻页（方向），`None` 表示不翻。
///
/// 规则：本次交互只要碰过正文文字（拖选 / 点字）就一律不翻页——
/// 这是「选中文本时误翻下一页」的修复保证；只有按下-抬起位移很小
/// （单击）且全程在纸面时，才按落点左右半屏翻页。
pub fn tap_turns_page(
    touched_glyph: bool,
    dragging: bool,
    pressed: Option<(f32, f32)>,
    up: (f32, f32),
    half: f32,
) -> Option<bool> {
    if touched_glyph || dragging {
        return None;
    }
    let (px, py) = pressed?;
    if (px - up.0).abs() >= 8.0 || (py - up.1).abs() >= 8.0 {
        return None; // 位移过大：纸面拖动，不翻页
    }
    Some(up.0 >= half)
}

/// 方向键 → 翻页方向：`→ / ↓` 下一页（Some(true)），`← / ↑` 上一页（Some(false)），
/// 其它键不翻页（None）。macOS 下 `keystroke.key` 为全小写命名（left/right/…），
/// 这里同时兼容旧写法与带前缀写法，避免键名差异让快捷键静默失效。
pub fn arrow_key_turns(key: &str) -> Option<bool> {
    match key {
        "right" | "Right" | "ArrowRight" | "down" | "Down" | "ArrowDown" => Some(true),
        "left" | "Left" | "ArrowLeft" | "up" | "Up" | "ArrowUp" => Some(false),
        _ => None,
    }
}

/// 样式面板的标签页：样式（纸色 / 墨色 / 版式）或字体。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StyleTab {
    Style,
    Font,
}

impl StyleTab {
    pub fn label(&self) -> &'static str {
        match self {
            StyleTab::Style => "样式",
            StyleTab::Font => "字体",
        }
    }
}

/// 拖拽选区的字键：(chapter, para, offset)。字典序即阅读顺序。
pub type GlyphKey = (usize, usize, usize);

/// 归一化选区：无论鼠标从左到右还是从右到左拖，都返回 (起点, 终点)（按阅读顺序）。
pub fn normalize_selection(
    a: GlyphKey,
    b: GlyphKey,
) -> (GlyphKey, GlyphKey) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// 某个字是否落在选区 [a, b] 内（含端点）。
pub fn key_in_range(k: GlyphKey, a: GlyphKey, b: GlyphKey) -> bool {
    k >= a && k <= b
}

/// 字体选择（按模式区分）。
///
/// 后六款为典籍排版常用字体（图 2 的字体行）：它们都是开源字库，
/// 随包放在 `resources/fonts/` 或装进系统后即可选中；未安装时
/// `fonts::family_for` 会自动沿回退链落到最接近的系统字体，不会出现豆腐块。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FontChoice {
    /// 古文：宋体
    SongTi,
    /// 古文：思源宋体
    SourceHanSerif,
    /// 现代：思源黑体
    SourceHanSans,
    /// 现代：系统默认
    SystemDefault,
    /// 朱雀仿宋
    ZhuqueFangsong,
    /// 京华老宋
    JinghuaOldSong,
    /// 汇文明朝
    HuiwenMincho,
    /// 汇文正楷
    HuiwenZhengkai,
    /// 寒蝉活仿楷
    HanchanHuofang,
    /// 寒蝉锦书宋
    HanchanJinshu,
}

impl FontChoice {
    pub fn label(&self) -> &'static str {
        match self {
            FontChoice::SongTi => "宋体",
            FontChoice::SourceHanSerif => "思源宋体",
            FontChoice::SourceHanSans => "思源黑体",
            FontChoice::SystemDefault => "默认字体",
            FontChoice::ZhuqueFangsong => "朱雀仿宋",
            FontChoice::JinghuaOldSong => "京华老宋",
            FontChoice::HuiwenMincho => "汇文明朝",
            FontChoice::HuiwenZhengkai => "汇文正楷",
            FontChoice::HanchanHuofang => "寒蝉活仿楷",
            FontChoice::HanchanJinshu => "寒蝉锦书宋",
        }
    }

    /// 全部可选的典籍字体（字体页的竖排字牌按此顺序陈列）。
    ///
    /// 思源黑体与思源宋体同族，一并列入——批注、题签、现代夹注里都要用黑体，
    /// 只在「横笺」模式给就选不到。
    pub const CLASSIC: [FontChoice; 9] = [
        FontChoice::SongTi,
        FontChoice::SourceHanSerif,
        FontChoice::SourceHanSans,
        FontChoice::ZhuqueFangsong,
        FontChoice::JinghuaOldSong,
        FontChoice::HuiwenMincho,
        FontChoice::HuiwenZhengkai,
        FontChoice::HanchanHuofang,
        FontChoice::HanchanJinshu,
    ];

    /// 横笺用的无衬线组（思源黑体 / 系统默认）。
    pub const MODERN: [FontChoice; 2] = [FontChoice::SourceHanSans, FontChoice::SystemDefault];
}

/// 字级四档：小 / 中 / 大 / 特大。
///
/// 取代原先那条七档圆点滑块——档位太多，相邻两档肉眼几乎分不出来，
/// 反而让人反复试。四档拉开差距，一档一个手感，且档名就是档名，不必数点。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ZoomLevel {
    Small,
    Normal,
    Large,
    Huge,
}

impl Default for ZoomLevel {
    fn default() -> Self {
        ZoomLevel::Normal
    }
}

impl ZoomLevel {
    pub const ALL: [ZoomLevel; 4] = [
        ZoomLevel::Small,
        ZoomLevel::Normal,
        ZoomLevel::Large,
        ZoomLevel::Huge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ZoomLevel::Small => "小",
            ZoomLevel::Normal => "中",
            ZoomLevel::Large => "大",
            ZoomLevel::Huge => "特大",
        }
    }

    /// 该档的字号倍率（乘在网格引擎算出的基准字号上）。
    pub fn factor(self) -> f32 {
        match self {
            ZoomLevel::Small => 0.80,
            ZoomLevel::Normal => 1.00,
            ZoomLevel::Large => 1.30,
            ZoomLevel::Huge => 1.60,
        }
    }

    /// 由倍率反查档位：取最接近的一档（旧存档 / 连续缩放的残留值都收得进来）。
    pub fn from_zoom(z: f32) -> Self {
        Self::ALL
            .iter()
            .copied()
            .min_by_key(|l| ((l.factor() - z).abs() * 1000.0) as i32)
            .unwrap_or(ZoomLevel::Normal)
    }
}

/// 分享卡片的文字版式（对应分享面板里的版式图标）。
/// **只要竖排**：横排版式（含图标、按钮与全部代码）已按用户要求删除。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CardLayout {
    /// 竖排分列、从右到左（古籍题跋式）
    #[default]
    Columns,
    /// 单列竖排（长条书签式）
    Single,
    /// 笺：竖丝栏分成 8/10/12/14 列，列数按分享内容长短自适应
    /// （八行笺 / 十行笺……十三行式笺纸）
    Note,
}

impl CardLayout {
    pub fn label(&self) -> &'static str {
        match self {
            CardLayout::Columns => "竖排",
            CardLayout::Single => "单页",
            CardLayout::Note => "八行笺",
        }
    }
}

/// 丝栏（竖排列间那道界行）的颜色：样式面板三选一。
/// 影印本里三路都常见：乌丝栏（黑口本一路）、金丝栏（佳椠 / 写经）、
/// 朱丝栏（写经体一路）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RuleColor {
    /// 乌丝栏：墨兑水的冷灰界行。
    #[default]
    Wu,
    /// 金丝栏：泥金界行。
    Jin,
    /// 朱丝栏：朱红界行。
    Zhu,
}

impl RuleColor {
    pub const ALL: [RuleColor; 3] = [RuleColor::Wu, RuleColor::Jin, RuleColor::Zhu];

    pub fn label(&self) -> &'static str {
        match self {
            RuleColor::Wu => "乌丝栏",
            RuleColor::Jin => "金丝栏",
            RuleColor::Zhu => "朱丝栏",
        }
    }
}

/// 划线的「起笔」判定：本字已划线、而阅读顺序上的前一个字没划线时，
/// 它就是一段连续划线的起点——起点处画朱点（🔴），其后只画朱线。
/// 段落 / 章节切换（offset == 0）一律视为新起笔。
pub fn underline_run_start(
    key: GlyphKey,
    marked: &std::collections::HashSet<GlyphKey>,
) -> bool {
    if !marked.contains(&key) {
        return false;
    }
    if key.2 == 0 {
        return true;
    }
    !marked.contains(&(key.0, key.1, key.2 - 1))
}

/// 找到某个字所在的面（目录里的划线条目要据此跳页）。
pub fn page_of_key(pages: &[Vec<Glyph>], key: GlyphKey) -> Option<usize> {
    pages.iter().position(|pg| {
        pg.iter()
            .any(|g| (g.chapter, g.para, g.offset) == key)
    })
}

/// 注释 / 标记类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    /// 普通注（朱红编号，排双行小字夹注）。
    #[default]
    Note,
    /// 通假标（靛蓝小字，走旁注 / 脚注）。
    Tongjia,
    /// 人名标记：画专名线（实线）。
    Person,
    /// 地名标记：画专名线（实线）。
    Place,
    /// 书名标记：画书名线（波浪线）。
    Book,
}

/// 专名线 / 书名线的种类：人名、地名同画实线；书名画波浪线。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkKind {
    /// 专名线（人名 / 地名）：实线。
    Name,
    /// 书名线（书名）：波浪线。
    Book,
}

impl NoteKind {
    /// 是否划线类标记（人名 / 地名 / 书名）。
    pub fn is_mark(&self) -> bool {
        matches!(self, NoteKind::Person | NoteKind::Place | NoteKind::Book)
    }
}

/// 夹注来源：用于区分「古籍原本夹注」与「用户自加夹注」，并据此决定
/// 渲染配色（用户注深朱砂红）与是否允许右键修改 / 删除（原本注不可删）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteSource {
    /// 古籍原本夹注（浅墨 / 朱编号，用户不可删除）。
    #[default]
    Original,
    /// 用户自己添加的夹注（深朱砂红，可右键修改 / 删除）。
    User,
}

/// 一条注释 / 标记。`at` 是段内字符偏移（从 0 起）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Note {
    #[serde(default)]
    pub at: usize,
    #[serde(default)]
    pub kind: NoteKind,
    pub text: String,
    /// 覆盖跨度（个字数）：专名 / 书名词组占多个字，线要逐字连贯；
    /// 普通夹注恒为 1。缺省按 1 兼容旧书稿。
    #[serde(default = "one_len")]
    pub len: usize,
    /// 夹注来源：原本注 / 用户注。缺省为原本注，兼容旧书稿（无此字段）。
    #[serde(default)]
    pub source: NoteSource,
}

fn one_len() -> usize {
    1
}

impl Note {
    /// 普通注释（单字夹注）。
    pub fn new(at: usize, kind: NoteKind, text: impl Into<String>) -> Self {
        Self {
            at,
            kind,
            text: text.into(),
            len: 1,
            source: NoteSource::Original,
        }
    }

    /// 用户自加夹注（单字夹注，标记为可改删的深朱砂红来源）。
    pub fn user(at: usize, kind: NoteKind, text: impl Into<String>) -> Self {
        Self {
            at,
            kind,
            text: text.into(),
            len: 1,
            source: NoteSource::User,
        }
    }

    /// 划线类标记：从 `at` 起覆盖 `len` 个字。
    pub fn span(at: usize, len: usize, kind: NoteKind, text: impl Into<String>) -> Self {
        Self {
            at,
            kind,
            text: text.into(),
            len: len.max(1),
            source: NoteSource::Original,
        }
    }
}

/// 一个自然段：正文 + 该段上的注释。
#[derive(Clone, Debug, Serialize)]
pub struct Paragraph {
    pub text: String,
    #[serde(default)]
    pub notes: Vec<Note>,
}

impl Paragraph {
    pub fn plain(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            notes: Vec::new(),
        }
    }
    pub fn with_notes(s: impl Into<String>, notes: Vec<Note>) -> Self {
        Self {
            text: s.into(),
            notes,
        }
    }
    /// 取该段指定偏移上的普通注释。
    pub fn note_at(&self, offset: usize) -> Option<&Note> {
        self.notes
            .iter()
            .find(|n| n.at == offset && n.kind == NoteKind::Note)
    }
    /// 取该段指定偏移上的通假注释。
    pub fn tongjia_at(&self, offset: usize) -> Option<&Note> {
        self.notes
            .iter()
            .find(|n| n.at == offset && n.kind == NoteKind::Tongjia)
    }
    /// 某偏移是否落在划线类标记（人名 / 地名 / 书名）的跨度内。
    pub fn mark_at(&self, offset: usize) -> Option<MarkKind> {
        self.notes.iter().find_map(|n| {
            if !n.kind.is_mark() || n.len == 0 || offset < n.at || offset >= n.at + n.len {
                return None;
            }
            match n.kind {
                NoteKind::Person | NoteKind::Place => Some(MarkKind::Name),
                NoteKind::Book => Some(MarkKind::Book),
                _ => None,
            }
        })
    }
}

/// 段落反序列化：既接受纯字符串，也接受 `{text, notes}` 对象。
impl<'de> Deserialize<'de> for Paragraph {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Plain(String),
            Rich {
                text: String,
                #[serde(default)]
                notes: Vec<Note>,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Plain(t) => Paragraph::plain(t),
            Raw::Rich { text, notes } => Paragraph { text, notes },
        })
    }
}

/// 封面信息：书名、副题、印章文字、主色。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cover {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub seal: String,
    #[serde(default)]
    pub accent: u32,
}

impl Default for Cover {
    fn default() -> Self {
        Self {
            title: String::new(),
            subtitle: String::new(),
            seal: String::new(),
            accent: 0,
        }
    }
}

impl Cover {
    pub fn seal_or_default(&self) -> String {
        if self.seal.is_empty() {
            "笺读".to_string()
        } else {
            self.seal.clone()
        }
    }
}

/// 一章内容。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chapter {
    pub title: String,
    pub paragraphs: Vec<Paragraph>,
}

impl Chapter {
    pub fn char_count(&self) -> usize {
        self.paragraphs.iter().map(|p| p.text.chars().count()).sum()
    }
}

/// 目录条目：一条可跳转的标题。
///
/// `chapter` 指向 `Book::chapters` 的下标；`level` 从 1 起，用于缩进层级。
/// epub 的 nav / ncx 里常有一层嵌套（卷 → 章），层级信息就存在这里。
///
/// `start_char_index` 是该章首字在「版面流」里的全局字符下标：
/// 版面流（`Book::to_flow`）在每段之前固定插入 2 个段首缩进占位，
/// 所以这里累加的是「正文 + 2 × 段数」而非纯正文，才能与分页器的下标口径一致——
/// 目录点击跳转时，引擎据此直接定位到该章开头的第一叶（见 `Root::goto_char_index`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TocEntry {
    pub title: String,
    #[serde(default)]
    pub chapter: usize,
    #[serde(default = "default_level")]
    pub level: usize,
    /// 该章首字在版面流里的全局字符下标（None 表示尚未计算）。
    #[serde(default)]
    pub start_char_index: Option<usize>,
}

fn default_level() -> usize {
    1
}

impl TocEntry {
    pub fn new(title: impl Into<String>, chapter: usize, level: usize) -> Self {
        Self {
            title: title.into(),
            chapter,
            level: level.max(1),
            start_char_index: None,
        }
    }
}

/// 一本书。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Book {
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub cover: Cover,
    /// 本书专属通假表（简体 -> 通假/借用字），优先于内置表。
    #[serde(default)]
    pub tongjia: BTreeMap<String, String>,
    /// 目录。没有就按章节自动生成（见 `ensure_toc`）。
    #[serde(default)]
    pub toc: Vec<TocEntry>,
    pub chapters: Vec<Chapter>,
}

impl Book {
    pub fn new(title: impl Into<String>, author: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            author: author.into(),
            cover: Cover::default(),
            tongjia: BTreeMap::new(),
            toc: Vec::new(),
            chapters: Vec::new(),
        }
    }

    pub fn total_chars(&self) -> usize {
        self.chapters.iter().map(|c| c.char_count()).sum()
    }

    /// 没有目录时，按章节标题补一份平铺目录。
    pub fn ensure_toc(&mut self) {
        if self.toc.is_empty() {
            self.rebuild_toc_from_chapters();
        } else {
            // 目录里可能有越界条目（书被改写），保守修正。
            let n = self.chapters.len();
            self.toc.retain(|t| t.chapter < n);
            if self.toc.is_empty() {
                self.rebuild_toc_from_chapters();
            } else {
                // 条目可能带层级 / 自定义标题，不能整份重建；
                // 但若章节字数变了（书被改写）要重算每条的 start_char_index。
                let mut acc = 0usize;
                for e in &mut self.toc {
                    e.start_char_index = Some(acc);
                    if let Some(c) = self.chapters.get(e.chapter) {
                        acc += Self::flow_chars_of(c);
                    }
                }
            }
        }
    }

    /// 一章在版面流里占的字符数：正文 + 段首缩进占位（每段 2 个，与
    /// `to_flow` 的插入口径一致），用于把「章」换算成「全局字符下标」。
    fn flow_chars_of(ch: &Chapter) -> usize {
        ch.paragraphs
            .iter()
            .map(|p| p.text.chars().count() + 2)
            .sum()
    }

    /// 用章节标题重建平铺目录（同时算出每条的 start_char_index）。
    pub fn rebuild_toc_from_chapters(&mut self) {
        let mut acc = 0usize;
        self.toc = self
            .chapters
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let entry = TocEntry {
                    title: c.title.clone(),
                    chapter: i,
                    level: 1,
                    start_char_index: Some(acc),
                };
                acc += Self::flow_chars_of(c);
                entry
            })
            .collect();
    }

    /// 全文检索：找出 `q` 在**正文**里出现的所有位置。
    ///
    /// 返回 `(chapter, para, offset)`，按「章 → 段 → 字」的阅读顺序排列；
    /// `offset` 是**字符下标**，与 `Glyph::offset`、`Note::at` 同一口径，
    /// 因此命中可以直接喂给渲染层高亮、喂给分页跳面。
    ///
    /// 只搜正文、不搜注文 —— 注文是后加的注解，混进结果会让
    /// 「跳到那一页」失去意义。重叠出现（如「aa」在「aaa」里）各自计入。
    pub fn search(&self, q: &str) -> Vec<(usize, usize, usize)> {
        let pat: Vec<char> = q.chars().collect();
        if pat.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (ci, ch) in self.chapters.iter().enumerate() {
            for (pi, p) in ch.paragraphs.iter().enumerate() {
                let chars: Vec<char> = p.text.chars().collect();
                if chars.len() < pat.len() {
                    continue;
                }
                for i in 0..=(chars.len() - pat.len()) {
                    if &chars[i..i + pat.len()] == pat.as_slice() {
                        out.push((ci, pi, i));
                    }
                }
            }
        }
        out
    }

    /// 把全书拍平成字形流（供分页使用）。
    /// 每段之前固定插入两个段首缩进占位，实现「首行缩进两个字」——
    /// 横排 / 竖排一视同仁，不再提供开关。缩进占位只是一个
    /// 字格的空白，不参与查注释。段首不再放置任何段落标记符。
    pub fn to_flow(&self, show_punct: bool) -> Vec<Glyph> {
        let mut out = Vec::new();
        for (ci, ch) in self.chapters.iter().enumerate() {
            for (pi, p) in ch.paragraphs.iter().enumerate() {
                for _ in 0..2 {
                    out.push(Glyph {
                        chapter: ci,
                        para: pi,
                        offset: 0,
                        ch: ' ',
                        kind: GlyphKind::Indent,
                    });
                }
                for (oi, c) in p.text.chars().enumerate() {
                    // 标点显隐：关闭时不产出标点字（offset 仍按原文计数，注文定位不受影响）。
                    if !show_punct && is_punct(c) {
                        continue;
                    }
                    out.push(Glyph {
                        chapter: ci,
                        para: pi,
                        offset: oi,
                        ch: c,
                        kind: GlyphKind::Char,
                    });
                }
            }
        }
        out
    }
}

/// 版面流里一个单元的种类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GlyphKind {
    /// 普通正文。
    Char,
    /// 段首缩进占位（空一格，渲染为空白字格）。
    Indent,
}

/// 版面流里的一个字（带归属信息，便于查注释）。
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub chapter: usize,
    /// 段落序号。
    pub para: usize,
    /// 段内字符偏移。
    pub offset: usize,
    pub ch: char,
    /// 单元种类，用来区分正文 / 段首缩进占位。
    pub kind: GlyphKind,
}

impl Glyph {
    pub fn is_indent(&self) -> bool {
        self.kind == GlyphKind::Indent
    }
}

/// 卷首题署里的「卷次」：章有标题就用标题（它即是卷 / 章之名），
/// 否则退回「卷一 / 卷二……」（中文数字，至多九十九卷）。
pub fn chapter_volume(ci: usize, title: &str) -> String {
    let t = title.trim();
    if !t.is_empty() {
        t.to_string()
    } else {
        format!("卷{}", han_small(ci + 1))
    }
}

/// 书耳题签取提要时的字数上限：耳片只有一两字高，提要只取开头几字，
/// 字号收缩与截断由渲染层（`shelf::fit_single_column`）负责。
const EAR_DIGEST_CHARS: usize = 8;

/// 书耳题签的内容：**按内容自适应**——篇章名 → 卷名 → 提要。
///
/// * 篇章名（章标题）最具体，优先；
/// * 无篇名的多章书用卷次（卷一 / 卷二……）标识读到第几卷；
/// * 无篇名的单章书编「卷一」没有意义，摘正文开头作提要兜底；
/// * 章既无题也无文时仍退回卷次，书耳绝不空白。
pub fn ear_label(title: &str, chapters: usize, ci: usize, opening: &str) -> String {
    let t: String = title
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(bracket_to_quote)
        .collect();
    if !t.is_empty() {
        return t;
    }
    if chapters > 1 {
        return chapter_volume(ci, "");
    }
    let digest: String = opening
        .chars()
        .filter(|c| !c.is_whitespace() && !is_punct(*c))
        .map(bracket_to_quote)
        .take(EAR_DIGEST_CHARS)
        .collect();
    if !digest.is_empty() {
        return digest;
    }
    chapter_volume(ci, "")
}

/// 方头括号【】在竖排古籍里一律显作「」：显示层 1:1 替换（源数据不动，
/// 导出仍是原样）。正文（`shown`）、夹注、章题、中缝篇题、题署、分享题识、
/// 书脊签、封面、书耳题签都经这一道——书本里出现的【】不露出一个。
pub fn bracket_to_quote(c: char) -> char {
    match c {
        '【' => '「',
        '】' => '」',
        other => other,
    }
}

/// 1..=99 的中文数字（卷次 / 叶码备用），超出则回退阿拉伯数字。
pub fn han_small(n: usize) -> String {    const DIGITS: [&str; 10] = ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    if n == 0 {
        return "零".to_string();
    }
    if n < 10 {
        return DIGITS[n].to_string();
    }
    if n < 100 {
        let tens = n / 10;
        let ones = n % 10;
        let mut s = String::new();
        if tens == 1 {
            s.push('十');
        } else {
            s.push_str(DIGITS[tens]);
            s.push('十');
        }
        if ones != 0 {
            s.push_str(DIGITS[ones]);
        }
        return s;
    }
    n.to_string()
}

/// 句读记号：古籍里标点排得极小，且多用朱色。
/// 「标点显隐」关闭时，这些字会被 `Book::to_flow` 过滤掉（连同分页一起）。
pub fn is_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '「'
            | '」'
            | '『'
            | '』'
            | '（'
            | '）'
            | '《'
            | '》'
            | '·'
            | '…'
            | '—'
            | '“'
            | '”'
            | '‘'
            | '’'
    )
}

/// 点字弹出的工具条所锚定的字：记录该字的归属键、字符与点击处的窗口坐标。
/// 坐标来自 `MouseDownEvent.position`，用于在点击位置附近浮起工具条。
#[derive(Clone, Copy)]
pub struct GlyphHit {
    /// 字的归属键 (chapter, para, offset)。
    pub key: (usize, usize, usize),
    /// 被点击的字符。
    pub ch: char,
    /// 点击处窗口 x 坐标（像素）。
    pub x: f32,
    /// 点击处窗口 y 坐标（像素）。
    pub y: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Chapter, NoteKind, Paragraph};

    fn sample() -> Book {
        let mut b = Book::new("测试", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![Paragraph::with_notes(
                "道可道，非常道。",
                vec![Note::new(0, NoteKind::Note, "注")],
            )],
        });
        b
    }

    #[test]
    fn tap_turns_page_rules() {
        let half = 600.0;
        // 纸面单击：右半屏下一页、左半屏上一页。
        assert_eq!(
            tap_turns_page(false, false, Some((900.0, 300.0)), (901.0, 301.0), half),
            Some(true)
        );
        assert_eq!(
            tap_turns_page(false, false, Some((100.0, 300.0)), (101.0, 299.0), half),
            Some(false)
        );
        // 碰过正文文字（点字 / 拖选）：一律不翻页——「选中文本误翻页」的修复保证。
        assert_eq!(
            tap_turns_page(true, false, Some((900.0, 300.0)), (900.0, 300.0), half),
            None
        );
        assert_eq!(
            tap_turns_page(false, true, Some((900.0, 300.0)), (900.0, 300.0), half),
            None
        );
        // 纸面拖动（位移过大）：不翻页。
        assert_eq!(
            tap_turns_page(false, false, Some((900.0, 300.0)), (200.0, 500.0), half),
            None
        );
        // 没有按下记录（按下点在工具栏 / 浮层上）：不翻页。
        assert_eq!(tap_turns_page(false, false, None, (900.0, 300.0), half), None);
    }

    #[test]
    fn arrow_key_turns_page() {
        // → / ↓ 下一页，← / ↑ 上一页；单面 / 双面 / 专注模式下同一套判定。
        assert_eq!(arrow_key_turns("right"), Some(true));
        assert_eq!(arrow_key_turns("ArrowRight"), Some(true));
        assert_eq!(arrow_key_turns("down"), Some(true));
        assert_eq!(arrow_key_turns("left"), Some(false));
        assert_eq!(arrow_key_turns("ArrowLeft"), Some(false));
        assert_eq!(arrow_key_turns("up"), Some(false));
        // 字母 / 功能键不翻页。
        assert_eq!(arrow_key_turns("a"), None);
        assert_eq!(arrow_key_turns("enter"), None);
        assert_eq!(arrow_key_turns("escape"), None);
    }

    #[test]
    fn to_flow_punct_toggle() {        let b = sample();
        let with = b.to_flow(true);
        let without = b.to_flow(false);
        // 关闭标点后字更少，且不含标点。
        assert!(with.len() > without.len());
        assert!(with.iter().any(|g| g.ch == '，'));
        assert!(!without.iter().any(|g| g.ch == '，'));
        // offset 仍按原文计数：注文所在 offset=0 的字仍在。
        assert!(without.iter().any(|g| g.offset == 0 && g.ch == '道'));
        // 缩进占位不受标点开关影响。
        assert_eq!(without.iter().filter(|g| g.is_indent()).count(), 2);
    }

    #[test]
    fn toc_start_char_index_matches_flow() {
        // 目录的 start_char_index 必须与排版引擎的版面流下标口径一致，
        // 否则「点章跳转」会落到错误的页。每章：正文 n 字 + 2 个段首缩进占位。
        let mut b = Book::new("卷目校验", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![Paragraph::plain("道可道")],
        });
        b.chapters.push(Chapter {
            title: "二".into(),
            paragraphs: vec![Paragraph::plain("非常道")],
        });
        b.ensure_toc();
        // 第一章首字在流 0；第二章首字在流 5（3 正文 + 2 缩进）。
        assert_eq!(b.toc[0].start_char_index, Some(0));
        assert_eq!(b.toc[1].start_char_index, Some(5));
        // 用 to_flow 反查：流下标 5 正指向第二章第 0 字。
        let flow = b.to_flow(true);
        let g = flow.get(5).expect("流下标 5 应命中第二章首字");
        assert_eq!(g.chapter, 1);
        assert_eq!(g.offset, 0);
    }

    #[test]
    fn is_punct_covers_cjk() {
        assert!(is_punct('，'));
        assert!(is_punct('。'));
        assert!(is_punct('？'));
        assert!(!is_punct('道'));
        assert!(!is_punct('一'));
    }

    #[test]
    fn chapter_volume_labels() {
        // 有章标题：直接用作卷次。
        assert_eq!(chapter_volume(0, "上經"), "上經");
        assert_eq!(chapter_volume(3, "  "), "卷四");
        // 无标题：退回中文数字卷次。
        assert_eq!(chapter_volume(0, ""), "卷一");
        assert_eq!(chapter_volume(9, ""), "卷十");
        assert_eq!(chapter_volume(10, ""), "卷十一");
        assert_eq!(chapter_volume(21, ""), "卷二十二");
    }

    #[test]
    fn ear_label_adapts_to_available_content() {
        // ① 有篇章名：用篇名（周边空白滤掉，长短交给渲染层收缩）。
        assert_eq!(ear_label("第一章", 9, 0, "天地玄黄"), "第一章");
        assert_eq!(ear_label("  周紀一 ", 9, 3, "…"), "周紀一");
        // 篇名里的【】同样显作「」（书本里出现的方括号不露出一个）。
        assert_eq!(ear_label("【史記】卷", 9, 0, "…"), "「史記」卷");
        // ② 无篇名的多章书：卷次（卷一 / 卷三 / 卷十二）。
        assert_eq!(ear_label("", 9, 0, "天地玄黄"), "卷一");
        assert_eq!(ear_label("", 9, 2, "天地玄黄"), "卷三");
        assert_eq!(ear_label("", 9, 11, "天地玄黄"), "卷十二");
        assert_eq!(ear_label("   ", 9, 0, "天地玄黄"), "卷一");
        // ③ 无篇名的单章书：摘正文开头作提要（滤标点，至多八字）。
        assert_eq!(ear_label("", 1, 0, "天地玄黄宇宙洪荒"), "天地玄黄宇宙洪荒");
        assert_eq!(ear_label("", 1, 0, "天地玄黄，宇宙洪荒日月盈昃"), "天地玄黄宇宙洪荒");
        // 提要开头带【】也换（如【題解】……）。
        assert_eq!(ear_label("", 1, 0, "【題解】天地玄黄"), "「題解」天地玄黄");
        // ④ 单章且无正文（或只有标点）：仍退回卷次，书耳不留空。
        assert_eq!(ear_label("", 1, 0, ""), "卷一");
        assert_eq!(ear_label("", 1, 0, "，。"), "卷一");
    }

    #[test]
    fn square_brackets_render_as_corner_quotes() {
        // 【】在竖排古籍里一律显作「」：显示层 1:1 替换，源数据不动。
        assert_eq!(bracket_to_quote('【'), '「');
        assert_eq!(bracket_to_quote('】'), '」');
        // 「」本身与正文原样不动。
        assert_eq!(bracket_to_quote('「'), '「');
        assert_eq!(bracket_to_quote('』'), '』');
        assert_eq!(bracket_to_quote('道'), '道');
    }

    #[test]
    fn selection_normalize_and_range() {
        // 正向拖拽（同段内）。
        assert_eq!(
            normalize_selection((0, 0, 2), (0, 0, 7)),
            ((0, 0, 2), (0, 0, 7))
        );
        // 反向拖拽（从右往左 / 从下往上）自动归一。
        assert_eq!(
            normalize_selection((0, 0, 7), (0, 0, 2)),
            ((0, 0, 2), (0, 0, 7))
        );
        // 跨段、跨章选区：按字典序（阅读顺序）归一。
        assert_eq!(
            normalize_selection((0, 2, 3), (1, 0, 1)),
            ((0, 2, 3), (1, 0, 1))
        );
        // 区间含端点。
        let (a, b) = normalize_selection((0, 1, 5), (0, 3, 2));
        assert!(key_in_range((0, 1, 5), a, b));
        assert!(key_in_range((0, 2, 0), a, b));
        assert!(key_in_range((0, 3, 2), a, b));
        assert!(!key_in_range((0, 1, 4), a, b));
        assert!(!key_in_range((0, 3, 3), a, b));
        assert!(!key_in_range((1, 0, 0), a, b));
    }

    /// 检索用样书：「长安」散在两章里，且注文中也埋了一次（不该被搜到）。
    fn search_book() -> Book {
        let mut b = Book::new("检索测试", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![
                // 正文：长安@0、长安@4、长安@8（第三段「。」前）
                Paragraph::with_notes(
                    "长安城中长安道，长安道上行路人。",
                    vec![Note::new(0, NoteKind::Note, "注文里也有长安")],
                ),
                Paragraph::plain("未见此词。"),
            ],
        });
        b.chapters.push(Chapter {
            title: "二".into(),
            paragraphs: vec![Paragraph::plain("长安不见使人愁。")],
        });
        b
    }

    #[test]
    fn search_finds_hits_in_reading_order() {
        let b = search_book();
        // 按「章 → 段 → 字」的阅读顺序返回：先第一章的 3 处，再第二章的 1 处。
        assert_eq!(
            b.search("长安"),
            vec![(0, 0, 0), (0, 0, 4), (0, 0, 8), (1, 0, 0)]
        );
    }

    #[test]
    fn search_only_scans_body_text_not_notes() {
        let b = search_book();
        // 注文里也有「长安」，但只搜正文 → 仍是 4 处（搜注文会是 5 处）。
        assert_eq!(b.search("长安").len(), 4);
        // 全书没有的词 → 空。
        assert!(b.search("洛阳").is_empty());
        // 空词不搜（否则会命中每一处，没有意义）。
        assert!(b.search("").is_empty());
    }

    #[test]
    fn search_offsets_are_char_indices_into_paragraph() {
        let b = search_book();
        for (c, p, o) in b.search("长安") {
            let text = &b.chapters[c].paragraphs[p].text;
            let got: String = text.chars().skip(o).take(2).collect();
            assert_eq!(got, "长安", "命中偏移 {o} 处取到的不是「长安」：{got}");
        }
    }

    #[test]
    fn search_counts_overlapping_occurrences() {
        let mut b = Book::new("重叠", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![Paragraph::plain("长长长")],
        });
        // 「长长」在「长长长」里重叠出现两次（0 与 1）。
        assert_eq!(b.search("长长"), vec![(0, 0, 0), (0, 0, 1)]);
        // 单字命中三处。
        assert_eq!(b.search("长"), vec![(0, 0, 0), (0, 0, 1), (0, 0, 2)]);
        // 全文比词短时不命中。
        assert!(b.search("长长长长").is_empty());
    }

    #[test]
    fn rare_pinyin_lookup() {
        assert_eq!(crate::dict::pinyin_of('徼'), Some("jiào"));
        assert_eq!(crate::dict::pinyin_of('昃'), Some("zè"));
        assert_eq!(crate::dict::pinyin_of('道'), None);
        assert!(crate::dict::rare_len() >= 40);
    }

    #[test]
    fn catalog_has_books() {
        let cat = crate::content::catalog();
        assert!(cat.len() >= 3);
        assert!(cat.iter().any(|c| c.key == "daodejing"));
        assert!(cat.iter().any(|c| c.key == "sanzi"));
        assert!(cat.iter().any(|c| c.key == "qianzi"));
        let sz = crate::content::catalog_book("sanzi").expect("三字经应在书库");
        assert_eq!(sz.title, "三字经");
        assert!(crate::content::catalog_book("nope").is_none());
    }

    #[test]
    fn underline_run_starts_only_at_run_head() {
        let mut m = std::collections::HashSet::new();
        for o in 3..7 {
            m.insert((0usize, 0usize, o));
        }
        // (0,0,3) 是起笔：画朱点；其后各字只画线。
        assert!(underline_run_start((0, 0, 3), &m));
        assert!(!underline_run_start((0, 0, 4), &m));
        assert!(!underline_run_start((0, 0, 6), &m));
        // 没划线的地方不是起笔。
        assert!(!underline_run_start((0, 0, 8), &m));
        // 段首（offset 0）自身划线时一定是起笔。
        let mut m2 = std::collections::HashSet::new();
        m2.insert((1usize, 2usize, 0usize));
        assert!(underline_run_start((1, 2, 0), &m2));
    }

    #[test]
    fn page_of_key_finds_owning_page() {
        let g = |c: usize, p: usize, o: usize| Glyph {
            chapter: c,
            para: p,
            offset: o,
            ch: '道',
            kind: GlyphKind::Char,
        };
        let pages = vec![vec![g(0, 0, 0), g(0, 0, 1)], vec![g(0, 1, 0)]];
        assert_eq!(page_of_key(&pages, (0, 0, 1)), Some(0));
        assert_eq!(page_of_key(&pages, (0, 1, 0)), Some(1));
        assert_eq!(page_of_key(&pages, (0, 9, 9)), None);
    }

    #[test]
    fn download_writes_book_to_library() {
        // JIANDU_HOME 是进程级环境变量，与 library 的测试共用同一把锁。
        let _g = crate::library::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 用临时目录作为 JIANDU_HOME，避免污染真实书架。
        let dir = std::env::temp_dir().join(format!("jiandu-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("JIANDU_HOME", &dir);
        let mut lib = crate::library::Library::load();
        let idx = lib.download("qianzi").expect("下载千字文应成功");
        assert_eq!(lib.books[idx].title, "千字文");
        // 重复下载复用原条目（不新增）。
        let idx2 = lib.download("qianzi").expect("重复下载应成功");
        assert_eq!(idx, idx2);
        assert_eq!(lib.books.len(), 1);
        // 书籍文件确实落盘。
        assert!(dir.join("books").join("qianzi.json").exists());
        std::env::remove_var("JIANDU_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
