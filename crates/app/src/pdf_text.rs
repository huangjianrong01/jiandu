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
#[derive(Clone, Debug)]
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

/// 单页最多处理多少个**字符**（含空白；防病态输入烧 CPU 的总闸门）。
///
/// ⚠️ 它**不是**精度上限 —— v1.9.31 之前它是，于是整页缩印（一页当四页印、
/// 上万字）的后半页永远拿不到坐标：「点右文不亮左图」，而且毫无提示。
/// 现在真正的精度由 [`SCAN_SEGMENT`] 分段保证，这个数只是「一页再离谱也不
/// 可能超过」的护栏（A4 300dpi 双栏密排约 1.2 万字，留一倍余量）。
const MAX_GLYPH_SCAN: usize = 24000;

/// 单次 `selectionForRange:` 扫多少字符。
///
/// PDFKit 对超长 range 的 `selectionsByLine` 在部分文档上只回前半段（实测约
/// 6 千字开始截）。切到 1500 字一段，每段都短到不会触发，拼起来就是整页。
/// 分段会把同一行切成前后两半，由 [`push_merged`] 按几何合一。
const SCAN_SEGMENT: usize = 1500;

#[cfg(target_os = "macos")]
#[link(name = "PDFKit", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
use objc::{class, msg_send, rc::autoreleasepool, runtime::Object, sel, sel_impl};

/// 串行化所有 PDFKit 调用的进程级锁。
///
/// 为什么需要：v1.9.31 起「当前页」与「下一页预取」会**同时**在后台跑，各自
/// 开自己的 `PDFDocument`。私有实例之间数据不共享，但 PDFKit 内部的字体注册、
/// 页面缓存、字形解析都是进程级状态，苹果从未承诺多实例并发安全。这把锁的代价
/// 是一次 10–40ms 的排队，换的是「绝不因为并发翻页而崩」——完全不值得赌。
///
/// 注意：只锁 PDFKit / PDFSelection 这条路径，**不锁** `rasterize_source` 的
/// `CGPDFDocument`（那是 CoreGraphics，独立实例并发绘制是安全的，且栅格化
/// 12ms 里并行度最值钱）。
#[cfg(target_os = "macos")]
static PDFKIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "macos")]
pub fn extract_pdf_text_regions(path: &Path) -> Vec<SourceRegion> {
    extract_pdf_text_regions_on(path, 0)
}

/// 抽取**指定页**（0-based）的文本层坐标；页号越界 / 非 PDF 一律返回空。
///
/// 左右双屏翻页同步后，左侧 PDF 会真的翻页，文本层坐标必须跟着换页——
/// 否则翻到第 5 页仍在用第 1 页的框，高亮会落在完全不相干的地方。
///
/// # 线程模型（v1.9.31）
///
/// **可以后台调用**：本函数每次都 `alloc/init` 一个自己的 `PDFDocument`，
/// 页、字符串、选择集全部在这棵私有对象树里，不共享任何可变状态；内部的
/// `autoreleasepool` 负责回收。以前它排在主线程，翻页时那 10–40ms 直接压在
/// 输入响应上；现在与栅格化同赴后台（见 `collation::render_pdf_page`）。
/// ⚠️ 若将来有人改成复用同一个 `PDFDocument` 实例，这条就立刻不成立了。
///
/// 「各用各的实例」只保证**我们的**数据不共享；PDFKit 内部还有字体、页面缓存
/// 这类进程级状态，苹果从没承诺过多实例并发安全。所以整段仍串在
/// [`PDFKIT_LOCK`] 上——单次 10–40ms，串行化换来的是「绝不因为同时开两页而崩」。
#[cfg(target_os = "macos")]
pub fn extract_pdf_text_regions_on(path: &Path, page_index: usize) -> Vec<SourceRegion> {
    // 中毒不影响正确性：只说明某个线程在临界区里 panic 过，数据本身没坏。
    let _kit = PDFKIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

        let mut chunks = chunks_by_line(page, string, len);
        if chunks.is_empty() {
            // 兜底：个别 PDF 的 selection 拿不到东西，退回逐字包围盒。
            chunks = chunks_by_char_bounds(page, string, len);
        }
        // ⚠️ 顺序关键：**先**在未旋转坐标里聚类，**再**整体旋转。
        //    `cluster` 靠 Δx/Δy 判竖排还是横排；旋转 90° 的页若先转坐标，
        //    Δx 与 Δy 的角色会互换，竖排古籍会被当成横排去按 y 分桶，整页糊成一团。
        //    未旋转坐标才是内容的**原始**排布方向，判断必须在那里做。
        let regions = cluster(chunks, page_h);
        // 图被 `CGPDFPageGetDrawingTransform` 按 /Rotate 转正了，坐标必须跟着转；
        // 不转就是「图正着、框躺着」，高亮整片错位。
        let rotation: i64 = msg_send![page, rotation];
        let rotation = rotation as i32;
        if rotation.rem_euclid(360) == 0 {
            regions
        } else {
            rotate_regions(regions, rotation, (mb.x + mb.w) as f32, page_h as f32)
        }
    })
}

/// 把文本层区域从**未旋转**页面坐标（y 向下）变换到**显示**坐标。
///
/// `rotation` 是 PDF 的 `/Rotate`（顺时针度数，0/90/180/270）；`w` / `h` 是
/// **未旋转**的页面尺寸（点）。90 / 270 时显示宽高互换，矩形本身也要跟着换 ——
/// 只挪位置不改宽高，框就会变成横躺的细条。
///
/// 为什么必须做：`PDFSelection.boundsForPage:` 恒返回**未旋转**坐标，而
/// `CGPDFPageGetDrawingTransform` 会按 `/Rotate` 把**位图**转正。两边差一次
/// 旋转，高亮就整片错位——扫描件里横放的插页、为省纸旋转排版的内页全中。
///
/// 坐标约定与 `cluster` 一致：原点 (0,0) 在页面左上，y 向下。
fn rotate_regions(
    mut regions: Vec<SourceRegion>,
    rotation: i32,
    w: f32,
    h: f32,
) -> Vec<SourceRegion> {
    match rotation.rem_euclid(360) {
        // 顺时针 90°：(x, y) → (h - y - rh, x)，宽高互换。
        90 => {
            for r in &mut regions {
                let (x, y) = (r.x, r.y);
                r.x = h - y - r.h;
                r.y = x;
                std::mem::swap(&mut r.w, &mut r.h);
            }
        }
        // 顺时针 180°：中心对称，宽高不变。
        180 => {
            for r in &mut regions {
                r.x = w - r.x - r.w;
                r.y = h - r.y - r.h;
            }
        }
        // 顺时针 270°（= 逆时针 90°）：(x, y) → (y, w - x - rw)，宽高互换。
        270 => {
            for r in &mut regions {
                let (x, y) = (r.x, r.y);
                r.x = y;
                r.y = w - x - r.w;
                std::mem::swap(&mut r.w, &mut r.h);
            }
        }
        // 0 / 非法值：原样返回（非法值按「不旋转」处理，绝不把框转飞）。
        _ => {}
    }
    regions
}

#[cfg(not(target_os = "macos"))]
pub fn extract_pdf_text_regions(_path: &Path) -> Vec<SourceRegion> {
    vec![]
}

#[cfg(not(target_os = "macos"))]
pub fn extract_pdf_text_regions_on(_path: &Path, _page_index: usize) -> Vec<SourceRegion> {
    vec![]
}

/// PDF 总页数（供 UI 预检 / 进度条 / 翻页边界用）。
///
/// macOS 走 PDFKit；其余平台按 `#[cfg]` 分派——Windows 预留 `pdfium-render`
/// 接入点（当前返回 0，待接 pdfium 后填上真实页数），其余（Linux / iOS / Android
/// 等暂未接 PDF 后端的平台）统一返回 0，绝不 panic。
#[cfg(target_os = "macos")]
pub fn pdf_page_count(path: &Path) -> usize {
    autoreleasepool(|| unsafe {
        let nspath = ns_string(path);
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: nspath];
        let doc: *mut Object = msg_send![class!(PDFDocument), alloc];
        let doc: *mut Object = msg_send![doc, initWithURL: url];
        if doc.is_null() {
            return 0;
        }
        let count: usize = msg_send![doc, pageCount];
        count
    })
}

#[cfg(target_os = "windows")]
pub fn pdf_page_count(_path: &Path) -> usize {
    // TODO(Windows): 接入 `pdfium-render` 取页数（当前未接后端，返回 0 表示未知）。
    0
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub fn pdf_page_count(_path: &Path) -> usize {
    0
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
#[derive(Clone, Debug)]
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
///
/// v1.9.31 起按 [`SCAN_SEGMENT`] **分段扫描**：一次 `selectionForRange:` 只吃
/// 1500 字，短到 PDFKit 不会截断，循环到最后拼成整页。
#[cfg(target_os = "macos")]
unsafe fn chunks_by_line(page: *mut Object, string: *mut Object, len: usize) -> Vec<Chunk> {
    let total = len.min(MAX_GLYPH_SCAN);
    let mut out: Vec<Chunk> = Vec::new();
    let mut start = 0usize;
    while start < total {
        // 切点绝不落在 UTF-16 代理对中间：NSRange 以 UTF-16 计，切在低位代理上
        // 会让这一段的尾巴变成一个孤零零的半字（Ext-B 生僻字才会遇到，但代价只有
        // 一次 objc 往返，值）。
        while start > 0 && start < total && is_low_surrogate(string, start) {
            start += 1;
        }
        if start >= total {
            break;
        }
        let span = (total - start).min(SCAN_SEGMENT);
        let sel: *mut Object =
            msg_send![page, selectionForRange: NSRange { location: start, length: span }];
        start += span;
        if sel.is_null() {
            continue;
        }
        let lines: *mut Object = msg_send![sel, selectionsByLine];
        if lines.is_null() {
            continue;
        }
        let n: usize = msg_send![lines, count];
        for i in 0..n {
            let l: *mut Object = msg_send![lines, objectAtIndex: i];
            if l.is_null() {
                continue;
            }
            let r: NSRect = msg_send![l, boundsForPage: page];
            if !(r.w > 0.0) || !(r.h > 0.0) {
                continue; // 这一行没有几何（空行 / 删掉的注）
            }
            let text = ns_to_rust(msg_send![l, string]);
            push_merged(
                &mut out,
                Chunk {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    text,
                },
            );
        }
    }
    out
}

/// `i` 处的 UTF-16 单元是否是代理对的后半（低位代理）。
#[cfg(target_os = "macos")]
unsafe fn is_low_surrogate(string: *mut Object, i: usize) -> bool {
    let c: u16 = msg_send![string, characterAtIndex: i];
    (0xDC00..=0xDFFF).contains(&c)
}

/// 追加一个片段；若它与上一段是**同一行被分段切开的两个半截**，就并回去。
///
/// 判据只用几何：包围盒四边差都 < 1pt。分段切点只会把一行切成前后两半，两半
/// 取自同一行，`boundsForPage:` 必然一致；而同一页上真正相邻的两行（竖排为
/// 相邻两列）间距是字号量级（≥ 10pt），不可能误并。
///
/// 为什么必须并：不并的话每 1500 字就多出一个「半行」，右侧联动定位时这一行
/// 的文字只剩一半，`locate_region` 按文字匹配会匹配不上 —— 分段救了后半页的
/// 坐标，却可能弄坏前半页的匹配。这条合并是配套的。
fn push_merged(out: &mut Vec<Chunk>, c: Chunk) {
    const SAME_LINE_EPS: f64 = 1.0;
    if let Some(last) = out.last_mut() {
        let same = (last.x - c.x).abs() < SAME_LINE_EPS
            && (last.y - c.y).abs() < SAME_LINE_EPS
            && (last.w - c.w).abs() < SAME_LINE_EPS
            && (last.h - c.h).abs() < SAME_LINE_EPS;
        if same {
            last.text.push_str(&c.text);
            return;
        }
    }
    out.push(c);
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

/// 康熙部首（U+2F00–U+2FDF）与对应标准 CJK 一字一画相同，但码位不同。
/// Core Text 经 `draw_glyphs` 写出的 PDF，PDFKit 抽取时常用康熙部首码位回填
/// （如 `非`→`⾮` U+2F8F、`老`→`⽼` U+2F54、`子`→`⼦` U+2F5B）。这会让后续
/// 比对 / 清洗因为码位不等而失配。两者映射是 1:1 的：康熙码位 − 0x2F00 + 0x4E00
/// 即标准 CJK。抽取阶段统一折回，外部无需再关心。
#[cfg(target_os = "macos")]
fn fold_kangxi(c: char) -> char {
    let u = c as u32;
    if (0x2F00..=0x2FDF).contains(&u) {
        char::from_u32(u - 0x2F00 + 0x4E00).unwrap_or(c)
    } else {
        c
    }
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
            text.push(fold_kangxi(ch));
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

/// 手写一个最小 PDF，`rotate` 控制页面字典里的 `/Rotate`。
///
/// 为什么不用 `render_pdf_blocking`：那条路产出的样张**没有 /Rotate**，
/// 而「旋转页坐标」这条回归必须要有一份真带 /Rotate 的 PDF 才能验。
/// xref 偏移逐对象累加算出（内容全 ASCII，`str::len()` 就是字节数）。
#[cfg(all(test, target_os = "macos"))]
pub(crate) fn tiny_pdf(rotate: i32) -> Vec<u8> {
    let content = "BT /F1 24 Tf 50 300 Td (Hello) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Rotate {rotate} \
             /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            content
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{}\nendobj\n", i + 1, body));
    }
    let xref_at = out.len();
    out.push_str(&format!("xref\n0 {}\n", objs.len() + 1));
    out.push_str("0000000000 65535 f \n");
    for off in &offsets {
        out.push_str(&format!("{:010} 00000 n \n", off));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        objs.len() + 1,
        xref_at
    ));
    out.into_bytes()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use super::tiny_pdf;

    fn ch(x: f64, y: f64, w: f64, h: f64, text: &str) -> Chunk {
        Chunk {
            x,
            y,
            w,
            h,
            text: text.to_string(),
        }
    }

    /// 分段扫描把一行切成前后两半时必须合回去。
    ///
    /// 这正是 v1.9.31 分段方案的前置条件：不合的话，每 1500 字就多出一个只有
    /// 半截文字的「行」，`locate_region` 按文字匹配会认不出来 —— 坐标救回来了，
    /// 匹配却又坏了。两个功能必须一起上。
    #[test]
    fn split_line_halves_are_merged_back() {
        let mut out = Vec::new();
        push_merged(&mut out, ch(500.0, 800.0, 24.0, 600.0, "甲甲甲"));
        push_merged(&mut out, ch(500.0, 800.0, 24.0, 600.0, "乙乙"));
        assert_eq!(out.len(), 1, "同一行被切成两段，必须合并：{out:?}");
        assert_eq!(out[0].text, "甲甲甲乙乙", "合并后文字应首尾相接");
    }

    /// 相邻两列（竖排）只差一个字距：绝不能被当成「同一行」并掉，
    /// 否则整页会被并成一列，右侧所有段落都指向同一处。
    #[test]
    fn adjacent_columns_are_not_merged() {
        let mut out = Vec::new();
        push_merged(&mut out, ch(500.0, 800.0, 24.0, 600.0, "甲"));
        push_merged(&mut out, ch(460.0, 800.0, 24.0, 600.0, "乙"));
        assert_eq!(out.len(), 2, "相邻两列被误并了：{out:?}");
    }

    /// 同一行的包围盒有亚像素抖动（PDFKit 浮点误差）仍要合并；
    /// 容差 1pt 若被误设成 0，缩印页会重新裂成一堆半行。
    #[test]
    fn sub_point_jitter_still_merges() {
        let mut out = Vec::new();
        push_merged(&mut out, ch(500.0, 800.0, 24.0, 600.0, "甲"));
        push_merged(&mut out, ch(500.4, 799.6, 23.8, 600.3, "乙"));
        assert_eq!(out.len(), 1, "亚像素抖动不该拆成两段：{out:?}");
        assert_eq!(out[0].text, "甲乙");
    }

    fn region(x: f32, y: f32, w: f32, h: f32) -> SourceRegion {
        SourceRegion {
            x,
            y,
            w,
            h,
            text: "甲".to_string(),
        }
    }

    /// `/Rotate 90`：左上角的竖条必须变成**顶部右侧**的横条，且宽高互换。
    ///
    /// 这条守的是「图转了、坐标也得转」：`GetDrawingTransform` 按 /Rotate 把位图
    /// 转正，`PDFSelection.boundsForPage:` 给的仍是未旋转坐标，差这一次旋转就是
    /// 整片高亮错位——扫描件里横放的插页全中。
    #[test]
    fn rotation_90_turns_a_left_column_into_a_top_row() {
        // 未旋转页面 300×400；左上一个 20×100 的竖条。
        let out = rotate_regions(vec![region(0.0, 0.0, 20.0, 100.0)], 90, 300.0, 400.0);
        let g = &out[0];
        // x' = h - y - rh = 400 - 0 - 100 = 300；y' = x = 0。
        assert!((g.x - 300.0).abs() < 0.01, "x 应为 300，实得 {}", g.x);
        assert!((g.y - 0.0).abs() < 0.01, "y 应为 0，实得 {}", g.y);
        assert!((g.w - 100.0).abs() < 0.01, "宽高必须互换，实得 w={}", g.w);
        assert!((g.h - 20.0).abs() < 0.01, "宽高必须互换，实得 h={}", g.h);
        // 显示页面是 400×300（宽高互换过），整块必须落在里面。
        assert!(
            g.x >= 0.0 && g.x + g.w <= 400.01 && g.y >= 0.0 && g.y + g.h <= 300.01,
            "旋转后越界：{g:?}（显示 400×300）"
        );
    }

    /// `/Rotate 270`：左上角竖条变成**左下角**横条（与 90° 镜像，方向别弄反）。
    #[test]
    fn rotation_270_turns_a_left_column_into_a_bottom_row() {
        let out = rotate_regions(vec![region(0.0, 0.0, 20.0, 100.0)], 270, 300.0, 400.0);
        let g = &out[0];
        // x' = y = 0；y' = w - x - rw = 300 - 0 - 20 = 280。
        assert!((g.x - 0.0).abs() < 0.01, "x 应为 0，实得 {}", g.x);
        assert!((g.y - 280.0).abs() < 0.01, "y 应为 280，实得 {}", g.y);
        assert!((g.w - 100.0).abs() < 0.01 && (g.h - 20.0).abs() < 0.01, "宽高必须互换");
    }

    /// 180° 是对合：转两次必须回到原位，且宽高**不**换。
    #[test]
    fn rotation_180_is_its_own_inverse() {
        let src = region(10.0, 20.0, 30.0, 40.0);
        let once = rotate_regions(vec![src.clone()], 180, 300.0, 400.0);
        assert!((once[0].x - 260.0).abs() < 0.01, "x = 300-10-30 = 260，实得 {}", once[0].x);
        assert!((once[0].y - 340.0).abs() < 0.01, "y = 400-20-40 = 340，实得 {}", once[0].y);
        assert!(
            (once[0].w - 30.0).abs() < 0.01 && (once[0].h - 40.0).abs() < 0.01,
            "180° 不该换宽高"
        );
        let twice = rotate_regions(once, 180, 300.0, 400.0);
        assert!((twice[0].x - 10.0).abs() < 0.01 && (twice[0].y - 20.0).abs() < 0.01, "转两次没回到原位");
    }

    /// 270 之后再 90（用互换后的页面尺寸）必须精确回到原位。
    ///
    /// 这条同时钉住「换宽高」这件事：宽高不互换的话，第二次旋转的基准就错了，
    /// 回不到原位——而人的直觉是「转回来就该一模一样」。
    #[test]
    fn rotation_270_then_90_returns_home() {
        let src = region(10.0, 20.0, 30.0, 40.0);
        let a = rotate_regions(vec![src.clone()], 270, 300.0, 400.0);
        // 270° 后显示页面 400×300。
        let b = rotate_regions(a, 90, 400.0, 300.0);
        assert!((b[0].x - 10.0).abs() < 0.01, "x 没回来：{}", b[0].x);
        assert!((b[0].y - 20.0).abs() < 0.01, "y 没回来：{}", b[0].y);
        assert!((b[0].w - 30.0).abs() < 0.01 && (b[0].h - 40.0).abs() < 0.01, "宽高没还回来");
    }

    /// 0° 与非法值一律原样返回（非法角度按「不旋转」处理，绝不把框转飞）。
    #[test]
    fn zero_or_bogus_rotation_is_a_no_op() {
        for rot in [0, 360, -360, 45, 91] {
            let out = rotate_regions(vec![region(10.0, 20.0, 30.0, 40.0)], rot, 300.0, 400.0);
            assert!(
                (out[0].x - 10.0).abs() < 0.01
                    && (out[0].y - 20.0).abs() < 0.01
                    && (out[0].w - 30.0).abs() < 0.01,
                "rotation={rot} 不该改动坐标：{:?}",
                out[0]
            );
        }
    }

    /// 整页所有区域旋转后都必须落在显示页面内——一个越界就说明公式某处取反了。
    #[test]
    fn every_region_stays_inside_the_rotated_page() {
        // 未旋转 300×400；铺满整页的 3×4 网格。
        let mut src = Vec::new();
        for row in 0..4 {
            for col in 0..3 {
                src.push(region(
                    col as f32 * 100.0,
                    row as f32 * 100.0,
                    80.0,
                    90.0,
                ));
            }
        }
        for rot in [90, 180, 270] {
            let (dw, dh) = if rot % 180 == 90 { (400.0, 300.0) } else { (300.0, 400.0) };
            for r in rotate_regions(src.clone(), rot, 300.0, 400.0) {
                assert!(
                    r.x >= -0.01 && r.y >= -0.01 && r.x + r.w <= dw + 0.01 && r.y + r.h <= dh + 0.01,
                    "rotation={rot} 有区域越界：{r:?}（显示 {dw}×{dh}）"
                );
            }
        }
    }

    /// 端到端：真带 `/Rotate 90` 的 PDF，框必须落在**旋转后**的页面里。
    ///
    /// 这条守的是整条链路：PDFKit 读 `page.rotation` → `rotate_regions` 变换 →
    /// 与 `rasterize_source` 里「宽高互换后的画布 + GetDrawingTransform」对齐。
    /// 少任何一环，用户看到的就是「点右文，左图的框跑到页边或页外」。
    ///
    /// 对照那一步不能省：只断言「旋转页的框不越界」是能被「碰巧都很小」蒙过去的，
    /// 必须同时证明未旋转页是另一个尺寸基准。
    #[test]
    fn rotated_pdf_regions_land_inside_the_display_page() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();

        let flat = dir.join(format!("jiandu_rot0_{pid}.pdf"));
        std::fs::write(&flat, tiny_pdf(0)).expect("写样张失败");
        let r0 = extract_pdf_text_regions(&flat);
        let _ = std::fs::remove_file(&flat);

        let spun = dir.join(format!("jiandu_rot90_{pid}.pdf"));
        std::fs::write(&spun, tiny_pdf(90)).expect("写样张失败");
        let r90 = extract_pdf_text_regions(&spun);
        let _ = std::fs::remove_file(&spun);

        assert!(!r0.is_empty(), "/Rotate 0 的样张抽不出文本层（手写 PDF 有问题？）");
        assert!(!r90.is_empty(), "/Rotate 90 的样张抽不出文本层（旋转角被当成 0 了？）");

        // 未旋转页面 300×400。
        for r in &r0 {
            assert!(
                r.x >= -1.0 && r.y >= -1.0 && r.x + r.w <= 301.0 && r.y + r.h <= 401.0,
                "未旋转页的框越界：{r:?}"
            );
        }
        // 旋转后显示页面 400×300（宽高互换）——超过 300 的高度就是没转。
        for r in &r90 {
            assert!(
                r.x >= -1.0 && r.y >= -1.0 && r.x + r.w <= 401.0 && r.y + r.h <= 301.0,
                "旋转页的框越界：{r:?}（显示页应为 400×300）"
            );
        }

        // ⚠️ 下面两条才是真正有区分力的判据。只断言「不越界」会被「碰巧都很小」
        //    蒙过去（"Hello" 那种小框在任何页面里都像在范围内）。
        //
        // ① 一行横排字转 90° 必然「变竖」：宽 < 高。若旋转角被当成 0，
        //    这里会立刻翻车（原来是宽 > 高）。
        let flat0 = &r0[0];
        let spun0 = &r90[0];
        assert!(
            flat0.w > flat0.h,
            "未旋转时这一行应是横的（宽>高），实得 w={} h={}",
            flat0.w,
            flat0.h
        );
        assert!(
            spun0.w < spun0.h,
            "旋转 90° 后这一行应变竖（宽<高），实得 w={} h={}——旋转角没生效？",
            spun0.w,
            spun0.h
        );
        // ② 位置也要对：原文落在页面左下偏左（x≈50，y_up≈300），顺时针转 90°
        //    之后应跑到**右上**（x 靠大、y 靠小）。这是「图转了坐标也转了」的证据。
        assert!(
            spun0.x > 200.0,
            "旋转后 x 应靠右（≈300），实得 {}——旋转方向或基准宽度算错了",
            spun0.x
        );
        assert!(
            spun0.y < 150.0,
            "旋转后 y 应靠上（≈50），实得 {}",
            spun0.y
        );
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

/// **全链路验收（仅在 macOS 真跑）**：自己导出的横排 CJK 矢量 PDF →
/// `pdf_text` 抽坐标 → `importer::import_file` 走完整漏斗（清洗 → 断章 → ParsedDoc）。
///
/// 这条测试锁死本次修复的两条死穴：
/// ① 全角【题解】/（注）绝不能变成 `[注]` 双行夹注；
/// ② 文本层抽出来不能粘连、坐标盒不塌成 0；ParsedDoc 必须带上 pdf_regions。
#[cfg(all(test, target_os = "macos"))]
mod pdf_import_e2e {
    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::url::CFURL;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_text::font;
    use std::path::Path;

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

    /// 用 **Core Text `draw_glyphs`** 生成一份竖排 CJK PDF：源文本按换行切成若干
    /// 竖列，列与列从右向左排——这正是古籍 PDF 的天然形态，也还原了「章见出し独占
    /// 一列」的结构，清洗层据此正确断章。
    ///
    /// 选 `draw_glyphs`（而非 `CTFrameDraw`）是有意为之：本机实测 `CTFrameDraw` 写出的
    /// 横排 PDF，PDFKit 的 `selectionForRange` 抽不到文本层（返回 0 行）；而逐字
    /// `draw_glyphs` 的竖排页能被 `selectionsByLine` 完整切出（`own_vector_pdf_yields_every_glyph`
    /// 早锁死这条路径）。代价是 ToUnicode 在 CID 字体上偶发把个别字映射成别的码位，
    /// 所以本测试**不**对具体汉字做字面对等断言，只锁死「坐标非空 / 全角括号原样保留 /
    /// 不出现 `[注]` / 正文非空」这几条与码位无关的不变量。
    fn vertical_pdf_of(path: &Path, text: &str) {
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
        let f = font::new_from_name("Songti SC", 22.0).unwrap();
        // 一行＝一竖列，列从右（540）向左退 55pt。
        const STEP: f64 = 34.0;
        let mut x = 540.0;
        for line in text.split('\n') {
            let mut y = 800.0;
            for ch in line.chars() {
                let buf = [ch as u16];
                let mut g: [u16; 2] = [0; 2];
                let ok = unsafe { f.get_glyphs_for_characters(buf.as_ptr(), g.as_mut_ptr(), 1) };
                if ok {
                    let p = [CGPoint::new(x, y)];
                    let gs = [g[0]];
                    f.draw_glyphs(&gs, &p, ctx.clone());
                    y -= STEP;
                }
            }
            x -= 55.0;
        }
        unsafe { CGPDFContextEndPage(raw) };
        unsafe { CGPDFContextClose(raw) };
        drop(ctx);
    }

    #[test]
    fn pdf_import_end_to_end_preserves_fullwidth_brackets() {
        let path = std::env::temp_dir().join("jiandu_pdf_e2e.pdf");
        // 故意塞进全角【题解】与（注），验收它们原样保留、不当夹注；
        // 同时避开 ToUnicode 偶发错位的字（老/子/非/言/用/二/无/生），使断言稳定。
        let src = "道德经\n第一章\n道可道，常道。名可名，常名。\n【题解】此章明道之体，开卷之总纲也。\n第三章\n天下皆知美之为美，斯恶已。\n（注）故有相成，难易相形。\n不尚贤，使民不争。";
        vertical_pdf_of(&path, src);

        // ① 文本层抽取：坐标 + 文本都在（不为空＝不是扫描件）。
        let regions = extract_pdf_text_regions(&path);
        let sz = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!(
            "── 生成的竖排 PDF：{:?} 大小={} 区域数={}",
            path,
            sz,
            regions.len()
        );
        assert!(!regions.is_empty(), "文本层坐标不该为空（扫描件才会空）");
        for r in &regions {
            assert!(r.w > 0.0 && r.h > 0.0, "坐标盒不该塌成 0：{r:?}");
        }
        let layer: String = regions.iter().map(|r| r.text.as_str()).collect::<Vec<_>>().join(" / ");
        println!("── PDF 文本层（逐区域）──\n{layer}");

        // ② 完整漏斗：PDF → 清洗 → ParsedDoc
        let doc = crate::importer::import_file(&path).expect("PDF 导入失败");
        let regions_in_doc = doc
            .pdf_regions
            .as_ref()
            .expect("ParsedDoc 应携带 pdf_regions");
        assert_eq!(
            regions_in_doc.len(),
            regions.len(),
            "ParsedDoc 的坐标应与原抽取一致"
        );

        // ③ 全角【题解】/（注）绝不能变成 `[注]` 双行夹注（本次修复的死穴）。
        //    【】（）是 CJK 标点，ToUnicode 不串码，断言稳定；`[注]` 含 ASCII 方括号，
        //    若清洗层误升格必然现形。
        let book = doc.book.to_book();
        let body: String = book
            .chapters
            .iter()
            .flat_map(|c| {
                std::iter::once(c.title.clone())
                    .chain(c.paragraphs.iter().map(|p| p.text.clone()))
            })
            .collect();
        println!(
            "── 清洗后正文（首 360 字）──\n{}",
            &body.chars().take(360).collect::<String>()
        );
        // 排障：逐级看全角括号在哪一层被吃掉的（留在这里，下次回归一眼定位）。
        println!(
            "── classicize 后（首 200 字）──\n{}",
            &jiandu_core::pipeline::classicize(src)
                .chars()
                .take(200)
                .collect::<String>()
        );

        // 死穴：全角括号被误转成 `[注]` 夹注，正文缺一段。
        assert!(!body.contains("[注]"), "全角括号被误转成夹注：[注] 残留在正文");
        // 【题解】 原样保留方头括号（v1.9.27）：它是**章节体例标签**，
        // 整理本印刷时就是这么排的。竖排里 `【】` 居中、形如标签框；换成
        // 直角引号反倒像个被引用的句子，读者会误以为是引言。
        // （旧版这里变成 `[注]题解[/注]`，于是「题解」二字进了夹注、正文少一截。）
        assert!(
            body.contains("【题解】"),
            "【题解】应原样保留在正文：{body}"
        );
        assert!(
            !body.contains("「题解」"),
            "【题解】不该再转成直角引号：{body}"
        );
        assert!(
            body.contains("此章明道之体"),
            "题解的内容被夹注吞掉了：{body}"
        );
        // （注） → 「注」：竖排化把现代括号换成直角引号（`cleanup_punct_inner`）。
        assert!(body.contains("「注」"), "全角（注）应转成「注」：{body}");
        // 正文非空、没被吞字：用 ToUnicode 稳定的字（道 / 章 / 不尚贤）做探针。
        assert!(!body.is_empty(), "清洗后正文不应为空");
        assert!(body.contains("道可道"), "正文丢失/粘连：{body}");
        assert!(body.contains("不尚贤"), "末段丢失：{body}");
        // 章级切分正常（第一章 / 第三章 独占一列，清洗层识别为见出し）。
        assert!(
            body.contains("第一章") && body.contains("第三章"),
            "章见出し切分异常：{body}"
        );

        let _ = std::fs::remove_file(&path);
    }
}

