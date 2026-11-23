# 浮窗界面移植（M6）

更新时间：2026-11-22。对应 task.md M6。

## 搬了什么

| 文件 | 来源 | 说明 |
|---|---|---|
| `web/style.css` | 原版 `src/renderer/style.css` | **逐行照搬**（742 行），配色/字号/间距/响应式断点全部沿用 |
| `apps/gifa-demo/web/chrome.html` | 原版 `src/renderer/index.html` | 内联 SVG 图标表、标题栏、URL 条、舞台、控制条、缩放手柄、toast |
| `apps/gifa-demo/web/app.js` | 原版 `src/renderer/app.js` 的**桥接改写** | 交互照搬，`window.api` / `<webview>` 换成本项目 IPC |

图标用内联 `<symbol>` 而不是 emoji：emoji 在不同系统字体下大小、颜色、基线都不一样，
浮窗缩到 240px 时会糊成一团。

## 桥接

```
页面 → Rust ：window.ipc.postMessage(JSON)       { t: 'load' | 'seek' | 'rate' | 'drag' … }
Rust → 页面 ：window.gifaState(state)            每 250ms 推一次完整状态
            window.gifaCalib(...)               标定（测试/自动化）
            window.gifaProbePost()              读回界面几何（自检）
```

尺寸契约（原版定值，改动必须两边同步）：

```
标题栏 30px + 控制条 34px；窗口 = 16:9 画面 + 64
```

页面每帧上报 `{titlebar, footer, urlbar, overlay[]}`，外壳据此算舞台矩形和 chrome 窗区域；
区域外的像素点击穿透，所以视频能正常播放/暂停。

## 交互清单

- 标题栏：视频标题、布局下拉、分 P 下拉（`P3 · 标题（12:34）`，当前 P 选中）、换链接、设置、最小化、关闭
- 控制条：播放/暂停（图标随状态切）、时间、进度条（拖拽即时 seek，拖时暂停状态刷新）、倍速（1 → 1.25 → 1.5 → 2 → 0.75）、静音
- 链接栏：粘贴 → 载入（回车/按钮），Esc 收起
- 自动隐藏：播放中且鼠标静止 2.6s，控制条淡出（暂停/拖动/标定时不隐藏，鼠标在标题栏或 URL 条上也不隐藏）
- 标题栏拖动走外壳的系统移动循环，右下角手柄走 `HTBOTTOMRIGHT`

## 自检

```powershell
cd apps/gifa-demo
cargo run -p gifa-demo -- --open BV1xx411c7mD --ui-test
cargo run -p gifa-demo -- --open BV1xx411c7mD --ui-test --ui-hold   # 保持窗口
```

| # | 项 | 实测 |
|---|---|---|
| 1 | 几何：标题栏 30 / 底栏 34 | `titlebar=30 footer=34` |
| 2 | 控件：布局下拉 / 分P / 播放态 | `layouts=3 playing=true speed="1.0×"` |
| 3 | 进度条拖动 → 发出 seek（50%） | `seek=1028.0`（时长 2056 → 一半） |
| 4 | 自动隐藏（静止 2.6s） | 已收起 |
| 5 | URL 条展开后舞台下移 | `stage.y=69`（30 + 39） |

截图：`apps/gifa-demo/docs/m6-ui.png`（896×568，标题栏 + B 站页面 + 控制条）。

## 本轮踩的坑（都很隐蔽）

1. **`file://` 页面拿不到 IPC**：一开始用 `chrome_url = file:///…/chrome.html` 让页面自己加载
   `style.css` / `app.js`。界面显示完全正常，但 `window.ipc.postMessage` 是 `undefined` ——
   wry 的 IPC 脚本对 `file://` 页面不注入。表现是"页面看着能用，一句话也传不回来"。
   改成编译期把 CSS/JS **拼进同一个 HTML**（`chrome_page()`），问题消失。

2. **自动隐藏条件写反**：写成 `if (!paused) return` —— 变成"只有暂停时才隐藏"。
   原版是 `if (!playing) return`（播放中才收）。

3. **URL 条默认可见**：原版标记里 `#urlbar` 没有 `hidden` 类，是 `app.js` 启动时收起的。
   我漏了这一步，结果舞台一开始就被顶下去 39px，第一次点"换链接"反而是把它收起来。

4. **Rust 每帧推 `showUrlbar:false` 把 URL 条压回去**：页面自己管开关，状态里就不要带这个字段。

## 已知限制

- 设置按钮目前只弹 toast（启动器面板还没做，M6 之后接）。
- 悬停预览（进度条 hover 显示时间气泡）没做：原版也没有，属于可选增强。
- 分 P 下拉在未加载视频时隐藏；页面内切 P 的同步依赖 M3 的导航事件。
