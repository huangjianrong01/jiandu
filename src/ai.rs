//! **问小笺**（v1.76.0 · V2.0 模块二）：AI 伴读的 UI 层。
//!
//! 形态上是一枚**右滑宣纸栏**（与朱批录 / 卡片匣同款），读法上是「一位坐在
//! 纸边上的古文先生」：框一句原文，问他，他一字一字往外吐答案。
//!
//! ── 为什么它必须是**独立 Entity**，不能做成 `Root` 的一个方法 ──────────
//! 流式输出时，服务端每秒要吐几十段字；每段到货都要 `notify` 一次。若这些
//! 状态住在 `Root` 里，每次 notify 都会带着**整棵阅读树**（分页 / 排版 /
//! 版心 / 句读）重绘一遍——翻页手感会被一个网络请求拖垮。把会话状态与流式
//! 缓冲收进 `AiPanel`，notify 只重绘这一条侧栏，纸上那本书纹丝不动。
//!
//! ── 三条硬规矩 ──────────────────────────────────────────────────
//!   ① **不持引用**：送进网络线程的是 `Ask`（全 `String` 的值），绝不把
//!      `Book` / `Glyph` 的引用带过线程边界；
//!   ② **不落历史**：问过什么关掉就没了（卡片匣才负责「留住」）；
//!   ③ **密钥不出门**：只出现在 `ai.json`（0600）与 `Authorization` 请求头，
//!      界面回显一律走 `AiConfig::key_mask`，日志一律不写。
//!
//! 网络走 **OpenAI 兼容的 `/chat/completions`**（hyper + rustls），SSE 增量
//! 解析交给 `jiandu_core::ai::SseParser`——中文常被切在多字节边界上，那边
//! 按字节攒、按行吐，这里只管把吐出来的字贴到答案尾部。

use crate::ime::InputField;
use gpui::{
    div, prelude::*, px, rgb, rgba, App, AsyncApp, Bounds, Context, Div, Entity,
    EntityInputHandler, FocusHandle, IntoElement, KeyDownEvent, Modifiers, MouseButton,
    ParentElement, Pixels, Point, Render, SharedString, Stateful, Styled, UTF16Selection,
    WeakEntity, Window,
};
use jiandu_core::ai::{self, AiConfig, Ask, StreamEvent, PROVIDERS};
use jiandu_core::theme::Theme;
use std::ops::Range;
use std::time::Duration;

/// 一次提问的字数上限（够写一句追问，不至于把整章贴进来）。
const ASK_MAX: usize = 200;
/// 地址 / 模型 / 密钥三栏的上限。
const FIELD_MAX: usize = 300;
/// 整轮对话的超时（秒）：连不上 / 卡住都要有个尽头，不能让面板永远转圈。
const REQUEST_TIMEOUT: u64 = 120;
/// 会话里最多留多少轮（再多只留最近的，免得面板越聊越重）。
const KEEP_TURNS: usize = 40;

// ── 草稿：与 `ime::InputField` 配套的自绘输入框状态 ──────────────────────

/// 一个输入框的全部状态：文本 + 字符光标 + 输入法组字区间。
///
/// 四个输入（地址 / 模型 / 密钥 / 提问）共用同一套读写逻辑——IME 那套
/// UTF-16 ↔ 字符下标的换算很容易写错，只写一遍最稳。
#[derive(Clone, Default)]
struct Draft {
    text: String,
    cursor: usize,
    marked: Option<Range<usize>>,
}

impl Draft {
    /// 按 UTF-16 区间替换（输入法给的 range 一律按 UTF-16 计位）。
    ///
    /// `range` 为 `None`（macOS 传 NSNotFound）时替换**当前组字区间**，
    /// 没有组字区间才落在光标处——否则第二次组字会把新拼音插在旧拼音后面。
    /// 返回落点（UTF-16 下标）。
    fn replace_utf16(&mut self, range: Option<Range<usize>>, text: &str, max: usize) -> usize {
        let eff = range.or_else(|| self.marked.clone());
        let (a, b) = match eff {
            Some(r) => (
                crate::ime::utf16_to_char(&self.text, r.start),
                crate::ime::utf16_to_char(&self.text, r.end),
            ),
            None => {
                let at = self.cursor.min(self.text.chars().count());
                (at, at)
            }
        };
        let head: String = self.text.chars().take(a).collect();
        let tail: String = self.text.chars().skip(b).collect();
        let room = max.saturating_sub(head.chars().count() + tail.chars().count());
        let ins: String = text.chars().take(room).collect();
        self.text = format!("{head}{ins}{tail}");
        self.cursor = head.chars().count() + ins.chars().count();
        crate::ime::char_to_utf16(&self.text, self.cursor)
    }

    fn move_cursor(&mut self, left: bool) {
        let n = self.text.chars().count();
        self.cursor = if left {
            self.cursor.saturating_sub(1)
        } else {
            (self.cursor + 1).min(n)
        };
    }

    /// 删光标前一个字（组字期间的退格由输入法自己消化，不走这里）。
    fn backspace(&mut self) {
        if self.marked.is_some() {
            self.marked = None;
            return;
        }
        let at = self.cursor.min(self.text.chars().count());
        if at == 0 {
            return;
        }
        let head: String = self.text.chars().take(at - 1).collect();
        let tail: String = self.text.chars().skip(at).collect();
        self.text = format!("{head}{tail}");
        self.cursor = at - 1;
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.marked = None;
    }
}

/// 四个输入框的编号（数组下标）。
const F_URL: usize = 0;
const F_MODEL: usize = 1;
const F_KEY: usize = 2;
const F_ASK: usize = 3;

/// 每栏的字数上限（与上面编号一一对应）。
fn max_of(i: usize) -> usize {
    match i {
        F_ASK => ASK_MAX,
        _ => FIELD_MAX,
    }
}

/// 会话里的一条消息。
#[derive(Clone)]
struct Msg {
    /// true = 用户问的，false = 小笺答的。
    mine: bool,
    text: String,
    /// 这一答还在往外吐字（尾部画一枚朱砂游标）。
    typing: bool,
    /// 这一问带着的引文（用户问的那句原文），仅在 `mine` 为真时有值。
    quote: String,
}

pub struct AiPanel {
    /// 连接配置（落盘见 `jiandu_core::ai`）。
    cfg: AiConfig,
    /// 面板自己的纸墨（由 `Root` 在开栏 / 换主题时同步过来）。
    theme: Theme,
    /// 正文字体族（同上，由 Root 同步）。
    song: SharedString,

    msgs: Vec<Msg>,
    /// 正在流式输出（`typing` 的那条还没收束）。
    streaming: bool,
    /// 请求序号：旧请求的迟到回包不得覆盖新一轮答案。
    seq: u64,

    /// 四个输入框的草稿。
    drafts: [Draft; 4],
    /// 四个输入框的焦点句柄（IME 靠它认「字往哪儿落」）。
    focuses: [FocusHandle; 4],
    /// 当前停在设置页（true）还是会话页（false）。
    settings: bool,
    /// 待问的**引文上下文**（工具条「问笺」带进来的那一段）。
    ctx: Option<Ask>,
    /// 最近一次出错的人话（ Verbindung / 401 / 模型不存在…）。
    error: Option<String>,
}

impl AiPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cfg = AiConfig::load();
        // 配好了就直进会话页；没配（缺地址 / 模型 / 密钥）才停在设置页——
        // 否则用户对着一个空输入框不知道要先干什么。
        let ready = cfg.ready();
        let mut drafts: [Draft; 4] = Default::default();
        drafts[F_URL].text = cfg.base_url.clone();
        drafts[F_URL].cursor = drafts[F_URL].text.chars().count();
        drafts[F_MODEL].text = cfg.model.clone();
        drafts[F_MODEL].cursor = drafts[F_MODEL].text.chars().count();
        drafts[F_KEY].text = cfg.api_key.clone();
        drafts[F_KEY].cursor = drafts[F_KEY].text.chars().count();
        Self {
            cfg,
            theme: Theme::default(),
            song: SharedString::from(""),
            msgs: Vec::new(),
            streaming: false,
            seq: 0,
            drafts,
            focuses: [
                cx.focus_handle(),
                cx.focus_handle(),
                cx.focus_handle(),
                cx.focus_handle(),
            ],
            // 没配过的小笺先开设置页（不然用户对着空输入框不知道要干什么）；
            // 配好了就直接进会话页。
            settings: !ready,
            ctx: None,
            error: None,
        }
    }

    // ── Root 侧的接线口 ────────────────────────────────────────────

    /// 同步纸墨与字体（开栏 / 换主题时由 Root 调）。
    pub fn sync_look(&mut self, theme: Theme, song: SharedString) {
        self.theme = theme;
        self.song = song;
    }

    /// 提问框的焦点句柄（开栏后 Root 用它把焦点递进来，否则打不进中文）。
    pub fn ask_focus(&self) -> FocusHandle {
        self.focuses[F_ASK].clone()
    }

    /// 当前设置页是否开着（Root 决定是否给焦点）。
    pub fn on_settings(&self) -> bool {
        self.settings
    }

    /// **带一句引文来问**：工具条「问笺」走这条。
    ///
    /// 引文只进不出：它作为这一轮提问的上下文随请求发出，不落库、不进存档。
    /// 已在流式输出就只换引文、不打断上一问（网络任务有自己的序号）。
    pub fn take_context(&mut self, ask: Ask, cx: &mut Context<Self>) {
        self.ctx = Some(ask);
        self.settings = false;
        cx.notify();
    }

    /// 直接发问（工具条「问笺」点下去就用默认问法问引文）。
    pub fn ask_quote(&mut self, cx: &mut Context<Self>) {
        self.drafts[F_ASK].clear();
        self.settings = false;
        self.send(cx);
    }

    /// 清空这一席对话（「清谈」钮）。
    pub fn clear_talk(&mut self, cx: &mut Context<Self>) {
        self.msgs.clear();
        self.seq = self.seq.wrapping_add(1); // 让在途的旧回包作废
        self.streaming = false;
        self.error = None;
        cx.notify();
    }

    // ── 发问 ──────────────────────────────────────────────────────

    /// 把「引文 + 提问框里的字」打包成一轮请求发出去。
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.streaming {
            self.error = Some("小笺正在作答，稍候再问".to_string());
            cx.notify();
            return;
        }
        if !self.cfg.ready() {
            self.error = Some("先在设置里填好接口与密钥".to_string());
            self.settings = true;
            cx.notify();
            return;
        }
        let q = self.drafts[F_ASK].text.trim().to_string();
        let mut ask = self.ctx.clone().unwrap_or_default();
        ask.question = q.clone();
        if ask.is_empty() {
            self.error = Some("先框选原文，或写一句想问的".to_string());
            cx.notify();
            return;
        }
        // 面板上先落一问一答的壳子：答的那条留空，等着被流式填满。
        let shown_q = if q.is_empty() {
            ask.quote.clone()
        } else {
            q.clone()
        };
        if !shown_q.trim().is_empty() {
            self.msgs.push(Msg {
                mine: true,
                text: shown_q,
                typing: false,
                quote: ask.quote.clone(),
            });
        }
        self.msgs.push(Msg {
            mine: false,
            text: String::new(),
            typing: true,
            quote: String::new(),
        });
        while self.msgs.len() > KEEP_TURNS * 2 {
            self.msgs.remove(0);
        }
        self.drafts[F_ASK].clear();
        self.error = None;
        self.streaming = true;
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;

        // ── 提示词在这里成形（core 负责组装，这里只给材料）───────────
        let body_user = ai::user_prompt(&ask, self.cfg.ctx_chars);
        let system = self.cfg.system_prompt().to_string();
        let cfg = self.cfg.clone();
        let stream = self.cfg.stream;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        spawn_chat(cfg, system, body_user, stream, tx);

        // 收流：一段一段贴进最后那条答案的尾部。**只 notify 本面板**——
        // 这正是把它做成独立 Entity 的全部意义（见文件头）。
        let _task = cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // AsyncApp 是 Clone：换成 owned 句柄才能安全跨 await（见 app.rs 同款）。
            let mut cx = cx.clone();
            async move {
                while let Some(ev) = rx.recv().await {
                    let _ = this.update(&mut cx, |panel, cx| {
                        if panel.seq != seq {
                            return; // 迟到回包：这轮已经被「清谈」或新问作废
                        }
                        panel.apply(ev);
                        cx.notify();
                    });
                }
            }
        });
        cx.notify();
    }

    /// 把一个流事件贴到当前答案上。
    fn apply(&mut self, ev: StreamEvent) {
        match ev {
            StreamEvent::Delta(s) => {
                if let Some(last) = self.msgs.last_mut() {
                    if !last.mine {
                        last.text.push_str(&s);
                    }
                }
            }
            StreamEvent::Done => {
                self.streaming = false;
                if let Some(last) = self.msgs.last_mut() {
                    last.typing = false;
                    last.text = ai::tidy_answer(&last.text);
                }
            }
            StreamEvent::Error(e) => {
                self.streaming = false;
                self.error = Some(e.clone());
                // 答不出来的那条壳子不能留着空在那儿——换成一句人话。
                if let Some(last) = self.msgs.last_mut() {
                    if !last.mine && last.text.trim().is_empty() {
                        last.text = format!("（未答：{e}）");
                        last.typing = false;
                    }
                }
            }
        }
    }

    // ── 设置页 ────────────────────────────────────────────────────

    /// 选服务商：把预设的地址与首个模型填进两栏（**不覆写手填过的密钥**）。
    fn pick_provider(&mut self, i: usize, cx: &mut Context<Self>) {
        let p = ai::provider_at(i);
        self.cfg.provider = i;
        self.cfg.provider_id = p.id.to_string();
        if !p.base_url.is_empty() {
            self.cfg.base_url = p.base_url.to_string();
            self.drafts[F_URL].text = p.base_url.to_string();
            self.drafts[F_URL].cursor = self.drafts[F_URL].text.chars().count();
        }
        if let Some(m) = p.models.first() {
            self.cfg.model = m.to_string();
            self.drafts[F_MODEL].text = m.to_string();
            self.drafts[F_MODEL].cursor = self.drafts[F_MODEL].text.chars().count();
        }
        self.cfg.sanitize();
        cx.notify();
    }

    /// 存设置：三栏草稿回写配置 → 落盘（0600）→ 回会话页。
    fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.cfg.base_url = self.drafts[F_URL].text.trim().to_string();
        self.cfg.model = self.drafts[F_MODEL].text.trim().to_string();
        self.cfg.api_key = self.drafts[F_KEY].text.trim().to_string();
        self.cfg.sanitize();
        self.cfg.save();
        self.settings = false;
        self.error = None;
        cx.notify();
    }

    /// 当前焦点落在第几栏（IME 用）。
    fn focused_field(&self, window: &Window) -> Option<usize> {
        self.focuses.iter().position(|f| f.is_focused(window))
    }

    /// 把焦点交给第 i 栏。
    fn focus_field(&self, i: usize, window: &mut Window) {
        if let Some(f) = self.focuses.get(i) {
            window.focus(f);
        }
    }

    // ── 键盘 ──────────────────────────────────────────────────────
    //
    // 由 `Root::handle_reading_key` 在面板开着时转交过来（返回 true 表示
    // 本面板已消费，Root 不要再当翻页键处理）。

    /// 处理一个按键。返回 true = 已消费（调用方应停止继续派发）。
    pub fn key_down(
        &mut self,
        k: &str,
        mods: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(f) = self.focused_field(window) else {
            return false; // 焦点不在任何输入框：交给 Root（Esc 收栏等）
        };
        match k {
            // ⏎ 发送（⌘⏎ 亦发送，给全键盘用户一个不与输入法打架的键）。
            "enter" | "Enter" => {
                if f == F_ASK {
                    self.send(cx);
                } else {
                    self.save_settings(cx);
                }
                true
            }
            // Esc：正在打字就先清掉这一栏；空手 Esc 不拦，好让 Root 收栏。
            "escape" | "Escape" | "esc" => {
                if !self.drafts[f].text.is_empty() {
                    self.drafts[f].clear();
                    cx.notify();
                    true
                } else {
                    false
                }
            }
            "backspace" | "Backspace" => {
                self.drafts[f].backspace();
                cx.notify();
                true
            }
            "left" | "Left" | "ArrowLeft" => {
                self.drafts[f].move_cursor(true);
                cx.notify();
                true
            }
            "right" | "Right" | "ArrowRight" => {
                self.drafts[f].move_cursor(false);
                cx.notify();
                true
            }
            // ⌘V 粘贴：剪贴板唯一入口（原生 NSPasteboard，见 clipboard.rs）。
            "v" | "V" if mods.platform || mods.control => {
                if let Some(text) = crate::clipboard::read_text(cx) {
                    let one = crate::clipboard::one_line(&text);
                    self.drafts[f].replace_utf16(None, &one, max_of(f));
                    cx.notify();
                }
                true
            }
            // 可打印键一律不直插：中文走输入法（见 ime.rs 架构说明）。
            _ => true,
        }
    }

    /// 点某栏就把焦点给它（鼠标落点即输入落点）。
    pub fn click_field(&self, i: usize, window: &mut Window) {
        self.focus_field(i, window);
    }
}

// ── 网络：一条 OpenAI 兼容的流式对话 ────────────────────────────────────

/// 起一条请求：工作线程里建 tokio 运行时，边读边把事件塞进 `tx`。
///
/// 用独立线程而非 `cx.background_executor()`：这条活儿是**阻塞式**的长连接
/// （一次请求可能持续一两分钟），不该去占 UI 的后台执行器。线程里
/// `catch_unwind` 兜住——网络栈的 panic 绝不能越过线程边界把进程带走。
fn spawn_chat(
    cfg: AiConfig,
    system: String,
    user: String,
    stream: bool,
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
) {
    std::thread::spawn(move || {
        let run = || -> anyhow::Result<()> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(chat(&cfg, &system, &user, stream, &tx))
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = tx.send(StreamEvent::Error(short(&e.to_string())));
            }
            Err(_) => {
                let _ = tx.send(StreamEvent::Error("网络出错，已停下".to_string()));
            }
        }
    });
}

/// 真的发一次请求并读完响应。
async fn chat(
    cfg: &AiConfig,
    system: &str,
    user: &str,
    stream: bool,
    tx: &tokio::sync::mpsc::UnboundedSender<StreamEvent>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use http_body_util::{BodyExt as _, Full};
    use hyper::Request;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let url = cfg.endpoint();
    anyhow::ensure!(!url.is_empty(), "接口地址为空，先在设置里填好");
    let body = ai::chat_body(cfg, system, user, stream);

    // 系统根证书走钥匙串（rustls-native-certs）；`https_or_http()` 让本机
    // Ollama 那种 `http://` 地址也能走通。
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| anyhow::anyhow!("读不到系统证书：{e}"))?
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Full<bytes::Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let mut req = Request::builder()
        .method(hyper::Method::POST)
        .uri(&url)
        .header("content-type", "application/json");
    if cfg.has_key() {
        req = req.header("authorization", format!("Bearer {}", cfg.api_key));
    }
    let req = req.body(Full::new(bytes::Bytes::from(body)))?;

    let fut = client.request(req);
    let resp = tokio::time::timeout(Duration::from_secs(REQUEST_TIMEOUT), fut)
        .await
        .map_err(|_| anyhow::anyhow!("等了 {REQUEST_TIMEOUT} 秒还没回音"))??;

    if !resp.status().is_success() {
        // 401 / 404 / 429…：把服务端那句人话带回去（比光报状态码有用得多）。
        let code = resp.status().as_u16();
        let text = String::from_utf8_lossy(&read_all(resp).await.unwrap_or_default()).to_string();
        let msg = ai::content_of(&text)
            .err()
            .unwrap_or_else(|| text.chars().take(80).collect::<String>());
        let _ = tx.send(StreamEvent::Error(format!("{code}：{msg}")));
        return Ok(());
    }

    if !stream {
        // 非流式：整段 JSON，一次取完。
        let text = String::from_utf8_lossy(&read_all(resp).await?).to_string();
        match ai::content_of(&text) {
            Ok(s) => {
                let _ = tx.send(StreamEvent::Delta(s));
            }
            Err(e) => {
                let _ = tx.send(StreamEvent::Error(short(&e)));
            }
        }
        let _ = tx.send(StreamEvent::Done);
        return Ok(());
    }

    // 流式：逐帧喂给 `SseParser`，攒出一条条事件就往面板送。
    let mut body = resp.into_body();
    let mut parser = ai::SseParser::new();
    let mut evs: Vec<StreamEvent> = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.context("读到一半断了")?;
        if let Ok(data) = frame.into_data() {
            parser.feed(&data, &mut evs);
            for ev in evs.drain(..) {
                let _ = tx.send(ev);
            }
        }
    }
    parser.flush(&mut evs);
    for ev in evs.drain(..) {
        let _ = tx.send(ev);
    }
    Ok(())
}

/// 把整个响应体读成一个字节块。
async fn read_all(resp: hyper::Response<hyper::body::Incoming>) -> anyhow::Result<bytes::Bytes> {
    use http_body_util::BodyExt as _;
    let mut body = resp.into_body();
    let mut out: Vec<u8> = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| anyhow::anyhow!("响应读到一半断了"))?;
        if let Ok(data) = frame.into_data() {
            out.extend_from_slice(&data);
        }
    }
    Ok(bytes::Bytes::from(out))
}

/// 报错只留一句：服务端有时把一整页 HTML 扔回来，别糊满面板。
fn short(s: &str) -> String {
    let one: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let t = one.trim();
    if t.chars().count() > 120 {
        format!("{}…", t.chars().take(120).collect::<String>())
    } else {
        t.to_string()
    }
}

// ── 面板渲染 ────────────────────────────────────────────────────────────

impl Render for AiPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let zhu = crate::reader::sheet_zhu(t);
        let ink = t.ink();
        let soft = t.ink_soft();
        let paper = t.paper();
        // 半透明宣纸底（GPUI 无 backdrop-filter，用纸色近似磨砂）。
        let veil = ((paper & 0xFFFFFF) << 8) | 0xF2;
        let sept = jiandu_core::theme::mix(paper, ink, 0.16);
        let song = self.song.clone();

        // ⚠️ 先把要用的状态摘成值：下面要给每枚钮挂 `cx.listener`（要
        // `&mut self`），迭代期间不能还借着 `self`。
        let settings = self.settings;
        let streaming = self.streaming;
        let has_ctx = self.ctx.as_ref().map(|a| !a.quote.is_empty()).unwrap_or(false);
        let ctx_quote = self
            .ctx
            .as_ref()
            .map(|a| a.quote.clone())
            .unwrap_or_default();
        let error = self.error.clone();
        let provider = self.cfg.provider;
        let key_mask = self.cfg.key_mask();
        let endpoint = self.cfg.endpoint();
        let ready = self.cfg.ready();
        let msgs: Vec<Msg> = self.msgs.clone();
        let drafts: [Draft; 4] = self.drafts.clone();
        let focused = self.focused_field(window);

        // ── 头：题名 + 两枚小钮 ────────────────────────────────────
        let head_btn = |label: &'static str| -> Div {
            div()
                .px(px(6.0))
                .py(px(1.0))
                .text_size(px(10.5))
                .text_color(rgb(soft))
                .cursor_pointer()
                .hover(move |s| s.text_color(rgb(zhu)))
                .child(SharedString::from(label))
        };
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(sept))
            .child(
                div()
                    .font_family(song.clone())
                    .text_size(px(12.5))
                    .text_color(rgb(zhu))
                    .child(SharedString::from(if settings {
                        "問小箋 · 設".to_string()
                    } else {
                        "問小箋".to_string()
                    })),
            );
        header = header.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(head_btn("設置").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.settings = !this.settings;
                        cx.notify();
                    }),
                ))
                .child(head_btn("清談").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.clear_talk(cx);
                    }),
                )),
        );

        // ── 中：会话 / 设置，二选一 ────────────────────────────────
        let mid = if settings {
            self.render_settings(
                provider,
                &key_mask,
                &endpoint,
                ready,
                &drafts,
                focused,
                zhu,
                ink,
                soft,
                paper,
                sept,
                song.clone(),
                cx,
            )
        } else {
            self.render_talk(&msgs, streaming, has_ctx, &error, zhu, ink, soft, song.clone())
        };

        // ── 尾：引文条 + 提问框 ────────────────────────────────────
        let footer = if settings {
            // 设置页不摆提问框（那三栏已经占满，再挤一排反而乱）。
            div().flex().flex_col().px_3().py_2().child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(soft))
                    .child(SharedString::from("密鑰只存本機 ai.json（權限 600），不隨書稿外傳")),
            )
        } else {
            self.render_ask_bar(&drafts, has_ctx, ctx_quote, zhu, ink, soft, paper, window, cx)
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgba(veil))
            .child(header)
            .child(mid)
            .child(footer)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
    }
}

impl AiPanel {
    /// 会话区：一问一答两列小字，答的那条尾部带一枚朱砂游标。
    fn render_talk(
        &self,
        msgs: &[Msg],
        streaming: bool,
        has_ctx: bool,
        error: &Option<String>,
        zhu: u32,
        ink: u32,
        soft: u32,
        song: SharedString,
    ) -> Stateful<Div> {
        let mut list = div()
            .id(SharedString::from("ai-talk-list"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .gap(px(10.0));

        if msgs.is_empty() {
            let hint = if has_ctx {
                "已引一句原文，直接按「問」即可"
            } else {
                "框選正文後點「問箋」，或在此直接發問"
            };
            list = list.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(soft))
                    .child(SharedString::from(hint)),
            );
        }
        for m in msgs {
            let label = if m.mine { "問" } else { "箋" };
            let mut body = div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .font_family(song.clone())
                        .text_size(px(10.0))
                        .text_color(rgb(if m.mine { zhu } else { soft }))
                        .child(SharedString::from(label)),
                );
            if !m.quote.is_empty() && m.mine {
                body = body.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(jiandu_core::theme::mix(ink, zhu, 0.45)))
                        .child(SharedString::from(format!("「{}」", brief(&m.quote, 40)))),
                );
            }
            if !m.text.is_empty() {
                let mut line = div()
                    .text_size(px(12.5))
                    .text_color(rgb(if m.mine { ink } else { ink }))
                    .child(SharedString::from(m.text.clone()));
                if m.typing {
                    // 输出中的游标：一枚朱砂小方块（✕ 类拉丁符号在 GPUI 零宽）。
                    line = line.child(
                        div()
                            .ml(px(2.0))
                            .w(px(6.0))
                            .h(px(12.0))
                            .bg(rgb(zhu)),
                    );
                }
                body = body.child(line);
            }
            list = list.child(body);
        }
        if let Some(e) = error {
            list = list.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(zhu))
                    .child(SharedString::from(e.clone())),
            );
        }
        if streaming {
            list = list.child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(soft))
                    .child(SharedString::from("……")),
            );
        }
        list
    }

    /// 提问条：引文一行 + 输入框一行 + 「問」钮。
    fn render_ask_bar(
        &self,
        drafts: &[Draft; 4],
        has_ctx: bool,
        ctx_quote: String,
        zhu: u32,
        ink: u32,
        soft: u32,
        paper: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let d = &drafts[F_ASK];
        let n = d.text.chars().count();
        let cur = d.cursor.min(n);
        let before: String = d.text.chars().take(cur).collect();
        let after: String = d.text.chars().skip(cur).collect();
        let cursor_on = self
            .focuses
            .get(F_ASK)
            .map(|f| f.is_focused(window))
            .unwrap_or(false);
        let box_body = div()
            .flex()
            .flex_row()
            .items_center()
            .text_size(px(13.0))
            .text_color(rgb(ink))
            .child(SharedString::from(before))
            .child(
                div()
                    .w(px(1.5))
                    .h(px(15.0))
                    .bg(rgb(if cursor_on { zhu } else { 0 })),
            )
            .child(SharedString::from(after));
        // 灰字提示：没字也没引文时给一句，别让用户对着空条发愣。
        let hint = if !d.text.is_empty() {
            String::new()
        } else if has_ctx {
            "直接按「問」，或寫一句追問".to_string()
        } else {
            "想問什麼".to_string()
        };
        let field = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .flex_1()
            .py_2()
            .border_b_1()
            .border_color(rgb(zhu))
            .child(if hint.is_empty() {
                box_body
            } else {
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(soft))
                    .child(SharedString::from(hint))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.click_field(F_ASK, window);
                    cx.notify();
                }),
            );
        let input = InputField::new(field, self.focuses[F_ASK].clone(), cx.entity());

        let send = div()
            .px(px(8.0))
            .py(px(2.0))
            .text_size(px(12.0))
            .text_color(rgb(if self.streaming { soft } else { zhu }))
            .cursor_pointer()
            .hover(move |s| s.text_color(rgb(zhu)))
            .child(SharedString::from(if self.streaming { "候" } else { "問" }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.send(cx);
                }),
            );

        let row = div()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .child(input)
            .child(send);

        let mut col = div().flex().flex_col().px_3().py_2().gap(px(6.0));
        if has_ctx {
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(zhu))
                            .child(SharedString::from("引")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.0))
                            .text_color(rgb(jiandu_core::theme::mix(ink, zhu, 0.35)))
                            .child(SharedString::from(brief(&ctx_quote, 60))),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(soft))
                            .cursor_pointer()
                            .hover(move |s| s.text_color(rgb(zhu)))
                            .child(SharedString::from("去"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.ctx = None;
                                    cx.notify();
                                }),
                            ),
                    ),
            );
        }
        col = col.child(row);
        let _ = paper;
        col
    }

    /// 设置页：服务商 chips + 三栏输入 + 「存」。
    #[allow(clippy::too_many_arguments)]
    fn render_settings(
        &self,
        provider: usize,
        key_mask: &str,
        endpoint: &str,
        ready: bool,
        drafts: &[Draft; 4],
        focused: Option<usize>,
        zhu: u32,
        ink: u32,
        soft: u32,
        paper: u32,
        sept: u32,
        song: SharedString,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let mut col = div()
            .id(SharedString::from("ai-settings"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .gap(px(10.0));

        // 服务商：一排 chips（GPUI 没有下拉框，一行小字钮最像纸上的选择）。
        let mut chips = div().flex().flex_row().flex_wrap().gap(px(6.0));
        for (i, p) in PROVIDERS.iter().enumerate() {
            let on = i == provider;
            chips = chips.child(
                div()
                    .px(px(7.0))
                    .py(px(2.0))
                    .text_size(px(11.0))
                    .text_color(rgb(if on { zhu } else { ink }))
                    .border_1()
                    .border_color(rgb(if on { zhu } else { sept }))
                    .rounded(px(3.0))
                    .cursor_pointer()
                    .hover(move |s| s.text_color(rgb(zhu)))
                    .child(SharedString::from(p.label))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.pick_provider(i, cx);
                        }),
                    ),
            );
        }
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(label("服務商", soft))
                .child(chips),
        );

        // 三栏：地址 / 模型 / 密鑰。
        col = col.child(self.field_row(
            F_URL, "接口地址", drafts, focused, zhu, ink, soft, paper, cx,
        ));
        col = col.child(self.field_row(
            F_MODEL, "模型", drafts, focused, zhu, ink, soft, paper, cx,
        ));
        col = col.child(self.field_row(
            F_KEY, "密鑰", drafts, focused, zhu, ink, soft, paper, cx,
        ));

        // 现状一行：端点与密钥外观（**只显掩码**，原值不出现在界面上）。
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(soft))
                        .child(SharedString::from(if endpoint.is_empty() {
                            "尚未填寫接口地址".to_string()
                        } else {
                            format!("端點 {endpoint}")
                        })),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(if ready { soft } else { zhu }))
                        .child(SharedString::from(format!(
                            "密鑰 {} · {}",
                            key_mask,
                            if ready { "可問" } else { "尚不能問" }
                        ))),
                ),
        );

        // 「存」：存完回会话页。
        col = col.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(3.0))
                        .font_family(song)
                        .text_size(px(12.0))
                        .text_color(rgb(zhu))
                        .border_1()
                        .border_color(rgb(zhu))
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .child(SharedString::from("存"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.save_settings(cx);
                            }),
                        ),
                ),
        );
        col
    }

    /// 设置页里的一栏：标签 + 自绘输入框（可聚焦、可接 IME）。
    #[allow(clippy::too_many_arguments)]
    fn field_row(
        &self,
        which: usize,
        label_text: &'static str,
        drafts: &[Draft; 4],
        focused: Option<usize>,
        zhu: u32,
        ink: u32,
        soft: u32,
        paper: u32,
        cx: &mut Context<Self>,
    ) -> Div {
        let d = &drafts[which];
        let n = d.text.chars().count();
        let cur = d.cursor.min(n);
        let on = focused == Some(which);
        // 密鑰栏**永不显示原值**：只画掩码，免得旁人路过一眼看走。
        let shown = if which == F_KEY {
            vec!['●'; d.text.chars().count().min(24)]
                .into_iter()
                .collect::<String>()
        } else {
            d.text.clone()
        };
        let shown_n = shown.chars().count();
        let cur = cur.min(shown_n);
        let before: String = shown.chars().take(cur).collect();
        let after: String = shown.chars().skip(cur).collect();
        let body = div()
            .flex()
            .flex_row()
            .items_center()
            .text_size(px(11.5))
            .text_color(rgb(ink))
            .child(SharedString::from(before))
            .child(div().w(px(1.5)).h(px(14.0)).bg(rgb(if on { zhu } else { 0 })))
            .child(SharedString::from(after));
        let field = div()
            .flex_1()
            .py_1()
            .px_1()
            .bg(rgb(paper))
            .border_b_1()
            .border_color(rgb(if on { zhu } else { soft }))
            .child(body)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.click_field(which, window);
                    cx.notify();
                }),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(label(label_text, soft))
            .child(InputField::new(field, self.focuses[which].clone(), cx.entity()))
    }
}

/// 设置页里的小标签。
fn label(text: &'static str, soft: u32) -> Div {
    div()
        .text_size(px(10.0))
        .text_color(rgb(soft))
        .child(SharedString::from(text))
}

/// 截断长句（引文条与气泡里都不该占半屏）。
fn brief(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

// ── IME：让四个输入框都能打中文 ─────────────────────────────────────────

impl EntityInputHandler for AiPanel {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let i = self.focused_field(_window)?;
        *adjusted_range = Some(range.clone());
        Some(crate::ime::utf16_slice(&self.drafts[i].text, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let i = self.focused_field(window)?;
        let at = crate::ime::char_to_utf16(&self.drafts[i].text, self.drafts[i].cursor);
        Some(UTF16Selection {
            range: at..at,
            reversed: false,
        })
    }

    fn marked_text_range(&self, window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        let i = self.focused_field(window)?;
        self.drafts[i].marked.clone()
    }

    fn unmark_text(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(i) = self.focused_field(window) {
            self.drafts[i].marked = None;
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.focused_field(window) else {
            return;
        };
        self.drafts[i].marked = None;
        self.drafts[i].replace_utf16(range, text, max_of(i));
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.focused_field(window) else {
            return;
        };
        let eff = range.or_else(|| self.drafts[i].marked.clone());
        let start = self.drafts[i].replace_utf16(eff, new_text, max_of(i));
        let end = start + crate::ime::utf16_len(new_text);
        self.drafts[i].marked = Some(start..end);
        self.drafts[i].cursor = match new_selected_range {
            Some(sel) => crate::ime::utf16_to_char(&self.drafts[i].text, sel.end),
            None => crate::ime::utf16_to_char(&self.drafts[i].text, end),
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// 让 `Root` 能把键盘事件转进来（面板开着且焦点在输入框时）。
pub fn forward_key(
    panel: &Entity<AiPanel>,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let k = event.keystroke.key.clone();
    let mods = event.keystroke.modifiers;
    panel.update(cx, |p, cx| p.key_down(&k, mods, window, cx))
}

// ── Root 侧的接线：开门 / 带引文来问 / 收栏 ──────────────────────────────

impl crate::app::Root {
    /// 开 / 收小笺栏（顶栏「问笺」走这条）。
    pub fn toggle_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_ui_state(crate::app::AppUIState::Ai, cx);
        // 开栏就把焦点递给提问框：否则输入法没有落点，打不进中文。
        if self.show_ai {
            let f = self.ai_panel.read(cx).ask_focus();
            window.focus(&f);
        }
        cx.notify();
    }

    /// **框了原文再来问**（选区工具条「问笺」走这条）。
    ///
    /// 引文在这里成形，随即交给面板：书名 / 章名 + 原文 + 前后文。
    /// 面板若没开就顺手开——用户的意图是「问这一句」，不是「开个面板看看」。
    pub fn ask_ai_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ask = self.ask_of_selection();
        if !self.show_ai {
            self.set_ui_state(crate::app::AppUIState::Ai, cx);
        }
        let ask2 = ask.clone();
        self.ai_panel.update(cx, |p, cx| {
            p.take_context(ask2, cx);
            p.ask_quote(cx);
        });
        let f = self.ai_panel.read(cx).ask_focus();
        window.focus(&f);
        cx.notify();
    }

    /// 收起小笺栏（Esc / 顶栏再点一次都走这里）。
    pub fn close_ai(&mut self) {
        self.show_ai = false;
        self.ai_target = false;
        self.ai_animating = self.ai_slide > 0.0;
    }

    /// 把「当前选区」压成一份 [`Ask`]。
    ///
    /// 引文取选区原文（没选区就退回工具条上那一个字），前后文按段落拼出
    /// 整章文本后按选区定位——**只读不写**，绝不碰分页与排版。
    fn ask_of_selection(&self) -> Ask {
        let quote = self.selected_text();
        let key = self
            .selection_range()
            .map(|(a, _)| a)
            .or_else(|| self.popup.as_ref().map(|h| h.key));
        let (chapter, para, offset) = key.unwrap_or((0, 0, 0));
        let book = self
            .book
            .as_ref()
            .map(|b| b.title.clone())
            .unwrap_or_default();
        let chapter_title = self.chapter_title(chapter);
        let len = quote.chars().count();
        let (before, after) = self.passage_around(chapter, para, offset, len);
        Ask {
            book,
            chapter: chapter_title,
            quote,
            before,
            after,
            question: String::new(),
        }
    }

    /// 选区在**整章文本**里的来路与去路。
    ///
    /// 把本章各段拼成一整条，记下每段起点，选区的段内偏移加上段起点就是
    /// 它在章里的位置；两侧各留一段（由 core 按 `ctx_chars` 再裁一次，
    /// 这里只做一个宽松的初剪，免得把整章几万字都拷进来）。
    fn passage_around(
        &self,
        chapter: usize,
        para: usize,
        offset: usize,
        len: usize,
    ) -> (String, String) {
        let Some(ch) = self.book.as_ref().and_then(|b| b.chapters.get(chapter)) else {
            return (String::new(), String::new());
        };
        let mut flat = String::new();
        let mut starts: Vec<usize> = Vec::with_capacity(ch.paragraphs.len());
        for p in &ch.paragraphs {
            starts.push(flat.chars().count());
            flat.push_str(&p.text);
        }
        let base = starts.get(para).copied().unwrap_or(0);
        let at = base + offset;
        let before: String = flat.chars().take(at).collect();
        let after: String = flat.chars().skip(at.saturating_add(len)).collect();
        // 先粗剪一刀（每侧 2000 字），真正的预算由 `AiConfig::ctx_chars` 定。
        (
            ai::clip_tail(&before, 2000),
            ai::clip_head(&after, 2000),
        )
    }
}
