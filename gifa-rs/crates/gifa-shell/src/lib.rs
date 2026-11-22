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
    bounds, close, close_popup, create, eval_chrome, hide, inbox, is_bilibili_url,
    minimize, navigate_stage, open_popup, player_command, popup_open,
    classify_at, click_through, float_on_top_at, window_at, foreground_info, focus_float, disc_size, hover_ticks, disc_update, disc_visible, immersion, overlay_center,
    overlay_drawn, overlay_drew_arc, overlay_drawn_log, overlay_init, overlay_rect, overlay_view_stale, overlay_move_center, overlay_placing, overlay_set_placing,
    overlay_set_radius, overlay_size, overlay_update, overlay_visible, set_click_through,
    set_hover_fade, set_immersion, DiscState, OverlayState,
    player_state, post_tick, reassert_topmost, run, set_bounds,
    set_interactive, set_opacity, show_inactive, snapshot, start_ticks, webview2_version,
    ShellConfig, WEBVIEW2_MIN_MAJOR,
};
pub use shell::{parse_version, runtime_ok};
