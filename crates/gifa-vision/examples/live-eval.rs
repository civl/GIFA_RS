//! 在**已录制的实机帧**上重放新算法（离线），看丢弃率与一致性。
//!
//!   cargo run -p gifa-vision --example live-eval
//!
//! 帧来自一次 95 秒的真机采集（原地转视角），把青色遮罩位打包后编译期嵌入，
//! 运行期从遮罩还原 RGBA 再走完整管线。还原是无损的（见 scripts/pack-live-masks.py）。
//!
//! 关注的指标（这些帧没有人工真值，所以看的是"行为"而不是"精度"）：
//!   · 丢弃率：融合分歧导致这一帧不上报的比例（**首版实测 52%，过长**）
//!   · v3 与先验的一致率
//!   · 帧间 >90° 跳变次数（越少越稳）

use gifa_vision::arrow::ArrowWorkspace;
use gifa_vision::cone::ConeWorkspace;
use gifa_vision::detect::{detect_window, detect_window_prior};
use gifa_vision::prior::ShapePrior;

const PACKED: &[u8] = include_bytes!("../../../docs/calibration/live-masks/packed.bin");

struct Frames {
    side: usize,
    list: Vec<(u32, Vec<u8>)>,
}

fn parse() -> Option<Frames> {
    if PACKED.len() < 16 || &PACKED[..8] != b"GMASK1\0\0" {
        return None;
    }
    let n = u32::from_le_bytes(PACKED[8..12].try_into().ok()?) as usize;
    let side = u32::from_le_bytes(PACKED[12..16].try_into().ok()?) as usize;
    let per = (side * side).div_ceil(8);
    let mut off = 16;
    let mut list = Vec::with_capacity(n);
    for _ in 0..n {
        if off + 4 + per > PACKED.len() {
            break;
        }
        let seq = u32::from_le_bytes(PACKED[off..off + 4].try_into().ok()?);
        off += 4;
        list.push((seq, PACKED[off..off + per].to_vec()));
        off += per;
    }
    Some(Frames { side, list })
}

/// 从位打包遮罩还原 RGBA：青色像素 (0,255,255)、背景 (40,93,103)
fn rgba_of(packed: &[u8], side: usize) -> Vec<u8> {
    let mut v = vec![0u8; side * side * 4];
    for i in 0..side * side {
        let bit = (packed[i / 8] >> (7 - (i % 8))) & 1;
        let (r, g, b) = if bit == 1 {
            (0u8, 255u8, 255u8)
        } else {
            (40u8, 93u8, 103u8)
        };
        v[i * 4] = r;
        v[i * 4 + 1] = g;
        v[i * 4 + 2] = b;
        v[i * 4 + 3] = 255;
    }
    v
}

fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

fn jumps(v: &[Option<f64>]) -> usize {
    let idx: Vec<usize> = (0..v.len()).filter(|i| v[*i].is_some()).collect();
    (1..idx.len())
        .filter(|k| ang_diff(v[idx[*k]].unwrap(), v[idx[k - 1]].unwrap()).abs() > 90.0)
        .count()
}

fn main() {
    let Some(fr) = parse() else {
        println!("没有打包帧（先跑 scripts/pack-live-masks.py）");
        return;
    };
    println!("实机帧 {}（边长 {}）", fr.list.len(), fr.side);

    let mut aws = ArrowWorkspace::default();
    let mut cws = ConeWorkspace::default();
    let mut prior = ShapePrior::new();

    let mut base_out = Vec::new();
    let mut fused_out = Vec::new();
    let mut srcs: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut drops = 0usize;
    let mut ready_at = None;
    let (mut agree, mut disagree, mut prior_none) = (0, 0, 0);
    let mut drop_run = (0usize, 0usize); // 当前长度, 最长

    for (i, (seq, packed)) in fr.list.iter().enumerate() {
        let rgba = rgba_of(packed, fr.side);
        let base = detect_window(&rgba, fr.side, &mut aws, &mut cws);
        base_out.push(base.arrow);

        let out = detect_window_prior(&rgba, fr.side, &mut aws, &mut cws, &mut prior);
        fused_out.push(out.arrow);
        *srcs.entry(out.arrow_source).or_insert(0) += 1;

        if prior.ready() {
            if ready_at.is_none() {
                ready_at = Some(i);
            }
            match (out.v3_diag, out.prior) {
                (Some(v), Some(p)) => {
                    if ang_diff(v, p).abs() <= 30.0 {
                        agree += 1;
                    } else {
                        disagree += 1;
                    }
                }
                _ => prior_none += 1,
            }
        }
        if out.arrow.is_none() {
            drops += 1;
            drop_run.0 += 1;
            drop_run.1 = drop_run.1.max(drop_run.0);
        } else {
            drop_run.0 = 0;
        }
        let _ = seq;
    }

    let n = fr.list.len();
    println!(
        "先验在第 {:?} 帧攒够；预热后 v3/先验 一致 {agree}、分歧 {disagree}、先验缺席 {prior_none}",
        ready_at
    );
    println!("来源分布（0=无 1=v1 2=v3 5=融合 6=丢弃 7=分歧用v3）: {srcs:?}");
    println!(
        "丢弃 {drops}/{n}（{:.0}%），最长连续丢弃 {} 帧",
        100.0 * drops as f64 / n as f64,
        drop_run.1
    );
    println!(
        "帧间 >90° 跳变：基线 {} 次，带先验 {} 次",
        jumps(&base_out),
        jumps(&fused_out)
    );
    println!(
        "上报覆盖率：基线 {}，带先验 {}",
        base_out.iter().filter(|x| x.is_some()).count(),
        fused_out.iter().filter(|x| x.is_some()).count()
    );
}
