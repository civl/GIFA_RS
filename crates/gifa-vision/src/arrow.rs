//! 箭头通道：忠实移植原版 `analyze()`（`webview-preload.js:566-960`）。
//!
//! 逐步骤对应，方便和原版对拍：
//!   自适应阈值(p99, floor 112) → 半径带 mask → 连通块取最大 → 72 格径向剖面
//!   → 三点平滑 → 候选峰(≥50% 峰高、间隔≥25°) + 支撑宽度 span
//!   → 峰值精修(平均单位向量) → 枚举三点组合打分 → 定鼻尖 → 角度与置信度
//!
//! **坐标约定**：这里的 `angle` 与原版一致，是 `atan2` 角度（0=东、逆时针），
//! 不是罗盘角。叠加层要的是罗盘角，边界处转换：`compass = (angle + 90) % 360`。

/// 每格角度（72 格 = 5°）
const NB: usize = 72;
/// 自适应阈值下限
const THRESHOLD_FLOOR: f64 = 112.0;
/// 阈值 = max(FLOOR, p99 × 这个比例)
const THRESHOLD_RATIO: f64 = 0.74;
/// p99 低于这个值 → 这一帧根本没有小地图
const NO_MAP_P99: f64 = 100.0;
/// 有效半径带（相对窗口半边）
const RADIUS_MIN: f64 = 0.04;
const RADIUS_MAX: f64 = 0.5;

#[derive(Debug, Clone, Copy, Default)]
pub struct Corner {
    /// `atan2` 角度（0=东，逆时针）
    pub deg: f64,
    /// 该方向上的最远半径
    pub far: f64,
    /// 支撑宽度（格数）：鼻子宽、尾巴窄
    pub span: i32,
}

#[derive(Debug, Clone, Default)]
pub struct ArrowResult {
    /// 三点齐全且各项余量达标
    pub valid: bool,
    /// 行进方向（`atan2` 角度）
    pub angle: f64,
    pub conf: f64,
    pub cyans: u32,
    /// 尾翼伸出长度 / 窗口尺寸
    pub tip_frac: f64,
    /// 角度与质心方向的差
    pub spread: f64,
    pub area: u32,
    pub ang_margin: f64,
    pub rad_margin: f64,
    pub nose_flat: f64,
    pub corners: Vec<Corner>,
    pub nose_deg: Option<f64>,
    /// 前一版算法的角度（质心方向），只用于诊断
    pub a1: f64,
    /// 最大青色块的质心（窗口坐标），重试策略用来判"是不是贴着窗口中心"
    pub bc_x: f64,
    pub bc_y: f64,
}

/// 复用的缓冲区（对应原版的 `BEARING.mask/lbl/stack`）
pub struct ArrowWorkspace {
    mask: Vec<u8>,
    lbl: Vec<i32>,
    stack: Vec<i32>,
    far: [f32; NB],
    sm: [f32; NB],
}

impl Default for ArrowWorkspace {
    fn default() -> Self {
        Self {
            mask: Vec::new(),
            lbl: Vec::new(),
            stack: Vec::new(),
            far: [0.0; NB],
            sm: [0.0; NB],
        }
    }
}

#[inline]
fn cyan_score(r: u8, g: u8, b: u8) -> i32 {
    g.min(b) as i32 - r as i32
}

#[inline]
fn deg_of(ux: f64, uy: f64) -> f64 {
    (uy.atan2(ux).to_degrees() + 360.0) % 360.0
}

#[inline]
pub fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

#[allow(dead_code)] // bis_err/nose 留作诊断
struct Triple {
    score: f64,
    ang_margin: f64,
    rad_margin: f64,
    rad_sym: f64,
    bis_err: f64,
    nose_flat: f64,
    nose: Option<Corner>,
    t1: Corner,
    t2: Corner,
}

fn eval_triple(n: &Corner, t1: &Corner, t2: &Corner, params: &ArrowParams) -> Triple {
    let sep = ang_diff(t1.deg, t2.deg).abs();
    let o1 = ang_diff(n.deg, t1.deg).abs();
    let o2 = ang_diff(n.deg, t2.deg).abs();
    let ang_margin = o1.min(o2) - sep;
    let rad_margin = (t1.far.min(t2.far) - n.far) / n.far.max(1.0);
    let rad_sym = 1.0 - (1.0f64).min((t1.far - t2.far).abs() / t1.far.max(t2.far).max(1.0));

    let axis = (t1.deg + ang_diff(t2.deg, t1.deg) / 2.0 + 720.0) % 360.0;
    let want = (axis + 180.0) % 360.0;
    let bis_err = ang_diff(n.deg, want).abs();

    let tail_span = (t1.span + t2.span) as f64 / 2.0;
    let nose_flat = ((n.span as f64 - tail_span) / 4.0).clamp(-1.0, 1.0);

    let ok = sep > 60.0
        && sep < 150.0
        && o1 >= params.tail_angle_min
        && o1 <= params.tail_angle_max
        && o2 >= params.tail_angle_min
        && o2 <= params.tail_angle_max
        && bis_err <= params.bis_err_max
        && n.far < t1.far.min(t2.far) * params.nose_far_ratio
        && nose_flat > params.nose_flat_min;

    let score = if ok {
        ang_margin.min(25.0) / 45.0
            + rad_margin * 1.5
            + rad_sym * 0.25
            + (1.0 - bis_err / params.bis_err_max) * 0.5
            + nose_flat * 0.45
    } else {
        -99.0
    };
    Triple {
        score,
        ang_margin,
        rad_margin,
        rad_sym,
        bis_err,
        nose_flat,
        nose: Some(*n),
        t1: *t1,
        t2: *t2,
    }
}

/// JS 的 `Math.round`：`floor(x + 0.5)`。
/// 注意 Rust 的 `round()` 是「四舍五入远离零」，遇到正好 .5 时和 JS 不一样
/// （-12.5 → Rust -13 / JS -12），对拍时会差出 0.01。
#[inline]
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// 跑一帧。`rgba` 长度必须 = size²×4。
/// 三点拟合的容差。
///
/// 默认值 = 原版；真机实测需要放宽 `bis_err_max`（见 `detect_window` 的注释）。
#[derive(Debug, Clone, Copy)]
pub struct ArrowParams {
    /// 鼻子偏离"两翼中分线反向"的容差（度）。原版 15。
    pub bis_err_max: f64,
    /// 三点评分余量的下限（度）。原版 4。
    pub ang_margin_min: f64,
    /// 鼻尖"比两翼更靠内"的容差倍数。原版 1.02（鼻子必须更靠内）。
    pub nose_far_ratio: f64,
    /// 鼻尖"扁平度"下限。原版 -0.5。
    pub nose_flat_min: f64,
    /// 鼻子到两翼的夹角允许区间（度）。原版 95..162。
    pub tail_angle_min: f64,
    pub tail_angle_max: f64,
}

impl Default for ArrowParams {
    fn default() -> Self {
        Self {
            bis_err_max: 15.0,
            ang_margin_min: 4.0,
            nose_far_ratio: 1.02,
            nose_flat_min: -0.5,
            tail_angle_min: 95.0,
            tail_angle_max: 162.0,
        }
    }
}

pub fn analyze(rgba: &[u8], size: usize, step: usize, ws: &mut ArrowWorkspace) -> ArrowResult {
    analyze_with(rgba, size, step, ws, &ArrowParams::default())
}

/// 同 [`analyze`]，但可以调容差（管线用更宽松的真机参数）。
pub fn analyze_with(
    rgba: &[u8],
    size: usize,
    step: usize,
    ws: &mut ArrowWorkspace,
    params: &ArrowParams,
) -> ArrowResult {
    let cells = size * size;
    if rgba.len() < cells * 4 || size < 8 {
        return ArrowResult::default();
    }
    let step = step.max(1);
    let sxf = size as f64;
    let c0 = (sxf - 1.0) / 2.0;

    // ---- 自适应阈值（p99）
    let probe_step = step.max(4);
    let mut probe: Vec<i32> = Vec::new();
    for y in (0..size).step_by(probe_step) {
        for x in (0..size).step_by(probe_step) {
            let i = (y * size + x) * 4;
            probe.push(cyan_score(rgba[i], rgba[i + 1], rgba[i + 2]));
        }
    }
    if probe.is_empty() {
        return ArrowResult::default();
    }
    probe.sort_unstable();
    let p99 = probe[((probe.len() as f64 * 0.99).floor() as usize).min(probe.len() - 1)] as f64;
    if p99 < NO_MAP_P99 {
        return ArrowResult::default();
    }
    let thr = THRESHOLD_FLOOR.max(p99 * THRESHOLD_RATIO);

    let r_min = sxf * RADIUS_MIN;
    let r_max = sxf * RADIUS_MAX;

    // ---- mask
    if ws.mask.len() != cells {
        ws.mask = vec![0; cells];
    }
    ws.mask.fill(0);
    let mut cyans: u32 = 0;
    for y in (0..size).step_by(step) {
        let dy = c0 - y as f64;
        for x in (0..size).step_by(step) {
            let i = (y * size + x) * 4;
            // 必须按 double 比：JS 是 `score < thr`（thr 是小数），
            // 我这里若把 thr 截断成整数，正好卡在阈值上的像素会多算进来
            if (cyan_score(rgba[i], rgba[i + 1], rgba[i + 2]) as f64) < thr {
                continue;
            }
            let dx = x as f64 - c0;
            let rho = (dx * dx + dy * dy).sqrt();
            if rho < r_min || rho > r_max {
                continue;
            }
            ws.mask[y * size + x] = 1;
            cyans += 1;
        }
    }
    if cyans < 12 {
        return ArrowResult {
            cyans,
            ..Default::default()
        };
    }

    // ---- 连通块（4 邻域，步长 step）
    if ws.lbl.len() != cells {
        ws.lbl = vec![0; cells];
    }
    if ws.stack.len() != cells {
        ws.stack = vec![0; cells];
    }
    ws.lbl.fill(0);
    let mut best_id = 0;
    let mut best_area: u32 = 0;
    let (mut best_cx, mut best_cy) = (0.0f64, 0.0f64);
    let mut comp_id = 0;
    let step_i = step as i32;
    let size_i = size as i32;

    for seed in 0..cells {
        if ws.mask[seed] == 0 || ws.lbl[seed] != 0 {
            continue;
        }
        comp_id += 1;
        ws.lbl[seed] = comp_id;
        let mut top = 0usize;
        ws.stack[top] = seed as i32;
        top += 1;
        let mut area = 0u32;
        let (mut sum_x, mut sum_y) = (0f64, 0f64);
        while top > 0 {
            top -= 1;
            let cur = ws.stack[top] as usize;
            let py = (cur / size) as i32;
            let px = cur as i32 - py * size_i;
            area += 1;
            sum_x += px as f64;
            sum_y += py as f64;
            for k in 0..4 {
                let nx = px
                    + if k == 0 {
                        step_i
                    } else if k == 1 {
                        -step_i
                    } else {
                        0
                    };
                let ny = py
                    + if k == 2 {
                        step_i
                    } else if k == 3 {
                        -step_i
                    } else {
                        0
                    };
                if nx < 0 || ny < 0 || nx >= size_i || ny >= size_i {
                    continue;
                }
                let n = (ny * size_i + nx) as usize;
                if ws.mask[n] == 0 || ws.lbl[n] != 0 {
                    continue;
                }
                ws.lbl[n] = comp_id;
                ws.stack[top] = n as i32;
                top += 1;
            }
        }
        if area > best_area {
            best_area = area;
            best_id = comp_id;
            best_cx = sum_x / area as f64;
            best_cy = sum_y / area as f64;
        }
    }
    if best_id == 0 {
        return ArrowResult {
            cyans,
            ..Default::default()
        };
    }

    let bc_dx = best_cx - c0;
    let bc_dy = c0 - best_cy;
    if (bc_dx * bc_dx + bc_dy * bc_dy).sqrt() > r_max * 0.85 {
        return ArrowResult {
            cyans,
            ..Default::default()
        };
    }

    // ---- 72 格径向剖面
    ws.far = [0.0; NB];
    for (i2, lbl) in ws.lbl.iter().enumerate().take(cells) {
        if *lbl != best_id {
            continue;
        }
        let py = (i2 / size) as f64;
        let px = i2 as f64 - py * sxf;
        let ddx = px - c0;
        let ddy = c0 - py;
        let rr = (ddx * ddx + ddy * ddy).sqrt();
        let mut a = ddy.atan2(ddx).to_degrees();
        if a < 0.0 {
            a += 360.0;
        }
        let b = ((a * NB as f64 / 360.0) as usize).min(NB - 1);
        // JS 里 rr 是 double，比较也在 double 上做，只在存进 Float32Array 时取整一次。
        // 先转 f32 再比较会在"几乎相等"的格子上分歧，进而影响候选峰。
        if rr > ws.far[b] as f64 {
            ws.far[b] = rr as f32;
        }
    }
    for i2 in 0..NB {
        let a = ws.far[(i2 + NB - 1) % NB] as f64;
        let b = ws.far[i2] as f64;
        let c = ws.far[(i2 + 1) % NB] as f64;
        ws.sm[i2] = ((a + b * 2.0 + c) / 4.0) as f32; // 与原版同样：double 计算、一次取整
    }
    let peak_max = ws.sm.iter().cloned().fold(0.0f32, f32::max) as f64;
    if peak_max < sxf * 0.06 {
        return ArrowResult {
            cyans,
            ..Default::default()
        };
    }

    let span_of = |idx: usize, far: &[f32; NB]| -> i32 {
        let thr_s = far[idx] * 0.86;
        let mut n = 1i32;
        for s in 1..13usize {
            let l = far[(idx + NB - s) % NB];
            let r = far[(idx + s) % NB];
            let ok_l = l >= thr_s;
            let ok_r = r >= thr_s;
            if !ok_l && !ok_r {
                break;
            }
            if ok_l {
                n += 1;
            }
            if ok_r {
                n += 1;
            }
        }
        n
    };

    // ---- 候选峰
    struct Cand {
        deg: f64,
        far: f64,
        span: i32,
        tx: f64,
        ty: f64,
        tn: u32,
        rmax: f64,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for i2 in 0..NB {
        let v = ws.sm[i2];
        if (v as f64) < peak_max * 0.5 {
            continue;
        }
        if v < ws.sm[(i2 + NB - 1) % NB] || v < ws.sm[(i2 + 1) % NB] {
            continue;
        }
        let a2 = (i2 as f64 * 360.0) / NB as f64;
        if cands.iter().all(|o| ang_diff(o.deg, a2).abs() >= 25.0) {
            cands.push(Cand {
                deg: a2,
                far: v as f64,
                span: span_of(i2, &ws.far),
                tx: 0.0,
                ty: 0.0,
                tn: 0,
                rmax: 0.0,
            });
        }
        if cands.len() >= 6 {
            break;
        }
    }
    if cands.is_empty() {
        return ArrowResult {
            cyans,
            ..Default::default()
        };
    }

    // ---- 精修：平均单位向量
    for (i2, lbl) in ws.lbl.iter().enumerate().take(cells) {
        if *lbl != best_id {
            continue;
        }
        let py = (i2 / size) as f64;
        let px = i2 as f64 - py * sxf;
        let ddx = px - c0;
        let ddy = c0 - py;
        let rr = (ddx * ddx + ddy * ddy).sqrt();
        for c in cands.iter_mut() {
            if rr < c.far * 0.92 {
                continue;
            }
            let mut a = ddy.atan2(ddx).to_degrees();
            if a < 0.0 {
                a += 360.0;
            }
            if ang_diff(a, c.deg).abs() > 16.0 {
                continue;
            }
            c.tx += ddx / rr;
            c.ty += ddy / rr;
            c.tn += 1;
            if rr > c.rmax {
                c.rmax = rr;
            }
        }
    }

    let mut corners: Vec<Corner> = Vec::new();
    for c in &cands {
        if c.tn == 0 {
            continue;
        }
        corners.push(Corner {
            deg: deg_of(c.tx, c.ty),
            far: if c.rmax > 0.0 { c.rmax } else { c.far },
            span: c.span,
        });
    }
    if corners.len() < 2 {
        return ArrowResult {
            cyans,
            ..Default::default()
        };
    }

    // ---- 枚举三点组合
    let mut chosen: Option<Triple> = None;
    if corners.len() >= 3 {
        for i in 0..corners.len() {
            for j in (i + 1)..corners.len() {
                for k in (j + 1)..corners.len() {
                    let trio = [corners[i], corners[j], corners[k]];
                    for m in 0..3 {
                        let others: Vec<Corner> =
                            (0..3).filter(|x| *x != m).map(|x| trio[x]).collect();
                        let r = eval_triple(&trio[m], &others[0], &others[1], params);
                        if chosen.as_ref().is_none_or(|c| r.score > c.score) {
                            chosen = Some(r);
                        }
                    }
                }
            }
        }
        if chosen.as_ref().is_some_and(|c| c.score <= -99.0) {
            chosen = None;
        }
    }

    // ---- 只有两个角：两个都是尾巴时才敢用
    if chosen.as_ref().is_none_or(|c| c.score <= 0.0) && corners.len() == 2 {
        let (p, q) = (corners[0], corners[1]);
        let sep = ang_diff(p.deg, q.deg).abs();
        let r_sym = 1.0 - (1.0f64).min((p.far - q.far).abs() / p.far.max(q.far).max(1.0));
        let both_sharp = p.span <= 4 && q.span <= 4;
        if sep <= 125.0 && r_sym > 0.82 && both_sharp {
            chosen = Some(Triple {
                score: 0.3,
                ang_margin: 8.0,
                rad_margin: 0.0,
                rad_sym: r_sym,
                bis_err: 0.0,
                nose_flat: 0.0,
                nose: None,
                t1: p,
                t2: q,
            });
        }
    }

    let Some(ch) = chosen else {
        return ArrowResult {
            cyans,
            corners,
            ..Default::default()
        };
    };

    let axis_tail = (ch.t1.deg + ang_diff(ch.t2.deg, ch.t1.deg) / 2.0 + 720.0) % 360.0;
    let angle = (axis_tail + 180.0) % 360.0;
    let a1 = deg_of(bc_dx, bc_dy);
    let tip_frac = peak_max / sxf;
    let spread = ang_diff(angle, a1).abs();
    let cover = (best_area as f64 * (step * step) as f64) / (sxf * sxf);

    let ang_conf = (ch.ang_margin / 22.0).clamp(0.0, 1.0);
    let rad_conf = ((ch.rad_margin + 0.08) / 0.3).clamp(0.0, 1.0);
    let size_conf = (cover / 0.05).clamp(0.0, 1.0);
    let flat_conf = ((ch.nose_flat + 0.2) / 1.2).clamp(0.0, 1.0);
    let conf = (ang_conf * 0.3
        + rad_conf * 0.24
        + size_conf * 0.18
        + ch.rad_sym * 0.16
        + flat_conf * 0.12)
        .clamp(0.0, 1.0);

    let valid = corners.len() >= 3
        && ch.ang_margin > params.ang_margin_min
        && ch.rad_margin > -0.12
        && ch.nose_flat >= -0.4
        && ch.score > 0.25
        && tip_frac >= 0.1
        && best_area as f64 * (step * step) as f64 >= 120.0;

    ArrowResult {
        valid,
        angle,
        conf,
        cyans,
        tip_frac,
        spread,
        area: best_area,
        // 原版返回的是**舍入后**的诊断值，这里照做，否则对拍会差 0.005
        ang_margin: js_round(ch.ang_margin * 10.0) / 10.0,
        rad_margin: js_round(ch.rad_margin * 100.0) / 100.0,
        nose_flat: js_round(ch.nose_flat * 100.0) / 100.0,
        corners,
        nose_deg: ch.nose.map(|n| n.deg),
        a1,
        bc_x: best_cx,
        bc_y: best_cy,
    }
}

/// 鼻子是否落在对称轴上（**我们加的**，原版没有这一条）。
///
/// 真箭头是轴对称图形：鼻子必然在中轴上。当小地图背景里另一块青色
/// （水域、任务标）混进同一个连通块时，配对选出的两翼会被拉偏，
/// 于是"鼻子角"和"输出轴角"对不上 —— 实测：
///
/// | 素材 | &#124;鼻子−轴&#124; | 角数 | 误差 |
/// |---|---|---|---|
/// | 合成飞镖 ×30 | 最大 2.5° | 3 | 0 |
/// | 真机干净箭头 | 6.5° | 4 | 1.2° |
/// | 真机箭头+地图背景 | **13.9°** | 5 | **11°** |
///
/// 所以 8° 这条线能挡住被污染的那一帧，同时不误杀前两类。
pub const NOSE_AXIS_MAX_DEG: f64 = 8.0;

/// 这一帧的鼻子是否在中轴上（见 [`NOSE_AXIS_MAX_DEG`]）
pub fn axis_consistent(r: &ArrowResult) -> bool {
    match r.nose_deg {
        Some(n) => ((n - r.angle + 540.0) % 360.0 - 180.0).abs() <= NOSE_AXIS_MAX_DEG,
        None => false,
    }
}

/// `atan2` 角 → 罗盘角（0=北、顺时针）。
///
/// atan2 空间是 0=东、逆时针；罗盘是 0=北、顺时针。两者是**镜像**关系：
/// `compass = (90 - atan2) mod 360`。搞成 `+90` 会让所有方向镜像，
/// 表现就是"走到哪儿都指向反方向"——第一版就是这么错的。
#[inline]
pub fn to_compass(deg: f64) -> f64 {
    (90.0 - deg).rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(size: usize) -> Vec<u8> {
        let mut v = vec![0u8; size * size * 4];
        for p in v.chunks_mut(4) {
            p[0] = 34;
            p[1] = 78;
            p[2] = 62;
            p[3] = 255;
        }
        v
    }

    fn put(v: &mut [u8], size: usize, x: f64, y: f64, rgb: (u8, u8, u8)) {
        let (xi, yi) = (x.round() as i32, y.round() as i32);
        if xi < 0 || yi < 0 || xi >= size as i32 || yi >= size as i32 {
            return;
        }
        let i = ((yi as usize) * size + xi as usize) * 4;
        v[i] = rgb.0;
        v[i + 1] = rgb.1;
        v[i + 2] = rgb.2;
    }

    /// 画一个"钝鼻 + 两个尖尾"的飞镖（与 JS 参考语料同一几何）
    fn draw_dart(v: &mut [u8], size: usize, theta_compass: f64) {
        let c = (size as f64 - 1.0) / 2.0;
        let rad = |d: f64| d * std::f64::consts::PI / 180.0;
        let dir = |d: f64| (rad(d).sin(), -rad(d).cos());
        let rn = size as f64 * 0.17;
        let rt = size as f64 * 0.235;
        let rc = size as f64 * 0.10;
        let p = |deg: f64, r: f64| {
            let (x, y) = dir(deg);
            (c + x * r, c + y * r)
        };
        let poly = [
            p(theta_compass, rn),
            p(theta_compass + 130.0, rt),
            p(theta_compass + 180.0, rc),
            p(theta_compass - 130.0, rt),
        ];
        let inside = |px: f64, py: f64| {
            let mut hit = false;
            let mut j = poly.len() - 1;
            for i in 0..poly.len() {
                let (xi, yi) = poly[i];
                let (xj, yj) = poly[j];
                if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
                    hit = !hit;
                }
                j = i;
            }
            hit
        };
        let r = rt.ceil() as i32 + 2;
        for dy in -r..=r {
            for dx in -r..=r {
                let (px, py) = (c + dx as f64, c + dy as f64);
                if inside(px, py) {
                    put(v, size, px, py, (34, 200, 232));
                }
            }
        }
    }

    fn analyze_dart(theta_compass: f64) -> ArrowResult {
        let size = 200usize;
        let mut frame = blank(size);
        draw_dart(&mut frame, size, theta_compass);
        let mut ws = ArrowWorkspace::default();
        analyze(&frame, size, 1, &mut ws)
    }

    #[test]
    fn blank_frame_has_no_arrow() {
        let size = 160usize;
        let frame = blank(size);
        let mut ws = ArrowWorkspace::default();
        let r = analyze(&frame, size, 1, &mut ws);
        assert!(!r.valid);
        assert_eq!(r.cyans, 0);
        assert_eq!(r.angle, 0.0);
    }

    #[test]
    fn dart_points_where_it_was_drawn() {
        // 画在罗盘 0/90/180/270，算法给 atan2 角，转成罗盘后应一致
        for theta in [0.0, 90.0, 180.0, 270.0] {
            let r = analyze_dart(theta);
            assert!(
                r.valid,
                "theta={theta} 应判有效，实际 corners={:?}",
                r.corners
            );
            let got = to_compass(r.angle);
            let diff = ang_diff(got, theta).abs();
            assert!(
                diff < 8.0,
                "theta={theta} 得到 {got:.1}（差 {diff:.1}°），corners={:?}",
                r.corners
            );
            assert!(r.conf > 0.4, "theta={theta} conf={}", r.conf);
        }
    }

    #[test]
    fn decoy_blobs_do_not_steer_the_arrow() {
        // 远处放三块青色（传送锚点/神瞳），方向不能被它们带跑
        let size = 200usize;
        let mut frame = blank(size);
        draw_dart(&mut frame, size, 45.0);
        let c = (size as f64 - 1.0) / 2.0;
        for a in [160.0f64, 250.0, 330.0] {
            let rad = a * std::f64::consts::PI / 180.0;
            let (bx, by) = (
                c + rad.sin() * size as f64 * 0.4,
                c - rad.cos() * size as f64 * 0.4,
            );
            for dy in -6..=6 {
                for dx in -6..=6 {
                    if dx * dx + dy * dy <= 36 {
                        put(
                            &mut frame,
                            size,
                            bx + dx as f64,
                            by + dy as f64,
                            (34, 200, 232),
                        );
                    }
                }
            }
        }
        let mut ws = ArrowWorkspace::default();
        let r = analyze(&frame, size, 1, &mut ws);
        assert!(r.valid);
        assert!(
            ang_diff(to_compass(r.angle), 45.0).abs() < 8.0,
            "被干扰带偏：{r:?}"
        );
    }
}
