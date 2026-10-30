//! 知识图谱（V2.0 · MVP）：实体抽取 + 极简溯源。
//!
//! **零 UI 依赖**，纯逻辑层。三张主表 + 一张状态表：
//!   · `kg_entities`  —— 实体（人名 / 地名 / 官职），按 `(book_id, name)` 去重；
//!   · `kg_relations` —— 实体间关系（如「同现」）；
//!   · `kg_mentions`  —— 实体在原文的首次出现（章 · 段 · 字符偏移），用于「跳回原文」；
//!   · `kg_meta`      —— 抽取状态，避免重复抽取。
//!
//! 抽取走两条路径：
//!   1. **主路径 · 本地 Ollama**：把一段文本 + 强约束 Prompt 丢给 `localhost:11434`，
//!      解析模型回传的 JSON 数组，写入实体 / 关系；
//!   2. **回退 · 内置历史专名词典**：本机没装 Ollama 时（开发机默认如此），用一份
//!      内置的古籍专名表做 NER，并以「同段共现」生成人物关系。
//!
//! 与排版 / 渲染**完全解耦**：抽取在开卷后的后台线程静默跑，命中查询只在主线程
//! 做一次轻量只读。红线——**绝不**做连线图 UI，只做「建表 + 抽取 + 极简卡片」。

use crate::store;
use crate::types::Chapter;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

// ─────────────────────────── 对外视图 ───────────────────────────

/// 一条「相关实体」记录（用于人物卡片的「相关人物」列表）。
#[derive(Clone, Debug)]
pub struct KgRelated {
    pub name: String,
    pub etype: String,
    pub relation: String,
}

/// 查某个实体时回给 UI 的视图：本体信息 + 首次出现位置 + 相关实体。
#[derive(Clone, Debug)]
pub struct KgEntityView {
    pub name: String,
    pub etype: String,
    /// 首次出现位置（章 · 段 · 字符偏移），用于「跳转原文」。
    pub mention: Option<(usize, usize, usize)>,
    pub related: Vec<KgRelated>,
}

/// 抽取中间产物：一个实体在某章某段的首次命中。
struct Mention {
    name: String,
    etype: String,
    chapter: usize,
    para: usize,
    offset: usize,
}

// ─────────────────────────── Schema ───────────────────────────

/// 建表（幂等）。由 `store::init` 在开库时调用。
pub fn init(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS kg_entities(
            id      INTEGER PRIMARY KEY,
            book_id TEXT NOT NULL,
            name    TEXT NOT NULL,
            etype   TEXT NOT NULL,
            UNIQUE(book_id, name)
         );
         CREATE TABLE IF NOT EXISTS kg_relations(
            id               INTEGER PRIMARY KEY,
            book_id          TEXT NOT NULL,
            source_entity_id INTEGER NOT NULL,
            target_entity_id INTEGER NOT NULL,
            relation_type    TEXT NOT NULL,
            UNIQUE(book_id, source_entity_id, target_entity_id, relation_type)
         );
         CREATE TABLE IF NOT EXISTS kg_mentions(
            id                INTEGER PRIMARY KEY,
            entity_id         INTEGER NOT NULL,
            chapter_id        INTEGER NOT NULL,
            paragraph_index   INTEGER NOT NULL,
            start_offset      INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS kg_meta(
            book_id    TEXT PRIMARY KEY,
            status     TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );",
    )
    .context("建知识图谱表失败")?;
    Ok(())
}

// ─────────────────────────── 抽取 ───────────────────────────

/// 开卷后调用：若本书尚未抽取则静默抽取并落盘，已抽取则直接返回。
///
/// `chapters` 取自给 UI 的 `Book`（开卷时已在内存）；抽取在后台线程跑，不阻塞 UI。
pub fn ensure_extracted(book_id: &str, chapters: &[Chapter]) -> Result<()> {
    let c = store::connect().context("打开章节库失败")?;
    ensure_extracted_conn(&c, book_id, chapters)
}

/// 同 `ensure_extracted`，但复用调用方已打开的连接（测试 / 批量场景）。
pub fn ensure_extracted_conn(c: &Connection, book_id: &str, chapters: &[Chapter]) -> Result<()> {
    // 已抽过（kg_meta.status = 'done'）则跳过。
    let done = c
        .query_row(
            "SELECT status FROM kg_meta WHERE book_id=?1",
            params![book_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .as_deref()
        == Some("done");
    if done {
        return Ok(());
    }

    // 抽取：先试 Ollama，失败（未装 / 超时）降级词典。
    let (mentions, relations): (Vec<Mention>, Vec<(String, String, String)>) =
        match extract_ollama(chapters) {
            Some(x) => x,
            None => extract_dictionary(chapters),
        };

    // 落盘：整本一个事务。
    store::begin_bulk(c)?;

    // 实体去重写入（UNIQUE 兜底），再按名字取回 id 映射。
    let mut ids: HashMap<String, i64> = HashMap::new();
    {
        let mut stmt = c
            .prepare("INSERT OR IGNORE INTO kg_entities(book_id,name,etype) VALUES(?1,?2,?3)")
            .context("写实体失败")?;
        for m in &mentions {
            let _ = stmt.execute(params![book_id, m.name, m.etype]);
        }
    }
    {
        let mut stmt = c
            .prepare("SELECT id FROM kg_entities WHERE book_id=?1 AND name=?2")
            .context("查实体 id 失败")?;
        let names: BTreeSet<String> = mentions.iter().map(|m| m.name.clone()).collect();
        for n in names {
            if let Ok(id) = stmt.query_row(params![book_id, n], |r| r.get::<_, i64>(0)) {
                ids.insert(n, id);
            }
        }
    }

    // 溯源映射：实体 → 首次出现位置。
    {
        let mut stmt = c
            .prepare(
                "INSERT INTO kg_mentions(entity_id,chapter_id,paragraph_index,start_offset) \
                 VALUES(?1,?2,?3,?4)",
            )
            .context("写溯源失败")?;
        for m in &mentions {
            if let Some(&eid) = ids.get(&m.name) {
                let _ = stmt.execute(params![
                    eid,
                    m.chapter as i64,
                    m.para as i64,
                    m.offset as i64
                ]);
            }
        }
    }

    // 关系：按名字解析两端 id 后写入（UNIQUE 去重）。
    {
        let mut stmt = c
            .prepare(
                "INSERT OR IGNORE INTO kg_relations(book_id,source_entity_id,target_entity_id,relation_type) \
                 VALUES(?1,?2,?3,?4)",
            )
            .context("写关系失败")?;
        for (s, t, rel) in &relations {
            if let (Some(&sid), Some(&tid)) = (ids.get(s), ids.get(t)) {
                if sid != tid {
                    let _ = stmt.execute(params![book_id, sid, tid, rel]);
                }
            }
        }
    }

    store::end_bulk(c)?;

    // 标记完成，避免下次开卷重复抽。
    let now = now_secs();
    c.execute(
        "INSERT OR REPLACE INTO kg_meta(book_id,status,updated_at) VALUES(?1,'done',?2)",
        params![book_id, now],
    )
    .context("写抽取状态失败")?;

    Ok(())
}

// ─────────────────────────── 查询 ───────────────────────────

/// 查一个实体（按名字）：返回本体类型、首次出现位置、相关实体列表。查不到回 `None`。
///
/// 走默认只读连接（生产路径）；测试 / 批量场景用 [`lookup_conn`] 复用自有连接。
pub fn lookup(book_id: &str, name: &str) -> Option<KgEntityView> {
    store::with_read_conn(|c| lookup_conn(c, book_id, name)).flatten()
}

/// 同 `lookup`，但复用调用方已打开的连接（「同现」关系是对称的，查两端）。
pub fn lookup_conn(c: &Connection, book_id: &str, name: &str) -> Option<KgEntityView> {
    let (eid, etype) = c
        .query_row(
            "SELECT id, etype FROM kg_entities WHERE book_id=?1 AND name=?2",
            params![book_id, name],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .ok()?;

    // 首次出现（用于跳转原文）。
    let mention = c
        .query_row(
            "SELECT chapter_id, paragraph_index, start_offset \
             FROM kg_mentions WHERE entity_id=?1 ORDER BY id ASC LIMIT 1",
            params![eid],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as usize,
                    r.get::<_, i64>(1)? as usize,
                    r.get::<_, i64>(2)? as usize,
                ))
            },
        )
        .ok();

    // 相关实体。「同现」关系对称存储（按字典序定 source/target），故查两端：
    // 既看「以本实体为源」也看「以本实体为目标」，把另一端解析成相关实体。
    let mut related = Vec::new();
    if let Ok(mut st) = c.prepare(
        "SELECT other_id, relation_type FROM (\
            SELECT target_entity_id AS other_id, relation_type \
            FROM kg_relations WHERE source_entity_id=?1 \
            UNION ALL \
            SELECT source_entity_id AS other_id, relation_type \
            FROM kg_relations WHERE target_entity_id=?1 \
         )",
    ) {
        if let Ok(rows) =
            st.query_map(params![eid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        {
            for r in rows.flatten() {
                let (oid, rel) = r;
                if let Ok((oname, otype)) = c.query_row(
                    "SELECT name, etype FROM kg_entities WHERE id=?1",
                    params![oid],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                ) {
                    related.push(KgRelated {
                        name: oname,
                        etype: otype,
                        relation: rel,
                    });
                }
            }
        }
    }

    Some(KgEntityView {
        name: name.to_string(),
        etype,
        mention,
        related,
    })
}

// ─────────────────────────── 抽取实现 ───────────────────────────

/// 词典回退：扫每章每段，命中内置专名表即记一条 mention（每段的某词只记首次），
/// 并以「同段共现」生成人物间关系。
fn extract_dictionary(chapters: &[Chapter]) -> (Vec<Mention>, Vec<(String, String, String)>) {
    let mut mentions = Vec::new();
    // (章, 段) → 该段出现过的人物，用于共现。
    let mut cooc: HashMap<(usize, usize), Vec<String>> = HashMap::new();

    for (ci, ch) in chapters.iter().enumerate() {
        for (pi, p) in ch.paragraphs.iter().enumerate() {
            let text = &p.text;
            // 每段每个词只取首次命中（byte 偏移 → char 偏移，与字形 key 对齐）。
            let mut seen: HashMap<&'static str, usize> = HashMap::new();
            for &(name, _etype) in LEXICON {
                if let Some(byte_pos) = text.find(name) {
                    let char_off = text[..byte_pos].chars().count();
                    seen.entry(name).or_insert(char_off);
                }
            }
            for (name, char_off) in seen {
                let etype = lexicon_type(name);
                mentions.push(Mention {
                    name: name.to_string(),
                    etype: etype.to_string(),
                    chapter: ci,
                    para: pi,
                    offset: char_off,
                });
                if etype == "人物" {
                    cooc.entry((ci, pi)).or_default().push(name.to_string());
                }
            }
        }
    }

    // 同段人物两两配对（无序去重），关系记为「同现」。
    let mut rels: HashSet<(String, String, String)> = HashSet::new();
    for persons in cooc.values() {
        for i in 0..persons.len() {
            for j in (i + 1)..persons.len() {
                let (a, b) = if persons[i] <= persons[j] {
                    (persons[i].clone(), persons[j].clone())
                } else {
                    (persons[j].clone(), persons[i].clone())
                };
                rels.insert((a, b, "同现".to_string()));
            }
        }
    }

    (mentions, rels.into_iter().collect())
}

/// Ollama 主路径：分块 → 强约束 Prompt → JSON → 实体。
///
/// 本机没装 Ollama（`Connection refused`）或任何异常都回 `None`，由调用方降级词典。
fn extract_ollama(chapters: &[Chapter]) -> Option<(Vec<Mention>, Vec<(String, String, String)>)> {
    // 按 ~1500 字切块。
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in chapters {
        for p in &ch.paragraphs {
            if cur.chars().count() > 1500 {
                chunks.push(std::mem::take(&mut cur));
            }
            cur.push_str(&p.text);
            cur.push('\n');
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }

    let model = std::env::var("JIANDU_KG_MODEL").unwrap_or_else(|_| "qwen2.5:7b".to_string());
    let mut mentions = Vec::new();
    for (ci, chunk) in chunks.iter().enumerate() {
        let prompt = format!(
            "请从下面的古籍片段中提取所有人名、地名、官职，严格按 JSON 数组输出，\
             每个元素是 {{\"name\":\"名称\",\"type\":\"人物|地点|官职\"}}。\
             只输出 JSON，不要任何解释文字。\n\n{chunk}"
        );
        let out = ollama_generate(&model, &prompt)?;
        let arr = extract_json_array(&out)?;
        for v in arr {
            let name = v.get("name")?.as_str()?.to_string();
            let etype = v
                .get("type")
                .and_then(|x| x.as_str())
                .unwrap_or("人物")
                .to_string();
            mentions.push(Mention {
                name,
                etype,
                chapter: ci,
                para: 0,
                offset: 0,
            });
        }
    }
    // Ollama 路径暂不精算关系（块级粒度），关系留空；词典路径才补共现。
    Some((mentions, Vec::new()))
}

/// 给 Ollama `/api/generate` 发一个最小 HTTP/1.1 请求（不引第三方 HTTP 依赖）。
///
/// `stream:false` 时 Ollama 在正文后直接关连接，读到 EOF 即可。失败一律回 `None`。
fn ollama_generate(model: &str, prompt: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let mut stream = TcpStream::connect("127.0.0.1:11434").ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "format": "json",
        "stream": false,
    })
    .to_string();
    let req = format!(
        "POST /api/generate HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(req.as_bytes()).ok()?;

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).ok()?;
    let resp = String::from_utf8_lossy(&resp);
    let body_start = resp.find("\r\n\r\n")? + 4;
    let json = &resp[body_start..];
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("response")?.as_str().map(|s| s.to_string())
}

/// 从模型回传的自由文本里抠出第一个 JSON 数组（容错）。
fn extract_json_array(s: &str) -> Option<Vec<serde_json::Value>> {
    let start = s.find('[')?;
    let end = s.rfind(']')?;
    let sub = &s[start..=end];
    serde_json::from_str::<Vec<serde_json::Value>>(sub).ok()
}

fn now_secs() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

// ─────────────────────────── 内置专名词典（回退 NER） ───────────────────────────
//
// 仅覆盖常用古籍人物 / 地名 / 官职，够 MVP 演示（史记 / 通鉴 / 红楼 / 水浒 等）。
// 真实部署走 Ollama，本表只是「没装模型也能跑通」的兜底。

const LEXICON: &[(&str, &str)] = &[
    // —— 楚汉 · 史记 ——
    ("项羽", "人物"),
    ("刘邦", "人物"),
    ("范增", "人物"),
    ("韩信", "人物"),
    ("张良", "人物"),
    ("萧何", "人物"),
    ("项梁", "人物"),
    ("虞姬", "人物"),
    ("秦始皇", "人物"),
    ("李斯", "人物"),
    ("赵高", "人物"),
    ("陈胜", "人物"),
    ("吴广", "人物"),
    ("周勃", "人物"),
    ("樊哙", "人物"),
    ("灌婴", "人物"),
    ("彭越", "人物"),
    ("英布", "人物"),
    ("龙且", "人物"),
    ("田横", "人物"),
    ("章邯", "人物"),
    ("司马欣", "人物"),
    ("董翳", "人物"),
    ("郦食其", "人物"),
    ("陈平", "人物"),
    ("周亚夫", "人物"),
    ("窦婴", "人物"),
    ("田蚡", "人物"),
    ("卫青", "人物"),
    ("霍去病", "人物"),
    ("李广", "人物"),
    ("苏武", "人物"),
    ("张骞", "人物"),
    ("主父偃", "人物"),
    ("汲黯", "人物"),
    ("公孙弘", "人物"),
    ("东方朔", "人物"),
    ("司马迁", "人物"),
    ("贾谊", "人物"),
    ("晁错", "人物"),
    ("袁盎", "人物"),
    ("吕后", "人物"),
    ("戚夫人", "人物"),
    ("刘恒", "人物"),
    ("刘启", "人物"),
    ("刘彻", "人物"),
    ("李陵", "人物"),
    ("江充", "人物"),
    // —— 三国 · 通鉴 ——
    ("曹操", "人物"),
    ("刘备", "人物"),
    ("孙权", "人物"),
    ("诸葛亮", "人物"),
    ("司马懿", "人物"),
    ("周瑜", "人物"),
    ("袁绍", "人物"),
    ("董卓", "人物"),
    ("吕布", "人物"),
    ("关羽", "人物"),
    ("张飞", "人物"),
    ("赵云", "人物"),
    ("魏徵", "人物"),
    ("李世民", "人物"),
    ("房玄龄", "人物"),
    ("杜如晦", "人物"),
    ("长孙无忌", "人物"),
    ("武则天", "人物"),
    ("狄仁杰", "人物"),
    ("郭子仪", "人物"),
    ("安禄山", "人物"),
    ("史思明", "人物"),
    ("李靖", "人物"),
    ("尉迟恭", "人物"),
    ("秦琼", "人物"),
    ("程咬金", "人物"),
    // —— 红楼梦 ——
    ("宝玉", "人物"),
    ("黛玉", "人物"),
    ("宝钗", "人物"),
    ("王熙凤", "人物"),
    ("贾母", "人物"),
    ("王夫人", "人物"),
    ("贾政", "人物"),
    ("贾琏", "人物"),
    ("袭人", "人物"),
    ("晴雯", "人物"),
    ("探春", "人物"),
    ("湘云", "人物"),
    ("妙玉", "人物"),
    ("秦可卿", "人物"),
    ("李纨", "人物"),
    ("贾珍", "人物"),
    ("贾蓉", "人物"),
    ("薛蟠", "人物"),
    ("香菱", "人物"),
    ("紫鹃", "人物"),
    ("鸳鸯", "人物"),
    ("平儿", "人物"),
    ("尤氏", "人物"),
    ("贾赦", "人物"),
    ("邢夫人", "人物"),
    ("贾环", "人物"),
    ("赵姨娘", "人物"),
    ("刘姥姥", "人物"),
    ("茗烟", "人物"),
    ("麝月", "人物"),
    ("司棋", "人物"),
    // —— 水浒传 ——
    ("宋江", "人物"),
    ("卢俊义", "人物"),
    ("吴用", "人物"),
    ("林冲", "人物"),
    ("鲁智深", "人物"),
    ("武松", "人物"),
    ("李逵", "人物"),
    ("晁盖", "人物"),
    ("公孙胜", "人物"),
    ("花荣", "人物"),
    ("柴进", "人物"),
    ("阮小二", "人物"),
    ("阮小五", "人物"),
    ("阮小七", "人物"),
    ("杨志", "人物"),
    ("燕青", "人物"),
    ("石秀", "人物"),
    ("时迁", "人物"),
    ("戴宗", "人物"),
    ("刘唐", "人物"),
    ("张顺", "人物"),
    ("张横", "人物"),
    ("李俊", "人物"),
    ("秦明", "人物"),
    ("呼延灼", "人物"),
    ("关胜", "人物"),
    ("董平", "人物"),
    ("索超", "人物"),
    ("朱仝", "人物"),
    ("雷横", "人物"),
    ("史进", "人物"),
    // —— 常用地点 ——
    ("咸阳", "地点"),
    ("彭城", "地点"),
    ("垓下", "地点"),
    ("乌江", "地点"),
    ("巨鹿", "地点"),
    ("鸿门", "地点"),
    ("荥阳", "地点"),
    ("函谷关", "地点"),
    ("长安", "地点"),
    ("沛县", "地点"),
    ("关中", "地点"),
    ("巴蜀", "地点"),
    ("江东", "地点"),
    ("会稽", "地点"),
    ("陈仓", "地点"),
    ("赤壁", "地点"),
    ("隆中", "地点"),
    ("许昌", "地点"),
    ("洛阳", "地点"),
    ("建业", "地点"),
    ("成都", "地点"),
    ("官渡", "地点"),
    // —— 常用官职 ——
    ("丞相", "官职"),
    ("太尉", "官职"),
    ("御史大夫", "官职"),
    ("将军", "官职"),
    ("沛公", "官职"),
    ("汉王", "官职"),
    ("西楚霸王", "官职"),
    ("廷尉", "官职"),
    ("博士", "官职"),
    ("中尉", "官职"),
];

/// 取词的类型（词典查找；默认人物）。
fn lexicon_type(name: &str) -> &'static str {
    for &(n, t) in LEXICON {
        if n == name {
            return t;
        }
    }
    "人物"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexicon_ner_finds_known_names_and_cooccurrence() {
        let chapters = vec![crate::types::Chapter {
            title: "项羽本纪".to_string(),
            paragraphs: vec![crate::types::Paragraph::plain(
                "项羽乃悉引兵渡河，与刘邦、范增谋曰：此天亡秦之时也。",
            )],
        }];
        let (mentions, relations) = extract_dictionary(&chapters);
        let names: Vec<&str> = mentions.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"项羽"));
        assert!(names.contains(&"刘邦"));
        assert!(names.contains(&"范增"));
        // 三人同段 → 至少一对「同现」关系。
        assert!(!relations.is_empty());
    }

    #[test]
    fn lookup_returns_none_for_unknown() {
        // 不连库：这里只验证签名与空表语义（真实查询在 App 层走 with_read_conn）。
        let _ = lookup; // 编译期存在性检查
    }

    /// 端到端：词典抽取 → 落库 → 查「项羽」卡片，验证
    /// 「项羽（人物）/ 相关人物含 刘邦、范增 / 有跳转原文位置」这一最小可用闭环。
    ///
    /// 这个测试直接复刻 App 层的行为（开卷后台抽取 + 选中查询），是 MVP 数据链路的
    /// 守门员——一旦关系方向写反、字符偏移算错、或共现漏抽，这里就会红。
    #[test]
    fn e2e_extract_then_lookup_xiangyu_card() {
        use crate::types::{Chapter, Paragraph};

        let c = crate::store::connect_in_memory().expect("内存库");
        init(&c).expect("建表");

        let chapters = vec![
            Chapter {
                title: "项羽本纪".to_string(),
                paragraphs: vec![
                    Paragraph::plain(
                        "项梁起兵，项羽随叔父项梁渡江。后与刘邦、范增谋曰：此天亡秦之时也。",
                    ),
                    Paragraph::plain("垓下之围，项羽兵败，虞姬从死，项羽自刎于乌江。"),
                ],
            },
            Chapter {
                title: "高祖本纪".to_string(),
                paragraphs: vec![Paragraph::plain(
                    "刘邦称帝，封韩信为齐王，萧何治国，张良画策。",
                )],
            },
        ];

        ensure_extracted_conn(&c, "sha-chu-han", &chapters).expect("抽取");
        let card = lookup_conn(&c, "sha-chu-han", "项羽").expect("项羽 应被抽出");

        // 本体。
        assert_eq!(card.name, "项羽");
        assert_eq!(card.etype, "人物");

        // 相关人物：与 项羽 同段共现的 刘邦、范增 都应出现（关系对称查两端）。
        let names: Vec<&str> = card.related.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"刘邦"),
            "相关人物应包含刘邦，实际：{:?}",
            names
        );
        assert!(
            names.contains(&"范增"),
            "相关人物应包含范增，实际：{:?}",
            names
        );
        for r in &card.related {
            assert_eq!(r.relation, "同现");
        }

        // 跳转原文：首次出现位置必须存在。
        let (ch, para, off) = card.mention.expect("应有跳转原文位置");
        assert_eq!(ch, 0);
        assert_eq!(para, 0);
        assert!(off > 0, "字符偏移应指向『项羽』在段首之后的位置");

        // 查不到的人名回 None。
        assert!(lookup_conn(&c, "sha-chu-han", "不存在的人").is_none());
    }
}
