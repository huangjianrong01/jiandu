//! 清洗管线 A/B 基准：同一套「去噪 + 括号夹注 + 异体归一」变换，
//! 对比「每段都返回新 `String`（旧调用模式）」与「线程局部缓冲区复用（新）」。
//!
//! 跑法：`cargo run --release --example bench_clean -p jiandu-core`

use std::hint::black_box;
use std::time::Instant;

use jiandu_core::cleaner::{clean_line, denoise_text, normalize_variants, paren_to_note};

/// 旧调用模式：三段各返回新 `String`，每段付 ~3~4 次全量分配。
fn naive_clean_line(s: &str) -> String {
    let t = denoise_text(s);
    if t.is_empty() || jiandu_core::cleaner::is_noise_para(&t) {
        return String::new();
    }
    normalize_variants(&paren_to_note(&t))
}

fn main() {
    // 一段「极端古籍」风格的样本：正文 + 拼音串（应删）+ 括号夹注（应升格）
    // + 异体字（应归一），夹杂标点。
    let para = "道可道非常道名可名非常名無名天地之始dào3（注文一則）峯巒疊翠故常無欲以觀其妙【題解】玄之又玄眾妙之門pinyinrun（小注二）";
    let n: usize = 200_000;
    let inputs: Vec<String> = (0..n).map(|_| para.to_string()).collect();
    let total_in: usize = inputs.iter().map(|s| s.len()).sum();

    // 预热（JIT / 页缓存稳定）。
    for s in &inputs {
        black_box(clean_line(s));
    }

    let t0 = Instant::now();
    let mut out_new = 0usize;
    for s in &inputs {
        out_new += clean_line(s).len();
    }
    let dt_new = t0.elapsed();

    let t0 = Instant::now();
    let mut out_old = 0usize;
    for s in &inputs {
        out_old += naive_clean_line(s).len();
    }
    let dt_old = t0.elapsed();

    let mb = total_in as f64 / 1e6;
    println!("输入：{} 段，{:.1} MB", n, mb);
    println!(
        "新 clean_line（缓冲区复用）：{:?}  -> {:.1} MB/s",
        dt_new,
        mb / dt_new.as_secs_f64()
    );
    println!(
        "旧 naive（每段全量分配）：  {:?}  -> {:.1} MB/s",
        dt_old,
        mb / dt_old.as_secs_f64()
    );
    println!(
        "加速比：{:.2}x   输出字节一致：{}",
        dt_old.as_secs_f64() / dt_new.as_secs_f64(),
        out_new == out_old
    );
}
