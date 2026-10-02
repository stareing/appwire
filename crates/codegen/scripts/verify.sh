#!/usr/bin/env bash
# 用各语言的真实编译器 / 分析器验证 app-mcp-codegen 的输出（tests/fixtures/shop.json）。
#
# 用法：bash crates/codegen/scripts/verify.sh [target ...]
#   不带参数时验证全部；缺少对应工具链的步骤标记为 SKIP，不算失败。
# 环境变量：
#   CARGO_TARGET_DIR   cargo 输出目录（默认 <仓库>/target）
#   CODEGEN_BIN        直接使用已构建的 app-mcp-codegen（跳过 cargo build）
#   VERIFY_WORK        临时工作目录（默认 <仓库>/target/codegen-verify）
#   VERIFY_ANDROID=0   跳过 AppFunctions 的 Android/KSP 编译（较慢，首次需要下载依赖）
#   VERIFY_WINDOWS=0   跳过 Windows App Actions 提供者的编译（需要下载 Windows SDK 投影包）
set -uo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$CRATE_DIR/../.." && pwd)"
FIXTURE="$CRATE_DIR/tests/fixtures/shop.json"
STUBS="$CRATE_DIR/scripts/stubs"
REPO_ROOT_DEFAULT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT_DEFAULT/target}"
WORK="${VERIFY_WORK:-$CARGO_TARGET_DIR/codegen-verify}"
BIN="${CODEGEN_BIN:-$CARGO_TARGET_DIR/debug/app-mcp-codegen}"

RESULTS=()
FAILED=0

record() { # record <状态> <名称> [说明]
  RESULTS+=("$(printf '%-5s %-28s %s' "$1" "$2" "${3:-}")")
  [[ "$1" == "FAIL" ]] && FAILED=1
  return 0
}

run_step() { # run_step <名称> <命令...>：执行并记录 PASS / FAIL（失败时打印输出）
  local name="$1"; shift
  local log="$WORK/logs/$name.log"
  echo "==> $name"
  if "$@" >"$log" 2>&1; then
    record PASS "$name"
  else
    record FAIL "$name" "日志：$log"
    tail -n 40 "$log"
  fi
}

gen() { # gen <target> <输出目录> [额外参数]
  local target="$1" out="$2"; shift 2
  rm -rf "$out" && mkdir -p "$out"
  "$BIN" --manifest "$FIXTURE" --target "$target" --out "$out" "$@" 2>"$WORK/logs/gen-$target.log"
}

want() { # 是否需要验证某个 target
  [[ ${#SELECTED[@]} -eq 0 ]] && return 0
  local t; for t in "${SELECTED[@]}"; do [[ "$t" == "$1" ]] && return 0; done
  return 1
}

SELECTED=("$@")
mkdir -p "$WORK/logs"

if [[ -z "${CODEGEN_BIN:-}" ]]; then
  echo "==> 构建 app-mcp-codegen"
  if ! (cd "$REPO_DIR" && cargo build -q -p app-mcp-codegen); then
    echo "构建失败" >&2
    exit 1
  fi
fi

# ---------------------------------------------------------------- TypeScript
if want typescript; then
  D="$WORK/typescript"
  gen typescript "$D/src"
  cat >"$D/src/usage.ts" <<'EOF'
import { dispatchShopTool, type ShopToolHandlers, type CartCheckoutParams } from "./shopTools.js";

const handlers: ShopToolHandlers = {
  catalogSearch: (p) => ({ keyword: p.keyword ?? "", limit: p.limit ?? 20 }),
  cartAdd: async (p) => `${p.productId} x ${p.qty}`,
  cartRemoveItem: (p) => p.itemId,
  cartCheckout: (p: CartCheckoutParams) => p.shipping?.method === "express",
  todosAdd: (p) => [p.title, p.class, p["is-urgent"]],
  todosClear: () => null,
  ordersExport: (p) => p.labels,
  statsSummary: (p) => p.period ?? "all",
};
export const result = dispatchShopTool(handlers, "cart.add", { productId: "p1", qty: 2 });
EOF
  cat >"$D/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "exactOptionalPropertyTypes": true,
    "noEmit": true
  },
  "include": ["src"]
}
EOF
  TSC="$REPO_DIR/node_modules/.bin/tsc"
  if [[ -x "$TSC" ]]; then
    run_step typescript "$TSC" -p "$D/tsconfig.json"
  else
    record SKIP typescript "未找到 $TSC"
  fi
fi

# ---------------------------------------------------------------- Python
if want python; then
  D="$WORK/python"
  gen python "$D"
  cat >"$D/usage.py" <<'EOF'
from typing import Any

from shop_tools import CartCheckoutParams, ShopToolHandlers, dispatch_shop_tool, TodosAddParams


class Impl:
    def catalog_search(self, params: Any) -> Any:
        return params.get("keyword")

    def cart_add(self, params: Any) -> Any:
        return f"{params['productId']} x {params['qty']}"

    def cart_remove_item(self, params: Any) -> Any:
        return params["itemId"]

    def cart_checkout(self, params: CartCheckoutParams) -> Any:
        return params.get("shipping", {}).get("method")

    def todos_add(self, params: TodosAddParams) -> Any:
        return params["title"], params.get("class"), params.get("is-urgent")

    def todos_clear(self, params: Any) -> Any:
        return None

    def orders_export(self, params: Any) -> Any:
        return params.get("labels")

    def stats_summary(self, params: Any) -> Any:
        return params.get("period")


handlers: ShopToolHandlers = Impl()
assert dispatch_shop_tool(handlers, "cart.add", {"productId": "p1", "qty": 2}) == "p1 x 2"
assert dispatch_shop_tool(handlers, "todos.add", {"title": "t", "class": "c"}) == ("t", "c", None)
try:
    dispatch_shop_tool(handlers, "nope", None)
except KeyError:
    pass
else:
    raise AssertionError("未知工具应抛出 KeyError")
print("ok")
EOF
  PY=""
  for cand in python3.13 python3.12 python3.11 python3; do
    if command -v "$cand" >/dev/null && "$cand" -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then
      PY="$cand"; break
    fi
  done
  if [[ -n "$PY" ]]; then
    run_step python-compile "$PY" -m py_compile "$D/shop_tools.py"
    run_step python-run bash -c "cd '$D' && '$PY' usage.py"
    if command -v uvx >/dev/null || [[ -x "$HOME/.local/bin/uvx" ]]; then
      UVX="$(command -v uvx || echo "$HOME/.local/bin/uvx")"
      run_step python-mypy bash -c "cd '$D' && '$UVX' --quiet mypy --strict --python-version 3.11 shop_tools.py usage.py"
    else
      record SKIP python-mypy "未找到 uvx"
    fi
  else
    record SKIP python "需要 Python 3.11+"
  fi
fi

# ---------------------------------------------------------------- Dart
if want dart; then
  D="$WORK/dart"
  gen dart "$D/lib"
  cat >"$D/pubspec.yaml" <<'EOF'
name: codegen_verify
publish_to: none
environment:
  sdk: ^3.5.0
EOF
  mkdir -p "$D/bin"
  cat >"$D/bin/main.dart" <<'EOF'
import 'dart:async';
import 'dart:convert';

import '../lib/shop_tools.dart';

class Impl implements ShopToolHandlers {
  @override
  FutureOr<Object?> catalogSearch(CatalogSearchParams params) => params.toJson();
  @override
  FutureOr<Object?> cartAdd(CartAddParams params) => '${params.productId} x ${params.qty}';
  @override
  FutureOr<Object?> cartRemoveItem(CartRemoveItemParams params) => params.itemId;
  @override
  FutureOr<Object?> cartCheckout(CartCheckoutParams params) => params.toJson();
  @override
  FutureOr<Object?> todosAdd(TodosAddParams params) => params.toJson();
  @override
  FutureOr<Object?> todosClear(TodosClearParams params) => null;
  @override
  FutureOr<Object?> ordersExport(OrdersExportParams params) => params.toJson();
  @override
  FutureOr<Object?> statsSummary(StatsSummaryParams params) => params.period?.value;
}

Future<void> main() async {
  final h = Impl();
  final checkout = {
    'addressId': 'a1',
    'shipping': {'method': 'express', 'deliverAfter': '2026-01-01T00:00:00Z'},
    'items': [
      {'itemId': 'i1', 'qty': 2},
    ],
    'giftWrap': true,
  };
  final out = await dispatchShopTool(h, 'cart.checkout', jsonDecode(jsonEncode(checkout)) as Map<String, dynamic>);
  if (jsonEncode(out) != jsonEncode(checkout)) {
    throw StateError('往返不一致：${jsonEncode(out)}');
  }
  final search = await dispatchShopTool(h, 'catalog.search', {'category': 'home-goods', 'limit': 5, 'maxPrice': 3});
  if (jsonEncode(search) != '{"category":"home-goods","limit":5,"maxPrice":3.0}') {
    throw StateError('catalog.search：${jsonEncode(search)}');
  }
  final todo = await dispatchShopTool(h, 'todos.add', {'title': 't', 'class': 'c', 'is-urgent': true, 'tags': ['x']});
  if (jsonEncode(todo) != '{"title":"t","tags":["x"],"class":"c","is-urgent":true}') {
    throw StateError('todos.add：${jsonEncode(todo)}');
  }
  final export = await dispatchShopTool(h, 'orders.export', {'labels': {'a': 'b'}, 'extra': [1, 2]});
  if (jsonEncode(export) != '{"labels":{"a":"b"},"extra":[1,2]}') {
    throw StateError('orders.export：${jsonEncode(export)}');
  }
  print('ok');
}
EOF
  DART="$HOME/.local/dart-sdk/bin/dart"
  command -v dart >/dev/null && DART="$(command -v dart)"
  if [[ -x "$DART" ]]; then
    run_step dart-analyze bash -c "cd '$D' && '$DART' analyze --fatal-infos lib/shop_tools.dart"
    run_step dart-run bash -c "cd '$D' && '$DART' run bin/main.dart"
  else
    record SKIP dart "未找到 dart"
  fi
fi

# ---------------------------------------------------------------- C#
if want csharp; then
  D="$WORK/csharp"
  gen csharp "$D"
  cat >"$D/Verify.csproj" <<'EOF'
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net9.0</TargetFramework>
    <Nullable>enable</Nullable>
    <ImplicitUsings>disable</ImplicitUsings>
    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>
    <GenerateDocumentationFile>true</GenerateDocumentationFile>
    <NoWarn>CS1591</NoWarn>
  </PropertyGroup>
</Project>
EOF
  cat >"$D/Program.cs" <<'EOF'
using System;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using AppMcp.Generated.Shop;

internal sealed class Impl : IShopToolHandlers
{
    public Task<object?> CatalogSearchAsync(CatalogSearchParams args, CancellationToken ct) => Task.FromResult<object?>(args);
    public Task<object?> CartAddAsync(CartAddParams args, CancellationToken ct) => Task.FromResult<object?>($"{args.ProductId} x {args.Qty}");
    public Task<object?> CartRemoveItemAsync(CartRemoveItemParams args, CancellationToken ct) => Task.FromResult<object?>(args.ItemId);
    public Task<object?> CartCheckoutAsync(CartCheckoutParams args, CancellationToken ct) => Task.FromResult<object?>(args);
    public Task<object?> TodosAddAsync(TodosAddParams args, CancellationToken ct) => Task.FromResult<object?>(args);
    public Task<object?> TodosClearAsync(TodosClearParams args, CancellationToken ct) => Task.FromResult<object?>(null);
    public Task<object?> OrdersExportAsync(OrdersExportParams args, CancellationToken ct) => Task.FromResult<object?>(args);
    public Task<object?> StatsSummaryAsync(StatsSummaryParams args, CancellationToken ct) => Task.FromResult<object?>(args.Period);
}

internal static class Program
{
    private static async Task<int> Main()
    {
        var h = new Impl();
        var checkout = JsonDocument.Parse("""{"addressId":"a1","shipping":{"method":"express","deliverAfter":"2026-01-01T00:00:00Z"},"items":[{"itemId":"i1","qty":2}],"giftWrap":true}""").RootElement;
        var result = (CartCheckoutParams)(await ShopTools.DispatchAsync(h, "cart.checkout", checkout))!;
        Check(result.Shipping?.Method == CartCheckoutShippingMethod.Express, "枚举解析");
        Check(result.Items?[0].Qty == 2, "嵌套数组");
        var search = (CatalogSearchParams)(await ShopTools.DispatchAsync(h, "catalog.search", JsonDocument.Parse("""{"category":"home-goods"}""").RootElement))!;
        Check(search.Category == CatalogSearchCategory.HomeGoods, "带连字符的枚举值");
        var json = JsonSerializer.Serialize(search, ShopTools.JsonOptions);
        Check(json.Contains("\"home-goods\""), "枚举序列化：" + json);
        var todo = (TodosAddParams)(await ShopTools.DispatchAsync(h, "todos.add", JsonDocument.Parse("""{"title":"t","class":"c","is-urgent":true}""").RootElement))!;
        Check(todo.Class == "c" && todo.IsUrgent == true, "保留字 / 连字符属性");
        try
        {
            await ShopTools.DispatchAsync(h, "cart.add", JsonDocument.Parse("""{"qty":1}""").RootElement);
            Check(false, "缺少必填字段应失败");
        }
        catch (JsonException) { }
        var empty = await ShopTools.DispatchAsync(h, "todos.clear", default);
        Check(empty is null, "空参数");
        Console.WriteLine("ok");
        return 0;
    }

    private static void Check(bool ok, string what)
    {
        if (!ok) throw new InvalidOperationException("检查失败：" + what);
    }
}
EOF
  if command -v dotnet >/dev/null; then
    run_step csharp-build dotnet build "$D/Verify.csproj" -nologo -v q
    run_step csharp-run dotnet run --project "$D/Verify.csproj" --no-build
  else
    record SKIP csharp "未找到 dotnet"
  fi
fi

# ---------------------------------------------------------------- Swift
SWIFTC=""
for cand in swiftc "$HOME/.local/swift/usr/bin/swiftc"; do
  if command -v "$cand" >/dev/null 2>&1 || [[ -x "$cand" ]]; then SWIFTC="$cand"; break; fi
done

if want swift; then
  D="$WORK/swift"
  gen swift "$D"
  cat >"$D/main.swift" <<'EOF'
import Foundation

struct Impl: ShopToolHandlers {
    func catalogSearch(_ params: CatalogSearchParams) async throws -> any Encodable & Sendable { params }
    func cartAdd(_ params: CartAddParams) async throws -> any Encodable & Sendable { "\(params.productId) x \(params.qty)" }
    func cartRemoveItem(_ params: CartRemoveItemParams) async throws -> any Encodable & Sendable { params.itemId }
    func cartCheckout(_ params: CartCheckoutParams) async throws -> any Encodable & Sendable { params }
    func todosAdd(_ params: TodosAddParams) async throws -> any Encodable & Sendable { params }
    func todosClear(_ params: TodosClearParams) async throws -> any Encodable & Sendable { "cleared" }
    func ordersExport(_ params: OrdersExportParams) async throws -> any Encodable & Sendable { params }
    func statsSummary(_ params: StatsSummaryParams) async throws -> any Encodable & Sendable { params }
}

func check(_ ok: Bool, _ what: String) {
    if !ok { fatalError("检查失败：\(what)") }
}

let h = Impl()
let checkout = #"{"addressId":"a1","shipping":{"method":"express"},"items":[{"itemId":"i1","qty":2}]}"#
let r = try await ShopTools.dispatch(h, name: "cart.checkout", arguments: Data(checkout.utf8)) as? CartCheckoutParams
check(r?.shipping?.method == .express && r?.items?.first?.qty == 2, "嵌套类型")
let t = try await ShopTools.dispatch(h, name: "todos.add", arguments: Data(#"{"title":"t","class":"c","is-urgent":true}"#.utf8)) as? TodosAddParams
check(t?.class == "c" && t?.isUrgent == true, "保留字 / 连字符属性")
let e = try await ShopTools.dispatch(h, name: "orders.export", arguments: Data(#"{"extra":{"a":[1,null,"x"]}}"#.utf8)) as? OrdersExportParams
check(e?.extra == .object(["a": .array([.number(1), .null, .string("x")])]), "JSONValue")
let s = try await ShopTools.dispatch(h, name: "todos.clear", arguments: Data()) as? String
check(s == "cleared", "空参数")
print("ok")
EOF
  if [[ -n "$SWIFTC" ]]; then
    run_step swift-build "$SWIFTC" -swift-version 6 -parse-as-library -warnings-as-errors -emit-library -module-name ShopTypes -o "$D/libShopTypes.so" "$D/ShopTools.swift"
    run_step swift-run bash -c "cd '$D' && '$SWIFTC' -swift-version 6 -o verify ShopTools.swift main.swift && ./verify"
  else
    record SKIP swift "未找到 swiftc"
  fi
fi

if want swift-app-intents; then
  D="$WORK/swift-app-intents"
  gen swift-app-intents "$D/src"
  if [[ -n "$SWIFTC" ]]; then
    # Linux 上没有 AppIntents 框架：先做语法解析，再对照按官方文档签名编写的桩模块做类型检查
    run_step app-intents-parse "$SWIFTC" -parse "$D/src/ShopAppIntents.swift"
    mkdir -p "$D/stub"
    run_step app-intents-stub-typecheck bash -c "
      '$SWIFTC' -swift-version 6 -parse-as-library -emit-module -emit-library -module-name AppIntents \
        -o '$D/stub/libAppIntents.so' -emit-module-path '$D/stub/AppIntents.swiftmodule' '$STUBS/AppIntents.swift' &&
      '$SWIFTC' -swift-version 6 -typecheck -warnings-as-errors -I '$D/stub' '$D/src/ShopTools.swift' '$D/src/ShopAppIntents.swift'"
  else
    record SKIP swift-app-intents "未找到 swiftc"
  fi
  record NOTE app-intents "真实 AppIntents 框架需 Xcode（macOS），本机只做了语法与桩类型检查"
fi

# 扩展布局 + 全部可选输出：共享包编为模块，再分别检查扩展入口与 App 侧（开发者的 provider 用一个最小实现模拟）
if want swift-app-intents-extension; then
  D="$WORK/swift-app-intents-extension"
  gen swift-app-intents "$D/src" --app-intents-extension --app-intents-execution-targets --app-intents-cancellable
  if [[ -n "$SWIFTC" ]]; then
    mkdir -p "$D/stub" "$D/mod"
    cat >"$D/Provider.swift" <<'EOF'
import ShopIntents

struct DemoHandlers: ShopToolHandlers {
    func catalogSearch(_ params: CatalogSearchParams) async throws -> any Encodable & Sendable { params }
    func cartAdd(_ params: CartAddParams) async throws -> any Encodable & Sendable { params.productId }
    func cartRemoveItem(_ params: CartRemoveItemParams) async throws -> any Encodable & Sendable { params.itemId }
    func cartCheckout(_ params: CartCheckoutParams) async throws -> any Encodable & Sendable { params }
    func todosAdd(_ params: TodosAddParams) async throws -> any Encodable & Sendable { params }
    func todosClear(_ params: TodosClearParams) async throws -> any Encodable & Sendable { "cleared" }
    func ordersExport(_ params: OrdersExportParams) async throws -> any Encodable & Sendable { params }
    func statsSummary(_ params: StatsSummaryParams) async throws -> any Encodable & Sendable { params }
}

enum ShopIntentHandlersProvider: ShopIntentHandlersProviding {
    static func makeHandlers() -> any ShopToolHandlers { DemoHandlers() }
}

func installCancelHook() {
    ShopIntentRuntime.onCancel { tool, reason in print(tool, reason) }
}
EOF
    S="$D/src/ShopIntents/Sources/ShopIntents"
    run_step app-intents-ext-stub-typecheck bash -c "
      '$SWIFTC' -swift-version 6 -parse-as-library -emit-module -emit-library -module-name AppIntents \
        -o '$D/stub/libAppIntents.so' -emit-module-path '$D/stub/AppIntents.swiftmodule' '$STUBS/AppIntents.swift' &&
      '$SWIFTC' -swift-version 6 -parse-as-library -warnings-as-errors -emit-module -emit-library -module-name ShopIntents \
        -I '$D/stub' -L '$D/stub' -lAppIntents -o '$D/mod/libShopIntents.so' -emit-module-path '$D/mod/ShopIntents.swiftmodule' \
        '$S/ShopTools.swift' '$S/ShopAppIntents.swift' &&
      '$SWIFTC' -swift-version 6 -parse-as-library -typecheck -warnings-as-errors -I '$D/stub' -I '$D/mod' \
        '$D/Provider.swift' '$D/src/ShopIntentsExtension/ShopIntentsExtension.swift' &&
      '$SWIFTC' -swift-version 6 -parse-as-library -typecheck -warnings-as-errors -I '$D/stub' -I '$D/mod' \
        '$D/Provider.swift' '$D/src/App/ShopAppIntentsPackage.swift'"
    SWIFT_BIN="$(dirname "$(command -v "$SWIFTC" || echo "$SWIFTC")")/swift"
    if [[ -x "$SWIFT_BIN" ]]; then
      run_step app-intents-ext-package bash -c "cd '$D/src/ShopIntents' && '$SWIFT_BIN' package dump-package >/dev/null"
    fi
  else
    record SKIP swift-app-intents-extension "未找到 swiftc"
  fi
  record NOTE app-intents-extension "扩展布局只做了桩类型检查；AppIntentsExtension / 包内 intent 元数据需 Xcode 实机验证"
fi

# ---------------------------------------------------------------- Kotlin（JVM）与 AppFunctions（Android + KSP）
GRADLE=""
for cand in gradle "$HOME"/.local/gradle-*/bin/gradle; do
  if command -v "$cand" >/dev/null 2>&1 || [[ -x "$cand" ]]; then GRADLE="$cand"; break; fi
done
ANDROID_SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
# Gradle / AGP 需要完整 JDK（javac、jlink）；系统只有 JRE 时尝试用户目录下的 JDK
if [[ -z "${JAVA_HOME:-}" || ! -x "${JAVA_HOME:-}/bin/javac" ]]; then
  for cand in "$HOME"/.local/jdk-* /usr/lib/jvm/*; do
    if [[ -x "$cand/bin/javac" ]]; then export JAVA_HOME="$cand"; break; fi
  done
fi
KOTLIN_VERSION="${KOTLIN_VERSION:-2.2.21}"
AGP_VERSION="${AGP_VERSION:-8.13.2}"
KSP_VERSION="${KSP_VERSION:-2.3.12}"

if want kotlin || want kotlin-appfunctions; then
  D="$WORK/kotlin"
  rm -rf "$D/typed/src" "$D/af/src"
  mkdir -p "$D/typed/src/main/kotlin/verify" "$D/af/src/main/kotlin"
  gen kotlin "$D/typed/src/main/kotlin/generated"
  gen kotlin-appfunctions "$D/af/src/main/kotlin/generated"
  INCLUDE_AF=0
  if want kotlin-appfunctions && [[ "${VERIFY_ANDROID:-1}" != "0" && -d "$ANDROID_SDK/platforms/android-36" ]]; then
    INCLUDE_AF=1
  fi
  {
    echo 'pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }'
    echo 'dependencyResolutionManagement { repositories { google(); mavenCentral() } }'
    echo 'rootProject.name = "codegen-verify"'
    echo 'include(":typed")'
    [[ $INCLUDE_AF == 1 ]] && echo 'include(":af")'
  } >"$D/settings.gradle.kts"
  {
    echo 'plugins {'
    echo "    kotlin(\"jvm\") version \"$KOTLIN_VERSION\" apply false"
    echo "    kotlin(\"android\") version \"$KOTLIN_VERSION\" apply false"
    echo "    kotlin(\"plugin.serialization\") version \"$KOTLIN_VERSION\" apply false"
    echo "    id(\"com.android.library\") version \"$AGP_VERSION\" apply false"
    echo "    id(\"com.google.devtools.ksp\") version \"$KSP_VERSION\" apply false"
    echo '}'
  } >"$D/build.gradle.kts"
  printf 'org.gradle.jvmargs=-Xmx2g -Dfile.encoding=UTF-8\nandroid.useAndroidX=true\n' >"$D/gradle.properties"
  echo "sdk.dir=$ANDROID_SDK" >"$D/local.properties"
  cp "$CRATE_DIR/scripts/kotlin/typed.gradle.kts" "$D/typed/build.gradle.kts"
  cp "$CRATE_DIR/scripts/kotlin/Main.kt" "$D/typed/src/main/kotlin/verify/Main.kt"
  if [[ $INCLUDE_AF == 1 ]]; then
    # AppFunctions 模块：Tools.kt 与 AppFunctions.kt 一起编译并运行 KSP
    cp "$CRATE_DIR/scripts/kotlin/af.gradle.kts" "$D/af/build.gradle.kts"
    printf '<?xml version="1.0" encoding="utf-8"?>\n<manifest xmlns:android="http://schemas.android.com/apk/res/android" />\n' \
      >"$D/af/src/main/AndroidManifest.xml"
  fi
  if [[ -n "$GRADLE" ]]; then
    if want kotlin; then
      run_step kotlin-jvm bash -c "cd '$D' && '$GRADLE' --console=plain -q :typed:run"
    fi
    if want kotlin-appfunctions; then
      if [[ $INCLUDE_AF == 1 ]]; then
        run_step kotlin-appfunctions-ksp bash -c "cd '$D' && '$GRADLE' --console=plain :af:compileReleaseKotlin"
      else
        record SKIP kotlin-appfunctions "需要 Android SDK platforms/android-36（或 VERIFY_ANDROID=0）"
      fi
    fi
  else
    record SKIP kotlin "未找到 gradle"
  fi
fi

# ---------------------------------------------------------------- Windows App Actions
if want windows-app-actions; then
  D="$WORK/windows-app-actions"
  gen windows-app-actions "$D/src"
  run_step windows-json bash -c "python3 -m json.tool '$D/src/Assets/ShopActions.json' >/dev/null && python3 -m json.tool '$D/src/Assets/McpServer/manifest.json' >/dev/null"
  if [[ "${VERIFY_WINDOWS:-1}" != "0" ]] && command -v dotnet >/dev/null; then
    # 在 Linux 上交叉编译：Windows.AI.Actions 投影需要较新的 Microsoft.Windows.SDK.NET.Ref
    cat >"$D/src/Provider.csproj" <<'EOF'
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Library</OutputType>
    <TargetFramework>net9.0-windows10.0.26100.0</TargetFramework>
    <WindowsSdkPackageVersion>10.0.26100.87</WindowsSdkPackageVersion>
    <EnableWindowsTargeting>true</EnableWindowsTargeting>
    <Nullable>enable</Nullable>
    <ImplicitUsings>disable</ImplicitUsings>
    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>
  </PropertyGroup>
</Project>
EOF
    run_step windows-provider-build dotnet build "$D/src/Provider.csproj" -nologo -v q
  else
    record SKIP windows-provider-build "VERIFY_WINDOWS=0 或未找到 dotnet"
  fi
  record NOTE windows-app-actions "未在 Windows 上注册运行（需要 MSIX 包标识）"
fi

# ---------------------------------------------------------------- 鸿蒙意图框架
# 用 OpenHarmony SDK 的 ohos-typescript + ArkTSLinter 检查生成的 ArkTS（含 @app-mcp/harmony 与 Kit 声明），
# 再用 ets-loader 的意图装饰器校验规则（含 ajv 编译 parameters）检查每个执行器。
if want harmony-insight-intents; then
  D="$WORK/harmony"
  OHOS_SDK_ETS="${OHOS_SDK_ETS:-$HOME/sdk/ohos/sdk/ets}"
  if command -v node >/dev/null && [[ -d "$OHOS_SDK_ETS/build-tools/ets-loader" ]]; then
    export OHOS_SDK_ETS
    gen harmony-insight-intents "$D/src/main"
    cp "$STUBS/HarmonyUsage.ets" "$D/src/main/ets/Usage.ets"
    run_step harmony-arkts node "$REPO_DIR/sdks/harmony/scripts/arkts-check.cjs" "$D/src/main/ets"
    run_step harmony-intents node "$CRATE_DIR/scripts/harmony-intents-check.cjs" "$D/src/main"
  else
    record SKIP harmony-insight-intents "未找到 node 或 OpenHarmony SDK（OHOS_SDK_ETS）"
  fi
  record NOTE harmony-insight-intents "未用 hvigor 打包、未在鸿蒙设备上执行（无 DevEco 工具链与真机）"
fi

# ---------------------------------------------------------------- 结果
echo
echo "================ 验证结果 ================"
printf '%s\n' "${RESULTS[@]}"
exit "$FAILED"
