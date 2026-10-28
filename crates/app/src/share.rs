//! 分享卡原生渲染引擎（v1.53.0）。
//!
//! # 为什么重写
//!
//! 旧版分享卡走「截阅读页原图 + 半透明遮罩」：截图依赖 `screencapture`
//! 时序、遮罩压掉正文细节，且截帧路径上的偶发 panic 会撞 GPUI↔ObjC 的
//! FFI 边界把整个进程强杀（v1.52.5 的导出闪退即此类）。v1.53.0 起整条
//! 作废，改为**纯 GPUI `div()` 原生拼装**：
//!
//!   1. 纯色背景 + 重排版文字——不截原图、不加遮罩，纸上只有设计过的内容；
//!   2. 卡片宽高比与内部列数跟随阅读版式（8/10/12/14 列自适应纸张）；
//!   3. 两种文字方向：竖排右起（复古，默认）/ 横排居中（现代）；
//!   4. 四套模板只换数据（背景 / 正文 / 落款 / 点缀四色 + 装饰式样），
//!      渲染逻辑完全共用；
//!   5. 左下角恒有一枚朱砂「笺讀」印——软件签名。
//!
//! # 防崩溃铁律（用户 v1.53.0 明令）
//!
//! 本模块及所有调用方**全链路零 `unwrap()`**：颜色 / 字体 / 路径 / 下标
//! 一律 `unwrap_or` / `match` 兜底。字体缺失时 `fonts::family_for` 沿
//! 回退链落到系统字体（链尾 `serif`），绝不 panic——GPUI 事件处理器里
//! 的 panic 会穿过 `extern "C"` 边界被 `panic_cannot_unwind` 强杀，
//! 连 jiandu 栈帧都留不下（见 `~/.jiandu/panic.log` 的实证）。

use gpui::{
    canvas, div, point, px, rgb, rgba, Div, FontWeight, ParentElement, PathBuilder, SharedString,
    Styled, Window,
};
use crate::types::{FontChoice, Script};

// ----------------------------------------------------------------------
// 文字方向
// ----------------------------------------------------------------------

/// 文字方向：竖排右起（复古刻本）/ 横排居中（现代诗签）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShareDir {
    /// 竖排右起：列自右向左，如刻本古籍。
    Vertical,
    /// 横排居中：行自上而下，居中排布。
    Horizontal,
}

impl ShareDir {
    pub const ALL: [ShareDir; 2] = [ShareDir::Vertical, ShareDir::Horizontal];

    pub fn toggle(self) -> Self {
        match self {
            ShareDir::Vertical => ShareDir::Horizontal,
            ShareDir::Horizontal => ShareDir::Vertical,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ShareDir::Vertical => "竖排右起",
            ShareDir::Horizontal => "横排居中",
        }
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, ShareDir::Vertical)
    }
}

// ----------------------------------------------------------------------
// 面板页签
// ----------------------------------------------------------------------

/// 分享面板的两个页签：样式（配色 / 版式 / 开关）/ 字体。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShareTab {
    Style,
    Font,
}

impl ShareTab {
    pub const ALL: [ShareTab; 2] = [ShareTab::Style, ShareTab::Font];

    pub fn label(self) -> &'static str {
        match self {
            ShareTab::Style => "样式",
            ShareTab::Font => "字体",
        }
    }
}

// ----------------------------------------------------------------------
// 模板
// ----------------------------------------------------------------------

/// 卡片装饰式样（每套模板的「点缀」落地方式）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Deco {
    /// 澄心堂纸：双线界框 + 竖丝栏 + 朱砂印，正统古籍内页。
    Classic,
    /// 青绿山水：卡片通体贴一道极细线框，无丝栏，大留白。
    Airy,
    /// 拓片金石：暗金双线界框 + 竖丝栏 + 粗体，碑帖气。
    Rubbing,
    /// 夜读幽兰：顶部蓝色微光晕 + 底部一道细线。
    Night,
}

/// 一套分享模板（用户 v1.53.0 给定的四色参数 + 装饰式样）。
pub struct ShareTpl {
    pub name: &'static str,
    /// 背景色（纯色；「微纹理」由装饰层承担，不引入位图）。
    pub bg: u32,
    /// 正文文字色。
    pub ink: u32,
    /// 书名 / 落款 / 日期色。
    pub meta: u32,
    /// 装饰边框 / 点缀色。
    pub accent: u32,
    pub deco: Deco,
    /// 正文加粗（拓片金石：厚重宋体，视觉冲击）。
    pub bold: bool,
    /// 字号放大系数（拓片金石略放大）。
    pub fs_boost: f32,
    /// 该模板建议的文字方向（用户未手动指定时随模板切换）。
    pub suggest_dir: ShareDir,
}

/// 四套模板。颜色参数逐字取自用户 v1.53.0 的配置指令。
pub const TEMPLATES: [ShareTpl; 4] = [
    // 模板 1：澄心堂纸（经典米黄）——最正统古籍风，竖排右起，文字两端留足空白。
    ShareTpl {
        name: "澄心堂纸",
        bg: 0xF5F4EE,
        ink: 0x2B2B2B,
        meta: 0x888888,
        accent: 0xC04838,
        deco: Deco::Classic,
        bold: false,
        fs_boost: 1.0,
        suggest_dir: ShareDir::Vertical,
    },
    // 模板 2：青绿山水（清雅宋韵）——极细石绿线框，留白多一点，呼吸感。
    ShareTpl {
        name: "青绿山水",
        bg: 0xE8F0EC,
        ink: 0x3A4A44,
        meta: 0x6B7D76,
        accent: 0x8A9A5B,
        deco: Deco::Airy,
        bold: false,
        fs_boost: 1.0,
        suggest_dir: ShareDir::Vertical,
    },
    // 模板 3：拓片金石（厚重历史）——纯黑拓片底，暗金界框，粗体大字。
    ShareTpl {
        name: "拓片金石",
        bg: 0x1A1A1A,
        ink: 0xE6E6E6,
        meta: 0x999999,
        accent: 0xB8860B,
        deco: Deco::Rubbing,
        bold: true,
        fs_boost: 1.18,
        suggest_dir: ShareDir::Vertical,
    },
    // 模板 4：夜读幽兰（护眼暗黑）——横排居中，顶部蓝色微光晕。
    ShareTpl {
        name: "夜读幽兰",
        bg: 0x222733,
        ink: 0xD1D5DB,
        meta: 0x7A8599,
        accent: 0x4A90E2,
        deco: Deco::Night,
        bold: false,
        fs_boost: 1.0,
        suggest_dir: ShareDir::Horizontal,
    },
];

/// 兜底模板（理论不可达：`TEMPLATES` 是常量数组，恒有首元素）。
/// 单独拎出来是为了让 `tpl()` 做到**零 unwrap**。
const FALLBACK_TPL: ShareTpl = ShareTpl {
    name: "澄心堂纸",
    bg: 0xF5F4EE,
    ink: 0x2B2B2B,
    meta: 0x888888,
    accent: 0xC04838,
    deco: Deco::Classic,
    bold: false,
    fs_boost: 1.0,
    suggest_dir: ShareDir::Vertical,
};

/// 取模板：越界回退到第一套，**绝不 panic**。
pub fn tpl(i: usize) -> &'static ShareTpl {
    match TEMPLATES.get(i) {
        Some(t) => t,
        None => &FALLBACK_TPL,
    }
}

/// 朱砂红：左下角印的印泥色（各底色通用，软件签名）。
pub const CINNABAR: u32 = 0xC3272D;

// ----------------------------------------------------------------------
// v1.56.0 新制：底色 / 版框 / 印章 三轴独立
//
// 旧制是「四套模板」——底色、版框、装饰被捆在一枚下标里，改底色就
// 连带换框。用户 v1.56.0 明确要求三者各自成轴：背景三档、版框三档，
// 任意组合都得成立。故旧 `TEMPLATES` 整套降级为历史数据（不再参与
// 渲染，按「渐进式重构」保留），渲染一律走下面的三轴。
// ----------------------------------------------------------------------

/// 卡片底色三档。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShareBg {
    /// 宣纸黄：最正统的古籍纸色，暖米黄。
    Xuan,
    /// 暗金黑：近墨的深褐金，夜间发朋友圈最压场。
    Gold,
    /// 极简纯白：现代极简，无一丝杂色。
    Plain,
}

impl ShareBg {
    pub const ALL: [ShareBg; 3] = [ShareBg::Xuan, ShareBg::Gold, ShareBg::Plain];

    pub fn label(self) -> &'static str {
        match self {
            ShareBg::Xuan => "宣纸黄",
            ShareBg::Gold => "暗金黑",
            ShareBg::Plain => "极简白",
        }
    }
}

/// 一套底色的四色：底 / 正文墨 / 落款淡墨 / 点缀朱砂。
pub struct Palette {
    pub bg: u32,
    pub ink: u32,
    pub meta: u32,
    /// 点缀色（版框红线的取色：暗底上要提亮，否则朱砂发闷）。
    pub accent: u32,
}

/// 底色 → 四色。数值逐档调过：暗金黑上的朱砂要提亮一档才不糊。
pub fn palette(b: ShareBg) -> Palette {
    match b {
        ShareBg::Xuan => Palette {
            bg: 0xF2E8D3,
            ink: 0x2A2622,
            meta: 0x8A7561,
            accent: 0xB03A2E,
        },
        ShareBg::Gold => Palette {
            bg: 0x1B1813,
            ink: 0xE8DFC9,
            meta: 0x9C8A6A,
            accent: 0xC9553C,
        },
        ShareBg::Plain => Palette {
            bg: 0xFFFFFF,
            ink: 0x1A1A1A,
            meta: 0x9A9A9A,
            accent: 0xC3272B,
        },
    }
}

/// 版框三档：红线框 / 黑线框 / 无边框。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShareFrame {
    /// 朱丝栏：红线双界框。
    Red,
    /// 乌丝栏：墨线双界框。
    Black,
    /// 无框：只留纸与字。
    None,
}

impl ShareFrame {
    pub const ALL: [ShareFrame; 3] = [ShareFrame::Red, ShareFrame::Black, ShareFrame::None];

    pub fn label(self) -> &'static str {
        match self {
            ShareFrame::Red => "红线框",
            ShareFrame::Black => "黑线框",
            ShareFrame::None => "无边框",
        }
    }
}

/// 版框取色：无框返回 `None`（调用方据此一层不画）。
pub fn frame_ink(f: ShareFrame, p: &Palette) -> Option<u32> {
    match f {
        ShareFrame::Red => Some(p.accent),
        ShareFrame::Black => Some(p.ink),
        ShareFrame::None => None,
    }
}

/// 印章两式：双鱼尾（软件标识）/ 书名篆意二字。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShareSeal {
    /// 双鱼尾：白文满印，纸色挖出上下两枚鱼尾。
    Twin,
    /// 书名：朱文细边方印，内刻书名首二字。
    Book,
}

impl ShareSeal {
    pub const ALL: [ShareSeal; 2] = [ShareSeal::Twin, ShareSeal::Book];

    pub fn label(self) -> &'static str {
        match self {
            ShareSeal::Twin => "双鱼尾印",
            ShareSeal::Book => "书名印",
        }
    }
}

/// 印文：取书名前两个有效字；书名空则回退「笺讀」。
pub fn seal_chars(book: &str) -> String {
    let s: String = book
        .chars()
        .filter(|c| !matches!(c, '《' | '》' | '·' | '（' | '）' | '(' | ')' | ' ' | '　' | '[' | ']'))
        .take(2)
        .collect();
    if s.is_empty() {
        "笺讀".to_string()
    } else {
        s
    }
}

// ----------------------------------------------------------------------
// 版面自适应
// ----------------------------------------------------------------------

/// 阅读版式的合法列数（`grid::LAYOUT_CONFIGS`：8/10/12/14）。
pub const COLUMN_STYLES: [usize; 4] = [8, 10, 12, 14];

/// 把任意列数归到 8/10/12/14 四档之一（取最近档）。
pub fn norm_columns(n: usize) -> usize {
    let mut best = COLUMN_STYLES[0];
    let mut best_d = usize::MAX;
    for &c in COLUMN_STYLES.iter() {
        let d = (n as isize - c as isize).unsigned_abs();
        if d < best_d {
            best_d = d;
            best = c;
        }
    }
    best
}

/// 卡片宽高比按列数自适应：列越多纸越宽（8 列≈0.64 … 14 列≈0.75）。
/// 竖排卡片本就是「竖长书衣」比例，宽高比 < 1。
pub fn card_ratio(columns: usize) -> f32 {
    let c = (columns as f32).clamp(8.0, 14.0);
    (0.50 + 0.018 * c).clamp(0.55, 0.80)
}

/// 预览卡片高（px）——**一档定值**，不再有滑杆（v1.64.0 用户点名删）。
///
/// 取「视口高减去抬头 / 图注 / 页签 / 控件 / 按钮所占的那一段」，再上下夹住：
/// 窗口再小也留得住 240px 的卡，再大也不让一张预览卡撑到荒唐（封顶 720）。
/// 旧制那根竖向滑杆正是为了绕开 cap 才把五档线性铺开的——既然档位没了，
/// 这套绕法连同 `H_STEPS` 一起删净，只留一个确定值。
pub fn card_h_for(vh: f32) -> f32 {
    (vh - 326.0).clamp(240.0, 720.0)
}

// ----------------------------------------------------------------------
// 字体
// ----------------------------------------------------------------------

/// 字体页签的可选项：`None` = 跟随阅读正文字体。
pub const SHARE_FONTS: [(&str, Option<FontChoice>); 6] = [
    ("随阅读字体", None),
    ("齐伋体", Some(FontChoice::Qiji)),
    ("文津宋体", Some(FontChoice::WenJinMincho)),
    ("梦源宋体", Some(FontChoice::MengYuanSerif)),
    ("玄冬楷书", Some(FontChoice::XuandongKaishu)),
    ("霞鹜文楷", Some(FontChoice::LXGWWenKai)),
];

/// 解析分享卡正文字体族名。
///
/// 防崩溃：`family_for` 内部沿回退链挑系统真正拥有的字体（链尾 `serif`），
/// 字体文件缺失时自动落到系统字体——不读文件、不 panic。
pub fn card_family(share_font: Option<usize>, reader_family: SharedString) -> SharedString {
    let choice = share_font
        .and_then(|i| SHARE_FONTS.get(i))
        .and_then(|(_, c)| *c);
    match choice {
        Some(c) => crate::fonts::family_for(c, Script::Ancient),
        None => reader_family,
    }
}

// ----------------------------------------------------------------------
// 日期
// ----------------------------------------------------------------------

/// 0–99 的整数转中文数字（月 / 日用）。
pub fn cn_small(n: usize) -> String {
    const D: [&str; 10] = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    match n {
        0..=9 => D[n].to_string(),
        10 => "十".to_string(),
        11..=19 => format!("十{}", D[n - 10]),
        20..=99 => {
            let tens = D[n / 10];
            if n.is_multiple_of(10) {
                format!("{tens}十")
            } else {
                format!("{tens}十{}", D[n % 10])
            }
        }
        _ => n.to_string(),
    }
}

/// 把「2026-10-18」转成「二〇二六年十月十八日」；解析失败返回空串。
pub fn cn_date_from_str(ymd: &str) -> String {
    let p: Vec<&str> = ymd.split('-').collect();
    if p.len() != 3 {
        return String::new();
    }
    let m = match p[1].parse::<usize>() {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let d = match p[2].parse::<usize>() {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    if p[0].is_empty() || !p[0].chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    let ys: String = p[0]
        .chars()
        .map(|c| match c {
            '0' => "〇",
            '1' => "一",
            '2' => "二",
            '3' => "三",
            '4' => "四",
            '5' => "五",
            '6' => "六",
            '7' => "七",
            '8' => "八",
            '9' => "九",
            _ => "",
        })
        .collect();
    format!("{ys}年{}月{}日", cn_small(m), cn_small(d))
}

// ----------------------------------------------------------------------
// v1.82.0：出图时间戳（右下角极简落款）
//
// 用户口径：导出图片的右下角加一行生成时间，**极简、字号最小档、颜色淡、
// 不破坏版面**。故它不走 `show_date` 那条「版心内左下竖排列」的老路——那条
// 会占 `reserve` 竖带、影响正文网格宽。时间戳单独画在**地脚带**（版框下缘
// 那一圈留白）里，一行小字，版式一个数都不用改。
// ----------------------------------------------------------------------

/// 出图时间戳的三档。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StampStyle {
    /// 古风：干支纪年 + 月季 + 浣（默认——对着一叶古籍，落个公历反而不搭）。
    #[default]
    Classic,
    /// 公历：`2026.10.28`。
    Plain,
    /// 不落款。
    Off,
}

impl StampStyle {
    pub const ALL: [StampStyle; 3] = [StampStyle::Classic, StampStyle::Plain, StampStyle::Off];

    /// 菜单上那一行的名字（短、能一眼看出当前档）。
    pub fn label(&self) -> &'static str {
        match self {
            StampStyle::Classic => "古风",
            StampStyle::Plain => "公历",
            StampStyle::Off => "关",
        }
    }

    /// 点一下切下一档（菜单项是循环的，不再开二级面板）。
    pub fn next(self) -> StampStyle {
        match self {
            StampStyle::Classic => StampStyle::Plain,
            StampStyle::Plain => StampStyle::Off,
            StampStyle::Off => StampStyle::Classic,
        }
    }
}

/// 十天干。
pub const GAN: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
/// 十二地支。
pub const ZHI: [&str; 12] = ["子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥"];

/// 干支纪年（如「丙午」）。
///
/// 干支年以**立春**（约 2 月 4 日）为界，不是元旦：2026-01-20 仍属乙巳年。
/// 这里不用节气表，只取 2 月 4 日这个近似分界——差最多一两天，落在水印上
/// 没人看得出来；真要考据另有别处，水印只管好看。
pub fn ganzhi_year(y: i64, m: u32, d: u32) -> String {
    let y = if m < 2 || (m == 2 && d < 4) { y - 1 } else { y };
    let i = (y - 4).rem_euclid(60) as usize;
    format!("{}{}", GAN[i % 10], ZHI[i % 12])
}

/// 月季雅称（孟 / 仲 / 季 + 春夏秋冬）。
///
/// ⚠️ 月份按**农历月 ≈ 公历月 − 1** 近拟（春节通常在 1 月下旬到 2 月中旬，
/// 农历月整体滞后公历约一个月），误差最多一个月。水印只求一眼的古意，
/// 不作考据用——故下面那行注释把它写明白，免得日后有人拿它对校历书。
fn season_of_month(m: u32) -> &'static str {
    // 农历月 = 公历月 − 1（1 月回卷为 11 → 季冬）。
    let lm = if m == 1 { 12 } else { m - 1 };
    let name = match lm {
        1..=3 => "春",
        4..=6 => "夏",
        7..=9 => "秋",
        _ => "冬",
    };
    match (lm - 1) % 3 {
        0 => match name {
            "春" => "孟春",
            "夏" => "孟夏",
            "秋" => "孟秋",
            _ => "孟冬",
        },
        1 => match name {
            "春" => "仲春",
            "夏" => "仲夏",
            "秋" => "仲秋",
            _ => "仲冬",
        },
        _ => match name {
            "春" => "季春",
            "夏" => "季夏",
            "秋" => "季秋",
            _ => "季冬",
        },
    }
}

/// 旬的雅称：上浣 / 中浣 / 下浣（1–10 / 11–20 / 21–月末）。
fn xun_of_day(d: u32) -> &'static str {
    match d {
        1..=10 => "上浣",
        11..=20 => "中浣",
        _ => "下浣",
    }
}

/// 今天的（年, 月, 日）；系统时钟不可信时返回 `None`（调用方按「不落款」处理）。
pub fn today_ymd() -> Option<(i64, u32, u32)> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let (y, m, d) = civil_from_days(secs / 86_400);
    Some((y, m, d))
}

/// 按档生成时间戳文本；`Off` 或时钟异常一律给空串（空串＝不画）。
pub fn stamp_text(style: StampStyle) -> String {
    let (y, m, d) = match today_ymd() {
        Some(v) => v,
        None => return String::new(),
    };
    match style {
        StampStyle::Off => String::new(),
        StampStyle::Plain => format!("{y:04}.{m:02}.{d:02}"),
        StampStyle::Classic => format!("{}年{}{}", ganzhi_year(y, m, d), season_of_month(m), xun_of_day(d)),
    }
}

/// 取系统当前日期的中文形式；失败返回空串（调用方按「不显示」处理）。
///
/// v1.70.0：**不再 fork 一个 `date` 子进程**（App Store 沙盒下外部进程
/// 随时可能被拦，且为了拿一个本地日期去起进程纯属浪费）。改用 Rust 标准
/// 库自己把 epoch 秒转成日历年月日——闰年规则写死在下面，无第三方依赖，
/// 2038 年之前都不会错。
pub fn cn_date_today() -> String {
    let secs = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return String::new(),
    };
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    cn_date_from_str(&format!("{y:04}-{m:02}-{d:02}"))
}

/// epoch 天 → (年, 月, 日)。Howard Hinnant 的 civil_from_days 算法：
/// 先按 400 年一循环把日子挪进 [0, 400年) 区间，再逐月减法。纯整数、
/// 无查表、无依赖。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ----------------------------------------------------------------------
// 卡片渲染
// ----------------------------------------------------------------------

/// 一张分享卡的全部输入（调用方快照，渲染期只读）。
pub struct CardSpec<'a> {
    /// 正文（已按繁简开关转过形）。
    pub body: &'a [char],
    /// 题识：章节 · 书名 · 作者（已裁切）。
    pub sign: &'a str,
    /// 中文日期（空串 = 不显示）。
    pub date: &'a str,
    /// **朱批标记**：与 `body` 等长——第 i 个字是否被用户划了红线。
    pub red: &'a [bool],
    /// **眉批正文**（已按条数 / 字数裁切；空串 = 不画批注区）。
    pub note: &'a str,
    pub show_sign: bool,
    pub show_date: bool,
    /// 是否显示批注（朱批红字 + 眉批小字 + 引线）。关 = 只分享正文。
    pub show_notes: bool,
    pub dir: ShareDir,
    /// 底色（宣纸黄 / 暗金黑 / 极简白）。
    pub bg: ShareBg,
    /// 版框（红线 / 黑线 / 无框）。
    pub frame: ShareFrame,
    /// 印章式样。
    pub seal: ShareSeal,
    /// 印文（书名首二字，见 [`seal_chars`]）。
    pub seal_text: &'a str,
    /// 正文字体族名（已做缺失回退）。
    pub family: SharedString,
    /// 阅读版式列数（8/10/12/14，决定内部列数与纸张比例）。
    pub columns: usize,
    pub w: f32,
    pub h: f32,
}

/// 组装一张分享卡（纯 GPUI 元素，零副作用、零 unwrap）。
pub fn build_card(spec: &CardSpec) -> Div {
    let p = palette(spec.bg);
    let w = if spec.w.is_finite() { spec.w } else { 360.0 }.max(160.0);
    let h = if spec.h.is_finite() { spec.h } else { 480.0 }.max(200.0);
    let pad = (w * 0.075).clamp(18.0, 46.0);

    // 卡片本体：纯色底 + 裁切（版框 / 光晕都是 absolute，溢出必须收）。
    let mut card = div()
        .relative()
        .w(px(w))
        .h(px(h))
        .bg(rgb(p.bg))
        .overflow_hidden();

    // ---- 版框（红线 / 黑线 / 无框；双线界框，刻本体例）----
    if let Some(ink) = frame_ink(spec.frame, &p) {
        for layer in frame_layers(ink, w, h, pad) {
            card = card.child(layer);
        }
    }

    // ---- 内容层 ----
    let inner_w = (w - pad * 2.0).max(80.0);
    let inner_h = (h - pad * 2.0).max(120.0);
    let content = match spec.dir {
        ShareDir::Vertical => vertical_content(&p, spec, inner_w, inner_h),
        ShareDir::Horizontal => horizontal_content(&p, spec, inner_w, inner_h),
    };
    card = card.child(
        div()
            .absolute()
            .left(px(pad))
            .top(px(pad))
            .w(px(inner_w))
            .h(px(inner_h))
            .child(content),
    );

    // ---- 朱砂印：左下角（软件签名，三底通用）----
    {
        let seal_size = (w * 0.085).clamp(28.0, 44.0);
        let inset = (pad * 0.55).max(8.0);
        card = card.child(
            div()
                .absolute()
                .left(px(inset))
                .bottom(px(inset))
                .child(seal_div(
                    spec.seal,
                    p.bg,
                    seal_size,
                    spec.seal_text,
                    spec.family.clone(),
                )),
        );
    }
    card
}

/// 版框两层：外线淡、内线深，两线之间即「界行」——刻本的规矩。
fn frame_layers(ink: u32, w: f32, h: f32, pad: f32) -> Vec<Div> {
    let o1 = (pad * 0.38).max(5.0);
    let o2 = (pad * 0.66).max(9.0);
    vec![
        div()
            .absolute()
            .left(px(o1))
            .top(px(o1))
            .w(px((w - o1 * 2.0).max(20.0)))
            .h(px((h - o1 * 2.0).max(20.0)))
            .border_1()
            .border_color(rgba((ink << 8) | 0x4D))
            .rounded(px(1.0)),
        div()
            .absolute()
            .left(px(o2))
            .top(px(o2))
            .w(px((w - o2 * 2.0).max(20.0)))
            .h(px((h - o2 * 2.0).max(20.0)))
            .border_1()
            .border_color(rgba((ink << 8) | 0xA6))
            .rounded(px(1.0)),
    ]
}

/// 装饰层：界框 / 丝栏由 `vertical_content` 画（要与列对齐），
/// 这里只画不随内容变的部分——外框、夜读光晕、底线。
fn deco_layers(t: &ShareTpl, w: f32, h: f32, pad: f32) -> Vec<Div> {
    let mut out: Vec<Div> = Vec::new();
    match t.deco {
        Deco::Classic | Deco::Rubbing => {
            // 双线界框（刻本体例）：外线淡、内线深，两线之间即「界行」。
            let frame_ink = if matches!(t.deco, Deco::Rubbing) {
                t.accent
            } else {
                t.ink
            };
            let o1 = (pad * 0.40).max(6.0);
            let o2 = (pad * 0.68).max(10.0);
            out.push(
                div()
                    .absolute()
                    .left(px(o1))
                    .top(px(o1))
                    .w(px((w - o1 * 2.0).max(20.0)))
                    .h(px((h - o1 * 2.0).max(20.0)))
                    .border_1()
                    .border_color(rgba((frame_ink << 8) | 0x59))
                    .rounded(px(1.0)),
            );
            out.push(
                div()
                    .absolute()
                    .left(px(o2))
                    .top(px(o2))
                    .w(px((w - o2 * 2.0).max(20.0)))
                    .h(px((h - o2 * 2.0).max(20.0)))
                    .border_1()
                    .border_color(rgba((frame_ink << 8) | 0x33))
                    .rounded(px(1.0)),
            );
        }
        Deco::Airy => {} // 线框已在卡片本体上
        Deco::Night => {
            // 顶部蓝色微光晕：GPUI 0.2.2 无渐变 API，用四层递减透明度
            // 叠出来——自上而下 0x24 / 0x18 / 0x0E / 0x07，高度递增。
            let bands: [(f32, u32); 4] = [
                (34.0, 0x26),
                (64.0, 0x18),
                (104.0, 0x0E),
                (156.0, 0x07),
            ];
            for (bh, a) in bands.iter() {
                out.push(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .w(px(w))
                        .h(px(*bh))
                        .bg(rgba((t.accent << 8) | *a)),
                );
            }
            // 底部一道极细线（收住光晕，也给深色卡一条「纸边」）。
            out.push(
                div()
                    .absolute()
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .w(px(w))
                    .h(px(1.0))
                    .bg(rgba((t.accent << 8) | 0x55)),
            );
        }
    }
    out
}

/// **朱砂印（v1.56.0 Canvas 手绘）**：一整块 `canvas` 画印面，字层另起。
///
/// 两式：
/// * [`ShareSeal::Twin`] —— **白文满印**：整方填朱砂，再用**纸色**挖出
///   上下两枚鱼尾（图形取 `banxin::fishtail_points`，与版心鱼尾同一副刀法），
///   中间留白口即「双鱼尾」标识；
/// * [`ShareSeal::Book`] —— **朱文细边**：只描一圈朱砂方框，框内书名首
///   二字由上层 div 以朱砂写出（印面干净、字口清楚）。
///
/// 挖白用的是**底色**而不是白：暗金黑上挖出的鱼尾是深褐而非纯白，
/// 才像「印泥盖在纸上」，不像贴了块白胶布。
pub fn seal_div(kind: ShareSeal, paper: u32, size: f32, text: &str, family: SharedString) -> Div {
    let red = CINNABAR;
    let ground = paper;
    let chars: Vec<char> = if text.is_empty() {
        "笺讀".chars().collect()
    } else {
        text.chars().take(2).collect()
    };

    let mut d = div().relative().w(px(size)).h(px(size));
    // ---- 印面（Canvas）----
    d = d.child(
        canvas(
            move |_b, _window, _cx| {},
            move |b, _s, window: &mut Window, _cx| {
                let unit = px(1.0);
                let w = b.size.width / unit;
                let h = b.size.height / unit;
                let x0 = b.origin.x / unit;
                let y0 = b.origin.y / unit;
                // 印面是一方，四角略收 0.5px——不顶边才像盖上去的。
                let square = [
                    point(px(x0 + 0.5), px(y0 + 0.5)),
                    point(px(x0 + w - 0.5), px(y0 + 0.5)),
                    point(px(x0 + w - 0.5), px(y0 + h - 0.5)),
                    point(px(x0 + 0.5), px(y0 + h - 0.5)),
                ];
                match kind {
                    ShareSeal::Twin => {
                        let mut sq = PathBuilder::fill();
                        sq.add_polygon(&square, true);
                        if let Ok(p) = sq.build() {
                            window.paint_path(p, rgb(red));
                        }
                        // 双鱼尾挖白：上鱼尾贴内上沿，下鱼尾贴内下沿，中间白口。
                        let inset = w * 0.15;
                        let inner_w = (w - inset * 2.0).max(4.0);
                        let inner_h = (h - inset * 2.0).max(8.0);
                        let th = inner_h * 0.40;
                        for (i, top) in [true, false].iter().enumerate() {
                            let ty = y0 + inset + (i as f32) * (inner_h - th);
                            paint_fishtail(
                                window,
                                *top,
                                x0 + inset,
                                ty,
                                inner_w,
                                th,
                                rgb(ground),
                            );
                        }
                    }
                    ShareSeal::Book => {
                        let mut sq = PathBuilder::stroke(px(1.3));
                        sq.add_polygon(&square, true);
                        if let Ok(p) = sq.build() {
                            window.paint_path(p, rgb(red));
                        }
                    }
                }
            },
        )
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(size))
        .h(px(size)),
    );

    // ---- 印文（仅朱文式；白文式的双鱼尾已由画布挖出）----
    if matches!(kind, ShareSeal::Book) {
        let mut col = div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(size))
            .h(px(size))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(0.5));
        for c in chars.iter() {
            col = col.child(
                div()
                    .font_family(family.clone())
                    .text_size(px(size * 0.34))
                    .text_color(rgb(red))
                    .child(SharedString::from(c.to_string())),
            );
        }
        d = d.child(col);
    }
    d
}

/// 在印面上画一枚鱼尾（直线多边形，绝不用贝塞尔——与版心鱼尾同一刀法）。
fn paint_fishtail(
    window: &mut Window,
    top: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: gpui::Rgba,
) {
    let pts = crate::banxin::fishtail_points(top, h);
    let poly: Vec<gpui::Point<gpui::Pixels>> = pts
        .iter()
        .map(|(fx, fy)| point(px(x + fx * w / 100.0), px(y + *fy)))
        .collect();
    let mut b = PathBuilder::fill();
    b.add_polygon(&poly, true);
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// 旧制印：朱砂底 + 纸色挖出「笺讀」二字（v1.53.0）。
/// 渲染已改用 [`seal_div`]，此函数按「渐进式重构」保留。
fn seal(t: &ShareTpl, family: SharedString, size: f32) -> Div {
    let mut s = div()
        .w(px(size))
        .h(px(size))
        .bg(rgb(CINNABAR))
        .rounded(px(2.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(0.5));
    for c in ['笺', '讀'] {
        s = s.child(
            div()
                .font_family(family.clone())
                .text_size(px(size * 0.36))
                .text_color(rgb(t.bg))
                .child(SharedString::from(c.to_string())),
        );
    }
    s
}

/// 竖排右起内容：日期列（最右）→ N 列正文 → 题识列（最左，印在其下）。
///
/// **8/10/12/14 列自适应纸张**：正文列数 = 阅读版式列数（归到四档），
/// 每列字数由内容均分，字号随「列数 × 列高」双向收敛——列多则字小、
/// 列少则字大，与阅读版面同一套版式逻辑。
fn vertical_content(p: &Palette, spec: &CardSpec, avail_w: f32, avail_h: f32) -> Div {
    let cols_style = norm_columns(spec.columns);
    let len = spec.body.len();
    let base_fs = (spec.h / 24.0).clamp(13.0, 26.0);
    let sign_fs = base_fs * 0.60;
    let date_fs = base_fs * 0.58;
    let note_fs = base_fs * 0.52;

    // 眉批区（朱砂小字 + 引线）只在「显示批注」且确有眉批时占位。
    let show_note = spec.show_notes && !spec.note.is_empty();
    let note_chars = if show_note {
        spec.note.chars().count()
    } else {
        0
    };

    let sign_w = if spec.show_sign && !spec.sign.is_empty() {
        sign_fs * 1.35
    } else {
        0.0
    };
    let date_w = if spec.show_date && !spec.date.is_empty() {
        date_fs * 1.25
    } else {
        0.0
    };
    let note_w = if show_note { note_fs * 1.55 } else { 0.0 };
    let col_gap = 5.0;
    let side_gap = 12.0;

    let body_cols = cols_style.min(len.max(1)).max(1);
    let per_col = ((len as f32) / (body_cols as f32)).ceil().max(1.0) as usize;

    // 字号双向收敛：先满足「每列 per_col 字放得下」，再满足「总宽放得下」
    // （总宽把题识 / 眉批 / 日期三列都算进去——开了眉批正文自动让位）。
    let mut fs = base_fs;
    loop {
        let ok_h = (per_col as f32) * (fs * 1.62) <= avail_h;
        let total_w =
            sign_w + date_w + note_w + side_gap * 2.0 + (body_cols as f32) * (fs + col_gap);
        if (ok_h && total_w <= avail_w) || fs <= 9.0 {
            break;
        }
        fs -= 0.5;
    }
    let fs = fs.min(30.0);

    // 视觉顺序（左→右）：题识列、正文列（倒序）、眉批列、日期列。
    let mut row = div()
        .flex()
        .flex_row()
        .items_start()
        .justify_center()
        .h_full()
        .gap(px(side_gap));

    // 题识列：竖排小字顶天，印由 `build_card` 钉在卡片左下角。
    if sign_w > 0.0 {
        row = row.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .flex_shrink_0()
                .w(px(sign_w))
                .child(vcol(spec.sign, sign_fs, p.meta, spec.family.clone())),
        );
    }

    // 正文列：右起 → flex_row 里倒序 push（第一段落在最右）。
    for (ci, chunk) in spec.body.chunks(per_col.max(1)).enumerate() {
        let base = ci * per_col.max(1);
        let mut c = div()
            .flex()
            .flex_col()
            .items_center()
            .flex_shrink_0()
            .w(px(fs + col_gap))
            .gap(px(col_gap * 0.55));
        for (i, ch) in chunk.iter().enumerate() {
            // 朱批：被划红线的字染朱砂、并在字右加一道细朱线（着重号）。
            let marked = spec.show_notes && spec.red.get(base + i).copied().unwrap_or(false);
            c = c.child(char_box(*ch, fs, p.ink, false, spec.family.clone(), marked));
        }
        row = row.child(c);
        // 乌丝栏：列界一道极细淡墨（朱丝栏的墨色版）。
        row = row.child(
            div()
                .w(px(0.5))
                .h_full()
                .bg(rgba((p.ink << 8) | 0x24))
                .flex_shrink_0(),
        );
    }

    // 眉批列：一道朱砂细竖线（引线）把批语挂回正文，顶端一横挑向被批处。
    if show_note {
        let note_h = (note_chars as f32) * note_fs * 1.32;
        row = row.child(
            div()
                .w(px(0.5))
                .h(px(note_h.min(avail_h).max(4.0)))
                .bg(rgba((CINNABAR << 8) | 0x77))
                .flex_shrink_0(),
        );
        row = row.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .flex_shrink_0()
                .w(px(note_w))
                .child(
                    div()
                        .w(px(note_w * 0.9))
                        .h(px(0.5))
                        .bg(rgba((CINNABAR << 8) | 0xBB)),
                )
                .child(vcol(spec.note, note_fs, CINNABAR, spec.family.clone())),
        );
    }

    // 日期列：最右，顶天。
    if date_w > 0.0 {
        row = row.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .flex_shrink_0()
                .w(px(date_w))
                .child(vcol(spec.date, date_fs, p.meta, spec.family.clone())),
        );
    }
    row
}

/// 横排居中内容：正文行居中，题识 / 日期小字居中垫在下方。
fn horizontal_content(p: &Palette, spec: &CardSpec, avail_w: f32, avail_h: f32) -> Div {
    let base_fs = (spec.h / 22.0).clamp(14.0, 28.0);
    let mut fs = base_fs;
    let mut cpl;
    loop {
        cpl = ((avail_w / (fs * 1.06)).floor() as usize).max(6);
        let lines = ((spec.body.len() as f32) / (cpl as f32)).ceil().max(1.0);
        // 行数 × 行高 + 落款两行 + 眉批一行，要放得下卡片高。
        let need = lines * (fs * 1.9) + fs * 3.4;
        if need <= avail_h || fs <= 11.0 {
            break;
        }
        fs -= 0.5;
    }
    let fs = fs.min(30.0);
    cpl = ((avail_w / (fs * 1.06)).floor() as usize).max(6);

    let mut col = div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .h_full()
        .gap(px(fs * 0.9));
    let line_h = fs * 1.9;
    for (ci, chunk) in spec.body.chunks(cpl).enumerate() {
        let base = ci * cpl;
        let mut row = div()
            .flex()
            .flex_row()
            .justify_center()
            .w_full()
            .line_height(px(line_h))
            .font_family(spec.family.clone())
            .text_size(px(fs))
            .text_color(rgb(p.ink));
        // 按「是否被划红线」切成一段段同色 run：朱批段落染朱砂。
        let mut run = String::new();
        let mut run_red = spec.show_notes && spec.red.get(base).copied().unwrap_or(false);
        for (i, ch) in chunk.iter().enumerate() {
            let r = spec.show_notes && spec.red.get(base + i).copied().unwrap_or(false);
            if r != run_red && !run.is_empty() {
                row = row.child(hspan(&run, run_red, p));
                run.clear();
            }
            run_red = r;
            run.push(*ch);
        }
        if !run.is_empty() {
            row = row.child(hspan(&run, run_red, p));
        }
        col = col.child(row);
    }
    // 眉批：正文下方一行朱砂小字（横排时引线即它上方那道留白的分界）。
    if spec.show_notes && !spec.note.is_empty() {
        col = col.child(
            div()
                .font_family(spec.family.clone())
                .text_size(px(fs * 0.58))
                .text_color(rgb(CINNABAR))
                .child(SharedString::from(spec.note.to_string())),
        );
    }
    // 落款：题识一行、日期一行，居中垫在正文下方。
    if spec.show_sign && !spec.sign.is_empty() {
        col = col.child(
            div()
                .font_family(spec.family.clone())
                .text_size(px(fs * 0.62))
                .text_color(rgb(p.meta))
                .child(SharedString::from(spec.sign.to_string())),
        );
    }
    if spec.show_date && !spec.date.is_empty() {
        col = col.child(
            div()
                .font_family(spec.family.clone())
                .text_size(px(fs * 0.56))
                .text_color(rgb(p.meta))
                .child(SharedString::from(spec.date.to_string())),
        );
    }
    col
}

/// 横排的一小段同色文字（`red = true` 即朱批段）。
fn hspan(s: &str, red: bool, p: &Palette) -> Div {
    div()
        .text_color(rgb(if red { CINNABAR } else { p.ink }))
        .child(SharedString::from(s.to_string()))
}

/// 单字槽位（竖排）：定宽定高，字身居中。
fn char_box(
    c: char,
    fs: f32,
    ink: u32,
    bold: bool,
    family: SharedString,
    marked: bool,
) -> Div {
    // 朱批字：字身染朱砂（朱色 > 墨色，一眼可辨「这是我划的」）。
    let color = if marked { CINNABAR } else { ink };
    let mut d = div()
        .relative()
        .w(px(fs))
        .h(px(fs * 1.42))
        .flex()
        .items_center()
        .justify_center()
        .font_family(family)
        .text_size(px(fs))
        .text_color(rgb(color))
        .child(SharedString::from(c.to_string()));
    if bold {
        d = d.font_weight(FontWeight(750.0));
    }
    if marked {
        // 着重线：竖排的着重号贴在字的右侧，一道细朱线。
        d = d.child(
            div()
                .absolute()
                .right(px(0.0))
                .top(px(fs * 0.20))
                .w(px(1.0))
                .h(px(fs * 0.72))
                .bg(rgba((CINNABAR << 8) | 0xCC)),
        );
    }
    d
}

/// 一列竖排小字（题识 / 日期）：逐字落槽，与正文同族。
fn vcol(text: &str, fs: f32, color: u32, family: SharedString) -> Div {
    let mut c = div().flex().flex_col().items_center().gap(px(1.0));
    for ch in text.chars() {
        c = c.child(
            div()
                .font_family(family.clone())
                .text_size(px(fs))
                .line_height(px(fs * 1.3))
                .text_color(rgb(color))
                .child(SharedString::from(crate::types::bracket_to_quote(ch).to_string())),
        );
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_columns_snaps_to_four_styles() {
        assert_eq!(norm_columns(8), 8);
        assert_eq!(norm_columns(10), 10);
        assert_eq!(norm_columns(12), 12);
        assert_eq!(norm_columns(14), 14);
        assert_eq!(norm_columns(9), 8); // 9 离 8 近
        assert_eq!(norm_columns(11), 10); // 11 与 10、12 等距，取先命中的小档
        assert_eq!(norm_columns(13), 12); // 13 与 12、14 等距，同样取小档
        assert_eq!(norm_columns(0), 8);
        assert_eq!(norm_columns(999), 14);
    }

    #[test]
    fn card_ratio_grows_with_columns() {
        let r8 = card_ratio(8);
        let r14 = card_ratio(14);
        assert!(r8 < r14, "列越多纸越宽");
        assert!((0.55..=0.80).contains(&r8));
        assert!((0.55..=0.80).contains(&r14));
        // 纸张永远是竖长比例（宽 < 高）。
        assert!(r14 < 1.0);
    }

    #[test]
    fn tpl_index_out_of_range_falls_back() {
        // 零 unwrap：越界下标必须回退而不是 panic。
        assert_eq!(tpl(99).name, "澄心堂纸");
        assert_eq!(tpl(0).name, "澄心堂纸");
        assert_eq!(tpl(3).name, "夜读幽兰");
    }

    #[test]
    fn four_templates_have_exact_user_colors() {
        assert_eq!(TEMPLATES[0].bg, 0xF5F4EE);
        assert_eq!(TEMPLATES[0].ink, 0x2B2B2B);
        assert_eq!(TEMPLATES[0].meta, 0x888888);
        assert_eq!(TEMPLATES[0].accent, 0xC04838);
        assert_eq!(TEMPLATES[1].bg, 0xE8F0EC);
        assert_eq!(TEMPLATES[1].ink, 0x3A4A44);
        assert_eq!(TEMPLATES[1].meta, 0x6B7D76);
        assert_eq!(TEMPLATES[1].accent, 0x8A9A5B);
        assert_eq!(TEMPLATES[2].bg, 0x1A1A1A);
        assert_eq!(TEMPLATES[2].ink, 0xE6E6E6);
        assert_eq!(TEMPLATES[2].meta, 0x999999);
        assert_eq!(TEMPLATES[2].accent, 0xB8860B);
        assert!(TEMPLATES[2].bold, "拓片金石必须粗体");
        assert_eq!(TEMPLATES[3].bg, 0x222733);
        assert_eq!(TEMPLATES[3].ink, 0xD1D5DB);
        assert_eq!(TEMPLATES[3].meta, 0x7A8599);
        assert_eq!(TEMPLATES[3].accent, 0x4A90E2);
    }

    #[test]
    fn card_height_leaves_room_for_chrome_and_is_capped() {
        // 任何视口的卡片高都必须给抬头 / 图注 / 页签 / 控件 / 按钮留位。
        for vh in [560.0f32, 700.0, 900.0, 1200.0, 1600.0] {
            let h = card_h_for(vh);
            assert!(h >= 240.0 && h <= 720.0, "卡片高越界：vh={vh} h={h}");
            if vh > 566.0 && vh < 1046.0 {
                assert!(h <= vh - 326.0 + 0.001, "必须给控件留位：vh={vh} h={h}");
            }
        }
        // 小窗不塌成 0、大屏不无限长。
        assert_eq!(card_h_for(300.0), 240.0);
        assert_eq!(card_h_for(4000.0), 720.0);
    }

    #[test]
    fn dir_toggle_roundtrip() {
        assert_eq!(ShareDir::Vertical.toggle(), ShareDir::Horizontal);
        assert_eq!(ShareDir::Horizontal.toggle(), ShareDir::Vertical);
        assert!(ShareDir::Vertical.is_vertical());
        assert!(!ShareDir::Horizontal.is_vertical());
    }

    #[test]
    fn cn_date_parses() {
        assert_eq!(cn_date_from_str("2026-10-18"), "二〇二六年十月十八日");
        assert_eq!(cn_date_from_str("2026-01-05"), "二〇二六年一月五日");
        assert_eq!(cn_date_from_str("2000-12-31"), "二〇〇〇年十二月三十一日");
        assert_eq!(cn_date_from_str(""), "");
        assert_eq!(cn_date_from_str("bad"), "");
        assert_eq!(cn_date_from_str("20x6-1a-2"), "");
    }

    #[test]
    fn card_family_falls_back_to_reader_font() {
        // 字体缺失 / 下标越界都不得 panic，只回退。
        let reader = SharedString::from("Songti SC");
        assert_eq!(card_family(None, reader.clone()), reader);
        let picked = card_family(Some(0), reader.clone());
        assert!(!picked.is_empty());
        let _ = card_family(Some(99), reader);
    }

    /// v1.70.0：本地日期换算（换掉 `date` 子进程后必须自证正确）。
    #[test]
    fn civil_from_days_matches_known_dates() {
        // 已知锚点：1970-01-01 = day 0。
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 闰年 2 月 29 日必须存在（2000 / 2024 是闰年，1900 / 2100 不是）。
        assert_eq!(civil_from_days(days_of(2000, 2, 29)), (2000, 2, 29));
        assert_eq!(civil_from_days(days_of(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(civil_from_days(days_of(2024, 3, 1)), (2024, 3, 1));
        assert_eq!(civil_from_days(days_of(2023, 3, 1)), (2023, 3, 1));
        // 年末 / 年初跨界。
        assert_eq!(civil_from_days(days_of(2025, 12, 31)), (2025, 12, 31));
        assert_eq!(civil_from_days(days_of(2026, 1, 1)), (2026, 1, 1));
        // 中文渲染。
        assert_eq!(cn_date_from_str("2026-10-27"), "二〇二六年十月二十七日");
        assert_eq!(cn_date_from_str("2024-02-29"), "二〇二四年二月二十九日");
    }

    #[test]
    fn ganzhi_year_switches_at_lichun_not_new_year() {
        // 2026 立春之前仍属乙巳年，之后才是丙午年——元旦不换甲子。
        assert_eq!(ganzhi_year(2026, 1, 20), "乙巳");
        assert_eq!(ganzhi_year(2026, 2, 3), "乙巳");
        assert_eq!(ganzhi_year(2026, 2, 4), "丙午");
        assert_eq!(ganzhi_year(2026, 10, 28), "丙午");
        // 六十年一轮回。
        assert_eq!(ganzhi_year(1966, 6, 6), "丙午");
        assert_eq!(ganzhi_year(2086, 6, 6), "丙午");
        assert_eq!(ganzhi_year(1984, 6, 6), "甲子");
    }

    #[test]
    fn season_and_xun_are_stable_and_in_range() {
        // 月季按「农历月 ≈ 公历月 − 1」走：公历 10 月 → 农历九月 → 季秋。
        assert_eq!(season_of_month(10), "季秋");
        assert_eq!(season_of_month(2), "孟春"); // 农历正月
        assert_eq!(season_of_month(1), "季冬"); // 回卷：农历腊月
        assert_eq!(season_of_month(5), "孟夏");
        assert_eq!(season_of_month(8), "孟秋"); // 农历七月
        assert_eq!(season_of_month(9), "仲秋"); // 农历八月
        // 浣只按公历日分三旬。
        assert_eq!(xun_of_day(1), "上浣");
        assert_eq!(xun_of_day(10), "上浣");
        assert_eq!(xun_of_day(11), "中浣");
        assert_eq!(xun_of_day(20), "中浣");
        assert_eq!(xun_of_day(21), "下浣");
        assert_eq!(xun_of_day(31), "下浣");
        // 十二个月都得出一个「孟/仲/季 + 春夏秋冬」的四字之内结果。
        for m in 1..=12u32 {
            let s = season_of_month(m);
            assert_eq!(s.chars().count(), 2, "{m} 月：{s}");
        }
    }

    #[test]
    fn stamp_text_three_styles() {
        // 关 → 空串（＝不画，连一个空格都不留）。
        assert_eq!(stamp_text(StampStyle::Off), "");
        // 公历：定长 `YYYY.MM.DD`，机器可读、不受历法近似影响。
        let plain = stamp_text(StampStyle::Plain);
        assert_eq!(plain.len(), 10, "{plain}");
        assert_eq!(plain.matches('.').count(), 2, "{plain}");
        // 古风：干支年 + 月季 + 浣，≤ 8 字（水印要极简，不能拉成一长条）。
        let cl = stamp_text(StampStyle::Classic);
        assert!(cl.ends_with("浣"), "{cl}");
        assert!(cl.contains("年"), "{cl}");
        assert!(cl.chars().count() <= 8, "太长了：{cl}");
        // 三档循环：关 → 古风 → 公历 → 关。
        assert_eq!(StampStyle::Off.next(), StampStyle::Classic);
        assert_eq!(StampStyle::Classic.next(), StampStyle::Plain);
        assert_eq!(StampStyle::Plain.next(), StampStyle::Off);
        // 默认档是古风（对着一叶古籍，落个公历反而不搭）。
        assert_eq!(StampStyle::default(), StampStyle::Classic);
    }

    /// 公历年月日 → epoch 天（与 `civil_from_days` 互逆，单测专用）。
    fn days_of(y: i64, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m as i64 - 3 } else { m as i64 + 9 }) + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }
}
