//! 字体加载与回退链。
//!
//! 古文模式：宋体 / 思源宋体 / 朱雀仿宋 / 京华老宋 / 汇文明朝 / 汇文正楷 /
//!          寒蝉活仿楷 / 寒蝉锦书宋（后六款为典籍排版常用开源字库）。
//! 现代模式：思源黑体 / 系统默认。
//!
//! GPUI 通过字体族名称在系统里查找字体；若随包提供了 TTF/OTF（放在
//! `resources/fonts/`），启动时全部注册，再按族名查找。
//! 未安装字体时沿回退链落到最接近的系统字体，避免出现「豆腐块」。
//!
//! 关键点：GPUI 0.2.2 的 `font_family` 只接受**一个**族名（没有 CSS 式
//! 的 fallback 列表），所以「回退链」必须由我们自己挑：启动时把系统可用
//! 字体名缓存下来，`family_for` 返回链上第一个真正存在的族名。

use crate::types::{FontChoice, Script};
use gpui::{App, SharedString};
use std::path::PathBuf;
use std::sync::OnceLock;

/// 随包字体的文件名（放进 `resources/fonts/` 即自动注册）。
/// 目录里其它 `.ttf/.otf/.ttc` 也会被扫描注册，这里只是常见命名的白名单。
const BUNDLED: &[&str] = &[
    "SourceHanSerifSC-Regular.otf",
    "SourceHanSansSC-Regular.otf",
    "ZhuqueFangsong-Regular.ttf",
    "ZhuqueFangsong.ttf",
    "JinghuaOldSong.ttf",
    "HuiwenMincho.ttf",
    "HuiwenZhengkai.ttf",
    "HanchanHuofangKai.ttf",
    "HanchanJinshuSong.ttf",
];

/// 系统可用字体族名（启动时缓存一次）。
static AVAILABLE: OnceLock<Vec<String>> = OnceLock::new();

/// 字体文件可能所在的目录（源码树运行 / .app bundle / 免安装包）。
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![PathBuf::from("resources/fonts")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("../Resources/fonts"));
            dirs.push(dir.join("resources/fonts"));
        }
    }
    dirs
}

/// 在应用启动时调用：注册随包字体（存在才注册），并缓存系统字体名。
pub fn register_bundled_fonts(cx: &mut App) {
    let mut seen: Vec<String> = Vec::new();
    for dir in font_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if seen.contains(&name) {
                continue;
            }
            seen.push(name);
            if let Ok(bytes) = std::fs::read(&path) {
                cx.text_system().add_fonts(vec![bytes.into()]).ok();
            }
        }
        // 目录不存在就继续找下一个候选目录。
        if !seen.is_empty() {
            break;
        }
    }
    // 白名单里的字体即使放在别处（例如手放到 Resources 根目录）也尽力注册。
    for name in BUNDLED {
        for dir in font_dirs() {
            let p = dir.join(name);
            if p.exists() {
                if let Ok(bytes) = std::fs::read(&p) {
                    cx.text_system().add_fonts(vec![bytes.into()]).ok();
                }
                break;
            }
        }
    }
    let names = cx.text_system().all_font_names();
    let _ = AVAILABLE.set(names);
}

/// 古文模式字体回退链。
pub fn ancient_chain(choice: FontChoice) -> Vec<&'static str> {
    match choice {
        FontChoice::SourceHanSerif => vec![
            "Source Han Serif SC",
            "Noto Serif CJK SC",
            "Source Han Serif",
            "Songti SC",
            "SimSun",
            "serif",
        ],
        FontChoice::ZhuqueFangsong => vec![
            "Zhuque Fangsong",
            "ZhuqueFangsong",
            "朱雀仿宋",
            "STFangsong",
            "FangSong",
            "Songti SC",
            "serif",
        ],
        FontChoice::JinghuaOldSong => vec![
            "Jinghua Old Song",
            "JinghuaOldSong",
            "京华老宋",
            "京华老宋体",
            "Songti SC",
            "STSong",
            "serif",
        ],
        FontChoice::HuiwenMincho => vec![
            "Huiwen Mincho",
            "HuiwenMincho",
            "汇文明朝",
            "汇文明朝体",
            "Songti SC",
            "STSong",
            "serif",
        ],
        FontChoice::HuiwenZhengkai => vec![
            "Huiwen Zhengkai",
            "HuiwenZhengkai",
            "汇文正楷",
            "Kaiti SC",
            "STKaiti",
            "serif",
        ],
        FontChoice::HanchanHuofang => vec![
            "Hanchan Huofang Kai",
            "HanchanHuofangKai",
            "寒蝉活仿楷",
            "Kaiti SC",
            "STKaiti",
            "serif",
        ],
        FontChoice::HanchanJinshu => vec![
            "Hanchan Jinshu Song",
            "HanchanJinshuSong",
            "寒蝉锦书宋",
            "Songti SC",
            "STSong",
            "serif",
        ],
        // 宋体 / 其它
        _ => vec![
            "SimSun",
            "Songti SC",
            "STSong",
            "Source Han Serif SC",
            "Noto Serif CJK SC",
            "serif",
        ],
    }
}

/// 现代模式字体回退链。
pub fn modern_chain(choice: FontChoice) -> Vec<&'static str> {
    match choice {
        FontChoice::SourceHanSans => vec![
            "Source Han Sans SC",
            "Noto Sans CJK SC",
            "Source Han Sans",
            "PingFang SC",
            "Microsoft YaHei",
            "sans-serif",
        ],
        _ => vec![
            "system-ui",
            "PingFang SC",
            "Microsoft YaHei",
            "sans-serif",
        ],
    }
}

/// 按当前模式与字体选择返回字体族名：回退链上第一个系统真正拥有的字体。
///
/// GPUI 0.2.2 的 `font_family` 接受 `impl Into<SharedString>`（没有
/// FontFamily 枚举，也没有 fallback 列表），所以这里先把「可用字体名」
/// 缓存下来做一次挑选，避免选了未安装的字体后整页变成方框。
pub fn family_for(choice: FontChoice, script: Script) -> SharedString {
    let chain = match script {
        Script::Ancient => ancient_chain(choice),
        Script::Modern => modern_chain(choice),
    };
    pick_font(&chain, AVAILABLE.get().map(|v| v.as_slice()).unwrap_or(&[]))
}

/// 给定一个偏好字体的回退链，返回第一个「可用」的字体族名。
/// `available` 为空（启动时取不到字体列表）时退回链首，交给系统兜底。
pub fn pick_font(chain: &[&str], available: &[String]) -> SharedString {
    if !available.is_empty() {
        for name in chain {
            if available.iter().any(|f| f.eq_ignore_ascii_case(name)) {
                return SharedString::new(*name);
            }
        }
    }
    SharedString::new(chain.first().copied().unwrap_or("serif"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_prefers_installed_family() {
        let avail = vec!["Songti SC".to_string(), "PingFang SC".to_string()];
        let chain = ancient_chain(FontChoice::ZhuqueFangsong);
        // 未装朱雀仿宋时落到链上第一个可用字体（Songti SC），不是链首。
        assert_eq!(pick_font(&chain, &avail), SharedString::new("Songti SC"));
    }

    #[test]
    fn chain_falls_back_to_first_when_unknown() {
        let chain = modern_chain(FontChoice::SourceHanSans);
        assert_eq!(pick_font(&chain, &[]), SharedString::new("Source Han Sans SC"));
    }

    #[test]
    fn every_classic_font_has_chain() {
        for c in FontChoice::CLASSIC {
            let chain = ancient_chain(c);
            assert!(!chain.is_empty(), "{} 缺回退链", c.label());
            assert!(chain.contains(&"serif"), "{} 缺 serif 兜底", c.label());
        }
    }
}
