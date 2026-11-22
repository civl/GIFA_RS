# gifa-demo

M2 + M3 切片：**真的 B 站页面** + 播放代理 + 解析/登录态。

## 跑

```powershell
cd apps/gifa-demo

cargo run -p gifa-demo -- --open BV1xx411c7mD                 # 直接载入
cargo run -p gifa-demo -- --open "https://b23.tv/AbCdEf"      # 短链
cargo run -p gifa-demo -- --check                             # 8 项自检（约 9s，自动退出）
cargo run -p gifa-demo -- --login                             # 起窗后自动弹登录窗（人工验收）
cargo run -p gifa-demo -- --runtime                           # 只查 WebView2 基线
```

`data/` 是运行数据：`webview2/`（WebView2 profile，登录态与缓存在这里）、`settings.json`（含「上次看到」），不进版本库。

## M3 做了什么

| 能力 | 实现 |
|---|---|
| 输入解析 | BV / av / 完整链接 / 带 `?p=` / 短链；`gifa_player::{extract_id, extract_part, video_url}` |
| 短链 | `b23.tv` / `bili2233.cn` **原样交给浏览器跳**，agent 报回落地地址后接管（不引 HTTP 栈，也不绕过 B 站自己的风控跳转） |
| 视频信息 | 页面内 `fetch('api.bilibili.com/x/web-interface/view')`（同源 + `credentials:'include'`）→ 分 P 列表 / 标题 / UP |
| 登录态 | 页面内 `fetch('/x/web-interface/nav')` → `isLogin/uname/mid/vip/level` |
| 扫码登录 | 独立弹窗 `passport.bilibili.com/login`，**共享同一个 WebContext** ⇒ Cookie 直接落到舞台 profile |
| 登录后 | 轮询 nav 发现 `isLogin` → 关弹窗 → 重载当前视频（B 站只在新文档里给高清） |
| 上次看到 | `gifa_core::Settings` 落盘：`lastText/lastPart/lastVideoId/lastTitle/lastPageCount/lastWatchedAt` |

## 自检项（`--check`，8 项）

时间轴：1.2s 拉 view+nav → 2.6s 校验 → 暂停 → 4.2s 校验 → 跳 60s → 6.2s 校验
→ 1.5× → 7.8s 校验 → 9.0s 出报告。

| # | 项 | 判据 |
|---|---|---|
| 1 | 找到 `<video>` 且有元数据 | `hasVideo && videoWidth>0 && duration>0` |
| 2 | agent 解析 BV 与分 P | `bvid` 非空且 `part>=1` |
| 3 | 标题可读 | `document.title` 非空 |
| 4 | **video 接口** | `pages.len()>=1`（顺带显示 UP 名） |
| 5 | **nav 接口** | 接口返回成功（未登录也是 OK） |
| 6 | 指令：暂停 | `paused == true` |
| 7 | 指令：跳转到 60s | `|currentTime-60| < 15` |
| 8 | 指令：倍速 1.5× | `|rate-1.5| < 0.01` |

实测（2026-11-21）：**8/8**，截图 `docs/m3-bilibili.png`（主窗）、`docs/m3-login.png`（登录弹窗）。

## 已知限制

- 未登录只有 480×360（B 站策略）。登录成功后本 demo 会重载并重新取 view，但**扫码本身需要人拿手机**，
  自动化只能验证到"弹窗打开 + 白名单拦截 + nav 轮询"这一步。
- 短链的 `pending_short` 分支（不透明短码）只做了单元测试；实机验证需要一条有效的 `b23.tv/xxxx` 短链。
- 分 P 下拉在单 P 视频里是隐藏的（这是预期行为）。
