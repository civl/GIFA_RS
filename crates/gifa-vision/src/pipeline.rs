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
use crate::cone::ConeWorkspace;
use crate::detect::{detect_window_prior, Detect, SampleBox};
use crate::prior::ShapePrior;
use crate::screen;

/// 一次识别结果（带耗时，便于判断性能是否达标）
#[derive(Debug, Clone, Copy)]
pub struct VisionEvent {
    pub seq: u64,
    /// 采集时刻（毫秒，用于 α-β 滤波的 dt）
    pub at_ms: f64,
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
    start_with_dump(window_of, interval, tx, None)
}

/// 同上，但可以把前 `dump_n` 帧的采样窗原样存到 `dump_dir`（离线看 ROI 是否对）。
pub fn start_with_dump<F>(
    window_of: F,
    interval: Duration,
    tx: Sender<VisionEvent>,
    dump: Option<(std::path::PathBuf, u32)>,
) -> std::io::Result<()>
where
    F: Fn() -> Option<SampleBox> + Send + 'static,
{
    let mut dump_left = dump.as_ref().map(|d| d.1).unwrap_or(0);
    let dump_dir = dump.map(|d| d.0);
    std::thread::Builder::new()
        .name("gifa-vision".into())
        .spawn(move || {
            let mut seq = 0u64;
            let mut ws = ArrowWorkspace::default();
            let mut cws = ConeWorkspace::default();
            /* 形状先验：运行中自举。
             * 攒够 MIN_FRAMES 帧（v1 与 v3 一致可信的帧）之前完全不影响行为，
             * 攒够之后 v3 与模板互证，分歧的帧不上报（由滤波层保持上一帧）。
             * 见 detect_window_prior 的注释与 docs/VISION.md。 */
            let mut prior = ShapePrior::new();
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
                let result = detect_window_prior(&frame.data, side, &mut ws, &mut cws, &mut prior);
                // 只 dump「画面里有东西」的帧：没有小地图的转场/作者卡存下来没意义
                // GIFA_DUMP_ALL=1 时每帧都存（做帧间位移/时序实验要连续序列）
                let dump_all = std::env::var_os("GIFA_DUMP_ALL").is_some();
                let interesting = dump_all
                    || result.arrow.is_some()
                    || result.view.is_some()
                    || result.cyan_frac > 0.0005;
                if dump_left > 0 && interesting {
                    dump_left -= 1;
                    if let Some(dir) = &dump_dir {
                        let _ = std::fs::create_dir_all(dir);
                        let name = format!("f{:03}-{}", seq + 1, side);
                        let _ = std::fs::write(dir.join(format!("{name}.bin")), &frame.data);
                        let _ = std::fs::write(
                            dir.join(format!("{name}.json")),
                            format!(
                                "{{\"w\":{},\"h\":{},\"cyan\":{:.5}}}",
                                frame.width, frame.height, result.cyan_frac
                            ),
                        );
                        eprintln!("[vision] dump {name} cyan={:.5}", result.cyan_frac);
                    }
                }
                let t2 = Instant::now();
                seq += 1;
                let at_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as f64)
                    .unwrap_or(0.0);
                if tx
                    .send(VisionEvent {
                        seq,
                        at_ms,
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
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "没有窗口就不该产出事件"
        );
    }
}
