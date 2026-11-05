#!/usr/bin/env bash
# 把发布包推到一台 Windows 机器（VM）上，并把它起成 Service。
#
#   bash scripts/vm-deploy.sh landesk@192.168.1.50 [21118]
#
# 它做的事：
#   1. 在远端建 C:\lanDesk，把 landesk-server.exe 拷过去
#   2. 用这台机器上已有的 master.key 推导出公钥，写成远端的 landesk.json
#   3. 起 --server（不带参数，让它读配置）
#   4. 回显远端的监听状态
#
# 需要：远端已开 OpenSSH Server，且当前用户能 ssh 过去（建议先用 ssh 手工验证一次）。
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${1:?用法: vm-deploy.sh user@host [port]}"
PORT="${2:-21118}"
KEY="${KEY:-master.key}"

DIST="$(ls -d dist/lanDesk-* 2>/dev/null | tail -1 || true)"
if [ -z "$DIST" ] || [ ! -f "$DIST/landesk-server.exe" ]; then
    echo "没找到 dist/lanDesk-*/landesk-server.exe，先 cargo build --release 后跑 bash scripts/dist.sh"
    exit 1
fi
if [ ! -f "$KEY" ]; then
    echo "没找到私钥 $KEY（在控制机上用 --landesk-genkey 生成，或设置 KEY=路径）"
    exit 1
fi

PUB="$("./$DIST/landesk-server.exe" --landesk-genkey "$KEY" 2>&1 | grep 'public key:' | awk '{print $3}')"
if [ -z "$PUB" ]; then
    echo "没能从 $KEY 得到公钥"
    exit 1
fi

echo "1/4 远端建目录"
ssh "$TARGET" 'powershell -NoProfile -Command "New-Item -ItemType Directory -Force -Path C:\lanDesk | Out-Null; Write-Output ok"'

echo "2/4 拷二进制"
scp "$DIST/landesk-server.exe" "$TARGET:C:/lanDesk/landesk-server.exe"

echo "3/4 写配置（端口 $PORT，公钥 ${PUB:0:16}…）"
ssh "$TARGET" "powershell -NoProfile -Command \"Set-Location C:\lanDesk; ./landesk-server.exe --landesk-save-config --landesk-listen $PORT --landesk-auth-key $PUB\""

echo "4/4 起 Service（前台，日志留在远端窗口/任务里）"
echo "   下一步在 VM 上跑：  C:\\lanDesk\\landesk-server.exe --server"
echo "   或者在 VM 的管理员 PowerShell 里：Start-Process C:\\lanDesk\\landesk-server.exe -ArgumentList '--server'"
echo "   然后从这台机器：    ./target/debug/landesk-server.exe --landesk-connect <VM-IP>:$PORT --landesk-key $KEY"
