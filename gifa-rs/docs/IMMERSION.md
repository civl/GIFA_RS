# 沉浸与鼠标避让（M7）

更新时间：2026-11-21。对应 task.md M7。

## 进入沉浸做了什么

```
1. 记下原窗口矩形（用于退出时精确还原）
2. 隐藏 chrome 层（标题栏/控制条/URL 条一起消失）
3. 强制点击穿透（WS_EX_TRANSPARENT）——沉浸时鼠标完全不属于浮窗
4. 把舞台矩形置零留白（titlebar/footer = 0）→ 舞台 WebView2 占满客户区
5. 按视频比例把窗口缩到"原矩形内刚好贴边"并居中
6. 通知页面 agent：presentation{videoOnly:true} → 只留 <video>
7. 起 60ms 心跳做鼠标避让
```

**不写设置**：沉浸是运行时状态，重启不会自动进入（与原版一致）。

## 退出时还原

```
1. 停心跳、清 hovered
2. 先恢复不透明度（避免半透明窗口还原时闪一下）
3. SetWindowPos 回原矩形 + ±2px 舍入二次修正
4. 显示 chrome、恢复原穿透状态、把布局还给页面上报的那份
```

舍入修正是必要的：系统对 `SetWindowPos` 会做 1~2px 取整，
读回实际值再补一次差值，才能做到"逐像素还原"（自检里对拍 `(80,80,896,568)`）。

## 按视频比例贴合

`fit_immersion(aspect)`：`aspect = videoWidth / videoHeight`（由应用层从播放状态传入）。

- 只会在**原矩形内**缩小，绝不放大；
- 同时不超过工作区；
- 以原矩形中心为中心；
- 宽高都算一遍，避免窄窗口时算出负高度。

实测：视频 480×360（4:3）→ 窗口 896×568 里贴合成 757×568，比例 1.333，居中且不越界。

> 注意：B 站未登录时可能是 4:3（480×360），不是 16:9。自检的期望比例取自**真实视频分辨率**，
> 不能写死 16:9 —— 第一版就是这么写错的。

## 鼠标避让

- 只在"沉浸 + 开关打开"时生效；
- 60ms 轮询光标（`SetTimer` + `WM_TIMER`），进出矩形各有 **120ms 防抖**；
- 移入降到 `min(用户设定, 0.1)`（约 26/255），移出恢复；
- 临时值**不写设置**：`base_opacity` 单独存用户设定值。

## 自检

```powershell
cd gifa-rs/apps/gifa-demo
cargo run -p gifa-demo -- --open BV1xx411c7mD --immersion-test
cargo run -p gifa-demo -- --open BV1xx411c7mD --immersion-test --immersion-hold   # 保持沉浸，便于截图
```

| # | 项 | 实测 |
|---|---|---|
| 1 | 按视频比例贴合且居中 | 1.333（期望 1.333）/ 757×568 / inside / centered |
| 2 | 隐藏 chrome + 强制穿透 | `chrome_visible=false` `click_through=true` |
| 3 | 鼠标移入 → 变淡到 10% | `hovered=true` `opacity=26` |
| 4 | 鼠标移出 → 恢复 | `hovered=false` `opacity=255` |
| 5 | 退出后尺寸精确还原 | `(80,80,896,568)` 前后一致 |
| 6 | 退出后 chrome 与穿透复原 | 可见、未穿透、`immersion=false` |

截图：`apps/gifa-demo/docs/m7-immersion.png`（757×568，页面只剩视频，无标题栏/控制条）。

## 本轮抓到的两个 bug

1. **`presentation` 字段名不匹配**：Rust 侧 `video_only` 序列化成蛇形，agent 读的是 `m.videoOnly`
   → 拿到 `undefined` → `setVideoOnly(false)` 静默失效。截图上"页面还带着 B 站顶栏"才暴露出来。
   修法：`#[serde(rename = "videoOnly")]`，并加单测固定这个字段名。
2. **沉浸时舞台没占满客户区**：窗口按 4:3 缩好了，但舞台 WebView2 还按"标题栏 30 + 控制条 34"留白，
   于是视频被塞进 757×504（1.5:1）里出现黑边。修法：沉浸期间把布局清零，
   并忽略隐藏的 chrome 页在 resize 时的上报（存起来等退出再用）。

## 环境依赖（重要）

鼠标避让的自检要靠 `SetCursorPos` 把光标移进/移出窗口。**锁屏、远程会话或没有交互桌面时，
`SetCursorPos` 会直接失败**（返回 false），这项就判 `SKIP` 而不是 `FAIL` —— 判据是代码路径，
不是"这台机器现在允不允许操作光标"。同一环境下 `SendInput` 也会被丢（见 `HOTKEYS.md` 的 UIPI 一节）。

## 已知限制

- 游戏使用**独占全屏**时窗口盖不上去（Windows 限制），UI 里提示改窗口化/无边框；
- 沉浸期间不响应拖动/缩放（`interactive=false`，chrome 也隐藏了）；
- 多显示器增删时按**当前窗口比例**重新贴合（`WM_DISPLAYCHANGE`），
  精确比例要等应用层下一次进入或页面报新分辨率；
- 鼠标避让只测到"系统光标进出矩形"，游戏锁定/自绘光标的情况需要实机验收。
