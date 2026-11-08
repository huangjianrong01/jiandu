//! L3 瓦片渲染（Tile Scheduler）：按视口懒加载切片 + LRU 缓存。
//! 仅在 Compare Mode 左图使用，零侵入 L1/L2（双向联动、滚动补间）与 core 主引擎。
//! 切片直接调现成 `image::imageops::crop_imm` + `to_image`，不写任何几何推导。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

pub struct TileCache {
    tile: u32,
    cap: usize,
    dir: PathBuf,
    map: HashMap<(u32, u32), PathBuf>,
    order: VecDeque<(u32, u32)>,
    src: Option<image::DynamicImage>,
    failed: bool,
    /// 换图代次号：写进瓦片文件名。GPUI 的 `img(path)` 按路径缓存纹理，
    /// 若翻页后瓦片仍叫 `t_0_0.png`，新图会命中旧纹理 → 左图永远停在第一页。
    /// 每次 reset() 递增，路径必变，强制重新加载。
    generation: u32,
}

impl TileCache {
    pub fn new(tile: u32, cap: usize) -> Self {
        let dir = std::env::temp_dir().join(format!("jiandu_tiles_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        Self {
            tile,
            cap,
            dir,
            map: HashMap::new(),
            order: VecDeque::new(),
            src: None,
            failed: false,
            generation: 0,
        }
    }

    fn ensure_src(&mut self, display: &Path) {
        if self.src.is_some() || self.failed {
            return;
        }
        match image::open(display) {
            Ok(img) => self.src = Some(img),
            Err(_) => self.failed = true,
        }
    }

    /// 返回 (col,row) 瓦片的临时 PNG 路径；首访才裁切编码，之后从 LRU 复用，避免重复解码。
    pub fn tile(&mut self, display: &Path, nw: u32, nh: u32, col: u32, row: u32) -> Option<PathBuf> {
        if self.failed {
            return None;
        }
        self.ensure_src(display);
        let src = self.src.as_ref()?;
        let key = (col, row);
        if let Some(p) = self.map.get(&key) {
            let p = p.clone();
            self.touch(key);
            return Some(p);
        }
        let t = self.tile;
        let x0 = col * t;
        let y0 = row * t;
        if x0 >= nw || y0 >= nh {
            return None;
        }
        let w = (x0 + t).min(nw) - x0;
        let h = (y0 + t).min(nh) - y0;
        // 直接调现成 image crate 切片，不手写像素循环。
        let sub = image::imageops::crop_imm(src, x0, y0, w, h);
        let tile_img = sub.to_image();
        let p = self
            .dir
            .join(format!("t_{}_{}_{}.png", self.generation, col, row));
        if tile_img.save(&p).is_err() {
            return None;
        }
        self.insert(key, p.clone());
        Some(p)
    }

    fn touch(&mut self, key: (u32, u32)) {
        if let Some(pos) = self.order.iter().position(|&k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key);
    }

    fn insert(&mut self, key: (u32, u32), p: PathBuf) {
        self.map.insert(key, p);
        self.order.push_back(key);
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                if let Some(op) = self.map.remove(&old) {
                    let _ = std::fs::remove_file(&op);
                }
            }
        }
    }

    /// 视口计算：缩放 `s`（每源像素显示 px）、滚动 offset(ox,oy)、视口尺寸(vw,vh)
    /// → 可见瓦片范围 (c0,r0,c1,r1)。纯现成几何，无复杂推导。
    pub fn visible_range(
        nw: u32,
        nh: u32,
        tile: u32,
        s: f32,
        ox: f32,
        oy: f32,
        vw: f32,
        vh: f32,
    ) -> (u32, u32, u32, u32) {
        let td = tile as f32 * s;
        if td <= 0.0 {
            return (0, 0, 0, 0);
        }
        let cols = (nw + tile - 1) / tile;
        let rows = (nh + tile - 1) / tile;
        let c0 = ((ox / td).floor().max(0.0)) as u32;
        let r0 = ((oy / td).floor().max(0.0)) as u32;
        let c1 = (((ox + vw) / td).ceil().max(0.0)) as u32;
        let r1 = (((oy + vh) / td).ceil().max(0.0)) as u32;
        let c1 = c1.min(cols.saturating_sub(1));
        let r1 = r1.min(rows.saturating_sub(1));
        (c0.min(c1), r0.min(r1), c1, r1)
    }

    pub fn tile_size(&self) -> u32 {
        self.tile
    }

    /// 换图时清缓存 + 删临时文件（防旧图瓦片串味）。
    pub fn reset(&mut self) {
        for p in self.map.values() {
            let _ = std::fs::remove_file(p);
        }
        self.map.clear();
        self.order.clear();
        self.src = None;
        self.failed = false;
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归断言：换页后瓦片**路径必须改变**，否则 GPUI 的 `img(path)` 会
    /// 复用上一页的纹理（用户实测「页码变了、左图还停在首页」的根因）。
    #[test]
    fn tile_path_changes_after_reset() {
        let p = std::env::temp_dir().join("tile_smoke_src.png");
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            600,
            400,
            image::Rgba([255, 255, 255, 255]),
        ));
        img.save(&p).expect("写测试图失败");
        let mut c = TileCache::new(512, 16);
        let a = c.tile(&p, 600, 400, 0, 0).expect("首图瓦片");
        c.reset();
        let b = c.tile(&p, 600, 400, 0, 0).expect("换页瓦片");
        assert_ne!(a, b, "换页后瓦片路径必须变化，否则 GPUI 复用旧纹理");
        let _ = std::fs::remove_file(&p);
    }
}
