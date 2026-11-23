//! 真机帧上的通道覆盖率统计：v1 有效多少、v3 有效多少、两路都有效时差多少。
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
    let params = ArrowParams {
        bis_err_max: 25.0,
        ang_margin_min: 18.0,
    };
    let mut ws = ArrowWorkspace::default();
    let (mut n, mut v1_ok, mut v3_ok, mut same) = (0, 0, 0, 0);
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        n += 1;
        let r = analyze_with(&bytes, side, 1, &mut ws, &params);
        if r.valid {
            v1_ok += 1;
        }
        let c = (side as f64 / 2.0, side as f64 / 2.0);
        if direction_v3(&bytes, side, c, 0.0).is_some() {
            v3_ok += 1;
        }
        if r.valid && direction_v3(&bytes, side, c, 0.0).is_some() {
            same += 1;
        }
    }
    println!(
        "真机帧 {n}：v1 有效 {v1_ok}（{:.0}%），v3 有效 {v3_ok}（{:.0}%），两路同时有效 {same}",
        100.0 * v1_ok as f64 / n as f64,
        100.0 * v3_ok as f64 / n as f64
    );
}
