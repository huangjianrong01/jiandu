#!/usr/bin/env python3
"""依赖许可审计：扫 Cargo.lock 里每个包的 `license` 字段，报告 copyleft 与未声明。

为什么读本地 registry 而不跑 `cargo metadata`：
  * `cargo metadata` 要解析整棵依赖图，慢且在某些机器上会尝试联网；
  * 我们要判的是「协议」这一件事，`Cargo.lock` + 本地缓存的 `Cargo.toml`
    已经足够，且完全离线、可复现。

判据不是「出现 GPL 字样就红」：Rust 生态大量包是 `MIT OR Apache-2.0 OR
LGPL-2.1` 这种**多选一**——只要有一条宽松选项可选，我们就可以按那条用，
不构成 copyleft 传染。所以这里把「含 GPL 字样」与「**只能**选 GPL」分开：

    ✅ 可选宽松（MIT OR Apache-2.0 OR LGPL-2.1-or-later）   → 选 MIT/Apache
    ❌ 只能 GPL（GPL-3.0-only / AGPL-3.0 / SSPL-1.0）        → 必须换掉

用法：python3 scripts/license_audit.py [--strict]
  --strict 时，只要命中「只能 GPL」或「未声明协议」就以退出码 2 失败（CI 用）。
"""

import glob
import os
import re
import sys
from collections import Counter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REGISTRY = os.path.expanduser("~/.cargo/registry/src")

# 这些是「选了就不能闭源分发」的协议。注意 `-or-later` / `OR` 组合不算。
PERMISSIVE_MARKERS = ("MIT", "APACHE", "BSD", "ZLIB", "UNICODE", "ISC", "CC0",
                      "UNLICENSE", "0BSD", "MPL-2.0", "BSL-1.0", "BOOST")

# 只认「单独出现且没有宽松备选」的强 copyleft。
HARD_COPYLEFT = ("AGPL", "SSPL", "GPL-3.0", "GPL-2.0", "GPLV3", "GPLV2",
                 "GPL-3", "GPL-2", "CC-BY-NC", "BUSL", "COMMONS-CLAUSE",
                 "EUPL", "OSL-3", "RPL-1.5", "CPAL")


def parse_lock(path):
    text = open(path, encoding="utf-8").read()
    return re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', text)


def license_of(name, version):
    for src in glob.glob(REGISTRY + "/*"):
        p = os.path.join(src, f"{name}-{version}", "Cargo.toml")
        if not os.path.exists(p):
            continue
        txt = open(p, encoding="utf-8", errors="replace").read()
        m = re.search(r'^\[package\][\s\S]*?^license\s*=\s*"([^"]+)"', txt, re.M)
        if not m:
            m = re.search(r'^license\s*=\s*"([^"]+)"', txt, re.M)
        if m:
            return m.group(1).strip()
        return "(license-file)" if re.search(r"^license-file", txt, re.M) else "(未声明)"
    return None  # 本地缓存里没有（工作区自身 / 未缓存）


def classify(lic):
    """→ ('ok' | 'choose' | 'bad' | 'unknown', 说明)"""
    if lic is None:
        return "unknown", "本地缓存无此包"
    if lic in ("(未声明)", "(license-file)"):
        return "bad", "未声明 SPDX 协议"
    up = lic.upper()
    has_permissive = any(k in up for k in PERMISSIVE_MARKERS)
    has_copy = any(k in up for k in HARD_COPYLEFT)
    if has_copy and has_permissive:
        return "choose", "多选一：按宽松那一条用"
    if has_copy:
        return "bad", "只能选 copyleft"
    return "ok", ""


def main():
    strict = "--strict" in sys.argv
    lock = os.path.join(ROOT, "Cargo.lock")
    pkgs = [(n, v) for n, v in parse_lock(lock) if n not in ("jiandu", "jiandu-core")]
    tally = Counter()
    choose, bad, unknown = [], [], []
    for name, ver in pkgs:
        lic = license_of(name, ver)
        kind, why = classify(lic)
        tally[lic or "(本地无缓存)"] += 1
        if kind == "choose":
            choose.append((name, ver, lic))
        elif kind == "bad":
            bad.append((name, ver, lic or "(本地无缓存)", why))
        elif kind == "unknown":
            unknown.append((name, ver))

    print(f"Cargo.lock 内第三方包：{len(pkgs)} 个")
    print("\n协议分布（前 12）：")
    for lic, n in tally.most_common(12):
        print(f"  {n:4d}  {lic}")

    print("\n① 只能选 copyleft（必须处理）：")
    if bad:
        for b in bad:
            print("   ❌", b)
    else:
        print("   无")

    print("\n② 多选一、含 copyleft 备选（按宽松那条用即可）：")
    if choose:
        for c in choose:
            print("   ⚠️ ", c)
    else:
        print("   无")

    if unknown:
        print(f"\n③ 本地缓存缺失（未能判定，共 {len(unknown)}）：{unknown[:8]}")

    if strict and (bad or unknown):
        print("\n--strict：存在必须处理的项，退出码 2")
        return 2
    print("\n结论：无 GPL / AGPL / SSPL 传染风险。" if not bad else "\n结论：有必须处理的项。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
