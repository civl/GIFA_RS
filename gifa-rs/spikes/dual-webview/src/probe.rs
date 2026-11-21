//! 客观观测手段：从屏幕 DC 取像素、用 `WindowFromPoint` 判命中归属。
//!
//! 这两件事是 spike 能做到「自动判 PASS/FAIL」的关键 —— 不靠肉眼看截图。

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC, CLR_INVALID};
use windows::Win32::UI::WindowsAndMessaging::{IsChild, WindowFromPoint};

pub type Rgb = (u8, u8, u8);

/// 取屏幕上一点的合成后颜色（含 DWM 合成结果）。
pub fn sample(x: i32, y: i32) -> Option<Rgb> {
    unsafe {
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let c = GetPixel(hdc, x, y);
        let _ = ReleaseDC(None, hdc);
        if c.0 == CLR_INVALID {
            return None;
        }
        let v = c.0;
        Some(((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8))
    }
}

pub fn close(a: Rgb, b: Rgb, tol: i32) -> bool {
    (a.0 as i32 - b.0 as i32).abs() <= tol
        && (a.1 as i32 - b.1 as i32).abs() <= tol
        && (a.2 as i32 - b.2 as i32).abs() <= tol
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// 落在舞台（B 站页面）上 —— 正是我们要的「点击穿透到网页」
    Stage,
    /// 落在我们的 chrome 界面上
    Chrome,
    /// 落在父窗空白处（说明两块子窗都没盖住，多半是布局算错了）
    Parent,
    Other,
}

/// 判断屏幕坐标处的窗口归属。用 `IsChild` 是因为真正接住鼠标的是
/// WebView2 内部的 Chrome_WidgetWin_0，而不是我们创建的子窗本身。
pub fn classify(x: i32, y: i32, parent: HWND, stage: HWND, chrome: HWND) -> Hit {
    unsafe {
        let hit = WindowFromPoint(POINT { x, y });
        if hit.is_invalid() {
            return Hit::Other;
        }
        if hit == stage || IsChild(stage, hit).as_bool() {
            return Hit::Stage;
        }
        if hit == chrome || IsChild(chrome, hit).as_bool() {
            return Hit::Chrome;
        }
        if hit == parent || IsChild(parent, hit).as_bool() {
            return Hit::Parent;
        }
        Hit::Other
    }
}
