//! TXT：最脏的一种——没有结构、编码不明、还全是软换行。
//!
//! 两件事：**猜对编码**、**把硬折行拼回段落**。
//!
//! 编码不能只试 UTF-8：中文电子书里 GB18030 / GBK 占了大半，直接按 UTF-8
//! 读会得到满屏 `�`（这就是用户说的「带乱码的 TXT」）。这里用
//! 「解出来像不像中文」来投票：谁解出的汉字多、替换符少，就信谁。

use std::io::BufRead;
use std::path::Path;

use super::traits::FormatParser;
use super::{read_streamed, ImportError};

/// TXT 解析器。
pub struct TxtParser;

impl FormatParser for TxtParser {
    fn parse(path: &Path) -> Result<String, ImportError> {
        let bytes = read_streamed(path)?;
        let (text, _enc) = decode_bytes(&bytes);
        Ok(merge_soft_wraps(&text))
    }

    fn extensions() -> &'static [&'static str] {
        &["txt", "text", "log", "csv"]
    }
}

/// 编码嗅探 + 解码（TXT / HTML / MD / JSON 共用同一套判据）。
///
/// 顺序：BOM > UTF-8 严格校验 > 候选码表投票。
/// 返回（文本，码表名），码表名只用于日志与测试断言。
pub(crate) fn decode_bytes(bytes: &[u8]) -> (String, &'static str) {
    // ① BOM 最可信，直接照办。
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (
            String::from_utf8_lossy(&bytes[3..]).into_owned(),
            "utf-8 (bom)",
        );
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(&bytes[2..]);
        return (s.into_owned(), "utf-16le");
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(&bytes[2..]);
        return (s.into_owned(), "utf-16be");
    }

    // ② 严格 UTF-8 就到此为止（绝大多数现代稿子）。
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_string(), "utf-8");
    }

    // ③ 投票：谁解出来最像中文。
    //
    // 打分＝汉字数 − 替换符×50。替换符是「这个字节序列在这张码表里不成立」的
    // 铁证，一个就够否掉整张表；汉字数是「解对了」的正向证据。
    let cands: [(&'static encoding_rs::Encoding, &'static str); 4] = [
        (encoding_rs::GB18030, "gb18030"),
        (encoding_rs::BIG5, "big5"),
        (encoding_rs::SHIFT_JIS, "shift_jis"),
        (encoding_rs::EUC_KR, "euc-kr"),
    ];
    let mut best: Option<(i64, String, &'static str)> = None;
    for (enc, name) in cands {
        let (cow, _, _) = enc.decode(bytes);
        let bad = cow.matches('\u{FFFD}').count() as i64;
        let cjk = cow.chars().filter(|c| is_cjk(*c)).count() as i64;
        let score = cjk - bad * 50;
        if best.as_ref().is_none_or(|(b, _, _)| score > *b) {
            best = Some((score, cow.into_owned(), name));
        }
    }
    match best {
        Some((_, s, name)) => (s, name),
        // 四张表全崩（二进制混进来的）：保底 lossy，绝不再往上报错——
        // 能读出错字总比「导入失败」强，读者自己看得见方块。
        None => (String::from_utf8_lossy(bytes).into_owned(), "lossy"),
    }
}

/// 软换行合并：**当前行没说完、下一行又没缩进，就把两行接起来**。
///
/// 判据（按优先级）：
///   1. 空行 → 段落结束（这是作者本来就想分段的地方，最可靠）；
///   2. 当前行以 `。！？；` 或收尾引号/括号结束 → 段落结束；
///   3. 下一行有缩进（全角空格 / 半角空格 / Tab）→ 新段开始，是作者手动缩进；
///   4. 任一行像标题（短、且以「卷/第…回/章」开头）→ 不并，否则会把回目
///      吞进正文第一句里（这是最容易被忽略的坑）；
///   5. 其余情况：合并（去掉 `\n`）。
pub fn merge_soft_wraps(text: &str) -> String {
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if cur.is_empty() {
            cur.push_str(line.trim_start());
            continue;
        }
        let mergeable = !ends_sentence(&cur)
            && !is_indented(raw)
            && !looks_like_heading(line)
            // 上一行本身是回目/章题（不以句读收尾）：绝不能往它后面粘正文。
            && !looks_like_heading(&cur);
        if mergeable {
            cur.push_str(line.trim_start());
        } else {
            paras.push(std::mem::take(&mut cur));
            cur.push_str(line.trim_start());
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }

    // 超长段（整本没有换行的 TXT）：按句拆开，否则一张「段」几万字，
    // 排版层会把它当成一整块处理，翻页/缓存全部退化。
    let mut out = String::new();
    for p in paras {
        for chunk in split_long_paragraph(&p) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&chunk);
        }
    }
    out
}

/// 这句说完了吗（竖排常见的收尾符号都算）。
fn ends_sentence(s: &str) -> bool {
    s.chars()
        .rev()
        .find(|c| !c.is_whitespace())
        .is_some_and(|c| {
            matches!(
                c,
                '。' | '！' | '？' | '；' | '…' | '」' | '』' | '）' | '】' | '》' | '.' | '!' | '?' | ';'
            )
        })
}

/// 行首有缩进（作者手动分段的地方）。
fn is_indented(raw: &str) -> bool {
    raw.starts_with('\u{3000}') || raw.starts_with(' ') || raw.starts_with('\t')
}

/// 长得像标题：短，且以「卷/第…回/章/节/篇/部/集」起手，或以冒号收尾。
fn looks_like_heading(line: &str) -> bool {
    let t = line.trim();
    let n = t.chars().count();
    if n == 0 || n > 28 {
        return false;
    }
    let head: String = t.chars().take(2).collect();
    let starts_vol = head.starts_with('卷')
        || head.starts_with('第')
        || head.starts_with('序')
        || head.starts_with('跋')
        || head.starts_with("凡例")
        || head.starts_with("目錄")
        || head.starts_with("目录");
    let has_unit = t.chars().any(|c| {
        matches!(
            c,
            '回' | '章' | '節' | '节' | '篇' | '部' | '集' | '卷' | '本' | '紀' | '传' | '傳'
        )
    });
    (starts_vol && has_unit) || t.ends_with('：') || t.ends_with(':')
}

/// 把超长段落按句切开（只按强停顿切，绝不切在词中间）。
fn split_long_paragraph(p: &str) -> Vec<String> {
    const SOFT: usize = 1600; // 超过这个字数才动刀
    const TARGET: usize = 400;
    if p.chars().count() <= SOFT {
        return vec![p.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut n = 0usize;
    for c in p.chars() {
        cur.push(c);
        n += 1;
        if n >= TARGET && matches!(c, '。' | '！' | '？' | '；' | '…') {
            out.push(std::mem::take(&mut cur));
            n = 0;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

/// 便于统计：某些章节切分要按行判断，这里给个小工具（测试也用）。
#[allow(dead_code)]
fn non_empty_lines(s: &str) -> usize {
    s.lines().filter(|l| !l.trim().is_empty()).count()
}

/// 只是为了让 `BufRead` 在文档里有个引用位置（实现里用的是 BufReader 分块）。
#[allow(dead_code)]
fn _bufread_marker<R: BufRead>(_r: &R) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_wraps_are_glued_but_headings_are_not() {
        let raw = "第一回 甄士隱夢幻識通靈\n此開卷第一回也。作者自云\n因曾歷過一番夢幻之後\n故將真事隱去。\n\n卻說賈雨村\n風塵懷閨秀。";
        let out = merge_soft_wraps(raw);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "第一回 甄士隱夢幻識通靈", "回目被并进正文了");
        assert_eq!(
            lines[1], "此開卷第一回也。作者自云因曾歷過一番夢幻之後故將真事隱去。",
            "软换行没拼回段落"
        );
        assert_eq!(lines[2], "卻說賈雨村風塵懷閨秀。");
    }

    #[test]
    fn indented_line_starts_a_new_paragraph() {
        let raw = "道可道非常道\n　　名可名非常名";
        let out = merge_soft_wraps(raw);
        assert_eq!(out.lines().count(), 2, "缩进行被误并：{out:?}");
    }

    #[test]
    fn gb18030_is_detected() {
        // 「道德經」的 GB18030 字节
        let bytes = [0xB5u8, 0xC0, 0xB5, 0xC2, 0xBD, 0x9B];
        let (text, enc) = decode_bytes(&bytes);
        assert_eq!(enc, "gb18030", "GB 码没认出来");
        assert_eq!(text, "道德經");
    }
}
