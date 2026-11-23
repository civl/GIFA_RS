//! 把录制帧依次喂进检测 + 滤波，统计"输出与视锥差 >90°"的帧数（真机实测的抖动指标）。
//!   cargo run -p gifa-vision --example filter-replay -- <帧目录> [门限]
use gifa_vision::arrow::ArrowWorkspace;
use gifa_vision::cone::ConeWorkspace;
use gifa_vision::detect::detect_window;
use gifa_vision::filter::AbFilter;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).cloned().unwrap_or_else(|| "target/live".into());
    let gate: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(65.0);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("帧目录")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("bin"))
        .collect();
    files.sort();
    let mut ws = ArrowWorkspace::default();
    let mut cws = ConeWorkspace::default();
    let mut f = AbFilter::with_gate(gate);
    let (mut n, mut far_raw, mut far_filtered, mut jumps_raw, mut jumps_filt) = (0, 0, 0, 0, 0);
    let mut prev_raw: Option<f64> = None;
    let mut prev_f: Option<f64> = None;
    let d = |a: f64, b: f64| ((a - b + 540.0) % 360.0) - 180.0;
    for (i, p) in files.iter().enumerate() {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let out = detect_window(&bytes, side, &mut ws, &mut cws);
        let Some(view) = out.view else { continue };
        let t = i as f64 * 100.0;
        if let Some(a) = out.arrow {
            n += 1;
            if d(a, view).abs() > 90.0 {
                far_raw += 1;
            }
            if let Some(pa) = prev_raw {
                if d(a, pa).abs() > 90.0 {
                    jumps_raw += 1;
                }
            }
            prev_raw = Some(a);
            if let Some(sm) = f.smooth(a, 0.7, t, view, 0.6) {
                if d(sm, view).abs() > 90.0 {
                    far_filtered += 1;
                }
                if let Some(pf) = prev_f {
                    if d(sm, pf).abs() > 90.0 {
                        jumps_filt += 1;
                    }
                }
                prev_f = Some(sm);
            }
        }
    }
    println!("门限 {gate}°：{n} 帧有箭头");
    println!("  与视锥差 >90°：原始 {far_raw} → 过滤后 {far_filtered}");
    println!("  帧间 >90° 跳变：原始 {jumps_raw} → 过滤后 {jumps_filt}");
}
