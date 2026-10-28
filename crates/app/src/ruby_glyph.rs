//! 生僻字注音的**真·旋转**位图（v1.85.0）。
//!
//! ## 为什么要有这个文件
//!
//! 竖排注音的正确形态是「整串拼音**顺时针旋转 90°** 贴在汉字右侧」。但
//! GPUI 0.2.2 的 `Window::paint_glyph` 把 `TransformationMatrix::unit()`
//! **写死**在场景图元里（见 `gpui-0.2.2/src/window.rs`），屏幕侧根本没有
//! 旋转字形的入口——v0.5.0 那版 `west::rotated_slots` 就是把 `angle_deg`
//! 算好了却无处施用，最后退化成「直立并排」（提交说明里写得很清楚）。
//!
//! 这里绕开它：**CoreGraphics 离屏**把整串按 90° 转好、渲成一张透明位图，
//! 再交给 GPUI 的 `img` 元素贴到字格右侧。位图内容是真的旋转过的字形，
//! 不是「字母竖着摞」。
//!
//! ## 坐标（照抄 `share_paint` 已验证的结论）
//!
//! * 位图缓冲的**第 0 行就是图像顶行**，故「屏幕 y」与「缓冲行号」同序；
//! * **绝不翻 CTM**（`translate(0,h) + scale(1,-1)` 会把字形垂直镜像）；
//!   这里只做 `scale → translate → rotate(-90°)`，字形保持正向。
//!
//! 旋转后：局部 `+x`（读序）→ 全局**向下**；局部 `+y`（字母的上）→ 全局**向右**。
//! 正是竖排注音该有的「从上往下读、字头朝右」。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 超采样倍数：位图按 3× 渲、按 1× 尺寸显示——Retina 上小字不糊。
const SS: f64 = 3.0;

/// 注音字体候选：拉丁字母 + 声调字符（ā á ǎ à ē ě ū ǚ …）必须齐全。
/// PingFang 全覆盖；Helvetica 老系统兜底；都缺字形就**不画**（退回竖叠）。
const FONTS: [&str; 3] = ["PingFangSC", "Helvetica", "Menlo"];

/// 位图格式：32 位**大端** + **预乘** alpha 在最后。
///
/// ⚠️ 这里必须是 `4 << 12`(=0x4000)，即 CG 规范的 `kCGBitmapByteOrder32Big`；
/// 大端 32 位下内存字节序恰是 `R,G,B,A`，与 `RgbaImage` 一致 —— 再由下面
/// `raster` 转成 gpui 要的 BGRA。写成其它值（比如多写几个零）CG 会忽略
/// 越界位、退回「主机序」，在小端 Mac 上通道就整个反了，朱砂注音会变蓝。
const BYTE_ORDER_32_BIG: u32 = 4 << 12;
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;
const BITMAP_INFO: u32 = BYTE_ORDER_32_BIG | ALPHA_PREMULTIPLIED_LAST;

/// 缓存：同一（拼音, 字号档, 墨色）只光栅一次。
fn cache() -> &'static Mutex<HashMap<String, Arc<gpui::RenderImage>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<gpui::RenderImage>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 一条拼音**顺时针旋转 90°** 后的位图。失败返回 `None`（调用方退化）。
///
/// * `fs`：拼音字号（由 `ruby::layout` 给出）；
/// * `color`：`0xRRGGBB` 墨色。
pub fn ruby_image(py: &str, fs: f32, color: u32) -> Option<Arc<gpui::RenderImage>> {
    if py.is_empty() || fs < 2.0 {
        return None;
    }
    // 字号按 0.25px 分档，免得每帧翻页都重光栅一批。
    let key = format!("{py}|{}|{color:X}", (fs * 4.0).round() as i32);
    if let Ok(c) = cache().lock() {
        if let Some(img) = c.get(&key) {
            return Some(img.clone());
        }
    }
    let img = render(py, fs as f64, color)?;
    let arc = Arc::new(img);
    if let Ok(mut c) = cache().lock() {
        c.insert(key, arc.clone());
    }
    Some(arc)
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
fn render(py: &str, fs: f64, color: u32) -> Option<gpui::RenderImage> {
    let (bw, bh, buf) = raster(py, fs, color)?;
    let img = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(bw as u32, bh as u32, buf)?;
    Some(gpui::RenderImage::new(vec![image::Frame::new(img)]))
}

/// 光栅成 **BGRA** 缓冲（gpui 的 `RenderImage` 内部就是 BGRA，见
/// `gpui/src/elements/img.rs` 里加载图片时的 RGBA→BGRA 交换）。
/// 拆出这一层是为了让单测能直接验通道序——颜色画反了在屏幕上一眼可见。
#[cfg(target_os = "macos")]
fn raster(py: &str, fs: f64, color: u32) -> Option<(usize, usize, Vec<u8>)> {
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::font::CGGlyph;
    use core_graphics::geometry::{CGPoint, CGSize};
    use core_text::font::new_from_name;
    use core_text::font_descriptor::kCTFontOrientationHorizontal;

    // ① 挑一台能覆盖整串的字体（缺任何一个字形就换下一台）。
    let mut picked: Option<core_text::font::CTFont> = None;
    let mut glyphs: Vec<CGGlyph> = Vec::new();
    for name in FONTS {
        if let Ok(f) = new_from_name(name, fs) {
            let mut gs: Vec<CGGlyph> = Vec::new();
            let mut complete = true;
            for c in py.chars() {
                let mut unit: [u16; 1] = [0];
                c.encode_utf16(&mut unit);
                let mut g: CGGlyph = 0;
                let ok = unsafe { f.get_glyphs_for_characters(unit.as_ptr(), &mut g, 1) };
                if !ok {
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

    // ② 逐个步进 → 整串连写的自然宽度（这就是旋转后的**纵向**长度）。
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

    // ③ 位图尺寸：横 = 字母的高（降部 + 升部），纵 = 串长。
    let asc = font.ascent();
    let desc = font.descent();
    let pad = 1.0;
    let w = (desc + asc + pad * 2.0).ceil().max(2.0);
    let h = (run_w + pad * 2.0).ceil().max(2.0);
    let bw = (w * SS).round() as usize;
    let bh = (h * SS).round() as usize;

    let space = CGColorSpace::create_device_rgb();
    let mut ctx =
        CGContext::create_bitmap_context(None, bw, bh, 8, bw * 4, &space, BITMAP_INFO);
    ctx.set_allows_antialiasing(true);
    ctx.set_should_antialias(true);
    // 新建的位图上下文内容全零 —— 即透明底，注音因此只叠在纸面上，不糊白块。

    // ④ 顺时针 90°：放大到超采样网格 → 原点挪到「顶行、基线」→ 转 −90°。
    ctx.scale(SS, SS);
    ctx.translate(pad + desc, h - pad);
    ctx.rotate(-std::f64::consts::FRAC_PI_2);

    let (r, g, b) = rgb_of(color);
    ctx.set_rgb_fill_color(r, g, b, 1.0);
    let mut x = 0.0f64;
    for i in 0..n {
        // `draw_glyphs` 按值取 context：`clone()` 是 CGContextRetain，随调用
        // 结束 drop（release），一增一减不泄漏（与 `share_paint` 同款）。
        font.draw_glyphs(&[glyphs[i]], &[CGPoint::new(x, 0.0)], ctx.clone());
        x += sizes[i].width;
    }

    // ⑤ 取像素：预乘 → 反预乘，并转成 gpui 要的 **BGRA**。
    let stride = ctx.bytes_per_row();
    let data = ctx.data();
    let mut buf: Vec<u8> = Vec::with_capacity(bw * bh * 4);
    for y in 0..bh {
        let base = y * stride;
        for x in 0..bw {
            let i = base + x * 4;
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
fn render(_py: &str, _fs: f64, _color: u32) -> Option<gpui::RenderImage> {
    // 非 macOS 没有 CoreGraphics：退回竖叠（由 `reader` 侧自行降级）。
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旋转后的注音必须是**竖长条**：纵向（串长）明显长于横向（字母高）。
    /// 这一条同时钉死两件事：位图真的画出来了，且方向是「整串转 90°」
    /// 而不是横排（横排的话宽会反过来大于高）。
    #[test]
    fn rotated_ruby_bitmap_is_tall_and_narrow() {
        let im = ruby_image("měng", 12.0, 0x8C2B22).expect("注音位图应当渲染成功");
        let s = im.size(0);
        let (w, h) = (s.width.0 as f32, s.height.0 as f32);
        assert!(
            h > w * 1.6,
            "四个字母转 90° 后应是竖长条，实得 {w:.1}×{h:.1}"
        );
    }

    /// 通道序钉死：给纯红，缓冲里必须是 `B=0,G=0,R=255`（gpui 存 BGRA）。
    /// 这条防的是 `BITMAP_INFO` 写错导致 CG 退回主机序、朱砂注音变蓝。
    #[cfg(target_os = "macos")]
    #[test]
    fn ruby_ink_keeps_its_channel_order() {
        let (bw, bh, buf) = raster("měng", 20.0, 0xFF0000).expect("位图");
        // 找一颗完全不透明的像素（字形内部），抗锯齿边缘不算。
        let mut hit: Option<(u8, u8, u8, u8)> = None;
        for y in 0..bh {
            for x in 0..bw {
                let i = (y * bw + x) * 4;
                let (b, g, r, a) = (buf[i], buf[i + 1], buf[i + 2], buf[i + 3]);
                if a == 255 {
                    hit = Some((b, g, r, a));
                    break;
                }
            }
            if hit.is_some() {
                break;
            }
        }
        let (b, g, r, a) = hit.expect("字形内部应至少有一颗不透明像素");
        assert_eq!(
            (b, g, r, a),
            (0, 0, 255, 255),
            "纯红注音画成了 B={b} G={g} R={r} —— 位图通道序反了"
        );
    }

    /// 同一条拼音只光栅一次（缓存命中）。
    #[test]
    fn same_pinyin_is_rasterized_once() {
        let a = ruby_image("bō", 11.0, 0x8C2B22).expect("位图");
        let b = ruby_image("bō", 11.0, 0x8C2B22).expect("位图");
        assert!(a.id == b.id, "缓存未命中：同一条拼音被光栅了两次");
    }
}
