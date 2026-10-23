//! 句读（标点）排版：古籍里标点**极小**、**偏居一格之右下**、色淡而轻。
//!
//! 刻本的句读是「小字偏居一角」的记号，绝不与正文争方格中心：
//!
//! ```text
//!   ┌─────────┐          ┌─────────┐
//!   │         │          │         │
//!   │   道    │    →     │   道    │
//!   │         │          │       ○ │   ← 句号缩到 0.3 倍，墨点贴住格右缘
//!   │    ○    │          └─────────┘
//!   └─────────┘
//! ```
//!
//! * **字号** = 正文字号 × 0.3（[`FS_RATIO`]）——小到只是个记号；
//! * **起点** x = 字格宽 × 0.3、y = 字格高 × 0.3（[`DX_RATIO`] / [`DY_RATIO`]），
//!   标点因此紧贴格子的**右下角**，格心留给正文；
//! * **盒** = `[dx, 格宽] × [dy, 格高]`：自起点铺到格的右 / 下缘，字面在
//!   盒内**靠右下对齐**，再大也压不出格，绝不侵占邻格（竖排右邻是上一列，
//!   压过去就是串行）。
//! * **墨迹补偿**（[`INK_DX_RATIO`]）：仅靠右下对齐还不够——字体把「，。」
//!   的墨点放在 em box 的左下，em box 贴住格右缘时墨点仍在格右侧约五分之
//!   一处，看着像「格心偏下的一颗黑痣」。故把 em box 再右推 `0.7 × fs`，
//!   让**墨点**（不是 em box）正贴格右缘。
//!
//! > 为什么是「靠右下对齐」而不是「按起点画」：字体把句读的墨迹放在 em box
//! > 的左下（。，、）或正中（？！）。按起点画，。，、 的墨点会飘回格子中间
//! > ——旧版看着像「居中」就是这个缘故；靠右下对齐后两种情形的墨迹都落在
//! > 格子的右下角。
//!
//! 竖排里「右」是上一列那一侧、「下」是读序前进的方向，故右下角正是刻本
//! 句读的位置：它跟着读序走在前一字的**行进前方**贴边，不挡字、不占位。
//!
//! 另有一事：[`place`] 只管几何，不管颜色与字形；弯引号 → 直角引号的
//! 替换在 [`crate::types::bracket_to_quote`]（竖排开口方向才对），
//! 悬挂 / 挤压在 [`crate::gloss::layout_book_from_opts`]。

/// 标点字号 / 正文字号。
///
/// **0.3** —— 用户定的刻本尺度（v1.28.0 从 0.4 再收到 0.3）：句读只是一粒
/// 「朱点」，不是缩小的正字。0.4 在 12 列窄格里仍像一颗小黑豆，0.3 才真正
/// 退成记号；再小（0.25 以下）在 12px 正文上就糊成灰了。
pub const FS_RATIO: f32 = 0.3;
/// 起点横向偏移 / 字格宽：`x = 格左缘 + 格宽 × 0.3`。
pub const DX_RATIO: f32 = 0.30;
/// 起点纵向偏移 / 字格高：`y = 格上缘 + 格高 × 0.3`。
pub const DY_RATIO: f32 = 0.30;
/// **墨迹补偿·横**（以字号为单位）：把 em box 再往右推这么多个「字号宽」。
///
/// 字体把「，。」的墨点放在 em box 的**左下角**（横排坐标：约占 em box
/// 左侧 30% 宽、下侧 30% 高）。竖排渲染不旋转字形，所以即便把 em box
/// 右缘贴住格右缘，墨点仍落在格子右侧约五分之一处——看着就是「格心偏下
/// 的一颗黑痣」（v1.30.0 用户反馈）。
///
/// 把 em box 再右推 `0.70 × fs`（em box 余下的空白探出格右缘，墨点不探出），
/// 墨迹右缘才正贴格右缘（界栏那一侧）。
///
/// 推导：墨迹左缘 = em box 左缘、右缘 = em box 左缘 + 0.3·fs；em box 左缘
/// = 格右缘 − fs。故墨迹右缘 = 格右缘 − 0.7·fs，需右移 0.7·fs 才贴上界栏。
pub const INK_DX_RATIO: f32 = 0.70;
/// **墨迹补偿·纵**：「，。」的墨迹底缘本就贴着 em box 底缘，故纵向不用补。
pub const INK_DY_RATIO: f32 = 0.0;
/// 字号下限（px）：正文再小，句读也得看得见（低于 6px 就不是墨点了）。
pub const FS_MIN: f32 = 6.0;
/// 字号上限（px）：正文再大，句读也不许涨成一颗正字。
pub const FS_MAX: f32 = 22.0;

/// 书名号（`︽ ︾`）/ 正文字号：**0.7 倍**（v1.30.0 用户定；原 0.8）。
///
/// 书名号不是句读——它包着一整串书名，0.3 倍会小到看不出在「括」什么；
/// 但也不该与正文齐平（刻本的书名号总比正文矮一头），更不许撑满格子：
/// 它附在相邻字的格右上角（见 `GlyphKind::Attach`），70% 才既看得清
/// 又不抢正文的格心。
pub const TITLE_FS_RATIO: f32 = 0.7;

/// 一枚句读在字格里的落位（纯几何，渲染层照抄即可）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PunctBox {
    /// 字号（px）。
    pub fs: f32,
    /// 相对字格左上角的横向起点（px）。
    pub dx: f32,
    /// 相对字格左上角的纵向起点（px）。
    pub dy: f32,
    /// 标点盒宽（px）：自起点到格右缘。
    pub w: f32,
    /// 标点盒高（px）：自起点到格下缘。
    pub h: f32,
    /// em box 的**墨迹补偿·横**（px）：字面整体再右移这么多，
    /// 使墨点（而非 em box）贴住格右缘。见 [`INK_DX_RATIO`]。
    pub ink_dx: f32,
    /// em box 的**墨迹补偿·纵**（px）：「，。」纵向上本就贴底，恒为 0。
    pub ink_dy: f32,
}

/// 句读落位：**字号 × 0.3，起点偏移到字格右下角**。
///
/// ```text
/// fs     = clamp(正文 × 0.3, FS_MIN, FS_MAX)   再夹进格内剩余空间
/// dx     = 格宽 × 0.30                        （向右偏移）
/// dy     = 格高 × 0.30                        （向下偏移）
/// w      = 格宽 - dx      h = 格高 - dy
/// ink_dx = 0.70 × fs      ink_dy = 0          （墨迹补偿，见上）
/// ```
///
/// `fs` 最后还要 ≤ `min(w, h)`：即便正文大到离谱（或格子窄到离谱），
/// 标点也不会探出字格、压到邻列。极端小格（比 `FS_MIN` 还小）时随格缩，
/// 不硬撑出一个比格还大的点。
pub fn place(fs_body: f32, cell_w: f32, cell_h: f32) -> PunctBox {
    let dx = (cell_w * DX_RATIO).max(0.0);
    let dy = (cell_h * DY_RATIO).max(0.0);
    let w = (cell_w - dx).max(1.0);
    let h = (cell_h - dy).max(1.0);
    let room = w.min(h);
    let mut fs = fs_body * FS_RATIO;
    fs = fs.min(room);
    if room >= FS_MIN {
        fs = fs.max(FS_MIN);
    }
    fs = fs.min(FS_MAX);
    PunctBox {
        fs,
        dx,
        dy,
        w,
        h,
        ink_dx: fs * INK_DX_RATIO,
        ink_dy: fs * INK_DY_RATIO,
    }
}

/// 标点墨色：朱砂兑纸色——比正文轻，却仍是「句读」的朱意。
///
/// `k` 是朱砂占比：0.72 ≈ 淡朱（深色纸上不刺眼、浅色纸上不抢字）。
/// 用兑色而非 opacity，深浅两套主题都稳（不依赖底色透出来）。
pub const INK_MIX: f32 = 0.72;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_size_is_a_third_of_the_body() {
        // 常用字号段里严格是 0.3 倍（只是记号，不是缩小的正字）。
        for fs in [12.0, 17.0, 24.0, 34.0, 44.0] {
            let b = place(fs, fs * 1.2, fs * 1.3);
            assert!(
                (b.fs - (fs * FS_RATIO).max(FS_MIN)).abs() < 0.01,
                "字号应为 0.3 倍正文：{:.2} vs {:.2}",
                b.fs,
                fs * FS_RATIO
            );
        }
        // 句号约正字三成高：一眼是「点」不是「字」。
        let b = place(34.0, 40.0, 44.0);
        assert!(b.fs < 34.0 * 0.35, "句读仍偏大：{}", b.fs);
    }

    #[test]
    fn title_mark_is_a_separate_larger_step() {
        // 书名号 0.7 倍（v1.30.0）、句读 0.3 倍：两档拉开，书名号才括得住
        // 书名，又不至于撑满它依附的那一格。
        assert!((TITLE_FS_RATIO - 0.7).abs() < 1e-6);
        assert!(TITLE_FS_RATIO > FS_RATIO * 2.0);
        assert!(TITLE_FS_RATIO < 1.0, "书名号不该与正文齐平");
    }

    #[test]
    fn origin_is_offset_to_the_bottom_right_never_centered() {
        let b = place(34.0, 40.0, 44.0);
        assert!((b.dx - 40.0 * DX_RATIO).abs() < 0.01, "横向偏移错：{}", b.dx);
        assert!((b.dy - 44.0 * DY_RATIO).abs() < 0.01, "纵向偏移错：{}", b.dy);
        // 「绝不能占据格子的正中心」：起点明显偏在右下象限。
        assert!(b.dx > 40.0 * 0.2, "起点太靠左（近乎居中）：{}", b.dx);
        assert!(b.dy > 44.0 * 0.2, "起点太靠上（近乎居中）：{}", b.dy);
        assert!(b.dx < 40.0 * 0.5 && b.dy < 44.0 * 0.5, "偏移过头、贴到角外了");
    }

    #[test]
    fn punct_never_spills_out_of_its_cell() {
        // 任意字号 × 任意格：标点盒连同字面都留在格内，绝不压到邻列 / 邻行。
        for fs in [8.0, 12.0, 17.0, 24.0, 34.0, 60.0, 96.0] {
            for (cw, ch) in [(16.0, 18.0), (24.0, 26.0), (40.0, 44.0), (80.0, 88.0)] {
                let b = place(fs, cw, ch);
                assert!(b.dx >= 0.0 && b.dy >= 0.0, "起点出格：{b:?}");
                assert!(b.dx + b.w <= cw + 0.01, "盒右缘出格：{b:?} / 格宽 {cw}");
                assert!(b.dy + b.h <= ch + 0.01, "盒下缘出格：{b:?} / 格高 {ch}");
                assert!(b.fs <= b.w + 0.01 && b.fs <= b.h + 0.01, "字面比盒大：{b:?}");
            }
        }
    }

    #[test]
    fn the_ink_dot_not_the_em_box_touches_the_right_rule() {
        // v1.30.0 用户反馈「逗号句号像格心偏下的黑痣」的根因与修法：
        // 字体把 。， 的墨迹放在 em box 的左下（约占 em box 左侧 30% 宽），
        // 只把 em box 右缘贴格右缘，墨点仍在格右侧 0.7×fs 处。
        //
        // 补偿后：墨迹左缘 = em box 左缘 = 格右缘 − fs + 0.7·fs，
        //         墨迹右缘 = 墨迹左缘 + 0.3·fs = 格右缘 —— 正贴界栏。
        let b = place(34.0, 40.0, 44.0);
        let em_left = b.dx + b.ink_dx + b.w - b.fs; // 盒内右对齐后的 em box 左缘
        let ink_right = em_left + 0.3 * b.fs; // 墨迹只占 em box 左侧三成
        assert!(
            (ink_right - 40.0).abs() < 0.5,
            "墨迹右缘 {ink_right:.2} 未贴格右缘 40"
        );
        // 补偿量就是 0.7 个字号，不多不少。
        assert!((b.ink_dx - b.fs * INK_DX_RATIO).abs() < 1e-6);
        assert_eq!(b.ink_dy, 0.0, "纵向墨迹本就贴底，不该再补");
        // 各种格子下都成立（不能只在 40×44 这种格子上碰巧对）。
        for fs in [12.0, 17.0, 24.0, 34.0, 60.0] {
            let (cw, ch) = (fs / 0.75, fs / 0.75); // 引擎真实格子：格宽 = 字级/0.75
            let b = place(fs, cw, ch);
            let ink_right = b.dx + b.ink_dx + b.w - b.fs + 0.3 * b.fs;
            assert!(
                (ink_right - cw).abs() < 0.5,
                "fs={fs} 墨迹右缘 {ink_right:.2} 未贴格右缘 {cw:.2}"
            );
        }
    }

    /// 目检用：`cargo test dump_punct -- --ignored --nocapture`。
    ///
    /// 打印常用字号下句读的字号与落位——一眼看出「0.5 倍字号、贴在右下角」。
    /// 格子按引擎的真实算式给：`格宽 = 字号/0.75`、`格高 = 格宽 × 1.1`。
    #[test]
    #[ignore]
    fn dump_punct_box() {
        for fs in [12.0f32, 17.0, 24.0, 34.0, 60.0] {
            let cw = fs / crate::grid::GRID_FS_RATIO;
            let ch = cw * crate::grid::GRID_ROW_RATIO;
            let b = place(fs, cw, ch);
            println!(
                "正文 {fs:>5.1} → 句读 {:>5.2}（{:.3}×正文）| 格 {:>5.1}×{:>5.1} → 起点 ({:>5.2}, {:>5.2}) 盒 {:>5.2}×{:>5.2}",
                b.fs,
                b.fs / fs,
                cw,
                ch,
                b.dx,
                b.dy,
                b.w,
                b.h,
            );
        }
    }

    #[test]
    fn font_size_has_a_floor_and_a_ceiling() {
        // 正文极小：保底 FS_MIN，句读不至于消失。
        assert!((place(6.0, 40.0, 44.0).fs - FS_MIN).abs() < 0.01);
        // 正文极大：封顶 FS_MAX，句读不涨成一颗正字。
        assert!((place(400.0, 400.0, 400.0).fs - FS_MAX).abs() < 0.01);
        // 格子比下限还小：随格缩，不硬撑。
        let b = place(34.0, 8.0, 9.0);
        assert!(b.fs <= b.w.min(b.h) + 0.01, "小格里仍撑破：{b:?}");
    }
}
