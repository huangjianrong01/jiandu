//! 纸色主题：宣纸 / 米黄 / 青简 / 夜读。
//!
//! 刻本的美感来自「层次」而不是「对比」：纸、墨、界行、版框、朱印各压一档，
//! 正文才立得住。夜读不用纯黑底 + 纯白字，改用暖调深棕 + 米色墨，
//! 长时间读不刺眼。

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

    pub fn is_dark(self) -> bool {
        matches!(self, Theme::Ye)
    }

    /// 纸面（正面）。
    pub fn paper(self) -> u32 {
        match self {
            Theme::Xuan => 0xF2EAD6,
            Theme::Mi => 0xF6EED4,
            Theme::Qing => 0xE6ECE4,
            Theme::Ye => 0x1E1A16,
        }
    }

    /// 纸面（翻过来的一面，略深一档）。
    pub fn paper_back(self) -> u32 {
        match self {
            Theme::Xuan => 0xE9DDC3,
            Theme::Mi => 0xEDE1C1,
            Theme::Qing => 0xDBE3DB,
            Theme::Ye => 0x181512,
        }
    }

    /// 正文墨色。
    pub fn ink(self) -> u32 {
        match self {
            Theme::Xuan => 0x2A241E,
            Theme::Mi => 0x2E2820,
            Theme::Qing => 0x1F2C28,
            Theme::Ye => 0xDCCFB8,
        }
    }

    /// 淡墨：小字注文、行款、背面。
    pub fn ink_soft(self) -> u32 {
        match self {
            Theme::Xuan => 0x4A4038,
            Theme::Mi => 0x4E443A,
            Theme::Qing => 0x3C4A44,
            Theme::Ye => 0xB3A68E,
        }
    }

    /// 界行（列与列之间的细线）。
    pub fn rule(self) -> u32 {
        match self {
            Theme::Xuan => 0x9C8A6C,
            Theme::Mi => 0xA08B64,
            Theme::Qing => 0x6F8A80,
            Theme::Ye => 0x4B4136,
        }
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

    /// 版框（四周双边）。
    pub fn frame(self) -> u32 {
        match self {
            Theme::Xuan => 0x6B5A44,
            Theme::Mi => 0x6E5A3E,
            Theme::Qing => 0x466157,
            Theme::Ye => 0x6A5C4B,
        }
    }

    /// 古籍版框：四周双边的外框，取近墨的深色（比界行重一档）。
    /// 对照刻本影印：框线是墨，不是棕；夜读反过来用淡米墨。
    pub fn frame_ink(self) -> u32 {
        match self {
            Theme::Xuan => 0x3A3128,
            Theme::Mi => 0x3C3328,
            Theme::Qing => 0x26332E,
            Theme::Ye => 0x9A8C76,
        }
    }

    /// 乌丝栏：竖排列间界行，比版框淡一档、比纸深一档。
    /// 取「墨兑水」的灰调（略去红黄），才是影印本里那种冷灰界行；
    /// 饱和度一高，细线在小字号下会发粉，看着像朱丝栏。
    pub fn rule_ink(self) -> u32 {
        match self {
            Theme::Xuan => 0x8A8375,
            Theme::Mi => 0x8C8474,
            Theme::Qing => 0x63776F,
            Theme::Ye => 0x574B3C,
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
    pub fn ink(self, t: Theme) -> u32 {
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

    /// 小字注文（繁体、夹注）的颜色。
    pub fn note(self, t: Theme) -> u32 {
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
        assert_ne!(
            InkPalette::Ink.z_note(Theme::Xuan, true),
            InkPalette::Ink.z_note(Theme::Xuan, false)
        );
    }
}
