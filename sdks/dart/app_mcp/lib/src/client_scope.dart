// McpScope 作用域与工具规格转换（client.dart 的 part）。

part of 'client.dart';

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// 作用域：注销时递归注销其下所有工具、资源与子作用域。
final class McpScope {
  McpScope._(this._client, this._ptr, this._parent, {this.isRoot = false});

  final AppMcp _client;
  final Pointer<AmScope> _ptr;
  final McpScope? _parent;
  final bool isRoot;
  bool _disposed = false;

  final Set<ToolHandle> _tools = {};
  final Set<ResourceHandle> _resources = {};
  final Set<McpScope> _children = {};

  AppMcp get client => _client;
  bool get isDisposed => _disposed || _client._disposed;

  /// 注册工具。名称重复、名称或 schema 非法时抛出 [AppMcpException]。
  ToolHandle tool(
    String name, {
    required String description,
    Map<String, Object?>? inputSchema,
    Risk risk = Risk.write,
    Activation? activation,
    String? title,
    bool enabled = true,
    ToolAnnotations? annotations,
    Map<String, Object?>? outputSchema,
    ToolSurface surface = ToolSurface.app,
    String? page,
    String? backgroundTool,
    int concurrency = 0,
    String? exclusive,
    List<String> implements = const [],
    CachePolicy? cache,
    ToolDeprecation? deprecated,
    required ToolHandler handler,
  }) =>
      registerTool(
          ToolSpec(
              name: name,
              description: description,
              inputSchema: inputSchema,
              risk: risk,
              activation: activation,
              title: title,
              enabled: enabled,
              annotations: annotations,
              outputSchema: outputSchema,
              surface: surface,
              page: page,
              backgroundTool: backgroundTool,
              concurrency: concurrency,
              exclusive: exclusive,
              implements: implements,
              cache: cache,
              deprecated: deprecated),
          handler);

  /// 用 [ToolSpec] 注册工具。
  ToolHandle registerTool(ToolSpec spec, ToolHandler handler) {
    _ensureAlive();
    cacheTtlToNative(spec.cache); // @why 登记回调目标之前拒绝无法表达的缓存声明，失败时不留下目标
    final rt = _client._rt;
    final entry = _ToolEntry(_client, handler);
    final id = rt.register(entry);
    final ptr = using((arena) {
      final s = _toolSpec(spec, arena);
      final options = _toolOptions(spec, arena);
      final out = arena<Pointer<AmTool>>();
      final status = rt.b.am_tool_register_ex(_ptr, s, options, rt.tool.nativeFunction,
          Pointer<Void>.fromAddress(id), rt.free.nativeFunction, out);
      if (status != AmStatus.ok) {
        final e = rt.error(status);
        rt.targets.remove(id);
        throw e;
      }
      return out.value;
    });
    _client._ownedIds.add(id);
    final handle = ToolHandle._(this, ptr, entry, spec);
    _tools.add(handle);
    return handle;
  }

  /// 注册资源。
  ///
  /// [realtime]：需实时推送（spec/lifecycle.md 第 13 节 B3）——被订阅时保持连接、休眠中变化时回连推送。
  /// 默认 false：订阅不阻止休眠，变化在下次连接时补发；只用于"模型在等待变化"的资源。
  /// [annotations]：资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上。
  /// [cache]：读取结果缓存声明（spec/protocol.md 3.6），Hub 在 TTL 内复用读取结果；为 null 时不声明。
  ResourceHandle resource(String name,
      {required String description,
      String? mimeType,
      bool realtime = false,
      ContentAnnotations? annotations,
      CachePolicy? cache,
      required ResourceReader read}) {
    _ensureAlive();
    final cacheTtl = cacheTtlToNative(cache);
    final annotationsJson = annotations == null ? null : jsonEncode(annotations.toJson());
    final rt = _client._rt;
    final entry = _ResourceEntry(_client, read);
    final id = rt.register(entry);
    final ptr = using((arena) {
      final s = arena<AmResourceSpec>();
      s.ref
        ..name = name.toNativeUtf8(allocator: arena)
        ..description = description.toNativeUtf8(allocator: arena)
        ..mime_type = _optStr(mimeType, arena);
      final options = arena<AmResourceOptions>();
      options.ref
        ..struct_size = sizeOf<AmResourceOptions>()
        ..realtime = realtime
        ..annotations_json = _optStr(annotationsJson, arena)
        ..cache_ttl_ms = cacheTtl
        ..cache_scope = cacheScopeToNative(cache);
      final out = arena<Pointer<AmResource>>();
      final status = rt.b.am_resource_register_ex(
          _ptr, s, options, rt.read.nativeFunction, Pointer<Void>.fromAddress(id), rt.free.nativeFunction, out);
      if (status != AmStatus.ok) {
        final e = rt.error(status);
        rt.targets.remove(id);
        throw e;
      }
      return out.value;
    });
    _client._ownedIds.add(id);
    final handle = ResourceHandle._(this, ptr, entry, name);
    _resources.add(handle);
    return handle;
  }

  /// 创建子作用域。
  McpScope scope(String name) {
    _ensureAlive();
    final rt = _client._rt;
    final ptr = using((arena) {
      final out = arena<Pointer<AmScope>>();
      rt.check(rt.b.am_scope_create(_ptr, name.toNativeUtf8(allocator: arena), out));
      return out.value;
    });
    final child = McpScope._(_client, ptr, this);
    _children.add(child);
    return child;
  }

  /// 注销该作用域下的全部工具、资源与子作用域并释放句柄。幂等。
  ///
  /// 对根作用域调用会注销全部工具与资源，但根作用域本身仍可继续注册。
  void dispose() {
    if (isDisposed) return;
    final rt = _client._rt;
    rt.b.am_scope_dispose(_ptr);
    if (isRoot) {
      for (final t in _tools.toList()) {
        t._release();
      }
      for (final r in _resources.toList()) {
        r._release();
      }
      for (final c in _children.toList()) {
        c._releaseTree();
      }
      return;
    }
    _releaseTree();
    _parent?._children.remove(this);
  }

  /// 释放本作用域及其后代的原生句柄（不再发送注销）。
  void _releaseTree() {
    if (_disposed) return;
    _disposed = true;
    for (final t in _tools.toList()) {
      t._release();
    }
    for (final r in _resources.toList()) {
      r._release();
    }
    for (final c in _children.toList()) {
      c._releaseTree();
    }
    _tools.clear();
    _resources.clear();
    _children.clear();
    _client._b.am_scope_free(_ptr);
  }

  void _ensureAlive() {
    _client._ensureAlive();
    if (_disposed) throw AppMcpException(AppMcpErrorCode.disposed, '作用域已注销');
  }
}

Pointer<AmToolSpec> _toolSpec(ToolSpec spec, Allocator arena) {
  final s = arena<AmToolSpec>();
  s.ref
    ..name = spec.name.toNativeUtf8(allocator: arena)
    ..description = spec.description.toNativeUtf8(allocator: arena)
    ..input_schema_json = _optStr(encodeSchema(spec.inputSchema), arena)
    ..risk = riskToNative(spec.risk)
    ..activation = activationToNative(spec.activation)
    ..title = _optStr(spec.title, arena)
    ..enabled = spec.enabled;
  return s;
}

/// v9：工具注解与 outputSchema（为 null 的字段不声明 / 清除）。
Pointer<AmToolOptions> _toolOptions(ToolSpec spec, Allocator arena) {
  final o = arena<AmToolOptions>();
  o.ref
    ..struct_size = sizeOf<AmToolOptions>()
    ..annotations_json = _optStr(encodeToolAnnotations(spec.annotations), arena)
    ..output_schema_json = _optStr(encodeSchema(spec.outputSchema), arena)
    ..page = _optStr(spec.page, arena)
    ..surface = surfaceToNative(spec.surface)
    ..background_tool = _optStr(spec.backgroundTool, arena)
    ..concurrency = toolConcurrencyToNative(spec.concurrency)
    ..exclusive = _optStr(spec.exclusive, arena)
    ..implements = _strArray(spec.implements, arena)
    ..implements_len = spec.implements.length
    ..cache_ttl_ms = cacheTtlToNative(spec.cache)
    ..cache_scope = cacheScopeToNative(spec.cache)
    ..deprecated_message = _optStr(spec.deprecated?.message, arena)
    ..deprecated_replacement = _optStr(spec.deprecated?.replacement, arena)
    ..deprecated_until = _optStr(spec.deprecated?.until, arena);
  return o;
}

/// 字符串数组（`const char *const *`，分配在 [arena]）；空时为 NULL。
Pointer<Pointer<Utf8>> _strArray(List<String> items, Allocator arena) {
  if (items.isEmpty) return nullptr;
  final array = arena<Pointer<Utf8>>(items.length);
  for (var i = 0; i < items.length; i++) {
    array[i] = items[i].toNativeUtf8(allocator: arena);
  }
  return array;
}
