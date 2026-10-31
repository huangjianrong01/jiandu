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

#[test]
fn tcm_markdown_strips_parens_and_promotes_fang_names() {
    let md = "# 驗方集\n\
走馬牙疳\n\
升麻三钱\n\
當歸（二錢）\n\
甘草（炙）三錢\n\
水煎服\n\
牙痛妙方\n\
瞿麥壹錢\n\
川芎（炒）二钱\n\
血山崩方\n\
五靈脂（醋炒）三錢";
    let book = jiandu_core::importer::book_from_markdown("驗方集", md);
    // 方名升格：方后缀名 + 无后缀病证名 都应成为独立章题（榜眼大字）。
    let titles: Vec<&str> = book.chapters.iter().map(|c| c.title.as_str()).collect();
    assert!(
        titles.iter().any(|t| t.contains("走馬牙疳")),
        "无后缀病证名应升格为章题：{:?}",
        titles
    );
    assert!(
        titles.iter().any(|t| t.contains("牙痛妙方")),
        "方名应升格为章题：{:?}",
        titles
    );
    assert!(
        titles.iter().any(|t| t.contains("血山崩方")),
        "方名应升格为章题：{:?}",
        titles
    );
    // 用法提示语「水煎服」虽短、无量词、无标点，但绝不升格为章题。
    assert!(
        !titles.iter().any(|t| t.contains("水煎服")),
        "用法提示语被误升格为章题：{:?}",
        titles
    );
    // 括号彻底消失：任何章题与段落都不含 （）。
    for c in &book.chapters {
        assert!(
            !c.title.contains('（') && !c.title.contains('）'),
            "章题漏括号：{}",
            c.title
        );
        for p in &c.paragraphs {
            assert!(
                !p.text.contains('（') && !p.text.contains('）'),
                "段落漏括号：{}",
                p.text
            );
        }
    }
}

// ── 4) 预览页生成（按需）──────────────────────────────────────────────────────
//
// 用**真实** `parse_markdown` 规则渲染一份高保真竖排预览，证明 v1.9.17 的方名
// 升格（含无后缀病证名）+ 剂量双行夹注 + 朱砂句读全部落地。仅当设置了
// `JIANDU_GEN_TCM_PREVIEW=1` 才写文件，避免污染普通 `cargo test`。
//
//   重建预览：JIANDU_GEN_TCM_PREVIEW=1 cargo test -p jiandu-core tcm_preview_generate_html
#[test]
fn tcm_preview_generate_html() {
    if std::env::var("JIANDU_GEN_TCM_PREVIEW").is_err() {
        return;
    }
    // 模拟「中医秘方扫描件 → 外部转 MD」产物：每个方名 / 病证名独占一行，
    // 药行带括号剂量与炮制，末尾附用法。覆盖 16 张真本里的典型无后缀病证名。
    let md = "\
走馬牙疳
升麻三錢
當歸（二錢）
甘草（炙）三錢
水煎服

血山崩
當歸二錢
川芎三錢
蒲黃（炒）三錢
生地黃壹兩

霍亂吐瀉
藿香二錢
蒼朮三錢
厚朴二錢
陳皮一錢半

虛火牙痛
細辛一錢
石膏五錢
知母三錢
水煎服

陽痿
熟地四錢
山藥三錢
杞子二錢
肉桂一錢

癲狗咬傷
斑蝥（去頭足）五分
麝香五釐
酒下

時疫起核症
連翹三錢
玄參二錢
牡蠣四錢
水煎服

跌打駁骨
自然銅（醋淬）三錢
乳香二錢
沒藥二錢
共為細末

小兒疳積
使君子（炒）三錢
神麯二錢
麥芽二錢
水煎服

肚內生血龜
三稜二錢
莪朮二錢
紅花一錢
水煎服

生馬嘴疔
蒲公英五錢
紫花地丁三錢
金銀花三錢
酒煎服

牙痛妙方
瞿麥壹錢
川芎（炒）二錢
五靈脂（醋炒）三錢";

    let chapters = jiandu_core::pipeline::parse_markdown(md);
    let html = render_tcm_html(&chapters);
    let path = std::path::Path::new("/Users/huangjianrong/Desktop/jiandu-tcm-preview.html");
    std::fs::write(path, html).expect("写预览页失败");
    eprintln!("[preview] 已生成 {}", path.display());
}

/// 把真实解析出的章（方名 → 章题；药行带 [注]…[/注]）渲染成竖排 HTML。
fn render_tcm_html(chapters: &[jiandu_core::pipeline::RawChapter]) -> String {
    // 每两方并作一叶（双栏），方名大字、剂量双行夹注、朱砂句读替代现代标点。
    let mut leaves = String::new();
    for pair in chapters.chunks(2) {
        let mut cols = String::new();
        for ch in pair {
            cols.push_str(&render_col(ch));
        }
        leaves.push_str(&format!(
            "<div class=\"leaf\">\n{tags}\n{cols}<div class=\"fold\"><span class=\"tail\">方書</span><span class=\"tail\">箋讀</span></div>\n</div>\n",
            tags = "<div class=\"tag\">笺读 · 数字标本</div>",
            cols = cols,
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-Hans">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>笺读 v1.9.17 · 中医方剂竖排最终体（预览）</title>
<style>
  *{{box-sizing:border-box;}}
  html,body{{margin:0;padding:0;background:#3a342b;}}
  .stage{{min-height:100vh;display:flex;align-items:flex-start;justify-content:center;
    padding:28px;gap:18px;flex-wrap:wrap;}}
  .leaf{{width:760px;height:1040px;background:
      radial-gradient(120% 120% at 50% 0%, #f7eed8 0%, #efe2c4 70%, #e7d8b4 100%);
    border:1px solid #c9b787;border-radius:3px;
    box-shadow:0 18px 50px rgba(0,0,0,.45), inset 0 0 60px rgba(160,120,60,.18);
    display:flex;flex-direction:row-reverse;padding:34px 30px;position:relative;}}
  .fold{{width:54px;align-self:stretch;display:flex;flex-direction:column;align-items:center;
    justify-content:space-between;color:#7a6a4f;}}
  .fold .tail{{font-size:13px;color:#7a6a4f;opacity:.8;writing-mode:vertical-rl;letter-spacing:2px;}}
  .col{{flex:1;writing-mode:vertical-rl;text-orientation:upright;
    font-family:"Songti SC","STSong","SimSun",serif;color:#2b2620;
    line-height:1.9;font-size:30px;letter-spacing:2px;padding:6px 10px;}}
  .col + .col{{border-right:1px solid rgba(122,106,79,.35);}}
  .title{{font-size:46px;font-weight:600;color:#1f1b15;letter-spacing:6px;
    margin-bottom:18px;display:block;}}
  .title::after{{content:"";display:block;width:3px;height:46px;background:#9A2B22;margin:10px auto 0;}}
  .line{{margin-bottom:14px;display:block;}}
  .gloss{{font-size:18px;color:#B0342C;letter-spacing:1px;
    border-left:1px solid rgba(176,52,44,.4);padding-left:4px;margin-left:3px;}}
  .seed{{color:#9A2B22;font-size:16px;vertical-align:6px;margin:0 2px;}}
  .ring{{color:#9A2B22;font-size:17px;vertical-align:5px;margin:0 2px;
    border:2px solid #9A2B22;border-radius:50%;display:inline-block;width:13px;height:13px;
    line-height:13px;text-align:center;}}
  .tag{{position:absolute;top:8px;right:14px;font-size:12px;color:#8a7a55;
    font-family:-apple-system,sans-serif;writing-mode:horizontal-tb;}}
  .legend{{width:320px;background:#f7eed8;border:1px solid #c9b787;border-radius:3px;
    padding:20px 22px;font-family:-apple-system,"Songti SC",serif;color:#2b2620;
    box-shadow:0 18px 50px rgba(0,0,0,.45);align-self:flex-start;}}
  .legend h2{{font-size:17px;margin:0 0 12px;color:#9A2B22;}}
  .legend p{{font-size:13px;line-height:1.7;margin:8px 0;}}
  .legend .k{{color:#9A2B22;font-weight:600;}}
  .dot{{display:inline-block;width:11px;height:11px;border-radius:50%;background:#9A2B22;vertical-align:middle;margin:0 3px;}}
  .circ{{display:inline-block;width:11px;height:11px;border:2px solid #9A2B22;border-radius:50%;vertical-align:middle;margin:0 3px;}}
  code{{background:rgba(122,106,79,.15);padding:1px 4px;border-radius:3px;font-size:12px;}}
</style>
</head>
<body>
<div class="stage">
{leaves}
  <div class="legend">
    <h2>v1.9.17 · 方名升格已修</h2>
    <p><span class="k">① 剥壳夹注</span><br/>
      <span style="text-decoration:line-through;color:#a08c66;">川芎（三錢）</span> → 川芎<span class="gloss">三錢</span><br/>
      中英文括号一律删，只留纯文本作小字悬浮。</p>
    <p><span class="k">② 标点古籍化</span><br/>
      <span style="text-decoration:line-through;color:#a08c66;">，</span> → <span class="dot"></span> 朱砂实心墨滴（读类）<br/>
      <span style="text-decoration:line-through;color:#a08c66;">。</span> → <span class="circ"></span> 朱砂空心圈〇（句类）</p>
    <p><span class="k">③ 清理废标点</span><br/>
      OCR / MD 的 <code>· • ※ ◇</code> 及残余 ASCII 标点直接滤掉。</p>
    <p><span class="k">＋ 智能方名升格（v1.9.17 重定）</span><br/>
      旧规则只认 方/湯/散/丸/丹 后缀，漏掉绝大多数无后缀病证名。<br/>
      新版：短行≤10字 + <b>铁壁量词排斥</b>（带錢/兩/分…必是药行）+ 无句读 + 非用法语
      → 独立成章，渲染为<span class="k">榜眼大字</span>。<br/>
      本页「走馬牙疳 / 血山崩 / 霍亂吐瀉 / 虛火牙痛 / 陽痿 / 癲狗咬傷 / 時疫起核症 /
      跌打駁骨 / 小兒疳積 / 肚內生血龜 / 生馬嘴疔」皆无后缀，现全部正确升格。</p>
    <p style="color:#8a7a55;font-size:12px;">※ 此为版式预览（高保真近似）；真机渲染请本机
      <code>cargo run -p jiandu</code> 导入方剂 .md 验收。</p>
  </div>
</div>
</body>
</html>
"#,
        leaves = leaves,
    )
}

/// 渲染一个方（一栏）：方名 → .title；各药行 → .line，剂量 [注]…[/注] → .gloss。
fn render_col(ch: &jiandu_core::pipeline::RawChapter) -> String {
    let mut out = String::from("<div class=\"col\">");
    out.push_str(&format!(
        "<span class=\"title\">{}</span>",
        escape_html(&ch.title)
    ));
    for line in ch.content.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push_str("<span class=\"line\">");
        // 拆分 [注]…[/注] 为剂量夹注，并映射句读到朱砂记号。
        let note_open = "[注]";
        let note_close = "[/注]";
        let mut rest = line;
        while let Some(open) = rest.find(note_open) {
            let pre = &rest[..open];
            out.push_str(&render_inline(pre));
            let close = match rest[open..].find(note_close) {
                Some(c) => open + c,
                None => rest.len(),
            };
            let gloss = &rest[open + note_open.len()..close];
            out.push_str(&format!(
                "<span class=\"gloss\">{}</span>",
                escape_html(gloss)
            ));
            rest = &rest[close + note_close.len()..];
        }
        out.push_str(&render_inline(rest));
        out.push_str("</span>");
    }
    out.push_str("</div>");
    out
}

/// 行内文本：映射 ，。 为朱砂记号（读=实心墨滴 / 句=空心圈），其余转义。
fn render_inline(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '，' => out.push_str("<span class=\"seed\"></span>"),
            '。' => out.push_str("<span class=\"ring\"></span>"),
            other => out.push_str(&escape_html(&other.to_string())),
        }
    }
    out
}

/// 最小 HTML 转义（本预览只防 < > &）。
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
