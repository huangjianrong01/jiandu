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

mod app;
mod exporter;
mod fonts;
mod gloss;
mod hit;
mod ime;
mod lines;
mod note_inject;
mod punct;
mod reader;
mod shelf;
mod sound;
mod zen_kit;

// 纯逻辑核心（v1.47.0 解耦）：`grid` / `parsers` / `dict` 等 17 个模块已
// 独立为 `jiandu-core` crate（零 GPUI 依赖，见 core/）。这里把它们再导出
// 回 crate 根——`crate::grid` / `crate::types` 等既有路径全部继续有效，
// UI 层（app / reader / shelf）代码零改动。
pub use jiandu_core::{
    banxin, content, dict, grid, importer, library, numcn, parsers, pdf, pipeline, prefs, ruby,
    store, stream, trad, theme, types, vclean, west,
};

use app::Root;
use anyhow::Result;
use gpui::{
    point, prelude::*, px, size, App, Application, AssetSource, Bounds, SharedString,
    TitlebarOptions, WindowBounds, WindowOptions,
};
use jiandu_core::library::Library;
use std::borrow::Cow;
use std::path::PathBuf;

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
    // 命令行模式：`jiandu --convert 输入 --out 输出`
    //   epub -> json / txt / md 的批量转换，不开窗口。
    let args: Vec<String> = std::env::args().collect();
    // `jiandu --zen`：直接打开原生 GPUI 竖笺演示窗口（天头地脚非对称 + Canvas 网格
    // + 三 Tab 设置 + 右侧热区 spring 抽屉），不开书架，便于单独验证排版与交互。
    // 演示模式下跳过 CLI 分支，直接进开窗流程。
    let zen_demo = args.iter().any(|a| a == "--zen");
    if !zen_demo && args.len() > 1 {
        if let Some(code) = run_cli(&args[1..]) {
            std::process::exit(code);
        }
    }

    Application::new()
        .with_assets(JianDuAssets {
            base: Library::home(),
        })
        .run(move |cx: &mut App| {
        // 注册随包字体（存在才生效），找不到则沿回退链使用系统字体。
        fonts::register_bundled_fonts(cx);

        // 窗口尺寸：JIANDU_WIN=宽x高（开发用，便于截图核对横屏对开 / 竖屏单页）。
        let (mut win_w, mut win_h) = (1320.0_f32, 880.0_f32);
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
        let bounds = Bounds::centered(None, size(px(win_w), px(win_h)), cx);
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
            cx.open_window(zen_window_options(bounds), |_window, cx| {
                cx.new(|cx| Root::new(cx))
            })
            .expect("failed to open window");
        }

        cx.activate(true);
    });
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
