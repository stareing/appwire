#!/usr/bin/env bash
# 按名寻址 Linux 全链路（spec/naming.md 4.1）：私有 D-Bus 会话总线 + `app-mcp-host app install` 写的激活文件 +
# `app-mcp-host stdio --name-service` 作为 Hub + C++ 示例 named_cpp（ClientConfig::register_name）。
#
# 发现不激活 → 调用触发 D-Bus 激活冷启动 → 调用 → 宽限后通道关闭、App（由激活启动）退出 → 再次调用再激活。
#
# 用法：naming_e2e.sh <named_cpp 可执行文件>
# 环境变量：CARGO_TARGET_DIR（默认 <repo>/target）、APP_MCP_HOST_BIN（直接指定 app-mcp-host，跳过构建）。
# 本机没有 dbus-daemon 或不是 Linux 时退出码 77（ctest 记为跳过）。
set -euo pipefail

APP_BIN="${1:?用法：$0 <named_cpp>}"
APP_BIN="$(cd "$(dirname "$APP_BIN")" && pwd)/$(basename "$APP_BIN")"
[[ "$(uname -s)" == Linux ]] || { echo "SKIP：仅 Linux（D-Bus）" >&2; exit 77; }
DBUS_DAEMON="$(command -v dbus-daemon || true)"
[[ -n "$DBUS_DAEMON" ]] || { echo "SKIP：本机没有 dbus-daemon" >&2; exit 77; }

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
if [[ -z "${APP_MCP_HOST_BIN:-}" ]]; then
  APP_MCP_HOST_BIN="$TARGET_DIR/debug/app-mcp-host"
  (cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build -q -p app-mcp-host)
fi

APP_ID="named-cpp-e2e"
TIMEOUT_S=20
WORK="$(mktemp -d)"
LOG="$WORK/events.log"
BUS_PID=""
HUB_PID=""
cleanup() {
  [[ -n "$HUB_PID" ]] && kill "$HUB_PID" 2>/dev/null || true
  [[ -n "$BUS_PID" ]] && kill "$BUS_PID" 2>/dev/null || true
  pkill -f "^$APP_BIN" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT
fail() {
  echo "FAIL：$*" >&2
  echo "--- events" >&2; cat "$LOG" 2>/dev/null >&2 || true
  echo "--- hub stderr" >&2; tail -n 60 "$WORK/hub.err" 2>/dev/null >&2 || true
  exit 1
}
count() { local n; n="$(grep -c "^$1 " "$LOG" 2>/dev/null)" || true; echo "${n:-0}"; }

# 1. 私有会话总线：激活目录取自临时 XDG_DATA_HOME（与 app install 写入的位置相同）；被激活的进程继承这里的环境。
mkdir -p "$WORK/data/dbus-1/services" "$WORK/sys" "$WORK/run" "$WORK/home"
cat >"$WORK/session.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$WORK/bus</listen>
  <auth>EXTERNAL</auth>
  <standard_session_servicedirs/>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
env -u DBUS_SESSION_BUS_ADDRESS -u DBUS_STARTER_ADDRESS -u DBUS_STARTER_BUS_TYPE \
  XDG_DATA_HOME="$WORK/data" XDG_DATA_DIRS="$WORK/sys" XDG_RUNTIME_DIR="$WORK/run" \
  APP_MCP_EVENT_LOG="$LOG" APP_MCP_APP_ID="$APP_ID" \
  "$DBUS_DAEMON" --config-file="$WORK/session.conf" --nofork --print-address=1 >"$WORK/bus.addr" 2>"$WORK/bus.err" &
BUS_PID=$!
for _ in $(seq 1 100); do [[ -s "$WORK/bus.addr" ]] && break; sleep 0.05; done
BUS_ADDR="$(head -n1 "$WORK/bus.addr")"
[[ -n "$BUS_ADDR" ]] || fail "dbus-daemon 未打印地址：$(cat "$WORK/bus.err")"
export DBUS_SESSION_BUS_ADDRESS="$BUS_ADDR"

# 2. 登记：激活文件 + App 登记文件 + 清单（Hub 未连接 App 也能列出工具）；默认调用 ReloadConfig（本总线）。
cat >"$WORK/manifest.json" <<EOF
{"manifestVersion": 1, "appId": "$APP_ID", "name": "按名寻址 e2e（C++）",
 "tools": [{"name": "echo", "description": "原样返回参数", "inputSchema": {"type": "object"}},
           {"name": "pid", "description": "返回进程号", "inputSchema": {"type": "object"}}]}
EOF
"$APP_MCP_HOST_BIN" app install --app-id "$APP_ID" --exec "$APP_BIN" --manifest "$WORK/manifest.json" \
  --home "$WORK/home" --data-home "$WORK/data" >"$WORK/install.out" 2>&1 || fail "app install：$(cat "$WORK/install.out")"
compgen -G "$WORK/data/dbus-1/services/dev.appmcp.App.*.service" >/dev/null || fail "未写激活文件：$(cat "$WORK/install.out")"

# 3. Hub：stdio MCP；不用唤醒器（证明激活只经名字服务），不占默认 IPC 端点与端口，宽限 400 ms、关闭租约。
coproc HUB { exec "$APP_MCP_HOST_BIN" stdio --home "$WORK/home" --ipc-endpoint none --listen 127.0.0.1:0 \
  --name-service --channel-grace-ms 400 --lease-ms 0 --waker none --tool-exposure all 2>"$WORK/hub.err"; }
HUB_PID=$HUB_PID
NEXT_ID=1
# rpc <method> <params-json>：发送请求并输出对应 id 的响应行（跳过通知）。
rpc() {
  local id=$NEXT_ID line
  NEXT_ID=$((NEXT_ID + 1))
  printf '{"jsonrpc":"2.0","id":%d,"method":"%s","params":%s}\n' "$id" "$1" "$2" >&"${HUB[1]}"
  while IFS= read -r -t "$TIMEOUT_S" line <&"${HUB[0]}"; do
    if [[ "$line" =~ \"id\":$id[,}] ]]; then printf '%s\n' "$line"; return 0; fi
  done
  fail "等待 $1 的响应超时"
}
call() { rpc tools/call "{\"name\":\"$1\",\"arguments\":$2}"; }

rpc initialize '{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"naming-e2e","version":"0"}}' >/dev/null
printf '{"jsonrpc":"2.0","method":"notifications/initialized"}\n' >&"${HUB[1]}"

# 4. 发现不激活：apps.list 中出现 nameService（来源 dbus、可激活、未运行），App 进程从未启动。
deadline=$((SECONDS + TIMEOUT_S))
while :; do
  listed="$(call apps.list '{}')"
  [[ "$listed" =~ nameService\\?\":\{ ]] && break
  (( SECONDS < deadline )) || fail "等待发现记录超时：$listed"
  sleep 0.1
done
[[ "$listed" =~ activatable\\?\":true ]] || fail "发现记录应可激活：$listed"
[[ "$listed" =~ running\\?\":false ]] || fail "发现记录应未运行：$listed"
sleep 0.2
[[ "$(count start)" == 0 ]] || fail "发现不得启动 App"

# 5. 调用触发激活冷启动；宽限内的第二次调用合并进同一通道。
r="$(call "$APP_ID.echo" '{"x":1}')"
[[ "$r" == *'"isError":true'* ]] && fail "第一次调用失败：$r"
[[ "$r" =~ echo\\?\":\{\\?\"x\\?\":1\} ]] || fail "echo 结果不正确：$r"
r="$(call "$APP_ID.pid" '{}')"
[[ "$r" =~ pid\\?\":[0-9]+ ]] || fail "pid 结果不正确：$r"
[[ "$(count start)" == 1 ]] || fail "应只激活一次（实际 $(count start)）"

# 6. 宽限后 Hub 关闭通道，由激活启动的 App 收到 on_idle_exit 退出。
deadline=$((SECONDS + TIMEOUT_S))
until [[ "$(count exit)" == 1 ]]; do
  (( SECONDS < deadline )) || fail "宽限后 App 未退出"
  sleep 0.1
done

# 7. 再次调用再激活（新进程）。
r="$(call "$APP_ID.echo" '{"x":2}')"
[[ "$r" =~ echo\\?\":\{\\?\"x\\?\":2\} ]] || fail "再激活后 echo 结果不正确：$r"
[[ "$(count start)" == 2 ]] || fail "应再激活一次（实际 $(count start)）"

eval "exec ${HUB[1]}>&-"
wait "$HUB_PID" 2>/dev/null || true
HUB_PID=""
echo "PASS（激活 $(count start) 次，退出 $(count exit) 次）" >&2
