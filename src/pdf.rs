//! 导出 PDF：把「当前页的 PNG 截图」光栅嵌入 A4。
//!
//! 设计取舍：GPUI 0.2.2 没有矢量文字接口，且离线构建拉不进字体子集化库，
//! 所以**不**走「用 Path 重画鱼尾 / 界栏 / 正文」的矢量路线——那份工程在
//! 没有内嵌 CJK 字体时中文根本出不来。改用更稳、更保真的做法：
//!
//!   1. 用 `capture_card_png`（macOS `screencapture`，Retina 真分辨率）截下
//!      当前页；
//!   2. 用 `image` 解码成 RGB；
//!   3. 手写一份最小合法 PDF：一页 A4（595.28×841.89 pt @72dpi），把图等比
//!      适配、居中贴上，像素即所见——鱼尾、界栏、双行夹注、句读全部原样
//!      保留，零重绘误差。
//!
//! PDF 是纯文本结构，`flate2` 把图压成 `/FlateDecode` 流即可，无需任何
//! 外部 PDF 库。坐标映射：图片像素 → A4 磅值，等比缩放 + 居中，故屏幕上
//! 的 margin_top / col_width / 字坐标都按比例落到 A4 上。

use anyhow::{Context, Result};
use flate2::{write::ZlibEncoder, Compression};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A4 尺寸（PostScript point，72pt = 1 inch）。
const A4_W: f32 = 595.28;
const A4_H: f32 = 841.89;

/// 把一张 PNG 光栅嵌入新生成的 A4 PDF。返回写出的 PDF 路径。
///
/// `png` 必须是已存在的 PNG（通常由 `capture_card_png` 截得）。输出文件名
/// 由 `out` 决定；若 `out` 无 `.pdf` 后缀则自动补上。
pub fn png_to_pdf(png: &Path, out: &Path) -> Result<PathBuf> {
    let img = image::open(png)
        .with_context(|| format!("读取 PNG 失败：{}", png.display()))?
        .to_rgb8();
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 {
        anyhow::bail!("PNG 尺寸为 0");
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
    enc.write_all(&px)
        .context("压缩图像数据失败")?;
    let img_stream = enc.finish().context("结束压缩流失败")?;

    // 等比适配 A4 并居中。
    let s = (A4_W / w as f32).min(A4_H / h as f32);
    let dw = w as f32 * s;
    let dh = h as f32 * s;
    let x = (A4_W - dw) / 2.0;
    let y = (A4_H - dh) / 2.0;
    // 先缩放再平移：当前用户空间里图像以自然尺寸 w×h 绘制。
    let content = format!(
        "q\n{:.4} 0 0 {:.4} {:.4} {:.4} cm\n/Im0 Do\nQ\n",
        s, s, x, y
    );

    let out = ensure_pdf_ext(out);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let bytes = build_pdf(&content, &img_stream, w, h);
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

/// 手写一份最小合法 PDF（1 页 A4 + 1 个图像 XObject）。
///
/// 对象表：1=Catalog，2=Pages，3=Page，4=Contents 流，5=Image 流。
fn build_pdf(content: &str, img: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    // 记录每个对象的起始偏移，用于末尾 xref。
    let mut offsets: Vec<usize> = Vec::new();
    let mut emit = |buf: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(buf.len());
        buf.extend_from_slice(body);
    };

    emit(&mut buf, &mut offsets, format!("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").as_bytes());
    emit(&mut buf, &mut offsets, format!(
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 {:.2} {:.2}] >>\nendobj\n",
        A4_W, A4_H
    ).as_bytes());
    emit(&mut buf, &mut offsets, b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>\nendobj\n");
    emit(&mut buf, &mut offsets, format!(
        "4 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
        content.len(), content
    ).as_bytes());
    emit(&mut buf, &mut offsets, format!(
        "5 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
        w, h, img.len()
    ).as_bytes());
    buf.extend_from_slice(img);
    buf.extend_from_slice(b"\nendstream\nendobj\n");

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
}
