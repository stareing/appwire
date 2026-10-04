// AppMcp 客户端（client.dart 的 part）。

part of 'client.dart';

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

/// app-mcp 客户端。
///
/// ```dart
/// final client = AppMcp(appId: 'shop', appName: '商店');
/// client.tool('cart.add', description: '加入购物车', inputSchema: {...},
///     handler: (args, ctx) async => {'ok': true});
/// client.start();
/// ```
///
/// 所有方法都应在创建它的 isolate 上调用；handler 也在该 isolate 上执行。
final class AppMcp {
  /// 创建客户端并启动原生后台线程（不连接，调用 [start] 后开始连接）。
  ///
  /// [overview] 为 App 总览，Host 在模型第一次接触该 App 时附带。
  ///
  /// [lifecycle] 缺省时按平台取 [LifecyclePolicy.platformDefault]；显式传入的策略原样使用。
  /// [heartbeat] 为心跳策略（spec/lifecycle.md 第 11 节 A3）。
  /// [callDedup] 为调用去重策略（spec/protocol.md 3.3，默认保留 5 分钟、最多 64 条；[CallDedupPolicy.off] 关闭）。
  /// [navigateInBackground] 见 [setNavigateInBackground]；缺省用平台默认（桌面 true，Android / iOS false）。
  /// [busyPolicy] 为用户正在操作（[setBusy]）期间写调用的处理方式，见 [setBusyPolicy]。
  ///
  /// [registerName] 为 true 时按名寻址（spec/naming.md）：[start] 后在系统名字服务登记，由 Hub 按名拨入
  /// （Linux：D-Bus `dev.appmcp.App.<appId>`；Windows：命名管道 `\\.\pipe\appmcp-<用户 SID>-<appId>`），需先用
  /// `app-mcp-host app install --app-id <appId> --exec <本程序>` 登记。通常与 [LifecycleMode.onDemand] +
  /// [Residency.exitWhenIdle] 同用：由激活启动（命令行带 `--app-mcp-activation`）的进程在通道关闭后收到 [onIdleExit]。
  /// 本平台不支持时经 [logs] 报告，其余照常。[nameInstance] 为登记实例名（`[a-z][a-z0-9-]{0,31}`，不能是 `default`），
  /// 另登记 `dev.appmcp.App.<appId>.<实例>`；不合法时抛出 [AppMcpException]（[AppMcpErrorCode.invalidConfig]）。
  ///
  /// [libraryPath] 指定原生库路径；缺省时读取环境变量 `APP_MCP_NATIVE_PATH`，
  /// 否则按平台默认名加载。失败时抛出 [AppMcpException]。
  factory AppMcp({
    required String appId,
    required String appName,
    String? hostUrl,
    String? appVersion,
    String? instanceId,
    String? instanceTitle,
    String? token,
    String? launchToken,
    ClientKind clientKind = ClientKind.native,
    int maxConcurrentCalls = 1,
    int maxQueuedCalls = 64,
    BusyPolicy busyPolicy = BusyPolicy.reject,
    AppOverview? overview,
    LifecyclePolicy? lifecycle,
    Duration? connectTimeout,
    HeartbeatMode heartbeat = HeartbeatMode.auto,
    CallDedupPolicy callDedup = const CallDedupPolicy(),
    String? libraryPath,
    AppMcpBindings? bindings,
    bool? navigateInBackground,
    bool registerName = false,
    String? nameInstance,
  }) {
    final b = bindings ?? _defaultBindings(libraryPath);
    final rt = _Runtime.of(b);
    final policy = lifecycle ??
        LifecyclePolicy.platformDefault(isAndroid: Platform.isAndroid, isIOS: Platform.isIOS);
    final client = AppMcp._(rt, policy);
    final id = rt.register(client);
    client._userDataId = id;
    try {
      using((arena) {
        final config = arena<AmClientConfig>();
        config.ref
          ..app_id = appId.toNativeUtf8(allocator: arena)
          ..app_name = appName.toNativeUtf8(allocator: arena)
          ..instance_id = _optStr(instanceId, arena)
          ..host_url = _optStr(hostUrl, arena)
          ..app_version = _optStr(appVersion, arena)
          ..instance_title = _optStr(instanceTitle, arena)
          ..token = _optStr(token, arena)
          ..launch_token = _optStr(launchToken, arena)
          ..client_kind = clientKindToNative(clientKind)
          ..max_concurrent_calls = maxConcurrentCalls < 0 ? 0 : maxConcurrentCalls
          ..overview_summary = _optStr(overview?.summary, arena)
          ..overview_body = _optStr(overview?.body, arena)
          ..overview_locale = _optStr(overview?.locale, arena);
        final callbacks = arena<AmClientCallbacks>();
        callbacks.ref
          ..on_state = rt.state.nativeFunction
          ..on_paired = rt.paired.nativeFunction
          ..on_log = rt.log.nativeFunction
          ..user_data = Pointer<Void>.fromAddress(id)
          ..free_user_data = rt.free.nativeFunction;
        final life = arena<AmLifecycle>();
        life.ref
          ..mode = lifecycleModeToNative(policy.mode)
          ..idle_timeout_ms = durationToMs(policy.idleTimeout)
          ..hidden_idle_timeout_ms = durationToMs(policy.hiddenIdleTimeout)
          ..grace_ms = durationToMs(policy.grace)
          ..residency = residencyToNative(policy.residency)
          ..wake_kind = wakeKindToNative(policy.wake?.kind)
          ..wake_target = _optStr(policy.wake?.target, arena)
          ..wake_background = policy.wake?.background ?? false;
        final options = arena<AmClientOptions>();
        final timeoutMs = connectTimeout == null ? 0 : durationToMs(connectTimeout);
        options.ref
          ..struct_size = sizeOf<AmClientOptions>()
          ..lifecycle = life
          ..connect_timeout_ms = timeoutMs > 0xFFFFFFFF ? 0xFFFFFFFF : timeoutMs
          ..on_idle_exit = rt.idleExit.nativeFunction
          ..heartbeat = heartbeatToNative(heartbeat)
          ..host_absent_retries = hostAbsentRetriesToNative(policy.hostAbsentRetries)
          ..legacy_timers = policy.legacyTimers
          ..merge_window_ms = mergeWindowToNative(policy.mergeWindow)
          ..sleep_on_background = policy.sleepOnBackground
          ..call_dedup_ttl_ms = dedupTtlToNative(callDedup.ttl)
          ..call_dedup_max_entries = dedupMaxEntriesToNative(callDedup.maxEntries)
          ..register_name = registerName
          ..name_instance = _optStr(nameInstance, arena)
          ..max_queued_calls = maxQueuedCallsToNative(maxQueuedCalls);
        final out = arena<Pointer<AmClient>>();
        rt.check(b.am_client_new_ex(config, callbacks, options, out));
        client._ptr = out.value;
        final scopeOut = arena<Pointer<AmScope>>();
        // @why AmClientOptions 不含 busy 策略（app_mcp.h v19 结构体不变），创建后立即设置。
        var status = b.am_client_set_busy_policy(client._ptr, busyPolicyToNative(busyPolicy));
        if (status == AmStatus.ok) status = b.am_client_root_scope(client._ptr, scopeOut);
        if (status != AmStatus.ok) {
          final e = rt.error(status);
          b.am_client_free(client._ptr);
          throw e;
        }
        client._root = McpScope._(client, scopeOut.value, null, isRoot: true);
      });
    } catch (_) {
      rt.targets.remove(id);
      rethrow;
    }
    if (navigateInBackground != null) client.setNavigateInBackground(navigateInBackground);
    return client;
  }

  AppMcp._(this._rt, this.lifecycle);

  /// 生命周期策略（创建时确定）。
  final LifecyclePolicy lifecycle;

  static AppMcpBindings? _shared;
  static final Map<String, AppMcpBindings> _byPath = {};

  /// 同一路径只加载一次，保证共享同一组回调（见文件头的生命周期说明）。
  static AppMcpBindings _defaultBindings(String? path) {
    if (path != null) return _byPath.putIfAbsent(path, () => AppMcpBindings.load(path));
    return _shared ??= AppMcpBindings.load();
  }

  final _Runtime _rt;
  AppMcpBindings get _b => _rt.b;
  late final Pointer<AmClient> _ptr;
  late final McpScope _root;
  late final int _userDataId;
  bool _disposed = false;
  // 用户正在操作：显式开关、未结束作用域数、已下发给原生库的值（见 [_pushBusy]）。
  bool _busyManual = false;
  int _busyScopes = 0;
  bool _busyApplied = false;

  final StreamController<McpConnectionState> _states = StreamController<McpConnectionState>.broadcast();
  final StreamController<String> _paired = StreamController<String>.broadcast();
  final StreamController<McpLogRecord> _logs = StreamController<McpLogRecord>.broadcast();
  final StreamController<void> _idleExit = StreamController<void>.broadcast();
  final Set<_PendingCall> _calls = {};
  final Set<_PendingRead> _reads = {};
  final Set<_PendingNavigate> _navigations = {};
  NavigationHandler? _navigationHandler;

  /// 该客户端注册过的所有 user_data ID（dispose 时从注册表删除）。
  final Set<int> _ownedIds = {};

  /// 从激活参数中提取唤醒令牌（不需要客户端）。不是唤醒参数时返回 null。
  static String? parseWakeToken(String args, {String? libraryPath}) {
    final b = _defaultBindings(libraryPath);
    return using((arena) => _takeString(b, b.am_parse_wake_token(args.toNativeUtf8(allocator: arena))));
  }

  /// 原生库版本，如 `0.1.0`。
  static String nativeVersion({String? libraryPath}) =>
      _defaultBindings(libraryPath).am_version().toDartString();

  /// 根作用域。
  McpScope get root => _root;

  bool get isDisposed => _disposed;

  /// 连接状态变化（在创建客户端的 isolate 上投递）。
  Stream<McpConnectionState> get states => _states.stream;

  /// 配对成功并获得新 token；App 应持久化，下次传入 `token`。
  Stream<String> get onPaired => _paired.stream;

  /// 原生库日志（在创建客户端的 isolate 上投递）。没有监听者时丢弃。
  Stream<McpLogRecord> get logs => _logs.stream;

  /// 当前状态（同步查询原生库）。
  McpConnectionState get state {
    _ensureAlive();
    return using((arena) {
      final status = arena<Int32>();
      final retry = arena<Uint64>();
      final reason = arena<Pointer<Utf8>>();
      reason.value = nullptr;
      _rt.check(_b.am_client_state(_ptr, status, retry, reason));
      final r = _takeString(_b, reason.value);
      return stateFromNative(status.value, retry.value, r, code: _queryStateCode());
    });
  }

  /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3），与 Host 日志中的 `cid` 对应；未连接时为 null。
  String? get connectionId {
    _ensureAlive();
    return _takeOut(_b.am_client_connection_id);
  }

  /// 当前状态的错误码（am_client_state_code）。
  String? _queryStateCode() => _takeOut(_b.am_client_state_code);

  /// 调用 `AmStatus f(client, char **out)` 形式的查询，取走输出字符串。
  String? _takeOut(int Function(Pointer<AmClient>, Pointer<Pointer<Utf8>>) f) => using((arena) {
        final out = arena<Pointer<Utf8>>();
        out.value = nullptr;
        _rt.check(f(_ptr, out));
        return _takeString(_b, out.value);
      });

  String get instanceId {
    _ensureAlive();
    return _takeString(_b, _b.am_client_instance_id(_ptr)) ?? '';
  }

  /// 当前 token（配置带入的或配对后获得的）。
  String? get token {
    _ensureAlive();
    return _takeString(_b, _b.am_client_token(_ptr));
  }

  /// 开始连接。重复调用无效果。
  void start() {
    _ensureAlive();
    _rt.check(_b.am_client_start(_ptr));
  }

  /// 停止：取消所有调用、断开连接、不再重连。
  void stop() {
    _ensureAlive();
    _rt.check(_b.am_client_stop(_ptr));
  }

  /// 设置导航回调（Host 的 `app/navigate`，spec/protocol.md 3.4）；null 清除（之后的导航请求以 `NAVIGATION_FAILED` 回复）。
  ///
  /// [handler] 在创建客户端的 isolate（Flutter 主 isolate）上执行：切换到目标页面，最好等新页面的工具注册之后再返回。
  /// 正常返回 = 完成；抛 [NavigationDeniedError] = 拒绝；抛 [UserActionRequiredError] = `USER_ACTION_REQUIRED`
  /// （reason / uri）；其他异常 = 失败。能力在握手时声明：建议在 [start] 之前设置，
  /// 连接后才设置的在下次连接生效。Flutter 可用 `app_mcp_flutter` 的 `McpNavigator` / go_router 适配。
  void setNavigationHandler(NavigationHandler? handler) {
    _ensureAlive();
    _navigationHandler = handler;
    _rt.check(_b.am_client_set_navigation_handler(
        _ptr,
        handler == null ? nullptr : _rt.navigate.nativeFunction,
        Pointer<Void>.fromAddress(handler == null ? 0 : _userDataId),
        nullptr));
  }

  /// App 在后台（[AppVisibility.hidden] / [AppVisibility.frozen]）时是否仍把导航交给导航回调（spec/protocol.md 3.4
  /// 「后台与前台」）。false 时直接以 `USER_ACTION_REQUIRED`（reason `foreground`）回复、不调用回调；true 时回调可自行
  /// 把窗口提到前台，或抛 [UserActionRequiredError]（如发通知后带 reason `foreground` 与 uri）。对之后到达的请求生效。
  void setNavigateInBackground(bool enabled) {
    _ensureAlive();
    _rt.check(_b.am_client_set_navigate_in_background(_ptr, enabled));
  }

  // ---- 用户正在操作（spec/protocol.md 5.3） ----

  /// 显式开关：声明用户正在 / 不再在 App 内操作。期间写调用（生效注解不是 `readOnlyHint: true` 的工具）按 [BusyPolicy]
  /// 拒绝或排队；只读调用与已开始的调用不受影响。何时算"正在操作"由 App 决定，随时生效。
  /// 有效 busy = 本开关 ∨ 未结束的 [beginBusy] 作用域数 > 0：`setBusy(false)` 不结束进行中的作用域。
  /// Flutter 可用 `app_mcp_flutter` 的 `McpBusy` 随 widget 声明。
  void setBusy(bool busy) {
    _ensureAlive();
    _busyManual = busy;
    _pushBusy();
  }

  /// 有效 busy（显式开关 ∨ 未结束的作用域数 > 0）。
  bool get isBusy {
    _ensureAlive();
    return _busyEffective;
  }

  /// 开始一个用户正在操作的作用域：引用计数 +1，[McpBusyHold.release] 时 -1（重复调用无效果）；可嵌套。
  /// 作用域结束不清 [setBusy] 的显式开关；全部作用域结束且开关为 false 时才解除。
  McpBusyHold beginBusy() {
    _ensureAlive();
    _busyScopes++;
    try {
      _pushBusy();
    } catch (_) {
      _busyScopes--;
      rethrow;
    }
    return McpBusyHold._(this);
  }

  bool get _busyEffective => _busyManual || _busyScopes > 0;

  /// @invariant 只在有效值变化时调用 am_client_set_busy；失败时已下发值不变（下次变化重试）。
  void _pushBusy() {
    final effective = _busyEffective;
    if (effective == _busyApplied || _disposed) return;
    _rt.check(_b.am_client_set_busy(_ptr, effective));
    _busyApplied = effective;
  }

  /// 修改用户正在操作期间写调用的处理方式；随即对排队中的调用生效（改为 [BusyPolicy.reject] 时排队的写调用被拒绝）。
  void setBusyPolicy(BusyPolicy policy) {
    _ensureAlive();
    _rt.check(_b.am_client_set_busy_policy(_ptr, busyPolicyToNative(policy)));
  }

  // ---- 事件（spec/protocol.md 3.5） ----

  /// 声明本实例可发出的事件（同名替换）。已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。
  /// SDK 不读清单：要发出的事件都需在运行时声明。[payloadSchema] 为载荷的 JSON Schema（JSON 对象，如 `Map`；描述用，Hub 不校验）。
  ///
  /// 名称不合法（[AppMcpErrorCode.invalidName]）、schema 不是 JSON 对象（[AppMcpErrorCode.invalidSchema]）时抛 [AppMcpException]。
  void declareEvent(String name, String description, {Object? payloadSchema}) {
    _ensureAlive();
    final schema = payloadSchema == null ? null : jsonEncode(payloadSchema);
    _rt.check(using((arena) => _b.am_client_declare_event(_ptr, name.toNativeUtf8(allocator: arena),
        description.toNativeUtf8(allocator: arena), schema?.toNativeUtf8(allocator: arena) ?? nullptr)));
  }

  /// 撤销事件声明；返回是否撤销了已有声明（未声明过为 false）。
  bool removeEvent(String name) {
    _ensureAlive();
    return using((arena) => _rt.flag((out) => _b.am_client_remove_event(_ptr, name.toNativeUtf8(allocator: arena), out)));
  }

  /// 发出已声明的事件；[payload] 须为 JSON 对象（如 `Map`），null = 无载荷。
  /// 已连接时发送并返回 true；未连接（休眠、断线、重连中、握手中）时丢弃并返回 false——不缓存、不为此连接或唤醒 Host，
  /// 也不推迟空闲休眠。需要可靠送达的状态变化请改用资源。
  ///
  /// 未声明 / 名称不合法（[AppMcpErrorCode.invalidName]）、载荷不是对象或超过 8 KiB（[AppMcpErrorCode.invalidJson]）时抛 [AppMcpException]。
  bool emitEvent(String name, [Object? payload]) {
    _ensureAlive();
    final json = payload == null ? null : jsonEncode(payload);
    return using((arena) => _rt.flag((out) => _b.am_client_emit_event(
        _ptr, name.toNativeUtf8(allocator: arena), json?.toNativeUtf8(allocator: arena) ?? nullptr, out)));
  }

  void setVisibility(AppVisibility visibility, {bool focused = true}) {
    _ensureAlive();
    _rt.check(_b.am_client_set_visibility(_ptr, visibilityToNative(visibility), focused));
  }

  // ---- 生命周期（spec/lifecycle.md 第 8 节） ----

  /// 处理操作系统激活参数 / URL：`app-mcp-wake:<token>`（Android extra、Windows 激活参数）、
  /// `<scheme>://app-mcp/wake?token=`（iOS / macOS `onOpenURL`）、`#app-mcp-wake=<token>`。
  /// 不是本 SDK 的唤醒返回 false。可以在 [start] 之前调用（冷启动唤醒）。
  bool handleWake(String args) {
    _ensureAlive();
    return using((arena) => _b.am_client_handle_wake(_ptr, args.toNativeUtf8(allocator: arena)));
  }

  /// App 主动回连（如用户打开了相关界面）。返回是否因此发起了回连。
  bool wake({WakeReason reason = WakeReason.app}) {
    _ensureAlive();
    return _rt.flag((out) => _b.am_client_wake_with_reason(_ptr, wakeReasonToNative(reason), out));
  }

  /// on-demand 模式下主动连接；尚未 [start] 时等同于 [start]。
  bool connectNow() {
    _ensureAlive();
    return _rt.flag((out) => _b.am_client_connect_now(_ptr, out));
  }

  /// App 主动请求休眠（不受空闲条件与持有影响）。返回是否有效果。
  bool sleep({SleepReason reason = SleepReason.app}) {
    _ensureAlive();
    return _rt.flag((out) => _b.am_client_sleep_with_reason(_ptr, sleepReasonToNative(reason), out));
  }

  /// 临时阻止自动休眠，直到返回的持有被释放。
  McpHold hold() {
    _ensureAlive();
    return _rt.hold((out) => _b.am_client_hold(_ptr, out));
  }

  /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
  String get toolsHash {
    _ensureAlive();
    final p = _b.am_client_tools_hash(_ptr);
    if (p == nullptr) throw _rt.error(AmStatus.internal);
    return _takeString(_b, p)!;
  }

  /// 已进入休眠且驻留策略（[Residency]）允许退出进程时触发；App 自行决定是否退出
  /// （例如先 [dispose] 再 `exit(0)`）。
  Stream<void> get onIdleExit => _idleExit.stream;

  /// 在根作用域注册工具。见 [McpScope.tool]。
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
    required ToolHandler handler,
  }) =>
      _root.tool(name,
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
          handler: handler);

  /// 在根作用域注册资源。见 [McpScope.resource]。
  ResourceHandle resource(String name,
          {required String description,
          String? mimeType,
          bool realtime = false,
          ContentAnnotations? annotations,
          CachePolicy? cache,
          required ResourceReader read}) =>
      _root.resource(name,
          description: description,
          mimeType: mimeType,
          realtime: realtime,
          annotations: annotations,
          cache: cache,
          read: read);

  /// 在根作用域下创建子作用域。
  McpScope scope(String name) => _root.scope(name);

  /// 停止并释放客户端。之后所有句柄失效。幂等。
  ///
  /// 会阻塞到原生后台线程结束（通常很快）。仍在执行的 handler 结果将被丢弃。
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _b.am_client_stop(_ptr);
    // 消费所有未完成的调用与读取，避免 AmCall / AmRead 泄漏。
    for (final call in _calls.toList()) {
      if (call.consumed) continue;
      call.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_call_fail(call.ptr, k, m));
      call.onCancel(CancelReason.stopped);
    }
    _calls.clear();
    for (final read in _reads.toList()) {
      if (read.consumed) continue;
      read.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_read_fail(read.ptr, k, m));
    }
    _reads.clear();
    for (final nav in _navigations.toList()) {
      if (nav.consumed) continue;
      nav.consumed = true;
      using((arena) => _b.am_navigate_fail(nav.ptr, '客户端已释放'.toNativeUtf8(allocator: arena)));
    }
    _navigations.clear();
    _root._releaseTree();
    _b.am_client_free(_ptr);
    for (final id in _ownedIds) {
      _rt.targets.remove(id);
    }
    _ownedIds.clear();
    _rt.targets.remove(_userDataId);
    _states.close();
    _paired.close();
    _logs.close();
    _idleExit.close();
  }

  void _ensureAlive() {
    if (_disposed) throw AppMcpException(AppMcpErrorCode.stopped, '客户端已释放');
  }

  // ---- 原生回调（已在本 isolate 上） ----

  void _onNativeState(int status, int retryInMs, String? reason) {
    if (_disposed) return;
    // @why 状态回调签名没有 code（C ABI v6 只新增查询函数），投递到本 isolate 时再查询；
    // 回调异步投递，状态可能已再次变化，此时 code 反映更新后的状态（可能为 null）。
    String? code;
    if (statusHasCode(statusFromNative(status))) {
      try {
        code = _queryStateCode();
      } on AppMcpException {
        code = null; // @why 原生客户端已停止 / 释放中：状态照常送达，只缺 code
      }
    }
    _states.add(stateFromNative(status, retryInMs, reason, code: code));
  }

  void _onNativePaired(String token) {
    if (_disposed) return;
    _paired.add(token);
  }

  void _onNativeIdleExit() {
    if (_disposed) return;
    _idleExit.add(null);
  }

  void _onNativeLog(LogLevel level, String message) {
    if (_disposed || !_logs.hasListener) return;
    _logs.add(McpLogRecord(level, message));
  }

  void _dispatchCall(_ToolEntry entry, Pointer<AmCall> ptr) {
    final call = _PendingCall(
      _rt,
      ptr,
      _b.am_call_id(ptr).toDartString(),
      _b.am_call_tool_name(ptr).toDartString(),
      _borrowedString(_b.am_call_idempotency_key(ptr)),
    );
    final argsJson = _b.am_call_arguments_json(ptr).toDartString();
    if (_disposed) {
      call.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_call_fail(ptr, k, m));
      return;
    }
    _calls.add(call);
    final cancelId = _rt.register(call);
    call.cancelId = cancelId;
    final status = _b.am_call_set_cancel_callback(
        ptr, _rt.cancel.nativeFunction, Pointer<Void>.fromAddress(cancelId), _rt.free.nativeFunction);
    if (status != AmStatus.ok) _rt.targets.remove(cancelId);

    final handler = entry.handler;
    Future<Object?>.sync(() => handler(decodeArguments(argsJson), call)).then(
      (value) => _completeCall(call, value),
      onError: (Object e, StackTrace _) => _failCall(call, e),
    );
  }

  void _completeCall(_PendingCall call, Object? value) {
    if (call.consumed) return;
    final EncodedResult result;
    try {
      result = encodeResult(value);
    } catch (e) {
      _failCall(call, e);
      return;
    }
    final status = using((arena) {
      final data = result.dataJson.toNativeUtf8(allocator: arena);
      final hints = arena<Pointer<Utf8>>(result.stateHints.isEmpty ? 1 : result.stateHints.length);
      for (var i = 0; i < result.stateHints.length; i++) {
        hints[i] = result.stateHints[i].toNativeUtf8(allocator: arena);
      }
      if (!result.isStructured) return _b.am_call_complete(call.ptr, data, hints, result.stateHints.length);
      final r = arena<AmCallResult>();
      r.ref
        ..struct_size = sizeOf<AmCallResult>()
        ..data_json = data
        ..state_hints = hints
        ..state_hints_len = result.stateHints.length
        ..status = resultStatusToNative(result.status)
        ..state_resource = _optStr(result.stateResource, arena)
        ..summary = _optStr(result.summary, arena)
        ..annotations_json = _optStr(result.annotationsJson, arena);
      return _b.am_call_complete_ex(call.ptr, r);
    });
    if (status == AmStatus.invalidJson) {
      // 未被消费，改为失败完成。
      _failCall(call, ToolCallError(ErrorKind.handlerError, '返回值不是合法的 JSON'));
      return;
    }
    _finishCall(call);
  }

  void _failCall(_PendingCall call, Object error) {
    if (call.consumed) return;
    _failNative(call.ptr, error,
        fail: _b.am_call_fail,
        failWithDetails: _b.am_call_fail_with_details,
        failUserAction: _b.am_call_fail_user_action);
    _finishCall(call);
  }

  void _finishCall(_PendingCall call) {
    call.consumed = true;
    _calls.remove(call);
    _rt.targets.remove(call.cancelId);
  }

  void _dispatchNavigate(Pointer<AmNavigate> ptr) {
    final nav = _PendingNavigate(ptr);
    final handler = _navigationHandler;
    if (_disposed || handler == null) {
      nav.consumed = true;
      using((arena) => _b.am_navigate_fail(
          ptr, (_disposed ? '客户端已释放' : '导航回调已清除').toNativeUtf8(allocator: arena)));
      return;
    }
    // 指针在 navigate 被消费前有效。
    final pagePtr = _b.am_navigate_page(ptr);
    final paramsPtr = _b.am_navigate_params_json(ptr);
    final request = NavigationRequest(
        pagePtr == nullptr ? '' : pagePtr.toDartString(), paramsPtr == nullptr ? null : paramsPtr.toDartString());
    _navigations.add(nav);
    void finish(int Function(Pointer<Utf8>)? f, [String message = '']) {
      _navigations.remove(nav);
      if (nav.consumed) return;
      nav.consumed = true;
      if (f == null) {
        _b.am_navigate_complete(ptr);
      } else {
        using((arena) => f(message.toNativeUtf8(allocator: arena)));
      }
    }

    Future<void>.sync(() => handler(request)).then((_) => finish(null), onError: (Object e) {
      if (e is NavigationDeniedError) {
        finish((m) => _b.am_navigate_deny(ptr, m), e.message);
      } else if (e is UserActionRequiredError) {
        finish((m) => using((arena) =>
            _b.am_navigate_fail_user_action(ptr, m, _optStr(e.reason, arena), _optStr(e.uri, arena))), e.message);
      } else {
        finish((m) => _b.am_navigate_fail(ptr, m), failureFromError(e).message);
      }
    });
  }

  void _dispatchRead(_ResourceEntry entry, Pointer<AmRead> ptr) {
    final read = _PendingRead(ptr);
    if (_disposed) {
      read.consumed = true;
      _withStrings2(
          ErrorKind.cancelled.wireName, '客户端已释放', (k, m) => _b.am_read_fail(ptr, k, m));
      return;
    }
    _reads.add(read);
    final reader = entry.reader;
    Future<Object?>.sync(reader).then((value) {
      if (read.consumed) return;
      final int status;
      try {
        final json = encodeJsonValue(value, '资源内容');
        status = using((arena) => _b.am_read_complete(ptr, json.toNativeUtf8(allocator: arena)));
      } catch (e) {
        _failRead(read, e);
        return;
      }
      if (status == AmStatus.invalidJson) {
        _failRead(read, ToolCallError(ErrorKind.handlerError, '资源内容不是合法的 JSON'));
        return;
      }
      read.consumed = true;
      _reads.remove(read);
    }, onError: (Object e, StackTrace _) => _failRead(read, e));
  }

  void _failRead(_PendingRead read, Object error) {
    if (read.consumed) return;
    _failNative(read.ptr, error,
        fail: _b.am_read_fail,
        failWithDetails: _b.am_read_fail_with_details,
        failUserAction: _b.am_read_fail_user_action);
    read.consumed = true;
    _reads.remove(read);
  }
}
