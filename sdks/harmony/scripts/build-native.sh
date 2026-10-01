#!/usr/bin/env bash
# 交叉编译 bindings/harmony 并把 libapp_mcp_harmony.so 放进本 HAR 的 libs/<ABI>/（hvigor 打包 HAR 时随包发布）。
#
# 用法：scripts/build-native.sh [--debug] [ABI ...]   ABI：arm64-v8a（默认，真机）、x86_64（模拟器）、armeabi-v7a
# 需要：OHOS_NDK_HOME=<OpenHarmony SDK 根目录>（含 native/），rustup 已安装对应 *-unknown-linux-ohos 目标。
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$here/../.." && pwd)"
crate="$repo/bindings/harmony"

: "${OHOS_NDK_HOME:?请设置 OHOS_NDK_HOME 为 OpenHarmony SDK 根目录（其下有 native/）}"
export PATH="$OHOS_NDK_HOME/native/llvm/bin:$PATH"
# 宿主机的编译器环境变量（如 conda 的 CFLAGS=-march=nocona）会被 cc 带进交叉编译，这里清除。
unset CFLAGS CPPFLAGS CXXFLAGS LDFLAGS CC CXX AR

profile=release
abis=()
for arg in "$@"; do
  case "$arg" in
    --debug) profile=debug ;;
    *) abis+=("$arg") ;;
  esac
done
[ ${#abis[@]} -eq 0 ] && abis=(arm64-v8a)

for abi in "${abis[@]}"; do
  case "$abi" in
    arm64-v8a) target=aarch64-unknown-linux-ohos ;;
    x86_64) target=x86_64-unknown-linux-ohos ;;
    armeabi-v7a) target=armv7-unknown-linux-ohos ;;
    *) echo "未知 ABI：$abi" >&2; exit 1 ;;
  esac
  flags=(--target "$target")
  [ "$profile" = release ] && flags+=(--release)
  (cd "$crate" && cargo build "${flags[@]}")
  mkdir -p "$here/libs/$abi"
  cp "$repo/target/$target/$profile/libapp_mcp_harmony.so" "$here/libs/$abi/"
  echo "已生成 libs/$abi/libapp_mcp_harmony.so"
done
