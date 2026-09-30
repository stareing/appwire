# app-mcp-host

本地 MCP Host：把本机各 App（通过 `@app-mcp/web` 等 SDK 连接）以及已有的 MCP 服务器聚合起来，
以一个 MCP Server 暴露给模型。协议与行为见 `spec/protocol.md`、`spec/manifest.md`。

本 crate 是命令行程序（`src/lib.rs`：命令行、配置、单实例探测、日志轮转、服务安装；`src/main.rs` 与
Windows 无窗口版 `src/bin/app-mcp-hostw.rs` 只是入口）与集成测试；Hub 的全部实现位于 `crates/hub`（`app-mcp-hub`，
可嵌入厂商自有 Agent，见 `crates/hub/README.md` 与 `spec/hub-api.md`）。

## 推荐接入：常驻服务 + HTTP

一个常驻的 `app-mcp-host serve` 进程同时提供：

- **App 连接服务**（WebSocket，默认 `ws://127.0.0.1:7717`）：各 App 的 SDK 连到这里；
- **MCP Streamable HTTP**（默认 `http://127.0.0.1:7718/mcp`）：每个 MCP 客户端（Claude Code、Claude Desktop、IDE……）
  各自建立一个 HTTP 会话，**共享同一组 App 连接**；每个会话有独立的 `apps.select` 选择与“首次接触附带总览”状态。

这样解决了 stdio 模式的根本问题：stdio 下每个 MCP 客户端各起一个 Host 进程，而 App 端口只能被一个进程占用。

```bash
# 安装为当前用户的登录自启服务（无需管理员）并立即启动；给出的参数写入 ~/.app-mcp/config.json
app-mcp-host service install --manifest ./examples/shop/app-mcp.json
app-mcp-host service status        # 安装与运行状态；未运行时退出码 3
app-mcp-host service stop | start
app-mcp-host service uninstall     # 停止并删除服务文件

# 或临时在前台运行（Ctrl+C 退出）；已有健康实例在运行时打印其信息并以退出码 0 退出
app-mcp-host serve
```

客户端配置（Claude Code：`claude mcp add --transport http app-mcp http://127.0.0.1:7718/mcp`）：

```json
{ "mcpServers": { "app-mcp": { "type": "http", "url": "http://127.0.0.1:7718/mcp" } } }
```

### 各平台的服务形式

| 平台 | 形式 | 位置 | 说明 |
|---|---|---|---|
| Linux | systemd 用户单元 | `~/.config/systemd/user/app-mcp-host.service` | `systemctl --user enable` + `restart`；`Restart=on-failure`；日志也进 `journalctl --user -u app-mcp-host`。没有 systemd 用户实例时报错并提示改用登录脚本运行 `serve` |
| macOS | launchd LaunchAgent | `~/Library/LaunchAgents/dev.app-mcp.host.plist` | `launchctl bootstrap gui/<uid>`；`RunAtLoad`，`KeepAlive.SuccessfulExit=false`（异常退出才重启） |
| Windows | 当前用户登录启动项 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 值 `app-mcp-host` | 指向同目录的 **`app-mcp-hostw.exe`**（GUI 子系统，同一份代码，不创建控制台窗口）；`service start` 以 `DETACHED_PROCESS \| CREATE_NO_WINDOW` 启动；`service stop` 按 `/healthz` 返回的 pid 结束进程 |

服务文件内容由代码生成，启动命令统一为 `<可执行文件绝对路径> serve --home <配置目录>`，其余设置都在配置文件中。
选 `Run` 项而不是计划任务：两者都无需管理员，但计划任务运行控制台程序会弹窗，且 `Run` 项最简单、可在“任务管理器 > 启动”中查看和禁用。
Hub 在 Windows 上启动的子进程（唤醒命令、上游 MCP 服务器）一律带 `CREATE_NO_WINDOW`，常驻进程不会间接弹出控制台。

### 单实例

`serve` 绑定端口失败（`AddrInUse`）时探测 `GET http://<http 地址>/healthz`：
返回 `{"service":"app-mcp", "version", "pid", "wsAddr", ...}` 则说明已有健康实例，打印信息后**退出码 0**；
端口被其他程序占用则报错（退出码 1）。因此 `serve` 可以放心重复执行（登录脚本、多个终端）。

## 配置

配置目录：`--home <DIR>` > 环境变量 `APP_MCP_HOME` > `~/.app-mcp`。其中：

| 文件 | 用途 |
|---|---|
| `config.json` | 常驻模式配置（`serve` 读取；`service install` 写入） |
| `token` | 本地访问令牌（首次需要时生成，Unix 权限 0600） |
| `logs/app-mcp-host.log` | 常驻模式日志，按大小轮转（默认 5 MiB × 保留 3 个历史文件）；stderr 仍同时输出 |
| `manifests/*.json` | 静态清单目录（默认） |

`config.json`（所有字段可省略；**命令行参数覆盖配置文件**，列表类参数追加）：

```json
{
  "wsAddr": "127.0.0.1:7717",
  "http": { "addr": "127.0.0.1:7718", "allowRemote": false, "auth": "browser" },
  "manifests": ["/path/to/app-mcp.json"],
  "manifestDirs": ["/path/to/manifests"],
  "allowOrigins": ["https://app.example.com"],
  "upstreams": {
    "files": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"], "env": {} }
  },
  "lifecycle": { "leaseMs": 60000, "wakeTimeoutMs": 15000, "wakeFromLaunch": false, "waker": "system" },
  "log": { "level": "info", "file": true, "maxBytes": 5242880, "keep": 3 }
}
```

`lifecycle.waker`：`"system"`（默认，按平台执行系统激活）/ `"none"`（不唤醒：休眠或未运行 App 的调用返回
`APP_DISCONNECTED` 与启动地址）/ `{"exec": ["程序", "参数", …]}`（执行该程序，参数不经 shell，唤醒请求以一行 JSON
写入其 stdin，退出码 0 表示已发出激活）。格式详见 spec/hub-api.md 3.5。

旧的 `--config` 文件（只有 `upstreams`）是它的子集，仍然可用。

命令行（`serve` 与 `service install` 相同）：`--ws-addr`、`--http`、`--http-allow-remote`、`--auth browser|all|off`、
`--manifest <file>`（可重复）、`--manifest-dir <dir>`（可重复）、`--allow-origin <pattern>`（可重复）、
`--upstream <name>=<命令行>`（可重复）、`--lease-ms`、`--wake-timeout-ms`、`--wake-from-launch`、
`--waker system|none|'{"exec":[...]}'`、`--log-level`、
`--no-log-file`、`--config <file>`、`--home <dir>`。`app-mcp-host token` 打印令牌（`--regenerate` 重新生成）。

## 安全

HTTP 端点的防护分三层：

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
- `/healthz` 不需要令牌（只返回服务名、版本、pid、WebSocket 地址），供单实例探测与 `service status` 使用；仍受 Origin 校验。
- 令牌比较为常量时间；`app-mcp-host token --regenerate` 轮换令牌（运行中的实例需重启）。

## stdio 模式（兼容 / 测试）

```bash
app-mcp-host stdio --manifest ./app-mcp.json    # 等同于旧用法：app-mcp-host [--stdio] [参数]
```

单个 MCP 客户端以子进程方式启动 Host，stdout 专用于 MCP。适用于测试、一次性脚本、无法安装服务的环境；
**不再推荐**用于日常接入：多个客户端各起一个 stdio Host 时，只有第一个能占用 App 连接端口。
stdio 模式只在显式 `--config` 时读取配置文件，不写日志文件、不使用令牌。旧用法的 `--http` 仍可用（不校验令牌），请改用 `serve`。

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
