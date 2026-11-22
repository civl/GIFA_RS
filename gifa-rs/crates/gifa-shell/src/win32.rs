//! Win32 基础工具。刻意极薄：GIFA 外壳要自己接管 WndProc，不引入 tao/winit。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn hinstance() -> windows::Win32::Foundation::HINSTANCE {
    let hm = unsafe { GetModuleHandleW(None).expect("GetModuleHandleW") };
    windows::Win32::Foundation::HINSTANCE(hm.0)
}

/// 进程级初始化：Per-Monitor-V2 + 单线程 COM。
///
/// WebView2 的控制器创建是 COM 调用，**必须先 CoInitializeEx**，
/// 否则 `build_as_child` 会卡住（不是报错，是直接不返回）。
pub fn init_process() {
    // WebView2 的浏览器参数只能通过环境变量传（必须在环境创建前设置）：
    //   · autoplay-policy          浮窗必须能无手势自动播放
    //   · UseEcoQoSForBackgroundProcess  否则窗口最小化后进程被降为 EcoQoS，
    //     低级钩子回调超时、甚至被系统摘掉（原版同样处理）
    if std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_none() {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--autoplay-policy=no-user-gesture-required --disable-features=UseEcoQoSForBackgroundProcess",
        );
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }
}

pub fn register_class(name: PCWSTR, wndproc: WNDPROC) {
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: wndproc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance(),
        hIcon: HICON::default(),
        hCursor: HCURSOR::default(),
        // 自己画背景：留默认刷子会在 WebView2 出首帧前闪白
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: name,
    };
    unsafe {
        if RegisterClassW(&wc) == 0 {
            // 1410 = ERROR_CLASS_ALREADY_EXISTS（同进程重复注册）
            let err = windows::Win32::Foundation::GetLastError().0;
            if err != 1410 {
                panic!("RegisterClassW 失败, GetLastError={err}");
            }
        }
    }
}

pub fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    (rc.right - rc.left, rc.bottom - rc.top)
}

pub fn window_rect(hwnd: HWND) -> RECT {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut rc);
    }
    rc
}

pub fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut pt = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
    }
    (pt.x, pt.y)
}

/// 注入左键按下/抬起（自检用：拖动标题栏这类交互需要真实鼠标事件）
pub fn click_left(down: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT,
    };
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: if down {
                    MOUSEEVENTF_LEFTDOWN
                } else {
                    MOUSEEVENTF_LEFTUP
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

/// 左键是否按着（自绘拖拽用，比设鼠标捕获可靠）
pub fn left_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

/// 移动系统光标（自检/调试用；正常界面不会调用）
pub fn move_cursor(x: i32, y: i32) -> bool {
    unsafe { SetCursorPos(x, y).is_ok() }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    (pt.x, pt.y)
}

/// 窗口所在显示器的**工作区**（已扣除任务栏）。
pub fn work_area(hwnd: HWND) -> RECT {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut info).as_bool() {
            info.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        }
    }
}

pub fn dpi(hwnd: HWND) -> u32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 {
        96
    } else {
        d
    }
}

pub fn scale(hwnd: HWND) -> f64 {
    dpi(hwnd) as f64 / 96.0
}

/// 把窗口标记为分层窗，并设置整体不透明度（0..255）。
/// 实测：该 alpha 会连子 WebView2 一起变淡，一次调用即可。
pub fn set_opacity(hwnd: HWND, alpha: u8) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if ex & WS_EX_LAYERED.0 as isize == 0 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        let _ = SetLayeredWindowAttributes(
            hwnd,
            windows::Win32::Foundation::COLORREF(0),
            alpha,
            LWA_ALPHA,
        );
    }
}

/// 读回当前窗口的整体不透明度（自检用）。
pub fn get_opacity(hwnd: HWND) -> Option<u8> {
    unsafe {
        let mut alpha: u8 = 0;
        let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS::default();
        GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags))
            .ok()
            .map(|_| alpha)
    }
}

pub fn topmost(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn is_topmost(hwnd: HWND) -> bool {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        ex & WS_EX_TOPMOST.0 as isize != 0
    }
}

/// 通用消息循环，直到收到 WM_QUIT。
pub fn run_message_loop() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }
}

/// 把 HWND 包成 wry 需要的 `HasWindowHandle`。
#[derive(Clone, Copy)]
pub struct HwndHost(pub isize);

impl raw_window_handle::HasWindowHandle for HwndHost {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let nz = std::num::NonZeroIsize::new(self.0)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        Ok(unsafe {
            let handle = raw_window_handle::Win32WindowHandle::new(nz);
            raw_window_handle::WindowHandle::borrow_raw(raw_window_handle::RawWindowHandle::Win32(
                handle,
            ))
        })
    }
}

/// 从 lParam 里取屏幕坐标（WM_NCHITTEST / WM_NCLBUTTONDOWN 等用）。
pub fn lparam_point(lp: LPARAM) -> (i32, i32) {
    let v = lp.0 as u32;
    let x = (v & 0xFFFF) as u16 as i16 as i32;
    let y = ((v >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

pub fn make_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y as u32) << 16) | (x as u32 & 0xFFFF)) as isize)
}

pub fn result(v: isize) -> LRESULT {
    LRESULT(v)
}
