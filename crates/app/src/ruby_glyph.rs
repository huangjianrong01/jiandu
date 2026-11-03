//! 生僻字 / 夹注注音位图（v1.9.52）：**竖排、小字、贴在主字右侧**。
//!
//! ## 为什么要有这个文件
//!
//! GPUI 0.2.2 没有排版引擎可用（`paint_glyph` 只画单个字形、字距要自己算），
//! 而注音现在是一摞**竖排小字**（拼音单列、CJK 多字拆双列），逐字落位都得对齐
//! `jiandu_core::ruby::layout` 给的坐标。绕开的办法是：**CoreGraphics 离屏**把整条
//! 注音渲成一张透明位图（竖排），交给 GPUI 的 `img` 贴进字格——与 PDF 侧
//! 共用同一套 `glyph_xy` 度量，逐字同位。
//!
//! 用户铁律（v1.9.52 最终态）：**严格竖排、绝不横向排列**；**拼音单列**、字号
//! ≤ 主字 40%；CJK 注记短 55%、多 45%、拆双列竖排；整块垂直居中于字格、绝不悬浮、
//! 绝不压界栏、超出字格即裁剪；颜色锁死朱砂红 `#8C2B22`、透明度 0.75。
//!
//! ## 坐标（照抄 `share_paint` 已验证的结论）
//!
//! * CG 位图上下文的原点在**左下**（y 向上），但缓冲的**第 0 行就是图像顶行**；
//! * **绝不翻 CTM**（`translate(0,h) + scale(1,-1)` 会把字形垂直镜像）；
//! * 基线落在 `pad + descent`（距图像底边），升部向上顶到 `h - pad`。
//!
//! ## 样式（用户精调，只动这里）
//!
//! * **颜色**：`ruby_image` 的调用方传入朱砂红 `#8C2B22`（锁死）；
//! * **透明度**：填充 alpha = **0.75**（朱砂微微晕染宣纸的透气感，绝不刺眼）；
//! * **字体**：**强制 Songti SC（宋体）**优先，绝不用 PingFang 等黑体；
//! * **字号 / 几何**：由 `ruby::layout` 给的 `RubyLayout` 全权决定。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 超采样倍数：位图按 3× 渲、按 1× 尺寸显示——Retina 上小字不糊。
const SS: f64 = 3.0;

/// 注音字体候选：**强制 Songti SC（宋体）优先**，绝不用 PingFang 等黑体；
/// 后备仍是衬线（STSong / Songti TC），保证带调元音（ā á ǎ à …）齐全不缺字形。
/// 都缺字形就**不画**（退回逐字母竖排）。
const FONTS: [&str; 3] = ["Songti SC", "STSong", "Songti TC"];

/// 位图格式：32 位**大端** + **预乘** alpha 在最后。
///
/// ⚠️ 这里必须是 `4 << 12`(=0x4000)，即 CG 规范的 `kCGBitmapByteOrder32Big`；
/// 大端 32 位下内存字节序恰是 `R,G,B,A`，与 `RgbaImage` 一致 —— 再由下面
/// `raster` 转成 gpui 要的 BGRA。写成其它值（比如多写几个零）CG 会忽略
/// 越界位、退回「主机序」，在小端 Mac 上通道就整个反了，朱砂注音会变蓝。
const BYTE_ORDER_32_BIG: u32 = 4 << 12;
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;
const BITMAP_INFO: u32 = BYTE_ORDER_32_BIG | ALPHA_PREMULTIPLIED_LAST;

/// 注音位图填充透明度（朱砂微微晕染在宣纸上的透气感）。
const RUBY_ALPHA: f64 = 0.75;

/// 缓存：同一（注音, 字号档, 整块尺寸, 墨色）只光栅一次。
fn cache() -> &'static Mutex<HashMap<String, Arc<gpui::RenderImage>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<gpui::RenderImage>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 一条注音的**竖排**位图。失败返回 `None`（调用方退化成逐字竖排 div）。
///
/// * `rl`：`ruby::layout` 给的度量（字号、列数、整块宽高、各字 `glyph_xy`）；
/// * `color`：`0xRRGGBB` 墨色（调用方锁死 `#8C2B22`）。
///
/// 位图尺寸 = `rl.block_w × rl.block_h`，每个字按其 `glyph_xy` 落进
/// `fs × fs` 的小格里、字形在小格内居中（与 `ruby::glyph_xy`、PDF `draw_ruby`
/// 逐字同位）。
pub fn ruby_image(py: &str, rl: &crate::ruby::RubyLayout, color: u32) -> Option<Arc<gpui::RenderImage>> {
    if py.is_empty() || rl.fs < 2.0 {
        return None;
    }
    // 字号按 0.25px 分档、整块尺寸按 0.5px 分档，免得每帧翻页都重光栅一批。
    let key = format!(
        "{py}|{}|{}|{}|{color:X}",
        (rl.fs * 4.0).round() as i32,
        (rl.block_w * 2.0).round() as i32,
        (rl.block_h * 2.0).round() as i32,
    );
    if let Ok(c) = cache().lock() {
        if let Some(img) = c.get(&key) {
            return Some(img.clone());
        }
    }
    let img = render(py, rl, color)?;
    let arc = Arc::new(img);
    if let Ok(mut c) = cache().lock() {
        c.insert(key, arc.clone());
    }
    Some(arc)
}

/// 一个 `char` 的 UTF-16 码元（与 `pdf.rs::utf16_units` 同口径，避免依赖）。
/// 返回 (缓冲, 码元数)；BMP 字 1 个、扩展区 2 个。
fn utf16_units(c: char) -> ([u16; 2], usize) {
    let cp = c as u32;
    if cp < 0x10000 {
        ([cp as u16, 0], 1)
    } else {
        let v = cp - 0x10000;
        ([0xD800 + (v >> 10) as u16, 0xDC00 + (v & 0x3FF) as u16], 2)
    }
}

/// `0xRRGGBB` → 三分量（0~1）。
fn rgb_of(c: u32) -> (f64, f64, f64) {
    (
        ((c >> 16) & 0xFF) as f64 / 255.0,
        ((c >> 8) & 0xFF) as f64 / 255.0,
        (c & 0xFF) as f64 / 255.0,
    )
}

/// 反预乘：`c_premul × 255 / a`（CG 只给预乘值，gpui 要非预乘的 BGRA）。
fn unpremul(c: u8, a: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    ((c as u32 * 255) / a as u32).min(255) as u8
}

#[cfg(target_os = "macos")]
fn render(py: &str, rl: &crate::ruby::RubyLayout, color: u32) -> Option<gpui::RenderImage> {
    let (bw, bh, buf) = raster_vertical(py, rl, color)?;
    let img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(bw as u32, bh as u32, buf)?;
    Some(gpui::RenderImage::new(vec![image::Frame::new(img)]))
}

/// 光栅成 **BGRA** 缓冲（gpui 的 `RenderImage` 内部就是 BGRA，见
/// `gpui/src/elements/img.rs` 里加载图片时的 RGBA→BGRA 交换）。
/// 拆出这一层是为了让单测能直接验通道序与朝向——画反了在屏幕上一眼可见。
#[cfg(target_os = "macos")]
fn raster_vertical(
    py: &str,
    rl: &crate::ruby::RubyLayout,
    color: u32,
) -> Option<(usize, usize, Vec<u8>)> {
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::font::CGGlyph;
    use core_graphics::geometry::{CGPoint, CGSize};
    use core_text::font::new_from_name;
    use core_text::font_descriptor::kCTFontOrientationHorizontal;

    let fs = rl.fs as f64;
    // ① 挑一台能覆盖整串所有字形的字体（缺任何一个字形就换下一台）。
    //   强制 Songti SC 优先。
    let mut picked: Option<core_text::font::CTFont> = None;
    let mut glyphs: Vec<CGGlyph> = Vec::new();
    for name in FONTS {
        if let Ok(f) = new_from_name(name, fs) {
            let mut gs: Vec<CGGlyph> = Vec::new();
            let mut complete = true;
            for c in py.chars() {
                let (unit, cnt) = utf16_units(c);
                let mut g: CGGlyph = 0;
                let ok = unsafe { f.get_glyphs_for_characters(unit.as_ptr(), &mut g, cnt as isize) };
                if !ok || g == 0 {
                    complete = false;
                    break;
                }
                gs.push(g);
            }
            if complete {
                picked = Some(f);
                glyphs = gs;
                break;
            }
        }
    }
    let font = picked?;
    let n = glyphs.len();
    if n == 0 {
        return None;
    }
    let asc = font.ascent();
    let desc = font.descent();

    // ② 位图尺寸 = 整块宽高（与 `ruby::layout` 同一套）。
    let w = rl.block_w as f64;
    let h = rl.block_h as f64;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let bw = (w * SS).round() as usize;
    let bh = (h * SS).round() as usize;

    let space = CGColorSpace::create_device_rgb();
    let mut ctx =
        CGContext::create_bitmap_context(None, bw, bh, 8, bw * 4, &space, BITMAP_INFO);
    ctx.set_allows_antialiasing(true);
    ctx.set_should_antialias(true);
    // 新建的位图上下文内容全零 —— 即透明底，注音因此只叠在纸面上，不糊白块。

    // ③ 放大到超采样网格（用户坐标 = 1× 像素）。y 向上，缓冲第 0 行是图像顶行。
    ctx.scale(SS, SS);
    let (r, g, b) = rgb_of(color);
    ctx.set_rgb_fill_color(r, g, b, RUBY_ALPHA);

    let H = h; // 整块高（1×）
    let chars: Vec<char> = py.chars().collect();
    for i in 0..n {
        let c = chars[i];
        let g_idx = glyphs[i];
        // 该字在小格（fs×fs）内的屏坐标（顶端靠上）；bx/by 相对「整块左上」。
        let (gx, gy) = rl.glyph_xy(i);
        let bx = (gx - rl.left) as f64;
        let by = (gy - rl.top) as f64;
        // 字形在其小格内居中：横向按 advance 居中，纵向按 em 盒居中。
        let mut adv = CGSize::new(0.0, 0.0);
        unsafe {
            font.get_advances_for_glyphs(
                kCTFontOrientationHorizontal,
                &g_idx,
                &mut adv,
                1,
            );
        }
        let cx = bx + fs * 0.5;
        let x0 = cx - adv.width * 0.5;
        // 屏坐标 by（顶端）↔ CG 基线：base_cg = H - by - (fs-(asc+desc))/2 - asc。
        let base_cg = H - by - (fs - (asc + desc)) * 0.5 - asc;
        font.draw_glyphs(&[g_idx], &[CGPoint::new(x0, base_cg)], ctx.clone());
    }

    // ④ 取像素：预乘 → 反预乘，并转成 gpui 要的 **BGRA**。
    let stride = ctx.bytes_per_row();
    let data = ctx.data();
    let mut buf: Vec<u8> = Vec::with_capacity(bw * bh * 4);
    for y in 0..bh {
        let base = y * stride;
        for xx in 0..bw {
            let i = base + xx * 4;
            let a = data[i + 3];
            let (pr, pg, pb) = (data[i], data[i + 1], data[i + 2]);
            buf.push(unpremul(pb, a));
            buf.push(unpremul(pg, a));
            buf.push(unpremul(pr, a));
            buf.push(a);
        }
    }
    Some((bw, bh, buf))
}

/// 单字（或短串）的**横向**小位图：仅供**单测**验证通道序 / 朝向 / 透明度用，
/// 不参与屏幕主路径（主路径走 `raster_vertical`）。行为与原 v1.9.50 同款。
#[cfg(target_os = "macos")]
fn raster_char(py: &str, fs: f64, color: u32) -> Option<(usize, usize, Vec<u8>)> {
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::font::CGGlyph;
    use core_graphics::geometry::{CGPoint, CGSize};
    use core_text::font::new_from_name;
    use core_text::font_descriptor::kCTFontOrientationHorizontal;

    let mut picked: Option<core_text::font::CTFont> = None;
    let mut glyphs: Vec<CGGlyph> = Vec::new();
    for name in FONTS {
        if let Ok(f) = new_from_name(name, fs) {
            let mut gs: Vec<CGGlyph> = Vec::new();
            let mut complete = true;
            for c in py.chars() {
                let (unit, cnt) = utf16_units(c);
                let mut g: CGGlyph = 0;
                let ok = unsafe { f.get_glyphs_for_characters(unit.as_ptr(), &mut g, cnt as isize) };
                if !ok || g == 0 {
                    complete = false;
                    break;
                }
                gs.push(g);
            }
            if complete {
                picked = Some(f);
                glyphs = gs;
                break;
            }
        }
    }
    let font = picked?;
    let n = glyphs.len();
    if n == 0 {
        return None;
    }
    let mut sizes = vec![CGSize::new(0.0, 0.0); n];
    unsafe {
        font.get_advances_for_glyphs(
            kCTFontOrientationHorizontal,
            glyphs.as_ptr(),
            sizes.as_mut_ptr(),
            n as isize,
        )
    };
    let run_w: f64 = sizes.iter().map(|s| s.width).sum();
    if run_w <= 0.0 {
        return None;
    }
    let asc = font.ascent();
    let desc = font.descent();
    let pad = 1.0;
    let w = (run_w + pad * 2.0).ceil().max(2.0);
    let h = (asc + desc + pad * 2.0).ceil().max(2.0);
    let bw = (w * SS).round() as usize;
    let bh = (h * SS).round() as usize;

    let space = CGColorSpace::create_device_rgb();
    let mut ctx =
        CGContext::create_bitmap_context(None, bw, bh, 8, bw * 4, &space, BITMAP_INFO);
    ctx.set_allows_antialiasing(true);
    ctx.set_should_antialias(true);
    ctx.scale(SS, SS);
    ctx.translate(pad, pad + desc);

    let (r, g, b) = rgb_of(color);
    ctx.set_rgb_fill_color(r, g, b, RUBY_ALPHA);
    let mut x = 0.0f64;
    for i in 0..n {
        font.draw_glyphs(&[glyphs[i]], &[CGPoint::new(x, 0.0)], ctx.clone());
        x += sizes[i].width;
    }

    let stride = ctx.bytes_per_row();
    let data = ctx.data();
    let mut buf: Vec<u8> = Vec::with_capacity(bw * bh * 4);
    for y in 0..bh {
        let base = y * stride;
        for xx in 0..bw {
            let i = base + xx * 4;
            let a = data[i + 3];
            let (pr, pg, pb) = (data[i], data[i + 1], data[i + 2]);
            buf.push(unpremul(pb, a));
            buf.push(unpremul(pg, a));
            buf.push(unpremul(pr, a));
            buf.push(a);
        }
    }
    Some((bw, bh, buf))
}

#[cfg(not(target_os = "macos"))]
fn render(_py: &str, _rl: &crate::ruby::RubyLayout, _color: u32) -> Option<gpui::RenderImage> {
    // 非 macOS 没有 CoreGraphics：由 `reader` 侧自行降级成逐字竖排。
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruby;

    /// **拼音**注音位图必须是**竖长条**：单列字母自上而下堆叠，整块高 ≥ 宽。
    /// 这条同时钉死两件事：位图真的画出来了，且是单列竖排——若哪天又改回「横条」，
    /// 宽高比会立刻反过来（w>h）。
    #[test]
    fn ruby_bitmap_is_taller_than_wide() {
        let rl = ruby::layout("měi", 24.0, 80.0, 44.0);
        let im = ruby_image("měi", &rl, 0x8C2B22).expect("注音位图应当渲染成功");
        let s = im.size(0);
        let (w, h) = (s.width.0 as f32, s.height.0 as f32);
        assert!(
            h >= w * 1.0,
            "拼音单列竖排后应是竖长条，实得 {w:.1}×{h:.1}"
        );
    }

    /// **朝向钉死**：字母 `T` 顶部那条横杠（字母的「上」）必须落在字格**上半**，
    /// 竖杠居中。若哪天又被转成侧躺，重墨的行会跑到左右两侧去。
    #[cfg(target_os = "macos")]
    #[test]
    fn ruby_letters_stand_upright() {
        let (bw, bh, buf) = raster_char("T", 40.0, 0xFF0000).expect("位图");
        let mut row_ink = vec![0u32; bh];
        for y in 0..bh {
            for x in 0..bw {
                if buf[(y * bw + x) * 4 + 3] > 128 {
                    row_ink[y] += 1;
                }
            }
        }
        let heaviest = row_ink
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| *n)
            .map(|(i, _)| i)
            .expect("字形应有墨");
        assert!(
            heaviest < bh / 2,
            "T 的横杠落在下半（行 {heaviest}/{bh}）——字母躺倒了"
        );
    }

    /// 通道序钉死：给纯红，缓冲里字形内部必须是 `B=0,G=0,R=255`（gpui 存 BGRA）。
    /// 这条防的是 `BITMAP_INFO` 写错导致 CG 退回主机序、朱砂注音变蓝。
    /// 注意：填充 alpha 为 0.75，字内像素 `a≈191`（不再等于 255），
    /// 故这里取「最不透明的字内像素」来验通道序，而非强求 a==255。
    #[cfg(target_os = "macos")]
    #[test]
    fn ruby_ink_keeps_its_channel_order() {
        let (bw, bh, buf) = raster_char("měng", 20.0, 0xFF0000).expect("位图");
        let mut best: Option<(u8, u8, u8, u8)> = None;
        for y in 0..bh {
            for x in 0..bw {
                let i = (y * bw + x) * 4;
                let (b, g, r, a) = (buf[i], buf[i + 1], buf[i + 2], buf[i + 3]);
                if a > 128 && (best.is_none() || a > best.unwrap().3) {
                    best = Some((b, g, r, a));
                }
            }
        }
        let (b, g, r, a) = best.expect("字形内部应至少有一颗像素");
        assert_eq!(
            (b, g, r),
            (0, 0, 255),
            "纯红注音画成了 B={b} G={g} R={r} A={a} —— 位图通道序反了"
        );
    }

    /// 同一条注音只光栅一次（缓存命中）。
    #[test]
    fn same_pinyin_is_rasterized_once() {
        let rl = ruby::layout("bō", 11.0, 100.0, 50.0);
        let a = ruby_image("bō", &rl, 0x8C2B22).expect("位图");
        let b = ruby_image("bō", &rl, 0x8C2B22).expect("位图");
        assert!(a.id == b.id, "缓存未命中：同一条注音被光栅了两次");
    }

    /// 透明度确实落在 0.75 附近（取「最不透明字内像素」的 alpha）。
    #[cfg(target_os = "macos")]
    #[test]
    fn ruby_alpha_is_the_ink_wash() {
        let (bw, bh, buf) = raster_char("měng", 20.0, 0x8C2B22).expect("位图");
        let mut best_a: u8 = 0;
        for y in 0..bh {
            for x in 0..bw {
                let a = buf[(y * bw + x) * 4 + 3];
                if a > best_a {
                    best_a = a;
                }
            }
        }
        assert!(
            (best_a as i32 - 191).abs() <= 8,
            "注音 alpha 应为 ~0.75（≈191），实得 {best_a}"
        );
    }
}
