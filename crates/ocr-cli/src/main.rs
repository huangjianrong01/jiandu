//! jiandu-ocr —— V2.0 离线预处理 CLI（schema-stub 阶段）
//!
//! 本阶段只负责把「扫描图 → JSON 契约」的文件格式与字段结构跑通。
//! 不接任何真实 OCR 引擎：glyphs / content 均为空壳
//! （占位坐标、空字、confidence=0）。
//! 真实检测将在 L4 对齐阶段接入，届时只需把 `glyphs` 数组填满、
//! `content` 三路（正文/夹注/批注）分离即可，字段契约保持不变。
//!
//! 设计要点（与蓝图对齐）：
//!   - `glyphs[]` 的 {x,y,w,h} 是 L4 AlignmentStore 的最小原型，
//!     导出 TEI 时线性换算成 <zone ulx uly lrx lry>。
//!   - `content` 在第一层就拆成 body / glosses / annotations，
//!     直接映射 TEI <body> / <note type="gloss"> / <note type="annotation">。

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::env;
use std::fs;
use std::path::Path;

// L4 真实引擎：macOS Vision FFI（仅 macOS 编译；其他平台此模块为空壳）。
#[cfg(target_os = "macos")]
mod vision;

const SCHEMA_ID: &str = "jiandu.image-ocr.draft-1";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OcrDoc {
    schema: String,
    tei_compatible: bool,
    source: SourceMeta,
    metadata: MetaData,
    layout: Layout,
    content: Content,
    glyphs: Vec<Glyph>,
    links: Vec<Link>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceMeta {
    book_id: String,
    page_id: String,
    image_path: String,
    image_width: u32,
    image_height: u32,
    dpi: Option<u32>,
    md5: String,
    vol: Option<u32>,
    page: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetaData {
    title: String,
    author: String,
    responsibility: String,
    catalog_ref: String,
    license: String,
    source_edition: String,
    encoding_desc: EncodingDesc,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EncodingDesc {
    project_desc: String,
    editorial_decl: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Layout {
    reading_direction: String,
    columns: u32,
    blocks: Vec<Zone>,
}

/// 矩形区域：版面块 / 文本节点聚合包围盒，→ TEI <zone>
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Zone {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    ulx: i64,
    uly: i64,
    lrx: i64,
    lry: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Content {
    body: Vec<TextNode>,
    glosses: Vec<TextNode>,
    annotations: Vec<TextNode>,
}

/// 文本节点：正文 / 夹注 / 批注 共用，`kind` 区分。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TextNode {
    id: String,
    kind: String,
    text: String,
    glyph_refs: Vec<String>,
    zone: Option<Zone>,
    corresp: Option<String>,
    gloss: Option<GlossMeta>,
    annotation: Option<AnnotationMeta>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GlossMeta {
    place: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationMeta {
    author: String,
    category: String,
    color: String,
}

/// 字格：L4 AlignmentStore 的种子，→ TEI <zone xml:id> + @facs 目标
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Glyph {
    id: String,
    ch: String,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    conf: f32,
    script: String,
    reading: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Link {
    text_ref: String,
    zone_ref: String,
    role: String,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // ── L4 子命令：vision <图片> [-o 输出] ── 真实 macOS Vision OCR
    #[cfg(target_os = "macos")]
    if let Some("vision") = args.get(1).map(|s| s.as_str()) {
        let mut img: Option<String> = None;
        let mut out: Option<String> = None;
        let mut overlay: Option<String> = None;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "-o" | "--output" => {
                    i += 1;
                    out = args.get(i).cloned();
                }
                "--overlay" => {
                    i += 1;
                    overlay = args.get(i).cloned();
                }
                s => {
                    if img.is_none() {
                        img = Some(s.to_string());
                    }
                }
            }
            i += 1;
        }
        let img = img.context("缺少输入（用法：jiandu-ocr vision <图片|目录> [-o 输出] [--overlay 叠加图]）")?;
        return vision::run_vision_cli(
            Path::new(&img),
            out.as_deref().map(Path::new),
            overlay.as_deref().map(Path::new),
        );
    }

    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut book = String::from("unknown");
    let mut vol: Option<u32> = None;
    let mut page: Option<u32> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                output = args.get(i).cloned();
            }
            "--book" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    book = v.clone();
                }
            }
            "--vol" => {
                i += 1;
                vol = args.get(i).and_then(|s| s.parse().ok());
            }
            "--page" => {
                i += 1;
                page = args.get(i).and_then(|s| s.parse().ok());
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            s if s.starts_with('-') && s.len() > 1 => {
                bail!("未知参数：{}", s);
            }
            s => {
                if input.is_none() {
                    input = Some(s.to_string());
                }
            }
        }
        i += 1;
    }

    let input = input.context("缺少输入图像路径（用法见 --help）")?;

    // 只读图像头拿尺寸，不解码整图（省内存、快）
    let (w, h) = image::image_dimensions(&input)
        .with_context(|| format!("无法读取图像尺寸：{}", input))?;

    let page_id = Path::new(&input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page")
        .to_string();

    let doc = OcrDoc {
        schema: SCHEMA_ID.to_string(),
        tei_compatible: true,
        source: SourceMeta {
            book_id: book,
            page_id,
            image_path: input.clone(),
            image_width: w,
            image_height: h,
            dpi: None,
            md5: String::new(), // stub：真实实现用图像字节算 MD5/SHA
            vol,
            page,
        },
        metadata: MetaData {
            title: String::new(),
            author: String::new(),
            responsibility: String::new(),
            catalog_ref: String::new(),
            license: String::new(),
            source_edition: String::new(),
            encoding_desc: EncodingDesc {
                project_desc: String::new(),
                editorial_decl: String::new(),
            },
        },
        layout: Layout {
            reading_direction: "vertical-rl".to_string(),
            columns: 0, // stub：真实版面分析后填
            blocks: vec![],
        },
        content: Content {
            body: vec![],
            glosses: vec![],
            annotations: vec![],
        },
        // stub：占位 glyph，仅用于展示字段形状（空字 / conf:0 / 占位坐标）。
        // 真实 OCR 接管后替换为检测到的字格数组（可能上千个）。
        glyphs: vec![Glyph {
            id: "g000000".to_string(),
            ch: String::new(),
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            conf: 0.0,
            script: String::new(),
            reading: String::new(),
        }],
        links: vec![],
    };

    let json = serde_json::to_string_pretty(&doc)?;

    match output {
        Some(path) => {
            fs::write(&path, &json)
                .with_context(|| format!("无法写入输出文件：{}", path))?;
            eprintln!("✓ 已写出 {}", path);
        }
        None => {
            println!("{}", json);
        }
    }

    Ok(())
}

fn print_help() {
    println!(
        r#"
jiandu-ocr —— V2.0 离线预处理 CLI（schema-stub）

用法：
  jiandu-ocr <输入图像> [选项]

选项：
  -o, --output <文件>   写出 JSON 到文件（默认打印到 stdout）
  --book <id>           书目 id（默认 unknown）
  --vol  <n>            卷号
  --page <n>            页号
  -h, --help            显示本帮助

说明：
  本阶段不接真实 OCR。输出为契约空壳：glyphs 含 1 个占位字格
  （空字 / conf:0 / 占位坐标），content 三路（正文/夹注/批注）为空。
  真实检测在 L4 对齐阶段接入，字段契约保持不变。
"#
    );
}
