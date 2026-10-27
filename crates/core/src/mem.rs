//! **内存护栏**（v1.81.0）：后台重活开工前，先看一眼自己吃了多少内存。
//!
//! # 为什么要有这一层
//!
//! 三件重活都在后台线程跑：FTS5 整本建索引、批量导入一本接一本刻、大文件
//! 流式解析。它们各自都不算离谱，但**堆在一起**就会出事：索引刚把一本书的
//! 全部章节 JSON 摊进内存，导入又在解另一本 EPUB，macOS 开始紧张地向磁盘
//! 换页——主线程下一次重绘就被拖住，用户看到的是「翻页卡一下」。
//!
//! 护栏的做法只有一条：**在块与块之间看一眼常驻内存（RSS），超过限额就在
//! 后台线程里歇一会儿**。绝不做的事：
//!   * 不杀任务、不丢数据——歇够了接着干；
//!   * 不在主线程里等（`breathe` 只许后台线程调）；
//!   * 不猜——探针拿不到数（非 macOS / 权限被拒 / 单元测试）就**直行**，
//!     宁可不管，也不凭一个编出来的数字卡住用户的活。
//!
//! # 分层
//!
//! 策略（阈值、退让、统计）在本模块，纯逻辑、可单测；**怎么读 RSS 是壳子的
//! 事**（macOS 用 `task_info`，见 `crates/app/src/mem.rs`），core 不许
//! `#[cfg(target_os)]`——这是 `scripts/check_core_purity.sh` 盯着的红线。
//! 壳子启动时 [`install_probe`] 交一个函数指针进来即可。

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// 默认限额：2 GB 常驻。用户可以在环境变量里改（见 [`limit`]）。
pub const DEFAULT_LIMIT_MB: u64 = 2048;

/// 退让线：降到限额的这个百分比以下才继续开工。
///
/// 卡在限额上反复「刚过线就停、降一点就走」会变成抖动的轮询；留 15% 的
/// 迟滞，一次退让就能干一段实活。
pub const RESUME_PCT: u64 = 85;

/// 退让时的轮询间隔（毫秒）。
pub const POLL_MS: u64 = 60;

/// 一次退让最多等多久（毫秒）。到点还没降下来就**照常继续**——
/// 宁可慢一点，也不能让用户的活永远停在这儿。
pub const MAX_WAIT_MS: u64 = 4_000;

/// 读常驻内存的探针：返回 `None` = 这台机器读不到（那就别管）。
pub type Probe = fn() -> Option<u64>;

/// 0 表示没装探针（函数指针不会是 0）。
static PROBE: AtomicUsize = AtomicUsize::new(0);
/// 累计退让次数（排查「为什么索引建得慢」时看这个）。
static PAUSES: AtomicU64 = AtomicU64::new(0);
/// 见过的最高的 RSS（只在有探针时更新）。
static PEAK: AtomicU64 = AtomicU64::new(0);
/// 最近一次读到的 RSS。
static LAST: AtomicU64 = AtomicU64::new(0);

/// 装探针（壳子启动时调一次；重复调以最后一次为准）。
pub fn install_probe(p: Probe) {
    PROBE.store(p as usize, Ordering::Relaxed);
}

/// 现在的探针（没装就是 `None`）。
pub fn probe() -> Option<Probe> {
    match PROBE.load(Ordering::Relaxed) {
        0 => None,
        // SAFETY: 只由 `install_probe` 写入，写入的就是一个 `fn() -> Option<u64>`。
        n => Some(unsafe { std::mem::transmute::<usize, Probe>(n) }),
    }
}

/// 限额（字节）。`JIANDU_MEM_LIMIT_MB` 可覆盖——调试时想复现护栏不必真吃 2G。
pub fn limit() -> u64 {
    let mb = std::env::var("JIANDU_MEM_LIMIT_MB")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_LIMIT_MB);
    mb * 1024 * 1024
}

/// 读一次 RSS，并顺手更新统计。
pub fn resident() -> Option<u64> {
    let v = probe()?()?;
    LAST.store(v, Ordering::Relaxed);
    PEAK.fetch_max(v, Ordering::Relaxed);
    Some(v)
}

/// 该不该让路：**到线就停**（不是超过才算）。
///
/// `None`（读不到）永远不停——护栏不该比它保护的东西更危险。
pub fn should_pause(limit: u64, resident: Option<u64>) -> bool {
    match resident {
        Some(r) => r >= limit.max(1),
        None => false,
    }
}

/// 可以复工了没：降到退让线以下。
pub fn should_resume(limit: u64, resident: Option<u64>) -> bool {
    match resident {
        Some(r) => r < limit.max(1) * RESUME_PCT / 100,
        None => true,
    }
}

/// **分块任务在块与块之间调这个**（只许后台线程）。
///
/// 超限就睡 [`POLL_MS`]，最多等 [`MAX_WAIT_MS`]，降到退让线即返回；等不到
/// 也照常返回——慢慢干总比不干强。返回 `true` 表示这次真的让过路（调用方
/// 可以据此打日志，或把块再切小一点）。
pub fn breathe(tag: &str) -> bool {
    let limit = limit();
    let Some(now) = resident() else {
        return false;
    };
    if !should_pause(limit, Some(now)) {
        return false;
    }
    let t0 = std::time::Instant::now();
    let mut waited = 0u64;
    while !should_resume(limit, resident()) {
        if t0.elapsed().as_millis() as u64 >= MAX_WAIT_MS {
            // 等不到就放行：用户的活不能永远停在这儿。
            eprintln!(
                "[mem] {tag}：等 {}ms 仍在限额以上，照常继续（rss≈{}MB / 限额 {}MB）",
                waited,
                resident().unwrap_or(0) / (1024 * 1024),
                limit / (1024 * 1024)
            );
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
        waited += POLL_MS;
    }
    PAUSES.fetch_add(1, Ordering::Relaxed);
    true
}

/// 护栏一瞥：给调试面板 / 日志用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub limit: u64,
    pub resident: u64,
    pub peak: u64,
    pub pauses: u64,
    /// 有没有装探针（false 时上面几个数都是 0，护栏等于不存在）。
    pub probed: bool,
}

/// 现在的护栏状态（会真读一次内存）。
pub fn snapshot() -> Snapshot {
    let r = resident();
    Snapshot {
        limit: limit(),
        resident: r.unwrap_or(0),
        peak: PEAK.load(Ordering::Relaxed),
        pauses: PAUSES.load(Ordering::Relaxed),
        probed: r.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    #[test]
    fn pause_at_the_line_and_resume_below_hysteresis() {
        let limit = 100 * MB;
        assert!(!should_pause(limit, None), "读不到就别管");
        assert!(!should_pause(limit, Some(99 * MB)));
        // 到线就停：不是「超过」才停。
        assert!(should_pause(limit, Some(100 * MB)));
        assert!(should_pause(limit, Some(150 * MB)));

        // 退让线 85%：降到 84MB 才复工，85MB 仍在等。
        assert!(!should_resume(limit, Some(85 * MB)));
        assert!(should_resume(limit, Some(84 * MB)));
    }

    #[test]
    fn zero_limit_does_not_divide_by_zero_or_always_pause() {
        // 手改环境变量写了 0 → `limit()` 会滤掉，这里直接测策略函数兜底。
        assert!(should_pause(0, Some(1)));
        assert!(!should_resume(0, Some(0)));
    }

    #[test]
    fn no_probe_means_breathe_returns_immediately() {
        // 单元测试里没装探针：绝不能 sleep 4 秒。
        let t0 = std::time::Instant::now();
        assert!(!breathe("test"));
        assert!(t0.elapsed().as_millis() < 200, "没探针却等了这么久");
    }

    #[test]
    fn probe_can_be_installed_and_read() {
        static FAKE: AtomicU64 = AtomicU64::new(0);
        fn fake() -> Option<u64> {
            Some(FAKE.load(Ordering::Relaxed))
        }
        FAKE.store(123 * MB, Ordering::Relaxed);
        install_probe(fake);
        assert_eq!(resident(), Some(123 * MB));
        assert!(snapshot().probed);
        // 收拾干净，别影响同进程里的其它测试。
        FAKE.store(0, Ordering::Relaxed);
        install_probe(|| None);
    }
}
