//! 极限压测入口（**不属于交付产物**，只给开发机跑）。
//!
//! 用法：
//! ```sh
//! cargo build -p jiandu-core --release --example stress_import
//! ./target/release/examples/stress_import /tmp/jiandu_stress_50mb.txt
//! ```
//!
//! 它跑的是**真实导入漏斗**（`importer::import_txt`），不是单独调某个清洗
//! 函数——要测的就是整条链的实际耗时与内存。想看内存峰值，用 shell 采样
//! RSS（macOS 的 `/usr/bin/time` 不支持 GNU 的 `-l`）：
//!
//! ```sh
//! ./target/release/examples/stress_import big.txt & PID=$!
//! PEAK=0; while kill -0 $PID 2>/dev/null; do
//!   R=$(ps -o rss= -p $PID 2>/dev/null | tr -d ' ')
//!   [ -n "$R" ] && [ "$R" -gt "$PEAK" ] && PEAK=$R
//! done; echo "peak RSS: $((PEAK/1024)) MB"
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

/// 只数数不落盘的 sink：用来量**流式**导入的真实内存占用。
///
/// 关键是 `push` 拿到章之后**立刻丢掉**（只累加字数），模拟真机上一章落盘
/// 随即释放的行为。若换成「全书攒齐再处理」，测出来的内存就是假的低。
#[derive(Default)]
struct CountingSink {
    chapters: usize,
    para_count: usize,
    chars: usize,
    sizes: Vec<usize>,
}

impl jiandu_core::stream::ChapterSink for CountingSink {
    fn push(&mut self, chapter: jiandu_core::types::Chapter) {
        let n: usize = chapter
            .paragraphs
            .iter()
            .map(|p| p.text.chars().count())
            .sum();
        self.chars += n;
        self.para_count += chapter.paragraphs.len();
        self.sizes.push(n);
        self.chapters += 1;
        // 拿完就丢：不持有 chapter。
        drop(chapter);
    }
    fn len(&self) -> usize {
        self.chapters
    }
    fn sizes(&self) -> &[usize] {
        &self.sizes
    }
    fn titles(&self) -> Vec<String> {
        Vec::new()
    }
    fn sample(&self, _i: usize) -> &str {
        ""
    }
    fn tick(&self, _pct: f32, _label: &str) {
        // 真机上这里驱动进度条：每篇一次，107 篇就是 107 次刷新。
    }
    fn advance(&mut self, _done: usize, _label: &str) {}
    fn drop_front(&mut self, n: usize) {
        self.chapters = self.chapters.saturating_sub(n);
    }
    fn drop_at(&mut self, _idx: &[usize]) {}
    fn set_title(&mut self, _i: usize, _t: &str) {}
}

fn main() {
    let path = std::env::args().nth(1).expect("用法：stress_import <文件>");
    let bytes = std::fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(0);

    // EPUB 走**流式**路径（`import_epub_into`，逐篇读 zip + 逐章交 sink），
    // 这才是真机上 UI 用的那条；老路径 `import_epub` 会把整本攒在内存里，
    // 拿来测内存是自欺欺人。
    let ext = PathBuf::from(&path)
        .extension()
        .and_then(|e| e.to_string_lossy().to_lowercase().to_string().into())
        .unwrap_or_default();
    if ext == "epub" {
        let t = Instant::now();
        let mut sink = CountingSink {
            chapters: 0,
            para_count: 0,
            chars: 0,
            sizes: Vec::new(),
        };
        let book = jiandu_core::importer::import_epub_into(Path::new(&path), &mut sink).unwrap();
        let el = t.elapsed();
        println!("┌─ 极限压测（EPUB 流式）：{}", path);
        println!("│ 压缩包         {:>9.2} MB", bytes as f64 / 1024.0 / 1024.0);
        println!("│ 章节数         {:>9}", sink.chapters);
        println!("│ 正文字数       {:>9}", sink.chars);
        println!("│ 书名           {}", book.title);
        println!("└─ 端到端合计    {:>9.1?}", el);
        println!(
            "   吞吐 {:.1} MB/s（按压缩包体积）",
            bytes as f64 / 1024.0 / 1024.0 / el.as_secs_f64().max(1e-9)
        );
        return;
    }

    // ① 读盘 + 解码（含加强版编码嗅探）。
    let t0 = Instant::now();
    let raw = std::fs::read(&path).expect("读不出文件");
    let t_read = t0.elapsed();

    let t1 = Instant::now();
    let text = jiandu_core::dclean::sniff_decode(&raw);
    let t_decode = t1.elapsed();

    // ② 广告整行剔除。
    let t2 = Instant::now();
    let (clean, dropped) = jiandu_core::dclean::strip_piracy_lines_counted(&text);
    let t_ad = t2.elapsed();

    // ③ **真机路径**：`import_txt_into`（逐行流式 + 每 256KB 报一次进度 +
    //    逐章交 sink 落盘）。这才是 UI 真正跑的那条；`book_from_plaintext`
    //    是老的整体路径，拿它测内存会偏乐观。
    let t3 = Instant::now();
    let mut sink = CountingSink::default();
    let book = jiandu_core::importer::import_txt_into(Path::new(&path), &mut sink).unwrap();
    let t_import = t3.elapsed();

    let paras: usize = sink.para_count;
    let chars: usize = sink.chars;
    let total = t0.elapsed();
    println!("│ 书名           {}", book.title);

    println!("┌─ 极限压测：{}", path);
    println!("│ 文件         {:>9.2} MB", bytes as f64 / 1024.0 / 1024.0);
    println!("│ 解码后字数   {:>9} 字", text.chars().count());
    println!("├─ 分阶段耗时");
    println!("│ 读盘         {:>9.1?}", t_read);
    println!("│ 解码+嗅探    {:>9.1?}", t_decode);
    println!("│ 广告剔除     {:>9.1?}  （剔 {} 行）", t_ad, dropped);
    println!("│ 切章+全角化  {:>9.1?}", t_import);
    println!("├─ 结果（真机流式路径）");
    println!("│ 章数         {:>9}", sink.chapters);
    println!("│ 段数         {:>9}", paras);
    println!("│ 正文字数     {:>9}", chars);
    println!("│ 平均段长     {:>9.1} 字", chars as f64 / paras.max(1) as f64);
    println!("└─ 端到端合计  {:>9.1?}", total);
    println!(
        "   吞吐 {:.1} MB/s",
        bytes as f64 / 1024.0 / 1024.0 / total.as_secs_f64().max(1e-9)
    );
    // 给想测内存峰值的脚本一个明确锚点。
    let _ = Path::new(&path);
}
