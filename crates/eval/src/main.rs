//! `jiandu-eval` 命令行入口（nightly 跑分 / 纠正沉淀 / 真实书目集成）。
//!
//! 子命令：
//!   run    --gold G.jsonl --pred P.jsonl [--out metrics.json]
//!          对金标集跑预测，打印逐条 + 聚合报告，可选落盘 metrics。
//!   fold   --corrections C.jsonl --gold G.jsonl --out O.jsonl
//!          把用户纠正并回金标（「改了=负例」自动沉淀）。
//!   real   [--books-dir DIR] [--out metrics.json]
//!          从打包书目（道德经/唐诗…）抽金标，用真实 jiandu_core::classicize
//!          清洗链做预测——既跑真实代码，又当回归守卫（清洗吞标点 F1 立刻掉）。
//!   help   打印用法。

use jiandu_core::pipeline::classicize;
use jiandu_eval::gold::{fold_corrections, load_corrections, load_gold, load_predictions, GoldCase, Prediction};
use jiandu_eval::score::punct_type_mismatches;
use jiandu_eval::{evaluate, Aggregate, CaseReport};
use std::collections::BTreeMap;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args);
    std::process::exit(code);
}

fn run(args: &[String]) -> i32 {
    let Some(cmd) = args.first() else {
        print_help();
        return 1;
    };
    match cmd.as_str() {
        "run" => cmd_run(args),
        "fold" => cmd_fold(args),
        "real" => cmd_real(args),
        "help" | "--help" | "-h" => {
            print_help();
            0
        }
        other => {
            eprintln!("未知子命令：{other}\n");
            print_help();
            1
        }
    }
}

fn opt<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn cmd_run(args: &[String]) -> i32 {
    let Some(gold_path) = opt(args, "--gold") else {
        eprintln!("run 需要 --gold <path>");
        return 1;
    };
    let Some(pred_path) = opt(args, "--pred") else {
        eprintln!("run 需要 --pred <path>");
        return 1;
    };
    let gold = match load_gold(gold_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("读金标失败：{e}");
            return 1;
        }
    };
    let pred = match load_predictions(pred_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("读预测失败：{e}");
            return 1;
        }
    };

    // 默认笺注走启发式基线；接真实 LLM 裁判时在此传入回调（见 judge 模块说明）。
    let (reports, agg) = evaluate(&gold, &pred, None);

    print_reports(&reports, &agg);

    if let Some(out) = opt(args, "--out") {
        let payload = serde_json::json!({ "aggregate": agg, "cases": reports });
        match std::fs::write(out, serde_json::to_string_pretty(&payload).unwrap()) {
            Ok(_) => eprintln!("已写指标：{out}"),
            Err(e) => eprintln!("写指标失败：{e}"),
        }
    }
    0
}

fn print_reports(reports: &[CaseReport], agg: &Aggregate) {
    println!("┌──── 笺读评测台 · 逐条 ────");
    for r in reports {
        let py = r
            .pinyin
            .as_ref()
            .map(|p| format!("拼音 {:.0}%", p.accuracy * 100.0))
            .unwrap_or_else(|| "拼音 —".into());
        let an = r
            .annotation
            .as_ref()
            .map(|a| format!("笺注 {:.2}({})", a.score, a.method))
            .unwrap_or_else(|| "笺注 —".into());
        println!(
            "│ [{id}] 标点F1 位置{pp:.2}/类型{pt:.2} · 文本留存{tx:.0}% · {py} · {an}",
            id = r.id,
            pp = r.punct_pos.f1,
            pt = r.punct_type.f1,
            tx = r.text.preservation * 100.0,
            py = py,
            an = an,
        );
    }
    println!("└──── 聚合（{n} 用例）────", n = agg.cases);
    println!(
        "  标点F1  位置 {:.3} | 类型 {:.3}",
        agg.punct_pos_f1, agg.punct_type_f1
    );
    println!("  文本留存 {:.3}", agg.text_preservation);
    println!(
        "  注音正确 {:.3} | AI笺注 {:.3}",
        agg.pinyin_acc, agg.annotation_score
    );
}

fn cmd_fold(args: &[String]) -> i32 {
    let Some(corr_path) = opt(args, "--corrections") else {
        eprintln!("fold 需要 --corrections <path>");
        return 1;
    };
    let Some(gold_path) = opt(args, "--gold") else {
        eprintln!("fold 需要 --gold <path>");
        return 1;
    };
    let Some(out) = opt(args, "--out") else {
        eprintln!("fold 需要 --out <path>");
        return 1;
    };
    let mut gold = match load_gold(gold_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("读金标失败：{e}");
            return 1;
        }
    };
    let corr = match load_corrections(corr_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("读纠正失败：{e}");
            return 1;
        }
    };
    fold_corrections(&mut gold, &corr);

    let mut buf = String::new();
    for g in &gold {
        buf.push_str(&serde_json::to_string(g).unwrap());
        buf.push('\n');
    }
    if let Err(e) = std::fs::write(out, buf) {
        eprintln!("写金标失败：{e}");
        return 1;
    }
    eprintln!("已并回 {}/{} 条纠正 → {out}", corr.len(), gold.len());
    0
}

/// 书目 JSON 的极简投影——只取评测要用的字段，避免耦合 `Book` 完整 schema。
#[derive(serde::Deserialize)]
struct BookFile {
    title: String,
    chapters: Vec<Chapter>,
}
#[derive(serde::Deserialize)]
struct Chapter {
    #[serde(default)]
    _title: Option<String>,
    paragraphs: Vec<Paragraph>,
}
#[derive(serde::Deserialize)]
struct Paragraph {
    text: String,
}

/// 真实书目集成：金标 = 书目段落原文的标准标点；预测 = `classicize(原文)`。
///
/// 打包书目本身就是「标准标点」的权威来源，而 `classicize` 是导入清洗总链。
/// 拿它俩对撞，既跑通了真实 `jiandu_core` 代码，又天然是回归守卫：
/// 哪天 `classicize` 开始吞标点 / 移位，F1 立刻掉。
fn cmd_real(args: &[String]) -> i32 {
    let books_dir = opt(args, "--books-dir")
        .unwrap_or("crates/core/resources/books")
        .to_string();
    let out = opt(args, "--out").map(|s| s.to_string());

    let Ok(entries) = std::fs::read_dir(&books_dir) else {
        eprintln!("读书目目录失败：{books_dir}");
        return 1;
    };
    let mut book_files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let is_json = p.extension().and_then(|x| x.to_str()) == Some("json");
            is_json.then(|| p.to_string_lossy().into_owned())
        })
        .collect();
    book_files.sort();

    if book_files.is_empty() {
        eprintln!("目录下没有书目 JSON：{books_dir}");
        return 1;
    }

    let mut all_gold: Vec<GoldCase> = Vec::new();
    let mut all_pred: Vec<Prediction> = Vec::new();
    let mut per_book: Vec<(String, usize, Aggregate)> = Vec::new();
    let mut total_punct = 0usize;
    // 类型错配直方图：(gold_kind, pred_kind) -> 次数。解释 by_type F1 为何掉。
    let mut mismatch: BTreeMap<(char, char), usize> = BTreeMap::new();

    for path in &book_files {
        let raw = match std::fs::read_to_string(path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("读 {path} 失败：{e}");
                continue;
            }
        };
        let book: BookFile = match serde_json::from_str(&raw) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("解析 {path} 失败：{e}");
                continue;
            }
        };
        let mut golds: Vec<GoldCase> = Vec::new();
        let mut preds: Vec<Prediction> = Vec::new();
        for (ci, ch) in book.chapters.iter().enumerate() {
            for (pi, par) in ch.paragraphs.iter().enumerate() {
                let t = par.text.trim();
                // 太短或纯空白的段落没有评测意义，跳过以免噪声。
                if t.chars().count() < 4 {
                    continue;
                }
                let id = format!("{}#{}:{}", book.title, ci, pi);
                let cleaned = classicize(t);
                total_punct += jiandu_eval::score::extract_puncts(t).len();
                golds.push(GoldCase {
                    id: id.clone(),
                    text: t.to_string(),
                    ..Default::default()
                });
                preds.push(Prediction {
                    id,
                    text: Some(cleaned),
                    ..Default::default()
                });
            }
        }
        // 逐段收集类型错配（位置对齐、kind 不同）。
        for (g, p) in golds.iter().zip(preds.iter()) {
            let g_spans = jiandu_eval::score::extract_puncts(&g.text);
            let p_spans = jiandu_eval::score::extract_puncts(p.text.as_deref().unwrap_or(""));
            for (ga, pb) in punct_type_mismatches(&g_spans, &p_spans) {
                *mismatch.entry((ga, pb)).or_insert(0) += 1;
            }
        }
        let (_rep, agg) = evaluate(&golds, &preds, None);
        per_book.push((book.title.clone(), golds.len(), agg));
        all_gold.extend(golds);
        all_pred.extend(preds);
    }

    let (_rep, agg) = evaluate(&all_gold, &all_pred, None);

    println!("┌──── 笺读评测台 · 真实书目集成 ────");
    println!(
        "│ 书目 {} 本 · 用例 {} 条 · 金标标点 {} 枚（classicize 幂等守卫）",
        per_book.len(),
        agg.cases,
        total_punct
    );
    for (title, n, a) in &per_book {
        println!(
            "│ 《{t}》 {n} 段 · 标点F1 位置{a_pp:.3}/类型{a_pt:.3} · 文本留存{a_tx:.3}",
            t = title,
            n = n,
            a_pp = a.punct_pos_f1,
            a_pt = a.punct_type_f1,
            a_tx = a.text_preservation,
        );
    }
    println!("└──── 聚合（{n} 用例）────", n = agg.cases);
    println!(
        "  标点F1  位置 {:.3} | 类型 {:.3}",
        agg.punct_pos_f1, agg.punct_type_f1
    );
    println!("  文本留存 {:.3}", agg.text_preservation);
    println!(
        "  注音正确 {:.3} | AI笺注 {:.3}",
        agg.pinyin_acc, agg.annotation_score
    );

    // 类型错配直方图：解释 by_type F1 为何 < 1（合法归一化 vs 隐患）。
    if !mismatch.is_empty() {
        println!("┌──── 标点类型错配（gold→pred，按次数）────");
        let mut rows: Vec<((char, char), usize)> = mismatch.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        for ((ga, pb), n) in rows.iter().take(12) {
            println!(
                "│  {g} → {p}   ×{n}",
                g = escape_punct(*ga),
                p = escape_punct(*pb),
                n = n
            );
        }
        println!("└────────────────────────────────────");
    }

    if let Some(out) = out {
        let payload = serde_json::json!({ "aggregate": agg, "per_book": per_book, "total_punct": total_punct });
        match std::fs::write(out.clone(), serde_json::to_string_pretty(&payload).unwrap()) {
            Ok(_) => eprintln!("已写指标：{out}"),
            Err(e) => eprintln!("写指标失败：{e}"),
        }
    }
    0
}

/// 把标点字符打印成可读形式（空白/空字符退化为 U+XXXX）。
fn escape_punct(c: char) -> String {
    if c.is_whitespace() || c == '\u{0000}' {
        format!("U+{:04X}", c as u32)
    } else {
        c.to_string()
    }
}

fn print_help() {
    let mut o = std::io::stderr();
    let _ = writeln!(o, "笺读评测台 jiandu-eval");
    let _ = writeln!(o, "  run    --gold G.jsonl --pred P.jsonl [--out metrics.json]");
    let _ = writeln!(o, "  fold   --corrections C.jsonl --gold G.jsonl --out O.jsonl");
    let _ = writeln!(o, "  real   [--books-dir DIR] [--out metrics.json]  · 真实书目 + classicize 回归守卫");
    let _ = writeln!(o, "  help");
    let _ = writeln!(
        o,
        "金标/预测均为 JSONL，每行一条；按 id 对齐。格式见 crates/eval/data/*.sample.jsonl"
    );
}
