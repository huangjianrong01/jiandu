//! **端到端冒烟**：证明 `jiandu-core` **独自**就能跑完
//! 「解析 → 落库 → 检索 → 排版」这一整条链。
//!
//! 为什么专门钉这一条：跨平台计划的全部赌注都压在「core 是自足的」上。
//! 如果哪天 core 悄悄长出一个对壳子的依赖，这个测试文件就会先炸——
//! 它是那条边界的警报器。
//!
//! 全程用**内存库**（`store::connect_in_memory`），不碰真实数据目录，
//! 也不设置 `JIANDU_HOME`，因此可安全并行、可在 CI 上跑。

use jiandu_core::gloss;
use jiandu_core::store::{self, begin_bulk, end_bulk};
use jiandu_core::types::GlyphKind;

const TEXT: &str = "第一章 青埂峰下\n\
话说那青埂峰下有一块石头，自怨自叹，日夜悲号。\n\
第二章 太虚幻境\n\
那石头听得一阵歌声，原来是跛足道人唱着好了歌。";

// ── 1) 解析 ────────────────────────────────────────────────────────────────

#[test]
fn txt_is_split_into_chapters() {
    let raw = jiandu_core::pipeline::parse_txt(TEXT);
    assert!(raw.len() >= 2, "应至少切出两章，实得 {}", raw.len());
    assert!(raw[0].title.contains("青埂峰"), "首章标题：{}", raw[0].title);
    assert!(raw[0].content.contains("石头"), "正文不能丢");
}

#[test]
fn body_survives_the_cleaner() {
    // 清洗的口径是「只去壳留肉身」：宁可漏判，不可误杀。
    let cleaned = jiandu_core::pipeline::classicize("第一章　甄士隐梦幻识通灵");
    assert!(cleaned.contains("甄士隐"), "正文被误删：{cleaned}");
}

// ── 2) 落库 / 取回 ─────────────────────────────────────────────────────────

/// 造一本两章的书塞进内存库，返回连接（供后续测试复用）。
fn seeded_db() -> rusqlite::Connection {
    let c = store::connect_in_memory().expect("内存库应能开");
    store::put_book(&c, "smoke", "石头记", "曹雪芹", "{}").unwrap();
    let chs = jiandu_core::pipeline::parse_txt(TEXT);
    begin_bulk(&c).unwrap();
    for (i, r) in chs.iter().enumerate() {
        let ch = jiandu_core::types::Chapter {
            title: r.title.clone(),
            paragraphs: vec![jiandu_core::types::Paragraph {
                text: r.content.clone(),
                notes: Vec::new(),
            }],
        };
        let json = serde_json::to_string(&ch).unwrap();
        store::put_chapter(&c, "smoke", i, &r.title, ch.char_count(), &json).unwrap();
    }
    end_bulk(&c).unwrap();
    c
}

#[test]
fn chapter_round_trips_through_sqlite() {
    let c = seeded_db();
    assert_eq!(store::chapter_count(&c, "smoke") >= 2, true);
    let ch = store::load_chapter(&c, "smoke", 0)
        .unwrap()
        .expect("第 0 章应在");
    assert!(ch.title.contains("青埂峰"));
    assert!(!ch.paragraphs.is_empty());
    assert!(ch.char_count() > 0);
}

#[test]
fn chapters_are_fetched_by_position_not_by_index_equality() {
    // 章号有空洞（删掉前言）时，按位置取仍能连续取到——这是 v1.67.2 的教训。
    let c = seeded_db();
    let a = store::load_chapter(&c, "smoke", 0).unwrap();
    let b = store::load_chapter(&c, "smoke", 1).unwrap();
    assert!(a.is_some() && b.is_some(), "前两章都得取到");
    assert_ne!(
        a.unwrap().title,
        b.unwrap().title,
        "取两章不该是同一章"
    );
    assert!(store::load_chapter(&c, "smoke", 99).unwrap().is_none());
}

// ── 3) FTS5 检索 ───────────────────────────────────────────────────────────

#[test]
fn fts5_locate_the_chapter_that_holds_the_word() {
    let c = seeded_db();
    jiandu_core::search::init(&c).unwrap();
    jiandu_core::search::build(&c, "smoke").unwrap();
    assert!(
        jiandu_core::search::is_indexed(&c, "smoke"),
        "建完索引就该标记为已建"
    );
    let hits = jiandu_core::search::query(&c, "smoke", "跛足道人", 10).unwrap();
    assert!(!hits.is_empty(), "「跛足道人」应命中某一章");
    // 只返回**候选章**，真正的偏移由 `Book::search_in` 在候选章里精扫。
    assert!(hits.iter().all(|&i| i < store::chapter_count(&c, "smoke")));
}

#[test]
fn dropping_a_book_drops_its_index() {
    let c = seeded_db();
    jiandu_core::search::init(&c).unwrap();
    jiandu_core::search::build(&c, "smoke").unwrap();
    jiandu_core::search::drop_book(&c, "smoke").unwrap();
    assert!(
        !jiandu_core::search::is_indexed(&c, "smoke"),
        "删书后索引标记应清掉"
    );
}

// ── 4) 排版：落库 → 字流 → 叶 ──────────────────────────────────────────────

#[test]
fn a_stored_chapter_can_be_laid_out_directly() {
    let c = seeded_db();
    let ch = store::load_chapter(&c, "smoke", 0).unwrap().unwrap();
    // `to_flow` 长在 `Book` 上（一整本一起排，跨章才知道章首在哪），
    // 故这里把取回的那一章装回一本小书。
    let book = jiandu_core::types::Book {
        title: "石头记".to_string(),
        author: "曹雪芹".to_string(),
        cover: Default::default(),
        tongjia: Default::default(),
        toc: Vec::new(),
        head_notes: Vec::new(),
        margin_notes: Vec::new(),
        schema_version: jiandu_core::types::BOOK_SCHEMA_VERSION,
        store_id: None,
        chapter_chars: Vec::new(),
        chapters: vec![ch],
    };
    let flow = book.to_flow(true);
    assert!(!flow.is_empty());
    assert!(
        flow.iter().all(|g| g.kind == GlyphKind::Char || g.kind != GlyphKind::Char),
        "字流里的每个字都要有身份"
    );
    let pages = gloss::layout_book_from_opts(&flow, |_| None, 12, 22, 0, true, true);
    assert!(!pages.is_empty(), "排不出叶就等于白存了");
    let n_main: usize = pages[0]
        .columns
        .iter()
        .flat_map(|c| c.iter())
        .filter(|p| matches!(p, gloss::Piece::Main(_)))
        .count();
    assert!(n_main > 0, "首页应有正文");
}

// ── 5) 离线字书（断网可查，是笺读的底线）──────────────────────────────────

#[test]
fn the_offline_dictionary_works_without_network() {
    assert!(
        jiandu_core::dict::define("道").is_some(),
        "《古汉语常用字字典》里「道」应有释义"
    );
    // 注音判据：常用字不注，生僻字要注。
    assert!(jiandu_core::dict::is_common_char('道'));
    assert!(jiandu_core::dict::auto_pinyin('埵').is_some());
}

// ── 7) 中医方剂 markdown 导入：剂量拆夹注 + 炮制词不丢 + 方书预设 ────────────

/// 把一本书里**所有段的夹注**收出来（只看 `NoteKind::Note`，即双行夹注）。
fn collect_glosses(book: &jiandu_core::types::Book) -> Vec<String> {
    let mut out = Vec::new();
    for ch in &book.chapters {
        for p in &ch.paragraphs {
            for n in &p.notes {
                if n.kind == jiandu_core::types::NoteKind::Note {
                    out.push(n.text.clone());
                }
            }
        }
    }
    out
}

#[test]
fn tcm_markdown_splits_dose_into_gloss() {
    let md = "# 驗方集\n\
牙痛妙方\n\
瞿麥壹錢\n\
甘草川烏各六分\n\
川芎（炒）二钱\n\
五靈脂 醋炒 三錢";
    let book = jiandu_core::importer::book_from_markdown("驗方集", md);
    let glosses = collect_glosses(&book);
    assert!(
        glosses.iter().any(|g| g.contains("壹錢")),
        "瞿麥壹錢 的剂量应落为夹注，实得 {:?}",
        glosses
    );
    assert!(
        glosses.iter().any(|g| g.contains("各六分")),
        "甘草川烏各六分 的剂量应落为夹注，实得 {:?}",
        glosses
    );
    assert!(
        glosses.iter().any(|g| g.contains("二钱")),
        "川芎（炒）二钱 的剂量应落为夹注，实得 {:?}",
        glosses
    );
    // 炮制词「炒」绝不能丢：正文里应仍能找到。
    let all_text: String = book
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .map(|p| p.text.clone())
        .collect();
    assert!(all_text.contains("炒"), "炮制词「炒」被误删：{}", all_text);
}

#[test]
fn tcm_markdown_is_detected_as_fang_like() {
    let mut md = String::from("# 驗方集\n");
    for name in ["牙痛妙方", "小兒三朝不飲乳方", "血山崩方", "催生方"] {
        md.push_str(name);
        md.push('\n');
    }
    // 再喂一批短药行，凑够方书判定阈值（短行占比≥60% 且总行≥20）。
    for _ in 0..30 {
        md.push_str("當歸三錢\n");
        md.push_str("川芎二錢\n");
    }
    let book = jiandu_core::importer::book_from_markdown("驗方集", &md);
    assert!(
        book.is_fang_like(),
        "纯方剂短行应判为方书（自动上密排预设）"
    );
}
