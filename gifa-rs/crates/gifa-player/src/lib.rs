//! 播放器协议：Rust ↔ 页面内 JS agent 的唯一契约。
//!
//! 设计原则（对应 task.md D2）：**播放控制全部留在页面里**。
//! Rust 只发指令、收状态；不解析 DOM、不猜 B 站内部结构。
//! 页面换了实现（播放器改版），改 agent 就行，Rust 侧不动。

use serde::{Deserialize, Serialize};

/// B 站要用桌面 UA：WebView2 默认带 `Edg/`，会走到不同分支。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 哪些是"短链"：需要先让浏览器跳一次才知道 BV 号
pub fn is_short_link(text: &str) -> bool {
    let s = text.to_ascii_lowercase();
    s.contains("b23.tv") || s.contains("bili2233.cn")
}

/// 视频页固定用 www（站外嵌入播放器上不了高清，见原版 `buildUrl`）。
pub fn video_url(id: &str, p: i32) -> String {
    let key = id.trim();
    let key = if let Some(rest) = key.strip_prefix("av") {
        format!("av{rest}")
    } else {
        key.to_string()
    };
    format!(
        "https://www.bilibili.com/video/{}/?p={}&autoplay=1",
        key,
        p.max(1)
    )
}

/// 从任意输入里抽视频 id（BV 号 / av 号 / 链接）。
pub fn extract_id(text: &str) -> Option<String> {
    let s = text.trim();
    if let Some(i) = s.find("BV") {
        let rest = &s[i + 2..];
        if rest.len() >= 10 && rest[..10].chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(format!("BV{}", &rest[..10]));
        }
    }
    // av 号：av123 / /av123
    let lower = s.to_ascii_lowercase();
    if let Some(i) = lower.find("av") {
        let digits: String = s[i + 2..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return Some(format!("av{digits}"));
        }
    }
    None
}

/// 从链接里抽分 P（`?p=3` / `&page=3`）。
pub fn extract_part(text: &str) -> Option<i32> {
    for key in ["?p=", "&p=", "?page=", "&page="] {
        if let Some(i) = text.find(key) {
            let digits: String = text[i + key.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<i32>() {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// 分 P 信息（`x/web-interface/view` 的 `pages[]`）
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PageInfo {
    pub page: i32,
    pub part: String,
    pub duration: i64,
}

/// 视频信息
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoInfo {
    pub bvid: String,
    pub title: String,
    pub owner: String,
    pub pages: Vec<PageInfo>,
}

impl VideoInfo {
    /// 该分 P 的标题（找不到就退回视频标题）
    pub fn part_title(&self, p: i32) -> &str {
        self.pages
            .iter()
            .find(|x| x.page == p)
            .map(|x| x.part.as_str())
            .unwrap_or(&self.title)
    }
}

/// `<video>` 在页面里的实际位置与缩放方式（标定要用它把屏幕坐标换算成视频像素）
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoRect {
    /// CSS 像素，相对舞台 webview 视口
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// 视频原始分辨率
    pub vw: i32,
    pub vh: i32,
    /// `contain` / `cover` / `fill`
    pub fit: String,
    /// 设备像素比：CSS 像素 × dpr = 物理像素
    pub dpr: f64,
}

/// 登录态（`x/web-interface/nav`）
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginState {
    pub logged_in: bool,
    pub uname: String,
    pub mid: i64,
    pub vip: bool,
    pub level: i32,
}

impl LoginState {
    /// 标题栏上那一行
    pub fn describe(&self) -> String {
        if !self.logged_in {
            "未登录 · 只能看预览和标清".into()
        } else if self.vip {
            format!("{} · 大会员 Lv{}", self.uname, self.level)
        } else {
            format!("{} · Lv{}", self.uname, self.level)
        }
    }
}

// ---------------------------------------------------------------- 页面 → Rust

/// agent 主动上报。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum PlayerMsg {
    /// agent 已注入并找到页面
    Ready {
        url: String,
        #[serde(default)]
        title: String,
    },
    /// 播放状态快照（事件驱动 + 1s 兜底轮询）
    State(PlayerState),
    /// 页面请求窗口全屏（拦截了 HTML 全屏）
    Fullscreen,
    /// `fetch-view` 的结果
    View {
        ok: bool,
        #[serde(default)]
        info: Option<VideoInfo>,
        #[serde(default)]
        error: String,
    },
    /// `video-rect` 的结果
    VideoRect {
        ok: bool,
        #[serde(default)]
        rect: Option<VideoRect>,
        #[serde(default)]
        error: String,
    },
    /// `fetch-nav` 的结果
    Nav {
        ok: bool,
        #[serde(default)]
        login: Option<LoginState>,
        #[serde(default)]
        error: String,
    },
    /// 诊断日志
    Log { msg: String },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayerState {
    pub paused: bool,
    pub ended: bool,
    pub current_time: f64,
    pub duration: f64,
    pub volume: f64,
    pub muted: bool,
    pub rate: f64,
    /// 视频原始分辨率（沉浸模式按它贴合比例）
    pub video_w: i32,
    pub video_h: i32,
    /// 是否已经找到 <video>（SPA 切 P 时会有短暂的 false）
    pub has_video: bool,
    pub bvid: String,
    pub part: i32,
    pub title: String,
    pub url: String,
}

// ---------------------------------------------------------------- Rust → 页面

/// 发给 agent 的指令。序列化成 `{cmd, ...}`。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum PlayerCommand {
    Toggle,
    Play,
    Pause,
    Seek { value: f64 },
    SeekBy { value: f64 },
    Rate { value: f64 },
    Volume { value: f64 },
    Mute { value: bool },
    /// 同步偏好（切 P / 新视频后要重新应用）
    Preferences {
        volume: f64,
        muted: bool,
        rate: f64,
    },
    /// 纯净模式
    Clean { on: bool },
    /// 只看视频（沉浸）
    ///
    /// 注意字段名：agent 里读的是 `m.videoOnly`，所以这里必须显式 camelCase，
    /// 否则序列化出 `video_only`，页面拿到 undefined 就悄悄失效了。
    Presentation {
        #[serde(rename = "videoOnly")]
        video_only: bool,
        session: u32,
    },
    /// 取视频信息（分 P 列表）：页面内 fetch，同源带 Cookie
    FetchView { id: String },
    /// 取登录态
    FetchNav,
    /// 取 `<video>` 的实际位置（标定用）
    VideoRect,
    Ping,
}

impl PlayerCommand {
    pub fn to_js(&self) -> String {
        let payload = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        format!("window.__gifa && window.__gifa.dispatch({payload})")
    }
}

/// 注入用的 agent 脚本（每次导航前都会执行）。
pub fn agent_js() -> &'static str {
    include_str!("../assets/player-agent.js")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bv_from_forms() {
        assert_eq!(
            extract_id("https://www.bilibili.com/video/BV1xx411c7mD/?p=2"),
            Some("BV1xx411c7mD".into())
        );
        assert_eq!(extract_id("BV1xx411c7mD"), Some("BV1xx411c7mD".into()));
        assert_eq!(
            extract_id("https://b23.tv/abc").map(|s| s.starts_with("BV")),
            None
        );
        assert_eq!(extract_id("随便说点什么"), None);
    }

    #[test]
    fn extracts_av() {
        assert_eq!(extract_id("av170001"), Some("av170001".into()));
        assert_eq!(
            extract_id("https://www.bilibili.com/video/av170001"),
            Some("av170001".into())
        );
    }

    #[test]
    fn extracts_part() {
        assert_eq!(extract_part("...?p=3"), Some(3));
        assert_eq!(extract_part("...&page=12"), Some(12));
        assert_eq!(extract_part("..."), None);
    }

    #[test]
    fn builds_www_url() {
        assert_eq!(
            video_url("BV1xx411c7mD", 2),
            "https://www.bilibili.com/video/BV1xx411c7mD/?p=2&autoplay=1"
        );
        assert_eq!(video_url("av1", 0), "https://www.bilibili.com/video/av1/?p=1&autoplay=1");
    }

    #[test]
    fn command_serializes_to_dispatch() {
        let js = PlayerCommand::SeekBy { value: 5.0 }.to_js();
        assert!(js.contains("window.__gifa"));
        assert!(js.contains("\"cmd\":\"seek-by\""));
        assert!(js.contains("\"value\":5.0"));
    }

    #[test]
    fn detects_short_links() {
        assert!(is_short_link("https://b23.tv/abc123"));
        assert!(is_short_link("b23.tv/abc"));
        assert!(is_short_link("https://bili2233.cn/xyz"));
        assert!(!is_short_link("https://www.bilibili.com/video/BV1xx411c7mD"));
    }

    #[test]
    fn parses_view_message() {
        let json = r#"{"t":"view","ok":true,"info":{"bvid":"BV1xx411c7mD","title":"T","owner":"UP",
                       "pages":[{"page":1,"part":"P1","duration":100},{"page":2,"part":"P2","duration":200}]}}"#;
        match serde_json::from_str::<PlayerMsg>(json).unwrap() {
            PlayerMsg::View { ok, info, .. } => {
                assert!(ok);
                let info = info.unwrap();
                assert_eq!(info.pages.len(), 2);
                assert_eq!(info.part_title(2), "P2");
                assert_eq!(info.part_title(9), "T");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_nav_message_and_describes() {
        let json = r#"{"t":"nav","ok":true,"login":{"loggedIn":true,"uname":"某人","mid":7,
                       "vip":false,"level":4}}"#;
        match serde_json::from_str::<PlayerMsg>(json).unwrap() {
            PlayerMsg::Nav { login, .. } => {
                let l = login.unwrap();
                assert!(l.logged_in);
                assert_eq!(l.describe(), "某人 · Lv4");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            LoginState::default().describe(),
            "未登录 · 只能看预览和标清"
        );
        assert_eq!(
            LoginState {
                logged_in: true,
                uname: "u".into(),
                vip: true,
                level: 6,
                ..Default::default()
            }
            .describe(),
            "u · 大会员 Lv6"
        );
    }

    #[test]
    fn presentation_uses_camel_case_field() {
        let js = PlayerCommand::Presentation {
            video_only: true,
            session: 3,
        }
        .to_js();
        assert!(js.contains("\"videoOnly\":true"), "agent 读的是 videoOnly：{js}");
        assert!(!js.contains("video_only"));
    }

    #[test]
    fn parses_state_message() {
        let json = r#"{"t":"state","paused":false,"currentTime":12.5,"duration":100,"volume":100,
                       "muted":false,"rate":1,"videoW":1920,"videoH":1080,"hasVideo":true,
                       "bvid":"BV1xx411c7mD","part":2,"title":"t","url":"u"}"#;
        match serde_json::from_str::<PlayerMsg>(json).unwrap() {
            PlayerMsg::State(s) => {
                assert_eq!(s.current_time, 12.5);
                assert_eq!(s.video_w, 1920);
                assert_eq!(s.part, 2);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
