//! **字表 / 字体体检命令行工具**（v1.9.78）。
//!
//! 用途：在把一份古籍喂进笺读之前，先算清楚「会不会出豆腐块、能救回多少」。
//!
//! ```bash
//! # 只看字体覆盖与可抢救字数
//! cargo run -p jiandu-core --example variant_audit -- \
//!     crates/app/assets/fonts/qiji.ttf  ~/Downloads/chinese-philosophy-main/cjk/cjk.txt
//!
//! # 再挂一份外部异体字表
//! cargo run -p jiandu-core --example variant_audit -- \
//!     crates/app/assets/fonts/qiji.ttf  book.txt  my_variants.tsv
//!
//! # 顺手导出「折叠后」的文本，肉眼比对有没有改错字
//! JIANDU_AUDIT_OUT=out.txt cargo run -p jiandu-core --example variant_audit -- font.ttf book.txt
//! ```
//!
//! ⚠️ 工具只**报告**和**示范**，绝不偷偷改用户的书：`rescue_missing` 只在字体
//! 确实没有字形时才折字，且折不了的字原样保留。

use std::env;
use std::fs;

use jiandu_core::glyph::GlyphSet;
use jiandu_core::variants::{self, VariantTable};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("用法: variant_audit <字体.ttf/ttc/otf> <文本或字表> [外部异体字表]");
        std::process::exit(2);
    }

    let font_bytes = fs::read(&args[0]).unwrap_or_else(|e| {
        eprintln!("读不到字体 {}: {e}", args[0]);
        std::process::exit(1);
    });
    let glyphs = GlyphSet::from_bytes(&font_bytes).unwrap_or_else(|e| {
        eprintln!("字体解析失败：{e}");
        std::process::exit(1);
    });

    let text_bytes = fs::read(&args[1]).unwrap_or_else(|e| {
        eprintln!("读不到文本 {}: {e}", args[1]);
        std::process::exit(1);
    });
    let (text, enc) = variants::decode_bytes(&text_bytes);

    // 外部表：解析成独立表，再合到内置基线上；不写磁盘、不动用户数据。
    let mut table = VariantTable::builtin().clone();
    if let Some(p) = args.get(2) {
        let b = fs::read(p).unwrap_or_else(|e| {
            eprintln!("读不到外部字表 {p}: {e}");
            std::process::exit(1);
        });
        let (d, _e) = variants::decode_bytes(&b);
        let outcome = VariantTable::parse(&d);
        if outcome.rows.is_empty() {
            eprintln!("外部字表一行都没认出来（{}）", outcome.format().label());
            std::process::exit(1);
        }
        let (ext, rep) = VariantTable::from_text(&d);
        table.merge(&ext);
        println!("外部字表 {p}：{}", rep.summary());
    }

    let tofu = variants::tofu_chars(&text, &glyphs);
    // 严格档 = 只折异体（这个结果才可以写回正文）；宽松档 = 含繁简降级，
    // 只准给渲染器临时用，绝不落库。两个数分开报，别混为一谈。
    let (rescued, rep) = variants::rescue_missing(&text, &glyphs, &table);
    let (_, disp) = variants::rescue_missing_display(&text, &glyphs, &table);

    println!("──────── 字体覆盖体检 ────────");
    println!("字体      : {}", args[0]);
    println!("字形码位  : {}（{} 个区间）", glyphs.len(), glyphs.range_count());
    println!("文本编码  : {enc}");
    println!("文本字数  : {}", text.chars().count());
    println!("汉字覆盖率: {:.2}%", glyphs.coverage(&text) * 100.0);
    println!("缺字形汉字: {} 个（去重）", tofu.len());
    if !tofu.is_empty() {
        let sample: String = tofu.iter().take(40).collect();
        println!("缺字样本  : {sample}");
    }
    println!();
    println!("──────── 映射表能救回多少 ────────");
    println!("激活表    : {}", variants::active_summary());
    println!("成功折回  : {} 字（严格档：仅异体归一，可写回正文）", rep.rescued.len());
    let sample: String = rep
        .rescued
        .iter()
        .take(20)
        .map(|(a, b)| format!("{a}→{b} "))
        .collect();
    if !sample.is_empty() {
        println!("折回样本  : {sample}");
    }
    println!(
        "显示层降级: {} 字（含繁简折算，仅屏幕临时替换，绝不落库）",
        disp.rescued.len()
    );
    println!("折了仍缺字: {} 字（只有换字体能救）", rep.hopeless.len());
    println!("未进表生僻: {} 字（需人工核对 / 补字表）", rep.unknown.len());
    println!();
    println!("结论：映射表能消除的是「异体字」的豆腐块；正字本身缺字形只能靠字体回退。");

    if let Ok(out) = env::var("JIANDU_AUDIT_OUT") {
        fs::write(&out, rescued).unwrap_or_else(|e| eprintln!("写 {out} 失败：{e}"));
        println!("\n严格档折叠后文本已写到 {out}（原文未改动）");
    }
}
