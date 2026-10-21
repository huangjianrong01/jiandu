//! 阅读偏好（全局设置）的本地持久化。
//!
//! 落在 `~/.jiandu/prefs.json`：主题、墨色组、字体、字级、列数、留白、
//! 夹注模式、繁简、标注开关、翻页音效开关。下次打开照单恢复，
//! 不必每次重调一遍。
//!
//! 与书架档案分工：`library.json` 记**每本书**的进度 / 笔记 / 划线，
//! `prefs.json` 记**跨书**的阅读环境与版式偏好。
//!
//! 枚举一律存**下标**（`Theme::ALL` / `InkPalette::ALL` / `FontChoice::CLASSIC`
//! 的序号），不存名字——改个标签不会让老存档失效；下标越界则回落到默认档。

use crate::library::Library;
use crate::theme::{InkPalette, Theme};
use crate::types::{FontChoice, FontPreset};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prefs {
    /// `Theme::ALL` 下标。
    pub theme: usize,
    /// `InkPalette::ALL` 下标。
    pub palette: usize,
    /// 古文（竖笺）字体：`FontChoice::CLASSIC` 下标。
    pub font_ancient: usize,
    /// 现代（横笺）字体：`FontChoice::MODERN` 下标。
    pub font_modern: usize,
    /// **批注字体**（夹注小字 / 放大注卡）：`FontChoice::NOTE` 下标。
    #[serde(default)]
    pub font_note: usize,
    /// **排版字体**（刻本宋体 / 手写楷体）：`FontPreset::ALL` 下标。
    #[serde(default)]
    pub font_preset: usize,
    /// 字级倍率（1.0 = 中）。
    pub zoom: f32,
    /// 每面列数（8 / 10 / 12 / 14）。
    pub columns: usize,
    /// 四边留白比例。
    pub margin: f32,
    /// true = 信笺（14列）模板：单张平铺稿纸，中缝占两格、立对鱼尾。
    pub letter: bool,
    /// 当前版式配置键（见 `crate::grid::LAYOUT_CONFIGS`）：数据驱动切换的句柄。
    pub layout_key: String,
    /// true = 双行夹注；false = 页脚注。
    pub notes_inline: bool,
    /// true = 繁体。
    pub trad: bool,
    /// 通假标注。
    pub tongjia: bool,
    /// 句读标点。
    pub punct: bool,
    /// 生僻字注音。
    pub rare: bool,
    /// 数字转汉字：阿拉伯数字在竖排里显示为汉字（一六二八）。
    pub han_digits: bool,
    /// 响应式单 / 双叶：按屏幕宽高比自动决定对开 / 单叶。
    #[serde(default = "default_true")]
    pub auto_duplex: bool,
    /// 注释显示。
    pub notes: bool,
    /// 翻页音效。
    pub sound: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: 0,
            palette: 0,
            font_ancient: 0,
            font_modern: 0,
            font_note: 0,
            font_preset: 0,
            zoom: 1.0,
            columns: 10,
            margin: 0.06,
            letter: false,
            layout_key: "standard10".to_string(),
            notes_inline: true,
            trad: true,
            tongjia: true,
            punct: true,
            rare: true,
            han_digits: false,
            auto_duplex: true,
            notes: true,
            sound: true,
        }
    }
}

/// 「默认开」的开关：老存档里没有这一项时按 true 回落（响应式开合是默认体验）。
fn default_true() -> bool {
    true
}

/// 按下标取枚举元素，越界回落到首项。
fn pick<T: Copy>(all: &[T], i: usize) -> T {
    all.get(i).copied().unwrap_or(all[0])
}

impl Prefs {
    pub fn path() -> PathBuf {
        Library::home().join("prefs.json")
    }

    /// 读偏好；文件缺失 / 损坏一律回落默认值（不让坏存档挡住启动）。
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// 写偏好（失败不影响使用）。
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(Library::home());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }

    pub fn theme(&self) -> Theme {
        pick(&Theme::ALL, self.theme)
    }
    pub fn palette(&self) -> InkPalette {
        pick(&InkPalette::ALL, self.palette)
    }
    pub fn font_ancient(&self) -> FontChoice {
        pick(&FontChoice::CLASSIC, self.font_ancient)
    }
    pub fn font_modern(&self) -> FontChoice {
        pick(&FontChoice::MODERN, self.font_modern)
    }
    pub fn font_note(&self) -> FontChoice {
        pick(&FontChoice::NOTE, self.font_note)
    }
    /// 排版字体（刻本宋体 / 手写楷体）。
    pub fn font_preset(&self) -> FontPreset {
        pick(&FontPreset::ALL, self.font_preset)
    }

    /// 从当前设置反向填好存档（存下标，故需反查在枚举里的位置）。
    pub fn capture(
        theme: Theme,
        palette: InkPalette,
        font_ancient: FontChoice,
        font_modern: FontChoice,
        font_note: FontChoice,
    ) -> (usize, usize, usize, usize, usize) {
        let idx = |all: &[Theme], v: Theme| all.iter().position(|x| *x == v).unwrap_or(0);
        let pidx =
            |all: &[InkPalette], v: InkPalette| all.iter().position(|x| *x == v).unwrap_or(0);
        let fidx =
            |all: &[FontChoice], v: FontChoice| all.iter().position(|x| *x == v).unwrap_or(0);
        (
            idx(&Theme::ALL, theme),
            pidx(&InkPalette::ALL, palette),
            fidx(&FontChoice::CLASSIC, font_ancient),
            fidx(&FontChoice::MODERN, font_modern),
            fidx(&FontChoice::NOTE, font_note),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let p = Prefs::default();
        let s = serde_json::to_string(&p).expect("序列化");
        let back: Prefs = serde_json::from_str(&s).expect("反序列化");
        assert_eq!(back.zoom, p.zoom);
        assert_eq!(back.theme, p.theme);
        assert!(back.notes_inline);
    }

    #[test]
    fn out_of_range_index_falls_back_to_first() {
        // 老存档里的下标可能因枚举扩容而越界：必须回落，不能 panic。
        let mut p = Prefs::default();
        p.theme = 99;
        p.font_ancient = 99;
        assert_eq!(p.theme(), Theme::ALL[0]);
        assert_eq!(p.font_ancient(), FontChoice::CLASSIC[0]);
        // 思源黑体进了典籍组，取得到（用户要的字体确实可选）。
        let i = FontChoice::CLASSIC
            .iter()
            .position(|f| *f == FontChoice::SourceHanSans)
            .expect("典籍字体组应含思源黑体");
        p.font_ancient = i;
        assert_eq!(p.font_ancient(), FontChoice::SourceHanSans);
    }

    #[test]
    fn capture_maps_enums_back_to_indices() {
        let (t, p, fa, fm, fn_) = Prefs::capture(
            Theme::Ye,
            InkPalette::Gold,
            FontChoice::SourceHanSans,
            FontChoice::SystemDefault,
            FontChoice::LXGWWenKai,
        );
        assert_eq!(Theme::ALL[t], Theme::Ye);
        assert_eq!(InkPalette::ALL[p], InkPalette::Gold);
        assert_eq!(FontChoice::CLASSIC[fa], FontChoice::SourceHanSans);
        assert_eq!(FontChoice::MODERN[fm], FontChoice::SystemDefault);
        assert_eq!(FontChoice::NOTE[fn_], FontChoice::LXGWWenKai);
    }

    #[test]
    fn old_prefs_without_new_fields_still_load() {
        // 老存档没有 `font_note` / `auto_duplex` / `font_preset`：不能整份
        // 反序列化失败（一失败就 `unwrap_or_default()`，用户所有偏好全丢）。
        let old = r#"{"theme":1,"palette":0,"font_ancient":0,"font_modern":0,"zoom":1.0,
            "columns":10,"margin":0.06,"letter":false,"layout_key":"standard10",
            "notes_inline":true,"trad":true,"tongjia":true,"punct":true,"rare":true,
            "han_digits":false,"notes":true,"sound":true}"#;
        let p: Prefs = serde_json::from_str(old).expect("老存档应能读进来");
        assert!(p.auto_duplex, "响应式开合默认开");
        assert_eq!(p.font_note(), FontChoice::NOTE[0]);
        assert_eq!(p.font_preset(), FontPreset::Song);
    }

    #[test]
    fn font_preset_round_trips() {
        // 排版字体：默认「刻本宋体」，切到「手写楷体」后存回来还是它。
        let mut p = Prefs::default();
        assert_eq!(p.font_preset(), FontPreset::Song);
        p.font_preset = FontPreset::Kai.index();
        assert_eq!(p.font_preset(), FontPreset::Kai);
        assert_eq!(FontPreset::from_index(p.font_preset), FontPreset::Kai);
        // 越界回落首项，不 panic。
        assert_eq!(FontPreset::from_index(99), FontPreset::Song);
    }
}
