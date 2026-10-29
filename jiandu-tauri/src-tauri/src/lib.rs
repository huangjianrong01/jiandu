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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet, test_core])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
