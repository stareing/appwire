# 13 开箱即用：从安装到工具出现（计划）

> 状态：计划（2026-10-02）。行为契约分别以 `spec/protocol.md`、`spec/hub-api.md`、`spec/naming.md` 为准，本文件只负责分析与任务拆分；
> 实施结果写入 `TASKS.md` 第 13 项。

## 0. 目标与验收

- **最终用户**：全新机器上，从零到工具出现在 Claude Code 中 **≤ 2 条命令、≤ 2 分钟**，不手写配置文件、不设环境变量。
- **App 开发者**：接入 = 加依赖 + 一行启动代码；端点、传输、生命周期模式、清单全部有平台默认值。
- **Agent 厂商**：嵌入 Hub 后，已安装但未打开过的 App 也能被发现并按名拉起（依赖 4d）。
- **出错可自愈**：每个连接错误码附一条可直接执行的修复命令，SDK 日志与 `doctor` 输出同一条。

## 1. 已知的已知（代码可证的事实）

| # | 事实 | 证据 |
|---|---|---|
| K1 | Host 只能从源码编译（`cargo build -p app-mcp-host`），仓库无发布流水线（无 `.github/workflows`） | `CLAUDE.md`「在 Claude Code 中使用」；仓库目录 |
| K2 | 自启服务已有：`service install/uninstall/status/start/stop`（systemd --user / launchd / Windows 登录启动项），参数写 `<home>/config.json` | `crates/host/src/cli.rs` `ServiceAction` |
| K3 | 令牌默认策略 `browser`：只有带 `Origin` 的请求需要令牌，本机非浏览器 MCP 客户端可不带；`all` 才需 `APP_MCP_TOKEN` | `crates/host/src/config.rs` `AuthMode` |
| K4 | 原生 SDK 缺省端点 = `APP_MCP_ENDPOINT` → 平台 IPC → ws，已实测 Python App 零配置经 IPC 配对 | `TASKS.md` 第 3 项结果 |
| K5 | 网页 SDK 依次尝试 7717 / 7737 / 7757；Chrome 本地网络访问拦截已能检测并提示 | `TASKS.md` 第 4、4b 项 |
| K6 | 清单 `app-mcp.json` 只有 Vite 插件（`@app-mcp/build`）能生成 | `packages/build` |
| K7 | `doctor` 已逐项检查并给修复建议（含 adb reverse、端口占用、Windows 排除端口段） | `cli.rs` `Command::Doctor` |
| K8 | 手机端经 `adb reverse 7717` 连 PC 上的 Host，仅适用于开发调试 | memory `android-test-device` |
| K9 | stdio 转发形态已在第 1 项删除；`stdio` 子命令仅用于测试 / 无法装服务的环境 | `TASKS.md` 第 1 项；`cli.rs` `Command::Stdio` |
| K10 | 4d 已规划"App 首次打开主动汇报"（只首次，内容变化才重报）与按名寻址 `appmcp://<appId>` | `spec/naming.md`、`TASKS.md` 4d |

## 2. 已知的未知（需验证）

| # | 未知 | 处理 |
|---|---|---|
| U1 | 各 Agent 的 MCP 配置写入方式与位置（Claude Code `claude mcp add` 的作用域与参数、Cursor、VS Code、Windsurf、Codex 等） | **验证**：逐个以本机安装版本的 `--help` / 官方文档为准，写入前核对；不认识的版本只打印手动配置说明，不写文件 |
| U2 | 发布渠道账号与签名：npm、PyPI、crates.io、Homebrew tap、winget；Windows 代码签名（SmartScreen）、macOS 公证 | **待确认**（需机主提供账号 / 证书）；无签名时先发 npm / PyPI 包装的预编译二进制，并在文档注明 |
| U3 | npm / PyPI 分发原生二进制的方式（按平台的可选依赖包 vs 首次运行下载） | **保守**：按平台可选依赖包（不在运行时联网下载）；以本仓库 napi-rs 已用的发布方式为参照 |
| U4 | 网页 App 连接 `/app` 是否也受 `browser` 令牌策略约束、首次授权能否免令牌 | **验证**：读 `crates/hub/src/app_server.rs` 鉴权路径并补测试 |
| U5 | macOS launchd 自启与 IPC（`getpeereid`）路径 | 无环境，**记为待验证** |
| U6 | 写入 Agent 配置时与用户已有条目冲突（同名 `app-mcp`、不同 URL） | **显式失败**：已存在且不同 → 提示并要求 `--force`，不静默覆盖 |

## 3. 未知的已知（已有、可直接复用）

- `service install` 已能注册自启并持久化参数 → `setup` 只需编排，不新写服务逻辑。
- `serve` 单实例锁：已有实例时打印信息并退出码 0 → `setup` 可重复执行（幂等）。
- `endpoints.json` 登记实际端口 → 写 Agent 配置时取实际地址，不假设 7717。
- `doctor --json` 机器可读 → `setup` 结尾自检直接复用。
- `@app-mcp/hub`、`@app-mcp/node` 已有 napi-rs 预编译分发结构 → Host 二进制分发可沿用同一套平台包布局。

## 4. 未知的未知（限制手段）

- `setup` 的每一步可单独执行、可回滚（`appwire uninstall` 撤销服务与写入的 Agent 配置条目，只删自己写的）。
- 写 Agent 配置前备份原文件；写入后用该 Agent 自己的命令回读校验（如 `claude mcp list`），失败即回滚。
- 端到端冒烟测试：临时 `--home` + 临时 Agent 配置目录，跑完整 `setup → 工具出现 → uninstall`，纳入 CI。

## 5. 方案与任务

### 第一部分：Host 分发与一条命令安装（可与 4e 并行）

- **D1 发布流水线**：GitHub Actions 构建 Linux（x64 / arm64）、Windows（x64 / arm64）、macOS（x64 / arm64）的 `app-mcp-host`；产物进 Release。
- **D2 包管理器入口**：`npx appwire` / `uvx appwire`（平台可选依赖包内含二进制，命令名 `appwire`，二进制仍为 `app-mcp-host`）；Homebrew、winget 在 U2 确认后加。
- **D3 `setup` 子命令**：注册自启（复用 K2）→ 等 `/healthz` → 检测已装 Agent 并写入 MCP 配置（U1、U6）→ `doctor` 自检 → 打印结果；幂等。
  对应 `uninstall`（撤销服务 + 只删自己写入的条目）。
- **D4 默认免令牌**：保持 `browser` 策略（K3），`setup` 不生成需要环境变量的配置；README 各语言版本把"快速开始"改为 2 条命令。

### 第二部分：App 一行接入（4e 第二部分之后）

- **E1 统一启动入口**：各 SDK `start(appId)` 一行；端点、传输、生命周期模式（移动 / 托盘 `on-demand`，桌面 / 网页 `idle`）按平台默认。
- **E2 清单生成扩展**：Gradle、SwiftPM、Cargo（build.rs）各一个生成步骤，与 `@app-mcp/build` 共用同一校验（`crates/manifest`）。
- **E3 首次打开登记**：随 4d 的"首次打开汇报"实现，用户不手动添加 App。

### 第三部分：移动端与自愈（4d 之后）

- **F1 手机端形态**：Agent 厂商嵌入 Hub（`mobile` 精简），App 间走 Binder / `bindService` 按名寻址（4d）；`adb reverse` 仅开发用，由 `setup --android` 自动执行。
- **F2 错误码附修复命令**：`spec/protocol.md` 第 10 节每个连接错误码增加"修复命令"列，SDK 日志与 `doctor` 共用同一张表（单一定义）。

## 6. 验证

1. 全新 Linux / Windows 用户目录（临时 `HOME` / 新建 Windows 用户）：`npx appwire setup` 后 Claude Code 中出现 `apps.*` 工具，计时 ≤ 2 分钟。
2. 重复执行 `setup` 无副作用；`uninstall` 后 Agent 配置恢复原样（与备份逐字节比对）。
3. 示例 App（Python、网页 shop、Android 示例）只保留一行启动代码即可配对。
4. macOS 记为待验证（U5）。
