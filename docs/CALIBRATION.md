# 小地图布局标定（M10）

更新时间：2026-11-21。对应 task.md M10。

## 数据模型（`gifa-core::bearing`，早在 #2 就完成）

| 概念 | 说明 |
|---|---|
| `Roi { cx, cy, r }` | 归一化：**混用**分母 —— `cx`、`r` 除以视频**宽**，`cy` 除以视频**高** |
| `Layout` | `id` + `name` + `roi` + `built_in` + `centered` + `revision` |
| `Layouts` | 布局表 + `default_layout_id` + `active_layout_id`，七个操作 |

七个操作：`select` / `set_default` / `remove` / `rename` / `create` / `calibrate` / `center`。
校验：名称 ≤40 字去重、`r∈[0.01,0.5]`、内置布局只读、`center` 要 revision 匹配（过期结果拒绝写回）。

内置两套预设：

```
你的影月月          cx=282/1920  cy=266/1080  r=276/1920
提瓦特高级特工穿山甲   cx=258/1920  cy=284/1080  r=260/1920
```

旧版迁移：`rois`（全局）+ `roiBy`（按视频）→ `legacy-N` 命名布局，
跳过两个 SEED 脏数据（`0.0875/0.122/0.046`、`0.148/0.222/0.163`）。

## 标定交互

```
用户点「标定」→ 页面开标定层（盖住舞台）→ 拖圆环 / 滚轮调半径
            → 页面报 cx/cy/r（舞台相对物理像素）
            → Rust 取 <video> 实际矩形（agent 的 video-rect 命令）
            → 去掉黑边（object-fit: contain）→ 归一化 ROI → 写进布局
```

两个关键换算：

1. **窗口坐标 → 舞台坐标**：页面在 `closeCalib` 里减掉标题栏 + URL 条的高度，
   所以 Rust 收到的是"相对舞台左上角"的像素。
2. **元素矩形 → 画面矩形**：`object-fit: contain` 时元素里可能有黑边，
   必须用 `gifa_core::content_rect` 先扣掉，否则 ROI 会偏。

三层断言都有单元测试：`roi_from_pixels`（混用归一化）、`content_rect`（黑边）、
`Layouts::calibrate/center`（revision 与越界）。

## 内置布局怎么处理

内置预设**只读**（原版语义）。对着内置布局做标定会**新建**一个"自定义 N"，
而不是改掉预设 —— 这样换博主视频时随时能切回去。

## 区域（为什么圆环能被点到）

标定层 `#calib` 只覆盖舞台（`top = 标题栏 + URL 条`），打开时页面把它加进
`layout.overlay` 数组上报 → 外壳把这块并进 chrome 窗的区域 → 圆环可见、可拖动。
关闭时移除并重新上报，区域恢复正常（舞台照常点击穿透）。

## 自检

```powershell
cd apps/gifa-demo
cargo run -p gifa-demo -- --open BV1xx411c7mD --calib-test    # 4 项
cargo run -p gifa-demo -- --open BV1xx411c7mD --calib-hold    # 保持标定界面，便于截图
```

| # | 项 | 实测 |
|---|---|---|
| 1 | 拿到 `<video>` 实际位置 | `(16,168,668,376) 480×360 contain dpr=1` |
| 2 | 标定层可命中（舞台区域归 chrome） | `Chrome`（关闭后是 `Float`） |
| 3 | 标定后生成合法自定义布局 | `自定义 1`，ROI 合法 |
| 4 | 布局切换（内置 ↔ 自定义） | 均可切 |

截图：`apps/gifa-demo/docs/m10-calib.png`（金色虚线圆环 + 底部"取消/完成"条 + 标题栏布局下拉）。

## 本轮修掉的两个 bug

1. **标定层高度为 0**：`#calib` 只写了 `left/right/bottom`、没写 `top`，
   `getBoundingClientRect()` 高度是 0 → 上报的区域是 0×0，圆环看不见也点不到。
   修法：打开时按"标题栏 + URL 条"算 `top`。
2. **圆环被摆到左上角、半径退化成 40**：`openCalib()` 里先量尺寸再加 `.active` 类，
   此时 `#calib` 还是 `display:none`，量到 0×0。修法：**先显示再量**。

## 已知限制

- 标定好的 ROI 目前只落盘，还没喂给识别 —— 识别在二期（`gifa-vision`），
  一期识别留在页面 JS 但 agent 里尚未接视觉。
- 自动定心（`center`）只有数据层与单元测试，UI 入口等识别接上后再开。
- 布局下拉在只有内置两项时也显示（原版是隐藏的），避免用户找不到入口。
