# 从 assets/icon.svg 生成 assets/icon.ico（多尺寸、透明底）
#
# 依赖：Edge（headless 渲染）+ Python 的 Pillow 打包 ico。
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File scripts/make-icon.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$assets = Join-Path $root 'apps/gifa-demo/assets'
$png = Join-Path $assets 'png'
New-Item -ItemType Directory -Force -Path $png | Out-Null

# 1) 用 headless Edge 把 SVG 渲染成 512px PNG（HTML 包一层，才能铺满窗口且保留透明）
$html = Join-Path $assets 'shot.html'
@'
<!doctype html><meta charset="utf-8">
<style>html,body{margin:0;padding:0;background:transparent;overflow:hidden}
img{display:block;width:100vw;height:100vh}</style>
<img src="icon.svg">
'@ | Set-Content -Path $html -Encoding UTF8

$out = Join-Path $png 'icon-512.png'
$edge = 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
if (-not (Test-Path $edge)) { $edge = 'C:\Program Files\Microsoft\Edge\Application\msedge.exe' }
& $edge --headless --disable-gpu --hide-scrollbars --default-background-color=00000000 `
    --force-device-scale-factor=1 --screenshot=$out --window-size=512,512 `
    ("file:///" + $html.Replace('\','/')) | Out-Null

# 2) Pillow 打包成多尺寸 ico
python -c @"
from PIL import Image
im = Image.open(r'$out').convert('RGBA')
im.save(r'$(Join-Path $assets 'icon.ico')', sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
print('icon.ico 已生成')
"@
Remove-Item $html -Force
