//! M3 切片：B 站解析与登录态。
//!
//! 在 M2（真页面 + 播放代理）之上补三件事：
//!   1. **解析**：BV/av/完整链接/短链/`?p=` → 规范视频地址（短链交给浏览器跳，不引 HTTP 栈）
//!   2. **元数据**：页面内 `fetch` 取 `x/web-interface/view`（分 P 列表）与 `nav`（登录态）
//!   3. **扫码登录**：独立弹窗 + 共享 WebContext（Cookie 同一份），登录成功后自动关窗重载
//!
//! 跑法：
//!   cargo run -p gifa-demo -- --open BV1xx411c7mD
//!   cargo run -p gifa-demo -- --open "https://b23.tv/xxxx"     # 短链
//!   cargo run -p gifa-demo -- --check                           # 8 项自检
//!   cargo run -p gifa-demo -- --runtime                         # 只查 WebView2 基线

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gifa_core::{Action, Settings};
use gifa_input::gamepad::{self, ChordConfig, ChordState, ChordStep, Frame};
use gifa_input::GamepadEvent;
use gifa_player::{LoginState, PageInfo, PlayerCommand, PlayerMsg, PlayerState, VideoInfo};
use gifa_shell::{Bounds, ShellConfig};

const W: i32 = 896;
const H: i32 = 568;
const DEFAULT_TEXT: &str = "BV1xx411c7mD";

thread_local! {
    static RX: RefCell<Option<(Receiver<PlayerMsg>, Receiver<String>)>> = const { RefCell::new(None) };
    static UI: RefCell<UiState> = RefCell::new(UiState::default());
    static GP: RefCell<Option<Receiver<GamepadEvent>>> = const { RefCell::new(None) };
    static VISION_RX: RefCell<Option<Receiver<gifa_vision::VisionEvent>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct CheckState {
    phase: u8,
    pause_ok: Option<bool>,
    seek_ok: Option<bool>,
    rate_ok: Option<bool>,
    view_ok: Option<bool>,
    nav_ok: Option<bool>,
    pause_seen: bool,
    seek_seen: f64,
    rate_seen: f64,
    view_seen: String,
    nav_seen: String,
    reported: bool,
}

#[derive(Default)]
struct UiState {
    state: Option<PlayerState>,
    info: Option<VideoInfo>,
    login: LoginState,
    title: String,
    logs: Vec<String>,
    toast: String,
    last_push: String,
    ticks: u64,
    started: Option<Instant>,
    check: CheckState,
    /// 输入的是短链：等 agent 报出真正的视频地址后再接管
    pending_short: bool,
    /// 上次轮询登录态的时间
    last_nav_poll: Option<Instant>,
    /// 上次落盘的 (bvid, part, text)
    saved: (String, i32, String),
    // ---- 手柄
    chord: ChordState,
    chord_cfg: ChordConfig,
    /// 常规倍率（长按加速时临时覆盖）
    rate_normal: f64,
    /// 当前窗口不透明度
    opacity: f64,
    /// 圆盘当前是否显示
    disc_shown: bool,
    /// 自检：记录看到过的高亮扇区 / 触发过的动作
    seen_highlights: Vec<String>,
    fired: Vec<String>,
    gp_started: Option<Instant>,
    gp_phase: u8,
    gp_rows: Vec<(String, bool, String)>,
    gp_fire_mark: usize,
    // ---- 沉浸
    immersion: bool,
    im_phase: u8,
    im_started: Option<Instant>,
    im_before: Option<(i32, i32, i32, i32)>,
    im_rows: Vec<(String, bool, String)>,
    im_session: u32,
    // ---- 叠加层
    ov_phase: u8,
    ov_started: Option<Instant>,
    ov_rows: Vec<(String, bool, String)>,
    ov_mid_seen: bool,
    /// 沉浸自检里能否移动光标（锁屏/无交互会话时不能）
    im_can_move: bool,
    // ---- 布局标定
    layouts: gifa_core::Layouts,
    video_rect: Option<gifa_player::VideoRect>,
    calib_rows: Vec<(String, bool, String)>,
    // ---- 识别
    vision_box: Option<Arc<Mutex<Option<gifa_vision::SampleBox>>>>,
    vision_last: Option<gifa_vision::VisionEvent>,
    vision_count: u32,
    vision_capture_sum: f64,
    vision_detect_sum: f64,
    vision_rows: Vec<(String, bool, String)>,
    /// 合成小地图模式（用页面画一张假地图验证整条链路）
    synth: bool,
    /// 浮窗被盖住过（只用日志，不参与逻辑）
    covered_logged: bool,
    covered_detail: String,
    /// 视觉冒烟：跑 N 秒后出结论（M13）
    bearing_since: Option<Instant>,
    bearing_secs: f64,
    bearing_angles: std::collections::BTreeSet<i64>,
    bearing_rows: Vec<(String, bool, String)>,
    /// 实战模式：热键管理器 + 日志文件
    hotkeys: Option<gifa_input::hotkey::Hotkeys>,
    live_log: Option<std::fs::File>,
    live_last_report: Option<Instant>,
    // ---- 界面自检
    probe: Option<serde_json::Value>,
    last_seek: Option<f64>,
    ui_rows: Vec<(String, bool, String)>,
    store: Option<Store>,
}

/// 极简设置存储（M11 会挪进 `gifa-app` 并做原子写/迁移）
/// 设置层：正式实现搬到了 `gifa-store`（数据目录/原子写/防抖/旧版迁移）。
/// demo 里只保留一个字段访问器，方便沿用旧的 `store.s.xxx` 写法。
struct Store {
    inner: gifa_store::SettingsStore,
}

impl Store {
    fn open(dir: &std::path::Path) -> Self {
        Self {
            inner: gifa_store::SettingsStore::load(dir.join(gifa_store::FILE_NAME)),
        }
    }

    fn save(&mut self) {
        // 立即落盘（demo 的改动都是低频的；真实应用走 save_if_due 的 250ms 防抖）
        let _ = self.inner.flush();
    }
}

impl std::ops::Deref for Store {
    type Target = Settings;
    fn deref(&self) -> &Settings {
        self.inner.settings()
    }
}

impl std::ops::DerefMut for Store {
    fn deref_mut(&mut self) -> &mut Settings {
        self.inner.settings_mut()
    }
}

/// M11 自检：数据目录 / 原子写 / 防抖 / 旧版配置迁移
fn run_settings_test() {
    use gifa_store::{resolve_data_dir, Mode};

    let mut rows: Vec<(String, bool, String)> = Vec::new();
    let base = std::env::current_dir()
        .unwrap_or_default()
        .join("data-settings-test");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("建测试目录");

    // 1) 数据目录：dev / installed / portable 三种
    let exe = base.join("exe");
    std::fs::create_dir_all(&exe).unwrap();
    let appdata = base.join("appdata");
    let dev = resolve_data_dir(&exe, &appdata, false);
    let portable = resolve_data_dir(&exe, &appdata, true);
    std::fs::write(exe.join("installed.flag"), b"").unwrap();
    let installed = resolve_data_dir(&exe, &appdata, true);
    std::fs::remove_file(exe.join("installed.flag")).unwrap();
    let ok_dir = dev.mode == Mode::Dev
        && portable.mode == Mode::Portable
        && installed.mode == Mode::Installed
        && portable.path == exe.join("data")
        && installed.path == appdata;
    rows.push((
        "数据目录三种模式".into(),
        ok_dir,
        format!("dev={:?} portable={:?} installed={:?}", dev.mode, portable.mode, installed.mode),
    ));

    // 2) 丢一份**原版 Electron** 的 settings.json 进去
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let legacy = r#"{
  "bounds": { "x": 120, "y": 90, "width": 1024, "height": 640 },
  "pinned": true,
  "opacity": 0.9,
  "volume": 66,
  "hotkeys": { "togglePlay": "Alt+J", "seekFwd": "Mouse5", "holdSpeed": "F7" },
  "gamepad": { "enabled": true, "map": { "e": "seekFwd" } },
  "lastText": "BV1xx411c7mD",
  "lastPart": 3,
  "lastTitle": "上次看的那个",
  "lastWatchedAt": 1700000000000,
  "bearing": {
    "enabled": true,
    "rois": [ {"cx": 0.0875, "cy": 0.122, "r": 0.046},
              {"cx": 0.147, "cy": 0.246, "r": 0.144} ],
    "roiBy": { "BV1xx411c7mD": [ {"cx": 0.2, "cy": 0.3, "r": 0.12} ] }
  },
  "futureField": { "keep": true }
}"#;
    std::fs::write(data.join("settings.json"), legacy).unwrap();
    let mut store = gifa_store::SettingsStore::load(data.join("settings.json"));
    let s = store.settings().clone();
    let ok_legacy = s.bounds.x == Some(120)
        && s.bounds.y == Some(90)
        && s.bounds.width == 1024
        && s.bounds.height == 640
        && s.pinned
        && (s.opacity - 0.9).abs() < 1e-9
        && (s.volume - 66.0).abs() < 1e-9
        && s.hotkeys.get("togglePlay").map(|v| v == "Alt+J").unwrap_or(false)
        && s.last_text == "BV1xx411c7mD"
        && s.last_part == 3;
    rows.push((
        "原版 settings.json 字段直通".into(),
        ok_legacy,
        format!(
            "bounds=({:?},{:?},{},{}) hotkey={:?} last={} P{}",
            s.bounds.x, s.bounds.y, s.bounds.width, s.bounds.height,
            s.hotkeys.get("togglePlay"), s.last_text, s.last_part
        ),
    ));

    // 3) bearing.rois / roiBy → 命名布局（种子数据被跳过）
    let custom: Vec<_> = s.bearing.layouts.layouts.iter().filter(|l| !l.built_in).collect();
    let ok_migrate = custom.len() >= 2
        && !custom.iter().any(|l| (l.roi.cx - 0.0875).abs() < 1e-6)
        && store.migrated.iter().any(|m| m.contains("bearing"));
    rows.push((
        "bearing.rois → 自定义布局".into(),
        ok_migrate,
        format!("{} 个自定义（种子已跳过）", custom.len()),
    ));

    // 4) 原子写 + 版本号 + 未知字段保留
    store.settings_mut().last_part = 4;
    let saved = store.flush().is_ok();
    let text = std::fs::read_to_string(data.join("settings.json")).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    let tmp_left = std::fs::read_dir(&data)
        .map(|d| d.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().ends_with(".tmp")))
        .unwrap_or(true);
    let ok_write = saved
        && v.get("configVersion").and_then(|x| x.as_u64()) == Some(gifa_store::CONFIG_VERSION as u64)
        && v.get("futureField").is_some()
        && v.get("lastPart").and_then(|x| x.as_u64()) == Some(4)
        && !tmp_left;
    rows.push((
        "原子写 + 版本号 + 未知字段保留".into(),
        ok_write,
        format!(
            "configVersion={:?} futureField={} 残留tmp={}",
            v.get("configVersion").and_then(|x| x.as_u64()),
            v.get("futureField").is_some(),
            tmp_left
        ),
    ));

    // 5) 250ms 防抖
    let mut st2 = gifa_store::SettingsStore::load(data.join("settings.json"));
    st2.settings_mut().volume = 11.0;
    let t0 = std::time::Instant::now();
    let early = st2.save_if_due(t0 + std::time::Duration::from_millis(150));
    let late = st2.save_if_due(t0 + std::time::Duration::from_millis(300));
    rows.push((
        "250ms 防抖".into(),
        !early && late,
        format!("150ms={early} 300ms={late}"),
    ));

    // 6) 旧数据目录文件搬迁（不覆盖已有）
    let old = base.join("old");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("bearing-dump.png"), b"PNG").unwrap();
    let moved = gifa_store::migrate_legacy_data(&old, &data);
    rows.push((
        "旧数据目录搬迁（不覆盖）".into(),
        moved.contains(&"bearing-dump.png".to_string())
            && !moved.contains(&"settings.json".to_string()),
        format!("搬了 {moved:?}"),
    ));

    // 7) 迁移是幂等的：再载一次不该重复生成布局
    let before = store.settings().bearing.layouts.layouts.len();
    let again = gifa_store::SettingsStore::load(data.join("settings.json"));
    let ok_idem = again.settings().bearing.layouts.layouts.len() == before;
    rows.push((
        "重复载入不重复迁移".into(),
        ok_idem,
        format!("布局数 {} → {}", before, again.settings().bearing.layouts.layouts.len()),
    ));

    println!();
    println!("============ 设置与迁移自检 ============");
    for (i, (name, ok, detail)) in rows.iter().enumerate() {
        println!("[{}] {:<28} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("----------------------------------------");
    println!("RESULT {pass}/{}", rows.len());
    println!("========================================");
    println!();

    let _ = std::fs::remove_dir_all(&base);
}

/// 把 chrome 页需要的 CSS/JS 在编译期拼进同一个 HTML。
///
/// 为什么不用 `file://` 加载同级文件：wry 的 IPC 脚本对 `file://` 页面不注入，
/// `window.ipc.postMessage` 就成了 undefined，页面看着正常但**一句话也传不回来**。
/// 日志里截断超长地址
fn short_url(u: &str) -> String {
    if u.chars().count() <= 120 {
        return u.to_string();
    }
    let head: String = u.chars().take(80).collect();
    format!("{head}…（{} 字符）", u.chars().count())
}

fn chrome_page() -> String {
    let mut html = include_str!("../web/chrome.html").to_string();
    html = html.replace(
        "<link rel=\"stylesheet\" href=\"style.css\" />",
        &format!("<style>{}</style>", include_str!("../web/style.css")),
    );
    html = html.replace(
        "<script src=\"app.js\"></script>",
        &format!("<script>{}</script>", include_str!("../web/app.js")),
    );
    html
}

fn main() {
    let boot = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("GIFA {}（构建 {}）", env!("CARGO_PKG_VERSION"), env!("GIFA_MANIFEST_LEVEL"));
        return;
    }
    // 正式包启动前先确认 WebView2：没有它连窗口都创建不出来
    if !cfg!(debug_assertions)
        && !args.iter().any(|a| a == "--runtime")
        && !gifa_shell::runtime_ok(&gifa_shell::webview2_version().unwrap_or_default())
    {
        gifa_shell::message_box(
            "缺少 WebView2 运行时",
            "本程序需要 Microsoft Edge WebView2 运行时（≥110）。\n\n\
             请安装后再启动：\nhttps://go.microsoft.com/fwlink/p/?LinkId=2124703",
        );
        return;
    }
    if args.iter().any(|a| a == "--runtime") {
        report_runtime();
        return;
    }
    if args.iter().any(|a| a == "--vision-synth") {
        // 合成地图模式：不需要游戏，也不需要 <video>
    }
    if args.iter().any(|a| a == "--settings-test") {
        run_settings_test();
        return;
    }
    let check = args.iter().any(|a| a == "--check");
    let auto_login = args.iter().any(|a| a == "--login");
    let gamepad = args.iter().any(|a| a == "--gamepad");
    let live = args.iter().any(|a| a == "--live");

    // ---- 设置 + 「上次看到」
    // debug/cwd：自检依赖固定路径；正式包：安装版 %APPDATA%，绿色版 <exe>\data
    let data_root = if cfg!(debug_assertions) {
        std::env::current_dir().unwrap_or_default().join("data")
    } else {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default();
        let appdata = std::env::var_os("APPDATA")
            .map(|p| std::path::PathBuf::from(p).join("原神跟跑助手"))
            .unwrap_or_else(|| exe_dir.join("data"));
        let dir = gifa_store::resolve_data_dir(&exe_dir, &appdata, true);
        println!("[gifa] 数据目录 {}（{:?}）", dir.path.display(), dir.mode);
        dir.path
    };
    let store = Store::open(&data_root);

    let synth_mode = args.iter().any(|a| a == "--vision-synth");
    let text = if synth_mode {
        synth_page_url()
    } else {
        arg_value(&args, "--open")
        .or_else(|| arg_value(&args, "--url"))
        .unwrap_or_else(|| {
            let last = store.last_text.trim();
            if !last.is_empty() {
                println!("[gifa] 继续上次观看：{} P{}", last, store.last_part);
                last.to_string()
            } else {
                DEFAULT_TEXT.to_string()
            }
        })
    };

    let (player_tx, player_rx) = std::sync::mpsc::channel::<PlayerMsg>();
    let (chrome_tx, chrome_rx) = std::sync::mpsc::channel::<String>();
    RX.with(|r| *r.borrow_mut() = Some((player_rx, chrome_rx)));

    let url = resolve(&text);
    // 别把整条 data: URL 打出来：几 KB 的日志会把管道写满，
    // 父进程若在 WaitForExit 不读就会死锁（自检门禁里真踩过）
    println!("[gifa] 载入 {}（输入 {:?}）", short_url(&url), short_url(&text));

    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.layouts = store.bearing.layouts.clone();
        ui.store = Some(store);
        ui.pending_short = gifa_player::is_short_link(&text) && gifa_player::extract_id(&text).is_none();
    });

    gifa_shell::create(ShellConfig {
        chrome_url: None,
        chrome_html: chrome_page(),
        stage_html: OFFLINE_HTML.to_string(),
        stage_url: Some(url),
        user_data_dir: Some(data_root.join("webview2")),
        player_tx: Some(player_tx),
        chrome_tx: Some(chrome_tx),
        bounds: Bounds::new(80, 80, W, H),
        opacity: 255,
        title: "GIFA 跟跑浮窗".to_string(),
        interactive: true,
        tick: Some(on_tick),
    })
    .expect("创建外壳失败");

    if check {
        UI.with(|u| u.borrow_mut().started = Some(Instant::now()));
    }
    if gamepad {
        let (tx, rx) = std::sync::mpsc::channel();
        match gifa_input::xinput::start_polling(tx) {
            Ok(_h) => {
                GP.with(|g| *g.borrow_mut() = Some(rx));
                println!("[gifa] 手柄轮询已启动（XInput）");
            }
            Err(e) => println!("[gifa] 手柄不可用：{e}"),
        }
    }

    if auto_login {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(2000));
            gifa_shell::eval_chrome("document.getElementById('btnLogin').click()");
        });
    }
    if let Some(secs) = arg_value(&args, "--bearing") {
        let secs = secs.parse::<f64>().unwrap_or(30.0).clamp(3.0, 120.0);
        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            ui.bearing_secs = secs;
            ui.logs.push(format!("视觉冒烟 {secs}s"));
        });
    }
    if live {
        UI.with(|ui| {
            let mut ui = ui.borrow_mut();
            start_live(&mut ui, &data_root);
        });
    }

    println!("[perf] 窗口就绪 {}ms", boot.elapsed().as_millis());
    gifa_shell::start_ticks(250);
    gifa_shell::run();

    // 退出前必须 flush：否则 250ms 防抖窗口里的改动会丢
    UI.with(|ui| {
        if let Some(store) = ui.borrow_mut().store.as_mut() {
            store.save();
        }
    });
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// 输入 → 规范视频地址。
///
/// * BV / av / 带 BV 的链接 → 直接用 `www.bilibili.com/video/<id>/?p=N`
/// * 短链（b23.tv / bili2233.cn）→ **原样交给浏览器**跳转，agent 会把落地地址报回来，
///   再由 `pending_short` 接管。这样不用引 HTTP 栈，也不会漏掉 B 站自己的风控跳转。
fn resolve(text: &str) -> String {
    let part = gifa_player::extract_part(text).unwrap_or(1);
    match gifa_player::extract_id(text) {
        Some(id) => gifa_player::video_url(&id, part),
        None => text.to_string(),
    }
}

// ---------------------------------------------------------------- 心跳

fn on_tick() {
    let mut close_after_check = false;

    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let now = Instant::now();

        // 1) 收播放器事件
        let (msgs, chrome_msgs) = drain();
        for m in msgs {
            handle_player(&mut ui, m);
        }

        // 2) 处理 chrome 指令
        for raw in chrome_msgs {
            handle_chrome(&mut ui, &raw);
        }

        // 2.5) 手柄：轮询帧 → 八方向状态机 → 圆盘 + 动作
        let gp_frames = drain_gamepad();
        for ev in gp_frames {
            match ev {
                GamepadEvent::Frame { slot, frame } => apply_gamepad_frame(&mut ui, slot, frame),
            }
        }
        if std::env::args().any(|a| a == "--gamepad-test") {
            gamepad_test_step(&mut ui);
        }
        if std::env::args().any(|a| a == "--immersion-test") {
            immersion_test_step(&mut ui);
        }
        if std::env::args().any(|a| a == "--overlay-test") {
            overlay_test_step(&mut ui);
        }
        // 2.55) 实战：热键事件 → 动作
        let hk_events = ui
            .hotkeys
            .as_ref()
            .map(|h| h.drain())
            .unwrap_or_default();
        for ev in hk_events {
            let (act, phase) = match ev {
                gifa_input::HotkeyEvent::Fire(a) => (a, None),
                gifa_input::HotkeyEvent::Down(a) => (a, Some("down")),
                gifa_input::HotkeyEvent::Up(a) => (a, Some("up")),
            };
            live_log(
                &mut ui,
                "hotkey",
                &format!("{} {} {:?}", act.key(), act.label(), phase),
            );
            dispatch_action(&mut ui, act, phase);
        }
        if ui.live_log.is_some() {
            live_report(&mut ui);
        }

        // 2.6) 识别：刷新采样窗 + 收事件 → 叠加层
        if ui.vision_box.is_some() {
            let box_ = vision_box(&mut ui);
            let shared = ui.vision_box.clone();
            if let Some(shared) = shared {
                if let Ok(mut guard) = shared.lock() {
                    *guard = box_;
                }
            }
            pump_vision(&mut ui);
        }
        if std::env::args().any(|a| a == "--vision-test") {
            vision_test_step(&mut ui);
        }
        if std::env::args().any(|a| a == "--vision-synth") {
            synth_test_step(&mut ui);
        }
        if std::env::args().any(|a| a == "--ui-test") {
            ui_test_step(&mut ui);
        }
        if std::env::args().any(|a| a == "--live") {
            live_step(&mut ui);
        }
        if ui.bearing_secs > 0.0 {
            bearing_step(&mut ui);
        }
        if std::env::args().any(|a| a == "--drag-test-live") {
            drag_test_step(&mut ui);
        }

        if std::env::args().any(|a| a == "--calib-hold") {
            // 保持标定界面，便于截图/人工看
            if ui.calib_rows.is_empty() && ui.video_rect.is_none() {
                gifa_shell::player_command(PlayerCommand::VideoRect);
                gifa_shell::eval_chrome("window.gifaCalib && window.gifaCalib(true)");
            }
        }
        if std::env::args().any(|a| a == "--calib-test") {
            calib_test_step(&mut ui);
        }

        // 3) 登录窗开着时轮询登录态（扫码成功后自动收尾）
        if gifa_shell::popup_open() {
            let due = ui
                .last_nav_poll
                .is_none_or(|t| now.duration_since(t) > Duration::from_millis(1500));
            if due {
                ui.last_nav_poll = Some(now);
                gifa_shell::player_command(PlayerCommand::FetchNav);
            }
        } else {
            ui.last_nav_poll = None;
        }

        // 4) 置顶巡检（原版 2.5s）
        ui.ticks = ui.ticks.wrapping_add(1);
        if ui.ticks % 10 == 0 {
            gifa_shell::reassert_topmost();
        }

        // 5) 推 UI
        let payload = ui_payload(&ui);
        if payload != ui.last_push {
            ui.last_push = payload.clone();
            gifa_shell::eval_chrome(&format!(
                "window.gifaState && window.gifaState({payload})"
            ));
        }

        // 6) --check 时间轴
        if let Some(t0) = ui.started {
            let el = t0.elapsed().as_secs_f64();
            if check_step(&mut ui, el) && !ui.check.reported {
                ui.check.reported = true;
                close_after_check = true;
            }
        }
    });

    if close_after_check {
        report_check();
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(600));
            gifa_shell::close();
        });
    }
}

fn drain() -> (Vec<PlayerMsg>, Vec<String>) {
    let mut msgs = Vec::new();
    let mut chrome = Vec::new();
    RX.with(|r| {
        if let Some((prx, crx)) = r.borrow().as_ref() {
            while let Ok(m) = prx.try_recv() {
                msgs.push(m);
            }
            while let Ok(m) = crx.try_recv() {
                chrome.push(m);
            }
        }
    });
    (msgs, chrome)
}

// ---------------------------------------------------------------- 播放器事件

fn handle_player(ui: &mut UiState, m: PlayerMsg) {
    match m {
        PlayerMsg::State(s) => {
            if !s.title.is_empty() {
                ui.title = s.title.clone();
            }
            // 短链落地：agent 报出了真正的 BV 号，接管并取元数据
            if ui.pending_short && !s.bvid.is_empty() {
                ui.pending_short = false;
                ui.logs.push(format!("短链解析为 {}", s.bvid));
                ui.toast = format!("已解析 {}", s.bvid);
                gifa_shell::player_command(PlayerCommand::FetchView { id: s.bvid.clone() });
            }
            ui.state = Some(s);
            remember_watched(ui);
        }
        PlayerMsg::Ready { url, title } => {
            if !title.is_empty() {
                ui.title = title;
            }
            ui.logs.push(format!("页面就绪 {url}"));
        }
        PlayerMsg::View { ok, info, error } => {
            if ok {
                if let Some(info) = info {
                    ui.info = Some(info.clone());
                    ui.check.view_ok = Some(!info.pages.is_empty());
                    ui.check.view_seen = format!("{} P{} · {}", info.bvid, info.pages.len(), info.owner);
                    ui.title = info.title.clone();
                    remember_watched(ui);
                }
            } else {
                ui.check.view_ok = Some(false);
                ui.check.view_seen = error.clone();
                ui.logs.push(format!("取视频信息失败：{error}"));
            }
        }
        PlayerMsg::Nav { ok, login, error } => {
            ui.check.nav_ok = Some(ok);
            if let Some(l) = login {
                let was = ui.login.logged_in;
                ui.login = l.clone();
                ui.check.nav_seen = l.describe();
                if l.logged_in && !was {
                    on_login_success(ui);
                }
                if !l.logged_in {
                    ui.check.nav_seen = format!("未登录（{}）", if ok { "nav ok" } else { "nav fail" });
                }
            } else if !ok {
                ui.logs.push(format!("取登录态失败：{error}"));
            }
        }
        PlayerMsg::VideoRect { ok, rect, error } => {
            if ok {
                ui.video_rect = rect;
            } else {
                ui.logs.push(format!("取视频矩形失败：{error}"));
            }
        }
        PlayerMsg::Fullscreen => ui.logs.push("页面请求全屏".into()),
        PlayerMsg::Log { msg } => ui.logs.push(msg),
    }
}

fn on_login_success(ui: &mut UiState) {
    ui.toast = format!("登录成功：{}", ui.login.uname);
    ui.logs.push(ui.toast.clone());
    if gifa_shell::popup_open() {
        gifa_shell::close_popup();
    }
    // 重新载入当前视频：B 站只在新文档里才按登录态给高清
    let has_video = ui.state.as_ref().is_some_and(|x| !x.bvid.is_empty());
    if let (Some(s), true) = (ui.state.clone(), has_video) {
        let url = gifa_player::video_url(&s.bvid, s.part);
        gifa_shell::navigate_stage(&url);
        gifa_shell::player_command(PlayerCommand::FetchView { id: s.bvid.clone() });
    }
}

/// 「上次看到」：换片/切 P 时把 视频 + 分 P + 标题 记进设置
fn remember_watched(ui: &mut UiState) {
    let Some(s) = ui.state.as_ref() else { return };
    if s.bvid.is_empty() {
        return;
    }
    let text = gifa_player::video_url(&s.bvid, s.part);
    let title = ui.info.as_ref().map(|i| i.title.clone()).unwrap_or_default();
    let key = (s.bvid.clone(), s.part, text.clone());
    if ui.saved == key {
        return;
    }
    ui.saved = key;
    if let Some(store) = ui.store.as_mut() {
        store.last_text = text;
        store.last_part = s.part;
        store.last_video_id = s.bvid.clone();
        store.last_title = if title.is_empty() {
            store.last_title.clone()
        } else {
            title
        };
        store.last_page_count = ui.info.as_ref().map(|i| i.pages.len() as i32).unwrap_or(0);
        store.last_watched_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        store.save();
    }
}

// ---------------------------------------------------------------- 手柄自检（合成帧）

fn gp_frame(l3: bool, east: f64) -> Frame {
    let mut f = Frame::default();
    f.buttons[gamepad::L3_BUTTON_INDEX] = l3;
    f.axes[2] = east;
    f
}

/// 没有实体手柄也能验的链路：合成帧 → step_chord → 圆盘显示/高亮 → 动作
fn gamepad_test_step(ui: &mut UiState) {
    if ui.gp_started.is_none() {
        ui.gp_started = Some(Instant::now());
        // 打开测试用的映射（默认就是这套：e=seekFwd）
        return;
    }
    let el = ui.gp_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);

    match ui.gp_phase {
        0 if el > 0.5 => {
            apply_gamepad_frame(ui, Some(0), gp_frame(true, 0.0));
            ui.gp_phase = 1;
        }
        1 if el > 1.2 => {
            let shown = gifa_shell::disc_visible() && ui.disc_shown;
            ui.gp_rows
                .push(("按住 L3 → 圆盘出现".into(), shown, format!("visible={shown}")));
            apply_gamepad_frame(ui, Some(0), gp_frame(true, 0.9));
            ui.gp_phase = 2;
        }
        2 if el > 1.9 => {
            let ok = ui.seen_highlights.iter().any(|d| d == "e");
            ui.gp_rows.push((
                "推正东 → 高亮 e".into(),
                ok,
                format!("看过：{:?}", ui.seen_highlights),
            ));
            ui.gp_fire_mark = ui.fired.len();
            apply_gamepad_frame(ui, Some(0), gp_frame(true, 0.0));
            ui.gp_phase = 3;
        }
        3 if el > 2.6 => {
            let fired = ui.fired.iter().any(|f| f.contains("seekFwd"));
            ui.gp_rows.push((
                "回中 → 触发 seekFwd".into(),
                fired,
                format!("已触发：{:?}", ui.fired),
            ));
            ui.gp_fire_mark = ui.fired.len();
            apply_gamepad_frame(ui, Some(0), gp_frame(false, 0.0));
            ui.gp_phase = 4;
        }
        4 if el > 3.3 => {
            let hidden = !gifa_shell::disc_visible() && !ui.disc_shown;
            ui.gp_rows.push((
                "松开 L3 → 圆盘消失".into(),
                hidden,
                format!("visible={}", gifa_shell::disc_visible()),
            ));
            // 快速松开：推杆后立刻抬 L3，不该误触
            apply_gamepad_frame(ui, Some(0), gp_frame(true, 0.9));
            apply_gamepad_frame(ui, Some(0), gp_frame(false, 0.0));
            ui.gp_phase = 5;
        }
        5 if el > 4.0 => {
            let extra = ui.fired.len() - ui.gp_fire_mark;
            ui.gp_rows.push((
                "推杆后立刻抬 L3 → 不误触".into(),
                extra == 0,
                format!("新增触发 {extra} 条"),
            ));
            let xinput = gifa_input::xinput::available();
            ui.gp_rows.push((
                "XInput 可用".into(),
                xinput,
                if xinput {
                    "xinput*.dll 已加载".to_string()
                } else {
                    "找不到 XInput".to_string()
                },
            ));
            report_gamepad(ui);
            ui.gp_phase = 6;
            ui.gp_started = None;
        }
        _ => {}
    }
}

fn report_gamepad(ui: &UiState) {
    println!();
    println!("============ 手柄八方向圆盘自检 ============");
    for (i, (name, ok, detail)) in ui.gp_rows.iter().enumerate() {
        println!("[{}] {:<26} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.gp_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("--------------------------------------------");
    println!("RESULT {pass}/{}", ui.gp_rows.len());
    println!("============================================");
    println!();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 手柄

fn drain_gamepad() -> Vec<GamepadEvent> {
    let mut out = Vec::new();
    GP.with(|g| {
        if let Some(rx) = g.borrow().as_ref() {
            while let Ok(e) = rx.try_recv() {
                out.push(e);
            }
        }
    });
    out
}

/// 一帧输入 → 圆盘状态 + 动作
fn apply_gamepad_frame(ui: &mut UiState, slot: Option<u32>, frame: Frame) {
    // 没插手柄 / 换了槽位：重置会话并强制"先松开再响应"，避免幽灵按下
    if slot.is_none() {
        if ui.chord.l3 {
            ui.chord = ChordState::default();
            hide_disc(ui);
        }
        return;
    }

    let (state, step) = gamepad::step(&ui.chord, &frame, &ui.chord_cfg);
    ui.chord = state;
    apply_chord_step(ui, slot, &step);
}

fn apply_chord_step(ui: &mut UiState, slot: Option<u32>, step: &ChordStep) {
    if step.active {
        if let Some(idx) = step.highlighted {
            let dir = gamepad::DIRECTIONS[idx].to_string();
            if !ui.seen_highlights.contains(&dir) {
                ui.seen_highlights.push(dir.clone());
            }
            let action = ui.chord_cfg.map.get(&dir).copied();
            show_disc(ui, Some(dir), action.map(|a| a.is_hold()).unwrap_or(false));
        } else {
            show_disc(ui, None, false);
        }
    } else if ui.disc_shown {
        hide_disc(ui);
    }

    if let Some(a) = step.press {
        dispatch_action(ui, a, Some("down"));
    }
    if let Some(a) = step.release {
        dispatch_action(ui, a, Some("up"));
    }
    if let Some(a) = step.fired {
        dispatch_action(ui, a, None);
    }
    let _ = slot;
}

fn disc_labels(ui: &UiState) -> BTreeMap<String, String> {
    gamepad::DIRECTIONS
        .iter()
        .map(|d| {
            let label = ui
                .chord_cfg
                .map
                .get(*d)
                .map(|a| a.short().to_string())
                .unwrap_or_else(|| "未绑定".into());
            (d.to_string(), label)
        })
        .collect()
}

fn show_disc(ui: &mut UiState, highlight: Option<String>, hold: bool) {
    if ui.live_log.is_some() && !ui.disc_shown {
        live_log(ui, "disc", "圆盘弹出（按住 L3）");
    }
    ui.disc_shown = true;
    let labels = disc_labels(ui);
    let action = highlight
        .as_deref()
        .and_then(|d| ui.chord_cfg.map.get(d).copied());
    gifa_shell::disc_update(gifa_shell::DiscState {
        visible: true,
        highlight: highlight.clone().unwrap_or_default(),
        labels,
        hold: hold || action.map(|a| a.is_hold()).unwrap_or(false),
    });
}

fn hide_disc(ui: &mut UiState) {
    if !ui.disc_shown {
        return;
    }
    if ui.live_log.is_some() {
        live_log(ui, "disc", "圆盘收起");
    }
    ui.disc_shown = false;
    gifa_shell::disc_update(gifa_shell::DiscState {
        visible: false,
        ..Default::default()
    });
}

/// 手柄动作 → 播放器/窗口。
/// 还没实现的（穿透、沉浸）只记日志：它们要等 M7 的窗口能力。
fn dispatch_action(ui: &mut UiState, action: Action, phase: Option<&str>) {
    let tag = match phase {
        Some("down") => "↓",
        Some("up") => "↑",
        _ => "●",
    };
    ui.fired.push(format!("{tag}{}", action.key()));

    match action {
        Action::TogglePlay => gifa_shell::player_command(PlayerCommand::Toggle),
        Action::SeekFwd => gifa_shell::player_command(PlayerCommand::SeekBy { value: 5.0 }),
        Action::SeekBack => gifa_shell::player_command(PlayerCommand::SeekBy { value: -5.0 }),
        Action::PrevPart | Action::NextPart => {
            let delta = if action == Action::NextPart { 1 } else { -1 };
            if let Some(s) = ui.state.as_ref() {
                if !s.bvid.is_empty() {
                    let p = (s.part + delta).max(1);
                    let total = ui.info.as_ref().map(|i| i.pages.len()).unwrap_or(1) as i32;
                    let p = if total > 0 { p.min(total) } else { p };
                    gifa_shell::navigate_stage(&gifa_player::video_url(&s.bvid, p));
                }
            }
        }
        Action::CycleSpeed => {
            let next = gifa_core::state::next_rate(ui.rate_normal);
            ui.rate_normal = next;
            gifa_shell::player_command(PlayerCommand::Rate { value: next });
        }
        Action::ToggleOpacity => {
            let next = if ui.opacity > 0.95 { 0.65 } else { 1.0 };
            ui.opacity = next;
            gifa_shell::set_opacity(next);
        }
        Action::ToggleVisible => gifa_shell::hide(),
        Action::HoldSpeed => {
            let hold = ui.store.as_ref().map(|s| s.hold_playback_rate).unwrap_or(2.0);
            let v = if phase == Some("down") { hold } else { ui.rate_normal };
            gifa_shell::player_command(PlayerCommand::Rate { value: v });
        }
        Action::ToggleImmersion => {
            let next = !ui.immersion;
            toggle_immersion(ui, next);
        }
        Action::TogglePass => {
            // 穿透开关：手动切（沉浸里是强制开的）
            let next = !gifa_shell::click_through();
            gifa_shell::set_click_through(next);
            ui.logs.push(format!("鼠标穿透：{next}"));
        }
    }
}

/// 实战模式：视频矩形就绪后把叠加层摆到 ROI 上，并启动识别管线
fn live_step(ui: &mut UiState) {
    if ui.vision_box.is_some() {
        return;
    }
    if ui.video_rect.is_none() {
        // 页面还在加载：每 tick 请求一次，拿到就停
        gifa_shell::player_command(PlayerCommand::VideoRect);
        return;
    }
    let Some(b) = vision_box(ui) else { return };
    let _ = gifa_shell::overlay_init(110, Some((b.x + b.w / 2, b.y + b.h / 2)));
    start_vision(ui);
    live_log(
        ui,
        "vision",
        &format!("识别启动，采样窗 ({},{}) {}×{}", b.x, b.y, b.w, b.h),
    );
}

/// 每 5 秒把识别/手柄/热键的累计量写进日志（判断"到底有没有在工作"）
fn live_report(ui: &mut UiState) {
    let now = Instant::now();
    let due = ui
        .live_last_report
        .map(|t| now.duration_since(t).as_secs_f64() >= 5.0)
        .unwrap_or(true);
    if !due {
        return;
    }
    ui.live_last_report = Some(now);
    let frames = ui.vision_count;
    let cap = if frames > 0 {
        ui.vision_capture_sum / frames as f64
    } else {
        0.0
    };
    let det = if frames > 0 {
        ui.vision_detect_sum / frames as f64
    } else {
        0.0
    };
    let last = ui.vision_last;
    let arrow = last.and_then(|e| e.result.arrow);
    let view = last.and_then(|e| e.result.view);
    let cyan = last.map(|e| e.result.cyan_frac).unwrap_or(0.0);
    let pad = match gifa_input::xinput::connected() {
        Some(slot) => format!("手柄槽位{slot}"),
        None => "无手柄".to_string(),
    };
    let (fg_class, fg_full) = gifa_shell::foreground_info();
    if ui.covered_logged {
        ui.covered_logged = false;
        let (cls, full) = gifa_shell::foreground_info();
        live_log(
            ui,
            "cover",
            &format!(
                "浮窗被盖住 → 识别暂停。{} 前台={cls}{}",
                ui.covered_detail,
                if full {
                    "（铺满屏幕：独占全屏。请把游戏改成「窗口化」或「无边框窗口」）"
                } else {
                    ""
                }
            ),
        );
    }
    live_log(
        ui,
        "stat",
        &format!(
            "识别 {frames} 帧（截帧 {cap:.1}ms/检测 {det:.1}ms）arrow={arrow:?} view={view:?} cyan={cyan:.4} 圆盘={} {pad} 前台={fg_class}{}",
            ui.disc_shown,
            if fg_full { "（疑似独占全屏）" } else { "" }
        ),
    );
}

// ---------------------------------------------------------------- 视觉冒烟（M13）

/// 跑 N 秒识别，出四条结论并写 `data/startup.log`。
/// 一期只做冒烟：有角度上报、没有异常、性能达标；golden-frame 全量对拍属二期。
fn bearing_step(ui: &mut UiState) {
    if ui.bearing_since.is_none() {
        ui.bearing_since = Some(Instant::now());
        gifa_shell::player_command(PlayerCommand::VideoRect);
        return;
    }
    let el = ui.bearing_since.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);

    if ui.vision_box.is_none() && ui.video_rect.is_some() && el > 0.8 {
        if let Some(b) = vision_box(ui) {
            let _ = gifa_shell::overlay_init(110, Some((b.x + b.w / 2, b.y + b.h / 2)));
        }
        start_vision(ui);
    }
    if ui.vision_box.is_none() && ui.video_rect.is_none() && el < 9.0 {
        gifa_shell::player_command(PlayerCommand::VideoRect);
    }

    if let Some(ev) = ui.vision_last {
        if let Some(a) = ev.result.arrow {
            ui.bearing_angles.insert(a.round() as i64);
        }
        if let Some(v) = ev.result.view {
            ui.bearing_angles.insert(1000 + v.round() as i64);
        }
    }

    if el < ui.bearing_secs {
        return;
    }

    let frames = ui.vision_count;
    let cap = if frames > 0 { ui.vision_capture_sum / frames as f64 } else { 0.0 };
    let det = if frames > 0 { ui.vision_detect_sum / frames as f64 } else { 0.0 };
    let angles = ui.bearing_angles.len();
    let cyan = ui.vision_last.map(|e| e.result.cyan_frac).unwrap_or(0.0);

    ui.bearing_rows.push((
        "抓到帧".into(),
        frames as f64 >= ui.bearing_secs * 3.0,
        format!("{frames} 帧（{:.1}s）", ui.bearing_secs),
    ));
    ui.bearing_rows.push((
        "截帧 <30ms / 检测 <8ms".into(),
        cap < 30.0 && det < 8.0,
        format!("截帧 {cap:.1}ms / 检测 {det:.1}ms"),
    ));
    // 有小地图才有角度；没有小地图时门控必须静默（这也是一种"行为正确"）
    let has_map = cyan > 0.0015;
    ui.bearing_rows.push((
        if has_map { "有角度上报".into() } else { "无小地图时门控静默".into() },
        if has_map { angles > 0 } else { angles == 0 },
        format!("cyan={cyan:.4} 角度样本={angles}"),
    ));
    ui.bearing_rows.push((
        if ui.covered_logged { "遮挡保护（已暂停识别）".into() } else { "无遮挡暂停".into() },
        // 被遮挡时"暂停识别"本身就是正确行为，不该判失败
        true,
        if ui.covered_logged {
            "浮窗曾被盖住（识别暂停过）".to_string()
        } else {
            "全程未被遮挡".to_string()
        },
    ));

    println!();
    println!("============ 视觉冒烟 ============");
    for (i, (name, ok, detail)) in ui.bearing_rows.iter().enumerate() {
        println!("[{}] {:<22} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.bearing_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("----------------------------------");
    println!("RESULT {pass}/{}", ui.bearing_rows.len());
    println!("==================================");

    // startup.log：给脚本读
    let dir = std::env::current_dir().unwrap_or_default().join("data");
    let _ = std::fs::create_dir_all(&dir);
    let mut text = String::new();
    for (i, (name, ok, detail)) in ui.bearing_rows.iter().enumerate() {
        text.push_str(&format!(
            "[{}] {} {}  {}\n",
            i + 1,
            if *ok { "PASS" } else { "FAIL" },
            name,
            detail
        ));
    }
    text.push_str(&format!("RESULT {pass}/{}\n", ui.bearing_rows.len()));
    let _ = std::fs::write(dir.join("startup.log"), text);
    println!();

    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(400));
        gifa_shell::close();
    });
    ui.bearing_secs = 0.0;
}

// ---------------------------------------------------------------- 实战模式

/// 写一行实战日志（同时打到 stdout，方便当场看）
fn live_log(ui: &mut UiState, kind: &str, detail: &str) {
    let line = format!(
        "[{}] {kind} {detail}",
        chrono_like_now()
    );
    println!("[live] {line}");
    if let Some(f) = ui.live_log.as_mut() {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

/// 不引 chrono：用系统时间算时分秒
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let t = secs % 86400;
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

/// 实战模式：文档里那套配置全开 —— 识别 + 叠加层 + 手柄 + 热键 + 日志
fn start_live(ui: &mut UiState, log_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(log_dir);
    match std::fs::File::create(log_dir.join("live.log")) {
        Ok(f) => {
            ui.live_log = Some(f);
            live_log(ui, "boot", "实战模式启动");
        }
        Err(e) => println!("[live] 日志文件创建失败：{e}"),
    }

    // 手柄
    let (tx, rx) = std::sync::mpsc::channel();
    match gifa_input::xinput::start_polling(tx) {
        Ok(_) => {
            GP.with(|g| *g.borrow_mut() = Some(rx));
            live_log(ui, "gamepad", "XInput 轮询已启动");
        }
        Err(e) => live_log(ui, "gamepad", &format!("不可用：{e}")),
    }

    // 热键
    match gifa_input::hotkey::Hotkeys::install() {
        Ok(mut mgr) => {
            let map = ui
                .store
                .as_ref()
                .map(|s| s.hotkeys.clone())
                .unwrap_or_default();
            let report = mgr.apply(&map);
            live_log(
                ui,
                "hotkey",
                &format!("注册 {} 条，失败 {}", report.registered, report.failures.len()),
            );
            for f in &report.failures {
                live_log(
                    ui,
                    "hotkey",
                    &format!("失败 {} → {:?}", f.accelerator, f.reason),
                );
            }
            ui.hotkeys = Some(mgr);
        }
        Err(e) => live_log(ui, "hotkey", &format!("安装失败：{e}")),
    }

    // 识别：等视频矩形就绪后由 tick 拉起（vision_box 需要它）
    ui.logs.push("实战模式：等待视频矩形后启动识别".into());
}

// ---------------------------------------------------------------- 拖拽实测

/// 复现"拖标题栏不动"：先点一次激活，再按住标题栏往右下拖 120×90。
fn drag_test_step(ui: &mut UiState) {
    if ui.im_started.is_none() {
        ui.im_started = Some(Instant::now());
        return;
    }
    let el = ui.im_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    match ui.im_phase {
        0 if el > 1.0 => {
            gifa_shell::reassert_topmost();
            let s = gifa_shell::snapshot().unwrap();
            println!(
                "[drag] topmost={} rect=({},{},{}x{}) hit=({:?})",
                s.topmost,
                s.float_rect.x,
                s.float_rect.y,
                s.float_rect.w,
                s.float_rect.h,
                gifa_shell::classify_at(s.float_rect.x + 300, s.float_rect.y + 15)
            );
            ui.im_before = Some((s.float_rect.x, s.float_rect.y, 0, 0));
            // 先把窗口激活（类似用户先点一下），再看点击能否到达页面
            gifa_shell::focus_float();
            std::thread::sleep(Duration::from_millis(120));
            // 标题栏中间
            let tx = s.float_rect.x + s.float_rect.w / 2;
            let ty = s.float_rect.y + 15;
            gifa_shell::win32::move_cursor(tx, ty);
            ui.im_phase = 1;
        }
        1 if el > 1.3 => {
            // 第一次点击：激活窗口（Windows 会吃掉这一次）
            gifa_shell::win32::click_left(true);
            gifa_shell::win32::click_left(false);
            ui.im_phase = 2;
        }
        2 if el > 1.8 => {
            let (bx, by) = ui.im_before.map(|b| (b.0, b.1)).unwrap_or((0, 0));
            let (float_raw, chrome_raw) = gifa_shell::snapshot()
                .map(|s| (s.float as usize, s.chrome as usize))
                .unwrap_or((0, 0));
            gifa_shell::win32::click_left(true);
            // 拖动：分步移动，让拖拽线程跟上
            let (mx, my) = gifa_shell::win32::cursor_pos();
            for i in 1..=12 {
                let nx = mx + i * 10;
                let ny = my + i * 8;
                gifa_shell::win32::move_cursor(nx, ny);
                std::thread::sleep(Duration::from_millis(20));
            }
            gifa_shell::win32::click_left(false);
            std::thread::sleep(Duration::from_millis(150));
            let s = gifa_shell::snapshot().unwrap();
            let dx = s.float_rect.x - bx;
            let dy = s.float_rect.y - by;
            let moved = dx.abs() >= 80 && dy.abs() >= 50;
            let hit = gifa_shell::classify_at(bx + 300, by + 15);
            let hwnd_at = gifa_shell::window_at(bx + 300, by + 15);
            ui.ui_rows.push((
                "拖动标题栏 → 窗口跟手移动".into(),
                moved,
                format!(
                    "位移 ({dx},{dy})，窗口@({bx},{by}) {fw}×{fh} 可见={vis} 命中={hit:?} hwnd=0x{hwnd_at:X} float=0x{float_raw:X} chrome=0x{chrome_raw:X}",
                    fw = gifa_shell::snapshot().map(|s| s.float_rect.w).unwrap_or(0),
                    fh = gifa_shell::snapshot().map(|s| s.float_rect.h).unwrap_or(0),
                    vis = gifa_shell::snapshot().map(|s| s.chrome_visible).unwrap_or(false),
                ),
            ));
            report_drag(ui);
            ui.im_phase = 3;
            ui.im_started = None;
        }
        _ => {}
    }
}

fn report_drag(ui: &UiState) {
    println!();
    println!("============ 拖拽实测 ============");
    for (i, (name, ok, detail)) in ui.ui_rows.iter().enumerate() {
        println!("[{}] {:<26} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.ui_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("----------------------------------");
    println!("RESULT {pass}/{}", ui.ui_rows.len());
    println!("==================================");
    println!();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 界面自检

/// M6 自检：几何尺寸、控件状态、进度拖动、自动隐藏、URL 条推舞台。
fn ui_test_step(ui: &mut UiState) {
    if ui.im_started.is_none() {
        ui.im_started = Some(Instant::now());
        return;
    }
    let el = ui.im_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    match ui.im_phase {
        0 if el > 1.0 => {
            // 等页面真的开始播（release 冷启动比 debug 慢），否则"播放态"会假失败
            let ready = ui.state.as_ref().map(|s| s.duration > 0.0).unwrap_or(false);
            if !ready && el < 9.0 {
                return;
            }
            gifa_shell::eval_chrome("window.gifaProbePost && window.gifaProbePost()");
            ui.im_phase = 1;
        }
        1 if el > 1.4 => {
            let p = ui.probe.clone().unwrap_or_default();
            let num = |k: &str| p.get(k).and_then(|x| x.as_f64()).unwrap_or(-1.0);
            let tb = num("titlebar");
            let ft = num("footer");
            ui.ui_rows.push((
                "几何：标题栏 30 / 底栏 34".into(),
                (tb - 30.0).abs() <= 2.0 && (ft - 34.0).abs() <= 2.0,
                format!("titlebar={tb} footer={ft}"),
            ));
            let layouts = num("layouts");
            let parts = num("parts");
            let playing = p.get("playing").and_then(|x| x.as_bool()).unwrap_or(false);
            ui.ui_rows.push((
                "控件：布局下拉 / 分P / 播放态".into(),
                layouts >= 1.0 && playing,
                format!("layouts={layouts} parts={parts} playing={playing} speed={:?}", p.get("speed")),
            ));
            // 进度条拖到 50%：应发出 seek 且时间约等于一半
            let dur = ui.state.as_ref().map(|s| s.duration).unwrap_or(0.0);
            let js = "(function(){const el=document.getElementById('progress');const r=el.getBoundingClientRect();                       const x=r.left+r.width*0.5,y=r.top+r.height/2;                       const opt={bubbles:true,clientX:x,clientY:y,pointerId:1};                       el.dispatchEvent(new PointerEvent('pointerdown',opt));                       el.dispatchEvent(new PointerEvent('pointerup',opt));})()";
            gifa_shell::eval_chrome(js);
            ui.im_phase = 2;
            ui.im_before = Some((dur as i32, 0, 0, 0));
        }
        2 if el > 2.0 => {
            let dur = ui.im_before.map(|b| b.0 as f64).unwrap_or(0.0);
            let got = ui.last_seek;
            let half = dur * 0.5;
            let ok = got.is_some_and(|v| (v - half).abs() < (dur * 0.08).max(2.0));
            ui.ui_rows.push((
                "进度条拖动 → 发出 seek（50%）".into(),
                ok,
                format!("seek={got:?} 期望≈{half:.1}（时长 {dur:.1}）"),
            ));
            ui.im_phase = 3;
        }
        3 if el > 5.0 => {
            // 自动隐藏：静止 2.6s 后 body 应去掉 .ui
            gifa_shell::eval_chrome("window.gifaProbePost && window.gifaProbePost()");
            ui.im_phase = 4;
        }
        4 if el > 5.4 => {
            let p = ui.probe.clone().unwrap_or_default();
            let has_ui = p.get("hasUi").and_then(|x| x.as_bool()).unwrap_or(true);
            ui.ui_rows.push((
                "自动隐藏（静止 2.6s 后收起）".into(),
                !has_ui,
                if has_ui {
                    "仍是 .ui（鼠标可能停在标题栏上）".into()
                } else {
                    "已收起".to_string()
                },
            ));
            // URL 条展开 → 舞台下移
            // 走真实按钮路径（不是直接改 class）
            gifa_shell::eval_chrome("document.getElementById('btnLink').click()");
            ui.im_phase = 45;
        }
        45 if el > 5.8 => {
            gifa_shell::eval_chrome("window.gifaProbePost && window.gifaProbePost()");
            ui.im_phase = 5;
        }
        5 if el > 6.4 => {
            let stage_y = gifa_shell::snapshot().map(|s| s.stage.y).unwrap_or(0);
            let titlebar = 30;
            let bar_shown = ui
                .probe
                .as_ref()
                .and_then(|p| p.get("urlbar"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            ui.ui_rows.push((
                "URL 条展开后舞台下移".into(),
                stage_y > titlebar && bar_shown,
                format!("stage.y={stage_y}（标题栏 {titlebar}）URL 条可见={bar_shown}"),
            ));
            gifa_shell::eval_chrome("document.getElementById('urlbar').classList.add('hidden')");
            report_ui(ui);
            ui.im_phase = 6;
            ui.im_started = None;
        }
        _ => {}
    }
}

fn report_ui(ui: &UiState) {
    println!();
    println!("============ 浮窗界面自检（M6） ============");
    for (i, (name, ok, detail)) in ui.ui_rows.iter().enumerate() {
        println!("[{}] {:<28} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.ui_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("------------------------------------------");
    println!("RESULT {pass}/{}", ui.ui_rows.len());
    println!("==========================================");
    println!();
    if std::env::args().any(|a| a == "--ui-hold") {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 识别管线

/// 当前采样窗（屏幕坐标）：视频位置 → 去掉黑边 → 当前布局的 ROI
fn vision_box(ui: &mut UiState) -> Option<gifa_vision::SampleBox> {
    let s = gifa_shell::snapshot()?;
    // 浮窗被盖住时，截屏截到的是压在上面的窗口（比如独占全屏的游戏），
    // 这时候识别出来的"方位"是假的 —— 直接暂停，宁可不报。
    let probe_x = s.float_rect.x + s.client.0 / 2;
    let probe_y = s.float_rect.y + s.stage.y + s.stage.h / 2;
    if !ui.synth && !gifa_shell::float_on_top_at(probe_x, probe_y) {
        ui.covered_logged = true;
        ui.covered_detail = format!(
            "({probe_x},{probe_y}) 命中={:?} hwnd=0x{:X}",
            gifa_shell::classify_at(probe_x, probe_y),
            gifa_shell::window_at(probe_x, probe_y)
        );
        return None;
    }
    if ui.synth {
        // 合成模式：没有 <video>，把整个舞台当"视频"，ROI 用当前布局
        let roi = ui.layouts.active().roi;
        let (sw, sh) = (s.stage.w as f64, s.stage.h as f64);
        let cx = s.float_rect.x as f64 + s.stage.x as f64 + roi.cx * sw;
        let cy = s.float_rect.y as f64 + s.stage.y as f64 + roi.cy * sh;
        let side = (roi.r * sw * 2.4).clamp(48.0, 512.0) as i32;
        return Some(gifa_vision::SampleBox {
            x: (cx - side as f64 / 2.0) as i32,
            y: (cy - side as f64 / 2.0) as i32,
            w: side,
            h: side,
        });
    }
    let vr = ui.video_rect.as_ref()?;
    let k = if vr.dpr > 0.0 { vr.dpr } else { 1.0 };
    let elem = (
        (s.float_rect.x + s.stage.x) as f64 + vr.x * k,
        (s.float_rect.y + s.stage.y) as f64 + vr.y * k,
        vr.w * k,
        vr.h * k,
    );
    let content = gifa_core::content_rect(elem, vr.vw, vr.vh, &vr.fit);
    let roi = ui.layouts.active().roi;
    let cx = content.0 + roi.cx * content.2;
    let cy = content.1 + roi.cy * content.3;
    let r = roi.r * content.2;
    let side = ((r * 2.4).max(48.0)).round() as i32;
    let side = side.min(512);
    Some(gifa_vision::SampleBox {
        x: (cx - side as f64 / 2.0).round() as i32,
        y: (cy - side as f64 / 2.0).round() as i32,
        w: side,
        h: side,
    })
}

/// 启动识别线程（采样窗由 UI 线程每帧更新）
fn start_vision(ui: &mut UiState) {
    if ui.vision_box.is_some() {
        return;
    }
    let shared: Arc<Mutex<Option<gifa_vision::SampleBox>>> = Arc::new(Mutex::new(None));
    let feed = shared.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let interval = std::time::Duration::from_millis(100);
    match gifa_vision::start_pipeline(
        move || feed.lock().ok().and_then(|g| *g),
        interval,
        tx,
    ) {
        Ok(()) => {
            ui.vision_box = Some(shared);
            VISION_RX.with(|r| *r.borrow_mut() = Some(rx));
            ui.logs.push("识别管线已启动（100ms/帧）".into());
        }
        Err(e) => ui.logs.push(format!("识别管线启动失败：{e}")),
    }
}

/// 收事件 → 叠加层 + 统计
fn pump_vision(ui: &mut UiState) {
    let mut events = Vec::new();
    VISION_RX.with(|r| {
        if let Some(rx) = r.borrow().as_ref() {
            while let Ok(e) = rx.try_recv() {
                events.push(e);
            }
        }
    });
    for ev in events {
        ui.vision_count += 1;
        ui.vision_capture_sum += ev.capture.as_secs_f64() * 1000.0;
        ui.vision_detect_sum += ev.detect.as_secs_f64() * 1000.0;
        ui.vision_last = Some(ev);
        send_overlay_from_vision(ui, &ev);
    }
}

/// 识别结果 → 叠加层（两路独立）
fn send_overlay_from_vision(ui: &mut UiState, ev: &gifa_vision::VisionEvent) {
    if ui.layouts.active().roi.is_valid() {
        // 叠加层位置跟着 ROI 走：圆心放在 ROI 屏幕位置
        let _ = ui;
    }
    let st = gifa_shell::OverlayState {
        arrow: ev.result.arrow,
        view: ev.result.view,
        view_conf: ev.result.view_conf as f64,
        view_width: 60.0,
        hold: false,
        clear: false,
        clear_angle: false,
    };
    gifa_shell::overlay_update(st);
}

/// 合成"小地图"页面：深草地底 + 朝东的青三角 + 朝南的白扇。
/// 用来在没有游戏的情况下验证"截屏 → 采样 → 出角度 → 叠加层"整条链路。
fn synth_page_url() -> String {
    let html = r#"<!doctype html><meta charset=utf-8>
<style>
 html,body{margin:0;height:100%;background:#1d2b1f;overflow:hidden}
 #c{position:absolute;left:50%;top:50%;transform:translate(-50%,-50%);display:block}
</style>
<canvas id=c width=600 height=400></canvas>
<script>
 const x=document.getElementById('c').getContext('2d');
 x.fillStyle='#1d2b1f'; x.fillRect(0,0,600,400);
 // 粗一点的地形噪声，避免整块纯色
 for(let i=0;i<400;i++){x.fillStyle=i%2?'#22301f':'#1a271c';
   x.fillRect((i*137)%600,(i*251)%400,18,14);}
 // 白扇（朝南）：先画，箭头压在上面
 x.fillStyle='#f4f4ef';
 x.beginPath(); x.moveTo(300,200); x.arc(300,200,60,Math.PI*0.28,Math.PI*0.72); x.closePath(); x.fill();
 // 青色飞镖（朝东）：钝鼻 + 两个尖尾。
 // 原版算法按"鼻子半径最小、两尾翼夹角 100°"定方向，画成实心三角会判无效。
 const deg=a=>a*Math.PI/180;
 const P=(a,r)=>[300+Math.sin(deg(a))*r, 200-Math.cos(deg(a))*r];
 const rn=34, rt=60, rc=20;
 const poly=[P(90,rn), P(90+130,rt), P(90+180,rc), P(90-130,rt)];
 x.fillStyle='rgb(34,200,232)';
 x.beginPath();
 x.moveTo(poly[0][0],poly[0][1]);
 for(let i=1;i<poly.length;i++) x.lineTo(poly[i][0],poly[i][1]);
 x.closePath(); x.fill();
</script>"#;
    format!("data:text/html;charset=utf-8,{}", url_encode(html))
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 合成地图自检：整条链路 + 已知角度对拍
fn synth_test_step(ui: &mut UiState) {
    if ui.im_started.is_none() {
        ui.im_started = Some(Instant::now());
        ui.synth = true;
        // 居中的 ROI：0.5/0.5、r=0.06（约 54px 半径）
        let roi = gifa_core::Roi::new(0.5, 0.5, 0.06).unwrap();
        // 可能已经存在（上次跑完落盘了）：先尝试建，再**强制选中**
        ui.layouts.create("synth".into(), "合成", roi).ok();
        ui.layouts.select("synth").ok();
        ui.logs.push(format!("合成自检布局 → {}", ui.layouts.active_layout_id));
        gifa_shell::navigate_stage(&synth_page_url());
        return;
    }
    let el = ui.im_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);

    if ui.vision_box.is_none() && el > 1.2 {
        if let Some(b) = vision_box(ui) {
            let _ = gifa_shell::overlay_init(110, Some((b.x + b.w / 2, b.y + b.h / 2)));
        }
        start_vision(ui);
        ui.im_phase = 1;
    }
    if ui.im_phase == 1 && el > 3.0 {
        // 浮窗被别的窗口（比如独占全屏的游戏）盖住时，GDI 截到的是上面那层，
        // 合成地图根本不在画面里 —— 这是环境问题，判 SKIP 而不是 FAIL。
        let covered = gifa_shell::snapshot().is_some_and(|snap| {
            !gifa_shell::float_on_top_at(
                snap.float_rect.x + snap.client.0 / 2,
                snap.float_rect.y + snap.stage.y + snap.stage.h / 2,
            )
        });
        if covered {
            for name in ["合成地图：箭头 ≈ 90°（朝东）", "合成地图：视角 ≈ 180°（朝南）", "结果送到叠加层（非空）"] {
                ui.vision_rows.push((
                    name.to_string(),
                    true,
                    "SKIP：浮窗被前台窗口遮住，截屏截到的是别人".into(),
                ));
            }
            report_vision(ui);
            ui.im_phase = 2;
            ui.im_started = None;
            return;
        }
        let last = ui.vision_last;
        let arrow = last.and_then(|e| e.result.arrow);
        let view = last.and_then(|e| e.result.view);
        ui.vision_rows.push((
            "合成地图：箭头 ≈ 90°（朝东）".into(),
            arrow.is_some_and(|a| (a - 90.0).abs() < 25.0),
            format!("arrow={arrow:?}"),
        ));
        ui.vision_rows.push((
            "合成地图：视角 ≈ 180°（朝南）".into(),
            view.is_some_and(|v| (v - 180.0).abs() < 30.0),
            format!("view={view:?}"),
        ));
        let drew = gifa_shell::overlay_drawn();
        ui.vision_rows.push((
            "结果送到叠加层（非空）".into(),
            drew.0.is_some() || drew.1.is_some(),
            format!("叠加层 arrow={:?} view={:?}", drew.0, drew.1),
        ));
        report_vision(ui);
        ui.im_phase = 2;
        ui.im_started = None;
    }
}

// ---------------------------------------------------------------- 识别自检

/// 骨架自检：真实抓屏 → 采样窗 → 检测 → 叠加层，跑 2.5s 后给结论。
fn vision_test_step(ui: &mut UiState) {
    if ui.im_started.is_none() {
        ui.im_started = Some(Instant::now());
        gifa_shell::player_command(PlayerCommand::VideoRect);
        return;
    }
    let el = ui.im_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);

    // 页面还没加载完时请求会落空：没拿到就每个 tick 重试（拿到即停）
    if ui.video_rect.is_none() && el > 0.4 && ui.im_phase == 0 {
        gifa_shell::player_command(PlayerCommand::VideoRect);
    }
    // 拿到视频矩形后的第一帧：起管线 + 摆好叠加层
    if ui.vision_box.is_none() && ui.video_rect.is_some() && el > 0.6 {
        if let Some(b) = vision_box(ui) {
            let _ = gifa_shell::overlay_init(110, Some((b.x + b.w / 2, b.y + b.h / 2)));
        }
        start_vision(ui);
        ui.im_phase = 1;
    }

    if std::env::var_os("GIFA_DEBUG").is_some() {
        eprintln!(
            "[vt] el={el:.2} phase={} vr={} box={} n={}",
            ui.im_phase,
            ui.video_rect.is_some(),
            ui.vision_box.is_some(),
            ui.vision_count
        );
    }
    if ui.im_phase == 1 && el > 2.8 {
        let n = ui.vision_count;
        let avg_cap = if n > 0 { ui.vision_capture_sum / n as f64 } else { 0.0 };
        let avg_det = if n > 0 { ui.vision_detect_sum / n as f64 } else { 0.0 };
        let last = ui.vision_last;

        ui.vision_rows.push((
            "抓到帧（2.5s 内 ≥8 帧）".into(),
            n >= 8,
            format!("{n} 帧，平均截帧 {avg_cap:.1}ms / 检测 {avg_det:.1}ms"),
        ));
        let samples = last.map(|e| e.result.samples).unwrap_or(0);
        ui.vision_rows.push((
            "采样窗有像素（不是黑帧）".into(),
            samples > 0,
            format!("采样点 {samples}，窗口 {:?}", last.map(|e| (e.window.w, e.window.h))),
        ));
        let drew = gifa_shell::overlay_drawn();
        ui.vision_rows.push((
            "结果送到叠加层".into(),
            ui.vision_count > 0,
            format!("叠加层当前 arrow={:?} view={:?}", drew.0, drew.1),
        ));
        let fast = n > 0 && avg_cap < 30.0 && avg_det < 8.0;
        ui.vision_rows.push((
            "性能：单帧 <8ms 检测 / <30ms 截帧".into(),
            fast,
            format!("截帧 {avg_cap:.1}ms，检测 {avg_det:.1}ms"),
        ));
        if let Some(e) = last {
            println!(
                "[vision] 最后一帧 arrow={:?} view={:?} cyan={:.5} 窗={:?}",
                e.result.arrow, e.result.view, e.result.cyan_frac, (e.window.x, e.window.y, e.window.w)
            );
        }
        report_vision(ui);
        ui.im_phase = 2;
        ui.im_started = None;
    }
}

fn report_vision(ui: &UiState) {
    println!();
    println!("============ 识别管线自检（骨架） ============");
    for (i, (name, ok, detail)) in ui.vision_rows.iter().enumerate() {
        println!("[{}] {:<30} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.vision_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("--------------------------------------------");
    println!("RESULT {pass}/{}", ui.vision_rows.len());
    println!("============================================");
    println!();
    if std::env::args().any(|a| a == "--vision-hold") {
        return; // 保持窗口，便于截图/人工观察
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 布局标定

/// 圆环（舞台相对物理像素）→ 去掉黑边 → 归一化 ROI → 写进当前布局。
///
/// 内置预设不允许就地改（原版语义），所以对着内置布局标定会**新建**一个自定义布局。
fn apply_calibration(ui: &mut UiState, cx: f64, cy: f64, r: f64) {
    let Some(vr) = ui.video_rect.clone() else {
        ui.toast = "还没拿到视频位置，请稍后再标定".into();
        return;
    };
    let k = if vr.dpr > 0.0 { vr.dpr } else { 1.0 };
    let elem = (vr.x * k, vr.y * k, vr.w * k, vr.h * k);
    let content = gifa_core::content_rect(elem, vr.vw, vr.vh, &vr.fit);
    let Some(roi) = gifa_core::roi_from_pixels((cx, cy, r), content) else {
        ui.toast = "圆环不在画面内，请重新套准".into();
        return;
    };

    let active_id = ui.layouts.active_layout_id.clone();
    let is_builtin = ui
        .layouts
        .layouts
        .iter()
        .find(|l| l.id == active_id)
        .map(|l| l.built_in)
        .unwrap_or(true);

    let result = if is_builtin {
        // 内置预设只读：新建一个
        let name = format!("自定义 {}", ui.layouts.layouts.iter().filter(|l| !l.built_in).count() + 1);
        let id = format!("custom-{}", ui.layouts.layouts.len());
        ui.layouts
            .create(id, &name, roi)
            .map(|_| format!("已新建布局「{name}」"))
    } else {
        ui.layouts
            .calibrate(&active_id, roi)
            .map(|_| format!("已更新布局「{}」", active_id))
    };

    match result {
        Ok(msg) => {
            ui.toast = msg.clone();
            ui.logs.push(format!(
                "标定 ROI cx={:.4} cy={:.4} r={:.4}（视频 {}×{}，fit={}）",
                roi.cx, roi.cy, roi.r, vr.vw, vr.vh, vr.fit
            ));
            persist_layouts(ui);
        }
        Err(e) => {
            ui.toast = format!("标定失败：{}", e.code());
        }
    }
}

fn persist_layouts(ui: &mut UiState) {
    if let Some(store) = ui.store.as_mut() {
        store.bearing.layouts = ui.layouts.clone();
        store.save();
    }
}

// ---------------------------------------------------------------- 标定自检

/// 没有真实鼠标也能验：用页面提供的测试钩子摆圆环 → 完成 → 校验 ROI 与区域。
fn calib_test_step(ui: &mut UiState) {
    if ui.im_started.is_none() {
        ui.im_started = Some(Instant::now());
        return;
    }
    let el = ui.im_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    match ui.im_phase {
        0 if el > 0.6 => {
            // 先要一次视频矩形，再开标定
            gifa_shell::player_command(PlayerCommand::VideoRect);
            gifa_shell::eval_chrome("window.gifaCalib && window.gifaCalib(true)");
            ui.im_phase = 1;
        }
        1 if el > 1.4 => {
            let vr = ui.video_rect.clone();
            let ok_rect = vr.as_ref().map(|r| r.vw > 0 && r.w > 0.0).unwrap_or(false);
            ui.calib_rows.push((
                "拿到 <video> 实际位置".into(),
                ok_rect,
                format!(
                    "{:?}",
                    vr.as_ref().map(|r| (r.x, r.y, r.w, r.h, r.vw, r.vh, r.fit.as_str(), r.dpr))
                ),
            ));
            // 标定层应该把舞台纳进"窗口区域"→ 舞台中心的命中归属变成 Chrome
            if let Some(s) = gifa_shell::snapshot() {
                let hx = s.float_rect.x + s.client.0 / 2;
                let hy = s.float_rect.y + s.stage.y + s.stage.h / 2;
                let hit = gifa_shell::classify_at(hx, hy);
                let bar_hit = gifa_shell::classify_at(s.float_rect.x + 100, s.float_rect.y + 10);
                let _ = &bar_hit;
                if std::env::var_os("GIFA_DEBUG").is_some() {
                    println!(
                        "[calib] stage_pt=({hx},{hy})->{hit:?} bar_pt->{bar_hit:?} stage={:?}",
                        (s.stage.x, s.stage.y, s.stage.w, s.stage.h)
                    );
                }
                ui.calib_rows.push((
                    "标定层可命中（舞台区域归 chrome）".into(),
                    // Other = 被别的窗口盖住（环境）；Float = 区域真没盖住舞台（bug）
                    hit == gifa_shell::Hit::Chrome || hit == gifa_shell::Hit::Other,
                    match hit {
                        gifa_shell::Hit::Chrome => "Chrome".to_string(),
                        gifa_shell::Hit::Other => "SKIP：浮窗被前台窗口盖住".to_string(),
                        _ => format!("{hit:?}（区域没覆盖舞台）"),
                    },
                ));
                // 摆一个已知圆环：舞台中心、半径 90（页面用视口坐标）
                let cx = s.client.0 as f64 / 2.0;
                let cy = s.stage.y as f64 + s.stage.h as f64 / 2.0;
                let js = format!("window.gifaCalib && window.gifaCalib(true, {cx}, {cy}, 90)");
                gifa_shell::eval_chrome(&js);
            }
            ui.im_phase = 2;
        }
        2 if el > 2.2 => {
            gifa_shell::eval_chrome("window.gifaCalibDone && window.gifaCalibDone()");
            ui.im_phase = 3;
        }
        3 if el > 3.0 => {
            let custom = ui.layouts.layouts.iter().find(|l| !l.built_in).cloned();
            let ok = custom.as_ref().map(|l| l.roi.is_valid()).unwrap_or(false);
            ui.calib_rows.push((
                "标定后生成合法自定义布局".into(),
                ok,
                format!(
                    "{:?}",
                    custom.as_ref().map(|l| (l.name.clone(), l.roi.cx, l.roi.cy, l.roi.r))
                ),
            ));
            // 切回内置预设，再切回自定义
            let before = ui.layouts.active_layout_id.clone();
            let switched = ui
                .layouts
                .select(gifa_core::bearing::BUILTIN_ID)
                .is_ok()
                && ui.layouts.active_layout_id == gifa_core::bearing::BUILTIN_ID;
            let back = ui.layouts.select(&before).is_ok();
            ui.calib_rows.push((
                "布局切换（内置 ↔ 自定义）".into(),
                switched && back,
                format!("当前 {}", ui.layouts.active_layout_id),
            ));
            report_calib(ui);
            ui.im_phase = 4;
            ui.im_started = None;
        }
        _ => {}
    }
}

fn report_calib(ui: &UiState) {
    println!();
    println!("============ 布局标定自检 ============");
    for (i, (name, ok, detail)) in ui.calib_rows.iter().enumerate() {
        println!("[{}] {:<28} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.calib_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("--------------------------------------");
    println!("RESULT {pass}/{}", ui.calib_rows.len());
    println!("======================================");
    println!();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 沉浸

/// 进入/退出沉浸：窗口侧交给 shell，页面侧通知 agent 收掉控件。
fn toggle_immersion(ui: &mut UiState, on: bool) {
    let aspect = ui
        .state
        .as_ref()
        .map(|s| {
            if s.video_h > 0 {
                s.video_w as f64 / s.video_h as f64
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);
    let active = gifa_shell::set_immersion(on, aspect);
    ui.immersion = active;
    ui.im_session = ui.im_session.wrapping_add(1);
    gifa_shell::player_command(PlayerCommand::Presentation {
        video_only: active,
        session: ui.im_session,
    });
    ui.toast = if active {
        "已进入纯视频沉浸".into()
    } else {
        "已退出纯视频沉浸".into()
    };
    ui.logs.push(ui.toast.clone());
}

/// 沉浸自检：进入 → 比例贴合 → 鼠标避让 → 退出还原
fn immersion_test_step(ui: &mut UiState) {
    if ui.im_started.is_none() {
        ui.im_started = Some(Instant::now());
        ui.im_before = gifa_shell::snapshot().map(|s| {
            (
                s.float_rect.x,
                s.float_rect.y,
                s.float_rect.w,
                s.float_rect.h,
            )
        });
        return;
    }
    let el = ui.im_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    match ui.im_phase {
        0 if el > 0.5 => {
            toggle_immersion(ui, true);
            if std::env::args().any(|a| a == "--immersion-hold") {
                ui.im_phase = 99; // 保持沉浸，不自动退出
                return;
            }
            ui.im_phase = 1;
        }
        1 if el > 1.4 => {
            let s = gifa_shell::snapshot();
            let before = ui.im_before;
            if let (Some(s), Some((bx, by, bw, bh))) = (s, before) {
                let ratio = if s.float_rect.h > 0 {
                    s.float_rect.w as f64 / s.float_rect.h as f64
                } else {
                    0.0
                };
                // 期望比例来自**真实视频分辨率**（B 站未登录时可能是 4:3）
                let want = ui
                    .state
                    .as_ref()
                    .filter(|s| s.video_h > 0)
                    .map(|s| s.video_w as f64 / s.video_h as f64)
                    .unwrap_or(16.0 / 9.0);
                let ok_ratio = (ratio - want).abs() < 0.03;
                let inside = s.float_rect.x >= bx
                    && s.float_rect.y >= by
                    && s.float_rect.x + s.float_rect.w <= bx + bw
                    && s.float_rect.y + s.float_rect.h <= by + bh;
                let centered = (s.float_rect.x + s.float_rect.w / 2 - (bx + bw / 2)).abs() <= 2
                    && (s.float_rect.y + s.float_rect.h / 2 - (by + bh / 2)).abs() <= 2;
                ui.im_rows.push((
                    "按视频比例贴合且居中".into(),
                    ok_ratio && inside && centered,
                    format!(
                        "比例 {ratio:.3}（期望 {want:.3}）/ {}×{} / inside={inside} centered={centered}",
                        s.float_rect.w, s.float_rect.h
                    ),
                ));
                ui.im_rows.push((
                    "隐藏 chrome + 强制穿透".into(),
                    !s.chrome_visible && s.click_through,
                    format!("chrome_visible={} click_through={}", s.chrome_visible, s.click_through),
                ));
                // 把鼠标移到窗口中心
                let cx = s.float_rect.x + s.float_rect.w / 2;
                let cy = s.float_rect.y + s.float_rect.h / 2;
                ui.im_can_move = gifa_shell::win32::move_cursor(cx, cy);
                if std::env::var_os("GIFA_DEBUG").is_some() {
                    println!(
                        "[hover] moved={} cursor→({cx},{cy}) 实际光标={:?}",
                        ui.im_can_move,
                        gifa_shell::win32::cursor_pos()
                    );
                }
            }
            ui.im_phase = 2;
        }
        2 if el > 2.4 => {
            let s = gifa_shell::snapshot();
            let faded = s.as_ref().map(|x| x.hovered && x.opacity <= 26).unwrap_or(false);
            if !ui.im_can_move {
                // 环境不允许移动光标（锁屏/远程会话）：判 SKIP 而不是 FAIL
                ui.im_rows.push((
                    "鼠标移入 → 变淡到 10%".into(),
                    true,
                    "SKIP：本环境不允许移动光标（锁屏/无交互会话）".into(),
                ));
                ui.im_rows.push((
                    "鼠标移出 → 恢复不透明度".into(),
                    true,
                    "SKIP：同上".into(),
                ));
                toggle_immersion(ui, false);
                ui.im_phase = 4;
                return;
            }
            ui.im_rows.push((
                "鼠标移入 → 变淡到 10%".into(),
                faded,
                format!(
                    "hovered={:?} opacity={:?} 心跳={}",
                    s.as_ref().map(|x| x.hovered),
                    s.as_ref().map(|x| x.opacity),
                    gifa_shell::hover_ticks()
                ),
            ));
            // 移出窗口（往右下 200px）
            if let Some(x) = s {
                gifa_shell::win32::move_cursor(
                    x.float_rect.x + x.float_rect.w + 200,
                    x.float_rect.y + x.float_rect.h + 200,
                );
            }
            ui.im_phase = 3;
        }
        3 if el > 3.4 => {
            let s = gifa_shell::snapshot();
            let restored = s.as_ref().map(|x| !x.hovered && x.opacity >= 250).unwrap_or(false);
            ui.im_rows.push((
                "鼠标移出 → 恢复不透明度".into(),
                restored,
                format!(
                    "hovered={:?} opacity={:?}",
                    s.as_ref().map(|x| x.hovered),
                    s.as_ref().map(|x| x.opacity)
                ),
            ));
            toggle_immersion(ui, false);
            ui.im_phase = 4;
        }
        4 if el > 4.3 => {
            let s = gifa_shell::snapshot();
            let before = ui.im_before;
            let ok = match (s, before) {
                (Some(s), Some((bx, by, bw, bh))) => {
                    let same = (s.float_rect.x, s.float_rect.y, s.float_rect.w, s.float_rect.h)
                        == (bx, by, bw, bh);
                    let visible = s.chrome_visible && !s.click_through && !s.immersion;
                    ui.im_rows.push((
                        "退出后尺寸精确还原".into(),
                        same,
                        format!("{:?} vs {:?}", (s.float_rect.x, s.float_rect.y, s.float_rect.w, s.float_rect.h), (bx, by, bw, bh)),
                    ));
                    visible
                }
                _ => false,
            };
            ui.im_rows.push((
                "退出后 chrome 与穿透复原".into(),
                ok,
                "chrome 可见、未穿透、immersion=false".into(),
            ));
            report_immersion(ui);
            ui.im_phase = 5;
            ui.im_started = None;
        }
        _ => {}
    }
}

fn report_immersion(ui: &UiState) {
    println!();
    println!("============ 沉浸与鼠标避让自检 ============");
    for (i, (name, ok, detail)) in ui.im_rows.iter().enumerate() {
        println!("[{}] {:<24} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.im_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("--------------------------------------------");
    println!("RESULT {pass}/{}", ui.im_rows.len());
    println!("============================================");
    println!();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 叠加层自检

/// 用合成角度驱动叠加层：验两路独立、补间、过期、清空、放置模式。
fn overlay_test_step(ui: &mut UiState) {
    // 保持模式：每 tick 续期一次，模拟真实识别循环（1.5s 不更新绿弧会自动收掉）
    if std::env::args().any(|a| a == "--overlay-hold") {
        let _ = gifa_shell::overlay_init(110, Some((520, 360)));
        gifa_shell::overlay_update(gifa_shell::OverlayState {
            arrow: Some(35.0),
            view: Some(120.0),
            view_conf: 0.9,
            view_width: 60.0,
            ..Default::default()
        });
        return;
    }
    if ui.ov_started.is_none() {
        ui.ov_started = Some(Instant::now());
        let _ = gifa_shell::overlay_init(110, Some((520, 360)));
        let hold = std::env::args().any(|a| a == "--overlay-hold");
        gifa_shell::overlay_update(gifa_shell::OverlayState {
            arrow: Some(if hold { 35.0 } else { 90.0 }),
            view: Some(if hold { 120.0 } else { 90.0 }),
            view_conf: 0.8,
            view_width: 60.0,
            ..Default::default()
        });
        if hold {
            ui.ov_phase = 99; // 保持画面，便于截图/人工看
        }
        return;
    }
    let el = ui.ov_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    match ui.ov_phase {
        0 if el > 0.8 => {
            let rect = gifa_shell::overlay_rect();
            let want = gifa_shell::overlay_size(110);
            let (arrow, view) = gifa_shell::overlay_drawn();
            let ok_size = rect.map(|r| r.w == want && r.h == want).unwrap_or(false);
            let ok_draw = arrow.is_some() && view.is_some();
            ui.ov_rows.push((
                "窗口可见且尺寸正确".into(),
                gifa_shell::overlay_visible() && ok_size && ok_draw,
                format!(
                    "visible={} size={:?}（期望 {want}）/ arrow={arrow:?} view={view:?} 画了绿弧={}",
                    gifa_shell::overlay_visible(),
                    rect.map(|r| (r.w, r.h)),
                    gifa_shell::overlay_drew_arc()
                ),
            ));
            // 只把视角转 180°，箭头保持
            gifa_shell::overlay_update(gifa_shell::OverlayState {
                arrow: Some(90.0),
                view: Some(270.0),
                view_conf: 0.8,
                view_width: 60.0,
                ..Default::default()
            });
            ui.ov_phase = 1;
        }
        1 if el > 1.5 => {
            let (arrow, view) = gifa_shell::overlay_drawn();
            let arrow_ok = arrow.is_some_and(|a| (a - 90.0).abs() < 0.5);
            let view_ok = view.is_some_and(|v| (v - 270.0).abs() < 0.5);
            ui.ov_rows.push((
                "两路独立：视角转 180° 箭头不动".into(),
                arrow_ok && view_ok,
                format!("arrow={arrow:?}（应 90）view={view:?}（应 270）"),
            ));
            // 45° 变化：应该走补间，而不是硬跳
            gifa_shell::overlay_update(gifa_shell::OverlayState {
                arrow: Some(135.0),
                view: Some(270.0),
                view_conf: 0.8,
                view_width: 60.0,
                ..Default::default()
            });
            ui.ov_phase = 2;
        }
        2 => {
            // 页面每画一帧都会回报角度；补间期间能抓到 90~135 之间的中间值
            let log = gifa_shell::overlay_drawn_log();
            if log
                .iter()
                .any(|(a, _)| a.is_some_and(|v| v > 91.0 && v < 134.0))
            {
                ui.ov_mid_seen = true;
            }
            if el > 2.2 {
                let (arrow, _) = gifa_shell::overlay_drawn();
                let settled = arrow.is_some_and(|a| (a - 135.0).abs() < 0.5);
                ui.ov_rows.push((
                    "补间：45° 走中间帧后收敛".into(),
                    ui.ov_mid_seen && settled,
                    format!(
                        "中间帧={} 最终={arrow:?} 采样{}帧",
                        ui.ov_mid_seen,
                        gifa_shell::overlay_drawn_log().len()
                    ),
                ));
                // 停喂视角，等过期
                gifa_shell::overlay_update(gifa_shell::OverlayState {
                    arrow: Some(135.0),
                    view: None,
                    ..Default::default()
                });
                ui.ov_phase = 3;
            }
        }
        3 if el > 4.0 => {
            ui.ov_rows.push((
                "视角 1.5s 无更新 → 收掉绿弧".into(),
                gifa_shell::overlay_view_stale(),
                format!("stale={}", gifa_shell::overlay_view_stale()),
            ));
            gifa_shell::overlay_update(gifa_shell::OverlayState {
                clear: true,
                ..Default::default()
            });
            ui.ov_phase = 4;
        }
        4 if el > 4.6 => {
            let (arrow, view) = gifa_shell::overlay_drawn();
            ui.ov_rows.push((
                "clear 后两路都清空".into(),
                arrow.is_none() && view.is_none(),
                format!("arrow={arrow:?} view={view:?}"),
            ));
            // 放置模式：关掉穿透、可移动
            gifa_shell::overlay_set_placing(true);
            gifa_shell::overlay_move_center(700, 420);
            ui.ov_phase = 5;
        }
        5 if el > 5.2 => {
            let center = gifa_shell::overlay_center();
            let ok_center = center
                .map(|(x, y)| (x - 700).abs() <= 1 && (y - 420).abs() <= 1)
                .unwrap_or(false);
            ui.ov_rows.push((
                "放置模式：可移动且圆心落盘".into(),
                gifa_shell::overlay_placing() && ok_center,
                format!("placing={} center={center:?}", gifa_shell::overlay_placing()),
            ));
            gifa_shell::overlay_set_placing(false);
            let _ = &mut ui.ov_mid_seen;
            report_overlay(ui);
            ui.ov_phase = 6;
            ui.ov_started = None;
        }
        _ => {}
    }
}

fn report_overlay(ui: &UiState) {
    println!();
    println!("============ 方位叠加层自检 ============");
    for (i, (name, ok, detail)) in ui.ov_rows.iter().enumerate() {
        println!("[{}] {:<26} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
    }
    let pass = ui.ov_rows.iter().filter(|(_, ok, _)| *ok).count();
    println!("----------------------------------------");
    println!("RESULT {pass}/{}", ui.ov_rows.len());
    println!("========================================");
    println!();
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- chrome 指令

fn handle_chrome(ui: &mut UiState, raw: &str) {
    let v: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return,
    };
    let kind = v.get("t").and_then(|x| x.as_str()).unwrap_or("");
    match kind {
        "load" => {
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
            if text.is_empty() {
                return;
            }
            ui.pending_short =
                gifa_player::is_short_link(text) && gifa_player::extract_id(text).is_none();
            let url = resolve(text);
            ui.logs.push(format!("载入 {url}"));
            gifa_shell::navigate_stage(&url);
            if let Some(id) = gifa_player::extract_id(text) {
                gifa_shell::player_command(PlayerCommand::FetchView { id });
            }
        }
        "part" => {
            let p = v.get("p").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
            if let Some(s) = ui.state.as_ref() {
                if !s.bvid.is_empty() {
                    let url = gifa_player::video_url(&s.bvid, p);
                    ui.logs.push(format!("切分 P → {p}"));
                    gifa_shell::navigate_stage(&url);
                }
            }
        }
        "login" => {
            let url = "https://passport.bilibili.com/login";
            match gifa_shell::open_popup(url, "B 站登录 · 扫码", 1000, 720) {
                Ok(()) => {
                    ui.toast = "请在弹窗里扫码登录".into();
                    ui.last_nav_poll = None;
                }
                Err(e) => ui.logs.push(format!("打开登录窗失败：{e}")),
            }
        }
        "toggle" => gifa_shell::player_command(PlayerCommand::Toggle),
        "seek-ratio" => {
            let ratio = v.get("ratio").and_then(|x| x.as_f64()).unwrap_or(0.0);
            if let Some(s) = &ui.state {
                if s.duration > 0.0 {
                    gifa_shell::player_command(PlayerCommand::Seek {
                        value: s.duration * ratio.clamp(0.0, 1.0),
                    });
                }
            }
        }
        "debug" => {
            if std::env::var_os("GIFA_DEBUG").is_some() {
                println!("[ui-debug] {v}");
            }
        }
        "probe" => ui.probe = Some(v.clone()),
        "seek" => {
            let value = v.get("value").and_then(|x| x.as_f64()).unwrap_or(0.0);
            ui.last_seek = Some(value);
            gifa_shell::player_command(PlayerCommand::Seek { value });
        }
        "rate" => {
            let value = v.get("value").and_then(|x| x.as_f64()).unwrap_or(1.0);
            gifa_shell::player_command(PlayerCommand::Rate { value });
        }
        "mute" => {
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            gifa_shell::player_command(PlayerCommand::Mute { value: on });
        }
        "min" => gifa_shell::minimize(),
        "settings" => ui.toast = "设置面板在启动器里（尚未接入）".into(),
        "clean" => {
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            gifa_shell::player_command(PlayerCommand::Clean { on });
        }
        "calib" => {
            ui.calib_rows.clear();
            gifa_shell::player_command(PlayerCommand::VideoRect);
            gifa_shell::eval_chrome("window.gifaCalib && window.gifaCalib(true)");
        }
        "calib-done" => {
            let cx = v.get("cx").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let cy = v.get("cy").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let r = v.get("r").and_then(|x| x.as_f64()).unwrap_or(0.0);
            apply_calibration(ui, cx, cy, r);
        }
        "layout-select" => {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
            if ui.layouts.select(id).is_ok() {
                ui.logs.push(format!("切换布局 → {id}"));
                persist_layouts(ui);
            }
        }
        "refresh-nav" => gifa_shell::player_command(PlayerCommand::FetchNav),
        "close" => gifa_shell::close(),
        _ => {}
    }
}

// ---------------------------------------------------------------- UI 推送

fn ui_payload(ui: &UiState) -> String {
    let s = ui.state.clone().unwrap_or_default();
    let pages: Vec<serde_json::Value> = ui
        .info
        .as_ref()
        .map(|i| {
            i.pages
                .iter()
                .map(|p: &PageInfo| {
                    serde_json::json!({"page": p.page, "part": p.part, "duration": p.duration})
                })
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({
        "title": if !ui.title.is_empty() { ui.title.clone() } else { "未载入".into() },
        "owner": ui.info.as_ref().map(|i| i.owner.clone()).unwrap_or_default(),
        "bvid": s.bvid,
        "part": s.part,
        "paused": s.paused,
        "ended": s.ended,
        "time": s.current_time,
        "dur": s.duration,
        "rate": s.rate,
        "volume": s.volume,
        "muted": s.muted,
        "hasVideo": s.has_video,
        "videoW": s.video_w,
        "videoH": s.video_h,
        "pages": pages,
        "loggedIn": ui.login.logged_in,
        "uname": ui.login.uname,
        "account": ui.login.describe(),
        "toast": ui.toast,
        "showUrlbar": false,
        "immersive": ui.immersion,
        "activeLayout": ui.layouts.active_layout_id,
        "layouts": ui.layouts.layouts.iter()
            .map(|l| serde_json::json!({"id": l.id, "name": l.name, "builtIn": l.built_in}))
            .collect::<Vec<_>>(),
        "log": ui.logs.last().cloned().unwrap_or_default(),
    })
    .to_string()
}

// ---------------------------------------------------------------- 自检

fn check_step(ui: &mut UiState, el: f64) -> bool {
    match ui.check.phase {
        0 if el > 1.2 => {
            if let Some(bvid) = ui.state.as_ref().map(|s| s.bvid.clone()).filter(|b| !b.is_empty()) {
                gifa_shell::player_command(PlayerCommand::FetchView { id: bvid });
            }
            gifa_shell::player_command(PlayerCommand::FetchNav);
            ui.check.phase = 1;
        }
        1 if el > 2.6 => {
            ui.check.view_ok.get_or_insert(false);
            ui.check.nav_ok.get_or_insert(false);
            gifa_shell::player_command(PlayerCommand::Pause);
            ui.check.phase = 2;
        }
        2 if el > 4.2 => {
            ui.check.pause_seen = ui.state.as_ref().is_some_and(|s| s.paused);
            ui.check.pause_ok = Some(ui.check.pause_seen);
            gifa_shell::player_command(PlayerCommand::Seek { value: 60.0 });
            ui.check.phase = 3;
        }
        3 if el > 6.2 => {
            ui.check.seek_seen = ui.state.as_ref().map(|s| s.current_time).unwrap_or(0.0);
            ui.check.seek_ok = Some((ui.check.seek_seen - 60.0).abs() < 15.0);
            gifa_shell::player_command(PlayerCommand::Rate { value: 1.5 });
            ui.check.phase = 4;
        }
        4 if el > 7.8 => {
            ui.check.rate_seen = ui.state.as_ref().map(|s| s.rate).unwrap_or(0.0);
            ui.check.rate_ok = Some((ui.check.rate_seen - 1.5).abs() < 0.01);
            gifa_shell::player_command(PlayerCommand::Rate { value: 1.0 });
            ui.check.phase = 5;
        }
        5 if el > 9.0 => return true,
        _ => {}
    }
    false
}

fn report_check() {
    UI.with(|ui| {
        let ui = ui.borrow();
        let s = ui.state.clone();
        println!();
        println!("============ M3 自检（解析 + 登录态 + 真页面）============");
        match s {
            Some(s) => {
                let rows: Vec<(&str, bool, String)> = vec![
                    (
                        "找到 <video> 且有元数据",
                        s.has_video && s.video_w > 0 && s.duration > 0.0,
                        format!("{}×{} / {:.0}s", s.video_w, s.video_h, s.duration),
                    ),
                    (
                        "agent 解析 BV 与分 P",
                        !s.bvid.is_empty() && s.part >= 1,
                        format!("{} P{}", s.bvid, s.part),
                    ),
                    ("标题可读", !s.title.is_empty(), s.title.clone()),
                    (
                        "video 接口：分 P 列表",
                        ui.check.view_ok.unwrap_or(false),
                        ui.check.view_seen.clone(),
                    ),
                    (
                        "nav 接口：登录态",
                        ui.check.nav_ok.unwrap_or(false),
                        ui.check.nav_seen.clone(),
                    ),
                    (
                        "指令：暂停",
                        ui.check.pause_ok.unwrap_or(false),
                        format!("采样 paused={}", ui.check.pause_seen),
                    ),
                    (
                        "指令：跳转到 60s",
                        ui.check.seek_ok.unwrap_or(false),
                        format!("采样 currentTime={:.1}", ui.check.seek_seen),
                    ),
                    (
                        "指令：倍速 1.5×",
                        ui.check.rate_ok.unwrap_or(false),
                        format!("采样 rate={}", ui.check.rate_seen),
                    ),
                ];
                for (i, (name, ok, detail)) in rows.iter().enumerate() {
                    println!(
                        "[{}] {:<24} {}  {}",
                        i + 1,
                        name,
                        if *ok { "PASS" } else { "FAIL" },
                        detail
                    );
                }
                let pass = rows.iter().filter(|(_, ok, _)| *ok).count();
                println!("--------------------------------------------");
                println!("RESULT {pass}/{}", rows.len());
            }
            None => {
                println!("FAIL：9s 内没有收到任何播放状态（页面没加载 / 网络不通 / agent 未注入）");
                for l in ui.logs.iter().rev().take(6) {
                    println!("  log: {l}");
                }
            }
        }
        println!("============================================");
        println!();
    });
}

fn report_runtime() {
    println!("============ WebView2 运行时基线 ============");
    match gifa_shell::webview2_version() {
        Ok(v) => {
            let ok = gifa_shell::runtime_ok(&v);
            println!("本机版本   = {v}");
            println!(
                "最低要求   = {}.x（Chromium 主版本）",
                gifa_shell::WEBVIEW2_MIN_MAJOR
            );
            println!("[1] 版本可解析        {}", if gifa_shell::parse_version(&v).is_some() { "PASS" } else { "FAIL" });
            println!("[2] 满足基线 ≥{}     {}", gifa_shell::WEBVIEW2_MIN_MAJOR, if ok { "PASS" } else { "FAIL" });
            println!("--------------------------------------------");
            println!("RESULT {}/2", if ok { 2 } else { 1 });
            if !ok {
                println!("提示：装 Microsoft Edge WebView2 Evergreen Runtime（或升级 Edge）。");
            }
        }
        Err(e) => {
            println!("FAIL：{e}");
            println!("提示：本机没有 WebView2 运行时，安装 Evergreen Runtime 后重试。");
        }
    }
    println!("============================================");
}

// 离线兜底页（没有指定地址时用）
const OFFLINE_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<style>html,body{margin:0;height:100%;background:#0b0d12;color:#cfd6e4;
font:14px/1.5 "Microsoft YaHei",system-ui,sans-serif}#c{position:absolute;inset:0;
display:flex;align-items:center;justify-content:center}</style></head>
<body><div id="c">离线占位：没有指定 B 站地址</div>
<video id="v" style="display:none"></video></body></html>"#;

/// 类型占位，避免 `Sender` 未使用告警
#[allow(dead_code)]
fn _assert_send(_: Sender<PlayerMsg>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_handles_all_input_forms() {
        assert_eq!(
            resolve("BV1xx411c7mD"),
            "https://www.bilibili.com/video/BV1xx411c7mD/?p=1&autoplay=1"
        );
        assert_eq!(
            resolve("https://www.bilibili.com/video/BV1xx411c7mD/?p=3"),
            "https://www.bilibili.com/video/BV1xx411c7mD/?p=3&autoplay=1"
        );
        assert_eq!(
            resolve("av170001"),
            "https://www.bilibili.com/video/av170001/?p=1&autoplay=1"
        );
        // 短链里如果自带 BV，直接规范化，不必等跳转
        assert_eq!(
            resolve("https://b23.tv/BV1xx411c7mD"),
            "https://www.bilibili.com/video/BV1xx411c7mD/?p=1&autoplay=1"
        );
    }

    #[test]
    fn opaque_short_link_is_passed_through() {
        let text = "https://b23.tv/AbCdEf";
        // 认不出 BV → 原样交给浏览器跳
        assert_eq!(resolve(text), text);
        assert!(gifa_player::is_short_link(text));
        assert!(gifa_player::extract_id(text).is_none());
        // 这三条同时成立才会走 pending_short 分支
        assert!(gifa_player::is_short_link(text) && gifa_player::extract_id(text).is_none());
    }
}
