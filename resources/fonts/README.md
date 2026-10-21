# 随包字体（可选）

默认**一个字体都不打包**——一部大字符集宋体动辄几十 MB，会把 App 体积撑爆。
程序启动时扫描本目录，把放进来的 `.ttf / .otf / .ttc` 全部注册（`src/fonts.rs`），
没放就沿回退链使用系统里最接近的字体，不会出现「豆腐块」。

## 四类角色，各司其职

| 角色 | 首选 | 备选 | 链尾兜底 |
| --- | --- | --- | --- |
| **正文**（大字符集宋体） | 文津宋体 | 梦源宋体 | 思源宋体 / 宋体 |
| **批注**（楷体） | 玄冬楷书 | 霞鹜文楷 | 系统楷体 |
| **标题 / UI**（黑体） | 思源黑体 | 系统默认黑体 | sans-serif |
| **等宽 / 西文** | 尚古等宽 · 西文数字（CM） | — | monospace / serif |

正文与批注分开选：正文要「生僻字全覆盖 + 竖排标点挤压正确」，批注要
「手写味、与正文一眼可分」——两件事的最优解不是同一个字体。

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
