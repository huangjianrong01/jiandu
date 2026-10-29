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

// 朗读状态机：'idle' | 'playing' | 'paused'
let ttsState = "idle";
let ttsItems = []; // [{ node, text }] —— 当前书依次朗读的片段
let ttsIdx = 0;
let zhVoice = null;

// 选一个中文语音（macOS/Windows WebView 都内置 zh-CN 语音）
function loadZhVoice() {
  const vs = window.speechSynthesis.getVoices();
  zhVoice =
    vs.find((v) => /^(zh|cmn|zh-CN|zh_CN)/i.test(v.lang)) ||
    vs.find((v) => /chinese|中文|普通话|国语/i.test(v.name)) ||
    null;
}
if (window.speechSynthesis) {
  window.speechSynthesis.onvoiceschanged = loadZhVoice;
  loadZhVoice();
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
    if (text) return [{ node: verse, text }];
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

function speakNext() {
  if (ttsState !== "playing") return;
  if (ttsIdx >= ttsItems.length) {
    finishTts();
    return;
  }
  const item = ttsItems[ttsIdx];
  const u = new SpeechSynthesisUtterance(item.text);
  u.lang = "zh-CN";
  u.rate = 0.95; // 略慢，更适合听书
  if (zhVoice) u.voice = zhVoice;

  clearReadingHighlight();
  if (item.node) {
    item.node.classList.add("reading");
    item.node.scrollIntoView({ block: "center", behavior: "smooth" });
  }

  u.onend = () => {
    if (ttsState !== "playing") return;
    ttsIdx++;
    speakNext();
  };
  u.onerror = (e) => {
    console.error("TTS error", e);
    if (ttsState !== "playing") return;
    ttsIdx++;
    speakNext();
  };
  window.speechSynthesis.speak(u);
}

function startTts() {
  if (!window.speechSynthesis) {
    alert("当前环境不支持语音合成（speechSynthesis）");
    return;
  }
  window.speechSynthesis.cancel(); // 清掉任何残留队列
  ttsItems = getReadableItems();
  if (!ttsItems.length) {
    alert("没有可朗读的内容");
    return;
  }
  ttsIdx = 0;
  ttsState = "playing";
  setTtsBtn();
  speakNext();
}

function pauseTts() {
  ttsState = "paused";
  window.speechSynthesis.pause();
  setTtsBtn();
}

function resumeTts() {
  ttsState = "playing";
  window.speechSynthesis.resume();
  setTtsBtn();
}

function stopTts() {
  ttsState = "idle";
  if (window.speechSynthesis) window.speechSynthesis.cancel();
  clearReadingHighlight();
  setTtsBtn();
}

function finishTts() {
  ttsState = "idle";
  clearReadingHighlight();
  setTtsBtn();
}

function onTtsClick() {
  if (!window.speechSynthesis) {
    alert("当前环境不支持语音合成（speechSynthesis）");
    return;
  }
  if (ttsState === "idle") startTts();
  else if (ttsState === "playing") pauseTts();
  else if (ttsState === "paused") resumeTts();
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
