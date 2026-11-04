//! 笺读 JianDu —— 原生中文阅读器
//!
//! Rust + GPUI 实现：
//!   * 书架：书封墙、导入 md/txt/epub/json、阅读档案记忆；
//!   * 封面：竖排书名、副题、朱印；
//!   * 古文竖笺：从右到左，每叶八行，朱丝栏，繁体标注、通假标注、注释脚注；
//!   * 现代横笺：仅横线信笺，不设竖格；
//!   * 单面 / 双面对开阅读。
//!
//! v1.47.0：纯逻辑已解耦至 `jiandu-core` crate（grid / parsers / dict 等），
//! 本二进制只留 UI 层（app / reader / shelf）与绘制耦合的排版辅助。

// ── 交付态 lint 政策（v1.55.0 发布前净化）──────────────────────────
// 以下两类的警告是**有意保留**的，不是遗漏，故在 crate 根统一放行，
// 免得逐处加 `#[allow]` 把无关代码搅乱：
//   * dead_code —— 旧分享卡 / Zen 长图 / 翻页音效等按「渐进式重构」要求
//     留着不物理删除，接口保留到下一轮统一清理；
//   * too_many_arguments —— 排版函数按几何逐项传值（列宽 / 行高 / 天头 /
//     地脚…），硬拆成 struct 反而掩盖每一项的意义。
#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]

/// **问小笺**（v1.76.0 · V2.0 模块二）：AI 伴读侧栏。
///
/// 做成独立 Entity（`AiPanel`）——流式输出每秒 notify 几十次，状态若住在
/// `Root` 里会带着整棵阅读树重绘。落盘（密钥 / 服务商）见 `jiandu_core::ai`。
/// **macOS 顶部状态栏常驻**（v1.9.44）：极简「笺」图标 + 毛玻璃下拉菜单
/// （今日阅卷 / 快速笺校 / 书架 / 最近打开 / 检查更新 / 退出）。
mod menubar;
mod ai;
mod app;
/// **本地日历日**（v1.9.38）：书架「今日读叶」与落款日期的唯一真源。
/// 时区只有平台知道，故 `localtime_r` 落在这里；core 只管纯日期算术。
mod clock;
/// **笺校工作台**（MVP · 模块三）：与阅读器完全解耦的全屏模态视图。
/// 顶层状态机 `AppMode` 在此；进入时 `Root::render` 直接返回它的视图实体，
/// 根本不构建阅读界面（见 `collation.rs` 文件头）。
mod collation;
/// **PDF 文本层坐标抽取**（macOS / PDFKit）：把带文本层的古籍 PDF 逐字包围盒
/// 聚合成 `SourceRegion`，是 `collation` 真实坐标映射的数据源。
mod pdf_text;
/// **内测闸门**（v1.0.0-beta）：启动时那张「箋讀 + 邀請碼」的纸。
/// 纯逻辑（码表 / 归一 / 凭据）在 `jiandu_core::gate`。
mod beta;
/// **批量导入**（v1.77.0）：后台串行刻印的作业对象 + 进度浮层。
/// 纯逻辑（点检 / 队列 / 取消语义）在 `jiandu_core::batch`。
mod batch;
/// **卡片匣**（v1.74.0）：跨书摘录卡的 UI 层（落盘见 `jiandu_core::cards`）。
mod cards;
// v1.71.0：剪贴板唯一入口（原生 NSPasteboard，取代 osascript / pbpaste）。
mod clipboard;
/// **繁简转换引擎（macOS · ICU）**（v1.81.0）：`Hans-Hant` / `Hant-Hans` 词组级
/// 变换。core 只定档位与兜底表，真正转字在这里——core 不许碰 objc。
mod conv;
/// **拖拽入书**（v1.9.5）：把 Finder 拖进来的文件接住。
///
/// gpui 0.2.2 没有文件拖放 API，这里手写 libobjc / AppKit 直调，给窗口
/// 的 NSView 补上 `performDragOperation:`——不加任何新依赖（纪律同 `sound`）。
mod dropzone;
mod exporter;
/// **逐字字体回退**（v1.9.25）：首选古籍字体缺字时，只许回退到系统 CJK
/// 字库（西文字体一律不许），全系统无解则画占位符并打 Warning。
/// 只挑字体、不参与几何——字格宽高恒由网格引擎给（见 `font_fallback` 头注）。
mod font_fallback;
mod fonts;
// `gloss`（双行夹注排版引擎）v1.0.0-beta 起住在 `jiandu-core`，本 crate 只
// 再导出（见下方 `pub use jiandu_core::{… gloss …}`）——跨平台壳子要的是
// 同一份 `Page`，不该被 GPUI 壳子绑架。
mod ime;
// v1.62.0：多格式导入的**漏斗**（各格式只出纯文本 → cleaner 唯一出口 →
// core 管线 → BookIR）。旧 `importer::*` API 在本模块内原样透传。
mod importer;
// v1.81.0：**快捷键壳**。键位表在 `jiandu_core::keys`（core 不碰 gpui），
// 这里只做「GPUI 按键 → Chord」的翻译——按住 Shift 时 GPUI 给大写键名，
// 归一化漏一处，整条 ⌘⇧A 就静默失效。
mod keys;
mod lines;
/// **授权**（v1.81.0 · 模块六）：激活纸的 UI 层。纯逻辑（校验位 /
/// 归一 / 凭据）在 `jiandu_core::license`。
mod license;
mod margin;
/// **全角标点的视觉居中**（v1.9.26）：竖排形 `﹁︽︵` 在字库里贴右缘设计，
/// 按前进宽度居中的结果就是「肉眼偏右」。这里量真实墨迹框算横移量，
/// 只改落笔 translate，绝不碰网格（见 `mark_align` 头注）。
mod mark_align;
/// **内存探针（macOS）**（v1.81.0）：`task_info(TASK_BASIC_INFO_64)` 读本进程
/// 常驻内存，交给 `jiandu_core::mem` 的护栏。core 不许 `#[cfg(target_os)]`，
/// 所以读 RSS 这一层必须留在壳里。
mod mem;
mod note_inject;
// v1.60.0：惰性分页器（按章分块 + 前缀和索引 + LRU 窗口）。
// 渲染层只向它要「当前这一叶」，绝不再每帧排全书。
mod pager;
// v1.80.0：**PDF 印制**。与 share_paint（PNG）同宗的 CoreGraphics 离屏
// 后端，只是画布换成 `CGPDFContext`（矢量 + 内嵌字体）。排版仍由
// `gloss::layout_book_from_opts` 产出，本模块只照着落笔。
#[cfg(target_os = "macos")]
mod pdf;
mod punct;
mod reader;
// v1.9.53：注音回归「汉字正上方横排」，GPUI 原生文字排版即可胜任，
// 原先为竖排/旋转服务的 CoreGraphics 离屏位图（`ruby_glyph`）已整体删除。
mod share;
// v1.57.0：分享卡拆成「版式数据层 + 两个渲染后端」。
//   share_layout  纯数据版式（零 GPUI / 零 macOS API，可单测）
//   share_paint   CoreGraphics 离屏光栅 → 1080×1920 PNG（仅 macOS）
mod share_layout;
#[cfg(target_os = "macos")]
mod share_paint;
mod shelf;
mod sound;
mod storekit;
/// **Dock 复活**（v1.9.35）：macOS 点 Dock 图标回不到主窗口的两个断点在此修。
/// 见模块注释——根因是 gpui 把 `applicationShouldHandleReopen:` 注册成了返回
/// `()` 而 AppKit 要 `BOOL`，返回值是残留垃圾，系统时而判定「不要恢复窗口」。
mod revive;
/// **解锁纸**（v1.9.23）：商店版内购的前台。官网版构建里这个模块仍然编进来
/// （「更多」里不挂入口），免得 `#[cfg]` 把整条链路切成两半、改一处忘一处。
mod unlock;
mod tts;
mod zen_kit;

// 纯逻辑核心（v1.47.0 解耦）：`grid` / `parsers` / `dict` 等 17 个模块已
// 独立为 `jiandu-core` crate（零 GPUI 依赖，见 core/）。这里把它们再导出
// 回 crate 根——`crate::grid` / `crate::types` 等既有路径全部继续有效，
// UI 层（app / reader / shelf）代码零改动。
pub use jiandu_core::{
    banxin, content, dict, gate, gloss, grid, hit, library, numcn, parsers, pipeline, prefs,
    ruby, store, stream, trad, theme, types, vclean, west,
};

use app::Root;
use anyhow::Result;
use gpui::{
    prelude::*, px, size, App, Application, AssetSource, Bounds, KeyBinding, Menu, MenuItem,
    Pixels, SharedString, TitlebarOptions, WindowBounds, WindowOptions,
};
use jiandu_core::library::Library;
use std::borrow::Cow;
use std::path::PathBuf;

// **应用级动作**（v1.9.22）：原生菜单栏里的那两条。
//
// 为什么要 Action 而不是直接调函数：GPUI 0.2.2 的菜单项必须携带一枚
// `Action`，点了之后由 `cx.on_action` 派发；没有 action 的菜单项在 AppKit
// 里是灰的（点了没反应）。命名空间 `jiandu` 只用于调试打印。
//
// ⚠️ 只能用 `//` 不能写 `///`：`actions!` 展开成一组 item，挂在宏上的
// 文档注释会被丢掉（`unused_doc_comments`）。
gpui::actions!(jiandu, [ShowAbout, QuitApp]);

/// 应用资源源：从书架主目录（`~/.jiandu`，可用 `JIANDU_HOME` 覆盖）加载资源，
/// 目前用于目录浮层的「鱼尾纹」SVG。注册后 `svg().path(...)` 才能读到文件。
struct JianDuAssets {
    base: PathBuf,
}

impl AssetSource for JianDuAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let full = self.base.join(path);
        match std::fs::read(&full) {
            Ok(data) => Ok(Some(Cow::Owned(data))),
            Err(_) => Ok(None),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let full = self.base.join(path);
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&full) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(SharedString::from(name.to_string()));
                }
            }
        }
        Ok(out)
    }
}

fn main() {
    // 全局 panic 钩子：把 panic 位置 + 回溯落盘，便于定位崩溃根因。
    // 背景：笺读的事件处理器经 GPUI 从 AppKit(ObjC) 事件循环回调进 Rust，
    // 一旦处理器 panic，Rust 展开无法越过这条 FFI 边界，运行时会直接
    // `panic_cannot_unwind` 强杀进程（abort 不打印、不展开），表现为
    // 「点一下就整个退出、什么都没留下」却看不到任何报错。此钩子在 abort
    // 之前把确切位置与回溯写进数据目录的 `logs/crash-<时间>.txt`（一次一枚，
    // 只留最近 10 枚），同时前台也打印一份。
    install_panic_logger();

    // 内存护栏的探针（v1.81.0）：越早装越好——批量导入与 FTS5 建索引都在
    // 后台线程跑，装晚了它们就有一段「裸奔」的路。
    mem::install();

    // 繁简滤镜的引擎（v1.81.0）：同上，越早越好——首次渲染就要用。
    conv::install();

    // 命令行模式：`jiandu --convert 输入 --out 输出`
    //   epub -> json / txt / md 的批量转换，不开窗口。
    let args: Vec<String> = std::env::args().collect();

    // 真机 TTS 验证（v1.9.9 紧急热修用，不影响正常 GUI）：`jiandu --tts-probe "文字"`
    // 从 main()（**主线程**）直接调用 crate::tts::speak，打印是否成功、是否闪退，然后退出。
    // 用于确认 AVFoundation FFI 在主线程能安全发声、不再 EXC_BAD_ACCESS。
    if let Some(pos) = args.iter().position(|a| a == "--tts-probe") {
        let text = args
            .get(pos + 1)
            .cloned()
            .unwrap_or_else(|| "床前明月光，疑是地上霜。举头望明月，低头思故乡。".to_string());
        eprintln!("[tts-probe] running on main thread (main())");
        // 探针打一个非 0 的号：回调把这个号带回来，才证明「读完自动续读」
        // 这条信号链（delegate → 队列 → 主线程取空）真的通了，而不只是能出声。
        let tag: u64 = 42;
        // `--tts-probe-stop`：念到一半按「停」，再等 4 秒**断言没有** didFinish。
        // 这是续读最危险的失效模式：停了还自己往下翻，用户会以为书被鬼翻。
        let stop_early = args.iter().any(|a| a == "--tts-probe-stop");
        let ok = crate::tts::speak_tagged(&text, 1.0, tag);
        eprintln!("[tts-probe] speak_tagged returned {ok} (tag={tag})");
        let t0 = std::time::Instant::now();
        let mut got: Option<u64> = None;
        if stop_early {
            // 真的等够 1 秒（`runMode:beforeDate:` 没事件时立刻返回，是空转，
            // 不能拿它计时），并确认引擎**确实在出声**——否则「停了没回调」
            // 是因为压根没开始念，这个测试就等于没测。
            while t0.elapsed().as_secs_f64() < 1.0 {
                crate::tts::pump_once(0.1);
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
            let speaking = crate::tts::is_speaking();
            eprintln!(
                "[tts-probe] stop early at {:.1}s, isSpeaking={speaking}",
                t0.elapsed().as_secs_f64()
            );
            if !speaking {
                eprintln!("[tts-probe] 没在念，这次「停」不算数（换更长的文本重试）");
                std::process::exit(6);
            }
            let stopped = crate::tts::stop();
            eprintln!("[tts-probe] stop() returned {stopped}");
            let t1 = std::time::Instant::now();
            while t1.elapsed().as_secs_f64() < 4.0 {
                crate::tts::pump_once(0.2);
                if let Some(t) = crate::tts::take_finished() {
                    got = Some(t);
                    break;
                }
            }
            match got {
                None => {
                    eprintln!("[tts-probe] 停之后 4s 内无迟到回调 —— 正确，不会自动续读");
                    std::process::exit(0);
                }
                Some(t) => {
                    eprintln!("[tts-probe] 停之后仍收到 didFinish tag={t} —— 会误续读！");
                    std::process::exit(5);
                }
            }
        }
        // 必须**转 runloop** 等回调：delegate 由主队列投递，sleep 等不到。
        // 顺手把逐字进度也收下来——能收到，才证明「卡拉 OK 高亮」有信号源；
        // 再把换算出的子串打出来，子串能读通，才证明 UTF-16 → char 没算错。
        let u16map = crate::app::utf16_char_map(&text);
        let mut pos: Vec<(u64, usize, usize)> = Vec::new();
        while t0.elapsed().as_secs_f64() < 30.0 {
            crate::tts::pump_once(0.2);
            while let Some(p) = crate::tts::take_speak_pos() {
                pos.push(p);
            }
            if let Some(t) = crate::tts::take_finished() {
                got = Some(t);
                break;
            }
        }
        eprintln!("[tts-probe] 逐字进度回调 {} 次", pos.len());
        let (f, r) = crate::tts::added_methods();
        eprintln!("[tts-probe] delegate 方法: didFinish={f} willSpeakRange={r}");
        for (i, (tag, loc, len)) in pos.iter().take(6).enumerate() {
            let s = crate::app::char_range_from_utf16(&u16map, *loc, *len);
            let slice: String = match s {
                Some((a, b)) => text.chars().skip(a).take(b - a).collect(),
                None => "<越界，不高亮>".to_string(),
            };
            eprintln!("[tts-probe]   #{i} tag={tag} loc={loc} len={len} → 「{slice}」");
        }
        match got {
            Some(t) if t == tag => eprintln!(
                "[tts-probe] didFinish tag={t} in {:.1}s —— 续读信号打通",
                t0.elapsed().as_secs_f64()
            ),
            Some(t) => eprintln!("[tts-probe] didFinish tag={t} 与期望 {tag} 不符（串号！）"),
            None => eprintln!("[tts-probe] 30s 内未收到 didFinish —— delegate 没接上"),
        }
        let stopped = crate::tts::stop();
        eprintln!("[tts-probe] stop() returned {stopped}; survived, exiting");
        std::process::exit(match got {
            Some(t) if t == tag => 0,
            Some(_) => 3,
            None => 4,
        });
    }

    // `jiandu --zen`：直接打开原生 GPUI 竖笺演示窗口（天头地脚非对称 + Canvas 网格
    // + 三 Tab 设置 + 右侧热区 spring 抽屉），不开书架，便于单独验证排版与交互。
    // 演示模式下跳过 CLI 分支，直接进开窗流程。
    let zen_demo = args.iter().any(|a| a == "--zen");
    if !zen_demo && args.len() > 1 {
        if let Some(code) = run_cli(&args[1..]) {
            std::process::exit(code);
        }
    }

    let app = Application::new().with_assets(JianDuAssets {
        base: Library::home(),
    });

    // ── Dock 复活 · 重建线（v1.9.35）──────────────────────────────────
    //
    // 窗口被红叉关掉后进程仍在跑（gpui 不会自己退出），此时点 Dock 图标走的是
    // `applicationShouldHandleReopen:hasVisibleWindows:` 的 **无可见窗口** 分支
    // → gpui 去调它的 `reopen` 回调。**没人注册那个回调就什么都不会发生**，
    // 用户看到的就是「双击毫无反应」。这里把主窗口重新建出来。
    //
    // 另一半（窗口还在、只是被最小化或压在别的 app 后面）走 `revive::install()`
    // 的通知线，两条线互补，根因见 `revive.rs` 模块注释。
    app.on_reopen(|cx: &mut App| {
        if cx.windows().is_empty() {
            let root = cx.new(Root::new);
            let bounds = default_window_bounds(cx);
            if cx
                .open_window(zen_window_options(bounds), move |_window, _cx| root.clone())
                .is_ok()
            {
                cx.activate(true);
            }
        } else {
            cx.activate(true);
        }
    });

    app.run(move |cx: &mut App| {
        // 注册随包字体（存在才生效），找不到则沿回退链使用系统字体。
        fonts::register_bundled_fonts(cx);

        // **macOS 顶部状态栏常驻**（v1.9.44）：在系统状态栏注册极简「笺」图标
        // + 毛玻璃下拉菜单（今日阅卷 / 快速笺校 / 书架 / 最近打开 / 检查更新 /
        // 退出）。必须在主线程调——NSStatusBar 不是线程安全的；非 macOS 是空实现。
        crate::menubar::install();

        // **内购链路开机**（v1.9.23）：必须在建窗之前调——`SKPaymentQueue`
        // 的观察者越早挂上，越能接住上次买到一半被杀掉的那笔交易（StoreKit
        // 会重投未 finish 的交易）。官网页构建里这一步是空实现。
        storekit::prepare();

        // 窗口尺寸：`JIANDU_WIN=宽x高`（开发用，便于截图核对横屏对开 / 竖屏单页）。
        let bounds = default_window_bounds(cx);
        // `--zen`：根视图换成原生 GPUI 竖笺演示（zen_kit::ZenView）——天头地脚非对称 +
        // Canvas 网格 + 三 Tab 设置 + 右侧热区 spring 抽屉；否则照旧开书架 Root。
        // （WindowOptions 不可 Clone，用工厂函数给两个分支各造一份。）
        if zen_demo {
            // ZenView::new 已返回 Entity<ZenView>（Entity<V: Render> 本身即 Render），
            // 直接交给 open_window，不要再套一层 cx.new（否则变成 Entity<Entity<..>>）。
            cx.open_window(zen_window_options(bounds), |_window, cx| {
                zen_kit::ZenView::new(cx)
            })
            .expect("failed to open window");
        } else {
            // 先把 Root 建出来（而不是在 open_window 的闭包里建）：菜单里的
            // 「关于」要拿到这个实体才能把卡片摊开，句柄必须活到 App 那层。
            let root = cx.new(Root::new);

            // ── 原生菜单栏（v1.9.22）────────────────────────────────
            //
            // 只放两条：**关于笺读** 与 **退出**。
            // ・前者是用户点名的入口（左侧栏底下还有一枚小印，两条路同一张纸）；
            // ・后者顺手把 **⌘Q** 接回来——GPUI 从 `set_menus` 之外不给菜单栏，
            //   没有菜单项就没有 responder，⌘Q 一直是哑的（老版本里的暗坑）。
            //
            // ⚠️ `set_menus` 会**整条替换** NSApp.mainMenu（platform.rs:921
            //    `app.setMainMenu_`），所以这里给出的就是全部——别指望系统
            //    再帮你补「编辑」菜单。
            let about_root = root.clone();
            cx.on_action(move |_: &ShowAbout, cx: &mut App| {
                about_root.update(cx, |this, cx| this.open_about(cx));
            });
            cx.on_action(|_: &QuitApp, cx: &mut App| cx.shutdown());
            cx.bind_keys([KeyBinding::new("cmd-q", QuitApp, None)]);
            cx.set_menus(vec![Menu {
                name: "笺读".into(),
                items: vec![
                    MenuItem::Action {
                        name: "关于笺读".into(),
                        action: Box::new(ShowAbout),
                        os_action: None,
                    },
                    MenuItem::separator(),
                    MenuItem::Action {
                        name: "退出笺读".into(),
                        action: Box::new(QuitApp),
                        os_action: None,
                    },
                ],
            }]);

            cx.open_window(zen_window_options(bounds), move |_window, _cx| root.clone())
                .expect("failed to open window");
        }

        // ── Dock 复活（v1.9.35）────────────────────────────────────────
        // 通知线：app 只要被激活就把主窗口前置，与 delegate 返回什么无关。
        // （重建线注册在 `Application::on_reopen` 上，见 `run` 之前那一处——
        //   那个方法是 `Application` 的，不在 `App` 上。）
        revive::install();
        cx.activate(true);
    });
}

/// 主窗口的初始位置与尺寸。
///
/// 抽成函数是为了让「启动开窗」与「关窗后点 Dock 重建」（`on_reopen`）用**同一
/// 套**几何：两条路各写一遍的话，重建出来的窗口迟早和首次启动的尺寸不一样。
fn default_window_bounds(cx: &mut App) -> Bounds<Pixels> {
    let (mut win_w, mut win_h) = (1320.0_f32, 880.0_f32);
    // `JIANDU_WIN=宽x高`：开发用，便于截图核对横屏对开 / 竖屏单页。
    if let Ok(v) = std::env::var("JIANDU_WIN") {
        if let Some((w, h)) = v.split_once('x') {
            if let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>()) {
                win_w = w.clamp(320.0, 3840.0);
                win_h = h.clamp(240.0, 2160.0);
            }
        }
    }
    // 屏幕容得下才是好窗口：默认 1320×880 在 13 寸（逻辑宽 1280 上下）上会被
    // 挤出屏幕，标题栏和左侧内容都被裁掉。这里按主屏可用区收一档，
    // 并给四周留出边距（窗口不会顶到菜单栏 / Dock）。
    if let Some(display) = cx.displays().first() {
        let ds = display.bounds().size;
        let unit = px(1.0);
        let (dw, dh) = (ds.width / unit, ds.height / unit);
        if dw > 0.0 && dh > 0.0 {
            win_w = win_w.min((dw - 24.0).max(320.0));
            win_h = win_h.min((dh - 90.0).max(240.0));
        }
    }
    Bounds::centered(None, size(px(win_w), px(win_h)), cx)
}

/// 书架 / 竖笺演示共用的窗口参数（标题、尺寸、居中）。
///
/// v1.52.0：**恢复 macOS 原生红绿灯**。
///
/// 上一版（v1.47.0）把 `traffic_light_position` 推到 (-10000, -10000)，
/// 让系统三钮裁到窗口外，改由 app.rs 自绘三枚淡墨线条钮——Mac 用户少了
/// 最熟悉的那三个圆点，全屏 / 最小化也没了系统手感。
///
/// 现在改回 `None`（即 AppKit 的标准位置：标题栏左上角 y≈13），
/// app.rs 那三枚自绘钮同时撤下，避免「两套窗口钮」并存。
fn zen_window_options(bounds: Bounds<gpui::Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("笺读 · JianDu".into()),
            // 不写这一项（None）= 系统标准位置；要微调才给坐标。
            traffic_light_position: None,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// 安装全局 panic 钩子：在进程被 `panic_cannot_unwind` 强杀（abort）之前，
/// 把 panic 的发生时间、位置、信息与完整回溯写进 `~/.jiandu/panic.log`，
/// 同时向前台（stderr）也打印一份，确保无终端的 GUI 启动也能拿到证据。
fn install_panic_logger() {
    let home = Library::home();
    let dir = home.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    std::panic::set_hook(Box::new(move |info| {
        let (file_stamp, human_stamp) = local_stamps();
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "未知位置".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "非字符串 panic 负载".to_string()
        };
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("未命名线程");
        let bt = std::backtrace::Backtrace::force_capture();
        let entry = format!(
            "笺读 {ver} · 崩溃报告\n\
             ========================================\n\
             时间：{stamp}\n\
             版本：{ver}（{profile} 构建）\n\
             系统：{os} / {arch}\n\
             线程：{thread}\n\
             位置：{loc}\n\
             信息：{msg}\n\
             ========================================\n\
             回溯：\n{bt}\n",
            ver = env!("CARGO_PKG_VERSION"),
            profile = if cfg!(debug_assertions) { "debug" } else { "release" },
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            stamp = human_stamp,
        );
        // **一枚崩溃一份文件**（v1.9.75）：旧实现固定写 `panic.log`，`fs::write`
        // 整份覆盖——用户连崩两次，第一次的证据就没了；而「导入奇葩古籍闪退」
        // 往往正是连着崩的那几次。落盘失败也不影响前台打印。
        let path = dir.join(format!("crash-{file_stamp}.txt"));
        let _ = std::fs::write(&path, &entry);
        // 只留最近 10 枚：崩溃报告是**证据**不是日志，攒多了没人看还会占空间。
        prune_crash_logs(&dir, 10);
        eprintln!("{entry}");
        eprintln!("[jiandu] 崩溃报告：{}", path.display());
    }));
}

/// (文件名时间戳 `YYYYMMDD-HHMMSS`, 人读时间 `YYYY-MM-DD HH:MM:SS`)，均取**本地**时区。
///
/// 不用 UTC：用户报「昨晚十点崩了」时，UTC 会差 8 小时，对不上号。
fn local_stamps() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::localtime_r(&secs, &mut tm) };
    if ok.is_null() {
        return (format!("{secs}"), format!("{secs} (unix)"));
    }
    let (y, mo, d) = (tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
    let (h, mi, s) = (tm.tm_hour, tm.tm_min, tm.tm_sec);
    (
        format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}"),
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"),
    )
}

/// 崩溃报告只留最近 `keep` 枚（按文件名里的时间戳排序，天然有序）。
fn prune_crash_logs(dir: &std::path::Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        .collect();
    if names.len() <= keep {
        return;
    }
    names.sort();
    for old in &names[..names.len() - keep] {
        let _ = std::fs::remove_file(dir.join(old));
    }
}

/// 处理命令行；返回 `Some(退出码)` 表示是 CLI 任务，不必开窗口。
fn run_cli(args: &[String]) -> Option<i32> {
    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut format: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--convert" | "-c" => {
                input = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" | "-o" => {
                out = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--format" | "-f" => {
                format = args.get(i + 1).cloned();
                i += 2;
            }
            "--help" | "-h" => {
                print_cli_help();
                return Some(0);
            }
            other if !other.starts_with('-') => {
                input = Some(PathBuf::from(other));
                i += 1;
            }
            other => {
                eprintln!("未知参数：{other}");
                print_cli_help();
                return Some(2);
            }
        }
    }

    let Some(input) = input else {
        print_cli_help();
        return Some(0);
    };

    // 没给输出路径就按 --format 或默认 txt 生成到同目录。
    let out = out.unwrap_or_else(|| {
        let ext = format.clone().unwrap_or_else(|| "txt".to_string());
        input.with_extension(ext)
    });

    match exporter::convert(&input, &out) {
        Ok(p) => {
            println!("已导出：{}", p.display());
            Some(0)
        }
        Err(e) => {
            eprintln!("转换失败：{e}");
            Some(1)
        }
    }
}

fn print_cli_help() {
    println!(
        "笺读 JianDu\n\
         \n\
         直接运行（不带参数）打开书架窗口：\n\
         \n\
           jiandu\n\
         \n\
         原生 GPUI 竖笺演示（天头地脚非对称 + Canvas 古籍网格 + 三 Tab 设置\n\
         + 右侧热区 spring 抽屉，不开书架）：\n\
         \n\
           jiandu --zen\n\
         \n\
         格式转换（epub -> json / txt / md）：\n\
         \n\
           jiandu --convert 输入.epub --out 输出.json\n\
           jiandu -c 输入.epub -f txt          # 输出到同目录同名 .txt\n\
         \n\
         说明：\n\
           json 保留封面、目录、注释、通假表，回笺读读最完整；\n\
           txt  顶部有目录清单，正文按章分节，注释以〔注〕附在段后；\n\
           md   目录用列表、章节用 ## 标题，方便再编辑或被其他工具读取。"
    );
}
