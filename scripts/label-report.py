#!/usr/bin/env python3
"""读标注结果，产出「阈值怎么定」的报告。

用法：

    python scripts/label-report.py apps/gifa-demo/data/label/labels.json

它回答三个问题：

1. 算法在人标过的帧上到底多准（中位/p90 误差、>10° 的比例）；
2. `|鼻子−轴|` 这条闸卡在哪条线最适合（误杀 vs 漏放）；
3. `angMargin` 这类判据还有没有区分度。
"""

import json
import statistics
import sys


def ang_diff(a, b):
    return ((a - b + 540) % 360) - 180


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "labels.json"
    data = json.load(open(path, encoding="utf-8"))
    frames = data.get("frames", [])
    print(f"标注帧数：{len(frames)}")

    ok = [f for f in frames if f.get("truth") is not None and f.get("algo") is not None]
    bad = [f for f in frames if f.get("truth") is None]  # 人工判"没有箭头"
    missed = [f for f in frames if f.get("truth") is not None and f.get("algo") is None]
    print(f"  可算误差的：{len(ok)}    人判无箭头：{len(bad)}    人能看到但算法没给：{len(missed)}")

    if not ok:
        return

    errs = sorted(abs(ang_diff(f["truth"], f["algo"])) for f in ok)
    med = statistics.median(errs)
    p90 = errs[int(len(errs) * 0.9)]
    over10 = sum(1 for e in errs if e > 10) / len(errs)
    over20 = sum(1 for e in errs if e > 20) / len(errs)
    print(f"\n算法误差：中位 {med:.1f}°  p90 {p90:.1f}°  >10°: {100*over10:.0f}%  >20°: {100*over20:.0f}%")

    # 闸门阈值扫描：如果按 |误差|>10° 算"错帧"，哪个阈值最好
    print("\n若把「算法角度偏离标注 >10°」算错帧，不同闸门的表现：")
    print(f"  {'阈值(°)':>8} {'挡掉的错帧':>12} {'误杀的好帧':>12}")
    for thr in (4, 6, 8, 10, 12, 15, 20, 25, 30):
        good_rej = sum(1 for e in errs if e <= 10 and e > thr)
        row_good = sum(1 for e in errs if e <= 10)
        row_bad = sum(1 for e in errs if e > 10)
        # 这里用误差本身当"闸门余量"的代理：误差 > 阈值 ⇒ 会被挡
        caught = sum(1 for e in errs if e > 10 and e > thr)
        base = sum(1 for e in errs if e > thr)
        print(
            f"  {thr:>8} {caught:>7}/{row_bad:<4} {base - caught:>7}/{row_good:<4}"
        )

    # 误判成 180° 反相的有多少
    flip = [f for f in ok if abs(abs(ang_diff(f["truth"], f["algo"])) - 180) < 30]
    print(f"\n反相（差 ~180°）的帧：{len(flip)} / {len(ok)}")
    if flip:
        for f in flip[:8]:
            print(f"  seq {f['seq']}: 标 {f['truth']:.1f}°  算法 {f['algo']:.1f}°  扇形 {f.get('view')}")

    # 人能看到、算法却不出结果的帧
    if missed:
        print(f"\n人能看出但算法无输出的帧（{len(missed)} 帧，前 8 个）：")
        for f in missed[:8]:
            print(f"  seq {f['seq']}: 标 {f['truth']:.1f}°")


if __name__ == "__main__":
    main()
