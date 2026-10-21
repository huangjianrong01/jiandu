# 随包字体（可选）

默认**一个字体都不打包**——一部大字符集宋体动辄几十 MB，会把 App 体积撑爆。
程序启动时扫描本目录，把放进来的 `.ttf / .otf / .ttc` 全部注册（`src/fonts.rs`），
没放就沿回退链使用系统里最接近的字体，不会出现「豆腐块」。

## 四类角色，各司其职

字体名**不写死在渲染代码里**：`src/fonts.rs` 的 `FONT_CONFIGS` 是唯一数据
源，正文 / 夹注 / 界面 / 西文各一条回退链，渲染只按角色取族名。

| 角色 | id | 首选 | 备选 | 链尾兜底 |
| --- | --- | --- | --- | --- |
| **正文**（大字符集宋体） | `main` | 文津宋体 | 梦源宋体 / 思源宋体 | 宋体 / serif |
| **夹注**（毛笔楷体） | `note` | 霞鹜文楷 | 玄冬楷书 | 系统楷体 / serif |
| **界面**（无衬线） | `ui` | 思源黑体 | PingFang SC | sans-serif |
| **西文 / 数字** | `latin` | Computer Modern（CMU） | Latin Modern / 尚古等宽 | serif |

正文与批注分开选：正文要「生僻字全覆盖 + 竖排标点挤压正确」，批注要
「手写味、与正文一眼可分」——两件事的最优解不是同一个字体。

设置面板的「排版字体」一行是**组合开关**：

- **刻本宋体** = 正文文津宋体 + 夹注霞鹜文楷（默认，匠体正文配手写小注）
- **手写楷体** = 正文霞鹜文楷 + 夹注玄冬楷书（写本味道）

切换时正文与夹注一起换、行高缩放一起换（宋体 1.05、楷体 1.12），
分页随之重算——所以切换不是「换个名字」，是整本书重排一遍。

## 行高缩放（lineHeightScale）

思源系列（Noto CJK）的 em box 偏大，同样字号下竖排上下两字容易贴住。
`fonts::line_height_scale` 按字体给一个系数，`compute_grid` 里
`row_h = 列宽 × 1.1 × 系数`：宋体 1.05、楷体 1.12、黑体 1.02、西文 1.02。
字形因此紧凑而不重叠；系数被夹在 0.85–1.6，异常值不会把行气弄散。

## 往本目录丢这些文件名即可被识别

```
WenJinMinchoP0-Regular.ttf    # 文津宋体（P0 平面，大字符集，推荐）
WenJinMincho-Regular.ttf
MengYuanSerif-Regular.ttf     # 梦源宋体
XuandongKaishu-Regular.ttf    # 玄冬楷书
LXGWWenKai-Regular.ttf        # 霞鹜文楷
LXGWWenKaiGB-Regular.ttf      # 霞鹜文楷 GB 版
SourceHanSansSC-Regular.otf   # 思源黑体
ShangguMono-Regular.ttf       # 尚古等宽
SourceHanSerifSC-Regular.otf  # 思源宋体（兜底）
```

其它文件名也能用——目录里所有字体文件都会被扫描注册，上面只是白名单。

## 获取地址（均为 SIL Open Font License 等自由许可）

- 文津宋体 <https://github.com/maqianos/WenJinMincho>（大字符集、大陆字形、竖排标点）
- 霞鹜文楷 <https://github.com/lxgw/LxgwWenKai>
- 玄冬楷书 —— 基于马善政毛笔楷书，含竖排特性
- 思源宋体 / 思源黑体 <https://github.com/adobe-fonts/source-han-serif> ·
  <https://github.com/adobe-fonts/source-han-sans>
- 尚古等宽 —— 思源系列衍生等宽

西文数字（Computer Modern）通常已在 TeX 发行版里，装一份 MacTeX / TeX Live
即可在链上命中；也可以直接放 `LatinModernMath-Regular.otf` 进来。
