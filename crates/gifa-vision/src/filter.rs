//! 时序层：α-β 滤波（行进方向）+ 视角状态机。
//!
//! 移植自 `webview-preload.js:1359-1538`。两条通道**各有一套状态**，
//! 因为它们回答的是不同问题：箭头是"人往哪走"，扇形是"镜头朝哪看"。
//!
//! 为什么不用"最近几帧取中值"：中值只延迟不预测，匀速转身时永远拖半拍；
//! 遇到单帧 180° 反相要攒三帧才纠回来，中间那三帧是明确的反向。
//! α-β 多带一个角速度项，匀速时零滞后、静止时又稳。

use crate::arrow::to_compass;

/// 视角宽限：扇形恒在，"这一帧没认出来"不等于"没有视角"
pub const VIEW_GRACE_MS: f64 = 1200.0;
/// 大跳挂起阈值：超过这个角度先不采纳，等下一帧确认
pub const VIEW_JUMP_DEG: f64 = 100.0;
/// 挂起确认容差：下一帧与挂起值相差小于它才承认是真转身
pub const VIEW_PENDING_MATCH_DEG: f64 = 30.0;

#[inline]
fn ang_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

// ------------------------------------------------------------------ 行进方向

/// 圆形 α-β 滤波 + 180° 翻转门限
#[derive(Debug, Clone)]
pub struct AbFilter {
    pub theta: f64,
    pub omega: f64,
    t: f64,
    suspect: u32,
    /// 单帧跳变门限（度）。原版固定 130°；真机实测发现 90~120° 的假跳变会漏进来，
    /// 而 10fps 下物理上限只有 540°/s × 0.1s = 54°，所以管线里调得更紧。
    /// 默认仍是 130，保证和原版逐帧对拍不变。
    pub flip_gate: f64,
}

impl Default for AbFilter {
    fn default() -> Self {
        Self {
            theta: -1.0,
            omega: 0.0,
            t: 0.0,
            suspect: 0,
            flip_gate: 130.0,
        }
    }
}

impl AbFilter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// `now` 是毫秒。返回 `None` = 这一帧被门限挡下，不该上报。
    pub fn smooth(
        &mut self,
        obs: f64,
        conf: f64,
        now: f64,
        cone_deg: f64,
        cone_conf: f64,
    ) -> Option<f64> {
        if self.theta < 0.0 {
            self.theta = obs;
            self.omega = 0.0;
            self.t = now;
            self.suspect = 0;
            return Some(obs);
        }

        // 暂停后恢复 / 切 P：dt 会很大，限幅避免速度项被推飞
        let dt = ((now - self.t) / 1000.0).clamp(0.008, 0.4);
        let pred = (self.theta + self.omega * dt + 360.0) % 360.0;
        let res = ang_diff(obs, pred);

        /* 门限 = min(flip_gate, 物理上限×1.3)，且不低于 60°：
         * 物理上限 = 540°/s × dt（ω 的钳位值），10fps 时是 54°/帧。
         * 真机实测：90~120° 的假跳变会从原版 130° 的门限里漏过去。 */
        let phys = (540.0 * dt * 1.3).max(60.0);
        let gate = if self.flip_gate < 130.0 {
            self.flip_gate.min(phys)
        } else {
            self.flip_gate
        };
        if res.abs() > gate {
            // 疑似 180° 翻转：让扇形投票（它只能回答"翻没翻"）
            let (mut vote_new, mut vote_old) = (0, 0);
            if cone_conf > 0.3 {
                let dn = ang_diff(obs, cone_deg).abs();
                let d0 = ang_diff(self.theta, cone_deg).abs();
                if dn + 15.0 < d0 {
                    vote_new += 1;
                } else if d0 + 15.0 < dn {
                    vote_old += 1;
                }
            }
            self.suspect += 1;
            let confirmed = vote_new > vote_old || (self.suspect >= 2 && conf >= 0.5);
            if !confirmed {
                self.t = now;
                return None;
            }
            self.theta = obs;
            self.omega = 0.0;
            self.suspect = 0;
            self.t = now;
            return Some(obs);
        }

        self.suspect = 0;
        // 增益随置信度浮动：认得越死跟得越快
        let a = 0.32 + 0.3 * conf;
        let b = 0.06 + 0.1 * conf;
        self.theta = (pred + a * res + 360.0) % 360.0;
        self.omega = (self.omega + (b * res) / dt).clamp(-540.0, 540.0);
        self.t = now;
        Some(self.theta)
    }
}

// ------------------------------------------------------------------ 视角通道

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSource {
    /// 这一帧扇形给出的
    Cone,
    /// 宽限期内保持上一帧
    Hold,
    /// 大跳挂起，等下一帧确认
    Pending,
}

impl ViewSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ViewSource::Cone => "cone",
            ViewSource::Hold => "hold",
            ViewSource::Pending => "pending",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ViewOut {
    /// 数学角（atan2 空间）
    pub deg: f64,
    pub conf: f64,
    pub src: ViewSource,
}

/// 一帧视锥观测
#[derive(Debug, Clone, Copy)]
pub struct ConeObs {
    pub deg: f64,
    pub valid: bool,
    pub conf: f64,
    pub width: f64,
}

#[derive(Debug, Clone)]
pub struct ViewState {
    pub theta: f64,
    pub conf: f64,
    t: f64,
    pending: f64,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            theta: -1.0,
            conf: 0.0,
            t: 0.0,
            pending: -1.0,
        }
    }
}

impl ViewState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn update(
        &mut self,
        cone: Option<ConeObs>,
        now: f64,
        map_present: bool,
    ) -> Option<ViewOut> {
        // 小地图确实不在 → 立刻清状态，不走宽限（否则绿弧挂着上一段的方向）
        if !map_present {
            self.theta = -1.0;
            self.conf = 0.0;
            self.t = now;
            self.pending = -1.0;
            return None;
        }

        let ok = cone.is_some_and(|c| c.valid && c.conf > 0.12);

        if !ok {
            if self.theta < 0.0 || now - self.t > VIEW_GRACE_MS {
                self.pending = -1.0;
                return None;
            }
            self.conf = (self.conf * 0.85).max(0.0);
            return Some(ViewOut {
                deg: self.theta,
                conf: self.conf,
                src: ViewSource::Hold,
            });
        }
        let c = cone?;

        // 结算上一帧挂起的大跳：真转身会连着多帧停在新方向，假峰是单帧脉冲
        if self.pending >= 0.0 {
            let p = self.pending;
            self.pending = -1.0;
            if ang_diff(c.deg, p).abs() < VIEW_PENDING_MATCH_DEG {
                self.theta = c.deg;
            }
        }

        let jump = if self.theta < 0.0 {
            999.0
        } else {
            ang_diff(c.deg, self.theta).abs()
        };

        if self.theta < 0.0 {
            self.theta = c.deg;
        } else if jump > VIEW_JUMP_DEG {
            self.pending = c.deg;
            self.conf = c.conf;
            self.t = now;
            return Some(ViewOut {
                deg: self.theta,
                conf: c.conf,
                src: ViewSource::Pending,
            });
        } else {
            let k = 0.10 + 0.62 * c.conf;
            self.theta = (self.theta + ang_diff(c.deg, self.theta) * k + 360.0) % 360.0;
        }

        self.conf = c.conf;
        self.t = now;
        Some(ViewOut {
            deg: self.theta,
            conf: c.conf,
            src: ViewSource::Cone,
        })
    }
}

/// 视角 → 上报字段（出口换算成罗盘角）
pub fn view_payload(
    view: Option<ViewOut>,
    cone_width: f64,
) -> (Option<f64>, f64, &'static str, f64) {
    match view {
        None => (None, 0.0, "", 0.0),
        Some(v) => (
            Some(to_compass(v.deg)),
            (v.conf * 100.0).round() / 100.0,
            v.src.as_str(),
            cone_width.round(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 门限收紧后：90~120° 的假跳变要被拦住，而真实的快速转身（50°/帧）照常跟。
    ///
    /// 背景：10fps 采样下物理上限是 540°/s × 0.1s = 54°/帧，
    /// 原版 130° 的门限会放过 90~120° 的假跳变（真机 40 秒能见到十几次）。
    #[test]
    fn tight_gate_rejects_impossible_jump() {
        let mut f = AbFilter {
            flip_gate: 80.0,
            ..Default::default()
        };
        let mut t = 0.0;
        f.smooth(30.0, 0.9, t, 0.0, 0.0);
        t += 100.0;
        assert!(
            f.smooth(150.0, 0.9, t, 0.0, 0.0).is_none(),
            "120° 单帧跳变应当被拦下"
        );

        // 真实快速转身：50°/帧
        let mut f2 = AbFilter {
            flip_gate: 80.0,
            ..Default::default()
        };
        let mut tt = 0.0;
        let mut last = 0.0f64;
        f2.smooth(0.0, 0.9, tt, 0.0, 0.0);
        for _ in 0..6 {
            tt += 100.0;
            last = (last + 50.0) % 360.0;
            assert!(
                f2.smooth(last, 0.9, tt, 0.0, 0.0).is_some(),
                "50°/帧 的真实转身不该被拦"
            );
        }
    }

    #[test]
    fn first_frame_snaps() {
        let mut f = AbFilter::default();
        let out = f.smooth(42.0, 0.9, 1000.0, -1.0, 0.0);
        assert_eq!(out, Some(42.0));
    }

    #[test]
    fn small_step_is_smoothed_not_snapped() {
        let mut f = AbFilter::default();
        f.smooth(0.0, 0.8, 1000.0, -1.0, 0.0);
        // 10° 的变化：α-β 只追一部分，不会直接跳到 10
        let out = f.smooth(10.0, 0.8, 1100.0, -1.0, 0.0).unwrap();
        assert!(out > 0.0 && out < 10.0, "应平滑过渡，实际 {out}");
    }

    #[test]
    fn big_flip_needs_confirmation() {
        let mut f = AbFilter::default();
        f.smooth(0.0, 0.9, 1000.0, -1.0, 0.0);
        // 180° 跳变、没有扇形支持、置信度不高 → 这一帧被挡下
        assert_eq!(f.smooth(180.0, 0.3, 1100.0, -1.0, 0.0), None);
        // 连续第二次跳、且够可信 → 承认（真掉头）
        assert_eq!(f.smooth(180.0, 0.9, 1200.0, -1.0, 0.0), Some(180.0));
    }

    #[test]
    fn cone_vote_can_confirm_flip_in_one_frame() {
        let mut f = AbFilter::default();
        f.smooth(0.0, 0.9, 1000.0, -1.0, 0.0);
        // 观测 180° 且扇形也指向 180° → 扇形投票直接支持新方向
        let out = f.smooth(180.0, 0.9, 1100.0, 180.0, 0.9);
        assert_eq!(out, Some(180.0));
    }

    #[test]
    fn view_holds_through_dropout_then_expires() {
        let mut v = ViewState::default();
        let c = ConeObs {
            deg: 90.0,
            valid: true,
            conf: 0.8,
            width: 90.0,
        };
        let first = v.update(Some(c), 0.0, true).unwrap();
        assert_eq!(first.src, ViewSource::Cone);
        assert!((first.deg - 90.0).abs() < 1e-9);

        // 漏检：宽限期内保持
        let hold = v.update(None, 500.0, true).unwrap();
        assert_eq!(hold.src, ViewSource::Hold);
        assert!((hold.conf - 0.68).abs() < 1e-9, "conf 逐帧衰减");

        // 超过宽限：放弃
        assert!(v.update(None, 500.0 + VIEW_GRACE_MS + 1.0, true).is_none());
    }

    #[test]
    fn view_map_gone_clears_immediately() {
        let mut v = ViewState::default();
        let c = ConeObs {
            deg: 10.0,
            valid: true,
            conf: 0.9,
            width: 90.0,
        };
        v.update(Some(c), 0.0, true);
        assert!(
            v.update(Some(c), 100.0, false).is_none(),
            "地图不在要立刻清"
        );
    }

    #[test]
    fn view_big_jump_is_held_one_frame() {
        let mut v = ViewState::default();
        let c = |deg: f64| ConeObs {
            deg,
            valid: true,
            conf: 0.9,
            width: 90.0,
        };
        v.update(Some(c(0.0)), 0.0, true);
        // 单帧 180° 假峰：挂起，输出仍是旧角度
        let pending = v.update(Some(c(180.0)), 100.0, true).unwrap();
        assert_eq!(pending.src, ViewSource::Pending);
        assert!((pending.deg - 0.0).abs() < 1e-9);
        // 下一帧不在那儿 → 丢弃挂起，正常平滑
        let back = v.update(Some(c(5.0)), 200.0, true).unwrap();
        assert_eq!(back.src, ViewSource::Cone);
        assert!((back.deg - 0.0).abs() < 1e-9 || back.deg < 10.0);
    }

    #[test]
    fn view_confirmed_jump_snaps_next_frame() {
        let mut v = ViewState::default();
        let c = |deg: f64| ConeObs {
            deg,
            valid: true,
            conf: 0.9,
            width: 90.0,
        };
        v.update(Some(c(0.0)), 0.0, true);
        v.update(Some(c(180.0)), 100.0, true); // 挂起
        let confirmed = v.update(Some(c(185.0)), 200.0, true).unwrap();
        assert!(
            (confirmed.deg - 185.0).abs() < 1e-9,
            "下一帧还在那儿 → 直接就位"
        );
    }
}
