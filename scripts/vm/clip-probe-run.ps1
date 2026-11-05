$ErrorActionPreference = 'Stop'
$user = "$env:COMPUTERNAME\$env:USERNAME"
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
  -Argument '-NoProfile -ExecutionPolicy Bypass -File C:\lanDesk\clip-probe.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName 'landesk-clip-probe' -Action $action -Principal $principal -Force | Out-Null
Start-ScheduledTask -TaskName 'landesk-clip-probe'
Start-Sleep -Seconds 3
if (Test-Path 'C:\lanDesk\clip-report.txt') { Get-Content 'C:\lanDesk\clip-report.txt' } else { 'no report' }
