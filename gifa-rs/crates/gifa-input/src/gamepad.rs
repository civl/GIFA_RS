//! 手柄八方向圆盘：按住 L3 弹出、右摇杆选动作。
//!
//! 移植自原版 `gamepad-config.js::stepChord`，连阈值和"防误触"路径一起照搬：
//!
//! ```text
//! 幅度 < deadzone(0.22)      静息漂移，算回中
//! 幅度 ≥ LATCH(0.55)         推到位：锁定扇区并高亮
//! 幅度 < COMMIT(0.35)        回中：触发已锁定的动作
//! ```
//!
//! 三个数值各司其职：锁存免疫扇区边界抖动（不需要角度滞回），
//! 回中阈值比锁存低一截，避免"手抖一下就被判成回中"。

use std::collections::BTreeMap;

use gifa_core::Action;

/// 八方位名，下标即 45° 扇区号（0 从正东起、顺时针，屏幕 y 轴向下）
pub const DIRECTIONS: [&str; 8] = ["e", "se", "s", "sw", "w", "nw", "n", "ne"];

/// XInput LEFT_THUMB / Gamepad API `buttons[10]`
pub const L3_BUTTON_INDEX: usize = 10;
pub const LATCH_AMPLITUDE: f64 = 0.55;
pub const COMMIT_AMPLITUDE: f64 = 0.35;
pub const DEFAULT_DEADZONE: f64 = 0.22;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub buttons: [bool; 16],
    /// 标准映射：`[lx, ly, rx, ry]`，右杆在 `[2]`/`[3]`
    pub axes: [f64; 4],
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            buttons: [false; 16],
            axes: [0.0; 4],
        }
    }
}

impl Frame {
    pub fn l3(&self) -> bool {
        self.buttons[L3_BUTTON_INDEX]
    }

    /// 右摇杆推到某个方位（不带 L3 语义，纯几何）
    pub fn push_direction(&mut self, direction: &str, amplitude: f64) {
        let idx = DIRECTIONS.iter().position(|d| *d == direction).unwrap_or(0);
        let angle = (idx as f64) * std::f64::consts::PI / 4.0;
        self.axes[2] = angle.cos() * amplitude;
        self.axes[3] = angle.sin() * amplitude;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChordConfig {
    pub deadzone: f64,
    /// 方位名 → 动作
    pub map: BTreeMap<String, Action>,
}

impl Default for ChordConfig {
    fn default() -> Self {
        Self {
            deadzone: DEFAULT_DEADZONE,
            map: default_map(),
        }
    }
}

pub fn default_map() -> BTreeMap<String, Action> {
    [
        ("n", Action::PrevPart),
        ("ne", Action::CycleSpeed),
        ("e", Action::SeekFwd),
        ("se", Action::ToggleOpacity),
        ("s", Action::NextPart),
        ("sw", Action::TogglePass),
        ("w", Action::SeekBack),
        ("nw", Action::TogglePlay),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChordState {
    pub l3: bool,
    pub latched: Option<usize>,
    /// 刚按下 L3 时杆就没回中：必须等一次回中才允许触发（防误触）
    pub awaiting_release: bool,
    pub holding: Option<Action>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChordStep {
    /// 圆盘是否该显示
    pub active: bool,
    /// 高亮扇区下标
    pub highlighted: Option<usize>,
    /// 一次性动作（回中那刻触发）
    pub fired: Option<Action>,
    /// 按住类动作：按下
    pub press: Option<Action>,
    /// 按住类动作：松开
    pub release: Option<Action>,
}

fn amplitude(x: f64, y: f64) -> f64 {
    if x.is_finite() && y.is_finite() {
        (x * x + y * y).sqrt()
    } else {
        0.0
    }
}

fn effective_deadzone(cfg: &ChordConfig) -> f64 {
    cfg.deadzone.min(COMMIT_AMPLITUDE - 0.05)
}

/// 角度量化：屏幕坐标 y 向下，所以 `atan2(y, x)` 的 0° 是正东、90° 是正南
pub fn quantize(axes: &[f64; 4], cfg: &ChordConfig) -> Option<usize> {
    let (x, y) = (axes[2], axes[3]);
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    if amplitude(x, y) < effective_deadzone(cfg) {
        return None;
    }
    let degrees = y.atan2(x).to_degrees();
    let sector = (((degrees + 360.0) % 360.0) / 45.0).round() as i64 % 8;
    Some(sector as usize)
}

pub fn step(state: &ChordState, frame: &Frame, cfg: &ChordConfig) -> (ChordState, ChordStep) {
    let pressed = frame.l3();
    let amp = amplitude(frame.axes[2], frame.axes[3]);
    let preview = quantize(&frame.axes, cfg);

    let idle = ChordStep::default();

    // L3 没按、之前也没按
    if !state.l3 && !pressed {
        return (state.clone(), idle);
    }
    // L3 抬起：没回中的候选作废；但按住中的动作必须补一发 release，
    // 否则「未回中就抬杆」会把播放留在临时倍率上。
    if !pressed {
        return (
            ChordState::default(),
            ChordStep {
                release: state.holding,
                ..idle
            },
        );
    }
    // 刚按下
    if !state.l3 {
        return (
            ChordState {
                l3: true,
                latched: None,
                awaiting_release: amp >= COMMIT_AMPLITUDE,
                holding: None,
            },
            ChordStep {
                active: true,
                ..idle
            },
        );
    }
    // 等回中
    if state.awaiting_release {
        if amp >= COMMIT_AMPLITUDE {
            return (
                state.clone(),
                ChordStep {
                    active: true,
                    ..idle
                },
            );
        }
        return (
            ChordState {
                awaiting_release: false,
                ..state.clone()
            },
            ChordStep {
                active: true,
                ..idle
            },
        );
    }

    // 满推才锁存；轻推连高亮都不给，免得看着像选中了却什么都不触发
    let latched = if amp >= LATCH_AMPLITUDE { preview } else { state.latched };

    // 回中：触发锁存的动作
    if amp < COMMIT_AMPLITUDE {
        let action = latched.and_then(|i| cfg.map.get(DIRECTIONS[i]).copied());
        return (
            ChordState {
                l3: true,
                latched: None,
                awaiting_release: false,
                holding: None,
            },
            ChordStep {
                active: true,
                highlighted: None,
                // 按住类动作在过锁存线那刻就生效了，回中只负责收尾
                fired: action.filter(|a| !a.is_hold()),
                press: None,
                release: state.holding,
            },
        );
    }

    let holding = latched
        .and_then(|i| cfg.map.get(DIRECTIONS[i]).copied())
        .filter(|a| a.is_hold());

    let press = holding.filter(|h| Some(*h) != state.holding);
    let release = state.holding.filter(|h| Some(*h) != holding);
    (
        ChordState {
            latched,
            awaiting_release: false,
            holding,
            ..state.clone()
        },
        ChordStep {
            active: true,
            highlighted: latched,
            fired: None,
            press,
            release,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ChordConfig {
        ChordConfig::default()
    }

    #[test]
    fn quantizes_eight_sectors() {
        let c = cfg();
        for (i, name) in DIRECTIONS.iter().enumerate() {
            let mut f = Frame::default();
            f.push_direction(name, 1.0);
            assert_eq!(quantize(&f.axes, &c), Some(i), "方位 {name}");
        }
    }

    #[test]
    fn deadzone_reads_as_center() {
        let mut f = Frame::default();
        f.axes[2] = 0.1;
        f.axes[3] = 0.1;
        assert_eq!(quantize(&f.axes, &cfg()), None);
    }

    #[test]
    fn no_l3_no_disc() {
        let mut f = Frame::default();
        f.push_direction("e", 1.0);
        let (st, sd) = step(&ChordState::default(), &f, &cfg());
        assert!(!sd.active);
        assert!(!st.l3);
    }

    #[test]
    fn press_highlights_then_center_fires() {
        let c = cfg();
        let mut f = Frame::default();
        f.buttons[L3_BUTTON_INDEX] = true;
        // 先回中按下 L3
        let (st, sd) = step(&ChordState::default(), &f, &c);
        assert!(sd.active && st.l3 && !st.awaiting_release);

        // 推到正东（e → seekFwd）
        f.push_direction("e", 0.9);
        let (st, sd) = step(&st, &f, &c);
        assert_eq!(sd.highlighted, Some(0));
        assert_eq!(sd.fired, None, "满推只高亮，不触发");

        // 回中
        f.axes[2] = 0.0;
        f.axes[3] = 0.0;
        let (_st, sd) = step(&st, &f, &c);
        assert_eq!(sd.fired, Some(Action::SeekFwd));
        assert_eq!(sd.highlighted, None);
    }

    #[test]
    fn light_push_does_not_highlight() {
        let c = cfg();
        let mut f = Frame::default();
        f.buttons[L3_BUTTON_INDEX] = true;
        let (st, _) = step(&ChordState::default(), &f, &c);
        // 0.45：过了回中线(0.35)、没到锁存线(0.55)
        f.push_direction("e", 0.45);
        let (st2, sd) = step(&st, &f, &c);
        assert_eq!(sd.highlighted, None);
        assert_eq!(sd.fired, None);
        // 再回中也不触发
        f.axes[2] = 0.0;
        f.axes[3] = 0.0;
        let (_s, sd) = step(&st2, &f, &c);
        assert_eq!(sd.fired, None);
    }

    #[test]
    fn latched_zone_survives_jitter() {
        let c = cfg();
        let mut f = Frame::default();
        f.buttons[L3_BUTTON_INDEX] = true;
        let (st, _) = step(&ChordState::default(), &f, &c);

        f.push_direction("e", 0.9);
        let (st, _) = step(&st, &f, &c);
        // 掉到 0.45（高于回中线、低于锁存线）——锁存仍在，仍算选中东
        f.push_direction("e", 0.45);
        let (st, sd) = step(&st, &f, &c);
        assert_eq!(sd.highlighted, Some(0));
        assert!(st.latched.is_some());
    }

    #[test]
    fn hold_action_reports_press_and_release() {
        let mut c = cfg();
        c.map.insert("e".into(), Action::HoldSpeed);
        let mut f = Frame::default();
        f.buttons[L3_BUTTON_INDEX] = true;
        let (st, _) = step(&ChordState::default(), &f, &c);

        f.push_direction("e", 0.9);
        let (st, sd) = step(&st, &f, &c);
        assert_eq!(sd.press, Some(Action::HoldSpeed), "过锁存线立刻生效");

        f.axes[2] = 0.0;
        f.axes[3] = 0.0;
        let (_st, sd) = step(&st, &f, &c);
        assert_eq!(sd.release, Some(Action::HoldSpeed));
        assert_eq!(sd.fired, None, "按住类不能重复 fire");
    }

    #[test]
    fn l3_up_mid_hold_still_releases() {
        let mut c = cfg();
        c.map.insert("e".into(), Action::HoldSpeed);
        let mut f = Frame::default();
        f.buttons[L3_BUTTON_INDEX] = true;
        let (st, _) = step(&ChordState::default(), &f, &c);
        f.push_direction("e", 0.9);
        let (st, _) = step(&st, &f, &c);

        // 没回中就抬杆
        f.buttons[L3_BUTTON_INDEX] = false;
        let (_st, sd) = step(&st, &f, &c);
        assert_eq!(sd.release, Some(Action::HoldSpeed));
        assert!(!sd.active);
    }

    #[test]
    fn pressing_while_pushed_requires_return_to_center_first() {
        let c = cfg();
        let mut f = Frame::default();
        // 一按下 L3 杆就已经在 0.4（高于回中线）
        f.buttons[L3_BUTTON_INDEX] = true;
        f.push_direction("e", 0.4);
        let (st, _) = step(&ChordState::default(), &f, &c);
        assert!(st.awaiting_release);

        // 直接推满也不高亮
        f.push_direction("e", 0.9);
        let (st, sd) = step(&st, &f, &c);
        assert_eq!(sd.highlighted, None);
        assert!(st.awaiting_release);

        // 回中后才解除
        f.axes[2] = 0.0;
        f.axes[3] = 0.0;
        let (st, _) = step(&st, &f, &c);
        assert!(!st.awaiting_release);
        assert_eq!(st.latched, None);
    }
}
