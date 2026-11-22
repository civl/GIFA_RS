//! 六边形法：按用户给定的箭头几何直接定方向（v1 的兜底）。
//!
//! ## 形状定义（用户 2026-11-22 给的）
//!
//! 箭头不是三角形，是**六边形**：
//!
//! ```text
//!          T                 T  尖端（1 个点，在上）
//!         / \                W1 W2 两翼（底部的外凸点）
//!        /   \               B  底边中点（底部第三个外凸点）
//!      W1     W2             N1 N2 两个**向内的**凹点
//!       \ N1N2 /             ────────────────
//!        \ B  /              3 个外凸 + 2 个内凹 + 1 个尖 = 6 边
//! ```
//!
//! ## 为什么这比"三点拟合"稳
//!
//! 三点拟合要在四个角里猜"哪个是鼻子、哪两个是翼"，翼角一被地图上的青色带偏，
//! 方向就整段歪（真机实测偏 118°）。而六边形里**两个凹点是唯一的**——
//! 它们一定在背后，谁也冒充不了。于是：
//!
//! 1. 两个凹点的中点 M 就是"背后"；
//! 2. 离 M 最远的边界点就是尖端 T；
//! 3. 方向 = M → T。
//!
//! 180° 歧义**从定义上就不存在**，不需要额外的翻转判据。
//!
//! ## 实测（两份人工真值）
//!
//! | 素材 | v1 单独 | v1 + 六边形兜底 |
//! |---|---|---|
//! | 视频蒙德 138 帧 | 138/138，0.0° | 138/138，0.0°（没用到兜底） |
//! | 真机绿地 39 帧 | 28/39，9.3° | **39/39，9.2°** |

use std::collections::VecDeque;

/* 试过但**撤掉**的闸：要求两个凹点相对尖端轴对称（差 ≤45°）。
 *
 * 动机：人工真值里"兜底翻错"那帧的对称差是 70°，而好帧 30.5° —— 看着能分。
 * 但那份真值的 39 帧几乎全是**同一帧的重复**（连通块像素数都是 412），
 * 真机连跑 60 秒时这条闸把覆盖率从 99% 砍到 31%（753 帧里只剩 64 帧走兜底）。
 *
 * 结论：单帧样本上调出来的阈值不可信。翻转交给时序层处理（实测吃掉 89%）。
 */

/// 结果
#[derive(Debug, Clone, Copy)]
pub struct HexDirection {
    /// 罗盘角（0=北、顺时针）
    pub deg: f64,
    /// 两个凹点的内凹深度（像素），越大越可信
    pub notch_depth: f64,
    /// 掩膜像素数
    pub area: usize,
}

/// 青色掩膜（与原版同一判据：`min(g,b) − r`，阈值 `max(112, p99×0.74)`）
fn mask_of(rgba: &[u8], size: usize) -> (Vec<u8>, usize) {
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
    for i in 0..n {
        if score[i] as f64 >= thr {
            mask[i] = 1;
        }
    }
    (mask, thr as usize)
}

/// 最大连通块（8 邻域）
fn largest_blob(mask: &[u8], size: usize) -> Vec<usize> {
    let n = size * size;
    let mut seen = vec![false; n];
    let mut best: Vec<usize> = Vec::new();
    let mut q: VecDeque<usize> = VecDeque::new();
    for seed in 0..n {
        if mask[seed] == 0 || seen[seed] {
            continue;
        }
        q.clear();
        q.push_back(seed);
        seen[seed] = true;
        let mut cur_blob = Vec::new();
        while let Some(cur) = q.pop_front() {
            cur_blob.push(cur);
            let y = cur / size;
            let x = cur % size;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (ny, nx) = (y as i32 + dy, x as i32 + dx);
                    if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                        continue;
                    }
                    let k = ny as usize * size + nx as usize;
                    if mask[k] != 0 && !seen[k] {
                        seen[k] = true;
                        q.push_back(k);
                    }
                }
            }
        }
        if cur_blob.len() > best.len() {
            best = cur_blob;
        }
    }
    best
}

/// 凸包（Andrew monotone chain）
fn convex_hull(mut pts: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lo: Vec<(f64, f64)> = Vec::new();
    for &p in &pts {
        while lo.len() >= 2 && cross(lo[lo.len() - 2], lo[lo.len() - 1], p) <= 0.0 {
            lo.pop();
        }
        lo.push(p);
    }
    let mut up: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while up.len() >= 2 && cross(up[up.len() - 2], up[up.len() - 1], p) <= 0.0 {
            up.pop();
        }
        up.push(p);
    }
    lo.pop();
    up.pop();
    lo.extend(up);
    lo
}

/// 点到凸多边形边界的距离（点在多边形内）
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

/// 六边形法求方向。返回 `None` = 这块掩膜不像箭头（找不到两个明确的内凹点）。
pub fn direction(rgba: &[u8], size: usize) -> Option<HexDirection> {
    if size < 24 || rgba.len() < size * size * 4 {
        return None;
    }
    let (mask, _thr) = mask_of(rgba, size);
    let blob = largest_blob(&mask, size);
    if blob.len() < 80 {
        return None;
    }
    // 边界点（数学坐标：x 右、y 上）
    let set: std::collections::HashSet<usize> = blob.iter().copied().collect();
    let mut boundary: Vec<(f64, f64)> = Vec::new();
    for &cur in &blob {
        let y = cur / size;
        let x = cur % size;
        let mut edge = false;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (ny, nx) = (y as i32 + dy, x as i32 + dx);
                if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                    edge = true;
                    break;
                }
                if !set.contains(&(ny as usize * size + nx as usize)) {
                    edge = true;
                    break;
                }
            }
            if edge {
                break;
            }
        }
        if edge {
            boundary.push((x as f64, (size - 1 - y) as f64));
        }
    }
    if boundary.len() < 12 {
        return None;
    }
    let hull = convex_hull(boundary.clone());
    if hull.len() < 3 {
        return None;
    }

    // 两个内凹点：离凸包最远、且彼此分得开
    let mut scored: Vec<(f64, (f64, f64))> = boundary
        .iter()
        .map(|&p| (dist_to_hull(&hull, p), p))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let n1 = scored.first()?.1;
    let mut n2 = None;
    for (_, p) in scored.iter().skip(1) {
        if ((p.0 - n1.0).powi(2) + (p.1 - n1.1).powi(2)).sqrt() > size as f64 * 0.10 {
            n2 = Some(*p);
            break;
        }
    }
    let n2 = n2?;
    let notch_depth = 0.5 * (scored[0].0 + scored[1].0);

    let m = ((n1.0 + n2.0) / 2.0, (n1.1 + n2.1) / 2.0);
    // 离凹口中点最远的边界点 = 尖端
    let mut best = (0.0f64, m);
    for &p in &boundary {
        let d = (p.0 - m.0).powi(2) + (p.1 - m.1).powi(2);
        if d > best.0 {
            best = (d, p);
        }
    }
    let tip = best.1;
    let (dx, dy) = (tip.0 - m.0, tip.1 - m.1);
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return None;
    }

    let deg = (90.0 - dy.atan2(dx).to_degrees()).rem_euclid(360.0);
    Some(HexDirection {
        deg,
        notch_depth,
        area: blob.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 用户给的那枚 36×36 箭头：尖端朝上（罗盘 ≈ 0/360）
    #[test]
    fn points_north_on_sample_arrow() {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/realgame/sample-arrow-36.bin");
        if !p.exists() {
            eprintln!("跳过：没有 sample-arrow-36.bin");
            return;
        }
        let b = std::fs::read(&p).expect("读样例箭头");
        let d = direction(&b, 36).expect("应能求出方向");
        let err = ((d.deg - 0.0 + 540.0) % 360.0 - 180.0).abs();
        assert!(err < 10.0, "尖头朝上，实际 {:.1}°（差 {err:.1}°）", d.deg);
    }

    /// 真机绿地帧：全部能出方向，且都在合理范围内（不要求逐帧精确，靠 v1 保精度）
    #[test]
    fn covers_realgame_frames() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/realgame");
        let mut n = 0;
        let mut ok = 0;
        for e in std::fs::read_dir(&dir).expect("realgame") {
            let path = e.expect("entry").path();
            if path.extension().and_then(|x| x.to_str()) != Some("bin") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("sample-") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("读帧");
            let side = (bytes.len() / 4).isqrt();
            n += 1;
            if direction(&bytes, side).is_some() {
                ok += 1;
            }
        }
        assert!(n >= 5, "真机素材少于 5 帧");
        assert_eq!(ok, n, "六边形法应对每一帧都能给出方向（{ok}/{n}）");
    }
}
