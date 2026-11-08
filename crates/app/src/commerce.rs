//! L9 · 商业导出引擎（零新增依赖，纯 Rust 标准库 + 现成 image crate）。
//!
//! * [`build_docx`] —— 最小 OOXML：`[Content_Types].xml` / `_rels/.rels` /
//!   `word/document.xml` 打进一个 store 模式的 ZIP。Word / WPS / Pages 均可打开编辑。
//! * [`build_searchable_pdf`] —— **双层可搜索 PDF**：原图 JPEG 作底层，
//!   Vision 识别的每个字按真实 x/y/w/h 以 `3 Tr`（不可见文字模式）覆盖在上方，
//!   并附 ToUnicode CMap，因此既能看到原图，又能被鼠标框选与全文搜索。
//!
//! 坐标系铁律：OCR 的 glyph 是**左上原点、y 向下**；PDF 页面是**左下原点、
//! y 向上**，故基线 `y_pdf = page_h - (glyph.y + glyph.h)`。绝不翻 CTM
//! （翻 CTM 会把底图与字形一起镜像，见 share_paint.rs 的 v1.58 事故）。

use std::io::Cursor;
use std::path::Path;

// ── ZIP（store 模式，无压缩，零依赖）─────────────────────────────────
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn zip_store(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let off = out.len() as u32;
        let crc = crc32(data);
        let nb = name.as_bytes();
        let len = data.len() as u32;
        // local file header
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method = store
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0u16.to_le_bytes()); // date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(nb);
        out.extend_from_slice(data);
        // central directory entry
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&(nb.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&off.to_le_bytes());
        central.extend_from_slice(nb);
    }
    let cd_off = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 按**阅读顺序**（字符下标 == `glyph.target`）把最新校对文本写成可编辑 Word。
///
/// 正注分离与右栏同源：复用 `compare_editor::classify_spans` 的
/// `（…）` / `[注:…]` / `[注]…[/注]` 口径，按连续层级切 run：
///   · 正文大字 12pt（`w:sz=24` half-points）；
///   · 夹注小字  8pt（`w:sz=16`）+ 灰色 + 段首缩进；
///   · 结构标签  7pt 斜体灰字，保留可编辑但不冒充正文。
/// 全文字符顺序严格不变，绝不按坐标重排、绝不丢字。
pub fn build_docx(text: &str) -> Result<Vec<u8>, String> {
    use crate::compare_editor::{resolve_spans, SpanKind};

    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
    // 归一化换行，保证 classify_spans 的 kinds 下标与 split 出的行严格对齐。
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    // 与右栏渲染同源：显式标记 → 字高聚类 → 注本体例切分。真实 OCR 纯文本
    // 也必须有大字/小字层级，不能只认 `[注]` 标签（否则整份 Word 一个字号）。
    let kinds = resolve_spans(&normalized, &[]);
    let mut body = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
    );
    let mut base = 0usize; // 本行首字在全文中的绝对下标
    for line in normalized.split('\n') {
        let n = line.chars().count();
        // 含夹注的段落整体缩进 240 twips，正文段落不缩进 —— 排版层级一眼可辨。
        let has_gloss = (0..n).any(|i| {
            matches!(
                kinds.get(base + i),
                Some(SpanKind::Gloss) | Some(SpanKind::GlossTag)
            )
        });
        let ind = if has_gloss {
            r#"<w:ind w:left="240"/>"#
        } else {
            ""
        };
        body.push_str(&format!(
            "<w:p><w:pPr><w:spacing w:after=\"120\" w:line=\"360\" w:lineRule=\"auto\"/>{}</w:pPr>",
            ind
        ));
        let mut j = 0usize;
        while j < n {
            let kind = kinds.get(base + j).copied().unwrap_or(SpanKind::Body);
            let mut k = j + 1;
            while k < n
                && kinds.get(base + k).copied().unwrap_or(SpanKind::Body) == kind
            {
                k += 1;
            }
            let seg: String = line.chars().skip(j).take(k - j).collect();
            let (sz, color, italic) = match kind {
                SpanKind::Body => (24u32, "1D1D1F", false),
                SpanKind::Gloss => (16u32, "5A5A5E", false),
                SpanKind::GlossTag => (14u32, "9A9A9E", true),
            };
            body.push_str(&format!(
                "<w:r><w:rPr><w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/>{}<w:color w:val=\"{color}\"/></w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
                if italic { "<w:i/>" } else { "" },
                xml_escape(&seg)
            ));
            j = k;
        }
        body.push_str("</w:p>");
        base += n + 1; // '\n' 不渲染，但占一个全文绝对下标
    }
    body.push_str("<w:sectPr/></w:body></w:document>");
    Ok(zip_store(&[
        ("[Content_Types].xml", types.as_bytes().to_vec()),
        ("_rels/.rels", rels.as_bytes().to_vec()),
        ("word/document.xml", body.into_bytes()),
    ]))
}

// ── 双层可搜索 PDF ──────────────────────────────────────────────────
/// `(x, y, w, h, target)`：图像像素坐标（左上原点），`target` 是 text 的字符下标。
pub fn build_searchable_pdf(
    png: &Path,
    glyphs: &[(f32, f32, f32, f32, usize)],
    text: &str,
    dims: (u32, u32),
) -> Result<Vec<u8>, String> {
    let (iw, ih) = dims;
    if iw == 0 || ih == 0 {
        return Err("图像尺寸为 0，无法生成 PDF".into());
    }
    let img = image::open(png).map_err(|e| format!("读取原图失败：{e}"))?;
    // ⚠️ 底图**实际像素**可能与 OCR 契约里的 dims 不同（导入时会降采样）。
    // 若拿 dims 直接当底图尺寸，文字层会整体漂移——先算两级比例：
    //   ① OCR 坐标 → 底图像素：kx / ky
    //   ② 底图像素 → PDF Point：s（统一按 A4 宽度等比缩放）
    let (aw, ah) = (img.width() as f64, img.height() as f64);
    if aw <= 0.0 || ah <= 0.0 {
        return Err("底图像素尺寸为 0".into());
    }
    let mut jpeg = Cursor::new(Vec::new());
    img.write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .map_err(|e| format!("JPEG 编码失败：{e}"))?;
    let jpeg = jpeg.into_inner();

    let kx = aw / iw as f64;
    let ky = ah / ih as f64;
    const A4_W_PT: f64 = 595.276; // A4 宽（pt）
    let s = A4_W_PT / aw;
    let page_w = aw * s;
    let page_h = ah * s;
    let chars: Vec<char> = text.chars().collect();
    // 与右栏 / DOCX 同源的层级：正文大字铺满字框，夹注小字按 0.62 收缩，
    // 结构标签 0.55。文字层虽不可见，选中框的相对大小仍保留古籍正注层次。
    use crate::compare_editor::{resolve_spans, SpanKind};
    let kinds = resolve_spans(text, &[]);

    // ── 图层顺序：先画底图（q…Q 隔离 CTM），再叠不可见文字层 ──
    // 绝不翻 CTM：翻 CTM 会把底图一起镜像（share_paint v1.58 事故）。
    // Y 轴翻转只在文字层里对**基线 y** 做：pdf_y = page_h − (顶 + 高)。
    let mut content = format!(
        "q\n{:.3} 0 0 {:.3} 0 0 cm\n/Im0 Do\nQ\n",
        page_w, page_h
    );
    // 每个字框一个独立 CID（双字节 Identity-H），而不是「按字符去重分配
    // 单字节码位」：一页古籍常有 300+ 个互不相同汉字，单字节码位只有 256
    // 个，超出后复用码位会让复制出来的文字变成乱码。CID 上限 65534。
    let mut cids: Vec<(u16, char)> = Vec::new();
    for &(x, y, gw, gh, target) in glyphs {
        let Some(&ch) = chars.get(target) else { continue };
        if ch.is_whitespace() || cids.len() >= 65534 {
            continue;
        }
        let cid = cids.len() as u16 + 1;
        cids.push((cid, ch));
        let gx = x as f64 * kx * s;
        let gy = y as f64 * ky * s;
        let gw_pt = (gw as f64 * kx * s).max(0.5);
        let gh_pt = (gh as f64 * ky * s).max(0.5);
        let kind_scale = match kinds.get(target) {
            Some(SpanKind::Gloss) => 0.62,
            Some(SpanKind::GlossTag) => 0.55,
            _ => 1.0,
        };
        let size = (gh_pt * 0.9 * kind_scale).max(1.0);
        // ★ Y 轴翻转（在 pt 坐标系里做，与底图缩放后的高度严格对应）
        let py = page_h - (gy + gh_pt);
        // Tz 水平缩放：让选中框宽度贴住字形框，鼠标框选才「死死盖在原图上」。
        let tz = ((gw_pt / (size * 1.0)) * 100.0).clamp(10.0, 1000.0);
        content.push_str(&format!(
            "BT\n3 Tr\n/F1 {:.3} Tf\n{:.1} Tz\n1 0 0 1 {:.3} {:.3} Tm\n<{:04X}> Tj\nET\n",
            size, tz, gx, py, cid
        ));
    }

    // ToUnicode CMap：双字节 CID → UTF-16BE，macOS「预览」/ PDFKit 据此
    // 还原可搜索、可复制的中文，绝不出乱码。
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    if !cids.is_empty() {
        cmap.push_str(&format!("{} beginbfchar\n", cids.len()));
        for (cid, ch) in &cids {
            let mut buf = [0u16; 2];
            let u = ch.encode_utf16(&mut buf);
            let hex: String = u.iter().map(|v| format!("{:04X}", v)).collect();
            cmap.push_str(&format!("<{:04X}> <{}>\n", cid, hex));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str(
        "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n",
    );

    fn stream_obj(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(data.len() + dict.len() + 32);
        v.extend_from_slice(format!("<< {} /Length {} >>\nstream\n", dict, data.len()).as_bytes());
        v.extend_from_slice(data);
        v.extend_from_slice(b"\nendstream");
        v
    }

    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        // MediaBox 用的是 **pt**（缩放后），不是像素：这就是像素 → PDF Point
        // 的比例映射，文字层的 x/y/字号全部乘同一个 s，零漂移。
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.3} {:.3}] \
             /Resources << /XObject << /Im0 4 0 R >> /Font << /F1 6 0 R >> >> \
             /Contents 5 0 R >>",
            page_w, page_h
        )
        .into_bytes(),
        stream_obj(
            &format!(
                "/Type /XObject /Subtype /Image /Width {} /Height {} \
                 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
                img.width(),
                img.height()
            ),
            &jpeg,
        ),
        stream_obj("", content.as_bytes()),
        // Type0 / Identity-H：双字节 CID → 每个字框独立码位，一页 300+ 个
        // 不同汉字也不会串码、不会乱码。3 Tr 让它完全不遮挡底图。
        b"<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light \
           /Encoding /Identity-H /DescendantFonts [8 0 R] /ToUnicode 7 0 R >>"
            .to_vec(),
        stream_obj("", cmap.as_bytes()),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /STSong-Light \
           /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
           /DW 1000 /W [1 [1000]] /FontDescriptor 9 0 R >>"
            .to_vec(),
        b"<< /Type /FontDescriptor /FontName /STSong-Light /Flags 4 \
           /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 880 \
           /Descent -120 /CapHeight 880 /StemV 80 >>"
            .to_vec(),
    ];

    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0usize; objs.len() + 1];
    for (i, body) in objs.iter().enumerate() {
        offsets[i + 1] = out.len();
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for i in 1..=objs.len() {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[i]).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objs.len() + 1,
            xref
        )
        .as_bytes(),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_png() -> PathBuf {
        PathBuf::from(
            std::env::var("COMMERCE_PNG")
                .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../test_ocr/page1.png").into()),
        )
    }

    // ── 标准一：DOCX 阅读顺序 + 正注分层 ──────────────────────────────
    #[test]
    fn docx_keeps_target_reading_order_and_separates_gloss() {
        let text = "天地（玄黃）宇宙\n第二章[注:校記]正文";
        let docx = build_docx(text).expect("DOCX");
        assert_eq!(&docx[..2], b"PK", "DOCX 必须是合法 ZIP");
        let xml = String::from_utf8_lossy(&docx);
        // store 模式 ZIP 里 document.xml 是明文，可直接验内容。
        let a = xml.find("天地").expect("正文起始");
        let b = xml.find("玄黃").expect("夹注");
        let c = xml.find("宇宙").expect("正文续");
        assert!(a < b && b < c, "必须严格按字符下标（target）阅读顺序输出");
        assert!(xml.find("第二章").unwrap() < xml.find("正文").unwrap());
        assert!(xml.contains(r#"w:sz w:val="24""#), "正文大字 12pt");
        assert!(xml.contains(r#"w:sz w:val="16""#), "夹注小字 8pt");
        assert!(xml.contains(r#"w:sz w:val="14""#), "结构标签 7pt");
        assert!(xml.contains(r#"<w:ind w:left="240"/>"#), "夹注段落缩进");
        std::fs::write("/tmp/commerce_verify.docx", &docx).expect("写 DOCX");
    }

    // ── 标准二：Y 轴翻转 + 像素→pt 比例 + 图层顺序 ────────────────────
    #[test]
    fn pdf_y_flip_scale_and_layer_order() {
        let png = sample_png();
        assert!(png.exists(), "缺少验证用 PNG：{}", png.display());
        let dims = image::image_dimensions(&png).expect("图像尺寸");
        let text = "天地玄黃宇宙洪荒";
        let glyphs: Vec<(f32, f32, f32, f32, usize)> = (0..8)
            .map(|i| (10.0 + i as f32 * 20.0, 30.0, 18.0, 20.0, i))
            .collect();
        let pdf = build_searchable_pdf(&png, &glyphs, text, dims).expect("双层 PDF");
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.starts_with("%PDF-1.4"));
        assert!(s.contains("3 Tr"), "必须有不可见文字层");
        assert!(s.contains("/DCTDecode"), "必须有原图底层");
        assert!(s.contains("/ToUnicode"), "必须有可搜索映射");
        assert!(s.contains("/Subtype /Type0"), "双字节 CID 字体");
        assert!(s.contains("/Identity-H"));
        assert!(s.contains("%%EOF"));
        // 底图必须先画、文字层后叠（否则 3 Tr 虽不可见，但结构不合规）。
        let img_at = s.find("/Im0 Do").expect("底图绘制");
        let text_at = s.find("3 Tr").expect("文字层");
        assert!(img_at < text_at, "图层顺序：原图在下、文字在上");
        // 像素 → pt：MediaBox 宽必须是 A4 宽，而不是像素宽。
        assert!(s.contains("/MediaBox [0 0 595.276"), "必须做像素→Point 缩放");
        // Y 轴翻转：第一个字框 y=30、h=20，基线须落在页面下半部。
        let s_pt = 595.276 / dims.0 as f64;
        let page_h = dims.1 as f64 * s_pt;
        let expect_y = page_h - ((30.0 + 20.0) * s_pt);
        assert!(expect_y > 0.0 && expect_y < page_h);
        assert!(
            s.contains(&format!("{:.3} Tm", expect_y).replace(" Tm", "")),
            "基线 y 必须等于 page_h − (y+h)×s，实际期望 {:.3}",
            expect_y
        );
        std::fs::write("/tmp/commerce_verify.pdf", &pdf).expect("写 PDF");
    }

    // ── 标准二补充：一页 300+ 不同汉字绝不串码 / 不乱码 ────────────────
    #[test]
    fn pdf_many_distinct_chars_have_unique_cids() {
        let png = sample_png();
        let dims = image::image_dimensions(&png).expect("图像尺寸");
        let text: String = (0..300u32)
            .map(|i| char::from_u32(0x4E00 + i).unwrap())
            .collect();
        let glyphs: Vec<(f32, f32, f32, f32, usize)> = (0..300)
            .map(|i| (5.0, 5.0 + i as f32, 10.0, 10.0, i))
            .collect();
        let pdf = build_searchable_pdf(&png, &glyphs, &text, dims).expect("双层 PDF");
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("300 beginbfchar"), "每个字框都要有独立 Unicode 映射");
        assert!(s.contains("<0001> <4E00>"), "首字 CID→U+4E00");
        // 0x4E00 + 299 = 0x4F2B；CID 300 = 0x012C。
        assert!(s.contains("<012C> <4F2B>"), "第 300 字 CID→U+4F2B");
        // 单字节码位方案会在第 256 字后复用码位，这里必须一个都没有。
        assert!(!s.contains("beginbfchar\n<20>"));
    }

    #[test]
    fn zero_dims_never_panic() {
        let err = build_searchable_pdf(Path::new("/nonexistent.png"), &[], "", (0, 0));
        assert!(err.is_err());
    }

    // ── 结项验收 · 全量生成自测（不靠人眼看按钮）─────────────────────
    /// 把 store 模式 ZIP 里所有 `<w:t>` 拼回纯文本，逐字核对字数与顺序。
    fn docx_plain_text(docx: &[u8]) -> String {
        let xml = String::from_utf8_lossy(docx);
        let tag = "<w:t xml:space=\"preserve\">";
        let mut out = String::new();
        let mut rest: &str = &xml;
        while let Some(p) = rest.find(tag) {
            rest = &rest[p + tag.len()..];
            if let Some(e) = rest.find("</w:t>") {
                out.push_str(&rest[..e]);
                rest = &rest[e + 6..];
            } else {
                break;
            }
        }
        out
    }

    #[test]
    fn docx_full_generation_keeps_exact_char_count() {
        // 模拟真实导出的「校对后最新文本」：含换行、夹注标签、长短句。
        let text = "道可道，非常道。名可名，非常名。\n第二章[注:校記]正文";
        let docx = build_docx(text).expect("DOCX 生成");
        let plain = docx_plain_text(&docx);
        let want: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        assert_eq!(plain, want, "必须逐字保序，不丢字、不添字、不重排");
        assert_eq!(plain.chars().count(), want.chars().count(), "字数必须精确相等");
        std::fs::write("/tmp/commerce_verify.docx", &docx).expect("写 DOCX 产物");
    }

    #[test]
    fn docx_plain_ocr_text_still_has_two_tiers() {
        // 无 `[注]` 标记的真实 OCR 纯文本，也必须在 Word 里分出大小字。
        let text = "道可道，非常道。可道之道，可名之名，指事造形，非其常也。";
        let docx = build_docx(text).unwrap();
        let xml = String::from_utf8_lossy(&docx);
        assert!(xml.contains(r#"w:sz w:val="24""#), "大字正文 12pt");
        assert!(xml.contains(r#"w:sz w:val="16""#), "夹注小字 8pt");
    }

    #[test]
    fn pdf_full_generation_has_selectable_text_layer() {
        let png = sample_png();
        assert!(png.exists(), "缺少验证用 PNG：{}", png.display());
        let dims = image::image_dimensions(&png).expect("图像尺寸");
        // 短经句（大字）+ 长注句（小字），一页两种层级。
        let text = "道可道，非常道。可道之道，可名之名，指事造形，非其常也。";
        let n = text.chars().count();
        let glyphs: Vec<(f32, f32, f32, f32, usize)> = (0..n)
            .map(|i| (10.0 + (i % 12) as f32 * 20.0, 30.0 + (i / 12) as f32 * 24.0, 18.0, 20.0, i))
            .collect();
        let pdf = build_searchable_pdf(&png, &glyphs, text, dims).expect("双层 PDF 生成");
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.starts_with("%PDF-1.4"));
        assert_eq!(s.matches("3 Tr").count(), n, "每个字框一条不可见文字指令");
        assert!(s.contains("/DCTDecode"), "底层原图存在");
        assert!(s.contains("/ToUnicode") && s.contains("/Identity-H"), "可复制映射存在");
        assert!(s.contains(&format!("{} beginbfchar", n)), "每字 CID→Unicode 齐全");
        // 用代码「读回」文字层字号：至少要有大字 / 小字两种 Tf。
        let mut sizes = std::collections::BTreeSet::new();
        let mut rest: &str = &s;
        while let Some(p) = rest.find("/F1 ") {
            rest = &rest[p + 4..];
            if let Some(e) = rest.find(" Tf") {
                sizes.insert(rest[..e].to_string());
                rest = &rest[e + 3..];
            } else {
                break;
            }
        }
        assert!(sizes.len() >= 2, "文字层必须体现正注层级：{sizes:?}");
        std::fs::write("/tmp/commerce_verify.pdf", &pdf).expect("写 PDF 产物");
    }

    /// 纯图片扫描件（无原生文本层）：OCR 出 105 个字形，导出的双层 PDF
    /// 必须逐个可复制。产物交给外部 PDFKit 做真实鼠标框选验证。
    #[test]
    fn pdf_pure_scan_page_105_chars_are_selectable() {
        let png = sample_png();
        assert!(png.exists(), "缺少扫描页 PNG：{}", png.display());
        let dims = image::image_dimensions(&png).expect("图像尺寸");
        let text: String = (0..105u32)
            .map(|i| char::from_u32(0x4E00 + i).unwrap())
            .collect();
        let glyphs: Vec<(f32, f32, f32, f32, usize)> = (0..105usize)
            .map(|i| {
                (
                    10.0 + (i % 15) as f32 * 20.0,
                    30.0 + (i / 15) as f32 * 24.0,
                    18.0,
                    20.0,
                    i,
                )
            })
            .collect();
        let pdf = build_searchable_pdf(&png, &glyphs, &text, dims).expect("扫描页双层 PDF");
        let s = String::from_utf8_lossy(&pdf);
        assert_eq!(s.matches("3 Tr").count(), 105, "105 个字各一条隐形文字指令");
        assert!(s.contains("105 beginbfchar"), "105 个字各一条 Unicode 映射");
        assert!(s.contains("/DCTDecode"), "扫描原图必须在底层");
        std::fs::write("/tmp/commerce_verify_105.pdf", &pdf).expect("写 105 字产物");
    }
}
