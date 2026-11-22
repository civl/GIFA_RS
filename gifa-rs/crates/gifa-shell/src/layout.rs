//! 布局模型：chrome 页面上报的界面尺寸 → 舞台矩形 + chrome 区域 + 窗口位置约束。
//!
//! 这一层是**纯函数**（不碰 HWND），所以能在 CI 里跑 `cargo test`。

use crate::region::{RectI, RegionSpec};

/// 窗口最小尺寸（与原版一致：240×150）
pub const MIN_W: i32 = 240;
pub const MIN_H: i32 = 150;

/// 标题栏 / 底栏的默认高度（原版：30 + 34）
pub const DEFAULT_TITLEBAR: i32 = 30;
pub const DEFAULT_FOOTER: i32 = 34;

/// chrome 页面上报的布局，全部是**物理像素**（页面自己乘过 devicePixelRatio）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeLayout {
    pub titlebar: i32,
    pub footer: i32,
    /// 标题栏里"可以拖动窗口"的那一段（排除右侧按钮区）
    pub drag: Option<RectI>,
    /// URL 条：可见时夹在标题栏与舞台之间，把舞台往下推
    pub urlbar: Option<RectI>,
    /// 除标题栏/底栏/URL 条以外，还需要"存在"的浮动控件：pill、toast、标定环、grip
    pub overlay: Vec<RectI>,
}

impl Default for ChromeLayout {
    fn default() -> Self {
        Self {
            titlebar: DEFAULT_TITLEBAR,
            footer: DEFAULT_FOOTER,
            drag: None,
            urlbar: None,
            overlay: Vec::new(),
        }
    }
}

impl ChromeLayout {
    /// 舞台顶边：标题栏（+ 展开的 URL 条）
    pub fn stage_top(&self) -> i32 {
        self.titlebar.max(0) + self.urlbar.map_or(0, |r| r.h.max(0))
    }

    /// 舞台矩形（舞台 WebView2 的 bounds，也是"视频能占的区域"）
    pub fn stage_rect(&self, w: i32, h: i32) -> RectI {
        let top = self.stage_top();
        let bottom = self.footer.max(0);
        RectI::new(0, top, w, (h - top - bottom).max(1))
    }

    /// chrome 窗要"存在"的区域：顶栏 + 底栏 + URL 条 + 浮动控件
    pub fn region(&self, w: i32, h: i32) -> RegionSpec {
        let mut spec = RegionSpec::new()
            .with_rect(RectI::new(0, 0, w, self.titlebar.max(0)))
            .with_rect(RectI::new(0, h - self.footer.max(0), w, self.footer.max(0)));
        if let Some(r) = self.urlbar {
            spec = spec.with_rect(r);
        }
        for r in &self.overlay {
            spec = spec.with_rect(*r);
        }
        spec
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl WorkArea {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

impl Bounds {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn size_clamped(self) -> Self {
        Self {
            w: self.w.max(MIN_W),
            h: self.h.max(MIN_H),
            ..self
        }
    }
}

/// 首次启动的默认位置：贴工作区右上，留 40/60 的边距（原版行为）。
pub fn default_bounds(work: WorkArea, w: i32, h: i32) -> Bounds {
    let w = w.max(MIN_W).min(work.w - 16);
    let h = h.max(MIN_H).min(work.h - 16);
    Bounds::new(work.right() - w - 40, work.y + 60, w, h)
}

/// 把窗口拉回工作区。
///
/// 规则（照原版 `clampBounds`）：
/// * 右/下边溢出的，退回留 40px 边距（但至少留 8px 贴着工作区边）
/// * 左/上边超出太多（至少有 40px 还露在工作区外）的，贴回 8px
/// * 尺寸不小于 240×150
pub fn clamp_to_work(b: Bounds, work: WorkArea) -> Bounds {
    let b = b.size_clamped();
    let mut x = b.x;
    let mut y = b.y;

    if x + b.w > work.right() - 8 {
        x = (work.right() - b.w - 40).max(work.x + 8);
    }
    if x < work.x - b.w + 40 {
        x = work.x + 8;
    }
    if y + b.h > work.bottom() - 8 {
        y = (work.bottom() - b.h - 40).max(work.y + 8);
    }
    if y < work.y - b.h + 40 {
        y = work.y + 8;
    }
    Bounds { x, y, ..b }
}

/// 缩小窗口时保证舞台不被压成负高度（内容区至少留 60px）。
pub fn resize_floor(work: WorkArea, layout: &ChromeLayout) -> (i32, i32) {
    let min_h = (layout.stage_top() + layout.footer.max(0) + 60).max(MIN_H);
    (MIN_W.min(work.w), min_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> WorkArea {
        WorkArea {
            x: 0,
            y: 0,
            w: 1920,
            h: 1040,
        }
    }

    #[test]
    fn stage_rect_accounts_for_urlbar() {
        let mut l = ChromeLayout::default();
        assert_eq!(l.stage_rect(896, 568), RectI::new(0, 30, 896, 504));

        l.urlbar = Some(RectI::new(0, 30, 896, 40));
        assert_eq!(l.stage_top(), 70);
        assert_eq!(l.stage_rect(896, 568), RectI::new(0, 70, 896, 464));
    }

    #[test]
    fn region_is_titlebar_footer_plus_floating() {
        let l = ChromeLayout {
            urlbar: Some(RectI::new(0, 30, 896, 40)),
            overlay: vec![RectI::new(200, 500, 400, 38)],
            ..Default::default()
        };
        let spec = l.region(896, 568);
        assert!(spec.hit(10, 10), "标题栏");
        assert!(spec.hit(10, 550), "底栏");
        assert!(spec.hit(10, 40), "URL 条");
        assert!(spec.hit(300, 510), "pill");
        assert!(!spec.hit(300, 300), "舞台中间必须穿透");
        assert!(!spec.hit(300, 100), "舞台上部必须穿透");
    }

    #[test]
    fn clamp_pulls_back_from_right_edge() {
        let b = Bounds::new(1900, 60, 896, 568);
        let c = clamp_to_work(b, work());
        assert_eq!(c.x, 1920 - 896 - 40);
        assert_eq!(c.w, 896);
    }

    #[test]
    fn clamp_enforces_min_size() {
        let c = clamp_to_work(Bounds::new(100, 100, 50, 20), work());
        assert_eq!((c.w, c.h), (MIN_W, MIN_H));
    }

    #[test]
    fn clamp_keeps_partially_visible_window_in_place() {
        // 左边露 100px，仍在"至少 40px 可见"的范围内 → 不动
        let b = Bounds::new(-856, 100, 896, 568);
        let c = clamp_to_work(b, work());
        assert_eq!(c.x, -856);
    }

    #[test]
    fn resize_floor_keeps_stage_usable() {
        let l = ChromeLayout::default();
        assert_eq!(resize_floor(work(), &l), (MIN_W, 150));
        let tall = ChromeLayout {
            titlebar: 200,
            footer: 100,
            ..Default::default()
        };
        assert_eq!(resize_floor(work(), &tall), (MIN_W, 360));
    }
}
