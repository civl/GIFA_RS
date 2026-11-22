//! 视锥通道：忠实移植原版 `analyzeCone()`（`webview-preload.js:1008-1358`）。
//!
//! 它认的是小地图上那层**半透明白膜**：`dst = src + α·(255 − src)`，α 随半径衰减。
//! 所以整条链路是：
//!
//! ```text
//! 极坐标格(120×12) → 不透明白核剔除+膨胀 → 每格 RGB 累加
//!   → 逐角度找「半径边」：两侧各 2 格取均值，三通道联合最小二乘解 α，
//!     并要求三通道解出**同一个 α**（水→陆这种底色边界会在残差上判 0）
//!   → 可见层严格多数（≥5 层）才算一条边
//!   → 配对：上升边在 −45°、下降边在 +45°，张角恒 90°；单边也允许
//!   → 亚格细化（±2 格强度质心）→ 中轴 → 角度
//! ```
//!
//! **坐标约定**：`deg` 与原版一致，是 `atan2` 角（0=东、逆时针）。

const NA: usize = 120; // 3° 一格
const NR: usize = 12; // 径向 12 层
const SIDE: usize = 2;
const A_MAX: f64 = 0.45;
const MIN_SUP: usize = 5;
const HALF: f64 = 15.0; // 45° / 3°

#[derive(Debug, Clone, Copy, Default)]
pub struct ConeResult {
    pub valid: bool,
    /// 视角方向（`atan2` 角）
    pub deg: f64,
    pub conf: f64,
    pub width: f64,
    pub cyan_frac: f64,
}

#[derive(Default)]
pub struct ConeWorkspace {
    /// 采样点 → 极坐标格（只和 size/step 有关）
    cell: Vec<i16>,
    mask: Vec<u8>,
    tmp: Vec<u8>,
    white_core: Vec<u8>,
    white_queue: Vec<i32>,
    sr: Vec<f32>,
    sg: Vec<f32>,
    sb: Vec<f32>,
    cnt: Vec<u16>,
    all: Vec<u16>,
    prof: Vec<f32>,
    nx: usize,
    key: usize,
}

/// JS 的 `Math.round` = `floor(x + 0.5)`（Rust 的 round 在 .5 上是远离零舍入）
#[inline]
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

impl ConeWorkspace {
    fn ensure(&mut self, size: usize, step: usize) {
        let nx = size.div_ceil(step);
        let key = size * 8 + step;
        if self.key == key && self.nx == nx {
            return;
        }
        self.key = key;
        self.nx = nx;
        let n = nx * nx;
        self.cell = vec![-1i16; n];
        let c0 = (size as f64 - 1.0) / 2.0;
        let r_in = size as f64 * 0.16;
        let r_out = size as f64 * 0.48;
        let inv = NR as f64 / (r_out - r_in);
        for gy in 0..nx {
            let dy = c0 - (gy * step) as f64;
            for gx in 0..nx {
                let dx = (gx * step) as f64 - c0;
                let rho = (dx * dx + dy * dy).sqrt();
                if rho < r_in || rho >= r_out {
                    continue;
                }
                let mut a = dy.atan2(dx).to_degrees();
                if a < 0.0 {
                    a += 360.0;
                }
                let k = ((a * NA as f64 / 360.0) as usize).min(NA - 1);
                let j = (((rho - r_in) * inv) as usize).min(NR - 1);
                self.cell[gy * nx + gx] = (j * NA + k) as i16;
            }
        }
        self.mask = vec![0; n];
        self.tmp = vec![0; n];
        self.white_core = vec![0; n];
        self.white_queue = vec![0; n];
        self.sr = vec![0.0; NA * NR];
        self.sg = vec![0.0; NA * NR];
        self.sb = vec![0.0; NA * NR];
        self.cnt = vec![0; NA * NR];
        self.all = vec![0; NA * NR];
        self.prof = vec![0.0; NA];
    }
}

/// 跑一帧的视锥通道。
pub fn analyze_cone(rgba: &[u8], size: usize, step: usize, ws: &mut ConeWorkspace) -> ConeResult {
    if size < 32 || rgba.len() < size * size * 4 {
        return ConeResult::default();
    }
    let step = step.max(1);
    ws.ensure(size, step);
    let nx = ws.nx;
    let n = nx * nx;

    ws.mask.fill(0);
    ws.tmp.fill(0);
    ws.sr.fill(0.0);
    ws.sg.fill(0.0);
    ws.sb.fill(0.0);
    ws.cnt.fill(0);
    ws.all.fill(0);
    ws.prof.fill(0.0);

    // ---- 第一遍：青色占比 + 不透明白核
    let mut cyan_hits = 0u32;
    let mut white_count = 0u32;
    let mut pale_count = 0u32;
    for gy in 0..nx {
        let row = gy * step * size;
        let o = gy * nx;
        for gx in 0..nx {
            let i = (row + gx * step) * 4;
            let (r, g, b) = (rgba[i], rgba[i + 1], rgba[i + 2]);
            if g.min(b) as i32 - r as i32 >= 80 {
                cyan_hits += 1;
            }
            if r >= 232 && g >= 232 && b >= 232 {
                pale_count += 1;
            }
            if r >= 236 && g >= 236 && b >= 236 && (r >= 244 || g >= 244 || b >= 244) {
                ws.tmp[o + gx] = 1;
                white_count += 1;
            }
        }
    }
    let cyan_frac = cyan_hits as f64 / n as f64;
    let pale_background = pale_count as usize > n / 2;
    if pale_background {
        white_count = 0;
        for gy in 0..nx {
            for gx in 0..nx {
                let off = (gy * step * size + gx * step) * 4;
                let white = rgba[off] >= 252 && rgba[off + 1] >= 252 && rgba[off + 2] >= 252;
                ws.tmp[gy * nx + gx] = u8::from(white);
                if white {
                    white_count += 1;
                }
            }
        }
    }

    // ---- 只膨胀紧凑白核，不把雪地当图标
    ws.white_core.fill(0);
    let max_white_area = 12usize.max(n * 6 / 100);
    if white_count as usize <= max_white_area {
        ws.white_core.copy_from_slice(&ws.tmp);
    } else {
        let mut tmp = std::mem::take(&mut ws.tmp);
        let mut queue = std::mem::take(&mut ws.white_queue);
        let mut keep = std::mem::take(&mut ws.white_core);
        for seed in 0..tmp.len() {
            if tmp[seed] == 0 {
                continue;
            }
            tmp[seed] = 0;
            queue[0] = seed as i32;
            let mut count = 1usize;
            let mut head = 0usize;
            while head < count {
                let cur = queue[head] as usize;
                head += 1;
                let row = cur / nx;
                let col = cur - row * nx;
                let y0 = row.saturating_sub(1);
                let y1 = (row + 1).min(nx - 1);
                let x0 = col.saturating_sub(1);
                let x1 = (col + 1).min(nx - 1);
                for yy in y0..=y1 {
                    for xx in x0..=x1 {
                        let nb = yy * nx + xx;
                        if tmp[nb] == 0 {
                            continue;
                        }
                        tmp[nb] = 0;
                        queue[count] = nb as i32;
                        count += 1;
                    }
                }
            }
            if count <= max_white_area {
                for index in 0..count {
                    keep[queue[index] as usize] = 1;
                }
            }
        }
        ws.tmp = tmp;
        ws.white_queue = queue;
        ws.white_core = keep;
    }

    // ---- 膨胀：先按行，再按列（列方向顺带把要排除的格子标成 2）
    let d = 1usize.max(js_round(size as f64 / 90.0 / step as f64) as usize);
    for gy in 0..nx {
        let o = gy * nx;
        let mut last: i64 = i64::MIN / 2;
        for gx in 0..nx {
            if ws.white_core[o + gx] != 0 {
                last = gx as i64;
            }
            if gx as i64 - last <= d as i64 {
                ws.mask[o + gx] = 1;
            }
        }
        last = i64::MAX / 2;
        for gx in (0..nx).rev() {
            if ws.white_core[o + gx] != 0 {
                last = gx as i64;
            }
            if last - gx as i64 <= d as i64 {
                ws.mask[o + gx] = 1;
            }
        }
    }
    for gx in 0..nx {
        let mut last: i64 = i64::MIN / 2;
        for gy in 0..nx {
            if ws.mask[gy * nx + gx] == 1 {
                last = gy as i64;
            }
            if gy as i64 - last <= d as i64 {
                ws.tmp[gy * nx + gx] = 2;
            }
        }
        last = i64::MAX / 2;
        for gy in (0..nx).rev() {
            if ws.mask[gy * nx + gx] == 1 {
                last = gy as i64;
            }
            if last - gy as i64 <= d as i64 {
                ws.tmp[gy * nx + gx] = 2;
            }
        }
    }

    // ---- 第二遍：按极坐标格累加 RGB（被图标盖住的格子不进 RGB）
    for gy in 0..nx {
        let row = gy * step * size;
        let o = gy * nx;
        for gx in 0..nx {
            let c = ws.cell[o + gx];
            if c < 0 {
                continue;
            }
            let c = c as usize;
            ws.all[c] = ws.all[c].wrapping_add(1);
            if ws.tmp[o + gx] == 2 {
                continue;
            }
            let i = (row + gx * step) * 4;
            ws.sr[c] += rgba[i] as f32;
            ws.sg[c] += rgba[i + 1] as f32;
            ws.sb[c] += rgba[i + 2] as f32;
            ws.cnt[c] = ws.cnt[c].wrapping_add(1);
        }
    }

    // ---- 逐角度的半径边剖面
    let mut vals: Vec<f64> = Vec::with_capacity(NR);
    for k in 0..NA {
        vals.clear();
        for j in 0..NR {
            let (mut lr, mut lg, mut lb) = (0f64, 0f64, 0f64);
            let (mut rr, mut rg, mut rb) = (0f64, 0f64, 0f64);
            let (mut ln, mut rn) = (0i64, 0i64);
            let (mut la, mut ra) = (0i64, 0i64);
            for s in 1..=SIDE {
                let cl = j * NA + ((k + NA - s) % NA);
                let cr = j * NA + ((k + s) % NA);
                lr += ws.sr[cl] as f64;
                lg += ws.sg[cl] as f64;
                lb += ws.sb[cl] as f64;
                ln += ws.cnt[cl] as i64;
                la += ws.all[cl] as i64;
                rr += ws.sr[cr] as f64;
                rg += ws.sg[cr] as f64;
                rb += ws.sb[cr] as f64;
                rn += ws.cnt[cr] as i64;
                ra += ws.all[cr] as i64;
            }
            if ln < 2 || rn < 2 || ln * 2 < la || rn * 2 < ra {
                continue;
            }
            lr /= ln as f64;
            lg /= ln as f64;
            lb /= ln as f64;
            rr /= rn as f64;
            rg /= rn as f64;
            rb /= rn as f64;
            let sign = if rr + rg + rb >= lr + lg + lb { 1.0 } else { -1.0 };
            let (or, og, ob) = if sign > 0.0 { (lr, lg, lb) } else { (rr, rg, rb) };
            let (hr, hg, hb) = if sign > 0.0 { (rr, rg, rb) } else { (lr, lg, lb) };
            let (big_h_r, big_h_g, big_h_b) = (255.0 - or, 255.0 - og, 255.0 - ob);
            let den = big_h_r * big_h_r + big_h_g * big_h_g + big_h_b * big_h_b;
            let near_white = big_h_r.max(big_h_g).max(big_h_b) < 18.0;
            if den < 300.0 && (!near_white || den < 108.0) {
                continue;
            }
            let a = ((hr - or) * big_h_r + (hg - og) * big_h_g + (hb - ob) * big_h_b) / den;
            if near_white
                && (hr - or).abs().max((hg - og).abs()).max((hb - ob).abs()) < 1.5
            {
                continue;
            }
            if a > A_MAX {
                continue;
            }
            let mut v = 0.0;
            if (hr - or - a * big_h_r).abs() <= 4.0 + 0.3 * a * big_h_r
                && (hg - og - a * big_h_g).abs() <= 4.0 + 0.3 * a * big_h_g
                && (hb - ob - a * big_h_b).abs() <= 4.0 + 0.3 * a * big_h_b
            {
                v = sign * a;
            }
            vals.push(v);
        }
        let need = MIN_SUP.max((vals.len() >> 1) + 1);
        if vals.len() < need {
            continue;
        }
        vals.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let up = vals[need - 1];
        let dn = vals[vals.len() - need];
        ws.prof[k] = (if up > 0.0 {
            up
        } else if dn < 0.0 {
            dn
        } else {
            0.0
        }) as f32;
    }

    // ---- 配对：上升边 −45°、下降边 +45°
    let rise = |k: i64| -> f64 {
        let idx = k.rem_euclid(NA as i64) as usize;
        let v = ws.prof[idx] as f64;
        if v > 0.0 {
            v
        } else {
            0.0
        }
    };
    let fall = |k: i64| -> f64 {
        let idx = k.rem_euclid(NA as i64) as usize;
        let v = ws.prof[idx] as f64;
        if v < 0.0 {
            -v
        } else {
            0.0
        }
    };
    let mut best: Option<(f64, f64, f64, f64)> = None; // (c, score, rise, fall)
    for c2 in 0..(NA * 2) {
        let c = c2 as f64 / 2.0;
        let (r1, f1) = if c2 % 2 == 0 {
            (rise(c as i64 - HALF as i64), fall(c as i64 + HALF as i64))
        } else {
            (
                rise((c - 0.5 - HALF) as i64).max(rise((c + 0.5 - HALF) as i64)),
                fall((c - 0.5 + HALF) as i64).max(fall((c + 0.5 + HALF) as i64)),
            )
        };
        let score = r1 + f1 + 0.5 * r1.min(f1);
        if best.is_none_or(|b| score > b.1) {
            best = Some((c, score, r1, f1));
        }
    }
    let Some((bc, score, b_rise, b_fall)) = best else {
        return ConeResult {
            cyan_frac,
            width: 90.0,
            ..Default::default()
        };
    };
    if score < 0.11 {
        return ConeResult {
            cyan_frac,
            width: 90.0,
            ..Default::default()
        };
    }

    // 亚格细化
    let refine = |center: f64, f: &dyn Fn(i64) -> f64| -> Option<f64> {
        let k0 = js_round(center);
        let mut w = 0.0f64;
        let mut s = 0.0f64;
        for dd in -2..=2i64 {
            let v = f(k0 as i64 + dd);
            w += v;
            s += v * (k0 + dd as f64);
        }
        if w > 1e-6 {
            Some(s / w)
        } else {
            None
        }
    };
    let e_r = if b_rise > 0.02 {
        refine(bc - HALF, &rise)
    } else {
        None
    };
    let e_f = if b_fall > 0.02 {
        refine(bc + HALF, &fall)
    } else {
        None
    };
    let axis = match (e_r, e_f) {
        (Some(r), Some(f)) => {
            ((r + HALF) * b_rise + (f - HALF) * b_fall) / (b_rise + b_fall)
        }
        (Some(r), None) => r + HALF,
        (None, Some(f)) => f - HALF,
        (None, None) => bc,
    };
    let to_deg = |bin: f64| {
        let v = ((bin + 0.5) * 360.0) / NA as f64;
        v.rem_euclid(360.0)
    };
    let conf = ((score - 0.06) / 0.34).clamp(0.0, 1.0);
    ConeResult {
        valid: conf > 0.12,
        deg: to_deg(axis),
        conf,
        width: 90.0,
        cyan_frac,
    }
}

/// `atan2` 角 → 罗盘角
#[inline]
pub fn to_compass(deg: f64) -> f64 {
    (90.0 - deg).rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 地形 + 半透明白膜扇形（张角 90°，α 随半径衰减）
    fn frame_with_cone(size: usize, phi_compass: f64) -> Vec<u8> {
        let mut v = vec![0u8; size * size * 4];
        let c = (size as f64 - 1.0) / 2.0;
        let rad = |d: f64| d * std::f64::consts::PI / 180.0;
        for y in 0..size {
            for x in 0..size {
                let i = (y * size + x) * 4;
                v[i] = 40;
                v[i + 1] = 93;
                v[i + 2] = 103;
                v[i + 3] = 255;
            }
        }
        let (ux, uy) = (rad(phi_compass).sin(), -rad(phi_compass).cos());
        let r_in = size as f64 * 0.16;
        let r_out = size as f64 * 0.48;
        for y in 0..size {
            for x in 0..size {
                let dx = x as f64 - c;
                let dy = y as f64 - c;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < r_in || dist > r_out {
                    continue;
                }
                let cosang = (dx * ux + dy * uy) / dist.max(1e-9);
                if cosang.acos() > rad(45.0) {
                    continue;
                }
                let i = (y * size + x) * 4;
                let a = 0.35 - 0.25 * ((dist - r_in) / (r_out - r_in));
                for k in 0..3 {
                    let src = v[i + k] as f64;
                    v[i + k] = (src + a * (255.0 - src)).min(255.0) as u8;
                }
            }
        }
        v
    }

    #[test]
    fn finds_cone_direction() {
        for phi in [0.0, 90.0, 180.0, 270.0] {
            let size = 200usize;
            let frame = frame_with_cone(size, phi);
            let mut ws = ConeWorkspace::default();
            let r = analyze_cone(&frame, size, 1, &mut ws);
            assert!(r.valid, "phi={phi} 应判有效（conf={}）", r.conf);
            let got = to_compass(r.deg);
            let diff = ((got - phi + 540.0) % 360.0) - 180.0;
            assert!(
                diff.abs() < 6.0,
                "phi={phi} 得到罗盘 {got:.1}（差 {diff:.1}°）"
            );
            assert!(r.cyan_frac < 0.01, "没有青色，cyanFrac 应接近 0");
        }
    }

    #[test]
    fn flat_terrain_has_no_cone() {
        let size = 200usize;
        let frame = vec![40u8; size * size * 4]
            .chunks_mut(4)
            .flat_map(|p| {
                p[0] = 40;
                p[1] = 93;
                p[2] = 103;
                p[3] = 255;
                [p[0], p[1], p[2], p[3]]
            })
            .collect::<Vec<u8>>();
        let mut ws = ConeWorkspace::default();
        let r = analyze_cone(&frame, size, 1, &mut ws);
        assert!(!r.valid, "纯地形不该报视角");
    }
}
