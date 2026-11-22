$out = 'G:\Code\GenshinImpact\GIFA\gifa-rs\apps\gifa-demo\long-res.csv'
"t,memMB,cpuSec,threads,handles" | Out-File -FilePath $out -Encoding utf8
for ($i = 0; $i -lt 16; $i++) {
  $p = Get-Process gifa-demo -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $p) { break }
  $t = [math]::Round((Get-Date - $p.StartTime).TotalSeconds)
  "$t,$([math]::Round($p.WorkingSet64/1MB)),$([math]::Round($p.TotalProcessorTime.TotalSeconds,1)),$($p.Threads.Count),$($p.HandleCount)" | Out-File -FilePath $out -Append -Encoding utf8
  Start-Sleep -Seconds 20
}
