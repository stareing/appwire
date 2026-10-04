#!/usr/bin/env bash
# 用各语言的真实编译器 / 分析器验证 app-mcp-codegen 的输出（tests/fixtures/shop.json；`deprecated` 步骤用 deprecated.json）。
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

# 标准意图的系统 schema 版本（tests/fixtures/intents.json，--standard-intents）：@AppIntent(schema:) 宏无法在桩中展开，
# 去掉宏行后对 intent 做桩类型检查，schema 表达式单独检查，App 实体用最小替身
if want swift-app-intents-standard-intents; then
  D="$WORK/swift-app-intents-standard-intents"
  rm -rf "$D" && mkdir -p "$D/src" "$D/stub"
  "$BIN" --manifest "$CRATE_DIR/tests/fixtures/intents.json" --target swift-app-intents --standard-intents --out "$D/src" \
    2>"$WORK/logs/gen-swift-app-intents-standard-intents.log"
  if [[ -n "$SWIFTC" ]]; then
    run_step app-intents-std-parse "$SWIFTC" -parse "$D/src/HubStandardIntents.swift"
    awk -v check="$D/SchemaCheck.swift" '
      BEGIN { print "import AppIntents\n\nfunc checkSchema<T: AppSchemaIntent>(_: T) {}\n\npublic struct HubBrowserTab: Sendable {}\n" > check }
      /^@AppIntent\(schema: / { schema = substr($0, 20, length($0) - 20); next }
      schema != "" && /^public struct / {
        name = $3; sub(/:$/, "", name)
        print "@available(iOS 18.0, macOS 15.0, visionOS 2.0, *)\nextension " name ": AssistantSchemaIntent {}" >> check
        print "@available(iOS 18.0, macOS 15.0, visionOS 2.0, *)\nfunc check" name "() { checkSchema(" schema ") }" >> check
        schema = ""
      }
      { print }' "$D/src/HubStandardIntents.swift" >"$D/HubStandardIntents.stripped.swift"
    run_step app-intents-std-stub-typecheck bash -c "
      grep -q 'extension .*: AssistantSchemaIntent' '$D/SchemaCheck.swift' &&
      '$SWIFTC' -swift-version 6 -parse-as-library -emit-module -emit-library -module-name AppIntents \
        -o '$D/stub/libAppIntents.so' -emit-module-path '$D/stub/AppIntents.swiftmodule' '$STUBS/AppIntents.swift' &&
      '$SWIFTC' -swift-version 6 -parse-as-library -typecheck -warnings-as-errors -I '$D/stub' \
        '$D/src/HubTools.swift' '$D/src/HubAppIntents.swift' '$D/HubStandardIntents.stripped.swift' '$D/SchemaCheck.swift'"
  else
    record SKIP swift-app-intents-standard-intents "未找到 swiftc"
  fi
  record NOTE app-intents-std "系统 schema 宏与 App 实体需 Xcode 验证，本机只做了语法、schema 表达式与桩类型检查"
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

# ---------------------------------------------------------------- Android 系统意图（kotlin-appfunctions --standard-intents）
# tests/fixtures/intents.json 的输出：intent-filter 片段的 XML 结构；把片段合并进 verify.StandardIntentActivity 后经 AGP
# 处理 manifest 并编译；Robolectric 在真实 Intent / Uri / PackageManager 上运行 scripts/kotlin/StandardIntentsTest.kt。
if want kotlin-appfunctions-standard-intents; then
  D="$WORK/kotlin-standard-intents"
  SI="$D/si/src"
  rm -rf "$SI" && mkdir -p "$SI/main/kotlin/verify" "$SI/test/kotlin/verify" "$D/fragment"
  "$BIN" --manifest "$CRATE_DIR/tests/fixtures/intents.json" --target kotlin-appfunctions --standard-intents \
    --out "$SI/main/kotlin/generated" 2>"$WORK/logs/gen-kotlin-appfunctions-standard-intents.log"
  # AppFunctions 文件已由 kotlin-appfunctions 步骤（KSP）检查，这里只编译类型文件与系统意图文件
  rm -f "$SI/main/kotlin/generated/HubAppFunctions.kt"
  mv "$SI/main/kotlin/generated/HubStandardIntentFilters.xml" "$D/fragment/"
  run_step standard-intents-xml python3 - "$D/fragment/HubStandardIntentFilters.xml" "$SI/main/AndroidManifest.xml" <<'EOF'
import sys, xml.etree.ElementTree as ET
A = "{http://schemas.android.com/apk/res/android}"
root = ET.parse(sys.argv[1]).getroot()
assert root.tag == "activity", root.tag
filters = root.findall("intent-filter")
assert filters and all(len(f.findall("action")) == 1 for f in filters), "每个 intent-filter 恰有一个 action"
assert all(c.get(A + "name") for f in filters for c in f if c.tag in ("action", "category")), "action / category 须有 android:name"
ET.register_namespace("android", A[1:-1])
manifest = ET.Element("manifest")
app = ET.SubElement(manifest, "application")
root.set(A + "name", "verify.StandardIntentActivity")
app.append(root)
ET.ElementTree(manifest).write(sys.argv[2], encoding="utf-8", xml_declaration=True)
print(f"{len(filters)} 个 intent-filter")
EOF
  if [[ -n "$GRADLE" && "${VERIFY_ANDROID:-1}" != "0" && -d "$ANDROID_SDK/platforms/android-36" ]]; then
    cp "$CRATE_DIR/scripts/kotlin/si.gradle.kts" "$D/si/build.gradle.kts"
    cp "$CRATE_DIR/scripts/kotlin/StandardIntentActivity.kt" "$SI/main/kotlin/verify/"
    cp "$CRATE_DIR/scripts/kotlin/StandardIntentsTest.kt" "$SI/test/kotlin/verify/"
    {
      echo 'pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }'
      echo 'dependencyResolutionManagement { repositories { google(); mavenCentral() } }'
      echo 'rootProject.name = "codegen-verify-standard-intents"'
      echo 'include(":si")'
    } >"$D/settings.gradle.kts"
    {
      echo 'plugins {'
      echo "    kotlin(\"android\") version \"$KOTLIN_VERSION\" apply false"
      echo "    kotlin(\"plugin.serialization\") version \"$KOTLIN_VERSION\" apply false"
      echo "    id(\"com.android.library\") version \"$AGP_VERSION\" apply false"
      echo '}'
    } >"$D/build.gradle.kts"
    printf 'org.gradle.jvmargs=-Xmx2g -Dfile.encoding=UTF-8\nandroid.useAndroidX=true\n' >"$D/gradle.properties"
    echo "sdk.dir=$ANDROID_SDK" >"$D/local.properties"
    run_step standard-intents-robolectric bash -c "cd '$D' && '$GRADLE' --console=plain :si:testDebugUnitTest"
  else
    record SKIP standard-intents-robolectric "需要 gradle 与 Android SDK platforms/android-36（或 VERIFY_ANDROID=0）"
  fi
  record NOTE kotlin-appfunctions-standard-intents "未在真机上由其他 App 发出 Intent 验证"
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

# 鸿蒙标准意图（tests/fixtures/intents.json，--standard-intents）：同样做 ArkTS 类型检查（含 App 实现的 handler 与媒体实体解析器），
# 再用 ets-loader 的解析器检查标准意图执行器（SDK 中有对应 schema 文件、类属性与标准意图参数一致、位置实体类）。
if want harmony-insight-intents-standard-intents; then
  D="$WORK/harmony-standard-intents"
  OHOS_SDK_ETS="${OHOS_SDK_ETS:-$HOME/sdk/ohos/sdk/ets}"
  if command -v node >/dev/null && [[ -d "$OHOS_SDK_ETS/build-tools/ets-loader" ]]; then
    export OHOS_SDK_ETS
    rm -rf "$D" && mkdir -p "$D/src/main"
    "$BIN" --manifest "$CRATE_DIR/tests/fixtures/intents.json" --target harmony-insight-intents --standard-intents \
      --out "$D/src/main" 2>"$WORK/logs/gen-harmony-standard-intents.log"
    cat >"$D/src/main/ets/Usage.ets" <<'EOF_HARMONY_STANDARD'
// verify.sh 的 harmony-standard-intents 步骤：实现 HubToolHandlers 与 HubMediaEntityResolver，检查标准意图生成的接口能接入。
import { AbilityStage } from '@kit.AbilityKit';
import { ToolCallError } from '@app-mcp/harmony';
import {
  HubMediaEntityResolver, HubPlayAudioRequest, HubPlayMusicListRequest, HubPlayVideoRequest, HubStandardIntents,
} from './appmcp/HubStandardIntents';
import { HubIntentRuntime } from './appmcp/HubInsightIntents';
import {
  BrowserOpenParams, CalendarAddParams, FilesShareParams, HubToolHandlers, MapNavigateParams, MessageComposeParams,
  MessageQuickParams, PlayerPlayParams,
} from './appmcp/HubTools';

class Handlers implements HubToolHandlers {
  messageCompose(p: MessageComposeParams): Object | null {
    return p.to.length;
  }

  calendarAdd(p: CalendarAddParams): Object | null {
    return p.title;
  }

  async playerPlay(p: PlayerPlayParams): Promise<Object | null> {
    return `${p.kind ?? 'song'}:${p.query}`;
  }

  filesShare(p: FilesShareParams): Object | null {
    return p.files;
  }

  browserOpen(p: BrowserOpenParams): Object | null {
    return p.url;
  }

  mapNavigate(p: MapNavigateParams): Object | null {
    return `${p.destination.name ?? ''}:${p.destination.lat ?? 0}:${p.mode ?? 'drive'}`;
  }

  messageQuick(p: MessageQuickParams): Object | null {
    return p.text;
  }

  notesCustom(): Object | null {
    return null;
  }

  notesPlain(): Object | null {
    return null;
  }
}

class Resolver implements HubMediaEntityResolver {
  playVideo(request: HubPlayVideoRequest): PlayerPlayParams {
    const params: PlayerPlayParams = { query: '', kind: 'video' };
    params.query = `${request.entityId}#${request.episodeNumber ?? 1}`;
    return params;
  }

  async playAudio(request: HubPlayAudioRequest): Promise<PlayerPlayParams> {
    const params: PlayerPlayParams = { query: request.soundId ?? request.entityId, kind: 'podcast' };
    return params;
  }

  playMusicList(request: HubPlayMusicListRequest): PlayerPlayParams {
    if (request.entityId === undefined) {
      throw new ToolCallError('RESOURCE_NOT_FOUND', '没有歌单');
    }
    const params: PlayerPlayParams = { query: request.entityId, kind: 'playlist' };
    return params;
  }
}

export default class HubStage extends AbilityStage {
  onCreate(): void {
    HubIntentRuntime.handlers = new Handlers();
    HubStandardIntents.mediaResolver = new Resolver();
  }
}
EOF_HARMONY_STANDARD
    run_step harmony-standard-arkts node "$REPO_DIR/sdks/harmony/scripts/arkts-check.cjs" "$D/src/main/ets"
    run_step harmony-standard-intents node "$CRATE_DIR/scripts/harmony-intents-check.cjs" "$D/src/main"
  else
    record SKIP harmony-insight-intents-standard-intents "未找到 node 或 OpenHarmony SDK（OHOS_SDK_ETS）"
  fi
  record NOTE harmony-insight-intents-standard-intents "未经小艺等系统入口调用（标准意图需 App 提供实体 / 平台接入，见报告）"
fi

# ---------------------------------------------------------------- 弃用标注（tests/fixtures/deprecated.json）
# 工具级与参数级弃用（spec/protocol.md 3.7）在各语言的标注：生成代码自身（分派、初始化器、提供者、意图）不得产生弃用警告，
# 各步骤沿用上面的严格选项（-warnings-as-errors、TreatWarningsAsErrors、allWarningsAsErrors、--fatal-infos、mypy --strict）；
# App 的实现（scripts/deprecated/ 下的用法文件）照常编译运行。
if want deprecated; then
  DW="$WORK/deprecated"
  DS="$CRATE_DIR/scripts/deprecated"
  dgen() { # dgen <target> <输出目录> [额外参数]
    local target="$1" out="$2"; shift 2
    rm -rf "$out" && mkdir -p "$out"
    "$BIN" --manifest "$CRATE_DIR/tests/fixtures/deprecated.json" --target "$target" --out "$out" "$@" \
      2>"$WORK/logs/gen-deprecated-$target.log"
  }

  dgen typescript "$DW/typescript/src"
  cp "$DS/usage.ts" "$DW/typescript/src/"
  printf '%s\n' '{"compilerOptions": {"target": "ES2022", "module": "NodeNext", "moduleResolution": "NodeNext", "strict": true,' \
    '  "noUnusedLocals": true, "noUnusedParameters": true, "exactOptionalPropertyTypes": true, "noEmit": true}, "include": ["src"]}' \
    >"$DW/typescript/tsconfig.json"
  if [[ -x "$REPO_DIR/node_modules/.bin/tsc" ]]; then
    run_step deprecated-typescript "$REPO_DIR/node_modules/.bin/tsc" -p "$DW/typescript/tsconfig.json"
  else
    record SKIP deprecated-typescript "未找到 tsc"
  fi

  dgen python "$DW/python"
  cp "$DS/usage.py" "$DW/python/"
  DPY=""
  for cand in python3.13 python3.12 python3.11 python3; do
    if command -v "$cand" >/dev/null && "$cand" -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then DPY="$cand"; break; fi
  done
  if [[ -n "$DPY" ]]; then
    run_step deprecated-python bash -c "cd '$DW/python' && '$DPY' -W error -m py_compile legacy_tools.py && '$DPY' usage.py"
    UVX="$(command -v uvx || echo "$HOME/.local/bin/uvx")"
    if [[ -x "$UVX" ]]; then
      run_step deprecated-python-mypy bash -c "cd '$DW/python' && '$UVX' --quiet mypy --strict --python-version 3.11 legacy_tools.py usage.py"
    else
      record SKIP deprecated-python-mypy "未找到 uvx"
    fi
  else
    record SKIP deprecated-python "需要 Python 3.11+"
  fi

  dgen dart "$DW/dart/lib"
  mkdir -p "$DW/dart/bin" && cp "$DS/main.dart" "$DW/dart/bin/"
  printf 'name: codegen_verify_deprecated\npublish_to: none\nenvironment:\n  sdk: ^3.5.0\n' >"$DW/dart/pubspec.yaml"
  DDART="$HOME/.local/dart-sdk/bin/dart"
  command -v dart >/dev/null && DDART="$(command -v dart)"
  if [[ -x "$DDART" ]]; then
    run_step deprecated-dart bash -c "cd '$DW/dart' && '$DDART' analyze --fatal-infos lib/legacy_tools.dart && '$DDART' run bin/main.dart"
  else
    record SKIP deprecated-dart "未找到 dart"
  fi

  if command -v dotnet >/dev/null; then
    dgen csharp "$DW/csharp"
    cp "$DS/Program.cs" "$DW/csharp/"
    printf '%s\n' '<Project Sdk="Microsoft.NET.Sdk">' '  <PropertyGroup>' '    <OutputType>Exe</OutputType>' \
      '    <TargetFramework>net9.0</TargetFramework>' '    <Nullable>enable</Nullable>' '    <ImplicitUsings>disable</ImplicitUsings>' \
      '    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>' '    <GenerateDocumentationFile>true</GenerateDocumentationFile>' \
      '    <NoWarn>CS1591</NoWarn>' '  </PropertyGroup>' '</Project>' >"$DW/csharp/Verify.csproj"
    run_step deprecated-csharp bash -c "dotnet build '$DW/csharp/Verify.csproj' -nologo -v q && dotnet run --project '$DW/csharp/Verify.csproj' --no-build"
    if [[ "${VERIFY_WINDOWS:-1}" != "0" ]]; then
      dgen windows-app-actions "$DW/windows"
      printf '%s\n' '<Project Sdk="Microsoft.NET.Sdk">' '  <PropertyGroup>' '    <OutputType>Library</OutputType>' \
        '    <TargetFramework>net9.0-windows10.0.26100.0</TargetFramework>' '    <WindowsSdkPackageVersion>10.0.26100.87</WindowsSdkPackageVersion>' \
        '    <EnableWindowsTargeting>true</EnableWindowsTargeting>' '    <Nullable>enable</Nullable>' '    <ImplicitUsings>disable</ImplicitUsings>' \
        '    <TreatWarningsAsErrors>true</TreatWarningsAsErrors>' '  </PropertyGroup>' '</Project>' >"$DW/windows/Provider.csproj"
      run_step deprecated-windows-provider dotnet build "$DW/windows/Provider.csproj" -nologo -v q
    else
      record SKIP deprecated-windows-provider "VERIFY_WINDOWS=0"
    fi
  else
    record SKIP deprecated-csharp "未找到 dotnet"
  fi

  if [[ -n "$SWIFTC" ]]; then
    dgen swift "$DW/swift"
    cp "$DS/main.swift" "$DW/swift/"
    run_step deprecated-swift bash -c "cd '$DW/swift' &&
      '$SWIFTC' -swift-version 6 -parse-as-library -warnings-as-errors -emit-library -module-name LegacyTypes -o libLegacyTypes.so LegacyTools.swift &&
      '$SWIFTC' -swift-version 6 -warnings-as-errors -o verify LegacyTools.swift main.swift && ./verify"
    dgen swift-app-intents "$DW/swift-app-intents/src"
    mkdir -p "$DW/swift-app-intents/stub"
    run_step deprecated-app-intents-stub-typecheck bash -c "
      '$SWIFTC' -swift-version 6 -parse-as-library -emit-module -emit-library -module-name AppIntents \
        -o '$DW/swift-app-intents/stub/libAppIntents.so' -emit-module-path '$DW/swift-app-intents/stub/AppIntents.swiftmodule' '$STUBS/AppIntents.swift' &&
      '$SWIFTC' -swift-version 6 -typecheck -warnings-as-errors -I '$DW/swift-app-intents/stub' \
        '$DW/swift-app-intents/src/LegacyTools.swift' '$DW/swift-app-intents/src/LegacyAppIntents.swift'"
  else
    record SKIP deprecated-swift "未找到 swiftc"
  fi
  record NOTE deprecated-app-intents "弃用 intent 的 @available 只对照桩检查；真实 AppIntents 元数据是否反映弃用需 Xcode 验证"

  if [[ -n "$GRADLE" ]]; then
    K="$DW/kotlin"
    rm -rf "$K/typed/src" "$K/af/src"
    mkdir -p "$K/typed/src/main/kotlin/verify"
    dgen kotlin "$K/typed/src/main/kotlin/generated"
    cp "$DS/Main.kt" "$K/typed/src/main/kotlin/verify/Main.kt"
    cp "$CRATE_DIR/scripts/kotlin/typed.gradle.kts" "$K/typed/build.gradle.kts"
    DAF=0
    if [[ "${VERIFY_ANDROID:-1}" != "0" && -d "$ANDROID_SDK/platforms/android-36" ]]; then
      DAF=1
      dgen kotlin-appfunctions "$K/af/src/main/kotlin/generated"
      cp "$CRATE_DIR/scripts/kotlin/af.gradle.kts" "$K/af/build.gradle.kts"
      sed -i 's/namespace = "appmcp.generated.shop"/namespace = "appmcp.generated.legacy"/' "$K/af/build.gradle.kts"
      printf '<?xml version="1.0" encoding="utf-8"?>\n<manifest xmlns:android="http://schemas.android.com/apk/res/android" />\n' \
        >"$K/af/src/main/AndroidManifest.xml"
    fi
    {
      echo 'pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }'
      echo 'dependencyResolutionManagement { repositories { google(); mavenCentral() } }'
      echo 'rootProject.name = "codegen-verify-deprecated"'
      echo 'include(":typed")'
      [[ $DAF == 1 ]] && echo 'include(":af")'
    } >"$K/settings.gradle.kts"
    {
      echo 'plugins {'
      echo "    kotlin(\"jvm\") version \"$KOTLIN_VERSION\" apply false"
      echo "    kotlin(\"android\") version \"$KOTLIN_VERSION\" apply false"
      echo "    kotlin(\"plugin.serialization\") version \"$KOTLIN_VERSION\" apply false"
      echo "    id(\"com.android.library\") version \"$AGP_VERSION\" apply false"
      echo "    id(\"com.google.devtools.ksp\") version \"$KSP_VERSION\" apply false"
      echo '}'
    } >"$K/build.gradle.kts"
    printf 'org.gradle.jvmargs=-Xmx2g -Dfile.encoding=UTF-8\nandroid.useAndroidX=true\n' >"$K/gradle.properties"
    echo "sdk.dir=$ANDROID_SDK" >"$K/local.properties"
    run_step deprecated-kotlin-jvm bash -c "cd '$K' && '$GRADLE' --console=plain -q :typed:run"
    if [[ $DAF == 1 ]]; then
      # AppFunctions 模块不开 allWarningsAsErrors（KSP 生成代码不受本库控制）：检查日志中没有弃用警告，且 KSP 把两个弃用工具的
      # @Deprecated 写进了函数元数据（<deprecation>）
      run_step deprecated-kotlin-appfunctions-ksp bash -c "cd '$K' && '$GRADLE' --console=plain :af:compileReleaseKotlin 2>&1 | tee '$WORK/logs/deprecated-af-gradle.log' &&
        ! grep -E '^w: .*(deprecated|Deprecated)' '$WORK/logs/deprecated-af-gradle.log' &&
        [[ \$(grep -c '<deprecation>' af/build/generated/ksp/release/resources/assets/legacy_app_function_service.xml) == 2 ]]"
    else
      record SKIP deprecated-kotlin-appfunctions "需要 Android SDK platforms/android-36（或 VERIFY_ANDROID=0）"
    fi
  else
    record SKIP deprecated-kotlin "未找到 gradle"
  fi

  OHOS_SDK_ETS="${OHOS_SDK_ETS:-$HOME/sdk/ohos/sdk/ets}"
  if command -v node >/dev/null && [[ -d "$OHOS_SDK_ETS/build-tools/ets-loader" ]]; then
    export OHOS_SDK_ETS
    dgen harmony-insight-intents "$DW/harmony/src/main"
    cp "$DS/Usage.ets" "$DW/harmony/src/main/ets/Usage.ets"
    run_step deprecated-harmony-arkts node "$REPO_DIR/sdks/harmony/scripts/arkts-check.cjs" "$DW/harmony/src/main/ets"
    run_step deprecated-harmony-intents node "$CRATE_DIR/scripts/harmony-intents-check.cjs" "$DW/harmony/src/main"
  else
    record SKIP deprecated-harmony "未找到 node 或 OpenHarmony SDK（OHOS_SDK_ETS）"
  fi
fi

# ---------------------------------------------------------------- 结果
echo
echo "================ 验证结果 ================"
printf '%s\n' "${RESULTS[@]}"
exit "$FAILED"
