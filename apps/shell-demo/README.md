# shell-demo

`gifa-shell` 骨架的可运行演示 + 自检。

## 模式

| 命令 | 行为 |
|---|---|
| `cargo run -p shell-demo` | 自检 6 项后退出（见下表） |
| `cargo run -p shell-demo -- --drag-test` | 模拟真实鼠标拖右下角，验证缩放链路 |
| `cargo run -p shell-demo -- --hold` | 只起窗，不测 |
| `cargo run -p shell-demo -- --hold --urlbar` | 起窗并自动展开 URL 条（看舞台下移） |

## 自检项

1. 客户区 == 整窗（`WM_NCCALCSIZE` 生效）
2. chrome 矩形跟随舞台窗
3. chrome 的 owner 是舞台窗（z 序）
4. 区域挖空与命中归属（顶栏/pill/grip → chrome；舞台 → 浮窗）
5. 缩放后联动 + 区域重算
6. `LWA_ALPHA` 不透明度

## 两个页面

- `web/chrome.html`：30px 标题栏 / 40px URL 条 / 34px 控制条 / 悬浮 pill / 右下 grip。
  加载后与每次 resize 都通过 IPC 上报布局（`{t:'layout', titlebar, footer, urlbar, overlay}`，**物理像素**）；
  标题栏按下发 `{t:'drag'}`，grip 按下发 `{t:'resize'}`。
- `web/stage.html`：B 站播放器占位，顶边一条绿线用来看舞台从哪开始。

截图：`docs/shell-urlbar.png`（URL 条展开状态；区域 5 块、舞台 464px）。
