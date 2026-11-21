//! 临时探针：只装 `WH_MOUSE_LL`，打印每一次回调，用来判断
//! "我们的集成有问题" 还是 "这个环境收不到低级鼠标钩子"。
//!
//! cargo run -p gifa-input --example mouse_probe

use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

static N: AtomicU64 = AtomicU64::new(0);
static ALL: AtomicU64 = AtomicU64::new(0);

extern "system" fn proc_(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let all = ALL.fetch_add(1, Ordering::Relaxed);
        if all < 3 {
            println!("[probe] 首次回调 #{all} msg={:#06x}", wp.0 as u32);
        }
    }
    if code >= 0 && wp.0 as u32 != WM_MOUSEMOVE {
        let n = N.fetch_add(1, Ordering::Relaxed);
        let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        println!(
            "[probe] #{n} msg={:#06x} mouseData={:#010x}",
            wp.0 as u32, info.mouseData
        );
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

fn main() {
    unsafe {
        let module = GetModuleHandleW(None).ok().map(|m| windows::Win32::Foundation::HINSTANCE(m.0));
        match SetWindowsHookExW(WH_MOUSE_LL, Some(proc_), module, 0) {
            Ok(h) => println!("[probe] 钩子已安装 {:#x}", h.0 as isize),
            Err(e) => {
                println!("[probe] 安装失败 {e:?}");
                return;
            }
        }
        // 5 秒内持续移动光标，制造真实鼠标输入
        let t = std::time::Instant::now();
        let mut msg = MSG::default();
        while t.elapsed().as_secs_f64() < 6.0 {
            // 有消息就处理，没有就转一会儿
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
            // 用 SendInput 造真实注入输入（SetCursorPos 不经过输入队列，钩子看不到）
            inject_move(3, 2);
            if (ALL.load(Ordering::Relaxed) as usize) < 3 {
                inject_click();
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        println!(
            "[probe] 结论：回调总数={} 非移动事件={}",
            ALL.load(Ordering::Relaxed),
            N.load(Ordering::Relaxed)
        );
    }
}

fn inject_move(dx: i32, dy: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
    };
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

fn inject_click() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT,
    };
    for f in [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP] {
        let inp = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: f,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
        }
    }
}
