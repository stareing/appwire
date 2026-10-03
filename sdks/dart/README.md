# app-mcp Dart / Flutter SDK

| 包 | 内容 |
|---|---|
| `app_mcp/` | Dart SDK：通过 `dart:ffi` 调用 C ABI（`bindings/c/include/app_mcp.h`，`AM_API_VERSION 3`） |
| `app_mcp_flutter/` | Flutter 适配：`AppMcpScope`、`McpToolGroup`、`McpTool` / `McpResource`、`useMcpTool`、生命周期可见性与回连、`AppMcpWakeChannel` |
| `app_mcp_go_router/` | go_router 适配：`mcpGoRouterHandler`（导航请求 → 命名路由；依赖 go_router，独立成包） |
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
- `ToolHandle.update`：未提供的参数保持不变，显式传 `null` 清除该声明（`title` / `activation` / `annotations` / `outputSchema`
  删除，`inputSchema` 变为无参数，`risk` 恢复 `Risk.write`，`enabled` 恢复 `true`；`description` 不可清除）。参数类型不符时抛
  `ArgumentError`。`ToolHandle.replace(spec)` 按 `ToolSpec` 整体替换。

## 界面级暴露与导航（spec/protocol.md 3.4）

```dart
// 依赖界面的工具：只在所在路由是栈顶（未被新页面 / 对话框盖住）时启用
McpTool(name: 'cart.checkout', description: '结算', surface: ToolSurface.view, page: 'cart', handler: checkout)

// Host 调用其他页面的工具前请求导航：在 start() 之前设置
client.setNavigationHandler(mcpGoRouterHandler(router));                       // go_router（app_mcp_go_router 包）
client.setNavigationHandler(mcpNavigatorHandler(navKey, routes: {'cart': '/cart'}));  // Navigator 命名路由
```

- `surface` / `page`：`McpTool`、`useMcpTool`、`McpScope.tool`、`ToolSpec`、`ToolHandle.update`（`null` 清除）。
- `setNavigationHandler((request) async {...})`：在主 isolate 上执行，正常返回 = 完成，抛 `NavigationDeniedError` = 拒绝
  （`NAVIGATION_DENIED`），其他异常 = 失败（`NAVIGATION_FAILED`）；`request.params` 为解码后的参数。传 `null` 清除；能力在握手时声明。
- view 工具的门控：所在路由是栈顶（`ModalRoute.isCurrentOf`）且祖先 `McpViewGate(active: …)` 都为真。keep-alive 的标签页
  （`IndexedStack`、`TabBarView`）不改变路由，用 `McpViewGate(active: index == current)` 标出；`McpRouteGate(observer: …)`
  （`RouteAware`，观察者 `McpRouteObserver` 放进 `navigatorObservers`）在路由被盖住 / 恢复时另外回调 `onChanged`。
- 后台时：需要前台的导航立即以 `USER_ACTION_REQUIRED`（reason `foreground`）返回；后台也要能用的能力做成 app 工具，或给 view
  工具声明 `backgroundTool`（后台时 Hub 改调的同 App app 工具，`McpTool` / `useMcpTool` / `ToolSpec` 均可传）；详见
  spec/protocol.md 3.4「后台与前台」。`AppMcp(navigateInBackground: …)` / `setNavigateInBackground(bool)` 控制后台时是否仍调用
  导航回调（默认随平台：桌面 true，Android / iOS false）；回调中抛 `UserActionRequiredError(message, reason: 'foreground', uri: …)`
  即以 `USER_ACTION_REQUIRED` 回复。

## 进程内控件兜底（Flutter，spec/ui-fallback.md）

没有声明工具的界面，可显式开启兜底工具 `ui.outline` / `click` / `fill` / `press` / `scroll` / `read`：模型拿到"短引用 + 角色 + 名称 +
状态"的控件大纲，动作经语义树（`SemanticsOwner.performAction`）直接作用于控件，不截图、不按坐标。默认关闭，建议只在开发环境或
用户明确开启时使用。需要 Flutter 3.35 及以上。

```dart
final fallback = McpUiFallback.enable(client);   // fallback.dispose() 注销
// 已有声明工具的控件：大纲中标出 [已声明：cart.clear]，提示模型优先调用该工具
McpDeclared(tool: 'cart.clear', child: TextButton(onPressed: clear, child: const Text('清空')))
```

- 工具以 `surface: view` 注册，只在 `AppLifecycleState` 为 resumed / inactive（有可见窗口）时启用；`outline` / `read` 声明
  `readOnlyHint: true`，其余为 `false`。
- 语义树（`SemanticsBinding.ensureSemantics()`）只在启用且已连接 Host 时开启，其余时间没有开销。
- 引用带指纹（角色 + 名称 + 所在分组）：语义节点被重建时按指纹沿用引用，节点被复用给别的控件时旧引用作废。
- 每个动作重新取节点并核对可见、启用；文本框先聚焦、等一帧再 `setText`；按键支持 Enter、Escape、Tab / Shift+Tab、Space。
- `obscureText` 的文本框：只显示 `••••`（不泄露长度），拒绝 `fill` 与按键。
- 对话框打开时下层控件被框架移出语义树，其引用在对话框关闭前按失效处理。

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
- `resource(..., annotations: ContentAnnotations(audience: [ContentAudience.user], priority: 0.5))` /
  `McpResource(annotations: ...)`：资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上；缺省不声明。
- 调用去重（spec/protocol.md 3.3）：`AppMcp(callDedup: CallDedupPolicy(ttl: Duration(minutes: 5), maxEntries: 64))`（即默认值）——
  同一 callId 在有效期内重复到达时重放首次结果、不再执行 handler；`CallDedupPolicy.off` 或任一项为 0 关闭。命中时 SDK 记一条警告日志。
- 调用调度（spec/protocol.md 5.3，只在 SDK 内生效、不同步给 Host）：工具声明 `concurrency`（本工具同时执行的调用上限，0 = 不单独限制，
  只受 `maxConcurrentCalls` 约束）与 `exclusive`（互斥组名，同组工具同一时刻至多一个在执行），`tool(...)` / `ToolSpec` /
  `McpTool` / `useMcpTool` 均可传；暂不能执行的调用按到达顺序排队，`AppMcp(maxQueuedCalls: 64)`（默认值，0 = 不限）为排队上限，
  超出时新调用以 `RATE_LIMITED`（details `{"scope": "queue", "limit": N}`）拒绝。
- 幂等键（spec/protocol.md 3.3「idempotencyKey」）：`ctx.idempotencyKey`（`String?`）是 Agent 给出的幂等键，原样提供，没有时为 null；
  同一工具同一键的重复调用已按首次结果重放（去重关闭时只透传），App 可另作业务去重键或传给后端。
- handler 内的长任务用 `ctx.hold()` 延长持有（必须在调用完成前获取）；调用进行中本身就视为非空闲。
- `throw ToolCallError(kind, message, details: {...})`：`details` 经 `am_call_fail_with_details` 上报，
  对象字段合并进协议错误的 `data`。
- `throw UserActionRequiredError(message, reason: UserActionReason.login, uri: 'myapp://login')`：需要用户本人操作
  （登录过期、权限未授予、需切到前台等），以 `USER_ACTION_REQUIRED` 失败；`reason` / `uri` 可选，缺省时不出现在 `data` 中。
  资源的 `read` 中抛出同样生效（`ToolCallError` 的 `details` 也随读取错误发出）。
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

## 按名寻址（spec/naming.md）

`AppMcp(..., registerName: true, nameInstance: null)`：`start()` 后在系统名字服务登记（Linux：D-Bus `dev.appmcp.App.<appId>`；
Windows：命名管道 `\\.\pipe\appmcp-<用户 SID>-<appId>`），App 不主动连接 Hub，由 Hub（`app-mcp-host serve --name-service`）
按名拨入，未运行时由系统激活（先用 `app-mcp-host app install --app-id <appId> --exec <程序>` 登记）。通常与
`LifecyclePolicy(mode: LifecycleMode.onDemand, residency: Residency.exitWhenIdle)` 同用：由激活启动（命令行带
`--app-mcp-activation`）的进程在通道关闭后收到 `onIdleExit`。`nameInstance` 为可选的登记实例名（`[a-z][a-z0-9-]{0,31}`，
不能是 `default`），不合法时抛出 `AppMcpException`（`AppMcpErrorCode.invalidConfig`）。对应 C ABI `AmClientOptions.register_name` /
`name_instance`（app_mcp.h v17）。本平台不支持时经 `logs` 报告，其余照常。

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
cd sdks/dart/app_mcp_go_router && flutter test

# Linux 桌面集成测试（需要 GTK 3、cmake、ninja、clang）
export NO_PROXY=127.0.0.1,::1,localhost no_proxy=127.0.0.1,::1,localhost   # 有代理时必须设置
export APP_MCP_NATIVE_PATH=$CARGO_TARGET_DIR/debug/libapp_mcp.so
export APP_MCP_FAKE_HOST=$CARGO_TARGET_DIR/debug/examples/fake_host
cd sdks/dart/app_mcp_flutter/example && flutter test integration_test -d linux
```
