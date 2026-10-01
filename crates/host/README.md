# app-mcp-host

本地 MCP Host：把本机各 App（通过 `@app-mcp/web` 等 SDK 连接）以及已有的 MCP 服务器聚合起来，
以一个 MCP Server 暴露给模型。协议与行为见 `spec/protocol.md`、`spec/manifest.md`。

本 crate 是命令行程序（`src/lib.rs`：命令行、配置、单实例探测、日志轮转、服务安装；`src/main.rs` 与
Windows 无窗口版 `src/bin/app-mcp-hostw.rs` 只是入口）与集成测试；Hub 的全部实现位于 `crates/hub`（`app-mcp-hub`，
可嵌入厂商自有 Agent，见 `crates/hub/README.md` 与 `spec/hub-api.md`）。

## 推荐接入：常驻服务 + HTTP

一个常驻的 `app-mcp-host serve` 进程提供：

- **一个 HTTP 端口**（默认 `127.0.0.1:7717`，`listen`），按路径分流（spec/protocol.md 1.3）：
  - `/app`：App 连接（WebSocket 升级）——网页 SDK（`ws://127.0.0.1:7717/app`），以及显式配置 `ws://` 的原生 App；
  - `/mcp`：MCP Streamable HTTP——每个 MCP 客户端（Claude Code、Claude Desktop、IDE……）各自建立一个 HTTP 会话，
    **共享同一组 App 连接**；每个会话有独立的 `apps.select` 选择与“首次接触附带总览”状态；
  - `/healthz`：Host 身份（`service`、`version`、`user`、`pid`）与监听信息；
  - `/status`：运行状态（各 App 实例在线 / 休眠 / 唤醒中、连接 ID、最近错误、SDK 诊断上报），`doctor` / `status` 读取；
    本地 IPC 直接允许，TCP 必须带令牌（未配置令牌时 403）。
- **本地 IPC**（原生 App 默认）：Linux `$XDG_RUNTIME_DIR/app-mcp/hub.sock`（未设置时 `~/.app-mcp/run/hub.sock`）、
  macOS `~/.app-mcp/run/hub.sock`、Windows 命名管道 `\\.\pipe\app-mcp-<用户 SID>`；只接受同一用户的进程（见下方「本地 IPC」）。

默认端口被占用（且没有显式配置 `listen`）时依次改用 `7737`、`7757`（网页 SDK 按同一顺序尝试）；实际监听位置写在
登记文件 `~/.app-mcp/run/endpoints.json`，原生 SDK、`service status` 与测试都读它。

这样解决了 stdio 模式的根本问题：stdio 下每个 MCP 客户端各起一个 Host 进程，而 App 端口只能被一个进程占用。

```bash
# 安装为当前用户的登录自启服务（无需管理员）并立即启动；给出的参数写入 ~/.app-mcp/config.json
app-mcp-host service install --manifest ./examples/shop/app-mcp.json
app-mcp-host service status        # 安装与运行状态；未运行时退出码 3
app-mcp-host status                # 一行摘要（pid、版本、地址、App 在线 / 休眠数）；未运行时退出码 3
app-mcp-host doctor [--json]       # 诊断：逐项给出结论与修复建议；有错误时退出码 1
app-mcp-host service stop | start
app-mcp-host service uninstall     # 停止并删除服务文件

# 或临时在前台运行（Ctrl+C 退出）；同一配置目录已有实例在运行时打印其信息并以退出码 0 退出
app-mcp-host serve
```

客户端配置（Claude Code：`claude mcp add --transport http app-mcp http://127.0.0.1:7717/mcp`）：

```json
{ "mcpServers": { "app-mcp": { "type": "http", "url": "http://127.0.0.1:7717/mcp" } } }
```

**从旧版本迁移**（合并端口之前 App 连接在 `7717`、MCP 在 `7718`）：MCP 客户端配置改为 `http://127.0.0.1:7717/mcp`。
兼容期内：旧 SDK 以根路径 `ws://127.0.0.1:7717` 连接仍被接受（首次出现时日志提示升级）；仍需旧 MCP 端口时显式设置
`http.addr` / `--http 127.0.0.1:7718`，Host 另开一个同样的监听器并记录弃用提示（默认不开）；配置文件中的 `wsAddr` /
`--ws-addr` 按 `listen` 使用并提示改名（与 `listen` 同时设置且不同时报错）。

### 一条命令安装：`setup` / `uninstall`

```bash
app-mcp-host setup                 # 或 npx appwire-cli setup / uvx appwire-cli setup；幂等，可重复执行
app-mcp-host setup --dry-run       # 只列出计划（读取 Agent 现有配置，不做任何修改）
app-mcp-host setup --agents claude-code,cursor --force --json
app-mcp-host uninstall [--purge] [--dry-run] [--json]
```

`setup` 依次：

1. **二进制就位**：当前程序位于包管理器目录（路径含 `node_modules`、`site-packages` 或 `_npx`，即 npx / uvx 缓存、
   全局 npm、pip）时复制到 `<home>/bin/`（Windows 连同 `app-mcp-hostw.exe`），内容相同时不复制；否则原地使用。
   选 `<home>/bin/`：与配置同在每用户目录、无需管理员、服务用绝对路径不依赖 PATH，`uninstall --purge` 可整体删除，
   也不会覆盖用户 PATH 中自行管理的同名程序。
2. **登录自启**：与 `service install` 同一实现（写 `config.json`、端口预检、令牌、安装并启动），等待 `/healthz` 就绪，
   从 `<home>/run/endpoints.json` 取实际监听地址（不假设 7717）。
3. **写入已安装 Agent 的 MCP 配置**（条目名 `app-mcp`，Streamable HTTP）：

   | Agent（`--agents`） | 检测 | 写入方式 | 依据 |
   |---|---|---|---|
   | `claude-code` | PATH 中的 `claude` | `claude mcp add --scope user --transport http`，读 `claude mcp get`，删 `claude mcp remove --scope user` | 本机 Claude Code 2.1.281 `--help` 与隔离 `CLAUDE_CONFIG_DIR` 实测 |
   | `codex` | PATH 中的 `codex` | `codex mcp add <名> --url`，读 `codex mcp get --json`，删 `codex mcp remove` | 本机 codex-cli 0.156.1 `--help` 与隔离 `CODEX_HOME` 实测 |
   | `gemini` | PATH 中的 `gemini` | `gemini mcp add --scope user --transport http`，读 `~/.gemini/settings.json`，删 `gemini mcp remove --scope user` | 本机 gemini 0.46.0 `--help` 与隔离 `HOME` 实测 |
   | `cursor` | `~/.cursor` 存在 | 文件 `~/.cursor/mcp.json`：`mcpServers.app-mcp = {"url": …}` | cursor.com/docs/context/mcp |
   | `vscode` | `<用户配置目录>/Code/User` 存在 | 文件 `…/Code/User/mcp.json`：`servers.app-mcp = {"type": "http", "url": …}`（默认 profile） | code.visualstudio.com 文档 mcp-servers、profiles |
   | `windsurf`、`claude-desktop` | 不检测 | 只打印手动说明（`--agents` 显式列出时） | 位置 / 格式无法从官方文档确认 |

   - 已有同名条目：URL 相同 → 不动；是本程序以前写入的（`setup.json` 中的 URL，如 Host 换了端口）→ 更新；
     其他内容 → **不覆盖**并报冲突，加 `--force` 才替换；Claude Code 中不在用户作用域的同名条目 `--force` 也不动。
   - 写入前把被修改的配置文件备份到 `<home>/backups/`；写入后用同一方式回读校验，不符即回滚
     （文件自写入后未再变化时恢复备份，否则只删本条目）。
   - 配置文件不是标准 JSON（含注释、尾逗号）时不写，打印手动配置说明。
   - 令牌策略为 `all` 时不写任何 Agent（不把令牌写进 Agent 配置），打印带 `Authorization` 头的手动说明。
4. **doctor 自检**并输出摘要。改动清单写入 `<home>/setup.json`；有失败 / 冲突 / doctor 错误时退出码 1。

`uninstall` 只按 `setup.json` 撤销：Agent 配置文件自 setup 写入后未变化 → 逐字节恢复为备份（原本不存在则删除）；
已有其他变化 → 只删除 URL 仍为写入值的 `app-mcp` 条目（被 `--force` 替换的原条目此时不能自动还原，提示备份位置）；
卸载 setup 安装的服务；`--purge` 同时删除 `<home>/bin`。没有 `setup.json` 时什么都不做（手动安装的服务用
`service uninstall`）。实现见 `src/setup/`（Agent 策略表 `src/setup/agents/`，每种 Agent 一个模块并注明依据）。

### 各平台的服务形式

| 平台 | 形式 | 位置 | 说明 |
|---|---|---|---|
| Linux | systemd 用户单元 | `~/.config/systemd/user/app-mcp-host.service` | `systemctl --user enable` + `restart`；`Restart=on-failure`；日志也进 `journalctl --user -u app-mcp-host`。没有 systemd 用户实例时报错并提示改用登录脚本运行 `serve` |
| macOS | launchd LaunchAgent | `~/Library/LaunchAgents/dev.app-mcp.host.plist` | `launchctl bootstrap gui/<uid>`；`RunAtLoad`，`KeepAlive.SuccessfulExit=false`（异常退出才重启） |
| Windows | 当前用户登录启动项 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 值 `app-mcp-host` | 指向同目录的 **`app-mcp-hostw.exe`**（GUI 子系统，同一份代码，不创建控制台窗口）；`service start` 以 `DETACHED_PROCESS \| CREATE_NO_WINDOW` 启动；`service stop` 按登记文件中的 pid（经 `/healthz` 确认）结束进程 |

服务文件内容由代码生成，启动命令统一为 `<可执行文件绝对路径> serve --home <配置目录>`，其余设置都在配置文件中。
选 `Run` 项而不是计划任务：两者都无需管理员，但计划任务运行控制台程序会弹窗，且 `Run` 项最简单、可在“任务管理器 > 启动”中查看和禁用。
Hub 在 Windows 上启动的子进程（唤醒命令、上游 MCP 服务器）一律带 `CREATE_NO_WINDOW`，常驻进程不会间接弹出控制台。

### 本地 IPC

原生 App（C / C++ / C# / Dart / Kotlin JVM / Swift / Python / Node / Rust）默认连接本地 IPC 端点，不经 TCP；
网页只能用 WebSocket。两者上的协议完全相同（IPC 上同样是 WebSocket 帧，spec/protocol.md 第 1 节）。

- 端点：配置文件 `ipcEndpoint`（`"unix:<绝对路径>"` / `"pipe:\\\\.\\pipe\\<名称>"`，`"none"` 关闭）或 `--ipc-endpoint`；
  缺省为上面的平台默认端点。改了端点或关闭 IPC 时，原生 SDK 从登记文件读到实际端点（IPC 关闭时为 `ws://<listen>/app`）；
  也可设置环境变量 `APP_MCP_ENDPOINT` 或在代码中配置端点。SDK 不会在连不上时自动换用其他传输。
- 鉴权：Unix 上套接字目录属于当前用户且为 0700、套接字 0600，并逐连接核对对端用户 ID（`SO_PEERCRED` / `getpeereid`）；
  Windows 上管道的 DACL 只允许当前用户、所有者为当前用户，拒绝远程客户端；SDK 也核对监听方是同一用户（防抢占）。
- Hub API 的 `InstanceInfo.pid` 为 IPC 连接的对端进程号。
- **MCP over IPC**：`serve` 在 IPC 端点上同样提供 `/mcp`（及 `/healthz`、`/status`），供支持本地套接字的 MCP 客户端 /
  厂商 Agent 使用（请求 URL `http://localhost/mcp`；Rust 示例见 `crates/hub/tests/mcp_ipc.rs`，用 rmcp 的
  `UnixSocketHttpClient`）。IPC 上**不校验令牌**：连接已确认是同一用户，而同一用户本来就能读取令牌文件；客户端应核对
  监听方是同一用户。不提供 stdio→HTTP 转发程序。

### 单实例与登记文件

- **单实例锁** `<配置目录>/run/hub.lock`：`serve`（以及 stdio 模式）在任何监听之前独占锁定（`flock` / `LockFileEx`，
  随进程退出释放，不会残留）。同一配置目录已有实例时，`serve` 打印其信息（pid、版本、MCP / App 地址、IPC 端点）并以
  **退出码 0** 结束，因此可以放心重复执行（登录脚本、多个终端）；stdio 模式报错并给出该实例的 MCP 地址。
- **登记文件** `<配置目录>/run/endpoints.json`（0600，原子写入，退出时删除）：实际 `listen` 地址、IPC 端点、pid、版本、
  用户、启动时间。`service status|start|stop|uninstall` 以“登记文件存在且其地址上 `/healthz` 的 pid 一致”判断实例在运行。
- **端口被占用**（锁已取得，说明不是同一配置目录的 Host）：显式配置的 `listen` 被占用时报错并说明占用者
  （`/healthz` 是另一个 app-mcp → 给出其 pid 与用户，提示“不同的配置目录或其他用户”；否则“其他程序”），退出码 1；
  缺省地址被占用时依次改用 7737、7757。IPC 端点被占用（另一个配置目录的 Host）同样报错。

## 诊断：`doctor` / `status`

`app-mcp-host doctor`（`--home` 指定配置目录，`--json` 机器可读）只读地逐项检查，每项给出状态（正常 / 信息 / 注意 / 错误 / 跳过）、
结论、修复建议与错误码（spec/protocol.md 第 10 节），有错误时退出码 1：

| 检查 | 内容 |
|---|---|
| Host 运行状态 | 登记文件 + `/healthz` 进程号一致；版本、身份、监听地址 |
| 单实例锁 | `run/hub.lock` 的持有者（Linux 读 `/proc/locks` 按 inode 匹配；其他平台读锁文件中的 pid 并核对进程存活）；**不尝试加锁** |
| 运行时目录 | `run/` 的权限与所有者（Unix 须 0700、属于当前用户） |
| 本地 IPC 端点 | 套接字及其目录的权限 / 所有者；Windows 经管道读 `/status` 时核对管道所有者 SID |
| 监听端口 | 实际地址与 7717 / 7737 / 7757：空闲 / 本 Host / 其他 app-mcp（pid、用户）/ 其他程序（pid 与进程名：Linux `/proc/net/tcp` → `/proc/*/fd`，Windows `GetExtendedTcpTable`，macOS 尽力用 `lsof`） |
| Windows 排除端口段 | 解析 `netsh int ipv4 show excludedportrange protocol=tcp`，候选端口落在其中时提示（显式地址或全部候选落入时为错误；Hyper-V / WSL 常保留端口段） |
| 防火墙 | 说明：只监听回环，回环连接不经入站防火墙规则 |
| 令牌与鉴权 | 令牌策略（browser / all / off）、令牌文件是否存在及权限 |
| App 实例 | 经 IPC（IPC 关闭时 TCP + 令牌）读 `/status`：各 App 在线 / 休眠 / 唤醒中、实例连接 ID 与 pid、最近错误 |
| 工具声明 | 逐个列出每个工具的 `risk` 与 Agent 实际看到的 MCP 注解（`readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint` / `title`），注明是 App 声明的还是按 `risk` 推导的、是否有 `outputSchema`；可据此配置 Agent 的放行规则（按工具全名）。`--json` 的 `details` 按 App 给出 |
| 资源保护 | 限流与大小上限的当前策略、`outputSchema` 核对方式，以及各 App 启动以来被 `RATE_LIMITED` / `PAYLOAD_TOO_LARGE` 拒绝的次数（有拒绝时为注意）；运行中的 Host 版本较旧时跳过 |
| SDK 上报 | SDK 在连接恢复后上报的此前问题（如浏览器拦截 `BLOCKED_*`）。被拦截期间页面无法连接 Host，Host 无从得知（spec/protocol.md 10.2） |
| Android adb reverse | `adb` 在 PATH 中时运行 `adb reverse --list`（5 秒超时），检查设备端 7717 是否转发到本机实际端口 |

`app-mcp-host status`：一行摘要，未运行时说明原因（如 `[PORT_BUSY]` 与占用进程），退出码 3。

`service install` 安装前先检查端口：本配置目录的 Host 已在运行时跳过；否则依次探测候选端口并打印每个占用者（pid 与进程名）——
显式 `listen` 或全部候选被占用时报 `PORT_BUSY` 且不安装，仅默认端口被占用时提示将改用备选端口后继续。`serve` 启动失败的
信息同样带错误码（`LOCK_HELD` / `PORT_BUSY` / `IPC_ENDPOINT_BUSY`）、占用进程与修复建议。

**连接 ID**：Host 为每条 App 连接（多路复用时每个通道）分配 `<启动标记>-<序号>`，日志字段 `cid`；SDK 握手成功后在自己的日志中
记录同一 ID（spec/protocol.md 10.3），`/status` 的实例信息也带 `connectionId`。MCP 会话为 `mcp-<序号>`。

## 配置

配置目录：`--home <DIR>` > 环境变量 `APP_MCP_HOME` > `~/.app-mcp`。其中：

| 文件 | 用途 |
|---|---|
| `config.json` | 常驻模式配置（`serve` 读取；`service install` 写入） |
| `token` | 本地访问令牌（首次需要时生成，Unix 权限 0600） |
| `policy.json` | 策略规则（可选）：`hide` 让 App / 工具对所有 Agent 不可见，`deny` 拒绝调用 / 唤醒（`POLICY_DENIED`）；无规则时默认放行。启动时加载（不合法时拒绝启动），`app-mcp-host policy reload` 重载；格式与语义见 spec/hub-api.md 3.13 |
| `logs/app-mcp-host.log` | 常驻模式日志，按大小轮转（默认 5 MiB × 保留 3 个历史文件）；stderr 仍同时输出 |
| `manifests/*.json` | 静态清单目录（默认） |
| `run/hub.lock`、`run/endpoints.json` | 单实例锁与登记文件（运行时，见上方「单实例与登记文件」） |

`config.json`（所有字段可省略；**命令行参数覆盖配置文件**，列表类参数追加）：

```json
{
  "listen": "127.0.0.1:7717",
  "ipcEndpoint": "unix:/run/user/1000/app-mcp/hub.sock",
  "http": { "allowRemote": false, "auth": "browser" },
  "manifests": ["/path/to/app-mcp.json"],
  "manifestDirs": ["/path/to/manifests"],
  "allowOrigins": ["https://app.example.com"],
  "upstreams": {
    "files": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"], "env": {} }
  },
  "lifecycle": { "leaseMs": 60000, "wakeTimeoutMs": 15000, "wakeFromLaunch": false, "waker": "system" },
  "tools": { "exposure": "auto", "threshold": 40, "outputValidation": "log" },
  "limits": {
    "toolRatePerMinute": 120, "toolRateBurst": 30, "appRatePerMinute": 600, "appRateBurst": 60,
    "maxArgumentsBytes": 1048576, "maxResultBytes": 4194304, "maxResourceBytes": 4194304
  },
  "log": { "level": "info", "file": true, "maxBytes": 5242880, "keep": 3 }
}
```

`lifecycle.waker`：`"system"`（默认，按平台执行系统激活）/ `"none"`（不唤醒：休眠或未运行 App 的调用返回
`APP_DISCONNECTED` 与启动地址）/ `{"exec": ["程序", "参数", …]}`（执行该程序，参数不经 shell，唤醒请求以一行 JSON
写入其 stdin，退出码 0 表示已发出激活）。格式详见 spec/hub-api.md 3.5。

`tools.exposure`（工具渐进暴露，spec/hub-api.md 3.7）：`"auto"`（默认）/ `"progressive"` / `"all"`。
渐进暴露时 `tools/list` 只含 `apps.list` / `apps.select` / `apps.overview` / `apps.tools`，以及本会话调用 `apps.tools`
展开过、直接调用过或 `apps.select` 选定了实例的 App 的工具；模型调用 `apps.tools {appId}` 得到该 App 的工具与参数 schema，
Host 随即只向该 MCP 会话发 `notifications/tools/list_changed`。未列出的工具按全名仍可直接调用。`auto` 在 App 与上游工具总数
超过 `tools.threshold`（默认 40）时渐进，否则全部列出（与旧行为相同）。

`limits`（资源保护，spec/hub-api.md 3.11）：保护 App 与设备，超出时返回明确错误，不静默丢弃、不截断（错误码见 spec/protocol.md 第 4 节）。
上表中的值即默认值，缺省字段取默认，`0` 表示不限；未知字段报错，`*PerMinute > 0` 而对应 `*Burst = 0` 时配置无效、启动失败。

| 字段 | 命令行 | 含义 |
|---|---|---|
| `toolRatePerMinute` / `toolRateBurst` | `--tool-rate-limit` / `--tool-rate-burst` | 每个（App, 工具）的令牌桶：每分钟补充次数 / 最多可攒的突发次数 |
| `appRatePerMinute` / `appRateBurst` | `--app-rate-limit` / `--app-rate-burst` | 每个 App（所有工具合计）的令牌桶 |
| `maxArgumentsBytes` | `--max-arguments-bytes` | 调用参数（JSON）的字节上限 |
| `maxResultBytes` | `--max-result-bytes` | 调用结果（含 `summary`）的字节上限；上游 MCP 服务器的结果同样适用 |
| `maxResourceBytes` | `--max-resource-bytes` | 资源内容的字节上限 |

- 被限流时调用返回 `RATE_LIMITED`（`data.retryAfterMs` 为建议等待毫秒数），不转发、不唤醒 App；参数超限返回 `PAYLOAD_TOO_LARGE`，同样不转发。
- 结果超限返回 `PAYLOAD_TOO_LARGE`（`data.part = "result"`），此时调用**可能已在 App 内执行**，错误信息如实说明。
- 唤醒另有每 App 每分钟上限 `--wake-rate-limit`（默认 6，spec/lifecycle.md 第 12 节）。

`tools.outputValidation` / `--output-validation`：App 结果与其声明的 `outputSchema` 不符时，`"log"`（默认，只记 warn 日志、照常返回）/
`"reject"`（调用以 `HANDLER_ERROR` 结束）/ `"off"`（不校验）。无返回值不校验。

App 声明的工具注解与结果契约（`annotations`、`outputSchema`、结果的 `status` / `summary`，以及无返回值时对模型输出"已完成"）
由 Host 如实传递，不据此拦截或确认调用，见 spec/protocol.md 3.2。

`listen` 缺省为 `127.0.0.1:7717`（被占用时依次尝试 7737、7757）；显式设置时只绑定该地址。`http.addr`（旧的独立 MCP
端口）已弃用：只在兼容期内需要时设置，Host 另开一个同样的监听器。`wsAddr` 是 `listen` 的旧名。

旧的 `--config` 文件（只有 `upstreams`）是它的子集，仍然可用。

命令行（`serve` 与 `service install` 相同）：`--listen <ADDR>`、`--ipc-endpoint <ENDPOINT|none>`、`--http <ADDR>`（已弃用，兼容期的旧 MCP 端口）、`--http-allow-remote`、`--auth browser|all|off`、
`--manifest <file>`（可重复）、`--manifest-dir <dir>`（可重复）、`--allow-origin <pattern>`（可重复）、
`--upstream <name>=<命令行>`（可重复）、`--lease-ms`、`--wake-timeout-ms`、`--wake-from-launch`、
`--waker system|none|'{"exec":[...]}'`、`--tool-exposure auto|progressive|all`、`--tool-exposure-threshold <N>`、
`--tool-rate-limit` / `--tool-rate-burst` / `--app-rate-limit` / `--app-rate-burst` / `--max-arguments-bytes` / `--max-result-bytes` /
`--max-resource-bytes <N>`、`--output-validation off|log|reject`、`--log-level`、
`--no-log-file`、`--config <file>`、`--home <dir>`。`app-mcp-host token` 打印令牌（`--regenerate` 重新生成）。

策略规则：`app-mcp-host policy hide <app> [--tool T]`、`policy deny <app> [--tool T] [--wake]`、`policy remove <id>` 编辑
`policy.json` 并让运行中的 Host 立即重载；`policy show [--json]`（生效规则与命中次数）、`policy validate [FILE]`、`policy reload`。
重载不合法的规则时 Host 保留之前的规则，`doctor` 的「策略规则」检查报出错误。

## 安全

HTTP 端口（`/app`、`/mcp`、`/healthz`）的防护分三层；App 连接（`/app`）的 `Origin` 在 `app/hello` 时按同一允许列表与配对规则处理，令牌只作用于 `/mcp`：

1. **只绑回环**：非回环地址必须显式 `--http-allow-remote`（同时关闭 Host 头校验，不推荐）。
2. **Host / Origin 校验**：`Host` 头必须是回环地址（防 DNS rebinding）；带 `Origin` 头的请求必须在允许列表中
   （默认 `http://localhost:*`、`http://127.0.0.1:*`，与 WebSocket 侧相同），否则 403。
3. **本地访问令牌**（`~/.app-mcp/token`，32 字节随机数，0600）：`Authorization: Bearer <令牌>`，不通过返回 401。

令牌策略 `http.auth` / `--auth`：

| 模式 | 带 `Origin` 的请求（浏览器） | 不带 `Origin` 的请求（本地客户端） |
|---|---|---|
| `browser`（默认） | 必须带令牌 | 可不带；带了就必须正确 |
| `all` | 必须带令牌 | 必须带令牌 |
| `off` | 不校验令牌 | 不校验令牌 |

**决策与理由**：

- **浏览器强制**：浏览器是本机上唯一会替“任意网站”向 `127.0.0.1` 发请求的程序。跨站页面已被 Origin 允许列表挡住，
  但允许列表包含 `http://localhost:*`——本机任何开发服务器、被入侵的本地页面都在其中。令牌只存放在用户目录的 0600 文件里，
  网页无法读取，因此要求浏览器来源携带令牌可以挡住“同源于 localhost 的恶意页面”。浏览器对 POST（含 `fetch`、表单）总是发送
  `Origin`，所以这条规则覆盖所有能修改状态的浏览器请求。
- **本地客户端默认不强制**：不带 `Origin` 的请求来自本机进程。同一用户的进程本来就能读取 `~/.app-mcp/token`，
  强制令牌对它们没有额外防护，却会让每个 MCP 客户端都要配置密钥。
- **多用户机器用 `all`**：回环端口对本机**其他用户**也可见，而他们读不到你的令牌文件。共享机器、远程桌面服务器上应设
  `"auth": "all"`，客户端配置 `Authorization: Bearer <令牌>`（Claude Code 的 `.mcp.json` 可写
  `"headers": {"Authorization": "Bearer ${APP_MCP_TOKEN:-}"}`，令牌放环境变量，不入库）。
- 携带了错误的令牌一律拒绝（即使该请求本可不带），避免配置错误被静默忽略；空令牌（`Bearer ` 后为空，环境变量未设置时）视为未携带。
- `/healthz` 不需要令牌（只返回服务名、版本、用户、pid、监听地址与 IPC 端点），供 `service status` 与端口占用诊断使用；仍受 Origin 校验。
- `/status`（实例、最近错误、SDK 上报）经 IPC 直接可读；经 TCP 必须带有效令牌，未配置令牌（`--auth off`）时 TCP 上一律 403。
- 令牌比较为常量时间；`app-mcp-host token --regenerate` 轮换令牌（运行中的实例需重启）。

## stdio 模式（兼容 / 测试）

```bash
app-mcp-host stdio --manifest ./app-mcp.json    # 等同于旧用法：app-mcp-host [--stdio] [参数]
```

单个 MCP 客户端以子进程方式启动 Host，stdout 专用于 MCP。适用于测试、一次性脚本、无法安装服务的环境；
**不再推荐**用于日常接入：stdio Host 同样取得单实例锁，同一配置目录已有 Host（常驻或另一个 stdio）时报错并给出其 MCP 地址。
stdio 模式只在显式 `--config` 时读取配置文件，不写日志文件、不使用令牌；`listen` 上只有 `/app` 与 `/healthz`（MCP 走 stdio）。
旧用法的 `--http <ADDR>` 仍可用（另开一个带 `/mcp` 的监听器，不校验令牌），请改用 `serve`。

## 能力来源

| 来源 | 工具名 | 资源 URI | `apps.list` 中的 `kind` |
|---|---|---|---|
| SDK 连接的 App（运行时） | `<appId>.<tool>` | `app-mcp://<appId>/<name>` | `app` |
| 静态清单（App 未运行） | `<appId>.<tool>` | `app-mcp://<appId>/<name>` | `app` |
| 上游 MCP 服务器 | `<name>.<tool>` | `app-mcp://<name>/<百分号编码的上游 URI>` | `upstream` |

上游名称须满足 appId 规则，且不能与清单 appId、保留名（`apps`、`os`、`ax`、`host`）冲突；
SDK 也不能以上游名称握手。上游进程退出后标记为未连接，并按指数退避（500ms 起，最大 30s）重启。
上游 `initialize` 结果中的 `instructions` 作为其总览：前 100 个字符为简介，其余（≤ 2000）为正文。

## 备忘：注册为 Windows 智能体连接器（未实现）

> 2026-09-30 已对照微软 Learn 文档核实（预发布内容，可能变化）：
> [MCP on Windows 概述](https://learn.microsoft.com/en-us/windows/ai/mcp/overview)、
> [注册 MCP 服务器](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-server-overview)、
> [包身份注册](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-windows-identity)、
> [隔离](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-containment)、
> [MCP bundle](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-mcpb)、
> [手动注册](https://learn.microsoft.com/en-us/windows/ai/mcp/servers/mcp-manual)、
> [odr.exe](https://learn.microsoft.com/en-us/windows/ai/mcp/odr-tool)。完整分析见 `app-mcp-plan.md` 8.6。

Windows（build 26220.7262 起，公开预览）提供 **Windows On-device Agent Registry（ODR）**：
登记在其中的 MCP 服务器称为 **agent connector（智能体连接器）**，由宿主（VS / VS Code 的 GitHub Copilot agent mode、
Microsoft Agent Framework 等）通过 `odr.exe list` 发现，并按其中的启动命令以 stdio 连接。
Host 本身就是一个 MCP 服务器，但**不能原样登记**，原因见第 2、3 条：

1. **打包**：有包身份的应用在包清单中声明 `uap3:AppExtension Name="com.microsoft.windows.ai.mcpServer"`，
   其 `Registration` 指向一份 **MCP bundle（MCPB）`manifest.json`**，安装 / 卸载时自动注册 / 注销；
   开发阶段可用 `odr mcp add <manifest.json>` 手动注册，远程服务器用 `odr mcp add --uri <url>`
   （另有 `odr mcp list / remove / configure / run`）。无包身份的 `.mcpb` 包默认**不会**出现在 ODR 中，
   除非用户在"设置 > 系统 > 高级 > AI 组件"打开 *Reduce protections for agent connectors*（仅供测试）。
2. **静态工具集合**：MCPB 清单的 `_meta["com.microsoft.windows"].static_responses` 必须写出 `initialize` 与 `tools/list`
   的完整响应，且与运行时完全一致；服务器**不得动态改变**工具集合、描述或 schema，否则不能以默认（隔离）模式运行。
   Host 的工具列表是动态的，因此只能登记一个工具集合固定的**网关**（如 `apps.list` / `apps.overview` / `apps.tools` / `apps.call`）。
3. **隔离**：连接器默认在**独立的 Windows 会话、独立的智能体用户账户**中运行，不能直接访问用户会话中的文件、
   设置 / 注册表 / 凭据、用户正在使用的应用与窗口，也不能运行修改用户会话的程序（可访问网络）。
   因此登记的网关只能转发到用户会话中常驻的 Host（`serve`，plan 第 8.4 节），由后者负责 App 连接与唤醒；
   跨会话的本地通道（回环 TCP、命名管道）是否可用，文档未说明，需实测。`--uri` 远程注册 `--http` 端点是另一条候选路径，同样待实测。
4. **反向接入**：ODR 中其他 App 的连接器本身也是 MCP 服务器，可从 `odr.exe list` 的 JSON 读取启动命令，作为 `--upstream`
   聚合进来（本 crate 已支持 stdio 上游）。
5. **安全**：ODR 的授权由系统负责——用户 / IT 管理员可在"设置"与 Intune 中按智能体控制访问；用户文件等资源需在包清单中声明能力，
   使用时提示用户授权，且授权按**宿主**而非按服务器生效。Host 自己的风险确认策略（按工具 `risk`）仍需保留，
   总览文字不产生任何授权效果（spec/protocol.md 第 7.4 节）。

另：**App Actions on Windows** 是另一套框架（动作 JSON + 包清单，URI 激活或 COM `IActionProvider`，输入输出为实体），不是 MCP；
**Agent Launchers** 基于 App Actions 在 ODR 中登记可对话的智能体。二者由 `app-mcp-codegen` 负责，不在 Host 中实现。
