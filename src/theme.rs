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
pub const PAPER_THEMES: &[PaperTheme] = &[
    PaperTheme {
        id: "xuanzhi",
        name: "宣纸",
        paper: 0xF5F0E6,
        text: 0x2F3535,
        note: 0xB23A30,  // 朱砂红
        border: 0x2F3535, // 乌丝栏
    },
    PaperTheme {
        id: "mihuang",
        name: "米黄",
        paper: 0xF2E6D0,
        text: 0x222B2D,
        note: 0x5C7C7F,
        border: 0x4A6B5D, // 绿丝栏
    },
    PaperTheme {
        id: "qingjian",
        name: "青简",
        paper: 0xD8E2DB,
        text: 0x1A2421,
        note: 0x6A5A4B,
        border: 0x1A2421,
    },
    PaperTheme {
        id: "yedou",
        name: "夜读",
        paper: 0x1A1C1D,
        text: 0xDCD6CD, // 月白
        note: 0xA68E5C, // 秋香
        border: 0x4A4D4F, // 灰丝栏
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    /// 宣纸：米白偏暖，最像常见的影印本。
    Xuan,
    /// 米黄：更黄一些，旧书味重。
    Mi,
    /// 青简：青灰纸配竹青界行。
    Qing,
    /// 夜读：暖调深底。
    Ye,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::Xuan
    }
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Xuan, Theme::Mi, Theme::Qing, Theme::Ye];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Xuan => "宣纸",
            Theme::Mi => "米黄",
            Theme::Qing => "青简",
            Theme::Ye => "夜读",
        }
    }

    /// 本主题在 [`PAPER_THEMES`] 里的下标。
    pub fn index(self) -> usize {
        match self {
            Theme::Xuan => 0,
            Theme::Mi => 1,
            Theme::Qing => 2,
            Theme::Ye => 3,
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
    /// 将来的朱丝版式 / 朱印装饰。
    pub fn rule_red(self) -> u32 {
        match self {
            Theme::Xuan => 0xB23A2E,
            Theme::Mi => 0xB23A2E,
            Theme::Qing => 0xA8443A,
            Theme::Ye => 0xD0634E,
        }
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

    /// 丝栏取色：按样式面板所选丝栏色给出界行颜色。
    /// 乌 = `rule_ink`（冷灰）；朱 = `rule_red`（朱红）；
    /// 金 = 泥金——亮纸上压一档防飘，夜读里提亮才看得见。
    pub fn rule_by(self, rc: crate::types::RuleColor) -> u32 {
        match rc {
            crate::types::RuleColor::Wu => self.rule_ink(),
            crate::types::RuleColor::Zhu => self.rule_red(),
            crate::types::RuleColor::Jin => match self {
                Theme::Xuan | Theme::Mi => 0xB08A3E,
                Theme::Qing => 0x8F7A45,
                Theme::Ye => 0xC2A25C,
            },
        }
    }

    /// 朱色：印章、句读、当前条目。
    pub fn accent(self) -> u32 {
        match self {
            Theme::Xuan => 0xA33B2B,
            Theme::Mi => 0xA63A22,
            Theme::Qing => 0x99503C,
            Theme::Ye => 0xC4614A,
        }
    }

    /// 靛色：通假标注。
    pub fn indigo(self) -> u32 {
        match self {
            Theme::Xuan => 0x3E5A8A,
            Theme::Mi => 0x3E5A8A,
            Theme::Qing => 0x365C6B,
            Theme::Ye => 0x7E9BC8,
        }
    }

    /// 界面底（工具栏、状态栏、浮层）。
    pub fn chrome(self) -> u32 {
        match self {
            Theme::Xuan => 0xFBF7EC,
            Theme::Mi => 0xFDF9E9,
            Theme::Qing => 0xF1F5F0,
            Theme::Ye => 0x211D18,
        }
    }

    /// 界面分隔线。
    pub fn chrome_line(self) -> u32 {
        match self {
            Theme::Xuan => 0xE4DBC6,
            Theme::Mi => 0xE8DEC5,
            Theme::Qing => 0xD8E1D9,
            Theme::Ye => 0x38312A,
        }
    }

    /// 界面文字。
    pub fn chrome_text(self) -> u32 {
        match self {
            Theme::Xuan => 0x4A443C,
            Theme::Mi => 0x4C4438,
            Theme::Qing => 0x3A4A44,
            Theme::Ye => 0xC9BDA8,
        }
    }

    /// 控件底（未选中的小按钮）。
    pub fn chip(self) -> u32 {
        match self {
            Theme::Xuan => 0xF3EFE4,
            Theme::Mi => 0xF6F0DC,
            Theme::Qing => 0xE7EDE6,
            Theme::Ye => 0x2B2620,
        }
    }

    /// 控件底（选中）。
    pub fn chip_on(self) -> u32 {
        match self {
            Theme::Xuan => 0xF3DFC6,
            Theme::Mi => 0xF5E0C1,
            Theme::Qing => 0xDCE7DF,
            Theme::Ye => 0x43382C,
        }
    }
}

/// 双行夹注目色向纸色的兑淡比例：0.38 ≈ 墨色兑三成纸色。
/// 再淡小字（半格）在纸上发虚；再浓又失去「夹注比正文淡」的层次。
pub const Z_NOTE_FADE: f32 = 0.38;

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

/// 正文 / 小字注释的配色方案。
///
/// 参照 09 图样式面板里成对的色块：一组两色，前者为正文，后者为小字注释。
/// 繁体批注取 `note`，通假（借字）取 `gloss`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InkPalette {
    /// 墨黑正文 · 朱红批注 · 靛蓝通假（默认，最接近常见刻本）
    Ink,
    /// 深墨绿正文 · 砖红批注 · 靛蓝通假（09 图所选）
    Forest,
    /// 赭褐正文 · 朱红批注 · 靛蓝通假
    Ochre,
    /// 靛青正文 · 靛蓝批注
    Indigo,
    /// 紫棠正文 · 洋红批注
    Plum,
    /// 暗金正文 · 米白注文（新增的一组：夜读下最靓——金墨配纸色；
    /// 浅纸下注文自动兑淡为淡金，避免米白隐进纸里）
    Gold,
}

impl Default for InkPalette {
    fn default() -> Self {
        InkPalette::Ink
    }
}

impl InkPalette {
    pub const ALL: [InkPalette; 6] = [
        InkPalette::Ink,
        InkPalette::Forest,
        InkPalette::Ochre,
        InkPalette::Indigo,
        InkPalette::Plum,
        InkPalette::Gold,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InkPalette::Ink => "墨朱",
            InkPalette::Forest => "松墨",
            InkPalette::Ochre => "赭朱",
            InkPalette::Indigo => "靛青",
            InkPalette::Plum => "紫棠",
            InkPalette::Gold => "暗金",
        }
    }

    /// 正文色。夜读（深底）时提亮，避免黑底黑字。
    ///
    /// **墨朱组（默认）直接取纸主题的正文色**（配置表 `textColor`）——
    /// 「纸—墨」是一套设计好的搭配，不该再被组内硬编码覆盖；
    /// 其余墨色组（松墨 / 赭朱 / 靛青 / 紫棠 / 暗金）仍是读者自选的组内配色。
    pub fn ink(self, t: Theme) -> u32 {
        if matches!(self, InkPalette::Ink) {
            return t.ink();
        }
        if t.is_dark() {
            return match self {
                InkPalette::Ink => 0xDCCFB8,
                InkPalette::Forest => 0xBCD3B6,
                InkPalette::Ochre => 0xD9C2A2,
                InkPalette::Indigo => 0xC2D2E6,
                InkPalette::Plum => 0xDAC2E0,
                InkPalette::Gold => 0xD9B55F,
            };
        }
        match self {
            InkPalette::Ink => 0x2A241E,
            InkPalette::Forest => 0x24402F,
            InkPalette::Ochre => 0x4A3520,
            InkPalette::Indigo => 0x233A52,
            InkPalette::Plum => 0x3E2A46,
            InkPalette::Gold => 0x7A5C1E,
        }
    }

    /// 小字注文（繁体、夹注）的颜色：默认组取纸主题的夹注色（`noteColor`），
    /// 与正文严格分色（见 [`InkPalette::ink`] 的说明）。
    pub fn note(self, t: Theme) -> u32 {
        if matches!(self, InkPalette::Ink) {
            return t.note();
        }
        if t.is_dark() {
            return match self {
                InkPalette::Indigo => 0x8FA8D0,
                InkPalette::Plum => 0xD08AB0,
                InkPalette::Gold => 0xD79A6B,
                _ => 0xD08163,
            };
        }
        match self {
            InkPalette::Ink => 0xA33B2B,
            InkPalette::Forest => 0xA8442E,
            InkPalette::Ochre => 0xB23A2E,
            InkPalette::Indigo => 0x3E5A8A,
            InkPalette::Plum => 0xB0417A,
            InkPalette::Gold => 0xAD5A33,
        }
    }

    /// 双行夹注的小字色：**由正文墨色向纸色兑淡一档**（见 `mix`）。
    ///
    /// 刻本里的夹注小字与正文同墨，因字小、墨薄，看上去就是「淡一点」。
    /// 这里把这件事显式算出来：正文字色按当前纸色明暗自动切换——
    /// 浅纸 → 墨色向纸兑淡（更浅）；深底 → 墨色向底色沉下去（更沉），
    /// 永远比正文淡一档，又不淡到看不清。
    /// 暗金组是唯一定死成对色的：深底下用米白（金墨的绝配），
    /// 浅纸上米白会隐进纸色，仍走兑淡（得到一枚淡金）。
    pub fn z_note(self, t: Theme, back: bool) -> u32 {
        if matches!(self, InkPalette::Gold) && t.is_dark() {
            return 0xEFE6CE; // 米白
        }
        // 墨朱组（默认）：双行夹注直接用纸主题的夹注色——进入夹注分支就
        // 换这支笔（正文 `textColor` / 夹注 `noteColor` 严格分色）。
        if matches!(self, InkPalette::Ink) {
            return t.note();
        }
        let paper = if back { t.paper_back() } else { t.paper() };
        mix(self.ink(t), paper, Z_NOTE_FADE)
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
            if matches!(p, InkPalette::Gold) {
                // 暗金组深底走成对色：米白。
                assert_eq!(note, 0xEFE6CE);
            } else {
                assert!(
                    lum(note) < lum(ink),
                    "{p:?} 深底夹注应比正文沉：ink={ink:06X} note={note:06X}"
                );
            }
        }
    }

    #[test]
    fn gold_group_pairs_gold_ink_with_off_white_note() {
        // 新墨色组：深底正文暗金（亮金墨）、夹注米白。
        assert_eq!(InkPalette::Gold.ink(Theme::Ye), 0xD9B55F);
        assert_eq!(InkPalette::Gold.z_note(Theme::Ye, false), 0xEFE6CE);
        // 浅纸：正文暗金可读，夹注自动兑淡（淡金，但仍比正文淡）。
        let ink = InkPalette::Gold.ink(Theme::Xuan);
        assert!(lum(ink) < 128.0, "浅纸暗金正文要够深：{ink:06X}");
        assert!(lum(InkPalette::Gold.z_note(Theme::Xuan, false)) > lum(ink));
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
        assert_eq!(PAPER_THEMES.len(), 4);
        let ids: Vec<&str> = PAPER_THEMES.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["xuanzhi", "mihuang", "qingjian", "yedou"]);
        let names: Vec<&str> = PAPER_THEMES.iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["宣纸", "米黄", "青简", "夜读"]);
        // 四色齐全、互不重复（纸 / 正文 / 夹注 / 界栏）。
        for p in PAPER_THEMES {
            assert!(luma(p.paper) != luma(p.text), "{} 纸与正文同色", p.id);
            assert_ne!(p.text, p.note, "{} 正文与夹注同色", p.id);
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
        assert_eq!(crate::banxin::FISH_INK, 0x333333);
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
