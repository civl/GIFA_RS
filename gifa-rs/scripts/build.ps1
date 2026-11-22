# GIFA 打包脚本：release 构建 → dist 目录 →（可选签名）→ zip → 安装器 → SHA256
# 用法： powershell -ExecutionPolicy Bypass -File scripts/build.ps1
#
# 顺序很重要：
#   1) 签名必须在 **打包 zip 之前**，否则 zip 里装的是没签名的 exe
#   2) 校验和必须在 **签名之后**，否则哈希对不上
#   签名怎么配见 docs/SIGNING.md；不配就跳过，不影响打包。
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot          # gifa-rs
$repo = Split-Path -Parent $root                  # 仓库根（reference/ 在这里）
$ver  = "0.2.6"
$name = "GIFA-$ver-win-x64"
$dist = Join-Path $root "dist\$name"

function Find-SignTool {
    $c = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $kits = "C:\Program Files (x86)\Windows Kits\10\bin"
    if (Test-Path $kits) {
        $hit = Get-ChildItem $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
               Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
               Sort-Object FullName -Descending | Select-Object -First 1
        if ($hit) { return $hit.FullName }
    }
    return $null
}

# 给一个文件签名；没配证书就什么都不做
function Invoke-Sign([string]$file, $signTool) {
    if (-not $signTool) { return }
    if ($env:GIFA_SIGN_PFX) {
        & $signTool sign /fd SHA256 /td SHA256 /tr http://timestamp.digicert.com `
            /f $env:GIFA_SIGN_PFX /p $env:GIFA_SIGN_PFX_PASS $file | Out-Null
    } else {
        & $signTool sign /fd SHA256 /td SHA256 /tr http://timestamp.digicert.com `
            /sha1 $env:GIFA_SIGN_CERT $file | Out-Null
    }
    if ($LASTEXITCODE -ne 0) { throw "签名失败：$file" }
    Write-Host ("  已签名 " + (Split-Path -Leaf $file)) -ForegroundColor Green
}

Write-Host "== 1/6 编译 release ==" -ForegroundColor Cyan
Push-Location $root
try {
    cargo build --release -p gifa-demo
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }
} finally { Pop-Location }

Write-Host "== 2/6 组装目录 ==" -ForegroundColor Cyan
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force $dist | Out-Null
Copy-Item (Join-Path $root "target\release\gifa-demo.exe") (Join-Path $dist "GIFA.exe")
New-Item -ItemType Directory -Force (Join-Path $dist "data") | Out-Null

$lic = Join-Path $dist "licenses"
New-Item -ItemType Directory -Force $lic | Out-Null
Copy-Item (Join-Path $repo "reference\GIFA\LICENSE")              (Join-Path $lic "LICENSE-GIFA.txt")
Copy-Item (Join-Path $repo "reference\GIFA\NOTICE")               (Join-Path $lic "NOTICE-GIFA.txt")
Copy-Item (Join-Path $repo "reference\GIFA\LICENSES\LGPL-3.0.txt") (Join-Path $lic "LICENSE-LGPL-3.0.txt")

Copy-Item (Join-Path $PSScriptRoot "README-dist.txt") (Join-Path $dist "README.txt")

$exe = Join-Path $dist "GIFA.exe"
$setup = Join-Path $root "dist\GIFA-$ver-setup.exe"
$zip = Join-Path $root "dist\$name.zip"
$sums = Join-Path $root "dist\SHA256SUMS.txt"

Write-Host "== 3/6 代码签名（可选，先签 exe 再打包） ==" -ForegroundColor Cyan
$signTool = $null
if ($env:GIFA_SIGN_CERT -or $env:GIFA_SIGN_PFX) {
    $signTool = Find-SignTool
    if (-not $signTool) {
        Write-Host "  配了证书但找不到 signtool.exe（装 Windows SDK 或用 VS 的开发者命令行）" -ForegroundColor Yellow
    }
}
if ($signTool) {
    Invoke-Sign $exe $signTool
} else {
    Write-Host "  跳过（未配置 GIFA_SIGN_CERT / GIFA_SIGN_PFX）" -ForegroundColor DarkGray
}

Write-Host "== 4/6 打包 zip ==" -ForegroundColor Cyan
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $dist -DestinationPath $zip -CompressionLevel Optimal

Write-Host "== 5/6 安装器（有 NSIS 才做） ==" -ForegroundColor Cyan
$makensis = $null
foreach ($c in @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe")) {
    if (Test-Path $c) { $makensis = $c; break }
}
if (-not $makensis) { $cmd = Get-Command makensis -ErrorAction SilentlyContinue; if ($cmd) { $makensis = $cmd.Source } }
if ($makensis) {
    & $makensis (Join-Path $root "installer\gifa.nsi") 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  安装器编译失败（继续）" -ForegroundColor Yellow
    } elseif (Test-Path $setup) {
        # 安装器也要签，否则用户装的时候照样弹 SmartScreen
        Invoke-Sign $setup $signTool
    }
} else {
    Write-Host "  未找到 makensis，跳过安装器" -ForegroundColor Yellow
}

Write-Host "== 6/6 校验和（必须在签名之后） ==" -ForegroundColor Cyan
$lines = Get-ChildItem $dist -Recurse -File | Where-Object { $_.Name -ne "SHA256SUMS.txt" } | ForEach-Object {
    $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    $rel = $_.FullName.Substring($dist.Length + 1).Replace('\', '/')
    "$h  $name/$rel"
}
foreach ($f in @($zip, $setup)) {
    if (Test-Path $f) {
        $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
        $rel = Split-Path -Leaf $f
        $lines += "$h  $rel"
    }
}
$lines | Set-Content $sums -Encoding ASCII

Write-Host ""
Write-Host "产物：" -ForegroundColor Green
Write-Host ("  {0}  ({1:N1} MB)" -f $exe, ((Get-Item $exe).Length / 1MB))
Write-Host ("  {0}  ({1:N1} MB)" -f $zip, ((Get-Item $zip).Length / 1MB))
Write-Host ("  {0}" -f $sums)
if (Test-Path $setup) {
    Write-Host ("  {0}  ({1:N1} MB)" -f $setup, ((Get-Item $setup).Length / 1MB))
}
if (-not $signTool) {
    Write-Host ""
    Write-Host "提示：产物未签名，首次运行会弹 SmartScreen（见 docs/SIGNING.md）" -ForegroundColor Yellow
}
