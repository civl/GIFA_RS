//! 低级钩子线程（`WH_KEYBOARD_LL` / `WH_MOUSE_LL`）。
//!
//! 为什么需要它（照搬原版的判断）：
//! * **鼠标侧键**：`RegisterHotKey` 只支持键盘，根本注册不了鼠标键
//! * **单键**（`J` / `F8`）：`RegisterHotKey` 能注册，但会**全局吞掉**该键 ——
//!   用户绑了 `J` 之后在任何程序里都打不出 `J`，这是不可接受的副作用
//! * **按住类动作**（长按加速）：需要配对的按下/松开，系统热键只给按下
//! * **循环类动作**：需要按物理按键去重，否则按住会连续切档
//!
//! 代价是**不拦截**：事件照常传给前台程序，所以单键在游戏里也会生效。
//! 界面里必须写清楚这一点。

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use gifa_core::{Accelerator, Action, Mod};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, VK_CONTROL, VK_LWIN, VK_MENU,
    VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// 一次性动作
    Fire(Action),
    /// 按住类动作：按下
    Down(Action),
    /// 按住类动作：松开
    Up(Action),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HookKind {
    Key(u32),
    /// 4 = 侧键后退，5 = 侧键前进（与原版 `BUTTON_OF` 一致）
    Mouse(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HookBinding {
    kind: HookKind,
    mods: u8,
    action: Action,
}

struct HookState {
    bindings: Vec<HookBinding>,
    tx: Option<Sender<HotkeyEvent>>,
    pressed: HashSet<String>,
    /// 长按中：抬起时补一发 Up
    active_hold: Option<(String, Action)>,
    /// 抑制集合（重挂时把当前按下的键挡住，避免松开时误触发）
    blocked: HashSet<String>,
}

fn state() -> &'static Mutex<HookState> {
    static S: OnceLock<Mutex<HookState>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(HookState {
            bindings: Vec::new(),
            tx: None,
            pressed: HashSet::new(),
            active_hold: None,
            blocked: HashSet::new(),
        })
    })
}

/// 钩子句柄（安装在**调用它的线程**上，该线程必须自己在泵消息）。
///
/// 为什么不另起线程：低级钩子由系统投递到"安装它的线程"，
/// 而真正稳定在泵消息的是 UI 线程。实测专用线程上会间歇性收不到回调。
pub struct HookHandle {
    kb: HHOOK,
    ms: HHOOK,
}

impl HookHandle {
    /// 被系统悄悄摘掉后重新挂上（窗口最小化恢复、休眠唤醒）。
    pub fn rearm(&self) {
        // 先摘再挂：被摘掉的钩子只能靠重建恢复
        unsafe {
            let _ = UnhookWindowsHookEx(self.kb);
            let _ = UnhookWindowsHookEx(self.ms);
            let module = module_handle();
            let _ = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), module, 0);
            let _ = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        }
    }

    pub fn stop(&self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.kb);
            let _ = UnhookWindowsHookEx(self.ms);
        }
    }
}

/// 公开版的 HookKind（避免把内部类型暴露到 crate 外）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKindPub {
    Key(u32),
    Mouse(u8),
}

/// 由 accelerator 决定钩子形态：鼠标键或键盘键
pub fn kind_of(acc: &Accelerator) -> Option<HookKindPub> {
    match acc.key.as_str() {
        "mouse1" => Some(HookKindPub::Mouse(1)),
        "mouse2" => Some(HookKindPub::Mouse(2)),
        "mouse3" => Some(HookKindPub::Mouse(3)),
        "mouse4" => Some(HookKindPub::Mouse(4)),
        "mouse5" => Some(HookKindPub::Mouse(5)),
        _ => vk_of(&acc.key).map(HookKindPub::Key),
    }
}

/// 更新绑定表。线程不用重启 —— 钩子过程查的就是这份全局状态。
pub fn set_bindings(bindings: Vec<(HookKindPub, u8, Action)>) {
    if let Ok(mut st) = state().lock() {
        st.bindings = bindings
            .into_iter()
            .map(|(k, mods, action)| HookBinding {
                kind: match k {
                    HookKindPub::Key(vk) => HookKind::Key(vk),
                    HookKindPub::Mouse(b) => HookKind::Mouse(b),
                },
                mods,
                action,
            })
            .collect();
        // 绑定变了：按下集合与长按状态都作废，避免用旧表配对抬起
        st.pressed.clear();
        st.blocked.clear();
        st.active_hold = None;
    }
}

/// 在**调用线程**上安装钩子。调用方必须正在泵消息（通常是 UI 线程）。
pub fn start(tx: Sender<HotkeyEvent>) -> Result<HookHandle, String> {
    {
        let mut st = state().lock().map_err(|_| "钩子状态锁失败")?;
        st.tx = Some(tx);
        st.bindings.clear();
        st.pressed.clear();
        st.blocked.clear();
        st.active_hold = None;
    }
    unsafe {
        let module = module_handle();
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), module, 0)
            .map_err(|e| format!("安装键盘钩子失败: {e}"))?;
        let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0).map_err(|e| {
            let _ = UnhookWindowsHookEx(kb);
            format!("安装鼠标钩子失败: {e}")
        })?;
        Ok(HookHandle { kb, ms })
    }
}

// ---------------------------------------------------------------- 钩子过程

/// 低级钩子的硬性约束：回调必须在毫秒级返回，否则系统会把它摘掉。
/// 所以这里**只做查表 + 发消息**，不打印、不分配大对象、不做 IO。
extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        KB_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut st) = state().try_lock() {
            // SAFETY: code >= 0 时 lp 指向 KBDLLHOOKSTRUCT
            let info = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
            let vk = info.vkCode;
            let token = format!("key:{vk}");
            let is_injected = info.flags.0 & LLKHF_INJECTED.0 != 0;
            match wp.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    // 注入事件（SendInput）也要认：自检就是靠它驱动真实链路的
                    let _ = is_injected;
                    on_down(&mut st, HookKind::Key(vk), &token);
                }
                WM_KEYUP | WM_SYSKEYUP => {
                    on_up(&mut st, &token);
                }
                _ => {}
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// 鼠标消息 → 按钮编号（1=左 2=右 3=中 **4=侧键后退 5=侧键前进**）。
/// 抽成纯函数是为了能单测：`WH_MOUSE_LL` 在部分环境（VM/远程桌面/沙箱）
/// 装上却收不到回调，映射逻辑不能只靠实机验证。
pub fn mouse_button_of(msg: u32, mouse_data: u32) -> Option<u8> {
    match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP => Some(1),
        WM_RBUTTONDOWN | WM_RBUTTONUP => Some(2),
        WM_MBUTTONDOWN | WM_MBUTTONUP => Some(3),
        // XBUTTON 的按钮号在 mouseData 的**高字**
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let hi = ((mouse_data >> 16) & 0xFFFF) as u16;
            Some(if hi == 1 { 4 } else { 5 })
        }
        _ => None,
    }
}

extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        MOUSE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut st) = state().try_lock() {
            // SAFETY: code >= 0 时 lp 指向 MSLLHOOKSTRUCT
            let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
            let w = wp.0 as u32;
            // 只统计按键类消息（移动会淹没计数）
            if w != WM_MOUSEMOVE && w != WM_NCMOUSEMOVE {
                MOUSE_SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let packed = ((w as u64) << 32) | (info.mouseData as u64);
                LAST_MOUSE.store(packed, std::sync::atomic::Ordering::Relaxed);
            }
            let button = mouse_button_of(wp.0 as u32, info.mouseData);
            if let Some(b) = button {
                let token = format!("mouse:{b}");
                let down = matches!(
                    wp.0 as u32,
                    WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
                );
                if down {
                    on_down(&mut st, HookKind::Mouse(b), &token);
                } else {
                    on_up(&mut st, &token);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

// ---------------------------------------------------------------- 匹配逻辑

fn current_mods(st: &HookState) -> u8 {
    // 优先用自己跟踪的按键集合；鼠标事件没有按键流，退回 GetAsyncKeyState
    let pressed_mod = |names: &[u32]| {
        names.iter().any(|vk| {
            st.pressed.contains(&format!("key:{vk}"))
                || unsafe { GetAsyncKeyState(*vk as i32) as u16 & 0x8000 != 0 }
        })
    };
    let mut mods = 0u8;
    if pressed_mod(&[VK_CONTROL.0 as u32, 0xA2, 0xA3]) {
        mods |= Mod::Ctrl as u8;
    }
    if pressed_mod(&[VK_MENU.0 as u32, 0xA4, 0xA5]) {
        mods |= Mod::Alt as u8;
    }
    if pressed_mod(&[VK_SHIFT.0 as u32, 0xA0, 0xA1]) {
        mods |= Mod::Shift as u8;
    }
    if pressed_mod(&[VK_LWIN.0 as u32, VK_RWIN.0 as u32]) {
        mods |= Mod::Super as u8;
    }
    mods
}

fn on_down(st: &mut HookState, kind: HookKind, token: &str) {
    if st.pressed.contains(token) || st.blocked.contains(token) {
        return; // 物理重复 / 重挂后抑制
    }
    st.pressed.insert(token.to_string());

    let mods = current_mods(st);
    let hit = st
        .bindings
        .iter()
        .find(|b| b.kind == kind && b.mods == mods)
        .map(|b| b.action);

    let Some(action) = hit else { return };
    let Some(tx) = st.tx.as_ref() else { return };

    if action.is_hold() {
        if st.active_hold.is_some() {
            return;
        }
        st.active_hold = Some((token.to_string(), action));
        let _ = tx.send(HotkeyEvent::Down(action));
    } else {
        let _ = tx.send(HotkeyEvent::Fire(action));
    }
}

fn on_up(st: &mut HookState, token: &str) {
    st.pressed.remove(token);
    st.blocked.remove(token);

    let Some((held_token, action)) = st.active_hold.clone() else {
        return;
    };
    let Some(tx) = st.tx.as_ref() else { return };

    let is_main_key = held_token == token;
    // 修饰键先松也要补 Up，否则临时倍速会一直生效
    let released_modifier = held_token
        .strip_prefix("key:")
        .and_then(|v| v.parse::<u32>().ok())
        .is_some_and(is_modifier_vk);
    let still_down = released_modifier
        && st.pressed.iter().any(|t| {
            t.strip_prefix("key:")
                .and_then(|v| v.parse().ok())
                .is_some_and(is_modifier_vk)
        });
    if is_main_key || (released_modifier && !still_down) {
        st.active_hold = None;
        let _ = tx.send(HotkeyEvent::Up(action));
    }
}

fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5
    )
}

/// 低级钩子需要模块句柄（HMODULE → HINSTANCE 同值不同类型）
fn module_handle() -> Option<windows::Win32::Foundation::HINSTANCE> {
    let hm = unsafe { GetModuleHandleW(None) }.ok()?;
    Some(windows::Win32::Foundation::HINSTANCE(hm.0))
}

/// 按键名 → 虚拟键码
pub fn vk_of(key: &str) -> Option<u32> {
    let k = key.to_ascii_lowercase();
    let vk = match k.as_str() {
        "space" => 0x20,
        "left" | "arrowleft" => 0x25,
        "up" | "arrowup" => 0x26,
        "right" | "arrowright" => 0x27,
        "down" | "arrowdown" => 0x28,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "tab" => 0x09,
        "backspace" => 0x08,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "minus" => 0xBD,
        "equal" => 0xBB,
        "bracketleft" => 0xDB,
        "bracketright" => 0xDD,
        "semicolon" => 0xBA,
        "comma" => 0xBC,
        "period" => 0xBE,
        "slash" => 0xBF,
        "backquote" => 0xC0,
        "backslash" => 0xDC,
        "quote" => 0xDE,
        _ => {
            if let Some(rest) = k.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u32>() {
                    if (1..=24).contains(&n) {
                        return Some(0x70 + n - 1);
                    }
                }
                return None;
            }
            let mut chars = k.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii() {
                return None;
            }
            c.to_ascii_uppercase() as u32
        }
    };
    Some(vk)
}

/// 让 `Accelerator` 的修饰位转成 `RegisterHotKey` 需要的 MOD_* 组合
pub fn win_mods(acc: &Accelerator) -> u32 {
    let mut m = 0u32;
    if acc.has(Mod::Ctrl) {
        m |= MOD_CONTROL.0;
    }
    if acc.has(Mod::Alt) {
        m |= MOD_ALT.0;
    }
    if acc.has(Mod::Shift) {
        m |= MOD_SHIFT.0;
    }
    if acc.has(Mod::Super) {
        m |= MOD_WIN.0;
    }
    m
}

/// 诊断：鼠标钩子一共看到多少个按键事件（0 说明钩子没收到或注入被丢）
pub fn mouse_events_seen() -> u64 {
    MOUSE_SEEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// 诊断：键盘钩子被调用次数
pub fn kb_calls() -> u64 {
    KB_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// 诊断：钩子线程状态
pub fn hook_thread_alive() -> bool {
    state().try_lock().is_ok()
}

/// 诊断：鼠标钩子被调用的总次数（含移动）
pub fn mouse_calls() -> u64 {
    MOUSE_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// 诊断：最后一次鼠标事件 (wParam, mouseData)
pub fn last_mouse_event() -> Option<(u32, u32)> {
    let v = LAST_MOUSE.load(std::sync::atomic::Ordering::Relaxed);
    (v != 0).then_some(((v >> 32) as u32, (v & 0xFFFF_FFFF) as u32))
}

static MOUSE_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MOUSE_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 诊断：键盘钩子被调用次数
static KB_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LAST_MOUSE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 调试用：当前按下了哪些 token
pub fn pressed_snapshot() -> Vec<String> {
    state()
        .lock()
        .map(|s| s.pressed.iter().cloned().collect())
        .unwrap_or_default()
}

/// 取消长按（休眠、锁屏、换片时调用），保证不会卡在临时倍率上
pub fn cancel_hold() {
    if let Ok(mut st) = state().lock() {
        if let Some((_, action)) = st.active_hold.take() {
            if let Some(tx) = st.tx.as_ref() {
                let _ = tx.send(HotkeyEvent::Up(action));
            }
        }
    }
}

/// 供 UI 显示：某个键当前是否被别的动作占用（调试）
pub fn bound_count() -> usize {
    state().lock().map(|s| s.bindings.len()).unwrap_or(0)
}

/// 键盘键名 → VK 的反查表（诊断输出用）
pub fn describe_kind(kind: HookKindPub) -> String {
    match kind {
        HookKindPub::Key(vk) => format!("vk:{vk:#04x}"),
        HookKindPub::Mouse(b) => format!("mouse:{b}"),
    }
}

// 把内部 Hash 类型收起来，避免上层误用
#[allow(dead_code)]
type _T = HashMap<u32, u32>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_button_mapping_matches_original() {
        assert_eq!(mouse_button_of(WM_LBUTTONDOWN, 0), Some(1));
        assert_eq!(mouse_button_of(WM_RBUTTONUP, 0), Some(2));
        assert_eq!(mouse_button_of(WM_MBUTTONDOWN, 0), Some(3));
        // XBUTTON1 = 侧键后退（原版 Mouse4），按钮号在高字
        assert_eq!(mouse_button_of(WM_XBUTTONDOWN, 1 << 16), Some(4));
        // XBUTTON2 = 侧键前进（原版 Mouse5）
        assert_eq!(mouse_button_of(WM_XBUTTONUP, 2 << 16), Some(5));
        // 移动/滚轮不是按钮
        assert_eq!(mouse_button_of(WM_MOUSEMOVE, 0), None);
        assert_eq!(mouse_button_of(WM_MOUSEWHEEL, 0), None);
    }

    #[test]
    fn vk_table_covers_common_keys() {
        assert_eq!(vk_of("space"), Some(0x20));
        assert_eq!(vk_of("Left"), Some(0x25));
        assert_eq!(vk_of("F8"), Some(0x77));
        assert_eq!(vk_of("j"), Some(0x4A));
        assert_eq!(vk_of("J"), Some(0x4A));
        assert_eq!(vk_of("Mouse4"), None, "鼠标不是 VK");
        assert_eq!(vk_of("Hyper"), None);
    }

    #[test]
    fn modifier_bits_map_to_win32_mods() {
        let acc = Accelerator::parse("Ctrl+Alt+K").unwrap();
        let m = win_mods(&acc);
        assert_eq!(m & MOD_CONTROL.0, MOD_CONTROL.0);
        assert_eq!(m & MOD_ALT.0, MOD_ALT.0);
        assert_eq!(m & MOD_SHIFT.0, 0);
    }
}
