#!/usr/bin/env python3
"""把「采样窗 .bin + 逐帧 CSV」打包成一个单文件标注页。

用法（在 `` 下）：

    python scripts/label-tool.py apps/gifa-demo/data/seq apps/gifa-demo/data/seq.csv \
        apps/gifa-demo/data/label

产出 `label.html`：双击用浏览器打开即可，不依赖网络、不依赖本地服务器。

标注约定：**从窗口中心往箭头尖端的方向拖一条线**，工具自己换算成角度。
"""

import argparse
import base64
import csv
import io
import json
import os
import re
import sys

try:
    from PIL import Image
except ImportError:  # pragma: no cover
    sys.exit("需要 Pillow：pip install pillow")


def load_csv(path):
    """seq → {arrow, view, conf, cyan, rej}"""
    out = {}
    if not path or not os.path.exists(path):
        return out

    def num(s):
        if s is None:
            return None
        m = re.search(r"-?\d+\.?\d*", s)
        return float(m.group()) if m else None

    for row in csv.DictReader(open(path, encoding="utf-8")):
        try:
            seq = int(row["seq"])
        except (KeyError, ValueError):
            continue
        out[seq] = {
            "arrow": num(row.get("arrow")),
            "view": num(row.get("view")),
            "cyan": num(row.get("cyan")),
            "rej": row.get("rej") or "",
        }
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("frames", help="采样窗 .bin 所在目录（GIFA_DUMP_ALL=1 跑出来的）")
    ap.add_argument("csv", nargs="?", default=None, help="逐帧 CSV（--vision-csv）")
    ap.add_argument("out", help="输出目录")
    ap.add_argument("--limit", type=int, default=0, help="只用前 N 帧")
    args = ap.parse_args()

    meta = load_csv(args.csv)
    files = sorted(
        (f for f in os.listdir(args.frames) if f.endswith(".bin")),
        key=lambda f: int(f.split("-")[0].lstrip("f") or 0),
    )
    if args.limit:
        files = files[: args.limit]
    if not files:
        sys.exit(f"{args.frames} 里没有 .bin（先跑 GIFA_DUMP_ALL=1 ... --dump-frame）")

    os.makedirs(args.out, exist_ok=True)
    items = []
    for name in files:
        m = re.match(r"f?(\d+)-(\d+)\.bin$", name)
        if not m:
            continue
        seq, side = int(m.group(1)), int(m.group(2))
        raw = open(os.path.join(args.frames, name), "rb").read()
        if len(raw) != side * side * 4:
            continue
        img = Image.frombytes("RGBA", (side, side), raw).convert("RGB")
        # 丢掉空白帧（视频还没出画面时截到的全黑/全灰）：标它们没有意义
        px = img.resize((16, 16)).getdata()
        if max(max(c) for c in px) < 24:
            continue
        buf = io.BytesIO()
        img.save(buf, format="PNG", optimize=True)
        b64 = base64.b64encode(buf.getvalue()).decode("ascii")
        info = meta.get(seq, {})
        items.append(
            {
                "seq": seq,
                "side": side,
                "png": b64,
                "arrow": info.get("arrow"),
                "view": info.get("view"),
                "rej": info.get("rej", ""),
            }
        )

    # 有算法角度的排前面：多数时候只需确认（空格），先把快的清完
    items.sort(key=lambda it: (it["arrow"] is None, it["seq"]))
    payload = json.dumps(items, ensure_ascii=False)
    html = TEMPLATE.replace("__ITEMS__", payload)
    path = os.path.join(args.out, "label.html")
    with open(path, "w", encoding="utf-8") as f:
        f.write(html)
    size = os.path.getsize(path) / 1e6
    print(f"{len(items)} 帧 → {path}（{size:.1f} MB）")
    print("双击打开，或用浏览器打开该文件；标注完点「导出」保存 labels.json")


TEMPLATE = r"""<!doctype html>
<meta charset="utf-8">
<title>箭头标注</title>
<style>
  :root { color-scheme: dark; }
  body { margin: 0; font: 13px/1.5 system-ui, "Microsoft YaHei", sans-serif;
         background: #14161a; color: #dfe4ea; display: flex; height: 100vh; }
  #left { flex: 1; display: flex; flex-direction: column; align-items: center;
          justify-content: center; position: relative; }
  #wrap { position: relative; }
  canvas { image-rendering: pixelated; border-radius: 50%; cursor: crosshair;
           box-shadow: 0 0 0 1px #2b3038, 0 8px 40px #0008; }
  #right { width: 320px; flex: 0 0 320px; border-left: 1px solid #262b33;
           padding: 14px; overflow: auto; }
  h1 { font-size: 15px; margin: 0 0 10px; }
  .k { color: #7d8794; }
  .v { font-variant-numeric: tabular-nums; }
  .big { font-size: 26px; font-variant-numeric: tabular-nums; margin: 6px 0 2px; }
  .row { display: flex; justify-content: space-between; padding: 3px 0;
         border-bottom: 1px solid #1e232b; }
  .ok { color: #4ade80; } .bad { color: #f87171; } .warn { color: #fbbf24; }
  button { background: #232830; color: #dfe4ea; border: 1px solid #333a45;
           border-radius: 6px; padding: 6px 10px; cursor: pointer; font: inherit; }
  button:hover { background: #2c323c; }
  .btns { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 10px; }
  kbd { background: #232830; border: 1px solid #333a45; border-radius: 4px;
        padding: 0 5px; font-size: 12px; }
  #bar { height: 4px; background: #232830; border-radius: 2px; margin: 12px 0; }
  #fill { height: 100%; background: #3b82f6; border-radius: 2px; transition: width .2s; }
  #hint { color: #7d8794; font-size: 12px; margin-top: 12px; line-height: 1.7; }
</style>

<div id="left"><div id="wrap"><canvas id="cv"></canvas></div></div>

<div id="right">
  <h1>箭头方向标注</h1>
  <div class="k">在这里拖一条线：从圆心 → 箭头尖端</div>
  <div class="big" id="userDeg">—</div>
  <div class="k">你的标注（罗盘角：0=北 90=东 180=南 270=西）</div>
  <div id="bar"><div id="fill" style="width:0%"></div></div>
  <div class="row"><span class="k">帧</span><span class="v" id="seq">—</span></div>
  <div class="row"><span class="k">算法角度</span><span class="v" id="algo">—</span></div>
  <div class="row"><span class="k">视角（扇形）</span><span class="v" id="view">—</span></div>
  <div class="row"><span class="k">差（你的−算法）</span><span class="v" id="diff">—</span></div>
  <div class="row"><span class="k">已标</span><span class="v" id="done">0</span></div>

  <div class="btns">
    <button id="bAccept">按算法值接受 <kbd>空格</kbd></button>
    <button id="bBad">看不出 / 没有箭头 <kbd>U</kbd></button>
  </div>
  <div class="btns">
    <button id="bPrev">上一帧 <kbd>←</kbd></button>
    <button id="bNext">下一帧 <kbd>→</kbd></button>
    <button id="bSave">导出 labels.json <kbd>S</kbd></button>
    <button id="bClear">清空重来</button>
  </div>

  <div id="hint">
    <b>怎么标</b><br>
    1. 看圆心那个青色箭头，<b>从中心往尖端拖</b>（按住鼠标拖，松手即定）<br>
    2. 方向对就按 <kbd>空格</kbd> 接受（多数帧都是一步）<br>
    3. 拖歪了就再拖一次；实在看不出（被特效盖住、地图不在）按 <kbd>U</kbd><br>
    4. 每帧都会自动存，关掉浏览器再开不会丢<br>
    <br>
    <span class="warn">绿色</span> = 你的标注，<span class="bad">红色</span> = 算法结果。
  </div>
</div>

<script>
const ITEMS = __ITEMS__;
const KEY = 'gifa-labels-v1';
const cv = document.getElementById('cv');
const ctx = cv.getContext('2d');
let i = 0;
let labels = JSON.parse(localStorage.getItem(KEY) || '{}');
let cur = null;

function angDiff(a, b) { return ((a - b + 540) % 360) - 180; }
function norm(d) { return ((d % 360) + 360) % 360; }

function show() {
  const it = ITEMS[i];
  const side = it.side;
  cv.width = side; cv.height = side;
  // 宽度跟着窗口走，避免小窗口时把右侧面板挤出屏幕
  const avail = Math.min(window.innerWidth - 360, window.innerHeight - 80, 620);
  const w = Math.max(220, Math.min(avail, side * 2));
  cv.style.width = w + 'px';
  cv.style.height = w + 'px';
  const img = new Image();
  img.onload = () => { cur = it; draw(); };
  img.src = 'data:image/png;base64,' + it.png;
  document.getElementById('seq').textContent = `${i + 1} / ${ITEMS.length} · seq ${it.seq}`;
  const algo = it.arrow;
  document.getElementById('algo').textContent = algo == null ? '（无）' : algo.toFixed(1) + '°';
  document.getElementById('view').textContent = it.view == null ? '（无）' : it.view.toFixed(1) + '°';
  const n = Object.keys(labels).length;
  document.getElementById('done').textContent = `${n} / ${ITEMS.length}`;
  document.getElementById('fill').style.width = (100 * n / ITEMS.length) + '%';
  updateDeg();
}

function draw() {
  if (!cur) return;
  const side = cur.side;
  ctx.clearRect(0, 0, side, side);
  const img = new Image();
  img.onload = () => {
    ctx.drawImage(img, 0, 0);
    const c = side / 2, R = side * 0.36;
    // 算法角度（红）
    if (cur.arrow != null) arrow(c, R, cur.arrow, '#f87171', 2);
    // 用户角度（绿）
    const u = labels[cur.seq];
    if (u && typeof u.deg === 'number') arrow(c, R, u.deg, '#4ade80', 3);
    // 十字
    ctx.strokeStyle = '#ffffff44'; ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(c - 6, c); ctx.lineTo(c + 6, c);
    ctx.moveTo(c, c - 6); ctx.lineTo(c, c + 6); ctx.stroke();
  };
  img.src = 'data:image/png;base64,' + cur.png;
  updateDeg();
}

function arrow(c, R, compassDeg, color, w) {
  // 罗盘角（0=北，顺时针）→ 屏幕坐标
  const a = (compassDeg - 90) * Math.PI / 180;
  const x = c + Math.cos(a) * R, y = c + Math.sin(a) * R;
  ctx.strokeStyle = color; ctx.lineWidth = w; ctx.lineCap = 'round';
  ctx.beginPath(); ctx.moveTo(c, c); ctx.lineTo(x, y); ctx.stroke();
  ctx.beginPath(); ctx.arc(x, y, 4, 0, Math.PI * 2); ctx.fillStyle = color; ctx.fill();
}

function updateDeg() {
  const u = cur ? labels[cur.seq] : null;
  const el = document.getElementById('userDeg');
  const df = document.getElementById('diff');
  if (u && typeof u.deg === 'number') {
    el.textContent = u.deg.toFixed(1) + '°';
    el.className = 'big';
    if (cur.arrow != null) {
      const d = angDiff(u.deg, cur.arrow);
      df.textContent = (d >= 0 ? '+' : '') + d.toFixed(1) + '°';
      df.className = 'v ' + (Math.abs(d) > 10 ? 'bad' : 'ok');
    } else { df.textContent = '—'; df.className = 'v'; }
  } else {
    el.textContent = u && u.bad ? '标为不可用' : '—';
    el.className = 'big' + (u && u.bad ? ' warn' : '');
    df.textContent = '—'; df.className = 'v';
  }
}

// 拖动标注：从圆心往尖端。**必须拖够长**才落标注 —— 防误触/误点
let drag = false;
let dragStart = null;
let dragEnd = null;
function setFromEvent(e, commit) {
  if (!cur) return;
  const r = cv.getBoundingClientRect();
  const x = (e.clientX - r.left) * (cv.width / r.width);
  const y = (e.clientY - r.top) * (cv.height / r.height);
  const c = cur.side / 2;
  const dx = x - c, dy = y - c;
  if (commit) {
    // 起点和终点都要离圆心够远，且拖动长度够长，才算一次标注
    if (!dragStart) return;
    const len = Math.hypot(dx - (dragStart.x - c), dy - (dragStart.y - c));
    if (len < cur.side * 0.12 || Math.hypot(dx, dy) < cur.side * 0.12) return;
  } else {
    dragEnd = { x, y };
  }
  if (Math.hypot(dx, dy) < 4) return;
  // 屏幕角 → 罗盘角：屏幕 y 向下，罗盘 0=北、顺时针
  const compass = norm(Math.atan2(dy, dx) * 180 / Math.PI + 90);
  labels[cur.seq] = { deg: Math.round(compass * 10) / 10, bad: false };
  persist(); draw();
}
cv.addEventListener('mousedown', e => {
  if (!cur) return;
  const r = cv.getBoundingClientRect();
  drag = true;
  dragStart = {
    x: (e.clientX - r.left) * (cv.width / r.width),
    y: (e.clientY - r.top) * (cv.height / r.height),
  };
});
cv.addEventListener('mousemove', e => { if (drag) setFromEvent(e, false); });
window.addEventListener('mouseup', e => {
  if (drag) {
    const r = cv.getBoundingClientRect();
    setFromEvent(
      { clientX: e.clientX ?? (dragStart ? r.left + dragStart.x * r.width / cv.width : 0),
        clientY: e.clientY ?? (dragStart ? r.top + dragStart.y * r.height / cv.height : 0) },
      true
    );
  }
  drag = false;
});

function persist() { localStorage.setItem(KEY, JSON.stringify(labels)); }

function accept() {
  if (!cur) return;
  if (cur.arrow == null) { markBad(); return; }
  labels[cur.seq] = { deg: Math.round(cur.arrow * 10) / 10, bad: false };
  persist(); next();
}
function markBad() {
  if (!cur) return;
  labels[cur.seq] = { deg: null, bad: true };
  persist(); next();
}
function next() { if (i < ITEMS.length - 1) { i++; show(); } }
function prev() { if (i > 0) { i--; show(); } }

function save() {
  const rows = ITEMS.filter(it => labels[it.seq] != null).map(it => ({
    seq: it.seq,
    truth: labels[it.seq].bad ? null : labels[it.seq].deg,
    bad: !!labels[it.seq].bad,
    algo: it.arrow,
    view: it.view,
  }));
  const blob = new Blob([JSON.stringify({ frames: rows }, null, 1)], { type: 'application/json' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = 'labels.json';
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 5000);
}

document.getElementById('bAccept').onclick = accept;
document.getElementById('bBad').onclick = markBad;
document.getElementById('bNext').onclick = next;
document.getElementById('bPrev').onclick = prev;
document.getElementById('bSave').onclick = save;
document.getElementById('bClear').onclick = () => {
  if (confirm('清空所有标注？（导出前请先确认）')) { labels = {}; persist(); show(); }
};

window.addEventListener('keydown', e => {
  if (e.key === ' ') { e.preventDefault(); accept(); }
  else if (e.key === 'u' || e.key === 'U') markBad();
  else if (e.key === 'ArrowRight') { e.preventDefault(); next(); }
  else if (e.key === 'ArrowLeft') { e.preventDefault(); prev(); }
  else if (e.key === 's' || e.key === 'S') save();
});

show();
</script>
"""

if __name__ == "__main__":
    main()
