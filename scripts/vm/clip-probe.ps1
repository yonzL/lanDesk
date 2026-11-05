$ErrorActionPreference = 'SilentlyContinue'
$out = 'C:\lanDesk\clip-report.txt'
$sid = (Get-Process -Id $PID).SessionId
$text = Get-Clipboard -Raw
$files = Get-Clipboard -Format FileDropList
$lines = @()
$lines += "session=$sid"
$lines += "time=" + (Get-Date -Format o)
$lines += "text=" + ($text -replace "`r?`n", '\n')
$lines += "files=" + (($files | ForEach-Object { $_.FullName }) -join '|')
Set-Content -Path $out -Value $lines -Encoding UTF8
