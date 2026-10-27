//! 内置样张：《道德经》第一 ~ 三章（含封面、注释与通假标注）。

use crate::types::{Book, Chapter, Cover, Note, NoteKind, Paragraph, TocEntry};
use std::path::PathBuf;

/// 书库目录项：内置可「下载」的一本书（随二进制分发，离线可用）。
pub struct CatalogItem {
    pub key: &'static str,
    pub title: String,
    pub author: String,
    pub subtitle: String,
    pub book: Book,
}

/// 内置书库：列出可下载的书。下载即写入书架 books/ 并加入索引。
pub fn catalog() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            key: "daodejing",
            title: "道德经".into(),
            author: "老子".into(),
            subtitle: "八十一章 · 五千言 · 内置样张".into(),
            book: sample_dao_de_jing(),
        },
        CatalogItem {
            key: "sanzi",
            title: "三字经".into(),
            author: "王应麟".into(),
            subtitle: "蒙学经典 · 试读样张".into(),
            book: sample_san_zi_jing(),
        },
        CatalogItem {
            key: "qianzi",
            title: "千字文".into(),
            author: "周兴嗣".into(),
            subtitle: "四言韵语 · 试读样张".into(),
            book: sample_qian_zi_wen(),
        },
    ]
}

/// 按 key 取一本书（书库下载用）。
pub fn catalog_book(key: &str) -> Option<Book> {
    catalog().into_iter().find(|c| c.key == key).map(|c| c.book)
}

/// **首启内置书目**（v1.56.0）。
///
/// 苹果审核必看：装完第一次打开不能是空书架，得有翻得动的真书。当前内置两本：
/// 《道德經》（老子，八十一章）与《唐詩三百首》（蘅塘退士輯，七體三百餘詩）。
///
/// 资源加载走**运行时读 Resources**（不再 `include_str!`）：打包脚本会把
/// `resources/books/*.json` 整目录拷进 `.app/Contents/Resources/books/`，App
/// 启动按需读取。好处是以后随时往这个目录丢一本新 JSON 重新打包即可「热更新」
/// 书目，不用改代码、不用重编二进制。
///
/// 书籍 JSON 本身仍由 `jiandu --convert` 走与用户导入完全相同的清洗链路产出，
/// 公有领域、无版权风险、离线可用。
///
/// 文件名 stem 即书名 key（`daodejing` / `tangshi`），与 [`BUNDLED_BOOKS`] 对应。
pub const BUNDLED_BOOKS: &[&str] = &["daodejing", "tangshi"];

/// 按文件名 stem 读取内置书目（运行期从 Resources/books 取，不编进二进制）。
///
/// 查找顺序（任一命中即返回）：
/// 1. `.app/Contents/Resources/books/<name>.json` —— 打包后正式运行；
/// 2. 沿可执行文件向上回溯 `resources/books/<name>.json` —— 开发 / 测试态
///    （`target/{debug,release}/jiandu` 一路回溯到仓库根 `resources`）；
/// 3. `CARGO_MANIFEST_DIR/resources/books` —— core crate 编译期目录兜底
///    （workspace 化后资源随 core 走：`crates/core/resources/books`）。
pub fn bundled_book(name: &str) -> Option<Book> {
    let file = format!("{name}.json");
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            // `.app/Contents/MacOS/<exe>` → `../Resources/books`
            candidates.push(p.join("../Resources/books").join(&file));
            // 开发 / 测试态：沿可执行文件逐层向上的 `resources/books`
            let mut d = Some(p.to_path_buf());
            while let Some(dir) = d {
                candidates.push(dir.join("resources/books").join(&file));
                // workspace 化后的新位置（仓库根往上是找不到的）
                candidates.push(dir.join("crates/core/resources/books").join(&file));
                d = dir.parent().map(|x| x.to_path_buf());
            }
        }
    }
    if let Ok(man) = std::env::var("CARGO_MANIFEST_DIR") {
        candidates.push(PathBuf::from(&man).join("resources/books").join(&file));
    }
    for p in &candidates {
        if let Ok(txt) = std::fs::read_to_string(p) {
            if let Ok(book) = serde_json::from_str::<Book>(&txt) {
                return Some(book);
            }
        }
    }
    None
}

/// 首启内置《道德經》全本（繁体，八十一章）。
pub fn bundled_dao_de_jing() -> Option<Book> {
    bundled_book("daodejing")
}

/// 首启内置《唐詩三百首》全本（繁体，按詩體分七類、三百餘首）。
pub fn bundled_tangshi() -> Option<Book> {
    bundled_book("tangshi")
}

pub fn sample_dao_de_jing() -> Book {
    let mut book = Book::new("道德经", "老子");
    book.cover = Cover {
        title: "道德经".into(),
        subtitle: "八十一章 · 五千言".into(),
        seal: "笺读".into(),
        accent: 0xB4452F,
    };
    // 典籍常见通假
    book.tongjia.insert("见".into(), "现".into());
    book.tongjia.insert("知".into(), "智".into());

    book.chapters.push(Chapter {
        title: "第一章".into(),
        paragraphs: vec![
            Paragraph::with_notes(
                "道可道，非常道。名可名，非常名。无名天地之始；有名万物之母。",
                vec![
                    Note::new(0, NoteKind::Note, "道：宇宙万物的本原，兼指其运行规律。"),
                    Note::new(3, NoteKind::Note, "可道：可以用言语表述。"),
                    // 106 字长注：一列放不下，自动跨列流到下一列列首（双行夹注演示）。
                    Note::new(
                        12,
                        NoteKind::Note,
                        "名：称谓、概念。凡有称谓者，皆非恒常之名；老子虑人执名以求道，故破名相，而欲其实践无为之玄览。名起于分别，分别起于对待，对待既立而道隐矣。此句与上章道可道相映，一破道名、一破事物之名，合而观之，乃知有无二谛之妙。",
                    ),
                    // 专名 / 书名线演示：天地、万物画地名线，「道」字所在的篇名画书名线。
                    Note::span(0, 1, NoteKind::Book, "道"),
                    Note::span(18, 2, NoteKind::Place, "天地"),
                    Note::span(25, 2, NoteKind::Place, "万物"),
                ],
            ),
            Paragraph::with_notes(
                "故常无欲，以观其妙；常有欲，以观其徼。",
                vec![Note::new(15, NoteKind::Note, "徼（jiào）：边际、端倪。")],
            ),
            Paragraph::with_notes(
                "此两者同出而异名，同谓之玄。玄之又玄，众妙之门。",
                vec![Note::new(13, NoteKind::Note, "玄：幽深微妙，不可名状。")],
            ),
        ],
    });

    book.chapters.push(Chapter {
        title: "第二章".into(),
        paragraphs: vec![
            Paragraph::plain("天下皆知美之为美，斯恶已。皆知善之为善，斯不善已。"),
            Paragraph::with_notes(
                "故有无相生，难易相成，长短相较，高下相倾，音声相和，前后相随。",
                vec![Note::new(0, NoteKind::Note, "相生：相互依存而生。六句并举例说明对立相成之理。")],
            ),
            Paragraph::plain(
                "是以圣人处无为之事，行不言之教；万物作焉而不辞，生而不有，为而不恃，功成而弗居。",
            ),
            Paragraph::plain("夫唯弗居，是以不去。"),
        ],
    });

    book.chapters.push(Chapter {
        title: "第三章".into(),
        paragraphs: vec![
            Paragraph::with_notes(
                "不尚贤，使民不争；不贵难得之货，使民不为盗；不见可欲，使民心不乱。",
                vec![
                    Note::new(0, NoteKind::Note, "尚贤：推崇贤能之名。"),
                    Note::new(24, NoteKind::Tongjia, "见，通「现」，显露。"),
                ],
            ),
            Paragraph::plain("是以圣人之治，虚其心，实其腹，弱其志，强其骨。"),
            Paragraph::plain("常使民无知无欲，使夫智者不敢为也。为无为，则无不治。"),
        ],
    });

    // 三级目录：上篇（卷）→ 章 → 章首名句，目录侧栏会逐级缩进呈现。
    book.toc = vec![
        TocEntry::new("道经 · 上篇", 0, 1),
        TocEntry::new("第一章", 0, 2),
        TocEntry::new("道可道，非常道", 0, 3),
        TocEntry::new("第二章", 1, 2),
        TocEntry::new("天下皆知美之为美", 1, 3),
        TocEntry::new("第三章", 2, 2),
        TocEntry::new("不尚贤，使民不争", 2, 3),
    ];

    book
}

/// 《三字经》试读样张（开篇数则，公版蒙学）。
fn sample_san_zi_jing() -> Book {
    let mut book = Book::new("三字经", "王应麟");
    book.cover = Cover {
        title: "三字经".into(),
        subtitle: "蒙学经典 · 试读样张".into(),
        seal: "笺读".into(),
        accent: 0x4E5D51,
    };
    book.chapters.push(Chapter {
        title: "劝学".into(),
        paragraphs: vec![
            Paragraph::with_notes(
                "人之初，性本善。性相近，习相远。",
                // 68 字长注：演示双行夹注跨列流转。
                vec![Note::new(
                    0,
                    NoteKind::Note,
                    "人之初：人初生之时。性本善：孟子道性善，谓恻隐、羞恶、辞让、是非四端，人皆有之，故曰性善。然气质有偏，嗜欲相攻，苟不教而习于不善，则与禽兽相去无几矣。此三字纲领全书，以下皆推广教之之说。",
                )],
            ),
            Paragraph::with_notes(
                "苟不教，性乃迁。教之道，贵以专。",
                vec![Note::new(8, NoteKind::Note, "专：纯一不杂，始终不变之谓。")],
            ),
            // 人名线演示：孟母、窦燕山各画一条通名实线。
            Paragraph::with_notes(
                "昔孟母，择邻处。子不学，断机杼。",
                vec![Note::span(1, 2, NoteKind::Person, "孟母")],
            ),
            Paragraph::with_notes(
                "窦燕山，有义方。教五子，名俱扬。",
                vec![Note::span(0, 3, NoteKind::Person, "窦燕山")],
            ),
            Paragraph::plain("养不教，父之过。教不严，师之惰。"),
            Paragraph::plain("子不学，非所宜。幼不学，老何为。"),
        ],
    });
    book.chapters.push(Chapter {
        title: "明义".into(),
        paragraphs: vec![
            Paragraph::plain("玉不琢，不成器。人不学，不知义。"),
            Paragraph::plain("为人子，方少时。亲师友，习礼仪。"),
            Paragraph::plain("香九龄，能温席。孝于亲，所当执。"),
            Paragraph::plain("融四岁，能让梨。弟于长，宜先知。"),
        ],
    });
    book.chapters.push(Chapter {
        title: "训蒙".into(),
        paragraphs: vec![
            Paragraph::plain("首孝悌，次见闻。知某数，识某文。"),
            Paragraph::plain("一而十，十而百。百而千，千而万。"),
            Paragraph::plain("三才者，天地人。三光者，日月星。"),
        ],
    });
    book.rebuild_toc_from_chapters();
    book
}

/// 《千字文》试读样张（开篇数韵，公版）。
fn sample_qian_zi_wen() -> Book {
    let mut book = Book::new("千字文", "周兴嗣");
    book.cover = Cover {
        title: "千字文".into(),
        subtitle: "四言韵语 · 试读样张".into(),
        seal: "笺读".into(),
        accent: 0x3E5A6E,
    };
    book.chapters.push(Chapter {
        title: "天文".into(),
        paragraphs: vec![
            Paragraph::with_notes(
                "天地玄黄，宇宙洪荒。日月盈昃，辰宿列张。",
                vec![Note::new(9, NoteKind::Note, "昃（zè）：太阳偏西。")],
            ),
            Paragraph::plain("寒来暑往，秋收冬藏。闰余成岁，律吕调阳。"),
            Paragraph::plain("云腾致雨，露结为霜。金生丽水，玉出昆冈。"),
        ],
    });
    book.chapters.push(Chapter {
        title: "物产".into(),
        paragraphs: vec![
            Paragraph::with_notes(
                "剑号巨阙，珠称夜光。果珍李柰，菜重芥姜。",
                vec![Note::new(13, NoteKind::Note, "柰（nài）：沙果，似林檎而小。")],
            ),
            // 人名线演示：龙师、火帝、鸟官、人皇四个并称人名。
            Paragraph::with_notes(
                "海咸河淡，鳞潜羽翔。龙师火帝，鸟官人皇。",
                vec![
                    Note::span(10, 2, NoteKind::Person, "龙师"),
                    Note::span(12, 2, NoteKind::Person, "火帝"),
                    Note::span(15, 2, NoteKind::Person, "鸟官"),
                    Note::span(17, 2, NoteKind::Person, "人皇"),
                ],
            ),
            Paragraph::plain("始制文字，乃服衣裳。推位让国，有虞陶唐。"),
        ],
    });
    book.rebuild_toc_from_chapters();
    book
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首启内置书目：`bundled_dao_de_jing` 必须解得开，且是全本。
    #[test]
    fn bundled_dao_de_jing_is_the_full_eighty_one_chapters() {
        let book = bundled_dao_de_jing().expect("内置《道德經》资源应能解析");
        assert_eq!(book.title, "道德經");
        assert_eq!(book.author, "〔周〕李耳");
        assert_eq!(book.chapters.len(), 81, "八十一章一章不能少");
        assert_eq!(book.schema_version, crate::types::BOOK_SCHEMA_VERSION);
        // 正文非空、无 `[注]` 之类未摘净的标签残留。
        let chars: usize = book
            .chapters
            .iter()
            .flat_map(|c| c.paragraphs.iter())
            .map(|p| p.text.chars().count())
            .sum();
        assert!(chars > 5000, "正文应约五千言，实得 {chars}");
        for c in &book.chapters {
            for p in &c.paragraphs {
                assert!(!p.text.contains("[注]"), "夹注标签没摘净：{}", p.text);
                assert!(!p.text.contains('\u{200B}'), "零宽字符没清掉：{}", p.text);
            }
        }
        // 两级目录：道經 / 德經 两篇 + 八十一章。
        let l1: Vec<&str> = book
            .toc
            .iter()
            .filter(|e| e.level == 1)
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(l1, vec!["道經", "德經"], "上下两篇");
        assert_eq!(book.toc.iter().filter(|e| e.level == 2).count(), 81);
    }

    /// 首启内置书目二：《唐詩三百首》必须解得开，且是全本、结构完整。
    #[test]
    fn bundled_tangshi_is_the_full_collection() {
        let book = bundled_tangshi().expect("内置《唐詩三百首》资源应能解析");
        assert_eq!(book.title, "唐詩三百首");
        assert!(book.chapters.len() > 300, "三百餘首，实得 {}", book.chapters.len());
        assert_eq!(book.schema_version, crate::types::BOOK_SCHEMA_VERSION);
        // 每首诗至少一行、无未摘净标签、无零宽字符。
        for c in &book.chapters {
            assert!(!c.paragraphs.is_empty(), "空诗：{}", c.title);
            for p in &c.paragraphs {
                assert!(!p.text.contains("[注]"), "夹注标签没摘净：{}", p.text);
                assert!(!p.text.contains('\u{200B}'), "零宽字符没清掉：{}", p.text);
            }
        }
        // 两级目录：七種詩體（一級）+ 每首詩（二級）。
        let l1: Vec<&str> = book
            .toc
            .iter()
            .filter(|e| e.level == 1)
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(l1.len(), 7, "七體：{:?}", l1);
        assert_eq!(
            book.toc.iter().filter(|e| e.level == 2).count(),
            book.chapters.len(),
            "每首诗都应在目录里"
        );
    }

    /// 运行时加载器：开发态（无 .app 包）也能从仓库 resources/books 找到两本。
    #[test]
    fn bundled_book_loader_finds_resources_in_dev_tree() {
        for name in BUNDLED_BOOKS {
            let book = bundled_book(name).expect("开发态应能从 resources/books 读到");
            assert!(!book.chapters.is_empty(), "{name} 解析后无章节");
        }
    }
}
