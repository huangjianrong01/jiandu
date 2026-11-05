//! **卡片匣**（v1.74.0）：跨书摘录卡的落盘层。
//!
//! 与「朱批」的区别：朱批是**书内**的批注（跟着 `Book` 一起序列化进
//! `books.meta_json`），卡片是**跨书**的摘录——读《庄子》摘一句、读《史记》
//! 摘一句，全都汇进同一个匣子，且**书删了卡片还在**（书名 / 章名 / 原文
//! 都做了快照，不依赖原书是否还在架上）。
//!
//! 表结构与 `store` 的章节库**同一个文件**（`chapters.db`）：卡片是低频、
//! 单条的小数据，没必要再开一个库多一条连接；`CREATE TABLE IF NOT EXISTS`
//! 保证老用户升级后表自动补上，零迁移成本。
//!
//! ⚠️ 定位用 `(章, 段, 段内字偏移)`——与眉批 `HeadNote` / 朱批 `Note` 是
//! **同一把尺子**（`GlyphKey`），这样点一条卡片才能复用现成的
//! `jump_to_key` 跳回原文。清洗或重导会让偏移漂移，所以**必须同时存
//! `quote` 原文快照**：坐标对不上时至少内容还在（卡片不会变白板）。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// 卡片来源。
///
/// v1.74.0 只有 `Quote`（选中原文入匣）；`Ai` 是模块二「问小笺」的落点；
/// `Dictionary` 是「生词本」——点【查典】成功取义后静默收录的查词卡；
/// `Underline` 是竖排正文右侧的「划线」朱线，绑定 Char Index 区间。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardKind {
    /// 摘录：用户框选的原文。
    Quote = 0,
    /// 问笺：AI 的回答（模块二接入）。
    Ai = 1,
    /// 查词卡片（生词本）：点【查典】成功取义后自动收录，与朱批 / 笔记区分开。
    Dictionary = 2,
    /// 划线（右侧朱线）：竖排正文右侧的前景朱线，绑定 Char Index 区间，与
    /// 朱批（背景半透明红底）彻底解耦。
    Underline = 3,
}

impl CardKind {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => CardKind::Ai,
            2 => CardKind::Dictionary,
            3 => CardKind::Underline,
            _ => CardKind::Quote,
        }
    }
    /// 卡片列表上的一枚小标签（单字，朱砂）。
    pub fn label(self) -> &'static str {
        match self {
            CardKind::Quote => "摘",
            CardKind::Ai => "笺",
            CardKind::Dictionary => "典",
            CardKind::Underline => "线",
        }
    }
}

/// 一枚卡片。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Card {
    /// 库内自增主键（删除用）。
    pub id: i64,
    /// 出处：书名快照（书删了也认得出是哪部书）。
    pub book: String,
    /// 出处：章名快照。
    pub chapter: String,
    /// 原书在章节库里的 id；还在架上时才能跳回。
    pub store_id: String,
    /// 定位锚点 `(章, 段, 段内字偏移)`，与 `GlyphKey` 同款。
    pub key: (usize, usize, usize),
    /// 选区长度（字）：跳回后点亮整段（`flash_note`），与朱批录同款。
    pub len: usize,
    /// 原文快照。
    pub quote: String,
    /// 按语（v1 为空；模块二由 AI 回答填入）。
    pub note: String,
    pub kind: CardKind,
    /// 入匣时刻（毫秒）。列表按它倒序——最新入匣的在最上。
    pub created_ms: i64,
    /// 拼音（查词卡用：生词本「读音」一栏；其余类型空）。
    pub pinyin: String,
    /// 查阅次数（查词卡去重后的热度，生词本默认按它降序）。
    pub view_count: i64,
    /// 最近查阅时间（毫秒）：去重命中时刷新。
    pub last_viewed_ms: i64,
    /// 是否已「加入重点」（查词卡一键转正为核心复习卡）。
    pub starred: bool,
    /// 线型（划线卡用：0 实线 / 1 波浪线，波浪线作专名号）。
    pub style: i64,
}

impl Card {
    /// 出处行：`书名·章名`（章名为空就只写书名）。
    pub fn source(&self) -> String {
        if self.chapter.is_empty() {
            self.book.clone()
        } else {
            format!("{}·{}", self.book, self.chapter)
        }
    }
}

/// 建表（`store::init` 里调一次；`IF NOT EXISTS` 保证老库自动补表）。
///
/// v1.9.89 起新增五列：`pinyin` / `view_count` / `last_viewed_ms` / `starred`
/// / `style`——分别服务「生词本」（拼音 / 查阅次数 / 最近查阅 / 星标）与
/// 「划线」（线型）。老库（v1.9.88 及更早）表里没有这些列，`init` 末尾的
/// `ensure_column` 会逐列 `ALTER TABLE ADD COLUMN` 补上，零迁移成本。
pub fn init(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS cards(
             id          INTEGER PRIMARY KEY AUTOINCREMENT,
             store_id    TEXT NOT NULL,
             book        TEXT NOT NULL,
             chapter     TEXT NOT NULL,
             chapter_idx INTEGER NOT NULL,
             para_idx    INTEGER NOT NULL,
             char_off    INTEGER NOT NULL,
             len         INTEGER NOT NULL,
             quote       TEXT NOT NULL,
             note        TEXT NOT NULL,
             kind        INTEGER NOT NULL,
             created_ms  INTEGER NOT NULL,
             pinyin      TEXT NOT NULL DEFAULT '',
             view_count  INTEGER NOT NULL DEFAULT 0,
             last_viewed_ms INTEGER NOT NULL DEFAULT 0,
             starred     INTEGER NOT NULL DEFAULT 0,
             style       INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_cards_time ON cards(created_ms DESC);
         CREATE INDEX IF NOT EXISTS idx_cards_book ON cards(store_id);
         CREATE INDEX IF NOT EXISTS idx_cards_kind ON cards(kind);",
    )
    .context("建卡片匣表失败")?;
    // 老库（v1.9.88 之前）没有新列，逐列补齐——只补缺失的，已有则跳过。
    ensure_column(c, "cards", "pinyin", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(c, "cards", "view_count", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(c, "cards", "last_viewed_ms", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(c, "cards", "starred", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(c, "cards", "style", "INTEGER NOT NULL DEFAULT 0")?;
    Ok(())
}

/// 老库补列：若 `table` 尚无 `col`，就 `ALTER TABLE ADD COLUMN`。
///
/// SQLite 不支持「加列若存在则跳过」的语法，故先 `PRAGMA table_info`
/// 探一遍，避免重复加列报错。
fn ensure_column(c: &Connection, table: &str, col: &str, def: &str) -> Result<()> {
    let mut st = c
        .prepare(&format!("PRAGMA table_info({table})"))
        .context("探表结构失败")?;
    let mut found = false;
    let rows = st
        .query_map([], |r| r.get::<_, String>(1))
        .context("读表结构失败")?;
    for r in rows {
        if r? == col {
            found = true;
            break;
        }
    }
    if !found {
        c.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {col} {def}"),
            [],
        )
        .with_context(|| format!("补列 {col} 失败"))?;
    }
    Ok(())
}

/// 当前毫秒（落库时间戳用；core 不依赖任何平台时钟 API）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 入匣一枚卡片，返回它的 `id`。
///
/// 单条写入、低频（一次点击一条），故**不开批量事务**——`begin_bulk` 是给
/// 导入千章用的，这里用反而要平白多一次 fsync 的账。
pub fn put_card(c: &Connection, card: &Card) -> Result<i64> {
    c.execute(
        "INSERT INTO cards(store_id,book,chapter,chapter_idx,para_idx,char_off,
                           len,quote,note,kind,created_ms,
                           pinyin,view_count,last_viewed_ms,starred,style)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            card.store_id,
            card.book,
            card.chapter,
            card.key.0 as i64,
            card.key.1 as i64,
            card.key.2 as i64,
            card.len as i64,
            card.quote,
            card.note,
            card.kind as i64,
            card.created_ms,
            card.pinyin,
            card.view_count,
            card.last_viewed_ms,
            if card.starred { 1i64 } else { 0i64 },
            card.style,
        ],
    )
    .context("入匣失败")?;
    Ok(c.last_insert_rowid())
}

/// 列出卡片：**最新入匣的在前**。
///
/// `limit` 是硬上限（UI 传一个大数即可）。卡片多了将来要分页 / 检索，
/// 但那是模块一 FTS5 的事。
pub fn list_cards(c: &Connection, limit: usize) -> Result<Vec<Card>> {
    let mut st = c.prepare(
        "SELECT id,store_id,book,chapter,chapter_idx,para_idx,char_off,
                len,quote,note,kind,created_ms,
                pinyin,view_count,last_viewed_ms,starred,style
         FROM cards ORDER BY created_ms DESC, id DESC LIMIT ?1",
    )?;
    let rows = st.query_map(params![limit as i64], |r| {
        Ok(Card {
            id: r.get(0)?,
            store_id: r.get(1)?,
            book: r.get(2)?,
            chapter: r.get(3)?,
            key: (r.get::<_, i64>(4)? as usize, r.get::<_, i64>(5)? as usize, r.get::<_, i64>(6)? as usize),
            len: r.get::<_, i64>(7)? as usize,
            quote: r.get(8)?,
            note: r.get(9)?,
            kind: CardKind::from_i64(r.get(10)?),
            created_ms: r.get(11)?,
            pinyin: r.get(12)?,
            view_count: r.get(13)?,
            last_viewed_ms: r.get(14)?,
            starred: r.get::<_, i64>(15)? != 0,
            style: r.get(16)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.context("读卡片失败")?);
    }
    Ok(out)
}

/// 丢弃一枚卡片（按 `id`）。
pub fn delete_card(c: &Connection, id: i64) -> Result<()> {
    c.execute("DELETE FROM cards WHERE id=?1", params![id])
        .context("删除卡片失败")?;
    Ok(())
}

/// 匣中卡片总数（面板表头用）。
pub fn count_cards(c: &Connection) -> Result<usize> {
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0))
        .context("统计卡片失败")?;
    Ok(n as usize)
}

/// 清空（书架「清空全部」时用得着）。
pub fn clear_cards(c: &Connection) -> Result<()> {
    c.execute_batch("DELETE FROM cards").context("清空卡片匣失败")
}

/// 卡片正文的**去重键**：同一本书、同一处、同一段原文算重复。
///
/// 手滑连点两次「存卡」不该在匣里躺两张一模一样的卡。
pub fn same_place(a: &Card, b: &Card) -> bool {
    a.store_id == b.store_id && a.key == b.key && a.quote == b.quote
}

/// **查词卡去重落库（生词本）**：同一「词条」(`quote`) 只留一张卡。
///
/// * 已存在 → 查阅次数 `+1`、刷新最近查阅时间，并把出处更新为最近一次查的
///   位置（书 / 章 / 锚点）——同一字在不同书里查，生词本只留一条，但记住
///   你最近是在哪儿查的。
/// * 不存在 → 新建，记下原词 / 拼音 / 释义 / 出处，`view_count` 起手为 1。
///
/// ⚠️ 与 `save_selection_card` 的「位置去重」(`same_place`) 区分开：生词本
/// 按「词」去重，不是按「位置」——否则同一字每出现一次就刷一条，污染匣子。
pub fn upsert_dict_card(
    c: &Connection,
    store_id: &str,
    book: &str,
    chapter: &str,
    key: (usize, usize, usize),
    word: &str,
    pinyin: &str,
    gloss: &str,
) -> Result<i64> {
    let existing: Option<i64> = c
        .query_row(
            "SELECT id FROM cards WHERE kind=?1 AND quote=?2 LIMIT 1",
            params![CardKind::Dictionary as i64, word],
            |r| r.get(0),
        )
        .ok();
    let now = now_ms();
    if let Some(id) = existing {
        // 重复词条：**只**累加查阅次数、刷新最近查阅时间（用户需求 #2）。
        // 释义 / 出处（首次落库时记录的书·章）一律不动——同一字在别处再查，
        // 不该把原来「在《庄子》查的」改写成「在《列子》查的」，否则出处失真。
        c.execute(
            "UPDATE cards SET view_count=view_count+1, last_viewed_ms=?1 WHERE id=?2",
            params![now, id],
        )
        .context("更新生词本失败")?;
        Ok(id)
    } else {
        let card = Card {
            id: 0,
            book: book.to_string(),
            chapter: chapter.to_string(),
            store_id: store_id.to_string(),
            key,
            len: word.chars().count(),
            quote: word.to_string(),
            note: gloss.to_string(),
            kind: CardKind::Dictionary,
            created_ms: now,
            pinyin: pinyin.to_string(),
            view_count: 1,
            last_viewed_ms: now,
            starred: false,
            style: 0,
        };
        put_card(c, &card)
    }
}

/// 查词卡「加入重点」：一键转正为核心复习卡（星标）。`starred` 只管生词本。
pub fn set_dict_starred(c: &Connection, id: i64, starred: bool) -> Result<()> {
    c.execute(
        "UPDATE cards SET starred=?1 WHERE id=?2",
        params![if starred { 1i64 } else { 0i64 }, id],
    )
    .context("星标失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    fn card(quote: &str, key: (usize, usize, usize)) -> Card {
        Card {
            id: 0,
            book: "莊子".to_string(),
            chapter: "內篇·逍遙遊".to_string(),
            store_id: "zhuangzi".to_string(),
            key,
            len: quote.chars().count(),
            quote: quote.to_string(),
            note: String::new(),
            kind: CardKind::Quote,
            created_ms: 1_700_000_000_000,
            pinyin: String::new(),
            view_count: 0,
            last_viewed_ms: 0,
            starred: false,
            style: 0,
        }
    }

    #[test]
    fn card_roundtrip_and_order() {
        let c = store::connect_in_memory().unwrap();
        assert_eq!(count_cards(&c).unwrap(), 0);
        let a = put_card(&c, &card("北冥有魚", (0, 1, 3))).unwrap();
        let b = put_card(&c, &card("其名為鯤", (0, 1, 12))).unwrap();
        assert!(b > a, "id 应当自增");
        assert_eq!(count_cards(&c).unwrap(), 2);

        // 最新入匣的在前。
        let all = list_cards(&c, 100).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].quote, "其名為鯤");
        assert_eq!(all[0].key, (0, 1, 12));
        assert_eq!(all[0].kind, CardKind::Quote);
        assert_eq!(all[0].source(), "莊子·內篇·逍遙遊");
        // 定位三元组必须原样回来，跳回原文全靠它。
        assert_eq!(all[1].key, (0, 1, 3));
    }

    #[test]
    fn delete_removes_only_one() {
        let c = store::connect_in_memory().unwrap();
        let a = put_card(&c, &card("甲", (0, 0, 0))).unwrap();
        let _b = put_card(&c, &card("乙", (0, 0, 1))).unwrap();
        delete_card(&c, a).unwrap();
        let all = list_cards(&c, 100).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].quote, "乙");
    }

    #[test]
    fn clear_empties_the_box() {
        let c = store::connect_in_memory().unwrap();
        put_card(&c, &card("甲", (0, 0, 0))).unwrap();
        put_card(&c, &card("乙", (0, 0, 1))).unwrap();
        clear_cards(&c).unwrap();
        assert_eq!(count_cards(&c).unwrap(), 0);
    }

    /// 连点两次「存卡」不该留下两张一样的卡（由调用方用 `same_place` 挡）。
    #[test]
    fn same_place_detects_duplicate() {
        let a = card("北冥有魚", (0, 1, 3));
        let b = card("北冥有魚", (0, 1, 3));
        let d = card("北冥有魚", (0, 1, 4)); // 同一个字，别的位置
        assert!(same_place(&a, &b));
        assert!(!same_place(&a, &d));
    }

    /// 书删了卡片还在：卡片自带书名 / 章名 / 原文快照，不依赖原书。
    #[test]
    fn card_survives_without_the_book() {
        let c = store::connect_in_memory().unwrap();
        put_card(&c, &card("北冥有魚", (0, 1, 3))).unwrap();
        // 章节表里根本没有这本书——卡片照样读得出来。
        assert!(store::load_chapter(&c, "zhuangzi", 0).unwrap().is_none());
        let all = list_cards(&c, 100).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].book, "莊子");
        assert_eq!(all[0].quote, "北冥有魚");
    }

    /// **生词本按「词」去重**：同一字查两次不该刷两条，而是次数累加；
    /// 重复时只刷新查阅次数与最近查阅时间，**不覆盖**首次落库的释义与出处。
    #[test]
    fn dict_card_dedups_by_word_and_bumps_count() {
        let c = store::connect_in_memory().unwrap();
        let _a = upsert_dict_card(&c, "zhuangzi", "莊子", "逍遙遊", (0, 1, 3), "鯤", "kūn", "鱼子也").unwrap();
        let b = upsert_dict_card(&c, "liezi", "列子", "湯問", (2, 0, 9), "鯤", "kūn", "大鱼也").unwrap();
        // 同一词「鯤」只留一张。
        let all = list_cards(&c, 100).unwrap();
        assert_eq!(all.len(), 1, "同一词条不应重复落库");
        assert_eq!(all[0].kind, CardKind::Dictionary);
        assert_eq!(all[0].view_count, 2, "查阅次数应累加");
        // 出处与释义保持首次落库时的值（在《庄子》查的「鱼子也」），
        // 不因后来在《列子》再查而改写成「大鱼也」。
        assert_eq!(all[0].book, "莊子", "出处应保持首次落库的书");
        assert_eq!(all[0].pinyin, "kūn");
        assert_eq!(all[0].note, "鱼子也", "释义不应被重复查阅覆盖");
        // 星标一键转正。
        set_dict_starred(&c, b, true).unwrap();
        let all = list_cards(&c, 100).unwrap();
        assert!(all[0].starred);
    }

    /// **四类卡片各归其位**：Quote / Ai / Dictionary / Underline 入匣后
    /// `from_i64` 与 `label` 都能正确还原。
    #[test]
    fn four_kinds_roundtrip() {
        let c = store::connect_in_memory().unwrap();
        for k in [CardKind::Quote, CardKind::Ai, CardKind::Dictionary, CardKind::Underline] {
            let mut cd = card("測", (0, 0, 0));
            cd.kind = k;
            cd.style = 1;
            put_card(&c, &cd).unwrap();
        }
        let all = list_cards(&c, 100).unwrap();
        assert_eq!(all.len(), 4);
        for cd in &all {
            assert_eq!(CardKind::from_i64(cd.kind as i64), cd.kind);
            assert_eq!(cd.style, 1);
        }
        assert_eq!(CardKind::Dictionary.label(), "典");
        assert_eq!(CardKind::Underline.label(), "线");
    }
}
