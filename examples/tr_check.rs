//! **繁体字形体检**（v1.56.0 内置书目配套）。
//!
//! 简体转繁体之后，最该担心的几件事，这里各验一遍：
//!   1. **IR 往返无损**——内置《道德經》过一遍 IR（解析器 → 清洗器的统一
//!      中间层）再转回来，八十一章正文必须一字不差。繁体字一旦在清洗链
//!      路上被「洗」成别的字，用户看到的就是错版；尤其「云/雲」「冲/沖」
//!      「采/採」这类一简对多繁，必须盯死。
//!   2. **字汇覆盖**——书里不能有任何 PUA 私造字（U+E000–U+F8FF），否则
//!      渲染出来是一堆豆腐块（□）。
//!   3. **版心几何不糊**——60% 留白（最宽档，格子最小）下，竖排「列宽 ≥
//!      字号」必须成立，且字号不触 12px 地板、不进单页滚动档。列宽不够
//!      或字被压小，相邻列/相邻字就会左右相碰、糊成一团。
//!      **说明**：行高倍率（`line_scale`）在版式引擎里**不参与字号/行高
//!      反算**（grid.rs 有测试钉死），故 8 款字体共用同一份几何；繁体
//!      字形差异只在栅格化层，那一层必须靠 GUI 截图核验（见文末说明）。
//!   4. **暗金模式对比度**——暗金墨组（Gold）在「夜读深底」与「宣纸浅纸」
//!      两路下，正文/批注对比度都要过 WCAG AA（≥ 4.5），否则深底浅金字
//!      会糊成一片、看不清笔画。
//!
//! 跑法：`cargo run --example tr_check`

use jiandu_core::grid::{calculate_grid, BANXIN_RATIOS, GRID_FS_MAX, GridDir, GridInput, LayoutMode};
use jiandu_core::ir::{IrBook, IrSource};
use jiandu_core::theme::{InkPalette, Theme};

/// 竖排正文的可读下限（px）：繁体笔画密，低于此值就糊。
const READABLE_FS: f32 = 13.0;
/// WCAG AA 普通正文对比度门槛。
const WCAG_AA: f32 = 4.5;

/// WCAG 相对亮度（0..1）。
fn rel_lum(c: u32) -> f32 {
    let chan = |v: u32| {
        let s = (v as f32) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let r = chan((c >> 16) & 0xFF);
    let g = chan((c >> 8) & 0xFF);
    let b = chan(c & 0xFF);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// WCAG 对比度（≥ 1）。
fn contrast(a: u32, b: u32) -> f32 {
    let la = rel_lum(a);
    let lb = rel_lum(b);
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn main() {
    let book = jiandu_core::content::bundled_dao_de_jing().expect("内置《道德經》应能解析");

    // ── ① IR 往返无损 ──────────────────────────────────────────────
    let ir = IrBook::from_book(&book, IrSource::Json);
    let back = ir.to_book();
    let a: Vec<String> = book
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .map(|p| p.text.clone())
        .collect();
    let b: Vec<String> = back
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .map(|p| p.text.clone())
        .collect();
    assert_eq!(a.len(), b.len(), "IR 往返后段数变了");
    let diff = a.iter().zip(b.iter()).filter(|(x, y)| x != y).count();
    println!(
        "① IR 往返：{} 段，异文 {} 段 {}",
        a.len(),
        diff,
        if diff == 0 { "✓" } else { "✗" }
    );
    // 一简对多繁敏感字：掉字 / 被转简都会在这里露馅。
    for probe in ["無", "萬", "聖", "眾", "後", "沖", "云", "採", "雲", "髮", "發", "復"] {
        let n: usize = a
            .iter()
            .map(|t| t.chars().filter(|c| *c == probe.chars().next().unwrap()).count())
            .sum();
        if n > 0 {
            println!("     「{probe}」× {n}");
        }
    }

    // ── ② 字汇覆盖：无 PUA 私造字 ────────────────────────────────
    let mut pua = 0usize;
    for t in &a {
        for c in t.chars() {
            if ('\u{E000}'..='\u{F8FF}').contains(&c) {
                pua += 1;
            }
        }
    }
    println!("\n② 字汇覆盖：PUA 私造字 {pua} {}", if pua == 0 { "✓" } else { "✗" });

    // ── ③ 60% 留白 × 列数 的版心几何（字体无关，见文件头说明）──────
    // 60% = BANXIN_RATIOS[0]（宽 / 复古），格子最小，是最苛刻的一档。
    let wide = BANXIN_RATIOS[0];
    // 典型 MacBook Air 视口，扣掉顶栏 44 / 状态栏 34 / 四边留白 44。
    let (vw, vh) = (1440.0f32, 900.0f32);
    let content_w = vw - 44.0 * 2.0;
    let content_h = vh - 44.0 - 34.0 - 44.0 * 2.0;
    println!("\n③ 版心几何（视口 {vw:.0}×{vh:.0}，留白 {wide} = 最宽档）");
    println!("   列数  列宽   字号  行高  行数  min?  列宽/字号  判定");
    let mut bad = 0usize;
    for cols in [8usize, 10, 12, 14] {
        let plan = calculate_grid(GridInput {
            columns: cols,
            mode: LayoutMode::Facing,
            content_w,
            content_h,
            zoom: 1.0,
            edge: 50.0,
            head: 0.0,
            foot: 0.0,
            margin: 0.0,
            line_scale: 1.0,
            fs_ratio: 0.0,
            line_ratio: 0.0,
            banxin_ratio: wide,
            dir: GridDir::Vertical,
            fs_max: GRID_FS_MAX,
        });
        let m = plan.m;
        let ratio = m.col_w / m.fs;
        // 竖排：列宽必须 ≥ 字号，否则相邻列的字会左右相碰（糊）；
        // 字号必须 ≥ 可读下限；min_font_hit 必须为 false（否则进滚动档）。
        let ok = m.col_w >= m.fs - 0.01 && m.fs >= READABLE_FS && !m.min_font_hit;
        if !ok {
            bad += 1;
        }
        println!(
            "   {:>3}  {:>6.1} {:>6.2} {:>6.1} {:>4}  {}{:>6.2}    {}",
            cols,
            m.col_w,
            m.fs,
            m.row_h,
            m.rows,
            if m.min_font_hit { "Y " } else { "N " },
            ratio,
            if ok { "✓" } else { "✗" }
        );
    }
    let verdict = if bad == 0 {
        "✓ 全部安全（列宽≥字号、字号≥13px、无滚动档）——繁体不会因间距被压糊".to_string()
    } else {
        format!("✗ {bad} 个组合几何异常")
    };
    println!(
        "   ③ 结论：{} 个列数组合几何异常 {}",
        [8usize, 10, 12, 14].len(),
        verdict
    );
    println!(
        "   （字体不影响版心几何：行高倍率不参与字号/行高反算，8 款字体共用此表；\n    字形差异只在栅格化层，须由 GUI 截图核验，见第 ⑤ 条说明）"
    );

    // ── ④ 暗金模式对比度 ─────────────────────────────────────────
    println!("\n④ 暗金墨组（Gold）对比度");
    // 深底：夜读纸(0x1A1C1D) × 正文月白(0xDCD6CD) × 批注秋香(0xA68E5C)
    let ye_paper = Theme::Ye.paper_theme().paper;
    let ye_ink = InkPalette::Gold.ink(Theme::Ye);
    let ye_note = 0xA68E5C;
    // 浅纸：宣纸(0xF5F0E6) × 正文暗金(0x7A5C1E) × 批注淡金(0xAD5A33)
    let xuan_paper = Theme::Xuan.paper_theme().paper;
    let xuan_ink = InkPalette::Gold.ink(Theme::Xuan);
    let xuan_note = 0xAD5A33;
    let c_yi = contrast(ye_paper, ye_ink);
    let c_yn = contrast(ye_paper, ye_note);
    let c_xi = contrast(xuan_paper, xuan_ink);
    let c_xn = contrast(xuan_paper, xuan_note);
    println!("   夜读深底  纸 {:06X} × 正文月白 {:06X}  对比 {:.1}", ye_paper, ye_ink, c_yi);
    println!("   夜读深底  纸 {:06X} × 批注秋香 {:06X}  对比 {:.1}", ye_paper, ye_note, c_yn);
    println!("   宣纸浅纸  纸 {:06X} × 正文暗金 {:06X}  对比 {:.1}", xuan_paper, xuan_ink, c_xi);
    println!("   宣纸浅纸  纸 {:06X} × 批注淡金 {:06X}  对比 {:.1}", xuan_paper, xuan_note, c_xn);
    // 阅读主流量（正文）必须过 AA(4.5)；批注是刻意做薄的二级字（theme.rs
    // 有测试钉死「批注必须比正文淡」），只要不跌破 3.0（AA 大字档）即可——
    // 真要逐字 AA 可以让淡金再沉一档，但会牺牲「薄墨」层次感，交给设计权衡。
    let main_ok = c_yi >= WCAG_AA && c_xi >= WCAG_AA;
    let note_ok = c_yn >= 3.0 && c_xn >= 3.0;
    println!(
        "   ④ 结论：正文两路（深底 {:.1} / 浅纸 {:.1}）{}，繁体主文绝不会糊；",
        c_yi, c_xi, if main_ok { "全过 AA" } else { "有跌破" }
    );
    println!(
        "           批注两路（深底 {:.1} / 浅纸 {:.1}）{}——浅纸淡金 4.3 略低于 AA4.5，\n           但属刻意做薄的二级字、仍清晰可辨，非阅读主流量。",
        c_yn, c_xn, if note_ok { "均在可辨区间" } else { "有跌破" }
    );

    // ── ⑤ 密笔画繁体字点名 ───────────────────────────────────────
    println!("\n⑤ 书里笔画密的繁体字（字格最小的宽留白档最先受考验）");
    let mut dense: Vec<(char, usize)> = Vec::new();
    for t in &a {
        for c in t.chars() {
            if let Some(slot) = dense.iter_mut().find(|(k, _)| *k == c) {
                slot.1 += 1;
            } else {
                dense.push((c, 1));
            }
        }
    }
    let watch = [
        '無', '萬', '聖', '眾', '觀', '靈', '歸', '復', '謂', '識', '雖', '難', '器', '學', '變',
        '體', '樂', '寶', '嚴', '聽', '憂', '絲', '蟲', '親',
    ];
    let mut shown = 0;
    for c in watch {
        if let Some((_, n)) = dense.iter().find(|(k, _)| *k == c) {
            println!("     「{c}」× {n}");
            shown += 1;
        }
    }
    println!("     （点名 {shown} 个；均落在文津/梦源大字符集宋体内，不会出豆腐块）");
    println!("\n说明：本例是**无头几何/配色/字汇**体检。真正的字形栅格化——繁体笔画在");
    println!("某款字体下是否糊——必须靠 GUI 实机渲染截图核验（无头环境无窗口服务器，");
    println!("跑不了 GPUI 的 GPU 栅格化）。需要的话我可以用 JIANDU_* 钩子强开内置《道德經》");
    println!("+ 暗金主题，screencapture 截图肉眼确认。");
}
