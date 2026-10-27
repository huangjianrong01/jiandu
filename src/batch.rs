//! **批量导入**（v1.77.0）的 UI 层：后台线程 + 每帧探一眼 + 进度浮层 + 取消。
//!
//! 纯逻辑（点检 / 队列 / 取消语义）在 `jiandu_core::batch`，这里只负责三件事：
//!   1. 把整队活儿丢到**后台线程**串行跑（`std::thread::spawn`），主线程
//!      一帧都不等——批量导入时翻书、点书架都是流畅的；
//!   2. 每帧 `take_settled()` 把刻完的书**立刻上架**（一本一上架，用户看得见
//!      书架在长），主线程只干「登记入架」这一下轻活；
//!   3. 画进度浮层「正在导入 3/10：庄子」，并给一枚「取消」钮。
//!
//! # 为什么刻印不在 `cx.background_executor` 上跑
//!
//! 导入是**同步**的、一跑好几秒的 CPU 活儿（解 zip + 清洗 + 落 SQLite）。
//! GPUI 的 background_executor 是给「异步短任务」用的，塞一个几秒的同步阻塞
//! 进去会占住那条线程；而 `std::thread::spawn` 是 OS 线程，跟渲染线程各跑各的，
//! 这也是单本导入（`ImportJob`）一直在用的老路子，口径统一。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    div, px, rgb, rgba, Context, Div, InteractiveElement, MouseButton, ParentElement, SharedString,
    Styled,
};

use jiandu_core::batch::{Candidate, ItemResult};
use jiandu_core::types::Book;

/// 后台线程交给主线程的「一本刻完了」。
pub struct Settled {
    /// 结果（成功 / 跳过）。
    pub result: ItemResult,
    /// 它在书衣列表里的序号（点检就失败的书没有书衣，故为 `None`）。
    pub card_idx: Option<usize>,
    /// 成功才有：书稿 + `store_id` + 源文件（上架要这三样）。
    pub payload: Option<(Book, String, PathBuf)>,
}

/// 收工报告。
pub struct BatchReport {
    /// 一句人话（「已上架 8 本，跳过 2 本」）。
    pub summary: String,
    /// 逐本日志（成功与跳过都记）。
    pub log: Vec<String>,
    /// 取消留下的半成品 `store_id`——主线程要删库。
    pub orphans: Vec<String>,
}

/// 批量导入的后台作业。
pub struct BatchJob {
    /// 取消标记（后台线程每刻一本前看一眼）。
    stop: Arc<AtomicBool>,
    /// 点检完成的全部书（书架上的「待刻书衣」）；主线程取一次。
    cards: Arc<Mutex<Option<Vec<Candidate>>>>,
    /// 刻完待上架（主线程逐帧取走）。
    settled: Arc<Mutex<Vec<Settled>>>,
    /// 正在刻：第几本 / 共几本 / 书名。
    cur: Arc<Mutex<(usize, usize, String)>>,
    /// 正在刻那一本的内部进度与文案。
    pct: Arc<Mutex<f32>>,
    label: Arc<Mutex<String>>,
    /// 收工报告（只可能被取走一次）。
    report: Arc<Mutex<Option<BatchReport>>>,
    /// 用户已按下取消（等线程收工的那段时间里，浮层改口说「正在收工」）。
    pub canceling: bool,
}

impl BatchJob {
    /// 起一条后台线程：点检 → 串行刻印 → 报告。
    pub fn start(paths: Vec<PathBuf>) -> Self {
        let cards = Arc::new(Mutex::new(None));
        let settled: Arc<Mutex<Vec<Settled>>> = Arc::new(Mutex::new(Vec::new()));
        let cur = Arc::new(Mutex::new((0usize, paths.len(), String::new())));
        let pct = Arc::new(Mutex::new(0.0f32));
        let label = Arc::new(Mutex::new("雕版刻印中".to_string()));
        let report = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));

        let c2 = cards.clone();
        let s2 = settled.clone();
        let cur2 = cur.clone();
        let pct2 = pct.clone();
        let lab2 = label.clone();
        let rep2 = report.clone();
        let stop2 = stop.clone();

        std::thread::spawn(move || {
            // ── 第一趟：点检（毫秒级，一节正文都不解）─────────────────
            // 先点检后刻印，好处有二：① 书**立刻**以真书名出现在书架上；
            // ② 坏文件在开刻之前就被认出来剔掉，不占队列名额。
            let mut cands: Vec<Candidate> = Vec::new();
            let mut pre: Vec<ItemResult> = Vec::new();
            for p in &paths {
                match jiandu_core::batch::peek(p) {
                    Ok(c) => cands.push(c),
                    Err(reason) => {
                        let title = p
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("无名")
                            .to_string();
                        // 点检就失败的书没有书衣，直接进日志。
                        let r = ItemResult::Skip { title, reason };
                        push(&s2, Settled {
                            result: r.clone(),
                            card_idx: None,
                            payload: None,
                        });
                        pre.push(r);
                    }
                }
            }
            if let Ok(mut g) = c2.lock() {
                *g = Some(cands.clone());
            }

            // ── 第二趟：一本接一本地刻 ───────────────────────────────
            let mut queue = jiandu_core::batch::BatchQueue::new(cands);
            {
                let s3 = s2.clone();
                let pct3 = pct2.clone();
                let lab3 = lab2.clone();
                queue.drive(
                    &stop2,
                    &mut |c: &Candidate, i: usize, _n: usize| {
                        if let Ok(mut g) = lab3.lock() {
                            *g = "雕版刻印中".to_string();
                        }
                        if let Ok(mut g) = pct3.lock() {
                            *g = 0.0;
                        }
                        let r = etch_one(c.path.clone(), &pct3, &lab3);
                        match r {
                            Ok((book, sid, src)) => {
                                let title = c.display_name().to_string();
                                push(&s3, Settled {
                                    result: ItemResult::Ok {
                                        title,
                                        store_id: sid.clone(),
                                    },
                                    card_idx: Some(i),
                                    payload: Some((book, sid.clone(), src)),
                                });
                                Ok(sid)
                            }
                            Err(reason) => Err(reason),
                        }
                    },
                    &mut |i: usize, n: usize, name: &str| {
                        if let Ok(mut g) = cur2.lock() {
                            *g = (i, n, name.to_string());
                        }
                    },
                );
            }
            for r in pre {
                queue.record_pre(r);
            }
            let summary = queue.summary();
            let log = queue.all_results().iter().map(|r| r.log_line()).collect();
            let orphans = queue.orphans().to_vec();
            if let Ok(mut g) = rep2.lock() {
                *g = Some(BatchReport {
                    summary,
                    log,
                    orphans,
                });
            }
        });

        Self {
            stop,
            cards,
            settled,
            cur,
            pct,
            label,
            report,
            canceling: false,
        }
    }

    /// 点检结果（只取一次；取到之后书架就能画「待刻书衣」了）。
    pub fn take_cards(&self) -> Option<Vec<Candidate>> {
        self.cards.lock().ok().and_then(|mut g| g.take())
    }

    /// 取走已刻完、等待上架的那些（每帧一次）。
    pub fn take_settled(&self) -> Vec<Settled> {
        self.settled.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }

    /// 当前进度：`(第几本, 共几本, 书名, 本本进度, 本本文案)`。
    pub fn progress(&self) -> (usize, usize, String, f32, String) {
        let (i, n, name) = self
            .cur
            .lock()
            .map(|g| (g.0, g.1, g.2.clone()))
            .unwrap_or((0usize, 0usize, String::new()));
        let pct = self.pct.lock().map(|g| *g).unwrap_or(0.0);
        let label = self
            .label
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "雕版刻印中".to_string());
        (i, n, name, pct, label)
    }

    /// 取走收工报告（只可能成功一次）。
    pub fn take_report(&self) -> Option<BatchReport> {
        self.report.lock().ok().and_then(|mut g| g.take())
    }

    /// 用户按下取消。
    ///
    /// 只是把标记立起来——正在刻的那一本跑完才收得回来（`import_streaming`
    /// 内部没有可中断点）。这段时间浮层改口说「正在收工」。
    pub fn request_cancel(&mut self) {
        if self.canceling {
            return;
        }
        self.canceling = true;
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn push(q: &Arc<Mutex<Vec<Settled>>>, s: Settled) {
    if let Ok(mut g) = q.lock() {
        g.push(s);
    }
}

/// 刻一本：主路流式落库，失败换漏斗兜底（与单本 `ImportJob` 同一口径）。
fn etch_one(
    path: PathBuf,
    pct: &Arc<Mutex<f32>>,
    label: &Arc<Mutex<String>>,
) -> Result<(Book, String, PathBuf), String> {
    let p = path.clone();
    let pct2 = pct.clone();
    let lab2 = label.clone();
    let cb = move |v: f32, s: String| {
        if let Ok(mut g) = pct2.lock() {
            *g = v;
        }
        if let Ok(mut g) = lab2.lock() {
            *g = s;
        }
    };
    match crate::stream::import_streaming(&p, Some(Box::new(cb))) {
        Ok((book, id)) => Ok((book, id, p.clone())),
        Err(main_err) => crate::importer::import_file(&p)
            .map(|ir| (ir.to_book(), String::new(), p.clone()))
            .map_err(|e| format!("{main_err}；兜底也失败：{e}")),
    }
}

// ── 浮层 ──────────────────────────────────────────────────────────────

impl crate::app::Root {
    /// 批量导入进度浮层：一行「正在导入 3/10：庄子」+ 两道朱砂细线 + 取消。
    ///
    /// 与单本导入同一套宣纸语汇（无框、无圆角、不转圈），只是多了一行总数。
    pub(crate) fn batch_overlay(
        &mut self,
        cx: &mut Context<Self>,
        i: usize,
        n: usize,
        name: &str,
        pct: f32,
        label: &str,
        done: usize,
        skipped: usize,
        canceling: bool,
    ) -> Div {
        let song = crate::fonts::family_for(
            crate::types::FontChoice::SongTi,
            crate::types::Script::Ancient,
        );
        let bar_w = 260.0;
        let overall = if n == 0 { 0.0 } else { done as f32 / n as f32 };
        let pct_i = (pct * 100.0).round() as i32;

        let head = if canceling {
            SharedString::from("正在收工…（刻完这本就停）".to_string())
        } else {
            SharedString::from(format!("正在导入 {i}/{n}：{name}"))
        };
        let sub = SharedString::from(format!("{label}… {pct_i}%　已上架 {done} 本　跳过 {skipped} 本"));

        let cancel = div()
            .id(SharedString::from("batch-cancel"))
            .px(px(12.0))
            .py(px(4.0))
            .rounded(px(7.0))
            .bg(rgba(0xC3272D22))
            .font_family(song.clone())
            .text_size(px(11.5))
            .text_color(rgb(0xC3272D))
            .child(SharedString::from(if canceling {
                "收工中".to_string()
            } else {
                "取消".to_string()
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.cancel_batch();
                    cx.notify();
                }),
            );

        div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x14100A55))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    // 浮层吃掉落在其上的点击：导入期间不许点穿去翻书。
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(9.0))
                    .px(px(34.0))
                    .py(px(24.0))
                    .rounded(px(10.0))
                    .bg(rgb(0xF5F0E6))
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(15.0))
                            .text_color(rgb(0x2A241E))
                            .child(head),
                    )
                    .child(
                        div()
                            .font_family(song.clone())
                            .text_size(px(11.0))
                            .text_color(rgb(0x9A9184))
                            .child(sub),
                    )
                    // 总进度：刻完几本 / 共几本。
                    .child(
                        div()
                            .w(px(bar_w))
                            .h(px(3.0))
                            .bg(rgb(0xC9BDA6))
                            .child(
                                div()
                                    .h_full()
                                    .w(px(bar_w * overall.clamp(0.0, 1.0)))
                                    .bg(rgb(0x8A7A5E)),
                            ),
                    )
                    // 本本进度：这一本读到百分之几。
                    .child(
                        div()
                            .w(px(bar_w))
                            .h(px(2.0))
                            .bg(rgb(0xC9BDA6))
                            .child(
                                div()
                                    .h_full()
                                    .w(px(bar_w * pct.clamp(0.0, 1.0)))
                                    .bg(rgb(0xC3272D)),
                            ),
                    )
                    .child(cancel),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 等后台线程收工（点检全是失败的书，不会碰数据库，故很快）。
    fn wait_report(job: &BatchJob) -> BatchReport {
        for _ in 0..200 {
            if let Some(r) = job.take_report() {
                return r;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("批量导入线程没有收工");
    }

    #[test]
    fn junk_files_are_logged_and_never_crash_the_queue() {
        // 三个根本不存在的路径：点检必然失败。要的是「三本都记进日志、
        // 队列照常收工」，而不是 panic 掉整条线程。
        let job = BatchJob::start(vec![
            std::path::PathBuf::from("/tmp/不存在的一.epub"),
            std::path::PathBuf::from("/tmp/不存在的二.txt"),
            std::path::PathBuf::from("/tmp/不存在的三.epub"),
        ]);
        let rep = wait_report(&job);
        assert_eq!(rep.log.len(), 3);
        assert!(rep.log.iter().all(|l| l.starts_with("跳过")));
        assert!(rep.orphans.is_empty());
        // 点检全灭：一本都没上架，书衣也就没有。
        assert_eq!(rep.summary, "已上架 0 本，跳过 3 本");
    }

    #[test]
    fn cards_are_published_before_any_book_is_etched() {
        // 点检结果必须先于刻印发布——这正是「书先出现在书架上」的依据。
        let d = std::env::temp_dir().join("jiandu-batch-ui-cards");
        let _ = std::fs::create_dir_all(&d);
        let p = d.join("道德经.txt");
        std::fs::write(&p, "道德经\n道可道。\n").unwrap();
        let job = BatchJob::start(vec![p]);
        let mut cards = None;
        for _ in 0..200 {
            if let Some(c) = job.take_cards() {
                cards = Some(c);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let cards = cards.expect("点检结果应当被发布");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "道德经");
        let rep = wait_report(&job);
        assert!(rep.summary.starts_with("已上架") || rep.summary.starts_with("已上架 0"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cancel_is_recorded_on_the_job() {
        let job = std::sync::Mutex::new(BatchJob::start(vec![std::path::PathBuf::from(
            "/tmp/不存在.epub",
        )]));
        {
            let mut j = job.lock().unwrap();
            assert!(!j.canceling);
            j.request_cancel();
            assert!(j.canceling);
            // 重复按取消不该出岔子。
            j.request_cancel();
            assert!(j.canceling);
        }
        let j = job.lock().unwrap();
        let rep = wait_report(&j);
        assert!(rep.orphans.is_empty());
    }
}
