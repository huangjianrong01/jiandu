//! 导出 PDF：把「页面 PNG 截图」光栅嵌入 A4。
//!
//! 设计取舍：GPUI 0.2.2 没有矢量文字接口，且离线构建拉不进字体子集化库，
//! 所以**不**走「用 Path 重画鱼尾 / 界栏 / 正文」的矢量路线——那份工程在
//! 没有内嵌 CJK 字体时中文根本出不来。改用更稳、更保真的做法：
//!
//!   1. 用 `capture_page_png`（macOS `screencapture`，Retina 真分辨率）截下
//!      页面（本页 / 全书逐页）；
//!   2. 用 `image` 解码成 RGB；
//!   3. 手写一份最小合法 PDF：每页一张 A4，把图等比适配、居中贴上，
//!      像素即所见——鱼尾、界栏、双行夹注、句读全部原样保留，零重绘误差。
//!
//! PDF 是纯文本结构，`flate2` 把图压成 `/FlateDecode` 流即可，无需任何
//! 外部 PDF 库。坐标映射：图片像素 → A4 磅值，等比缩放 + 居中，故屏幕上
//! 的 margin_top / col_width / 字坐标都按比例落到 A4 上。
//!
//! v1.49.0：`pngs_to_pdf` 支持**多页**（全书导出）；页面朝向按图幅自适应
//! （对开双叶是宽幅 → A4 横躺，单叶窄幅 → A4 竖立），不再一律竖 A4 留
//! 两条大秃边。

use anyhow::{Context, Result};
use flate2::{write::ZlibEncoder, Compression};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A4 尺寸（PostScript point，72pt = 1 inch）。
const A4_W: f32 = 595.28;
const A4_H: f32 = 841.89;

/// 一页待嵌入的图（已解码、已压好的流 + 版面参数）。
struct Sheet {
    /// Flate 后的 RGB 字节（PDF /FlateDecode 流）。
    stream: Vec<u8>,
    w: u32,
    h: u32,
    /// 该页 MediaBox（朝向自适应后）。
    page_w: f32,
    page_h: f32,
    /// 图片在页面上的绘制矩形（等比适配 + 居中）。
    dx: f32,
    dy: f32,
    /// 缩放系数（内容流 `cm` 用）。
    scale: f32,
}

/// 把一张 PNG 光栅嵌入新生成的 A4 PDF。返回写出的 PDF 路径。
///
/// `png` 必须是已存在的 PNG（通常由 `capture_page_png` 截得）。输出文件名
/// 由 `out` 决定；若 `out` 无 `.pdf` 后缀则自动补上。
pub fn png_to_pdf(png: &Path, out: &Path) -> Result<PathBuf> {
    let pb = png.to_path_buf();
    pngs_to_pdf(std::slice::from_ref(&pb), out)
}

/// 把**一组** PNG 按序嵌入一份多页 PDF（全书导出用）。
///
/// 每页独立适配：页面朝向随该页图幅的自适应（宽幅横躺、窄幅竖立），
/// 故对开双叶与单叶混排也各自妥帖。空列表返回 `Err`。
///
/// 注意：`Path` 是 `?Sized`，无法构成 `&[Path]` 切片，故入参用 `&[PathBuf]`。
pub fn pngs_to_pdf(pngs: &[PathBuf], out: &Path) -> Result<PathBuf> {
    if pngs.is_empty() {
        anyhow::bail!("没有可嵌入的页面");
    }
    let mut sheets: Vec<Sheet> = Vec::with_capacity(pngs.len());
    for png in pngs {
        let img = image::open(png)
            .with_context(|| format!("读取 PNG 失败：{}", png.display()))?
            .to_rgb8();
        let w = img.width();
        let h = img.height();
        if w == 0 || h == 0 {
            anyhow::bail!("PNG 尺寸为 0：{}", png.display());
        }
        // RGB 字节：PDF 图像原点在左下，而 `image` 是上到下，故逐行翻转。
        let px: Vec<u8> = {
            let raw = img.into_raw();
            let mut out = Vec::with_capacity(raw.len());
            let stride = w as usize * 3;
            for row in (0..h as usize).rev() {
                out.extend_from_slice(&raw[row * stride..(row + 1) * stride]);
            }
            out
        };
        // Flate 压缩（zlib = PDF 的 /FlateDecode）。
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&px).context("压缩图像数据失败")?;
        let stream = enc.finish().context("结束压缩流失败")?;

        // 朝向自适应：宽幅（对开双叶）横躺，窄幅竖立。
        let (page_w, page_h) = if w as f32 >= h as f32 {
            (A4_H, A4_W)
        } else {
            (A4_W, A4_H)
        };
        // 等比适配并居中。
        let s = (page_w / w as f32).min(page_h / h as f32);
        let dw = w as f32 * s;
        let dh = h as f32 * s;
        sheets.push(Sheet {
            stream,
            w,
            h,
            page_w,
            page_h,
            dx: (page_w - dw) / 2.0,
            dy: (page_h - dh) / 2.0,
            scale: s,
        });
    }

    let out = ensure_pdf_ext(out);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let bytes = build_pdf(&sheets);
    std::fs::write(&out, bytes).with_context(|| format!("写出 PDF 失败：{}", out.display()))?;
    Ok(out)
}

/// 给路径补 `.pdf` 后缀（若没有）。
fn ensure_pdf_ext(p: &Path) -> PathBuf {
    let has = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);
    if has {
        p.to_path_buf()
    } else {
        let mut s = p.as_os_str().to_owned();
        s.push(".pdf");
        PathBuf::from(s)
    }
}

/// 手写一份最小合法多页 PDF。
///
/// 对象表：1=Catalog，2=Pages，随后每页三个对象（Page / Contents / Image），
/// 按 `3 + i*3` 起编。xref 偏移逐对象记录，末尾统一回填。
fn build_pdf(sheets: &[Sheet]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    // 记录每个对象的起始偏移，用于末尾 xref。
    let mut offsets: Vec<usize> = Vec::new();
    let emit = |buf: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(buf.len());
        buf.extend_from_slice(body);
    };

    emit(
        &mut buf,
        &mut offsets,
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );

    // Kids：每页的 Page 对象号 = 3 + i*3。
    let kids: Vec<String> = (0..sheets.len())
        .map(|i| format!("{} 0 R", 3 + i * 3))
        .collect();
    emit(
        &mut buf,
        &mut offsets,
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
            kids.join(" "),
            sheets.len()
        )
        .as_bytes(),
    );

    for (i, sh) in sheets.iter().enumerate() {
        let page_obj = 3 + i * 3;
        let cont_obj = page_obj + 1;
        let img_obj = page_obj + 2;
        // 先缩放再平移：当前用户空间里图像以自然尺寸 w×h 绘制。
        let content = format!(
            "q\n{:.4} 0 0 {:.4} {:.4} {:.4} cm\n/Im0 Do\nQ\n",
            sh.scale, sh.scale, sh.dx, sh.dy
        );
        emit(
            &mut buf,
            &mut offsets,
            format!(
                "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>\nendobj\n",
                page_obj, sh.page_w, sh.page_h, img_obj, cont_obj
            )
            .as_bytes(),
        );
        emit(
            &mut buf,
            &mut offsets,
            format!(
                "{} 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
                cont_obj,
                content.len(),
                content
            )
            .as_bytes(),
        );
        emit(
            &mut buf,
            &mut offsets,
            format!(
                "{} 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
                img_obj, sh.w, sh.h, sh.stream.len()
            )
            .as_bytes(),
        );
        buf.extend_from_slice(&sh.stream);
        buf.extend_from_slice(b"\nendstream\nendobj\n");
    }

    let xref_pos = buf.len();
    let n = offsets.len() + 1; // 含 0 号自由对象
    buf.extend_from_slice(format!("xref\n0 {}\n", n).as_bytes());
    buf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        buf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            n, xref_pos
        )
        .as_bytes(),
    );
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_png(path: &Path, w: u32, h: u32) {
        let img = image::ImageBuffer::from_fn(w, h, |x, y| {
            image::Rgb([
                ((x * 37) % 256) as u8,
                ((y * 53) % 256) as u8,
                ((x + y) % 256) as u8,
            ])
        });
        img.save(path).expect("写测试 PNG 失败");
    }

    #[test]
    fn png_to_pdf_produces_valid_pdf() {
        let dir = std::env::temp_dir();
        let png = dir.join("pdf_test_in.png");
        let pdf = dir.join("pdf_test_out"); // 无后缀，应自动补 .pdf
        make_png(&png, 64, 96);
        let out = png_to_pdf(&png, &pdf).expect("生成 PDF 失败");
        assert_eq!(out.extension().unwrap(), "pdf");
        let data = std::fs::read(&out).expect("读回 PDF 失败");
        let head = &data[0..8];
        assert_eq!(head, b"%PDF-1.4", "PDF 头部不符");
        let text = String::from_utf8_lossy(&data);
        assert!(text.contains("/Im0"), "缺图像对象");
        assert!(text.contains("/FlateDecode"), "缺压缩过滤");
        assert!(text.contains("startxref"), "缺 xref");
        assert!(text.contains("%%EOF"), "缺 EOF");
    }

    #[test]
    fn multi_page_pdf_has_all_pages() {
        let dir = std::env::temp_dir();
        let a = dir.join("pdf_multi_a.png");
        let b = dir.join("pdf_multi_b.png");
        let c = dir.join("pdf_multi_c.png");
        make_png(&a, 80, 60); // 宽幅 → 横躺
        make_png(&b, 60, 80); // 窄幅 → 竖立
        make_png(&c, 100, 70);
        let out = pngs_to_pdf(&[a, b, c], &dir.join("pdf_multi_out.pdf")).expect("多页 PDF 失败");
        let data = std::fs::read(&out).expect("读回 PDF 失败");
        let text = String::from_utf8_lossy(&data);
        assert!(text.contains("/Count 3"), "应为 3 页");
        assert!(text.contains("/Type /Page "), "缺 Page 对象");
        // 横躺页 MediaBox 宽 > 高（841.89 × 595.28）。
        assert!(text.contains("[0 0 841.89 595.28]"), "宽幅应横躺");
        assert!(text.contains("[0 0 595.28 841.89]"), "窄幅应竖立");
        assert_eq!(
            text.matches("/Im0 Do").count(),
            3,
            "每页各绘制一次图像"
        );
    }

    #[test]
    fn empty_page_list_is_error() {
        let dir = std::env::temp_dir();
        assert!(pngs_to_pdf(&[], &dir.join("pdf_empty.pdf")).is_err());
    }
}
