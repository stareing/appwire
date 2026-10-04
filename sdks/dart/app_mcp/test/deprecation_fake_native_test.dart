// 工具弃用声明（spec/protocol.md 3.7，C ABI v23 deprecated_message / deprecated_replacement / deprecated_until）经假原生库
// （test/fake_native/fake_deprecated.c）核对：字段偏移、注册、update 未提供保持 / 替换 / null 清除、replace 整体替换。
import 'dart:ffi';

import 'package:app_mcp/app_mcp.dart';
import 'package:app_mcp/src/bindings.dart';
import 'package:ffi/ffi.dart';
import 'package:test/test.dart';

import 'support/fake_native.dart';

void main() {
  final path = buildFakeLibrary();
  if (path == null) {
    test('假原生库', () {}, skip: '没有 C 编译器，跳过');
    return;
  }
  final fake = FakeNative(path);
  late AppMcp client;

  setUp(() {
    client = AppMcp(appId: 'orders', appName: '订单', libraryPath: path);
  });
  tearDown(() => client.dispose());

  test('v23 字段偏移与 C 一致（AmToolOptions 的 deprecated_*），结构体大小一致', () {
    expect(sizeOf<AmToolOptions>(), fake.sizeOf(16));
    final o = calloc<AmToolOptions>();
    try {
      final (m, r, u) = (Pointer<Utf8>.fromAddress(0x10), Pointer<Utf8>.fromAddress(0x20), Pointer<Utf8>.fromAddress(0x30));
      o.ref
        ..deprecated_message = m
        ..deprecated_replacement = r
        ..deprecated_until = u;
      expect(Pointer<IntPtr>.fromAddress(o.address + fake.sizeOf(33)).value, 0x10);
      expect(Pointer<IntPtr>.fromAddress(o.address + fake.sizeOf(34)).value, 0x20);
      expect(Pointer<IntPtr>.fromAddress(o.address + fake.sizeOf(35)).value, 0x30);
    } finally {
      calloc.free(o);
    }
  });

  test('deprecated 经 AmToolOptions 传入；update 未提供保持、替换、null 清除', () {
    final t = client.tool('orders.list',
        description: '旧版列表',
        deprecated: const ToolDeprecation('改用 orders.list2', replacement: 'list2', until: '2027-06-30'),
        handler: (args, ctx) => null);
    expect(fake.deprecatedOf('orders.list'), '改用 orders.list2|list2|2027-06-30');
    t.update(description: '旧版列表（新）');
    expect(fake.deprecatedOf('orders.list'), '改用 orders.list2|list2|2027-06-30');
    t.update(deprecated: const ToolDeprecation('即将移除'));
    expect(fake.deprecatedOf('orders.list'), '即将移除|-|-');
    expect(t.spec.deprecated, const ToolDeprecation('即将移除'));
    t.update(deprecated: null);
    expect(fake.deprecatedOf('orders.list'), '-');
    expect(t.spec.deprecated, isNull);
    expect(() => t.update(deprecated: '即将移除'), throwsArgumentError);
    t.replace(t.spec.copyWith(deprecated: const ToolDeprecation('m', until: '2028-01-01')));
    expect(fake.deprecatedOf('orders.list'), 'm|-|2028-01-01');
    client.tool('orders.plain', description: '未声明', handler: (args, ctx) => null);
    expect(fake.deprecatedOf('orders.plain'), '-');
  });

  test('ToolSpec 的 deprecated 参与相等、hashCode 与 copyWith', () {
    const a = ToolSpec(name: 'a', description: 'b', deprecated: ToolDeprecation('m', replacement: 'c'));
    expect(a, isNot(const ToolSpec(name: 'a', description: 'b')));
    expect(a, const ToolSpec(name: 'a', description: 'b', deprecated: ToolDeprecation('m', replacement: 'c')));
    expect(a.hashCode, const ToolSpec(name: 'a', description: 'b', deprecated: ToolDeprecation('m', replacement: 'c')).hashCode);
    expect(a, isNot(const ToolSpec(name: 'a', description: 'b', deprecated: ToolDeprecation('m'))));
    expect(a, isNot(const ToolSpec(name: 'a', description: 'b', deprecated: ToolDeprecation('m', replacement: 'c', until: '2027-01-01'))));
    expect(a, isNot(const ToolSpec(name: 'a', description: 'b', deprecated: ToolDeprecation('n', replacement: 'c'))));
    expect(const ToolSpec(name: 'a', description: 'b').copyWith(deprecated: const ToolDeprecation('x')).deprecated,
        const ToolDeprecation('x'));
    expect(a.copyWith(description: 'd').deprecated, a.deprecated);
  });
}
