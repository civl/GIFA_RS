//! 设置模型：与 Electron 版的 `settings.json` **同构**（字段同名同义），
//! 这样老用户可以直接把旧文件丢进来。
//!
//! 合并策略：`apply_patch` 做深合并（对象递归、其余替换），然后 `normalize` 钳位。
//! 未知字段保留在 `extra` 里，避免「新版本写、旧版本读」时丢配置。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::action::Action;
use crate::bearing::Layouts;

pub const PLAYBACK_RATES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
pub const HOLD_PLAYBACK_RATES: [f64; 5] = [1.25, 1.5, 2.0, 2.5, 3.0];

pub const MIN_W: i32 = 240;
pub const MIN_H: i32 = 150;
pub const MIN_OPACITY: f64 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Bounds {
    pub width: i32,
    pub height: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// 用户手动拉过之后置 2，程序不再做一次性 16:9 纠正
    pub aspect: i32,
}

impl Default for Bounds {
    fn default() -> Self {
        // 内容区 896×504 正好 16:9，加标题栏 30 + 控制条 34 = 568
        Self {
            width: 896,
            height: 568,
            x: None,
            y: None,
            aspect: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiscSettings {
    pub radius: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for DiscSettings {
    fn default() -> Self {
        Self {
            radius: 132,
            x: None,
            y: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StickSettings {
    pub deadzone: f64,
}

impl Default for StickSettings {
    fn default() -> Self {
        Self { deadzone: 0.22 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GamepadSettings {
    pub enabled: bool,
    pub disc: DiscSettings,
    pub stick: StickSettings,
    /// 方位名 → 动作 key
    pub map: BTreeMap<String, String>,
}

impl Default for GamepadSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            disc: DiscSettings::default(),
            stick: StickSettings::default(),
            map: default_gamepad_map(),
        }
    }
}

pub fn default_gamepad_map() -> BTreeMap<String, String> {
    [
        ("n", "prevPart"),
        ("ne", "cycleSpeed"),
        ("e", "seekFwd"),
        ("se", "toggleOpacity"),
        ("s", "nextPart"),
        ("sw", "togglePass"),
        ("w", "seekBack"),
        ("nw", "togglePlay"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BearingSettings {
    pub layouts: Layouts,
    pub enabled: bool,
    pub radius: i32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// 旧版字段（`rois` / `roiBy`）原样留在这里，供迁移读取
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for BearingSettings {
    fn default() -> Self {
        Self {
            layouts: Layouts::default(),
            enabled: false,
            radius: 110,
            x: None,
            y: None,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub bounds: Bounds,
    pub pinned: bool,
    pub opacity: f64,
    pub click_through: bool,
    pub mode: String,
    pub volume: f64,
    pub muted: bool,
    pub playback_rate: f64,
    pub hold_playback_rate: f64,
    pub hover_fade_enabled: bool,
    pub muxed: bool,
    pub last_text: String,
    pub last_part: i32,
    pub last_title: String,
    pub last_video_id: String,
    pub last_page_count: i32,
    pub last_watched_at: i64,
    pub resume_on_launch: bool,
    /// 动作 key → 加速器
    pub hotkeys: BTreeMap<String, String>,
    pub bearing: BearingSettings,
    pub gamepad: GamepadSettings,
    /// 未知字段（版本兼容）
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bounds: Bounds::default(),
            pinned: true,
            opacity: 1.0,
            click_through: false,
            mode: "web".into(),
            volume: 100.0,
            muted: false,
            playback_rate: 1.0,
            hold_playback_rate: 2.0,
            hover_fade_enabled: true,
            muxed: false,
            last_text: String::new(),
            last_part: 1,
            last_title: String::new(),
            last_video_id: String::new(),
            last_page_count: 0,
            last_watched_at: 0,
            resume_on_launch: true,
            hotkeys: default_hotkeys(),
            bearing: BearingSettings::default(),
            gamepad: GamepadSettings::default(),
            extra: serde_json::Map::new(),
        }
    }
}

pub fn default_hotkeys() -> BTreeMap<String, String> {
    [
        (Action::TogglePlay, "Alt+Space"),
        (Action::SeekBack, "Alt+Left"),
        (Action::SeekFwd, "Alt+Right"),
        (Action::PrevPart, "Alt+Up"),
        (Action::NextPart, "Alt+Down"),
        (Action::ToggleVisible, "Alt+H"),
        (Action::TogglePass, "Alt+L"),
        (Action::ToggleOpacity, "Alt+O"),
        (Action::ToggleImmersion, "Alt+I"),
        (Action::CycleSpeed, "Alt+S"),
        (Action::HoldSpeed, "F8"),
    ]
    .into_iter()
    .map(|(a, v)| (a.key().to_string(), v.to_string()))
    .collect()
}

impl Settings {
    pub fn hotkey(&self, action: Action) -> Option<&str> {
        self.hotkeys.get(action.key()).map(String::as_str)
    }

    pub fn set_hotkey(&mut self, action: Action, accelerator: &str) {
        self.hotkeys
            .insert(action.key().to_string(), accelerator.trim().to_string());
    }

    /// 深合并一个 patch（来自 IPC 的部分更新）
    pub fn apply_patch(&mut self, patch: &Value) -> Result<(), String> {
        let Value::Object(_) = patch else {
            return Err("patch 必须是对象".into());
        };
        let mut base = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        merge(&mut base, patch);
        *self = serde_json::from_value(base).map_err(|e| e.to_string())?;
        self.normalize();
        Ok(())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut s: Settings = serde_json::from_str(text).map_err(|e| e.to_string())?;
        s.normalize();
        Ok(s)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// 钳位到合法范围（非法值不崩，退化成默认/边界）
    pub fn normalize(&mut self) {
        self.opacity = clamp(self.opacity, MIN_OPACITY, 1.0);
        self.volume = clamp(self.volume, 0.0, 100.0);
        self.playback_rate = clamp(self.playback_rate, 0.25, 3.0);
        self.hold_playback_rate = snap_rate(self.hold_playback_rate, &HOLD_PLAYBACK_RATES);
        self.last_part = self.last_part.max(1);
        if !matches!(self.bounds.aspect, 1 | 2) {
            self.bounds.aspect = 1;
        }
        self.bounds.width = self.bounds.width.max(MIN_W);
        self.bounds.height = self.bounds.height.max(MIN_H);
        self.bearing.radius = clamp_i(self.bearing.radius, 50, 400);
        self.gamepad.disc.radius = clamp_i(self.gamepad.disc.radius, 60, 320);
        self.gamepad.stick.deadzone = clamp(self.gamepad.stick.deadzone, 0.0, 0.9);
        if self.mode.is_empty() {
            self.mode = "web".into();
        }
        // 老配置文件可能只写了几个键：缺的补默认（显式写成 "" 表示用户解绑，保留）
        for (k, v) in default_hotkeys() {
            self.hotkeys.entry(k).or_insert(v);
        }
        for (k, v) in default_gamepad_map() {
            self.gamepad.map.entry(k).or_insert(v);
        }
    }
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        lo
    }
}

fn clamp_i(v: i32, lo: i32, hi: i32) -> i32 {
    v.clamp(lo, hi)
}

fn snap_rate(v: f64, allowed: &[f64]) -> f64 {
    allowed
        .iter()
        .copied()
        .min_by(|a, b| (a - v).abs().partial_cmp(&(b - v).abs()).unwrap())
        .unwrap_or(allowed[0])
}

fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_electron_schema() {
        let s = Settings::default();
        let v: Value = serde_json::from_str(&s.to_json_pretty()).unwrap();
        assert_eq!(v["bounds"]["width"], 896);
        assert_eq!(v["bounds"]["height"], 568);
        assert_eq!(v["hotkeys"]["togglePlay"], "Alt+Space");
        assert_eq!(v["hotkeys"]["holdSpeed"], "F8");
        assert_eq!(v["bearing"]["layouts"]["activeLayoutId"], "builtin");
        assert_eq!(v["gamepad"]["map"]["nw"], "togglePlay");
        assert_eq!(v["gamepad"]["stick"]["deadzone"], 0.22);
    }

    #[test]
    fn patch_merges_deeply_without_wiping_siblings() {
        let mut s = Settings::default();
        s.apply_patch(&serde_json::json!({"volume": 42}))
            .unwrap();
        assert_eq!(s.volume, 42.0);
        // 同级的其它字段不受影响
        assert_eq!(s.hotkeys.get("togglePlay").unwrap(), "Alt+Space");
        assert_eq!(s.gamepad.disc.radius, 132);

        s.apply_patch(&serde_json::json!({"hotkeys": {"seekBack": "Alt+J"}}))
            .unwrap();
        assert_eq!(s.hotkey(Action::SeekBack), Some("Alt+J"));
        assert_eq!(s.hotkey(Action::SeekFwd), Some("Alt+Right"));
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn normalize_clamps_out_of_range() {
        let mut s = Settings::default();
        s.opacity = 0.05;
        s.volume = 500.0;
        s.hold_playback_rate = 1.7;
        s.bearing.radius = 9999;
        s.gamepad.stick.deadzone = -1.0;
        s.normalize();
        assert_eq!(s.opacity, MIN_OPACITY);
        assert_eq!(s.volume, 100.0);
        assert_eq!(s.hold_playback_rate, 1.5, "吸附到最近的允许倍率");
        assert_eq!(s.bearing.radius, 400);
        assert_eq!(s.gamepad.stick.deadzone, 0.0);
    }

    #[test]
    fn unknown_fields_survive_roundtrip() {
        let mut s = Settings::default();
        s.apply_patch(&serde_json::json!({"futureFlag": {"a": 1}}))
            .unwrap();
        let back = Settings::from_json(&s.to_json_pretty()).unwrap();
        assert_eq!(back.extra["futureFlag"]["a"], 1);
    }

    #[test]
    fn reads_legacy_electron_settings() {
        // 原版 settings.json 的关键字段都能直接读进来
        let legacy = r#"{
            "bounds": {"width": 896, "height": 568, "x": 100, "y": 50, "aspect": 2},
            "pinned": false,
            "opacity": 0.65,
            "lastText": "BV1xx411c7mD",
            "lastPart": 3,
            "hotkeys": {"togglePlay": "Alt+Space", "holdSpeed": "F8"},
            "bearing": {"enabled": true, "radius": 130}
        }"#;
        let s = Settings::from_json(legacy).unwrap();
        assert_eq!(s.bounds.x, Some(100));
        assert!(!s.pinned);
        assert_eq!(s.opacity, 0.65);
        assert_eq!(s.last_part, 3);
        assert_eq!(s.bearing.radius, 130);
        assert!(s.bearing.enabled);
        // 缺的字段用默认补齐
        assert_eq!(s.hotkey(Action::SeekFwd), Some("Alt+Right"));
        assert_eq!(s.gamepad.disc.radius, 132);
    }
}
