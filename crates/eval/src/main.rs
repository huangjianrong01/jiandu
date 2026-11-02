//! `jiandu-eval` 命令行入口（nightly 跑分 / 纠正沉淀）。
//!
//! 子命令：
//!   run   --gold G.jsonl --pred P.jsonl [--out metrics.json]
//!         对金标集跑预测，打印逐条 + 聚合报告，可选落盘 metrics。
//!   fold  --corrections C.jsonl --gold G.jsonl --out O.jsonl
//!         把用户纠正并回金标（「改了=负例」自动沉淀）。
//!   help  打印用法。

use jiandu_eval::gold::{fold_corrections, load_corrections, load_gold, load_predictions};
use jiandu_eval::{evaluate, Aggregate, CaseReport};
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

fn print_help() {
    let mut o = std::io::stderr();
    let _ = writeln!(o, "笺读评测台 jiandu-eval");
    let _ = writeln!(o, "  run   --gold G.jsonl --pred P.jsonl [--out metrics.json]");
    let _ = writeln!(o, "  fold  --corrections C.jsonl --gold G.jsonl --out O.jsonl");
    let _ = writeln!(o, "  help");
    let _ = writeln!(
        o,
        "金标/预测均为 JSONL，每行一条；按 id 对齐。格式见 crates/eval/data/*.sample.jsonl"
    );
}
