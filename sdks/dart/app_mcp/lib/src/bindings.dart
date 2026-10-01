// 手写的 FFI 绑定，与 bindings/c/include/app_mcp.h 逐一对应。
//
// 本文件只做类型映射与符号查找，不含任何逻辑；惯用封装见 client.dart。
// 修改 app_mcp.h 时需同步更新本文件。

// ignore_for_file: constant_identifier_names, non_constant_identifier_names

import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

/// 头文件中的 `AM_API_VERSION`。
const int AM_API_VERSION = 3;

// ---------------------------------------------------------------------------
// 状态码与枚举（C 枚举按 int 传递）
// ---------------------------------------------------------------------------

abstract final class AmStatus {
  static const int ok = 0;
  static const int invalidArgument = 1;
  static const int invalidName = 2;
  static const int invalidSchema = 3;
  static const int duplicateName = 4;
  static const int invalidJson = 5;
  static const int invalidConfig = 6;
  static const int alreadyCompleted = 7;
  static const int disposed = 8;
  static const int stopped = 9;
  static const int internal = 10;
  static const int panic = 11;
}

abstract final class AmClientKind {
  static const int native = 0;
  static const int hybrid = 1;
}

abstract final class AmRisk {
  static const int read = 0;
  static const int write = 1;
  static const int destructive = 2;
  static const int payment = 3;
  static const int osSensitive = 4;
}

abstract final class AmActivation {
  static const int none = -1;
  static const int headless = 0;
  static const int background = 1;
  static const int foreground = 2;
}

abstract final class AmVisibility {
  static const int visible = 0;
  static const int hidden = 1;
  static const int frozen = 2;
}

abstract final class AmCancelReason {
  static const int requested = 0;
  static const int timeout = 1;
  static const int disconnected = 2;
  static const int stopped = 3;
}

abstract final class AmStateStatus {
  static const int idle = 0;
  static const int connecting = 1;
  static const int handshaking = 2;
  static const int pendingPairing = 3;
  static const int connected = 4;
  static const int backoff = 5;
  static const int rejected = 6;
  static const int stopped = 7;
  static const int dormant = 8;
  static const int waking = 9;
  // v5：对端不是期望的 Host（spec/protocol.md 1.6）
  static const int hostMismatch = 10;
}

abstract final class AmLogLevel {
  static const int debug = 0;
  static const int info = 1;
  static const int warn = 2;
  static const int error = 3;
}

// v3：生命周期

abstract final class AmLifecycleMode {
  static const int persistent = 0;
  static const int idle = 1;
  static const int onDemand = 2;
}

abstract final class AmResidency {
  static const int keep = 0;
  static const int exitWhenIdle = 1;
  static const int exitAlways = 2;
}

abstract final class AmWakeKind {
  static const int unset = -1;
  static const int none = 0;
  static const int uri = 1;
  static const int aumid = 2;
  static const int appleEvent = 3;
  static const int dbus = 4;
  static const int androidIntent = 5;
  static const int webUrl = 6;
}

abstract final class AmWakeReason {
  static const int osActivation = 0;
  static const int app = 1;
  static const int visible = 2;
  static const int coldStart = 3;
}

abstract final class AmSleepReason {
  static const int idle = 0;
  static const int grace = 1;
  static const int background = 2;
  static const int app = 3;
}

// ---------------------------------------------------------------------------
// 不透明句柄
// ---------------------------------------------------------------------------

final class AmClient extends Opaque {}

final class AmScope extends Opaque {}

final class AmTool extends Opaque {}

final class AmResource extends Opaque {}

final class AmCall extends Opaque {}

final class AmRead extends Opaque {}

/// v3：阻止自动休眠的持有。
final class AmHold extends Opaque {}

// ---------------------------------------------------------------------------
// 回调类型
// ---------------------------------------------------------------------------

typedef AmFreeFnNative = Void Function(Pointer<Void> userData);
// API 版本 2 起，状态 / 配对 / 日志回调中的 char* 归接收方所有，读取后须 am_string_free。
typedef AmStateFnNative = Void Function(
    Pointer<Void> userData, Int32 status, Uint64 retryInMs, Pointer<Utf8> reason);
typedef AmPairedFnNative = Void Function(Pointer<Void> userData, Pointer<Utf8> token);
typedef AmLogFnNative = Void Function(Pointer<Void> userData, Int32 level, Pointer<Utf8> message);
typedef AmToolFnNative = Void Function(Pointer<Void> userData, Pointer<AmCall> call);
typedef AmReadFnNative = Void Function(Pointer<Void> userData, Pointer<AmRead> read);
typedef AmCancelFnNative = Void Function(Pointer<Void> userData, Int32 reason);
// v3：休眠完成且驻留策略允许退出。
typedef AmIdleExitFnNative = Void Function(Pointer<Void> userData);

// ---------------------------------------------------------------------------
// 结构体
// ---------------------------------------------------------------------------

final class AmClientConfig extends Struct {
  external Pointer<Utf8> app_id;
  external Pointer<Utf8> app_name;
  external Pointer<Utf8> instance_id;
  external Pointer<Utf8> host_url;
  external Pointer<Utf8> app_version;
  external Pointer<Utf8> instance_title;
  external Pointer<Utf8> token;
  external Pointer<Utf8> launch_token;
  @Int32()
  external int client_kind;
  @Uint32()
  external int max_concurrent_calls;
  external Pointer<Utf8> overview_summary;
  external Pointer<Utf8> overview_body;
  external Pointer<Utf8> overview_locale;
}

final class AmClientCallbacks extends Struct {
  external Pointer<NativeFunction<AmStateFnNative>> on_state;
  external Pointer<NativeFunction<AmPairedFnNative>> on_paired;
  external Pointer<NativeFunction<AmLogFnNative>> on_log;
  external Pointer<Void> user_data;
  external Pointer<NativeFunction<AmFreeFnNative>> free_user_data;
}

/// v3：生命周期策略。
final class AmLifecycle extends Struct {
  @Int32()
  external int mode;
  @Uint64()
  external int idle_timeout_ms;
  @Uint64()
  external int hidden_idle_timeout_ms;
  @Uint64()
  external int grace_ms;
  @Int32()
  external int residency;
  @Int32()
  external int wake_kind;
  external Pointer<Utf8> wake_target;
  @Bool()
  external bool wake_background;
}

/// v3：`am_client_new_ex` 的扩展选项。
final class AmClientOptions extends Struct {
  @Uint32()
  external int struct_size;
  external Pointer<AmLifecycle> lifecycle;
  @Uint32()
  external int connect_timeout_ms;
  external Pointer<NativeFunction<AmIdleExitFnNative>> on_idle_exit;
}

final class AmToolSpec extends Struct {
  external Pointer<Utf8> name;
  external Pointer<Utf8> description;
  external Pointer<Utf8> input_schema_json;
  @Int32()
  external int risk;
  @Int32()
  external int activation;
  external Pointer<Utf8> title;
  @Bool()
  external bool enabled;
}

final class AmResourceSpec extends Struct {
  external Pointer<Utf8> name;
  external Pointer<Utf8> description;
  external Pointer<Utf8> mime_type;
}

// ---------------------------------------------------------------------------
// 函数
// ---------------------------------------------------------------------------

/// libapp_mcp 中全部导出函数。
final class AppMcpBindings {
  AppMcpBindings(this.library);

  /// 按 `APP_MCP_NATIVE_PATH` 环境变量或平台默认名加载。
  factory AppMcpBindings.load([String? path]) => AppMcpBindings(openNativeLibrary(path));

  final DynamicLibrary library;

  // 通用
  late final am_version =
      library.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('am_version');
  late final am_last_error_message = library
      .lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>('am_last_error_message');
  late final am_string_free = library
      .lookupFunction<Void Function(Pointer<Utf8>), void Function(Pointer<Utf8>)>('am_string_free');

  // 客户端
  late final am_client_new = library.lookupFunction<
      Int32 Function(Pointer<AmClientConfig>, Pointer<AmClientCallbacks>, Pointer<Pointer<AmClient>>),
      int Function(Pointer<AmClientConfig>, Pointer<AmClientCallbacks>,
          Pointer<Pointer<AmClient>>)>('am_client_new');
  late final am_client_free = library
      .lookupFunction<Void Function(Pointer<AmClient>), void Function(Pointer<AmClient>)>(
          'am_client_free');
  late final am_client_start = library
      .lookupFunction<Int32 Function(Pointer<AmClient>), int Function(Pointer<AmClient>)>(
          'am_client_start');
  late final am_client_stop = library
      .lookupFunction<Int32 Function(Pointer<AmClient>), int Function(Pointer<AmClient>)>(
          'am_client_stop');
  late final am_client_set_visibility = library.lookupFunction<
      Int32 Function(Pointer<AmClient>, Int32, Bool),
      int Function(Pointer<AmClient>, int, bool)>('am_client_set_visibility');
  late final am_client_state = library.lookupFunction<
      Int32 Function(Pointer<AmClient>, Pointer<Int32>, Pointer<Uint64>, Pointer<Pointer<Utf8>>),
      int Function(Pointer<AmClient>, Pointer<Int32>, Pointer<Uint64>,
          Pointer<Pointer<Utf8>>)>('am_client_state');
  late final am_client_instance_id = library.lookupFunction<
      Pointer<Utf8> Function(Pointer<AmClient>),
      Pointer<Utf8> Function(Pointer<AmClient>)>('am_client_instance_id');
  late final am_client_token = library.lookupFunction<Pointer<Utf8> Function(Pointer<AmClient>),
      Pointer<Utf8> Function(Pointer<AmClient>)>('am_client_token');
  late final am_client_root_scope = library.lookupFunction<
      Int32 Function(Pointer<AmClient>, Pointer<Pointer<AmScope>>),
      int Function(Pointer<AmClient>, Pointer<Pointer<AmScope>>)>('am_client_root_scope');

  // 客户端：生命周期（v3）
  late final am_lifecycle_init = library.lookupFunction<Void Function(Pointer<AmLifecycle>),
      void Function(Pointer<AmLifecycle>)>('am_lifecycle_init');
  late final am_client_new_ex = library.lookupFunction<
      Int32 Function(Pointer<AmClientConfig>, Pointer<AmClientCallbacks>, Pointer<AmClientOptions>,
          Pointer<Pointer<AmClient>>),
      int Function(Pointer<AmClientConfig>, Pointer<AmClientCallbacks>, Pointer<AmClientOptions>,
          Pointer<Pointer<AmClient>>)>('am_client_new_ex');
  late final am_client_handle_wake = library.lookupFunction<
      Bool Function(Pointer<AmClient>, Pointer<Utf8>),
      bool Function(Pointer<AmClient>, Pointer<Utf8>)>('am_client_handle_wake');
  late final am_client_wake = library.lookupFunction<Int32 Function(Pointer<AmClient>, Pointer<Bool>),
      int Function(Pointer<AmClient>, Pointer<Bool>)>('am_client_wake');
  late final am_client_wake_with_reason = library.lookupFunction<
      Int32 Function(Pointer<AmClient>, Int32, Pointer<Bool>),
      int Function(Pointer<AmClient>, int, Pointer<Bool>)>('am_client_wake_with_reason');
  late final am_client_connect_now = library.lookupFunction<
      Int32 Function(Pointer<AmClient>, Pointer<Bool>),
      int Function(Pointer<AmClient>, Pointer<Bool>)>('am_client_connect_now');
  late final am_client_sleep = library.lookupFunction<Int32 Function(Pointer<AmClient>, Pointer<Bool>),
      int Function(Pointer<AmClient>, Pointer<Bool>)>('am_client_sleep');
  late final am_client_sleep_with_reason = library.lookupFunction<
      Int32 Function(Pointer<AmClient>, Int32, Pointer<Bool>),
      int Function(Pointer<AmClient>, int, Pointer<Bool>)>('am_client_sleep_with_reason');
  late final am_client_hold = library.lookupFunction<
      Int32 Function(Pointer<AmClient>, Pointer<Pointer<AmHold>>),
      int Function(Pointer<AmClient>, Pointer<Pointer<AmHold>>)>('am_client_hold');
  late final am_hold_release_ptr =
      library.lookup<NativeFunction<Void Function(Pointer<AmHold>)>>('am_hold_release');
  late final am_hold_release = am_hold_release_ptr.asFunction<void Function(Pointer<AmHold>)>();
  late final am_client_tools_hash = library.lookupFunction<Pointer<Utf8> Function(Pointer<AmClient>),
      Pointer<Utf8> Function(Pointer<AmClient>)>('am_client_tools_hash');
  late final am_parse_wake_token = library.lookupFunction<Pointer<Utf8> Function(Pointer<Utf8>),
      Pointer<Utf8> Function(Pointer<Utf8>)>('am_parse_wake_token');

  // Scope
  late final am_scope_create = library.lookupFunction<
      Int32 Function(Pointer<AmScope>, Pointer<Utf8>, Pointer<Pointer<AmScope>>),
      int Function(Pointer<AmScope>, Pointer<Utf8>, Pointer<Pointer<AmScope>>)>('am_scope_create');
  late final am_scope_dispose = library
      .lookupFunction<Int32 Function(Pointer<AmScope>), int Function(Pointer<AmScope>)>(
          'am_scope_dispose');
  late final am_scope_free = library
      .lookupFunction<Void Function(Pointer<AmScope>), void Function(Pointer<AmScope>)>(
          'am_scope_free');

  // 工具
  late final am_tool_register = library.lookupFunction<
      Int32 Function(
          Pointer<AmScope>,
          Pointer<AmToolSpec>,
          Pointer<NativeFunction<AmToolFnNative>>,
          Pointer<Void>,
          Pointer<NativeFunction<AmFreeFnNative>>,
          Pointer<Pointer<AmTool>>),
      int Function(
          Pointer<AmScope>,
          Pointer<AmToolSpec>,
          Pointer<NativeFunction<AmToolFnNative>>,
          Pointer<Void>,
          Pointer<NativeFunction<AmFreeFnNative>>,
          Pointer<Pointer<AmTool>>)>('am_tool_register');
  late final am_tool_update = library.lookupFunction<
      Int32 Function(Pointer<AmTool>, Pointer<AmToolSpec>),
      int Function(Pointer<AmTool>, Pointer<AmToolSpec>)>('am_tool_update');
  late final am_tool_set_enabled = library.lookupFunction<Int32 Function(Pointer<AmTool>, Bool),
      int Function(Pointer<AmTool>, bool)>('am_tool_set_enabled');
  late final am_tool_dispose = library
      .lookupFunction<Int32 Function(Pointer<AmTool>), int Function(Pointer<AmTool>)>(
          'am_tool_dispose');
  late final am_tool_free = library
      .lookupFunction<Void Function(Pointer<AmTool>), void Function(Pointer<AmTool>)>(
          'am_tool_free');

  // 资源
  late final am_resource_register = library.lookupFunction<
      Int32 Function(
          Pointer<AmScope>,
          Pointer<AmResourceSpec>,
          Pointer<NativeFunction<AmReadFnNative>>,
          Pointer<Void>,
          Pointer<NativeFunction<AmFreeFnNative>>,
          Pointer<Pointer<AmResource>>),
      int Function(
          Pointer<AmScope>,
          Pointer<AmResourceSpec>,
          Pointer<NativeFunction<AmReadFnNative>>,
          Pointer<Void>,
          Pointer<NativeFunction<AmFreeFnNative>>,
          Pointer<Pointer<AmResource>>)>('am_resource_register');
  late final am_resource_notify_changed = library
      .lookupFunction<Int32 Function(Pointer<AmResource>), int Function(Pointer<AmResource>)>(
          'am_resource_notify_changed');
  late final am_resource_dispose = library
      .lookupFunction<Int32 Function(Pointer<AmResource>), int Function(Pointer<AmResource>)>(
          'am_resource_dispose');
  late final am_resource_free = library
      .lookupFunction<Void Function(Pointer<AmResource>), void Function(Pointer<AmResource>)>(
          'am_resource_free');

  // 调用
  late final am_call_id = library.lookupFunction<Pointer<Utf8> Function(Pointer<AmCall>),
      Pointer<Utf8> Function(Pointer<AmCall>)>('am_call_id');
  late final am_call_tool_name = library.lookupFunction<Pointer<Utf8> Function(Pointer<AmCall>),
      Pointer<Utf8> Function(Pointer<AmCall>)>('am_call_tool_name');
  late final am_call_arguments_json = library.lookupFunction<
      Pointer<Utf8> Function(Pointer<AmCall>),
      Pointer<Utf8> Function(Pointer<AmCall>)>('am_call_arguments_json');
  late final am_call_is_cancelled = library
      .lookupFunction<Bool Function(Pointer<AmCall>), bool Function(Pointer<AmCall>)>(
          'am_call_is_cancelled');
  late final am_call_set_cancel_callback = library.lookupFunction<
      Int32 Function(Pointer<AmCall>, Pointer<NativeFunction<AmCancelFnNative>>, Pointer<Void>,
          Pointer<NativeFunction<AmFreeFnNative>>),
      int Function(Pointer<AmCall>, Pointer<NativeFunction<AmCancelFnNative>>, Pointer<Void>,
          Pointer<NativeFunction<AmFreeFnNative>>)>('am_call_set_cancel_callback');
  late final am_call_complete = library.lookupFunction<
      Int32 Function(Pointer<AmCall>, Pointer<Utf8>, Pointer<Pointer<Utf8>>, Size),
      int Function(
          Pointer<AmCall>, Pointer<Utf8>, Pointer<Pointer<Utf8>>, int)>('am_call_complete');
  late final am_call_fail = library.lookupFunction<
      Int32 Function(Pointer<AmCall>, Pointer<Utf8>, Pointer<Utf8>),
      int Function(Pointer<AmCall>, Pointer<Utf8>, Pointer<Utf8>)>('am_call_fail');

  late final am_call_fail_with_details = library.lookupFunction<
      Int32 Function(Pointer<AmCall>, Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>),
      int Function(Pointer<AmCall>, Pointer<Utf8>, Pointer<Utf8>,
          Pointer<Utf8>)>('am_call_fail_with_details');
  late final am_call_hold = library.lookupFunction<
      Int32 Function(Pointer<AmCall>, Pointer<Pointer<AmHold>>),
      int Function(Pointer<AmCall>, Pointer<Pointer<AmHold>>)>('am_call_hold');

  // 资源读取
  late final am_read_resource_name = library.lookupFunction<
      Pointer<Utf8> Function(Pointer<AmRead>),
      Pointer<Utf8> Function(Pointer<AmRead>)>('am_read_resource_name');
  late final am_read_complete = library.lookupFunction<
      Int32 Function(Pointer<AmRead>, Pointer<Utf8>),
      int Function(Pointer<AmRead>, Pointer<Utf8>)>('am_read_complete');
  late final am_read_fail = library.lookupFunction<
      Int32 Function(Pointer<AmRead>, Pointer<Utf8>, Pointer<Utf8>),
      int Function(Pointer<AmRead>, Pointer<Utf8>, Pointer<Utf8>)>('am_read_fail');
}

/// 原生库默认文件名。
String defaultNativeLibraryName() {
  if (Platform.isWindows) return 'app_mcp.dll';
  if (Platform.isMacOS) return 'libapp_mcp.dylib';
  return 'libapp_mcp.so';
}

/// 打开原生库：显式路径 > 环境变量 `APP_MCP_NATIVE_PATH` > 平台默认名。
///
/// iOS 上库通常静态链接进可执行文件，此时（没有显式路径与环境变量）使用
/// [DynamicLibrary.process]。
DynamicLibrary openNativeLibrary([String? path]) {
  final explicit = path ?? Platform.environment['APP_MCP_NATIVE_PATH'];
  if (explicit != null && explicit.isNotEmpty) return DynamicLibrary.open(explicit);
  if (Platform.isIOS) return DynamicLibrary.process();
  return DynamicLibrary.open(defaultNativeLibraryName());
}
