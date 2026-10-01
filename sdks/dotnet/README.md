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
