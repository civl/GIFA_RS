//! 播放状态机：倍速循环、长按临时加速、半透明/穿透开关。
//!
//! 这些是「纯状态」——不碰窗口也不碰播放器，所以能被单测覆盖，
//! 也避免同一套语义在 Rust 和页面 JS 里各写一遍还写歪。

use crate::settings::PLAYBACK_RATES;

pub const DEFAULT_OPACITY: f64 = 1.0;
pub const FADED_OPACITY: f64 = 0.65;
pub const HOVER_FADE_OPACITY: f64 = 0.1;

/// 循环倍速：1 → 1.25 → 1.5 → 2 → 1
pub fn next_rate(current: f64) -> f64 {
    let idx = PLAYBACK_RATES
        .iter()
        .position(|r| (r - current).abs() < 1e-6)
        .unwrap_or(0);
    PLAYBACK_RATES[(idx + 1) % PLAYBACK_RATES.len()]
}

/// 半透明开关：不透明 → 0.65 → 不透明
pub fn toggle_opacity(current: f64) -> f64 {
    if current > 0.95 {
        FADED_OPACITY
    } else {
        DEFAULT_OPACITY
    }
}

/// 长按临时加速的状态。
///
/// 用「来源集合」而不是布尔：键盘和手柄可以同时按住，先松的那个不能把后松的取消掉。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HoldSpeed {
    sources: std::collections::BTreeSet<&'static str>,
}

impl HoldSpeed {
    pub fn is_active(&self) -> bool {
        !self.sources.is_empty()
    }

    /// 返回「**生效状态**是否发生变化」（多来源按住时，第二个来源不算变化）
    pub fn press(&mut self, source: &'static str) -> bool {
        let before = self.is_active();
        self.sources.insert(source);
        self.is_active() != before
    }

    /// 同上：还有别的来源按着时，松掉一个不算变化
    pub fn release(&mut self, source: &'static str) -> bool {
        let before = self.is_active();
        self.sources.remove(source);
        self.is_active() != before
    }

    /// 取消所有来源（休眠、锁屏、换片、改绑）
    pub fn cancel_all(&mut self) -> bool {
        let had = !self.sources.is_empty();
        self.sources.clear();
        had
    }

    /// 生效倍率：按住时是临时倍率，否则是常规倍率
    pub fn effective_rate(&self, normal: f64, hold: f64) -> f64 {
        if self.is_active() {
            hold
        } else {
            normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_cycles_and_wraps() {
        assert_eq!(next_rate(1.0), 1.25);
        assert_eq!(next_rate(1.25), 1.5);
        assert_eq!(next_rate(1.5), 2.0);
        assert_eq!(next_rate(2.0), 1.0);
        // 非标称值从第一档重新开始
        assert_eq!(next_rate(1.33), 1.25);
    }

    #[test]
    fn opacity_toggles_between_two_levels() {
        assert_eq!(toggle_opacity(1.0), FADED_OPACITY);
        assert_eq!(toggle_opacity(0.65), 1.0);
        assert_eq!(toggle_opacity(0.8), 1.0);
    }

    #[test]
    fn hold_speed_tracks_multiple_sources() {
        let mut h = HoldSpeed::default();
        assert!(!h.is_active());
        assert!(h.press("keyboard"));
        assert!(!h.press("keyboard"), "重复按下不算变化");
        assert!(!h.press("gamepad"), "已经有来源按着，第二个来源不改变生效状态");
        assert!(h.is_active());
        assert_eq!(h.effective_rate(1.0, 2.0), 2.0);
        // 先松键盘，手柄还按着 → 仍然生效
        assert!(!h.release("keyboard"));
        assert_eq!(h.effective_rate(1.0, 2.0), 2.0);
        assert!(h.release("gamepad"));
        assert_eq!(h.effective_rate(1.0, 2.0), 1.0);
    }

    #[test]
    fn cancel_all_clears_state() {
        let mut h = HoldSpeed::default();
        h.press("keyboard");
        assert!(h.cancel_all());
        assert!(!h.is_active());
        assert!(!h.cancel_all(), "已经空了就不算变化");
    }
}
