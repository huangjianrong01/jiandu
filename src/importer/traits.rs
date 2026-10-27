//! 格式解析器的统一特征。
//!
//! 漏斗的第一段只有一件事：**把字节还原成 UTF-8 纯文本**。至于这段文本是
//! 从 zip 里的 XHTML、还是从 JSON 的 `content` 字段、还是从一堆 GB18030
//! 字节里捞出来的，漏斗下游一概不问。
//!
//! 故这个特征刻意做得很窄——窄到任何格式都不可能「顺手」在下游该管的地方
//! 动手脚（比如在这里偷偷做断章、洗标点）：那些是 [`super::cleaner`] 与
//! `pipeline` 的职责，越权就会让同一条脏数据从不同入口进来得到不同结果。

use super::ImportError;
use std::path::Path;

/// 一种格式 = 一个实现：给定文件路径，交出「原始纯文本」。
///
/// 用关联函数（`Self::parse`）而不是 `&self` 方法：解析器没有状态，
/// 造一个空结构体只为了调一次函数是纯噪音。
pub trait FormatParser {
    /// 只提取原始纯文本。
    ///
    /// 约定：
    /// * **段落之间用 `\n`**，不保证已经分章（分章是下游的事）；
    /// * 保留原文用字（简繁、异体、数字形态都不改）——清洗在下一站；
    /// * 任何失败都必须转成 [`ImportError`]，**绝不 panic**。
    fn parse(path: &Path) -> Result<String, ImportError>
    where
        Self: Sized;

    /// 这种格式认哪些后缀（大小写不敏感，不带点）。
    ///
    /// 目前只作自检与文档用：真正的后缀派发在 [`super::Format::from_path`]
    /// 一处完成，避免两份名单各自漂移。
    fn extensions() -> &'static [&'static str]
    where
        Self: Sized;
}
