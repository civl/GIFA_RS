# 设置、持久化与迁移（M11）

更新时间：2026-11-22。对应 task.md M11。实现：`crates/gifa-store`。

## 数据目录

| 模式 | 判定 | 位置 |
|---|---|---|
| `Dev` | 非打包运行 | `%APPDATA%\原神跟跑助手\dev` |
| `Installed` | 程序目录有 `installed.flag` | `%APPDATA%\原神跟跑助手` |
| `Portable` | 程序目录可写（真的写一个探针文件） | `<exe>\data` |
| `Fallback` | 上面都不行 | `%APPDATA%\原神跟跑助手` |

绿色版和安装版用 `installed.flag` 区分（原版同款）。探针写文件而不是只看 `exists()` ——
只读目录 `create_dir_all` 也可能成功，必须真的写一次。

## 原子写

```
写 settings.json.tmp（fsync）→ ReplaceFileW 替换 → 失败退化成 rename
```

为什么先写 tmp：直接覆写时如果进程崩在中间，用户丢的是**全部设置**。
`ReplaceFileW` 还会保留原文件的 ACL 与时间戳；目标不存在时直接 `rename`。

## 250ms 防抖

拖动浮窗时每帧都会改 `bounds`，每帧写盘既慢又伤盘。所以：

```
改动 → 标脏(时间戳) → 每帧 save_if_due() → 距上次改动 ≥250ms 才真正写
```

退出、提权重启前必须 `flush()` —— 否则窗口内的改动会丢。demo 在消息循环返回后补了一次。

## 版本与降级保护

配置文件里加了 `configVersion`（原版没有这个字段）。载入时：

- `version == 0`（旧文件）→ 当成当前版本；
- `version > 当前` → **尽量读**（未知字段本来就会保留），但第一次保存前把原文件备份成
  `settings.json.v{版本}.bak`。

这样"降级打开"不会静默吃掉用户的高版本配置。

## 旧版迁移

原版 `settings.json` 的字段与新结构**同名同义**，直接 `Settings::from_json` 就能吃下：

| 原版字段 | 处理 |
|---|---|
| `bounds / pinned / opacity / clickThrough / mode / volume / muted / rate` | 直通 |
| `hotkeys` | 直通（缺的用默认补齐，非法值丢弃） |
| `gamepad.enabled / map / trigger / deadzone…` | 直通（深合并） |
| `bearing.layouts` | 直通 |
| `bearing.rois` / `bearing.roiBy` | **转成 `legacy-N` 命名布局**，跳过两个 SEED 脏数据 |
| `lastText / lastPart / lastVideoId / lastTitle / lastWatchedAt / lastPageCount / lastPageTitle` | 直通（"继续上次观看"） |
| 其它未知字段 | 原样保留（`#[serde(flatten)] extra`），保存时写回去 |

`bearing.rois` 这类旧键在新结构里没有位置，所以 `BearingSettings` 加了 `extra` 兜住它们，
迁移读完就转成布局 —— 只有用户**还没有自定义布局**时才迁移，避免覆盖新数据。

数据文件搬迁：`migrate_legacy_data(from, to)` 搬 `settings.json` / `bearing-dump.png` /
`bearing-frame.png`，**目标已存在就不覆盖**。候选旧目录：`<exe>\data`、
`migrate-from.txt` 里写的路径。

## 自检

```powershell
cd gifa-rs/apps/gifa-demo
cargo run -p gifa-demo -- --settings-test
```

| # | 项 | 实测 |
|---|---|---|
| 1 | 数据目录三种模式 | `dev / portable / installed` 各自路径正确 |
| 2 | 原版 `settings.json` 字段直通 | bounds、hotkey、lastText/lastPart 全部生效 |
| 3 | `bearing.rois` → 自定义布局 | 2 个自定义，SEED 已跳过 |
| 4 | 原子写 + 版本号 + 未知字段保留 | `configVersion=1`，`futureField` 还在，无残留 `.tmp` |
| 5 | 250ms 防抖 | 150ms 不写、300ms 写 |
| 6 | 旧数据目录搬迁（不覆盖） | 只搬了 `bearing-dump.png` |
| 7 | 重复载入不重复迁移 | 布局数 4 → 4 |

`cargo test -p gifa-store` 另有 10 个单元测试（目录解析、原子写往返、防抖状态机、
legacy ROI 迁移、种子跳过、幂等、降级备份、文件搬迁）。

## 已知限制

- demo 仍然用 `cwd/data`（自检需要固定路径）；正式应用启动时用 `resolve_data_dir()`。
- 登录态（`Partitions\Local State` / Cookie）不在搬迁范围：体积大且可能被占用，
  首次启动需要重新扫码 —— 与 M3 的结论一致，不算回退。
- 没有做"设置文件损坏"的自动修复：JSON 解析失败会退回默认并打印一行日志，
  旧文件仍在盘上（用户可手工救）。
