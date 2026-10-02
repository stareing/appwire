// 兜底工具的大纲与变化摘要格式：唯一定义见 spec/ui-fallback.md 第 4–7 节，本文件只是 Dart 实现。

/// 条目类别：控件、分组（带引用，可用于 `within`）、标题（只作分组）。
enum UiEntryKind { item, container, heading }

/// 角色 → 中文标签（spec/ui-fallback.md 第 5 节）。
const Map<String, String> uiRoleLabels = {
  'button': '按钮',
  'link': '链接',
  'textbox': '输入框',
  'searchbox': '搜索框',
  'checkbox': '复选框',
  'radio': '单选框',
  'switch': '开关',
  'combobox': '下拉框',
  'listbox': '列表框',
  'option': '选项',
  'slider': '滑块',
  'spinbutton': '数字框',
  'tab': '标签页',
  'menuitem': '菜单项',
  'treeitem': '树节点',
  'gridcell': '单元格',
  'status': '提示',
  'alert': '警告',
  'generic': '元素',
  'window': '窗口',
  'dialog': '对话框',
  'alertdialog': '警告框',
  'navigation': '导航',
  'main': '主区域',
  'form': '表单',
  'search': '搜索区',
  'group': '分组',
  'list': '列表',
  'scrollable': '滚动区',
  'heading': '标题',
};

/// 密码类控件的标签与固定掩码（spec/ui-fallback.md 8.1：不泄露长度）。
const String uiSecureLabel = '密码框';
const String uiSecureMask = '••••';

const int uiNameMax = 40;
const int uiStatusNameMax = 80;
const int uiValueMax = 30;
const int uiMaxChanges = 15;

/// 折叠空白。
String uiCollapse(String s) => s.replaceAll(RegExp(r'\s+'), ' ').trim();

/// 截断到 [max] 字（保留前 max − 1 字、去掉尾部空白再加 `…`）。
String uiTruncate(String s, int max) {
  final chars = s.runes.toList();
  if (chars.length <= max) return s;
  return '${String.fromCharCodes(chars.take(max - 1)).trimRight()}…';
}

/// 大纲中的一个条目（控件 / 分组 / 标题）。
final class UiEntry {
  UiEntry({
    required this.kind,
    required this.key,
    required this.role,
    required this.label,
    this.ref,
    this.name = '',
    this.value,
    this.states = const [],
    this.required = false,
    this.declared,
    this.level,
    this.depth = 0,
    this.chain = const [],
    this.containers = const [],
  });

  final UiEntryKind kind;

  /// 前后两次快照中对应同一控件的键（控件与分组为引用，标题为节点标识）。
  final String key;
  final String role;
  final String label;
  final String? ref;
  final String name;
  final String? value;
  final List<String> states;
  final bool required;
  String? declared;
  final int? level;

  /// 所在分组层数（缩进）。
  final int depth;

  /// 祖先分组（分组与标题）在条目数组中的下标，外层在前。
  final List<int> chain;

  /// 祖先分组的键，外层在前。
  final List<String> containers;

  /// `按钮「结算」`。
  String get described => name.isEmpty ? label : '$label「$name」';
}

/// `ui.outline` 的结果（spec/ui-fallback.md 4.1）。
final class UiOutline {
  UiOutline(this.text, this.items, this.total, {this.remaining, this.hint});

  final String text;
  final List<Map<String, Object?>> items;
  final int total;
  final int? remaining;
  final String? hint;

  Map<String, Object?> toJson() => {
        'text': text,
        'items': items,
        'total': total,
        if (remaining != null) 'remaining': remaining,
        if (hint != null) 'hint': hint,
      };
}

const String _indent = '  ';

String _groupLabel(UiEntry e) => e.kind == UiEntryKind.heading ? e.name : e.described;

String _haystack(UiEntry e, List<UiEntry> entries) => [
      e.label,
      e.role,
      e.name,
      e.value ?? '',
      e.declared ?? '',
      for (final i in e.chain) _groupLabel(entries[i]),
    ].join(' ').toLowerCase();

String _itemLine(UiEntry e) {
  final b = StringBuffer('${e.ref} ${e.described}');
  if (e.value != null) b.write('= "${e.value}"');
  if (e.states.isNotEmpty) b.write(' ${e.states.join(' ')}');
  if (e.required) b.write(' (必填)');
  if (e.declared != null) b.write(' [已声明：${e.declared}]');
  return b.toString();
}

String _containerLine(UiEntry e) =>
    '» ${e.ref} ${e.described}${e.declared != null ? ' [已声明：${e.declared}]' : ''}';

String _headingLine(UiEntry e) => '${'#' * (e.level ?? 2)} ${e.name}';

/// 过滤、截断、分组并渲染大纲（spec/ui-fallback.md 4.2 / 4.3）。
UiOutline renderUiOutline(List<UiEntry> entries, {String? query, required int limit}) {
  final tokens = (query ?? '').toLowerCase().split(RegExp(r'\s+')).where((t) => t.isNotEmpty).toList();
  final matched = <int>[];
  for (var i = 0; i < entries.length; i++) {
    final e = entries[i];
    if (e.kind != UiEntryKind.item) continue;
    if (tokens.isNotEmpty) {
      final h = _haystack(e, entries);
      if (!tokens.every(h.contains)) continue;
    }
    matched.add(i);
  }
  final shown = matched.take(limit).toSet();
  final keptGroups = {for (final i in shown) ...entries[i].chain};

  final lines = <String>[];
  final items = <Map<String, Object?>>[];
  var declared = false;
  for (var i = 0; i < entries.length; i++) {
    final e = entries[i];
    final indent = _indent * e.depth;
    if (e.kind == UiEntryKind.item) {
      if (!shown.contains(i)) continue;
      lines.add(indent + _itemLine(e));
      final group = e.chain.map((g) => _groupLabel(entries[g])).join(' › ');
      if (e.declared != null) declared = true;
      items.add({
        'ref': e.ref,
        'role': e.role,
        'name': e.name,
        if (e.value != null) 'value': e.value,
        if (e.states.isNotEmpty) 'states': e.states,
        if (e.required) 'required': true,
        if (e.declared != null) 'declared': e.declared,
        if (group.isNotEmpty) 'group': group,
      });
    } else if (keptGroups.contains(i)) {
      if (e.kind == UiEntryKind.container) {
        lines.add(indent + _containerLine(e));
        if (e.declared != null) declared = true;
      } else {
        lines.add(indent + _headingLine(e));
      }
    }
  }
  final remaining = matched.length - shown.length;
  if (lines.isEmpty) {
    lines.add(tokens.isNotEmpty ? '（没有与「$query」匹配的可交互元素）' : '（没有可见的可交互元素）');
  }
  if (remaining > 0) lines.add('…另有 $remaining 个元素未列出，可用 query 或 within 缩小范围');
  return UiOutline(lines.join('\n'), items, matched.length,
      remaining: remaining > 0 ? remaining : null,
      hint: declared ? '标注 [已声明：…] 的元素已有对应工具，请优先直接调用该工具' : null);
}

const Set<String> _ignoredStates = {'focused'};
const List<List<String>> _exclusiveStates = [
  ['checked', 'unchecked', 'mixed'],
  ['expanded', 'collapsed'],
];

List<String> _stateChange(List<String> before, List<String> after) {
  final b = before.where((s) => !_ignoredStates.contains(s)).toList();
  final a = after.where((s) => !_ignoredStates.contains(s)).toList();
  final added = a.where((s) => !b.contains(s)).toList();
  final removed = b
      .where((s) => !a.contains(s))
      .where((r) => !_exclusiveStates.any((g) => g.contains(r) && added.any(g.contains)))
      .toList();
  return [
    if (added.isNotEmpty) '变为 ${added.join('、')}',
    if (removed.isNotEmpty) '不再 ${removed.join('、')}',
  ];
}

String _visibleStates(UiEntry e) {
  final s = e.states.where((x) => !_ignoredStates.contains(x));
  return s.isEmpty ? '' : ' ${s.join(' ')}';
}

String _changeLabel(UiEntry e) => e.kind == UiEntryKind.heading ? '标题「${e.name}」' : '${e.described}(${e.ref})';

/// 外层最先出现的"同样是新增 / 同样被移除"的祖先分组。
String? _outerChanged(UiEntry e, Map<String, UiEntry> set, Map<String, UiEntry> other) {
  for (final c in e.containers) {
    if (set.containsKey(c) && !other.containsKey(c)) return c;
  }
  return null;
}

/// 操作前后的变化摘要（spec/ui-fallback.md 7.2）。
List<String> diffUiEntries(List<UiEntry> before, List<UiEntry> after, {required String prefix}) {
  final byBefore = {for (final e in before) e.key: e};
  final byAfter = {for (final e in after) e.key: e};
  final out = <String>[];

  Map<String, int> childCounts(List<UiEntry> list, Map<String, UiEntry> set, Map<String, UiEntry> other) {
    final counts = <String, int>{};
    for (final e in list) {
      if (other.containsKey(e.key)) continue;
      final outer = _outerChanged(e, set, other);
      if (outer != null) counts[outer] = (counts[outer] ?? 0) + (e.kind == UiEntryKind.item ? 1 : 0);
    }
    return counts;
  }

  final addedChildren = childCounts(after, byAfter, byBefore);
  final removedChildren = childCounts(before, byBefore, byAfter);

  for (final e in after) {
    final old = byBefore[e.key];
    if (old == null) {
      if (_outerChanged(e, byAfter, byBefore) != null) continue;
      final b = StringBuffer('新增${_changeLabel(e)}');
      if (e.kind == UiEntryKind.item) {
        if (e.value != null) b.write(' = "${e.value}"');
        b.write(_visibleStates(e));
      }
      final n = addedChildren[e.key];
      if (n != null && n > 0) b.write('，含 $n 个可交互元素（可用 $prefix.outline({ within: "${e.ref}" }) 查看）');
      out.add(b.toString());
      continue;
    }
    if (e.kind == UiEntryKind.heading) {
      if (old.name != e.name) out.add('标题「${old.name}」变为「${e.name}」');
      continue;
    }
    final parts = <String>[
      if (old.name != e.name) '名称变为「${e.name}」',
      if (old.value != e.value) e.value == null ? '值已清空' : '值变为 "${e.value}"',
      ..._stateChange(old.states, e.states),
    ];
    if (parts.isNotEmpty) out.add('${e.ref} ${old.name.isEmpty ? old.label : old.name} ${parts.join('，')}');
  }

  for (final e in before) {
    if (byAfter.containsKey(e.key)) continue;
    if (_outerChanged(e, byBefore, byAfter) != null) continue;
    final n = removedChildren[e.key];
    out.add('${_changeLabel(e)} 已消失${n != null && n > 0 ? '（含 $n 个可交互元素）' : ''}');
  }

  if (out.length > uiMaxChanges) {
    final rest = out.length - uiMaxChanges;
    out
      ..length = uiMaxChanges
      ..add('…另有 $rest 项变化，请调用 $prefix.outline 查看');
  }
  return out;
}
