//! 注音几何（v1.9.54 定稿）：**汉字纹丝不动、拼音做小巧外挂**。
//!
//! ## 为什么废弃 v1.9.53 的「主字让位」
//!
//! v1.9.53 让拼音占住字格顶部、主字下移让位（`char_top` / `char_h` / `char_fs`）。
//! 用户一眼看出问题：**带拼音的字被挤下去了，整列基线不齐**。这是致命的——
//! 竖排一列读的是「一条线」，任何一个字下沉半格，列气就断了。
//!
//! ## v1.9.54 的铁律（用户原话）
//!
//! 1. **绝对锁定正文坐标**：主字**永远**在字格的正中心、**永远**是正文字号、
//!    行盒**永远**等于格高。有注音没注音，主字一个像素都不许动。
//!    —— 本文件的结构体里**没有任何主字字段**，这是结构性保证，不是口头承诺。
//! 2. **拼音叠加（Overlay）**：拼音是贴在字格**顶端**的一枚小标签，浮在字面
//!    上方 13%~17% 的天然空当里，**零占正文空间**。
//! 3. **极致降噪**：拼音字号 ≤ 主字 **25%**，浅灰 `RUBY_INK`，无点无下划线。
//! 4. **绝不横向溢出**：整串宽 ≤ 格宽 86%，水平居中，绝不压左右相邻列。
//!
//! ## 几何（竖排字格是「宽而扁」的：12 列 55.5×27.75、字号 23.6）
//!
//! ```text
//! ┌──────────────┐  ← 字格顶（top = 0）
//! │    m ě i     │  band_h = 30% 格高（裁剪框，标签永不越此框）
//! │ ┌──────────┐ │  ← 主字墨迹顶 ≈ 13.4% 格高（居中后天然空当）
//! │ │    猛    │ │  主字：格心、正文号、行盒 = 格高 —— 恒不变
//! │ └──────────┘ │
//! └──────────────┘  ← 字格底
//! ```
//!
//! 竖排里 `格高 = 1.176 × 字号`，主字墨高约 `0.86 × 字号`，居中后顶上只剩
//! `0.134 × 格高`（12 列 ≈ 3.7px）的空当。拼音墨高 = `0.25 × 字号 × 0.78`
//! ≈ `0.166 × 格高`，比空当高出约 1px；屏幕上行盒的半个行距还会先吃掉
//! 约 0.17×拼音字号（12 列 ≈ 1px），故**实际压边约 2~3px（≈11% 格高）**。
//!
//! 这 2~3px 是几何硬账，也是用户明确许可的：「至多允许顶端轻微溢出」。
//! 为什么绝不到上一字：上一字的墨迹**底**离它自己的格底还有 13.4% 的空当，
//! 而标签只从**本格顶**往下长——两头差着一个完整字格的 13%，永远接不上。
//! 视觉上它是一抹 25% 大小的浅灰（`RUBY_INK`、90% 不透明），压在字头最上
//! 一线，远看只是「这个字有注」的记号；要看清读音，读器里有注音卡片。
//!
//! ⚠️ 屏幕（`reader::ancient_cell`）与 PDF（`pdf::draw_ruby`）共用本文件的度量，
//! 逐字同位；**不许各自再算一套**。

/// 拼音字号上限 = 主字的 **25%**（用户铁律：小巧外挂，浮在字面上）。
///
/// 12 列时约 5.9px——远看只是一抹灰痕，不抢正文；要看清读音，读器里有
/// 「注音卡片」（点字弹出的那张，正文字号排）承担可读性。
pub const RUBY_PINYIN_MAX: f32 = 0.25;
/// 拼音裁剪框高 = 格高的 **30%**。标签只许活在这个框里，框外什么都不画。
///
/// 取 30% 而不是「拼音墨高 + 1px」，是为了保证屏幕 `line_height` 不小于
/// 拼音字号——行盒若小于字号，字体的 ascent 会顶出盒子被 `overflow_hidden`
/// 削掉一笔（声调符号最先受害）。
pub const RUBY_BAND_FRAC: f32 = 0.30;
/// 一行拉丁字母的**墨迹高 / 字号**：含声调符（`ě` `ā`）与下行字母（`g` `y`）。
/// 取 0.78 是「小写 + 声调」的稳妥上界。
pub const RUBY_INK_OF_EM: f32 = 0.78;
/// 拼音整串宽度上限 = 字格宽的 **86%**：两侧各留 7%，绝不压到相邻列。
pub const RUBY_WIDTH_FRAC: f32 = 0.86;
/// 拼音字号的物理下限（像素）：极端窄格里的兜底，再由裁剪带切掉溢出部分。
pub const RUBY_MIN_FS: f32 = 2.5;
/// 字与字之间的额外字距（以字号为 1em 的比例）：极简，一丝不糊在一起。
pub const RUBY_TRACKING: f32 = 0.02;
/// 拼音墨色：**浅灰**（降噪，不与朱砂句读抢眼）。
pub const RUBY_INK: u32 = 0x8C877E;

/// 一条注音（拼音 / CJK 小字注记）的**外挂标签**度量。
///
/// 长度单位都是屏幕逻辑像素，坐标**相对字格左上角**。
///
/// ⚠️ 这里**故意不含** `char_fs` / `char_top` / `char_h`：主字的位置与字号
/// 由排版本身决定（格心、正文号、行盒 = 格高），注音无权干涉。测试
/// `ruby_layout_carries_no_character_geometry` 会穷举解构所有字段，
/// 谁想把「主字让位」加回来，谁就先编译不过。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyLayout {
    /// 拼音字号（≤ 主字 25%）。
    pub fs: f32,
    /// 标签裁剪框高（= 30% 格高），从格顶往下量。
    pub band_h: f32,
    /// 整串的估算宽度（各字母前进宽之和，≤ 86% 格宽）。
    pub block_w: f32,
    /// 标签左缘（水平居中于字格）。
    pub left: f32,
    /// 标签上缘（恒为 0：悬挂在字格顶端）。
    pub top: f32,
}

impl RubyLayout {
    /// 拼音墨迹高度（实际占的高度，≤ `band_h`）。
    pub fn ink_h(&self) -> f32 {
        (self.fs * RUBY_INK_OF_EM).min(self.band_h)
    }

    /// 第 `i` 个字母的**左缘**相对标签左缘的偏移（PDF 逐字落笔用）。
    pub fn glyph_dx(&self, text: &str, i: usize) -> f32 {
        let mut acc = 0.0f32;
        for (k, c) in text.chars().enumerate() {
            if k >= i {
                break;
            }
            acc += advance_em(c) + RUBY_TRACKING;
        }
        acc * self.fs
    }
}

/// 单字母的**前进宽度**（以字号为 1em 的比例）。
///
/// 核心 crate 里没有字体度量（不许依赖 gpui / CoreText），故用一张按字形
/// 归类的表：窄字母（`i l j t f r`）≈0.32em、宽字母（`m w`）≈0.86em、
/// 其余小写 ≈0.56em、大写 ≈0.70em、带调元音按普通小写算、CJK 满格 1.0em。
/// 误差几个百分点，而 `RUBY_WIDTH_FRAC = 0.86` 留出的余量足够吸收。
pub fn advance_em(c: char) -> f32 {
    match c {
        'i' | 'l' | 'j' | 't' | 'f' | 'r' | 'I' | '.' | '\'' | ' ' => 0.32,
        'm' | 'w' | 'M' | 'W' => 0.86,
        'a'..='z' => 0.56,
        'A'..='Z' => 0.70,
        // CJK 注记（「四行本」这类）是全宽字，必须按 1em 算——
        // 否则 PDF 逐字落笔会把它挤成一团（屏幕走真字体排版，看不出来）。
        _ if is_cjk(c) => 1.0,
        _ => 0.58, // 带调元音（ā á ǎ à …）、其他
    }
}

/// 是否是 CJK 汉字（含扩展 A / 兼容区 / 扩展 B 以上）。
fn is_cjk(c: char) -> bool {
    let u = c as u32;
    (0x2E80..=0x9FFF).contains(&u)          // 部首补充 / 康熙部首 / 统一表意
        || (0xF900..=0xFAFF).contains(&u)   // 兼容表意
        || (0x20000..=0x3FFFF).contains(&u) // 扩展 B~H
        || (0x3000..=0x303F).contains(&u)   // CJK 标点
}

/// 整串的宽度（以字号为 1em 的比例），含字距。
pub fn text_width_em(text: &str) -> f32 {
    let n = text.chars().count();
    if n == 0 {
        return 0.0;
    }
    let w: f32 = text.chars().map(advance_em).sum();
    w + RUBY_TRACKING * (n.saturating_sub(1)) as f32
}

/// 计算一条注音的**外挂标签**度量。
///
/// * `text`：注音串（`měi` / `四行本` …）；
/// * `body_fs`：正文字号（**只读**，本函数绝不返回任何主字几何）；
/// * `cell_w` / `cell_h`：字格宽高。
///
/// 约束链是纯 `min()`：
/// `fs = min(25% 正文, 按 86% 格宽, 按 30% 格高)`，再夹物理下限；
/// 宽度再兜一次硬顶格宽。所有约束都是不等式，故结果天然满足
/// 「不超宽、不出带、不压邻列」。
pub fn layout(text: &str, body_fs: f32, cell_w: f32, cell_h: f32) -> RubyLayout {
    let w_em = text_width_em(text).max(0.05);

    // ── ① 裁剪框：30% 格高（标签的活动范围，主字一概不管）──────────────
    let band_h = (cell_h * RUBY_BAND_FRAC).max(1.0).min(cell_h);

    // ── ② 字号 = min(25% 正文, 按 86% 格宽, 按框高) ────────────────────
    let by_width = (cell_w * RUBY_WIDTH_FRAC) / w_em;
    let by_band = band_h / RUBY_INK_OF_EM;
    let fs = (body_fs * RUBY_PINYIN_MAX)
        .min(by_width)
        .min(by_band)
        .max(RUBY_MIN_FS)
        .min(by_width.max(RUBY_MIN_FS)) // 再兜一次：绝不超过 86% 格宽
        .min(by_band.max(RUBY_MIN_FS)); // 再兜一次：绝不越出裁剪框

    // ── ③ 水平居中（用户：拼音与汉字同轴）──────────────────────────
    let block_w = (fs * w_em).min(cell_w);
    let left = ((cell_w - block_w) * 0.5).max(0.0);

    RubyLayout {
        fs,
        band_h,
        block_w,
        left,
        top: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{compute_grid, GridConfig, GridDir};

    /// 主字墨高 / 字号的保守比例：竖排格高 = 1.176×字号，字面居中后
    /// 格顶到字面顶的空当 = `(cell_h − 0.86×fs) / 2`。
    const CHAR_INK_OF_EM: f32 = 0.86;

    /// 8 / 10 / 12 / 14 列的真实几何（宽屏 1320×880，与 `grid` 测试同源）。
    fn cell(cols: usize) -> (f32, f32, f32) {
        let m = compute_grid(
            1320.0,
            880.0 - 78.0,
            GridConfig {
                columns: cols,
                dir: GridDir::Vertical,
                margin: 0.04,
                line_scale: 1.0,
                fs_ratio: 0.75,
                line_ratio: 1.0 / 0.75,
                target_chars: crate::grid::LayoutConfig::by_columns(cols)
                    .map(|c| c.target_chars)
                    .unwrap_or(21),
                banxin_ratio: crate::grid::BANXIN_RATIOS[crate::grid::BANXIN_TIER_DEFAULT],
            },
            false,
            1.0,
            50.0,
            0.0,
            12.0,
            0.0,
            crate::grid::GRID_FS_MAX,
        );
        (m.col_w, m.row_h, m.fs)
    }

    /// **契约测试**：主字几何绝不许被注音影响。
    ///
    /// 穷举解构 `RubyLayout` 的每一个字段——一旦有人把 v1.9.53 的
    /// `char_fs` / `char_top` / `char_h`（「主字让位」）加回来，这里立即编译失败。
    /// 「整列基线不齐」那次事故，从类型上就再也不可能发生。
    #[test]
    fn ruby_layout_carries_no_character_geometry() {
        let l = layout("měi", 24.0, 56.0, 28.0);
        let RubyLayout {
            fs: _,
            band_h: _,
            block_w: _,
            left: _,
            top: _,
        } = l;
    }

    /// 拼音必须悬挂在字格顶端：`top == 0`，裁剪框 ≤ 30% 格高。
    #[test]
    fn pinyin_hangs_from_the_cell_top() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["měi", "yǐ", "cāng", "zhuang"] {
                let l = layout(py, fs, cw, chh);
                assert_eq!(l.top, 0.0, "{cols} 列 / {py}：标签必须贴字格顶");
                assert!(
                    l.band_h <= chh * RUBY_BAND_FRAC + 0.01,
                    "{cols} 列 / {py}：裁剪框 {:.2} 超 30% 格高（{:.2}）",
                    l.band_h,
                    chh * RUBY_BAND_FRAC
                );
                assert!(l.ink_h() <= l.band_h + 1e-3, "{cols} 列：墨高越出裁剪框");
            }
        }
    }

    /// 字号 ≤ 主字 25%（用户铁律：极致降噪）。
    #[test]
    fn pinyin_never_exceeds_25pct_of_body() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["měi", "yǐ", "zhǎng", "zhuāng", "chuáng", "四行本"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.fs <= fs * RUBY_PINYIN_MAX + 0.01,
                    "{cols} 列 / {py}：拼音字号 {:.2} 超主字 25%（{:.2}）",
                    l.fs,
                    fs * RUBY_PINYIN_MAX
                );
                assert!(l.fs >= RUBY_MIN_FS - 1e-3, "{cols} 列 / {py}：低于物理下限");
            }
        }
    }

    /// 拼音墨高只许**微微**超出血缘净空（≤1.2px），否则整块压死字头。
    ///
    /// 这是「主字既不下移、拼音又要看得见」的几何答卷：格高 1.176×字号，
    /// 主字墨高 0.86×字号，顶上净空打对折只剩 13.4% 格高；拼音墨高
    /// 0.25×0.78 = 0.195×字号（16.6% 格高），故只许溢出一点点。
    /// （屏幕上还要再吃 0.17×拼音字号的半个行距，故实际压边 2~3px——
    /// 见文件头注释，那是渲染器的事，不是这里的度量。）
    #[test]
    fn pinyin_ink_never_exceeds_the_headroom_by_much() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            let headroom = ((chh - fs * CHAR_INK_OF_EM) * 0.5).max(0.0);
            for py in ["měi", "yǐ", "cāng"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.ink_h() <= headroom + 1.2,
                    "{cols} 列 / {py}：拼音墨高 {:.2} 压过字面 {:+.2}px（净空 {:.2}）",
                    l.ink_h(),
                    l.ink_h() - headroom,
                    headroom
                );
            }
        }
    }

    /// 绝不横向溢出：整串 ≤ 86% 格宽，居中后左右都不越界（不压邻列）。
    #[test]
    fn pinyin_never_bleeds_sideways() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["měi", "zhuāng", "zhuāngzhòng", "四行本", "双行夹注"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.block_w <= cw * RUBY_WIDTH_FRAC + 0.01,
                    "{cols} 列 / {py}：整串宽 {:.2} 超格宽 86%（{:.2}）",
                    l.block_w,
                    cw * RUBY_WIDTH_FRAC
                );
                assert!(l.left >= -1e-3, "{cols} 列 / {py}：左缘越界");
                assert!(
                    l.left + l.block_w <= cw + 0.01,
                    "{cols} 列 / {py}：右缘越界（{:.2} > {:.2}）",
                    l.left + l.block_w,
                    cw
                );
            }
        }
    }

    /// 拼音与汉字同轴：左缘恰为 `(格宽 − 整串宽) / 2`。
    #[test]
    fn pinyin_is_centered_over_the_char() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            let l = layout("cāng", fs, cw, chh);
            let want = (cw - l.block_w) * 0.5;
            assert!(
                (l.left - want).abs() < 0.01,
                "{cols} 列：未居中（left {:.2}，应为 {:.2}）",
                l.left,
                want
            );
        }
    }

    /// 长注记**按宽自动缩**：字号随串长单调不增，长到溢宽必缩，且绝不破 86%。
    ///
    /// ⚠️ 25% 上限极低（≈每格只给 2.3 个字母的宽度），所以只有 8em 以上的
    /// 长注记才会触发宽度约束——这正说明「短拼音吃满 25%、长注记按宽收」
    /// 的分工是对的，别把宽度测试写在短串上（那会误报「没缩」）。
    #[test]
    fn long_note_shrinks_by_width() {
        let (cw, chh, fs) = cell(12);
        let mut prev = f32::MAX;
        for t in ["měi", "zhuāng", "chuángshàng", "双行夹注示例之又一"] {
            let l = layout(t, fs, cw, chh);
            assert!(
                l.fs <= prev + 1e-3,
                "{t}：字号反而变大（{:.2} > {:.2}）",
                l.fs,
                prev
            );
            assert!(
                l.block_w <= cw * RUBY_WIDTH_FRAC + 0.01,
                "{t}：整串宽 {:.2} 超 86% 格宽（{:.2}）",
                l.block_w,
                cw * RUBY_WIDTH_FRAC
            );
            prev = l.fs;
        }
        // 九枚全宽字的夹注（9em）必然越过宽度上限，故必须真的缩下来。
        let short = layout("měi", fs, cw, chh);
        let long = layout("双行夹注示例之又一", fs, cw, chh);
        assert!(
            long.fs < short.fs - 0.01,
            "长注记未缩（长 {:.2} vs 短 {:.2}）",
            long.fs,
            short.fs
        );
        assert!(long.fs >= RUBY_MIN_FS - 1e-3);
        // 缩到刚好贴住 86% 格宽（宽度约束真的在起作用，不是碰巧）。
        assert!((long.block_w - cw * RUBY_WIDTH_FRAC).abs() < 0.6);
    }

    /// **10 列专项**（用户点名要同步的那一档）：宽 61.00 / 高 30.50 / 字号 25.92。
    ///
    /// 10 列的字格比 12 列宽、比 8 列窄，是「拼音能不能挤进净空」的临界档，
    /// 单独钉死它的四个数，免得日后调 `RUBY_*` 常量时只测 12 列。
    #[test]
    fn ten_columns_is_covered() {
        let (cw, chh, fs) = cell(10);
        assert!((cw - 61.0).abs() < 0.6, "10 列格宽实测 {cw:.2}，预期 ≈61.00");
        assert!((chh - 30.5).abs() < 0.6, "10 列格高实测 {chh:.2}，预期 ≈30.50");
        assert!((fs - 25.92).abs() < 0.6, "10 列字号实测 {fs:.2}，预期 ≈25.92");

        let l = layout("měi", fs, cw, chh);
        // 25% 上限是关键约束：10 列下应当正好吃到 25%。
        assert!(
            (l.fs - fs * RUBY_PINYIN_MAX).abs() < 0.05,
            "10 列：拼音字号 {:.2} 未吃到 25% 上限 {:.2}",
            l.fs,
            fs * RUBY_PINYIN_MAX
        );
        assert_eq!(l.top, 0.0);
        assert!((l.band_h - chh * RUBY_BAND_FRAC).abs() < 0.01);
        assert!(l.block_w < cw * 0.5, "10 列：三字母拼音不该占掉半格宽");
        assert!((l.left - (cw - l.block_w) * 0.5).abs() < 0.01);
        // 墨迹压在净空里（10 列净空 ≈ (30.5 − 22.3)/2 ≈ 4.1px）。
        let headroom = (chh - fs * CHAR_INK_OF_EM) * 0.5;
        assert!(l.ink_h() <= headroom + 1.2, "10 列：拼音压进字面太深");
    }

    /// CJK 注记（如「四行本」）同样走顶端外挂，且按 1em/字 计宽（不挤成一团）。
    #[test]
    fn cjk_note_also_hangs_from_the_top() {
        let (cw, chh, fs) = cell(12);
        let l = layout("四行本", fs, cw, chh);
        assert_eq!(l.top, 0.0);
        assert!(l.block_w <= cw * RUBY_WIDTH_FRAC + 0.01, "12 列：注记超宽");
        // 三枚全宽字 = 3em：估宽必须 ≈ 3×字号，否则 PDF 会叠字。
        assert!(
            (l.block_w - 3.0 * l.fs).abs() < l.fs * 0.2,
            "CJK 注记估宽 {:.2} 偏离 3em（{:.2}）",
            l.block_w,
            3.0 * l.fs
        );
    }

    /// 逐字左缘单调右移（PDF 依此落笔，绝不回头叠字）。
    #[test]
    fn glyph_dx_increases_monotonically() {
        let (cw, chh, fs) = cell(12);
        let l = layout("chuáng", fs, cw, chh);
        let mut prev = -1.0;
        for i in 0..6 {
            let dx = l.glyph_dx("chuáng", i);
            assert!(dx > prev, "第 {i} 字的左缘未右移");
            prev = dx;
        }
    }
}
