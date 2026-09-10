$cmd = 'cmd /c "C:\laragon\www\Bea\.smoke\launch-bea.cmd"'
$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
Write-Output ("launch wrapper ReturnValue=" + $r.ReturnValue + " Pid=" + $r.ProcessId)
Start-Sleep -Seconds 10
$bea = Get-Process bea -ErrorAction SilentlyContinue
if ($bea) { Write-Output ("bea.exe running Pid=" + ($bea.Id -join ',')) } else { Write-Output "bea.exe NOT running" }
$wv = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -match 'bea' }
Write-Output ("webview2 procs for bea: " + @($wv).Count)
