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
- handler 以 GCHandle 交给原生库，务必 `Dispose` / `DisposeAsync`。
- `ListApps()` / `ListTools()` 返回类型化的 `AppInfo` / `HubToolInfo`（`GetApps()` / `GetTools()` 返回原始 JSON）。
  事件类型常量见 `HubEventTypes`，可用性常量见 `HubAvailability`。

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
