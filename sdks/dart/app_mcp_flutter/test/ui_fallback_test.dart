// 进程内控件兜底（spec/ui-fallback.md）：执行引擎在 widget 测试中直接驱动；注册与门控用 app_mcp 的假原生库（需要 cc）。
import 'dart:ffi';

import 'package:app_mcp_flutter/app_mcp_flutter.dart';
import 'package:ffi/ffi.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/fake_library.dart';

/// 测试页面：按钮、禁用按钮、已声明按钮、复选框、文本框、密码框、打开对话框、可移除按钮、长列表。
class _Page extends StatefulWidget {
  const _Page({required this.log, required this.name, required this.password});

  final List<String> log;
  final TextEditingController name;
  final TextEditingController password;

  @override
  State<_Page> createState() => _PageState();
}

class _PageState extends State<_Page> {
  bool agree = false;
  bool showTemp = true;

  @override
  Widget build(BuildContext context) => Scaffold(
        body: Column(children: [
          ElevatedButton(onPressed: () => widget.log.add('pay'), child: const Text('Pay')),
          const ElevatedButton(onPressed: null, child: Text('Locked')),
          McpDeclared(
              tool: 'cart.clear',
              child: TextButton(onPressed: () => widget.log.add('clear'), child: const Text('Clear'))),
          CheckboxListTile(
              title: const Text('Agree'), value: agree, onChanged: (v) => setState(() => agree = v ?? false)),
          TextField(controller: widget.name, decoration: const InputDecoration(labelText: 'Name')),
          TextField(
              controller: widget.password, obscureText: true, decoration: const InputDecoration(labelText: 'Password')),
          if (showTemp) TextButton(onPressed: () => setState(() => showTemp = false), child: const Text('Remove me')),
          TextButton(
            onPressed: () => showDialog<void>(
                context: context,
                builder: (c) => AlertDialog(
                      title: const Text('Confirm'),
                      actions: [
                        TextButton(onPressed: () => Navigator.pop(c), child: const Text('Cancel')),
                        TextButton(
                            onPressed: () {
                              widget.log.add('ok');
                              Navigator.pop(c);
                            },
                            child: const Text('OK')),
                      ],
                    )),
            child: const Text('Open dialog'),
          ),
          SizedBox(
            height: 120,
            child: ListView(children: [
              for (var i = 0; i < 30; i++) ListTile(title: Text('Row $i'), onTap: () => widget.log.add('row $i')),
            ]),
          ),
        ]),
      );
}

void main() {
  late List<String> log;
  late TextEditingController name;
  late TextEditingController password;
  late SemanticsHandle semantics;

  Future<McpUiInspector> pumpPage(WidgetTester tester) async {
    log = [];
    name = TextEditingController();
    password = TextEditingController(text: 'hunter2');
    addTearDown(name.dispose);
    addTearDown(password.dispose);
    semantics = tester.ensureSemantics();
    await tester.pumpWidget(MaterialApp(home: _Page(log: log, name: name, password: password)));
    // 两帧（第二帧 +20 ms）：对话框淡入的第一帧不透明度为 0（语义被排除），对应真实环境的"一帧 + 50 ms"。
    return McpUiInspector(
        frame: () async {
          await tester.pump();
          await tester.pump(const Duration(milliseconds: 20));
        },
        settleDelay: Duration.zero);
  }

  /// SemanticsHandle 必须在测试体内释放（flutter_test 在 tearDown 前检查）。
  void uiTest(String description, Future<void> Function(WidgetTester tester, McpUiInspector ui) body) =>
      testWidgets(description, (tester) async {
        final ui = await pumpPage(tester);
        try {
          await body(tester, ui);
        } finally {
          semantics.dispose();
        }
      });

  Map<String, Object?> item(UiOutline o, String name) =>
      o.items.firstWhere((i) => i['name'] == name, orElse: () => throw StateError('no item $name in\n${o.text}'));
  String refOf(UiOutline o, String name) => item(o, name)['ref'] as String;

  Matcher toolError(String? reason, {String? contains}) => isA<ToolCallError>()
      .having((e) => e.kind, 'kind', ErrorKind.invalidInput)
      .having((e) => (e.details as Map?)?['reason'], 'reason', reason)
      .having((e) => e.message, 'message', contains == null ? anything : contains_(contains));

  uiTest('outline：可见控件、角色 / 状态、密码掩码不泄露长度、已声明、query / limit', (tester, ui) async {
    final o = ui.outline();
    expect(o.text, contains('按钮「Pay」'));
    expect(o.text, contains('按钮「Locked」 disabled'));
    expect(o.text, contains('按钮「Clear」 [已声明：cart.clear]'));
    expect(o.text, contains('输入框「Name」'));
    expect(o.text, contains('密码框「Password」= "••••"'));
    expect(o.text, isNot(contains('hunter2')));
    expect(o.text, isNot(contains('•••••••'))); // 7 位密码不按长度输出
    expect(item(o, 'Agree')['role'], 'checkbox');
    expect(item(o, 'Agree')['states'], ['unchecked']);
    expect(item(o, 'Locked')['states'], ['disabled']);
    expect(item(o, 'Clear')['declared'], 'cart.clear');
    expect(o.hint, isNotNull);
    expect(o.text, contains('Row 0'));
    expect(o.text, isNot(contains('Row 20'))); // 滚动区外的缓存节点不列出
    expect(RegExp(r'^e[1-9]\d*$').hasMatch(refOf(o, 'Pay')), isTrue);

    final q = ui.outline(query: 'row 1');
    expect(q.items.map((i) => i['name']), everyElement(startsWith('Row 1')));
    final limited = ui.outline(limit: 2);
    expect(limited.items, hasLength(2));
    expect(limited.remaining, o.total - 2);
    expect(limited.text, contains('…另有 ${o.total - 2} 个元素未列出'));
    // 引用稳定
    expect(refOf(ui.outline(), 'Pay'), refOf(o, 'Pay'));
  });

  uiTest('click：激活按钮、复选框；变化摘要只列变化', (tester, ui) async {
    final o = ui.outline();
    final r = await ui.click(refOf(o, 'Pay'));
    expect(log, ['pay']);
    expect(r['ok'], isTrue);

    final agree = refOf(o, 'Agree');
    final changed = await ui.click(agree);
    expect(changed['changes'], contains('$agree Agree 变为 checked'));

    final declared = await ui.click(refOf(o, 'Clear'));
    expect(log, ['pay', 'clear']);
    expect(declared['hint'], contains('cart.clear'));
  });

  uiTest('fill：文本框（聚焦、等一帧、setText）与复选框', (tester, ui) async {
    final o = ui.outline();
    final r = await ui.fill(refOf(o, 'Name'), 'Alice');
    expect(name.text, 'Alice');
    expect((r['changes'] as List).join('\n'), contains('值变为 "Alice"'));
    await ui.fill(refOf(o, 'Agree'), true);
    expect(item(ui.outline(), 'Agree')['states'], ['checked']);
    // 已是目标状态：不切换
    await ui.fill(refOf(o, 'Agree'), true);
    expect(item(ui.outline(), 'Agree')['states'], ['checked']);
    await expectLater(ui.fill(refOf(o, 'Name'), true), throwsA(toolError(null, contains: '需要文本值')));
    await expectLater(ui.fill(refOf(o, 'Pay'), 'x'), throwsA(toolError('unsupported')));
  });

  uiTest('密码类控件：拒绝填写与按键，read 只返回掩码', (tester, ui) async {
    final o = ui.outline();
    final pwd = refOf(o, 'Password');
    await expectLater(ui.fill(pwd, 'stolen'), throwsA(toolError('secure')));
    await expectLater(ui.press(pwd, 'Enter'), throwsA(toolError('secure')));
    expect(password.text, 'hunter2');
    expect(ui.read(ref: pwd)['text'], 'Password ••••');
    expect(ui.read()['text'], isNot(contains('hunter2')));
  });

  uiTest('失效引用与禁用控件', (tester, ui) async {
    final o = ui.outline();
    await expectLater(ui.click(refOf(o, 'Locked')), throwsA(toolError('TOOL_DISABLED', contains: '已禁用')));
    final temp = refOf(o, 'Remove me');
    await ui.click(temp);
    await expectLater(ui.click(temp), throwsA(toolError(null, contains: '引用 $temp 已失效')));
    await expectLater(ui.click('e9999'), throwsA(toolError(null, contains: '已失效')));
    expect(ui.outline().text, isNot(contains('Remove me')));
  });

  uiTest('对话框：新增分组摘要、within、模态遮挡下层、Escape 关闭', (tester, ui) async {
    final o = ui.outline();
    final pay = refOf(o, 'Pay');
    final opened = await ui.click(refOf(o, 'Open dialog'));
    await tester.pumpAndSettle();
    final changes = (opened['changes'] as List).cast<String>();
    expect(changes.any((c) => c.startsWith('新增对话框「Alert」') && c.contains('含 2 个可交互元素')), isTrue,
        reason: changes.join('\n'));

    final withDialog = ui.outline();
    expect(withDialog.text, contains('» '));
    expect(withDialog.text, isNot(contains('Pay'))); // 模态路由屏蔽下层
    // 模态路由的 BlockSemantics 把下层节点移出语义树：引用暂时失效，对话框关闭后恢复。
    await expectLater(ui.click(pay), throwsA(toolError(null, contains: '已失效')));
    final dialogRef = RegExp(r'» (e\d+) 对话框').firstMatch(withDialog.text)!.group(1)!;
    final inside = ui.outline(within: dialogRef);
    expect(inside.items.map((i) => i['name']), containsAll(['Cancel', 'OK']));
    expect(item(inside, 'OK')['group'], '对话框「Alert」');
    expect(ui.read(ref: dialogRef)['text'], contains('Confirm'));

    await ui.press(null, 'Escape');
    await tester.pumpAndSettle();
    expect(ui.outline().text, contains('Pay'));
    await ui.click(pay);
    expect(log, ['pay']);
    await expectLater(ui.press(null, 'F13'), throwsA(toolError(null, contains: '不支持的按键')));
  });

  uiTest('press：Tab 移动焦点、Enter 提交文本框', (tester, ui) async {
    final o = ui.outline();
    await ui.press(refOf(o, 'Name'), 'Tab');
    expect(item(ui.outline(), 'Password')['states'], contains('focused'));
    await ui.press(refOf(o, 'Pay'), 'Enter');
    expect(log, ['pay']);
  });

  uiTest('scroll：按方向翻页；read 读取文本', (tester, ui) async {
    final o = ui.outline();
    final row0 = refOf(o, 'Row 0');
    final scrolled = await ui.scroll(row0, direction: 'down');
    await tester.pumpAndSettle();
    expect(scrolled['ok'], isTrue);
    expect(ui.outline().text, isNot(contains('「Row 0」')));
    await expectLater(ui.scroll(row0, direction: 'sideways'), throwsA(toolError(null)));
    expect(ui.read(ref: refOf(o, 'Pay'))['text'], 'Pay');
    final short = ui.read(maxChars: 5);
    expect(short['truncated'], isTrue);
    expect((short['text'] as String).runes.length, lessThanOrEqualTo(5));
    expect(short['text'], endsWith('…'));
  });

  final path = buildFakeLibrary();

  testWidgets('McpUiFallback：view 工具、注解如实声明、随可见窗口启用、连接时才开启语义树', (tester) async {
    final lib = DynamicLibrary.open(path!);
    final enabled = lib.lookupFunction<Int32 Function(Pointer<Utf8>), int Function(Pointer<Utf8>)>('fake_tool_enabled');
    final emitState = lib.lookupFunction<Void Function(Int32, Uint64, Pointer<Utf8>), void Function(int, int, Pointer<Utf8>)>(
        'fake_emit_state');
    final stringFree = lib.lookupFunction<Void Function(Pointer<Utf8>), void Function(Pointer<Utf8>)>('am_string_free');
    String? text(String symbol, String name) {
      final f = lib.lookupFunction<Pointer<Utf8> Function(Pointer<Utf8>), Pointer<Utf8> Function(Pointer<Utf8>)>(symbol);
      final p = using((a) => f(name.toNativeUtf8(allocator: a)));
      if (p == nullptr) return null;
      final s = p.toDartString();
      stringFree(p);
      return s;
    }

    int on(String name) => using((a) => enabled(name.toNativeUtf8(allocator: a)));

    final client = AppMcp(appId: 'shop', appName: '商店', libraryPath: path);
    addTearDown(client.dispose);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    final fallback = McpUiFallback.enable(client);
    addTearDown(fallback.dispose);
    const names = ['ui.outline', 'ui.click', 'ui.fill', 'ui.press', 'ui.scroll', 'ui.read'];
    expect([for (final n in names) on(n)], everyElement(1));
    expect(text('fake_tool_view', 'ui.click'), '1|null');
    expect(text('fake_tool_options', 'ui.outline'), startsWith('{"title":"界面控件大纲","readOnlyHint":true}'));
    expect(text('fake_tool_options', 'ui.click'), startsWith('{"title":"点击控件","readOnlyHint":false}'));
    expect(text('fake_tool_options', 'ui.read'), contains('"readOnlyHint":true'));

    // 未连接时不开启语义树；连接后开启；进入后台时工具禁用、语义树关闭。
    expect(fallback.semanticsActive, isFalse);
    using((a) => emitState(ConnectionStatus.connected.index, 0, nullptr));
    await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 100)));
    await tester.pump(); // 状态流的订阅在测试的 fake async 区域中投递
    expect(fallback.semanticsActive, isTrue);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    expect(fallback.enabled, isFalse);
    expect([for (final n in names) on(n)], everyElement(0));
    expect(fallback.semanticsActive, isFalse);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    expect([for (final n in names) on(n)], everyElement(1));
    expect(fallback.semanticsActive, isTrue);

    fallback.dispose();
    expect(on('ui.outline'), -1);
  }, skip: path == null);
}

Matcher contains_(String s) => contains(s);
