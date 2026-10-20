#!/bin/bash
# 双击即可打开「笺读」。若可执行文件不存在则先编译。
cd "$(dirname "$0")"
BIN=/tmp/jiandu_target/debug/jiandu
if [ ! -x "$BIN" ]; then
  echo "首次运行，正在编译（需要几分钟）..."
  CARGO_TARGET_DIR=/tmp/jiandu_target cargo build --bin jiandu || {
    echo "编译失败，请查看上面的报错。"; read -r -p "按回车关闭..."; exit 1; }
fi
"$BIN" >/tmp/jiandu_run.log 2>&1 &
echo "笺读已启动。日志：/tmp/jiandu_run.log"
sleep 1
