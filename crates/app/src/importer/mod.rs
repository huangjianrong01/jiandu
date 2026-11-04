//! **多格式导入的漏斗**（v1.62.0）
//!
//! 一句话原则：**上面开很多口，下面只收一个口**。
//!
//! ```text
//!   json / epub / txt / html / md / docx
//!        │        │      │      │     │     │
//!        └────────┴──────┴──────┴─────┴─────┘
//!                        │  FormatParser::parse  （只出「原始纯文本」）
//!                        ▼
//!                  cleaner::clean          （全局唯一清洗出口）
//!                        ▼
//!              pipeline::parse_str         （core 既有管线，一字不改）
//!                        ▼
//!                     BookIR
//! ```
//!
//! # 为什么这么切
//!
//! 各格式的**差异只到「文本长什么样」为止**：EPUB 是 zip 里的 XHTML、DOCX 是
//! zip 里的 XML、JSON 是嵌套对象、TXT 是一堆字节。一旦把它们各自还原成纯文本，
//! 后面的事（断章、断卷、数字汉字化、夹注抽取、竖排标点）就全是同一套逻辑。
//! 所以本模块**只做前半段**：把字节变成 UTF-8 纯文本；后半段一律交给
//! `jiandu_core` 的既有管线——core、渲染层、BookIR 结构**一个字都不动**。
//!
//! # 三条不可破的底线
//!
//! 1. **绝不全量 `read_to_string`**：一律 `BufReader` 分块（[`read_streamed`]），
//!    并有 [`MAX_BYTES`] 上限，坏文件 / 巨型文件不会把内存吃光。
//! 2. **绝不 `unwrap` / `expect`**：任何解析失败都收成 [`ImportError`]，UI 层
//!    捕获后弹提示。导入是最容易被脏文件攻击的入口，这里 panic 就是闪退。
//! 3. **清洗只有一处**：[`cleaner::clean`]。任何 parser 都不许自己偷偷洗，
//!    否则同一条脏数据从不同入口进来会得到不同结果。

pub mod cleaner;
pub mod docx_parser;
pub mod epub_rules;
pub mod epub_parser;
pub mod html_parser;
pub mod json_parser;
pub mod md_parser;
pub mod pdf_parser;
pub mod traits;
pub mod txt_parser;

// 老 API 原样透传（`import_from_path` / `write_book_json` / `import_json`…）：
// UI 与 exporter 里既有的 `crate::importer::xxx` 调用一句都不用改。
pub use jiandu_core::importer::*;

use jiandu_core::ir::{IrBook, IrSource};
use jiandu_core::pipeline::{parse_str, SrcKind};
use std::io::Read;
use std::path::Path;

use traits::FormatParser;

/// 用户口径里的「BookIR」＝ core 的 [`IrBook`]（同一份结构，绝不复刻第二份）。
pub type BookIR = IrBook;

/// 导入漏斗的统一返回：**薄包装** [`BookIR`]，附带可选 PDF 坐标。
///
/// 设计原则（v1.9.x）：**绝不改 `BookIR` 本身**——所有历史测试与调用点只需
/// `.book` 拆出 `BookIR`（`into_book()` 一句即可）就能跑，逻辑一行不变。
/// PDF 文本层坐标只是「顺带的赠品」：非 PDF 一律 `None`，扫描 PDF 在入口处
/// 已由 [`ImportError::OcrNeeded`] 提前报错，不会走到这里。
#[derive(Debug)]
pub struct ParsedDoc {
    /// 既有的书稿中间表示，一字未改。
    pub book: BookIR,
    /// PDF 文本层坐标（仅 PDF 导入时有；扫描件为 `None`）。
    pub pdf_regions: Option<Vec<crate::pdf_text::SourceRegion>>,
}

impl ParsedDoc {
    /// 拆出历史 API 用的 [`jiandu_core::types::Book`]（与 `BookIR::to_book` 同语义，
    /// 老调用点 `ir.to_book()` 一行都不用改）。
    pub fn to_book(&self) -> jiandu_core::types::Book {
        self.book.to_book()
    }
}

/// 单文件读取上限（128 MiB）：超过就报错而不是把内存吃穿。
///
/// 一部《四库全书》级别的纯文本约 8 亿字、1.6 GB——那种量级该走分卷，不该
/// 由「打开一个文件」承担。真需要时把这个常量调大并配 `--release` 跑。
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// 导入期的一切失败（**永不 panic**）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// 读文件失败（不存在 / 没权限 / 半截盘）。
    Io { path: String, detail: String },
    /// 后缀不认识。
    Unsupported(String),
    /// 压缩包（epub / docx）读不动。
    Zip(String),
    /// XML / XHTML 结构坏了。
    Xml(String),
    /// JSON 完全不成立。
    Json(String),
    /// 文本拿到了，但 core 的管线排不出章。
    Parse(String),
    /// 压缩包里缺关键成员（如 docx 的 `word/document.xml`）。
    MissingEntry(String),
    /// 超过 [`MAX_BYTES`]。
    TooLarge { limit: u64 },
    /// PDF 是扫描图、没有文本层，需要先 OCR 才能导入。
    OcrNeeded(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io { path, detail } => write!(f, "读不了这个文件：{path}（{detail}）"),
            ImportError::Unsupported(ext) => {
                write!(f, "认不出「{ext}」这种格式，暂只收 epub / txt / md / html / json / docx / pdf")
            }
            ImportError::Zip(d) => write!(f, "压缩包损坏或加密：{d}"),
            ImportError::Xml(d) => write!(f, "书里的 XML/XHTML 结构坏了：{d}"),
            ImportError::Json(d) => write!(f, "JSON 读不通：{d}"),
            ImportError::Parse(d) => write!(f, "文本读到了，但切不出章节：{d}"),
            ImportError::MissingEntry(n) => write!(f, "压缩包里缺 {n}，这本多半不是正经电子书"),
            ImportError::TooLarge { limit } => {
                write!(f, "文件超过 {} MiB，请先分卷再导入", limit / 1024 / 1024)
            }
            ImportError::OcrNeeded(path) => write!(
                f,
                "「{path}」是扫描图 PDF，没有文本层，无法导入。请先用 OCR 工具把文字识别出来，再导入 txt / md。"
            ),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        ImportError::Io {
            path: String::new(),
            detail: e.to_string(),
        }
    }
}

/// 支持的格式（后缀 → 解析策略）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Json,
    Epub,
    Txt,
    Html,
    Markdown,
    Docx,
    /// PDF 文本层（macOS 走 PDFKit，其他平台预留 pdfium 入口）。
    Pdf,
}

impl Format {
    /// 按后缀定格式（大小写不敏感；`.xhtml`/`.htm` 归 HTML）。
    pub fn from_path(path: &Path) -> Option<Format> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        Some(match ext.as_str() {
            "json" => Format::Json,
            "epub" => Format::Epub,
            "txt" | "text" | "log" | "csv" => Format::Txt,
            "html" | "htm" | "xhtml" | "xml" => Format::Html,
            "md" | "markdown" | "mdown" | "mkd" => Format::Markdown,
            "docx" => Format::Docx,
            "pdf" => Format::Pdf,
            _ => return None,
        })
    }

    /// 纯文本 → core 管线需要的（来源类型，书稿来源标记）。
    ///
    /// 一律走 `SrcKind::Txt`：各 parser 已经把结构抹平成「一行一段」，
    /// 再让 core 的 Markdown/HTML 解析器去二次解释只会重复劳动。
    fn kinds(self) -> (SrcKind, IrSource) {
        match self {
            Format::Json => (SrcKind::Txt, IrSource::Json),
            Format::Epub => (SrcKind::Txt, IrSource::Epub),
            Format::Docx => (SrcKind::Txt, IrSource::Docx),
            Format::Html => (SrcKind::Txt, IrSource::Html),
            Format::Markdown => (SrcKind::Txt, IrSource::Markdown),
            Format::Txt => (SrcKind::Txt, IrSource::Txt),
            Format::Pdf => (SrcKind::Txt, IrSource::Pdf),
        }
    }

    /// 名字给人看（报错用）。
    pub fn label(self) -> &'static str {
        match self {
            Format::Json => "JSON",
            Format::Epub => "EPUB",
            Format::Txt => "TXT",
            Format::Html => "HTML",
            Format::Markdown => "Markdown",
            Format::Docx => "DOCX",
            Format::Pdf => "PDF",
        }
    }
}

/// **漏斗总入口**：任意支持的文件 → [`ParsedDoc`]（薄包装 [`BookIR`] + 可选 PDF 坐标）。
///
/// 五步，一步都不许跳：
///   ① 按后缀定格式；② 该格式的 parser 只出「原始纯文本」（PDF 顺带抽坐标）；
///   ③ 清洗层唯一出口洗一遍；④ core 管线断章断卷；⑤ 拼成 BookIR 包进 ParsedDoc。
pub fn import_file(path: &Path) -> Result<ParsedDoc, ImportError> {
    import_file_with(path, &mut |_, _| {})
}

/// **取「解析后的毛坯文本」**：只跑各格式的 parser（②），**不**走清洗层
/// （③），**不**做康熙部首折叠 / 异体字转换 / 幽灵字符清除。
///
/// 用途：笺校台【原文对照】。用户要看到的是「已提取段落、但还没被正则/归一
/// 动过的原始脏文本」，而不是 EPUB 的 HTML 源码——这样才能直观对比出清洗
/// 引擎到底删了哪些字符、改了哪些字，防误杀。
///
/// 解析失败（不支持后缀 / 文件损坏）返回 `None`，调用方照常走清洗后的正文。
pub fn import_raw_text(path: &Path) -> Option<String> {
    let fmt = Format::from_path(path)?;
    let raw = match fmt {
        Format::Json => json_parser::JsonParser::parse(path).ok()?,
        Format::Epub => epub_parser::EpubParser::parse(path).ok()?,
        Format::Txt => txt_parser::TxtParser::parse(path).ok()?,
        Format::Html => html_parser::HtmlParser::parse(path).ok()?,
        Format::Markdown => md_parser::MdParser::parse(path).ok()?,
        Format::Docx => docx_parser::DocxParser::parse(path).ok()?,
        Format::Pdf => {
            let pages = pdf_parser::collect_pages(path, &mut |_, _| {});
            crate::pdf_text::pages_to_text(&pages)
        }
    };
    Some(raw)
}

/// 与 [`import_file`] 相同，但把**逐页进度**交出去（v1.9.34）。
///
/// 只有 PDF 会真的用到它：文本层要一页页抽，一本两百页的书几百毫秒到几秒，
/// 而导入看门狗 3 秒不见 `pct`/文案变化就当卡死中止、报「文件损坏」。其余格式
/// 一次性读完，回调只在收尾时打一枪 100%。
///
/// 回调在**后台导入线程**里被调用，绝不许碰 UI 状态——与 `import_streaming`
/// 的 `on_progress` 同一约定。
pub fn import_file_with(
    path: &Path,
    progress: &mut dyn FnMut(f32, String),
) -> Result<ParsedDoc, ImportError> {
    let fmt = Format::from_path(path).ok_or_else(|| {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("(无后缀)")
            .to_string();
        ImportError::Unsupported(ext)
    })?;

    // ② 各格式的「字节 → 纯文本」；PDF 顺带抽文本层坐标（扫描件为空）。
    let stem = jiandu_core::importer::file_stem(path);
    let (raw, pdf_regions) = match fmt {
        Format::Json => (json_parser::JsonParser::parse(path)?, None),
        Format::Epub => (epub_parser::EpubParser::parse(path)?, None),
        Format::Txt => (txt_parser::TxtParser::parse(path)?, None),
        Format::Html => (html_parser::HtmlParser::parse(path)?, None),
        Format::Markdown => (md_parser::MdParser::parse(path)?, None),
        Format::Docx => (docx_parser::DocxParser::parse(path)?, None),
        Format::Pdf => {
            // PDF 走系统原生文本层抽取（macOS PDFKit / 其他平台预留 pdfium）。
            // 扫描图 PDF 无文本层 → 全本坐标空 → 提前报「需 OCR」，绝不把空笺塞给用户。
            //
            // ⚠️ v1.9.34 之前这里只抽**第 1 页**（`collect_regions` = 第 0 页），
            //    书架上导入一本 200 页的 PDF，进来只有一页的字，而且悄无声息。
            //    现在改为整本逐页抽，页间空行分段，跨页重复的短行（书口字 /
            //    页眉 / 页脚）先剔掉——否则「金剛般若波羅蜜經」会被抄进正文 120 遍。
            let mut pages = pdf_parser::collect_pages(path, &mut |done, total| {
                let pct = if total == 0 {
                    0.0
                } else {
                    done as f32 / total as f32
                };
                progress(pct, format!("刻版 {}/{}", (done + 1).min(total), total));
            });
            if pages.iter().all(|p| p.is_empty()) {
                return Err(ImportError::OcrNeeded(path.display().to_string()));
            }
            crate::pdf_text::strip_running_lines(&mut pages, &stem);
            let raw = crate::pdf_text::pages_to_text(&pages);
            // `pdf_regions` 仍只装**第 1 页**：它服务的是校对台左侧参考图，
            // 一次只显示一页，坐标系必须与当前页一致，跨页坐标没有意义。
            // ⚠️ 走「单页抽取」的老路径（`for_book = false`）：整本入书的页文本
            //    现在带 `[注]…[/注]` 契约标签，若把这串也塞进 region.text，左图
            //    框选按文字回链时就会与屏幕上已吸成夹注的干净正文失配。
            //    老路径不带标签，联动匹配与 v1.9.86 完全一致。
            let first = crate::pdf_text::extract_pdf_text_regions_on(path, 0);
            (raw, Some(first))
        }
    };

    // ③ 全局唯一清洗出口（把换行 / 乱码 / 假夹注彻底洗掉）
    let mut text = cleaner::clean(&raw);

    let (kind, source) = fmt.kinds();

    // 空书不是错误：一本空笺也是笺（UI 弹「此书空白」比弹「导入失败」诚实）。
    if text.trim().is_empty() {
        return Ok(ParsedDoc {
            book: BookIR::new(&stem, "", source),
            pdf_regions,
        });
    }

    // ④ JSON 的元数据（`book` / `author` 字段）**单独提出来**当书名 / 作者
    // （v1.69.0）——它们既不该混进正文流，也不该退化成文件名。不依赖正文顺序，先提。
    let json_meta = if fmt == Format::Json {
        match json_parser::extract_json_meta(&std::fs::read_to_string(path).unwrap_or_default()) {
            (Some(t), a) => (Some(t), a),
            (None, a) => (None, a),
        }
    } else {
        (None, None)
    };
    // v1.80.0：EPUB 同理——净化漏斗把题名页整节丢了，正文首行是篇题
    // （「释诂第一」），从正文猜书名会猜错，只能信 OPF 的 `dc:title`。
    let epub_title = if fmt == Format::Epub {
        epub_parser::read_epub_title(path)
    } else {
        None
    };

    // ⑤ 标题 / 作者：先从正文首两行认（「书名 · 作者」的排法），认不出用文件名。
    //    ⚠️ 必须在去重**之前**做：书名是从正文首行认出来的，认完才能拿它去删正文里
    //    那枚重复的书名（见下方 `strip_leading_title_duplicates`）。
    let (t, a0) = jiandu_core::importer::detect_title_author(&text);
    let t = json_meta.0.or(epub_title).unwrap_or(t);
    let mut a = json_meta.1.unwrap_or(a0);
    // 从正文猜出来的「作者」若是出版噪声（「管锡华◎译注」那类），宁可留空。
    if jiandu_core::purge::is_pub_noise(&a) {
        a.clear();
    }
    let title = if t.trim().is_empty() { stem.clone() } else { t };
    // v1.69.0：书名最后再过一遍「元数据粉碎机」。盗版 EPUB 认不出题名时
    // 兜底用的是**文件名**——那一长串 `尔雅……(管锡华 [guanxihua])
    // (z-library.sk, 1lib.sk, z-lib.sk)` 会整串变成书名，书架卡片和版心
    // 书口全被撑爆。网址 / 拼音 / 站名 / 特殊符号一律剥掉，再截到 15 字。
    let title = jiandu_core::vclean::tidy_meta_title(&jiandu_core::vclean::strip_meta_junk(&title));
    let title = if title.trim().is_empty() {
        stem.clone()
    } else {
        title
    };
    let author = jiandu_core::vclean::tidy_meta_title(&jiandu_core::vclean::strip_meta_junk(&a));

    // ★ 标题去重（v1.9.64）：正文开头若把书名本身又抄了一遍——`资治通鉴` 既在版心
    // 书口显示一次，正文首行又是一枚 `资治通鉴`，视觉上成了两个一模一样的书名。
    // 删掉开头**整行精确等于书名**（含《》包裹）的连续行，让书名只在版心出现一次。
    // 严格限制「开头 + 精确匹配」，正文中间任何同名文字绝不动（见 cleaner 单测）。
    text = cleaner::strip_leading_title_duplicates(&text, &title);

    // ④ core 既有管线（一字未改）
    //    ⚠️ PDF 走**不升格全角括号**的那条：印刷品的 `【题解】` / `（注）` 是
    //    章节标签与现代括号，升格成双行夹注就是正文缺一段（尔雅式 PDF 的老 bug）。
    let chapters = if fmt == Format::Pdf {
        jiandu_core::pipeline::parse_str_keeping_brackets(&text)
    } else {
        parse_str(kind, &text)
    }
    .map_err(|e| ImportError::Parse(e.to_string()))?;

    Ok(ParsedDoc {
        book: BookIR::from_chapters(&title, &author, source, &chapters),
        pdf_regions,
    })
}

/// **分块读取**：`BufReader` + 64 KiB 缓冲，绝不 `read_to_string` 一把梭。
///
/// 返回原始字节；超过 `MAX_BYTES` 直接报 [`ImportError::TooLarge`]（读到一半
/// 就停，不会先把 1 GB 吃进内存再判断）。
pub(crate) fn read_streamed(path: &Path) -> Result<Vec<u8>, ImportError> {
    let file = std::fs::File::open(path).map_err(|e| ImportError::Io {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    let mut br = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = br.read(&mut chunk).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        if n == 0 {
            break;
        }
        if out.len() as u64 + n as u64 > MAX_BYTES {
            return Err(ImportError::TooLarge { limit: MAX_BYTES });
        }
        out.extend_from_slice(&chunk[..n]);
    }
    Ok(out)
}

/// 供 UI 用：这个后缀我们收不收（文件选择框的过滤器与预检共用一套判据）。
pub fn is_supported(path: &Path) -> bool {
    Format::from_path(path).is_some()
}

#[cfg(test)]
mod tests;
