//! 句读双端校验数据（v1.9.84）。
//!
//! 屏幕走 `cubic_bezier_to`（原生三次曲线），PDF 走 `seed_outline`（折线
//! 采样）。这个例子把**两边真实读到的坐标**原样吐成 JSON，供离线对比图 /
//! PDF 样张使用——对比图不是另画一遍，数据就来自生产代码。
//!
//!     cargo run -p jiandu --example punct_preview --release > /tmp/punct.json

use jiandu::punct;
use serde_json::json;

fn p(q: (f32, f32)) -> serde_json::Value {
    json!([q.0, q.1])
}

fn main() {
    let ds = [4.0f32, 9.27, 17.0, 34.0];
    let mut cubes = serde_json::Map::new();
    let mut pdfs = serde_json::Map::new();
    for d in ds {
        let (a0, segs) = punct::cubic_path(d, false);
        cubes.insert(
            format!("{d}"),
            json!({
                "a0": p(a0),
                "s0": {"c1": p(segs[0].c1), "c2": p(segs[0].c2), "to": p(segs[0].to)},
                "s1": {"c1": p(segs[1].c1), "c2": p(segs[1].c2), "to": p(segs[1].to)},
            }),
        );
        let pts: Vec<_> = punct::seed_outline(d, false).iter().map(|q| p(*q)).collect();
        pdfs.insert(format!("{d}"), json!(pts));
    }
    let old: Vec<_> = punct::seed_outline_belly(9.27, false, punct::SEED_BELLY_AT)
        .iter()
        .map(|q| p(*q))
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "cubic": cubes,
            "pdf": pdfs,
            "old": old,
            "const": {
                "w_of_h": punct::CUBIC_W_OF_H,
                "tilt_deg": punct::CUBIC_TILT_DEG,
                "bleed_scale": punct::CUBIC_BLEED_SCALE,
                "bleed_alpha": punct::CUBIC_BLEED_ALPHA,
                "segments": 2,
            }
        }))
        .unwrap()
    );
}
