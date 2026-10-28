//! **平台能力抽象层**（v1.0.0-beta · 为 Windows / iOS / Android 做准备）。
//!
//! 一句话分工：**core 说「要印什么」，壳子说「怎么印」**。
//!
//! 本模块只放**接口**与**与平台无关的数据**，不含一行绘制代码。壳子按自己的
//! 本事注入实现：
//!
//! ```text
//!   macOS     → CoreGraphics（矢量 PDF，真字内嵌）      crates/app/src/pdf.rs
//!   Windows   → 待接：printpdf / pdf-writer（纯 Rust）
//!   iOS/安卓  → 待接：宿主 App 的绘制 API，或同一套纯 Rust 后端
//! ```
//!
//! 由此换来一件事：**排版结果（`gloss::Page`）是全平台同一份**。
//! 换壳子只是换「落笔的人」，不换「排出来的叶」。
//!
//! ⚠️ 铁律：本文件不得出现 `use gpui` / `use cocoa` / `use objc`，
//! 也不得引入任何平台 crate。校验见 `scripts/check_core_purity.sh`。

use crate::gloss::Page;

// ─────────────────────────────────────────────────────────────────────────────
// 一、PDF 印制
// ─────────────────────────────────────────────────────────────────────────────

/// 正文用哪种字体的**语义类**（不是具体字体名）。
///
/// 为什么不当字符串传：字体名是平台方言（macOS 认 `Songti SC`，Windows 认
/// `SimSun`），同一个「宋」在两个系统上得落到不同的字。core 只说「要宋体」，
/// 由壳子翻译成自己系统里的 PostScript / 家族名。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PdfFontKind {
    /// 宋体（刻本正统）。
    #[default]
    Song,
    /// 楷体（手写气，注文传统上用楷）。
    Kai,
    /// 黑体（现代极简）。
    Hei,
}

/// 一整份 PDF 共用的版式与配色（**纯数据**，零平台概念）。
#[derive(Clone, Debug)]
pub struct PdfGeometry {
    /// 列数（8 / 10 / 12 / 14）。
    pub cols: usize,
    /// 字格宽（= 列宽）。
    pub cell_w: f32,
    /// 字格高。
    pub cell_h: f32,
    /// 每列行数。
    pub rows: usize,
    /// 正文字号。
    pub fs: f32,
    /// 章首页开卷列顶部预留给卷首题署的行数。
    pub reserve: usize,
    /// 版框双边预留（纸面外缘 → 字格区）。
    pub edge: f32,
    /// 天头（地脚 = 天头 ÷ 1.5，三大铁律之一）。
    pub margin_top: f32,
    /// 地脚（含叶码条）。
    pub margin_bot: f32,
    /// 纸色 / 墨色 / 界行色 / 注文色 / 朱砂（皆 `0xRRGGBB`）。
    pub paper: u32,
    pub ink: u32,
    pub rule: u32,
    /// **内框与界行**的取色（v1.83.0）——版框外层仍用 `rule`。
    ///
    /// 刻本的层次是「外框最重、内框与界行退一档」；旧制两者同色，印出来
    /// 是「一圈粗细不匀的框」。非仿刻本主题由壳子传**与 `rule` 相同的值**
    /// （见 `Theme::rule_line`），所以老版面的 PDF 一个像素都不变。
    pub rule_soft: u32,
    pub note_ink: u32,
    pub accent: u32,
    /// 正文 / 注文字体的**语义类**，由壳子翻译成具体字体。
    pub font: PdfFontKind,
    pub note_font: PdfFontKind,
    /// 「数字转汉字」：开了则每个数字独立一格（与分页同一开关）。
    pub han_digits: bool,
    /// 繁体显示（与阅读器的繁简开关同源）。
    pub traditional: bool,
    /// **句读模式**（现代标点 / 古籍句读，v1.81.0）。
    ///
    /// 纸上与屏幕必须同一个口径：`gloss::jodu_of` 是唯一数据源，壳子只照
    /// 它挑画法（同尺寸同位置），故 PDF 里的「〇」与屏幕上逐字同位。
    pub punct_mode: crate::gloss::PunctMode,
    /// **句读墨色**（已解析成 `0xRRGGBB`）。
    ///
    /// 由壳子按 `PunctInk::color(palette, theme)` 算好传进来——core 不认识
    /// 主题 / 墨色组，也不该认识。
    pub punct_ink: u32,
}

impl PdfGeometry {
    /// 一叶的纸面尺寸（字格区 + 版框 + 天头地脚）。
    pub fn sheet_w(&self) -> f32 {
        self.cols as f32 * self.cell_w + self.edge
    }
    /// 同上，纵向。
    pub fn sheet_h(&self) -> f32 {
        self.rows as f32 * self.cell_h + self.foot_h() + self.edge + self.margin_top + self.margin_bot
    }
    /// 地脚条高（叶码）。
    pub fn foot_h(&self) -> f32 {
        (self.fs * 0.36).max(9.0) + 10.0
    }
}

/// 一叶要印的内容（**只读**：排版引擎已经排完，这里不再动一个字）。
pub struct LeafSpec<'a> {
    /// 排版引擎产出的那一叶（`gloss::Page`）。
    pub page: &'a Page,
    /// 章题（章首页的篇题列用）。
    pub title: &'a str,
    /// 第几章（0 起，篇题数用）。
    pub chapter: usize,
    /// 第几叶（1 起，地脚叶码用）。
    pub leaf: usize,
    /// 卷首题署第一行：书名 + 卷次。
    pub inscription: &'a str,
    /// 卷首题署第二行：署名。
    pub author: &'a str,
    /// 本叶的注音表：`(章, 段, 段内偏移) → 拼音`。
    ///
    /// 汉字的读音（EPUB `<ruby>` / `X（bō）` / `[拼音:bō]` 导入的
    /// `NoteKind::Ruby`，以及「生僻字注音」自动补的那一层）**不住在 `Page`
    /// 里**——`Page` 只有正文与夹注。故由调用方按章备好一张表，落笔时按键取音。
    /// 空表 = 本叶不注音。
    pub ruby: &'a std::collections::HashMap<(usize, usize, usize), String>,
}

/// **印制 PDF 的能力**。
///
/// 实现者只管「把已经排好的叶画成 PDF 字节」，不许自己分页、不许自己算夹注
/// 怎么断——那都是 `gloss` 的活，抢过来就会变成「屏幕上一套、纸上一套」。
pub trait PdfExporter {
    /// 后端名字（出错时给用户看，也便于排查「到底走的哪条路」）。
    fn name(&self) -> &'static str;

    /// 印一叠叶 → 完整 PDF 文件的字节。
    fn render_leaves(
        &self,
        leaves: &[LeafSpec<'_>],
        geo: &PdfGeometry,
    ) -> anyhow::Result<Vec<u8>>;
}

/// **兜底后端**：明确报错，绝不静默产出一张空白纸。
///
/// 跨平台时如果直接给个「返回空 Vec」的实现，用户会拿到一个能打开、但一片
/// 空白的 PDF，然后花两小时怀疑是不是自己书坏了。宁可响亮地失败。
pub struct UnavailablePdfExporter;

impl PdfExporter for UnavailablePdfExporter {
    fn name(&self) -> &'static str {
        "unavailable"
    }
    fn render_leaves(&self, _leaves: &[LeafSpec<'_>], _geo: &PdfGeometry) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!(
            "本平台尚无 PDF 后端。请在壳子里实现 `jiandu_core::platform::PdfExporter` 并注入；\
             纯 Rust 路线见 `README_cross_platform.md`（printpdf / pdf-writer）"
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 二、输入法（IME）
// ─────────────────────────────────────────────────────────────────────────────

/// **文本输入槽**：core 只认三种动作，平台差异全挡在外面。
///
/// 为什么这么窄：中文输入法是各家 OS 最深的一块方言（macOS 是
/// `NSTextInputClient` 的 `markedText` 机制，Windows 是 IMM32 / TSF，
/// 安卓是 `InputConnection`）。但它们**最后都归结成同一件事**：
/// 有一段「组字中」的文本，和一段「已上屏」的文本。
///
/// core 侧（将来的检索 / 问笺 / 批注逻辑）只要会喊这三句，就能在任何平台上
/// 收中文，而不必知道 `inputContext` 是什么。
pub trait InputSink {
    /// 已确认（上屏）的文本，写进光标处。
    fn insert_text(&mut self, text: &str);
    /// 组字中的文本（拼音 / 假名 / 手写候选）；传空串表示清掉。
    fn set_marked(&mut self, text: &str);
    /// 结束组字（丢弃未上屏的部分）。
    fn clear_marked(&mut self);
}

// ─────────────────────────────────────────────────────────────────────────────
// 三、注音
// ─────────────────────────────────────────────────────────────────────────────

/// **读音来源**：core 自带一套（内置字书 + `pinyin` crate），
/// 但留了口子让壳子挂别的（比如带多音字消歧的模型，或用户自建的读音表）。
pub trait PinyinSource {
    /// 这个字该注什么音；返回 `None` 表示「不用注」。
    fn pinyin_of(&self, c: char) -> Option<String>;
    /// 这个字算不算常用（常用的不注）。
    fn is_common(&self, c: char) -> bool;
}

/// 内置实现：`dict` 模块那套（精选生僻字表 → 常用字不注 → `pinyin` 兜底）。
pub struct BuiltinPinyin;

impl PinyinSource for BuiltinPinyin {
    fn pinyin_of(&self, c: char) -> Option<String> {
        crate::dict::auto_pinyin(c)
    }
    fn is_common(&self, c: char) -> bool {
        crate::dict::is_common_char(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_sheet_matches_grid_plus_frame() {
        let g = PdfGeometry {
            cols: 12,
            cell_w: 30.0,
            cell_h: 30.0,
            rows: 22,
            fs: 24.0,
            reserve: 0,
            edge: 34.0,
            margin_top: 40.0,
            margin_bot: 26.0,
            paper: 0,
            ink: 0,
            rule: 0,
            rule_soft: 0,
            note_ink: 0,
            accent: 0,
            font: PdfFontKind::Song,
            note_font: PdfFontKind::Kai,
            han_digits: false,
            traditional: false,
            punct_mode: crate::gloss::PunctMode::Modern,
            punct_ink: 0xC3272D,
        };
        assert!((g.sheet_w() - (12.0 * 30.0 + 34.0)).abs() < 1e-6);
        assert!(g.sheet_h() > g.sheet_w(), "A4 竖放：高必须大于宽");
        // 天头∶地脚 = 3∶2 是铁律，几何里就得留得出这个比例
        assert!(g.margin_top > g.margin_bot);
    }

    #[test]
    fn unavailable_backend_fails_loudly() {
        let e = UnavailablePdfExporter;
        assert_eq!(e.name(), "unavailable");
        let page = Page {
            glyphs: Vec::new(),
            start_chapter: false,
            columns: Vec::new(),
        };
        let ruby = std::collections::HashMap::new();
        let leaf = LeafSpec {
            page: &page,
            title: "",
            chapter: 0,
            leaf: 1,
            inscription: "",
            author: "",
            ruby: &ruby,
        };
        let g = PdfGeometry {
            cols: 12,
            cell_w: 30.0,
            cell_h: 30.0,
            rows: 22,
            fs: 24.0,
            reserve: 0,
            edge: 34.0,
            margin_top: 40.0,
            margin_bot: 26.0,
            paper: 0,
            ink: 0,
            rule: 0,
            rule_soft: 0,
            note_ink: 0,
            accent: 0,
            font: PdfFontKind::Song,
            note_font: PdfFontKind::Kai,
            han_digits: false,
            traditional: false,
            punct_mode: crate::gloss::PunctMode::Modern,
            punct_ink: 0xC3272D,
        };
        assert!(e.render_leaves(std::slice::from_ref(&leaf), &g).is_err());
    }

    #[test]
    fn builtin_pinyin_annotates_rare_not_common() {
        let p = BuiltinPinyin;
        // 「般」是常用字，不该注；「埵」是生僻字，该注。
        assert!(p.is_common('般'));
        assert!(!p.is_common('埵'));
        assert!(p.pinyin_of('埵').is_some());
    }
}
