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
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io { path, detail } => write!(f, "读不了这个文件：{path}（{detail}）"),
            ImportError::Unsupported(ext) => {
                write!(f, "认不出「{ext}」这种格式，暂只收 epub / txt / md / html / json / docx")
            }
            ImportError::Zip(d) => write!(f, "压缩包损坏或加密：{d}"),
            ImportError::Xml(d) => write!(f, "书里的 XML/XHTML 结构坏了：{d}"),
            ImportError::Json(d) => write!(f, "JSON 读不通：{d}"),
            ImportError::Parse(d) => write!(f, "文本读到了，但切不出章节：{d}"),
            ImportError::MissingEntry(n) => write!(f, "压缩包里缺 {n}，这本多半不是正经电子书"),
            ImportError::TooLarge { limit } => {
                write!(f, "文件超过 {} MiB，请先分卷再导入", limit / 1024 / 1024)
            }
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
        }
    }
}

/// **漏斗总入口**：任意支持的文件 → `BookIR`。
///
/// 五步，一步都不许跳：
///   ① 按后缀定格式；② 该格式的 parser 只出「原始纯文本」；
///   ③ 清洗层唯一出口洗一遍；④ core 管线断章断卷；⑤ 拼成 BookIR。
pub fn import_file(path: &Path) -> Result<BookIR, ImportError> {
    let fmt = Format::from_path(path).ok_or_else(|| {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("(无后缀)")
            .to_string();
        ImportError::Unsupported(ext)
    })?;

    // ② 各格式的「字节 → 纯文本」
    let raw = match fmt {
        Format::Json => json_parser::JsonParser::parse(path)?,
        Format::Epub => epub_parser::EpubParser::parse(path)?,
        Format::Txt => txt_parser::TxtParser::parse(path)?,
        Format::Html => html_parser::HtmlParser::parse(path)?,
        Format::Markdown => md_parser::MdParser::parse(path)?,
        Format::Docx => docx_parser::DocxParser::parse(path)?,
    };

    // ③ 全局唯一清洗出口
    let text = cleaner::clean(&raw);

    let stem = jiandu_core::importer::file_stem(path);
    let (kind, source) = fmt.kinds();

    // 空书不是错误：一本空笺也是笺（UI 弹「此书空白」比弹「导入失败」诚实）。
    if text.trim().is_empty() {
        return Ok(BookIR::new(&stem, "", source));
    }

    // ④ core 既有管线（一字未改）
    let chapters = parse_str(kind, &text).map_err(|e| ImportError::Parse(e.to_string()))?;

    // ④ JSON 的元数据（`book` / `author` 字段）**单独提出来**当书名 / 作者
    // （v1.69.0）——它们既不该混进正文流，也不该退化成文件名。
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
    Ok(BookIR::from_chapters(&title, &author, source, &chapters))
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
