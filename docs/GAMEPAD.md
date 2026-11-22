# 手柄与八方向圆盘（M5）

更新时间：2026-11-21。对应 task.md M5。

## 输入源：直接读 XInput

```
gifa-input::xinput::start_polling()  →  每 16ms  XInputGetState(0..4)
                          ↓ 只有"帧变了"才推
GamepadEvent::Frame { slot, frame }  →  gamepad::step()  →  ChordStep
```

为什么不用浏览器的 Gamepad API（原版结论）：Chromium 要求文档收到过**真实用户手势**才启动手柄监视，
而浮窗是"不抢焦点、鼠标穿透、从不 show"的宿主，永远拿不到手势。XInput 是 OS 层轮询，与焦点无关。

为什么不引 `gilrs`：只需要 4 个槽位的 `XInputGetState`。直连导出函数少一条依赖链，
行为也更好预期（找不到 DLL 就明确报错）。

**坐标陷阱**：XInput 的 Y 轴向上为正，Gamepad API / `Frame` 是向下为正。
两根摇杆的 Y **必须取反**，否则圆盘的南北整体反过来。`xinput::frame_from_state` 里有单测固定这条。

## 状态机：三级幅值

| 幅度 | 行为 |
|---|---|
| `< deadzone(0.22)` | 回中：算作中心 |
| `≥ latch(0.55)` | 推到位：锁定扇区并高亮（之后抖回 0.45 也保持） |
| `< commit(0.35)` | 触发：一次性动作在这里 fire |

三个数值各司其职：锁存免疫扇区边界抖动（不需要角度滞回），
回中阈值比锁存低一截，避免"手抖一下就被判成回中"。

其它两条防误触：

- **按住 L3 时杆就没回中** → `awaiting_release`，必须先回中一次才允许触发；
- **推杆后立刻抬 L3**（没回中） → 一次性动作作废，但**按住类动作要补发 Up**（否则播放会卡在临时倍率）。

扇区下标即角度：0 = 正东，顺时针，`['e','se','s','sw','w','nw','n','ne']`，与原版一致。

## 圆盘窗口

惰性创建（第一次按住 L3 才建），`296×296`（`radius 132 × 2.24`）：

| 属性 | 为什么 |
|---|---|
| `WS_EX_LAYERED` + `WS_EX_TRANSPARENT` | 点击全部穿透到游戏，圆盘只是 HUD |
| `WS_EX_NOACTIVATE` / `WS_EX_TOOLWINDOW` | 不抢焦点、不上任务栏 |
| `WS_EX_TOPMOST` | 压在浮窗和游戏之上 |
| `DwmExtendFrameIntoClientArea(-1)` | 逐像素透明，能透出下面的视频/游戏 |
| 每次弹出重贴浮窗中心 | 浮窗可能刚被拖过 |

画的是 8 个扇区 + 金色高亮 + 中心提示（"推杆选择" / "松杆触发" / "回中恢复"），
标签取自动作的窄名（`Action::short()`）。

## 动作路由

`dispatch_action` 把动作接到已有能力上：

| 动作 | 行为 |
|---|---|
| `togglePlay` | 播放器 toggle |
| `seekFwd` / `seekBack` | ±5 秒 |
| `prevPart` / `nextPart` | 切分 P（带上下界） |
| `cycleSpeed` | 1 → 1.25 → 1.5 → 2 |
| `toggleOpacity` | 1.0 ↔ 0.65（`set_opacity`） |
| `holdSpeed` | 按下切临时倍率、松开回常规倍率 |
| `toggleVisible` | 隐藏浮窗 |
| `togglePass` / `toggleImmersion` | **未实现**（要等 M7 的穿透/沉浸能力），只记日志 |

## 验证

### 合成帧自检（不需要手柄）

```powershell
cd apps/gifa-demo
cargo run -p gifa-demo -- --gamepad-test
```

用合成 `Frame` 走**同一条** `step_chord → 圆盘 → 动作` 链路，6 项：

| # | 项 | 实测 |
|---|---|---|
| 1 | 按住 L3 → 圆盘出现 | `IsWindowVisible=true` |
| 2 | 推正东 → 高亮 e | `seen_highlights=["e"]` |
| 3 | 回中 → 触发 seekFwd | `fired=["●seekFwd"]` |
| 4 | 松开 L3 → 圆盘消失 | `visible=false` |
| 5 | 推杆后立刻抬 L3 → 不误触 | 新增触发 0 条 |
| 6 | XInput 可用 | `xinput*.dll` 已加载 |

截图：`apps/gifa-demo/docs/m5-disc.png`（圆盘压在视频上，e 扇区金色高亮，中心"松杆触发"）。

### 实体手柄（待实机）

```powershell
cargo run -p gifa-demo -- --gamepad
```

插上手柄，按住 L3 弹盘、右摇杆选方向、松开触发。**本机没插手柄，这一步未做。**

## 已知限制

- 没插手柄时轮询会每 16ms 空转一次 `XInputGetState`（4 个槽位，成本极低），不会产生事件流；
- 手柄断连/切槽位会重置会话并强制"先松开再响应"，避免幽灵按下；
- 休眠/锁屏时应调用 `cancel_hold`（M7 接 `WM_POWERBROADCAST`）；
- 圆盘位置目前固定浮窗中心，原版可拖动并记忆（`settings.gamepad.disc`），留给 M11 设置层接线。
