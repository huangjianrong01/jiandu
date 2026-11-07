//! L5 落库 · SQLite 持久化适配（消费 core 的 `AlignmentStore`）。
//!
//! 设计要点：
//! - 库选择 `rusqlite`（Rust 官方 SQLite 绑定，成熟、安全、零 async 开销）。
//!   生产用 `features=["bundled"]` 自带 SQLite 源码，无需用户机器装 libsqlite3；
//!   本 dev 验证用系统 lib（macOS 自带，编译快）。
//! - 安全写入：`journal_mode=WAL` + `synchronous=NORMAL` + **单事务包所有 INSERT**，
//!   进程崩溃 / 断电均原子回滚；参数化 `?` 占位（防注入 + 提速）；热循环复用 prepared。
//! - 空格无 BoundingBox：逐字符落库，空格行 `is_space=1` 且 x/y/w/h=0，坐标连续不漏位。
//!
//! 该层刻意放在 `ocr-cli`（独立 crate），与 `jiandu-core` 的纯模型解耦——生产可抽成
//! 独立 `crates/align-sqlite` 供 App / CLI 共享，core 始终保持零 DB 依赖（纯洁红线）。

use anyhow::{Context, Result};
use jiandu_core::ocr_provider::AlignmentStore;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

/// 建表 SQL（核心交付物，供审阅）。文档 / 页 / 字形 三层，外键级联删除。
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    id          INTEGER PRIMARY KEY,
    title       TEXT    NOT NULL,
    source_path TEXT,
    provider    TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE(title)
);
CREATE TABLE IF NOT EXISTS pages (
    id           INTEGER PRIMARY KEY,
    doc_id       INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    page_index   INTEGER NOT NULL,
    image_width  INTEGER NOT NULL,
    image_height INTEGER NOT NULL,
    source_image TEXT,
    text         TEXT    NOT NULL,
    UNIQUE(doc_id, page_index)
);
CREATE TABLE IF NOT EXISTS glyphs (
    id          INTEGER PRIMARY KEY,
    page_id     INTEGER NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    char_index  INTEGER NOT NULL,
    ch          TEXT,
    x           REAL    NOT NULL,
    y           REAL    NOT NULL,
    w           REAL    NOT NULL,
    h           REAL    NOT NULL,
    confidence  REAL,
    is_space    INTEGER NOT NULL DEFAULT 0
);
-- ① 点击右栏文字 → 查左图坐标框（毫秒级）：按 (页, 字符序) 定位
CREATE INDEX IF NOT EXISTS idx_glyphs_page_char ON glyphs(page_id, char_index);
-- ② 点击左图某点 → 反查右栏字符（毫秒级正向命中）：按 (页, x, y) 过滤
CREATE INDEX IF NOT EXISTS idx_glyphs_page_xy   ON glyphs(page_id, x, y);
"#;

/// 打开（或创建）SQLite 库并设定安全 PRAGMA。
pub fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("打开 SQLite 失败：{}", path.display()))?;
    // WAL：崩溃 / 断电安全，且允许并发读；NORMAL：WAL 下已足够安全又更快。
    conn.pragma_update(None, "journal_mode", "WAL")
        .context("设置 journal_mode=WAL 失败")?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .context("设置 synchronous=NORMAL 失败")?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("开启 foreign_keys 失败")?;
    Ok(conn)
}

/// 建表（幂等，CREATE TABLE IF NOT EXISTS）。
pub fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_SQL).context("建表失败")?;
    Ok(())
}

/// 把一个 `AlignmentStore` 落库到指定文档 / 页。
/// 返回 `(字形行总数, 含真实框的行数)`（空格占位行 is_space=1、框归零）。
/// 幂等：同一 `(doc_title, page_index)` 重复写入会先删旧页再插（级联删 glyphs）。
pub fn ingest_store(
    conn: &mut Connection,
    doc_title: &str,
    page_index: i64,
    store: &AlignmentStore,
) -> Result<(usize, usize)> {
    let tx = conn.transaction().context("开启事务失败")?;

    // 文档（按 title 去重）
    tx.execute(
        "INSERT OR IGNORE INTO documents(title, source_path, provider) VALUES (?1, ?2, ?3)",
        rusqlite::params![doc_title, store.source_image, store.provider],
    )
    .context("插入 documents 失败")?;
    let doc_id: i64 = tx.query_row(
        "SELECT id FROM documents WHERE title = ?1",
        rusqlite::params![doc_title],
        |r| r.get(0),
    )
    .context("查询 doc_id 失败")?;

    // 旧页若已存在先删（级联删 glyphs），保证幂等可重跑
    tx.execute(
        "DELETE FROM pages WHERE doc_id = ?1 AND page_index = ?2",
        rusqlite::params![doc_id, page_index],
    )
    .context("删除旧页失败")?;

    tx.execute(
        "INSERT INTO pages(doc_id, page_index, image_width, image_height, source_image, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            doc_id,
            page_index,
            store.image_width,
            store.image_height,
            store.source_image,
            store.text
        ],
    )
    .context("插入 pages 失败")?;
    let page_id = tx.last_insert_rowid();

    // 字形：逐字符落库（空格占位 is_space=1、框归零）。复用 prepared 提速。
    // 用独立块收窄 `stmt` 借用，确保 `tx.commit()` 前 stmt 已 drop（否则借用冲突）。
    let (rows, with_box) = {
        let mut stmt = tx
            .prepare(
                "INSERT INTO glyphs(page_id, char_index, ch, x, y, w, h, confidence, is_space)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .context("准备 glyphs INSERT 失败")?;

        // char_index -> 真实框（JSON 只给非空格字形）
        let by_idx: HashMap<usize, &jiandu_core::ocr_provider::OcrGlyph> =
            store.glyphs.iter().map(|g| (g.char_index, g)).collect();

        let mut rows = 0usize;
        let mut with_box = 0usize;
        for (idx, ch) in store.text.chars().enumerate() {
            let is_space = ch.is_whitespace();
            let (x, y, w, h, conf) = if is_space {
                (0.0f64, 0.0, 0.0, 0.0, 0.0f64)
            } else if let Some(g) = by_idx.get(&idx) {
                (
                    g.x as f64,
                    g.y as f64,
                    g.w as f64,
                    g.h as f64,
                    g.confidence as f64,
                )
            } else {
                // 兜底（非空格必在 glyphs 里，理论不可达）：框归零
                (0.0, 0.0, 0.0, 0.0, 0.0)
            };
            if !is_space {
                with_box += 1;
            }
            stmt.execute(rusqlite::params![
                page_id,
                idx as i64,
                ch.to_string(),
                x,
                y,
                w,
                h,
                conf,
                is_space as i64
            ])
            .with_context(|| format!("插入 glyph char_index={} 失败", idx))?;
            rows += 1;
        }
        (rows, with_box)
    };

    tx.commit().context("提交事务失败")?;
    Ok((rows, with_box))
}

/// 从 JSON 文件读取并落库（CLI 入口用）。
pub fn ingest_json_file(
    conn: &mut Connection,
    doc_title: &str,
    page_index: i64,
    json_path: &Path,
) -> Result<(usize, usize)> {
    let raw = std::fs::read_to_string(json_path)
        .with_context(|| format!("读 JSON 失败：{}", json_path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("解析 JSON 失败：{}", json_path.display()))?;
    let store = AlignmentStore::from_compare_json(&v)
        .ok_or_else(|| anyhow::anyhow!("对齐 JSON 格式不合法：{}", json_path.display()))?;
    ingest_store(conn, doc_title, page_index, &store)
}
