//! **问小笺**（V2.0 模块二 · AI 伴读）的纯逻辑层。
//!
//! 这里只回答三件「与界面无关」的事：
//!   ① **问什么**——把书名 / 章名 / 选区原文 / 前后文 / 用户的问题，压成一段
//!      有分寸的提示词（见 [`Ask`] 与 [`user_prompt`]）；
//!   ② **问谁**——服务商预设与连接参数（见 [`AiConfig`]）：base_url、模型、
//!      密钥、温度。密钥落盘时**单独一个文件、权限 0600**，绝不混进
//!      `prefs.json`（那份会被同步 / 导出），也绝不进日志；
//!   ③ **怎么读回**——把 SSE 字节流增量切成一条条 [`StreamEvent`]
//!      （见 [`SseParser`]）：中文常被切在多字节边界上，故缓冲按**字节**攒，
//!      只在见到 `\n` 时才成行解析，绝不按 `char` 拼。
//!
//! HTTP 本身不在这里（core 零网络依赖）：真发请求的是 UI 层 `src/ai.rs`，
//! 它拿 [`chat_body`] 当请求体、拿 [`SseParser`] 当解析器。
//!
//! 与书架 / 章节库的关系：**不落任何历史**。问过小笺什么，关掉就没了——
//! 卡片匣（模块三）才负责「留住」，问小笺只负责「当下这一问」。

use crate::library::Library;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

/// 配置文件名（落在 `Library::home()` 下，与 `prefs.json` 同级）。
///
/// 单独一份而非并入 `prefs.json`：密钥不该搭用户的阅读偏好一起被同步、
/// 导出或贴到工单里。`save` 时额外把权限压到 0600。
pub const AI_FILE: &str = "ai.json";

/// **系统提示词**：给小笺定身份与边界。
///
/// 三条硬约束是刻意写死的，因为它们直接决定「答得像不像读古文的先生」：
///   1. **不许臆造**——引文没有的就答「原文未及」，绝不用常识补典故；
///   2. **先串讲后点睛**——先现代汉语讲通，再点关键字词，最后才给出处；
///   3. **不写 markdown**——面板里没有渲染器，`#` 与 `**` 只会变成噪点。
pub const DEFAULT_SYSTEM: &str = "\
你是「笺读」中的伴读小笺，一位熟读经史子集的古文先生，为正在读古籍的人答疑。
作答须守以下分寸：
一、只依据用户给出的原文与上下文作答。原文未及之处，直说「原文未及」，不要以常识臆造典故、人名、年份。
二、先以现代汉语串讲文义（三句以内），再点出关键字词（读音 / 通假 / 词义），确有出处时才注明出处。
三、不写 markdown：不要用 # 标题、**加粗**、列表符号。分段用换行即可，引原文用「」。
四、用户以简体提问就用简体作答，以繁体提问就用繁体作答。
五、有争议处并列两说，并说明分歧所在，不下独断。";

/// 没输入问题、只框了句子时的默认问法（工具条「问笺」直点即用这一句）。
pub const DEFAULT_QUESTION: &str = "请疏通此句：以现代汉语讲通文义，点出关键字词与用典。";

/// 服务商预设：一份「照抄即可用」的连接参数。
///
/// 全部是 **OpenAI 兼容**的 `/chat/completions` 接口——国产大模型几乎都提供
/// 这种兼容端点，故只写一份协议实现就能通吃，不必给每家写一套。
#[derive(Clone, Copy, Debug)]
pub struct Provider {
    /// 稳定标识（存档用这个，不用下标——下标会随列表增删而错位）。
    pub id: &'static str,
    /// 界面上的名字。
    pub label: &'static str,
    /// 接口根地址（`endpoint()` 会补上 `/chat/completions`）。
    pub base_url: &'static str,
    /// 常用模型（界面下拉里给，可手填）。
    pub models: &'static [&'static str],
    /// 密钥输入框的灰字提示。
    pub key_hint: &'static str,
    /// 是否必须填密钥（本地 Ollama 不需要）。
    pub needs_key: bool,
}

/// 预设清单（顺序即界面下拉顺序）。
pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "deepseek",
        label: "深度求索",
        base_url: "https://api.deepseek.com/v1",
        models: &["deepseek-chat", "deepseek-reasoner"],
        key_hint: "sk-…（控制台「API Keys」里取）",
        needs_key: true,
    },
    Provider {
        id: "moonshot",
        label: "月之暗面",
        base_url: "https://api.moonshot.cn/v1",
        models: &["moonshot-v1-8k", "moonshot-v1-32k", "kimi-k2-turbo-preview"],
        key_hint: "sk-…（控制台里取）",
        needs_key: true,
    },
    Provider {
        id: "qwen",
        label: "通义千问",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &["qwen-plus", "qwen-max", "qwen-turbo"],
        key_hint: "sk-…（百炼控制台里取）",
        needs_key: true,
    },
    Provider {
        id: "zhipu",
        label: "智谱清言",
        base_url: "https://open.bigmodel.cn/api/paas/v4",
        models: &["glm-4-flash", "glm-4-plus", "glm-4.6"],
        key_hint: "id.secret（控制台里取）",
        needs_key: true,
    },
    Provider {
        id: "openai",
        label: "OpenAI",
        base_url: "https://api.openai.com/v1",
        models: &["gpt-4o-mini", "gpt-4o"],
        key_hint: "sk-…（platform.openai.com 里取）",
        needs_key: true,
    },
    Provider {
        id: "siliconflow",
        label: "硅基流动",
        base_url: "https://api.siliconflow.cn/v1",
        models: &["Qwen/Qwen2.5-7B-Instruct", "THUDM/glm-4-9b-chat"],
        key_hint: "sk-…（控制台里取）",
        needs_key: true,
    },
    Provider {
        id: "ollama",
        label: "本机 Ollama",
        base_url: "http://localhost:11434/v1",
        models: &["qwen2.5:7b", "llama3.1:8b"],
        key_hint: "本机无需密钥（留空即可）",
        needs_key: false,
    },
    Provider {
        id: "custom",
        label: "自定义",
        base_url: "",
        models: &[],
        key_hint: "按你的服务商要求填",
        needs_key: false,
    },
];

/// 按 id 取预设；查不到（老存档里的陌生 id）回落首项，绝不 panic。
pub fn provider_of(id: &str) -> &'static Provider {
    PROVIDERS
        .iter()
        .find(|p| p.id == id)
        .unwrap_or(&PROVIDERS[0])
}

/// 按下标取预设（越界回落首项）。
pub fn provider_at(i: usize) -> &'static Provider {
    PROVIDERS.get(i).unwrap_or(&PROVIDERS[0])
}

/// 预设在清单里的下标（按 id 查，查不到给 0）。
pub fn provider_index(id: &str) -> usize {
    PROVIDERS
        .iter()
        .position(|p| p.id == id)
        .unwrap_or(0)
}

/// 连接配置。
///
/// `provider` 只作**初始化来源**（选预设时把 base_url / 模型填上），真正的
/// 连接参数以 `base_url` / `model` 为准——用户手改过就以自己的为准，切预设
/// 才覆写。这样「选了智谱又手填了个内部模型」不会被下拉框悄悄改回去。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiConfig {
    /// `PROVIDERS` 下标（**只是初值来源**，见上）。
    #[serde(default)]
    pub provider: usize,
    /// 服务商标识（存档认这个，不认下标）。
    #[serde(default = "default_provider_id")]
    pub provider_id: String,
    /// 接口根地址。
    #[serde(default)]
    pub base_url: String,
    /// 模型名。
    #[serde(default)]
    pub model: String,
    /// API Key。**只**写进 `ai.json`（0600），不进日志、不进其它存档。
    #[serde(default)]
    pub api_key: String,
    /// 温度（0 = 最稳，1 = 最活）。古文答疑宜偏低。
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// 上下文窗口（**字数**，两侧合计上限）：给模型的原文前后文总长。
    #[serde(default = "default_ctx_chars")]
    pub ctx_chars: usize,
    /// 系统提示词（空则用 [`DEFAULT_SYSTEM`]）。
    #[serde(default)]
    pub system: String,
    /// 流式输出（一字一字往外吐）。关掉则等一整段回来。
    #[serde(default = "default_true")]
    pub stream: bool,
}

fn default_provider_id() -> String {
    PROVIDERS[0].id.to_string()
}
fn default_temperature() -> f32 {
    0.3
}
fn default_ctx_chars() -> usize {
    1200
}
fn default_true() -> bool {
    true
}

impl Default for AiConfig {
    fn default() -> Self {
        Self::for_provider(0)
    }
}

impl AiConfig {
    /// 按预设造一份：把预设的 base_url 与首个模型填上，密钥留空等人填。
    pub fn for_provider(i: usize) -> Self {
        let p = provider_at(i);
        Self {
            provider: i,
            provider_id: p.id.to_string(),
            base_url: p.base_url.to_string(),
            model: p.models.first().copied().unwrap_or_default().to_string(),
            api_key: String::new(),
            temperature: default_temperature(),
            ctx_chars: default_ctx_chars(),
            system: String::new(),
            stream: true,
        }
    }

    pub fn path() -> PathBuf {
        Library::home().join(AI_FILE)
    }

    /// 读配置；文件缺失 / 损坏一律回落默认（不让坏存档挡住启动）。
    pub fn load() -> Self {
        let mut c: Self = std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        c.sanitize();
        c
    }

    /// 写配置：目录顺手建，**权限压到 0600**（同机其它用户读不到密钥）。
    /// 失败一律静默——配不上小笺不该打断阅读。
    pub fn save(&self) {
        let dir = Library::home();
        let _ = std::fs::create_dir_all(&dir);
        let Ok(s) = serde_json::to_string_pretty(self) else {
            return;
        };
        let path = Self::path();
        if std::fs::write(&path, s).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }

    /// 存档里的下标与 `provider_id` 可能对不上（列表增删过）：以 id 为准回正。
    /// 顺手 trim 三处文本、去掉 base_url 尾部斜杠（拼接时才不会 `//`）。
    pub fn sanitize(&mut self) {
        if !self.provider_id.is_empty() {
            self.provider = provider_index(&self.provider_id);
        }
        let p = provider_at(self.provider);
        self.provider_id = p.id.to_string();
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.model = self.model.trim().to_string();
        self.api_key = self.api_key.trim().to_string();
        if self.temperature.is_nan() || !(0.0..=2.0).contains(&self.temperature) {
            self.temperature = default_temperature();
        }
        if self.ctx_chars == 0 {
            self.ctx_chars = default_ctx_chars();
        }
    }

    /// 是否具备发问条件：地址与模型齐全，且（需要密钥时）密钥已填。
    pub fn ready(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty() && (self.has_key() || !self.needs_key())
    }

    pub fn has_key(&self) -> bool {
        !self.api_key.is_empty()
    }

    /// 是否要密钥：**预设要求**，或**地址不是本机**（远端几乎都要验身份）。
    ///
    /// 两者取「或」而非「且」：自定义档明明填了个远端地址，就不能因为
    /// 「自定义不强制」就把没填密钥的请求放出去（那是必然的 401）。
    pub fn needs_key(&self) -> bool {
        provider_at(self.provider).needs_key || !is_local(&self.base_url)
    }

    /// 完整的对话端点：根地址已带 `/chat/completions` 就不重复拼。
    pub fn endpoint(&self) -> String {
        let b = self.base_url.trim_end_matches('/');
        if b.is_empty() {
            return String::new();
        }
        if b.ends_with("/chat/completions") {
            b.to_string()
        } else {
            format!("{b}/chat/completions")
        }
    }

    /// 密钥的**外观**：只留前三后四，中间全遮。
    ///
    /// 界面回显与日志一律走这里——原始密钥除了请求头，哪儿都不出现。
    pub fn key_mask(&self) -> String {
        let k = self.api_key.trim();
        if k.is_empty() {
            return "（未填）".to_string();
        }
        let n = k.chars().count();
        if n <= 8 {
            // 短到遮不出结构：全遮，只报长度。
            return format!("{} 位 · 已填", n);
        }
        let head: String = k.chars().take(3).collect();
        let tail: String = k.chars().skip(n.saturating_sub(4)).collect();
        format!("{head}···{tail}")
    }

    /// 系统提示词：用户没改写就用默认的。
    pub fn system_prompt(&self) -> &str {
        if self.system.trim().is_empty() {
            DEFAULT_SYSTEM
        } else {
            self.system.as_str()
        }
    }
}

/// 是否本机地址（本机服务一般不要密钥，故免检）。
fn is_local(base: &str) -> bool {
    let b = base.trim();
    b.starts_with("http://localhost")
        || b.starts_with("http://127.0.0.1")
        || b.starts_with("http://0.0.0.0")
        || b.starts_with("http://[::1]")
}

/// **一问的全部材料**：出处 + 引文 + 前后文 + 问题。
///
/// 全部是 `String`（不持 `Book` / `Glyph` 的引用）：这条材料要跨线程送到
/// 网络任务里去，引用活不了那么久。
#[derive(Clone, Debug, Default)]
pub struct Ask {
    /// 书名（可空）。
    pub book: String,
    /// 章名 / 卷次（可空）。
    pub chapter: String,
    /// 框选的原文（主体）。
    pub quote: String,
    /// 引文之前的上下文（已按字数裁好）。
    pub before: String,
    /// 引文之后的上下文（已按字数裁好）。
    pub after: String,
    /// 用户的问题（空则用 [`DEFAULT_QUESTION`]）。
    pub question: String,
}

impl Ask {
    /// 有没有可问的东西（书名 + 引文 + 问题三者全空才算没得问）。
    pub fn is_empty(&self) -> bool {
        self.book.is_empty() && self.quote.is_empty() && self.question.trim().is_empty()
    }
}

/// 裁尾巴：留**最后** `max` 字（前文要的就是靠近引文的那一段）。
pub fn clip_tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let skip = n - max;
    format!("…{}", s.chars().skip(skip).collect::<String>())
}

/// 裁头：留**最前** `max` 字（后文同理）。
pub fn clip_head(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max).collect::<String>())
}

/// 组装给模型的**用户消息**。
///
/// 次序是刻意的：先报出处（模型才知道在读哪一本）、再给上下文与引文、
/// **最后**才是问题——把问题压在末尾，模型最不容易忘掉真正要答什么。
/// 各段用 `【】` 起头：面板里没有 markdown，靠这几处方括号分节。
pub fn user_prompt(a: &Ask, ctx_chars: usize) -> String {
    let mut s = String::new();
    let src = match (a.book.trim(), a.chapter.trim()) {
        ("", "") => String::new(),
        (b, "") => b.to_string(),
        ("", c) => c.to_string(),
        (b, c) => format!("{b}·{c}"),
    };
    if !src.is_empty() {
        s.push_str(&format!("【出处】{src}\n"));
    }
    // 上下文预算：前后各半（前文稍多——古文是顺着读下来的，来路更要紧）。
    let half = ctx_chars / 2;
    let before = clip_tail(&a.before, half);
    let after = clip_head(&a.after, half);
    if !before.is_empty() {
        s.push_str(&format!("【前文】{before}\n"));
    }
    if !a.quote.is_empty() {
        s.push_str(&format!("【原文】{}\n", a.quote));
    }
    if !after.is_empty() {
        s.push_str(&format!("【后文】{after}\n"));
    }
    let q = a.question.trim();
    let q = if q.is_empty() {
        DEFAULT_QUESTION.to_string()
    } else {
        q.to_string()
    };
    s.push_str(&format!("【问题】{q}"));
    s
}

/// 请求体（OpenAI 兼容的 `/chat/completions`）。
///
/// `stream` 由调用方给：面板要一字一字往外吐就 true；`stream: false` 时
/// 服务端回整段 JSON，`SseParser` 一样读得动（见 `choices[0].message.content`）。
pub fn chat_body(cfg: &AiConfig, system: &str, user: &str, stream: bool) -> String {
    let temp = if cfg.temperature.is_nan() {
        default_temperature()
    } else {
        cfg.temperature.clamp(0.0, 2.0)
    };
    json!({
        "model": cfg.model,
        "stream": stream,
        "temperature": temp,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    })
    .to_string()
}

/// SSE 流里的一件事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    /// 又吐出一段字（增量，直接往答案尾部追加）。
    Delta(String),
    /// 正常收束（`data: [DONE]` 或 `finish_reason` 落定）。
    Done,
    /// 服务端报错（401 / 余额不足 / 模型不存在…）：带一句人话。
    Error(String),
}

/// **增量 SSE 解析器**：喂字节，吐事件。
///
/// 缓冲按**字节**攒（不是 `String`）：一次网络读可能正停在某个汉字的第二个
/// 字节上，若先转 `String` 就会得到 `U+FFFD`。这里只在攒到 `\n` 时才成行
/// 解析，故每一行都是完整的，`from_utf8_lossy` 永远不会咬到半个字。
///
/// 容错原则与清洗层一致——**宁可漏判不可误杀**：认不出的行整行丢掉，
/// 绝不把半截 JSON 当错误抛给界面（那会让一次网络抖动变成满屏报错）。
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 喂一段字节，事件追加进 `out`。
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) {
        self.buf.extend_from_slice(bytes);
        // 逐行切：只认 `\n`（SSE 规范用 CRLF / LF，`\r` 由 trim 吃掉）。
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.line(&String::from_utf8_lossy(&line), out);
        }
        // 单行过长（服务端不换行地灌）时兜一下：别让缓冲无限涨。
        if self.buf.len() > 1 << 20 {
            self.buf.clear();
        }
    }

    /// 流结束：把最后没换行的一段也当一行处理（服务端不一定以 `\n` 收尾）。
    pub fn flush(&mut self, out: &mut Vec<StreamEvent>) {
        if self.buf.is_empty() {
            return;
        }
        let line: Vec<u8> = self.buf.drain(..).collect();
        self.line(&String::from_utf8_lossy(&line), out);
    }

    fn line(&self, raw: &str, out: &mut Vec<StreamEvent>) {
        let s = raw.trim();
        if s.is_empty() || s.starts_with(':') {
            return; // 空行 / 心跳注释
        }
        let Some(data) = s.strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() {
            return;
        }
        if data == "[DONE]" {
            out.push(StreamEvent::Done);
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return;
        };
        // 报错优先：{"error":{"message":"..."}} 或 {"error":"..."}
        if let Some(e) = v.get("error") {
            let msg = e
                .get("message")
                .and_then(|m| m.as_str())
                .or_else(|| e.as_str())
                .unwrap_or("服务端返回了错误");
            out.push(StreamEvent::Error(msg.to_string()));
            return;
        }
        if let Some(c) = v
            .pointer("/choices/0/delta/content")
            .and_then(|x| x.as_str())
        {
            if !c.is_empty() {
                out.push(StreamEvent::Delta(c.to_string()));
            }
        } else if let Some(c) = v
            .pointer("/choices/0/message/content")
            .and_then(|x| x.as_str())
        {
            // 非流式：整段内容一次性回来。
            if !c.is_empty() {
                out.push(StreamEvent::Delta(c.to_string()));
            }
        }
        // 有些服务端先给 content 再在同一帧给 finish_reason，两者都要。
        if v.pointer("/choices/0/finish_reason")
            .and_then(|x| x.as_str())
            .is_some()
        {
            out.push(StreamEvent::Done);
        }
    }
}

/// **非流式**回包：整段 JSON 不是 SSE，走这里取内容。
///
/// 返回 `Ok(内容)` / `Err(一句人话)`。只在 `stream: false` 时用到——开着
/// 流式时字节流一律交给 [`SseParser`]。
pub fn content_of(body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "回包不是合法 JSON".to_string())?;
    if let Some(e) = v.get("error") {
        let msg = e
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| e.as_str())
            .unwrap_or("服务端返回了错误");
        return Err(msg.to_string());
    }
    v.pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "回包里没有答案".to_string())
}

/// 把可能带前导空格 / 换行的回答收拾干净（模型爱在开头吐两个空行）。
pub fn tidy_answer(s: &str) -> String {
    s.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n'])
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_normalizes_trailing_slash() {
        let mut c = AiConfig::default();
        c.base_url = "https://api.deepseek.com/v1/".to_string();
        assert_eq!(c.endpoint(), "https://api.deepseek.com/v1/chat/completions");
        // 已带完整路径：不重复拼。
        c.base_url = "https://open.bigmodel.cn/api/paas/v4/chat/completions".to_string();
        assert_eq!(
            c.endpoint(),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        // 空地址不硬凑出 "/chat/completions"。
        c.base_url = String::new();
        assert_eq!(c.endpoint(), "");
    }

    #[test]
    fn ready_requires_url_model_and_key() {
        let mut c = AiConfig::default();
        assert!(!c.ready(), "没填密钥不该放行");
        c.api_key = "sk-test".to_string();
        assert!(c.ready());
        c.model = String::new();
        assert!(!c.ready(), "没模型不该放行");
        // 本机 Ollama：免密钥。
        let mut o = AiConfig::for_provider(provider_index("ollama"));
        assert!(o.ready(), "本机服务不该要密钥");
        o.base_url = "https://example.com/v1".to_string();
        assert!(!o.ready(), "换成远端地址就得要密钥");
    }

    #[test]
    fn key_mask_hides_the_middle() {
        let mut c = AiConfig::default();
        assert_eq!(c.key_mask(), "（未填）");
        c.api_key = "sk-abcdefghijklmnop".to_string();
        let m = c.key_mask();
        assert!(m.starts_with("sk-"));
        assert!(m.ends_with("mnop"));
        assert!(!m.contains("ghijk"), "中段必须遮掉：{m}");
        // 短密钥只报长度，不泄字符。
        c.api_key = "abc123".to_string();
        assert_eq!(c.key_mask(), "6 位 · 已填");
    }

    #[test]
    fn config_round_trips_and_keeps_unknown_provider_id() {
        let mut c = AiConfig::default();
        c.provider_id = "zhipu".to_string();
        c.model = "glm-4-flash".to_string();
        c.temperature = 0.8;
        let s = serde_json::to_string(&c).expect("序列化");
        let back: AiConfig = serde_json::from_str(&s).expect("反序列化");
        assert_eq!(back.provider_id, "zhipu");
        assert_eq!(back.model, "glm-4-flash");
        // 老存档没有新字段：不能整份反序列化失败。
        let old = r#"{"base_url":"https://x/v1","model":"m","api_key":"k"}"#;
        let old: AiConfig = serde_json::from_str(old).expect("老存档应能读进来");
        assert!(old.stream, "流式默认开");
        assert_eq!(old.temperature, 0.3);
        assert_eq!(old.provider_id, PROVIDERS[0].id);
    }

    #[test]
    fn sanitize_fixes_index_by_id_and_trims() {
        let mut c = AiConfig {
            provider: 99,
            provider_id: "qwen".to_string(),
            base_url: "  https://dashscope.aliyuncs.com/compatible-mode/v1/  ".to_string(),
            model: " qwen-plus ".to_string(),
            api_key: " sk-x ".to_string(),
            temperature: f32::NAN,
            ctx_chars: 0,
            system: String::new(),
            stream: true,
        };
        c.sanitize();
        assert_eq!(c.provider, provider_index("qwen"));
        assert_eq!(c.base_url, "https://dashscope.aliyuncs.com/compatible-mode/v1");
        assert_eq!(c.model, "qwen-plus");
        assert_eq!(c.api_key, "sk-x");
        assert_eq!(c.temperature, 0.3);
        assert_eq!(c.ctx_chars, default_ctx_chars());
    }

    #[test]
    fn saved_config_file_is_private() {
        // 不碰真实的用户目录：临时目录里另起炉灶。
        let tmp = std::env::temp_dir().join(format!("jiandu-ai-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let path = tmp.join(AI_FILE);
        let mut c = AiConfig::default();
        c.api_key = "sk-should-not-be-world-readable".to_string();
        let s = serde_json::to_string_pretty(&c).expect("序列化");
        std::fs::write(&path, s).expect("写盘");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("改权限");
            let mode = std::fs::metadata(&path)
                .expect("读元信息")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "密钥文件必须只有本人能读");
        }
        let back: AiConfig =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("读回")).expect("解析");
        assert_eq!(back.api_key, c.api_key);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sse_reads_delta_across_byte_boundary() {
        // 「道」= E9 81 93：故意把一次网络读切在第二个字节后，
        // 解析器必须攒够了才吐，绝不能先吐一个 U+FFFD。
        let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"道\"}}]}\n\n";
        let bytes = payload.as_bytes();
        let cut = bytes
            .iter()
            .position(|b| *b == 0x81)
            .expect("找得到切点")
            + 1;
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.feed(&bytes[..cut], &mut out);
        assert!(out.is_empty(), "半个字不能提前吐事件：{out:?}");
        p.feed(&bytes[cut..], &mut out);
        assert_eq!(out, vec![StreamEvent::Delta("道".to_string())]);
    }

    #[test]
    fn sse_done_and_error_and_noise() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.feed(b": ping\n\n", &mut out);
        p.feed(
            "data: {\"choices\":[{\"delta\":{\"content\":\"可\"}}]}\n\n".as_bytes(),
            &mut out,
        );
        p.feed(
            b"data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                StreamEvent::Delta("可".to_string()),
                StreamEvent::Done,
                StreamEvent::Done,
            ]
        );

        // 报错（对象与字符串两种形态都要认）。
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.feed("data: {\"error\":{\"message\":\"余额不足\"}}\n\n".as_bytes(), &mut out);
        assert_eq!(out, vec![StreamEvent::Error("余额不足".to_string())]);
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.feed(b"data: {\"error\":\"no such model\"}\n\n", &mut out);
        assert_eq!(out, vec![StreamEvent::Error("no such model".to_string())]);

        // 认不出的行整行丢掉（绝不把噪声当错误）。
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.feed(b"data: not-json\n\nevent: ping\n\n", &mut out);
        assert!(out.is_empty(), "噪声不能变成报错：{out:?}");
    }

    #[test]
    fn sse_flush_handles_trailing_line_without_newline() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        p.feed(
            "data: {\"choices\":[{\"delta\":{\"content\":\"末\"}}]}".as_bytes(),
            &mut out,
        );
        assert!(out.is_empty(), "没换行就先攒着");
        p.flush(&mut out);
        assert_eq!(out, vec![StreamEvent::Delta("末".to_string())]);
    }

    #[test]
    fn non_stream_response_is_read_as_a_whole_json() {
        // 关掉流式时回的是整段 JSON（不是 SSE），走 `content_of`。
        let body = "{\"choices\":[{\"message\":{\"content\":\"整段回来\"},\"finish_reason\":\"stop\"}]}";
        assert_eq!(content_of(body).expect("取到内容"), "整段回来");
        // 报错也认。
        assert!(content_of("{\"error\":{\"message\":\"余额不足\"}}").is_err());
        // 空回包不 panic，给一句人话。
        assert!(content_of("{}").is_err());
        assert!(content_of("not json").is_err());
    }

    #[test]
    fn user_prompt_puts_question_last_and_clips_context() {
        let a = Ask {
            book: "莊子".to_string(),
            chapter: "逍遙遊".to_string(),
            quote: "北冥有魚".to_string(),
            before: "甲".repeat(500),
            after: "乙".repeat(500),
            question: String::new(),
        };
        let s = user_prompt(&a, 100);
        assert!(s.starts_with("【出处】"), "首行报出处：{s}");
        assert!(s.contains("莊子·逍遙遊"));
        assert!(s.contains("【原文】北冥有魚"));
        // 问题压在最末，且空问题时用默认问法。
        assert!(s.ends_with(&format!("【问题】{DEFAULT_QUESTION}")));
        // 上下文被裁到预算内（每侧 50 字 + 省略号）。
        let before_line = s.lines().find(|l| l.contains("前文")).unwrap_or("");
        assert!(before_line.chars().count() <= 60, "前文没裁：{before_line}");
        assert!(before_line.contains('…'));
    }

    #[test]
    fn clip_keeps_the_side_nearest_the_quote() {
        let s = "一二三四五六七八九十";
        assert_eq!(clip_tail(s, 3), "…八九十");
        assert_eq!(clip_head(s, 3), "一二三…");
        // 没超限就原样（不加省略号）。
        assert_eq!(clip_tail(s, 20), s);
        assert_eq!(clip_head(s, 20), s);
    }

    #[test]
    fn body_carries_model_stream_and_messages() {
        let mut c = AiConfig::default();
        c.model = "deepseek-chat".to_string();
        let body = chat_body(&c, DEFAULT_SYSTEM, "【問題】何謂道", true);
        let v: Value = serde_json::from_str(&body).expect("请求体应是合法 JSON");
        assert_eq!(v["model"], "deepseek-chat");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(v["messages"][1]["content"], "【問題】何謂道");
        // 温度越界要夹住，不能把 9.9 这种值送出去。
        c.temperature = 9.9;
        let v: Value = serde_json::from_str(&chat_body(&c, "", "", false)).expect("JSON");
        assert_eq!(v["temperature"], 2.0);
        assert_eq!(v["stream"], false);
    }

    #[test]
    fn tidy_answer_strips_leading_blank_lines() {
        assert_eq!(tidy_answer("\n\n 道可道。\n"), "道可道。");
        assert_eq!(tidy_answer("﻿可"), "可");
    }
}
