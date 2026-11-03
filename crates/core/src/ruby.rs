//! 注音（生僻字拼音 / 多字注记）几何：**竖排、小字、贴在主字右侧**，古籍双行
//! 夹注形态。
//!
//! v1.9.52——**拼音（拉丁字母）单列竖排**。v1.9.51 把拼音也按 CJK 注记的规矩
//! 拆成「双列竖排」，`wéi` / `cāng` 于是变成 2×2 的字母方块，撞碎界栏、压住句读
//! 红点，是彻底的视觉车祸。现在按两条规矩分开：
//!
//! * **拼音（全拉丁字母，如 `měi` / `yǐ` / `cāng`）**：**单列竖排**——字母从上到
//!   下垂直堆叠，像一条细长的小尾巴顺着文字流向（日文注音形态）。字号上限
//!   **主字的 40%**；太长（如 `zhuang`）就按字格高自动缩小，硬下限 18%，再放
//!   不下由渲染层**裁剪**，绝不越出字格。
//! * **CJK 多字注记（如 `四行本`）**：维持古籍**双行/多列竖排夹注**——一列写前
//!   几个字、一列写后几个字，字号 45%~55%。
//!
//! 两类的共同铁律：
//! * 主字在字格内**贴左缘**，注音整块**垂直居中**于字格、绝不悬浮在间隙里；
//! * 主字与注音之间恒留 ≥1px 呼吸缝（`RUBY_GAP_MIN`），绝不粘连；
//! * 注音整块右缘恒在字格右缘内侧 `RUBY_RIGHT_INSET`（绝不压右边界栏）；
//! * 绝不与句读红点抢位（单列拼音本来就窄，只占主字右侧一小条）。
//!
//! ```text
//! ┌─┬──────┐      ┌─┬──────┐
//! │猛│ m    │      │猛│四行  │   ← 拼音单列竖排（m/ě/i 自上而下）
//! │  │ ě    │      │  │  本  │      vs  CJK 注记双列竖排
//! │  │ i    │      └─┴──────┘
//! └─┴──────┘
//! ```
//!
//! ⚠️ 屏幕与 PDF 共用本文件的度量（`layout` + `glyph_xy`），逐字同位。

/// 拼音（拉丁）字号上限 = 主字的 **40%**（用户铁律：拼音必须比主字小得多）。
pub const RUBY_SCALE_PINYIN_MAX: f32 = 0.40;
/// 拼音太长时的硬下限 = 主字的 **18%**（再小则由渲染层裁剪，绝不越格）。
pub const RUBY_SCALE_PINYIN_MIN: f32 = 0.18;

/// CJK 短注音（≤2 字）的目标字号 = 主字的 **55%**（古籍双行小字约半大的观感）。
pub const RUBY_SCALE_SHORT: f32 = 0.55;
/// CJK 多字注记（≥3 字）的目标字号 = 主字的 **45%**。
pub const RUBY_SCALE_LONG: f32 = 0.45;
/// CJK 字号硬下限 = 主字的 30%（再小就不可读；仅当右侧空当极窄时兜底触发）。
pub const RUBY_SCALE_MIN: f32 = 0.30;

/// 带注音的主字在字格内**贴左缘**时的左内边距（像素）。屏幕 `ancient_cell` 与
/// PDF `draw_cell` 共用，确保「主字左移」与注音落点严格同位。
pub const RUBY_CHAR_LEFT: f32 = 1.0;
/// 主字与注音之间的最小呼吸缝（像素）：用户铁律「绝不能粘在一起」——
/// 注音整块左缘恒在主字右缘右侧 ≥ 此值处。
pub const RUBY_GAP_MIN: f32 = 1.0;
/// 注音整块右缘距字格右缘的内边距（像素）：绝不压到右边的界栏线。
pub const RUBY_RIGHT_INSET: f32 = 1.0;
/// CJK 多列注音之间的列距（像素）：极简，仅一丝呼吸。
pub const RUBY_COL_GAP: f32 = 1.0;

/// 一次注音排版的度量结果。
///
/// 所有长度都是「屏幕逻辑像素」，且都**相对字格左上角**（渲染层直接拿来定位，
/// PDF 侧同一套）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubyLayout {
    /// 注音字号。
    pub fs: f32,
    /// 竖排的列数：拼音恒为 1（单列小尾巴）；CJK 注记可能拆 2 列以上。
    pub cols: usize,
    /// 每列最多行数（拼音 = 字数；CJK 注记 = ⌊(格高 − 1) / 字号⌋）。
    pub rows: usize,
    /// 整块宽（CJK 多列 + 列距；拼音 = 字号）。
    pub block_w: f32,
    /// 整块高（已按字格高裁剪、并垂直居中）。
    pub block_h: f32,
    /// **整块左缘**（紧贴主字右缘那一侧）相对字格左边的偏移。
    pub left: f32,
    /// 整块上缘相对字格上边的偏移（已垂直居中）。
    pub top: f32,
}

impl RubyLayout {
    /// 第 `i` 个字（0 基，按古籍阅读序：先左列后右列、列内自上而下）的
    /// **左上角**坐标（相对字格左上角）。
    ///
    /// 单列（拼音）时 `rows = 字数`，故恒 `col = 0`、`r = i`——字母自上而下堆叠。
    pub fn glyph_xy(&self, i: usize) -> (f32, f32) {
        let rows = self.rows.max(1);
        let col = i / rows; // 拼音恒 0；CJK 注记 0 = 紧贴主字
        let r = i % rows;
        let x = self.left + col as f32 * (self.fs + RUBY_COL_GAP);
        let y = self.top + r as f32 * self.fs;
        (x, y)
    }
}

/// 是否 CJK（含扩展 A / 兼容区 / 扩展 B 起）。用来区分「CJK 注记」与「拉丁拼音」。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF      // 扩展 A
        | 0x4E00..=0x9FFF    // 基本区
        | 0xF900..=0xFAFF    // 兼容
        | 0x20000..=0x3FFFF  // 扩展 B 起（含生僻字）
    )
}

/// 计算一条注音的竖排度量。
///
/// * `text`：注音串（拉丁拼音如 `měi`；或 CJK 注记如 `四行本`）；
/// * `body_fs`：正文字号；
/// * `cell_w` / `cell_h`：字格宽高（本 app 竖排：字格**宽 > 高**，故主字右侧
///   恒有横向空当容得下竖排小字）。
pub fn layout(text: &str, body_fs: f32, cell_w: f32, cell_h: f32) -> RubyLayout {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len().max(1);
    // 主字贴左缘：右缘 = RUBY_CHAR_LEFT + body_fs。
    let char_right = RUBY_CHAR_LEFT + body_fs;
    // 注音可用宽 = 主字右缘 → 字格右缘 之间、扣掉两侧呼吸。
    let avail_w = (cell_w - char_right - RUBY_GAP_MIN - RUBY_RIGHT_INSET).max(2.0);
    let left = char_right + RUBY_GAP_MIN;

    // 全拉丁（无 CJK）→ 拼音：**单列竖排**，字母自上而下堆叠。
    let is_pinyin = !chars.iter().any(|c| is_cjk(*c));
    if is_pinyin {
        // 字号 = min(主字 40%, 竖着塞进字格高, 右侧可用宽)，硬下限 18%。
        let by_h = ((cell_h - 1.0) / n as f32).max(1.0);
        let mut fs = (body_fs * RUBY_SCALE_PINYIN_MAX)
            .min(by_h)
            .min(avail_w)
            .max(3.0);
        fs = fs.max(body_fs * RUBY_SCALE_PINYIN_MIN);
        let block_w = fs;
        // 绝不越出字格：整块高按字格高裁剪（再放不下由渲染层 mask 掉）。
        let block_h = (n as f32 * fs).min(cell_h);
        let top = ((cell_h - block_h) * 0.5).max(0.0);
        return RubyLayout {
            fs,
            cols: 1,
            rows: n,
            block_w,
            block_h,
            left,
            top,
        };
    }

    // CJK 多字注记：维持古籍双行/多列竖排（短 55%、多 45%，按可用宽等比缩）。
    let scale0 = if n <= 2 { RUBY_SCALE_SHORT } else { RUBY_SCALE_LONG };
    let mut scale = scale0;
    let (fs, cols, rows, block_w, block_h, top);
    loop {
        let fs_cand = (body_fs * scale).max(3.0);
        // 每列最多行数（竖向紧贴，步进 = 字号；留 1px 防粘连）。
        let rows_cand = (((cell_h - 1.0) / fs_cand).floor().max(1.0)) as usize;
        let cols_cand = (n + rows_cand - 1) / rows_cand; // ⌈n / rows⌉
        let bw = cols_cand as f32 * fs_cand + (cols_cand - 1).max(0) as f32 * RUBY_COL_GAP;
        if bw <= avail_w + 1e-3 || scale <= RUBY_SCALE_MIN + 1e-3 {
            fs = fs_cand;
            cols = cols_cand;
            rows = rows_cand;
            block_w = bw;
            block_h = (rows as f32 * fs_cand).min(cell_h);
            top = ((cell_h - block_h) * 0.5).max(0.0);
            break;
        }
        scale = (scale * 0.9).max(RUBY_SCALE_MIN);
    }
    RubyLayout {
        fs,
        cols,
        rows,
        block_w,
        block_h,
        left,
        top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{compute_grid, GridConfig, GridDir};

    /// 8 / 14 列两档的真实几何（宽屏 1320×880，与 `grid` 测试同源）。
    fn cell(cols: usize) -> (f32, f32, f32) {
        let m = compute_grid(
            1320.0,
            880.0 - 78.0,
            GridConfig {
                columns: cols,
                dir: GridDir::Vertical,
                margin: 0.04, line_scale: 1.0, fs_ratio: 0.75, line_ratio: 1.0 / 0.75,
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

    /// 拼音恒**单列竖排**、字号 ≤ 主字 40%、整块高不出格。
    #[test]
    fn pinyin_is_single_column_under_40pct() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["měi", "yǐ", "wéi", "cāng", "zhuang"] {
                let l = layout(py, fs, cw, chh);
                assert_eq!(l.cols, 1, "{cols} 列 / {py}：拼音必须单列（不得拆两列）");
                assert_eq!(l.rows, py.chars().count(), "{cols} 列 / {py}：行数应等于字母数");
                assert!(
                    l.fs <= fs * RUBY_SCALE_PINYIN_MAX + 0.01,
                    "{cols} 列 / {py}：拼音字号 {:.3} 超主字 40%",
                    l.fs / fs
                );
                assert!(
                    l.fs >= fs * RUBY_SCALE_PINYIN_MIN - 0.01,
                    "{cols} 列 / {py}：拼音字号跌破 18% 下限"
                );
                // 整块高不出字格；右缘不越界栏。
                assert!(l.top >= 0.0 && l.top + l.block_h <= chh + 0.01, "{cols} 列 / {py}：纵向出格");
                assert!(l.left + l.block_w <= cw - RUBY_RIGHT_INSET + 0.01, "{cols} 列 / {py}：越右界栏");
                let char_right = RUBY_CHAR_LEFT + fs;
                assert!(l.left >= char_right + RUBY_GAP_MIN - 1e-3, "{cols} 列 / {py}：压住主字");
            }
        }
    }

    /// 拼音字母自上而下堆叠：第 i 个字母的 y 严格递增、x 恒定。
    #[test]
    fn pinyin_letters_stack_vertically() {
        let (cw, chh, fs) = cell(12);
        let l = layout("cāng", fs, cw, chh);
        let (x0, y0) = l.glyph_xy(0);
        for i in 1..4 {
            let (x, y) = l.glyph_xy(i);
            assert!((x - x0).abs() < 1e-3, "字母 {i} 的 x 应与首字母一致（同列）");
            assert!(y > y0, "字母 {i} 的 y 应大于首字母（自上而下）");
        }
    }

    /// CJK 多字注记维持双列竖排（不因拼音改动被误伤）。
    #[test]
    fn cjk_note_still_splits_into_columns() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            let l = layout("四行本", fs, cw, chh);
            assert_eq!(l.cols, 2, "{cols} 列：四行本应拆 2 列（古籍双行夹注）");
            assert!(l.cols * l.rows >= 3, "列数 × 行数 不足以容下 3 字");
        }
    }

    /// CJK 短注音吃满 55%、多字 45%，且都不溢出字格右缘。
    #[test]
    fn short_keeps_055_long_keeps_045() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["猛", "四行"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    (l.fs - fs * RUBY_SCALE_SHORT).abs() < 0.01,
                    "{cols} 列 / {py}：短注音没吃满 0.55（实得 {:.3}）",
                    l.fs / fs
                );
            }
            for py in ["四行本"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    (l.fs - fs * RUBY_SCALE_LONG).abs() < 0.01,
                    "{cols} 列 / {py}：多字注记没吃满 0.45（实得 {:.3}）",
                    l.fs / fs
                );
            }
        }
    }

    /// CJK：整块右缘绝不越过字格右缘、左缘不压主字、纵向不出格。
    #[test]
    fn cjk_note_stays_in_cell() {
        for cols in [8usize, 10, 12, 14] {
            let (cw, chh, fs) = cell(cols);
            for py in ["猛", "四行", "四行本", "双行夹注示例"] {
                let l = layout(py, fs, cw, chh);
                assert!(
                    l.left + l.block_w <= cw - RUBY_RIGHT_INSET + 0.01,
                    "{cols} 列 / {py}：注音越过字格右缘（右缘 {:.2} > {:.2}）",
                    l.left + l.block_w,
                    cw - RUBY_RIGHT_INSET
                );
                let char_right = RUBY_CHAR_LEFT + fs;
                assert!(
                    l.left >= char_right + RUBY_GAP_MIN - 1e-3,
                    "{cols} 列 / {py}：注音压住了主字"
                );
                assert!(l.top >= 0.0 && l.top + l.block_h <= chh + 0.01, "{cols} 列 / {py}：纵向出格");
            }
        }
    }

    /// 字号下限兜底：即使右侧空当极窄，也不跌破 30%、也不压到字上、也不越界栏。
    #[test]
    fn never_below_floor() {
        let (cw, chh, fs) = cell(14);
        let l = layout("双行夹注示例", fs, cw, chh);
        assert!(
            l.fs >= fs * RUBY_SCALE_MIN - 0.01,
            "字号跌破 30% 下限（{:.3}）",
            l.fs / fs
        );
        assert!(l.left + l.block_w <= cw - RUBY_RIGHT_INSET + 0.01);
        let char_right = RUBY_CHAR_LEFT + fs;
        assert!(l.left >= char_right + RUBY_GAP_MIN - 1e-3);
    }
}
