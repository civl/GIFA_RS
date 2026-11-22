//! M1 前置 spike：一个父窗 + 两块子 WebView2（舞台 / chrome 层），
//! 自动判定三件事：
//!
//! 1. chrome 层用**窗口区域（SetWindowRgn）成型**后，顶部/底部条正常渲染，
//!    其余区域不存在 —— 不需要 HTTRANSPARENT，也不需要父窗转发命中测试。
//! 2. 区域之外的点，命中测试落到**舞台子窗**（即点击会进 B 站页面）；
//!    悬浮 pill（URL 条）区域落在 chrome 上，并且是**半透明叠加**在舞台之上的。
//! 3. 父窗 `SetLayeredWindowAttributes(LWA_ALPHA)` 的透明度是否同时作用于
//!    两块子 WebView2（WebView2 走 DirectComposition，不保证跟随）。
//!
//! 用法：
//!   cargo run -p dual-webview-spike           # 自检 → 打印报告 → 自动退出
//!   cargo run -p dual-webview-spike -- --hold  # 自检后保持窗口，便于人工看/截图

mod probe;
mod win;

use std::cell::RefCell;
use std::time::Duration;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use wry::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use wry::{Rect, WebView, WebViewBuilder};

use probe::Hit;

pub const BAR_H: i32 = 30;
pub const FOOT_H: i32 = 34;
const WIN_W: i32 = 1024;
const WIN_H: i32 = 640;
const PILL_W: i32 = 520;
const PILL_H: i32 = 38;
const PILL_GAP: i32 = 14;

const MSG_PHASE_A: u32 = WM_APP + 1;
const MSG_PHASE_B: u32 = WM_APP + 2;
const MSG_PHASE_C: u32 = WM_APP + 3;

const CHROME_HTML: &str = include_str!("../web/chrome.html");
const STAGE_HTML: &str = include_str!("../web/stage.html");

/// chrome 层顶部条颜色，与 chrome.html 里的 `#1b1d24` 保持一致
const BAR_RGB: probe::Rgb = (0x1b, 0x1d, 0x24);
/// 舞台页里那个大红方块
const STAGE_RGB: probe::Rgb = (255, 0, 0);

#[derive(Clone, Copy, Default)]
struct Layout {
    w: i32,
    #[allow(dead_code)]
    h: i32,
    pill_x: i32,
    pill_y: i32,
}

struct App {
    hwnd: HWND,
    stage: HWND,
    chrome: HWND,
    stage_wv: WebView,
    chrome_wv: WebView,
    layout: Layout,
    hold: bool,
    /// chrome 层是独立顶层窗（true）还是父窗的子窗（false）
    toplevel: bool,
    phase_a: Option<PhaseA>,
}

#[derive(Clone, Copy)]
struct PhaseA {
    s_title: Option<probe::Rgb>,
    s_stage: Option<probe::Rgb>,
    s_pill: Option<probe::Rgb>,
    hit_title: Hit,
    hit_stage: Hit,
    hit_pill: Hit,
    /// 只给父窗设 alpha 后的采样
    s_stage_parent_alpha: Option<probe::Rgb>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    win::register_classes();
    let hold = std::env::args().any(|a| a == "--hold");
    let toplevel = std::env::args().any(|a| a == "--toplevel");

    let hwnd = win::create_main_window(WIN_W, WIN_H);
    let stage = win::create_child(hwnd, "stage");
    // chrome 的两种形态：子窗（默认）或独立顶层玻璃窗（--toplevel）
    let chrome = if toplevel {
        win::create_top_window(60, 60, WIN_W, WIN_H)
    } else {
        win::create_child(hwnd, "chrome")
    };

    let stage_wv = build_webview(stage, STAGE_HTML, "stage");
    let chrome_wv = build_webview(chrome, CHROME_HTML, "chrome");

    // 父窗加分层属性：后面用它验证 alpha 是否会传递到子 WebView2
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }

    APP.with(|slot| {
        *slot.borrow_mut() = Some(App {
            hwnd,
            stage,
            chrome,
            stage_wv,
            chrome_wv,
            layout: Layout::default(),
            hold,
            toplevel,
            phase_a: None,
        });
    });

    layout();

    println!(
        "[spike] 模式：{}（chrome 层 = {}）",
        if toplevel { "toplevel" } else { "child" },
        if toplevel {
            "独立顶层玻璃窗 + SetWindowRgn"
        } else {
            "父窗子窗 + SetWindowRgn"
        }
    );
    println!("[spike] 主窗 {WIN_W}x{WIN_H}（无边框 + WS_EX_LAYERED, alpha=255）");
    println!(
        "[spike] bar={BAR_H}px  stage=[30,{}]  foot={FOOT_H}px",
        WIN_H - FOOT_H
    );
    println!("[spike] 3.5s 后开始自检…");
    post_later(hwnd, MSG_PHASE_A, 3500);

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }
    // WebView 的析构要走 COM，进程直接退更干净（spike 专属做法）
    std::process::exit(0);
}

fn build_webview(parent: HWND, html: &str, tag: &'static str) -> WebView {
    let (w, h) = win::client_size(parent);
    WebViewBuilder::new()
        .with_html(html)
        .with_transparent(true)
        .with_bounds(rect_for(w, h))
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            println!("[ipc:{tag}] {}", req.body());
        })
        .with_devtools(false)
        .build_as_child(&win::HwndHost(parent.0 as isize))
        .expect("创建 WebView2 失败（检查 WebView2 Runtime）")
}

fn rect_for(w: i32, h: i32) -> Rect {
    Rect {
        position: Position::Physical(PhysicalPosition::new(0, 0)),
        size: Size::Physical(PhysicalSize::new(w.max(1) as u32, h.max(1) as u32)),
    }
}

fn post_later(hwnd: HWND, msg: u32, delay_ms: u64) {
    let raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        unsafe {
            let _ = PostMessageW(Some(HWND(raw as _)), msg, WPARAM(0), LPARAM(0));
        }
    });
}

fn set_alpha(hwnd: HWND, a: u8) {
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), a, LWA_ALPHA);
    }
}

fn layout() {
    APP.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(g) => g,
            Err(_) => return, // 重入（例如区域变化触发的 WM_WINDOWPOSCHANGED）直接跳过
        };
        let Some(app) = guard.as_mut() else { return };

        let (w, h) = win::client_size(app.hwnd);
        if w <= 0 || h <= 0 {
            return;
        }
        let pill_w = PILL_W.min(w - 40).max(80);
        let pill_x = (w - pill_w) / 2;
        let pill_y = (h - FOOT_H - PILL_H - PILL_GAP).max(BAR_H);

        unsafe {
            let _ = SetWindowPos(
                app.stage,
                None,
                0,
                BAR_H,
                w,
                h - BAR_H - FOOT_H,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if app.toplevel {
                // 顶层 chrome 窗：跟着父窗客户区的屏幕位置走。
                // 每次布局都重新 HWND_TOPMOST，保证它压在父窗之上（两者都在 topmost 带里）。
                let (sx, sy) = win::client_to_screen(app.hwnd, 0, 0);
                let _ = SetWindowPos(app.chrome, Some(HWND_TOPMOST), sx, sy, w, h, SWP_NOACTIVATE);
            } else {
                let _ = SetWindowPos(app.chrome, Some(HWND_TOP), 0, 0, w, h, SWP_NOACTIVATE);
            }
        }

        win::apply_chrome_region(app.chrome, w, h, (pill_x, pill_y, pill_w, PILL_H));

        // WebView2 不会自动跟随子窗尺寸，必须显式同步（这是原生外壳的日常功课）
        let _ = app.stage_wv.set_bounds(rect_for(w, h - BAR_H - FOOT_H));
        let _ = app.chrome_wv.set_bounds(rect_for(w, h));

        app.layout = Layout {
            w,
            h,
            pill_x,
            pill_y,
        };
    });
}

pub(crate) unsafe fn on_main_message(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            layout();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            layout();
            LRESULT(0)
        }
        MSG_PHASE_A => {
            phase_a();
            LRESULT(0)
        }
        MSG_PHASE_B => {
            phase_b();
            LRESULT(0)
        }
        MSG_PHASE_C => {
            phase_c();
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn phase_a() {
    APP.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(app) = guard.as_mut() else { return };
        let l = app.layout;

        let p_title = win::client_to_screen(app.hwnd, 200, BAR_H / 2);
        let p_stage = win::client_to_screen(app.hwnd, l.w / 2, BAR_H + 160);
        let p_pill =
            win::client_to_screen(app.hwnd, l.pill_x + l.pill_w() - 34, l.pill_y + PILL_H / 2);

        app.phase_a = Some(PhaseA {
            s_title: probe::sample(p_title.0, p_title.1),
            s_stage: probe::sample(p_stage.0, p_stage.1),
            s_pill: probe::sample(p_pill.0, p_pill.1),
            hit_title: probe::classify(p_title.0, p_title.1, app.hwnd, app.stage, app.chrome),
            hit_stage: probe::classify(p_stage.0, p_stage.1, app.hwnd, app.stage, app.chrome),
            hit_pill: probe::classify(p_pill.0, p_pill.1, app.hwnd, app.stage, app.chrome),
            s_stage_parent_alpha: None,
        });

        // 第一步：只给父窗设 alpha
        set_alpha(app.hwnd, 128);
        post_later(app.hwnd, MSG_PHASE_B, 900);
    });
}

/// 只设父窗 alpha 之后采样；没变化就退一步给舞台子窗也设一遍。
fn phase_b() {
    APP.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(app) = guard.as_mut() else { return };
        let l = app.layout;
        let p = win::client_to_screen(app.hwnd, l.w / 2, BAR_H + 160);
        let after = probe::sample(p.0, p.1);

        let Some(mut a) = app.phase_a else { return };
        a.s_stage_parent_alpha = after;
        app.phase_a = Some(a);

        let propagated = match (a.s_stage, after) {
            (Some(b), Some(x)) => !probe::close(b, x, 6),
            _ => false,
        };
        if propagated {
            set_alpha(app.hwnd, 255);
            finish(
                app,
                &a,
                true,
                format!(
                    "alpha=255 {:?} → alpha=128 {:?}（只需给**父窗**设 LWA_ALPHA）",
                    a.s_stage, after
                ),
            );
            return;
        }

        // 退一步：给舞台子窗也加 WS_EX_LAYERED + LWA_ALPHA
        unsafe {
            let ex = GetWindowLongPtrW(app.stage, GWL_EXSTYLE);
            SetWindowLongPtrW(app.stage, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
            let _ = SetLayeredWindowAttributes(app.stage, COLORREF(0), 128, LWA_ALPHA);
            let _ = SetWindowPos(
                app.stage,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        post_later(app.hwnd, MSG_PHASE_C, 900);
    });
}

fn phase_c() {
    APP.with(|slot| {
        let mut guard = match slot.try_borrow_mut() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(app) = guard.as_mut() else { return };
        let l = app.layout;
        let p = win::client_to_screen(app.hwnd, l.w / 2, BAR_H + 160);
        let after = probe::sample(p.0, p.1);
        let Some(a) = app.phase_a else { return };

        let propagated = match (a.s_stage, after) {
            (Some(b), Some(x)) => !probe::close(b, x, 6),
            _ => false,
        };
        set_alpha(app.hwnd, 255);
        unsafe {
            let _ = SetLayeredWindowAttributes(app.stage, COLORREF(0), 255, LWA_ALPHA);
        }
        let detail = if propagated {
            format!(
                "父窗无效，**必须同时给舞台子窗设 LWA_ALPHA**：255 {:?} → 128 {:?}",
                a.s_stage, after
            )
        } else {
            format!(
                "父窗与子窗单独设置都无效（{:?} 两点一致）⇒ 透明度只能靠逐个 WebView2 自己实现",
                a.s_stage
            )
        };
        finish(app, &a, propagated, detail);
    });
}

fn finish(app: &mut App, a: &PhaseA, alpha_ok: bool, alpha_detail: String) {
    let mut report: Vec<(String, bool, String)> = Vec::new();

    let ok1 = a.s_title.is_some_and(|p| probe::close(p, BAR_RGB, 16));
    report.push((
        "chrome 层渲染（顶部条）".into(),
        ok1,
        fmt_rgb(a.s_title, BAR_RGB),
    ));

    let ok2 = a.s_stage.is_some_and(|p| probe::close(p, STAGE_RGB, 24));
    report.push((
        "舞台透出（红块原色）".into(),
        ok2,
        fmt_rgb(a.s_stage, STAGE_RGB),
    ));

    // 0.85*(16,16,16) + 0.15*(255,0,0) ≈ (52,14,14)
    let ok3 = a
        .s_pill
        .is_some_and(|(r, g, b)| (28..=95).contains(&(r as i32)) && g <= 55 && b <= 55);
    report.push((
        "透明叠加（pill 混色）".into(),
        ok3,
        format!(
            "采样 {:?}（期望 r≈52/g≈14/b≈14；纯白=背景不透明，纯黑=透明失败）",
            a.s_pill
        ),
    ));

    let ok4 = a.hit_title == Hit::Chrome && a.hit_stage == Hit::Stage && a.hit_pill == Hit::Chrome;
    report.push((
        "命中测试归属".into(),
        ok4,
        format!(
            "标题栏→{:?}（期望 Chrome） / 舞台→{:?}（期望 Stage） / pill→{:?}（期望 Chrome）",
            a.hit_title, a.hit_stage, a.hit_pill
        ),
    ));

    report.push(("窗口 alpha 传递".into(), alpha_ok, alpha_detail));

    let pass = report.iter().filter(|(_, ok, _)| *ok).count();
    println!();
    println!("================ GIFA M1 spike 自检 ================");
    println!(
        "模式：{} / chrome 层 = {}",
        if app.toplevel { "toplevel" } else { "child" },
        if app.toplevel {
            "独立顶层玻璃窗"
        } else {
            "父窗子窗"
        }
    );
    for (i, (name, ok, detail)) in report.iter().enumerate() {
        println!(
            "[{}] {:<22} {}  {}",
            i + 1,
            name,
            if *ok { "PASS" } else { "FAIL" },
            detail
        );
    }
    println!("---------------------------------------------------");
    println!("RESULT {}/{}", pass, report.len());
    println!("====================================================");
    println!();

    if !app.hold {
        let raw = app.hwnd.0 as isize;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1200));
            unsafe {
                let _ = PostMessageW(Some(HWND(raw as _)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        });
    }
}

impl Layout {
    fn pill_w(&self) -> i32 {
        // 与 layout() 里同一算法：pill_w = min(520, w-40)，这里只需右边界
        let w = if self.w > 0 { self.w } else { WIN_W };
        PILL_W.min(w - 40).max(80)
    }
}

fn fmt_rgb(got: Option<probe::Rgb>, want: probe::Rgb) -> String {
    match got {
        Some((r, g, b)) => format!("采样 ({r},{g},{b})，期望 ≈{:?}", want),
        None => "采样失败（GetPixel 返回 CLR_INVALID）".to_string(),
    }
}
