# GIFA 打包脚本：release 构建 → dist 目录 → SHA256 → zip
# 用法： powershell -ExecutionPolicy Bypass -File scripts/build.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot          # gifa-rs
$repo = Split-Path -Parent $root                  # 仓库根（reference/ 在这里）
$ver  = "0.2.6"
$name = "GIFA-$ver-win-x64"
$dist = Join-Path $root "dist\$name"

Write-Host "== 1/4 编译 release ==" -ForegroundColor Cyan
Push-Location $root
try {
    cargo build --release -p gifa-demo
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }
} finally { Pop-Location }

Write-Host "== 2/4 组装目录 ==" -ForegroundColor Cyan
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

Write-Host "== 3/4 校验和 ==" -ForegroundColor Cyan
$sums = Join-Path $root "dist\SHA256SUMS.txt"
$lines = Get-ChildItem $dist -Recurse -File | Where-Object { $_.Name -ne "SHA256SUMS.txt" } | ForEach-Object {
    $h = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    $rel = $_.FullName.Substring($dist.Length + 1).Replace('\', '/')
    "$h  $name/$rel"
}
$lines | Set-Content $sums -Encoding ASCII

Write-Host "== 4/4 打包 zip ==" -ForegroundColor Cyan
$zip = Join-Path $root "dist\$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $dist -DestinationPath $zip -CompressionLevel Optimal

Write-Host "== 5/5 安装器（有 NSIS 才做） ==" -ForegroundColor Cyan
$makensis = $null
foreach ($c in @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe")) {
    if (Test-Path $c) { $makensis = $c; break }
}
if (-not $makensis) { $cmd = Get-Command makensis -ErrorAction SilentlyContinue; if ($cmd) { $makensis = $cmd.Source } }
$setup = Join-Path $root "dist\GIFA-$ver-setup.exe"
if ($makensis) {
    & $makensis (Join-Path $root "installer\gifa.nsi") 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Host "  安装器编译失败（继续）" -ForegroundColor Yellow }
} else {
    Write-Host "  未找到 makensis，跳过安装器" -ForegroundColor Yellow
}

$exe = Join-Path $dist "GIFA.exe"
Write-Host ""
Write-Host "产物：" -ForegroundColor Green
Write-Host ("  {0}  ({1:N1} MB)" -f $exe, ((Get-Item $exe).Length / 1MB))
Write-Host ("  {0}  ({1:N1} MB)" -f $zip, ((Get-Item $zip).Length / 1MB))
Write-Host ("  {0}" -f $sums)
if (Test-Path $setup) {
    Write-Host ("  {0}  ({1:N1} MB)" -f $setup, ((Get-Item $setup).Length / 1MB))
}
