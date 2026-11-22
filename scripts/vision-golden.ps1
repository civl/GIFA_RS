# 视觉黄金对拍：生成合成语料 → 跑原版 JS → Rust 逐帧比对
#
#   powershell -ExecutionPolicy Bypass -File scripts/vision-golden.ps1
#
# 需要 node（用来跑 reference/GIFA 里的原版算法）。

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$ref = Join-Path $root "tools\vision-ref"
$frames = Join-Path $ref "frames"

Write-Host "== 1/3 生成合成帧 ==" -ForegroundColor Cyan
node (Join-Path $ref "harness.js") gen $frames
if ($LASTEXITCODE -ne 0) { throw "生成合成帧失败" }

Write-Host "== 2/3 跑原版 JS 参考实现 ==" -ForegroundColor Cyan
node (Join-Path $ref "harness.js") batch $frames
if ($LASTEXITCODE -ne 0) { throw "参考实现失败" }

Write-Host "== 3/3 Rust 对拍 ==" -ForegroundColor Cyan
Push-Location $root
try {
    $ErrorActionPreference = 'Continue'
    $out = cargo test -p gifa-vision --test golden -- --nocapture 2>&1 | Out-String
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    Write-Output $out
    if ($code -ne 0) { throw "对拍失败" }
} finally { Pop-Location }

Write-Host "对拍通过" -ForegroundColor Green
