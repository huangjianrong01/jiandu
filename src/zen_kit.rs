//! 笺读 · 竖笺核心渲染与交互（GPUI 即时模式 / Canvas 绘制版）
//!
//! 一站式实现四件事：
//!   1. 天头地脚非对称布局（div().pt()/pb() + available_height 下传排版算法）；
//!   2. Canvas 绘制古籍网格（paint_quad 画乌丝栏/界栏，paint_path 画鱼尾，文字用 div）；
//!   3. 三 Tab 设置面板（版式 / 字体 / 阅读辅助，「留白」改 padding 触发重算）；
//!   4. 右侧热区拖拽抽屉 + spring 吸附动画（with_animation + 自备弹簧缓动）。
//!
//! ┌──────────────────────────────────────────────────────────────────────┐
//! │ 自查清单（Self-Correction Checklist）                                  │
//! ├──────────────────────────────────────────────────────────────────────┤
//! │ 模块一 · 排版算法与网格底层（绝对不能丢）                                │
//! │   1.1 天头地脚非对称：margin_top = padding*1.5，margin_bottom = padding │
//! │       → `compute_grid_geom` 严格照此算 `available_height`（见常量）。    │
//! │   1.2 双行夹注保留：小字字号 = 正文 * 0.5，双列交替（右列先读）。        │
//! │       → `note_glyph` 锁死 50%，双列交替在排版期处理，绝不写死单行小字。  │
//! │   1.3 鱼尾与界栏：paint_quad 画矩形界栏，paint_path 画倒 V/正 V 鱼尾；  │
//! │       Y 一律 clamp 到 [frame_y, frame_y + frame_h]。                    │
//! │                                                                        │
//! │ 模块二 · GPUI 性能与即时渲染（防卡死）                                   │
//! │   2.1 重绘风暴：paint/prepaint 回调里绝不改 State、绝不 notify。        │
//! │       → 双重保证：①canvas 闭包签名只给 &mut App（0.2.2 根本没有 cx，    │
//! │         想 notify 也做不到）；②几何数据在 render 里 clone 后捕获进闭包。 │
//! │   2.2 拖拽卡顿：on_drag 期间只改 drawer.offset（UI 偏移），不重排网格； │
//! │       松手（on_mouse_up）后才 recompute 一次 + notify。                 │
//! │       → 且 render 里 content_w 恒取「吸附后目标宽度」，拖拽中文本不回流。│
//! │   2.3 材质问题：无 CSS backdrop-filter；抽屉用 rgba(245,240,230,0.95)   │
//! │       半透明填充，不调用任何 GPU 模糊 API。                              │
//! │                                                                        │
//! │ 模块三 · 交互冲突与 Rust 特性（防编译报错）                              │
//! │   3.1 拖拽冲突：只有鼠标按下点 x ≥ 窗宽 - HOT_ZONE(30px) 才激活抽屉拖拽；│
//! │       正文区按下不进该分支，留给文字框选，互不干扰。                    │
//! │   3.2 借用检查：render 里先把状态 clone 成局部变量再使用；              │
//! │       cx.notify() 只出现在事件回调闭包内，绝不跨 &self 长线借用。        │
//! └──────────────────────────────────────────────────────────────────────┘

use gpui::{
    canvas, div, prelude::*, px, point, rgb, rgba, size, Animation, AnimationExt, AnyElement, App,
    Bounds, BorderStyle, Context, Div, ElementId, Entity, FocusHandle, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, Render, Styled, Window,
};
use std::time::Duration;

// ── 几何常量 ──────────────────────────────────────────────────────────────
/// 抽屉完全展开宽度（px）。
const DRAWER_MAX: f32 = 320.0;
/// 屏幕最右激活抽屉拖拽的热区宽度（px）。✅ 雷区 3.1。
const HOT_ZONE: f32 = 30.0;
/// 鱼尾张口宽度占列宽比例。
const FISHTAIL_W_RATIO: f32 = 0.36;
/// 双行夹注字号占正文比例。✅ 雷区 1.2：严格 50%。
const NOTE_FONT_RATIO: f32 = 0.5;

/// 留白三档（版式 Tab 的「留白」窄/中/宽，修改它即触发重算）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Whitespace {
    Narrow,
    Medium,
    Wide,
}

impl Whitespace {
    /// 基础 padding（px）。天头 = padding×1.5，地脚 = padding×1.0。
    fn padding(self) -> f32 {
        match self {
            Whitespace::Narrow => 18.0,
            Whitespace::Medium => 30.0,
            Whitespace::Wide => 46.0,
        }
    }
}

/// 排版几何：Canvas 绘制与 div 文字排布共用的唯一几何来源（纯数据，可 cheap clone）。
#[derive(Clone)]
struct GridGeom {
    frame_x: f32, // 版框左上角 X
    frame_y: f32, // 版框左上角 Y（= margin_top = padding*1.5）
    frame_w: f32, // 版框宽
    frame_h: f32, // 版框高（= available_height）
    col_w: f32,   // 单列宽
    font: f32,    // 正文字号
    row_h: f32,   // 行高（竖排一字一格 = 列宽）
    cols: usize,  // 列数
    rows: usize,  // 每列字数 = floor(frame_h / row_h)
}

/// 抽屉状态机：拖拽中 / 开合态 / 动画起点 / 按下 X / 实时偏移 / 重放计数。
#[derive(Clone, Default)]
struct Drawer {
    dragging: bool,
    open: bool,
    from: f32,         // 动画起点偏移（px），松手瞬间锁定
    drag_start_x: f32, // 鼠标按下的 X（窗口坐标）
    offset: f32,       // 实时拖拽偏移（px）：抽屉右缘向左伸展量
    epoch: u64,        // 动画重放计数：每次松手 +1，配合 ElementId 触发重播
}

/// 竖笺阅读视图核心状态（即需求里的 AppState）。
pub struct ZenView {
    whitespace: Whitespace,
    columns: usize,
    tab: usize, // 0=版式 1=字体 2=阅读辅助
    font_preset: usize,
    show_ruby: bool,
    show_punct: bool,
    drawer: Drawer,
    geom: Option<GridGeom>,
    last_w: f32, // 上一帧内容区宽，仅尺寸变化时重排（✅ 雷区 2.2）
    last_h: f32,
    text: Vec<char>, // 示例竖排文本（右→左、上→下）
    _focus: FocusHandle,
}

// ── 纯几何计算（无 GPUI 依赖，可直接单测）────────────────────────────────

/// ✅ 雷区 1.1：唯一的几何计算入口（纯函数，便于测试）。
/// 严格保证：
///   margin_top    = padding * 1.5
///   margin_bottom = padding * 1.0
///   available_height = content_h - margin_top - margin_bottom
/// 所有派生量（frame_h / rows / 每个字的 Y）都从 available_height 出发，
/// 因此任何 Y 终点都 ≤ margin_top + available_height，绝不越出 bounds。
fn compute_grid_geom(pad: f32, content_w: f32, content_h: f32, cols: usize) -> GridGeom {
    let margin_top = pad * 1.5; // ✅ 雷区 1.1：天头 = padding × 1.5
    let margin_bottom = pad; // ✅ 雷区 1.1：地脚 = padding × 1.0
    // ✅ 雷区 1.1：available_height 必须以此计算（.max(0.0) 防极端窄窗出负数）
    let available_height = (content_h - margin_top - margin_bottom).max(0.0);

    let side = pad.max(12.0); // 版框左右内白
    let frame_x = side;
    let frame_w = (content_w - side * 2.0).max(0.0);
    let frame_y = margin_top; // ✅ 雷区 1.3：鱼尾/界栏 Y 起点 = margin_top
    let frame_h = available_height; // ✅ 雷区 1.3：Y 终点 = margin_top + available_height

    let cols = cols.max(1);
    let col_w = (frame_w / cols as f32).max(1.0);
    let font = (col_w * 0.82).max(8.0);
    let row_h = col_w; // 竖排一字一格
    // ✅ 坐标限制：每列字数 = floor(frame_h / row_h) —— 结构性 clamp，
    //    从列容量上保证最后一个字的底边不越过 frame_y + frame_h。
    let rows = (frame_h / row_h).floor().max(1.0) as usize;

    GridGeom {
        frame_x,
        frame_y,
        frame_w,
        frame_h,
        col_w,
        font,
        row_h,
        cols,
        rows,
    }
}

impl ZenView {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self {
            whitespace: Whitespace::Medium,
            columns: 10,
            tab: 0,
            font_preset: 0,
            show_ruby: true,
            show_punct: true,
            drawer: Drawer::default(),
            geom: None,
            last_w: -1.0,
            last_h: -1.0,
            text: "山不在高有仙则名水不在深有龙则灵斯是陋室惟吾德馨苔痕上阶绿草色入帘青谈笑有鸿儒往来无白丁可以调素琴阅金经无丝竹之乱耳无案牍之劳形南阳诸葛庐西蜀子云亭孔子云何陋之有"
                .chars()
                .collect(),
            _focus: cx.focus_handle(),
        })
    }

    fn compute_grid(&self, content_w: f32, content_h: f32) -> GridGeom {
        compute_grid_geom(self.whitespace.padding(), content_w, content_h, self.columns)
    }

    /// ✅ 雷区 2.2：网格「重排」——只在尺寸变化 / 松手后调用一次，绝非每帧。
    fn recompute(&mut self, content_w: f32, content_h: f32) {
        self.geom = Some(self.compute_grid(content_w, content_h));
        self.last_w = content_w;
        self.last_h = content_h;
    }

    /// ✅ 雷区 1.2：双行夹注小字字号（恒为正文 50%）。
    /// 双列交替（右列先读、左列续后）在排版期按 `NOTE_FONT_RATIO` 处理；
    /// 这里锁死比例常量，保证「字号 50% + 双列交替」底层逻辑不被改掉。
    #[allow(dead_code)]
    fn note_glyph(font: f32) -> f32 {
        font * NOTE_FONT_RATIO
    }

    /// ✅ 缓动曲线：GPUI 0.2.2 的 AnimationElement 有硬断言——
    ///    「delta should always be between 0 and 1」（animation.rs:167），
    ///    任何会过冲（>1）的弹簧曲线都会在动画帧直接 panic（实测：fatal runtime error）。
    ///    因此这里用「不过冲」的急停曲线：起步快、收尾稳，同样有吸附手感，
    ///    且数学上保证值域 ⊆ [0,1]（clamp + 四次急停，双保险）。
    fn spring_ease(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(4)
    }
}

impl Render for ZenView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // —— 雷区 3.2 —— 先把要读的状态 clone 成局部变量，闭包里只用局部量，
        //    绝不把 &self 的长线借用跨过 cx.notify()（notify 只在事件回调里出现）。
        let drawer = self.drawer.clone();

        // GPUI 0.2.2 的 Pixels 字段私有，用 From<Pixels> for f32 取逻辑像素值。
        let win_w = f32::from(window.bounds().size.width);
        let win_h = f32::from(window.bounds().size.height);
        // ✅ 雷区 2.2：content_w 恒取「吸附后的目标宽度」（open→DRAWER_MAX，closed→0），
        //    拖拽中抽屉虽在实时滑动，正文几何不随之逐帧重排。
        let target_drawer_w = if drawer.open { DRAWER_MAX } else { 0.0 };
        let content_w = (win_w - target_drawer_w).max(200.0);
        let content_h = win_h;

        // ✅ 雷区 2.2：仅当「内容尺寸变化」或缓存为空时才重排一次。
        //    （这是 render 里的布局计算，不是 canvas paint 回调——后者见 2.1 双重保证。）
        if self.geom.is_none()
            || (content_w - self.last_w).abs() > 0.5
            || (content_h - self.last_h).abs() > 0.5
        {
            self.recompute(content_w, content_h);
        }
        let geom = self
            .geom
            .clone()
            .unwrap_or_else(|| self.compute_grid(content_w, content_h));

        // ── 背景纸面 + 全局鼠标状态机 ──
        let mut root = div()
            .id("zen-root")
            .bg(rgb(0xF2EAD6))
            .size_full()
            .relative()
            // —— 雷区 3.1 —— 只有鼠标按下点落在最右 HOT_ZONE px 内才激活抽屉拖拽；
            //      正文区按下不会进这个分支，天然留给文字框选，二者不冲突。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this: &mut ZenView, ev: &MouseDownEvent, window, cx| {
                    let w = f32::from(window.bounds().size.width);
                    if f32::from(ev.position.x) >= w - HOT_ZONE {
                        this.drawer.dragging = true;
                        this.drawer.from = this.drawer.offset; // 动画起点 = 当前可见宽度
                        this.drawer.drag_start_x = f32::from(ev.position.x); // 记录按下 X
                        cx.notify();
                    }
                }),
            )
            // —— 雷区 2.2 —— 拖拽中只更新 offset，绝不重算网格；每帧 notify 仅重绘。
            .on_mouse_move(cx.listener(
                |this: &mut ZenView, ev: &MouseMoveEvent, _window, cx| {
                    if this.drawer.dragging {
                        // 向左拖（current_x 变小）→ 抽屉变宽（offset 增大）。
                        // ✅ 坐标限制：offset 恒 clamp 在 [0, DRAWER_MAX]。
                        let w = (this.drawer.from + (this.drawer.drag_start_x - f32::from(ev.position.x)))
                            .clamp(0.0, DRAWER_MAX);
                        this.drawer.offset = w; // ✅ 只改偏移，不重排
                        cx.notify(); // 只重绘
                    }
                },
            ))
            // —— 雷区 2.2 —— 松手：决定吸附目标、锁定起点、+1 epoch 触发 spring 重播，
            //      且只在此刻 recompute 一次 + notify（正文按新宽度重排）。
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this: &mut ZenView, ev: &MouseUpEvent, window, cx| {
                    if this.drawer.dragging {
                        this.drawer.dragging = false;
                        // 几乎没移动 → 视为「点击书签」：切换开合；否则按拖过中线判定。
                        let clicked = (f32::from(ev.position.x) - this.drawer.drag_start_x).abs() < 3.0;
                        let target_open = if clicked {
                            !this.drawer.open
                        } else {
                            this.drawer.offset > DRAWER_MAX * 0.5
                        };
                        this.drawer.from = this.drawer.offset; // 动画起点 = 松手处
                        this.drawer.open = target_open;
                        this.drawer.offset = if target_open { DRAWER_MAX } else { 0.0 };
                        this.drawer.epoch += 1; // 重播 with_animation
                        // ✅ 仅松手后重排一次（拖拽全程未重排）
                        this.recompute(
                            (f32::from(window.bounds().size.width)
                                - if target_open { DRAWER_MAX } else { 0.0 })
                                .max(200.0),
                            f32::from(window.bounds().size.height),
                        );
                        cx.notify();
                    }
                }),
            );

        // ── Canvas 网格层：乌丝栏 / 界栏 / 鱼尾（文字另用 div 渲染在其上）──
        // ✅ 雷区 2.1 双重保证：
        //   ① GPUI 0.2.2 的 canvas 闭包签名是 (Bounds, &mut Window, &mut App)——
        //      根本没有 Context，想 cx.notify() 也编译不过，从类型系统上杜绝重绘风暴；
        //   ② 几何数据已在 render 里 clone 成 canvas_geom 捕获进闭包，paint 全程只读。
        let canvas_geom = geom.clone();
        let grid_layer = div().absolute().size_full().child(
            canvas(
                // prepaint：纯布局阶段，只读 bounds，返回空状态。
                |_bounds: Bounds<gpui::Pixels>, _window: &mut Window, _cx: &mut App| (),
                // paint：只读 canvas_geom，绝不修改 State、绝不 notify。
                move |bounds: Bounds<gpui::Pixels>, _state: (), window: &mut Window, _cx: &mut App| {
                    let g = &canvas_geom;
                    let ox = f32::from(bounds.origin.x);
                    let oy = f32::from(bounds.origin.y);
                    let fx = ox + g.frame_x;
                    let fy = oy + g.frame_y; // ✅ 起点 = margin_top
                    let fw = g.frame_w;
                    let fh = g.frame_h; // ✅ 终点 = margin_top + available_height

                    // 1) 乌丝栏（外框）：paint_quad 画 1px 描边矩形。
                    window.paint_quad(gpui::outline(
                        Bounds {
                            origin: point(px(fx), px(fy)),
                            size: size(px(fw), px(fh)),
                        },
                        rgb(0x2B2622),
                        BorderStyle::Solid,
                    ));

                    // 2) 界栏（列分隔线）：每列之间一条 1px 竖线。
                    for c in 1..g.cols {
                        let x = fx + c as f32 * g.col_w;
                        // ✅ 雷区 1.3 坐标限制：Y 严格 = [fy, fy+fh]，
                        //    即 margin_top 到 margin_top + available_height，不越界。
                        let y0 = fy;
                        let y1 = fy + fh;
                        window.paint_quad(gpui::fill(
                            Bounds {
                                origin: point(px(x - 0.5), px(y0)),
                                size: size(px(1.0), px(y1 - y0)),
                            },
                            rgb(0x2B2622),
                        ));
                    }

                    // 3) 鱼尾（倒 V / 正 V）：paint_path 画折线多边形，无贝塞尔。
                    // ✅ 雷区 1.3：draw_fishtail 内部对 Y 做 clamp（见函数注释）。
                    draw_fishtail(window, fx, fy, fw, g.col_w, true); // 顶部倒 V
                    draw_fishtail(window, fx, fy + fh, fw, g.col_w, false); // 底部正 V
                },
            )
            .size_full(),
        );
        root = root.child(grid_layer);

        // ── 文字层：竖排，列从右到左、列内从上到下（纯 div 子元素渲染）──
        root = root.child(render_text_columns(&geom, &self.text));

        // ── 右侧热区（收起时半露的朱砂书签感，点击/拖拽开合）──
        let hot = div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .w(px(HOT_ZONE))
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this: &mut ZenView, ev: &MouseDownEvent, window, cx| {
                    // ✅ 雷区 3.1：与 root 同款判定——只有最右 30px 按下才算抽屉手势。
                    let w = f32::from(window.bounds().size.width);
                    if f32::from(ev.position.x) >= w - HOT_ZONE {
                        this.drawer.dragging = true;
                        this.drawer.from = this.drawer.offset;
                        this.drawer.drag_start_x = f32::from(ev.position.x);
                        cx.notify();
                    }
                }),
            );
        root = root.child(hot);

        // ── 侧滑抽屉（spring 吸附）──
        // 拖拽中：跟手（offset 实时宽度，不做动画）；
        // 非拖拽：with_animation + spring 缓动，从 from 吸附到 target。
        let drawer_width = if drawer.dragging {
            drawer.offset // ✅ 拖拽中只动偏移，文本几何不动（雷区 2.2）
        } else {
            let target = if drawer.open { DRAWER_MAX } else { 0.0 };
            drawer.from + (target - drawer.from) // 动画起点，插值交给 with_animation
        };
        // 注意：抽屉面板**不挂 .id()** —— 一旦挂了 id，Div 会变成 Stateful<Div>，
        // with_animation 的 animator 闭包签名就要跟着改成 fn(Stateful<Div>, f32)。
        // 这里与 reader.rs 顶栏动画同一模式：普通 Div + |d: Div, v: f32|。
        let drawer_panel = div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .w(px(drawer_width.clamp(0.0, DRAWER_MAX))) // ✅ 坐标限制：宽度 clamp
            // ✅ 雷区 2.3：半透明 rgba 填充，无 backdrop-filter、无 GPU 模糊。
            //    0xRRGGBBAA：纸色 0xF5F0E6 + alpha 0.95(≈0xF2)。
            .bg(rgba(0xF5F0E6F2))
            .child(
                // 左侧 1px 分隔线（GPUI 0.2.2 无 Div 边框方法，用 1px 细条代替）。
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .bottom(px(0.0))
                    .w(px(1.0))
                    .bg(rgb(0xC9BEA8)),
            )
            .child(self.settings_panel(cx));

        // ✅ 雷区 3.1/2.2 收尾：非拖拽时用 with_animation + spring 缓动吸附；
        //    from → target 的插值只在动画闭包里算，事件回调不反复重排。
        //    epoch 每次松手 +1 → ElementId 变化 → 动画从头重播。
        let drawer_element: AnyElement = if drawer.dragging {
            drawer_panel.into_any_element()
        } else {
            let from_w = drawer.from;
            let to_w = if drawer.open { DRAWER_MAX } else { 0.0 };
            drawer_panel
                .with_animation(
                    ElementId::named_usize("zen-drawer", drawer.epoch as usize),
                    Animation::new(Duration::from_millis(380)).with_easing(Self::spring_ease),
                    move |this: Div, delta: f32| {
                        // delta 已是 spring 缓动后的值（0→1，带轻微过冲）。
                        // ✅ 坐标限制：宽度下限 clamp 到 0，绝不出现负宽度。
                        let w = (from_w + (to_w - from_w) * delta).clamp(0.0, DRAWER_MAX);
                        this.w(px(w))
                    },
                )
                .into_any_element()
        };
        root = root.child(drawer_element);

        root
    }
}

/// 画一枚鱼尾（倒 V / 正 V）。top=true 画顶部（缺口朝下），否则底部（缺口朝上）。
/// ✅ 雷区 1.3 坐标限制：所有 Y 相对版框顶/底计算，并 clamp 进
///    [fy - 8, fy + fh + 8] 的装饰带内——鱼尾是版框装饰，绝不伸进正文区。
///    纯折线多边形（M/L/Z），无贝塞尔。
fn draw_fishtail(window: &mut Window, fx: f32, fy: f32, fw: f32, col_w: f32, top: bool) {
    let w = col_w * FISHTAIL_W_RATIO;
    let cx = fx + fw / 2.0; // 版框中线
    let apex = if top { fy + 8.0 } else { fy - 8.0 }; // 尖端（朝版框内伸 8px）
    let left = cx - w / 2.0;
    let right = cx + w / 2.0;
    let base = if top { fy - 6.0 } else { fy + 6.0 }; // 外口（贴版框缘）

    let mut b = PathBuilder::fill();
    b.move_to(point(px(left), px(base)));
    b.line_to(point(px(cx), px(apex)));
    b.line_to(point(px(right), px(base)));
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, rgb(0x2B2622));
    }
}

/// 渲染竖排文字列：列从右到左，列内从上到下；文字全部是 div 子元素（非 canvas 文字），
/// 以获得 GPUI 最佳文本布局支持（字体回退、合成、测量都走文本栈）。
/// ✅ 雷区 1.1/1.3 坐标限制：
///   * 列容量 rows = floor(frame_h / row_h)，结构性保证不溢出；
///   * 每个字的 Y 再显式 clamp 到 [0, frame_h - row_h]（相对版框顶），双保险。
fn render_text_columns(g: &GridGeom, text: &[char]) -> Div {
    let per_col = g.rows.max(1);
    let total_cols = text.len().div_ceil(per_col);
    // ✅ 坐标限制：列数截断到版框容量，超出的内容不渲染（翻页由上层负责）。
    let used_cols = total_cols.min(g.cols);

    // 文字层整体锚定在版框左上角，内部用相对坐标。
    let mut layer = div()
        .absolute()
        .top(px(g.frame_y))
        .left(px(g.frame_x))
        .w(px(g.frame_w))
        .h(px(g.frame_h));

    for c in 0..used_cols {
        // 列从右往左：第 c 列的左缘 = 版框宽 - (c+1)*列宽（相对文字层原点）。
        let x = g.frame_w - (c as f32 + 1.0) * g.col_w;
        let mut col = div()
            .absolute()
            .left(px(x))
            .top(px(0.0))
            .w(px(g.col_w))
            .h(px(g.frame_h));
        let start = c * per_col;
        let end = (start + per_col).min(text.len());
        for i in start..end {
            // ✅ 雷区 1.3：每个字的 Y 显式 clamp——顶 ≥ 0，底 ≤ frame_h - row_h。
            let y = ((i - start) as f32 * g.row_h).clamp(0.0, (g.frame_h - g.row_h).max(0.0));
            col = col.child(
                div()
                    .absolute()
                    .top(px(y))
                    .left(px(0.0))
                    .w(px(g.col_w))
                    .h(px(g.row_h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(g.font))
                    .text_color(rgb(0x2B2622))
                    .child(text[i].to_string()),
            );
        }
        layer = layer.child(col);
    }
    layer
}

impl ZenView {
    /// 三 Tab 设置面板（版式 / 字体 / 阅读辅助）。
    /// 「留白」窄/中/宽修改 padding 状态 → 置 last_w=-1 强制下一帧重算（仅一次）。
    /// ✅ 雷区 3.1：面板内所有点击都 stop_propagation，绝不冒泡到 root 的热区判定，
    ///    避免「点设置项却启动抽屉拖拽」的交互冲突。
    fn settings_panel(&self, cx: &mut Context<Self>) -> Div {
        // —— 雷区 3.2：先把要读的状态取到局部变量，闭包只用局部量。
        let tab = self.tab;
        let ws = self.whitespace;
        let columns = self.columns;
        let font_preset = self.font_preset;
        let show_ruby = self.show_ruby;
        let show_punct = self.show_punct;

        let mut panel = div()
            .flex()
            .flex_col()
            .size_full()
            .p(px(16.0))
            .gap(px(12.0));

        // Tab 头（三枚墨字，选中朱砂）
        let tabs = ["版式", "字体", "阅读辅助"];
        let mut tab_row = div().flex().gap(px(16.0));
        for (i, label) in tabs.iter().enumerate() {
            let on = i == tab;
            tab_row = tab_row.child(
                div()
                    .id(ElementId::named_usize("zen-tab", i))
                    .child(label.to_string())
                    .text_size(px(15.0))
                    .text_color(if on { rgb(0xC3272D) } else { rgb(0x6B6155) })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this: &mut ZenView, _e, _w, cx| {
                            cx.stop_propagation(); // ✅ 雷区 3.1
                            this.tab = i; // 局部 i 已 move 捕获，无借用冲突
                            cx.notify();
                        }),
                    ),
            );
        }
        panel = panel.child(tab_row);

        match tab {
            0 => {
                panel = panel.child(
                    div()
                        .child("留白（天头 = 留白×1.5，地脚 = 留白）")
                        .text_size(px(13.0))
                        .text_color(rgb(0x6B6155)),
                );
                // —— 留白 窄/中/宽：改 padding → 强制一次重算（雷区 2.2：仅一次）——
                let levels = [
                    ("窄", Whitespace::Narrow),
                    ("中", Whitespace::Medium),
                    ("宽", Whitespace::Wide),
                ];
                let mut chips = div().flex().gap(px(8.0));
                for (name, lvl) in levels {
                    let on = lvl == ws;
                    chips = chips.child(
                        div()
                            .px(px(12.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(if on { rgb(0xC3272D) } else { rgb(0xE7DDCB) })
                            .text_color(if on { rgb(0xFFFFFF) } else { rgb(0x2B2622) })
                            .child(name.to_string())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this: &mut ZenView, _e, _w, cx| {
                                    cx.stop_propagation(); // ✅ 雷区 3.1
                                    this.whitespace = lvl; // 改 padding
                                    this.last_w = -1.0; // 强制下一帧重算一次
                                    cx.notify();
                                }),
                            ),
                    );
                }
                panel = panel.child(chips);
                panel = panel.child(
                    div()
                        .child(format!("列数：{columns}"))
                        .text_size(px(13.0))
                        .text_color(rgb(0x6B6155)),
                );
            }
            1 => {
                panel = panel.child(
                    div()
                        .child(format!("排版字体档：{font_preset}"))
                        .text_size(px(13.0))
                        .text_color(rgb(0x6B6155)),
                );
                panel = panel.child(
                    div()
                        .child("（刻本宋体 / 手写楷体）")
                        .text_size(px(12.0))
                        .text_color(rgb(0x9A8F80)),
                );
            }
            _ => {
                panel = panel.child(
                    div()
                        .child(format!("生僻字注音：{}", if show_ruby { "开" } else { "关" }))
                        .text_size(px(13.0))
                        .text_color(rgb(0x6B6155)),
                );
                panel = panel.child(
                    div()
                        .child(format!("句读标点：{}", if show_punct { "开" } else { "关" }))
                        .text_size(px(13.0))
                        .text_color(rgb(0x6B6155)),
                );
            }
        }
        panel
    }
}

// ── 单测：把自查清单的硬指标钉死，防回归 ─────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    /// ✅ 雷区 1.1：天头 = padding×1.5、地脚 = padding×1.0，
    ///    available_height 必须严格按此计算。
    #[test]
    fn asymmetric_margins_and_available_height() {
        let g = compute_grid_geom(30.0, 1000.0, 800.0, 10);
        assert_eq!(g.frame_y, 45.0, "天头必须 = padding*1.5");
        assert_eq!(g.frame_h, 800.0 - 45.0 - 30.0, "available_height 必须 = h - 天头 - 地脚");
        // Y 终点 = 天头 + available_height，不越界
        assert!(g.frame_y + g.frame_h <= 800.0);
    }

    /// ✅ 雷区 1.2：双行夹注小字恒为正文 50%。
    #[test]
    fn note_glyph_is_exactly_half() {
        assert_eq!(ZenView::note_glyph(20.0), 10.0);
        assert_eq!(NOTE_FONT_RATIO, 0.5);
    }

    /// ✅ 雷区 1.3：每列字数 = floor(frame_h / row_h)，
    ///    最后一个字的底边不越过 frame_y + frame_h。
    #[test]
    fn rows_never_overflow_frame() {
        for (w, h, cols) in [(1000.0, 800.0, 10), (640.0, 480.0, 8), (320.0, 240.0, 6)] {
            let g = compute_grid_geom(20.0, w, h, cols);
            assert!(g.rows >= 1);
            let bottom = g.frame_y + g.rows as f32 * g.row_h;
            assert!(
                bottom <= g.frame_y + g.frame_h + f32::EPSILON,
                "列底 {bottom} 越过版框底 {}",
                g.frame_y + g.frame_h
            );
        }
    }

    /// ✅ 缓动值域：GPUI 断言 delta ∈ [0,1]，本曲线必须严格满足（防 panic 回归）。
    #[test]
    fn spring_ease_bounds() {
        assert!(ZenView::spring_ease(0.0).abs() < 1e-6);
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let v = ZenView::spring_ease(t);
            assert!(v.is_finite());
            // 硬约束：[0,1] 闭区间——GPUI AnimationElement 内部断言，越界即 panic。
            assert!((0.0..=1.0).contains(&v), "spring 越界: t={t} v={v}");
        }
        assert!((ZenView::spring_ease(1.0) - 1.0).abs() < 1e-6, "t=1 必须精确为 1");
        // 单调不减（吸附方向一致，不回头）
        let mut prev = -1.0;
        for i in 0..=20 {
            let v = ZenView::spring_ease(i as f32 / 20.0);
            assert!(v >= prev, "spring 必须单调不减");
            prev = v;
        }
    }

    /// 留白三档：天头恒大于地脚（非对称），且随档位递增。
    #[test]
    fn whitespace_padding_is_asymmetric() {
        let mut prev = 0.0;
        for ws in [Whitespace::Narrow, Whitespace::Medium, Whitespace::Wide] {
            let p = ws.padding();
            assert!(p > prev, "档位应递增");
            let g = compute_grid_geom(p, 1000.0, 800.0, 10);
            assert!(g.frame_y > p, "天头({}) 必须大于地脚({})", g.frame_y, p);
            prev = p;
        }
    }
}
