"""把实机录制的采样窗压成"青色遮罩 + 位打包"，供 Rust 侧 include_bytes 重放。

为什么要打包：本机沙箱下 Rust 子进程读不了文件系统，而 python/pwsh 可以。
把遮罩位打包成一个文件后，rustc 在编译期就能把它嵌进二进制。

为什么从遮罩能无损还原：`cyan_mask` 的阈值是 p99*0.74 且下限 112。
还原时箭头像素给 (0,255,255) 得分 255、背景给 (40,93,103) 得分 53，
无论箭头占比是否超过 1%，阈值都落在两者之间，所以还原出的遮罩与原帧一致。

格式：magic "GMASK1\0\0" | u32 帧数 | u32 边长 | 每帧 { u32 seq, 位打包遮罩 }
"""
import pathlib
import re
import struct
import sys

import numpy as np

src = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path(
    r"C:\Users\Zhou\AppData\Local\Temp\gifa-live\frames"
)
out = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else pathlib.Path("docs/calibration/live-masks/packed.bin")
stride = int(sys.argv[3]) if len(sys.argv) > 3 else 1

files = sorted(src.glob("f*-*.bin"))
if not files:
    sys.exit(f"{src} 里没有帧")
files = files[::stride]

records = []
side = None
for p in files:
    m = re.match(r"f(\d+)-(\d+)\.bin$", p.name)
    if not m:
        continue
    seq = int(m.group(1))
    d = np.frombuffer(p.read_bytes(), dtype=np.uint8)
    s = int((len(d) // 4) ** 0.5)
    if side is None:
        side = s
    if s != side:
        continue
    a = d.reshape(s, s, 4).astype(np.int32)
    r, g, b = a[:, :, 0], a[:, :, 1], a[:, :, 2]
    score = np.clip(np.minimum(g, b) - r, 0, 255)
    hist = np.bincount(score.ravel(), minlength=256)
    tot = hist.sum()
    acc = 0
    p99 = 0
    for v, c in enumerate(hist):
        acc += c
        if acc >= tot * 0.99:
            p99 = v
            break
    thr = max(p99 * 0.74, 112.0)
    mask = (score >= thr).astype(np.uint8)
    packed = np.packbits(mask.ravel()).tobytes()
    records.append((seq, packed))

out.parent.mkdir(parents=True, exist_ok=True)
with out.open("wb") as f:
    f.write(b"GMASK1\0\0")
    f.write(struct.pack("<II", len(records), side))
    for seq, packed in records:
        f.write(struct.pack("<I", seq))
        f.write(packed)

nbytes = sum(len(p) for _, p in records)
print(f"打包 {len(records)} 帧（步长 {stride}，边长 {side}）→ {out}（{out.stat().st_size/1e6:.2f} MB）")
print(f"  每帧遮罩 {nbytes//max(len(records),1)} 字节")
