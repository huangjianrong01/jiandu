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
//! ── 两条防雷（与 `share_paint` 同一口径）────────────────────────
//! 1. **不翻 CTM**：`CGPDFContext` 与位图上下文同是 CG 坐标（原点左下、
//!    y 向上），版式数据是屏幕坐标（原点左上）。只把每个元素的 y 做一次
//!    镜像（[`Brush::cy`]），**绝不动 CTM**——一动字形就被垂直镜像。
//! 2. **CFRelease**：`CGContext` / `CFURL` 都是 `TCFType` 包装，`Drop` 自动
//!    release；字体按 (字形类, 字号) 缓存复用，一叶只建两三台字体对象。

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

// ── 画笔 ──────────────────────────────────────────────────────────────
/// 一支笔：持一个 CG 上下文（**PDF**），把「屏幕坐标」翻成 CG 坐标落笔。
struct Brush {
    ctx: CGContext,
    /// 页高（换算用）。
    h: f64,
    /// 字体缓存：`(字形类, 字号×4 取整) → (字体, ascent, descent)`。
    fonts: Vec<(ShareFont, i32, CTFont, f64, f64)>,
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
        self.fonts.push((kind, key, font.clone(), asc, desc));
        Ok((font, asc, desc))
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

    /// 落一枚字：横向按 `cx` 居中、纵向按行盒居中。
    fn glyph_at(
        &mut self,
        ch: char,
        cx: f64,
        y: f64,
        h: f64,
        fs: f64,
        color: u32,
        kind: ShareFont,
    ) {
        // 字形回退：宋体缺 `ō ǎ ě` 这类带调元音时（拼音注音的老毛病），
        // 依次试楷 / 黑——只要有一个字体有这枚字就落笔，绝不静默丢字
        // （PDF 里「少一个 ī」比「字形风格不统一」难看得多）。
        let mut unit: [u16; 1] = [0];
        ch.encode_utf16(&mut unit);
        let chain = [kind, ShareFont::Kai, ShareFont::Hei, ShareFont::Song];
        for k in chain {
            let (font, asc, desc) = match self.font(k, fs) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut glyph: core_graphics::font::CGGlyph = 0;
            let ok = unsafe { font.get_glyphs_for_characters(unit.as_ptr(), &mut glyph, 1) };
            if !ok {
                continue;
            }
            let baseline = y + (h - (asc + desc)) * 0.5 + asc;
            let (r, g, b) = rgb(color);
            self.ctx.set_rgb_fill_color(r, g, b, 1.0);
            font.draw_glyphs(
                &[glyph],
                &[CGPoint::new(cx - fs * 0.5, self.cy(baseline))],
                self.ctx.clone(),
            );
            return;
        }
    }
}

/// 竖排注音：**紧凑竖叠**、贴字格右侧（与屏幕 `ruby::layout` 同一套几何）。
///
/// 屏幕那侧是 GPUI 里一串竖排小 div；这里是 CG 逐字母落笔。两者共用
/// `jiandu_core::ruby::layout` 的度量（字号 40%、步进 0.82、长拼音收敛下限
/// 22%、右侧留呼吸间距），所以 PDF 里的 `bō rě` 与屏幕逐字同位——
/// **绝不横向排**（横向会撑破一格宽、压到邻列），纵向也绝不溢出格高。
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
    // 右上角落点（屏幕坐标）：右缘内缩一个 `rl.gutter`（与屏幕同一套度量）。
    let right = x + cell_w - rl.gutter as f64;
    let top = y + rl.top as f64;
    if draw_ruby_rotated(b, py, right, top, &rl, g.note_ink, ShareFont::from(g.note_font)).is_err() {
        // 退化：逐字母竖叠（步进取字母全高，绝不叠墨）。
        let bw = rl.block_w as f64;
        let step = rl.fs as f64 * 1.02;
        let mut ty = top;
        for c in py.chars() {
            b.glyph_at(
                c,
                right - bw + bw * 0.5,
                ty,
                step,
                rl.fs as f64,
                g.note_ink,
                ShareFont::from(g.note_font),
            );
            ty += step;
        }
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

/// 整串**顺时针旋转 90°** 落笔（与屏幕 `ruby_glyph` 同一套度量）。
///
/// 纸面上 CoreGraphics 的 CTM **能真转**（不像 GPUI 把变换矩阵写死），
/// 所以这里直接 `translate → rotate(-90°) → 逐字落笔`：
/// 局部 `+x`（读序）→ 全局向下；局部 `+y`（字母的上）→ 全局向右。
fn draw_ruby_rotated(
    b: &mut Brush,
    py: &str,
    right_x: f64,
    top_y: f64,
    rl: &crate::ruby::RubyLayout,
    color: u32,
    kind: ShareFont,
) -> Result<(), String> {
    use core_graphics::font::CGGlyph;
    use core_graphics::geometry::{CGPoint, CGSize};
    use core_text::font_descriptor::kCTFontOrientationHorizontal;

    let fs = rl.fs as f64;
    let (font, asc, _desc) = b.font(kind, fs)?;
    let mut glyphs: Vec<CGGlyph> = Vec::new();
    let mut advs: Vec<f64> = Vec::new();
    for c in py.chars() {
        let mut unit: [u16; 1] = [0];
        c.encode_utf16(&mut unit);
        let mut gl: CGGlyph = 0;
        let ok = unsafe { font.get_glyphs_for_characters(unit.as_ptr(), &mut gl, 1) };
        if !ok {
            return Err(format!("字体缺字形 {c:?}"));
        }
        let mut sz = CGSize::new(0.0, 0.0);
        unsafe {
            font.get_advances_for_glyphs(kCTFontOrientationHorizontal, &gl, &mut sz, 1)
        };
        glyphs.push(gl);
        advs.push(sz.width);
    }
    let run_w: f64 = advs.iter().sum();
    if run_w <= 0.0 {
        return Err("串宽为零".to_string());
    }
    // 真实串宽比 core 预留的长时整体缩一档——绝不溢出到同列下一个字。
    let k = (rl.block_h as f64 / run_w).min(1.0);

    let (r, gg, bb) = rgb(color);
    b.ctx.save();
    // **顺时针 90°**（与屏幕 `ruby_glyph` 同向）：字母顶部朝 +x，故最右一列
    // 是**升部**，基线落在 `right_x - asc`；串自上而下读，起点（局部 x=0）
    // 在整条的顶端 `top_y`。
    b.ctx.translate(right_x - asc, b.cy(top_y));
    b.ctx.rotate(-std::f64::consts::FRAC_PI_2);
    if k < 1.0 {
        b.ctx.scale(k, k);
    }
    b.ctx.set_rgb_fill_color(r, gg, bb, 1.0);
    let mut ax = 0.0f64;
    for i in 0..glyphs.len() {
        font.draw_glyphs(&[glyphs[i]], &[CGPoint::new(ax, 0.0)], b.ctx.clone());
        ax += advs[i];
    }
    b.ctx.restore();
    Ok(())
}

// ── 画一叶 ────────────────────────────────────────────────────────────
/// 单叶在纸面上的落位（由 [`PdfGeo`] + A4 算出，**所有叶共用**）。
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

    // ── 版框：刻本「四周双边」——外框一粗、内框一细 ──────────────────
    let fw = cols as f64 * cell_w;
    b.rect(x0 - pad, y0 - pad, fw + pad * 2.0, block_h + pad * 2.0, g.rule, Some(1.0));
    b.rect(
        x0 - pad + 3.0,
        y0 - pad + 3.0,
        fw + pad * 2.0 - 6.0,
        block_h + pad * 2.0 - 6.0,
        g.rule_soft,
        Some(0.5),
    );

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

// ── 对外入口 ──────────────────────────────────────────────────────────
/// 把若干叶写成一份 **A4 矢量 PDF**（一叶一页）。返回写出的页数。
///
/// 失败一律返回可读原因，绝不 panic（导出路径最怕「崩在半路」）。
pub fn write_pdf(path: &Path, leaves: &[PdfLeaf], g: &PdfGeo) -> Result<usize, String> {
    if leaves.is_empty() {
        return Err("没有可印制的叶面".to_string());
    }
    let url = CFURL::from_path(path, false).ok_or_else(|| {
        format!("路径无法转成文件 URL：{}", path.display())
    })?;
    let media = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(A4_W, A4_H));
    let raw = unsafe {
        CGPDFContextCreateWithURL(
            url.as_concrete_TypeRef(),
            &media,
            std::ptr::null(),
        )
    };
    if raw.is_null() {
        return Err("无法创建 PDF 画布（CoreGraphics 返回空）".to_string());
    }
    // `from_ptr` 接管所有权：Drop 时 `CGContextRelease`，与 CGPDFContextClose
    // 一前一后，绝不泄漏。
    let ctx = unsafe { CGContext::from_ptr(raw) };
    ctx.set_allows_antialiasing(true);
    ctx.set_should_antialias(true);
    let mut b = Brush::new(ctx, A4_H);
    let place = place_of(g);
    for leaf in leaves {
        // ⚠️ **必须先开页再落笔**：CG 的 PDF 上下文在 `BeginPage` 之前
        // 收到的绘制会被丢掉，页面只剩一个空的内容流（控制台会报
        // “No content stream…”）。整页底色因此也在开页之后铺。
        unsafe { CGPDFContextBeginPage(raw, std::ptr::null()) };
        // 纸色铺满 A4（纸面之外也是纸，印出来才不会有白边感）。
        let (pr, pg, pb) = rgb(g.paper);
        b.ctx.set_rgb_fill_color(pr, pg, pb, 1.0);
        b.ctx.fill_rect(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(A4_W, A4_H),
        ));
        draw_leaf(&mut b, leaf, g, &place);
        unsafe { CGPDFContextEndPage(raw) };
    }
    // 收笔：写完所有页再 close（它把交叉引用表落盘）。
    let n = leaves.len();
    unsafe { CGPDFContextClose(raw) };
    drop(b);
    Ok(n)
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
