// 笺读 Tauri 前端：全格式导入 + 渲染
const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke;
const dialog = tauri?.dialog;

const importBtn = document.querySelector("#import-btn");
const testCoreBtn = document.querySelector("#test-core-btn");
const welcome = document.querySelector("#welcome");
const bookView = document.querySelector("#book-view");
const fileNameEl = document.querySelector("#file-name");

// 导入书籍：选文件 → invoke import_and_parse → 渲染 Book
if (importBtn) {
  importBtn.addEventListener("click", async () => {
    try {
      const path = await dialog.open({
        multiple: false,
        filters: [
          { name: "书籍", extensions: ["epub", "txt", "docx", "json", "md"] },
        ],
      });
      if (!path) return; // 用户取消
      fileNameEl.textContent = path;
      const book = await invoke("import_and_parse", { path });
      renderBook(book);
    } catch (e) {
      alert("导入失败：" + e);
    }
  });
}

// 把 Book 结构（title / author / chapters[].paragraphs[].text）塞进宣纸 div
function renderBook(book) {
  welcome.hidden = true;
  bookView.hidden = false;
  bookView.innerHTML = "";

  const h1 = document.createElement("h1");
  h1.className = "book-title";
  h1.textContent =
    (book.title || "无题") + (book.author ? " · " + book.author : "");
  bookView.appendChild(h1);

  const chapters = book.chapters || [];
  for (const ch of chapters) {
    const section = document.createElement("section");
    const h2 = document.createElement("h2");
    h2.textContent = ch.title || "（无题）";
    section.appendChild(h2);

    const paras = ch.paragraphs || [];
    for (const p of paras) {
      const para = document.createElement("p");
      para.textContent = p.text || "";
      section.appendChild(para);
    }
    bookView.appendChild(section);
  }
}

// 测试 core 大脑：点击 → 调用 Rust 的 test_core 命令 → alert 跨平台数据目录
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
