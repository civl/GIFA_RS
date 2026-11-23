#!/usr/bin/env python3
"""把一整段连拍帧抽样成"够标但不多"的子集。

为什么要抽样：识别管线是 10 fps，录 60 秒就是 600 帧。
人工标注 600 帧不现实，而统计上 60~80 帧已经足够定中位误差和分朝向偏差。

用法（在仓库根目录）：

    python scripts/corpus-subsample.py data/seq data/seq-sub 8

    # 第 1 个参数：连拍出来的 .bin 目录（--dump-frame 指的那个）
    # 第 2 个参数：抽样输出目录（会建）
    # 第 3 个参数：步长，8 表示每 8 帧留 1 帧

帧文件名保持原样（f001-158.bin），所以逐帧 CSV **不用改** ——
标注页按文件名里的 seq 去 CSV 里查元数据，查不到的行会被忽略。
"""

import argparse
import os
import re
import shutil
import sys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("frames", help="连拍的 .bin 目录")
    ap.add_argument("out", help="抽样输出目录")
    ap.add_argument("stride", nargs="?", type=int, default=8, help="步长，默认 8")
    args = ap.parse_args()

    if not os.path.isdir(args.frames):
        sys.exit(f"没有这个目录：{args.frames}")
    if args.stride < 1:
        sys.exit("步长至少 1")

    names = [f for f in os.listdir(args.frames) if f.endswith(".bin")]
    if not names:
        sys.exit(f"{args.frames} 里没有 .bin")

    # 按序排好再抽样，保证朝向是均匀扫过的
    def seq_of(n):
        m = re.match(r"f?(\d+)-(\d+)\.bin$", n)
        return int(m.group(1)) if m else 1 << 30

    names.sort(key=seq_of)
    os.makedirs(args.out, exist_ok=True)

    picked = names[:: args.stride]
    for n in picked:
        shutil.copy2(os.path.join(args.frames, n), os.path.join(args.out, n))

    print(f"{len(names)} 帧 → 抽样 {len(picked)} 帧（步长 {args.stride}）→ {args.out}")
    if len(picked) > 120:
        print(f"提示：{len(picked)} 帧还是偏多，可以把步长调大一点")


if __name__ == "__main__":
    main()
