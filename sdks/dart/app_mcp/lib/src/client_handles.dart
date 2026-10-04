// ToolHandle / ResourceHandle 句柄（client.dart 的 part）。

part of 'client.dart';

// ---------------------------------------------------------------------------
// 句柄
// ---------------------------------------------------------------------------

/// [AppMcp.beginBusy] 返回的用户正在操作作用域：[release] 归还一次计数（重复调用无效果）。
final class McpBusyHold {
  McpBusyHold._(this._client);
  final AppMcp _client;
  bool _released = false;

  bool get isReleased => _released;

  /// 结束作用域。客户端已释放时只记账、不调用原生库。
  void release() {
    if (_released) return;
    _released = true;
    _client._busyScopes--;
    _client._pushBusy();
  }
}

/// [ToolHandle.update] 的缺省标记类型（区分“未提供”与 null）。
final class _KeepField {
  const _KeepField();
}

/// 已注册的工具。
final class ToolHandle {
  ToolHandle._(this._scope, this._ptr, this._entry, this._spec);

  final McpScope _scope;
  final Pointer<AmTool> _ptr;
  final _ToolEntry _entry;
  ToolSpec _spec;
  bool _released = false;

  String get name => _spec.name;
  ToolSpec get spec => _spec;
  bool get isDisposed => _released || _scope._client._disposed;

  /// 更新定义：未提供的字段保持不变；显式传 null 清除该声明（与 @app-mcp/web 一致）。
  ///
  /// 各参数类型同 [ToolSpec] 对应字段（`description` String、`inputSchema` / `outputSchema`
  /// `Map<String, Object?>`、`risk` [Risk]、`activation` [Activation]、`title` String、`enabled` bool、
  /// `annotations` [ToolAnnotations]、`surface` [ToolSurface]、`page` / `backgroundTool` / `exclusive` String、`concurrency` int、`implements` `List<String>`、`cache` [CachePolicy]、`deprecated` [ToolDeprecation]）。null 的含义：`title` / `activation` /
  /// `annotations` / `outputSchema` / `page` / `backgroundTool` / `exclusive` 清除声明，`concurrency` 恢复 0（不单独限制），`implements` / `cache` / `deprecated` 清除声明，`surface` 恢复 [ToolSurface.app]，`inputSchema` 为无参数，`risk` 恢复 [Risk.write]，`enabled` 恢复 true，
  /// `description` 保持不变。
  ///
  /// @error 类型不符时抛 [ArgumentError]，不产生协议消息。
  /// @compat 参数声明为 `Object?` 以区分“未提供”与 null；原有按类型传值的调用不受影响。
  void update({
    Object? description = _keep,
    Object? inputSchema = _keep,
    Object? risk = _keep,
    Object? activation = _keep,
    Object? title = _keep,
    Object? enabled = _keep,
    Object? annotations = _keep,
    Object? outputSchema = _keep,
    Object? surface = _keep,
    Object? page = _keep,
    Object? backgroundTool = _keep,
    Object? concurrency = _keep,
    Object? exclusive = _keep,
    Object? implements = _keep,
    Object? cache = _keep,
    Object? deprecated = _keep,
  }) {
    final s = _spec;
    replace(ToolSpec(
        name: s.name,
        description: _patch<String?>(description, s.description, 'description') ?? s.description,
        inputSchema: _patch<Map<String, Object?>?>(inputSchema, s.inputSchema, 'inputSchema'),
        risk: _patch<Risk?>(risk, s.risk, 'risk') ?? Risk.write,
        activation: _patch<Activation?>(activation, s.activation, 'activation'),
        title: _patch<String?>(title, s.title, 'title'),
        enabled: _patch<bool?>(enabled, s.enabled, 'enabled') ?? true,
        annotations: _patch<ToolAnnotations?>(annotations, s.annotations, 'annotations'),
        outputSchema: _patch<Map<String, Object?>?>(outputSchema, s.outputSchema, 'outputSchema'),
        surface: _patch<ToolSurface?>(surface, s.surface, 'surface') ?? ToolSurface.app,
        page: _patch<String?>(page, s.page, 'page'),
        backgroundTool: _patch<String?>(backgroundTool, s.backgroundTool, 'backgroundTool'),
        concurrency: _patch<int?>(concurrency, s.concurrency, 'concurrency') ?? 0,
        exclusive: _patch<String?>(exclusive, s.exclusive, 'exclusive'),
        implements: _patch<List<String>?>(implements, s.implements, 'implements') ?? const [],
        cache: _patch<CachePolicy?>(cache, s.cache, 'cache'),
        deprecated: _patch<ToolDeprecation?>(deprecated, s.deprecated, 'deprecated')));
  }

  /// [update] 参数缺省标记。
  static const Object _keep = _KeepField();

  /// @output 未提供（[_keep]）时为 [current]，否则为 [value]（null 或 [T]）。
  /// @error [value] 不是 [T] 时抛 [ArgumentError]。
  static T _patch<T>(Object? value, T current, String field) {
    if (identical(value, _keep)) return current;
    if (value is T) return value;
    throw ArgumentError.value(value, field, '类型应为 $T');
  }

  /// 用新定义整体替换（名称不可变，`spec.name` 被忽略；为 null 的 annotations / outputSchema 表示清除该声明）。
  /// 与当前定义相同时不做任何事。
  void replace(ToolSpec spec) {
    _ensureAlive();
    final next = ToolSpec(
        name: _spec.name,
        description: spec.description,
        inputSchema: spec.inputSchema,
        risk: spec.risk,
        activation: spec.activation,
        title: spec.title,
        enabled: spec.enabled,
        annotations: spec.annotations,
        outputSchema: spec.outputSchema,
        surface: spec.surface,
        page: spec.page,
        backgroundTool: spec.backgroundTool,
        concurrency: spec.concurrency,
        exclusive: spec.exclusive,
        implements: spec.implements,
        cache: spec.cache,
        deprecated: spec.deprecated);
    if (next == _spec) return;
    final rt = _scope._client._rt;
    using((arena) =>
        rt.check(rt.b.am_tool_update_ex(_ptr, _toolSpec(next, arena), _toolOptions(next, arena))));
    _spec = next;
  }

  void setEnabled(bool enabled) {
    _ensureAlive();
    if (enabled == _spec.enabled) return;
    final rt = _scope._client._rt;
    rt.check(rt.b.am_tool_set_enabled(_ptr, enabled));
    _spec = _spec.copyWith(enabled: enabled);
  }

  /// 替换 handler（不产生协议消息，供框架适配在重建时刷新闭包）。
  void setHandler(ToolHandler handler) {
    _entry.handler = handler;
  }

  /// 注销工具并释放句柄。幂等。
  void dispose() {
    if (isDisposed) return;
    _scope._client._b.am_tool_dispose(_ptr);
    _release();
  }

  void _release() {
    if (_released) return;
    _released = true;
    _entry.disposed = true;
    _scope._tools.remove(this);
    _scope._client._b.am_tool_free(_ptr);
  }

  void _ensureAlive() {
    if (isDisposed) throw AppMcpException(AppMcpErrorCode.disposed, '工具已注销');
  }
}

/// 已注册的资源。
final class ResourceHandle {
  ResourceHandle._(this._scope, this._ptr, this._entry, this.name);

  final McpScope _scope;
  final Pointer<AmResource> _ptr;
  final _ResourceEntry _entry;
  final String name;
  bool _released = false;

  bool get isDisposed => _released || _scope._client._disposed;

  /// 通知 Host 资源内容已变化。
  void notifyChanged() {
    if (isDisposed) throw AppMcpException(AppMcpErrorCode.disposed, '资源已注销');
    final rt = _scope._client._rt;
    rt.check(rt.b.am_resource_notify_changed(_ptr));
  }

  /// 替换读取函数（不产生协议消息）。
  void setReader(ResourceReader read) {
    _entry.reader = read;
  }

  /// 注销资源并释放句柄。幂等。
  void dispose() {
    if (isDisposed) return;
    _scope._client._b.am_resource_dispose(_ptr);
    _release();
  }

  void _release() {
    if (_released) return;
    _released = true;
    _entry.disposed = true;
    _scope._resources.remove(this);
    _scope._client._b.am_resource_free(_ptr);
  }
}
