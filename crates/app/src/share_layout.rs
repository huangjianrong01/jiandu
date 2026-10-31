//! 分享卡的**纯数据版式层**（v1.57.0 重构）
//!
//! 旧制是「GPUI 元素直接拼出来 + `screencapture` 截屏」：截图依赖屏幕录制权限、
//! 分辨率随窗口走、锁屏时直接失败——用户明令**禁止截屏、必须离屏高清出图**。
//! 故把版式抽成与渲染后端无关的一层数据：
//!
//! ```text
//!   LayoutSpec  ──►  layout()  ──►  CardLayout
//!                                      ├─► GPUI 预览（屏幕上的小卡，`paint_gpu`）
//!                                      └─► CoreGraphics 出图（1080×1920 PNG，`share_paint`）
//! ```
//!
//! 两个后端读同一份 `CardLayout`，所以**预览就是成图**：同样的底色、界框、
//! 竖排列款、朱批引线、印章位置。本文件零 GPUI / 零 macOS API，纯函数可单测。
//!
//! 排版铁律（与阅读引擎同宗）：
//! 1. **强制首行顶格**：每一列的第一个字 `y` 恒等于 `y0`，绝不因列长而错动
//!    （对应阅读侧的 `grid::top_baseline`）。且 `y0` **紧贴版框内缘**
//!    （见 [`top_baseline`]）——正文块不再垂直居中，天头不留空档。
//! 2. **从右向左**：第 0 列在最右，符合古籍行款。
//! 3. **禁用 `unwrap`**：越界 / 除零一律走默认值。
//! 4. **边栏 / 界行齐全**（v1.61.0）：版框（外框线）与界行（列间竖线）
//!    由版式层一次算好，绘制层只管按序落笔——预览与成图同源。

use crate::share::{ShareBg, ShareFrame, ShareSeal, frame_ink, norm_columns, palette};

/// 分享卡正文的字体**语义类**（离屏渲染器据此挑系统 PostScript 字体）。
///
/// GPUI 侧用的是注册过的家族名（文津宋体 / 霞鹜文楷…），那些字体不一定在系统里
/// 有同名 PostScript 字体；CoreGraphics 只认 PostScript 名，故另立一档语义。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShareFont {
    #[default]
    /// 宋体（刻本正统）。
    Song,
    /// 楷体（手写气）。
    Kai,
    /// 黑体（现代极简）。
    Hei,
}

impl ShareFont {
    /// 离屏渲染用的候选 PostScript 名（按序回退，第一个系统真有的生效）。
    pub fn postscript_chain(self) -> &'static [&'static str] {
        match self {
            ShareFont::Song => &["Songti SC", "STSong", "SimSun", "Songti TC", "serif"],
            ShareFont::Kai => &["Kaiti SC", "STKaiti", "KaiTi", "Kaiti TC", "serif"],
            ShareFont::Hei => &["PingFang SC", "Heiti SC", "Heiti TC", "sans-serif"],
        }
    }
}

/// 单字的 **Markdown 装饰位**（由 [`clean_md`] 产出，与 `LayoutSpec::body` 等长）。
///
/// 只记「这个字要怎么印」，不记语法本身——`#` / `**` 在清洗期就已经从串里
/// 剔干净了，版式层拿到的永远是纯文字 + 一排小旗子。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MdMark {
    /// 标题字（`#` 行）：字号放大一档，**字格不动**（见 [`HEAD_FS_RATIO`]）。
    pub head: bool,
    /// 加粗字（`**…**`）：离屏落笔时同字加厚，不换字体。
    pub bold: bool,
}

/// 一枚落位的字（字格左上为锚点，绘制时按字格居中）。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedChar {
    pub ch: char,
    /// 字格左上 x / y。
    pub x: f32,
    pub y: f32,
    /// 字格边长（正方形字格，正文与批注同制）。
    pub cell: f32,
    /// 字号。
    pub fs: f32,
    /// 墨色（`0xRRGGBB`）。
    pub color: u32,
    /// 标点：竖排里靠字格**右上**安置，不占整格（见 `punct_box`）。
    pub punct: bool,
    /// 列首句读被**悬挂**到网格右缘之外（禁则·避头尾）：不占列内格位，
    /// 正文首字因此仍钉在基准线上，绝不被往下挤。
    pub hung: bool,
    /// 加粗（来自 Markdown `**…**`）：光栅层用「同字微量偏移再落两笔」加厚。
    /// 不换字体面——系统宋体未必有 PostScript Bold 名，回退链一断整张卡就没了。
    pub bold: bool,
}

/// 一块矩形：填充（`stroke = None`）或描边（`stroke = Some(线宽)`）。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// `0xRRGGBBAA`。
    pub color: u32,
    pub stroke: Option<f32>,
}

/// 一条线（朱批引线 / 界行）。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLine {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    /// `0xRRGGBBAA`。
    pub color: u32,
    pub width: f32,
}

/// 朱印。
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedSeal {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub kind: ShareSeal,
    pub text: String,
}

/// 一张卡的全部绘制指令。
#[derive(Clone, Debug, Default)]
pub struct CardLayout {
    pub w: f32,
    pub h: f32,
    /// 底色 `0xRRGGBB`。
    pub bg: u32,
    /// 正文字体语义（离屏渲染据此挑系统 PostScript 字体）。
    pub font: ShareFont,
    pub rects: Vec<PlacedRect>,
    /// **界行**（列间竖细线，v1.61.0）：绘制序在 `rects` 之后、`chars` 之前
    /// ——线在下、字在上，绝不让界行压住墨迹。
    pub rules: Vec<PlacedLine>,
    pub lines: Vec<PlacedLine>,
    pub chars: Vec<PlacedChar>,
    pub seal: Option<PlacedSeal>,
}

/// 版式输入（调用方快照，渲染期只读）。
pub struct LayoutSpec<'a> {
    pub body: &'a [char],
    /// 与 `body` 等长：第 i 字是否被用户划了红线（朱批高亮）。
    pub red: &'a [bool],
    /// 与 `body` 等长：第 i 字的 Markdown 装饰位（标题 / 加粗）。
    ///
    /// **空切片 = 全部正文**——阅读器出图、分享卡等既有调用点根本不知道
    /// Markdown 的存在，它们传 `&[]`，版式与 v1.9.x 逐像素一致。
    pub md: &'a [MdMark],
    /// 眉批正文（已裁切；空 = 不画批注列）。
    pub note: &'a str,
    pub show_notes: bool,
    /// 落款：书名 · 章节名。
    pub sign: &'a str,
    pub show_sign: bool,
    pub date: &'a str,
    pub show_date: bool,
    pub columns: usize,
    pub bg: ShareBg,
    pub frame: ShareFrame,
    pub seal: ShareSeal,
    pub seal_text: &'a str,
    pub font: ShareFont,
    /// **出图时间戳**（v1.82.0）：一行极简小字画在**右下地脚带**里。
    /// 空串 = 不画。
    ///
    /// 它与 `date` 是两回事：`date` 走「版心内左下竖排列」，会占 `reserve`
    /// 竖带、动到正文网格宽；时间戳**一个数都不改**，只往地脚留白里补一行。
    /// 「不破坏版面」靠的就是这个分家。
    pub stamp: &'a str,
    pub w: f32,
    pub h: f32,
}

/// 行高 / 字格比：一格 = 字号 × 1.55（竖排古籍的行款，字与字之间有气口）。
pub const CELL_RATIO: f32 = 1.55;
/// 朱批高亮底的透明度（极淡朱砂，不盖字）。
pub const MARK_ALPHA: u8 = 0x26;
/// 朱批小字与正文的字号比。
pub const NOTE_FS_RATIO: f32 = 0.44;
/// 落款小字与正文的字号比。
pub const META_FS_RATIO: f32 = 0.40;
/// **出图时间戳的字号**（px，1080 基准，随卡宽缩放）：全卡最小的一档——
/// 水印只要「看得见、认得出」，绝不能抢正文。硬下限 9px：再小就成噪点了。
pub const STAMP_FS: f32 = 11.0;
/// 时间戳的**淡度**：`0` = 全纸色（看不见），`1` = 满墨色。
/// 0.5 是「比落款再淡一档」——落款是给人读的，时间戳只是个记号。
pub const STAMP_FADE: f32 = 0.50;
/// 朱批引线宽（px，按卡片宽缩放后的值）。
pub const LEAD_W: f32 = 1.0;

// ----------------------------------------------------------------------
// v1.61.0：版框（边栏）与界行（列间竖线）
//
// 古籍版刻的三件套是「边栏 + 界行 + 中缝」：边栏是四周的框线，界行是界在
// 列与列之间的竖细线。刻本里两者同色、粗细有别——边栏粗（朱丝栏 2px 上下）、
// 界行细（1px 且更淡），字始终压在最上层。
// ----------------------------------------------------------------------

/// **版框（边栏）线宽**（px，以 1080 宽为基准，按卡宽缩放）。
pub const FRAME_W: f32 = 2.0;
/// **界行（列间竖线）线宽**（px，1080 基准）——比版框细，绝不抢字。
pub const RULE_W: f32 = 1.0;
/// 版框墨色不透明度（`0xRRGGBBAA` 的低字节）：实心但不刺眼。
pub const FRAME_ALPHA: u32 = 0xC8;
/// **双边的内框**不透明度（v1.83.0）：比外框淡一档。
///
/// 照片上量的就是这个层次——外框浓黑压得住整版，内框与界行同属「退一档
/// 的灰」。两者同浓的话，双线看着像两圈一样重的框，反而比单线更乱。
pub const INNER_ALPHA: u32 = 0x96;
/// **双边内外框之间的气口**（px，1080 基准，随卡宽缩放）。
///
/// 教科书上的文武边栏约「外线一分、气口半分、内线半分」。2px 在 1080 卡上
/// 就是那个半分——再宽，双线会散成两个不相干的框。
pub const DOUBLE_GAP: f32 = 2.0;
/// 界行墨色不透明度：比版框**淡一档**，只作分列的骨架。
pub const RULE_ALPHA: u32 = 0x38;
/// 版框内缘 → 首字顶端的**气口**（px，1080 基准）：字顶不压线，也不留天头。
pub const TOP_GAP: f32 = 4.0;

/// **列首字的绝对基准线**（v1.59.0）：所有列的第一个字，顶边一律从它起算。
///
/// 取 0 是刻意的——基准线本身不带任何位移，位移只来自版框天头（见
/// [`top_baseline`]）。渲染层（GPUI 预览 / CoreGraphics 出图）都只读这一个
/// 常量，谁也不许就地加偏移：两处算法一旦分家，顶格就悄悄漂了。
pub const TOP_BASELINE: f32 = 0.0;

/// 列首基准线的**唯一取用口径**：`TOP_BASELINE + 版框内缘 + 气口`。
///
/// v1.61.0 起**不再垂直居中**：正文块的上沿与版框内缘（框线的下缘）只隔一个
/// [`TOP_GAP`]——第一个字的最高点正好卡在框线下方的起点上，不压线、不悬空。
/// 这是「首行顶格」与「版框」的视觉联动，唯一允许的天头位移就是这一段气口。
pub fn top_baseline(inner_top: f32, scale: f32) -> f32 {
    TOP_BASELINE + inner_top + TOP_GAP * scale
}

/// **悬挂沟**：网格右缘外侧留出的一条竖带（字格比例），专供列首句读挂靠。
///
/// 有了它，句读被推出列外时**不会画出画布 / 画到版框上**，正文也照旧顶格。
pub const HANG_GUTTER: f32 = 0.62;
/// 悬挂句读与网格右缘之间的横向间隙（字格比例）。
pub const HANG_GAP: f32 = 0.08;

/// **列首标点**（逗号 / 句号等）挂到版框**外侧**（最右列之右）时的横向外探量
/// （字格比例）。标点被挂出去后**不再占列内格位**，正文首字因此仍钉在
/// [`TOP_BASELINE`] 上——禁则（避头尾）绝不牺牲顶格。
pub const HEAD_PUNCT_OUTSET: f32 = 0.30;
/// 多枚列首标点连排时，在框外的纵向错开量（字格比例），避免叠成一坨。
pub const HEAD_PUNCT_STEP: f32 = 0.60;

/// 竖排里需要「靠右上安置」的标点（占半格、不顶格）。
pub fn is_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '、' | '；' | '：' | '！' | '？' | '「' | '」' | '『' | '』'
            | '《' | '》' | '〈' | '〉' | '（' | '）' | '—' | '…' | ',' | '.' | ';'
            | ':' | '!' | '?' | '(' | ')' | '　' | ' '
    )
}

/// 标点的落位盒：字格**右上**象限，字号六成。
///
/// 被悬挂的句读（[`PlacedChar::hung`]）走另一路：它的 `x` 已经是「网格右缘
/// 之外」的最终落点，故不再加 `0.42` 格的内偏移，只留 [`HANG_GAP`] 的气口。
pub fn punct_box(p: &PlacedChar) -> (f32, f32, f32) {
    if p.hung {
        (p.x + p.cell * 0.02, p.y + p.cell * 0.10, p.fs * 0.62)
    } else {
        // 竖排古籍的句读挂在字的右上方，不占整格——正文仍按整格推进。
        (p.x + p.cell * 0.42, p.y + p.cell * 0.04, p.fs * 0.62)
    }
}

// ----------------------------------------------------------------------
// Markdown 清洗器（v1.20.0）——「源码编辑 + 预览解析」的解析那一半
//
// 笺校工作台允许直接导入 .md：右侧编辑器**原样显示源码**（`#`、`**`、`---`
// 都在，用户改的是原文），只有竖排预览出图前才过这道清洗器。这样校对精度
// 不受影响——源码里第几行第几字，用户眼睛看得见、手也改得到；成图里则是
// 干净的竖排文字。
//
// 清洗只做五件事（刻意不做完整 Markdown 语义，古籍稿用不到）：
//   ① 块级标记整行丢：`---` / `***` / `___` / `===` 分隔线；
//   ② 块级前缀剥掉：`#`（并记成标题）、`>` 引用、`- ` / `1. ` 列表；
//   ③ 行内标记剥掉：`**` / `__`（记成加粗）、`*` / `_` 斜体、`` ` `` 代码；
//   ④ 链接 / 图片只留文字：`[文](url)` → `文`；
//   ⑤ 表格竖线、行内 HTML 标签一并剔掉。
//
// 全部按 `char` 走，绝不用字节下标切 CJK——那是 v1.9.18 崩过一回的老坑。
// ----------------------------------------------------------------------

/// 标题字相对正文的**字号倍率**（`#` 行）。
///
/// 只放大字号、**不动字格**：竖排网格的列宽 / 行数 / 界行全由 `cell` 定，
/// 一格一变大，整版网格立刻散架。1.28 是「一眼看出是标题、又不挤到邻格」
/// 的那一档（放大后字身约占格高 0.71，上下仍有余气）。
pub const HEAD_FS_RATIO: f32 = 1.28;
/// 放大后的字号上限（字格比例）：即便标题倍率被调大，墨迹也绝不撑破本格。
pub const HEAD_CELL_FILL: f32 = 0.86;

/// Markdown 清洗结果：只剩「要印出来的字」，每字配一枚装饰位。
#[derive(Clone, Debug, Default)]
pub struct CleanMd {
    pub body: Vec<char>,
    pub md: Vec<MdMark>,
}

impl CleanMd {
    /// 清洗后的纯文本（不含任何 Markdown 记号）。
    pub fn text(&self) -> String {
        self.body.iter().collect()
    }
}

/// 把 Markdown 源码洗成「可竖排的纯文字 + 装饰位」。
///
/// 幂等：拿清洗结果再洗一遍，字一个不变（回归测试钉着）。
pub fn clean_md(src: &str) -> CleanMd {
    let mut out = CleanMd::default();
    for raw in src.split('\n') {
        let line = raw.trim_end_matches('\r');
        let mut t = line.trim();
        if t.is_empty() {
            continue; // 空行不留空白格，段落自然接排（竖排里换行只是排版噪声）
        }
        if is_md_rule(t) {
            continue; // ① 分隔线：整行丢弃，绝不印成一道横杠
        }
        // ② 块级前缀：可叠加（`> - 文字` / `> 1. 文字`），最多剥四层防死循环。
        let mut guard = 0;
        loop {
            let before = t;
            if let Some(rest) = t.strip_prefix('>') {
                t = rest.trim_start();
            }
            if let Some(rest) = strip_list_marker(t) {
                t = rest;
            }
            if t == before || guard >= 4 {
                break;
            }
            guard += 1;
        }
        if t.is_empty() {
            continue;
        }
        // ②b ATX 标题：`#`～`######` + 空白开头才认（`#tag` 不是标题）。
        let mut head = false;
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && hashes <= 6 {
            let after = &t[hashes..]; // '#' 是 ASCII：字节下标恰好等于个数
            if after.is_empty() || after.starts_with(' ') || after.starts_with('\t') {
                head = true;
                // 收尾的 `#`（`## 标题 ##`）一并剥掉。
                t = after.trim().trim_end_matches('#').trim_end();
            }
        }
        if t.is_empty() {
            continue;
        }
        inline_md_into(t, head, &mut out);
    }
    out
}

/// 整行是否是一条 Markdown 分隔线（`---` / `***` / `___` / `===`，允许空格）。
fn is_md_rule(t: &str) -> bool {
    let cs: Vec<char> = t.chars().filter(|c| !c.is_whitespace()).collect();
    if cs.len() < 3 {
        return false;
    }
    let first = cs[0];
    if !matches!(first, '-' | '*' | '_' | '=') {
        return false;
    }
    cs.iter().all(|c| *c == first)
}

/// 剥列表前缀（`- ` / `* ` / `+ ` / `1. ` / `1) ` / `1、`），返回余下内容。
fn strip_list_marker(t: &str) -> Option<&str> {
    if let Some(c) = t.chars().next() {
        if matches!(c, '-' | '*' | '+') {
            let rest = &t[c.len_utf8()..];
            return rest.strip_prefix(' ').map(|r| r.trim_start());
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits <= 3 {
        let rest = &t[digits..]; // ASCII 数字：字节数 == 个数
        if let Some(c) = rest.chars().next() {
            if matches!(c, '.' | ')' | '、') {
                let tail = &rest[c.len_utf8()..];
                if let Some(r) = tail.strip_prefix(' ') {
                    return Some(r.trim_start());
                }
            }
        }
    }
    None
}

/// 剥行内标记，把干净的字连同装饰位推进 `out`。
fn inline_md_into(line: &str, head: bool, out: &mut CleanMd) {
    let cs: Vec<char> = line.chars().collect();
    let mut buf: Vec<(char, bool)> = Vec::with_capacity(cs.len());
    let mut i = 0usize;
    let mut bold = false;
    while i < cs.len() {
        let c = cs[i];
        // 反斜杠转义：`\*` 出 `*`，`\\` 出 `\`
        if c == '\\' && i + 1 < cs.len() {
            buf.push((cs[i + 1], bold));
            i += 2;
            continue;
        }
        // 行内代码：反引号本身丢，里面的字照印
        if c == '`' {
            i += 1;
            continue;
        }
        // ③ 加粗 `**` / `__`：成对翻转，跨行自然复位（本函数一次只吃一行）
        if (c == '*' || c == '_') && cs.get(i + 1) == Some(&c) {
            bold = !bold;
            i += 2;
            continue;
        }
        // ③b 斜体 `*`：记号丢、字留
        if c == '*' {
            i += 1;
            continue;
        }
        // ③c 下划线只在「两侧不都是字母数字」时当标记（保住 `foo_bar`）
        if c == '_' {
            let prev = if i > 0 { cs[i - 1] } else { ' ' };
            let next = cs.get(i + 1).copied().unwrap_or(' ');
            if !(prev.is_alphanumeric() && next.is_alphanumeric()) {
                i += 1;
                continue;
            }
        }
        // ④ 图片 `![alt](url)`：`!` 丢掉，剩下的 `[alt](url)` 交给下一轮
        if c == '!' && cs.get(i + 1) == Some(&'[') {
            i += 1;
            continue;
        }
        if c == '[' {
            if let Some((txt, next)) = take_bracket(&cs, i) {
                for ch in txt.chars() {
                    buf.push((ch, bold));
                }
                i = next;
                continue;
            }
        }
        // ⑤ 表格竖线当空格；行内 HTML 标签整段丢弃
        if c == '|' {
            buf.push((' ', bold));
            i += 1;
            continue;
        }
        if c == '<' {
            if let Some(j) = cs[i..].iter().position(|c| *c == '>') {
                i += j + 1;
                continue;
            }
        }
        buf.push((c, bold));
        i += 1;
    }
    for (ch, b) in buf {
        out.body.push(ch);
        out.md.push(MdMark { head, bold: b });
    }
}

/// `[文字](目标)` → `("文字", 目标之后的下标)`；不是链接就返回 `None`。
fn take_bracket(cs: &[char], i: usize) -> Option<(String, usize)> {
    let close = cs[i..].iter().position(|c| *c == ']')? + i;
    if close == i {
        return None; // `[]` 空文字：不是链接，交给后面照常印
    }
    let text: String = cs[i + 1..close].iter().collect();
    let mut next = close + 1;
    if cs.get(next) == Some(&'(') {
        if let Some(j) = cs[next..].iter().position(|c| *c == ')') {
            next += j + 1;
        }
    }
    Some((text, next))
}

/// 排一张卡：底色 → 界框 → 竖排正文（含高亮 / 批注列 / 引线）→ 落款 → 朱印。
pub fn layout(s: &LayoutSpec) -> CardLayout {
    let p = palette(s.bg);
    let w = if s.w.is_finite() { s.w } else { 1080.0 }.max(240.0);
    let h = if s.h.is_finite() { s.h } else { 1920.0 }.max(360.0);
    // 缩放：所有线宽 / 气口都以 1080 宽为基准，小卡同步收细，绝不糊成粗杠。
    let scale = (w / 1080.0).clamp(0.5, 2.0);
    // 版框外缘留白：左右按宽、上下按高各取一次（竖长卡的天头地脚要更从容）。
    let pad_x = (w * 0.075).clamp(18.0, 46.0);
    let pad_y = (h * 0.035).clamp(18.0, 72.0);
    let fw = FRAME_W * scale;

    let mut out = CardLayout {
        w,
        h,
        bg: p.bg,
        font: s.font,
        ..Default::default()
    };

    // ---- 版框（边栏）：一整圈矩形框线，朱丝栏取朱砂、乌丝栏取墨 ----
    //
    // v1.61.0：旧制是「内外两条细线」，框感太弱且不与首行咬合。现在只画一
    // 圈实心粗线（1080 卡上 2px），且**描边骑在路径上**（线宽一半在内、一半
    // 在外），故框的「内缘」= `inset + fw/2`——首行基准线就认这条内缘。
    //
    // v1.83.0：`ShareFrame::Double` 把「双线」请回来，但**不是**旧制那两条
    // 同粗的细线——外粗内细（文武边栏），中间隔一道气口。刻本的层次全靠
    // 这个粗细差，不是靠调淡墨色。
    let border = frame_ink(s.frame, &p);
    let double = s.frame == ShareFrame::Double;
    // 内细线宽（外框的一半）与气口（1080 基准，随卡宽缩放）。
    let fi = (fw * 0.5).max(0.6);
    let dg = if double { DOUBLE_GAP * scale } else { 0.0 };
    if let Some(ink) = border {
        out.rects.push(PlacedRect {
            x: pad_x,
            y: pad_y,
            w: (w - pad_x * 2.0).max(20.0),
            h: (h - pad_y * 2.0).max(20.0),
            color: (ink << 8) | FRAME_ALPHA,
            stroke: Some(fw),
        });
        if double {
            // 内框：整圈细线，向内让 `fw/2 + dg`（外框内缘 + 气口）。
            let o = fw * 0.5 + dg;
            out.rects.push(PlacedRect {
                x: pad_x + o,
                y: pad_y + o,
                w: (w - (pad_x + o) * 2.0).max(20.0),
                h: (h - (pad_y + o) * 2.0).max(20.0),
                color: (ink << 8) | INNER_ALPHA,
                stroke: Some(fi),
            });
        }
    }

    // ---- 版框内的工作区（版心）----
    // 双边时再往里退一条内细线：正文绝不压在内框上。
    let frame_pad = fw * 0.5 + if double { dg + fi } else { 0.0 };
    let inner_l = pad_x + frame_pad;
    let inner_r = (w - pad_x - frame_pad).max(inner_l + 40.0);
    let inner_t = pad_y + frame_pad;
    let inner_b = (h - pad_y - frame_pad).max(inner_t + 40.0);

    let n = s.body.len();
    let cols_max = norm_columns(s.columns).max(1);
    // 开「展示朱批」且有批注：最左让出一列给批注（竖排右起，左为后进）。
    let want_note = s.show_notes && !s.note.is_empty() && n > 0;
    let note_cols = if want_note { 1 } else { 0 };

    // 字格：先按「满列铺开」取，装不下就整体缩字号（绝不加列、绝不溢出）。
    //
    // 分母多算三段：批注列 + 右侧**悬挂沟**（列首句读挂出去的地方）。悬挂沟
    // 先按字格比例占位，句读因此绝不会画到版框上、更不会出画布。
    let denom = (cols_max + note_cols) as f32 + HANG_GUTTER;
    // 左下角落款 / 朱印要占一条竖带，先从版心宽里扣掉（否则满列时字压落款）。
    let meta_on = (s.show_sign && !s.sign.is_empty()) || (s.show_date && !s.date.is_empty());
    let meta_cell_of = |cell: f32| -> f32 {
        ((cell / CELL_RATIO) * META_FS_RATIO).clamp(11.0, 26.0) * CELL_RATIO
    };
    let reserve = if meta_on {
        meta_cell_of((inner_r - inner_l) / denom) * 2.0 + 14.0 * scale
    } else {
        0.0
    };
    let grid_l = inner_l + reserve; // 正文网格的左界（版心内）
    let avail_w = (inner_r - grid_l).max(60.0);
    let mut cell = avail_w / denom;
    let mut fs = (cell / CELL_RATIO).max(8.0);
    let mut rows = ((inner_b - top_baseline(inner_t, scale)) / cell).floor().max(3.0);
    let mut guard = 0;
    while (cols_max as f32 * rows) < n as f32 && fs > 9.0 && guard < 40 {
        fs = (fs * 0.92).max(9.0);
        cell = fs * CELL_RATIO;
        rows = ((inner_b - top_baseline(inner_t, scale)) / cell).floor().max(3.0);
        guard += 1;
    }

    // 容量上限：字太多（超长选区）就按能装下的量截断——绝不为了塞满而把
    // 行数撑爆、把列首顶出纸外（`rows_used` 因此永不超过 `rows`）。
    let cap = cols_max * rows.max(1.0) as usize;
    let shown = n.min(cap);
    let cols_used = if shown == 0 {
        0
    } else {
        ((shown as f32 / rows).ceil() as usize).clamp(1, cols_max)
    };
    let rows_used = if cols_used == 0 {
        0
    } else {
        (shown as f32 / cols_used as f32).ceil() as usize
    };
    let block_w = cols_used as f32 * cell;
    // 居中：批注列留在正文块的左侧，右侧再让出悬挂沟，三者一并居中。
    let span = note_cols as f32 * cell + block_w + HANG_GUTTER * cell;
    let x0 = grid_l + ((avail_w - span) * 0.5).max(0.0) + note_cols as f32 * cell;
    // 铁律 1 + 版框联动：列首 y 恒等于 y0 —— 每列都从这里起笔，绝不因列长而
    // 错动；y0 本身紧贴**版框内缘**（`top_baseline` 是它的唯一口径），故第一
    // 个字的最高点正好卡在框线下方的起点上：不压线、也不留天头空档。
    let y0 = top_baseline(inner_t, scale);

    // ---- 界行（列间竖细线）：通版心高，界在列与列之间 ----
    //
    // 刻本的界行只画**内部列界**（最左 / 最右由版框自己收口），故条数是
    // 「总列数 − 1」；线色比版框淡一档、线宽 1px，绝不抢墨迹的视觉焦点。
    if border.is_some() && cols_used > 0 {
        let grid_left = x0 - note_cols as f32 * cell; // 最左一列（含批注列）的左缘
        let total = cols_used + note_cols;
        let rule_c = ((border.unwrap_or(p.ink)) << 8) | RULE_ALPHA;
        for k in 1..total {
            let x = grid_left + k as f32 * cell;
            out.rules.push(PlacedLine {
                x1: x,
                y1: inner_t,
                x2: x,
                y2: inner_b,
                color: rule_c,
                width: RULE_W * scale,
            });
        }
    }

    // ---- 正文：从右向左，逐格落字 ----
    let mut first_marked: Option<usize> = None;
    // 列内占格游标：与「源串下标 i」分家——列首标点被挂出框外后**不占格位**，
    // 故它不推进游标，正文首字因此仍落在 row 0（＝基准线）。
    let (mut col, mut row, mut hung) = (0usize, 0usize, 0usize);
    for (i, &ch) in s.body.iter().enumerate().take(shown) {
        // 禁则·列首标点悬挂（v1.59.0 / v1.61.0 收进悬挂沟）：这一格若落在列首
        // 且是句读，就**不占格**，改挂到网格右缘之外的悬挂沟里（X 轴偏移），
        // 正文照旧顶格——绝不为让标点而把后面的字往下挤。
        let head_punct = row == 0 && is_punct(ch);
        let x = if head_punct {
            x0 + block_w + cell * HANG_GAP
        } else {
            x0 + (cols_used as f32 - 1.0 - col as f32) * cell
        };
        let y = if head_punct {
            y0 + hung as f32 * cell * HEAD_PUNCT_STEP
        } else {
            y0 + row as f32 * cell
        };
        if head_punct {
            hung += 1;
        } else {
            row += 1;
            if row >= rows_used.max(1) {
                row = 0;
                col += 1;
                hung = 0;
            }
        }
        let marked = s.red.get(i).copied().unwrap_or(false);
        if marked && first_marked.is_none() {
            first_marked = Some(i);
        }
        // 高亮底：极淡朱砂，垫在字下（先于字绘制，见 `rects` 的顺序保证）。
        if marked {
            out.rects.push(PlacedRect {
                x,
                y,
                w: cell,
                h: cell,
                color: ((p.accent) << 8) | MARK_ALPHA as u32,
                stroke: None,
            });
        }
        // 朱批字本身染朱砂（红线 + 红字，两重标记）。
        let color = if marked { p.accent } else { p.ink };
        // Markdown 装饰位：标题放大字号（**字格不动**，网格因此一点不变），
        // 加粗只记旗子、由光栅层落笔加厚。空切片时 `0` = 正文，行为与旧版一致。
        let mark = s.md.get(i).copied().unwrap_or_default();
        let ch_fs = if mark.head {
            (fs * HEAD_FS_RATIO).min(cell * HEAD_CELL_FILL)
        } else {
            fs
        };
        out.chars.push(PlacedChar {
            ch,
            x,
            y,
            cell,
            fs: ch_fs,
            color,
            punct: is_punct(ch),
            hung: head_punct,
            bold: mark.bold,
        });
    }

    // ---- 朱批列（杀手锏）：正文块左侧一列小字 + 极细朱砂引线 ----
    if want_note {
        let note_fs = (fs * NOTE_FS_RATIO).max(9.0);
        let note_cell = note_fs * CELL_RATIO;
        let nx = x0 - cell; // 最左一列的左邻
        let take = (((inner_b - y0) * 0.62) / note_cell).floor() as usize;
        let mut ny = y0;
        for (i, ch) in s.note.chars().take(take).enumerate() {
            out.chars.push(PlacedChar {
                ch,
                x: nx,
                y: ny,
                cell: note_cell,
                fs: note_fs,
                color: p.accent,
                punct: is_punct(ch),
                hung: false,
                bold: false,
            });
            ny += note_cell;
            let _ = i;
        }
        // 引线：被批的第一个字（无划线则取**最贴近批注列的首字**）→ 批注首字。
        //
        // 无划线时不能取全篇首字（它在最右一列！）：那根引线会横穿整片正文，
        // 把字划得乱七八糟。改取最左一列的首字，引线只在批注列与正文之间。
        let anchor = first_marked.unwrap_or_else(|| {
            let left_col = cols_used.saturating_sub(1);
            (left_col * rows_used).min(shown.saturating_sub(1))
        });
        let acol = anchor / rows_used.max(1);
        let arow = anchor % rows_used.max(1);
        let ax = x0 + (cols_used as f32 - 1.0 - acol as f32) * cell;
        let ay = y0 + arow as f32 * cell + cell * 0.5;
        out.lines.push(PlacedLine {
            x1: ax,
            y1: ay,
            x2: nx + note_cell,
            y2: y0 + note_cell * 0.5,
            color: (p.accent << 8) | 0x9A,
            width: LEAD_W * (w / 1080.0).max(0.6),
        });
    }

    // ---- 落款与朱印：版心内左下角（古籍里「版框内」的题识位）----
    //
    // v1.61.0：整体收进版框内缘，且左侧已预留 `reserve` 竖带，落款与正文
    // 网格永不叠字。
    let seal_size = (w * 0.085).clamp(28.0, 44.0);
    let inset = inner_l + 6.0 * scale;
    let meta_fs = (fs * META_FS_RATIO).clamp(11.0, 26.0);
    let meta_cell = meta_fs * CELL_RATIO;

    // 竖排两列：书名·章节在右（靠内），日期在左；自内而外排，读序自右向左。
    let mut cols_meta: Vec<(f32, &str)> = Vec::new();
    if s.show_sign && !s.sign.is_empty() {
        cols_meta.push((0.0, s.sign));
    }
    if s.show_date && !s.date.is_empty() {
        cols_meta.push((0.0, s.date));
    }
    // 印章上方留 12px 气口给落款，落款末字与印面不粘。
    let base_bottom = inner_b - 6.0 * scale - seal_size - 12.0 * scale;
    // 可用高：从「首行基准线」到落款底——落款按它截字，绝不溢出版框。
    let avail = (base_bottom - y0).max(meta_cell);
    let take_meta = (avail / meta_cell).floor() as usize;
    for (k, (_x, text)) in cols_meta.iter().enumerate() {
        let cx = inset + (cols_meta.len() as f32 - 1.0 - k as f32) * meta_cell;
        let chars: Vec<char> = text.chars().take(take_meta).collect();
        let block = chars.len() as f32 * meta_cell;
        let top = (base_bottom - block).max(y0);
        for (i, ch) in chars.iter().enumerate() {
            out.chars.push(PlacedChar {
                ch: *ch,
                x: cx,
                y: top + i as f32 * meta_cell,
                cell: meta_cell,
                fs: meta_fs,
                color: p.meta,
                punct: is_punct(*ch),
                hung: false,
                bold: false,
            });
        }
    }

    out.seal = Some(PlacedSeal {
        x: inset,
        y: (inner_b - 6.0 * scale - seal_size).max(y0),
        size: seal_size,
        kind: s.seal,
        text: s.seal_text.to_string(),
    });

    // ---- 出图时间戳（v1.82.0）：右下**地脚带**里一行极简小字 ----
    //
    // 位置刻意选在版框**之外**的下留白（地脚）：那一带是空的，落一行小字
    // 既不压正文、也不用给正文让 `reserve`，版面一个数都不用改——这就是
    // 「不破坏版面」的落法。横排（竖排网格里塞一行横字太怪），右缘与版框
    // 外缘齐平，颜色比落款再淡一档。
    if !s.stamp.is_empty() {
        let cs: Vec<char> = s.stamp.chars().collect();
        let sfs = (STAMP_FS * scale).clamp(9.0, 18.0);
        let total = cs.len() as f32 * sfs;
        // 右缘：版框外缘（pad_x 处）往里收一个气口；左界兜住，绝不越出版心。
        let right = (w - pad_x - 2.0 * scale).max(inner_l + total);
        // 地脚带中心：版框下缘（h - pad_y）到画布底之间。
        let cy = h - pad_y * 0.5;
        let color = fade(p.meta, p.bg, STAMP_FADE);
        for (i, ch) in cs.iter().enumerate() {
            out.chars.push(PlacedChar {
                ch: *ch,
                x: right - total + i as f32 * sfs,
                y: cy - sfs * 0.62,
                cell: sfs,
                fs: sfs,
                color,
                punct: false,
                hung: false,
                bold: false,
            });
        }
    }

    out
}

/// 把 `fg` 往 `bg` 方向掺淡（`ink` = 1 满色、0 全底），返回 `0xRRGGBB`。
///
/// 不走 alpha：字是逐个 `draw_glyphs` 落的，填充色里带 alpha 会在字与字的
/// 重叠处叠出深浅不一的斑；直接算一个实色的中间色最稳。
fn fade(fg: u32, bg: u32, ink: f32) -> u32 {
    let k = ink.clamp(0.0, 1.0);
    let mut out = 0u32;
    for shift in (0..24).step_by(8) {
        let a = ((fg >> shift) & 0xFF) as f32;
        let b = ((bg >> shift) & 0xFF) as f32;
        let v = (a * k + b * (1.0 - k)).round().clamp(0.0, 255.0) as u32;
        out |= v << shift;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1080.0;
    const H: f32 = 1920.0;

    /// 带时间戳的 spec（其余与 [`spec`] 同）。
    fn spec_stamp<'a>(body: &'a [char], red: &'a [bool], stamp: &'a str) -> LayoutSpec<'a> {
        let mut s = spec(body, red);
        s.stamp = stamp;
        s
    }

    fn spec<'a>(body: &'a [char], red: &'a [bool]) -> LayoutSpec<'a> {
        LayoutSpec {
            body,
            red,
            md: &[],
            note: "",
            show_notes: false,
            sign: "道德經 · 第一章",
            show_sign: true,
            date: "二〇二六年十月",
            show_date: true,
            columns: 12,
            bg: ShareBg::Xuan,
            frame: ShareFrame::Red,
            seal: ShareSeal::Twin,
            seal_text: "道德",
            font: ShareFont::Song,
            stamp: "",
            w: W,
            h: H,
        }
    }

    /// 铁律 1：每一列的第一个字，y 必须绝对相同（首行顶格）。
    #[test]
    fn stamp_lands_in_the_foot_margin_and_never_moves_the_grid() {
        let body: Vec<char> = "道可道非常道名可名非常名无名天地之始".chars().collect();
        let red = vec![false; body.len()];
        let a = layout(&spec_stamp(&body, &red, ""));
        let b = layout(&spec_stamp(&body, &red, "丙午年季秋下浣"));
        // ① 正文一个字都没挪：版画指令**逐条相同**，只在末尾多出戳那几个字。
        //    这就是「不破坏版面」——戳不进正文网格、不占 reserve 竖带。
        assert_eq!(
            b.chars.len(),
            a.chars.len() + 7,
            "戳应是纯增量：{} vs {}",
            b.chars.len(),
            a.chars.len()
        );
        assert_eq!(&b.chars[..a.chars.len()], &a.chars[..], "正文被戳挪动了");
        // ② 戳落在**版框之外**的下留白（地脚带）里，绝不压正文。
        let pad_y = (H * 0.035).clamp(18.0, 72.0);
        let frame_bottom = H - pad_y;
        for c in &b.chars[a.chars.len()..] {
            assert!(c.y > frame_bottom, "戳压进版框了：y={}", c.y);
            assert!(c.y + c.fs * 1.3 < H, "戳掉出画布了：y={}", c.y);
            assert!(c.x > 0.0 && c.x + c.fs <= W, "戳横向越界：x={}", c.x);
        }
        // ③ 右对齐：最后一个字的右缘贴住版框外缘（差一个气口）。
        let last = b.chars.last().unwrap();
        let pad_x = (W * 0.075).clamp(18.0, 46.0);
        assert!(
            (W - pad_x - (last.x + last.fs)).abs() < 6.0,
            "戳没贴住右下角：右缘 {} / 版框外缘 {}",
            last.x + last.fs,
            W - pad_x
        );
        // ④ 字号是全卡最小的一档，且比正文小得多。
        let body_fs = a.chars[0].fs;
        assert!(last.fs < body_fs, "戳不该比正文还大：{} vs {}", last.fs, body_fs);
        assert!(last.fs <= STAMP_FS + 0.01, "戳超过最小档：{}", last.fs);
        // ⑤ 颜色比落款淡（往纸色方向掺过），不能是满墨。
        let p = crate::share::palette(ShareBg::Xuan);
        assert_ne!(last.color, p.meta, "戳该比落款淡");
        let lum = |c: u32| -> i32 {
            (((c >> 16) & 0xFF) as i32 * 3 + ((c >> 8) & 0xFF) as i32 * 6 + (c & 0xFF) as i32) / 10
        };
        assert!(
            lum(last.color) > lum(p.meta),
            "戳应比落款亮（更淡）：{} vs {}",
            lum(last.color),
            lum(p.meta)
        );
    }

    #[test]
    fn every_column_starts_on_the_same_baseline() {
        // 字数要能撑出多列（12 列档下每列约 24 字），否则无从比较列首。
        let body: Vec<char> = "道可道非常道名可名非常名無名天地之始有名萬物之母故常無欲以觀其妙常有欲以觀其徼此兩者同出而異名同謂之玄玄之又玄眾妙之門"
            .chars()
            .collect();
        let red = vec![false; body.len()];
        let l = layout(&spec(&body, &red));
        let body_chars: Vec<&PlacedChar> =
            l.chars.iter().filter(|c| c.fs > 30.0).collect(); // 排除落款小字
        assert!(!body_chars.is_empty());
        // 按 x 分组（一列一个 x），每组最小 y 必须等于该列的列首 y0。
        let mut xs: Vec<f32> = body_chars.iter().map(|c| c.x).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert!(xs.len() > 1, "应当排出多列：{:?}", xs);
        let heads: Vec<f32> = xs
            .iter()
            .map(|x| {
                body_chars
                    .iter()
                    .filter(|c| (c.x - *x).abs() < 0.01)
                    .map(|c| c.y)
                    .fold(f32::MAX, f32::min)
            })
            .collect();
        let first = heads[0];
        for yy in &heads {
            assert!(
                (yy - first).abs() < 0.01,
                "列首未顶格：{heads:?}（应全部相等）"
            );
        }
    }

    /// 从右向左：第 0 个字必在最右一列。
    #[test]
    fn first_char_sits_in_the_rightmost_column() {
        let body: Vec<char> = "道可道非常道名可名非常名".chars().collect();
        let red = vec![false; body.len()];
        let l = layout(&spec(&body, &red));
        let max_x = l.chars.iter().map(|c| c.x).fold(f32::MIN, f32::max);
        assert!((l.chars[0].x - max_x).abs() < 0.01, "首字不在最右列");
    }

    /// 朱批高亮：划线的字下面垫一块极淡朱砂，未划线的没有。
    #[test]
    fn marked_chars_get_a_faint_cinnabar_ground() {
        let body: Vec<char> = "道可道非常道".chars().collect();
        let red = vec![true, false, false, true, false, false];
        let l = layout(&spec(&body, &red));
        // 高亮块数 = 划线的字数（落款 / 批注不产生高亮）。
        assert_eq!(l.rects.iter().filter(|r| r.stroke.is_none()).count(), 2);
        // 划线的字染朱砂色（accent = 0xB03A2E，宣纸黄档）。
        let accented = l.chars.iter().filter(|c| c.color == 0xB03A2E).count();
        assert_eq!(accented, 2);
    }

    /// 开「展示朱批」：多出一列批注小字 + 一条引线，且批注列在正文左侧。
    #[test]
    fn note_column_and_leader_line_appear_when_enabled() {
        let body: Vec<char> = "道可道非常道名可名非常名".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.note = "此句疑有衍文";
        s.show_notes = true;
        let l = layout(&s);
        assert_eq!(l.lines.len(), 1, "应恰好一条引线");
        // 批注小字：朱砂色 + 字号明显小于正文（落款是灰色，不在此列）。
        let body_fs = l.chars[0].fs;
        let note_chars: Vec<&PlacedChar> = l
            .chars
            .iter()
            .filter(|c| c.color == 0xB03A2E && c.fs < body_fs * 0.6)
            .collect();
        assert!(note_chars.len() >= 5, "批注列字数不足：{}", note_chars.len());
    }

    /// 关掉批注：一条引线都不画。
    #[test]
    fn no_leader_line_when_notes_are_off() {
        let body: Vec<char> = "道可道非常道".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.note = "此句疑有衍文";
        s.show_notes = false;
        let l = layout(&s);
        assert!(l.lines.is_empty());
    }

    /// 无边框：一层面框都不画（界框属于 `rects` 的 stroke 项）。
    /// **四周双边**（v1.83.0）：外框一粗、内框一细，中间隔一道气口；
    /// 正文退到内框以里，绝不压线。
    ///
    /// 钉死四件事：① 确实画出两条（不是一圈粗线冒充双线）；② 外粗内细；
    /// ③ 内框比外框淡（照片上的层次）；④ 首字落在内框以里。
    /// 另加一条反向闸：非双边的卡仍只画一条——老版面一个像素都不许变。
    #[test]
    fn double_frame_draws_an_outer_and_an_inner_line() {
        let body: Vec<char> = "道可道非常道名可名非常名無名天地之始".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.frame = ShareFrame::Double;
        s.bg = ShareBg::Keben;
        let d = layout(&s);
        // 只有「框线」带 stroke；引线 / 朱批是实心块，用 stroke 挑出来。
        let frames: Vec<&PlacedRect> =
            d.rects.iter().filter(|r| r.stroke.is_some()).collect();
        assert_eq!(frames.len(), 2, "双边应画两条框线，实为 {}", frames.len());
        let (outer, inner) = (frames[0], frames[1]);
        let (wo, wi) = (outer.stroke.unwrap(), inner.stroke.unwrap());
        assert!(wo > wi, "必须外粗内细：外 {wo} / 内 {wi}");
        // 两条线不重叠：内框整圈向内让「外框半宽 + 气口」。
        let off = inner.x - outer.x;
        assert!(
            off >= wo * 0.5 + 1.0,
            "内外框贴在一起了（分不出双线）：偏移 {off} / 外框半宽 {}",
            wo * 0.5
        );
        assert!(
            (inner.y - outer.y - off).abs() < 0.01,
            "内框四边让位不均：x {off} / y {}",
            inner.y - outer.y
        );
        // 内框比外框淡一档（层次：外框最重）。
        assert!(
            (inner.color & 0xFF) < (outer.color & 0xFF),
            "内框该比外框淡：{:02X} / {:02X}",
            inner.color & 0xFF,
            outer.color & 0xFF
        );
        // 首字必须落在内框以里——双边不是画着看的，正文得给它让位。
        let first = d.chars.first().expect("应有正文");
        assert!(
            first.y >= inner.y,
            "首字压到内框上了：{} < {}",
            first.y,
            inner.y
        );
        assert!(
            first.x + first.cell <= inner.x + inner.w + 0.01,
            "首字压到右内框：{} > {}",
            first.x + first.cell,
            inner.x + inner.w
        );
        // 反向闸：其余三档仍只画一条。
        for f in [ShareFrame::Red, ShareFrame::Black] {
            let mut one = spec(&body, &red);
            one.frame = f;
            let n = layout(&one)
                .rects
                .iter()
                .filter(|r| r.stroke.is_some())
                .count();
            assert_eq!(n, 1, "{f:?} 不该画成双线");
        }
    }

    #[test]
    fn frameless_card_draws_no_border() {
        let body: Vec<char> = "道可道".chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.frame = ShareFrame::None;
        let l = layout(&s);
        assert!(l.rects.iter().all(|r| r.stroke.is_none()));
    }

    /// 防崩：空正文 / 超大字号请求 / 长文都不 panic，且字格为正。
    #[test]
    fn degenerate_inputs_never_panic_and_never_overflow() {
        let red: Vec<bool> = Vec::new();
        let l = layout(&spec(&[], &red));
        assert_eq!(l.chars.iter().filter(|c| c.fs > 30.0).count(), 0);
        assert!(l.seal.is_some());

        let body: Vec<char> = "道".repeat(4000).chars().collect();
        let red = vec![false; body.len()];
        let mut s = spec(&body, &red);
        s.w = 300.0;
        s.h = 500.0;
        let l = layout(&s);
        for c in &l.chars {
            assert!(c.cell > 0.0 && c.fs > 0.0);
            assert!(c.x >= -1.0 && c.x <= l.w + 1.0, "字溢出：{}", c.x);
            assert!(c.y >= -1.0 && c.y <= l.h + 1.0, "字溢出：{}", c.y);
        }
    }

    /// 标点靠右上安置，且不占整格（仍按整格推进）。
    #[test]
    fn punctuation_hangs_in_the_upper_right() {
        let body: Vec<char> = "道可道，非常道。".chars().collect();
        let red = vec![false; body.len()];
        let l = layout(&spec(&body, &red));
        let comma = match l.chars.iter().find(|c| c.ch == '，') {
            Some(c) => c,
            None => panic!("未排出逗号"),
        };
        assert!(comma.punct);
        let (px, py, pfs) = punct_box(comma);
        assert!(px > comma.x, "句读应靠右");
        assert!(py < comma.y + comma.cell * 0.5, "句读应靠上");
        assert!(pfs < comma.fs, "句读字号应收小");
    }

    // ── Markdown 清洗器（v1.20.0）───────────────────────────────────────

    /// 带 Markdown 装饰位的 spec（`red` 由调用方提供，与 `body` 等长）。
    fn spec_md<'a>(c: &'a CleanMd, red: &'a [bool]) -> LayoutSpec<'a> {
        let mut s = spec(&c.body, red);
        s.md = &c.md;
        s
    }

    /// ① 语法记号一个都不印：`#` / `**` / `---` / `- ` / 表格竖线全部消失。
    #[test]
    fn md_cleaner_strips_syntax_and_marks_headings() {
        let src = "# 千字文\n\n天地玄黃，宇宙洪荒。\n\n---\n\n**金生麗水**，玉出昆岡。\n\n- 龍師火帝\n";
        let c = clean_md(src);
        let s = c.text();
        for bad in ['#', '*', '|', '`'] {
            assert!(!s.contains(bad), "记号没洗掉：{bad:?} → {s}");
        }
        assert!(!s.contains("---"), "分隔线没丢：{s}");
        assert!(!s.contains("- "), "列表标记没丢：{s}");
        // 标题字：开头三字（千字文）是 head，正文不是。
        assert_eq!(c.body.len(), c.md.len(), "装饰位必须与字等长");
        assert!(c.md.iter().take(3).all(|m| m.head), "标题字未标记");
        assert!(
            c.md.iter().skip(3).all(|m| !m.head),
            "非标题字被误标成标题"
        );
        // 换行与空行不产字（竖排是连续流，`\n` 只会变成空白格）。
        assert!(!c.body.iter().any(|c| c.is_whitespace() && *c != ' '));
        // 加粗：`**金生麗水**` 四字带 bold 旗子，其余不带。
        let at = c.body.windows(4).position(|w| w == ['金', '生', '麗', '水']).expect("正文丢了");
        assert!(c.md[at..at + 4].iter().all(|m| m.bold), "加粗未标记");
        assert!(c.md[at + 5].bold == false);
        assert!(!c.md[0].bold, "标题不该被当成加粗");
    }

    /// ② 链接只留文字、斜体只去记号、行内代码去反引号、HTML 标签整段丢。
    #[test]
    fn md_cleaner_keeps_only_the_words() {
        let c = clean_md("見[莊子](https://example.com)注，*逍遙*遊，`齊物`論，<span>人間世</span>\n");
        let s = c.text();
        assert_eq!(s, "見莊子注，逍遙遊，齊物論，人間世", "清洗结果不符：{s}");
    }

    /// ③ 幂等：清洗结果再洗一遍，**字一个不变**（用户会在编辑器里反复刷预览）。
    ///
    /// 装饰位不参与幂等——记号已经洗掉了，第二遍自然标不出标题 / 加粗；
    /// 这是「先出干净文字、再谈样式」的题中之义，不是缺陷。
    #[test]
    fn md_cleaner_is_idempotent() {
        let src = "# 題\n**甲**乙[丙](x)\n---\n- 丁\n";
        let a = clean_md(src);
        let b = clean_md(&a.text());
        assert_eq!(a.body, b.body, "清洗不幂等：{} vs {}", a.text(), b.text());
        assert!(
            b.md.iter().all(|m| *m == MdMark::default()),
            "二次清洗不该再产生装饰位"
        );
    }

    /// ④ 标题只放大字号，**字格 / 位置一格不动**——网格是竖排的命。
    #[test]
    fn heading_chars_get_a_bigger_font_but_the_same_cell() {
        let c = clean_md("# 千字文\n天地玄黃宇宙洪荒\n");
        let red = vec![false; c.body.len()];
        let l = layout(&spec_md(&c, &red));
        let head = &l.chars[0];
        let body = &l.chars[3];
        assert!(head.fs > body.fs * 1.1, "标题没放大：{} vs {}", head.fs, body.fs);
        assert!(
            head.fs <= head.cell * HEAD_CELL_FILL + 0.01,
            "放大后撑破字格：fs={} cell={}",
            head.fs,
            head.cell
        );
        assert!(
            (head.cell - body.cell).abs() < 0.01,
            "字格被标题改动了：{} vs {}",
            head.cell,
            body.cell
        );
        // 每个字的 x/y 网格位置与「无装饰」版完全一致（只有字号不同）。
        let plain: Vec<char> = c.body.clone();
        let red = vec![false; plain.len()];
        let l0 = layout(&spec(&plain, &red));
        for (a, b) in l0.chars.iter().zip(l.chars.iter()) {
            assert!((a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01, "标题挪动了网格");
        }
    }

    /// ⑤ 加粗只改笔重，不改版式：坐标逐字相同。
    #[test]
    fn bold_changes_weight_only() {
        let body: Vec<char> = "金生麗水玉出昆岡".chars().collect();
        let red = vec![false; body.len()];
        let plain = layout(&spec(&body, &red));
        let md: Vec<MdMark> = (0..body.len())
            .map(|i| MdMark {
                head: false,
                bold: i < 4,
            })
            .collect();
        let mut s = spec(&body, &red);
        s.md = &md;
        let bolded = layout(&s);
        for (a, b) in plain.chars.iter().zip(bolded.chars.iter()) {
            assert!((a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01, "加粗挪动了网格");
            assert!((a.fs - b.fs).abs() < 0.01, "加粗改了字号");
        }
        assert_eq!(
            bolded.chars.iter().filter(|c| c.bold).count(),
            4,
            "加粗旗子没带到落位结果里"
        );
    }
}
