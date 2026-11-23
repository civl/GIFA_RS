//! 方位布局：内置两套博主预设 + 用户自定义命名布局 + 旧版迁移。
//!
//! 注意 ROI 的归一化是**混用**的（照搬原版 `main.js` / `webview-preload.js`）：
//! `cx` 除以**视频宽**、`cy` 除以**视频高**、`r` 除以**视频宽**。
//! 搞错这个，换分辨率后圆环就会偏。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Roi {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
}

impl Roi {
    pub fn new(cx: f64, cy: f64, r: f64) -> Option<Self> {
        let v = Self { cx, cy, r };
        v.is_valid().then_some(v)
    }

    pub fn is_valid(&self) -> bool {
        self.cx.is_finite()
            && self.cy.is_finite()
            && self.r.is_finite()
            && self.cx > 0.0
            && self.cx < 1.0
            && self.cy > 0.0
            && self.cy < 1.0
            && (0.01..=0.5).contains(&self.r)
    }

    /// 视频像素尺寸 → 圆心与半径（半径按宽算，与原版一致）
    pub fn to_pixels(&self, video_w: i32, video_h: i32) -> (f64, f64, f64) {
        (
            self.cx * video_w as f64,
            self.cy * video_h as f64,
            self.r * video_w as f64,
        )
    }
}

/// 窗口像素 → 归一化 ROI。
///
/// 归一是**混用**的（照搬原版）：`cx`、`r` 除以视频**宽**，`cy` 除以视频**高**。
/// `video` 与 `ring` 必须用同一坐标系（这里都用浮窗客户区像素）。
pub fn roi_from_pixels(ring: (f64, f64, f64), video: (f64, f64, f64, f64)) -> Option<Roi> {
    let (cx, cy, r) = ring;
    let (vx, vy, vw, vh) = video;
    if !(vw > 1.0 && vh > 1.0 && r > 0.0) {
        return None;
    }
    let roi = Roi {
        cx: (cx - vx) / vw,
        cy: (cy - vy) / vh,
        r: r / vw,
    };
    roi.is_valid().then_some(roi)
}

/// `object-fit: contain`（或 cover）下，视频**画面**在元素里的实际矩形。
///
/// 标定圆环套的是"看到的画面"，而 ROI 要相对**视频帧**存，
/// 所以必须先去掉黑边（contain）或裁掉溢出（cover）。
pub fn content_rect(
    elem: (f64, f64, f64, f64),
    vw: i32,
    vh: i32,
    fit: &str,
) -> (f64, f64, f64, f64) {
    let (ex, ey, ew, eh) = elem;
    if vw <= 0 || vh <= 0 || ew <= 0.0 || eh <= 0.0 {
        return elem;
    }
    let content_aspect = vw as f64 / vh as f64;
    let elem_aspect = ew / eh;
    if fit.eq_ignore_ascii_case("fill") || (content_aspect - elem_aspect).abs() < 1e-6 {
        return elem;
    }
    let wider = content_aspect > elem_aspect; // 画面比元素更宽 → 上下留黑边
    match fit.eq_ignore_ascii_case("cover") {
        true => {
            // cover：画面被裁切，元素就是可见区域（ROI 仍相对整个画面，这里直接返回元素）
            elem
        }
        false => {
            if wider {
                let h = ew / content_aspect;
                (ex, ey + (eh - h) / 2.0, ew, h)
            } else {
                let w = eh * content_aspect;
                (ex + (ew - w) / 2.0, ey, w, eh)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub id: String,
    pub name: String,
    pub roi: Roi,
    #[serde(default)]
    pub built_in: bool,
    #[serde(default)]
    pub centered: bool,
    #[serde(default)]
    pub revision: u32,
}

pub const BUILTIN_ID: &str = "builtin";
pub const PANGOLIN_ID: &str = "pangolin";

fn builtin_layouts() -> Vec<Layout> {
    vec![
        Layout {
            id: BUILTIN_ID.into(),
            name: "你的影月月".into(),
            roi: Roi {
                cx: 282.0 / 1920.0,
                cy: 266.0 / 1080.0,
                r: 276.0 / 1920.0,
            },
            built_in: true,
            centered: true,
            revision: 0,
        },
        Layout {
            id: PANGOLIN_ID.into(),
            name: "提瓦特高级特工穿山甲".into(),
            roi: Roi {
                cx: 258.0 / 1920.0,
                cy: 284.0 / 1080.0,
                r: 260.0 / 1920.0,
            },
            built_in: true,
            centered: true,
            revision: 0,
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layouts {
    pub layouts: Vec<Layout>,
    pub default_layout_id: String,
    pub active_layout_id: String,
}

impl Default for Layouts {
    fn default() -> Self {
        Self {
            layouts: builtin_layouts(),
            default_layout_id: BUILTIN_ID.into(),
            active_layout_id: BUILTIN_ID.into(),
        }
    }
}

/// 操作结果：失败时带原因码，界面据此提示
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// 找不到这个布局
    NotFound,
    /// 内置布局不允许改名/删除/标定
    BuiltIn,
    /// 名称为空
    Name,
    /// 名称重复
    Duplicate,
    /// ROI 非法
    Roi,
    /// revision 不匹配（自动定心结果过期）
    Stale,
}

impl LayoutError {
    pub fn code(&self) -> &'static str {
        match self {
            LayoutError::NotFound => "not-found",
            LayoutError::BuiltIn => "built-in",
            LayoutError::Name => "name",
            LayoutError::Duplicate => "duplicate",
            LayoutError::Roi => "roi",
            LayoutError::Stale => "stale",
        }
    }
}

fn clean_name(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(40)
        .collect()
}

impl Layouts {
    /// 修复非法状态：至少有一个布局，且 active / default 都指向真实存在的那个。
    ///
    /// 为什么需要：`Layouts` 是直接 `Deserialize` 的，用户手改或版本回退都可能
    /// 留下 `layouts: []`。而 [`active`](Self::active) 里 `expect("至少有一个布局")`
    /// 在这种情况下会 panic —— 启动即崩，且崩在配置文件解析之后，很难查。
    pub fn repair(&mut self) {
        if self.layouts.is_empty() {
            *self = Self::default();
            return;
        }
        if !self.layouts.iter().any(|l| l.id == self.active_layout_id) {
            self.active_layout_id = self.layouts[0].id.clone();
        }
        if !self.layouts.iter().any(|l| l.id == self.default_layout_id) {
            self.default_layout_id = self.layouts[0].id.clone();
        }
    }

    pub fn active(&self) -> &Layout {
        self.layouts
            .iter()
            .find(|l| l.id == self.active_layout_id)
            .or_else(|| self.layouts.first())
            .expect("至少有一个布局")
    }

    fn index_of(&self, id: &str) -> Result<usize, LayoutError> {
        self.layouts
            .iter()
            .position(|l| l.id == id)
            .ok_or(LayoutError::NotFound)
    }

    pub fn select(&mut self, id: &str) -> Result<(), LayoutError> {
        let i = self.index_of(id)?;
        self.active_layout_id = self.layouts[i].id.clone();
        Ok(())
    }

    pub fn set_default(&mut self, id: &str) -> Result<(), LayoutError> {
        let i = self.index_of(id)?;
        self.default_layout_id = self.layouts[i].id.clone();
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), LayoutError> {
        let i = self.index_of(id)?;
        if self.layouts[i].built_in {
            return Err(LayoutError::BuiltIn);
        }
        self.layouts.remove(i);
        if self.default_layout_id == id {
            self.default_layout_id = BUILTIN_ID.into();
        }
        if self.active_layout_id == id {
            self.active_layout_id = self.default_layout_id.clone();
        }
        Ok(())
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), LayoutError> {
        let i = self.index_of(id)?;
        if self.layouts[i].built_in {
            return Err(LayoutError::BuiltIn);
        }
        let name = clean_name(name);
        if name.is_empty() {
            return Err(LayoutError::Name);
        }
        if self
            .layouts
            .iter()
            .enumerate()
            .any(|(j, l)| j != i && l.name.eq_ignore_ascii_case(&name))
        {
            return Err(LayoutError::Duplicate);
        }
        self.layouts[i].name = name;
        Ok(())
    }

    /// 新建并立即切到该布局
    pub fn create(&mut self, id: String, name: &str, roi: Roi) -> Result<(), LayoutError> {
        let name = clean_name(name);
        if name.is_empty() {
            return Err(LayoutError::Name);
        }
        if self
            .layouts
            .iter()
            .any(|l| l.name.eq_ignore_ascii_case(&name))
        {
            return Err(LayoutError::Duplicate);
        }
        if !roi.is_valid() {
            return Err(LayoutError::Roi);
        }
        self.layouts.push(Layout {
            id: id.clone(),
            name,
            roi,
            built_in: false,
            centered: false,
            revision: 0,
        });
        self.active_layout_id = id;
        Ok(())
    }

    /// 手持标定：改 ROI、清 centered、revision+1
    pub fn calibrate(&mut self, id: &str, roi: Roi) -> Result<(), LayoutError> {
        let i = self.index_of(id)?;
        if self.layouts[i].built_in || self.active_layout_id != id {
            return Err(LayoutError::BuiltIn);
        }
        if !roi.is_valid() {
            return Err(LayoutError::Roi);
        }
        let l = &mut self.layouts[i];
        l.roi = roi;
        l.centered = false;
        l.revision += 1;
        Ok(())
    }

    /// 自动定心的结果回写：revision 必须匹配，位移量有上限
    pub fn center(&mut self, id: &str, revision: u32, d: (f64, f64)) -> Result<(), LayoutError> {
        let i = self.index_of(id)?;
        if self.active_layout_id != id {
            return Err(LayoutError::NotFound);
        }
        let l = &self.layouts[i];
        if l.revision != revision {
            return Err(LayoutError::Stale);
        }
        if d.0.abs() > l.roi.r / 2.0 || d.1.abs() > l.roi.r {
            return Err(LayoutError::Roi);
        }
        let roi = Roi {
            cx: l.roi.cx + d.0,
            cy: l.roi.cy + d.1,
            r: l.roi.r,
        };
        if !roi.is_valid() {
            return Err(LayoutError::Roi);
        }
        let l = &mut self.layouts[i];
        l.roi = roi;
        l.centered = true;
        l.revision += 1;
        Ok(())
    }

    /// 旧版结构（`rois` 全局 + `roiBy` 按视频）迁成命名布局。
    /// 两个「种子」ROI 是旧版默认值的痕迹，跳过。
    pub fn migrate_legacy(rois: &[Roi], roi_by: &[(String, Vec<Roi>)]) -> Self {
        const SEEDS: [(f64, f64, f64); 2] = [(0.0875, 0.122, 0.046), (0.148, 0.222, 0.163)];
        let mut out = Self::default();
        let mut push_all = |list: &[Roi], label: &str| -> Option<String> {
            let mut first = None;
            for roi in list {
                if SEEDS.iter().any(|(cx, cy, r)| {
                    (cx - roi.cx).abs() < 1e-4
                        && (cy - roi.cy).abs() < 1e-4
                        && (r - roi.r).abs() < 1e-4
                }) {
                    continue;
                }
                if out.layouts.iter().any(|l| l.roi == *roi) {
                    first.get_or_insert_with(|| out.layouts.last().unwrap().id.clone());
                    continue;
                }
                let id = format!("legacy-{}", out.layouts.len());
                let name = format!("{label} · 原标定 {}", out.layouts.len());
                out.layouts.push(Layout {
                    id: id.clone(),
                    name,
                    roi: *roi,
                    built_in: false,
                    centered: false,
                    revision: 0,
                });
                first.get_or_insert(id);
            }
            first
        };
        // 先按视频（最近看的那个优先），再全局
        for (video, list) in roi_by {
            if let Some(id) = push_all(list, video) {
                out.default_layout_id = id;
            }
        }
        if let Some(id) = push_all(rois, "旧版") {
            if out.default_layout_id == BUILTIN_ID {
                out.default_layout_id = id;
            }
        }
        out.active_layout_id = out.default_layout_id.clone();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roi() -> Roi {
        Roi::new(0.147, 0.246, 0.144).unwrap()
    }

    #[test]
    fn defaults_are_two_builtins() {
        let l = Layouts::default();
        assert_eq!(l.layouts.len(), 2);
        assert_eq!(l.active().id, BUILTIN_ID);
        assert!(l.layouts.iter().all(|x| x.built_in));
    }

    #[test]
    fn roi_validation() {
        assert!(Roi::new(0.0, 0.5, 0.1).is_none());
        assert!(Roi::new(0.5, 0.5, 0.6).is_none());
        assert!(Roi::new(0.5, 0.5, 0.1).is_some());
    }

    #[test]
    fn create_select_remove() {
        let mut l = Layouts::default();
        l.create("x1".into(), "  测试   布局 ", roi()).unwrap();
        assert_eq!(l.active_layout_id, "x1");
        assert_eq!(l.layouts.last().unwrap().name, "测试 布局");
        l.select(BUILTIN_ID).unwrap();
        assert_eq!(l.active().id, BUILTIN_ID);
        l.set_default("x1").unwrap();
        l.remove("x1").unwrap();
        assert_eq!(l.default_layout_id, BUILTIN_ID);
        assert_eq!(l.layouts.len(), 2);
    }

    #[test]
    fn duplicate_name_rejected() {
        let mut l = Layouts::default();
        l.create("x1".into(), "你的影月月", roi()).unwrap_err();
        l.create("x2".into(), "新布局", roi()).unwrap();
        l.create("x3".into(), "新布局", roi()).unwrap_err();
    }

    #[test]
    fn builtin_cannot_be_removed_or_renamed() {
        let mut l = Layouts::default();
        assert_eq!(l.remove(BUILTIN_ID), Err(LayoutError::BuiltIn));
        assert_eq!(l.rename(BUILTIN_ID, "x"), Err(LayoutError::BuiltIn));
    }

    #[test]
    fn calibrate_bumps_revision_and_clears_centered() {
        let mut l = Layouts::default();
        l.create("x1".into(), "自定义", roi()).unwrap();
        let before = l.active().revision;
        l.calibrate("x1", Roi::new(0.2, 0.3, 0.12).unwrap())
            .unwrap();
        let a = l.active();
        assert_eq!(a.revision, before + 1);
        assert!(!a.centered);
    }

    #[test]
    fn center_requires_matching_revision() {
        let mut l = Layouts::default();
        l.create("x1".into(), "自定义", roi()).unwrap();
        let rev = l.active().revision;
        assert_eq!(
            l.center("x1", rev + 1, (0.001, 0.001)),
            Err(LayoutError::Stale)
        );
        l.center("x1", rev, (0.001, 0.0)).unwrap();
        assert!(l.active().centered);
        assert_eq!(l.active().revision, rev + 1);
    }

    #[test]
    fn center_rejects_absurd_offset() {
        let mut l = Layouts::default();
        l.create("x1".into(), "自定义", roi()).unwrap();
        let rev = l.active().revision;
        // 半径 0.144，上限 x=r/2=0.072
        assert_eq!(l.center("x1", rev, (0.5, 0.0)), Err(LayoutError::Roi));
    }

    #[test]
    fn legacy_migration_skips_seed_rois() {
        let legacy = vec![
            Roi::new(0.0875, 0.122, 0.046).unwrap(), // 种子，跳过
            Roi::new(0.147, 0.246, 0.144).unwrap(),
        ];
        let l = Layouts::migrate_legacy(&legacy, &[]);
        assert_eq!(l.layouts.len(), 3); // 2 内置 + 1 legacy
        assert!(l.layouts.iter().any(|x| x.id.starts_with("legacy-")));
        assert_ne!(l.default_layout_id, BUILTIN_ID);
    }

    #[test]
    fn legacy_migration_dedupes_same_roi() {
        let r = Roi::new(0.147, 0.246, 0.144).unwrap();
        let l = Layouts::migrate_legacy(&[r], &[("BV1xx".into(), vec![r])]);
        let custom: Vec<_> = l.layouts.iter().filter(|x| !x.built_in).collect();
        assert_eq!(custom.len(), 1);
    }

    #[test]
    fn roi_from_pixels_uses_mixed_normalization() {
        // 视频占客户区 (0,70) 起、1280×720；圆环中心 (282,266)、半径 276
        let roi = roi_from_pixels((282.0, 266.0, 276.0), (0.0, 70.0, 1280.0, 720.0)).unwrap();
        assert!((roi.cx - 282.0 / 1280.0).abs() < 1e-9, "cx 按宽归一");
        assert!(
            (roi.cy - 196.0 / 720.0).abs() < 1e-9,
            "cy 先减偏移再按**高**归一"
        );
        assert!((roi.r - 276.0 / 1280.0).abs() < 1e-9, "r 按宽归一");
        assert!(roi.is_valid());
    }

    #[test]
    fn roi_from_pixels_rejects_out_of_range() {
        // 圆环完全在视频外
        assert!(roi_from_pixels((5000.0, 5000.0, 50.0), (0.0, 0.0, 1280.0, 720.0)).is_none());
        // 半径 > 视频宽的一半
        assert!(roi_from_pixels((640.0, 360.0, 700.0), (0.0, 0.0, 1280.0, 720.0)).is_none());
        // 尺寸非法
        assert!(roi_from_pixels((10.0, 10.0, 5.0), (0.0, 0.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn content_rect_strips_letterbox() {
        // 元素 800×600（4:3），视频 16:9 → 上下留黑边
        let (x, y, w, h) = content_rect((0.0, 0.0, 800.0, 600.0), 1920, 1080, "contain");
        assert_eq!((x, w), (0.0, 800.0));
        assert!((h - 450.0).abs() < 1e-9);
        assert!((y - 75.0).abs() < 1e-9);

        // 元素 800×300（很宽），视频 4:3 → 左右留黑边
        let (x, y, w, h) = content_rect((0.0, 0.0, 800.0, 300.0), 640, 480, "contain");
        assert!((w - 400.0).abs() < 1e-9);
        assert!((x - 200.0).abs() < 1e-9);
        assert_eq!((y, h), (0.0, 300.0));
    }

    #[test]
    fn content_rect_passthrough_when_matching_or_fill() {
        let e = (10.0, 20.0, 1280.0, 720.0);
        assert_eq!(content_rect(e, 1920, 1080, "contain"), e, "比例一致不改");
        assert_eq!(content_rect(e, 640, 480, "fill"), e, "fill 不改");
        assert_eq!(content_rect(e, 640, 480, "cover"), e, "cover 返回元素本身");
    }

    #[test]
    fn roi_roundtrip_pixels() {
        let roi = Roi::new(0.22, 0.33, 0.14).unwrap();
        let (x, y, r) = roi.to_pixels(1280, 720);
        let back = roi_from_pixels((x, y, r), (0.0, 0.0, 1280.0, 720.0)).unwrap();
        assert!((back.cx - roi.cx).abs() < 1e-9);
        assert!((back.cy - roi.cy).abs() < 1e-9);
        assert!((back.r - roi.r).abs() < 1e-9);
    }

    #[test]
    fn roi_to_pixels_matches_original_mixed_normalization() {
        let r = Roi {
            cx: 282.0 / 1920.0,
            cy: 266.0 / 1080.0,
            r: 276.0 / 1920.0,
        };
        let (x, y, rad) = r.to_pixels(1920, 1080);
        assert!((x - 282.0).abs() < 1e-6);
        assert!((y - 266.0).abs() < 1e-6);
        assert!((rad - 276.0).abs() < 1e-6);
    }
}
