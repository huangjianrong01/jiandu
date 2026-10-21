//! 竖排西文 / 特殊符号混排：宽度账、旋转坐标、竖排字形替换、数字转汉字。
//!
//! 竖排古籍里遇到 `1628`、`ISBN` 这类西文串，刻本的处理是**整串横躺**
//! （顺时针转 90°）顺着列往下读，而不是一个字母占一格竖着摞。本模块只做
//! 「纯计算」，不碰 GPUI：
//!
//! * [`run_cells`] / [`width_units`]：按字符数折算占几格（两个西文字符
//!   占一个汉字格；`.` 这类零宽符号不占独立格）；
//! * [`rotated_slots`]：整串旋转 90° 后每个字符的**旋转中心 / 绘制原点**
//!   （渲染层据此摆位；将来 GPUI 支持元素级变换时直接套用）；
//! * [`vertical_paren`] / [`vertical_hyphen`]：竖排专用字形（︵ ︶ 丨）；
//! * [`digits_to_han`]：数字转汉字（一六二八 / 壹陆贰捌），供「古籍纯粹感」开关。
//!
//! 分页（`gloss`）与渲染（`reader`）都调这里的函数，宽度账同源，
//! 不会出现「分页算得下、渲染放不下」。

/// 一个汉字格内容纳的西文字符数：两个数字 / 字母占一格。
pub const WEST_PER_CELL: usize = 2;

/// 是否 CJK（含假名 / 谚文）：用来把「西文」与「汉字」分开。
pub fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x2FFF     // 部首扩展
        | 0x3040..=0x30FF   // 平假名 / 片假名
        | 0x3400..=0x4DBF   // 扩展 A
        | 0x4E00..=0x9FFF   // 基本区
        | 0xF900..=0xFAFF   // 兼容区
        | 0xAC00..=0xD7AF   // 谚文
    )
}

/// 是否西文字符（字母 / 数字）：需要整串竖排压缩的字符。
///
/// 只认「拉丁字母 + 数字」。标点另行判断——`.` / `-` 只有夹在
/// 西文串中间时才并入该串（见 [`is_run_tail`]）。
pub fn is_west(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || (c.is_alphabetic() && !is_cjk(c) && !c.is_ascii())
}

/// 零宽符号：小数点（不占独立网格，缩小叠在前一数字右下）。
pub fn is_west_dot(c: char) -> bool {
    matches!(c, '.' | '．')
}

/// 连字符：竖排里旋转为竖线 `丨`。
pub fn is_west_hyphen(c: char) -> bool {
    matches!(c, '-' | '‐' | '–' | '−')
}

/// 括号（半角与全角）：竖排里换成 `︵` / `︶`。
pub fn is_paren(c: char) -> bool {
    matches!(c, '(' | ')' | '（' | '）')
}

/// `c` 能否**接在西文串后面**继续算同一串（字母数字 / 小数点在串中 / 连字符）。
///
/// 首字符必须是 [`is_west`]；`.` 与 `-` 只有前面已经有西文字符时才并入，
/// 避免把句末句号、破折号误吞进字母串。
pub fn is_run_tail(c: char, has_prev: bool) -> bool {
    is_west(c) || (has_prev && (is_west_dot(c) || is_west_hyphen(c)))
}

/// 一个字符占几个「半格」：西文字符 1 个单位，小数点 0 个。
///
/// 两个单位 = 一个汉字格，与 [`WEST_PER_CELL`] 一致。
pub fn char_units(c: char) -> usize {
    if is_west_dot(c) {
        0
    } else {
        1
    }
}

/// 一串字符的宽度单位（半格数）。`1628` → 4，`3.14` → 3。
pub fn width_units<I: IntoIterator<Item = char>>(chars: I) -> usize {
    chars.into_iter().map(char_units).sum()
}

/// 一串字符占几个汉字格：`ceil(units / 2)`。`1628` → 2 格。
pub fn run_cells<I: IntoIterator<Item = char>>(chars: I) -> usize {
    let u = width_units(chars);
    (u + WEST_PER_CELL - 1) / WEST_PER_CELL
}

/// 竖排括号字形：`(` → `︵`，`)` → `︶`（全角同理）。
///
/// `︵`/`︶` 是 Unicode 的竖排专用括号，本身已「横躺」，不需要再旋转；
/// 在竖列里上下相接，正好贴合汉字基线。
pub fn vertical_paren(c: char) -> Option<char> {
    match c {
        '(' | '（' => Some('︵'),
        ')' | '）' => Some('︶'),
        _ => None,
    }
}

/// 竖排连字符：转为竖线 `丨`（旋转 90° 后的样子）。
pub fn vertical_hyphen(_c: char) -> char {
    '丨'
}

/// 数字转汉字。`upper = true` 用大写（壹贰叁…），否则用小写（一二三…）。
///
/// 只转数字，其余字符原样保留——`ISBN 978-7` → `ISBN 九七八-七`。
pub fn digits_to_han(s: &str, upper: bool) -> String {
    const LOWER: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    const UPPER: [char; 10] = ['零', '壹', '贰', '叁', '肆', '伍', '陆', '柒', '捌', '玖'];
    let table = if upper { UPPER } else { LOWER };
    s.chars()
        .map(|c| match c.to_digit(10) {
            Some(d) => table[d as usize],
            None => c,
        })
        .collect()
}

/// 单个西文字符的竖排显示形态：连字符转竖线，其余原样。
pub fn vertical_glyph(c: char) -> char {
    if is_west_hyphen(c) {
        vertical_hyphen(c)
    } else {
        c
    }
}

/// 西文竖排字号：两个字符并排塞进一个汉字格，字号随格宽与正文字号收敛。
///
/// 下限 7px（再小就成墨点）；上限取正文字号的 0.66（西文字面比汉字窄，
/// 略小于正文才不显突兀）。
pub fn west_font_size(cell_w: f32, body_fs: f32) -> f32 {
    let by_cell = cell_w / WEST_PER_CELL as f32 * 1.30;
    (body_fs * 0.66).min(by_cell).max(7.0)
}

/// 旋转后的一个字符槽位：绘制时把字形绕 `(cx, cy)` 顺时针转 `angle_deg`。
///
/// 竖排读序自上而下，故第 `j` 个字符的槽位在 `y = j * cell_h / 2`；
/// 旋转不改变方框位置（正方形绕中心转 90° 仍是原框），只改**框内内容的朝向**。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WestSlot {
    /// 旋转中心（相对该列左上角）。
    pub cx: f32,
    /// 旋转中心纵坐标。
    pub cy: f32,
    /// 字形方框边长（= 字号）。
    pub size: f32,
    /// 顺时针旋转角（度）。90 = 横躺。
    pub angle_deg: f32,
    /// 旋转后绘制原点（方框左上角；与未旋转时同一位置）。
    pub x: f32,
    /// 绘制原点纵坐标。
    pub y: f32,
}

impl WestSlot {
    /// 字形旋转后覆盖的矩形 `(x, y, w, h)`。
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        (self.x, self.y, self.size, self.size)
    }
}

/// 计算一串西文旋转 90° 后的逐字槽位。
///
/// 几何（`cell_w × cell_h` 为一格）：
/// * 每格纵向容纳 [`WEST_PER_CELL`] 个字符，故字符间距 `cell_h / 2`；
/// * 每格内两个字符**纵向均分**，整体在格中垂直居中；
/// * 横向居中于列宽 `cell_w / 2`；
/// * 旋转角固定 90°（顺时针，字头朝右——中文竖排里西文的传统朝向）。
///
/// 返回的槽位可直接用于「按中心旋转绘制」；当前 GPUI 0.2.2 无元素级
/// 变换，渲染层退化为「同槽位并排直立」，**槽位与宽度账完全一致**，
/// 故版面不会因渲染方式不同而错位。
pub fn rotated_slots(chars: &[char], cell_w: f32, cell_h: f32, size: f32) -> Vec<WestSlot> {
    let slot_h = cell_h / WEST_PER_CELL as f32;
    let mut out = Vec::with_capacity(chars.len());
    let mut j = 0usize;
    for c in chars {
        // 小数点零宽：与前一字符同槽，不推进读序。
        if is_west_dot(*c) && j > 0 {
            let prev = out.last().copied().unwrap();
            out.push(WestSlot { ..prev });
            continue;
        }
        let cy = j as f32 * slot_h + slot_h * 0.5;
        out.push(WestSlot {
            cx: cell_w * 0.5,
            cy,
            size,
            angle_deg: 90.0,
            x: cell_w * 0.5 - size * 0.5,
            y: cy - size * 0.5,
        });
        j += 1;
    }
    out
}

/// 把一串西文按「一格两个字符」切成分组（小数点跟随前一字符）。
///
/// `"3.14"` → `["3.1", "4"]`（小数点零宽、并进当前格，格数仍是 2）；
/// `"ISBN"` → `["IS", "BN"]`。
/// 渲染层逐组排一行，高度恰为 `组数 × cell_h`，与 [`run_cells`] 对齐。
pub fn pack_cells(chars: &[char]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut units_in_cur = 0usize;
    for &c in chars {
        if is_west_dot(c) {
            // 小数点并进当前组；若当前组空（串首），单独起一组兜底。
            match out.last_mut() {
                Some(last) => last.push(c),
                None => out.push(c.to_string()),
            }
            continue;
        }
        if units_in_cur >= WEST_PER_CELL || out.is_empty() {
            out.push(String::new());
            units_in_cur = 0;
        }
        out.last_mut().unwrap().push(c);
        units_in_cur += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_two_west_per_cell() {
        assert_eq!(width_units("1628".chars()), 4);
        assert_eq!(run_cells("1628".chars()), 2);
        assert_eq!(run_cells("ISBN".chars()), 2);
        assert_eq!(run_cells("1".chars()), 1);
        assert_eq!(run_cells("123".chars()), 2); // 3 个 → 2 格（余一个独占一格）
    }

    #[test]
    fn decimal_point_is_zero_width() {
        assert_eq!(width_units("3.14".chars()), 3);
        assert_eq!(run_cells("3.14".chars()), 2);
        // 小数点零宽并进当前格，格数仍与 run_cells 一致（2 格）。
        assert_eq!(pack_cells(&"3.14".chars().collect::<Vec<_>>()), vec!["3.1", "4"]);
        assert_eq!(pack_cells(&"3.14".chars().collect::<Vec<_>>()).len(), 2);
    }

    #[test]
    fn hyphen_only_joins_inside_run() {
        assert!(is_run_tail('1', false));
        assert!(!is_run_tail('-', false)); // 串首不是连字符
        assert!(is_run_tail('-', true)); // 串中可以是
        assert!(!is_run_tail('。', true));
    }

    #[test]
    fn vertical_forms_for_paren_and_hyphen() {
        assert_eq!(vertical_paren('('), Some('︵'));
        assert_eq!(vertical_paren(')'), Some('︶'));
        assert_eq!(vertical_paren('（'), Some('︵'));
        assert_eq!(vertical_paren('）'), Some('︶'));
        assert_eq!(vertical_paren('1'), None);
        assert_eq!(vertical_glyph('-'), '丨');
    }

    #[test]
    fn digits_convert_to_han() {
        assert_eq!(digits_to_han("1628", false), "一六二八");
        assert_eq!(digits_to_han("1628", true), "壹陆贰捌");
        assert_eq!(digits_to_han("ISBN 978", false), "ISBN 九七八");
    }

    #[test]
    fn rotated_slots_stay_in_two_cells() {
        let chars: Vec<char> = "1628".chars().collect();
        let (cw, ch) = (32.0, 30.0);
        let slots = rotated_slots(&chars, cw, ch, 14.0);
        assert_eq!(slots.len(), 4);
        // 前两字在第一格，后两字在第二格；全部横向居中。
        assert!((slots[0].cy - ch * 0.25).abs() < 0.01);
        assert!((slots[1].cy - ch * 0.75).abs() < 0.01);
        assert!((slots[2].cy - ch * 1.25).abs() < 0.01);
        assert!((slots[3].cy - ch * 1.75).abs() < 0.01);
        for s in &slots {
            assert!((s.cx - cw * 0.5).abs() < 0.01);
            assert_eq!(s.angle_deg, 90.0);
            let (x, y, w, h) = s.rect();
            assert!(x >= -0.01 && x + w <= cw + 0.01, "横向溢出列宽");
            assert!(y >= -0.01 && y + h <= 2.0 * ch + 0.01, "纵向溢出两格");
        }
        // 槽位总高恰为两格 = run_cells(4) * cell_h。
        assert!((slots.last().unwrap().cy + ch * 0.25 - 2.0 * ch).abs() < 0.01);
    }

    #[test]
    fn pack_cells_groups_by_two() {
        let isbn: Vec<char> = "ISBN".chars().collect();
        assert_eq!(pack_cells(&isbn), vec!["IS", "BN"]);
        let lone: Vec<char> = "7".chars().collect();
        assert_eq!(pack_cells(&lone), vec!["7"]);
    }

    #[test]
    fn west_font_never_zero() {
        assert!(west_font_size(4.0, 8.0) >= 7.0); // 极窄格也保底 7px
        assert!(west_font_size(40.0, 30.0) <= 19.8 + 0.01);
    }
}
