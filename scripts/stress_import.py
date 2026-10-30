#!/usr/bin/env python3
"""极限压测 runner：跑真实导入漏斗，采样内存峰值 RSS。

macOS 的 `/usr/bin/time` 是 BSD 版，不支持 GNU 的 `-l`（拿不到峰值内存），
所以这里用 `ps` 轮询采样。采样间隔 10ms，足够抓住峰值。

用法：
    cargo build -p jiandu-core --release --example stress_import
    python3 scripts/stress_import.py <大文件> [bin 路径]
"""
import subprocess
import sys
import time


def main() -> int:
    if len(sys.argv) < 2:
        print("用法：stress_import.py <大文件> [bin 路径]")
        return 2
    path = sys.argv[1]
    bin_path = sys.argv[2] if len(sys.argv) > 2 else "target/release/examples/stress_import"

    t0 = time.time()
    proc = subprocess.Popen([bin_path, path])
    proc.wait()
    wall = time.time() - t0

    # 峰值内存走 getrusage(RUSAGE_CHILDREN)：内核记账的**真实峰值**，比 ps
    # 轮询采样准得多（采样会漏掉两次采样之间的尖峰）。
    # ⚠️ 单位不统一：macOS 是字节，Linux 是 KB。
    import resource
    import sys as _sys
    ru = resource.getrusage(resource.RUSAGE_CHILDREN)
    per_mb = 1024.0 * 1024.0 if _sys.platform == "darwin" else 1024.0
    peak_mb = ru.ru_maxrss / per_mb

    print()
    print("══ 内存峰值 RSS: %.1f MB（内核记账 getrusage，非采样）" % peak_mb)
    print("══ 墙钟总耗时:   %.2f s" % wall)
    print("══ 退出码:       %d" % proc.returncode)
    return proc.returncode


if __name__ == "__main__":
    raise SystemExit(main())
