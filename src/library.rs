//! 书架：书籍目录、导入、阅读档案持久化。
//!
//! 目录结构（默认 `~/.jiandu/`，可用环境变量 `JIANDU_HOME` 覆盖）：
//! ```text
//! ~/.jiandu/
//! ├── library.json      # 书架索引 + 每本书的阅读档案
//! └── books/            # 导入的书籍副本
//! ```

use crate::types::Book;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 一本书的书架条目（含阅读档案）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShelfEntry {
    pub path: PathBuf,
    pub title: String,
    #[serde(default)]
    pub author: String,
    /// 书封主色。
    #[serde(default)]
    pub accent: u32,
    /// 印章文字。
    #[serde(default)]
    pub seal: String,
    /// 上次读到第几面。
    #[serde(default)]
    pub last_page: usize,
    /// 上次的总面数（用于书架进度）。
    #[serde(default)]
    pub total_pages: usize,
    /// 上次的排版模式：ancient / modern。
    #[serde(default = "default_script")]
    pub script: String,
    /// 上次的单面 / 双面选择：single / facing（古文竖笺恒定单面）。
    #[serde(default = "default_duplex")]
    pub duplex: String,
    /// 最近一次阅读的 Unix 时间戳（秒）；0 表示从未读过。
    /// 书架的「正在阅读」按它排序。
    #[serde(default)]
    pub last_read: u64,
    /// 用户笔记：页序号 -> 笔记正文。
    #[serde(default)]
    pub notes: std::collections::HashMap<usize, String>,
    /// 用户划线的字：(chapter, para, offset)。
    #[serde(default)]
    pub underlines: Vec<(usize, usize, usize)>,
}

fn default_script() -> String {
    "ancient".into()
}

fn default_duplex() -> String {
    "single".into()
}

impl ShelfEntry {
    /// 阅读进度（0.0 ~ 1.0）。
    pub fn progress(&self) -> f32 {
        if self.total_pages == 0 {
            0.0
        } else {
            ((self.last_page + 1) as f32 / self.total_pages as f32).clamp(0.0, 1.0)
        }
    }
    pub fn is_started(&self) -> bool {
        self.last_page > 0
    }
}

/// 书架索引。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub books: Vec<ShelfEntry>,
}

/// 书封主色候选（宣纸 / 黛青 / 赭石 / 松烟…）。
const PALETTE: [u32; 8] = [
    0xB4452F, // 朱砂
    0x8C6239, // 赭石
    0x4E5D51, // 松绿
    0x3E5A6E, // 黛青
    0x6B4A6B, // 紫檀
    0x9A6B4F, // 版框
    0x2F4858, // 墨蓝
    0x7A4E3A, // 檀木
];

/// 由书名推一个稳定的主色（同一本书每次配色一致）。
pub fn accent_of(title: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in title.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    PALETTE[(h as usize) % PALETTE.len()]
}

/// 当前 Unix 时间戳（秒）；取不到返回 0。
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Library {
    pub fn home() -> PathBuf {
        if let Ok(p) = std::env::var("JIANDU_HOME") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        let base = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join(".jiandu")
    }

    pub fn books_dir() -> PathBuf {
        Self::home().join("books")
    }

    pub fn index_path() -> PathBuf {
        Self::home().join("library.json")
    }

    /// 读取索引（过滤掉已被删除的文件）。
    pub fn load() -> Self {
        let mut lib: Self = std::fs::read_to_string(Self::index_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        lib.books.retain(|b| b.path.exists());
        lib
    }

    /// 保存索引（失败不影响使用）。
    pub fn save(&self) {
        let dir = Self::home();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::index_path(), s);
        }
    }

    /// 更新一本书的阅读档案（同时记下最近阅读时间，「正在阅读」用）。
    pub fn touch(&mut self, idx: usize, page: usize, total: usize, script: &str, duplex: &str) {
        if let Some(e) = self.books.get_mut(idx) {
            e.last_page = page;
            e.total_pages = total;
            e.script = script.to_string();
            e.duplex = duplex.to_string();
            e.last_read = now_secs();
        }
        self.save();
    }

    /// 重命名书架第 idx 本书：更新条目，并尽力同步改写 json 书籍文件内的书名
    /// （阅读页页眉、封面都从文件读取，只改条目会不一致）。
    pub fn rename(&mut self, idx: usize, new_title: &str) -> Result<()> {
        let title = new_title.trim();
        if title.is_empty() {
            anyhow::bail!("书名不能为空");
        }
        let Some(e) = self.books.get_mut(idx) else {
            anyhow::bail!("书架里没有这本书");
        };
        e.title = title.to_string();
        let path = e.path.clone();
        self.save();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(s) = std::fs::read_to_string(&path) {
                if let Ok(mut book) = serde_json::from_str::<Book>(&s) {
                    book.title = title.to_string();
                    if let Ok(out) = serde_json::to_string_pretty(&book) {
                        let _ = std::fs::write(&path, out);
                    }
                }
            }
        }
        Ok(())
    }

    /// 从书架删除第 idx 本书（索引条目 + books/ 里的文件副本）。
    pub fn remove(&mut self, idx: usize) -> Result<()> {
        let Some(e) = self.books.get(idx) else {
            anyhow::bail!("书架里没有这本书");
        };
        let path = e.path.clone();
        self.books.remove(idx);
        self.save();
        // 只删书架目录内的副本；文件缺失（已被外部清理）不算失败。
        if path.exists() {
            std::fs::remove_file(&path).context("删除书籍文件失败")?;
        }
        Ok(())
    }

    /// 保存一本书的笔记与划线。
    pub fn save_annotations(
        &mut self,
        idx: usize,
        notes: &std::collections::HashMap<usize, String>,
        underlines: &[(usize, usize, usize)],
    ) {
        if let Some(e) = self.books.get_mut(idx) {
            e.notes = notes.clone();
            e.underlines = underlines.to_vec();
        }
        self.save();
    }

    /// 导入一本书：解析 -> 写入 books/ -> 加入书架。
    ///
    /// EPUB 会先解析成结构化 `Book`，再以 **JSON** 落盘（`books/<书名>.json`）：
    /// 之后打开不必每次解压 zip，标注 / 进度也有稳定的 JSON 载体。
    /// md / txt 仍按原样复制。
    pub fn import_path(&mut self, src: &Path) -> Result<usize> {
        let book: Book = crate::importer::import_from_path(src)?;
        let dir = Self::books_dir();
        std::fs::create_dir_all(&dir).context("无法创建书架目录")?;

        let ext = src
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("book")
            .to_string();
        let mut dst: PathBuf = if ext == "epub" {
            dir.join(format!("{stem}.json"))
        } else {
            dir.join(
                src.file_name()
                    .map(|s| s.to_os_string())
                    .unwrap_or_else(|| std::ffi::OsString::from("book.txt")),
            )
        };
        if ext != "epub" && dst != src && dst.exists() {
            let mut n = 2;
            loop {
                let cand = dir.join(format!("{stem}-{n}.{ext}"));
                if !cand.exists() {
                    dst = cand;
                    break;
                }
                n += 1;
            }
        }

        // 同一文件重复导入则复用原条目。
        if let Some(pos) = self.books.iter().position(|b| b.path == dst) {
            self.save();
            return Ok(pos);
        }

        if ext == "epub" {
            crate::importer::write_book_json(&book, &dst)?;
        } else if dst != src {
            std::fs::copy(src, &dst).context("复制书籍到书架失败")?;
        }

        let accent = if book.cover.accent != 0 {
            book.cover.accent
        } else {
            accent_of(&book.title)
        };
        self.books.push(ShelfEntry {
            path: dst,
            title: book.title.clone(),
            author: book.author.clone(),
            accent,
            seal: book.cover.seal_or_default(),
            last_page: 0,
            total_pages: 0,
            script: "ancient".into(),
            duplex: "single".into(),
            last_read: 0,
            notes: std::collections::HashMap::new(),
            underlines: Vec::new(),
        });
        self.save();
        Ok(self.books.len() - 1)
    }

    /// 首次运行时把内置样张写进书架，让书架不空。
    pub fn seed_sample(&mut self) -> Result<usize> {
        let dir = Self::books_dir();
        std::fs::create_dir_all(&dir).context("无法创建书架目录")?;
        let path = dir.join("daodejing.json");
        let book = crate::content::sample_dao_de_jing();
        if !path.exists() {
            let json = serde_json::to_string_pretty(&book)?;
            std::fs::write(&path, json).context("写入样张失败")?;
        }
        self.import_path(&path)
    }

    /// 从内置书库下载一本书：把书内容写成 json 存入 books/，再加入书架。
    /// 重复下载同一本会复用原条目（import_path 的去重逻辑）。
    pub fn download(&mut self, key: &str) -> Result<usize> {
        let book = crate::content::catalog_book(key)
            .ok_or_else(|| anyhow::anyhow!("书库没有这本书：{key}"))?;
        let dir = Self::books_dir();
        std::fs::create_dir_all(&dir).context("无法创建书架目录")?;
        let path = dir.join(format!("{key}.json"));
        let json = serde_json::to_string_pretty(&book)?;
        std::fs::write(&path, json).context("写入书库书籍失败")?;
        self.import_path(&path)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `JIANDU_HOME` 是进程级环境变量：并行测试同时改会互相污染，
    /// 所有动它的测试都先抢这把锁（types.rs 的下载测试共用）。
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jiandu-lib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn rename_updates_entry_and_json_file() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = temp_home("rename");
        std::env::set_var("JIANDU_HOME", &home);
        let mut lib = Library::load();
        let idx = lib.download("qianzi").expect("下载千字文应成功");
        assert_eq!(lib.books[idx].title, "千字文");
        lib.rename(idx, "  千字文（ renamed ）  ").expect("重命名应成功");
        assert_eq!(lib.books[idx].title, "千字文（ renamed ）");
        // 文件内的书名同步改写（阅读页页眉从文件读）。
        let path = lib.books[idx].path.clone();
        let book: Book = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(book.title, "千字文（ renamed ）");
        // 空名拒绝。
        assert!(lib.rename(idx, "   ").is_err());
        // 索引落盘：重载后仍是新名。
        let reloaded = Library::load();
        assert_eq!(reloaded.books[idx].title, "千字文（ renamed ）");
        std::env::remove_var("JIANDU_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn remove_drops_entry_and_file() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = temp_home("remove");
        std::env::set_var("JIANDU_HOME", &home);
        let mut lib = Library::load();
        let idx = lib.download("sanzi").expect("下载三字经应成功");
        let path = lib.books[idx].path.clone();
        assert!(path.exists());
        lib.remove(idx).expect("删除应成功");
        assert!(lib.books.is_empty());
        assert!(!path.exists(), "书籍文件副本应被删掉");
        std::env::remove_var("JIANDU_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn epub_import_is_stored_as_json() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = temp_home("epub");
        std::env::set_var("JIANDU_HOME", &home);
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/daodejing-toc.epub");
        assert!(src.exists(), "样张 epub 应存在");
        let mut lib = Library::load();
        let idx = lib.import_path(&src).expect("导入 epub 应成功");
        let path = lib.books[idx].path.clone();
        // 入库后是 JSON，而不是原样复制的 .epub。
        assert_eq!(
            path.extension().and_then(|s| s.to_str()),
            Some("json"),
            "epub 应转成 json 入库：{}",
            path.display()
        );
        assert!(path.exists());
        let book: Book =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).expect("json 可读");
        assert!(!book.chapters.is_empty(), "应有正文");
        assert!(!book.title.is_empty(), "应有书名");
        // 再次导入复用同一条目，不会生成第二个副本。
        let idx2 = lib.import_path(&src).expect("重复导入应成功");
        assert_eq!(idx, idx2);
        assert_eq!(lib.books.len(), 1);
        std::env::remove_var("JIANDU_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn touch_records_last_read() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = temp_home("touch");
        std::env::set_var("JIANDU_HOME", &home);
        let mut lib = Library::load();
        let idx = lib.download("qianzi").expect("下载千字文应成功");
        assert_eq!(lib.books[idx].last_read, 0);
        lib.touch(idx, 3, 20, "ancient", "single");
        assert!(lib.books[idx].last_read > 0, "touch 应记下阅读时间");
        assert_eq!(lib.books[idx].last_page, 3);
        std::env::remove_var("JIANDU_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }
}
