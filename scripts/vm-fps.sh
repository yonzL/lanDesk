#!/usr/bin/env bash
# 取被控端（Windows VM）画面链路的证据：编码器、线程数、帧率调整、编码失败。
#
# 用法： bash scripts/vm-fps.sh dev@192.168.99.191 [尾部行数]
#
# 读的是 SYSTEM 服务进程的日志目录：Config::get_home() 会把 system32\config\systemprofile
# 改写成 ServiceProfiles\LocalService，所以路径不是 %APPDATA%。
set -euo pipefail

TARGET="${1:?用法: bash scripts/vm-fps.sh user@host [lines]}"
LINES="${2:-200}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/landesk_vm}"

ps1=$(mktemp /tmp/vm-fps-XXXX.ps1)
trap 'rm -f "$ps1"' EXIT

cat >"$ps1" <<'EOF'
$dir = 'C:\Windows\ServiceProfiles\LocalService\AppData\Roaming\lanDesk\log'
$log = Get-ChildItem $dir -Recurse -Filter *.log -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime | Select-Object -Last 1
if (-not $log) { Write-Output 'no server log found'; exit 1 }
Write-Output ("log: " + $log.FullName)
Get-Content $log.FullName -Tail __LINES__ |
  Select-String -Pattern 'new encoder|cpu num|codec thread|Set fps|reduce delay|encode fail|switch to change quality|Option update'
EOF

sed -i "s/__LINES__/${LINES}/" "$ps1"

remote_ps1='C:/ld/vm-fps.ps1'
scp -i "$SSH_KEY" -o StrictHostKeyChecking=no -o BatchMode=yes "$ps1" "$TARGET:$remote_ps1" >/dev/null
ssh -i "$SSH_KEY" -o StrictHostKeyChecking=no -o BatchMode=yes "$TARGET" \
  "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\ld\\vm-fps.ps1" 2>&1 | tr -d '\r'
