# app-mcp SDK ↔ Host 协议规范 v1

本文件是 SDK（App 内的客户端）与 Host（本地 MCP Host）之间通信协议的权威定义。
类型定义见 `crates/protocol`，两者必须保持一致；修改任一方都要同步修改另一方。

Host 对模型一侧使用标准 MCP，不在本规范范围内。

## 1. 传输

### 1.1 消息与帧

- 消息为 UTF-8 JSON 文本，一条消息对应一个 WebSocket 文本帧。所有传输（1.2）上的帧与消息完全相同。
- 每条消息是一个 JSON-RPC 2.0 对象；不支持批量（数组）消息。
- 双方都可以发送请求、通知和响应。请求 ID 由发送方生成，只需在发送方内唯一。

### 1.2 端点

SDK 用一个**端点字符串**指定 Host（原生 SDK 的 `host_url` / `hostUrl`，网页 SDK 的 `hostUrl`）：

| 形式 | 传输 | 使用者 |
|---|---|---|
| `ws://<host>:<port>` / `wss://…` | WebSocket over TCP | 网页（只能用这种）；原生 App 显式配置时 |
| `unix:<绝对路径>` | WebSocket over Unix 域套接字 | Linux、macOS 原生 App（默认） |
| `pipe:\\.\pipe\<名称>` | WebSocket over Windows 命名管道 | Windows 原生 App（默认） |

- 本地 IPC（`unix:` / `pipe:`）上跑的仍是 RFC 6455 WebSocket：连接建立后客户端发送 HTTP Upgrade 握手，
  请求 URL 固定为 `ws://localhost/`（Host 不检查路径与 `Host` 头），不使用 TLS；之后的帧、心跳（`ping`）、
  Close 与 TCP 上逐字节相同。这样 SDK 核心、Host 的消息处理与超时逻辑对所有传输只有一份实现。
- 格式不合法、或当前平台不支持该形式（如 Windows 上的 `unix:`）时，SDK 在创建客户端时报配置错误。
- 实现：`app_mcp_protocol::endpoint`（解析、默认位置）。

### 1.3 默认端点

Host 默认同时监听：

- WebSocket：`127.0.0.1:7717`（网页与显式配置 `ws://` 的 App）。
- 本地 IPC（平台默认 IPC 端点）：
  - Linux：`$XDG_RUNTIME_DIR/app-mcp/hub.sock`；未设置 `XDG_RUNTIME_DIR` 时 `~/.app-mcp/run/hub.sock`；
  - macOS：`~/.app-mcp/run/hub.sock`（设置了 `XDG_RUNTIME_DIR` 时同 Linux）；
  - Windows：`\\.\pipe\app-mcp-<当前用户 SID>`（如 `\\.\pipe\app-mcp-S-1-5-21-…-1001`）；
  - Android / iOS：无（App 沙箱之间不能共享套接字，这些平台用 WebSocket，如 Android 经 `adb reverse tcp:7717`）。

原生 SDK 未配置端点时按以下顺序**确定**端点：

1. 环境变量 `APP_MCP_ENDPOINT`（非空时原样使用，不合法则报配置错误）；
2. 平台默认 IPC 端点（同上）；
3. `ws://127.0.0.1:7717`（平台没有默认 IPC 端点时）。

这是配置的解析顺序，**不是连接失败后的回退**：选定的端点连不上时，SDK 按 5.6 退避重连同一个端点，
不会自动换用其他传输。Host 关闭了 IPC 服务或改了 IPC 端点时，App 需设置 `APP_MCP_ENDPOINT` 或显式配置端点。
网页 SDK 的默认端点始终是 `ws://127.0.0.1:7717`。

### 1.4 连接鉴权

在 `app/hello` 的配对与 `Origin` 规则（第 6 节）之前，Host 按传输做一层连接级校验：

- **TCP**：只接受来自回环地址的连接。
- **Unix 域套接字**：
  - 套接字所在目录必须属于当前用户且组 / 其他用户不可写（Host 新建的目录为 `0700`），套接字文件为 `0600`；
  - Host 对每个连接读取对端凭据（Linux `SO_PEERCRED`，macOS `getpeereid`），有效用户 ID 与 Host 不同则直接关闭；
  - SDK 连接后同样核对监听方的有效用户 ID，不同则断开并按连接失败处理（防止他人抢占路径冒充 Host）。
- **Windows 命名管道**：
  - 管道的安全描述符为 `O:<用户 SID>D:P(A;;GA;;;<用户 SID>)`：所有者是当前用户，只有当前用户可以打开；
    拒绝远程客户端（`PIPE_REJECT_REMOTE_CLIENTS`）；
  - SDK 打开管道后核对管道所有者 SID 与自己的用户 SID 相同（其他用户无法把对象所有者设为别人的 SID），
    不同则断开并按连接失败处理；客户端以 `SECURITY_IDENTIFICATION` 级别连接，Host 不能以 App 身份行事。
- 本地 IPC 连接由操作系统提供对端进程号（`SO_PEERCRED` / `LOCAL_PEERPID` / `GetNamedPipeClientProcessId`），
  Host 记录在实例信息中（Hub API 的 `InstanceInfo.pid`，spec/hub-api.md），不在协议消息中传递。
- 通过连接级校验后，`app/hello` 的处理对所有传输相同（IPC 连接通常不带 `Origin`，按原生 App 处理）。

### 1.5 单实例

同一个端点只能有一个 Host 监听：

- Unix：路径上已有套接字时 Host 先尝试连接——能连上说明另一个 Host 正在监听，启动失败（`AddrInUse`）；
  连接被拒绝说明是异常退出留下的文件，删除后重新绑定；路径上是普通文件时拒绝覆盖。Host 停止时删除自己创建的套接字文件。
- Windows：第一个管道实例以 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建，同名管道已存在时启动失败（`AddrInUse`）。
- `app-mcp-host serve` 遇到 `AddrInUse`（WebSocket 端口或 IPC 端点）时探测 MCP HTTP 端口的 `/healthz`：
  是健康的 app-mcp 则视为已在运行、以退出码 0 结束，否则报错。

## 2. 消息一览

| 方向 | 方法 | 类型 | 参数 → 结果 |
|---|---|---|---|
| SDK → Host | `app/hello` | 请求 | `HelloParams` → `HelloResult` |
| SDK → Host | `app/ready` | 通知 | `{}` |
| SDK → Host | `app/visibility` | 通知 | `VisibilityParams` |
| SDK → Host | `tools/sync` | 通知 | `ToolsSyncParams` |
| SDK → Host | `tools/changed` | 通知 | `ToolsChangedParams` |
| SDK → Host | `resources/sync` | 通知 | `ResourcesSyncParams` |
| SDK → Host | `resources/changed` | 通知 | `ResourcesChangedParams` |
| SDK → Host | `resources/updated` | 通知 | `ResourceUpdatedParams` |
| SDK → Host | `app/sleep` | 请求 | `SleepParams` → `SleepResult`（第 8 节） |
| Host → SDK | `tools/invoke` | 请求 | `ToolsInvokeParams` → `ToolsInvokeResult` |
| Host → SDK | `tools/cancel` | 通知 | `ToolsCancelParams` |
| Host → SDK | `resources/read` | 请求 | `ResourcesReadParams` → `ResourcesReadResult` |
| Host → SDK | `resources/subscribe` | 请求 | `ResourceSubscribeParams` → `{}` |
| Host → SDK | `resources/unsubscribe` | 请求 | `ResourceSubscribeParams` → `{}` |
| Host → SDK | `app/activate` | 请求 | `ActivateParams` → `{}` |
| Host → SDK | `app/pairingResult` | 通知 | `PairingResultParams` |
| Host → SDK | `app/lease` | 通知 | `LeaseParams`（第 8 节） |
| 双向 | `ping` | 请求 | 无参数 → `{}` |

所有字段名为 camelCase。可选字段缺省时不序列化。

## 3. 类型

```ts
type ClientKind = "web" | "native" | "hybrid"
type Risk = "read" | "write" | "destructive" | "payment" | "os-sensitive"   // 缺省 "write"
type Activation = "headless" | "background" | "foreground"
type Visibility = "visible" | "hidden" | "frozen"
type PairingStatus = "paired" | "pending" | "rejected"

interface HelloParams {
  appId: string            // [a-z][a-z0-9-]{0,62}
  appName: string
  protocolVersion: string  // 当前为 "1"
  sdkVersion: string
  clientKind: ClientKind
  instanceId: string       // 每个标签页 / 进程唯一，刷新后保持
  appVersion?: string
  origin?: string          // 网页来源
  instanceTitle?: string
  instanceUrl?: string
  token?: string           // 之前配对得到的 token
  launchToken?: string     // Host 唤醒时的一次性 token
  overview?: AppOverview   // App 总览（第 7 节）
  resumeToken?: string     // 上次 app/sleep 被接受时 Host 返回的恢复令牌（第 8 节）
  toolsHash?: string       // 当前工具与资源定义的摘要，与 resumeToken 一起发送（第 8.4 节）
  wakeReason?: WakeReason  // 本次连接的原因
}

type WakeReason = "os-activation" | "app" | "visible" | "cold-start"

interface AppOverview {
  summary: string          // 一句话简介，≤ 100 字符
  body?: string            // 总览正文（Markdown），≤ 2000 字符
  locale?: string          // 如 "zh-CN"
}

interface HelloResult {
  status: PairingStatus
  token?: string           // status 为 paired 时返回
  protocolVersion: string
  hostVersion: string
  reason?: string          // status 为 rejected 时的原因
  toolsCurrent?: boolean   // 缺省 false；true 时 SDK 跳过 tools/sync 与 resources/sync（第 8.3 节）
}

interface PairingResultParams { status: "paired" | "rejected"; token?: string; reason?: string }
interface VisibilityParams { visibility: Visibility; focused: boolean }
interface ActivateParams { mode: Activation }

interface ToolInfo {
  name: string             // [a-zA-Z0-9_.-]{1,64}，App 内唯一，不含 appId
  description: string
  inputSchema: object      // JSON Schema，type 必须为 "object"
  risk?: Risk
  activation?: Activation
  title?: string
}
interface ToolsSyncParams { tools: ToolInfo[] }
interface ToolsChangedParams { upserted: ToolInfo[]; removed: string[] }

interface ToolsInvokeParams { callId: string; name: string; arguments: object; timeoutMs?: number }
interface ToolsInvokeResult { data: unknown; stateHints?: string[] }
interface ToolsCancelParams { callId: string; reason?: string }

interface ResourceInfo { name: string; description: string; mimeType?: string }
interface ResourcesSyncParams { resources: ResourceInfo[] }
interface ResourcesChangedParams { upserted: ResourceInfo[]; removed: string[] }
interface ResourceUpdatedParams { name: string }
interface ResourcesReadParams { name: string }
interface ResourcesReadResult { contents: unknown; mimeType?: string }
interface ResourceSubscribeParams { name: string }

// 生命周期（第 8 节）
type SleepReason = "idle" | "grace" | "background" | "app"
type WakeKind = "uri" | "aumid" | "apple-event" | "dbus" | "android-intent" | "web-url" | "none"
interface WakeDescriptor {
  kind: WakeKind
  target?: string          // 各 kind 的定位信息（scheme、AUMID、D-Bus 名、组件名、URL 等）
  background?: boolean     // 能否不把窗口带到前台就唤醒，缺省 false
}
interface SleepParams { reason: SleepReason; wake?: WakeDescriptor; toolsHash: string }
interface SleepResult { accepted: boolean; resumeToken?: string; retryAfterMs?: number }
interface LeaseParams { ttlMs: number }   // 0 表示取消租约
```

### 3.1 名称规则：局部名与全名

- SDK 注册、`tools/sync`、`tools/invoke`、`resources/*` 以及清单 `tools[].name` / `resources[].name` 中的名称都是
  **App 内的局部名**：`[a-zA-Z0-9_.-]{1,64}`，可以含 `.` 分组（如 `cart.checkout`），**不含 appId**。
- Host 对模型暴露的**全名** = `<appId>.<局部名>`（如 App `shop` 的 `cart.checkout` → `shop.cart.checkout`）。
  拼接只在 Host 一处进行；SDK、构建工具、清单都不写 appId 前缀。
- 局部名以 `<appId>.` 开头在协议上仍合法（按原样拼接），但几乎总是误把全名当成局部名（全名会变成
  `shop.shop.info`）。核心在注册时、Host 在收到同步 / 加载清单时给出警告；`@app-mcp/build` 的注释扫描直接报错。

## 4. 错误

失败统一用 JSON-RPC 错误对象返回，`data.kind` 为错误类别：

| kind | code | 含义 |
|---|---|---|
| `TOOL_NOT_FOUND` | -32001 | 工具不存在 |
| `TOOL_DISABLED` | -32002 | 工具存在但被禁用 |
| `INVALID_INPUT` | -32003 | 参数不符合 inputSchema |
| `USER_REJECTED` | -32004 | 用户拒绝 |
| `TIMEOUT` | -32005 | 超时 |
| `HANDLER_ERROR` | -32006 | handler 出错 |
| `CANCELLED` | -32007 | 被取消 |
| `APP_DISCONNECTED` | -32008 | App 未连接 |
| `APP_NOT_INSTALLED` | -32009 | App 未安装 |
| `LAUNCH_FAILED` | -32010 | 唤醒失败 |
| `APP_NOT_RESPONDING` | -32011 | UI 线程无响应 |
| `INSTANCE_FROZEN` | -32012 | 页面被冻结 |
| `RESOURCE_NOT_FOUND` | -32013 | 资源不存在 |
| `UNAUTHORIZED` | -32014 | 未配对 |
| `UNSUPPORTED_PROTOCOL` | -32015 | 协议版本不兼容 |

`message` 面向模型，应说明原因和建议的下一步。`data` 中除 `kind` 外可携带其他字段。
标准 JSON-RPC 错误码（-32700、-32600、-32601、-32602、-32603）用于协议层错误。

## 5. SDK 行为（`app-mcp-core` 实现，所有语言一致）

### 5.1 连接与握手

1. 连接建立后，SDK 立即发送 `app/hello`（在收到结果前不发送其他消息）。
   `handshakeTimeoutMs`（默认 10s，0 表示不限）内没有收到结果：关闭连接并进入重连（5.6）。
   进入 `PendingPairing` 后不再受此限制。
2. 结果为 `paired`：
   1. 保存 `token`（若返回）；与配置中的 token 不同时通知驱动层持久化。
   2. 依次发送 `tools/sync`、`resources/sync`（全量，只含已启用的工具）。
      若本次 `app/hello` 携带了 `resumeToken` 且结果为 `toolsCurrent: true`，跳过这两条（第 8.3 节）。
   3. 发送 `app/visibility`（当前值）。
   4. 发送 `app/ready`，状态变为 `Connected`。
   5. 清零重连计数。
3. 结果为 `pending`：状态变为 `PendingPairing`，等待 `app/pairingResult`；
   收到 `paired` 后执行第 2 步，收到 `rejected` 按第 4 步处理。
4. 结果为 `rejected`，或 `app/hello` 返回错误：状态变为 `Rejected`，关闭连接，不再自动重连。
5. 握手期间 Host 发来的 `tools/invoke` 等请求返回 `UNAUTHORIZED` 错误。

### 5.2 注册变更

- 未连接时，注册 / 注销 / 启用状态变化只更新本地注册表，连接后通过全量同步发送。
- 已连接时，同一轮内的多次变更合并为一条 `tools/changed`（和 / 或 `resources/changed`），
  在驱动层下一次取事件时发出。同一名称先加后删则两者抵消，不发送。
- 工具被禁用等同于从 Host 的视角移除（出现在 `removed` 中），重新启用等同于新增。

### 5.3 调用

- 收到 `tools/invoke`：
  - 名称不存在 → `TOOL_NOT_FOUND`；存在但禁用 → `TOOL_DISABLED`。
  - 否则进入调用队列。正在执行的调用数小于 `maxConcurrentCalls`（默认 1）时立即执行，
    否则排队，按到达顺序执行。
- `timeoutMs` 从收到请求时开始计时（包含排队时间）。超时后取消 handler，返回 `TIMEOUT`。
- 收到 `tools/cancel`：取消对应调用（排队中的直接移出），返回 `CANCELLED`。
- handler 完成后返回 `ToolsInvokeResult`；handler 出错返回其错误（缺省类别 `HANDLER_ERROR`）。
- 已取消或已超时的调用，其后到达的完成结果被丢弃。
- 连接断开时取消所有进行中和排队的调用，不发送任何响应。
- SDK 主动停止（`stop`）或休眠时，已排队但尚未交给驱动层的消息（如刚完成的调用结果）
  先于关闭连接发出；连接已断开时则丢弃。

### 5.4 资源

- 收到 `resources/read`：资源不存在 → `RESOURCE_NOT_FOUND`；否则请求驱动层读取并返回。
- `resources/subscribe` / `unsubscribe` 维护订阅集合，返回 `{}`；资源不存在 → `RESOURCE_NOT_FOUND`。
  订阅集合在断线后清空，由 Host 重新订阅。
- 资源内容变化时，仅对已订阅资源发送 `resources/updated`；同一资源两次通知之间
  至少间隔 `resourceUpdateThrottleMs`（默认 100ms），节流期内的多次变化合并为一次。

### 5.5 心跳

- `Connected` 状态下，每隔 `heartbeat.intervalMs`（默认 15s）发送 `ping`。
- 超过超时时间（可见时 `timeoutMs` 默认 10s；隐藏或冻结时 `hiddenTimeoutMs` 默认 120s）
  未收到响应，视为断开：关闭连接并进入重连。
- 收到 Host 的 `ping` 请求，立即返回 `{}`。收到任何消息都不重置心跳计时，只有 `ping` 的响应才算。

### 5.6 重连

- 非 `Stopped` / `Rejected` / `Dormant` 状态下连接断开，进入 `Backoff`，
  延迟为 `min(initialDelayMs * multiplier^n, maxDelayMs)`（默认 500ms 起，×2，最大 30s），
  n 为自上次成功握手以来的重试次数。
- 到期后请求驱动层重新连接。

### 5.7 其他

- 无法解析的消息、未知通知：忽略并记录警告。
- 未知方法的请求：返回 `-32601`。
- 参数无法解析的请求：返回 `-32602`。
- 未知 ID 的响应：忽略并记录警告。

### 5.8 连接状态

`Idle` → `Connecting` → `Handshaking` →（`PendingPairing`）→ `Connected`；断线 `Backoff`；
终态 `Rejected`、`Stopped`。生命周期（第 8 节）新增：

| 状态 | 含义 |
|---|---|
| `Dormant` | 已与 Host 完成 `app/sleep` 握手后断开（或 `on-demand` 模式启动后尚未连接）。不重连、无定时器，注册表保留 |
| `Waking` | 收到唤醒后正在建立连接（等同于 `Connecting`），之后进入 `Handshaking` |

`app/sleep` 发出到收到结果之间为内部过渡态 `sleeping`，对外仍为 `Connected`。

## 6. Host 行为（M1）

- 对每个连接执行握手（先按 1.4 做连接级校验）；M1 对来自回环地址或本地 IPC、`Origin` 在允许列表内（默认 `http://localhost:*`、
  `http://127.0.0.1:*`）或无 `Origin` 的连接直接返回 `paired` 并分配随机 token。
  其他 `Origin` 返回 `rejected`。
- `protocolVersion` 不为 `"1"` 时返回 `rejected`，`reason` 说明版本不兼容。
- 同一 `appId` 可有多个实例（`instanceId` 区分）；同一 `instanceId` 重复连接时，新连接替换旧连接。
- 向 SDK 发送 `tools/invoke` 前，已按 inputSchema 校验参数；默认 `timeoutMs` 为 30000。
- 每隔 15s 向 SDK 发送 `ping`；45s 内没有收到 SDK 的任何消息则关闭连接。
  实例处于 `hidden` / `frozen` 时（浏览器会限流后台页面的定时器），该超时放宽为 180s。
- 返回 `rejected` 的握手结果发送后，Host 关闭连接。
- TCP 只接受来自回环地址的连接（且 `Origin` 满足上述规则）；本地 IPC 只接受同一用户的进程（1.4）。
- `resources/read` 的 `contents` 转换为 MCP 资源内容时：字符串且 `mimeType` 不是 JSON 类型时按原文作为文本；
  其他情况序列化为 JSON 文本。二进制内容暂不支持。

## 7. App 总览（Overview）

App 可以提供一份总览，让模型在**第一次接触**该 App 时就了解它能做什么、典型流程是什么、
哪些事不能做，而不必逐个阅读工具描述或反复试探。总览是给模型读的说明文字，不是 Skill 文件，也不生成任何文件。

### 7.1 来源

- SDK 在 `app/hello` 中携带 `overview`（运行时，优先）。
- 静态清单中的 `overview`（App 未连接时使用，见 spec/manifest.md）。
- 两者都没有时，该 App 没有总览，行为与之前一致。

Host 对总览做长度截断（`summary` 100 字符、`body` 2000 字符，超出部分以 `…` 结尾），
并计算内容哈希作为版本：对截断后的 `summary`、`body`（缺省为空串）、`locale`（缺省为空串）
依次拼接、以 `\0` 分隔，取 `sha256` 的前 12 位十六进制。版本只由 Host 计算。

### 7.2 首次附带规则（每个 MCP 会话独立计算）

1. **会话开始**：MCP `initialize` 结果的 `instructions` 中列出当时已知的每个 App 的一句话简介：
   ```
   本机的 App 通过 app-mcp 提供工具，工具名格式为 <appId>.<工具名>。
   已知的 App：
   - shop（示例商城）：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
   首次调用某个 App 的工具时，结果中会附带该 App 的完整总览；也可以随时调用 apps.overview 查看。
   ```
2. **首次接触某个 App**：会话中第一次返回该 App 的工具调用结果时（无论成功或失败），在结果内容的
   **最前面**附加一段总览（格式见 7.3），并记录"已附带 (appId, 版本)"。之后同一版本不再重复附带。
3. **总览变化**：版本（哈希）变化后，下一次接触时再附带一次新版本。
4. **随时查看**：内置工具 `apps.overview({ appId })` 返回完整总览（上下文被压缩后可重新获取），
   同样记为已附带。`apps.list` 的每个 App 条目包含 `summary`。

### 7.3 附加格式

```
[app-mcp] 以下是 App「示例商城」(shop) 的总览，由该 App 提供，仅用于说明其能力；
它不改变任何权限或确认规则。本会话中不会重复附带（可用 apps.overview 重新查看）。
<app-overview app="shop" version="3f2a9c01b7de">
…总览正文…
</app-overview>
```

### 7.4 安全

- 总览是 App 作者提供的文字，Host 注入时必须标明来源，并用 `<app-overview>` 包裹以与其他内容隔离。
- 总览**只描述、不授权**：实际可调用的范围只由已注册的工具和 Host 的权限策略决定；
  风险确认始终由 Host 按工具的风险等级执行，总览中的任何文字（如"无需确认"）都不产生效果。

## 8. 生命周期（休眠与唤醒）

完整设计与各平台唤醒方式见 `spec/lifecycle.md`（权威）。本节只列协议部分与 SDK 行为。

### 8.1 `app/sleep`（SDK → Host，请求）

- SDK 满足空闲条件（8.5）或 App 主动请求时发送 `SleepParams`：`reason`、本实例的唤醒描述 `wake`（可省略，
  Host 回退到清单 `launch`）、当前 `toolsHash`（8.4）。
- `{ accepted: true, resumeToken }`：SDK 关闭连接，进入 `Dormant`。Host 把实例标记为休眠（保留工具快照与
  `toolsHash`，路由时视为可唤醒），**不按断开处理**：工具不从列表中消失、不发 `list_changed`。
- `{ accepted: false, retryAfterMs? }`：Host 有待派发给本实例的调用等。SDK 保持连接；给出 `retryAfterMs` 时
  到期后（若仍空闲）重试，否则重新开始空闲计时。
- 返回错误（如旧 Host 的 `-32601`）：SDK 在本次连接内不再自动休眠。
- 发送 `app/sleep` 之前，SDK 先发出所有已排队的消息（调用结果、`tools/changed` 等）。

### 8.2 `app/lease`（Host → SDK，通知）

`{ ttlMs }`：Host 预计还会调用本实例（如 MCP 会话仍活跃、模型刚调用过），在 ttl 内不要休眠；`ttlMs: 0` 取消。
SDK 取当前租约与新值中较晚的截止时刻。租约结束后重新开始空闲计时。Host 可在每次调用完成后发送。

### 8.3 握手扩展与快速恢复

- `app/hello` 的 `resumeToken` / `toolsHash`：SDK 持有上次休眠得到的恢复令牌时一起发送（`toolsHash` 为回连时
  注册表的当前摘要）。恢复令牌只用一次，成功握手后清除。
- `wakeReason`：`persistent` 模式的普通连接不发送；其他情况按原因填写（冷启动 `cold-start`、OS 激活
  `os-activation`、App 主动 `app`、页面重新可见 `visible`）。
- Host 在令牌有效且 `toolsHash` 与休眠时一致时返回 `toolsCurrent: true`，SDK 跳过 `tools/sync` /
  `resources/sync`，直接 `app/visibility` → `app/ready`，并以当前注册表作为 Host 已知快照继续增量追踪。
  否则返回 `toolsCurrent: false`（或省略），SDK 走完整同步。休眠期间的注册变更会使摘要不一致，从而触发完整同步。
- 唤醒令牌：Host 唤醒时生成一次性 `wakeToken`（≥ 128 位随机，60 秒有效，字符限于 `[A-Za-z0-9._~-]`），
  通过激活参数传给 App；SDK 在 `app/hello.launchToken` 中携带。SDK 识别的激活参数形式：
  `app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=<token>`（及 `<scheme>:app-mcp/wake?token=`）、
  URL 片段 `#app-mcp-wake=<token>`。

### 8.4 工具摘要（toolsHash）

`sha256(规范化 JSON({"resources": R, "tools": T}))` 的前 16 个十六进制字符。`T`、`R` 为 `tools/sync`、
`resources/sync` 参数中的数组（只含已启用的工具），按 `name` 的字节序排序；规范化 JSON 为对象键按字节序排序、
无空白，字符串按 JSON 标准转义（非 ASCII 原样输出）。固定向量（`crates/protocol/src/hash.rs`）：

- 空注册表：`69c61b185225ee82`
- 工具 `todo.add`（描述"添加待办"，schema `{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}`，
  risk `write`）、`cart.checkout`（描述"结算"，schema `{"type":"object"}`，risk `payment`，activation `foreground`，
  title `Checkout`），资源 `cart.state`（描述"购物车"）：`ba703035ddca2f91`

### 8.5 SDK 行为

- 模式：`persistent`（默认，不休眠）/ `idle`（启动即连接，空闲 `idleTimeoutMs` 后休眠）/
  `on-demand`（启动时不连接，进入 `Dormant`；被唤醒或 `connectNow()` 时连接，空闲 `graceMs` 后休眠）。
  隐藏 / 冻结时使用 `min(模式超时, hiddenIdleTimeoutMs)`。
- 空闲条件（全部满足才开始计时，任一变化重置计时）：没有进行中或排队的调用、资源读取；没有有效租约；
  没有资源订阅；没有 App 的持有（`hold()`，含调用上的 `hold`）。可见性变化也重新计时。
- 自动休眠（空闲计时到期）的 `reason`：`on-demand` 为 `grace`，否则 `idle`——可见性只决定计时长短，不改变原因。
  `background` 专指"进入后台立即休眠"（网页 bfcache `pagehide(persisted)`、移动端进入后台），由封装层显式发起。
  App 显式 `sleep()` 为 `app`（不看空闲条件与持有；被拒后按 `retryAfterMs`，缺省 5s 重试）。
- `Dormant`：无连接、无定时器（`poll_timeout()` 为空）；收到的注册变更只更新本地注册表，不唤醒。
- 唤醒：`handleWake(args)` 识别到令牌 → `Waking` 并连接（未 `start` 时记录，`start` 时连接；`Backoff` 时立即重连）；
  `wake()` / `connectNow()` 同理（原因 `app`）。休眠握手进行中收到唤醒或持有：休眠完成后立即回连。
- 驻留：`residency` 为 `exit-always`，或为 `exit-when-idle` 且本进程由唤醒冷启动（配置带 launch token，或首次
  握手前收到唤醒）时，进入 `Dormant` 后通知 App（`onIdleExit`），由 App 决定是否退出。
