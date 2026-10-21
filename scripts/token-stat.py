#!/usr/bin/env python3
"""笺读 · 会话 token 消耗统计（精确值，非估算）。

WorkBuddy 的会话日志（.jsonl）里，每次模型请求都会带一条
`providerData.rawUsage`：

    rawUsage: { prompt_tokens, completion_tokens, total_tokens,
                completion_tokens_details: { reasoning_tokens, ... },
                prompt_tokens_details: { cached_tokens, ... } }

本脚本把所有含 rawUsage 的记录累加，得出这个项目从第一行代码到现在
**真实消耗**的输入 / 输出 / 缓存 / 思考 token，并顺带统计请求次数与工具调用次数。

用法：
    python3 scripts/token-stat.py [jsonl ...]
不给参数就扫 ~/.workbuddy/projects 下所有提到「笺读 / jiandu」的会话日志。
"""

from __future__ import annotations

import json
import os
import sys
from collections import Counter

KEYS = [
    "prompt_tokens",
    "completion_tokens",
    "total_tokens",
    "cached_tokens",
    "reasoning_tokens",
]


def usage_of(pd: dict) -> dict:
    """从 providerData 里取出归一化的用量。"""
    u = pd.get("rawUsage") or pd.get("usage") or {}
    if not u:
        return {}
    prompt_details = u.get("prompt_tokens_details") or {}
    completion_details = u.get("completion_tokens_details") or {}
    return {
        "prompt_tokens": u.get("prompt_tokens", 0),
        "completion_tokens": u.get("completion_tokens", 0),
        "total_tokens": u.get("total_tokens", 0)
        or u.get("prompt_tokens", 0) + u.get("completion_tokens", 0),
        "cached_tokens": prompt_details.get("cached_tokens", 0),
        "reasoning_tokens": completion_details.get("reasoning_tokens", 0),
    }


def scan(path: str):
    tot = Counter()
    requests = 0
    tool_calls = 0
    tool_results = 0
    user_msgs = 0
    assistant_msgs = 0
    models = Counter()
    with open(path, "r", encoding="utf-8", errors="ignore") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                m = json.loads(line)
            except json.JSONDecodeError:
                continue
            role = m.get("role")
            if role == "user":
                user_msgs += 1
            elif role == "assistant":
                assistant_msgs += 1
            pd = m.get("providerData")
            if not isinstance(pd, dict):
                continue
            if pd.get("model"):
                models[pd["model"]] += 1
            if pd.get("argumentsDisplayText") is not None or pd.get("toolResult") is not None:
                if pd.get("argumentsDisplayText") is not None:
                    tool_calls += 1
                if pd.get("toolResult") is not None:
                    tool_results += 1
            u = usage_of(pd)
            if u:
                requests += 1
                for k in KEYS:
                    tot[k] += u.get(k, 0)
    stat = {k: tot[k] for k in KEYS}
    stat.update(
        {
            "requests": requests,
            "tool_calls": tool_calls,
            "tool_results": tool_results,
            "user_msgs": user_msgs,
            "assistant_msgs": assistant_msgs,
            "models": models,
        }
    )
    return stat


def show(tag: str, s: dict) -> None:
    print(f"== {tag}")
    print(f"   模型请求 {s['requests']} 次 / 工具调用 {s['tool_calls']} 次 / 用户发言 {s['user_msgs']} 条")
    for k in KEYS:
        print(f"   {k:20s} {s[k]:>14,d}")
    if s["models"]:
        print(f"   模型分布：{dict(s['models'])}")


def jiandu_logs() -> list:
    root = os.path.expanduser("~/.workbuddy/projects")
    hits = []
    for dirpath, _dirs, files in os.walk(root):
        for fn in files:
            if not fn.endswith(".jsonl"):
                continue
            p = os.path.join(dirpath, fn)
            try:
                with open(p, "r", encoding="utf-8", errors="ignore") as f:
                    head = f.read(400_000)
            except OSError:
                continue
            if "笺读" in head or "jiandu" in head:
                hits.append(p)
    return hits


def main() -> None:
    args = sys.argv[1:] or jiandu_logs()
    total = Counter()
    for p in sorted(args):
        s = scan(p)
        if s["requests"] == 0:
            continue
        show(p.replace(os.path.expanduser("~"), "~"), s)
        for k in KEYS:
            total[k] += s[k]
        for k in ("requests", "tool_calls", "tool_results", "user_msgs", "assistant_msgs"):
            total[k] += s[k]
    print("\n== 合计（笺读项目全部会话）")
    for k in KEYS:
        print(f"   {k:20s} {total[k]:>14,d}")
    print(f"   模型请求 {total['requests']} 次 / 工具调用 {total['tool_calls']} 次")
    inp = total["prompt_tokens"]
    out = total["completion_tokens"]
    print(f"\n   输入 ≈ {inp:,d} token（其中缓存命中 {total['cached_tokens']:,d}）")
    print(f"   输出 ≈ {out:,d} token（其中思考 {total['reasoning_tokens']:,d}）")
    print(f"   合计 ≈ {total['total_tokens']:,d} token")


if __name__ == "__main__":
    main()
