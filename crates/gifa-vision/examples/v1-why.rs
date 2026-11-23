//! v1 在真机帧上"为什么不出结果"：把被挡的原因分桶统计。
//!   cargo run -p gifa-vision --example v1-why -- <帧目录>
use gifa_vision::arrow::{analyze_with, ArrowParams, ArrowWorkspace};

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
        ..Default::default()
    };
    let mut ws = ArrowWorkspace::default();
    let mut reasons: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut margins = Vec::new();
    let mut cyans_blocked = Vec::new();
    let mut p99s = Vec::new();
    let mut corner_counts = Vec::new();
    let mut n = 0;
    for p in &files {
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let r = analyze_with(&bytes, side, 1, &mut ws, &params);
        n += 1;
        if r.valid {
            reasons.entry(0).and_modify(|c| *c += 1).or_insert(1);
        } else {
            *reasons.entry(1).or_insert(0) += 1;
            margins.push(r.ang_margin);
            cyans_blocked.push(r.cyans);
            corner_counts.push(r.corners.len());
            // 窗口内青色分数的 p99（原版的门控就是看它）
            let mut probe: Vec<i32> = Vec::new();
            for y in (0..side).step_by(4) {
                for x in (0..side).step_by(4) {
                    let i = (y * side + x) * 4;
                    probe.push(bytes[i + 1] as i32 - bytes[i] as i32);
                }
            }
            probe.sort_unstable();
            p99s.push(probe[(probe.len() as f64 * 0.99) as usize % probe.len()]);
        }
    }
    println!(
        "{n} 帧，v1 有效 {}/{}",
        reasons.get(&0).copied().unwrap_or(0),
        n
    );

    margins.sort_by(|a, b| a.partial_cmp(b).unwrap());
    cyans_blocked.sort_unstable();
    if !cyans_blocked.is_empty() {
        println!(
            "被挡帧的 cyans：中位 {}，p10 {}，最大 {}（门槛 12）",
            cyans_blocked[cyans_blocked.len() / 2],
            cyans_blocked[cyans_blocked.len() / 10],
            cyans_blocked[cyans_blocked.len() - 1]
        );
    }
    corner_counts.sort_unstable();
    if !corner_counts.is_empty() {
        println!(
            "被挡帧的角点数：中位 {}，最小 {}，最大 {}（三点枚举至少要 3）",
            corner_counts[corner_counts.len() / 2],
            corner_counts[0],
            corner_counts[corner_counts.len() - 1]
        );
    }
    p99s.sort_unstable();
    if !p99s.is_empty() {
        println!(
            "被挡帧的 p99：中位 {}，p90 {}，最大 {}（门槛 100）",
            p99s[p99s.len() / 2],
            p99s[p99s.len() * 9 / 10],
            p99s[p99s.len() - 1]
        );
    }
    if !margins.is_empty() {
        println!(
            "被挡帧的 angMargin：中位 {:.1}°，p10 {:.1}°，最大 {:.1}°（门槛 18）",
            margins[margins.len() / 2],
            margins[margins.len() / 10],
            margins[margins.len() - 1]
        );
    }
}
