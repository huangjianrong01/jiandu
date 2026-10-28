//! 纸色主题：宣纸 / 米黄 / 青简 / 夜读。
//!
//! 刻本的美感来自「层次」而不是「对比」：纸、墨、界行、版框、朱印各压一档，
//! 正文才立得住。夜读不用纯黑底 + 纯白字，改用暖调深棕 + 米色墨，
//! 长时间读不刺眼。

/// 纸面主题配置：**纸 / 正文 / 夹注 / 界栏**四色。
///
/// 这是视觉配色的**唯一数据源**：
///   * 设置面板的颜色预览按钮由 [`PAPER_THEMES`] 遍历自动生成；
///   * 渲染层经 [`Theme::paper_theme`] 取这四色上色
///     （背景=纸色，界栏/版框=界栏色，正文=正文色，双行夹注=夹注色）。
///
/// 新增一套配色只在此加一行，UI 与渲染零改动。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaperTheme {
    /// 配置 id（持久化 / 反查用）。
    pub id: &'static str,
    /// 面板上显示的名字。
    pub name: &'static str,
    /// 纸色（全局背景）。
    pub paper: u32,
    /// 正文色（松烟墨）。
    pub text: u32,
    /// 夹注色（双行小字）。
    pub note: u32,
    /// 界栏 / 版框色（乌丝栏）。
    pub border: u32,
}

/// 主题配置表：顺序即面板按钮顺序，下标即 `Theme` 的编号。
///
/// **纸色固定四套**（宣纸 / 米黄 / 青简 / 夜读），与「墨色」四套
/// （墨朱 / 松墨 / 赭朱 / 暗金）一一联动（见 [`Theme::linked_palette`]）。
/// 每套纸的 `text` / `note` 即联动墨色的浅纸取值，选「墨朱」组时直接取这里。
/// 新增配色只在此加一行（同时给 `Theme` 加一个变体）。
pub const PAPER_THEMES: &[PaperTheme] = &[
    PaperTheme {
        id: "xuanzhi",
        name: "宣纸",
        paper: 0xF5F0E6,
        text: 0x2F3535,  // 墨朱·正文
        note: 0xB23A30,  // 墨朱·批注（朱砂红）
        border: 0x2F3535, // 乌丝栏
    },
    PaperTheme {
        id: "mihuang",
        name: "米黄",
        paper: 0xF2E6D0,
        text: 0x3D352F,  // 赭朱·正文
        note: 0xA63F2E,  // 赭朱·批注
        border: 0x4A6B5D, // 绿丝栏
    },
    PaperTheme {
        id: "qingjian",
        name: "青简",
        paper: 0xD8E2DB,
        text: 0x222B2D,  // 松墨·正文
        note: 0x8C3A3A,  // 松墨·批注
        border: 0x222B2D,
    },
    PaperTheme {
        id: "yedou",
        name: "夜读",
        paper: 0x1A1C1D,
        text: 0xDCD6CD, // 暗金·正文（月白）
        note: 0xA68E5C, // 暗金·批注（秋香）
        border: 0x4A4D4F, // 灰丝栏
    },
    // ── v1.83.0 · 仿古籍精刻本 ──────────────────────────────────────
    // 用户点名的那一档：**纸是 #F7F6F1**（冷一点的米白，不是宣纸黄），
    // 墨是刻本的**焦墨**（近黑，比松烟墨更硬），界栏比正文再深半档——
    // 照片里「外框最重、内框与界行退一档」的层次就靠这三个值拉开。
    //
    // 这一档不只是「换纸色」：`is_keben()` 一并把版框/界行的**笔路**
    // 切换成精刻本的规矩（见 [`Theme::rule_line`]），出图那一层也改走
    // `ShareFrame::Double`（外粗内细的双边）。
    PaperTheme {
        id: "keben",
        name: "仿刻本",
        paper: 0xF7F6F1,
        text: 0x1E1B18,   // 焦墨（刻本的字比宋体屏显字更黑一档）
        note: 0x8C2B22,   // 朱（比墨朱稍沉，压在冷米白上不刺）
        border: 0x2E2823, // 版框：比正文更深，外框才立得住
    },
];

impl PaperTheme {
    /// 按 id 取配置（未知 id 回落首项）。
    pub fn by_id(id: &str) -> &'static PaperTheme {
        PAPER_THEMES.iter().find(|p| p.id == id).unwrap_or(&PAPER_THEMES[0])
    }

    /// 是否深底（由纸色亮度推导，不写死枚举）。
    pub fn is_dark(&self) -> bool {
        luma(self.paper) < DARK_LUMA
    }
}

/// 纸色亮度低于此值即视为「夜读」深底（0..255）。
pub const DARK_LUMA: f32 = 110.0;

/// 感知亮度（0..255），用于深底判定与「谁更淡」的比较。
pub fn luma(c: u32) -> f32 {
    let r = ((c >> 16) & 0xFF) as f32;
    let g = ((c >> 8) & 0xFF) as f32;
    let b = (c & 0xFF) as f32;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// 彩度（RGB 三通道极差，0..255）：用来把「低饱和国色」写成可回归的约束。
///
/// 竞品爱用高饱和的蓝 / 紫 / 绿做阅读底，一眼就跳；这套配色全部压在极差
/// 之下——纸色极差不超过 60，正文与界栏不超过 80。夹注朱印是另一回事，
/// 它可以艳，但不许染成荧光（上限 170）。
pub fn sat(c: u32) -> f32 {
    let r = ((c >> 16) & 0xFF) as f32;
    let g = ((c >> 8) & 0xFF) as f32;
    let b = (c & 0xFF) as f32;
    r.max(g).max(b) - r.min(g).min(b)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[derive(Default)]
pub enum Theme {
    /// 宣纸：米白偏暖，最像常见的影印本（联动「墨朱」）。
    #[default]
    Xuan,
    /// 米黄：更黄一些，旧书味重（联动「赭朱」）。
    Mi,
    /// 青简：青灰纸配竹青界行（联动「松墨」）。
    Qing,
    /// 夜读：暖调深底（联动「暗金」）。
    Ye,
    /// **仿古籍精刻本**（v1.83.0）：冷米白纸 #F7F6F1 + 焦墨 + 双边版框。
    Ke,
}


impl Theme {
    /// 顺序必须与 [`PAPER_THEMES`] 完全一致——下标即主题编号。
    pub const ALL: [Theme; 5] = [
        Theme::Xuan,
        Theme::Mi,
        Theme::Qing,
        Theme::Ye,
        Theme::Ke,
    ];

    /// 面板上显示的名字（直接取配置表，免得两套名字不同步）。
    pub fn label(self) -> &'static str {
        self.paper_theme().name
    }

    /// 本主题在 [`PAPER_THEMES`] 里的下标。
    pub fn index(self) -> usize {
        match self {
            Theme::Xuan => 0,
            Theme::Mi => 1,
            Theme::Qing => 2,
            Theme::Ye => 3,
            Theme::Ke => 4,
        }
    }

    /// **是否仿刻本模式**（v1.83.0）。
    ///
    /// 只此一处判定：版框笔路、界行淡度、出图底与框、默认字体、句读式样
    /// 全都问这一个问题，绝不各区各写一份 `matches!(…)`——那样迟早分家，
    /// 屏幕上是一套、导出的是另一套。
    pub fn is_keben(self) -> bool {
        matches!(self, Theme::Ke)
    }

    /// 「纸色 → 墨色」智能联动的默认配对（任务四：一键换肤）。
    ///
    /// 宣纸→墨朱、米黄→赭朱、青简→松墨、夜读→暗金；仿刻本→墨朱（焦墨那套）；
    /// 用户随后仍可手动把墨色切到任意一套（手动覆盖默认联动）。
    pub fn linked_palette(self) -> InkPalette {
        match self {
            Theme::Xuan | Theme::Ke => InkPalette::Ink,
            Theme::Mi => InkPalette::Ochre,
            Theme::Qing => InkPalette::Forest,
            Theme::Ye => InkPalette::Gold,
        }
    }

    /// 取本主题的四色配置（纸 / 正文 / 夹注 / 界栏）。
    pub fn paper_theme(self) -> &'static PaperTheme {
        &PAPER_THEMES[self.index()]
    }

    /// 是否深底：由纸色亮度推导（新增深底主题无需改这里）。
    pub fn is_dark(self) -> bool {
        self.paper_theme().is_dark()
    }

    /// 纸面（正面）：纸色。
    pub fn paper(self) -> u32 {
        self.paper_theme().paper
    }

    /// 纸面（翻过来的一面）：纸色向「墨 / 纯黑」压深一档。
    pub fn paper_back(self) -> u32 {
        let p = self.paper_theme();
        mix(p.paper, if p.is_dark() { 0x000000 } else { p.text }, 0.07)
    }

    /// 正文墨色（松烟墨）。
    pub fn ink(self) -> u32 {
        self.paper_theme().text
    }

    /// 夹注色（双行小字）：与正文严格区分的一色。
    pub fn note(self) -> u32 {
        self.paper_theme().note
    }

    /// 淡墨：小字注文、行款、背面——正文色向纸色兑淡。
    pub fn ink_soft(self) -> u32 {
        let p = self.paper_theme();
        mix(p.text, p.paper, 0.30)
    }

    /// 界行（列与列之间的细线）：界栏色向纸色兑淡，比版框再轻一档。
    pub fn rule(self) -> u32 {
        let p = self.paper_theme();
        mix(p.border, p.paper, 0.25)
    }

    /// 朱丝栏：写经体的朱红界行（古本里的另一路做法）。
    /// 竖排正文目前走「乌丝栏」（`rule_ink`，黑口本一路），此色留给
    /// 将来的朱丝版式 / 朱印装饰——取夹注的朱色，与全书的朱印同源。
    pub fn rule_red(self) -> u32 {
        self.note()
    }

    /// 版框（四周双边）：界栏色向纸色兑淡一档（比界行轻，框不抢字）。
    pub fn frame(self) -> u32 {
        let p = self.paper_theme();
        mix(p.border, p.paper, 0.35)
    }

    /// 古籍版框：四周双边的外框——**界栏色**（配置表的 `borderColor`）。
    pub fn frame_ink(self) -> u32 {
        self.paper_theme().border
    }

    /// 乌丝栏：竖排列间界行——**界栏色**（配置表的 `borderColor`）。
    pub fn rule_ink(self) -> u32 {
        self.paper_theme().border
    }

    /// **黑鱼尾**的墨色：深灰 `#333333`，不是纯黑——纯黑在纸面上太跳，
    /// 会盖过正文的墨。深底（夜读）反过来取「淡墨」：黑纸上的黑鱼尾
    /// 与纸只剩十来个单位的亮度差，看上去像一枚脏印；改用纸→墨的方向
    /// 兑到六成，才是一枚看得清、又不抢正文的灰鱼尾。
    pub fn fish_ink(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.paper, self.ink(), 0.62)
        } else {
            crate::banxin::FISH_INK
        }
    }

    /// 版心折痕：极淡的灰 `#999999`——纸对折的物理褶，不是界栏，
    /// 故比界行再淡一档（深底上略压暗，免得亮成一道白线）。
    pub fn fold_ink(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            // 深底：折痕只在纸上比底色亮一点点（界行 0x4A4D4F 已够淡，
            // 折痕必须更淡，否则中缝会亮成一道灯管）。
            mix(p.paper, crate::banxin::FOLD_INK, 0.30)
        } else {
            crate::banxin::FOLD_INK
        }
    }

    /// 丝栏取色：按版字面板所选丝栏色给出界行颜色。
    /// 乌 = `rule_ink`（界栏色）；朱 = `rule_red`（夹注朱色）；
    /// 金 = 泥金——亮纸上压一档防飘，深底里提亮才看得见（不写死枚举，
    /// 新增国色主题零改动）。
    pub fn rule_by(self, rc: crate::types::RuleColor) -> u32 {
        match rc {
            crate::types::RuleColor::Wu => self.rule_ink(),
            crate::types::RuleColor::Zhu => self.rule_red(),
            crate::types::RuleColor::Jin => {
                if self.is_dark() {
                    0xC2A25C
                } else {
                    0xB08A3E
                }
            }
        }
    }

    /// **界行 / 内框用色**（v1.83.0）：仿刻本退一档，其余主题原样。
    ///
    /// 精刻本的层次是「**外框最重 → 内框与界行退一档 → 字最重**」：
    /// 照片里外框一道浓黑，内框和界行都淡成灰。旧制内外同色同粗，
    /// 远看是「一圈粗细不匀的框」；这里只把**内**的那一笔退下去，
    /// 外框的墨一分不动——所以非刻本主题逐值等于 [`Theme::rule_by`]，
    /// 老版面一个像素都不变。
    ///
    /// 退的幅度 [`KB_RULE_FADE`] 是刻意的：再淡就散成一层雾，界行立不住。
    pub fn rule_line(self, rc: crate::types::RuleColor) -> u32 {
        let c = self.rule_by(rc);
        if self.is_keben() {
            mix(c, self.paper(), KB_RULE_FADE)
        } else {
            c
        }
    }

    /// 朱色：印章、句读、当前条目。
    ///
    /// 取夹注的朱色（与全书朱印同源），深底再向纸色提亮一档——
    /// 否则夜读里朱印会沉进深纸看不见。浅纸直接用，深底自动提亮。
    pub fn accent(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.note, p.paper, 0.20)
        } else {
            p.note
        }
    }

    /// 靛色：通假标注。
    pub fn indigo(self) -> u32 {
        if self.is_dark() {
            0x7E9BC8
        } else {
            0x3E5A8A
        }
    }

    /// 界面底（工具栏、状态栏、浮层）——纸色向黑（深底）或向白（浅底）压，
    /// 永远与纸面同调，不另起一套色；深底自动更暗、浅底自动更亮。
    pub fn chrome(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.paper, 0x000000, 0.16)
        } else {
            mix(p.paper, 0xFFFFFF, 0.10)
        }
    }

    /// 界面分隔线——向对比色压一档（深底提亮、浅底压暗）。
    pub fn chrome_line(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.paper, 0xFFFFFF, 0.16)
        } else {
            mix(p.paper, 0x000000, 0.10)
        }
    }

    /// 界面文字——深底取浅、浅底取深，保证与 `chrome()` 永远反色，
    /// 绝不出现白底白字 / 黑底黑字。
    pub fn chrome_text(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.paper, 0xFFFFFF, 0.30)
        } else {
            mix(p.text, p.paper, 0.10)
        }
    }

    /// 控件底（未选中的小按钮）——纸色向白压一档（深浅都压白，控件浮于纸面）。
    pub fn chip(self) -> u32 {
        let p = self.paper_theme();
        mix(p.paper, 0xFFFFFF, if p.is_dark() { 0.12 } else { 0.06 })
    }

    /// 控件底（选中）——朱色向纸色兑淡，深浅底都压到「可辨的淡朱」。
    pub fn chip_on(self) -> u32 {
        let p = self.paper_theme();
        mix(p.note, p.paper, if p.is_dark() { 0.45 } else { 0.62 })
    }
}

/// 双行夹注目色向纸色的兑淡比例：0.38 ≈ 墨色兑三成纸色。
/// 再淡小字（半格）在纸上发虚；再浓又失去「夹注比正文淡」的层次。
pub const Z_NOTE_FADE: f32 = 0.38;

/// **仿刻本的界行淡度**（v1.83.0）：版框色向纸色掺 45%。
///
/// 照片上量的：外框是浓墨，内框与界行淡成一道灰。45% 是「看得见是线、
/// 又绝不与正文抢」的那一档——再淡（>0.55）界行散成一层雾，行列就分不清了。
pub const KB_RULE_FADE: f32 = 0.45;

/// 把颜色 `a` 向颜色 `b` 混 `k`（0..=1，k=1 全为 b），返回 0xRRGGBB。
/// 用于「墨色 + 纸色 → 夹注目色」的明暗底自动推导。
pub fn mix(a: u32, b: u32, k: f32) -> u32 {
    let ch = |s: u32| {
        let ca = ((a >> s) & 0xFF) as f32;
        let cb = ((b >> s) & 0xFF) as f32;
        ((ca * (1.0 - k) + cb * k).round() as u32) & 0xFF
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// 正文 / 小字注释的配色方案（墨色）。固定四套，与纸色一一联动。
///
/// 一组两色：**前者为正文（`ink`）、后者为小字注释（`note`）**。
/// 渲染时正文用 `ink`、夹注 / 句读 / 眉批用 `note`——两者严格分色，
/// 绝不能正文注文一个色（任务三·生死攸关）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[derive(Default)]
pub enum InkPalette {
    /// 墨黑正文 · 朱红批注（默认，最接近常见刻本；浅纸直取纸主题 text/note）
    #[default]
    Ink,
    /// 墨绿正文 · 砖红批注（联动青简）
    Forest,
    /// 赭褐正文 · 朱红批注（联动米黄）
    Ochre,
    /// 暗金正文 · 米白/金批注（联动夜读；浅纸走暗金，深底走月白+秋香）
    Gold,
}


impl InkPalette {
    pub const ALL: [InkPalette; 4] = [
        InkPalette::Ink,
        InkPalette::Forest,
        InkPalette::Ochre,
        InkPalette::Gold,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InkPalette::Ink => "墨朱",
            InkPalette::Forest => "松墨",
            InkPalette::Ochre => "赭朱",
            InkPalette::Gold => "暗金",
        }
    }

    /// 正文色。夜读（深底）时提亮，避免黑底黑字。
    ///
    /// **墨朱组（默认）直接取纸主题的正文色**（`PAPER_THEMES` 的 `text`）——
    /// 「纸—墨」是一套设计好的搭配；其余墨色组是读者自选的组内配色。
    pub fn ink(self, t: Theme) -> u32 {
        if matches!(self, InkPalette::Ink) {
            return t.ink();
        }
        if t.is_dark() {
            return match self {
                InkPalette::Ink => 0xDCD6CD,
                InkPalette::Forest => 0xC2D0BC,
                InkPalette::Ochre => 0xD9C2A2,
                InkPalette::Gold => 0xDCD6CD, // 月白（深底正文）
            };
        }
        match self {
            InkPalette::Ink => 0x2F3535,
            InkPalette::Forest => 0x222B2D,
            InkPalette::Ochre => 0x3D352F,
            InkPalette::Gold => 0x7A5C1E, // 暗金（浅纸正文）
        }
    }

    /// 小字注文（繁体、夹注、句读、眉批）的颜色：默认组取纸主题的夹注色
    /// （`PAPER_THEMES` 的 `note`），与正文严格分色。
    pub fn note(self, t: Theme) -> u32 {
        if matches!(self, InkPalette::Ink) {
            return t.note();
        }
        if t.is_dark() {
            return match self {
                InkPalette::Ink => 0xA68E5C,
                InkPalette::Forest => 0xD08163,
                InkPalette::Ochre => 0xD08163,
                InkPalette::Gold => 0xA68E5C, // 秋香（深底批注）
            };
        }
        match self {
            InkPalette::Ink => 0xB23A30,
            InkPalette::Forest => 0x8C3A3A,
            InkPalette::Ochre => 0xA63F2E,
            InkPalette::Gold => 0xAD5A33, // 淡金（浅纸批注）
        }
    }

    /// 双行夹注的小字色：**用 `note` 色向纸色兑淡一档**（见 `mix`）。
    ///
    /// 刻本里的夹注小字与正文同墨，因字小、墨薄，看上去就是「淡一点」；
    /// 但这里刻意让夹注**保持 `note` 的色相**（朱红 / 砖红 / 秋香），
    /// 只是向纸色沉一档做「薄墨」感——正文纯正文色、夹注纯注文色，
    /// 两套笔永远不混（任务三）。
    pub fn z_note(self, t: Theme, back: bool) -> u32 {
        // 墨朱组（默认）：双行夹注直接用纸主题的夹注色——进入夹注分支就
        // 换这支笔（正文 `text` / 夹注 `note` 严格分色）。
        if matches!(self, InkPalette::Ink) {
            return t.note();
        }
        let paper = if back { t.paper_back() } else { t.paper() };
        mix(self.note(t), paper, Z_NOTE_FADE)
    }

    /// 通假（借字）标注色。
    pub fn gloss(self, t: Theme) -> u32 {
        if t.is_dark() {
            0x8FA8D0
        } else {
            0x3E5A8A
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 相对亮度（感知近似，用于「谁更淡」的比较）。
    fn lum(c: u32) -> f32 {
        let r = ((c >> 16) & 0xFF) as f32;
        let g = ((c >> 8) & 0xFF) as f32;
        let b = (c & 0xFF) as f32;
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    #[test]
    fn z_note_is_fainter_than_ink_on_light_paper() {
        // 浅纸：夹注目色 = 墨色向纸色兑淡 → 一定比正文淡（亮度更高）。
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing] {
            for p in InkPalette::ALL {
                let ink = p.ink(t);
                let note = p.z_note(t, false);
                assert!(
                    lum(note) > lum(ink),
                    "{:?}/{:?} 浅纸夹注应比正文淡：ink={ink:06X} note={note:06X}",
                    p,
                    t
                );
                assert_ne!(note, ink, "夹注目色不应与正文同色");
            }
        }
    }

    #[test]
    fn z_note_auto_switches_on_dark_paper() {
        // 深底：正文被提亮，夹注跟着沉下去 → 仍比正文淡（亮度更低）。
        for p in InkPalette::ALL {
            let ink = p.ink(Theme::Ye);
            let note = p.z_note(Theme::Ye, false);
            assert!(
                lum(note) < lum(ink),
                "{p:?} 深底夹注应比正文沉：ink={ink:06X} note={note:06X}"
            );
        }
    }

    #[test]
    fn gold_group_pairs_gold_ink_with_off_white_note() {
        // 暗金组：深底正文月白（0xDCD6CD）、批注秋香（0xA68E5C）。
        assert_eq!(InkPalette::Gold.ink(Theme::Ye), 0xDCD6CD);
        // 深底夹注 = 0xA68E5C 向夜读深纸兑淡 0.38 档。
        assert_eq!(InkPalette::Gold.z_note(Theme::Ye, false), 0x716344);
        // 浅纸：正文暗金可读，夹注自动兑淡（淡金，但仍比正文淡）。
        let ink = InkPalette::Gold.ink(Theme::Xuan);
        assert!(lum(ink) < 128.0, "浅纸暗金正文要够深：{ink:06X}");
        assert!(lum(InkPalette::Gold.z_note(Theme::Xuan, false)) > lum(ink));
    }

    #[test]
    fn four_paper_themes_are_registered_and_indexed() {
        // 主题数与配置表严格同长：加一套配色，两边必须同时加。
        assert_eq!(Theme::ALL.len(), PAPER_THEMES.len());
        assert_eq!(PAPER_THEMES.len(), 5);
        assert_eq!(InkPalette::ALL.len(), 4);
        for (i, t) in Theme::ALL.iter().copied().enumerate() {
            let p = t.paper_theme();
            assert_eq!(t.index(), i, "{:?} 下标错位", t);
            assert_eq!(t.label(), p.name, "{:?} 名字与配置表不同步", t);
            assert_eq!(t.paper(), p.paper);
            assert_eq!(t.ink(), p.text);
            assert_eq!(t.note(), p.note);
            assert_eq!(t.rule_ink(), p.border);
            // 纸色与墨色智能联动：每套纸都有默认墨色配对。
            assert_eq!(t.linked_palette().ink(t), p.text, "{:?} 联动墨色错位", t);
        }
    }

    #[test]
    fn themes_are_low_saturation_guose() {
        // 摒弃高饱和刺眼色：纸 / 正文 / 界栏一律低饱和（通道极差受限）。
        // 夹注朱色是朱印语汇，可以比纸「艳」，但仍不许染成荧光。
        for t in Theme::ALL {
            let p = t.paper_theme();
            assert!(sat(p.paper) < 60.0, "{} 纸色太艳：{:06X}", t.label(), p.paper);
            assert!(sat(p.text) < 80.0, "{} 正文太艳：{:06X}", t.label(), p.text);
            assert!(sat(p.border) < 80.0, "{} 界栏太艳：{:06X}", t.label(), p.border);
            assert!(sat(p.note) < 170.0, "{} 夹注过艳：{:06X}", t.label(), p.note);
        }
    }

    #[test]
    fn ui_chrome_always_inverts_with_text() {
        // 深底（夜读）下界面底要更暗、界面字要更亮——绝不出现白底白字 / 黑底黑字。
        for t in Theme::ALL {
            let d = (luma(t.chrome_text()) - luma(t.chrome())).abs();
            assert!(d > 60.0, "{} 界面字与界面底对比不足：{d:.1}", t.label());
            // 正文与纸面同样要有可辨对比。
            let di = (luma(t.ink()) - luma(t.paper())).abs();
            assert!(di > 60.0, "{} 正文与纸对比不足：{di:.1}", t.label());
            // 深底判定：纸暗则底更暗、字更亮。
            if t.is_dark() {
                assert!(luma(t.chrome()) < luma(t.paper()), "{} 深底界面底应更沉", t.label());
                assert!(luma(t.chrome_text()) > luma(t.paper()), "{} 深底界面字应提亮", t.label());
            } else {
                assert!(luma(t.chrome()) > luma(t.paper()), "{} 浅纸界面底应更亮", t.label());
                assert!(luma(t.chrome_text()) < luma(t.paper()), "{} 浅纸界面字应更深", t.label());
            }
        }
    }

    #[test]
    fn mix_blends_channels() {
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xFFFFFF, 1.0), 0xFFFFFF);
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x405060, 0.5), 0x283848);
    }

    #[test]
    fn back_leaf_uses_back_paper() {
        // 对开左叶纸色略深 → 兑淡基点不同（两叶夹注目色有细微差别）。
        // 墨朱组已改为直取纸主题夹注色（两叶同色），故这里看组内配色组。
        assert_ne!(
            InkPalette::Forest.z_note(Theme::Xuan, true),
            InkPalette::Forest.z_note(Theme::Xuan, false)
        );
    }

    // ---- 纸面主题配置表（UI 与渲染的唯一数据源）----

    #[test]
    fn paper_themes_are_the_single_source_of_truth() {
        assert_eq!(PAPER_THEMES.len(), 5);
        let ids: Vec<&str> = PAPER_THEMES.iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec!["xuanzhi", "mihuang", "qingjian", "yedou", "keben"]
        );
        let names: Vec<&str> = PAPER_THEMES.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec!["宣纸", "米黄", "青简", "夜读", "仿刻本"]
        );
        // 四色齐全、互不重复（纸 / 正文 / 夹注 / 界栏）。
        for p in PAPER_THEMES {
            assert!(luma(p.paper) != luma(p.text), "{} 纸与正文同色", p.id);
            assert_ne!(p.text, p.note, "{} 正文与夹注同色", p.id);
        }
    }

    /// 仿刻本（v1.83.0）：纸号对不对、界行是否退了一档、其余主题是否
    /// **一个像素都没动**——这是「不许碰老版面」的那道闸。
    #[test]
    fn keben_paper_is_f7f6f1_and_rules_fade_one_step() {
        // ① 用户点名的纸号，逐位钉死（改纸色＝改这一档的全部气质）。
        assert_eq!(Theme::Ke.paper(), 0xF7F6F1);
        assert!(Theme::Ke.is_keben());
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing, Theme::Ye] {
            assert!(!t.is_keben(), "{t:?} 不该被当成仿刻本");
        }
        // ② 仿刻本的界行 / 内框比外框淡一档（外框仍是 `frame_ink` 的浓墨）。
        for rc in crate::types::RuleColor::ALL {
            let outer = Theme::Ke.frame_ink();
            let inner = Theme::Ke.rule_line(rc);
            assert_ne!(inner, outer, "{rc:?} 的内框与界行必须退一档");
            assert!(
                luma(inner) > luma(outer),
                "{rc:?} 的内框应比外框淡：{inner:06X} vs {outer:06X}"
            );
            assert!(
                luma(Theme::Ke.paper()) - luma(inner) > 12.0,
                "{rc:?} 的界行淡到看不见了：{inner:06X}"
            );
        }
        // ③ 其余四套主题：`rule_line` 必须与 `rule_by` 逐值相同
        //    （老版面的框线颜色一分都不许变）。
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing, Theme::Ye] {
            for rc in crate::types::RuleColor::ALL {
                assert_eq!(
                    t.rule_line(rc),
                    t.rule_by(rc),
                    "{t:?}/{rc:?} 的界行色被新主题带偏了"
                );
            }
        }
    }

    #[test]
    fn theme_accessors_read_from_paper_theme_config() {
        // 渲染四色严格取自配置表：背景=纸色、正文=textColor、
        // 夹注=noteColor、界栏/版框=borderColor。
        for t in Theme::ALL {
            let p = t.paper_theme();
            assert_eq!(t.paper(), p.paper);
            assert_eq!(t.ink(), p.text);
            assert_eq!(t.note(), p.note);
            assert_eq!(t.rule_ink(), p.border, "界栏必须用 borderColor");
            assert_eq!(t.frame_ink(), p.border, "版框必须用 borderColor");
        }
        // 默认墨色组：正文 / 夹注分别用配置表的 textColor / noteColor。
        for t in Theme::ALL {
            let p = t.paper_theme();
            assert_eq!(InkPalette::Ink.ink(t), p.text);
            assert_eq!(InkPalette::Ink.note(t), p.note);
            assert_eq!(InkPalette::Ink.z_note(t, false), p.note);
        }
    }

    #[test]
    fn dark_theme_is_derived_from_paper_luma() {
        assert!(Theme::Ye.is_dark());
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing] {
            assert!(!t.is_dark());
        }
        // 深底判定来自纸色亮度，不是写死枚举。
        assert!(PaperTheme::by_id("yedou").is_dark());
        assert!(!PaperTheme::by_id("xuanzhi").is_dark());
        // 未知 id 回落首项（配置损坏不崩）。
        assert_eq!(PaperTheme::by_id("nope").id, "xuanzhi");
    }

    #[test]
    fn fish_ink_is_dark_grey_not_black_and_visible_on_every_paper() {
        // 黑鱼尾：浅纸上就是 #333333（不取纯黑，纯黑太跳）。
        assert_eq!(Theme::Xuan.fish_ink(), crate::banxin::FISH_INK);
        assert_eq!(crate::banxin::FISH_INK, 0x222222);
        // 深底（夜读）兑淡：鱼尾不能因为纸黑就整枚消失。
        let ye = Theme::Ye.fish_ink();
        assert_ne!(ye, crate::banxin::FISH_INK);
        assert!(luma(ye) > luma(Theme::Ye.paper()), "夜读的鱼尾应比纸亮");
        // 任何纸上鱼尾与纸色都要有可辨的对比。
        for t in Theme::ALL {
            let d = (luma(t.fish_ink()) - luma(t.paper())).abs();
            assert!(d > 25.0, "{} 的鱼尾与纸对比不足：{d:.1}", t.label());
        }
    }

    #[test]
    fn fold_ink_stays_faint_but_readable() {
        // 折痕是「纸的褶」，必须比界行再淡一档。
        for t in Theme::ALL {
            let f = t.fold_ink();
            let d = (luma(f) - luma(t.paper())).abs();
            assert!(d > 8.0, "{} 的折痕看不见：{d:.1}", t.label());
            assert!(
                d < (luma(t.rule_ink()) - luma(t.paper())).abs(),
                "{} 的折痕不该比界行还重",
                t.label()
            );
        }
    }

    #[test]
    fn paper_back_is_darker_than_paper() {
        for t in Theme::ALL {
            let p = t.paper();
            let b = t.paper_back();
            assert_ne!(p, b);
            if t.is_dark() {
                assert!(luma(b) <= luma(p), "深底背面应更沉");
            } else {
                assert!(luma(b) < luma(p), "浅纸背面应更深");
            }
        }
    }
}
