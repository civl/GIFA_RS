//! 一致性融合的**冷启动**评估：从零开始，边跑边自举形状先验。
//!
//!   cargo run -p gifa-vision --example fusion-eval
//!
//! 分两段统计：
//!   · 冷启动段（先验还没攒够）→ 只有 v3，与今天的行为一致
//!   · 预热后段（先验可用）    → v3 + 模板互证
//!
//! 语料走 `include_bytes!` 编译期嵌入（本机沙箱下子进程读不了文件）。

#[path = "support/m1_corpus.rs"]
mod m1_corpus;

use gifa_vision::arrow::ArrowWorkspace;
use gifa_vision::cone::ConeWorkspace;
use gifa_vision::detect::{detect_window, detect_window_prior};
use gifa_vision::prior::ShapePrior;

fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

struct Stat {
    n: usize,
    med: f64,
    p90: f64,
    max: f64,
    gross: usize,
    dropped: usize,
    total: usize,
}

fn stat(mut e: Vec<f64>, dropped: usize, total: usize) -> Stat {
    e.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = e.len();
    if n == 0 {
        return Stat {
            n: 0,
            med: f64::NAN,
            p90: f64::NAN,
            max: f64::NAN,
            gross: 0,
            dropped,
            total,
        };
    }
    Stat {
        n,
        med: e[n / 2],
        p90: e[(n * 9) / 10],
        max: e[n - 1],
        gross: e.iter().filter(|x| **x > 45.0).count(),
        dropped,
        total,
    }
}

impl Stat {
    fn line(&self, label: &str) {
        println!(
            "  {label:<14} 覆盖 {:>2}/{}  中位 {:>5.1}°  p90 {:>6.1}°  最大 {:>6.1}°  >45° {}  丢弃 {}",
            self.n, self.total, self.med, self.p90, self.max, self.gross, self.dropped
        );
    }
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

    let mut aws = ArrowWorkspace::default();
    let mut cws = ConeWorkspace::default();
    let mut prior = ShapePrior::new();

    let (mut cold_err, mut warm_err, mut base_err) = (Vec::new(), Vec::new(), Vec::new());
    let (mut cold_drop, mut warm_drop, mut cold_n, mut warm_n) = (0, 0, 0, 0);
    let mut ready_at = None;

    for (seq, bytes) in m1_corpus::FRAMES {
        let Some(&truth) = truth_of.get(seq) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let warm = prior.ready();

        // 基线：不带先验（= 今天的行为）
        let base = detect_window(bytes, side, &mut aws, &mut cws);
        if let Some(a) = base.arrow {
            base_err.push(ang_diff(a, truth).abs());
        }

        // 带先验
        let out = detect_window_prior(bytes, side, &mut aws, &mut cws, &mut prior);
        if warm {
            warm_n += 1;
            match out.arrow {
                Some(a) => warm_err.push(ang_diff(a, truth).abs()),
                None => warm_drop += 1,
            }
        } else {
            cold_n += 1;
            match out.arrow {
                Some(a) => cold_err.push(ang_diff(a, truth).abs()),
                None => cold_drop += 1,
            }
        }
        if ready_at.is_none() && prior.ready() {
            ready_at = Some(*seq);
        }
    }

    println!(
        "语料 {} 帧；先验在 seq {:?} 处攒够 {} 帧",
        cold_n + warm_n,
        ready_at,
        gifa_vision::prior::MIN_FRAMES
    );
    let total = cold_n + warm_n;
    stat(base_err, 0, total).line("基线（无先验）");
    stat(cold_err, cold_drop, cold_n).line("冷启动段");
    stat(warm_err, warm_drop, warm_n).line("预热后段");

    /* ---- 稳态：先用整段自举一遍，再全量评估
     *
     * 冷启动段里那 4 个遮挡帧（25/161/169/177）都出现在攒够 8 帧之前，
     * 所以上表没机会看到融合去修它们。这里补一条稳态数据。
     * 注意自举只吸收"v1 ∧ v3 一致"的帧，而遮挡帧上 v1 要么不出、要么与 v3 分歧，
     * 所以它们不会被当成训练样本（不会自我污染）。
     */
    let mut prior2 = ShapePrior::new();
    for (seq, bytes) in m1_corpus::FRAMES {
        if !truth_of.contains_key(seq) {
            continue;
        }
        let side = (bytes.len() / 4).isqrt();
        // 只跑检测以喂先验，结果丢弃
        let _ = detect_window_prior(bytes, side, &mut aws, &mut cws, &mut prior2);
    }
    let mut steady_err = Vec::new();
    let mut steady_drop = 0;
    let mut gross_rows: Vec<String> = Vec::new();
    let mut gross_truths: Vec<(u64, f64)> = Vec::new();
    let t0 = std::time::Instant::now();
    for (seq, bytes) in m1_corpus::FRAMES {
        let Some(&truth) = truth_of.get(seq) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let out = detect_window_prior(bytes, side, &mut aws, &mut cws, &mut prior2);
        match out.arrow {
            Some(a) => {
                let e = ang_diff(a, truth).abs();
                steady_err.push(e);
                if e > 45.0 {
                    gross_rows.push(format!(
                        "  seq {seq}: err {e:.1} src {}  prior {:?}  prec {:.2} recall {:.2}  v3 {:?}",
                        out.arrow_source, out.prior, out.prior_precision, out.prior_recall, out.v3_diag
                    ));
                    gross_truths.push((*seq, truth));
                }
            }
            None => {
                steady_drop += 1;
                if let Some(v3) = out.v3_diag {
                    let e = ang_diff(v3, truth).abs();
                    if e > 45.0 {
                        gross_rows.push(format!(
                            "  seq {seq}: 丢弃（v3 错 {e:.1}°）  prior {:?}  prec {:.2} recall {:.2}",
                            out.prior, out.prior_precision, out.prior_recall
                        ));
                        gross_truths.push((*seq, truth));
                    }
                }
            }
        }
    }
    let with_prior = t0.elapsed().as_secs_f64() * 1000.0 / m1_corpus::FRAMES.len() as f64;

    let t1 = std::time::Instant::now();
    let mut n_base = 0;
    for (seq, bytes) in m1_corpus::FRAMES {
        if !truth_of.contains_key(seq) {
            continue;
        }
        let side = (bytes.len() / 4).isqrt();
        let _ = detect_window(bytes, side, &mut aws, &mut cws);
        n_base += 1;
    }
    let base_ms = t1.elapsed().as_secs_f64() * 1000.0 / n_base.max(1) as f64;

    println!("  （先验攒了 {} 帧）", prior2.frames());
    stat(steady_err, steady_drop, total).line("稳态（全量）");
    if !gross_rows.is_empty() {
        println!("  大错帧明细:");
        for r in &gross_rows {
            println!("{}", r);
        }
        // 对第一个大错帧打印"真值角度上的得分 vs 匹配选出的得分"
        if let (Some(&(seq_bad, truth)), Some(_bytes)) = (
            gross_truths.first(),
            gross_truths
                .first()
                .and_then(|(s, _)| m1_corpus::FRAMES.iter().find(|(fs, _)| fs == s))
                .map(|(_, b)| *b),
        ) {
            let bytes = m1_corpus::FRAMES
                .iter()
                .find(|(s, _)| *s == seq_bad)
                .map(|(_, b)| *b);
            if let Some(bytes) = bytes {
                let side = (bytes.len() / 4).isqrt();
                println!("  seq {seq_bad} 在各角度的得分（precision/recall）:");
                for probe in [
                    ("真值", truth),
                    (
                        "prior 选的",
                        prior2
                            .match_frame(bytes, side, 15.0)
                            .map(|m| m.deg)
                            .unwrap_or(0.0),
                    ),
                ] {
                    if let Some((p, r)) = prior2.precision_at(bytes, side, probe.1) {
                        println!(
                            "    {} {:>6.1}° → precision {p:.3} recall {r:.3}",
                            probe.0, probe.1
                        );
                    }
                }
                if let Some(render) = prior2.debug_render(bytes, side) {
                    println!(
                        "    模板 | 候选(seq {seq_bad}) | 交集(T=只有模板 C=只有候选 = =都有):"
                    );
                    for line in render.lines() {
                        println!("    {line}");
                    }
                }
            }
        }
    }
    /* ---- 对照：用**人工真值**建先验（oracle）
     *
     * 区分"自举模板质量不行"和"度量/裁切本身不行"：
     * oracle 能修好那 4 帧 → 问题在自举；修不好 → 问题在度量。
     */
    let mut oracle = ShapePrior::new();
    for (seq, bytes) in m1_corpus::FRAMES {
        let Some(&truth) = truth_of.get(seq) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        oracle.observe(bytes, side, truth);
    }
    let mut oe = Vec::new();
    let mut od = 0;
    let mut og = Vec::new();
    for (seq, bytes) in m1_corpus::FRAMES {
        let Some(&truth) = truth_of.get(seq) else {
            continue;
        };
        let side = (bytes.len() / 4).isqrt();
        let out = detect_window_prior(bytes, side, &mut aws, &mut cws, &mut oracle);
        match out.arrow {
            Some(a) => {
                let e = ang_diff(a, truth).abs();
                oe.push(e);
                if e > 45.0 {
                    og.push(*seq);
                }
            }
            None => od += 1,
        }
    }
    println!("  （oracle 先验用 {} 帧建）", oracle.frames());
    stat(oe, od, total).line("oracle 先验");
    if !og.is_empty() {
        println!("  oracle 仍大错的帧: {og:?}");
    }

    println!(
        "  耗时：基线 {base_ms:.2}ms/帧，带先验 {with_prior:.2}ms/帧（+{:.0}%）",
        (with_prior / base_ms.max(1e-6) - 1.0) * 100.0
    );
}
