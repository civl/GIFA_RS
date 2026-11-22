//! 设置持久化（M11）。
//!
//! 四件事，逐条对着原版 `main.js`：
//! 1. **数据目录**：安装版 `%APPDATA%\原神跟跑助手`，绿色版 `exe\data`（探写权限后回退）
//! 2. **原子写**：先写 `.tmp` 再 `ReplaceFileW`（保留原文件的 ACL/时间戳），失败退化成 rename
//! 3. **防抖**：改动后 250ms 落盘，避免拖窗口时每帧写盘
//! 4. **迁移**：直接吃原版 `settings.json`；`bearing.rois`/`roiBy` 走 legacy 布局迁移

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gifa_core::{Layouts, Settings};
use serde_json::Value;

/// 配置文件格式版本（原版没有这个字段，新增用于"未来加字段"时的降级保护）
pub const CONFIG_VERSION: u32 = 1;

/// 落盘防抖窗口（原版 `persist()` → `setTimeout(flushSettings, 250)`）
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(250);

pub const FILE_NAME: &str = "settings.json";
const INSTALLED_FLAG: &str = "installed.flag";
const MIGRATE_FROM_FILE: &str = "migrate-from.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 源码直跑（不写程序目录）
    Dev,
    /// 安装版：程序目录有 `installed.flag`
    Installed,
    /// 绿色版：程序目录可写
    Portable,
    /// 兜底：程序目录不可写，退回用户目录
    Fallback,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Dev => "dev",
            Mode::Installed => "installed",
            Mode::Portable => "portable",
            Mode::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DataDir {
    pub path: PathBuf,
    pub mode: Mode,
}

/// 解析数据目录。`exe_dir` 是程序所在目录，`appdata` 是用户配置根（通常是 `%APPDATA%\原神跟跑助手`）。
pub fn resolve_data_dir(exe_dir: &Path, appdata: &Path, packaged: bool) -> DataDir {
    if !packaged {
        return DataDir {
            path: appdata.join("dev"),
            mode: Mode::Dev,
        };
    }
    if exe_dir.join(INSTALLED_FLAG).exists() {
        return DataDir {
            path: appdata.to_path_buf(),
            mode: Mode::Installed,
        };
    }
    // 绿色版：程序目录里建 data 并探一次写权限（只判断 exists 不够）
    let dir = exe_dir.join("data");
    if probe_writable(&dir) {
        return DataDir {
            path: dir,
            mode: Mode::Portable,
        };
    }
    DataDir {
        path: appdata.to_path_buf(),
        mode: Mode::Fallback,
    }
}

fn probe_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    if std::fs::write(&probe, b"ok").is_err() {
        return false;
    }
    let _ = std::fs::remove_file(&probe);
    true
}

/// 旧版数据目录候选（原版 `migrateLegacyData` 的做法）
pub fn legacy_candidates(exe_dir: &Path, appdata: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // 安装版升级：旧数据在 exe 旁边的 data\
    out.push(exe_dir.join("data"));
    // 绿色版/安装版互换：另一个位置
    if appdata.ends_with("data") {
        out.push(appdata.to_path_buf());
    }
    // migrate-from.txt（安装器迁移时写下旧目录）
    if let Ok(text) = std::fs::read_to_string(exe_dir.join(MIGRATE_FROM_FILE)) {
        let p = PathBuf::from(text.trim());
        if !p.as_os_str().is_empty() {
            out.push(p);
        }
    }
    out
}

/// 需要搬迁的旧文件（登录态单独处理，体积大且可能被占用）
const MIGRATE_FILES: [&str; 3] = [FILE_NAME, "bearing-dump.png", "bearing-frame.png"];

/// 把旧目录里的设置与标定图搬到新目录（已存在的不覆盖）。返回搬了哪些名字。
pub fn migrate_legacy_data(from: &Path, to: &Path) -> Vec<String> {
    let mut done = Vec::new();
    if from == to || !from.is_dir() {
        return done;
    }
    if std::fs::create_dir_all(to).is_err() {
        return done;
    }
    for name in MIGRATE_FILES {
        let src = from.join(name);
        let dst = to.join(name);
        if !src.is_file() || dst.exists() {
            continue;
        }
        if std::fs::copy(&src, &dst).is_ok() {
            done.push(name.to_string());
        }
    }
    done
}

/// 原子写：`.tmp` → `ReplaceFileW`（失败退化成 rename）
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    replace_file(&tmp, path)
}

#[cfg(windows)]
fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::ReplaceFileW;

    let wide = |p: &Path| -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    if !dst.exists() {
        // 目标还不存在：ReplaceFileW 要求目标存在，直接改名
        return std::fs::rename(src, dst);
    }
    let s = wide(dst);
    let r = wide(src);
    unsafe {
        ReplaceFileW(
            PCWSTR(s.as_ptr()),
            PCWSTR(r.as_ptr()),
            PCWSTR::null(),
            windows::Win32::Storage::FileSystem::REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    }
    .map_err(|e| std::io::Error::other(format!("ReplaceFileW 失败: {e}")))
    // 极少数文件系统上 ReplaceFileW 不可用：退化成 rename（同卷内同样是原子的）
    .or_else(|_| std::fs::rename(src, dst))
}

#[cfg(not(windows))]
fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(src, dst)
}

/// 设置仓储：内存里的 `Settings` + 落盘策略
pub struct SettingsStore {
    path: PathBuf,
    settings: Settings,
    version: u32,
    dirty: bool,
    dirty_since: Option<Instant>,
    /// 载入时发现配置来自更新的版本：第一次保存前先备份
    newer_version: Option<u32>,
    /// 上次迁移搬过来的文件（诊断用）
    pub migrated: Vec<String>,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> Self {
        let mut migrated = Vec::new();
        let (mut settings, version, newer) = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let raw: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                let version = raw
                    .get("configVersion")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .unwrap_or(0);
                let settings = Settings::from_json(&text).unwrap_or_default();
                let newer = (version > CONFIG_VERSION).then_some(version);
                (settings, version, newer)
            }
            Err(_) => (Settings::default(), 0, None),
        };

        // 旧版 bearing.rois / roiBy → legacy 布局
        if migrate_bearing(&mut settings).is_some() {
            migrated.push("bearing.rois→layouts".into());
        }

        Self {
            path,
            settings,
            version: if version == 0 {
                CONFIG_VERSION
            } else {
                version
            },
            dirty: false,
            dirty_since: None,
            newer_version: newer,
            migrated,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// 改设置：自动标记脏（不立即写盘）
    pub fn settings_mut(&mut self) -> &mut Settings {
        self.dirty = true;
        self.dirty_since = Some(Instant::now());
        &mut self.settings
    }

    /// 深合并一个 patch（IPC 传进来的部分更新）
    pub fn apply_patch(&mut self, patch: &Value) -> Result<(), String> {
        let mut next = self.settings.clone();
        next.apply_patch(patch)?;
        self.settings = next;
        self.dirty = true;
        self.dirty_since = Some(Instant::now());
        Ok(())
    }

    /// 每帧调用：到 250ms 才真正落盘。返回是否写了盘。
    pub fn save_if_due(&mut self, now: Instant) -> bool {
        if !self.dirty {
            return false;
        }
        let due = self
            .dirty_since
            .map(|t| now.duration_since(t) >= SAVE_DEBOUNCE)
            .unwrap_or(true);
        if !due {
            return false;
        }
        self.flush().is_ok()
    }

    /// 立即落盘（退出、提权重启前必须调）
    pub fn flush(&mut self) -> Result<(), String> {
        // 降级保护：配置来自更新版本时，先留一份原样备份
        if let Some(v) = self.newer_version.take() {
            let bak = self.path.with_extension(format!("json.v{v}.bak"));
            let _ = std::fs::copy(&self.path, &bak);
        }
        let mut out = serde_json::to_value(&self.settings).map_err(|e| e.to_string())?;
        if let Value::Object(map) = &mut out {
            map.insert("configVersion".into(), Value::from(CONFIG_VERSION));
        }
        let text = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?;
        write_atomic(&self.path, text.as_bytes()).map_err(|e| e.to_string())?;
        self.dirty = false;
        self.dirty_since = None;
        Ok(())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn version(&self) -> u32 {
        self.version
    }
}

/// 旧版的 `bearing.rois` / `bearing.roiBy` → 命名布局。
/// 只有在用户还没有自定义布局时才迁移，避免覆盖新数据。
pub fn migrate_bearing(settings: &mut Settings) -> Option<usize> {
    let has_custom = settings.bearing.layouts.layouts.iter().any(|l| !l.built_in);
    if has_custom {
        return None;
    }
    let extra = &settings.bearing.extra;

    let parse_list = |v: &Value| -> Vec<gifa_core::Roi> {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        let f = |k: &str| e.get(k).and_then(|x| x.as_f64());
                        gifa_core::Roi::new(f("cx")?, f("cy")?, f("r")?)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let rois = extra.get("rois").map(parse_list).unwrap_or_default();
    let roi_by: Vec<(String, Vec<gifa_core::Roi>)> = extra
        .get("roiBy")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), parse_list(v)))
                .collect()
        })
        .unwrap_or_default();

    if rois.is_empty() && roi_by.is_empty() {
        return None;
    }
    let migrated: Layouts = gifa_core::Layouts::migrate_legacy(&rois, &roi_by);
    let custom = migrated.layouts.iter().filter(|l| !l.built_in).count();
    settings.bearing.layouts = migrated;
    Some(custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用临时目录：放在 target/ 下（一定可写，也不会污染系统临时目录）
    fn tmpdir(tag: &str) -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        let p = base.join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap_or_else(|e| {
            panic!(
                "建临时目录失败 {p:?}: {e}（temp_dir={:?}）",
                std::env::temp_dir()
            )
        });
        p
    }

    #[test]
    fn dev_mode_uses_appdata() {
        let root = tmpdir("dev");
        let d = resolve_data_dir(&root.join("exe"), &root.join("appdata"), false);
        assert_eq!(d.mode, Mode::Dev);
        assert!(d.path.starts_with(root.join("appdata")));
    }

    #[test]
    fn installed_flag_wins() {
        let root = tmpdir("installed");
        let exe = root.join("exe");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::write(exe.join(INSTALLED_FLAG), b"").unwrap();
        let d = resolve_data_dir(&exe, &root.join("appdata"), true);
        assert_eq!(d.mode, Mode::Installed);
        assert_eq!(d.path, root.join("appdata"));
    }

    #[test]
    fn portable_when_exe_dir_writable() {
        let root = tmpdir("portable");
        let exe = root.join("exe");
        std::fs::create_dir_all(&exe).unwrap();
        let d = resolve_data_dir(&exe, &root.join("appdata"), true);
        assert_eq!(d.mode, Mode::Portable);
        assert_eq!(d.path, exe.join("data"));
        assert!(!exe.join("data").join(".write-probe").exists());
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = tmpdir("atomic");
        let path = dir.join(FILE_NAME);
        write_atomic(&path, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":2}");
        // 不留 .tmp
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不该留下临时文件");
    }

    #[test]
    fn store_roundtrips_with_version() {
        let dir = tmpdir("roundtrip");
        let path = dir.join(FILE_NAME);
        {
            let mut s = SettingsStore::load(path.clone());
            s.settings_mut().volume = 42.0;
            s.flush().unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"configVersion\""));
        let again = SettingsStore::load(path);
        assert_eq!(again.settings().volume, 42.0);
        assert_eq!(again.version(), CONFIG_VERSION);
        assert!(!again.is_dirty());
    }

    #[test]
    fn debounce_waits_250ms() {
        let dir = tmpdir("debounce");
        let mut s = SettingsStore::load(dir.join(FILE_NAME));
        s.settings_mut().opacity = 0.5;
        let t0 = Instant::now();
        assert!(!s.save_if_due(t0), "刚改完不该立刻写");
        assert!(!s.save_if_due(t0 + Duration::from_millis(200)));
        assert!(
            s.save_if_due(t0 + Duration::from_millis(260)),
            "过了窗口要写"
        );
        assert!(!s.is_dirty());
    }

    #[test]
    fn migrates_legacy_bearing_rois() {
        let dir = tmpdir("legacy");
        let path = dir.join(FILE_NAME);
        let legacy = r#"{
            "volume": 55,
            "hotkeys": {"togglePlay": "Alt+J"},
            "bearing": {
                "enabled": true,
                "rois": [{"cx": 0.0875, "cy": 0.122, "r": 0.046},
                         {"cx": 0.147, "cy": 0.246, "r": 0.144}],
                "roiBy": {"BV1xx411c7mD": [{"cx": 0.2, "cy": 0.3, "r": 0.12}]}
            }
        }"#;
        std::fs::write(&path, legacy).unwrap();
        let s = SettingsStore::load(path);
        let st = s.settings();
        assert_eq!(st.volume, 55.0);
        assert_eq!(st.hotkeys.get("togglePlay").unwrap(), "Alt+J");
        assert!(s.migrated.iter().any(|m| m.contains("bearing")));
        let custom: Vec<_> = st
            .bearing
            .layouts
            .layouts
            .iter()
            .filter(|l| !l.built_in)
            .collect();
        assert!(
            custom.len() >= 2,
            "种子 ROI 被跳过，其余进 legacy 布局：{custom:?}"
        );
        assert_ne!(st.bearing.layouts.default_layout_id, "builtin");
        // 种子（0.0875/0.122/0.046）不该出现
        assert!(!custom.iter().any(|l| (l.roi.cx - 0.0875).abs() < 1e-6));
    }

    #[test]
    fn migration_is_idempotent_when_custom_layouts_exist() {
        let dir = tmpdir("idem");
        let path = dir.join(FILE_NAME);
        let text = r#"{"bearing":{"rois":[{"cx":0.2,"cy":0.3,"r":0.1}]}}"#;
        std::fs::write(&path, text).unwrap();
        let mut s = SettingsStore::load(path.clone());
        let first = s.settings().bearing.layouts.layouts.len();
        s.flush().unwrap();
        let again = SettingsStore::load(path);
        assert_eq!(
            again.settings().bearing.layouts.layouts.len(),
            first,
            "第二次载入不再迁移"
        );
    }

    #[test]
    fn newer_config_is_backed_up_before_overwrite() {
        let dir = tmpdir("newer");
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, r#"{"configVersion": 9, "volume": 33}"#).unwrap();
        let mut s = SettingsStore::load(path.clone());
        assert_eq!(s.settings().volume, 33.0, "不认识版本也要尽量读");
        s.flush().unwrap();
        let bak = path.with_extension("json.v9.bak");
        assert!(bak.exists(), "应留下原样备份");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!("\"configVersion\": {CONFIG_VERSION}")));
    }

    #[test]
    fn migrates_files_without_overwriting() {
        let from = tmpdir("from");
        let to = tmpdir("to");
        std::fs::write(from.join(FILE_NAME), b"{}").unwrap();
        std::fs::write(to.join(FILE_NAME), b"{\"keep\":1}").unwrap();
        let done = migrate_legacy_data(&from, &to);
        assert!(done.is_empty(), "目标已存在就不覆盖");
        std::fs::remove_file(to.join(FILE_NAME)).unwrap();
        let done = migrate_legacy_data(&from, &to);
        assert_eq!(done, vec![FILE_NAME.to_string()]);
    }
}
