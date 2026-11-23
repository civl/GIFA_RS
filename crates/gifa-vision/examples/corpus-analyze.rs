//! 分析一段"人工标注的真机语料"：v1 / v3 / 管线 各自多准、误差是常数偏置还是随朝向变化。
//!
//!   cargo run -p gifa-vision --example corpus-analyze -- <labels.json> <帧目录>
//!
//! 默认指向 `data/result/labels.json` + `data/corpus/m1-sub`。
//!
//! 为什么单独一个工具：老的语料只覆盖 315°~360° 一个扇区，
//! 在那种样本上"v1 偏 9°"分不清是常数偏置还是那个朝向特有的误差。
//! 这里按 45° 分桶打印**带符号**误差，一眼能看出来。

use gifa_vision::arrow::{analyze_with, ArrowParams, ArrowWorkspace};
use gifa_vision::cone::ConeWorkspace;
use gifa_vision::detect::detect_window;
use gifa_vision::hexagon::{direction_v3, direction_v5};

fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

fn find_frame(dir: &std::path::Path, seq: u64) -> Option<std::path::PathBuf> {
    let prefix = format!("f{seq:03}-");
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix) && n.ends_with(".bin"))
                .unwrap_or(false)
        })
}

struct Row {
    seq: u64,
    truth: f64,
    v1: Option<(f64, f64)>, // (罗盘角, angMargin)
    v3: Option<(f64, f64)>, // (罗盘角, span_px)
    v3_blunt: f64,
    v5: Option<(f64, f64)>,  // (罗盘角, span_px)
    pipe: Option<(f64, u8)>, // (罗盘角, source)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let labels = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "data/result/labels.json".into());
    let frames_dir = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "data/corpus/m1-sub".into());
    let txt = std::fs::read_to_string(&labels).expect("读不到 labels.json");
    let v: serde_json::Value = serde_json::from_str(&txt).expect("labels.json 格式不对");
    let dir = std::path::Path::new(&frames_dir);

    let params = ArrowParams {
        bis_err_max: 25.0,
        ang_margin_min: 18.0,
        ..Default::default()
    };
    let mut aws = ArrowWorkspace::default();
    let mut cws = ConeWorkspace::default();
    let mut rows = Vec::new();
    let mut missing = 0;

    for f in v["frames"].as_array().cloned().unwrap_or_default() {
        let Some(seq) = f["seq"].as_u64() else {
            continue;
        };
        if f["bad"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(truth) = f["truth"].as_f64() else {
            continue;
        };
        let Some(path) = find_frame(dir, seq) else {
            missing += 1;
            continue;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            missing += 1;
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let r = analyze_with(&bytes, side, 1, &mut aws, &params);
        let c = (side as f64 / 2.0, side as f64 / 2.0);
        let h = direction_v3(&bytes, side, c, 0.0);
        let p = detect_window(&bytes, side, &mut aws, &mut cws);
        rows.push(Row {
            seq,
            truth,
            v1: if r.valid {
                Some(((90.0 - r.angle).rem_euclid(360.0), r.ang_margin))
            } else {
                None
            },
            v3: h.map(|x| (x.deg, x.span)),
            v3_blunt: h.map(|x| x.tip_blunt).unwrap_or(0.0),
            v5: direction_v5(&bytes, side, c, 0.0, 2).map(|x| (x.deg, x.span)),
            pipe: p.arrow.map(|a| (a, p.arrow_source)),
        });
    }

    println!(
        "语料 {} 帧（缺帧 {}）← {} + {}",
        rows.len(),
        missing,
        labels,
        frames_dir
    );
    if rows.is_empty() {
        return;
    }

    let stat = |name: &str, vals: &[(f64, f64)]| {
        if vals.is_empty() {
            println!("  {name:<8} 覆盖 0");
            return;
        }
        let mut e: Vec<f64> = vals.iter().map(|(a, t)| ang_diff(*a, *t).abs()).collect();
        e.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gross = e.iter().filter(|x| **x > 45.0).count();
        println!(
            "  {name:<8} 覆盖 {:>3}/{}  中位 {:>5.1}°  p90 {:>5.1}°  最大 {:>6.1}°  >45° {}",
            vals.len(),
            rows.len(),
            e[e.len() / 2],
            e[(e.len() * 9) / 10],
            e[e.len() - 1],
            gross
        );
    };

    println!("\n== 总览 ==");
    stat(
        "v1",
        &rows
            .iter()
            .filter_map(|r| r.v1.map(|(a, _)| (a, r.truth)))
            .collect::<Vec<_>>(),
    );
    stat(
        "v3",
        &rows
            .iter()
            .filter_map(|r| r.v3.map(|(a, _)| (a, r.truth)))
            .collect::<Vec<_>>(),
    );
    stat(
        "闭运算r2",
        &rows
            .iter()
            .filter_map(|r| r.v5.map(|(a, _)| (a, r.truth)))
            .collect::<Vec<_>>(),
    );
    stat(
        "管线",
        &rows
            .iter()
            .filter_map(|r| r.pipe.map(|(a, _)| (a, r.truth)))
            .collect::<Vec<_>>(),
    );

    // 分朝向：带符号误差的均值/标准差。常数偏置 → 各桶均值接近且 std 小；
    // 朝向依赖 → 各桶均值明显不同。
    println!("\n== 分朝向（45° 桶）带符号误差：均值 ± 标准差（帧数）==");
    println!("  桶           v1                      v3");
    for b in (0..360).step_by(45) {
        let f = |x: &f64| (*x as i64 / 45) * 45 == b as i64;
        let mut line = format!("  {b:>3}°~{:<3}°  ", b + 45);
        for which in 0..2 {
            let vals: Vec<f64> = rows
                .iter()
                .filter(|r| f(&r.truth))
                .filter_map(|r| {
                    let a = if which == 0 {
                        r.v1.map(|(a, _)| a)
                    } else {
                        r.v3.map(|(a, _)| a)
                    };
                    a.map(|a| ang_diff(a, r.truth))
                })
                .collect();
            if vals.is_empty() {
                line.push_str("      —              ");
            } else {
                let n = vals.len() as f64;
                let m = vals.iter().sum::<f64>() / n;
                let sd = (vals.iter().map(|x| (x - m).powi(2)).sum::<f64>() / n).sqrt();
                line.push_str(&format!("{m:+7.1}° ± {sd:>5.1}° ({:>2})      ", vals.len()));
            }
        }
        println!("{line}");
    }

    // 箭头尺寸：决定主通道是 v1 还是 v3
    println!("\n== 箭头轴长（决定走 v1 还是 v3，阈值 40px）==");
    let mut spans: Vec<f64> = rows.iter().filter_map(|r| r.v3.map(|(_, s)| s)).collect();
    spans.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !spans.is_empty() {
        println!(
            "  中位 {:.1}px  最小 {:.1}px  最大 {:.1}px   ≥40px 的 {} / {}",
            spans[spans.len() / 2],
            spans[0],
            spans[spans.len() - 1],
            spans.iter().filter(|s| **s >= 40.0).count(),
            spans.len()
        );
    }

    // 管线用哪一路
    println!("\n== 管线来源分布 ==");
    let mut srcs: std::collections::BTreeMap<u8, u32> = Default::default();
    for r in &rows {
        if let Some((_, s)) = r.pipe {
            *srcs.entry(s).or_insert(0) += 1;
        }
    }
    for (s, n) in srcs {
        println!(
            "  src={s}（{}）: {n} 帧",
            match s {
                1 => "v1",
                2 => "v3",
                3 => "融合",
                _ => "?",
            }
        );
    }

    // 逐帧列出大误差，看清是翻转(±180)还是轴错(~90)
    println!("\n== 偏 >45° 的帧（误差符号能区分翻转/轴错）==");
    println!("  seq    真值      v1       v3      管道    v3误差   v3轴长");
    for r in rows.iter() {
        let e = r.v3.map(|(a, _)| ang_diff(a, r.truth)).unwrap_or(f64::NAN);
        if e.is_nan() || e.abs() <= 45.0 {
            continue;
        }
        println!(
            "  {:>3}  {:>6.1}°  {:>7}  {:>6.1}°  {:>7}  {:+7.1}°  {:>5.1}px",
            r.seq,
            r.truth,
            r.v1.map(|(a, _)| format!("{a:.1}°")).unwrap_or("—".into()),
            r.v3.unwrap().0,
            r.pipe
                .map(|(a, _)| format!("{a:.1}°"))
                .unwrap_or("—".into()),
            e,
            r.v3.unwrap().1
        );
    }

    /* ---- 形状可疑（尖端被路线条切掉）的帧统计
     *
     * 这类帧 v3 会给出 ~120° 的错值。detect 把它们的置信度压到 0.2，
     * 于是滤波层"连续 2 帧 + conf≥0.5"的确认路径走不通，会保持上一帧。 */
    let flagged: Vec<&Row> = rows.iter().filter(|r| r.v3_blunt >= 0.85).collect();
    println!(
        "\n== 尖端钝度 ≥0.85（标记可疑）==\n  {} 帧 / {}",
        flagged.len(),
        rows.len()
    );
    let bad_flagged = flagged
        .iter()
        .filter(|r| {
            r.v3.map(|(a, _)| ang_diff(a, r.truth).abs() > 45.0)
                .unwrap_or(false)
        })
        .count();
    println!("  其中确实偏 >45° 的 {bad_flagged} 帧");

    // v1 的 angMargin 分布：看门槛 18 卡在哪
    let mut margins: Vec<f64> = rows.iter().filter_map(|r| r.v1.map(|(_, m)| m)).collect();
    margins.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if margins.is_empty() {
        println!("\n== v1 angMargin ==\n  没有任何帧通过 v1 的门");
    } else {
        println!(
            "\n== v1 angMargin（门槛 18）==\n  中位 {:.1}  最小 {:.1}  最大 {:.1}",
            margins[margins.len() / 2],
            margins[0],
            margins[margins.len() - 1]
        );
    }
}
