//! **整本导出内存验证（v1.85.5）**
//!
//! 用与 App 完全相同的内核 API 复现「整本导出」的两条布局路径，并让外部用
//! `/usr/bin/time -l` 测量峰值 RSS：
//!
//!   * `whole`  —— 旧路径：`layout_book_from_opts` 一次性把全书排成 `Vec<Page>`
//!                 全攒在内存里（这是极巨著布局阶段内存随字数爆炸的根因）；
//!   * `stream` —— 新路径：`layout_book_streaming` 逐章排版、每章排完即吐出、
//!                 回调返回后即释放，内存里同一时刻最多只有「一章」的页面。
//!
//! 运行（在仓库根目录）：
//!   cargo build --release --example stream_mem -p jiandu-core
//!   /usr/bin/time -l ./target/release/examples/stream_mem whole  60 3000
//!   /usr/bin/time -l ./target/release/examples/stream_mem stream 300 3000
//!
//! 预期：`stream` 的峰值 RSS 随章节数**基本不变**（只受单章厚度约束）；
//! `whole` 的峰值 RSS 随章节数**近似线性增长**。

use jiandu_core::gloss::{layout_book_from_opts, layout_book_streaming, GlossPayload, Page};
use jiandu_core::types::{Glyph, GlyphKind};

const COLS: usize = 12;
const ROWS: usize = 22;
const RESERVE: usize = 0;

fn gloss_none(_g: &Glyph) -> Option<GlossPayload> {
    None
}

fn build_flow(chapters: usize, chars_per_chapter: usize) -> Vec<Glyph> {
    let mut flow = Vec::with_capacity(chapters * chars_per_chapter);
    for ch in 0..chapters {
        for i in 0..chars_per_chapter {
            flow.push(Glyph {
                chapter: ch,
                para: 0,
                offset: i,
                ch: '經',
                kind: GlyphKind::Char,
            });
        }
    }
    flow
}

fn run_whole(flow: &[Glyph]) -> usize {
    // 旧路径：全攒。
    let pages: Vec<Page> = layout_book_from_opts(flow, &gloss_none, COLS, ROWS, RESERVE, true, false);
    let n = pages.len();
    // 故意在末尾保活，模拟「导出前必须持有整本产物」的真实占用。
    println!("mode=whole total_pages={} held_pages={}", n, pages.len());
    n
}

fn run_stream(flow: &[Glyph]) -> (usize, usize) {
    // 新路径：逐章吐出，回调返回即释放。
    let mut total: usize = 0;
    let mut max_chapter_pages: usize = 0;
    layout_book_streaming(
        flow,
        gloss_none,
        COLS,
        ROWS,
        RESERVE,
        true,
        false,
        |_order, pages| {
            total += pages.len();
            max_chapter_pages = max_chapter_pages.max(pages.len());
            // 真实导出此处会 `stream.push(&leaf)` 写盘并随即释放 `pages`。
        },
    );
    println!(
        "mode=stream total_pages={} max_simultaneously_held_pages={}",
        total, max_chapter_pages
    );
    (total, max_chapter_pages)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: {} <whole|stream> <chapters> <chars_per_chapter>", args[0]);
        std::process::exit(2);
    }
    let mode = args[1].as_str();
    let chapters: usize = args[2].parse().expect("chapters");
    let cpc: usize = args[3].parse().expect("chars_per_chapter");
    let flow = build_flow(chapters, cpc);
    println!(
        "flow_glyphs={} chapters={} chars_per_chapter={}",
        flow.len(),
        chapters,
        cpc
    );
    match mode {
        "whole" => {
            run_whole(&flow);
        }
        "stream" => {
            run_stream(&flow);
        }
        other => {
            eprintln!("unknown mode: {other}");
            std::process::exit(2);
        }
    }
    println!("done");
}
