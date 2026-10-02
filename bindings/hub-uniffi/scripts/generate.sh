#!/usr/bin/env bash
# 构建 app-mcp-hub-uniffi 并生成 Kotlin / Python / Swift 绑定，复制到各 SDK 目录。
#
# 用法：bash bindings/hub-uniffi/scripts/generate.sh [--release] [--android] [--hub-app] [--abi <abi>]... [--android-features <list>]
#        [--only kotlin|python|swift] [--no-strip]
#
#   --release   以 cargo profile bindings-release 构建本机库（release 优化，只去调试信息、保留 uniffi 元数据
#               所在的符号表）。发布 jar / wheel / Swift 包前必须加；默认 debug
#   --android   额外交叉编译 Android 各 ABI 的 .so（总是 cargo profile mobile-release：体积优先、strip）
#               到 sdks/kotlin/app-mcp-hub-android/src/main/jniLibs/<abi>/（AAR :app-mcp-hub-android 打包）
#               （需要 ANDROID_NDK_HOME 或 ~/Android/Sdk/ndk/<ver>，以及对应 rustup target）
#   --abi X     与 --android 连用，只编译指定 ABI（可重复；arm64-v8a、x86_64、armeabi-v7a、x86），
#               默认全部；发布前必须全 ABI 重编（uniffi 加载时校验 checksum）
#   --android-features L  与 --android 连用：Hub 能力组合（bindings/hub-uniffi/Cargo.toml 的 features，逗号分隔），
#               默认 mobile,schema-validation（不含 MCP 出口与上游聚合，保留 Hub 侧参数校验）；体积优先可用 mobile（去掉校验，约省 2.7 MB）、完整能力用 desktop。
#               独立 Hub App 需要 MCP 出口，用 --hub-app 另编一份（不要改这里的默认组合）。
#               本机库（jar / wheel / Swift）总是 desktop（完整能力）；各组合的 uniffi 接口相同
#   --hub-app   额外交叉编译独立 Hub App（sdks/kotlin/hub-app-android，fd 上的 MCP）用的一份 .so：
#               features mobile,schema-validation,mcp-server（arm64 约 9.5 MB，比默认组合多约 1.8 MB），
#               输出到 sdks/kotlin/hub-app-android/src/main/jniLibs/<abi>/；Hub App 的 packaging 规则优先打包这一份、
#               忽略 :app-mcp-hub-android 里的同名精简库（见该模块 build.gradle.kts）。可与 --android 同时给出；
#               同样受 --abi 约束，发布前全 ABI 重编
#   --only X    只生成一种语言
#   --no-strip  复制到 SDK 目录的本机库保留调试信息（默认 strip -S，debug 版约 250 MB → 约 40 MB）
#
# 生成物（均已在各 SDK 的 .gitignore 中忽略，不纳入版本控制）：
#   sdks/python/src/app_mcp_hub/app_mcp_hub_uniffi.py、libapp_mcp_hub_uniffi.so
#   sdks/kotlin/app-mcp-hub/src/generated/kotlin/dev/appmcp/hub/ffi/app_mcp_hub_uniffi.kt
#   sdks/kotlin/app-mcp-hub/src/generated/resources/<jna-platform>/libapp_mcp_hub_uniffi.so
#   sdks/kotlin/app-mcp-hub-android/src/main/jniLibs/<abi>/libapp_mcp_hub_uniffi.so（--android）
#   sdks/kotlin/hub-app-android/src/main/jniLibs/<abi>/libapp_mcp_hub_uniffi.so（--hub-app）
#   sdks/swift/Sources/AppMcpHubBindings/AppMcpHubBindings.swift
#   sdks/swift/Sources/app_mcp_hub_uniffiFFI/include/{app_mcp_hub_uniffiFFI.h,module.modulemap}
#   sdks/swift/lib/libapp_mcp_hub_uniffi.so（macOS 为 .dylib）
#
# 与 App 端绑定（bindings/uniffi）的名称全部区分（Kotlin 包 dev.appmcp.hub.ffi、Swift 模块
# AppMcpHubBindings、Python 包 app_mcp_hub），两者可在同一进程加载。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/target}"

PROFILE=debug
CARGO_PROFILE_ARGS=()
ANDROID=0
ONLY=""
ABIS=()
ANDROID_FEATURES=mobile,schema-validation
# 独立 Hub App：精简组合 + MCP 出口（HubService 启动前检查 hub_features().mcp_server，缺少时向 Agent 报 HUB_UNSUPPORTED）。
HUB_APP_FEATURES=mobile,schema-validation,mcp-server
HUB_APP=0
STRIP=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) PROFILE=bindings-release; CARGO_PROFILE_ARGS=(--profile bindings-release) ;;
    --android) ANDROID=1 ;;
    --hub-app) HUB_APP=1 ;;
    --only) ONLY="$2"; shift ;;
    --abi) ABIS+=("$2"); shift ;;
    --android-features) ANDROID_FEATURES="$2"; shift ;;
    --no-strip) STRIP=0 ;;
    -h|--help) sed -n '2,37p' "$0"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
  shift
done
[[ ${#ABIS[@]} -gt 0 ]] || ABIS=(arm64-v8a armeabi-v7a x86_64 x86)

want() { [[ -z "$ONLY" || "$ONLY" == "$1" ]]; }

cd "$ROOT"
echo "==> 构建 app-mcp-hub-uniffi（$PROFILE）"
# 不用 release profile：其 strip = true 会去掉 UniFFI 元数据符号；bindings-release 只去调试信息（见根 Cargo.toml）。
cargo build -p app-mcp-hub-uniffi "${CARGO_PROFILE_ARGS[@]}"

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
LIB="$OUT/libapp_mcp_hub_uniffi.$LIB_EXT"
[[ -f "$LIB" ]] || { echo "找不到 $LIB" >&2; exit 1; }

BINDGEN="$OUT/hub-uniffi-bindgen"
GEN_TMP="$(mktemp -d)"
trap 'rm -rf "$GEN_TMP"' EXIT

gen() {
  local lang="$1"
  echo "==> 生成 $lang 绑定"
  # library 模式；配置取自 bindings/hub-uniffi/uniffi.toml。
  "$BINDGEN" generate "$LIB" --language "$lang" --out-dir "$GEN_TMP/$lang" --no-format
}

# 复制本机库；默认去掉调试信息（uniffi 元数据在符号表里，不受影响）。
copy_lib() {
  local dst="$1"
  cp "$LIB" "$dst"
  if [[ $STRIP -eq 1 && "$LIB_EXT" == so ]] && command -v strip >/dev/null; then
    strip -S "$dst"
  fi
}

if want python; then
  gen python
  PY_PKG="$ROOT/sdks/python/src/app_mcp_hub"
  cp "$GEN_TMP/python/app_mcp_hub_uniffi.py" "$PY_PKG/app_mcp_hub_uniffi.py"
  copy_lib "$PY_PKG/libapp_mcp_hub_uniffi.$LIB_EXT"
fi

if want kotlin; then
  gen kotlin
  KT="$ROOT/sdks/kotlin/app-mcp-hub/src/generated"
  rm -rf "$KT/kotlin" "$KT/resources"
  mkdir -p "$KT/kotlin" "$KT/resources/$JNA_OS-$JNA_ARCH"
  cp -r "$GEN_TMP/kotlin/." "$KT/kotlin/"
  copy_lib "$KT/resources/$JNA_OS-$JNA_ARCH/libapp_mcp_hub_uniffi.$LIB_EXT"
fi

if want swift; then
  gen swift
  SW="$ROOT/sdks/swift"
  mkdir -p "$SW/Sources/AppMcpHubBindings" "$SW/Sources/app_mcp_hub_uniffiFFI/include" "$SW/lib"
  cp "$GEN_TMP/swift/AppMcpHubBindings.swift" "$SW/Sources/AppMcpHubBindings/AppMcpHubBindings.swift"
  cp "$GEN_TMP/swift/app_mcp_hub_uniffiFFI.h" "$SW/Sources/app_mcp_hub_uniffiFFI/include/app_mcp_hub_uniffiFFI.h"
  cp "$GEN_TMP/swift/app_mcp_hub_uniffiFFI.modulemap" "$SW/Sources/app_mcp_hub_uniffiFFI/include/module.modulemap"
  # SwiftPM 动态链接（Package.swift 设置 -L 与 rpath 指向 sdks/swift/lib）。
  # Apple 平台发布时应在 Cargo.toml 的 crate-type 加 staticlib，并打包为 XCFramework。
  copy_lib "$SW/lib/libapp_mcp_hub_uniffi.$LIB_EXT"
fi

# 交叉编译一份 Android .so：$1 = features，$2 = jniLibs 目录（其下 <abi>/libapp_mcp_hub_uniffi.so）。
# @why 两种组合产物同名、同在 target/<triple>/mobile-release/：每次编译后立即复制，不依赖 target 中留下的是哪一份。
build_android() {
  local features="$1" jni="$2"
  local NDK="${ANDROID_NDK_HOME:-}"
  if [[ -z "$NDK" ]]; then
    NDK="$(ls -d "$HOME"/Android/Sdk/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
  fi
  [[ -d "$NDK" ]] || { echo "找不到 Android NDK（设置 ANDROID_NDK_HOME）" >&2; exit 1; }
  local TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
  local API=24
  for pair in aarch64-linux-android:arm64-v8a x86_64-linux-android:x86_64 \
              armv7-linux-androideabi:armeabi-v7a i686-linux-android:x86; do
    local triple="${pair%%:*}" abi="${pair##*:}" clang
    if [[ " ${ABIS[*]} " != *" $abi "* ]]; then continue; fi
    case "$triple" in
      armv7-linux-androideabi) clang="armv7a-linux-androideabi$API-clang" ;;
      *) clang="$triple$API-clang" ;;
    esac
    local env_triple
    env_triple="$(echo "$triple" | tr 'a-z-' 'A-Z_')"
    echo "==> 交叉编译 $triple → $abi（mobile-release，features：$features）→ ${jni#"$ROOT"/}"
    env "CARGO_TARGET_${env_triple}_LINKER=$TOOLCHAIN/$clang" \
        "CC_${triple//-/_}=$TOOLCHAIN/$clang" \
        "AR_${triple//-/_}=$TOOLCHAIN/llvm-ar" \
      cargo build -p app-mcp-hub-uniffi --lib --no-default-features --features "$features" \
        --target "$triple" --profile mobile-release
    mkdir -p "$jni/$abi"
    cp "$CARGO_TARGET_DIR/$triple/mobile-release/libapp_mcp_hub_uniffi.so" "$jni/$abi/"
  done
}

if [[ $ANDROID -eq 1 ]]; then
  build_android "$ANDROID_FEATURES" "$ROOT/sdks/kotlin/app-mcp-hub-android/src/main/jniLibs"
fi
if [[ $HUB_APP -eq 1 ]]; then
  build_android "$HUB_APP_FEATURES" "$ROOT/sdks/kotlin/hub-app-android/src/main/jniLibs"
fi

echo "==> 完成"
