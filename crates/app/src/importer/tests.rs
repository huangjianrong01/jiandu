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
fn all_text(doc: &ParsedDoc) -> String {
    let b = doc.book.to_book();
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
    assert!(ir.book.juans.len() >= 1, "一章都没切出来");
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
    assert!(ir.book.juans.is_empty(), "空文件居然切出了章");
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
    let md = "---\ntitle: 道德經\nauthor: 老子\ntags: [a, b]\n---\n\n# 第一章\n\n> **道**可道，*非常*道。\n\n```\n名可名，非常名。\n```\n\n- 上善若水，水善利万物而不争。\n- [链接](http://example.com)\n\n![图](x.png)\n";
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
    // 围栏代码块整块丢弃：里面的「名可名，非常名。」不该进古籍正文。
    assert!(!t.contains("名可名"), "围栏代码块内容不该进正文：{t}");
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
       <w:r><w:t>正文一：道可道，非常道；名可名，非常名。</w:t></w:r><w:r><w:br/><w:t>正文二：天下皆知美之为美，斯恶已。</w:t></w:r></w:p>
  <w:tbl><w:tr><w:tc><w:p><w:r><w:t>表格单元内的说明文字亦须完整保留，不可遗失。</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
  <w:p><w:del><w:r><w:delText>被删掉的字</w:delText></w:r></w:del><w:r><w:t>末段：此书终。</w:t></w:r></w:p>
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
    // PDF 自 v1.9.25 起是**正式格式**（统一漏斗里的一等公民），不再是「不支持
    // 的后缀」。这条契约改用真正没实现的后缀来钉。
    let p = write_bytes(&dir, "书.xyz", b"whatever");
    let e = import_file(&p).unwrap_err();
    assert!(matches!(e, ImportError::Unsupported(_)), "实际：{e:?}");
    assert!(e.to_string().contains("xyz"), "错误信息没点出后缀：{e}");
    assert!(!is_supported(&p));
    assert!(is_supported(Path::new("a.EPUB")), "后缀大小写该不敏感");
    // 反向钉死：PDF 进了支持表，且坏的 PDF 报的是「需 OCR」而不是「不认识」。
    let bad = write_bytes(&dir, "坏.pdf", b"%PDF-1.4");
    assert!(is_supported(&bad), "PDF 应进支持表");
    let e2 = import_file(&bad).unwrap_err();
    assert!(
        matches!(e2, ImportError::OcrNeeded(_)),
        "坏 PDF 应报「需 OCR」：{e2:?}"
    );
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

// ----------------------------------------------------------------------
// 九、注音（`<ruby><rt>`）：拼音必须绑在字上，绝不能散进正文
// ----------------------------------------------------------------------

/// 建一本只有一章、正文里带 `<ruby>` 的 epub（ patches 7 的验收件）。
fn ruby_epub(dir: &Path) -> PathBuf {
    let container = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
    let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <manifest><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;
    let ch = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
<h1>第一章 天地</h1>
<p>子曰：<ruby>漢<rt>han4</rt></ruby>字，<ruby>道<rp>(</rp><rt>dao4</rt><rp>)</rp></ruby>可道也。</p>
</body></html>"#;
    write_zip(
        dir,
        "注音.epub",
        &[
            ("mimetype", "application/epub+zip"),
            ("META-INF/container.xml", container),
            ("OEBPS/content.opf", opf),
            ("OEBPS/ch1.xhtml", ch),
        ],
    )
}

#[test]
fn ruby_pinyin_stays_bound_to_its_character() {
    let dir = tmpdir("ruby");
    let p = ruby_epub(&dir);
    let ir = import_file(&p).expect("注音 epub 应能导入");
    let b = ir.to_book();

    let body: String = b
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter().map(|p| p.text.as_str()))
        .collect();
    // ① 拼音绝不散在正文里（散了就是竖排里的一排「电线杆」）
    assert!(!body.contains("han4") && !body.contains("dao4"), "拼音散进正文：{body:?}");
    assert!(!body.contains("拼音"), "注音壳漏进正文：{body:?}");
    // ② 宿主字一个都不能少
    assert!(body.contains("子曰：漢字，道可道也。"), "正文被误删：{body:?}");

    // ③ 注音挂在正确的字上：漢(idx 3) / 道(idx 6)
    let rubys: Vec<(usize, &str)> = b
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .flat_map(|p| p.notes.iter())
        .filter(|n| n.kind == jiandu_core::types::NoteKind::Ruby)
        .map(|n| (n.at, n.text.as_str()))
        .collect();
    assert_eq!(rubys.len(), 2, "该有两条注音，实际：{rubys:?}");
    assert!(rubys.iter().any(|(at, t)| *at == 3 && t.trim() == "han4"), "漢 的注音没绑上：{rubys:?}");
    assert!(rubys.iter().any(|(at, t)| *at == 6 && t.trim() == "dao4"), "道 的注音没绑上：{rubys:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `classicize` 不得把注音标记里的冒号折成全角——折了下游就认不出这枚标记。
#[test]
fn classicize_never_touches_inside_a_pinyin_mark() {
    let s = jiandu_core::pipeline::classicize("子[拼音:han4]曰,道.");
    assert!(s.contains("[拼音:han4]"), "注音标记被破坏：{s:?}");
    assert!(!s.contains("[拼音："), "冒号被折成全角：{s:?}");
    // 标记之外照常古籍化
    assert!(s.contains('，') && s.contains('。'), "标记外的标点没古籍化：{s:?}");
}

// ----------------------------------------------------------------------
// 真实书验收（**不进 CI**）：`cargo test -p jiandu --bin jiandu real_books -- --ignored
//   --nocapture`，文件路径由环境变量给。
//
// 这两本是「极端脏」的两极：一本把全书句读做成 91 万张 `<img>`，
// 另一本是中华书局点校本（`〔〕` 夹注 + `【章：】` 校勘记）。它们证明了
// 漏斗的两条分支各自有效，人工跑一次看统计即可。
// ----------------------------------------------------------------------

/// 讀環境變數指定的真書，回傳（清洗後正文, 抽到的字數）。
fn parse_real(var: &str) -> Option<(String, String)> {
    let p = std::env::var(var).ok()?;
    if p.is_empty() {
        return None;
    }
    let t = super::epub_parser::EpubParser::parse(std::path::Path::new(&p)).ok()?;
    Some((p, t))
}

#[test]
#[ignore = "需要真书：设 JIANDU_EPUB_IMG / JIANDU_EPUB_ZH 后手动跑"]
fn real_books_are_cleaned() {
    for (var, tag) in [
        ("JIANDU_EPUB_IMG", "图片句读本"),
        ("JIANDU_EPUB_ZH", "中华书局点校本"),
    ] {
        let Some((path, text)) = parse_real(var) else {
            println!("[{tag}] 未设 {var}，跳过");
            continue;
        };
        println!("\n════ {tag}：{path} ════");
        println!("  总字符数 {}", text.chars().count());
        println!("  行数     {}", text.lines().count());
        for (name, c) in [("逗号", '，'), ("句号", '。'), ("顿号", '、'), ("分号", '；')] {
            println!("  {name} {c} × {}", text.matches(c).count());
        }
        // 句读图片绝不能留下任何痕迹
        assert!(!text.contains("image19703"), "[{tag}] 句读图片没换成标点");
        assert!(!text.contains("<img"), "[{tag}] 标签壳漏进正文");
        assert!(!text.contains("class="), "[{tag}] 标签壳漏进正文");
        // 校勘记整条剔除
        if let Some(k) = text.find("『") {
            let a = text.floor_char_boundary(k.saturating_sub(40));
            let b = text.floor_char_boundary((k + 500).min(text.len()));
            println!("  ~~ 首个『 上下文: {:?}", &text[a..b]);
        }
        for kw in ["章：乙十一行本", "章：十二行本"] {
            if let Some(k) = text.find(kw) {
                let a = text.floor_char_boundary(k.saturating_sub(120));
                let b = text.floor_char_boundary((k + 160).min(text.len()));
                println!("  !! 残留 {kw} → {:?}", &text[a..b]);
            }
            assert!(!text.contains(kw), "[{tag}] 校勘记没剔除：{kw}");
        }
        // 结果得像文言：句号 + 逗号得有量
        assert!(
            text.matches('。').count() > 100,
            "[{tag}] 句号太少，句读多半没还原"
        );
        println!("  ── 首 6 行 ──");
        for l in text.lines().take(24) {
            println!("    {}", l.chars().take(90).collect::<String>());
        }
    }
}

/// **外部格式残留总验收**：一段「PDF 抽取 + 扒站 EPUB + OCR」三合一的
/// 典型脏文本，脏进 → 干净出。
///
/// 这份样本不是编的，每一条都能在真实电子书里找到出处（括号里的注释就是
/// 它的来源）。它锁死 [`jiandu_core::residue`] 的整条防线——任何一个函数
/// 被改坏，这里立刻现形。
#[test]
fn dirty_format_residue_is_filtered_end_to_end() {
    let raw = concat!(
        "道德經　上篇\n",                    // 页眉
        "第一章　道可道\n",
        "道 可 道 ， 非 常 道 。\n",           // 汉字间空格（PDF/OCR 头号产物）
        "- 12 -\n",                            // 孤立页码
        "名可名，非常名。\u{000C}無名天地之始。\n", // 分页符粘连
        "· 13 ·\n",
        "有名萬物之母。\n",
        "道 可 道 ， 非 常 道 。\n",           // 重复行（重章叠句，故意保留）
        "&nbsp;&ldquo;玄之又玄&rdquo;&#12288;眾妙之門。\n", // HTML 实体
        "**此句為全書綱領**\n",                // MD 加粗残留
        "> 案：此句疑衍。\n",                  // MD 引用残留
        "第二章　天下皆知美之為美[1]\n",        // 上标注号
        "①斯惡已。②有無相生。\n",              // 圈号分条（是内容，必须留）
        "目錄\n",
        "第一章　導論………………1\n",       // 点线目录
        "第二章　本論………………25\n",
        "其政悶悶，其民淳淳□□。\n",           // OCR 缺字框（留给字体回退）
        "絕學無憂――此之謂也。\n",              // 破折号残留（U+2015 ×2）
        "第 14 页\n",
        "Page 15 of 300\n",
        "上善若水。水善利萬物\n",              // 竖排碎片（无句读收尾）
        "而不爭。\n",
    );
    let out = cleaner::clean(raw);
    println!("\n===== clean() 输出 =====");
    for (i, l) in out.lines().enumerate() {
        println!("{i:>2}| {l}");
    }

    // ① 汉字间伪空格必须全部消失（竖排里每个空格都会顶出一个空字格）。
    assert!(!out.contains("道 可 道"), "汉字间空格没清掉：{out}");
    assert!(out.contains("道可道，非常道。"), "空格清掉后没拼回原句：{out}");

    // ② 页码行：四种写法一个都不许剩。
    for p in ["- 12 -", "· 13 ·", "第 14 页", "Page 15 of 300"] {
        assert!(!out.contains(p), "页码行残留 `{p}`：{out}");
    }

    // ③ HTML 实体：既要解出来，也要解成**直角引号**而不是字面的 `&ldquo;`。
    //    （v1.9.28 起清洗期就把弯引号归成 `「」`，竖排不认横排字形。）
    assert!(!out.contains('&'), "HTML 实体没解干净：{out}");
    assert!(out.contains("「玄之又玄」"), "实体没解成直角引号：{out}");

    // ④ Markdown 标记剥掉、内容留下。
    assert!(!out.contains("**"), "Markdown 加粗标记残留：{out}");
    assert!(out.contains("此句為全書綱領"), "Markdown 内容被误删：{out}");
    assert!(out.contains("案：此句疑衍。"), "引用标记没剥掉：{out}");

    // ⑤ 上标注号剥掉；圈号**必须留**（方书里「①人参 ②白朮」是内容）。
    assert!(!out.contains("[1]"), "上标注号残留：{out}");
    assert!(out.contains("①斯惡已"), "圈号是内容，不该被剥：{out}");

    // ⑥ 点线目录整行删。
    assert!(!out.contains("導論"), "点线目录没删：{out}");
    assert!(!out.contains("本論"), "点线目录没删：{out}");

    // ⑦ 换页符断段而不是粘连：两页的文字必须分处两行。
    assert!(
        out.contains("名可名，非常名。\n") && out.contains("無名天地之始。"),
        "换页符把两段粘住了：{out}"
    );

    // ⑧ 破折号字形归一（U+2015 → U+2014，长度不变）。
    assert!(out.contains("——"), "破折号没归一：{out}");
    assert!(!out.contains('―'), "旧破折号字形残留：{out}");

    // ⑨ OCR 缺字框保留原样，交给字体回退机制去画占位。
    assert!(out.contains("□□"), "缺字框被误删（这是原文信息）：{out}");
}

/// **【题解】这类章节标签必须活下来**。
///
/// 三条链路（清洗层升格 / `purge_bracket_marks` 换引号）各有一道坎，任何
/// 一道拦错了，正文就少一段——而且少得看不出来，因为「题解」两个字被
/// 排成了双行小字，读者只会觉得这一段怎么少了开头。
#[test]
fn tijie_label_survives_import() {
    let raw = "【题解】\n此章言道之體。\n\n【注釋】\n道可道，非常道。\n\n正文一段，其間有（注）者。\n\n〔案〕此句疑衍。\n";
    let text = cleaner::clean(raw);
    let body: String = parse_str(SrcKind::Txt, &text)
        .unwrap_or_default()
        .iter()
        .map(|c| c.content.clone())
        .collect();
    println!("\n===== 题解链路 =====\n{body}\n");

    // ① 标签原样保留方头括号（整理本印刷就这么排）。
    assert!(body.contains("【题解】"), "【题解】被改写了：{body}");
    assert!(body.contains("【注釋】"), "【注釋】被改写了：{body}");
    // ② 绝不升格成夹注。
    assert!(!body.contains("[注]题解"), "【题解】被升格成夹注：{body}");
    assert!(!body.contains("[注]注釋"), "【注釋】被升格成夹注：{body}");
    // ③ 标签后面的正文一个字都不能少。
    assert!(body.contains("此章言道之體"), "题解正文丢了：{body}");
    // ④ 单字圆括号 `（注）` 不许升格 —— `[注]注[/注]` 会被下游当壳吞掉，
    //    于是「其間有（注）者」变成「其間有者」，凭空少两个字。
    assert!(body.contains("其間有"), "正文被夹注吞了：{body}");
    assert!(!body.contains("[注]注[/注]"), "单字括号不该升格：{body}");
    // ⑤ 行首的〔案〕剥壳留字（它是按语，不是标签）。
    assert!(body.contains("案此句疑衍"), "〔案〕内容丢了：{body}");
}

/// **性能护栏**：十万字脏文本的清洗必须停在毫秒级。
///
/// 这不是炫耀数字，是拦退化用的。`residue` 里任何一个函数被写成「对每个
/// 空格回溯找左右邻居」或者「对每行 re-scan 全文」，在十万字上都会掉到
/// 秒级——导入时用户看到的就是转圈。实测约 13 ms（含全部脏特征），
/// 阈值给到 200 ms 留了十几倍余量，只为拦住量级错误，不为卡死机器抖动。
#[test]
fn clean_stays_fast_on_a_hundred_thousand_chars() {
    // 每一条都是真脏：汉字间空格 / HTML 实体 / 上标注号 / 假分页。
    let dirty: String = "道 可 道 ， 非 常 道 。&nbsp;名可名[1]，非常名。\n".repeat(3000);
    let n = dirty.chars().count();
    assert!(n > 90_000, "样本太小，测不出退化：{n}");
    let _ = cleaner::clean(&dirty); // 预热
    let t = std::time::Instant::now();
    let out = cleaner::clean(&dirty);
    let dt = t.elapsed();
    println!("[清洗吞吐] {n} 字 → {} 字，耗时 {dt:?}", out.chars().count());
    assert!(
        dt < std::time::Duration::from_millis(200),
        "清洗退化到 {dt:?}（>200ms），十万字不该这么慢"
    );
    // 顺便钉死「脏进干净出」：空格与实体都不能剩。
    assert!(!out.contains("道 可 道"), "汉字间空格没清掉");
    assert!(!out.contains('&'), "HTML 实体没解掉");
}

/// 真机回归（v1.9.28）：竖排源稿的「孤列」与畸形引号。
///
/// 用户真机看到「标题后面凭空多了一个国字」「善哉前面多了一个一字」
/// 「『希有 的引号和文字排版全乱了」。逐个码位实测过：`国`=U+56FD、
/// `一`=U+4E00、`〇`=U+3007，**都是普通字，没有零宽／控制符**——
/// 它们不是脏字节，是版心留下的字（叶码 / 函次 / 分卷记号），竖排抽取时
/// 各占一列；一列就是一行，下游一行一取段，于是成了「孤列」。竖排自右
/// 向左读，这一列紧挨正文列，眼睛就把它读进了正文。
#[test]
fn orphan_column_and_quote_are_cleaned() {
    let raw = "金刚经\n○善现启请分\u{3000}第二\n国\n时长老须菩提。\n一\n'善哉，善哉。'\n〇\n须菩提。\n";
    let out = cleaner::clean(raw);
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    // ① 版心符号、幽灵单字：整行删。
    assert!(!out.contains('○'), "分节圈点必须剥：{out}");
    assert!(!out.contains('〇'), "叶码 〇 必须删：{out}");
    assert!(!out.contains('国'), "幽灵「国」必须删：{out}");
    // ② 正文首字必须回到列首——孤列删掉后不许留空行占位。
    assert!(
        lines.iter().any(|l| l.starts_with("时长老须菩提")),
        "「国」删掉后正文应回到列首：{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("『善哉，善哉。』")),
        "半角单引号应归成成对的『』：{lines:?}"
    );
}

/// 真机回归（v1.9.29）：引号在**清洗期**就归成成对的直角引号。
#[test]
fn quotes_are_normalized_before_layout() {
    // 成对的单引号 = 内层引号，保持双层。
    assert_eq!(cleaner::clean("‘希有’世尊。"), "『希有』世尊。\n");
    assert_eq!(cleaner::clean("佛言：“善哉。”"), "佛言：「善哉。」\n");
    // **未配对的**一律降成单直角（用户口径：`『希有` → `「希有」`）。
    // 单层时无从判断是不是内层，`「」` 最中性；`『` 还是避头符，留着会把
    // 后面整段的列一列列往下推。
    assert_eq!(cleaner::clean("‘希有。"), "「希有。」\n");
    assert_eq!(cleaner::clean("'希有。"), "「希有。」\n");
}

