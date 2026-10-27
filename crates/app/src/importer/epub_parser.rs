//! EPUB：一个 zip，里面装着 OPF 目录 + 一堆 XHTML。
//!
//! v1.80.0 起本模块多扛一副担子：**古籍净化漏斗的结构层（DOM 层）**。
//! 现代出版社做的古籍 EPUB 里，正文只占一小半——封面题名页、版权页、丛书名、
//! 译注者、目录、前言、索引全被转换器当成「正文」塞进同一个阅读流。一册
//! 《尔雅》能把「管锡华◎译注」「全本全注全译丛书」和「初、哉、首、基」排
//! 在同一叶上。所以抽取流程改成**三层漏斗**，脏数据在**解析 HTML 结构时就
//! 被杀掉**，绝不落进 JSON / 数据库再回头补救：
//!
//! ```text
//!   第一层 结构过滤（本文件）
//!     ├ 目录优先：解析 toc.ncx（EPUB2）/ nav.xhtml（EPUB3），
//!     │  只收「目录里真实存在的章节」对应的节；
//!     ├ 名前判性：目录项叫「前言 / 序言 / 目录 / 索引 / 后记 / 附录」的
//!     │  整节不收（front / back matter）；
//!     ├ 现代标签：epub:type="titlepage" / copyright-page / colophon …
//!     │  与文件名含 cover / title / copyright / toc 的节不收；
//!     └ 垃圾文件：字数极少（< 200 可见字）且满篇出版词 → 整节丢。
//!   第二层 关键词屏蔽（jiandu_core::purge，逐行）
//!     └ ISBN / 出版社 / 版权所有 / 定价 / 印张 / 丛书 / 译注 / 主编 /
//!       责任编辑 / 图书在版编目 …铁证词无条件杀；软词只杀短行。
//!   第三层 启发式正文起点（jiandu_core::purge::find_body_start）
//!     └ 目录不可信时，逐段打「文言度」分，连着三段像文言才算正文开始，
//!       之前的全部扔掉；认不出就不动。
//! ```
//!
//! 步骤固定四步，缺一步都不行：
//!   1. `META-INF/container.xml` → 找到 `.opf` 的路径（书脊目录在这里）；
//!   2. 读 OPF 的 `manifest`（id → href）与 `spine`（阅读顺序的 idref 列表）——
//!      **顺序必须照 spine 走**，按文件名排序会把书排乱（`.xhtml` 的编号
//!      常常是倒着的，或者干脆没有编号）；
//!   3. 读 `toc.ncx` / `nav.xhtml` 拿真实章节表 → 定出「收哪些节」；
//!   4. 逐节抽正文（复用 [`super::html_parser`]）并过漏斗，节与节之间断行。
//!
//! 容错：任何一节打不开 / 畸形，**跳过那一节继续**；OPF 彻底读不出来时，
//! 退化成「把 zip 里所有 xhtml 按文件名排序」——总比整本导入失败强。

use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::html_parser::{extract_html_reader, extract_html_reader_keep_tables};
use super::traits::FormatParser;
use super::ImportError;

/// 垃圾节的字数门槛（可见字）：低于它且噪声行够多 → 丢。
const JUNK_MIN_CHARS: usize = 200;
/// 垃圾节的噪声行门槛：节内至少这么多行命中出版词 → 丢。
const JUNK_MIN_NOISE: usize = 2;
/// 目录未收录的节（多半是封面 / 版权页）要被当成正文收下，最少得有这么多字。
const ORPHAN_MIN_CHARS: usize = 400;
/// 「这一节是目录页」的判定：非空行里至少这个比例能在目录标题里对上号。
const TOC_PAGE_HIT_RATIO: f32 = 0.6;
/// 目录页判定的最小行数：少于这么几行的短节不劳烦判目录页。
const TOC_PAGE_MIN_LINES: usize = 5;
/// 起点定位的保护：算出来的起点若落在全文这个比例之后，认为是误判，不动。
const START_MAX_RATIO: f32 = 0.3;
/// 起点定位的绝对行保护（小册子用，见 [`cut_to_body_start`]）。
const START_ABS_MAX_LINES: usize = 30;

/// EPUB 解析器。
pub struct EpubParser;

impl FormatParser for EpubParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let file = std::fs::File::open(path).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| ImportError::Zip(e.to_string()))?;

        // ① 找 .opf
        let mut opf_path = read_container_opf(&mut zip);
        if opf_path.is_none() {
            // 退路：zip 里第一个 .opf
            opf_path = find_first(&mut zip, |n| n.to_ascii_lowercase().ends_with(".opf"));
        }

        // ② spine 顺序 + manifest（拿不到就为空，走退路）
        let opf = match opf_path.as_deref() {
            Some(p) => read_opf(&mut zip, p),
            None => Opf::default(),
        };

        // ③ 目录（toc.ncx 优先，其次 nav.xhtml）
        let toc = match opf.ncx.as_deref() {
            Some(p) => read_ncx(&mut zip, p),
            None => Vec::new(),
        };
        let toc = if toc.is_empty() {
            match opf.nav.as_deref() {
                Some(p) => read_nav(&mut zip, p),
                None => Vec::new(),
            }
        } else {
            toc
        };

        // ④ 漏斗第一层：定出「收哪些节」
        let plan = build_plan(&opf, &toc);

        // ⑤ 逐节抽正文并过第二层（文本屏蔽）
        let mut out = String::new();
        let mut used = 0usize;
        for sec in &plan {
            let Some(raw) = read_section(&mut zip, &sec.name, true) else {
                continue;
            };
            // 噪声行必须数**清洗前**的原文：先删再数就数不到东西了
            // （题名页正是「删掉两行噪声后只剩一个书名」的那类节）。
            let noise = jiandu_core::purge::noise_line_count(&raw);
            let text = jiandu_core::purge::strip_pub_lines(&raw);
            if text.trim().is_empty() {
                continue;
            }
            if jiandu_core::purge::visible_chars(&text) < JUNK_MIN_CHARS
                && noise >= JUNK_MIN_NOISE
            {
                continue;
            }
            if !sec.strict && jiandu_core::purge::visible_chars(&text) < ORPHAN_MIN_CHARS {
                continue;
            }
            if is_toc_page(&text, &toc) {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&text);
            used += 1;
        }

        // ⑥ 漏斗第三层：正文起点定位（前两层都漏了时的兜底）
        if used > 0 {
            out = cut_to_body_start(&out, &toc);
        }

        // 一节都没抽到 → 退化成「全部 xhtml 按名排序」（仍先弃表 + 过文本层）
        if used == 0 {
            out.clear();
            for name in sorted_html_names(&mut zip) {
                if let Some(raw) = read_section(&mut zip, &name, true) {
                    let noise = jiandu_core::purge::noise_line_count(&raw);
                    let text = jiandu_core::purge::strip_pub_lines(&raw);
                    if text.trim().is_empty() {
                        continue;
                    }
                    if jiandu_core::purge::visible_chars(&text) < JUNK_MIN_CHARS
                        && noise >= JUNK_MIN_NOISE
                    {
                        continue;
                    }
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&text);
                    used += 1;
                }
            }
            out = cut_to_body_start(&out, &toc);
        }

        // ── v1.69.0 退路：整本都用 `<table>` 排版的劣质转换件 ──────────
        // 「表格整块丢弃」是死命令，但真有本子把全部正文塞进表格里，
        // 全本弃表后一个字都不剩。那时**保留表格文字**再抽一遍：排版再丑
        // 也比空书强，且这一步只在「弃表后真的空了」时才跑，绝不影响
        // 正常的本子。
        if used == 0 {
            for name in sorted_html_names(&mut zip) {
                if let Some(text) = read_section(&mut zip, &name, false) {
                    let text = jiandu_core::purge::strip_pub_lines(&text);
                    if text.trim().is_empty() {
                        continue;
                    }
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&text);
                    used += 1;
                }
            }
        }

        if used == 0 {
            return Err(ImportError::Xml(
                "整本没有一个能读的 XHTML 节（可能是加密 EPUB 或纯图片扫描版）".to_string(),
            ));
        }
        Ok(out)
    }

    fn extensions() -> &'static [&'static str] {
        &["epub"]
    }
}

// ----------------------------------------------------------------------
// 第一层：结构过滤
// ----------------------------------------------------------------------

/// OPF 里读出来的书脊信息。
#[derive(Default)]
struct Opf {
    /// OPF 所在目录（`OEBPS/`），href 都相对它。
    dir: String,
    /// spine 顺序（相对 [`Opf::dir`] 的 href）。
    spine: Vec<String>,
    /// `toc.ncx` 的路径（zip 内全路径）。
    ncx: Option<String>,
    /// EPUB3 的 `nav.xhtml`（zip 内全路径）。
    nav: Option<String>,
    /// `dc:title`（书名）：目录里与它同名的那一项是书名页，不是章节。
    title: String,
    /// manifest 里 href → properties（`nav` / `cover-image` …）。
    props: HashMap<String, String>,
}

/// 目录里的一条（章节名 → 指向的文件）。
struct TocEntry {
    title: String,
    /// zip 内全路径。
    href: String,
}

/// 待抽取的一节。
struct Section {
    /// zip 内全路径。
    name: String,
    /// 目录给的章节名（目录没收录时为 `None`）。
    title: Option<String>,
    /// **目录明确收录**（true）/ mere spine 里有但目录没列（false）。
    strict: bool,
}

/// 读 OPF：manifest + spine + 目录文件位置。
fn read_opf(zip: &mut zip::ZipArchive<std::fs::File>, opf: &str) -> Opf {
    let Ok(mut f) = zip.by_name(opf) else {
        return Opf {
            dir: dir_of(opf),
            ..Opf::default()
        };
    };
    let mut s = String::new();
    if f.read_to_string(&mut s).is_err() {
        return Opf {
            dir: dir_of(opf),
            ..Opf::default()
        };
    }
    let dir = dir_of(opf);
    let mut map: HashMap<String, String> = HashMap::new();
    let mut spine: Vec<String> = Vec::new();
    let mut ncx_id: Option<String> = None;
    let mut props: HashMap<String, String> = HashMap::new();
    let mut in_dc_title = false;
    let mut dc_title = String::new();
    let mut out = Opf {
        dir: dir.clone(),
        ..Opf::default()
    };

    let mut xml = Reader::from_str(&s);
    xml.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let tag = local(&e.name().as_ref().to_vec());
                if tag == "title" {
                    in_dc_title = true;
                    dc_title.clear();
                } else if tag == "item" {
                    let (mut id, mut href, mut mt, mut properties) = (
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    );
                    for a in e.attributes().flatten() {
                        let k = local(&a.key.as_ref().to_vec());
                        let v = a.unescape_value().map(|s| s.to_string()).unwrap_or_default();
                        match k.as_str() {
                            "id" => id = v,
                            "href" => href = v,
                            "media-type" => mt = v,
                            "properties" => properties = v,
                            _ => {}
                        }
                    }
                    let href = percent_decode(&href);
                    if !href.is_empty() {
                        props.insert(href.clone(), properties.clone());
                    }
                    // EPUB2 的目录：media-type 是 ncx
                    if mt.contains("dtbncx") {
                        ncx_id = Some(id.clone());
                    }
                    // EPUB3 的目录：properties 含 nav
                    if properties.split_whitespace().any(|p| p == "nav") && !href.is_empty() {
                        out.nav = Some(join_zip_path(&dir, &href));
                    }
                    if !id.is_empty() && !href.is_empty() {
                        map.insert(id, href);
                    }
                } else if tag == "itemref" {
                    for a in e.attributes().flatten() {
                        if local(&a.key.as_ref().to_vec()) == "idref" {
                            if let Ok(v) = a.unescape_value() {
                                spine.push(v.to_string());
                            }
                        }
                    }
                } else if tag == "spine" {
                    // EPUB2：`<spine toc="ncx">` 的 toc 是 manifest 里的 id
                    for a in e.attributes().flatten() {
                        if local(&a.key.as_ref().to_vec()) == "toc" {
                            if let Ok(v) = a.unescape_value() {
                                if !v.is_empty() {
                                    ncx_id = Some(v.to_string());
                                }
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if in_dc_title {
                    dc_title.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                if local(&e.name().as_ref().to_vec()) == "title" {
                    in_dc_title = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out.title = collapse_ws(&dc_title);
    out.spine = spine
        .into_iter()
        .filter_map(|id| map.get(&id).cloned())
        .collect();
    out.props = props;
    if let Some(id) = ncx_id {
        if let Some(h) = map.get(&id) {
            out.ncx = Some(join_zip_path(&dir, h));
        }
    }
    out
}

/// 读 EPUB2 的 `toc.ncx`：`<navPoint><navLabel><text>…` + `<content src=…>`。
fn read_ncx(zip: &mut zip::ZipArchive<std::fs::File>, path: &str) -> Vec<TocEntry> {
    let Ok(mut f) = zip.by_name(path) else {
        return Vec::new();
    };
    let mut s = String::new();
    if f.read_to_string(&mut s).is_err() {
        return Vec::new();
    }
    let dir = dir_of(path);
    let mut out: Vec<TocEntry> = Vec::new();
    let mut cur_label = String::new();
    let mut cur_src: Option<String> = None;
    let mut in_label = false;

    let mut xml = Reader::from_str(&s);
    xml.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                match local(&e.name().as_ref().to_vec()).as_str() {
                    "navpoint" => {
                        cur_label.clear();
                        cur_src = None;
                    }
                    "navlabel" => in_label = true,
                    "content" => {
                        if cur_src.is_none() {
                            for a in e.attributes().flatten() {
                                if local(&a.key.as_ref().to_vec()) == "src" {
                                    if let Ok(v) = a.unescape_value() {
                                        cur_src = Some(percent_decode(&v));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if in_label {
                    cur_label.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                match local(&e.name().as_ref().to_vec()).as_str() {
                    "navlabel" => in_label = false,
                    "navpoint" => {
                        let title = collapse_ws(&cur_label);
                        if !title.is_empty() {
                            if let Some(src) = cur_src.take() {
                                out.push(TocEntry {
                                    title,
                                    href: join_zip_path(&dir, &strip_fragment(&src)),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// 读 EPUB3 的 `nav.xhtml`：`<nav><ol><li><a href=…>标题</a>`。
fn read_nav(zip: &mut zip::ZipArchive<std::fs::File>, path: &str) -> Vec<TocEntry> {
    let Ok(mut f) = zip.by_name(path) else {
        return Vec::new();
    };
    let mut s = String::new();
    if f.read_to_string(&mut s).is_err() {
        return Vec::new();
    }
    let dir = dir_of(path);
    let mut out: Vec<TocEntry> = Vec::new();
    let mut nav_depth = 0usize;
    let mut in_a = false;
    let mut cur_href = String::new();
    let mut cur_text = String::new();

    let mut xml = Reader::from_str(&s);
    xml.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let tag = local(&e.name().as_ref().to_vec());
                if tag == "nav" {
                    nav_depth += 1;
                } else if tag == "a" && nav_depth > 0 {
                    in_a = true;
                    cur_text.clear();
                    cur_href.clear();
                    for a in e.attributes().flatten() {
                        if local(&a.key.as_ref().to_vec()) == "href" {
                            if let Ok(v) = a.unescape_value() {
                                cur_href = percent_decode(&v);
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if in_a {
                    cur_text.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                let tag = local(&e.name().as_ref().to_vec());
                match tag.as_str() {
                    "nav" => nav_depth = nav_depth.saturating_sub(1),
                    "a" if in_a => {
                        in_a = false;
                        let title = collapse_ws(&cur_text);
                        if !title.is_empty() && !cur_href.is_empty() {
                            out.push(TocEntry {
                                title,
                                href: join_zip_path(&dir, &strip_fragment(&cur_href)),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// 定出「收哪些节」。
///
/// * 目录可信时：目录里叫 front / back matter（前言 / 目录 / 索引…）的节**整节不收**，
///   其余照 spine 顺序收；spine 里目录没列的节（封面 / 版权页之流）只在
///   「内容够多」时收——不能因为目录漏了一节就丢掉一整章。
/// * 目录读不出来时：spine 全收，交给第二层（文本屏蔽）与第三层（起点定位）。
/// * `epub:type` / 文件名带现代书籍标签的节一律不收。
fn build_plan(opf: &Opf, toc: &[TocEntry]) -> Vec<Section> {
    let mut excluded: HashSet<String> = HashSet::new();
    let mut allowed: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (i, e) in toc.iter().enumerate() {
        let name = norm_zip_name(&e.href);
        // 目录头两项若与书名同名（「尔雅」指向题名页那种），那是书名页不是章节。
        let is_book_itself = i < 2
            && !opf.title.is_empty()
            && (opf.title.starts_with(&e.title) || e.title.starts_with(&opf.title));
        if is_front_matter_title(&e.title) || is_book_itself {
            excluded.insert(name);
        } else {
            allowed.entry(name.clone()).or_insert_with(|| e.title.clone());
            if !order.contains(&name) {
                order.push(name);
            }
        }
    }

    let mut plan: Vec<Section> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for rel in &opf.spine {
        let name = norm_zip_name(&join_zip_path(&opf.dir, rel));
        if seen.contains(&name) {
            continue;
        }
        // 现代书籍标签：epub:type 或文件名
        if is_modern_section(&name, opf) {
            continue;
        }
        if excluded.contains(&name) {
            continue;
        }
        seen.insert(name.clone());
        let title = allowed.get(&name).cloned();
        let strict = title.is_some() || toc.is_empty();
        plan.push(Section {
            name,
            title,
            strict,
        });
    }

    // 目录里有、spine 里没有的节（有的书 spine 不全）：补在末尾，按目录顺序。
    for name in order {
        if seen.contains(&name) || excluded.contains(&name) {
            continue;
        }
        let title = allowed.get(&name).cloned();
        seen.insert(name.clone());
        plan.push(Section {
            name,
            title,
            strict: true,
        });
    }
    plan
}

/// 目录项名是不是 front / back matter（core 的判据 + 「与书名同名」）。
fn is_front_matter_title(title: &str) -> bool {
    jiandu_core::purge::is_front_matter_title(title)
}

/// 这一节是不是现代书籍部件：文件名 / `epub:type` 露了馅。
fn is_modern_section(name: &str, opf: &Opf) -> bool {
    let lower = name.to_ascii_lowercase();
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    // 文件名判据：cover / title / copyright / colophon / toc / nav / imprint
    if [
        "cover", "titlepage", "title", "copyright", "colophon", "imprint", "toc", "nav",
        "halftitle", "dedication", "acknowledgements", "about",
    ]
    .iter()
    .any(|k| base.contains(k))
    {
        return true;
    }
    // epub:type 判据（manifest 的 properties）
    if let Some(p) = opf.props.get(name).or_else(|| {
        let rel = name.strip_prefix(&opf.dir).unwrap_or(name);
        opf.props.get(rel)
    }) {
        let p = p.to_ascii_lowercase();
        if [
            "titlepage",
            "copyright-page",
            "colophon",
            "imprint",
            "halftitlepage",
            "cover",
            "toc",
            "frontmatter",
            "dedication",
            "acknowledgments",
        ]
        .iter()
        .any(|k| p.contains(k))
        {
            return true;
        }
    }
    false
}

/// 这一节是不是**目录页**：大半行都能在目录标题里对上号。
fn is_toc_page(text: &str, toc: &[TocEntry]) -> bool {
    if toc.is_empty() {
        return false;
    }
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if lines.len() < TOC_PAGE_MIN_LINES {
        return false;
    }
    let mut hit = 0usize;
    for l in &lines {
        if toc.iter().any(|e| !e.title.is_empty() && l.starts_with(&e.title)) {
            hit += 1;
        }
    }
    hit as f32 >= lines.len() as f32 * TOC_PAGE_HIT_RATIO
}

// ----------------------------------------------------------------------
// 第三层：启发式正文起点
// ----------------------------------------------------------------------

/// 把「正文真正开始」之前的内容整段砍掉。
///
/// 先看**目录锚点**：第一个正经章节名（如「释诂第一」）在文本里第一次作为
/// 独立行出现的位置，就是起点——这是最硬的证据。目录不可用时退到 core 的
/// 文言度扫描（[`jiandu_core::purge::find_body_start`]）。
///
/// 两道保险：① 起点必须落在全文前 30%（[`START_MAX_RATIO`]）之内，否则认为
/// 是误判、原样返回；② 认不出起点就返回原文——**宁可留着脏，不能砍掉真正文**。
fn cut_to_body_start(text: &str, toc: &[TocEntry]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.to_string();
    }
    let mut start: Option<usize> = None;

    // ① 目录锚点
    for e in toc {
        if is_front_matter_title(&e.title) || e.title.is_empty() {
            continue;
        }
        if let Some(i) = lines
            .iter()
            .position(|l| l.trim() == e.title || l.trim().starts_with(&e.title))
        {
            start = Some(i);
            break;
        }
    }
    // ② 文言度扫描
    if start.is_none() {
        let i = jiandu_core::purge::find_body_start(&lines);
        if i > 0 {
            start = Some(i);
        }
    }

    // 保护：起点要么落在全文前 30%，要么至少在前 30 行内——小册子（几十行）
    // 不能套百分比，否则「前言只占头 5 行」也会被判成误判而放过去。
    match start {
        Some(i)
            if i > 0
                && ((i as f32) < lines.len() as f32 * START_MAX_RATIO
                    || i < START_ABS_MAX_LINES) =>
        {
            lines[i..].join("\n")
        }
        _ => text.to_string(),
    }
}

// ----------------------------------------------------------------------
// 底层工具
// ----------------------------------------------------------------------

/// 只取 EPUB 的**书名**（`dc:title`）——开一次 zip，读完就走。
///
/// 为什么非要单独开一道口子：净化漏斗把题名页 / 版权页全丢了，正文首行
/// 就成了「释诂第一」这种篇题，`detect_title_author` 会把它误认成书名
/// （实测《尔雅》被叫成了「释诂第一」）。元数据里的 `dc:title` 才是真书名。
/// **不取 `dc:creator`**：现代古籍书的 creator 是译注者 / 整理者（「管锡华」），
/// 那正是要杀灭的东西，宁可留空。
pub fn read_epub_title(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    let opf_path = read_container_opf(&mut zip)
        .or_else(|| find_first(&mut zip, |n| n.to_ascii_lowercase().ends_with(".opf")))?;
    let opf = read_opf(&mut zip, &opf_path);
    let t = jiandu_core::vclean::main_title_of(&opf.title);
    if t.trim().is_empty() {
        None
    } else {
        Some(t)
    }
}

/// 读 `META-INF/container.xml`，取出第一个 `rootfile` 的 `full-path`。
fn read_container_opf(zip: &mut zip::ZipArchive<std::fs::File>) -> Option<String> {
    let mut f = zip.by_name("META-INF/container.xml").ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    let mut xml = Reader::from_str(&s);
    xml.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local(&e.name().as_ref().to_vec()) == "rootfile" {
                    for a in e.attributes().flatten() {
                        if local(&a.key.as_ref().to_vec()) == "full-path" {
                            let v = a.unescape_value().ok()?.to_string();
                            return Some(v);
                        }
                    }
                }
            }
            Ok(Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

/// 打开 zip 里的某一节并抽正文（流式：`BufReader` 直接喂给 XML 解析器）。
///
/// `drop_tables = true` 时**整块丢弃 `<table>`**（v1.69.0 死命令）；
/// `false` 是「弃表后整本空了」的救回退路。
fn read_section(
    zip: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
    drop_tables: bool,
) -> Option<String> {
    if !is_html_name(name) {
        return None;
    }
    let f = zip.by_name(name).ok()?;
    let (text, _err) = if drop_tables {
        extract_html_reader(BufReader::new(f))
    } else {
        extract_html_reader_keep_tables(BufReader::new(f))
    };
    Some(text)
}

/// zip 里所有 HTML 节，按文件名排序（退路用）。
fn sorted_html_names(zip: &mut zip::ZipArchive<std::fs::File>) -> Vec<String> {
    let mut names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| is_html_name(n))
        .collect();
    names.sort();
    names
}

fn is_html_name(n: &str) -> bool {
    let l = n.to_ascii_lowercase();
    l.ends_with(".xhtml") || l.ends_with(".html") || l.ends_with(".htm")
}

/// zip 内路径规范化：去 `./`、去 `#fragment`。
fn norm_zip_name(s: &str) -> String {
    strip_fragment(s.trim_start_matches("./"))
}

fn strip_fragment(s: &str) -> String {
    match s.split_once('#') {
        Some((a, _)) => a.to_string(),
        None => s.to_string(),
    }
}

/// 目录文本归一：连续空白折成一个空格、去首尾。
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// zip 内路径拼接（`OEBPS/content.opf` 相对的是 `OEBPS/`）。
fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..=i].to_string(),
        None => String::new(),
    }
}

fn join_zip_path(dir: &str, rel: &str) -> String {
    let rel = rel.trim_start_matches("./");
    if dir.is_empty() {
        return rel.to_string();
    }
    format!("{dir}{rel}")
}

fn find_first<F: Fn(&str) -> bool>(
    zip: &mut zip::ZipArchive<std::fs::File>,
    pred: F,
) -> Option<String> {
    (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().map(|f| f.name().to_string()))
        .find(|n| pred(n))
}

fn local(q: &[u8]) -> String {
    let s = String::from_utf8_lossy(q);
    s.rsplit(':').next().unwrap_or("").to_ascii_lowercase()
}

/// `%E7%AC%AC` → `第`（OPF 里的 href 常带百分号转义）。
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_section_names_are_rejected() {
        let opf = Opf::default();
        for n in [
            "OEBPS/Text/cover_page.xhtml",
            "OEBPS/Text/titlepage.xhtml",
            "OEBPS/Text/copyright.xhtml",
            "OEBPS/Text/colophon.xhtml",
            "OEBPS/Text/nav.xhtml",
        ] {
            assert!(is_modern_section(n, &opf), "未识别出：{n}");
        }
        assert!(!is_modern_section("OEBPS/Text/part0003.xhtml", &opf));
        assert!(!is_modern_section("OEBPS/Text/chapter1.xhtml", &opf));
    }

    #[test]
    fn front_matter_titles_are_excluded_from_the_plan() {
        let opf = Opf {
            dir: "OEBPS/".to_string(),
            title: "尔雅——中华经典名著全本全注全译丛书".to_string(),
            spine: vec![
                "Text/cover_page.xhtml".into(),
                "Text/part0000.xhtml".into(),
                "Text/part0001.xhtml".into(),
                "Text/part0002.xhtml".into(),
                "Text/part0003.xhtml".into(),
                "Text/part0022.xhtml".into(),
            ],
            ncx: None,
            nav: None,
            props: HashMap::new(),
        };
        let toc = vec![
            TocEntry {
                title: "尔雅".into(),
                href: "OEBPS/Text/part0000.xhtml".into(),
            },
            TocEntry {
                title: "前言".into(),
                href: "OEBPS/Text/part0002.xhtml".into(),
            },
            TocEntry {
                title: "目录".into(),
                href: "OEBPS/Text/part0001.xhtml".into(),
            },
            TocEntry {
                title: "释诂第一".into(),
                href: "OEBPS/Text/part0003.xhtml".into(),
            },
            TocEntry {
                title: "《尔雅》词语笔画索引".into(),
                href: "OEBPS/Text/part0022.xhtml".into(),
            },
        ];
        let plan = build_plan(&opf, &toc);
        let names: Vec<&str> = plan.iter().map(|s| s.name.as_str()).collect();
        // cover_page（现代名）+ 前言 + 目录 + 索引 + 书名页 全被过滤，
        // 只留真正的第一篇。
        assert_eq!(names, vec!["OEBPS/Text/part0003.xhtml"], "{names:?}");
        assert_eq!(plan[0].title.as_deref(), Some("释诂第一"));
        assert!(plan[0].strict);
    }

    #[test]
    fn orphan_sections_are_kept_but_marked_non_strict() {
        let opf = Opf {
            dir: "OEBPS/".to_string(),
            spine: vec!["Text/part0003.xhtml".into(), "Text/extra.xhtml".into()],
            ncx: None,
            nav: None,
            title: String::new(),
            props: HashMap::new(),
        };
        let toc = vec![TocEntry {
            title: "释诂第一".into(),
            href: "OEBPS/Text/part0003.xhtml".into(),
        }];
        let plan = build_plan(&opf, &toc);
        assert_eq!(plan.len(), 2);
        assert!(plan[0].strict);
        assert!(!plan[1].strict, "目录没列的节不该被当硬证据");
    }

    #[test]
    fn without_toc_everything_is_strict() {
        let opf = Opf {
            dir: "OEBPS/".to_string(),
            spine: vec!["Text/part0000.xhtml".into(), "Text/part0003.xhtml".into()],
            ncx: None,
            nav: None,
            title: String::new(),
            props: HashMap::new(),
        };
        let plan = build_plan(&opf, &[]);
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|s| s.strict), "无目录时不许大面积丢弃");
    }

    #[test]
    fn toc_page_is_detected_by_title_overlap() {
        // 目录页的条目应当都在 toc 里（真书如此）。
        let toc = vec![
            TocEntry { title: "释诂第一".into(), href: "OEBPS/Text/part0003.xhtml".into() },
            TocEntry { title: "释言第二".into(), href: "OEBPS/Text/part0004.xhtml".into() },
            TocEntry { title: "释训第三".into(), href: "OEBPS/Text/part0005.xhtml".into() },
            TocEntry { title: "释亲第四".into(), href: "OEBPS/Text/part0006.xhtml".into() },
            TocEntry { title: "释宫第五".into(), href: "OEBPS/Text/part0007.xhtml".into() },
        ];
        let page = "目  录\n释诂第一\n释言第二\n释训第三\n释亲第四\n释宫第五";
        assert!(is_toc_page(page, &toc), "目录页没认出来");
        let body = "初、哉、首、基、肈、祖、元、胎、俶、落、权舆，始也。\n林、烝、天、帝、皇、王、后、辟、公、侯，君也。\n如、適、之、嫁、徂、逝，往也。\n赉、贡、锡、畀、予、贻，赐也。\n仪、若、祥、淑、鲜、省、臧、嘉、令、类、綝、彀、攻、穀、介、徽，善也。";
        assert!(!is_toc_page(body, &toc), "正文被当成目录页");
    }

    #[test]
    fn body_start_cuts_the_front_matter_by_toc_anchor() {
        let toc = vec![TocEntry {
            title: "释诂第一".into(),
            href: "OEBPS/Text/part0003.xhtml".into(),
        }];
        let text = concat!(
            "尔雅\n",
            "全本全注全译丛书\n",
            "管锡华◎译注\n",
            "前言\n",
            "《尔雅》是第一部内容和体例都比较完备的汉语词典。\n",
            "释诂第一\n",
            "初、哉、首、基、肈、祖、元、胎、俶、落、权舆，始也。\n",
        );
        let out = cut_to_body_start(text, &toc);
        assert!(out.starts_with("释诂第一"), "起点没落在首章：{out}");
        assert!(!out.contains("译注"), "译注没杀掉：{out}");
    }

    #[test]
    fn body_start_never_cuts_more_than_the_head() {
        let text: String = (0..100)
            .map(|i| format!("第{i}行　初、哉、首、基、肈、祖、元、胎，始也。\n"))
            .collect();
        // 起点若落在 30% 之后（误判），原样返回。
        let out = cut_to_body_start(&text, &[]);
        assert_eq!(out.lines().count(), text.lines().count());
    }

    /// **真书验证**（v1.80.0）：拿一本真实的现代出版古籍 EPUB 走一遍漏斗。
    ///
    /// 默认不跑（读不到环境变量就直接返回），跑法：
    ///
    /// ```text
    /// JIANDU_EPUB_CHECK=/path/to/尔雅.epub \
    ///   cargo test -p jiandu epub_funnel_on_a_real_book -- --ignored --nocapture
    /// ```
    ///
    /// 钉三件事：① 现代出版文字**一个都不剩**；② 真正的经文还在；
    /// ③ 章节数没被误删成零。
    #[test]
    #[ignore]
    fn epub_funnel_on_a_real_book() {
        let Ok(path) = std::env::var("JIANDU_EPUB_CHECK") else {
            println!("（未设 JIANDU_EPUB_CHECK，跳过真书验证）");
            return;
        };
        let text = match EpubParser::parse(std::path::Path::new(&path)) {
            Ok(t) => t,
            Err(e) => panic!("解析失败：{e}"),
        };
        let lines: Vec<&str> = text.lines().collect();
        println!("── 漏斗产出 ──────────────────────────");
        println!("行数 {}  可见字 {}", lines.len(), jiandu_core::purge::visible_chars(&text));
        println!("前 8 行：");
        for l in lines.iter().take(8) {
            println!("  | {l}");
        }
        println!("末 3 行：");
        for l in lines.iter().rev().take(3).rev() {
            println!("  | {l}");
        }

        // ① 现代出版文字必须死绝
        for bad in [
            "管锡华",
            "译注",
            "全本全注全译",
            "中华书局",
            "出版社",
            "ISBN",
            "责任编辑",
            "图书在版编目",
            "版权所有",
            "z-library",
            "前言",
            "笔画索引",
        ] {
            assert!(!text.contains(bad), "「{bad}」没被杀干净");
        }
        // ② 真正的经文还在（且没被砍掉大半）
        assert!(
            jiandu_core::purge::visible_chars(&text) > 20,
            "正文被砍空了：只剩「{text}」"
        );
        // 样本书可能只有几行（`samples/daodejing-toc.epub` 就 4 行），
        // 门槛按「没被砍空」定，不按具体行数。
        assert!(lines.len() >= 3, "正文被砍得太狠：只剩 {} 行", lines.len());
        // ③ 首行就是正经篇题或经文，不是出版信息
        let first = lines.first().unwrap_or(&"").trim();
        assert!(
            !jiandu_core::purge::is_pub_noise(first),
            "首行仍是出版噪声：{first}"
        );

        // ④ 端到端：走完整漏斗（清洗 → 断章），书名单与章节都得干净
        let ir = match crate::importer::import_file(std::path::Path::new(&path)) {
            Ok(b) => b,
            Err(e) => panic!("导入失败：{e}"),
        };
        println!("── 端到端 ──────────────────────────");
        println!("书名「{}」作者「{}」卷数 {}", ir.meta.title, ir.meta.author, ir.juans.len());
        for j in ir.juans.iter().take(8) {
            println!("  卷：{}（{} 块）", j.title, j.blocks.len());
        }
        for bad in ["管锡华", "译注", "全本全注全译", "z-library", "中华书局"] {
            assert!(!ir.meta.title.contains(bad), "书名里还有「{bad}」：{}", ir.meta.title);
        }
        // 样本书只有两三卷，门槛按「切出了卷」定；《尔雅》那类真书会切出 19 卷。
        assert!(!ir.juans.is_empty(), "一卷都没切出来");
    }

    #[test]
    fn names_and_paths_are_normalized() {
        assert_eq!(norm_zip_name("./Text/part0003.xhtml#a"), "Text/part0003.xhtml");
        assert_eq!(
            join_zip_path("OEBPS/", "./Text/part0003.xhtml"),
            "OEBPS/Text/part0003.xhtml"
        );
        assert_eq!(collapse_ws(" 目  录 \n"), "目 录");
    }
}
