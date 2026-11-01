//! PDF 文本层坐标抽取（仅 macOS）。
//!
//! 用系统 **PDFKit** 取每段文字的包围盒，按横排（按 y 聚行）/ 竖排（按 x 聚列）
//! 自适应聚合成 `SourceRegion`，坐标恒为 **PDF 页面点（point，72/inch）**。
//! 与 `collation.rs` 的 `source_size`（页面点尺寸）共用同一单位，显示时由
//! `link_to_paragraph` 按缩放比换算，绝不写死像素。
//!
//! 这是「真实坐标映射」的数据源：载入带文本层的古籍 PDF 时由它填 `regions`。
//! 扫描图 PDF（无文本层）返回空，由调用方提示「需 OCR」。
//!
//! ## 为什么用 `PDFSelection` 而不是 `characterBoundsAtIndex:`
//!
//! 这是本模块最贵的一条经验（v1.9.23）。原先逐字调
//! `PDFPage.characterBoundsAtIndex:`，在我们的矢量导出上拿到的是**残废数据**：
//!
//! * 约七分之一的字返回 3.7×6.1 的退化矩形（正常 24×24）；
//! * 更糟的是**整页后半部分的字返回 (0,0,0,0)**——`characterBoundsAtIndex:` 对
//!   「逐字向下排布」的页面（竖排、书名页、目录）只算前半段，后半段一律给空矩形。
//!   于是「点右文 → 亮左图」在一页里只能命中一半，另一半永远「未匹配到原图区域」。
//!
//! 换 `PDFPage.selectionForRange:` + `PDFSelection.boundsForPage:` 之后，同一份
//! PDF 上 20/20 个字全部拿到准确坐标（实测 500.00 / 791.84 / 33.60 这种整数级
//! 精度，逐字步进 40.00）。`selectionsByLine` 一次还能把整页切好行，省掉 N 次
//! objc 往返。`characterBoundsAtIndex:` 只留作**兜底**（极少数 PDF 上 selection
//! 可能返回空），这样两条路都断的页面才彻底放弃。
//!
//! ⚠️ 旋转页（`/Rotate` 非 0）未做坐标变换，当前按未旋转页空间处理；古籍 PDF
//! 多为 0 旋转，少数旋转页的映射会有偏移，留待后续补旋转矩阵。

use std::os::raw::c_void;
use std::path::Path;

/// 原图文本层的一个文本区域（PDF 页面点坐标，单位 = point）。
///
/// 横排 PDF：一个区域 ≈ 一行；竖排 PDF：一个区域 ≈ 一列。文本为区域原文
/// （匹配时按文字找，不依赖索引对齐）。
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

/// 单页最多处理多少个**文本片段**。
///
/// 本模块必须在主线程跑（PDFKit 不保证线程安全），没法后台化，所以只能把工作量
/// 钉死在上限内：正常一页几十到几百片段，`selectionsByLine` 一遍下来是毫秒级；
/// 整页缩印（一页当四页用）能到上万片段，那时宁可少框几列，也绝不冻住界面。
/// 古籍影印本单页几乎不可能破这条线。
const MAX_GLYPH_SCAN: usize = 6000;

#[cfg(target_os = "macos")]
#[link(name = "PDFKit", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
use objc::{class, msg_send, rc::autoreleasepool, runtime::Object, sel, sel_impl};

#[cfg(target_os = "macos")]
pub fn extract_pdf_text_regions(path: &Path) -> Vec<SourceRegion> {
    extract_pdf_text_regions_on(path, 0)
}

/// 抽取**指定页**（0-based）的文本层坐标；页号越界 / 非 PDF 一律返回空。
///
/// 左右双屏翻页同步后，左侧 PDF 会真的翻页，文本层坐标必须跟着换页——
/// 否则翻到第 5 页仍在用第 1 页的框，高亮会落在完全不相干的地方。
#[cfg(target_os = "macos")]
pub fn extract_pdf_text_regions_on(path: &Path, page_index: usize) -> Vec<SourceRegion> {
    // ⚠️ 必须有 autorelease pool：PDFKit / Foundation 创建的对象（NSURL / NSString /
    // PDFDocument / PDFPage / PDFSelection）多为 autoreleased，脱离主 runloop 的调用点
    // 若没有 pool 承载，对象无处释放会直接崩溃（"导入即闪退"的元凶）。
    autoreleasepool(|| unsafe {
        let nspath = ns_string(path);
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: nspath];
        let doc: *mut Object = msg_send![class!(PDFDocument), alloc];
        let doc: *mut Object = msg_send![doc, initWithURL: url];
        if doc.is_null() {
            return vec![];
        }
        let count: usize = msg_send![doc, pageCount];
        if count == 0 || page_index >= count {
            return vec![];
        }
        let page: *mut Object = msg_send![doc, pageAtIndex: page_index];
        if page.is_null() {
            return vec![];
        }
        // 页面高度（PDF 坐标，y 向上）；用于把包围盒翻转为位图坐标（y 向下）。
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

        let mut chunks = chunks_by_line(page, len);
        if chunks.is_empty() {
            // 兜底：个别 PDF 的 selection 拿不到东西，退回逐字包围盒。
            chunks = chunks_by_char_bounds(page, string, len);
        }
        cluster(chunks, page_h)
    })
}

#[cfg(not(target_os = "macos"))]
pub fn extract_pdf_text_regions(_path: &Path) -> Vec<SourceRegion> {
    vec![]
}

#[cfg(not(target_os = "macos"))]
pub fn extract_pdf_text_regions_on(_path: &Path, _page_index: usize) -> Vec<SourceRegion> {
    vec![]
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NSRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// `NSRange`（location + length，两个 `NSUInteger`）。
#[cfg(target_os = "macos")]
#[repr(C)]
struct NSRange {
    location: usize,
    length: usize,
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

/// 取一个 objc 字符串的 Rust 副本（`NSString` / `CFString` 都吃）。
#[cfg(target_os = "macos")]
unsafe fn ns_to_rust(obj: *mut Object) -> String {
    if obj.is_null() {
        return String::new();
    }
    let utf8: *const std::os::raw::c_char = msg_send![obj, UTF8String];
    if utf8.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
}

/// 一段文字（PDFKit 认作的**一行**，或兜底路径里的**一个字**）。
///
/// 我们的 PDF 是逐字落笔（每个字形一个文本对象），所以一「行」往往就是一个字；
/// 别的软件排的 PDF 一行十几个字，这里就是一整行。两种形态都往后传，由
/// [`cluster`] 统一聚成列 / 行。
#[cfg(target_os = "macos")]
#[derive(Clone)]
struct Chunk {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    text: String,
}

#[cfg(target_os = "macos")]
impl Chunk {
    /// 有没有「可见墨迹」。空串、纯控制符（`\n` `\t`）都算没有。
    fn is_blank(&self) -> bool {
        self.text.chars().all(|c| c.is_control() || c.is_whitespace())
    }

    /// 可见字数（决定能不能对这个小盒子做「补尺寸」）。
    fn visible_len(&self) -> usize {
        self.text
            .chars()
            .filter(|c| !c.is_control() && !c.is_whitespace())
            .count()
    }
}

/// 主路径：`PDFSelection` 按行切 → 每行取 `boundsForPage:`。
///
/// 这是 v1.9.23 的核心修复，理由见模块头的对照说明（`characterBoundsAtIndex:`
/// 在竖排 / 逐字排布的页面上只给前半段坐标，后半段全空）。
#[cfg(target_os = "macos")]
unsafe fn chunks_by_line(page: *mut Object, len: usize) -> Vec<Chunk> {
    let span = len.min(MAX_GLYPH_SCAN);
    let sel: *mut Object = msg_send![page, selectionForRange: NSRange { location: 0, length: span }];
    if sel.is_null() {
        return vec![];
    }
    let lines: *mut Object = msg_send![sel, selectionsByLine];
    if lines.is_null() {
        return vec![];
    }
    let n: usize = msg_send![lines, count];
    let mut out: Vec<Chunk> = Vec::with_capacity(n.min(MAX_GLYPH_SCAN));
    let mut seen = 0usize;
    for i in 0..n {
        if i >= MAX_GLYPH_SCAN || seen >= MAX_GLYPH_SCAN {
            break;
        }
        let l: *mut Object = msg_send![lines, objectAtIndex: i];
        if l.is_null() {
            continue;
        }
        let r: NSRect = msg_send![l, boundsForPage: page];
        if !(r.w > 0.0) || !(r.h > 0.0) {
            continue; // 这一行没有几何（空行 / 删掉的注）
        }
        let text = ns_to_rust(msg_send![l, string]);
        seen += text.chars().count();
        out.push(Chunk {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            text,
        });
    }
    out
}

/// 兜底路径：逐字 `characterBoundsAtIndex:`（含 UTF-16 代理对合并）。
///
/// 已知在竖排 / 逐字排布的页面上**后半段会返回空矩形**，所以只有在
/// `selectionsByLine` 一无所获时才走这里——聊胜于无，能框住前半段也比全无强。
#[cfg(target_os = "macos")]
unsafe fn chunks_by_char_bounds(page: *mut Object, string: *mut Object, len: usize) -> Vec<Chunk> {
    let len = len.min(MAX_GLYPH_SCAN);
    let mut out: Vec<Chunk> = Vec::with_capacity(len);
    let mut i = 0usize;
    while i < len {
        let ch: u16 = msg_send![string, characterAtIndex: i];
        let r: NSRect = msg_send![page, characterBoundsAtIndex: i as isize];
        let cp: u32 = if (0xD800..=0xDBFF).contains(&ch) && i + 1 < len {
            let lo: u16 = msg_send![string, characterAtIndex: i + 1];
            i += 1;
            0x10000 + (((ch - 0xD800) as u32) << 10) + ((lo - 0xDC00) as u32)
        } else {
            ch as u32
        };
        i += 1;
        let text = char::from_u32(cp).map(String::from).unwrap_or_default();
        out.push(Chunk {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            text,
        });
    }
    out
}

/// 中位数（排序取中）。空集返回 0.0，非有限值剔除。
#[cfg(target_os = "macos")]
fn median<I: Iterator<Item = f64>>(it: I) -> f64 {
    let mut v: Vec<f64> = it.filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// 字距推尺寸时的经验比。
///
/// 一个字格 = 「字距（pitch）」的 0.6–0.7 是汉字排版的常态（本项目竖排网格
/// 12 列 / 24 pt 字 → 字距 40、字高 24，比值 0.6）。只有当「好片段的尺寸中位数」
/// 本身不可信（整页都退化）时才用到它。
#[cfg(target_os = "macos")]
const GLYPH_OF_PITCH: f64 = 0.7;

/// 尺寸判据：中位数若不到字距的四成，说明这一页的包围盒整体不可信，改由字距推。
#[cfg(target_os = "macos")]
fn plausible(med: f64, pitch: f64) -> bool {
    med > 0.0 && (pitch <= 0.0 || med >= 0.4 * pitch)
}

/// 把「退化盒子」修回「一个正常字」的大小。
///
/// ## 为什么需要这一步
///
/// 兜底路径（`characterBoundsAtIndex:`）对**我们自己导出的矢量 PDF** 会返回
/// 3.7×6.1 这类退化矩形（正常 24×24）。主路径（selection）已经能绕开它，但别的
/// PDF、别的系统版本未必一直这么友好，所以这一步留着当保险丝。
///
/// ## 为什么能修
///
/// 退化盒子的**尺寸**是假的，但**位置**很规整：实测退化字的 (x, y) 落在真实字格
/// 中心附近，误差 ≤ 1.5 pt（例：`彼` 报 (66.69, 631.50)，该字格是
/// x∈[53.2, 77.3]、y∈[618, 642]，中心 (65.2, 630)）。于是以它为**中心**、把尺寸
/// 换成 `ref_w × ref_h`（相邻片段的中位尺寸，退化时由字距推出），字就回位了。
///
/// 只对「一个可见字」的片段做：多字片段（一整行）窄是正常的，不该被撑开。
#[cfg(target_os = "macos")]
fn repaired(c: Chunk, ref_w: f64, ref_h: f64, thresh: f64) -> Chunk {
    if ref_w <= 0.0 || ref_h <= 0.0 || thresh <= 0.0 || c.visible_len() > 1 {
        return c;
    }
    let mut g = c;
    if g.w < thresh {
        let cx = g.x + g.w * 0.5;
        g.x = cx - ref_w * 0.5;
        g.w = ref_w;
    }
    if g.h < thresh {
        let cy = g.y + g.h * 0.5;
        g.y = cy - ref_h * 0.5;
        g.h = ref_h;
    }
    g
}

/// 把一桶片段压成一个 `SourceRegion`（坐标翻转为位图坐标 y 向下）。
#[cfg(target_os = "macos")]
fn bucket_to_region(b: Vec<Chunk>, page_h: f64, min_ref: f64) -> Option<SourceRegion> {
    let (mut minx, mut miny) = (f64::INFINITY, f64::INFINITY);
    let (mut maxx, mut maxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut text = String::new();
    let mut inked = false;
    for c in b {
        for ch in c.text.chars() {
            if ch.is_control() || ch.is_whitespace() {
                continue;
            }
            text.push(ch);
        }
        // 走到这里才是「真有墨的片段」，才允许它撑包围盒。空白片段（`\n`、空行）
        // 只参与聚类、不参与几何，否则每一列都会凭空多出一格空白。
        if c.is_blank() {
            continue;
        }
        inked = true;
        minx = minx.min(c.x);
        miny = miny.min(c.y);
        maxx = maxx.max(c.x + c.w);
        maxy = maxy.max(c.y + c.h);
    }
    // 整桶只有空白：没有墨就没有框。宁可丢掉，也不要在原图上画一条空线。
    if !inked || maxx <= minx || maxy <= miny {
        return None;
    }
    let (rw, rh) = (maxx - minx, maxy - miny);
    // —— 异常块过滤：行 / 列必须是**细长**的 ——
    // 正常的竖排列窄边 ≈ 一个字宽、横排行窄边 ≈ 一个字高。若**窄边**也超过
    // 4 个字，说明这个桶把多行多列并成了一个大方块（某些 PDF 按文本 run 返回
    // 包围盒时就是这样）。这种框画在原图上完全对不上任何一行，宁可丢掉。
    if min_ref > 0.0 && rw.min(rh) > 4.0 * min_ref {
        return None;
    }
    Some(SourceRegion {
        x: minx as f32,
        y: (page_h - maxy) as f32, // 翻转为位图坐标（y 向下）
        w: rw as f32,
        h: rh as f32,
        text,
    })
}

/// 片段 → 文本区域（一行 / 一列）。
///
/// 四步，每一步都在修一类真实缺陷（全部有实测依据）：
///
/// ① **定方向**：竖排逐字向下推进（Δy 占优）、横排向右推进（Δx 占优）。
/// ② **定尺寸**：先取「尺寸像样」的片段的中位宽高；若整页都退化，则用**字距中位数**
///    反推字格大小。字距是逐字推进量，它比包围盒稳得多。
/// ③ **修退化盒**：小于参考尺寸一半的单字盒子按「中心不变、尺寸归一」重建。
/// ④ **聚类**：按投影分桶（竖排按 x、横排按 y），容差取 0.35 个字；桶键用**均值**
///    而不是首元素，避免首片段退化把整桶带偏。
#[cfg(target_os = "macos")]
fn cluster(chunks: Vec<Chunk>, page_h: f64) -> Vec<SourceRegion> {
    if chunks.is_empty() {
        return vec![];
    }

    // ① 方向。
    let mut sum_dx = 0.0;
    let mut sum_dy = 0.0;
    for w in chunks.windows(2) {
        sum_dx += (w[1].x - w[0].x).abs();
        sum_dy += (w[1].y - w[0].y).abs();
    }
    let vertical = sum_dy > sum_dx;

    // ②a 尺寸基准（第一遍）：只信「有墨且尺寸像样」的片段。
    let usable: Vec<&Chunk> = chunks
        .iter()
        .filter(|c| !c.is_blank() && c.w > 0.0 && c.h > 0.0)
        .collect();
    let sample: Vec<&Chunk> = if usable.is_empty() {
        chunks.iter().collect()
    } else {
        usable
    };
    let med_w = median(sample.iter().map(|c| c.w));
    let med_h = median(sample.iter().map(|c| c.h));

    // ②b 字距：只取「同一行 / 同一列内相邻两段」的推进量（垂直方向偏移很小）。
    let cross_ref = if vertical { med_w } else { med_h };
    let mut deltas: Vec<f64> = Vec::new();
    for w in chunks.windows(2) {
        let (along, cross) = if vertical {
            (w[1].y - w[0].y, w[1].x - w[0].x)
        } else {
            (w[1].x - w[0].x, w[1].y - w[0].y)
        };
        if along.abs() > 0.01 && (cross_ref <= 0.0 || cross.abs() <= 0.5 * cross_ref) {
            deltas.push(along.abs());
        }
    }
    let pitch = median(deltas.iter().copied());

    // ②c 参考尺寸：可信就用中位数，不可信就由字距推。
    let fallback = if pitch > 0.0 { pitch * GLYPH_OF_PITCH } else { 0.0 };
    let ref_w = if plausible(med_w, pitch) { med_w } else { fallback };
    let ref_h = if plausible(med_h, pitch) { med_h } else { fallback };
    let min_ref = [ref_w, ref_h]
        .iter()
        .copied()
        .filter(|v| *v > 0.0)
        .fold(f64::INFINITY, f64::min);
    let min_ref = if min_ref.is_finite() { min_ref } else { 0.0 };

    // ③ 修退化盒（阈值 = 半个参考字）。
    let thresh = min_ref * 0.5;
    let fixed: Vec<Chunk> = chunks
        .into_iter()
        .map(|c| repaired(c, ref_w, ref_h, thresh))
        .collect();

    // ④ 聚类。容差取 0.35 个字：0.6 会把间距偏窄的相邻列/行并成胖块（实测出现过
    // 201×183 的假框），0.35 只认同一列 / 同一行内部的抖动。
    let cross_size = if vertical { ref_w } else { ref_h };
    let tol = (if cross_size > 0.0 { cross_size } else { 1.0 }) * 0.35;
    let mut buckets: Vec<(f64, Vec<Chunk>)> = Vec::new();
    for c in fixed {
        let key = if vertical { c.x } else { c.y };
        let hit = buckets
            .iter()
            .position(|b| (key - b.0).abs() <= tol);
        match hit {
            Some(i) => {
                let b = &mut buckets[i];
                let n = b.1.len() as f64;
                b.0 = (b.0 * n + key) / (n + 1.0); // 均值键：首段退化也不至于带偏整桶
                b.1.push(c);
            }
            None => buckets.push((key, vec![c])),
        }
    }

    // 阅读顺序：竖排列按 x 降序（右→左），横排行按 y 降序（上→下）；
    // 桶内：竖排上→下、横排左→右。
    if vertical {
        buckets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (_, b) in buckets.iter_mut() {
            // y 在 PDF 空间**向上**为正，故「上→下」= y 降序。升序会把整列读反
            // （「天地玄黄」变成「黄玄地天」），字面比对必然失配。
            b.sort_by(|p, q| q.y.partial_cmp(&p.y).unwrap());
        }
    } else {
        // y 在 PDF 空间是**向上**为正，故「上→下」= y 降序。
        buckets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (_, b) in buckets.iter_mut() {
            b.sort_by(|p, q| p.x.partial_cmp(&q.x).unwrap());
        }
    }

    buckets
        .into_iter()
        .filter_map(|(_, b)| bucket_to_region(b, page_h, min_ref))
        .collect()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn ch(x: f64, y: f64, w: f64, h: f64, text: &str) -> Chunk {
        Chunk {
            x,
            y,
            w,
            h,
            text: text.to_string(),
        }
    }

    /// 退化盒子（3.7×6.1，中心在真实字格中心）必须被补回一个字的大小，
    /// 且仍然并进它原来那一列。
    #[test]
    fn degenerate_boxes_are_repaired_into_their_column() {
        // 一列 6 个字：24×24、步进 40；第 3 个是退化盒（中心 = 正确字格中心）。
        let mut v = Vec::new();
        for i in 0..6 {
            let y = 800.0 - i as f64 * 40.0;
            let (w, h, cx, cy) = if i == 3 {
                (3.7, 6.1, 500.0 + 12.0, 800.0 - 3.0 * 40.0 + 12.0)
            } else {
                (24.0, 24.0, 500.0, y)
            };
            let (x, yy) = if i == 3 {
                (cx - w * 0.5, cy - h * 0.5)
            } else {
                (cx, cy)
            };
            v.push(ch(x, yy, w, h, "字"));
        }
        let regions = cluster(v, 842.0);
        assert_eq!(regions.len(), 1, "六个字应聚成一列");
        let r = &regions[0];
        assert_eq!(r.text.chars().count(), 6);
        // 宽 ≈ 一个字（不是 3.7），高 ≈ 5 个步进 + 一个字。
        assert!((r.w - 24.0).abs() < 1.0, "列宽 {} 应约 24", r.w);
        assert!((r.h - (5.0 * 40.0 + 24.0)).abs() < 1.0, "列高 {} 应约 224", r.h);
    }

    /// 整页都退化时，参考尺寸由**字距**推出来，框仍然是「一个字宽的一列」。
    #[test]
    fn all_degenerate_page_falls_back_to_pitch() {
        let mut v = Vec::new();
        for i in 0..8 {
            let y = 800.0 - i as f64 * 40.0;
            v.push(ch(506.0, y + 18.0, 3.7, 6.1, "字")); // 中心落在 x=507.8 / y+...
        }
        let regions = cluster(v, 842.0);
        assert_eq!(regions.len(), 1);
        let r = &regions[0];
        assert!(
            (r.w as f64 - 40.0 * GLYPH_OF_PITCH).abs() < 2.0,
            "列宽 {} 应由字距 40 推出（约 28）",
            r.w
        );
    }

    /// 断行符（PDFKit 给每个字之间插的 `\n`）不能撑高包围盒：一列 5 个字就是
    /// 5 个字的框，不会凭空多出一格。
    #[test]
    fn control_chunks_do_not_inflate_the_box() {
        let mut v = Vec::new();
        for i in 0..5 {
            let y = 800.0 - i as f64 * 40.0;
            v.push(ch(500.0, y, 24.0, 24.0, "字"));
            v.push(ch(500.0, y - 20.0, 24.0, 24.0, "\n"));
        }
        let regions = cluster(v, 842.0);
        assert_eq!(regions.len(), 1);
        let r = &regions[0];
        assert_eq!(r.text.chars().count(), 5, "断行符不进文本");
        assert!(
            (r.h - (4.0 * 40.0 + 24.0)).abs() < 1.0,
            "列高 {} 不该被 \\n 撑成 6 格",
            r.h
        );
    }

    /// 横排 PDF：PDFKit 一行给一个片段，同一行的碎片按 y 聚，不同行绝不串味。
    #[test]
    fn horizontal_lines_stay_separate() {
        let v = vec![
            ch(50.0, 700.0, 300.0, 24.0, "天地玄黃宇宙洪荒"),
            ch(50.0, 660.0, 280.0, 24.0, "日月盈昃辰宿列張"),
            ch(380.0, 700.0, 160.0, 24.0, "日月盈昃"),
        ];
        let regions = cluster(v, 842.0);
        assert_eq!(regions.len(), 2, "两行 → 两个区域，同一行的两段要合并");
        assert_eq!(regions[0].text.chars().count(), 12);
        assert_eq!(regions[1].text.chars().count(), 8);
    }

    /// 空片段（只有 `\n`）聚成的桶直接丢掉，不许在原图上画空线。
    #[test]
    fn blank_only_bucket_is_dropped() {
        let v = vec![
            ch(500.0, 800.0, 24.0, 24.0, "\n"),
            ch(520.0, 800.0, 24.0, 24.0, "\t"),
        ];
        assert!(cluster(v, 842.0).is_empty());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod vector_pdf_tests {
    //! 端到端：**我们自己导出的矢量 PDF** → 文本层 → 区域。
    //!
    //! 这条测试就是 v1.9.23 那个发现的守门人：`characterBoundsAtIndex:` 在竖排
    //! 页面上只给前半段坐标，`PDFSelection` 才给全。若哪天 Apple 改了行为，这里
    //! 会先红。

    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};

    extern "C" {
        fn CGPDFContextCreateWithURL(
            url: *const c_void,
            media: *const CGRect,
            aux: *const c_void,
        ) -> *mut c_void;
        fn CGPDFContextBeginPage(ctx: *mut c_void, info: *const c_void);
        fn CGPDFContextEndPage(ctx: *mut c_void);
        fn CGPDFContextClose(ctx: *mut c_void);
    }

    /// 用 24 pt 宋体、40 pt 步进、x=500 自上而下排一列，模拟笺读的竖排落笔。
    fn vertical_sample_pdf(path: &Path, n: usize) {
        let _ = std::fs::remove_file(path);
        let url = CFURL::from_path(path, false).unwrap();
        let media = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(595.0, 842.0));
        let raw = unsafe {
            CGPDFContextCreateWithURL(
                url.as_concrete_TypeRef() as *const c_void,
                &media,
                std::ptr::null(),
            )
        };
        assert!(!raw.is_null(), "建 PDF 上下文失败");
        let ctx = unsafe { CGContext::from_existing_context_ptr(raw as *mut _) };
        unsafe { CGPDFContextBeginPage(raw, std::ptr::null()) };
        let font = core_text::font::new_from_name("Songti SC", 24.0).unwrap();
        let sample: Vec<char> = "天地玄黃宇宙洪荒日月盈昃辰宿列張寒來暑往秋收冬藏"
            .chars()
            .collect();
        for (i, ch) in sample.iter().take(n).enumerate() {
            let buf = [*ch as u16];
            let mut g: [u16; 2] = [0; 2];
            let ok = unsafe { font.get_glyphs_for_characters(buf.as_ptr(), g.as_mut_ptr(), 1) };
            assert!(ok, "宋体缺字：{ch}");
            // CG 坐标 y 向上：自上而下 = y 递减。
            let y = 800.0 - i as f64 * 40.0;
            let p = [CGPoint::new(500.0, y)];
            let gs = [g[0]];
            font.draw_glyphs(&gs, &p, ctx.clone());
        }
        unsafe { CGPDFContextEndPage(raw) };
        unsafe { CGPDFContextClose(raw) };
        drop(ctx);
    }

    /// 20 个字必须**一个不少**地拿到坐标，且框正好套在这列字上。
    ///
    /// 旧实现（`characterBoundsAtIndex:`）在这里只回 10 个，另外 10 个是 (0,0,0,0)。
    #[test]
    fn own_vector_pdf_yields_every_glyph() {
        let path = std::env::temp_dir().join("jiandu_pdf_text_vertical.pdf");
        let n = 20usize;
        vertical_sample_pdf(&path, n);
        let regions = extract_pdf_text_regions(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(regions.len(), 1, "一列 20 字应聚成一个区域");
        let r = &regions[0];
        assert_eq!(
            r.text.chars().count(),
            n,
            "20 个字一个都不能丢（实际文本：{:?}）",
            r.text
        );
        // x 从 500 起，宽 ≈ 一个字；高 ≈ 19 个步进 + 一个字。
        assert!((r.x - 500.0).abs() < 2.0, "x = {} 应约 500", r.x);
        assert!(r.w > 15.0 && r.w < 40.0, "列宽 {} 应约一个字", r.w);
        let expect_top = 842.0 - (800.0 + 12.0); // 首字中心 800，上沿 812
        assert!(
            (r.y - expect_top).abs() < 40.0,
            "列顶 y = {} 应约 {}",
            r.y,
            expect_top
        );
        assert!(
            r.h > 19.0 * 40.0,
            "列高 {} 应覆盖 19 个步进（不能只是前半个字）",
            r.h
        );
    }
}

