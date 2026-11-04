#!/usr/bin/env python3
"""从外部 CJK 字表生成内核基线 `crates/core/src/data/variant_cjk.txt`。

外部表格式（chinese-philosophy 的 cjk/*.txt）：每行**恰好三个汉字**
    异体 繁体 简体
绝大多数行三字相同，那只是「此字存在」的收录清单，对映射无用，直接丢弃。

用法：
    python3 scripts/gen_variant_table.py /path/to/cjk/cjk.txt
"""
import sys, pathlib

def main():
    if len(sys.argv) < 2:
        print(__doc__); return 1
    src = pathlib.Path(sys.argv[1])
    out = pathlib.Path(__file__).resolve().parent.parent / "crates/core/src/data/variant_cjk.txt"
    triples, bad = [], 0
    for raw in src.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line:
            continue
        if len(line) != 3:
            bad += 1
            continue
        a, b, c = line
        if a == b == c:      # 纯收录行，无映射信息
            continue
        triples.append((a, b, c))
    head = [
        "# 繁简 / 异体三元组基线（由 scripts/gen_variant_table.py 生成，勿手改）",
        "# 每行三字：异体 繁体 简体。三列相同者已在生成时剔除。",
        f"# 条目 {len(triples)}",
        "",
    ]
    out.write_text("\n".join(head) + "\n".join("".join(t) for t in triples) + "\n", encoding="utf-8")
    print(f"写入 {out}  条目 {len(triples)}  丢弃异常行 {bad}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
