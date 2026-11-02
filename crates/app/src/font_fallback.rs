//! **逐字字体回退**（font fallback）+ 缺字占位（v1.9.25）。
//!
//! 三条硬规矩（用户口径，逐条落到代码里）：
//!
//!   ① **首选内置 / 用户自备的古籍字体**——正文永远是 `fonts::family_for`
//!      挑出来的那一支，绝不因为「这字生僻」就整句话换字体；
//!   ② **缺字只许回退到系统 CJK 字体，绝不回退到英文字体**——判据见
//!      [`CJK_PROBE`]：一支字库必须自己认识「漢」字才算中文字库。于是
//!      `Times` / `Helvetica` / `CMU Serif` 这类西文库**天然进不了**回退链，
//!      不必维护一张「黑名单」（黑名单永远会漏）。
//!   ③ **全系统都无解的生僻字 → 画占位「□」并打一行 Warning**，绝不 panic、
//!      绝不闪退，也绝不把一个 `.notdef`（豆腐块）当字画出去。
//!
//! ## 为什么不靠 GPUI 自带兜底
//!
//! GPUI 0.2.2 的 `font_family` 只接受**一个**族名（没有 CSS 式 fallback
//! 列表）。缺字时它内部按平台默认链去找，第一顺位往往是西文字体，生僻字
//! 就掉进豆腐块——而古籍里生僻字是常态（尔雅、说文、方书里的字形一大片
//! 在 GB18030 之外）。所以「该用哪支字体」这件事必须由我们自己算。
//!
//! ## 网格保护（核心）
//!
//! 回退**只管挑字体，绝不参与几何**：字格的宽高恒等于网格引擎给的
//! `cell_w × cell_h`，行盒恒锁 `cell_h`（见 `reader::ancient_cell`）。回退
//! 字体的自带留白（比如某支楷体的巨大 ascent）因此撑不动竖排网格——
//! 这是「换字体绝不能让一行字往下漂」的同一条铁律的延伸。
//!
//! ## 开销
//!
//! 每字一次判断、结果按字缓存（`picks`）。命中「首选字体」这一支是
//! **绝大多数**（99%+ 的正文），此时只做一次 HashMap 取 + 一次 `Arc` 克隆，
//! 不触碰字体系统；真正去问字体系统只在两种时刻：某字第一次出现、或用户
//! 换了字体（缓存按 `(choice, script, face)` 整批作废）。

use crate::types::{FontChoice, Script};
use gpui::{px, App, FontId, SharedString};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// 缺字占位符：空心方块（U+25A1）。刻意选**几何图形**而不是豆腐块——
/// 豆腐块与「字体没装好」长得一样，用户分不清是书的问题还是软件的问题；
/// 空心方块一眼就是「这里缺一个字」。
pub const PLACEHOLDER: char = '□';

/// 中文字库探针字。一支字体若连「漢」都不认识，就不是中文字库——
/// 用它把西文字体挡在回退链之外（比维护黑名单可靠得多）。
const CJK_PROBE: char = '漢';

/// 探针字号：只用来量一次 advance（判断字形在不在），与真实排版无关。
const PROBE_PX: f32 = 32.0;

/// 一个字的字体归属。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickState {
    /// 首选字体本身就有这个字（最常见）。
    Primary,
    /// 首选缺字，落到某支系统 CJK 字体上。
    Fallback,
    /// 全系统都无解 → 画占位符。
    Missing,
}

/// 某个字最终用哪支字体画。
#[derive(Clone, Debug)]
pub struct GlyphPick {
    /// 要下给 `.font_family()` 的族名（`Missing` 时为首选字体，用来画占位符）。
    pub family: SharedString,
    pub state: PickState,
}

impl GlyphPick {
    pub fn is_missing(&self) -> bool {
        self.state == PickState::Missing
    }

    /// 是否需要「锁字框」：回退字体的行盒 / 留白与首选不同，字格要显式
    /// 钉死宽高再居中，绝不让它把竖排网格撑歪。
    pub fn needs_lock(&self) -> bool {
        self.state != PickState::Primary
    }
}

/// 字体角色：正文 / 批注。批注恒定走古文链（横笺没有楷体链）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Face {
    Body,
    Note,
}

impl Face {
    fn id(self) -> u8 {
        match self {
            Face::Body => 0,
            Face::Note => 1,
        }
    }
}

/// 系统兜底链：链表**只为「形制接近宋体」的中文字库**，且都必须过
/// [`CJK_PROBE`] 那一关。`Arial Unicode MS` 名字里带 Arial，但它确实是
/// 一支覆盖了大半 CJK 扩展区的字库——判据是「认不认识漢字」，不是名字。
///
/// 顺序：大字符集宋体 → 系统宋体 → 日文明朝（汉字字形略异，但远胜豆腐块）
/// → 系统黑体 → 最后的万金油。
const CJK_SPARE: &[&str] = &[
    "Source Han Serif SC",
    "Noto Serif CJK SC",
    "Songti SC",
    "STSong",
    "Hiragino Mincho ProN",
    "PingFang SC",
    "Hiragino Sans GB",
    "Heiti SC",
    "STHeiti",
    "Apple LiGothic",
    "Arial Unicode MS",
];

/// 通用族名（`serif` / `sans-serif` …）：它们落到哪支字体由平台定，
/// 可能是西文衬线——不进回退链，免得「偷偷回退到英文字体」。
fn is_generic_family(f: &str) -> bool {
    matches!(f, "serif" | "sans-serif" | "monospace" | "system-ui")
}

/// 回退链：先用**当前选择自己的链**（同气质，形制最接近），再去系统兜底。
/// 纯函数，可单测。
fn candidate_chain(choice: FontChoice, script: Script, face: Face) -> Vec<&'static str> {
    let chain = match face {
        Face::Note => crate::fonts::ancient_chain(choice),
        Face::Body => match script {
            Script::Ancient => crate::fonts::ancient_chain(choice),
            Script::Modern => crate::fonts::modern_chain(choice),
        },
    };
    let mut out: Vec<&'static str> = chain
        .into_iter()
        .filter(|f| !is_generic_family(f))
        .collect();
    for f in CJK_SPARE {
        if !out.contains(f) {
            out.push(f);
        }
    }
    out
}

/// 在候选里挑第一支**真中文字库且含此字**的。
///
/// 两个判据分开传，是为了能脱离 GPUI 单测：
///   * `is_cjk(f)` —— 这支字体认识「漢」吗；
///   * `has_glyph(f)` —— 这支字体有这个字吗。
///
/// **先过 `is_cjk` 再过 `has_glyph`**（顺序不能反）：反过来的话，一支西文
/// 字体若恰好有个兼容字形，就会被选中——这正是「回退到英文字体」的漏洞。
pub fn choose_covering<C, G>(cands: &[&str], mut is_cjk: C, mut has_glyph: G) -> Option<String>
where
    C: FnMut(&str) -> bool,
    G: FnMut(&str) -> bool,
{
    for f in cands {
        if !is_cjk(f) {
            continue;
        }
        if has_glyph(f) {
            return Some((*f).to_string());
        }
    }
    None
}

// ----------------------------------------------------------------------
// 缓存
// ----------------------------------------------------------------------

struct Cache {
    /// 缓存属于哪一组（换字体 / 换简繁 → 整批作废）。
    face: Option<(FontChoice, Script, u8)>,
    /// 该组下「字 → 归属」。
    picks: HashMap<char, GlyphPick>,
    /// 某支字体有没有某个字：`(FontId, char)`。字体与字都是稳定的，
    /// 故这张表永不失效（FontId 本身随注册表变化，天然分代）。
    cover: HashMap<(usize, char), bool>,
    /// 某支字体是不是中文字库：`FontId`。
    cjk: HashMap<usize, bool>,
    /// 已经报过 Warning 的字（同一个字只报一行，免得刷屏）。
    warned: HashSet<char>,
}

fn cache() -> MutexGuard<'static, Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(Cache {
            face: None,
            picks: HashMap::new(),
            cover: HashMap::new(),
            cjk: HashMap::new(),
            warned: HashSet::new(),
        })
    })
    .lock()
    .unwrap_or_else(|e| e.into_inner())
}

/// 已经遇到过的缺字数（调试 / 诊断用；UI 不必展示）。
pub fn missing_count() -> usize {
    cache().warned.len()
}

/// 取一支字体的 `FontId`。
///
/// **两道保险**：
///   ① 名字必须真的出现在系统族名表里（`fonts::is_installed`）——`resolve_font`
///      在「族名不存在、兜底链也全挂」时会 **panic**，一条写错的族名足以把
///      整个 App 带走（`fonts.rs` 里那条「族名错一字 GPUI 静默回退」的老坑
///      的恶性版本）；
///   ② 即便如此仍包一层 `catch_unwind`：字体文件损坏时底层可能直接炸，
///      这里兜住它，让这个字退化成占位符，而不是让窗口消失。
///
/// `pub(crate)`：`mark_align`（标点视觉居中）也要量同一支字体的字形框。
pub(crate) fn font_id_of(cx: &App, family: &str) -> Option<FontId> {
    if !crate::fonts::is_installed(&[family]) {
        return None;
    }
    let ts = cx.text_system();
    // 族名先收成owned（`SharedString::from(&str)` 要求 `'static`——传进来的
    // 名字来自回退链，短命，必须复制一份）。
    let fam: SharedString = SharedString::from(family.to_string());
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ts.resolve_font(&gpui::font(fam))
    }))
    .ok()
}

/// 这支字体有没有这个字（走 GPUI 自己的 glyph 查找——`font-kit` 在缺字时
/// 返回 `None`，`advance` 随之报错，故 `is_ok()` 就是「有」）。
fn covers(cx: &App, family: &str, ch: char) -> bool {
    let Some(id) = font_id_of(cx, family) else {
        return false;
    };
    if let Some(v) = cache().cover.get(&(id.0, ch)) {
        return *v;
    }
    // 借 `text_system` 期间不能持锁（底层自己也有锁，别嵌套）。
    let ts = cx.text_system();
    let found = ts.advance(id, px(PROBE_PX), ch).is_ok();
    cache().cover.insert((id.0, ch), found);
    found
}

/// 这支字体是不是中文字库（认识「漢」）。
fn is_cjk_font(cx: &App, family: &str) -> bool {
    let Some(id) = font_id_of(cx, family) else {
        return false;
    };
    if let Some(v) = cache().cjk.get(&id.0) {
        return *v;
    }
    let ts = cx.text_system();
    let ok = ts.advance(id, px(PROBE_PX), CJK_PROBE).is_ok();
    cache().cjk.insert(id.0, ok);
    ok
}

/// 打一行 Warning（同一个字只报一次；不同字限流，免得「缺 3000 字」刷屏）。
fn warn_missing(ch: char) {
    let mut c = cache();
    if !c.warned.insert(ch) {
        return;
    }
    let n = c.warned.len();
    // 前 20 个逐条报，之后每 100 个报一次——排障够用，日志不爆。
    if n <= 20 || n % 100 == 0 {
        eprintln!(
            "[笺读][字体] 缺字 U+{:04X}「{}」：首选字体与系统 CJK 字库均无此字，已画占位符「{}」（累计第 {} 个）。\
如需显示，可把覆盖 CJK 扩展区的字库（如花园明朝 / 思源宋体全字集）拖进 fonts/ 目录。",
            ch as u32, ch, PLACEHOLDER, n
        );
    }
}

fn primary_for(choice: FontChoice, script: Script, face: Face) -> SharedString {
    match face {
        Face::Body => crate::fonts::family_for(choice, script),
        Face::Note => crate::fonts::note_family(choice),
    }
}

fn pick_face(
    choice: FontChoice,
    script: Script,
    face: Face,
    ch: char,
    cx: &App,
) -> GlyphPick {
    // 空白 / 控制字符不探：它们不落墨，也不该因为「字体里没有 0x20」
    // 就被判成缺字。
    if ch.is_whitespace() || ch.is_control() {
        return GlyphPick {
            family: primary_for(choice, script, face),
            state: PickState::Primary,
        };
    }
    let key = (choice, script, face.id());
    {
        let c = cache();
        if c.face == Some(key) {
            if let Some(p) = c.picks.get(&ch) {
                return p.clone();
            }
        }
    }
    let primary = primary_for(choice, script, face);
    let picked = if covers(cx, primary.as_ref(), ch) {
        GlyphPick {
            family: primary.clone(),
            state: PickState::Primary,
        }
    } else {
        let cands = candidate_chain(choice, script, face);
        match choose_covering(
            &cands,
            |f| is_cjk_font(cx, f),
            |f| covers(cx, f, ch),
        ) {
            Some(f) => GlyphPick {
                family: SharedString::from(f),
                state: PickState::Fallback,
            },
            None => {
                warn_missing(ch);
                GlyphPick {
                    // 占位符用首选字体画：首选字体再缺字也有「□」。
                    family: primary.clone(),
                    state: PickState::Missing,
                }
            }
        }
    };
    let mut c = cache();
    if c.face != Some(key) {
        c.face = Some(key);
        c.picks.clear();
    }
    c.picks.insert(ch, picked.clone());
    picked
}

/// **正文**某个字该用哪支字体。
pub fn body(choice: FontChoice, script: Script, ch: char, cx: &App) -> GlyphPick {
    pick_face(choice, script, Face::Body, ch, cx)
}

/// **批注 / 夹注**某个字该用哪支字体（恒定古文链：楷体家族）。
pub fn note(choice: FontChoice, ch: char, cx: &App) -> GlyphPick {
    pick_face(choice, Script::Ancient, Face::Note, ch, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_families_never_enter_the_fallback_chain() {
        // `serif` 落到哪支字体由平台定，可能是西文衬线——不许进回退链。
        for g in ["serif", "sans-serif", "monospace", "system-ui"] {
            assert!(is_generic_family(g), "{g} 应判为通用族");
        }
        for c in [FontChoice::WenJinMincho, FontChoice::LXGWWenKai] {
            let chain = candidate_chain(c, Script::Ancient, Face::Body);
            assert!(
                !chain.iter().any(|f| is_generic_family(f)),
                "{} 的回退链里漏进了通用族：{chain:?}",
                c.label()
            );
        }
    }

    #[test]
    fn fallback_chain_always_ends_with_a_system_cjk_face() {
        // 无论选了哪支字体，链尾都必须有系统中文宋体 / 黑体兜底，
        // 否则「首选没装」时整页就是豆腐块。
        for c in FontChoice::CLASSIC {
            let chain = candidate_chain(c, Script::Ancient, Face::Body);
            assert!(
                chain.iter().any(|f| f.contains("Songti") || f.contains("Source Han")),
                "{} 缺系统 CJK 兜底：{chain:?}",
                c.label()
            );
        }
    }

    #[test]
    fn latin_only_font_is_rejected_even_if_it_has_the_glyph() {
        // 关键判据：一支「不是中文字库」的字体，哪怕它自称有这个字，
        // 也绝不许被选中——这就是「绝不回退到英文字体」。
        let cands = ["CMU Serif", "Songti SC"];
        let got = choose_covering(
            &cands,
            |f| f != "CMU Serif",   // CMU 不认识「漢」
            |_| true,               // 两支都说自己有这个字
        );
        assert_eq!(got.as_deref(), Some("Songti SC"));
    }

    #[test]
    fn cjk_font_without_the_glyph_is_skipped() {
        // 中文字库也可能缺扩展区的字：认识「漢」不等于有这个字。
        let cands = ["Songti SC", "Source Han Serif SC"];
        let got = choose_covering(
            &cands,
            |_| true,
            |f| f == "Source Han Serif SC", // 只有大字库有
        );
        assert_eq!(got.as_deref(), Some("Source Han Serif SC"));
    }

    #[test]
    fn no_cjk_font_covers_means_missing() {
        // 全系统都无解 → None（调用方据此画占位符 + 打 Warning）。
        let cands = ["Songti SC", "PingFang SC"];
        assert_eq!(
            choose_covering(&cands, |_| true, |_| false),
            None,
            "无人覆盖时应返回 None，交给占位符"
        );
    }

    #[test]
    fn placeholder_is_a_geometric_square_not_tofu() {
        // 占位符刻意选几何方块：豆腐块与「字体没装好」长得一样，用户分不清。
        assert_eq!(PLACEHOLDER, '□');
        assert_eq!(PLACEHOLDER as u32, 0x25A1);
    }

    #[test]
    fn lock_is_required_for_every_non_primary_pick() {
        // 网格保护：凡是回退 / 缺字的字，字格必须显式锁宽高。
        let p = GlyphPick {
            family: SharedString::from("Songti SC"),
            state: PickState::Primary,
        };
        assert!(!p.needs_lock(), "首选字体沿用网格原字框，不必额外包裹");
        for st in [PickState::Fallback, PickState::Missing] {
            let p = GlyphPick {
                family: SharedString::from("Songti SC"),
                state: st,
            };
            assert!(p.needs_lock(), "{st:?} 必须锁字框");
            assert_eq!(p.is_missing(), st == PickState::Missing);
        }
    }
}
