'use strict';

/* 视觉算法的**原版参考实现**：直接从 reference/GIFA 的 webview-preload.js 里切出来跑。
 *
 * 为什么要切而不是抄：抄一遍就会和原版漂移，对拍就失去意义。
 * 这里在运行时读原文件、按行号截取纯数学函数段（cyanScore … analyzeConeWithRecovery），
 * 补上它们需要的少量全局（BEARING.mask 缓冲），再用 new Function 装成模块。
 *
 * 用法：
 *   node harness.js analyze <frame.bin> <size> <step>     → 输出 JSON
 *   node harness.js gen <outDir>                           → 生成合成帧 + index.json
 */

const fs = require('fs');
const path = require('path');

const SRC = path.resolve(__dirname, '../../../reference/GIFA/src/webview-preload.js');

/** 从原版源码里截出算法段并求值，返回 {analyze, analyzeCone, smoothBearing, updateView, ...} */
function loadReference() {
  const lines = fs.readFileSync(SRC, 'utf8').split('\n');
  // cyanScore(562) … analyzeConeWithRecovery 结束（tickBearing 之前）
  const start = lines.findIndex((l) => l.startsWith('function cyanScore('));
  const endLine = lines.findIndex((l) => l.startsWith('function tickBearing('));
  if (start < 0 || endLine < 0) throw new Error('找不到算法段边界');
  let body = lines.slice(start, endLine).join('\n');
  // 实验用：THR_RATIO 覆盖自适应阈值的系数（原版 0.74），用来验证"背景青色污染"假设
  if (process.env.THR_RATIO || process.env.THR_FLOOR) {
    const ratio = process.env.THR_RATIO || '0.74';
    const floor = process.env.THR_FLOOR || '112';
    body = body
      .replace(/Math\.max\(112, p99 \* 0\.74\)/g, `Math.max(${floor}, p99 * ${ratio})`)
      .replace(/Math\.max\(THRESHOLD_FLOOR, p99 \* 0\.74\)/g, `Math.max(${floor}, p99 * ${ratio})`);
  }
  if (process.env.DEBUG) {
    // 只在自己的副本上加打点，不动原版文件
    body = body
      .replace('if (cyans < 12) return empty;', "if (cyans < 12) { console.error('GATE cyans', cyans); return empty; }")
      .replace('if (!bestId) return empty;', "if (!bestId) { console.error('GATE noBlob'); return empty; }")
      .replace(
        'if (bcRho > rMax * 0.85) return empty;',
        "if (bcRho > rMax * 0.85) { console.error('GATE centroid', bcRho, rMax); return empty; }"
      )
      .replace(
        'if (!chosen) return empty;',
        "if (!chosen) { console.error('GATE noChosen corners=', corners.length, JSON.stringify(corners)); return empty; }"
      );
  }

  const prelude = `
    const BEARING = {
      mask: null, canvas: null, ctx: null, layoutId: null, revision: 0,
      ab: { theta: -1, omega: 0, t: 0, suspect: 0 },
      view: { theta: -1, conf: 0, src: '', t: 0, pending: -1 },
    };
    const DEFAULTS = { step: 1 };
    const results = {};
  `;
  const epilogue = `
    results.cyanScore = cyanScore;
    results.analyze = analyze;
    results.analyzeCone = analyzeCone;
    results.degOf = degOf;
    results.angDiff = angDiff;
    results.smoothBearing = smoothBearing;
    results.updateView = updateView;
    results.analyzeArrowWithRecovery = analyzeArrowWithRecovery;
    results.analyzeConeWithRecovery = analyzeConeWithRecovery;
    results.markInvalid = markInvalid;
    results.viewPayload = viewPayload;
    results.BEARING = BEARING;
    return results;
  `;
  // eslint-disable-next-line no-new-func
  return new Function(prelude + body + epilogue)();
}

/** ImageData 的替身：只要 { data, width, height } */
function imageFromFile(file, size) {
  const buf = fs.readFileSync(file);
  if (buf.length !== size * size * 4) {
    throw new Error(`帧大小不符：${buf.length} != ${size * size * 4}`);
  }
  const data = new Uint8ClampedArray(buf.buffer, buf.byteOffset, buf.length);
  return { data, width: size, height: size };
}

// ------------------------------------------------------------------ 合成帧

/**
 * 画一张"类小地图"的帧：
 *  - 深绿地形 + 噪声（模拟草地/树影，cyanScore 20~90）
 *  - 青色三角（角色箭头），指向 theta
 *  - 白色扇形（视锥），指向 phi
 *  - 可选干扰：远处青色小岛（传送锚点/神瞳）
 */
function synthFrame({ size, theta, phi, decoys = 0, seed = 1 }) {
  const d = new Uint8ClampedArray(size * size * 4);
  let s = seed >>> 0 || 1;
  const rand = () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 0xffffffff;
  };

  // 地形：深绿，带轻微噪声
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const i = (y * size + x) * 4;
      const n = rand() * 26 - 13;
      d[i] = Math.max(0, Math.min(255, 34 + n));
      d[i + 1] = Math.max(0, Math.min(255, 78 + n));
      d[i + 2] = Math.max(0, Math.min(255, 62 + n));
      d[i + 3] = 255;
    }
  }

  const put = (x, y, r, g, b) => {
    if (x < 0 || y < 0 || x >= size || y >= size) return;
    const i = (Math.round(y) * size + Math.round(x)) * 4;
    d[i] = r; d[i + 1] = g; d[i + 2] = b; d[i + 3] = 255;
  };

  const c = (size - 1) / 2;
  const rad = (deg) => (deg * Math.PI) / 180;
  // 屏幕坐标：x 右、y 下；罗盘角 0=北、顺时针 → 单位向量 (sin, -cos)
  const dir = (deg) => [Math.sin(rad(deg)), -Math.cos(rad(deg))];

  // 白膜扇形（先画，箭头压在上面）。
  // 关键：cone 算法认的是**半透明白膜** `dst = src + α·(255−src)`，α 随半径衰减
  // （实测 0.17R≈0.35 → 0.72R≈0.12），不是实心白。画成实心白会解出 α≈1 被 A_MAX 否掉。
  // 张角 90°（两条半径边在 ±45°），这是算法配对时用的固定关系。
  if (phi != null) {
    const [ux, uy] = dir(phi);
    const rIn = size * 0.16;
    const rOut = size * 0.48;
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        const dx = x - c;
        const dy = y - c;
        const dist = Math.hypot(dx, dy);
        if (dist < rIn || dist > rOut) continue;
        const cosang = (dx * ux + dy * uy) / (dist || 1);
        if (Math.acos(Math.max(-1, Math.min(1, cosang))) > rad(45)) continue;
        const i = (y * size + x) * 4;
        const a = 0.35 - 0.25 * ((dist - rIn) / (rOut - rIn));
        d[i] = Math.min(255, d[i] + a * (255 - d[i]));
        d[i + 1] = Math.min(255, d[i + 1] + a * (255 - d[i + 1]));
        d[i + 2] = Math.min(255, d[i + 2] + a * (255 - d[i + 2]));
      }
    }
  }

  // 青色箭头：**飞镖形**（钝鼻 + 两个尖尾）。
  // 原版算法靠"半径最小的钝角 = 鼻子、两个尖角 = 尾巴"定方向，
  // 实心三角形只有一个径向峰值，根本过不了三点枚举 —— 第一版就是这么失败的。
  if (theta != null) {
    const [ux, uy] = dir(theta);
    const wing = rad(62);
    const rn = size * 0.17;   // 鼻子（离圆心近、钝）
    const rt = size * 0.235;  // 尾巴（远、尖）
    const rc = size * 0.10;   // 凹口（尾巴之间、更靠近圆心）
    const P = (deg, r) => {
      const [x, y] = dir(deg);
      return [c + x * r, c + y * r];
    };
    // 尾翼在鼻子的**反侧** ±118°：算法要求「鼻子落在两尾翼中分线的正对面」，
    // 画成同侧 ±62° 会让 o1/o2 只有 62°，直接被硬约束否掉。
    // 翼角 ±130° → 两尾翼夹角 100°、鼻子到尾翼 130°，与实测一致
    // （实测 tails 100°、nose→tail 123~137，angMargin 才是正的）
    const poly = [P(theta, rn), P(theta + 130, rt), P(theta + 180, rc), P(theta - 130, rt)];
    const inside = (px, py) => {
      let hit = false;
      for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
        const [xi, yi] = poly[i];
        const [xj, yj] = poly[j];
        if ((yi > py) !== (yj > py) && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi) hit = !hit;
      }
      return hit;
    };
    const R = Math.ceil(rt) + 2;
    for (let dy = -R; dy <= R; dy++) {
      for (let dx = -R; dx <= R; dx++) {
        const px = c + dx;
        const py = c + dy;
        if (inside(px, py)) put(px, py, 34, 200, 232);
      }
    }
    void wing;
  }

  // 干扰：远处青色斑块（不该影响箭头方向）
  for (let i = 0; i < decoys; i++) {
    const a = rand() * 360;
    const r = size * (0.34 + rand() * 0.1);
    const [ux, uy] = dir(a);
    const bx = c + ux * r;
    const by = c + uy * r;
    const br = size * 0.03;
    for (let y = -br; y <= br; y++) {
      for (let x = -br; x <= br; x++) {
        if (x * x + y * y <= br * br) put(bx + x, by + y, 34, 200, 232);
      }
    }
  }
  return d;
}

// ------------------------------------------------------------------ 命令行

function main() {
  const [mode, ...rest] = process.argv.slice(2);

  if (mode === 'analyze') {
    const [file, sizeStr, stepStr] = rest;
    const size = Number(sizeStr);
    const step = Number(stepStr || 1);
    const ref = loadReference();
    const img = imageFromFile(file, size);
    const a = ref.analyze(img, size, step);
    const c = ref.analyzeCone(img, size, step);
    process.stdout.write(
      JSON.stringify({ arrow: a, cone: c })
    );
    return;
  }

  if (mode === 'batch') {
    // 对 frames/index.json 里每一帧跑一遍参考实现，结果写成 expected.json
    const dir = rest[0] || 'frames';
    const cases = JSON.parse(fs.readFileSync(path.join(dir, 'index.json'), 'utf8'));
    const ref = loadReference();
    const out = [];
    for (const c of cases) {
      const img = imageFromFile(path.join(dir, c.name), c.size);
      const a = ref.analyze(img, c.size, 1);
      const cone = ref.analyzeCone(img, c.size, 1);
      out.push({
        name: c.name,
        size: c.size,
        theta: c.theta,
        arrow: {
          valid: a.valid,
          angle: a.angle,
          conf: a.conf,
          cyans: a.cyans,
          tipFrac: a.tipFrac,
          angMargin: a.angMargin ?? 0,
          radMargin: a.radMargin ?? 0,
          noseFlat: a.noseFlat ?? 0,
        },
        cone: { valid: cone.valid, deg: cone.deg ?? 0, conf: cone.conf },
      });
    }
    fs.writeFileSync(path.join(dir, 'expected.json'), JSON.stringify(out, null, 1));
    console.log(`参考结果 ${out.length} 条 → ${dir}/expected.json`);
    return;
  }

  if (mode === 'seq') {
    // 生成一串观测，跑原版 smoothBearing / updateView，导出输入+输出给 Rust 复现
    const outFile = rest[0] || 'filter-seq.json';
    const ref = loadReference();
    let s0 = 12345;
    const rnd = () => { s0 = (s0 * 1664525 + 1013904223) >>> 0; return s0 / 0xffffffff; };
    const steps = [];
    let truth = 0;      // 真实朝向（缓慢变化）
    let now = 0;
    let t = 0;
    for (let i = 0; i < 240; i++) {
      now += 90 + Math.floor(rnd() * 40);        // 90~130ms/帧
      truth = (truth + (rnd() - 0.5) * 12 + 360) % 360;
      const r = rnd();
      let obs = (truth + (rnd() - 0.5) * 6 + 360) % 360;  // 通常贴着真值
      let conf = 0.55 + rnd() * 0.45;
      if (r < 0.05) obs = (obs + 180) % 360;              // 5% 单帧 180° 假峰
      else if (r < 0.12) conf = 0.2 + rnd() * 0.2;        // 7% 低置信
      const coneValid = rnd() > 0.15;
      const coneDeg = (truth + (rnd() - 0.5) * 20 + 360) % 360;
      const coneConf = coneValid ? 0.3 + rnd() * 0.7 : 0;
      steps.push({
        now, obs, conf, coneDeg, coneConf,
        coneValid, mapPresent: i % 200 !== 199,
      });
    }
    const smooth = [];
    const view = [];
    for (const st of steps) {
      smooth.push(ref.smoothBearing(st.obs, st.conf, st.now, st.coneDeg, st.coneConf));
      const cone = { valid: st.coneValid, deg: st.coneDeg, conf: st.coneConf, width: 90 };
      const v = ref.updateView(st.coneValid ? cone : null, st.now, st.mapPresent);
      view.push(v ? { deg: v.deg, conf: v.conf, src: v.src } : null);
    }
    fs.writeFileSync(outFile, JSON.stringify({ steps, smooth, view }, null, 1));
    console.log(`时序序列 ${steps.length} 步 → ${outFile}`);
    return;
  }

  if (mode === 'gen') {
    const outDir = rest[0];
    fs.mkdirSync(outDir, { recursive: true });
    const cases = [];
    const sizes = [160, 200, 320];
    const angles = [0, 45, 90, 135, 180, 225, 270, 315];
    for (const size of sizes) {
      for (const theta of angles) {
        const phi = (theta + 50) % 360;
        const name = `s${size}_a${theta}.bin`;
        const data = synthFrame({ size, theta, phi, decoys: 0, seed: size + theta });
        fs.writeFileSync(path.join(outDir, name), Buffer.from(data.buffer));
        cases.push({ name, size, theta, phi, decoys: 0 });
      }
      // 带干扰的
      for (const theta of [30, 200]) {
        const phi = (theta + 40) % 360;
        const name = `s${size}_a${theta}_d3.bin`;
        const data = synthFrame({ size, theta, phi, decoys: 3, seed: size * 7 + theta });
        fs.writeFileSync(path.join(outDir, name), Buffer.from(data.buffer));
        cases.push({ name, size, theta, phi, decoys: 3 });
      }
      // 没有小地图的帧（只有地形）
      const name = `s${size}_empty.bin`;
      const data = synthFrame({ size, theta: null, phi: null, decoys: 0, seed: 99 });
      fs.writeFileSync(path.join(outDir, name), Buffer.from(data.buffer));
      cases.push({ name, size, theta: null, phi: null, decoys: 0 });
    }
    fs.writeFileSync(path.join(outDir, 'index.json'), JSON.stringify(cases, null, 2));
    console.log(`生成 ${cases.length} 帧 → ${outDir}`);
    return;
  }

  console.error('用法: node harness.js analyze <file> <size> [step] | gen <outDir>');
  process.exit(2);
}

if (require.main === module) main();
module.exports = { loadReference, synthFrame, imageFromFile };
