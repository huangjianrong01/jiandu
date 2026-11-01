# 随包第三方组件与许可（NOTICE）

本文件随 `.app` 一起分发（`笺读.app/Contents/Resources/LICENSES/`）。
凡是**随包**进入用户机器的第三方内容都列在这里；仅按名称调用、不随包的系统
字体与系统框架不在其列（见 `docs/版权合规清单.md`）。

---

## 1. 齐伋体 QIJI —— `crates/app/assets/fonts/qiji.ttf`

```
Copyright (c) 2020, Lingdong Huang i@lingdong.works, with Reserved Font Name QIJI.
This Font Software is licensed under the SIL Open Font License, Version 1.1.
Full text: see OFL.txt in this directory.
Source: https://github.com/LingDong-/qiji-font
```

* 许可：**SIL Open Font License 1.1**（全文见同目录 `OFL.txt`）。
* 使用方式：**原样随包分发，未做任何修改**（未改字形、未改表、未改名）。
  OFL 第 3 条的「保留字体名 QIJI」因此不成问题——我们没有做衍生字体。
* 义务履行：OFL 第 2 条要求「每一份都带着上述版权声明与本许可」。故
  `qiji.ttf` 旁放 `OFL.txt`，打包时整体拷入 `Contents/Resources/LICENSES/`；
  「关于」页亦有一行署名（见 `reader.rs::render_about_sheet`）。
* 该字体基于思源宋体（Source Han Serif，OFL）改造，作者已按 OFL 要求同协议发布。

---

## 2. 《說文解字》数据 —— `crates/core/resources/dict/dict.db` 的 `shuowen` 字段

```
Copyright 2019 Shuowen.org
Licensed under the Apache License, Version 2.0
http://www.apache.org/licenses/LICENSE-2.0
Source: https://github.com/shuowenjiezi/shuowen
```

* 《說文解字》**正文**本身是公有领域（东汉许慎撰，公元 100 年成书）；
  受许可约束的是 shuowen.org 的**数字化整理成果**（标点、小篆字形、勘误）。
* 我们的改动（Apache-2.0 第 4(b) 条要求声明）：把该仓库中数千个碎 JSON
  合并进一张 SQLite 表 `dict`，与《康熙字典》同表存放，并新增 `pinyin` /
  `radical` / `fanqie` 三列。**文字内容未改写**，仅做格式归并。
* 该数据在编译期经 `include_bytes!` 嵌入二进制（`crates/core/src/dict.rs`）。

---

## 3. 《康熙字典》文本 —— `dict.db` 的 `content` 字段

清·张玉书等奉敕撰，成书 1716 年，**公有领域**。本次仅做格式归并（同上）。

---

## 4. Rust 依赖

构建图内 499 个第三方 crate，协议全部为 MIT / Apache-2.0 / BSD / Zlib /
Unicode-3.0 一类的宽松许可，**无 GPL / AGPL / SSPL**。逐包清单见
`docs/版权合规清单.md` 第 3 节；发布前用
`python3 scripts/license_audit.py` 可复现该结论。
