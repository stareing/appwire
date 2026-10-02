// 兜底工具的执行引擎：大纲、点击、填写、按键、滚动、读取（spec/ui-fallback.md 第 2–7 节，8.2 Flutter 列）。
// 与客户端无关，便于在 widget 测试中直接驱动；工具注册、门控与语义树句柄见 fallback.dart。

import 'dart:async';

import 'package:app_mcp/app_mcp.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart' show TextInputAction;
import 'package:flutter/widgets.dart';

import '../view.dart' show mcpNextFrame;
import 'format.dart';
import 'semantics_tree.dart';

const int uiReadDefault = 1000;
const int uiReadMax = 20000;
const int uiLimitMax = 500;

/// 把内容滚到可见时最多翻几页。
const int _scrollIntoViewMaxPages = 20;

/// 查看方向 → 语义滚动动作（spec/ui-fallback.md 第 2 节：`down` = 向下翻看更多内容 = 内容上移）。
const Map<String, SemanticsAction> uiScrollActions = {
  'down': SemanticsAction.scrollUp,
  'up': SemanticsAction.scrollDown,
  'right': SemanticsAction.scrollLeft,
  'left': SemanticsAction.scrollRight,
};

enum _Key { enter, escape, tab, shiftTab, space }

/// 支持的按键（spec/ui-fallback.md 2.1）。
const Map<String, _Key> _keys = {
  'enter': _Key.enter,
  'return': _Key.enter,
  'escape': _Key.escape,
  'esc': _Key.escape,
  'tab': _Key.tab,
  'shift+tab': _Key.shiftTab,
  ' ': _Key.space,
  'space': _Key.space,
};

/// 兜底工具的执行引擎。所有方法都在主 isolate（UI 线程）上调用；需要语义树已开启（`SemanticsHandle`）。
class McpUiInspector {
  McpUiInspector({
    this.prefix = 'ui',
    this.maxItems = 60,
    Future<void> Function()? frame,
    this.settleDelay = const Duration(milliseconds: 50),
  }) : _frame = frame ?? mcpNextFrame;

  /// 工具名前缀（用于提示文字）。
  final String prefix;

  /// `outline` 缺省列出的控件数。
  final int maxItems;

  /// 等待下一帧（widget 测试中传入 `tester.pump`）。
  final Future<void> Function() _frame;

  /// 操作后在一帧之外再等的时长。
  final Duration settleDelay;

  final UiRefRegistry _refs = UiRefRegistry();

  /// 丢弃全部引用（之后旧引用一律失效）。
  void clear() => _refs.clear();

  UiSnapshot _snapshot() => collectUiEntries(_refs);

  ToolCallError _error(String message, {String? ref, String? reason}) => ToolCallError(ErrorKind.invalidInput, message,
      details: {if (ref != null) 'ref': ref, if (reason != null) 'reason': reason});

  /// 按引用在当前界面中取控件；失效 / 不可见时抛出（spec/ui-fallback.md 7.1）。
  UiNode _resolve(String ref, UiSnapshot snap) {
    final node = snap.byRef[ref];
    if (node != null) return node;
    if (_refs.attached(ref)) {
      throw _error('引用 $ref 对应的控件当前不可见，请重新调用 $prefix.outline', ref: ref, reason: 'hidden');
    }
    throw _error('引用 $ref 已失效，请重新调用 $prefix.outline', ref: ref);
  }

  String _label(UiNode n) => '${n.entry.ref} ${n.entry.described}';

  /// 可操作：仍在界面上、可见、启用。
  UiNode _actionable(String ref, UiSnapshot snap) {
    final n = _resolve(ref, snap);
    if (!n.enabled) throw _error('${_label(n)}已禁用，当前无法操作', ref: ref, reason: 'TOOL_DISABLED');
    return n;
  }

  Never _unsupported(UiNode n, [String? why]) =>
      throw _error('${_label(n)}不支持该操作${why == null ? '' : '：$why'}', ref: n.entry.ref, reason: 'unsupported');

  void _refuseSecure(UiNode n) {
    if (n.secure) throw _error('${_label(n)}是密码类控件，兜底工具不填写', ref: n.entry.ref, reason: 'secure');
  }

  Future<void> _settle() async {
    await _frame();
    if (settleDelay > Duration.zero) await Future<void>.delayed(settleDelay);
  }

  Future<Map<String, Object?>> _act(UiSnapshot before, UiNode? target, Future<void> Function() run) async {
    await run();
    await _settle();
    final after = _snapshot();
    final declared = target?.entry.declared;
    return {
      'ok': true,
      'changes': diffUiEntries(before.entries, after.entries, prefix: prefix),
      if (declared != null) 'hint': '该元素已声明为工具 $declared，下次可直接调用',
    };
  }

  /// `ui.outline`（spec/ui-fallback.md 第 4 节）。
  UiOutline outline({String? query, String? within, int? limit}) {
    final n = (limit ?? maxItems).clamp(1, uiLimitMax);
    if (within == null) return renderUiOutline(_snapshot().entries, query: query, limit: n);
    final scope = _resolve(within, _snapshot());
    final sub = collectUiEntries(_refs, start: (scope.owner, scope.node));
    return renderUiOutline(sub.entries, query: query, limit: n);
  }

  /// `ui.click`：激活控件（语义 tap）。
  Future<Map<String, Object?>> click(String ref) async {
    final before = _snapshot();
    final n = _actionable(ref, before);
    if (!n.has(SemanticsAction.tap)) _unsupported(n);
    return _act(before, n, () async => n.perform(SemanticsAction.tap));
  }

  /// `ui.fill`：文本框写入文本；复选框 / 开关 / 单选框按布尔值切换。
  Future<Map<String, Object?>> fill(String ref, Object? value) async {
    final before = _snapshot();
    final n = _actionable(ref, before);
    _refuseSecure(n);
    if (n.isTextField) {
      final text = switch (value) {
        String s => s,
        num v => v.toString(),
        _ => throw _error('${_label(n)}需要文本值', ref: ref),
      };
      if (n.entry.states.contains('readonly')) _unsupported(n, '只读');
      return _act(before, n, () => _setText(ref, n, text));
    }
    if (const {'checkbox', 'switch', 'radio'}.contains(n.entry.role)) {
      if (value is! bool) throw _error('${_label(n)}需要 true / false', ref: ref);
      final checked = n.entry.states.contains('checked');
      if (checked == value) return _act(before, n, () async {});
      if (n.entry.role == 'radio' && !value) _unsupported(n, '单选框不能直接取消选中，请选中同组的其他项');
      if (!n.has(SemanticsAction.tap)) _unsupported(n);
      return _act(before, n, () async => n.perform(SemanticsAction.tap));
    }
    _unsupported(n, '只能填写文本框、复选框、开关与单选框；下拉框请先 click 展开再 click 选项');
  }

  /// 聚焦、等一帧（文本框聚焦后才接受 setText），再在新的语义树上写入。
  Future<void> _setText(String ref, UiNode n, String text) async {
    if (!n.focused) {
      n.perform(n.has(SemanticsAction.focus) ? SemanticsAction.focus : SemanticsAction.tap);
      await _frame();
    }
    final fresh = _actionable(ref, _snapshot());
    _refuseSecure(fresh);
    if (!fresh.has(SemanticsAction.setText)) _unsupported(fresh, '文本框没有接受输入');
    fresh.perform(SemanticsAction.setText, text);
  }

  /// `ui.press`（spec/ui-fallback.md 2.1）：[ref] 给出时先聚焦该控件。
  Future<Map<String, Object?>> press(String? ref, String key) async {
    final k = _keys[key.trim().toLowerCase()];
    if (k == null) throw _error('不支持的按键「$key」；支持 Enter、Escape、Tab、Shift+Tab、Space');
    final before = _snapshot();
    final target = ref == null ? null : _actionable(ref, before);
    if (target != null) _refuseSecure(target);
    var handled = true;
    final result = await _act(before, target, () async {
      if (target != null && !target.focused && target.has(SemanticsAction.focus)) {
        target.perform(SemanticsAction.focus);
        await _frame();
      }
      if (target == null && _focusedSecure(before)) {
        throw _error('当前焦点在密码类控件上，兜底工具不对其按键', reason: 'secure');
      }
      handled = _applyKey(k, target);
    });
    if (!handled) return {...result, 'hint': '按键 $key 没有被任何控件处理'};
    return result;
  }

  /// 把按键作用到当前焦点（同步执行，焦点上下文不跨越异步间隙）。
  bool _applyKey(_Key k, UiNode? target) {
    final focus = FocusManager.instance.primaryFocus;
    final context = focus?.context;
    return switch (k) {
      _Key.tab => focus?.nextFocus() ?? FocusManager.instance.rootScope.nextFocus(),
      _Key.shiftTab => focus?.previousFocus() ?? FocusManager.instance.rootScope.previousFocus(),
      _Key.escape => _invoke(context, const DismissIntent()),
      _Key.enter => _submitText(context) || _activate(target, context),
      _Key.space => _activate(target, context),
    };
  }

  bool _focusedSecure(UiSnapshot snap) => snap.byRef.values.any((n) => n.focused && n.secure);

  bool _invoke(BuildContext? context, Intent intent) =>
      context != null && context.mounted && Actions.maybeInvoke(context, intent) != null;

  /// 焦点在文本框中：触发其输入动作（完成 / 提交）。
  bool _submitText(BuildContext? context) {
    final editable = context?.findAncestorStateOfType<EditableTextState>();
    if (editable == null) return false;
    editable.performAction(editable.widget.textInputAction ?? TextInputAction.done);
    return true;
  }

  bool _activate(UiNode? target, BuildContext? context) {
    if (target != null && target.has(SemanticsAction.tap)) {
      target.perform(SemanticsAction.tap);
      return true;
    }
    return _invoke(context, const ActivateIntent());
  }

  /// `ui.scroll`：无 [direction] 时把控件滚动到可见，否则滚动控件（或其所在滚动区）一页。
  Future<Map<String, Object?>> scroll(String ref, {String? direction}) async {
    final before = _snapshot();
    if (direction != null) {
      final action = uiScrollActions[direction];
      if (action == null) throw _error('direction 应为 up / down / left / right');
      final n = _resolve(ref, before);
      final scroller = _scrollerFor(n, action);
      if (scroller == null) return {..._noChange(), 'hint': '已到尽头或不可滚动'};
      return _act(before, null, () async => scroller.owner.performAction(scroller.node.id, action));
    }
    final visible = before.byRef[ref];
    if (visible != null) return _noChange();
    if (!_refs.attached(ref)) throw _error('引用 $ref 已失效，请重新调用 $prefix.outline', ref: ref);
    return _act(before, null, () => _scrollIntoView(ref));
  }

  Map<String, Object?> _noChange() => {'ok': true, 'changes': const <String>[]};

  /// 最近的（含自身）具有 [action] 的祖先节点。
  ({SemanticsOwner owner, SemanticsNode node})? _scrollerFor(UiNode n, SemanticsAction action) {
    for (SemanticsNode? cur = n.node; cur != null; cur = cur.parent) {
      if (cur.getSemanticsData().hasAction(action)) return (owner: n.owner, node: cur);
    }
    return null;
  }

  /// 屏外缓存中的控件：按它在滚动区中相对可见控件的位置向前 / 向后翻页，直到可见。
  Future<void> _scrollIntoView(String ref) async {
    for (var page = 0; page < _scrollIntoViewMaxPages; page++) {
      final hidden = _hiddenNode(ref);
      if (hidden == null) return;
      final (owner, node) = hidden;
      final scroller = _hiddenScroller(node);
      if (scroller == null) return;
      final action = _isBeforeVisible(scroller, node) ? SemanticsAction.scrollDown : SemanticsAction.scrollUp;
      if (!scroller.getSemanticsData().hasAction(action)) return;
      owner.performAction(scroller.id, action);
      await _frame();
      if (_snapshot().byRef.containsKey(ref)) return;
    }
  }

  (SemanticsOwner, SemanticsNode)? _hiddenNode(String ref) {
    for (final (owner, root) in uiSemanticsRoots()) {
      SemanticsNode? found;
      void find(SemanticsNode n) {
        if (found != null) return;
        if (_refs.refForNode(n) == ref) {
          found = n;
          return;
        }
        n.visitChildren((c) {
          find(c);
          return found == null;
        });
      }

      find(root);
      if (found != null) return (owner, found!);
    }
    return null;
  }

  SemanticsNode? _hiddenScroller(SemanticsNode n) {
    for (var cur = n.parent; cur != null; cur = cur.parent) {
      if (cur.getSemanticsData().flagsCollection.hasImplicitScrolling) return cur;
    }
    return null;
  }

  /// [target] 在滚动区子节点顺序中是否位于第一个可见子节点之前。
  bool _isBeforeVisible(SemanticsNode scroller, SemanticsNode target) {
    final order = <SemanticsNode>[];
    void walk(SemanticsNode n) {
      order.add(n);
      n.visitChildren((c) {
        walk(c);
        return true;
      });
    }

    walk(scroller);
    final firstVisible = order.indexWhere((n) => !identical(n, scroller) && !uiNodeHidden(n));
    return firstVisible >= 0 && order.indexOf(target) < firstVisible;
  }

  /// `ui.read`：控件（缺省为全部视图）的可见文本，折叠空白；密码类控件只返回掩码。
  Map<String, Object?> read({String? ref, int? maxChars}) {
    final max = (maxChars ?? uiReadDefault).clamp(1, uiReadMax);
    final parts = <String>[];
    void walk(SemanticsNode n) {
      if (uiNodeHidden(n)) return;
      final d = n.getSemanticsData();
      final secure = d.flagsCollection.isObscured;
      for (final s in [d.label, secure ? (d.value.isEmpty ? '' : uiSecureMask) : d.value]) {
        if (s.trim().isNotEmpty) parts.add(s);
      }
      n.visitChildren((c) {
        walk(c);
        return true;
      });
    }

    if (ref != null) {
      walk(_resolve(ref, _snapshot()).node);
    } else {
      for (final (_, root) in uiSemanticsRoots()) {
        walk(root);
      }
    }
    final full = uiCollapse(parts.join(' '));
    final truncated = full.runes.length > max;
    return {'ref': ref ?? 'root', 'text': truncated ? uiTruncate(full, max) : full, 'truncated': truncated};
  }
}
