//! 评估 detect_window（含两路融合）在语料上的精度，按来源分桶。
//!   cargo run -p gifa-vision --example detect-eval
use gifa_vision::arrow::ArrowWorkspace;
use gifa_vision::cone::ConeWorkspace;
use gifa_vision::detect::detect_window;

fn err(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0 - 180.0).abs()
}

fn load(dir: &str, labels: &str) -> Vec<(Vec<u8>, usize, f64)> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Ok(txt) = std::fs::read_to_string(root.join(labels)) else {
        return Vec::new();
    };
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
    let mut out = Vec::new();
    for f in v["frames"].as_array().cloned().unwrap_or_default() {
        let (Some(seq), Some(truth)) = (f["seq"].as_u64(), f["truth"].as_f64()) else {
            continue;
        };
        if f["bad"].as_bool().unwrap_or(false) {
            continue;
        }
        let d = root.join(format!("apps/gifa-demo/data/{dir}"));
        let prefix = format!("f{seq:03}-");
        let Some(path) = std::fs::read_dir(&d).ok().and_then(|it| {
            it.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix) && n.ends_with(".bin"))
                    .unwrap_or(false)
            })
        }) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        out.push((bytes, side, truth));
    }
    out
}

fn stat(mut v: Vec<f64>) -> (usize, f64, f64, usize) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let gross = v.iter().filter(|e| **e > 45.0).count();
    let n = v.len();
    if n == 0 {
        (0, f64::NAN, f64::NAN, 0)
    } else {
        (n, v[n / 2], v[n - 1], gross)
    }
}

fn main() {
    let mut aws = ArrowWorkspace::default();
    let mut cws = ConeWorkspace::default();
    for (name, dir, lab) in [
        (
            "真机 39 帧",
            "gamelabel",
            "docs/calibration/realgame-labels.json",
        ),
        (
            "视频 138 帧",
            "labelseq",
            "docs/calibration/mondstadt-labels.json",
        ),
    ] {
        let frames = load(dir, lab);
        if frames.is_empty() {
            println!("{name}: 缺语料");
            continue;
        }
        let mut by_src: std::collections::BTreeMap<u8, Vec<f64>> = Default::default();
        let mut all = Vec::new();
        for (bytes, side, truth) in &frames {
            let o = detect_window(bytes, *side, &mut aws, &mut cws);
            if let Some(a) = o.arrow {
                let e = err(a, *truth);
                by_src.entry(o.arrow_source).or_default().push(e);
                all.push(e);
            }
        }
        let (n, med, mx, gross) = stat(all.clone());
        println!(
            "\n{name}（n={}，detect 有效 {}）中位 {:.1}° 最大 {:.1}° 翻转 {gross}",
            frames.len(),
            n,
            med,
            mx
        );
        for (s, v) in &by_src {
            let (n, med, mx, g) = stat(v.clone());
            println!(
                "  src={s}（{}）: {n} 帧  中位 {med:.1}°  最大 {mx:.1}°  翻转 {g}",
                match s {
                    1 => "v1",
                    2 => "v3 兜底",
                    3 => "融合",
                    4 => "分歧取v1",
                    _ => "?",
                }
            );
        }
    }
}
