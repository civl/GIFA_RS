//! 重试策略：主判据失败时，换几个偏移再试一遍，用**聚类投票**决定要不要改判。
//!
//! 移植自 `webview-preload.js:1569-1614`（`centeredPatch` / `analyzeArrowWithRecovery`
//! / `analyzeConeWithRecovery`）。
//!
//! ## 为什么需要它
//!
//! 箭头贴着窗口边缘时（ROI 标得略偏），主判据会因为采样不全而判失败。做法是：
//! 把窗口往里缩 2.5%，再往 8 个方向各偏移一次，每个都跑一遍检测；把结果按角度聚成簇，
//! 只有"至少 3 个、且占多数"的簇才被采纳。
//!
//! 单次重试很容易被噪声带偏，**簇投票**才是重点：孤立的伪峰凑不出 3 票。

use crate::arrow::{analyze_with, ang_diff, ArrowParams, ArrowResult, ArrowWorkspace};
use crate::cone::{analyze_cone, ConeResult, ConeWorkspace};

/// 从 `size×size` 的中心裁出 `width×width`，再平移 `(dx, dy)`。
pub fn centered_patch(rgba: &[u8], size: usize, width: usize, dx: i32, dy: i32) -> Option<Vec<u8>> {
    if width > size || size * size * 4 > rgba.len() {
        return None;
    }
    // JS 的 Math.round 是 floor(x+0.5)
    let base = ((size - width) as f64 / 2.0 + 0.5).floor() as i32;
    let x = base + dx;
    let y = base + dy;
    if x < 0 || y < 0 || (x as usize + width) > size || (y as usize + width) > size {
        return None;
    }
    let row_bytes = width * 4;
    let mut out = vec![0u8; width * width * 4];
    for row in 0..width {
        let src = ((y as usize + row) * size + x as usize) * 4;
        out[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(&rgba[src..src + row_bytes]);
    }
    Some(out)
}

/// 箭头 + 重试
pub fn analyze_arrow_with_recovery(
    rgba: &[u8],
    size: usize,
    step: usize,
    ws: &mut ArrowWorkspace,
    params: &ArrowParams,
) -> ArrowResult {
    let base = analyze_with(rgba, size, step, ws, params);
    // 已经认出来、窗口太小、或者青色本来就很少 → 不重试
    if base.valid
        || size < 100
        || (base.cyans as f64 * (step * step) as f64) < (size * size) as f64 * 0.025
    {
        return base;
    }
    let shift = ((size as f64 * 0.025) + 0.5).floor() as i32;
    if shift <= 0 {
        return base;
    }
    let width = size - (shift as usize) * 2;
    let mut candidates: Vec<ArrowResult> = Vec::new();
    for (dx, dy) in [
        (-shift, 0),
        (shift, 0),
        (0, -shift),
        (0, shift),
        (-shift, -shift),
        (shift, shift),
        (-shift, shift),
        (shift, -shift),
    ] {
        let Some(patch) = centered_patch(rgba, size, width, dx, dy) else {
            continue;
        };
        let r = analyze_with(&patch, width, step, ws, params);
        if !r.valid || r.conf < 0.5 || r.tip_frac > 0.28 {
            continue;
        }
        // 质心必须贴着窗口中心（否则多半是窗外的地形青）
        let center = width as f64 / 2.0;
        let dxc = r.bc_x - center;
        let dyc = r.bc_y - center;
        if (dxc * dxc + dyc * dyc).sqrt() > width as f64 * 0.1 {
            continue;
        }
        if (r.area as f64) * (step * step) as f64 > (width * width) as f64 * 0.1 {
            continue;
        }
        candidates.push(r);
    }
    if candidates.len() < 3 {
        return base;
    }
    // 按角度聚簇，取最大的簇（>=3 票且过半）
    let mut best_cluster: Vec<usize> = Vec::new();
    for i in 0..candidates.len() {
        let group: Vec<usize> = (0..candidates.len())
            .filter(|&j| ang_diff(candidates[j].angle, candidates[i].angle).abs() <= 12.0)
            .collect();
        if group.len() > best_cluster.len() {
            best_cluster = group;
        }
    }
    if best_cluster.len() < 3 || best_cluster.len() * 2 <= candidates.len() {
        return base;
    }
    let best = best_cluster.iter().map(|&i| &candidates[i]).fold(
        &candidates[best_cluster[0]],
        |acc, cur| {
            if cur.conf > acc.conf {
                cur
            } else {
                acc
            }
        },
    );
    let delta: f64 = best_cluster
        .iter()
        .map(|&i| ang_diff(candidates[i].angle, best.angle))
        .sum::<f64>()
        / best_cluster.len() as f64;
    let mut out = best.clone();
    out.angle = (best.angle + delta + 360.0) % 360.0;
    out.conf = best.conf.min(0.68);
    out
}

/// 视锥 + 重试
pub fn analyze_cone_with_recovery(
    rgba: &[u8],
    size: usize,
    step: usize,
    ws: &mut ConeWorkspace,
    map_cyan_min: f64,
) -> ConeResult {
    let outer = analyze_cone(rgba, size, step, ws);
    // 窗口太小 / 没有小地图 / 两边都够强 → 不重试
    if size < 380 || outer.cyan_frac < map_cyan_min || (outer.valid && outer.edge_min() >= 0.095) {
        return outer;
    }
    let width = ((size as f64 * 0.64) + 0.5).floor() as usize;
    let Some(patch) = centered_patch(rgba, size, width, 0, 0) else {
        return outer;
    };
    let inner = analyze_cone(&patch, width, 1, ws);
    if !inner.valid || inner.edge_min() < 0.14 {
        return outer;
    }
    if outer.valid && ang_diff(inner.deg, outer.deg).abs() < 18.0 {
        return outer;
    }
    let mut out = inner;
    out.cyan_frac = outer.cyan_frac;
    out.conf = inner.conf.min(0.75);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(size: usize) -> Vec<u8> {
        // 灰底 + 中间一块青色，贴着左上边缘（模拟"箭头被窗口切掉"）
        let mut v = vec![0u8; size * size * 4];
        for i in 0..size * size {
            v[i * 4] = 40;
            v[i * 4 + 1] = 93;
            v[i * 4 + 2] = 103;
            v[i * 4 + 3] = 255;
        }
        for y in size / 2 - 12..size / 2 + 12 {
            for x in size / 2 - 12..size / 2 + 12 {
                let i = (y * size + x) * 4;
                v[i] = 0;
                v[i + 1] = 240;
                v[i + 2] = 255;
            }
        }
        v
    }

    #[test]
    fn centered_patch_keeps_size_and_center() {
        let f = frame(64);
        let p = centered_patch(&f, 64, 60, 1, 0).expect("裁剪应成功");
        assert_eq!(p.len(), 60 * 60 * 4);
        // 越界的偏移要返回 None
        assert!(centered_patch(&f, 64, 60, 10, 0).is_none());
    }

    #[test]
    fn recovery_returns_base_when_nothing_cyan() {
        let size = 160;
        let blank = vec![0u8; size * size * 4];
        let mut ws = ArrowWorkspace::default();
        let r = analyze_arrow_with_recovery(&blank, size, 1, &mut ws, &ArrowParams::default());
        assert!(!r.valid);
    }
}
