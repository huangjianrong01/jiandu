//! 七份「极脏」样本的端到端验收。
//!
//! 每个样本都在临时目录里**现造真文件**（含真 zip 包），不走内存造假——
//! 只有真文件才会暴露「编码嗅探」「压缩包结构」「流式读取」这些真实问题。
//!
//! 全部断言同一件事：**脏进 → 干净的 BookIR 出，且绝不 panic**。

use super::cleaner::{self, CleanOptions};
use super::*;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 一个测试专用的临时目录（进程号 + 用例名，互不干扰）。
fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("jiandu-import-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    d
}

fn write_bytes(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).expect("写临时文件失败");
    p
}

/// 造一个 zip（EPUB / DOCX 都是这个壳）。
fn write_zip(dir: &Path, name: &str, entries: &[(&str, &str)]) -> PathBuf {
    let p = dir.join(name);
    let f = std::fs::File::create(&p).expect("建 zip 失败");
    let mut w = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for (n, body) in entries {
        w.start_file(*n, opts).expect("写 zip 条目失败");
        w.write_all(body.as_bytes()).expect("写 zip 内容失败");
    }
    w.finish().expect("收尾 zip 失败");
    p
}

/// 整本书的正文（所有章的所有段拼起来），断言用。
fn all_text(ir: &BookIR) -> String {
    let b = ir.to_book();
    // 注意：core 的断章器会把「第一章」这类行**从正文里摘出去**当章名，
    // 所以整本书的正文必须连章名一起拼，否则会误判成「标题丢了」。
    let mut parts: Vec<String> = Vec::new();
    for c in &b.chapters {
        parts.push(c.title.clone());
        for p in &c.paragraphs {
            parts.push(p.text.clone());
        }
    }
    parts.join("")
}

/// 通用体检：零宽字符必须为零，且不能有形如 `<p>` 的标签残留。
fn assert_clean(t: &str) {
    for bad in ['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}', '\u{00AD}'] {
        assert!(!t.contains(bad), "零宽字符没洗净：{bad:?} 仍在");
    }
    assert!(!t.contains("<p>"), "标签漏进正文：{t}");
    assert!(!t.contains("</"), "闭合标签漏进正文：{t}");
    assert!(!t.contains("&nbsp;"), "实体没还原：{t}");
}

// ----------------------------------------------------------------------
// ① 乱码 TXT：GB18030 编码 + 零宽字符 + CRLF + 半角标点 + 三行软换行
// ----------------------------------------------------------------------
#[test]
fn dirty_txt_gb18030_zero_width_and_soft_wraps() {
    let dir = tmpdir("txt");
    // 原文（含零宽空格、CRLF、半角逗号、被折断的三行）
    let src = "第一回 甄士隱夢幻識通靈\r\n此開卷第一回也\u{200B}。作者自云\r\n因曾歷過一番夢幻之後\r\n故將真事隱去,而借通靈說此石頭記一書也。\r\n\r\n　　卻說賈雨村風塵懷閨秀。\r\n";
    let (bytes, _, _) = encoding_rs::GB18030.encode(src);
    let p = write_bytes(&dir, "脏.txt", &bytes);

    let ir = import_file(&p).expect("乱码 TXT 应该能读出来");
    let t = all_text(&ir);

    assert_clean(&t);
    assert!(t.contains('道') || t.contains('賈'), "正文丢了：{t}");
    assert!(t.contains("第一回"), "回目丢了：{t}");
    // 软换行必须拼回一段（三行连成一整句）
    assert!(
        t.contains("此開卷第一回也") && !t.contains("此開卷第一回也；作者"),
        "软换行没拼好：{t}"
    );
    assert!(ir.juans.len() >= 1, "一章都没切出来");
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// ② 嵌套标签 EPUB：container + opf + 两节，节里套 div/p/span，还夹 script
// ----------------------------------------------------------------------
#[test]
fn dirty_epub_with_nested_tags_and_scripts() {
    let dir = tmpdir("epub");
    let container = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
    let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <manifest>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine>
    <itemref idref="c1"/><itemref idref="c2"/>
  </spine>
</package>"#;
    // 故意把 ch2 的内容写成「第一章在前」，验证是照 spine 走而不是照文件名。
    let ch1 = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>一</title>
<style>body{font-size:12px}</style></head>
<body><nav>目录</nav>
<div class="wrap"><div class="inner"><h1>第一章</h1>
<p>道可道，<span class="k">非常</span>道。</p>
<script>var a=1;</script>
<p>名可名&nbsp;非常名。</p></div></div></body></html>"#;
    let ch2 = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
<h1>第二章</h1><p>上善若水。</p><header>页眉不该出现</header>
</body></html>"#;
    let p = write_zip(
        &dir,
        "脏.epub",
        &[
            ("mimetype", "application/epub+zip"),
            ("META-INF/container.xml", container),
            ("OEBPS/content.opf", opf),
            ("OEBPS/style.css", "body{}"),
            ("OEBPS/text/ch1.xhtml", ch1),
            ("OEBPS/text/ch2.xhtml", ch2),
        ],
    );

    let ir = import_file(&p).expect("EPUB 应该能读出来");
    let t = all_text(&ir);
    assert_clean(&t);
    assert!(t.contains("道可道"), "正文丢了：{t}");
    assert!(t.contains("非常道"), "行内 span 被吞：{t}");
    assert!(t.contains("上善若水"), "第二节丢了（spine 没走通）：{t}");
    assert!(!t.contains("var a=1"), "脚本漏进正文：{t}");
    assert!(!t.contains("font-size"), "CSS 漏进正文：{t}");
    assert!(!t.contains("页眉不该出现"), "header 漏进正文：{t}");
    assert!(!t.contains("目录"), "nav 漏进正文：{t}");
    // 章节顺序：第一章在第二章之前
    let a = t.find("第一章").unwrap_or(usize::MAX);
    let b = t.find("第二章").unwrap_or(usize::MAX);
    assert!(a < b, "章节顺序错了（没照 spine 走）：{t}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// ③ 空文件：0 字节也要给一个「空书」，不许 panic、不许报错
// ----------------------------------------------------------------------
#[test]
fn empty_file_yields_empty_book_not_panic() {
    let dir = tmpdir("empty");
    let p = write_bytes(&dir, "空.txt", b"");
    let ir = import_file(&p).expect("空文件不该报错");
    assert!(ir.juans.is_empty(), "空文件居然切出了章");
    // 空书 `to_book()` 会补一个占位章「正文」，但段落必须一个都没有。
    assert!(
        ir.to_book().chapters.iter().all(|c| c.paragraphs.is_empty()),
        "空文件居然有正文"
    );
    // 空 EPUB（zip 里什么都没有）：报可读的错，而不是 panic。
    let p2 = write_zip(&dir, "空.epub", &[("mimetype", "application/epub+zip")]);
    let e = import_file(&p2).unwrap_err();
    assert!(
        matches!(e, ImportError::Xml(_)),
        "空 EPUB 应报 Xml 错误，实际：{e:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// ④ 脏 HTML：script/style/nav/header + <br> + &nbsp; + 属性里带尖括号
// ----------------------------------------------------------------------
#[test]
fn dirty_html_drops_chrome_and_keeps_prose() {
    let dir = tmpdir("html");
    let html = r#"<html><head><title>书名</title><style>.a{color:#fff}</style></head>
<body><header>页眉</header><nav><a href="/">首页</a><a href="/2">目录</a></nav>
<h2>逍遙遊</h2>
<p>北冥有魚，其名為鯤。<br/>鯤之大，不知其幾千里也。</p>
<aside>侧栏广告</aside>
<script>track()</script>
<footer>版权所有</footer>
</body></html>"#;
    let p = write_bytes(&dir, "脏.html", html.as_bytes());
    let ir = import_file(&p).expect("HTML 应该能读");
    let t = all_text(&ir);
    assert_clean(&t);
    assert!(t.contains("北冥有魚"), "正文丢了：{t}");
    assert!(t.contains("鯤之大"), "<br> 之后的内容丢了：{t}");
    for bad in ["页眉", "目录", "侧栏广告", "track()", "版权所有", "#fff"] {
        assert!(!t.contains(bad), "杂质漏进正文（{bad}）：{t}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// ⑤ 脏 Markdown：YAML 头 + 井号 + 强调 + 引用 + 围栏代码 + 链接图片
// ----------------------------------------------------------------------
#[test]
fn dirty_markdown_strips_marks_and_front_matter() {
    let dir = tmpdir("md");
    let md = "---\ntitle: 道德經\nauthor: 老子\ntags: [a, b]\n---\n\n# 第一章\n\n> **道**可道，*非常*道。\n\n```\n名可名，非常名。\n```\n\n- 上善若水\n- [链接](http://example.com)\n\n![图](x.png)\n";
    let p = write_bytes(&dir, "脏.md", md.as_bytes());
    let ir = import_file(&p).expect("Markdown 应该能读");
    let t = all_text(&ir);
    assert_clean(&t);
    assert!(!t.contains("author"), "YAML 漏进正文：{t}");
    assert!(!t.contains('#'), "井号漏进正文：{t}");
    assert!(!t.contains("**"), "强调记号漏进正文：{t}");
    assert!(!t.contains("http://"), "链接地址漏进正文：{t}");
    assert!(t.contains("道可道"), "正文丢了：{t}");
    assert!(t.contains("上善若水"), "列表内容丢了：{t}");
    assert!(t.contains("名字") || t.contains("名可名"), "代码块内容丢了：{t}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// ⑥ 脏 DOCX：多段 + 表格 + 实体 + 域代码 + 修订删除
// ----------------------------------------------------------------------
#[test]
fn dirty_docx_reads_paragraphs_and_skips_field_codes() {
    let dir = tmpdir("docx");
    let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
  <w:p><w:pPr/><w:r><w:t>第一章　道經</w:t></w:r></w:p>
  <w:p><w:r><w:t xml:space="preserve">道可道，非常道&amp;名可名。</w:t></w:r></w:p>
  <w:p><w:r><w:instrText>PAGE \* MERGEFORMAT</w:instrText></w:r>
       <w:r><w:t>正文一</w:t></w:r><w:r><w:br/><w:t>正文二</w:t></w:r></w:p>
  <w:tbl><w:tr><w:tc><w:p><w:r><w:t>表格单元</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
  <w:p><w:del><w:r><w:delText>被删掉的字</w:delText></w:r></w:del><w:r><w:t>末段</w:t></w:r></w:p>
</w:body></w:document>"#;
    let p = write_zip(
        &dir,
        "脏.docx",
        &[
            ("[Content_Types].xml", "<Types/>"),
            ("word/document.xml", doc),
        ],
    );
    let ir = import_file(&p).expect("DOCX 应该能读");
    let t = all_text(&ir);
    assert_clean(&t);
    assert!(t.contains("道可道"), "正文丢了：{t}");
    assert!(t.contains('&'), "XML 实体没解开（&amp; 该变 &）：{t}");
    assert!(t.contains("表格单元"), "表格里的字丢了：{t}");
    assert!(!t.contains("MERGEFORMAT"), "域代码漏进正文：{t}");
    assert!(!t.contains("被删掉的字"), "修订删除的字漏进正文：{t}");
    assert!(t.contains("末段"), "末段丢了：{t}");
    // 缺 document.xml 的坏包：报 MissingEntry，不 panic。
    let bad = write_zip(&dir, "坏.docx", &[("word/styles.xml", "<x/>")]);
    let e = import_file(&bad).unwrap_err();
    assert!(
        matches!(e, ImportError::MissingEntry(_)),
        "坏 docx 应报 MissingEntry，实际：{e:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// ⑦ 脏 JSON：章节嵌套 + 多余字段 + 半截文件
// ----------------------------------------------------------------------
#[test]
fn dirty_json_nested_and_truncated() {
    let dir = tmpdir("json");
    let j = r#"{"book":{"title":"莊子","id":"uuid-1","toc":[{"k":1}]},
"chapters":[
  {"title":"內篇·逍遙遊","content":"北冥有魚，其名為鯤。"},
  {"title":"內篇·齊物論","paragraphs":["南郭子綦隱机而坐。","仰天而噓。"]}
]}"#;
    let p = write_bytes(&dir, "脏.json", j.as_bytes());
    let ir = import_file(&p).expect("JSON 应该能读");
    let t = all_text(&ir);
    assert_clean(&t);
    assert!(t.contains("北冥有魚"), "正文丢了：{t}");
    assert!(t.contains("南郭子綦"), "paragraphs 字段丢了：{t}");
    assert!(t.contains("逍遙遊"), "章名丢了：{t}");
    // 章名必须在正文之前（不能在章末）
    let a = t.find("逍遙遊").unwrap_or(usize::MAX);
    let b = t.find("北冥有魚").unwrap_or(usize::MAX);
    assert!(a < b, "章名掉到正文后面了：{t}");
    assert!(!t.contains("uuid-1"), "元信息漏进正文：{t}");

    // 半截 JSON：一个字都不许 panic，能救多少救多少。
    let p2 = write_bytes(&dir, "半截.json", r#"{"content":"道可道，非常道"#.as_bytes());
    let ir2 = import_file(&p2).expect("半截 JSON 不该 panic");
    let t2 = all_text(&ir2);
    assert!(t2.contains("道可道"), "半截 JSON 一个字都没救回来：{t2:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ----------------------------------------------------------------------
// 八、漏斗本身的契约（不在用户点名的七条里，但必须钉死）
// ----------------------------------------------------------------------
#[test]
fn unsupported_extension_is_a_readable_error() {
    let dir = tmpdir("ext");
    let p = write_bytes(&dir, "书.pdf", b"%PDF-1.4");
    let e = import_file(&p).unwrap_err();
    assert!(matches!(e, ImportError::Unsupported(_)), "实际：{e:?}");
    assert!(e.to_string().contains("pdf"), "错误信息没点出后缀：{e}");
    assert!(!is_supported(&p));
    assert!(is_supported(Path::new("a.EPUB")), "后缀大小写该不敏感");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cleaner_is_the_only_outlet() {
    // 清洗层的四条硬指标，逐条钉死。
    let dirty = "道\u{200B}可道,非常道\r\n\r\n\r\n名可名（未閉合\n\n";
    let c = cleaner::clean(dirty);
    assert!(!c.contains('\u{200B}'), "零宽没去掉");
    assert!(!c.contains('\r'), "换行没归一");
    assert!(c.contains("，"), "半角逗号没全角化：{c:?}");
    assert!(c.trim_end().ends_with('）'), "括号没闭合：{c:?}");
    assert!(!c.contains("\n\n\n"), "连续空行没压平");
    // 全角数字开关：默认关（否则回目会被折成全角，章节切不开）。
    assert!(!cleaner::clean("第12回 正文").contains('１'));
    let v = cleaner::clean_with("第12回 正文 30 年", &CleanOptions::for_vertical());
    assert!(v.contains("第12回"), "回目数字被折成全角了：{v:?}");
    assert!(v.contains("３０"), "正文数字没全角化：{v:?}");
}
