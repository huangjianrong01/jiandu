//! 双行夹注 · 溢出与流转（同页流转 + 跨页 pendingNote）· 控制台验证程序。
//!
//! 用法：
//!   cargo run --release --example zflow --manifest-path /Users/huangjianrong/Downloads/jiandu/Cargo.toml
//!
//! 只算坐标、不渲染：按「面 → 列 → 半格行」打印每个字的落点，
//! 并对四条流转规格做即时断言：
//!   ① 列底不截断：注文排到列底还没完，绝不截断、一字不丢；
//!   ② 同页流转：剩余注文流到本面下一列，该列是纯注文列（不再排正文）；
//!   ③ 跨页流转：本面排满后余文进 pendingNote，新面首列优先续排缓存注文；
//!   ④ 主进度指针：ci 每消费一个原文字符 +1（恒等于全字计数），
//!      块内读序 note_idx 连续（竖排右先左后的依据）。

use jiandu::gloss::{layout_z_flow_ci, layout_z_flow_pages, parse_annotated, Seg, ZFlowCell};

/// 网格量纲与 zform 一致：colWidth=46、rowHeight=1.1×colWidth。
const COL_W: f32 = 46.0;
const ROW_H: f32 = 1.1 * COL_W;
const SUB_ROW: f32 = ROW_H / 2.0;
/// 版面：3 列 × 2 行 → 每列 cap=4 半格，一面 12 半格。
const COLS: usize = 3;
const ROWS: usize = 2;
const CAP: usize = ROWS * 2;

fn main() {
    // 样文：3 个正文 + 19 字长注（超过单列 4 半格，也超过单面 12 半格）+ 3 个正文。
    let text = "道可道[注]恒先天地生，寂兮寥兮独立不改，周行不殆[/注]非常道";

    println!("== 样文的标记解析 ==");
    for seg in parse_annotated(text) {
        match seg {
            Seg::Text(t) => println!("  正文：{t}"),
            Seg::Note(t) => println!("  注文：{t}"),
        }
    }

    let pages = layout_z_flow_pages(text, COLS, ROWS, COL_W);
    println!();
    println!(
        "== 逐面 · 逐列 · 半格行 ==\n  cols={COLS} rows={ROWS} → cap={CAP} 半格/列，一面 {} 半格",
        COLS * CAP
    );
    println!("  （列序为 DOM 从左数；竖排渲染时右 → 左镜像）");

    for (pi, pg) in pages.iter().enumerate() {
        println!("  第 {pi} 面");
        for col in 0..COLS {
            let cells: Vec<&ZFlowCell> = pg.iter().filter(|c| c.col == col).collect();
            if cells.is_empty() {
                continue;
            }
            let kind = if cells.iter().all(|c| c.note) {
                "纯注文列（同页流转列）"
            } else {
                "正文列"
            };
            println!("    列 {col}（{kind}）");
            // 按 y 归并半格行。
            let mut rows: Vec<(f32, Vec<&ZFlowCell>)> = Vec::new();
            for c in cells {
                if let Some(last) = rows.last_mut() {
                    if (last.0 - c.y).abs() < 1e-3 {
                        last.1.push(c);
                        continue;
                    }
                }
                rows.push((c.y, vec![c]));
            }
            for (y, cs) in rows {
                let c0 = cs[0];
                if !c0.note {
                    println!(
                        "      y={y:>7.2}  正文「{}」 x={:>6.2} w={:>5.2} h={:>6.2} fs={:>6.2}",
                        c0.ch, c0.x, c0.w, c0.h, c0.fs
                    );
                } else {
                    let l = cs
                        .iter()
                        .find(|c| c.sub == 0)
                        .map(|c| c.ch)
                        .unwrap_or('　');
                    let r = cs
                        .iter()
                        .find(|c| c.sub == 1)
                        .map(|c| c.ch)
                        .unwrap_or('　');
                    let idxs: Vec<String> =
                        cs.iter().map(|c| format!("{}", c.note_idx)).collect();
                    println!(
                        "      y={y:>7.2}  注文 左「{l}」右「{r}」 idx={} x={:>6.2} w={:>5.2} h={:>6.2} fs={:>6.2}",
                        idxs.join(","),
                        c0.x,
                        c0.w,
                        c0.h,
                        c0.fs
                    );
                }
            }
        }
    }

    // ---------------- 断言 ----------------
    let flat: Vec<&ZFlowCell> = pages.iter().flat_map(|p| p.iter()).collect();
    let stripped = parse_annotated(text)
        .iter()
        .map(|s| match s {
            Seg::Text(t) => t.chars().count(),
            Seg::Note(t) => t.chars().count(),
        })
        .sum::<usize>();
    let main: String = flat.iter().filter(|c| !c.note).map(|c| c.ch).collect();
    let note: String = flat.iter().filter(|c| c.note).map(|c| c.ch).collect();

    println!();
    println!("== 断言 ==");

    // ① 列底不截断：全字守恒、双流各自保序。
    println!(
        "  ① 全字守恒：布局 {} 字 == 源文本 {} 字 → {}；正文「{main}」/ 注文「{note}」各保序",
        flat.len(),
        stripped,
        flat.len() == stripped
    );
    assert_eq!(flat.len(), stripped, "列底不截断：一字不能少");
    assert_eq!(main, "道可道非常道");
    assert_eq!(note, "恒先天地生，寂兮寥兮独立不改，周行不殆");

    // ② 同页流转列是纯注文列（只排小字、不再排正文）。
    let mut note_only_cols: Vec<(usize, usize)> = Vec::new();
    for (pi, pg) in pages.iter().enumerate() {
        for col in 0..COLS {
            let cs: Vec<&ZFlowCell> = pg.iter().filter(|c| c.col == col).collect();
            if !cs.is_empty() && cs.iter().all(|c| c.note) {
                note_only_cols.push((pi, col));
            }
        }
    }
    println!("  ② 同页流转纯注文列（面, 列）：{note_only_cols:?}");
    assert!(!note_only_cols.is_empty(), "应流转出至少一个纯注文列");

    // ③ 跨页 pendingNote：新面首列优先续排缓存注文（排在后续正文之前）。
    let first_p1 = pages[1]
        .iter()
        .min_by(|a, b| {
            (a.col, a.y)
                .partial_cmp(&(b.col, b.y))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();
    let max_idx_p0 = pages[0]
        .iter()
        .filter(|c| c.note)
        .map(|c| c.note_idx)
        .max()
        .unwrap();
    println!(
        "  ③ 跨页 pendingNote：第 1 面首列首字「{}」(idx {}) 接续第 0 面 idx {}，且排在后续正文「非」之前",
        first_p1.ch, first_p1.note_idx, max_idx_p0
    );
    assert!(first_p1.note, "新面首列必须是缓存注文优先，不是新正文");
    assert_eq!(
        first_p1.note_idx,
        max_idx_p0 + 1,
        "缓存注文的块内读序必须跨页连续"
    );

    // ④ 主进度指针 ci 守恒 + 块内读序 note_idx 连续（右先左后）。
    let ci = layout_z_flow_ci(text, COLS, ROWS, COL_W);
    let lens: Vec<usize> = parse_annotated(text)
        .iter()
        .filter_map(|s| match s {
            Seg::Note(t) => Some(t.chars().count()),
            _ => None,
        })
        .collect();
    let idxs: Vec<usize> = flat.iter().filter(|c| c.note).map(|c| c.note_idx).collect();
    let want: Vec<usize> = lens.iter().flat_map(|&l| 0..l).collect();
    println!(
        "  ④ 主进度指针：ci={ci} == 全字 {stripped} → {}；块内 note_idx 连续 → {}",
        ci == stripped,
        idxs == want
    );
    assert_eq!(ci, stripped, "主进度指针乱掉：正文与注文交替消费必须守恒");
    assert_eq!(idxs, want, "块内读序断档：跨列 / 跨页 note_idx 必须连续");

    // 右先左后：块内前 half=ceil(len/2) 字进右小列（sub=1），余下左小列。
    let half = (note.chars().count() + 1) / 2;
    let ok_rl = flat
        .iter()
        .filter(|c| c.note)
        .all(|c| c.sub == u8::from(c.note_idx < half));
    println!("  ⑤ 竖排右先左后：前 half={half} 字进右小列、余下进左小列 → {ok_rl}");
    assert!(ok_rl);

    // ⑥ 行高对齐：小行高严格为正文行高一半。
    let mh = flat.iter().find(|c| !c.note).unwrap().h;
    let nh = flat.iter().find(|c| c.note).unwrap().h;
    println!(
        "  ⑥ 行高对齐：2 × 小行高 {:.2} == 正文行高 {:.2} → {}",
        2.0 * nh,
        mh,
        (2.0 * nh - mh).abs() < 1e-3
    );
    assert!((2.0 * nh - mh).abs() < 1e-3);

    // ⑦ 列容量：每面每列已用高度 ≤ cap 半格。
    let mut ok_cap = true;
    for (pi, pg) in pages.iter().enumerate() {
        for col in 0..COLS {
            let used = pg
                .iter()
                .filter(|c| c.col == col)
                .map(|c| c.y + c.h)
                .fold(0.0f32, f32::max);
            if used > CAP as f32 * SUB_ROW + 1e-3 {
                ok_cap = false;
                println!("    超容：第 {pi} 面第 {col} 列 已用 {used:.2} > {}", CAP as f32 * SUB_ROW);
            }
        }
    }
    println!("  ⑦ 列容量：每面每列 ≤ {CAP} 半格 → {ok_cap}");
    assert!(ok_cap);

    println!();
    println!(
        "全部断言通过：溢出不截断 / 同页流转纯注列 / 跨页 pendingNote 优先 / 主指针 ci 守恒。"
    );
}
