//! 屏幕截取（Windows GDI BitBlt）。
//!
//! 为什么先用 GDI 而不是 DXGI Desktop Duplication：
//! 骨架阶段要的是"能拿到一帧"，DXGI 的 COM 生命周期、桌面复制会话重建、
//! 全屏独占切换都要额外处理，写进去会把链路验证和 COM 调试混在一起。
//! GDI 走的是 DWM 合成后的桌面，窗口化/无边框游戏都拿得到；
//! 独占全屏会返回黑帧 —— 那本来就要求用户改窗口化（见 `docs/IMMERSION.md`）。
//!
//! 二期换 DXGI 时只要实现同样的 `Frame`，上层不用动。

use crate::frame::Frame;

/// 截取屏幕的一块矩形（物理像素）。失败返回原因而不是 panic。
#[cfg(windows)]
pub fn capture_screen(x: i32, y: i32, w: i32, h: i32) -> Result<Frame, String> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };

    if w <= 0 || h <= 0 {
        return Err(format!("截取区域非法：{w}×{h}"));
    }

    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("GetDC 失败".into());
        }
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            let _ = ReleaseDC(None, screen);
            return Err("CreateCompatibleDC 失败".into());
        }

        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // 负高度 = 自顶向下，省一次翻转
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = match CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(mem);
                let _ = ReleaseDC(None, screen);
                return Err(format!("CreateDIBSection 失败: {e}"));
            }
        };
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(None, screen);
            return Err("DIB 位指针为空".into());
        }

        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let blt = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY);

        // ⚠️ 像素必须在 DeleteObject 之前读完：DIB 的 bits 归这块位图所有，
        // 先删再读就是 use-after-free（第一版这么写直接 ACCESS_VIOLATION）。
        let n = (w as usize) * (h as usize);
        let mut data = Vec::with_capacity(n * 4);
        if blt.is_ok() {
            let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
            for i in 0..n {
                let o = i * 4;
                data.push(src[o + 2]); // R
                data.push(src[o + 1]); // G
                data.push(src[o]); // B
                data.push(255);
            }
        }

        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);

        if blt.is_err() {
            return Err(format!("BitBlt 失败: {:?}", blt.err()));
        }
        Ok(Frame::new(w, h, data))
    }
}

#[cfg(not(windows))]
pub fn capture_screen(_x: i32, _y: i32, _w: i32, _h: i32) -> Result<Frame, String> {
    Err("屏幕截取目前只实现了 Windows".into())
}

/// 主显示器尺寸（物理像素）
#[cfg(windows)]
pub fn screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

#[cfg(not(windows))]
pub fn screen_size() -> (i32, i32) {
    (0, 0)
}
