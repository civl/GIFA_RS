//! 观测工具：屏幕取像素 + `WindowFromPoint` 归属判定。
//!
//! 自检/回归用它来**客观判定**，而不是靠肉眼看截图。

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
        Some((
            (v & 0xFF) as u8,
            ((v >> 8) & 0xFF) as u8,
            ((v >> 16) & 0xFF) as u8,
        ))
    }
}

pub fn close(a: Rgb, b: Rgb, tol: i32) -> bool {
    (a.0 as i32 - b.0 as i32).abs() <= tol
        && (a.1 as i32 - b.1 as i32).abs() <= tol
        && (a.2 as i32 - b.2 as i32).abs() <= tol
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// 落在舞台窗（及其 WebView2）上 —— 也就是"点击会进 B 站页面"
    Float,
    /// 落在 chrome 界面层上（顶栏/底栏/URL 条/浮动控件）
    Chrome,
    Other,
}

/// 判断屏幕坐标处的窗口归属。
///
/// 用 `IsChild` 是因为真正接住鼠标的是 WebView2 内部的 `Chrome_WidgetWin_0`，
/// 而不是我们自己创建的那个 HWND。
pub fn classify(x: i32, y: i32, float: HWND, chrome: HWND) -> Hit {
    unsafe {
        let hit = WindowFromPoint(POINT { x, y });
        if hit.is_invalid() {
            return Hit::Other;
        }
        if hit == chrome || IsChild(chrome, hit).as_bool() {
            return Hit::Chrome;
        }
        if hit == float || IsChild(float, hit).as_bool() {
            return Hit::Float;
        }
        Hit::Other
    }
}
