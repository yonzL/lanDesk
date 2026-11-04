#!/usr/bin/env bash
# landesk 冒烟：在本机起一个 Service，然后用同一个二进制当 Master，把四条路径跑一遍。
#
#   1) 持钥首次连接   → 成功，指纹记为未验证
#   2) 持钥第二次     → 成功
#   3) 换一把私钥     → 被拒，Service 留一行 reject 日志
#   4) 指纹被改       → 被拒（host-fingerprint-mismatch）
#
# 另外断言：Service 没有启动上游 rendezvous（"无出站"）。
#
# 用法：先 cargo build -p rustdesk --bin rustdesk，然后
#   bash scripts/landesk-smoke.sh            # 默认端口 21118
#   PORT=31118 bash scripts/landesk-smoke.sh # 换端口（本机已有 Service 时）

set -u

BIN="${BIN:-./target/debug/rustdesk.exe}"
PORT="${PORT:-21118}"
ADDR="127.0.0.1:${PORT}"
TMP="$(mktemp -d)"
SVC_PID=""

cleanup() {
    [ -n "$SVC_PID" ] && kill "$SVC_PID" 2>/dev/null
    rm -rf "$TMP"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*"
    echo "--- service log ---"
    cat "$TMP/service.log" 2>/dev/null | tail -20
    exit 1
}

[ -x "$BIN" ] || { echo "先构建：cargo build -p rustdesk --bin rustdesk"; exit 1; }

echo "1/6 生成两把密钥对"
"$BIN" --landesk-genkey "$TMP/master.key" > "$TMP/genkey.txt" 2>&1 || fail genkey
PUB="$(grep -o 'public key: .*' "$TMP/genkey.txt" | awk '{print $3}')"
[ -n "$PUB" ] || fail "没有拿到公钥"
"$BIN" --landesk-genkey "$TMP/wrong.key" > /dev/null 2>&1 || fail genkey-wrong

echo "2/6 起 Service（端口 $PORT，绑定公钥 ${PUB:0:16}…）"
"$BIN" --server --landesk-listen "$PORT" --landesk-auth-key "$PUB" > "$TMP/service.log" 2>&1 &
SVC_PID=$!
sleep 6
grep -q "landesk: service listening on .*:$PORT" "$TMP/service.log" || fail "Service 没有监听"
grep -q "skipping the rendezvous mediator" "$TMP/service.log" \
    || fail "Service 启动了上游 rendezvous（违反无出站）"

echo "3/6 导入地址簿"
printf '组,子组,显示名,地址\n测试,本机,smoke,%s\n' "$ADDR" > "$TMP/hosts.csv"
"$BIN" --landesk-book-import "$TMP/hosts.csv" --landesk-book "$TMP/book.json" > /dev/null \
    || fail "CSV 导入失败"

echo "4/6 持钥连接两次"
"$BIN" --landesk-connect "$ADDR" --landesk-key "$TMP/master.key" --landesk-book "$TMP/book.json" \
    || fail "首次连接失败"
grep -q '"verified": false' "$TMP/book.json" || fail "首次连接没有把指纹记为未验证"
"$BIN" --landesk-connect "$ADDR" --landesk-key "$TMP/master.key" --landesk-book "$TMP/book.json" \
    || fail "第二次连接失败"

echo "5/6 错私钥必须被拒"
if "$BIN" --landesk-connect "$ADDR" --landesk-key "$TMP/wrong.key" --landesk-book "$TMP/book.json" \
    > /dev/null 2>&1; then
    fail "错私钥竟然连上了"
fi
grep -q "landesk: reject bad-signature" "$TMP/service.log" || fail "Service 没有留下拒绝日志"

echo "6/6 标记已验证 + 指纹被改必须被拒"
"$BIN" --landesk-book-verify "$ADDR" --landesk-book "$TMP/book.json" > /dev/null || fail "标记已验证失败"
"$BIN" --landesk-connect "$ADDR" --landesk-key "$TMP/master.key" --landesk-book "$TMP/book.json" \
    || fail "已验证后再连失败"

cp "$TMP/book.json" "$TMP/book.bak"
sed -i 's/"host_pk": "[0-9a-f]*"/"host_pk": "ffffeeeeddddccccbbbbaaaa9999888877776666555544443333222211110000"/' \
    "$TMP/book.json"
if "$BIN" --landesk-connect "$ADDR" --landesk-key "$TMP/master.key" --landesk-book "$TMP/book.json" \
    > /dev/null 2>&1; then
    fail "指纹被改之后竟然还能连"
fi
cp "$TMP/book.bak" "$TMP/book.json"

echo "OK：四条路径全部符合预期"
