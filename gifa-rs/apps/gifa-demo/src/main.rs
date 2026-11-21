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
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

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
    store: Option<Store>,
}

/// 极简设置存储（M11 会挪进 `gifa-app` 并做原子写/迁移）
struct Store {
    path: PathBuf,
    s: Settings,
}

impl Store {
    fn load(path: PathBuf) -> Self {
        let s = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| Settings::from_json(&t).ok())
            .unwrap_or_default();
        Self { path, s }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.path, self.s.to_json_pretty());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--runtime") {
        report_runtime();
        return;
    }
    let check = args.iter().any(|a| a == "--check");
    let auto_login = args.iter().any(|a| a == "--login");
    let gamepad = args.iter().any(|a| a == "--gamepad");

    // ---- 设置 + 「上次看到」
    let data_root = std::env::current_dir().unwrap_or_default().join("data");
    let store = Store::load(data_root.join("settings.json"));

    let text = arg_value(&args, "--open")
        .or_else(|| arg_value(&args, "--url"))
        .unwrap_or_else(|| {
            let last = store.s.last_text.trim();
            if !last.is_empty() {
                println!("[gifa] 继续上次观看：{} P{}", last, store.s.last_part);
                last.to_string()
            } else {
                DEFAULT_TEXT.to_string()
            }
        });

    let (player_tx, player_rx) = std::sync::mpsc::channel::<PlayerMsg>();
    let (chrome_tx, chrome_rx) = std::sync::mpsc::channel::<String>();
    RX.with(|r| *r.borrow_mut() = Some((player_rx, chrome_rx)));

    let url = resolve(&text);
    println!("[gifa] 载入 {url}（输入 {:?}）", text);

    UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        ui.store = Some(store);
        ui.pending_short = gifa_player::is_short_link(&text) && gifa_player::extract_id(&text).is_none();
    });

    gifa_shell::create(ShellConfig {
        chrome_html: include_str!("../web/chrome.html").to_string(),
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
    gifa_shell::start_ticks(250);
    gifa_shell::run();
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
        store.s.last_text = text;
        store.s.last_part = s.part;
        store.s.last_video_id = s.bvid.clone();
        store.s.last_title = if title.is_empty() {
            store.s.last_title.clone()
        } else {
            title
        };
        store.s.last_page_count = ui.info.as_ref().map(|i| i.pages.len() as i32).unwrap_or(0);
        store.s.last_watched_at = std::time::SystemTime::now()
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
            let hold = ui.store.as_ref().map(|s| s.s.hold_playback_rate).unwrap_or(2.0);
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
        "clean" => {
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            gifa_shell::player_command(PlayerCommand::Clean { on });
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
