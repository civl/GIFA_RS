# gifa-rs

GIFA 的 Rust 复刻工程（对应仓库根的 `task.md`）。

```
gifa-rs/
├─ Cargo.toml
├─ docs/
│  ├─ WEBVIEW2.md               # 运行时基线（≥110）与分发策略
│  ├─ HOTKEYS.md                # 热键两套机制、冲突回滚、UIPI 约束
│  ├─ GAMEPAD.md                # XInput 轮询、八方向状态机、圆盘窗口
│  ├─ IMMERSION.md              # 沉浸、比例贴合、鼠标避让
│  ├─ OVERLAY.md                # 两路角度、补间、过期、放置模式
│  ├─ CALIBRATION.md            # ROI 归一化、标定交互、布局 CRUD
│  ├─ SETTINGS.md               # 数据目录、原子写、防抖、旧版迁移
│  ├─ VISION.md                 # 识别骨架：截屏、采样窗、占位检测、门控
│  ├─ FLOAT-UI.md               # 浮窗界面移植：桥接、交互、自检
│  ├─ LIVE-TEST.md              # 实战联调：真实游戏里抓到并修掉的 7 个问题
│  ├─ PACKAGING.md              # 打包：清单注入、单文件、校验和、WebView2 检查
│  └─ REGRESSION.md             # 回归门禁：17 项、性能预算、手工矩阵清单
├─ crates/
│  ├─ gifa-core/                # 纯领域：设置 / 方位布局 / 快捷键解析 / 播放状态机
│  │  ├─ src/settings.rs        #   与原版 settings.json 同构 + 深合并 + 钳位
│  │  ├─ src/bearing.rs         #   内置布局 + CRUD + revision + 旧版迁移
│  │  ├─ src/hotkey.rs          #   加速器解析、分流（钩子/系统热键）、冲突交换
│  │  ├─ src/action.rs          #   11 个动作 + 中英文标签
│  │  └─ src/state.rs           #   倍速循环 / 长按加速（多来源）/ 透明度切换
│  ├─ gifa-input/               # XInput 轮询 + 八方向状态机 + 全局热键（注册/钩子）
│  ├─ gifa-vision/              # 二期：帧源抽象 + 青色分数/阈值（算法骨架）
│  ├─ gifa-player/              # B 站播放器协议 + 页面内 agent（player-agent.js）
│  └─ gifa-shell/               # 外壳：舞台窗 + 顶层 chrome 玻璃窗 + 区域联动
│     ├─ src/shell.rs           #   装配、WndProc、两窗同步、IPC 契约
│     ├─ src/layout.rs          #   纯函数：舞台矩形 / 区域 / 位置约束
│     ├─ src/region.rs          #   RectI + SetWindowRgn
│     ├─ src/win32.rs           #   极薄 Win32 封装（DPI / 分层 / 置顶 / 消息循环）
│     └─ src/probe.rs           #   屏幕取像素 + WindowFromPoint 归属（自检用）
├─ apps/
│  ├─ shell-demo/               # 骨架演示 + 自检（M1 出口验证）
│  │  ├─ src/main.rs
│  │  ├─ web/{chrome,stage}.html
│  │  └─ docs/shell-urlbar.png
│  └─ gifa-demo/                # M2/M3：真 B 站页面 + 播放代理 + 解析/登录态
│     ├─ src/main.rs
│     ├─ web/chrome.html
│     ├─ docs/{m2-bilibili,m3-bilibili,m3-login}.png
│     └─ README.md
└─ spikes/
   └─ dual-webview/             # M1 前置 spike（两个窗口结构的可行性验证）
      ├─ FINDINGS.md            #   ← 先读这个
      └─ docs/spike-toplevel.png
```

## 为什么不用 tao

外壳要自己接管 WndProc（`WM_NCCALCSIZE`、命中测试、`SetWindowRgn`、分层属性、DPI），
框架把 WndProc 藏起来后每件事都要靠 subclass 绕。wry 只要求一个 `HasWindowHandle`
（本仓库里是 `win32::HwndHost`），所以直接创建原始 HWND 喂给它。

## 架构（评审基线）

```
W_CHROME  独立顶层窗：WS_EX_TOOLWINDOW|NOACTIVATE，owner = 舞台窗
          DwmExtendFrameIntoClientArea(-1)  ⇒ 逐像素透明，能透出舞台
          SetWindowRgn(顶栏 ∪ URL条 ∪ 底栏 ∪ 浮动控件) ⇒ 其余点击穿透
          内含 chrome WebView2（我们自己的 HTML/CSS）
                    ▲ owner（z 序由系统保证）
W_FLOAT   舞台窗：WS_EX_LAYERED + LWA_ALPHA（透明度一次作用于整棵窗口树）
          WM_NCCALCSIZE 让客户区 == 整窗
          内含 stage WebView2（www.bilibili.com），只占视频该占的矩形
```

## 跑

```powershell
cd gifa-rs

# 骨架自检（6 项，自动退出）
cargo run -p shell-demo

# 真实鼠标拖右下角，验证「HTML grip → IPC → 系统缩放循环 → 两窗联动」
cargo run -p shell-demo -- --drag-test

# 起窗并自动展开 URL 条（人工看/截图）
cargo run -p shell-demo -- --hold --urlbar

# M2：载入真 B 站页面并自检（约 10s，自动退出）
cargo run -p gifa-demo -- --check

# M2/M3：直接载入指定视频（支持 BV/av/链接/短链）
cargo run -p gifa-demo -- --open BV1xx411c7mD

# M3：起窗后自动弹登录窗（人工扫码验收）
cargo run -p gifa-demo -- --login

# WebView2 运行时基线（不建窗口）
cargo run -p gifa-demo -- --runtime

# 全局热键自检（注入真实输入；前台完整性级别更高时会自动 SKIP）
cargo run -p shell-demo -- --hotkey-test

# 手柄圆盘自检（合成帧，不需要实体手柄）
cargo run -p gifa-demo -- --gamepad-test

# 沉浸与鼠标避让自检（--immersion-hold 可保持沉浸便于截图）
cargo run -p gifa-demo -- --open BV1xx411c7mD --immersion-test

# 方位叠加层自检（--overlay-hold 持续喂角度便于截图）
cargo run -p gifa-demo -- --open BV1xx411c7mD --overlay-test

# 布局标定自检（--calib-hold 保持标定界面便于截图）
cargo run -p gifa-demo -- --open BV1xx411c7mD --calib-test

# 设置与迁移自检（吃一份原版 settings.json）
cargo run -p gifa-demo -- --settings-test

# 浮窗界面自检（几何/控件/进度拖动/自动隐藏/URL 条）
cargo run -p gifa-demo -- --open BV1xx411c7mD --ui-test

# 识别链路自检：真实截屏 / 合成地图端到端
cargo run -p gifa-demo -- --open BV1xx411c7mD --vision-test
cargo run -p gifa-demo -- --vision-synth

# 纯函数单测
cargo test

# 发布前门禁：单测 + 10 个自检 + 识别冒烟 + 性能预算（17 项，退出码即结论）
powershell -ExecutionPolicy Bypass -File scripts/check.ps1
```

`GIFA_DEBUG=1` 会打印外壳内部步骤。

## 已验证的行为（2026-11-21，Win11 26300 / WebView2 157）

| 机制 | 结果 |
|---|---|
| `WM_NCCALCSIZE` 去非客户区 | 客户区 == 整窗（896×568） |
| 两窗联动 | 移动/缩放/DPI 后 chrome 矩形恒等于舞台窗矩形 |
| z 序联动 | chrome 的 owner = 舞台窗；最小化/销毁自动跟随 |
| 区域挖空 | 顶栏/pill/grip 命中 chrome，舞台中间命中浮窗（点击进网页） |
| 缩放后重算 | 加宽 240px 后舞台与区域同步；真实拖角 896×568 → 1038×682 |
| 不透明度 | `LWA_ALPHA` 读回 128；chrome 侧改用页面 `opacity`（不能用 LWA，否则逐像素透明失效） |
| 布局变化 | URL 条展开后舞台 504 → 464，区域 4 → 5 块 |

## M2：真 B 站页面（2026-11-21）

舞台从离线占位页换成 `www.bilibili.com/video/BV...`，页面里注入 `player-agent.js`：

```
Rust ──PlayerCommand{cmd:seek-by,value:5}──► agent.dispatch()  → <video>
Rust ◄──{t:"state", paused, currentTime, duration, bvid, part, …}── agent
```

**已验证**（`cargo run -p gifa-demo -- --check`，6/6）：

| # | 项 | 实测 |
|---|---|---|
| 1 | 找到 `<video>` 且有元数据 | 480×360 / 2056s（未登录画质，与原版一致） |
| 2 | agent 解析 BV 与分 P | `BV1xx411c7mD P1` |
| 3 | 标题可读 | `字幕君交流场所_哔哩哔哩_bilibili` |
| 4 | 指令：暂停 | `paused=true` |
| 5 | 指令：跳转到 60s | `currentTime=60.0` |
| 6 | 指令：倍速 1.5× | `rate=1.5` |

截图：`apps/gifa-demo/docs/m2-bilibili.png`（真页面在浮窗里播放，标题栏/进度条显示真实状态）。

### M2 阶段的取舍

- **agent 用 initialization script 注入**：每次导航（切 P、整页跳转）都会重新执行，自带幂等守卫，只在顶层文档跑。
- **偏好只缓存不落盘**：`volume/rate` 由 Rust 下发，但持久化要等 M11 的设置层。
- **HTML 全屏被拦截**：agent 把 `requestFullscreen` 改成发 `{t:'fullscreen'}` 事件，窗口全屏留到 M7。
- **`setVideoOnly` 已实现但未接线**：沉浸模式窗口侧（锁定尺寸/穿透/贴合比例）属 M7。
- **未登录只有 480×360**：B 站策略，不是 bug；扫码登录属 M3。

## M3：解析与登录态（2026-11-21）

```
输入 ──► resolve() ──► 规范地址 ──► 舞台导航
                     └─ 短链(b23.tv) 原样交给浏览器 → agent 报回落地 URL → 接管

页面 agent ──fetch──► api.bilibili.com/x/web-interface/view   → 分 P 列表/标题/UP
           └─fetch──► api.bilibili.com/x/web-interface/nav    → isLogin/uname/vip/level
```

**已验证**（`cargo run -p gifa-demo -- --check`，**8/8**）：

| # | 项 | 实测 |
|---|---|---|
| 4 | video 接口 | `BV1xx411c7mD P1 · 碧诗` |
| 5 | nav 接口 | `未登录（nav ok）` |
| 1-3,6-8 | 元数据 / 三条指令 | 同 M2 |

登录窗（`docs/m3-login.png`）：独立顶层窗 `1000×720`，加载 `passport.bilibili.com/login`，
**与舞台共用同一个 WebContext** —— Cookie 落在同一份 profile，扫完码不用搬 token。
登录成功后轮询 `nav` 自动发现，关窗并重载视频（B 站只在新文档里按登录态给高清）。

### 三个关键决定

1. **不引 HTTP 栈**：短链交给浏览器跳。代价是可能多一次页面加载，好处是复用 WebView2 的 UA/Cookie/代理，
   也不会绕过 B 站自己的风控跳转。
2. **元数据走页面内 fetch**：`credentials:'include'` 自动带 Cookie，不用把 Cookie 搬到 Rust 侧。
3. **登录窗共享 WebContext**：这是"登录一次全局生效"的实现基础，也是唯一需要 wry 保持 context 存活的理由
   （`Shell._web_context` 字段就是为了这个）。

### 坑

- **白名单一开始漏了 `b23.tv`**：它不在 `bilibili.com` 域下，单元测试直接抓出来了。
- **弹窗关掉不能退出进程**：`WM_DESTROY` 原来无条件 `PostQuitMessage`，关登录窗会把整个应用带走 ——
  现在只有舞台窗销毁才退出。

## M5：手柄八方向圆盘（2026-11-21）

```
16ms  XInputGetState(0..4)  →  Frame{buttons[16], axes[4]}  →  step_chord  →  ChordStep
                                                                  ├─ 圆盘（透明 HUD，惰性创建）
                                                                  └─ 动作（播放/切P/倍速/透明/加速）
```

三级幅值 `deadzone 0.22 / latch 0.55 / commit 0.35`：锁存免疫扇区抖动，回中阈值更低避免手抖误判。
两条防误触：按住时杆未回中要先回中；推杆后立刻抬 L3 时一次性动作作废、按住类补发 Up。

**坐标陷阱**：XInput 的 Y 向上为正，`Frame` 向下为正 —— 摇杆 Y 必须取反，否则南北反向（有单测固定）。

自检 6/6（合成帧，不需要手柄）：圆盘出现 / 高亮 e / 回中触发 / 松开消失 / 快速松开不误触 / XInput 可用。
截图 `apps/gifa-demo/docs/m5-disc.png`。

## M7：沉浸与鼠标避让（2026-11-21）

进入沉浸时：隐藏 chrome → 强制穿透 → **舞台占满客户区** → 按 `videoW/videoH` 贴合 → 通知页面
`presentation{videoOnly:true}` → 起 60ms 心跳做鼠标避让。退出时先恢复透明度再还原矩形
（含 ±2px 舍入二次修正），做到逐像素还原。

自检 6/6：比例贴合 / chrome+穿透 / 移入变淡到 26 / 移出恢复 255 / 尺寸精确还原 / 状态复原。
截图 `apps/gifa-demo/docs/m7-immersion.png`。

**两个真 bug**（截图才暴露的）：`presentation` 的 `video_only` 序列化成蛇形而 agent 读 `videoOnly`
→ 纯视频模式静默失效；沉浸时舞台还按"标题栏+控制条"留白 → 视频被塞进 1.5:1 区域出现黑边。

## M9：方位标记叠加层（2026-11-21）

红箭头（行进方向）与绿弧（镜头视角）**两路独立**，各有自己的补间状态：
每帧追 45%、>90° 大跳直接就位、绿弧 1.5s 无更新就收掉且不续期。

窗口是透明 HUD（点击穿透、不激活、置顶、DWM 逐像素透明），放置模式下临时去掉穿透并用
原生 `HTCAPTION` 拖动。

自检 6/6：尺寸 246 / 两路独立 / 补间走中间帧 / 过期 / clear / 放置。截图 `docs` 下 `m9-overlay.png`。

**抓到的 bug**：`setTarget('view')` 先于 `touchView()` 时首帧 `at=0` 被判"已过期"，绿弧永远画不出来。

## M10：布局标定（2026-11-21）

内置两套博主预设 + 自定义命名布局，七个操作带完整校验（去重、范围、内置只读、revision 过期）。

标定链路：页面报 `video-rect` → **去黑边**（`object-fit: contain` 时元素里可能有黑边）→
按"混用分母"归一化（`cx/r` 除以宽、`cy` 除以高）→ 写进布局。
内置预设只读，对着它标定会新建"自定义 N"。

自检 4/4：video-rect / 标定层命中 / 生成合法布局 / 布局切换。截图 `docs` 下 `m10-calib.png`。

**两个 bug**：`#calib` 没写 `top` 导致高度 0（区域 0×0，圆环点不到）；
`openCalib()` 先量尺寸再加 `.active`，`display:none` 时量到 0×0。

## M11：设置与迁移（2026-11-22）

新增 `gifa-store`：数据目录四态（Dev/Installed/Portable/Fallback）、`.tmp` + `ReplaceFileW` 原子写、
250ms 防抖、退出前 flush、`configVersion` 与降级备份。

迁移直接吃原版 Electron 的 `settings.json`：同名字段直通，`bearing.rois`/`roiBy` 转成 `legacy-N`
布局（跳过两个 SEED 脏数据），旧目录的标定图按需搬迁且不覆盖。

自检 7/7 + 10 个单测。

## M8 骨架：识别链路（2026-11-22）

GDI 截屏 → 采样窗（`r×2.4`）→ 占位检测（青素质心/白像素质心 + 小地图存在性门控）
→ 叠加层。采样窗由 UI 线程动态喂入，窗口移动/切布局不用重启线程。

自检：`--vision-test` 真实截屏 4/4（20 帧/2.5s，截帧 11.6ms、检测 0.1ms）；
`--vision-synth` 合成地图 3/3（页面画青三角+白扇，从真实屏幕抓回来，箭头 90°、视角 180°）。

**这不是 M8 交付**：真实算法（自适应阈值/连通块/72 格剖面/最小二乘/α-β 滤波）与黄金帧基线仍未做。

## M6：浮窗界面移植（2026-11-22）

原版 `renderer/index.html` + 742 行 `style.css` **逐行照搬**，`app.js` 改成我们的 IPC 桥：
标题栏（标题/布局/分P/换链接/设置/最小化/关闭）、控制条（播放/时间/进度拖拽/倍速/静音）、
链接栏、toast、2.6s 自动隐藏、16:9 舞台与 webview 尺寸同步。

自检 5/5：几何 30/34、控件状态、进度拖动 seek、自动隐藏、URL 条推舞台。截图 `docs/m6-ui.png`。

**四个隐蔽的坑**：`file://` 页面拿不到 IPC（wry 不注入，改用编译期内联）；
自动隐藏条件写反；`#urlbar` 默认可见把舞台顶下去；Rust 每帧推 `showUrlbar:false` 覆盖页面开关。

## 实战联调（2026-11-22）

第一次把整套丢进真实游戏（原神）跑：浮窗压得住、标题栏能拖、`Alt+I` 正常、手柄 L3 弹盘正常、
识别 3330 帧（截帧 11.1ms / 检测 0.1ms）。**功能可用**。

过程中抓到 7 个只在真实环境暴露的问题并全部修掉——最典型的三个：
`-webkit-app-region: drag` 在 WebView2 里会吞掉标题栏鼠标事件；
`WS_EX_NOACTIVATE` 宿主下 WebView2 不派发 DOM 鼠标事件（表现为"第一次点击只激活"）；
浮窗被遮挡时截屏截到的是上面那个窗口，会报出**假方位**（现已加遮挡保护，宁可暂停）。

详见 `docs/LIVE-TEST.md`。

## M12：打包（2026-11-22）

```powershell
cd gifa-rs
powershell -ExecutionPolicy Bypass -File scripts/build.ps1   # → dist/GIFA-0.2.6-win-x64.zip
```

- **单文件 exe**（1.7 MB）：页面/样式/脚本编译期内联，不需要附带资源文件
- **清单注入**：`requireAdministrator`（release）+ PerMonitorV2 + longPathAware，走链接器
  `/MANIFESTINPUT`，不依赖 `rc.exe`
- **debug 不提权**：否则自检一跑就要 UAC；`GIFA_NO_ADMIN=1 cargo build --release` 可出不提权的
  release 用来验证二进制本身
- 数据目录：release 走 `resolve_data_dir`（安装版 `%APPDATA%` / 绿色版 `<exe>\data`）
- WebView2 缺失或 <110：弹框给下载地址后退出
- 未做：NSIS 安装器（本机无 `makensis`）、exe 图标与版本资源（本机无 `rc.exe`）

## M13：回归门禁（2026-11-22）

```powershell
powershell -ExecutionPolicy Bypass -File scripts/check.ps1
```

一条命令 17 项：编译、单测（105）、clippy（0 warning）、10 个自检、识别冒烟、性能预算。
退出码 0/1，可直接接 CI。

性能实测：**冷启动 896ms / 内存 24MB / CPU 0.2%**（预算 2500ms / 200MB / 10%）。
内存这么低是因为识别只截 216×216 的采样窗，不是整屏。

视觉回归一期只做冒烟（`--bearing N`）；黄金帧全量对拍属二期。

## 库分层（#2 骨架）

| crate | 依赖 | 职责 | 测试 |
|---|---|---|---|
| `gifa-core` | 无 OS 依赖 | 设置模型、方位布局、快捷键解析、播放状态机 | 28 |
| `gifa-input` | `gifa-core` + `windows` | XInput 轮询 + 圆盘状态机 + 热键注册/钩子/交换回滚 | 18 |
| `gifa-vision` | 无 | 帧源抽象 + 青色分数/自适应阈值（二期算法骨架） | 8 |
| `gifa-player` | 无 | B 站播放协议 + 页面 agent | 6 |
| `gifa-shell` | `gifa-player` | 两个窗口、区域、状态机、WebView2 宿主 | 12 |
| `gifa-demo` | 全部 | M2/M3 的应用装配与自检 | 2 |

合计 **80 个测试**，`cargo clippy --workspace --all-targets` 0 warning。

设计约束：**core 不依赖 windows crate** —— 设置迁移、布局规则、快捷键冲突这些最容易
出错的逻辑因此能在 CI 里跑，不需要图形环境。

## M4：全局热键（2026-11-21）

两套机制合成一个 `HotkeyEvent` 流：

| 绑定 | 机制 | 原因 |
|---|---|---|
| 组合键 | `RegisterHotKey` | 系统级独占，游戏前台也收得到 |
| 单键 / 侧键 / 按住类 / 循环类 | `WH_KEYBOARD_LL` / `WH_MOUSE_LL` | 单键走注册会**全局吞键**；鼠标键注册不了；按住类要配对松开 |

冲突：占用则**交换**，任一注册失败**整体回滚**。钩子被系统摘掉时 `rearm()` 先摘再挂。

**UIPI 是本轮最重要的发现**：低级钩子只能收到"前台窗口完整性级别 ≤ 自己"的按键 ——
所以游戏以管理员运行时，非提权的 GIFA 收不到任何热键。这把 M12 的清单提权变成了硬需求，
也解释了自检在有提权窗口在前台时会整体 SKIP。详见 `docs/HOTKEYS.md`。

## WebView2 基线（#3）

- 最低 **Chromium 110**（覆盖 UA 覆盖 / 透明背景 / 二期 SharedBuffer）
- 本机 `157.0.4341.60` 实测达标：`gifa-demo --runtime` → `RESULT 2/2`
- 细节与分发策略见 `docs/WEBVIEW2.md`；Win10 22H2 干净机仍待实测

## 三个坑（写代码时别踩）

1. **`WS_THICKFRAME` 有 ~8px 非客户区** —— 必须用 `WM_NCCALCSIZE` 把客户区顶回整窗，否则子窗和 WebView2 整体内缩、窗口边缘漏出别的程序。
2. **借用与重入**：`SetWindowPos` 会同步回调 `WM_WINDOWPOSCHANGED`。凡是要触发窗口消息的公开 API，**先取 HWND、放下借用再调用**；页面布局变化走 `WM_APP_RELAYOUT`，等借用放下再 `sync()`。
3. **thread_local 状态跨线程是空的**：`post_tick` / `eval_chrome` / `close` 都可能被后台线程调用，所以句柄存了一份全局副本、JS 走队列 + `WM_APP_EVAL` 回 UI 线程执行。
4. **页面上报函数名要和 Rust 调的一致**：`eval_chrome("window.gifaState(...)")` 对不上页面里定义的 `gifaSet` 时是**静默无效**（`&&` 短路），UI 会一直显示"未载入"。M2 就踩了一次 —— 页面导出统一用 `window.gifaState`。
