//! 真实像素回归：用两枚来自真机截图的箭头，钉住移植后的行为。
//!
//! 素材（`tools/vision-ref/`，由 2560×1440 实机截图裁出）：
//!
//! * `real-arrow-clean-96.bin` —— 玩家箭头本体贴在平底色上，96×96。
//!   这是"算法该认得的样子"：4 个角、angMargin 23.4、输出 90.2°。
//!   真值用镜像对称轴法算是 89.0°，所以误差 1.2°。
//! * `real-minimap-112.bin` —— 同一枚箭头连着小地图背景，112×112。
//!   背景里另有一块青色（约 140~150°、半径 26）混进同一个连通块，
//!   角数涨到 5 个、输出飘到 101.8°（偏 11°），但 angMargin 仍有 13.5 ——
//!   闸门没拦住。这正是"真机上会偏十几度"的典型来源。
//!
//! 期望值是在同一批像素上用原版 JS（`harness.js analyze`）跑出来的，
//! 这里只要求 Rust 与它一致；**不**要求它等于真值。

use std::path::PathBuf;

use gifa_vision::arrow::{analyze, ArrowWorkspace};

fn load(name: &str) -> (Vec<u8>, usize) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("读 {}: {e}", path.display()));
    let side = (bytes.len() / 4).isqrt();
    assert_eq!(side * side * 4, bytes.len(), "{name} 不是正方形 RGBA");
    (bytes, side)
}

#[test]
fn clean_real_arrow_matches_reference() {
    let (data, side) = load("real-arrow-clean-96.bin");
    let mut ws = ArrowWorkspace::default();
    let r = analyze(&data, side, 1, &mut ws);
    assert!(r.valid, "干净箭头应判有效（angMargin={}）", r.ang_margin);
    assert_eq!(r.cyans, 331, "青色像素数应与原版一致");
    assert_eq!(r.corners.len(), 4, "干净箭头是 4 个角");
    let err = ((r.angle - 90.2 + 540.0) % 360.0 - 180.0).abs();
    assert!(err < 0.05, "角度应与原版一致：Rust {:.4} / JS 90.2", r.angle);
    assert!(
        (r.ang_margin - 23.4).abs() < 0.05,
        "angMargin 应与原版一致：Rust {} / JS 23.4",
        r.ang_margin
    );
}

#[test]
fn minimap_background_pulls_the_angle_but_still_passes_gates() {
    let (data, side) = load("real-minimap-112.bin");
    let mut ws = ArrowWorkspace::default();
    let r = analyze(&data, side, 1, &mut ws);
    // 这一帧原版判有效、但角度偏了 11°（背景青色混进连通块）——
    // 移植必须复现同样的"错"，因为它是算法本身的行为
    assert_eq!(r.corners.len(), 5, "带背景时是 5 个角");
    let err = ((r.angle - 101.8 + 540.0) % 360.0 - 180.0).abs();
    assert!(err < 0.05, "角度应与原版一致：Rust {:.4} / JS 101.8", r.angle);
    assert!(
        r.valid,
        "原版在这一帧仍判有效（angMargin 13.5 > 4），移植不应自行加严"
    );
}
