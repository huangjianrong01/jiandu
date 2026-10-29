// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// 测试命令：验证 core 大脑已接入 Tauri 壳。
/// 直接调用 jiandu_core::library::Library::home()，返回 core 计算的
/// 跨平台应用数据目录（Windows/macOS/Linux 三态通用）。
#[tauri::command]
fn test_core() -> String {
    let home = jiandu_core::library::Library::home();
    format!("core 大脑已接入！Library::home() = {}", home.display())
}

/// 全格式书籍导入与渲染：接收前端传来的文件路径，按后缀交给 core 的
/// `import_from_path` 解析（txt / md / json / epub / docx 全支持，
/// 并复用 Mac 端统一的清洗 + JSON 适配器管线）。解析出的完整 `Book`
/// 结构直接序列化交前端渲染，无需在 Rust 侧拼装 DTO。
///
/// 用 `spawn_blocking` 把同步解析放到专用线程，避免大文件解析阻塞 async
/// 运行时导致 UI 假死（异步调度，界面不卡）。文件由 Rust 直接用 std::fs
/// 读取，不依赖 Tauri fs 插件，无需额外 fs 权限。
#[tauri::command]
async fn import_and_parse(path: String) -> Result<jiandu_core::types::Book, String> {
    let p = path.clone();
    let book = tauri::async_runtime::spawn_blocking(move || {
        jiandu_core::importer::import_from_path(std::path::Path::new(&p))
    })
    .await
    .map_err(|e| format!("导入线程崩溃：{e}"))?
    .map_err(|e| e.to_string())?;
    Ok(book)
}

/// 系统支持性检查（用于老旧系统友好提示）。
///
/// - 非 Windows 平台（macOS / Linux 开发环境）直接放行。
/// - Windows：读取注册表 `CurrentBuildNumber`，按下列规则判定：
///     * Win11            build >= 22000   → 支持
///     * Win10 (1803+)    build >= 17134   → 支持
///     * 其它（含 Win7/8）→ 不支持，返回友好提示引导「云端代排服务」
/// 这样在 App 首屏即可拦截不支持的系统，而不是让 WebView2 崩溃。
#[tauri::command]
fn os_support() -> serde_json::Value {
    use serde_json::json;

    let os = std::env::consts::OS;
    if os != "windows" {
        return json!({
            "supported": true,
            "platform": os,
            "version": "",
            "build": 0,
            "message": ""
        });
    }

    // 读取 Windows 构建号（注册表），无需引入额外 Rust 依赖。
    let build: u32 = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion').CurrentBuildNumber",
        ])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let (version, supported, message) = if build >= 22000 {
        ("Windows 11".to_string(), true, String::new())
    } else if build >= 17134 {
        // 17134 = Windows 10 1803（April 2018 Update）
        ("Windows 10".to_string(), true, String::new())
    } else {
        (
            format!("Windows (build {build})"),
            false,
            "当前 Windows 版本过低，无法运行笺读桌面版。\n\
             请升级到 Windows 10（1803 及以上）或 Windows 11；\n\
             或直接使用我们的「云端代排服务」，在浏览器里即可完成古籍排版与导出。"
                .to_string(),
        )
    };

    json!({
        "supported": supported,
        "platform": "windows",
        "version": version,
        "build": build,
        "message": message
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![greet, test_core, import_and_parse, os_support])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
