//! 箭头方向识别 v2：凸包 + 凹口定位。
//!
//! ## 为什么不用"三角形三个顶点"
//!
//! 真机箭头（2560×1440 实测：尖端 17.5、两翼 16.6/16.7、底边中点 12.6）近似**等边**，
//! 三个顶点谁当"尖"、谁当"翼"，光看形状分不出来 —— 模板拟合的 IoU 对三种分派几乎一样高，
//! 方向就随机飘（实测给出 211°、334°）。所以 v1 那种"枚举三点 + 硬约束"的路子在真箭头上
//! 本质不可靠，不只是阈值问题。
//!
//! ## v2 的判据
//!
//! 真正能把方向钉死的是**凹口**：底边中点比两翼更靠近中心（12.6 < 16.6），
//! 也就是说整块掩膜**唯一的内凹点**就在"背后"。于是：
//!
//! 1. 取最大青色连通块，求它的**凸包**；
//! 2. 在边界点里找"离凸包最远"的那个 —— 那就是凹口（内凹深度 ≥ 0.08R 才算数）；
//! 3. 方向 = 质心 → 凹口的反方向；
//! 4. 再用对称轴在 ±15° 内精修（真箭头是轴对称图形）。
//!
//! 纯凸的形状（比如一团青色圆形）凹口深度≈0，直接判不可用 —— 这是诚实的：
//! 没有凹口就没有方向信息，宁可不上报。

/// 两个模型允许的最大分歧。实测：干净帧 ~10°，被背景污染的帧 ~35°。
pub const MAX_DISAGREE_DEG: f64 = 16.0;

/// 拟合结果
#[derive(Debug, Clone, Copy, Default)]
pub struct Fit {
    /// 尖端朝向（`atan2` 角：0=东、逆时针）
    pub angle: f64,
    /// 凹口内凹深度 / 尖端半径（越大越像箭头）
    pub notch_depth: f64,
    /// 凸包面积 / 掩膜面积（越接近 1 越凸；箭头约 1.02~1.15）
    pub solidity: f64,
    /// 尖端半径（像素）
    pub radius: f64,
}

#[inline]
fn ang_of(dx: f64, dy: f64) -> f64 {
    (dy.atan2(dx).to_degrees() + 360.0) % 360.0
}

/// 凸包（Andrew monotone chain），点已按 (x,y) 排序
fn convex_hull(mut pts: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// 点到凸多边形边界的距离（点在内部，取到各边的最小距离）
fn dist_to_hull(hull: &[(f64, f64)], p: (f64, f64)) -> f64 {
    let mut best = f64::MAX;
    let n = hull.len();
    for i in 0..n {
        let a = hull[i];
        let b = hull[(i + 1) % n];
        let (abx, aby) = (b.0 - a.0, b.1 - a.1);
        let len2 = abx * abx + aby * aby;
        let t = if len2 <= 1e-12 {
            0.0
        } else {
            (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0)
        };
        let (qx, qy) = (a.0 + abx * t, a.1 + aby * t);
        best = best.min(((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt());
    }
    best
}

fn poly_area(poly: &[(f64, f64)]) -> f64 {
    let mut a = 0.0;
    let n = poly.len();
    for i in 0..n {
        let (x1, y1) = poly[i];
        let (x2, y2) = poly[(i + 1) % n];
        a += x1 * y2 - x2 * y1;
    }
    (a / 2.0).abs()
}

/// 青色掩膜（与原版同一判据：`min(g,b) − r`，自适应阈值）
fn cyan_mask(rgba: &[u8], size: usize) -> (Vec<u8>, f64) {
    let n = size * size;
    let mut score = vec![0i16; n];
    let mut hist = [0u32; 256];
    for i in 0..n {
        let r = rgba[i * 4] as i32;
        let g = rgba[i * 4 + 1] as i32;
        let b = rgba[i * 4 + 2] as i32;
        let s = (g.min(b) - r).clamp(-255, 255);
        score[i] = s as i16;
        if s >= 0 {
            hist[s as usize] += 1;
        }
    }
    let total: u32 = hist.iter().sum();
    let mut acc = 0u32;
    let mut p99 = 0i32;
    for (s, n) in hist.iter().enumerate() {
        acc += n;
        if acc as f64 >= total as f64 * 0.99 {
            p99 = s as i32;
            break;
        }
    }
    let thr = ((p99 as f64) * 0.74).max(112.0);
    let mut mask = vec![0u8; n];
    let mut hits = 0u32;
    for i in 0..n {
        if score[i] as f64 >= thr {
            mask[i] = 1;
            hits += 1;
        }
    }
    (mask, hits as f64 / n as f64)
}

fn largest_blob(mask: &[u8], size: usize) -> Vec<usize> {
    let n = size * size;
    let mut seen = vec![false; n];
    let mut best: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for seed in 0..n {
        if mask[seed] == 0 || seen[seed] {
            continue;
        }
        stack.clear();
        stack.push(seed);
        seen[seed] = true;
        let mut blob = Vec::new();
        while let Some(cur) = stack.pop() {
            blob.push(cur);
            let y = cur / size;
            let x = cur % size;
            for yy in y.saturating_sub(1)..=(y + 1).min(size - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(size - 1) {
                    let k = yy * size + xx;
                    if mask[k] != 0 && !seen[k] {
                        seen[k] = true;
                        stack.push(k);
                    }
                }
            }
        }
        if blob.len() > best.len() {
            best = blob;
        }
    }
    best
}

/// 在掩膜上求箭头方向。返回 `None` = 这一帧看不出方向（没有凹口 / 面积太小）。
pub fn fit_arrow(rgba: &[u8], size: usize) -> Option<Fit> {
    if size < 24 || rgba.len() < size * size * 4 {
        return None;
    }
    let (mask, cyan_frac) = cyan_mask(rgba, size);
    if cyan_frac < 0.002 {
        return None;
    }
    let blob = largest_blob(&mask, size);
    if blob.len() < 60 {
        return None;
    }

    // 质心（数学坐标）
    let (mut cx, mut cy) = (0.0f64, 0.0f64);
    for &i in &blob {
        cx += (i % size) as f64;
        cy += (size - 1 - i / size) as f64;
    }
    cx /= blob.len() as f64;
    cy /= blob.len() as f64;

    // 边界点（有非掩膜邻居的像素）
    let mut boundary: Vec<(f64, f64)> = Vec::new();
    for &i in &blob {
        let y = i / size;
        let x = i % size;
        let mut edge = false;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                    edge = true;
                    continue;
                }
                if mask[ny as usize * size + nx as usize] == 0 {
                    edge = true;
                }
            }
        }
        if edge {
            boundary.push((x as f64 - cx, (size - 1 - y) as f64 - cy));
        }
    }
    if boundary.len() < 12 {
        return None;
    }
    let hull = convex_hull(boundary.clone());
    if hull.len() < 3 {
        return None;
    }

    // 凹口 = 离凸包边界最远的边界点（唯一的内凹处）
    let mut notch = (0.0f64, 0.0f64, 0.0f64);
    for &p in &boundary {
        let d = dist_to_hull(&hull, p);
        if d > notch.2 {
            notch = (p.0, p.1, d);
        }
    }

    let radii: Vec<f64> = blob
        .iter()
        .map(|&i| {
            let dx = (i % size) as f64 - cx;
            let dy = (size - 1 - i / size) as f64 - cy;
            (dx * dx + dy * dy).sqrt()
        })
        .collect();
    let radius = radii.iter().cloned().fold(0.0f64, f64::max);
    if radius < 4.0 {
        return None;
    }

    // 方向 = 凹口的反方向（先给个粗值，再用对称性精修）
    let notch_ang = ang_of(notch.0, notch.1);
    let mut angle = (notch_ang + 180.0) % 360.0;

    /* 对称轴精修：在 ±25° 内找让**极坐标占据图**左右最对称的轴。
     * 用占据（0/1）而不是半径加权：加权会让远端少量像素主导，实测偏 18°。 */
    let bins = 360usize;
    let rings = radius.ceil().max(2.0) as usize;
    let mut grid = vec![0f32; rings * bins];
    for &i in &blob {
        let dx = (i % size) as f64 - cx;
        let dy = (size - 1 - i / size) as f64 - cy;
        let r = (dx * dx + dy * dy).sqrt();
        let ri = r as usize;
        if ri >= rings {
            continue;
        }
        let k = ((ang_of(dx, dy) / 360.0 * bins as f64) as usize).min(bins - 1);
        grid[ri * bins + k] = 1.0;
    }
    let sym_at = |axis: f64| -> f64 {
        let mut inter = 0.0f64;
        let mut uni = 0.0f64;
        for ri in 0..rings {
            let row = &grid[ri * bins..(ri + 1) * bins];
            let mut cnt = 0.0f64;
            for &v in row {
                cnt += v as f64;
            }
            if cnt < 2.0 {
                continue;
            }
            for k in 0..bins {
                if row[k] == 0.0 {
                    continue;
                }
                let a = k as f64 * 360.0 / bins as f64;
                let mk = ((2.0 * axis - a).rem_euclid(360.0) * bins as f64 / 360.0) as usize % bins;
                inter += (row[k].min(row[mk])) as f64;
                uni += (row[k].max(row[mk])) as f64;
            }
        }
        if uni > 0.0 {
            inter / uni
        } else {
            0.0
        }
    };
    let mut best_axis = angle;
    let mut best_sym = -1.0f64;
    for d in -25..=25 {
        let axis = (angle + d as f64 + 360.0) % 360.0;
        let sym = sym_at(axis);
        if sym > best_sym {
            best_sym = sym;
            best_axis = axis;
        }
    }
    angle = best_axis;

    let solidity = if poly_area(&hull) > 0.0 {
        blob.len() as f64 / poly_area(&hull)
    } else {
        0.0
    };
    let notch_depth = notch.2 / radius;
    // 凹口太浅 ≈ 纯凸形状，没有方向信息
    if notch_depth < 0.03 {
        return None;
    }
    Some(Fit {
        angle,
        notch_depth,
        solidity,
        radius,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn load(name: &str) -> (Vec<u8>, usize) {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let b = std::fs::read(&p).unwrap_or_else(|e| panic!("读 {}: {e}", p.display()));
        let side = (b.len() / 4).isqrt();
        (b, side)
    }

    #[test]
    fn report_fits() {
        for name in ["real-arrow-clean-96.bin", "real-minimap-112.bin"] {
            let (data, side) = load(name);
            match fit_arrow(&data, side) {
                Some(f) => println!(
                    "{name}: angle={:.1} notch={:.3} solidity={:.2} r={:.1}",
                    f.angle, f.notch_depth, f.solidity, f.radius
                ),
                None => println!("{name}: 无结果（无凹口）"),
            }
        }
    }
}
