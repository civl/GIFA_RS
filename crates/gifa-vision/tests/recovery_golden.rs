//! 重试策略的效果验证：加不加 recovery，在两份人工真值上的覆盖率。

use std::path::PathBuf;

use gifa_vision::arrow::{analyze_with, ArrowParams, ArrowWorkspace};
use gifa_vision::recovery::analyze_arrow_with_recovery;

fn params() -> ArrowParams {
    ArrowParams {
        bis_err_max: 25.0,
        ang_margin_min: 18.0,
        ..Default::default()
    }
}

fn load_dir(dir: PathBuf, prefix_skip: bool) -> Vec<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for e in std::fs::read_dir(&dir).expect("dir") {
        let path = e.expect("entry").path();
        if path.extension().and_then(|x| x.to_str()) != Some("bin") {
            continue;
        }
        if prefix_skip
            && path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sample-")
        {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read");
        let side = (bytes.len() / 4).isqrt();
        out.push((bytes, side));
    }
    out.sort_by_key(|(b, _)| b.len());
    out
}

#[test]
fn recovery_does_not_lose_frames() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/realgame");
    let frames = load_dir(dir, true);
    if frames.is_empty() {
        eprintln!("跳过：没有真机素材");
        return;
    }
    let mut ws = ArrowWorkspace::default();
    let (mut base_ok, mut rec_ok) = (0, 0);
    for (bytes, side) in &frames {
        let b = analyze_with(bytes, *side, 1, &mut ws, &params());
        let r = analyze_arrow_with_recovery(bytes, *side, 1, &mut ws, &params());
        if b.valid {
            base_ok += 1;
        }
        if r.valid {
            rec_ok += 1;
        }
        // 主判据已经认出来的帧，重试策略不应该把它改判成无效
        if b.valid && !r.valid {
            panic!("重试策略弄丢了本来有效的帧（{}px）", side);
        }
    }
    println!(
        "真机素材 {} 帧：主判据 {base_ok} 有效 → 加重试 {rec_ok} 有效",
        frames.len()
    );
    assert!(rec_ok >= base_ok);
}
