//! PDF 文本层坐标抽取（仅 macOS）。
//!
//! 用系统 **PDFKit**（`PDFDocument` / `PDFPage`）逐字取包围盒，按横排（按 y 聚行）/
//! 竖排（按 x 聚列）自适应聚合成 `SourceRegion`，坐标恒为 **PDF 页面点（point，72/inch）**。
//! 与 `collation.rs` 的 `source_size`（页面点尺寸）共用同一单位，显示时由
//! `link_to_paragraph` 按缩放比换算，绝不写死像素。
//!
//! 这是「真实坐标映射」的数据源：载入带文本层的古籍 PDF 时，用本模块替换掉
//! `collation::mock_regions()` 的假数据。扫描图 PDF（无文本层）返回空，由调用方
//! 决定是否回退到 Mock / 提示需 OCR。
//!
//! ⚠️ 旋转页（/Rotate 非 0）未做坐标变换，当前按未旋转页空间处理；古籍 PDF 多为
//! 0 旋转，少数旋转页的映射会有偏移，留待后续补旋转矩阵。

use std::os::raw::c_void;
use std::path::Path;

/// 原图文本层的一个文本区域（PDF 页面点坐标，单位 = point）。
///
/// 横排 PDF：一个区域 ≈ 一行；竖排 PDF：一个区域 ≈ 一列。文本为区域原文
/// （用于核对 / 状态展示；映射本身按阅读顺序索引，不依赖文本匹配）。
#[derive(Clone)]
pub struct SourceRegion {
    /// 左上角 X（页面点）。
    pub x: f32,
    /// 左上角 Y（页面点，**已翻转为 y 向下**，与栅格化位图坐标系一致）。
    pub y: f32,
    /// 区域宽（页面点）。
    pub w: f32,
    /// 区域高（页面点）。
    pub h: f32,
    /// 该区域识别出的文本。
    pub text: String,
}

#[cfg(target_os = "macos")]
#[link(name = "PDFKit", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
use objc::{class, msg_send, rc::autoreleasepool, runtime::Object, sel, sel_impl};

#[cfg(target_os = "macos")]
pub fn extract_pdf_text_regions(path: &Path) -> Vec<SourceRegion> {
    // ⚠️ 必须有 autorelease pool：PDFKit / Foundation 创建的对象（NSURL / NSString /
    // PDFDocument / PDFPage）多为 autoreleased，脱离主 runloop 的调用点若没有 pool
    // 承载，对象无处释放会直接崩溃（"导入即闪退"的元凶）。即使本函数被主线
    // 程调用，foreground 异步轮询未必有 runloop pool，故显式包裹最稳。
    autoreleasepool(|| unsafe {
        let nspath = ns_string(path);
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: nspath];
        let doc: *mut Object = msg_send![class!(PDFDocument), alloc];
        let doc: *mut Object = msg_send![doc, initWithURL: url];
        if doc.is_null() {
            return vec![];
        }
        let count: usize = msg_send![doc, pageCount];
        if count == 0 {
            return vec![];
        }
        let page: *mut Object = msg_send![doc, pageAtIndex: 0usize];
        if page.is_null() {
            return vec![];
        }
        // 页面高度（PDF 坐标，y 向上）；用于把字符包围盒翻转为位图坐标（y 向下）。
        let mb: NSRect = msg_send![page, boundsForBox: 0usize]; // kPDFDisplayBoxMediaBox
        let page_h = mb.y + mb.h;
        let string: *mut Object = msg_send![page, string];
        if string.is_null() {
            return vec![]; // 扫描图 PDF：无文本层
        }
        let len: usize = msg_send![string, length];
        if len == 0 {
            return vec![];
        }

        // 逐字取包围盒 + 码点（含 UTF-16 代理对合并）。
        let mut chars: Vec<(f64, f64, f64, f64, u32)> = Vec::with_capacity(len);
        let mut i = 0usize;
        while i < len {
            let ch: u16 = msg_send![string, characterAtIndex: i];
            let rect: NSRect = msg_send![page, characterBoundsAtIndex: i as isize];
            let cp: u32 = if (0xD800..=0xDBFF).contains(&ch) && i + 1 < len {
                let lo: u16 = msg_send![string, characterAtIndex: i + 1];
                i += 1;
                0x10000 + (((ch - 0xD800) as u32) << 10) + ((lo - 0xDC00) as u32)
            } else {
                ch as u32
            };
            chars.push((rect.x, rect.y, rect.w, rect.h, cp));
            i += 1;
        }
        cluster(chars, page_h)
    })
}

#[cfg(not(target_os = "macos"))]
pub fn extract_pdf_text_regions(_path: &Path) -> Vec<SourceRegion> {
    vec![]
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct NSRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[cfg(target_os = "macos")]
unsafe fn ns_string(s: &Path) -> *mut Object {
    use std::os::unix::ffi::OsStrExt;
    let bytes = s.as_os_str().as_bytes();
    let cls = class!(NSString);
    let obj: *mut Object = msg_send![cls, alloc];
    let obj: *mut Object = msg_send![obj, initWithBytes: bytes.as_ptr() as *const c_void
        length: bytes.len()
        encoding: 4usize]; // NSUTF8StringEncoding
    obj
}

#[cfg(target_os = "macos")]
unsafe fn cluster(chars: Vec<(f64, f64, f64, f64, u32)>, page_h: f64) -> Vec<SourceRegion> {
    if chars.is_empty() {
        return vec![];
    }
    // 坐标投影分桶：竖排按 x 聚列、横排按 y 聚行。换行符在竖排 PDF 里常被逐字插入，
    // 不可靠，故始终走坐标聚类（阅读顺序已由 `string` 枚举顺序给出）。
    let mut sum_dx = 0.0;
    let mut sum_dy = 0.0;
    for w in chars.windows(2) {
        sum_dx += (w[1].0 - w[0].0).abs();
        sum_dy += (w[1].1 - w[0].1).abs();
    }
    let vertical = sum_dy > sum_dx;
    let mut ws: Vec<f64> = chars.iter().map(|c| c.2).collect();
    let mut hs: Vec<f64> = chars.iter().map(|c| c.3).collect();
    ws.sort_by(|a, b| a.partial_cmp(b).unwrap());
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = if vertical { ws[ws.len() / 2] } else { hs[hs.len() / 2] };
    let tol = med * 0.6;
    let mut buckets: Vec<Vec<(f64, f64, f64, f64, u32)>> = Vec::new();
    for c in chars {
        let key = if vertical { c.0 } else { c.1 };
        let mut placed = false;
        for b in buckets.iter_mut() {
            let bkey = if vertical { b[0].0 } else { b[0].1 };
            if (key - bkey).abs() <= tol {
                b.push(c);
                placed = true;
                break;
            }
        }
        if !placed {
            buckets.push(vec![c]);
        }
    }
    // 阅读顺序：竖排列按 x 降序（右→左），横排行按 y 降序（上→下）；
    // 桶内：竖排上→下、横排左→右。
    if vertical {
        buckets.sort_by(|a, b| b[0].0.partial_cmp(&a[0].0).unwrap());
        for b in buckets.iter_mut() {
            b.sort_by(|a, c| a.1.partial_cmp(&c.1).unwrap());
        }
    } else {
        buckets.sort_by(|a, b| b[0].1.partial_cmp(&a[0].1).unwrap());
        for b in buckets.iter_mut() {
            b.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap());
        }
    }

    let mut regions = Vec::new();
    for b in buckets {
        let mut minx = f64::INFINITY;
        let mut miny = f64::INFINITY;
        let mut maxx = f64::NEG_INFINITY;
        let mut maxy = f64::NEG_INFINITY;
        let mut text = String::new();
        for (x, y, w, h, cp) in b {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x + w);
            maxy = maxy.max(y + h);
            if let Some(ch) = char::from_u32(cp) {
                if !ch.is_control() {
                    text.push(ch);
                }
            }
        }
        if maxx <= minx || maxy <= miny {
            continue;
        }
        regions.push(SourceRegion {
            x: minx as f32,
            y: (page_h - maxy) as f32, // 翻转为位图坐标（y 向下）
            w: (maxx - minx) as f32,
            h: (maxy - miny) as f32,
            text,
        });
    }
    regions
}
