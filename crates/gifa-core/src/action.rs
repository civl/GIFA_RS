//! 动作：11 个可绑定动作。字符串键与 settings.json 里的 `hotkeys` 同构。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    TogglePlay,
    SeekBack,
    SeekFwd,
    PrevPart,
    NextPart,
    ToggleVisible,
    TogglePass,
    ToggleOpacity,
    ToggleImmersion,
    CycleSpeed,
    HoldSpeed,
}

impl Action {
    pub const ALL: [Action; 11] = [
        Action::TogglePlay,
        Action::SeekBack,
        Action::SeekFwd,
        Action::PrevPart,
        Action::NextPart,
        Action::ToggleVisible,
        Action::TogglePass,
        Action::ToggleOpacity,
        Action::ToggleImmersion,
        Action::CycleSpeed,
        Action::HoldSpeed,
    ];

    /// settings.json 里 `hotkeys` 的键
    pub fn key(self) -> &'static str {
        match self {
            Action::TogglePlay => "togglePlay",
            Action::SeekBack => "seekBack",
            Action::SeekFwd => "seekFwd",
            Action::PrevPart => "prevPart",
            Action::NextPart => "nextPart",
            Action::ToggleVisible => "toggleVisible",
            Action::TogglePass => "togglePass",
            Action::ToggleOpacity => "toggleOpacity",
            Action::ToggleImmersion => "toggleImmersion",
            Action::CycleSpeed => "cycleSpeed",
            Action::HoldSpeed => "holdSpeed",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == k)
    }

    /// 「按住类」动作：按下生效、松开恢复。手柄圆盘用摇杆位置代偿。
    pub fn is_hold(self) -> bool {
        matches!(self, Action::HoldSpeed)
    }

    /// 启动器里显示的中文名
    pub fn label(self) -> &'static str {
        match self {
            Action::TogglePlay => "播放 / 暂停",
            Action::SeekBack => "后退 5 秒",
            Action::SeekFwd => "前进 5 秒",
            Action::PrevPart => "上一分 P",
            Action::NextPart => "下一分 P",
            Action::ToggleVisible => "隐藏 / 显示浮窗",
            Action::TogglePass => "鼠标穿透 开 / 关",
            Action::ToggleOpacity => "半透明 开 / 关",
            Action::ToggleImmersion => "纯视频沉浸 开 / 关",
            Action::CycleSpeed => "循环倍速 1 / 1.25 / 1.5 / 2",
            Action::HoldSpeed => "长按临时加速（松开恢复）",
        }
    }

    /// 手柄圆盘扇区上的窄标签
    pub fn short(self) -> &'static str {
        match self {
            Action::TogglePlay => "暂停",
            Action::SeekBack => "后退",
            Action::SeekFwd => "前进",
            Action::PrevPart => "上一P",
            Action::NextPart => "下一P",
            Action::ToggleVisible => "显隐",
            Action::TogglePass => "穿透",
            Action::ToggleOpacity => "透明",
            Action::ToggleImmersion => "沉浸",
            Action::CycleSpeed => "倍速",
            Action::HoldSpeed => "加速",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip() {
        for a in Action::ALL {
            assert_eq!(Action::from_key(a.key()), Some(a));
        }
        assert_eq!(Action::from_key("nope"), None);
    }

    #[test]
    fn serializes_to_camel_case_string() {
        assert_eq!(
            serde_json::to_string(&Action::TogglePlay).unwrap(),
            "\"togglePlay\""
        );
        assert_eq!(Action::HoldSpeed.key(), "holdSpeed");
        assert!(Action::HoldSpeed.is_hold());
        assert!(!Action::TogglePlay.is_hold());
    }
}
