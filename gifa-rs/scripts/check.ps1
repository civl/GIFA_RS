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

function Run-DemoTest($name, $argLine, $timeoutSec = 60, $exePath = $null, $workDir = $null) {
    if (-not $exePath) { $exePath = $exe }
    if (-not $workDir) { $workDir = $demo }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exePath
    $psi.Arguments = $argLine
    $psi.WorkingDirectory = $workDir
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $p = [System.Diagnostics.Process]::Start($psi)
    if (-not $p.WaitForExit($timeoutSec * 1000)) {
        try { $p.Kill() } catch {}
        Add-Row $name $false "超时 $timeoutSec s"
        return
    }
    $out = $p.StandardOutput.ReadToEnd() + $p.StandardError.ReadToEnd()
    $m = [regex]::Match($out, "RESULT\s+(\d+)/(\d+)")
    if (-not $m.Success) {
        Add-Row $name $false "没有 RESULT 行"
        return
    }
    $got = [int]$m.Groups[1].Value
    $all = [int]$m.Groups[2].Value
    Add-Row $name ($got -eq $all) "$got/$all"
}

Write-Host "== 1/4 编译 ==" -ForegroundColor Cyan
Push-Location $root
try {
    $r = Invoke-Native "cargo" @("build", "--workspace")
    if ($r.Code -ne 0) { throw "cargo build 失败：$($r.Out)" }
} finally { Pop-Location }
Add-Row "cargo build --workspace" $true "ok"

Write-Host "== 2/4 单元测试 + clippy ==" -ForegroundColor Cyan
Push-Location $root
try {
    $out = (Invoke-Native "cargo" @("test", "--workspace")).Out
    $p = 0; $f = 0
    foreach ($m in [regex]::Matches($out, "test result: ok\.\s+(\d+) passed; (\d+) failed")) {
        $p += [int]$m.Groups[1].Value
        $f += [int]$m.Groups[2].Value
    }
    Add-Row "cargo test" ($f -eq 0 -and $p -gt 0) "passed=$p failed=$f"

    $clippy = (Invoke-Native "cargo" @("clippy", "--workspace", "--all-targets")).Out
    $warn = ([regex]::Matches($clippy, "(?m)^(warning|error)")).Count
    Add-Row "cargo clippy" ($warn -eq 0) "warnings=$warn"
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
    Add-Row "识别冒烟（12s）" $ok $(if ($m.Success) { "$($m.Groups[1].Value)/$($m.Groups[2].Value)" } else { "无 RESULT" })

    $boot = [regex]::Match($out, "窗口就绪\s+(\d+)ms")
    $bootMs = if ($boot.Success) { [int]$boot.Groups[1].Value } else { 99999 }
    Add-Row "冷启动 <2500ms" ($bootMs -lt 2500) "${bootMs}ms"
    Add-Row "内存 <200MB" ($mem -lt 200) "${mem}MB"
    Add-Row "CPU <10%（播放+识别）" ($cpu -lt 10) "${cpu}%"
}

Write-Host ""
$fail = ($script:rows | Where-Object { $_.结果 -eq "FAIL" }).Count
$total = $script:rows.Count
Write-Host "================ 汇总 ================" -ForegroundColor Cyan
$script:rows | Format-Table -AutoSize
if ($fail -eq 0) {
    Write-Host "全部通过（$total 项）" -ForegroundColor Green
    exit 0
} else {
    Write-Host "失败 $fail / $total 项" -ForegroundColor Red
    exit 1
}
