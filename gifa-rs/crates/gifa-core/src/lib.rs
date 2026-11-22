//! GIFA 领域模型（无 OS 依赖，`cargo test` 可跑在 CI）。
//!
//! 这一层的价值：把「设置语义、方位布局规则、快捷键分流」从窗口与网络代码里剥出来，
//! 既能在无头环境测，也能被将来的 GUI/CLI 复用。

pub mod action;
pub mod bearing;
pub mod hotkey;
pub mod settings;
pub mod state;

pub use action::Action;
pub use bearing::{content_rect, roi_from_pixels, Layout, LayoutError, Layouts, Roi};
pub use hotkey::{plan_swap, Accelerator, Mod, ParseError, SwapPlan};
pub use settings::{Bounds, RecentEntry, Settings, FAVORITES_CAP, HISTORY_CAP};
pub use state::{toggle_opacity, HoldSpeed};
