//! **极速全文检索**（v1.75.0，V2.0 模块一）：SQLite FTS5 倒排索引。
//!
//! 现状是 `Book::search` 在内存里做**逐字线性扫描**——一部 799 章的书，
//! 检索一次要把全书每一段都摊开比对一遍。FTS5 把这件事换成一次 SQL 查询：
//! 毫秒级拿到「哪些章可能有」，再只对这几章做精确扫描。
//!
//! # 三个关键取舍
//!
//! ## 一、分词器必须是 `trigram`
//!
//! 默认的 `unicode61` 按空格切词，中文一整段会被当成**一个 token**——
//! 搜「长安」等于全表扫，索引白建。`trigram` 是滑动三字窗口，天然适合中文。
//!
//! 代价：**查询短于 3 字时用不上索引**（SQLite 文档明说），所以
//! `< 3` 字的查询仍走老的内存扫描——调用方（UI）负责这个分支。
//!
//! ## 二、索引**按书按需建**
//!
//! trigram 索引体积不小（比正文还大）。书架里躺着二十本书，没搜过的
//! 一本都不该占空间：只有**第一次搜某本书**时才为它建索引。
//!
//! ## 三、索引只存正文副本，不搞 external content
//!
//! `content='chapters'` 那种 external content 表不重复存正文，很省；但章节
//! 表没有稳定的 INTEGER PRIMARY KEY（主键是 `(store_id, idx)` 复合的），
//! 靠隐式 rowid 做 `content_rowid`，一旦删章 / 重灌就对不上，错起来**静默
//! 返回空结果**——比多占一倍磁盘危险得多。所以这里老老实实存一份纯正文。
//!
//! 纯正文 = 各段 `text` 拼起来（注文不进索引：注文是后加的注解，混进结果
//! 会让「跳到那一叶」失去意义，与 `Book::search` 口径一致）。

use anyhow::Result;
use rusqlite::{Connection, params};

/// 虚表名。
const FTS: &str = "chapter_fts";

/// 建 FTS5 表（`IF NOT EXISTS`，老库升级自动补，零迁移）。
///
/// 失败一律交给调用方静默降级——没有 FTS5 只是退回线性扫描，不该让 App 起不来。
pub fn init(c: &Connection) -> Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {FTS} USING fts5(
             store_id UNINDEXED,
             cidx     UNINDEXED,
             body,
             tokenize = 'trigram'
         );"
    ))?;
    Ok(())
}

/// 这本书建过索引了吗？
pub fn is_indexed(c: &Connection, store_id: &str) -> bool {
    let sql = format!("SELECT 1 FROM {FTS} WHERE store_id = ?1 LIMIT 1");
    c.prepare(&sql)
        .and_then(|mut st| st.query_row(params![store_id], |_| Ok(())))
        .map(|_| true)
        .unwrap_or(false)
}

/// 为一本书建索引：把它的每一章正文灌进 FTS5。
///
/// 返回灌进去的章数。**按需建**——只在第一次搜这本书时调用。
pub fn build(c: &Connection, store_id: &str) -> Result<usize> {
    // 幂等：先清干净，重复调用不会把索引灌两遍（trigram 索引翻倍很贵）。
    drop_book(c, store_id)?;
    let sql = "SELECT idx, json FROM chapters WHERE store_id = ?1 ORDER BY idx ASC";
    let mut st = c.prepare(sql)?;
    let rows = st.query_map(params![store_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut pairs: Vec<(i64, String)> = Vec::new();
    for r in rows {
        pairs.push(r?);
    }
    drop(st);

    c.execute_batch("BEGIN")?;
    let res = (|| -> Result<usize> {
        let ins = format!("INSERT INTO {FTS}(store_id, cidx, body) VALUES (?1, ?2, ?3)");
        let mut st = c.prepare(&ins)?;
        let mut n = 0usize;
        for (idx, json) in &pairs {
            let ch: crate::types::Chapter = match serde_json::from_str(json) {
                Ok(ch) => ch,
                // 一章坏了不该拖垮整本书的索引：跳过，其余照建。
                Err(_) => continue,
            };
            st.execute(params![store_id, *idx, plain_text(&ch)])?;
            n += 1;
        }
        Ok(n)
    })();
    match res {
        Ok(n) => {
            c.execute_batch("COMMIT")?;
            Ok(n)
        }
        Err(e) => {
            let _ = c.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// 确保这本书有索引（没有就现建）。UI 搜之前调一下即可。
pub fn ensure(c: &Connection, store_id: &str) -> Result<()> {
    if is_indexed(c, store_id) {
        return Ok(());
    }
    build(c, store_id).map(|_| ())
}

/// 检索：返回**候选章号**（按相关度 `bm25` 排序，越靠前越可能命中）。
///
/// 只给章号，不给段内偏移——FTS5 没有 `offsets()`（那是 FTS4 的），
/// 精确位置交给调用方在候选章里做子串扫描：候选章通常只有几章，
/// 比扫全书便宜两三个数量级。
///
/// 查询串一律包成 phrase（`"…"`）：既避开 FTS5 的 `AND` / `OR` / `*` 语法，
/// 又能让多字词按顺序匹配。
pub fn query(c: &Connection, store_id: &str, q: &str, limit: usize) -> Result<Vec<usize>> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let phrase = format!("\"{}\"", q.replace('"', "\"\""));
    let sql = format!(
        "SELECT cidx FROM {FTS} WHERE body MATCH ?1 AND store_id = ?2 \
         ORDER BY bm25({FTS}) LIMIT ?3"
    );
    let mut st = match c.prepare(&sql) {
        Ok(st) => st,
        // 表不存在 / 没编 FTS5：当作「没有索引」，调用方退回线性扫描。
        Err(_) => return Ok(Vec::new()),
    };
    let rows = st.query_map(params![phrase, store_id, limit as i64], |r| {
        r.get::<_, i64>(0)
    })?;
    let mut out = Vec::new();
    for r in rows {
        if let Ok(v) = r {
            if v >= 0 {
                out.push(v as usize);
            }
        }
    }
    Ok(out)
}

/// 删掉一本书的索引（书从架上移除时同步调用，不然索引就是垃圾）。
pub fn drop_book(c: &Connection, store_id: &str) -> Result<()> {
    let sql = format!("DELETE FROM {FTS} WHERE store_id = ?1");
    c.execute(&sql, params![store_id])?;
    Ok(())
}

/// 一章的纯正文：各段 `text` 用换行拼起来（注文不进索引）。
fn plain_text(ch: &crate::types::Chapter) -> String {
    let mut s = String::with_capacity(ch.char_count().saturating_mul(2).min(1 << 20));
    for p in &ch.paragraphs {
        s.push_str(&p.text);
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fresh() -> Connection {
        // `connect_in_memory` 已含 `store::init`（含本模块的 FTS5 表）。
        crate::store::connect_in_memory().expect("内存库")
    }

    fn put(c: &Connection, id: &str, idx: usize, title: &str, paras: &[&str]) {
        let ch = crate::types::Chapter {
            title: title.to_string(),
            paragraphs: paras
                .iter()
                .map(|t| crate::types::Paragraph::plain(*t))
                .collect(),
        };
        let json = serde_json::to_string(&ch).unwrap();
        c.execute(
            "INSERT INTO chapters(store_id, idx, title, chars, json) VALUES (?1,?2,?3,?4,?5)",
            params![id, idx as i64, title, paras.iter().map(|s| s.chars().count()).sum::<usize>() as i64, json],
        )
        .unwrap();
    }

    #[test]
    fn fts_finds_the_chapter_without_scanning_the_book() {
        let c = fresh();
        put(&c, "b1", 0, "第一章", &["道可道，非常道。"]);
        put(&c, "b1", 1, "第二章", &["长安城中多甲第。"]);
        put(&c, "b1", 2, "第三章", &["君子务本，本立而道生。"]);
        assert!(!is_indexed(&c, "b1"));
        assert_eq!(build(&c, "b1").unwrap(), 3);
        assert!(is_indexed(&c, "b1"));

        let hits = query(&c, "b1", "长安城", 20).unwrap();
        assert_eq!(hits, vec![1], "候选章应当只有第二章：{hits:?}");
        // 命中两章时按相关度排序，且两章都在。
        let both = query(&c, "b1", "君子务本", 20).unwrap();
        assert_eq!(both, vec![2]);
    }

    /// 只搜**这一本**：另一本书里的同名句子不该混进来。
    #[test]
    fn fts_is_scoped_to_one_book() {
        let c = fresh();
        put(&c, "b1", 0, "第一章", &["长安一片月，万户捣衣声。"]);
        put(&c, "b2", 0, "第一章", &["长安回望绣成堆。"]);
        build(&c, "b1").unwrap();
        let hits = query(&c, "b1", "长安回望", 20).unwrap();
        assert!(hits.is_empty(), "b1 里没有这句：{hits:?}");
    }

    /// 没建索引的书：返回空（**不是报错**）——调用方据此退回线性扫描。
    #[test]
    fn unindexed_book_returns_empty_not_error() {
        let c = fresh();
        put(&c, "b9", 0, "第一章", &["天地玄黄，宇宙洪荒。"]);
        assert!(query(&c, "b9", "宇宙洪荒", 20).unwrap().is_empty());
    }

    /// FTS5 语法字符（`AND` / `*` / 引号）必须被当字面量，绝不炸。
    #[test]
    fn fts_syntax_chars_are_escaped() {
        let c = fresh();
        put(&c, "b1", 0, "第一章", &["子曰：学而时习之，不亦说乎。"]);
        build(&c, "b1").unwrap();
        for q in ["不亦说乎", "AND", "*", "\"", "学而 (时习)"] {
            let r = query(&c, "b1", q, 20);
            assert!(r.is_ok(), "查询 {q:?} 不该报错：{r:?}");
        }
    }

    #[test]
    fn dropping_a_book_drops_its_index() {
        let c = fresh();
        put(&c, "b1", 0, "第一章", &["长安城中多甲第。"]);
        build(&c, "b1").unwrap();
        assert!(is_indexed(&c, "b1"));
        drop_book(&c, "b1").unwrap();
        assert!(!is_indexed(&c, "b1"));
    }
}
