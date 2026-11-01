//! iOS 专属：把 core 排版结果导出为矢量 PDF。
//!
//! **底层桥接代码框架**（IPAD_POC_CHECKLIST.md §2.2）：用 raw CoreGraphics +
//! CoreText C FFI 在 iOS 上真正生成 PDF——不走 PDFKit（iOS 的 PDFKit 没有绘制
//! surface，无法「生成」PDF，只能观看/标注）。
//!
//! 关键设计：
//! * 复用 core 的 `calculate_grid` 算出的真实版式度量（字格宽高 / 行数 / 纸面尺寸），
//!   排版几何与 macOS 端同源、不重写；
//! * 坐标系保持**左上原点**（设备坐标 = `outer_h - y_top`），与 SwiftUI / Apple
//!   Pencil 的视图坐标同系 → 将来 Pencil 红线坐标反推 PDF 空间时零 Y 翻转歧义；
//! * 逐字用 CoreText 取字形、CoreGraphics 落笔（CJK 经字形映射，非 MacRoman）。
//!
//! ⚠️ 本文件只在 `target_os = "ios"` 编译（见 lib.rs 的 `#[cfg]`）。

#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "CoreText", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    // ── CoreFoundation ──
    fn CFStringCreateWithCString(
        alloc: *mut core::ffi::c_void,
        cstr: *const core::ffi::c_char,
        encoding: i64,
    ) -> *const core::ffi::c_void;
    fn CFURLCreateWithFileSystemPath(
        alloc: *mut core::ffi::c_void,
        path: *const core::ffi::c_void,
        path_style: i64,
        is_directory: u8,
    ) -> *const core::ffi::c_void;
    fn CFRelease(cf: *const core::ffi::c_void);
    fn CFAllocatorGetDefault() -> *mut core::ffi::c_void;

    // ── CoreGraphics：PDF 上下文 + 绘制 ──
    fn CGDataConsumerCreateWithURL(url: *const core::ffi::c_void) -> *mut core::ffi::c_void;
    fn CGDataConsumerRelease(consumer: *mut core::ffi::c_void);
    fn CGPDFContextCreate(
        consumer: *mut core::ffi::c_void,
        media_box: *const CGRect,
        auxiliary_info: *mut core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    fn CGPDFContextBeginPage(ctx: *mut core::ffi::c_void, page_info: *mut core::ffi::c_void);
    fn CGPDFContextEndPage(ctx: *mut core::ffi::c_void);
    fn CGPDFContextClose(ctx: *mut core::ffi::c_void);
    fn CGContextSetRGBFillColor(
        ctx: *mut core::ffi::c_void,
        r: f64,
        g: f64,
        b: f64,
        a: f64,
    );
    fn CGContextSetFont(ctx: *mut core::ffi::c_void, font: *mut core::ffi::c_void);
    fn CGContextSetFontSize(ctx: *mut core::ffi::c_void, size: f64);
    fn CGContextShowGlyphsAtPoint(
        ctx: *mut core::ffi::c_void,
        x: f64,
        y: f64,
        glyphs: *const u16,
        count: usize,
    );

    // ── CoreText：取字形 ──
    fn CTFontCreateWithName(
        name: *const core::ffi::c_void,
        size: f64,
        matrix: *const CGAffineTransform,
    ) -> *mut core::ffi::c_void;
    fn CTFontGetGlyphsForCharacters(
        font: *mut core::ffi::c_void,
        characters: *const u16,
        glyphs: *mut u16,
        count: usize,
    ) -> u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

const K_CF_STRING_ENCODING_UTF8: i64 = 0x0800_0100;
const K_CF_URL_POSIX_PATH_STYLE: i64 = 0;
/// iOS 系统 CJK 字体（PingFang SC）。生产环境应内嵌字体子集（见 IPAD_POC_CHECKLIST.md R1）。
const FONT_NAME: &[u8] = b"PingFang SC\0";

/// 把一段文本按 core 真实版式排成竖排网格、绘制成「一叶一页」的矢量 PDF。
///
/// 返回 `Ok(())` 表示 PDF 已写到 `out_path`；任何 Core 框架调用失败都转成 `Err` 字符串。
pub fn render(out_path: &str, text: &str, columns: u32) -> Result<(), crate::ffi::PdfError> {
    // 1) 复用 core 排版，拿到真实纸面度量。
    let m = jiandu_core::grid::calculate_grid(jiandu_core::grid::GridInput {
        columns: columns.max(1) as usize,
        mode: jiandu_core::grid::LayoutMode::Facing,
        content_w: 800.0,
        content_h: 1000.0,
        zoom: 1.0,
        edge: 40.0,
        head: 70.0,
        foot: 35.0,
        margin: 0.06,
        line_scale: 1.0,
        fs_ratio: 0.0,
        line_ratio: 0.0,
        banxin_ratio: 0.0,
        dir: jiandu_core::grid::GridDir::Vertical,
        fs_max: f32::INFINITY,
    })
    .m;

    let outer_w = m.outer_w as f64;
    let outer_h = m.outer_h as f64;
    let col_w = m.col_w as f64;
    let row_h = m.row_h as f64;
    let rows = m.rows.max(1);
    let fs = (m.fs as f64).max(1.0);
    let margin_top = m.margin_top as f64;

    // 2) 媒体框（PDF 单位是 pt；这里用 px 当量，单页）。
    let media = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: outer_w,
            height: outer_h,
        },
    };

    // 3) 路径 → CFURL → CGDataConsumer → CGPDFContext。
    let c_path = std::ffi::CString::new(out_path).map_err(|e| format!("path error: {e}"))?;
    let cf_path = unsafe {
        CFStringCreateWithCString(
            std::ptr::null_mut(),
            c_path.as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if cf_path.is_null() {
        return Err(crate::ffi::PdfError::CoreFramework("CFStringCreateWithCString failed".into()));
    }
    let cf_url = unsafe {
        CFURLCreateWithFileSystemPath(CFAllocatorGetDefault(), cf_path, K_CF_URL_POSIX_PATH_STYLE, 0)
    };
    if cf_url.is_null() {
        unsafe { CFRelease(cf_path) };
        return Err(crate::ffi::PdfError::CoreFramework("CFURLCreateWithFileSystemPath failed".into()));
    }
    let consumer = unsafe { CGDataConsumerCreateWithURL(cf_url) };
    if consumer.is_null() {
        unsafe { CFRelease(cf_url); CFRelease(cf_path) };
        return Err(crate::ffi::PdfError::CoreFramework("CGDataConsumerCreateWithURL failed".into()));
    }
    let ctx = unsafe { CGPDFContextCreate(consumer, &media, std::ptr::null_mut()) };
    if ctx.is_null() {
        unsafe { CGDataConsumerRelease(consumer); CFRelease(cf_url); CFRelease(cf_path) };
        return Err(crate::ffi::PdfError::CoreFramework("CGPDFContextCreate failed".into()));
    }

    // 4) 字体（CJK 经字形映射，size 由 CTFont 携带）。
    let c_font = std::ffi::CString::new(FONT_NAME).unwrap();
    let cf_font = unsafe {
        CFStringCreateWithCString(std::ptr::null_mut(), c_font.as_ptr(), K_CF_STRING_ENCODING_UTF8)
    };
    let font = unsafe { CTFontCreateWithName(cf_font, fs, std::ptr::null()) };

    // 5) 落笔：左上原点 → 设备坐标 = outer_h - y_top（保持文字正立，不做 Y 翻转）。
    let result = unsafe {
        CGPDFContextBeginPage(ctx, std::ptr::null_mut());
        CGContextSetRGBFillColor(ctx, 0.13, 0.13, 0.13, 1.0); // 近墨色（与 macOS 主题同源）
        CGContextSetFont(ctx, font);
        CGContextSetFontSize(ctx, fs);

        let total_cols = ((text.chars().count() + rows - 1) / rows).max(1);
        let grid_left = (outer_w - total_cols as f64 * col_w) / 2.0;

        let mut ok = true;
        for (i, ch) in text.chars().enumerate() {
            let c = i / rows;
            let r = i % rows;
            let x = grid_left + (total_cols - 1 - c) as f64 * col_w;
            let y_top = margin_top + r as f64 * row_h;
            // 设备坐标（左下原点）：基线约在格中心偏下。
            let y_dev = outer_h - (y_top + row_h * 0.78);

            let uni = (ch as u32) as u16; // BMP 单码元（PoC；代理对/Ext-B 后续处理）
            let mut glyph: u16 = 0;
            let got = CTFontGetGlyphsForCharacters(font, &uni, &mut glyph, 1);
            if got == 0 || glyph == 0 {
                // 取不到字形（如 Ext-B）：跳过，不中断整页。
                continue;
            }
            CGContextShowGlyphsAtPoint(ctx, x + col_w / 2.0, y_dev, &glyph, 1);
            let _ = &mut ok;
        }

        CGPDFContextEndPage(ctx);
        CGPDFContextClose(ctx);
        Ok::<(), crate::ffi::PdfError>(())
    };

    // 6) 释放 Core 对象（顺序无所谓，都是 CF/CT 引用）。
    unsafe {
        if !font.is_null() {
            CFRelease(font);
        }
        if !cf_font.is_null() {
            CFRelease(cf_font);
        }
        CGDataConsumerRelease(consumer);
        CFRelease(cf_url);
        CFRelease(cf_path);
    }

    result
}
