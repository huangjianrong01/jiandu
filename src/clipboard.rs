//! 系统剪贴板：一律走 GPUI 平台层（macOS 下即原生 `NSPasteboard`）。
//!
//! v1.71.0 上架整改（检查清单第 15 / 16 条）。改前的两颗雷：
//!
//! - **写**：`osascript -e 'set the clipboard to …'` —— 走 AppleEvents，
//!   沙盒下要 `com.apple.security.automation.apple-events` entitlement，
//!   审核必问「为什么你要控制别的 App」，且首次会弹权限框；
//! - **读**：`pbpaste` 子进程 —— 沙盒里 `HOME` 被重定位、`PATH` 未必含
//!   `/usr/bin`，随时可能被拦，且每次 fork 一个进程。
//!
//! 现在统一走 `App::write_to_clipboard / read_from_clipboard`，macOS 实现
//! 就是 `NSPasteboard.generalPasteboard()`（见 gpui 的
//! `platform/mac/platform.rs`）：**无子进程、无新依赖、无权限弹窗**。
//! 图片直接塞 PNG（`public.png` UTI），不再是 TIFF 转码——微信 / 备忘录
//! 粘回来仍是真 PNG。

use gpui::{App, ClipboardItem, Image, ImageFormat};

/// 读系统剪贴板里的纯文本；剪贴板是图片或空时返回 `None`。
pub fn read_text(cx: &App) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

/// 把纯文本写进系统剪贴板。
pub fn write_text(cx: &App, text: impl Into<String>) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
}

/// 把 PNG 字节写进系统剪贴板（UTI `public.png`）。
pub fn write_png(cx: &App, png: Vec<u8>) {
    cx.write_to_clipboard(ClipboardItem::from(Image::from_bytes(
        ImageFormat::Png,
        png,
    )));
}

/// 输入框粘贴口径：换行 / 回车一律压成空格。
///
/// 一条笔记被回车劈成几段会破坏「一字一光标」的草稿模型，
/// 所以粘贴时不保留换行（改前 `pbpaste` 那三处就是这个口径，原样继承）。
pub fn paste_text(cx: &App) -> Option<String> {
    read_text(cx).map(|s| one_line(&s))
}

/// 纯函数：把所有换行 / 回车压成空格（可单测）。
pub fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::one_line;

    #[test]
    fn paste_collapses_every_kind_of_newline() {
        assert_eq!(one_line("上\n下"), "上 下");
        assert_eq!(one_line("上\r\n下"), "上  下");
        assert_eq!(one_line("单行"), "单行");
        assert_eq!(one_line(""), "");
    }

    /// 防回归：绝不能因为换了剪贴板实现就把正文里的字吃掉。
    #[test]
    fn paste_keeps_every_character() {
        let s = "子曰：學而時習之，不亦說乎。";
        assert_eq!(one_line(s), s);
    }
}
