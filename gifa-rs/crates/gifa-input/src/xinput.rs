//! XInput 轮询：OS 层读取手柄，与窗口焦点/用户手势无关。
//!
//! 为什么不用浏览器的 Gamepad API（原版的结论）：Chromium 要求文档收到过真实用户手势
//! 才启动手柄监视，而浮窗是"不抢焦点、鼠标穿透、从不 show"的宿主窗，永远拿不到手势。
//! 为什么不用 gilrs：只为读 4 个槽位的 `XInputGetState`，直连导出的函数更可控，
//! 也少一条依赖链（原版同样是自己轮询 XInput）。
//!
//! 坐标约定：XInput 的 Y 轴**向上为正**，而 Gamepad API / 我们的 `Frame` 是**向下为正**，
//! 所以两根摇杆的 Y 都要取反。搞错这一条，圆盘的南北会整体反过来。

use std::sync::mpsc::Sender;

use crate::gamepad::Frame;

// ---- XInput 结构（与 xinput.h 一致）
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct XInputGamepad {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    thumb_lx: i16,
    thumb_ly: i16,
    thumb_rx: i16,
    thumb_ry: i16,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct XInputState {
    packet_number: u32,
    gamepad: XInputGamepad,
}

pub type GetStateFn = unsafe extern "system" fn(u32, *mut XInputState) -> u32;

const ERROR_SUCCESS: u32 = 0;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

// ---- 按钮位（xinput.h）
const XINPUT_GAMEPAD_DPAD_UP: u16 = 0x0001;
const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;
const XINPUT_GAMEPAD_DPAD_LEFT: u16 = 0x0004;
const XINPUT_GAMEPAD_DPAD_RIGHT: u16 = 0x0008;
const XINPUT_GAMEPAD_START: u16 = 0x0010;
const XINPUT_GAMEPAD_BACK: u16 = 0x0020;
const XINPUT_GAMEPAD_LEFT_THUMB: u16 = 0x0040;
const XINPUT_GAMEPAD_RIGHT_THUMB: u16 = 0x0080;
const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
const XINPUT_GAMEPAD_A: u16 = 0x1000;
const XINPUT_GAMEPAD_B: u16 = 0x2000;
const XINPUT_GAMEPAD_X: u16 = 0x4000;
const XINPUT_GAMEPAD_Y: u16 = 0x8000;

/// 扳机按下的判定阈值（0..255）
const TRIGGER_ON: u8 = 30;

/// Gamepad API 标准映射的下标（与原版 `L3_BUTTON_INDEX = 10` 一致）
pub fn button_indices(buttons: u16, left_trigger: u8, right_trigger: u8) -> [bool; 16] {
    let mut b = [false; 16];
    b[0] = buttons & XINPUT_GAMEPAD_A != 0;
    b[1] = buttons & XINPUT_GAMEPAD_B != 0;
    b[2] = buttons & XINPUT_GAMEPAD_X != 0;
    b[3] = buttons & XINPUT_GAMEPAD_Y != 0;
    b[4] = buttons & XINPUT_GAMEPAD_LEFT_SHOULDER != 0;
    b[5] = buttons & XINPUT_GAMEPAD_RIGHT_SHOULDER != 0;
    b[6] = left_trigger > TRIGGER_ON;
    b[7] = right_trigger > TRIGGER_ON;
    b[8] = buttons & XINPUT_GAMEPAD_BACK != 0;
    b[9] = buttons & XINPUT_GAMEPAD_START != 0;
    b[10] = buttons & XINPUT_GAMEPAD_LEFT_THUMB != 0; // L3
    b[11] = buttons & XINPUT_GAMEPAD_RIGHT_THUMB != 0;
    b[12] = buttons & XINPUT_GAMEPAD_DPAD_UP != 0;
    b[13] = buttons & XINPUT_GAMEPAD_DPAD_DOWN != 0;
    b[14] = buttons & XINPUT_GAMEPAD_DPAD_LEFT != 0;
    b[15] = buttons & XINPUT_GAMEPAD_DPAD_RIGHT != 0;
    b
}

fn axis(v: i16) -> f64 {
    (v as f64 / 32767.0).clamp(-1.0, 1.0)
}

/// XInput 状态 → 统一 `Frame`（Y 轴取反，见模块文档）
pub fn frame_from_state(g: &XInputGamepad) -> Frame {
    Frame {
        buttons: button_indices(g.buttons, g.left_trigger, g.right_trigger),
        axes: [
            axis(g.thumb_lx),
            -axis(g.thumb_ly),
            axis(g.thumb_rx),
            -axis(g.thumb_ry),
        ],
    }
}

/// 加载 xinput DLL 并返回 `XInputGetState`
fn load_get_state() -> Option<GetStateFn> {
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    let names = ["xinput1_4.dll", "xinput1_3.dll", "xinput9_1_0.dll"];
    for name in names {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let Ok(lib) = LoadLibraryW(PCWSTR(wide.as_ptr())) else {
                continue;
            };
            let Some(addr) = GetProcAddress(
                lib,
                windows::core::PCSTR(c"XInputGetState".as_ptr() as *const u8),
            ) else {
                continue;
            };
            let f: GetStateFn = std::mem::transmute(addr);
            return Some(f);
        }
    }
    None
}

/// XInput 可用性（只加载不求状态）
pub fn available() -> bool {
    load_get_state().is_some()
}

/// 当前连着的手柄槽位（没有则 None）——只查状态，不读整帧
pub fn connected() -> Option<u32> {
    let get_state = load_get_state()?;
    for slot in 0..4u32 {
        let mut state = XInputState::default();
        if unsafe { get_state(slot, &mut state) } == ERROR_SUCCESS {
            return Some(slot);
        }
    }
    None
}

/// 轮询一次：返回第一个连着的手柄的槽位与帧
pub fn poll_once(get_state: GetStateFn) -> Option<(u32, Frame)> {
    for slot in 0..4u32 {
        let mut state = XInputState::default();
        let rc = unsafe { get_state(slot, &mut state) };
        match rc {
            ERROR_SUCCESS => return Some((slot, frame_from_state(&state.gamepad))),
            ERROR_DEVICE_NOT_CONNECTED => continue,
            _ => continue,
        }
    }
    None
}

/// 手柄事件：连接状态 + 帧
#[derive(Debug, Clone, PartialEq)]
pub enum GamepadEvent {
    /// `slot` = None 表示没插手柄
    Frame { slot: Option<u32>, frame: Frame },
}

/// 后台 16ms 轮询。只有"帧签名变了"才推一条，静默时不产生消息。
pub fn start_polling(tx: Sender<GamepadEvent>) -> Result<std::thread::JoinHandle<()>, String> {
    let get_state = load_get_state().ok_or_else(|| "找不到 XInput DLL".to_string())?;
    std::thread::Builder::new()
        .name("gifa-xinput".into())
        .spawn(move || {
            let mut last: Option<(Option<u32>, Frame)> = None;
            loop {
                let now = poll_once(get_state);
                let item = match now {
                    Some((slot, frame)) => Some((Some(slot), frame)),
                    None => Some((None, Frame::default())),
                };
                if last != item {
                    if let Some((slot, frame)) = item {
                        let _ = tx.send(GamepadEvent::Frame { slot, frame });
                    }
                    last = item;
                }
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        })
        .map_err(|e| format!("启动手柄轮询线程失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l3_maps_to_button_10() {
        let b = button_indices(XINPUT_GAMEPAD_LEFT_THUMB, 0, 0);
        assert!(b[10]);
        assert_eq!(b.iter().filter(|x| **x).count(), 1);
    }

    #[test]
    fn face_buttons_map_to_standard_order() {
        let b = button_indices(XINPUT_GAMEPAD_A | XINPUT_GAMEPAD_B, 0, 0);
        assert!(b[0] && b[1]);
        assert!(!b[2] && !b[3]);
    }

    #[test]
    fn triggers_become_buttons_6_and_7() {
        let b = button_indices(0, 200, 0);
        assert!(b[6]);
        assert!(!b[7]);
        let b = button_indices(0, 10, 10);
        assert!(!b[6] && !b[7], "低于阈值不算按下");
    }

    #[test]
    fn dpad_maps_to_12_to_15() {
        let b = button_indices(XINPUT_GAMEPAD_DPAD_UP | XINPUT_GAMEPAD_DPAD_RIGHT, 0, 0);
        assert!(b[12] && b[15]);
        assert!(!b[13] && !b[14]);
    }

    #[test]
    fn stick_y_is_inverted() {
        // XInput: 向上推 = +32767；Gamepad API: 向上推 = -1
        let g = XInputGamepad {
            thumb_ly: 32767,
            thumb_ry: -32767,
            thumb_lx: 32767,
            ..Default::default()
        };
        let f = frame_from_state(&g);
        assert!((f.axes[0] - 1.0).abs() < 1e-6, "X 不取反");
        assert!((f.axes[1] + 1.0).abs() < 1e-6, "Y 要取反");
        assert!((f.axes[3] - 1.0).abs() < 1e-6, "右摇杆 Y 同样取反");
    }

    #[test]
    fn center_is_zero_amplitude() {
        let f = frame_from_state(&XInputGamepad::default());
        assert_eq!(f.axes, [0.0; 4]);
        assert!(f.buttons.iter().all(|b| !b));
    }
}
