//! **全角对称标点的视觉居中**（v1.9.26）。
//!
//! ## 问题
//!
//! 竖排里 `「」『』《》〈〉（）【】` 这些全角标点，肉眼看着「偏向一边」。
//! 根因不是渲染算错了，而是**两套坐标系错位**：
//!
//!   * 排版把字放在格心，靠的是**前进宽度**（advance）——一只全角字
//!     advance = 1em，格子按这个盒子居中；
//!   * 但竖排专用字形（`﹁﹂﹃﹄︽︾︵︶`，U+FE10–FE4F）在字库里是
//!     **贴右缘**设计的：竖排时标点长在字列的右侧，墨迹只占 em 方块的
//!     右半，左边半格是空的。
//!
//! 于是「盒子居中」= 墨迹偏右。差多少，取决于字体把墨迹画在哪儿——
//! 这个问题**没有通用常数可写**（文津、齐伋、系统宋体各不相同）。
//!
//! ## 解法
//!
//! 不猜数字，**量**：向字体系统要这枚字形的真实墨迹框
//! （`TextSystem::typographic_bounds`），算出「墨迹中心」与「前进盒子中心」
//! 的差，把标点按这个差横移——墨迹中心就落在格心。
//!
//!   ```text
//!   dx = advance/2 − (ink.x + ink.w/2)
//!   ```
//!
//! 对一枚已经居中的字形（比如汉字），`dx ≈ 0`，此式自动退化为「不动」。
//!
//! ## 三条边界（用户点名，逐条守住）
//!
//!   ① **只改落笔的 translate**：`dx` 只加在一个绝对定位的满格盒子 `left`
//!      上，元素的宽高仍是 `cell_w × cell_h`——**网格引擎
//!      （`calculate_grid`）一行都不碰**，字距、列宽、分页全不变。
//!   ② **句读照旧贴角**：`，、。；：` 这些是「读类」，古籍习惯是贴在字格
//!      右上方（见 `punct::place`），**永不居中**——[`is_corner_punct`] 把它
//!      们挡在外面，本模块对它们返回 0。
//!   ③ **纵向不硬挪**：竖排标点的纵向位置本身有体例含义（`﹁` 贴格上缘
//!      领起下文、`︾` 贴格下缘收束书名），硬挪到正中反而断了行气。纵向
//!      只做一件事：把行盒锁成格高（`line_height = cell_h`），让字形位置
//!      只由格高决定，与字体的 ascent/descent 无关。

use crate::font_fallback::font_id_of;
use gpui::{px, App};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// 量墨迹框用的字号（越大越准；结果按 em 比例缓存，与真实字号无关）。
const PROBE_PX: f32 = 64.0;

/// 横移量的安全上限（占前进宽度的比例）。任何字体、任何字形，都不许把
/// 标点推出字格——宁可不居中，也绝不许压到邻列的界行上。
const MAX_SHIFT_RATIO: f32 = 0.34;

/// 需要「视觉居中」的全角对称标点：竖排形与横排形都算（导入的稿子里
/// 两种写法都可能有，渲染前未必都换成了竖排形）。
pub fn is_centered_mark(ch: char) -> bool {
    matches!(
        ch,
        // 引号（横排形 / 竖排形）
        '「' | '」' | '『' | '』' | '﹁' | '﹂' | '﹃' | '﹄'
        // 书名号（横排形 / 竖排形）
        | '《' | '》' | '〈' | '〉' | '︽' | '︾' | '︿' | '﹀'
        // 括号（横排形 / 竖排形）
        | '（' | '）' | '︵' | '︶'
        // 方括号 / 六角括号
        | '〔' | '〕' | '【' | '】' | '［' | '］'
    )
}

/// **只贴角的句读**：古籍里它们跟着读序贴在字格右上（`punct::place`），
/// 绝不居中。本模块对它们一律返回 0 偏移——这是「逗号句号照旧贴角」的判据。
pub fn is_corner_punct(ch: char) -> bool {
    matches!(
        ch,
        '，' | '、' | '。'
            | '．'
            | '·'
            | '；'
            | '：'
            | '！'
            | '？'
            | '…'
            | '‥'
            | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '.'
    )
}

/// 由三个度量算横移量（**纯函数**，好单测也好看）：
///   * `advance_w` —— 前进宽度（盒子宽）；
///   * `ink_x` —— 墨迹左缘（相对落笔原点，含左边距）；
///   * `ink_w` —— 墨迹宽。
///
/// 返回正值 = 往右挪。
pub fn dx_from_metrics(advance_w: f32, ink_x: f32, ink_w: f32) -> f32 {
    if !(advance_w.is_finite() && ink_x.is_finite() && ink_w.is_finite()) || advance_w <= 0.0 {
        return 0.0;
    }
    let want = advance_w / 2.0 - (ink_x + ink_w / 2.0);
    want.clamp(-advance_w * MAX_SHIFT_RATIO, advance_w * MAX_SHIFT_RATIO)
}

/// 缓存：`(FontId, char) → dx / 字号`（em 比例，与字号无关）。
fn cache() -> MutexGuard<'static, HashMap<(usize, char), f32>> {
    static C: OnceLock<Mutex<HashMap<(usize, char), f32>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 这枚标点在**当前字体**下要横移多少 px，墨迹才落到格心。
///
/// 不走 `is_centered_mark` 的字（汉字、句读）直接返回 0——调用方因此可以
/// 无脑问一句，不必自己判类型。
pub fn center_dx(cx: &App, family: &str, ch: char, size: f32) -> f32 {
    if !is_centered_mark(ch) || size <= 0.0 {
        return 0.0;
    }
    let Some(id) = font_id_of(cx, family) else {
        return 0.0;
    };
    if let Some(r) = cache().get(&(id.0, ch)) {
        return r * size;
    }
    let ts = cx.text_system();
    // 缺字（扫描件 / 异体）时不给偏移：宁可不动，也别把一个本来就画不出的
    // 字形推到邻居格上。
    let (Ok(adv), Ok(ink)) = (
        ts.advance(id, px(PROBE_PX), ch),
        ts.typographic_bounds(id, px(PROBE_PX), ch),
    ) else {
        return 0.0;
    };
    let dx = dx_from_metrics(
        f32::from(adv.width),
        f32::from(ink.origin.x),
        f32::from(ink.size.width),
    );
    let ratio = dx / PROBE_PX;
    cache().insert((id.0, ch), ratio);
    ratio * size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_marks_cover_the_four_kinds_the_user_named() {
        // 用户点名的四类：书名号、单书名号、引号、括号（横排形与竖排形都收）。
        for ch in [
            '《', '》', '〈', '〉', '︽', '︾', '「', '」', '﹁', '﹂', '（', '）', '︵', '︶', '『',
            '』', '【', '】', '〔', '〕',
        ] {
            assert!(is_centered_mark(ch), "{ch} 应算「要居中的全角标点」");
        }
    }

    #[test]
    fn corner_punct_never_gets_centered() {
        // 铁律：逗号句号照旧贴在字格右上角——绝不能因为「居中」把句读挪到格心。
        for ch in ['，', '、', '。', '；', '：', '！', '？'] {
            assert!(is_corner_punct(ch), "{ch} 应算「贴角句读」");
            assert!(
                !is_centered_mark(ch),
                "{ch} 同时被判成居中，两套判据打架了"
            );
        }
    }

    #[test]
    fn already_centered_glyph_gets_no_shift() {
        // 汉字这类墨迹本就居中的字形：dx 必须是 0（此式对它们自然退化）。
        assert_eq!(dx_from_metrics(100.0, 5.0, 90.0), 0.0);
    }

    #[test]
    fn right_hugging_vertical_form_is_pulled_left() {
        // 竖排形的典型形态：墨迹只占右半边（x 从 50 到 100）。
        // 盒心 50、墨心 75 → 必须往左挪 25。
        let dx = dx_from_metrics(100.0, 50.0, 50.0);
        assert!((dx + 25.0).abs() < 0.01, "应左移 25，实得 {dx}");
    }

    #[test]
    fn left_hugging_glyph_is_pushed_right() {
        // 横排形（如 `「`）墨迹在左上：x 从 0 到 40，墨心 20、盒心 50 → 右移 30。
        let dx = dx_from_metrics(100.0, 0.0, 40.0);
        assert!((dx - 30.0).abs() < 0.01, "应右移 30，实得 {dx}");
    }

    #[test]
    fn absurd_metrics_are_clamped_into_the_cell() {
        // 字体报了个离谱的框（墨迹远在格右缘外）：也绝不许把字推出格子。
        let dx = dx_from_metrics(100.0, 500.0, 10.0);
        assert!(
            dx >= -100.0 * MAX_SHIFT_RATIO - 0.01,
            "左移越界：{dx}（上限 {MAX_SHIFT_RATIO} 格宽）"
        );
        let dx2 = dx_from_metrics(100.0, -500.0, 10.0);
        assert!(dx2 <= 100.0 * MAX_SHIFT_RATIO + 0.01, "右移越界：{dx2}");
    }

    #[test]
    fn bad_metrics_never_panic() {
        // 字体系统偶尔会吐 NaN / 0：绝不许让它变成布局里的 NaN（会糊掉整棵树）。
        for (a, x, w) in [
            (0.0_f32, 0.0, 0.0),
            (f32::NAN, 1.0, 1.0),
            (100.0, f32::INFINITY, 1.0),
            (-10.0, 0.0, 0.0),
        ] {
            assert_eq!(dx_from_metrics(a, x, w), 0.0, "({a},{x},{w}) 应返回 0");
        }
    }
}
