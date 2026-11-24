"""渲染实机采集里被融合丢弃的帧，肉眼核对 v3/先验为什么分歧。"""
import csv
import os
import sys
from PIL import Image, ImageDraw

csv_path = sys.argv[1] if len(sys.argv) > 1 else os.path.expandvars(r"%TEMP%\gifa-live\fusion.csv")
frames_dir = sys.argv[2] if len(sys.argv) > 2 else os.path.expandvars(r"%TEMP%\gifa-live\frames")

rows = list(csv.DictReader(open(csv_path, encoding="utf-8")))
drops = [int(r["seq"]) for r in rows if r["src"] == "6"]
print(f"丢弃帧 {len(drops)}，采样 3 帧")

picks = [drops[len(drops) // 4], drops[len(drops) // 2], drops[3 * len(drops) // 4]]
ims = []
for seq in picks:
    p = os.path.join(frames_dir, f"f{seq:03}-158.bin")
    if not os.path.exists(p):
        print("缺", p)
        continue
    data = open(p, "rb").read()
    side = int((len(data) // 4) ** 0.5)
    im = Image.frombytes("RGBA", (side, side), data).convert("RGB").resize((side * 3, side * 3), Image.NEAREST)
    d = ImageDraw.Draw(im)
    d.text((4, 4), f"f{seq:03} dropped", fill=(255, 255, 0))
    ims.append(im)

if ims:
    w = sum(i.width for i in ims)
    h = max(i.height for i in ims)
    out = Image.new("RGB", (w, h), (0, 0, 0))
    x = 0
    for i in ims:
        out.paste(i, (x, 0))
        x += i.width
    out_path = os.path.join(frames_dir, "drops.png")
    out.save(out_path)
    print("拼图完成", out.size, "采样帧", picks, "→", out_path)
