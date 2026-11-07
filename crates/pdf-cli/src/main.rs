//! 笺读 L7 · PDF 流式光栅化 PoC
//!
//! 三约定（用户硬指标）：
//!   1. **强制 Resize 降采样**：长边压到 `--target`（默认 1200px ≈ 把 300dpi 扫描压到 ~150dpi）。
//!   2. **坚决无视自带破碎文本层**：只走 CoreGraphics 光栅化，绝不读 PDFKit 文本层。
//!   3. **逐页释放内存**：光栅 →（可选 OCR 子进程）→ 立刻 drop 位图 + 删临时 PNG，下一页重新分配，绝不累积。
//!
//! 架构：每页独立 `CGPDFDocumentCreateWithURL` 打开→取该页→栅格化到 RGBA 位图→（写盘给 OCR 子进程）→释放。
//! 全程不把整本 PDF 的位图同时驻留内存——正是为了扛住 168 页《資治通鑑》。

use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

#[cfg(target_os = "macos")]
mod raster {
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGAffineTransform, CGPoint, CGRect, CGSize};
    use core_graphics::sys;
    use foreign_types::ForeignType;
    use std::os::raw::c_void;
    use std::path::Path;
    use std::ptr;

    #[repr(C)]
    struct CGBox {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    // ── CoreGraphics PDF 只读 FFI（与 collation::rasterize_source 同一套签名，已验证）──
    extern "C" {
        fn CGPDFDocumentCreateWithURL(url: core_foundation::url::CFURLRef) -> *mut c_void;
        fn CGPDFDocumentGetNumberOfPages(doc: *mut c_void) -> usize;
        fn CGPDFDocumentGetPage(doc: *mut c_void, page_number: usize) -> *mut c_void;
        fn CGPDFPageRetain(page: *mut c_void) -> *mut c_void;
        fn CGPDFPageGetBoxRect(page: *mut c_void, box_: u32) -> CGBox;
        fn CGPDFPageGetRotationAngle(page: *mut c_void) -> i32;
        fn CGPDFPageGetDrawingTransform(
            page: *mut c_void,
            box_: u32,
            rect: CGRect,
            rotate: i32,
            preserve_aspect_ratio: bool,
        ) -> CGAffineTransform;
        fn CGContextConcatCTM(ctx: sys::CGContextRef, t: CGAffineTransform);
        fn CGContextDrawPDFPage(ctx: sys::CGContextRef, page: *mut c_void);
        fn CGPDFDocumentRelease(doc: *mut c_void);
        fn CGPDFPageRelease(page: *mut c_void);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
        ) -> sys::CGContextRef;
    }

    // 降采样上限（与 collation 同口径）：长边 ≤ 4000px；位图 ≤ 200MB。
    const MAX_RASTER_DIM: f64 = 4000.0;
    const MAX_RASTER_BYTES: f64 = 200.0 * 1024.0 * 1024.0;

    /// 由显示尺寸反推画布像素尺寸（不把 MediaBox 的点当像素，避免 300dpi 先降后升变糊）。
    /// `target_px` = 目标**长边**像素数（≈ 控制输出 DPI：本扫描书 MediaBox 即 300dpi 像素值，
    /// 长边 3508px@300dpi → 目标 1754px ≈ 150dpi，2339px ≈ 200dpi）。
    /// 关键：`want` 用 `.min(1.0)` —— 只允许降采样、**绝不升采样**（旧实现 `.max(1.0)` 会卡死在 1.0，
    /// 永远无法把 300dpi 压下去，正是本次 PoC 一开始没缩的原因）。
    fn raster_px(nat_w: f64, nat_h: f64, target_px: f32) -> (usize, usize) {
        let long = nat_w.max(nat_h);
        let want = if target_px.is_finite() && target_px > 0.0 {
            (target_px as f64 / long.max(1.0)).min(1.0)
        } else {
            1.0
        };
        let dim_cap = if long > 0.0 { MAX_RASTER_DIM / long } else { 1.0 };
        let area_cap = {
            let bytes = nat_w * nat_h * 4.0;
            if bytes > 0.0 {
                (MAX_RASTER_BYTES / bytes).sqrt()
            } else {
                1.0
            }
        };
        let scale = want.min(dim_cap).min(area_cap).max(0.01);
        let w = (nat_w * scale).max(1.0).ceil() as usize;
        let h = (nat_h * scale).max(1.0).ceil() as usize;
        (w, h)
    }

    pub struct PageRaster {
        pub img: image::RgbaImage,
        pub nat_w: f64,
        pub nat_h: f64,
        pub page: usize,
        pub total: usize,
    }

    /// 光栅化单页：纯像素，绝不碰文本层。`target_px` 为长边目标像素。
    pub fn rasterize(path: &Path, page_no: usize, target_px: f32) -> Result<PageRaster, String> {
        let url = CFURL::from_path(path, false).ok_or_else(|| format!("无法解析路径：{:?}", path))?;
        let doc = unsafe { CGPDFDocumentCreateWithURL(url.as_concrete_TypeRef()) };
        if doc.is_null() {
            return Err(format!("无法打开 PDF：{:?}", path));
        }
        let total = unsafe { CGPDFDocumentGetNumberOfPages(doc) };
        let page_no = page_no.clamp(1, total.max(1));
        let page = unsafe { CGPDFDocumentGetPage(doc, page_no) };
        // ⚠️ 先 retain 住 page 再 release doc，杜绝 use-after-free。
        if !page.is_null() {
            unsafe { CGPDFPageRetain(page) };
        }
        unsafe { CGPDFDocumentRelease(doc) };
        if page.is_null() {
            return Err(format!("PDF 第 {} 页不存在", page_no));
        }
        let mb = unsafe { CGPDFPageGetBoxRect(page, 0) }; // kCGPDFMediaBox = 0
        let rotation = unsafe { CGPDFPageGetRotationAngle(page) };
        let quarter_turn = rotation.rem_euclid(180) == 90;
        let (nat_w, nat_h) = if quarter_turn { (mb.h, mb.w) } else { (mb.w, mb.h) };
        if nat_w <= 0.0 || nat_h <= 0.0 {
            unsafe { CGPDFPageRelease(page) };
            return Err("PDF 页面尺寸为 0".to_string());
        }
        let (w, h) = raster_px(nat_w, nat_h, target_px);
        let space = CGColorSpace::create_device_rgb();
        // 行距按 16 字节对齐（CoreGraphics 对齐要求）。
        let bpr = ((4 * w + 15) / 16) * 16;
        let raw = unsafe {
            CGBitmapContextCreate(
                ptr::null_mut(),
                w,
                h,
                8,
                bpr,
                space.as_ptr() as *mut c_void,
                // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big
                1 | (4 << 12),
            )
        };
        if raw.is_null() {
            unsafe { CGPDFPageRelease(page) };
            return Err(format!(
                "位图上下文分配失败（{}×{}，可能内存不足）",
                w, h
            ));
        }
        let mut ctx = unsafe { CGContext::from_ptr(raw) };
        // 用官方 GetDrawingTransform 处理 /Rotate + 缩放，绝不手动 Y 翻转。
        let target = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64));
        let t = unsafe { CGPDFPageGetDrawingTransform(page, 0, target, rotation, true) };
        unsafe {
            CGContextConcatCTM(ctx.as_ptr(), t);
            CGContextDrawPDFPage(ctx.as_ptr(), page);
            CGPDFPageRelease(page);
        }
        let bpr_actual = ctx.bytes_per_row();
        let row_bytes = w * 4;
        if bpr_actual < row_bytes {
            return Err("位图行距异常".to_string());
        }
        let data = ctx.data();
        let tight = row_bytes * h;
        let pixels: Vec<u8> = if bpr_actual == row_bytes {
            data.get(..tight)
                .ok_or_else(|| "位图缓冲区偏短".to_string())?
                .to_vec()
        } else {
            let mut v: Vec<u8> = Vec::with_capacity(tight);
            for row in 0..h {
                let s = row * bpr_actual;
                let e = s + row_bytes;
                v.extend_from_slice(data.get(s..e).ok_or_else(|| "位图越界".to_string())?);
            }
            v
        };
        drop(ctx);
        let img = image::RgbaImage::from_raw(w as u32, h as u32, pixels)
            .ok_or_else(|| format!("位图尺寸不符（{}×{}）", w, h))?;
        Ok(PageRaster {
            img,
            nat_w,
            nat_h,
            page: page_no,
            total,
        })
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> Result<()> {
    anyhow::bail!("本 PoC 仅支持 macOS（依赖 CoreGraphics 光栅化）");
}

#[cfg(target_os = "macos")]
fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let mut pdf_path: Option<PathBuf> = None;
    let mut target_px: f32 = 1754.0; // 长边目标像素 ≈ 本 300dpi 扫描的 150dpi；--target 2339 ≈ 200dpi
    let mut limit: Option<usize> = None;
    let mut do_ocr = false;
    let mut keep = false;
    let mut ocr_bin =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/jiandu-ocr");

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                target_px = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1200.0);
                i += 2;
            }
            "--pages" => {
                limit = args.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "--ocr" => {
                do_ocr = true;
                i += 1;
            }
            "--keep" => {
                keep = true;
                i += 1;
            }
            "--ocr-bin" => {
                ocr_bin = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
                i += 2;
            }
            s if !s.starts_with("--") => {
                pdf_path = Some(PathBuf::from(s));
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    let pdf_path = pdf_path.context("用法：pdf-cli <pdf路径> [--target 1200] [--pages N] [--ocr]")?;
    println!("▶ PDF      : {:?}", pdf_path);
    println!(
        "▶ 降采样    : 长边目标 {}px（降采样、绝不升采样）；忽略自带文本层，纯光栅化",
        target_px
    );
    println!(
        "▶ OCR 子进程: {}",
        if do_ocr { "开（逐页独立进程）" } else { "关（加 --ocr 启用）" }
    );
    if !ocr_bin.exists() {
        eprintln!("  ⚠ 未找到 OCR 子进程 {:?}，--ocr 时该页将跳过 OCR", ocr_bin);
    }

    let tmp_dir = std::env::temp_dir().join("jiandu_pdf_poc");
    std::fs::create_dir_all(&tmp_dir)?;
    let aln_dir = tmp_dir.join("alignment"); // 落库目录：每页 OCR JSON 落盘，下一阶段接 L5 AlignmentStore
    std::fs::create_dir_all(&aln_dir).ok();
    let mut total_glyphs: usize = 0;

    let started = Instant::now();
    let mut total: usize = 0;
    let mut limit = limit.unwrap_or(usize::MAX);
    let mut page = 0usize;

    loop {
        page += 1;
        if page > limit {
            break;
        }
        let t0 = Instant::now();
        let r = raster::rasterize(&pdf_path, page, target_px).map_err(|e| anyhow::anyhow!(e))?;
        if total == 0 {
            total = r.total;
            if limit == usize::MAX {
                limit = total;
            }
            println!("▶ 总页数    : {}，本次处理前 {} 页", total, limit);
        }
        let (w, h) = (r.img.width() as usize, r.img.height() as usize);
        let mb = (w * h * 4) as f64 / 1e6;

        // 写临时 PNG（OCR 子进程需读盘；不开 OCR 也落盘便于人工抽检）。
        let png_path = tmp_dir.join(format!("page_{:04}.png", page));
        image::DynamicImage::from(r.img)
            .save(&png_path)
            .context("写 PNG 失败")?;

        let mut glyphs = 0usize;
        if do_ocr && ocr_bin.exists() {
            // L4/L5 同款：逐页独立子进程跑 Vision OCR（绝不在本进程内第二次 performRequests）。
            if let Ok(out) = Command::new(&ocr_bin)
                .arg("vision")
                .arg(&png_path)
                .output()
            {
                if out.status.success() {
                    // 落库：原始 OCR JSON 按页存盘（L5 AlignmentStore 的 SQLite 落库在下一阶段接入）。
                    let _ = std::fs::write(
                        aln_dir.join(format!("page_{:04}.ocr.json", page)),
                        &out.stdout,
                    );
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                        glyphs = v
                            .get("glyphs")
                            .and_then(|g| g.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                    }
                    total_glyphs += glyphs;
                } else {
                    eprintln!("  ⚠ OCR 失败（page {}）", page);
                }
            }
        }

        // ★ 关键：立刻释放本页内存 —— 临时 PNG 删除；r.img 在此轮作用域结束自动 drop（Vec<u8> 释放）。
        //   下一页重新分配位图，绝不累积 → 内存应随页数【不增长】（看活动监视器）。
        if !keep {
            std::fs::remove_file(&png_path).ok();
        }

        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        let pct = page as f64 / limit as f64 * 100.0;
        let filled = (pct / 2.0) as usize;
        let bar: String = (0..50).map(|b| if b < filled { '█' } else { '░' }).collect();
        println!(
            "  [{:>3}/{}] {} {:5.1}%  {}×{}px({}x{}pt) {:6.1}MB  {:6.1}ms{}",
            r.page,
            limit,
            bar,
            pct,
            w,
            h,
            r.nat_w as u32,
            r.nat_h as u32,
            mb,
            dt,
            if do_ocr {
                format!("  glyphs={}", glyphs)
            } else {
                String::new()
            }
        );
    }

    println!(
        "✅ 完成 {} 页，总耗时 {:.1}s；累计落库字形 {} 个",
        limit,
        started.elapsed().as_secs_f64(),
        total_glyphs
    );
    if do_ocr {
        println!("✅ 每页 OCR JSON 已落库：{}", aln_dir.display());
    }
    println!("✅ 每页位图在循环末尾 drop + 临时 PNG 删除：内存应随页数【不增长】——盯着活动监视器看本进程 RSS。");
    Ok(())
}
