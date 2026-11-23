//! 在"直走"录制上对比：v3 用整幅 mask vs 只用中心内圆（1.2×箭头半径）。
//! 真值代理 = 同一录制里的视锥角度（直走时角色朝向≈镜头朝向）。
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
    let (ai, vi) = (ci("arrow"), ci("view"));

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("帧目录")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("bin"))
        .collect();
    files.sort();

    // 对每个半径参数统计：与视锥差 <15° 的比例、帧间抖动
    for ratio in [0.0f64, 0.06, 0.09, 0.12, 0.16] {
        let mut good = 0;
        let mut total = 0;
        let mut prev: Option<f64> = None;
        let mut steps = Vec::new();
        for (i, f) in files.iter().enumerate() {
            // CSV 行与帧大致按序对应（第 0 行是表头）
            let Some(row) = rows.get(i + 1) else { break };
            let (Some(view), Some(_arrow)) = (n(&row[vi]), n(&row[ai])) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(f) else {
                continue;
            };
            let side = (bytes.len() / 4).isqrt();
            let c = (side as f64 / 2.0, side as f64 / 2.0);
            let r = if ratio > 0.0 {
                side as f64 * ratio
            } else {
                0.0
            };
            let Some(d) = direction_v3(&bytes, side, c, r) else {
                continue;
            };
            total += 1;
            let diff = ((d.deg - view + 540.0) % 360.0 - 180.0).abs();
            if diff < 15.0 {
                good += 1;
            }
            if let Some(p) = prev {
                steps.push(((d.deg - p + 540.0) % 360.0 - 180.0).abs());
            }
            prev = Some(d.deg);
        }
        if total == 0 {
            println!("ratio {ratio:.2}: 没有数据");
            continue;
        }
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut goodfrac = 100.0 * good as f64 / total as f64;
        if good > total {
            goodfrac = 100.0;
        }
        println!(
            "内圆半径 {:.2}×边(={:.0}px): 与视锥差<15° 的比例 {:.0}%（{}/{}），帧间中位抖动 {:.1}°",
            ratio,
            ratio * 158.0,
            goodfrac,
            good,
            total,
            steps.get(steps.len() / 2).copied().unwrap_or(0.0)
        );
    }
}
