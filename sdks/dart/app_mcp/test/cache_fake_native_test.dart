// 结果缓存声明（spec/protocol.md 3.6，C ABI v22 cache_ttl_ms / cache_scope）经假原生库（test/fake_native/fake_cache.c）
// 核对：字段偏移、工具注册 / 更新（未提供保持、null 清除）、资源注册、无法表达的 ttl 被封装层拒绝。
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
    client = AppMcp(appId: 'quote', appName: '报价', libraryPath: path);
  });
  tearDown(() => client.dispose());

  test('v22 字段偏移与 C 一致（AmToolOptions / AmResourceOptions 的 cache_ttl_ms / cache_scope）', () {
    final tool = calloc<AmToolOptions>();
    final res = calloc<AmResourceOptions>();
    try {
      tool.ref
        ..cache_ttl_ms = 86400000
        ..cache_scope = AmCacheScope.shared;
      expect(Pointer<Uint64>.fromAddress(tool.address + fake.sizeOf(29)).value, 86400000);
      expect(Pointer<Int32>.fromAddress(tool.address + fake.sizeOf(30)).value, 1);
      res.ref
        ..cache_ttl_ms = 5
        ..cache_scope = AmCacheScope.shared;
      expect(Pointer<Uint64>.fromAddress(res.address + fake.sizeOf(31)).value, 5);
      expect(Pointer<Int32>.fromAddress(res.address + fake.sizeOf(32)).value, 1);
    } finally {
      calloc.free(tool);
      calloc.free(res);
    }
  });

  test('工具 cache 经 AmToolOptions 传入；update 未提供保持、替换、null 清除', () {
    final t = client.tool('quote.get',
        description: '查询报价',
        risk: Risk.read,
        cache: const CachePolicy(60000, scope: CacheScope.shared),
        handler: (args, ctx) => null);
    expect(fake.cacheOf('tool:quote.get'), '60000|1');
    t.update(description: '查询报价（新）');
    expect(fake.cacheOf('tool:quote.get'), '60000|1');
    t.update(cache: const CachePolicy(1000));
    expect(fake.cacheOf('tool:quote.get'), '1000|0');
    expect(t.spec.cache, const CachePolicy(1000, scope: CacheScope.private));
    t.update(cache: null);
    expect(fake.cacheOf('tool:quote.get'), '-');
    expect(t.spec.cache, isNull);
    expect(() => t.update(cache: 1000), throwsArgumentError);
    client.tool('quote.plain', description: '未声明', handler: (args, ctx) => null);
    expect(fake.cacheOf('tool:quote.plain'), '-');
  });

  test('声明了缓存但 ttl ≤ 0：invalidConfig，不产生注册 / 更新', () {
    Matcher invalidConfig() =>
        throwsA(isA<AppMcpException>().having((e) => e.code, 'code', AppMcpErrorCode.invalidConfig));
    expect(() => client.tool('quote.zero', description: 'x', cache: const CachePolicy(0), handler: (a, c) => null),
        invalidConfig());
    expect(fake.cacheOf('tool:quote.zero'), isNull);
    final t = client.tool('quote.keep',
        description: 'x', cache: const CachePolicy(5000), handler: (args, ctx) => null);
    expect(() => t.update(cache: const CachePolicy(-1)), invalidConfig());
    expect(fake.cacheOf('tool:quote.keep'), '5000|0');
    expect(t.spec.cache, const CachePolicy(5000));
    expect(() => client.resource('quotes.zero', description: 'x', cache: const CachePolicy(0), read: () => null),
        invalidConfig());
    expect(fake.cacheOf('resource:quotes.zero'), isNull);
  });

  test('资源 cache 经 AmResourceOptions 传入；未声明为 -', () {
    client.resource('quotes',
        description: '全部报价', cache: const CachePolicy(30000, scope: CacheScope.shared), read: () => {'v': 1});
    expect(fake.cacheOf('resource:quotes'), '30000|1');
    client.resource('plain', description: '未声明', read: () => null);
    expect(fake.cacheOf('resource:plain'), '-');
  });

  test('ToolSpec 的 cache 参与相等、hashCode 与 copyWith', () {
    const a = ToolSpec(name: 'a', description: 'b', cache: CachePolicy(1));
    expect(a, isNot(const ToolSpec(name: 'a', description: 'b')));
    expect(a, const ToolSpec(name: 'a', description: 'b', cache: CachePolicy(1)));
    expect(a.hashCode, const ToolSpec(name: 'a', description: 'b', cache: CachePolicy(1)).hashCode);
    expect(a, isNot(const ToolSpec(name: 'a', description: 'b', cache: CachePolicy(1, scope: CacheScope.shared))));
    expect(const ToolSpec(name: 'a', description: 'b').copyWith(cache: const CachePolicy(7)).cache, const CachePolicy(7));
  });
}
