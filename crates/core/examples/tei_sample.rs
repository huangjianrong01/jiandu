//! 样例：把真实书稿《資治通鑑演示版》(zztj.json) 喂进 IR，并叠加一个
//! 「TEI 三路分离 · 实体标签示范」卷，分别导出为真实 TEI XML。
//!
//! 运行：
//!   cargo run --example tei_sample -p jiandu-core --release
//!
//! 输出：
//!   /tmp/jiandu_zztj.tei.xml   —— 真实书稿（12 章纯正文）
//!   /tmp/jiandu_tei_demo.tei.xml —— 三路分离 + 实体标签示范

use jiandu_core::exporter::export_tei;
use jiandu_core::ir::{IrBook, IrJuan, IrBlock, IrSpan, IrNoteKind, IrSource};

/// 真实路径：读 zztj.json → 每章一行正文 → IrJuan → IrBook。
fn build_real(path: &str) -> Option<IrBook> {
    let txt = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let title = v["title"].as_str().unwrap_or("未命名").to_string();
    let author = v["author"].as_str().unwrap_or("").to_string();
    let mut book = IrBook::new(&title, &author, IrSource::Json);
    if let Some(chapters) = v["chapters"].as_array() {
        for c in chapters {
            let ct = c["title"].as_str().unwrap_or("").to_string();
            let mut j = IrJuan::new(ct);
            j.level = 1;
            if let Some(paras) = c["paragraphs"].as_array() {
                for p in paras {
                    if let Some(t) = p["text"].as_str() {
                        let t = t.trim();
                        if !t.is_empty() {
                            j.para(t);
                        }
                    }
                }
            }
            book.juans.push(j);
        }
    }
    Some(book)
}

/// 实体标签示范卷：一次性铺开三路分离 + persName / placeName / title / gloss / ruby / hi。
fn build_demo() -> IrBook {
    let mut book = IrBook::new("資治通鑑（TEI 標記示範）", "司馬光 等撰", IrSource::Json);
    let mut j = IrJuan::new("【TEI 三路分離 · 實體標籤示範】");
    j.level = 1;

    // 批注流：眉批 / 题解
    j.blocks.push(IrBlock::HeadNote(
        "周威烈王二十三年，初命晉大夫魏斯、趙籍、韓虔為諸侯。".into(),
    ));
    // 小节标题（结构）
    j.blocks.push(IrBlock::SubHead {
        level: 2,
        text: "臣光曰".into(),
    });
    // 正文段：body + 人物 + 夹注 + 注音 + 地名 + 书名 + 着重
    j.blocks.push(IrBlock::Para(vec![
        IrSpan::Text("臣光曰：臣聞".into()),
        IrSpan::Note { kind: IrNoteKind::Person, text: "天子".into() },
        IrSpan::Text("之職莫大於".into()),
        IrSpan::Note { kind: IrNoteKind::Note, text: "禮者，紀綱也。".into() }, // 夹注
        IrSpan::Text("，".into()),
        IrSpan::Note { kind: IrNoteKind::Note, text: "分者，君臣之位也。".into() }, // 夹注
        IrSpan::Text("莫大於".into()),
        IrSpan::Ruby { base: "分".into(), rt: "fēn".into() }, // 注音
        IrSpan::Text("，".into()),
        IrSpan::Note { kind: IrNoteKind::Person, text: "晉文公".into() },
        IrSpan::Text("召".into()),
        IrSpan::Note { kind: IrNoteKind::Person, text: "周襄王".into() },
        IrSpan::Text("於".into()),
        IrSpan::Note { kind: IrNoteKind::Place, text: "河陽".into() }, // 地名
        IrSpan::Text("，".into()),
        IrSpan::Note { kind: IrNoteKind::Person, text: "孔子".into() },
        IrSpan::Text("非之。又觀".into()),
        IrSpan::Note { kind: IrNoteKind::Book, text: "資治通鑑".into() }, // 书名
        IrSpan::Text("一書，".into()),
        IrSpan::Note { kind: IrNoteKind::Emph, text: "誠古今之龜鑑也".into() }, // 着重
        IrSpan::Text("。".into()),
    ]));
    // 独立夹注块
    j.blocks.push(IrBlock::Annotation {
        kind: IrNoteKind::Tongjia,
        spans: vec![IrSpan::Text("按：三家分晉，實為戰國之端。".into())],
    });

    book.juans.push(j);
    book
}

fn print_first_20(tag: &str, xml: &str) {
    println!("\n=== {tag}（前 20 行）===");
    for (i, l) in xml.lines().take(20).enumerate() {
        println!("{:>2}: {}", i + 1, l);
    }
}

fn main() {
    // 1) 真实书稿
    if let Some(book) = build_real("/tmp/jiandu_sandbox/books/zztj.json") {
        let xml = export_tei(&book);
        std::fs::write("/tmp/jiandu_zztj.tei.xml", &xml).expect("write zztj");
        print_first_20("真实书稿《資治通鑑演示版》TEI 导出", &xml);
        let chars: usize = book.juans.iter().map(|j| j.char_count()).sum();
        println!(
            "\n真实书稿统计：{} 卷、约 {} 字，已写出 /tmp/jiandu_zztj.tei.xml（{} 字节）。",
            book.juans.len(),
            chars,
            xml.len()
        );
    } else {
        println!("（未找到 zztj.json，跳过真实导出）");
    }

    // 2) 三路分离 + 实体标签示范
    let demo = build_demo();
    let dxml = export_tei(&demo);
    std::fs::write("/tmp/jiandu_tei_demo.tei.xml", &dxml).expect("write demo");
    print_first_20("TEI 三路分离 · 实体标签示范", &dxml);
    println!("\n示范卷已写出 /tmp/jiandu_tei_demo.tei.xml（{} 字节）。", dxml.len());
}
