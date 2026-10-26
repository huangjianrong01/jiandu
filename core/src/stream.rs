//! 流式导入（v1.50.0）：百万字丛书的**不落地内存**通道。
//!
//! 旧法子是把整个 EPUB / TXT 读成一个 `String`、解析成 `Vec<Chapter>` 全部
//! 挂在 `Book` 上——百万字就是几百万个 `char` 常驻，导入卡、翻页更卡。
//! 现在改成一条**流水线**：
//!
//! ```text
//! 字节流 → 逐篇 / 逐行解析 → 清洗 → 【限长切片】→ 落盘 SQLite → 立刻 drop
//!                                              ↑ 内存里只有这一章
//! ```
//!
//! 解析完一章就交给 [`ChapterSink`]，它负责切片、写库、然后把那份
//! `Chapter` 扔掉；主流程手里只剩「章题 + 字数 + 一小段样本」，那是几 KB
//! 的东西。读完时内存里从来没同时存在过两章正文。

use crate::store;
use crate::types::{Book, Chapter};
// 只在单测里构造段落（`Paragraph::plain`），正式构建里用不到，故按 cfg 收口。
#[cfg(test)]
use crate::types::Paragraph;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub use crate::store::MAX_CHAPTER_CHARS;

/// 章节出口：解析出**一章**就交一章，交出去的那一刻内存就该被还回来。
///
/// 之所以是 trait 而不是直接写库：单元测试可以塞一个内存 sink 进来，
/// 不必碰磁盘。
pub trait ChapterSink {
    /// 交出一章（内部负责限长切片与落盘）。
    fn push(&mut self, chapter: Chapter);
    /// 已落盘的总章数（切片之后的数量，可能多于交进来的次数）。
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// 各章字数（与落盘章号一一对应）。
    fn sizes(&self) -> &[usize];
    /// 各章题名。
    fn titles(&self) -> Vec<String>;
    /// 第 i 章正文的**前若干字**样本。
    ///
    /// 前言 / 散页那几道判据要读正文（薄章、通篇无句读），但不必把整章
    /// 拉回内存——400 字足够判「这是不是一页目录」。
    fn sample(&self, i: usize) -> &str;
    /// 报告进度（0.0~1.0）。
    fn tick(&self, pct: f32, label: &str);
    /// 推进「已完成工作量」（量纲与导入时给的 `total_hint` 一致）。
    fn advance(&mut self, done: usize, label: &str);
    /// 砍掉卷首 n 章（前言 / 版权页 / 目录），章号随后在 `finish` 里压实。
    fn drop_front(&mut self, n: usize);
    /// 砍掉指定章（夹在正文中间的散页）。
    fn drop_at(&mut self, idxs: &[usize]);
    /// 改写章题（目录润色之后题名才定得下来，那时正文早已落盘）。
    fn set_title(&mut self, idx: usize, title: &str);
}

/// 往 SQLite 里落的 sink（生产用）。
pub struct DbSink {
    conn: Connection,
    id: String,
    sizes: Vec<usize>,
    samples: Vec<String>,
    on_progress: Option<Box<dyn Fn(f32, String) + Send>>,
    /// 总工作量提示（EPUB 是 spine 文件数，TXT 是字节数），只用来算百分比。
    total_hint: usize,
    done_hint: usize,
    chars: usize,
}

impl DbSink {
    pub fn new(
        conn: Connection,
        id: impl Into<String>,
        total_hint: usize,
        on_progress: Option<Box<dyn Fn(f32, String) + Send>>,
    ) -> Self {
        Self {
            conn,
            id: id.into(),
            sizes: Vec::new(),
            samples: Vec::new(),
            on_progress,
            total_hint: total_hint.max(1),
            done_hint: 0,
            chars: 0,
        }
    }

    /// 推进进度条（`done` 是已完成的工作量单位，与 `total_hint` 同量纲）。
    pub fn advance(&mut self, done: usize, label: &str) {
        self.done_hint = self.done_hint.max(done);
        let pct = (self.done_hint as f32 / self.total_hint as f32).clamp(0.0, 1.0);
        self.tick(pct, label);
    }

    /// 全书字数（导入报告用）。
    pub fn chars(&self) -> usize {
        self.chars
    }

    /// 底层连接（单测要直接查库验证「确实写进去了」）。
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// 仓库 id。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 收尾：把「题名 + 字数 + 样本」交出去之前，先把章号压实（前言被砍后
    /// 章号会断）。
    pub fn finish(&self) -> Result<()> {
        store::renumber(&self.conn, &self.id)
    }
}

impl ChapterSink for DbSink {
    fn push(&mut self, mut chapter: Chapter) {
        let chars = chapter.char_count();
        if chars == 0 {
            return;
        }
        // 样本：正文前 400 字（判据用），随章落盘后正文就扔了。
        let sample: String = chapter
            .paragraphs
            .iter()
            .flat_map(|p| p.text.chars())
            .take(400)
            .collect();
        // **限长切片**：超 5000 字就在段落边界切开，切成几章各自落盘。
        // 切片之后 `chapter.paragraphs` 已被 take 走，这一章的正文就此释放。
        let parts = crate::pipeline::split_oversized_chapter(
            &chapter.title,
            std::mem::take(&mut chapter.paragraphs),
            MAX_CHAPTER_CHARS,
        );
        for part in parts {
            let n = part.char_count();
            if n == 0 {
                continue;
            }
            let json = match serde_json::to_string(&part) {
                Ok(j) => j,
                Err(_) => continue,
            };
            let idx = self.sizes.len();
            if store::put_chapter(&self.conn, &self.id, idx, &part.title, n, &json).is_err() {
                continue;
            }
            self.sizes.push(n);
            self.samples.push(sample.clone());
            self.chars += n;
        }
        // `part` 在这里离开作用域 -> Vec<Paragraph> 释放。
        drop(chapter);
    }

    fn len(&self) -> usize {
        self.sizes.len()
    }
    fn sizes(&self) -> &[usize] {
        &self.sizes
    }
    fn titles(&self) -> Vec<String> {
        // 题名以库里为准：直接从 chapters 表按章号读回。
        match self.conn.prepare(
            "SELECT title FROM chapters WHERE store_id=?1 ORDER BY idx ASC",
        ) {
            Ok(mut st) => st
                .query_map(rusqlite::params![&self.id], |r| r.get::<_, String>(0))
                .map(|rows| rows.filter_map(|v| v.ok()).collect())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }
    fn sample(&self, i: usize) -> &str {
        self.samples.get(i).map(|s| s.as_str()).unwrap_or("")
    }
    fn tick(&self, pct: f32, label: &str) {
        if let Some(f) = &self.on_progress {
            f(pct, label.to_string());
        }
    }
    fn advance(&mut self, done: usize, label: &str) {
        self.done_hint = self.done_hint.max(done);
        let pct = (self.done_hint as f32 / self.total_hint as f32).clamp(0.0, 1.0);
        self.tick(pct, label);
    }
    fn drop_front(&mut self, n: usize) {
        if n == 0 || n >= self.sizes.len() {
            return;
        }
        self.drop_at(&(0..n).collect::<Vec<_>>());
    }
    fn drop_at(&mut self, idxs: &[usize]) {
        if idxs.is_empty() {
            return;
        }
        let sorted = {
            let mut v = idxs.to_vec();
            v.sort_unstable();
            v.dedup();
            v
        };
        let _ = store::drop_chapters(&self.conn, &self.id, &sorted);
        // 章号留着空洞，交给 `finish()` 的 renumber 压实；这里的
        // sizes / samples 必须与库里「剩下的章」重新对齐，故整表重读。
        self.sizes = store::chapter_sizes(&self.conn, &self.id);
        let keep: std::collections::HashSet<usize> =
            (0..self.samples.len()).collect::<std::collections::HashSet<_>>()
                .difference(&sorted.iter().copied().collect())
                .copied()
                .collect();
        let mut kept = std::mem::take(&mut self.samples);
        self.samples = kept
            .drain(..)
            .enumerate()
            .filter(|(i, _)| keep.contains(i))
            .map(|(_, s)| s)
            .collect();
        // 空章会留下空样本，补齐长度以免 `sample(i)` 取错。
        while self.samples.len() < self.sizes.len() {
            self.samples.push(String::new());
        }
    }
    fn set_title(&mut self, idx: usize, title: &str) {
        let _ = store::set_title(&self.conn, &self.id, idx, title);
    }
}

/// 导入一整本书（流式）：解析、清洗、切片、落盘一气呵成。
///
/// 返回 `(骨架 Book, store_id)`。骨架 `Book` 的 `chapters` 里只有题名，
/// 正文都在库里——`Book::store_id` 记着仓库 id，读的时候按章取回。
///
/// `on_progress` 在后台线程里被调用（0.0~1.0 + 一句人话），UI 拿它画
/// 「雕版刻印中… 34%」。
pub fn import_streaming(
    path: &Path,
    on_progress: Option<Box<dyn Fn(f32, String) + Send>>,
) -> Result<(Book, String)> {
    let id = store_id_for(path);
    let conn = store::connect()?;
    let file_len = std::fs::metadata(path)
        .map(|m| m.len() as usize)
        .unwrap_or(1_000_000);
    let mut sink = DbSink::new(conn, id.clone(), file_len.max(1), on_progress);

    let mut book = crate::importer::import_into(path, &mut sink)
        .with_context(|| format!("导入失败：{}", path.display()))?;

    if sink.is_empty() {
        anyhow::bail!("这本书没解析出正文");
    }
    sink.finish()?;

    // 骨架化：题名留着，正文一律不留（导入这一刻它们刚被释放过一次，
    // 这里再兜一次底，保证 Book 里绝不残留正文）。
    let sizes = sink.sizes().to_vec();
    let titles = sink.titles();
    book.chapters = titles
        .iter()
        .map(|t| Chapter {
            title: t.clone(),
            paragraphs: Vec::new(),
        })
        .collect();
    book.chapter_chars = sizes;
    book.store_id = Some(id.clone());
    book.ensure_toc();

    let meta = serde_json::to_string(&book).context("书稿元数据序列化失败")?;
    {
        let c = store::connect()?;
        store::put_book(&c, &id, &book.title, &book.author, &meta)?;
    }
    Ok((book, id))
}

/// 取回一章的正文（虚拟化读路径）。
pub fn hydrate_chapter(store_id: &str, idx: usize) -> Option<Chapter> {
    let c = store::connect().ok()?;
    store::load_chapter(&c, store_id, idx).ok().flatten()
}

/// 把骨架 `Book` 的**所有章**取回内存（开卷时用一次）。
///
/// -virtual 化的终态是「只留当前 1~2 章」，但分页引擎要一次排完整本书
/// 才能算出连续的面号，故开卷这一刻必须先把全书正文调齐——**逐章取、
/// 逐章排**由上层负责，这里只保证「缺哪章补哪章」：已经 hydrate 过的章
/// 不会重复读库。
pub fn hydrate_all(book: &mut Book) {
    let Some(id) = book.store_id.clone() else {
        return;
    };
    let Ok(c) = store::connect() else {
        return;
    };
    for (i, ch) in book.chapters.iter_mut().enumerate() {
        if ch.paragraphs.is_empty() {
            if let Some(full) = store::load_chapter(&c, &id, i).ok().flatten() {
                *ch = full;
            }
        }
    }
}

/// 卸下全书正文（`chapters[i].paragraphs` 清空，内存还回去）。
///
/// 换书 / 退出阅读时调用：章题与目录都还在，再开卷时 `hydrate_all` 会
/// 逐章补回来。
pub fn unload_all(book: &mut Book) {
    for ch in book.chapters.iter_mut() {
        ch.paragraphs.clear();
    }
    book.chapters.shrink_to_fit();
}

/// 仓库 id：文件路径 + 大小 + 修改时间 的稳定哈希，同一文件重复导入
/// 落在同一条记录上（覆盖），不会把库撑爆。
pub fn store_id_for(path: &Path) -> String {
    use std::hash::Hash;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        if let Ok(t) = m.modified() {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .hash(&mut h);
        }
    }
    let v = std::hash::Hasher::finish(&h);
    let stem = crate::importer::file_stem(path);
    format!("{stem}-{v:016x}")
}

/// 单元测试用的内存 sink：不落盘，只记下来交进了什么。
#[cfg(test)]
pub struct MemSink {
    pub chapters: Vec<Chapter>,
    pub sizes: Vec<usize>,
}

#[cfg(test)]
impl MemSink {
    pub fn new() -> Self {
        Self {
            chapters: Vec::new(),
            sizes: Vec::new(),
        }
    }
}

#[cfg(test)]
impl Default for MemSink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl ChapterSink for MemSink {
    fn push(&mut self, chapter: Chapter) {
        let n = chapter.char_count();
        if n == 0 {
            return;
        }
        let parts = crate::pipeline::split_oversized_chapter(
            &chapter.title,
            chapter.paragraphs.clone(),
            MAX_CHAPTER_CHARS,
        );
        for p in parts {
            self.sizes.push(p.char_count());
            self.chapters.push(p);
        }
    }
    fn len(&self) -> usize {
        self.sizes.len()
    }
    fn sizes(&self) -> &[usize] {
        &self.sizes
    }
    fn titles(&self) -> Vec<String> {
        self.chapters.iter().map(|c| c.title.clone()).collect()
    }
    fn sample(&self, i: usize) -> &str {
        // 测试里直接给整章，样本就是它自己（判据要读正文）。
        self.chapters
            .get(i)
            .and_then(|c| c.paragraphs.first())
            .map(|p| p.text.as_str())
            .unwrap_or("")
    }
    fn tick(&self, _pct: f32, _label: &str) {}
    fn advance(&mut self, _done: usize, _label: &str) {}
    fn drop_front(&mut self, n: usize) {
        if n == 0 || n >= self.chapters.len() {
            return;
        }
        self.chapters.drain(..n);
        self.sizes.drain(..n);
    }
    fn drop_at(&mut self, idxs: &[usize]) {
        let mut v = idxs.to_vec();
        v.sort_unstable();
        v.dedup();
        for i in v.iter().rev() {
            if *i < self.chapters.len() {
                self.chapters.remove(*i);
                if *i < self.sizes.len() {
                    self.sizes.remove(*i);
                }
            }
        }
    }
    fn set_title(&mut self, idx: usize, title: &str) {
        if let Some(c) = self.chapters.get_mut(idx) {
            c.title = title.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一章超过 5000 字必须被切成多章，且**切完每章都不超上限**。
    #[test]
    fn oversized_chapter_is_split_at_paragraph_boundaries() {
        let mut sink = MemSink::new();
        let paras: Vec<Paragraph> = (0..40)
            .map(|i| Paragraph::plain("一二三四五六七八九十".repeat(20) + &format!("{i}")))
            .collect();
        sink.push(Chapter {
            title: "卷一".into(),
            paragraphs: paras,
        });
        assert!(sink.len() > 1, "八千多字必须切成两章以上");
        assert!(sink.sizes().iter().all(|&n| n <= MAX_CHAPTER_CHARS + 200));
        // 字数不能丢：切完的总和要等于原章字数。
        let expect: usize = (0..40)
            .map(|i| 200 + format!("{i}").chars().count())
            .sum::<usize>();
        let total: usize = sink.sizes().iter().sum();
        assert_eq!(total, expect);
    }

    /// 段是最小完整单位：绝不把一段劈成两半（夹注的 offset 会错位）。
    #[test]
    fn splitting_never_breaks_a_paragraph() {
        let mut sink = MemSink::new();
        let paras = vec![
            Paragraph::plain("甲".repeat(3000)),
            Paragraph::plain("乙".repeat(3000)),
        ];
        sink.push(Chapter {
            title: "卷".into(),
            paragraphs: paras,
        });
        assert_eq!(sink.len(), 2);
        for c in &sink.chapters {
            assert_eq!(c.paragraphs.len(), 1, "每段必须整段留在一章里");
        }
    }

    /// 小章原样通过，不切片。
    #[test]
    fn small_chapter_passes_through_unsplit() {
        let mut sink = MemSink::new();
        sink.push(Chapter {
            title: "小卷".into(),
            paragraphs: vec![Paragraph::plain("子曰学而时习之")],
        });
        assert_eq!(sink.len(), 1);
        assert_eq!(sink.titles(), vec!["小卷".to_string()]);
    }

    /// 在内存里造一份**一万字的 GBK 文本流**，走真的 `import_txt_into`：
    /// 验证①编码探测（GBK 字节不是合法 UTF-8）②按 5000 字切片
    /// ③确实写进了 SQLite「章节虚拟表」④取回来是可读的汉字、不是乱码。
    #[test]
    fn ten_thousand_char_gbk_stream_is_decoded_split_and_stored() {
        use std::io::Write;

        // ── 造料：一条章题 + 20 段 × 512 字 ≈ 一万字（章内必被硬切）──
        let mut text = String::new();
        text.push_str("第一回 灵根育孕源流出\n");
        for _ in 0..20 {
            text.push_str(&"天地玄黄宇宙洪荒".repeat(64)); // 8 × 64 = 512 字
            text.push('\n');
        }
        let src_chars = text.chars().filter(|c| !c.is_whitespace()).count();
        assert!(src_chars > 10_000, "造出来的料得够大：{src_chars} 字");

        let dir = std::env::temp_dir().join(format!("jiandu-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("大书.txt");

        // 落成 **GBK** 字节——老书 txt 的常态，且它绝不是合法 UTF-8，
        // 正好用来验证 `detect_utf8` 会走 GBK 那条分支。
        let (bytes, _, _) = encoding_rs::GBK.encode(&text);
        assert!(
            std::str::from_utf8(&bytes).is_err(),
            "这份字节必须是非法 UTF-8，否则测不到 GBK 分支"
        );
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&bytes).unwrap();
        drop(f);

        // ── 流式导入：内存库，不污染 ~/.jiandu ──
        let conn = store::connect_in_memory().expect("内存库");
        let id = "test-gbk-stream";
        let mut sink = DbSink::new(conn, id, bytes.len(), None);
        let book = crate::importer::import_txt_into(&path, &mut sink).unwrap();
        assert!(!book.title.is_empty());

        // ①切片：一万字必须切成两章以上，且每章不超上限（含单段容差）。
        let sizes = sink.sizes().to_vec();
        assert!(
            sizes.len() >= 2,
            "一万字应切成 2 章以上，实际 {} 章：{sizes:?}",
            sizes.len()
        );
        let biggest = *sizes.iter().max().unwrap();
        assert!(
            biggest <= MAX_CHAPTER_CHARS + 600,
            "单章 {biggest} 字，超过 5000 字上限太多"
        );

        // ②字数不丢：落盘总字数 = 原文 − 章题那一行（章题进 `title`，
        // 不占正文字格——古籍版心本就不把卷题排进正文）。
        let stored: usize = sizes.iter().sum();
        assert!(
            stored >= src_chars - 12,
            "落盘 {stored} 字，比原文 {src_chars} 字少了太多，解析丢了内容"
        );

        // ③确实写进了 SQLite 表里（不是只在 sink 的 Vec 里）。
        let n = store::chapter_count(sink.conn(), id);
        assert_eq!(n, sizes.len(), "库里的章数应与 sink 记的一致");
        assert_eq!(store::chapter_sizes(sink.conn(), id), sizes);

        // ④取回来是**可读的汉字**：GBK 解码正确，不是一坨乱码。
        let ch0 = store::load_chapter(sink.conn(), id, 0)
            .unwrap()
            .expect("第 0 章应在库里");
        let body: String = ch0.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert!(
            body.contains("天地玄黄"),
            "GBK 解码失败，取回的是乱码：{}",
            body.chars().take(20).collect::<String>()
        );
        // 章题也被认出来了（不是「正文」这种兜底名）。
        let titles = sink.titles();
        assert!(
            titles.iter().any(|t| t.contains("第一回")),
            "章题没被认出来：{titles:?}"
        );

        // ⑤虚拟化卸货：`unload_chapter` 之后正文归零、题名还在。
        let mut skeleton = Book::new(&book.title, &book.author);
        skeleton.store_id = Some(id.to_string());
        skeleton.chapter_chars = sizes.clone();
        skeleton.chapters = titles
            .iter()
            .map(|t| Chapter {
                title: t.clone(),
                paragraphs: Vec::new(),
            })
            .collect();
        assert!(!skeleton.chapter_loaded(0), "骨架模式下第 0 章是空的");
        assert!(skeleton.is_virtual());

        std::fs::remove_file(&path).ok();
    }

    /// 同一条流水线走 **UTF-8** 分支：探测不能只认 GBK。
    #[test]
    fn utf8_stream_is_detected_and_stored() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("jiandu-u8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("小书.txt");
        let text = "卷一\n道可道，非常道。名可名，非常名。\n";
        std::fs::File::create(&path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();

        let conn = store::connect_in_memory().unwrap();
        let mut sink = DbSink::new(conn, "test-utf8", text.len(), None);
        let _ = crate::importer::import_txt_into(&path, &mut sink).unwrap();
        assert_eq!(sink.len(), 1, "一小章不该被切开");
        let ch = store::load_chapter(sink.conn(), "test-utf8", 0)
            .unwrap()
            .unwrap();
        let body: String = ch.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert!(body.contains("道可道"), "UTF-8 解码失败：{body}");
        std::fs::remove_file(&path).ok();
    }

    /// 进度回调要被真的调用到，且单调不回头（UI 那条「雕版刻印中…」靠它）。
    #[test]
    fn progress_reports_monotonically() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("jiandu-prog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("进度.txt");
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!("第{i}段：天地玄黄宇宙洪荒日月盈昃辰宿列张。\n"));
        }
        std::fs::File::create(&path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let seen2 = seen.clone();
        let conn = store::connect_in_memory().unwrap();
        let mut sink = DbSink::new(
            conn,
            "test-progress",
            text.len(),
            Some(Box::new(move |pct: f32, _label: String| {
                seen2.lock().unwrap().push(pct);
            })),
        );
        let _ = crate::importer::import_txt_into(&path, &mut sink).unwrap();
        let v = seen.lock().unwrap().clone();
        assert!(!v.is_empty(), "进度回调一次都没被调用");
        assert!(v.windows(2).all(|w| w[1] >= w[0]), "进度不能往回走：{v:?}");
        assert!(*v.last().unwrap() <= 1.0);
        std::fs::remove_file(&path).ok();
    }
}
