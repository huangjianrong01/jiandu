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

use gpui::{div, px, rgb, rgba, Div, FontWeight, ParentElement, SharedString, Styled};
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

/// 朱砂红：左下角「笺讀」印的印泥色（四套模板通用，软件签名）。
pub const CINNABAR: u32 = 0xC3272D;

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

/// 高度滑杆五档（视口高比例）。
pub const H_STEPS: [f32; 5] = [0.50, 0.60, 0.70, 0.80, 0.88];

/// 由档位算卡片高（px）：既按视口比例，又给标题 / 页签 / 控件 / 按钮留位。
pub fn card_h_for(vh: f32, step: usize) -> f32 {
    let frac = match H_STEPS.get(step) {
        Some(f) => *f,
        None => 0.70,
    };
    let cap = (vh - 330.0).max(240.0);
    // 直接按 `vh * frac` 再 clamp 会让上面几档一起撞上 cap、塌成同一个值
    // （900px 视口下 0.70/0.80/0.88 全被截到 570，滑杆等于失效）。
    // 故把档位**线性铺开**在 [220, cap] 区间：低档固定 220，高档顶到 cap，
    // 中间等分——保证每一档都真的比上一档高。
    let lo = H_STEPS.first().copied().unwrap_or(0.5);
    let hi = H_STEPS.last().copied().unwrap_or(0.88);
    let t = if hi > lo {
        ((frac - lo) / (hi - lo)).clamp(0.0, 1.0)
    } else {
        0.5
    };
    220.0 + (cap - 220.0).max(0.0) * t
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

/// 取系统当前日期的中文形式；失败返回空串（调用方按「不显示」处理）。
pub fn cn_date_today() -> String {
    let ymd = match std::process::Command::new("date").arg("+%Y-%m-%d").output() {
        Ok(o) => match String::from_utf8(o.stdout) {
            Ok(s) => s.trim().to_string(),
            Err(_) => return String::new(),
        },
        Err(_) => return String::new(),
    };
    cn_date_from_str(&ymd)
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
    pub show_sign: bool,
    pub show_date: bool,
    pub dir: ShareDir,
    /// 模板下标。
    pub tpl_idx: usize,
    /// 正文字体族名（已做缺失回退）。
    pub family: SharedString,
    /// 阅读版式列数（8/10/12/14，决定内部列数与纸张比例）。
    pub columns: usize,
    pub w: f32,
    pub h: f32,
}

/// 组装一张分享卡（纯 GPUI 元素，零副作用、零 unwrap）。
pub fn build_card(spec: &CardSpec) -> Div {
    let t = tpl(spec.tpl_idx);
    let w = if spec.w.is_finite() { spec.w } else { 360.0 }.max(160.0);
    let h = if spec.h.is_finite() { spec.h } else { 480.0 }.max(200.0);
    let pad = (w * 0.075).clamp(18.0, 46.0);

    // 卡片本体：纯色底 + 裁切（装饰层用 absolute，溢出必须收）。
    let mut card = div()
        .relative()
        .w(px(w))
        .h(px(h))
        .bg(rgb(t.bg))
        .overflow_hidden();
    // 青绿山水：通体贴边一道极细线框（边框在卡片本体上，不占内容层）。
    if matches!(t.deco, Deco::Airy) {
        card = card.border_1().border_color(rgb(t.accent));
    }

    // ---- 装饰层（先画，内容层后画压在上）----
    for layer in deco_layers(t, w, h, pad) {
        card = card.child(layer);
    }

    // ---- 内容层 ----
    let inner_w = (w - pad * 2.0).max(80.0);
    let inner_h = (h - pad * 2.0).max(120.0);
    let content = match spec.dir {
        ShareDir::Vertical => vertical_content(t, spec, inner_w, inner_h),
        ShareDir::Horizontal => horizontal_content(t, spec, inner_w, inner_h),
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

    // ---- 朱砂印：左下角（软件签名，四模板通用）----
    {
        let seal_size = (w * 0.072).clamp(26.0, 38.0);
        let inset = (pad * 0.55).max(8.0);
        card = card.child(
            div()
                .absolute()
                .left(px(inset))
                .bottom(px(inset))
                .child(seal(t, spec.family.clone(), seal_size)),
        );
    }
    card
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

/// 朱砂「笺讀」印：白文方印（朱砂底 + 纸色 Knock-out 二字竖排）。
/// 字色取卡片底色——在任何模板上都是「印泥挖空露纸」的效果。
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
fn vertical_content(t: &ShareTpl, spec: &CardSpec, avail_w: f32, avail_h: f32) -> Div {
    let cols_style = norm_columns(spec.columns);
    let len = spec.body.len();
    let base_fs = ((spec.h / 24.0).clamp(13.0, 26.0)) * t.fs_boost;
    let sign_fs = base_fs * 0.60;
    let date_fs = base_fs * 0.58;

    // 落款列 / 日期列预留宽度（关时留 0，但仍留一点呼吸位）。
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
    // 列间距：有丝栏的模板字距紧（乌丝栏替它呼吸），无丝栏的拉开发散。
    let col_gap = if matches!(t.deco, Deco::Classic | Deco::Rubbing) {
        5.0
    } else {
        16.0
    };
    let side_gap = 14.0;

    // 正文列数：取版式列数与内容量的较小值（内容不足以填满时不硬撑空列）。
    let body_cols = cols_style.min(len.max(1)).max(1);
    let per_col = ((len as f32) / (body_cols as f32)).ceil().max(1.0) as usize;

    // 字号双向收敛：先满足「每列 per_col 字放得下」，再满足「总宽放得下」。
    let mut fs = base_fs;
    loop {
        let pitch = fs * 1.62;
        let ok_h = (per_col as f32) * pitch <= avail_h;
        let col_w = fs + col_gap;
        let total_w = sign_w + date_w + side_gap * 2.0 + (body_cols as f32) * col_w;
        let ok_w = total_w <= avail_w;
        if (ok_h && ok_w) || fs <= 9.0 {
            break;
        }
        fs -= 0.5;
    }
    let fs = fs.min(30.0);

    // 视觉顺序（左→右）：题识列、正文列（从右往左的逆序）、日期列。
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
                .child(vcol(spec.sign, sign_fs, t.meta, spec.family.clone())),
        );
    }

    // 正文列：右起 → flex_row 里倒序 push（第一段落在最右）。
    let chunks: Vec<&[char]> = spec.body.chunks(per_col.max(1)).collect();
    for chunk in chunks.iter().rev() {
        let mut c = div()
            .flex()
            .flex_col()
            .items_center()
            .flex_shrink_0()
            .w(px(fs + col_gap))
            .gap(px(col_gap * 0.55));
        for ch in chunk.iter() {
            c = c.child(char_box(*ch, fs, t.ink, t.bold, spec.family.clone()));
        }
        row = row.child(c);
        // 乌丝栏：列界一道极细淡墨（古籍朱丝栏的墨色版）。
        if matches!(t.deco, Deco::Classic | Deco::Rubbing) {
            let rule_c = if matches!(t.deco, Deco::Rubbing) {
                rgba((t.accent << 8) | 0x30)
            } else {
                rgba((t.ink << 8) | 0x24)
            };
            row = row.child(
                div()
                    .w(px(0.5))
                    .h_full()
                    .bg(rule_c)
                    .flex_shrink_0(),
            );
        }
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
                .child(vcol(spec.date, date_fs, t.meta, spec.family.clone())),
        );
    }
    row
}

/// 横排居中内容：正文行居中，题识 / 日期小字居中垫在下方。
fn horizontal_content(t: &ShareTpl, spec: &CardSpec, avail_w: f32, avail_h: f32) -> Div {
    let base_fs = ((spec.h / 22.0).clamp(14.0, 28.0)) * t.fs_boost;
    // 每行字数：至少 6 字，最多把可用宽度用满（CJK 字宽≈字号）。
    let mut fs = base_fs;
    let mut cpl;
    loop {
        cpl = ((avail_w / (fs * 1.06)).floor() as usize).max(6);
        // 行数 × 行高 + 落款两行，要放得下卡片高。
        let lines = ((spec.body.len() as f32) / (cpl as f32)).ceil().max(1.0);
        let need = lines * (fs * 1.9) + fs * 2.6;
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
    for chunk in spec.body.chunks(cpl) {
        let s: String = chunk.iter().collect();
        let mut row = div()
            .flex()
            .flex_row()
            .justify_center()
            .w_full()
            .line_height(px(line_h))
            .font_family(spec.family.clone())
            .text_size(px(fs))
            .text_color(rgb(t.ink));
        if t.bold {
            row = row.font_weight(FontWeight(750.0));
        }
        col = col.child(row.child(SharedString::from(s)));
    }
    // 落款：题识一行、日期一行，居中垫在正文下方。
    if spec.show_sign && !spec.sign.is_empty() {
        col = col.child(
            div()
                .font_family(spec.family.clone())
                .text_size(px(fs * 0.62))
                .text_color(rgb(t.meta))
                .child(SharedString::from(spec.sign.to_string())),
        );
    }
    if spec.show_date && !spec.date.is_empty() {
        col = col.child(
            div()
                .font_family(spec.family.clone())
                .text_size(px(fs * 0.56))
                .text_color(rgb(t.meta))
                .child(SharedString::from(spec.date.to_string())),
        );
    }
    col
}

/// 单字槽位（竖排）：定宽定高，字身居中。
fn char_box(c: char, fs: f32, ink: u32, bold: bool, family: SharedString) -> Div {
    let mut d = div()
        .w(px(fs))
        .h(px(fs * 1.42))
        .flex()
        .items_center()
        .justify_center()
        .font_family(family)
        .text_size(px(fs))
        .text_color(rgb(ink))
        .child(SharedString::from(c.to_string()));
    if bold {
        d = d.font_weight(FontWeight(750.0));
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
    fn card_h_steps_are_ordered_and_capped() {
        let vh = 900.0;
        let mut prev = 0.0;
        for step in 0..H_STEPS.len() {
            let h = card_h_for(vh, step);
            assert!(h > prev, "滑杆档位必须递增");
            assert!(h <= vh - 330.0 + 0.001, "必须给控件留位");
            prev = h;
        }
        // 越界档位回退中档，不 panic。
        assert!(card_h_for(vh, 99) > 0.0);
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
}
