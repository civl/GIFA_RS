//! 与原版 JS 的**对拍测试**：同一批帧，参考实现 vs Rust 移植，逐项比对。
//!
//! 素材由 `scripts/vision-golden.ps1`（或手动两步）生成：
//!   1. `node tools/vision-ref/harness.js gen tools/vision-ref/frames`
//!   2. `node tools/vision-ref/harness.js batch tools/vision-ref/frames`
//!
//! 没有素材时测试**跳过**（打印一行说明），不让"没跑过 node"变成红叉 ——
//! 但 CI 上应该先跑脚本再跑测试。

use std::path::{Path, PathBuf};

use gifa_vision::arrow::{analyze, ArrowWorkspace};
use gifa_vision::cone::{analyze_cone, ConeWorkspace};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    size: usize,
    /// 生成的箭头方向（罗盘角）；空帧为 null（未用，仅记录）
    #[allow(dead_code)]
    theta: Option<f64>,
    arrow: Expected,
    cone: ConeExpected,
}

#[derive(Debug, Deserialize)]
struct ConeExpected {
    valid: bool,
    deg: f64,
    conf: f64,
}

#[derive(Debug, Deserialize)]
struct Expected {
    valid: bool,
    angle: f64,
    conf: f64,
    cyans: u32,
    #[serde(rename = "tipFrac")]
    tip_frac: f64,
    #[serde(rename = "angMargin")]
    ang_margin: f64,
    #[serde(rename = "radMargin")]
    rad_margin: f64,
    #[serde(rename = "noseFlat")]
    nose_flat: f64,
}

fn frames_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/vision-ref/frames")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("tools/vision-ref/frames"))
}

fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

#[test]
fn matches_javascript_reference_on_golden_frames() {
    let dir = frames_dir();
    let expected_path = dir.join("expected.json");
    if !expected_path.exists() {
        eprintln!(
            "跳过对拍：没有 {}（先跑 scripts/vision-golden.ps1）",
            expected_path.display()
        );
        return;
    }
    let text = std::fs::read_to_string(&expected_path).expect("读 expected.json");
    let cases: Vec<Case> = serde_json::from_str(&text).expect("解析 expected.json");
    assert!(!cases.is_empty(), "对拍素材为空");

    let mut checked = 0;
    let mut max_angle_err = 0.0f64;
    let mut max_conf_err = 0.0f64;
    let mut max_cone_err = 0.0f64;

    for c in &cases {
        let bytes =
            std::fs::read(dir.join(&c.name)).unwrap_or_else(|e| panic!("读 {}: {e}", c.name));
        let mut ws = ArrowWorkspace::default();
        let got = analyze(&bytes, c.size, 1, &mut ws);

        assert_eq!(
            got.cyans, c.arrow.cyans,
            "{}: 青色像素数不一致（Rust {} / JS {}）",
            c.name, got.cyans, c.arrow.cyans
        );
        assert_eq!(
            got.valid, c.arrow.valid,
            "{}: valid 不一致（Rust {} / JS {}）",
            c.name, got.valid, c.arrow.valid
        );

        if c.arrow.valid {
            let err = ang_diff(got.angle, c.arrow.angle).abs();
            max_angle_err = max_angle_err.max(err);
            assert!(
                err < 0.05,
                "{}: 角度差 {err:.4}°（Rust {:.4} / JS {:.4}）",
                c.name,
                got.angle,
                c.arrow.angle
            );
        }
        let cerr = (got.conf - c.arrow.conf).abs();
        max_conf_err = max_conf_err.max(cerr);
        assert!(
            cerr < 1e-3,
            "{}: conf 差 {cerr:.6}（Rust {:.6} / JS {:.6}）",
            c.name,
            got.conf,
            c.arrow.conf
        );

        // ---- 视锥通道
        let mut cws = ConeWorkspace::default();
        let cone = analyze_cone(&bytes, c.size, 1, &mut cws);
        assert_eq!(
            cone.valid, c.cone.valid,
            "{}: cone valid 不一致（Rust {} / JS {}）",
            c.name, cone.valid, c.cone.valid
        );
        if c.cone.valid {
            let err = ang_diff(cone.deg, c.cone.deg).abs();
            max_cone_err = max_cone_err.max(err);
            assert!(
                err < 0.05,
                "{}: cone 角度差 {err:.4}°（Rust {:.4} / JS {:.4}）",
                c.name,
                cone.deg,
                c.cone.deg
            );
        }
        assert!(
            (cone.conf - c.cone.conf).abs() < 1e-3,
            "{}: cone conf 差（Rust {:.6} / JS {:.6}）",
            c.name,
            cone.conf,
            c.cone.conf
        );

        // 余量字段也一起比：它们是 valid 的判据，差了说明中间步骤有分歧
        assert!(
            (got.tip_frac - c.arrow.tip_frac).abs() < 1e-3,
            "{}: tipFrac 不一致",
            c.name
        );
        assert!(
            (got.ang_margin - c.arrow.ang_margin).abs() < 1e-6,
            "{}: angMargin 不一致（Rust {:.3} / JS {:.3}）",
            c.name,
            got.ang_margin,
            c.arrow.ang_margin
        );
        // radMargin / angMargin 是**诊断字段**：它们由像素级的 max 半径算出，
        // f32 缓冲 + 边界像素归属差一个点，就会动 ~1e-3 量级。
        // 真正的判据（valid / angle / conf / cyans / tipFrac）都用严格容差在上面卡着。
        assert!(
            (got.rad_margin - c.arrow.rad_margin).abs() < 1e-6,
            "{}: radMargin 不一致（Rust {:.4} / JS {:.4}），corners={:?}",
            c.name,
            got.rad_margin,
            c.arrow.rad_margin,
            got.corners
        );
        assert!(
            (got.nose_flat - c.arrow.nose_flat).abs() < 1e-3,
            "{}: noseFlat 不一致",
            c.name
        );
        checked += 1;
    }

    println!(
        "对拍 {checked} 帧全部一致：箭头最大角度差 {max_angle_err:.4}°，视锥最大角度差 {max_cone_err:.4}°，最大 conf 差 {max_conf_err:.2e}"
    );
}
