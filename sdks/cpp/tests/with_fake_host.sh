#!/usr/bin/env bash
# 构建 fake_host 后运行 "<命令> [参数...] <fake_host 路径>"（生命周期集成测试用）。
# 环境变量：CARGO_TARGET_DIR（默认 <repo>/target）、FAKE_HOST（直接指定可执行文件，跳过构建）。
set -euo pipefail
if [[ $# -eq 0 ]]; then
  echo "用法：$0 <命令> [参数...]" >&2
  exit 2
fi
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
if [[ -z "${FAKE_HOST:-}" ]]; then
  FAKE_HOST="$TARGET_DIR/debug/examples/fake_host"
  (cd "$REPO_ROOT" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build -q -p app-mcp-native --example fake_host)
fi
exec "$@" "$FAKE_HOST"
