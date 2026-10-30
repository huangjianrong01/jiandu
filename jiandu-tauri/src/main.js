// 笺读 Tauri 前端：全格式导入 + 渲染
const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke;
const dialog = tauri?.dialog;

const importBtn = document.querySelector("#import-btn");
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

// 导入书籍（**字节优先**，Android / 桌面通用）：
// Android 作用域存储下文件选择器只给 content:// URI，拿不到能直接 fs::read 的路径，
// 所以统一走 <input type="file"> + FileReader 把文件读成 Uint8Array，直接 invoke
// `import_from_bytes` 交给 core 内存解析（不落盘）。桌面端同样适用。
const fileInput = document.querySelector("#file-input");
if (importBtn && fileInput) {
  importBtn.addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", async () => {
    const file = fileInput.files && fileInput.files[0];
    if (!file) return;
    try {
      fileNameEl.textContent = file.name;
      const buf = await file.arrayBuffer();
      const bytes = new Uint8Array(buf);
      const book = await invoke("import_from_bytes", {
        fileName: file.name,
        bytes,
      });
      renderBook(book);
    } catch (e) {
      alert("导入失败：" + String(e));
    } finally {
      fileInput.value = ""; // 允许重复选同一个文件
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

// 转义 HTML 特殊字符（书名/正文可能含 < > &，防 XSS 与排版错乱）
function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  }[c]));
}

// 把标点包裹成 .dian（CSS 伪元素会将其渲染为朱砂红圆点）
function punctuate(html) {
  return html.replace(/([，。、；！？：])/g, '<span class="dian">$1</span>');
}

// 把 Book 结构（title / author / chapters[].paragraphs[].text）塞进宣纸 div
function renderBook(book) {
  welcome.hidden = true;
  bookView.hidden = false;
  bookView.innerHTML = "";
  ttsItems = []; // 重新收集朗读片段

  const h1 = document.createElement("h1");
  h1.className = "book-title";
  h1.innerHTML = punctuate(
    escapeHtml((book.title || "无题") + (book.author ? " · " + book.author : ""))
  );
  bookView.appendChild(h1);

  const chapters = book.chapters || [];
  for (const ch of chapters) {
    const section = document.createElement("section");
    const h2 = document.createElement("h2");
    h2.innerHTML = punctuate(escapeHtml(ch.title || "（无题）"));
    section.appendChild(h2);
    ttsItems.push({ node: h2, text: ch.title || "（无题）" }); // 章标题也读

    const paras = ch.paragraphs || [];
    for (const p of paras) {
      const para = document.createElement("p");
      const raw = p.text || "";
      para.innerHTML = punctuate(escapeHtml(raw));
      section.appendChild(para);
      if (raw.trim()) {
        ttsItems.push({ node: para, text: raw });
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
  // 圆形悬浮钮内用符号表达状态，避免长文字溢出
  b.textContent =
    ttsState === "playing" ? "⏸" : ttsState === "paused" ? "▶" : "♪";
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
