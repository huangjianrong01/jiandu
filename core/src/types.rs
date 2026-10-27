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
    /// 与 serde 的反序列化同一口径（`Script` 只有两变体，不值得为它引入
    /// `FromStr` 的 `Err` 类型）：解析不出「modern」就当古籍，不失败。
    #[allow(clippy::should_implement_trait)]
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

/// 眉心（版心）水平点击带：返回 `Some((左缘, 右缘))`（视口坐标）。
///
/// 中缝是两叶之间 / 信笺纸心那道一字宽的版心——鱼尾、篇题、叶码、
/// 朱砂小印都立在这条带上。点它才呼出目录；带外照旧按左右半屏翻页。
///
/// 几何与渲染同源（`reader::render_gutter` / `render_letter_gutter`）：
///   · 对开：舞台（左叶 + 中缝 + 右叶）在正文区居中且左右对称，
///     中缝恰落在视口水平中心，宽 `gutter_w`（约一字宽）；
///   · 信笺：整张纸居中，中缝列位于纸面左起
///     「纸边16 + 外框3 + 框边3 + 内框1 + 左侧若干正文列 + 半列」处，
///     宽一列（`col_w`）；信笺锁 14 列，中缝左侧恒为 7 个正文列；
///   · 单页古籍没有中缝 → `None`（没有眉心，点中间照旧翻页）。
///
/// `present` 传入中缝此刻是否真的渲染出来了：对开末页没有左半叶，
/// 中缝不渲染——那一屏没有眉心，点中间应当翻页而不是呼目录。
pub fn meixin_band(
    viewport_w: f32,
    letter: bool,
    facing: bool,
    present: bool,
    gutter_w: f32,
    col_w: f32,
    columns: usize,
) -> Option<(f32, f32)> {
    // 命中容差：中缝约一字宽（≈57px），±4px 让边缘也好点。
    const TOL: f32 = 4.0;
    if letter {
        // 叶宽 = (列数+1)×列宽 + 纸边16×2 + 外框3×2 + 框边3×2 + 内框1×2。
        let leaf_w = (columns as f32 + 1.0) * col_w + 46.0;
        let stage_left = ((viewport_w - leaf_w) / 2.0).max(0.0);
        // 中缝列左缘 = 纸边16 + 外框3 + 框边3 + 内框1 + 左侧正文列
        // （信笺 14 列时恒为 7 列；列数变化时与渲染同式：列数-7）。
        let left_cols = columns.saturating_sub(7) as f32;
        let center = stage_left + 23.0 + (left_cols + 0.5) * col_w;
        Some((center - col_w * 0.5 - TOL, center + col_w * 0.5 + TOL))
    } else if facing && present {
        let center = viewport_w * 0.5;
        Some((center - gutter_w * 0.5 - TOL, center + gutter_w * 0.5 + TOL))
    } else {
        None
    }
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

/// 字体选择。
///
/// 分工（换字体方案的核心）：
///   * **正文** —— 大字库宋体（文津宋体 / 梦源宋体）：生僻字全覆盖、
///     竖排标点挤压正确，古籍正文只认这一类；
///   * **批注** —— 楷体（玄冬楷书 / 霞鹜文楷）：夹注小字、放大注卡、题识；
///   * **标题 / UI** —— 思源黑体；
///   * **等宽 / 西文** —— 尚古等宽、西文数字（Computer Modern）。
///
/// 全部为开源字库，**不随包打包**（一部大字符集宋体动辄几十 MB，
/// 会撑爆 App 体积）。放进 `resources/fonts/` 或装进系统即可选中；
/// 未安装时 `fonts::family_for` 沿回退链落到最接近的系统字体，
/// 不会出现豆腐块。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FontChoice {
    // —— 正文：大字符集宋体 ——
    /// 文津宋体（首选：生僻字全覆盖、大陆字形、竖排标点正确）
    WenJinMincho,
    /// 梦源宋体（思源宋体多字重衍生，字重大、屏显厚重）
    MengYuanSerif,
    // —— 批注：楷体 ——
    /// 玄冬楷书（马善政毛笔楷书衍生，含竖排特性）
    XuandongKaishu,
    /// 霞鹜文楷（GB 版，手写楷体、覆盖常用字）
    LXGWWenKai,
    // —— 标题 / UI ——
    /// 思源黑体
    SourceHanSans,
    // —— 等宽 / 西文 ——
    /// 尚古等宽
    ShangguMono,
    /// 西文数字（Computer Modern）
    LatinModern,
    /// 系统默认
    SystemDefault,
    /// 齐伋体（明刻本活字风貌，木味古拙，自带古籍刻本气韵；随包静态打包）
    Qiji,

    // —— 旧版遗留：仅为兼容旧存档与内部兜底，UI 已不再陈列 ——
    SongTi,
    SourceHanSerif,
    ZhuqueFangsong,
    JinghuaOldSong,
    HuiwenMincho,
    HuiwenZhengkai,
    HanchanHuofang,
    HanchanJinshu,
}

impl FontChoice {
    pub fn label(&self) -> &'static str {
        match self {
            FontChoice::WenJinMincho => "文津宋体",
            FontChoice::MengYuanSerif => "梦源宋体",
            FontChoice::XuandongKaishu => "玄冬楷书",
            FontChoice::LXGWWenKai => "霞鹜文楷",
            FontChoice::SourceHanSans => "思源黑体",
            FontChoice::ShangguMono => "尚古等宽",
            FontChoice::LatinModern => "西文数字",
            FontChoice::SystemDefault => "默认字体",
            FontChoice::Qiji => "齐伋体",
            // 旧版遗留
            FontChoice::SongTi => "宋体",
            FontChoice::SourceHanSerif => "思源宋体",
            FontChoice::ZhuqueFangsong => "朱雀仿宋",
            FontChoice::JinghuaOldSong => "京华老宋",
            FontChoice::HuiwenMincho => "汇文明朝",
            FontChoice::HuiwenZhengkai => "汇文正楷",
            FontChoice::HanchanHuofang => "寒蝉活仿楷",
            FontChoice::HanchanJinshu => "寒蝉锦书宋",
        }
    }

    /// 正文字体组（版字页的竖排字牌按此顺序陈列）：
    /// 两款大字库宋体打头，楷体 / 黑体 / 等宽 / 西文随后——一套字体覆盖
    /// 正文、批注、标题、混排西文四类角色，不必再装一堆零散字库。
    pub const CLASSIC: [FontChoice; 8] = [
        FontChoice::WenJinMincho,
        FontChoice::MengYuanSerif,
        FontChoice::XuandongKaishu,
        FontChoice::LXGWWenKai,
        FontChoice::SourceHanSans,
        FontChoice::ShangguMono,
        FontChoice::LatinModern,
        FontChoice::Qiji,
    ];

    /// **批注字体**组：楷体二选一（玄冬楷书 / 霞鹜文楷）。
    ///
    /// 夹注小字与放大注卡专用——正文用宋体、批注用楷体，正是刻本里
    /// 「正文匠体、注文手写体」的层次；两款都装了就按用户选的走。
    pub const NOTE: [FontChoice; 2] = [FontChoice::XuandongKaishu, FontChoice::LXGWWenKai];

    /// 横笺用的无衬线组（思源黑体 / 系统默认）。
    pub const MODERN: [FontChoice; 2] = [FontChoice::SourceHanSans, FontChoice::SystemDefault];
}

/// **排版字体**（设置面板那一行）：刻本宋体 ↔ 手写楷体。
///
/// 一个是「匠体」、一个是「写体」——刻本正文都是匠体宋字，而手写楷体
/// 是稿本 / 写本的味道。切换时正文与批注**一起换**，但两者始终不同体
/// （见 `main_choice` / `note_choice`），层次感不丢。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FontPreset {
    /// 刻本宋体：正文文津宋体、批注霞鹜文楷（默认）。
    Song,
    /// 手写楷体：正文霞鹜文楷、批注玄冬楷书。
    Kai,
}

impl FontPreset {
    pub const ALL: [FontPreset; 2] = [FontPreset::Song, FontPreset::Kai];

    /// 面板上的名字。
    pub fn label(&self) -> &'static str {
        match self {
            FontPreset::Song => "刻本宋体",
            FontPreset::Kai => "手写楷体",
        }
    }

    /// 面板上的副题：写清正文明用哪一款。
    pub fn hint(&self) -> &'static str {
        match self {
            FontPreset::Song => "文津宋体 · 注用霞鹜文楷",
            FontPreset::Kai => "霞鹜文楷 · 注用玄冬楷书",
        }
    }

    /// 该预设的**正文**字体。
    pub fn main_choice(&self) -> FontChoice {
        match self {
            FontPreset::Song => FontChoice::WenJinMincho,
            FontPreset::Kai => FontChoice::LXGWWenKai,
        }
    }

    /// 该预设的**批注**字体（永远与正文不同款）。
    pub fn note_choice(&self) -> FontChoice {
        match self {
            FontPreset::Song => FontChoice::LXGWWenKai,
            FontPreset::Kai => FontChoice::XuandongKaishu,
        }
    }

    /// **正文字体 → 所属预设**（v1.35.0：面板只剩「字体」一行，这是它与
    /// 行高 / 批注字体之间换算的**唯一**公式）。
    ///
    /// 楷体属「写体」（行间要更松、注配玄冬楷书），其余都按匠体排。
    /// 面板上「排版字体」那一行已删——它和「字体」行写的是同一个
    /// `font_ancient`，留着只会互相打脸（见 `Root::set_font`）。
    pub fn for_main(c: FontChoice) -> FontPreset {
        match c {
            FontChoice::XuandongKaishu | FontChoice::LXGWWenKai => FontPreset::Kai,
            _ => FontPreset::Song,
        }
    }

    /// 存档用的下标（与 `ALL` 同源，越界回落首项）。
    pub fn index(self) -> usize {
        FontPreset::ALL
            .iter()
            .position(|p| *p == self)
            .unwrap_or(0)
    }

    pub fn from_index(i: usize) -> FontPreset {
        FontPreset::ALL.get(i).copied().unwrap_or(FontPreset::Song)
    }
}

/// 字级四档：小 / 中 / 大 / 特大。
///
/// 取代原先那条七档圆点滑块——档位太多，相邻两档肉眼几乎分不出来，
/// 反而让人反复试。四档拉开差距，一档一个手感，且档名就是档名，不必数点。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[derive(Default)]
pub enum ZoomLevel {
    Small,
    #[default]
    Normal,
    Large,
    Huge,
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

    /// 版字面板上的**古称**：蝇头 / 中楷 / 大字 / 榜书。
    ///
    /// 面板里再写「小 / 中 / 大 / 特大」是尺规语言，写「蝇头 / 榜书」才是
    /// 书林语言——前者在描述字号，后者在描述一本书的气象。
    pub fn ancient_name(self) -> &'static str {
        match self {
            ZoomLevel::Small => "蝇头",
            ZoomLevel::Normal => "中楷",
            ZoomLevel::Large => "大字",
            ZoomLevel::Huge => "榜书",
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
    /// 晒本：当前叶（含夹注）按真实版式重排成一张竖长纸——
    /// 版框双边、乌丝栏、双行小字夹注、章题列 / 书耳 / 地脚叶码一应俱全，
    /// 保存即导出 PNG（`render_ancient` + 几何覆盖，非简化文本卡）。
    Scroll,
}

impl CardLayout {
    pub fn label(&self) -> &'static str {
        match self {
            CardLayout::Columns => "竖排",
            CardLayout::Single => "单页",
            CardLayout::Note => "八行笺",
            CardLayout::Scroll => "晒本",
        }
    }
}

/// 丝栏（竖排列间那道界行）的颜色：版字面板三选一。
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
    /// **着重**：导入管线里 markdown 的 `**加粗**` / HTML 的 `<strong>`
    /// 转来的 `[高亮]…[/高亮]`。刻本里不用「加粗」这种西式强调，而是
    /// 在字旁加**朱砂圈点**（着重号），故渲染为字右侧的一枚小朱点。
    Emph,
    /// **注音**：EPUB `<ruby><rt>…</rt></ruby>` 经清洗管线转来的
    /// `[拼音:…]` 标记。`text` 是拼音串，挂在宿主字格右上角逐字母竖叠
    /// （走 `ruby::layout`，与生僻字注音同一条渲染路径）。
    ///
    /// 与 `Note` 的区别：注音**不是**注文，不进双行夹注、不加朱红编号；
    /// 它是字格的附属标记，`len` 恒为 1。
    Ruby,
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
    /// **保存时记下的 charIndex 锚点**（章, 段, 偏移）——朱批录一键跳转
    /// 凭它定位（与 `HeadNote::key` / `page_anchor()` 同一口径）。
    ///
    /// v1.49.0 立：用户新写 / 改写的注文在落盘时由注入点回填；书稿解析
    /// 出来的原本注没有此字段（`None`），届时按「所在章段 + `at`」兜底
    /// 推导（见 [`Note::char_index`]）——旧 JSON 因此无需迁移。
    #[serde(default)]
    pub anchor: Option<(usize, usize, usize)>,
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
            anchor: None,
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
            anchor: None,
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
            anchor: None,
        }
    }

    /// 这条注的 **charIndex 锚点**：保存时记的就用记下的；旧注没有记录，
    /// 按「所在章段 + 段内偏移」推导——与 `page_anchor()` / `HeadNote::key`
    /// 同一口径，故朱批录跳转、切列数回找都走得通。
    pub fn char_index(&self, chapter: usize, para: usize) -> (usize, usize, usize) {
        self.anchor.unwrap_or((chapter, para, self.at))
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
    /// 某偏移是否在**着重**（`[高亮]…[/高亮]`）的跨度内。
    ///
    /// 与 `mark_at` 分开：专名 / 书名是「线」，着重是「点」，视觉与语义都不同，
    /// 混在一起会把 `**加粗**` 画成一条专名线，读起来就成了人名。
    pub fn emph_at(&self, offset: usize) -> bool {
        self.notes.iter().any(|n| {
            n.kind == NoteKind::Emph && n.len > 0 && offset >= n.at && offset < n.at + n.len
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
#[derive(Default)]
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
/// 版面流（`Book::to_flow`）自 v1.52.0 起不再插入段首缩进占位（绝对顶格），
/// 此处直接累加纯正文下标，与分页器的下标口径一致——
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
    /// **天头眉批**（v1.37.0）：读者点在版框上方那段天头里手写的一列
    /// 朱砂小字。存的是**归一化坐标**（见 [`HeadNote`]），故窗口缩放、
    /// 换留白档、改列数之后位置自动等比跟随，不用回写存储值。
    #[serde(default)]
    pub head_notes: Vec<HeadNote>,
    /// **天头眉批（v1.55.0 新制）**：`anchor_char_index` 绝对锚定。
    ///
    /// 旧制 `head_notes` 存的是天头里的**归一化坐标** `u/v`——只记「写在天头
    /// 哪个位置」，换列数重排就漂。新制存**被批的那个字在全书的第几个字**，
    /// 天头画在哪个 x、引线连到哪个字，全由当帧版面**反算**（见
    /// [`MarginNote`]）。旧数据在 `normalize_after_load` 里一律清空。
    #[serde(default)]
    pub margin_notes: Vec<MarginNote>,
    /// **书稿 JSON 的格式版本号**（v1.55.0）。落盘时写 [`BOOK_SCHEMA_VERSION`]；
    /// 读进来的老文件没有这个字段 → `#[serde(default)]` 得 0 → 判定为旧格式，
    /// 旧的 `u/v` 眉批直接丢弃。以后再改结构，靠它做升级分叉。
    #[serde(default)]
    pub schema_version: u32,
    /// **章节仓库 id**（v1.50.0「流式 + 虚拟化」）：百万字的丛书正文
    /// **不驻内存**——导入时边解析边落盘到 `~/.jiandu/chapters.db`，
    /// 读书时按章取回、读完即还。`Some(id)` 表示这本书走虚拟化：
    /// `chapters[i].paragraphs` 平时是空的，正文在库里；
    /// `None` 表示全书在内存（小书 / 旧 JSON / 导出用）。
    #[serde(default)]
    pub store_id: Option<String>,
    /// 每章字数。骨架模式下 `chapters[i].paragraphs` 为空，
    /// 进度、跳章偏移、字数统计全靠这张表，不必把正文拉回内存。
    #[serde(default)]
    pub chapter_chars: Vec<usize>,
    pub chapters: Vec<Chapter>,
}

/// **书稿 JSON 的当前格式版本号**。
///
/// * `0` —— v1.55.0 之前：天头眉批存归一化 `u/v`（`HeadNote`），无本字段。
/// * `1` —— v1.55.0 起：眉批改用 `anchor_char_index` 绝对锚定（`MarginNote`）。
///
/// 读档时 `schema_version < 1` 即判为旧格式，旧的 `u/v` 眉批**直接丢弃**
/// （用户明示：旧数据全是测试乱写的，不必迁移）。
pub const BOOK_SCHEMA_VERSION: u32 = 1;

/// **天头眉批（v1.55.0 新制）**：用**全书扁平字符索引**绝对锚定正文。
///
/// 与旧制 [`HeadNote`] 的根本差别：旧制存天头里的归一化坐标 `u/v`，位置是
/// 「记下来的」；新制只存**被批的那个字在全书的第几个字**（`anchor_char_index`），
/// 屏幕坐标永远由当帧版面**反算**出来——视口滚动、改字号、换列数、拖窗口，
/// 都能重新算出那个字的物理位置，朱砂引线始终精准连到原字。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarginNote {
    /// 唯一标识（删改、悬停联动都靠它认人；新建取 `max(id)+1`）。
    #[serde(default)]
    pub id: u64,
    /// **锚定正文的字符索引**（全书扁平下标，与 [`Book::flat_index`] 同口径）。
    /// 这是核心字段：天头画在哪个 x、引线连到哪个字，全由它算出。
    #[serde(default)]
    pub anchor_char_index: usize,
    /// 眉批正文。
    #[serde(default)]
    pub content: String,
    /// 创建时间（Unix 秒）。
    #[serde(default)]
    pub created_at: u64,
}

impl MarginNote {
    pub fn new(id: u64, anchor_char_index: usize, content: impl Into<String>) -> Self {
        Self {
            id,
            anchor_char_index,
            content: content.into(),
            created_at: margin_now_secs(),
        }
    }

    /// **碰撞避让（防重叠）**：一批眉批在天头上的横向占位 `[(x0, x1)]`
    /// → 各自落在第几条「车道」（lane）。**车道就是 Y 轴偏移**：
    /// 同一车道同一高度，x 撞车的自动往下再开一条，于是后写的眉批依次下移，
    /// 绝不会叠成一团墨（用户点名的要求）。
    ///
    /// 算法是**区间划分贪心**：按 x0 升序扫，每条放进「末端已空出来的最低车道」。
    /// 纯函数、不碰存储，便于单测（见 `tests::margin_lanes_*`）。
    /// 返回值与入参**同序**（入参不必预先排序）。
    pub fn assign_lanes(spans: &[(f32, f32)]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..spans.len()).collect();
        order.sort_by(|&a, &b| {
            spans[a]
                .0
                .partial_cmp(&spans[b].0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // 每条车道当前占用到的 x 末端
        let mut lane_end: Vec<f32> = Vec::new();
        let mut out = vec![0usize; spans.len()];
        for i in order {
            let (x0, x1) = spans[i];
            let mut lane = None;
            for (li, end) in lane_end.iter_mut().enumerate() {
                // 0.5px 容差：浮点毛刺不该逼出一条新车道
                if x0 >= *end - 0.5 {
                    *end = x1;
                    lane = Some(li);
                    break;
                }
            }
            let li = match lane {
                Some(l) => l,
                None => {
                    lane_end.push(x1);
                    lane_end.len() - 1
                }
            };
            out[i] = li;
        }
        out
    }
}

/// 当前 Unix 秒（core 层不依赖上层 `library::now_secs`）。
fn margin_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Book {
    /// [`Book::flat_index`] 的**逆运算**：全书扁平字符索引 → 字形键
    /// `(章, 段, 段内偏移)`。
    ///
    /// 渲染层用它把 `anchor_char_index` 还原成「哪一章哪一段第几个字」，
    /// 再在当页里找到那个字、算出屏幕物理坐标。口径与 `flat_index` 对称
    /// （见 `tests::flat_index_roundtrip`）。
    pub fn key_from_flat(&self, idx: usize) -> Option<GlyphKey> {
        let mut acc = 0usize;
        for (ci, ch) in self.chapters.iter().enumerate() {
            let ch_len = Self::flow_chars_of(ch);
            if idx < acc + ch_len {
                let mut pacc = acc;
                for (pi, p) in ch.paragraphs.iter().enumerate() {
                    let plen = p.text.chars().count();
                    if idx < pacc + plen {
                        return Some((ci, pi, idx - pacc));
                    }
                    pacc += plen;
                }
                // 落在章内但超出所有段落（flow 里还有夹注等）：钉在该章末段首字。
                let last = ch.paragraphs.len().saturating_sub(1);
                return Some((ci, last, 0));
            }
            acc += ch_len;
        }
        None
    }

    /// 新眉批的下一个 id（现有最大 id + 1，从 1 起）。
    pub fn next_margin_id(&self) -> u64 {
        self.margin_notes.iter().map(|m| m.id).max().unwrap_or(0) + 1
    }

    /// **读档后规整**：旧格式的 `u/v` 眉批一律清空，版本号升到当前。
    ///
    /// 用户明示旧眉批全是测试乱写的，不做迁移——所以 `schema_version <
    /// BOOK_SCHEMA_VERSION` 时直接 `head_notes.clear()`。顺带丢掉内容为空的
    /// 残条（老 `u/v` 条目反序列化进来 `content` 是空串，正是这么被筛掉的）。
    /// 幂等，可反复调用。
    pub fn normalize_after_load(&mut self) -> bool {
        let legacy = self.schema_version < BOOK_SCHEMA_VERSION;
        if legacy {
            self.head_notes.clear();
        }
        let before = self.margin_notes.len();
        self.margin_notes.retain(|m| !m.content.trim().is_empty());
        self.schema_version = BOOK_SCHEMA_VERSION;
        legacy || self.margin_notes.len() != before
    }
}

impl Book {
    /// 这本书是不是「虚拟化」的（正文在库里、不在内存）。
    pub fn is_virtual(&self) -> bool {
        self.store_id.is_some()
    }

    /// 第 i 章**是否已把正文取回内存**。
    pub fn chapter_loaded(&self, i: usize) -> bool {
        self.chapters
            .get(i)
            .map(|c| !c.paragraphs.is_empty())
            .unwrap_or(false)
    }

    /// 卸下一章的正文（把内存还给系统）。章题与目录不动，
    /// 需要时按 `store_id` + 章号再取回即可。
    pub fn unload_chapter(&mut self, i: usize) {
        if let Some(c) = self.chapters.get_mut(i) {
            c.paragraphs.clear();
            c.paragraphs.shrink_to_fit();
        }
    }
}

/// 同一处再写一条眉批时**向左挪一列**的距离（占纸面宽的比例）。
///
/// 竖排右起，故「下一列」在左边——与传统批书的续写方向一致。
pub const HEAD_NOTE_STEP: f32 = 0.042;
/// 判定「同一位置」的纵向容差（占天头高的比例）：上下差得比这多就不算
/// 挤在一起，不必挪列（天头能并排写两行短批）。
pub const HEAD_NOTE_V_TOL: f32 = 0.28;

/// **一条天头眉批**。
///
/// 位置只存**比例**，不存像素：
///   * `u` — 横向 [0,1]，0 = 纸面左缘（竖排右起，故「向左挪一列」是 `u` 减小）；
///   * `v` — 纵向 [0,1]，0 = 天头顶缘。
///
/// 这样 `margin_top` 随留白档变大、窗口被拖宽拖窄，眉批都跟着天头等比缩放，
/// 绝不会出现「改一次版面，批语全飘到正文里」的事（v1.37.0 用户点名要求）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeadNote {
    /// 钉在哪一面上：该面**首字**的字键 `(chapter, para, offset)`
    /// （与 `page_anchor()` 同源）。分页变了也能靠它把眉批找回去。
    pub key: (usize, usize, usize),
    /// 天头带内的归一化横向位置 [0,1]。
    pub u: f32,
    /// 天头带内的归一化纵向位置 [0,1]。
    pub v: f32,
    /// 批语正文（竖排显示，一般一句）。
    pub text: String,
}

impl HeadNote {
    pub fn new(key: (usize, usize, usize), u: f32, v: f32, text: impl Into<String>) -> Self {
        Self {
            key,
            u: u.clamp(0.0, 1.0),
            v: v.clamp(0.0, 1.0),
            text: text.into(),
        }
    }

    /// **防重叠**：同一面、纵向又挨着的已有眉批，把新批的 `u` **向左挪
    /// 一列**——竖排右起，故「下一列」在左边，与传统批书续写方向一致。
    /// 挪出左缘则绕回右缘（天头写满了就从头接着排，绝不叠成一团墨）。
    ///
    /// 纯函数（不碰存储），便于单测：见 `tests::head_notes_never_overlap`。
    pub fn free_u(notes: &[HeadNote], key: (usize, usize, usize), u: f32, v: f32) -> f32 {
        let step = HEAD_NOTE_STEP;
        let tol = HEAD_NOTE_V_TOL;
        let mut u = u.clamp(0.0, 1.0);
        for _ in 0..24 {
            let taken = notes.iter().any(|n| {
                n.key == key && (n.u - u).abs() < step * 0.8 && (n.v - v).abs() < tol
            });
            if !taken {
                return u;
            }
            u -= step;
            if u < 0.01 {
                u = 0.99;
            }
        }
        u
    }
}

impl Book {
    pub fn new(title: impl Into<String>, author: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            author: author.into(),
            cover: Cover::default(),
            tongjia: BTreeMap::new(),
            toc: Vec::new(),
            head_notes: Vec::new(),
            margin_notes: Vec::new(),
            schema_version: BOOK_SCHEMA_VERSION,
            store_id: None,
            chapter_chars: Vec::new(),
            chapters: Vec::new(),
        }
    }

    pub fn total_chars(&self) -> usize {
        // 骨架模式（正文在库里、chapters 是空壳）：走 `chapter_chars` 账，
        // **不必**为了数数把百万字正文拉回内存。
        if !self.chapter_chars.is_empty() && self.chapter_chars.len() == self.chapters.len() {
            return self.chapter_chars.iter().sum();
        }
        self.chapters.iter().map(|c| c.char_count()).sum()
    }

    /// **平均一章多少字**——自动列数的判据（见 `grid::auto_columns`）。
    ///
    /// 取平均而非总字数：一部一百章的短章语录（如《道德经》）与一部十章的
    /// 长篇，总字数可能相近，但「一叶该装多少」完全不同。空书返回 0。
    pub fn avg_chapter_chars(&self) -> usize {
        if self.chapters.is_empty() {
            return 0;
        }
        self.total_chars() / self.chapters.len()
    }

    /// 把字键 `(chapter, para, offset)` 换算成**全书统一的字符下标**（charIndex）。
    ///
    /// 与 `to_flow` / `flow_chars_of` 同一口径：自 v1.52.0 起 `to_flow` 不再
    /// 插入段首缩进占位（绝对顶格），此处直接按纯正文累加，所以这个下标和
    /// 「版面流里的第几个字」一一对应——长按浮签上显示的「第 N 字」就是它，
    /// 可直接和目录的 `start_char_index` 对照。
    pub fn flat_index(&self, key: GlyphKey) -> usize {
        let (ci, pi, off) = key;
        let mut acc = 0usize;
        for (i, ch) in self.chapters.iter().enumerate() {
            if i == ci {
                for (j, p) in ch.paragraphs.iter().enumerate() {
                    if j == pi {
                        return acc + off;
                    }
                    acc += p.text.chars().count();
                }
                return acc;
            }
            acc += Self::flow_chars_of(ch);
        }
        acc
    }

    /// **吸入正文里残留的 `[注]…[/注]` 标签**（幂等，可反复调用）。
    ///
    /// 标签是「从这里开始排双行小字」的开关信号，**不是正文**：留在文本里
    /// 就会被当成 `[` `注` `]` `/` 四个正文字排进字格（v1.36 修的正是这个）。
    /// 这里把它们摘成 `Note{ NoteKind::Note }` 挂到宿主字上，正文从此只剩
    /// 纯字——翻页、检索、长按取字拿到的也都是干净文本。
    ///
    /// 导入管线新产出的段落已经拆过一遍（`pipeline::paragraph_of`），这一道
    /// 是给「直接反序列化进来的旧 JSON」兜底：书架里那些早先存下的书，
    /// 打开时也会被顺手洗干净。
    pub fn ingest_inline_notes(&mut self) {
        for ch in self.chapters.iter_mut() {
            for p in ch.paragraphs.iter_mut() {
                // v1.40.0：旧档里残留的现代括号先转成 `[注]…[/注]`
                // （`〔名午，考王之子〕` 这类括注当年是直接排进字格的），
                // 于是老书**开卷即洗净**，不必重新导入。
                if p.text.contains('〔') || p.text.contains('【') || p.text.contains('[') {
                    p.text = crate::parsers::purge_bracket_marks(&p.text);
                }
                if !p.text.contains("[注]") {
                    continue;
                }
                let (clean, notes) = crate::pipeline::split_notes(&p.text);
                p.text = clean;
                for (at, text) in notes {
                    p.notes.push(Note::new(at, NoteKind::Note, text));
                }
            }
        }
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

    /// 一章在版面流里占的字符数：自 v1.52.0 起 `to_flow` 不再插入段首缩进占位
    /// （绝对顶格），此处直接按纯正文计数，与 `to_flow` 的口径一致——
    /// 用于把「章」换算成「全局字符下标」。
    fn flow_chars_of(ch: &Chapter) -> usize {
        ch.paragraphs.iter().map(|p| p.text.chars().count()).sum()
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

    /// **单章**的字形流（v1.60.0 惰性分页的基石）。
    ///
    /// 分页引擎里，**章边界必然收面**（`layout_book_from_opts` 的
    /// `chapter_start` 分支无条件 `seal!()`），所以「逐章排完再串起来」与
    /// 「全书一次排完」在数学上等价。由此可以把百万字的排版拆成若干块，
    /// 只在需要时排其中一块——`to_flow` 的章内实现与全书版逐字一致
    /// （同一套标点显隐、同一套书名号替换），故两种排法的字序也相同。
    ///
    /// 返回的 `Glyph.chapter` 仍是**全书章号** `ci`（不是 0），这样分页引擎
    /// 判章边界、渲染层反查章节的行为都与全书流一致。
    pub fn flow_of_chapter(&self, ci: usize, show_punct: bool) -> Vec<Glyph> {
        let mut out = Vec::new();
        let Some(ch) = self.chapters.get(ci) else {
            return out;
        };
        for (pi, p) in ch.paragraphs.iter().enumerate() {
            for (oi, c) in p.text.chars().enumerate() {
                if !show_punct && is_punct(c) {
                    continue;
                }
                out.push(Glyph {
                    chapter: ci,
                    para: pi,
                    offset: oi,
                    ch: title_mark(c),
                    kind: if is_title_mark(c) {
                        GlyphKind::Attach
                    } else {
                        GlyphKind::Char
                    },
                });
            }
        }
        out
    }

    /// 把全书拍平成字形流（供分页使用）。
    /// **绝对顶格**：每段之前不再插入任何段首缩进占位——古籍竖排本就
    /// 顶格起排，现代「首行缩进两个字」在此为刻意剔除（v1.52.0 起）。
    /// 字符流里已无任何 `Indent` 占位，段与段之间只靠换行 / 列尾自然收束，
    /// 正文第一列从版框顶缘起排。
    pub fn to_flow(&self, show_punct: bool) -> Vec<Glyph> {
        let mut out = Vec::new();
        for (ci, ch) in self.chapters.iter().enumerate() {
            for (pi, p) in ch.paragraphs.iter().enumerate() {
                for (oi, c) in p.text.chars().enumerate() {
                    // 标点显隐：关闭时不产出标点字（offset 仍按原文计数，注文定位不受影响）。
                    if !show_punct && is_punct(c) {
                        continue;
                    }
                    // **书名号在进排版引擎之前就换成竖排专用形**（v1.30.0）：
                    // 《》→︽︾，且标成 `Attach`——它不独占一格，缩到正文 70%
                    // 叠在相邻字的格右上角。渲染层因此不必再判横排原形，
                    // 分页与渲染拿到的是同一个字符、同一套口径。
                    let ch = title_mark(c);
                    let kind = if is_title_mark(c) {
                        GlyphKind::Attach
                    } else {
                        GlyphKind::Char
                    };
                    out.push(Glyph {
                        chapter: ci,
                        para: pi,
                        offset: oi,
                        ch,
                        kind,
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
    /// 段首缩进占位：v1.52.0 起 `Book::to_flow` 已不再产出此变体（绝对顶格），
    /// 此处保留仅作历史兼容——渲染 / 命中 / 注文注入等处的 `is_indent()` 过滤
    /// 仍可安全调用（对当前流恒为 false）。
    Indent,
    /// **挤压标点**：本列已排满、紧随末字之后的收束型句读（。，、；？！」…）。
    ///
    /// 它不独占一格（半格账记 0），由渲染层叠画在**前一个字格的右下角**，
    /// 故绝不另起一列占列首格。只出现在 `gloss::layout_book*` 产出的
    /// `Piece::Main` 里；`Page::glyphs` 的读序里它仍是 `Char`（保 charIndex
    /// 与检索、注文锚点一致）。见 [`crate::punct`]。
    Hang,
    /// **书名号附着记号**（v1.30.0）：`︽ ︾ ︿ ﹀`。
    ///
    /// 书名号**绝不单独占据一个网格**——半格账记 0，由渲染层缩到正文
    /// [`crate::punct::TITLE_FS_RATIO`]（0.7）叠画到**相邻字格**的右上角：
    /// 开号（`︽`）领起书名，贴书名首字那一格；闭号（`︾`）收束书名，
    /// 贴书名末字那一格。如此书名号永不落列首、也永不落列尾，行气不断。
    ///
    /// 与 `Hang` 的区别：`Hang` 是「列满了硬挤进末字」的兜底，画在格**右下角**；
    /// `Attach` 是常态排版，画在格**右上角**、且字号是单独一档（0.7 而非 0.3）。
    Attach,
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
    /// 挤在前一字格右下角的句读（不独占一格）。
    pub fn is_hang(&self) -> bool {
        self.kind == GlyphKind::Hang
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

/// 竖排古籍只用**直角引号**：方头括号【】与西式弯引号 “”‘’ 在显示层
/// 1:1 换成 「」『』（源数据不动，导出仍是原样）。
///
/// 弯引号是横排铅字的形制，竖排里它的开口朝上下、方向整个是错的；直角
/// 引号「」的开口朝左右，正是竖排该有的样子——故不必旋转 90° 兜底，
/// 直接换字即可。
///
/// 正文（`shown`）、夹注、章题、中缝篇题、题署、分享题识、书脊签、封面、
/// 书耳题签都经这一道——书本里露出的引号只有直角形。
pub fn bracket_to_quote(c: char) -> char {
    match c {
        '【' | '“' => '「',
        '】' | '”' => '」',
        '‘' => '『',
        '’' => '』',
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

/// 书名号：横排的 `《》〈〉` 与竖排专用 `︽︾︿﹀`。
///
/// 它们**不是句读**——包着一整串书名，不能按 0.4 倍印成小点；渲染层给
/// 0.8 倍（`punct::TITLE_FS_RATIO`）并强制换成竖排形态（[`title_mark`]）。
pub fn is_title_mark(c: char) -> bool {
    matches!(c, '《' | '》' | '〈' | '〉' | '︽' | '︾' | '︿' | '﹀')
}

/// 横排书名号 → 竖排专用字形：`《`→`︽`、`》`→`︾`（单书名号同理）。
///
/// 竖排里 `《` 的那道尖角朝下，插在列中会把行气戳断；`︽` 是它的竖排
/// 形态（尖端朝右、两撇横卧），才立得住。已经是竖排形态的原样返回。
pub fn title_mark(c: char) -> char {
    match c {
        '《' => '︽',
        '》' => '︾',
        '〈' => '︿',
        '〉' => '﹀',
        other => other,
    }
}

/// 书名号是否**开启**类（`︽ ︿ 《 〈`）：领起书名，宿主是**后一个字**。
///
/// v1.30.0 起书名号不再独占一格：它缩到正文 70%、叠加到相邻字的格右上角。
/// 开号领起书名，故贴书名**首字**那一格（渲染层 look-behind）；
/// 闭号收束书名，故贴书名**末字**那一格（渲染层 look-ahead，与 `Hang` 同路）。
pub fn is_open_title(c: char) -> bool {
    matches!(c, '︽' | '︿' | '《' | '〈')
}

/// 书名号是否**收束**类（`︾ ﹀ 》 〉`）。宿主是**前一个字**。
pub fn is_close_title(c: char) -> bool {
    matches!(c, '︾' | '﹀' | '》' | '〉')
}

/// **剔除没有文字包裹的空书名号**（v1.30.0，目录专用）。
///
/// 目录里常见 `︽︾`、`《》` 这种空壳（清洗漏下来的、或书名缺失），
/// 竖排时它们会变成两个孤零零的小角标，把行气戳断——连壳删掉。
///
/// 只删**成对相邻**的空壳；`︽道德经︾` 这种有内容的原样保留。
/// 落单的一个（如只有 `︽`）也删——没有包裹对象的书名号没有意义。
pub fn strip_empty_title_marks(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(cs.len());
    let mut i = 0usize;
    while i < cs.len() {
        if is_title_mark(cs[i]) {
            // 向后扫：连续的书名号（含空壳与噪点空白）算一簇。
            let mut j = i;
            let mut has_text = false;
            let mut close_seen = false;
            while j < cs.len() {
                let c = cs[j];
                if is_open_title(c) {
                    if close_seen {
                        break; // 上一对已闭合，这是下一对的开始
                    }
                } else if is_close_title(c) {
                    close_seen = true;
                } else if c.is_whitespace() {
                    // 空白不计作「包裹的文字」
                } else {
                    has_text = true;
                    break;
                }
                j += 1;
            }
            if !has_text {
                // 空壳 / 落单：整簇丢掉
                i = j;
                continue;
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out.into_iter().collect()
}

/// 引号：直角引号 `「」『』` 与弯引号 `“”‘’`（含已是竖排异体的）。
pub fn is_quote_mark(c: char) -> bool {
    matches!(
        c,
        '「' | '」'
            | '『'
            | '』'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '﹁'
            | '﹂'
            | '﹃'
            | '﹄'
    )
}

/// 引号 → **旋转 90° 的竖排异体**：`「`→`﹁`、`」`→`﹂`、`『`→`﹃`、`』`→`﹄`。
///
/// 竖排里直角引号的开口要朝下（读序方向），等于把横排的字形转 90°。
/// GPUI 0.2.2 没有元素级旋转变换（`TransformationMatrix` 只在内部 sprite
/// 用），所以直接用 Unicode 的竖排呈现形（U+FE41..FE44）——**字库里存的
/// 就是转过的那一枚**，等于「旋转」而不必真做变换。
pub fn vertical_quote(c: char) -> char {
    match c {
        '「' | '“' => '﹁',
        '」' | '”' => '﹂',
        '『' | '‘' => '﹃',
        '』' | '’' => '﹄',
        other => other,
    }
}

/// 书名号 / 引号的「竖排形态」一把抓：观感层统一走它（正文、夹注、晒本）。
pub fn vertical_mark(c: char) -> char {
    if is_title_mark(c) {
        title_mark(c)
    } else if is_quote_mark(c) {
        vertical_quote(c)
    } else {
        c
    }
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

/// **竖排标点**：横排标点在竖排里要么躺倒（`:` `;`）、要么开口朝错方向
/// （`“ ”` `（ ）`），必须换成 Unicode 的**竖排呈现形**（U+FE10–FE19 与
/// U+FE41–FE44）——字库里存的就是转好的那一枚，GPUI 0.2.2 无元素级旋转，
/// 这是唯一正解（v1.30.0 用户反馈「船山遗书：第一」的冒号极度突兀）。
pub fn vertical_punct(c: char) -> char {
    match c {
        // 半角（EPUB 里常见未清洗的 ASCII 标点）
        ':' => '︓', // U+FE13 竖排冒号
        ';' => '︔', // U+FE14 竖排分号
        '!' => '︕', // U+FE15 竖排叹号
        '?' => '︖', // U+FE16 竖排问号
        ',' => '︐', // U+FE10 竖排逗号
        '.' => '︒', // U+FE12 竖排句号
        '(' => '︵', // U+FE35 竖排左括号
        ')' => '︶', // U+FE36 竖排右括号
        '[' | '［' => '︗', // U+FE47 竖排左方括号
        ']' | '］' => '︘', // U+FE48 竖排右方括号
        // 全角横排标点 → 竖排形
        '：' => '︓',
        '；' => '︔',
        '！' => '︕',
        '？' => '︖',
        '，' => '︐',
        '。' => '︒',
        '、' => '︑', // U+FE11 竖排顿号
        '“' => '﹁',
        '”' => '﹂',
        '‘' => '﹃',
        '’' => '﹄',
        '（' => '︵',
        '）' => '︶',
        other => other,
    }
}

/// 是否**竖排标点**（含横排原形与其竖排呈现形）：是就走句读那一档
/// （[`crate::punct`] 的小字 + 右下角偏移），绝不与正文争格心。
pub fn is_vertical_punct(c: char) -> bool {
    matches!(
        c,
        ':' | ';' | '!' | '?' | ',' | '.' | '(' | ')' | '[' | ']'
            | '：' | '；' | '！' | '，' | '。' | '、'
            | '“' | '”' | '‘' | '’' | '（' | '）'
            // 竖排呈现形本身
            | '︓' | '︔' | '︕' | '︖' | '︐' | '︒' | '︑'
            | '﹁' | '﹂' | '﹃' | '﹄' | '︵' | '︶' | '︗' | '︘'
    )
}

/// 句读分两形：**句**用「朱点」（实心小圆），**读**用「朱弧」（小弧线）。
///
/// 句号 / 叹号 / 问号（含其竖排呈现形）算「句」，落一枚朱点；其余标点
/// （逗号 / 顿号 / 冒号 / 分号 / 省略号 / 破折号）算「读」，落一枚小弧。
/// 字库里的 `，。` 字形在极小字号下容易缩成一枚「奇怪的黑痣」，故渲染层
/// 不再画字体字形，而是直接画圆点 / 弧线（见 `reader::punct_overlay`）。
pub fn is_stop_punct(c: char) -> bool {
    matches!(c, '。' | '．' | '！' | '？' | '︒' | '︕' | '︖' | '…' | '‥' | '.')
}

/// 是否「读」类句读（逗号 / 顿号 / 冒号 / 分号 / 省略号 / 破折号等）。
///
/// 在 `is_punct` 集合内、且非 [`is_stop_punct`] 的，都算「读」——渲染成
/// 一枚小弧线，与「句」的朱点区分。
pub fn is_comma_punct(c: char) -> bool {
    is_punct(c) && !is_stop_punct(c)
}

/// **剔除残留的孤立注释标记**（v1.30.0）。
///
/// 清洗管线偶有漏网的 `[注]` / `[/注]` / `[ / 注 ]`（标签被截断、或原书
/// 里本就写着这两个词），渲染时会变成四个方括号加一个「注」字的乱码，
/// 且占掉四五个字格。这里按「成对才算注」的口径扫一遍：
///   * 完整的 `[注]…[/注]` —— 原样保留（交给双行夹注渲染）；
///   * 落单的 `[注]` / `[/注]` / 带空格的变体 —— 连壳删掉。
pub fn strip_dangling_note_marks(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < n {
        if chars[i] != '[' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // 试着读一个注标记（允许内部空白：`[注]` / `[ / 注 ]` / `[/注]` …）。
        match read_note_marker(&chars, i) {
            Some((MarkerKind::Open, span)) => {
                // 向前找配对的收标记，按开标记深度计数，绝不跨层配对。
                let mut depth = 1usize;
                let mut j = i + span;
                let mut close_end: Option<usize> = None;
                while j < n {
                    if chars[j] == '[' {
                        if let Some((k, sp)) = read_note_marker(&chars, j) {
                            match k {
                                MarkerKind::Open => depth += 1,
                                MarkerKind::Close => {
                                    depth -= 1;
                                    if depth == 0 {
                                        close_end = Some(j + sp);
                                        break;
                                    }
                                }
                            }
                            j += sp;
                            continue;
                        }
                    }
                    j += 1;
                }
                if let Some(ce) = close_end {
                    // 成对：整段（含两端标记）原样保留，并推进到配对收标记之后。
                    out.extend(&chars[i..ce]);
                    i = ce;
                } else {
                    // 落单的开标记：连壳删，推进过这个开标记。
                    i += span;
                }
            }
            Some((MarkerKind::Close, span)) => {
                // 落单的收标记（前方没有对应的开）：连壳删。
                i += span;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// 注标记种类。
enum MarkerKind {
    Open,  // [注]
    Close, // [/注]
}

/// 在 `chars[i]` 处读取一个注标记（允许方括号内空白），返回 `(种类, 字符长度)`。
/// 读不到合法标记（如 `[[` 或 `[x]`）返回 `None`。
fn read_note_marker(chars: &[char], i: usize) -> Option<(MarkerKind, usize)> {
    let n = chars.len();
    if chars.get(i) != Some(&'[') {
        return None;
    }
    // 跳过 `[` 后的空白。
    let mut k = i + 1;
    while k < n && chars[k].is_whitespace() {
        k += 1;
    }
    let close = chars.get(k) == Some(&'/');
    if close {
        k += 1;
    }
    // 收集非空白内容。
    let mut inner = String::new();
    while k < n && chars[k] != ']' {
        if !chars[k].is_whitespace() {
            inner.push(chars[k]);
        }
        k += 1;
    }
    if k >= n {
        return None; // 没有收括号
    }
    if inner != "注" {
        return None; // 不是注标记（可能是 `[拼音:…]` 以外的东西）
    }
    // 跳过 `]` 后的空白。
    let mut end = k + 1;
    while end < n && chars[end].is_whitespace() {
        end += 1;
    }
    Some((if close { MarkerKind::Close } else { MarkerKind::Open }, end - i))
}


/// 点字弹出的工具条所锚定的字：记录该字的归属键、字符与点击处的窗口坐标。
/// 坐标来自 `MouseDownEvent.position`，用于在点击位置附近浮起工具条。
// 注意：不是 `Copy`——带了选中文本（`String`）。取用一律 `.clone()` /
// `as_ref()`，改动点集中在 `render_glyph_menu` 与 `render_lookup_card`。
#[derive(Clone)]
pub struct GlyphHit {
    /// 字的归属键 (chapter, para, offset)——即这个字在全书里的 charIndex。
    pub key: (usize, usize, usize),
    /// 被点击的字符。
    pub ch: char,
    /// 选中的**文本内容**：有拖选区取选区全文，单击 / 长按单字就退回这一个字。
    ///
    /// 长按查字浮签要同时显示「字」与「选中文本」，所以在这里一并记下，
    /// 免得浮层再回头重算一遍选区（算选区要重新分页，代价不小）。
    pub text: String,
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

    // ---- v1.55.0 新制：anchor_char_index 绝对锚定的天头眉批 ----

    /// 两条眉批 x 撞车 → 后一条**下移一条车道**（Y 轴偏移），绝不叠字。
    #[test]
    fn margin_lanes_push_down_on_overlap() {
        let lanes = MarginNote::assign_lanes(&[(10.0, 40.0), (15.0, 45.0)]);
        assert_eq!(lanes, vec![0, 1], "x 重叠却没下移：{lanes:?}");
    }

    /// x 离得远 → 共用同一条车道（同一高度，不该无谓下移）。
    #[test]
    fn margin_lanes_share_when_far_apart() {
        let lanes = MarginNote::assign_lanes(&[(10.0, 40.0), (200.0, 230.0)]);
        assert_eq!(lanes, vec![0, 0], "相距很远却分了车道：{lanes:?}");
    }

    /// 三条全撞车 → 0 / 1 / 2 三条车道依次下移。
    #[test]
    fn margin_lanes_three_overlapping_stack() {
        let lanes = MarginNote::assign_lanes(&[(10.0, 40.0), (12.0, 42.0), (14.0, 44.0)]);
        assert_eq!(lanes, vec![0, 1, 2], "三条没依次下移：{lanes:?}");
    }

    /// 入参无需预先排序：**返回值仍与入参同序**，车道按 x 的实际先后派。
    #[test]
    fn margin_lanes_keeps_input_order() {
        // 两条互相重叠、但入参是倒着给的：x 小的那条（入参第二个）拿车道 0，
        // x 大的（入参第一个）被挤到车道 1 —— 结果与顺序无关，只与 x 有关。
        let lanes = MarginNote::assign_lanes(&[(15.0, 45.0), (10.0, 40.0)]);
        assert_eq!(lanes, vec![1, 0], "乱序入参车道派错了：{lanes:?}");
        // 顺序换回来，同一对区间应得到同样的车道分配（只是顺序跟着换了）。
        let lanes2 = MarginNote::assign_lanes(&[(10.0, 40.0), (15.0, 45.0)]);
        assert_eq!(lanes2, vec![0, 1], "顺序变了车道也变了：{lanes2:?}");
    }

    /// `flat_index` 与 `key_from_flat` 必须互为逆运算——这是「绝对锚定」的地基。
    #[test]
    fn flat_index_roundtrip() {
        let b = sample();
        for k in [(0usize, 0usize, 0usize), (0, 0, 3), (0, 0, 7)] {
            let idx = b.flat_index(k);
            assert_eq!(b.key_from_flat(idx), Some(k), "索引 {idx} 还原不成 {k:?}");
        }
        // 越界索引不该凭空造出一个键。
        assert!(b.key_from_flat(usize::MAX).is_none());
    }

    /// 眉批要能落 JSON、原样读回（本地持久化）。
    #[test]
    fn margin_note_json_roundtrip() {
        let n = MarginNote::new(7, 1234, "批曰：此本与《索隐》异");
        let s = serde_json::to_string(&n).expect("序列化失败");
        let back: MarginNote = serde_json::from_str(&s).expect("反序列化失败");
        assert_eq!(n, back);
        assert!(s.contains("anchor_char_index"), "核心字段没落进 JSON：{s}");
        assert_eq!(back.anchor_char_index, 1234);
        assert_eq!(back.content, "批曰：此本与《索隐》异");
    }

    /// **旧格式（u/v 眉批）读档即清空**，版本号升到当前。
    #[test]
    fn legacy_head_notes_cleared_on_load() {
        let mut b = sample();
        b.schema_version = 0; // 伪造成老文件
        b.head_notes.push(HeadNote::new((0, 0, 0), 0.5, 0.5, "旧测试批"));
        // 老条目反序列化进来 content 是空串（字段名对不上），正是这么被筛掉的。
        b.margin_notes.push(MarginNote {
            id: 1,
            anchor_char_index: 0,
            content: String::new(),
            created_at: 0,
        });
        b.normalize_after_load();
        assert!(b.head_notes.is_empty(), "旧 u/v 眉批没被清空");
        assert!(b.margin_notes.is_empty(), "空内容残条没被清掉");
        assert_eq!(b.schema_version, BOOK_SCHEMA_VERSION, "版本号没升级");
    }

    /// 新格式（版本号已是当前）不该误伤正常眉批。
    #[test]
    fn current_schema_keeps_valid_margin_notes() {
        let mut b = sample();
        b.schema_version = BOOK_SCHEMA_VERSION;
        b.margin_notes.push(MarginNote::new(1, 42, "真批语"));
        b.normalize_after_load();
        assert_eq!(b.margin_notes.len(), 1, "正常眉批被误删");
        assert_eq!(b.margin_notes[0].anchor_char_index, 42);
    }

    /// 天头眉批**绝不允许叠字**：同一处再写一条，`u` 必须向左让开一列。
    #[test]
    fn head_notes_never_overlap() {
        let k = (0usize, 0usize, 0usize);
        let first = HeadNote::new(k, 0.70, 0.22, "批一");
        // 同一位置第二条：必须向左挪（u 变小），不能原地叠上去。
        let u2 = HeadNote::free_u(std::slice::from_ref(&first), k, 0.70, 0.22);
        assert!(u2 < 0.70 - 1e-6, "同位置没让开：u2 = {u2}");
        assert!(u2 > 0.0, "让出了左缘：u2 = {u2}");
        // 第三条继续往左。
        let two = vec![first.clone(), HeadNote::new(k, u2, 0.22, "批二")];
        let u3 = HeadNote::free_u(&two, k, 0.70, 0.22);
        assert!(u3 < u2, "第三条没继续让：u3 = {u3} vs u2 = {u2}");
        // 纵向差得远（不在同一行气里）就不算挤——原位即可。
        let u_far = HeadNote::free_u(std::slice::from_ref(&first), k, 0.70, 0.90);
        assert!((u_far - 0.70).abs() < 1e-6, "上下不挨着却被挪：{u_far}");
        // 另一面的眉批互不相干（key 不同）。
        let u_other = HeadNote::free_u(&[first], (3, 1, 7), 0.70, 0.22);
        assert!((u_other - 0.70).abs() < 1e-6, "别面的眉批不该干扰：{u_other}");
    }

    /// 眉批的新字段必须向后兼容：旧 JSON 里没有 `head_notes` 也能读进来。
    #[test]
    fn book_without_head_notes_still_loads() {
        let j = r#"{"title":"旧档","chapters":[{"title":"一","paragraphs":["甲"]}]}"#;
        let b: Book = serde_json::from_str(j).expect("旧书稿应能读");
        assert!(b.head_notes.is_empty());
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
    fn meixin_band_facing_centered_on_viewport() {
        // 对开：中缝恰在视口中心，宽 = gutter_w（+4px 容差）。
        let (lo, hi) = meixin_band(1200.0, false, true, true, 57.0, 0.0, 14).unwrap();
        assert!((lo - (600.0 - 57.0 / 2.0 - 4.0)).abs() < 0.01);
        assert!((hi - (600.0 + 57.0 / 2.0 + 4.0)).abs() < 0.01);
        // 带比中缝略宽：带外左右各留至少 1px 的翻页区。
        assert!(lo < 600.0 - 57.0 / 2.0 && hi > 600.0 + 57.0 / 2.0);
    }

    #[test]
    fn meixin_band_facing_absent_on_last_spread() {
        // 对开末页：没有左半叶 → 中缝不渲染 → 没有眉心（None），
        // 点中间照旧按左右半屏翻页。
        assert_eq!(meixin_band(1200.0, false, true, false, 57.0, 0.0, 14), None);
    }

    #[test]
    fn meixin_band_single_ancient_has_none() {
        // 单页古籍没有中缝：没有眉心。
        assert_eq!(meixin_band(800.0, false, false, false, 57.0, 0.0, 8), None);
    }

    #[test]
    fn meixin_band_letter_gutter_at_paper_heart() {
        // 信笺 14 列：col_w = (content_w - 50)/15，content_w = vw - 44。
        // 与渲染同源重算：叶宽 = 15×col_w + 46，
        // 中缝心 = (vw - 叶宽)/2 + 23 + 7.5×col_w。
        let vw = 1202.0;
        let col_w = (vw - 44.0 - 50.0) / 15.0;
        let (lo, hi) = meixin_band(vw, true, false, false, 0.0, col_w, 14).unwrap();
        let leaf_w = 15.0 * col_w + 46.0;
        let center = (vw - leaf_w) / 2.0 + 23.0 + 7.5 * col_w;
        assert!((lo - (center - col_w / 2.0 - 4.0)).abs() < 0.01);
        assert!((hi - (center + col_w / 2.0 + 4.0)).abs() < 0.01);
        // 常规窗口下中缝几乎正落在视口中心（纸面近满宽，偏差 < 2px）。
        assert!((center - vw * 0.5).abs() < 2.0);
    }

    #[test]
    fn meixin_band_letter_narrow_window_stays_positive() {
        // 极窄窗：纸宽超过视口时 stage_left 兜底为 0，带不跑出负坐标。
        let (lo, hi) = meixin_band(320.0, true, false, false, 0.0, 30.0, 14).unwrap();
        assert!(lo > 0.0 && hi > lo);
        assert!(hi <= 320.0 + 30.0);
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
        // v1.52.0 起不再产出段首缩进占位——流里应无任何 Indent 字形。
        assert_eq!(without.iter().filter(|g| g.is_indent()).count(), 0);
    }

    #[test]
    fn toc_start_char_index_matches_flow() {
        // 目录的 start_char_index 必须与排版引擎的版面流下标口径一致，
        // 否则「点章跳转」会落到错误的页。v1.52.0 起每段不再塞段首缩进占位
        // （绝对顶格），故第二章首字直接接在第一章正文之后。
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
        // 第一章首字在流 0；第二章首字在流 3（紧接第一章 3 字正文，无缩进占位）。
        assert_eq!(b.toc[0].start_char_index, Some(0));
        assert_eq!(b.toc[1].start_char_index, Some(3));
        // 用 to_flow 反查：流下标 3 正指向第二章第 0 字。
        let flow = b.to_flow(true);
        let g = flow.get(3).expect("流下标 3 应命中第二章首字");
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
    fn western_quotes_become_corner_quotes() {
        // 西式弯引号开口朝上下、竖排里方向是错的：一律换成直角引号。
        assert_eq!(bracket_to_quote('“'), '「');
        assert_eq!(bracket_to_quote('”'), '」');
        assert_eq!(bracket_to_quote('‘'), '『');
        assert_eq!(bracket_to_quote('’'), '』');
        // 一整句：引号成对翻转，正文不动。
        let s: String = "子曰：“學而時習之。”".chars().map(bracket_to_quote).collect();
        assert_eq!(s, "子曰：「學而時習之。」");
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
    fn flat_index_matches_flow_order() {
        let mut b = Book::new("下标", "佚名");
        b.chapters.push(Chapter {
            title: "一".into(),
            paragraphs: vec![Paragraph::plain("道可道"), Paragraph::plain("非常道")],
        });
        b.chapters.push(Chapter {
            title: "二".into(),
            paragraphs: vec![Paragraph::plain("上善若水")],
        });
        // 绝对顶格：每段不再有 2 个段首缩进占位，下标即纯正文累加。
        // 第一章：段① 3 字；段② 3 字。
        assert_eq!(b.flat_index((0, 0, 0)), 0);
        assert_eq!(b.flat_index((0, 0, 2)), 2);
        assert_eq!(b.flat_index((0, 1, 0)), 3);
        assert_eq!(b.flat_index((0, 1, 2)), 5);
        // 第二章接在第一章之后：3 + 3 = 6 起（无缩进占位）。
        assert_eq!(b.flat_index((1, 0, 0)), 6);
        // 单调递增：阅读顺序即下标顺序。
        let keys = [(0, 0, 0), (0, 0, 1), (0, 1, 0), (1, 0, 0), (1, 0, 3)];
        let idx: Vec<usize> = keys.iter().map(|k| b.flat_index(*k)).collect();
        assert_eq!(idx, vec![0, 1, 3, 6, 9]);
        // 越界键不 panic（返回全书正文总数：6 + 4）。
        assert_eq!(b.flat_index((9, 9, 9)), 10);
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
