//! 插入夹注 + 半叶起点局部重排（绝不重排全书）。
//!
//! 设计要点（与 `gloss::layout_book` 同源，不另起炉灶）：
//!   * 笺读生产引擎的夹注**不**把 `[注]…[/注]` 字面镶进正文文本——
//!     那样会破坏 `to_flow` 的字形账（每段前 2 个段首缩进占位的下标口径），
//!     还会把方括号渲染成正文。夹注是「按宿主字形挂接」的：在宿主字的
//!     `(chapter, para, offset)` 上挂一条 `Note{ kind: NoteKind::Note }`
//!     记录，分页时由 `layout_book` 的 `gloss(g)` 闭包取出，作为双行小字
//!     夹注流入该列（本列放不下自动跨列 / 跨面，见 `gloss`）。
//!   * 因此「在 charIndex 注入 `[注]…[/注]`」=「在 flow[idx] 宿主字上
//!     追加一条 Note 记录」；正文流长度与后续字的全局下标**完全不变**，
//!     后续只需从「当前半叶起点」重排即可。
//!   * 局部重排的不变量：`layout_book` 是纯函数，对「同一段流 + 同一几何
//!     + 同一注文」确定。后缀起点恰是原函数开始排第 N 面时的流位置，
//!
//!     所以重排出的尾面与「全书重排」得到的第 N.. 面逐面一致——局部 ==
//!     全局，无漂移，前 N−1 面更是原样复用、零重算。

use crate::gloss::{layout_book_from, GlossPayload, Page, Piece};
use crate::types::{Book, Glyph, GlyphKind, Note, NoteKind, NoteSource};

/// 在指定宿主字 `(chapter, para, offset)` 上挂一条夹注（或覆盖同字已有夹注）。
///
/// `source` 标记来源：用户自加用 `NoteSource::User`（深朱砂红、可改删），
/// 古籍原本注用 `NoteSource::Original`。`Note::user` / `Note::new` 已分别设好
/// 默认来源，这里统一在注入点强制一次 `source`，保证「用户重新标注原注处」
/// 也能转成可改删的用户注。
pub fn inject_inline_note_at_key(
    book: &mut Book,
    chapter: usize,
    para: usize,
    offset: usize,
    text: &str,
    source: NoteSource,
) {
    if let Some(ch) = book.chapters.get_mut(chapter) {
        if let Some(p) = ch.paragraphs.get_mut(para) {
            if let Some(n) = p
                .notes
                .iter_mut()
                .find(|n| n.at == offset && n.kind == NoteKind::Note)
            {
                n.text = text.to_string(); // 同字已有夹注：覆盖而非重复
                n.source = source;
                n.anchor = Some((chapter, para, offset)); // v1.49.0：落盘即记 charIndex
            } else {
                let mut note = Note::new(offset, NoteKind::Note, text);
                note.source = source;
                note.anchor = Some((chapter, para, offset)); // v1.49.0：朱批录跳转凭据
                p.notes.push(note);
            }
        }
    }
}

/// ① 注入夹注：在 `flow[idx]` 处的宿主字形上挂一条「用户自加夹注」。
///
/// 等价于在底层数据源 `Book::chapters[c].paragraphs[p].notes` 里追加
/// （或覆盖同字已有夹注）一条 `Note{ at: offset, kind: NoteKind::Note,
/// source: NoteSource::User }`。缩进占位（`GlyphKind::Indent`）与越界下标不挂注。
pub fn inject_inline_note(book: &mut Book, flow: &[Glyph], idx: usize, text: &str) {
    let g = match flow.get(idx) {
        Some(g) if g.kind == GlyphKind::Char => *g,
        _ => return,
    };
    inject_inline_note_at_key(book, g.chapter, g.para, g.offset, text, NoteSource::User);
}

/// 移除某宿主字上的「用户自加夹注」（原本夹注不可删）。
///
/// 返回是否真的移除了一条。移除后由调用方对 `idx` 所在半叶做局部重排
/// （`relayout_from_halfleaf`）—— 释放该注所占的半格，后续字上移回流。
pub fn remove_user_note_at_key(
    book: &mut Book,
    chapter: usize,
    para: usize,
    offset: usize,
) -> bool {
    if let Some(ch) = book.chapters.get_mut(chapter) {
        if let Some(p) = ch.paragraphs.get_mut(para) {
            let before = p.notes.len();
            p.notes.retain(|n| {
                !(n.at == offset && n.kind == NoteKind::Note && n.source == NoteSource::User)
            });
            return p.notes.len() < before;
        }
    }
    false
}

/// ③ 移除（flow 下标版，委托 key 版，仅供测试 / 旧调用点）。
pub fn remove_user_note(book: &mut Book, flow: &[Glyph], idx: usize) -> bool {
    let g = match flow.get(idx) {
        Some(g) if g.kind == GlyphKind::Char => *g,
        _ => return false,
    };
    remove_user_note_at_key(book, g.chapter, g.para, g.offset)
}

/// ② 局部重排：只在「当前半叶起点」之后重算，绝不重排全书。
///
/// `prefix_pages` 是注入前的整书分页（用来定位半叶 N 与它的起始流下标，
/// 以及原样复用前 N−1 面）。`flow`/`gloss`/`cols`/`rows`/`reserve_rows`
/// 是注入后的最新参数。`idx` 为被注入字（或任意锚点字）的全局流下标。
///
/// 返回：前缀 `prefix_pages[0..N]`（原样）＋ 后缀 `layout_book(flow[N_start..])`
/// 得到的新尾面。
pub fn relayout_from_halfleaf(
    prefix_pages: &[Page],
    flow: &[Glyph],
    gloss: impl Fn(&Glyph) -> Option<GlossPayload>,
    cols: usize,
    rows: usize,
    reserve_rows: usize,
    idx: usize,
) -> Vec<Page> {
    let (n, page_start) = page_and_start(prefix_pages, idx);
    let mut out: Vec<Page> = prefix_pages[..n.min(prefix_pages.len())].to_vec();
    // 后缀首面是否当章首：沿用原 page N 的 `start_chapter`（原分页已正确判定），
    // 这样中段续排不会被误判成章首页（少一列 + 误加题署带）。后缀内部若遇到
    // 真正的章边界，layout_book_from 仍会正常另起章首页。
    let start_as_chapter = prefix_pages
        .get(n)
        .map(|p| p.start_chapter)
        .unwrap_or(true);
    let tail = layout_book_from(
        &flow[page_start.min(flow.len())..],
        gloss,
        cols,
        rows,
        reserve_rows,
        start_as_chapter,
    );
    out.extend(tail);
    out
}

/// 当前半叶 N 与其在全局流里的起始下标。
///
/// `Page.glyphs` 是「读序 + 含段首缩进占位」的版面流切片（`layout_book`
/// 把每个 `g`——含 `Indent`——都推进 `glyphs`），所以累计 `glyphs.len()`
/// 即得到每面在 `flow` 里的区间，与 `Root::goto_char_index` 的口径一致。
pub fn page_and_start(pages: &[Page], idx: usize) -> (usize, usize) {
    let mut acc = 0usize;
    for (pi, pg) in pages.iter().enumerate() {
        let n = pg.glyphs.len();
        if acc <= idx && idx < acc + n {
            return (pi, acc);
        }
        acc += n;
    }
    // 越界（落在末字之后）：返回末面起点，整段重排（等同全书重排末段）。
    (pages.len().saturating_sub(1), acc)
}

/// ③ 点击坐标 → 全局流下标 的桥接（竖排、从右往左读）。
///
/// `read_col` 已是读序列号（0 = 最右列，最先读）；`row` 是该列自上而下
/// 的「主字序号」（`Piece::Main` 的个数，夹注小字占半格、不单独计主字行）。
/// `page_start` 是当前半叶在全局流里的起始下标。
///
/// 关键：`Page.glyphs` 的读序与「各列 `Piece::Main` 读序」严格一一对应
/// （`layout_book` 每消费一个 `g` 就同时 `glyphs.push` + 该列 `push(Main)`，
/// 夹注只追加 `Gloss`、不进 `glyphs`），所以主字数累加即 glyphs 下标。
pub fn cell_to_flow_index(page: &Page, read_col: usize, row: usize, page_start: usize) -> usize {
    let cols = page.columns.len();
    if cols == 0 {
        return page_start;
    }
    let rc = read_col.min(cols - 1);
    let mut k = 0usize; // 已越过的主字数 = glyphs 读序下标
    for c in 0..rc {
        k += main_count(&page.columns[c]);
    }
    k += nth_main_index(&page.columns[rc], row);
    page_start + k
}

/// 一列里 `Piece::Main` 的个数（= 该列在 `glyphs` 读序里占的主字数）。
fn main_count(col: &[Piece]) -> usize {
    col.iter().filter(|p| matches!(p, Piece::Main(_))).count()
}

/// 列内第 `row` 个主字在「主字读序」里的下标；越界则落到该列末主字之后。
fn nth_main_index(col: &[Piece], row: usize) -> usize {
    let mut main_idx = 0usize;
    let mut seen = 0usize;
    for p in col {
        if matches!(p, Piece::Main(_)) {
            if seen == row {
                return main_idx;
            }
            seen += 1;
            main_idx += 1;
        }
    }
    main_idx
}

/// 一页的结构指纹（用于断言「局部 == 全局」逐面一致）。
fn page_fingerprint(p: &Page) -> String {
    let mut s = String::new();
    s.push_str(&format!("G{}|", p.glyphs.len()));
    for col in &p.columns {
        for piece in col {
            match piece {
                Piece::Main(g) => s.push(g.ch),
                Piece::Gap => s.push('·'),
                Piece::Gloss(t, src, _) => s.push_str(&format!(
                    "[{}:{}]",
                    t.chars().count(),
                    match src {
                        NoteSource::User => 'U',
                        NoteSource::Original => 'O',
                    }
                )),
            }
        }
        s.push(';');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gloss::layout_book;
    use crate::grid::GRID_COLUMN_CHOICES;
    use crate::types::{Chapter, GlyphKind, Paragraph};

    fn gloss_of(book: &Book) -> impl Fn(&Glyph) -> Option<GlossPayload> + '_ {
        move |g: &Glyph| {
            let p = &book.chapters[g.chapter].paragraphs[g.para];
            p.notes
                .iter()
                .find(|n| n.at == g.offset && n.kind == NoteKind::Note)
                .map(|n| (n.text.clone(), n.source, *g))
        }
    }

    fn book_one_chapter() -> Book {
        let mut b = Book::new("测试", "佚名");
        b.chapters.push(Chapter {
            title: "卷一".into(),
            paragraphs: vec![Paragraph {
                text: "天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏闰余成岁律吕调阳".repeat(8),
                notes: vec![],
            }],
        });
        b
    }

    fn book_two_chapters() -> Book {
        let mut b = Book::new("两卷", "佚名");
        for ci in 0..2 {
            b.chapters.push(Chapter {
                title: format!("卷{}", ci + 1),
                paragraphs: vec![Paragraph {
                    text: "天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏闰余成岁律吕调阳".repeat(3),
                    notes: vec![],
                }],
            });
        }
        b
    }

    /// 核心断言：注入夹注后，局部重排得到的结果与全书重排逐面一致。
    #[test]
    fn local_relayout_equals_global_after_inject() {
        for cols in GRID_COLUMN_CHOICES {
            for (make, reserve) in [
                (book_one_chapter as fn() -> Book, 0usize),
                (book_two_chapters as fn() -> Book, 4usize),
            ] {
                let mut book = make();
                let rows = 11usize;
                let flow0 = book.to_flow(true);
                let full0 = layout_book(&flow0, gloss_of(&book), cols, rows, reserve);

                // 选一个偏后的锚点（保证当前半叶不是第一面），并跳过段首缩进占位，
                // 确保命中正文字、真正触发注入。
                let total: usize = full0.iter().map(|p| p.glyphs.len()).sum();
                let mut idx = total * 3 / 4;
                while idx < flow0.len() && flow0[idx].kind != GlyphKind::Char {
                    idx += 1;
                }
                assert!(idx < flow0.len(), "找不到正文字锚点");
                let g = flow0[idx];
                assert_eq!(g.kind, GlyphKind::Char, "锚点应为正文字");

                // 注入夹注（注意：flow 长度不变，后续字全局下标稳定）。
                inject_inline_note(&mut book, &flow0, idx, "夹注内容示例");

                let flow1 = book.to_flow(true);
                assert_eq!(flow1.len(), flow0.len(), "注入夹注不应改变正文流长度");

                // 局部重排：保留前缀，后缀重排。
                let local = relayout_from_halfleaf(
                    &full0,
                    &flow1,
                    gloss_of(&book),
                    cols,
                    rows,
                    reserve,
                    idx,
                );

                // 全书重排对照。
                let global = layout_book(&flow1, gloss_of(&book), cols, rows, reserve);

                // 1) 页数一致（分页边界相同，无多页 / 漏页）。
                assert_eq!(
                    local.len(),
                    global.len(),
                    "{cols}列{reserve}题署：局部与全书页数不一致"
                );
                // 2) 逐面结构指纹一致（局部 == 全局，含跨列 / 跨面流转）。
                for i in 0..local.len() {
                    assert_eq!(
                        page_fingerprint(&local[i]),
                        page_fingerprint(&global[i]),
                        "{cols}列{reserve}题署：第 {i} 面局部与全书不一致"
                    );
                }
                // 3) 夹注一字不丢，且确实流入了尾面。
                let local_gloss: String = local
                    .iter()
                    .flat_map(|p| p.columns.iter())
                    .flatten()
                    .filter_map(|p| if let Piece::Gloss(s, _, _) = p {
                        Some(s.clone())
                    } else {
                        None
                    })
                    .collect();
                assert!(
                    local_gloss.contains("夹注内容示例"),
                    "{cols}列{reserve}题署：夹注未进入版面"
                );
                // 4) 正文一字不丢（主字流与全局一致）。
                let local_main: String = local
                    .iter()
                    .flat_map(|p| p.glyphs.iter())
                    .map(|g| g.ch)
                    .collect();
                let global_main: String = global
                    .iter()
                    .flat_map(|p| p.glyphs.iter())
                    .map(|g| g.ch)
                    .collect();
                assert_eq!(local_main, global_main, "{cols}列{reserve}题署：正文错乱");
            }
        }
    }

    /// 来源标记一路带到版面：用户自加夹注的 `Piece::Gloss` 必须是
    /// `NoteSource::User` 且携带宿主字形（右键改删据此定位书稿里那条注文）；
    /// 古籍原本的夹注则是 `Original`。二者不得混淆。
    #[test]
    fn gloss_piece_carries_source_and_host() {
        let mut book = book_one_chapter();
        let (cols, rows, reserve) = (10usize, 11usize, 0usize);
        // 先放一条「古籍原本的夹注」（source 默认 Original）。
        book.chapters[0].paragraphs[0]
            .notes
            .push(Note::new(3, NoteKind::Note, "原注"));
        let flow = book.to_flow(true);
        // 再在另一个字上加一条「用户自加夹注」。
        let idx = flow
            .iter()
            .position(|g| g.kind == GlyphKind::Char && g.offset == 9)
            .expect("样本里应有 offset 9 的正文字");
        let host = flow[idx];
        inject_inline_note(&mut book, &flow, idx, "我加的注");

        let pages = layout_book(&flow, gloss_of(&book), cols, rows, reserve);
        let pieces: Vec<&Piece> = pages
            .iter()
            .flat_map(|p| p.columns.iter())
            .flatten()
            .collect();
        let user: Vec<_> = pieces
            .iter()
            .filter_map(|p| {
                if let Piece::Gloss(s, src, h) = p {
                    Some((s.as_str(), *src, *h))
                } else {
                    None
                }
            })
            .collect();

        // 两条注文都进了版面，且来源分明。
        assert!(
            user.iter().any(|(s, src, _)| s.contains("原注")
                && matches!(src, NoteSource::Original)),
            "原本夹注应标 Original"
        );
        // 用户自加夹注可能因「列底换行」被拆成多段 `Piece::Gloss`（跨列续排，
        // 见 `gloss::layout_book` 的「放不下的排下一列列首」）——这是正常版式，
        // 不是 bug。故按来源聚合文本再判定，且逐段校验来源与宿主。
        let user_pieces: Vec<&(&str, NoteSource, Glyph)> = user
            .iter()
            .filter(|(_, src, _)| matches!(src, NoteSource::User))
            .collect();
        let mine_text: String = user_pieces.iter().map(|(s, _, _)| *s).collect();
        assert!(mine_text.contains("我加的注"), "用户夹注应进入版面（可能跨列拆分）");
        for (_, src, h) in user_pieces {
            assert!(
                matches!(src, NoteSource::User),
                "用户自加夹注应标 User（渲染据此染深朱砂红）"
            );
            assert_eq!(
                (h.chapter, h.para, h.offset),
                (host.chapter, host.para, host.offset),
                "用户夹注应携带宿主字形，供右键改删定位"
            );
        }
    }

    /// 局部重排不会重算前缀：前 N−1 面与全书重排的前 N−1 面完全相同。
    #[test]
    fn prefix_pages_are_reused_untouched() {
        let mut book = book_one_chapter();
        let cols = 10usize;
        let rows = 11usize;
        let reserve = 0usize;
        let flow0 = book.to_flow(true);
        let full0 = layout_book(&flow0, gloss_of(&book), cols, rows, reserve);
        let total: usize = full0.iter().map(|p| p.glyphs.len()).sum();
        let idx = total / 2;
        inject_inline_note(&mut book, &flow0, idx, "复用前缀验证");
        let flow1 = book.to_flow(true);
        let local = relayout_from_halfleaf(
            &full0,
            &flow1,
            gloss_of(&book),
            cols,
            rows,
            reserve,
            idx,
        );
        let (n, _) = page_and_start(&full0, idx);
        // 锚点落首页时无可复用前缀，该检查本身空真；仅当 n≥1 时验证前缀原样复用。
        if n == 0 {
            return;
        }
        // 前 N 面（含当前半叶）之外的前缀部分逐面一致。
        let global = layout_book(&flow1, gloss_of(&book), cols, rows, reserve);
        for i in 0..n {
            assert_eq!(
                page_fingerprint(&local[i]),
                page_fingerprint(&global[i]),
                "第 {i} 面前缀被重算，违背了「局部重排」约定"
            );
        }
    }

    /// 点击坐标桥接：cell_to_flow_index 反算的下标，落回当前半叶且宿主字一致。
    #[test]
    fn cell_to_flow_index_roundtrips() {
        let book = book_one_chapter();
        let cols = 8usize;
        let rows = 11usize;
        let flow = book.to_flow(true);
        let pages = layout_book(&flow, gloss_of(&book), cols, rows, 0);
        // 取第 2 面（非章首页）中部一列中部一行。
        let page = &pages[1];
        let read_col = 3usize;
        let row = 4usize;
        let (n, page_start) = page_and_start(&pages, pages[0].glyphs.len() + 50);
        let fi = cell_to_flow_index(page, read_col, row, page_start);
        // fi 应落在第 n 面区间内。
        let (n2, ps2) = page_and_start(&pages, fi);
        assert_eq!(n2, n, "反算下标落错半叶");
        assert_eq!(ps2, page_start, "反算下标的半叶起点不一致");
        // 该下标对应的宿主字，确在 read_col 列的 row 主字位置（结构一致）。
        let k = fi - page_start;
        let mut before = 0usize;
        for c in 0..read_col {
            before += main_count(&page.columns[c]);
        }
        let within = nth_main_index(&page.columns[read_col], row);
        assert_eq!(k, before + within, "cell→flow 下标与列内主字计数不符");
    }

    /// 删除用户夹注后，局部重排应使版面回到「无该注」状态：夹注消失、正文不变。
    #[test]
    fn remove_user_note_relayout_restores() {
        let mut book = book_one_chapter();
        let cols = 10usize;
        let rows = 11usize;
        let reserve = 0usize;
        let flow0 = book.to_flow(true);
        let full0 = layout_book(&flow0, gloss_of(&book), cols, rows, reserve);
        let total: usize = full0.iter().map(|p| p.glyphs.len()).sum();
        let mut idx = total / 2;
        while idx < flow0.len() && flow0[idx].kind != GlyphKind::Char {
            idx += 1;
        }
        // 注入用户夹注
        inject_inline_note(&mut book, &flow0, idx, "待删的用户注");
        let flow1 = book.to_flow(true);
        let local_with = relayout_from_halfleaf(
            &full0, &flow1, gloss_of(&book), cols, rows, reserve, idx,
        );
        let gloss_with: String = local_with
            .iter()
            .flat_map(|p| p.columns.iter())
            .flatten()
            .filter_map(|p| if let Piece::Gloss(s, _, _) = p { Some(s.clone()) } else { None })
            .collect();
        assert!(gloss_with.contains("待删的用户注"), "注入后夹注应出现");

        // 删除用户夹注
        let removed = remove_user_note(&mut book, &flow1, idx);
        assert!(removed, "应移除一条用户夹注");
        let flow2 = book.to_flow(true);
        let local_after = relayout_from_halfleaf(
            &full0, &flow2, gloss_of(&book), cols, rows, reserve, idx,
        );
        // 夹注应消失
        let gloss_after: String = local_after
            .iter()
            .flat_map(|p| p.columns.iter())
            .flatten()
            .filter_map(|p| if let Piece::Gloss(s, _, _) = p { Some(s.clone()) } else { None })
            .collect();
        assert!(!gloss_after.contains("待删的用户注"), "删除后夹注应消失");
        // 正文应与原书一致（删除用户注不改变任何正文字）
        let main_after: String = local_after
            .iter()
            .flat_map(|p| p.glyphs.iter())
            .map(|g| g.ch)
            .collect();
        let main_orig: String = full0
            .iter()
            .flat_map(|p| p.glyphs.iter())
            .map(|g| g.ch)
            .collect();
        assert_eq!(main_after, main_orig, "删除用户注不应改变正文");
    }
}
