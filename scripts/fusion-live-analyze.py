"""分析融合管线的实机采集 CSV。

用法：python scripts/fusion-live-analyze.py <csv>
"""
import csv
import re
import statistics
import sys
from collections import Counter


def num(s):
    if s is None:
        return None
    m = re.search(r"-?\d+\.?\d*", s)
    return float(m.group()) if m else None


def d(a, b):
    return ((a - b + 540.0) % 360.0) - 180.0


def main():
    path = sys.argv[1]
    rows = list(csv.DictReader(open(path, encoding="utf-8")))
    print(f"总帧 {len(rows)}")

    src_name = {"1": "v1", "2": "v3", "5": "融合(一致)", "6": "分歧丢弃", "": "无"}
    srcs = Counter(src_name.get(r["src"], r["src"]) for r in rows)
    print("来源分布:", dict(srcs))

    # 原始箭头角 vs 滤波后
    raw = [num(r["arrow"]) for r in rows]
    filt = [num(r["arrowF"]) for r in rows]
    view = [num(r["view"]) for r in rows]
    valid_raw = [(i, a) for i, a in enumerate(raw) if a is not None]
    valid_filt = [(i, a) for i, a in enumerate(filt) if a is not None]
    print(f"箭头有效（raw）{len(valid_raw)}，滤波后输出 {len(valid_filt)}")

    # 帧间 >90° 跳变：raw vs filtered（过滤后应该显著更少）
    def jumps(seq):
        idx = [i for i, a in seq]
        return sum(
            1 for k in range(1, len(idx))
            if abs(d(seq[k][1], seq[k - 1][1])) > 90.0
        )

    print(f"帧间 >90° 跳变：raw {jumps(valid_raw)}  滤波后 {jumps(valid_filt)}")

    # 每个来源的滤波输出与视锥一致性（按帧，只在两者都有值时）
    agree = Counter()
    for r in rows:
        a, v = num(r["arrowF"]), num(r["view"])
        if a is None or v is None:
            continue
        s = src_name.get(r["src"], r["src"])
        agree[s + ("一致" if abs(d(a, v)) < 30 else "分歧")] += 1
    print("滤波输出 vs 视锥（±30° 内算一致）:", dict(agree))

    # 丢弃帧的连续长度分布（融合分歧导致）
    drops = [i for i, r in enumerate(rows) if r["src"] == "6"]
    runs, cur = [], 0
    for i in range(len(rows)):
        if i in set(drops):
            cur += 1
        elif cur:
            runs.append(cur)
            cur = 0
    if cur:
        runs.append(cur)
    print(f"丢弃段: {len(drops)} 帧，连续段长度 {sorted(runs, reverse=True)[:8]}")

    # 分时段看来源占比（冷启动 → 预热）
    n = len(rows)
    for seg in range(3):
        part = rows[seg * n // 3 : (seg + 1) * n // 3]
        c = Counter(src_name.get(r["src"], r["src"]) for r in part)
        print(f"  第 {seg*len(rows)//30}~{(seg+1)*len(rows)//30} 秒来源: {dict(c)}")


if __name__ == "__main__":
    main()
