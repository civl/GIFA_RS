//! 视觉识别的**二期**骨架（task.md D2：一期识别留在页面 JS 里）。
//!
//! 这里只落三样东西，够二期直接往里填：
//!   1. `Frame` / `Roi` / `FrameSource`：帧源抽象（换 mpv 也不改算法）
//!   2. `color::cyan_score`：整个箭头通道的第一块砖
//!   3. `Arrow` / `Cone` / `Bearing`：输出结构
//!
//! 刻意**不**在这里写"半成品算法"：二期的移植纪律是逐函数对照
//! `webview-preload.js`，凭理解先写一版反而会把对拍基线搞乱。

pub mod arrow;
pub mod color;
pub mod cone;
pub mod detect;
pub mod pipeline;
pub mod frame;
pub mod result;
pub mod screen;

pub use arrow::{analyze, to_compass, ArrowResult, ArrowWorkspace, Corner};
pub use cone::{analyze_cone, ConeResult, ConeWorkspace};
pub use detect::{detect_simple, detect_window, Detect, Origin, SampleBox};
pub use frame::{Frame, FrameSource, Roi};
pub use pipeline::{start as start_pipeline, VisionEvent};
pub use screen::capture_screen;
pub use result::{Arrow, Bearing, Cone};

/// 二期移植的参数（照抄原版常量，集中放这里便于对拍时逐个核对）
pub mod params {
    /// 角剖面分格数（5°/格）
    pub const ARROW_BINS: usize = 72;
    /// 视锥极坐标角度分格（3°/格）
    pub const CONE_ANGLE_BINS: usize = 120;
    /// 视锥径向层数
    pub const CONE_RADIAL_BINS: usize = 12;
    /// 自适应阈值下限
    pub const THRESHOLD_FLOOR: f64 = 112.0;
    /// 自适应阈值 = max(FLOOR, p99 * RATIO)
    pub const THRESHOLD_RATIO: f64 = 0.74;
    /// 整块画面没有这个分数以上的像素 → 判定"这一帧没有小地图"
    pub const NO_MAP_P99: f64 = 100.0;
    /// 有效半径带（相对采样窗半边）
    pub const RADIUS_MIN: f64 = 0.04;
    pub const RADIUS_MAX: f64 = 0.5;
    /// 小地图存在性门控（青色像素占比）
    pub const MAP_CYAN_FRAC_MIN: f64 = 0.0015;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyan_score_matches_original_formula() {
        // min(g,b) - r：原神箭头约 rgb(34,200,232) → 166
        assert_eq!(color::cyan_score(34, 200, 232), 166);
        // 草地阴影实测 20~90，必须远低于箭头
        assert!(color::cyan_score(60, 90, 70) < 40);
        // 纯红没有青色成分 → 负值
        assert!(color::cyan_score(255, 0, 0) < 0);
        // 灰白（雪地）也接近 0
        assert_eq!(color::cyan_score(240, 240, 240), 0);
    }

    #[test]
    fn percentile_picks_expected_cut() {
        let mut samples: Vec<f64> = (0..=100).map(|i| i as f64).collect();
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((color::percentile(&samples, 0.99) - 99.0).abs() <= 1.0);
        assert!((color::percentile(&samples, 0.5) - 50.0).abs() <= 1.0);
        assert_eq!(color::percentile(&[], 0.99), 0.0);
    }

    #[test]
    fn adaptive_threshold_uses_floor_and_ratio() {
        // p99 低于 100 → 这一帧没有箭头
        assert_eq!(color::adaptive_threshold(90.0), None);
        // p99 = 166 → max(112, 122.8) = 122.84
        let t = color::adaptive_threshold(166.0).unwrap();
        assert!((t - 122.84).abs() < 0.01);
        // p99 很高时下限仍然生效
        assert_eq!(color::adaptive_threshold(120.0).unwrap(), 112.0_f64.max(88.8));
    }

    #[test]
    fn roi_pixels_use_video_width_for_radius() {
        let r = Roi {
            cx: 0.5,
            cy: 0.5,
            r: 0.1,
        };
        let (x, y, rad) = r.to_pixels(1920, 1080);
        assert_eq!((x, y, rad), (960.0, 540.0, 192.0));
    }

    #[test]
    fn frame_sampling_keeps_original_colors() {
        // 2×2 帧：data 是 RGBA，索引按 (y*w+x)*4
        let f = Frame::new(2, 2, vec![
            1, 2, 3, 255, 4, 5, 6, 255,
            7, 8, 9, 255, 10, 11, 12, 255,
        ]);
        assert_eq!(f.pixel(0, 0), Some((1, 2, 3)));
        assert_eq!(f.pixel(1, 1), Some((10, 11, 12)));
        assert_eq!(f.pixel(2, 0), None);
    }
}
