$ErrorActionPreference = 'SilentlyContinue'
Write-Output "=== 进程（含 session） ==="
Get-Process landesk-server | Select-Object Id, SessionId, StartTime | Format-Table -AutoSize | Out-String
Write-Output "=== 交互式会话 ==="
query session | Out-String
$dir = "C:\Windows\System32\config\systemprofile\AppData\Roaming\lanDesk\log\server"
Write-Output "=== SYSTEM 日志目录: $dir ==="
Get-ChildItem "$dir\*.log" | Select-Object Name, Length, LastWriteTime | Format-Table -AutoSize | Out-String
$latest = Get-ChildItem "$dir\*.log" | Sort-Object LastWriteTime | Select-Object -Last 1
if ($latest) {
  Write-Output ("=== " + $latest.Name + " ===")
  Get-Content $latest.FullName -Tail 20 | Out-String
}
