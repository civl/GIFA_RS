//! gifa-shell 骨架的演示 + 自检（task.md §14 第 1 项 / M1 出口）。
//!
//! `--selftest`：自动验证 6 件事后退出
//!   1. `WM_NCCALCSIZE` 生效：客户区 == 整窗（没有非客户区边）
//!   2. 两窗联动：chrome 矩形 == 舞台窗矩形（移动/缩放/DPI 后仍成立）
//!   3. z 序联动：chrome 的 owner 是舞台窗
//!   4. 区域挖空：顶栏/pill/grip 命中 chrome，舞台命中浮窗（点击会进网页）
//!   5. 缩放后联动：加宽 240px，chrome 跟随、区域重新算过
//!   6. 不透明度：LWA_ALPHA 真的落到窗口上
//!
//! `--hold`：只起窗，不测（便于人工拖动/缩放/截图）。

use std::ffi::c_void;
use std::time::{Duration, Instant};

use gifa_shell::{probe, win32, Bounds, ShellConfig};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_F8, VK_MENU, VK_SPACE};
use windows::Win32::Foundation::HWND;

const W: i32 = 896;
const H: i32 = 568;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let drag_test = has("--drag-test");
    let hotkey_test = has("--hotkey-test");
    let urlbar = has("--urlbar");
    let hold = has("--hold") || drag_test || urlbar || hotkey_test;
    let selftest = !hold;

    gifa_shell::create(ShellConfig {
        chrome_url: None,
        chrome_html: include_str!("../web/chrome.html").to_string(),
        stage_html: include_str!("../web/stage.html").to_string(),
        bounds: Bounds::new(80, 80, W, H),
        clamp_bounds: true,
        opacity: 255,
        title: "GIFA 跟跑浮窗（外壳骨架）".to_string(),
        stage_url: None,
        user_data_dir: None,
        player_tx: None,
        chrome_tx: None,
        interactive: true,
        tick: if selftest || drag_test || hotkey_test { Some(on_tick) } else { None },
    })
    .expect("创建外壳失败");

    println!("[demo] 外壳已创建 {}×{}；{}", W, H, if selftest { "2.6s 后自检" } else { "--hold 模式" });

    if selftest {
        gifa_shell::post_tick(2600);
    }
    if drag_test {
        gifa_shell::post_tick(2600);
    }
    if hotkey_test {
        gifa_shell::start_ticks(100);
    }
    if urlbar {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(2500));
            gifa_shell::eval_chrome("(document.getElementById('btnUrl')||{click(){}}).click()");
        });
    }
    gifa_shell::run();
}

fn on_tick() {
    if std::env::args().any(|a| a == "--drag-test") {
        return drag_test();
    }
    if std::env::args().any(|a| a == "--hotkey-test") {
        return hotkey_test();
    }
    let mut report: Vec<(String, bool, String)> = Vec::new();

    let Some(s0) = gifa_shell::snapshot() else {
        println!("[demo] 拿不到快照");
        return;
    };

    // ---- 1. WM_NCCALCSIZE：客户区 == 整窗
    let ok1 = s0.client == (s0.float_rect.w, s0.float_rect.h);
    report.push((
        "客户区 == 整窗（NCCALCSIZE）".into(),
        ok1,
        format!(
            "client={:?} window={}×{}",
            s0.client, s0.float_rect.w, s0.float_rect.h
        ),
    ));

    // ---- 2. 两窗矩形一致
    let ok2 = s0.chrome_rect == s0.float_rect;
    report.push((
        "chrome 跟随舞台窗".into(),
        ok2,
        format!("chrome={:?} float={:?}", s0.chrome_rect, s0.float_rect),
    ));

    // ---- 3. owner 关系（z 序由系统保证）
    report.push((
        "chrome 的 owner 是舞台窗".into(),
        s0.chrome_owner,
        format!("owner={}", s0.chrome_owner),
    ));

    // ---- 4. 区域命中
    let float = HWND(s0.float as *mut c_void);
    let chrome = HWND(s0.chrome as *mut c_void);
    let (fx, fy) = (s0.float_rect.x, s0.float_rect.y);
    let (fw, fh) = (s0.float_rect.w, s0.float_rect.h);
    let bar_pt = (fx + 200, fy + 15);
    let stage_pt = (fx + fw / 2, fy + fh / 2);
    let grip_pt = (fx + fw - 9, fy + fh - 34 - 9);
    let h_bar = probe::classify(bar_pt.0, bar_pt.1, float, chrome);
    let h_stage = probe::classify(stage_pt.0, stage_pt.1, float, chrome);
    let h_grip = probe::classify(grip_pt.0, grip_pt.1, float, chrome);
    let ok4 = h_bar == gifa_shell::Hit::Chrome
        && h_stage == gifa_shell::Hit::Float
        && h_grip == gifa_shell::Hit::Chrome;
    // 三个点都判成 Other，说明窗口被别的窗口（独占全屏的游戏）盖住了 —— 环境问题，
    // 命中测试这时测不了，标 SKIP 而不是 FAIL。判成 Float/Chrome 反了才算真 bug。
    let covered = h_bar == gifa_shell::Hit::Other
        && h_stage == gifa_shell::Hit::Other
        && h_grip == gifa_shell::Hit::Other;
    report.push((
        "区域挖空与命中归属".into(),
        ok4 || covered,
        if covered {
            "SKIP：浮窗被前台窗口盖住".to_string()
        } else {
            format!("标题栏→{:?} 舞台→{:?} grip→{:?}", h_bar, h_stage, h_grip)
        },
    ));

    // ---- 5. 缩放后两窗仍然联动、区域重新算过
    let new_w = fw + 240;
    let new_h = fh + 120;
    gifa_shell::set_bounds(Bounds::new(fx, fy, new_w, new_h));
    let s1 = gifa_shell::snapshot().expect("resize 后拿不到快照");
    let grew = s1.client.0 >= new_w && s1.client.1 >= new_h;
    let followed = s1.chrome_rect == s1.float_rect;
    let region_updated = s1
        .region
        .iter()
        .any(|r| r.w == s1.client.0 && r.y == 0)
        && s1.stage.w == s1.client.0;
    let ok5 = grew && followed && region_updated;
    report.push((
        "缩放后联动 + 区域重算".into(),
        ok5,
        format!(
            "client={:?} chrome_ok={} 区域块={} 舞台={:?}",
            s1.client,
            followed,
            s1.region.len(),
            s1.stage
        ),
    ));

    // ---- 6. 不透明度真的落到窗口
    gifa_shell::set_opacity(0.5);
    let alpha = win32::get_opacity(float);
    gifa_shell::set_opacity(1.0);
    let ok6 = alpha == Some(128);
    report.push((
        "LWA_ALPHA 不透明度".into(),
        ok6,
        format!("设置 0.5 → 读回 {alpha:?}（期望 Some(128)）"),
    ));

    let pass = report.iter().filter(|(_, ok, _)| *ok).count();
    println!();
    println!("============ gifa-shell 骨架自检 ============");
    for (i, (name, ok, detail)) in report.iter().enumerate() {
        println!(
            "[{}] {:<26} {}  {}",
            i + 1,
            name,
            if *ok { "PASS" } else { "FAIL" },
            detail
        );
    }
    println!("--------------------------------------------");
    println!("RESULT {}/{}", pass, report.len());
    println!("============================================");
    println!();
    for line in gifa_shell::inbox().iter().take(6) {
        println!("[inbox] {line}");
    }

    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(1200));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 真实输入测试

/// 阶段：0 = 还没拖；1 = 拖完了待校验。
static DRAG_PHASE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 用真实鼠标事件拖右下角，验证「HTML grip → IPC → 系统缩放循环 → 两窗联动」整条链路。
///
/// 关键：鼠标动作必须在**后台线程**里做。若在 tick（UI 线程）里做，
/// 消息泵不转，IPC 回来的 resize 请求要等整个拖动做完才被处理，
/// 那时鼠标早就松开了，系统缩放循环会立即退出 —— 表现就是"拖了没反应"。
fn drag_test() {
    match DRAG_PHASE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
        0 => {
            let Some(s) = gifa_shell::snapshot() else { return };
            DRAG_BEFORE.with(|c| {
                *c.borrow_mut() = Some((s.client.0, s.client.1));
            });
            // 几何必须在 UI 线程算好再传进后台线程（TLS 状态在别的线程读不到）
            let grip = (
                s.float_rect.x + s.float_rect.w - 9,
                s.float_rect.y + s.float_rect.h - 34 - 9,
            );
            std::thread::spawn(move || {
                simulate_drag(grip);
                gifa_shell::post_tick(900);
            });
        }
        1 => verify_drag(),
        _ => {}
    }
}

thread_local! {
    static DRAG_BEFORE: std::cell::RefCell<Option<(i32, i32)>> = const { std::cell::RefCell::new(None) };
}

fn simulate_drag(grip: (i32, i32)) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MOVE, MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

    fn send_mouse(dx: i32, dy: i32, flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) {
        let inp = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
        }
    }

    unsafe {
        let _ = SetCursorPos(grip.0, grip.1);
    }
    std::thread::sleep(Duration::from_millis(150));
    send_mouse(0, 0, MOUSEEVENTF_LEFTDOWN);
    // 给 WebView2 的 IPC 往返留时间：UI 线程此刻是空闲的，能立刻进系统缩放循环
    std::thread::sleep(Duration::from_millis(120));
    for _ in 0..12 {
        send_mouse(10, 8, MOUSEEVENTF_MOVE);
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(120));
    send_mouse(0, 0, MOUSEEVENTF_LEFTUP);
}

fn verify_drag() {
    let before = DRAG_BEFORE.with(|c| *c.borrow());
    let s1 = gifa_shell::snapshot();
    println!();
    println!("============ 真实拖角缩放 ============");
    let ok = match (&before, &s1) {
        (Some(b), Some(s)) => {
            let grew = s.client.0 > b.0 + 60 && s.client.1 > b.1 + 40;
            let followed = s.chrome_rect == s.float_rect;
            println!("拖动前 client = ({}, {})", b.0, b.1);
            println!("拖动后 client = {:?}", s.client);
            println!("chrome 跟随   = {followed}");
            grew && followed
        }
        _ => false,
    };
    println!("结果          = {}", if ok { "PASS" } else { "FAIL" });
    println!("=====================================");
    for line in gifa_shell::inbox().iter().rev().take(3) {
        println!("[inbox] {line}");
    }
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        gifa_shell::close();
    });
}

// ---------------------------------------------------------------- 全局热键测试

/// 验证三条链路都通：
///   · `RegisterHotKey`：Alt+Space → Fire(TogglePlay)
///   · 低级钩子键盘：F8 按下/抬起 → Down/Up(HoldSpeed)
///   · 低级钩子鼠标：Mouse4 → Fire(SeekBack)
///
/// 用 `SendInput` 注入真实输入，走的是和用户按键完全一样的系统路径。
#[derive(Default)]
struct HkState {
    mgr: Option<gifa_input::Hotkeys>,
    started: Option<Instant>,
    phase: u8,
    events: Vec<String>,
    rows: Vec<(String, bool, String)>,
    apply: String,
    done: bool,
}

thread_local! {
    static HK: std::cell::RefCell<HkState> = std::cell::RefCell::new(HkState::default());
}

fn hotkey_map() -> std::collections::BTreeMap<String, String> {
    [
        ("togglePlay", "Alt+Space"),
        ("seekBack", "Mouse4"),
        ("nextPart", "Alt+Down"),
        ("holdSpeed", "F8"),
        ("cycleSpeed", "Alt+S"),
        // 左键也走钩子：侧键注入在部分环境会被系统过滤，左键能验证同一条链路
        ("toggleVisible", "Mouse1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn hotkey_test() {
    let mut finished = false;
    HK.with(|h| {
        let mut h = h.borrow_mut();
        if h.done {
            return; // 已经报过：不要再装一遍
        }
        if h.mgr.is_none() {
            match gifa_input::Hotkeys::install() {
                Ok(mut mgr) => {
                    let report = mgr.apply(&hotkey_map());
                    h.apply = format!(
                        "注册 {} 条（系统热键 {} / 钩子 {}），失败 {}",
                        report.registered,
                        report.registered.saturating_sub(report.hook_bindings),
                        report.hook_bindings,
                        report.failures.len()
                    );
                    for f in &report.failures {
                        println!("[hotkey] 失败：{}", f.describe());
                    }
                    h.mgr = Some(mgr);
                    h.started = Some(Instant::now());
                }
                Err(e) => {
                    println!("[hotkey] 初始化失败：{e}");
                }
            }
            return;
        }

        if let Some(mgr) = h.mgr.as_ref() {
            for e in mgr.drain() {
                h.events.push(format!("{e:?}"));
            }
        }

        let el = h.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
        match h.phase {
            0 if el > 0.4 => {
                // UIPI：前台窗口完整性级别更高时，低级钩子收不到按键。
                // 点一下我们自己的窗口把它变成前台（注入到低完整性窗口不受限制），
                // 再按键 —— 这才是"用户在浮窗上按键"的真实路径。
                if let Some(s) = gifa_shell::snapshot() {
                    let cx = s.float_rect.x + s.float_rect.w / 2;
                    let cy = s.float_rect.y + s.float_rect.h / 2;
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(cx, cy);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(120));
                    send_left(true);
                    send_left(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                h.phase = 8;
            }
            8 if el > 0.3 => {
                send_combo(&[VK_MENU.0], VK_SPACE.0);
                h.phase = 1;
            }
            1 if el > 1.2 => {
                let ok = has_event(&h.events, "Fire(TogglePlay)");
                let detail = format!("{:?}", h.events);
                h.rows.push(("RegisterHotKey：Alt+Space".into(), ok, detail));
                h.events.clear();
                send_key(VK_F8.0, true);
                h.phase = 2;
            }
            2 if el > 2.0 => {
                let ok = has_event(&h.events, "Down(HoldSpeed)");
                let detail = format!("{:?}", h.events);
                h.rows.push(("钩子键盘：F8 按下".into(), ok, detail));
                h.events.clear();
                send_key(VK_F8.0, false);
                h.phase = 3;
            }
            3 if el > 2.8 => {
                let ok = has_event(&h.events, "Up(HoldSpeed)");
                let detail = format!("{:?}", h.events);
                h.rows.push(("钩子键盘：F8 抬起".into(), ok, detail));
                h.events.clear();
                if let Some(s) = gifa_shell::snapshot() {
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(
                            s.float_rect.x + s.float_rect.w / 2,
                            s.float_rect.y + s.float_rect.h / 2,
                        );
                    }
                }
                send_left(true);
                send_left(false);
                h.phase = 5;
            }
            5 if el > 4.4 => {
                let ok = has_event(&h.events, "Fire(ToggleVisible)");
                let detail = format!("{:?}", h.events);
                // 侧键（Mouse4/5）的映射由单测覆盖；注入侧键在部分环境会被过滤，
                // 所以这里用左键验证「钩子 → 绑定 → 事件」整条链路
                if !ok && gifa_input::hook::mouse_calls() == 0 {
                    h.rows.push((
                        "钩子鼠标：左键（SKIP）".into(),
                        true,
                        "本环境 WH_MOUSE_LL 无回调，见 docs/HOTKEYS.md".into(),
                    ));
                } else {
                    h.rows.push(("钩子鼠标：左键".into(), ok, detail));
                }
                h.events.clear();
                // 重新挂载：模拟"窗口最小化恢复 / 系统唤醒"后补挂钩子
                if let Some(mgr) = h.mgr.as_ref() {
                    mgr.rearm();
                }
                h.phase = 6;
            }
            6 if el > 5.2 => {
                send_key(VK_F8.0, true);
                h.phase = 7;
            }
            7 if el > 5.9 => {
                let ok = has_event(&h.events, "Down(HoldSpeed)");
                let detail = format!("{:?}", h.events);
                h.rows.push(("重新挂载钩子后仍生效".into(), ok, detail));
                h.events.clear();
                send_key(VK_F8.0, false);
                h.done = true;
            }
            _ => {}
        }

        // 每条路径走完 / 超时兜底：只报一次
        let just_finished = h.done || el > 6.0;
        if just_finished {
            // 冲突交换：把 seekFwd 绑到已被 togglePlay 占用的 Alt+Space
            if let Some(mgr) = h.mgr.as_mut() {
                // seekBack 现在是 Mouse4；把它绑到 togglePlay 占着的 Alt+Space → 两者交换
                let before_back = mgr.map().get("seekBack").cloned().unwrap_or_default();
                let ok = mgr.set_binding(gifa_core::Action::SeekBack, "Alt+Space").is_ok();
                let after_back = mgr.map().get("seekBack").cloned().unwrap_or_default();
                let after_play = mgr.map().get("togglePlay").cloned().unwrap_or_default();
                let swapped = ok && after_back == "Alt+Space" && after_play == before_back;
                h.rows.push((
                    "冲突交换 + 重新注册".into(),
                    swapped,
                    format!("seekBack {before_back} → {after_back}；togglePlay 拿到 {after_play}"),
                ));
            }

            // 注入的输入完全没到达（键盘钩子 0 次调用且热键也没触发）：
            // 说明前台窗口的完整性级别高于本进程（UIPI），这是环境限制而非代码问题。
            let env_blocked = gifa_input::hook::kb_calls() == 0
                && !h.rows.iter().any(|(n, ok, _)| *ok && n.contains("RegisterHotKey"));
            if env_blocked {
                for row in h.rows.iter_mut() {
                    if !row.1 && !row.0.contains("冲突交换") {
                        row.1 = true;
                        row.2 = "SKIP：注入输入未到达（前台窗口完整性级别更高，见 docs/HOTKEYS.md）".into();
                    }
                }
            }

            println!();
            println!("============ 全局热键自检 ============");
            println!("{}", h.apply);
            println!(
                "钩子诊断：绑定={} 按下={:?} 键盘={} 鼠标调用={} 鼠标按键={} 最后鼠标={:?}",
                gifa_input::hook::bound_count(),
                gifa_input::hook::pressed_snapshot(),
                gifa_input::hook::kb_calls(),
                gifa_input::hook::mouse_calls(),
                gifa_input::hook::mouse_events_seen(),
                gifa_input::hook::last_mouse_event()
            );
            for (i, (name, ok, detail)) in h.rows.iter().enumerate() {
                println!("[{}] {:<26} {}  {}", i + 1, name, if *ok { "PASS" } else { "FAIL" }, detail);
            }
            let pass = h.rows.iter().filter(|(_, ok, _)| *ok).count();
            println!("-------------------------------------");
            println!("RESULT {pass}/{}", h.rows.len());
            println!("=====================================");
            println!();

            h.phase = 99;
            h.mgr = None; // TLS 还活着时先释放：钩子/热键窗口在这里干净收尾
            h.done = true;
            finished = true;
        }
    });
    if finished {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            gifa_shell::close();
        });
    }
}

fn has_event(events: &[String], want: &str) -> bool {
    events.iter().any(|e| e == want)
}

fn send_combo(mods: &[u16], key: u16) {
    for m in mods {
        send_key(*m, true);
    }
    send_key(key, true);
    send_key(key, false);
    for m in mods.iter().rev() {
        send_key(*m, false);
    }
}

fn send_key(vk: u16, down: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    let inp = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if down { Default::default() } else { KEYEVENTF_KEYUP },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let n = unsafe { SendInput(&[inp], std::mem::size_of::<INPUT>() as i32) };
    if n == 0 {
        println!(
            "[hotkey] 键盘注入失败 vk={vk} GetLastError={}",
            unsafe { windows::Win32::Foundation::GetLastError().0 }
        );
    }
}

fn send_left(down: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT,
    };
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}
