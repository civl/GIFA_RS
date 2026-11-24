//! 同域模板匹配（遮挡鲁棒版）实验。
//!
//!   cargo run -p gifa-vision --example prior-eval -- [角度步长]
//!
//! 核心思路：**遮挡只会减少青色，不会增加青色**。
//! 所以用「候选青色里有 90% 被模板解释掉」这种**精确率**当分数，
//! 而不是对称的 NCC / F1 —— 被路线条盖掉一块的箭头，精确率仍然是 1.0。
//!
//!   precision = |T_rot ∩ C| / |C|   ← 遮挡不变
//!   recall    = |T_rot ∩ C| / |T|   ← 遮挡会降低，用来当"至少对上多少"的闸
//!
//! 模板用**留一交叉验证**建：把其他帧按真值反向旋到统一朝向再平均。
//!
//! 语料走 `include_bytes!` 嵌入（见 `m1_corpus.rs`），因为本机沙箱下
//! 子进程读不了文件系统。这是实验脚手架，不影响库代码。

#[path = "support/m1_corpus.rs"]
mod m1_corpus;

use gifa_vision::hexagon::direction_v3;

const HALF: usize = 32; // 中心 64×64 裁切，真机箭头 ~35px 足够

fn mask_of(rgba: &[u8], size: usize) -> Vec<f32> {
    let n = size * size;
    let mut score = vec![0i16; n];
    let mut hist = [0u32; 256];
    for i in 0..n {
        let r = rgba[i * 4] as i32;
        let g = rgba[i * 4 + 1] as i32;
        let b = rgba[i * 4 + 2] as i32;
        let s = (g.min(b) - r).clamp(-255, 255);
        score[i] = s as i16;
        if s >= 0 {
            hist[s as usize] += 1;
        }
    }
    let total: u32 = hist.iter().sum();
    let mut acc = 0u32;
    let mut p99 = 0i32;
    for (s, n) in hist.iter().enumerate() {
        acc += n;
        if acc as f64 >= total as f64 * 0.99 {
            p99 = s as i32;
            break;
        }
    }
    let thr = ((p99 as f64) * 0.74).max(112.0);
    score
        .iter()
        .map(|v| if (*v as f64) >= thr { 1.0 } else { 0.0 })
        .collect()
}

/// 以给定中心裁切 64×64；`center=None` 时用窗口中心。
///
/// 为什么需要按质心裁：箭头并不在采样窗正中（真机实测偏 5~7px）。
/// 若按窗口中心裁，56 帧叠出来的模板会糊成一团，匹配时哪个角度都对不上
/// （实测最高精确率只有 0.74）。
fn crop_at(m: &[f32], side: usize, center: Option<(f64, f64)>) -> Vec<f32> {
    let mut out = vec![0.0f32; (2 * HALF) * (2 * HALF)];
    let (cx, cy) = center.unwrap_or(((side as f64 - 1.0) / 2.0, (side as f64 - 1.0) / 2.0));
    let c0x = cx.round() as i32;
    let c0y = cy.round() as i32;
    for y in 0..(2 * HALF) {
        for x in 0..(2 * HALF) {
            let sy = c0y - HALF as i32 + y as i32;
            let sx = c0x - HALF as i32 + x as i32;
            if sy < 0 || sx < 0 || sy >= side as i32 || sx >= side as i32 {
                continue;
            }
            out[y * 2 * HALF + x] = m[sy as usize * side + sx as usize];
        }
    }
    out
}

/// 掩码质心（全窗口坐标）
fn centroid(m: &[f32], side: usize) -> Option<(f64, f64)> {
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    for (i, v) in m.iter().enumerate() { if *v > 0.5 { sx += (i % side) as f64; sy += (i / side) as f64; n += 1.0; } }
    if n < 20.0 {
        None
    } else {
        Some((sx / n, sy / n))
    }
}

fn rotate(src: &[f32], deg: f64) -> Vec<f32> {
    let n = 2 * HALF;
    let c = (n as f64 - 1.0) / 2.0;
    let (s, co) = (deg.to_radians().sin(), deg.to_radians().cos());
    let mut out = vec![0.0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let dx = x as f64 - c;
            let dy = y as f64 - c;
            let sx = c + dx * co + dy * s;
            let sy = c - dx * s + dy * co;
            if sx < 0.0 || sy < 0.0 || sx >= n as f64 - 1.0 || sy >= n as f64 - 1.0 {
                continue;
            }
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
            let g = |xx: usize, yy: usize| src[yy * n + xx];
            out[y * n + x] = g(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + g(x0 + 1, y0) * fx * (1.0 - fy)
                + g(x0, y0 + 1) * (1.0 - fx) * fy
                + g(x0 + 1, y0 + 1) * fx * fy;
        }
    }
    out
}

/// 返回 (precision, recall)
fn score_rot(t: &[f32], c: &[f32]) -> (f64, f64) {
    let (mut hit, mut tn, mut cn) = (0usize, 0usize, 0usize);
    for i in 0..t.len() {
        let ti = t[i] >= 0.5;
        let ci = c[i] >= 0.5;
        if ti {
            tn += 1;
        }
        if ci {
            cn += 1;
        }
        if ti && ci {
            hit += 1;
        }
    }
    let p = if cn > 0 { hit as f64 / cn as f64 } else { 0.0 };
    let r = if tn > 0 { hit as f64 / tn as f64 } else { 0.0 };
    (p, r)
}

fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 * q) as usize).min(v.len() - 1)]
}

fn main() {
    let v: serde_json::Value = serde_json::from_slice(m1_corpus::LABELS).expect("m1-labels.json");
    let mut truth_of = std::collections::BTreeMap::new();
    for f in v["frames"].as_array().cloned().unwrap_or_default() {
        if f["bad"].as_bool().unwrap_or(false) {
            continue;
        }
        if let (Some(seq), Some(t)) = (f["seq"].as_u64(), f["truth"].as_f64()) {
            truth_of.insert(seq, t);
        }
    }

    let mut frames: Vec<(u64, f64, Vec<f32>, &[u8])> = Vec::new();
    let mut offs = Vec::new();
    for (seq, bytes) in m1_corpus::FRAMES {
        let Some(&truth) = truth_of.get(seq) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let m = mask_of(bytes, side);
        let Some(c) = centroid(&m, side) else {
            continue;
        };
        let wc = (side as f64 - 1.0) / 2.0;
        offs.push(((c.0 - wc).powi(2) + (c.1 - wc).powi(2)).sqrt());
        frames.push((*seq, truth, crop_at(&m, side, Some(c)), *bytes));
    }
    if frames.is_empty() {
        println!("没有帧");
        return;
    }
    offs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "语料 {} 帧（裁切 {}×{}，按质心对齐；质心离窗心中位 {:.1}px）",
        frames.len(),
        2 * HALF,
        2 * HALF,
        offs[offs.len() / 2]
    );

    let step: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5.0);
    // 模板二值化阈值：平均后 >= 该值的像素算"模板内部"
    let tpl_thr: f32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5);
    // 召回门：模板至少被候选覆盖到这个比例，否则判"只对上一点点"出局
    let recall_min: f64 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.25);
    let angles: Vec<f64> = (0..(360.0 / step).round() as i32)
        .map(|i| i as f64 * step)
        .collect();

    let mut errs: Vec<(u64, f64, f64)> = Vec::new();
    let mut tpl_areas: Vec<usize> = Vec::new();
    let mut cand_areas: Vec<usize> = Vec::new();
    for i in 0..frames.len() {
        // 留一：模板用其余帧建
        let mut tpl = vec![0.0f32; (2 * HALF) * (2 * HALF)];
        let mut n = 0.0f32;
        for (j, (_, truth, m, _)) in frames.iter().enumerate() {
            if i == j {
                continue;
            }
            let r = rotate(m, -truth);
            for k in 0..tpl.len() {
                tpl[k] += r[k];
            }
            n += 1.0;
        }
        for x in tpl.iter_mut() {
            *x /= n.max(1.0);
        }
        let tpl_bin: Vec<f32> = tpl
            .iter()
            .map(|x| if *x >= tpl_thr { 1.0 } else { 0.0 })
            .collect();
        tpl_areas.push(tpl_bin.iter().filter(|x| **x > 0.5).count());
        cand_areas.push(frames[i].2.iter().filter(|x| **x > 0.5).count());

        let (seq, truth, cand, _) = &frames[i];
        let mut best = (f64::NEG_INFINITY, 0.0f64);
        let mut curve: Vec<(f64, f64, f64)> = Vec::new();
        for &a in &angles {
            let t = rotate(&tpl_bin, a);
            let (p, r) = score_rot(&t, cand);
            curve.push((a, p, r));
            let s = if r < recall_min { -1.0 } else { p };
            if s > best.0 {
                best = (s, a);
            }
        }
        // 第一个帧把曲线打出来，方便判断打分是否退化
        if i == 0 && std::env::args().nth(4).as_deref() == Some("dump") {
            let mut top = curve.clone();
            top.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap());
            println!("  诊断 seq {seq}（真值 {truth:.1}°）precision 前 6：");
            for (a, p, r) in top.iter().take(6) {
                println!("    a={a:>6.1}°  precision {p:.3}  recall {r:.3}");
            }
            let rs: Vec<f64> = curve.iter().map(|x| x.2).collect();
            println!(
                "    recall 范围 {:.3}..{:.3}",
                rs.iter().cloned().fold(f64::MAX, f64::min),
                rs.iter().cloned().fold(0.0, f64::max)
            );
        }
        if i == 0 {
            tpl_areas.sort_unstable();
            let a = tpl_areas.len() / 2;
            cand_areas.sort_unstable();
            println!(
                "诊断：模板中位面积 {} px，候选中位面积 {} px（阈值 {tpl_thr}、召回门 {recall_min}）",
                tpl_areas[a], cand_areas[cand_areas.len() / 2]
            );
        }
        errs.push((
            *seq,
            ang_diff(best.1, *truth).abs(),
            ang_diff(best.1, *truth),
        ));
    }

    let mut e: Vec<f64> = errs.iter().map(|(_, d, _)| *d).collect();
    let n = e.len();
    let med = pct(&mut e, 0.5);
    let p90 = pct(&mut e, 0.9);
    let max = e[n - 1];
    let gross: Vec<u64> = errs
        .iter()
        .filter(|(_, d, _)| *d > 45.0)
        .map(|(s, _, _)| *s)
        .collect();
    println!(
        "\n模板匹配（遮挡鲁棒，步长 {step}°，留一）：\n  中位 {med:.1}°  p90 {p90:.1}°  最大 {max:.1}°  >45° {} 帧 {:?}",
        gross.len(),
        gross
    );

    // 对照：v3 在同一批帧上，并做"只有两者一致才上报"的融合
    let mut v3e: Vec<f64> = Vec::new();
    let mut v3g = Vec::new();
    let mut fused_e: Vec<f64> = Vec::new();
    let mut fused_drop = 0usize;
    for (seq, truth, _, bytes) in &frames {
        let side = (bytes.len() / 4).isqrt();
        let c = (side as f64 / 2.0, side as f64 / 2.0);
        let Some(d) = direction_v3(bytes, side, c, 0.0) else {
            continue;
        };
        let err = ang_diff(d.deg, *truth).abs();
        v3e.push(err);
        if err > 45.0 {
            v3g.push(*seq);
        }
        // 模板匹配（留一）的结果已在 errs 里
        if let Some((_, _, signed)) = errs.iter().find(|(s, _, _)| s == seq) {
            let tpl_deg = truth + signed;
            if ang_diff(tpl_deg, d.deg).abs() <= 30.0 {
                // 两者一致 → 取平均（等价于把噪声压一半）
                let avg = (tpl_deg + ang_diff(d.deg, tpl_deg) / 2.0 + 720.0) % 360.0;
                fused_e.push(ang_diff(avg, *truth).abs());
            } else {
                fused_drop += 1;
            }
        }
    }
    let n3 = v3e.len();
    let m3 = pct(&mut v3e, 0.5);
    let p3 = pct(&mut v3e, 0.9);
    println!(
        "  v3 对照：中位 {m3:.1}°  p90 {p3:.1}°  最大 {:.1}°  >45° {} 帧 {:?}",
        v3e[n3 - 1],
        v3g.len(),
        v3g
    );
    let nf = fused_e.len();
    let mf = pct(&mut fused_e, 0.5);
    let pf = pct(&mut fused_e, 0.9);
    println!(
        "  融合（一致才上报）：覆盖 {nf}/{}  中位 {mf:.1}°  p90 {pf:.1}°  最大 {:.1}°  >45° {}  丢弃 {fused_drop} 帧",
        frames.len(),
        fused_e[nf - 1],
        fused_e.iter().filter(|x| **x > 45.0).count()
    );

    println!("\n遮挡帧（25/161/169/177）逐帧对比：");
    for (seq, d, _) in &errs {
        if [25u64, 161, 169, 177].contains(seq) {
            println!("  seq {seq:>3}  模板 {d:6.1}°");
        }
    }

    // 大错帧的带符号误差：±180 说明是"符号翻转"，其它说明是形状/度量问题
    println!("\n模板匹配的大错帧（带符号误差）：");
    for (seq, d, _) in &errs {
        if *d > 45.0 {
            println!("  seq {seq:>3}  误差 {d:6.1}°");
        }
    }
}
