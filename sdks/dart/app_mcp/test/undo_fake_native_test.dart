// 撤销（spec/protocol.md 3.8，C ABI v24 AmToolOptions.undoable 与 AmCallResult.undo_*）经假原生库（test/fake_native/fake_undo.c）
// 核对：字段偏移、undoable 注册 / update 保持 / 替换 / null 恢复 false、ToolResult.undo 编码进 am_call_complete_ex。
// 不合法的撤销信息由真实库的核心去掉（一致性用例 result-undo）。
import 'dart:convert';
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
    client = AppMcp(appId: 'todo', appName: '待办', libraryPath: path);
  });
  tearDown(() => client.dispose());

  test('v24 字段偏移与 C 一致（AmToolOptions.undoable、AmCallResult.undo_*），结构体大小一致', () {
    expect(sizeOf<AmToolOptions>(), fake.sizeOf(16));
    expect(sizeOf<AmCallResult>(), fake.sizeOf(17));
    final o = calloc<AmToolOptions>();
    final r = calloc<AmCallResult>();
    try {
      o.ref.undoable = true;
      expect(Pointer<Uint8>.fromAddress(o.address + fake.sizeOf(36)).value, 1);
      r.ref
        ..undo_tool = Pointer<Utf8>.fromAddress(0x10)
        ..undo_arguments_json = Pointer<Utf8>.fromAddress(0x20)
        ..undo_label = Pointer<Utf8>.fromAddress(0x30);
      expect(Pointer<IntPtr>.fromAddress(r.address + fake.sizeOf(37)).value, 0x10);
      expect(Pointer<IntPtr>.fromAddress(r.address + fake.sizeOf(38)).value, 0x20);
      expect(Pointer<IntPtr>.fromAddress(r.address + fake.sizeOf(39)).value, 0x30);
    } finally {
      calloc.free(o);
      calloc.free(r);
    }
  });

  test('undoable 经 AmToolOptions 传入；update 未提供保持、替换、null 恢复 false', () {
    final t = client.tool('todo.add', description: '添加待办', undoable: true, handler: (args, ctx) => null);
    expect(fake.undoableOf('todo.add'), '1');
    t.update(description: '添加待办（新）');
    expect(fake.undoableOf('todo.add'), '1');
    t.update(undoable: false);
    expect(fake.undoableOf('todo.add'), '0');
    t.update(undoable: true);
    t.update(undoable: null);
    expect((fake.undoableOf('todo.add'), t.spec.undoable), ('0', false));
    expect(() => t.update(undoable: 1), throwsArgumentError);
    t.replace(t.spec.copyWith(undoable: true));
    expect(fake.undoableOf('todo.add'), '1');
    client.tool('todo.plain', description: '未声明', handler: (args, ctx) => null);
    expect(fake.undoableOf('todo.plain'), '0');
  });

  test('ToolResult.undo 经 am_call_complete_ex 传入：完整、只有 tool、无撤销信息', () async {
    client.tool('todo.add',
        description: '添加待办',
        handler: (args, ctx) => switch (args['form']) {
              'full' => const ToolResult({'id': 3},
                  undo: UndoAction('todo.remove', arguments: {'id': 3}, label: '删除刚添加的待办')),
              'min' => const ToolResult({'on': true}, undo: UndoAction('todo.toggle')),
              _ => const ToolResult({'id': 4}, summary: '已添加'),
            });
    Future<(Map<String, dynamic>, String?)> run(String form) async {
      final idx = fake.invoke('todo.add', jsonEncode({'form': form}));
      final result = jsonDecode(await fake.waitResult(idx)) as Map<String, dynamic>;
      return (result, fake.callUndo(idx));
    }

    final (full, fullUndo) = await run('full');
    expect(full['data'], {'id': 3});
    expect(fullUndo, 'todo.remove|{"id":3}|删除刚添加的待办');
    expect((await run('min')).$2, 'todo.toggle|-|-');
    expect((await run('none')).$2, '-');
  });

  test('UndoAction 参数无法编码为 JSON 时调用以 HANDLER_ERROR 失败', () async {
    client.tool('todo.bad',
        description: '参数无法编码',
        handler: (args, ctx) => ToolResult(null, undo: UndoAction('todo.remove', arguments: {'at': DateTime(2026)})));
    final idx = fake.invoke('todo.bad', '{}');
    final r = jsonDecode(await fake.waitResult(idx)) as Map<String, dynamic>;
    expect((r['ok'], r['kind']), (false, 'HANDLER_ERROR'));
    expect(r['message'], contains('撤销参数'));
  });

  test('ToolSpec 的 undoable 参与相等、hashCode 与 copyWith', () {
    const a = ToolSpec(name: 'a', description: 'b', undoable: true);
    expect(a, isNot(const ToolSpec(name: 'a', description: 'b')));
    expect(a.hashCode, const ToolSpec(name: 'a', description: 'b', undoable: true).hashCode);
    expect(const ToolSpec(name: 'a', description: 'b').copyWith(undoable: true), a);
    expect(a.copyWith(description: 'd').undoable, isTrue);
  });
}
