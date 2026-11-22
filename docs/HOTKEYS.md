# 全局热键（M4）

更新时间：2026-11-21。对应 task.md M4。

## 两套机制，一个事件流

| 绑定形态 | 机制 | 为什么 |
|---|---|---|
| 组合键（`Alt+Space`） | `RegisterHotKey` | 系统级独占注册，游戏在前台也收得到 |
| 单键（`J` / `F8`） | `WH_KEYBOARD_LL` 钩子 | `RegisterHotKey` 会**全局吞键**——绑了 `J` 之后任何程序都打不出 `J` |
| 鼠标侧键（`Mouse4/5`） | `WH_MOUSE_LL` 钩子 | `RegisterHotKey` 根本不支持鼠标 |
| 按住类（`holdSpeed`） | 钩子 | 需要配对的按下/松开 |
| 循环类（`cycleSpeed`） | 钩子 | 需要按物理按键去重，按住不能连续切档 |

分流判定在 `gifa_core::hotkey::Accelerator::needs_hook`，与原版 `hook-hotkeys.js` 一致。
两条链路都产出同一个 `HotkeyEvent`：

```rust
enum HotkeyEvent { Fire(Action), Down(Action), Up(Action) }
```

UI 线程每个 tick 用 `Hotkeys::drain()` 取走。

## 冲突交换与回滚

`Hotkeys::set_binding(action, acc)`：

1. `gifa_core::hotkey::swap_map` 算出新表（占用者拿到被改动作的旧键；对方原本没绑定就清空）；
2. 全量重注册（`UnregisterHotKey` 全部 → 逐条 `RegisterHotKey` / 重建钩子绑定表）；
3. **任何一条失败就回滚到旧表**并重新注册 —— 要么全成，要么不变。

自检里实测：`seekBack Mouse4 → Alt+Space；togglePlay 拿到 Mouse4`。

## 重新挂载（rearm）

Windows 会在进程被降级到后台/效率模式时**悄悄摘掉**低级钩子：`running` 还是 true、界面还显示"监听中"，
但按键再也进不来。`Hotkeys::rearm()` 先摘再挂（`UnhookWindowsHookEx` + `SetWindowsHookExW`），
成本几十毫秒。接线点：窗口最小化恢复、系统唤醒、浮窗重新显示。

## 关键约束：UIPI（这条决定必须提权）

**低级钩子只能收到"前台窗口完整性级别 ≤ 自己"的按键。**

也就是说：游戏以管理员运行时，非提权的 GIFA 收不到任何按键 —— 表现是"启动器在前台时热键好用，
一进游戏就全废"。这不是 bug，是 Windows 的 UIPI 规则，原版也因此把清单设成
`requireAdministrator`（README 里写明了"第一次双击会弹一次 UAC"）。

因此：

- 开发/自检时如果前台是提权窗口（比如提权的终端或编辑器），注入的按键**完全不会到达**钩子；
- 发布版必须走 M12 的 `requestedExecutionLevel=requireAdministrator`；
- 界面里要给"热键注册失败"的明确提示（`Failure::describe()` 已实现三种原因）。

## 自动自检

```powershell
cd gifa-rs
cargo run -p shell-demo -- --hotkey-test
```

用 `SendInput` 注入真实输入，走和用户按键完全一样的系统路径。6 项：
RegisterHotKey / F8 按下 / F8 抬起 / 左键钩子 / rearm 后仍生效 / 冲突交换。

**SKIP 语义**：如果注入的输入一次都没到达（键盘钩子调用数 0 且热键没触发），
说明前台窗口完整性级别更高（UIPI），这些项标 `SKIP` 而不是 `FAIL` ——
判据是"代码路径对不对"，不是"这台机器现在允不允许注入"。

### 实测记录

| 时间 | 结果 |
|---|---|
| 22:2x（前台为同级别进程） | `[1] PASS [2] PASS [3] PASS`，`[5] PASS`（rearm 后 F8 仍触发），`[6] PASS` |
| 22:3x（前台变为高完整性窗口） | 全部降级为 SKIP，`[6]` 仍 PASS |

结论：**机制本身验证过；注入式回归受前台完整性级别影响，不能作为唯一判据。**

### 人工验收（在真桌面 / 游戏里做）

1. `cargo run -p shell-demo -- --hold` 起窗（或 `gifa-demo`）；
2. 焦点切到游戏（窗口化/无边框），按 `Alt+Space` → 焦点/日志里应出现 `Fire(TogglePlay)`；
3. 按住 `F8` → `Down(HoldSpeed)`，松开 → `Up(HoldSpeed)`；
4. 点鼠标侧键（后退键）→ `Fire(SeekBack)`（需在启动器里把"后退 5 秒"绑成 `鼠标侧键 ◀`）；
5. 单键绑 `J` 后，在记事本里仍能打出 `J`（验证"被动监听不吞键"）。

## 已知限制

- 游戏用 Raw Input 独占键盘时，任何全局热键都收不到 —— 需要换键位（原版同样限制）。
- 鼠标侧键若被外设驱动（G HUB / 雷蛇）绑成宏，会被驱动先截走。
- 注入式测试对前台完整性级别敏感（见上），不能替代实机验收。
- `crates/gifa-input/examples/mouse_probe.rs` 是排查"钩子收不到鼠标"的独立探针，支持时用它。
