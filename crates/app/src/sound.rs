//! 翻页音效：翻纸的「沙沙」声。
//!
//! 视觉拟物之外再补一层听觉拟物——翻面时一声纸响，手感立刻就实了。
//!
//! 做法上不引第三方音频库（会拖来一整套解码器），也不打包音频资源：
//! 直接**合成**一段 WAV 落进 `~/.jiandu/sounds/`，再用系统播放器放。
//! 纸声的物理构成很简单，合成得出来：
//!   * 主体是**宽带噪声**（纸张纤维摩擦）——白噪声过一阶高通，只留高频的「沙」，
//!     低通掉的那部分低频轰鸣是纸声里没有的；
//!   * 包络是**两记峰 + 一条指数尾巴**：第一记是纸页脱离手指，第二记是落下贴合；
//!   * 再叠几粒随机「爆音」（纤维崩断的那声脆响），密度别高，高了像电流声。
//!
//! 播放走**原生 `NSSound`**（v1.71.0）：不再 fork `afplay` 子进程——沙盒下
//! `PATH` / `HOME` 都不可靠，子进程随时被拦，且每次翻页起一个进程太贵。
//! `NSSound` 由 FFI 直调（不加新依赖、不要权限），`play` 本身是异步的，
//! 调完立刻返回，调用方不阻塞；连续翻页另有节流，不会叠成一团。

use crate::library::Library;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

/// 音效总开关（版字面板 / 设置里可关）。
static ENABLED: AtomicBool = AtomicBool::new(true);
/// 上次播放的毫秒时间戳（节流用）。
static LAST_MS: AtomicU64 = AtomicU64::new(0);
/// 合成文件只写一次。
static WAV: OnceLock<PathBuf> = OnceLock::new();

/// 采样率；44.1k 足够表现纸声的高频（再高徒增体积）。
const SR: u32 = 44100;
/// 时长（秒）：真实翻页约 0.2~0.3s，长了就拖沓。
const DUR: f32 = 0.26;
/// 节流间隔（毫秒）：快于此的连续翻页不重复出声。
const THROTTLE_MS: u64 = 90;

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 当前毫秒时间戳（取不到为 0）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn wav_path() -> PathBuf {
    Library::home().join("sounds").join("page_turn.wav")
}

/// 确定性 PRNG（xorshift）：同一份种子每次合成出同一段波形，
/// 文件可以缓存复用，不必每次启动重新算。
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// [-1, 1) 的均匀噪声。
    ///
    /// 取高 24 位（0 .. 2^24-1），故要除以 2^23 再减 1 才落到 [-1,1)。
    /// 除以 2^15 会让噪声实际范围是 [-1, 511) —— 幅度大到 tanh 全程饱和，
    /// 包络再怎么收都是满幅，「沙沙声」退化成一段持续白噪声。
    fn noise(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / 8_388_608.0 - 1.0
    }
}

/// 合成翻纸声，返回 16-bit 单声道采样。
pub fn synth() -> Vec<i16> {
    let n = (SR as f32 * DUR) as usize;
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut out = Vec::with_capacity(n);
    // 一阶高通的状态：y[n] = x[n] - x[n-1] 再带一点泄漏，避免完全微分后发尖。
    let mut prev_x = 0.0f32;
    let mut prev_y = 0.0f32;
    // 爆音：距下一粒还剩多少采样，以及当前这粒的剩余幅度。
    let mut crackle_in = (SR as f32 * 0.012) as i32;
    let mut crackle_amp = 0.0f32;

    for i in 0..n {
        let t = i as f32 / SR as f32;
        let x = rng.noise();
        // 高通（泄漏积分器）：截止约 1.2kHz，留下「沙」，滤掉「轰」。
        let a = 0.86;
        let y = x - prev_x + a * prev_y;
        prev_x = x;
        prev_y = y;

        // 包络：极短起音 → 两记峰 → 指数尾巴。
        let attack = 1.0 - (-t / 0.004).exp();
        let tail = (-t / 0.115).exp();
        // 第二记峰（纸落下）落在 ~90ms。
        let second = 0.55 * (-(t - 0.09).abs() / 0.030).exp();
        let swell = (0.75 + 0.25 * (2.0 * std::f32::consts::PI * 11.0 * t).sin())
            .max(0.0);
        let env = attack * tail * swell + second;

        // 纤维崩断的脆响：稀疏触发，触发后幅度指数跌落（几十采样内收干净）。
        crackle_in -= 1;
        if crackle_in <= 0 {
            crackle_in = (SR as f32 * (0.010 + 0.030 * rng.noise().abs())) as i32;
            crackle_amp = 0.25 + 0.25 * rng.noise().abs();
        }
        let crackle = crackle_amp * rng.noise();
        crackle_amp *= 0.90;

        // 脆响也必须乘整体包络：它是周期性重新触发的，不乘的话整段都在响，
        // 「沙」一下就收的纸声会变成持续白噪声（测试 head/tail 能量比会抓到）。
        let mut s = (y * 0.42 + crackle) * env;
        // 软削波：纸声不该有数字过载的硬边。
        s = s.tanh();
        out.push((s * 22000.0).clamp(-32768.0, 32767.0) as i16);
    }
    out
}

/// 写 44 字节 RIFF/WAVE 头 + PCM 数据。
fn encode_wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + samples.len() * 2);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk 长度
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    v.extend_from_slice(&SR.to_le_bytes());
    v.extend_from_slice(&(SR * 2).to_le_bytes()); // 字节率
    v.extend_from_slice(&2u16.to_le_bytes()); // 块对齐
    v.extend_from_slice(&16u16.to_le_bytes()); // 位深
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

/// 确保 WAV 已落盘，返回其路径。
fn ensure_wav() -> Option<&'static PathBuf> {
    if let Some(p) = WAV.get() {
        return Some(p);
    }
    let p = wav_path();
    if p.exists() {
        return Some(WAV.get_or_init(|| p));
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    if std::fs::write(&p, encode_wav(&synth())).is_ok() {
        Some(WAV.get_or_init(|| p))
    } else {
        None
    }
}

// ----------------------------------------------------------------------
// 播放：原生 NSSound（v1.71.0，取代 `afplay` 子进程）
//
// 三个要点：
// 1. **不加新依赖**：`objc_getClass` / `sel_registerName` / `objc_msgSend`
//    三个符号都在 libobjc 里，直接 `extern "C"` 声明即可（gpui 本身也这么
//    链接 libobjc），不必拖进 `objc` / `cocoa` crate。
// 2. **一台对象复用**：`[[NSSound alloc] initWithContentsOfFile:…]` 只建一次，
//    缓存在 `OnceLock` 里（存 `usize` 以免 `*mut c_void` 不是 Send/Sync）。
//    `init` 出来的对象我们持有一份引用计数，不会被自动释放池收走。
// 3. **主线程直调**：`play` 立即返回、播放由 AppKit 在后台完成；主线程也
//    自带自动释放池（`stringWithUTF8String:` 返回的临时 NSString 有池可归）。
//    失败一律静默——没声音不影响读书。
// ----------------------------------------------------------------------

/// 缓存的 NSSound 对象（裸指针按 usize 存，跨线程读需要 Send/Sync）。
static SOUND: OnceLock<usize> = OnceLock::new();

#[cfg(target_os = "macos")]
unsafe fn nssound_play_file(path: &str) -> bool {
    use std::ffi::CString;
    use std::os::raw::c_char;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut std::os::raw::c_void;
        fn sel_registerName(name: *const c_char) -> *mut std::os::raw::c_void;
        fn objc_msgSend(
            receiver: *mut std::os::raw::c_void,
            op: *mut std::os::raw::c_void,
            ...
        ) -> *mut std::os::raw::c_void;
    }

    let sel = |n: &[u8]| -> *mut std::os::raw::c_void {
        sel_registerName(n.as_ptr() as *const c_char)
    };
    // 空指针 / 空类一律当「这台机器没有 NSSound」，静默降级。
    let cls = objc_getClass(b"NSSound\0".as_ptr() as *const c_char);
    if cls.is_null() {
        return false;
    }
    let ns_string = objc_getClass(b"NSString\0".as_ptr() as *const c_char);
    if ns_string.is_null() {
        return false;
    }
    let Ok(cpath) = CString::new(path) else {
        return false;
    };
    let s = objc_msgSend(
        ns_string,
        sel(b"stringWithUTF8String:\0"),
        cpath.as_ptr(),
    );
    if s.is_null() {
        return false;
    }
    let alloc = objc_msgSend(cls, sel(b"alloc\0"));
    if alloc.is_null() {
        return false;
    }
    let snd = objc_msgSend(
        alloc,
        sel(b"initWithContentsOfFile:byReference:\0"),
        s,
        0i32,
    );
    if snd.is_null() {
        return false;
    }
    // BOOL 返回值不关心（失败也只是没声音），故按指针收一下即可。
    let _ = objc_msgSend(snd, sel(b"play\0"));
    SOUND.get_or_init(|| snd as usize);
    true
}

#[cfg(not(target_os = "macos"))]
unsafe fn nssound_play_file(_path: &str) -> bool {
    false
}

/// 放一声翻纸。节流 + 总开关都在这里拦；失败静默（没声音不影响读书）。
pub fn page_turn() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let now = now_ms();
    let last = LAST_MS.load(Ordering::Relaxed);
    if now > last && now - last < THROTTLE_MS {
        return;
    }
    LAST_MS.store(now, Ordering::Relaxed);

    let Some(path) = ensure_wav() else { return };
    // 已建过对象就直接重放（省一次文件解析），否则按路径建一台。
    if let Some(p) = SOUND.get() {
        #[cfg(target_os = "macos")]
        {
            use std::os::raw::c_void;
            #[link(name = "objc")]
            extern "C" {
                fn objc_msgSend(
                    receiver: *mut c_void,
                    op: *mut c_void,
                    ...
                ) -> *mut c_void;
                fn sel_registerName(name: *const std::os::raw::c_char) -> *mut c_void;
            }
            let snd = *p as *mut c_void;
            unsafe {
                let _ = objc_msgSend(
                    snd,
                    sel_registerName(b"play\0".as_ptr() as *const std::os::raw::c_char),
                );
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = p;
        return;
    }
    let path = path.clone();
    let _ = unsafe { nssound_play_file(&path.to_string_lossy()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_is_deterministic_and_in_range() {
        let a = synth();
        let b = synth();
        assert_eq!(a, b, "同一段纸声每次合成必须一致（才能缓存复用）");
        assert!(!a.is_empty());
        assert!(a.iter().all(|s| *s >= -32768));
    }

    #[test]
    fn synth_has_energy_then_decays() {
        // 前段（纸页摩擦）能量应明显高于尾段（收音）——否则听不出「翻」。
        let s = synth();
        let rms = |seg: &[i16]| -> f32 {
            let sum: f64 = seg.iter().map(|x| (*x as f64) * (*x as f64)).sum();
            (sum / seg.len() as f64).sqrt() as f32
        };
        let head = rms(&s[..s.len() / 6]);
        let tail = rms(&s[s.len() * 5 / 6..]);
        assert!(head > tail * 2.0, "纸声应前响后收：head={head} tail={tail}");
        // 也不是死寂：整段都得有能量。
        assert!(head > 100.0, "纸声太轻听不见：{head}");
    }

    #[test]
    fn wav_header_is_well_formed() {
        let s = synth();
        let w = encode_wav(&s);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(&w[36..40], b"data");
        // RIFF 声明的长度 = 文件总长 - 8。
        let declared = u32::from_le_bytes([w[4], w[5], w[6], w[7]]) as usize;
        assert_eq!(declared, w.len() - 8);
        assert_eq!(w.len(), 44 + s.len() * 2);
    }
}
