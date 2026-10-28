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

/// 缺 `punct_ink` 时的落点：**朱砂**（`PunctInk::ALL[1]`）。
///
/// 单独写成函数是因为 serde 的 `default = "..."` 要一条**路径**，
/// 不能直接写 `1`；也让「为什么不是 0」这件事在代码里有名有姓。
fn cinnabar() -> usize {
    1
}

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
    ///
    /// v1.84.0 起出厂是**霞鹜文楷**（`default_font_note`）：老存档没有这一项
    /// 时也按它回落（原先是玄冬楷书）。
    #[serde(default = "default_font_note")]
    pub font_note: usize,
    /// **排版字体**（刻本宋体 / 手写楷体）：`FontPreset::ALL` 下标。
    #[serde(default)]
    pub font_preset: usize,
    /// 字级倍率（1.0 = 中）。
    pub zoom: f32,
    /// 每面列数（8 / 10 / 12 / 14）。
    ///
    /// **只在 `columns_auto == false` 时生效**——自动档每帧按「书的内容 +
    /// 可用宽」重算列数，这个字段仅作手动档的记忆与自动档的初值。
    pub columns: usize,
    /// **列数是否自动**（true = 自动档）。
    ///
    /// 自动档同时看两头：这本书**平均一章多少字**（短章疏排、长篇密排）与
    /// **当前可用宽**（字级落在舒适区）。见 `grid::auto_columns`。
    ///
    /// 老存档没有这一项 → `default_true`。这正是升级的迁移点：旧存的
    /// 8 / 10 列不再把新密度盖住，一进来就按内容重算；想锁死就手动切一次
    /// 列数（手动切换会把这儿置 false）。
    #[serde(default = "default_true")]
    pub columns_auto: bool,
    /// 四边留白比例。
    pub margin: f32,
    /// **留白档**下标（[`crate::grid::BANXIN_RATIOS`]）：0 = 宽（复古 60%）、
    /// 1 = 中（标准 75%，默认）、2 = 窄（满屏 90%）。
    ///
    /// 版心（界行文字区 + 版框）宽 ÷ 单叶可用宽的比例——宋版书版心只占纸宽
    /// 六成，故「宽」档最接近文物原型；「窄」档让文字区铺满叶面。宽档联动
    /// 更大的天头（文物感），见 `crate::grid::top_frac_for`。
    ///
    /// 老存档没有这一项 → 回落默认档「中」（`default_margin_tier`）。
    #[serde(default = "default_margin_tier")]
    pub margin_tier: usize,
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
    /// **渲染模式**下标（[`crate::grid::LayoutMode::ALL`]）：
    /// 0 = 对开翻阅、1 = 手机竖屏单叶、2 = 整页等比缩放。
    #[serde(default)]
    pub layout_mode: usize,
    /// 注释显示。
    pub notes: bool,
    /// 翻页音效。
    pub sound: bool,
    /// **句读模式**下标（[`crate::gloss::PunctMode::ALL`]）：
    /// 0 = 现代标点、1 = 古籍句读（句画「〇」、疑叹画大斜点）。
    ///
    /// 老存档没有这一项 → 0（现代），升级后读者自己切，不擅自改人家的版。
    #[serde(default)]
    pub punct_mode: usize,
    /// **句读墨色**下标（[`crate::gloss::PunctInk::ALL`]）：0 = 墨、1 = 朱砂。
    ///
    /// 只在**浅纸**上分这两档：深底（暗金 / 夜读）一律走淡金，朱砂压在
    /// 深底上会发闷，也压过了正文的月白。
    ///
    /// ⚠️ **必须显式指定默认函数**：这一项是 v1.66.0 才加的，之前所有存档
    /// 都没有它；光写 `#[serde(default)]` 会落到 `usize::default()` = **0**，
    /// 也就是「墨」——升级后句读会**悄悄从朱笔变墨笔**，而 `Default for Prefs`
    /// 里明明写着 1。用户那句「是不是朱砂红？」问的就是这个。
    #[serde(default = "cinnabar")]
    pub punct_ink: usize,
    /// **繁简显示档**（v1.81.0）：0 = 原文、1 = 简体、2 = 繁体。
    ///
    /// 老存档没有这一项（`None`）→ 由下面那个旧的 `trad` 布尔推：true 当繁体、
    /// false 当简体，与升级前的观感**完全一致**。新存档一律写这一项。
    #[serde(default)]
    pub trad_mode: Option<usize>,
    /// **回锋**（v1.80.0）：读那一撇的出锋末端带不带一小勾。
    #[serde(default)]
    pub punct_hook: bool,
    /// **改过的快捷键**（v1.81.0）：`(动作 id, 键位串)`，只存改过的那些。
    ///
    /// 出厂键位在 [`crate::keys::Action::default_chord`] 里，这里留空即「全按
    /// 出厂」。落 `Vec` 而不是 map，是为了让 JSON 保持数组形态、老版本读到时
    /// 也不会因为多出一个对象字段而整份作废。
    #[serde(default)]
    pub keys: Vec<(String, String)>,
    /// 窄屏（手机竖持 / 窗口拉窄）首次提示已看过（v1.52.3）：
    /// 「竖排自右向左，纸面贴右缘」只说一次，跨会话不再唠叨。
    /// 老存档没有这一项 → false（升级后第一次进窄屏会看到）。
    #[serde(default)]
    pub narrow_hint_seen: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: 0,
            palette: 0,
            // v1.84.0：正文出厂**清刻本悦宋**、批注出厂**霞鹜文楷**。
            font_ancient: default_font_ancient(),
            font_modern: 0,
            font_note: default_font_note(),
            font_preset: 0,
            zoom: 1.0,
            columns: 12,
            // 自动档默认开：列数按书的内容与视口算，不写死某一档。
            columns_auto: true,
            margin: 0.06,
            // 留白默认「中（标准 75%）」：既留古籍两侧留白的优雅，又保证
            // 现代屏幕上的阅读体验（用户 v1.33.0 定稿）。
            margin_tier: crate::grid::BANXIN_TIER_DEFAULT,
            // v1.28.0：默认版式由 10 列改为 12 列（密度）。旧存档里的
            // 自选列数仍照读，只是新装 / 重置时落到 12 列。
            layout_key: "standard12".to_string(),
            notes_inline: true,
            trad: true,
            tongjia: true,
            punct: true,
            rare: true,
            han_digits: false,
            auto_duplex: true,
            layout_mode: 0,
            notes: true,
            sound: true,
            punct_mode: 0,
            // 朱砂（v1.66.0 用户定）：句读一直是朱笔点的，升级不该悄悄换色。
            punct_ink: 1,
            trad_mode: None,
            punct_hook: false,
            keys: Vec::new(),
            narrow_hint_seen: false,
        }
    }
}

/// 「默认开」的开关：老存档里没有这一项时按 true 回落（响应式开合是默认体验）。
fn default_true() -> bool {
    true
}

/// **出厂正文**（v1.84.0 用户定稿）：**方正清刻本悦宋**——真·清刻本匠体，
/// 横细竖粗、字面略窄，与「仿古籍精刻本」纸是一对。
///
/// 存的是 `FontChoice::CLASSIC` 下标：这里按名字反查，往后加字体也不会
/// 把默认值顶歪。没装这款时渲染沿回退链落宋体（见 `fonts::pick_font`）。
fn default_font_ancient() -> usize {
    FontChoice::CLASSIC
        .iter()
        .position(|c| *c == FontChoice::FZQingKeBen)
        .unwrap_or(0)
}

/// **出厂批注**：**霞鹜文楷**（`FontChoice::NOTE` 下标）——正文匠体、
/// 注文手写体，正是刻本的层次。
fn default_font_note() -> usize {
    FontChoice::NOTE
        .iter()
        .position(|c| *c == FontChoice::LXGWWenKai)
        .unwrap_or(0)
}

/// 留白档默认值：「中（标准 75%）」。老存档没有 `margin_tier` 时回落这里。
fn default_margin_tier() -> usize {
    crate::grid::BANXIN_TIER_DEFAULT
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
    /// 句读模式（现代标点 / 古籍句读）。
    pub fn punct_mode(&self) -> crate::gloss::PunctMode {
        pick(&crate::gloss::PunctMode::ALL, self.punct_mode)
    }
    /// 繁简显示档（原文 / 简体 / 繁体）。老存档由 `trad` 布尔推。
    pub fn trad_mode(&self) -> crate::trad::TradMode {
        match self.trad_mode {
            Some(i) => pick(&crate::trad::TradMode::ALL, i),
            None => {
                if self.trad {
                    crate::trad::TradMode::Traditional
                } else {
                    crate::trad::TradMode::Simplified
                }
            }
        }
    }
    /// 句读墨色（墨 / 朱砂）。
    pub fn punct_ink(&self) -> crate::gloss::PunctInk {
        pick(&crate::gloss::PunctInk::ALL, self.punct_ink)
    }

    /// 只把**繁简档**写盘（不动其它字段）。
    ///
    /// 全仓没有「界面 → Prefs」的总收口，所以每一处改设置都得自己写这一下；
    /// 写成静态方法，免得调用方再去拼一遍下标。
    pub fn save_trad(m: crate::trad::TradMode) {
        let mut p = Self::load();
        p.trad_mode = crate::trad::TradMode::ALL.iter().position(|x| *x == m);
        p.trad = m == crate::trad::TradMode::Traditional;
        p.save();
    }

    /// 留白档 → 版心率（0.60 / 0.75 / 0.90）。下标越界回落默认档，
    /// 绝不 panic（老存档 / 手改档都能安全读）。
    pub fn banxin_ratio(&self) -> f32 {
        crate::grid::BANXIN_RATIOS
            .get(self.margin_tier)
            .copied()
            .unwrap_or(crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT])
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

    /// **老存档缺 `punct_ink` 必须落到朱砂**（v1.83.1）。
    ///
    /// 这一项 v1.66.0 才加，之前所有 prefs.json 里都没有它。原来写的是
    /// `#[serde(default)]`，落到 `usize::default()` = 0 = **墨**——于是
    /// 所有老用户升级后句读从朱笔悄悄变墨笔（用户原话：「是不是朱砂红？」）。
    /// 这条测试把「缺项 = 朱砂」钉死，`Default for Prefs` 与 serde 两条路
    /// 从此必须同口径。
    #[test]
    fn missing_punct_ink_falls_back_to_cinnabar() {
        let mut v = serde_json::to_value(Prefs::default()).expect("序列化");
        v.as_object_mut().expect("对象").remove("punct_ink");
        let p: Prefs = serde_json::from_value(v).expect("反序列化");
        assert_eq!(
            p.punct_ink(),
            crate::gloss::PunctInk::Zhu,
            "老存档缺 punct_ink 落到了墨色——句读不该悄悄换笔"
        );
        // 内存默认值与 serde 缺省值必须是同一个档。
        assert_eq!(Prefs::default().punct_ink(), crate::gloss::PunctInk::Zhu);
    }

    #[test]
    fn out_of_range_index_falls_back_to_first() {
        // 老存档里的下标可能因枚举扩容而越界：必须回落，不能 panic。
        let p = Prefs {
            theme: 99,
            font_ancient: 99,
            ..Prefs::default()
        };
        assert_eq!(p.theme(), Theme::ALL[0]);
        assert_eq!(p.font_ancient(), FontChoice::CLASSIC[0]);
        // 思源黑体进了典籍组，取得到（用户要的字体确实可选）。
        let i = FontChoice::CLASSIC
            .iter()
            .position(|f| *f == FontChoice::SourceHanSans)
            .expect("典籍字体组应含思源黑体");
        let q = Prefs {
            font_ancient: i,
            ..Prefs::default()
        };
        assert_eq!(q.font_ancient(), FontChoice::SourceHanSans);
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
            "columns":10,"margin":0.06,"layout_key":"standard10",
            "notes_inline":true,"trad":true,"tongjia":true,"punct":true,"rare":true,
            "han_digits":false,"notes":true,"sound":true}"#;
        let p: Prefs = serde_json::from_str(old).expect("老存档应能读进来");
        assert!(p.auto_duplex, "响应式开合默认开");
        // v1.84.0：批注出厂改成霞鹜文楷，老存档缺这一项时同样按新默认值落
        // （原来是玄冬楷书）。
        assert_eq!(p.font_note(), FontChoice::LXGWWenKai);
        assert_eq!(p.font_preset(), FontPreset::Song);
    }

    /// v1.84.0 定稿：出厂**正文清刻本悦宋、批注霞鹜文楷**。
    ///
    /// 出厂值走「按名字反查下标」而不是写死数字——往后往 `CLASSIC` 队尾
    /// 加字体也不会把默认值顶歪。
    #[test]
    fn factory_fonts_are_qingkeben_for_body_and_wenkai_for_notes() {
        let p = Prefs::default();
        assert_eq!(p.font_ancient(), FontChoice::FZQingKeBen, "正文出厂应为清刻本悦宋");
        assert_eq!(p.font_note(), FontChoice::LXGWWenKai, "批注出厂应为霞鹜文楷");
        // 「刻本宋体」预设的注文字体正是霞鹜文楷，两处不许打架。
        assert_eq!(FontPreset::Song.note_choice(), FontChoice::LXGWWenKai);
        assert_eq!(FontPreset::for_main(FontChoice::FZQingKeBen), FontPreset::Song);
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

    #[test]
    fn narrow_hint_seen_defaults_false_and_round_trips() {
        // 窄屏提示只说一次：默认没看过；老存档没有这一项也按没看过（升级后
        // 第一次进窄屏会看到那句「竖排自右向左，纸面贴右缘」）。
        let p = Prefs::default();
        assert!(!p.narrow_hint_seen);
        let old = r#"{"theme":0,"palette":0,"font_ancient":0,"font_modern":0,"zoom":1.0,
            "columns":12,"margin":0.06,"layout_key":"standard12",
            "notes_inline":true,"trad":true,"tongjia":true,"punct":true,"rare":true,
            "han_digits":false,"notes":true,"sound":true}"#;
        let q: Prefs = serde_json::from_str(old).expect("老存档应能读进来");
        assert!(!q.narrow_hint_seen, "老存档按未看过处理");
        let s = serde_json::to_string(&p).expect("序列化");
        let back: Prefs = serde_json::from_str(&s).expect("反序列化");
        assert!(!back.narrow_hint_seen);
    }

    #[test]
    fn margin_tier_defaults_to_standard_and_round_trips() {
        // 留白档：默认「中（75%）」——既留古籍两侧留白的优雅，又保证现代
        // 屏幕上的阅读体验（用户 v1.33.0 定稿）。
        let p = Prefs::default();
        assert_eq!(p.margin_tier, crate::grid::BANXIN_TIER_DEFAULT);
        assert!((p.banxin_ratio() - 0.75).abs() < 1e-6);
        // 三档都取得到，且宽 < 中 < 窄（版心越来越满）。
        for (i, r) in crate::grid::BANXIN_RATIOS.iter().enumerate() {
            let q = Prefs {
                margin_tier: i,
                ..Prefs::default()
            };
            assert!((q.banxin_ratio() - r).abs() < 1e-6);
        }
        // 越界回落默认档，不 panic。
        let q = Prefs {
            margin_tier: 99,
            ..Prefs::default()
        };
        assert!((q.banxin_ratio() - 0.75).abs() < 1e-6);
    }

    #[test]
    fn old_prefs_without_margin_tier_falls_back_to_standard() {
        // 老存档没有 `margin_tier`：必须能读进来，且留白档回落「中」。
        let old = r#"{"theme":0,"palette":0,"font_ancient":0,"font_modern":0,"zoom":1.0,
            "columns":12,"margin":0.06,"layout_key":"standard12",
            "notes_inline":true,"trad":true,"tongjia":true,"punct":true,"rare":true,
            "han_digits":false,"notes":true,"sound":true}"#;
        let p: Prefs = serde_json::from_str(old).expect("老存档应能读进来");
        assert!((p.banxin_ratio() - 0.75).abs() < 1e-6, "老存档留白档应回落 75%");
    }
}
