//! 把某帧的青色 mask 打成 ASCII，用来核对几何形状。
//!   cargo run -p gifa-vision --example dump-mask -- <file.bin> [每行字符数]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("用法: dump-mask <file.bin> [宽]");
    let bytes = std::fs::read(path).expect("读文件");
    let side = (bytes.len() / 4).isqrt();
    let step: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let score = |i: usize| -> i32 {
        let b = bytes[i * 4] as i32;
        let g = bytes[i * 4 + 1] as i32;
        let _r = bytes[i * 4 + 2] as i32;
        g.min(255) - b
    };
    // 用与 hexagon 相同的阈值思路：p99 * 0.74
    let mut vals: Vec<i32> = (0..side * side).map(score).collect();
    vals.sort_unstable();
    let p99 = vals[(vals.len() * 99 / 100).min(vals.len() - 1)];
    let thr = 112.max((p99 as f64 * 0.74) as i32);
    println!("# {path} side={side} thr={thr}");
    for y in (0..side).step_by(step) {
        let mut line = String::new();
        for x in (0..side).step_by(step) {
            line.push(if score(y * side + x) >= thr { '#' } else { '.' });
        }
        println!("{line}");
    }
}
