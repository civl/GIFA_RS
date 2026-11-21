# M1 前置 spike 结论（2026-11-21，Windows 11 26300 / WebView2 157.0.4341.60）

跑法：

```powershell
cd gifa-rs
cargo run -p dual-webview-spike                 # 子窗模式（对照组）
cargo run -p dual-webview-spike -- --toplevel   # 顶层玻璃窗模式（结论采用）
cargo run -p dual-webview-spike -- --toplevel --hold   # 保持窗口，便于人工看/截图
```

自检会打印 5 项 PASS/FAIL 并自动退出。截图工具：`tools/shot.ps1`（按标题取窗口矩形 → 存 PNG）。

## 一、结论（直接影响架构）

| 问题 | 结论 |
|---|---|
| chrome 层怎么叠在舞台上面？ | **必须是独立顶层窗，不能是子窗**。子窗里的透明 WebView2 只能透出「自己这块表面」的背景，透不到下面的兄弟窗；顶层窗 + `DwmExtendFrameIntoClientArea(-1)` 才能逐像素透出下层的舞台窗。 |
| 点击怎么穿过 chrome 落到 B 站页面？ | **`SetWindowRgn` 把 chrome 窗"挖空"**：区域只保留顶部条 / 底部条 / 浮动元素（URL 条、提示、标定环）。区域之外不存在窗口，命中测试自然落到舞台。不需要 `HTTRANSPARENT`，也不需要父窗代转命中测试。 |
| 不透明度能不能一次搞定？ | **能**。给浮窗调 `SetLayeredWindowAttributes(LWA_ALPHA)` 会连子 WebView2 一起变淡。注意 DWM 重合成有延迟：**采样必须等 ≥600ms**，450ms 时会读到旧像素（本次就踩了，误判过一次）。 |
| 舞台/界面各放几个 WebView2 进程？ | 2 个（舞台 + chrome），与 Electron 原版的 `<webview>` + 渲染进程结构一一对应。 |

### 五项目检结果

```
模式：toplevel / chrome 层 = 独立顶层玻璃窗
[1] chrome 层渲染（顶部条）   PASS  采样 (27,29,36)，期望 ≈(27,29,36)
[2] 舞台透出（红块原色）      PASS  采样 (255,0,0)
[3] 透明叠加（pill 混色）     PASS  采样 (52,14,14)   ← 理论值 0.85*(16,16,16)+0.15*(255,0,0)
[4] 命中测试归属             PASS  标题栏→Chrome / 舞台→Stage / pill→Chrome
[5] 窗口 alpha 传递          PASS  255:(255,0,0) → 128:(255,127,127)
RESULT 5/5
```

对照组（chrome 作为子窗）：4/5，**第 3 项失败** —— pill 采样 (18,19,19)，即透明区透出的是 chrome 子窗自己的背景刷（24,32,32）混合结果，而不是下面的舞台。这是"子窗方案不可行"的直接证据。

截图：`docs/spike-toplevel.png`（红=舞台页，顶部 30px / 底部 34px 是 chrome 层，中间半透明 pill 能透出舞台上的白色按钮）。

## 二、踩到的坑（都已写进 task.md §8）

1. **`WS_THICKFRAME` 会带来 ~8px 非客户区**。带它时，子窗与 WebView2 整体内缩，窗口四边漏出后面的其他窗口（第一次截图就是这样，边缘能看到别的程序文字）。真外壳要可拖角缩放，就必须在 `WM_NCCALCSIZE` 里把客户区顶回整个窗口。
2. **wry 的 `Rect` 要用 `Physical`，不能用 `Logical`**。我们是 PerMonitorV2、按物理像素算布局；用 `Logical` 会被再乘一次 DPI 缩放。
3. **`DwmExtendFrameIntoClientArea` 在 windows crate 0.62 里受 `Win32_UI_Controls` feature 控制**（`MARGINS` 在同模块）。不引这个 feature 会报"found an item that was configured out"。
4. **WebView2 不会跟随宿主窗口尺寸**。`WM_SIZE` 里必须显式 `webview.set_bounds(...)`。
5. **DWM 重合成延迟**：改 alpha / 改区域后立刻 `GetPixel` 会读到旧像素。自检里改成 900ms 后稳定。
6. 顶层 chrome 窗要 `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`，并且每次布局都用 `SetWindowPos(HWND_TOPMOST)` 重新压到浮窗之上（两者同属 topmost 带，靠顺序决定上下）。
7. 自检用的 `elementFromPoint` 回传（IPC）非常好用：这次"截图里为什么有个白框"就是靠它三分钟定位到是测试页 `#cta` 漏写 `position:absolute`，而不是合成 bug。

## 三、对 task.md 的修订

- §0.1 增加 **D3**：浮窗 = 「舞台窗」+「独立顶层 chrome 玻璃窗」两层。
- §3.3 行为映射表：鼠标穿透、不透明度两行按实测结论改写。
- §4：`gifa-shell` 下拆成 `float.rs`（舞台窗）/ `chrome.rs`（顶层 chrome 窗）/ `overlay.rs`（方位叠加层，同一套玻璃窗机制）。
- §6 M1：前置 spike 标记为**已完成**，剩下的 M1 工作改为"按结论实现 + 补齐 WM_NCCALCSIZE 可缩放"。
- §8：补上上面 7 条坑。
- §9 R7：从"中/中"降为"低/中"（架构已用可运行代码验证）。
