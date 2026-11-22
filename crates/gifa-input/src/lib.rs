//! 输入层：手柄（纯逻辑）+ 全局热键（Win32）。

pub mod gamepad;

#[cfg(windows)]
pub mod hook;
#[cfg(windows)]
pub mod hotkey;
#[cfg(windows)]
pub mod xinput;

pub use gamepad::{ChordConfig, ChordState, ChordStep, Frame};

#[cfg(windows)]
pub use hook::HotkeyEvent;
#[cfg(windows)]
pub use hotkey::{ApplyReport, Failure, FailureReason, Hotkeys};
#[cfg(windows)]
pub use xinput::GamepadEvent;

#[cfg(windows)]
pub(crate) fn hinstance() -> windows::Win32::Foundation::HINSTANCE {
    let hm = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
        .expect("GetModuleHandleW");
    windows::Win32::Foundation::HINSTANCE(hm.0)
}
