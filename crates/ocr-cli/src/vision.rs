//! 笺读 · L4 真实引擎 · macOS Vision FFI（独立 crate，不进 core）
//!
//! 用 `objc2` 直调原生 Vision 框架的 `VNRecognizeTextRequest`，离线、免费、
//! 走 Neural Engine，无需任何模型文件。
//!
//! 依赖只引入 `objc2_foundation::CGRect`（该结构体已由上游正确实现 `Encode`，
//! **绝不手搓**底层几何结构）；Vision 对象全部走 `msg_send!` 直调——这正是
//! `objc2-vision` 封装层的底层做法，稳定且版本无关。
//!
//! ⚠️ 坐标系铁律（用户反复强调）：Vision 返回的归一化坐标 **y 轴原点在左下角**
//! （y 向上），而右栏 / 命中框 / 反向联动全用 **左上角原点**（y 向下）。每个包围盒
//! 都必须做 Y 轴镜像翻转，否则字会画到屏幕外、点字联动错位。
//!
//! 翻转公式（normalized rect：(nx, ny, nw, nh)，图像像素 (W, H)）：
//!   px_x      =  nx                * W
//!   px_w      =  nw                * W
//!   px_y_top  = (1.0 - ny - nh)    * H      ← 仅此处翻转
//!   px_h      =  nh                * H

use anyhow::{anyhow, Context, Result};
use jiandu_core::ocr_provider::{OcrGlyph, OcrInput, OcrProvider, OcrResult};
use objc2::msg_send;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::CGRect;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "macos")]
use image::{Rgba, RgbaImage};

// 链接 Vision 框架（仅声明，符号由系统提供）。
#[link(name = "Vision", kind = "framework")]
unsafe extern "C" {}

/// 把 `&str` 转成 `NSString*`（class 方法 `stringWithUTF8String:`，返回自动释放对象，
/// 在 autoreleasepool 内使用即可）。
unsafe fn ns_string(s: &str) -> *mut AnyObject {
    let cstr = std::ffi::CString::new(s).expect("CString");
    let cls = AnyClass::get("NSString").expect("NSString class");
    msg_send![cls, stringWithUTF8String: cstr.as_ptr()]
}

/// 从 `NSString*` 读出 Rust `String`（UTF8String + CStr）。
unsafe fn ns_to_str(ns: *mut AnyObject) -> String {
    if ns.is_null() {
        return String::new();
    }
    let utf8: *const std::ffi::c_char = msg_send![ns as *mut objc2_foundation::NSString, UTF8String];
    if utf8.is_null() {
        String::new()
    } else {
        CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}

/// 真实 macOS Vision OCR 提供者：实现 core 的 `OcrProvider` 契约。
pub struct VisionOcrProvider {
    /// 识别语言（Vision `recognitionLanguages`），默认中 + 英。
    languages: Vec<String>,
}

impl Default for VisionOcrProvider {
    fn default() -> Self {
        Self {
            languages: vec![
                "zh-Hans".to_string(),
                "zh-Hant".to_string(),
                "en-US".to_string(),
            ],
        }
    }
}

impl VisionOcrProvider {
    /// 自定义语言列表。
    #[allow(dead_code)]
    pub fn with_languages(languages: Vec<String>) -> Self {
        Self { languages }
    }

    /// 把输入解析成本地路径（字节输入先落临时文件）。
    fn to_path(input: &OcrInput) -> Result<PathBuf> {
        match input {
            OcrInput::Path(p) => Ok(p.clone()),
            OcrInput::Bytes(b) => {
                let tmp = std::env::temp_dir().join("jiandu_vision_input.png");
                std::fs::write(&tmp, b).context("写临时图片失败")?;
                Ok(tmp)
            }
        }
    }
}

impl OcrProvider for VisionOcrProvider {
    fn name(&self) -> &str {
        "vision"
    }

    fn recognize(&self, input: &OcrInput) -> Result<OcrResult> {
        let path = Self::to_path(input)?;
        let (w, h) = image::image_dimensions(&path)
            .with_context(|| format!("无法读取图像尺寸：{}", path.display()))?;
        let (w_f, h_f) = (w as f64, h as f64);

        autoreleasepool(|_| unsafe {
            // ── 1. 输入 URL（class 方法 fileURLWithPath:）──
            let url_cls = AnyClass::get("NSURL").ok_or_else(|| anyhow!("NSURL 类缺失"))?;
            let url: *mut AnyObject =
                msg_send![url_cls, fileURLWithPath: ns_string(&path.display().to_string())];
            if url.is_null() {
                return Err(anyhow!("无法构建文件 URL：{}", path.display()));
            }

            // ── 2. 请求 ──
            let req_cls = AnyClass::get("VNRecognizeTextRequest")
                .ok_or_else(|| anyhow!("VNRecognizeTextRequest 不可用（非 macOS？）"))?;
            let req: *mut AnyObject = msg_send![req_cls, new];
            let req = Retained::from_raw(req).context("创建 VNRecognizeTextRequest 失败")?;

            // 0 = VNRequestTextRecognitionLevelAccurate（高精度，非实时场景）
            let _: () = msg_send![&*req, setRecognitionLevel: 0usize];
            let _: () = msg_send![&*req, setUsesLanguageCorrection: true];

            // 语言：NSMutableArray + addObject:（避免变参 arrayWithObjects:）
            if !self.languages.is_empty() {
                let ma_cls = AnyClass::get("NSMutableArray").ok_or_else(|| anyhow!("NSMutableArray 类缺失"))?;
                let langs: *mut AnyObject = msg_send![ma_cls, array];
                for lang in &self.languages {
                    let _: () = msg_send![langs, addObject: ns_string(lang)];
                }
                let _: () = msg_send![&*req, setRecognitionLanguages: langs];
            }

            // ── 3. 处理器（initWithURL:options:，options 用空字典）──
            let h_cls = AnyClass::get("VNImageRequestHandler")
                .ok_or_else(|| anyhow!("VNImageRequestHandler 不可用（非 macOS？）"))?;
            let dict_cls = AnyClass::get("NSDictionary").ok_or_else(|| anyhow!("NSDictionary 类缺失"))?;
            let options: *mut AnyObject = msg_send![dict_cls, dictionary];
            let h_alloc: *mut AnyObject = msg_send![h_cls, alloc];
            let h_raw: *mut AnyObject =
                msg_send![h_alloc, initWithURL: url, options: options];
            let handler = Retained::from_raw(h_raw).context("创建 VNImageRequestHandler 失败")?;

            // ── 4. 执行 ──
            let nsarr_cls = AnyClass::get("NSArray").ok_or_else(|| anyhow!("NSArray 类缺失"))?;
            let requests: *mut AnyObject = msg_send![nsarr_cls, arrayWithObject: &*req];
            let mut error: *mut objc2_foundation::NSError = std::ptr::null_mut();
            let ok: bool =
                msg_send![&*handler, performRequests: requests, error: &mut error];
            if !ok {
                return Err(anyhow!("Vision performRequests 失败"));
            }

            // ── 5. 取结果 ──
            let results: *mut AnyObject = msg_send![&*req, results];
            let mut text = String::new();
            let mut glyphs: Vec<OcrGlyph> = Vec::new();
            let mut char_index = 0usize;

            if !results.is_null() {
                let count: usize = msg_send![results, count];
                for i in 0..count {
                    let obs: *mut AnyObject = msg_send![results, objectAtIndex: i];

                    // 最佳候选文本（topCandidates(1)）
                    let cands: *mut AnyObject = msg_send![obs, topCandidates: 1usize];
                    let cand_count: usize = if cands.is_null() { 0 } else { msg_send![cands, count] };
                    if cand_count == 0 {
                        continue;
                    }
                    let cand: *mut AnyObject = msg_send![cands, objectAtIndex: 0usize];
                    let line_text = ns_to_str(msg_send![cand, string]);
                    if line_text.trim().is_empty() {
                        continue;
                    }
                    let conf: f32 = msg_send![cand, confidence];

                    // 逐字框：优先 elements()（macOS 11+），但老 SDK 可能不响应
                    // 该选择器——先用 respondsToSelector: 探测，没有就退回整行 boundingBox。
                    let has_elements: bool = msg_send![cand, respondsToSelector: objc2::sel!(elements)];
                    let elements: *mut AnyObject = if has_elements {
                        msg_send![cand, elements]
                    } else {
                        std::ptr::null_mut()
                    };
                    let mut elems: Vec<(*mut AnyObject, String)> = Vec::new();
                    if !elements.is_null() {
                        let ec: usize = msg_send![elements, count];
                        for e in 0..ec {
                            let elem: *mut AnyObject = msg_send![elements, objectAtIndex: e];
                            let estr = ns_to_str(msg_send![elem, string]);
                            elems.push((elem, estr));
                        }
                    }
                    if elems.is_empty() {
                        // 退回：整行作为一个元素（哨兵用空指针 → 走 obs.boundingBox）
                        elems.push((std::ptr::null_mut(), line_text.clone()));
                    }

                    for (elem, estr) in &elems {
                        // bounds / boundingBox 返回 CGRect（归一化，左下角原点）
                        let rect: CGRect = if elem.is_null() {
                            msg_send![obs, boundingBox]
                        } else {
                            msg_send![*elem, bounds]
                        };
                        let chars: Vec<char> = estr.chars().collect();
                        let n = chars.len().max(1);
                        for (k, &ch) in chars.iter().enumerate() {
                            if ch.is_whitespace() {
                                text.push(ch);
                                char_index += 1;
                                continue;
                            }
                            // 归一化框（左下角原点），按字符数均分宽度
                            let nx = rect.origin.x + rect.size.width * (k as f64) / (n as f64);
                            let ny = rect.origin.y;
                            let nw = rect.size.width / (n as f64);
                            let nh = rect.size.height;
                            // ⚠️ Y 轴镜像翻转 → 左上角原点（像素）
                            let px = (nx * w_f) as f32;
                            let py = ((1.0 - ny - nh) * h_f) as f32;
                            let pw = (nw * w_f) as f32;
                            let ph = (nh * h_f) as f32;
                            glyphs.push(OcrGlyph {
                                char_index,
                                ch: ch.to_string(),
                                x: px,
                                y: py,
                                w: pw,
                                h: ph,
                                confidence: conf,
                            });
                            text.push(ch);
                            char_index += 1;
                        }
                    }
                }
            }

            Ok(OcrResult {
                image_width: w,
                image_height: h,
                text,
                glyphs,
                provider: "vision".to_string(),
            })
        })
    }
}

/// 独立 CLI 入口（macOS）：把一张本地图片（或整目录）喂给 Vision，产出与
/// `app.rs::load_compare_json` 兼容的坐标 JSON，并可叠加一张「红框 overlay」PNG
/// 用于人工核验坐标质量（弯曲书脊 / 倾斜扫描等复杂排版）。
///
/// - 单图模式：`vision <图片> [-o json] [--overlay png]`
/// - 批量模式：`vision <目录>` → 目录内每张图产出 `<名>.json` + `<名>._overlay.png`
///   并汇总越界数 / y 区间等统计，便于一次性扫一堆扫描页。
pub fn run_vision_cli(input: &Path, output: Option<&Path>, overlay: Option<&Path>) -> Result<()> {
    if input.is_dir() {
        return run_vision_dir(input);
    }
    let provider = VisionOcrProvider::default();
    let result = provider.recognize(&OcrInput::from_path(input))?;
    let store = jiandu_core::ocr_provider::AlignmentStore::from_ocr(result);
    let pretty = serde_json::to_string_pretty(&store.to_compare_json())?;
    match output {
        Some(p) => {
            std::fs::write(p, &pretty).with_context(|| format!("写文件失败：{}", p.display()))?;
            eprintln!("✓ Vision OCR 完成：{} 字，写出 {}", store.glyphs.len(), p.display());
        }
        None => {
            println!("{}", pretty);
        }
    }
    if let Some(ov) = overlay {
        draw_overlay(input, &store, ov)?;
        eprintln!("✓ overlay 写出 {}", ov.display());
    }
    let (oob, ymin, ymax) = glyph_stats(&store);
    eprintln!(
        "  provider=vision  image={}x{}  glyphs={}  oob={}  y∈[{:.0},{:.0}]",
        store.image_width, store.image_height, store.glyphs.len(), oob, ymin, ymax
    );
    Ok(())
}

/// 批量目录模式：取一层目录内的图像文件，逐张产出 JSON + 红框 overlay，并打印统计。
/// 批量目录模式：取一层目录内的图像文件，逐张产出 JSON + 红框 overlay，并打印统计。
///
/// ⚠️ 进程隔离（L4 收尾加固）：苹果 Vision 在**同一进程内第二个** `performRequests`
/// 必现失败（底层 GPU/ANE 状态跨调用不释放），而**单进程单图** 100% 成功。因此批量
/// 逐张 fork 独立子进程（即 `jiandu-ocr vision <单图>`），父进程只收集结果落库。
/// 这是确定性的「逐个处理并释放」，不依赖任何玄学重试。子进程若不可用，则回退到
/// 进程内单图识别（同一进程只跑这一张，成功率同样高）。
fn run_vision_dir(dir: &Path) -> Result<()> {
    let provider = VisionOcrProvider::default();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("无法读取目录：{}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| is_image_ext(p))
        .collect();
    entries.sort();
    eprintln!(
        "▶ 批量 Vision OCR：{} 共 {} 张图（逐张独立子进程隔离）",
        dir.display(),
        entries.len()
    );
    // L5 落库：把每张图的对齐仓库按文件名 stem 落进 <dir>/.jiandu_align/ 目录库，
    // 便于整批扫描后按 key 重载 / 检索（纯 core，ocr-cli 仅调用，不破纯洁红线）。
    let lib = jiandu_core::ocr_provider::AlignmentLibrary::open(dir.join(".jiandu_align"))?;
    // 当前可执行文件自身作为子进程镜像（单图模式会写 JSON + overlay 后退出）。
    let exe = std::env::current_exe().ok();

    let mut total = 0usize;
    let mut failed = 0usize;
    for p in &entries {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
        let json_path = dir.join(format!("{}.json", stem));
        let ov_path = dir.join(format!("{}._overlay.png", stem));

        // ── 主路径：独立子进程跑单图（Vision 跨进程状态彻底隔离）──
        let ran_subprocess = match &exe {
            Some(exe) => matches!(
                Command::new(exe)
                    .arg("vision")
                    .arg(p)
                    .arg("-o")
                    .arg(&json_path)
                    .arg("--overlay")
                    .arg(&ov_path)
                    .status(),
                Ok(s) if s.success()
            ),
            None => false,
        };

        if ran_subprocess {
            // 子进程已写好 JSON + overlay；父进程读回用于落库与统计。
            match std::fs::read_to_string(&json_path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| jiandu_core::ocr_provider::AlignmentStore::from_compare_json(&v))
            {
                Some(mut store) => {
                    store.source_image = Some(p.display().to_string());
                    total += store.glyphs.len();
                    if let Err(e) = lib.insert(stem, &store) {
                        eprintln!("  ⚠️ {} 落库失败（不影响 JSON/overlay）：{:#}", stem, e);
                    }
                    let (oob, ymin, ymax) = glyph_stats(&store);
                    eprintln!(
                        "  ✓ {} → {} 字, oob={}, y∈[{:.0},{:.0}] (+overlay)",
                        stem, store.glyphs.len(), oob, ymin, ymax
                    );
                }
                None => {
                    eprintln!("  ⚠️ {} 子进程成功但 JSON 读回失败（跳过）", stem);
                    failed += 1;
                }
            }
        } else {
            // ── 回退：进程内单图识别（该进程只跑这一张，规避同进程多 handler 的坑）──
            match provider.recognize(&OcrInput::from_path(p)) {
                Ok(r) => {
                    let mut store = jiandu_core::ocr_provider::AlignmentStore::from_ocr(r);
                    store.source_image = Some(p.display().to_string());
                    total += store.glyphs.len();
                    std::fs::write(
                        &json_path,
                        serde_json::to_string_pretty(&store.to_compare_json())?,
                    )?;
                    draw_overlay(p, &store, &ov_path)?;
                    if let Err(e) = lib.insert(stem, &store) {
                        eprintln!("  ⚠️ {} 落库失败（不影响 JSON/overlay）：{:#}", stem, e);
                    }
                    let (oob, ymin, ymax) = glyph_stats(&store);
                    eprintln!(
                        "  ✓ {} → {} 字, oob={}, y∈[{:.0},{:.0}] (+overlay 回退)",
                        stem, store.glyphs.len(), oob, ymin, ymax
                    );
                }
                Err(e) => {
                    eprintln!("  ⚠️ {} 识别失败（跳过）：{:#}", stem, e);
                    failed += 1;
                }
            }
        }
    }
    let keys = lib.list().unwrap_or_default();
    eprintln!(
        "✅ 批量完成：{} 张图，成功 {} 张，跳过 {} 张，共 {} 字",
        entries.len(),
        entries.len() - failed,
        failed,
        total
    );
    eprintln!(
        "📚 对齐库已落库 {} 条 → {}/.jiandu_align/",
        keys.len(),
        dir.display()
    );
    Ok(())
}

/// 常见扫描图扩展名（批量模式据此挑文件）。
fn is_image_ext(p: &Path) -> bool {
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp" | "webp" | "gif"
    )
}

/// 统计越界字数与 y 区间（左上角原点）。oob=0 且 y 区间合理即说明 Y 翻转正确。
fn glyph_stats(store: &jiandu_core::ocr_provider::AlignmentStore) -> (u32, f32, f32) {
    let (w_f, h_f) = (store.image_width as f32, store.image_height as f32);
    let mut oob = 0u32;
    let mut ymin = f32::MAX;
    let mut ymax = f32::MIN;
    for g in &store.glyphs {
        if g.x < 0.0 || g.y < 0.0 || g.x + g.w > w_f || g.y + g.h > h_f {
            oob += 1;
        }
        ymin = ymin.min(g.y);
        ymax = ymax.max(g.y + g.h);
    }
    if ymin == f32::MAX {
        ymin = 0.0;
        ymax = 0.0;
    }
    (oob, ymin, ymax)
}

/// 在原图上叠加红色字形包围盒，存为 PNG，用于人工核验坐标是否贴合字形。
/// 这是判断「坐标精度」最直观的依据：弯曲书脊 / 倾斜扫描下，看红框是否仍钉在
/// 各自字符上（Vision 给的是轴对齐框，不随文字旋转，倾斜页会显露出基线错位）。
fn draw_overlay(
    image_path: &Path,
    store: &jiandu_core::ocr_provider::AlignmentStore,
    out_path: &Path,
) -> Result<()> {
    let img = image::open(image_path)
        .with_context(|| format!("无法打开原图绘制 overlay：{}", image_path.display()))?
        .to_rgba8();
    let (w_img, h_img) = (img.width() as i64, img.height() as i64);
    let mut canvas = img;
    let red = Rgba([255u8, 0u8, 0u8, 255u8]);
    for g in &store.glyphs {
        draw_rect(
            &mut canvas,
            g.x.round() as i64,
            g.y.round() as i64,
            g.w.round() as i64,
            g.h.round() as i64,
            &red,
            w_img,
            h_img,
        );
    }
    canvas
        .save(out_path)
        .with_context(|| format!("保存 overlay 失败：{}", out_path.display()))?;
    Ok(())
}

/// 画 2px 红色描边矩形（clamp 到图像内，绝不越界写入）。
fn draw_rect(
    img: &mut RgbaImage,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    color: &image::Rgba<u8>,
    w_img: i64,
    h_img: i64,
) {
    let x0 = x.max(0).min(w_img - 1);
    let y0 = y.max(0).min(h_img - 1);
    let x1 = (x + w).max(0).min(w_img - 1);
    let y1 = (y + h).max(0).min(h_img - 1);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let t = 2i64;
    for xx in x0..=x1 {
        for dy in 0..t {
            if y0 + dy < h_img {
                img.put_pixel(xx as u32, (y0 + dy) as u32, *color);
            }
            if y1 - dy >= 0 {
                img.put_pixel(xx as u32, (y1 - dy) as u32, *color);
            }
        }
    }
    for yy in y0..=y1 {
        for dx in 0..t {
            if x0 + dx < w_img {
                img.put_pixel((x0 + dx) as u32, yy as u32, *color);
            }
            if x1 - dx >= 0 {
                img.put_pixel((x1 - dx) as u32, yy as u32, *color);
            }
        }
    }
}
