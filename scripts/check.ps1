﻿﻿# GIFA 发布前门禁：一条命令跑完单测 + 全部自检 + 性能预算
#
#   powershell -ExecutionPolicy Bypass -File scripts/check.ps1
#   ... -Fast        跳过较慢的 M3 网页自检
#   ... -SkipPerf    跳过内存/CPU/冷启动采样
#
# 退出码：0 = 全绿；1 = 有失败

param(
    [switch]$Fast,
    [switch]$SkipPerf
)

$ErrorActionPreference = "Stop"

# 原生程序往 stderr 写东西时，PowerShell 会当成异常（NativeCommandError）。
# 这几个 cargo 调用单独放宽，靠 $LASTEXITCODE 判定成败。
function Invoke-Native($file, $argList) {
    $ErrorActionPreference = 'Continue'
    $out = & $file @argList 2>&1 | Out-String
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    return @{ Out = $out; Code = $code }
}
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "target\debug\gifa-demo.exe"
$shell = Join-Path $root "target\debug\shell-demo.exe"
$demo = Join-Path $root "apps\gifa-demo"

$script:rows = @()

function Add-Row($name, $ok, $detail) {
    $script:rows += [pscustomobject]@{ 项目 = $name; 结果 = $(if ($ok) { "PASS" } else { "FAIL" }); 明细 = $detail }
    $c = if ($ok) { "Green" } else { "Red" }
    Write-Host ("  {0,-32} {1}  {2}" -f $name, $(if ($ok) { "PASS" } else { "FAIL" }), $detail) -ForegroundColor $c
}

function New-Psi($exePath, $argLine, $workDir) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exePath
    $psi.Arguments = $argLine
    $psi.WorkingDirectory = $workDir
    if ($env:GIFA_TEST_DATA) {
        # 会改设置的自检指到临时目录，别动用户真实的 settings.json
        $psi.EnvironmentVariables["GIFA_DATA_DIR"] = $env:GIFA_TEST_DATA
    }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    return $psi
}

function Run-DemoTest($name, $argLine, $timeoutSec = 60, $exePath = $null, $workDir = $null) {
    if (-not $exePath) { $exePath = $exe }
    if (-not $workDir) { $workDir = $demo }
    $p = [System.Diagnostics.Process]::Start((New-Psi $exePath $argLine $workDir))
    # 必须先挂异步读取：输出超过管道缓冲（4KB）时，子进程会阻塞在写入，
    # 而父进程卡在 WaitForExit 不读 → 死锁（门禁里真踩过）
    $so = $p.StandardOutput.ReadToEndAsync()
    $se = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit($timeoutSec * 1000)) {
        try { $p.Kill() } catch {}
        Add-Row $name $false "超时 $timeoutSec s"
        return
    }
    $out = $so.Result + $se.Result
    $m = [regex]::Match($out, "RESULT\s+(\d+)/(\d+)")
    if (-not $m.Success) {
        Add-Row $name $false "没有 RESULT 行"
        return
    }
    $got = [int]$m.Groups[1].Value
    $all = [int]$m.Groups[2].Value
    if ($got -eq $all) {
        Add-Row $name $true "$got/$all"
        return
    }
    # 失败时把具体哪一行挂了带上，不然只知道 3/4，还得手动复跑
    $bad = ($out -split "`r?`n" | Where-Object { $_ -match '^\[[0-9]+\].*FAIL' } | Select-Object -First 2) -join ' ; '
    if ($bad) { Write-Host ("    " + $bad) -ForegroundColor Yellow }
    # 这些自检会抢窗口/注入输入，偶尔被前台窗口干扰：失败重试一次再定性
    Write-Host ("  {0,-32} 第一次 {1}/{2}，重试…" -f $name, $got, $all) -ForegroundColor Yellow
    $p2 = [System.Diagnostics.Process]::Start((New-Psi $exePath $argLine $workDir))
    $so2 = $p2.StandardOutput.ReadToEndAsync()
    $se2 = $p2.StandardError.ReadToEndAsync()
    if (-not $p2.WaitForExit($timeoutSec * 1000)) { try { $p2.Kill() } catch {} }
    $out2 = $so2.Result + $se2.Result
    $m2 = [regex]::Match($out2, "RESULT\s+(\d+)/(\d+)")
    if ($m2.Success) {
        $g2 = [int]$m2.Groups[1].Value
        $a2 = [int]$m2.Groups[2].Value
        Add-Row $name ($g2 -eq $a2) "$g2/$a2（重试）"
    } else {
        Add-Row $name $false "重试也没结果"
    }
}

Write-Host "== 1/4 编译 ==" -ForegroundColor Cyan
Push-Location $root
try {
    $r = Invoke-Native "cargo" @("build", "--workspace")
    if ($r.Code -ne 0) { throw "cargo build 失败：$($r.Out)" }
} finally { Pop-Location }
Add-Row "cargo build --workspace" $true "ok"

Write-Host "== 2/4 视觉对拍素材 + 单元测试 + clippy ==" -ForegroundColor Cyan
# 有 node 就现场生成对拍语料（调用原版 JS），这样 cargo test 里的 golden 测试不是空跑
$node = Get-Command node -ErrorAction SilentlyContinue
if ($node) {
    $ref = Join-Path $root "tools\vision-ref\harness.js"
    $frames = Join-Path $root "tools\vision-ref\frames"
    $ErrorActionPreference = 'Continue'
    & node $ref gen $frames 2>&1 | Out-Null
    $g1 = $LASTEXITCODE
    & node $ref batch $frames 2>&1 | Out-Null
    $g2 = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    Add-Row "生成视觉对拍语料" ($g1 -eq 0 -and $g2 -eq 0) "frames + expected.json"
} else {
    Add-Row "生成视觉对拍语料" $true "跳过（没有 node）"
}
Push-Location $root
try {
    $out = (Invoke-Native "cargo" @("test", "--workspace")).Out
    $p = 0; $f = 0
    foreach ($m in [regex]::Matches($out, "test result: ok\.\s+(\d+) passed; (\d+) failed")) {
        $p += [int]$m.Groups[1].Value
        $f += [int]$m.Groups[2].Value
    }
    Add-Row "cargo test" ($f -eq 0 -and $p -gt 0) "passed=$p failed=$f"

    # -D warnings：门禁里任何 warning 都算失败（本仓库的约定是零警告）
    $clippy = (Invoke-Native "cargo" @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings")).Out
    $warn = ([regex]::Matches($clippy, "(?m)^(warning|error)")).Count
    Add-Row "cargo clippy -D warnings" ($warn -eq 0) "warnings=$warn"

    # 代码风格：fmt 一次对齐过，之后门禁卡住，避免格式漂移混进 diff
    $fmt = (Invoke-Native "cargo" @("fmt", "--all", "--", "--check")).Out
    $diff = ([regex]::Matches($fmt, "(?m)^Diff in")).Count
    Add-Row "cargo fmt --check" ($diff -eq 0) "未格式化文件块=$diff"
} finally { Pop-Location }

Write-Host "== 3/4 自检 ==" -ForegroundColor Cyan
$shellDir = Join-Path $root "apps\shell-demo"
Run-DemoTest "外壳（区域/拖角/命中）" "" 60 $shell $shellDir
Run-DemoTest "热键（注册/钩子/交换）" "--hotkey-test" 60 $shell $shellDir
Run-DemoTest "设置与迁移" "--settings-test"
Run-DemoTest "视觉冒烟（合成地图）" "--vision-synth"
Run-DemoTest "浮窗界面" "--open BV1xx411c7mD --ui-test"
Run-DemoTest "布局标定" "--open BV1xx411c7mD --calib-test"
Run-DemoTest "方位叠加层" "--open BV1xx411c7mD --overlay-test"
Run-DemoTest "手柄八方向圆盘" "--open BV1xx411c7mD --gamepad-test"
Run-DemoTest "沉浸与鼠标避让" "--open BV1xx411c7mD --immersion-test"
if (-not $Fast) {
    Run-DemoTest "B 站加载与播放（M3）" "--open BV1xx411c7mD --check" 90
}
# 会改设置的三条：用临时数据目录跑，不污染真实配置
$env:GIFA_TEST_DATA = Join-Path $root "target\gate-data"
Run-DemoTest "热键录制换绑" "--bind-test" 60
Run-DemoTest "历史/收藏" "--fav-test" 60
Run-DemoTest "按分 P 载入" "--load-test" 60
$env:GIFA_TEST_DATA = $null

Write-Host "== 4/4 识别冒烟 + 性能 ==" -ForegroundColor Cyan
if ($SkipPerf) {
    Add-Row "性能预算" $true "跳过（-SkipPerf）"
} else {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = "--open BV1xx411c7mD --bearing 12"
    $psi.WorkingDirectory = $demo
    $psi.RedirectStandardOutput = $true
    $psi.UseShellExecute = $false
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $p = [System.Diagnostics.Process]::Start($psi)

    Start-Sleep -Seconds 7
    $proc = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
    $mem = 0; $cpu = 0
    if ($proc) {
        $mem = [math]::Round($proc.WorkingSet64 / 1MB, 0)
        $t0 = $proc.TotalProcessorTime
        Start-Sleep -Seconds 3
        $proc.Refresh()
        $dt = ($proc.TotalProcessorTime - $t0).TotalMilliseconds
        $cpu = [math]::Round(100 * $dt / (3 * 1000 * [Environment]::ProcessorCount), 1)
    }
    if (-not $p.WaitForExit(40000)) { try { $p.Kill() } catch {} }
    $out = $p.StandardOutput.ReadToEnd()

    $m = [regex]::Match($out, "RESULT\s+(\d+)/(\d+)")
    $ok = $m.Success -and ($m.Groups[1].Value -eq $m.Groups[2].Value)
    $detail = if ($m.Success) { "$($m.Groups[1].Value)/$($m.Groups[2].Value)" } else { "无 RESULT" }
    if (-not $ok) {
        # 把挂的那一行打出来，省得只知道 3/4 还要手动复跑
        ($out -split "`r?`n" | Where-Object { $_ -match '^\[[0-9]+\].*FAIL' } | Select-Object -First 2) |
            ForEach-Object { Write-Host ("    " + $_) -ForegroundColor Yellow; $detail += " | " + $_.Trim() }
    }
    Add-Row "识别冒烟（12s）" $ok $detail

    $boot = [regex]::Match($out, "窗口就绪\s+(\d+)ms")
    $bootMs = if ($boot.Success) { [int]$boot.Groups[1].Value } else { 99999 }
    Add-Row "冷启动 <2500ms" ($bootMs -lt 2500) "${bootMs}ms"
    Add-Row "内存 <200MB" ($mem -lt 200) "${mem}MB"
    Add-Row "CPU <10%（播放+识别）" ($cpu -lt 10) "${cpu}%"
}

Write-Host ""
# 注意：PowerShell 5.1 里空管道的 .Count 不是 0，必须用 @() 包起来
$fail = @($script:rows | Where-Object { $_.结果 -eq "FAIL" }).Count
$total = @($script:rows).Count
Write-Host "================ 汇总 ================" -ForegroundColor Cyan
$script:rows | Format-Table -AutoSize
if ($fail -eq 0) {
    Write-Host "全部通过（$total 项）" -ForegroundColor Green
    exit 0
} else {
    Write-Host "失败 $fail / $total 项" -ForegroundColor Red
    exit 1
}
