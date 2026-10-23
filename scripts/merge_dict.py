#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
把《康熙字典》(kangxi.db) 与《說文解字》(数千个碎 JSON) **合并成一本**
离线字书 `resources/dict/dict.db`，供笺读编译期 `include_bytes!` 嵌入。

为什么必须合并、而不是把几千个碎文件塞进 .app：
  * 碎文件 9833 个，逐个打包 / 逐个读取既不体面也慢；
  * SQLite 单文件 + 主键索引，查询是 O(log n) 的一次 seek；
  * `include_bytes!` 只吃得下一个文件。

产出表结构（在原 kangxi.db 的 `dict` 表上**新增字段**，不动原字段）：
    dict(id, word, content, word1, word2, volume,
         shuowen,       -- 《說文解字》原文（新增）
         pinyin,        -- 拼音（新增，帶調：yī）
         radical,       -- 部首（新增）
         fanqie)        -- 反切（新增：於悉切）
    alias(word, head)   -- 繁简兜底：简体字 -> 正条字头

用法：
    python3 scripts/merge_dict.py \
        --kangxi ~/Desktop/词典/kangxi.db \
        --shuowen ~/Desktop/词典/shuowen-master/data \
        --out resources/dict/dict.db
"""

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile

# ----------------------------------------------------------------------
# 繁简兜底表：macOS ICU 的 Han 转换（zhconv 的离线替代）
# ----------------------------------------------------------------------
#
# crates.io 上的 `zhconv` 不在本地缓存里、构建机又断网，拉不进来。
# 但 macOS 自带 ICU，`NSString.applyingTransform("Hant-Hans")` 就是同一套
# 汉字简繁映射——这里在**构建期**把它跑一遍，只把「字书里真正出现的字」
# 转成对照表落库，运行时零依赖、零网络。
#
# 只跑一次、结果进 db，所以运行时不调 swift、不调 ICU。

ICU_SWIFT = r"""
import Foundation
let path = CommandLine.arguments[1]
guard let raw = try? String(contentsOfFile: path, encoding: .utf8) else { exit(1) }
let chars = raw.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
var out: [String] = []
for c in chars {
    if c.isEmpty { out.append(""); continue }
    let t = (c as NSString).applyingTransform(StringTransform("Hant-Hans"), reverse: false)
    out.append(t ?? c)
}
// 用 \u{1} 作行分隔：汉字里绝不会出现，避免与内容混淆
print(out.joined(separator: "\u{1}"))
"""


def build_variant_map(chars):
    """用 macOS ICU 把「传统字形」批量转成「简化字形」，返回 {繁: 简}。

    只收**单字**且转换后仍是单字、且确实变了的条目；没装 swift 就返回空表
    （字典照常能查，只是简体输入兜底少一条路）。
    """
    chars = sorted({c for c in chars if len(c) == 1})
    if not chars or shutil.which("swift") is None:
        return {}
    fd, src = tempfile.mkstemp(suffix=".swift")
    os.close(fd)
    fd2, inp = tempfile.mkstemp(suffix=".txt")
    os.close(fd2)
    try:
        with open(src, "w", encoding="utf-8") as f:
            f.write(ICU_SWIFT)
        with open(inp, "w", encoding="utf-8") as f:
            f.write("\n".join(chars))
        r = subprocess.run(
            ["swift", src, inp], capture_output=True, text=True, timeout=300
        )
        if r.returncode != 0:
            print("  ! swift 转换失败，跳过繁简兜底表", file=sys.stderr)
            return {}
        got = r.stdout.rstrip("\n").split("\x01")
        if len(got) != len(chars):
            print("  ! 转换结果长度不符，跳过繁简兜底表", file=sys.stderr)
            return {}
        m = {}
        for a, b in zip(chars, got):
            if len(b) == 1 and b != a:
                m[a] = b
        return m
    finally:
        for p in (src, inp):
            try:
                os.unlink(p)
            except OSError:
                pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kangxi", required=True)
    ap.add_argument("--shuowen", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    os.makedirs(os.path.dirname(a.out) or ".", exist_ok=True)
    if os.path.exists(a.out):
        os.unlink(a.out)
    shutil.copyfile(a.kangxi, a.out)

    db = sqlite3.connect(a.out)
    db.execute("PRAGMA journal_mode=OFF")
    db.execute("PRAGMA synchronous=OFF")

    # ---- 新增字段（幂等：已存在就跳过） --------------------------------
    have = {r[1] for r in db.execute("PRAGMA table_info(dict)")}
    for col, ddl in (
        ("shuowen", "shuowen text"),
        ("pinyin", "pinyin text"),
        ("radical", "radical text"),
        ("fanqie", "fanqie text"),
    ):
        if col not in have:
            db.execute(f"ALTER TABLE dict ADD COLUMN {ddl}")

    # ---- 读《說文解字》碎 JSON ----------------------------------------
    sw = {}          # 字头 -> 说文条目
    files = sorted(
        (f for f in os.listdir(a.shuowen) if f.endswith(".json")),
        key=lambda s: int(s[:-5]) if s[:-5].isdigit() else 0,
    )
    for name in files:
        try:
            with open(os.path.join(a.shuowen, name), encoding="utf-8") as f:
                d = json.load(f)
        except Exception:
            continue
        head = (d.get("wordhead") or "").strip()
        if not head:
            continue
        item = {
            "shuowen": (d.get("explanation") or "").strip(),
            "pinyin": (d.get("pinyin_full") or "").strip(),
            "radical": (d.get("radical") or "").strip(),
            "fanqie": (d.get("pronunciation") or "").strip(),
        }
        sw[head] = item
        # 异体字（古文 / 籀文）也指回同一条，正文里遇到能查到。
        for v in d.get("variants") or []:
            vh = (v.get("wordhead") or "").strip()
            if vh and vh not in sw:
                sub = dict(item)
                sub["shuowen"] = (v.get("explanation") or item["shuowen"]).strip()
                sw[vh] = sub
    print(f"  說文解字：{len(files)} 个碎文件 -> {len(sw)} 个字头")

    # ---- 合并进 dict ---------------------------------------------------
    rows = {}
    for rid, word, word1 in db.execute("SELECT id, word, word1 FROM dict"):
        for w in (word, word1):
            if w:
                rows.setdefault(w, []).append(rid)

    n_upd = 0
    new_rows = []
    do_upd = []
    for head, it in sw.items():
        ids = rows.get(head)
        if ids:
            for rid in ids:
                do_upd.append(
                    (it["shuowen"], it["pinyin"], it["radical"], it["fanqie"], rid)
                )
            n_upd += len(ids)
        else:
            # 康熙未收、說文独有（多为小篆隶定字）：立一条只带說文的新记录。
            new_rows.append((head, it["shuowen"], it["pinyin"], it["radical"], it["fanqie"]))
    db.executemany(
        "UPDATE dict SET shuowen=?, pinyin=?, radical=?, fanqie=? WHERE id=?",
        do_upd,
    )
    db.executemany(
        "INSERT INTO dict(word, shuowen, pinyin, radical, fanqie) VALUES(?,?,?,?,?)",
        new_rows,
    )
    print(f"  并入康熙：{n_upd} 行；說文独有新增：{len(new_rows)} 行")

    # ---- 繁简兜底 ------------------------------------------------------
    heads = [w for w in rows.keys() if len(w) == 1] + [
        h for h in sw.keys() if len(h) == 1
    ]
    t2s = build_variant_map(heads)
    db.execute("DROP TABLE IF EXISTS alias")
    db.execute("CREATE TABLE alias(word text, head text)")
    # 繁 -> 繁 本身也立一条，查询时一次 SQL 就能 union 上，不必分两次查。
    pairs = set()
    for h in set(heads):
        pairs.add((h, h))
        s = t2s.get(h)
        if s:
            pairs.add((s, h))
    db.executemany("INSERT INTO alias VALUES(?,?)", sorted(pairs))
    # `variant`：简体 -> **首选繁体字头**。康熙常把「学」与「學」各立一条，
    # 两条都有释义；点简体字查词时应落在繁体那条（古籍字头才是正体），
    # 故单独存一张偏好表，查询时让它排在最前。
    db.execute("DROP TABLE IF EXISTS variant")
    db.execute("CREATE TABLE variant(word text, trad text)")
    known = {w for w in rows.keys()} | set(sw.keys())
    variant = sorted((s, h) for (h, s) in t2s.items() if h in known)
    db.executemany("INSERT INTO variant VALUES(?,?)", variant)
    print(f"  繁简兜底：{len(t2s)} 组对照 -> {len(pairs)} 条索引 / {len(variant)} 条正体偏好")

    db.execute("DROP INDEX IF EXISTS IDX_Dict")
    db.execute("CREATE INDEX IF NOT EXISTS idx_dict_word ON dict(word)")
    db.execute("CREATE INDEX IF NOT EXISTS idx_alias_word ON alias(word)")
    db.commit()
    db.execute("VACUUM")
    db.commit()
    n = db.execute("SELECT count(*) FROM dict").fetchone()[0]
    db.close()

    print(f"\n  合并完成：{a.out}")
    print(f"  总条目 {n} 条，{os.path.getsize(a.out)/1048576:.2f} MB")


if __name__ == "__main__":
    main()
