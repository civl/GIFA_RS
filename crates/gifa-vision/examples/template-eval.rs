//! 模板匹配实验：多角度模板库 + NMS，和现有 v1/v3 对比方向精度。
//!
//!   cargo run -p gifa-vision --example template-eval [角度步长]
//!
//! 做法（用真值训练模板，只看"匹配能不能比几何法更准"）：
//!   1. 把每帧的青色 mask 按真值角度**反向旋转**到统一朝向 → 平均成一张模板
//!   2. 逐角度旋转模板，和帧做归一化互相关（NCC）
//!   3. 取峰值角度；多个峰之间做 NMS（±15° 内抑制），报告峰值一致性

use gifa_vision::hexagon::direction_v3;

struct Frame {
    rgba: Vec<u8>,
    mask: Vec<f32>,
    side: usize,
    truth: f64,
}

fn cyan_mask(rgba: &[u8], side: usize) -> Vec<f32> {
    let mut score = Vec::with_capacity(side * side);
    for i in 0..side * side {
        let b = rgba[i * 4] as i32;
        let g = rgba[i * 4 + 1] as i32;
        score.push((g.min(255) - b).max(0) as f32);
    }
    let mut sorted = score.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = sorted[(sorted.len() * 99 / 100).min(sorted.len() - 1)];
    let thr = 112.0f32.max(p99 * 0.74);
    score
        .iter()
        .map(|v| if *v >= thr { 1.0 } else { 0.0 })
        .collect()
}

fn load(dir: &str, labels: &str) -> Vec<Frame> {
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
        out.push(Frame {
            mask: cyan_mask(&bytes, side),
            rgba: bytes,
            side,
            truth,
        });
    }
    out
}

/// 双线性缩放到 side×side（把不同采样窗统一到同一尺寸，才能跨域比模板）
fn resize(src: &[f32], from: usize, to: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; to * to];
    let k = (from as f64 - 1.0) / (to as f64 - 1.0).max(1.0);
    for y in 0..to {
        for x in 0..to {
            let sx = x as f64 * k;
            let sy = y as f64 * k;
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(from - 1), (y0 + 1).min(from - 1));
            let (fx, fy) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
            let i = |xx: usize, yy: usize| src[yy * from + xx];
            out[y * to + x] = i(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + i(x1, y0) * fx * (1.0 - fy)
                + i(x0, y1) * (1.0 - fx) * fy
                + i(x1, y1) * fx * fy;
        }
    }
    out
}

/// 以中心为原点旋转采样（双线性），返回旋转后的图
fn rotate(src: &[f32], side: usize, deg: f64) -> Vec<f32> {
    let c = (side as f64 - 1.0) / 2.0;
    let (s, co) = (deg.to_radians().sin(), deg.to_radians().cos());
    let mut out = vec![0.0; side * side];
    for y in 0..side {
        for x in 0..side {
            let dx = x as f64 - c;
            let dy = y as f64 - c;
            // 反向映射
            let sx = c + dx * co + dy * s;
            let sy = c - dx * s + dy * co;
            if sx < 0.0 || sy < 0.0 || sx >= side as f64 - 1.0 || sy >= side as f64 - 1.0 {
                continue;
            }
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
            let i = |xx: usize, yy: usize| src[yy * side + xx];
            out[y * side + x] = i(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + i(x0 + 1, y0) * fx * (1.0 - fy)
                + i(x0, y0 + 1) * (1.0 - fx) * fy
                + i(x0 + 1, y0 + 1) * fx * fy;
        }
    }
    out
}

fn ncc(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..a.len() {
        let (x, y) = (a[i] - ma, b[i] - mb);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da < 1e-6 || db < 1e-6 {
        return 0.0;
    }
    num / (da * db).sqrt()
}

fn err(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0 - 180.0).abs()
}

fn build_template(frames: &[Frame], skip: usize) -> Vec<f32> {
    let side = frames[0].side;
    let mut tpl = vec![0.0f32; side * side];
    let mut n = 0.0f32;
    for (i, f) in frames.iter().enumerate() {
        if i == skip {
            continue;
        }
        let r = rotate(&f.mask, side, -f.truth);
        for k in 0..tpl.len() {
            tpl[k] += r[k];
        }
        n += 1.0;
    }
    for v in tpl.iter_mut() {
        *v /= n.max(1.0);
    }
    tpl
}

/// 返回 (最佳角度, 前两名角度差)
fn match_best(f: &Frame, tpl: &[f32], step: f64) -> (f64, f64) {
    let side = f.side;
    let angles: Vec<f64> = (0..(360.0 / step).round() as i32)
        .map(|i| i as f64 * step)
        .collect();
    let scores: Vec<f32> = angles
        .iter()
        .map(|a| ncc(&rotate(tpl, side, *a), &f.mask))
        .collect();
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|x, y| scores[*y].partial_cmp(&scores[*x]).unwrap());
    // NMS：±15° 内抑制
    let mut peaks: Vec<usize> = Vec::new();
    for idx in order {
        if peaks.iter().all(|p| err(angles[idx], angles[*p]) > 15.0) {
            peaks.push(idx);
        }
        if peaks.len() >= 2 {
            break;
        }
    }
    let second = peaks
        .get(1)
        .map(|p| err(angles[peaks[0]], angles[*p]))
        .unwrap_or(180.0);
    (angles[peaks[0]], second)
}

fn main() {
    let mut store: Vec<(Vec<Frame>, usize)> = Vec::new();
    let step: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    for (label, dir, lab) in [
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
            println!("{label}: 缺语料");
            continue;
        }
        let side = frames[0].side;
        let angles = (360.0 / step).round() as usize;

        let (mut tmpl_e, mut v3_e, mut v1_e) = (Vec::new(), Vec::new(), Vec::new());
        let mut disagree = 0;
        let mut ws = gifa_vision::arrow::ArrowWorkspace::default();
        let params = gifa_vision::arrow::ArrowParams {
            bis_err_max: 25.0,
            ang_margin_min: 18.0,
        };
        for i in 0..frames.len() {
            let f = &frames[i];
            // 留一：模板用其余帧训练
            let tpl = build_template(&frames, i);
            let (best, gap) = match_best(f, &tpl, step);
            tmpl_e.push(err(best, f.truth));
            if gap < 30.0 {
                disagree += 1;
            }
            let c = (side as f64 / 2.0, side as f64 / 2.0);
            if let Some(d) = direction_v3(&f.rgba, side, c, 0.0) {
                v3_e.push(err(d.deg, f.truth));
            }
            let r = gifa_vision::arrow::analyze_with(&f.rgba, side, 1, &mut ws, &params);
            if r.valid {
                let deg = (90.0 - r.angle).rem_euclid(360.0);
                v1_e.push(err(deg, f.truth));
            }
        }
        let stat = |mut v: Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let gross = v.iter().filter(|e| **e > 45.0).count();
            (
                v.len(),
                if v.is_empty() {
                    f64::NAN
                } else {
                    v[v.len() / 2]
                },
                v.last().copied().unwrap_or(f64::NAN),
                gross,
            )
        };
        let (tn, tmed, tmax, tgross) = stat(tmpl_e);
        let (vn, vmed, vmax, vgross) = stat(v3_e);
        let (n1, m1, _, g1) = stat(v1_e);
        println!("\n{label}（窗口 {side}px，{angles} 个角度，步长 {step}°，**留一交叉验证**）");
        println!("  模板匹配+NMS  {tn:>3} 帧  中位 {tmed:>5.1}°  最大 {tmax:>5.1}°  翻转 {tgross}  前两名<30° 的有 {disagree}");
        println!("  v3 兜底       {vn:>3} 帧  中位 {vmed:>5.1}°  最大 {vmax:>5.1}°  翻转 {vgross}");
        println!("  v1 三点       {n1:>3} 帧  中位 {m1:>5.1}°              翻转 {g1}");
        store.push((frames, side));
    }

    /* ---- 跨域：用 A 语料训模板，去 B 语料上匹配（统一缩放到 96×96） ---- */
    if store.len() == 2 {
        const N: usize = 96;
        for (train_idx, test_idx) in [(0usize, 1usize), (1, 0)] {
            let (train, _) = &store[train_idx];
            let (test, _) = &store[test_idx];
            let mut tpl = vec![0.0f32; N * N];
            for f in train {
                let r = rotate(&resize(&f.mask, f.side, N), N, -f.truth);
                for k in 0..tpl.len() {
                    tpl[k] += r[k];
                }
            }
            let n = train.len() as f32;
            for v in tpl.iter_mut() {
                *v /= n;
            }
            let mut errs = Vec::new();
            for f in test {
                let m = resize(&f.mask, f.side, N);
                let (best, _) = match_best(
                    &Frame {
                        rgba: Vec::new(),
                        mask: m,
                        side: N,
                        truth: f.truth,
                    },
                    &tpl,
                    step,
                );
                errs.push(err(best, f.truth));
            }
            errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let gross = errs.iter().filter(|e| **e > 45.0).count();
            let name = if train_idx == 0 {
                "真机→视频"
            } else {
                "视频→真机"
            };
            println!(
                "  跨域 {name}  n={} 中位 {:.1}° 最大 {:.1}° 翻转 {}",
                errs.len(),
                errs[errs.len() / 2],
                errs.last().copied().unwrap_or(0.0),
                gross
            );
        }
    }
}
