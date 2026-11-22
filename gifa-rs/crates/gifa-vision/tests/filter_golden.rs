//! 时序层对拍：同一串观测，原版 JS 与 Rust 的输出逐帧一致。
//!
//! 素材：`node tools/vision-ref/harness.js seq tools/vision-ref/filter-seq.json`
//! 序列包含缓慢转向、5% 单帧 180° 假峰、7% 低置信、15% 扇形漏检、地图消失等情形。

use std::path::PathBuf;

use gifa_vision::filter::{AbFilter, ConeObs, ViewState};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Step {
    now: f64,
    obs: f64,
    conf: f64,
    #[serde(rename = "coneDeg")]
    cone_deg: f64,
    #[serde(rename = "coneConf")]
    cone_conf: f64,
    #[serde(rename = "coneValid")]
    cone_valid: bool,
    #[serde(rename = "mapPresent")]
    map_present: bool,
}

#[derive(Debug, Deserialize)]
struct ViewOut {
    deg: f64,
    conf: f64,
    #[serde(rename = "src")]
    src: String,
}

#[derive(Debug, Deserialize)]
struct Seq {
    steps: Vec<Step>,
    /// `null` = 这一帧被门限挡下
    smooth: Vec<Option<f64>>,
    view: Vec<Option<ViewOut>>,
}

fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

#[test]
fn filter_matches_javascript_on_sequence() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/vision-ref/filter-seq.json");
    if !path.exists() {
        eprintln!("跳过时序对拍：没有 {}（先跑 harness.js seq）", path.display());
        return;
    }
    let seq: Seq =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("读 filter-seq.json"))
            .expect("解析");

    let mut ab = AbFilter::default();
    let mut view = ViewState::default();
    let mut max_angle_err = 0.0f64;

    for (i, st) in seq.steps.iter().enumerate() {
        let got = ab.smooth(st.obs, st.conf, st.now, st.cone_deg, st.cone_conf);
        let want = seq.smooth[i];
        match (got, want) {
            (None, None) => {}
            (Some(g), Some(w)) => {
                let err = ang_diff(g, w).abs();
                max_angle_err = max_angle_err.max(err);
                assert!(err < 1e-9, "第 {i} 帧平滑角度差 {err}（Rust {g} / JS {w}）");
            }
            (g, w) => panic!("第 {i} 帧是否被挡不一致：Rust {g:?} / JS {w:?}"),
        }

        let cone = st.cone_valid.then_some(ConeObs {
            deg: st.cone_deg,
            valid: true,
            conf: st.cone_conf,
            width: 90.0,
        });
        let v = view.update(cone, st.now, st.map_present);
        match (v, &seq.view[i]) {
            (None, None) => {}
            (Some(g), Some(w)) => {
                let err = ang_diff(g.deg, w.deg).abs();
                max_angle_err = max_angle_err.max(err);
                assert!(
                    err < 1e-9,
                    "第 {i} 帧视角角度差 {err}（Rust {} / JS {}）",
                    g.deg,
                    w.deg
                );
                assert!(
                    (g.conf - w.conf).abs() < 1e-9,
                    "第 {i} 帧视角 conf 不一致（Rust {} / JS {}）",
                    g.conf,
                    w.conf
                );
                assert_eq!(g.src.as_str(), w.src, "第 {i} 帧视角来源不一致");
            }
            (g, w) => panic!("第 {i} 帧视角是否为空不一致：Rust {g:?} / JS {w:?}"),
        }
    }

    println!(
        "时序对拍 {} 步全部一致：最大角度差 {max_angle_err:.2e}°",
        seq.steps.len()
    );
}
