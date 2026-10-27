//! 章节仓库（v1.50.0「流式 + 虚拟化」架构的落盘层）。
//!
//! 百万字的丛书（如《资治通鉴》全套）**绝不整本驻在内存**：导入时边解析
//! 边把章节写进这里，写完立刻 `drop` 掉那份 `Chapter`——内存里自始至终
//! 只有「正在处理的那一章」。读书的时候再按章取回，用完就还。
//!
//! 落盘的是**清洗之后的纯文本**（`Chapter` 的段落与夹注），不是原始 EPUB
//! 字节，所以取回即可直接排版，不必再跑一遍清洗管线。
//!
//! 表结构极简：`books`（书目元数据）+ `chapters`（一章一行，正文 JSON）+
//! `cards`（卡片匣，v1.74.0）。
//! 一章上限 5000 字（见 [`MAX_CHAPTER_CHARS`]），故单行 JSON 约 10KB，
//! 单行读取飞快——这正是「翻到章末才去取下一章」能丝滑的前提。

use crate::types::Chapter;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::cell::RefCell;
use std::path::PathBuf;

/// **单章字数的硬上限**（导入时强制）。
///
/// 一章太大有两害：①取回一章就要把几十万字拉进内存，虚拟化形同虚设；
/// ②章首页要留「卷首题署」空白带，章越长这个代价摊得越薄、越显突兀。
/// 5000 字对古籍是「一卷」的量级，切在句读处，读起来仍是完整的段落。
pub const MAX_CHAPTER_CHARS: usize = 5000;

/// 仓库文件：`Library::home()/chapters.db`（与书架索引**同一处**）。
///
/// v1.70.0 起**必须与书架同源**：旧制这里硬拼 `~/.jiandu`，而书架走
/// `Library::home()`（`~/Library/Application Support/com.jiandu.app`）。
/// Mac App Store 强制沙盒后 `HOME` 被重定向到容器，两条路径会**分裂成
/// 两份**：书架索引落在容器里、章节库落在容器外的 `~/.jiandu`，于是「书
/// 架有这本书、点开却是空白」（反之亦然）。上架必炸，故统一走同一入口。
pub fn db_path() -> PathBuf {
    crate::library::Library::home().join("chapters.db")
}

/// 打开仓库（没有就建库建表）。每次操作各开一条连接：导入在后台线程、
/// 读取在主线程，共用一条 `Connection` 会打架（rusqlite 的 `Connection`
/// 是 `Send` 而非 `Sync`），各开各的最省心。
pub fn connect() -> Result<Connection> {
    let p = db_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).ok();
    }
    let c = Connection::open(&p).with_context(|| format!("打开章节仓库失败：{}", p.display()))?;
    init(&c)?;
    // 老用户的书还在旧库里（见 [`legacy_db_path`]）：只读挂上，查询 miss 时回退。
    attach_legacy(&c);
    Ok(c)
}

/// v1.69 及更早的章节库位置：`~/.jiandu/chapters.db`。
///
/// v1.70.0 把库统一到 `Library::home()`（书架索引所在处）之后，**老用户
/// 的书全在旧库里**——只换路径不改回退，书架上的书会集体变空白（骨架在
/// 新库、正文在旧库，hydrate 取不到任何一章）。实测本机就是：旧库 7 部
/// 3385 章、新库 1 部 799 章，六本书直接读不出来。
///
/// 这里**只读挂载**旧库（`ATTACH … AS legacy`），读 miss 时回退：
///   · **零写入**：绝不改、绝不删旧库，用户原数据原样留着；
///   · **沙盒下自动失效**：容器里没有 `~/.jiandu`，挂载失败即跳过，
///     所以 App Store 版不会因此多要任何权限；
///   · 之后导入的书一律进新库，旧库自然退役。
fn legacy_db_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join(".jiandu").join("chapters.db");
    // 同一个文件（`JIANDU_HOME` 被指到旧目录时）不必挂两次。
    if !p.is_file() || p == db_path() {
        return None;
    }
    Some(p)
}

/// 把旧库以 `legacy` 只读挂到这条连接上；失败一律忽略（旧库损坏 / 沙盒
/// 里读不到都不该影响新库工作）。
fn attach_legacy(c: &Connection) {
    let Some(p) = legacy_db_path() else {
        return;
    };
    // 路径里理论上不会有单引号，仍然转义一次（防御性，成本为零）。
    let esc = p.display().to_string().replace('\'', "''");
    if let Err(e) = c.execute_batch(&format!("ATTACH DATABASE '{esc}' AS legacy")) {
        eprintln!("[jiandu] 旧章节库挂载失败，已忽略：{e}");
    }
}

/// 内存库（单测用）：与落盘库同一套表结构，但不碰 `~/.jiandu` 里的真数据。
pub fn connect_in_memory() -> Result<Connection> {
    let c = Connection::open_in_memory().context("打开内存章节库失败")?;
    init(&c)?;
    Ok(c)
}

fn init(c: &Connection) -> Result<()> {
    // **写盘档位**（v1.67.2）：导入一部大书要落上千章，逐章自动提交等于
    // 上千次 fsync——实测 9.5MB 的《尔雅》光落盘就 825ms，占了整趟导入的
    // 82%。这里把库调成 WAL + synchronous=NORMAL，配合 `begin_bulk` 的
    // 「整本一个事务」，fsync 次数从「每章两次」降到「整本一次」。
    //（WAL 是写进库头的持久设置，只生效一次；另两个是连接级，每次开库都设。）
    // 顺带：WAL 让「后台导入写」与「主线程读」互不阻塞，锁等待也少。
    c.pragma_update(None, "journal_mode", &"WAL")?;
    c.pragma_update(None, "synchronous", &1i64)?; // NORMAL
    c.pragma_update(None, "temp_store", &2i64)?; // MEMORY
    // 并发（导入线程写 / 主线程读）撞锁时等 5 秒，别直接抛 SQLITE_BUSY。
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS books(
             store_id   TEXT PRIMARY KEY,
             title      TEXT NOT NULL,
             author     TEXT NOT NULL,
             meta_json  TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS chapters(
             store_id TEXT NOT NULL,
             idx      INTEGER NOT NULL,
             title    TEXT NOT NULL,
             chars    INTEGER NOT NULL,
             json     TEXT NOT NULL,
             PRIMARY KEY(store_id, idx)
         );
         CREATE INDEX IF NOT EXISTS idx_chapters_book ON chapters(store_id);",
    )
    .context("建章节仓库表失败")?;
    // v1.74.0：卡片匣与章节同库（`IF NOT EXISTS`，老库升级自动补表，零迁移）。
    crate::cards::init(c)?;
    // v1.75.0：全文检索索引。失败**静默降级**——没有 FTS5 只是退回线性扫描，
    // 不该让 App 起不来（老库、精简编译的 SQLite 都可能没编 FTS5）。
    crate::search::init(c).ok();
    Ok(())
}

/// 写书目元数据（`meta_json` 是 `Book` 去掉 chapters 之后的那部分：
/// 书名 / 作者 / 目录 / 通假表 / 眉批）。
pub fn put_book(c: &Connection, id: &str, title: &str, author: &str, meta_json: &str) -> Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO books(store_id,title,author,meta_json)
         VALUES(?1,?2,?3,?4)",
        params![id, title, author, meta_json],
    )?;
    Ok(())
}

/// **开批量写事务**：一整本书只在开头 `BEGIN` 一次，末尾 `COMMIT` 一次。
///
/// 不包事务的话，`put_chapter` 每执行一次就是一次自动提交（一次 fsync +
/// 一次日志文件开销）。千章级的大书会因此多花好几秒，还把磁盘按在地上
/// 摩擦——这正是「导入卡顿」的主因（v1.67.2 实测）。
pub fn begin_bulk(c: &Connection) -> Result<()> {
    c.execute_batch("BEGIN").context("开启批量写事务失败")
}

/// 提交批量写事务（与 [`begin_bulk`] 配对）。
pub fn end_bulk(c: &Connection) -> Result<()> {
    c.execute_batch("COMMIT").context("提交批量写事务失败")
}

/// 写一章（`idx` 从 0 起、连续）。
///
/// 调用方应先 [`begin_bulk`]、写完全书再 [`end_bulk`]；否则每一章都是一次
/// 独立的自动提交（见那里的注释）。
pub fn put_chapter(
    c: &Connection,
    id: &str,
    idx: usize,
    title: &str,
    chars: usize,
    json: &str,
) -> Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO chapters(store_id,idx,title,chars,json)
         VALUES(?1,?2,?3,?4,?5)",
        params![id, idx as i64, title, chars as i64, json],
    )?;
    Ok(())
}

/// 取回**第 n 章**（按章号排序后的第 n 条，从 0 起；没有返回 `None`）。
///
/// **按位置取，不按章号等值取**（v1.67.2）：砍掉前言 / 散页会在章号里留下
/// 空洞，等值查询就会「读到空」。按 `ORDER BY idx … OFFSET n` 取，空洞自然
/// 被跳过——于是导入末尾再也不必把全表读回内存重写一遍（那部 17MB 的丛书
/// 光这一趟要 1.1 秒）。
///
/// v1.70.0：新库 miss 就回退旧库（见 [`legacy_db_path`]）。
pub fn load_chapter(c: &Connection, id: &str, n: usize) -> Result<Option<Chapter>> {
    let sql = "SELECT json FROM chapters WHERE store_id=?1 ORDER BY idx ASC LIMIT 1 OFFSET ?2";
    let mut st = c.prepare(sql)?;
    let mut rows = st.query(params![id, n as i64])?;
    if let Some(row) = rows.next()? {
        let json: String = row.get(0)?;
        let ch: Chapter = serde_json::from_str(&json).context("章节 JSON 损坏")?;
        return Ok(Some(ch));
    }
    Ok(legacy_chapter(c, id, n))
}

/// 读路径的**线程本地连接缓存**（v1.70.0）。
///
/// `hydrate_chapter` 在扫索引时会被调上千次：一部 799 章的书「冷跳到末
/// 叶」就要 hydrate 799 次。旧制**每章都 `connect()` 一次**——开库 + 三条
/// PRAGMA + ATTACH 旧库，实测整趟 244ms，里面绝大部分是重复开库，不是读
/// 数据。缓存后整趟只开一次库。
///
/// 为什么是 `thread_local` 而不是全局 `static`：`rusqlite::Connection` 是
/// `Send` 而非 `Sync`，全局拿不到；而取页 / 渲染全在主线程，线程本地缓存
/// 既安全又无需加锁。
thread_local! {
    static READ_CONN: RefCell<Option<Connection>> = RefCell::new(None);
}

/// 借一条读库连接干活（`with` 闭包，免得把 `&Connection` 借出 `RefCell`）。
/// 库打不开就返回 `None`，调用方按「没读到」处理，绝不崩。
pub fn with_read_conn<T>(f: impl FnOnce(&Connection) -> T) -> Option<T> {
    READ_CONN.with(|slot| {
        let mut b = slot.borrow_mut();
        if b.is_none() {
            *b = connect().ok();
        }
        b.as_ref().map(f)
    })
}

/// 丢弃缓存的读连接（下次用时重开）。
///
/// 导入跑在后台线程、写的是**另一条**连接，提交后主线程的读连接本就能看
/// 到（WAL + 每条查询各自开隐式事务）；这里留个显式重置口子，给「库文件
/// 被换掉 / 修过」这类场景兜底。
pub fn reset_read_conn() {
    READ_CONN.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// 旧库回退取章。**任何一步失败都当没查到**（旧库没挂载 / 损坏 / 沙盒
/// 读不到，都不该影响新库那条主路）。
fn legacy_chapter(c: &Connection, id: &str, n: usize) -> Option<Chapter> {
    let sql = "SELECT json FROM legacy.chapters WHERE store_id=?1 ORDER BY idx ASC LIMIT 1 OFFSET ?2";
    let mut st = c.prepare(sql).ok()?;
    let mut rows = st.query(params![id, n as i64]).ok()?;
    let row = rows.next().ok()??;
    let json: String = row.get(0).ok()?;
    serde_json::from_str(&json).ok()
}

/// 该书有几章（新库为 0 时回退旧库——老书整本都在旧库里，count 为 0 会
/// 让书架上的书「点开是空」）。
pub fn chapter_count(c: &Connection, id: &str) -> usize {
    let n = c
        .query_row(
            "SELECT COUNT(*) FROM chapters WHERE store_id=?1",
            params![id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize;
    if n > 0 {
        return n;
    }
    c.query_row(
        "SELECT COUNT(*) FROM legacy.chapters WHERE store_id=?1",
        params![id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0) as usize
}

/// 该书的「章号 → 字数」表（按章号升序）——骨架层用来算进度与跳章偏移，
/// 不必把正文拉回内存。新库为空时回退旧库。
pub fn chapter_sizes(c: &Connection, id: &str) -> Vec<usize> {
    let v = sizes_in(c, "main", id);
    if !v.is_empty() {
        return v;
    }
    sizes_in(c, "legacy", id)
}

fn sizes_in(c: &Connection, schema: &str, id: &str) -> Vec<usize> {
    let sql = format!("SELECT chars FROM {schema}.chapters WHERE store_id=?1 ORDER BY idx ASC");
    let mut st = match c.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match st.query_map(params![id], |r| r.get::<_, i64>(0)) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|v| v.ok()).map(|v| v as usize).collect()
}

/// 删掉整本书（从书架移除时调用）。
pub fn delete_book(c: &Connection, id: &str) -> Result<()> {
    c.execute("DELETE FROM chapters WHERE store_id=?1", params![id])?;
    c.execute("DELETE FROM books WHERE store_id=?1", params![id])?;
    // v1.75.0：顺带清掉这本书的全文索引，否则索引里躺着永不命中的垃圾，
    // 白白占着 trigram 那份不小的体积。
    crate::search::drop_book(c, id).ok();
    Ok(())
}

/// 重排章号：砍掉前言 / 散页之后，剩下的章重新从 0 连续编号。
///
/// 只在导入末尾走一次；册数动辄数百，但每章才 10KB，整体重写不过几十毫秒。
pub fn renumber(c: &Connection, id: &str) -> Result<()> {
    let mut st = c.prepare(
        "SELECT idx,title,chars,json FROM chapters WHERE store_id=?1 ORDER BY idx ASC",
    )?;
    let rows = st
        .query_map(params![id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .filter_map(|v| v.ok())
        .collect::<Vec<_>>();
    let tx = c.unchecked_transaction()?;
    tx.execute("DELETE FROM chapters WHERE store_id=?1", params![id])?;
    for (i, (_, title, chars, json)) in rows.iter().enumerate() {
        tx.execute(
            "INSERT INTO chapters(store_id,idx,title,chars,json) VALUES(?1,?2,?3,?4,?5)",
            params![id, i as i64, title, *chars, json],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 删掉指定章（按**当前位置**，即章号排序后的第 n 条）。
///
/// 与 [`load_chapter`] 同一套口径（v1.67.2）：位置为准，章号里的空洞不算数。
/// 调用方要**从大到小**删——删掉靠前的一条，后面的位置会整体前移。
pub fn drop_chapters(c: &Connection, id: &str, doomed: &[usize]) -> Result<()> {
    for i in doomed {
        c.execute(
            "DELETE FROM chapters WHERE rowid = (
                 SELECT rowid FROM chapters WHERE store_id=?1
                 ORDER BY idx ASC LIMIT 1 OFFSET ?2)",
            params![id, *i as i64],
        )?;
    }
    Ok(())
}

/// 清掉某本书的全部章（重导同一文件时用，避免残留上一次的旧章）。
pub fn clear_chapters(c: &Connection, id: &str) -> Result<()> {
    c.execute("DELETE FROM chapters WHERE store_id=?1", params![id])?;
    Ok(())
}

/// 改写**第 n 章**的章题（目录润色之后才定得下来题名，那时正文早已落盘）。
///
/// 与 [`load_chapter`] 同一套位置口径（v1.67.2）。
pub fn set_title(c: &Connection, id: &str, n: usize, title: &str) -> Result<()> {
    c.execute(
        "UPDATE chapters SET title=?1 WHERE rowid = (
             SELECT rowid FROM chapters WHERE store_id=?2
             ORDER BY idx ASC LIMIT 1 OFFSET ?3)",
        params![title, id, n as i64],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v1.70.0：旧库**没挂载时**（沙盒 / 旧库不存在 / 内存库），回退查询
    /// 必须静默降级成「没查到」，绝不能把 `no such table: legacy.chapters`
    /// 当错误抛出去——那会让所有老书一打开就报错。
    #[test]
    fn legacy_fallback_degrades_silently_when_absent() {
        let c = connect_in_memory().expect("内存库");
        put_book(&c, "b1", "书", "", "{}").expect("建书");
        let ch = Chapter {
            title: "第一章".to_string(),
            paragraphs: vec![crate::types::Paragraph::plain("道可道，非常道。")],
        };
        let json = serde_json::to_string(&ch).expect("序列化");
        put_chapter(&c, "b1", 0, "第一章", 8, &json).expect("落章");

        // 主库查得到：照旧返回。
        assert!(load_chapter(&c, "b1", 0).unwrap().is_some());
        assert_eq!(chapter_count(&c, "b1"), 1);
        assert_eq!(chapter_sizes(&c, "b1").len(), 1);

        // 主库查不到 + 没有 legacy：全部降级为「空」，且不返回 Err。
        assert!(load_chapter(&c, "nope", 0).unwrap().is_none());
        assert_eq!(chapter_count(&c, "nope"), 0);
        assert!(chapter_sizes(&c, "nope").is_empty());
    }
}
