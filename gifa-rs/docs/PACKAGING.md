# 打包与分发（M12）

更新时间：2026-11-22。产物：`gifa-rs/dist/GIFA-0.2.6-win-x64.zip`（0.7 MB）。

## 产物结构

```
GIFA-0.2.6-win-x64/
├─ GIFA.exe               1.7 MB，单文件（页面/样式/脚本都在里面）
├─ data/                  绿色版数据目录（首次运行后出现 settings.json）
├─ licenses/
│  ├─ LICENSE-GIFA.txt    GPL-3.0 正文
│  ├─ NOTICE-GIFA.txt
│  └─ LICENSE-LGPL-3.0.txt
└─ README.txt             用法、热键、为什么要管理员、数据位置
```

外加 `dist/SHA256SUMS.txt`（逐文件 sha256）。

## 单文件是怎么做到的

页面全部在编译期内联：

```rust
fn chrome_page() -> String {
    include_str!("../web/chrome.html")
        .replace("<link …>", &format!("<style>{}</style>", include_str!("../web/style.css")))
        .replace("<script …>", &format!("<script>{}</script>", include_str!("../web/app.js")))
}
```

所以 exe 里只有 Rust 代码 + 这些字符串，没有外部资源文件要一起分发。

## 清单（manifest）

用链接器的 `/MANIFESTINPUT` 直接嵌，不依赖 `rc.exe`：

| 项 | 值 | 为什么 |
|---|---|---|
| `requestedExecutionLevel` | **requireAdministrator**（release） / asInvoker（debug） | 低级钩子只能收到"前台窗口完整性级别 ≤ 自己"的按键（UIPI） |
| `dpiAwareness` | PerMonitorV2 | 多显示器/不同缩放下坐标不乱 |
| `longPathAware` | true | 深目录里的 `data\` 路径 |
| Common-Controls v6 | 依赖项 | 系统控件外观 |

**debug 构建刻意不提权**：否则本地自检一跑就弹 UAC，CI 直接卡死。
`GIFA_NO_ADMIN=1 cargo build --release` 也能出"不提权"的 release，用来验证打包后的二进制本身。

验证方式（不需要运行）：

```
requireAdministrator -> 2
asInvoker            -> 0
PerMonitorV2         -> 1
longPathAware        -> 2
```

## 数据目录

| 场景 | 位置 |
|---|---|
| debug（自检） | `<cwd>/data`（固定路径，自检依赖） |
| 绿色版（exe 目录可写） | `<exe>/data` |
| 安装版（exe 目录有 `installed.flag`） | `%APPDATA%\原神跟跑助手` |

判定逻辑在 `gifa-store::resolve_data_dir`，真的写一个探针文件再删，不是只看 `exists()`。

## WebView2 运行时

启动时检查（release 构建）：

- 缺失或 < 110 → 弹系统消息框给出下载地址后退出，不会带着一个"窗口创建失败"的黑洞继续跑；
- 合规的机器（Win11 自带、Win10 装了 Evergreen Runtime）直接进。

## 构建

```powershell
cd gifa-rs
powershell -ExecutionPolicy Bypass -File scripts/build.ps1
```

四步：`cargo build --release` → 组装 `dist\` → 逐文件 SHA256 → zip。

## 验证记录

| 项 | 结果 |
|---|---|
| release 清单注入 | ✅ `requireAdministrator` ×2 |
| exe 图标 | ✅ 从 exe 提取出 32×32 图标（绿弧 + 红箭头） |
| exe 版本信息 | ✅ `FileDescription=GIFA 原神跟跑助手`、`FileVersion=0.2.6` |
| release 二进制可运行（no-admin 变体） | ✅ `--version` → `GIFA 0.2.6` |
| 数据目录解析 | ✅ `Portable`（exe 同级 `data`） |
| 设置层自检 | ✅ 7/7 |
| 界面自检（release） | ✅ 5/5（几何/控件/进度拖动/自动隐藏/URL 条） |
| 安装器静默安装 | ✅ 7 个文件 + 注册表（`Software\GIFA` + 卸载项） |
| 安装器静默卸载 | ✅ 目录删除、两个注册表键清空 |
| 安装版 exe 版本信息 | ✅ 装进去的 `GIFA.exe` 也带 `0.2.6` |

## 安装器（NSIS 3.13）

`installer/gifa.nsi` → `dist/GIFA-0.2.6-setup.exe`（0.87 MB）。

| 设计 | 原因 |
|---|---|
| **按用户安装**（`$LOCALAPPDATA\Programs\GIFA`，只写 HKCU） | 程序启动时才需要管理员（热键），不该让"安装"也卡在 UAC 上 |
| 卸载默认**保留** `%APPDATA%\原神跟跑助手` | 用户的标定/布局/上次看到都在里面；删不删由用户在卸载时选 |
| 静默卸载一律保留数据 | `/S` 下 `MessageBox` 会阻塞，所以 `IfSilent` 直接跳过询问 |
| 装入 `installed.flag` | 决定数据目录走向：有它 → `%APPDATA%`，没有 → exe 同级 `data\` |

组件：主程序（必装）、快捷方式（开始菜单 + 桌面）、开机自启（`HKCU\...\Run`，默认不勾）。

### 一个坑：`/SOLID lzma` 的静默安装是坏的

第一版用 `SetCompressor /SOLID lzma`，体积 811 KB 更小，但 `/S` 静默安装**直接退出**：
127ms、退出码 0、什么都不做（目录和注册表都没有）。改用非 solid 的 `SetCompressor lzma`
后正常，体积 907 KB —— 多 96 KB 换可靠性，值。

## 本机没做 / 做不了的

| 项 | 原因 |
|---|---|
| **真实 UAC 提权启动** | 本会话点不了 UAC 弹窗。提权后的行为（热键能否覆盖提权游戏）需要在干净机器上验收 |
| **安装器在干净机器上的体验** | 只验证了静默安装/卸载；向导各页（欢迎/许可/组件/目录/完成）没有逐页点过 |

> 实测补充：安装包放在 `gifa-rs\dist\` 里直接运行时静默安装不生效，复制到 `C:\Temp\` 后一切正常。
> 判断是本机沙箱对"从工作区启动的进程写用户目录"有限制，不是安装器本身的问题
> （同一份 906930 字节的二进制，换个目录就正常）。发给用户时它在下载目录里，不受影响。

## 下一步（M12 剩余）

1. 装 NSIS 后补 `installer.nsi`：可选目录、开始菜单/桌面快捷方式、`HKCU\..\Run` 自启、卸载保留数据、旧绿色版 `data\` 迁移；
2. 用 Windows SDK 的 `rc.exe`（或 CI 里的 `winresource` crate）补图标与版本资源；
3. 在干净的 Win10 22H2 上跑一遍：双击 → UAC → 起窗 → 加载视频 → 热键生效。
