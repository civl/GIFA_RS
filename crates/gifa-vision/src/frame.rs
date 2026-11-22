//! 帧源抽象：视觉管线只依赖这个 trait，将来从"页面 canvas"换到"mpv 帧回调"
//! 或者"原生解码"都不用改算法。

/// 归一化 ROI（相对**视频像素**；`r` 按宽度归一，与原版一致）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roi {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
}

impl Roi {
    pub fn to_pixels(&self, video_w: i32, video_h: i32) -> (f64, f64, f64) {
        (
            self.cx * video_w as f64,
            self.cy * video_h as f64,
            self.r * video_w as f64,
        )
    }

    pub fn is_valid(&self) -> bool {
        self.cx > 0.0
            && self.cx < 1.0
            && self.cy > 0.0
            && self.cy < 1.0
            && (0.01..=0.5).contains(&self.r)
    }
}

/// 一帧 RGBA8。**不做缩放**：原版强调过，双线性插值会把箭头描边和地形混在一起。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    pub width: i32,
    pub height: i32,
    /// RGBA，长度 = width * height * 4
    pub data: Vec<u8>,
}

impl Frame {
    pub fn new(width: i32, height: i32, data: Vec<u8>) -> Self {
        debug_assert_eq!(data.len(), (width.max(0) * height.max(0) * 4) as usize);
        Self {
            width,
            height,
            data,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0 || self.data.is_empty()
    }

    pub fn pixel(&self, x: i32, y: i32) -> Option<(u8, u8, u8)> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 4) as usize;
        Some((self.data[i], self.data[i + 1], self.data[i + 2]))
    }
}

/// 帧源：由 shell 侧实现（页面 canvas / 未来的原生解码）
pub trait FrameSource {
    /// 视频原始尺寸
    fn video_size(&self) -> (i32, i32);
    /// 取一帧（可能因为视频暂停/换源而暂时不可用）
    fn next_frame(&mut self) -> Option<Frame>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roi_pixels_and_validation() {
        let r = Roi {
            cx: 0.147,
            cy: 0.246,
            r: 0.144,
        };
        assert!(r.is_valid());
        let (x, y, rad) = r.to_pixels(1920, 1080);
        assert!((x - 0.147 * 1920.0).abs() < 1e-9);
        assert!((y - 0.246 * 1080.0).abs() < 1e-9);
        assert!((rad - 0.144 * 1920.0).abs() < 1e-9);

        assert!(!Roi {
            cx: 0.0,
            cy: 0.5,
            r: 0.1
        }
        .is_valid());
        assert!(!Roi {
            cx: 0.5,
            cy: 0.5,
            r: 0.9
        }
        .is_valid());
    }
}
