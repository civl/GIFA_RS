//! 旋转扫描：把真机帧绕中心旋转任意角度，测各算法是否"旋转等变"。
//!
//!   cargo run -p gifa-vision --example rotation-sweep [步长]
//!
//! 目的：现有真机语料 39 帧里 28 帧都集中在 315°~360° 一个朝向，
//! 单朝向的误差分不清"常数偏置"和"这个朝向才有的误差"。
//! 把真实帧转一圈，就能看出误差随朝向的变化曲线。

use gifa_vision::arrow::{analyze_with, ArrowParams, ArrowWorkspace};
use gifa_vision::hexagon::direction_v3;

fn rotate_rgba(src: &[u8], side: usize, deg: f64) -> Vec<u8> {
    let c = (side as f64 - 1.0) / 2.0;
    let (s, co) = (deg.to_radians().sin(), deg.to_radians().cos());
    let mut out = vec![0u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let dx = x as f64 - c;
            let dy = y as f64 - c;
            let sx = c + dx * co + dy * s;
            let sy = c - dx * s + dy * co;
            let i = (y * side + x) * 4;
            if sx < 0.0 || sy < 0.0 || sx >= side as f64 - 1.0 || sy >= side as f64 - 1.0 {
                out[i + 3] = 255;
                continue;
            }
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
            for ch in 0..3 {
                let g = |xx: usize, yy: usize| src[(yy * side + xx) * 4 + ch] as f32;
                let v = g(x0, y0) * (1.0 - fx) * (1.0 - fy)
                    + g(x0 + 1, y0) * fx * (1.0 - fy)
                    + g(x0, y0 + 1) * (1.0 - fx) * fy
                    + g(x0 + 1, y0 + 1) * fx * fy;
                out[i + ch] = v.round().clamp(0.0, 255.0) as u8;
            }
            out[i + 3] = 255;
        }
    }
    out
}

fn err(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0 - 180.0).abs()
}

fn main() {
    let step: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(15.0);
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(txt) = std::fs::read_to_string(root.join("docs/calibration/realgame-labels.json"))
    else {
        println!("缺真机语料");
        return;
    };
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
    // 取 3 帧不同亮度的样本
    let mut picked = Vec::new();
    for f in v["frames"].as_array().cloned().unwrap_or_default() {
        let (Some(seq), Some(truth)) = (f["seq"].as_u64(), f["truth"].as_f64()) else {
            continue;
        };
        let p = root.join(format!("apps/gifa-demo/data/gamelabel/f{seq:03}-126.bin"));
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        picked.push((seq, truth, bytes));
        if picked.len() >= 3 {
            break;
        }
    }
    if picked.is_empty() {
        println!("没有帧");
        return;
    }
    let params = ArrowParams {
        bis_err_max: 25.0,
        ang_margin_min: 18.0,
    };
    println!("旋转扫描（步长 {step}°）：每格 = 误差；x = 该帧不可用");
    println!("  朝向   v1      v3");
    let mut ws = ArrowWorkspace::default();
    let (mut v1_all, mut v3_all) = (Vec::new(), Vec::new());
    let mut a = 0.0;
    while a < 360.0 {
        let mut row1 = Vec::new();
        let mut row3 = Vec::new();
        for (_, truth, bytes) in &picked {
            let side = (bytes.len() / 4).isqrt();
            // 把这一帧转到"真值指向 a"
            let rot = rotate_rgba(bytes, side, a - truth);
            let r = analyze_with(&rot, side, 1, &mut ws, &params);
            if r.valid {
                let deg = (90.0 - r.angle).rem_euclid(360.0);
                row1.push(err(deg, a));
            }
            let c = (side as f64 / 2.0, side as f64 / 2.0);
            if let Some(d) = direction_v3(&rot, side, c, 0.0) {
                row3.push(err(d.deg, a));
            }
        }
        let fmt = |row: &Vec<f64>| {
            if row.is_empty() {
                "  x  ".to_string()
            } else {
                format!("{:>5.1}", row.iter().sum::<f64>() / row.len() as f64)
            }
        };
        v1_all.extend(row1.iter());
        v3_all.extend(row3.iter());
        println!("  {a:>3.0}°  {} {}", fmt(&row1), fmt(&row3));
        a += step;
    }
    let stat = |mut v: Vec<f64>| {
        v.sort_by(|x, y| x.partial_cmp(y).unwrap());
        (v.len(), v[v.len() / 2], *v.last().unwrap())
    };
    let (n1, m1, x1) = stat(v1_all);
    let (n3, m3, x3) = stat(v3_all);
    println!("\n  合计  v1: {n1} 次 中位 {m1:.1}° 最大 {x1:.1}°");
    println!("        v3: {n3} 次 中位 {m3:.1}° 最大 {x3:.1}°");
}
