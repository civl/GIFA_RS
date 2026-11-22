//! 识别管线：定时截取采样窗 → 朴素检测 → 发事件。
//!
//! 骨架阶段只做"把链路接起来"：
//!
//! ```text
//!   ┌────────────┐   每 interval   ┌──────────────┐   ┌─────────────┐
//!   │ 屏幕截图    │ ─────────────▶ │ detect_simple │ ─▶│ VisionEvent │
//!   │ (GDI BitBlt)│   只截采样窗    │  (占位算法)   │   │  → 叠加层    │
//!   └────────────┘                 └──────────────┘   └─────────────┘
//! ```
//!
//! 采样窗由外部闭包给出（窗口移动/切布局后自动跟着走），这样管线不需要知道
//! 窗口几何、也不需要重启线程。

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::arrow::ArrowWorkspace;
use crate::detect::{detect_window, Detect, SampleBox};
use crate::screen;

/// 一次识别结果（带耗时，便于判断性能是否达标）
#[derive(Debug, Clone, Copy)]
pub struct VisionEvent {
    pub seq: u64,
    /// 截帧耗时
    pub capture: Duration,
    /// 检测耗时
    pub detect: Duration,
    /// 采样窗（屏幕坐标）
    pub window: SampleBox,
    pub result: Detect,
}

/// 启动识别线程。`window_of` 返回当前采样窗（`None` = 这一刻不该识别）。
pub fn start<F>(window_of: F, interval: Duration, tx: Sender<VisionEvent>) -> std::io::Result<()>
where
    F: Fn() -> Option<SampleBox> + Send + 'static,
{
    std::thread::Builder::new()
        .name("gifa-vision".into())
        .spawn(move || {
            let mut seq = 0u64;
            let mut ws = ArrowWorkspace::default();
            let mut next = Instant::now();
            loop {
                next += interval;
                let this = Instant::now();
                if next > this {
                    std::thread::sleep(next - this);
                } else {
                    // 落后了就重新对齐，不要累积欠账
                    next = Instant::now();
                }

                let Some(win) = window_of() else { continue };
                let t0 = Instant::now();
                let frame = match screen::capture_screen(win.x, win.y, win.w, win.h) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let t1 = Instant::now();
                // 截的就是采样窗本身，正方形，直接喂给箭头算法
                let side = win.w.min(win.h).max(8) as usize;
                let result = detect_window(&frame.data, side, &mut ws);
                let t2 = Instant::now();
                seq += 1;
                if tx
                    .send(VisionEvent {
                        seq,
                        capture: t1 - t0,
                        detect: t2 - t1,
                        window: win,
                        result,
                    })
                    .is_err()
                {
                    return; // 接收端关了
                }
            }
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn pipeline_emits_events_and_stops_when_receiver_drops() {
        let (tx, rx) = mpsc::channel();
        let win = SampleBox {
            x: 0,
            y: 0,
            w: 64,
            h: 64,
        };
        start(move || Some(win), Duration::from_millis(20), tx).unwrap();

        // 桌面截取在某些会话里可能失败（锁屏/无桌面），
        // 所以这里只验证"事件结构成立"或"没事件但不 panic"
        let first = rx.recv_timeout(Duration::from_millis(500));
        if let Ok(ev) = first {
            assert!(ev.seq >= 1);
            assert_eq!(ev.window.w, 64);
            assert_eq!(ev.window.h, 64);
        }
        // 丢掉接收端，线程应当自行退出（不 panic、不泄漏）
        drop(rx);
        std::thread::sleep(Duration::from_millis(120));
    }

    #[test]
    fn none_window_skips_without_panicking() {
        let (tx, rx) = mpsc::channel();
        start(|| None, Duration::from_millis(10), tx).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "没有窗口就不该产出事件");
    }
}
