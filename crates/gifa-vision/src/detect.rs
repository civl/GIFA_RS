//! 采样窗 + 朴素检测（**占位实现**）。
//!
//! ⚠️ 这里的 `detect_simple` 不是原版算法，只是把链路打通用的占位：
//! 原版是"自适应阈值 → 最大连通块 → 72 格径向剖面 → 角候选 → 三点枚举评分 → 鼻尖"，
//! 外加极坐标格 + 最小二乘的白膜边检测，以及 α-β 滤波。
//! 二期移植必须逐函数对照 `webview-preload.js:365-2050`，不能拿这一版当基线。
//!
//! 占位版做两件简单的事，够验证"截帧→裁剪→出角度"是通的：
//! * 青色像素（`cyan_score`）的质心方向 → 箭头角
//! * 近白像素（雪地/白膜）的质心方向 → 视角角

use crate::color;
use crate::frame::Frame;
use crate::params;

/// 采样窗（方形），以 ROI 中心为中心
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleBox {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl SampleBox {
    /// 由归一化 ROI + 视频像素尺寸算出采样窗。
    /// 原版 `grabCenter(roi, scale=0.62, cap=320)`：窗口边长 = `r × 2 × 0.62`，上限 320。
    ///
    /// 这个系数很关键：窗口开大了，箭头在采样点里的占比掉到 1% 以下，
    /// `analyze` 的 `p99 < 100` 就会判"这一帧没有小地图" —— 实测就是这样把真箭头漏掉的。
    pub fn from_roi(roi: crate::frame::Roi, video_w: i32, video_h: i32) -> Option<Self> {
        if !roi.is_valid() || video_w <= 0 || video_h <= 0 {
            return None;
        }
        let cx = (roi.cx * video_w as f64).round() as i32;
        let cy = (roi.cy * video_h as f64).round() as i32;
        let r = (roi.r * video_w as f64).round() as i32;
        let side = ((r.max(8) as f64) * 2.0 * 0.62).round().clamp(32.0, 320.0) as i32;
        Some(Self {
            x: cx - side / 2,
            y: cy - side / 2,
            w: side,
            h: side,
        })
    }
}

/// 检测结果
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Detect {
    /// 行进方向（罗盘角：0=北，顺时针），`None` = 这一帧没有
    pub arrow: Option<f64>,
    /// 镜头视角（罗盘角）
    pub view: Option<f64>,
    pub arrow_conf: f32,
    pub view_conf: f32,
    /// 青色像素占比（小地图存在性门控用）
    pub cyan_frac: f32,
    /// 采样点数
    pub samples: u32,
    /// 这一帧有没有小地图（青色占比门控）
    pub map_present: bool,
    /// 箭头因"鼻子不在轴上"被挡掉（背景青色污染，原版会给出 10° 以上的错误角度）
    pub rejected_axis: bool,
    /// 第二意见（凸包+凹口模型）的角度与两模型差角，调试用
    pub cross_check: Option<(f64, f64)>,
    /// view 是否来自真视锥（而不是占位）
    pub view_is_cone: bool,
    /// 视锥的"白膜几何校验"是否通过（扇形内部整体比对面亮）
    pub membrane_ok: bool,
    /// 0=没被挡 1=鼻子不在轴上
    pub reject_reason: u8,
    /// 箭头来源：0=v1 三点拟合，2=六边形兜底
    pub arrow_source: u8,
    /// |鼻子角 − 输出轴角|（标定时用来看闸门余量）
    pub nose_gap: Option<f32>,
    /// 原版的三点评分余量（越大越可信）
    pub ang_margin: Option<f32>,
}

/// 屏幕/浮窗坐标 → 帧内坐标的偏移
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Origin {
    pub x: i32,
    pub y: i32,
}

impl Detect {
    fn empty() -> Self {
        Self::default()
    }
}

/// 真·箭头检测：采样窗必须是正方形（管线就是这么截的）。
///
/// 行为分支和原版一致：`analyze` 说无效就**不报箭头**（不拿视角顶替），
/// 视角暂时仍用质心占位（扇形通道还没移植）。
/// 箭头拟合参数（真机实测标定）。
///
/// 原版：bis_err ≤15°、ang_margin ≥4。
/// 真机 2560×1440 绿地 39 帧人工真值实测：
///
/// | 配置 | 覆盖率 | 误差中位 | 坏帧（>30°） |
/// |---|---|---|---|
/// | 原版（15/4） | 0/39 | — | — |
/// | bisErr 25 / margin 4 | 35/39 | 9.7° | 7 帧（偏 118°）|
/// | bisErr 25 / **margin 18** | **28/39** | **9.7°** | **0** |
///
/// 视频素材（138 帧标注）上这套参数**完全等价于原版**：有效帧 138/138、
/// 误差中位 0.03°、最大 2.35°（原版 angMargin 最小 20.3，18 这条线碰不到它）。
fn arrow_params() -> crate::arrow::ArrowParams {
    crate::arrow::ArrowParams {
        bis_err_max: 25.0,
        ang_margin_min: 18.0,
    }
}

pub fn detect_window(
    rgba: &[u8],
    size: usize,
    aws: &mut crate::arrow::ArrowWorkspace,
    cws: &mut crate::cone::ConeWorkspace,
) -> Detect {
    let mut out = Detect::default();
    /* 箭头的采样窗要比视锥小一圈。
     *
     * `analyze` 里有一道 `p99 < 100 ⇒ 判"这一帧没有小地图"` 的闸（原版用来跳过
     * 转场/菜单）。它要求箭头在窗口里占 1% 以上。真机 2560×1440 实测：小地图直径
     * ~260px、箭头只有 ~40px，用 253px 的窗口时 p99 = 73 → 直接被判"没有小地图"，
     * 而同一帧的视锥通道 100% 有效。
     *
     * 所以两个通道用两个窗口：视锥用整窗（它要看到扇形边缘），箭头用中心 55% 的
     * 裁切（箭头占 8% 左右，p99 稳定过线）。不额外截屏，只是同一块像素里换窗口。 */
    let crop_frac: f64 = std::env::var("GIFA_ARROW_CROP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.55);
    let arrow_side = ((size as f64 * crop_frac) as usize).clamp(48, size) & !1;
    let crop_off = (size - arrow_side) / 2;
    let arrow = if arrow_side + 8 < size {
        let mut crop = vec![0u8; arrow_side * arrow_side * 4];
        for y in 0..arrow_side {
            let src = ((y + crop_off) * size + crop_off) * 4;
            let dst = y * arrow_side * 4;
            crop[dst..dst + arrow_side * 4].copy_from_slice(&rgba[src..src + arrow_side * 4]);
        }
        crate::recovery::analyze_arrow_with_recovery(&crop, arrow_side, 1, aws, &arrow_params())
    } else {
        crate::recovery::analyze_arrow_with_recovery(rgba, size, 1, aws, &arrow_params())
    };
    let nose_gap = arrow
        .nose_deg
        .map(|n| ((n - arrow.angle + 540.0) % 360.0 - 180.0).abs() as f32);
    let ang_margin = Some(arrow.ang_margin as f32);
    let cone =
        crate::recovery::analyze_cone_with_recovery(rgba, size, 1, cws, params::MAP_CYAN_FRAC_MIN);

    out.samples = arrow.cyans;
    out.nose_gap = nose_gap;
    out.ang_margin = ang_margin;
    out.cyan_frac = if size > 0 {
        arrow.cyans as f32 / (size * size) as f32
    } else {
        0.0
    };

    // 小地图存在性门控：青色占比不够就什么都不报（转场/特写/看视频）
    // 原版用 cone 里算的 cyanFrac（固定阈值 80），比 arrow 的自适应阈值更可靠
    if cone.cyan_frac < params::MAP_CYAN_FRAC_MIN {
        return out;
    }
    out.map_present = true;

    if cone.valid {
        out.view = Some(crate::cone::to_compass(cone.deg));
        out.view_conf = cone.conf as f32;
        out.view_is_cone = true;
        out.membrane_ok = cone.membrane_ok;
    }
    /* v1 不出（或评分余量不够）时兜底。
     *
     * 用 v3（主轴 + 两端宽度）而不是 v1 六边形（凹口中点→最远点）：
     * 真机语料上 v1 六边形 2/39 帧偏 >45°、视频语料 59/138 帧偏 >45°，
     * v3 两份都是 0 帧（中位 2.4°/4.0°）。见 hexagon.rs 的实测表。 */
    let v1_strong = arrow.valid && arrow.ang_margin >= 18.0;
    if !v1_strong {
        let c = (size as f64 / 2.0, size as f64 / 2.0);
        if let Some(h) = crate::hexagon::direction_v3(rgba, size, c, 0.0) {
            out.arrow = Some(h.deg);
            out.arrow_conf = 0.5;
            out.arrow_source = 2; // 几何兜底
            return out;
        }
    }

    if arrow.valid {
        // 原版给的是 atan2 角，叠加层要罗盘角。
        // 额外过一道"鼻子在轴上"的检查：小地图背景的青色混进连通块时，
        // 原版仍会判有效、但角度能偏 10° 以上（实测见 NOSE_AXIS_MAX_DEG 文档）。
        /* 撒手不管的两个旧闸：
         * · `axis_consistent`（|鼻子−轴| ≤8°）—— 真机实测**反了**：
         *   好帧 24.0~24.5°，坏帧 18~22.6°。它只会把对的挡掉。
         * · v2（凸包+凹口）—— 真机序列上 74% 的帧与 v1 分歧，不可靠。
         * 现在靠 `ang_margin` 区分好坏（见 arrow_params 的注释）。 */

        /* 第二意见：凸包 + 凹口定位（结构完全不同的模型）。
         * 两个模型在干净帧上差 ~10°，在"背景青色混进连通块"的帧上差 35° ——
         * 差得太多就说明这一帧的形状被污染了，宁可不上报。 */
        let second = crate::template::fit_arrow(rgba, size);
        let agree = match second {
            Some(f) => {
                let d = ((f.angle - arrow.angle + 540.0) % 360.0 - 180.0).abs();
                out.cross_check = Some((f.angle, d));
                d <= crate::template::MAX_DISAGREE_DEG
            }
            // 拿不到第二意见（没有明显凹口）就不拦：以 v1 + 鼻子闸为准
            None => true,
        };
        /* 视锥互证**不成立**，已撤：原神的移动是"镜头相对"的，按 A/D 横移时
         * 箭头（行进）与视锥（镜头）本来就**垂直**；按 S 倒走时还差 180°。
         * 实测在蒙德素材上它会误挡 16/290 帧。留在文档里当结论。 */

        /* v2 只记录、不拦截：真实视频上它和 v1 有 74% 的帧分歧 >16°，
         * 说明 v2 还不够稳（在 2 张真机静态图上表现好，不代表序列上可靠）。
         * 等它准了再把 agree 加回判据。 */
        let _ = agree;
        if true {
            out.arrow = Some(crate::arrow::to_compass(arrow.angle));
            out.arrow_conf = arrow.conf as f32;
            out.arrow_source = 1;
        } else {
            out.rejected_axis = true;
            out.reject_reason = 1;
        }
    }
    out
}

/// 从一帧里截取采样窗（越界部分裁掉，返回实际矩形）
fn crop(frame: &Frame, b: SampleBox, origin: Origin) -> Option<(SampleBox, &[u8], usize)> {
    let x = b.x - origin.x;
    let y = b.y - origin.y;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + b.w).min(frame.width);
    let y1 = (y + b.h).min(frame.height);
    if x1 - x0 < 8 || y1 - y0 < 8 {
        return None;
    }
    let w = x1 - x0;
    let h = y1 - y0;
    let stride = frame.width as usize * 4;
    let start = (y0 as usize * stride) + (x0 as usize * 4);
    let len = ((h as usize - 1) * stride) + (w as usize * 4);
    let slice = frame.data.get(start..start + len)?;
    Some((
        SampleBox {
            x: x0 + origin.x,
            y: y0 + origin.y,
            w,
            h,
        },
        slice,
        stride,
    ))
}

/// 罗盘角：0=北、顺时针。屏幕坐标 y 向下，所以 dy 取反。
fn compass(dx: f64, dy: f64) -> f64 {
    let deg = dx.atan2(-dy).to_degrees();
    (deg + 360.0) % 360.0
}

/// 占位检测：青色质心 + 近白质心
pub fn detect_simple(frame: &Frame, window: SampleBox, origin: Origin) -> Detect {
    let Some((box_, data, stride)) = crop(frame, window, origin) else {
        return Detect::empty();
    };
    let cx = box_.w as f64 / 2.0;
    let cy = box_.h as f64 / 2.0;

    // 只采中心圆盘（原版 `RADIUS_MIN..RADIUS_MAX` 的那条带）
    let r_out = (box_.w.min(box_.h) as f64 / 2.0) * params::RADIUS_MAX;
    let r_in = (box_.w.min(box_.h) as f64 / 2.0) * params::RADIUS_MIN;

    let (mut cyans, mut whites) = (0u32, 0u32);
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    let (mut wx, mut wy) = (0.0f64, 0.0f64);
    let mut samples = 0u32;

    for y in 0..box_.h {
        let row = (y as usize) * stride;
        for x in 0..box_.w {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d < r_in || d > r_out {
                continue;
            }
            samples += 1;
            let o = row + (x as usize) * 4;
            let (r, g, b) = (data[o] as i32, data[o + 1] as i32, data[o + 2] as i32);
            let score = color::cyan_score(r as u8, g as u8, b as u8) as f64;
            if score >= params::THRESHOLD_FLOOR {
                cyans += 1;
                sx += dx;
                sy += dy;
            }
            // 近白：亮度高、饱和低（雪地、白膜边）
            let mx = r.max(g).max(b);
            let mn = r.min(g).min(b);
            if mn > 200 && mx - mn < 28 {
                whites += 1;
                wx += dx;
                wy += dy;
            }
        }
    }

    let cyan_frac = if samples > 0 {
        cyans as f32 / samples as f32
    } else {
        0.0
    };

    // 小地图存在性门控：整块画面几乎没有青色 → 这里根本没有小地图
    // （转场/特写/对话/看视频），那就**什么都不报**，不要拿白像素硬凑视角。
    if !color::map_present(cyan_frac as f64) {
        return Detect {
            samples,
            cyan_frac,
            ..Detect::empty()
        };
    }

    let mut out = Detect {
        samples,
        cyan_frac,
        ..Detect::empty()
    };

    // 箭头：青色质心；像素太少就不猜
    if cyans >= 12 {
        out.arrow = Some(compass(sx, sy));
        out.arrow_conf = (cyans as f32 / 240.0).min(1.0);
    }
    // 视角：白膜质心方向（扇形对称时质心方向≈朝向）
    if whites >= 40 {
        out.view = Some(compass(wx, wy));
        out.view_conf = (whites as f32 / 600.0).min(1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: i32, h: i32) -> Frame {
        Frame::new(w, h, [40u8, 60, 45, 255].repeat((w * h) as usize))
    }

    fn put(f: &mut Frame, x: i32, y: i32, rgb: (u8, u8, u8)) {
        if x < 0 || y < 0 || x >= f.width || y >= f.height {
            return;
        }
        let i = ((y * f.width + x) * 4) as usize;
        f.data[i] = rgb.0;
        f.data[i + 1] = rgb.1;
        f.data[i + 2] = rgb.2;
    }

    fn blob(f: &mut Frame, cx: i32, cy: i32, rad: i32, rgb: (u8, u8, u8)) {
        for dy in -rad..=rad {
            for dx in -rad..=rad {
                if dx * dx + dy * dy <= rad * rad {
                    put(f, cx + dx, cy + dy, rgb);
                }
            }
        }
    }

    const CYAN: (u8, u8, u8) = (34, 200, 232);
    const WHITE: (u8, u8, u8) = (245, 245, 242);

    #[test]
    fn arrow_east_of_center_reads_90() {
        let mut f = blank(200, 200);
        blob(&mut f, 165, 150, 5, CYAN); // 采样窗中心 (150,150) 正东 15px
        let win = SampleBox {
            x: 100,
            y: 100,
            w: 100,
            h: 100,
        };
        let d = detect_simple(&f, win, Origin { x: 0, y: 0 });
        let a = d.arrow.expect("应检测到箭头");
        assert!((a - 90.0).abs() < 12.0, "东侧应约 90°，实际 {a}");
        assert!(d.cyan_frac > 0.001);
    }

    #[test]
    fn arrow_north_reads_zero() {
        let mut f = blank(200, 200);
        blob(&mut f, 150, 135, 5, CYAN); // 中心正北 15px
        let win = SampleBox {
            x: 100,
            y: 100,
            w: 100,
            h: 100,
        };
        let d = detect_simple(&f, win, Origin { x: 0, y: 0 });
        let a = d.arrow.expect("应检测到箭头");
        assert!(a > 348.0 || a < 12.0, "北侧应约 0°/360°，实际 {a}");
    }

    #[test]
    fn no_cyan_means_no_map_and_no_output() {
        // 一块纯白画面（视频、UI）：没有青色 → 判"没有小地图"，箭头和视角都不报
        let mut f = blank(200, 200);
        blob(&mut f, 150, 160, 12, WHITE);
        let d = detect_simple(
            &f,
            SampleBox {
                x: 100,
                y: 100,
                w: 100,
                h: 100,
            },
            Origin { x: 0, y: 0 },
        );
        assert_eq!(d.view, None, "没有青色就是没有小地图，不能靠白像素报视角");
        assert_eq!(d.arrow, None);
    }

    #[test]
    fn no_cyan_means_no_arrow() {
        let f = blank(120, 120);
        let d = detect_simple(
            &f,
            SampleBox {
                x: 60,
                y: 60,
                w: 80,
                h: 80,
            },
            Origin { x: 0, y: 0 },
        );
        assert_eq!(d.arrow, None, "没有青色就不猜");
        assert_eq!(d.cyan_frac, 0.0);
    }

    #[test]
    fn white_fan_gives_view_angle() {
        let mut f = blank(200, 200);
        blob(&mut f, 150, 175, 9, WHITE); // 中心正南 25px
                                          // 门控要求画面里存在青色（真实场景里箭头通常也在）
        blob(&mut f, 150, 150, 4, CYAN);
        let d = detect_simple(
            &f,
            SampleBox {
                x: 100,
                y: 100,
                w: 100,
                h: 100,
            },
            Origin { x: 0, y: 0 },
        );
        let v = d.view.expect("应检测到视角");
        assert!((v - 180.0).abs() < 20.0, "南侧应约 180°，实际 {v}");
    }

    #[test]
    fn origin_offset_is_applied() {
        // 帧是屏幕坐标 (1000, 800) 起，采样窗给了屏幕坐标
        let mut f = blank(120, 120);
        blob(&mut f, 50, 60, 6, CYAN); // 帧内西侧
        let win = SampleBox {
            x: 1000,
            y: 800,
            w: 120,
            h: 120,
        };
        let d = detect_simple(&f, win, Origin { x: 1000, y: 800 });
        let a = d.arrow.expect("应检测到箭头");
        assert!((a - 270.0).abs() < 20.0, "西侧应约 270°，实际 {a}");
    }

    #[test]
    fn sample_box_from_roi_is_larger_than_roi() {
        let roi = crate::frame::Roi {
            cx: 0.5,
            cy: 0.5,
            r: 0.06,
        };
        let b = SampleBox::from_roi(roi, 1920, 1080).unwrap();
        // r = 115px → 窗 = r×2×0.62 ≈ 143px（与原版 grabCenter 一致）
        assert!((b.w - 143).abs() <= 2, "实际 {}", b.w);
        assert_eq!(b.w, b.h);
        assert!((b.x + b.w / 2 - 960).abs() <= 1);
    }
}
