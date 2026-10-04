# AppMcp（C# SDK）

基于 `bindings/c`（`app_mcp.h`）的 P/Invoke 封装，目标框架 `net9.0`。

```bash
cargo build -p app-mcp-c          # 生成 libapp_mcp.so / app_mcp.dll / libapp_mcp.dylib
cd sdks/dotnet && dotnet build && dotnet test
```

原生库查找顺序：环境变量 `APP_MCP_NATIVE_PATH`（文件或目录）→ 程序目录 →
`runtimes/<rid>/native` → 系统默认。构建时会从 `$(CARGO_TARGET_DIR)/debug`（默认 `<repo>/target/debug`）
复制到输出目录，可用 MSBuild 属性 `AppMcpNativeDir` 覆盖。

## 线程

handler 与事件在 `AppMcpClientOptions.Dispatcher` 上执行。不设置时捕获 `AppMcpClient.Create`
调用时的 `SynchronizationContext.Current`：在 WPF / WinUI 的 UI 线程上创建客户端，handler 即在 UI 线程执行；
显式设为 `null` 时 handler 在线程池执行。WPF 示例见 `samples/Wpf`（仅 Windows 可构建，不在 sln 中）。

## 生命周期：务必调用 Dispose

- 每个已注册的 handler / reader 以 `GCHandle`（强引用）交给原生库，直到工具被注销或客户端被释放。
- 如果 handler 捕获了持有客户端的对象（常见：WPF 窗口的 lambda 捕获 `this`，而窗口字段里保存着
  `AppMcpClient`），就形成 **GCHandle → handler → 窗口 → 客户端** 的引用环：客户端永远不会被 GC 回收，
  finalizer 也不会运行，后台线程和连接会一直存在。
- 因此：
  - 不再需要时显式调用 `client.Dispose()`（UI 线程上用 `await client.DisposeAsync()`，避免阻塞 UI），
    例如在窗口 `Closing` / `Closed` 中。
  - 页面级工具放进 `client.CreateScope(...)`，页面关闭时 `scope.Dispose()` 注销整组工具并释放 handler。
  - `ToolRegistration` / `ResourceRegistration` 的 `Dispose()` 会注销对应工具 / 资源；仅被 GC 回收时只释放句柄，
    **不会**注销。
- `Dispose()` 会阻塞到原生后台线程结束，不要在 handler 内部同步调用。

## 工具声明与结构化结果（可选，spec/protocol.md 3.2）

```csharp
client.RegisterTool("order.submit", "提交订单",
    (args, ctx) => Task.FromResult<object?>(new ToolResult
    {
        Status = ToolResultStatus.Pending,           // Done（缺省）/ Pending / Partial / Noop
        StateResource = "order.status",              // Pending 时可读取后续状态的资源名
        Summary = "已提交，等待用户在 App 内确认",
        Annotations = new ContentAnnotations { Audience = [ContentAudience.User] },
    }),
    new ToolOptions
    {
        Annotations = new ToolAnnotations { DestructiveHint = true, IdempotentHint = false, OpenWorldHint = true },
        OutputSchemaJson = ToolSchema.For<OrderReceipt>(),
    });
```

- `ToolOptions.Annotations`（`ToolAnnotations`：`Title` / `ReadOnlyHint` / `DestructiveHint` / `IdempotentHint` / `OpenWorldHint`）
  是标准 MCP 工具注解，原样转发给 Agent；与旧写法 `ToolOptions.Risk` 同时给出时声明的字段逐个优先，缺少的按 `Risk` 推导。
  AppWire 不据此拦截或放行调用。
- `ToolOptions.OutputSchemaJson`：结果的 JSON Schema 文本；根类型不是 `object` 时 Hub 包装为 `{result: …}`。
- handler 返回普通对象即 `Done` 结果；返回 `ToolResult`（`Data`、`Status`、`StateResource`、`Summary`、`Annotations`、`StateHints`）
  时另带业务状态、摘要与内容标注。无返回值（`null` 且无 `Summary`、状态 `Done`）时 Hub 对模型输出固定文本"已完成"。
- `ToolRegistration.Update(description, options)` 整体替换：`options` 中为 `null` 的 `Annotations` / `OutputSchemaJson` 表示清除该声明。
- `ToolErrorKind.RateLimited` / `PayloadTooLarge` 由 Hub 产生（限流 / 超过大小上限），handler 不会收到，也不必抛出。

## 事件（spec/protocol.md 3.5）

App 告诉 Agent "发生了什么"（订单已发货、下载完成）：先声明，再在发生时发出。

```csharp
client.DeclareEvent("order.shipped", "订单已发货", """{"type":"object","properties":{"orderId":{"type":"string"}}}""");
bool sent = client.EmitEvent("order.shipped", new { orderId = "o1" });   // 载荷按 SerializerOptions 序列化，须为 JSON 对象
client.RemoveEvent("order.shipped");
```

- 已连接时发送并返回 `true`；未连接（休眠、断线、重连中、握手中）时丢弃并返回 `false`——不缓存、不为发事件连接或唤醒 Host，
  也不推迟空闲休眠。需要可靠送达的状态变化请改用资源（`ResourceRegistration.NotifyChanged`）。
- SDK 不读清单：要发出的事件都需运行时 `DeclareEvent`（同名替换，连接后自动同步）。未声明、名称不合法抛 `AppMcpException`
  （`InvalidName`），载荷不是对象或超过 8 KiB 为 `InvalidJson`；`EmitEventJson(name, json)` 直接传 JSON 文本。
- Agent 经 Hub 内置工具 `apps.events.subscribe` / `apps.events` 订阅与取件（spec/hub-api.md 3.17）。

## 标准意图（spec/intents.md）

工具可声明自己实现了通用动词（如"发消息""打开链接"），Agent 用 Hub 内置工具 `apps.intents` 按动词找到实现者，不必先知道有哪些 App：

```csharp
client.RegisterTool("compose.send", "发邮件", handler, new ToolOptions
{
    InputSchemaJson = """{"type":"object","properties":{"to":{"type":"array","items":{"type":"string"}},"text":{"type":"string"}},"required":["to","text"]}""",
    Implements = ["message.send@1"],
});
```

- 每项为 `"<动词>@<主版本>"`，最多 4 项、不重复；格式不合法抛 `AppMcpException`（`InvalidName`）。参数名按词表（spec/intents.md 第 2 节），
  词表的必填参数须出现在 `inputSchema.properties` 中，否则 Hub 不把它列为实现者（工具照常可调用）。
- `ToolRegistration.Update(description, options)` 中 `Implements` 为 null 或空表示清除声明。需要 C ABI v21（`AmToolOptions.implements`）。

## 结果缓存（spec/protocol.md 3.6）

只读工具与资源可声明 Hub 在一段时间内复用结果（命中不唤醒 App）；缓存多久、能否跨调用方共用由 App 判断：

```csharp
client.RegisterTool("stock.quote", "查询报价", handler, new ToolOptions
{
    Risk = ToolRisk.Read,                                   // 只对生效注解只读的工具生效
    Cache = new CachePolicy(60_000, CacheScope.Shared),     // 与调用方无关的数据才用 Shared；缺省 Private
});
client.RegisterResource("quotes", "全部报价", reader, cache: new CachePolicy(30_000));
```

- `TtlMs` 须在 1..=86400000 之间，否则抛 `AppMcpException`（`InvalidConfig`）；`Update` 时 `Cache` 为 null 表示清除。需要 C ABI v22。
- Hub 侧（`AppMcp.Hub`）：`new CallRequest(...) { CacheBypass = true }` 不查缓存；命中时 `CallOutcome.CachedAgeMs` 为结果的年龄；
  上限 `HubOptions.ResultCache = new HubResultCacheLimits { MaxEntries, MaxBytes, MaxEntryBytes }`（`MaxEntries = 0` 关闭）；
  统计见 `hub.Status().Cache`（spec/hub-api.md 3.20）。

## 工具弃用（spec/protocol.md 3.7）

工具改版时可声明旧版弃用：照常列出与调用，Hub 把声明原样交给 Agent 并在描述前标注：

```csharp
client.RegisterTool("orders.list", "列出订单", handler, new ToolOptions
{
    Deprecated = new ToolDeprecation("改用 orders.list2：支持分页", Replacement: "orders.list2", Until: "2027-06-30"),
});
```

- `Message` 须为 1..=500 个字符，`Replacement` 为同一 App 中的局部名（不得指向自身），`Until` 为 `YYYY-MM-DD`；不合法时抛
  `AppMcpException`（`InvalidConfig`）；`Update` 时 `Deprecated` 为 null 表示清除。需要 C ABI v23。
- Hub 侧（`AppMcp.Hub`）：`HubToolInfo.Deprecated`（原样声明）、`HubToolInfo.SchemaHash`（inputSchema / outputSchema 变化时随之变化）；
  不兼容变化记录见 `hub.Status().SchemaChanges`（spec/hub-api.md 3.21）。

## 界面级暴露与导航（spec/protocol.md 3.4）

```csharp
// 依赖界面的工具：声明 Surface = View 与所在页面，只在页面可见且窗口激活时启用
var checkout = client.RegisterTool("cart.checkout", "结算", Checkout,
    new ToolOptions { Surface = ToolSurface.View, Page = "cart" });
_binding = AppMcp.Wpf.WpfViewTools.Bind(cartPage, checkout);   // WinUI：AppMcp.WinUI.WinUIViewTools.Bind(page, window, checkout)

// Host 调用其他页面的工具前会请求导航：在 Start() 之前设置
client.SetNavigationHandler(AppMcp.Wpf.WpfNavigation.ForFrame(MainFrame, new Dictionary<string, Func<NavigationRequest, object>>
{
    ["cart"] = _ => new CartPage(client),
}));
client.Start();
```

- `SetNavigationHandler(Func<NavigationRequest, Task>)`：handler 在 `Dispatcher`（UI 线程）上执行；正常返回 = 导航完成，
  抛 `NavigationDeniedException` = 拒绝（`NAVIGATION_DENIED`，如用户正在输入），其他异常 = 失败（`NAVIGATION_FAILED`）。
  传 `null` 清除；能力在握手时声明，连接后才设置的在下次连接生效。
- `ViewToolGate`：与框架无关的门控（可见 且 最上层 → 启用，否则禁用），可绑定到任意界面事件。
- 后台时：需要前台的导航以 `USER_ACTION_REQUIRED`（reason `foreground`）返回；后台也要能用的能力做成 app 工具，或给 view 工具
  声明 `ToolOptions.BackgroundTool`（后台时 Hub 改调的同 App app 工具）；详见 spec/protocol.md 3.4「后台与前台」。
  `AppMcpClientOptions.NavigateInBackground` / `SetNavigateInBackground(bool)` 控制后台时是否仍调用导航回调（Windows 默认 true）：
  `WpfNavigation.ForFrame` 先还原并激活窗口，系统不允许前置时回复 `USER_ACTION_REQUIRED`；自写回调可抛
  `UserActionRequiredException(message, UserActionReason.Foreground, uri)`。
- `AppMcp.Wpf`（`net9.0-windows`，WPF）与 `AppMcp.WinUI`（WindowsAppSDK）为独立项目，只能在 Windows 上构建，不在 `AppMcp.sln` 中。

## 进程内控件兜底（WPF / WinUI 3，spec/ui-fallback.md）

没有声明工具的窗口，可显式开启兜底工具 `ui.outline` / `click` / `fill` / `press` / `scroll` / `read`：模型拿到"短引用 + 角色 + 名称 +
状态"的控件大纲，动作经 AutomationPeer 直接作用于控件（Invoke / Toggle / SelectionItem / ExpandCollapse / Value / RangeValue / Scroll），
不截图、不按坐标。默认关闭，建议只在开发环境或用户明确开启时使用。

```csharp
#if DEBUG
_fallback = AppMcp.Wpf.UiFallback.WpfUiFallback.Enable(client);   // 在 UI 线程调用；_fallback.Dispose() 注销
#endif
```

- 工具以 `Surface = View` 注册，只在应用有可见（未最小化）窗口时启用；`outline` / `read` 声明 `ReadOnlyHint = true`，其余为 `false`。
- 每个动作在 UI 线程上按引用重新取控件，核对仍在界面上、可见（不在屏外、所在窗口未被模态对话框禁用）、启用后再执行；
  按钮 Invoke 是异步投递的，执行后先让出调度器到空闲再取变化摘要。文本框写入后调用 `UpdateSource()`（默认 LostFocus 绑定也写回）。
- `PasswordBox`：大纲与 `read` 只显示 `••••`（不泄露长度），拒绝 `fill` 与按键。
- 经 `WpfViewTools.Bind(控件, 工具)` 绑定了工具的控件在大纲中标出 `[已声明：…]`，提示模型优先调用该工具。
- 选项：`WpfUiFallbackOptions { Prefix = "ui", MaxItems = 60, SettleDelay = 50 ms }`。
- WinUI 3：`AppMcp.WinUI.UiFallback.WinUIUiFallback.Enable(client, mainWindow)`（WinUI 3 不能枚举窗口，其他窗口用 `AddWindow`
  加入）；`ContentDialog` 与轻触即关的浮出层遮挡窗口内容；按键按语义执行（Tab 走 `FocusManager`，Enter / Space 激活焦点控件，
  Escape 关闭对话框 / 浮出层）；已声明标注来自 `WinUIViewTools.Bind`。平台映射见 spec/ui-fallback.md 8.3。
- 引用、核对、变化摘要、工具注册等与框架无关的部分在 `AppMcp/UiFallback/`（`UiInspectorCore`、`UiTreeBuilder`、`UiFallbackTools`），
  WPF / WinUI 只实现控件树遍历与动作。
- 测试：共用部分在 `tests/AppMcp.Tests`（`UiOutlineFormatTests`、`UiFallbackCoreTests`，任意平台）；`tests/AppMcp.Wpf.Tests`（真实 WPF 窗口，
  只能在 Windows 上运行，不在 `AppMcp.sln` 中）。WinUI 部分暂无窗口测试（需要打包的 WinUI 测试宿主），只在 Windows 上编译验证。

## Hub SDK（Agent 端）：`AppMcp.Hub`

`src/AppMcp.Hub` 是 `bindings/hub-c`（`app_mcp_hub.h`）的 P/Invoke 封装，供助手厂商在自己的进程里嵌入 Hub：
列出并调用各 App 的工具、接收事件、用自己的 UI 审批 / 配对，并按 OpenAI / Anthropic / Gemini / MCP 格式导出工具。

```bash
cargo build -p app-mcp-hub-c      # 生成 libapp_mcp_hub.so / app_mcp_hub.dll / libapp_mcp_hub.dylib
cd sdks/dotnet && dotnet test tests/AppMcp.Hub.Tests   # 集成测试另需 cargo build -p app-mcp-c
```

原生库查找顺序同上，环境变量为 `APP_MCP_HUB_NATIVE_PATH`。

```csharp
await using var hub = AppMcpHub.Start(new HubOptions
{
    Listen = "127.0.0.1:7717",          // /app 为 App 连接；McpHttp = true 时另有 /mcp
    RequireApprovalAtOrAbove = HubRisk.Destructive,
});
hub.Event += (_, e) => Console.WriteLine(e.Type);            // appConnected、toolsChanged……（未知类型原样透传）
hub.ApprovalHandler = async (req, ct) => await AskUserAsync(req, ct);

var tools = hub.ExportTools(ToolFormat.Anthropic);           // 交给模型
var result = await hub.DispatchAsync(ToolFormat.Anthropic, toolUseJson);  // 模型返回的 tool_use → tool_result
var outcome = await hub.CallAsync("notes.add", new { text = "买牛奶" });  // 或直接按全名调用
```

- 线程：事件与审批 / 配对 handler 在 `HubOptions.Dispatcher` 上执行（默认取 `Start` 时的
  `SynchronizationContext.Current`）；显式设为 `null` 时事件在 Hub 分发线程上直接触发（须尽快返回），
  handler 在线程池执行。
- 审批 / 配对 handler 抛异常按拒绝处理；其 `CancellationToken` 在 Hub 释放时取消。
- `CallAsync` 的取消令牌会调用 `am_hub_cancel_call`（结果为 `CANCELLED`）。工具失败不抛异常，见 `CallOutcome.Error`；
  `ReadResourceAsync` 失败抛 `HubCallException`；FFI 层错误抛 `HubException`（带 `HubStatus`）。
- 进度：`CallAsync(request, IProgress<CallProgress> progress, ct)`（`am_hub_call_with_progress`）——结果返回前按顺序收到
  `CallProgress(CallId, Progress, Total?, Message?)`，结果返回后不再报告；合并间隔 `HubOptions.ProgressInterval`（默认 250 毫秒）。
- handler 以 GCHandle 交给原生库，务必 `Dispose` / `DisposeAsync`。
- `ListApps()` / `ListTools()` 返回类型化的 `AppInfo` / `HubToolInfo`（`GetApps()` / `GetTools()` 返回原始 JSON）。
- `Status()` 返回运行状态 `HubStatusInfo`（监听、令牌策略、各 App 状态与最近错误、SDK 诊断上报，与 `GET /status` 相同；
  `GetStatus()` 返回原始 JSON）；`InstanceInfo.ConnectionId` 为 Hub 分配的连接 ID（与日志 `cid` 相同）。
  事件类型常量见 `HubEventTypes`，可用性常量见 `HubAvailability`。

### 资源保护与结果约定（spec/hub-api.md 3.11）

| `HubOptions` | 默认 | 说明 |
|---|---|---|
| `Limits.ToolRatePerMinute` / `ToolRateBurst` | 120 / 30 | 每（App, 工具）令牌桶；超出 → `RATE_LIMITED`；`0` 次/分钟不限 |
| `Limits.AppRatePerMinute` / `AppRateBurst` | 600 / 60 | 每 App（所有工具合计）令牌桶 |
| `Limits.MaxArgumentsBytes` / `MaxResultBytes` / `MaxResourceBytes` | 1 MiB / 4 MiB / 4 MiB | 超出 → `PAYLOAD_TOO_LARGE`（不截断）；`0` 不限 |
| `OutputValidation` | `Log` | 结果与 outputSchema 不符时：`Off` 不校验 / `Log` 只记日志 / `Reject` 以 `HANDLER_ERROR` 结束 |

- `HubLimits` 中为 `null` 的字段取默认值；限流时 `*Burst = 0` 启动失败。生效值见 `HubStatusInfo.Limits` / `OutputValidation`，
  各 App 被拒绝次数见 `AppStatusInfo.RateLimited` / `TooLarge`，工具声明见 `AppStatusInfo.Tools`（`ToolDeclarationInfo`）。
- 被拒绝的调用照常返回 `CallOutcome`，`Error.Kind` 为 `HubError.RateLimited` / `HubError.PayloadTooLarge`（`Details` 字段见 spec/protocol.md 第 4 节）。
- App 的声明：`HubToolInfo.Annotations`（Agent 实际看到的注解：声明优先、缺少的按 risk 推导）、`HubToolInfo.OutputSchema`、
  `ApprovalRequest.Annotations`；结构化结果：`CallOutcome.Status`（`HubResultStatus`）、`StateResource`、`Summary`、`Annotations`。
- 策略挂点（spec/hub-api.md 3.13）：`HubOptions.Policy`（`HubPolicy`）或运行中 `AppMcpHub.SetPolicy(...)`；`hide` 使 App / 工具从所有列表消失
  （调用按 `TOOL_NOT_FOUND`），`deny` 使调用以 `HubError.PolicyDenied` 结束（`Details.ruleId`）。无规则时行为不变；生效规则与命中次数见
  `HubStatusInfo.Policy`。

### 页面、导航与幂等键（spec/hub-api.md 3.14 / 3.15）

- `HubToolInfo.Surface`（`HubToolSurface.App` / `View`）与 `Page`：App 工具的界面依赖与所在页面；内置与上游工具为 `null`。
- 自动导航的等待上限 `HubOptions.NavigateTimeout`（默认 5 秒）；改调后台替代时 `CallOutcome.RoutedTo` 为实际调用的工具全名。
- `new CallRequest("shop.order.submit", args) { IdempotencyKey = "order-7" }`：幂等键原样转交 App（`ToolContext.IdempotencyKey`），
  不合法时 `Error.Kind` 为 `INVALID_INPUT`。
- `new CallRequest(...) { Priority = CallPriority.Interactive }`：调用优先级（`Interactive` / `Normal` / `Background`，null = Normal）
  原样转交 App，App 的调用队列先交互、后后台（第 16 项 P6）。
- 内置工具 `apps.activate` / `apps.release` 总是列出，有页面目录时另有 `apps.page` / `apps.navigate`（经 `CallAsync` / `DispatchAsync` 调用）。
- 调用对象（第 16 项 P5，spec/hub-api.md 3.6「调用对象」）：内置工具 `apps.calls`（列出本会话进行中的调用）/ `apps.cancel`（`{callId}`，
  取消本会话的调用，发起方得到 `CANCELLED`）总是列出；`Status().Calls` 为全部进行中的调用（`CallStatusInfo`：`CallId`、`Name`、
  `Caller`、`Subject`、`State`（`CallState.Created` / `Approving` / `Activating` / `Running`）、`ElapsedMs`、`InstanceId`、`Progress`、
  `PlatformState`）。嵌入方取消用 `CancelCall(callId)`（不检查归属）。
- 调用元信息：`CallOutcome.DurationMs`（Hub 收到调用到得出结果的毫秒数，含审批、唤醒与等待 App）、`CallOutcome.Woke`
  （本次 App 工具调用是否经历了唤醒 / 按名激活；内置与上游工具恒为 false）。旧 Hub 未给出时为 0 / false。

### 事件、订阅与信箱（spec/hub-api.md 3.17）

- Agent 用内置工具 `apps.events.subscribe {appId, event?, filter?}` / `apps.events.unsubscribe {subscriptionId}` / `apps.events {max?}`
  订阅与取件（总是列出；按会话 / 已登记 Agent 归属，经 `CallAsync` / `DispatchAsync` 调用）；资源 `app-mcp://apps/events` 为信箱只读视图。
- 厂商 / 机主回调：`hub.SetEventHandler(ev => ...)`（`HubAppEvent`：`Id`、`AppId`、`InstanceId`、`Name`、`Payload`、`At`），每个通过校验的
  事件（不论有无订阅）调用一次，在 `Dispatcher` 上执行（null 时在分发线程上，须尽快返回）；传 `null` 清除。`Event` 也收到同一事件
  （`HubEventTypes.AppEvent`），但处理过慢时可能 `lagged`。
- 上限：`HubOptions.EventLimits = new HubEventLimits { MaxSubscriptions, MaxInboxEvents, InboxTtl, PerSubscriptionPerMinute }`
  （为 null 的字段取默认值 32 / 100 / 24 小时 / 60；`PerSubscriptionPerMinute = 0` 不限）。
- `Status().Events`（`EventsStatusInfo`）：`Subscriptions`（`EventSubscriptionStatusInfo`：`SubscriptionId`、`Subscriber`、`AppId`、
  `Event`、`Delivered`、`Dropped`、`Pending`）与 `DroppedInvalid`（未声明 / 载荷不合法而丢弃的事件数）。

### 标准意图（spec/intents.md 第 4 节）

- Agent 用内置工具 `apps.intents {intent?}`（总是列出，不唤醒 App）按动词列出实现者；`HubToolInfo.Implements` 为 App 声明的动词。
- 机主默认表：`hub.SetIntentDefaults(new Dictionary<string, string> { ["message.send"] = "mail.compose.send" })`（键为动词或
  `动词@主版本`，值为工具全名；空表清空）。默认工具在 `apps.intents` 中排首位并标 `default: true`，只是提示，Hub 不据此路由。
  不合法时抛 `HubException`（`InvalidConfig`），之前的默认表继续生效。
- `hub.Intents()` / `Status().Intents`（`IntentsStatusInfo`）：生效的 `Defaults` 与最近一次替换失败的 `LastError`。

### 休眠与唤醒（spec/hub-api.md 3.5）

App 以 `LifecycleMode.Idle` 等方式运行时，空闲后进入休眠：其工具仍列出（`Availability == "dormant"`），
`AppInfo.DormantInstances` 列出休眠实例（`IsDormant`），并收到 `appDormant` 事件；调用这些工具时 Hub 生成一次性令牌、
发 `appWaking` 事件并调用唤醒实现，App 回连后再派发（`WakeTimeout` 内未回连 → `APP_NOT_RESPONDING`）。
默认唤醒实现按平台执行系统命令；可用 `Waker` 替换（对应 C 接口 `am_hub_set_waker_cb` / `am_hub_waker_complete`，API v2）：

```csharp
hub.Waker = async (req, ct) =>
{
    // req.AppId、req.InstanceId（null = 冷启动）、req.Descriptor（Kind / Target / Background）、req.Token、req.ActivationArg
    if (req.Descriptor.Kind != "uri") throw new WakeFailedException("LAUNCH_FAILED", "不支持");
    await LaunchAsync(req.Descriptor.Target!, req.ActivationArg, ct);  // App 端把参数交给 HandleWake
};
// 正常返回 = 已发出激活；WakeFailedException 以指定类别结束调用；其他异常 → LAUNCH_FAILED；设为 null 恢复默认
```

相关配置：`HubOptions.LeaseTtl`（默认 60 秒，`TimeSpan.Zero` 关闭）、`WakeTimeout`（15 秒）、`WakeTokenTtl`（60 秒）、
`DormantTtl`（24 小时）、`DormantReplacedByNewInstance`（默认 true）、`WakeFromLaunch`（默认 false）。

## 生命周期：休眠与唤醒（spec/lifecycle.md）

```csharp
var client = AppMcpClient.Create(new AppMcpClientOptions
{
    AppId = "my-app",
    AppName = "My App",
    Lifecycle = new LifecycleOptions
    {
        Mode = LifecycleMode.Idle,              // persistent（默认）/ idle / on-demand
        IdleTimeout = TimeSpan.FromSeconds(60),
        Residency = Residency.ExitWhenIdle,     // keep / exit-when-idle / exit-always
        Wake = WakeDescriptorFactory.ForWindows("my-app"), // MSIX → aumid，未打包 → uri
    },
    ConnectTimeout = TimeSpan.FromSeconds(5),
});
```

| API | 说明 |
|---|---|
| `HandleWake(string)` / `HandleWake(IEnumerable<string>)` | 交给客户端的 OS 激活参数（命令行、协议 URI、AUMID 激活参数）；不是唤醒返回 false。可在 `Start()` 前调用 |
| `Wake(reason)` / `ConnectNow()` / `Sleep(reason)` | App 主动回连 / on-demand 下连接 / 主动休眠 |
| `Hold()` → `IDisposable` | 临时阻止自动休眠；`ToolContext.Hold()` 让 handler 发起的长任务在调用完成后仍保持连接 |
| `ToolsHash` | 工具与资源定义摘要 |
| `IdleExit` 事件 | 休眠完成且 `Residency` 允许退出时触发（在 Dispatcher 上），App 自行决定是否退出 |
| `ClientStatus.Dormant` / `Waking` | 新增状态 |
| `ToolCallException(kind, message, details)` | 带结构化详情失败：对象字段合并进错误 `data`，其他值放在 `data.details` |
| `UserActionRequiredException(message, reason, uri)` | 需要用户本人操作（登录过期、权限未授予、需切到前台等）：以 `USER_ACTION_REQUIRED` 失败，`reason`（`UserActionReason.Login` 等）与 `uri`（App 内入口）可选；资源 reader 中抛出同样生效（`ToolCallException` 的 `details` 也随读取错误发出） |

### 功耗选项（spec/lifecycle.md 第 11、13 节）

| 选项 | 默认 | 说明 |
|---|---|---|
| `AppMcpClientOptions.Heartbeat` | `Auto` | `Auto`（本地 IPC / 桌面本机回环不发心跳）/ `Always` / `Off` |
| `LifecycleOptions.HostAbsentRetries` | 3 | idle / on-demand 下连续多少次"Host 不在"后转休眠；0 = 一直重连 |
| `LifecycleOptions.MergeWindow` | 2 秒 | 调用 / 资源读取后的合并窗口，之后是否在线由 Hub 租约决定；`TimeSpan.Zero` = 不留窗口 |
| `LifecycleOptions.SleepOnBackground` | false | idle / on-demand 下进入后台（`SetVisibility(Hidden)`）且空闲时立即休眠，不等租约 |
| `LifecycleOptions.LegacyTimers` | false | 回退到 4e 之前的定时器行为 |
| `RegisterResource(..., realtime: true)` | false | 模型在等待变化的资源：被订阅时保持连接、休眠中变化时回连推送；普通资源的订阅不阻止休眠 |

`RegisterResource(..., annotations: new ContentAnnotations { Audience = [ContentAudience.User], Priority = 0.5 })`：资源内容的标注
（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上；缺省不声明。

调用去重（spec/protocol.md 3.3）：同一 `callId` 在有效期内重复到达时重放首次结果、不再执行 handler。
`AppMcpClientOptions.CallDedup = new CallDedupOptions { Ttl = TimeSpan.FromMinutes(5), MaxEntries = 64 }`（为 null 时即此默认值；
`CallDedupOptions.Off` 或任一项为 0 关闭）。命中时 SDK 记一条警告日志。

调用调度（spec/protocol.md 5.3，只在 SDK 内生效、不同步给 Host）：`ToolOptions.Concurrency`（本工具同时执行的调用上限，0 = 不单独限制，
只受 `MaxConcurrentCalls` 约束）与 `ToolOptions.Exclusive`（互斥组名，同组工具同一时刻至多一个在执行）。暂不能执行的调用按到达顺序排队，
`AppMcpClientOptions.MaxQueuedCalls`（默认 64，0 = 不限）为排队上限，超出时新调用以 `RATE_LIMITED`（details `{"scope": "queue", "limit": N}`）拒绝。

用户正在操作（spec/protocol.md 5.3「用户正在操作」）：`client.SetBusy(true)` / `SetBusy(false)` 声明用户正在 App 内操作，期间写调用
（生效注解不是 `ReadOnlyHint = true` 的工具）按 `AppMcpClientOptions.BusyPolicy` 处理——`BusyPolicy.Reject`（默认）以 `RATE_LIMITED`
（details `{"scope": "busy"}`）拒绝，`BusyPolicy.Queue` 排队到 `SetBusy(false)` 后按序执行；只读调用不受影响。`client.SetBusyPolicy(...)` 运行中修改策略。
`using (client.Busy()) { ... }` 在作用域内声明：引用计数、可嵌套、可跨线程 Dispose。有效 busy = `SetBusy` 显式开关 ∨ 未结束作用域数 > 0，
二者互不清除（`SetBusy(false)` 不结束进行中的作用域，作用域结束也不清开关）；`client.IsBusy` 返回该有效值。

幂等键（spec/protocol.md 3.3「idempotencyKey」）：`ToolContext.IdempotencyKey`（`string?`）是 Agent 给出的幂等键，原样提供，没有时为 null；
同一工具同一键的重复调用已按首次结果重放（同上，去重关闭时只透传），App 可另作业务去重键或传给后端。

`LifecycleOptions` 为 record，可用 `with` 改个别字段。

**桌面平台默认**：`AppMcpClientOptions.Lifecycle` 为 null 时仍是 persistent（兼容）。已用 `SingleInstance` 接收转交参数的 App
可用 `single.DefaultLifecycle(wake)` 取平台默认值——有唤醒描述的窗口程序 `Idle`（2 秒合并窗口）；`wake.Background = true`
（托盘 / 无窗口进程）`OnDemand` + `SleepOnBackground`；`WakeKind.None` 保持 `Persistent`（没有唤醒入口，休眠后 Host 只能按清单
`launch` 冷启动新实例）。显式传入的 `Lifecycle` 不会被替换：

```csharp
var single = SingleInstance.Acquire("my-app", e.Args)!;
var wake = WakeDescriptorFactory.ForWindows("my-app");        // 托盘程序：ForWindows("my-app", background: true)
var client = AppMcpClient.Create(new AppMcpClientOptions
{
    AppId = "my-app",
    AppName = "My App",
    Lifecycle = single.DefaultLifecycle(wake) with { IdleTimeout = TimeSpan.FromMinutes(5) },
});
```

### Windows 唤醒入口（`AppMcp.Activation`）

- **未打包应用**：`ProtocolRegistration.Register(scheme, exePath)` 写入 `HKCU\Software\Classes\<scheme>`
  （`URL Protocol`、`shell\open\command = "exe" "%1"`，无需管理员）。Host 唤醒时打开
  `<scheme>:app-mcp/wake?token=…`。测试或自定义存储可传入 `IRegistryWriter`。
- **MSIX 打包应用**：`WakeDescriptorFactory.ForWindows` 检测到包身份（`GetCurrentPackageFullName`）后使用
  `aumid`；Host 以 `ActivateApplication(aumid, "app-mcp-wake:<token>")` 激活，参数同样交给 `HandleWake`。
- **单实例重定向**：激活会启动第二个进程，用 `SingleInstance`（`Mutex` + 命名管道，`CurrentUserOnly`）
  把参数转交给已运行的实例：

```csharp
// App.OnStartup
var single = SingleInstance.Acquire("my-app", e.Args);
if (single is null) { Shutdown(); return; }   // 已转交给首实例
// 创建客户端后：
client.HandleWake(e.Args);                    // 冷启动唤醒
single.AttachClient(client);                  // 之后转交来的参数自动 HandleWake
single.Activated += (_, a) => { if (!a.WakeHandled) mainWindow.Activate(); };
```

WinUI 3 可改用 `AppInstance.FindOrRegisterForKey` + `RedirectActivationToAsync`，在 `Activated` 中取
`ProtocolActivatedEventArgs.Uri` / 启动参数交给 `HandleWake`。

### exit-when-idle 时如何退出

`IdleExit` 只在本进程由唤醒冷启动（启动参数含唤醒令牌）且已休眠后触发（`ExitAlways` 为每次休眠）。
事件在 Dispatcher（UI 线程）上触发，不要在里面同步 `Dispose()` 客户端：

```csharp
// WPF
client.IdleExit += (_, _) => { if (!userInteracted) Application.Current.Shutdown(); };
// WinUI 3
client.IdleExit += (_, _) => { if (!userInteracted) Application.Current.Exit(); };
```

窗口 `Closing` 中照常 `await client.DisposeAsync()`。完整示例见 `samples/Wpf`（`App.xaml.cs` 单实例 + 协议注册，
`MainWindow.xaml.cs` 生命周期配置与 `IdleExit`）。

## 按名寻址（spec/naming.md）

App 在系统名字服务登记名字，不主动连接 Hub；Hub（`app-mcp-host serve --name-service`）发现它时不启动进程，调用时按名拨号，
App 未运行由系统激活（Linux：D-Bus 会话总线名 `dev.appmcp.App.<AppId>`；Windows：命名管道 `\\.\pipe\appmcp-<用户 SID>-<AppId>`，
Hub 直接运行登记的程序）。通道在最后一次调用后宽限（默认 15 秒）关闭。

```csharp
await using var client = AppMcpClient.Create(new AppMcpClientOptions
{
    AppId = "named-dotnet",
    AppName = "按名寻址示例",
    RegisterName = true,                 // Start() 后登记名字
    NameInstance = null,                 // 可选：另登记实例名 [a-z][a-z0-9-]{0,31}（不能是 default），不合法时 Create 抛 InvalidConfig
    Lifecycle = new LifecycleOptions { Mode = LifecycleMode.OnDemand, Residency = Residency.ExitWhenIdle },
});
client.IdleExit += (_, _) => quit.TrySetResult();  // 由激活启动（命令行带 --app-mcp-activation）的进程在通道关闭后退出
client.Start();
```

```bash
dotnet build samples/Named
app-mcp-host app install --app-id named-dotnet --exec <输出目录>/AppMcp.Samples.Named(.exe) [--manifest app-mcp.json]
app-mcp-host serve --name-service
```

- 对应 C ABI `AmClientOptions.register_name` / `name_instance`（app_mcp.h v17，按 `struct_size` 读取）。本平台不支持时经 `Log` 报告，其余照常。
- 完整示例 `samples/Named`；全链路测试 `tests/AppMcp.Tests/NamingE2eTests.cs`（`app install` → `app-mcp-host stdio --name-service`
  → 发现不激活 → 冷激活调用 → 宽限后 App 退出 → 再激活；Linux 用私有 D-Bus 会话总线，没有 `dbus-daemon` 时跳过；Windows 用命名管道，
  登记目录经 `LOCALAPPDATA` 指到临时目录）。需要 `cargo build -p app-mcp-host`（或环境变量 `APP_MCP_HOST_BIN`），找不到时跳过。
