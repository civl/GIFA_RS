//! 颜色通道：箭头通道的第一块砖。
//!
//! 原版结论（`webview-preload.js`）：
//! * 角色箭头是接近纯青的 `rgb(34,200,232)`
//! * 草地/树影阴影处的 `min(g,b)-r` 能到 20~90
//! * 所以阈值下限必须高（112），否则满屏草地都被当成青色

use crate::params;

/// 青色分数：`min(g, b) - r`
pub fn cyan_score(r: u8, g: u8, b: u8) -> i32 {
    g.min(b) as i32 - r as i32
}

/// 已排序样本的分位数（`ratio` 取 0.99 即 p99）
pub fn percentile(sorted: &[f64], ratio: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * ratio).floor() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// 自适应阈值。
///
/// `p99 < 100` 说明整块画面里没有接近纯青的像素 —— 这一帧根本没有箭头
/// （转场 / 特写 / 对话），直接返回 `None`，不要硬猜。
pub fn adaptive_threshold(p99: f64) -> Option<f64> {
    if p99 < params::NO_MAP_P99 {
        return None;
    }
    Some(params::THRESHOLD_FLOOR.max(p99 * params::THRESHOLD_RATIO))
}

/// 小地图存在性门控：青色像素占采样点的比例。
/// 166 帧实测：有小地图 ≥ 0.019，无小地图 ≤ 0.0011，取 0.0015 留余量。
pub fn map_present(cyan_fraction: f64) -> bool {
    cyan_fraction >= params::MAP_CYAN_FRAC_MIN
}
