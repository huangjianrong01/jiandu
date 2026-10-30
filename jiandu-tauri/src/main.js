// 笺读 Tauri 前端：全格式导入 + 渲染
const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke;
const dialog = tauri?.dialog;

const importBtn = document.querySelector("#import-btn");
const testCoreBtn = document.querySelector("#test-core-btn");
const welcome = document.querySelector("#welcome");
const bookView = document.querySelector("#book-view");
const fileNameEl = document.querySelector("#file-name");

// 系统不支持引导页：老旧 Windows（如 Win7）→ 引导「云端代排服务」
const unsupported = document.querySelector("#unsupported");
const unsupportedMsg = document.querySelector("#unsupported-msg");
const cloudBtn = document.querySelector("#cloud-btn");
// TODO: 替换为真实「云端代排服务」地址
const CLOUD_URL = "https://jiandu.example.com/cloud";

if (cloudBtn) {
  cloudBtn.addEventListener("click", () => {
    const opener = tauri && tauri.opener;
    if (opener && opener.openUrl) opener.openUrl(CLOUD_URL);
    else window.open(CLOUD_URL, "_blank");
  });
}

// 首屏系统支持检查：不支持则显示引导页并隐藏阅读区/顶栏
if (invoke) {
  invoke("os_support")
    .then((res) => {
      if (res && res.supported === false) {
        if (unsupportedMsg) unsupportedMsg.textContent = res.message || "当前系统不支持。";
        if (unsupported) unsupported.hidden = false;
        if (welcome) welcome.hidden = true;
        if (bookView) bookView.hidden = true;
        const topbar = document.querySelector(".topbar");
        if (topbar) topbar.style.display = "none";
      }
    })
    .catch(() => {
      /* 检查失败则放行，不阻断正常使用 */
    });
}

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

// 朗读状态机：'idle' | 'playing'
let ttsState = "idle";
let ttsItems = []; // [{ node, text }] —— 当前书依次朗读的片段
let ttsIdx = 0;

// 单句朗读：macOS 走原生 say（经 Rust invoke "speak"），其余环境回退 Web Speech。
// 返回一个在「本句结束（自然或被中止）」时 resolve 的 Promise，便于顺序串联。
// 改用原生 say 的根因：WKWebView 未实现 speechSynthesis，调用即崩溃渲染进程。
function speakOne(text) {
  if (invoke) {
    return invoke("speak", { text, voice: "", rate: 0.95 });
  }
  return new Promise((resolve) => {
    try {
      if (!window.speechSynthesis) { resolve(); return; }
      const u = new SpeechSynthesisUtterance(text);
      u.lang = "zh-CN";
      u.rate = 0.95;
      u.onend = () => resolve();
      u.onerror = () => resolve();
      window.speechSynthesis.speak(u);
    } catch (e) {
      console.error("TTS fallback error", e);
      resolve();
    }
  });
}

function stopOne() {
  if (invoke) {
    invoke("stop_speak").catch(() => {});
  } else if (window.speechSynthesis) {
    window.speechSynthesis.cancel();
  }
}

// 把 Book 结构（title / author / chapters[].paragraphs[].text）塞进宣纸 div
function renderBook(book) {
  welcome.hidden = true;
  bookView.hidden = false;
  bookView.innerHTML = "";
  ttsItems = []; // 重新收集朗读片段

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
    ttsItems.push({ node: h2, text: h2.textContent }); // 章标题也读

    const paras = ch.paragraphs || [];
    for (const p of paras) {
      const para = document.createElement("p");
      para.textContent = p.text || "";
      section.appendChild(para);
      if ((p.text || "").trim()) {
        ttsItems.push({ node: para, text: p.text });
      }
    }
    bookView.appendChild(section);
  }
}

// 未导入时，朗读默认欢迎页《静夜思》
function getReadableItems() {
  if (ttsItems.length) return ttsItems;
  const verse = document.querySelector("#welcome .verse");
  if (verse) {
    const text = (verse.innerText || verse.textContent || "").replace(
      /\s+/g,
      ""
    );
    if (text) {
      // 按句读切分成短句逐句朗读：规避语音引擎对长/含标点文本静默截断的坑
      // （《静夜思》曾"断在低头"，根因就是整首诗作为单条 utterance 被截断）
      const clauses = text.match(/[^。，、；！？]+[。，、；！？]?/g);
      if (clauses && clauses.length > 1) {
        return clauses.map((c) => ({ node: verse, text: c }));
      }
      return [{ node: verse, text }];
    }
  }
  return [];
}

function clearReadingHighlight() {
  document
    .querySelectorAll(".reading")
    .forEach((el) => el.classList.remove("reading"));
}

function setTtsBtn() {
  const b = document.querySelector("#tts-btn");
  if (!b) return;
  b.textContent =
    ttsState === "playing" ? "暂停" : ttsState === "paused" ? "继续" : "朗读";
  b.classList.toggle("active", ttsState !== "idle");
}

// 顺序朗读循环：逐句 await speakOne，每句前后检查状态（被 stop 则退出）。
async function speakLoop() {
  while (ttsIdx < ttsItems.length) {
    if (ttsState !== "playing") break;
    const item = ttsItems[ttsIdx];
    clearReadingHighlight();
    if (item.node) {
      item.node.classList.add("reading");
      item.node.scrollIntoView({ block: "center", behavior: "smooth" });
    }
    try {
      await speakOne(item.text);
    } catch (e) {
      console.error("TTS speak error", e);
    }
    if (ttsState !== "playing") break; // 朗读中被中止
    ttsIdx++;
  }
  clearReadingHighlight();
  ttsState = "idle";
  setTtsBtn();
}

function startTts() {
  ttsItems = getReadableItems();
  if (!ttsItems.length) {
    alert("没有可朗读的内容");
    return;
  }
  ttsIdx = 0;
  ttsState = "playing";
  setTtsBtn();
  speakLoop();
}

function stopTts() {
  ttsState = "idle";
  stopOne();
  clearReadingHighlight();
  setTtsBtn();
}

function onTtsClick() {
  if (ttsState === "idle") startTts();
  else stopTts();
}

const ttsBtn = document.querySelector("#tts-btn");
const ttsStopBtn = document.querySelector("#tts-stop-btn");
if (ttsBtn) ttsBtn.addEventListener("click", onTtsClick);
if (ttsStopBtn) ttsStopBtn.addEventListener("click", stopTts);
setTtsBtn(); // 初始化按钮文案

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
