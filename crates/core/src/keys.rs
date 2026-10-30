//! **全局快捷键**：动作表 + 键位映射的唯一数据源。
//!
//! 分三层，各管一件事：
//!   * [`Action`]——**能干什么**。每个动作一个稳定 id（`"search"` / `"punct_mode"`），
//!     外加中文标签、分组、默认键位。id 一旦定下就**不许改**：它是
//!     `prefs.json` 里用户改键记录的键，改一个 id 等于把老用户的改键全丢。
//!   * [`Chord`]——**按了什么**。修饰键 + 一个键名，与 UI 框架无关（core 不许
//!     碰 gpui）。`"cmd+shift+a"` 这种字符串是可读、可落盘的规范写法。
//!   * [`KeyMap`]——**动作 → 键位**的现行映射，含用户改写。
//!
//! # 为什么不把键位写死在 app 的 match 里
//!
//! 原先 24 个 ⌘ 组合散在 `reader::handle_reading_key` 的一个大 `match` 里：
//! 加一个键要改三处（match、帮助面板的说明、README），改一个键要翻半天，
//! 而且**用户没法自己改**——撞上系统快捷键（比如某些输入法占用 ⌘⇧A）就只能忍。
//! 现在键位只有这一份表：面板照着 [`Action::ALL`] 渲染，按键走
//! [`KeyMap::resolve`]，两者读的是同一个源，绝不会「说明里写 ⌘F、按下没反应」。
//!
//! # 冲突口径
//!
//! [`KeyMap::bind`] 拒绝把两个动作绑到同一个 chord 上，并报出占用者的标签
//! （面板直接显示这句话，用户不必猜）。**不做自动挪位**——自动挪位会在用户
//! 眼前悄悄换掉另一个键，比报错难受得多。

use std::collections::BTreeMap;

/// 一个可绑键的动作。
///
/// `id` 是持久化口径，`label` 是面板文案，`section` 是面板分组。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    // ── 阅读 ──────────────────────────────────────────────
    Search,
    /// **下一处命中**：`search_next` 早就写好了，只差一个键位绑上去
    /// （v1.9.10）。检索框里敲完词，就该能一路按下去跳，不必回鼠标点。
    SearchNext,
    SearchPrev,
    Lookup,
    Copy,
    Underline,
    Notes,
    Tongjia,
    Punct,
    PunctMode,
    Rare,
    Traditional,
    Theme,
    // ── 视图 ──────────────────────────────────────────────
    Style,
    Side,
    Focus,
    Cards,
    Help,
    /// **文本量化分析**（v1.81.0 · 模块五）：字词 / 句读 / 文风三栏抽屉。
    Analysis,
    /// **连续缩放**（v1.9.10）：字级四档是「气象」，⌘+/⌘- 是「寸尺」——
    /// 档位给人换手感，微调给人换眼睛。两者写同一个 `zoom` 倍率。
    ZoomIn,
    ZoomOut,
    // ── 墨色（六套）────────────────────────────────────────
    Palette1,
    Palette2,
    Palette3,
    Palette4,
    Palette5,
    Palette6,
    // ── 问小笺 ────────────────────────────────────────────
    AskAi,
    // ── 印制 ──────────────────────────────────────────────
    ExportLeaf,
    ExportChapter,
}

impl Action {
    /// 面板渲染顺序 = 这份数组的顺序（分组靠 [`Action::section`] 相邻聚拢）。
    pub const ALL: [Action; 30] = [
        Action::Search,
        Action::SearchNext,
        Action::SearchPrev,
        Action::Lookup,
        Action::Copy,
        Action::Underline,
        Action::Notes,
        Action::Tongjia,
        Action::Punct,
        Action::PunctMode,
        Action::Rare,
        Action::Traditional,
        Action::Theme,
        Action::Style,
        Action::Side,
        Action::Focus,
        Action::Cards,
        Action::Help,
        Action::Analysis,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::Palette1,
        Action::Palette2,
        Action::Palette3,
        Action::Palette4,
        Action::Palette5,
        Action::Palette6,
        Action::AskAi,
        Action::ExportLeaf,
        Action::ExportChapter,
    ];

    /// 持久化 id：**永久不变**。
    pub fn id(self) -> &'static str {
        match self {
            Action::Search => "search",
            Action::SearchNext => "search_next",
            Action::SearchPrev => "search_prev",
            Action::Lookup => "lookup",
            Action::Copy => "copy",
            Action::Underline => "underline",
            Action::Notes => "notes",
            Action::Tongjia => "tongjia",
            Action::Punct => "punct",
            Action::PunctMode => "punct_mode",
            Action::Rare => "rare",
            Action::Traditional => "traditional",
            Action::Theme => "theme",
            Action::Style => "style",
            Action::Side => "side",
            Action::Focus => "focus",
            Action::Cards => "cards",
            Action::Help => "help",
            Action::Analysis => "analysis",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::Palette1 => "palette1",
            Action::Palette2 => "palette2",
            Action::Palette3 => "palette3",
            Action::Palette4 => "palette4",
            Action::Palette5 => "palette5",
            Action::Palette6 => "palette6",
            Action::AskAi => "ask_ai",
            Action::ExportLeaf => "export_leaf",
            Action::ExportChapter => "export_chapter",
        }
    }

    /// 反查 id（读 `prefs.json` 用）。未知 id 一律 `None`——旧版本删掉的动作
    /// 不该让整份改键记录作废。
    pub fn from_id(s: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.id() == s)
    }

    /// 面板上的中文说明。
    pub fn label(self) -> &'static str {
        match self {
            Action::Search => "全库检索",
            Action::SearchNext => "下一处命中",
            Action::SearchPrev => "上一处命中",
            Action::Lookup => "查字（选区或本页首字）",
            Action::Copy => "复制（有选区复制选区，否则整页）",
            Action::Underline => "划线（有选区划选区）",
            Action::Notes => "注释显隐",
            Action::Tongjia => "通假显隐",
            Action::Punct => "句读显隐",
            Action::PunctMode => "现代标点 / 古籍句读",
            Action::Rare => "生僻注音显隐",
            Action::Traditional => "简体 / 繁体互转",
            Action::Theme => "切换纸色",
            Action::Style => "版字面板",
            Action::Side => "左侧栏（目录 / 书签 / 批注）",
            Action::Focus => "专注模式",
            Action::Cards => "卡片匣",
            Action::Help => "帮助",
            Action::Analysis => "文本分析（字词 / 句读 / 文风）",
            Action::ZoomIn => "放大字号（连续微调）",
            Action::ZoomOut => "缩小字号（连续微调）",
            Action::Palette1 => "墨色：朱",
            Action::Palette2 => "墨色：墨",
            Action::Palette3 => "墨色：纸",
            Action::Palette4 => "墨色：青",
            Action::Palette5 => "墨色：黛",
            Action::Palette6 => "墨色：暗金",
            Action::AskAi => "问小笺（有选区带原文）",
            Action::ExportLeaf => "印制：本叶 PDF",
            Action::ExportChapter => "印制：本章 PDF",
        }
    }

    /// 面板分组标题。相邻同名的自动并成一段。
    pub fn section(self) -> &'static str {
        match self {
            Action::Search
            | Action::SearchNext
            | Action::SearchPrev
            | Action::Lookup
            | Action::Copy
            | Action::Underline
            | Action::Notes
            | Action::Tongjia
            | Action::Punct
            | Action::PunctMode
            | Action::Rare
            | Action::Traditional
            | Action::Theme => "阅读",
            Action::Style
            | Action::Side
            | Action::Focus
            | Action::Cards
            | Action::Help
            | Action::Analysis
            | Action::ZoomIn
            | Action::ZoomOut => "视图",
            Action::Palette1
            | Action::Palette2
            | Action::Palette3
            | Action::Palette4
            | Action::Palette5
            | Action::Palette6 => "墨色",
            Action::AskAi => "问小笺",
            Action::ExportLeaf | Action::ExportChapter => "印制",
        }
    }

    /// 出厂键位（`Chord::parse` 的规范写法）。
    ///
    /// 刻意避开 macOS 系统保留键：⌘H（隐藏）、⌘M（最小化）、⌘Q（退出）、
    /// ⌘W（关窗）、⌘S（保存）。⌘⇧A 与 ⌘⌥P 是用户点名要的。
    pub fn default_chord(self) -> &'static str {
        match self {
            Action::Search => "cmd+f",
            // ⌘⇧G 是「查找下一个」的行规（⌘G 已被「查字」占着，让不出来）。
            Action::SearchNext => "cmd+shift+g",
            Action::SearchPrev => "cmd+alt+g",
            Action::Lookup => "cmd+g",
            Action::Copy => "cmd+c",
            Action::Underline => "cmd+u",
            Action::Notes => "cmd+n",
            // 通假让出 ⌘J（v1.81.0：⌘J 归「现代标点 / 古籍句读」），挪到 ⌘Y。
            Action::Tongjia => "cmd+y",
            Action::Punct => "cmd+b",
            Action::PunctMode => "cmd+j",
            Action::Rare => "cmd+r",
            Action::Traditional => "cmd+t",
            Action::Theme => "cmd+p",
            Action::Style => "cmd+e",
            Action::Side => "cmd+l",
            Action::Focus => "cmd+z",
            Action::Cards => "cmd+k",
            Action::Help => "cmd+/",
            Action::Analysis => "cmd+i",
            // 键名经 norm_key 归一：`+` / `plus` / `equal` 一律落 `=`，
            // 所以无论框架报 ⌘= 还是 ⌘+（Shift 态），都认得出来。
            Action::ZoomIn => "cmd+=",
            Action::ZoomOut => "cmd+-",
            Action::Palette1 => "cmd+1",
            Action::Palette2 => "cmd+2",
            Action::Palette3 => "cmd+3",
            Action::Palette4 => "cmd+4",
            Action::Palette5 => "cmd+5",
            Action::Palette6 => "cmd+6",
            Action::AskAi => "cmd+shift+a",
            Action::ExportLeaf => "cmd+alt+p",
            Action::ExportChapter => "cmd+alt+shift+p",
        }
    }
}

/// 一次按键：四个修饰键 + 一个键名。
///
/// 与 UI 框架解耦（core 不许 `use gpui`），转换在壳子里做。键名统一小写，
/// `"escape"` / `"enter"` 这类命名键用全称。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    /// 小写键名：`"a"` / `"1"` / `"/"` / `"escape"`。
    pub key: String,
}

impl Chord {
    /// 造一个新的 chord（键名会被小写化）。
    pub fn new(cmd: bool, shift: bool, alt: bool, ctrl: bool, key: &str) -> Self {
        Chord {
            cmd,
            shift,
            alt,
            ctrl,
            key: norm_key(key),
        }
    }

    /// 解析 `"cmd+shift+a"` 这类规范写法。
    ///
    /// 修饰词认 `cmd`/`command`/`meta`、`shift`、`alt`/`opt`/`option`、
    /// `ctrl`/`control`（不分大小写）；最后一个片段是键名，可以是一个字符
    /// （`"a"` / `"1"` / `"/"`）或命名键（`"escape"`）。
    pub fn parse(s: &str) -> Option<Chord> {
        let mut cmd = false;
        let mut shift = false;
        let mut alt = false;
        let mut ctrl = false;
        let mut key: Option<String> = None;
        for raw in s.split('+') {
            let t = raw.trim();
            if t.is_empty() {
                continue;
            }
            match t.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "meta" | "super" => cmd = true,
                "shift" => shift = true,
                "alt" | "opt" | "option" => alt = true,
                "ctrl" | "control" => ctrl = true,
                other => {
                    // 键名原样留着（`"/"`、`"="` 也得能绑），只做小写化。
                    key = Some(norm_key(other));
                }
            }
        }
        Some(Chord {
            cmd,
            shift,
            alt,
            ctrl,
            key: key?,
        })
    }

    /// 面板上显示的样子：`⌃⌥⇧⌘` + 键名（与 macOS 的书写顺序一致）。
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push('⌃');
        }
        if self.alt {
            s.push('⌥');
        }
        if self.shift {
            s.push('⇧');
        }
        if self.cmd {
            s.push('⌘');
        }
        s.push_str(&pretty_key(&self.key));
        s
    }

    /// 回写成可落盘的规范串。
    pub fn to_spec(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.cmd {
            parts.push("cmd");
        }
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        let mut s = parts.join("+");
        if !s.is_empty() {
            s.push('+');
        }
        s.push_str(&self.key);
        s
    }

    /// 有没有按修饰键。裸字母不该被当成快捷键——否则正文里打字会触发动作。
    pub fn has_modifier(&self) -> bool {
        self.cmd || self.ctrl || self.alt
    }
}

/// 键名归一：一律小写；`esc` / `return` 之类的别名收敛到全称。
fn norm_key(k: &str) -> String {
    let t = k.trim().to_ascii_lowercase();
    match t.as_str() {
        "esc" => "escape".to_string(),
        "return" => "enter".to_string(),
        // ⌘+ 在有的框架里报 `"+"`、有的报 `"="`（还有报 `"equal"` 的），
        // 三种都收敛到 `"="`，否则「放大」这条键在部分键盘上永远按不响。
        // ⌘- 同理收 `"minus"`；`/` 上的 `⌘?` 不受影响（那是 shift 态的 `/`）。
        "+" | "plus" | "equal" => "=".to_string(),
        "minus" => "-".to_string(),
        _ => t,
    }
}

/// 显示用：单字符转大写，命名键给中文习惯的缩写。
fn pretty_key(k: &str) -> String {
    match k {
        "escape" => "Esc".to_string(),
        "enter" => "⏎".to_string(),
        "space" => "空格".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "backspace" => "⌫".to_string(),
        "tab" => "⇥".to_string(),
        other => {
            let mut cs = other.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => c.to_uppercase().to_string(),
                _ => other.to_string(),
            }
        }
    }
}

/// 动作 → 键位 的现行映射。
///
/// 只存**被改过的**那些（`overrides`）：出厂值仍在 [`Action::default_chord`]
/// 里，改键记录因此极小，日后调整出厂键位也能自动惠及没改过的用户。
#[derive(Clone, Debug, Default)]
pub struct KeyMap {
    overrides: BTreeMap<Action, Chord>,
}

impl KeyMap {
    /// 全出厂键位。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从落盘的 `(动作 id, 键位串)` 还原。认不出的 id / 解析不了的键位一律跳过
    /// ——坏一条不该让整份改键记录作废。
    pub fn from_pairs(pairs: &[(String, String)]) -> Self {
        let mut m = KeyMap::new();
        for (id, spec) in pairs {
            let (Some(a), Some(c)) = (Action::from_id(id), Chord::parse(spec)) else {
                continue;
            };
            m.overrides.insert(a, c);
        }
        m
    }

    /// 落盘用的 `(id, 键位串)`，按 id 排序保证文件稳定（便于 diff）。
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        self.overrides
            .iter()
            .map(|(a, c)| (a.id().to_string(), c.to_spec()))
            .collect()
    }

    /// 这个动作现在的键位（没改过就是出厂值）。
    pub fn chord(&self, a: Action) -> Chord {
        self.overrides
            .get(&a)
            .cloned()
            .or_else(|| Chord::parse(a.default_chord()))
            .unwrap_or_else(|| Chord::new(true, false, false, false, "f"))
    }

    /// 是否被用户改过（面板据此点亮「复位」）。
    pub fn is_custom(&self, a: Action) -> bool {
        self.overrides.contains_key(&a)
    }

    /// 改键。撞车就报错并**不改**，错误串直接给面板显示。
    pub fn bind(&mut self, a: Action, c: Chord) -> Result<(), String> {
        if let Some(other) = self.owner_of(&c, Some(a)) {
            return Err(format!("{} 已被「{}」占用", c.display(), other.label()));
        }
        self.overrides.insert(a, c);
        Ok(())
    }

    /// 复位到出厂键位。
    pub fn reset(&mut self, a: Action) {
        self.overrides.remove(&a);
    }

    /// 全部复位。
    pub fn reset_all(&mut self) {
        self.overrides.clear();
    }

    /// 这个 chord 现在归谁（`skip` 用于改键时排除自己）。
    pub fn owner_of(&self, c: &Chord, skip: Option<Action>) -> Option<Action> {
        Action::ALL
            .iter()
            .copied()
            .filter(|a| Some(*a) != skip)
            .find(|a| self.chord(*a) == *c)
    }

    /// 按键 → 动作。**裸键（无 ⌘⌃⌥）一律不认**：正文里打字不能触发任何动作。
    pub fn resolve(&self, c: &Chord) -> Option<Action> {
        if !c.has_modifier() {
            return None;
        }
        self.owner_of(c, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_unique_id_and_chord() {
        let mut ids = std::collections::HashSet::new();
        let mut map = KeyMap::new();
        for a in Action::ALL {
            assert!(ids.insert(a.id()), "id 重复：{}", a.id());
            assert_eq!(Action::from_id(a.id()), Some(a));
            assert!(!a.label().is_empty());
            assert!(!a.section().is_empty());
            let c = map.chord(a);
            assert!(c.has_modifier(), "出厂键位必须有修饰键：{}", a.id());
            // 出厂键位之间不许撞车。
            assert_eq!(
                map.owner_of(&c, Some(a)),
                None,
                "出厂键位冲突：{} 与 {}",
                a.id(),
                c.display()
            );
            map.overrides.insert(a, c);
        }
    }

    #[test]
    fn parse_and_display_round_trip() {
        let c = Chord::parse("cmd+shift+a").unwrap();
        assert!(c.cmd && c.shift && !c.alt && !c.ctrl);
        assert_eq!(c.key, "a");
        assert_eq!(c.display(), "⇧⌘A");
        assert_eq!(Chord::parse(&c.to_spec()).unwrap(), c);

        // 符号键与命名键也要能往返。
        for spec in ["cmd+/", "cmd+alt+p", "cmd+1", "escape", "ctrl+alt+shift+cmd+k"] {
            let c = Chord::parse(spec).unwrap_or_else(|| panic!("解析不了：{spec}"));
            assert_eq!(
                Chord::parse(&c.to_spec()).unwrap(),
                c,
                "往返丢失：{spec}"
            );
        }
    }

    #[test]
    fn bare_keys_never_resolve() {
        let map = KeyMap::new();
        // 裸字母：正文里打字，绝不能触发「全库检索」。
        assert_eq!(map.resolve(&Chord::parse("f").unwrap()), None);
        assert_eq!(map.resolve(&Chord::parse("shift+a").unwrap()), None);
        assert_eq!(map.resolve(&Chord::parse("cmd+f").unwrap()), Some(Action::Search));
    }

    #[test]
    fn rebind_rejects_conflict_and_reset_restores() {
        let mut map = KeyMap::new();
        let clash = map.chord(Action::Search);
        let err = map.bind(Action::Lookup, clash).unwrap_err();
        assert!(err.contains("全库检索"), "冲突提示要点名占用者：{err}");
        // 撞车后原键位不动。
        assert_eq!(map.chord(Action::Lookup), Chord::parse("cmd+g").unwrap());

        let mine = Chord::parse("cmd+shift+l").unwrap();
        map.bind(Action::Lookup, mine.clone()).unwrap();
        assert!(map.is_custom(Action::Lookup));
        assert_eq!(map.resolve(&mine), Some(Action::Lookup));
        // 旧键位让出来，不再触发。
        assert_eq!(map.resolve(&Chord::parse("cmd+g").unwrap()), None);

        map.reset(Action::Lookup);
        assert!(!map.is_custom(Action::Lookup));
        assert_eq!(map.resolve(&Chord::parse("cmd+g").unwrap()), Some(Action::Lookup));
    }

    #[test]
    fn plus_and_equal_are_the_same_zoom_key() {
        let map = KeyMap::new();
        // 框架报 ⌘= 或 ⌘+（乃至 ⌘plus / ⌘equal）都得落到「放大字号」。
        // 走 `Chord::new`（框架那条路）——`parse` 里 `+` 是修饰分隔符，
        // 写不出 `"cmd++"`，出厂串因此一律用 `"cmd+="`。
        for k in ["=", "+", "plus", "equal", "PLUS"] {
            let c = Chord::new(true, false, false, false, k);
            assert_eq!(c.key, "=", "键名未归一：{k} → {}", c.key);
            assert_eq!(map.resolve(&c), Some(Action::ZoomIn), "⌘{k} 没落到放大");
        }
        for k in ["-", "minus"] {
            let c = Chord::new(true, false, false, false, k);
            assert_eq!(c.key, "-", "键名未归一：{k} → {}", c.key);
            assert_eq!(map.resolve(&c), Some(Action::ZoomOut), "⌘{k} 没落到缩小");
        }
        // 出厂串本身也要能解析回来（改键面板靠它）。
        assert_eq!(Chord::parse("cmd+=").unwrap().key, "=");
        assert_eq!(Chord::parse("cmd+-").unwrap().key, "-");
    }

    #[test]
    fn next_hit_has_its_own_chord() {
        let map = KeyMap::new();
        assert_eq!(map.resolve(&Chord::parse("cmd+f").unwrap()), Some(Action::Search));
        assert_eq!(
            map.resolve(&Chord::parse("cmd+shift+g").unwrap()),
            Some(Action::SearchNext)
        );
        assert_eq!(
            map.resolve(&Chord::parse("cmd+alt+g").unwrap()),
            Some(Action::SearchPrev)
        );
        // ⌘G 仍是「查字」，没被挤掉。
        assert_eq!(map.resolve(&Chord::parse("cmd+g").unwrap()), Some(Action::Lookup));
    }

    #[test]
    fn bad_pairs_are_skipped_not_fatal() {
        let pairs = vec![
            ("search".to_string(), "cmd+shift+f".to_string()),
            ("不存在的动作".to_string(), "cmd+x".to_string()),
            ("lookup".to_string(), "".to_string()),
        ];
        let map = KeyMap::from_pairs(&pairs);
        assert_eq!(map.chord(Action::Search), Chord::parse("cmd+shift+f").unwrap());
        // 坏记录跳过 → 回落出厂值。
        assert_eq!(map.chord(Action::Lookup), Chord::parse("cmd+g").unwrap());
        assert_eq!(map.to_pairs().len(), 1);
    }
}
