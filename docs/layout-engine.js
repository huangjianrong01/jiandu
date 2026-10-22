/*!
 * 笺读 JianDu · 响应式多版式排版引擎（参考实现，与 Rust `src/grid.rs` 同构）
 *
 * 三件事：
 *   1. calculateGrid(columns, mode) —— 一套算式，三种铺法；
 *   2. 监听屏幕尺寸自动切换布局（ResizeObserver + window.resize，带防抖）；
 *   3. 渲染循环里三种模式的核心差异（整页是「外层 transform 缩放」，
 *      不改内部网格比例）。
 *
 * 零依赖：浏览器 <script src> 与 Node require 都能用。
 */
(function (root) {
  'use strict';

  // ── 1. 常量（与 Rust 端逐项对齐）────────────────────────────────
  var GRID_FS_RATIO = 0.75; // fontSize = colWidth × 0.75
  var GRID_ROW_RATIO = 1.1; // rowHeight = colWidth × 1.1
  var GRID_FS_MIN = 12; // 最小字号阈值：14 列窄屏也不至于小如蚂蚁
  var GRID_FS_MAX = 220;

  // 整叶基准版式（一整叶 = 两叶 + 中缝），比例取刻本开本 ≈1.44:1
  var FULL_BASE_W = 1180;
  var FULL_BASE_H = 820;
  var FULL_SCALE_MIN = 0.25;
  var FULL_SCALE_MAX = 3.0;

  // 响应式判据
  var FACING_MIN_ASPECT = 1.15; // 宽高比低于此值 → 手机竖屏
  var FACING_MIN_LEAF_W = 300; // 双叶时单叶的最小可读宽
  var FACING_MIN_FS = 14; // 双叶时单叶字号下限，低于此值宁可单叶
  var RESP_MARGIN = 16;
  var RESP_GUTTER = 24;

  var LEAF_GAP_MIN = 22;
  var LEAF_GAP_MAX = 110;

  /** 三种渲染模式 */
  var LayoutMode = {
    Facing: 'facing', // 对开翻阅（电脑 / 平板横屏）
    Single: 'single', // 手机竖屏（单叶，贴右）
    Full: 'full', // 标准刻本整页（等比缩放看版式）
    ALL: ['facing', 'single', 'full'],
    label: function (m) {
      return { facing: '对开', single: '单叶', full: '整页' }[m] || m;
    },
    /** 整页铺的也是一整叶（两叶 + 中缝），故与对开同路 */
    spreads: function (m) {
      return m === 'facing' || m === 'full';
    },
  };

  function clamp(v, lo, hi) {
    return v < lo ? lo : v > hi ? hi : v;
  }

  /** 中缝（版心）宽：约等于一行字宽，列数越多缝越窄 */
  function gutterW(contentW, columns, edge, margin) {
    var c = Math.max(1, columns);
    var k = Math.max(0.1, 1 - 2 * clamp(margin, 0, 0.45));
    var num = Math.max(1, (contentW * k) / 2 - edge);
    return clamp(num / (c + k / 2), LEAF_GAP_MIN, LEAF_GAP_MAX);
  }

  /** 整页缩放系数：取「宽能塞下」「高能塞下」里更小的那个（object-fit: contain） */
  function fullScale(contentW, contentH, zoom) {
    var k = Math.min(contentW / FULL_BASE_W, contentH / FULL_BASE_H);
    return clamp(k * Math.max(0.1, zoom), FULL_SCALE_MIN, FULL_SCALE_MAX);
  }

  function isNarrow(vw, vh) {
    if (vw <= 1 || vh <= 1) return true;
    return vw / vh < FACING_MIN_ASPECT;
  }

  /** 响应式判定：该不该双叶对开（宽高比 + 单叶宽 + 字号三重把关） */
  function shouldFace(vw, vh, columns, zoom) {
    if (isNarrow(vw, vh)) return false;
    var contentW = Math.max(1, vw - 2 * RESP_MARGIN);
    var perLeaf = (contentW - RESP_GUTTER) / 2;
    if (perLeaf < FACING_MIN_LEAF_W) return false;
    var fs = (perLeaf / Math.max(1, columns)) * GRID_FS_RATIO * zoom;
    return fs >= FACING_MIN_FS;
  }

  // ── 2. 核心网格计算 ────────────────────────────────────────────
  /**
   * 底层网格算式（三模式共用）：
   *   colWidth  = 可用宽 / columns
   *   fontSize  = 0.75 × colWidth      （线性缩放）
   *   rowHeight = 1.1  × colWidth × lineScale
   *   totalRows = floor(可用高 / rowHeight)
   * 字号跌破 GRID_FS_MIN 时**反推列宽**（宁可纸面略宽、由外层滚动），
   * 绝不把字挤成一团。
   */
  function computeGrid(contentW, contentH, cfg, facing, zoom, edge, head, foot) {
    var columns = Math.max(1, cfg.columns || 8);
    var margin = cfg.margin || 0;
    var lineScale = cfg.lineScale || 1.0;

    var perLeaf = facing
      ? Math.max(60, (contentW - gutterW(contentW, columns, edge, margin)) / 2)
      : contentW;

    var mw = Math.max(0, perLeaf * margin);
    var mh = Math.max(0, contentH * margin);
    var innerW = Math.max(40, perLeaf - edge - 2 * mw);
    var innerH = Math.max(40, contentH - head - foot - 2 * mh);

    var colRaw = innerW / columns;
    var want = colRaw * GRID_FS_RATIO * zoom;
    var minHit = want < GRID_FS_MIN;
    var fs = clamp(want, GRID_FS_MIN, GRID_FS_MAX);
    var colW = minHit ? fs / GRID_FS_RATIO : colRaw;

    var rowH = colW * GRID_ROW_RATIO * clamp(lineScale, 0.85, 1.6);
    var rows = Math.max(1, Math.floor(innerH / rowH));

    return {
      columns: columns,
      rows: rows,
      colW: colW,
      rowH: rowH,
      fs: fs,
      gridW: columns * colW,
      gridH: rows * rowH,
      outerW: columns * colW + edge + 2 * mw,
      outerH: rows * rowH + head + foot + 2 * mh,
      minFontHit: minHit,
    };
  }

  /**
   * **统一入口**：calculateGrid(columns, mode)
   *
   * @param {object} inp
   * @param {number} inp.columns     列数：8 / 10 / 12 / 14
   * @param {string} inp.mode        LayoutMode.Facing | Single | Full
   * @param {number} inp.contentW    可用区宽（已扣工具栏 / 状态栏 / 留白）
   * @param {number} inp.contentH    可用区高
   * @param {number} [inp.zoom]      字级倍率（默认 1）
   * @param {number} [inp.edge]      版框双边预留（默认 50）
   * @param {number} [inp.head]      天头预留
   * @param {number} [inp.foot]      地脚预留
   * @param {number} [inp.margin]    四周留白比例（默认 0.04）
   * @param {number} [inp.lineScale] 行高缩放（随字体，默认 1）
   * @returns {{m:object, boxW:number, boxH:number, facing:boolean, scale:number}}
   */
  function calculateGrid(inp) {
    var columns = inp.columns || 8;
    var mode = inp.mode || LayoutMode.Facing;
    var contentW = inp.contentW || 0;
    var contentH = inp.contentH || 0;
    var zoom = inp.zoom == null ? 1 : inp.zoom;
    var edge = inp.edge == null ? 50 : inp.edge;
    var head = inp.head || 0;
    var foot = inp.foot || 0;
    var margin = inp.margin == null ? 0.04 : inp.margin;
    var lineScale = inp.lineScale == null ? 1 : inp.lineScale;

    var scale = mode === LayoutMode.Full ? fullScale(contentW, contentH, zoom) : 1;

    // 三种模式只在这里分派一次：内容区怎么给 + 铺几叶。
    var boxW, boxH, facing, z;
    if (mode === LayoutMode.Full) {
      // 整页：按基准版式等比缩放，内部网格比例恒定不变。
      boxW = FULL_BASE_W * scale;
      boxH = FULL_BASE_H * scale;
      facing = true;
      z = 1; // zoom 已并入 scale
    } else if (mode === LayoutMode.Single) {
      // 手机竖屏：整幅宽都给这一叶 → 字最大；中缝贴右 / 隐藏。
      boxW = contentW;
      boxH = contentH;
      facing = false;
      z = zoom;
    } else {
      // 对开：单叶可用宽对半（扣中缝）；中缝落在正中。
      boxW = contentW;
      boxH = contentH;
      facing = true;
      z = zoom;
    }

    var m = computeGrid(
      boxW,
      boxH,
      { columns: columns, margin: margin, lineScale: lineScale },
      facing,
      z,
      // 整页模式下边框 / 天头 / 地脚同乘系数，否则内部比例会随屏幕漂移。
      edge * scale,
      head * scale,
      foot * scale
    );

    return { m: m, boxW: boxW, boxH: boxH, facing: facing, scale: scale };
  }

  // ── 3. 监听屏幕尺寸，自动切换布局 ──────────────────────────────
  /**
   * 自动模式：只在对开 ⇄ 单叶之间来回（整页是手动档，「看整体版式」用，
   * 自动切进去会打断阅读）。
   */
  function autoMode(viewportW, viewportH, columns, zoom, manual) {
    if (manual === LayoutMode.Full) return LayoutMode.Full;
    return shouldFace(viewportW, viewportH, columns, zoom)
      ? LayoutMode.Facing
      : LayoutMode.Single;
  }

  /**
   * 挂上 resize 监听：尺寸一变就重算网格并回调（防抖，免得拖动窗口时
   * 每帧重排整本书）。返回取消监听的函数。
   *
   * @param {HTMLElement} el     观察的容器
   * @param {function} onChange  ({width, height}) => void
   * @param {number} [delay]     防抖毫秒（默认 120）
   */
  function onViewportResize(el, onChange, delay) {
    delay = delay == null ? 120 : delay;
    var timer = null;
    var last = { w: 0, h: 0 };

    function emit(w, h) {
      if (w === last.w && h === last.h) return; // 尺寸没真变（软键盘 / 工具栏收放）
      last = { w: w, h: h };
      onChange({ width: w, height: h });
    }

    function handler() {
      if (timer) clearTimeout(timer);
      timer = setTimeout(function () {
        var rect = el.getBoundingClientRect
          ? el.getBoundingClientRect()
          : { width: el.innerWidth, height: el.innerHeight };
        emit(Math.round(rect.width), Math.round(rect.height));
      }, delay);
    }

    if (typeof ResizeObserver === 'function') {
      var ro = new ResizeObserver(handler);
      ro.observe(el);
    }
    // 旋转屏幕 / 跨屏拖动时 ResizeObserver 有时不触发，故再挂一层。
    if (typeof window !== 'undefined' && window.addEventListener) {
      window.addEventListener('resize', handler);
      window.addEventListener('orientationchange', handler);
    }
    handler(); // 首帧先算一次

    return function off() {
      if (timer) clearTimeout(timer);
      if (typeof ResizeObserver === 'function' && ro) ro.disconnect();
      if (typeof window !== 'undefined' && window.removeEventListener) {
        window.removeEventListener('resize', handler);
        window.removeEventListener('orientationchange', handler);
      }
    };
  }

  // ── 4. 渲染循环：三种模式的核心差异 ────────────────────────────
  /**
   * @param {CanvasRenderingContext2D} ctx
   * @param {object} plan   calculateGrid 的返回值
   * @param {object} state  { mode, columns, gutter, chars }
   */
  function render(ctx, plan, state) {
    var m = plan.m;
    var mode = state.mode;
    var vw = state.viewportW;
    var vh = state.viewportH;

    ctx.clearRect(0, 0, vw, vh);

    if (mode === LayoutMode.Full) {
      // 整页：只对外层做 transform 缩放，内部按基准版式原样绘制。
      // —— 这就是「不改变网格比例，只缩放容器」的真身。
      var k = plan.scale;
      ctx.save();
      ctx.translate((vw - FULL_BASE_W * k) / 2, (vh - FULL_BASE_H * k) / 2);
      ctx.scale(k, k);
      drawSpread(ctx, plan, state, FULL_BASE_W, FULL_BASE_H);
      ctx.restore();
      return;
    }

    if (mode === LayoutMode.Facing) {
      // 对开：两半叶铺满屏幕，中缝落在正中（含鱼尾 / 叶码）。
      drawSpread(ctx, plan, state, vw, vh);
      return;
    }

    // 手机竖屏：只排一叶，纸面贴屏幕右缘（竖排自右读起），
    // 中缝不画——整幅宽全给正文，字号最大。
    var x = vw - m.outerW;
    var y = (vh - m.outerH) / 2;
    drawLeaf(ctx, plan, state, x, y, 'current');
  }

  /** 对开 / 整页：两叶 + 中缝 */
  function drawSpread(ctx, plan, state, boxW, boxH) {
    var m = plan.m;
    var gut = state.gutter || gutterW(boxW, m.columns, 50, 0.04);
    var totalW = m.outerW * 2 + gut;
    var left0 = (boxW - totalW) / 2;
    var top = (boxH - m.outerH) / 2;
    drawLeaf(ctx, plan, state, left0, top, 'prev');
    drawLeaf(ctx, plan, state, left0 + m.outerW + gut, top, 'current');
    drawGutter(ctx, left0 + m.outerW, top, gut, m.outerH, state);
  }

  /** 一叶：版框 + 界行 + 竖排字格（右起第一列在最右） */
  function drawLeaf(ctx, plan, state, x, y, which) {
    var m = plan.m;
    ctx.save();
    ctx.strokeStyle = state.frame || '#2F3535';
    ctx.lineWidth = 1;
    ctx.strokeRect(x + 0.5, y + 0.5, m.outerW, m.outerH);

    // 乌丝栏：列间细线
    ctx.strokeStyle = state.rule || 'rgba(47,53,53,0.18)';
    for (var c = 1; c < m.columns; c++) {
      var lx = x + m.outerW - c * m.colW;
      ctx.beginPath();
      ctx.moveTo(lx + 0.5, y + 8);
      ctx.lineTo(lx + 0.5, y + m.outerH - 8);
      ctx.stroke();
    }

    // 正文：竖排，自右向左、自上而下（示意用 state.chars）
    ctx.fillStyle = state.ink || '#2F3535';
    ctx.font = m.fs + 'px "Songti SC", serif';
    ctx.textBaseline = 'top';
    var chars = state.chars || '';
    var i = 0;
    for (var col = 0; col < m.columns && i < chars.length; col++) {
      var cx = x + m.outerW - (col + 1) * m.colW;
      for (var row = 0; row < m.rows && i < chars.length; row++) {
        ctx.fillText(chars[i++], cx + (m.colW - m.fs) / 2, y + 8 + row * m.rowH);
      }
    }
    ctx.restore();
  }

  /** 版心（中缝）：上下相对的鱼尾 + 叶码 */
  function drawGutter(ctx, x, y, w, h, state) {
    ctx.save();
    ctx.strokeStyle = state.rule || 'rgba(47,53,53,0.18)';
    ctx.beginPath();
    ctx.moveTo(x + w / 2, y + h * 0.12);
    ctx.lineTo(x + w / 2, y + h * 0.88);
    ctx.stroke();
    // 鱼尾：上下各一枚小三角
    ctx.fillStyle = state.fish || '#333';
    ctx.beginPath();
    ctx.moveTo(x + w / 2, y + h * 0.16);
    ctx.lineTo(x + w / 2 - 5, y + h * 0.16 - 6);
    ctx.lineTo(x + w / 2 + 5, y + h * 0.16 - 6);
    ctx.closePath();
    ctx.fill();
    ctx.beginPath();
    ctx.moveTo(x + w / 2, y + h * 0.84);
    ctx.lineTo(x + w / 2 - 5, y + h * 0.84 + 6);
    ctx.lineTo(x + w / 2 + 5, y + h * 0.84 + 6);
    ctx.closePath();
    ctx.fill();
    ctx.restore();
  }

  var api = {
    LayoutMode: LayoutMode,
    GRID_FS_RATIO: GRID_FS_RATIO,
    GRID_ROW_RATIO: GRID_ROW_RATIO,
    GRID_FS_MIN: GRID_FS_MIN,
    FULL_BASE_W: FULL_BASE_W,
    FULL_BASE_H: FULL_BASE_H,
    gutterW: gutterW,
    fullScale: fullScale,
    isNarrow: isNarrow,
    shouldFace: shouldFace,
    computeGrid: computeGrid,
    calculateGrid: calculateGrid,
    autoMode: autoMode,
    onViewportResize: onViewportResize,
    render: render,
  };

  root.JianDuLayout = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
