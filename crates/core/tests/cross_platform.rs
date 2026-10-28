//! **跨平台边界的集成测试**（v1.0.0-beta）。
//!
//! 与单元测的区别：这里**只**用 `jiandu_core` 的公共 API，站在一个「外部
//! 壳子」的立场上验证——跨平台壳子能拿到的东西，就应该是这些。
//!
//! 钉住三件事：
//!   1. **不依赖任何 UI crate 也能完成一次完整排版**（这是复用的前提）；
//!   2. 平台接缝（`platform`）与 C ABI（`ffi`）从外部看是好用、一致的；
//!   3. 排版 → 命中测试这条链路，两个模块对得上。

use jiandu_core::gloss::{self, Page, Piece};
use jiandu_core::platform::{
    BuiltinPinyin, PdfExporter, PdfFontKind, PdfGeometry, PinyinSource, UnavailablePdfExporter,
};
use jiandu_core::types::{Glyph, GlyphKind};

/// 把一句话变成字流（壳子侧的活其实只有这一步：把正文喂进来）。
fn flow_of(s: &str) -> Vec<Glyph> {
    s.chars()
        .enumerate()
        .map(|(i, c)| Glyph {
            chapter: 0,
            para: 0,
            offset: i,
            ch: c,
            kind: GlyphKind::Char,
        })
        .collect()
}

fn geo() -> PdfGeometry {
    PdfGeometry {
        cols: 12,
        cell_w: 30.0,
        cell_h: 30.0,
        rows: 22,
        fs: 24.0,
        reserve: 0,
        edge: 34.0,
        margin_top: 40.0,
        margin_bot: 26.0,
        paper: 0xFFFDF6,
        ink: 0x1A1A1A,
        rule: 0x999999,
        rule_soft: 0x999999,
        note_ink: 0x555555,
        accent: 0xB03A2E,
        font: PdfFontKind::Song,
        note_font: PdfFontKind::Kai,
        han_digits: true,
        traditional: false,
        punct_mode: jiandu_core::gloss::PunctMode::Modern,
        punct_ink: 0xC3272D,
    }
}

// ── 1) 排版：不碰 UI 也能排完一整叶 ────────────────────────────────────────

#[test]
fn core_can_layout_a_page_without_any_ui_crate() {
    let flow = flow_of("道可道非常道名可名非常名");
    let pages = gloss::layout_book_from_opts(&flow, |_| None, 8, 8, 0, true, true);
    assert!(!pages.is_empty(), "应至少排出一面");

    let text: String = pages[0]
        .columns
        .iter()
        .flat_map(|c| c.iter())
        .filter_map(|p| match p {
            Piece::Main(g) => Some(g.ch),
            _ => None,
        })
        .collect();
    assert_eq!(text, "道可道非常道名可名非常名", "一个字都不能丢、不能错位");
}

#[test]
fn double_line_gloss_stays_inside_the_same_page() {
    // 夹注挂在第一个「道」上；跨列流转也应落在同一面的后续列里。
    let flow = flow_of("道可道非常道");
    let pages = gloss::layout_book_from_opts(
        &flow,
        |g| {
            if g.offset == 0 {
                Some(("河上公曰：无形无名".to_string(), Default::default(), *g))
            } else {
                None
            }
        },
        8,
        8,
        0,
        true,
        true,
    );
    let glossy: usize = pages[0]
        .columns
        .iter()
        .flat_map(|c| c.iter())
        .filter(|p| matches!(p, Piece::Gloss(..)))
        .count();
    assert!(glossy > 0, "夹注应真的排进去");
}

// ── 2) 排版 → 命中测试：两个模块同一把尺子 ────────────────────────────────

#[test]
fn hit_spans_agree_with_the_layout_result() {
    let flow = flow_of("甲乙丙丁戊己庚辛");
    let pages = gloss::layout_book_from_opts(&flow, |_| None, 8, 8, 0, true, true);
    let page: &Page = &pages[0];
    let col = &page.columns[0];
    let spans = jiandu_core::hit::column_spans(col, 30.0, true);
    let main_n = col.iter().filter(|p| matches!(p, Piece::Main(_))).count();
    assert_eq!(spans.len(), main_n, "每个正文字都要有一块命中区");
    for s in &spans {
        assert!(s.h > 0.0, "命中区不能是零高度");
    }
}

// ── 3) 平台接缝 ────────────────────────────────────────────────────────────

#[test]
fn pdf_geometry_is_platform_free_data() {
    let g = geo();
    assert!((g.sheet_w() - (12.0 * 30.0 + 34.0)).abs() < 1e-6);
    assert!(g.sheet_h() > g.sheet_w());
    // 天头∶地脚 = 3∶2 的铁律，在数据层就该体现
    assert!(g.margin_top > g.margin_bot);
    // 字体只是**语义**，不是具体字体名——跨平台翻译交给壳子
    assert_eq!(g.font, PdfFontKind::Song);
}

#[test]
fn missing_pdf_backend_fails_loudly_not_silently() {
    let e = UnavailablePdfExporter;
    assert_eq!(e.name(), "unavailable");
    let pages = gloss::layout_book_from_opts(&flow_of("箋讀"), |_| None, 8, 8, 0, true, true);
    let ruby = std::collections::HashMap::new();
    let leaf = jiandu_core::platform::LeafSpec {
        page: &pages[0],
        title: "箋讀",
        chapter: 0,
        leaf: 1,
        inscription: "箋讀",
        author: "佚名",
        ruby: &ruby,
    };
    let err = e
        .render_leaves(std::slice::from_ref(&leaf), &geo())
        .expect_err("没有后端时必须报错");
    assert!(
        err.to_string().contains("PdfExporter"),
        "错误信息要指到该接的地方：{err}"
    );
}

#[test]
fn pinyin_source_is_the_single_seam_for_annotation() {
    let p = BuiltinPinyin;
    // 常用字不注
    assert!(p.is_common('道'));
    assert!(p.pinyin_of('道').is_none(), "常用字不该注音");
    // 生僻字要注
    assert!(!p.is_common('埵'));
    assert!(p.pinyin_of('埵').is_some(), "生僻字该有一根拐杖");
}

// ── 4) C ABI（非 Rust 壳子走这条路）────────────────────────────────────────

#[test]
fn ffi_round_trip_from_an_external_caller() {
    use std::ffi::CStr;

    let v = jiandu_core::ffi::jiandu_version();
    assert!(!v.is_null());
    let vs = unsafe { CStr::from_ptr(v) }.to_str().unwrap().to_string();
    unsafe { jiandu_core::ffi::jiandu_string_free(v) };
    assert_eq!(vs, env!("CARGO_PKG_VERSION"), "ABI 报的版本应与包一致");

    // 埵 U+57F5 有音；般 U+822C 是常用字、不注
    let p = jiandu_core::ffi::jiandu_auto_pinyin(0x57F5);
    assert!(!p.is_null());
    unsafe { jiandu_core::ffi::jiandu_string_free(p) };
    assert_eq!(jiandu_core::ffi::jiandu_is_common_char(0x822C), 1);
    // 非法码位不崩、返回 null / 0
    assert!(jiandu_core::ffi::jiandu_auto_pinyin(0x110000).is_null());
    assert_eq!(jiandu_core::ffi::jiandu_is_common_char(0x110000), 0);
}

#[test]
fn ffi_gate_is_the_same_gate_as_the_native_app() {
    use std::ffi::CString;
    // 大小写 / 空格归一：与 Rust 侧 `gate::valid` 同一口径
    for ok in ["SHUYE2026", "shuye2026", " shuye 2026 "] {
        let c = CString::new(ok).unwrap();
        assert_eq!(
            unsafe { jiandu_core::ffi::jiandu_gate_check(c.as_ptr()) },
            1,
            "{ok} 应放行"
        );
    }
    let bad = CString::new("随便乱填").unwrap();
    assert_eq!(unsafe { jiandu_core::ffi::jiandu_gate_check(bad.as_ptr()) }, 0);
    // null 是安全的空操作
    assert_eq!(
        unsafe { jiandu_core::ffi::jiandu_gate_check(std::ptr::null()) },
        0
    );
}
