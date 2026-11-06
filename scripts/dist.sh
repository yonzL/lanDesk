#!/usr/bin/env bash
# 打一个可以拷给别人试用的包：两个 exe + 一页说明。
#
#   cargo build --release -p landesk-server -p landesk-client
#   bash scripts/dist.sh
# 产物：dist/lanDesk-<日期>/ 与 dist/lanDesk-<日期>.zip
set -eu

cd "$(dirname "$0")/.."

SRC=target/release
SERVER=landesk-server.exe
CLIENT=landesk-client.exe
if [ ! -f "$SRC/$SERVER" ] || [ ! -f "$SRC/$CLIENT" ]; then
    echo "先构建：cargo build --release --bin landesk-server -p landesk-client"
    exit 1
fi

STAMP="$(date +%Y%m%d)"
OUT="dist/lanDesk-$STAMP"
rm -rf "$OUT"
mkdir -p "$OUT"

cp "$SRC/$SERVER" "$OUT/"
cp "$SRC/$CLIENT" "$OUT/"

# Flutter 主界面（正式版）：整套 runner 产物放进 master/，并把 exe 改名成 landesk-master.exe。
# 里面还要放一份 landesk-client.exe——双击连接时它会拉起同目录的这个 eframe 壳。
FLUTTER_OUT=flutter/build/windows/x64/runner/Release
if [ -f "$FLUTTER_OUT/rustdesk.exe" ]; then
    mkdir -p "$OUT/master"
    cp -r "$FLUTTER_OUT/." "$OUT/master/"
    mv "$OUT/master/rustdesk.exe" "$OUT/master/landesk-master.exe"
    cp "$SRC/$CLIENT" "$OUT/master/"
else
    echo "提示：没有 $FLUTTER_OUT —— 先跑 flutter build windows --release，否则包里只有 eframe 界面"
fi

cat > "$OUT/使用说明.txt" <<'EOF'
lanDesk 试用包
==============

两个程序：
  landesk-server.exe  被控机跑它（Service），控制机上也能用它做命令行操作
  master\landesk-master.exe  控制机的正式界面（Flutter）
  landesk-client.exe  控制机的备用界面（eframe），也是 master 双击连接时拉起的那个

一、被控机（要被人远程看到的那台）
  1. 在控制机上先跑一次，生成密钥对并记下打印出的 public key：
         landesk-server.exe --landesk-genkey master.key
  2. 把 public key 拷到被控机，写成配置：
         landesk-server.exe --landesk-save-config --landesk-listen 21118 --landesk-auth-key <public key>
     这会在 landesk-server.exe 同目录生成 landesk.json。
  3. 直接用（本次开机有效）：
         landesk-server.exe --server
     或装成开机自启的服务（需要管理员）：
         landesk-server.exe --install-service
  4. 放行端口：管理员命令提示符里
         netsh advfirewall firewall add rule name="lanDesk" dir=in action=allow protocol=TCP localport=21118

二、控制机
  1. 把 master.key 放在 master\landesk-master.exe 同目录，并改名为 private.key。
  2. 准备一份清单 hosts.csv：
         组,子组,显示名,地址
         办公室,开发组,工位-01,10.0.0.5:21118
     导入：
         landesk-server.exe --landesk-book-import hosts.csv
     （地址簿文件 addressbook.json 要在 master\ 里，界面才读得到；或者直接在界面里“添加”。）
  3. 打开 master\landesk-master.exe：左侧是说明，右侧是地址簿树；双击机器即开一个会话窗口。
  4. 首次连接会把对方主机指纹记下来（未验证）。核对无误后在那一行的菜单里选“标记指纹为已验证”。
     之后指纹一变就连不上——那说明机器换了密钥，需要你确认。

三、批量分发（同一批文件推给多台）
  1. 图形界面：顶部切到“文件分发”，左侧勾选多台，填远端目录与文件，点开始。
  2. 命令行（适合脚本）：
         landesk-server.exe --landesk-push 10.0.0.5:21118,10.0.0.6:21118 \
           --landesk-to "C:\landesk-incoming" \
           --landesk-files "D:\build\agent.exe,D:\build\config.json" \
           --landesk-key master.key
     远端目录要已存在；每台一个独立的文件会话并行推，结果逐台打印。

被拒时看什么
  Service 日志里恰好一行 landesk: reject <原因> from <地址>：
    no-bound-key              被控机没有给 --landesk-auth-key
    bad-signature             私钥与对方绑定的公钥不配对
    host-fingerprint-mismatch 对方主机密钥变了
    no-proof-yet              握手没走完（超时或对端不是 landesk）

先自检：landesk-server.exe --landesk-doctor

已知未做
  “启动文件”未做（需要新协议动作）；剪贴板传文件已随会话启用，
  但跨机是否生效请在两台机器上亲自试一次（本机无法区分）。
EOF

if command -v zip > /dev/null 2>&1; then
    (cd dist && zip -qr "lanDesk-$STAMP.zip" "lanDesk-$STAMP")
    echo "产物：dist/lanDesk-$STAMP.zip"
else
    if powershell -NoProfile -Command "Compress-Archive -Path '$OUT' -DestinationPath 'dist/lanDesk-$STAMP.zip' -Force"; then
        echo "产物：dist/lanDesk-$STAMP.zip"
    else
        echo "产物：$OUT/（未压缩）"
    fi
fi
ls -la "$OUT"
