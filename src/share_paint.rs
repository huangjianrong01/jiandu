//! 分享卡的**离屏光栅器**（v1.57.0）：CoreGraphics 位图上下文 + CoreText 逐字落笔。
//!
//! 为什么不截屏：旧制 `screencapture` 依赖屏幕录制权限、分辨率随窗口走、
//! 锁屏时直接失败。这里是**真离屏**——自己开一块 `width × height` 的位图，
//! 按 `share_layout::CardLayout` 的指令画完，再编码成 PNG。1080×1920 与
//! 屏幕、窗口、权限全部无关。
//!
//! 两处必防雷（用户点名）：
//! 1. **坐标系翻转**：`CGBitmapContextCreate` 出来的是 CG 坐标（原点**左下**、
//!    y 向上），而版式数据是屏幕坐标（原点左上、y 向下）。不翻就整张上下颠倒。
//!    故一开上下文就 `translate(0, h)` + `scale(1, -1)`，此后一律按左上原点落笔。
//! 2. **CFRelease / 泄漏**：用到的 CoreFoundation 对象（`CGContext` /
//!    `CGColorSpace` / `CTFont`）都是 Rust 侧 `TCFType` 包装，`Drop` 自动
//!    `CFRelease`——不手动 retain、不用 `into_raw`、不 `mem::forget`。
//!    字体按字号缓存复用，几百字的卡片只建两三台字体对象。

use crate::share::CINNABAR;
use crate::share_layout::{CardLayout, PlacedChar, PlacedSeal, ShareFont, punct_box};

use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_text::font::new_from_name;

/// 位图格式：32 位大端（内存序 R,G,B,X）+ 忽略 alpha。
///
/// 取 `NoneSkipLast` 而非 `PremultipliedLast`：底是不透明纯色，文字抗锯齿直接
/// 在 RGB 空间与底色插值，省一次反预乘，字节序又正好与 `RgbaImage` 一致。
const BYTE_ORDER_32_BIG: u32 = 0x0400_0000;
const ALPHA_NONE_SKIP_LAST: u32 = 5;
const BITMAP_INFO: u32 = BYTE_ORDER_32_BIG | ALPHA_NONE_SKIP_LAST;

/// `0xRRGGBB` → 三分量（0~1）。
fn rgb(c: u32) -> (f64, f64, f64) {
    (
        ((c >> 16) & 0xFF) as f64 / 255.0,
        ((c >> 8) & 0xFF) as f64 / 255.0,
        (c & 0xFF) as f64 / 255.0,
    )
}

/// `0xRRGGBBAA` → 四分量。
fn rgba(c: u32) -> (f64, f64, f64, f64) {
    let (r, g, b) = rgb(c >> 8);
    (r, g, b, (c & 0xFF) as f64 / 255.0)
}

/// 沿回退链挑一台系统真有的字体；全都没有就报错（绝不 unwrap）。
fn pick_font(kind: ShareFont, fs: f64) -> Result<core_text::font::CTFont, String> {
    for name in kind.postscript_chain() {
        if let Ok(f) = new_from_name(name, fs) {
            return Ok(f);
        }
    }
    Err("系统里找不到可用的中文字体".to_string())
}

/// 一枚字的落笔点（字形原点 = 基线）：字格内居中，标点靠右上小盒。
fn glyph_origin(c: &PlacedChar, asc: f64, desc: f64) -> (f64, f64) {
    let (gx, gy, gfs) = if c.punct {
        punct_box(c)
    } else {
        (c.x, c.y, c.fs)
    };
    let fs = gfs as f64;
    if c.punct {
        // 句读：小盒左上起笔，基线压在盒底偏上（竖排句读挂右上角）。
        (gx as f64, gy as f64 + fs * 0.86)
    } else {
        // 居中：字形盒（asc 上 / desc 下）的中心对齐字格中心。
        let x = gx as f64 + c.cell as f64 * 0.5 - fs * 0.5;
        let baseline = gy as f64 + c.cell as f64 * 0.5 + (asc - desc) * 0.5;
        (x, baseline)
    }
}

/// 把一张 `CardLayout` 光栅成 PNG 字节（失败返回可读原因，绝不 panic）。
pub fn render_png(l: &CardLayout) -> Result<Vec<u8>, String> {
    let w = l.w.round().max(1.0) as usize;
    let h = l.h.round().max(1.0) as usize;

    // ---- 位图上下文（Drop → CGContextRelease；ColorSpace 同理）----
    let space = CGColorSpace::create_device_rgb();
    let mut ctx = CGContext::create_bitmap_context(None, w, h, 8, w * 4, &space, BITMAP_INFO);
    // 防雷 ①：翻成「左上原点、y 向下」，与 CardLayout 同构。
    ctx.translate(0.0, h as f64);
    ctx.scale(1.0, -1.0);
    ctx.set_allows_antialiasing(true);
    ctx.set_should_antialias(true);

    // ---- 底色 ----
    let (br, bg, bb) = rgb(l.bg);
    ctx.set_rgb_fill_color(br, bg, bb, 1.0);
    ctx.fill_rect(CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(w as f64, h as f64),
    ));

    // ---- 矩形：高亮底（填充）/ 界框（描边）----
    for r in &l.rects {
        let (cr, cg, cb, ca) = rgba(r.color);
        let rect = CGRect::new(
            &CGPoint::new(r.x as f64, r.y as f64),
            &CGSize::new(r.w as f64, r.h as f64),
        );
        match r.stroke {
            Some(lw) => {
                ctx.set_rgb_stroke_color(cr, cg, cb, ca);
                ctx.set_line_width(lw as f64);
                ctx.stroke_rect(rect);
            }
            None => {
                ctx.set_rgb_fill_color(cr, cg, cb, ca);
                ctx.fill_rect(rect);
            }
        }
    }

    // ---- 正文 / 批注 / 落款：逐字落笔 ----
    let mut fonts: Vec<(i32, core_text::font::CTFont, f64, f64)> = Vec::new();
    for c in &l.chars {
        let key = (c.fs * 4.0).round() as i32;
        let (font, asc, desc) = match fonts.iter().position(|(k, _, _, _)| *k == key) {
            Some(i) => (fonts[i].1.clone(), fonts[i].2, fonts[i].3),
            None => {
                let f = pick_font(l.font, c.fs as f64)?;
                let a = f.ascent();
                let d = f.descent();
                fonts.push((key, f.clone(), a, d));
                (f, a, d)
            }
        };
        let (gx, gy) = glyph_origin(c, asc, desc);
        // char → CGGlyph：UTF-16 码元（古籍用字都在 BMP，一个码元够用）。
        let mut unit: [u16; 1] = [0];
        c.ch.encode_utf16(&mut unit);
        let mut glyph: core_graphics::font::CGGlyph = 0;
        let ok = unsafe { font.get_glyphs_for_characters(unit.as_ptr(), &mut glyph, 1) };
        if !ok {
            continue; // 该字体无此字形（生僻字）：跳过，绝不崩整张卡。
        }
        let (fr, fg, fb) = rgb(c.color);
        ctx.set_rgb_fill_color(fr, fg, fb, 1.0);
        // `draw_glyphs` 按值取 context：`clone()` 底层是 `CGContextRetain`，
        // 随调用结束 drop（release），一增一减，绝不泄漏。
        font.draw_glyphs(&[glyph], &[CGPoint::new(gx, gy)], ctx.clone());
    }

    // ---- 朱批引线 ----
    for ln in &l.lines {
        let (r, g, b, a) = rgba(ln.color);
        ctx.set_rgb_stroke_color(r, g, b, a);
        ctx.set_line_width(ln.width as f64);
        ctx.begin_path();
        ctx.move_to_point(ln.x1 as f64, ln.y1 as f64);
        ctx.add_line_to_point(ln.x2 as f64, ln.y2 as f64);
        ctx.stroke_path();
    }

    // ---- 朱印 ----
    if let Some(s) = &l.seal {
        paint_seal(&ctx, s, l.bg);
    }

    // ---- 取像素 → PNG ----
    // `bytes_per_row` 可能带对齐填充，故逐行拷贝，绝不按 `w*4` 直接切片。
    let stride = ctx.bytes_per_row();
    let data = ctx.data();
    let mut buf: Vec<u8> = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let base = y * stride;
        for x in 0..w {
            let i = base + x * 4;
            buf.push(data[i]);
            buf.push(data[i + 1]);
            buf.push(data[i + 2]);
            buf.push(255);
        }
    }
    let img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(w as u32, h as u32, buf)
        .ok_or_else(|| "位图缓冲尺寸不符".to_string())?;
    let mut png: Vec<u8> = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败：{e}"))?;
    Ok(png)
}

/// 朱砂方章：**纯色红方块 + 黑色「笺读」二字**（横排居中）。
///
/// 先跑通闭环：不画鱼尾、不刻书名——印就是一枚朱砂底、墨字落款的方章。
fn paint_seal(ctx: &CGContext, s: &PlacedSeal, _paper: u32) {
    let (r, g, b) = rgb(CINNABAR);
    let sz = s.size as f64;
    ctx.set_rgb_fill_color(r, g, b, 1.0);
    ctx.fill_rect(CGRect::new(
        &CGPoint::new(s.x as f64, s.y as f64),
        &CGSize::new(sz, sz),
    ));

    // 印文「笺讀」：黑字、横排、居中（两字各占半方）。
    let fs = sz * 0.42;
    let font = match pick_font(ShareFont::Song, fs) {
        Ok(f) => f,
        Err(_) => return, // 没有字体就不刻字，方块照样在——绝不因印文失败整张图。
    };
    let asc = font.ascent();
    let desc = font.descent();
    let baseline = s.y as f64 + sz * 0.5 + (asc - desc) * 0.5;
    for (i, ch) in ['\u{7b46}', '\u{8b80}'].iter().enumerate() {
        let mut unit: [u16; 1] = [0];
        ch.encode_utf16(&mut unit);
        let mut glyph: core_graphics::font::CGGlyph = 0;
        let ok = unsafe { font.get_glyphs_for_characters(unit.as_ptr(), &mut glyph, 1) };
        if !ok {
            continue;
        }
        let x = s.x as f64 + sz * 0.08 + i as f64 * fs;
        ctx.set_rgb_fill_color(0.06, 0.06, 0.06, 1.0);
        font.draw_glyphs(&[glyph], &[CGPoint::new(x, baseline)], ctx.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::{ShareBg, ShareFrame, ShareSeal};
    use crate::share_layout::{LayoutSpec, layout};

    /// **闭环验收**：1080×1920 离屏出图——底色 + 竖排正文 + 朱砂方章，
    /// 落成一张真 PNG 写到桌面（用户核对用），并断言文件头与尺寸。
    #[test]
    fn renders_a_1080x1920_png_with_body_and_seal() {
        let body: Vec<char> = "道可道非常道名可名非常名無名天地之始有名萬物之母故常無欲以觀其妙常有欲以觀其徼此兩者同出而異名同謂之玄玄之又玄眾妙之門"
            .chars()
            .collect();
        let mut red = vec![false; body.len()];
        for i in 0..8 {
            red[i] = true; // 前八字划朱线
        }
        let spec = LayoutSpec {
            body: &body,
            red: &red,
            note: "此章言道為萬物之始",
            show_notes: true,
            sign: "道德經 · 第一章",
            show_sign: true,
            date: "二〇二六年十月",
            show_date: true,
            columns: 12,
            bg: ShareBg::Xuan,
            frame: ShareFrame::Red,
            seal: ShareSeal::Twin,
            seal_text: "笺读",
            font: ShareFont::Song,
            w: 1080.0,
            h: 1920.0,
        };
        let l = layout(&spec);
        let png = match render_png(&l) {
            Ok(p) => p,
            Err(e) => panic!("离屏出图失败：{e}"),
        };
        // PNG 魔数 + IHDR 宽高（大端）。
        assert!(png.len() > 5000, "图太小，多半没画上：{}", png.len());
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert_eq!((w, h), (1080, 1920), "分辨率不对：{w}x{h}");

        // 落盘给用户核对（HOME 取不到就跳过写文件，不因此失败）。
        if let Ok(home) = std::env::var("HOME") {
            let dest = std::path::PathBuf::from(home)
                .join("Desktop")
                .join("笺读-分享卡-1080x1920.png");
            if std::fs::write(&dest, &png).is_ok() {
                println!("已出图：{}", dest.display());
            }
        }
    }
}
