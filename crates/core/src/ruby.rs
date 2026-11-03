//! 注音几何（v1.9.56 定稿）：**汉字纹丝不动、拼音悬挂在字缝里**。
//!
//! ## 为什么废弃 v1.9.53 的「主字让位」
//!
//! v1.9.53 让拼音占住字格顶部、主字下移让位（`char_top` / `char_h` / `char_fs`）。
//! 用户一眼看出问题：**带拼音的字被挤下去了，整列基线不齐**。这是致命的——
//! 竖排一列读的是「一条线」，任何一个字下沉半格，列气就断了。
//!
//! ## 铁律（用户三次定稿后的口径）
//!
//! 1. **绝对锁定正文坐标**：主字**永远**在字格的正中心、**永远**是正文字号、
//!    行盒**永远**等于格高。有注音没注音，主字一个像素都不许动。
//!    —— 本文件的结构体里**没有任何主字字段**，这是结构性保证，不是口头承诺。
//! 2. **拼音悬挂（Hanging）**：标签骑在字格的上边框上——墨迹**一半在格里、
//!    一半在格外**，正好落在上一字与这一字的字缝正中间，**零占正文空间**。
//! 3. **极小**：字号 ≤ 主字 **26%**，**朱砂** `RUBY_INK`，无点无下划线。
//! 4. **绝不横向溢出**：整串宽 ≤ 格宽 86%，水平居中，绝不压左右相邻列。
//!
//! ## 几何（竖排字格是「宽而扁」的：12 列 55.5×27.75、字号 23.6）
//!
//! ```text
//! ── 上一字的墨迹底 ──────────────  −13.4% 格高（上一格的下空当）
//!        m ě i                     ← 拼音墨迹：骑在 y=0 上，上下各半
//! ── 本格顶（y = 0）──────────────
//!        ┌──────────┐             ← 主字墨迹顶 ≈ +13.4% 格高
//!        │    猛    │                主字：格心、正文号、行盒 = 格高
//!        └──────────┘
//! ── 本格底 ──────────────────────
//! ```
//!
//! 竖排 `格高 = 1.176 × 字号`、主字墨高 `0.86 × 字号`，故**上下两格的墨迹
//! 之间有一条 0.316×字号 的字缝**（12 列 ≈ 7.45px）——这条缝就是注音的
//! 唯一居所。标签把墨迹**居中放进这条缝**（`top = −fs/2` 使墨心中在格线上），
//! 上下各留 ≈1.3px，既不压上一字的墨，也不碰本字的头。
//!
//! 这就是用户要的「极小的字号 + 绝不与旁的点糊在一起」：拼音只活在格线上
//! 那一条 7px 的窄带里，句读红点在 0.72 格高处，中间隔着 60% 格高。
//!
//! ⚠️ 屏幕（`reader::ancient_cell`）与 PDF（`pdf::draw_ruby`）共用本文件的度量，
//! 逐字同位；**不许各自再算一套**。

/// 拼音字号上限 = 主字的 **26%**。
///
/// 25% → 30% → 26% 的三次取舍：30%（v1.9.55「稍微放大」）在真机上仍显挤，
/// 墨高 0.234×字号 把 0.316×字号 的字缝吃掉四分之三，只剩 1px 上下；
/// 26% 的墨高 0.203×字号，上下各留 1.3px 的净空，一眼看得出是「缝里的
/// 小字」而不是「压在字头上的重影」。**上限动的是「多大」，主字一概不参与**。
pub const RUBY_PINYIN_MAX: f32 = 0.26;
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
/// 拼音墨色：**古籍朱砂**（v1.9.55）。
///
/// 取的正是全书那一支朱砂（`theme::zhu()` 的 keben 档 `0xB0342C`）——
/// 与句读红点、朱印、朱丝栏同源，**不另起一个红**。比用户举例的
/// `#C0392B` 更沉一档、更近古墨，压在字头上不会跳成「亮红」。
pub const RUBY_INK: u32 = 0xB0342C;
/// 句读记号（红点 / 空心圈）的心在格内的**纵向位置**（0.72 格高，见 `punct`）。
///
/// 只用来做「朱砂拼音绝不与朱砂句读糊在一起」的避让判据：
/// 拼音墨迹整块骑在 y=0 的格线上（上沿 ≈ −4.8% 格高），离 0.72 那一带
/// 还差着 0.6 格高——两支朱砂一上一下，永远连不成一片。
pub const PUNCT_ZONE_Y: f32 = 0.72;

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
    /// 拼音字号（≤ 主字 30%）。
    pub fs: f32,
    /// 标签裁剪框高（= 30% 格高），从格顶往下量。
    pub band_h: f32,
    /// 整串的估算宽度（各字母前进宽之和，≤ 86% 格宽）。
    pub block_w: f32,
    /// 标签左缘（水平居中于字格）。
    pub left: f32,
    /// 标签**行盒上缘**（负值：悬挂到字格顶线之上）。
    ///
    /// 恒为 `−fs / 2`，于是墨迹（在行盒里居中）正好**骑在 y = 0 的格线上**，
    /// 上下各半，落在上一字与本字之间的那道字缝正中。
    pub top: f32,
}

impl RubyLayout {
    /// 拼音墨迹高度（实际占的高度，≤ `band_h`）。
    pub fn ink_h(&self) -> f32 {
        (self.fs * RUBY_INK_OF_EM).min(self.band_h)
    }

    /// 墨迹上缘（相对字格左上角；负值＝在字格之上）。
    ///
    /// 行盒高 = 拼音字号，墨迹在行盒里居中——屏幕（`line_height(fs)`）与
    /// PDF（`glyph_at` 在 `fs × fs` 的盒里居中落笔）**同一条口径**。
    pub fn ink_top(&self) -> f32 {
        self.top + (self.fs - self.ink_h()) * 0.5
    }

    /// 墨迹下缘（相对字格左上角）。
    pub fn ink_bottom(&self) -> f32 {
        self.ink_top() + self.ink_h()
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
/// `fs = min(26% 正文, 按 86% 格宽, 按 30% 格高)`，再夹物理下限；
/// 宽度再兜一次硬顶格宽。所有约束都是不等式，故结果天然满足
/// 「不超宽、不出带、不压邻列」。
pub fn layout(text: &str, body_fs: f32, cell_w: f32, cell_h: f32) -> RubyLayout {
    let w_em = text_width_em(text).max(0.05);

    // ── ① 裁剪框：30% 格高（标签的活动范围，主字一概不管）──────────────
    let band_h = (cell_h * RUBY_BAND_FRAC).max(1.0).min(cell_h);

    // ── ② 字号 = min(26% 正文, 按 86% 格宽, 按框高) ────────────────────
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

    // ── ④ 悬挂（v1.9.56）：行盒上缘抬到 −fs/2 ──────────────────────
    // 行盒高 = 字号、墨迹在盒里居中 ⇒ 墨心正好落在 `top + fs/2 = 0`，
    // 也就是**格线上**：一半在格里、一半在格外（上一字的下空当里）。
    // 上一字的墨底在 −13.4% 格高、本字的墨顶在 +13.4% 格高，墨高
    // 0.203×字号只占那条 0.316×字号 字缝的 64%，上下各剩 1.3px 净空。
    let top = -fs * 0.5;

    RubyLayout {
        fs,
        band_h,
        block_w,
        left,
        top,
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

    /// 拼音**悬挂**在字格顶线上：行盒上缘 `−fs/2`，墨心正落在 `y = 0`。
    ///
    /// 这是 v1.9.56 的核心几何：一半在格里、一半在格外（上一字的下空当），
    /// 所以既不吃正文的位置，也不压在字头上。
    #[test]
    fn pinyin_hangs_from_the_cell_top() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["měi", "yǐ", "cāng", "zhuang"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    (l.top + l.fs * 0.5).abs() < 1e-3,
                    "{cols} 列 / {py}：行盒上缘应为 −fs/2（{:.2}），实为 {:.2}",
                    -l.fs * 0.5,
                    l.top
                );
                // 墨心骑在格线上：上缘为负、下缘为正，且对称。
                assert!(l.ink_top() < 0.0, "{cols} 列 / {py}：墨迹没挂出格顶");
                assert!(l.ink_bottom() > 0.0, "{cols} 列 / {py}：墨迹整个跑到格顶之上");
                assert!(
                    (l.ink_top() + l.ink_bottom()).abs() < 1e-3,
                    "{cols} 列 / {py}：墨心没落在格线上（{:.2}）",
                    l.ink_top() + l.ink_bottom()
                );
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

    /// 字号 ≤ 主字 26%（v1.9.56 由 30% 收回，真机上仍显挤）。
    #[test]
    fn pinyin_never_exceeds_26pct_of_body() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["měi", "yǐ", "zhǎng", "zhuāng", "chuáng", "四行本"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.fs <= fs * RUBY_PINYIN_MAX + 0.01,
                    "{cols} 列 / {py}：拼音字号 {:.2} 超主字 26%（{:.2}）",
                    l.fs,
                    fs * RUBY_PINYIN_MAX
                );
                assert!(l.fs >= RUBY_MIN_FS - 1e-3, "{cols} 列 / {py}：低于物理下限");
            }
        }
    }

    /// **墨迹整块待在字缝里**：既碰不到上一字的墨底，也碰不到本字的墨顶。
    ///
    /// 竖排 `格高 = 1.176×字号`、主字墨高 `0.86×字号`，故相邻两字的墨迹
    /// 之间有一条 `0.316×字号` 的字缝（`±headroom`，12 列 ≈ ±3.7px）。
    /// 拼音墨迹必须整块落在这条缝内——这是「不与正文糊在一起」的硬判据，
    /// 比「压边不超过 N px」这种容差说法规矩得多。
    #[test]
    fn pinyin_ink_sits_inside_the_gap_between_chars() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            let headroom = ((chh - fs * CHAR_INK_OF_EM) * 0.5).max(0.0);
            for py in ["měi", "yǐ", "cāng", "zhuāng"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.ink_top() >= -headroom - 0.01,
                    "{cols} 列 / {py}：拼音上沿 {:.2} 越过上一字的墨底 {:.2}",
                    l.ink_top(),
                    -headroom
                );
                assert!(
                    l.ink_bottom() <= headroom + 0.01,
                    "{cols} 列 / {py}：拼音下沿 {:.2} 压进本字墨顶 {:.2}",
                    l.ink_bottom(),
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
    /// ⚠️ 30% 上限仍然很低（≈每格只给 2.7 个字母的宽度），所以只有 8em 以上的
    /// 长注记才会触发宽度约束——这正说明「短拼音吃满 30%、长注记按宽收」
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
        // 26% 上限是关键约束：10 列下应当正好吃到 26%（6.74px）。
        assert!(
            (l.fs - fs * RUBY_PINYIN_MAX).abs() < 0.05,
            "10 列：拼音字号 {:.2} 未吃到 26% 上限 {:.2}",
            l.fs,
            fs * RUBY_PINYIN_MAX
        );
        assert!((l.top + l.fs * 0.5).abs() < 1e-3, "10 列：未悬挂（top 应为 −fs/2）");
        assert!((l.band_h - chh * RUBY_BAND_FRAC).abs() < 0.01);
        assert!(l.block_w < cw * 0.5, "10 列：三字母拼音不该占掉半格宽");
        assert!((l.left - (cw - l.block_w) * 0.5).abs() < 0.01);
        // 墨迹整块落在字缝里（10 列字缝 ≈ ±4.10px，墨高 5.26px 只占 64%，
        // 上下各剩 ≈1.47px 净空）。
        let headroom = (chh - fs * CHAR_INK_OF_EM) * 0.5;
        assert!(
            l.ink_top() >= -headroom - 0.01 && l.ink_bottom() <= headroom + 0.01,
            "10 列：拼音墨迹越出字缝（{:.2} … {:.2}，缝 ±{:.2}）",
            l.ink_top(),
            l.ink_bottom(),
            headroom
        );
        assert!(
            l.ink_h() <= headroom * 2.0 * 0.7,
            "10 列：拼音墨高 {:.2} 占字缝太多（缝宽 {:.2}）",
            l.ink_h(),
            headroom * 2.0
        );
    }

    /// **朱砂避让**（v1.9.55）：拼音是朱砂，句读也是朱砂，两者绝不许糊在一起。
    ///
    /// 句读记号（红点 / 空心圈）的心钉在 `PUNCT_ZONE_Y = 0.72` 格高处，
    /// 记号半径约 `0.35 × 字号 × 0.5 ≈ 0.15 格高`，故它的上沿在 `0.57` 格高。
    /// 拼音墨迹必须**整块停在这条线以上**——中间留出至少 17% 格高的空白，
    /// 一红在上、一红在下，永远不会连成一坨。
    #[test]
    fn pinyin_ink_stays_clear_of_the_punct_zone() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            // 句读上沿：心 0.72 − 半径 0.15
            let punct_top = chh * (PUNCT_ZONE_Y - 0.15);
            for py in ["měi", "yǐ", "cāng", "zhuāng", "四行本"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.top + l.ink_h() <= punct_top,
                    "{cols} 列 / {py}：拼音墨底 {:.2} 压进句读带（上沿 {:.2}）",
                    l.top + l.ink_h(),
                    punct_top
                );
                // 同一格里的「挤压句读」也挂在右下角，横向同样不许碰：
                // 拼音整串恒居中，最多占到 93% 格宽；句读心在 0.72 格宽、
                // 半径 0.15 格宽 → 左沿 0.57。两者只在墨底远高于它时才算安全。
                assert!(
                    l.left + l.block_w <= cw + 0.01,
                    "{cols} 列 / {py}：拼音横向越出字格"
                );
            }
        }
    }

    /// CJK 注记（如「四行本」）同样走顶端外挂，且按 1em/字 计宽（不挤成一团）。
    #[test]
    fn cjk_note_also_hangs_from_the_top() {
        let (cw, chh, fs) = cell(12);
        let l = layout("四行本", fs, cw, chh);
        assert!(l.top < 0.0, "CJK 注记也要悬挂（top {:.2} 应 < 0）", l.top);
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
