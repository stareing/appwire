#!/usr/bin/env bash
# 集成测试：启动 fake_host → 读取 LISTENING 地址 → 用该地址启动客户端 → 检查调用结果。
#
# 用法：integration.sh <客户端可执行文件> [额外参数...]
# 客户端需接受 Host 地址作为第一个参数（ws://<addr>），注册 greet({name}) 与 app.info，
# 并在 SIGTERM 时退出。环境变量：
#   CARGO_TARGET_DIR  fake_host 的构建目录（默认 <repo>/target）
#   FAKE_HOST         直接指定 fake_host 可执行文件
set -euo pipefail

CLIENT=("$@")
if [[ ${#CLIENT[@]} -eq 0 ]]; then
  echo "用法：$0 <client> [args...]" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
FAKE_HOST="${FAKE_HOST:-$TARGET_DIR/debug/examples/fake_host}"

if [[ ! -f "$REPO_ROOT/crates/native/examples/fake_host.rs" && ! -x "$FAKE_HOST" ]]; then
  echo "SKIP：fake_host 尚不存在" >&2
  exit 77
fi
(cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build -q -p app-mcp-native --example fake_host)

WORK="$(mktemp -d)"
HOST_OUT="$WORK/host.out"
CLIENT_LOG="$WORK/client.log"
HOST_PID=""
CLIENT_PID=""
cleanup() {
  [[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
  [[ -n "$HOST_PID" ]] && kill "$HOST_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

"$FAKE_HOST" --addr 127.0.0.1:0 \
  --invoke greet --args '{"name":"World"}' \
  --invoke greet --args '{}' \
  --read app.info \
  --timeout-ms 15000 >"$HOST_OUT" 2>"$WORK/host.err" &
HOST_PID=$!

ADDR=""
for _ in $(seq 1 100); do
  ADDR="$(sed -n 's/^LISTENING \(.*\)$/\1/p' "$HOST_OUT" | head -n1)"
  [[ -n "$ADDR" ]] && break
  sleep 0.1
done
if [[ -z "$ADDR" ]]; then
  echo "FAIL：fake_host 没有输出 LISTENING" >&2
  cat "$WORK/host.err" >&2
  exit 1
fi

"${CLIENT[@]}" "ws://$ADDR" >"$CLIENT_LOG" 2>&1 &
CLIENT_PID=$!

HOST_RC=0
wait "$HOST_PID" || HOST_RC=$?
HOST_PID=""
kill -TERM "$CLIENT_PID" 2>/dev/null || true
wait "$CLIENT_PID" 2>/dev/null || true
CLIENT_PID=""

echo "--- fake_host stdout" >&2
cat "$HOST_OUT" >&2
FAIL=0
if [[ $HOST_RC -ne 0 ]]; then
  echo "FAIL：fake_host 退出码 $HOST_RC" >&2
  FAIL=1
fi
grep -q '"type":"tools"' "$HOST_OUT" || { echo "FAIL：未收到工具列表" >&2; FAIL=1; }
grep '"type":"invoke"' "$HOST_OUT" | grep -q 'Hello, World!' || { echo "FAIL：greet 结果不正确" >&2; FAIL=1; }
grep '"type":"invoke"' "$HOST_OUT" | grep -q '"error"' || { echo "FAIL：缺少 name 时应返回错误" >&2; FAIL=1; }
grep -q '"lang"' "$HOST_OUT" || { echo "FAIL：app.info 读取结果不正确" >&2; FAIL=1; }
if [[ $FAIL -ne 0 ]]; then
  echo "--- fake_host stderr" >&2
  cat "$WORK/host.err" >&2
  echo "--- client log" >&2
  cat "$CLIENT_LOG" >&2
  exit 1
fi
echo "PASS" >&2
