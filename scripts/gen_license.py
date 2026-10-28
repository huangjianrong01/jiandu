#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""铸笺读的授权 key（离线、不绑机）。

与 `crates/core/src/license.rs` **必须严格同算法**：盐、字符表、FNV-1a、
三次雪崩搅动、5 比特切分，任何一处走偏，Rust 那边就不认。对表的活儿由
`license::tests::keys_minted_by_the_python_script_are_accepted` 那条单测
承担——改任一边，那条测试会红。

用法：
    python3 scripts/gen_license.py          # 铸一枚
    python3 scripts/gen_license.py -n 20    # 铸 20 枚（批量发给内测 / 买家）

输出形态 `XXXX-XXXX-XXXX-XXXX`，可直接贴进邮件 / 微信；用户粘贴时带空格、
小写、全角都不影响，App 会先归一化。
"""
from __future__ import annotations

import argparse
import secrets
import sys

# ── 与 Rust 侧一一对应 ────────────────────────────────────────────────────
SALT = b"JIANDU-LIC-1"
# 数字去 0 1，字母去 I O：手抄最容易混的四枚。32 个 = 每字符 5 比特。
ALPHABET = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ"
PAYLOAD_LEN = 12
CHECK_LEN = 4
M64 = (1 << 64) - 1


def digest(payload: str) -> int:
    """FNV-1a 64 + murmur3 finalizer 搅三遍（与 Rust `digest` 同式）。"""
    h = 0xCBF29CE484222325
    for b in SALT + b":" + payload.encode("ascii"):
        h ^= b
        h = (h * 0x00000100000001B3) & M64
    h ^= h >> 33
    h = (h * 0xFF51AFD7ED558CCD) & M64
    h ^= h >> 33
    h = (h * 0xC4CEB9FE1A85EC53) & M64
    h ^= h >> 33
    return h


def check_of(payload: str) -> str:
    x = digest(payload)
    out = []
    for _ in range(CHECK_LEN):
        out.append(ALPHABET[x & 0x1F])
        x >>= 5
    return "".join(out)


def mint() -> str:
    payload = "".join(secrets.choice(ALPHABET) for _ in range(PAYLOAD_LEN))
    return payload + check_of(payload)


def pretty(key: str) -> str:
    return "-".join(key[i : i + 4] for i in range(0, len(key), 4))


def valid(raw: str) -> bool:
    """与 Rust `valid` 同口径：铸完自检一遍，铸错总比发出去好。"""
    n = "".join(
        c
        for c in raw.upper().replace("-", "").replace("_", "").replace(" ", "")
        if c in ALPHABET
    )
    if len(n) != PAYLOAD_LEN + CHECK_LEN:
        return False
    return n[PAYLOAD_LEN:] == check_of(n[:PAYLOAD_LEN])


def main() -> int:
    ap = argparse.ArgumentParser(description="铸笺读授权 key（离线，不绑机）")
    ap.add_argument("-n", "--count", type=int, default=1, help="铸几枚（默认 1）")
    ap.add_argument("--raw", action="store_true", help="不带连字符（默认分组展示）")
    args = ap.parse_args()

    for _ in range(args.count):
        k = mint()
        assert valid(k), "自检不过，算法走偏了：%s" % k
        print(k if args.raw else pretty(k))
    return 0


if __name__ == "__main__":
    sys.exit(main())
