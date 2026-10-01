# 按名寻址与系统名字服务规范

版本：草案 v0.1（2026-10-01）。本文件是"Hub 如何按名字找到 App、建立连接、发现 App、更新目录、结束与回收连接"的
**权威契约**（任务来源：TASKS.md 4d；设计背景：`docs/plans/4e-lifecycle-power.md` P2）。

边界（不在本文件重复定义）：

| 内容 | 权威位置 |
|---|---|
| JSON-RPC 消息、握手、帧、连接级错误码表 | `spec/protocol.md` |
| 休眠 / 唤醒状态机、租约、`toolsHash`、WakeDescriptor | `spec/lifecycle.md` |
| 静态清单 `app-mcp.json` | `spec/manifest.md` |
| Hub API 与各语言绑定 | `spec/hub-api.md` |

本文件只替换"找到对方"这一层：**协议消息与 sans-IO 核心不变**。本文件中标注"待实现时加入 spec/protocol.md / spec/hub-api.md"
的条目是 4d 实施时要合入那些文件的增量，在合入之前以本文件为准、不视为已实现。

## 目录

1. 术语
2. 地址
3. 连接方向与角色
4. 各平台名字映射
5. 发现
6. 更新
7. 结束与回收
8. Android 功耗约束
9. 多 Hub 共存与旧路径兼容
10. 安全
11. 调试
12. 错误码（新增，待合入）
13. 事实 / 未知 / 风险

## 1. 术语

| 术语 | 含义 |
|---|---|
| 名字 | 系统名字服务中代表一个 App（或其一个实例）的条目：D-Bus 总线名、Android Service 组件、命名管道名、launchd Mach 服务名 |
| 地址 | 与平台无关的 App 身份 `appmcp://<appId>[/<instance>]`（第 2 节），由 Hub 映射为当前平台的名字 |
| 拨号 | Hub 按地址打开一条到 App 的通道（第 3 节） |
| 激活 | 名字的所有者进程未运行时，由系统在拨号时拉起它（D-Bus 激活、`bindService`、launchd 按需启动、COM / 协议激活） |
| 通道 | 拨号得到的一条字节流（socketpair 的一端、命名管道、XPC 传来的 fd），其上跑与现有传输相同的帧与消息 |
| App 登记文件 | App 安装 / 首次运行时写下的元数据文件（第 5.3 节）。**不同于** spec/protocol.md 1.7 的 Host 登记文件 `endpoints.json` |
| 发现记录 | Hub 本地为每个 appId 保存的合并结果（来源、时间、签名指纹、快照，第 5.4 节） |
| 活引用 | 能让 App 进程保持运行或被唤醒的东西：绑定、连接、fd、跨进程回调对象、定时器、WakeLock |
| 宽限 | 最后一次调用完成后仍保持通道的时间，用于合并连续调用（第 7.2 节） |

## 2. 地址

### 2.1 语法

```
address  = "appmcp://" app-id [ "/" instance ]
app-id   = %x61-7A *62( %x61-7A / DIGIT / "-" )      ; [a-z][a-z0-9-]{0,62}
instance = %x61-7A *31( %x61-7A / DIGIT / "-" )      ; [a-z][a-z0-9-]{0,31}
```

- `app-id` 与 `HelloParams.appId`、清单 `appId` 的规则相同（spec/protocol.md 第 3 节；实现 `app_mcp_protocol::is_valid_app_id`）。
- `instance` 是**登记实例名**：App 为一个窗口 / 进程在名字服务中登记的短名（如 `main`、`w2`）。它不等于握手中的
  `instanceId`（后者每个进程 / 标签页唯一、可以很长）；Hub 在通道握手成功后记录"地址 ↔ instanceId"的对应。
  字符集限制为小写字母开头、只含小写字母 / 数字 / `-`，使它能无歧义地映射到 D-Bus 名元素（不能以数字开头）、
  对象路径元素和大小写不敏感的命名管道名。
- 只有上面这一种规范形式：scheme 必须是小写 `appmcp`，不允许用户信息、端口、查询串、片段、尾部 `/`、百分号编码、
  大写字母。解析器对任何其他形式**显式失败**（配置错误 / `INVALID_ADDRESS`，第 12 节），不做规范化猜测。
- 省略 `instance` 的地址指 App 的**默认名字**：唯一可被系统激活的名字。实例名字只在该实例运行期间存在，不能激活。

### 2.2 保留名

- `app-id` 不能是保留名 `apps`、`os`、`ax`、`host`（`app_mcp_manifest::RESERVED_APP_IDS`，单一定义处），也不能与该 Hub
  配置的上游 MCP 服务器名称相同（与握手校验一致，spec/protocol.md 10.1 `INVALID_HELLO`）。
- `instance` 保留 `default`（指默认名字本身，不能作为登记实例名）。

### 2.3 与"统一调用链接"的关系

`app-mcp-plan.md` 10.4 规划过 `appmcp://<appId>/<toolName>?args=…&sig=…` 形式的给人点击的调用链接（尚未实现，代码中无
`appmcp://` 处理）。两者的第一个路径段语义冲突。本规范规定：**`appmcp://` 的第一个路径段是登记实例名**；本地址是 Hub
内部与诊断输出用的标识，4d 不向操作系统注册 `appmcp:` URI 协议。调用链接如果实施，须改用不冲突的形式（另一个 scheme
或查询参数），见第 13.2 节 U-17。

## 3. 连接方向与角色

```mermaid
flowchart LR
  subgraph os["系统名字服务"]
    name["名字（D-Bus 名 / Service / 管道 / Mach 服务）"]
  end
  hub["Hub（拨号方）"] -- "1 解析地址 → 名字" --> name
  name -- "2 未运行则激活" --> app["App 进程（被连接方）"]
  hub -- "3 Open() 换得通道" --> app
  app -- "4 app/hello（SDK 先发）" --> hub
```

- **操作系统层的发起方是 Hub**：App 在系统名字服务登记名字，不常驻、不重连 Hub；Hub 在需要时拨号。Hub 与 App 的
  启动顺序无关，多个 Hub 可以共存（第 9 节）。
- **协议层角色不变**：通道建立后仍由 SDK 先发 `app/hello`，Hub 仍是"Host"一方，消息、握手、配对、快速恢复、
  `tools/sync`、调用全部按 spec/protocol.md；核心（`crates/core`）是同一个状态机，只新增一个"被连接方"驱动
  （接受一条已建立的通道后从 `Connecting → Handshaking` 开始跑现有核心；任务来源 TASKS 4d-B）。
- **帧不变**：通道上与本地 IPC 一样跑 HTTP/1.1 + WebSocket（spec/protocol.md 1.2），**SDK 仍是 WebSocket 客户端**
  （发送到 `ws://localhost/app` 的升级请求），Hub 仍是服务端。也就是说：OS 层"谁拨号"反转，WebSocket 与 JSON-RPC 的角色
  不反转——这样 SDK 核心、Hub 的消息处理、超时与关闭逻辑仍只有一份实现。网页经扩展的通道例外（第 4.6 节）。
- **心跳**：通道是本地传输，存活由系统对端死亡通知与 EOF 判断（第 7.5 节），两端不发 `ping`（与 spec/lifecycle.md 中本地
  传输去心跳的规则一致）。
- **认领**：Hub 拨号是为了派发某些待派调用；在**它自己拨出的通道**上就绪的实例直接认领这些调用，不需要 `wakeToken`。
  SDK 在该通道的 `app/hello` 中填 `wakeReason: "os-activation"`（沿用现有取值）。
- **生命周期映射**（细则见第 7 节）：

  | 现有概念（spec/lifecycle.md） | 按名寻址下 |
  |---|---|
  | 休眠（`dormant`） | Hub 关闭通道；名字保留；Hub 保留快照 |
  | 唤醒 | Hub 拨号；进程未运行时由系统激活 |
  | `wakeToken` / WakeDescriptor | 降为**后备**：只用于不支持激活的平台 / App（Windows 未打包 App 的协议激活、网页 `web-url`、旧 SDK） |
  | 租约（`app/lease`） | 仍由 Hub 计算，决定**何时关闭通道**（第 7.2 节） |
  | App 端空闲计时 | Hub 发起的通道上不运行：关闭时机只由 Hub 决定；App 可用 `app/sleep { reason: "app" }` 请 Hub 提前关闭 |

### 3.1 地址解析与拨号流程

```mermaid
flowchart TD
  call["调用 / 资源读取到达，目标 appId（可带实例）"] --> live{"已有活连接？（任一方发起）"}
  live -- "是" --> dispatch["派发"]
  live -- "否" --> rec{"发现记录中有名字或激活方式？"}
  rec -- "否" --> wd{"有 WakeDescriptor？"}
  wd -- "是" --> wake["现有唤醒 + 等待 App 回连（spec/lifecycle.md）"]
  wd -- "否" --> disc["APP_DISCONNECTED"]
  rec -- "是" --> parse["解析地址 → 平台名字（第 4 节）"]
  parse --> idchk{"名字所有者身份与记录一致？（运行中时）"}
  idchk -- "否" --> mismatch["PEER_IDENTITY_MISMATCH，不拨号"]
  idchk -- "是 / 未运行" --> dial["拨号：Open() / bindService + open() / 打开管道 / XPC open"]
  dial -- "系统激活失败或拒绝" --> fail["LAUNCH_FAILED（data.code = ACTIVATION_DENIED / ACTIVATION_TIMEOUT）；确认未安装则 APP_NOT_INSTALLED"]
  dial -- "得到通道" --> hello["SDK 发 app/hello → 快速恢复或完整同步 → app/ready"]
  hello --> claim["认领本次拨号的待派调用"]
  claim --> dispatch
  wake --> dispatch
```

- 带实例的地址只解析到运行中的实例名字；实例不存在时**不**退回默认名字（显式失败 `NAME_NOT_FOUND`），由调用方决定是否改用无实例地址。
- 同一 App 已有拨号在进行时，后来的调用加入等待，不重复拨号（与 spec/lifecycle.md 唤醒去重同构）。

## 4. 各平台名字映射

### 4.0 总表

| 平台 | 名字 | 拨号 → 通道 | 激活 | 发现 | 支持 |
|---|---|---|---|---|---|
| Linux（桌面） | 会话总线名 `dev.appmcp.App.<id>`、实例 `dev.appmcp.App.<id>.<inst>`；对象路径 `/dev/appmcp/App[/<inst>]` | 方法 `dev.appmcp.App1.Open() → h`（socketpair 一端） | D-Bus 服务激活（`.service` 文件） | `ListActivatableNames` / `ListNames` + `NameOwnerChanged` + XDG 登记 | 是 |
| Android | 导出的绑定式 Service，Intent 动作 `dev.appmcp.TOOLS` | `bindService` → `IBinder` → `open(instance) → ParcelFileDescriptor`（socketpair 一端） | `bindService(BIND_AUTO_CREATE)` | `queryIntentServices` + `<meta-data>` 清单资源 + 包变更 | 是 |
| Windows | 命名管道 `\\.\pipe\appmcp-<SID>-<id>[.<inst>]` | Hub 作为管道客户端打开 | 未打包：协议激活后等待管道出现；打包：待验证（U-07），确认前同未打包 | App 登记文件（`%LOCALAPPDATA%\app-mcp\apps\`）+ 打包 App 扩展目录 | 是 |
| macOS | launchd 用户 Agent 的 Mach 服务 `dev.appmcp.App.<id>` | XPC 连接，消息 `open` → 回复中带 fd | launchd 按需启动 | `~/Library/LaunchAgents` / App 内嵌 Agent plist + App 登记文件 | 仅非沙盒；沙盒不支持（4.4） |
| iOS | 无 | — | — | — | **不支持**：只走 App Intents（codegen） |
| 网页 | 扩展已知的标签页 | 页面 → content script → 扩展 service worker → Native Messaging → Hub | `web-url`（WakeDescriptor 后备） | 扩展枚举已打开且加载了 SDK 的标签页 | 是（方向不反转，见 4.6） |
| 鸿蒙 | 暂无 | — | — | — | 4d 不覆盖，沿用现有 WebSocket 路径 |

`<id>`、`<inst>` 在 D-Bus 名与对象路径中按 4.1 的转义规则写出；其他平台原样使用。

### 4.1 Linux：D-Bus 会话总线

- **名字**：默认名字 `dev.appmcp.App.<id>`；实例名字 `dev.appmcp.App.<id>.<inst>`（同一进程可以拥有多个名字；
  另开进程的实例各自拥有自己的实例名字）。元素数量区分二者：`appId` 与 `instance` 都不含 `.`。
- **转义**：`appId` / `instance` 中的 `-` 写为 `_`（二者字符集都不含 `_`，映射可逆）。理由：对象路径元素只允许
  `[A-Za-z0-9_]`；总线名虽允许 `-` 但规范不推荐并建议换成 `_`（F-20）；Desktop Entry 规范的对象路径推导同样把 `-` 写成 `_`。
  例：`appmcp://my-shop/w2` → 名字 `dev.appmcp.App.my_shop.w2`、路径 `/dev/appmcp/App/w2`。总线名总长 ≤ 255：
  前缀 15 + appId 63 + `.` + instance 32 = 111，不会超限。
- **接口**：`dev.appmcp.App1`（接口名带主版本号，演进时新增 `App2`，旧接口保留一个弃用周期）。
  - `Open() → (h channel)`：App 创建 socketpair，返回一端（UNIX_FD），自己保留另一端并在其上以"被连接方"驱动跑核心；
    消息走 fd，**不经总线转发**。默认路径上的 `Open()` 选择默认实例（进程内的主实例），实例路径上的选择对应实例。
  - 每次 `Open()` 创建新通道；同一 Hub 的旧通道不复用。
- **激活**：`$XDG_DATA_HOME/dbus-1/services/dev.appmcp.App.<id>.service`（或 `$XDG_DATA_DIRS` 下的同名文件，由包管理器安装），
  `Name=dev.appmcp.App.<id>`、`Exec=<程序> --app-mcp-activation`；文件名与 `Name` 一致。由 `app-mcp-host app install` / codegen /
  打包模板生成，开发者不手写。写入后调用 `org.freedesktop.DBus.ReloadConfig`（dbus-broker 是否自动发现新文件未见文档，F-21 / U-05）。
  不使用 `$XDG_RUNTIME_DIR/dbus-1/services`（该目录不被监视）。实例名字不写激活文件。向未运行的可激活名字发方法调用即触发激活
  （不带 `NO_AUTO_START`）。
- **调用方身份**：App 在 `Open()` 中用 `GetConnectionUnixUser`（或 `GetConnectionCredentials`）查询调用方的 uid，
  与自身不同则返回 D-Bus 错误 `org.freedesktop.DBus.Error.AccessDenied`；Hub 拨号前用 `GetConnectionCredentials` 查询名字所有者的
  uid 与进程号，uid 不同则不拨（`PEER_IDENTITY_MISMATCH`），进程号用于取可执行文件路径（`/proc/<pid>/exe`）并与登记文件核对。
  默认会话总线策略允许同一用户的任何连接拥有任何名字（先到先得），**名字本身不证明身份**（10.3）。
- **接入**：Rust（`crates/native` 的 `NameServer` 实现）、Python（GApplication / dbus 库）、Qt、C / C++。

### 4.2 Android：绑定式 Service

- **名字**：App 清单中导出的 Service（SDK 提供 `dev.appmcp.android.ToolsService`，经清单合并加入），intent-filter 动作
  `dev.appmcp.TOOLS`；`<meta-data android:name="dev.appmcp.manifest" android:resource="@raw/app_mcp"/>` 指向构建期生成的
  静态清单。appId 由该清单给出（不由包名推导），Hub 核对"包名 ↔ appId ↔ 签名指纹"（第 10.3 节）。
- **拨号**：Hub `bindService(显式 Intent(动作 + 包名 + 组件), flags)`（flags 见第 8 节；`bindService` 必须用显式组件）→
  `onServiceConnected(IBinder)` → 调用 AIDL 方法 `open(String instance) → ParcelFileDescriptor`：App 以
  `ParcelFileDescriptor.createSocketPair()` 创建一对、返回一端，消息走 fd 上的同一帧格式。
- **冻结**：同步 Binder 调用打到已冻结的进程会使该进程被杀、调用方得到 `RemoteException`（F-08）。因此 `open()` 只在
  `onServiceConnected` 之后调用（绑定把进程从缓存态抬起；以 `BIND_WAIVE_PRIORITY` 绑定的 Service 在其客户端都进入缓存态之前不被冻结）；
  通道建立后消息走 fd、不再经 Binder。`open()` 收到 `RemoteException` 按"激活失败"处理，不在同一次调用内反复重试（U-02）。
- **一次绑定只交换一个 fd**：不传递任何回调 Binder 对象（远端代理会把对方对象钉住、阻碍 GC）；Service 不保存客户端引用；
  每次使用前重新 `bindService`（冻结中的进程由绑定解冻），不复用旧会话的 `IBinder` 代理。
- **调用方身份**：`open()` 在 Binder 事务内执行，App 用 `Binder.getCallingUid()` 取 Hub 的 uid，按第 10.2 节的 Hub 信任规则
  校验后才创建通道；不通过返回 `SecurityException`（Hub 记 `HUB_NOT_TRUSTED`）。`onBind()` 不做校验（不在调用方事务内）。
- **发现**：Hub 清单声明 `<queries><intent><action android:name="dev.appmcp.TOOLS"/></intent></queries>`，用
  `queryIntentServices(Intent(dev.appmcp.TOOLS), GET_META_DATA)` 一次性枚举，经 `PackageItemInfo.loadXmlMetaData` / 目标包资源读
  `<meta-data>` 指向的清单（PackageManager 接口，不经 App 进程；"确实不启动进程"列为待真机确认，U-01）；**不申请**
  `QUERY_ALL_PACKAGES`（Play 上需审批）。增量：Hub 启动时 `getChangedPackages(上次序号)`（序号每次开机归零，归零或返回异常时
  做一次全量 `queryIntentServices`）；运行期间以**动态注册**的接收器收包变更广播（清单注册的接收器在 API 26+ 收不到
  `PACKAGE_ADDED` / `REPLACED`）。
- **旧路径**：`WakeReceiver`（广播 `dev.appmcp.action.WAKE`）+ 加急 WorkManager 唤醒保留给"没有 Android 端 Hub、Hub 在 PC
  经 `adb reverse` 连接"的场景（spec/lifecycle.md 第 5 节）。

### 4.3 Windows：每 App 每用户命名管道

- **名字**：`\\.\pipe\appmcp-<用户 SID>-<appId>`（默认）与 `\\.\pipe\appmcp-<用户 SID>-<appId>.<instance>`（实例）。
  分隔符用 `.` 而不是 TASKS 草拟的 `-`：`appId` 与 `instance` 都可含 `-`，用 `-` 会有歧义（`shop-w2` 无法区分）。管道名不区分大小写，
  地址只用小写，因此映射无歧义。前缀 `appmcp-` 与
  Host 自己的管道 `\\.\pipe\app-mcp-<SID>`（spec/protocol.md 1.3）不同，互不冲突。完整名受 256 字符上限约束，
  创建前按 `app_mcp_protocol::endpoint::check_pipe_name` 检查（超长 → `IPC_PATH_TOO_LONG`）。
- **管道属主是 App**：App 以 `FILE_FLAG_FIRST_PIPE_INSTANCE` 创建第一个实例（名字已被占用即失败并报告），安全描述符与
  spec/protocol.md 1.4 相同（只有当前用户、`PIPE_REJECT_REMOTE_CLIENTS`）。Hub 作为管道客户端打开它；通道就是这个管道连接
  （Windows 不传 fd）。
- **调用方身份**：App 用 `GetNamedPipeClientProcessId` 取 Hub 进程并核对其用户 SID；Hub 打开后核对管道所有者 SID 与服务端进程
  （`GetNamedPipeServerProcessId`）的可执行文件路径与登记文件中的 `executable` 一致（第 10.3 节）。
- **App 登记文件**：`%LOCALAPPDATA%\app-mcp\apps\<appId>.json`（格式见 5.3），记录激活方式与可执行文件。管道没有官方的枚举接口，
  Hub **只从登记文件（及打包 App 的扩展目录）得知有哪些管道**，不枚举 `\\.\pipe\`。
- **激活**：
  - 未打包 App：协议激活（登记的 URI scheme，沿用 WakeDescriptor `uri`），随后 Hub 在激活超时内等待管道出现。管道尚不存在时
    `WaitNamedPipe` 立即失败（不等待超时），因此用有界退避重试（如 50 ms 起、×2、上限 1 s），只在本次激活窗口内进行，
    不是常驻轮询；超时 → `ACTIVATION_TIMEOUT`。
  - 打包（MSIX）App：发现用 AppExtension（`AppExtensionCatalog`，打包桌面 App 可用）；激活用 COM 本地服务器（`com:ExeServer`）或
    App Service。App Service 需要包身份、未打包 Hub 能否调用二者均待验证（U-07），确认前打包 App 也走协议激活 + 登记文件。
  - 打包 App 写 `%LOCALAPPDATA%` 可能被虚拟化到包私有位置、其他进程看不到（F-23）：打包 App 不依赖自写登记文件被 Hub 读到，
    而用 AppExtension 声明，或在包清单中把 `app-mcp\apps` 排除出虚拟化。
- **接入**：C#、C++、Rust、Python；WSL 侧 Hub 经 interop 的可达性列为待验证。

### 4.4 macOS：launchd + XPC

- **名字**：用户 LaunchAgent（标签 `dev.appmcp.App.<appId>`）的 `MachServices` 中声明 `dev.appmcp.App.<appId>`；
  Agent plist 随 App 包分发（包内 `Contents/Library/LaunchAgents/`，经 `SMAppService.agent(plistName:)` 注册，macOS 13+，
  出现在"登录项"中、受用户批准）或由安装程序写入 `~/Library/LaunchAgents/`。实例不登记独立 Mach 服务，以 XPC 消息参数 `instance` 区分。
- **名字的所有者是 launchd 作业**：Apple 不支持两个 App 之间直接用 XPC 通信，推荐经 launchd 作业会合（F-27）；Mach 服务只能由该
  作业的进程签入。因此按需进程是 Agent 作业（App 包内的无界面辅助程序，或以无界面模式运行的 App 本体）；用户从 Finder 打开的 GUI
  进程不是该作业，如何与作业共享工具（转交 / 合并为一个进程）在 4d-F 决定（U-09），确认前 GUI 进程仍走现有 App 拨 Hub 路径。
- **拨号**：Hub 以 `xpc_connection_create_mach_service` 连接，发送 `{op: "open", v: 1, instance}`，回复中以 `xpc_dictionary_set_fd`
  携带 socketpair 的一端；之后消息走 fd。XPC 连接在拿到 fd 后即取消，不长期持有。
- **激活**：launchd 在有人连接该 Mach 服务时按需启动 Agent（`MachServices` + 不设 `KeepAlive`）。
- **调用方身份**：App 核对 XPC 对端 euid（`xpc_connection_get_euid`）与自身相同；macOS 12+ 用
  `xpc_connection_set_peer_code_signing_requirement` 限定对端（可信 Hub 的签名要求）。XPC 没有公开的审计令牌接口；Unix 套接字上有
  `LOCAL_PEERTOKEN`。
- **沙盒 App**：App Group 容器内的 Unix 套接字只对**同一开发团队（Team ID）**的进程可用（F-28），第三方 Hub 无法使用；沙盒 Hub 查找
  全局 Mach 服务需要临时例外权利。因此沙盒 App（及沙盒 Hub）**不支持按名寻址**，走现有 WakeDescriptor + App 拨 Hub 路径。
- 无 Mac 实机：Linux 上编译与单元测试，实机验收列入 TASKS 待办。

### 4.5 iOS：不支持

iOS 上没有可供第三方使用的 launchd / XPC 服务接口（`xpc_connection_create_mach_service`、`SMAppService` 只在 macOS 提供），
系统给出的暴露动作的机制是 App Intents（F-30）。iOS 上**不提供**按名寻址，App 的能力经 codegen 生成的 App Intents 暴露；现有 `uri`
唤醒（前台）与 SDK 直连保留。

### 4.6 网页：浏览器扩展 + Native Messaging

- **方向不反转**：Hub 无法拨号到网页。页面 SDK 检测到扩展后经 `window.postMessage` → content script → 扩展 service worker →
  `chrome.runtime.connectNative("dev.appmcp.host")` 与 Hub 通信，不使用任何端口。Native Messaging 宿主程序为
  `app-mcp-host native-messaging`（宿主清单 `type: "stdio"`、`allowed_origins` / Firefox `allowed_extensions` 只列本扩展，由
  `service install` 写入各浏览器的用户级位置：Chrome / Edge / Firefox 在 Linux、macOS 为用户目录下的 `NativeMessagingHosts`，
  Windows 为 `HKCU` 注册表项）：它是浏览器要求的 stdio 进程，由它以本地 IPC 连接常驻 Hub 并承载帧——这是浏览器的进程模型决定的入口，
  不是为绕过问题加的转发层（原则 8 的边界说明）。
- **帧**：Native Messaging 的帧是"32 位本机字节序长度前缀 + UTF-8 JSON"，不是 WebSocket。通道上承载的仍是同一套 JSON-RPC 消息，
  并使用 spec/protocol.md 第 9 节的多路复用帧（每个标签页一个通道）。这是一种新传输形式，待实现时加入 spec/protocol.md 1.2。
- **零活引用**：打开的 Native Messaging 端口会让浏览器保持宿主进程运行、并保持 MV3 service worker 存活（Chrome 105+）。扩展只在
  存在加载了 SDK 的标签页、且其中至少一个实例有连接需要时持有端口；全部实例休眠后断开端口（与 spec/protocol.md 9.2"最后一个通道
  关闭即关闭连接"一致）。
- **页面 ↔ 扩展**：content script 经 `window.postMessage` 与页面通信（同一 DOM）；content script 收到的页面消息视为不可信输入，
  校验后再转发。
- **地址**：`appmcp://<appId>/<instance>` 中的实例对应一个标签页（实例名由扩展分配，如 `t<标签页序号>`）；无实例的地址
  不能激活网页，只能经 `web-url` 后备打开页面。
- **来源可信度**：扩展以浏览器 API 取得的标签页 URL 作为 `origin`，优先于页面自报。
- **消息大小**：宿主 → 扩展单条消息上限 1 MB（Chrome；扩展 → 宿主 64 MiB）。一个多路复用帧超过 1 MB 时 Hub 不发送，改为对该调用 /
  读取返回明确错误（`MESSAGE_TOO_LARGE`），不截断、不静默丢弃。
- **WebMCP**：页面提供 WebMCP 时（规范草案的接口为 `document.modelContext`，Chrome 149 起源试用，F-33；TASKS 4d-H 写作
  `navigator.modelContext`，以规范为准）经扩展把其工具暴露为该页实例的工具。
- 回环 WebSocket（`/app`）改为默认关闭、用户显式开启（TASKS 4d-H）；未安装扩展时页面 SDK 仍按现有规则连接已开启的端口。

## 5. 发现

### 5.1 原则

- **只读数据，从不为发现而启动进程**：发现只读安装期元数据、App 登记文件与名字服务的列表 / 事件；不调用 App 的任何方法、
  不 `bindService`、不连接 Mach 服务。
- **时机**：Hub 启动时扫描一次（Android 用 `getChangedPackages(sequence)` 增量）+ Hub 运行期间订阅系统变更事件
  （包安装 / 更新 / 卸载、inotify / `ReadDirectoryChangesW` 监视登记目录、D-Bus `NameOwnerChanged`）。**无轮询**。
- **多来源合并**：读安装信息只是其中一种主动鉴别方式；四种来源合并为每个 appId 一条发现记录（5.4）。

### 5.2 来源

| 来源 `source` | 内容 | 何时出现 | 依赖 App 运行 |
|---|---|---|---|
| `install` | 平台安装元数据：Android `<meta-data>` 清单资源、Linux `.service` + XDG 登记、Windows 打包 App 扩展 / 登记文件、macOS 包内清单 + launchd plist | 安装 / 更新时；Hub 启动扫描与变更事件读到 | 否（主路径） |
| `self-report` | App 首次打开（及登记内容变化时）的一次性自报 | 见 5.5 | 仅自报那一刻 |
| `name-service` | 名字出现 / 消失（`NameOwnerChanged`、管道 / Mach 服务的出现由激活与通道得知） | App 运行并登记名字时 | 是 |
| `manual` | 用户执行 `app-mcp-host app install <清单或程序>` 写入的 App 登记文件 | 用户操作时 | 否 |

### 5.3 App 登记文件

桌面平台（Linux、Windows、macOS）共用一种 JSON，`install`（安装程序写入）、`manual`（`app install` 写入）、`self-report`
（SDK 写入，5.5）三种来源都用它，以 `source` 字段区分：

```jsonc
{ "registrationVersion": 1,
  "appId": "shop",
  "name": "示例商城",
  "source": "install",                         // install | manual | self-report
  "manifest": "/opt/shop/app-mcp.json",         // 静态清单的绝对路径（可省略）
  "manifestSha256": "…",                        // 清单内容摘要（可省略）
  "executable": "/opt/shop/bin/shop",           // 用于判断"已不存在"与核对通道对端
  "activation": { "kind": "dbus" | "launchd" | "uri" | "com" | "app-service" | "none", "target": "…" },
  "signature": { "kind": "authenticode" | "codesign" | "none", "fingerprint": "sha256:…" } }   // 可省略；Hub 自己计算的值优先
```

| 平台 | 目录（用户级优先于系统级） |
|---|---|
| Linux | `$XDG_DATA_HOME/app-mcp/apps/<appId>.json`、`$XDG_DATA_DIRS` 各项下的 `app-mcp/apps/<appId>.json` |
| Windows | `%LOCALAPPDATA%\app-mcp\apps\<appId>.json`（被虚拟化的打包 App 写入此处对其他进程不可见，F-23；打包 App 改用 AppExtension，U-08） |
| macOS | `~/Library/Application Support/app-mcp/apps/<appId>.json` |

文件名必须等于 `<appId>.json` 且与内容 `appId` 一致，否则忽略并记录错误。目录与文件须属于当前用户（Linux / macOS 组与其他用户
不可写），否则不读。Android 不使用登记文件（安装元数据总在包内）。

### 5.4 合并规则

```mermaid
flowchart TD
  i["install：安装元数据"] --> m["按 appId 合并"]
  s["self-report：首次打开自报"] --> m
  n["name-service：名字出现 / 消失"] --> m
  u["manual：app install"] --> m
  m --> r["发现记录：sources[]、指纹、快照、激活方式"]
  r --> fp{"签名指纹与首次记录一致？"}
  fp -- "是" --> ok["正常列出"]
  fp -- "否" --> warn["标记 fingerprintChanged：提示用户，调用需确认"]
  r --> clr{"卸载事件 / 元数据或可执行文件不存在 / 激活确认未安装？"}
  clr -- "是" --> del["移除该来源；无来源时删除记录"]
```

- **记录**：每个来源一项 `{source, firstSeenMs, lastSeenMs, detail}`（`detail` 为包名、文件路径、总线名等），由 Hub 持久化在
  `<配置目录>/apps/discovery.json`（与 `endpoints.json` 同样的权限要求）。`doctor` 与 `apps.list` 列出每个 App 的来源。
- **字段优先级**：按字段取优先级最高的、提供了该字段的来源：`install` > `manual` > `self-report` > `name-service`。
  `name-service` 只提供"正在运行"与对端身份，不提供元数据。
- **签名指纹**：首次见到某 appId 时记录指纹（Android：签名证书 SHA-256；Windows：Authenticode 发布者证书；macOS：Team ID +
  代码签名要求；Linux：无平台签名，记录可执行文件路径与包管理器归属，可用时取其摘要）。之后：
  - `self-report` **不能**改写已登记 appId 的指纹；
  - 任何来源带来不同指纹 → 记录标记 `fingerprintChanged`，发 Hub 事件，`doctor` 报告；在用户确认之前，对该 App 的每次调用都经
    `ApprovalHandler` 确认（不因为风险等级是 `read` 而跳过）；用户确认后新指纹成为记录值（原则 7）。
- **清除**（**不按时间过期**——App 不会每次打开都上报，按时间清除会误删正常 App）：只在以下情况移除对应来源：
  - 卸载事件（Android `PACKAGE_REMOVED`（非替换）/ `PACKAGE_FULLY_REMOVED`、打包 App 卸载事件）；
  - 安装元数据、登记文件或其中的 `executable` 已不存在（启动扫描与目录变更时检查）；
  - 激活时系统确认"未安装"（组件不存在、激活文件指向的程序不存在）→ 调用返回 `APP_NOT_INSTALLED`。
  `name-service` 来源在名字消失时不删除记录，只把运行状态改为"未运行"。记录没有任何来源时删除。
- **重建**：Hub 本地数据丢失后，`install` / `manual` 由下一次扫描重建，`self-report` 由该 App 下一次通道握手（带同样信息）重建。

### 5.5 App 首次打开时主动上报

- **何时**：只在首次打开、以及登记内容变化时（升级后清单摘要 / `toolsHash` / 激活描述 / 签名指纹的摘要改变）上报一次；之后的
  普通打开不再上报。SDK 在 App 私有存储记下"已登记内容的摘要"，只在上报**确认成功**后写入；未成功则下次打开再试。
- **内容**：appId、清单摘要 / `toolsHash`、激活描述（WakeDescriptor 或名字服务激活方式）、签名指纹（SDK 能取得时）。
- **载体**（同一内容，按平台选一种）：
  1. 桌面平台登记目录可写时：SDK 写 `source: "self-report"` 的 App 登记文件（5.3）——不需要 Hub 在运行，写入成功即"确认成功"；
     Hub 经启动扫描或目录变更事件读到。可由 App 配置关闭。
  2. 其他情况（旧的 App 拨 Hub 路径、Hub 在另一台机器上）：一次短连接——正常握手后发 `app/register` 请求，Hub 持久化后返回
     `{ recorded: true }`，SDK 随即 `app/sleep { reason: "app" }` 并关闭。旧 Host 返回 `-32601` 视为未确认，下次打开再试。
     **待实现时加入 spec/protocol.md 第 2、3 节**（请求 `app/register`，参数 `RegisterParams { manifestSha256?, toolsHash, wake?, signature? }`，
     结果 `{ recorded: boolean }`）。
- **不常驻**：Hub 未运行时 SDK 不等待、不重试、不保持连接（服从第 7 节零活引用不变式）；登记由下次打开或名字服务事件补上。
- 方向反转后（App 已在名字服务登记名字），自报即"名字出现 + Hub 首次拨号握手"，不再需要 App 拨 Hub。
- Hub 持久化自报登记后经 `apps.list` 与 MCP `notifications/tools/list_changed` 通知 Agent 客户端。

### 5.6 安装信息覆盖面

| 能读到（安装期写入包内，由 `@app-mcp/build` / codegen 生成，开发者不手写） |
|---|
| Android APK：Service `<meta-data>` 指向的清单资源（PackageManager，不启动进程） |
| Linux：D-Bus `.service` 激活文件 + XDG 数据目录中的 App 登记文件 / 清单 |
| Windows 打包 App：MSIX 清单中的 AppExtension / AppService 声明（PackageManager / AppExtensionCatalog API，待验证） |
| macOS：App 包内清单 + launchd Agent plist（沙盒 App 经系统接口，待验证） |

| 读不到 | 补法 |
|---|---|
| 运行时注册的工具 | 连接时的快照 + `toolsHash` 持久化（第 6 节） |
| `view` 工具（TASKS 4c） | 只在连接期间有效；页面目录走静态清单 `pages` |
| 未打包的 Windows / Linux 程序 | 安装程序或 `app-mcp-host app install` 写登记文件；SDK 自报（5.5）；从未登记也从未运行过的 App 如实报告"不可发现" |
| 网页 | 扩展发现已打开的页面；已安装 PWA 的 Web App Manifest 可带自定义成员（未知成员被忽略），但扩展 / 宿主读取已安装 PWA 清单的官方接口未找到（U-14），确认前已安装 PWA 只在打开时被发现 |
| iOS | 沙盒不可读，只走 App Intents |

## 6. 更新

三层，从慢到快：

| 层 | 内容 | 来源 | 何时更新 |
|---|---|---|---|
| 静态 | 清单中的工具、资源、总览、激活方式 | 安装元数据 / 登记文件 | 随包版本：包更新事件或登记文件变化 |
| 动态快照 | 运行时注册的工具与资源 + `toolsHash` | 最近一次通道握手（`tools/sync` / `resources/sync`） | 仅在已连接时获得；持久化到 Hub 本地，Hub 重启后仍可列出 |
| 实时 | 增量变化 | 连接期间的 `tools/changed` / `resources/changed` | 推送 |

- **不为刷新而连接**：快照可能过期；下次因调用而建立通道时，握手中的 `toolsHash` 比对（spec/lifecycle.md 的快速恢复）纠正它。
- 快照中的工具在列表中标记为"需要激活"，调用时按快照的 schema 校验、按快照的风险等级审批，再拨号派发；拨号后发现工具已不存在 →
  `TOOL_NOT_FOUND`（并以新快照替换）。
- `view` 工具不缓存为可调用：通道关闭即撤下。
- 静态与快照冲突：已连接时以实例实际注册的为准（spec/manifest.md 第 5 节规则不变）。

## 7. 结束与回收

### 7.1 不变式（全平台，用测试强制）

**Hub 对 App 只保存数据（目录、快照、`toolsHash`、发现记录）；宽限期过后不持有任何活引用**（绑定、通道、fd、跨进程回调对象、
定时器、WakeLock）。系统回收或杀掉 App 进程是正常路径，不是异常。

### 7.2 按调用建立通道 + 宽限

```mermaid
stateDiagram-v2
  direction LR
  [*] --> unbound
  unbound --> dialing: 有待派调用
  dialing --> active: 通道就绪（app/ready）
  dialing --> unbound: 激活失败 / 超时（调用返回错误）
  active --> grace: 无进行中调用、无持有
  grace --> active: 新调用（合并，计时作废）
  grace --> unbound: 到期：关闭通道 / unbindService
  active --> unbound: 对端死亡 / EOF
  grace --> unbound: 内存压力 / 被 LRU 挤出 / 对端死亡
```

- 通道只在有调用时建立。最后一次调用完成、且没有进行中调用与持有后进入宽限；关闭时刻为
  `max(最后一次调用完成 + graceMs, 当前租约到期)`；宽限内到来的调用合并进同一通道。
- `graceMs` 默认 **15 秒**（Hub 配置，待实现时加入 spec/hub-api.md）；租约的时长、自适应与会话结束收回以 spec/lifecycle.md（4e）为准，
  本规范只规定租约在按名寻址下的作用：**决定关闭通道的时机**，不再下发给 App 用于 App 端计时。
- **持有**：Hub 发起的通道上 App 的 `hold()`（含 handler 内的 `context.hold()`）须告知 Hub，否则 Hub 无从知道。SDK 发通知
  `app/hold { held: boolean }`（**待实现时加入 spec/protocol.md**），Hub 把 `held: true` 视为活动；持有有上限租期
  `maxHoldMs`（Hub 配置，默认 10 分钟），到期 Hub 照常关闭并记录告警——App 不能借 Hub 无限保活，需要更久的工作由 App 自行用平台
  后台机制（Android WorkManager）完成。
- App 主动 `app/sleep { reason: "app" }`：Hub 在无进行中调用时接受并立即关闭通道。

### 7.3 同时绑定上限

- Hub 同时保持的通道数上限 `maxBoundApps`（默认 **4**）。达到上限时关闭**最久未用的宽限中**通道（LRU）；全部通道都有进行中调用时
  不驱逐，新拨号照常进行并记录超限——调用不因上限而失败。

### 7.4 内存压力与 Hub 退出

- 系统给出内存压力信号时，Hub 立即关闭全部宽限中的通道。Android：`onTrimMemory`——API 34 起不再通知 `RUNNING_*` 级别、API 35
  弃用除 `UI_HIDDEN` / `BACKGROUND` 外的级别（F-07），因此以收到 `TRIM_MEMORY_BACKGROUND` 及更高级别（旧系统）为准；Hub 自己进入
  后台（`UI_HIDDEN`）时同样关闭宽限中的通道。其他平台有对应信号时同样处理。
- Hub 进程退出时系统自动解除全部绑定、关闭全部 fd；App 侧按 7.5 的对端死亡处理。

### 7.5 对端死亡通知（不用心跳）

| 平台 | Hub 感知 App 死亡 | App 感知 Hub 死亡 |
|---|---|---|
| Android | `IBinder.linkToDeath`（绑定期间注册，解绑前注销）+ fd EOF | fd EOF |
| Linux | `NameOwnerChanged`（新所有者为空）+ fd EOF | fd EOF |
| Windows | 管道读到 EOF / 管道断开 | 同左 |
| macOS | fd EOF（XPC 连接在拿到 fd 后已取消） | fd EOF |

- 心跳在这些通道上关闭（第 3 节）。
- App 被杀后实例转为"未运行 + 快照"，不发 `list_changed`（工具仍按快照列出）。
- **调用中对端死亡**：返回 `APP_NOT_RESPONDING`，`data.outcome = "unknown"`、`data.reason = "peer-died"`（`data` 允许携带额外字段，
  spec/protocol.md 第 4 节）。风险等级为 `read` 的工具 Hub 自动重拨重试一次；其他等级**不自动重试**（防止重复执行）。

### 7.6 App 内回收

- SDK 不持有 Activity / View / 窗口的强引用；`view` 工具经 LifecycleOwner / 弱引用随界面销毁注销。
- 通道关闭后释放 Rust 运行时线程与 FFI 句柄（spec/lifecycle.md 第 7 节的休眠资源要求同样适用）。
- 不使用前台服务、`START_STICKY`、周期性闹钟 / Job、电池优化豁免；`onBind` 轻量（惰性初始化，见第 8 节）。

### 7.7 测试强制

- 宽限期后断言：绑定数 / 打开的通道 fd / Hub 中与该 App 相关的定时器 / 运行时线程数归零（核心与 Hub 的确定性测试 + 各平台集成测试）。
- Android 测试接入 LeakCanary（Service、SDK 对象无泄漏）。
- 真机验收：调用 + 宽限后 App 进程状态回到 cached 并被冻结（验收步骤见 TASKS 4d-D）。

## 8. Android 功耗约束

- **绑定标志**：`BIND_AUTO_CREATE | BIND_WAIVE_PRIORITY | BIND_ALLOW_OOM_MANAGEMENT | BIND_NOT_FOREGROUND`，API 29+ 加
  `BIND_NOT_PERCEPTIBLE`——不把 App 抬到 Hub 的调度 / 内存优先级（`BIND_WAIVE_PRIORITY` 使目标留在后台 LRU 中管理），
  `BIND_ALLOW_OOM_MANAGEMENT` 允许系统在内存紧张时照常回收它。注意 `BIND_NOT_FOREGROUND` 单独使用时目标仍至少获得客户端的内存优先级，
  须与 `BIND_WAIVE_PRIORITY` 同用。不加 `BIND_ALLOW_ACTIVITY_STARTS`（Hub 不替 App 获得启动界面的权限；需要前台的工具走
  `activation: foreground` 的审批与现有唤醒）。
- **只在调用期间绑定**：结果返回后经宽限（7.2）即 `unbindService`，进程回到缓存态，由系统冻结 / 回收。
- **零定时器存活检测**：通道上关闭心跳，存活由 `linkToDeath` + fd EOF 判断。
- **Hub 侧**：不常驻前台服务，只在助手活跃时运行；发现用一次性 `queryIntentServices`（`<queries>` 声明）+ 本地缓存 + 运行期间的
  包变更接收器；不申请 `QUERY_ALL_PACKAGES`。
- **App 侧**：`onBind` 只返回 Binder，SDK 运行时在第一次 `open()` 时惰性创建；解绑即释放运行时线程；不持 WakeLock、不启动前台服务；
  超过一次调用的长任务走 `hold`（有上限，7.2）+ WorkManager。
- **OEM 拦截**：部分国产 ROM 的"关联启动 / 自启动"管控会拦截跨 App 绑定。`bindService` 返回 `false` 或抛出 `SecurityException` 时
  Hub 报 `ACTIVATION_DENIED`，错误信息与 `doctor` 给出设置指引，不回退到反复重试。

## 9. 多 Hub 共存与旧路径兼容

### 9.1 多 Hub 共存

- 每个 Hub 独立发现、独立拨号、独立保存发现记录；Hub 之间不共享状态（只是读同一批登记文件 / 安装元数据）。
- App 必须能同时接受至少 **2** 条来自不同 Hub 的通道：每条通道独立握手、独立调用队列，注册表共享，注册变更向每条通道各发
  `tools/changed`。超过 App 的通道上限（SDK 配置，默认 4）时，`Open()` / `open()` 返回错误（Hub 记 `CHANNEL_LIMIT`）。
- 同一 Hub 对同一实例同时只保持一条通道；常驻 Host 与厂商内嵌 Hub 是不同的 Hub。
- 单实例锁（spec/protocol.md 1.5）仍只约束同一配置目录的 Host，不限制其他 Hub。

### 9.2 与 App 拨 Hub（IPC / WebSocket）的并存

- **App 未登记名字时**（旧 SDK、未生成激活文件、iOS、鸿蒙、沙盒 macOS）：一切按现有路径——App 经本地 IPC / WebSocket 拨 Hub，
  休眠后经 WakeDescriptor 唤醒（spec/lifecycle.md）。
- **Hub 的路由顺序**（对每次调用）：已有的活连接（无论谁发起）→ 按名拨号（发现记录中有名字或激活方式）→ WakeDescriptor 唤醒 →
  `APP_DISCONNECTED`。
- **避免双连接**：
  - 登记了名字的 App 在非 `persistent` 模式下不主动拨 Hub；
  - `persistent` 模式的 App 仍可主动拨 Hub（旧行为）；Hub 发现某实例已有 App 发起的活连接时不再拨号；
  - 竞态（Hub 的通道与 App 发起的连接同时就绪、同一 `instanceId`）：**App 发起的连接优先**，Hub 关闭自己拨出的通道并把待派调用
    转到 App 的连接上。Hub 从不因为自己拨了号而关闭 App 发起的连接（否则 App 会按断线重连，形成循环）。
- **版本演进**：名字接口带主版本（D-Bus `dev.appmcp.App1`、AIDL 接口描述符带版本、XPC 消息 `op` 带版本），Hub 优先用自己支持的
  最高版本；握手中的 `protocolVersion` 规则不变。

## 10. 安全

### 10.1 同用户

- 桌面平台的信任边界与现有本地 IPC 相同（spec/protocol.md 1.4）：同一操作系统用户即同一信任域。D-Bus 会话总线、命名管道 DACL、
  launchd 用户域都限于当前用户；两端仍各自核对对端 uid / SID（4.1、4.3、4.4），不同即拒绝。

### 10.2 Hub 信任（App 侧，Android）

Android 上每个 App 是不同的 uid，"同用户"不成立，App 须判断拨号的 Hub 是否可信：

- `open()` 内以 `Binder.getCallingUid()` 取调用方包与签名证书，按以下规则放行：①签名证书摘要在 App 配置的可信 Hub 列表中
  （构建期写入）；②用户曾在 App 内确认过该 Hub（与现有配对同构，记录证书摘要）。都不满足时拒绝（`HUB_NOT_TRUSTED`）；不在冷启动绑定中
  弹出界面。
- **不以签名级权限作为唯一手段**（修正 TASKS 4d-D 的 `dev.appmcp.permission.BIND_TOOLS`）：`signature` 级权限只授予与**定义该权限的
  App** 同证书签名的 App（F-05）；系统不允许不同证书的包定义同名权限（后安装者失败）；被要求而无人定义的权限是"孤儿权限"，恶意 App
  可抢先定义并获得它（F-06）。因此：
  - 若由每个 App 各自定义 `dev.appmcp.permission.BIND_TOOLS`：第二个不同开发者的 App 无法安装——**禁止**；
  - 若由 Hub 定义：只有与该 Hub 同证书的 App 能拿到，且不同厂商的 Hub 互相冲突，Hub 未安装时还会成为孤儿权限——**不采用**；
  - 可选附加：App 自己定义**以包名为前缀**的权限（`<包名>.permission.BIND_APPMCP_TOOLS`，与现有 `WakeReceiver` 的写法一致），
    `protectionLevel="knownSigner"`（API 31+）+ `knownCerts` 列出可信 Hub 证书；Hub 须在自己的清单中预先 `uses-permission` 这些名字，
    只适合 App 与 Hub 事先约定的场景。默认不启用，以 `open()` 内的调用方校验为准。

### 10.3 自我声明不是授权（原则 7）

- 安装元数据、登记文件、清单、自报、总览都是 App 的**自我声明**：它们决定"列出什么、如何激活"，不决定"能不能执行"。执行仍由工具
  风险等级与审批决定。
- Hub 核对身份对应关系：Android 包名 ↔ appId ↔ 签名证书；Windows 登记文件 `executable` ↔ 管道服务端进程映像 ↔ Authenticode 发布者；
  macOS bundle id ↔ Team ID；Linux 名字所有者 uid ↔ 可执行文件路径。对应关系与首次记录不一致 → `PEER_IDENTITY_MISMATCH`（拒绝拨号或
  关闭通道）或 `fingerprintChanged`（5.4，提示并要求确认）。
- 名字抢注：同一用户下的恶意进程可能抢先拥有某个名字（D-Bus 名、管道名）。同用户本就是同一信任域，但 Hub 仍以 10.3 的对应关系检测
  "名字所有者不是登记的那个程序"，不一致时拒绝并在 `doctor` 中报告。

## 11. 调试

`app-mcp-host doctor` 增加名字服务检查组（检查项 ID 以 `naming.` 开头，`--json` 中同名）：

| 检查项 | 内容 |
|---|---|
| `naming.discovery` | 每个 App 的发现来源（`source`、首次 / 最近见到时间）、指纹状态、是否可激活 |
| `naming.dbus`（Linux） | 会话总线可达；`dev.appmcp.App.*` 激活文件与名字列表；激活文件中 `Exec` 指向的程序是否存在 |
| `naming.android`（Android 端 Hub，或经 adb） | `dev.appmcp.TOOLS` Service 声明、`<meta-data>` 清单资源、最近一次绑定结果（含 OEM 拦截提示） |
| `naming.pipes`（Windows） | `appmcp-<SID>-*` 管道列表、App 登记文件与其 `executable` 是否存在 |
| `naming.launchd`（macOS） | `dev.appmcp.App.*` Agent 登记与 Mach 服务 |
| `naming.bindings` | 当前通道 / 绑定数与上限、每条通道的宽限剩余时间、最近的对端死亡事件 |

人工排查命令（文档给出用法）：

| 平台 | 命令 |
|---|---|
| Linux | `busctl --user list \| grep dev.appmcp`、`busctl --user introspect dev.appmcp.App.<id> /dev/appmcp/App`、`dbus-monitor --session "type='signal',member='NameOwnerChanged'"` |
| Android | `adb shell dumpsys activity services <包名>`、`adb shell dumpsys package <包名>`（Service 与权限声明）、`adb shell dumpsys activity processes`（进程状态 / 冻结） |
| Windows | PowerShell `Get-ChildItem \\.\pipe\ \| Where-Object Name -like 'appmcp-*'`（非官方文档化的用法，仅供人工排查；`doctor` 不依赖它，以登记文件为准，U-06） |
| macOS | `launchctl print gui/$(id -u)/dev.appmcp.App.<id>`（输出不是稳定接口，`doctor` 不解析，只提示命令） |

Android 冻结状态：`adb shell dumpsys activity | grep -A 20 "Apps frozen:"`。

## 12. 错误码（新增，待合入）

以下连接级错误码**待 4d 实现时加入 spec/protocol.md 第 10.1 节**（当前不改该表，以免与进行中的 4e 冲突；`ConnectionErrorCode` 与
`reason()` / `hint()` 届时同步）。在合入之前，实现不得假定这些码已存在。

| code | 类别 | 出现在 | 原因 | 修复建议 |
|---|---|---|---|---|
| `INVALID_ADDRESS` | naming | 配置解析、Hub API | 地址不符合 2.1 的规范形式 | 按 `appmcp://<appId>[/<instance>]` 书写 |
| `NAME_NOT_FOUND` | naming | Hub `last_error`、`doctor` | 地址没有对应的名字，也没有激活方式（App 未登记或已卸载） | 安装 App 或 `app-mcp-host app install`；`doctor` 查看发现来源 |
| `ACTIVATION_DENIED` | naming | Hub `last_error`、调用错误 `data.code` | 系统拒绝激活（Android 关联启动 / 自启动拦截、`bindService` 返回 false、D-Bus 激活被拒） | 在系统设置中允许该 App 被关联启动；`doctor` 给出指引 |
| `ACTIVATION_TIMEOUT` | naming | 同上 | 已发出激活，但名字 / 通道在超时内没有出现 | 检查 App 是否启动失败（App 日志）；登记的激活方式是否正确 |
| `BIND_PERMISSION_DENIED` | naming | 同上 | Hub 无权拨号（Android `SecurityException`、D-Bus `AccessDenied`、管道拒绝访问） | 以同一用户运行；Android 检查权限声明 |
| `HUB_NOT_TRUSTED` | naming | SDK 日志、Hub `last_error` | App 拒绝了该 Hub（10.2） | 在 App 内确认该 Hub，或把 Hub 证书加入 App 的可信列表 |
| `PEER_IDENTITY_MISMATCH` | identity | Hub `last_error`、`doctor` | 名字的所有者与登记的程序 / 用户 / 签名不一致 | 停止占用名字的进程；`doctor` 给出对端进程 |
| `FINGERPRINT_CHANGED` | identity | Hub 事件、`doctor`（警告） | App 签名指纹与首次记录不同 | 确认是正常升级后在审批中允许 |
| `PEER_DIED` | disconnect | Hub 实例状态、SDK `backoff` 不适用（Hub 侧） | 系统对端死亡通知（`linkToDeath`、`NameOwnerChanged`） | 无需处理；频繁出现时查看 App 崩溃日志 |
| `CHANNEL_LIMIT` | naming | Hub `last_error` | App 的同时通道数已达上限（9.1） | 关闭其他 Hub，或调大 App 的通道上限 |
| `NATIVE_HOST_UNAVAILABLE` | browser | 网页 SDK / 扩展 | 扩展连接不到 Native Messaging 宿主（宿主清单缺失、Host 未安装） | `app-mcp-host service install` 写入宿主清单 |
| `MESSAGE_TOO_LARGE` | browser | 调用错误 `data.code` | 单条消息超过 Native Messaging 上限 | 减小参数 / 结果，或改用分页资源 |

工具调用层继续使用 spec/protocol.md 第 4 节的类别：激活被拒 / 超时 → `LAUNCH_FAILED`（`data.code` 为上表的码）；确认未安装 →
`APP_NOT_INSTALLED`；调用中对端死亡 → `APP_NOT_RESPONDING` + `data.outcome = "unknown"`（7.5）。

## 13. 事实 / 未知 / 风险

平台事实于 2026-10-01 按官方文档核对（原文抓取，不凭记忆）；编号只增不改。

### 13.1 事实（本规范依据的已确认接口与平台 API）

项目内（代码 / 现行规范）：

| # | 事实 | 来源 |
|---|---|---|
| F-01 | appId 规则 `[a-z][a-z0-9-]{0,62}`；保留名 `apps` / `os` / `ax` / `host`；握手还拒绝上游名与空 `instanceId` | `crates/protocol/src/messages.rs` `is_valid_app_id`；`crates/manifest/src/lib.rs` `RESERVED_APP_IDS`；`crates/hub/src/app_server.rs` 握手校验 |
| F-02 | 所有传输上同一 HTTP/1.1 + WebSocket 帧，SDK 是 WebSocket 客户端、连接后先发 `app/hello`；IPC 连接级鉴权（`SO_PEERCRED` / 管道 DACL + 所有者 SID） | spec/protocol.md 1.1、1.2、1.4、5.1 |
| F-03 | 现有休眠 / 唤醒、`wakeToken`、`toolsHash` 快速恢复、租约、唤醒去重与认领规则 | spec/lifecycle.md 第 4、6、9 节；spec/hub-api.md 3.5 |
| F-04 | 连接级错误码表的单一实现 `app_mcp_protocol::diagnostic::ConnectionErrorCode`（测试核对表格）；工具错误 `data` 可带额外字段 | spec/protocol.md 第 4、10 节 |
| F-09 | 管道名长度检查 `check_pipe_name`（256 UTF-16 码元）已存在并已在 Windows 实测 | spec/protocol.md 10.1；TASKS 4b 结果 |
| F-10 | 4e0 真机测量：本地 IPC / 回环断开由 EOF / RST 在毫秒级感知；一次回连 SDK 线程约 5.6 ms；Flyme 后台约 62 s 冻结缓存进程 | TASKS 4e0 结果 |
| F-11 | `app-mcp-plan.md` 10.4 的 `appmcp://` 调用链接未实现（代码中无该 scheme 的处理） | 全仓搜索 `appmcp://` 仅命中计划文档 |
| F-12 | `app-mcp-host app install` 子命令尚不存在（`crates/host/src/cli.rs` 只有 `service install`）；`doctor` 检查项以 `id` 区分 | `crates/host/src/cli.rs`、`crates/host/src/doctor.rs` |
| F-13 | Android SDK 现有唤醒：`dev.appmcp.android.WakeReceiver`、广播 `dev.appmcp.action.WAKE`、可选 `<包名>.permission.APP_MCP_WAKE` | `sdks/kotlin/app-mcp-android/src/main/AndroidManifest.xml` |

Android（developer.android.com / source.android.com）：

| # | 事实 | 来源 |
|---|---|---|
| F-05 | `signature` 权限只授予与定义该权限的 App 同证书签名的 App；不同证书的包不能定义同名权限；`knownSigner` + `knownCerts` 为 API 31 | `guide/topics/manifest/permission-element`、`guide/topics/permissions/defining` |
| F-06 | 无人定义的被要求权限为"孤儿权限"，恶意 App 可抢先定义并获得 | `privacy-and-security/risks/custom-permissions` |
| F-07 | `onTrimMemory`：API 34 起不再通知 `RUNNING_*`；API 35 弃用除 `UI_HIDDEN` / `BACKGROUND` 外的级别 | `reference/android/content/ComponentCallbacks2` |
| F-08 | 冻结器：Android 11+；14+ 进入缓存态 10 s 后冻结；同步 Binder 调用打到冻结进程会杀死该进程、调用方得 `RemoteException`；以 `BIND_WAIVE_PRIORITY` 绑定的 Service 在其客户端都缓存前不冻结；`dumpsys activity` 的 "Apps frozen" | `source.android.com/docs/core/perf/cached-apps-freezer` |
| F-14 | 绑定标志：`BIND_AUTO_CREATE`（1）、`BIND_NOT_FOREGROUND`（8，目标仍至少得客户端内存优先级）、`BIND_WAIVE_PRIORITY` / `BIND_ALLOW_OOM_MANAGEMENT`（14）、`BIND_NOT_PERCEPTIBLE`（29）；`bindService` 需显式组件 | `reference/android/content/Context` |
| F-15 | 包可见性（API 30）：`<queries><intent>` 使匹配 intent-filter 的 App 可见；`QUERY_ALL_PACKAGES` 在 Play 上需审批；绑定了你的 Service 的 App 自动对你可见 | `training/package-visibility` |
| F-16 | `getChangedPackages(seq)`（API 26）返回自序号以来变化的包，序号每次开机归零 | `reference/android/content/pm/PackageManager` |
| F-17 | API 26+ 清单注册的接收器收不到 `PACKAGE_ADDED` / `REPLACED`（`PACKAGE_FULLY_REMOVED` 例外）；动态注册不受限 | `develop/background-work/background-tasks/broadcasts/broadcast-exceptions` |
| F-18 | `linkToDeath` / `binderDied`；`Binder.getCallingUid()` 只在事务内返回调用方 uid（事务外返回自身）；`onBind` 只对首个客户端调用、`IBinder` 被缓存；`hasSigningCertificate`（API 28） | `reference/android/os/IBinder`、`.../os/Binder`、`guide/components/bound-services` |
| F-19 | `ParcelFileDescriptor.createSocketPair`（API 19），PFD 可经 AIDL 传递；`<meta-data android:resource>` 经 `GET_META_DATA` + `loadXmlMetaData` 读取 | `reference/android/os/ParcelFileDescriptor`、`guide/topics/manifest/meta-data-element` |
| F-34 | 后台限制不影响绑定式 Service（"other components can bind… whether or not your app is in the foreground"）；API 34 起可见 App 绑定后台 App 时需 `BIND_ALLOW_ACTIVITY_STARTS` 才转交启动界面的权限 | `about/versions/oreo/background`、`about/versions/14/behavior-changes-14` |

Linux D-Bus（dbus.freedesktop.org、specifications.freedesktop.org）：

| # | 事实 | 来源 |
|---|---|---|
| F-20 | 总线名元素 `[A-Za-z0-9_-]`、`-` 不推荐（建议换 `_`）、≤ 255、≥ 2 元素、元素不以数字开头；对象路径与接口名元素只允许 `[A-Za-z0-9_]` | D-Bus 规范 "Valid Names" |
| F-21 | 会话服务激活目录：`$XDG_DATA_HOME/dbus-1/services`、`$XDG_DATA_DIRS/*/dbus-1/services` 等；`$XDG_RUNTIME_DIR/dbus-1/services` 不被 inotify 监视、需 `ReloadConfig`；键 `Name=` / `Exec=`；未带 `NO_AUTO_START` 的消息会激活目标名字 | D-Bus 规范 "Message Bus Starting Services"、dbus-daemon(1) |
| F-22 | `UNIX_FD`（`h`）经 `NEGOTIATE_UNIX_FD` 协商；`ListActivatableNames`、`NameOwnerChanged`、`GetConnectionUnixUser`、`GetConnectionCredentials`（UnixUserID、ProcessID 等）；默认会话策略允许同用户任一连接拥有任一名字 | D-Bus 规范；`bus/session.conf.in` |

Windows（learn.microsoft.com）：

| # | 事实 | 来源 |
|---|---|---|
| F-23 | 虚拟化的打包 App 在 `AppData\Local` 新建的文件写入包私有位置、只对该 App 可见；可用 `unvirtualizedResources` / Win11 `ExcludedDirectories` 排除 | `windows/msix/desktop/desktop-to-uwp-behind-the-scenes`、`.../flexible-virtualization` |
| F-24 | 管道名 ≤ 256 字符、不区分大小写；`WaitNamedPipe` 在无实例时立即返回失败；`FILE_FLAG_FIRST_PIPE_INSTANCE`、`PIPE_REJECT_REMOTE_CLIENTS`、`GetNamedPipeClientProcessId`；官方建议从注册表 / 文件等持久来源得知管道名 | `win32/ipc/pipe-names`、`CreateNamedPipe`、`WaitNamedPipe` |
| F-25 | `ReadDirectoryChangesW`：缓冲溢出时须重新枚举目录 | `win32/api/winbase/nf-winbase-readdirectorychangesw` |
| F-26 | AppExtension 支持 UWP 与打包桌面 App（`AppExtensionCatalog.FindAllAsync` 与安装 / 更新 / 卸载事件）；App Service 需要包身份；`com:ExeServer` 对应 LocalServer32 | `uwp/launch-resume/how-to-create-an-extension`、`apps/desktop/modernize/modernize-packaged-apps` |

macOS / iOS（developer.apple.com、Xcode man pages）：

| # | 事实 | 来源 |
|---|---|---|
| F-27 | `MachServices` 在 launchd 引导命名空间登记服务，不设 `KeepAlive` 时按需启动；`xpc_connection_create_mach_service` 要求名字在 launchd plist 中声明；`xpc_dictionary_set_fd`、`xpc_connection_get_euid` 公开；Apple 称不支持 App 之间直接 XPC，建议经 launchd 作业会合 | launchd.plist(5)；`documentation/xpc`；Developer Forums thread 715338（Apple 工程师） |
| F-28 | App Group 容器中的 Unix 套接字只对同一 Team ID 的进程可用；沙盒进程查找全局 Mach 服务需临时例外权利 | `bundleresources/entitlements/com.apple.security.application-groups`；Forums thread 742759 |
| F-29 | `SMAppService.agent(plistName:)`（macOS 13+，plist 位于包内 `Contents/Library/LaunchAgents`，需用户批准）；`xpc_connection_set_peer_code_signing_requirement` macOS 12+；XPC 无公开审计令牌接口，Unix 套接字有 `LOCAL_PEERTOKEN`；`launchctl print` 输出不是接口 | `servicemanagement/smappservice`；`xpc/xpc_connection_set_peer_code_signing_requirement`；launchctl(1) |
| F-30 | App Intents（iOS 16+）是向系统暴露动作的机制；launchd / XPC 服务接口只在 macOS 提供 | `documentation/appintents`；`xpc_connection_create_mach_service` 平台可用性 |

浏览器（developer.chrome.com、MDN、W3C）：

| # | 事实 | 来源 |
|---|---|---|
| F-31 | Native Messaging：宿主清单 `name`（小写字母数字、`_`、`.`）/ `path` / `type: "stdio"` / `allowed_origins`；各平台用户级位置；帧为 32 位本机字节序长度 + UTF-8 JSON；宿主→扩展 1 MB、扩展→宿主 64 MiB；端口存在期间浏览器保持宿主运行；Firefox 用 `allowed_extensions`，Edge 有自己的注册表项 | `extensions/develop/concepts/native-messaging`；MDN Native manifests；Edge native-messaging |
| F-32 | `connectNative()` 使 MV3 service worker 保持存活（Chrome 105+） | `extensions/develop/concepts/service-workers/lifecycle` |
| F-33 | WebMCP 规范草案（W3C Web ML CG，2026-09-30）接口为 `document.modelContext`；Chrome 149 起源试用 | `webmachinelearning.github.io/webmcp`；Chrome 博客 ai-webmcp-origin-trial |
| F-35 | Web App Manifest 的未知成员被实现忽略（可放自定义成员） | `w3c.github.io/manifest` |

### 13.2 未知（每项给出处理方式）

| # | 未知 | 处理方式 |
|---|---|---|
| U-01 | 读其他 App 的 `<meta-data>` 资源是否确实不启动其进程 | **验证**：真机上对未运行的示例 App 读取后查 `dumpsys activity processes`；若会启动则改为只在包变更时读取并缓存 |
| U-02 | `bindService` 后、`onServiceConnected` 时目标是否一定已解冻；Flyme 冻结行为（4e0 观察到约 62 s，非 AOSP 的 10 s） | **验证**（真机）+ **保守处理**：只在 `onServiceConnected` 后 `open()`，`RemoteException` 按激活失败、不在同一调用内重试 |
| U-03 | 国产 ROM"关联启动 / 自启动"拦截时 `bindService` 的表现（返回 false / 抛异常 / 静默不回调） | **显式失败**：绑定超时与 false 都报 `ACTIVATION_DENIED` 并给出设置指引；**验证**：Flyme 真机 |
| U-04 | 同一 Hub 跨用户（Android 多用户 / 工作资料）时的可见性与绑定 | **明确不在范围**：只支持当前用户 |
| U-05 | dbus-broker 是否自动发现新的 `.service` 文件 | **保守处理**：写入后总是调用 `ReloadConfig`；**验证**：Fedora（dbus-broker）与 Debian（dbus-daemon）各一次 |
| U-06 | 枚举 `\\.\pipe\` 无官方文档 | **保守处理**：Hub / `doctor` 不枚举管道，只用登记文件；命令仅作人工排查 |
| U-07 | 打包 App 的激活方式（COM `ExeServer` / App Service）能否被未打包的 Hub 调用 | **验证**（Windows 实机）；确认前打包 App 走协议激活 + 等待管道 |
| U-08 | 哪些打包形态属于"被虚拟化"（RuntimeBehavior / TrustLevel 组合） | **保守处理**：打包 App 一律不依赖自写登记文件，用 AppExtension 声明；**验证** |
| U-09 | macOS 上 launchd 作业进程与用户打开的 GUI 进程如何共享工具 | **待确认**（4d-F 设计）；确认前 GUI 进程走现有 App 拨 Hub 路径 |
| U-10 | WSL 中的 Hub 能否打开 Windows 侧命名管道、激活 Windows App | **验证**（interop 实测）；不能则 WSL Hub 只服务 Linux App |
| U-11 | 自报登记文件写入（5.5 载体 1）对开发者是否可接受（首次运行的文件副作用） | **待确认**；提供关闭开关，默认开启仅限桌面平台 |
| U-12 | `maxBoundApps = 4`、`graceMs = 15 s`、`maxHoldMs = 10 min` 是否合适 | **验证**：真机功耗测量后调整；均为配置项 |
| U-13 | 4e 对租约（自适应、会话结束收回）的最终规则 | **待确认**：以 4e 完成后的 spec/lifecycle.md 为准，本规范只引用"租约决定关闭时机" |
| U-14 | 扩展 / 宿主能否读取已安装 PWA 的清单 | **待确认**；确认前已安装 PWA 只在打开时被发现 |
| U-15 | `app/register`、`app/hold` 的最终消息形态 | **待确认**：4d 实现时与 spec/protocol.md 合入一起定稿；之前不得实现为公开协议 |
| U-16 | 一个 App 同时接受多个 Hub 通道需要核心如何支持（多会话共享注册表） | **待确认**（4d-B 设计）；确认前通道上限按 1 实现并对第二个返回 `CHANNEL_LIMIT` |
| U-17 | `app-mcp-plan.md` 10.4 调用链接与本地址共用 `appmcp://` | **待确认**：本规范已规定第一个路径段为实例；调用链接实施时改用其他形式 |
| U-18 | Linux 上无平台签名，"签名指纹"只能是路径 + 包管理器归属 | **保守处理**：Linux 指纹变化只提示不阻断；记录为限制 |
| U-19 | Electron / Tauri 等混合应用的名字登记由主进程还是渲染进程负责 | **待确认**（按原生规则由主进程负责为默认假设） |

### 13.3 风险与限制手段

| # | 风险 | 领域 | 限制手段 |
|---|---|---|---|
| R-01 | 绑定 / 通道泄漏，App 被长期保活，费电 | 功耗 / 资源 | 零活引用不变式 + 宽限后归零的测试（7.7）；`maxHoldMs`；LRU 上限；内存压力即解绑；`doctor` `naming.bindings` 可观测 |
| R-02 | 冻结进程上的同步 Binder 调用导致 App 被杀 | 功耗 / 兼容 | Binder 上只有一次 `open()` 且在 `onServiceConnected` 之后；其余消息走 fd；`RemoteException` 不重试（U-02） |
| R-03 | 调用中 App 被杀导致写操作重复执行 | 正确性 | `outcome: "unknown"`；非 `read` 工具不自动重试（7.5） |
| R-04 | Android 权限设计错误（同名签名权限冲突、孤儿权限被抢先定义） | 安全 | 不以签名级权限为唯一手段，`open()` 内调用方 uid + 证书校验（10.2）；附加权限必须以包名为前缀 |
| R-05 | 同用户恶意进程抢注 D-Bus 名 / 管道名冒充 App | 安全 | 拨号前核对名字所有者 uid / 进程映像与登记一致（10.3）；`FILE_FLAG_FIRST_PIPE_INSTANCE`；不一致即 `PEER_IDENTITY_MISMATCH` |
| R-06 | 自报 / 安装信息被当作授权 | 安全 | 原则 7：只决定列出与激活，执行由风险与审批决定；指纹变化强制确认（5.4） |
| R-07 | OEM 拦截关联启动，按名寻址在部分手机上不可用 | 兼容 / OEM 差异 | 显式 `ACTIVATION_DENIED` + 设置指引；保留 `WakeReceiver` + WorkManager 旧路径（4.2） |
| R-08 | 新旧路径并存导致双连接、重连循环 | 兼容 | App 发起的连接优先、Hub 不因自己拨号关闭它（9.2）；登记名字的 App 非 `persistent` 不主动拨 Hub |
| R-09 | 多 Hub 同时调用同一 App 互相干扰 | 并发 | 每通道独立握手与调用队列；通道上限 + `CHANNEL_LIMIT`；未支持多会话前上限为 1（U-16） |
| R-10 | Native Messaging 端口常开使宿主与 service worker 常驻；1 MB 上限 | 功耗 / 兼容 | 只在有实例需要时持有端口；超限 `MESSAGE_TOO_LARGE` 显式失败 |
| R-11 | 发现记录不按时间过期，可能残留已卸载 App | 数据 | 卸载事件、元数据 / 可执行文件不存在、激活确认未安装三种清除条件；`doctor` 列出来源以便人工排查 |
| R-12 | 平台 API 在新系统版本变化（Android 冻结策略、`onTrimMemory` 弃用、MSIX 虚拟化） | 兼容 | 平台事实集中在本节并标注来源与日期；各平台集成测试与真机验收；配置项可回退到旧路径（App 不登记名字即走旧路径） |
| R-13 | 未覆盖的平台（iOS、鸿蒙、沙盒 macOS）行为不一致 | 兼容 | 明确列为"不支持按名寻址"，沿用现有路径，不做部分实现 |
| R-14 | 未知的未知（如系统名字服务异常、激活风暴） | 稳定性 | 每 App 唤醒 / 拨号速率上限（复用 4e O4）；拨号去重；所有失败带错误码进入 `last_error` 与 `doctor` |
