//! 自动定心：在一块**大范围**截屏里找玩家箭头，反推小地图圆心与半径。
//!
//! 为什么要它：ROI（小地图在哪、多大）一直靠预设或手动标定，换个作者、换个分辨率、
//! 换个 HUD 缩放就对不上 —— 这一路踩的坑（冰原素材 0 帧、真机 0 帧）几乎都出在这。
//! 而箭头本身是**全屏最亮的青色**，找它比"猜小地图位置"靠得住得多。
//!
//! 做法：
//! 1. 以当前 ROI 为中心截一块 6 倍大的方图（够容纳整个小地图 + 偏移）；
//! 2. 算青色分数 `min(g,b) − r`，取分数最高的一小撮像素（前 0.15%）求质心 ——
//!    那就是箭头，和 `analyze` 用的是同一套判据，不引入新的颜色假设；
//! 3. 由箭头的外接尺寸反推小地图半径：实测箭头宽度 ≈ 小地图直径的 0.30~0.35，
//!    取 0.32 反推，再换算成 `r`（相对基准宽的半径比例）。
//!
//! 只在需要时跑一次（换视频/换分辨率/手动触发），不进每帧链路。


/// 自动定心的结果（全部是相对基准矩形的比例）
#[derive(Debug, Clone, Copy)]
pub struct Centered {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
    /// 找到箭头的青色分数峰值（越大越可信）
    pub peak: i32,
    /// 箭头外接尺寸（像素），调试用
    pub arrow_px: i32,
}

/// 在 `base`（屏幕坐标的基准矩形，比如游戏客户区或视频内容区）上找小地图。
///
/// `probe` 是当前的猜测位置：先在小范围里找，找不到再看整块区域。
///
/// **关键**：不"取最亮的青色求质心"就完事 —— 视频里的蓝色 UI、水面、字幕都会骗人
/// （实测把 cx 定位到 0.83）。这里把候选青色块逐个交给**真正的箭头检测器**验收，
/// 通过了才认。
pub fn autocenter(
    base_x: i32,
    base_y: i32,
    base_w: i32,
    base_h: i32,
    probe: (f64, f64, f64),
) -> Option<Centered> {
    if base_w < 200 || base_h < 200 {
        return None;
    }
    let cx = base_x as f64 + probe.0 * base_w as f64;
    let cy = base_y as f64 + probe.1 * base_h as f64;
    let side = (probe.2 * base_w as f64 * 6.0).clamp(240.0, 1000.0) as i32;
    let x = (cx - side as f64 / 2.0) as i32;
    let y = (cy - side as f64 / 2.0) as i32;

    let area = Rect {
        x,
        y,
        w: side,
        h: side,
    };
    let base = Rect {
        x: base_x,
        y: base_y,
        w: base_w,
        h: base_h,
    };
    if let Some(c) = scan(area, base) {
        return Some(c);
    }
    // 猜测点周围没有：扫整块基准区域
    scan(base, base)
}

/// 矩形：截屏区域与"基准区域"都用它
#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// 在一张截屏里找箭头并换算成比例
fn scan(area: Rect, base: Rect) -> Option<Centered> {
    let Rect { x, y, w, h } = area;
    let frame = crate::screen::capture_screen(x, y, w, h).ok()?;
    let fw = frame.width.max(1) as usize;
    let fh = frame.height.max(1) as usize;

    // 1) 青色掩膜（阈值取 p99.5 与地板 120 的较大者）
    let mut scores = Vec::with_capacity(fw * fh);
    for i in 0..(fw * fh) {
        let r = frame.data[i * 4] as i32;
        let g = frame.data[i * 4 + 1] as i32;
        let b = frame.data[i * 4 + 2] as i32;
        scores.push((g.min(b) - r).clamp(-255, 255));
    }
    let mut hist = [0u32; 256];
    for &s in &scores {
        if s >= 0 {
            hist[s as usize] += 1;
        }
    }
    let total: u32 = hist.iter().sum();
    let mut acc = 0u32;
    let mut p995 = 0i32;
    for (s, n) in hist.iter().enumerate() {
        acc += n;
        if acc as f64 >= total as f64 * 0.995 {
            p995 = s as i32;
            break;
        }
    }
    let thr = ((p995 as f64) * 0.8).max(120.0) as i32;
    let mut mask = vec![0u8; fw * fh];
    for i in 0..(fw * fh) {
        if scores[i] >= thr {
            mask[i] = 1;
        }
    }

    // 2) 连通块 → 候选（按"像箭头"排序：面积适中、外接框接近方形）
    let mut seen = vec![false; fw * fh];
    let mut cands: Vec<(f64, i32, i32, i32)> = Vec::new(); // 分数, cx, cy, 尺寸
    let mut stack: Vec<usize> = Vec::new();
    for seed in 0..(fw * fh) {
        if mask[seed] == 0 || seen[seed] {
            continue;
        }
        stack.clear();
        stack.push(seed);
        seen[seed] = true;
        let (mut sx, mut sy, mut n) = (0f64, 0f64, 0usize);
        let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0usize, usize::MAX, 0usize);
        while let Some(cur) = stack.pop() {
            n += 1;
            let px = cur % fw;
            let py = cur / fw;
            sx += px as f64;
            sy += py as f64;
            x0 = x0.min(px);
            x1 = x1.max(px);
            y0 = y0.min(py);
            y1 = y1.max(py);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = px as i32 + dx;
                    let ny = py as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= fw as i32 || ny >= fh as i32 {
                        continue;
                    }
                    let k = ny as usize * fw + nx as usize;
                    if mask[k] != 0 && !seen[k] {
                        seen[k] = true;
                        stack.push(k);
                    }
                }
            }
        }
        if !(80..=20000).contains(&n) {
            continue;
        }
        let bw = (x1 - x0 + 1) as f64;
        let bh = (y1 - y0 + 1) as f64;
        let aspect = bw.min(bh) / bw.max(bh);
        if aspect < 0.45 {
            continue;
        }
        let px = sx / n as f64;
        let py = sy / n as f64;
        // 分母小一点让"方形且不太大"的块排前面；真正的验收在下面
        let score = aspect / (n as f64).sqrt();
        cands.push((score, px as i32, py as i32, bw.max(bh) as i32));
    }
    cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // 3) 逐个交给真正的箭头检测器验收
    let mut ws = crate::arrow::ArrowWorkspace::default();
    for (_, px, py, size) in cands.into_iter().take(8) {
        let win = ((size as f64) * 3.4).clamp(64.0, 320.0) as i32;
        let wx = (x + px - win / 2).max(base.x);
        let wy = (y + py - win / 2).max(base.y);
        let Some(f) = crate::screen::capture_screen(wx, wy, win, win).ok() else {
            continue;
        };
        let side = f.width.min(f.height).max(8) as usize;
        let r = crate::arrow::analyze(&f.data, side, 1, &mut ws);
        if !r.valid {
            continue;
        }
        let cx_frac = (wx as f64 + win as f64 / 2.0 - base.x as f64) / base.w as f64;
        let cy_frac = (wy as f64 + win as f64 / 2.0 - base.y as f64) / base.h as f64;
        if !(0.0..=1.0).contains(&cx_frac) || !(0.0..=1.0).contains(&cy_frac) {
            continue;
        }
        // 箭头宽度 ≈ 小地图直径的 0.32 → 半径 ≈ size/0.64
        let r_frac = ((size as f64 / 0.64) / base.w as f64).clamp(0.02, 0.35);
        return Some(Centered {
            cx: cx_frac,
            cy: cy_frac,
            r: r_frac,
            peak: thr,
            arrow_px: size,
        });
    }
    None
}
