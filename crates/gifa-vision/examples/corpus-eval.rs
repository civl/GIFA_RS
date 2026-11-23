//! 语料评估：对比 v1 / 现有六边形 / 新几何模型 v2 的覆盖率与精度。
//!
//!   cargo run -p gifa-vision --example corpus-eval
//!
//! 两份语料：
//!   · 真机 39 帧  apps/gifa-demo/data/gamelabel/f{seq:03}-126.bin  + docs/calibration/realgame-labels.json
//!   · 视频 138 帧 apps/gifa-demo/data/labelseq/f{seq:03}-142.bin    + docs/calibration/mondstadt-labels.json

use gifa_vision::arrow::{analyze_with, ArrowParams, ArrowWorkspace};
use gifa_vision::hexagon::{direction, direction_v3};

struct Frame {
    data: Vec<u8>,
    side: usize,
    truth: f64,
}

/// 按 `f{seq:03}-*.bin` 找帧（窗口尺寸后缀会变：126 / 142 / 250 …）
fn find_frame(dir: &std::path::Path, seq: u64) -> Option<std::path::PathBuf> {
    let prefix = format!("f{seq:03}-");
    let mut hits: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix) && n.ends_with(".bin"))
                .unwrap_or(false)
        })
        .collect();
    hits.sort();
    hits.pop()
}

fn load(dir: &str, labels: &str, _w: usize) -> Vec<Frame> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lab = match std::fs::read_to_string(root.join(labels)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let v: serde_json::Value = serde_json::from_str(&lab).unwrap_or_default();
    let mut out = Vec::new();
    for f in v["frames"].as_array().cloned().unwrap_or_default() {
        let (Some(seq), Some(truth)) = (f["seq"].as_u64(), f["truth"].as_f64()) else {
            continue;
        };
        if f["bad"].as_bool().unwrap_or(false) {
            continue;
        }
        let dirp = root.join(format!("apps/gifa-demo/data/{dir}"));
        let Some(path) = find_frame(&dirp, seq) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        out.push(Frame {
            data: bytes,
            side,
            truth,
        });
    }
    out
}

fn err(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0 - 180.0).abs()
}

fn report(name: &str, sides: &[(Option<f64>, f64)]) {
    let hits: Vec<f64> = sides
        .iter()
        .filter_map(|(a, t)| a.map(|x| err(x, *t)))
        .collect();
    let mut v = hits.clone();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |q: f64| -> f64 {
        if v.is_empty() {
            f64::NAN
        } else {
            v[((v.len() - 1) as f64 * q).round() as usize]
        }
    };
    let gross = v.iter().filter(|e| **e > 45.0).count();
    println!(
        "  {name:<26} 覆盖 {:>3}/{:<3}  中位 {:>5.1}°  p90 {:>5.1}°  最大 {:>6.1}°  >45° {}",
        hits.len(),
        sides.len(),
        pct(0.5),
        pct(0.9),
        v.last().copied().unwrap_or(f64::NAN),
        gross
    );
}

fn main() {
    let params = ArrowParams {
        bis_err_max: 25.0,
        ang_margin_min: 18.0,
        ..Default::default()
    };
    for (label, dir, labels, w) in [
        (
            "真机 39 帧",
            "gamelabel",
            "docs/calibration/realgame-labels.json",
            126,
        ),
        (
            "视频 138 帧",
            "labelseq",
            "docs/calibration/mondstadt-labels.json",
            142,
        ),
    ] {
        let frames = load(dir, labels, w);
        if frames.is_empty() {
            println!("{label}: 语料缺失，跳过");
            continue;
        }
        println!("\n{label}（n={}）", frames.len());
        let mut ws = ArrowWorkspace::default();

        // A: v1
        let a: Vec<(Option<f64>, f64)> = frames
            .iter()
            .map(|f| {
                let r = analyze_with(&f.data, f.side, 1, &mut ws, &params);
                // v1 输出 atan2（0=东、逆时针），真值是罗盘（0=北、顺时针）
                (
                    if r.valid {
                        Some((90.0 - r.angle).rem_euclid(360.0))
                    } else {
                        None
                    },
                    f.truth,
                )
            })
            .collect();
        report("v1（三点拟合）", &a);

        // B: 现有六边形（凹口中点 → 最远点）
        let b: Vec<(Option<f64>, f64)> = frames
            .iter()
            .map(|f| (direction(&f.data, f.side).map(|d| d.deg), f.truth))
            .collect();
        report("六边形 v1（现状）", &b);

        // C2: v3（主轴 + 两端宽度）
        let c3: Vec<(Option<f64>, f64)> = frames
            .iter()
            .map(|f| {
                let c0 = (f.side as f64 / 2.0, f.side as f64 / 2.0);
                (
                    direction_v3(&f.data, f.side, c0, 0.0).map(|d| d.deg),
                    f.truth,
                )
            })
            .collect();
        report("六边形 v3（主轴+宽度）", &c3);

        // D: 管线的实际策略：v1 优先，失败才用六边形
        let d1: Vec<(Option<f64>, f64)> = a
            .iter()
            .zip(&b)
            .map(|((x, t), (y, _))| (x.or(*y), *t))
            .collect();
        report("v1 + 六边形 v1", &d1);
        let d3: Vec<(Option<f64>, f64)> = a
            .iter()
            .zip(&c3)
            .map(|((x, t), (y, _))| (x.or(*y), *t))
            .collect();
        report("v1 + 六边形 v3", &d3);
    }
}
