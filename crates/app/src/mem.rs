//! **内存探针（macOS）**：core 的护栏要一个「现在吃了多少」的数，这里给它。
//!
//! `task_info(TASK_BASIC_INFO_64)` 读的是**本进程**的 `resident_size`——
//! 不用权限、不用子进程（绝不 fork `ps`：那玩意儿自己就要几十毫秒，还会
//! 在沙盒里被拦）。与 `sound.rs` 直调 libobjc 是同一路数：系统调用一层薄
//! FFI，不算「依赖某个 UI 框架」。
//!
//! 读不到（极少数情况，比如返回值非 `KERN_SUCCESS`）就返 `None`——core 那边
//! 见到 `None` 会**直行**，护栏退化成不存在。宁可不管，也不拿一个编出来的
//! 数字卡住用户的活。

/// `mach_task_basic_info_data_t` 的逐字段镜像（顺序与字段宽度必须与
/// `<mach/task_info.h>` 完全一致，差一个字节后面的字段全错位）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MachTaskBasicInfo {
    virtual_size: u64,
    resident_size: u64,
    resident_size_max: u64,
    /// `time_value_t` = { seconds, microseconds }，各 32 位。
    user_time: [i32; 2],
    system_time: [i32; 2],
    policy: i32,
    suspend_count: i32,
}

/// `TASK_BASIC_INFO_64` 的 flavor 号（`<mach/task_info.h>`）。
const TASK_BASIC_INFO_64: u32 = 5;

extern "C" {
    /// libSystem 导出的**变量**（`mach_task_self()` 在 C 里是个宏，实际读的
    /// 就是它）。声明成函数会链接不上——这是这类 FFI 最常见的坑。
    static mach_task_self_: u32;
    /// `kern_return_t task_info(task_name_t, task_flavor_t, task_info_t, mach_msg_type_number_t *)`
    fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
}

/// 本进程常驻内存（字节）。读不到返回 `None`。
fn resident() -> Option<u64> {
    let mut info = MachTaskBasicInfo::default();
    // count 的单位是 `natural_t`（4 字节），不是结构体个数。
    let mut count = (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<i32>()) as u32;
    // SAFETY: `info` 是本地栈上一块足量的、按 C 布局的缓冲；`count` 与它一致。
    let kr = unsafe {
        task_info(
            mach_task_self_,
            TASK_BASIC_INFO_64,
            &mut info as *mut MachTaskBasicInfo as *mut i32,
            &mut count,
        )
    };
    if kr == 0 {
        Some(info.resident_size)
    } else {
        None
    }
}

/// 把探针交给 core 的护栏（启动时调一次）。
pub fn install() {
    jiandu_core::mem::install_probe(resident);
}

/// 今天吃了多少（调试面板 / 日志用）。
pub fn snapshot() -> jiandu_core::mem::Snapshot {
    jiandu_core::mem::snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_plausible_resident_size() {
        // 任何跑得起来的 Rust 进程都不止 1MB、也不会超过 64GB。
        let r = resident().expect("macOS 上 task_info 不该失败");
        assert!(r > 1024 * 1024, "RSS 太小，字段错位了？{r}");
        assert!(r < 64 * 1024 * 1024 * 1024, "RSS 大得离谱：{r}");
    }
}
