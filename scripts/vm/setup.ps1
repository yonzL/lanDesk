$ErrorActionPreference = 'Stop'
$src = 'C:\lanDesk'
$dst = 'C:\Program Files\lanDesk'
New-Item -ItemType Directory -Force -Path $dst | Out-Null
Copy-Item "$src\landesk-server.exe" $dst -Force
Copy-Item "$src\landesk.json" $dst -Force
Get-ChildItem $dst | Select-Object Name, Length | Format-Table -AutoSize
