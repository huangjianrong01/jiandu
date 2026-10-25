#!/usr/bin/env python3
"""mdx2concise.py — 把 MDict（.mdx）词书转成笺读「简明」页签的 JSON 数据。

背景
----
笺读的字书浮签有三个页签：簡明 / 康熙 / 說文。前两个走 SQLite（康熙、
說文由 `scripts/merge_dict.py` 产出 `resources/dict/dict.db`），而「簡明」
原先是一张手写小表（core/src/dict.rs 的 `CONCISE`，几十个高频字）。

本脚本把任意 .mdx（MDict 词书）转成**同一份 JSON 格式**（记录数组）：

    [[字, 释义], [字, 释义], ...]

再由 `core/src/dict.rs` 用 `include_str!` 在编译期整体嵌入，替换手写小表。
GPUI 不渲染 HTML，所以释义里的标签全部剥掉（见 `strip_html`）。

用法
----
    python3 scripts/mdx2concise.py 输入.mdx [-o resources/dict/concise.json]
        [--keep-multi]     保留复音词词头（默认只收单字）
        [--max-len 90]     释义最长字数（超出截断）
        [--limit N]        最多收多少条（调试用）
        [--follow-links]   跟随 @@@LINK= 重定向取目标词条
        [--stats-only]     只统计不写文件

依赖：`mdict-utils`（pip3 install mdict-utils，自带解密/解压，无需 python-lzo）。

示例
----
    python3 scripts/mdx2concise.py \
        "~/Desktop/古漢語常用字字典4（王力版）2019-10-1.mdx" \
        -o resources/dict/concise.json --max-len 90
"""

from __future__ import annotations

import argparse
import html
import json
import re
import sys
from html.parser import HTMLParser
from pathlib import Path

# ----------------------------------------------------------------------
# HTML 清洗：GPUI 只吃纯文本。块级标签转成换行，其余标签连同类名一起丢掉。
# ----------------------------------------------------------------------

_BLOCK_TAGS = {"br", "p", "div", "li", "tr", "td", "h1", "h2", "h3", "h4", "h5", "h6"}
_DROP_TAGS = {"script", "style", "img", "audio", "video", "object", "embed"}


class _TextOnly(HTMLParser):
    """收集可见文本：块级元素边界产出换行，实体自动反转。"""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self._skip_depth = 0

    def handle_starttag(self, tag: str, attrs) -> None:
        if tag in _DROP_TAGS:
            self._skip_depth += 1
        elif tag in _BLOCK_TAGS and self._skip_depth == 0:
            self.parts.append("\n")

    def handle_endtag(self, tag: str) -> None:
        if tag in _DROP_TAGS and self._skip_depth > 0:
            self._skip_depth -= 1
        elif tag in _BLOCK_TAGS and self._skip_depth == 0:
            self.parts.append("\n")

    def handle_data(self, data: str) -> None:
        if self._skip_depth == 0:
            self.parts.append(data)

    def text(self) -> str:
        return "".join(self.parts)


def strip_html(raw: bytes | str) -> str:
    if isinstance(raw, bytes):
        raw = raw.decode("utf-8", "replace")
    p = _TextOnly()
    try:
        p.feed(raw)
        p.close()
    except Exception:
        raw = re.sub(r"<[^>]+>", "\n", raw)
        p = None
    t = p.text() if p else raw
    t = html.unescape(t)
    # 折叠空白：每段内多空格合一；空行压成一个换行。
    lines = [re.sub(r"[ \t\u3000]+", " ", ln).strip() for ln in t.split("\n")]
    return "\n".join(ln for ln in lines if ln)


# ----------------------------------------------------------------------
# 取「第一条义」：简明页签只要一行概括。
# ----------------------------------------------------------------------

# 义项编号：㈠㈡㈢ / ❶❷❸ / ①②③
_NUM = r"[㈠㈡㈢㈣㈤❶❷❸❹❺❻❼❽❾❿①②③④⑤⑥⑦⑧⑨⑩]"
_NUM_HEAD = re.compile(rf"^\s*{_NUM}\s*")
# 行尾拼音（含声调拉丁字母）与（异体字）括号——「字头 + 拼音」行要整行剥掉。
_PINYIN_TAIL = re.compile(r"[a-zA-Z\u00c0-\u024f]+\s*$")
_PAREN_GROUP = re.compile(r"[（(][^）)]*[）)]")
# 释义行内括号里的拼音：「万俟（qí）」→「万俟」。
_PAREN_PINYIN = re.compile(r"[（(][a-zA-Z\u00c0-\u024f\s]+[）)]")
# 以书名号起的行多半是例句 / 提示（《…》），不是义本身。
_QUOTE = re.compile(r"^《.*》$")
# 互见条目：「見1頁」。
_SEE_OTHER = re.compile(r"^見\s*[\d一二三四五六七八九十]+\s*頁")
# 索引条目（繁简词头索引）：「[部·筆] 一部0畫一」——没有释义，跳过。
_INDEX_META = re.compile(r"^\[部[·・.]?筆\]")


def _bare(l: str) -> str:
    """剥掉行尾拼音与（异体）括号，剩下的候选字头。"""
    l = _PINYIN_TAIL.sub("", l)
    l = _PAREN_GROUP.sub("", l)
    return l.strip(" \u3000")


def first_sense(key: str, text: str) -> str:
    """从整条释义里取「第一义」的一行概括。

    王力《古汉语常用字字典》的版式：首行「字 拼音」，随后每条义以
    ❶ 起头（如「❶數詞。」「❶到……去。」），再跟《書名》引文与例句。
    引文行一律跳过；义项行砍到第一个句读；太短（如「介詞」）则并到
    第二个句读，免得只丢一个词类标签。
    """
    lines = [ln.strip() for ln in text.split("\n") if ln.strip()]
    i = 0
    while i < len(lines) and _bare(lines[i]) == key:
        i += 1  # 跳过「字 + 拼音（+ 异体）」行
    for ln in lines[i:]:
        if _SEE_OTHER.match(ln) or _QUOTE.match(ln):
            continue
        parts = [p.strip() for p in re.split(r"[。；;]", ln) if p.strip()]
        if not parts:
            continue
        seg = parts[0]
        if len(_NUM_HEAD.sub("", seg)) < 6 and len(parts) >= 2:
            # 第一刀太短：「介詞」→「介詞，用法相當於『於』」。
            seg = parts[0] + "，" + parts[1]
        seg = _NUM_HEAD.sub("", seg)
        seg = _PAREN_PINYIN.sub("", seg)
        return seg.strip("，,、 　")
    return ""


def is_single_cjk(key: str) -> bool:
    return len(key) == 1 and "\u4e00" <= key <= "\u9fff"


# 词头数字后缀：「人0」「人1」是这部字典的繁简索引式词头（正文在前、
# 索引字在后），归一化时剥掉尾部数字，否则单字过滤会漏掉整批常用字。
_KEY_SUFFIX = re.compile(r"\d+$")


# ----------------------------------------------------------------------
# 主流程
# ----------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser(description="MDX → 笺读简明词典 JSON")
    ap.add_argument("mdx", type=Path, help="输入 .mdx 文件")
    ap.add_argument("-o", "--out", type=Path,
                    default=Path("resources/dict/concise.json"), help="输出 JSON")
    ap.add_argument("--keep-multi", action="store_true",
                    help="保留复音词词头（默认只收单字）")
    ap.add_argument("--max-len", type=int, default=90, help="释义最长字数")
    ap.add_argument("--limit", type=int, default=0, help="最多收多少条（0=不限）")
    ap.add_argument("--follow-links", action="store_true", help="跟随 @@@LINK= 重定向")
    ap.add_argument("--stats-only", action="store_true", help="只统计不写文件")
    args = ap.parse_args()

    try:
        from mdict_utils.reader import MDX
    except ImportError:
        print("需要 mdict-utils：pip3 install mdict-utils", file=sys.stderr)
        return 2

    print(f"打开 {args.mdx} …")
    m = MDX(str(args.mdx), encoding="utf-8")

    rows: list[list[str]] = []
    seen: dict[str, int] = {}
    n_total = n_link = n_skip = 0
    raw: dict[str, bytes] = {}  # 供 --follow-links 用

    def emit(key: str, val: bytes | str) -> None:
        nonlocal n_skip, n_link
        if not args.keep_multi and not is_single_cjk(key):
            return
        if isinstance(val, bytes):
            if val.startswith(b"@@@LINK="):
                n_link += 1
                return
            txt = strip_html(val)
        else:
            if val.startswith("@@@LINK="):
                n_link += 1
                return
            txt = strip_html(val)
        if _INDEX_META.match(txt.strip()):
            return  # 索引元信息行，不是释义
        sense = first_sense(key, txt)
        if not sense:
            n_skip += 1
            return
        if len(sense) > args.max_len:
            sense = sense[: args.max_len].rstrip("，,、 ") + "…"
        if key in seen:
            # 重复字头（索引条目人0/人1…归一化后都撞主字头）：
            # 先到先得——正文主条目排在索引前面，别让元信息覆盖释义。
            return
        seen[key] = len(rows)
        rows.append([key, sense])

    for k, v in m.items():
        n_total += 1
        if isinstance(k, bytes):
            k = k.decode("utf-8", "replace").strip()
        k = _KEY_SUFFIX.sub("", k.strip())
        if not k:
            continue
        if args.follow_links and isinstance(v, bytes) and v.startswith(b"@@@LINK="):
            raw.setdefault(k, v)  # 第二轮再解析
            continue
        emit(k, v)
        if args.limit and len(rows) >= args.limit:
            print(f"达到 --limit {args.limit}，提前收手")
            break

    print(f"词条总数 {n_total}，LINK 重定向 {n_link}，无义可抽 {n_skip}，"
          f"收录 {len(rows)} 条")

    if args.stats_only:
        for r in rows[:10]:
            print("  样例:", r[0], "→", r[1][:60])
        return 0

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(rows, f, ensure_ascii=False, separators=(",", ":"))
    size = args.out.stat().st_size
    print(f"写出 {args.out}（{len(rows)} 条，{size/1024:.0f} KB）")
    for r in rows[:8]:
        print("  样例:", r[0], "→", r[1][:60])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
