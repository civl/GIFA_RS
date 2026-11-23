# 真机语料采集与标注

目标：拿到**覆盖全朝向**的真机小地图帧 + 人工真值，用来回答"某个通道在哪个朝向上偏了多少"。

## 为什么不用录屏软件（OBS / NVIDIA ShadowPlay）

它们能录，但**不适合当语料**：

- 录屏是 H.264/HEVC，色度 4:2:0（宽度减半）+ 有损压缩。青色箭头的边缘会被抹开 1~2px、色相偏移。
- 我们的识别链路吃的是 BitBlt 抓的**原始 BGRA**，没有经过编解码。

两者不是同一个分布。而我们正要修的是"v1 在真机上有 ~9° 系统偏差"这种细节 ——
用被压缩过的帧定标，很可能把压缩带来的偏移当成算法偏差修掉，得到假的修正。

用来**回放核对效果**没问题，用来**定标**不行。

## 正确做法：程序自带落盘

### 0. 准备

```powershell
cd G:\Code\GenshinImpact\GIFA
mkdir data\corpus\m1
```

> `--dump-frame` 会自动建目录，但 `--vision-csv` **不会**（父目录不存在时静默失败）。

### 1. 录制（每段约 45 秒）

先跑一次自动定心，确认圆心找对了，顺便看箭头多少像素：

```powershell
.\target\debug\gifa-demo.exe --game 3 --autocenter
```

输出里会有 `自动定心：cx=… cy=… r=…（青色峰值 …，箭头 NNpx）`。
`箭头 NNpx` 很关键：<40px 走 v3、≥40px 走 v1（见 `detect.rs` 的 `ARROW_PX_PREFER_V1`）。

然后正式录：

```powershell
$env:GIFA_DUMP_ALL="1"
.\target\debug\gifa-demo.exe --game 45 `
    --roi "0.0861,0.1149,0.050" `
    --dump-frame data\corpus\m1 `
    --dump-count 2000 `
    --vision-csv data\corpus\m1.csv
```

**录制期间要做的动作**（这是语料有没有价值的关键）：

- 站在开阔地，用鼠标**原地缓慢转一圈 360°**（约 30~45 秒转完）
- 不要快速甩视角（会糊、也会触发滤波的跳变门）
- 不要进菜单、不要传送、不要让弹窗挡住小地图

**要录几段**：至少 3 段，换不同地点（草地 / 城里石板 / 雪地 / 水边），
背景差异比数量更重要。

### 2. 抽样

10 fps 下 45 秒 ≈ 450 帧，人工标太多。抽到 50~70 帧足够定中位误差和分朝向偏差：

```powershell
python scripts\corpus-subsample.py data\corpus\m1 data\corpus\m1-sub 8
```

### 3. 生成标注页

```powershell
python scripts\label-tool.py data\corpus\m1-sub data\corpus\m1.csv data\corpus\m1-label
```

产出 `data\corpus\m1-label\label.html`，**双击用浏览器打开**（离线，不依赖服务器）。

### 4. 标注

页面上：

- **红线** = 算法识别的方向
- **绿线** = 你标的方向（在图上按住拖动）
- **空格** = 采纳红线（算法值）
- **U** = 这帧没箭头/看不清，标"坏帧"（会被排除）
- **方向键** = 上一帧 / 下一帧
- **S** = 保存

> ⚠️ **不要盲目按空格。**
> 空格是把算法的值直接当答案，标完的语料等于"算法自己"，再拿去测算法必然 100% 准确，
> 这个语料就白标了。正确做法是：**看箭头尖往哪指，绿的拖过去**；只有当你确认
> 红线确实压在箭头尖端上时，才按空格省事。

标完点「导出」，把 `labels.json` 放到 `data\corpus\m1-label\` 下。

### 5. 先自己看一眼报告

```powershell
python scripts\label-report.py data\corpus\m1-label\labels.json
```

它会给出：算法在你标的帧上多准、`angMargin` 这类判据的区分度。

## 交给我什么

整个 `data\corpus\` 目录（每段约 7~10 MB）即可。我会：

1. 把语料并进 `docs/calibration/`，`detect-eval` / `corpus-eval` 直接能跑
2. 分朝向看 v1/v3 的误差曲线 —— 这能回答"v1 的 9° 到底是常数偏置还是随朝向变化"
3. 决定 40px 那条主通道阈值要不要挪

## 附：现有语料的来源

| 语料 | 帧数 | 说明 |
|---|---|---|
| `apps/gifa-demo/data/gamelabel/` | 39 | 真机 2560×1440，**只覆盖 315°~360° 一个朝向区间** |
| `apps/gifa-demo/data/labelseq/` | 138 | 视频（B站 UP 主把游戏小地图放大 + 加了"北南西东"），不代表真机分布 |

真机那份朝向覆盖不全，正是 `v1 9°` 结论只能在一个扇区成立的原因。
