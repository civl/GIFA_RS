//! 区域（region）：chrome 窗只在这些矩形上"存在"，其余地方点击穿透到舞台。
//!
//! 用 `SetWindowRgn` 而不是 `WS_EX_TRANSPARENT`/`HTTRANSPARENT`：
//! 区域之外的像素**根本不属于这个窗口**，命中测试自然落到下面 —— 不依赖线程、
//! 不依赖 z 序，也不需要父窗代转鼠标消息。

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, HGDIOBJ, RGN_OR};

/// 整数矩形（物理像素，窗口客户区坐标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// 裁到 (0,0,w,h) 之内；完全在外的返回 None。
    pub fn clamp_inside(&self, w: i32, h: i32) -> Option<RectI> {
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x + self.w).min(w);
        let y1 = (self.y + self.h).min(h);
        if x1 <= x0 || y1 <= y0 {
            None
        } else {
            Some(RectI::new(x0, y0, x1 - x0, y1 - y0))
        }
    }
}

/// 一组矩形。区域按"并集"处理：多块不连续的地方各自独立。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegionSpec {
    pub rects: Vec<RectI>,
}

impl RegionSpec {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一块矩形（builder 风格）
    pub fn with_rect(mut self, r: RectI) -> Self {
        self.rects.push(r);
        self
    }

    pub fn with_opt(mut self, r: Option<RectI>) -> Self {
        if let Some(r) = r {
            self.rects.push(r);
        }
        self
    }

    /// 去掉空矩形，并把超出窗口的部分裁掉。全空时返回 `None`（表示"整窗都不存在"）。
    pub fn normalized(&self, w: i32, h: i32) -> Option<Vec<RectI>> {
        let mut out: Vec<RectI> = self
            .rects
            .iter()
            .filter_map(|r| r.clamp_inside(w, h))
            .collect();
        out.retain(|r| !r.is_empty());
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        self.rects.iter().any(|r| r.contains(x, y))
    }

    /// 应用到窗口。系统接管 region 的所有权，这里不删除它。
    ///
    /// 注意：全部矩形都被裁没了的时候会退化成"整窗一块"，而不是"没有区域" ——
    /// `SetWindowRgn(None)` 的语义是"窗口不再有区域"（=整窗可见），跟我们要的相反。
    pub fn apply(&self, hwnd: HWND, w: i32, h: i32) {
        let Some(rects) = self.normalized(w, h) else {
            return;
        };
        unsafe {
            let mut acc = CreateRectRgn(0, 0, 0, 0);
            for (i, r) in rects.iter().enumerate() {
                let piece = CreateRectRgn(r.x, r.y, r.x + r.w, r.y + r.h);
                if i == 0 {
                    let _ = DeleteObject(HGDIOBJ(acc.0));
                    acc = piece;
                } else {
                    let _ = CombineRgn(Some(acc), Some(acc), Some(piece), RGN_OR);
                    let _ = DeleteObject(HGDIOBJ(piece.0));
                }
            }
            let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, Some(acc), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_drops_offscreen_parts() {
        let r = RectI::new(-10, -10, 20, 20).clamp_inside(100, 100).unwrap();
        assert_eq!(r, RectI::new(0, 0, 10, 10));
        assert!(RectI::new(200, 200, 10, 10)
            .clamp_inside(100, 100)
            .is_none());
    }

    #[test]
    fn normalized_is_none_when_everything_clipped() {
        let spec = RegionSpec::new().with_rect(RectI::new(500, 500, 10, 10));
        assert_eq!(spec.normalized(100, 100), None);
    }

    #[test]
    fn hit_uses_half_open_bounds() {
        let spec = RegionSpec::new().with_rect(RectI::new(0, 0, 10, 10));
        assert!(spec.hit(0, 0));
        assert!(spec.hit(9, 9));
        assert!(!spec.hit(10, 9));
    }
}
