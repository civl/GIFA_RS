# WebView2 运行时基线

更新时间：2026-11-21。对应 task.md §14 第 3 项。

## 结论

| 项 | 值 |
|---|---|
| **最低要求** | Chromium 主版本 **≥ 110**（运行时版本号形如 `110.x.y.z`） |
| **本机实测** | `157.0.4341.60`（Windows 11 26300，2026-11-21） |
| 结论 | 达标；`gifa-demo --runtime` 输出 `RESULT 2/2` |

## 为什么是 110

不是拍脑袋，而是逐条对着我们实际依赖的 API 抬出来的：

| 依赖 | 引入版本 | 用在哪 |
|---|---|---|
| `put_UserAgent` | 86.0.616.0 | 舞台 UA 伪装（去掉 `Edg/`，否则 B 站走不同分支） |
| `put_DefaultBackgroundColor`（透明背景） | 90.0.818.0 | chrome 玻璃窗的逐像素透明 |
| `AddScriptToExecuteOnDocumentCreated` | 早期 | 播放代理每次导航重新注入 |
| `ExecuteScript` | 早期 | Rust → 页面指令 |
| `WebMessageReceived` / `window.ipc` | 早期 | 页面 → Rust 状态上报 |
| **SharedBuffer**（二期帧传输） | **110.0.1587.0** | 把 ROI 像素零拷贝传给 `gifa-vision` |
| `DwmExtendFrameIntoClientArea` | 不是 WebView2 API | 系统 DWM，Win10 1809+ 都有 |

取其中最高的 110。二期如果不做 SharedBuffer（改用自定义协议传字节），
理论上 90 就够；但基线留在 110 更省事，也覆盖了 Edge 的稳定分支。

## 怎么查

```powershell
# 1) 应用自己的探针（不建窗口）
cd apps/gifa-demo
cargo run -p gifa-demo -- --runtime

# 2) 系统层面（注册表）
Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' | Select-Object pv

# 3) 直接看目录
Get-Item 'C:\Program Files (x86)\Microsoft\EdgeWebView\Application\*' | Select-Object Name
```

代码里：

```rust
gifa_shell::webview2_version()   // Ok("157.0.4341.60")
gifa_shell::parse_version(&v)    // Some((157, 0, 4341, 60))
gifa_shell::runtime_ok(&v)       // true
```

启动时 `gifa-shell` 会打一行日志（`GIFA_DEBUG=1` 才可见）：
`[shell] WebView2 运行时 157.0.4341.60（基线 ≥110，达标）`。

## 缺失时会发生什么

运行时不存在时，`wry::webview_version()` 返回错误，`WebViewBuilder::build_as_child`
也会失败，外壳创建报：

```
创建舞台 WebView2 失败: ...
```

`--runtime` 会给出可执行的提示（装 Evergreen Runtime）。
**没有**静默降级 —— 宁可启动失败并说清楚，也不要出一个白窗。

## 分发策略

| 场景 | 做法 |
|---|---|
| Windows 11 | 系统自带，不打包 |
| Windows 10（1803+） | 需用户装 **Evergreen Runtime**；安装包里附检测 + 跳转链接 |
| 企业/离线 | 可选 **Fixed Version** 运行时随包分发（体积 +150MB，暂不采用） |

许可：Evergreen Runtime 允许应用分发/引导安装；若将来改为随包分发，
需在 `resources/licenses/` 补 Microsoft 的运行时许可与隐私说明（见 task.md §12）。

## 未完成项

- [ ] **Win10 22H2 干净机实测**（task.md §14 第 3 项的判据原文）。
      本机只有 Win11 26300，虚拟机/第二台机器上跑 `gifa-demo --runtime` 与 `--check` 才能勾掉。
