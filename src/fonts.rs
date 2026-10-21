//! 字体加载与回退链。
//!
//! 分工（见 `types::FontChoice`）：
//!   * **正文** —— 文津宋体 / 梦源宋体：大字符集宋体，生僻字全覆盖、
//!     竖排标点挤压正确，古籍正文只认这一类；
//!   * **批注** —— 玄冬楷书 / 霞鹜文楷：夹注小字与放大注卡专用；
//!   * **标题 / UI** —— 思源黑体；
//!   * **等宽 / 西文** —— 尚古等宽、西文数字（Computer Modern）。
//!
//! 这些字库**不随包打包**（一部大字符集宋体几十 MB，会撑爆体积）：
//! 放进 `resources/fonts/` 或装进系统即可选中。GPUI 通过字体族名称在
//! 系统里查找；未安装时沿回退链落到最接近的系统字体，避免「豆腐块」。
//!
//! 关键点：GPUI 0.2.2 的 `font_family` 只接受**一个**族名（没有 CSS 式
//! 的 fallback 列表），所以「回退链」必须由我们自己挑：启动时把系统可用
//! 字体名缓存下来，`family_for` 返回链上第一个真正存在的族名。
//!
//! 族名同时列出「带空格的家族名」与「PostScript 连写名」两种写法——
//! GPUI 的 `all_font_names()` 在不同平台返回的形态不一致，多列几个
//! 别名能显著提高命中率。

use crate::types::{FontChoice, FontPreset, Script};
use gpui::{App, SharedString};
use std::path::PathBuf;
use std::sync::OnceLock;

// ----------------------------------------------------------------------
// 全局字体配置对象
// ----------------------------------------------------------------------
//
// 渲染代码里**不许出现字体名**：一律按「角色」取族名（`family_for_role`），
// 字体名只在这张配置表里出现一次。对应前端那张表的语义：
//
// ```javascript
// const fontConfigs = {
//   main:  'WenJinMincho, "Noto Serif SC", serif',  // 正文：大字符集宋体
//   note:  'LXGW WenKai, "KaiTi", serif',           // 夹注：毛笔质感小字
//   ui:    '"Noto Sans SC", "PingFang SC", sans-serif',
//   latin: '"CMU Serif", serif',                    // 西文与数字
// }
// ```
//
// GPUI 0.2.2 的 `font_family` 只接受单个族名（没有 CSS 的 fallback 列表），
// 所以「链」由我们自己挑：见 [`pick_font`]。

/// 一类字体的配置：`chain` 按优先级排列，挑第一个系统真正有的。
#[derive(Clone, Copy, Debug)]
pub struct FontConfig {
    /// 角色标识（存档 / 调试用）。
    pub id: &'static str,
    /// 面板上的名字。
    pub name: &'static str,
    /// 回退链：族名与 PostScript 名都列（各平台 `all_font_names()` 形态不一）。
    pub chain: &'static [&'static str],
}

/// 字体角色：正文 / 夹注 / 界面 / 西文。渲染按角色取族名，不写字族名。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontRole {
    /// 正文：大字符集宋体（生僻字全覆盖、竖排标点挤压正确）。
    Main,
    /// 夹注 / 批注：楷体（毛笔质感，小字阅读友好）。
    Note,
    /// 界面 / 标题：无衬线。
    Ui,
    /// 西文与数字：Computer Modern（学术排版）。
    Latin,
}

impl FontRole {
    /// 与 `FONT_CONFIGS` **同序**：`cfg()` 按下标取，顺序错了就串味。
    pub const ALL: [FontRole; 4] = [FontRole::Main, FontRole::Note, FontRole::Ui, FontRole::Latin];

    pub fn id(self) -> &'static str {
        self.cfg().id
    }

    pub fn cfg(self) -> &'static FontConfig {
        &FONT_CONFIGS[self as usize]
    }

    pub fn by_id(id: &str) -> Option<FontRole> {
        FontRole::ALL.into_iter().find(|r| r.id() == id)
    }
}

/// **全局字体配置表**：四类角色，唯一数据源。
pub const FONT_CONFIGS: [FontConfig; 4] = [
    FontConfig {
        id: "main",
        name: "正文",
        chain: &[
            "WenJin Mincho",
            "WenJinMincho",
            "WenJinMinchoP0-Regular",
            "文津宋体",
            "MengYuan Serif",
            "MengYuanSerif",
            "梦源宋体",
            "Source Han Serif SC",
            "Noto Serif SC",
            "Songti SC",
            "SimSun",
            "serif",
        ],
    },
    FontConfig {
        id: "note",
        name: "夹注",
        chain: &[
            "LXGW WenKai",
            "LXGWWenKai",
            "LXGW WenKai GB",
            "霞鹜文楷",
            "Xuandong Kaishu",
            "XuandongKaishu",
            "玄冬楷书",
            "Kaiti SC",
            "STKaiti",
            "serif",
        ],
    },
    FontConfig {
        id: "ui",
        name: "界面",
        chain: &[
            "Source Han Sans SC",
            "Noto Sans SC",
            "Noto Sans CJK SC",
            "Source Han Sans",
            "PingFang SC",
            "Microsoft YaHei",
            "sans-serif",
        ],
    },
    FontConfig {
        id: "latin",
        name: "西文",
        chain: &[
            "CMU Serif",
            "Latin Modern Roman",
            "Latin Modern Math",
            "Computer Modern",
            "Shanggu Mono",
            "ShangguMono",
            "尚古等宽",
            "Times New Roman",
            "serif",
        ],
    },
];

/// 按角色取族名：`FONT_CONFIGS` 里那条链上第一个系统真正有的字体。
pub fn family_for_role(role: FontRole) -> SharedString {
    pick_font(
        role.cfg().chain,
        AVAILABLE.get().map(|v| v.as_slice()).unwrap_or(&[]),
    )
}

/// **排版字体**（设置面板那一行）对应的正文 / 批注族名。
///
/// 「刻本宋体」= 正文文津宋体 + 批注霞鹜文楷；「手写楷体」= 正文霞鹜文楷
/// + 批注玄冬楷书——两种模式里正文与批注始终不同体，刻本的层次感不丢。
pub fn family_for_preset(role: FontRole, preset: FontPreset) -> SharedString {
    let choice = match role {
        FontRole::Main => preset.main_choice(),
        FontRole::Note => preset.note_choice(),
        // 界面 / 西文不随「排版字体」变：UI 永远是无衬线、数字永远是 CM。
        FontRole::Ui | FontRole::Latin => return family_for_role(role),
    };
    family_for(choice, Script::Ancient)
}

// ----------------------------------------------------------------------
// 行高缩放
// ----------------------------------------------------------------------
//
// 思源系列（Noto CJK）的 em box 偏大：同样的 fontSize，字面比文津宋体
// 「撑」，竖排时上下两字容易贴到、甚至重叠。给行高乘一个 `lineHeightScale`
// 把字距松开一格余量，字形就紧凑而不粘连。宋体 1.05、楷体 1.12（楷体
// 字形更散，需要更多余量）、黑体 1.02、西文 1.02。

/// 默认行高缩放（未识别的字体取它）。
pub const LINE_SCALE_DEFAULT: f32 = 1.05;
/// 宋体 / 仿宋系：字面方正，余量最小。
pub const LINE_SCALE_SONG: f32 = 1.05;
/// 楷体系：笔势舒展，需要更多行间余量。
pub const LINE_SCALE_KAI: f32 = 1.12;
/// 黑体 / 无衬线：字重大，略松。
pub const LINE_SCALE_SANS: f32 = 1.02;
/// 等宽 / 西文：基线体系不同，与正文同理取最小余量。
pub const LINE_SCALE_LATIN: f32 = 1.02;

/// 一款字体的行高缩放：渲染时 `row_h = col_w × GRID_ROW_RATIO × scale`。
pub fn line_height_scale(choice: FontChoice) -> f32 {
    match choice {
        FontChoice::XuandongKaishu
        | FontChoice::LXGWWenKai
        | FontChoice::HuiwenZhengkai
        | FontChoice::HanchanHuofang => LINE_SCALE_KAI,
        FontChoice::SourceHanSans | FontChoice::SystemDefault => LINE_SCALE_SANS,
        FontChoice::LatinModern | FontChoice::ShangguMono => LINE_SCALE_LATIN,
        _ => LINE_SCALE_SONG,
    }
}

/// 当前排版字体的行高缩放（正文那款字体说了算）。
pub fn preset_line_scale(preset: FontPreset) -> f32 {
    line_height_scale(preset.main_choice())
}

/// 随包字体的文件名（放进 `resources/fonts/` 即自动注册）。
/// 目录里其它 `.ttf/.otf/.ttc` 也会被扫描注册，这里只是常见命名的白名单。
///
/// 只有用户**自己放进来**的才注册——默认一个都不打包，App 体积才压得住。
const BUNDLED: &[&str] = &[
    // 正文：大字符集宋体
    "WenJinMinchoP0-Regular.ttf",
    "WenJinMincho-Regular.ttf",
    "MengYuanSerif-Regular.ttf",
    // 批注：楷体
    "XuandongKaishu-Regular.ttf",
    "LXGWWenKai-Regular.ttf",
    "LXGWWenKaiGB-Regular.ttf",
    // 标题 / UI
    "SourceHanSansSC-Regular.otf",
    // 等宽 / 西文
    "ShangguMono-Regular.ttf",
    "SourceHanSerifSC-Regular.otf",
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
        // —— 正文：大字符集宋体（生僻字全覆盖 + 竖排标点挤压）——
        FontChoice::WenJinMincho => vec![
            "WenJin Mincho",
            "WenJinMincho",
            "WenJinMinchoP0-Regular",
            "文津宋体",
            "Source Han Serif SC",
            "Songti SC",
            "SimSun",
            "serif",
        ],
        FontChoice::MengYuanSerif => vec![
            "MengYuan Serif",
            "MengYuanSerif",
            "Mengyuan Serif",
            "梦源宋体",
            "Source Han Serif SC",
            "Songti SC",
            "SimSun",
            "serif",
        ],
        // —— 批注：楷体（两款互为回退，装了哪款都能出楷体味）——
        FontChoice::XuandongKaishu => vec![
            "Xuandong Kaishu",
            "XuandongKaishu",
            "玄冬楷书",
            "LXGW WenKai",
            "Kaiti SC",
            "STKaiti",
            "serif",
        ],
        FontChoice::LXGWWenKai => vec![
            "LXGW WenKai",
            "LXGWWenKai",
            "LXGW WenKai GB",
            "霞鹜文楷",
            "Xuandong Kaishu",
            "Kaiti SC",
            "STKaiti",
            "serif",
        ],
        // —— 等宽 / 西文 ——
        FontChoice::ShangguMono => vec![
            "Shanggu Mono",
            "ShangguMono",
            "尚古等宽",
            "Source Han Sans HW SC",
            "Sarasa Mono SC",
            "PingFang SC",
            "monospace",
            "serif",
        ],
        FontChoice::LatinModern => vec![
            "Latin Modern Math",
            "Latin Modern Roman",
            "Latin Modern Mono",
            "CMU Serif",
            "Computer Modern",
            "Times New Roman",
            "serif",
        ],
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
        // 宋体 / 楷体 / 等宽 / 西文：横笺里也照本族取字。
        // `MODERN` 组只列黑体，但字体页把 `CLASSIC` 一并陈列，选中的楷体
        // 若在横笺里退化成系统黑体，用户会以为「换了字体没生效」。
        FontChoice::WenJinMincho
        | FontChoice::MengYuanSerif
        | FontChoice::XuandongKaishu
        | FontChoice::LXGWWenKai
        | FontChoice::ShangguMono
        | FontChoice::LatinModern
        | FontChoice::SongTi
        | FontChoice::SourceHanSerif
        | FontChoice::ZhuqueFangsong
        | FontChoice::JinghuaOldSong
        | FontChoice::HuiwenMincho
        | FontChoice::HuiwenZhengkai
        | FontChoice::HanchanHuofang
        | FontChoice::HanchanJinshu => ancient_chain(choice),
        _ => vec![
            "system-ui",
            "PingFang SC",
            "Microsoft YaHei",
            "sans-serif",
        ],
    }
}

/// **批注字体**的族名：夹注小字、放大注卡专用。
///
/// 恒定走古文链（`Script::Ancient`）——楷体是古籍角色，横笺里没有楷体链，
/// 走现代链只会退化成系统黑体。
pub fn note_family(choice: FontChoice) -> SharedString {
    let chain = ancient_chain(choice);
    pick_font(&chain, AVAILABLE.get().map(|v| v.as_slice()).unwrap_or(&[]))
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
            // 链尾必须落到某个通用族，链首字体全没装时才不会变成豆腐块。
            assert!(
                chain.iter().any(|f| matches!(
                    *f,
                    "serif" | "sans-serif" | "monospace" | "system-ui"
                )),
                "{} 缺通用族兜底：{:?}",
                c.label(),
                chain
            );
        }
    }

    #[test]
    fn every_note_font_has_chain() {
        // 批注字体组（楷体二选一）同样必须有兜底。
        for c in FontChoice::NOTE {
            let chain = ancient_chain(c);
            assert!(chain.contains(&"serif"), "{} 缺 serif 兜底", c.label());
            // 楷体链里至少带一个系统楷体，未装开源楷体时仍是楷体味。
            assert!(
                chain.iter().any(|f| f.contains("Kaiti")),
                "{} 缺系统楷体回退：{:?}",
                c.label(),
                chain
            );
        }
    }

    #[test]
    fn wenjin_mincho_is_the_default_body_face_and_leads_the_chain() {
        // 正文首选文津宋体：它必须排在正文字体组首位，且链首就是它。
        assert_eq!(FontChoice::CLASSIC[0], FontChoice::WenJinMincho);
        assert_eq!(ancient_chain(FontChoice::WenJinMincho)[0], "WenJin Mincho");
        // 生僻字覆盖靠「大字库」：链里必须带大字符集宋体兜底。
        let chain = ancient_chain(FontChoice::WenJinMincho);
        assert!(chain.contains(&"Source Han Serif SC"));
    }

    #[test]
    fn kai_faces_cross_fall_back_to_each_other() {
        // 玄冬楷书与霞鹜文楷互为回退：只装了其中一款，另一款也出楷体。
        let xu = ancient_chain(FontChoice::XuandongKaishu);
        let lx = ancient_chain(FontChoice::LXGWWenKai);
        assert!(xu.contains(&"LXGW WenKai"));
        assert!(lx.iter().any(|f| f.starts_with("Xuandong")));
    }

    #[test]
    fn latin_chain_reaches_computer_modern_before_generic_serif() {
        let chain = ancient_chain(FontChoice::LatinModern);
        let cm = chain
            .iter()
            .position(|f| f.starts_with("Latin Modern") || f.starts_with("CMU"));
        let serif = chain.iter().position(|f| *f == "serif");
        assert!(cm.expect("应有 CM 系族名") < serif.expect("应有 serif 兜底"));
    }

    #[test]
    fn font_configs_cover_all_four_roles() {
        // 四类角色齐全、id 互不相同、链尾都有通用族兜底。
        assert_eq!(FONT_CONFIGS.len(), 4);
        for r in FontRole::ALL {
            let c = r.cfg();
            assert!(c.chain.len() >= 3, "{} 链太短", c.id);
            assert!(
                c.chain.iter().any(|f| matches!(
                    *f,
                    "serif" | "sans-serif" | "monospace" | "system-ui"
                )),
                "{} 缺通用族兜底",
                c.id
            );
            assert_eq!(FontRole::by_id(c.id), Some(r));
        }
        // 正文首推文津宋体、夹注首推霞鹜文楷、西文首推 CM、界面首推思源黑体。
        assert_eq!(FontRole::Main.cfg().chain[0], "WenJin Mincho");
        assert_eq!(FontRole::Note.cfg().chain[0], "LXGW WenKai");
        assert_eq!(FontRole::Latin.cfg().chain[0], "CMU Serif");
        assert_eq!(FontRole::Ui.cfg().chain[0], "Source Han Sans SC");
        // 角色 id 唯一。
        let mut ids: Vec<&str> = FONT_CONFIGS.iter().map(|c| c.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn role_family_picks_the_first_installed_name() {
        let avail = vec!["Songti SC".to_string()];
        // 未装文津 / 思源时，正文落到系统宋体而不是链首（避免豆腐块）。
        assert_eq!(
            pick_font(FontRole::Main.cfg().chain, &avail),
            SharedString::new("Songti SC")
        );
        // 一台都没有时退回链首，交给系统兜底。
        assert_eq!(
            pick_font(FontRole::Latin.cfg().chain, &[]),
            SharedString::new("CMU Serif")
        );
    }

    #[test]
    fn line_height_scale_is_tighter_for_song_and_looser_for_kai() {
        // 楷体笔势舒展 → 行间余量必须大于宋体，否则上下两字粘连。
        assert!(line_height_scale(FontChoice::LXGWWenKai) > line_height_scale(FontChoice::WenJinMincho));
        assert!(
            (line_height_scale(FontChoice::WenJinMincho) - LINE_SCALE_SONG).abs() < f32::EPSILON
        );
        assert!((line_height_scale(FontChoice::XuandongKaishu) - LINE_SCALE_KAI).abs() < f32::EPSILON);
        // 默认档不得小于 1（否则字会重叠），也不得过大（否则行气散）。
        for c in FontChoice::CLASSIC {
            let s = line_height_scale(c);
            assert!((1.0..1.25).contains(&s), "{} 行高缩放 {:.2} 越界", c.label(), s);
        }
    }

    #[test]
    fn presets_keep_body_and_note_in_different_faces() {
        // 两种排版字体里，正文与批注**都不能是同一款**——刻本的层次靠这个。
        for p in FontPreset::ALL {
            assert_ne!(p.main_choice(), p.note_choice(), "{} 正文批注同体", p.label());
            // 行高缩放随正文那款字体走。
            assert_eq!(preset_line_scale(p), line_height_scale(p.main_choice()));
        }
        // 刻本宋体：正文宋、批注楷；手写楷体：正文楷、批注另一款楷。
        assert_eq!(FontPreset::Song.main_choice(), FontChoice::WenJinMincho);
        assert_eq!(FontPreset::Song.note_choice(), FontChoice::LXGWWenKai);
        assert_eq!(FontPreset::Kai.main_choice(), FontChoice::LXGWWenKai);
        assert_eq!(FontPreset::Kai.note_choice(), FontChoice::XuandongKaishu);
    }
}
