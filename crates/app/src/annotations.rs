//! 文本划线批注 · SQLite 持久化层（app 专属，与 `jiandu-core` 零耦合）。
//!
//! **防雷架构（用户 2026-11-08 钦定）**：批注是「覆盖式」数据——只记
//! `(文档键, 起止字符下标, 类型, 颜色)`，**绝不**写进正文富文本 / 排版引擎。
//! 渲染时由 `collation` 的画线循环按索引还原，排版数学（固定字宽 + 软折行）
//! 与选区高亮完全同源，零侵入。
//!
//! 表结构（与用户需求一一对应）：
//! `id, doc_key(page_id), start_char_index, end_char_index, kind, color`。
use rusqlite::Connection;
use std::path::PathBuf;

/// 批注类型：0=下划线 / 1=波浪线 / 2=矩形框。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnKind {
    Underline = 0,
    Wavy = 1,
    Box = 2,
}

/// 一条批注（内存态 + 落库态）。`id` 为 `-1` 表示尚未落库的内存草稿。
#[derive(Clone, Debug)]
pub struct Annotation {
    pub id: i64,
    pub start: usize,
    pub end: usize,
    pub kind: AnnKind,
    /// 0xAARRGGBB，仅作存档 / 将来导出用，绘制时按 `kind` 取固定淡色。
    pub color: u32,
}

/// 淡朱砂红（下划线 / 波浪线）。
pub const C_VERMILION: u32 = 0x998C2B22;
/// 淡墨蓝（矩形框）。
pub const C_INKBLUE: u32 = 0x992B4C7E;

/// 按类型给默认淡色（落库用）。
pub fn default_color(kind: AnnKind) -> u32 {
    match kind {
        AnnKind::Box => C_INKBLUE,
        _ => C_VERMILION,
    }
}

/// 打开（或创建）批注库：专用 `annotations.db`，建表幂等，WAL + NORMAL 保安全。
pub fn open_db() -> Option<Connection> {
    let dir = support_dir()?;
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("annotations.db");
    let conn = Connection::open(&path).ok()?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS annotations(
            id               INTEGER PRIMARY KEY,
            doc_key         TEXT    NOT NULL,
            start_char_index INTEGER NOT NULL,
            end_char_index   INTEGER NOT NULL,
            kind             INTEGER NOT NULL,
            color            INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_ann_doc
            ON annotations(doc_key, start_char_index);",
    )
    .ok()?;
    Some(conn)
}

/// 写入一条批注，返回自增 id。
pub fn insert(conn: &Connection, doc_key: &str, a: &Annotation) -> Option<i64> {
    conn.execute(
        "INSERT INTO annotations(doc_key, start_char_index, end_char_index, kind, color)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            doc_key,
            a.start as i64,
            a.end as i64,
            a.kind as i32,
            a.color as i64
        ],
    )
    .ok()?;
    Some(conn.last_insert_rowid())
}

/// 读取某文档的全部批注（按起点排序）。
pub fn list(conn: &Connection, doc_key: &str) -> Vec<Annotation> {
    let mut stmt = match conn.prepare(
        "SELECT id, start_char_index, end_char_index, kind, color \
         FROM annotations WHERE doc_key = ?1 ORDER BY start_char_index",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![doc_key], |r| {
        Ok(Annotation {
            id: r.get(0)?,
            start: r.get(1)?,
            end: r.get(2)?,
            kind: match r.get::<_, i32>(3)? {
                1 => AnnKind::Wavy,
                2 => AnnKind::Box,
                _ => AnnKind::Underline,
            },
            color: r.get::<_, i64>(4)? as u32,
        })
    });
    match rows {
        Ok(it) => it.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// 按 id 删除一条批注。
pub fn delete(conn: &Connection, id: i64) {
    let _ = conn.execute("DELETE FROM annotations WHERE id = ?1", rusqlite::params![id]);
}

/// `~/Library/Application Support/com.jiandu.app`（与正文库同目录，互不干扰）。
fn support_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join("Library/Application Support/com.jiandu.app"))
}
