//! Win32 小工具：注册窗口类、创建主窗/子窗、区域（region）与客户区换算。
//!
//! 刻意**不使用 tao**：GIFA 的外壳需要自己接管 WndProc（命中测试、置顶巡检、
//! 区域、分层属性），而框架把 WndProc 藏起来后每件事都要靠 subclass 绕。
//! wry 只要求一个 `HasWindowHandle`，所以直接喂原始 HWND 就行。

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject,
    EndPaint, FillRect, SetWindowRgn, HBRUSH, HGDIOBJ, PAINTSTRUCT, RGN_OR,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// 子窗背景（深色），与 chrome/stage 页面底色接近，避免首帧闪白
const CHILD_BG: u32 = 0x0020_2018;

pub const CLASS_MAIN: PCWSTR = w!("GifaSpikeMain");
pub const CLASS_CHILD: PCWSTR = w!("GifaSpikeChild");
pub const CLASS_TOP: PCWSTR = w!("GifaSpikeTop");

/// 给 wry 用的 HWND 包装：wry 只要求 `HasWindowHandle`。
#[derive(Clone, Copy)]
pub struct HwndHost(pub isize);

impl raw_window_handle::HasWindowHandle for HwndHost {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let nz = std::num::NonZeroIsize::new(self.0)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        let handle = raw_window_handle::Win32WindowHandle::new(nz);
        Ok(unsafe {
            raw_window_handle::WindowHandle::borrow_raw(raw_window_handle::RawWindowHandle::Win32(
                handle,
            ))
        })
    }
}

/// UTF-16（带结尾 0）。调用方负责保持存活。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn hinstance() -> HINSTANCE {
    // GetModuleHandleW 返回 HMODULE，与 HINSTANCE 同源同型，这里显式转换
    let hm = unsafe { GetModuleHandleW(None).unwrap() };
    HINSTANCE(hm.0)
}

fn register(name: PCWSTR, wndproc: WNDPROC) {
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: wndproc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance(),
        hIcon: HICON::default(),
        hCursor: HCURSOR::default(),
        hbrBackground: HBRUSH::default(), // 自己画，避免闪白
        lpszMenuName: PCWSTR::null(),
        lpszClassName: name,
    };
    unsafe {
        if RegisterClassW(&wc) == 0 {
            // 1410 = ERROR_CLASS_ALREADY_EXISTS（同进程重复注册），忽略
            if GetLastError().0 != 1410 {
                panic!("RegisterClassW 失败, GetLastError={}", GetLastError().0);
            }
        }
    }
}

pub fn register_classes() {
    register(CLASS_MAIN, Some(main_wndproc));
    register(CLASS_CHILD, Some(child_wndproc));
    register(CLASS_TOP, Some(top_wndproc));
}

/// 第二种 chrome 形态：独立顶层窗（WS_EX_TOPMOST + 工具窗 + 不激活）。
///
/// 关键一步是 `DwmExtendFrameIntoClientArea(-1,-1,-1,-1)`（俗称"玻璃板"）：
/// 它让窗口自身不再画不透明背景，DWM 就会把窗口里**没有画东西的地方**按逐像素
/// alpha 合成 —— 于是 WebView2 的透明区域能透出**下面的另一个窗口**（舞台窗）。
/// 子窗口拿不到这个效果：子窗的透明只能透出自己所属窗口的表面。
pub fn create_top_window(x: i32, y: i32, w: i32, h: i32) -> HWND {
    let title = wide("GIFA chrome (top-level)");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_TOP,
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinstance()),
            None,
        )
        .expect("CreateWindowExW(chrome 顶层窗) 失败")
    };
    let margins = windows::Win32::UI::Controls::MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        let _ = windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
    hwnd
}

extern "system" fn top_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let _hdc = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// 创建无边框主窗（WS_POPUP），尺寸为物理像素。
pub fn create_main_window(w: i32, h: i32) -> HWND {
    let title = wide("GIFA M1 spike");
    // 注意：**不要**加 WS_THICKFRAME。带 thick frame 的窗口有 ~8px 非客户区，
    // 子窗与 WebView2 会整体内缩，窗口边缘会漏出后面的其他窗口。
    // 真外壳要可缩放，就得在 WM_NCCALCSIZE 里把客户区顶回整窗（M1 的任务）。
    let style = WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN;
    unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW,
            CLASS_MAIN,
            PCWSTR(title.as_ptr()),
            style,
            60,
            60,
            w,
            h,
            None,
            None,
            Some(hinstance()),
            None,
        )
        .expect("CreateWindowExW(主窗) 失败")
    }
}

/// 子窗：stage（舞台）与 chrome（我们的界面层）都是它的实例。
pub fn create_child(parent: HWND, tag: &str) -> HWND {
    let name = wide(tag);
    unsafe {
        CreateWindowExW(
            WS_EX_NOPARENTNOTIFY,
            CLASS_CHILD,
            PCWSTR(name.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            10,
            10,
            Some(parent),
            None,
            Some(hinstance()),
            None,
        )
        .expect("CreateWindowExW(子窗) 失败")
    }
}

pub fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    (rc.right - rc.left, rc.bottom - rc.top)
}

pub fn client_to_screen(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut pt = POINT { x, y };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
    }
    (pt.x, pt.y)
}

/// 给 chrome 子窗装一个「成型」区域：只保留顶部条 / 底部条 / 悬浮 pill。
/// 区域之外的像素不存在，于是鼠标直接落到舞台子窗上 —— 不需要 HTTRANSPARENT，
/// 也不需要父窗帮忙做命中测试转发。
pub fn apply_chrome_region(chrome: HWND, w: i32, h: i32, pill: (i32, i32, i32, i32)) {
    let (px, py, pw, ph) = pill;
    unsafe {
        let bar = CreateRectRgn(0, 0, w, super::BAR_H);
        let foot = CreateRectRgn(0, h - super::FOOT_H, w, h);
        let pillr = CreateRectRgn(px, py, px + pw, py + ph);
        let all = CreateRectRgn(0, 0, 1, 1);
        let _ = CombineRgn(Some(all), Some(bar), Some(foot), RGN_OR);
        let _ = CombineRgn(Some(all), Some(all), Some(pillr), RGN_OR);
        // 系统接管 all 的所有权，之后不能删它
        let _ = SetWindowRgn(chrome, Some(all), true);
        for r in [bar, foot, pillr] {
            let _ = DeleteObject(HGDIOBJ(r.0));
        }
    }
}

extern "system" fn child_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let brush = CreateSolidBrush(COLORREF(CHILD_BG));
                FillRect(hdc, &rc, brush);
                let _ = DeleteObject(HGDIOBJ(brush.0));
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

extern "system" fn main_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { crate::on_main_message(hwnd, msg, wp, lp) }
}
