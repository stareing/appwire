#!/usr/bin/env bash
# 构建 app-mcp-uniffi 并生成 Kotlin / Python / Swift 绑定，复制到各 SDK 目录。
#
# 用法：bash bindings/uniffi/scripts/generate.sh [--release] [--android] [--abi <abi>]... [--only kotlin|python|swift] [--no-strip]
#
#   --release   以 cargo profile bindings-release 构建本机库（release 优化，只去调试信息、保留 uniffi 元数据
#               所在的符号表）。发布 jar / wheel / Swift 包前必须加；默认 debug
#   --android   额外交叉编译 Android 各 ABI 的 .so（总是 cargo profile mobile-release：体积优先、strip；
#               debug 版每个约 100 MB）到 sdks/kotlin/app-mcp-android/src/main/jniLibs/<abi>/
#               （需要 ANDROID_NDK_HOME 或 ~/Android/Sdk/ndk/<ver>，以及对应 rustup target）
#   --abi X     与 --android 连用，只编译指定 ABI（可重复；arm64-v8a、armeabi-v7a、x86_64、x86），
#               默认全部。真机调试时只要 arm64-v8a 可省时间与磁盘；发布前必须全 ABI 重编
#               （uniffi 加载时校验 checksum，旧 .so 与新绑定不匹配）
#   --only X    只生成一种语言
#   --no-strip  复制到 SDK 目录的本机库保留调试信息（默认 strip -S，debug 版约 117 MB → 约 20 MB）
#
# 生成物（均已在各 SDK 的 .gitignore 中忽略，不纳入版本控制）：
#   sdks/python/src/app_mcp/app_mcp_uniffi.py、libapp_mcp_uniffi.so
#   sdks/kotlin/app-mcp/src/generated/kotlin/dev/appmcp/ffi/app_mcp_uniffi.kt
#   sdks/kotlin/app-mcp/src/generated/resources/<jna-platform>/libapp_mcp_uniffi.so
#   sdks/swift/Sources/AppMcpBindings/AppMcpBindings.swift
#   sdks/swift/Sources/app_mcp_uniffiFFI/include/{app_mcp_uniffiFFI.h,module.modulemap}
#   sdks/swift/lib/libapp_mcp_uniffi.so（macOS 为 .dylib）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/target}"

PROFILE=debug
CARGO_PROFILE_ARGS=()
ANDROID=0
ONLY=""
ABIS=()
STRIP=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) PROFILE=bindings-release; CARGO_PROFILE_ARGS=(--profile bindings-release) ;;
    --android) ANDROID=1 ;;
    --only) ONLY="$2"; shift ;;
    --abi) ABIS+=("$2"); shift ;;
    --no-strip) STRIP=0 ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
  shift
done

# 复制本机库；默认去掉调试信息（uniffi 元数据在符号表里，不受影响）。
copy_lib() {
  local dst="$1"
  cp "$LIB" "$dst"
  if [[ $STRIP -eq 1 && "$LIB_EXT" == so ]] && command -v strip >/dev/null; then
    strip -S "$dst"
  fi
}

want() { [[ -z "$ONLY" || "$ONLY" == "$1" ]]; }

cd "$ROOT"
echo "==> 构建 app-mcp-uniffi（$PROFILE）"
# 不用 release profile：其 strip = true 会去掉 UniFFI 元数据符号，uniffi-bindgen 读不到
# （报 "No UniFFI metadata found"）；bindings-release 只去调试信息（见根 Cargo.toml）。
cargo build -p app-mcp-uniffi "${CARGO_PROFILE_ARGS[@]}"

case "$(uname -s)" in
  Darwin) LIB_EXT=dylib; JNA_OS=darwin ;;
  *) LIB_EXT=so; JNA_OS=linux ;;
esac
case "$(uname -m)" in
  x86_64|amd64) JNA_ARCH=x86-64 ;;
  aarch64|arm64) JNA_ARCH=aarch64 ;;
  *) JNA_ARCH="$(uname -m)" ;;
esac
OUT="$CARGO_TARGET_DIR/$PROFILE"
LIB="$OUT/libapp_mcp_uniffi.$LIB_EXT"
STATIC_LIB="$OUT/libapp_mcp_uniffi.a"
[[ -f "$LIB" ]] || { echo "找不到 $LIB" >&2; exit 1; }

BINDGEN="$OUT/uniffi-bindgen"
GEN_TMP="$(mktemp -d)"
trap 'rm -rf "$GEN_TMP"' EXIT

gen() {
  local lang="$1"
  echo "==> 生成 $lang 绑定"
  # library 模式（uniffi 0.32 自动识别 cdylib，--library 已废弃）；配置取自 bindings/uniffi/uniffi.toml。
  "$BINDGEN" generate "$LIB" --language "$lang" --out-dir "$GEN_TMP/$lang" --no-format
}

if want python; then
  gen python
  PY_PKG="$ROOT/sdks/python/src/app_mcp"
  cp "$GEN_TMP/python/app_mcp_uniffi.py" "$PY_PKG/app_mcp_uniffi.py"
  copy_lib "$PY_PKG/libapp_mcp_uniffi.$LIB_EXT"
fi

if want kotlin; then
  gen kotlin
  KT="$ROOT/sdks/kotlin/app-mcp/src/generated"
  rm -rf "$KT"
  mkdir -p "$KT/kotlin" "$KT/resources/$JNA_OS-$JNA_ARCH"
  cp -r "$GEN_TMP/kotlin/." "$KT/kotlin/"
  copy_lib "$KT/resources/$JNA_OS-$JNA_ARCH/libapp_mcp_uniffi.$LIB_EXT"
fi

if want swift; then
  gen swift
  SW="$ROOT/sdks/swift"
  mkdir -p "$SW/Sources/AppMcpBindings" "$SW/Sources/app_mcp_uniffiFFI/include" "$SW/lib"
  cp "$GEN_TMP/swift/AppMcpBindings.swift" "$SW/Sources/AppMcpBindings/AppMcpBindings.swift"
  cp "$GEN_TMP/swift/app_mcp_uniffiFFI.h" "$SW/Sources/app_mcp_uniffiFFI/include/app_mcp_uniffiFFI.h"
  cp "$GEN_TMP/swift/app_mcp_uniffiFFI.modulemap" "$SW/Sources/app_mcp_uniffiFFI/include/module.modulemap"
  # SwiftPM 动态链接（Package.swift 设置 -L 与 rpath 指向 sdks/swift/lib）。
  # Apple 平台发布时应改为打包 XCFramework（静态库 $STATIC_LIB）。
  copy_lib "$SW/lib/libapp_mcp_uniffi.$LIB_EXT"
fi

if [[ $ANDROID -eq 1 ]]; then
  NDK="${ANDROID_NDK_HOME:-}"
  if [[ -z "$NDK" ]]; then
    NDK="$(ls -d "$HOME"/Android/Sdk/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
  fi
  [[ -d "$NDK" ]] || { echo "找不到 Android NDK（设置 ANDROID_NDK_HOME）" >&2; exit 1; }
  TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
  API=24
  JNI="$ROOT/sdks/kotlin/app-mcp-android/src/main/jniLibs"
  for pair in aarch64-linux-android:arm64-v8a armv7-linux-androideabi:armeabi-v7a \
              x86_64-linux-android:x86_64 i686-linux-android:x86; do
    triple="${pair%%:*}"; abi="${pair##*:}"
    if [[ ${#ABIS[@]} -gt 0 && " ${ABIS[*]} " != *" $abi "* ]]; then continue; fi
    case "$triple" in
      armv7-linux-androideabi) clang="armv7a-linux-androideabi$API-clang" ;;
      *) clang="$triple$API-clang" ;;
    esac
    env_triple="$(echo "$triple" | tr 'a-z-' 'A-Z_')"
    echo "==> 交叉编译 $triple → $abi（mobile-release）"
    env "CARGO_TARGET_${env_triple}_LINKER=$TOOLCHAIN/$clang" \
        "CC_${triple//-/_}=$TOOLCHAIN/$clang" \
        "AR_${triple//-/_}=$TOOLCHAIN/llvm-ar" \
      cargo build -p app-mcp-uniffi --lib --no-default-features --target "$triple" --profile mobile-release
    mkdir -p "$JNI/$abi"
    cp "$CARGO_TARGET_DIR/$triple/mobile-release/libapp_mcp_uniffi.so" "$JNI/$abi/"
  done
fi

echo "==> 完成"
