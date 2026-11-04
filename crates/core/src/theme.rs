//! 纸色主题：宣纸 / 米黄 / 青简 / 夜读。
//!
//! 刻本的美感来自「层次」而不是「对比」：纸、墨、界行、版框、朱印各压一档，
//! 正文才立得住。夜读不用纯黑底 + 纯白字，改用暖调深棕 + 米色墨，
//! 长时间读不刺眼。

/// 纸面主题配置：**纸 / 正文 / 夹注 / 界栏**四色。
///
/// 这是视觉配色的**唯一数据源**：
///   * 设置面板的颜色预览按钮由 [`PAPER_THEMES`] 遍历自动生成；
///   * 渲染层经 [`Theme::paper_theme`] 取这四色上色
///     （背景=纸色，界栏/版框=界栏色，正文=正文色，双行夹注=夹注色）。
///
/// 新增一套配色只在此加一行，UI 与渲染零改动。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaperTheme {
    /// 配置 id（持久化 / 反查用）。
    pub id: &'static str,
    /// 面板上显示的名字。
    pub name: &'static str,
    /// 纸色（全局背景）。
    pub paper: u32,
    /// 正文色（松烟墨）。
    pub text: u32,
    /// 夹注色（双行小字）。
    pub note: u32,
    /// 界栏 / 版框色（乌丝栏）。
    pub border: u32,
    /// **句读朱砂**（v1.84.0）：句读专用的一档朱——比 [`Self::note`]
    /// （朱印 / 夹注）**更鲜**。
    ///
    /// 为什么另开一档：用户拿精刻本照片比过——照片里那枚朱砂句读是
    /// `#B3262F` 那一路的**活朱**，压在冷米白纸上是跳出来的；`note` 那档
    /// 是「沉朱」（`#8C2B22`），用来盖印、写夹注很稳，用来点句读就读成
    /// 一枚暗红的墨点，毫无书卷气。两档各归各的，谁也不迁就谁。
    pub zhu: u32,
}

/// 主题配置表：顺序即面板按钮顺序，下标即 `Theme` 的编号。
///
/// **纸色固定四套**（宣纸 / 米黄 / 青简 / 夜读），与「墨色」四套
/// （墨朱 / 松墨 / 赭朱 / 暗金）一一联动（见 [`Theme::linked_palette`]）。
/// 每套纸的 `text` / `note` 即联动墨色的浅纸取值，选「墨朱」组时直接取这里。
/// 新增配色只在此加一行（同时给 `Theme` 加一个变体）。
pub const PAPER_THEMES: &[PaperTheme] = &[
    PaperTheme {
        id: "xuanzhi",
        name: "宣纸",
        paper: 0xF5F0E6,
        text: 0x2F3535,  // 墨朱·正文
        note: 0xB23A30,  // 墨朱·批注（朱砂红）
        zhu: 0xB2382E,   // 墨朱·句读
        border: 0x2F3535, // 乌丝栏
    },
    PaperTheme {
        id: "mihuang",
        name: "米黄",
        paper: 0xF2E6D0,
        text: 0x3D352F,  // 赭朱·正文
        note: 0xA63F2E,  // 赭朱·批注
        zhu: 0xA8412F,   // 赭朱·句读
        border: 0x4A6B5D, // 绿丝栏
    },
    PaperTheme {
        id: "qingjian",
        name: "青简",
        paper: 0xD8E2DB,
        text: 0x222B2D,  // 松墨·正文
        note: 0x8C3A3A,  // 松墨·批注
        zhu: 0x9A3C36,   // 松墨·句读
        border: 0x222B2D,
    },
    PaperTheme {
        id: "yedou",
        name: "夜读",
        paper: 0x1A1C1D,
        text: 0xDCD6CD, // 暗金·正文（月白）
        note: 0xA68E5C, // 暗金·批注（秋香）
        zhu: 0xC0A26A,  // 暗金·句读（秋香，深底上不刺眼）
        border: 0x4A4D4F, // 灰丝栏
    },
    // ── v1.83.0 · 仿古籍精刻本 ──────────────────────────────────────
    // 用户点名的那一档：**纸是 #F7F6F1**（冷一点的米白，不是宣纸黄），
    // 墨是刻本的**焦墨**（近黑，比松烟墨更硬），界栏比正文再深半档——
    // 照片里「外框最重、内框与界行退一档」的层次就靠这三个值拉开。
    //
    // 这一档不只是「换纸色」：`is_keben()` 一并把版框/界行的**笔路**
    // 切换成精刻本的规矩（见 [`Theme::rule_line`]），出图那一层也改走
    // `ShareFrame::Double`（外粗内细的双边）。
    PaperTheme {
        id: "keben",
        name: "仿刻本",
        paper: 0xF7F6F1,
        text: 0x1E1B18,   // 焦墨（刻本的字比宋体屏显字更黑一档）
        note: 0x8C2B22,   // 朱（比墨朱稍沉，压在冷米白上不刺）
        zhu: 0xB0342C,   // 句读朱砂：照精刻本照片取（照片实测 #B3262F）
        border: 0x2E2823, // 版框：比正文更深，外框才立得住
    },
    // ── v1.9.82 · 无障碍高对比（视弱用户）────────────────────────────
    // 这**不是**第三套国色，是另一套目标：前面五套追求「层次」（纸、墨、
    // 界行、朱印各压一档，正文才立得住），这里只有一条硬指标——WCAG 2.1
    // 的 **7:1**（AAA 正文级）。所以纸是纯白 / 纯黑，字是纯黑 / 纯白，
    // 中间**不留**任何「淡淡的、看得见就行」的灰。层次让位给可读。
    //
    // 注意 `note`（夹注）也一并拉到 7:1 以上：夹注是双行小字，本来就比正文
    // 难认，在无障碍档里再让它「淡一档」就等于把它删了。
    PaperTheme {
        id: "a11y_day",
        name: "高对比日",
        paper: 0xFFFFFF, // 纯白
        text: 0x000000,  // 纯黑（21:1）
        note: 0x0B2A5B,  // 深靛（13.9:1）——夹注仍要与正文分得开
        zhu: 0x9E0000,   // 朱（8.6:1）：句读是图形，但太暗就成墨点了
        border: 0x000000,
    },
    PaperTheme {
        id: "a11y_night",
        name: "高对比夜",
        paper: 0x000000, // 纯黑
        text: 0xFFFFFF,  // 纯白（21:1）
        note: 0xFFC24D,  // 琥珀（13.1:1）
        zhu: 0xFF7A66,   // 亮朱（8.2:1）：0xB0342C 这种沉朱在黑底上只有 3.4:1
        border: 0x8A8A8A, // 界行 6.1:1——1px 细线在黑底上不能更淡
    },
    // ── v1.9.83 · 护眼（黑底黄字）────────────────────────────────
    // 与「高对比夜」看着像、目的不同：那一档是**视弱**要极致对比（纯白
    // 21:1），这一档是**久读**要少蓝光——649nm 以上的暖黄几乎不触发
    // 黑视素，夜里读两小时眼睛不涩；纯黑底配 OLED 还能真省电。
    //
    // 黄不是越亮越好：#FFFF00 对黑 19.6:1，但那是荧光笔的黄，盯久了
    // 发花。取 #FFD24A（14.6:1）——仍远超 AAA，又保留一点暖。
    PaperTheme {
        id: "huyan",
        name: "护眼",
        paper: 0x000000, // 纯黑
        text: 0xFFD24A,  // 暖黄（14.6:1）
        note: 0xC9A24A,  // 暗金（8.9:1）：夹注仍要退一档，否则小字抢正文
        zhu: 0xFF9A70,   // 亮朱（深底再兑 0.20 后约 4.9:1）
        border: 0x8A7A4A, // 暗金界行（兑下去 4.2:1）
    },
];

/// **无障碍高对比的「朗读高亮」三色**（v1.9.82）。
///
/// 为什么另开一张表：纸 / 墨 / 朱是「书长什么样」，高亮是「读到哪儿」——
/// 后者在无障碍档里的任务是**在纯黑纯白之间再立起两块绝对可辨的色块**，
/// 而不是在纸上添一层淡晕。两者服务于不同的眼睛，混在一张表里必然互相迁就。
///
/// 三块硬指标（都能用 [`contrast_ratio`] 断言，见 `tests`）：
///   * 句：深蓝底 + 白字 ≥ 7:1；
///   * 词：纯黄底 + 黑字 ≥ 7:1；
///   * 词 vs 句 ≥ 7:1 —— 这样「当前这个词」与「这一句的其余字」一眼分得开，
///     不靠深浅差一档去猜（旧版的 0.34 / 0.12 是给常视力看的，对弱视不够）。
pub struct A11yTheme {
    /// 与 [`PaperTheme::id`] 同一个 id：两张表按 id 配对，不按下标。
    pub id: &'static str,
    /// 当前句的底。
    pub line_bg: u32,
    /// 当前句的字。
    pub line_ink: u32,
    /// 当前句的「下划线」。竖排里它落在字格**左内侧**（见
    /// `reader` 里的说明：字格底边是下一个字的顶边，画在底边会变成梯子）。
    pub line_rule: u32,
    /// 当前词的底。
    pub word_bg: u32,
    /// 当前词的字。
    pub word_ink: u32,
}

/// 两套无障碍高亮（日 / 夜）。顺序与 [`A11Y_THEMES`] 无关紧要——
/// 查找一律走 [`a11y_theme`]，绝不按下标取。
pub const A11Y_THEMES: &[A11yTheme] = &[
    A11yTheme {
        id: "a11y_day",
        line_bg: 0x0A2A5E,   // 深靛蓝：白字 13.8:1
        line_ink: 0xFFFFFF,
        line_rule: 0xFFFFFF,
        word_bg: 0xFFFF00,   // 纯黄：黑字 19.6:1
        word_ink: 0x000000,
    },
    A11yTheme {
        id: "a11y_night",
        // 夜里蓝块要比白天**亮一档**才看得出「这里有一块」：黑纸上
        // 0x0A2A5E 的亮度只有 0.026，块与纸几乎不分。提亮到 0.085 之后
        // 白字仍有 7.8:1。
        line_bg: 0x1B50A0,
        line_ink: 0xFFFFFF,
        line_rule: 0xFFFFFF,
        word_bg: 0xFFFF00,
        word_ink: 0x000000,
    },
];

/// 按 id 取无障碍高亮配色；非无障碍主题给 `None`。
pub fn a11y_theme(id: &str) -> Option<&'static A11yTheme> {
    A11Y_THEMES.iter().find(|a| a.id == id)
}

impl PaperTheme {
    /// 按 id 取配置（未知 id 回落首项）。
    pub fn by_id(id: &str) -> &'static PaperTheme {
        PAPER_THEMES.iter().find(|p| p.id == id).unwrap_or(&PAPER_THEMES[0])
    }

    /// 是否深底（由纸色亮度推导，不写死枚举）。
    pub fn is_dark(&self) -> bool {
        luma(self.paper) < DARK_LUMA
    }

    /// **纸色是不是「纯黑 / 纯白」**——没有「再亮 / 再暗一档」的余地。
    ///
    /// 这不是「是不是无障碍档」的同义词：护眼档纸也是纯黑。要问的是
    /// **物理事实**（还推不推得动），而不是标签——按标签问，就会在下一个
    /// 纯色主题上重演「顶栏与纸糊成一片」。
    ///
    /// 用到它的地方：界面底 / 界面分隔线 / 界面字（三处都要「偏一档」，
    /// 但纯色没有可偏的余量）、纸背（深底本该更暗，纯黑只能反向提亮）。
    pub fn is_pure(&self) -> bool {
        matches!(self.paper, 0x000000 | 0xFFFFFF)
    }
}

/// 纸色亮度低于此值即视为「夜读」深底（0..255）。
pub const DARK_LUMA: f32 = 110.0;

/// 感知亮度（0..255），用于深底判定与「谁更淡」的比较。
pub fn luma(c: u32) -> f32 {
    let r = ((c >> 16) & 0xFF) as f32;
    let g = ((c >> 8) & 0xFF) as f32;
    let b = (c & 0xFF) as f32;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// 彩度（RGB 三通道极差，0..255）：用来把「低饱和国色」写成可回归的约束。///
/// 竞品爱用高饱和的蓝 / 紫 / 绿做阅读底，一眼就跳；这套配色全部压在极差
/// 之下——纸色极差不超过 60，正文与界栏不超过 80。夹注朱印是另一回事，
/// 它可以艳，但不许染成荧光（上限 170）。
pub fn sat(c: u32) -> f32 {
    let r = ((c >> 16) & 0xFF) as f32;
    let g = ((c >> 8) & 0xFF) as f32;
    let b = (c & 0xFF) as f32;
    r.max(g).max(b) - r.min(g).min(b)
}

/// **WCAG 2.1 相对亮度**（0.0..1.0，纯函数）。
///
/// 与 [`luma`] **不是一回事**：`luma` 是「感知亮度」（直接在线性 sRGB 数值上
/// 加权，0..255，用来判深浅、比淡浓，够快够糙）；这里要先做**伽马还原**
/// （sRGB 传输函数的分段逆），再做 Y 加权。两者在中间调能差出一倍。
///
/// 对比度是法务级别的数字，必须走这一条——用 `luma` 算出来的比值不可引用。
pub fn rel_luminance(c: u32) -> f32 {
    fn ch(v: u32) -> f32 {
        let s = (v & 0xFF) as f32 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * ch(c >> 16) + 0.7152 * ch(c >> 8) + 0.0722 * ch(c)
}

/// **WCAG 2.1 对比度**（1.0..21.0，纯函数，与参数顺序无关）。
///
/// 阈值口径（WCAG 2.1）：正文 4.5:1（AA）/ 7:1（AAA）；
/// 大字号 3:1（AA）/ 4.5:1（AAA）；图形与界面构件 3:1（1.4.11）。
/// 无障碍档一律按 **7:1** 立规——视弱用户看的是「字号够大但不够黑」的正文。
pub fn contrast_ratio(a: u32, b: u32) -> f32 {
    let (la, lb) = (rel_luminance(a), rel_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[derive(Default)]
pub enum Theme {
    /// 宣纸：米白偏暖，最像常见的影印本（联动「墨朱」）。
    #[default]
    Xuan,
    /// 米黄：更黄一些，旧书味重（联动「赭朱」）。
    Mi,
    /// 青简：青灰纸配竹青界行（联动「松墨」）。
    Qing,
    /// 夜读：暖调深底（联动「暗金」）。
    Ye,
    /// **仿古籍精刻本**（v1.83.0）：冷米白纸 #F7F6F1 + 焦墨 + 双边版框。
    Ke,
    /// **无障碍高对比 · 日间**（v1.9.82）：纯白纸 + 纯黑字（21:1）。
    A11yDay,
    /// **无障碍高对比 · 夜间**（v1.9.82）：纯黑纸 + 纯白字（21:1）。
    A11yNight,
    /// **护眼**（v1.9.83）：纯黑纸 + 暖黄字（14.6:1）——久读不刺眼。
    ///
    /// 与 [`Theme::A11yNight`] 的区别是**目的**而不是深浅：那一档为视弱
    /// 要极致对比，这一档为久读要少蓝光。菜单栏「高对比护眼模式」进的是它。
    Huyan,
}


impl Theme {
    /// 顺序必须与 [`PAPER_THEMES`] 完全一致——下标即主题编号。
    pub const ALL: [Theme; 8] = [
        Theme::Xuan,
        Theme::Mi,
        Theme::Qing,
        Theme::Ye,
        Theme::Ke,
        Theme::A11yDay,
        Theme::A11yNight,
        Theme::Huyan,
    ];

    /// 面板上显示的名字（直接取配置表，免得两套名字不同步）。
    pub fn label(self) -> &'static str {
        self.paper_theme().name
    }

    /// 本主题在 [`PAPER_THEMES`] 里的下标。
    pub fn index(self) -> usize {
        match self {
            Theme::Xuan => 0,
            Theme::Mi => 1,
            Theme::Qing => 2,
            Theme::Ye => 3,
            Theme::Ke => 4,
            Theme::A11yDay => 5,
            Theme::A11yNight => 6,
            Theme::Huyan => 7,
        }
    }

    /// **是否护眼档**（v1.9.83）：黑底黄字，菜单栏一键进出。
    ///
    /// 只此一处判定。与 [`Self::is_a11y`] 并列而不是合并：两者的界面
    /// 规矩不同（护眼不换上无障碍高亮实色块，仍用朱砂淡染——黄字配深蓝
    /// 实色块会打架）。
    pub fn is_huyan(self) -> bool {
        matches!(self, Theme::Huyan)
    }

    /// 「一键进 / 出护眼档」：在档里就回 `back`，否则进护眼。
    ///
    /// 与 [`Self::toggle_a11y`] 同一套「原样还原」纪律：退出时回用户
    /// 进档前那套纸色，不擅自猜一套近似的。
    pub fn toggle_huyan(self, back: Option<Theme>) -> Theme {
        if self.is_huyan() {
            return back.unwrap_or(Theme::Xuan);
        }
        Theme::Huyan
    }

    /// **是否无障碍高对比档**（v1.9.82）。
    ///
    /// 只此一处判定。要问的场合：朗读高亮换不换色、字号下不下限、动画要不要
    /// 收着来。**绝不在别处写 `matches!(self, …)`**——那样迟早分家。
    pub fn is_a11y(self) -> bool {
        matches!(self, Theme::A11yDay | Theme::A11yNight)
    }

    /// 无障碍高亮配色；常视力主题给 `None`。
    ///
    /// 按 **id** 配对而不是下标：两张表（纸墨 / 高亮）本来就不等长，
    /// 下标一错就是「夜读套白天的高亮」，而那种错不会崩、只会难看，
    /// 最难发现。
    pub fn a11y(self) -> Option<&'static A11yTheme> {
        a11y_theme(self.paper_theme().id)
    }

    /// 无障碍档的**默认主题**（进/出这个模式时用）：日 / 夜由当前是不是深底决定。
    pub fn a11y_default(dark: bool) -> Theme {
        if dark {
            Theme::A11yNight
        } else {
            Theme::A11yDay
        }
    }

    /// 「一键切换高对比」：已在无障碍档就退回来处（`back`），否则按当前的
    /// 深浅进对应的无障碍档。
    ///
    /// `back` 是进入前的那套主题；退出去时原样还原——**不要**自己猜一套
    /// 「近似的普通主题」，用户精心调的纸色不该被一次快捷键抹掉。
    pub fn toggle_a11y(self, back: Option<Theme>) -> Theme {
        if self.is_a11y() {
            return back.unwrap_or(Theme::Xuan);
        }
        Theme::a11y_default(self.is_dark())
    }

    /// **是否仿刻本模式**（v1.83.0）。
    ///
    /// 只此一处判定：版框笔路、界行淡度、出图底与框、默认字体、句读式样
    /// 全都问这一个问题，绝不各区各写一份 `matches!(…)`——那样迟早分家，
    /// 屏幕上是一套、导出的是另一套。
    pub fn is_keben(self) -> bool {
        matches!(self, Theme::Ke)
    }

    /// 「纸色 → 墨色」智能联动的默认配对（任务四：一键换肤）。
    ///
    /// 宣纸→墨朱、米黄→赭朱、青简→松墨、夜读→暗金；仿刻本→墨朱（焦墨那套）；
    /// 用户随后仍可手动把墨色切到任意一套（手动覆盖默认联动）。
    pub fn linked_palette(self) -> InkPalette {
        match self {
            // 无障碍档与护眼档恒连锁「墨朱」= 直接取纸主题的 `text`，
            // 即纯黑 / 纯白 / 暖黄。若这里放行别的墨色组（赭朱、暗金），
            // 正文就不再是那个承诺的色，对比度与「黑底黄字」当场作废。
            Theme::Xuan | Theme::Ke | Theme::A11yDay | Theme::A11yNight | Theme::Huyan => {
                InkPalette::Ink
            }
            Theme::Mi => InkPalette::Ochre,
            Theme::Qing => InkPalette::Forest,
            Theme::Ye => InkPalette::Gold,
        }
    }

    /// 取本主题的四色配置（纸 / 正文 / 夹注 / 界栏）。
    pub fn paper_theme(self) -> &'static PaperTheme {
        &PAPER_THEMES[self.index()]
    }

    /// 是否深底：由纸色亮度推导（新增深底主题无需改这里）。
    pub fn is_dark(self) -> bool {
        self.paper_theme().is_dark()
    }

    /// 纸面（正面）：纸色。
    pub fn paper(self) -> u32 {
        self.paper_theme().paper
    }

    /// 纸面（翻过来的一面）：纸色向「墨 / 纯黑」压深一档。
    pub fn paper_back(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            // 深底：往纯黑再压一档。**但纯黑已经压不动了**（无障碍夜 / 护眼），
            // 那时反过来向正文提一档——目标始终是「两叶分得开」，不是
            // 「一定更黑」。
            if p.is_pure() {
                mix(p.paper, p.text, 0.05)
            } else {
                mix(p.paper, 0x000000, 0.07)
            }
        } else {
            mix(p.paper, p.text, 0.07)
        }
    }

    /// 正文墨色（松烟墨）。
    pub fn ink(self) -> u32 {
        self.paper_theme().text
    }

    /// 夹注色（双行小字）：与正文严格区分的一色。
    pub fn note(self) -> u32 {
        self.paper_theme().note
    }

    /// 淡墨：小字注文、行款、背面——正文色向纸色兑淡。
    pub fn ink_soft(self) -> u32 {
        let p = self.paper_theme();
        mix(p.text, p.paper, 0.30)
    }

    /// 界行（列与列之间的细线）：界栏色向纸色兑淡，比版框再轻一档。
    pub fn rule(self) -> u32 {
        let p = self.paper_theme();
        mix(p.border, p.paper, 0.25)
    }

    /// 朱丝栏：写经体的朱红界行（古本里的另一路做法）。
    /// 竖排正文目前走「乌丝栏」（`rule_ink`，黑口本一路），此色留给
    /// 将来的朱丝版式 / 朱印装饰——取夹注的朱色，与全书的朱印同源。
    pub fn rule_red(self) -> u32 {
        self.note()
    }

    /// 版框（四周双边）：界栏色向纸色兑淡一档（比界行轻，框不抢字）。
    pub fn frame(self) -> u32 {
        let p = self.paper_theme();
        mix(p.border, p.paper, 0.35)
    }

    /// 古籍版框：四周双边的外框——**界栏色**（配置表的 `borderColor`）。
    pub fn frame_ink(self) -> u32 {
        self.paper_theme().border
    }

    /// **版框外层粗线用色**（v1.85.0）：深底提亮一档，浅纸原样。
    ///
    /// 夜读里 `border`（0x4A4D4F）压在深纸上只比纸亮十来级，3px 粗框与
    /// 1px 内框几乎同亮度，远看是一圈「糊在一起的灰」——层次全无。故外框
    /// 向**正文色**提 [`DARK_FRAME_LIFT`]，内框（`rule_line`）反而向纸压
    /// [`DARK_RULE_FADE`]，一进一出，粗线压得住、细线退得开。
    ///
    /// 浅纸四套主题**逐值等于** [`Theme::frame_ink`]（即 `border`），老版面
    /// 一个像素都不动；`frame_ink` 本身仍严格等于 `border`，鱼尾、象鼻、
    /// 牌记那些「浓墨一笔」照旧取它。
    pub fn frame_outer(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.border, self.ink(), DARK_FRAME_LIFT)
        } else {
            p.border
        }
    }

    /// 乌丝栏：竖排列间界行——**界栏色**（配置表的 `borderColor`）。
    pub fn rule_ink(self) -> u32 {
        self.paper_theme().border
    }

    /// **黑鱼尾**的墨色：深灰 `#333333`，不是纯黑——纯黑在纸面上太跳，
    /// 会盖过正文的墨。深底（夜读）反过来取「淡墨」：黑纸上的黑鱼尾
    /// 与纸只剩十来个单位的亮度差，看上去像一枚脏印；改用纸→墨的方向
    /// 兑到六成，才是一枚看得清、又不抢正文的灰鱼尾。
    pub fn fish_ink(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.paper, self.ink(), 0.62)
        } else {
            crate::banxin::FISH_INK
        }
    }

    /// 版心折痕：极淡的灰 `#999999`——纸对折的物理褶，不是界栏，
    /// 故比界行再淡一档（深底上略压暗，免得亮成一道白线）。
    pub fn fold_ink(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            // 深底：折痕只在纸上比底色亮一点点（界行 0x4A4D4F 已够淡，
            // 折痕必须更淡，否则中缝会亮成一道灯管）。
            mix(p.paper, crate::banxin::FOLD_INK, 0.30)
        } else {
            crate::banxin::FOLD_INK
        }
    }

    /// 丝栏取色：按版字面板所选丝栏色给出界行颜色。
    /// 乌 = `rule_ink`（界栏色）；朱 = `rule_red`（夹注朱色）；
    /// 金 = 泥金——亮纸上压一档防飘，深底里提亮才看得见（不写死枚举，
    /// 新增国色主题零改动）。
    pub fn rule_by(self, rc: crate::types::RuleColor) -> u32 {
        match rc {
            crate::types::RuleColor::Wu => self.rule_ink(),
            crate::types::RuleColor::Zhu => self.rule_red(),
            crate::types::RuleColor::Jin => {
                if self.is_dark() {
                    0xC2A25C
                } else {
                    0xB08A3E
                }
            }
        }
    }

    /// **界行 / 内框用色**（v1.83.0）：仿刻本与深底主题各退一档。
    ///
    /// 精刻本的层次是「**外框最重 → 内框与界行退一档 → 字最重**」：
    /// 照片里外框一道浓黑，内框和界行都淡成灰。旧制内外同色同粗，
    /// 远看是「一圈粗细不匀的框」；这里只把**内**的那一笔退下去，
    /// 外框的墨一分不动——所以浅纸主题逐值等于 [`Theme::rule_by`]，
    /// 老版面一个像素都不变。
    ///
    /// v1.85.0 起深底（夜读）也退一档（[`DARK_RULE_FADE`]）：夜里 1px 细线
    /// 压在深纸上本就发糊，外框按 [`Theme::frame_outer`] 提亮后若不把内框
    /// 压下去，双线就并成一条亮带。**内框与界行共用这一笔**，故一起退，
    /// 框角与界行才不会在深底上断成两截。
    ///
    /// 退的幅度是刻意的：再淡就散成一层雾，界行立不住。
    pub fn rule_line(self, rc: crate::types::RuleColor) -> u32 {
        let c = self.rule_by(rc);
        let p = self.paper();
        if self.is_keben() {
            mix(c, p, KB_RULE_FADE)
        } else if self.is_dark() {
            mix(c, p, DARK_RULE_FADE)
        } else {
            c
        }
    }

    /// 朱色：印章、句读、当前条目。
    ///
    /// 取夹注的朱色（与全书朱印同源），深底再向纸色提亮一档——
    /// 否则夜读里朱印会沉进深纸看不见。浅纸直接用，深底自动提亮。
    pub fn accent(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.note, p.paper, 0.20)
        } else {
            p.note
        }
    }

    /// **句读朱砂**（v1.84.0）：句读专走这一档，见 [`PaperTheme::zhu`]。
    ///
    /// 深底上同样向纸色兑淡一档（与 [`Self::accent`] 同口径），免得朱砂压在
    /// 夜读底上发闷；浅纸上就是表里那一只活朱，不作任何兑淡——**句读要跳
    /// 出来才对**，那是朱笔，不是正文。
    pub fn zhu(self) -> u32 {
        let p = self.paper_theme();
        if p.is_dark() {
            mix(p.zhu, p.paper, 0.20)
        } else {
            p.zhu
        }
    }

    /// 靛色：通假标注。
    pub fn indigo(self) -> u32 {
        if self.is_dark() {
            0x7E9BC8
        } else {
            0x3E5A8A
        }
    }

    /// 界面底（工具栏、状态栏、浮层）——纸色向黑（深底）或向白（浅底）压，
    /// 永远与纸面同调，不另起一套色；深底自动更暗、浅底自动更亮。
    pub fn chrome(self) -> u32 {
        let p = self.paper_theme();
        if p.is_pure() {
            // **纯白 / 纯黑没有「再亮一档」的余地**：往正文方向偏一档。
            // 深浅两种情况都成立，画面永远是「纸 + 一条比纸偏一档的横带」，
            // 绝不出现「顶栏与纸同色、浑然一片」（护眼档黑纸黑栏就是这么来的）。
            return mix(p.paper, p.text, 0.07);
        }
        if p.is_dark() {
            mix(p.paper, 0x000000, 0.16)
        } else {
            mix(p.paper, 0xFFFFFF, 0.10)
        }
    }

    /// 界面分隔线——向对比色压一档（深底提亮、浅底压暗）。
    pub fn chrome_line(self) -> u32 {
        let p = self.paper_theme();
        if p.is_pure() {
            // 纯色纸的界面分隔线不许是「一档淡灰」：0.55 兑下去 ≈ #737373，
            // 对白纸 4.7:1、对黑纸约 4.0:1，一条线就立得住。
            return mix(p.paper, p.text, 0.55);
        }
        if p.is_dark() {
            mix(p.paper, 0xFFFFFF, 0.16)
        } else {
            mix(p.paper, 0x000000, 0.10)
        }
    }

    /// 界面文字——深底取浅、浅底取深，保证与 `chrome()` 永远反色，
    /// 绝不出现白底白字 / 黑底黑字。
    pub fn chrome_text(self) -> u32 {
        let p = self.paper_theme();
        if p.is_pure() {
            // 界面字 = 正文色（纯黑 / 纯白 / 暖黄）。`chrome()` 只偏了 7%，
            // 所以 13:1 上下——顶栏的字与书页的字同色，眼睛不用切换。
            return p.text;
        }
        if p.is_dark() {
            mix(p.paper, 0xFFFFFF, 0.30)
        } else {
            mix(p.text, p.paper, 0.10)
        }
    }

    /// 控件底（未选中的小按钮）——纸色向白压一档（深浅都压白，控件浮于纸面）。
    pub fn chip(self) -> u32 {
        let p = self.paper_theme();
        mix(p.paper, 0xFFFFFF, if p.is_dark() { 0.12 } else { 0.06 })
    }

    /// 控件底（选中）——朱色向纸色兑淡，深浅底都压到「可辨的淡朱」。
    pub fn chip_on(self) -> u32 {
        let p = self.paper_theme();
        mix(p.note, p.paper, if p.is_dark() { 0.45 } else { 0.62 })
    }
}

/// 双行夹注目色向纸色的兑淡比例：0.38 ≈ 墨色兑三成纸色。
/// 再淡小字（半格）在纸上发虚；再浓又失去「夹注比正文淡」的层次。
pub const Z_NOTE_FADE: f32 = 0.38;

/// **仿刻本的界行淡度**（v1.83.0）：版框色向纸色掺 45%。
///
/// 照片上量的：外框是浓墨，内框与界行淡成一道灰。45% 是「看得见是线、
/// 又绝不与正文抢」的那一档——再淡（>0.55）界行散成一层雾，行列就分不清了。
pub const KB_RULE_FADE: f32 = 0.45;

/// **深底（夜读）版框的提亮 / 内框的压暗**（v1.85.0）。
///
/// 夜读纸 0x1E1F21、版框色 0x4A4D4F，两者只差十来级亮度；外框 3px 与内框
/// 1px 同色时，远看是「一圈灰带」，看不出文武边栏的双线。故外框向正文色
/// 提 0.20（≈0x6D6E6E 一级），内框与界行向纸压 0.22（≈0x3D3F41）——
/// 一进一出拉开约 0x30 的亮度差，双线在暗金底上才分得开。
/// 两值都是「刚好看得出层次、又不至于跳出来抢字」的那一档。
pub const DARK_FRAME_LIFT: f32 = 0.20;
pub const DARK_RULE_FADE: f32 = 0.22;

/// 把颜色 `a` 向颜色 `b` 混 `k`（0..=1，k=1 全为 b），返回 0xRRGGBB。
/// 用于「墨色 + 纸色 → 夹注目色」的明暗底自动推导。
pub fn mix(a: u32, b: u32, k: f32) -> u32 {
    let ch = |s: u32| {
        let ca = ((a >> s) & 0xFF) as f32;
        let cb = ((b >> s) & 0xFF) as f32;
        ((ca * (1.0 - k) + cb * k).round() as u32) & 0xFF
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// 正文 / 小字注释的配色方案（墨色）。固定四套，与纸色一一联动。
///
/// 一组两色：**前者为正文（`ink`）、后者为小字注释（`note`）**。
/// 渲染时正文用 `ink`、夹注 / 句读 / 眉批用 `note`——两者严格分色，
/// 绝不能正文注文一个色（任务三·生死攸关）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[derive(Default)]
pub enum InkPalette {
    /// 墨黑正文 · 朱红批注（默认，最接近常见刻本；浅纸直取纸主题 text/note）
    #[default]
    Ink,
    /// 墨绿正文 · 砖红批注（联动青简）
    Forest,
    /// 赭褐正文 · 朱红批注（联动米黄）
    Ochre,
    /// 暗金正文 · 米白/金批注（联动夜读；浅纸走暗金，深底走月白+秋香）
    Gold,
}


impl InkPalette {
    pub const ALL: [InkPalette; 4] = [
        InkPalette::Ink,
        InkPalette::Forest,
        InkPalette::Ochre,
        InkPalette::Gold,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InkPalette::Ink => "墨朱",
            InkPalette::Forest => "松墨",
            InkPalette::Ochre => "赭朱",
            InkPalette::Gold => "暗金",
        }
    }

    /// 正文色。夜读（深底）时提亮，避免黑底黑字。
    ///
    /// **墨朱组（默认）直接取纸主题的正文色**（`PAPER_THEMES` 的 `text`）——
    /// 「纸—墨」是一套设计好的搭配；其余墨色组是读者自选的组内配色。
    pub fn ink(self, t: Theme) -> u32 {
        // 无障碍档**不许被墨色组改写**：这里放行就等于允许把纯黑换成赭褐，
        // 7:1 当场作废。读者在无障碍档里也改墨色（UI 拦了一道）也不会破功。
        if t.is_a11y() {
            return t.ink();
        }
        if matches!(self, InkPalette::Ink) {
            return t.ink();
        }
        if t.is_dark() {
            return match self {
                InkPalette::Ink => 0xDCD6CD,
                InkPalette::Forest => 0xC2D0BC,
                InkPalette::Ochre => 0xD9C2A2,
                InkPalette::Gold => 0xDCD6CD, // 月白（深底正文）
            };
        }
        match self {
            InkPalette::Ink => 0x2F3535,
            InkPalette::Forest => 0x222B2D,
            InkPalette::Ochre => 0x3D352F,
            InkPalette::Gold => 0x7A5C1E, // 暗金（浅纸正文）
        }
    }

    /// 小字注文（繁体、夹注、句读、眉批）的颜色：默认组取纸主题的夹注色
    /// （`PAPER_THEMES` 的 `note`），与正文严格分色。
    pub fn note(self, t: Theme) -> u32 {
        // 同 `ink`：无障碍档的夹注色是**算过 7:1 的**，不许被墨色组顶掉。
        if t.is_a11y() {
            return t.note();
        }
        if matches!(self, InkPalette::Ink) {
            return t.note();
        }
        if t.is_dark() {
            return match self {
                InkPalette::Ink => 0xA68E5C,
                InkPalette::Forest => 0xD08163,
                InkPalette::Ochre => 0xD08163,
                InkPalette::Gold => 0xA68E5C, // 秋香（深底批注）
            };
        }
        match self {
            InkPalette::Ink => 0xB23A30,
            InkPalette::Forest => 0x8C3A3A,
            InkPalette::Ochre => 0xA63F2E,
            InkPalette::Gold => 0xAD5A33, // 淡金（浅纸批注）
        }
    }

    /// 双行夹注的小字色：**用 `note` 色向纸色兑淡一档**（见 `mix`）。
    ///
    /// 刻本里的夹注小字与正文同墨，因字小、墨薄，看上去就是「淡一点」；
    /// 但这里刻意让夹注**保持 `note` 的色相**（朱红 / 砖红 / 秋香），
    /// 只是向纸色沉一档做「薄墨」感——正文纯正文色、夹注纯注文色，
    /// 两套笔永远不混（任务三）。
    pub fn z_note(self, t: Theme, back: bool) -> u32 {
        // 墨朱组（默认）：双行夹注直接用纸主题的夹注色——进入夹注分支就
        // 换这支笔（正文 `text` / 夹注 `note` 严格分色）。
        if matches!(self, InkPalette::Ink) {
            return t.note();
        }
        let paper = if back { t.paper_back() } else { t.paper() };
        mix(self.note(t), paper, Z_NOTE_FADE)
    }

    /// 通假（借字）标注色。
    pub fn gloss(self, t: Theme) -> u32 {
        if t.is_dark() {
            0x8FA8D0
        } else {
            0x3E5A8A
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 相对亮度（感知近似，用于「谁更淡」的比较）。
    fn lum(c: u32) -> f32 {
        let r = ((c >> 16) & 0xFF) as f32;
        let g = ((c >> 8) & 0xFF) as f32;
        let b = (c & 0xFF) as f32;
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// **WCAG 对比度的算式本身**（v1.9.82）。
    ///
    /// 先把这条钉死，再用它去约束配色——否则「7:1」只是句口号：
    ///   * 纯黑对纯白 = 21:1（公式的上界，差一点就说明伽马还原写错了）；
    ///   * 同色 = 1:1（下界）；
    ///   * 黑白颠倒 = 同一个值（对比度与顺序无关）。
    ///
    /// ⚠️ 这条**必须**用 `rel_luminance`（含 sRGB 伽马还原），不能用上面
    /// 那个 `lum`：线性近似会把中灰的对比度算高一倍，于是「算着 7:1、
    /// 看着 4:1」——那是最坏的一种通过。
    #[test]
    fn z_contrast_ratio_matches_the_wcag_reference_values() {
        // 上界：纯黑 / 纯白 = 21:1。
        assert!((contrast_ratio(0x000000, 0xFFFFFF) - 21.0).abs() < 0.05);
        // 下界：同色 = 1:1。
        assert!((contrast_ratio(0x336699, 0x336699) - 1.0).abs() < 1e-6);
        // 与参数顺序无关。
        assert_eq!(
            contrast_ratio(0x0A2A5E, 0xFFFFFF),
            contrast_ratio(0xFFFFFF, 0x0A2A5E)
        );
        // 参考值：`#767676` 对白 = 4.54:1（WCAG 文档里最常引的那个「刚好 AA」）。
        let r = contrast_ratio(0x767676, 0xFFFFFF);
        assert!((4.50..4.60).contains(&r), "#767676 对白应为 4.54，实得 {r:.3}");
        // 伽马还原确实生效：线性近似会把 `#808080` 算成 ~5.3:1，
        // 正确的值是 ~3.95:1（低于 AA，这正是「中灰不够黑」的意思）。
        let gray = contrast_ratio(0x808080, 0xFFFFFF);
        assert!((3.8..4.1).contains(&gray), "#808080 对白应为 3.95，实得 {gray:.3}");
        // rel_luminance 单调：越亮值越大。
        assert!(rel_luminance(0x333333) < rel_luminance(0x777777));
        assert!((rel_luminance(0xFFFFFF) - 1.0).abs() < 1e-6);
        assert!(rel_luminance(0x000000).abs() < 1e-6);
    }

    /// **无障碍档全表过 7:1**（v1.9.82）：这是这一档存在的**唯一**理由。
    ///
    /// 四组关系逐一套算：
    ///   * 正文 / 纸、夹注 / 纸 —— 书页本身；
    ///   * 句底字 / 句底、词底字 / 词底 —— 朗读高亮；
    ///   * 词底 / 句底 —— 「当前这个词」必须从「这一句的其余字」里跳出来，
    ///     否则弱视用户只能看到一个整块的蓝，看不见光标走到哪。
    ///
    /// 任何一条掉到 7 以下，这一档就不该发布——所以这里用的是 `assert!`
    /// 而不是 `warn!`。
    #[test]
    fn z_a11y_themes_clear_wcag_aaa_seven_to_one() {
        // 两张表必须能配上对（按 id），少一行就是「选了无障碍纸色却没有高亮」。
        assert_eq!(A11Y_THEMES.len(), 2, "日 / 夜两套，多一套少一套都要改这里");

        for id in ["a11y_day", "a11y_night"] {
            let p = PaperTheme::by_id(id);
            assert_eq!(p.id, id, "PAPER_THEMES 里没有 {id}");
            let a = a11y_theme(id).unwrap_or_else(|| panic!("A11Y_THEMES 里没有 {id}"));

            // ① 书页本身。
            let body = contrast_ratio(p.text, p.paper);
            assert!(body >= 7.0, "{id} 正文对纸只有 {body:.2}:1");
            let note = contrast_ratio(p.note, p.paper);
            assert!(note >= 7.0, "{id} 夹注对纸只有 {note:.2}:1（小字更要够黑）");

            // ② 朗读高亮的两块色。
            let line = contrast_ratio(a.line_ink, a.line_bg);
            assert!(line >= 7.0, "{id} 句底字对句底只有 {line:.2}:1");
            let word = contrast_ratio(a.word_ink, a.word_bg);
            assert!(word >= 7.0, "{id} 词底字对词底只有 {word:.2}:1");

            // ③ 词必须从句里跳出来。
            let split = contrast_ratio(a.word_bg, a.line_bg);
            assert!(split >= 7.0, "{id} 词底对句底只有 {split:.2}:1（光标看不出来）");

            // ④ 句读朱在这张纸上得看得见（图形线 3:1 是 WCAG 1.4.11 的下限）。
            let zhu = contrast_ratio(p.zhu, p.paper);
            assert!(zhu >= 3.0, "{id} 句读朱对纸只有 {zhu:.2}:1");

            // ⑤ 句底字必须与**句底**分得开；它和正文同色没关系（夜档两者都是
            //    纯白，正该如此），但不能和色块同色，否则字就沉进块里了。
            assert_ne!(a.line_ink, a.line_bg, "{id} 句底字与句底同色");
        }
    }

    /// **护眼档：黑底必须是黄字，且每一项都可断言**（v1.9.83）。
    ///
    /// 菜单栏「高对比护眼模式」一键进的就是这一档。测三件事：
    ///   ① 纸真是纯黑（不是深棕）——OLED 才省电，蓝光才少；
    ///   ② 字真是暖黄（不是白）——红通道要显著高于蓝通道，否则就是
    ///      换了名字的高对比夜；
    ///   ③ 正文 / 夹注 / 句读 / 界面字四路对比度达标——「护眼」不许
    ///      变成「看不清」的委婉说法。
    #[test]
    fn z_huyan_is_black_on_amber_and_readable() {
        let t = Theme::Huyan;
        assert!(t.is_huyan() && t.is_dark());
        assert_eq!(t.paper(), 0x000000, "护眼档纸必须是纯黑");

        let ink = t.ink();
        let r = (ink >> 16) & 0xFF;
        let b = ink & 0xFF;
        assert!(r >= b + 100, "护眼正文要是暖黄而非白：{ink:06X}");
        let body = contrast_ratio(ink, t.paper());
        assert!(body >= 7.0, "护眼正文对纸只有 {body:.2}:1");

        // 夹注是小字，7:1 才配得上「护眼」这两个字。
        let note = contrast_ratio(t.note(), t.paper());
        assert!(note >= 7.0, "护眼夹注对纸只有 {note:.2}:1");

        // 句读取 `zhu()`（深底会兑 20% 纸色），按图形线 3:1 卡。
        let zhu = contrast_ratio(t.zhu(), t.paper());
        assert!(zhu >= 3.0, "护眼角读对纸只有 {zhu:.2}:1");

        // 界面带被纯黑逼着向正文偏了 7%，字仍是正文色——顶栏不糊。
        let chrome = contrast_ratio(t.chrome_text(), t.chrome());
        assert!(chrome >= 7.0, "护眼界面字对界面底只有 {chrome:.2}:1");

        // 一键进 / 出：进档恒 Huyan，出档原样回进档前那套。
        assert_eq!(Theme::Mi.toggle_huyan(None), Theme::Huyan);
        assert_eq!(Theme::Huyan.toggle_huyan(Some(Theme::Mi)), Theme::Mi);
        // 冷启动就在档里（没有 back）回默认宣纸，而不是卡在档里出不来。
        assert_eq!(Theme::Huyan.toggle_huyan(None), Theme::Xuan);
    }

    /// **无障碍档不能被墨色组顶掉**（v1.9.82）。
    ///
    /// 读者若把墨色切到「赭朱 / 暗金」，正文就不再是纯黑纯白，7:1 立刻失真。
    /// 故 `InkPalette::ink/note` 在无障碍档里**直接短路**到纸主题——这条测试
    /// 就是在钉那个短路：四套墨色算出来必须是同一个值。
    #[test]
    fn z_a11y_ignores_the_ink_palette_choice() {
        for t in [Theme::A11yDay, Theme::A11yNight] {
            let want_ink = t.ink();
            let want_note = t.note();
            for p in InkPalette::ALL {
                assert_eq!(p.ink(t), want_ink, "{p:?}/{:?} 把正文改掉了", t);
                assert_eq!(p.note(t), want_note, "{p:?}/{:?} 把夹注改掉了", t);
            }
            assert!(contrast_ratio(want_ink, t.paper()) >= 7.0);
        }
    }

    /// **一键切换的来去**（v1.9.82）：进得去、退得回，且不吞掉用户原来的纸色。
    #[test]
    fn z_toggle_a11y_round_trips_to_the_original_theme() {
        // 浅纸进「日」，深纸进「夜」。
        assert_eq!(Theme::Xuan.toggle_a11y(None), Theme::A11yDay);
        assert_eq!(Theme::Ke.toggle_a11y(None), Theme::A11yDay);
        assert_eq!(Theme::Ye.toggle_a11y(None), Theme::A11yNight);
        // 退回来处：用户调的那套原样还给他。
        assert_eq!(Theme::A11yDay.toggle_a11y(Some(Theme::Mi)), Theme::Mi);
        assert_eq!(Theme::A11yNight.toggle_a11y(Some(Theme::Ke)), Theme::Ke);
        // 没有来处（比如冷启动就停在无障碍档）就回默认，不许留在原地打转。
        assert_eq!(Theme::A11yDay.toggle_a11y(None), Theme::Xuan);
        // 两次切换 = 回到原点。
        for t in [Theme::Xuan, Theme::Mi, Theme::Ye, Theme::Ke] {
            let there = t.toggle_a11y(None);
            assert!(there.is_a11y());
            assert_eq!(there.toggle_a11y(Some(t)), t);
        }
    }

    /// **句读朱砂必须是「活朱」**（v1.84.0）：用户拿精刻本照片比过——照片里    /// 那枚朱砂读点是 `#B3262F` 那一路的鲜红，压在冷米白上跳出来；而朱印那
    /// 档（`accent`）是沉朱，用来点句读就读成一枚暗红的墨点，没有书卷气。
    /// 故句读另开 `zhu` 一档，且它必须**明显偏红**（R 高出 G/B 一大截）。
    #[test]
    fn z_zhu_is_a_livelier_cinnabar_than_the_seal_ink() {
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing, Theme::Ke] {
            let zhu = t.zhu();
            let (r, g, b) = (
                ((zhu >> 16) & 0xFF) as i32,
                ((zhu >> 8) & 0xFF) as i32,
                (zhu & 0xFF) as i32,
            );
            assert!(
                r - g.max(b) > 90,
                "{t:?} 句读朱不够朱（#{zhu:06X}）：压差只有 {}，读起来是枚暗红墨点",
                r - g.max(b)
            );
            // 与朱印那档是两只不同的朱（各归各的档，谁也别盖过谁）。
            assert_ne!(zhu, t.accent(), "{t:?} 句读朱与朱印同色，白分一档");
        }
        // 仿刻本那一档照照片取色（照片实测 #B3262F，压一档沉度得 #B0342C）。
        assert_eq!(Theme::Ke.zhu(), 0xB0342C);
        // 夜读深底：朱砂向纸色兑淡一档，别在深底上发闷。
        assert!(lum(Theme::Ye.zhu()) < lum(Theme::Ye.paper()) + 130.0);
    }

    #[test]
    fn z_note_is_fainter_than_ink_on_light_paper() {
        // 浅纸：夹注目色 = 墨色向纸色兑淡 → 一定比正文淡（亮度更高）。
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing] {
            for p in InkPalette::ALL {
                let ink = p.ink(t);
                let note = p.z_note(t, false);
                assert!(
                    lum(note) > lum(ink),
                    "{:?}/{:?} 浅纸夹注应比正文淡：ink={ink:06X} note={note:06X}",
                    p,
                    t
                );
                assert_ne!(note, ink, "夹注目色不应与正文同色");
            }
        }
    }

    #[test]
    fn z_note_auto_switches_on_dark_paper() {
        // 深底：正文被提亮，夹注跟着沉下去 → 仍比正文淡（亮度更低）。
        for p in InkPalette::ALL {
            let ink = p.ink(Theme::Ye);
            let note = p.z_note(Theme::Ye, false);
            assert!(
                lum(note) < lum(ink),
                "{p:?} 深底夹注应比正文沉：ink={ink:06X} note={note:06X}"
            );
        }
    }

    #[test]
    fn gold_group_pairs_gold_ink_with_off_white_note() {
        // 暗金组：深底正文月白（0xDCD6CD）、批注秋香（0xA68E5C）。
        assert_eq!(InkPalette::Gold.ink(Theme::Ye), 0xDCD6CD);
        // 深底夹注 = 0xA68E5C 向夜读深纸兑淡 0.38 档。
        assert_eq!(InkPalette::Gold.z_note(Theme::Ye, false), 0x716344);
        // 浅纸：正文暗金可读，夹注自动兑淡（淡金，但仍比正文淡）。
        let ink = InkPalette::Gold.ink(Theme::Xuan);
        assert!(lum(ink) < 128.0, "浅纸暗金正文要够深：{ink:06X}");
        assert!(lum(InkPalette::Gold.z_note(Theme::Xuan, false)) > lum(ink));
    }

    #[test]
    fn paper_themes_are_registered_and_indexed() {
        // 主题数与配置表严格同长：加一套配色，两边必须同时加。
        assert_eq!(Theme::ALL.len(), PAPER_THEMES.len());
        assert_eq!(PAPER_THEMES.len(), 8);
        assert_eq!(InkPalette::ALL.len(), 4);
        for (i, t) in Theme::ALL.iter().copied().enumerate() {
            let p = t.paper_theme();
            assert_eq!(t.index(), i, "{:?} 下标错位", t);
            assert_eq!(t.label(), p.name, "{:?} 名字与配置表不同步", t);
            assert_eq!(t.paper(), p.paper);
            assert_eq!(t.ink(), p.text);
            assert_eq!(t.note(), p.note);
            assert_eq!(t.rule_ink(), p.border);
            // 纸色与墨色智能联动：每套纸都有默认墨色配对。
            assert_eq!(t.linked_palette().ink(t), p.text, "{:?} 联动墨色错位", t);
        }
    }

    #[test]
    fn themes_are_low_saturation_guose() {
        // 摒弃高饱和刺眼色：纸 / 正文 / 界栏一律低饱和（通道极差受限）。
        // 夹注朱色是朱印语汇，可以比纸「艳」，但仍不许染成荧光。
        //
        // **无障碍两套与护眼档不在此列**（v1.9.82 / v1.9.83）：那几档的
        // 指标是对比度而不是饱和度，纯黄 / 深蓝 / 暖黄本来就是高饱和的——
        // 硬拿这条去卡，等于要求「看得清」与「看着不刺眼」同时成立，
        // 而前者优先。判据用「纯色纸」这个物理事实，不贴标签。
        for t in Theme::ALL {
            if t.paper_theme().is_pure() {
                continue;
            }
            let p = t.paper_theme();
            assert!(sat(p.paper) < 60.0, "{} 纸色太艳：{:06X}", t.label(), p.paper);
            assert!(sat(p.text) < 80.0, "{} 正文太艳：{:06X}", t.label(), p.text);
            assert!(sat(p.border) < 80.0, "{} 界栏太艳：{:06X}", t.label(), p.border);
            assert!(sat(p.note) < 170.0, "{} 夹注过艳：{:06X}", t.label(), p.note);
        }
    }

    #[test]
    fn ui_chrome_always_inverts_with_text() {
        // 深底（夜读）下界面底要更暗、界面字要更亮——绝不出现白底白字 / 黑底黑字。
        for t in Theme::ALL {
            let d = (luma(t.chrome_text()) - luma(t.chrome())).abs();
            assert!(d > 60.0, "{} 界面字与界面底对比不足：{d:.1}", t.label());
            // 正文与纸面同样要有可辨对比。
            let di = (luma(t.ink()) - luma(t.paper())).abs();
            assert!(di > 60.0, "{} 正文与纸对比不足：{di:.1}", t.label());
            if t.is_a11y() || t.is_huyan() {
                // 无障碍 / 护眼档：纸是纯白 / 纯黑的一端，「更亮 / 更暗」这个
                // 方向已经不成立（白到顶、黑到底）。改为要求「界面底必须与纸
                // 分得开」+「界面字照 AAA 正文级」。
                assert_ne!(
                    luma(t.chrome()),
                    luma(t.paper()),
                    "{} 界面底与纸同色，顶栏会糊成一片",
                    t.label()
                );
                let r = contrast_ratio(t.chrome_text(), t.chrome());
                assert!(r >= 7.0, "{} 界面字对界面底只有 {r:.2}:1", t.label());
                continue;
            }
            // 深底判定：纸暗则底更暗、字更亮。
            if t.is_dark() {
                assert!(luma(t.chrome()) < luma(t.paper()), "{} 深底界面底应更沉", t.label());
                assert!(luma(t.chrome_text()) > luma(t.paper()), "{} 深底界面字应提亮", t.label());
            } else {
                assert!(luma(t.chrome()) > luma(t.paper()), "{} 浅纸界面底应更亮", t.label());
                assert!(luma(t.chrome_text()) < luma(t.paper()), "{} 浅纸界面字应更深", t.label());
            }
        }
    }

    #[test]
    fn mix_blends_channels() {
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xFFFFFF, 1.0), 0xFFFFFF);
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x405060, 0.5), 0x283848);
    }

    #[test]
    fn back_leaf_uses_back_paper() {
        // 对开左叶纸色略深 → 兑淡基点不同（两叶夹注目色有细微差别）。
        // 墨朱组已改为直取纸主题夹注色（两叶同色），故这里看组内配色组。
        assert_ne!(
            InkPalette::Forest.z_note(Theme::Xuan, true),
            InkPalette::Forest.z_note(Theme::Xuan, false)
        );
    }

    // ---- 纸面主题配置表（UI 与渲染的唯一数据源）----

    #[test]
    fn paper_themes_are_the_single_source_of_truth() {
        assert_eq!(PAPER_THEMES.len(), 8);
        let ids: Vec<&str> = PAPER_THEMES.iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                "xuanzhi", "mihuang", "qingjian", "yedou", "keben", "a11y_day", "a11y_night",
                "huyan"
            ]
        );
        let names: Vec<&str> = PAPER_THEMES.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec![
                "宣纸",
                "米黄",
                "青简",
                "夜读",
                "仿刻本",
                "高对比日",
                "高对比夜",
                "护眼"
            ]
        );
        // 四色齐全、互不重复（纸 / 正文 / 夹注 / 界栏）。
        for p in PAPER_THEMES {
            assert!(luma(p.paper) != luma(p.text), "{} 纸与正文同色", p.id);
            assert_ne!(p.text, p.note, "{} 正文与夹注同色", p.id);
        }
    }

    /// 仿刻本（v1.83.0）：纸号对不对、界行是否退了一档、其余主题是否
    /// **一个像素都没动**——这是「不许碰老版面」的那道闸。
    #[test]
    fn keben_paper_is_f7f6f1_and_rules_fade_one_step() {
        // ① 用户点名的纸号，逐位钉死（改纸色＝改这一档的全部气质）。
        assert_eq!(Theme::Ke.paper(), 0xF7F6F1);
        assert!(Theme::Ke.is_keben());
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing, Theme::Ye] {
            assert!(!t.is_keben(), "{t:?} 不该被当成仿刻本");
        }
        // ② 仿刻本的界行 / 内框比外框淡一档（外框仍是 `frame_ink` 的浓墨）。
        for rc in crate::types::RuleColor::ALL {
            let outer = Theme::Ke.frame_ink();
            let inner = Theme::Ke.rule_line(rc);
            assert_ne!(inner, outer, "{rc:?} 的内框与界行必须退一档");
            assert!(
                luma(inner) > luma(outer),
                "{rc:?} 的内框应比外框淡：{inner:06X} vs {outer:06X}"
            );
            assert!(
                luma(Theme::Ke.paper()) - luma(inner) > 12.0,
                "{rc:?} 的界行淡到看不见了：{inner:06X}"
            );
        }
        // ③ 浅纸三套：`rule_line` 必须与 `rule_by` 逐值相同
        //    （老版面的框线颜色一分都不许变）。夜读自 v1.85.0 起另退一档，
        //    见下一段。
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing] {
            for rc in crate::types::RuleColor::ALL {
                assert_eq!(
                    t.rule_line(rc),
                    t.rule_by(rc),
                    "{t:?}/{rc:?} 的界行色被新主题带偏了"
                );
            }
        }
        // ④ 夜读（v1.85.0）：外框提亮、内框退暗，双线才分得开。
        for rc in crate::types::RuleColor::ALL {
            let outer = Theme::Ye.frame_outer();
            let inner = Theme::Ye.rule_line(rc);
            assert!(
                luma(outer) > luma(Theme::Ye.rule_ink()),
                "{rc:?} 夜读的外框没提亮：{outer:06X}"
            );
            assert!(
                luma(inner) < luma(Theme::Ye.rule_by(rc)),
                "{rc:?} 夜读的内框没退暗：{inner:06X}"
            );
            // 层次 = 「一进一出」的亮度差；金丝栏本是亮金，方向反着也算拉开。
            assert!(
                (luma(outer) - luma(inner)).abs() >= 12.0,
                "{rc:?} 夜读的双线拉不开层次：{outer:06X} vs {inner:06X}"
            );
            // 退了还得看得见：内框与纸之间必须有可辨的亮度差。
            assert!(
                (luma(inner) - luma(Theme::Ye.paper())).abs() > 8.0,
                "{rc:?} 夜读的界行淡到看不见了：{inner:06X}"
            );
        }
        // ⑤ 浅纸四套的外框取色一分不动（提亮只发生在深底）。
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing, Theme::Ke] {
            assert_eq!(
                t.frame_outer(),
                t.paper_theme().border,
                "{t:?} 的外框色被带偏了"
            );
        }
    }

    #[test]
    fn theme_accessors_read_from_paper_theme_config() {
        // 渲染四色严格取自配置表：背景=纸色、正文=textColor、
        // 夹注=noteColor、界栏/版框=borderColor。
        for t in Theme::ALL {
            let p = t.paper_theme();
            assert_eq!(t.paper(), p.paper);
            assert_eq!(t.ink(), p.text);
            assert_eq!(t.note(), p.note);
            assert_eq!(t.rule_ink(), p.border, "界栏必须用 borderColor");
            assert_eq!(t.frame_ink(), p.border, "版框必须用 borderColor");
        }
        // 默认墨色组：正文 / 夹注分别用配置表的 textColor / noteColor。
        for t in Theme::ALL {
            let p = t.paper_theme();
            assert_eq!(InkPalette::Ink.ink(t), p.text);
            assert_eq!(InkPalette::Ink.note(t), p.note);
            assert_eq!(InkPalette::Ink.z_note(t, false), p.note);
        }
    }

    #[test]
    fn dark_theme_is_derived_from_paper_luma() {
        assert!(Theme::Ye.is_dark());
        for t in [Theme::Xuan, Theme::Mi, Theme::Qing] {
            assert!(!t.is_dark());
        }
        // 深底判定来自纸色亮度，不是写死枚举。
        assert!(PaperTheme::by_id("yedou").is_dark());
        assert!(!PaperTheme::by_id("xuanzhi").is_dark());
        // 未知 id 回落首项（配置损坏不崩）。
        assert_eq!(PaperTheme::by_id("nope").id, "xuanzhi");
    }

    #[test]
    fn fish_ink_is_dark_grey_not_black_and_visible_on_every_paper() {
        // 黑鱼尾：浅纸上就是 #333333（不取纯黑，纯黑太跳）。
        assert_eq!(Theme::Xuan.fish_ink(), crate::banxin::FISH_INK);
        assert_eq!(crate::banxin::FISH_INK, 0x222222);
        // 深底（夜读）兑淡：鱼尾不能因为纸黑就整枚消失。
        let ye = Theme::Ye.fish_ink();
        assert_ne!(ye, crate::banxin::FISH_INK);
        assert!(luma(ye) > luma(Theme::Ye.paper()), "夜读的鱼尾应比纸亮");
        // 任何纸上鱼尾与纸色都要有可辨的对比。
        for t in Theme::ALL {
            let d = (luma(t.fish_ink()) - luma(t.paper())).abs();
            assert!(d > 25.0, "{} 的鱼尾与纸对比不足：{d:.1}", t.label());
        }
    }

    #[test]
    fn fold_ink_stays_faint_but_readable() {
        // 折痕是「纸的褶」，必须比界行再淡一档。
        for t in Theme::ALL {
            let f = t.fold_ink();
            let d = (luma(f) - luma(t.paper())).abs();
            assert!(d > 8.0, "{} 的折痕看不见：{d:.1}", t.label());
            assert!(
                d < (luma(t.rule_ink()) - luma(t.paper())).abs(),
                "{} 的折痕不该比界行还重",
                t.label()
            );
        }
    }

    #[test]
    fn paper_back_is_darker_than_paper() {
        for t in Theme::ALL {
            let p = t.paper();
            let b = t.paper_back();
            assert_ne!(p, b);
            if t.paper_theme().is_pure() {
                // 纯白 / 纯黑已经在一端，背叶只能**反向**偏一档（夜档向白提
                // 5%）。要的是「两叶分得开」，不是「一定更黑」。
                continue;
            }
            if t.is_dark() {
                assert!(luma(b) <= luma(p), "深底背面应更沉");
            } else {
                assert!(luma(b) < luma(p), "浅纸背面应更深");
            }
        }
    }
}
