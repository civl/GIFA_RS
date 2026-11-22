# Rust 复刻 GIFA：实施方案与任务书

> **对象仓**：`reference/GIFA`，commit `1f6f0a4`，发布版本 v0.2.6（2026-11-21 克隆）
> **对应产品**：GI-Follow-Assist（原神跟跑助手），GPL-3.0-only
> **本文档性质**：技术方案 + 可执行任务分解（评审稿 v1）
> **读者**：负责 Rust 重写的开发/评审人

---

## 0. 结论摘要（TL;DR）

**一句话方案**：用 **Rust 重写外壳与全部系统层能力**（窗口、置顶/穿透/分层、全局热键、低级钩子、手柄、持久化、视觉识别、叠加层渲染），**用 WebView2 承载 B 站网页播放器**，并把原版 `src/renderer`、`src/console` 的 HTML/CSS 与 `webview-preload.js` 里的页面代理脚本**按 GPL-3.0 原样复用**（只把 Electron 的 preload 桥改成 wry IPC 桥）。

三个必须先达成共识的判断：

| # | 判断 | 依据 | 影响 |
|---|---|---|---|
| 1 | **播放层不能原生重写**，必须保留一个真正的 Chromium/Edge 引擎 | 高清播放依赖登录态 Cookie、B 站播放器 JS、DASH 地址需要 wbi 签名与风控；原版正是靠"真网页"绕开全部这些问题 | 选 WebView2；若要做纯原生（libmpv）需另立项目，见 §3.1 路线 B |
| 2 | **视觉识别的关键在像素 1:1**，不能用窗口截图替代 | `src/webview-preload.js:470` 明确记录：一旦缩放，双线性插值会把青色描边和地形混在一起，`min(g,b)-r` 掉一半 | 帧源必须来自页面内 `drawImage(video)` 或原生解码，**不能**用 WGC/截屏降采样的画面 |
| 3 | **热键必须分成两套机制**，不能只用 `RegisterHotKey` | 鼠标侧键 `RegisterHotKey` 不支持；单键会被系统全局吞掉，用户在任何程序里打不出该键 | 组合键走 `RegisterHotKey`，单键/侧键/按住类动作走 `WH_KEYBOARD_LL`/`WH_MOUSE_LL` 被动监听 |

**工作量估算**：**一期（与原版功能等价）~74 人日**；**二期（视觉原生化）+19 人日**，合计 ~93 人日（1 人 ~19 周 / 2 人 ~9.5 周）。
**MVP 可用闭环**（能看视频 + 能置顶 + 能暂停）：**~31 人日**。

### 0.1 已确认决策（Decision Log）

| # | 决策 | 确认于 | 后果 |
|---|---|---|---|
| **D1** | **路线 A**：Rust 外壳（tao/wry + WebView2 + 原生 Win32）+ 复用原版 HTML/CSS 与页面代理（GPL 同源） | 2026-11-21 | 不评估 libmpv 路线；§3.1 的 B/C 仅作背景保留 |
| **D2** | **视觉一期留在页面 JS**：`player-agent.js` 沿用原版识别逻辑，只把外壳换成 Rust；`gifa-vision` 二期再移植 | 2026-11-21 | 一期不定义帧传输；`gifa-vision` 一期只落 `trait FrameSource` 空壳；黄金回归随二期 |
| **D3** | **浮窗 = 两层窗口**：「舞台窗」（只有 B 站 WebView2）+「独立顶层 chrome 玻璃窗」（`DwmExtendFrameIntoClientArea(-1)` + `SetWindowRgn` 挖空） | 2026-11-21（spike 实测） | chrome 层不能做成子窗；详见 `gifa-rs/spikes/dual-webview/FINDINGS.md` |

**D2 带来的第一条硬约束（很容易被"顺手重构"破坏）**：

> 一期裁剪 `webview-preload.js` 时，必须**完整保留**识别部分——`BEARING` 状态机、`analyze`、`analyzeCone`、`smoothBearing`、`updateView`、自动定心全套、`tickBearing`、`sendBearing`。只允许改"与 Electron 绑定的部分"：`ipcRenderer.*` → IPC 桥、`webview` 注入方式、`sendToHost` → 桥。
> 理由：二期的对拍基线就是这份 JS。任何"我觉得可以简化"的改动都会让二期失去可比对的 ground truth。

**由此调整的两件事**：

1. **一期风险显著下降**：最难调参的视觉部分不在关键路径上，`§9 R1` 的概率/影响降为中/中，且只作用于二期。
2. **一期交付即可替代原版日常使用**（P3 出口），二期是"去掉页面侧识别负载"的优化，不是功能补齐。

**一期真正的头号风险**因此变成 **R7（多 WebView2 同窗的层级/命中测试）** 与 **R3（透明分层窗的驱动兼容）**——两者都是"能不能把原版的窗口行为一比一还原"的问题，建议 P0 阶段就先用一个最小 demo 打穿，不要留到集成时。

> ✅ **该 spike 已于 2026-11-21 完成并跑通**：`gifa-rs/spikes/dual-webview`（5/5 通过，含截图）。
> 结论见 **D3** 与 `FINDINGS.md`；R7 因此降级为低/低。

---

## 1. 复刻目标

### 1.1 必须复刻（P0，缺一不可）

- [ ] 启动器（所有设置入口）+ 跟跑浮窗（无边框置顶播放器）双窗口结构
- [ ] **永远置顶**：压住窗口化/无边框游戏；层级被抢后自动夺回；显示时不抢游戏焦点
- [ ] **全局快捷键**：游戏全屏、鼠标在游戏里也生效；组合键 + 单键 + 鼠标侧键 + 按住类动作
- [ ] **B 站网页版播放器**：BV/av/完整链接/`b23.tv` 短链/`?p=3` 分 P 链接解析；自动点"网页全屏"
- [ ] **扫码登录**：标题栏登录 → B 站扫码窗 → 成功广播 → 高清重载；登录态跨启动保留
- [ ] **分 P 切换**：读取分 P 列表、下拉切换、快捷键上下切、自动连播跟随
- [ ] **鼠标穿透**、**不透明度**、**音量/静音/倍速**
- [ ] **纯视频沉浸**：隐藏全部控件、锁定移动缩放、按视频原比例贴合、退出精确还原
- [ ] **位置/尺寸/设置持久化**，便携版与安装版两种数据目录
- [ ] 全局 UAC 提权（游戏以管理员身份运行时快捷键仍生效）

### 1.2 应当复刻（P1）

- [ ] 小地图方位标记：红箭头（行进方向）+ 绿弧（镜头视角）双通道、独立滤波、tween 补间（**一期识别复用页面 JS**，见 D2）
- [ ] 方位标定：内置布局（影月月/穿山甲）、自定义命名布局、拖动圆环 + 滚轮调半径、自动定心
- [ ] 手柄八方向圆盘：L3 按住弹出、右摇杆选动作、锁存/回中阈值、按住类动作用摇杆位置代偿
- [ ] 鼠标移入自动变淡（沉浸模式下 60ms 轮询 + 120ms 防抖 → 10% 不透明度）
- [ ] 长按临时加速（松开恢复）、循环倍速
- [ ] 纯净模式（隐藏顶栏/推荐/评论/广告）
- [ ] 上次看到（视频 + 分 P + 标题 + 时间）、启动自动续播
- [ ] 启动诊断日志（含 128KB 滚动上限）

### 1.3 二期目标（视觉原生化）

- [ ] `gifa-vision` 独立 crate：识别不再依赖页面 JS，输入为 RGBA 帧
- [ ] 帧通道：页面只负责 `drawImage(video)` 取证像素，几何与滤波都在 Rust
- [ ] 500 帧黄金回归对拍达标（§7.5），并作为 CI 门禁

> 一期的方位标记**功能不降级**：识别仍由页面代理里那份原版 JS 完成，Rust 只做"收角度 → 转叠加层"。

### 1.4 明确不做（Non-goals）

- 弹幕发送、视频下载、投屏、多账号管理
- Linux / macOS 支持（原版的钩子、XInput、分层窗口全是 Windows 专属；跨平台会把工作量翻倍）
- "导航级"路径规划：原版自述误差 ±10~20°，本复刻不承诺更高精度
- 反向工程 B 站私有播放接口来绕开网页（见 §12 合规）
- 复刻 Electron 时代的 Chromium 沙箱兜底（`src/sandbox-fallback.js`）——Rust 版不存在这个缺陷，直接删除

### 1.5 验收定义

"复刻成功"= 在 Windows 11 + 原神（窗口化/无边框）+ 真实 B 站视频 上，完成 §11 全部勾选项，且**行为差异表**（§3.3）中每一项都已逐条确认。

---

## 2. 原版现状解剖

### 2.1 技术栈与体量

| 项 | 值 |
|---|---|
| 运行时 | Electron 45.2.3（Chromium + Node 22.12+/24） |
| 打包 | electron-builder 26.18.0（NSIS + portable） |
| 生产依赖 | **只有一个**：`uiohook-napi@1.5.5`（libuiohook 的 N-API 绑定） |
| 源码体量 | 11,054 行（JS/CSS/HTML），其中 `webview-preload.js` 2,777、`main.js` 2,147、`console.js` 996、`renderer/app.js` 948 |
| 许可 | GPL-3.0-only（v0.2.4/0.2.5 历史版本另有 MIT） |
| 提权 | 清单 `requestedExecutionLevel: requireAdministrator` |
| 语言资源 | 仅 zh-CN + en-US |

### 2.2 窗口与进程模型

原版是**单主进程 + 4 类顶层窗口**（Electron 每个 `BrowserWindow` 自带渲染进程）：

| 窗口 | 变量 | 关键属性 | 职责 |
|---|---|---|---|
| 启动器 | `consoleWin` | 1280×720（16:9，按工作区等比缩放到 ≥800×450）、`titleBarOverlay` 深色、可最小化/最大化 | 四个分页（启动 / 方位标定 / 浮窗 / 快捷键）+ 账号行 + 提权横幅；**关闭启动器 = 退出整个应用** |
| 跟跑浮窗 | `win` | 无边框、`alwaysOnTop: 'screen-saver'`、最小 240×150、`webviewTag: true`、`disableHtmlFullscreen` | 自绘标题栏(30px) + 控件条(34px) + 16:9 舞台(896×504) + 拖角手柄 |
| 方位叠加层 | `overlayWin` | 透明、无边框、`skipTaskbar`、`focusable:false`、点击穿透、尺寸 = `radius × 2.24` | canvas 画红箭头 + 绿弧；放置模式下可拖动 |
| 手柄圆盘 | `discWin` | 同叠加层，尺寸 = `disc.radius × 2.24` | 八扇区圆盘 + 高亮 + 中心提示 |
| 登录窗 | `loginWin` | 普通窗口，B 站域名白名单放行子窗口 | 扫码登录，与播放器共用 `persist:bili` 分区 |

**渲染层内的第三层**：浮窗里的 `<webview>` 标签承载 `www.bilibili.com/video/BVxxx/?p=N&autoplay=1`，其 preload（`webview-preload.js`）承担三件事：播放器控制、页面纯净样式、**像素级视觉识别**。

### 2.3 数据流

```
                        ┌──────────────── 启动器 (console) ────────────────┐
                        │ 设置/快捷键录制/布局管理/账号态                     │
                        └───────────▲───────────────────┬─────────────────┘
                                    │ ipc invoke        │ broadcast settings:changed
   ┌────────────────────────────────┴───────────────────▼──────────────────────────┐
   │                         主进程 (main.js)  —— 唯一真相源                        │
   │  设置持久化 · 置顶/穿透/透明度 · 沉浸 · 热键(RegisterHotKey + LL Hook)          │
   │  手柄状态机 · B 站解析 · 布局 CRUD · 窗口生命周期 · 提权                       │
   └───┬───────────────┬────────────────┬──────────────────┬───────────────────────┘
       │ bearing(send) │ player(cmd)    │ disc:ui          │ float:cmd
       ▼               ▼                ▼                  ▼
  ┌─────────┐   ┌──────────────┐   ┌──────────┐    ┌────────────────────┐
  │ 叠加层   │   │ 浮窗 webview │   │  圆盘     │    │ 浮窗 renderer      │
  │ canvas  │◄──│ (B 站页面)   │   │ canvas   │    │ 标题栏/进度/标定    │
  └─────────┘   └──────┬───────┘   └──────────┘    └────────────────────┘
                 每帧 │ rVFC 触发 tickBearing()
                      ▼
              drawImage(video) → getImageData → 青色三角/白色扇形 → 角度
```

关键点：**识别端在页面进程内**，因为它必须读 `<video>` 的真实像素；主进程只收到 `{angle, view, confidence, ...}` 几十字节。

### 2.4 模块清单与移植去向

| 原文件 | 行数 | 职责 | Rust 版去向 |
|---|---|---|---|
| `src/main.js` | 2147 | 全部窗口/系统能力/生命周期 | `crates/gifa-app`（拆成 `win/`、`state/`、`bili/`、`ipc/`） |
| `src/webview-preload.js` | 2777 | 播放器控制 + 纯净样式 + 视觉识别 | **一期整体留在页面代理 JS**（仅改 IPC 桥）；二期再把识别移植到 `gifa-vision` |
| `src/hook-hotkeys.js` | 349 | libuiohook 低级钩子、按下/松开、rearm | `crates/gifa-input/src/hook.rs`（自研 `SetWindowsHookExW`） |
| `src/hotkey-config.js` | 145 | 加速器解析/交换冲突事务 | `crates/gifa-core/src/hotkey.rs` |
| `src/gamepad.js` | 265 | PowerShell + XInput 轮询子进程 | `crates/gifa-input/src/gamepad.rs`（`gilrs` 或直连 XInput） |
| `src/gamepad-config.js` | 321 | 帧归一化、扇区量化、锁存状态机 | `crates/gifa-input/src/chord.rs`（纯函数，好测） |
| `src/bearing-layouts.js` | 170 | 布局 CRUD / 旧版迁移 | `crates/gifa-core/src/bearing/layout.rs` |
| `src/sandbox-fallback.js` | 239 | Chromium 沙箱失败兜底 | **删除** |
| `src/preload.js` | 47 | contextBridge 桥 | 换成 wry IPC shim（约 150 行 JS） |
| `src/renderer/*` | 948+ | 浮窗 UI + 标定交互 | **复用 HTML/CSS**，`app.js` 改桥接层 |
| `src/console/*` | 996+ | 启动器 UI | **复用 HTML/CSS**，`console.js` 改桥接层 |
| `src/overlay/*`、`src/disc/*` | ~250 | canvas 绘制 | 一期复用；二期换 `tiny-skia` 原生分层窗 |
| `scripts/vision-probe/*` | — | 视觉实验/回归 | 转成 Rust 的 golden-frame 回归（§7.5） |

### 2.5 六条不容忽视的实现约束

1. **为什么必须是真网页**：`buildUrl()` 固定用 `www.bilibili.com/video/{id}/?p={n}&autoplay=1`，注释写明站外嵌入播放器（`player.bilibili.com`）无论如何上不了高清；高清依赖登录 Cookie + 网页播放器的画质协商。
2. **为什么帧必须来自 `drawImage(video)`**：见 §0 判断 2；`centerWindow()` 还特意用**整数步长隔点采样**（`step: size > 320 ? 2 : 1`）而不是缩放，因为整数采样保持原始颜色值。
3. **为什么箭头要算连通块**：小地图上还有传送锚点、神瞳、水域等青色零散块，任何一个比箭头伸得远都会劫持"最远点"。实测能把角度带偏 100°+。
4. **为什么不能取"最远点"当朝向**：角色箭头是"钝鼻 + 两个尖尾"的飞镖形，最远的是**尾巴**，直接取会反 180°。正解是取**半径最小且最钝**的角（鼻尖）。
5. **为什么要管理员权限**：UIPI 限制——非提权进程无法向提权进程（游戏）注入/投递，低级钩子在不提权时收不到提权前台窗口的输入。
6. **为什么用 `screen-saver` 层级**：Windows 上它比普通 `alwaysOnTop` 更能压住无边框全屏窗口；且需要每 2.5s 巡检 + 每次 `blur` 时重新压制。

---

## 3. 架构选型

### 3.1 三条路线

| | **A. Rust 外壳 + WebView2（推荐）** | B. 纯原生 + libmpv | C. Tauri 2 |
|---|---|---|---|
| 播放 | B 站真网页（原版同款） | 需自建 DASH 取流 + wbi 签名 + 风控 | 同 A |
| 视觉帧源 | 页面内 `drawImage`（1:1，零妥协） | mpv 帧回调 / 解码后帧（更好，但要自建管线） | 同 A |
| 保真度 | **极高**，DOM/CSS 选择器、播放器交互全部照搬 | 中，播放器行为需重写 | 高 |
| 工作量 | **一期 ~74 人日**；含视觉原生化 ~93 | 250+ 人日，且随时被 B 站改版打断 | ~100 人日（框架约束带来的返工） |
| 依赖 | WebView2 Evergreen（Win10 1803+ / Win11 自带） | 无浏览器依赖，但需 ffmpeg/libmpv | 同 A |
| 关键短板 | 仍需 Edge 运行时；两层窗口（舞台+chrome）的位置/z-order/区域联动要自己写（spike 已验证可行） | 合规与稳定性风险极高 | 窗口语义控制不如裸 wry 直接 |

**选 A。** 理由：本项目的价值 80% 在"把 B 站网页压到游戏上并读懂小地图"，而不是"自己写一个播放器"。B 路线会把 80% 的工程量花在与产品差异化无关的流媒体逆向工作上，并且每次 B 站改接口都要跟进。

> **保留逃生口**：把帧源定义成 `trait FrameSource`（§7.3），视觉管线只依赖 trait。将来若要转 B 路线，替换帧源实现即可，识别代码零改动。

### 3.2 推荐架构分层

```
┌─────────────────────────── 页面层（复用原版前端资源，GPL 同源） ───────────────────────────┐
│  launcher.html/js   float.html/js   overlay.html/js   disc.html/js                        │
│  player-agent.js（由 webview-preload.js 裁剪：播放控制 + 纯净样式 + 帧取样）                 │
└───────────────────────────────────────▲───────────────────────────────────────────────────┘
                                        │  wry IPC (JSON) / 自定义协议 (二进制帧)
┌───────────────────────────────────────┴───────────────────────────────────────────────────┐
│                              gifa-app（Win32 外壳 + 状态机 + IPC 路由）                      │
│  win: 舞台窗/顶层 chrome 窗/置顶/穿透/透明度/DPI     hotkey: RegisterHotKey + LL Hook        │
│  input: 手柄 + 圆盘状态机                          state: 设置/布局/持久化                   │
│  bili: 链接解析 + 视频信息（页面内 fetch）          ipc: 命令枚举 → 处理器                    │
└───────────────┬───────────────────────────────────────┬────────────────────────────────────┘
                │ 帧 (RGBA ROI)                          │ 角度 / 置信度
┌───────────────▼───────────────┐        ┌──────────────▼─────────────────────────────────┐
│  gifa-vision（纯算法 crate）   │───────►│  bearing pipeline: 滤波/门控 → 输出事件          │
│  arrow / cone / filter / auto │        └──────────────┬─────────────────────────────────┘
└───────────────────────────────┘                       │
                        ┌───────────────────────────────▼─────────────────┐
                        │ overlay 渲染（canvas 或 tiny-skia 分层窗）        │
                        └─────────────────────────────────────────────────┘
```

**窗口分层（D3，spike 已验证）**：

```
        ┌────────── W_CHROME：独立顶层窗（WS_EX_TOPMOST|TOOLWINDOW|NOACTIVATE）──────────┐
        │  DwmExtendFrameIntoClientArea(-1)  ⇒ 逐像素 alpha，能透出下面的舞台窗            │
        │  SetWindowRgn(区域 = 顶栏 ∪ 底栏 ∪ URL条 ∪ 提示 ∪ 标定环) ⇒ 其余区域点击穿透       │
        │  内含 chrome WebView2（我们自己的 HTML/CSS，body 透明）                          │
        └──────────────────────────────────────────▲────────────────────────────────────┘
                                                   │ 区域外的点击直接落到下层
        ┌──────────────────────────────────────────┴────────────────────────────────────┐
        │  W_FLOAT：浮窗（WS_EX_LAYERED + LWA_ALPHA ⇒ 透明度一次作用于整棵窗口树）             │
        │  内含 stage WebView2（www.bilibili.com），铺满客户区                               │
        └───────────────────────────────────────────────────────────────────────────────┘
```

**为什么 chrome 必须是独立顶层窗**：子窗里的透明 WebView2 只能透出「自己这块表面」的背景，透不到兄弟窗；spike 对照组实测 pill 采样 `(18,19,19)`（自己的背景刷混色）而不是理论值 `(52,14,14)`。改成顶层玻璃窗后 `(52,14,14)` 精确命中。

> **一期的实际形态（D2）**：上图中间的 `gifa-vision` 链条**不启用**。识别留在页面代理 `player-agent.js` 里，直接把 `{angle, view, viewConfidence, viewWidth, hold, ...}` 经 IPC 交给 Rust；Rust 只做"转给叠加层 + 落日志"。二期再把「帧 (RGBA ROI) → gifa-vision」这条通道打开，页面退化为纯帧源。

### 3.3 Electron → Rust 行为映射表（**评审必须逐条确认**）

| 能力 | Electron 做法 | Rust 做法 | 差异与风险 |
|---|---|---|---|
| 置顶 | `setAlwaysOnTop(true,'screen-saver')` | `SetWindowPos(HWND_TOPMOST)` + `WS_EX_TOPMOST`；2.5s 巡检 + `WM_ACTIVATE`/`WM_WINDOWPOSCHANGED` 重压 | 无 `screen-saver` 语义，层级竞争下可能被同为 topmost 的窗口压住；巡检必须覆盖**所有**标记窗（原版 GIFA-011 缺陷，顺手修掉） |
| 不抢焦点 | `showInactive()` | `ShowWindow(SW_SHOWNOACTIVATE)` + `SetWindowPos(SWP_NOACTIVATE + SWP_SHOWWINDOW)`；叠加层/圆盘加 `WS_EX_NOACTIVATE` | 首次显示时 WebView2 可能自己抢焦点，需在 `NavigationCompleted` 后 `SetFocus(NULL)` |
| 浮窗结构 | 一个 `BrowserWindow` + `<webview>` 标签 | **两个顶层窗**：舞台窗（只装 B 站 WebView2）+ chrome 玻璃窗（`DwmExtendFrameIntoClientArea(-1)`） | 子窗里透明 WebView2 透不到兄弟窗（spike 对照实测）；两窗需同步位置与 z-order |
| 鼠标穿透 | `setIgnoreMouseEvents(true,{forward:true})` | **`SetWindowRgn` 把 chrome 窗挖空**：区域=顶栏∪底栏∪浮动控件，其余不落窗口 | spike 已验证：区域外 `WindowFromPoint` 落到舞台；`forward:true` 的"鼠标移动仍上报"用 60ms 轮询 `GetCursorPos` 实现 |
| 透明度 | `setOpacity(v)` | 舞台窗 `WS_EX_LAYERED` + `SetLayeredWindowAttributes(hwnd, 0, alpha*255, LWA_ALPHA)` | **实测会连子 WebView2 一起变淡**，一次搞定；但 DWM 重合成有延迟，脚本化采样必须等 ≥600ms |
| 无边框拖动 | `-webkit-app-region: drag` | `WM_NCHITTEST → HTCAPTION` 命中标题栏区，或在 JS 里 `mousedown` → IPC → `ReleaseCapture()+SendMessage(WM_NCLBUTTONDOWN,HTCAPTION)` | WebView2 内容窗口需把 hit-test 结果上报给宿主（`ICoreWebView2CompositionController` 走 composition 模式） |
| 拖角缩放 | HTML `#grip` + 手动 setBounds | `WM_NCHITTEST → HTBOTTOMRIGHT`，让 `DefWindowProc` 走系统缩放循环 | 16:9 约束需在 `WM_SIZING` 里按 `RECT` 修正 |
| 窗口全屏 | `setFullScreen()` | `WM_SYSCOMMAND`/手动改 style；建议自实现 `SetWindowLongPtr` 去边框 + 记录还原矩形 | 与沉浸模式互斥（原版亦如此） |
| 全局热键 | `globalShortcut` | `RegisterHotKey`（每绑定一个 HWND 消息 `WM_HOTKEY`） | 完全等价（Electron 在 Windows 上就是它） |
| 单键/侧键/按住 | libuiohook（`WH_*_LL`） | 自研 `SetWindowsHookExW(WH_KEYBOARD_LL/WH_MOUSE_LL)`，**独立线程 + 消息泵** | 钩子会被"超时"摘掉；原版靠 `minimize`/`restore` 时 `rearm()`，我们再加一次 `WM_POWERBROADCAST` 后重挂 |
| 手柄 | PowerShell 子进程调 XInput | `gilrs`（XInput 后端，原生热插拔）或直连 `XInputGetState` | 帧率 16ms；轮询不得依赖焦点 |
| 屏幕/DPI | `screen.*` 事件 | `EnumDisplayMonitors` + `WM_DISPLAYCHANGE` + `WM_DPICHANGED` + `GetDpiForWindow` | **必须** Per-Monitor-V2（清单），否则 150% 下坐标全错 |
| 数据目录 | `app.getPath('userData')` | `%APPDATA%\原神跟跑助手` 或 `exe目录\data`（探写权限） | 保持同名同路径，便于用户无痛迁移 |
| 单实例 | `requestSingleInstanceLock` | 命名互斥体 `CreateMutexW` + 第二实例用 `WM_COPYDATA`/自定义消息唤回启动器 | 需处理"唤回时机早于窗口创建" |
| 防休眠 | `powerSaveBlocker('prevent-app-suspension')` | `SetThreadExecutionState(ES_CONTINUOUS + ES_SYSTEM_REQUIRED + ES_DISPLAY_REQUIRED)` | 退出时务必清除，否则屏幕永不休眠 |
| 提权 | 清单 `requireAdministrator` | 同（`embed-manifest` 在 `build.rs` 注入） | 同一 UAC 体验 |
| 自动播放 | 页面 `autoplay=1` + 静音占位 | 同（WebView2 默认允许自动播放；必要时 `--autoplay-policy`） | 需实测：Edge 对有声自动播放的策略 |

### 3.4 混合前端策略（省掉约 40% 工作量）

原版前端是**纯静态资源**（HTML/CSS/原生 JS，无框架、无构建步骤），与许可同源，可直接搬：

- **原样复制**：`src/renderer/index.html`、`style.css`、`src/console/index.html`、`console.css`、`src/overlay/*`、`src/disc/*`
- **改桥接层**：`app.js` / `console.js` 里的 `window.api.*`（Electron preload 暴露）换成 wry 的 `invoke()`/`window.ipc.postMessage`；`ipcRenderer.on` 换成 `window.__gifa.on(channel, cb)`
- **不动**：所有 CSS 变量、布局、动画

**代价**：需要写一个约 150 行的 JS shim，把 `api` 对象映射到 Rust 命令；以及一套 Rust → JS 的事件分发（按窗口路由）。

### 3.5 渲染 vs 原生的取舍（建议分期）

| 部件 | 一期（求快、保真） | 二期（去 WebView2 负荷） |
|---|---|---|
| 浮窗 UI | 复用 HTML（WebView2 子窗） | 维持 |
| 叠加层 | 透明 WebView2 + 复用 `overlay.js` | `tiny-skia` → 预乘 BGRA → `UpdateLayeredWindow` |
| 圆盘 | 透明 WebView2 + 复用 `disc-draw.js` | 同上 |

二期收益：少 2 个 WebView2 进程（约 −120MB 内存）、消除"透明窗首帧闪白"、穿透与非激活行为 100% 可控。**但一期必须先把功能跑通**，不要在第一天就追求原生绘制。

---

## 4. 目标工程结构

> **当前进度（2026-11-21）**：`gifa-shell`（外壳）与 `apps/shell-demo`（骨架演示+自检）已落地，
> 下面树里的 `gifa-core` / `gifa-vision` / `gifa-input` / `gifa-app` 仍是规划。
> 实际目录见 `gifa-rs/README.md`。

```
gifa-rs/
├─ Cargo.toml                      # workspace
├─ build.rs                        # 清单注入：requireAdministrator + PerMonitorV2 DPI
├─ crates/
│  ├─ gifa-core/                   # 无 Win32 依赖的领域模型（可跑单测/CI）
│  │  ├─ settings.rs               #   Settings 结构 + 合并/校验/迁移
│  │  ├─ bearing/layout.rs         #   Layout / ROI / CRUD / 旧版迁移
│  │  ├─ hotkey.rs                 #   加速器解析、冲突交换事务
│  │  ├─ part.rs                   #   URL ↔ BV/av/p 解析
│  │  └─ state.rs                  #   纯状态机（沉浸/穿透/透明度/倍速）
│  ├─ gifa-vision/                 # 纯算法：输入 RGBA，输出角度（无 OS 依赖）——二期启用（D2）
│  │  ├─ color.rs                  #   cyan_score / whiteness
│  │  ├─ roi.rs                    #   center_window / crop（整数步长）
│  │  ├─ arrow.rs                  #   阈值→连通块→径向剖面→三角评分
│  │  ├─ cone.rs                   #   极坐标格→边检测→视锥
│  │  ├─ filter.rs                 #   α-β 滤波 + 180° 门限
│  │  ├─ view.rs                   #   视角通道（独立状态机）
│  │  ├─ autocenter.rs             #   质心投票定心
│  │  └─ pipeline.rs               #   tick() 编排
│  ├─ gifa-input/                  # 热键与手柄（只依赖 windows crate）
│  │  ├─ hotkey_win.rs             #   RegisterHotKey 封装
│  │  ├─ hook.rs                   #   LL 钩子线程 + 重挂
│  │  └─ gamepad.rs                #   gilrs 轮询 → Frame
│  ├─ gifa-shell/                  # 窗口与 WebView2（最脏的一层）
│  │  ├─ float.rs                  #   舞台窗：B 站 WebView2 + LWA_ALPHA + WM_NCCALCSIZE
│  │  ├─ chrome.rs                 #   顶层 chrome 玻璃窗：DWM 玻璃 + 区域挖空
│  │  ├─ launcher.rs               #   启动器窗口
│  │  ├─ overlay.rs                #   方位叠加层（同一套玻璃窗机制）
│  │  ├─ region.rs                 #   区域合成：矩形并集 → SetWindowRgn
│  │  ├─ overlay_draw.rs           #   一期 canvas 桥 / 二期 tiny-skia
│  │  └─ layout.rs                 #   clamp/bounds/多屏/两窗联动
│  └─ gifa-app/                    # 二进制：组装 + IPC 路由 + 生命周期
│     ├─ main.rs
│     ├─ ipc.rs                    #   Command 枚举（serde）
│     ├─ bili.rs                   #   resolve/view/nav
│     ├─ persist.rs                #   原子写 + 迁移
│     └─ log.rs                    #   tracing + 128KB 滚动
└─ assets/
   ├─ web/                         # 复用原版 HTML/CSS/JS（含 player-agent.js）
   └─ build/                       # icon.ico / installer*.bmp / LICENSE
```

### 4.1 线程模型（约定）

| 线程 | 职责 | 硬约束 |
|---|---|---|
| **Main**（Win32 消息泵） | 所有 HWND 创建/修改、`tao` EventLoop、IPC 路由 | Win32 要求窗口只能由创建它的线程操作；**禁止**在此做阻塞 HTTP/重计算 |
| **Vision**（专用线程） | 收帧 → `gifa-vision` → 发 `BearingOut` | 目标 <8ms/帧；队列长度 1，丢旧帧（延迟优先） |
| **Hook**（专用线程） | `SetWindowsHookExW` + `GetMessage` 泵 + 键位映射 | 钩子回调必须 <1ms，只做 `mpsc::try_send` |
| **Gamepad**（专用线程） | 16ms 轮询 `gilrs` | 静默时也必须轮询（要检测按下） |
| **Async**（tokio multi-thread） | `b23.tv` 跳转、`api.bilibili.com` 兜底请求、定时器 | 页面内 fetch 优先，减少这里的使用 |
| **Overlay**（main 线程定时器） | tween 逐帧重绘（16ms） | 大跳 >90° 不做补间（原版行为） |

通信统一用 `crossbeam-channel`（跨线程）+ `tokio::sync::mpsc`（异步侧）。**禁止**共享可变状态跨线程，全部用消息。

---

## 5. 依赖选型

| 用途 | crate | 备注 |
|---|---|---|
| 窗口/事件循环 | `tao` 0.30+ | 与 wry 配套；提供 DPI/monitor API |
| WebView 承载 | `wry` 0.50+ | WebView2 后端；`with_ipc_handler` / `with_custom_protocol` |
| WebView2 深度操作 | `webview2-com` 0.36+ | CookieManager、`CapturePreview`、`DefaultBackgroundColor`、SharedBuffer |
| Win32 | `windows` 0.60+ | `Win32_UI_WindowsAndMessaging`、`Win32_UI_Input_KeyboardAndMouse`、`Win32_Graphics_Dwm`、`Win32_System_Threading`、`Win32_UI_HiDpi` |
| 全局热键 | `global-hotkey` 0.7 | 底层即 `RegisterHotKey`；也支持 `GlobalHotKeyEvent` 路由 |
| 低级钩子 | 自研（`windows` + 约 300 行） | 不用 `rdev`：它不允许细粒度忽略注入事件、也不能按 token 去重 |
| 手柄 | `gilrs` 0.11 | XInput 后端；比原版 PowerShell 方案可靠得多 |
| HTTP | `reqwest` 0.12（rustls） | 仅 `b23.tv` 跳转与兜底 |
| 序列化 | `serde` + `serde_json` | 设置文件与原版**同构**，便于对拍与迁移 |
| 日志 | `tracing` + `tracing-appender`（rolling） | 128KB 上限，同 `startup.log` |
| 单实例 | 自研命名互斥体（`windows`） | 比 `single-instance` crate 更能定制"唤回"行为 |
| 原子写 | 自研 `tmp + rename` | Windows 上 `ReplaceFileW` 更稳 |
| 图像/绘制 | `tiny-skia` 0.11（二期）、`image` 0.25（仅调试导出） | **不要**在识别链路里用 `fast_image_resize` |
| 错误 | `anyhow` + `thiserror` | 库层 `thiserror`，应用层 `anyhow` |
| CLI | `clap` 4 | `--open BVxxx`、`--bearing`、`--elevated` |
| 清单注入 | `embed-manifest` 2 | 在 `build.rs` 里写 `requireAdministrator` + `PerMonitorV2` |

> **明确移除**：`uiohook-napi`（LGPL 原生模块）→ 自研钩子；`electron`/`electron-builder` → `cargo` + NSIS。

---

## 6. 任务分解

> 估算单位为**人日**（含自测，不含评审与实机验收）；每项给了"原版行为锚点"以便逐条对照。
>
> **分期（D2）**：`M0~M13` 为**一期**，出口是"与原版 v0.2.6 功能等价"；`M8` 为**二期**（视觉原生化）。一期**不做** M8，方位标记靠页面 JS 那份原版识别直接可用。

### M0 工程骨架与生命周期 `3d` [P0]

- [ ] workspace + 5 个 crate（4 库 + 1 二进制）骨架 + `cargo fmt` / `clippy -D warnings`
- [ ] 清单：`requireAdministrator` + `PerMonitorV2` + `longPathAware`
- [ ] CLI：`--open BV1xx?p=2`、`--bearing`、`--elevated`（提权标记，用于避免循环重启）
- [ ] 单实例：`CreateMutexW` → 第二实例发自定义消息唤回启动器（启动器未建时先建）
- [ ] 数据目录解析：`installed.flag` → `%APPDATA%\原神跟跑助手`；否则探写 `exe\data`，失败回退
- [ ] `startup.log`：128KB 滚动；记录版本、模式、钩子/手柄状态
- [ ] `SetThreadExecutionState(ES_CONTINUOUS|ES_SYSTEM_REQUIRED)` 防挂起
- [ ] `WM_POWERBROADCAST`：suspend/lock → 停手柄、取消临时加速；resume → `rearm()` 钩子
- **锚点**：`main.js:1676-2147`（`app:boot`、生命周期）
- **验收**：双击 exe 弹一次 UAC；第二次双击唤回已开启动器；`%APPDATA%` 下出现 `data/` 与 `startup.log`

### M1 窗口基座 `8d` [P0] ✅ 骨架完成（spike 1d + 实现 3d，剩 4d 收尾）

- [x] 前置 spike：`gifa-rs/spikes/dual-webview`（5/5），结论见 `FINDINGS.md`
- [x] 两层窗口实现：`crates/gifa-shell`（舞台窗 + 顶层 chrome 玻璃窗）
- [x] 无边框 + 可缩放：`WM_NCCALCSIZE`（客户区=整窗，最大化时补边）+ `WM_GETMINMAXINFO` 最小尺寸
- [x] 拖动/缩放入口：页面 `{t:'drag'}` / `{t:'resize'}` → `SendMessage(WM_NCLBUTTONDOWN, HTCAPTION/HTBOTTOMRIGHT)` 走系统模态循环
       ↳ 真实鼠标拖角实测：896×568 → 1038×682，chrome 跟随
- [x] 两窗联动：`WM_WINDOWPOSCHANGED/WM_SIZE/WM_MOVE/WM_DPICHANGED` → 同步位置/尺寸/舞台矩形/区域
- [x] z 序联动：chrome 的 owner = 舞台窗（随最小化/销毁自动跟随，无需自己维护层级）
- [x] 区域随 UI 状态重算：页面上报 `{t:'layout', titlebar, footer, urlbar, overlay[]}`（物理像素）→ 重算舞台矩形 + 区域
       ↳ URL 条展开实测：舞台 504→464，区域 4→5 块
- [x] 透明度：舞台窗 `LWA_ALPHA`；chrome 侧改页面 `opacity`（用 LWA 会破坏逐像素透明）
- [x] 位置钳制：`layout::clamp_to_work` / `default_bounds`（含 4 条单测）
- [x] 非激活显示：`SW_SHOWNOACTIVATE` + chrome 的 `WS_EX_NOACTIVATE` / `WM_MOUSEACTIVATE→MA_NOACTIVATE`
- [ ] 置顶巡检接到定时器（`reassert_topmost` 已实现，等应用层 2.5s 定时调用）
- [ ] 移动/缩放落盘（`bounds()` 已实现，等设置层接入）
- [ ] 旧配置 `aspect != 2 && width < 720` 的一次性 16:9 纠正
- [ ] 多显示器增删（`WM_DISPLAYCHANGE`）时重新钳制
- **锚点**：`main.js:653-700`（clampBounds）、`686-700`（applyPin）、`701-810`（createWindow）
- **验收**：`cargo run -p shell-demo` 6/6；`--drag-test` PASS；`--hold --urlbar` 截图为区域 5 块、舞台 464
- **已知取舍**：边缘缩放目前只有右下角 grip（与原版 `#grip` 一致）；`WM_NCHITTEST` 的边框命中已实现，
  但舞台 WebView2 铺满客户区，边缘点击落到网页 —— 真要做四边缩放，需要给舞台留 6px 内缩，留到实机验收再定

### M2 舞台 WebView2 与页面代理 `8d` [P0] ✅ 已完成（真页面 + 6/6 自检）

- [x] 舞台 WebView2 随父窗客户区同步（`gifa-shell::sync`）
- [x] 浏览器参数（`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`）：`--autoplay-policy=no-user-gesture-required`
      + `--disable-features=UseEcoQoSForBackgroundProcess`
- [x] UA 伪装（`gifa_player::UA`，去掉 WebView2 默认的 `Edg/`）
- [x] 独立 `user-data-dir`（`apps/gifa-demo/data/webview2`），登录态可持久化
- [x] **agent 用 initialization script 注入**：每次导航都会重新执行，自带幂等守卫、只在顶层文档跑
- [x] 页面代理 `crates/gifa-player/assets/player-agent.js`：
  - [x] 指令集：`toggle/play/pause/seek/seek-by/rate/volume/mute/preferences/clean/presentation/ping`
  - [x] `snapshot()` 上报：`{paused,currentTime,duration,volume,muted,rate,videoW,videoH,hasVideo,bvid,part,title,url}`
  - [x] `<video>` 探测：按 `readyState/duration/videoWidth` 打分取最优 + 700ms 兜底轮询（SPA 切 P 会重建）
  - [x] URL 轮询 500ms：自动连播改 `?p=` 时立刻上报新的分 P
  - [x] 全屏拦截：改写 `requestFullscreen` → 发 `{t:'fullscreen'}` 事件（窗口全屏留 M7）
  - [x] 纯净模式：注入 `CLEAN_CSS` + 自动点 `.bpx-player-ctrl-web`（1s × 最多 12 次）
  - [x] 沉浸样式 `setVideoOnly`：已实现（隐藏 body 只留 video、`object-fit:contain`），**等 M7 接线**
  - [x] 状态去重：签名变化立刻推，否则最多 4Hz
- [x] IPC 桥：页面 `window.ipc.postMessage(JSON)` → `gifa-shell` 解析 → 应用层事件通道
- **锚点**：`webview-preload.js:48-365`（偏好/事件/snapshot）、`2553-2777`（apply/boot）
- **验收**：`cargo run -p gifa-demo -- --check` → **6/6**（真 B 站页面：找到 video / 解析 BV+分P / 标题 /
  暂停指令 / 跳转 60s / 倍速 1.5×）；截图 `apps/gifa-demo/docs/m2-bilibili.png`
- **已知取舍**：
  - 未登录只有 480×360（B 站策略，与原版一致）；高清与扫码登录属 M3
  - 播放偏好只缓存不落盘（M11 设置层）
  - `setVideoOnly` / 全屏事件已产出但窗口侧未接（M7）

### M3 B 站解析与登录态 `4d` [P0] ✅ 已完成（8/8 自检）

- [x] `extract_id`：`BV[0-9A-Za-z]{10}`、`[?&/]av(\d+)`、`^av(\d+)$`（`gifa_player::extract_id`）
- [x] `extract_part`：`?p=` / `&page=`
- [x] **短链**：`b23.tv` / `bili2233.cn` 原样交给浏览器跳转，agent 报回落地地址后接管
      （不引 HTTP 栈；代价是多一次页面加载，好处是复用 UA/Cookie/代理，且不绕过 B 站自家风控跳转）
- [x] 视频信息：页面内 `fetch(api.bilibili.com/x/web-interface/view)`（同源 + `credentials:'include'`）
      → `bvid/title/owner/pages[]`
- [x] 登录态：页面内 `fetch(/x/web-interface/nav)` → `isLogin/uname/mid/vip/level`
- [x] 扫码登录窗：独立顶层窗 `1000×720` 加载 `passport.bilibili.com/login`，
      **共享同一个 WebContext**（Cookie 落在同一 profile，登录一次全局生效）
- [x] 域名白名单：登录窗只放行 `bilibili.com` / `b23.tv` / `bili2233.cn`，其余导航与新开窗一律拒绝
- [x] 登录成功回流：轮询 nav → 关窗 → 重载当前视频（B 站只在新文档里按登录态给高清）
- [x] `video_url(id,p)` 固定 `www.bilibili.com/video/...?p=&autoplay=1`
- [x] "上次看到"：`lastText/lastPart/lastVideoId/lastTitle/lastPageCount/lastWatchedAt` 落盘，
      启动无 `--open` 时自动续播
- **锚点**：`main.js:1473-1560`、`2014-2050`
- **验收**：`cargo run -p gifa-demo -- --check` → **8/8**
      （video 接口 `BV1xx411c7mD P1 · 碧诗`、nav 接口 ok、三条指令往返）
      截图 `apps/gifa-demo/docs/m3-bilibili.png`（主窗）、`docs/m3-login.png`（登录弹窗）
- **已知限制**：
  - 扫码需要人拿手机，自动化只能验到"弹窗打开 + 白名单 + nav 轮询"；真实登录成功后的高清切换待人工验收
  - 不透明短码（`b23.tv/AbCdEf`）的 `pending_short` 分支只有单测，缺实链样本
  - 设置落盘是 demo 里的极简实现（无原子写/迁移），M11 会挪进 `gifa-app`

### M4 全局热键 `8d` [P0] ⚠️ 机制完成，实机验收待做

- [x] 加速器解析：`Alt+Space`/`Mouse4`/`F8`/单字符；别名 `ArrowLeft`↔`Left` 等（`gifa_core::hotkey`）
- [x] **分流规则**：按住类/循环类/鼠标键/无修饰单键 → 低级钩子；其余 → `RegisterHotKey`
- [x] 钩子实现：`WH_KEYBOARD_LL` + `WH_MOUSE_LL`，装**调用线程**（UI 线程）上；
      `pressed` 去重、`blocked` 抑制、回调里只查表 + 发消息（守住 300ms 超时线）
- [x] 按住类配对：`Down`/`Up`；修饰键先松、未回中抬杆都会补 `Up`（不会卡在临时倍率）
- [x] 冲突事务：`swap_map` 算新表 → 全量重注册 → **任一失败整体回滚**
- [x] 注册失败上报：`Occupied` / `Unsupported` / `HookUnavailable` 三种原因 + 中文说明
- [x] rearm：`Unhook` + 重装（窗口恢复/唤醒时补挂）
- [ ] 输入态让路（打字/录制时忽略单键）—— 依赖启动器 UI，随 M6 接
- [ ] 录制新键位（`capturingHotkey` 期间暂停触发）—— 同上
- **锚点**：`hook-hotkeys.js` 全文、`main.js:1411-1472`
- **验收**：`cargo run -p shell-demo -- --hotkey-test`（6 项，见 `docs/HOTKEYS.md`）
- **实测**：`RegisterHotKey`(Alt+Space)、钩子键盘(F8 down/up)、rearm 后仍生效、冲突交换
  在 22:2x 的一轮里全部真实 PASS；之后前台变成高完整性窗口，注入输入不再到达，自检降级为 SKIP
- **实战（22 日 00:46）**：游戏前台时 `Alt+I` 按两次触发两次（去掉了自动重复）；11 条热键全部注册成功
- **关键发现（必须记住）**：**UIPI —— 低级钩子只能收到"前台窗口完整性级别 ≤ 自己"的按键**。
  游戏以管理员运行时，非提权的 GIFA 收不到任何热键（原版因此要求 `requireAdministrator`）。
  这把 M12 的"清单提权"从"建议"变成了"必须"，也解释了为什么自检在提权终端前台时会全 SKIP

### M5 手柄与八方向圆盘 `5d` [P1] ✅ 完成（合成帧 6/6；实体手柄待实机）

- [x] **XInput 直连轮询**（不引 gilrs）：`xinput1_4/1_3/9_1_0` 动态加载 → 16ms 轮询 4 槽位 → 归一化
      `Frame{buttons[16], axes[4]}`；帧签名变化才推事件
- [x] L3（XInput `0x0040` / 标准映射 `buttons[10]`）按住弹盘
- [x] 扇区：8 个 45°，0 号从正东起顺时针（`['e','se','s','sw','w','nw','n','ne']`）
- [x] 三级幅值：`<0.22` 回中、`≥0.55` 锁存、`<0.35` 提交；锁存后免疫边界抖动
- [x] 防误触：按住 L3 时杆未回中 → `awaiting_release`，必须先回中
- [x] 按住类动作代偿：过锁存线 = down，回中/抬杆 = up（未回中就抬杆也补 Up）
- [x] 断连/切槽 → 重置会话并强制"先松开再响应"
- [x] 圆盘窗口：惰性创建 `296×296`，`WS_EX_LAYERED|TRANSPARENT|NOACTIVATE|TOOLWINDOW|TOPMOST`
      + DWM 玻璃（逐像素透明），每次弹出重贴浮窗中心
- [x] 圆盘 UI：8 扇区 + 金色高亮 + 中心提示（"推杆选择"/"松杆触发"/"回中恢复"），标签取 `Action::short()`
- [x] 动作路由：播放/±5s/切P/倍速/透明/隐藏/长按加速；穿透与沉浸留 M7（只记日志）
- **锚点**：`gamepad.js`、`gamepad-config.js`、`main.js:1183-1358`
- **验收**：`cargo run -p gifa-demo -- --gamepad-test` → **6/6**
      （圆盘出现 / 高亮 e / 回中触发 / 松开消失 / 快速松开不误触 / XInput 可用）；
      截图 `apps/gifa-demo/docs/m5-disc.png`
- **实战（22 日 00:46）**：XInput 槽位 0，按住 L3 弹盘 4 次、松开收起 4 次，日志全部对上 ✅

### M6 浮窗 UI 移植 `8d` [P0] ✅ 完成（5/5 自检）

- [x] 搬 `renderer/index.html` + `style.css`（**逐行照搬** 742 行 CSS）；`app.js` 改桥
- [x] 标题栏：视频标题、布局下拉、分 P 下拉（`P3 · 标题（12:34）`，当前 P 选中）、
      换链接、设置、最小化、关闭（内联 SVG 图标，不用 emoji）
- [x] 控件条：播放/暂停（图标随状态）、时间、进度条（拖拽即时 seek）、倍速（1/1.25/1.5/2/0.75）、静音
- [x] 链接栏：粘贴 → 载入（回车/按钮），Esc 收起
- [x] toast（带开合时的区域重算）
- [x] UI 自动隐藏：播放中且静止 2.6s 淡出；暂停/拖动/标定/鼠标在标题栏或 URL 条上时不隐藏
- [x] 16:9 舞台与 `webview` 尺寸同步：页面每帧上报 `{titlebar, footer, urlbar, overlay[]}`，
      外壳据此算舞台矩形与区域
- **锚点**：`renderer/app.js` 全文
- **验收**：`cargo run -p gifa-demo -- --ui-test` → **5/5**
      （几何 30/34、控件状态、进度拖动 seek、自动隐藏、URL 条推舞台）；
      截图 `apps/gifa-demo/docs/m6-ui.png`
- **本轮踩的坑**：
  1. `file://` 页面拿不到 IPC（wry 不注入）→ 改成编译期把 CSS/JS 拼进同一个 HTML
  2. 自动隐藏条件写反（`!paused` 应为 `paused`）
  3. `#urlbar` 默认可见（原版是启动时收起的）→ 舞台一开始被顶下去 39px
  4. Rust 每帧推 `showUrlbar:false` 把 URL 条压回去 → 页面自己管
- **实战（22 日）**：标题栏拖动修好（见 `docs/LIVE-TEST.md` 的 2/3/4 条）
- **已知限制**：设置按钮只弹 toast（启动器面板未做）；进度条悬停预览未做（原版也没有）

### M7 沉浸与鼠标避让 `4d` [P1] ✅ 完成（6/6 自检）

- [x] 进入：记原矩形、隐藏 chrome、强制穿透、**舞台占满客户区**、按视频比例贴合、
      通知页面 `presentation{videoOnly:true}`、起 60ms 心跳
- [x] 按视频比例贴合：`aspect = videoW/videoH`，只在原矩形内缩、不超工作区、居中；
      不自作主张放大
- [x] 退出：先恢复不透明度再还原矩形（含 ±2px 舍入二次修正）、恢复 chrome 与穿透、把布局还给页面上报值
- [x] 不持久化沉浸状态（重启不自动进入）
- [x] 鼠标避让：60ms 轮询 + 120ms 防抖，移入 `min(base,0.1)`、移出恢复；临时值不写设置
- [x] `WM_DISPLAYCHANGE` 时重新贴合
- [x] 沉浸期间忽略隐藏页面的 layout 上报（否则标题栏留白又回来）
- **锚点**：`main.js:836-940`、`ISSUES.md` GIFA-003/004/013
- **验收**：`cargo run -p gifa-demo -- --immersion-test` → **6/6**
      （比例贴合 / chrome+穿透 / 移入变淡 / 移出恢复 / 尺寸精确还原 / chrome+穿透复原）；
      截图 `apps/gifa-demo/docs/m7-immersion.png`（757×568，页面只剩视频）
- **本轮修掉两个真 bug**：
  1. `presentation` 的 `video_only` 序列化成蛇形，agent 读 `videoOnly` 拿到 undefined → 纯视频模式静默失效
  2. 沉浸时舞台没占满客户区，视频被塞进 1.5:1 的区域出现黑边

### M8 视觉原生化（二期）`19d` [P4] — 🟡 骨架已通，算法未做

> **2026-11-22 补充**：按用户要求先把**链路**打通（不是二期交付）。
> 已做：GDI 截屏 → 采样窗 → 占位检测 → 叠加层，附合成地图端到端对拍。
> 未做：真实算法、DXGI 帧通道、黄金帧基线。详见 `gifa-rs/docs/VISION.md`。

**骨架（已通，非二期验收）**
- [x] `gifa_vision::screen::capture_screen`：GDI BitBlt 截一块矩形（BGRA→RGBA）
- [x] `SampleBox::from_roi`：ROI → 采样窗（边长 `r×2.4`，与原版采样窗同规则）
- [x] `detect_simple`：青素质心 → 箭头角、白像素质心 → 视角角；**小地图存在性门控**（`cyan_frac≥0.0015`）
- [x] `pipeline::start`：100ms 定时线程，采样窗由 UI 线程动态喂入（窗口移动/切布局不用重启）
- [x] 应用接线：视频矩形 → 去黑边 → 当前布局 ROI → 屏幕坐标 → 叠加层
- [x] 自检：`--vision-test`（真实截屏 4/4）、`--vision-synth`（合成地图 3/3，箭头 90°/视角 180°）
- [x] 修 bug：GDI `DeleteObject` 后读 bits 的 use-after-free（单测直接 ACCESS_VIOLATION）
- ⚠️ 明确未做：自适应阈值/最大连通块/72 格剖面/三点枚举/极坐标格/最小二乘/α-β 滤波

**M8b 算法移植 `15d`** ⚠️ 二期最高风险

- [ ] `arrow`：青色分数 → p99 自适应阈值 → 最大连通块 → 72 格径向剖面 → 角候选 → 三点枚举评分 → 鼻尖
- [ ] `cone`：极坐标格（120×12）→ 白膜边检测（最小二乘解 α）→ 视锥方向
- [ ] `filter`：α-β 滤波（`a=0.32+0.3c`、`b=0.06+0.1c`、`ω` 限 ±540°/s）+ 130° 大跳门限 + 扇形投票
- [ ] `view`：视角通道独立状态机（1200ms 宽限、>100° 大跳挂起、30° 复核）
- [ ] 小地图存在性门控：`cyanFrac ≥ 0.0015`
- [ ] 重试策略：`analyzeArrowWithRecovery` / `analyzeConeWithRecovery`
- [ ] 自动定心：最大青块质心 → `κ=0.139` 反推支点 → 多帧投票（6 票 + 倍率 2）→ 保存 trim
- [ ] 性能：单帧 <8ms
- [ ] 切换：`player-agent.js` 关闭识别循环，只保留帧取样；Rust 侧接上
- **锚点**：`webview-preload.js:365-2050`
- **验收**：§7.5 黄金帧回归通过（有效率不低于基线 −3%，角度 P95 ≤ 3°），且一期的 §11.5 全部复核通过

### M9 叠加层渲染 `5d` [P1] ✅ 完成（6/6 自检；数据源暂为合成角度）

- [x] 窗：透明、`WS_EX_LAYERED|TRANSPARENT|NOACTIVATE|TOOLWINDOW|TOPMOST`、DWM 玻璃、点击穿透
- [x] 尺寸 `radius × 2.24`（半径 50~400），惰性创建
- [x] 红箭头：闭合轮廓（细尾→渐宽箭杆→后掠箭头）、白描边 + 暗投影、红色三段渐变、中轴高光
- [x] 绿弧：半张角 `clamp(18°,34°, width*0.45)`、中段等厚两端收细的月牙、中央小尖、径向渐变、暗描边
- [x] 补间：每帧追 45%、收敛阈值 0.25°、>90° 大跳直接就位
- [x] 视角过期：1500ms 无更新收掉绿弧且不续期
- [x] 放置模式：虚线环 + 十字、去穿透、`HTCAPTION` 原生拖动、松手取圆心
- [x] `clear` / `clearAngle` / `hold` 三种指令语义
- [x] 页面回报"当前画出来的角度"，自检靠它判定补间与过期
- **锚点**：`overlay/overlay.js`、`main.js:1048-1182`
- **验收**：`cargo run -p gifa-demo -- --overlay-test` → **6/6**
      （尺寸 246 / 两路独立 / 补间走中间帧 / 1.5s 过期 / clear 清空 / 放置模式）；
      截图 `apps/gifa-demo/docs/m9-overlay.png`
- **本轮修掉的 bug**：`setTarget('view')` 先于 `touchView()` 时，首帧 `at=0` 被判过期 →
      绿弧永远画不出来。改成先续期再设目标
- **已知限制**：数据源是合成角度（一期识别仍在页面 JS，agent 还没接视觉）；
  叠加层的置顶巡检没覆盖（原版 GIFA-011 的已知缺陷）

### M10 布局标定 `5d` [P1] ✅ 完成（4/4 自检；ROI 尚未喂给识别）

- [x] 内置布局：影月月 `282/1920, 266/1080, 276/1920`、穿山甲 `258/1920, 284/1080, 260/1920`
      （归一化**混用**：`cx`、`r` 除以视频宽，`cy` 除以视频高）
- [x] 自定义布局：`select / default / remove / rename / create / calibrate / center` 七个操作 + 校验
      （名称去重、`r∈[0.01,0.5]`、内置只读、`center` 要 revision 匹配）
- [x] 旧版迁移：`rois` + `roiBy` → `legacy-N`，跳过 2 个 SEED 脏数据
- [x] 标定交互：agent 报 `video-rect`（元素位置/尺寸/objectFit/dpr）→ 去黑边（`content_rect`）
      → `roi_from_pixels` 归一化 → 写进布局；拖圆环 + 滚轮调半径 + 完成/取消
- [x] 内置布局只读：对着预设标定会**新建**"自定义 N"，不改预设
- [x] `revision` 机制：`calibrate`/`center` 自增，过期结果拒绝写回（数据层 + 单测）
- [x] 每帧只识别**当前一块**区域（布局模型层面保证：只有 `active_layout_id`）
- [x] 布局切换：标题栏下拉，选择即切并落盘
- **锚点**：`bearing-layouts.js` 全文、`renderer/app.js:234-510`
- **验收**：`cargo run -p gifa-demo -- --calib-test` → **4/4**
      （video-rect / 标定层命中 / 生成合法布局 / 布局切换）；
      截图 `apps/gifa-demo/docs/m10-calib.png`
- **本轮修掉两个 bug**：
  1. `#calib` 没写 `top` → 高度 0，区域 0×0，圆环看不见也点不到
  2. `openCalib()` 先量尺寸再加 `.active`，`display:none` 时量到 0×0 → 圆环跑到左上角、半径退化成 40
- **已知限制**：ROI 只落盘未喂识别（识别在二期）；自动定心的 UI 入口等识别接上再开

### M11 设置与迁移 `3d` [P0] ✅ 完成（7/7 自检 + 10 个单测）

- [x] `Settings` 与原版**字段同名同义**（§13-A），serde `default` + 未知字段保留（`flatten extra`）
- [x] 合并策略：`hotkeys` / `gamepad` / `bearing` 深合并；缺字段用默认、非法值丢弃
- [x] 原子写：`.tmp` + `ReplaceFileW`（失败退化 rename），保留原文件 ACL
- [x] **250ms 防抖**（原版 `persist()` → `setTimeout(flushSettings, 250)`）；退出前 `flush()`
- [x] 迁移：直接读原版 Electron `settings.json`（bounds/hotkeys/gamepad/lastText 全部直通）；
      `bearing.rois` + `roiBy` → `legacy-N` 布局，跳过 2 个 SEED 脏数据
- [x] 数据目录：Dev / Installed(`installed.flag`) / Portable(探写) / Fallback 四态
- [x] 数据文件搬迁：`settings.json` / `bearing-dump.png` / `bearing-frame.png`，**不覆盖已有**
- [x] 版本号 `configVersion` + 降级保护（高版本先备份成 `settings.json.v{N}.bak`）
- **锚点**：`main.js` 的 `settingsApi`/`loadSettings`/`migrateLegacyData`
- **验收**：`cargo run -p gifa-demo -- --settings-test` → **7/7**
- **已知限制**：demo 仍用 `cwd/data`；登录态不在搬迁范围（需重新扫码）

### 实战联调（2026-11-22）✅ 功能可用

第一次把整套放进真实游戏跑，抓到并修掉 7 个只在真实环境暴露的问题，详见 `gifa-rs/docs/LIVE-TEST.md`：

1. 独占全屏不可覆盖（要求窗口化/无边框）
2. `-webkit-app-region: drag` 在 WebView2 里吞掉标题栏鼠标事件
3. `WS_EX_NOACTIVATE` 宿主下 WebView2 不派发 DOM 鼠标事件（第一次点击只激活）
4. 拖动区坐标漏加窗口原点
5. `RegisterHotKey` 自动重复（8 秒触发 10 次）
6. 被遮挡时截屏截到"上面的窗口"，报出假方位
7. 置顶被游戏抢走，需无条件重新抬起

### M12 打包与分发 `5d` [P0] 🟡 绿色版完成，安装器待补

- [x] `cargo build --release` + **清单注入**（管理员 + `PerMonitorV2` + `longPathAware` + Common-Controls）
      用链接器 `/MANIFESTINPUT`，不依赖 `rc.exe`
- [x] 单文件 exe（页面/样式/脚本编译期内联）1.7 MB
- [x] 绿色版 zip：`dist/GIFA-0.2.6-win-x64.zip`（0.7 MB），顶层 `GIFA-0.2.6-win-x64/`，
      含 `GIFA.exe` + `data/` + `licenses/` + `README.txt`
- [x] `SHA256SUMS.txt`（逐文件）
- [x] 许可打包：`LICENSE-GIFA.txt`、`NOTICE-GIFA.txt`、`LICENSE-LGPL-3.0.txt`
- [x] 数据目录：release 走 `resolve_data_dir`（安装版 `%APPDATA%` / 绿色版 `<exe>\data`）
- [x] WebView2 运行时检查：缺失或 <110 时弹框给下载地址后退出
- [x] 构建脚本 `scripts/build.ps1`（构建 → 组装 → 校验和 → zip）
- [ ] NSIS 安装器（本机无 `makensis`；原版 `build/installer.nsh` 与素材已就位）
- [ ] exe 图标/版本资源（本机无 `rc.exe`，MSYS `windres` 预处理失败）
- **验收（已验证部分）**：release 清单含 `requireAdministrator`×2；no-admin release 变体
  `--version` 正常、`--settings-test` 7/7、`--ui-test` 5/5
- **待验收**：干净 Win10 22H2 上双击 → UAC → 起窗 → 加载视频 → 热键生效
- **详见**：`gifa-rs/docs/PACKAGING.md`

### M13 测试与对拍 `8d` [P0]

- [ ] 单测（`cargo test`，无 UI）：设置合并、布局 CRUD/迁移、加速器解析、冲突事务、chord 状态机、URL 解析、α-β 滤波、三角评分
- [ ] 视觉回归：**一期**只做冒烟（固定视频开 30 秒，断言有角度上报、无异常日志）；golden-frame 全量对拍属二期（§7.5）
- [ ] 端到端脚本：`--open BVxxx --bearing` → 截图 + 读 `startup.log` + 断言角度上报
- [ ] 手工矩阵：Win10 22H2 / Win11 24H2 × DPI 100/125/150/200 × 单/双屏 × 窗口化/无边框 × 管理员/非管理员 × 有线/无线手柄 × 罗技驱动侧键
- [ ] 性能预算：空闲内存 <200MB、CPU <3%（播放时识别 <10%）、冷启动 <2.5s

---

## 7. 视觉识别移植专章

> **本节是二期（P4）的移植规格书（D2）**。一期的这份逻辑以 JS 形式原样运行在 `player-agent.js` 里；本节用于二期"逐函数对照移植"。
> **一期开发时不要按本节重构页面代码**——保留原样，二期才有可比对的 ground truth。

### 7.1 两个独立通道（**绝不可合并**）

| 通道 | 证据 | 语义 | 失效时的行为 |
|---|---|---|---|
| **行进方向**（红箭头） | 青色三角（玩家箭头） | 人往哪走 | 不猜、不显示，保持不动 |
| **镜头视角**（绿弧） | 白色扇形（视锥） | 镜头朝哪看 | 1200ms 宽限内保持上次；小地图确实不在才返回 null |

原版反复强调：站着不动拉鼠标，人没动、视角转 180°。所以这是两个量，各自的滤波与门控完全独立。

### 7.2 参数表（移植时逐项对齐）

| 参数 | 值 | 出处 |
|---|---|---|
| 青色分数 | `min(g,b) - r` | `webview-preload.js:562` |
| 自适应阈值 | 采样 p99 < 100 → 判无箭头；`thr = max(112, p99*0.74)` | `:576-587` |
| 有效半径带 | `rMin=0.04*size`、`rMax=0.5*size` | `:589-592` |
| 最小青像素 | 12 | `:612` |
| 最大连通块质心约束 | `ρ ≤ 0.85*rMax` | `:697` |
| 角剖面 | `NB=72`（5°/格）+ 环形 3 点平滑 | `:701-712` |
| 峰阈值 | `peakMax*0.5` 且为局部极大、相邻峰 ≥25°、最多 6 个 | `:745-756` |
| 支撑宽度 | `span`：以峰自身 0.86 倍为材料阈值，最多 ±12 格 | `:735-742` |
| 三角硬约束 | 尾夹角 60~150°；鼻到两尾 95~162°；中分线反向 ±15°；鼻半径最小且不比尾尖 | `:820-860` |
| 箭头窗口 | `scale=0.62`、`cap=320`、`size>320 → step=2` | `:501-527` |
| 扇形窗口 | `scale=1.5`、`cap=512` | `:1616-1632` |
| 极坐标格 | `NA=120`（3°）、`NR=12`、`rIn=0.16*size`、`rOut=0.48*size` | `:1011-1016` |
| 白膜 α 上限 | `A_MAX=0.45`、`MIN_SUP=5` 层同号、`SIDE=2`、`HALF=15` | `:1199-1202` |
| 小地图存在门控 | `cyanFrac ≥ 0.0015` | `:1437` |
| 视角宽限 | `VIEW_GRACE_MS=1200` | `:1433` |
| 大跳门限 | `VIEW_JUMP_DEG=100`、`VIEW_PENDING_MATCH_DEG=30` | `:1444-1445` |
| α-β 滤波 | `a=0.32+0.3c`、`b=0.06+0.1c`、`dt∈[0.008,0.4]`、`ω∈±540`、`abs(res)>130` 触发疑似翻转 | `:1359-1410` |
| 自动定心 | `κ=0.139`、`RING=24`、`TOL=20°`、`MIN_BLOB=40`、6 票锁存、倍率 2 | `:1756-1775` |

### 7.3 帧数据契约（**二期关键设计决策**）

`trait FrameSource` 只有三个方法：`frame_size()`、`next_frame() -> Option<Frame>`、`roi_window(roi, scale, cap) -> Option<Rect>`。

三个候选实现：

| 方案 | 通道 | 每帧数据量 | 评价 |
|---|---|---|---|
| **A. 整窗 RGBA**（推荐一期） | 页面 `fetch('gifa-frame://roi', {method:'POST', body: arrayBuffer})` → wry `with_custom_protocol` 收字节 | 箭头 320²×4=410KB；扇形 512²×4=1.05MB | 简单、保真；30fps 下约 14MB/s 进程内拷贝，可接受但别 60fps |
| **B. 掩膜压缩**（推荐二期优化） | 页面只回传 `cyan_mask`(1B/px，整数步长) + `white_alpha`(1B/px) | 箭头 102KB、扇形 262KB | 传输降 75%；但"阈值/连通块"逻辑留在 JS，Rust 只做几何 —— 边界不清 |
| **C. SharedBuffer 零拷贝** | `webview2-com` + `postMessageWithAdditionalObjects` 共享内存 | 0 拷贝 | 最快，但依赖 WebView2 1.0.1901+ 且与 wry 的 IPC 抽象冲突，需绕开 wry 直连 COM |

**决策**：一期用 **A**；把 `Frame` 定义为 `{width, height, stride, data: Vec<u8>}`（BGRA→RGBA 转换放 Rust 侧，避免 JS 侧逐像素改写颜色）。
**注意**：绝对不要在传输前缩放或转灰度——那正是原版反复警告的失效原因。

### 7.4 函数映射表（逐函数对照移植，禁止"凭理解重写"）

| JS | Rust |
|---|---|
| `cyanScore(r,g,b)` | `vision::color::cyan_score` |
| `centerWindow / grabCenter / cropCenterWindow` | `vision::roi::{window, crop}` |
| `analyze(img,size,step)` | `vision::arrow::detect` |
| `analyzeCone(img,size,step)` | `vision::cone::detect` |
| `analyzeArrowWithRecovery / analyzeConeWithRecovery` | `vision::retry` |
| `smoothBearing(obs,conf,now,coneDeg,coneConf)` | `vision::filter::AlphaBeta::step` |
| `updateView / viewPayload` | `vision::view::ViewChannel::step` |
| `markInvalid / sendBearing` | `vision::report::{Invalid, Update}` |
| `tickBearing` | `vision::pipeline::tick` |
| `autoCenterAttempt / decideAutoCenter / begin / finish / step / reset` | `vision::autocenter::*` |
| `startBearingLoop / stopBearingLoop`（rVFC） | `vision::scheduler`（帧回调驱动；无 rVFC 时 50ms 定时） |
| `BEARING` 全局状态 | `vision::BearingState`（单实例、`&mut`） |

### 7.5 对拍（Golden Frame）方案 —— 二期验收基石

1. **素材**：优先用原版 `scripts/vision-probe` 引用的 500 帧真实数据（原在系统临时目录 `vidframes500/`）；若缺，用 OBS 以 1080p 录制原版输出目标视频片段，再用脚本按 30fps 抽帧 + 原版标注的 ROI 一起固化。
2. **胶水**：写一个 `gifa-vision-bench` 二进制：读帧序列 + ROI → 输出 `frame_id,angle,conf,cyans,view,valid` 的 CSV。
3. **基线**：用原版 JS 实现（Node + 一个 30 行的 `canvas` 桩或离线注入）跑同一批帧，得到基线 CSV。
4. **判据**（写进 CI）：
   - 有效率：`valid_frames / total` **不低于基线 − 3 个百分点**
   - 角度：有效帧中 `P95(|Δangle|) ≤ 3°`，`max ≤ 15°`（大跳帧单独豁免，按原版"疑似反向"标记剔除）
   - 视角通道：`P95 ≤ 5°`，且漏检帧序列的起止位置与基线相差 ≤1 帧
   - 定心：最终 `trim` 与基线相差 ≤1px
5. **回归纪律**：任何参数改动都必须跑全量 500 帧，禁止"看单帧感觉对了就提交"。

---

## 8. Win32 技术要点速查

| 需求 | 手段 | 坑 |
|---|---|---|
| 完全无边框但保留阴影 | `WS_POPUP` + `WM_NCCALCSIZE` 返回 0 + `DwmExtendFrameIntoClientArea` | 处理不当会出现 1px 白边 |
| 子 WebView2 随父窗缩放 | `WM_SIZE` 里 `SetWindowPos` 子 HWND | WebView2 有 1~2 帧滞后，需 `SWP_NOREDRAW` 避免闪烁 |
| chrome 层与舞台的命中测试 | chrome 顶层窗 `SetWindowRgn(顶栏∪底栏∪浮动控件)` | ✅ 已验证：区域外 `WindowFromPoint` 落到舞台；※区域要随 UI 状态重算 |
| 分层窗 + WebView2 | 顶层 chrome 窗 + `DwmExtendFrameIntoClientArea(-1,-1,-1,-1)`（玻璃板） | ✅ 已验证：透出下层舞台窗、pill 混色精确；※子窗做不到（只能透出自己的表面） |
| 带 `WS_THICKFRAME` 的窗口 | —— | ⚠️ 有 ~8px 非客户区，子窗/WebView2 整体内缩、窗口边漏出别的程序；要可缩放必须自己算 `WM_NCCALCSIZE` |
| wry 的 `Rect` 单位 | `Position::Physical` / `Size::Physical` | ⚠️ PerMonitorV2 下用 `Logical` 会被再乘一次 DPI，尺寸翻倍或错位 |
| WebView2 跟随宿主尺寸 | `WM_SIZE` 里显式 `webview.set_bounds(...)` | ⚠️ **不会**自动跟随，尤其子窗/顶层窗自绘时 |
| 改 alpha / 区域后采样 | 等 ≥600ms 再 `GetPixel` | ⚠️ DWM 重合成有延迟，450ms 会读到旧像素（本次误判过一次） |
| 透明叠层画箭头 | `UpdateLayeredWindow`（预乘 BGRA）+ `tiny-skia` | 必须预乘 alpha，否则边缘发黑 |
| 不抢焦点建窗 | `CreateWindowEx(WS_EX_NOACTIVATE)` + `SW_SHOWNOACTIVATE` | 用 `SW_SHOW` 会激活窗口；`ShowWindow` 之后若游戏已在前台，不要再 `SetForegroundWindow` |
| 系统级热键被游戏吞掉 | 提权 + `RegisterHotKey`；游戏用 Raw Input 独占时无解 → UI 提示换键 | 原版 README 已列为已知限制 |
| 钩子被"超时摘掉" | `LowLevelHooksTimeout` 默认 300ms；钩子回调里**只**做 `try_send` | 任何 `println!`/锁竞争都可能超时 |
| 多显示器坐标 | 全部用虚拟屏坐标（可为负）；`MonitorFromPoint` 而非 `Primary` | 原版 `clampBounds` 有"别把窗口从副屏拽回主屏"的专门逻辑 |
| DPI | `PerMonitorV2` + `WM_DPICHANGED` 里用建议矩形 `SetWindowPos` | 混用 DPI 时 `GetWindowRect` 是物理像素，`GetClientRect` 是逻辑像素 |
| 游戏独占全屏 | 无解（DWM 不合成），UI 引导改窗口化/无边框 | 与原版一致 |

---

## 9. 风险登记册

| ID | 风险 | 概率 | 影响 | 对策 |
|---|---|---|---|---|
| R1 | **二期视觉移植调参成本被低估**（原版靠 500 帧实拍反复迭代） | 中 | 中 | 一期不碰它（D2），风险只落在二期；先做 §7.5 工具链再动算法，预留 15→25 人日弹性 |
| R2 | B 站改版导致选择器/接口失效 | 中 | 高 | 页面代理里所有选择器集中成常量表；`pickVideo` 走"任意 `video` 且 `readyState>0`"的宽松匹配；接口失败降级为"直接载入页面" |
| R3 | WebView2 透明子窗/分层窗在某些驱动上黑底 | 中 | 中 | 叠加层二期改 `UpdateLayeredWindow`；提供"关闭叠加层"降级开关 |
| R4 | 提权后 WebView2 与 Edge 进程通信异常（沙箱） | 低 | 高 | WebView2 有自研沙箱，不受 Chromium 提权缺陷影响；但需在 Win10 19045 实测，并保留"关闭沙箱"启动参数兜底 |
| R5 | 全局钩子在反作弊/杀软下被判可疑 | 中 | 中 | 与原版同一技术（同为 `WH_*_LL`），无额外风险；提供"仅用 RegisterHotKey"模式 |
| R6 | 硬件解码不可用 → 4K 播放掉帧 | 中 | 中 | WebView2 用 Edge 的解码栈；测试 HEVC/AV1；必要时引导用户装 HEVC 扩展 |
| R7 | 多 WebView2 同窗的层级/焦点问题返工 | **低** | 中 | ✅ 已被 spike 排除：chrome 层改用独立顶层玻璃窗 + 区域挖空，5/5 通过（`gifa-rs/spikes/dual-webview`） |
| R8 | 迁移原版 `settings.json` 时字段语义漂移 | 低 | 中 | 字段逐一对照 §13-A；写迁移单测，用原版真实设置文件做 fixture |
| R9 | 许可合规（复用了 GPL 前端资源） | 低 | 高 | 整体 GPL-3.0-only；保留 NOTICE 与历史 MIT 说明；**不**引入 GPL 不兼容的第三方（如某些闭源 SDK） |

---

## 10. 里程碑

| 里程碑 | 内容 | 人日 | 出口标准 |
|---|---|---|---|
| **P0 垂直切片** | M0 + M1 + M2 + M3 + M6 | ~31 | 双击 → 启动器 → 粘链接 → 浮窗置顶播放 → `Alt+Space` 暂停 |
| **P1 功能对齐** | + M4 + M5 + M7 + M11 | ~51 (+20) | §1.1 的 P0 需求全绿；可用它替代原版日常使用 |
| **P2 视觉可用** | + M9 + M10 | ~61 (+10) | 方位标记（页面 JS 识别）能画、能标定；§11.5 通过 |
| **P3 一期收尾（可发布）** | + M12 + M13 | **~74 (+13)** | 安装包/绿色版产出；§11 全绿；**与原版 v0.2.6 功能等价** |
| **P4 视觉原生化（二期）** | + M8a + M8b | ~93 (+19) | golden-frame 对拍达标；识别不再跑在页面里 |

建议节奏：2 人并行（A：窗口/输入；B：前端桥/播放代理），P2 起合流做标定与叠加层；二期由最熟悉视觉的人单独推进（1 人约 4 周）。

---

## 11. 验收清单

### 11.1 系统能力

- [ ] 双击 exe 只弹一次 UAC；非管理员启动被拒绝或提示
- [ ] 第二次双击唤回已开启动器（不新开进程）
- [ ] 浮窗置顶压住窗口化原神；切任务管理器抢层后 2.5s 内自动回顶
- [ ] 浮窗显示时游戏不切到后台（不抢焦点）
- [ ] `Alt+H` 隐藏/显示；隐藏时热键仍生效（会重新拉起浮窗）
- [ ] `Alt+L` 切换穿透后点击直接落到游戏
- [ ] `Alt+O` 在 1.0 ↔ 0.65 间切换，重启保留
- [ ] 拖标题栏移动、拖右下角缩放；位置尺寸重启后原位复活
- [ ] 150% DPI 下位置与尺寸不偏；副屏重启仍在副屏

### 11.2 播放

- [ ] BV 号 / av 号 / 完整链接 / `b23.tv` / `?p=3` 五种输入都能载入
- [ ] 未登录只能标清；扫码登录后 1080P/4K；重开仍是登录态
- [ ] `Alt+←/→` 后退/前进 5 秒；`Alt+↑/↓` 切分 P；下拉框标出当前 P
- [ ] 自动连播到下一 P 时，UI 的 P 号与标题跟上
- [ ] 音量/静音/倍速跨切 P 与重启保留（GIFA-001/002）
- [ ] B 站播放器内改音量/倍速会回写应用设置
- [ ] 纯净模式无顶栏/推荐/评论/广告
- [ ] 播放器"全屏"按钮变成窗口全屏（不遮住标题栏）

### 11.3 沉浸

- [ ] `Alt+I` 进入：控件全隐、鼠标穿透、锁定移动缩放、不抢焦点
- [ ] 视频原比例贴合、不留多余黑边（GIFA-013）
- [ ] 鼠标移入变 10% 不透明、移出恢复（120ms 防抖不闪）
- [ ] 退出后位置/尺寸/穿透/透明度精确还原
- [ ] 重启不自动进入沉浸

### 11.4 手柄

- [ ] 游戏前台按住 L3 弹盘、松开消失
- [ ] 右摇杆八方向高亮正确；锁存后不再抖
- [ ] 按住类动作（临时加速）可用摇杆位置触发/释放
- [ ] 断连重连后不卡在"按下"状态

### 11.5 方位标记（一期由页面 JS 识别；二期切换后须全项复核）

- [ ] 开启后屏幕上出现红箭头 + 绿弧
- [ ] 站着转视角：箭头不动、绿弧跟着转
- [ ] 直线跑动：箭头指向行进方向（±20° 内）
- [ ] 转场/特写/对话时标记不指假方向（箭头保持、绿弧 1.2s 后消失）
- [ ] 切换内置/自定义布局后标记立即对齐
- [ ] 标定：拖圆环套住小地图、滚轮调大小、完成后识别正常
- [ ] 自动定心后箭头有效率明显提升（主观：不再频繁丢失）
- [ ] 标记窗被抢层后能自动回顶（修掉原版 GIFA-011）

### 11.6 工程

- [ ] `cargo test` 全绿；`cargo clippy -D warnings` 干净
- [ ] golden-frame 回归达标（§7.5）
- [ ] 空闲内存 <200MB；冷启动 <2.5s；识别单帧 <8ms
- [ ] `startup.log` 在故障时有可诊断信息
- [ ] 安装包/绿色版都能起，卸载保留数据
- [ ] 旧版 `settings.json` 可平滑接管

---

## 12. 许可证与合规

- 本复刻是原项目的**衍生作品**，必须整体 **GPL-3.0-only**；分发二进制时须提供对应完整源码。
- 复用 `src/renderer`、`src/console`、`src/overlay`、`src/disc` 的 HTML/CSS/JS 属于直接复用，保留版权声明与 `NOTICE` 中关于 v0.2.4/0.2.5 追加 GPLv3 授权的说明。
- 移除 `uiohook-napi`（LGPL）后，`LICENSES/LGPL-3.0.txt` 与 `libuiohook-NOTICE.txt` 不再是必需项，但**保留** `LICENSES/MIT-legacy.txt` 作为历史记录。
- WebView2 Evergreen Runtime 可在应用内分发（Microsoft 允许），需在 `resources/licenses/` 附其许可与隐私说明；若走"系统自带 Edge"模式则无需分发。
- 不调用 B 站私有/未公开接口，不绕过会员与付费内容权限；一切播放行为都发生在用户自己已登录的网页内 —— 与原版一致。
- 不打包任何第三方影视内容；仓库不放实拍素材（沿用原版 AGENTS.md 的目录边界约定）。

---

## 13. 附录

### A. 默认设置结构（与 `settings.json` 同构，迁移用）

```jsonc
{
  "bounds": { "width": 896, "height": 568, "x": null, "y": null, "aspect": 2 },
  "pinned": true,
  "opacity": 1,
  "clickThrough": false,
  "mode": "web",
  "volume": 100,
  "muted": false,
  "playbackRate": 1,
  "holdPlaybackRate": 2,
  "hoverFadeEnabled": true,
  "muxed": false,
  "lastText": "", "lastPart": 1,
  "lastTitle": "", "lastVideoId": "", "lastPageCount": 0, "lastWatchedAt": 0,
  "resumeOnLaunch": true,
  "hotkeys": { /* 见 B */ },
  "bearing": {
    "layouts": [ /* {id,name,roi:{cx,cy,r},builtIn,centered,revision} */ ],
    "defaultLayoutId": "builtin",
    "activeLayoutId": "builtin",
    "enabled": false, "radius": 110, "x": null, "y": null
  },
  "gamepad": {
    "enabled": true,
    "disc": { "radius": 132, "x": null, "y": null },
    "stick": { "deadzone": 0.22 },
    "map": { "n":"prevPart","ne":"cycleSpeed","e":"seekFwd","se":"toggleOpacity",
             "s":"nextPart","sw":"togglePass","w":"seekBack","nw":"togglePlay" }
  }
}
```

### B. 默认快捷键

| 动作 key | 默认 | 中文 |
|---|---|---|
| `togglePlay` | `Alt+Space` | 播放 / 暂停 |
| `seekBack` | `Alt+Left` | 后退 5 秒 |
| `seekFwd` | `Alt+Right` | 前进 5 秒 |
| `prevPart` | `Alt+Up` | 上一分 P |
| `nextPart` | `Alt+Down` | 下一分 P |
| `toggleVisible` | `Alt+H` | 隐藏 / 显示浮窗 |
| `togglePass` | `Alt+L` | 鼠标穿透开关 |
| `toggleOpacity` | `Alt+O` | 半透明开关 |
| `toggleImmersion` | `Alt+I` | 纯视频沉浸开关 |
| `cycleSpeed` | `Alt+S` | 循环倍速 1→1.25→1.5→2 |
| `holdSpeed` | `F8` | 长按临时加速（默认 2×） |

### C. 时序与尺寸常量

| 量 | 值 |
|---|---|
| 舞台内容 | 896×504（16:9）+ 标题栏 30 + 控件条 34 = **568** |
| 浮窗最小 | 240×150 |
| 启动器 | 1280×720（最小 800×450，按工作区 16:9 缩） |
| 叠加层/圆盘窗 | `radius × 2.24` |
| 置顶巡检 | 2500ms |
| 鼠标避让轮询 | 60ms（120ms 防抖） |
| 页面 URL 轮询 | 500ms |
| `<video>` 兜底轮询 | 700ms |
| 状态推送 | 1000ms（播放中） |
| 全屏尝试点击 | 1s × 最多 12 次 |
| 叠加层 tween | 每帧 45%，阈值 0.25°，>90° 直跳 |
| 绿弧过期 | 1500ms |
| 设置写入节流 | 250ms（`persist()`） |
| 日志上限 | 128KB |

### D. 阅读索引（源仓关键位置）

| 主题 | 位置 |
|---|---|
| 默认设置/热键表 | `src/main.js:360-446` |
| 窗口创建与置顶 | `src/main.js:653-810` |
| 沉浸与鼠标避让 | `src/main.js:836-940` |
| 叠加层与圆盘 | `src/main.js:1048-1358` |
| 热键注册与分流 | `src/main.js:1411-1472`、`src/hook-hotkeys.js` |
| B 站解析/登录 | `src/main.js:1473-1630` |
| IPC 全表 | `src/main.js:1629-2050` |
| 播放器代理 | `src/webview-preload.js:1-365`、`2553-2777` |
| 视觉识别 | `src/webview-preload.js:365-2050` |
| 布局与迁移 | `src/bearing-layouts.js` |
| 手柄状态机 | `src/gamepad-config.js`、`src/gamepad.js` |
| 叠加层绘制 | `src/overlay/overlay.js`、`src/disc/disc-draw.js` |
| 已知缺陷与历史决策 | `ISSUES.md`、`AGENTS.md`、`README.md` |
| **M1 spike（可运行代码 + 结论）** | `gifa-rs/spikes/dual-webview/`、`FINDINGS.md`、`docs/spike-toplevel.png` |

---

## 14. 开工清单（Next Actions）

按顺序做，前三项做完再写业务代码。

| # | 动作 | 产出 | 判定 |
|---|---|---|---|
| 0 | ~~M1 前置 spike：双 WebView2 窗口结构~~ | ✅ `gifa-rs/spikes/dual-webview` | 5/5 通过，结论见 `FINDINGS.md` |
| 1 | ~~按 D3 把 spike 扩成 `gifa-shell` 骨架~~ | ✅ `crates/gifa-shell` + `apps/shell-demo` | 自检 6/6、真实拖角 PASS、单测 9/9 |
| 2 | ~~建 workspace 全骨架（`gifa-core` / `gifa-vision` / `gifa-input`）~~ | ✅ 三个 crate 已落地，非空壳：设置/布局/热键/手柄状态机 + 视觉帧源抽象 | `cargo test` **60 通过 0 失败**、clippy 0 warning |
| 2.5 | ~~M2：真 B 站页面 + 播放代理 + IPC 桥~~ | ✅ `crates/gifa-player` + `apps/gifa-demo` | `--check` 6/6 |
| 3 | ~~确认 WebView2 运行时基线~~ | ✅ 基线 ≥110 已写入 `gifa_shell::WEBVIEW2_MIN_MAJOR` + `gifa-demo --runtime` 探针 + `docs/WEBVIEW2.md` | 本机 157.0.4341.60 → `RESULT 2/2`；**Win10 22H2 干净机仍待实测** |
| 4 | 固化视觉素材与基线 | 500 帧导入（或缺素材时的 OBS 替代方案） | 基线 CSV 能生成（二期用，一期先占位） |
| 5 | 决定首个可演示目标 | P0 垂直切片里程碑 | 能对着原版并排跑 |

**下一步（M3）**：B 站解析与登录态 —— 链接/短链解析、`x/web-interface/view` 取分 P 列表、
扫码登录窗（复用 chrome 玻璃窗机制）、未登录/登录后的画质差异。

**M2 已完成**：`apps/gifa-demo` + `crates/gifa-player`（真 B 站页面 6/6 自检）。

**已完成到 #3**：`#0` spike、`#1` shell 骨架、`#2` workspace 全骨架、`#3` WebView2 基线。
剩下 `#4`（视觉素材与基线，二期用）与 `#5`（首个可演示目标，即 P0 垂直切片，已由 `apps/gifa-demo` 覆盖大半）。

**并行可做的事**：前端桥 shim（`window.api` → wry IPC）与页面代理裁剪不依赖 Rust 窗口能力，可与 M3 并行推进。

**一期完成即冻结的接口**（二期要接）：`Bearing` 上报结构（`{angle, view, viewConfidence, viewWidth, hold, clear}`）、`trait FrameSource`、`Settings` 的 `bearing.layouts[].revision` 语义。
