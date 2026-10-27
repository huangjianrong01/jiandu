//! **批量导入**（v1.77.0）：点检 → 串行刻印 → 可取消 → 留错误日志。
//!
//! # 为什么分成「点检」和「刻印」两趟
//!
//! 一本 20MB 的 EPUB 全量解析要跑好几秒；一个文件夹里躺着十几本，
//! 挨个刻完用户要等好几分钟，中间书架还是空的——看着就像卡死了。
//!
//! 所以第一趟只做**点检**：解 OPF 拿书名 / 作者 / 目录（不解一节正文，
//! 毫秒级），让这些书**立刻以真书名出现在书架上**（待刻书衣）。第二趟才
//! 由后台线程一本一本地刻正文、落库，刻完一本那张书衣就变成真书。
//!
//! 点检另一桩好处：坏文件在开刻**之前**就被认出来剔掉（而不是刻到一半
//! 崩了才知道），队列里剩下的都是大概率能成的。
//!
//! # 三条铁律
//!
//! 1. **绝不在主线程干重活**：刻印在调用方的线程里跑（`BatchQueue::drive`
//!    是同步的，由调用方决定扔到哪条线程上），主线程只每帧来探一眼。
//! 2. **一本崩了不许带崩整队**：每本各自 `catch_unwind`，panic 收成一条
//!    失败记录；损坏的 EPUB / 不支持的格式一律跳过并记进日志。
//! 3. **取消要清干净**：取消后，已经刻完但还没上架的、以及正在刻的那一本，
//!    其 `store_id` 全部进 [`BatchQueue::orphans`]，由调用方删库——不留
//!    半成品在 `chapters.db` 里躺着。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// 管线认得的全部扩展名（与 `importer::import_from_path` 的分派表同源）。
///
/// 扫文件夹时按这张表过滤——混进来的 `.DS_Store` / `.pdf` / 图片都在这一关
/// 被剔掉，既不占队列名额，也不进错误日志（它们本来就不是书）。
pub const SUPPORTED_EXTS: &[&str] = &[
    "md", "markdown", "txt", "text", "html", "htm", "xhtml", "xml", "epub", "docx", "json",
];

/// 一个文件夹最多吃多少本（防呆：有人会直接选整个「下载」目录）。
pub const MAX_FILES: usize = 500;

/// 目录最多取多少条（只用于展示「共 N 章」，多了没意义还拖慢点检）。
const MAX_TOC: usize = 200;

/// 这个文件管线认得吗（只看扩展名，不打开）。
pub fn is_importable(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    SUPPORTED_EXTS.contains(&ext.as_str())
}

/// **点检**的结果：一本书的「脸面」——元数据 + 目录，不含正文。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    /// 点检到的书名（拿不到就退回文件名）。
    pub title: String,
    pub author: String,
    /// 章题列表（拿不到为空；书架上的待刻书衣显示「共 N 章」用）。
    pub toc: Vec<String>,
    /// 文件大小（字节）。
    pub bytes: u64,
}

impl Candidate {
    /// 展示用的那一行（进度浮层与日志都用这个）。
    pub fn display_name(&self) -> &str {
        if self.title.trim().is_empty() {
            self.path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("无名")
        } else {
            self.title.as_str()
        }
    }
}

/// **点检**：只读元数据与目录，一个字的正文都不解。
///
/// 失败（打不开 / 不是合法容器 / 加密 EPUB）返回一句人话的 `Err`——
/// **绝不 panic**，调用方据此把它剔出队列。
pub fn peek(path: &Path) -> Result<Candidate, String> {
    let bytes = std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or(0);
    let fallback = crate::importer::file_stem(path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "epub" => peek_epub(path, fallback, bytes),
        // txt / md：读开头一小段，第一行看着像标题就当书名。只挑一丁点读，
        // 百万字的大书也不至于为个书名把整个文件扫一遍。
        "txt" | "text" | "md" | "markdown" => peek_plain(path, fallback, bytes),
        // 其余（html / docx / json …）：书名就是文件名，交给刻印那一趟去定夺。
        _ => Ok(Candidate {
            path: path.to_path_buf(),
            title: fallback,
            author: String::new(),
            toc: Vec::new(),
            bytes,
        }),
    }
}

/// EPUB 点检：解 `META-INF/container.xml` → OPF → `dc:title` / `dc:creator`，
/// 再顺手摸一下目录（ncx 优先，其次 nav 文档）。**一节正文都不抽**。
fn peek_epub(path: &Path, fallback: String, bytes: u64) -> Result<Candidate, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打不开：{e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "不是合法的 zip 容器（EPUB 损坏？）".to_string())?;

    let container = read_zip(&mut zip, "META-INF/container.xml")
        .ok_or_else(|| "缺少 META-INF/container.xml（不是标准 EPUB）".to_string())?;
    let opf_path = attr_value(&container, "full-path")
        .ok_or_else(|| "container.xml 里没有 full-path".to_string())?;
    let opf = read_zip(&mut zip, &opf_path)
        .ok_or_else(|| "读不出 OPF（EPUB 目录损坏）".to_string())?;

    let title = crate::vclean::clean_book_title(
        &tag_text(&opf, "dc:title").unwrap_or(fallback),
    );
    let author = tag_text(&opf, "dc:creator").unwrap_or_default();

    // 目录：manifest 里有 `properties="nav"` 的（EPUB3），或老式的 ncx。
    let items = opf_items(&opf);
    let mut toc: Vec<String> = Vec::new();
    if let Some((_, (href, _, _))) = items.iter().find(|(_, (_, _, props))| props.contains("nav")) {
        if let Some(nav) = read_zip(&mut zip, &join_dir(&opf_path, href)) {
            toc = nav_titles(&nav);
        }
    }
    if toc.is_empty() {
        if let Some((_, (href, _, _))) = items
            .iter()
            .find(|(_, (_, mt, _))| mt == "application/x-dtbncx+xml")
        {
            if let Some(ncx) = read_zip(&mut zip, &join_dir(&opf_path, href)) {
                toc = ncx_titles(&ncx);
            }
        }
    }
    // 兜底：ncx / nav 都没有，就用 spine 的节数报个「共 N 章」。
    if toc.is_empty() {
        let n = spine_count(&opf);
        if n > 0 {
            toc = (1..=n).map(|i| format!("第 {i} 节")).collect();
        }
    }
    if toc.len() > MAX_TOC {
        toc.truncate(MAX_TOC);
    }

    Ok(Candidate {
        path: path.to_path_buf(),
        title: if title.trim().is_empty() {
            crate::importer::file_stem(path)
        } else {
            title
        },
        author,
        toc,
        bytes,
    })
}

/// txt / md 点检：只读开头 4KB，第一行像标题就拿它当书名。
fn peek_plain(path: &Path, fallback: String, bytes: u64) -> Result<Candidate, String> {
    let mut buf = vec![0u8; 4096.min(bytes.max(1) as usize)];
    let n = {
        let Ok(mut f) = std::fs::File::open(path) else {
            return Err("打不开".to_string());
        };
        f.read(&mut buf).unwrap_or(0)
    };
    buf.truncate(n);
    // 老书多为 GBK：先按 UTF-8 试，不成再转。书名乱码也比报错强。
    let head = match String::from_utf8(buf.clone()) {
        Ok(s) => s,
        Err(_) => {
            let (cow, _, _) = encoding_rs::GBK.decode(&buf);
            cow.into_owned()
        }
    };
    let first = head
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    // 太长的第一行多半是正文而不是书名（有的 txt 没有标题行）。
    let title = if first.chars().count() <= 40 {
        crate::vclean::clean_book_title(&first)
    } else {
        String::new()
    };
    Ok(Candidate {
        path: path.to_path_buf(),
        title: if title.trim().is_empty() { fallback } else { title },
        author: String::new(),
        toc: Vec::new(),
        bytes,
    })
}

/// 扫一个目录，收集可导入的文件（**不递归子目录**时只看这一层）。
///
/// 结果按文件名排序——队列顺序稳定，用户才能对得上进度条上的「第几本」。
/// 超过 [`MAX_FILES`] 就截断（选到「下载」目录这种事真的会发生）。
pub fn collect(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if recursive && out.len() < MAX_FILES {
                let mut sub = collect(&p, true);
                out.append(&mut sub);
            }
            continue;
        }
        if is_importable(&p) && out.len() < MAX_FILES {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// 一本的结局：上架了（带 `store_id`，取消时要靠它清库）还是跳过了。
#[derive(Debug, Clone)]
pub enum ItemResult {
    /// 刻成并已落库。`store_id` 为空表示走的是漏斗兜底（整本入库）。
    Ok { title: String, store_id: String },
    /// 跳过：损坏 / 不支持 / 解析崩溃。带一句人话的原因。
    Skip { title: String, reason: String },
}

impl ItemResult {
    pub fn ok(&self) -> bool {
        matches!(self, ItemResult::Ok { .. })
    }
    pub fn title(&self) -> &str {
        match self {
            ItemResult::Ok { title, .. } => title,
            ItemResult::Skip { title, .. } => title,
        }
    }
    /// 日志一行：成功写「已上架」，跳过写原因。
    pub fn log_line(&self) -> String {
        match self {
            ItemResult::Ok { title, .. } => format!("已上架：{title}"),
            ItemResult::Skip { title, reason } => format!("跳过「{title}」：{reason}"),
        }
    }
}

/// **批量导入队列**：一本接一本，串行推进，随时可取消。
///
/// 它自己不开线程——`drive` 是同步的，由调用方丢到后台线程上跑。这样
/// 单测里可以塞一个假的「刻印函数」进来，把取消 / 崩溃 / 跳过全测一遍。
pub struct BatchQueue {
    items: Vec<Candidate>,
    /// 下一本的下标。
    pos: usize,
    /// **点检就没过**的那些（不占书衣位，但也要进日志）。
    pre: Vec<ItemResult>,
    /// 已结清的记录（成功 + 跳过，按完成顺序）。
    results: Vec<ItemResult>,
    /// 用户按下取消。
    canceled: bool,
    /// 取消时**已经刻完但还没上架**的 `store_id`——调用方要删库。
    orphans: Vec<String>,
    /// 正在刻的第几本（1 起；0 表示还没开刻）。
    in_flight: usize,
}

impl BatchQueue {
    pub fn new(items: Vec<Candidate>) -> Self {
        Self {
            items,
            pos: 0,
            pre: Vec::new(),
            results: Vec::new(),
            canceled: false,
            orphans: Vec::new(),
            in_flight: 0,
        }
    }

    pub fn total(&self) -> usize {
        self.items.len()
    }
    /// 已结清多少本（含跳过）。
    pub fn settled(&self) -> usize {
        self.results.len()
    }
    pub fn canceled(&self) -> bool {
        self.canceled
    }
    /// 取消后待清理的半成品 `store_id`。
    pub fn orphans(&self) -> &[String] {
        &self.orphans
    }
    pub fn results(&self) -> &[ItemResult] {
        &self.results
    }
    /// 记一条「点检就没过」的书（损坏 / 不支持，压根没进队列）。
    ///
    /// 它不占书衣位（本来也没点检出来），但要进错误日志——用户得知道
    /// 自己那个 `.epub` 为什么没出现在书架上。
    pub fn record_pre(&mut self, r: ItemResult) {
        self.pre.push(r);
    }
    /// 全部结局（点检淘汰的在前，其后按刻印顺序）。
    pub fn all_results(&self) -> Vec<ItemResult> {
        let mut v = self.pre.clone();
        v.extend(self.results.iter().cloned());
        v
    }
    /// 还沒刻的（取消之后剩下的那些）。
    pub fn remaining(&self) -> &[Candidate] {
        &self.items[self.pos..]
    }
    /// 正在刻的那本（取消后为 `None`）。
    pub fn current(&self) -> Option<&Candidate> {
        if self.canceled || self.pos >= self.items.len() {
            None
        } else {
            self.items.get(self.pos)
        }
    }
    /// 正在刻第几本（1 起；用于「正在导入 3/10」）。
    pub fn current_no(&self) -> usize {
        self.in_flight
    }

    pub fn cancel(&mut self) {
        self.canceled = true;
    }

    /// 逐本推进：`etch` 刻一本，返回 `store_id`（成功）或一句原因（跳过）。
    ///
    /// - 每本各自 `catch_unwind`：**一本 panic 只记一条失败，队列继续**。
    /// - 每本开刻前先看 `stop`；用户按了取消，当前这本刻完就收工（它的
    ///   `store_id` 进 `orphans`，不会被上架）。
    /// - `on_tick(i, n, name)` 用来把进度报到界面上。
    pub fn drive<E, T>(&mut self, stop: &AtomicBool, etch: &mut E, on_tick: &mut T)
    where
        E: FnMut(&Candidate, usize, usize) -> Result<String, String>,
        T: FnMut(usize, usize, &str),
    {
        let total = self.items.len();
        while self.pos < total {
            if stop.load(Ordering::Relaxed) {
                self.canceled = true;
                break;
            }
            let c = self.items[self.pos].clone();
            self.in_flight = self.pos + 1;
            on_tick(self.in_flight, total, c.display_name());

            // ── 内存护栏（v1.81.0）─────────────────────────────────
            // 导入是最吃内存的一环：解 zip、解析 XML、清洗、排版全在内存里
            // 摊开。上一本刚刻完，堆还没回落，紧接着开下一本最容易把机器
            // 拖进换页。这里在**本线程**（后台）里等它降下来——排队等的是
            // 后台线程，主线程照样能翻页。
            crate::mem::breathe("batch-import");

            // ── 容灾：一本崩了不许带崩整队 ──────────────────────────
            // `catch_unwind` 把 panic 收成一条普通失败。没有这一层，一个
            // 畸形 EPUB 就能让剩下的九本书永远躺在队列里没人管。
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| etch(&c, self.pos, total)));
            // 取消可能就发生在这一本刻到一半的时候（`stop` 在刻印途中被置起）。
            // 这一本虽然刻成了，但用户没要它——不许上架，排队等删。
            if stop.load(Ordering::Relaxed) {
                self.canceled = true;
            }
            match r {
                Ok(Ok(sid)) => {
                    if self.canceled {
                        // 取消发生在刻印途中：这半本不许上架，排队等删。
                        if !sid.is_empty() {
                            self.orphans.push(sid.clone());
                        }
                    }
                    self.results.push(ItemResult::Ok {
                        title: c.display_name().to_string(),
                        store_id: sid,
                    });
                }
                Ok(Err(reason)) => self.results.push(ItemResult::Skip {
                    title: c.display_name().to_string(),
                    reason,
                }),
                Err(_) => self.results.push(ItemResult::Skip {
                    title: c.display_name().to_string(),
                    reason: "解析器在这本书上崩溃了（多半是文件损坏或非标准结构）".to_string(),
                }),
            }
            self.pos += 1;
        }
        self.in_flight = 0;
    }

    /// 收工汇报：一句人话（「已上架 8 本，跳过 2 本」）。
    pub fn summary(&self) -> String {
        let all = self.all_results();
        let ok = all.iter().filter(|r| r.ok()).count();
        let skip = all.len() - ok;
        if self.canceled {
            return format!(
                "已取消：上架 {ok} 本{}，剩 {} 本没刻",
                if skip > 0 {
                    format!("，跳过 {skip} 本")
                } else {
                    String::new()
                },
                self.remaining().len()
            );
        }
        if skip == 0 {
            format!("已上架 {ok} 本")
        } else {
            format!("已上架 {ok} 本，跳过 {skip} 本")
        }
    }
}

// ── OPF / ncx 的极简读取（故意不引 XML 库：只要书名和目录）────────────

fn read_zip<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    let mut f = zip.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

/// 取某属性的第一个值（`full-path="…"`）。
fn attr_value(xml: &str, attr: &str) -> Option<String> {
    let mut rest = xml;
    while let Some(i) = rest.find(attr) {
        let after = &rest[i + attr.len()..];
        let after = after.trim_start();
        if let Some(stripped) = after.strip_prefix('=') {
            let v = stripped.trim_start().trim_start_matches('"').trim_start_matches('\'');
            let end = v.find(['"', '\'']).unwrap_or(v.len());
            return Some(unescape(&v[..end]));
        }
        rest = &rest[i + attr.len()..];
    }
    None
}

/// 取 `<tag>…</tag>` 的正文（`dc:title` / `dc:creator` 这类）。
fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let i = xml.find(&open)?;
    let rest = &xml[i..];
    let gt = rest.find('>')?;
    let body = &rest[gt + 1..];
    let lt = body.find('<')?;
    let t = unescape(body[..lt].trim());
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
}

/// OPF 的 manifest：`id → (href, media-type, properties)`。
fn opf_items(opf: &str) -> Vec<(String, (String, String, String))> {
    let mut out = Vec::new();
    let mut rest = opf;
    while let Some(i) = rest.find("<item") {
        let seg = &rest[i..];
        let Some(end) = seg.find('>') else { break };
        let tag = &seg[..=end];
        let id = attr_value(tag, "id").unwrap_or_default();
        let href = attr_value(tag, "href").unwrap_or_default();
        let mt = attr_value(tag, "media-type").unwrap_or_default();
        let props = attr_value(tag, "properties").unwrap_or_default();
        if !id.is_empty() {
            out.push((id, (href, mt, props)));
        }
        rest = &rest[i + 5..];
    }
    out
}

fn spine_count(opf: &str) -> usize {
    opf.find("<spine")
        .map(|i| opf[i..].matches("<itemref").count())
        .unwrap_or(0)
}

/// ncx 的目录：`<navLabel><text>…</text></navLabel>`。
fn ncx_titles(ncx: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = ncx;
    while let Some(i) = rest.find("<text") {
        let seg = &rest[i..];
        let Some(gt) = seg.find('>') else { break };
        let body = &seg[gt + 1..];
        let Some(lt) = body.find('<') else { break };
        let t = unescape(body[..lt].trim());
        if !t.is_empty() {
            out.push(t);
        }
        rest = &rest[i + 5..];
        if out.len() >= MAX_TOC {
            break;
        }
    }
    out
}

/// EPUB3 nav 文档的目录：`<a …>…</a>`（只在 `<nav>` 里才算，粗略取即可）。
fn nav_titles(nav: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = nav;
    while let Some(i) = rest.find("<a ") {
        let seg = &rest[i + 3..];
        let Some(gt) = seg.find('>') else { break };
        let body = &seg[gt + 1..];
        let Some(lt) = body.find('<') else { break };
        let t = unescape(body[..lt].trim());
        if !t.is_empty() {
            out.push(t);
        }
        rest = &rest[i + 3 + gt + 1 + lt..];
        if out.len() >= MAX_TOC {
            break;
        }
    }
    out
}

/// 相对 OPF 所在目录拼出 zip 内的完整路径。
fn join_dir(opf_path: &str, href: &str) -> String {
    let base = match opf_path.rfind('/') {
        Some(i) => &opf_path[..i],
        None => "",
    };
    if base.is_empty() {
        href.to_string()
    } else {
        format!("{base}/{href}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 造一个最小可跑的 EPUB（container.xml + content.opf + 一节 xhtml + ncx）。
    fn make_epub(dir: &Path, name: &str, title: &str, broken: bool) -> PathBuf {
        let p = dir.join(name);
        let f = std::fs::File::create(&p).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        z.start_file("META-INF/container.xml", opts).unwrap();
        if broken {
            z.write_all(b"not xml at all").unwrap();
        } else {
            z.write_all(
                br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            )
            .unwrap();
        }
        z.start_file("OEBPS/content.opf", opts).unwrap();
        z.write_all(
            format!(
                r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata><dc:title>{title}</dc:title><dc:creator>庄周</dc:creator></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#
            )
            .as_bytes(),
        )
        .unwrap();
        z.start_file("OEBPS/toc.ncx", opts).unwrap();
        z.write_all(
            r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>逍遥游</text></navLabel></navPoint><navPoint><navLabel><text>齐物论</text></navLabel></navPoint></navMap></ncx>"#
                .as_bytes(),
        )
        .unwrap();
        z.start_file("OEBPS/c1.xhtml", opts).unwrap();
        z.write_all("<html><body><p>北冥有鱼</p></body></html>".as_bytes())
            .unwrap();
        z.finish().unwrap();
        p
    }

    #[test]
    fn ext_filter_keeps_junk_out() {
        let d = std::env::temp_dir().join("jiandu-batch-ext");
        let _ = std::fs::create_dir_all(&d);
        for n in ["a.epub", "b.txt", "c.pdf", "d.md", ".DS_Store", "e"] {
            let _ = std::fs::write(d.join(n), b"x");
        }
        let got = collect(&d, false);
        let names: Vec<String> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.epub", "b.txt", "d.md"]);
        // .DS_Store 与 .pdf 连队列都进不去——它们不是书，不该出现在错误日志里。
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn collect_is_sorted_and_capped() {
        let d = std::env::temp_dir().join("jiandu-batch-sort");
        let _ = std::fs::create_dir_all(&d);
        for n in ["c.txt", "a.txt", "b.txt"] {
            let _ = std::fs::write(d.join(n), b"x");
        }
        let got = collect(&d, false);
        let names: Vec<String> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);
        assert!(MAX_FILES >= 100);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn peek_epub_reads_title_author_and_toc() {
        let d = std::env::temp_dir().join("jiandu-batch-peek");
        let _ = std::fs::create_dir_all(&d);
        let p = make_epub(&d, "zhuangzi.epub", "庄子", false);
        let c = peek(&p).expect("点检应当成功");
        assert_eq!(c.title, "庄子");
        assert_eq!(c.author, "庄周");
        assert_eq!(c.toc, vec!["逍遥游".to_string(), "齐物论".to_string()]);
        assert!(c.bytes > 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn peek_broken_epub_errors_instead_of_panicking() {
        let d = std::env::temp_dir().join("jiandu-batch-broken");
        let _ = std::fs::create_dir_all(&d);
        // ① 压根不是 zip
        let junk = d.join("junk.epub");
        std::fs::write(&junk, b"this is not a zip").unwrap();
        assert!(peek(&junk).is_err());
        // ② 是 zip 但 container.xml 是垃圾
        let bad = make_epub(&d, "bad.epub", "坏书", true);
        assert!(peek(&bad).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn peek_txt_takes_first_line_as_title() {
        let d = std::env::temp_dir().join("jiandu-batch-txt");
        let _ = std::fs::create_dir_all(&d);
        let p = d.join("道德经.txt");
        std::fs::write(&p, "道德经\n道可道，非常道。\n").unwrap();
        let c = peek(&p).unwrap();
        assert_eq!(c.title, "道德经");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn queue_runs_in_order_and_counts_settled() {
        let items: Vec<Candidate> = ["甲", "乙", "丙"]
            .iter()
            .map(|t| Candidate {
                path: PathBuf::from(format!("/tmp/{t}.epub")),
                title: t.to_string(),
                author: String::new(),
                toc: Vec::new(),
                bytes: 0,
            })
            .collect();
        let mut q = BatchQueue::new(items);
        let stop = AtomicBool::new(false);
        let mut seen = Vec::new();
        q.drive(
            &stop,
            &mut |c, _i, _n| Ok(format!("sid-{}", c.title)),
            &mut |i, n, name| seen.push(format!("{i}/{n}:{name}")),
        );
        assert_eq!(q.settled(), 3);
        assert_eq!(q.results().iter().filter(|r| r.ok()).count(), 3);
        assert_eq!(seen[0], "1/3:甲");
        assert_eq!(seen[2], "3/3:丙");
        assert_eq!(q.summary(), "已上架 3 本");
    }

    #[test]
    fn a_panicking_book_does_not_kill_the_queue() {
        let items: Vec<Candidate> = ["甲", "乙", "丙"]
            .iter()
            .map(|t| Candidate {
                path: PathBuf::from(format!("/tmp/{t}.epub")),
                title: t.to_string(),
                author: String::new(),
                toc: Vec::new(),
                bytes: 0,
            })
            .collect();
        let mut q = BatchQueue::new(items);
        let stop = AtomicBool::new(false);
        q.drive(
            &stop,
            &mut |c, _i, _n| {
                if c.title == "乙" {
                    panic!("畸形文件");
                }
                Ok("sid".to_string())
            },
            &mut |_, _, _| {},
        );
        // 三本都结清了：乙被记成跳过，丙照刻不误。
        assert_eq!(q.settled(), 3);
        assert!(q.results()[1].log_line().contains("崩溃"));
        assert!(q.results()[2].ok());
        assert_eq!(q.summary(), "已上架 2 本，跳过 1 本");
    }

    #[test]
    fn unsupported_file_is_skipped_with_a_reason() {
        let items: Vec<Candidate> = vec![Candidate {
            path: PathBuf::from("/tmp/坏.epub"),
            title: "坏".to_string(),
            author: String::new(),
            toc: Vec::new(),
            bytes: 0,
        }];
        let mut q = BatchQueue::new(items);
        let stop = AtomicBool::new(false);
        q.drive(&stop, &mut |_, _, _| Err("加密 EPUB".to_string()), &mut |_, _, _| {});
        assert_eq!(q.settled(), 1);
        assert!(!q.results()[0].ok());
        assert!(q.results()[0].log_line().contains("加密 EPUB"));
    }

    #[test]
    fn cancel_stops_queue_and_marks_half_done_book_as_orphan() {
        let items: Vec<Candidate> = ["甲", "乙", "丙", "丁"]
            .iter()
            .map(|t| Candidate {
                path: PathBuf::from(format!("/tmp/{t}.epub")),
                title: t.to_string(),
                author: String::new(),
                toc: Vec::new(),
                bytes: 0,
            })
            .collect();
        let mut q = BatchQueue::new(items);
        let stop = AtomicBool::new(false);
        // 刻完第二本之后按下取消：第三本不再开刻。
        let mut n = 0;
        q.drive(
            &stop,
            &mut |c, _i, _n| {
                n += 1;
                if n == 2 {
                    stop.store(true, Ordering::Relaxed);
                }
                Ok(format!("sid-{}", c.title))
            },
            &mut |_, _, _| {},
        );
        assert!(q.canceled());
        assert_eq!(q.settled(), 2);
        assert_eq!(q.remaining().len(), 2);
        // 甲已上架（调用方拿走），乙刻完了但还没上架 → 进 orphan 等删。
        assert_eq!(q.orphans(), &["sid-乙".to_string()]);
        assert!(q.summary().starts_with("已取消"));
        assert!(q.current().is_none());
    }

    #[test]
    fn collect_recurses_only_when_asked() {
        let d = std::env::temp_dir().join("jiandu-batch-rec");
        let _ = std::fs::create_dir_all(d.join("sub"));
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        std::fs::write(d.join("sub").join("b.txt"), b"x").unwrap();
        assert_eq!(collect(&d, false).len(), 1);
        let deep = collect(&d, true);
        let names: Vec<String> = deep
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pre_scan_skips_count_toward_the_summary() {
        // 点检就淘汰的书也要进汇总——否则用户会看到「已上架 3 本」，
        // 却发现自己选了 5 本，另外 2 本人间蒸发。
        let mut q = BatchQueue::new(Vec::new());
        q.record_pre(ItemResult::Skip {
            title: "坏书.epub".to_string(),
            reason: "不是合法的 zip 容器".to_string(),
        });
        q.record_pre(ItemResult::Skip {
            title: "图.pdf".to_string(),
            reason: "不支持的格式".to_string(),
        });
        assert_eq!(q.summary(), "已上架 0 本，跳过 2 本");
        assert_eq!(q.all_results().len(), 2);
        assert!(q.results().is_empty()); // 它们没进过队列
    }

    #[test]
    fn orphans_are_cleaned_by_the_caller() {
        // 取消语义的关键：orphan 只是「待清理清单」，删库的动作在调用方
        // （UI 层）。这里钉住「取消前的成果不进 orphan」——否则会把用户
        // 已经看见上架的书一起删掉。
        let items: Vec<Candidate> = ["甲", "乙"]
            .iter()
            .map(|t| Candidate {
                path: PathBuf::from(format!("/tmp/{t}.epub")),
                title: t.to_string(),
                author: String::new(),
                toc: Vec::new(),
                bytes: 0,
            })
            .collect();
        let mut q = BatchQueue::new(items);
        let stop = AtomicBool::new(true); // 一开刻就取消
        q.drive(&stop, &mut |_, _, _| Ok("sid".to_string()), &mut |_, _, _| {});
        assert!(q.canceled());
        assert_eq!(q.settled(), 0);
        assert!(q.orphans().is_empty());
    }
}
