# app-mcp Dart / Flutter SDK

| 包 | 内容 |
|---|---|
| `app_mcp/` | Dart SDK：通过 `dart:ffi` 调用 C ABI（`bindings/c/include/app_mcp.h`，`AM_API_VERSION 3`） |
| `app_mcp_flutter/` | Flutter 适配：`AppMcpScope`、`McpToolGroup`、`McpTool` / `McpResource`、`useMcpTool`、生命周期可见性与回连、`AppMcpWakeChannel` |
| `app_mcp_flutter/example/` | 示例：购物车（含 Linux 桌面集成测试） |

```dart
final client = AppMcp(
  appId: 'shop',
  appName: '商店',
  overview: const AppOverview(summary: '演示商城', body: '## 典型流程\n...', locale: 'zh-CN'),
)..start();
client.logs.listen((r) => debugPrint('app-mcp $r'));   // 原生库日志
client.onPaired.listen(saveToken);                        // 配对后持久化 token
runApp(AppMcpScope(client: client, disposeClient: true, child: const ShopApp()));
```

原生库查找顺序：`libraryPath` 参数 → 环境变量 `APP_MCP_NATIVE_PATH` → 平台默认名
（`libapp_mcp.so` / `libapp_mcp.dylib` / `app_mcp.dll`；iOS 为静态链接进程内符号）。

## 工具声明与结构化结果（可选，spec/protocol.md 3.2）

```dart
client.tool('order.submit',
    description: '提交订单',
    annotations: const ToolAnnotations(destructiveHint: true, idempotentHint: false, openWorldHint: true),
    outputSchema: {'type': 'object', 'properties': {'orderId': {'type': 'string'}}},
    handler: (args, ctx) => ToolResult(null,
        status: ToolResultStatus.pending,          // done（缺省）/ pending / partial / noop
        stateResource: 'order.status',             // pending 时可读取后续状态的资源名
        summary: '已提交，等待用户在 App 内确认',
        annotations: const ContentAnnotations(audience: [ContentAudience.user])));
```

- `annotations`（`ToolAnnotations`：`title` / `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`）是标准 MCP
  工具注解，原样转发给 Agent；与旧写法 `risk` 同时给出时声明的字段逐个优先，缺少的按 `risk` 推导。AppWire 不据此拦截或放行调用。
- `outputSchema`：结果的 JSON Schema；根类型不是 `object` 时 Hub 包装为 `{result: …}`。
- 返回普通值即 `done` 结果；需要业务状态、摘要或内容标注（`ContentAnnotations`：`audience` / `priority` / `lastModified`）时返回
  `ToolResult`。无返回值（`null` 且无 `summary`、状态 `done`）时 Hub 对模型输出固定文本"已完成"。
- `McpTool`、`useMcpTool`、`McpScope.tool`、`ToolHandle.update` 接受同样的 `annotations` / `outputSchema` 参数。

## 生命周期（休眠与唤醒，spec/lifecycle.md）

```dart
final client = AppMcp(
  appId: 'shop',
  appName: '商店',
  lifecycle: const LifecyclePolicy(
    mode: LifecycleMode.idle,               // persistent / idle / onDemand
    idleTimeout: Duration(seconds: 60),
    hiddenIdleTimeout: Duration(seconds: 15),
    grace: Duration(seconds: 10),           // onDemand：任务完成后保留连接的时间
    residency: Residency.keep,              // keep / exitWhenIdle / exitAlways
    wake: WakeDescriptor.uri('myshop'),     // 随 app/sleep 上报；不填时 Host 回退到清单 launch
  ),
  connectTimeout: const Duration(seconds: 5),
)..start();

client.handleWake(args);        // OS 激活参数 / URL；不是本 SDK 的唤醒返回 false
client.wake();                  // App 主动回连
client.connectNow();            // onDemand 模式主动连接
client.sleep();                 // App 主动休眠
final hold = client.hold();     // 临时阻止休眠 … hold.release();
client.onIdleExit.listen((_) { /* residency 允许时：App 自行决定是否退出 */ });
```

- 不传 `lifecycle` 时按平台取默认值（`LifecyclePolicy.platformDefault`，spec/lifecycle.md 第 13 节 B1）：Android / iOS 为
  `onDemand` + `sleepOnBackground: true` + `residency: keep`（启动后不连接，第一次进入前台、被唤醒或 `connectNow()` 时连接；
  进入后台且空闲即休眠；进程交给系统回收，SDK 不持有前台服务 / WakeLock / 后台任务）；iOS 另设 `hiddenIdleTimeout: 0`。
  桌面为 `persistent`：本封装没有单实例重定向，休眠后经 URI / 清单 `launch` 唤醒会冷启动新进程而不是回连本实例；
  有可靠唤醒入口（macOS URL scheme、自行实现的单实例转交）的桌面 App 显式传 `LifecycleMode.idle`。
  显式传入的 `lifecycle` 原样使用；只改个别字段用 `LifecyclePolicy.platformDefault(...).copyWith(...)`。
- 功耗选项（spec/lifecycle.md 第 11、13 节）：

  | 选项 | 默认 | 说明 |
  |---|---|---|
  | `AppMcp(heartbeat: ...)` | `HeartbeatMode.auto` | `auto`（本地 IPC / 桌面本机回环不发心跳）/ `always` / `off` |
  | `LifecyclePolicy.hostAbsentRetries` | 3 | idle / onDemand 下连续多少次"Host 不在"后转休眠；0 = 一直重连 |
  | `LifecyclePolicy.mergeWindow` | 2 秒 | 调用 / 资源读取后的合并窗口，之后是否在线由 Hub 租约决定；`Duration.zero` = 不留窗口 |
  | `LifecyclePolicy.sleepOnBackground` | false（移动端默认 true） | idle / onDemand 下进入后台且空闲时立即休眠，不等租约 |
  | `LifecyclePolicy.legacyTimers` | false | 回退到 4e 之前的定时器行为 |
  | `resource(..., realtime: true)` / `McpResource(realtime: true)` | false | 模型在等待变化的资源：被订阅时保持连接、休眠中变化时回连推送；普通资源的订阅不阻止休眠 |
- handler 内的长任务用 `ctx.hold()` 延长持有（必须在调用完成前获取）；调用进行中本身就视为非空闲。
- `throw ToolCallError(kind, message, details: {...})`：`details` 经 `am_call_fail_with_details` 上报，
  对象字段合并进协议错误的 `data`。
- `ConnectionStatus` 新增 `dormant`（已休眠，无连接无定时器）与 `waking`（正在回连）。

### Flutter：可见性与回到前台

`AppMcpScope`（`trackLifecycle: true`，默认）用 `AppLifecycleListener` 上报可见性：
`resumed` → visible + focused，`inactive` → visible，`hidden` → hidden，`paused` → 移动端 frozen / 桌面 hidden，
`detached` → frozen。进入后台后由原生层休眠（`sleepOnBackground` 时空闲即休眠，否则按 `hiddenIdleTimeout`）；
`idle` / `onDemand` 模式下可见性从隐藏 / 冻结变为可见（含首次上报即可见，即启动后第一次进入前台）时以原因 `visible`
调用 `wake`（未休眠时无效果）；`inactive ↔ resumed` 只是焦点变化，不回连。

### WakeDescriptor 怎么填

| 平台 | 描述 | App 侧接收 |
|---|---|---|
| Android | `WakeDescriptor.androidIntent('com.example.shop/dev.appmcp.WakeReceiver')`（`background: true`） | Kotlin `WakeReceiver` → MethodChannel |
| iOS | `WakeDescriptor.uri('myshop')`（`background: false`，系统会把 App 带到前台） | `onOpenURL` / `app_links` |
| macOS | `WakeDescriptor.uri('myshop', background: true)`（Host 用 `open -g`） | `app_links` / `onOpenURL` |
| Windows / Linux 桌面 | `WakeDescriptor.uri('myshop')` / `WakeDescriptor(WakeKind.dbus, target: 'com.example.Shop', background: true)` | 命令行参数 / D-Bus action → `handleWake` |

### 把唤醒参数交给 `handleWake`

Flutter 应用的唤醒参数来自平台通道。`AppMcpWakeChannel` 监听 MethodChannel `dev.appmcp/wake` 的
`handleWake(String)` 调用并转交客户端：

```dart
final wake = AppMcpWakeChannel(client)..attach();
```

**Android**：Host 发送显式广播 `dev.appmcp.action.WAKE`，extra 为 `app-mcp-wake:<token>`。Kotlin 侧
`WakeReceiver`（后续由 Kotlin SDK 提供）立即 `goAsync()`，有 Flutter 引擎（前台实例）时经通道转发：

```kotlin
MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "dev.appmcp/wake")
    .invokeMethod("handleWake", "app-mcp-wake:$token")
```

没有界面时由 Kotlin SDK 以加急 WorkManager 任务回连处理（不启动前台服务）。在 `AndroidManifest.xml` 中
声明该 receiver（`exported="true"`，action `dev.appmcp.action.WAKE`），并把组件名写进 `WakeDescriptor.androidIntent`。

**iOS**：在 `Info.plist` 注册 URL scheme（`CFBundleURLTypes`），Host 打开 `<scheme>://app-mcp/wake?token=…`。
用 `app_links` 包在 Dart 侧直接转交（冷启动的初始链接也要处理）：

```dart
final appLinks = AppLinks();
final initial = await appLinks.getInitialLink();
if (initial != null) wake.handleLink(initial);    // 冷启动：start 之前调用也可以
appLinks.uriLinkStream.listen(wake.handleLink);
```

或在 `AppDelegate` / `SceneDelegate` 的 `application(_:open:options:)` / `scene(_:openURLContexts:)` 中经同一个
MethodChannel 调用 `handleWake`。iOS 没有后台唤醒（后台调用走 App Intents）。

## 线程模型

所有原生回调都用 `NativeCallable.listener` 接收，投递到创建 `AppMcp` 的 isolate（Flutter 中即主 isolate）
的事件循环上执行，handler 可以直接 `setState`。

C ABI 自 API 版本 2 起，状态（`reason`）、配对（`token`）、日志（`message`）回调中的字符串归接收方所有，
异步投递后仍然有效；SDK 读取后立即 `am_string_free`。本 SDK 使用 v3 新增的 `am_client_new_ex` 等函数，
只能与 v3 的原生库配合使用。idle-exit 回调同样经 `NativeCallable.listener` 投递到本 isolate。
C ABI 没有运行时版本查询函数，`AM_API_VERSION` 与头文件的一致性由测试保证。

## 限制

### Flutter 热重启前需 `dispose`

热重启（hot restart）会重建 Dart isolate，但原生库仍留在进程中：旧客户端的后台线程、连接与已注册工具都还在，
而它们的回调指向已销毁的 isolate（悬空回调）。热重启不会运行 `State.dispose` / `AppMcpScope` 的清理逻辑，
结果是 Host 上出现重复实例，对旧实例的调用无人处理，直至超时。

- 热重启前先对旧客户端调用 `client.dispose()`（例如开发期放一个调试按钮，或在重新装配应用时释放）。
- 热重载（hot reload）不重建 isolate，不受影响；`McpTool` 在重建时只刷新 handler。
- 正式构建没有热重启，不受影响。

### 不支持 UI 线程无响应超时（`APP_NOT_RESPONDING`）

其他语言的封装在切换到 UI 线程时设置超时（设计文档 12.3.2），超时返回 `APP_NOT_RESPONDING`。Dart 无法实现：

- `NativeCallable.listener` 只是把消息投递给主 isolate，原生线程无从得知消息何时被处理；
- 主 isolate 被同步代码卡住时，同一 isolate 内的任何计时器都无法运行，也无法从其他线程抢占它；
- 从另一个 isolate 代为 `am_call_fail` 会与主 isolate 稍后的完成竞争同一个 `AmCall`（重复消费是未定义行为）。

因此卡住的调用只能由 Host 侧的调用超时结束（取消原因 `timeout`）。handler 中的耗时计算请放到
`Isolate.run` / `compute` 中，保持主 isolate 响应。

### 其他

- `dispose()` 会阻塞到原生后台线程结束（通常很快），不要在 handler 内部调用。
- isolate 退出时仍在途的回调消息会被丢弃（其中的字符串随之泄漏，量很小）。

## 构建与测试

```bash
（默认输出到仓库根目录 target/，无需设置 CARGO_TARGET_DIR）
cargo build -p app-mcp-c
cargo build -p app-mcp-native --example fake_host

# Dart SDK：假原生库测试（需要 cc）+ 真实原生库集成测试（含生命周期往返；找不到库时跳过）
cd sdks/dart/app_mcp && dart test

# Flutter 适配：单元 / widget 测试
cd sdks/dart/app_mcp_flutter && flutter test

# Linux 桌面集成测试（需要 GTK 3、cmake、ninja、clang）
export NO_PROXY=127.0.0.1,::1,localhost no_proxy=127.0.0.1,::1,localhost   # 有代理时必须设置
export APP_MCP_NATIVE_PATH=$CARGO_TARGET_DIR/debug/libapp_mcp.so
export APP_MCP_FAKE_HOST=$CARGO_TARGET_DIR/debug/examples/fake_host
cd sdks/dart/app_mcp_flutter/example && flutter test integration_test -d linux
```
