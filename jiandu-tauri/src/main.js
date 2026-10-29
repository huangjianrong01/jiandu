const { invoke } = window.__TAURI__.core;

let greetInputEl;
let greetMsgEl;

async function greet() {
  // Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
  greetMsgEl.textContent = await invoke("greet", { name: greetInputEl.value });
}

window.addEventListener("DOMContentLoaded", () => {
  greetInputEl = document.querySelector("#greet-input");
  greetMsgEl = document.querySelector("#greet-msg");
  document.querySelector("#greet-form").addEventListener("submit", (e) => {
    e.preventDefault();
    greet();
  });

  // 测试 core 大脑：点击按钮 → 调用 Rust 的 test_core 命令 → alert 结果
  const testCoreBtn = document.querySelector("#test-core-btn");
  const coreResultEl = document.querySelector("#core-result");
  if (testCoreBtn) {
    testCoreBtn.addEventListener("click", async () => {
      try {
        const res = await invoke("test_core");
        alert(res);
        if (coreResultEl) coreResultEl.textContent = res;
      } catch (e) {
        alert("调用 core 失败：" + e);
      }
    });
  }
});
