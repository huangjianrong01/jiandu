//! 笺读 · V2.0 L4 对齐层 · OCR 提供者契约（纯内核，零平台绑定）
//!
//! 本模块只定义「图片 → 结构化 OCR 结果」的**契约**，以及把结果翻译成
//! 双屏对照模式右栏可消费 JSON 的 [`AlignmentStore`]。
//!
//! **不依赖任何 UI、不依赖 `image` / `objc` / `cfg(target_os)`**，因此完全符合
//! `scripts/check_core_purity.sh` 的 core 纯洁红线——真实引擎（macOS Vision
//! FFI / PaddleOCR）的实现放在独立的 `crates/ocr-cli` 或壳子里，只要
//! `impl OcrProvider` 即可无缝接入。
//!
//! 数据链路（L4 目标，本模块负责中段）：
//!
//! ```text
//! 扫描图 ──▶ OcrProvider::recognize ──▶ OcrResult ──▶ AlignmentStore
//!        └──────────────────────────────────────────▶ to_compare_json()
//!   （与 app.rs::load_compare_json 逐字节兼容）──▶ 右栏渲染 / 点字联动 / 反向联动
//! ```
//!
//! FFI 调研结论（详见仓库 `docs/V2.1-进阶功能排期.md` 与 `crates/ocr-cli`）：
//! - 首选 macOS 原生 Vision：`objc2` + `objc2-vision` 调 `VNRecognizeTextRequest`
//!   / `VNImageRequestHandler`；`topCandidates(1)` 取文本，`elements()` 取逐字
//!   归一化框（origin 在左下角，需 Y 翻转成像素坐标）。离线、免费、走 Neural Engine。
//! - 跨平台兜底 PaddleOCR：`rust-paddle-ocr`（MNN 后端，纯 Rust，支持竖排 / 繁体
//!   / PP-OCRv5）或 `paddle-ocr-rs`（ONNX Runtime），需下载模型、~400MB 内存。

use anyhow::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// OCR 入参：支持「文件路径」或「内存字节」（后者便于测试与内嵌资源）。
#[derive(Debug, Clone)]
pub enum OcrInput {
    /// 磁盘上的图片路径。
    Path(PathBuf),
    /// 内存中的图片字节（PNG / JPEG 等）。
    Bytes(Vec<u8>),
}

impl OcrInput {
    /// 从路径构造。
    pub fn from_path<P: Into<PathBuf>>(p: P) -> Self {
        OcrInput::Path(p.into())
    }
    /// 从字节构造。
    pub fn from_bytes(b: Vec<u8>) -> Self {
        OcrInput::Bytes(b)
    }
}

/// 单字（或最小识别单元）的几何 + 置信信息。
///
/// 坐标统一使用**图像像素坐标系**（原点左上角，x 向右、y 向下），与
/// `app.rs::load_compare_json` / `CompareGlyph` 完全一致——右栏命中框、点字
/// 联动、反向联动无需任何换算即可复用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcrGlyph {
    /// 该字在 [`OcrResult::text`] 中的字符索引（即对照 JSON 的 `target`）。
    pub char_index: usize,
    /// 该字本身（便于校验与调试；落 JSON 时由 `target` 反查，`ch` 不强制落盘）。
    #[serde(default)]
    pub ch: String,
    /// 包围盒左上角 x（像素）。
    pub x: f32,
    /// 包围盒左上角 y（像素）。
    pub y: f32,
    /// 包围盒宽（像素）。
    pub w: f32,
    /// 包围盒高（像素）。
    pub h: f32,
    /// 识别置信度 0..1（Vision / PaddleOCR 均给出）。
    pub confidence: f32,
}

/// 一次 OCR 的完整结果。
///
/// `text` 是右栏正文（按阅读顺序拼接的扁平字符串）；`glyphs` 中每个字的
/// `char_index` 严格指向 `text` 中的字符位置——这是与现有对照 JSON 契约对齐的
/// 关键不变量。真实引擎应保证 `text.chars().count() == glyphs.len()`，且
/// `glyphs[i].char_index == i`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    /// 原始图片宽（像素），用于坐标换算与越界保护。
    pub image_width: u32,
    /// 原始图片高（像素）。
    pub image_height: u32,
    /// 右栏正文（扁平字符串，阅读顺序）。
    pub text: String,
    /// 逐字几何 + 置信。
    pub glyphs: Vec<OcrGlyph>,
    /// 提供者名（调试 / 选择用，例如 `mock` / `vision` / `paddle`）。
    #[serde(default)]
    pub provider: String,
}

/// OCR 提供者契约。任何引擎（Vision / PaddleOCR / 云端 / Mock）只要实现这个
/// trait，就能无缝接入 L4 对齐层——内核只认契约，不认引擎。
pub trait OcrProvider {
    /// 提供者标识（调试 / 多引擎选择用）。
    fn name(&self) -> &str;
    /// 识别一张图，返回结构化结果。
    fn recognize(&self, input: &OcrInput) -> Result<OcrResult>;
}

/// 把一组成员的 (y, x) 升序索引算出来，供 [`AlignmentStore::glyph_at`] 二分命中。
fn build_sorted(glyphs: &[OcrGlyph]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..glyphs.len()).collect();
    idx.sort_by(|&a, &b| {
        glyphs[a]
            .y
            .partial_cmp(&glyphs[b].y)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                glyphs[a]
                    .x
                    .partial_cmp(&glyphs[b].x)
                    .unwrap_or(Ordering::Equal)
            })
    });
    idx
}

/// L4 对齐仓库：把 [`OcrResult`] 固化为「左图坐标 ↔ 右文字符」的双向映射，并
/// 序列化成与 `app.rs::load_compare_json` **逐字节兼容**的 JSON。
///
/// 这层是蓝图 L4 的「落库 + 双向」版——L2 只在内存里跑通了点击联动
/// （`HitTester`），这里把同一思想做成可持久化、可反向查询的纯数据结构，
/// 不依赖任何 UI。
#[derive(Debug, Clone)]
pub struct AlignmentStore {
    /// 原始图片宽（像素）。
    pub image_width: u32,
    /// 原始图片高（像素）。
    pub image_height: u32,
    /// 右栏正文（扁平字符串）。
    pub text: String,
    /// 逐字几何 + 置信。
    pub glyphs: Vec<OcrGlyph>,
    /// 来源引擎名。
    pub provider: String,
    /// 该对齐来源图像（路径或内容哈希），便于落库后回查属于哪张扫描图。
    /// 落 JSON 时存在 `source` 键，旧 app 加载器会忽略未知键，零冲突。
    pub source_image: Option<String>,
    /// glyph 索引按 (y, x) 升序，供 `glyph_at` 二分。
    sorted: Vec<usize>,
}

impl AlignmentStore {
    /// 从 OCR 结果直接构建（自动计算排序索引）。
    pub fn from_ocr(result: OcrResult) -> Self {
        let sorted = build_sorted(&result.glyphs);
        Self {
            image_width: result.image_width,
            image_height: result.image_height,
            text: result.text,
            glyphs: result.glyphs,
            provider: result.provider,
            source_image: None,
            sorted,
        }
    }

    /// 序列化成对照 JSON：顶层 `image_width` / `image_height` / `text` /
    /// `glyphs`，glyph 输出 `{x, y, w, h, target, confidence}`，其中
    /// `target == char_index`。该形状与 `app.rs::load_compare_json` **完全兼容**
    /// （多出的 `confidence` 会被旧加载器忽略）。
    pub fn to_compare_json(&self) -> serde_json::Value {
        serde_json::json!({
            "image_width": self.image_width,
            "image_height": self.image_height,
            "text": self.text,
            "glyphs": self.glyphs.iter().map(|g| serde_json::json!({
                "x": g.x,
                "y": g.y,
                "w": g.w,
                "h": g.h,
                "target": g.char_index,
                "confidence": g.confidence,
            })).collect::<Vec<_>>(),
        })
    }

    /// 从对照 JSON 解析（与 app 的加载器同形），用于校验链路往返，也便于
    /// 从磁盘已有的对照 JSON 重建对齐仓库。
    pub fn from_compare_json(v: &serde_json::Value) -> Option<Self> {
        let iw = v.get("image_width")?.as_u64()? as u32;
        let ih = v.get("image_height")?.as_u64()? as u32;
        let text = v
            .get("text")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let mut glyphs = Vec::new();
        if let Some(arr) = v.get("glyphs").and_then(|a| a.as_array()) {
            for g in arr {
                let x = g.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let y = g.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let w = g.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let h = g.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let char_index =
                    g.get("target").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let confidence = g
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                glyphs.push(OcrGlyph {
                    char_index,
                    ch: String::new(),
                    x,
                    y,
                    w,
                    h,
                    confidence,
                });
            }
        }
        let sorted = build_sorted(&glyphs);
        Some(Self {
            image_width: iw,
            image_height: ih,
            text,
            glyphs,
            provider: "json".to_string(),
            source_image: v
                .get("source")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
            sorted,
        })
    }

    /// 正向查询：给定图像像素坐标，返回命中的**字符索引**（O(log n + k)，与
    /// `app.rs::HitTester` 同构）。用于「点左图某字 → 右文对应字符亮红框」。
    pub fn glyph_at(&self, x: f32, y: f32) -> Option<usize> {
        let rows = &self.sorted;
        // 二分：找第一个 y > py 的下标 lo，再从 lo-1 向前回扫，直到 y+h < py。
        let mut lo = 0usize;
        let mut hi = rows.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.glyphs[rows[mid]].y > y {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let mut i = lo;
        while i > 0 {
            i -= 1;
            let r = &self.glyphs[rows[i]];
            if r.y + r.h < y {
                break; // 已越过 py 所在行上方，更高的字不可能命中
            }
            if x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h {
                return Some(r.char_index);
            }
        }
        None
    }

    /// 反向查询：给定字符索引，返回其在左图的包围盒 `(x, y, w, h)`。用于
    /// 「点右文字 → 左图同源字形红框」（L2 反向联动的落库版）。
    pub fn char_box(&self, idx: usize) -> Option<(f32, f32, f32, f32)> {
        self.glyphs
            .get(idx)
            .map(|g| (g.x, g.y, g.w, g.h))
    }

    /// L5 落库：把对齐仓库序列化到磁盘（JSON 文件）。
    ///
    /// 文件格式 = [`to_compare_json`] 的完整形状（与 `app.rs::load_compare_json`
    /// **逐字节兼容**）再附加 `source` / `provider` / `format_version` 三个元数据键
    /// —— 旧加载器忽略未知键，混合使用零冲突。纯 `std::fs`，不碰任何平台 / UI，
    /// 符合 core 纯洁红线。
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let mut v = self.to_compare_json();
        v["source"] = serde_json::json!(self.source_image);
        v["provider"] = serde_json::json!(self.provider);
        v["format_version"] = serde_json::json!(1u32);
        let s = serde_json::to_string_pretty(&v)?;
        std::fs::write(path.as_ref(), s)
            .with_context(|| format!("写对齐文件失败：{}", path.as_ref().display()))?;
        Ok(())
    }

    /// L5 入库：从 [`save`] 写出的对齐文件重建对齐仓库（`source` / `provider`
    /// 一并还原）。文件不存在 / 格式非法返回错误。
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let p = path.as_ref();
        let raw = std::fs::read_to_string(p)
            .with_context(|| format!("读对齐文件失败：{}", p.display()))?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        let mut store = Self::from_compare_json(&v)
            .ok_or_else(|| anyhow::anyhow!("对齐文件格式不合法：{}", p.display()))?;
        store.source_image = v
            .get("source")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        store.provider = v
            .get("provider")
            .and_then(|s| s.as_str())
            .unwrap_or("json")
            .to_string();
        Ok(store)
    }
}

/// L5 对齐库：把多张扫描图的对齐仓库按 key 落盘到一个目录，便于整本书的对照
/// 数据随工程一起保存 / 重载。纯 `std::fs`，不依赖任何平台或 UI，符合 core
/// 纯洁红线——壳子（macOS / Windows / Linux）只要持有一个目录路径即可复用。
pub struct AlignmentLibrary {
    /// 库目录（`.align.json` 文件的容器）。
    dir: PathBuf,
}

impl AlignmentLibrary {
    /// 打开（或创建）一个对齐库目录。
    pub fn open<P: Into<PathBuf>>(dir: P) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建对齐库目录失败：{}", dir.display()))?;
        Ok(Self { dir })
    }

    /// 把 key 安全化为文件名（去除路径分隔符，避免越界写）。
    fn file_of(&self, key: &str) -> PathBuf {
        let safe: String = key
            .chars()
            .map(|c| if c == '/' || c == '\\' { '_' } else { c })
            .collect();
        self.dir.join(format!("{}.align.json", safe))
    }

    /// 写入一条对齐（key 一般用源图文件名 stem 或内容哈希）。
    pub fn insert(&self, key: &str, store: &AlignmentStore) -> Result<()> {
        store.save(self.file_of(key))
    }

    /// 读取一条对齐；不存在返回 `Ok(None)`。
    pub fn get(&self, key: &str) -> Result<Option<AlignmentStore>> {
        let p = self.file_of(key);
        if !p.exists() {
            return Ok(None);
        }
        Ok(Some(AlignmentStore::load(p)?))
    }

    /// 删除一条对齐；不存在返回 `Ok(false)`。
    pub fn remove(&self, key: &str) -> Result<bool> {
        let p = self.file_of(key);
        if p.exists() {
            std::fs::remove_file(&p)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 列出库内所有 key（即所有 `*.align.json` 的文件名 stem，已排序）。
    pub fn list(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".align.json") {
                keys.push(stem.to_string());
            }
        }
        keys.sort();
        Ok(keys)
    }

    /// key 是否存在于库内。
    pub fn contains(&self, key: &str) -> bool {
        self.file_of(key).exists()
    }
}

/// 假数据提供者：在固定网格上铺一排假字，坐标 / 置信齐备。
///
/// 用途：无需真引擎即可让「图 → OCR → JSON → AlignmentStore → 右栏」整条链路
/// 在测试中无缝流转，并作为真实引擎接入时的对照基线。
pub struct MockOcrProvider {
    /// 假图宽（像素）。
    pub image_width: u32,
    /// 假图高（像素）。
    pub image_height: u32,
    /// 假正文（扁平，阅读顺序，不含换行——保证 `char_index == 位置`）。
    pub text: String,
    /// 每字基准宽（像素）。
    pub cell_w: f32,
    /// 每字基准高（像素）。
    pub cell_h: f32,
}

impl MockOcrProvider {
    /// 构造。`cols` 为每行字数（用于把扁平 text 摊成网格坐标）。
    pub fn new(image_width: u32, image_height: u32, text: &str, cell_w: f32, cell_h: f32) -> Self {
        Self {
            image_width,
            image_height,
            text: text.to_string(),
            cell_w,
            cell_h,
        }
    }
}

impl OcrProvider for MockOcrProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn recognize(&self, _input: &OcrInput) -> Result<OcrResult> {
        let chars: Vec<char> = self.text.chars().collect();
        let cols = ((self.image_width as f32) / self.cell_w).floor().max(1.0) as usize;
        let mut glyphs = Vec::with_capacity(chars.len());
        for (i, &c) in chars.iter().enumerate() {
            if c == '\n' {
                continue; // 不产换行占位字，保持 char_index == 扁平位置
            }
            let col = i % cols;
            let row = i / cols;
            let x = col as f32 * self.cell_w;
            let y = row as f32 * self.cell_h;
            glyphs.push(OcrGlyph {
                char_index: i,
                ch: c.to_string(),
                x,
                y,
                w: self.cell_w,
                h: self.cell_h,
                confidence: 0.99,
            });
        }
        Ok(OcrResult {
            image_width: self.image_width,
            image_height: self.image_height,
            text: self.text.clone(),
            glyphs,
            provider: "mock".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MockOcrProvider {
        MockOcrProvider::new(
            1000,
            800,
            "天地玄黃宇宙洪荒日月盈昃辰宿列張寒來暑往秋收冬藏閏餘成歲律呂調陽雲騰致雨露結為霜",
            100.0,
            80.0,
        )
    }

    #[test]
    fn mock_produces_char_index_aligned_glyphs() {
        let p = sample();
        let r = p.recognize(&OcrInput::from_bytes(vec![])).unwrap();
        assert_eq!(r.glyphs.len(), r.text.chars().count());
        for (i, g) in r.glyphs.iter().enumerate() {
            assert_eq!(g.char_index, i, "char_index 必须严格等于扁平位置");
            assert_eq!(g.ch, r.text.chars().nth(i).unwrap().to_string());
            assert!((g.confidence - 0.99).abs() < 1e-6);
        }
    }

    #[test]
    fn chain_roundtrip_to_compare_json() {
        let p = sample();
        let r = p.recognize(&OcrInput::from_bytes(vec![])).unwrap();
        let store = AlignmentStore::from_ocr(r.clone());
        let json = store.to_compare_json();

        // 形状与 app.rs::load_compare_json 兼容
        assert_eq!(json["image_width"], 1000);
        assert_eq!(json["image_height"], 800);
        assert_eq!(json["text"], r.text);
        let g0 = &json["glyphs"][0];
        assert_eq!(g0["x"], 0.0);
        assert_eq!(g0["target"], 0);

        // 往返：JSON → AlignmentStore → 再 JSON 逐字段一致
        let store2 = AlignmentStore::from_compare_json(&json).unwrap();
        assert_eq!(store2.text, store.text);
        assert_eq!(store2.glyphs.len(), store.glyphs.len());
        assert_eq!(store2.glyphs[0].x, store.glyphs[0].x);
        assert_eq!(store2.glyphs[0].char_index, store.glyphs[0].char_index);
        let json2 = store2.to_compare_json();
        assert_eq!(json, json2, "往返后 JSON 必须完全一致");
    }

    #[test]
    fn forward_and_reverse_lookup() {
        let p = sample();
        let r = p.recognize(&OcrInput::from_bytes(vec![])).unwrap();
        let store = AlignmentStore::from_ocr(r);

        // 正向：点第 0 个字中心 → 命中 idx 0
        let g0 = &store.glyphs[0];
        let hit = store.glyph_at(g0.x + g0.w / 2.0, g0.y + g0.h / 2.0);
        assert_eq!(hit, Some(0));

        // 反向：idx 5 的包围盒应与 glyphs[5] 一致
        let (x, y, w, h) = store.char_box(5).unwrap();
        assert!((x - store.glyphs[5].x).abs() < 1e-6);
        assert!((y - store.glyphs[5].y).abs() < 1e-6);
        assert_eq!(w, store.glyphs[5].w);
        assert_eq!(h, store.glyphs[5].h);
    }

    #[test]
    fn compatible_with_app_loader_shape() {
        // 确保 to_compare_json 产出的 key 集合正好是 app 加载器读取的那几个
        let p = sample();
        let r = p.recognize(&OcrInput::from_bytes(vec![])).unwrap();
        let store = AlignmentStore::from_ocr(r);
        let json = store.to_compare_json();
        let top_keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        for k in ["image_width", "image_height", "text", "glyphs"] {
            assert!(
                top_keys.contains(&&k.to_string()),
                "对照 JSON 缺顶层键 {k}"
            );
        }
        let gk: Vec<&String> = json["glyphs"][0].as_object().unwrap().keys().collect();
        for k in ["x", "y", "w", "h", "target"] {
            assert!(gk.contains(&&k.to_string()), "对照 JSON glyph 缺键 {k}");
        }
    }

    #[test]
    fn l5_save_load_roundtrip_preserves_everything() {
        let p = sample();
        let r = p.recognize(&OcrInput::from_bytes(vec![])).unwrap();
        let mut store = AlignmentStore::from_ocr(r);
        store.source_image = Some("page001.jpeg".to_string());
        let tmp = std::env::temp_dir().join("jiandu_align_rt.json");
        store.save(&tmp).unwrap();
        let loaded = AlignmentStore::load(&tmp).unwrap();
        assert_eq!(loaded.text, store.text);
        assert_eq!(loaded.glyphs.len(), store.glyphs.len());
        assert_eq!(loaded.glyphs[0].x, store.glyphs[0].x);
        assert_eq!(loaded.source_image, store.source_image);
        assert_eq!(loaded.provider, store.provider);
        // 排序索引重建后，正向/反向查询结果与落库前一致
        let g = &store.glyphs[3];
        assert_eq!(
            loaded.glyph_at(g.x + g.w / 2.0, g.y + g.h / 2.0),
            Some(g.char_index)
        );
        assert_eq!(loaded.char_box(3), Some((g.x, g.y, g.w, g.h)));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn l5_alignment_library_insert_get_list_remove() {
        let tmp = std::env::temp_dir().join("jiandu_align_lib");
        let _ = std::fs::remove_dir_all(&tmp);
        let lib = AlignmentLibrary::open(&tmp).unwrap();
        let p = sample();
        let store = AlignmentStore::from_ocr(p.recognize(&OcrInput::from_bytes(vec![])).unwrap());
        lib.insert("page001", &store).unwrap();
        assert!(lib.contains("page001"));
        let got = lib.get("page001").unwrap().unwrap();
        assert_eq!(got.text, store.text);
        assert_eq!(got.glyphs.len(), store.glyphs.len());
        assert_eq!(lib.list().unwrap(), vec!["page001".to_string()]);
        assert!(lib.remove("page001").unwrap());
        assert!(!lib.contains("page001"));
        assert!(lib.get("page001").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
