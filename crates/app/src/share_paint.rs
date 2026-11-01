//! 分享卡的**离屏光栅器**（v1.57.0）：CoreGraphics 位图上下文 + CoreText 逐字落笔。
//!
//! 为什么不截屏：旧制 `screencapture` 依赖屏幕录制权限、分辨率随窗口走、
//! 锁屏时直接失败。这里是**真离屏**——自己开一块 `width × height` 的位图，
//! 按 `share_layout::CardLayout` 的指令画完，再编码成 PNG。1080×1920 与
//! 屏幕、窗口、权限全部无关。
//!
//! 两处必防雷（用户点名）：
//! 1. **坐标系换算（v1.59.0 重写，勿退回旧法）**：`CGBitmapContextCreate`
//!    出来的是 CG 坐标（原点**左下**、y 向上），而版式数据是屏幕坐标（原点
//!    左上、y 向下）。
//!    ⚠️ **绝不能用 `translate(0,h) + scale(1,-1)` 翻 CTM** —— 那是 v1.58
//!    的 bug：CTM 一翻，**字形也跟着被垂直镜像**，矩形对称看不出事，每个字
//!    却被上下颠倒（用户实测「整个图是镜像反的、字是躺着的」）。
//!    正解是**不动 CTM**，只把每个元素的 y 做一次换算（见 [`cg_y`]）：
//!    矩形取「顶边 → CG 底边」，文字取「基线 → CG 基线」。字形因此恒为正向。
//! 2. **CFRelease / 泄漏**：用到的 CoreFoundation 对象（`CGContext` /
//!    `CGColorSpace` / `CTFont`）都是 Rust 侧 `TCFType` 包装，`Drop` 自动
//!    `CFRelease`——不手动 retain、不用 `into_raw`、不 `mem::forget`。
//!    字体按字号缓存复用，几百字的卡片只建两三台字体对象。

use crate::share::CINNABAR;
use crate::share_layout::{
    Binding, CardLayout, PlacedChar, PlacedSeal, ShareFont, punct_box, spread_slots,
};

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

/// **基线沉量**：字格顶到基线要落下多少——首行顶格的唯一依据。
///
/// 就取 `font.ascent()`，理由是实测出来的（宋体 46.5pt）：
///
/// ```text
///   ascent = 49.3 (1.06 em)     ← 已含拉丁字母的升部余量
///   「國」的墨顶 = 33.6 (0.72 em)  ← 汉字字身的真实墨顶
///   字格 = 1.55 em
/// ```
///
/// 于是 `baseline = 格顶 + ascent` 让汉字墨迹在格里**上留 0.34em、下留
/// 0.32em**——正正好居中。这正是「字格顶贴住边栏、字在格里坐正」的口径。
///
/// ⚠️ 两个诱人但错误的替代（都实测过，别走回头路）：
///   1. 用字体级 `bounding_box()`（origin −36.6 / 高 80.1 = 1.72em）：它被
///      拉丁变音符撑高，会把整列字往下推 18px，「贴框」变「悬空」；
///   2. 用「國」单字 bbox 的墨顶（0.72em）：字顶死贴框线，但每格下方空出
///      0.66em，行气被拉散——首字顶格了，整列却散了。
///
/// `ascent` 拿不到（异常字体）就按 0.88 的字号比例兜底，绝不因此崩整张卡。
fn baseline_drop(fs: f64, f: &core_text::font::CTFont) -> f64 {
    let a = f.ascent();
    if a > 0.0 {
        a
    } else {
        fs * 0.88
    }
}

/// 一枚字的落笔点（字形原点 = 基线）：**字形顶边贴住字格顶 `y`**。
///
/// CoreText 的基线语义：字形最高点 = `baseline − ascent`。要让最高点正好压在
/// 字格顶 `y` 上，基线就得**往下沉一个 ascent**：
///
/// ```text
/// baseline = y + ascent        // ← 屏幕坐标（左上原点，y 向下）
/// ```
///
/// 直接把 `y` 当基线传进去是错的（那会把整字画到格顶**之上**）。列首字的
/// `y` 就是版式层的 `y0`（＝ `share_layout::TOP_BASELINE`），故每列第一个字的
/// **最高点**统统钉在同一条线上——这才是「首行顶格」。
fn glyph_origin(c: &PlacedChar, asc: f64) -> (f64, f64) {
    let (gx, gy, gfs) = if c.punct {
        punct_box(c)
    } else {
        (c.x, c.y, c.fs)
    };
    let fs = gfs as f64;
    if c.punct {
        // 句读：小盒内同样按「顶边贴盒顶」落，基线沉一个 ascent 的比例量。
        (gx as f64, gy as f64 + fs * 0.86)
    } else {
        // 横向：字形在字格里居中（左右均分）；纵向：顶边贴格顶（`+ asc`）。
        let x = gx as f64 + c.cell as f64 * 0.5 - fs * 0.5;
        let baseline = gy as f64 + asc;
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
    draw_card(l, &ctx, h as f64, 0.0, 0.0)?;
    encode_png(&mut ctx, w, h)
}

/// 拼版**中缝线**的线宽（pt，随纸宽等比缩放）。
///
/// 用户 2026-11-01 真机反馈：「留了 30pt 间距，但视觉上完全不像古籍折页」。
/// 光靠空白，读者只会觉得「两页靠得比较近」；画一条贯穿版框高的竖线，才
/// 一眼看得出这是**一张纸对折、左边一页右边一页**。
pub const SPREAD_FOLD_W: f32 = 1.4;

/// 中缝线的不透明度（取版框墨色的这一档，比版框淡一档，不抢版面）。
const SPREAD_FOLD_ALPHA: f32 = 0.70;

// ---- 页级裁剪（v1.9.21）----------------------------------------------------
//
// 拼版是「两页并一张纸」：两页共用一块画布 / 一个 PDF 页。排版本该严格自律
// （每页的字只落在自己那半张纸上），可一旦哪个算式兜底失效，越界的字**不会
// 被任何东西挡住**——它会直接画到隔壁那页上，中文古籍里就是「左页最右一列
// 压在中缝鱼尾上」那种事故（用户 2026-11-01 真机截图）。
//
// 排版侧已经收紧（`grid::compute_grid` 第 9 条：一叶绝不许宽过半张纸），这里
// 是**第二道保险**：落每一页之前先 clip 到它的整页矩形，谁越界谁被裁掉。
// `core-graphics` 0.24 没包 `CGContextSave/RestoreGState`，就地声明。
extern "C" {
    fn CGContextSaveGState(ctx: core_graphics::sys::CGContextRef);
    fn CGContextRestoreGState(ctx: core_graphics::sys::CGContextRef);
}

/// 在**一页自己的矩形**里落笔：进出各一次 gstate，越界墨迹一律裁掉。
fn with_page_clip(
    ctx: &CGContext,
    rect: CGRect,
    draw: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    use foreign_types::ForeignType;
    unsafe { CGContextSaveGState(ctx.as_ptr()) };
    ctx.clip_to_rect(rect);
    let r = draw();
    unsafe { CGContextRestoreGState(ctx.as_ptr()) };
    r
}

/// 版框在**屏幕坐标**（左上原点）里的纵向范围，以及它的墨色。
///
/// 中缝线只画到版框的上下边，绝不贯穿整张纸——贯穿就成了裁切线，不像书。
fn frame_span(l: &CardLayout) -> (f32, f32, u32) {
    let f = l
        .rects
        .iter()
        .filter(|r| r.stroke.is_some())
        .max_by(|a, b| {
            (a.w * a.h)
                .partial_cmp(&(b.w * b.h))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    match f {
        Some(r) => (r.y, r.y + r.h, r.color),
        None => (0.0, l.h, 0x2b2620FF),
    }
}

/// 在一张拼版纸的**正中**画那条中缝线（PDF 与位图共用）。
///
/// `hf` 是画布高（CG 原点在左下），故 `cg = hf − y`：与 [`draw_card`] 同一套
/// 换算，绝不翻 CTM。
fn draw_fold(ctx: &CGContext, hf: f64, sheet_w: f32, page: &CardLayout) {
    let (top, bottom, color) = frame_span(page);
    let (r, g, b, a) = rgba(color);
    // 线宽随纸宽缩放：预览位图（1280px）与 PDF（1191pt）看着一样粗细。
    let scale = (sheet_w / 1191.0).max(0.5);
    ctx.set_rgb_stroke_color(r, g, b, (a as f32 * SPREAD_FOLD_ALPHA) as f64);
    ctx.set_line_width((SPREAD_FOLD_W * scale) as f64);
    let x = (sheet_w * 0.5) as f64;
    // core-graphics 0.24 没包 `CGContextStrokeLine`，用等价的多段线接口。
    ctx.stroke_line_segments(&[
        CGPoint::new(x, hf - bottom as f64),
        CGPoint::new(x, hf - top as f64),
    ]);
}

/// **双页拼版的一张纸**：把两个单页并排画进一块 `sheet` 大小的位图（预览用）。
///
/// 与 PDF 拼版共用 [`spread_slots`] 与同一份 `CardLayout`，所以屏幕上看的就是
/// 印出来的：右半前一页、左半后一页，中间一条中缝。
///
/// 只有一页时（末张单页）另一侧留整片纸色——书末页背面本来就是空的。
pub fn render_spread_png(
    pages: &[CardLayout],
    sheet_w: f32,
    sheet_h: f32,
    binding: Binding,
) -> Result<Vec<u8>, String> {
    let first = pages.first().ok_or_else(|| "没有可拼版的页面".to_string())?;
    let w = sheet_w.round().max(1.0) as usize;
    let h = sheet_h.round().max(1.0) as usize;
    let space = CGColorSpace::create_device_rgb();
    let mut ctx = CGContext::create_bitmap_context(None, w, h, 8, w * 4, &space, BITMAP_INFO);

    // 整张纸先铺底色（中缝 / 空白半页都靠它，绝不透出黑色）。
    let (br, bg, bb) = rgb(first.bg);
    ctx.set_rgb_fill_color(br, bg, bb, 1.0);
    ctx.fill_rect(CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(w as f64, h as f64),
    ));

    // 两页按装订模式落到各自的半边（右开本奇左偶右，左开本反之）。
    // 每页在自己的矩形里落笔并裁剪 —— 一页的字绝不许画到另一页上（见
    // `with_page_clip` 的说明）。
    let slots = spread_slots(sheet_w, first.w, binding);
    for (i, p) in pages.iter().take(2).enumerate() {
        let ox = slots[i] as f64;
        let box_ = CGRect::new(
            &CGPoint::new(ox, 0.0),
            &CGSize::new(p.w as f64, h as f64),
        );
        with_page_clip(&ctx, box_, || draw_card(p, &ctx, h as f64, ox, 0.0))?;
    }
    // 正中一条中缝线：让「一张纸对折、左右各一页」一眼可辨。
    draw_fold(&ctx, h as f64, sheet_w, first);
    encode_png(&mut ctx, w, h)
}

/// 把位图上下文里的像素抄成 PNG（`bytes_per_row` 可能带对齐填充，逐行拷贝）。
fn encode_png(ctx: &mut CGContext, w: usize, h: usize) -> Result<Vec<u8>, String> {
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

// ── PDF：同一套版式，矢量落盘（A4 / A3）───────────────────────────────
//
// 「Markdown 与 A3 导出的翻译层」的最后一环：版式层只认 `w / h`，所以 A3 不需要
// 另一条绘制链——同一份 `CardLayout` 画进位图是 PNG，画进 PDF 上下文就是矢量页。
// 这里只补 CoreGraphics 里 core-graphics 0.24 没包的四个入口。

#[cfg(target_os = "macos")]
extern "C" {
    fn CGPDFContextCreateWithURL(
        url: core_foundation::url::CFURLRef,
        media_box: *const CGRect,
        auxiliary_info: core_foundation::base::CFTypeRef,
    ) -> core_graphics::sys::CGContextRef;
    fn CGPDFContextBeginPage(
        ctx: core_graphics::sys::CGContextRef,
        page_info: core_foundation::base::CFTypeRef,
    );
    fn CGPDFContextEndPage(ctx: core_graphics::sys::CGContextRef);
    fn CGPDFContextClose(ctx: core_graphics::sys::CGContextRef);
}

/// 把若干页版式写成一份**矢量 PDF**（一页一页流式落盘），返回写出的页数。
///
/// 页面尺寸**完全由 `pages[0].w / .h` 决定**：本函数不认识 A3、也不认识 A4，
/// 只认版式交给它的两个数。调用方用 `Paper::page_pdf_geom()`（72dpi＝pt）排版，
/// 于是单页 A3 = 841.89×1190.55pt，字级直接就是磅值，缩放交给阅读器插值。
///
/// 这是**单页拼版**（一版一页）。双页折页见 [`render_pdf_spread`]。
///
/// 失败一律返回可读原因，绝不 panic——导出路径最怕崩在半路。
#[cfg(target_os = "macos")]
pub fn render_pdf(path: &std::path::Path, pages: &[CardLayout]) -> Result<usize, String> {
    let first = pages
        .first()
        .ok_or_else(|| "没有可导出的页面".to_string())?;
    let plan: Vec<Vec<(f64, f64, usize)>> =
        (0..pages.len()).map(|i| vec![(0.0, 0.0, i)]).collect();
    pdf_write(path, first.w as f64, first.h as f64, pages, &plan, false)
}

/// **双页拼版（书籍折页）**：一张横放的大纸上并排两个单页。
///
/// ```text
///   右開本（古籍，默认）            左開本（現代）
///   ┌──────────┬─┬──────────┐      ┌──────────┬─┬──────────┐
///   │  第 1 页 │中│  第 2 页 │      │  第 2 页 │中│  第 1 页 │
///   │  （奇）  │縫│  （偶）  │      │  （偶）  │縫│  （奇）  │
///   └──────────┴─┴──────────┘      └──────────┴─┴──────────┘
/// ```
///
/// * 媒体框只认 `sheet_w × sheet_h`（横的；A3 = 1190.55×841.89pt）；
/// * 每张纸放 **2 个单页**，落点由 [`spread_slots`] 给（与屏幕拼版预览同源）；
///   两页各自在自己那半边居中，中间是中缝——**绝不拉宽铺满整张纸**；
/// * 末张只剩一页时，另一侧留白——书末页背面本来就是空的。
///
/// 返回写出的**纸张数**（不是单页数）。
#[cfg(target_os = "macos")]
pub fn render_pdf_spread(
    path: &std::path::Path,
    pages: &[CardLayout],
    sheet_w: f32,
    sheet_h: f32,
    binding: Binding,
) -> Result<usize, String> {
    let first = pages
        .first()
        .ok_or_else(|| "没有可导出的页面".to_string())?;
    // `slots[0]` = 奇数页的落点、`slots[1]` = 偶数页的落点（页序，不是左右）。
    // `pages` 的下标 0 就是第 1 页（奇数），故两块一批正好对上。
    let slots = spread_slots(sheet_w, first.w, binding);
    let plan: Vec<Vec<(f64, f64, usize)>> = pages
        .chunks(2)
        .enumerate()
        .map(|(k, chunk)| {
            let mut one = vec![(slots[0] as f64, 0.0, k * 2)];
            if chunk.len() > 1 {
                one.push((slots[1] as f64, 0.0, k * 2 + 1));
            }
            one
        })
        .collect();
    pdf_write(path, sheet_w as f64, sheet_h as f64, pages, &plan, true)
}

/// PDF 落盘的**唯一实现**：`plan` 里每一项是一张纸，`(dx, dy, 页下标)` 是这页在
/// 该纸上的屏幕坐标偏移。
///
/// 单页拼版 = 每张纸一个 `(0,0,i)`；双页拼版 = 每两个页下标并到一张纸。
#[cfg(target_os = "macos")]
fn pdf_write(
    path: &std::path::Path,
    sheet_w: f64,
    sheet_h: f64,
    pages: &[CardLayout],
    plan: &[Vec<(f64, f64, usize)>],
    fold: bool,
) -> Result<usize, String> {
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;
    use foreign_types::ForeignType;

    let url = CFURL::from_path(path, false)
        .ok_or_else(|| format!("路径无法转成文件 URL：{}", path.display()))?;
    let media = CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(sheet_w, sheet_h),
    );
    let raw = unsafe { CGPDFContextCreateWithURL(url.as_concrete_TypeRef(), &media, std::ptr::null()) };
    if raw.is_null() {
        return Err("无法创建 PDF 画布（CoreGraphics 返回空）".to_string());
    }
    // `from_ptr` 接管所有权：Drop 时 `CGContextRelease`；`Close` 在末尾显式调。
    let ctx = unsafe { CGContext::from_ptr(raw) };
    ctx.set_allows_antialiasing(true);
    ctx.set_should_antialias(true);

    // 纸色：取第一页的底色（同一批导出必然是同一套样式）。
    let base = pages
        .first()
        .map(|p| p.bg)
        .ok_or_else(|| "没有可导出的页面".to_string())?;
    let (br, bg, bb) = rgb(base);

    let mut n = 0usize;
    for slots in plan {
        // ⚠️ **必须先 BeginPage 再落笔**：BeginPage 之前收到的绘制会被丢掉，
        //    整页只剩一个空内容流（控制台报 "No content stream"）。纸色同理。
        unsafe { CGPDFContextBeginPage(raw, std::ptr::null()) };
        ctx.set_rgb_fill_color(br, bg, bb, 1.0);
        ctx.fill_rect(CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(sheet_w, sheet_h),
        ));
        let mut failed: Option<String> = None;
        for (dx, dy, idx) in slots {
            let Some(page) = pages.get(*idx) else { continue };
            // 每页自带底色，先铺满它自己那块（单页拼版时与整纸同色，多铺一次无害）。
            let (pr, pg, pb) = rgb(page.bg);
            let box_ = CGRect::new(
                &CGPoint::new(*dx, sheet_h - page.h as f64 - *dy),
                &CGSize::new(page.w as f64, page.h as f64),
            );
            ctx.set_rgb_fill_color(pr, pg, pb, 1.0);
            ctx.fill_rect(box_);
            // 落笔前 clip 到本页矩形：越界的字被裁掉，绝不写进隔壁那半张纸。
            let drew = with_page_clip(&ctx, box_, || {
                draw_card(page, &ctx, sheet_h, *dx, *dy)
            });
            if let Err(e) = drew {
                failed = Some(e);
                break;
            }
        }
        // 双页拼版：整纸正中画一条中缝线（两页都落完笔才画，压在最上层）。
        if fold && failed.is_none() {
            if let Some(page) = slots.first().and_then(|s| pages.get(s.2)) {
                draw_fold(&ctx, sheet_h, sheet_w as f32, page);
            }
        }
        if let Some(e) = failed {
            // 半路失败也要把这一页收尾再关文件，绝不落一个坏 PDF。
            unsafe { CGPDFContextEndPage(raw) };
            unsafe { CGPDFContextClose(raw) };
            return Err(e);
        }
        unsafe { CGPDFContextEndPage(raw) };
        // 催 CG 把刚收的这一页写下去（不等 Close 时一次性吐），内存与页数无关。
        ctx.flush();
        n += 1;
    }
    unsafe { CGPDFContextClose(raw) };
    Ok(n)
}

#[cfg(not(target_os = "macos"))]
pub fn render_pdf(_path: &std::path::Path, _pages: &[CardLayout]) -> Result<usize, String> {
    Err("PDF 导出仅在 macOS 支持".to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn render_pdf_spread(
    _path: &std::path::Path,
    _pages: &[CardLayout],
    _sheet_w: f32,
    _sheet_h: f32,
    _binding: Binding,
) -> Result<usize, String> {
    Err("PDF 导出仅在 macOS 支持".to_string())
}

/// **把一张版式画进任意 CG 上下文**（位图上下文与 PDF 上下文共用）。
///
/// `hf` 是画布高度（CG 坐标原点在**左下**，而版式数据是屏幕坐标原点在**左上**）。
/// `(ox, oy)` 是这一页在画布上的**屏幕坐标偏移**——双页拼版靠它把后一页推到
/// 纸的左半边；单页拼版恒 `(0, 0)`，与 v1.9.x 逐像素一致。
///
/// 两处上下文的行为相同：`CGBitmapContext` 与 `CGPDFContext` 都是 CG 坐标、
/// 都是 y 向上，故同一套「只把每个元素的 y 镜像一次」的换算完全通用——
/// **两边都不许翻 CTM**（翻了字会被镜像，v1.58 的坑）。
fn draw_card(l: &CardLayout, ctx: &CGContext, hf: f64, ox: f64, oy: f64) -> Result<(), String> {
    // 防雷 ①：**不翻 CTM**。整幅坐标仍走 CG 原生（原点左下、y 向上），
    // 每个元素落笔前用 `cg_*` 把屏幕坐标（左上原点）换算过来。
    // 位图缓冲的第 0 行是图像**顶行**，故「屏幕 y」与「缓冲行号」同序。
    // 矩形顶边（屏幕 y + 偏移）→ CG 底边 y：矩形整体镜像到下半平面。
    let cg_top = move |y_top: f64, hgt: f64| hf - oy - y_top - hgt;
    // 基线（屏幕 y + 偏移）→ CG 基线：**只做镜像，不做缩放**，字形因此保持正向。
    let cg_base = move |y: f64| hf - oy - y;
    let sx = move |x: f64| x + ox;
    ctx.set_allows_antialiasing(true);
    ctx.set_should_antialias(true);

    // ---- 底色 ----
    let (br, bg, bb) = rgb(l.bg);
    ctx.set_rgb_fill_color(br, bg, bb, 1.0);
    ctx.fill_rect(CGRect::new(
        &CGPoint::new(ox, hf - oy - l.h as f64),
        &CGSize::new(l.w as f64, l.h as f64),
    ));

    // ---- 矩形：高亮底（填充）/ 界框（描边）----
    for r in &l.rects {
        let (cr, cg, cb, ca) = rgba(r.color);
        let rect = CGRect::new(
            &CGPoint::new(sx(r.x as f64), cg_top(r.y as f64, r.h as f64)),
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

    // ---- 界行（列间竖细线）：**在文字之前**落笔，字恒压在线上 ----
    //
    // 绘制序（用户 v1.61.0 明令）：底色 → 版框 / 界行 → 文字。
    // 线宽 1px 时 CG 会把线骑在路径两侧各半个像素上，落在整数坐标会糊成两
    // 行灰边——故 x 取「整像素 + 0.5」，界行才是一根干净的细线。
    for r in &l.rules {
        let (cr, cg, cb, ca) = rgba(r.color);
        ctx.set_rgb_stroke_color(cr, cg, cb, ca);
        ctx.set_line_width(r.width as f64);
        let x = (sx(r.x1 as f64)).floor() + 0.5;
        ctx.begin_path();
        ctx.move_to_point(x, cg_base(r.y1 as f64));
        ctx.add_line_to_point(x, cg_base(r.y2 as f64));
        ctx.stroke_path();
    }

    // ---- 朱批引线：也在文字之前落笔（引线可以穿过格，但绝不盖在字上）----
    for ln in &l.lines {
        let (r, g, b, a) = rgba(ln.color);
        ctx.set_rgb_stroke_color(r, g, b, a);
        ctx.set_line_width(ln.width as f64);
        ctx.begin_path();
        ctx.move_to_point(sx(ln.x1 as f64), cg_base(ln.y1 as f64));
        ctx.add_line_to_point(sx(ln.x2 as f64), cg_base(ln.y2 as f64));
        ctx.stroke_path();
    }

    // ---- 正文 / 批注 / 落款：逐字落笔（永远最后，字恒在最顶层）----
    let mut fonts: Vec<(i32, core_text::font::CTFont, f64)> = Vec::new();
    for c in &l.chars {
        let key = (c.fs * 4.0).round() as i32;
        let (font, asc) = match fonts.iter().position(|(k, _, _)| *k == key) {
            Some(i) => (fonts[i].1.clone(), fonts[i].2),
            None => {
                let f = pick_font(l.font, c.fs as f64)?;
                // 基线沉量（＝字格顶到基线）——见 [`baseline_drop`]。
                let a = baseline_drop(c.fs as f64, &f);
                fonts.push((key, f.clone(), a));
                (f, a)
            }
        };
        let (gx, gy) = glyph_origin(c, asc);
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
        // ⚠️ 竖排**不旋转字形**：方块汉字竖排靠「纵向依次排列」实现，
        // 逐字落笔即可，转 90° 反而把字放倒。这里只把基线镜像到 CG 侧，
        // 字形本身保持正向（CTM 未翻，故无镜像）。
        font.draw_glyphs(&[glyph], &[CGPoint::new(sx(gx), cg_base(gy))], ctx.clone());
        // **加粗**（Markdown `**…**`）：同字再落两笔极微量偏移，墨迹自然加厚。
        //
        // 为什么不换 Bold 字体面：`pick_font` 走的是 PostScript 回退链，系统宋体
        // 未必有 `Songti SC Bold` 这个名字；一旦一个都取不到，整张卡直接报错——
        // 为一个视觉细节赔上出图，不划算。半像素级的三次叠印在 1080 宽的卡上
        // 已经能看出「这一行是重的」，且与底色同一抗锯齿，不会有描边毛刺。
        if c.bold {
            let d = 0.45;
            font.draw_glyphs(
                &[glyph],
                &[CGPoint::new(sx(gx) + d, cg_base(gy))],
                ctx.clone(),
            );
            font.draw_glyphs(
                &[glyph],
                &[CGPoint::new(sx(gx), cg_base(gy + d))],
                ctx.clone(),
            );
        }
    }

    // ---- 朱印 ----
    if let Some(s) = &l.seal {
        paint_seal(&ctx, s, l.bg, hf, ox, oy);
    }

    Ok(())
}

/// 朱砂方章：**纯色红方块 + 黑色「笺读」二字**（横排居中）。
///
/// 先跑通闭环：不画鱼尾、不刻书名——印就是一枚朱砂底、墨字落款的方章。
fn paint_seal(ctx: &CGContext, s: &PlacedSeal, _paper: u32, h: f64, ox: f64, oy: f64) {
    let (r, g, b) = rgb(CINNABAR);
    let sz = s.size as f64;
    ctx.set_rgb_fill_color(r, g, b, 1.0);
    // 方章是实心块，顶边镜像即可（对称图形，翻不翻看不出——但坐标必须换，
    // 否则它会跑到画布上端去）。
    ctx.fill_rect(CGRect::new(
        &CGPoint::new(s.x as f64 + ox, h - oy - s.y as f64 - sz),
        &CGSize::new(sz, sz),
    ));

    // 印文「笺讀」：黑字、横排、居中（两字各占半方）。
    let fs = sz * 0.42;
    let font = match pick_font(ShareFont::Song, fs) {
        Ok(f) => f,
        Err(_) => return, // 没有字体就不刻字，方块照样在——绝不因印文失败整张图。
    };
    // 印文基线：字形顶边贴住「方块内居中位」，再镜像到 CG 侧。
    let asc = baseline_drop(fs, &font);
    let baseline = h - oy - (s.y as f64 + sz * 0.5 - fs * 0.5 + asc);
    // 印文取版式层给的 `seal_text`（默认「笺讀」），不再写死二字——
    // 写死会让「书名印」永远刻成「笺讀」。
    let text: Vec<char> = if s.text.is_empty() {
        "笺讀".chars().collect()
    } else {
        s.text.chars().take(2).collect()
    };
    for (i, ch) in text.iter().enumerate() {
        let mut unit: [u16; 1] = [0];
        ch.encode_utf16(&mut unit);
        let mut glyph: core_graphics::font::CGGlyph = 0;
        let ok = unsafe { font.get_glyphs_for_characters(unit.as_ptr(), &mut glyph, 1) };
        if !ok {
            continue;
        }
        let x = s.x as f64 + ox + sz * 0.08 + i as f64 * fs;
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
            md: &[],
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
            stamp: "",
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

    /// **字形正向 + 绝对顶格**（v1.59.0 回归防线）
    ///
    /// 用两个上下不对称的探针字把「镜像」钉死：「丁」横在顶（墨迹重心偏上）、
    /// 「上」横在底（重心偏下）。字形一旦被垂直镜像，两条断言必然崩。
    #[test]
    fn glyphs_are_upright_and_columns_start_on_the_baseline() {
        let body: Vec<char> = "丁上道德經第一章道可道非常道名可名非常名無名天地之始有名萬物之母"
            .chars()
            .collect();
        let red = vec![false; body.len()];
        let spec = LayoutSpec {
            body: &body,
            red: &red,
            md: &[],
            note: "",
            show_notes: false,
            sign: "道德經 · 第一章",
            show_sign: true,
            date: "二〇二六年十月",
            show_date: true,
            columns: 12,
            bg: ShareBg::Xuan,
            frame: ShareFrame::None,
            seal: ShareSeal::Twin,
            seal_text: "笺讀",
            font: ShareFont::Song,
            w: 1080.0,
            h: 1920.0,
            stamp: "",
        };
        let l = layout(&spec);
        let png = render_png(&l).expect("离屏出图失败");
        let img = image::load_from_memory(&png).expect("PNG 读不回来").to_rgb8();
        let (iw, ih) = (img.width(), img.height());
        let br = ((l.bg >> 16) & 0xFF) as i32;
        let bgg = ((l.bg >> 8) & 0xFF) as i32;
        let bb = (l.bg & 0xFF) as i32;
        let is_ink = |x: u32, y: u32| {
            let p = img.get_pixel(x, y);
            (p[0] as i32 - br).abs() + (p[1] as i32 - bgg).abs() + (p[2] as i32 - bb).abs() > 120
        };
        // 某字格内墨迹重心：归一到 −1（格顶）… +1（格底）。
        let bias = |ch: char| -> Option<f64> {
            let c = l.chars.iter().find(|c| c.ch == ch)?;
            let s = c.cell.max(1.0) as u32;
            let (x0, y0) = (c.x.max(0.0) as u32, c.y.max(0.0) as u32);
            let (mut sy, mut n) = (0.0f64, 0.0f64);
            for y in y0..(y0 + s).min(ih) {
                for x in x0..(x0 + s).min(iw) {
                    if is_ink(x, y) {
                        sy += (y - y0) as f64;
                        n += 1.0;
                    }
                }
            }
            if n < 8.0 {
                return None;
            }
            Some((sy / n / s as f64) * 2.0 - 1.0)
        };
        let ding = bias('丁').expect("「丁」没画上");
        let shang = bias('上').expect("「上」没画上");
        println!("探针字墨迹重心：丁={ding:+.3}（应为负＝偏上）　上={shang:+.3}（应为正＝偏下）");
        // 印章（朱砂）重心应在左下。
        let (mut sx, mut sy, mut sn) = (0.0f64, 0.0f64, 0.0f64);
        for y in 0..ih {
            for x in 0..iw {
                let p = img.get_pixel(x, y);
                if p[0] > 140 && p[1] < 110 && p[2] < 110 {
                    sx += x as f64;
                    sy += y as f64;
                    sn += 1.0;
                }
            }
        }
        assert!(sn > 0.0, "没找到朱砂像素");
        println!(
            "朱砂重心：x={:.0}/{iw}　y={:.0}/{ih}（应偏左下）",
            sx / sn,
            sy / sn
        );
        // 首行顶格：**正文**每一列第一个字的 y 必须彼此相等。
        // 落款 / 批注是小字（字号不同、位置也不同），先按「正文字号」剔掉，
        // 否则会把左下角的落款误当成一列正文。
        let body_fs = l
            .chars
            .iter()
            .map(|c| c.fs)
            .fold(0.0f32, f32::max);
        let head: Vec<f32> = {
            let mut seen = std::collections::HashSet::new();
            l.chars
                .iter()
                .filter(|c| !c.punct && (c.fs - body_fs).abs() < 0.01)
                .filter(|c| seen.insert((c.x * 4.0).round() as i64))
                .map(|c| c.y)
                .collect()
        };
        println!("各列首字 y：{head:?}");
        assert!(head.len() >= 2, "列数太少，顶格无从验证：{}", head.len());
        let y_first = head[0];
        for y in &head {
            assert!(
                (y - y_first).abs() < 0.01,
                "列首未对齐基准线：{y} ≠ {y_first}"
            );
        }
        assert!(ding < 0.0, "「丁」重心偏下，字形被垂直镜像了：{ding:+.3}");
        assert!(shang > 0.0, "「上」重心偏上，字形被垂直镜像了：{shang:+.3}");
    }

    /// **版框 + 界行 + 贴框顶格 + 列首悬挂**（v1.61.0 像素级防线）
    ///
    /// 四条各钉一处，任何一条回归都会立刻崩：
    ///   ① 版框：框线那一整行确实是框色（一整圈矩形，不是半截）；
    ///   ② 界行：列间竖线自上而下贯通版心，且**不盖字**（字在图里仍可读）；
    ///   ③ 顶格：首字最高点正好卡在框线下方，**上方一格空白都没有**；
    ///   ④ 悬挂：列首句读被推到网格右缘之外，绝不占格把正文往下挤。
    #[test]
    fn frame_rules_top_alignment_and_hung_punct_are_all_in_place() {
        // 起手就是句读 → 逼出「列首悬挂」这条禁则路径。
        let body: Vec<char> = "，道可道非常道名可名非常名無名天地之始有名萬物之母故常無欲以觀其妙常有欲以觀其徼此兩者同出而異名"
            .chars()
            .collect();
        let red = vec![false; body.len()];
        let spec = LayoutSpec {
            body: &body,
            red: &red,
            md: &[],
            note: "此章言道為萬物之始",
            show_notes: true,
            sign: "道德經 · 第一章",
            show_sign: true,
            date: "二〇二六年十月",
            show_date: true,
            columns: 8,
            bg: ShareBg::Xuan,
            frame: ShareFrame::Red,
            seal: ShareSeal::Twin,
            seal_text: "笺讀",
            font: ShareFont::Song,
            w: 1080.0,
            h: 1920.0,
            stamp: "",
        };
        let l = layout(&spec);

        // ---- ① 版框（版式层）：一整圈描边矩形，内缘是首行的天花板 ----
        let frame = l
            .rects
            .iter()
            .find(|r| r.stroke.is_some())
            .unwrap_or_else(|| panic!("没排出版框"));
        let fw = frame.stroke.unwrap_or(1.0);
        let inner_top = frame.y + fw * 0.5;

        // ---- ② 界行：条数 = 总列数 − 1，且纵向贯通版心 ----
        let body_fs = l.chars.iter().map(|c| c.fs).fold(0.0f32, f32::max);
        let body_cols: Vec<&PlacedChar> = l
            .chars
            .iter()
            .filter(|c| (c.fs - body_fs).abs() < 0.01 && !c.punct)
            .collect();
        let mut xs: Vec<f32> = body_cols.iter().map(|c| c.x).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        xs.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert!(xs.len() >= 2, "列数太少：{}", xs.len());
        // 批注列也算一列，故界行 = 正文列数 + 批注列 − 1。
        assert_eq!(
            l.rules.len(),
            xs.len(),
            "界行条数不对：{:?}（列 {} + 批注 1 − 1）",
            l.rules.len(),
            xs.len()
        );
        let r0 = &l.rules[0];
        assert!((r0.y2 - r0.y1) > l.h * 0.7, "界行没贯通版心");
        assert!(r0.width < fw + 0.01, "界行比版框还粗，抢了字的焦点");

        // ---- ③ 顶格：每一列首字同 y，且紧贴版框内缘 ----
        let y0 = body_cols.iter().map(|c| c.y).fold(f32::MAX, f32::min);
        for x in &xs {
            let head = body_cols
                .iter()
                .filter(|c| (c.x - *x).abs() < 0.01)
                .map(|c| c.y)
                .fold(f32::MAX, f32::min);
            assert!((head - y0).abs() < 0.01, "列首未顶格：{head} ≠ {y0}");
        }
        assert!(
            (y0 - inner_top) >= 0.0 && (y0 - inner_top) < 12.0,
            "首行没卡在框线下方：y0={y0} 框内缘={inner_top}"
        );

        // ---- ④ 悬挂：列首句读在网格右缘之外，且不占格 ----
        let comma = l
            .chars
            .iter()
            .find(|c| c.ch == '，')
            .unwrap_or_else(|| panic!("没排出句读"));
        assert!(comma.hung, "列首句读没被悬挂");
        let grid_right = body_cols
            .iter()
            .map(|c| c.x + c.cell)
            .fold(f32::MIN, f32::max);
        assert!(
            comma.x >= grid_right - 1.0,
            "句读没挂到网格右缘之外：{} < {}",
            comma.x,
            grid_right
        );
        // 悬挂不占格：句读之后的正文首字仍与句读同 y（都在基准线上）。
        let first_real = body_cols
            .iter()
            .copied()
            .min_by_key(|c| (c.y * 4.0).round() as i64)
            .unwrap_or_else(|| panic!("没有正文字"));
        assert!(
            (first_real.y - y0).abs() < 0.01,
            "句读把正文挤下去了：{} ≠ {}",
            first_real.y,
            y0
        );

        // ---- 像素验收：出图，逐条核对 ----
        let png = render_png(&l).unwrap_or_else(|e| panic!("离屏出图失败：{e}"));
        let img = image::load_from_memory(&png).expect("PNG 读不回来").to_rgb8();
        let (iw, ih) = (img.width(), img.height());
        let br = ((l.bg >> 16) & 0xFF) as i32;
        let bgg = ((l.bg >> 8) & 0xFF) as i32;
        let bb = (l.bg & 0xFF) as i32;
        // 「与底色不同」的宽松判据：界行很淡（α 0x38），不能用墨迹的高阈值。
        let diff = |x: u32, y: u32| {
            let p = img.get_pixel(x, y);
            (p[0] as i32 - br).abs() + (p[1] as i32 - bgg).abs() + (p[2] as i32 - bb).abs()
        };
        // ① 版框顶边：整行的绝大多数像素都该是框色。
        let fy = frame.y.round() as u32;
        let x0u = (frame.x.round() as u32).min(iw - 1);
        let x1u = ((frame.x + frame.w).round() as u32).min(iw - 1);
        let mut on = 0u32;
        let mut tot = 0u32;
        for x in x0u..=x1u {
            tot += 1;
            if diff(x, fy) > 24 {
                on += 1;
            }
        }
        assert!(
            on as f32 / tot as f32 > 0.90,
            "版框顶边没画全：{on}/{tot}"
        );
        // ② 界行：沿竖线自上而下取样，绝大多数该有色。
        let rx = (r0.x1.round() as u32).min(iw - 1);
        let (mut on2, mut tot2) = (0u32, 0u32);
        let mut y = (r0.y1.round() as u32) + 4;
        while y < (r0.y2.round() as u32).min(ih).saturating_sub(4) {
            tot2 += 1;
            if diff(rx, y) > 12 || diff(rx.saturating_sub(1), y) > 12 {
                on2 += 1;
            }
            y += 7;
        }
        assert!(
            on2 as f32 / tot2.max(1) as f32 > 0.85,
            "界行没画出来：{on2}/{tot2}"
        );
        // ③ 顶格：框线以下到 y0 之前**一格墨都没有**（字顶严格不压线）。
        // 界行是贯通版心的竖线，会合法地穿过这段天头——扫描时避开它们。
        let rule_xs: Vec<u32> = l.rules.iter().map(|r| r.x1.round() as u32).collect();
        let on_rule = |x: u32| rule_xs.iter().any(|rx| (*rx as i32 - x as i32).abs() <= 2);
        let col_x = (xs[0].round() as u32).min(iw - 1);
        let scan_from = (inner_top + fw + 1.0).round() as u32;
        let scan_to = (y0 - 1.0).max(scan_from as f32 + 1.0).round() as u32;
        for y in scan_from..scan_to.min(ih) {
            for x in col_x..(col_x + 40).min(iw) {
                if on_rule(x) {
                    continue;
                }
                assert!(
                    diff(x, y) <= 24,
                    "首字压到框线上了：({x},{y}) 有墨（y0={y0}）"
                );
            }
        }
        // 首字的最顶端墨迹必须落在基准线附近（不悬空）。
        let first_ink = (scan_to..ih)
            .find(|&y| (col_x..(col_x + 40).min(iw)).any(|x| diff(x, y) > 120));
        let first_ink = first_ink.unwrap_or_else(|| panic!("首行没有墨迹"));
        assert!(
            (first_ink as f32) < y0 + body_cols[0].cell * 0.40,
            "首字没贴住基准线：墨迹起于 {first_ink}，y0={y0}"
        );

        if let Ok(home) = std::env::var("HOME") {
            let dest = std::path::PathBuf::from(home)
                .join("Desktop")
                .join("笺读-分享卡-界行版.png");
            if std::fs::write(&dest, &png).is_ok() {
                println!("已出图：{}", dest.display());
            }
        }
    }
    /// **拼版预览**（`render_spread_png`）：横放的纸上真有左右两版，中间空一条。
    ///
    /// 这是屏幕那一路的验收——预览与 PDF 共用同一份 `CardLayout` 与同一个
    /// `spread_slots`，所以「屏幕上看到的」必须就是「印出来的」。
    #[test]
    fn spread_preview_png_has_two_pages_and_a_gutter() {
        use crate::share_layout::{Binding, Imposition, Paper};

        let page = Paper::A3.preview_page_geom(Imposition::Spread);
        let sheet = Paper::A3.preview_sheet_geom(Imposition::Spread);
        let a_text: Vec<char> = "天地玄黃宇宙洪荒日月盈昃辰宿列張".chars().collect();
        let b_text: Vec<char> = "寒來暑往秋收冬藏閏餘成歲律呂調陽".chars().collect();
        let mut cards = Vec::new();
        for body in [&a_text, &b_text] {
            let red = vec![false; body.len()];
            let spec = LayoutSpec {
                body,
                red: &red,
                md: &[],
                note: "",
                show_notes: false,
                sign: "",
                show_sign: false,
                date: "",
                show_date: false,
                columns: 8,
                bg: ShareBg::Xuan,
                frame: ShareFrame::Red,
                seal: ShareSeal::Twin,
                seal_text: "笺校",
                font: ShareFont::Song,
                stamp: "",
                w: page.w,
                h: page.h,
            };
            cards.push(layout(&spec));
        }
        let png = render_spread_png(&cards, sheet.w, sheet.h, Binding::Right)
            .unwrap_or_else(|e| panic!("拼版预览出图失败：{e}"));
        let img = image::load_from_memory(&png).expect("PNG 读不回来").to_rgb8();
        assert_eq!((img.width(), img.height()), (sheet.w as u32, sheet.h as u32));

        // 与底色不同的像素 = 有东西（文字 / 版框 / 界行）。
        let bg = cards[0].bg;
        let (br, bgc, bb) = (
            ((bg >> 16) & 0xFF) as i32,
            ((bg >> 8) & 0xFF) as i32,
            (bg & 0xFF) as i32,
        );
        let ink = |x: u32, y: u32| -> bool {
            let p = img.get_pixel(x, y);
            (p[0] as i32 - br).abs() + (p[1] as i32 - bgc).abs() + (p[2] as i32 - bb).abs() > 40
        };
        let half = img.width() / 2;
        let count = |x0: u32, x1: u32| -> u32 {
            let mut n = 0;
            for y in 0..img.height() {
                for x in x0..x1 {
                    if ink(x, y) {
                        n += 1;
                    }
                }
            }
            n
        };
        let left = count(0, half);
        let right = count(half, img.width());
        assert!(left > 500, "左半张没画上东西：{left} 个墨点");
        assert!(right > 500, "右半张没画上东西：{right} 个墨点");
        // 中缝：纸正中有一条**中缝线**，线两侧各留一条空白带。
        //
        // 用户 2026-11-01 真机反馈「不像古籍折页」，故正中现在画一条竖线：
        // ・线本身必须有墨（否则就是又回到「两页挨着、看不出折页」）；
        // ・线两侧 8px 内必须干净（有墨就是两页粘连 / 线画歪了）。
        let line = count(half - 2, half + 2);
        assert!(line > 0, "中缝线没画出来（正中 4px 无墨）");
        let near_l = count(half - 10, half - 3);
        let near_r = count(half + 3, half + 10);
        assert_eq!(near_l, 0, "中缝线左边有墨：{near_l}（两页粘连了）");
        assert_eq!(near_r, 0, "中缝线右边有墨：{near_r}（两页粘连了）");
    }

    /// **A3 矢量 PDF**：媒体框必须真是 A3（842×1191pt），文件头是 %PDF。
    ///
    /// 这条测试盯的是「物理尺寸参数化」的最后一毫米：版式层只吃了两个数
    /// （`w/h`），PDF 的纸幅必须原样等于这两个数——不是把 A4 画布放大。
    #[test]
    fn renders_an_a3_pdf_whose_media_box_is_really_a3() {
        use crate::share_layout::Paper;

        let body: Vec<char> = "天地玄黃宇宙洪荒日月盈昃辰宿列張".chars().collect();
        let red = vec![false; body.len()];
        let g = Paper::A3.pdf_geom();
        let spec = LayoutSpec {
            body: &body,
            red: &red,
            md: &[],
            note: "",
            show_notes: false,
            sign: "",
            show_sign: false,
            date: "",
            show_date: false,
            columns: 12,
            bg: ShareBg::Xuan,
            frame: ShareFrame::Red,
            seal: ShareSeal::Twin,
            seal_text: "笺校",
            font: ShareFont::Song,
            stamp: "",
            w: g.w,
            h: g.h,
        };
        let card = layout(&spec);
        let out = std::env::temp_dir().join("jiandu_a3_pdf_test.pdf");
        let n = render_pdf(&out, &[card]).expect("A3 PDF 应能写出");
        assert_eq!(n, 1, "页数不对");

        let bytes = std::fs::read(&out).expect("PDF 应已落盘");
        assert!(bytes.starts_with(b"%PDF"), "不是 PDF 文件");
        assert!(bytes.len() > 2000, "PDF 过小，多半是空内容流：{}", bytes.len());
        // 媒体框：CG 会把 842 / 1191 写成 "842 1191"（或带小数），两者都认。
        let hay = String::from_utf8_lossy(&bytes);
        assert!(hay.contains("MediaBox"), "PDF 里找不到 MediaBox");
        let near = hay.find("MediaBox").map(|i| &hay[i..(i + 60).min(hay.len())]);
        let near = near.unwrap_or("");
        assert!(
            near.contains("842") || near.contains("841.8"),
            "媒体框不是 A3 宽度：{near}"
        );
        let _ = std::fs::remove_file(&out);
    }
}
