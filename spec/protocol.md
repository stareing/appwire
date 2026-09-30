# app-mcp SDK ↔ Host 协议规范 v1

本文件是 SDK（App 内的客户端）与 Host（本地 MCP Host）之间通信协议的权威定义。
类型定义见 `crates/protocol`，两者必须保持一致；修改任一方都要同步修改另一方。

Host 对模型一侧使用标准 MCP，不在本规范范围内。

## 1. 传输

- 消息为 UTF-8 JSON 文本，一条消息对应一个 WebSocket 文本帧（或本地 socket 上的一行，M2）。
- 每条消息是一个 JSON-RPC 2.0 对象；不支持批量（数组）消息。
- M1 只有 WebSocket，Host 默认监听 `ws://127.0.0.1:7717`。
- 双方都可以发送请求、通知和响应。请求 ID 由发送方生成，只需在发送方内唯一。

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

- 对每个连接执行握手；M1 对来自回环地址、`Origin` 在允许列表内（默认 `http://localhost:*`、
  `http://127.0.0.1:*`）或无 `Origin` 的连接直接返回 `paired` 并分配随机 token。
  其他 `Origin` 返回 `rejected`。
- `protocolVersion` 不为 `"1"` 时返回 `rejected`，`reason` 说明版本不兼容。
- 同一 `appId` 可有多个实例（`instanceId` 区分）；同一 `instanceId` 重复连接时，新连接替换旧连接。
- 向 SDK 发送 `tools/invoke` 前，已按 inputSchema 校验参数；默认 `timeoutMs` 为 30000。
- 每隔 15s 向 SDK 发送 `ping`；45s 内没有收到 SDK 的任何消息则关闭连接。
  实例处于 `hidden` / `frozen` 时（浏览器会限流后台页面的定时器），该超时放宽为 180s。
- 返回 `rejected` 的握手结果发送后，Host 关闭连接。
- 只接受来自回环地址的连接（且 `Origin` 满足上述规则）。
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
- 自动休眠的 `reason`：`on-demand` 为 `grace`；实例隐藏 / 冻结时为 `background`；否则 `idle`。
  App 显式 `sleep()` 为 `app`（不看空闲条件与持有；被拒后按 `retryAfterMs`，缺省 5s 重试）。
- `Dormant`：无连接、无定时器（`poll_timeout()` 为空）；收到的注册变更只更新本地注册表，不唤醒。
- 唤醒：`handleWake(args)` 识别到令牌 → `Waking` 并连接（未 `start` 时记录，`start` 时连接；`Backoff` 时立即重连）；
  `wake()` / `connectNow()` 同理（原因 `app`）。休眠握手进行中收到唤醒或持有：休眠完成后立即回连。
- 驻留：`residency` 为 `exit-always`，或为 `exit-when-idle` 且本进程由唤醒冷启动（配置带 launch token，或首次
  握手前收到唤醒）时，进入 `Dormant` 后通知 App（`onIdleExit`），由 App 决定是否退出。
