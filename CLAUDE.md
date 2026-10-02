# app-mcp 仓库指南

把 App 内的业务动作以 MCP 工具暴露给模型。理念「万物皆工具」见 `README.md`（英文为主，其他语言在 `docs/README.<lang>.md`：zh-CN、zh-TW、ja、ko、es、pt-BR、fr、de、ru、it；修改理念或概览时各语言版本同步）；总体设计见 `app-mcp-plan.md`，当前阶段为 **M1**（第 15 节）。

对外品牌名为 **AppWire**（仓库 `github.com/stareing/appwire`）；包名、crate 名、二进制与协议标识符仍沿用工作名 `app-mcp`。
Logo 与社交卡片在 `docs/assets/`（`logo/appwire-mark.svg` 为主图形，各 README 页头用 `appwire-logo-{light,dark}.png`）。
面向用户的文档与包描述（npm / crates.io / PyPI / NuGet / pub.dev）用英文、以 AppWire 称呼项目，便于检索；`llms.txt` 与 README 首段保持一致。

## 产品理念

理念的权威表述在 `README.md`「Philosophy」（九条原则），这里只列开发时必须对照的两条：

- **万物皆工具**：按钮、表单、菜单命令、状态库动作、系统能力、已有 MCP 服务器，都以"名称 + 输入 schema + 声明 + handler"表达；模型只需要 list / call / read。
- **召之即来，挥之即去**（原则 4）：目标是"永远可调用"，而不是"永远在运行"。
  - 发现不启动 App：工具从清单与休眠快照列出，`apps.list` / `tools/list` 不得触发唤醒。
  - 调用才激活：经各平台原生激活（广播 / bindService、D-Bus、launchd / XPC、协议 / AUMID、URL）唤醒，办完即释放连接与线程，进程交还系统。
  - 不强占驻留：不加前台服务、WakeLock、后台任务断言、每 App 常驻守护进程，不用轮询；进程生命周期归操作系统，本库不与之对抗。
  - 保温只是优化：租约与合并窗口可以缩短下一次调用，但正确性不依赖进程存活（进程随时可能被系统回收）。
  - 新增功能先问：空闲时它会不会让 App 或 Host 多一个定时器、连接、线程或常驻内存？会的话需要说明理由，并有功耗 / 内存测量支撑。

## 目录结构（M1）

```
Cargo.toml              Rust workspace
package.json            pnpm workspace（packages/*、examples/*、e2e）
spec/
  protocol.md           SDK ↔ Host 协议规范（权威）
  manifest.md           静态清单 app-mcp.json 规范（权威）
crates/
  protocol/             协议类型：JSON-RPC 信封、消息、错误码（Host 与 SDK 共用）
  core/                 sans-IO 客户端核心（状态机，无 I/O）
  manifest/             清单类型、解析、校验、目录加载
  hub/                  Agent 端 Hub 库（厂商嵌入）：注册表、路由、总览、App 连接、上游聚合、MCP 出口、格式导出
                        （cargo features mcp-server / upstream / schema-validation，默认全开；移动端精简见 spec/hub-api.md 3.10）
  host/                 app-mcp-host 可执行程序（Hub 之上的命令行薄壳）
  native/               原生共用运行时：后台线程驱动核心、WebSocket 连接、回调分发（Rust App 直接用）
  tauri-plugin/         tauri-plugin-app-mcp：Tauri v2 插件（独立 workspace，Linux 需 webkit2gtk-4.1）
bindings/
  wasm/                 app-mcp-core 的 wasm-bindgen 绑定
  c/                    C ABI（include/app_mcp.h 为契约）→ C、C++、C#、Dart
  uniffi/               uniffi 绑定 → Kotlin、Swift、Python
  node/                 napi-rs 绑定 → @app-mcp/node
  hub-c/                Hub SDK 的 C ABI（include/app_mcp_hub.h）
  hub-uniffi/           Hub SDK 的 uniffi 绑定 → Kotlin、Swift、Python
  hub-node/             Hub SDK 的 napi-rs 绑定 → @app-mcp/hub
  harmony/              鸿蒙 Node-API 模块（napi-ohos 编译 bindings/node 同一份源码）→ libapp_mcp_harmony.so；
                        独立 workspace（不在根 workspace），产物仍在根 target/
packages/
  web/                  @app-mcp/web：浏览器 SDK（WASM 核心 + JS 驱动层 + WebSocket）
  react/                @app-mcp/react：useTool / useResource / ToolScope
  build/                @app-mcp/build：Vite 插件，生成 app-mcp.json
  node/                 @app-mcp/node：Node / Electron 主进程 SDK
  electron/             @app-mcp/electron：主进程接入 + 渲染进程 IPC 桥接
  tauri/                @app-mcp/tauri：Tauri 页面侧（插件注入的桥接；页面也可直接用 @app-mcp/web）
sdks/
  cpp/                  C++ 封装（RAII）+ 示例
  dotnet/               C#（P/Invoke）SDK + 示例
  kotlin/               Kotlin（JVM / Android）SDK
  python/               Python SDK
  swift/                Swift SDK
  dart/                 Dart（dart:ffi）SDK 与 Flutter 适配
  harmony/              鸿蒙 ArkTS SDK（ohpm HAR 包 @app-mcp/harmony：UIAbility 前后台、Want 唤醒）
examples/
  shop/                 React Demo：待办 + 购物车
  tauri/                Tauri 示例：页面工具 + Rust 工具（src-tauri 为独立 workspace）
e2e/                    端到端测试（集成阶段）
```

语言与绑定的对应关系：

| 语言 | 绑定路径 | 线程切换（封装层负责） |
|---|---|---|
| Rust（Tauri、egui） | 直接依赖 `crates/native` | — |
| Tauri v2 | `crates/tauri-plugin`（基于 `crates/native`）；页面用 `@app-mcp/web`，经插件注入的桥接走 Tauri IPC | Rust handler 在分发线程；页面 handler 在 WebView 内 |
| C / C++ | `bindings/c` | 调用方自行处理 |
| C# | `bindings/c` + P/Invoke | `SynchronizationContext`（WPF Dispatcher、WinUI DispatcherQueue） |
| Dart / Flutter | `bindings/c` + dart:ffi | 回到主 isolate |
| Kotlin | `bindings/uniffi` | `Dispatchers.Main`（Android）/ 可配置 |
| Swift | `bindings/uniffi` | `@MainActor` |
| Python | `bindings/uniffi` | 可配置（Qt 信号、Tk `after`） |
| Node / Electron | `bindings/node` | Node 事件循环（threadsafe function） |
| ArkTS（HarmonyOS NEXT） | `bindings/harmony` | 创建客户端的 ArkTS 线程（threadsafe function；在 UIAbility / AbilityStage 创建即主线程） |

## 微内核范围

AppWire 定位为 Agent 的系统调用层（模型 = 用户态程序，App 工具 = 系统调用，Hub = 内核），按微内核取舍：**内核只提供机制，策略在 Agent 与 App**。
新增能力前先对照本节；落在"不做"一列的需求放到 Agent / App / 外部工具，或在报告中说明理由后由机主决定。分析与任务拆分见 `docs/plans/16-agent-os.md`、`docs/plans/14-safety.md`。

| 做（机制） | 不做（策略 / 用户态） |
|---|---|
| 如实传递 App 声明（工具、schema、MCP 注解、内容标注） | 风险分级判断、要不要确认、事先授权、无人值守策略（归 Agent；高风险最终确认归 App） |
| 路由、发现、按名寻址、唤醒与生命周期 | 选哪个模型、提示词编排、Agent 的长期记忆与知识库 |
| Agent 任务对象及其名下资源（句柄、锁、租约、订阅）的所有权与回收 | Agent 之间直接对话协作（A2A）；Agent 间只经句柄与事件间接协作 |
| 资源保护：限流、唤醒上限、大小上限、按 Agent 记账与配额 | 据内容推断数据敏感度、外发拦截等信息流判定 |
| 执行点：策略挂点（`hide` / `deny`），规则由用户 / 厂商写，无规则时默认放行（见 16 P2） | 内置任何安全策略或启发式 |
| 事件投递、进度、取消、幂等、作业控制 | 代 Agent 发起调用（触发器只投递事件） |
| 自省：调用日志、`status` / `doctor`、Hub 状态作为只读资源 | 截图 + 视觉 / 坐标点击（2026-10-02 决定不做） |

## 契约文件（修改需谨慎）

以下文件是并行开发的接口契约。实现方可以新增内部代码，但**不要修改已有的公开签名或字段**；
确需修改时，在最终报告中写明修改内容与原因。

| 文件 | 契约内容 |
|---|---|
| `spec/protocol.md`、`crates/protocol/` | SDK ↔ Host 消息格式与行为 |
| `spec/manifest.md` | 清单格式 |
| `spec/hub-api.md` | Hub SDK（Agent 端）API 与各语言绑定 |
| `crates/core/src/lib.rs` 中的公开 API | 核心与各语言绑定之间的接口 |
| `packages/web/src/types.ts` | @app-mcp/web 与上层包之间的接口 |
| `crates/native/src/lib.rs` 中的公开 API | 原生运行时与各原生绑定之间的接口 |
| `bindings/c/include/app_mcp.h` | C ABI，C / C++ / C# / Dart 共用 |

## 约定

- 标识符用英文；注释、文档、面向用户的错误信息用中文（与现有代码一致）。
- Rust：edition 2024；不使用 `unwrap()` 处理外部输入；库代码不 panic；`cargo clippy` 无警告。
- TypeScript：`strict`；ESM；不引入未在 package.json 中声明的依赖。
- 每个模块都要有测试。Rust 用内置测试；TS 用 vitest。
- 日志：Host 的 stdout 专用于 MCP 协议，所有日志写 stderr。
- 测试里启动 Hub / Host 时不要占用默认 IPC 端点（常驻 Host 可能正在用）：设 `ipc_endpoint: None`（各绑定 `ipcEndpoint: null` / `enable_ipc = false` / `DisableIpc`）或临时路径。
- 文件行数上限：源码 800 行、测试 1200 行（`pnpm check:size`，即 `scripts/check-file-size.mjs`；提交前钩子 `.githooks/pre-commit`，
  启用：`git config core.hooksPath .githooks`）。已超限的文件记在 `scripts/file-size-baseline.json`，只许变小不许变大：
  往这些文件里加功能时，新代码放进新的子模块（Rust 用 `foo.rs` + `foo/` 目录按职责拆分），顺手拆小后运行 `--update` 收紧基线。
- 测试里的 TCP 监听一律绑定端口 0，从监听器（`Hub::listen_addr()`）或登记文件（`<home>/run/endpoints.json`）取实际地址；
  不要"先绑定 0 取端口再释放"（释放后可能被其他进程占用）。启动 `app-mcp-host` 进程的测试用临时 `--home`（锁与登记文件在其中）。

## 常用命令

```bash
# Rust：编译产物统一放在仓库根目录 target/（cargo 默认；不要设置 CARGO_TARGET_DIR 指到 ~/.cache，根分区空间紧张）
cargo test -p app-mcp-protocol
cargo test -p app-mcp-core
cargo test -p app-mcp-host
cargo clippy --workspace --all-targets

# WASM（wasm-bindgen CLI 在 ~/.cargo/bin，版本 0.2.129，与 Cargo.toml 固定版本一致）
pnpm --filter @app-mcp/web build:wasm

# 鸿蒙（OpenHarmony SDK 6.0 / API 20 装在 ~/sdk/ohos/sdk，见 sdks/harmony/README.md）
OHOS_NDK_HOME=~/sdk/ohos/sdk sdks/harmony/scripts/build-native.sh arm64-v8a x86_64
node sdks/harmony/scripts/arkts-check.cjs        # ArkTS 类型检查 + ArkTSLinter
node sdks/harmony/tests/run.cjs                  # 封装层单元测试（假原生模块，Node 上运行）

# JS
pnpm -r test
pnpm -r typecheck
pnpm --filter @app-mcp/example-shop dev
```

## 在 Claude Code 中使用

仓库根 `.mcp.json` 以 HTTP 连接常驻 Host：`http://127.0.0.1:7717/mcp`（同一端口的 `/app` 是网页 App 的 WebSocket 连接、`/healthz` 是健康检查；原生 App 默认走本地 IPC：Linux `$XDG_RUNTIME_DIR/app-mcp/hub.sock` / Windows 命名管道，见 `spec/protocol.md` 第 1 节）。单实例锁 `~/.app-mcp/run/hub.lock`，实际监听位置在 `~/.app-mcp/run/endpoints.json`（默认端口被占用时改用 7737 / 7757）。先让 Host 跑起来（二选一）：

```bash
cargo build -p app-mcp-host
# 登录自启（当前用户服务，写入 ~/.app-mcp/config.json）；卸载：service uninstall
target/debug/app-mcp-host service install --manifest examples/shop/app-mcp.json
# 或临时前台运行（同一配置目录已有实例在运行时打印其信息并以退出码 0 退出）
target/debug/app-mcp-host serve --manifest examples/shop/app-mcp.json
```

启动 Demo：`pnpm --filter @app-mcp/example-shop dev`，浏览器打开后工具即出现。多个 Claude Code 会话共享同一个 Host。
排查连接问题：`target/debug/app-mcp-host status`（一行摘要）、`target/debug/app-mcp-host doctor`（`--json`；Host / 锁 / IPC 权限 / 端口占用进程 / Windows 排除端口段 / 令牌 / adb reverse / 各 App 状态与最近错误）；错误码表见 `spec/protocol.md` 第 10 节，Host 与 SDK 日志中的连接 ID（`cid`）可对照。
令牌策略为 `--auth all` 时，把令牌放环境变量：`export APP_MCP_TOKEN=$(app-mcp-host token)`（`.mcp.json` 用 `${APP_MCP_TOKEN:-}`）。

## 并行开发注意

- 不要在仓库根目录运行 `pnpm install` / `pnpm add`；依赖已预先安装。缺少依赖时在报告中说明。
- 不要修改根 `Cargo.toml` 的 workspace 成员；可以在自己负责的 crate 的 Cargo.toml 中增加依赖（在报告中列出）。
- 需要安装额外工具链（Gradle、Swift、Dart 等）时，只安装到用户目录（`~/.local`、`~/.cache`），不使用 sudo。
- 不要提交 git。
- 只修改自己负责的目录（见任务说明）。
