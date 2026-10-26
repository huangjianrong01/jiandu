//! 天头眉批的**画布落位**与引线几何（纯计算，零 GPUI 依赖，可单测）。
//!
//! ## 为什么单开一个模块
//!
//! 旧制（`types::HeadNote`）把眉批存成天头里的归一化 `u/v`，渲染时用
//! `div()` 绝对定位——换字号、换留白后只是等比缩放，**它并不知道自己批的是
//! 哪个字**。新制（`types::MarginNote`）只存 `anchor_char_index`，屏幕坐标
//! 每帧由当帧版面反算出来。反算之后的「怎么排进天头那条窄带」就是本模块的活：
//!
//! * **x 跟着锚定字走**——谁批的字，批语就压在谁头顶上；
//! * **x 撞车就沿 Y 轴下移**——后写的眉批依次往下让，永不叠成一团墨
//!   （用户点名的防重叠要求）；
//! * **天头不够高就自动分列**——竖排写完一列往左再来一列（刻本眉批就是
//!   这么排的），一列排多少字由天头高度反算，绝不探进版框盖住正文；
//! * **实在排不下就截断**（末字换成「…」）或整条不画——宁可少一条，
//!   也绝不让眉批压到正文上。
//!
//! 引线的控制点也在这里算：出笔垂直于字柱向下、收笔斜落到锚定字，
//! 这样即使眉批被挤到下边车道、与锚定字错开半列，线条仍看得出是从哪条
//! 批语牵下来的。

/// 一条眉批的**输入**：锚定字在纸面坐标里的位置 + 批语。
///
/// 坐标一律是**纸面（sheet）坐标**：原点 = 纸面左上角，与
/// [`crate::hit::GlyphBox`] 同一套口径换算而来（见 `reader::render_margin_layer`
/// 里的 `FRAME_OFF`）。
#[derive(Clone, Debug)]
pub struct MarginInput {
    /// 锚定字**中心** x。
    pub ax: f32,
    /// 锚定字**顶缘** y（引线就收到这里，不压字）。
    pub ay: f32,
    /// 批语正文。
    pub text: String,
}

/// 一条眉批在天头里的**落位**（纸面坐标）。
#[derive(Clone, Debug)]
pub struct MarginBox {
    /// 对应 [`layout_notes`] 入参的下标（返回值按此升序排好）。
    pub src: usize,
    /// 字柱区**左缘** x（最左那一列的外侧）。
    pub x: f32,
    /// 字柱区**顶缘** y。
    pub y: f32,
    /// 字柱区总宽 = `(列数-1) × 列距 + 字宽`。
    pub w: f32,
    /// 字柱区高 = `每列字数 × 行距`。
    pub h: f32,
    /// 字宽（= 字号）。
    pub fs: f32,
    /// 相邻两列的间距（列距）。
    pub pitch: f32,
    /// 每列字数。
    pub per_col: usize,
    /// 实际显示的字串（可能被天头高度截断，末字换成「…」）。
    pub text: String,
    /// 引线**起点** x：贴着锚定字的那一列（眉批被挤偏时仍在字柱区内）。
    pub lx: f32,
    /// 引线**终点**（锚定字顶缘中点）。
    pub ax: f32,
    pub ay: f32,
}

impl MarginBox {
    /// 列数（竖排分了几列）。
    pub fn cols(&self) -> usize {
        let n = self.text.chars().count();
        if n == 0 || self.per_col == 0 {
            return 0;
        }
        (n + self.per_col - 1) / self.per_col
    }

    /// 第 `k` 列（**0 = 最右一列**，竖排自右向左读）的字。
    pub fn col_text(&self, k: usize) -> String {
        self.text
            .chars()
            .skip(k * self.per_col)
            .take(self.per_col)
            .collect()
    }

    /// 第 `k` 列的**左缘** x（最右列 k=0 摆在区右端）。
    pub fn col_x(&self, k: usize) -> f32 {
        let cols = self.cols().max(1) as f32;
        self.x + (cols - 1.0 - k as f32) * self.pitch
    }

    /// 命中测试（悬停联动）：点是否落在这条眉批的字柱区里。
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// 一列的**列距**倍率（字宽 × 此值）。
///
/// 竖排小字列与列之间要留一线空隙，否则两列笔画贴成一片墨；1.18 给的是
/// 「看得清是两列、又不至于散开」的距离。
pub const COL_PITCH: f32 = 1.18;
/// 两条眉批错行时的**行间空隙**（px）。
pub const LANE_GAP: f32 = 5.0;
/// 判定「x 撞车」时向两侧各放宽的余量（× 列距）。
///
/// 0.35 列距 ≈ 半个字多一点：真挨在一起的两条批语要错开，但差出半个字
/// 以上的不会白白被挤出一条新车道。
pub const X_PAD: f32 = 0.35;
/// 一条眉批最多占天头宽度的此比例。
///
/// 一条批语横贯整版会把别人的位置全占了（天头横向是公共资源），
/// 故封顶在天头宽的一半多一点。
pub const BAND_W_FRAC: f32 = 0.55;

/// 把一批眉批排进天头那条窄带。
///
/// * `band_w` / `band_h` —— 天头带的宽高（纸面坐标；`band_h` 即引擎算出的
///   天头 `margin_top`）。
/// * `fs` / `lh` —— 眉批字号与行距。
///
/// 返回按 `src` 升序排好的落位表；**排不下的整条不出现在返回值里**
/// （调用方据此知道它这一帧没画出来）。
pub fn layout_notes(
    inputs: &[MarginInput],
    band_w: f32,
    band_h: f32,
    fs: f32,
    lh: f32,
) -> Vec<MarginBox> {
    if inputs.is_empty() || band_w <= fs || band_h < lh || lh <= 0.0 || fs <= 0.0 {
        return Vec::new();
    }
    let pitch = fs * COL_PITCH;
    // 一列最多排几个字：由天头高度反算（竖排分列排，绝不探进版框）。
    let per_col_max = (band_h / lh).floor() as usize;
    if per_col_max == 0 {
        return Vec::new();
    }
    // 最多分几列：占满天头宽度的 BAND_W_FRAC 为止。
    let max_cols = ((band_w * BAND_W_FRAC) / pitch).floor() as usize;
    if max_cols == 0 {
        return Vec::new();
    }
    // 一条眉批在这条天头里最多能显示多少字。
    let cap = per_col_max * max_cols;

    // 自左向右扫（x 升序），保证「谁先占位谁在上」与视觉顺序一致，
    // 与批语的书写顺序无关——结果只由 x 决定，重排后不会跳位。
    let mut order: Vec<usize> = (0..inputs.len()).collect();
    order.sort_by(|&a, &b| {
        inputs[a]
            .ax
            .partial_cmp(&inputs[b].ax)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });

    let pad = pitch * X_PAD;
    let mut placed: Vec<MarginBox> = Vec::new();
    let mut out: Vec<MarginBox> = Vec::new();
    for i in order {
        let inp = &inputs[i];
        let chars: Vec<char> = inp.text.chars().collect();
        if chars.is_empty() {
            continue;
        }
        // 天头装不下就截断：末位让给「…」，读者知道这条没完。
        let text: String = if chars.len() <= cap {
            inp.text.clone()
        } else if cap >= 2 {
            let mut t: String = chars[..cap - 1].iter().collect();
            t.push('…');
            t
        } else {
            chars[..1].iter().collect()
        };
        let m = text.chars().count();
        let cols = ((m + per_col_max - 1) / per_col_max).min(max_cols).max(1);
        let per_col = ((m + cols - 1) / cols).min(per_col_max).max(1);
        let w = (cols as f32 - 1.0) * pitch + fs;
        let h = per_col as f32 * lh;
        // 字柱区居中压在锚定字上；贴边时夹回带内，绝不探出纸缘。
        let x = (inp.ax - w * 0.5).clamp(0.0, (band_w - w).max(0.0));
        // 防重叠：先看这条与已排的哪些在横向上撞车，取它们里**最低的底边**
        // 往下让一格——这就是「后写的眉批在 Y 轴上下移」。
        let mut top = 0.0f32;
        let mut hit_any = false;
        for b in placed.iter() {
            if x - pad < b.x + b.w + pad && b.x - pad < x + w + pad {
                hit_any = true;
                top = top.max(b.y + b.h);
            }
        }
        let top = if hit_any { top + LANE_GAP } else { 0.0 };
        // 让下去就出带 → 这条这一帧画不下（宁可不画，绝不叠字 / 压正文）。
        if top + h > band_h {
            continue;
        }
        let lx = inp.ax.clamp(x + fs * 0.5, (x + w - fs * 0.5).max(x + fs * 0.5));
        let mb = MarginBox {
            src: i,
            x,
            y: top,
            w,
            h,
            fs,
            pitch,
            per_col,
            text,
            lx,
            ax: inp.ax,
            ay: inp.ay,
        };
        placed.push(mb.clone());
        out.push(mb);
    }
    out.sort_by_key(|b| b.src);
    out
}

/// 引线的**二次贝塞尔控制点**：出笔垂直于字柱向下、收笔斜落到锚定字。
///
/// 控制点取在起点正下方（垂直走 62% 的落差），于是曲线起笔是竖直的——
/// 视觉上像从批语最后一字笔锋顺势拖下来，再斜着搭到被批的那个字上。
pub fn leader_ctrl(x0: f32, y0: f32, x1: f32, y1: f32) -> (f32, f32) {
    let dy = (y1 - y0).max(1.0);
    (x0, y0 + dy * 0.62)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(ax: f32, ay: f32, text: &str) -> MarginInput {
        MarginInput {
            ax,
            ay,
            text: text.to_string(),
        }
    }

    /// 天头 600×96、字号 14 / 行距 18（≈ 真机 12% 天头率下的量级）。
    const BW: f32 = 600.0;
    const BH: f32 = 96.0;
    const FS: f32 = 14.0;
    const LH: f32 = 18.0;

    #[test]
    fn note_sits_right_over_its_anchor() {
        let out = layout_notes(&[inp(300.0, 140.0, "道可道")], BW, BH, FS, LH);
        assert_eq!(out.len(), 1);
        let b = &out[0];
        // 锚定字必须落在字柱区的横向范围内——引线才不会莫名其妙斜着走。
        assert!(
            b.ax >= b.x - 0.01 && b.ax <= b.x + b.w + 0.01,
            "锚定字 {} 不在字柱区 [{}, {}] 内",
            b.ax,
            b.x,
            b.x + b.w
        );
        // 竖排一列写完：3 字 → 1 列。
        assert_eq!(b.cols(), 1);
        assert_eq!(b.col_text(0), "道可道");
        // 绝不探进版框：底边不超过天头带。
        assert!(b.y + b.h <= BH + 0.01, "探进版框了：{}", b.y + b.h);
    }

    #[test]
    fn long_note_wraps_into_more_columns() {
        // 一列最多 5 字（96/18），11 字 → 3 列，绝不纵向撑破天头。
        let out = layout_notes(&[inp(300.0, 140.0, "批曰此本与索隐异文")], BW, BH, FS, LH);
        assert_eq!(out.len(), 1);
        let b = &out[0];
        assert_eq!(b.per_col, 5);
        assert_eq!(b.cols(), 2); // 9 字 → 2 列
        assert!(b.y + b.h <= BH + 0.01);
        // 竖排自右向左：第 0 列是最右那列，取前 5 字。
        assert_eq!(b.col_text(0), "批曰此本与");
        assert_eq!(b.col_text(1), "索隐异文");
        assert!(b.col_x(0) > b.col_x(1), "第 0 列必须在右");
    }

    #[test]
    fn overlapping_notes_stagger_down() {
        // 两条批语锚在相邻的两个字上：第二条必须整体下移，绝不叠字。
        let out = layout_notes(
            &[inp(300.0, 140.0, "甲批"), inp(306.0, 140.0, "乙批")],
            BW,
            BH,
            FS,
            LH,
        );
        assert_eq!(out.len(), 2, "两条都该排得下");
        let (a, b) = (&out[0], &out[1]);
        assert!(
            b.y >= a.y + a.h - 0.01,
            "第二条没下移：{} vs {}",
            b.y,
            a.y + a.h
        );
        // 两个框不得相交。
        let disjoint = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
        assert!(disjoint, "两条眉批叠在一起了：{:?} / {:?}", a, b);
        assert!(b.y + b.h <= BH + 0.01);
    }

    #[test]
    fn three_nearby_notes_stack_in_order() {
        let out = layout_notes(
            &[
                inp(300.0, 140.0, "一"),
                inp(302.0, 140.0, "二"),
                inp(304.0, 140.0, "三"),
            ],
            BW,
            BH,
            FS,
            LH,
        );
        assert_eq!(out.len(), 3);
        assert!(out[0].y < out[1].y && out[1].y < out[2].y, "没依次下移");
        for b in &out {
            assert!(b.y + b.h <= BH + 0.01);
        }
    }

    #[test]
    fn far_apart_notes_share_the_top() {
        let out = layout_notes(
            &[inp(120.0, 140.0, "甲批"), inp(480.0, 140.0, "乙批")],
            BW,
            BH,
            FS,
            LH,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].y, 0.0);
        assert_eq!(out[1].y, 0.0, "隔得远的两条不该白白往下让");
    }

    #[test]
    fn result_is_sorted_by_input_index() {
        // 入参乱序（第二条 x 更小），返回值仍按入参下标升序——调用方
        // 拿返回值按 src 取 id 时才不会张冠李戴。
        let out = layout_notes(
            &[inp(480.0, 140.0, "后写"), inp(120.0, 140.0, "先写")],
            BW,
            BH,
            FS,
            LH,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].src, 0);
        assert_eq!(out[1].src, 1);
    }

    #[test]
    fn narrow_band_truncates_with_ellipsis() {
        // 天头只剩一列宽：11 字必然截断，末字让给「…」。
        let out = layout_notes(&[inp(20.0, 140.0, "批曰此本与索隐异文")], 44.0, BH, FS, LH);
        assert_eq!(out.len(), 1);
        let b = &out[0];
        assert_eq!(b.cols(), 1, "窄带只能排一列");
        assert!(
            b.text.ends_with('…'),
            "超长批语没截断：{}",
            b.text
        );
        assert!(b.y + b.h <= BH + 0.01);
    }

    #[test]
    fn impossible_band_draws_nothing() {
        // 天头比一个字还矮：硬画必然压到正文，故一条都不排。
        let out = layout_notes(&[inp(300.0, 140.0, "批")], BW, 6.0, FS, LH);
        assert!(out.is_empty(), "天头太窄时不该画");
    }

    #[test]
    fn leader_starts_under_the_note_and_lands_on_the_char() {
        let (cx, cy) = leader_ctrl(300.0, 40.0, 306.0, 140.0);
        assert!((cx - 300.0).abs() < 0.01, "出笔必须是竖直的：{}", cx);
        assert!(cy > 40.0 && cy < 140.0, "控制点该落在落差中段：{}", cy);
    }

    #[test]
    fn hit_test_finds_the_note_under_the_pointer() {
        let out = layout_notes(&[inp(300.0, 140.0, "道可道")], BW, BH, FS, LH);
        let b = &out[0];
        assert!(b.contains(b.x + b.w * 0.5, b.y + 2.0), "框内应命中");
        assert!(!b.contains(b.x - 5.0, b.y + 2.0), "框外不该命中");
        assert!(!b.contains(b.x + b.w * 0.5, b.y + b.h + 8.0), "框下不该命中");
    }

    #[test]
    fn a_crowd_of_notes_never_overlaps() {
        // 八条批语挤在相邻的几个字上：能排下的必须两两不叠，排不下的不画。
        let inputs: Vec<MarginInput> = (0..8)
            .map(|i| inp(280.0 + i as f32 * 4.0, 140.0, "批语"))
            .collect();
        let out = layout_notes(&inputs, BW, BH, FS, LH);
        assert!(!out.is_empty());
        for i in 0..out.len() {
            for j in i + 1..out.len() {
                let (a, b) = (&out[i], &out[j]);
                let disjoint = a.x + a.w <= b.x
                    || b.x + b.w <= a.x
                    || a.y + a.h <= b.y
                    || b.y + b.h <= a.y;
                assert!(disjoint, "第 {i} / {j} 条叠字了");
            }
            assert!(out[i].y + out[i].h <= BH + 0.01, "第 {i} 条探进版框");
        }
    }
}
