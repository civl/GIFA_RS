//! 直走录制上，分别看 v1 / v3 与视锥（真值代理）的一致性。
use gifa_vision::arrow::{analyze_with, ArrowParams, ArrowWorkspace};
use gifa_vision::hexagon::direction_v3;

fn n(s: &str) -> Option<f64> {
    let t: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit() && *c != '-')
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    t.parse().ok()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).cloned().unwrap_or_else(|| "target/walk".into());
    let csv_path = args.get(2).cloned().unwrap_or_else(|| "walk2.csv".into());
    let rows: Vec<Vec<String>> = std::fs::read_to_string(&csv_path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split(',').map(|s| s.to_string()).collect())
        .collect();
    let head = &rows[0];
    let ci = |name: &str| head.iter().position(|c| c == name).unwrap_or(0);
    let vi = ci("view");

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
        ..Default::default()
    };
    let mut ws = ArrowWorkspace::default();
    let (mut v1_n, mut v1_good, mut v3_n, mut v3_good) = (0, 0, 0, 0);
    let mut both_n = 0;
    let mut v1_bad_examples = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let Some(row) = rows.get(i + 1) else { break };
        let Some(view) = n(&row[vi]) else { continue };
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let c = (side as f64 / 2.0, side as f64 / 2.0);
        let r = analyze_with(&bytes, side, 1, &mut ws, &params);
        let v3 = direction_v3(&bytes, side, c, 0.0);
        if r.valid {
            v1_n += 1;
            let deg = (90.0 - r.angle).rem_euclid(360.0);
            let d = ((deg - view + 540.0) % 360.0 - 180.0).abs();
            if d < 15.0 {
                v1_good += 1;
            } else if v1_bad_examples.len() < 6 {
                v1_bad_examples.push((
                    f.file_name().unwrap().to_string_lossy().into_owned(),
                    deg,
                    view,
                    d,
                ));
            }
            if v3.is_some() {
                both_n += 1;
            }
        }
        if let Some(d) = v3 {
            v3_n += 1;
            let dd = ((d.deg - view + 540.0) % 360.0 - 180.0).abs();
            if dd < 15.0 {
                v3_good += 1;
            }
        }
    }
    println!(
        "v1 有效 {v1_n}，其中与视锥差<15° 的 {v1_good}（{:.0}%）",
        100.0 * v1_good as f64 / v1_n.max(1) as f64
    );
    println!(
        "v3 有效 {v3_n}，其中与视锥差<15° 的 {v3_good}（{:.0}%）",
        100.0 * v3_good as f64 / v3_n.max(1) as f64
    );
    println!("两路同时有效 {both_n}");
    println!("v1 判错的例子（文件, v1, 视锥, 差）:");
    for e in &v1_bad_examples {
        println!("  {}  {:.1}° vs {:.1}°  差 {:.1}°", e.0, e.1, e.2, e.3);
    }
}
