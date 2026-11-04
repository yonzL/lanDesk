#!/usr/bin/env bash
# 打一个可以拷给别人试用的包：两个 exe + 一页说明。
#
#   bash scripts/dist.sh          # 先 cargo build --release
# 产物：dist/landesk-<日期>/
set -eu

cd "$(dirname "$0")/.."

SRC=target/release
if [ ! -f "$SRC/rustdesk.exe" ] || [ ! -f "$SRC/landesk-gui.exe" ]; then
    echo "先构建：cargo build --release -p rustdesk --bin rustdesk -p landesk-gui"
    exit 1
fi

STAMP="$(date +%Y%m%d)"
OUT="dist/landesk-$STAMP"
rm -rf "$OUT"
mkdir -p "$OUT"

cp "$SRC/rustdesk.exe" "$OUT/"
cp "$SRC/landesk-gui.exe" "$OUT/"

cat > "$OUT/使用说明.txt" <<'EOF'
lanDesk 试用包
==============

两个程序：
  rustdesk.exe    被控机跑它（Service），控制机上也能用它做命令行操作
  landesk-gui.exe 控制机的图形界面

一、被控机（要被人远程看到的那台）
  1. 在控制机上先跑一次，生成密钥对并记下打印出的 public key：
         rustdesk.exe --landesk-genkey master.key
  2. 把 public key 拷到被控机，写成配置：
         rustdesk.exe --landesk-save-config --landesk-listen 21118 --landesk-auth-key <public key>
     这会在 rustdesk.exe 同目录生成 landesk.json。
  3. 直接用（本次开机有效）：
         rustdesk.exe --server
     或装成开机自启的服务（需要管理员）：
         rustdesk.exe --install-service
  4. 放行端口：管理员命令提示符里
         netsh advfirewall firewall add rule name="lanDesk" dir=in action=allow protocol=TCP localport=21118

二、控制机
  1. 把 master.key 放在 landesk-gui.exe 同目录。
  2. 准备一份清单 hosts.csv：
         组,子组,显示名,地址
         办公室,开发组,工位-01,10.0.0.5:21118
     导入：
         rustdesk.exe --landesk-book-import hosts.csv
  3. 打开 landesk-gui.exe，左边选机器，点“连接（申请操作权）”。
  4. 首次连接会把对方主机指纹记下来（未验证）。核对无误后点“核过指纹，标为已验证”。
     之后指纹一变就连不上——那说明机器换了密钥，需要你确认。

被拒时看什么
  Service 日志里恰好一行 landesk: reject <原因> from <地址>：
    no-bound-key              被控机没有给 --landesk-auth-key
    bad-signature             私钥与对方绑定的公钥不配对
    host-fingerprint-mismatch 对方主机密钥变了
    no-proof-yet              握手没走完（超时或对端不是 landesk）

自查
  bash scripts/landesk-smoke.sh        （在源码仓库里；本包不含脚本）
EOF

if command -v zip > /dev/null 2>&1; then
    (cd dist && zip -qr "landesk-$STAMP.zip" "landesk-$STAMP")
    echo "产物：dist/landesk-$STAMP.zip"
else
    if powershell -NoProfile -Command "Compress-Archive -Path '$OUT' -DestinationPath 'dist/landesk-$STAMP.zip' -Force"; then
        echo "产物：dist/landesk-$STAMP.zip"
    else
        echo "产物：$OUT/（未压缩）"
    fi
fi
ls -la "$OUT"
