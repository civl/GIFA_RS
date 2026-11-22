//! GIFA 外壳（task.md §4 的 `gifa-shell`）。
//!
//! 结构：**舞台窗**（只装 B 站 WebView2）+ **顶层 chrome 玻璃窗**（我们的 HTML 界面），
//! 两者用 owner 关系绑死 z 序，chrome 用区域挖空实现点击穿透。
//!
//! 该 crate 的职责边界：只碰窗口/WebView2/区域/DPI，不碰设置、B 站解析、视觉识别。

pub mod layout;
pub mod probe;
pub mod region;
pub mod shell;
pub mod win32;

pub use layout::{Bounds, ChromeLayout, WorkArea, MIN_H, MIN_W};
pub use probe::Hit;
pub use region::{RectI, RegionSpec};
pub use shell::{
    activate_existing, bounds, classify_at, click_through, close, close_popup, create, disc_size,
    disc_update, disc_visible, eval_chrome, find_window_by_class, float_on_top_at, focus_float,
    foreground_info, hide, hover_ticks, immersion, inbox, init_tray, is_bilibili_url, is_elevated,
    list_windows, message_box, minimize, navigate_stage, open_popup, overlay_center, overlay_drawn,
    overlay_drawn_log, overlay_drew_arc, overlay_init, overlay_move_center, overlay_placing,
    overlay_rect, overlay_set_placing, overlay_set_radius, overlay_size, overlay_update,
    overlay_view_stale, overlay_visible, player_command, player_state, popup_open, post_tick,
    prevent_suspension, primary_work_area, reassert_topmost, relaunch_elevated, run, set_bounds,
    set_click_through, set_hover_fade, set_immersion, set_interactive, set_opacity, show_inactive,
    show_window, snapshot, start_ticks, take_power_event, take_tray_action, tray_ready,
    try_single_instance, visible, webview2_version, window_at, DiscState, OverlayState, PowerEvent,
    ShellConfig, TrayAction, WEBVIEW2_MIN_MAJOR,
};
pub use shell::{parse_version, runtime_ok};
