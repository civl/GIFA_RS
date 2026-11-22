# 代码签名与 SmartScreen

> 现象：下载后双击，弹出「Windows 已保护你的电脑 / Microsoft Defender SmartScreen 阻止了
> 无法识别的应用启动」，应用名 `GIFA.exe`，发行者「发布者未知」。

这不是病毒，是 **SmartScreen 对"没有签名 + 没有下载量"的文件的默认反应**。
本文说清楚它为什么出现、有哪些办法、各自代价，以及现在这个仓库怎么处理。

---

## 1. 它是怎么判定的

文件从浏览器/聊天工具下载后，NTFS 会打上 **Mark-of-the-Web**（`Zone.Identifier` 备用数据流）。
双击时 SmartScreen 看三件事：

| 检查 | 我们目前的状态 |
|---|---|
| 是否有**有效**的 Authenticode 签名 | ❌ 没有 |
| 签名证书是否属于已知的可信发布者 | ❌ 没有证书 |
| 这个文件/发布者是否有足够的**下载信誉** | ❌ 新项目，零下载量 |

三者都不满足 → "发布者未知"，弹窗。
注意：`CompanyName` 之类的**版本信息字段不参与判定**，填得再全也显示"发布者未知"。

---

## 2. 真正能消掉弹窗的四条路

### 2.1 EV 代码签名证书（唯一"第一天就没有弹窗"的办法）

* 价格：约 **¥3000–5000 / 年**（含硬件令牌或云 HSM）
* 效果：签名当天起，任何机器上都不弹；发布者显示为公司名
* 代价：要**企业/组织资质**（营业执照等），个人基本申请不下来
* 注意：证书必须放在硬件令牌/HSM 里，不能导出

### 2.2 OV 代码签名证书（较便宜，但还会弹一阵）

* 价格：约 **¥1000–2000 / 年**
* 效果：**签名有效、发布者能显示名字**，但 SmartScreen 仍会弹，直到这个证书攒够下载信誉
  （通常要几周甚至几个月、成千上万次下载）
* 适合：不着急、愿意慢慢养信誉的项目

### 2.3 Azure Trusted Signing（推荐的现实选择）

* 价格：**约 $9.99 / 月**（Microsoft 自家服务，证书由 Microsoft 签发）
* 门槛：
  * 个人可申请，但要求**至少 3 年的可验证身份历史**（企业则要求注册满 3 年）
  * 需要完成组织/身份验证（几天到几周）
* 效果：签名链可追到 Microsoft → SmartScreen 明显宽容，绝大多数机器第一次就放行
* 流程：注册 Azure → 建 Trusted Signing Account + Certificate Profile →
  CI 里用 `trusted-signing-cli` 或 Azure CLI 签名

### 2.4 SignPath.io Foundation（开源项目免费）

* 价格：**免费**（面向 OSS 项目，GPL-3.0 符合条件）
* 效果：在它们的云 HSM 里做签名，证书是签发给 **SignPath Foundation** 的
  —— 发布者会显示 "SignPath Foundation"，而不是你自己的名字
* 门槛：项目要公开、有一定活跃度；需要申请并通过审核
* 适合：**本项目**这种 GPL-3.0 开源项目，性价比最高

---

## 3. 不能解决问题的做法

| 做法 | 为什么没用 |
|---|---|
| 自签名证书 | 证书本身不在信任链里，SmartScreen 一样弹，有些情况下比不签更糟 |
| 改文件名 / 重新打包 | 判定看文件内容哈希 + 签名，不看名字 |
| 把 `CompanyName` 填满 | 版本资源不参与判定，已写全，照样"发布者未知" |
| 用户关掉 SmartScreen | 对用户不负责，别在文档里教 |
| 只在本地 `Unblock-File` | 只对那台机器那一次有效，别人下载还是要弹 |

---

## 4. 现在的实际做法

### 4.1 用户侧（已写进 `dist/README.txt`）

1. 弹窗里点 **「更多信息」→「仍要运行」**（截图里的第二个对话框就有这个按钮）
2. 或者先解除锁定再运行（推荐，能减少以后重复弹）：

   ```powershell
   Unblock-File .\GIFA.exe
   ```

3. **校验哈希**（这是我们能给出的最强保证）：

   ```powershell
   Get-FileHash .\GIFA.exe -Algorithm SHA256
   # 与发布页的 SHA256SUMS.txt 对比
   ```

   打包脚本已经把每个文件的哈希写进 `dist/SHA256SUMS.txt`。

### 4.2 发布侧

* 每个 release 都附 `SHA256SUMS.txt`（`scripts/build.ps1` 自动生成）
* 每次重新构建后，可以把文件提交给微软做人工分析，确认干净后会**认这个哈希**：

  <https://www.microsoft.com/en-us/wdsi/filesubmission>
  选 "Software developer" → 上传 → 等结果。

  局限：**按哈希**生效，下一版要重新提交；且发布者仍显示"未知"。

### 4.3 代码里已经做过的（无关但值得记）

* 清单 `requireAdministrator` + `PerMonitorV2` + `longPathAware`
* 完整版本资源（`FileDescription` / `ProductName` / `LegalCopyright`）
* 图标、GPL-3.0 许可证与 NOTICE 随包分发

这些让文件看起来更正规，**但都不能替代签名**。

---

## 5. 建议路线

| 场景 | 建议 |
|---|---|
| 自己用 / 小圈子分发 | 不做签名，README 写清「更多信息 → 仍要运行」+ 提供哈希 |
| 开源项目想免费解决 | 申请 **SignPath Foundation**（GPL 符合） |
| 有预算、想显示自己名字 | **Azure Trusted Signing**（¥70/月）或 OV 证书 |
| 商业分发、要求零弹窗 | **EV 证书**（要企业资质） |

签名接入点在 `scripts/build.ps1`：在复制出 `dist\GIFA.exe` 之后、算哈希之前加一步

```powershell
& signtool.exe sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 /a (Join-Path $dist "GIFA.exe")
```

**签名必须在算 SHA256 之前**，否则哈希对不上。
