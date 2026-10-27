//! 书架：书籍目录、导入、阅读档案持久化。
//!
//! 目录结构（跨平台标准应用数据目录，macOS → `~/Library/Application Support/com.jiandu.app/`；
//! 可用环境变量 `JIANDU_HOME` 覆盖，开发与测试大量依赖它）：
//! ```text
//! ~/Library/Application Support/com.jiandu.app/
//! ├── library.json      # 书架索引 + 每本书的阅读档案
//! └── books/            # 导入的书籍副本
//! ```
//!
//! **App Store 沙盒**：上述目录在沙盒下会被系统透明重定向到容器
//! `Data/Library/Application Support/com.jiandu.app/`，同一份代码三态通用。
//! 首次启动若发现旧的 `~/.jiandu/`，会把旧数据一次性迁移到新目录（保留旧目录不删）。

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
    /// 四部分类：经 / 史 / 子 / 集；空串表示按书名自动归类，
    /// 其它值即用户自定的书单名（「自定义书单」页签按它聚合）。
    #[serde(default)]
    pub category: String,
    /// **章节仓库 id**（v1.50.0 流式架构）：`Some` 表示这本书的正文落在
    /// `~/.jiandu/chapters.db` 里，开卷时按章取回；`None` 表示老条目
    /// （正文整本在 json / txt 里），走旧的一次性读取。
    #[serde(default)]
    pub store_id: Option<String>,
    /// 注者（校注 / 集注 / 笺注者）；空表示无。
    #[serde(default)]
    pub annotator: String,
    /// 版本或朝代（如「宋刻本」「〔宋〕」）；空表示按作者朝代 / 整理方式推。
    #[serde(default)]
    pub edition: String,
    /// 全书章数（导入时记下；「当前卷册」没有记录时按进度推算）。
    #[serde(default)]
    pub chapters: usize,
    /// 上次读到的卷册标题（`touch` 时写入，书架直接显示，不必再开书）。
    #[serde(default)]
    pub chapter_title: String,
    /// 书签：每枚记录定位三元组（chapter/para/offset）、章节名、一句摘录与
    /// 创建时间。落档到书架，跨会话保留（v1.53.0 新增）。
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
}

/// 一枚书签。
///
/// 定位用 `(chapter, para, offset)` 三元组——与 `Root::page_anchor` 同源，
/// 换列数重新分页后仍可靠找回（见 `page_anchor`/`restore_anchor` 的设计意图）。
/// 章节名与摘录在落档时快照下来，列表渲染直接显示，不必每次回查书稿。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bookmark {
    /// 稳定 id，删除时用；由纳秒时间戳生成，不会撞。
    pub id: String,
    /// 定位三元组。
    pub chapter: usize,
    pub para: usize,
    pub offset: usize,
    /// 章节名（落档时快照）。
    pub chapter_title: String,
    /// 一句简短的摘录（落档时快照）。
    pub excerpt: String,
    /// 创建时间（Unix 秒），排序用。
    pub created: u64,
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
    /// 归入哪一页签：用户设了经史子集就用它，没设就按书名自动归，
    /// 自定书单（任意非四部名）统一落到「自定」页签。
    pub fn cat_key(&self) -> &'static str {
        match self.category.as_str() {
            "经" => "经",
            "史" => "史",
            "子" => "子",
            "集" => "集",
            "" => auto_category(&self.title),
            _ => "自定",
        }
    }

    /// 「作者 / 注者」行：作者串里的朝代标注另列，注者缀在后面。
    /// 导入时截断留下的尾巴（`_ …`）一并清掉，书架不显示这种毛刺。
    pub fn author_line(&self) -> String {
        let (_, name) = split_dynasty(&self.author);
        let name = name
            .trim_end_matches(|c: char| c.is_whitespace() || c == '…' || c == '_' || c == '·')
            .trim()
            .to_string();
        let mut s = if name.is_empty() {
            "佚名".to_string()
        } else {
            name
        };
        if !self.annotator.is_empty() {
            s = if s == "佚名" {
                self.annotator.clone()
            } else {
                format!("{s} {} 注", self.annotator)
            };
        }
        s
    }

    /// 「版本或朝代」行：用户填的版本优先，否则从作者串取朝代，
    /// 都没有就按入库形态说（json 是整理本，md/txt 是钞本）。
    pub fn edition_line(&self) -> String {
        let e = self.edition.trim();
        if !e.is_empty() {
            return e.to_string();
        }
        let (dynasty, _) = split_dynasty(&self.author);
        if !dynasty.is_empty() {
            return format!("〔{dynasty}〕");
        }
        match self
            .path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("json") => "整理本".to_string(),
            Some("epub") => "排印本".to_string(),
            _ => "钞本".to_string(),
        }
    }

    /// 「当前卷册」行：读过就显示上次那一卷（过长截断），
    /// 没读过但有章数就按进度估一个，全然没有则「未始读」。
    pub fn volume_line(&self) -> String {
        let t = self.chapter_title.trim();
        if !t.is_empty() {
            return ellipsis(t, 9);
        }
        if self.chapters > 0 && self.last_page > 0 {
            let per = self.progress() * self.chapters as f32;
            let idx = (per.ceil() as usize).clamp(1, self.chapters);
            return format!("卷 {idx} / {}", self.chapters);
        }
        if self.last_page > 0 {
            return format!("第 {} 面", self.last_page + 1);
        }
        "未始读".to_string()
    }
}

/// 四部归类关键词：先命中先归（表内顺序即优先级）。
/// 只收常见书，命中不了落到「集」——四部里集部最杂，做兜底最不容易错。
const SIKU: &[(&str, &[&str])] = &[
    (
        "经",
        &[
            "论语", "孟子", "诗经", "尚书", "周易", "礼记", "春秋", "左传", "公羊", "谷梁",
            "大学", "中庸", "孝经", "尔雅", "说文", "广韵", "千字文", "三字经", "百家姓",
            "弟子规", "声律启蒙", "笠翁对韵", "十三经", "四书", "五经",
        ],
    ),
    (
        "史",
        &[
            "史记", "汉书", "后汉书", "三国志", "资治通鉴", "通鉴", "战国策", "国语", "竹书",
            "通典", "通志", "文献通考", "明史", "宋史", "唐书", "元史", "清史", "汉书", "纪事本末",
            "水经注", "洛阳伽蓝", "东京梦华", "徐霞客",
        ],
    ),
    (
        "子",
        &[
            "道德经", "老子", "庄子", "列子", "荀子", "韩非", "墨子", "孙子", "吴子", "管子",
            "淮南", "论衡", "世说新语", "本草", "内经", "伤寒", "天工开物", "梦溪笔谈", "齐民要术",
            "红楼梦", "西游记", "水浒", "三国演义", "儒林外史", "聊斋", "山海经", "搜神记",
        ],
    ),
    (
        "集",
        &[
            "楚辞", "唐诗", "宋词", "元曲", "文选", "乐府", "诗集", "文集", "全集", "古文观止",
            "诗话", "词话", "陶渊明", "李白", "杜甫", "苏轼", "李清照", "辛弃疾",
        ],
    ),
];

/// 按书名自动归入四部之一。
pub fn auto_category(title: &str) -> &'static str {
    for (cat, keys) in SIKU {
        if keys.iter().any(|k| title.contains(k)) {
            return cat;
        }
    }
    "集"
}

/// 从「〔宋〕朱熹」里拆出（朝代, 人名）。支持〔〕[]（）() 四种括号。
pub fn split_dynasty(author: &str) -> (String, String) {
    let t = author.trim();
    for (open, close) in [('〔', '〕'), ('[', ']'), ('（', '）'), ('(', ')')] {
        if let Some(rest) = t.strip_prefix(open) {
            if let Some(i) = rest.find(close) {
                let dynasty = rest[..i].trim().to_string();
                let name = rest[i + close.len_utf8()..].trim().to_string();
                return (dynasty, name);
            }
        }
    }
    (String::new(), t.to_string())
}

/// 按字数截断并缀省略号（不足则原样）。
pub fn ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
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
        // 开发 / 测试态可用 JIANDU_HOME 覆盖（保留，大量测试依赖它）。
        if let Ok(p) = std::env::var("JIANDU_HOME") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        // 跨平台标准应用数据目录：macOS → ~/Library/Application Support。
        // App Sandbox 下系统会把这里透明重定向到容器
        // Data/Library/Application Support，所以同一份代码在「开发态 / 普通分发态 /
        // App Store 沙盒态」都能正确落盘，无需判环境。
        let data = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("com.jiandu.app");
        Self::migrate_legacy_home(&data);
        data
    }

    /// 首次启动把旧的 `~/.jiandu/` 数据迁移到新的标准目录。
    ///
    /// 只在「新目录尚不存在」时做一次（幂等）；保留旧目录不删除，失败静默忽略——
    /// 宁可多一份副本，也不冒丢失用户书架 / 阅读进度的险。
    fn migrate_legacy_home(new_home: &Path) {
        let legacy = match std::env::var("HOME") {
            Ok(h) => PathBuf::from(h).join(".jiandu"),
            Err(_) => return,
        };
        if !legacy.is_dir() || new_home.exists() {
            return;
        }
        let _ = Self::copy_dir(&legacy, new_home);
    }

    /// 递归拷贝目录（仅文件，不进符号链接环），用于旧数据一次性迁移。
    fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let dst = to.join(entry.file_name());
            if ft.is_dir() {
                Self::copy_dir(&entry.path(), &dst)?;
            } else if ft.is_file() {
                std::fs::copy(entry.path(), &dst)?;
            }
        }
        Ok(())
    }

    pub fn books_dir() -> PathBuf {
        Self::home().join("books")
    }

    /// 用户自备字库的**沙盒拖拽入口**：`com.jiandu.app/fonts/`。
    ///
    /// 把整部大字符集字体（覆盖 CJK 扩展 B 的花园明朝 / 思源宋体等，几十 MB 一部）
    /// 拖进这里即被 `src/fonts.rs::font_dirs` 扫到并注册，补主字体缺字的系统兜底。
    /// 默认**不**随安装包分发大字符集字库（会撑爆体积），用户按需自行下载放入。
    pub fn fonts_dir() -> PathBuf {
        Self::home().join("fonts")
    }

    pub fn index_path() -> PathBuf {
        Self::home().join("library.json")
    }

    /// 读取索引（过滤掉已被删除的文件，再把旧索引里的老书**并入**）。
    pub fn load() -> Self {
        let mut lib: Self = std::fs::read_to_string(Self::index_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        lib.books.retain(|b| b.path.exists());
        lib.absorb_legacy_index();
        lib
    }

    /// 把旧书架索引（`~/.jiandu/library.json`）里**还在、且当前索引里没有**
    /// 的书并入内存列表。
    ///
    /// 为什么需要：书架索引一直在 `Library::home()`（Application Support），
    /// 但 v1.69 及更早的版本用过 `~/.jiandu`。换过数据目录的用户，书架会
    /// 「凭空少掉一批书」——本机实测就是 8 本老书在新索引里一本不剩。
    ///
    /// **只读**：只往内存里的列表追加，一个文件都不写。落盘等用户下次正常
    /// 操作（导入 / 删书 / 翻页写进度）时自然发生——绝不擅自动用户的存档。
    /// 沙盒下容器里根本没有 `~/.jiandu`，读不到就直接返回，也不多要权限。
    fn absorb_legacy_index(&mut self) {
        let legacy = match std::env::var("HOME") {
            Ok(h) => PathBuf::from(h).join(".jiandu").join("library.json"),
            Err(_) => return,
        };
        if legacy == Self::index_path() {
            return; // 同一个文件（`JIANDU_HOME` 指过去时），别自己并自己。
        }
        let Ok(s) = std::fs::read_to_string(&legacy) else {
            return;
        };
        let Ok(old): Result<Self, _> = serde_json::from_str(&s) else {
            return;
        };
        let have: std::collections::HashSet<String> = self
            .books
            .iter()
            .map(|b| b.path.to_string_lossy().to_string())
            .collect();
        for b in old.books {
            // 只收「书稿文件还在」且「当前索引里没有」的——重复的一概不收。
            if b.path.exists() && !have.contains(&b.path.to_string_lossy().to_string()) {
                self.books.push(b);
            }
        }
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
    /// `chapter` 是当前读到的卷册标题，书架「当前卷册」直接显示它。
    pub fn touch(
        &mut self,
        idx: usize,
        page: usize,
        total: usize,
        script: &str,
        duplex: &str,
        chapter: &str,
    ) {
        if let Some(e) = self.books.get_mut(idx) {
            e.last_page = page;
            e.total_pages = total;
            e.script = script.to_string();
            e.duplex = duplex.to_string();
            e.last_read = now_secs();
            if !chapter.is_empty() {
                e.chapter_title = chapter.to_string();
            }
        }
        self.save();
    }

    /// 改一本书的归类。`cat` 传「经」「史」「子」「集」或自定书单名；
    /// 传空串恢复「按书名自动归类」。
    pub fn set_category(&mut self, idx: usize, cat: &str) {
        if let Some(e) = self.books.get_mut(idx) {
            e.category = cat.to_string();
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

    /// 保存一本书的书签（v1.53.0）。
    pub fn save_bookmarks(&mut self, idx: usize, bookmarks: &[Bookmark]) {
        if let Some(e) = self.books.get_mut(idx) {
            e.bookmarks = bookmarks.to_vec();
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
            category: String::new(),
            annotator: String::new(),
            edition: String::new(),
            chapters: book.chapters.len(),
            chapter_title: String::new(),
            bookmarks: Vec::new(),
            store_id: None,
        });
        self.save();
        Ok(self.books.len() - 1)
    }

    /// **流式导入的收尾**（v1.50.0）：后台线程已经把正文逐章落盘，
    /// 这里只负责「登记入架」——把骨架书稿（含 `store_id`）写成 json，
    /// 建一条书架条目。
    ///
    /// 之所以拆成两步：解析落盘在后台线程（百万字要跑几秒），
    /// 而 `Library` 归主线程所有，不能跨线程改。
    pub fn admit_streamed(
        &mut self,
        src: &Path,
        book: &Book,
        store_id: &str,
    ) -> Result<usize> {
        let dir = Self::books_dir();
        std::fs::create_dir_all(&dir).context("无法创建书架目录")?;
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("book")
            .to_string();
        let mut dst = dir.join(format!("{stem}.json"));
        if dst.exists() {
            let mut n = 2;
            loop {
                let cand = dir.join(format!("{stem}-{n}.json"));
                if !cand.exists() {
                    dst = cand;
                    break;
                }
                n += 1;
            }
        }
        // 骨架 json：题名 / 目录 / 通假表 / 眉批 + `store_id`，正文不入库
        // （正文在 `chapters.db` 里，按章取回）。
        crate::importer::write_book_json(book, &dst)?;

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
            category: String::new(),
            annotator: String::new(),
            edition: String::new(),
            chapters: book.chapters.len(),
            chapter_title: String::new(),
            store_id: Some(store_id.to_string()),
            bookmarks: Vec::new(),
        });
        self.save();
        Ok(self.books.len() - 1)
    }

    /// **首次启动的内置书目**（v1.56.0）：把内置全本《道德經》《唐詩三百首》写进书架。
    ///
    /// 只在书架为空时调一次（`Root::new`），之后用户爱删爱换都不再插手。
    /// 遍历 [`crate::content::BUNDLED_BOOKS`]，每本走与用户导入同一条清洗链路的
    /// 产物（运行时从 Resources/books 读取）写入书架，解析不出才退回旧手搓样张——
    /// 宁可书架有书，也别开天窗。返回**第一本**（《道德經》）的书架索引。
    pub fn seed_sample(&mut self) -> Result<usize> {
        let dir = Self::books_dir();
        std::fs::create_dir_all(&dir).context("无法创建书架目录")?;
        let mut first_idx = None;
        for name in crate::content::BUNDLED_BOOKS {
            let book = crate::content::bundled_book(name)
                .unwrap_or_else(crate::content::sample_dao_de_jing);
            let path = dir.join(format!("{name}.json"));
            if !path.exists() {
                let json = serde_json::to_string_pretty(&book)?;
                std::fs::write(&path, json).context("写入内置书目失败")?;
            }
            let idx = self.import_path(&path)?;
            first_idx.get_or_insert(idx);
        }
        Ok(first_idx.unwrap_or(0))
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
        // 样张在仓库根的 samples/（本 crate 在 core/，路径退一级）。
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../samples/daodejing-toc.epub");
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
        lib.touch(idx, 3, 20, "ancient", "single", "卷二 女诫");
        assert!(lib.books[idx].last_read > 0, "touch 应记下阅读时间");
        assert_eq!(lib.books[idx].last_page, 3);
        // 「当前卷册」随 touch 一起落档，书架不必开书就能显示。
        assert_eq!(lib.books[idx].chapter_title, "卷二 女诫");
        assert_eq!(lib.books[idx].volume_line(), "卷二 女诫");
        std::env::remove_var("JIANDU_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }

    // ---- 书架五项的取值逻辑（纯函数，不需要文件系统）----

    fn entry(title: &str, author: &str, category: &str) -> ShelfEntry {
        ShelfEntry {
            path: PathBuf::from("x.json"),
            title: title.to_string(),
            author: author.to_string(),
            accent: 0,
            seal: String::new(),
            last_page: 0,
            total_pages: 0,
            script: "ancient".into(),
            duplex: "single".into(),
            last_read: 0,
            notes: std::collections::HashMap::new(),
            underlines: Vec::new(),
            category: category.to_string(),
            annotator: String::new(),
            edition: String::new(),
            chapters: 0,
            chapter_title: String::new(),
            bookmarks: Vec::new(),
            store_id: None,
        }
    }

    #[test]
    fn auto_category_covers_siku() {
        assert_eq!(auto_category("论语"), "经");
        assert_eq!(auto_category("资治通鉴"), "史");
        assert_eq!(auto_category("庄子"), "子");
        assert_eq!(auto_category("楚辞"), "集");
        // 收不进去的落到集部兜底，而不是留空。
        assert_eq!(auto_category("某未知名目"), "集");
        // 千字文属小学，归经部。
        assert_eq!(auto_category("千字文"), "经");
    }

    #[test]
    fn category_override_wins_and_custom_lists_go_to_ziding() {
        // 空串 = 自动归类。
        let e = entry("论语", "", "");
        assert_eq!(e.cat_key(), "经");
        // 用户指定覆盖自动结果。
        let e = entry("论语", "", "史");
        assert_eq!(e.cat_key(), "史");
        // 自定书单名：进「自定」页签（名字原样留在 category 里）。
        let e = entry("论语", "", "床头常读");
        assert_eq!(e.cat_key(), "自定");
        assert_eq!(e.category, "床头常读");
    }

    #[test]
    fn dynasty_is_split_out_of_author() {
        assert_eq!(
            split_dynasty("〔宋〕朱熹"),
            ("宋".to_string(), "朱熹".to_string())
        );
        assert_eq!(
            split_dynasty("[清]曹雪芹"),
            ("清".to_string(), "曹雪芹".to_string())
        );
        assert_eq!(split_dynasty("佚名"), (String::new(), "佚名".to_string()));
        // 括号没闭合就当没有朝代，原样留作人名，别把整串吃掉。
        assert_eq!(
            split_dynasty("〔宋朱熹"),
            (String::new(), "〔宋朱熹".to_string())
        );
    }

    #[test]
    fn five_book_fields_have_sane_fallbacks() {
        // 作者行：朝代标注另列；注者缀「注」。
        let mut e = entry("论语", "〔魏〕何晏", "");
        assert_eq!(e.author_line(), "何晏");
        e.annotator = "朱熹".into();
        assert_eq!(e.author_line(), "何晏 朱熹 注");
        // 版本行：取不到版本就用朝代。
        assert_eq!(e.edition_line(), "〔魏〕");
        e.edition = "宋刻本".into();
        assert_eq!(e.edition_line(), "宋刻本");
        // 导入时截断留下的 `_ …` 尾巴要清掉，别让书架显示毛刺。
        let e = entry("资治通鉴", "（宋）司马光 著 _ …", "");
        assert_eq!(e.author_line(), "司马光 著");
        // 无名无朝代的书：作者佚名、版本按入库形态（json=整理本）。
        let e = entry("残卷", "", "");
        assert_eq!(e.author_line(), "佚名");
        assert_eq!(e.edition_line(), "整理本");
        // 卷册：没读过 = 未始读。
        assert_eq!(e.volume_line(), "未始读");
    }

    #[test]
    fn volume_line_truncates_long_chapter_titles() {
        let mut e = entry("某书", "", "");
        e.chapter_title = "第一卷 序言与凡例总目提要".into();
        let v = e.volume_line();
        assert!(v.ends_with('…'), "过长卷名应截断：{v}");
        assert_eq!(v.chars().count(), 9);
        // 没记卷名但读过：按进度在总章数里估一卷。
        let mut e = entry("某书", "", "");
        e.chapters = 10;
        e.last_page = 4;
        e.total_pages = 10;
        assert_eq!(e.volume_line(), "卷 5 / 10");
    }

    /// **首启内置书目**：空书架种下去的是全本《道德經》，目录两级不丢。
    #[test]
    fn seed_sample_puts_the_bundled_book_on_the_shelf() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = temp_home("seed");
        std::env::set_var("JIANDU_HOME", &home);
        let mut lib = Library::load();
        assert!(lib.books.is_empty(), "临时书架空");
        let idx = lib.seed_sample().expect("首启应种上内置书目");
        assert_eq!(lib.books[idx].title, "道德經");
        assert_eq!(lib.books[idx].chapters, 81, "八十一章");
        assert_eq!(lib.books[idx].author, "〔周〕李耳");
        // 走的是 import_path（即与用户导入同一条清洗链路），目录不能被重建成一摊。
        let raw = std::fs::read_to_string(&lib.books[idx].path).unwrap();
        let book: Book = serde_json::from_str(&raw).unwrap();
        assert_eq!(book.chapters.len(), 81, "清洗后章数不变");
        assert_eq!(
            book.toc.iter().filter(|e| e.level == 1).count(),
            2,
            "道經 / 德經 两篇应保住"
        );
        std::env::remove_var("JIANDU_HOME");
        let _ = std::fs::remove_dir_all(&home);
    }
}
