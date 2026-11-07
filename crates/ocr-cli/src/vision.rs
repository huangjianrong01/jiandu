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

/// 独立 CLI 入口：把一张本地图片喂给 Vision，打印与 `app.rs::load_compare_json`
/// 兼容的坐标 JSON（图像维度 + 逐字 x/y/w/h/target/confidence）。
pub fn run_vision_cli(image: &Path, output: Option<&Path>) -> Result<()> {
    let provider = VisionOcrProvider::default();
    let result = provider.recognize(&OcrInput::from_path(image))?;
    let store = jiandu_core::ocr_provider::AlignmentStore::from_ocr(result);
    let json = store.to_compare_json();
    let pretty = serde_json::to_string_pretty(&json)?;
    match output {
        Some(p) => {
            std::fs::write(p, &pretty).with_context(|| format!("写文件失败：{}", p.display()))?;
            eprintln!(
                "✓ Vision OCR 完成：{} 字，写出 {}",
                store.glyphs.len(),
                p.display()
            );
        }
        None => {
            println!("{}", pretty);
        }
    }
    eprintln!(
        "  provider=vision  image={}x{}  glyphs={}",
        store.image_width, store.image_height, store.glyphs.len()
    );
    Ok(())
}
