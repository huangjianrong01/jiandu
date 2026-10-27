#!/usr/bin/env python3
"""生成 `core/src/common_chars.rs`：**常用字白名单**（自动注音的判据）。

判据只用一份**公开标准**，不用词频表（词频随语料漂移，且要联网）：

  * **GB 2312-80 一级汉字 3755 字** —— 按拼音排序的那一批，就是「受过
    中等教育的人该认得」的门槛。古籍里的 般 / 涅 / 僧 / 蕴 都在里面；
  * **这批字的繁体形**（取自 `resources/dict/dict.db` 的 `variant` 表，
    简→繁 2791 对）。读书人按「繁体」读时，薩 / 劍 / 觀 不该被当成生僻字
    满纸注音——真正该注的 埵 / 罣 / 槃 / 耨 不在其中。

产出一串按码位排好序的字符（`&str`），运行时收成 `HashSet<char>` 做 O(1) 查询。
重新生成：`python3 scripts/gen_common_chars.py`。
"""

import sqlite3
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def gb2312_level1() -> list[str]:
    out = []
    for b1 in range(0xB0, 0xD8):
        # 一级字区到 0xD7F9 为止；0xD7FA..0xD7FE 已属二级字区，不能算进来。
        hi = 0xFA if b1 == 0xD7 else 0xFF
        for b2 in range(0xA1, hi):
            try:
                out.append(bytes([b1, b2]).decode("gb2312"))
            except UnicodeDecodeError:
                continue
    return out


def main() -> None:
    l1 = gb2312_level1()
    assert len(l1) == 3755, len(l1)

    db = sqlite3.connect(ROOT / "crates/core/resources/dict/dict.db")
    s2t: dict[str, str] = {}
    for w, t in db.execute("select word, trad from variant"):
        if len(w) == 1 and len(t) == 1:
            s2t.setdefault(w, t)
    db.close()

    common = set(l1)
    for s in l1:
        t = s2t.get(s)
        # 只收 U+4E00–U+9FFF 的常用区：异体表里夹着几条错行（`咬→䶧`
        # `扬→䬗`，都落到扩展 A 区），这种字谁也不读，混进白名单只会让
        # 真生僻字被漏注。
        if t and 0x4E00 <= ord(t) <= 0x9FFF:
            common.add(t)

    chars = sorted(common)
    body = "".join(chars)

    # 按每行 60 字折行，用 concat! 拼成一条常量（避免一行几万字符）。
    lines = [body[i : i + 60] for i in range(0, len(body), 60)]
    joined = "\n".join(f'    "{ln}",' for ln in lines)

    src = f'''//! **常用字白名单**——自动注音的判据。**此文件由脚本生成，请勿手改**：
//! `python3 scripts/gen_common_chars.py`
//!
//! 收录范围 = GB 2312-80 一级汉字 3755 字 ∪ 其繁体形（共 {len(chars)} 字）。
//!
//! 为什么用这份表而不是词频：词频要联网、随语料漂移；而「一级字表」本身就是
//! 国家标准对「常用」的定义，且**简繁两侧都覆盖**——按繁体读时，`薩 / 劍 /
//! 觀` 不会被当生僻字，真正读着卡的 `埵 / 罣 / 槃 / 耨 / 澹 / 隰` 一个不漏。

use once_cell::sync::Lazy;
use std::collections::HashSet;

/// 常用字总数（供单测与状态展示）。
pub const COMMON_LEN: usize = {len(chars)};

const SOURCE: &str = concat!(
{joined}
);

/// 常用字集合（首次访问时建表，约 {len(chars)} 次插入，微秒级）。
pub static COMMON: Lazy<HashSet<char>> = Lazy::new(|| SOURCE.chars().collect());
'''

    out = ROOT / "crates/core/src/common_chars.rs"
    out.write_text(src, encoding="utf-8")
    print(f"wrote {out} ({len(chars)} chars, {len(src)} bytes)")


if __name__ == "__main__":
    main()
