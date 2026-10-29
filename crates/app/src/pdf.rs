//! **PDF 印制**（v1.80.0）：把古籍竖排叶面（含**双行夹注**）落成矢量 PDF。
//!
//! 与「本叶 PNG」（`share_paint`）同宗——**离屏** CoreGraphics，不读屏幕、
//! 不要权限、锁屏也能出图。差别只在**画布**：
//!
//! ```text
//!   PNG  → CGBitmapContext（位图，2160px 长边）
//!   PDF  → CGPDFContext  （矢量，字是**真字**、内嵌字体子集）
//! ```
//!
//! 矢量带来的三件实事：任意倍放大不糊、文字可复制 / 可检索、文件只有几十 KB。
//!
//! ── 排版从哪儿来 ────────────────────────────────────────────────
//! **一页 PDF = 一叶**：列 / 片（`Piece::Main` / `Piece::Gloss` / `Piece::Gap`）
//! 全部由**阅读器那一台排版引擎**（`gloss::layout_book_from_opts`）产出，
//! 本模块只做「照着落笔」，绝不自己分页、绝不自己算夹注怎么断——
//! 所以屏幕上的双行夹注怎么转行、怎么跨列，PDF 里就怎么转、怎么跨。
//! 几何（列宽 / 行高 / 行数 / 字号）同样由 `grid::calculate_grid` 按 A4 试算，
//! 与屏幕同一套算式，只是换了纸。
//!
//! ── 三条防雷（与 `share_paint` 同一口径）────────────────────────
//! 1. **不翻 CTM**：`CGPDFContext` 与位图上下文同是 CG 坐标（原点左下、
//!    y 向上），版式数据是屏幕坐标（原点左上）。只把每个元素的 y 做一次
//!    镜像（[`Brush::cy`]），**绝不动 CTM**——一动字形就被垂直镜像。
//! 2. **CFRelease**：`CGContext` / `CFURL` 都是 `TCFType` 包装，`Drop` 自动
//!    release；字体按 (字形类, 字号) 缓存复用，一叶只建两三台字体对象。
//! 3. **整本导出的两条底线（v1.85.3，商业化印刷代排）**：
//!    - **UTF-16 绝不越界**：码元缓冲按 `char::len_utf16()` 精确开
//!      （[`utf16_units`]）。扩展 B 区的字（如 `U+27B82`）要两枚码元，
//!      旧代码写死 `[u16; 1]` 会 **panic**，整本《离骚》一碰生僻字就崩。
//!    - **逐页流式落盘**：[`PdfStream`] 一叶一 `EndPage` + `flush`，
//!      写完即交 CG，本模块**不持有任何一页的产物**——内存与书的厚薄无关。
//!    - **字体三级回退**：本字体 → 楷/黑/宋 → 系统兜底链（宋体 / 苹方 /
//!      LastResort），缺字一路往下换，绝不画空格也绝不崩。

use crate::gloss::Piece;
use jiandu_core::platform::{LeafSpec, PdfExporter, PdfGeometry};
use crate::share_layout::ShareFont;
use crate::types::GlyphKind;
use core_foundation::base::TCFType;
use core_foundation::url::CFURL;
use core_graphics::context::CGContext;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys;
use core_text::font::CTFont;
use foreign_types::ForeignType;
use std::path::Path;

// ── CoreGraphics 里 core-graphics 0.24 没包的三个 PDF 入口 ─────────────
// 符号来自已链接的 CoreGraphics.framework（`core-graphics` 依赖它），
// 只需自己声明签名；`CGContext` 由 `ForeignType::from_ptr` 包一层，
// 之后的绘制全部走 crate 的安全方法，不再碰裸指针。
extern "C" {
    fn CGPDFContextCreateWithURL(
        url: core_foundation::url::CFURLRef,
        media_box: *const CGRect,
        auxiliary_info: core_foundation::base::CFTypeRef,
    ) -> sys::CGContextRef;
    fn CGPDFContextBeginPage(ctx: sys::CGContextRef, page_info: core_foundation::base::CFTypeRef);
    fn CGPDFContextEndPage(ctx: sys::CGContextRef);
    fn CGPDFContextClose(ctx: sys::CGContextRef);
}

/// A4 竖放（PDF 点，1pt = 1/72 英寸）。
pub const A4_W: f64 = 595.276;
pub const A4_H: f64 = 841.89;

/// **版框层的四个数**（v1.85.0）。它们是「双线永不粘连」的全部依据，
/// 故提成模块常量、由 [`frame_plan`] 单测钉死——绘制代码里不再散落魔数。
///
/// * 线宽只有两档：外 1.0pt（粗栏线）、内 0.5pt（细栏线）。0.5 是细线在
///   PDF 栅格化后仍不消失的下限，再细就被抗锯齿吃掉。
/// * 内外线**中心距恒为 3.0pt**，与字号、列数、行数全不相干：改字级只让
///   格子变大，框层照旧——这就是「无论怎么缩放字号，双线间距一分不差」。
///   净距 = 3.0 − 半个粗线 − 半个细线 = 2.25pt（≈0.8mm），肉眼是一道缝，
///   纸面分辨率下绝不会并成一根。
pub const FRAME_OUTER_W: f64 = 1.0;
pub const FRAME_INNER_W: f64 = 0.5;
/// 内外线中心距（pt）。
pub const FRAME_GAP: f64 = 3.0;

/// 半像素对齐：线宽 ≤1pt 时，坐标落在 0.5 的整数倍上才不会糊成两条灰边。
fn snap(v: f64) -> f64 {
    (v * 2.0).round() / 2.0
}

/// 一叶版框的落笔几何（**纯数据**，不碰 `CGContext`，故可单测）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePlan {
    /// 外框矩形：`(x, y, w, h)`，坐标是**线的中心线**。
    pub outer: (f64, f64, f64, f64),
    /// 内框两角：左上 + 右下（同样是中心线）。
    pub inner: (f64, f64, f64, f64),
}

impl FramePlan {
    /// 内外线的**净距**（两根线之间露出的纸）——「粘不粘连」就看它。
    pub fn gap(&self) -> f64 {
        (self.inner.0 - FRAME_INNER_W * 0.5) - (self.outer.0 + FRAME_OUTER_W * 0.5)
    }
}

/// 算一叶的版框几何。`x0/y0` 是字格区左上，`fw/block_h` 是字格区宽高。
///
/// 位置关系（与屏幕 `reader::render_ancient` 同一套层次）：
/// **纸缘 → 外框粗线 → 一道缝 → 内框细线 → 框内留边 → 字格**。
/// 内线永远缩在外线之内的 [`FRAME_GAP`] 处，不跟着字格跑——所以字级
/// 从 14 调到 40、列数从 8 换到 14，这两根线的间距仍是 3pt。
pub fn frame_plan(edge: f64, x0: f64, y0: f64, fw: f64, block_h: f64) -> FramePlan {
    let pad = edge * 0.5;
    // 外框：路径内收半个线宽 → 整根线落在纸面之内（与屏幕 `border_3` 同位）。
    let half = FRAME_OUTER_W * 0.5;
    let ox = snap(x0 - pad) + half;
    let oy = snap(y0 - pad) + half;
    let ow = snap(fw + pad * 2.0) - FRAME_OUTER_W;
    let oh = snap(block_h + pad * 2.0) - FRAME_OUTER_W;
    // 内框：外框之内 FRAME_GAP 处。整份 PDF 只此一个间距，故逐页一致。
    let d = FRAME_GAP;
    FramePlan {
        outer: (ox, oy, ow, oh),
        inner: (
            snap(ox + d),
            snap(oy + d),
            snap(ox + ow - d),
            snap(oy + oh - d),
        ),
    }
}

/// 一叶要印的一页 / 整份 PDF 的版式与配色：
/// **定义住在 `jiandu_core::platform`**（壳子无关），本模块只是拿来用。
///
/// 为什么搬走：跨平台壳子（Windows / 移动端）也要印同样的叶，它们不该
/// 为了拿这两个结构体去依赖一个 GPUI + CoreGraphics 的壳子。
pub use jiandu_core::platform::{LeafSpec as PdfLeaf, PdfFontKind, PdfGeometry as PdfGeo};

/// 壳子的字体语义（`ShareFont`，带 PostScript 回退链）↔ core 的中立语义
/// （`PdfFontKind`）。**两边各说各的方言，只在这里翻译一次**。
impl From<PdfFontKind> for ShareFont {
    fn from(k: PdfFontKind) -> Self {
        match k {
            PdfFontKind::Song => ShareFont::Song,
            PdfFontKind::Kai => ShareFont::Kai,
            PdfFontKind::Hei => ShareFont::Hei,
        }
    }
}

impl From<ShareFont> for PdfFontKind {
    fn from(k: ShareFont) -> Self {
        match k {
            ShareFont::Song => PdfFontKind::Song,
            ShareFont::Kai => PdfFontKind::Kai,
            ShareFont::Hei => PdfFontKind::Hei,
        }
    }
}

// ── 生僻字安全：UTF-16 码元 ────────────────────────────────────────────
/// 一个 `char` 折成 UTF-16 码元，连同**实际枚数**（BMP = 1，扩展区 = 2）。
///
/// ⚠️ v1.85.3 修的正是这里 —— 旧代码：
/// ```ignore
/// let mut unit: [u16; 1] = [0];
/// ch.encode_utf16(&mut unit);   // ← 缓冲不够时 **panic**，不是返回错误
/// ```
/// `U+27B82`（扩展 B 区）要**两枚**码元（代理对），塞进一枚的缓冲直接
/// `need 2 bytes to encode U+27B82 but buffer has just 1` 把进程带走——
/// 整本《离骚》《资治通鉴》一碰生僻字就崩，商业印刷代排绝不能这样。
///
/// 容量**只由 `len_utf16()` 精确得出**（等价于 `encode_utf16().count()`）：
/// 绝不用 `text.len()`（那是 UTF-8 字节数）也绝不用 `chars().count()`
/// （那是码点数）——这两个在「一个汉字 ≠ 一个码元」的地方永远猜错。
fn utf16_units(ch: char) -> ([u16; 2], usize) {
    let n = ch.len_utf16(); // 精确：1（BMP）或 2（代理对）
    let mut buf = [0u16; 2]; // 上限恒为 2，`n` 绝不会超出
    ch.encode_utf16(&mut buf[..n]);
    (buf, n)
}

/// **系统兜底字体链**：自备字体 / 刻本字体缺的异体字、扩展区生僻字靠它们收尾。
///
/// 印刷代排的底线是「纸上一枚字都不能少，也绝不能崩」：字形查不到就一路
/// 往下换字体，直到系统里一定在的宋 / 黑。顺序按「最像刻本」排——先宋后黑，
/// 最后 `LastResort`（macOS 的系统兜底字体，覆盖全 Unicode，样子是带码位的
/// 方框：**有框能被看见，也比空白 / 崩溃强一百倍**）。
const SYS_FALLBACK: &[&str] = &[
    "Songti SC",
    "SongtiSC",
    "STSong",
    "SimSun",
    "Songti TC",
    "PingFang SC",
    "PingFangSC",
    "Hiragino Sans GB",
    "HiraginoSansGB",
    "Heiti SC",
    "STHeiti",
    "Arial Unicode MS",
    "LastResort",
];

/// 字体缓存上限：键是 `(字体, 字号)`，正常一本只会有十来个组合；
/// 设个上限只为防「字号被算成几百种」的意外把缓存撑大。**流式导出时
/// 缓存不清空**（清空会让每页重建字体，反而更慢更费），只在上限处兜底。
const FONT_CACHE_MAX: usize = 64;

// ── 画笔 ──────────────────────────────────────────────────────────────
/// 一支笔：持一个 CG 上下文（**PDF**），把「屏幕坐标」翻成 CG 坐标落笔。
struct Brush {
    ctx: CGContext,
    /// 页高（换算用）。
    h: f64,
    /// 字体缓存：`(字形类, 字号×4 取整) → (字体, ascent, descent)`。
    fonts: Vec<(ShareFont, i32, CTFont, f64, f64)>,
    /// 系统兜底字体缓存：`(PostScript 名, 字号×4) → (字体, ascent, descent)`。
    sys: Vec<(&'static str, i32, CTFont, f64, f64)>,
    /// **缺字计数**：连系统兜底都查不到字形的字（只记数，绝不崩、绝不静默）。
    missing: usize,
}

/// `0xRRGGBB` → 三分量（0~1）。
fn rgb(c: u32) -> (f64, f64, f64) {
    (
        ((c >> 16) & 0xFF) as f64 / 255.0,
        ((c >> 8) & 0xFF) as f64 / 255.0,
        (c & 0xFF) as f64 / 255.0,
    )
}

impl Brush {
    fn new(ctx: CGContext, h: f64) -> Self {
        Self {
            ctx,
            h,
            fonts: Vec::new(),
            sys: Vec::new(),
            missing: 0,
        }
    }

    /// 屏幕 y（左上原点）→ CG y（左下原点）。**只镜像，不缩放**。
    fn cy(&self, y: f64) -> f64 {
        self.h - y
    }

    /// 取（并缓存）一台字体：`(ascent, descent)` 用于行盒居中。
    fn font(&mut self, kind: ShareFont, fs: f64) -> Result<(CTFont, f64, f64), String> {
        let key = (fs * 4.0).round() as i32;
        if let Some(f) = self
            .fonts
            .iter()
            .find(|(k, kk, _, _, _)| *k == kind && *kk == key)
        {
            return Ok((f.2.clone(), f.3, f.4));
        }
        let mut font: Option<CTFont> = None;
        for name in kind.postscript_chain() {
            if let Ok(f) = core_text::font::new_from_name(name, fs) {
                font = Some(f);
                break;
            }
        }
        let font = font.ok_or_else(|| "系统里找不到可用的中文字体".to_string())?;
        let asc = if font.ascent() > 0.0 {
            font.ascent()
        } else {
            fs * 0.88
        };
        let desc = font.descent().abs();
        if self.fonts.len() >= FONT_CACHE_MAX {
            self.fonts.clear();
        }
        self.fonts.push((kind, key, font.clone(), asc, desc));
        Ok((font, asc, desc))
    }

    /// 取（并缓存）一台**系统兜底**字体（按 PostScript / 家族名）。
    /// 系统里没有这个名字就返回 `None`（换下一个），绝不 panic。
    fn sys_font(&mut self, name: &'static str, fs: f64) -> Option<(CTFont, f64, f64)> {
        let key = (fs * 4.0).round() as i32;
        for (n, k, f, a, d) in self.sys.iter() {
            if *n == name && *k == key {
                return Some((f.clone(), *a, *d));
            }
        }
        let font = core_text::font::new_from_name(name, fs).ok()?;
        let asc = if font.ascent() > 0.0 {
            font.ascent()
        } else {
            fs * 0.88
        };
        let desc = font.descent().abs();
        if self.sys.len() >= FONT_CACHE_MAX {
            self.sys.clear();
        }
        self.sys.push((name, key, font.clone(), asc, desc));
        Some((font, asc, desc))
    }

    /// 存 / 取图形状态（配合 [`Brush::clip_top_rect`] 用）。
    fn save(&self) {
        self.ctx.save();
    }
    fn restore(&self) {
        self.ctx.restore();
    }

    /// 按**屏幕坐标**（左上 + 宽高）裁切后续绘制。
    fn clip_top_rect(&self, x: f64, y: f64, w: f64, h: f64) {
        self.ctx.clip_to_rect(CGRect::new(
            &CGPoint::new(x, self.cy(y + h)),
            &CGSize::new(w.max(0.0), h.max(0.0)),
        ));
    }

    /// 一块实心 / 空心矩形（屏幕坐标：左上 + 宽高）。
    fn rect(&self, x: f64, y: f64, w: f64, h: f64, color: u32, stroke: Option<f64>) {
        let (r, g, b) = rgb(color);
        let rect = CGRect::new(&CGPoint::new(x, self.cy(y + h)), &CGSize::new(w, h));
        match stroke {
            Some(lw) => {
                self.ctx.set_rgb_stroke_color(r, g, b, 1.0);
                self.ctx.set_line_width(lw);
                self.ctx.stroke_rect(rect);
            }
            None => {
                self.ctx.set_rgb_fill_color(r, g, b, 1.0);
                self.ctx.fill_rect(rect);
            }
        }
    }

    /// 一条线段（屏幕坐标）。
    fn line(&self, x1: f64, y1: f64, x2: f64, y2: f64, color: u32, w: f64) {
        let (r, g, b) = rgb(color);
        self.ctx.set_rgb_stroke_color(r, g, b, 1.0);
        self.ctx.set_line_width(w);
        self.ctx.begin_path();
        self.ctx.move_to_point(x1, self.cy(y1));
        self.ctx.add_line_to_point(x2, self.cy(y2));
        self.ctx.stroke_path();
    }

    /// 实心多边形（屏幕坐标），鱼尾用。
    fn poly(&self, pts: &[(f64, f64)], color: u32) {
        if pts.len() < 3 {
            return;
        }
        let (r, g, b) = rgb(color);
        self.ctx.set_rgb_fill_color(r, g, b, 1.0);
        self.ctx.begin_path();
        self.ctx.move_to_point(pts[0].0, self.cy(pts[0].1));
        for p in &pts[1..] {
            self.ctx.add_line_to_point(p.0, self.cy(p.1));
        }
        self.ctx.close_path();
        self.ctx.fill_path();
    }

    /// 椭圆（屏幕坐标：**中心** + 宽高），实心 / 空心。句读用。
    fn ellipse(&self, cx: f64, cy: f64, w: f64, h: f64, color: u32, stroke: Option<f64>) {
        let (r, g, b) = rgb(color);
        let rect = CGRect::new(
            &CGPoint::new(cx - w * 0.5, self.cy(cy + h * 0.5)),
            &CGSize::new(w, h),
        );
        match stroke {
            Some(lw) => {
                self.ctx.set_rgb_stroke_color(r, g, b, 1.0);
                self.ctx.set_line_width(lw);
                self.ctx.stroke_ellipse_in_rect(rect);
            }
            None => {
                self.ctx.set_rgb_fill_color(r, g, b, 1.0);
                self.ctx.fill_ellipse_in_rect(rect);
            }
        }
    }

    /// 落一枚字：**字形在给定盒子里居中**（CSS half-leading，与 GPUI 的
    /// `.line_height(px(cell_h))` + 居中同一口径）。
    ///
    /// `x/y/w/h` 是屏幕坐标的盒子；`fs` 是字号；`color` 是墨色。
    fn glyph_in(&mut self, ch: char, x: f64, y: f64, w: f64, h: f64, fs: f64, color: u32, kind: ShareFont) {
        self.glyph_at(ch, x + w * 0.5, y, h, fs, color, kind);
    }

    /// 在一台字体里查 `unit` 的字形；**查不到（或缺字）返回 `None`**。
    ///
    /// 判定「缺字」用 `!ok || glyph == 0`：字形序号 0 是 `.notdef`，
    /// 画出来是个空格 / 方框，**等于白印一个字**——必须当缺字处理，
    /// 交给下一级字体（这是回退机制真正生效的关键一判）。
    fn lookup(
        &mut self,
        k: ShareFont,
        fs: f64,
        unit: &[u16],
    ) -> Option<(CTFont, f64, f64, core_graphics::font::CGGlyph)> {
        let (font, asc, desc) = self.font(k, fs).ok()?;
        let mut glyphs: [core_graphics::font::CGGlyph; 2] = [0; 2];
        let ok = unsafe {
            font.get_glyphs_for_characters(unit.as_ptr(), glyphs.as_mut_ptr(), unit.len() as isize)
        };
        if !ok || glyphs[0] == 0 {
            return None;
        }
        Some((font, asc, desc, glyphs[0]))
    }

    /// 把一枚字形落到纸上（拿到的 `asc/desc` 决定行盒居中）。
    fn put(
        &self,
        font: &CTFont,
        glyph: core_graphics::font::CGGlyph,
        cx: f64,
        y: f64,
        h: f64,
        fs: f64,
        asc: f64,
        desc: f64,
        color: u32,
    ) {
        let baseline = y + (h - (asc + desc)) * 0.5 + asc;
        let (r, g, b) = rgb(color);
        self.ctx.set_rgb_fill_color(r, g, b, 1.0);
        font.draw_glyphs(
            &[glyph],
            &[CGPoint::new(cx - fs * 0.5, self.cy(baseline))],
            self.ctx.clone(),
        );
    }

    /// 落一枚字：横向按 `cx` 居中、纵向按行盒居中。返回**有没有真落上**。
    ///
    /// ── 三级字体回退（v1.85.3，商业化印刷代排底线）─────────────────
    ///   ① 本字体（刻本 / 楷 / 黑，含用户自备字库）
    ///   ② 楷 → 黑 → 宋（老规矩：宋体缺 `ō ǎ ě` 这类带调元音时靠这层补）
    ///   ③ **系统兜底链**（Songti SC / PingFang SC / … / LastResort）
    ///
    /// 三级都查不到才放弃这一枚（只 `missing += 1`），**绝不 panic、绝不画
    /// 空格**。扩展 B 区的字（如 `U+27B82`）在自备刻本字体里多半没有，
    /// 但系统宋体 / LastResort 里一定有——所以生僻字照样安安全全落在纸上。
    fn glyph_at(
        &mut self,
        ch: char,
        cx: f64,
        y: f64,
        h: f64,
        fs: f64,
        color: u32,
        kind: ShareFont,
    ) -> bool {
        // ① UTF-16 码元：**按 `len_utf16()` 精确开缓冲**，扩展区两枚也不越界。
        let (buf, n) = utf16_units(ch);
        let unit = &buf[..n];

        // ② 本字体 → 楷 / 黑 / 宋
        let chain = [kind, ShareFont::Kai, ShareFont::Hei, ShareFont::Song];
        for k in chain {
            if let Some((font, asc, desc, glyph)) = self.lookup(k, fs, unit) {
                self.put(&font, glyph, cx, y, h, fs, asc, desc, color);
                return true;
            }
        }
        // ③ 系统兜底：自备字体缺的异体字 / 扩展区字靠这一层收尾。
        for name in SYS_FALLBACK {
            let Some((font, asc, desc)) = self.sys_font(name, fs) else {
                continue;
            };
            let mut glyphs: [core_graphics::font::CGGlyph; 2] = [0; 2];
            let ok = unsafe {
                font.get_glyphs_for_characters(unit.as_ptr(), glyphs.as_mut_ptr(), unit.len() as isize)
            };
            if !ok || glyphs[0] == 0 {
                continue;
            }
            self.put(&font, glyphs[0], cx, y, h, fs, asc, desc, color);
            return true;
        }
        // 全系统都没有这枚字：记一笔，**绝不崩、绝不画空格**。
        self.missing += 1;
        false
    }
}

/// 注音：**横排、正立、从左到右**，落在汉字右侧的列间空白里
/// （与屏幕 `ruby::layout` 同一套几何）。
///
/// 屏幕那侧是一张 CoreGraphics 离屏位图；这里是 CG 逐字落笔。两者共用
/// `jiandu_core::ruby::layout` 的度量（字号 40%、按可用宽收敛到 22%、
/// 左缘从汉字右缘让开），所以 PDF 里的 `měng` 与屏幕逐字同位——
/// 绝不歪着排、绝不被转 90°，也绝不碰到下一列的字形。
fn draw_ruby(
    b: &mut Brush,
    py: &str,
    x: f64,
    y: f64,
    cell_w: f64,
    cell_h: f64,
    fs: f64,
    g: &PdfGeo,
) {
    let rl = crate::ruby::layout(py, fs as f32, cell_w as f32, cell_h as f32);
    let left = x + rl.left as f64;
    let top = y + rl.top as f64;
    // 等距分格落笔：格宽由 `ruby::layout` 定死，字距均匀，绝不会把串画长。
    let n = py.chars().count().max(1) as f64;
    let step = rl.block_w as f64 / n;
    for (i, c) in py.chars().enumerate() {
        b.glyph_at(
            c,
            left + (i as f64 + 0.5) * step,
            top,
            rl.block_h as f64,
            rl.fs as f64,
            g.note_ink,
            ShareFont::from(g.note_font),
        );
    }
    // 三点提示（与屏幕同款）：贴字格下缘右侧，三道极细短横——只作
    // 「此字有音」的暗示，不抢正文。宽高都随格缩，A4 上是一道灰线。
    let dw = (cell_w * 0.34) / (3.0 * 1.9);
    let gap = dw * 0.9;
    let total = dw * 3.0 + gap * 2.0;
    let left = x + cell_w - cell_w * 0.10 - total;
    let dy = y + cell_h - cell_h * 0.055 - 0.7;
    for i in 0..3 {
        b.rect(left + i as f64 * (dw + gap), dy, dw, 0.7, g.note_ink, None);
    }
}

// ── 画一叶 ────────────────────────────────────────────────────────────
/// 单叶在纸面上的落位（由 [`PdfGeo`] + A4 算出，**所有叶共用**）。
#[derive(Clone, Copy)]
struct Place {
    /// 字格区左上（PDF 页坐标）。
    gx: f64,
    gy: f64,
}

fn place_of(g: &PdfGeo) -> Place {
    let sw = g.sheet_w() as f64;
    let sh = g.sheet_h() as f64;
    // 纸面在 A4 上居中；横向留白由版心率决定，纵向天头地脚已是 3∶2。
    let ox = (A4_W - sw) * 0.5;
    let oy = (A4_H - sh) * 0.5;
    Place {
        gx: ox + (sw - g.cols as f64 * g.cell_w as f64) * 0.5,
        gy: oy + g.edge as f64 * 0.5 + g.margin_top as f64,
    }
}

/// 画一整叶：`纸 → 版框 → 界行 → 正文 / 夹注 → 章题列 → 叶码 → 朱印`。
fn draw_leaf(b: &mut Brush, leaf: &PdfLeaf, g: &PdfGeo, p: &Place) {
    let cols = g.cols;
    let cell_w = g.cell_w as f64;
    let cell_h = g.cell_h as f64;
    let fs = g.fs as f64;
    let foot = g.foot_h() as f64;
    let start_chapter = leaf.page.start_chapter;
    // 章首页：最右一列让给篇题，正文列少一列（与分页同规则）。
    let text_cols = if start_chapter {
        cols.saturating_sub(1).max(1)
    } else {
        cols
    };
    let block_h = g.rows as f64 * cell_h + foot;
    let pad = g.edge as f64 * 0.5;
    let x0 = p.gx;
    let y0 = p.gy;

    // ── 版框（v1.85.0）：外框一粗四边满画，内框一细按形制裁边 ──────────
    //
    // 屏幕上的层次（见 `reader::render_ancient`）：外框 3px → 框内留边
    // 3px → 内框 1px → 字格。纸上照同一张表落笔（`frame_plan`）：外线贴
    // 着纸缘（线在纸内，与屏幕 border 一样），内线贴着字格（屏幕的内框
    // 就是字格的那一圈边）。间距由 `frame_plan` 保证，见那里的单测。
    let fw = cols as f64 * cell_w;
    let plan = frame_plan(g.edge as f64, x0, y0, fw, block_h);
    let (ox, oy, ow, oh) = plan.outer;
    b.rect(ox, oy, ow, oh, g.rule, Some(FRAME_OUTER_W));

    // 内框：只在形制给的边上画（四周双边满画 / 左右双边只两道竖线 /
    // 四周单边一道也不画）。`sides` 与屏幕上 `FrameStyle::inner_sides`
    // 是同一个函数的输出，故屏幕与纸上不可能一密一疏。
    let (ix0, iy0, ix1, iy1) = plan.inner;
    let sides = g.inner_sides();
    if sides[0] {
        b.line(ix0, iy0, ix1, iy0, g.rule_soft, FRAME_INNER_W);
    }
    if sides[1] {
        b.line(ix1, iy0, ix1, iy1, g.rule_soft, FRAME_INNER_W);
    }
    if sides[2] {
        b.line(ix0, iy1, ix1, iy1, g.rule_soft, FRAME_INNER_W);
    }
    if sides[3] {
        b.line(ix0, iy0, ix0, iy1, g.rule_soft, FRAME_INNER_W);
    }


    // ── 界行（乌丝栏）：每列左缘一道通高细线，空列照样画 ──────────────
    // 1px 线骑在路径两侧各半个像素上，落在整数坐标会糊成两条灰边——
    // 故 x 取「整像素 + 0.5」（与 share_paint 同一处理）。
    //
    // v1.83.0：界行走 `rule_soft`（外框用 `rule`）——仿刻本里外框是浓墨、
    // 界行退成一道灰，与内框同属「第二层」。其余主题两值相同。
    for i in 1..cols {
        let x = (x0 + i as f64 * cell_w).round() + 0.5;
        b.line(x, y0, x, y0 + block_h, g.rule_soft, 0.7);
    }

    // ── 栏内裁切（兜底）────────────────────────────────────────────────
    // 分页已保证每列不越格；这里再兜一道，万一某列算多了（或注块取整后多出
    // 半格），溢出的字被**版框裁掉**，而不是漫到纸边上——印出来才不像「漏排」。
    // 版框、叶码、朱印都在裁切区之外（叶码在 block_h 内、印在框外），
    // 故 restore 之后再画。
    b.save();
    b.clip_top_rect(x0 - 1.0, y0 - 1.0, fw + 2.0, block_h + 2.0);

    // ── 正文列：读序号 i=0 在最右，故横向位置 = (text_cols-1-i) ───────
    for i in 0..text_cols {
        let items: &[Piece] = leaf.page.columns.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
        let cx = x0 + (text_cols - 1 - i) as f64 * cell_w;
        // 章首页开卷列（紧邻篇题列）：顶部让出题署带，正文从带下起排。
        let skip = if start_chapter && i == 0 { g.reserve } else { 0 };
        let mut y = y0 + crate::grid::top_baseline(skip as f32, g.cell_h) as f64;
        // 卷首题署：章首页开卷列顶端的**书名·卷次 + 署名**（与阅读器
        // `render_inscription` 同序：顶格书名、空一格、低一格署名）。
        // 只许占住预留带——长了就裁在带内，绝不压到带下的正文上。
        if skip > 0 && !leaf.inscription.is_empty() {
            let res = skip as f64;
            let mut used = 0usize;
            for ch in leaf.inscription.chars() {
                if used as f64 >= res - 1.0 {
                    break;
                }
                b.glyph_in(ch, cx, y0 + used as f64 * cell_h, cell_w, cell_h, fs, g.ink, ShareFont::from(g.font));
                used += 1;
            }
            if (used as f64) < res - 1.0 {
                used += 1; // 书名与署名之间空一格（对应预留带的 +1）。
            }
            for ch in leaf.author.chars() {
                if used as f64 >= res {
                    break;
                }
                b.glyph_in(
                    ch,
                    cx,
                    y0 + used as f64 * cell_h,
                    cell_w,
                    cell_h,
                    fs * 0.86,
                    g.ink,
                    ShareFont::from(g.font),
                );
                used += 1;
            }
        }
        walk_column(b, items, cx, &mut y, g, &leaf.ruby);
    }

    // ── 章题列（最右一列）：象鼻 + 上鱼尾 + 篇题 + 篇题数 + 下鱼尾 ────
    if start_chapter {
        draw_title_column(b, leaf, g, x0 + (cols - 1) as f64 * cell_w, y0, block_h, foot);
    }
    b.restore();

    // ── 地脚叶码：地脚条正中，汉字叶数 ──────────────────────────────
    let num = crate::reader::han_num(leaf.leaf);
    let nfs = (fs * 0.36).max(9.0);
    let chars: Vec<char> = num.chars().collect();
    let mut ty = y0 + g.rows as f64 * cell_h + (foot - nfs) * 0.5;
    for ch in chars {
        b.glyph_at(ch, x0 + fw * 0.5, ty, nfs * 1.25, nfs, g.ink, ShareFont::from(g.font));
        ty += nfs * 1.15;
    }

    // ── 朱砂「笺读」印：纸面左下角外侧（与阅读器同一处）─────────────
    let s = 16.0;
    let sx = p.gx - pad - s - 4.0;
    let sy = y0 + block_h + pad - s;
    if sx > 4.0 {
        b.rect(sx, sy, s, s, g.accent, None);
        let t: Vec<char> = "笺讀".chars().collect();
        for (i, ch) in t.iter().enumerate() {
            b.glyph_in(
                *ch,
                sx + s * 0.08 + i as f64 * (s * 0.42),
                sy + s * 0.29,
                s * 0.42,
                s * 0.42,
                s * 0.42,
                0x101010,
                ShareFont::Song,
            );
        }
    }
}

/// 篇题列：与阅读器 `ancient_title_column` 同形制（象鼻 1px + 上下鱼尾 +
/// 竖排篇题 + 篇题数），只是把 GPUI canvas 换成 CG 落笔。
fn draw_title_column(
    b: &mut Brush,
    leaf: &PdfLeaf,
    g: &PdfGeo,
    x: f64,
    y: f64,
    block_h: f64,
    foot: f64,
) {
    let cell_w = g.cell_w as f64;
    let cell_h = g.cell_h as f64;
    let fs = g.fs as f64;
    let (_, th) = crate::banxin::banxin_tail_size(cell_w as f32, g.cell_h, g.cell_w);
    let th = th as f64;
    let head = crate::banxin::xiangbi_len(th as f32) as f64;
    let tw = ((cell_w - 2.0).max(crate::banxin::FISHTAIL_W_MIN as f64)) as f64;

    // 象鼻：版框内缘 → 上鱼尾的一根 1px 单线（铁律：绝不画成墨条）。
    b.line(x + cell_w * 0.5, y, x + cell_w * 0.5, y + head, g.ink, 1.0);
    // 上鱼尾：五点多边形（V 口朝下咬）。
    fish_tail(b, x + cell_w * 0.5, y + head, tw, th, true, g.ink);

    // 篇题：字号略大于正文（1.12），格高同倍。
    let tfs = fs * crate::reader::TITLE_COL_SCALE as f64;
    let tcell = cell_h * crate::reader::TITLE_COL_SCALE as f64;
    let mut ty = y + head + th + cell_h * 0.18;
    // 篇题数（本篇第几章）占的高度先让出来，篇题绝不越界压到鱼尾上。
    let nfs = (fs * 0.42).max(9.0);
    let num = crate::reader::han_num(leaf.chapter + 1);
    let num_h = (nfs + 2.0) * num.chars().count().max(1) as f64;
    let room = y + block_h - foot - th - head * 0.5 - num_h - (ty - y);
    let title: Vec<char> = leaf.title.chars().collect();
    let max_chars = if tcell > 0.0 {
        ((room / tcell).floor() as usize).max(1)
    } else {
        title.len()
    };
    for ch in title.iter().take(max_chars) {
        b.glyph_in(*ch, x, ty, cell_w, tcell, tfs.min(tcell), g.ink, ShareFont::from(g.font));
        ty += tcell;
    }
    // 篇题数：篇题下方，小字。
    ty += cell_h * 0.12;
    for ch in num.chars() {
        b.glyph_in(ch, x, ty, cell_w, nfs * 1.2, nfs, g.ink, ShareFont::from(g.font));
        ty += nfs * 1.15;
    }
    // 下鱼尾：贴正文块下沿（V 口朝上）。
    fish_tail(
        b,
        x + cell_w * 0.5,
        y + block_h - foot - th,
        tw,
        th,
        false,
        g.ink,
    );
}

/// 一枚黑鱼尾：`banxin::fishtail_points` 给的是「x 百分位 + y 绝对值」，
/// 这里按传入的 `w × h` 铺开（**宽恒等于列宽**，两翼压住界栏）。
fn fish_tail(b: &Brush, cx: f64, y: f64, w: f64, h: f64, top: bool, color: u32) {
    let pts = crate::banxin::fishtail_points(top, h as f32);
    let v: Vec<(f64, f64)> = pts
        .iter()
        .map(|(fx, fy)| (cx - w * 0.5 + w * (*fx as f64 / 100.0), y + *fy as f64))
        .collect();
    b.poly(&v, color);
}

/// 一列从上往下走：**每片的推进量与分页的半格账逐项对齐**（错一格就会
/// 整列串行，这是本模块最容易出事的地方，改前先读 `gloss::Filler`）。
fn walk_column(
    b: &mut Brush,
    items: &[Piece],
    x: f64,
    y: &mut f64,
    g: &PdfGeo,
    ruby: &std::collections::HashMap<(usize, usize, usize), String>,
) {
    let cell_w = g.cell_w as f64;
    let cell_h = g.cell_h as f64;
    let fs = g.fs as f64;
    let west_mode = !g.han_digits;
    let mut idx = 0usize;
    while idx < items.len() {
        match &items[idx] {
            // 数字串 / 西文串：整串成组、每字半格（与分页的半格账同源）。
            Piece::Main(gd) if west_mode && crate::west::is_digit(gd.ch) => {
                let j = run_end(items, idx, true);
                draw_run(b, items, idx, j, x, y, g, true);
                idx = j;
            }
            Piece::Main(gd) if west_mode && crate::west::is_west(gd.ch) => {
                let j = run_end(items, idx, false);
                draw_run(b, items, idx, j, x, y, g, false);
                idx = j;
            }
            // 书名号（`Attach`）：半格账记 0，由宿主字格顺手画掉。
            Piece::Main(gd) if gd.kind == GlyphKind::Attach => {
                let has_host = if crate::types::is_open_title(gd.ch) {
                    items[idx + 1..]
                        .iter()
                        .any(|p| matches!(p, Piece::Main(m) if m.kind != GlyphKind::Attach))
                } else {
                    idx > 0
                        && matches!(items[idx - 1], Piece::Main(m) if m.kind != GlyphKind::Attach)
                };
                if !has_host {
                    // 落单（列尾 / 段尾）：单独占一格，仍是 0.7 倍小字。
                    draw_cell(b, gd.ch, x, *y, cell_w, cell_h, fs, g);
                    *y += cell_h;
                }
                idx += 1;
            }
            Piece::Main(gd) => {
                // 挤压标点（`Hang`）与本字同格；闭书名号也由本字带走。
                let hang = match items.get(idx + 1) {
                    Some(Piece::Main(h)) if h.kind == GlyphKind::Hang => Some(*h),
                    _ => None,
                };
                let close_mk = match items.get(idx + 1) {
                    Some(Piece::Main(m))
                        if m.kind == GlyphKind::Attach && crate::types::is_close_title(m.ch) =>
                    {
                        Some(m.ch)
                    }
                    _ => None,
                };
                let open_mk = match items.get(idx.wrapping_sub(1)) {
                    Some(Piece::Main(m))
                        if m.kind == GlyphKind::Attach && crate::types::is_open_title(m.ch) =>
                    {
                        Some(m.ch)
                    }
                    _ => None,
                };
                idx += 1 + hang.map(|_| 1).unwrap_or(0) + close_mk.map(|_| 1).unwrap_or(0);
                draw_cell(b, gd.ch, x, *y, cell_w, cell_h, fs, g);
                // 注音：贴着这一格右上角竖排，**不占额外界位**（与屏幕同）。
                if let Some(py) = ruby.get(&(gd.chapter, gd.para, gd.offset)) {
                    draw_ruby(b, py, x, *y, cell_w, cell_h, fs, g);
                }
                let cy = *y;
                // 书名号：贴本格右上角，0.7 倍正文（开号领起书名、闭号收束）。
                let mfs = (fs * crate::punct::TITLE_FS_RATIO as f64).max(9.0);
                if let Some(c) = open_mk {
                    b.glyph_in(
                        crate::types::title_mark(c),
                        x + cell_w * 0.42,
                        cy,
                        cell_w * 0.58,
                        cell_h * 0.5,
                        mfs,
                        g.ink,
                        ShareFont::from(g.font),
                    );
                }
                if let Some(c) = close_mk {
                    b.glyph_in(
                        crate::types::title_mark(c),
                        x + cell_w * 0.42,
                        cy + cell_h * 0.5,
                        cell_w * 0.58,
                        cell_h * 0.5,
                        mfs,
                        g.ink,
                        ShareFont::from(g.font),
                    );
                }
                if let Some(h) = hang {
                    draw_punct(b, h.ch, x, cy, cell_w, cell_h, fs, g, true);
                }
                *y += cell_h;
            }
            // 双行小字夹注：两个半宽小列，各占 50%（v1.25.0 用户指定）。
            Piece::Gloss(txt, _src, _host) => {
                draw_gloss(b, txt, x, *y, g, idx == 0);
                let chars = txt.chars().count();
                let nrows = chars.div_ceil(2);
                *y += crate::grid::ceil_to_cells(nrows as f32 * g.cell_h * 0.5, g.cell_h) as f64;
                idx += 1;
            }
            // 避头尾禁则让出的空白格：占一个整字格，不落任何字。
            Piece::Gap => {
                *y += cell_h;
                idx += 1;
            }
        }
    }
}

/// 一串连续数字 / 西文的结束下标（不含）。
fn run_end(items: &[Piece], from: usize, digit: bool) -> usize {
    let mut j = from;
    while j < items.len() {
        match &items[j] {
            Piece::Main(g) => {
                let ok = if digit {
                    crate::west::in_digit_run(g.ch, true)
                } else {
                    crate::west::is_west(g.ch)
                };
                if !ok {
                    break;
                }
                j += 1;
            }
            _ => break,
        }
    }
    j.max(from + 1)
}

/// 数字 / 西文串：每字**半格**，字号取 `west::digit_font_size`。
fn draw_run(b: &mut Brush, items: &[Piece], from: usize, to: usize, x: f64, y: &mut f64, g: &PdfGeo, digit: bool) {
    let cell_w = g.cell_w as f64;
    let half = g.cell_h as f64 * 0.5;
    let dfs = crate::west::digit_font_size(g.cell_w, g.cell_h, g.fs) as f64;
    for k in from..to {
        if let Piece::Main(gd) = &items[k] {
            let ch = if digit && g.han_digits {
                // 数字转汉字开启时不分组，这里不会走到；兜底仍是原字。
                gd.ch
            } else {
                gd.ch
            };
            let ch = crate::west::vertical_glyph(ch);
            b.glyph_in(ch, x, *y + (k - from) as f64 * half, cell_w, half, dfs.min(half), g.ink, ShareFont::from(g.font));
        }
    }
    *y += (to - from) as f64 * half;
}

/// 一枚正文单字：句读走**手绘**（与 `punct::place` 同一落位），
/// 括号 / 引号 / 书名号走竖排专用字形，其余按繁简开关落字。
fn draw_cell(b: &mut Brush, ch: char, x: f64, y: f64, cell_w: f64, cell_h: f64, fs: f64, g: &PdfGeo) {
    let c = crate::types::bracket_to_quote(ch);
    if crate::types::is_punct(c) {
        // 句读：格心整个让给正文，只在角上落一枚朱砂记号。
        draw_punct(b, c, x, y, cell_w, cell_h, fs, g, false);
        return;
    }
    if let Some(vp) = crate::west::vertical_paren(c) {
        let pf = (fs * 0.88).max(8.0).min(cell_h);
        b.glyph_in(vp, x, y, cell_w, cell_h, pf, g.ink, ShareFont::from(g.font));
        return;
    }
    if crate::types::is_title_mark(c) {
        let tf = (fs * crate::punct::TITLE_FS_RATIO as f64).max(9.0).min(cell_h);
        b.glyph_in(crate::types::title_mark(c), x, y, cell_w, cell_h, tf, g.ink, ShareFont::from(g.font));
        return;
    }
    if crate::types::is_quote_mark(c) {
        b.glyph_in(crate::types::vertical_quote(c), x, y, cell_w, cell_h, fs.min(cell_h), g.ink, ShareFont::from(g.font));
        return;
    }
    if crate::west::is_west_hyphen(c) {
        b.glyph_in('丨', x, y, cell_w, cell_h, (fs * 0.92).min(cell_h), g.ink, ShareFont::from(g.font));
        return;
    }
    let shown = if g.han_digits && c.is_ascii_digit() {
        crate::west::digits_to_han(&c.to_string(), false).chars().next().unwrap_or(c)
    } else {
        c
    };
    let shown = if g.traditional {
        crate::trad::traditional_of(shown).unwrap_or(shown)
    } else {
        crate::trad::simplified_of(shown).unwrap_or(shown)
    };
    b.glyph_in(shown, x, y, cell_w, cell_h, fs.min(cell_h), g.ink, ShareFont::from(g.font));
}

/// 一枚**手绘句读**：落位与尺寸全读 `punct::place`（阅读器同一函数），
/// 只是把 `paint_path` 换成 CG 的椭圆——空心圈描边、瓜子点填实。
fn draw_punct(
    b: &mut Brush,
    ch: char,
    x: f64,
    y: f64,
    cell_w: f64,
    cell_h: f64,
    fs: f64,
    g: &PdfGeo,
    hung: bool,
) {
    // 形制读**当前句读模式**（`PdfGeo::punct_mode`）——筛选口径与屏幕完全
    // 同源（`gloss::jodu_of`），纸上那枚「〇」才与屏幕上逐字同位。
    let kind = crate::punct::kind_for(ch, g.punct_mode)
        .unwrap_or(crate::punct::PunctKind::Dot);
    let m = if hung {
        crate::punct::place_hang(fs as f32, cell_w as f32, cell_h as f32, kind)
    } else {
        crate::punct::place(fs as f32, cell_w as f32, cell_h as f32, kind)
    };
    let cx = x + m.cx as f64;
    let cy = y + m.cy as f64;
    let d = m.d as f64;
    let ink = g.punct_ink;
    match m.kind {
        // 现代句：空心小圈（细线描边）。
        crate::punct::PunctKind::Dot => b.ellipse(cx, cy, d, d, ink, Some(m.stroke as f64)),
        // 古籍「〇」：略呈椭圆、笔画厚一档（毛笔写的圈不是正圆）。
        crate::punct::PunctKind::Ring => b.ellipse(
            cx,
            cy,
            d * (1.0 + crate::punct::RING_OVAL as f64),
            d,
            ink,
            Some(m.stroke as f64),
        ),
        // 读 / 疑叹：**与屏幕同一枚墨滴**（v1.84.0）。
        //
        // 旧版在这里偷懒画了个「宽 = SEED_W_OF_H × 高的竖长椭圆」——倾角
        // 完全丢了，纸上是一枚躺着的橄榄，与屏幕上那枚拖尾的墨滴根本不是
        // 同一个东西。现在读 `punct::seed_outline`（屏幕也是这两个函数出的
        // 几何），折线填实：纸上的走向、钝口、拖尾与屏幕逐字同位。
        crate::punct::PunctKind::Seed | crate::punct::PunctKind::BigSeed => {
            let pts: Vec<(f64, f64)> = crate::punct::seed_outline(m.d, m.hook)
                .into_iter()
                .map(|(px_, py_)| (cx + px_ as f64, cy + py_ as f64))
                .collect();
            b.poly(&pts, ink);
        }
    }
}

/// **双行夹注**：右列读在前、左列续后，两个半宽小列各占 50%。
///
/// 几何全部取自阅读器 `ancient_gloss` 的同一批常量
/// （`NOTE_PAD_RATIO` / `NOTE_EDGE_INSET` / `NOTE_PUNCT_RATIO` /
/// `grid::GLOSS_HEAD_INSET`），故注文在 PDF 里的疏密与屏幕上逐字一致。
fn draw_gloss(b: &mut Brush, txt: &str, x: f64, y: f64, g: &PdfGeo, head: bool) {
    let cell_w = g.cell_w as f64;
    let row_h = g.cell_h as f64 * 0.5;
    let chars: Vec<char> = txt.chars().collect();
    let nrows = chars.len().div_ceil(2).max(1);
    let floor = if g.cols >= 10 { 10.0 } else { 7.0 };
    let nfs = ((g.fs as f64 * 0.5).max(floor)).min(row_h.max(7.0));
    let pad = cell_w * crate::gloss::NOTE_PAD_RATIO as f64;
    let inset = cell_w * crate::gloss::NOTE_EDGE_INSET as f64;
    let sub_w = (cell_w - pad - inset * 2.0) * 0.5;
    let block_h = crate::grid::ceil_to_cells(nrows as f32 * g.cell_h * 0.5, g.cell_h) as f64;
    // 列首夹注允许略降（缩进量不许超过取整让出的空当，否则注文被裁）。
    let slack = (block_h - nrows as f64 * row_h).max(0.0);
    let head_inset = if head {
        (g.cell_h as f64 * crate::grid::GLOSS_HEAD_INSET as f64).min(slack)
    } else {
        0.0
    };
    // 注文里的句读再收一档：全角句号与注文同号时一个点顶半颗字，
    // 整块夹注会糊成黑团（v1.28.0）。
    let pfs = nfs * crate::gloss::NOTE_PUNCT_RATIO as f64;
    for r in 0..nrows {
        // 右列读在前、左列续后 → 先落左列（第 r+rows 个）、再落右列（第 r 个）。
        let pair = [chars.get(r + nrows).copied(), chars.get(r).copied()];
        for (k, c) in pair.iter().enumerate() {
            let Some(c) = c else { continue };
            let ch = crate::types::bracket_to_quote(*c);
            let size = if crate::gloss::is_dot_punct(ch) || crate::types::is_punct(ch) {
                pfs
            } else {
                nfs
            };
            b.glyph_in(
                ch,
                x + pad + inset + k as f64 * sub_w,
                y + head_inset + r as f64 * row_h,
                sub_w,
                row_h,
                size.min(row_h),
                g.note_ink,
                ShareFont::from(g.note_font),
            );
        }
    }
}

// ── 流式写手 ──────────────────────────────────────────────────────────
/// **一页一页写的 PDF 写手**（v1.85.3，整本导出的内存底线）。
///
/// 五千页的《资治通鉴》绝不能先把五千页的绘制结果攒在内存里。这里是：
///
/// ```text
///   begin(path) ──► push(叶1) ──► push(叶2) ──► … ──► finish()
///                     │
///                     └─ BeginPage → 铺纸 → 落笔 → EndPage
///                        → flush（催 CG 把这一页写下去）
///                        → 本页临时全丢（不持有任何一页的产物）
/// ```
///
/// `CGPDFContext` 本身是**数据消费者**：`EndPage` 一过，这一页的内容流就
/// 交给它往下写；本模块只留一支笔（字体缓存是十来个对象，与页数无关）。
/// 因此内存占用**与书有多厚无关**，只与「一页有多大」有关。
///
/// `Drop` 会兜底 `CGPDFContextClose`——哪怕中途出错提前 return，也不会
/// 落一个没写交叉引用表的坏 PDF 在磁盘上。
pub struct PdfStream {
    /// 裸指针：`BeginPage` / `EndPage` / `Close` 这三个入口 core-graphics
    /// 没包，只能自己声明（见文件头 `extern "C"`）。
    raw: sys::CGContextRef,
    /// 同一对象的 Rust 包装（持所有权，Drop 时 release）。
    ///
    /// ⚠️ `raw` 与 `ctx` **指向同一个对象**：`raw` 只用来调那三个 C 入口，
    /// 生命周期交给 `ctx`。绝不能再 `from_ptr` 一次（会双 release）。
    ctx: CGContext,
    brush: Brush,
    /// 已写页数。
    pages: usize,
    /// 落位（各叶共用，算一次存着）。
    place: Option<Place>,
    /// 是否已经 `Close`（`Drop` 里防重复关）。
    closed: bool,
}

impl PdfStream {
    /// 开一份新的 PDF（**先建文件，再一页页喂**）。
    pub fn begin(path: &Path) -> Result<Self, String> {
        let url = CFURL::from_path(path, false)
            .ok_or_else(|| format!("路径无法转成文件 URL：{}", path.display()))?;
        let media = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(A4_W, A4_H));
        let raw = unsafe { CGPDFContextCreateWithURL(url.as_concrete_TypeRef(), &media, std::ptr::null()) };
        if raw.is_null() {
            return Err("无法创建 PDF 画布（CoreGraphics 返回空）".to_string());
        }
        // `from_ptr` 接管所有权：Drop 时 `CGContextRelease`，与
        // `CGPDFContextClose` 一前一后，绝不泄漏。
        let ctx = unsafe { CGContext::from_ptr(raw) };
        ctx.set_allows_antialiasing(true);
        ctx.set_should_antialias(true);
        // ⚠️ 先 `clone` 再 move：`Brush` 要的是同一台上下文的另一份引用
        // （`CGContext` 的 `Clone` 是 retain，`Drop` 各 release 一次，配平）。
        let brush = Brush::new(ctx.clone(), A4_H);
        Ok(Self {
            raw,
            ctx,
            brush,
            pages: 0,
            place: None,
            closed: false,
        })
    }

    /// 写一叶：开页 → 铺纸 → 落笔 → 收页 → **flush**。
    ///
    /// 这一页画完，本模块**不留下任何属于这一页的东西**（`Page` / `Piece`
    /// 都是调用方持有的借用，`draw_leaf` 只是照着读一遍），所以内存不会
    /// 随页数累积。
    pub fn push(&mut self, leaf: &PdfLeaf<'_>, g: &PdfGeo) {
        // ⚠️ **必须先开页再落笔**：CG 的 PDF 上下文在 `BeginPage` 之前
        // 收到的绘制会被丢掉，页面只剩一个空的内容流（控制台会报
        // “No content stream…”）。整页底色因此也在开页之后铺。
        unsafe { CGPDFContextBeginPage(self.raw, std::ptr::null()) };
        // 纸色铺满 A4（纸面之外也是纸，印出来才不会有白边感）。
        let (pr, pg, pb) = rgb(g.paper);
        self.brush.ctx.set_rgb_fill_color(pr, pg, pb, 1.0);
        self.brush.ctx.fill_rect(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(A4_W, A4_H),
        ));
        let place = *self.place.get_or_insert_with(|| place_of(g));
        draw_leaf(&mut self.brush, leaf, g, &place);
        unsafe { CGPDFContextEndPage(self.raw) };
        // 催 CG 把刚收的这一页写下去（不等 `Close` 时才一次性吐）。
        self.brush.ctx.flush();
        self.pages += 1;
    }

    /// 关文件（写交叉引用表）。返回写出的**页数**。
    pub fn finish(mut self) -> Result<usize, String> {
        self.close();
        let n = self.pages;
        if self.brush.missing > 0 {
            // 绝不静默：印厂拿到纸才发现少了字就晚了。
            eprintln!(
                "[笺读] PDF 印制完成 {n} 叶，其中 {} 枚字在所有字体中都查不到字形（已跳过，未印）",
                self.brush.missing
            );
        }
        Ok(n)
    }

    /// 已写页数（中途看进度用）。
    pub fn pages(&self) -> usize {
        self.pages
    }

    fn close(&mut self) {
        if !self.closed {
            unsafe { CGPDFContextClose(self.raw) };
            self.closed = true;
        }
    }
}

impl Drop for PdfStream {
    fn drop(&mut self) {
        self.close();
    }
}

// ── 对外入口 ──────────────────────────────────────────────────────────
/// 把若干叶写成一份 **A4 矢量 PDF**（一叶一页，**逐页流式落盘**）。
/// 返回写出的页数。
///
/// 失败一律返回可读原因，绝不 panic（导出路径最怕「崩在半路」）。
pub fn write_pdf(path: &Path, leaves: &[PdfLeaf], g: &PdfGeo) -> Result<usize, String> {
    if leaves.is_empty() {
        return Err("没有可印制的叶面".to_string());
    }
    let mut s = PdfStream::begin(path)?;
    // 逐叶喂给写手：**写完一叶就落盘**，绝不攒一个「整本的绘制结果」。
    for leaf in leaves {
        s.push(leaf, g);
    }
    s.finish()
}

/// **迭代器版**入口：调用方可以边排版边喂（一叶一叶来），
/// 本模块始终只持有一叶的量——《资治通鉴》那种体量靠这条入口才真流式。
pub fn write_pdf_streaming<'a, I>(
    path: &Path,
    g: &PdfGeo,
    leaves: I,
) -> Result<usize, String>
where
    I: IntoIterator<Item = PdfLeaf<'a>>,
{
    let mut s = PdfStream::begin(path)?;
    for leaf in leaves {
        s.push(&leaf, g);
    }
    match s.finish() {
        Ok(0) => Err("没有可印制的叶面".to_string()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gloss::Page;
    use crate::types::{Glyph, NoteSource};

    /// 造一叶「正文 + 双行夹注」，验证 **PDF 是矢量**：有 `%PDF-` 头、
    /// 有 `/Type /Page`、并且**内嵌了字体**（`/Font`）而不是图片。
    #[test]
    fn writes_a_vector_pdf_with_embedded_fonts() {
        let main = |ch: char, off: usize| Glyph {
            chapter: 0,
            para: 0,
            offset: off,
            ch,
            kind: GlyphKind::Char,
        };
        let body: Vec<char> = "項籍者下相人也字羽初起時年二十四其季父項梁梁父即楚將項燕為秦將王翦所戮者也"
            .chars()
            .collect();
        let mut col: Vec<Piece> = Vec::new();
        for (i, c) in body.iter().enumerate() {
            col.push(Piece::Main(main(*c, i)));
            if i == 6 {
                // 双行夹注：五字 → 三行（每行两字），占 1.5 格 → 取整 2 格。
                col.push(Piece::Gloss(
                    "下相地名".to_string(),
                    NoteSource::Original,
                    main(*c, i),
                ));
            }
        }
        let page = Page {
            glyphs: body
                .iter()
                .enumerate()
                .map(|(i, c)| main(*c, i))
                .collect(),
            start_chapter: false,
            columns: vec![col],
        };
        let g = PdfGeo {
            cols: 8,
            cell_w: 46.0,
            cell_h: 66.0,
            rows: 10,
            fs: 36.0,
            reserve: 0,
            edge: 40.0,
            margin_top: 40.0,
            margin_bot: 26.0,
            paper: 0xF5EFE2,
            ink: 0x1A1A1A,
            rule: 0x8A7A5C,
            rule_soft: 0x8A7A5C,
            frame_style: crate::types::FrameStyle::default(),
            note_ink: 0xC3272D,
            accent: 0xC3272D,
            font: PdfFontKind::Song,
            note_font: PdfFontKind::Kai,
            han_digits: false,
            traditional: false,
            punct_mode: crate::gloss::PunctMode::Modern,
            punct_ink: 0xC3272D,
        };
        // 注音表：给首个字挂一条，验证「注音也进 PDF」这条链路不被优化掉。
        let mut ruby_map: std::collections::HashMap<(usize, usize, usize), String> =
            std::collections::HashMap::new();
        if let Some(g0) = page.glyphs.first() {
            ruby_map.insert((g0.chapter, g0.para, g0.offset), "duǒ".to_string());
        }
        let dir = std::env::temp_dir().join("jiandu-pdf-test");
        std::fs::create_dir_all(&dir).ok();
        let out = dir.join("leaf.pdf");
        let leaf = PdfLeaf {
            page: &page,
            title: "項羽本紀",
            chapter: 0,
            leaf: 1,
            inscription: "",
            author: "",
            ruby: &ruby_map,
        };
        let n = write_pdf(&out, std::slice::from_ref(&leaf), &g).expect("写 PDF 失败");
        assert_eq!(n, 1);
        let bytes = std::fs::read(&out).expect("读回 PDF 失败");
        let text = String::from_utf8_lossy(&bytes);
        assert!(bytes.starts_with(b"%PDF-"), "文件头不是 %PDF-");
        assert!(text.contains("/Type /Page") || text.contains("/Type/Page"), "没有页面对象");
        // 真字：文字走内嵌字体，而不是位图。
        assert!(text.contains("/Font"), "字体未内嵌——文字没落成矢量");
        std::fs::remove_file(&out).ok();
    }

    /// **三种版框形制都印得出来**（v1.85.0）。
    ///
    /// 同一叶纸印三份到临时目录（`jiandu-frame-check/`），留给人眼验收
    /// 「像不像刀刻」；这里只钉「不报错、页数对、文件是 PDF」。
    #[test]
    fn all_three_frame_styles_print() {
        use crate::types::FrameStyle;
        let main = |ch: char, off: usize| Glyph {
            chapter: 0,
            para: 0,
            offset: off,
            ch,
            kind: GlyphKind::Char,
        };
        let body: Vec<char> =
            "版框者一書之面也四周雙邊文武為欄左右雙邊坊刻常見四周單邊公文紙式".chars().collect();
        let col: Vec<Piece> = body
            .iter()
            .enumerate()
            .map(|(i, c)| Piece::Main(main(*c, i)))
            .collect();
        let page = Page {
            glyphs: body
                .iter()
                .enumerate()
                .map(|(i, c)| main(*c, i))
                .collect(),
            start_chapter: false,
            columns: vec![col],
        };
        let dir = std::env::temp_dir().join("jiandu-frame-check");
        std::fs::create_dir_all(&dir).ok();
        let ruby = std::collections::HashMap::new();
        let leaf = PdfLeaf {
            page: &page,
            title: "版框",
            chapter: 0,
            leaf: 1,
            inscription: "",
            author: "",
            ruby: &ruby,
        };
        for style in FrameStyle::ALL {
            let g = PdfGeo {
                frame_style: style,
                ..test_geo()
            };
            let out = dir.join(format!("frame-{}.pdf", style.index()));
            let n = write_pdf(&out, std::slice::from_ref(&leaf), &g)
                .unwrap_or_else(|e| panic!("{} 印不出来：{e}", style.label()));
            assert_eq!(n, 1, "{} 页数不对", style.label());
            let bytes = std::fs::read(&out).expect("读回 PDF 失败");
            assert!(bytes.starts_with(b"%PDF-"), "{} 不是 PDF", style.label());
        }
    }

    /// **版框几何**（v1.85.0）：间距永不粘连、且与字号无关。
    ///
    /// 这是用户点名的「拿刀刻」那条要求：字体放大缩小只改字格，框层的
    /// 两根线该在哪儿还在哪儿——间距是个常量，不是字号的函数。
    #[test]
    fn frame_plan_keeps_the_two_lines_apart_at_any_type_size() {
        // ① 同一个 edge、不同字格 / 行数：净距一模一样（字大只让格子变大）。
        let mut gaps = Vec::new();
        for (cell, rows) in [(24.0, 28), (30.0, 22), (46.0, 10), (60.0, 8)] {
            let plan = frame_plan(34.0, 100.0, 120.0, cell * 12.0, cell * 1.4 * rows as f64);
            gaps.push(plan.gap());
            // 中心距恒 = FRAME_GAP。
            assert!(
                (plan.inner.0 - plan.outer.0 - FRAME_GAP).abs() < 1e-9,
                "内外线中心距被字级带偏：{:.3}",
                plan.inner.0 - plan.outer.0
            );
        }
        for w in gaps.windows(2) {
            assert!((w[0] - w[1]).abs() < 1e-9, "字号一变净距就漂了：{:?}", gaps);
        }
        // ② 任何 edge 都留得出 2pt 以上净距（A4 缩放的极端值都在里面）。
        for edge in [10.0_f64, 14.0, 20.0, 34.0, 40.0, 56.0] {
            let plan = frame_plan(edge, 0.0, 0.0, 360.0, 600.0);
            assert!(
                plan.gap() >= 2.0,
                "edge={edge} 时双线贴到了一起：净距 {:.2}",
                plan.gap()
            );
            // ③ 内线缩在外线之内（不许戳出纸缘、也不许越出外框）。
            assert!(plan.inner.0 > plan.outer.0, "edge={edge} 内线越过了外框");
            assert!(plan.inner.2 < plan.outer.0 + plan.outer.2, "edge={edge} 内线戳出右缘");
        }
        // ④ 外框整根落在纸面之内：路径内收半个线宽。
        let plan = frame_plan(34.0, 100.0, 120.0, 360.0, 600.0);
        assert!((plan.outer.0 - (100.0 - 17.0 + 0.5)).abs() < 1e-9);
        assert!((plan.outer.2 - (360.0 + 34.0 - 1.0)).abs() < 1e-9);
        // ⑤ 外框四条边的位置与字架无关（不碰网格：字格区一个像素都不动）。
        let a = frame_plan(34.0, 100.0, 120.0, 360.0, 600.0);
        let b = frame_plan(34.0, 100.0, 120.0, 300.0, 500.0);
        assert_eq!(a.outer.0, b.outer.0, "改字格宽不该挪动外框左缘");
        assert_eq!(a.outer.1, b.outer.1, "改行数不该挪动外框上缘");
    }

    /// **形制与四周表同源**：屏幕裁内框、纸上裁内线，用的是同一个函数。
    #[test]
    fn inner_sides_follow_the_frame_style() {
        use crate::types::FrameStyle;
        let all = [true; 4];
        assert_eq!(FrameStyle::DoubleAll.inner_sides(all), [true; 4]);
        // 左右双边：只留左右两道竖线（上 / 下是单线）。
        assert_eq!(
            FrameStyle::DoubleSides.inner_sides(all),
            [false, true, false, true]
        );
        // 四周单边：内层整圈不画。
        assert_eq!(FrameStyle::Single.inner_sides(all), [false; 4]);
        // 四边表的「缺边」照样生效（三层裁边是**叠加**，不是覆盖）。
        let no_bottom = [true, true, false, true];
        assert_eq!(
            FrameStyle::DoubleAll.inner_sides(no_bottom),
            [true, true, false, true]
        );
        // 档位越界回落出厂值（老存档 / 手改档都不许 panic）。
        assert_eq!(FrameStyle::from_index(99), FrameStyle::DoubleAll);
        assert_eq!(FrameStyle::DoubleAll.index(), 0);
    }

    /// 纸面尺寸 = 字格区 + 版框 + 天头地脚；A4 上必须放得下。
    #[test]
    fn sheet_fits_on_a4() {
        let g = PdfGeo {
            cols: 12,
            cell_w: 30.0,
            cell_h: 40.0,
            rows: 16,
            fs: 22.0,
            reserve: 0,
            edge: 40.0,
            margin_top: 40.0,
            margin_bot: 26.0,
            paper: 0xFFFFFF,
            ink: 0x000000,
            rule: 0x000000,
            rule_soft: 0x000000,
            frame_style: crate::types::FrameStyle::default(),
            note_ink: 0x000000,
            accent: 0xC3272D,
            font: PdfFontKind::Song,
            note_font: PdfFontKind::Kai,
            han_digits: false,
            traditional: false,
            punct_mode: crate::gloss::PunctMode::Modern,
            punct_ink: 0xC3272D,
        };
        assert!(g.sheet_w() as f64 <= A4_W, "纸宽超出 A4");
        assert!(g.sheet_h() as f64 <= A4_H, "纸高超出 A4");
    }

    /// 测试用的版式（与上一个用例同制，抽出来给下面几条共用）。
    fn test_geo() -> PdfGeo {
        PdfGeo {
            cols: 8,
            cell_w: 46.0,
            cell_h: 66.0,
            rows: 10,
            fs: 36.0,
            reserve: 0,
            edge: 40.0,
            margin_top: 40.0,
            margin_bot: 26.0,
            paper: 0xF5EFE2,
            ink: 0x1A1A1A,
            rule: 0x8A7A5C,
            rule_soft: 0x8A7A5C,
            frame_style: crate::types::FrameStyle::default(),
            note_ink: 0xC3272D,
            accent: 0xC3272D,
            font: PdfFontKind::Song,
            note_font: PdfFontKind::Kai,
            han_digits: false,
            traditional: false,
            punct_mode: crate::gloss::PunctMode::Modern,
            punct_ink: 0xC3272D,
        }
    }

    /// 由一串字造一叶（正文单列，无夹注）。
    fn page_of(s: &str) -> Page {
        let glyphs: Vec<Glyph> = s
            .chars()
            .enumerate()
            .map(|(i, c)| Glyph {
                chapter: 0,
                para: 0,
                offset: i,
                ch: c,
                kind: GlyphKind::Char,
            })
            .collect();
        let columns = vec![glyphs
            .iter()
            .map(|g| Piece::Main(*g))
            .collect::<Vec<Piece>>()];
        Page {
            glyphs,
            start_chapter: false,
            columns,
        }
    }

    fn tmp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("jiandu-pdf-test");
        std::fs::create_dir_all(&dir).ok();
        dir
    }

    /// **UTF-16 缓冲必须精确**：BMP 一枚、扩展区两枚（代理对）。
    ///
    /// 钉的是 v1.85.3 那个真崩溃：`[u16; 1]` 装不下 `U+27B82`，
    /// `encode_utf16` panic，整本《离骚》《资治通鉴》导出当场死。
    #[test]
    fn utf16_units_are_exact_for_bmp_and_astral() {
        let (u, n) = utf16_units('a');
        assert_eq!(n, 1, "BMP 只要一枚码元");
        assert_eq!(u[0], 0x0061);

        let (u, n) = utf16_units('\u{27B82}');
        assert_eq!(n, 2, "扩展 B 区必须是两枚码元（代理对）");
        assert!((0xD800..0xDC00).contains(&u[0]), "首枚应是高位代理");
        assert!((0xDC00..0xE000).contains(&u[1]), "次枚应是低位代理");
        // 反解回来：一枚字都没丢。
        let back = String::from_utf16(&u[..n]).expect("反解失败");
        assert_eq!(back.chars().next().unwrap(), '\u{27B82}');

        // 最容易写错的那两个口径，这里一并钉死它们**不该**被拿来定缓冲：
        let s = "\u{27B82}";
        assert_eq!(s.len(), 4, "UTF-8 是 4 字节 ≠ 码元数");
        assert_eq!(s.chars().count(), 1, "码点数 ≠ 码元数");
        assert_eq!(s.encode_utf16().count(), 2, "真正要的是这个：2");
    }

    /// **生僻字不崩、且真落进 PDF**（扩展 B 区 + 扩展 B 区外的另一枚）。
    #[test]
    fn astral_rare_chars_do_not_crash_and_land_in_the_pdf() {
        // `\u{27B82}` 就是线上把导出打崩的那一枚；`\u{20000}` 是扩展 B 区首字。
        let body = "扈江離與辟芷兮\u{27B82}紉秋蘭以為佩\u{20000}汨余若將不及兮恐年歲之不吾與";
        let page = page_of(body);
        let ruby = std::collections::HashMap::new();
        let g = test_geo();
        let out = tmp_dir().join("astral.pdf");
        let leaf = PdfLeaf {
            page: &page,
            title: "離騷",
            chapter: 0,
            leaf: 1,
            inscription: "",
            author: "",
            ruby: &ruby,
        };
        // 旧实现在这里 panic（encode_utf16 越界）——能走到下一行就算过。
        // 但「没崩」还不够：还要**真印上去**（走字体回退找到字形），
        // 所以直接用写手，收工时看缺字计数必须是 0。
        let mut s = PdfStream::begin(&out).expect("开 PDF 失败");
        s.push(&leaf, &g);
        assert_eq!(
            s.brush.missing, 0,
            "扩展区生僻字一枚都没印上——字体回退没生效"
        );
        let n = s.finish().expect("写 PDF 失败");
        assert_eq!(n, 1);
        let bytes = std::fs::read(&out).expect("读回 PDF 失败");
        assert!(bytes.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Font"), "字体未内嵌");
        std::fs::remove_file(&out).ok();
    }

    /// **逐页流式**：写一叶就落一叶，`pages()` 随时可读，收笔页数对得上。
    #[test]
    fn streaming_writer_writes_and_releases_one_page_at_a_time() {
        let page = page_of("帝曰咨汝羲暨和朞三百有六旬有六日以閏月定四時成歲");
        let ruby = std::collections::HashMap::new();
        let g = test_geo();
        let out = tmp_dir().join("stream.pdf");
        let mut s = PdfStream::begin(&out).expect("开 PDF 失败");
        for i in 0..40usize {
            let leaf = PdfLeaf {
                page: &page,
                title: "堯典",
                chapter: 0,
                leaf: i + 1,
                inscription: "",
                author: "",
                ruby: &ruby,
            };
            s.push(&leaf, &g);
            assert_eq!(s.pages(), i + 1, "每推一叶，页数就该加一");
        }
        let n = s.finish().expect("收笔失败");
        assert_eq!(n, 40);
        let bytes = std::fs::read(&out).expect("读回 PDF 失败");
        assert!(bytes.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("/Type /Page") || text.contains("/Type/Page"),
            "没有页面对象"
        );
        std::fs::remove_file(&out).ok();
    }

    /// **整本压力**（冒烟）：三百叶一气写完，页数不丢、不崩、不超内存预期。
    ///
    /// 这里断言的是「页数相等 + 真有 300 个页面对象」——内存是否真的不随
    /// 页数涨，靠的是 `PdfStream` 不持有任何一页产物（见该类型的文档），
    /// 单测测不到 RSS，但这条能保证「写得完、写得对」。
    #[test]
    fn three_hundred_leaves_stream_out_in_one_pass() {
        let page = page_of("資治通鑑卷第一周紀一起著雍攝提格盡玄黓困敦凡三十五年");
        let ruby = std::collections::HashMap::new();
        let g = test_geo();
        let out = tmp_dir().join("bulk.pdf");
        // 想压测就自己加量：`JIANDU_PDF_STRESS=5000 cargo test -p jiandu three_hundred`。
        let total: usize = std::env::var("JIANDU_PDF_STRESS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        let leaves: Vec<PdfLeaf> = (0..total)
            .map(|i| PdfLeaf {
                page: &page,
                title: "資治通鑑",
                chapter: 0,
                leaf: i + 1,
                inscription: "",
                author: "",
                ruby: &ruby,
            })
            .collect();
        let n = write_pdf(&out, &leaves, &g).expect("整本写失败");
        assert_eq!(n, total);
        // 压测模式（`JIANDU_PDF_STRESS` 已设）两件事：
        //   · 文件**留着**不删，方便你去看体积 / 用预览器翻；
        //   · 设 `JIANDU_PDF_STRESS_NOREAD=1` 可跳过「读回整份文件」这步——
        //     那一步是**测试自己**的开销（整份 PDF 进内存再转 String 扫页面数），
        //     不是写手的开销。想量「光写要多少内存」，就把它设上。
        let stress = std::env::var("JIANDU_PDF_STRESS").is_ok();
        if !std::env::var("JIANDU_PDF_STRESS_NOREAD").is_ok() {
            let bytes = std::fs::read(&out).expect("读回 PDF 失败");
            assert!(bytes.starts_with(b"%PDF-"));
            let text = String::from_utf8_lossy(&bytes);
            let n_page =
                text.matches("/Type /Page").count() + text.matches("/Type/Page").count();
            assert!(n_page >= total, "页面对象只有 {n_page} 个，少于 {total} 叶");
        }
        if !stress {
            std::fs::remove_file(&out).ok();
        }
    }
}

// ── 平台后端注册（v1.0.0-beta）──────────────────────────────────────────────
//
// core 只定义 `PdfExporter` 这个「要说的话」，这里是 macOS 的「说话方式」。
// 将来 Windows / 移动端各写一个 `impl PdfExporter`，`exporter()` 里按
// `#[cfg]` 分派即可——`LeafSpec` 与 `PdfGeometry` 一份都不用改。

/// macOS：**矢量**后端（CGPDFContext）。字是真字、内嵌字体子集，
/// 任意倍放大不糊、可复制可检索，一叶几十 KB。
#[cfg(target_os = "macos")]
pub struct CoreGraphicsPdfExporter;

#[cfg(target_os = "macos")]
impl PdfExporter for CoreGraphicsPdfExporter {
    fn name(&self) -> &'static str {
        "coregraphics"
    }

    fn render_leaves(&self, leaves: &[LeafSpec<'_>], geo: &PdfGeometry) -> anyhow::Result<Vec<u8>> {
        if leaves.is_empty() {
            anyhow::bail!("没有可印制的叶面");
        }
        // CG 只肯写 URL，而保存框要的是字节 → 先落临时文件再读回。
        // （与 `exporter::save_bytes` 的「唯一写盘出口」不冲突：那个管的是
        //  **用户最终选中的路径**，这里是渲染过程的中间产物。）
        let dir = std::env::temp_dir().join("jiandu-pdf");
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!("leaf-{}-{}.pdf", std::process::id(), leaves[0].leaf));
        write_pdf(&tmp, leaves, geo).map_err(|e| anyhow::anyhow!(e))?;
        let bytes = std::fs::read(&tmp)?;
        let _ = std::fs::remove_file(&tmp);
        Ok(bytes)
    }
}

/// 本平台该用哪个 PDF 后端。
///
/// 非 macOS 时退回 core 的 `UnavailablePdfExporter`——**明确报错**，
/// 而不是给用户一张能打开、但一片空白的 PDF 让他怀疑自己的书坏了。
pub fn exporter() -> Box<dyn PdfExporter> {
    #[cfg(target_os = "macos")]
    {
        Box::new(CoreGraphicsPdfExporter)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(jiandu_core::platform::UnavailablePdfExporter)
    }
}
