//! 全局热键：**两套机制合流**成一个事件流。
//!
//! | 绑定 | 机制 | 原因 |
//! |---|---|---|
//! | 组合键（`Alt+Space`） | `RegisterHotKey` | 系统级独占，游戏前台也收得到 |
//! | 单键（`J`/`F8`） | 低级钩子 | 注册会**全局吞键**，用户别处就打不出这个键了 |
//! | 鼠标侧键 | 低级钩子 | `RegisterHotKey` 不支持鼠标 |
//! | 按住类 / 循环类 | 低级钩子 | 需要配对的按下/松开、需要按物理按键去重 |
//!
//! 冲突处理：新键被占用时**交换**，任一注册失败则整体回滚
//! （`gifa_core::hotkey::swap_map` 负责算新表，这里负责应用与回滚）。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{Receiver, Sender};

use gifa_core::{plan_swap, Accelerator, Action};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::hook::{self, HookHandle, HookKindPub, HotkeyEvent};

const CLASS_HOTKEY: PCWSTR = w!("GifaHotkey");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    /// 被其它程序占用
    Occupied,
    /// 认不出的键位
    Unsupported,
    /// 低级钩子不可用（被杀软/权限挡住）
    HookUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub action: Action,
    pub accelerator: String,
    pub reason: FailureReason,
}

impl Failure {
    /// 界面上的说明
    pub fn describe(&self) -> String {
        let why = match self.reason {
            FailureReason::Occupied => "被其他程序占用",
            FailureReason::Unsupported => "无法识别的键位",
            FailureReason::HookUnavailable => "低级钩子不可用（可能被安全软件拦截）",
        };
        format!("{}（{}）：{}", self.action.label(), self.accelerator, why)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub registered: usize,
    pub hook_bindings: usize,
    pub failures: Vec<Failure>,
}

/// (事件出口, 热键 id → 动作)
type Sink = Option<(Sender<HotkeyEvent>, HashMap<i32, Action>)>;

thread_local! {
    /// 热键窗口的 WndProc 靠它把事件投出去（回调在 UI 线程）
    static SINK: RefCell<Sink> = const { RefCell::new(None) };
}

/// TLS 在进程退出阶段可能已被销毁，WndProc/Drop 里一律走 `try_with`
/// 同一动作的重复抑制窗口：比自动重复间隔大、比人手两次按键慢。
/// 原版对钩子键用"按下集合"去重；`RegisterHotKey` 拿不到 keyup，只能按时间。
const REPEAT_GUARD: std::time::Duration = std::time::Duration::from_millis(300);

thread_local! {
    static LAST_FIRE: RefCell<std::collections::HashMap<Action, std::time::Instant>> =
        RefCell::new(std::collections::HashMap::new());
}

fn repeat_allowed(action: Action) -> bool {
    let now = std::time::Instant::now();
    LAST_FIRE.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(t) = m.get(&action) {
            if now.duration_since(*t) < REPEAT_GUARD {
                return false;
            }
        }
        m.insert(action, now);
        true
    })
}

fn with_sink<R>(f: impl FnOnce(&mut Sink) -> R) -> Option<R> {
    SINK.try_with(|s| f(&mut s.borrow_mut())).ok()
}

#[allow(dead_code)] // action/accelerator 留给启动器显示与冲突提示
struct Bound {
    id: i32,
    action: Action,
    accelerator: String,
}

pub struct Hotkeys {
    hwnd: HWND,
    #[allow(dead_code)]
    tx: Sender<HotkeyEvent>,
    rx: Receiver<HotkeyEvent>,
    hook: Option<HookHandle>,
    bound: Vec<Bound>,
    map: BTreeMap<String, String>,
    failures: Vec<Failure>,
    next_id: i32,
}

impl Hotkeys {
    /// 在**当前线程**创建热键窗口（需要一个正在跑的消息泵）。
    pub fn install() -> Result<Self, String> {
        win32_register_class()?;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_HOTKEY,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .map_err(|e| format!("创建热键窗口失败: {e}"))?
        };

        let (tx, rx) = std::sync::mpsc::channel();
        with_sink(|s| *s = Some((tx.clone(), HashMap::new())));

        // 钩子装在本线程（UI 线程），它本来就在泵消息
        let hook = match hook::start(tx.clone()) {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("[hotkeys] 钩子线程启动失败：{e}");
                None
            }
        };

        Ok(Self {
            hwnd,
            tx,
            rx,
            hook,
            bound: Vec::new(),
            map: BTreeMap::new(),
            failures: Vec::new(),
            next_id: 1,
        })
    }

    /// 全量重建（启动、设置变更都走这里）
    pub fn apply(&mut self, map: &BTreeMap<String, String>) -> ApplyReport {
        self.unregister_all();

        let mut hook_bindings: Vec<(HookKindPub, u8, Action)> = Vec::new();
        let mut failures = Vec::new();
        let mut registered = 0usize;

        let mut ids = HashMap::new();
        for (key, acc) in map {
            let Some(action) = Action::from_key(key) else {
                continue;
            };
            if acc.trim().is_empty() {
                continue; // 用户显式解绑
            }
            let parsed = match Accelerator::parse(acc) {
                Ok(a) => a,
                Err(_) => {
                    failures.push(Failure {
                        action,
                        accelerator: acc.clone(),
                        reason: FailureReason::Unsupported,
                    });
                    continue;
                }
            };

            if parsed.needs_hook(action) {
                match hook::kind_of(&parsed) {
                    Some(kind) => {
                        hook_bindings.push((kind, parsed.mods, action));
                        registered += 1;
                    }
                    None => failures.push(Failure {
                        action,
                        accelerator: acc.clone(),
                        reason: FailureReason::Unsupported,
                    }),
                }
                continue;
            }

            let Some(vk) = hook::vk_of(&parsed.key) else {
                failures.push(Failure {
                    action,
                    accelerator: acc.clone(),
                    reason: FailureReason::Unsupported,
                });
                continue;
            };
            let id = self.next_id;
            self.next_id += 1;
            let ok = unsafe {
                RegisterHotKey(
                    Some(self.hwnd),
                    id,
                    HOT_KEY_MODIFIERS(hook::win_mods(&parsed)),
                    vk,
                )
                .is_ok()
            };
            if ok {
                ids.insert(id, action);
                self.bound.push(Bound {
                    id,
                    action,
                    accelerator: acc.clone(),
                });
                registered += 1;
            } else {
                failures.push(Failure {
                    action,
                    accelerator: acc.clone(),
                    reason: FailureReason::Occupied,
                });
            }
        }

        // 钩子不可用时，所有钩子类绑定都要标出来，界面才解释得清
        let hook_ok = self.hook.is_some();
        if !hook_ok {
            for (key, acc) in map {
                let Some(action) = Action::from_key(key) else { continue };
                if acc.trim().is_empty() {
                    continue;
                }
                if let Ok(parsed) = Accelerator::parse(acc) {
                    if parsed.needs_hook(action) {
                        failures.push(Failure {
                            action,
                            accelerator: acc.clone(),
                            reason: FailureReason::HookUnavailable,
                        });
                    }
                }
            }
        }

        if hook_ok {
            hook::set_bindings(hook_bindings.clone());
        }

        with_sink(|s| {
            if let Some((_, map)) = s.as_mut() {
                *map = ids;
            }
        });

        self.map = map.clone();
        self.failures = failures.clone();
        ApplyReport {
            registered,
            hook_bindings: hook_bindings.len(),
            failures,
        }
    }

    /// 改一个绑定：冲突则交换，注册失败整体回滚。返回新表或错误。
    pub fn set_binding(
        &mut self,
        action: Action,
        accelerator: &str,
    ) -> Result<BTreeMap<String, String>, String> {
        let next = gifa_core::hotkey::swap_map(&self.map, action, accelerator)
            .map_err(|e| format!("键位无法解析: {e:?}"))?;
        let before = self.map.clone();
        let report = self.apply(&next);
        // 有失败就回滚，保证"要么全成，要么不变"
        if !report.failures.is_empty() {
            self.apply(&before);
            return Err(report
                .failures
                .first()
                .map(|f| f.describe())
                .unwrap_or_else(|| "注册失败".into()));
        }
        Ok(next)
    }

    /// 校验用：给出交换计划但不落盘
    pub fn preview_swap(&self, action: Action, accelerator: &str) -> Vec<(Action, String)> {
        let current: Vec<(Action, String)> = self
            .map
            .iter()
            .filter_map(|(k, v)| Action::from_key(k).map(|a| (a, v.clone())))
            .collect();
        plan_swap(action, accelerator, &current)
            .map(|p| p.displaced)
            .unwrap_or_default()
    }

    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }

    pub fn map(&self) -> &BTreeMap<String, String> {
        &self.map
    }

    /// 排空事件（UI 线程每个 tick 调一次）
    pub fn drain(&self) -> Vec<HotkeyEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            out.push(e);
        }
        out
    }

    /// 重新挂载低级钩子（窗口从最小化恢复、系统休眠唤醒后调用）
    pub fn rearm(&self) {
        if let Some(h) = self.hook.as_ref() {
            h.rearm();
        }
    }

    /// 取消长按（休眠/锁屏/换片）
    pub fn cancel_hold(&self) {
        hook::cancel_hold();
    }

    fn unregister_all(&mut self) {
        for b in self.bound.drain(..) {
            unsafe {
                let _ = UnregisterHotKey(Some(self.hwnd), b.id);
            }
        }
        with_sink(|s| {
            if let Some((_, map)) = s.as_mut() {
                map.clear();
            }
        });
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.unregister_all();
        if let Some(h) = self.hook.take() {
            h.stop();
        }
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn win32_register_class() -> Result<(), String> {
    let wc = WNDCLASSW {
        lpfnWndProc: Some(hotkey_proc),
        hInstance: crate::hinstance(),
        lpszClassName: CLASS_HOTKEY,
        ..Default::default()
    };
    unsafe {
        if RegisterClassW(&wc) == 0 {
            let err = windows::Win32::Foundation::GetLastError().0;
            if err != 1410 {
                return Err(format!("注册热键窗口类失败: {err}"));
            }
        }
    }
    Ok(())
}

extern "system" fn hotkey_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_HOTKEY {
        let id = wp.0 as i32;
        with_sink(|s| {
            if let Some((tx, map)) = s.as_ref() {
                if let Some(action) = map.get(&id) {
                    // RegisterHotKey 按住会连续发 WM_HOTKEY（Windows 自动重复）。
                    // 按键类动作必须去重，否则按住 Alt+I 会一秒切十次沉浸。
                    if repeat_allowed(*action) {
                        let _ = tx.send(HotkeyEvent::Fire(*action));
                    }
                }
            }
        });
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}
