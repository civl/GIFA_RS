//! 看新的"白膜几何校验"在真机帧上有多少帧没过（没过会被降 conf）。
//!   cargo run -p gifa-vision --example cone-check -- <帧目录>
use gifa_vision::arrow::ArrowWorkspace;
use gifa_vision::cone::{analyze_cone, ConeWorkspace};
use gifa_vision::detect::detect_window;

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
    let mut ws = ConeWorkspace::default();
    let mut aws = ArrowWorkspace::default();
    let (mut n, mut valid, mut bad_membrane, mut valid_but_bad) = (0, 0, 0, 0);
    // 白膜校验与"箭头-视锥一致性"的关系（直走时两者应当接近）
    let mut pass_diff = Vec::new();
    let mut fail_diff = Vec::new();
    let d = |a: f64, b: f64| ((a - b + 540.0) % 360.0 - 180.0).abs();
    for p in &files {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let r = analyze_cone(&bytes, side, 1, &mut ws);
        n += 1;
        if r.valid {
            valid += 1;
        }
        if (r.rise > 0.0 || r.fall > 0.0) && !r.membrane_ok {
            bad_membrane += 1;
        }
        if !r.membrane_ok && r.conf > 0.12 {
            valid_but_bad += 1;
        }
        let out = detect_window(&bytes, side, &mut aws, &mut ws);
        if let (Some(a), Some(v)) = (out.arrow, out.view) {
            if out.arrow_source == 1 {
                // 只统计 v1 出结果的帧（v1 有效说明箭头可信）
                (if out.membrane_ok {
                    &mut pass_diff
                } else {
                    &mut fail_diff
                })
                .push(d(a, v));
            }
        }
    }
    println!("{n} 帧：视锥有效 {valid}，白膜校验未过 {bad_membrane}，其中仍判有效 {valid_but_bad}");
    let stat = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if v.is_empty() {
            f64::NAN
        } else {
            v[v.len() / 2]
        }
    };
    println!(
        "v1 出结果的帧里，|箭头-视锥|：白膜过 {:.1}°（{} 帧）/ 未过 {:.1}°（{} 帧）",
        stat(pass_diff.clone()),
        pass_diff.len(),
        stat(fail_diff.clone()),
        fail_diff.len()
    );
}
