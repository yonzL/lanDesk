$ErrorActionPreference = 'Stop'
$probe = Join-Path $PSScriptRoot 'clip-probe.ps1'
$report = Join-Path $PSScriptRoot 'clip-report.txt'
Remove-Item $report -Force -ErrorAction SilentlyContinue
$user = "$env:COMPUTERNAME\$env:USERNAME"
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
  -Argument ('-NoProfile -ExecutionPolicy Bypass -File "' + $probe + '"')
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName 'landesk-clip-probe' -Action $action -Principal $principal -Force | Out-Null
Start-ScheduledTask -TaskName 'landesk-clip-probe'
Start-Sleep -Seconds 4
if (Test-Path $report) { Get-Content $report } else { 'no report at ' + $report }
