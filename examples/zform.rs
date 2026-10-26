//! 双行夹注（双列小字）Z 字型布局 · 控制台验证程序。
//!
//! 用法：
//!   cargo run --release --example zform --manifest-path /Users/huangjianrong/Downloads/jiandu/Cargo.toml
//!
//! 只算坐标、不渲染：打印每个正文大字与注文小字的 (x, y, w, h, 字号)，
//! 并对「行高对齐」「Z 字型交替」「半格守恒」做即时断言。

use jiandu::gloss::{parse_annotated, z_layout, Seg};

/// 取网格引擎同款量纲：colWidth=46、rowHeight=1.1×colWidth、fontSize=0.75×colWidth。
const COL_W: f32 = 46.0;
const ROW_H: f32 = 1.1 * COL_W;
const FS: f32 = 0.75 * COL_W;

fn main() {
    // 带标记的样文：正文里两处夹注（一短一长，含奇数字符数）。
    let text = "道可道[注]常，非恒常[/注]非常道[注]道本无名，强名为道，故可道即非真道，此破道名也[/注]名可名非常名";

    println!("== 样文的标记解析 ==");
    for seg in parse_annotated(text) {
        match seg {
            Seg::Text(t) => println!("  正文：{t}"),
            Seg::Note(t) => println!("  注文：{t}"),
        }
    }

    println!();
    println!(
        "== Z 字型双行夹注 · 逐字坐标 ==\n  colWidth={COL_W}  rowHeight={ROW_H:.2}  fontSize={FS}  noteFontSize={:.2}",
        FS * 0.5
    );
    println!(
        "  {:>3}  {:<4} {:>8} {:>8} {:>7} {:>7} {:>7}  说明",
        "seq", "字", "x", "y", "w", "h", "fs"
    );
    println!("  {}", "-".repeat(64));

    let cells = z_layout(text, COL_W, ROW_H, FS);
    for c in &cells {
        let side = if !c.note {
            "正文大字".to_string()
        } else if c.x.abs() < f32::EPSILON {
            format!("注文·左小列(r{})", (c.y / (ROW_H / 2.0)).round() as i32)
        } else {
            format!("注文·右小列(r{})", (c.y / (ROW_H / 2.0)).round() as i32)
        };
        println!(
            "  {:>3}  {:<4} {:>8.2} {:>8.2} {:>7.2} {:>7.2} {:>7.2}  {}",
            c.seq, c.ch, c.x, c.y, c.w, c.h, c.fs, side
        );
    }

    // ---- 即时断言：数量级 / 守恒 / 对齐 ----
    let sub_row = ROW_H / 2.0;
    let sub_w = COL_W / 2.0;

    // ① 注文小字只有两个 x：0（左小列）与 col_w/2（右小列）。
    let mut note_xs: Vec<f32> = cells
        .iter()
        .filter(|c| c.note)
        .map(|c| (c.x * 100.0).round() / 100.0)
        .collect();
    note_xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    note_xs.dedup();
    println!();
    println!("== 断言 ==");
    println!(
        "  ① 注文小字 x 取值集合 = {note_xs:?}  → 期望 [0.0, {}]（左/右小列）",
        sub_w
    );
    assert_eq!(note_xs, vec![0.0, sub_w], "小列宽度必须是 colWidth/2");

    // ② 行高对齐：同一注文块内，相邻小字行距 = row_h/2；
    //    且 2 × 小行高严格 == 正文行高（两个上下小字 = 一大字 + 行间距）。
    let notes: Vec<&_> = cells.iter().filter(|c| c.note).collect();
    let ys: Vec<f32> = notes.iter().map(|c| (c.y * 100.0).round() / 100.0).collect();
    let min_y = ys.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let rows = ((max_y - min_y) / sub_row).round() as i32 + 1;
    println!(
        "  ② 块内小字行数 = {rows}，块高 = {} 半格；2×小行高 {:.2} == 正文行高 {:.2} → {}",
        (max_y - min_y) / sub_row + 1.0,
        2.0 * sub_row,
        ROW_H,
        (2.0 * sub_row - ROW_H).abs() < 1e-3
    );
    assert!((2.0 * sub_row - ROW_H).abs() < 1e-3, "行高必须严格对齐");

    // ③ Z 字型右先左后：块内前 half=ceil(len/2) 字进右小列（读序在前），
    //    余下进左小列；两小列各自自上而下，块与块之间重新起算。
    let lens: Vec<usize> = parse_annotated(text)
        .iter()
        .filter_map(|s| match s {
            Seg::Note(t) => Some(t.chars().count()),
            _ => None,
        })
        .collect();
    let mut ok_alt = true;
    let mut off = 0usize;
    for len in &lens {
        let half = len.div_ceil(2);
        let anchor = notes[off].y;
        for k in 0..*len {
            let (want_x, want_row) = if k < half {
                (sub_w, k) // 右小列第 k 行
            } else {
                (0.0, k - half) // 左小列第 (k-half) 行
            };
            if (notes[off + k].x - want_x).abs() > 1e-3 {
                ok_alt = false;
            }
            if (notes[off + k].y - (anchor + want_row as f32 * sub_row)).abs() > 1e-3 {
                ok_alt = false;
            }
        }
        off += *len;
    }
    println!(
        "  ③ Z 字型右先左后（块内前 half 字 → 右小列，余下 → 左小列，各自自上而下）→ {ok_alt}"
    );
    assert!(ok_alt, "Z 字型填充顺序不符");

    // ④ 半格守恒：注文字符数全在（块数 = 注文条数，字数与源一致）。
    let src_notes: usize = parse_annotated(text)
        .iter()
        .filter_map(|s| match s {
            Seg::Note(t) => Some(t.chars().count()),
            _ => None,
        })
        .sum();
    println!(
        "  ④ 注文字数守恒：布局 {} 字 == 源标记 {} 字 → {}",
        notes.len(),
        src_notes,
        notes.len() == src_notes
    );
    assert_eq!(notes.len(), src_notes, "注文字符丢失");

    // ⑤ 字总数（正文 + 注文）与去标记文本一致。
    let stripped: String = parse_annotated(text)
        .iter()
        .map(|s| match s {
            Seg::Text(t) => t.clone(),
            Seg::Note(t) => t.clone(),
        })
        .collect();
    println!(
        "  ⑤ 全字守恒：布局 {} 字 == 去标记文本 {} 字 → {}",
        cells.len(),
        stripped.chars().count(),
        cells.len() == stripped.chars().count()
    );
    assert_eq!(cells.len(), stripped.chars().count());

    println!();
    println!("全部断言通过：双行夹注 Z 字型坐标计算无误。");
}
