// 生命周期策略的公开类型（spec/lifecycle.md）。纯 Dart，不依赖 FFI；由 types.dart 重导出。

/// 生命周期模式。
enum LifecycleMode {
  /// 不休眠（桌面端默认，兼容旧行为）。
  persistent,

  /// 启动即连接；空闲后休眠；唤醒后回连。
  idle,

  /// 启动时不连接；被唤醒或 `connectNow()` 时连接，任务完成后经过合并窗口休眠（移动端默认）。
  onDemand,
}

/// 心跳策略（spec/lifecycle.md 第 11 节 A3）。
enum HeartbeatMode {
  /// 按传输：本地 IPC / 桌面本机回环不发，远程发（默认）。
  auto,
  always,
  off,
}

/// 休眠后的进程驻留策略。
enum Residency {
  /// 只断开连接（默认；移动端进程交给系统回收）。
  keep,

  /// 仅当本进程由唤醒冷启动时，休眠后触发 `onIdleExit`，由 App 决定是否退出。
  exitWhenIdle,

  /// 每次休眠后都触发 `onIdleExit`（无界面的辅助进程）。
  exitAlways,
}

/// 唤醒方式（spec/lifecycle.md 第 5 节）。
enum WakeKind { none, uri, aumid, appleEvent, dbus, androidIntent, webUrl }

/// 回连原因。
enum WakeReason { osActivation, app, visible, coldStart }

/// 休眠原因。
enum SleepReason { idle, grace, background, app }

/// 本实例的唤醒描述，随 `app/sleep` 上报给 Host。
final class WakeDescriptor {
  const WakeDescriptor(this.kind, {this.target, this.background = false});

  /// iOS / macOS / 未打包 Windows：自定义 URL scheme（Host 打开 `<scheme>://app-mcp/wake?token=`）。
  const WakeDescriptor.uri(String scheme, {bool background = false})
      : this(WakeKind.uri, target: scheme, background: background);

  /// Android：显式广播到 `WakeReceiver`，[component] 为 `包名/接收器类名`。
  const WakeDescriptor.androidIntent(String component)
      : this(WakeKind.androidIntent, target: component, background: true);

  final WakeKind kind;

  /// scheme、AUMID、bundle id、D-Bus 名、组件名或 URL。
  final String? target;

  /// 能否不把窗口带到前台就唤醒。
  final bool background;

  @override
  bool operator ==(Object other) =>
      other is WakeDescriptor &&
      other.kind == kind &&
      other.target == target &&
      other.background == background;

  @override
  int get hashCode => Object.hash(kind, target, background);

  @override
  String toString() => 'WakeDescriptor(${kind.name}, $target, background: $background)';
}

/// 生命周期策略。
/// 调用去重策略（spec/protocol.md 3.3，[AppMcp] 的 `callDedup`）：同一 callId 在有效期内重复到达时重放首次结果、
/// 不再执行 handler。任一项为 0 关闭去重。
final class CallDedupPolicy {
  const CallDedupPolicy({this.ttl = const Duration(minutes: 5), this.maxEntries = 64});

  /// 关闭去重。
  static const off = CallDedupPolicy(ttl: Duration.zero, maxEntries: 0);

  /// 首次结果的保留时长。
  final Duration ttl;

  /// 最多保留的结果数（超出淘汰最早的）。
  final int maxEntries;

  @override
  bool operator ==(Object other) => other is CallDedupPolicy && other.ttl == ttl && other.maxEntries == maxEntries;

  @override
  int get hashCode => Object.hash(ttl, maxEntries);

  @override
  String toString() => 'CallDedupPolicy(ttl: $ttl, maxEntries: $maxEntries)';
}

final class LifecyclePolicy {
  const LifecyclePolicy({
    this.mode = LifecycleMode.persistent,
    this.idleTimeout = const Duration(seconds: 60),
    this.hiddenIdleTimeout = const Duration(seconds: 15),
    this.grace = const Duration(seconds: 10),
    this.residency = Residency.keep,
    this.wake,
    this.hostAbsentRetries = 3,
    this.legacyTimers = false,
    this.mergeWindow = const Duration(seconds: 2),
    this.sleepOnBackground = false,
  });

  /// 按平台选择默认值（spec/lifecycle.md 第 13 节 B1"平台默认"）：Android / iOS 为 [LifecycleMode.onDemand]
  /// + [sleepOnBackground]、[Residency.keep]（iOS 另设 `hiddenIdleTimeout = 0`）；其他平台为 [LifecycleMode.persistent]。
  ///
  /// @why 桌面不默认 `idle`：本封装没有单实例重定向，休眠后经 URI / 清单 `launch` 唤醒会冷启动新进程而不是回连本实例。
  /// 有可靠唤醒入口（如 macOS URL scheme、自行实现的单实例转交）的桌面 App 显式传入 `idle`。
  factory LifecyclePolicy.platformDefault({
    required bool isAndroid,
    required bool isIOS,
    WakeDescriptor? wake,
  }) {
    if (isIOS) {
      return LifecyclePolicy(
          mode: LifecycleMode.onDemand, hiddenIdleTimeout: Duration.zero, sleepOnBackground: true, wake: wake);
    }
    if (isAndroid) return LifecyclePolicy(mode: LifecycleMode.onDemand, sleepOnBackground: true, wake: wake);
    return LifecyclePolicy(wake: wake);
  }

  final LifecycleMode mode;

  /// `idle` 模式下空闲多久进入休眠。
  final Duration idleTimeout;

  /// 可见性为 hidden / frozen 时使用的更短空闲时间。
  final Duration hiddenIdleTimeout;

  /// `onDemand` 模式下任务完成后保留连接的时间。
  final Duration grace;
  final Residency residency;

  /// 未设置时 Host 回退到清单 `launch`。
  final WakeDescriptor? wake;

  /// `idle` / `onDemand` 下连续多少次"Host 不在"后转休眠（第 11 节 A2）；0 = 一直重连。
  final int hostAbsentRetries;

  /// true：回退到 4e 之前的定时器行为（第 11、13 节）。
  final bool legacyTimers;

  /// 调用 / 资源读取后的合并窗口（第 13 节 B1）；[Duration.zero] = 不留窗口（调用后只看租约）。
  final Duration mergeWindow;

  /// true：`idle` / `onDemand` 下进入后台（可见 → 隐藏 / 冻结）且空闲时立即休眠，不等租约（第 13 节 B4）。
  final bool sleepOnBackground;

  LifecyclePolicy copyWith({
    LifecycleMode? mode,
    Duration? idleTimeout,
    Duration? hiddenIdleTimeout,
    Duration? grace,
    Residency? residency,
    WakeDescriptor? wake,
    int? hostAbsentRetries,
    bool? legacyTimers,
    Duration? mergeWindow,
    bool? sleepOnBackground,
  }) =>
      LifecyclePolicy(
        mode: mode ?? this.mode,
        idleTimeout: idleTimeout ?? this.idleTimeout,
        hiddenIdleTimeout: hiddenIdleTimeout ?? this.hiddenIdleTimeout,
        grace: grace ?? this.grace,
        residency: residency ?? this.residency,
        wake: wake ?? this.wake,
        hostAbsentRetries: hostAbsentRetries ?? this.hostAbsentRetries,
        legacyTimers: legacyTimers ?? this.legacyTimers,
        mergeWindow: mergeWindow ?? this.mergeWindow,
        sleepOnBackground: sleepOnBackground ?? this.sleepOnBackground,
      );

  @override
  bool operator ==(Object other) =>
      other is LifecyclePolicy &&
      other.mode == mode &&
      other.idleTimeout == idleTimeout &&
      other.hiddenIdleTimeout == hiddenIdleTimeout &&
      other.grace == grace &&
      other.residency == residency &&
      other.wake == wake &&
      other.hostAbsentRetries == hostAbsentRetries &&
      other.legacyTimers == legacyTimers &&
      other.mergeWindow == mergeWindow &&
      other.sleepOnBackground == sleepOnBackground;

  @override
  int get hashCode => Object.hash(mode, idleTimeout, hiddenIdleTimeout, grace, residency, wake,
      hostAbsentRetries, legacyTimers, mergeWindow, sleepOnBackground);

  @override
  String toString() => 'LifecyclePolicy(${mode.name}, idle: $idleTimeout, hidden: $hiddenIdleTimeout, '
      'grace: $grace, residency: ${residency.name}, wake: $wake, hostAbsentRetries: $hostAbsentRetries, '
      'legacyTimers: $legacyTimers, mergeWindow: $mergeWindow, sleepOnBackground: $sleepOnBackground)';
}
