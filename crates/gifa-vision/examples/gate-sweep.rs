//! 扫 v1 的几何门，看"覆盖率 vs 与 v3 的一致性"如何权衡。
//!   cargo run -p gifa-vision --example gate-sweep -- <帧目录>
use gifa_vision::arrow::{analyze_with, ArrowParams, ArrowWorkspace};
use gifa_vision::hexagon::direction_v3;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/live".into());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("帧目录")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("bin"))
        .collect();
    files.sort();
    // 先把每帧的 v3 结果算好当参照
    let mut ws = ArrowWorkspace::default();
    let mut frames = Vec::new();
    for p in &files {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let c = (side as f64 / 2.0, side as f64 / 2.0);
        let v3 = direction_v3(&bytes, side, c, 0.0).map(|d| d.deg);
        frames.push((bytes, side, v3));
    }
    let d = |a: f64, b: f64| ((a - b + 540.0) % 360.0 - 180.0).abs();
    println!("参数组合 → v1 覆盖 / 与 v3 差 <15° 的比例（分母=两路都有值的帧）");
    let combos: &[(&str, ArrowParams)] = &[
        (
            "原版门（95..162, far1.02, flat-0.5）",
            ArrowParams::default(),
        ),
        (
            "放宽角度到 80..175",
            ArrowParams {
                tail_angle_min: 80.0,
                tail_angle_max: 175.0,
                ..Default::default()
            },
        ),
        (
            "放宽 far 到 1.25",
            ArrowParams {
                nose_far_ratio: 1.25,
                ..Default::default()
            },
        ),
        (
            "放宽 flat 到 -1.0",
            ArrowParams {
                nose_flat_min: -1.0,
                ..Default::default()
            },
        ),
        (
            "三者都放宽",
            ArrowParams {
                tail_angle_min: 80.0,
                tail_angle_max: 175.0,
                nose_far_ratio: 1.25,
                nose_flat_min: -1.0,
                ..Default::default()
            },
        ),
    ];
    for (name, p) in combos {
        let mut cov = 0;
        let mut both = 0;
        let mut agree = 0;
        let mut diffs = Vec::new();
        for (bytes, side, v3) in &frames {
            let r = analyze_with(bytes, *side, 1, &mut ws, p);
            if !r.valid {
                continue;
            }
            cov += 1;
            let a = (90.0 - r.angle).rem_euclid(360.0);
            if let Some(v) = v3 {
                both += 1;
                let dd = d(a, *v);
                diffs.push(dd);
                if dd < 15.0 {
                    agree += 1;
                }
            }
        }
        diffs.sort_by(|x, y| x.partial_cmp(y).unwrap());
        println!(
            "  {name:<28} 覆盖 {cov:>3}  一致 {:.0}%（{}/{both}）  中位差 {:.1}°",
            if both > 0 {
                100.0 * agree as f64 / both as f64
            } else {
                0.0
            },
            agree,
            if diffs.is_empty() {
                f64::NAN
            } else {
                diffs[diffs.len() / 2]
            }
        );
    }
}
