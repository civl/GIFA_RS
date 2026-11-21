Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class W2 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
}
"@
[W2]::SetProcessDPIAware() | Out-Null
$script:t = [IntPtr]::Zero
$cb = [W2+EnumProc]{ param($h,$l)
  $sb = New-Object System.Text.StringBuilder 256
  [W2]::GetWindowTextW($h,$sb,256) | Out-Null
  if ($sb.ToString() -like 'GIFA*') { $script:t = $h; return $false }
  return $true }
[W2]::EnumWindows($cb,[IntPtr]::Zero) | Out-Null
if ($script:t -eq [IntPtr]::Zero) { 'not found'; exit 1 }
$r = New-Object W2+RECT
[W2]::GetWindowRect($script:t,[ref]$r) | Out-Null
$w = $r.R-$r.L; $h = $r.B-$r.T
"rect $($r.L),$($r.T) ${w}x${h}"
$bmp = New-Object System.Drawing.Bitmap $w,$h
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L,$r.T,0,0,(New-Object System.Drawing.Size $w,$h))
$bmp.Save('G:\Code\GenshinImpact\GIFA\gifa-rs\apps\shell-demo\docs\shell-urlbar.png',[System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose(); 'saved'
