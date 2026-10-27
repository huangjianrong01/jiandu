//! **惰性分页器**（v1.60.0）——百万字巨著的开卷与翻页引擎。
//!
//! ## 它解决的问题
//!
//! 旧架构里 `Root::layout_pages()` 每次调用都做「全书 `to_flow` + 全书
//! `layout_book_from_opts`」，而渲染循环**每帧都调它一次**。实测《资治通鉴》
//! （308 万字 / 799 章 / 12002 叶）：
//!
//! ```text
//! to_flow 13ms + 排版 94ms ≈ 107ms / 帧，且每帧分配 308 万个 Glyph
//! ```
//!
//! 不到 10 FPS，内存分配器还被按在地上摩擦——这就是「导入完卡死 / 进度条
//! 不动」的真凶（排版本身一点也不慢，慢在**重复**）。
//!
//! ## 三条支柱
//!
//! 1. **轻量索引**：`scan_to(ci)` 逐章排版只为取**面数**，只留 `lens` /
//!    `starts` 两张 `usize` 表（799 章 ≈ 6KB），排完的 `Page` 当场丢弃。
//!    有了前缀和，「第 N 叶在第几章」是 O(log n)，不必把排版矩阵留在内存。
//! 2. **分块 + LRU**：真正要显示时才排那一章（`pages_cached`），排出的面进
//!    容量 24 章的 LRU 窗口。Viewport 之外的一概不驻留。
//! 3. **按需取正文**：章的正文平时在 SQLite 里（`stream::hydrate_chapter`，
//!    单章 2ms）。`ensure_loaded` 缺哪章读哪章——开卷不再 `hydrate_all`
//!    （全书 261ms 一次买断），而是「读当前章 → 秒开 → 后台补全索引」。
//!
//! ## 数学保证
//!
//! 分页引擎里**章边界必然收面**（`layout_book_from_opts` 的 `chapter_start`
//! 分支无条件 `seal!()`），故
//!
//! ```text
//! 逐章排完串起来  ≡  全书一次排完
//! ```
//!
//! 面数相同、每一面的字序逐字相同（`bench_layout_and_chapterwise_equivalence`
//! 用 396 万字真书钉死）。于是「分块」不是近似，是**恒等变形**。
//!
//! ## 不变量
//!
//! * `lens[k]` / `starts[k]` 一旦算出**永不改变**（章的面数只取决于该章正文
//!   与版式参数），故前缀和无漂移、书签叶号不会跑；
//! * `scanned` 是「已探明的连续前缀 `[0, scanned)`」，单调推进，从不回退；
//! * 缓存 miss 只会**变慢**，不会**排错**——正文没 hydrate 时排出的面数为 0，
//!   所以 `ensure_loaded` 是正确性的前提，不是优化。

use crate::gloss::{layout_book_from_opts, GlossPayload, Page};
use crate::types::{Book, Glyph, GlyphKind, NoteKind};
use std::collections::{HashMap, VecDeque};

/// 版式参数指纹：任一项变了，整份缓存作废。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LayoutKey {
    /// 一面几列。
    pub cols: usize,
    /// 一列几字。
    pub rows: usize,
    /// 卷首题署预留行数。
    pub reserve: usize,
    /// 标点显隐（影响 `to_flow` 的字数）。
    pub show_punct: bool,
    /// 双行夹注是否进正文流（影响占格与面数）。
    ///
    /// 夹注占格，故它与列数一样属于**版式指纹**：变了面数就变，缓存必须作废。
    pub inline_notes: bool,
    /// 西文成组压缩（影响占格）。
    pub west_mode: bool,
}

/// 一章的排版结果在 LRU 里最多留多少章。24 章 ≈ 300 面，足够视口 +
/// 前后翻页缓冲，又不至于把整本书摁在内存里。
const LRU_CAP: usize = 24;

pub struct Pager {
    /// 当前版式指纹。
    key: LayoutKey,
    /// 章 → 该章的面数（探明后才有值）。
    lens: Vec<Option<usize>>,
    /// 章 → 该章首面的全局叶号（前缀和）。
    starts: Vec<Option<usize>>,
    /// 已探明的连续前缀 `[0, scanned)`。
    scanned: usize,
    /// 章 → 该章的排版结果（LRU 窗口）。
    cache: HashMap<usize, Vec<Page>>,
    /// LRU 淘汰顺序（队首最旧）。
    order: VecDeque<usize>,
    /// 虚拟化书稿的仓库 id（为 `None` 表示正文全在内存里，无需 hydrate）。
    store_id: Option<String>,
}

impl Pager {
    /// 为一本 `n` 章的书立一台分页器（此刻一章都还没排）。
    pub fn new(n: usize, key: LayoutKey, store_id: Option<String>) -> Self {
        Self {
            key,
            lens: vec![None; n],
            starts: vec![None; n],
            scanned: 0,
            cache: HashMap::new(),
            order: VecDeque::new(),
            store_id,
        }
    }

    /// 章数（书换了要重建分页器）。
    pub fn chapters(&self) -> usize {
        self.lens.len()
    }

    /// 版式参数变了 → 整份缓存作废（`lens` 也要重算，因为面数依赖版式）。
    pub fn rekey(&mut self, key: LayoutKey) {
        if key == self.key {
            return;
        }
        self.key = key;
        self.lens.iter_mut().for_each(|v| *v = None);
        self.starts.iter_mut().for_each(|v| *v = None);
        self.scanned = 0;
        self.cache.clear();
        self.order.clear();
    }

    /// 换书（或书被换掉）后重挂仓库 id。
    pub fn set_store(&mut self, store_id: Option<String>) {
        self.store_id = store_id;
    }

    /// **按需取正文**：这一章的正文若不在内存（虚拟化书稿被骨架化了），
    /// 就从 SQLite 读回来。单章约 2ms——「开卷只取当前章」全靠它。
    fn ensure_loaded(&self, book: &mut Book, ci: usize) {
        let already = book
            .chapters
            .get(ci)
            .map(|c| !c.paragraphs.is_empty())
            .unwrap_or(false);
        if already {
            return;
        }
        let Some(id) = &self.store_id else {
            return; // 老条目：正文本就全在内存里（空就是真空章）。
        };
        if let Some(ch) = crate::stream::hydrate_chapter(id, ci) {
            if let Some(slot) = book.chapters.get_mut(ci) {
                *slot = ch;
            }
        }
    }

    /// 排**一章**，返回它的面。**不进缓存**（调用方决定去留）。
    fn layout_chapter(
        &self,
        book: &Book,
        ci: usize,
    ) -> Vec<Page> {
        let flow = book.flow_of_chapter(ci, self.key.show_punct);
        if flow.is_empty() {
            return Vec::new();
        }
        // 每章都是「章首面」起排（`start_as_chapter = true`）：章边界在全书
        // 流里同样触发收面，故恒等于「接着上一章排」。
        //
        // 夹注闭包在此**就地**构造（只借 `&Book`）：`ensure_loaded` 那边的
        // `&mut Book` 借用此刻早已结束，两边互不打架——这也是为什么 `gloss`
        // 不由调用方传入。
        let inline = self.key.inline_notes;
        let gloss = move |g: &Glyph| -> Option<GlossPayload> {
            if !inline || g.kind != GlyphKind::Char {
                return None;
            }
            let p = book.chapters.get(g.chapter)?.paragraphs.get(g.para)?;
            p.notes
                .iter()
                .find(|n| n.at == g.offset && n.kind == NoteKind::Note)
                .map(|n| (n.text.clone(), n.source, *g))
        };
        layout_book_from_opts(
            &flow,
            gloss,
            self.key.cols,
            self.key.rows,
            self.key.reserve,
            true,
            self.key.west_mode,
        )
    }

    /// 探明到章 `ci`（含）：把 `[0, ci]` 各章的面数与起始叶号算出来。
    ///
    /// 只留数字、不留排版——`Page` 当场丢弃，所以「全书索引」常驻的代价
    /// 只是两张 `usize` 表。
    pub fn scan_to(
        &mut self,
        book: &mut Book,
        ci: usize,
    ) {
        let n = self.lens.len();
        if n == 0 {
            return;
        }
        let end = (ci + 1).min(n);
        if end <= self.scanned {
            return;
        }
        for k in self.scanned..end {
            self.ensure_loaded(book, k);
            let len = self.layout_chapter(book, k).len();
            self.lens[k] = Some(len);
        }
        // 前缀和：从头累加（799 章 × 每帧几次，比二分还便宜）。
        let mut acc = 0usize;
        for k in 0..end {
            let len = self.lens.get(k).copied().flatten().unwrap_or(0);
            if let Some(slot) = self.starts.get_mut(k) {
                *slot = Some(acc);
            }
            acc += len;
        }
        self.scanned = end;
    }

    /// 探明全书（后台线程干这个）。
    pub fn scan_all(
        &mut self,
        book: &mut Book,
    ) {
        let n = self.lens.len();
        if n == 0 {
            return;
        }
        self.scan_to(book, n - 1);
    }

    /// 已探明的叶数（`scanned` 之前那些章的面数之和）。
    ///
    /// 探明全书之前它是**部分值**——调用方拿它给页码做上限是安全的
    /// （保守，不会指向不存在的面）。
    pub fn known_total(&self) -> usize {
        self.lens
            .iter()
            .take(self.scanned)
            .filter_map(|v| *v)
            .sum()
    }

    /// 全书是否已探明。
    pub fn is_complete(&self) -> bool {
        self.scanned >= self.lens.len()
    }

    /// 按字数粗估总叶数（探明过程中给 UI 报「约 N 叶」用）。
    pub fn estimate_total(&self, book: &Book) -> usize {
        let per_leaf = (self.key.cols.saturating_sub(1).max(1)) * self.key.rows.max(1);
        if per_leaf == 0 {
            return self.known_total();
        }
        let chars: usize = book
            .chapter_chars
            .iter()
            .copied()
            .sum::<usize>()
            .max(
                book.chapters
                    .iter()
                    .flat_map(|c| c.paragraphs.iter())
                    .map(|p| p.text.chars().count())
                    .sum(),
            );
        (chars / per_leaf).max(self.known_total())
    }

    /// 第 `i` 叶落在第几章（二分；只在前缀和已探明的区间里查）。
    pub fn chapter_of(&self, i: usize) -> Option<usize> {
        if self.scanned == 0 {
            return None;
        }
        // 找最后一个 start <= i。
        let (mut lo, mut hi) = (0usize, self.scanned - 1);
        if i < self.starts.get(0).copied().flatten().unwrap_or(0) {
            return None;
        }
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let s = self.starts.get(mid).copied().flatten().unwrap_or(0);
            if s <= i {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Some(lo)
    }

    /// LRU 窗口里当前驻留的章数（bench / 诊断用）。
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// 某章首面的全局叶号。
    pub fn start_of(&self, ci: usize) -> Option<usize> {
        self.starts.get(ci).copied().flatten()
    }

    /// 取一章的排版结果（进 LRU；命中即零成本）。
    fn pages_cached(
        &mut self,
        book: &mut Book,
        ci: usize,
    ) -> Option<&Vec<Page>> {
        if !self.cache.contains_key(&ci) {
            self.ensure_loaded(book, ci);
            let ps = self.layout_chapter(book, ci);
            self.cache.insert(ci, ps);
            self.order.push_back(ci);
            self.evict();
        }
        self.cache.get(&ci)
    }

    fn evict(&mut self) {
        while self.cache.len() > LRU_CAP {
            match self.order.pop_front() {
                Some(old) => {
                    self.cache.remove(&old);
                }
                None => break,
            }
        }
    }

    /// **取第 `i` 叶**（渲染 / 命中用）。必要时先把索引探到它所在章。
    ///
    /// 返回的是 clone——单叶约 250 字，比借出 `&Page` 把 `&mut self` 锁死
    /// 要划算得多（渲染要连取相邻两叶）。
    pub fn page(
        &mut self,
        book: &mut Book,
        i: usize,
    ) -> Option<Page> {
        // 索引没探到这里就**逐章**推进（翻到哪算到哪，绝不一口气 hydrate
        // 全书）。`scanned` 单调增，故循环必然终止；探过之后就是 O(log n)。
        let n = self.lens.len();
        while i >= self.known_total() && self.scanned < n {
            self.scan_to(book, self.scanned);
        }
        let ci = self.chapter_of(i)?;
        let start = self.start_of(ci)?;
        let idx = i - start;
        let page = self.pages_cached(book, ci)?.get(idx).cloned();
        page
    }

    /// 某字所在叶（跳章 / 搜索 / 恢复进度用）。
    ///
    /// 逐章推进、命中即返回——比「排全书再线性找」省掉后面所有章。
    pub fn page_of_glyph(
        &mut self,
        book: &mut Book,
        key: (usize, usize, usize),
    ) -> Option<usize> {
        let n = self.lens.len();
        for ci in 0..n {
            self.scan_to(book, ci);
            let start = match self.start_of(ci) {
                Some(s) => s,
                None => continue,
            };
            let hit = match self.pages_cached(book, ci) {
                Some(ps) => ps.iter().position(|p| {
                    p.glyphs
                        .iter()
                        .any(|g| (g.chapter, g.para, g.offset) == key)
                }),
                None => None,
            };
            if let Some(k) = hit {
                return Some(start + k);
            }
        }
        None
    }

    /// 排全书并串成一份完整的 `Vec<Page>`（**导出用**：Anki / Markdown 等
    /// 确实需要逐叶遍历的场合）。渲染路径**不许**调它——那正是旧架构的
    /// 病灶。
    pub fn all_pages(
        &mut self,
        book: &mut Book,
    ) -> Vec<Page> {
        let n = self.lens.len();
        if n == 0 {
            return Vec::new();
        }
        self.scan_all(book);
        let mut out: Vec<Page> = Vec::new();
        for ci in 0..n {
            let ps = self.pages_cached(book, ci).cloned();
            if let Some(ps) = ps {
                out.extend(ps);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Book, Chapter, Paragraph};

    fn book_of(chapters: &[&str]) -> Book {
        let mut b = Book::new("测", "");
        b.chapters = chapters
            .iter()
            .enumerate()
            .map(|(i, t)| Chapter {
                title: format!("第{i}章"),
                paragraphs: vec![Paragraph::plain((*t).to_string())],
            })
            .collect();
        b.chapter_chars = b
            .chapters
            .iter()
            .map(|c| c.paragraphs.iter().map(|p| p.text.chars().count()).sum())
            .collect();
        b
    }

    fn key() -> LayoutKey {
        LayoutKey {
            cols: 8,
            rows: 4,
            reserve: 0,
            show_punct: true,
            inline_notes: false,
            west_mode: false,
        }
    }

    fn no_gloss(_: &Glyph) -> Option<GlossPayload> {
        None
    }

    /// **分块 ≡ 全书**：逐章排完串起来，与全书一次排**逐字同面**。
    /// 这是整套惰性架构的地基（真书验证见 `bench_layout_and_chapterwise_equivalence`）。
    #[test]
    fn chapterwise_pages_equal_whole_book_pages() {
        let b = book_of(&[
            "一二三四五六七八九十一二三四五六七八九十一二三四",
            "甲乙丙丁戊己庚辛",
            "子丑寅卯辰巳午未申酉戌亥子丑寅卯辰巳午未申酉戌亥",
        ]);
        let k = key();
        let flow = b.to_flow(true);
        let whole =
            layout_book_from_opts(&flow, &no_gloss, k.cols, k.rows, k.reserve, true, k.west_mode);

        let mut pg = Pager::new(b.chapters.len(), k, None);
        let all = pg.all_pages(&mut b.clone());

        assert_eq!(whole.len(), all.len(), "面数不一致");
        let a: Vec<_> = whole
            .iter()
            .flat_map(|p| p.glyphs.iter().map(|g| (g.chapter, g.para, g.offset)))
            .collect();
        let c: Vec<_> = all
            .iter()
            .flat_map(|p| p.glyphs.iter().map(|g| (g.chapter, g.para, g.offset)))
            .collect();
        assert_eq!(a, c, "字序不一致");
    }

    /// 前缀和不漂移：`starts` 单调，且 `chapter_of` 与 `start_of` 自洽。
    #[test]
    fn prefix_sums_are_monotonic_and_consistent() {
        let b = book_of(&["一二三四五六七八九十", "甲乙丙丁", "子丑寅卯辰巳"]);
        let mut pg = Pager::new(b.chapters.len(), key(), None);
        let mut bb = b.clone();
        pg.scan_all(&mut bb);
        assert!(pg.is_complete());
        let total = pg.known_total();
        let mut seen = 0usize;
        for ci in 0..b.chapters.len() {
            let s = pg.start_of(ci).expect("章应有起始叶号");
            assert!(s >= seen);
            seen = s;
            // 该章首面 → 反查章号应回到自己。
            assert_eq!(pg.chapter_of(s), Some(ci));
        }
        assert!(total > 0);
        // 末叶也落在最后一章。
        let last = total.saturating_sub(1);
        assert_eq!(pg.chapter_of(last), Some(b.chapters.len() - 1));
    }

    /// 惰性：`page(i)` 只探到 i 所在章，不把全书探完。
    #[test]
    fn page_is_lazy_not_eager() {
        let b = book_of(&["一二三四五六七八九十", "甲乙丙丁戊己庚辛", "子丑寅卯辰巳午未"]);
        let mut pg = Pager::new(b.chapters.len(), key(), None);
        let mut bb = b.clone();
        assert_eq!(pg.scanned, 0, "立起来时不该排任何一章");
        let p0 = pg.page(&mut bb, 0);
        assert!(p0.is_some(), "第一叶应该取得到");
        assert!(
            pg.scanned < b.chapters.len(),
            "只取第一叶不该把全书探完（scanned={}）",
            pg.scanned
        );
    }

    /// LRU：窗口容量有上限，翻远了旧章会被淘汰（内存不随全书线性膨胀）。
    #[test]
    fn lru_keeps_the_window_small() {
        let mut titles = Vec::new();
        for i in 0..80 {
            titles.push("一二三四五六七八九十甲乙丙丁".to_string());
            let _ = i;
        }
        let b = book_of(&titles.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let mut pg = Pager::new(b.chapters.len(), key(), None);
        let mut bb = b.clone();
        pg.scan_all(&mut bb);
        for ci in 0..b.chapters.len() {
            let _ = pg.page(&mut bb, pg.start_of(ci).unwrap_or(0));
        }
        assert!(
            pg.cache.len() <= LRU_CAP,
            "缓存应被 LRU 压在 {} 章以内，实为 {}",
            LRU_CAP,
            pg.cache.len()
        );
    }

    /// 换版式 → 缓存作废重排（面数变了，绝不能拿旧索引）。
    #[test]
    fn rekey_invalidates_everything() {
        let b = book_of(&["一二三四五六七八九十一二三四"]);
        let mut pg = Pager::new(b.chapters.len(), key(), None);
        let mut bb = b.clone();
        pg.scan_all(&mut bb);
        let before = pg.known_total();
        let k2 = LayoutKey {
            cols: 4,
            ..key()
        };
        pg.rekey(k2);
        assert_eq!(pg.scanned, 0, "换版式后索引应作废");
        pg.scan_all(&mut bb);
        assert_ne!(pg.known_total(), before, "列数变了，面数理应不同");
    }

    /// 取叶：越界的叶返回 `None`，绝不 panic、绝不给空面。
    #[test]
    fn out_of_range_page_is_none() {
        let b = book_of(&["一二三四"]);
        let mut pg = Pager::new(b.chapters.len(), key(), None);
        let mut bb = b.clone();
        pg.scan_all(&mut bb);
        let total = pg.known_total();
        assert!(pg.page(&mut bb, total).is_none());
        assert!(pg.page(&mut bb, total + 999).is_none());
    }
}
