//! 形状先验（同域模板）：运行时自举 + 遮挡鲁棒的匹配。
//!
//! ## 为什么需要
//!
//! 小地图上会画红白色的**导航路线条**，它可能从箭头中间穿过去、把尖端整个盖掉。
//! 剩下的后半截楔子本身是个"小三角"，PCA 主轴会偏 50~60°，方向错 120° 左右
//! （真机 m1 语料 57 帧里有 4 帧这样）。
//!
//! ## 为什么不能用"随包发的模板"
//!
//! 实测跨域完全不可用：真机模板去匹配视频是 13.7°、视频模板回匹配真机是 20.6°。
//! 模板学到的是**这一种渲染外形**（分辨率、缩放、抗锯齿、背景混色都算在内）。
//! 所以模板必须在**设备上自举** —— 用这台机器自己的帧现场叠一个。
//!
//! ## 自举用什么当真值
//!
//! v1（三点拟合）和 v3（主轴）是两个**结构完全不同**的估计器。
//! 它们在同一帧上给出相近的答案时，几乎不可能同时错到同一个地方
//! （实测：两者的大错帧集合互不重叠）。所以拿"v1 ∧ v3 一致 ≤15°"的帧当训练样本。
//!
//! ## 为什么用精确率而不是 NCC
//!
//! 遮挡只**减少**青色、不会凭空增加。所以
//!
//! ```text
//! precision = |T_rot ∩ C| / |C|   ← 候选的青色有几成被模板解释掉：遮挡不变
//! recall    = |T_rot ∩ C| / |T|   ← 模板有几成被覆盖：遮挡会降低，只当闸门
//! ```
//!
//! 对称的 NCC / F1 会把"被盖掉一块"当成不相似，正好丢掉这一点。

/// 裁切半径：中心 `2*HALF` 见方。真机箭头轴长 ~26~35px，64 见方足够。
pub const HALF: usize = 32;
const CROP: usize = 2 * HALF;

/// 至少积累这么多帧才认为先验可用。
/// 取 8：真机上 v1∧v3 一致的帧约占三成，8 帧大约十几秒就能攒够。
pub const MIN_FRAMES: u32 = 8;

/// 单帧青色面积（像素）的合理区间：太小说明没箭头，太大说明把别的东西并进来了。
const AREA_MIN: usize = 180;
const AREA_MAX: usize = 900;

/// 形状先验：一张"尖端朝上（罗盘 0°）"的平均模板。
#[derive(Debug, Clone)]
pub struct ShapePrior {
    /// 建这个先验时的采样窗边长；换了窗口就得重建
    side: usize,
    /// 累加的遮罩（0..1，除以 `n` 后得到"这一像素是青色的比例"）
    accum: Vec<f32>,
    n: u32,
}

impl Default for ShapePrior {
    fn default() -> Self {
        Self {
            side: 0,
            accum: vec![0.0; CROP * CROP],
            n: 0,
        }
    }
}

impl ShapePrior {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已积累帧数
    pub fn frames(&self) -> u32 {
        self.n
    }

    /// 先验是否可用
    pub fn ready(&self) -> bool {
        self.n >= MIN_FRAMES
    }

    /// 换窗口尺寸时重置（不同尺度的模板不能混用）
    fn ensure_side(&mut self, side: usize) {
        if self.side != side {
            self.side = side;
            self.accum.iter_mut().for_each(|v| *v = 0.0);
            self.n = 0;
        }
    }

    /// 喂一帧已知朝向的样本。`deg` 是罗盘角（0=北，顺时针）。
    ///
    /// 调用方负责只在**可信**的帧上调用（见模块注释：v1 ∧ v3 一致）。
    /// 返回是否真的吸收了。
    ///
    /// 中心化用的是**箭头连通块的质心**（4 邻域最大块），不是全窗口青色质心：
    /// 实机小地图上有路点图标等其它青色元素，混进质心会把裁切中心带偏
    /// （首份实机采集里 52% 的帧因此与 v3 分歧被丢弃）。
    pub fn observe(&mut self, rgba: &[u8], size: usize, deg: f64) -> bool {
        if size < 24 || rgba.len() < size * size * 4 {
            return false;
        }
        let Some((m, c, area)) = blob_of(rgba, size) else {
            return false;
        };
        if !area_ok(area, size) {
            return false;
        }
        self.ensure_side(size);
        let crop = crop_at(&m, size, c);
        // 转到"尖端朝上"的规范朝向
        let r = rotate(&crop, -deg);
        for i in 0..self.accum.len() {
            self.accum[i] += r[i];
        }
        self.n += 1;
        true
    }

    /// 当前先验的二值遮罩（比例 ≥0.5 的像素算内部）
    fn mask(&self) -> Vec<f32> {
        if self.n == 0 {
            return vec![0.0; CROP * CROP];
        }
        let inv = 1.0 / self.n as f32;
        self.accum
            .iter()
            .map(|v| if *v * inv >= 0.5 { 1.0 } else { 0.0 })
            .collect()
    }

    /// 诊断：把模板和当前帧的候选裁切渲染成 ASCII（模板 + 候选 + 两者交集）。
    ///
    /// 每行三栏并排，用于肉眼核对模板形状与候选形状到底差在哪。
    pub fn debug_render(&self, rgba: &[u8], size: usize) -> Option<String> {
        let (m, _c, area) = blob_of(rgba, size)?;
        if !area_ok(area, size) {
            return None;
        }
        let cand = crop_at(&m, size, _c);
        let tpl = self.mask();
        let mut s = String::new();
        for y in 0..CROP {
            let mut row_t = String::new();
            let mut row_c = String::new();
            let mut row_i = String::new();
            for x in 0..CROP {
                let i = y * CROP + x;
                let (t, c) = (tpl[i] >= 0.5, cand[i] >= 0.5);
                row_t.push(if t { '#' } else { '.' });
                row_c.push(if c { '#' } else { '.' });
                row_i.push(if t && c {
                    '=' // 两者都有
                } else if t {
                    'T' // 只有模板有
                } else if c {
                    'C' // 只有候选有
                } else {
                    '.'
                });
            }
            s.push_str(&format!("{row_t}  {row_c}  {row_i}\n"));
        }
        Some(s)
    }

    /// 诊断：返回在**指定角度**上的 (precision, recall)。
    ///
    /// 用于回答"模板在真值角度上得分多少"——如果真值角度的分数比匹配选出的
    /// 角度还低，说明模板本身被污染（自举吸收了坏帧），而不是搜索的问题。
    pub fn precision_at(&self, rgba: &[u8], size: usize, deg: f64) -> Option<(f64, f64)> {
        if !self.ready() || size < 24 || rgba.len() < size * size * 4 {
            return None;
        }
        let (m, c, area) = blob_of(rgba, size)?;
        if !area_ok(area, size) {
            return None;
        }
        let cand = crop_at(&m, size, c);
        let tpl = self.mask();
        if tpl.iter().all(|v| *v < 0.5) {
            return None;
        }
        let t = rotate(&tpl, deg);
        Some(score(&t, &cand))
    }

    /// 匹一帧，返回最可能的朝向。
    ///
    /// 先粗扫 `coarse_deg`，再在最优点附近细扫 `coarse_deg / 5`。
    pub fn match_frame(&self, rgba: &[u8], size: usize, coarse_deg: f64) -> Option<PriorMatch> {
        if !self.ready() || size < 24 || rgba.len() < size * size * 4 {
            return None;
        }
        let (m, c, area) = blob_of(rgba, size)?;
        if !area_ok(area, size) {
            return None;
        }
        let cand = crop_at(&m, size, c);
        let tpl = self.mask();
        if tpl.iter().all(|v| *v < 0.5) {
            return None;
        }

        let coarse = coarse_deg.clamp(2.0, 30.0);
        let mut best = (f64::NEG_INFINITY, 0.0f64, 0.0f64, 0.0f64);
        let mut a = 0.0;
        while a < 360.0 {
            let t = rotate(&tpl, a);
            let (p, r) = score(&t, &cand);
            let s = if r < 0.25 { -1.0 } else { p };
            if s > best.0 {
                best = (s, a, p, r);
            }
            a += coarse;
        }
        if best.1.is_nan() || best.0 < 0.0 {
            return None;
        }
        // 细扫
        let fine = coarse / 5.0;
        let center = best.1;
        let mut b = center - coarse;
        while b <= center + coarse {
            let t = rotate(&tpl, b);
            let (p, r) = score(&t, &cand);
            let s = if r < 0.25 { -1.0 } else { p };
            if s > best.0 {
                best = (s, b, p, r);
            }
            b += fine;
        }
        Some(PriorMatch {
            deg: best.1.rem_euclid(360.0),
            precision: best.2,
            recall: best.3,
        })
    }
}

/// 一次匹配的结果
#[derive(Debug, Clone, Copy)]
pub struct PriorMatch {
    /// 罗盘角（0=北，顺时针）
    pub deg: f64,
    /// 候选青色里有几成被模板解释掉（遮挡不变）
    pub precision: f64,
    /// 模板有几成被候选覆盖（遮挡会降低）
    pub recall: f64,
}

// ---------------------------------------------------------------- 基础操作

/// 青色遮罩（与 hexagon.rs 同一套阈值：p99×0.74、下限 112）
fn cyan_mask(rgba: &[u8], size: usize) -> Vec<f32> {
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
    for (s, cnt) in hist.iter().enumerate() {
        acc += cnt;
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

/// 提取**用于匹配的载荷**：裁剪用的遮罩、裁剪中心、箭头面积。
///
/// 中心用「中心圆内所有青色像素」的质心，**不是**最大连通块的质心：
/// 路线条会把箭头切成两半，最大块只是其中一片，用它当中心会让模板与候选错位
/// ——实测即使用人工真值建模板也修不好那 4 个遮挡帧（误差仍 125°）。
/// 用中心圆内的全部青色，两片残块一起参与，中心就落在箭头真正的中心上。
/// 圆半径限制是为了排除远处路点图标之类的青色。
///
/// 面积（用来判断"这是不是一个箭头"）仍按最大连通块算：
/// 混进外来青色时全窗口面积会虚高，最大块更接近箭头本身。
fn blob_of(rgba: &[u8], size: usize) -> Option<(Vec<f32>, (f64, f64), usize)> {
    let m = cyan_mask(rgba, size);
    let n = size * size;

    // 中心圆内的全青色质心
    let (wc, r2) = {
        let wc = (size as f64 - 1.0) / 2.0;
        let r = size as f64 * 0.30;
        (wc, r * r)
    };
    let (mut sx, mut sy, mut cnt) = (0.0, 0.0, 0.0);
    for (i, v) in m.iter().enumerate() {
        if *v <= 0.5 {
            continue;
        }
        let (x, y) = ((i % size) as f64, (i / size) as f64);
        let (dx, dy) = (x - wc, y - wc);
        if dx * dx + dy * dy > r2 {
            continue;
        }
        sx += x;
        sy += y;
        cnt += 1.0;
    }
    if cnt < 40.0 {
        return None;
    }
    let center = (sx / cnt, sy / cnt);

    // 最大连通块只用来定"箭头有多大"
    let mut seen = vec![false; n];
    let mut best = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    for seed in 0..n {
        if m[seed] <= 0.5 || seen[seed] {
            continue;
        }
        stack.clear();
        stack.push(seed);
        seen[seed] = true;
        let mut comp = 0usize;
        while let Some(cur) = stack.pop() {
            comp += 1;
            let y = cur / size;
            let x = cur % size;
            for (dy, dx) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let ny = y as i32 + dy;
                let nx = x as i32 + dx;
                if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                    continue;
                }
                let k = ny as usize * size + nx as usize;
                if m[k] > 0.5 && !seen[k] {
                    seen[k] = true;
                    stack.push(k);
                }
            }
        }
        best = best.max(comp);
    }
    if best < 40 {
        return None;
    }
    Some((m, center, best))
}

/// 块面积的合理区间（绝对值；本项目的采样窗都是 ~158px，箭头 ~400px）
fn area_ok(area: usize, _size: usize) -> bool {
    (AREA_MIN..=AREA_MAX).contains(&area)
}

/// 以 `center` 为中心裁 64×64。
///
/// **必须按质心裁**：箭头不在采样窗正中（真机实测偏 6.3px，而箭头半径才 ~14px）。
/// 按窗口中心裁会把模板糊成一团，匹配时任何角度都对不上（实测最高精确率仅 0.74）。
fn crop_at(m: &[f32], side: usize, center: (f64, f64)) -> Vec<f32> {
    let mut out = vec![0.0f32; CROP * CROP];
    let c0x = center.0.round() as i32;
    let c0y = center.1.round() as i32;
    for y in 0..CROP {
        for x in 0..CROP {
            let sy = c0y - HALF as i32 + y as i32;
            let sx = c0x - HALF as i32 + x as i32;
            if sy < 0 || sx < 0 || sy >= side as i32 || sx >= side as i32 {
                continue;
            }
            out[y * CROP + x] = m[sy as usize * side + sx as usize];
        }
    }
    out
}

/// 以裁切图中心为原点旋转（双线性反向映射）
fn rotate(src: &[f32], deg: f64) -> Vec<f32> {
    let c = (CROP as f64 - 1.0) / 2.0;
    let (s, co) = (deg.to_radians().sin(), deg.to_radians().cos());
    let mut out = vec![0.0f32; CROP * CROP];
    for y in 0..CROP {
        for x in 0..CROP {
            let dx = x as f64 - c;
            let dy = y as f64 - c;
            let sx = c + dx * co + dy * s;
            let sy = c - dx * s + dy * co;
            if sx < 0.0 || sy < 0.0 || sx >= CROP as f64 - 1.0 || sy >= CROP as f64 - 1.0 {
                continue;
            }
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
            let g = |xx: usize, yy: usize| src[yy * CROP + xx];
            out[y * CROP + x] = g(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + g(x0 + 1, y0) * fx * (1.0 - fy)
                + g(x0, y0 + 1) * (1.0 - fx) * fy
                + g(x0 + 1, y0 + 1) * fx * fy;
        }
    }
    out
}

/// 返回 (precision, recall)
fn score(t: &[f32], c: &[f32]) -> (f64, f64) {
    let (mut hit, mut tn, mut cn) = (0usize, 0usize, 0usize);
    for i in 0..CROP * CROP {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 画一个"尖端朝上"的楔形箭头（带燕尾缺口），放到窗口中心
    fn synth(side: usize, deg: f64, dx: f64, dy: f64) -> Vec<u8> {
        let mut v = vec![0u8; side * side * 4];
        let c = (
            (side as f64 - 1.0) / 2.0 + dx,
            (side as f64 - 1.0) / 2.0 + dy,
        );
        let rad = deg.to_radians();
        let (ux, uy) = (rad.sin(), -rad.cos()); // 罗盘 → 屏幕
        let (px, py) = (-uy, ux);
        for y in 0..side {
            for x in 0..side {
                let (fx, fy) = (x as f64 - c.0, y as f64 - c.1);
                let along = fx * ux + fy * uy;
                let across = fx * px + fy * py;
                // 楔形：沿轴 -6..+12，横向随 along 收窄；尾巴留一个缺口
                let w = if along < 0.0 {
                    (12.0 + along * 0.55).max(0.0)
                } else {
                    (12.0 - along * 0.95).max(0.0)
                };
                let in_tail_notch = along < -2.0 && across.abs() < 1.6;
                let inside = across.abs() <= w && (-7.0..=12.0).contains(&along);
                if inside && !in_tail_notch {
                    let i = (y * side + x) * 4;
                    // 青色：g、b 高，r 低
                    v[i] = 60;
                    v[i + 1] = 230;
                    v[i + 2] = 225;
                    v[i + 3] = 255;
                } else {
                    let i = (y * side + x) * 4;
                    v[i] = 40;
                    v[i + 1] = 93;
                    v[i + 2] = 103;
                    v[i + 3] = 255;
                }
            }
        }
        v
    }

    /// 先验能自举，并且能认出合成箭头的朝向
    #[test]
    fn prior_bootstraps_and_recovers_direction() {
        let side = 120usize;
        let mut p = ShapePrior::new();
        for i in 0..MIN_FRAMES {
            let deg = (i as f64 * 37.0) % 360.0;
            assert!(
                p.observe(&synth(side, deg, 0.0, 0.0), side, deg),
                "第 {i} 帧应当被吸收"
            );
        }
        assert!(p.ready(), "攒够 {} 帧后应当可用", MIN_FRAMES);
        for deg in [0.0, 90.0, 180.0, 270.0, 45.0, 200.0] {
            let m = p
                .match_frame(&synth(side, deg, 0.0, 0.0), side, 10.0)
                .unwrap();
            let err = ((m.deg - deg + 540.0) % 360.0 - 180.0).abs();
            assert!(
                err < 20.0,
                "朝向 {deg}° 识别成 {:.1}°（误差 {err:.1}°）",
                m.deg
            );
        }
    }

    /// 先验在"箭头偏离窗心"时仍然有效（质心对齐的作用）
    #[test]
    fn prior_tolerates_off_center_arrow() {
        let side = 120usize;
        let mut p = ShapePrior::new();
        for i in 0..MIN_FRAMES {
            let deg = (i as f64 * 41.0) % 360.0;
            let (dx, dy) = ((i as f64 % 5.0) - 2.0, ((i as f64 * 3.0) % 5.0) - 2.0);
            p.observe(&synth(side, deg, dx, dy), side, deg);
        }
        assert!(p.ready());
        let m = p
            .match_frame(&synth(side, 137.0, 4.0, -3.0), side, 10.0)
            .unwrap();
        let err = ((m.deg - 137.0 + 540.0) % 360.0 - 180.0).abs();
        assert!(err < 25.0, "偏 5px 后仍应认出 137°，实际 {:.1}°", m.deg);
    }

    /// 窗口尺寸变了要先验失效（不同尺度不能混用）
    #[test]
    fn prior_resets_when_window_size_changes() {
        let mut p = ShapePrior::new();
        for i in 0..MIN_FRAMES {
            let deg = (i as f64 * 30.0) % 360.0;
            p.observe(&synth(120, deg, 0.0, 0.0), 120, deg);
        }
        assert!(p.ready());
        // 换窗口 → 观察被重置
        p.observe(&synth(160, 30.0, 0.0, 0.0), 160, 30.0);
        assert_eq!(p.frames(), 1, "换窗口后应当从头攒");
        assert!(!p.ready());
    }
}
