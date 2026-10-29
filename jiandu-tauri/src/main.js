// 笺读 Tauri 前端：仅保留「测试 core 大脑」按钮的 IPC 调用
const { invoke } = window.__TAURI__.core;

window.addEventListener("DOMContentLoaded", () => {
  // 测试 core 大脑：点击 → 调用 Rust 的 test_core 命令 → alert 跨平台数据目录
  const testCoreBtn = document.querySelector("#test-core-btn");
  if (testCoreBtn) {
    testCoreBtn.addEventListener("click", async () => {
      try {
        const res = await invoke("test_core");
        alert(res);
      } catch (e) {
        alert("调用 core 失败：" + e);
      }
    });
  }
});
