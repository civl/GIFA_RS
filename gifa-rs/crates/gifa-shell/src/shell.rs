//! 外壳装配：舞台窗（B 站 WebView2）+ 顶层 chrome 玻璃窗（我们的界面）+ 两者联动。
//!
//! 关键设计（见 `gifa-rs/spikes/dual-webview/FINDINGS.md` 与 task.md D3）：
//!
//! * 舞台窗是**唯一**持有视频的窗口，`WS_EX_LAYERED` + `LWA_ALPHA` 负责整体不透明度；
//! * chrome 是**独立的顶层窗**，靠 `DwmExtendFrameIntoClientArea(-1)` 拿到逐像素透明，
//!   才能让 URL 条 / 提示条半透明地压在视频上；
//! * chrome 窗用 `SetWindowRgn` 挖空，区域之外鼠标直接落到舞台（点击穿透）；
//! * chrome 窗的 **owner 是舞台窗** —— 于是 z 序、最小化、销毁全由系统保证，不用自己维护。

use std::cell::RefCell;

use serde::Deserialize;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::*;
use wry::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use wry::{Rect, WebView, WebViewBuilder};

use crate::layout::{self, Bounds, ChromeLayout, WorkArea};
use crate::region::RectI;
use crate::win32::{self, wide, HwndHost};

/// 设 `GIFA_DEBUG=1` 时打印外壳内部步骤（默认安静，避免干扰正常输出）
pub fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GIFA_DEBUG").is_some())
}

#[macro_export]
macro_rules! shell_log {
    ($($arg:tt)*) => {
        if $crate::shell::debug_on() {
            eprintln!("[shell] {}", format!($($arg)*));
        }
    };
}

/// WebView2 运行时基线（Chromium 主版本）。
///
/// 依据：我们依赖 `with_user_agent`（≥86）、透明背景（≥90）、
/// 以及二期要用的 SharedBuffer（≥110）。Win11 自带，Win10 需装 Evergreen。
pub const WEBVIEW2_MIN_MAJOR: u32 = 110;

/// 圆盘页面（透明 HUD，只画扇区与中心提示）
const DISC_HTML: &str = include_str!("../assets/disc.html");
/// 方位标记叠加层
const OVERLAY_HTML: &str = include_str!("../assets/overlay.html");

pub const CLASS_FLOAT: PCWSTR = w!("GifaFloat");
pub const CLASS_CHROME: PCWSTR = w!("GifaChrome");
pub const CLASS_POPUP: PCWSTR = w!("GifaPopup");
pub const CLASS_DISC: PCWSTR = w!("GifaDisc");
pub const CLASS_OVERLAY: PCWSTR = w!("GifaOverlay");

/// 用于"从 chrome 页发起拖动/缩放"的自投递消息：
/// 必须先把借用放下，再进系统模态循环，否则循环里的 WM_MOVE 拿不到状态。
const WM_APP_CMD: u32 = WM_APP + 10;
/// 外部定时器把"该自检/刷新了"投递回 UI 线程（thread_local 状态只能在创建线程访问）
const WM_APP_TICK: u32 = WM_APP + 11;
/// 页面改了布局（URL 条展开/收起）后，等借用放下再重算区域与舞台矩形
const WM_APP_RELAYOUT: u32 = WM_APP + 12;
/// 后台线程要求执行 JS：队列 + 消息，回到 UI 线程再真正执行
const WM_APP_EVAL: u32 = WM_APP + 13;

/// 待执行的 JS 队列（允许从任意线程 push）
static EVAL_QUEUE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 边框可缩放厚度（逻辑像素，会按 DPI 缩放）。
/// 注意：舞台 WebView2 会铺满客户区，所以这条边框只在子窗尚未覆盖时生效；
/// 真正的缩放入口是 chrome 页右下角的 grip（HTML 里发起，见 `ChromeMsg::Resize`）。
const RESIZE_BORDER: i32 = 6;

/// chrome 页面的 IPC 消息（JSON，字段单位是物理像素）。
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ChromeMsg {
    /// 上报界面尺寸：标题栏/底栏高度、URL 条、浮动控件矩形
    Layout {
        titlebar: i32,
        footer: i32,
        #[serde(default)]
        urlbar: Option<RectDe>,
        /// 标题栏里可拖动的那一段
        #[serde(default)]
        drag: Option<RectDe>,
        #[serde(default)]
        overlay: Vec<RectDe>,
    },
    /// 请求拖动窗口（标题栏按下去时发）
    Drag,
    /// 请求从右下角缩放
    Resize,
    /// 页面内改整体不透明度（chrome 窗不能设 LWA，会破坏逐像素透明）
    Opacity { v: f64 },
    /// 页面点"关闭"：关掉舞台窗（owned 的 chrome 会被系统一并销毁）
    Close,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RectDe {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectDe {
    fn to_rect(self) -> RectI {
        RectI::new(
            self.x.round() as i32,
            self.y.round() as i32,
            self.w.round() as i32,
            self.h.round() as i32,
        )
    }
}

pub struct ShellConfig {
    /// chrome 页的地址（`file:///.../chrome.html`）：给了就用它，能加载同级 style.css / app.js
    pub chrome_url: Option<String>,
    pub chrome_html: String,
    pub stage_html: String,
    pub bounds: Bounds,
    /// 是否把初始 bounds 夹回工作区。
    ///
    /// 恢复用户上次摆好的位置时**要关掉**：用户可能故意让窗口底边压到任务栏下面
    /// （那里是游戏的底部 UI，没人在意），clamp 会把它往上推，于是每次启动都漂一点。
    pub clamp_bounds: bool,
    /// 0..=255
    pub opacity: u8,
    pub title: String,
    /// 舞台要加载的 URL；为空则用 `stage_html`（离线演示）
    pub stage_url: Option<String>,
    /// WebView2 用户数据目录（登录态、缓存）；None = 用默认
    pub user_data_dir: Option<std::path::PathBuf>,
    /// 播放器事件出口（Ready/State/Log/Fullscreen）
    pub player_tx: Option<std::sync::mpsc::Sender<gifa_player::PlayerMsg>>,
    /// chrome 页面发来的**原始**消息（应用层自定义指令走这里：load/toggle/clean…）
    pub chrome_tx: Option<std::sync::mpsc::Sender<String>>,
    /// 允许拖动/缩放（沉浸模式下会关掉）
    pub interactive: bool,
    /// 由 `post_tick` 在 UI 线程上回调（自检/定时刷新用）
    pub tick: Option<fn()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    Drag,
    Resize,
}

struct Shell {
    float: HWND,
    chrome: HWND,
    stage_wv: WebView,
    chrome_wv: WebView,
    layout: ChromeLayout,
    opacity: u8,
    interactive: bool,
    /// 沉浸模式（纯视频）：隐藏 chrome、锁定尺寸、强制穿透
    immersion: bool,
    /// 进入沉浸前的窗口矩形（退出时精确还原）
    immersion_bounds: Option<(i32, i32, i32, i32)>,
    /// 进入沉浸前的 chrome 布局（沉浸期间舞台要占满客户区）
    saved_layout: Option<ChromeLayout>,
    /// 用户设定的不透明度（0..255），鼠标避让只临时覆盖
    base_opacity: u8,
    hover_fade: bool,
    hovered: bool,
    hover_candidate: bool,
    hover_candidate_since: Option<std::time::Instant>,
    click_through: bool,
    pending: Pending,
    /// 页面发来的原始消息（调试/自检用）
    pub inbox: Vec<String>,
    /// 上次推给页面的 (窗口w, 窗口h, 舞台h, 区域块数)，用于节流且不漏状态变化
    last_push: (i32, i32, i32, i32),
    tick: Option<fn()>,
    /// 布局变了但还没重算
    relayout_pending: bool,
    /// WebView2 的 WebContext 必须活得比 WebView 长
    _web_context: wry::WebContext,
    player_tx: Option<std::sync::mpsc::Sender<gifa_player::PlayerMsg>>,
    chrome_tx: Option<std::sync::mpsc::Sender<String>>,
    /// 最近一次播放状态（供 UI 轮询）
    player: Option<gifa_player::PlayerState>,
    stage_url: String,
    /// 独立弹窗（扫码登录等）：共享同一个 WebContext，所以 Cookie 是同一份
    popup: Option<Popup>,
    /// 手柄八方向圆盘（透明 HUD，惰性创建）
    disc: Option<Disc>,
    /// 方位标记叠加层
    overlay: Option<Overlay>,
    /// 系统电源事件（WM_POWERBROADCAST 记录下来，应用层在 tick 里取走）
    power: Option<PowerEvent>,
    /// 单实例互斥体（活着就代表本进程是唯一实例）
    instance_mutex: Option<isize>,
}

/// 系统电源事件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// 挂起（睡眠/休眠）
    Suspend,
    /// 锁屏
    Lock,
    /// 解锁
    Unlock,
    /// 恢复
    Resume,
}

struct Overlay {
    hwnd: HWND,
    wv: WebView,
    radius: i32,
    /// 放置模式：可拖动、可点击
    placing: bool,
    /// 页面回报的"当前画出来的角度"（自检用）
    drawn: (Option<f64>, Option<f64>),
    /// 页面回报的"视角是否已过期"
    view_stale: bool,
    /// 页面回报的最后一帧是否真的画了绿弧（诊断）
    drew_arc: bool,
    /// 最近若干帧画出来的角度（补间过程会经过中间值，自检靠它判定）
    drawn_log: std::collections::VecDeque<(Option<f64>, Option<f64>)>,
}

struct Disc {
    hwnd: HWND,
    wv: WebView,
    /// 上次推给页面的 JSON，用来去重
    last: String,
}

struct Popup {
    hwnd: HWND,
    wv: WebView,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
}

/// 舞台窗句柄的全局副本：`post_tick`/`close` 可能从后台线程调用，
/// 那时 thread_local 里的 SHELL 是空的，只能靠这个拿到 HWND。
static FLOAT_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// 本机 WebView2 运行时版本（如 `157.0.4341.60`）。
pub fn webview2_version() -> Result<String, String> {
    wry::webview_version().map_err(|e| format!("读不到 WebView2 运行时版本：{e}"))
}

/// 版本字符串 → `(major, minor, build, patch)`
pub fn parse_version(v: &str) -> Option<(u32, u32, u32, u32)> {
    let mut it = v.trim().split('.').map(|p| p.parse::<u32>().ok());
    Some((
        it.next()??,
        it.next().flatten()?,
        it.next().flatten()?,
        it.next().flatten()?,
    ))
}

/// 是否满足基线
pub fn runtime_ok(v: &str) -> bool {
    parse_version(v).is_some_and(|(major, ..)| major >= WEBVIEW2_MIN_MAJOR)
}

/// 创建外壳（窗口 + 两个 WebView2 + 首次布局同步）。必须在 UI 线程调用。
pub fn create(cfg: ShellConfig) -> Result<(), String> {
    win32::init_process();
    win32::register_class(CLASS_FLOAT, Some(float_proc));
    win32::register_class(CLASS_CHROME, Some(chrome_proc));
    win32::register_class(CLASS_POPUP, Some(popup_proc));
    win32::register_class(CLASS_DISC, Some(disc_proc));
    win32::register_class(CLASS_OVERLAY, Some(overlay_proc));

    match webview2_version() {
        Ok(v) => crate::shell_log!(
            "WebView2 运行时 {v}（基线 ≥{WEBVIEW2_MIN_MAJOR}，{}）",
            if runtime_ok(&v) { "达标" } else { "偏低" }
        ),
        Err(e) => crate::shell_log!("{e}"),
    }
    crate::shell_log!("创建舞台窗…");
    let float = create_float_window(&cfg)?;
    crate::shell_log!("创建舞台 WebView2…");
    let mut web_context = match &cfg.user_data_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
            wry::WebContext::new(Some(dir.clone()))
        }
        None => wry::WebContext::new(None),
    };
    let stage_wv = build_stage(&float, &cfg, &mut web_context)?;
    crate::shell_log!("创建 chrome 窗…");
    let chrome = create_chrome_window(float)?;
    crate::shell_log!("创建 chrome WebView2…");
    let chrome_wv = build_chrome(&chrome, &cfg)?;
    // 16ms 拖动轮询：不依赖页面事件（WebView2 未激活时不派发 DOM 鼠标事件）
    let tid = unsafe { SetTimer(Some(float), DRAG_TIMER_ID, 16, None) };
    crate::shell_log!("拖动轮询定时器 id={tid}");
    crate::shell_log!("首次同步…");

    SHELL.with(|slot| {
        *slot.borrow_mut() = Some(Shell {
            float,
            chrome,
            stage_wv,
            chrome_wv,
            layout: ChromeLayout::default(),
            opacity: 255,
            interactive: cfg.interactive,
            immersion: false,
            immersion_bounds: None,
            saved_layout: None,
            base_opacity: cfg.opacity,
            hover_fade: true,
            hovered: false,
            hover_candidate: false,
            hover_candidate_since: None,
            click_through: false,
            pending: Pending::None,
            inbox: Vec::new(),
            last_push: (0, 0, 0, 0),
            tick: cfg.tick,
            relayout_pending: false,
            _web_context: web_context,
            player_tx: cfg.player_tx,
            chrome_tx: cfg.chrome_tx,
            player: None,
            stage_url: cfg.stage_url.clone().unwrap_or_default(),
            popup: None,
            disc: None,
            overlay: None,
            power: None,
            instance_mutex: None,
        });
    });

    FLOAT_HWND.store(float.0 as isize, std::sync::atomic::Ordering::SeqCst);

    set_opacity(cfg.opacity as f64 / 255.0);
    sync();
    Ok(())
}

fn create_float_window(cfg: &ShellConfig) -> Result<HWND, String> {
    let title = wide(&cfg.title);
    // WS_THICKFRAME：让系统提供缩放边框（配合 WM_NCCALCSIZE 隐藏它）
    let style = WS_POPUP | WS_CLIPCHILDREN | WS_THICKFRAME | WS_MINIMIZEBOX;
    // 先按默认工作区算一次位置，稍后 clamp 到窗口所在屏
    let b = cfg.bounds.size_clamped();
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            CLASS_FLOAT,
            PCWSTR(title.as_ptr()),
            style,
            b.x,
            b.y,
            b.w,
            b.h,
            None,
            None,
            Some(win32::hinstance()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW(float) 失败: {e}"))?;

        // 落到所在屏的工作区里（恢复用户位置时跳过，见 clamp_bounds 的注释）
        let work = win32::work_area(hwnd);
        let clamped = if cfg.clamp_bounds {
            layout::clamp_to_work(
                Bounds::new(b.x, b.y, b.w, b.h),
                WorkArea {
                    x: work.left,
                    y: work.top,
                    w: work.right - work.left,
                    h: work.bottom - work.top,
                },
            )
        } else {
            Bounds::new(b.x, b.y, b.w, b.h)
        };
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            clamped.x,
            clamped.y,
            clamped.w,
            clamped.h,
            SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

fn create_chrome_window(float: HWND) -> Result<HWND, String> {
    let title = wide("GIFA chrome");
    // owner = 舞台窗：z 序天然在它上面，随它最小化/销毁
    unsafe {
        let hwnd = CreateWindowExW(
            // 不能加 NOACTIVATE：WebView2 在宿主未被激活时不派发 DOM 鼠标事件，
            // 表现就是"第一次点击没反应，点一下别的按钮之后就正常了"。
            // 代价是点浮窗会把焦点从游戏拿走（点回游戏即可恢复）。
            WS_EX_TOOLWINDOW,
            CLASS_CHROME,
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            10,
            10,
            Some(float),
            None,
            Some(win32::hinstance()),
            None,
        )
        .map_err(|e| format!("CreateWindowExW(chrome) 失败: {e}"))?;

        // "玻璃板"：窗口本身不再画不透明背景，DWM 按逐像素 alpha 合成
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

fn rect_for(r: RectI) -> Rect {
    Rect {
        position: Position::Physical(PhysicalPosition::new(r.x, r.y)),
        size: Size::Physical(PhysicalSize::new(r.w.max(1) as u32, r.h.max(1) as u32)),
    }
}

fn build_stage(
    float: &HWND,
    cfg: &ShellConfig,
    ctx: &mut wry::WebContext,
) -> Result<WebView, String> {
    let mut builder = WebViewBuilder::new_with_web_context(ctx)
        // agent 在**每次导航**前注入：切 P、换片、整页跳转都由它跟着走
        .with_initialization_script(gifa_player::agent_js())
        .with_user_agent(gifa_player::UA)
        // 浮窗必须能无手势自动播放
        .with_autoplay(true)
        .with_bounds(rect_for(RectI::new(0, 0, 640, 360)))
        .with_ipc_handler(|req: wry::http::Request<String>| {
            on_stage_message(req.body());
        })
        // 页面要开新窗一律拒绝：GIFA 只有固定几个窗口，弹窗会破坏置顶/区域模型
        .with_new_window_req_handler(|_url, _features| wry::NewWindowResponse::Deny)
        .with_devtools(false);

    builder = match cfg.stage_url.as_deref() {
        Some(url) if !url.is_empty() => builder.with_url(url),
        _ => builder.with_html(&cfg.stage_html),
    };

    builder
        .build_as_child(&HwndHost(float.0 as isize))
        .map_err(|e| format!("创建舞台 WebView2 失败: {e}"))
}

/// 页面 agent 的消息入口：解析协议 → 存状态 → 转发给应用层。
fn on_stage_message(body: &str) {
    let msg: gifa_player::PlayerMsg = match serde_json::from_str(body) {
        Ok(m) => m,
        Err(e) => {
            crate::shell_log!("非法播放器消息 {body:?}: {e}");
            return;
        }
    };
    if let gifa_player::PlayerMsg::State(st) = &msg {
        with_shell(|s| s.player = Some(st.clone()));
    }
    let tx = with_shell(|s| {
        if matches!(msg, gifa_player::PlayerMsg::Log { .. }) {
            s.inbox.push(body.to_string());
        }
        s.player_tx.clone()
    })
    .flatten();
    if let Some(tx) = tx {
        let _ = tx.send(msg);
    }
}

/// 发一条指令给页面 agent。
pub fn player_command(cmd: gifa_player::PlayerCommand) {
    let js = cmd.to_js();
    with_shell(|s| {
        let _ = s.stage_wv.evaluate_script(&js);
    });
}

/// 打开一个独立弹窗（扫码登录用）。
///
/// 关键点：**复用同一个 WebContext** —— 弹窗里登录成功后，Cookie 直接落到
/// 舞台那份 profile 里，不需要搬运 token，也不需要再登录一次。
pub fn open_popup(url: &str, title: &str, width: i32, height: i32) -> Result<(), String> {
    // 已经开着就只把它提到前面
    let existing = with_shell(|s| s.popup.as_ref().map(|p| p.hwnd)).flatten();
    if let Some(h) = existing {
        // 舞台窗是置顶的，弹窗必须也置顶，否则会被自己的浮窗压在下面（实测踩过）
        win32::topmost(h);
        unsafe {
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
        return Ok(());
    }

    // 居中到舞台窗所在屏幕
    let (px, py, pw, ph) = with_shell(|s| {
        let r = win32::window_rect(s.float);
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    })
    .unwrap_or((100, 100, 800, 600));
    let (x, y) = (px + (pw - width) / 2, py + (ph - height) / 2);

    let wtitle = wide(title);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_APPWINDOW,
            CLASS_POPUP,
            PCWSTR(wtitle.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN,
            x.max(0),
            y.max(0),
            width,
            height,
            None,
            None,
            Some(win32::hinstance()),
            None,
        )
        .map_err(|e| format!("创建弹窗失败: {e}"))?
    };

    let (cw, ch) = win32::client_size(hwnd);
    let wv = with_shell(|s| {
        let ctx = &mut s._web_context;
        let builder = WebViewBuilder::new_with_web_context(ctx)
            .with_url(url)
            .with_bounds(rect_for(RectI::new(0, 0, cw.max(1), ch.max(1))))
            // 登录窗只允许 B 站域名：第三方跳转一律拦下
            .with_navigation_handler(|u| is_bilibili_url(&u))
            .with_new_window_req_handler(|_u, _f| wry::NewWindowResponse::Deny)
            .with_devtools(false);
        builder
            .build_as_child(&HwndHost(hwnd.0 as isize))
            .map_err(|e| format!("弹窗 WebView 创建失败: {e}"))
    })
    .ok_or_else(|| "外壳尚未初始化".to_string())??;

    // 舞台窗是置顶的，弹窗必须也置顶，否则会被自己的浮窗压在下面（实测踩过）
    win32::topmost(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    with_shell(|s| s.popup = Some(Popup { hwnd, wv }));

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

pub fn close_popup() {
    let h = with_shell(|s| s.popup.as_ref().map(|p| p.hwnd)).flatten();
    if let Some(h) = h {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
    with_shell(|s| s.popup = None);
}

/// 圆盘状态：由应用层每个 tick 算好后丢进来。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscState {
    /// 是否显示（按住 L3 时为 true）
    pub visible: bool,
    /// 高亮扇区：`e`/`se`/`s`/`sw`/`w`/`nw`/`n`/`ne`，空串表示无
    pub highlight: String,
    /// 方位 → 窄标签
    pub labels: std::collections::BTreeMap<String, String>,
    /// 高亮的是按住类动作（中心提示改成"回中恢复"）
    pub hold: bool,
}

/// 更新圆盘：需要时惰性创建窗口，然后显示/隐藏 + 推状态。
pub fn disc_update(state: DiscState) {
    let size = disc_size();

    if state.visible && !with_shell(|s| s.disc.is_some()).unwrap_or(false) {
        if let Err(e) = disc_create(size) {
            eprintln!("[disc] 创建失败：{e}");
            return;
        }
    }

    let Some(hwnd) = with_shell(|s| {
        let d = s.disc.as_mut()?;
        let json = serde_json::to_string(&state).unwrap_or_else(|_| "{}".into());
        if d.last != json {
            d.last = json.clone();
            let js = format!("window.gifaDisc && window.gifaDisc({json})");
            let _ = d.wv.evaluate_script(&js);
        }
        Some(d.hwnd)
    })
    .flatten() else {
        return;
    };

    unsafe {
        if state.visible {
            // 每次弹出都重新贴到浮窗中心（浮窗可能刚被拖过）
            if let Some(r) = with_shell(|s| win32::window_rect(s.float)) {
                let x = r.left + ((r.right - r.left) - size) / 2;
                let y = r.top + ((r.bottom - r.top) - size) / 2;
                let _ = SetWindowPos(hwnd, None, x, y, size, size, SWP_NOACTIVATE | SWP_NOZORDER);
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// 圆盘窗口边长：半径 ×2.24（和原版同一规则，留 12% 余量给扇区文字）
pub fn disc_size() -> i32 {
    (132.0_f64 * 2.24).round() as i32
}

fn is_chrome(hwnd: HWND) -> bool {
    with_shell(|s| s.chrome == hwnd).unwrap_or(false)
}

fn is_disc(hwnd: HWND) -> bool {
    with_shell(|s| s.disc.as_ref().is_some_and(|d| d.hwnd == hwnd)).unwrap_or(false)
}

fn disc_create(size: i32) -> Result<(), String> {
    let title = wide("GIFA disc");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOPMOST,
            CLASS_DISC,
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            size,
            size,
            None,
            None,
            Some(win32::hinstance()),
            None,
        )
        .map_err(|e| format!("创建圆盘窗口失败: {e}"))?
    };
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }

    let (cw, ch) = win32::client_size(hwnd);
    let wv = with_shell(|s| {
        let ctx = &mut s._web_context;
        WebViewBuilder::new_with_web_context(ctx)
            .with_html(DISC_HTML)
            .with_transparent(true)
            .with_bounds(rect_for(RectI::new(0, 0, cw.max(1), ch.max(1))))
            .with_devtools(false)
            .build_as_child(&HwndHost(hwnd.0 as isize))
            .map_err(|e| format!("圆盘 WebView 创建失败: {e}"))
    })
    .ok_or_else(|| "外壳尚未初始化".to_string())??;

    with_shell(|s| {
        s.disc = Some(Disc {
            hwnd,
            wv,
            last: String::new(),
        })
    });
    Ok(())
}

/// 叠加层状态：两路角度**互相独立**，各自 `Option`。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayState {
    /// 行进方向（罗盘角）
    pub arrow: Option<f64>,
    /// 镜头视角（罗盘角）
    pub view: Option<f64>,
    #[serde(default)]
    pub view_conf: f64,
    #[serde(default)]
    pub view_width: f64,
    /// 这一帧判据是"疑似反向"：保持原角度不动
    #[serde(default)]
    pub hold: bool,
    /// 全部清空
    #[serde(default)]
    pub clear: bool,
    /// 只清箭头（视角继续走）
    #[serde(default)]
    pub clear_angle: bool,
}

/// 叠加层边长：半径 ×2.24（与原版同一条余量规则，箭头尖端不会被裁）
pub fn overlay_size(radius: i32) -> i32 {
    (radius.clamp(50, 400) as f64 * 2.24).round() as i32
}

/// 创建叠加层（幂等）。`center` 为屏幕坐标圆心，缺省放工作区左上角附近。
pub fn overlay_init(radius: i32, center: Option<(i32, i32)>) -> Result<(), String> {
    if with_shell(|s| s.overlay.is_some()).unwrap_or(false) {
        if let Some(r) = center {
            overlay_move_center(r.0, r.1);
        }
        return Ok(());
    }
    let size = overlay_size(radius);
    let (cx, cy) = center.unwrap_or_else(|| {
        with_shell(|s| {
            let w = win32::work_area(s.float);
            (w.left + 260, w.top + 260)
        })
        .unwrap_or((400, 300))
    });

    let title = wide("GIFA overlay");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOPMOST,
            CLASS_OVERLAY,
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            cx - size / 2,
            cy - size / 2,
            size,
            size,
            None,
            None,
            Some(win32::hinstance()),
            None,
        )
        .map_err(|e| format!("创建叠加层失败: {e}"))?
    };
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }

    let (cw, ch) = win32::client_size(hwnd);
    let wv = with_shell(|s| {
        let ctx = &mut s._web_context;
        let mut builder = WebViewBuilder::new_with_web_context(ctx)
            .with_html(OVERLAY_HTML)
            .with_transparent(true)
            .with_bounds(rect_for(RectI::new(0, 0, cw.max(1), ch.max(1))))
            .with_ipc_handler(|req: wry::http::Request<String>| on_overlay_message(req.body()))
            .with_devtools(false);
        builder = builder.with_initialization_script("");
        builder
            .build_as_child(&HwndHost(hwnd.0 as isize))
            .map_err(|e| format!("叠加层 WebView 创建失败: {e}"))
    })
    .ok_or_else(|| "外壳尚未初始化".to_string())??;

    with_shell(|s| {
        s.overlay = Some(Overlay {
            hwnd,
            wv,
            radius: radius.clamp(50, 400),
            placing: false,
            drawn: (None, None),
            view_stale: false,
            drew_arc: false,
            drawn_log: std::collections::VecDeque::new(),
        })
    });
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(())
}

/// 推一帧角度给叠加层
pub fn overlay_update(state: OverlayState) {
    let json = match serde_json::to_string(&state) {
        Ok(j) => j,
        Err(_) => return,
    };
    with_shell(|s| {
        if let Some(o) = s.overlay.as_ref() {
            let js = format!("window.gifaOverlay && window.gifaOverlay({json})");
            let _ = o.wv.evaluate_script(&js);
        }
    });
}

/// 放置模式：开=可拖动可点击，关=恢复穿透
pub fn overlay_set_placing(on: bool) {
    let payload = if on { "true" } else { "false" };
    let hwnd = with_shell(|s| {
        let o = s.overlay.as_mut()?;
        o.placing = on;
        let _ =
            o.wv.evaluate_script(&format!("window.gifaPlace && window.gifaPlace({payload})"));
        Some(o.hwnd)
    })
    .flatten();
    let Some(h) = hwnd else { return };
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let next = if on {
            ex & !(WS_EX_TRANSPARENT.0 as isize)
        } else {
            ex | WS_EX_TRANSPARENT.0 as isize
        };
        if next != ex {
            SetWindowLongPtrW(h, GWL_EXSTYLE, next);
            let _ = SetWindowPos(
                h,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

pub fn overlay_placing() -> bool {
    with_shell(|s| s.overlay.as_ref().map(|o| o.placing))
        .flatten()
        .unwrap_or(false)
}

pub fn overlay_visible() -> bool {
    with_shell(|s| {
        s.overlay
            .as_ref()
            .map(|o| unsafe { IsWindowVisible(o.hwnd).as_bool() })
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

/// 当前圆心（屏幕坐标）
pub fn overlay_center() -> Option<(i32, i32)> {
    let h = with_shell(|s| s.overlay.as_ref().map(|o| o.hwnd)).flatten()?;
    let r = win32::window_rect(h);
    Some((
        r.left + (r.right - r.left) / 2,
        r.top + (r.bottom - r.top) / 2,
    ))
}

/// 按圆心摆放（半径变化时也要跟着改大小）
pub fn overlay_move_center(x: i32, y: i32) {
    let Some((h, size)) =
        with_shell(|s| s.overlay.as_ref().map(|o| (o.hwnd, overlay_size(o.radius)))).flatten()
    else {
        return;
    };
    unsafe {
        let _ = SetWindowPos(
            h,
            None,
            x - size / 2,
            y - size / 2,
            size,
            size,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 改半径（50~400），窗口跟着缩放但圆心不动
pub fn overlay_set_radius(radius: i32) {
    let r = radius.clamp(50, 400);
    let center = overlay_center();
    with_shell(|s| {
        if let Some(o) = s.overlay.as_mut() {
            o.radius = r;
        }
    });
    if let Some((x, y)) = center {
        overlay_move_center(x, y);
    }
}

/// 页面回报的"当前画出来的角度"，自检用
pub fn overlay_drawn() -> (Option<f64>, Option<f64>) {
    with_shell(|s| s.overlay.as_ref().map(|o| o.drawn))
        .flatten()
        .unwrap_or((None, None))
}

/// 最近画过的角度序列（诊断/自检：补间过程可见）
pub fn overlay_drawn_log() -> Vec<(Option<f64>, Option<f64>)> {
    with_shell(|s| {
        s.overlay
            .as_ref()
            .map(|o| o.drawn_log.iter().copied().collect::<Vec<_>>())
    })
    .flatten()
    .unwrap_or_default()
}

/// 最后一帧是否真的画了绿弧（诊断）
pub fn overlay_drew_arc() -> bool {
    with_shell(|s| s.overlay.as_ref().map(|o| o.drew_arc))
        .flatten()
        .unwrap_or(false)
}

/// 页面是否已把绿弧判为过期（1.5s 没有新视角）
pub fn overlay_view_stale() -> bool {
    with_shell(|s| s.overlay.as_ref().map(|o| o.view_stale))
        .flatten()
        .unwrap_or(true)
}

/// 叠加层窗口矩形（自检核对尺寸用）
pub fn overlay_rect() -> Option<RectI> {
    let h = with_shell(|s| s.overlay.as_ref().map(|o| o.hwnd)).flatten()?;
    let r = win32::window_rect(h);
    Some(RectI::new(
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
    ))
}

fn on_overlay_message(body: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    if v.get("t").and_then(|x| x.as_str()) != Some("drawn") {
        return;
    }
    let arrow = v.get("arrow").and_then(|x| x.as_f64());
    let view = v.get("view").and_then(|x| x.as_f64());
    let stale = v.get("stale").and_then(|x| x.as_bool()).unwrap_or(false);
    with_shell(|s| {
        if let Some(o) = s.overlay.as_mut() {
            o.drawn = (arrow, view);
            o.view_stale = stale;
            o.drew_arc = v.get("arc").and_then(|x| x.as_bool()).unwrap_or(false);
            o.drawn_log.push_back((arrow, view));
            while o.drawn_log.len() > 120 {
                o.drawn_log.pop_front();
            }
        }
    });
}

fn is_overlay(hwnd: HWND) -> bool {
    with_shell(|s| s.overlay.as_ref().is_some_and(|o| o.hwnd == hwnd)).unwrap_or(false)
}

pub fn disc_visible() -> bool {
    with_shell(|s| {
        s.disc
            .as_ref()
            .map(|d| unsafe { IsWindowVisible(d.hwnd).as_bool() })
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

pub fn popup_open() -> bool {
    with_shell(|s| s.popup.is_some()).unwrap_or(false)
}

/// 只放行 bilibili.com（含子域）；协议只允许 http/https
pub fn is_bilibili_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = match lower.split_once("://") {
        Some(("http", r)) | Some(("https", r)) => r,
        _ => return false,
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    host == "bilibili.com"
        || host.ends_with(".bilibili.com")
        // B 站自家的短链域名
        || host == "b23.tv"
        || host.ends_with(".b23.tv")
        || host == "bili2233.cn"
        || host.ends_with(".bili2233.cn")
}

/// 屏幕坐标处的命中归属（自检用：判定点击会落到 chrome 还是舞台）
pub fn classify_at(x: i32, y: i32) -> crate::probe::Hit {
    match (float_hwnd(), chrome_hwnd()) {
        (Some(f), Some(c)) => crate::probe::classify(x, y, f, c),
        _ => crate::probe::Hit::Other,
    }
}

/// 最近一次播放状态。
pub fn player_state() -> Option<gifa_player::PlayerState> {
    with_shell(|s| s.player.clone()).flatten()
}

/// 让舞台导航到新地址（会自动重新注入 agent）。
pub fn navigate_stage(url: &str) {
    with_shell(|s| {
        s.stage_url = url.to_string();
    });
    with_shell(|s| {
        let _ = s.stage_wv.load_url(url);
    });
}

fn build_chrome(chrome: &HWND, cfg: &ShellConfig) -> Result<WebView, String> {
    let mut b = WebViewBuilder::new()
        .with_transparent(true)
        .with_bounds(rect_for(RectI::new(0, 0, 640, 360)))
        .with_ipc_handler(|req: wry::http::Request<String>| {
            on_chrome_message(req.body());
        })
        .with_devtools(false);
    // 给了 file:// 地址就用地址加载：同级 style.css / app.js 才能解析
    b = match cfg.chrome_url.as_deref() {
        Some(url) => b.with_url(url),
        None => b.with_html(&cfg.chrome_html),
    };
    b.build_as_child(&HwndHost(chrome.0 as isize))
        .map_err(|e| format!("创建 chrome WebView2 失败: {e}"))
}

/// 把落盘的窗口位置还给调用方（用于持久化）。
pub fn bounds() -> Option<Bounds> {
    let r = with_shell(|s| win32::window_rect(s.float))?;
    Some(Bounds::new(
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
    ))
}

fn float_hwnd() -> Option<HWND> {
    // 先试本线程的状态；后台线程没有 TLS，就退回全局副本
    with_shell(|s| s.float).or_else(|| {
        let raw = FLOAT_HWND.load(std::sync::atomic::Ordering::SeqCst);
        (raw != 0).then_some(HWND(raw as *mut std::ffi::c_void))
    })
}

fn chrome_hwnd() -> Option<HWND> {
    with_shell(|s| s.chrome)
}

/// 设置窗口矩形。
///
/// 注意：**先取 HWND、再释放借用**，然后才 `SetWindowPos` ——
/// 否则 `WM_WINDOWPOSCHANGED` 回来时借用还没放下，`sync()` 会被跳过一次。
pub fn set_bounds(b: Bounds) {
    let Some(h) = float_hwnd() else { return };
    unsafe {
        let _ = SetWindowPos(
            h,
            None,
            b.x,
            b.y,
            b.w.max(layout::MIN_W),
            b.h.max(layout::MIN_H),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn set_opacity(v: f64) {
    let alpha = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    with_shell(|s| {
        s.opacity = alpha;
        // 用户设定的才是基准；鼠标避让只是临时覆盖
        s.base_opacity = alpha;
    });
    let Some(f) = float_hwnd() else { return };
    // 舞台：整窗 alpha（会连 WebView2 一起淡）
    win32::set_opacity(f, alpha);
    // chrome：不能用 LWA（会把逐像素透明压成整体 alpha），改让页面自己设 opacity
    let js = format!(
        "document.documentElement.style.opacity = '{:.3}'",
        alpha as f64 / 255.0
    );
    with_shell(|s| {
        let _ = s.chrome_wv.evaluate_script(&js);
    });
}

/// 进入/退出纯视频沉浸。
///
/// `aspect` = 视频宽/高（0 表示还不知道，那就只做隐藏与穿透，不贴合比例）。
///
/// 进入时：记下原矩形、隐藏 chrome、强制点击穿透、按视频比例贴合；
/// 退出时：精确还原矩形（含 ±2px 舍入修正）、恢复 chrome 与原穿透状态。
/// **不写设置** —— 沉浸是运行时状态，重启不自动进入（与原版一致）。
pub fn set_immersion(on: bool, aspect: f64) -> bool {
    let current = with_shell(|s| s.immersion).unwrap_or(false);
    if current == on {
        return current;
    }

    if on {
        // 记原矩形 + 切状态
        let saved = with_shell(|s| {
            let r = win32::window_rect(s.float);
            s.immersion_bounds = Some((r.left, r.top, r.right - r.left, r.bottom - r.top));
            s.immersion = true;
            s.interactive = false;
            s.hovered = false;
            s.hover_candidate = false;
            s.hover_candidate_since = None;
        });
        let _ = saved;
        // 隐藏 chrome，强制穿透
        let Some(c) = chrome_hwnd() else { return false };
        unsafe {
            let _ = ShowWindow(c, SW_HIDE);
        }
        set_click_through(true);
        // 沉浸时舞台要占满客户区：把标题栏/控制条的留白清零
        with_shell(|s| {
            s.saved_layout = Some(s.layout.clone());
            s.layout = ChromeLayout {
                titlebar: 0,
                footer: 0,
                drag: None,
                urlbar: None,
                overlay: Vec::new(),
            };
        });
        sync();
        fit_immersion(aspect);
        // 60ms 心跳：鼠标避让
        start_hover_timer(true);
        crate::shell_log!("进入沉浸（比例 {aspect:.3}）");
    } else {
        start_hover_timer(false);
        with_shell(|s| {
            s.immersion = false;
            s.interactive = true;
            s.hovered = false;
        });
        if float_hwnd().is_none() {
            return false;
        }
        // 先把透明度恢复，再还原矩形（避免半透明窗口在还原瞬间闪一下）
        with_shell(|s| {
            let base = s.base_opacity;
            s.opacity = base;
            win32::set_opacity(s.float, base);
        });
        restore_immersion_bounds();
        if let Some(c) = chrome_hwnd() {
            unsafe {
                let _ = ShowWindow(c, SW_SHOWNOACTIVATE);
            }
        }
        set_click_through(false);
        with_shell(|s| {
            if let Some(l) = s.saved_layout.take() {
                s.layout = l;
            }
        });
        sync();
        crate::shell_log!("退出沉浸");
    }
    with_shell(|s| s.immersion).unwrap_or(false)
}

pub fn immersion() -> bool {
    with_shell(|s| s.immersion).unwrap_or(false)
}

/// 沉浸时是否要把鼠标移入压暗
pub fn set_hover_fade(on: bool) {
    with_shell(|s| s.hover_fade = on);
}

/// 鼠标穿透（`WS_EX_TRANSPARENT`）：点击直接落到下面的游戏
pub fn set_click_through(on: bool) {
    let Some(h) = float_hwnd() else { return };
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let mut next = ex;
        if on {
            next |= (WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0) as isize;
        } else {
            next &= !(WS_EX_TRANSPARENT.0 as isize);
        }
        if next != ex {
            SetWindowLongPtrW(h, GWL_EXSTYLE, next);
            let _ = SetWindowPos(
                h,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
    with_shell(|s| s.click_through = on);
}

pub fn click_through() -> bool {
    with_shell(|s| s.click_through).unwrap_or(false)
}

/// 按视频比例把窗口缩到"原矩形内刚好贴边"，并居中。
fn fit_immersion(aspect: f64) {
    if !(aspect > 0.1 && aspect < 10.0) {
        return;
    }
    let (Some(h), Some(saved)) = (float_hwnd(), with_shell(|s| s.immersion_bounds).flatten())
    else {
        return;
    };
    let (sx, sy, sw, sh) = saved;
    let work = win32::work_area(h);
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    // 不超出原矩形，也不超出工作区
    let max_w = sw.min(ww);
    let max_h = sh.min(wh);
    let mut w = max_w.min((max_h as f64 * aspect).round() as i32);
    let mut hh = (w as f64 / aspect).round() as i32;
    if hh > max_h {
        hh = max_h;
        w = (hh as f64 * aspect).round() as i32;
    }
    w = w.max(160);
    hh = hh.max(90);
    let x = sx + (sw - w) / 2;
    let y = sy + (sh - hh) / 2;
    unsafe {
        let _ = SetWindowPos(h, None, x, y, w, hh, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// 还原沉浸前的矩形。系统对 `SetWindowPos` 可能做 1~2px 舍入，
/// 这里读回实际值再补一次差值（原版同样处理）。
fn restore_immersion_bounds() {
    let (Some(h), Some(saved)) = (float_hwnd(), with_shell(|s| s.immersion_bounds).flatten())
    else {
        return;
    };
    let (x, y, w, hh) = saved;
    unsafe {
        let _ = SetWindowPos(h, None, x, y, w, hh, SWP_NOZORDER | SWP_NOACTIVATE);
    }
    for _ in 0..2 {
        let r = win32::window_rect(h);
        let dx = x - r.left;
        let dy = y - r.top;
        let dw = w - (r.right - r.left);
        let dh = hh - (r.bottom - r.top);
        if dx.abs() <= 1 && dy.abs() <= 1 && dw.abs() <= 1 && dh.abs() <= 1 {
            break;
        }
        unsafe {
            let _ = SetWindowPos(
                h,
                None,
                x + dx,
                y + dy,
                w + dw,
                hh + dh,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    with_shell(|s| s.immersion_bounds = None);
}

fn start_hover_timer(on: bool) {
    let Some(h) = float_hwnd() else { return };
    unsafe {
        if on {
            SetTimer(Some(h), HOVER_TIMER_ID, 60, None);
        } else {
            let _ = KillTimer(Some(h), HOVER_TIMER_ID);
        }
    }
}

/// 鼠标避让：移入降到 min(base, 0.1)，移出恢复。
/// 60ms 轮询 + 120ms 防抖（原版同样的时序），临时值不写设置。
fn hover_tick() {
    HOVER_TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some(h) = float_hwnd() else { return };
    let now = std::time::Instant::now();
    let (immersion, enabled, base, candidate, since, hovered) = with_shell(|s| {
        (
            s.immersion,
            s.hover_fade,
            s.base_opacity,
            s.hover_candidate,
            s.hover_candidate_since,
            s.hovered,
        )
    })
    .unwrap_or((false, false, 255, false, None, false));
    if !immersion || !enabled {
        return;
    }

    let (cx, cy) = win32::cursor_pos();
    let r = win32::window_rect(h);
    let inside = cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom;

    let mut since = since;
    if inside != candidate {
        with_shell(|s| {
            s.hover_candidate = inside;
            s.hover_candidate_since = Some(now);
        });
        since = Some(now);
    }
    // 进出都要压住 120ms，避免在边缘抖动
    if since.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(120)) {
        return;
    }
    if inside == hovered {
        return;
    }
    let alpha = if inside { base.min(26) } else { base }; // 0.1 * 255 ≈ 26
    with_shell(|s| {
        s.hovered = inside;
        s.opacity = alpha;
        win32::set_opacity(s.float, alpha);
    });
}

const HOVER_TIMER_ID: usize = 0xA1;

static HOVER_TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 诊断：避让心跳被触发过多少次
pub fn hover_ticks() -> u64 {
    HOVER_TICKS.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_interactive(on: bool) {
    with_shell(|s| s.interactive = on);
}

/// 找游戏主窗口：按窗口类名精确匹配，返回 (hwnd, 客户区屏幕矩形)。
///
/// 原神的窗口类是 `UnityWndClass`（标题 `YuanShen`）。取**客户区**而不是整窗：
/// 无边框全屏时两者相同，窗口模式下客户区才是画面区域。
pub fn find_window_by_class(class: &str) -> Option<(isize, RectI)> {
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetClientRect, IsWindowVisible,
    };

    struct Ctx {
        target: Vec<u16>,
        found: Vec<(isize, RectI)>,
    }

    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows::core::BOOL {
        let ctx = &mut *(l.0 as *mut Ctx);
        let mut buf = [0u16; 256];
        let n = GetClassNameW(h, &mut buf);
        if n <= 0 {
            return true.into();
        }
        if &buf[..n as usize] != ctx.target.as_slice() {
            return true.into();
        }
        if !IsWindowVisible(h).as_bool() {
            return true.into();
        }
        let mut rc = RECT::default();
        if GetClientRect(h, &mut rc).is_err() {
            return true.into();
        }
        let mut tl = POINT { x: 0, y: 0 };
        let mut br = POINT {
            x: rc.right,
            y: rc.bottom,
        };
        let _ = ClientToScreen(h, &mut tl);
        let _ = ClientToScreen(h, &mut br);
        let r = RectI {
            x: tl.x,
            y: tl.y,
            w: br.x - tl.x,
            h: br.y - tl.y,
        };
        if r.w > 200 && r.h > 200 {
            ctx.found.push((h.0 as isize, r));
        }
        true.into()
    }

    let mut ctx = Ctx {
        target: class.encode_utf16().collect(),
        found: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    // 同类名可能有多份（多开/残留）：取面积最大的
    ctx.found.sort_by_key(|(_, r)| -(r.w as i64 * r.h as i64));
    ctx.found.into_iter().next()
}

/// 诊断：列出可见的顶层窗口（类名 / 标题 / 尺寸），排查"找不到游戏窗口"
pub fn list_windows() -> Vec<(String, String, RectI)> {
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetClientRect, GetWindowTextW, IsWindowVisible,
    };

    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows::core::BOOL {
        let out = &mut *(l.0 as *mut Vec<(String, String, RectI)>);
        if !IsWindowVisible(h).as_bool() {
            return true.into();
        }
        let mut cb_buf = [0u16; 256];
        let n = GetClassNameW(h, &mut cb_buf);
        let class = String::from_utf16_lossy(&cb_buf[..n.max(0) as usize]);
        let mut t_buf = [0u16; 256];
        let tn = GetWindowTextW(h, &mut t_buf);
        let title = String::from_utf16_lossy(&t_buf[..tn.max(0) as usize]);
        let mut rc = RECT::default();
        if GetClientRect(h, &mut rc).is_ok() && rc.right > 300 && rc.bottom > 300 {
            out.push((
                class,
                title,
                RectI {
                    x: 0,
                    y: 0,
                    w: rc.right,
                    h: rc.bottom,
                },
            ));
        }
        true.into()
    }

    let mut out: Vec<(String, String, RectI)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// 主屏工作区（应用层算默认窗口尺寸/位置时用）
pub fn primary_work_area() -> RectI {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut rc = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rc as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    RectI {
        x: rc.left,
        y: rc.top,
        w: rc.right - rc.left,
        h: rc.bottom - rc.top,
    }
}

/// 取走一个待处理的系统电源事件（应用层在 tick 里调）
pub fn take_power_event() -> Option<PowerEvent> {
    with_shell(|s| s.power.take()).flatten()
}

/// 告诉系统"别挂起我"。
///
/// 常驻工具的最小化/后台场景下，进程被 EcoQoS 或省电策略挂起，
/// 全局钩子和识别线程都会停 —— 用户看到的就是"窗口最小化后热键失灵"。
pub fn prevent_suspension() -> bool {
    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };
    // 返回 0 表示失败（上一次的设置被沿用）；非 0 是设置前的旧状态
    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED).0 != 0 }
}

/// 当前进程是否已提权（决定热键能不能收到提权游戏里的按键）
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// 以管理员身份重启自己（UAC 被取消时返回错误，调用方继续以普通权限运行）。
///
/// 提权后的新进程会带 `--elevated` 标记 —— 那个标记的唯一用途是**避免循环重启**：
/// 如果已经是管理员却仍不满足条件，就不要再弹 UAC 了。
pub fn relaunch_elevated() -> Result<(), String> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = std::env::current_exe().map_err(|e| format!("取 exe 路径失败: {e}"))?;
    let exe_w: Vec<u16> = exe
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // 参数原样带过去，但去掉可能已经存在的 --elevated（避免重复）
    let mut args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--elevated")
        .collect();
    args.push("--elevated".into());
    let args_w: Vec<u16> = args
        .join(" ")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let ret = unsafe {
        ShellExecuteW(
            None,
            w!("runas"),
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(args_w.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 返回值 > 32 才算成功
    if ret.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("UAC 被取消或启动失败（{}）", ret.0 as isize))
    }
}

/// 单实例：抢一个命名互斥体。返回 true = 本进程是唯一实例。
///
/// 抢不到说明已有实例在跑 —— 调用方应该把那个窗口叫回来再退出。
/// 互斥体句柄存进 Shell 状态，进程退出时由系统回收（不需要显式 CloseHandle）。
pub fn try_single_instance(name: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let name_w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name_w.as_ptr())) };
    match handle {
        Ok(h) => {
            let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            if already {
                return false;
            }
            with_shell(|s| s.instance_mutex = Some(h.0 as isize));
            true
        }
        // 建不出来就当作"没有其它实例"，不要因为保护机制挡住启动
        Err(_) => true,
    }
}

/// 把已经在跑的实例叫回来（单实例的第二次启动）
pub fn activate_existing() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    for class in ["GifaFloat", "GifaChrome"] {
        let c: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
        let h = unsafe { FindWindowW(PCWSTR(c.as_ptr()), PCWSTR::null()) };
        if let Ok(h) = h {
            if !h.is_invalid() {
                unsafe {
                    if IsIconic(h).as_bool() {
                        let _ = ShowWindow(h, SW_RESTORE);
                    }
                    let _ = SetForegroundWindow(h);
                }
                return true;
            }
        }
    }
    false
}

/// 诊断：屏幕坐标处 WindowFromPoint 返回的 HWND 裸值
pub fn window_at(x: i32, y: i32) -> isize {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::WindowFromPoint;
    unsafe { WindowFromPoint(POINT { x, y }).0 as isize }
}

/// 浮窗在自己的区域上是否真的"露着"（没被别的窗口盖住）。
///
/// 截屏是按**屏幕坐标**取的：窗口被独占全屏的游戏盖住时，截到的是游戏画面，
/// 识别就会把游戏里的青色当成小地图箭头 —— 必须自己挡住这种"看着有数据"的假象。
pub fn float_on_top_at(x: i32, y: i32) -> bool {
    matches!(
        classify_at(x, y),
        crate::probe::Hit::Float | crate::probe::Hit::Chrome
    )
}

/// 弹一个系统消息框（WebView2 缺失这类"跑不起来"的情况，页面还没建）
pub fn message_box(title: &str, text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let t = wide(title);
    let m = wide(text);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(m.as_ptr()),
            PCWSTR(t.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 前台窗口诊断：类名 + 是否铺满屏幕（判断"游戏是不是独占全屏"）
pub fn foreground_info() -> (String, bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return ("(none)".into(), false);
        }
        let mut buf = [0u16; 128];
        let n = GetClassNameW(fg, &mut buf);
        let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        let r = win32::window_rect(fg);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let full = (r.right - r.left) >= sw && (r.bottom - r.top) >= sh;
        (class, full)
    }
}

/// 把浮窗（和 chrome）切到前台（自检用）
pub fn focus_float() {
    if let Some(h) = float_hwnd() {
        unsafe {
            let _ = SetForegroundWindow(h);
        }
    }
}

/// 轮询式拖动：WebView2 在宿主未激活时不派发 DOM 鼠标事件，
/// 所以不能只靠页面的 mousedown。这里直接看"光标是否在拖动区 + 左键刚按下"。
fn drag_poll_tick() {
    static LAST_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let down = win32::left_button_down();
    // 每秒打一次现场（调试拖动时很有用）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now != LAST_LOG.load(std::sync::atomic::Ordering::Relaxed) {
        LAST_LOG.store(now, std::sync::atomic::Ordering::Relaxed);
        let (cx, cy) = win32::cursor_pos();
        let zone = with_shell(|s| s.layout.drag).flatten();
        crate::shell_log!("drag_poll tick 光标=({cx},{cy}) 按下={down} 区域={zone:?}");
    }
    let was = DRAG_WAS_DOWN.swap(down, std::sync::atomic::Ordering::Relaxed);
    if !down || was {
        return;
    }
    if DRAG_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let Some(zone) = with_shell(|s| s.layout.drag).flatten() else {
        return;
    };
    // 页面报的是**窗口内**坐标，要和屏幕光标比，得先加上 chrome 窗原点
    let Some(origin) = chrome_hwnd().map(win32::window_rect) else {
        return;
    };
    let zx = origin.left + zone.x;
    let zy = origin.top + zone.y;
    let (cx, cy) = win32::cursor_pos();
    if cx < zx || cy < zy || cx >= zx + zone.w || cy >= zy + zone.h {
        return;
    }
    // 光标下必须是我们的窗（否则是在点别的窗口）
    if !matches!(
        classify_at(cx, cy),
        crate::probe::Hit::Float | crate::probe::Hit::Chrome
    ) {
        return;
    }
    crate::shell_log!("drag_poll 命中拖动区 ({cx},{cy})");
    begin_drag();
}

static DRAG_WAS_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DRAG_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const DRAG_TIMER_ID: usize = 0xA2;

/// 自己驱动拖拽：不进入系统的模态移动循环。
///
/// 为什么不用 `WM_NCLBUTTONDOWN`：那个循环依赖窗口拿到鼠标捕获，
/// 而游戏（尤其是带鼠标锁定的窗口化游戏）会把捕获抢走，循环就卡死不跟手。
/// 这里直接轮询光标 + `SetWindowPos`，只要左键还按着就跟着走。
fn begin_drag() {
    DRAG_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    let Some(h) = float_hwnd() else {
        DRAG_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    let (mx, my) = win32::cursor_pos();
    let r = win32::window_rect(h);
    let (wx, wy) = (r.left, r.top);
    let raw = h.0 as isize; // HWND 不是 Send，跨线程只传裸值
    crate::shell_log!("begin_drag from ({mx},{my}) window ({wx},{wy})");

    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(8));
            if !win32::left_button_down() {
                break;
            }
            if start.elapsed().as_secs() > 60 {
                break;
            }
            let (cx, cy) = win32::cursor_pos();
            let nx = wx + (cx - mx);
            let ny = wy + (cy - my);
            unsafe {
                let _ = SetWindowPos(
                    HWND(raw as *mut _),
                    None,
                    nx,
                    ny,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        DRAG_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::shell_log!("begin_drag end");
    });
}

/// 最小化（标题栏的最小化按钮）
pub fn minimize() {
    if let Some(h) = float_hwnd() {
        unsafe {
            let _ = ShowWindow(h, SW_MINIMIZE);
        }
    }
}

pub fn hide() {
    let (Some(f), Some(c)) = (float_hwnd(), chrome_hwnd()) else {
        return;
    };
    unsafe {
        let _ = ShowWindow(c, SW_HIDE);
        let _ = ShowWindow(f, SW_HIDE);
    }
}

pub fn show_inactive() {
    let (Some(f), Some(c)) = (float_hwnd(), chrome_hwnd()) else {
        return;
    };
    unsafe {
        let _ = ShowWindow(f, SW_SHOWNOACTIVATE);
        let _ = ShowWindow(c, SW_SHOWNOACTIVATE);
    }
    win32::topmost(f);
    win32::topmost(c);
}

/// 调试/自检用的快照。
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// 舞台窗 HWND（裸值，便于自检里做命中测试）
    pub float: isize,
    /// chrome 窗 HWND
    pub chrome: isize,
    pub float_rect: RectI,
    pub client: (i32, i32),
    pub chrome_rect: RectI,
    pub stage: RectI,
    pub region: Vec<RectI>,
    pub chrome_owner: bool,
    pub layout: ChromeLayout,
    pub opacity: u8,
    pub topmost: bool,
    /// 沉浸模式是否开启
    pub immersion: bool,
    /// 鼠标避让是否把窗口压暗了
    pub hovered: bool,
    /// 窗口是否点击穿透
    pub click_through: bool,
    /// chrome 层是否可见
    pub chrome_visible: bool,
}

pub fn snapshot() -> Option<Snapshot> {
    with_shell(|s| {
        let fr = win32::window_rect(s.float);
        let cr = win32::window_rect(s.chrome);
        let (cw, ch) = win32::client_size(s.float);
        let stage = s.layout.stage_rect(cw, ch);
        let region = s
            .layout
            .region(cw, ch)
            .normalized(cw, ch)
            .unwrap_or_default();
        let owner = unsafe {
            GetWindow(s.chrome, GW_OWNER)
                .map(|h| h == s.float)
                .unwrap_or(false)
        };
        Snapshot {
            float: s.float.0 as isize,
            chrome: s.chrome.0 as isize,
            float_rect: RectI::new(fr.left, fr.top, fr.right - fr.left, fr.bottom - fr.top),
            client: (cw, ch),
            chrome_rect: RectI::new(cr.left, cr.top, cr.right - cr.left, cr.bottom - cr.top),
            stage,
            region,
            chrome_owner: owner,
            layout: s.layout.clone(),
            opacity: s.opacity,
            topmost: win32::is_topmost(s.float),
            immersion: s.immersion,
            hovered: s.hovered,
            click_through: s.click_through,
            chrome_visible: unsafe { IsWindowVisible(s.chrome).as_bool() },
        }
    })
}

pub fn inbox() -> Vec<String> {
    with_shell(|s| s.inbox.clone()).unwrap_or_default()
}

pub fn run() {
    win32::run_message_loop();
    // WebView 的析构要走 COM，直接退更干净
    std::process::exit(0);
}

// ---------------------------------------------------------------- 内部

fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with(|slot| match slot.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        // 重入（sync 里 SetWindowPos 又会进 WndProc）时安全跳过
        Err(_) => None,
    })
}

/// 舞台矩形 + chrome 位置/尺寸 + 区域，一次算全。
fn sync() {
    SHELL.with(|slot| {
        let Ok(mut g) = slot.try_borrow_mut() else {
            return;
        };
        let Some(s) = g.as_mut() else { return };
        let r = win32::window_rect(s.float);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return;
        }

        // 舞台 WebView2 只占视频该占的地方（标题栏与底栏之间）
        let stage = s.layout.stage_rect(w, h);
        let _ = s.stage_wv.set_bounds(rect_for(stage));

        // chrome 顶层窗贴着舞台窗的整块矩形
        unsafe {
            let _ = SetWindowPos(
                s.chrome,
                None,
                r.left,
                r.top,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let _ = s.chrome_wv.set_bounds(rect_for(RectI::new(0, 0, w, h)));

        // 区域：只有顶栏/底栏/URL条/浮动控件存在，其余点击穿透
        let spec = s.layout.region(w, h);
        let n = spec.normalized(w, h).map(|v| v.len()).unwrap_or(0);
        spec.apply(s.chrome, w, h);

        // 把联动结果推给页面（自检/演示用；真实界面也能靠它做自适应）
        let push_key = (w, h, stage.h, n as i32);
        if push_key != s.last_push {
            s.last_push = push_key;
            let js = format!(
                "window.gifaSet && window.gifaSet({})",
                serde_json::json!({
                    "w": w, "h": h,
                    "stageW": stage.w, "stageH": stage.h,
                    "regions": n,
                })
            );
            let _ = s.chrome_wv.evaluate_script(&js);
        }
    });
}

fn on_chrome_message(body: &str) {
    // 应用层自定义指令（load / toggle / clean …）原样转发，外壳不解释
    let tx = with_shell(|s| s.chrome_tx.clone()).flatten();
    if let Some(tx) = tx {
        let _ = tx.send(body.to_string());
    }

    let msg: ChromeMsg = match serde_json::from_str(body) {
        Ok(m) => m,
        Err(e) => {
            crate::shell_log!("外壳未处理的消息 {body:?}: {e}");
            return;
        }
    };
    SHELL.with(|slot| {
        let Ok(mut g) = slot.try_borrow_mut() else {
            return;
        };
        let Some(s) = g.as_mut() else { return };
        s.inbox.push(body.to_string());
        match msg {
            ChromeMsg::Layout {
                titlebar,
                footer,
                urlbar,
                drag,
                overlay,
            } => {
                let incoming = ChromeLayout {
                    titlebar: titlebar.max(0),
                    footer: footer.max(0),
                    drag: drag.map(|r| r.to_rect()),
                    urlbar: urlbar.map(|r| r.to_rect()),
                    overlay: overlay.into_iter().map(|r| r.to_rect()).collect(),
                };
                if s.immersion {
                    // 沉浸期间舞台占满客户区，页面的布局只记下来、等退出时再用
                    s.saved_layout = Some(incoming);
                } else {
                    s.layout = incoming;
                    s.relayout_pending = true;
                }
            }
            ChromeMsg::Close => {
                let float = s.float;
                unsafe {
                    let _ = PostMessageW(Some(float), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            ChromeMsg::Drag => s.pending = Pending::Drag,
            ChromeMsg::Resize => s.pending = Pending::Resize,
            ChromeMsg::Opacity { v } => {
                let alpha = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                s.opacity = alpha;
                win32::set_opacity(s.float, alpha);
                let js = format!(
                    "document.documentElement.style.opacity = '{:.3}'",
                    alpha as f64 / 255.0
                );
                let _ = s.chrome_wv.evaluate_script(&js);
            }
        }
        // 自投递消息，等借用放下后再做需要重新借用的动作
        if s.pending != Pending::None {
            unsafe {
                let _ = PostMessageW(Some(s.chrome), WM_APP_CMD, WPARAM(0), LPARAM(0));
            }
        }
        if s.relayout_pending {
            unsafe {
                let _ = PostMessageW(Some(s.chrome), WM_APP_RELAYOUT, WPARAM(0), LPARAM(0));
            }
        }
    });
}

/// 走系统自带的移动/缩放循环：把 chrome 收到的按下转发成舞台窗的非客户区按下。
fn start_system_loop(hwnd: HWND, hit: u32) {
    crate::shell_log!("start_system_loop hit={hit}");
    unsafe {
        let _ = ReleaseCapture();
        let (x, y) = win32::cursor_pos();
        let _ = SendMessageW(
            hwnd,
            WM_NCLBUTTONDOWN,
            Some(WPARAM(hit as usize)),
            Some(win32::make_lparam(x, y)),
        );
    }
}

// ---------------------------------------------------------------- WndProc

extern "system" fn float_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    dispatch(hwnd, msg, wp, lp)
}

extern "system" fn chrome_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    dispatch(hwnd, msg, wp, lp)
}

extern "system" fn popup_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    dispatch(hwnd, msg, wp, lp)
}

extern "system" fn disc_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    dispatch(hwnd, msg, wp, lp)
}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    dispatch(hwnd, msg, wp, lp)
}

fn dispatch(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // ---- 舞台窗：无边框化（客户区 = 整窗）
        WM_NCCALCSIZE if wp.0 != 0 => {
            let maximized = unsafe { IsZoomed(hwnd).as_bool() };
            if maximized {
                // 最大化时系统会把窗口撑出边框厚度，这里补回来，否则内容溢出屏幕
                unsafe {
                    let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
                    let (fx, fy) = frame_thickness();
                    params.rgrc[0].left += fx;
                    params.rgrc[0].top += fy;
                    params.rgrc[0].right -= fx;
                    params.rgrc[0].bottom -= fy;
                }
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- 舞台窗：边框/标题栏命中（子窗未覆盖时的兜底）
        WM_NCHITTEST if with_shell(|s| s.float == hwnd).unwrap_or(false) => {
            let interactive = with_shell(|s| s.interactive).unwrap_or(false);
            if !interactive {
                return windows::Win32::Foundation::LRESULT(HTCLIENT as isize);
            }
            let (sx, sy) = win32::lparam_point(lp);
            let r = win32::window_rect(hwnd);
            let border = (RESIZE_BORDER as f64 * win32::scale(hwnd)).round() as i32;
            let (cx, cy) = (sx - r.left, sy - r.top);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let left = cx < border;
            let right = cx >= w - border;
            let top = cy < border;
            let bottom = cy >= h - border;
            let code = match (left, right, top, bottom) {
                (true, _, true, _) => HTTOPLEFT,
                (_, true, true, _) => HTTOPRIGHT,
                (true, _, _, true) => HTBOTTOMLEFT,
                (_, true, _, true) => HTBOTTOMRIGHT,
                (true, _, _, _) => HTLEFT,
                (_, true, _, _) => HTRIGHT,
                (_, _, true, _) => HTTOP,
                (_, _, _, true) => HTBOTTOM,
                _ => HTCLIENT,
            };
            windows::Win32::Foundation::LRESULT(code as isize)
        }

        // ---- 舞台窗：缩放上下限
        WM_GETMINMAXINFO => {
            let min = with_shell(|s| {
                let (w, h) = win32::client_size(s.float);
                let work = win32::work_area(s.float);
                layout::resize_floor(
                    WorkArea {
                        x: work.left,
                        y: work.top,
                        w: work.right - work.left,
                        h: work.bottom - work.top,
                    },
                    &s.layout,
                )
                .max((w.min(layout::MIN_W), h.min(layout::MIN_H)))
            });
            if let Some((min_w, min_h)) = min {
                unsafe {
                    let mmi = &mut *(lp.0 as *mut MINMAXINFO);
                    mmi.ptMinTrackSize = POINT { x: min_w, y: min_h };
                }
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- chrome 被激活：如果这时正按着标题栏，直接把拖动接上
        WM_ACTIVATE if is_chrome(hwnd) => {
            let active = (wp.0 & 0xFFFF) != 0;
            if active && win32::left_button_down() {
                let (cx, cy) = win32::cursor_pos();
                let in_zone = with_shell(|s| {
                    let Some(z) = s.layout.drag else { return false };
                    let o = win32::window_rect(s.chrome);
                    cx >= o.left + z.x
                        && cy >= o.top + z.y
                        && cx < o.left + z.x + z.w
                        && cy < o.top + z.y + z.h
                })
                .unwrap_or(false);
                if in_zone {
                    crate::shell_log!("激活即拖动 ({cx},{cy})");
                    begin_drag();
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }

        // ---- 鼠标避让心跳
        WM_TIMER if wp.0 == HOVER_TIMER_ID => {
            hover_tick();
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- 拖动轮询（16ms）
        WM_TIMER if wp.0 == DRAG_TIMER_ID => {
            drag_poll_tick();
            windows::Win32::Foundation::LRESULT(0)
        }

        /* ---- 电源事件：挂起/锁屏/恢复
         *
         * 只在钩子里记一笔，真正的处理交给应用层（它才知道手柄在不在跑、
         * 临时加速是不是开着）。挂起时钩子会被系统摘掉，恢复后必须 rearm ——
         * 这一步不做，用户「睡一觉醒来按热键没反应」。
         */
        WM_POWERBROADCAST => {
            // 常量直接写字面量：windows crate 只导出了 SetThreadExecutionState 那部分
            const PBT_APMSUSPEND: u32 = 0x0004;
            const PBT_APMRESUMESUSPEND: u32 = 0x0007;
            const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
            let ev = match wp.0 as u32 {
                PBT_APMSUSPEND => Some(PowerEvent::Suspend),
                PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => Some(PowerEvent::Resume),
                _ => None,
            };
            if let Some(ev) = ev {
                with_shell(|s| s.power = Some(ev));
            }
            windows::Win32::Foundation::LRESULT(1) // TRUE = 已处理
        }

        /* ---- 显示器增删/分辨率变化
         *
         * 两件事：沉浸模式重新贴合；非沉浸时把窗口**钳回工作区** ——
         * 拔掉副屏后窗口可能整个落在不存在的坐标上，用户看到"程序还在跑但找不到窗口"。
         */
        WM_DISPLAYCHANGE => {
            if !with_shell(|s| s.immersion).unwrap_or(false) {
                if let Some(f) = float_hwnd() {
                    let work = win32::work_area(f);
                    let rect = win32::window_rect(f);
                    let b = layout::clamp_to_work(
                        Bounds::new(
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                        ),
                        WorkArea {
                            x: work.left,
                            y: work.top,
                            w: work.right - work.left,
                            h: work.bottom - work.top,
                        },
                    );
                    if b.x != rect.left || b.y != rect.top {
                        unsafe {
                            let _ = SetWindowPos(
                                f,
                                None,
                                b.x,
                                b.y,
                                b.w,
                                b.h,
                                SWP_NOACTIVATE | SWP_NOZORDER,
                            );
                        }
                    }
                }
            }
            if with_shell(|s| s.immersion).unwrap_or(false) {
                // 用当前比例重算（先取现有窗口比例作为近似，真实比例由应用层下次进入时提供）
                let r = with_shell(|s| win32::window_rect(s.float));
                if let Some(r) = r {
                    let w = r.right - r.left;
                    let h = r.bottom - r.top;
                    if h > 0 {
                        fit_immersion(w as f64 / h as f64);
                    }
                }
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- 叠加层：放置模式下可拖动；尺寸变化时 WebView2 跟着走
        WM_NCHITTEST if is_overlay(hwnd) && overlay_placing() => {
            windows::Win32::Foundation::LRESULT(HTCAPTION as isize)
        }
        WM_EXITSIZEMOVE if is_overlay(hwnd) => {
            // 拖完由应用层通过 overlay_center() 落盘
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_WINDOWPOSCHANGED | WM_SIZE if is_overlay(hwnd) => {
            let (w, h) = win32::client_size(hwnd);
            with_shell(|s| {
                if let Some(o) = s.overlay.as_mut() {
                    let _ =
                        o.wv.set_bounds(rect_for(RectI::new(0, 0, w.max(1), h.max(1))));
                }
            });
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }

        // ---- 圆盘尺寸变化：WebView2 跟着走
        WM_WINDOWPOSCHANGED | WM_SIZE | WM_MOVE if is_disc(hwnd) => {
            let (w, h) = win32::client_size(hwnd);
            with_shell(|s| {
                if let Some(d) = s.disc.as_mut() {
                    let _ =
                        d.wv.set_bounds(rect_for(RectI::new(0, 0, w.max(1), h.max(1))));
                }
            });
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }

        // ---- 位置/尺寸/DPI 变化：两窗联动；弹窗只同步自己的 WebView2
        WM_WINDOWPOSCHANGED | WM_SIZE | WM_MOVE => {
            let is_popup =
                with_shell(|s| s.popup.as_ref().is_some_and(|p| p.hwnd == hwnd)).unwrap_or(false);
            if is_popup {
                let (w, h) = win32::client_size(hwnd);
                with_shell(|s| {
                    if let Some(p) = s.popup.as_mut() {
                        let _ =
                            p.wv.set_bounds(rect_for(RectI::new(0, 0, w.max(1), h.max(1))));
                    }
                });
                return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            }
            let r = unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            sync();
            r
        }
        WM_DPICHANGED => {
            if let Some(h) = with_shell(|s| s.float) {
                if h == hwnd {
                    // 用系统建议的矩形，避免缩放后跑到屏幕外
                    unsafe {
                        let sug = &*(lp.0 as *const RECT);
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            sug.left,
                            sug.top,
                            sug.right - sug.left,
                            sug.bottom - sug.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
            }
            sync();
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- 外部定时器回调（不在借用内，回调里可以随便调公开 API）
        WM_APP_TICK => {
            let f = with_shell(|s| s.tick).flatten();
            if let Some(f) = f {
                f();
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- 后台线程投来的 JS：在 UI 线程上批量执行
        WM_APP_EVAL => {
            let batch: Vec<String> = EVAL_QUEUE
                .lock()
                .map(|mut q| std::mem::take(&mut *q))
                .unwrap_or_default();
            if !batch.is_empty() {
                with_shell(|s| {
                    for js in &batch {
                        let _ = s.chrome_wv.evaluate_script(js);
                    }
                });
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- 页面布局变化：重算舞台矩形 + 区域（此时没有借用）
        WM_APP_RELAYOUT => {
            let pending = with_shell(|s| {
                let p = s.relayout_pending;
                s.relayout_pending = false;
                p
            })
            .unwrap_or(false);
            if pending {
                sync();
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- chrome 窗：进入系统移动/缩放循环（借用已放下）
        WM_APP_CMD => {
            let pending = SHELL.with(|slot| match slot.try_borrow_mut() {
                Ok(mut g) => g.as_mut().map(|s| {
                    let p = s.pending;
                    s.pending = Pending::None;
                    (p, s.float)
                }),
                Err(_) => None,
            });
            if let Some((p, float)) = pending {
                match p {
                    // 自绘拖拽：不进系统模态循环，游戏抢鼠标捕获也照样跟手
                    Pending::Drag => begin_drag(),
                    Pending::Resize => start_system_loop(float, HTBOTTOMRIGHT),
                    Pending::None => {}
                }
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        // ---- chrome 窗：不激活、不擦背景（DWM 玻璃负责透明）
        WM_MOUSEACTIVATE => windows::Win32::Foundation::LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => windows::Win32::Foundation::LRESULT(1),
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let _ = hdc;
                let _ = EndPaint(hwnd, &ps);
            }
            // 舞台窗也要走默认绘制，否则无子窗区域会残留
            if with_shell(|s| s.chrome == hwnd).unwrap_or(false) {
                windows::Win32::Foundation::LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
        }

        // ---- 舞台窗显示/隐藏时带上 chrome（owned 窗只在最小化时自动跟随）
        WM_SHOWWINDOW => {
            let is_float = with_shell(|s| s.float == hwnd).unwrap_or(false);
            let visible = wp.0 != 0;
            let r = unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            if is_float {
                if let Some(c) = chrome_hwnd() {
                    unsafe {
                        let _ = ShowWindow(c, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
                    }
                }
            }
            r
        }

        WM_CLOSE => {
            // 弹窗自己关：销毁即可
            let is_popup =
                with_shell(|s| s.popup.as_ref().is_some_and(|p| p.hwnd == hwnd)).unwrap_or(false);
            if is_popup {
                with_shell(|s| s.popup = None);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return windows::Win32::Foundation::LRESULT(0);
            }
            // chrome 窗不许自己关；关舞台窗才算关浮窗
            if with_shell(|s| s.chrome == hwnd).unwrap_or(false) {
                return windows::Win32::Foundation::LRESULT(0);
            }
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_DESTROY => {
            // 只有舞台窗销毁才退出进程；登录弹窗关掉不该带走整个应用
            let is_float = with_shell(|s| s.float == hwnd).unwrap_or(false);
            if is_float {
                unsafe { PostQuitMessage(0) };
            }
            windows::Win32::Foundation::LRESULT(0)
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn frame_thickness() -> (i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER),
            GetSystemMetrics(SM_CYSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER),
        )
    }
}

/// 在 chrome 页面里执行一段 JS（设置类操作、展开 URL 条等）。
///
/// 可以从**任意线程**调用：先进队列，再让 UI 线程取出来执行
/// （WebView2 的 `ExecuteScript` 只能在创建它的线程上调用）。
pub fn eval_chrome(js: &str) {
    if let Ok(mut q) = EVAL_QUEUE.lock() {
        q.push(js.to_string());
    }
    if let Some(f) = float_hwnd() {
        unsafe {
            let _ = PostMessageW(Some(f), WM_APP_EVAL, WPARAM(0), LPARAM(0));
        }
    }
}

/// 延时后把一个 tick 投递回 UI 线程。
pub fn post_tick(delay_ms: u64) {
    let Some(f) = float_hwnd() else { return };
    let raw = f.0 as isize;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        unsafe {
            let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_APP_TICK, WPARAM(0), LPARAM(0));
        }
    });
}

/// 起一个周期心跳：每 `interval_ms` 往 UI 线程投一次 `tick`。
/// 应用层用它做「收播放状态 → 刷 UI → 置顶巡检」。
pub fn start_ticks(interval_ms: u64) {
    let Some(f) = float_hwnd() else { return };
    let raw = f.0 as isize;
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        unsafe {
            let _ = PostMessageW(Some(HWND(raw as *mut _)), WM_APP_TICK, WPARAM(0), LPARAM(0));
        }
    });
}

/// 关掉浮窗（等价于点标题栏的关闭）。
pub fn close() {
    let Some(f) = float_hwnd() else { return };
    unsafe {
        let _ = PostMessageW(Some(f), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// 定期把舞台窗重新压到最顶层（原版 2.5s 巡检；owned 的 chrome 会跟着走）。
pub fn reassert_topmost() {
    // 两个窗都无条件重新抬起：光是"标记为 topmost"不够 ——
    // 另一个 topmost 窗口后抬起时仍会盖住我们，只有重新 SetWindowPos 才抢回来。
    with_shell(|s| {
        win32::topmost(s.float);
        win32::topmost(s.chrome);
        // 弹窗（登录/扫码）必须在最后抬：否则巡检会把它重新压到浮窗下面
        if let Some(p) = s.popup.as_ref() {
            win32::topmost(p.hwnd);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runtime_versions() {
        assert_eq!(parse_version("157.0.4341.60"), Some((157, 0, 4341, 60)));
        assert_eq!(parse_version("110.0.1587.0"), Some((110, 0, 1587, 0)));
        assert_eq!(parse_version("157"), None);
        assert_eq!(parse_version("abc.0.0.0"), None);
    }

    #[test]
    fn baseline_is_major_110() {
        assert!(runtime_ok("157.0.4341.60"));
        assert!(runtime_ok("110.0.1587.0"));
        assert!(!runtime_ok("109.0.1518.78"));
        assert!(!runtime_ok("nonsense"));
    }
}

#[cfg(test)]
mod url_tests {
    use super::is_bilibili_url;

    #[test]
    fn whitelist_accepts_only_bilibili() {
        assert!(is_bilibili_url("https://passport.bilibili.com/login"));
        assert!(is_bilibili_url(
            "https://www.bilibili.com/video/BV1xx411c7mD"
        ));
        assert!(is_bilibili_url("http://b23.tv/abc"));
        assert!(!is_bilibili_url("https://evil.com/bilibili.com"));
        assert!(!is_bilibili_url("https://bilibili.com.evil.com/x"));
        assert!(!is_bilibili_url("javascript:alert(1)"));
        assert!(!is_bilibili_url("file:///C:/x"));
    }
}
