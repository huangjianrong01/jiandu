//! **批量识别队列**（v1.9.25）：把一次选中的多个 PDF / 图片排成一队，等识别。
//!
//! # 为什么单独一层
//!
//! `batch` 处理的是「电子书进书架」，认的是 `.epub/.txt/.md`；而校对的**原图侧**
//! 认的是 `.pdf` 与图片，两者的归宿完全不同（前者刻成书，后者要的是**文本层坐标**）。
//! 混进 `batch` 会让那张扩展名白名单开始自相矛盾，所以这里独立成模块。
//!
//! # 现在做到哪一步
//!
//! 产品目前**不内置 OCR**：带文本层的 PDF 由 `pdf_text` 直接抽坐标，扫描件只能
//! 提示「需 OCR」。所以这一层只做**队列本身**——收文件、判型、排队、改状态、
//! 给 UI 读数。接 API 时只需写一个消费循环：`take_next()` 取一件 → 外呼 →
//! `mark_done/mark_failed`，UI 一行都不用改。
//!
//! # 三条纪律
//!
//! 1. **不认的扩展名直接拒**（返回 `None`），绝不入队——队列里不该有「等一下
//!    才发现根本处理不了」的东西。
//! 2. **同一路径只排一次**：用户重复点两次「批量导入」不该变成两份活。
//! 3. **状态只许前进**：`Pending → Running → Done|Failed`，`Running` 不许被
//!    第二次 `take_next` 取走（否则同一件活会被两个 worker 同时干）。

use std::path::{Path, PathBuf};

/// 队列认得的原图类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrKind {
    Pdf,
    Image,
}

impl OcrKind {
    /// 给 UI 的两字标签（不引入任何图标资源）。
    pub fn label(self) -> &'static str {
        match self {
            OcrKind::Pdf => "PDF",
            OcrKind::Image => "图",
        }
    }
}

/// 一件活的四种状态。**没有 `Cancelled`**：取消是整队的事（`clear`），
/// 单件取消在 UI 上等于「移除」，直接从表里删掉更干净。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrState {
    Pending,
    Running,
    Done,
    Failed,
}

impl OcrState {
    pub fn label(self) -> &'static str {
        match self {
            OcrState::Pending => "待识别",
            OcrState::Running => "识别中",
            OcrState::Done => "已完成",
            OcrState::Failed => "失败",
        }
    }
}

/// 队列里的一件活。
#[derive(Debug, Clone)]
pub struct OcrJob {
    pub id: u64,
    /// 用户选中的绝对路径（消费方拿它去读盘）。
    pub path: PathBuf,
    /// 展示用文件名（取不到就退回整条路径）。
    pub name: String,
    pub kind: OcrKind,
    pub state: OcrState,
    /// 失败原因 / 完成说明；空串表示没话说。
    pub note: String,
}

/// 先进先出的识别队列。
///
/// 用 `Vec` 而不是 `VecDeque`：长度上限就是「用户一次手选的文件数」（几十），
/// 顺序消费、需要按下标给 UI 渲染，`Vec` 的 `iter()` 更直接。
#[derive(Debug, Default, Clone)]
pub struct OcrQueue {
    jobs: Vec<OcrJob>,
    next_id: u64,
}

impl OcrQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 收一件。**不认的扩展名**（或重复路径）返回 `None`，队列不变。
    pub fn push(&mut self, path: PathBuf) -> Option<u64> {
        let kind = kind_of(&path)?;
        if self.jobs.iter().any(|j| j.path == path) {
            return None;
        }
        self.next_id += 1;
        let id = self.next_id;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        self.jobs.push(OcrJob {
            id,
            path,
            name,
            kind,
            state: OcrState::Pending,
            note: String::new(),
        });
        Some(id)
    }

    /// 批量收。返回 `(入队件数, 被拒件数)`——被拒的是扩展名不认或重复。
    pub fn push_many<I: IntoIterator<Item = PathBuf>>(&mut self, paths: I) -> (usize, usize) {
        let mut ok = 0usize;
        let mut bad = 0usize;
        for p in paths {
            match self.push(p) {
                Some(_) => ok += 1,
                None => bad += 1,
            }
        }
        (ok, bad)
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn jobs(&self) -> &[OcrJob] {
        &self.jobs
    }

    /// 各状态计数：`(待识别, 识别中, 已完成, 失败)`。
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0);
        for j in &self.jobs {
            match j.state {
                OcrState::Pending => c.0 += 1,
                OcrState::Running => c.1 += 1,
                OcrState::Done => c.2 += 1,
                OcrState::Failed => c.3 += 1,
            }
        }
        c
    }

    /// 还有没有活要干（待识别或识别中）。
    pub fn has_work(&self) -> bool {
        self.jobs
            .iter()
            .any(|j| matches!(j.state, OcrState::Pending | OcrState::Running))
    }

    /// **取下一件**：第一个 `Pending` 置为 `Running` 并返回它的下标。
    ///
    /// 返回下标而不是 `&mut OcrJob`，是为了让调用方能顺手 `cx.notify()` 而
    /// 不与借用检查器打架（消费循环拿到下标后可再 `job_mut(i)`）。
    pub fn take_next(&mut self) -> Option<usize> {
        let i = self
            .jobs
            .iter()
            .position(|j| j.state == OcrState::Pending)?;
        self.jobs[i].state = OcrState::Running;
        Some(i)
    }

    pub fn job(&self, id: u64) -> Option<&OcrJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn job_mut(&mut self, id: u64) -> Option<&mut OcrJob> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// 标完成（幂等：已是 Done 不报错，只覆盖说明）。
    pub fn mark_done(&mut self, id: u64, note: &str) {
        if let Some(j) = self.job_mut(id) {
            j.state = OcrState::Done;
            j.note = note.to_string();
        }
    }

    /// 标失败。失败件**不会**被 `take_next` 再取走。
    pub fn mark_failed(&mut self, id: u64, note: &str) {
        if let Some(j) = self.job_mut(id) {
            j.state = OcrState::Failed;
            j.note = note.to_string();
        }
    }

    /// 从队列里移除一件（UI 上「移除」= 不做这件活，不留 Failed 记录）。
    pub fn remove(&mut self, id: u64) {
        self.jobs.retain(|j| j.id != id);
    }

    pub fn clear(&mut self) {
        self.jobs.clear();
    }

    /// 一行摘要，直接贴到状态条上。
    pub fn summary(&self) -> String {
        if self.jobs.is_empty() {
            return "识别队列为空".to_string();
        }
        let (p, r, d, f) = self.counts();
        let mut s = format!("识别队列 {} 件", self.jobs.len());
        if p > 0 {
            s.push_str(&format!(" · 待识别 {p}"));
        }
        if r > 0 {
            s.push_str(&format!(" · 识别中 {r}"));
        }
        if d > 0 {
            s.push_str(&format!(" · 完成 {d}"));
        }
        if f > 0 {
            s.push_str(&format!(" · 失败 {f}"));
        }
        s
    }
}

/// 按扩展名判型。**大小写不敏感**（`.PDF` / `.Jpg` 都认）。
pub fn kind_of(path: &Path) -> Option<OcrKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some(OcrKind::Pdf),
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp" | "webp" | "heic" => {
            Some(OcrKind::Image)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn kind_by_extension_is_case_insensitive() {
        assert_eq!(kind_of(&p("/a/b.PDF")), Some(OcrKind::Pdf));
        assert_eq!(kind_of(&p("/a/b.pdf")), Some(OcrKind::Pdf));
        assert_eq!(kind_of(&p("/a/b.JpG")), Some(OcrKind::Image));
        assert_eq!(kind_of(&p("/a/b.tiff")), Some(OcrKind::Image));
        assert_eq!(kind_of(&p("/a/b.epub")), None);
        assert_eq!(kind_of(&p("/a/b")), None);
    }

    #[test]
    fn unsupported_extension_never_enters_queue() {
        let mut q = OcrQueue::new();
        assert_eq!(q.push(p("/x/book.epub")), None);
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn same_path_is_queued_only_once() {
        let mut q = OcrQueue::new();
        assert!(q.push(p("/x/a.pdf")).is_some());
        assert!(q.push(p("/x/a.pdf")).is_none());
        assert_eq!(q.len(), 1);
        // 不同路径照收。
        assert!(q.push(p("/x/b.png")).is_some());
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn push_many_reports_accepted_and_rejected() {
        let mut q = OcrQueue::new();
        let (ok, bad) = q.push_many(vec![
            p("/x/a.pdf"),
            p("/x/b.png"),
            p("/x/c.epub"), // 不认
            p("/x/a.pdf"),  // 重复
        ]);
        assert_eq!((ok, bad), (2, 2));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn running_job_is_not_handed_out_twice() {
        let mut q = OcrQueue::new();
        let a = q.push(p("/x/a.pdf")).unwrap();
        let b = q.push(p("/x/b.pdf")).unwrap();
        let first = q.take_next().unwrap();
        let second = q.take_next().unwrap();
        assert_ne!(first, second);
        assert!(q.take_next().is_none(), "两件都已 Running，不该再派活");
        assert_eq!(q.job(a).unwrap().state, OcrState::Running);
        assert_eq!(q.job(b).unwrap().state, OcrState::Running);
    }

    #[test]
    fn failed_job_is_skipped_by_take_next() {
        let mut q = OcrQueue::new();
        let a = q.push(p("/x/a.pdf")).unwrap();
        let _b = q.push(p("/x/b.pdf")).unwrap();
        q.mark_failed(a, "扫描件分辨率过低");
        let idx = q.take_next().unwrap();
        assert_eq!(q.jobs()[idx].id, _b, "失败件不该再被取走");
        assert!(q.take_next().is_none());
        assert_eq!(q.jobs()[idx].note, String::new());
    }

    #[test]
    fn counts_and_summary_read_correctly() {
        let mut q = OcrQueue::new();
        let a = q.push(p("/x/a.pdf")).unwrap();
        let b = q.push(p("/x/b.png")).unwrap();
        let _c = q.push(p("/x/c.pdf")).unwrap();
        // 顺序：a 先被取走并失败，b 取走并完成，c 仍待识别。
        assert_eq!(q.take_next(), Some(0));
        q.mark_failed(a, "boom");
        assert_eq!(q.take_next(), Some(1));
        q.mark_done(b, "42 段");
        assert_eq!(q.counts(), (1, 0, 1, 1));
        assert!(q.has_work(), "还有一件待识别，has_work 必须为真");
        let s = q.summary();
        assert!(s.contains("3 件"), "{s}");
        assert!(s.contains("待识别 1"), "{s}");
        assert!(s.contains("失败 1"), "{s}");
    }

    #[test]
    fn remove_and_clear_leave_queue_consistent() {
        let mut q = OcrQueue::new();
        let a = q.push(p("/x/a.pdf")).unwrap();
        let b = q.push(p("/x/b.pdf")).unwrap();
        q.remove(a);
        assert_eq!(q.len(), 1);
        assert!(q.job(a).is_none());
        assert_eq!(q.job(b).unwrap().state, OcrState::Pending);
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.summary(), "识别队列为空");
        // 清空后 id 仍递增，不复用（避免 UI 拿着旧 id 误删新件）。
        let c = q.push(p("/x/c.pdf")).unwrap();
        assert!(c > b);
    }

    #[test]
    fn name_falls_back_to_whole_path() {
        let mut q = OcrQueue::new();
        q.push(p("/x/y/z.pdf"));
        assert_eq!(q.jobs()[0].name, "z.pdf");
    }
}
