// 从 Flutter 语义树收集兜底大纲条目（spec/ui-fallback.md 第 4–6 节、8.2 Flutter 列）。
//
// 只用公开的非测试接口：RendererBinding.renderViews → PipelineOwner.semanticsOwner → SemanticsNode。
// 语义树只在有 SemanticsHandle 时存在（由 McpUiFallback 持有）。

import 'dart:ui' show CheckedState, Tristate;

import 'package:flutter/rendering.dart';

import 'format.dart';

/// [McpDeclared] 写入语义 identifier 的前缀：`mcp:<工具名>`。
const String uiDeclaredPrefix = 'mcp:';

/// 一个可见的语义节点及其大纲条目。
final class UiNode {
  UiNode(this.owner, this.node, this.entry, {required this.secure});

  final SemanticsOwner owner;
  final SemanticsNode node;
  final UiEntry entry;

  /// 密码类控件（`obscureText`）：值固定为掩码，拒绝填写与按键。
  final bool secure;

  SemanticsData get data => node.getSemanticsData();
  bool has(SemanticsAction action) => data.hasAction(action);
  bool get enabled => data.flagsCollection.isEnabled != Tristate.isFalse;
  bool get focused => data.flagsCollection.isFocused == Tristate.isTrue;
  bool get isTextField => data.flagsCollection.isTextField;

  void perform(SemanticsAction action, [Object? args]) => owner.performAction(node.id, action, args);
}

/// 一次收集的结果：条目（按树序）与引用 → 节点。
final class UiSnapshot {
  UiSnapshot(this.entries, this.byRef);

  final List<UiEntry> entries;
  final Map<String, UiNode> byRef;
}

final class _RefRecord {
  _RefRecord(SemanticsNode node, this.fingerprint) : node = WeakReference(node);

  WeakReference<SemanticsNode> node;
  String fingerprint;
}

/// 引用 `eN` ↔ 语义节点（spec/ui-fallback.md 第 3 节）：弱引用 + 指纹（角色 + 名称 + 所在分组）。
/// 节点被框架重建时，按指纹在当前界面中唯一匹配到的新节点沿用原引用。
final class UiRefRegistry {
  Expando<String> _byNode = Expando('ui-ref');
  final Map<String, _RefRecord> _byRef = {};
  int _next = 1;

  /// 取得（必要时分配或按指纹沿用）节点的引用。[taken] 为本轮收集已分配的引用，[fingerprintCount] 为本轮各指纹的出现次数。
  /// [verify] 为 false（子树收集，分组路径不完整）时不核对指纹。
  String refOf(SemanticsNode node, String fingerprint, Set<String> taken, Map<String, int> fingerprintCount,
      {bool verify = true}) {
    final existing = _byNode[node];
    if (existing != null) {
      final record = _byRef[existing];
      if (!verify || record == null || record.fingerprint == fingerprint) return existing;
      // @why 框架复用了节点（同类型 widget 移位、改名）：已是另一个控件，旧引用作废，不能让它指向新控件。
      _byRef.remove(existing);
    }
    if ((fingerprintCount[fingerprint] ?? 0) == 1) {
      final orphans = _byRef.entries.where((r) {
        if (taken.contains(r.key) || r.value.fingerprint != fingerprint) return false;
        final old = r.value.node.target;
        return old == null || !old.attached || identical(old, node);
      }).toList();
      if (orphans.length == 1) {
        final ref = orphans.single.key;
        orphans.single.value.node = WeakReference(node);
        _byNode[node] = ref;
        return ref;
      }
    }
    final ref = 'e${_next++}';
    _byNode[node] = ref;
    _byRef[ref] = _RefRecord(node, fingerprint);
    return ref;
  }

  /// 节点已分配的引用（不分配新引用）。
  String? refForNode(SemanticsNode node) => _byNode[node];

  /// 引用对应的节点仍挂在语义树上（但可能不可见）。
  bool attached(String ref) => _byRef[ref]?.node.target?.attached ?? false;

  /// 丢弃已被回收的节点对应的条目。
  void prune() => _byRef.removeWhere((_, r) {
        final n = r.node.target;
        return n == null;
      });

  void clear() {
    _byNode = Expando('ui-ref');
    _byRef.clear();
  }
}

typedef _Info = ({UiEntryKind kind, String role, String label, int? level});

_Info _item(String role) => (kind: UiEntryKind.item, role: role, label: uiRoleLabels[role] ?? '元素', level: null);
_Info _container(String role) =>
    (kind: UiEntryKind.container, role: role, label: uiRoleLabels[role] ?? '分组', level: null);

/// [SemanticsRole] → 角色（只列有对应关系的）。
const Map<SemanticsRole, String> _roleTable = {
  SemanticsRole.tab: 'tab',
  SemanticsRole.menuItem: 'menuitem',
  SemanticsRole.menuItemCheckbox: 'menuitem',
  SemanticsRole.menuItemRadio: 'menuitem',
  SemanticsRole.comboBox: 'combobox',
  SemanticsRole.spinButton: 'spinbutton',
  SemanticsRole.cell: 'gridcell',
  SemanticsRole.status: 'status',
  SemanticsRole.alert: 'alert',
};

/// 对话框不在此列：其路由节点（`scopesRoute` + `namesRoute`）已作为对话框分组，避免两层。
const Map<SemanticsRole, String> _containerRoleTable = {
  SemanticsRole.navigation: 'navigation',
  SemanticsRole.main: 'main',
  SemanticsRole.form: 'form',
  SemanticsRole.list: 'list',
};

/// 按语义标志分类：先容器 / 标题，再控件（顺序即优先级），都不是时返回 null（不列出、只遍历子节点）。
_Info? _classify(SemanticsData d) {
  final f = d.flagsCollection;
  final containerRole = _containerRoleTable[d.role];
  if (containerRole != null) return _container(containerRole);
  if (f.scopesRoute && f.namesRoute) return _container('dialog');
  if (f.isHeader) return (kind: UiEntryKind.heading, role: 'heading', label: '标题', level: d.headingLevel.clamp(1, 3));
  final rules = <(bool Function(), String)>[
    (() => f.isTextField, 'textbox'),
    (() => f.isSlider, 'slider'),
    (() => f.isChecked != CheckedState.none && f.isInMutuallyExclusiveGroup, 'radio'),
    (() => f.isChecked != CheckedState.none, 'checkbox'),
    (() => f.isToggled != Tristate.none, 'switch'),
    (() => f.isLink, 'link'),
    (() => _roleTable.containsKey(d.role), _roleTable[d.role] ?? 'generic'),
    (() => f.isButton, 'button'),
    (() => f.isLiveRegion && d.label.isNotEmpty, 'status'),
    (() => d.hasAction(SemanticsAction.tap), 'generic'),
  ];
  for (final (applies, role) in rules) {
    if (applies()) return _item(role);
  }
  if (f.hasImplicitScrolling) return _container('scrollable');
  return null;
}

String _name(SemanticsData d, String role) {
  final raw = [d.label, d.tooltip, d.hint].map(uiCollapse).firstWhere((s) => s.isNotEmpty, orElse: () => '');
  return uiTruncate(raw, role == 'status' || role == 'alert' ? uiStatusNameMax : uiNameMax);
}

String? _value(SemanticsData d, {required bool secure}) {
  if (secure) return d.value.isEmpty ? null : uiSecureMask;
  final v = uiCollapse(d.value);
  return v.isEmpty ? null : uiTruncate(v, uiValueMax);
}

List<String> _states(SemanticsData d) {
  final f = d.flagsCollection;
  final checked = f.isChecked != CheckedState.none ? f.isChecked : _fromToggle(f.isToggled);
  return [
    if (f.isEnabled == Tristate.isFalse) 'disabled',
    if (checked == CheckedState.isTrue) 'checked',
    if (checked == CheckedState.isFalse) 'unchecked',
    if (checked == CheckedState.mixed) 'mixed',
    if (f.isExpanded == Tristate.isTrue) 'expanded',
    if (f.isExpanded == Tristate.isFalse) 'collapsed',
    if (f.isSelected == Tristate.isTrue) 'selected',
    if (f.isTextField && f.isReadOnly) 'readonly',
    if (d.validationResult == SemanticsValidationResult.invalid) 'invalid',
    if (f.isFocused == Tristate.isTrue) 'focused',
  ];
}

CheckedState _fromToggle(Tristate t) => switch (t) {
      Tristate.isTrue => CheckedState.isTrue,
      Tristate.isFalse => CheckedState.isFalse,
      Tristate.none => CheckedState.none,
    };

String? _declaredOf(SemanticsData d) =>
    d.identifier.startsWith(uiDeclaredPrefix) ? d.identifier.substring(uiDeclaredPrefix.length) : null;

/// 节点（连同子树）是否不可见：零尺寸 / 变换不可逆、屏外缓存（滚动区外）、已合并进父节点。
bool uiNodeHidden(SemanticsNode n) =>
    n.isInvisible || n.isMergedIntoParent || n.getSemanticsData().flagsCollection.isHidden;

/// 全部视图的语义树根。没有语义树（未持有 SemanticsHandle）时为空。
Iterable<(SemanticsOwner, SemanticsNode)> uiSemanticsRoots() sync* {
  for (final view in RendererBinding.instance.renderViews) {
    final owner = view.owner?.semanticsOwner;
    final root = owner?.rootSemanticsNode;
    if (owner != null && root != null) yield (owner, root);
  }
}

List<SemanticsNode> _children(SemanticsNode n) {
  final out = <SemanticsNode>[];
  n.visitChildren((c) {
    out.add(c);
    return true;
  });
  return out;
}

final class _Frame {
  _Frame(this.container);
  final int container;
  final List<({int level, int index})> headings = [];
}

/// 收集 [start]（缺省为全部视图）下可见的条目，并为控件与分组分配引用。
/// [full] 为 true 时（从全部视图收集）更新引用的指纹；子树收集时分组路径不完整，不更新。
UiSnapshot collectUiEntries(UiRefRegistry refs, {(SemanticsOwner, SemanticsNode)? start}) {
  final full = start == null;
  final roots = start == null ? uiSemanticsRoots().toList() : [start];

  // 第一遍：收集可见节点、分类与分组路径（用于指纹），第二遍再分配引用。
  final raw = <({SemanticsOwner owner, SemanticsNode node, _Info info, String name, String path, int depth, List<int> chain, List<int> containers, bool secure})>[];
  final declaredGroups = <({String tool, int from, int to})>[];
  final frames = <_Frame>[_Frame(-1)];
  final containerStack = <int>[];

  List<int> chain() => [
        for (final f in frames) ...[if (f.container >= 0) f.container, for (final h in f.headings) h.index],
      ];
  String path(List<int> c) => c.map((i) => raw[i].info.kind == UiEntryKind.heading ? raw[i].name : '${raw[i].info.label}「${raw[i].name}」').join(' › ');

  void walk(SemanticsOwner owner, SemanticsNode n) {
    if (uiNodeHidden(n)) return;
    final d = n.getSemanticsData();
    final info = _classify(d);
    var pushed = false;
    final groupDeclared = info == null ? _declaredOf(d) : null;
    final declaredFrom = raw.length;
    if (info != null) {
      final secure = d.flagsCollection.isObscured;
      final name = info.kind == UiEntryKind.heading ? uiTruncate(uiCollapse(d.label), uiNameMax) : _name(d, info.role);
      final c = chain();
      if (info.kind == UiEntryKind.heading) {
        if (name.isNotEmpty) {
          final frame = frames.last;
          final level = info.level ?? 2;
          while (frame.headings.isNotEmpty && frame.headings.last.level >= level) {
            frame.headings.removeLast();
          }
          final hc = chain();
          raw.add((owner: owner, node: n, info: info, name: name, path: path(hc), depth: frames.length - 1, chain: hc, containers: List.of(containerStack), secure: false));
          frame.headings.add((level: level, index: raw.length - 1));
        }
      } else {
        raw.add((owner: owner, node: n, info: info, name: name, path: path(c), depth: frames.length - 1, chain: c, containers: List.of(containerStack), secure: secure));
        if (info.kind == UiEntryKind.container) {
          frames.add(_Frame(raw.length - 1));
          containerStack.add(raw.length - 1);
          pushed = true;
        }
      }
    }
    // 控件的子节点（文本框内的文字等）不再单独列出。
    if (info == null || info.kind != UiEntryKind.item) {
      for (final c in _children(n)) {
        walk(owner, c);
      }
    }
    if (pushed) {
      frames.removeLast();
      containerStack.removeLast();
    }
    if (groupDeclared != null) declaredGroups.add((tool: groupDeclared, from: declaredFrom, to: raw.length));
  }

  for (final (owner, root) in roots) {
    walk(owner, root);
  }

  // 第二遍：指纹计数 → 分配引用 → 条目。
  String fingerprintOf(int i) => '${raw[i].info.role}|${raw[i].name}|${raw[i].path}';
  final counts = <String, int>{};
  for (var i = 0; i < raw.length; i++) {
    if (raw[i].info.kind != UiEntryKind.heading) counts.update(fingerprintOf(i), (v) => v + 1, ifAbsent: () => 1);
  }
  final taken = <String>{};
  final refsByIndex = <int, String>{};
  for (var i = 0; i < raw.length; i++) {
    if (raw[i].info.kind == UiEntryKind.heading) continue;
    // 子树收集时不按指纹沿用（分组路径不完整）。
    final ref = refs.refOf(raw[i].node, fingerprintOf(i), taken, full ? counts : const {}, verify: full);
    taken.add(ref);
    refsByIndex[i] = ref;
  }
  String keyOf(int i) => refsByIndex[i] ?? 'h${raw[i].node.id}';

  final entries = <UiEntry>[];
  final byRef = <String, UiNode>{};
  for (var i = 0; i < raw.length; i++) {
    final r = raw[i];
    final d = r.node.getSemanticsData();
    final isItem = r.info.kind == UiEntryKind.item;
    final entry = UiEntry(
      kind: r.info.kind,
      key: keyOf(i),
      role: r.info.role,
      label: r.secure ? uiSecureLabel : r.info.label,
      ref: refsByIndex[i],
      name: r.name,
      value: isItem ? _value(d, secure: r.secure) : null,
      states: isItem ? _states(d) : const [],
      required: isItem && d.flagsCollection.isRequired == Tristate.isTrue,
      declared: r.info.kind == UiEntryKind.heading ? null : _declaredOf(d),
      level: r.info.level,
      depth: r.depth,
      chain: r.chain,
      containers: [for (final c in r.containers) keyOf(c)],
    );
    entries.add(entry);
    final ref = refsByIndex[i];
    if (ref != null) byRef[ref] = UiNode(r.owner, r.node, entry, secure: r.secure);
  }
  // 非控件节点上的 `mcp:<工具名>`（如按钮外层的 Semantics）：子树中恰好一个控件时标给它。
  for (final g in declaredGroups) {
    final items = [for (var i = g.from; i < g.to; i++) if (entries[i].kind == UiEntryKind.item) entries[i]];
    if (items.length == 1) items.single.declared ??= g.tool;
  }
  refs.prune();
  return UiSnapshot(entries, byRef);
}
