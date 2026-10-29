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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![greet, test_core, import_and_parse])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
