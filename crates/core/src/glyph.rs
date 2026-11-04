//! **字体字形覆盖**（v1.9.78）：自带一个极小的 TrueType / OpenType `cmap`
//! 解析器，只为回答一个问题——**这个字，这款字体画得出来吗？**
//!
//! # 为什么内核里要自己解字体
//!
//! 「导入古籍绝不出豆腐块」这句话只能靠字体保证，任何映射表都替不了：
//! 字体缺字形，屏幕上就是一个空心方框，跟这个字有没有异体毫无关系。
//! 要判断缺不缺，唯一的办法是读字体的 `cmap` 表。
//!
//! 不引第三方 crate 的理由：core 的纯洁性守卫要求零 UI / 零平台依赖，
//! 而 `cmap` 只有两种格式在实际字体里覆盖 99.9%（format 4 与 format 12），
//! 加起来不到 200 行。为了这点活儿拖进一个字体引擎，不值得。
//!
//! # 用法
//!
//! ```ignore
//! let bytes = std::fs::read("qiji.ttf")?;
//! let glyphs = GlyphSet::from_bytes(&bytes)?;
//! if !glyphs.contains('𠀀') {
//!     // 这个字会变豆腐块
//! }
//! ```
//!
//! # 边界
//!
//! * 只读 `cmap`，不读 `glyf` / `CFF`——有码位映射就认为画得出来。
//! * 复合字体集合（`.ttc`）取**第一款**字体。
//! * 遇到不认识的 format 就跳过，不报错：宁少认几个字，也不许因为一个
//!   怪字体把导入流程卡死。

use std::collections::HashSet;

/// 字体解析失败的原因。**只说不认识，不说「坏」**——很多老字体确实带怪表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlyphError {
    /// 文件太短，连 sfnt 头都不够。
    TooShort,
    /// 不是 TrueType / OpenType / TTC。
    NotAFont,
    /// 找不到可用的 `cmap` 子表。
    NoCmap,
}

impl std::fmt::Display for GlyphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GlyphError::TooShort => write!(f, "文件太小，不是有效字体"),
            GlyphError::NotAFont => write!(f, "不是 TrueType / OpenType 字体"),
            GlyphError::NoCmap => write!(f, "字体里没有可读的 cmap 表"),
        }
    }
}

impl std::error::Error for GlyphError {}

/// 一款字体的字形覆盖。
///
/// 内部存**码位区间**而不是 `HashSet<u32>`：`Songti SC` 有三万多个字形，
/// 存成 HashSet 要 1.2MB，存成区间（约 300 段）不到 5KB，查询是一次
/// 二分。分批导入几十万字时这个差别是能被用户感知到的。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlyphSet {
    /// 已排序、已合并的闭区间。
    ranges: Vec<(u32, u32)>,
    /// 去重后的码位总数（区间长度之和）。
    total: u32,
}

impl GlyphSet {
    /// 从字体文件字节解析。
    pub fn from_bytes(data: &[u8]) -> Result<Self, GlyphError> {
        if data.len() < 12 {
            return Err(GlyphError::TooShort);
        }
        let base = sfnt_offset(data)?;
        let cmap = find_table(data, base, b"cmap").ok_or(GlyphError::NoCmap)?;

        let mut cps: HashSet<u32> = HashSet::new();
        let num = be_u16(data, cmap + 2).unwrap_or(0) as usize;
        for i in 0..num {
            // encoding record: platformID u16 / encodingID u16 / offset u32
            let rec = cmap + 4 + i * 8;
            let Some(sub_rel) = be_u32(data, rec + 4) else { continue };
            let sub = cmap + sub_rel as usize;
            let Some(fmt) = be_u16(data, sub) else { continue };
            match fmt {
                4 => parse_format4(data, sub, &mut cps),
                12 => parse_format12(data, sub, &mut cps),
                6 => parse_format6(data, sub, &mut cps),
                0 => parse_format0(data, sub, &mut cps),
                _ => {} // format 2/8/10/13/14：古籍用不到，跳过
            }
        }
        if cps.is_empty() {
            return Err(GlyphError::NoCmap);
        }

        let mut v: Vec<u32> = cps.into_iter().collect();
        v.sort_unstable();
        let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(v.len() / 4 + 8);
        for cp in v {
            match ranges.last_mut() {
                Some(last) if cp == last.1 + 1 => last.1 = cp,
                _ => ranges.push((cp, cp)),
            }
        }
        let total = ranges.iter().map(|(a, b)| b - a + 1).sum();
        Ok(Self { ranges, total })
    }

    /// 这款字体有多少个码位。
    pub fn len(&self) -> u32 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// 画得出来吗？非汉字（ASCII / 标点 / 空白）一律当「有」——
    /// 字体缺 ASCII 的概率远低于我们误判它的概率。
    pub fn contains(&self, c: char) -> bool {
        if !crate::variants::is_han(c) {
            return true;
        }
        let cp = c as u32;
        match self.ranges.binary_search_by(|&(a, b)| {
            if cp < a {
                std::cmp::Ordering::Greater
            } else if cp > b {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    /// 一段文本里画不出来的字（去重、保序）。
    pub fn missing(&self, s: &str) -> Vec<char> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for c in s.chars() {
            if !self.contains(c) && seen.insert(c) {
                out.push(c);
            }
        }
        out
    }

    /// 覆盖率（0.0–1.0），只统计汉字。
    pub fn coverage(&self, s: &str) -> f32 {
        let mut hit = 0usize;
        let mut all = 0usize;
        for c in s.chars() {
            if crate::variants::is_han(c) {
                all += 1;
                if self.contains(c) {
                    hit += 1;
                }
            }
        }
        if all == 0 {
            1.0
        } else {
            hit as f32 / all as f32
        }
    }

    /// 区间段数（调试 / 报告用）。
    pub fn range_count(&self) -> usize {
        self.ranges.len()
    }
}

// ------------------------------------------------------------------
// 底层读取：**每一步都做边界检查**，脏数据只返回 None，绝不 panic。
// ------------------------------------------------------------------

fn be_u16(d: &[u8], off: usize) -> Option<u16> {
    let b = d.get(off..off + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn be_u32(d: &[u8], off: usize) -> Option<u32> {
    let b = d.get(off..off + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn sfnt_offset(d: &[u8]) -> Result<usize, GlyphError> {
    let tag = d.get(0..4).ok_or(GlyphError::TooShort)?;
    match tag {
        b"ttcf" => be_u32(d, 12).map(|o| o as usize).ok_or(GlyphError::TooShort),
        b"\x00\x01\x00\x00" | b"OTTO" | b"true" | b"typ1" => Ok(0),
        _ => Err(GlyphError::NotAFont),
    }
}

/// 在表目录里按 tag 找表，返回绝对偏移。
fn find_table(d: &[u8], base: usize, want: &[u8; 4]) -> Option<usize> {
    let num = be_u16(d, base + 4)? as usize;
    for i in 0..num {
        let rec = base + 12 + i * 16;
        if d.get(rec..rec + 4)? == want {
            return Some(be_u32(d, rec + 8)? as usize);
        }
    }
    None
}

/// format 4：BMP 分段映射。**必须算 idRangeOffset**——
/// 光看 start/end 会把整段都当「有字形」，而段内真有可能指向 glyph 0。
fn parse_format4(d: &[u8], sub: usize, out: &mut HashSet<u32>) {
    let seg_x2 = match be_u16(d, sub + 6) {
        Some(v) => v as usize,
        None => return,
    };
    let seg = seg_x2 / 2;
    if seg == 0 {
        return;
    }
    let end_base = sub + 14;
    let start_base = end_base + seg_x2 + 2; // +2：reservedPad
    let delta_base = start_base + seg_x2;
    let range_base = delta_base + seg_x2;

    for i in 0..seg {
        let (Some(end), Some(start)) = (be_u16(d, end_base + i * 2), be_u16(d, start_base + i * 2))
        else {
            return;
        };
        if start > end {
            continue;
        }
        let delta = be_u16(d, delta_base + i * 2).unwrap_or(0);
        let ro = be_u16(d, range_base + i * 2).unwrap_or(0);

        if ro == 0 {
            for cp in start..=end {
                let g = (cp as u32 + delta as u32) & 0xFFFF;
                if g != 0 {
                    out.insert(cp as u32);
                }
            }
            continue;
        }
        // glyphIdArray 索引：从本 idRangeOffset 位置起算。
        let arr = range_base + i * 2 + ro as usize;
        for cp in start..=end {
            let step = (cp - start) as usize * 2;
            let Some(raw) = be_u16(d, arr + step) else { break };
            if raw == 0 {
                continue;
            }
            let g = (raw as u32 + delta as u32) & 0xFFFF;
            if g != 0 {
                out.insert(cp as u32);
            }
        }
    }
}

/// format 12：分组映射，扩展 B 及以后全靠它。
fn parse_format12(d: &[u8], sub: usize, out: &mut HashSet<u32>) {
    let n = match be_u32(d, sub + 12) {
        Some(v) => v,
        None => return,
    };
    // 上限保护：真字体的分组数在几千量级，超过十万基本是损坏数据。
    for i in 0..n.min(200_000) as usize {
        let g = sub + 16 + i * 12;
        let (Some(s), Some(e), Some(gid)) =
            (be_u32(d, g), be_u32(d, g + 4), be_u32(d, g + 8))
        else {
            return;
        };
        if s > e || gid == 0 {
            continue;
        }
        for cp in s..=e {
            out.insert(cp);
        }
    }
}

/// format 6：密集排列的连续映射（少数老字体用）。
fn parse_format6(d: &[u8], sub: usize, out: &mut HashSet<u32>) {
    let first = be_u16(d, sub + 6).unwrap_or(0) as u32;
    let count = be_u16(d, sub + 8).unwrap_or(0) as usize;
    for i in 0..count {
        let Some(g) = be_u16(d, sub + 10 + i * 2) else { return };
        if g != 0 {
            out.insert(first + i as u32);
        }
    }
}

/// format 0：256 字节单字节表。
fn parse_format0(d: &[u8], sub: usize, out: &mut HashSet<u32>) {
    for i in 0..256usize {
        let Some(&g) = d.get(sub + 6 + i) else { return };
        if g != 0 {
            out.insert(i as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_junk_without_panicking() {
        assert_eq!(GlyphSet::from_bytes(b""), Err(GlyphError::TooShort));
        assert_eq!(GlyphSet::from_bytes(&[0u8; 4]), Err(GlyphError::TooShort));
        let junk = [0xFFu8; 64];
        assert!(GlyphSet::from_bytes(&junk).is_err());
        // 随机字节不许 panic —— 用户会拖任何文件进来。
        for n in [12usize, 40, 200, 5000] {
            let v: Vec<u8> = (0..n).map(|i| (i * 37 % 251) as u8).collect();
            let _ = GlyphSet::from_bytes(&v);
        }
    }

    #[test]
    fn ascii_is_always_considered_present() {
        let g = GlyphSet::default();
        assert!(g.contains('a'));
        assert!(g.contains('。'));
        assert!(!g.contains('一'));
    }

    #[test]
    fn ranges_binary_search_is_correct() {
        // 手工构造区间表：验证二分命中与相邻边界。
        let g = GlyphSet { ranges: vec![(0x4E00, 0x4E10), (0x20000, 0x20005)], total: 21 };
        assert!(g.contains('一'));
        assert!(g.contains('\u{4E10}'));
        assert!(!g.contains('\u{4E11}'));
        assert!(g.contains('\u{20003}'));
        assert!(!g.contains('\u{20006}'));
        assert_eq!(g.len(), 21);
    }

    #[test]
    fn missing_dedups_and_keeps_order() {
        let g = GlyphSet { ranges: vec![(0x4E00, 0x4E00)], total: 1 }; // 只有「一」
        let m = g.missing("一丁丁丙一");
        assert_eq!(m, vec!['丁', '丙']);
    }

    #[test]
    fn coverage_only_counts_han() {
        let g = GlyphSet { ranges: vec![(0x4E00, 0x4E00)], total: 1 };
        // 「一丁」= 1/2，标点不计。
        assert!((g.coverage("一丁。") - 0.5).abs() < 1e-6);
        assert_eq!(g.coverage("abc。"), 1.0);
    }

    /// 真实字体：macOS 自带的 Songti SC 必须能读出来，且认得出常用字。
    #[test]
    fn reads_a_real_system_font_if_available() {
        let path = "/System/Library/Fonts/Supplemental/Songti.ttc";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("跳过：本机没有 {path}");
            return;
        };
        let g = GlyphSet::from_bytes(&bytes).expect("系统字体应该能解析");
        assert!(g.len() > 1000, "字形数太少：{}", g.len());
        assert!(g.contains('道'), "宋体该有「道」");
        // 反过来：扩展 B 是系统宋体也画不出来的，这正是「豆腐块」的来源，
        // 也说明「靠映射表治豆腐块」为什么治不了。
        assert!(!g.contains('𠀀'), "不测到缺口就证明不了这个工具有用");
    }
}
