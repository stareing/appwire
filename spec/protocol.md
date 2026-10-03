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
| `ws://<host>:<port>/app` / `wss://…` | WebSocket over TCP | 网页（只能用这种）；原生 App 显式配置时 |
| `unix:<绝对路径>` | WebSocket over Unix 域套接字 | Linux、macOS 原生 App（默认） |
| `pipe:\\.\pipe\<名称>` | WebSocket over Windows 命名管道 | Windows 原生 App（默认） |

- TCP 与本地 IPC 上都是同一个 HTTP/1.1 服务（1.3），App 连接是其中路径 `/app` 上的 RFC 6455 WebSocket 升级。
  本地 IPC（`unix:` / `pipe:`）上客户端发送的握手 URL 固定为 `ws://localhost/app`（Host 不检查 `Host` 头），
  不使用 TLS；之后的帧、心跳（`ping`）、Close 与 TCP 上逐字节相同。这样 SDK 核心、Host 的消息处理与超时逻辑
  对所有传输只有一份实现。
- 兼容期：合并端口之前的 SDK 以根路径 `/` 升级（`ws://127.0.0.1:7717`、IPC 上 `ws://localhost/`），Host 仍按 `/app`
  处理，并在首次出现时记录一条提示日志；新 SDK 一律使用 `/app`。
- 格式不合法、或当前平台不支持该形式（如 Windows 上的 `unix:`）时，SDK 在创建客户端时报配置错误。
- 实现：`app_mcp_protocol::endpoint`（解析、默认位置）。

### 1.3 默认端点

Host 默认监听：

- **HTTP 服务** `127.0.0.1:7717`（回环 TCP），同一端口按路径分流：

  | 路径 | 内容 | 校验 |
  |---|---|---|
  | `/app` | WebSocket 升级 → App 连接（本规范的消息） | `Origin` 在 `app/hello` 时按允许列表 / 配对处理（第 6 节） |
  | `/mcp` | MCP Streamable HTTP（`app-mcp-host serve` 开启；嵌入式 Hub 可选） | `Origin` 允许列表（403）→ 本地访问令牌或已登记 Agent 的令牌（401；Agent 令牌同时确定请求的主体，spec/hub-api.md 3.6「Agent 身份」） |
  | `/healthz` | `GET`：Host 身份与监听信息（1.6），不需要令牌 | `Origin` 允许列表（403） |
  | `/status` | `GET`：运行状态（各 App 实例的连接 / 休眠 / 唤醒、最近错误、SDK 上报，10.2），供 `app-mcp-host doctor` / `status` | 本地 IPC 直接允许；TCP 必须带有效令牌（未配置令牌时 403，请经 IPC 访问） |
  | `/policy` | `POST`：替换策略规则（spec/hub-api.md 3.13），供 `app-mcp-host policy reload` | 同 `/status` |
  | `/agents` | `POST`：替换已登记的 Agent（spec/hub-api.md 3.6「Agent 身份」），供 `app-mcp-host agent add / remove / reload` | 同 `/status`（Agent 令牌不能访问） |

  未显式配置监听地址且默认端口被占用时，Host 依次尝试**固定的备选端口** `7737`、`7757`，实际地址写入登记文件（1.7）。
  合并之前的独立 MCP 端口 `7718` 不再默认监听；兼容期内可显式配置（`app-mcp-host` 的 `http.addr` / `--http`，
  另开一个提供同样路径的监听器），启动时记录弃用提示。
- **本地 IPC**（平台默认 IPC 端点）：
  - Linux：`$XDG_RUNTIME_DIR/app-mcp/hub.sock`；未设置 `XDG_RUNTIME_DIR` 时 `~/.app-mcp/run/hub.sock`；
  - macOS：`~/.app-mcp/run/hub.sock`（设置了 `XDG_RUNTIME_DIR` 时同 Linux）；
  - Windows：`\\.\pipe\app-mcp-<当前用户 SID>`（如 `\\.\pipe\app-mcp-S-1-5-21-…-1001`）；
  - Android / iOS / 鸿蒙（HarmonyOS NEXT / OpenHarmony）：无（App 沙箱之间不能共享套接字，这些平台用 WebSocket，如 Android 经
    `adb reverse tcp:7717 tcp:7717`、鸿蒙经 `hdc rport tcp:7717 tcp:7717`）。鸿蒙目标（`aarch64-` / `x86_64-unknown-linux-ohos`）的
    `target_os` 是 `linux`，按 `target_env = "ohos"` 识别；平台分类的唯一实现是 `app_mcp_protocol::platform::Target`
    （`is_app_sandboxed` / `shares_host_filesystem` / `default_ipc_kind`）。

  IPC 上是同一个 HTTP 路由：`/app`、`/healthz`、`/status`，以及开启 MCP 时（`app-mcp-host serve`；嵌入式 Hub 的
  `HubConfig.mcp_http`）的 `/mcp`——供厂商 Agent、支持本地套接字的 MCP 客户端使用（如 rmcp 的 `UnixSocketHttpClient`，
  请求 URL 用 `http://localhost/mcp`）。IPC 上的 `/mcp` **不校验令牌**：连接级鉴权（1.4）已确认对端是同一用户，
  而同一用户本来就能读取令牌文件；客户端应像 SDK 一样核对监听方是同一用户。不提供 stdio→HTTP 的转发程序
  （`app-mcp-host stdio` 是独立的单客户端 Host，与常驻 Host 互斥，见 crates/host/README.md）。

原生 SDK 未配置端点时按以下顺序**确定**端点（创建配置时解析一次）：

1. 环境变量 `APP_MCP_ENDPOINT`（非空时原样使用，不合法则报配置错误）；
2. 登记文件（1.7）中运行中的 Host 写下的端点：有本地 IPC 端点时用它，否则 `ws://<listen>/app`（沙箱平台——Android / iOS /
   鸿蒙——不与 Host 共享文件系统，跳过此步）；
3. 平台默认 IPC 端点（同上）；
4. `ws://127.0.0.1:7717/app`（平台没有默认 IPC 端点时）。

这是配置的解析顺序，**不是连接失败后的回退**：选定的端点连不上时，SDK 按 5.6 退避重连同一个端点，
不会自动换用其他传输。Host 关闭了 IPC 服务或改了 IPC 端点时，登记文件给出实际端点；也可设置 `APP_MCP_ENDPOINT`
或显式配置端点。

网页 SDK 的默认端点为 `ws://127.0.0.1:7717/app`。未显式指定 `hostUrl` 时按 Host 相同的顺序依次尝试
`7717 → 7737 → 7757`：连接建立不了、或握手结果判定"不是 app-mcp"（1.6）时换下一个；成功握手后固定在该端口
（之后断线先重试同一端口）；三个都不是 app-mcp 时停在 `HostMismatch`（5.8）。浏览器读不到登记文件，也不知道
操作系统用户，因此网页只核对 `service`、不核对 `user`（多用户机器上应显式指定 `hostUrl`）。显式指定 `hostUrl` 时只连它。

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

- **单实例锁**：`app-mcp-host` 在任何监听之前以独占、非阻塞方式锁定 `<配置目录>/run/hub.lock`
  （Unix `flock`，Windows `LockFileEx`；配置目录为 `--home` > `APP_MCP_HOME` > `~/.app-mcp`）。锁随进程退出由
  操作系统释放，异常退出也不会残留。已被锁定时 `serve` 读登记文件（1.7），打印已运行实例的信息并以**退出码 0**
  结束（可放心重复执行）；stdio 模式报错并给出该实例的 MCP 地址。嵌入式 Hub 经 `HubConfig.run_dir` 选择参与
  （spec/hub-api.md 3.6）。
- 同一个端点只能有一个监听者（不同配置目录的两个 Host，或其他程序）：
  - Unix 套接字：路径上已有套接字时 Host 先尝试连接——能连上说明另一个 Host 正在监听，启动失败（`AddrInUse`）；
    连接被拒绝说明是异常退出留下的文件，删除后重新绑定；路径上是普通文件时拒绝覆盖。Host 停止时删除自己创建的套接字文件。
  - Windows：第一个管道实例以 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建，同名管道已存在时启动失败（`AddrInUse`）。
  - TCP：显式配置的地址被占用即失败；`serve` 探测该地址的 `/healthz` 说明占用者（另一个 app-mcp Host 的 pid 与用户，
    或"其他程序"），退出码 1。

### 1.6 Host 身份

`app/hello` 的结果、`/healthz` 与登记文件都带 Host 身份（`app_mcp_protocol::identity::HostIdentity`）：

| 字段 | 含义 |
|---|---|
| `service` | 固定 `"app-mcp"` |
| `version` | Host 版本（握手结果中为既有字段 `hostVersion`） |
| `user` | Host 进程的操作系统用户：Unix 为十进制有效 uid，Windows 为用户 SID |
| `pid` | Host 进程号 |

SDK 核心在处理握手结果之前核对（`app_mcp_protocol::identity::check_hello`）：

- `service` 存在且不是 `"app-mcp"`、`app/hello` 返回 `-32601`（方法不存在）、或结果无法解析为 `HelloResult` →
  对端不是 app-mcp Host；
- 原生 SDK（桌面平台）以自己的用户为期望用户，结果中的 `user` 与之不同 → Host 属于其他用户（防止连到本机其他用户
  占用的端口）。Android / iOS（Host 在另一台机器上）与网页不核对用户。

两种情况都进入 `HostMismatch`（5.8）：断开、**不再自动重试**，原因写在状态的 `reason` 中；App 调用 `wake()` /
`connectNow()` 时再试一次。旧 Host 不带这些字段：无法核对，按通过处理。

### 1.7 登记文件

持有单实例锁的 Host 在绑定完成后把实际监听位置原子写入 `<配置目录>/run/endpoints.json`
（先写临时文件再改名；Unix 权限 `0600`，目录 `0700` 且必须属于当前用户），正常退出时删除；异常退出留下的旧文件
由下一个取得锁的 Host 覆盖：

```json
{ "service": "app-mcp", "version": "0.1.0", "user": "1000", "pid": 4242,
  "listen": "127.0.0.1:7717", "ipcEndpoint": "unix:/run/user/1000/app-mcp/hub.sock", "startedAtMs": 1790000000000 }
```

`listen` / `ipcEndpoint` 未开启时省略。读者：原生 SDK 的端点解析（1.3）、`app-mcp-host service status|start|stop`
（登记文件 + `/healthz` 进程号一致才算"本配置目录的实例在运行"）、测试（监听端口 0，从登记文件取实际地址）。
实现：`app_mcp_protocol::registry`（路径与读取）、`crates/hub/src/instance.rs`（加锁与写入）。

### 1.8 名字服务通道（按名寻址，spec/naming.md）

除 SDK 拨号到 1.2 的端点之外，Hub 也可以经系统名字服务**拨入** App（Linux：向会话总线名 `dev.appmcp.App.<appId>` 调用
`dev.appmcp.App1.Open() → h`，得到 socketpair 的一端；地址、映射与发现见 spec/naming.md）。这条通道上：

- **帧与角色不变**：仍是 1.1 的 HTTP/1.1 + WebSocket，SDK 发送 `ws://localhost/app` 的升级请求、是 WebSocket 客户端，Hub 是服务端；
  连接建立后 SDK 先发 `app/hello`，`wakeReason` 为 `"os-activation"`，握手、同步、调用全部按本规范。
- **鉴权**：两端都核对对端为同一操作系统用户（App 在 `Open()` 中查调用方 uid，Hub 用通道对端的 `SO_PEERCRED`），与 1.4 相同的信任边界。
- **心跳与关闭**：SDK 声明 `heartbeatMs: 0`（不论端点是何种传输），Hub 也不发 `ping`、不做无消息断开；SDK 不做空闲计时、不发自动的
  `app/sleep`（`app/sleep { reason: "app" }` 仍可用），关闭时机由 Hub 决定（spec/naming.md 7.2）。通道断开后 SDK 不重连：
  `persistent` 回到 5.6 的重连，其余模式进入 `Dormant`（`Residency` 规则同 8.5）。
- **同时只接受一条**：App 已有连接（含建立中）时拒绝拨入（D-Bus 错误 `org.freedesktop.DBus.Error.LimitsExceeded`，说明以
  `CHANNEL_LIMIT` 开头；spec/naming.md U-16）。
- **错误码**：名字服务相关的错误码（spec/naming.md 第 12 节，`app_mcp_protocol::naming::codes`）暂不并入 10.1 的连接级错误码表，
  只出现在工具错误的 `details.code`（`LAUNCH_FAILED` / `APP_NOT_INSTALLED`）、Hub `last_error` 与日志中；并入时字符串不变。
- 实现：`app_mcp_core::Client::accept_channel`（核心）、`app_mcp_native::NativeConfig::register_name`（原生运行时登记与接受）、
  `app_mcp_hub::connector`（Hub 拨号）。

### 1.9 Host 按需启动（第 4f 项 f / 4g b）

Host 自身也可以"召之即来，挥之即去"：服务管理器代为持有 1.3 的 TCP 监听与 1.2 的本地 IPC 套接字，**第一个连接到来时**启动
`app-mcp-host serve`，Host 空闲后退出。对 SDK 与 MCP 客户端完全透明——端点、帧、握手都不变，只是连接可能多等一次 Host 冷启动。

- **安装**：`app-mcp-host service install --on-demand`（缺省仍为登录自启）。
  - Linux：`~/.config/systemd/user/app-mcp-host.socket`（`ListenStream=<IP:端口>` 与 `ListenStream=<IPC 路径>`，`SocketMode=0600`、
    `DirectoryMode=0700`，`WantedBy=sockets.target`）+ 不随登录启动的 `app-mcp-host.service`（`Requires=` 套接字单元，没有 `[Install]`）。
  - macOS：LaunchAgent plist 的 `Sockets`（键 `Http`：`SockNodeName` / `SockServiceName` / `SockFamily`；键 `Ipc`：`SockPathName`、
    `SockPathMode` 384 = `0600`），不设 `RunAtLoad` / `KeepAlive`（`KeepAlive.SuccessfulExit` 隐含 `RunAtLoad`）。IPC 目录由安装命令以 `0700` 创建。
  - 监听地址必须是 `IP:端口` 且端口不为 0（systemd 不绑定 `…:0`；客户端需要固定地址才能"连接即启动"）。
  - Windows：没有与套接字激活对等的机制（命名管道与 TCP 都不能交给系统代为监听并按需拉起普通进程），保持登录自启。
- **取得套接字**：Linux 按 `sd_listen_fds(3)` 协议读 `LISTEN_PID`（必须等于本进程，否则视为给别的进程的变量而忽略）/ `LISTEN_FDS`
  （fd 从 3 起，上限 8）/ `LISTEN_FDNAMES`（只用于日志），取到后清除这三个变量并给 fd 设 `FD_CLOEXEC`（上游 MCP 子进程不继承）；
  macOS 以 `launch_activate_socket("Http" / "Ipc")` 取得（不是由 launchd 启动、或没有该键时视为未激活）。交来的 fd 按地址族分类：
  IPv4 / IPv6 → HTTP 监听，Unix → 本地 IPC；必须是监听中的流套接字，同类多于一个、变量不是数字时**启动失败**。
  IPC 套接字所在目录同样必须属于当前用户且组 / 其他用户不可写（1.4），每个连接仍核对对端用户。
- **只服务交来的监听**：激活时 `listen` / `ipcEndpoint` 配置被交来的套接字取代；没有交来的那一个**不自己绑定**（自己绑定的端口在空闲退出后
  无人监听，也不会触发再次启动）。Host 不删除交来的套接字文件（归服务管理器）。单实例锁与登记文件（1.5、1.7）照常：登记文件写交来的地址。
  同一配置目录已有手动运行的 Host（锁被占用）时激活的实例**以失败退出**（以 0 退出会让服务管理器因队列中的连接立即再次启动它）。
- **空闲退出**（`lifecycle.idleExitMs` / `--idle-exit-ms`，缺省 600000 = 10 分钟，0 = 不退出；只在套接字由服务管理器交来时生效）：
  - 占用：已接受、尚未结束的连接（含其上的请求、SSE 流、升级后的 App WebSocket，TCP 与 IPC 都算），以及进行中的调用、进行中的唤醒、
    在线 App 实例（含按名拨入的通道）、MCP 会话（`Mcp-Session-Id`）。连接进出由事件通知驱动，不轮询。
  - 没有占用并持续 `idleExitMs` 后：关闭接受闸门（正在等待的 `accept` 被取消，连接留在内核队列中），等所有接受循环都离开 `accept`，
    再复核一次占用与连接计数——期间漏进来的连接会被完整服务、闸门重新打开；确认无占用后正常停止（写出休眠记录、删除登记文件、退出码 0）。
  - 空闲期间只有这一个一次性倒计时；倒计时到点时若仍有非连接类占用（如按名拨入的通道），重新倒计时（每 `idleExitMs` 至多一次唤醒）。
  - 退出后排在监听套接字上的连接由服务管理器再次启动 Host 接受；休眠实例记录已持久化（spec/hub-api.md 3.5「持久化」），
    重启后休眠 App 仍列出、可唤醒。租约（spec/lifecycle.md 4.2）随 Host 退出失效，只影响保温，不影响正确性。
- `status` / `doctor` / `service install` 的就绪检查会连接监听地址，因此也会按需启动 Host（之后照常空闲退出）。
- 实现：`crates/host/src/activation.rs`（取得套接字）、`crates/host/src/service.rs`（单元 / plist）、`app_mcp_hub::PreboundListeners` +
  `Hub::start_with` / `Hub::wait_idle`（`crates/hub/src/activity.rs`：接受闸门与连接计数）、
  `app_mcp_protocol::naming::launchd::activate_socket`（与 App 侧共用的唯一 `launch_activate_socket` FFI）。

## 2. 消息一览

| 方向 | 方法 | 类型 | 参数 → 结果 |
|---|---|---|---|
| SDK → Host | `app/mux` | 请求（仅作连接上的第一条消息） | `MuxParams` → `MuxResult`（第 9 节） |
| SDK → Host | `app/hello` | 请求 | `HelloParams` → `HelloResult` |
| SDK → Host | `app/ready` | 通知 | `{}` |
| SDK → Host | `app/visibility` | 通知 | `VisibilityParams` |
| SDK → Host | `tools/sync` | 通知 | `ToolsSyncParams` |
| SDK → Host | `tools/changed` | 通知 | `ToolsChangedParams` |
| SDK → Host | `resources/sync` | 通知 | `ResourcesSyncParams` |
| SDK → Host | `resources/changed` | 通知 | `ResourcesChangedParams` |
| SDK → Host | `resources/updated` | 通知 | `ResourceUpdatedParams` |
| SDK → Host | `app/sleep` | 请求 | `SleepParams` → `SleepResult`（第 8 节） |
| SDK → Host | `app/diagnostic` | 通知 | `DiagnosticParams`（第 10 节） |
| SDK → Host | `tools/progress` | 通知 | `ToolsProgressParams`（3.3） |
| Host → SDK | `tools/invoke` | 请求 | `ToolsInvokeParams` → `ToolsInvokeResult` |
| Host → SDK | `tools/cancel` | 通知 | `ToolsCancelParams` |
| Host → SDK | `resources/read` | 请求 | `ResourcesReadParams` → `ResourcesReadResult` |
| Host → SDK | `resources/subscribe` | 请求 | `ResourceSubscribeParams` → `{}` |
| Host → SDK | `resources/unsubscribe` | 请求 | `ResourceSubscribeParams` → `{}` |
| Host → SDK | `app/activate` | 请求 | `ActivateParams` → `{}` |
| Host → SDK | `app/navigate` | 请求 | `NavigateParams` → `NavigateResult`（3.4；只发给声明了 `capabilities.navigate` 的实例） |
| Host → SDK | `app/pairingResult` | 通知 | `PairingResultParams` |
| Host → SDK | `app/lease` | 通知 | `LeaseParams`（第 8 节） |
| 双向 | `ping` | 请求 | 无参数 → `{}` |

所有字段名为 camelCase。可选字段缺省时不序列化。

## 3. 类型

```ts
type ClientKind = "web" | "native" | "hybrid"
type Risk = "read" | "write" | "destructive" | "payment" | "os-sensitive"   // 缺省 "write"；旧写法，新代码用 ToolAnnotations（3.2）
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
  heartbeatMs?: number     // SDK 心跳声明（5.5）：0 = 不发心跳、靠连接断开感知；> 0 = 每隔该毫秒数发 ping；省略 = 旧行为
  lifecycleMode?: LifecycleMode // SDK 的生命周期模式（8.5），供 Host 观测；省略 = 未知
  capabilities?: SdkCapabilities // 可选能力（3.4）；省略 = 都不支持（旧 SDK）
  wake?: WakeDescriptor    // 本实例的唤醒描述（同 app/sleep.wake，8.2）；Host 据此持久化在线实例，Host 异常退出重启后仍可唤醒；省略 = 未配置或旧 SDK
}

interface SdkCapabilities {
  navigate?: boolean       // 能处理 app/navigate（App 设置了导航回调）；缺省 false，false 时不序列化
}

type WakeReason = "os-activation" | "app" | "visible" | "cold-start"
type LifecycleMode = "persistent" | "idle" | "on-demand"

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
  service?: string         // Host 身份（1.6）：固定 "app-mcp"
  user?: string            // Host 进程的操作系统用户（Unix uid / Windows SID）
  pid?: number             // Host 进程号
}

interface PairingResultParams { status: "paired" | "rejected"; token?: string; reason?: string }
interface VisibilityParams { visibility: Visibility; focused: boolean }
interface ActivateParams { mode: Activation }

interface ToolInfo {
  name: string             // [a-zA-Z0-9_.-]{1,64}，App 内唯一，不含 appId
  description: string
  inputSchema: object      // JSON Schema，type 必须为 "object"
  risk?: Risk              // 旧写法（3.2）
  activation?: Activation
  title?: string
  annotations?: ToolAnnotations  // 标准 MCP 工具注解（3.2）
  outputSchema?: object    // 结果的 JSON Schema（MCP outputSchema），根类型任意（3.2）
  surface?: ToolSurface    // 对界面的依赖（3.4），缺省 "app"，"app" 时不序列化
  page?: string            // 所在页面名（3.4），[a-zA-Z0-9_.-]{1,64}
  backgroundTool?: string  // 后台替代（3.4）：只对 view 工具有意义，同一 App 中一个 app 工具的局部名
}

type ToolSurface = "app" | "view"

// 标准 MCP 工具注解，Host 原样转发给 Agent（3.2）
interface ToolAnnotations {
  title?: string
  readOnlyHint?: boolean
  destructiveHint?: boolean
  idempotentHint?: boolean
  openWorldHint?: boolean
}

// 标准 MCP 内容注解，Host 原样转发（3.2）
interface ContentAnnotations {
  audience?: ("user" | "assistant")[]
  priority?: number        // 0（可选）~ 1（必需）
  lastModified?: string    // ISO 8601
}
interface ToolsSyncParams { tools: ToolInfo[] }
interface ToolsChangedParams { upserted: ToolInfo[]; removed: string[] }

interface ToolsInvokeParams { callId: string; name: string; arguments: object; timeoutMs?: number;
  idempotencyKey?: string     // Agent 的幂等键，1..=256 个字符，原样（3.3）
  priority?: "interactive" | "normal" | "background" }  // Agent 给出的调用优先级（5.3），省略 = normal
interface ToolsInvokeResult {
  data: unknown            // 无返回值时为 null
  stateHints?: string[]
  status?: ResultStatus    // 缺省 "done"，done 时不序列化（3.2）
  stateResource?: string   // status 为 pending 时：可读取后续状态的资源名（局部名）
  summary?: string         // 一句面向模型 / 用户的结论；partial 时说明完成了哪部分
  annotations?: ContentAnnotations  // 结果内容的标注
}
type ResultStatus = "done" | "pending" | "partial" | "noop"
interface ToolsCancelParams { callId: string; reason?: string }
// 进行中调用的进度（3.3）
interface ToolsProgressParams { callId: string; progress: number; total?: number; message?: string }

interface ResourceInfo {
  name: string
  description: string
  mimeType?: string
  realtime?: boolean       // 需实时推送：被订阅时 SDK 保持连接（spec/lifecycle.md 第 13 节 B3）；缺省 false，只在 true 时序列化
  annotations?: ContentAnnotations  // 资源内容的标注（3.2）
}
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

// 导航（3.4）
interface NavigateParams { page: string; params?: object }
interface NavigateResult { ok: boolean }   // 成功为 { ok: true }；失败用 JSON-RPC 错误（NAVIGATION_FAILED / NAVIGATION_DENIED / USER_ACTION_REQUIRED）
```

### 3.1 名称规则：局部名与全名

- SDK 注册、`tools/sync`、`tools/invoke`、`resources/*` 以及清单 `tools[].name` / `resources[].name` 中的名称都是
  **App 内的局部名**：`[a-zA-Z0-9_.-]{1,64}`，可以含 `.` 分组（如 `cart.checkout`），**不含 appId**。
- Host 对模型暴露的**全名** = `<appId>.<局部名>`（如 App `shop` 的 `cart.checkout` → `shop.cart.checkout`）。
  拼接只在 Host 一处进行；SDK、构建工具、清单都不写 appId 前缀。
- 局部名以 `<appId>.` 开头在协议上仍合法（按原样拼接），但几乎总是误把全名当成局部名（全名会变成
  `shop.shop.info`）。核心在注册时、Host 在收到同步 / 加载清单时给出警告；`@app-mcp/build` 的注释扫描直接报错。

### 3.2 工具声明与调用结果（第 14 项 S1 / S2、第 19 项 R1–R3）

本库只**如实传递** App 的声明，不据此做任何判断（是否确认、是否放行由 Agent 决定，高风险操作的最终确认在 App 内，
docs/plans/14-safety.md 第 1 节）。以下字段均为可选新增，缺省时消息与之前完全相同（`toolsHash` 不变）。

- **工具注解 `annotations`**：标准 MCP `ToolAnnotations`，Host 在 MCP `tools/list` 中原样给出。与旧写法 `risk` 同时存在时
  **声明的字段逐个优先**，缺少的字段按 `risk` 推导：`read` → `readOnlyHint: true`；`destructive` / `payment` →
  `readOnlyHint: false, destructiveHint: true`；`write` / `os-sensitive` → `readOnlyHint: false`（`Risk::annotations`，唯一定义）。
  只声明 `risk` 时 Agent 看到的注解与之前相同。`risk` 保留为旧写法（不删除），Hub SDK 的 `ApprovalPolicy` 仍按它审批。
- **输出 schema `outputSchema`**：结果 `data` 的 JSON Schema。MCP 要求 `outputSchema` 根类型为 `object`：根类型是 `object` 时
  原样给出、对象结果放入 `structuredContent`；其他根类型（数组、字符串等）包装为
  `{ "type": "object", "properties": { "result": <schema> }, "required": ["result"] }`，结果放入 `structuredContent: { "result": data }`。
  未声明时只有对象结果放入 `structuredContent`（之前的行为）。Host 可按配置核对结果是否符合（默认只记日志，spec/hub-api.md 3.11）。
- **结果状态 `status`**：handler 正常返回只说明请求被处理。`pending` = 已受理、尚未完成（等待用户在 App 内确认、异步处理），
  附 `stateResource`；`partial` = 只完成一部分，`summary` 说明；`noop` = 没有做任何改动。Host 在 MCP 结果最前面加一句说明
  （如"已受理，尚未完成……不要当作已完成，也不要重复提交"），并在 `_meta` 写 `dev.appwire/status`、`dev.appwire/stateResource`（资源 URI）；
  `done` 时都不加。
- **摘要 `summary` 与无返回值**：有 `summary` 时作为一段文本放在返回值之前。`data` 为 `null` 且没有 `summary`、状态为 `done` 时，
  Host 对模型输出固定文本"已完成"（不再输出 `null`），且不填 `structuredContent`。`summary` 计入结果大小上限（spec/hub-api.md 3.11）。
- **内容注解 `annotations`**（结果与资源）：标准 MCP 内容注解。结果的注解加在 App 给出的内容块（摘要、返回值）上，不加在 Host
  生成的说明、总览与资源变化提示上；资源的注解出现在 MCP `resources/list` 中。Host 不修正、不据此决策。

### 3.3 调用 ID、去重与进度（第 16 项 N7a / O2）

以下均为新增行为，消息格式只多一条可选通知；旧 Host 收到 `tools/progress` 按未知通知忽略（5.7），旧 SDK 不发送。

- **`callId`**：`ToolsInvokeParams.callId` 由 Host 为每次调用生成（MCP 出口每个 `tools/call` 一个新 ID）。Hub API 的调用方可以
  自带（`CallRequest.call_id`；按 LLM 格式分派时为 tool_call 的 `id`，spec/hub-api.md 3.4），以同一 `callId` 重试同一次调用即可得到
  去重保护。Host 自身不会在断线 / 唤醒 / 回连后重发 `tools/invoke`（断线时调用以 `APP_DISCONNECTED`"结果未知"结束）。
  调用元信息（`_meta` 中的 `callId` 等）归第 19 项 R4，不在此定义。
- **`idempotencyKey`（Agent 幂等键，第 4f 项 j）**：Agent 在 MCP 请求 `_meta` 中给出的幂等键（键名与 Hub 侧规则见
  spec/hub-api.md 3.15），Host **原样**放进 `ToolsInvokeParams.idempotencyKey`（1..=256 个字符，`MAX_IDEMPOTENCY_KEY_LEN`；Host 拒绝
  不合法的键，不转发）。用途：MCP 客户端重试同一操作时每次是新的 `callId`，`callId` 去重保护不到；Agent 给出的幂等键跨重试不变。
  - SDK 在 handler 上下文中原样提供（没有时为空），App 决定如何使用（如作为业务层去重键、传给后端）。各语言入口：Rust 核心
    `Event::InvokeTool.idempotency_key`、原生运行时 `CallHandle::idempotency_key()`、C ABI v16 `am_call_idempotency_key`、uniffi
    `Call.idempotency_key()`、Node 原生模块 `Call.idempotencyKey`、WASM `invokeTool` 事件 `idempotencyKey`；各语言封装的
    handler 上下文 `idempotencyKey`（Python `idempotency_key`、C# `IdempotencyKey`）。
  - 去重（下一条）**另外**按（工具名, 幂等键）匹配：不同 `callId`、同一工具的同一幂等键与同一 `callId` 一样处理（重放首次结果 /
    挂到执行中的调用）；同一键用于其他工具互不影响；没有键时只按 `callId`。去重关闭时只透传。
- **去重（SDK，`app-mcp-core` 实现，所有语言一致）**：handler **已开始执行**的 `callId`，其首次最终回复（成功、handler 错误、
  `TIMEOUT`、执行中被 `tools/cancel` 的 `CANCELLED`）在有效期内保留；同一 `callId` 的 `tools/invoke` 再次到达时**不再执行**，直接回复
  该结果。执行中（含排队中）再次到达的请求挂到同一次执行上，完成时一并回复（不再返回 -32602）。执行中因断线或 SDK 停止被中断的
  调用记为 `CANCELLED`（`data.interrupted: true`、`data.callId`）："结果未知，App 内可能已执行"。排队中被取消 / 超时、在
  `TOOL_NOT_FOUND` / `TOOL_DISABLED` 等检查处被拒绝的调用 handler 没有执行过，不记录，同一 `callId` 可以再次执行。
  - 规则对所有工具一致（不区分只读与写）：Host 不复用 `callId`，只读工具只在调用方显式重试同一 `callId` 时得到缓存结果。
  - 有效期与容量由 SDK 配置 `callDedup`（`ttlMs` 默认 300000、`maxEntries` 默认 64，超出淘汰最早的；任一为 0 关闭，关闭时
    执行中重复的 `callId` 按旧行为返回 -32602）。去重表跨连接、跨休眠保留，进程退出即清空。各语言 SDK 的配置入口：
    Rust `ClientConfig.call_dedup` / `NativeConfig.call_dedup`；网页、Node、鸿蒙 `callDedup: { ttlMs, maxEntries }`；C ABI（v13）
    `AmClientOptions.call_dedup_ttl_ms` / `call_dedup_max_entries`（0 = 缺省，负数 = 关闭）；C++ `ClientConfig::call_dedup`；
    C# `AppMcpClientOptions.CallDedup`；Dart `AppMcp(callDedup:)`；uniffi `ClientConfig.call_dedup`（Kotlin / Swift `callDedup`、
    Python `AppMcp(call_dedup=CallDedup(...))`）。
  - 可观测性：命中（重放首次结果或挂到执行中的调用）只在 SDK 本地记一条警告日志（核心 `Event::Warning`，经各 SDK 的日志回调输出，
    带 `callId`），不计数、不上报 Host——`app/diagnostic` 专用于连接问题（第 10 节）。调用方自带 `callId` 重试时可在 App 日志中确认是否命中。
- **进度 `tools/progress`（SDK → Host，通知）**：handler 经 `ctx.progress(progress, total?, message?)` 报告；只对执行中的调用发送，
  未连接时丢弃（不排队、不补发）。`progress` 应递增；非有限数不发送，非有限的 `total` 视为未知。
  Host 只接受被路由到该调用的那条连接发来的进度，按配置的最小间隔合并（间隔内只保留最新一条）、丢弃不递增的值、`message`
  截断到 200 字符，转发给请求了进度的一方（MCP 请求带 `progressToken` 时为 `notifications/progress`，spec/hub-api.md 3.12）；
  没有接收方时丢弃。调用结束后到达的进度被忽略。
- **取消**：Agent 取消（MCP `notifications/cancelled`、Hub API `cancel_call`）→ Host 发 `tools/cancel` → SDK 取消 handler（5.3）。

### 3.4 界面级暴露与导航（第 4c 项）

本节是 `surface` / `page` / `app/navigate` 的唯一定义；Hub 侧的页面目录与渐进披露见 spec/hub-api.md 3.14，清单的 `pages`
见 spec/manifest.md 2.3。以下均为新增：字段缺省时消息与之前完全相同（`toolsHash` 不变），旧 SDK 不声明能力、Host 不向其发导航。

- **`surface`**：`app`（缺省）= 不依赖界面，后台可调、可唤醒、进清单与原生意图（`app-mcp-codegen` 只为它生成）；`view` =
  依赖界面，只在所在界面**真正可见且处于最上层**时注册（启用）。是否注册由 App / 封装层按可见性决定（第 4c 项 D / E），协议与
  核心只如实传递声明。
- **`page`**：工具所在页面（页面目录的键，与清单 `pages[].name` 同一命名空间）。Hub 记下 SDK 上报过的 `page`，工具因切页注销后
  仍知道它在哪个页面；调用不在当前页面的工具时据此导航。
- **能力协商**：App 设置了导航回调时，SDK 在 `app/hello.capabilities.navigate` 声明 `true`；能力只在握手时声明，连接后才设置 /
  清除的回调在下次连接时生效（之前到达的请求按当时的回调处理）。
- **`app/navigate`（Host → SDK，请求）**：`{page, params?}` → `{ok: true}`。SDK 行为（`app-mcp-core` 实现，所有语言一致）：
  - 未完成握手 → `UNAUTHORIZED`（同 5.1 第 5 步）；参数无法解析或 `page` 不合法 → `-32602`。
  - 没有导航回调（`capabilities.navigate` 为 false）→ `NAVIGATION_FAILED`，`data.reason = "unsupported"`。
  - 实例不可见（可见性 `hidden` / `frozen`）且 `navigateInBackground` 为 false → **立即**回复 `USER_ACTION_REQUIRED`
    （`data.reason = "foreground"`，无 `uri`），不调用导航回调（见下文「后台与前台」）。
  - 否则交给导航回调（核心事件 `Navigate`），回调切换界面后完成：成功回复 `{ok: true}`；页面不存在 / 参数不合法等以
    `NAVIGATION_FAILED`（`reason: "error"`）失败；不愿切换（用户正在输入、页面需要登录等）以 `NAVIGATION_DENIED`
    （`reason: "app"`）拒绝，`message` 面向模型 / 用户。导航改变用户可见界面：是否允许由 App 决定，本库不加确认。
  - 进行中的导航阻止空闲休眠（与进行中的资源读取相同，spec/lifecycle.md 第 3 节）；连接断开时丢弃，不回复。
  - 导航后的工具注册 / 注销照常经 `tools/changed` 同步（5.2）；回调最好在新页面的工具注册之后再完成，Host 会等待目标工具出现。
- **后台与前台**（本节与 spec/hub-api.md 3.14 后台替代的共同前提；选择工具 surface 的指引只在此处定义）：
  - 平台大多不允许后台 App 自行回到前台（Android 10+ 限制后台启动 Activity，iOS、鸿蒙同理；浏览器标签页不能自行切到前台）。
    `navigateInBackground`（SDK 设置项，核心 `ClientConfig.navigate_in_background`）声明实例不可见时导航请求是否仍交给导航回调：
    `false` 时 SDK 按上文立即以 `USER_ACTION_REQUIRED`（`foreground`）回复，Agent 据此请用户打开 App，而不是等到超时
    （`NAVIGATION_FAILED` / `timeout`）。缺省按平台：原生运行时在桌面（能恢复 / 激活自己的窗口）为 `true`，Android、iOS、鸿蒙为
    `false`；网页（WASM 核心）为 `false`。App 随时可以改。
  - App 要自己处理后台导航时（如发一条"点按继续"的通知、点开后打开目标页面）把它设为 `true`，在导航回调中按自己的界面状态处理，
    并以 `USER_ACTION_REQUIRED`（`reason: "foreground"`，`uri` 为该页面的 App 内入口）完成——各 SDK 的导航句柄 / 回调支持与工具
    handler 相同的"需要用户操作"写法。是否发通知、发什么是 App 的决定，本库只提供机制（Android 的可选辅助：app-mcp-android 的
    `AppMcpContinueNotification`，Android 13+ 需 App 自行声明并申请 `POST_NOTIFICATIONS`）。
  - **app 工具还是 view 工具**：后台也必须能用的能力（Agent 在用户不看 App 时调用，如加入购物车、查询订单）做成 `app` 工具——
    后台可调、可唤醒；`view` 工具只用于离不开当前界面状态的操作（当前表单的输入、选中项、页面上的弹窗）。一个操作两种界面都需要时，
    把业务逻辑放在 `app` 工具里，`view` 工具只做界面相关的部分，并用 `backgroundTool` 声明后台替代。
  - **`backgroundTool`**（`ToolInfo`，可选，只对 `view` 工具有意义）：同一 App 中一个 `app` 工具的局部名。该 view 工具没有实例
    注册、而 App 在后台（或导航因 `foreground` 被拒）时，Hub 改调这个 app 工具（规则见 spec/hub-api.md 3.14），结果标出实际调用的工具。
    两个工具的 inputSchema 应兼容（Hub 按替代工具的 schema 校验参数，不符时不改调）。缺省时不序列化（`toolsHash` 不变）。
- **`data.reason`**（`NAVIGATION_FAILED` / `NAVIGATION_DENIED`，第 4 节）：`unsupported`（不支持导航）、`error`（回调出错）、
  `timeout`（Host 在时限内没有收到回复）、`tool-not-registered`（导航完成但时限内目标工具没有注册）、`app`（App 拒绝）、
  `not-navigable`（清单声明该页面不可导航，Host 不发请求）。Host 产生的错误另带 `appId`、`page`。
- 各语言入口（第 4c 项第一部分只提供底层接口，框架绑定在第二部分）：Rust `ClientConfig.navigation` / `Client::set_navigation`、
  `Event::Navigate`、`Client::complete_navigate`；原生运行时 `NativeClient::set_navigation_handler(NavigationHandler)`、
  `NavigateHandle`（`complete` / `fail` / `deny`）、`ToolOptions.surface` / `page`；C ABI v14 `am_client_set_navigation_handler`、
  `AmNavigate`、`AmToolOptions.page` / `surface`；uniffi `AppMcpClient.set_navigation_handler`、`NavigationHandler`、`Navigate`、
  `ToolSpec.surface` / `page`；Node 原生模块 `setNavigationHandler`、`Navigate`、`ToolSpecInit.surface` / `page`；WASM
  `setNavigation`、`navigate` 事件、`completeNavigate`、工具定义 `surface` / `page`。
  后台导航与 `backgroundTool`：Rust `ClientConfig.navigate_in_background` / `Client::set_navigate_in_background`、`ToolDef.background_tool`；
  原生运行时 `NativeClient::set_navigate_in_background`、`NavigateHandle::fail_user_action`、`ToolOptions.background_tool`；
  C ABI v15 `am_client_set_navigate_in_background`、`am_navigate_fail_user_action`、`AmToolOptions.background_tool`；uniffi
  `AppMcpClient.set_navigate_in_background`、`Navigate.fail_user_action`、`ToolSpec.background_tool`；Node 原生模块
  `setNavigateInBackground`、`Navigate.failUserAction`、`ToolSpecInit.backgroundTool`；WASM `setNavigateInBackground`、工具定义
  `backgroundTool`。各语言封装：设置项 `navigateInBackground`（Python `navigate_in_background`、C# `NavigateInBackground`，缺省 =
  平台缺省）、工具声明 `backgroundTool`；导航回调里抛出 / 返回该 SDK 的"需要用户操作"（与工具 handler 相同）即回复
  `USER_ACTION_REQUIRED`，其他异常仍为 `NAVIGATION_FAILED`。Electron 主进程 `attachAppMcp({ raiseWindow })` 给出时才在后台导航
  （否则立即以 foreground 拒绝）；Tauri 插件在桌面先恢复并聚焦窗口；WPF 导航先恢复并激活窗口（WinUI 暂不能）。
- 进程内控件兜底（第 4c 项 H）：没有声明工具的界面由 SDK 以 `ui.*` view 工具兜底，格式与行为见 spec/ui-fallback.md（协议不感知）。
- 网页封装层（`@app-mcp/web`，第 4c 项 D；Electron / Tauri 页面侧的桥接实现相同）：
  - 入口：`ToolDefinition.surface` / `page` / `visibility`、`scope(name, { anchor, layer, page, surface, visibility })`（其下工具
    未声明时继承，最近的 scope 优先）、`createViewLayer(name)`、`AppMcp.setNavigationHandler(handler, options)`；`@app-mcp/react`
    的 `<ToolScope anchor page surface>`、`<ToolLayer>`、`useRouterNavigation`；Vue Router 适配 `@app-mcp/web/vue-router`。
  - `view` 工具的门控（`visibility: 'always'` 关闭）：页面可见（`visibilityState`）、不在已打开的界面层之下、锚点已挂载且未被
    `hidden` / `inert` / 打开的模态 `<dialog>` 遮挡、已渲染、在视口内；不满足时以 `enabled: false` 同步（对 Host 即注销）。
    界面层内的工具不继承层外 scope 的 `page`（层只在打开时存在，不作为导航目标）。
  - 导航：回调完成后等界面稳定（两帧，上限可配，默认 500 ms）并重新评估门控再回复；有打开的界面层时缺省以
    `NAVIGATION_DENIED`（`app`）拒绝、不调用回调（`whileLayerOpen: 'allow'` 关闭）。

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
| `RATE_LIMITED` | -32016 | Host 限流：对该（App, 工具）或该 App 的调用过于频繁，调用未转发。`data`：`retryAfterMs`（建议等待毫秒数）、`scope`（`tool` / `app`）、`perMinute`、`burst`、`appId`、`tool`（spec/hub-api.md 3.11） |
| `PAYLOAD_TOO_LARGE` | -32017 | Host 大小上限：调用参数、调用结果或资源内容超过上限，未转发 / 未返回（不截断）。`data`：`part`（`arguments` / `result` / `resource`）、`sizeBytes`、`limitBytes`。`result` 超限时调用可能已在 App 内执行 |
| `POLICY_DENIED` | -32018 | 调用被用户 / 厂商写的策略规则拒绝（`deny`，spec/hub-api.md 3.13），未转发、未唤醒；与 `USER_REJECTED`（用户当场拒绝）不同，重试不会改变结果。`data`：`ruleId`（命中规则的标识，不含规则内容）、`hook`（`call` / `wake`）、`appId`、`tool` |
| `USER_ACTION_REQUIRED` | -32019 | 需要用户本人操作后才能继续：登录过期、系统权限未授予、需切到前台、需在 App 内确认等。由 App 的 handler 返回（各语言 SDK 提供构造方法），也是导航在后台无法完成时的回复（3.4，`reason: "foreground"`）；调用未完成，用户操作后可重试。`message` 面向用户（Agent 应转告用户），`data`：`reason`（可选，建议取值 `login` / `permission` / `foreground` / `confirm`，其他字符串按原样展示）、`uri`（可选，App 内入口，如深链接）。`reason: "os-permission"`（2026-10-02）由 Hub 产生：按名拨号时目标 App 已安装、但操作系统阻止 Hub 启动 / 绑定它（Android 关联启动 / 自启动管控、OEM 拦截；spec/naming.md 第 12 节 `ACTIVATION_BLOCKED`），需用户在系统设置中放行该 App；`message` 为「系统阻止了 AppWire Hub 启动『<应用名>』。请在系统设置中允许『<应用名>』自启动 / 关联启动后重试。」，不含组件名等内部信息；`data` 另带 `appId`、`appName`（面向用户的应用名，未知时为 appId）、`packageName`（平台包名，宿主给出时；Agent 可据此打开系统的应用详情设置页，Android `HubClient.settingsIntent`）、`code`（`ACTIVATION_BLOCKED`）。Host 原样转为 MCP 错误结果，不据此做任何决定；错误消息与结果一样计入结果大小上限（spec/hub-api.md 3.11） |
| `NAVIGATION_FAILED` | -31001 | 导航没有完成（3.4）：App 不支持导航、导航回调出错、超时，或导航后时限内目标工具没有注册。调用未执行。`data`：`reason`（`unsupported` / `error` / `timeout` / `tool-not-registered`）、`appId`、`page` |
| `NAVIGATION_DENIED` | -31002 | 导航被拒绝（3.4）：App 拒绝本次导航（`reason: "app"`，`message` 来自 App），或清单声明该页面不可由 Agent 导航（`reason: "not-navigable"`）。调用未执行；重试不会改变结果，应请用户自行打开该页面 |
| `LOCKED` | -31003 | 对象锁冲突（spec/hub-api.md 3.6「对象锁」，只由 Host 产生）：App 正被其他 Agent 以 `apps.lock` 锁定，写调用未转发、未唤醒；或要加的锁已被他人持有。`data`：`appId`、`key`（命名锁时）、`holder`（持有者的记账主体：`agent:<名>` / `local` / `api`）、`retryAfterMs`（锁的剩余有效期）。可等待后重试、改做只读操作，或请用户协调 |

**错误码分区**（`ErrorKind::code`，唯一定义）：-32001 ~ -32019 为既有类别（JSON-RPC 实现自定义区，保留不变）；-32020 ~ -32099
归 MCP 规范（`HeaderMismatch` -32020、`MissingRequiredClientCapability` -32021、`UnsupportedProtocolVersion` -32022 等，
docs/plans/12-mcp-2026-07-28.md m10），本协议不使用；此后新增的类别从 -31001 起编号（JSON-RPC 保留区 -32768 ~ -32000 之外的应用
定义区），避免 Hub 把上游 MCP 服务器的错误码误认作本协议的类别。接收方以 `data.kind` 为准，数字码只作后备。

`message` 面向模型，应说明原因和建议的下一步。`data` 中除 `kind` 外可携带其他字段。
标准 JSON-RPC 错误码（-32700、-32600、-32601、-32602、-32603）用于协议层错误。

## 5. SDK 行为（`app-mcp-core` 实现，所有语言一致）

### 5.1 连接与握手

1. 连接建立后，SDK 立即发送 `app/hello`（在收到结果前不发送其他消息）。
   `handshakeTimeoutMs`（默认 10s，0 表示不限）内没有收到结果：关闭连接并进入重连（5.6）。
   进入 `PendingPairing` 后不再受此限制。收到结果后先核对 Host 身份（1.6），不通过则进入 `HostMismatch`。
2. 结果为 `paired`：
   1. 保存 `token`（若返回）；与配置中的 token 不同时通知驱动层持久化。
   2. 依次发送 `tools/sync`、`resources/sync`（全量，只含已启用的工具）。
      若本次 `app/hello` 携带了 `resumeToken` 且结果为 `toolsCurrent: true`，跳过这两条（第 8.3 节）。
   3. 发送 `app/visibility`（当前值）。
   4. 发送 `app/ready`，状态变为 `Connected`。
   5. 清零重连计数。
3. 结果为 `pending`：状态变为 `PendingPairing`，等待 `app/pairingResult`；
   收到 `paired` 后执行第 2 步，收到 `rejected` 按第 4 步处理。
4. 结果为 `rejected`，或 `app/hello` 返回错误（`-32601` 除外，见 1.6）：状态变为 `Rejected`，关闭连接，不再自动重连。
5. 握手期间 Host 发来的 `tools/invoke` 等请求返回 `UNAUTHORIZED` 错误。

### 5.2 注册变更

- 未连接时，注册 / 注销 / 启用状态变化只更新本地注册表，连接后通过全量同步发送。
- 已连接时，同一轮内的多次变更合并为一条 `tools/changed`（和 / 或 `resources/changed`），
  在驱动层下一次取事件时发出。同一名称先加后删则两者抵消，不发送。
- 工具被禁用等同于从 Host 的视角移除（出现在 `removed` 中），重新启用等同于新增。

### 5.3 调用

- 收到 `tools/invoke`：
  - `callId`（或同一工具的同一 `idempotencyKey`）已执行过（有效期内）→ 回复首次结果，不执行；正在执行或排队 → 挂到同一次执行上（3.3）。
  - 名称不存在 → `TOOL_NOT_FOUND`；存在但禁用 → `TOOL_DISABLED`。
  - 否则进入调用队列，按到达顺序调度：一个排队的调用在以下条件都满足时开始执行——正在执行的调用数小于 `maxConcurrentCalls`
    （默认 1）；该工具正在执行的调用数小于其 `concurrency`（工具声明，0 / 缺省 = 不单独限制）；该工具声明了互斥组 `exclusive`
    （命名规则同工具名）时，同组没有正在执行的调用。因本工具或其互斥组正忙而不能开始的调用留在原位，其后能开始的调用先开始
    （不被队头阻塞）；占用者结束后按原顺序优先。`concurrency` / `exclusive` 只在 SDK 内生效，不随 `tools/sync` 发给 Host，
    也不计入 `toolsHash`；放宽声明后排队中的调用随即可能开始。用途：App 声明哪些工具不能并发（如操作同一份文档的写工具），
    调用方（Agent）不必知道（第 16 项 N6；Agent 之间的协调用 Hub 的对象锁，spec/hub-api.md 3.6）。
  - 新到的调用需要排队而排队中的调用数已达 `maxQueuedCalls`（默认 64，0 = 不限）→ `RATE_LIMITED`，`data` 带
    `{"scope": "queue", "limit": N}`（不带 `retryAfterMs`：何时空出取决于正在执行的调用）。被拒绝的调用未开始，不记入去重表，
    同一 `callId` 稍后可重发。
  - **优先级**（第 16 项 P6）：调用队列先按 `priority`（`interactive` > `normal` > `background`）、再按到达顺序排列，上面的调度规则
    在此顺序上进行（交互调用排到已排队的普通 / 后台调用之前，同级不插队）；已开始的调用不被打断。新到的调用需要排队而队列已满时，
    若队列中有优先级更低的调用，改为拒绝其中**最后到达**的一个（`RATE_LIMITED`，`data` 另带 `"preempted": true`），新调用入队；
    否则照常拒绝新调用。接收方把不认识的 `priority` 取值当作 `normal`（向后兼容；Host 侧对 Agent 的取值另做严格校验，
    spec/hub-api.md 3.15）。优先级由 Agent 判断（如用户在对话中等结果时用 `interactive`、定时 / 批量任务用 `background`），
    本库不推断。
  - **用户正在操作**（第 16 项 N6）：App 用 `setBusy(true / false)` 声明用户此刻正在 App 内操作（何时算由 App 决定，如编辑框
    获得焦点、拖拽中；本库不推断）。期间**写调用**（生效注解不是 `readOnlyHint: true` 的工具，按 `risk` 推导规则）按客户端配置
    `busyPolicy` 处理：`reject`（默认）→ `RATE_LIMITED`，`data` 带 `{"scope": "busy"}`（不带 `retryAfterMs`），未开始、不记入
    去重表，同一 `callId` 稍后可重发；`queue` → 留在调用队列中（不被其阻塞的调用照常先开始；仍受 `maxQueuedCalls` 与 `timeoutMs`
    约束），`setBusy(false)` 后按到达顺序开始。只读调用与已开始的调用不受影响；`setBusy(true)` 时策略为 `reject` 则排队中的写调用
    随即被拒绝。`busyPolicy` 可在运行时修改（`setBusyPolicy`，如由用户在 App 设置中选择），随即对排队中的调用生效。busy 状态只在
    SDK 内，不发给 Host；`app/navigate`、资源读取不受影响。
- `timeoutMs` 从收到请求时开始计时（包含排队时间）。超时后取消 handler，返回 `TIMEOUT`。
- 收到 `tools/cancel`：取消对应调用（排队中的直接移出），返回 `CANCELLED`。
- handler 完成后返回 `ToolsInvokeResult`；handler 出错返回其错误（缺省类别 `HANDLER_ERROR`）。
- 已取消或已超时的调用，其后到达的完成结果被丢弃。
- 连接断开时取消所有进行中和排队的调用，不发送任何响应（已完成但尚未发出的响应一并丢弃，其结果仍保留在去重表中，3.3）。
- handler 执行期间可报告进度（`tools/progress`，3.3）。
- SDK 主动停止（`stop`）或休眠时，已排队但尚未交给驱动层的消息（如刚完成的调用结果）
  先于关闭连接发出；连接已断开时则丢弃。

### 5.4 资源

- 收到 `resources/read`：资源不存在 → `RESOURCE_NOT_FOUND`；否则请求驱动层读取并返回。
- `resources/subscribe` / `unsubscribe` 维护订阅集合，返回 `{}`；资源不存在 → `RESOURCE_NOT_FOUND`。
  订阅集合在断线（含休眠）后清空，由 Host 重新订阅。
- 跨连接补发（spec/lifecycle.md 第 13 节 B3）：断开时的订阅集合留作"待恢复订阅"；未连接期间（或回连后 Host 重新订阅之前）
  其中的资源发生变化则标记；Host 重新订阅被标记的资源时，SDK 先回复 `{}`，再发送 `resources/updated`（受节流约束）。
  休眠期间声明了 `realtime` 的待恢复订阅资源变化时，SDK 以原因 `app` 回连推送。
- 资源内容变化时，仅对已订阅资源发送 `resources/updated`；同一资源两次通知之间
  至少间隔 `resourceUpdateThrottleMs`（默认 100ms），节流期内的多次变化合并为一次。

### 5.5 心跳

- 是否发心跳由 `heartbeat.mode`（`auto` 默认 / `always` / `off`）与驱动层告知的传输类别决定（spec/lifecycle.md 第 11 节 A3）：
  `auto` 下本地 IPC 与桌面本机回环（含网页到本机回环 / 共享连接）**不发**，靠连接断开（EOF / RST）感知；远程、沙箱平台上的回环
  （`adb reverse` 等转发）与未知传输发。`lifecycle.legacyTimers` 恢复旧行为（一律发）。
- SDK 在 `app/hello.heartbeatMs` 中声明本连接的心跳：不发为 `0`，发则为间隔；`legacyTimers` 时不带该字段。
- 发心跳时：`Connected` 状态下，每隔 `heartbeat.intervalMs`（默认 15s）发送 `ping`。
- 超过超时时间（可见时 `timeoutMs` 默认 10s；隐藏或冻结时 `hiddenTimeoutMs` 默认 120s）
  未收到响应，视为断开：关闭连接并进入重连。
- 收到 Host 的 `ping` 请求，立即返回 `{}`。收到任何消息都不重置心跳计时，只有 `ping` 的响应才算。
- 挂起保护：`ping` 的响应截止时刻之后又过了一整个超时才被调度（进程被冻结 / 挂起、页面计时器被限流），不判断开，
  重新发 `ping` 计时。

### 5.6 重连

- 非 `Stopped` / `Rejected` / `Dormant` / `HostMismatch` 状态下连接断开，进入 `Backoff`，
  延迟为 `min(initialDelayMs * multiplier^n, maxDelayMs)`（默认 500ms 起，×2，最大 30s），
  n 为自上次成功握手以来的重试次数。
- 到期后请求驱动层重新连接。
- Host 不在：`idle` / `on-demand` 模式下连续 `lifecycle.hostAbsentRetries`（默认 3，0 = 不限）次以"Host 不在"类错误码
  （目前只有 `HOST_NOT_RUNNING`，`ConnectionErrorCode::means_host_absent`）建立连接失败，进入 `Dormant` 而不是 `Backoff`，
  不再重试；之后与休眠相同，由页面 / 界面重新可见、`wake()` / `connectNow()` 或 Host 唤醒回连。其他错误码打断"连续"。
  `persistent` 与 `legacyTimers` 不受影响（spec/lifecycle.md 第 11 节 A2）。

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
| `HostMismatch { reason, code }` | 对端不是期望的 Host（1.6）。已断开，不重连、无定时器；`wake()` / `connectNow()` 时再连一次。绑定中的名称：Rust `StateStatus::HostMismatch`、C `AM_STATE_HOST_MISMATCH`（10）、JS `'host-mismatch'` |

`Rejected`、`HostMismatch` 带错误码 `code`（第 10 节）；`Backoff` 在建立连接失败时带 `reason` / `code`
（驱动层按系统错误归类，如 `HOST_NOT_RUNNING`、`IPC_PERMISSION_DENIED`），断线、握手超时时也带相应的码。

`app/sleep` 发出到收到结果之间为内部过渡态 `sleeping`，对外仍为 `Connected`。

## 6. Host 行为（M1）

- 对每个连接执行握手（先按 1.4 做连接级校验）；M1 对来自回环地址或本地 IPC、`Origin` 在允许列表内（默认 `http://localhost:*`、
  `http://127.0.0.1:*`）或无 `Origin` 的连接直接返回 `paired` 并分配随机 token。
  其他 `Origin` 返回 `rejected`。
- `protocolVersion` 不为 `"1"` 时返回 `rejected`，`reason` 说明版本不兼容。
- 同一 `appId` 可有多个实例（`instanceId` 区分）；同一 `instanceId` 重复连接时，新连接替换旧连接。
- 连接上的第一条消息为 `app/mux` 时进入多路复用模式（第 9 节），此后本节规则对每个通道分别适用。
- 向 SDK 发送 `tools/invoke` 前，已按 inputSchema 校验参数；默认 `timeoutMs` 为 30000。
- 存活判断按 SDK 在 `app/hello.heartbeatMs` 中的声明（spec/lifecycle.md 第 11 节 A3）：
  - 省略（旧 SDK 或 `legacyTimers`）或 Host 配置 `legacy_heartbeat`：每隔 15s 向 SDK 发送 `ping`；45s 内没有收到 SDK 的任何消息则关闭连接。
    实例处于 `hidden` / `frozen` 时（浏览器会限流后台页面的定时器），该超时放宽为 180s。
  - `0`（本地传输，SDK 不发心跳）：Host 不发 `ping`，握手完成后**不做**无消息断开，只按连接断开处理；半开连接由下次派发调用的超时发现。
  - 大于 0（远程，SDK 单向心跳）：Host 不发 `ping`；无消息断开的超时取 `max(上面按可见性的值, 3 × heartbeatMs)`。
  - 握手完成前（含等待配对）沿用原规则。
- 返回 `rejected` 的握手结果发送后，Host 关闭连接。
- TCP 只接受来自回环地址的连接（且 `Origin` 满足上述规则）；本地 IPC 只接受同一用户的进程（1.4）。
- 资源订阅与休眠（spec/lifecycle.md 第 13 节 B3）：实例休眠时保留其资源订阅；实例回连 `app/ready` 后对仍被订阅的资源
  重新发送 `resources/subscribe`；对休眠实例的 `resources/read` 与工具调用一样先唤醒再派发。
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

`{ ttlMs, adaptive? }`：Host 预计还会调用本实例（如 MCP 会话仍活跃、模型刚调用过），在 ttl 内不要休眠；`ttlMs: 0` 取消（两种租约都取消）。
SDK 取当前租约与新值中较晚的截止时刻；收到租约时重新开始空闲计时，休眠时刻 = max(空闲起点 + 空闲时长, 租约到期)（8.5）。Host 可在每次调用完成后发送。
`adaptive`（可选，只在为 `true` 时发送）：租约来自 Host 按调用间隔的统计；缺省为默认值租约。SDK 分别记两种租约，
后台连接（握手时不可见且 `sleepOnBackground`）只认自适应租约（spec/lifecycle.md 第 13 节 B4）。旧 SDK 忽略该字段。

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

- 休眠时刻 = max(空闲起点 + 空闲时长, 租约到期)：租约与空闲计时并行（`legacyTimers` 时为旧规则：租约到期后才开始计空闲时长）。
- 模式：`persistent`（默认，不休眠）/ `idle`（启动即连接，空闲 `idleTimeoutMs` 后休眠）/
  `on-demand`（启动时不连接，进入 `Dormant`；被唤醒或 `connectNow()` 时连接，空闲 `graceMs` 后休眠）。
  隐藏 / 冻结时使用 `min(模式超时, hiddenIdleTimeoutMs)`。
- 空闲条件（全部满足才开始计时，任一变化重置计时）：没有进行中或排队的调用、资源读取；没有对 `realtime` 资源的订阅
  （普通资源的订阅不阻止休眠，`legacyTimers` 时任何订阅都阻止）；没有 App 的持有（`hold()`，含调用上的 `hold`）。
  可见性变化也重新计时。租约是休眠时刻的下限（见上）。
- 合并窗口（spec/lifecycle.md 第 13 节 B1）：本连接处理过 `tools/invoke` / `resources/read` 后，空闲时长取
  `min(mergeWindowMs（默认 2000）, 按模式与可见性的空闲时长)`；之后是否在线只由租约决定。
- 后台立即休眠（B4）：`sleepOnBackground` 且模式非 `persistent` 时，可见性从 `visible` 变为 `hidden` / `frozen` 后，
  空闲条件一成立就以 `reason: "background"` 发送 `app/sleep`，不等租约与空闲时长；`backoff` 中则直接进入 `Dormant`。
  回到可见或休眠被拒后恢复普通规则。
- 自动休眠（空闲计时到期）的 `reason`：`on-demand` 为 `grace`，否则 `idle`——可见性只决定计时长短，不改变原因。
  `background` 专指"进入后台立即休眠"（网页 bfcache `pagehide(persisted)`、移动端进入后台），由封装层显式发起，
  或由 `sleepOnBackground` 在进入后台时自动发起（B4）。
  App 显式 `sleep()` 为 `app`（不看空闲条件与持有；被拒后按 `retryAfterMs`，缺省 5s 重试）。
- `Dormant`：无连接、无定时器（`poll_timeout()` 为空）；收到的注册变更只更新本地注册表，不唤醒。
- 唤醒：`handleWake(args)` 识别到令牌 → `Waking` 并连接（未 `start` 时记录，`start` 时连接；`Backoff` 时立即重连）；
  `wake()` / `connectNow()` 同理（原因 `app`）。休眠握手进行中收到唤醒或持有：休眠完成后立即回连。
- 驻留：`residency` 为 `exit-always`，或为 `exit-when-idle` 且本进程由唤醒冷启动（配置带 launch token，或首次
  握手前收到唤醒）时，进入 `Dormant` 后通知 App（`onIdleExit`），由 App 决定是否退出。

## 9. 多路复用（一条连接承载多个实例）

用途：同一来源的多个浏览器标签页共用一条到 Host 的 WebSocket（网页 SDK 经 SharedWorker 持有），
同时**每个标签页仍是独立实例**——各自 `instanceId`、各自注册的工具，随标签页存亡。实现：`app_mcp_protocol::mux`、
`crates/hub/src/app_server.rs`（`run_mux`）、`packages/web/src/mux/`。

### 9.1 协商

- SDK 以 `app/mux` 请求作为连接上的**第一条消息**：`MuxParams { version: 1 }`（SDK 支持的最高版本）。
- 支持的 Host 返回 `MuxResult { version, maxChannels }`（`version` ≤ 请求值，当前为 1；`maxChannels` 默认 64），
  此后该连接上的每一帧都是 9.2 的多路复用帧。
- 旧 Host 不认识 `app/mux`，按"握手前的请求"返回错误（`UNAUTHORIZED`）。SDK 收到**任何错误响应**即判定不支持，
  关闭该连接，改为每个实例各开一条普通连接（能力协商，不是失败回退）；该判定在 60 秒内复用。
- `app/mux` 只在第一条消息时有效；握手后的 `app/mux` 按未知方法返回 `-32601`。

```ts
interface MuxParams { version: number }
interface MuxResult { version: number; maxChannels: number }
```

### 9.2 帧

```ts
type MuxFrame =
  | { type: "open";  ch: number }                    // SDK → Host：打开通道
  | { type: "msg";   ch: number; msg: JsonRpcMessage } // 双向：通道上的一条 v1 消息，原样嵌入
  | { type: "close"; ch: number; reason?: string }    // 双向：关闭通道
```

- 通道号由 SDK 选择：正整数，同一连接内不重复使用。`open` 之后该通道上的第一条消息必须是 `app/hello`。
- 每个通道等同于一条独立的 v1 连接：握手、配对、Origin 校验（使用该 WebSocket 的 `Origin`）、心跳（`ping`）、
  45s / 180s 空闲超时、`app/sleep`、同一 `instanceId` 的替换规则都按通道分别进行。
- `close`（任一方）等同于该通道的连接断开：Host 按断开处理该实例（已 `app/sleep` 被接受的实例保持休眠）。
  Host 结束一个通道（握手被拒、被同一实例的新连接替换、空闲超时）时发送 `close`。
  关闭后收到的、发往该通道的帧直接丢弃；对已关闭通道回送的 `close` 也忽略。
- 打开的通道数达到 `maxChannels` 时，Host 以 `close`（带 `reason`）拒绝新通道。无法解析的帧记录警告后忽略。
- 连接断开时其上全部通道断开。连接上没有通道时，Host 在 180s 内没有收到任何帧则关闭连接；
  SDK 侧在最后一个通道关闭时即关闭连接（所有标签页都休眠时没有 WebSocket）。

### 9.3 网页 SDK 的连接持有方

- 首选 SharedWorker（`@app-mcp/web` 的 `mux-worker.js`）；没有 SharedWorker（Chrome Android 148 之前等）而有
  Web Locks 与 `BroadcastChannel` 时，用锁选出一个主标签页持有连接，其他标签页经 `BroadcastChannel` 收发；
  主标签页在 `pagehide` / `freeze` 时让位，其上的通道全部断开，由各标签页的核心按断开重连到新的主标签页。
- 标签页进入 bfcache（`pagehide` persisted）时，先由其核心发出 `app/sleep`，再请持有方暂存发给它的消息
  （向缓存中的页面投递消息会使其被逐出）；`pageshow` 恢复时取回。页面卸载时关闭其全部通道。
- 本地网络访问授权（Chrome LNA）只能由页面发起：授权为 `prompt` 时经共享连接失败，该次连接改由页面直接建立。


## 10. 诊断：错误码、上报与连接 ID

### 10.1 连接级错误码

SDK 的连接状态（`Backoff` / `Rejected` / `HostMismatch`，网页另有 `blocked`）与 Host 的启动失败都带一个机器可读的错误码
`code`，同时保留中文说明 `reason` / `message`。错误码是字符串，新版本可能增加；接收方遇到不认识的码时只展示说明，不报错。
实现：`app_mcp_protocol::diagnostic::ConnectionErrorCode`（`reason()` / `hint()` 与下表一致，测试核对本表列出了每个码）。

| code | 类别 | 出现在 | 原因 | 修复建议 |
|---|---|---|---|---|
| `HOST_NOT_RUNNING` | connect | SDK `backoff` | Host 未运行：端点上没有监听者（连接被拒绝、套接字 / 管道不存在） | 启动 Host（`app-mcp-host serve` 或 `service install`）；`app-mcp-host doctor` 查看端点 |
| `CONNECT_TIMEOUT` | connect | SDK `backoff` | 规定时间内没能建立连接 | 检查 Host 是否卡住（`doctor`）、防火墙 / 代理是否拦截回环连接 |
| `CONNECT_FAILED` | connect | SDK `backoff` | 建立连接失败（其他系统错误） | 查看 SDK 日志中的系统错误并运行 `doctor` |
| `IPC_PERMISSION_DENIED` | connect | 原生 SDK `backoff` | 本地 IPC 端点属于其他用户，或当前用户无权访问（1.4） | 以同一用户运行 Host 与 App；套接字目录 0700 且属于当前用户（`doctor` 检查） |
| `CONNECTION_CLOSED` | disconnect | SDK `backoff`（断线） | 已建立的连接被 Host 正常关闭（Close 帧或连接结束：Host 停止、重启、主动断开） | 自动重连；Host 已停止时启动它；频繁出现时查看 Host 日志 |
| `CONNECTION_LOST` | disconnect | SDK `backoff`（断线） | 已建立的连接因 I/O 错误中断（连接被重置、管道断开，未经关闭握手） | 自动重连；频繁出现时检查 Host 是否崩溃（`doctor`、Host 日志）、代理 / 安全软件是否切断连接 |
| `HEARTBEAT_TIMEOUT` | disconnect | SDK `backoff`（断线） | 心跳超时：Host 没有及时响应 `ping`（5.5），SDK 主动断开 | 自动重连；Host 可能卡住或过载：查看 Host 日志，必要时重启 |
| `HOST_NOT_APP_MCP` | identity | SDK `host-mismatch` | 对端不是 app-mcp Host（端口被其他程序占用，1.6） | 停止占用端口的程序（`doctor` 给出进程），或指定正确端点 |
| `HOST_OTHER_USER` | identity | 原生 SDK `host-mismatch` | 对端是其他操作系统用户的 Host | 启动自己的 Host，或用 `APP_MCP_ENDPOINT` 指定自己的端点 |
| `HANDSHAKE_TIMEOUT` | handshake | SDK `backoff` | Host 没有及时回复 `app/hello` | 查看 Host 日志，必要时重启 Host |
| `PROTOCOL_INCOMPATIBLE` | handshake | SDK `rejected` | 协议版本不兼容 | 升级 SDK 或 Host |
| `ORIGIN_NOT_ALLOWED` | handshake | 网页 SDK `rejected` | 网页来源不在允许列表中（第 6 节） | `--allow-origin <来源>` |
| `INVALID_HELLO` | handshake | SDK `rejected` | 握手参数不合法（appId 格式、保留名、与上游重名、instanceId 为空） | 检查 appId 与 instanceId |
| `PAIRING_REJECTED` | handshake | SDK `rejected` | 用户拒绝了配对请求 | 在配对提示中允许后重试（`wake()` / `connectNow()`） |
| `REJECTED` | handshake | SDK `rejected` | 其他拒绝（Host 未给出错误码，如旧 Host；`app/hello` 返回其他错误） | 查看原因说明与 Host 日志 |
| `BLOCKED_LOCAL_NETWORK_ACCESS` | browser | 网页 SDK `blocked` | 浏览器本地网络访问（LNA）权限未授予 | 站点设置中允许「本机上的应用」，授权后自动重连 |
| `BLOCKED_INSECURE_CONTEXT` | browser | 网页 SDK `blocked` | 非 HTTPS 的公网页面不能连接本机 | 改用 HTTPS，或在 localhost 打开 |
| `BLOCKED_CSP` | browser | 网页 SDK `blocked` | 内容安全策略 `connect-src` 不允许连接 Host | CSP 加入 `ws://127.0.0.1:7717`（及 7737、7757）后刷新 |
| `LOCK_HELD` | host | `app-mcp-host serve` / `doctor` | 同一配置目录已有 Host 在运行（1.5） | 无需处理；重启前先 `service stop` |
| `PORT_BUSY` | host | `serve` / `service install` / `doctor` | 监听端口被占用 | `doctor` 查看占用进程并停止它，或 `--listen` 换端口 |
| `IPC_ENDPOINT_BUSY` | host | `serve` / `doctor` | 本地 IPC 端点被占用（另一个配置目录的 Host） | 停止它，或 `--ipc-endpoint` 换端点 |
| `IPC_PATH_TOO_LONG` | host | `serve` / Hub 启动 / `doctor`；原生 SDK `backoff` | 本地 IPC 端点超过系统上限（Unix 套接字路径 `sockaddr_un.sun_path`：Linux 107 字节、macOS 103 字节；Windows 命名管道完整名：256 字符） | `--ipc-endpoint unix:<较短的绝对路径>` / `pipe:\\.\pipe\<较短名称>`（嵌入式 Hub 为 `HubConfig.ipc_endpoint`，SDK 为 `APP_MCP_ENDPOINT` / `host_url`），或缩短 `XDG_RUNTIME_DIR` / `--home` 所在路径 |
| `SDK_INIT_FAILED` | sdk | 网页 SDK `rejected` | SDK 本地初始化失败（WASM 核心加载失败、创建核心失败），没有连接 Host | 检查 `wasmUrl` 能否加载、CSP 是否允许 WebAssembly（`'wasm-unsafe-eval'`）与控制台错误 |
| `WAKE_RATE_LIMITED` | wake | Hub 调用结果（工具错误 `LAUNCH_FAILED` 的 `data.code`）、`/status` 最近错误 | 该 App 最近一分钟内被唤醒的次数已达上限（`HubConfig.wake_rate_limit`，spec/lifecycle.md 第 12 节），本次调用不再唤醒 | 稍后重试（`data.retryAfterMs`），或让用户打开该 App；频繁出现说明 App 刚唤醒就休眠，检查其空闲时长 / 租约，必要时调大 `--wake-rate-limit` |

- Host 拒绝握手时在 `HelloResult.code` / `PairingResultParams.code` 中给出码（`PROTOCOL_INCOMPATIBLE`、`ORIGIN_NOT_ALLOWED`、
  `INVALID_HELLO`、`PAIRING_REJECTED`）；旧 Host 不带时 SDK 用 `REJECTED`。
- 原生 SDK 的驱动层把建立连接时的系统错误归类（`app_mcp_protocol::diagnostic::connect_error_code`）：连接被拒绝 / 不存在 →
  `HOST_NOT_RUNNING`，超时 → `CONNECT_TIMEOUT`，权限 → `IPC_PERMISSION_DENIED`，其他 → `CONNECT_FAILED`。另外，TCP 已接受但
  WebSocket 握手未完成即被关闭 / 重置（`adb reverse` / `hdc rport` 等转发在远端没有监听者时的表现）也归为 `HOST_NOT_RUNNING`
  （`crates/native` `connect_issue`）。网页驱动层的连接在打开前失败、且不属于浏览器拦截（`BLOCKED_*`）时归为 `HOST_NOT_RUNNING`
  （浏览器不给出原因；此前为 `CONNECT_FAILED`）。
- Unix 域套接字路径在绑定 / 连接前按 `app_mcp_protocol::endpoint::check_unix_socket_path` 检查长度（上限
  `MAX_UNIX_SOCKET_PATH_BYTES`）：Hub 启动返回 `InvalidInput`，错误内含 `ConnectionIssue`（`IPC_PATH_TOO_LONG`，说明带实际长度、
  上限与建议，可经 `io::Error::get_ref` 取出）；原生 SDK 进入带该码的 `backoff`。
- Windows 命名管道完整名（`\\.\pipe\<名称>`，含前缀）按 `app_mcp_protocol::endpoint::check_pipe_name` 检查长度（上限
  `MAX_PIPE_NAME_CHARS` = 256 个 UTF-16 码元，`CreateNamedPipeW` 的限制）：Hub 创建管道前检查，返回同样的 `InvalidInput` +
  `IPC_PATH_TOO_LONG`；`app-mcp-host doctor` 的 IPC 检查同样报出。原生 SDK 连接管道前同样检查（`crates/native`），进入带该码的 `backoff`。
- 已建立的连接断开（类别 `disconnect`）：原生驱动层收到 Close 帧或读到连接结束 → `CONNECTION_CLOSED`（说明中带关闭码与原因），
  读取出错 → `CONNECTION_LOST`，经核心 `Client::handle_disconnected_with` 进入带码的 `Backoff`；核心自身的心跳超时 →
  `HEARTBEAT_TIMEOUT`（此前为 `CONNECT_FAILED`）、握手超时 → `HANDSHAKE_TIMEOUT`。驱动层未给出原因的断线（`handle_disconnected`）
  仍不带码（原生与网页驱动层都已不再使用）。
- 网页 SDK 的驱动层（`packages/web`，WASM 核心 `handleDisconnectedWith`）：浏览器只给出 `close` / `error` 事件——先到的是
  `close`（CloseEvent）→ `CONNECTION_CLOSED`（说明带关闭码、原因，`wasClean` 为假时注明"未经关闭握手"，如 1006）；先到的是
  `error`（连接失败、被重置）→ `CONNECTION_LOST`。经共享连接（第 9 节）时由持有方归类：Host 关闭通道（`close` 帧）→
  `CONNECTION_CLOSED`，持有方到 Host 的连接断开按上面的规则，持有方本身不可用（让位、无响应）→ `CONNECTION_LOST`。
- 各语言的状态对象都带 `code`：Rust 核心 `ConnectionState::{Backoff, Rejected, HostMismatch}` 的 `code` 字段、
  原生 `StateInfo.code`、JS `state.code`。

### 10.2 `app/diagnostic`（SDK → Host，通知）

```ts
interface DiagnosticParams {
  code: string      // 10.1 的错误码
  message: string   // 最近一次的中文说明
  count: number     // 自上次上报以来发生的次数，缺省 1
}
```

SDK 记录连接失败期间遇到的问题（同一 `code` 合并计数），在下一次握手成功（`app/ready` 之后）时逐条上报并清空。
Host 记录每个 App 最近的上报（`/status` 的 `reports`，`app-mcp-host doctor` 展示）。

**局限**：问题发生时 SDK 与 Host 之间没有可用的通道——浏览器拦截（`blocked`）期间页面无法连接 Host，Host 无从得知；
只有连接恢复后（如用户授予了本地网络访问权限、CSP 修正后刷新）才能上报"此前被拦截过"。从未连上的页面不会出现在 Host 的诊断中，
需在页面自己的日志 / 状态（`state.code`）中查看。

### 10.3 连接 ID

Host 为每条 App 连接（多路复用时为每个通道）分配连接 ID（`<Host 启动标记>-<序号>`，如 `3f9a1c-12`），在 `HelloResult.connectionId`
中返回；每个 MCP 会话同样有会话 ID（`mcp-<序号>`）。Host 日志中与该连接 / 会话有关的记录都带 `cid` 字段；SDK 握手成功后把连接 ID
写入日志（原生日志回调、网页 `logger`），连接期间的日志带 `[cid]` 前缀（原生为 `[cid] …`，网页为 `[app-mcp] [cid] …`），便于在两边日志中对照同一条连接。`/status` 的实例信息也带
`connectionId`。Electron / Tauri 页面经桥接（`window.appMcpBridge`）连接时，连接属于主进程（Rust 侧）的客户端：桥接的 hello 回复与
`state` 事件带可选 `connectionId`（`BRIDGE_VERSION` 仍为 1，版本内只做可选字段新增；旧主进程不带时页面为 `undefined`），
页面的 `AppMcp.connectionId` 与主进程日志中的 `[cid]` 相同。
