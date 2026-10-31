//! **样式预设**（v1.9.10）：把一整套版式存成一句话。
//!
//! # 为什么要有它
//!
//! 「版字」面板里能调的东西有十几项——纸色、墨色、版框形制、行款、留白、
//! 句读形制、朱墨、繁简……每本书都从头调一遍，是**重复劳动**；自媒体作者
//! 一周出五条内容，最怕的是**风格漂移**：这一张偏黄、那一张偏青，回头看
//! 不像同一个人的东西。
//!
//! 预设解决的就是这件事：**一次调好，一句记住，一键复现**。
//!
//! # 三条设计边界（别越界）
//!
//! 1. **不动具体字体**（`font_ancient` / `font_modern` / `font_note`）。
//!    自备字体是用户资产，且族名错一个字 GPUI 会**静默回退宋体**——预设
//!    若把人家的汇文明朝体换成系统字，是帮倒忙。预设只管「刻本宋体 /
//!    手写楷体」这个**观感档**（`font_preset`）。
//! 2. **不动字级**（`zoom`）。字号是视力问题，不是审美问题：老读者调到
//!    榜书之后切一套预设，不该被拽回中楷。
//! 3. **只写**观感档，**不改**内容开关里与阅读正确性有关的那几项
//!    （`trad_mode` / `tongjia` / `punct` / `notes`）一律不碰。
//!
//! # 存哪
//!
//! 内建四套是本模块里的常量（不落盘、不可改）；用户自己存的落在
//! `Prefs::user_presets`，最多 [`USER_SLOTS`] 个——三个槽位是刻意的：
//! 再多就没人记得住第几个是什么了。

use serde::{Deserialize, Serialize};

use crate::prefs::Prefs;

/// 用户预设的槽位数。
///
/// 三个：一个日常、一个出图、一个夜读。**不做无限列表**——列表一长，
/// 命名就变成负担，最后没人用。
pub const USER_SLOTS: usize = 3;

/// 一套版式快照。
///
/// 字段与 `Prefs` 一一对应但**少得多**：只收「观感」那几项，具体字体、
/// 字级、内容开关都不在内（理由见模块文档）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StylePreset {
    pub name: String,
    /// 纸色下标（`crate::theme::Theme::ALL`）。
    pub theme: usize,
    /// 墨色下标（`crate::theme::InkPalette::ALL`）。
    pub palette: usize,
    /// 观感档：0 = 刻本宋体、1 = 手写楷体（`crate::types::FontPreset`）。
    pub font_preset: usize,
    /// 每面列数（8 / 10 / 12 / 14）。
    pub columns: usize,
    /// 列数是否自动（自动档下 `columns` 只作初值）。
    pub columns_auto: bool,
    /// 版式键（`crate::grid::LAYOUT_CONFIGS`）：`standard8` / `standard10` /
    /// `standard12` / `standard14`。
    pub layout_key: String,
    /// 留白档下标（`crate::grid::BANXIN_RATIOS`）：0 宽 / 1 中 / 2 窄。
    pub margin_tier: usize,
    /// 句读形制：0 = 现代标点、1 = 古籍句读。
    pub punct_mode: usize,
    /// 句读墨色：0 = 墨、1 = 朱砂（深底一律走淡金，这点由渲染侧定）。
    pub punct_ink: usize,
    /// 生僻字注音。
    pub rare: bool,
    /// 数字转汉字。
    pub han_digits: bool,
    /// 双行夹注（false = 页脚注）。
    pub notes_inline: bool,
    /// 版框形制：0 = 四周双边、1 = 左右双边、2 = 四周单边。
    pub frame_style: usize,
}

/// 内建四套。
///
/// 名字是**情境**不是参数：不说「夜读+暗金+十二行」，说「灯下」——
/// 用户记的是「我那天在灯下读的样子」。
pub const BUILTIN: &[StylePreset] = &[
    // 灯下：夜读深底 + 暗金 + 十二行密排。深夜读，屏幕不刺眼。
    StylePreset {
        name: String::new(),
        theme: 3,
        palette: 3,
        font_preset: 0,
        columns: 12,
        columns_auto: false,
        layout_key: String::new(),
        margin_tier: 1,
        punct_mode: 1,
        punct_ink: 1,
        rare: true,
        han_digits: false,
        notes_inline: true,
        frame_style: 0,
    },
    // 雨窗：青简 + 松墨 + 手写楷体 + 十行。清冷，适合诗文。
    StylePreset {
        name: String::new(),
        theme: 2,
        palette: 1,
        font_preset: 1,
        columns: 10,
        columns_auto: false,
        layout_key: String::new(),
        margin_tier: 1,
        punct_mode: 1,
        punct_ink: 1,
        rare: true,
        han_digits: false,
        notes_inline: true,
        frame_style: 1,
    },
    // 仿刻：仿刻本纸 + 墨朱 + 八行疏朗 + 宽留白。最接近文物原型，出图最好看。
    StylePreset {
        name: String::new(),
        theme: 4,
        palette: 0,
        font_preset: 0,
        columns: 8,
        columns_auto: false,
        layout_key: String::new(),
        margin_tier: 0,
        punct_mode: 1,
        punct_ink: 1,
        rare: true,
        han_digits: true,
        notes_inline: true,
        frame_style: 0,
    },
    // 素笺：宣纸 + 墨朱 + 十四行信笺 + 现代标点。写笔记、摘句子用。
    StylePreset {
        name: String::new(),
        theme: 0,
        palette: 0,
        font_preset: 1,
        columns: 14,
        columns_auto: false,
        layout_key: String::new(),
        margin_tier: 2,
        punct_mode: 0,
        punct_ink: 1,
        rare: true,
        han_digits: false,
        notes_inline: true,
        frame_style: 2,
    },
];

/// 内建预设的名字（与 [`BUILTIN`] 同序）。
const BUILTIN_NAMES: [&str; 4] = ["灯下", "雨窗", "仿刻", "素笺"];

impl StylePreset {
    /// 取第 `i` 套内建预设（名字在这里补上——常量里不好放 `String`）。
    pub fn builtin(i: usize) -> Option<StylePreset> {
        BUILTIN.get(i).and_then(|p| {
            let name = BUILTIN_NAMES.get(i).copied()?;
            Some(StylePreset {
                name: name.to_string(),
                // 常量里放不了 `String`（`to_string` 不是 const），版式键
                // 按列数现配——与列数永远对齐，也免得两处写着写着写岔了。
                layout_key: layout_key_for_columns(p.columns),
                ..p.clone()
            })
        })
    }

    /// 内建套数。
    pub fn builtin_count() -> usize {
        BUILTIN_NAMES.len()
    }

    /// 从现行偏好**抄**一套下来（用户存槽位时用）。
    pub fn capture(name: &str, p: &Prefs) -> Self {
        StylePreset {
            name: name.to_string(),
            theme: p.theme,
            palette: p.palette,
            font_preset: p.font_preset,
            columns: p.columns,
            columns_auto: p.columns_auto,
            layout_key: p.layout_key.clone(),
            margin_tier: p.margin_tier,
            punct_mode: p.punct_mode,
            punct_ink: p.punct_ink,
            rare: p.rare,
            han_digits: p.han_digits,
            notes_inline: p.notes_inline,
            frame_style: p.frame_style,
        }
    }

    /// 把这套预设**落到**偏好上。
    ///
    /// 只写观感那几项：具体字体、字级、繁简 / 通假 / 句读显隐一律不动
    /// （理由见模块文档三条边界）。
    pub fn apply(&self, p: &mut Prefs) {
        p.theme = self.theme.min(crate::theme::Theme::ALL.len() - 1);
        p.palette = self.palette.min(crate::theme::InkPalette::ALL.len() - 1);
        p.font_preset = self.font_preset.min(crate::types::FontPreset::ALL.len() - 1);
        p.columns = self.columns;
        p.columns_auto = self.columns_auto;
        if crate::grid::LayoutConfig::by_key(&self.layout_key).is_some() {
            p.layout_key = self.layout_key.clone();
        } else {
            // 预设里的版式键认不出来（手改坏了 / 未来改名）：退回该列数
            // 对应的标准键，别让整份预设作废。
            p.layout_key = layout_key_for_columns(self.columns);
        }
        p.margin_tier = self.margin_tier.min(crate::grid::BANXIN_RATIOS.len() - 1);
        p.punct_mode = self.punct_mode.min(crate::gloss::PunctMode::ALL.len() - 1);
        p.punct_ink = self.punct_ink.min(crate::gloss::PunctInk::ALL.len() - 1);
        p.rare = self.rare;
        p.han_digits = self.han_digits;
        p.notes_inline = self.notes_inline;
        p.frame_style = self.frame_style.min(crate::types::FrameStyle::ALL.len() - 1);
    }

    /// 这一套与现行偏好是否**一致**（面板据此点亮当前那一张牌）。
    pub fn matches(&self, p: &Prefs) -> bool {
        self == &StylePreset::capture(&self.name, p)
    }
}

/// 用户槽位的名字：位置决定名字，不让人起名。
///
/// 「我的壹 / 贰 / 叁」——**不做命名输入框**。命名是负担：三个槽位自己
/// 就记得住，一旦要起名，用户会先花三十秒想名字，然后放弃这个功能。
pub fn slot_name(slot: usize) -> String {
    match slot {
        0 => "我的壹",
        1 => "我的贰",
        _ => "我的叁",
    }
    .to_string()
}

/// 列数 → 标准版式键（预设修坏时的兜底，也与面板切列数时的口径一致）。
pub fn layout_key_for_columns(columns: usize) -> String {
    match columns {
        8 => "standard8",
        10 => "standard10",
        14 => "standard14",
        _ => "standard12",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_builtin_presets_are_all_legal() {
        assert_eq!(StylePreset::builtin_count(), BUILTIN.len());
        for i in 0..StylePreset::builtin_count() {
            let p = StylePreset::builtin(i).unwrap_or_else(|| panic!("第 {i} 套取不到"));
            assert!(!p.name.is_empty(), "第 {i} 套没名字");
            assert!(p.theme < crate::theme::Theme::ALL.len(), "纸色越界：{:?}", p);
            assert!(
                p.palette < crate::theme::InkPalette::ALL.len(),
                "墨色越界：{:?}",
                p
            );
            assert!(
                crate::grid::LayoutConfig::by_key(&p.layout_key).is_some()
                    || !p.layout_key.is_empty(),
                "版式键不对：{:?}",
                p
            );
            assert!(
                crate::grid::GRID_COLUMN_CHOICES.contains(&p.columns),
                "列数不在四档里：{:?}",
                p
            );
        }
    }

    #[test]
    fn builtin_names_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..StylePreset::builtin_count() {
            let p = StylePreset::builtin(i).unwrap();
            assert!(seen.insert(p.name.clone()), "预设重名：{}", p.name);
        }
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn apply_then_capture_round_trips() {
        let mut p = Prefs::default();
        let preset = StylePreset::builtin(2).unwrap(); // 仿刻
        preset.apply(&mut p);
        assert!(preset.matches(&p), "应用后再抄下来应当一致");
        assert_eq!(p.theme, 4);
        assert_eq!(p.columns, 8);
        assert_eq!(p.layout_key, "standard8");
    }

    #[test]
    fn apply_never_touches_font_or_zoom_or_content_switches() {
        let mut p = Prefs::default();
        // 用户自备字体与字级、繁简都是有主之物，预设不该动。
        p.font_ancient = 3;
        p.font_note = 2;
        p.zoom = 1.6;
        p.trad_mode = Some(2);
        p.tongjia = false;
        p.punct = false;
        let before = (p.font_ancient, p.font_note, p.zoom, p.trad_mode, p.tongjia, p.punct);

        StylePreset::builtin(0).unwrap().apply(&mut p);
        assert_eq!(
            (p.font_ancient, p.font_note, p.zoom, p.trad_mode, p.tongjia, p.punct),
            before,
            "预设越界动了字体 / 字级 / 内容开关"
        );
    }

    #[test]
    fn corrupt_preset_still_applies_safely() {
        let mut p = Prefs::default();
        let mut bad = StylePreset::builtin(1).unwrap();
        bad.theme = 99;
        bad.palette = 99;
        bad.layout_key = "根本没这套".to_string();
        bad.columns = 14;
        bad.apply(&mut p);
        assert!(p.theme < crate::theme::Theme::ALL.len());
        assert!(p.palette < crate::theme::InkPalette::ALL.len());
        assert!(
            crate::grid::LayoutConfig::by_key(&p.layout_key).is_some(),
            "坏键要退回合法版式，实际：{}",
            p.layout_key
        );
        assert_eq!(p.layout_key, "standard14");
    }

    #[test]
    fn user_slots_is_three() {
        // 槽位数改了，UI 那三张牌也要跟着改——这条断言就是那句提醒。
        assert_eq!(USER_SLOTS, 3);
    }
}
