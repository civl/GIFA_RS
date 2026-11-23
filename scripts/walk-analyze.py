#!/usr/bin/env python3
"""分析"直线行走"录制的 arrow / view 一致性。

原理：按住 W 直走时角色朝向 ≈ 镜头朝向，青色箭头应与白色视野扇形重合。
所以两路角度的差应当集中在 0 附近；若稳定偏一个常数，就是箭头通道的系统偏差。

用法：python scripts/walk-analyze.py walk.csv
"""
import csv
import re
import statistics
import sys


def num(s):
    m = re.search(r"-?\d+\.?\d*", s or "")
    return float(m.group()) if m else None


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "walk.csv"
    rows = list(csv.DictReader(open(path, encoding="utf-8")))
    seq = []
    for i, r in enumerate(rows):
        a, v = num(r["arrow"]), num(r["view"])
        seq.append((i, a, v, r["src"]))
    print(f"总帧 {len(seq)}（10 fps，约 {len(seq)/10:.0f}s）")
    have = [(i, a, v) for i, a, v, _ in seq if a is not None and v is not None]
    print(f"两路都有值 {len(have)} 帧")

    # 每 1 秒（10 帧）一段
    print("\n按秒统计（箭头中位 / 视锥中位 / 差）：")
    wins = []
    for s in range(0, len(seq), 10):
        seg = [(a, v) for _, a, v, _ in seq[s:s + 10] if a is not None and v is not None]
        if len(seg) < 5:
            continue
        am = statistics.median(a for a, _ in seg)
        vm = statistics.median(v for _, v in seg)
        d = ((am - vm + 540) % 360) - 180
        spread = statistics.median(abs(((a - am + 540) % 360) - 180) for a, _ in seg)
        wins.append((s // 10, am, vm, d, spread, len(seg)))
        print(f"  {s//10:>3}s  箭头 {am:6.1f}°  视锥 {vm:6.1f}°  差 {d:+6.1f}°  箭头抖动 {spread:4.1f}°")

    # 找"最像直走"的连续段：差最稳定且抖动小
    if not wins:
        print("\n没有足够数据")
        return
    best = sorted(wins, key=lambda w: w[4])[:8]
    stable = [w for w in best if w[4] < 3.0]
    print("\n抖动最小的几段（最可能是直走 / 静止）：")
    for w in sorted(stable, key=lambda x: x[0]):
        print(f"  {w[0]:>3}s  差 {w[3]:+6.1f}°  抖动 {w[4]:.1f}°")
    if stable:
        ds = [w[3] for w in stable]
        print(f"\n这些段的差值中位 {statistics.median(ds):+.1f}°")
        if abs(statistics.median(ds)) < 8:
            print("→ 箭头与视锥基本一致：通道没有系统性偏差")
        else:
            print("→ 存在系统性偏差：箭头通道需要标定修正")


if __name__ == "__main__":
    main()
