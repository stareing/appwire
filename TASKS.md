# app-mcp 任务列表

更新：2026-09-30。设计见 `app-mcp-plan.md`，仓库约定见 `CLAUDE.md`，协议见 `spec/`。

## 全量验证（2026-09-30）

WSL2 本机，`CARGO_TARGET_DIR=~/.cache/tastyrice/target-hub`。全部通过，**无需修改代码**。

| # | 命令 | 结果 | 测试数 | 耗时 |
|---|---|---|---|---|
| 1 | `cargo test --workspace` | 通过 | 332 / 0 失败（39 个测试套件） | 68s（从零构建） |
| 1 | `cargo clippy --workspace --all-targets -- -D warnings` | 通过，0 警告 | — | 40s |
| 1 | `cargo check -p app-mcp-wasm --target wasm32-unknown-unknown` | 通过 | — | 8s |
| 2 | `pnpm -r typecheck` | 通过（12 个包） | — | 17s |
| 2 | `pnpm -r test` | 通过 | 437：web 169、dom 52、build 37、node 34、store 32、inspect 31、electron 24、react 24、hub 22、e2e 12 | 53s |
| 2 | `pnpm -r build` | 通过（shop 仅 chunk 体积提示） | — | 28s |
| 3 | `sdks/cpp` cmake + `ctest` | 通过 | 5/5 | 21s（含 cargo build） |
| 3 | `bindings/hub-c`：C11/C++17 编译头文件 + 链接 `libapp_mcp_hub.so` 冒烟 | 通过 | 启动 / wsAddr / apps / Anthropic 导出 / 关闭 | — |
| 4 | `dotnet test sdks/dotnet/AppMcp.sln` | 通过 | 44（AppMcp 35 + Hub 9） | 15s |
| 5 | `sdks/dart/app_mcp`：`dart test` | 通过，0 跳过（真实原生库） | 55 | 4s |
| 5 | `app_mcp_flutter`：`flutter test` + `example` 的 `flutter test integration_test -d linux` | 通过（Linux 桌面实构建实跑） | 6 + 1 | 2s + 18s |
| 6 | `sdks/python`：pytest（venv） | 通过，0 跳过 | 38 | 4s |
| 7 | `sdks/kotlin`：`./gradlew test --offline` | 通过（integration 用例实跑） | JVM 11 + Hub 3 + Robolectric 7（debug / release 各一次） | 35s + 29s |
| 8 | `sdks/swift`：`swift test` | 通过 | 18 + Hub 3 | 5s（增量） |
| 9 | `bash crates/codegen/scripts/verify.sh`（含 Android KSP、Windows provider） | 16/16 PASS | — | 12s |
| 10 | `pnpm --filter e2e test` | 通过 | 12/12 | 23s |

另：uniffi / hub-uniffi 的 Python、Kotlin、Swift 生成物与当前源码重新生成的结果逐字一致（未过期）。

环境问题：开工时根分区只剩 258 MB——上一会话遗留的 `cargo test -p app-mcp-hub` 卡在修复前的旧 `lifecycle` 测试（死锁版本，已挂起 33 分钟），其已删除的二进制仍占空间；已结束该进程并清空 `target-hub/debug` 后重建（验证后剩约 3.7 GB）。

## 已完成

| 模块 | 目录 | 结果 |
|---|---|---|
| 协议 | `crates/protocol` | 9 测试；含 `AppOverview` |
| 客户端核心（sans-IO） | `crates/core` | 36 测试；wasm32 可编译 |
| 清单 | `crates/manifest` | 17 测试；含 overview 校验 |
| Host | `crates/host` | 38 单元 + 17 集成；rmcp 3.5（限制协议版本 ≤ 2025-11-25）；overview 首次附带、`apps.list/select/overview`、实例路由、后台页面 180s 超时 |
| 原生运行时 | `crates/native` | 24 测试；`examples/fake_host.rs` 供各语言集成测试 |
| WASM 绑定 + Web SDK | `bindings/wasm`、`packages/web` | 66 测试；WASM gzip 121.8 KB（预算 150 KB） |
| React / Vite 插件 / Demo | `packages/react`、`packages/build`、`examples/shop` | 20 + 17 测试；Demo 含 `overview.md` |
| C ABI / C++ / C# | `bindings/c`、`sdks/cpp`、`sdks/dotnet` | 13 + ctest 3/3 + dotnet 16/16；`AM_API_VERSION = 2`（回调字符串改为由接收方释放） |
| Dart / Flutter | `sdks/dart` | 44 测试（C ABI v2、日志回调恢复、Dormant/Waking）；Flutter 单元 3 + Linux 桌面集成 1/1 实跑（真实 libapp_mcp.so + fake_host） |
| Node / Electron | `bindings/node`、`packages/node`、`packages/electron` | 24 + 15 测试（含 fake_host 集成） |
| 状态库适配 | `packages/store` | 32 测试；Zustand / Redux / Pinia |
| Host 超集 | `crates/host` | 42 单元 + 22 集成；`--http` Streamable HTTP（仅回环、Origin 校验、按会话首次附带 overview）；`--upstream`/`--config` 聚合现有 MCP 服务器（`<name>.<tool>`、崩溃退避重启）；README |
| HTML 属性声明 + 纯 HTML 示例 | `packages/dom`、`examples/vanilla` | 51 测试；`data-mcp-*` + WebMCP 声明式表单（`toolname`/`toolautosubmit`/`respondWith`）；集合（`data-mcp-key`）；精简快照；兼容 React 受控输入 |
| 兜底操作 | `packages/inspect` | 31 测试；`ui.outline/click/fill/press/scroll/submit/read`（`ui.eval` 需 `allowScript`）；大纲约为页面 HTML 的 1/12；操作只回变化摘要；默认不启用 |
| uniffi + Kotlin / Python / Swift | `bindings/uniffi`、`sdks/kotlin`、`sdks/python`、`sdks/swift` | Rust 6；Python 16+1；Kotlin JVM 5+1、Android AAR 可构建（4 ABI）；Swift 7+1（Linux）；生成脚本 `bindings/uniffi/scripts/generate.sh [--release] [--android]` |
| WebMCP 命令式 API 超集 | `packages/web/src/webmcp`（`@app-mcp/web/webmcp`） | web 118 测试（新增 52）；按 WebMCP 规范 2026-09-29 版（`document.modelContext`、`registerTool` 返回 Promise + `signal` 注销）并兼容旧接口；polyfill / 桥接原生两种模式；`appMcp.tool()` 工具双向镜像到浏览器原生 |
| 原生意图代码生成 | `crates/codegen` | 13 单元 + 12 快照/CLI；9 个目标：App Intents、AppFunctions（alpha12）、Windows App Actions + agent connector 清单，及 TS/C#/Swift/Kotlin/Python/Dart 类型化接口；`scripts/verify.sh` 16 步全过 |
| Hub SDK 阶段 1 | `crates/hub`、`crates/host`（薄壳） | hub 58 单元 + 12 集成；host 17+5 集成；单一调用逻辑（MCP 出口 / API / dispatch 共用）；5 种格式导出 + 名称编码；审批 / 配对回调；`examples/embed.rs` 实跑 |
| 生命周期 阶段 A | `crates/protocol`、`crates/core`、`crates/native`、`bindings/wasm` | core +29 生命周期测试；native +7 真实 fake_host；`toolsHash` 固定向量；休眠时销毁 tokio 运行时；另修：握手超时 10s、连接超时 5s、stop/休眠前冲刷结果、`fail_with_details`、`wss://`；C/Node/uniffi 已补 Dormant/Waking（`AM_STATE_DORMANT=8`、`AM_STATE_WAKING=9`） |
| 注册侧 + Web 修复 | `crates/manifest`、`packages/{web,build,react,electron}`、`examples/shop` | 清单 `wake`（平台→描述数组，spec/manifest.md §2.2）；惰性 `load`（`LazyToolDefinition`）；复制标签页 instanceId 冲突检测；Electron 桥接移入 web；wasm-opt（gzip 121.8→117.5 KB）；临时 paths 改为 `@app-mcp/source` 导出条件；web 141、build 37、react 21、electron 17、manifest 21 测试 |
| Hub SDK Node 绑定 | `bindings/hub-node`、`packages/hub`（`@app-mcp/hub`） | 19 测试（含与 `@app-mcp/node` 同进程集成）；OpenAI / Anthropic / Gemini / Vercel AI 适配；README |
| Android 真机验证 | `sdks/kotlin/sample-android` | 魅族 18 Pro 实测：握手、工具同步、调用（handler 在 Main 线程）、stateHints、资源读取、INVALID_INPUT / TOOL_NOT_FOUND / RESOURCE_NOT_FOUND、断线重连均正确；设备已还原 |
| 生命周期 B：Node / Electron | `bindings/node`、`packages/node`、`packages/electron` | node 34、electron 20 测试；`handleWake/wake/sleep/hold/connectNow/onIdleExit`；`failWithDetails`（zod issues 不再丢失）；node 支持惰性 `load`；Electron `attachLifecycle`（second-instance / open-url / argv 唤醒，idle-exit→quit） |
| Hub SDK C ABI + C# | `bindings/hub-c`、`sdks/dotnet/src/AppMcp.Hub` | C ABI 10/11（1 个因 hub 死锁失败，已转交修复）；C# 8 测试（嵌入 Hub + App 端 C# SDK 同进程，事件/审批回到 UI 线程） |
| 生命周期 B：C ABI v3 / C++ / C# / Dart·Flutter | `bindings/c`、`sdks/{cpp,dotnet,dart}` | `AM_API_VERSION 3`（`am_client_new_ex` + `AmClientOptions{struct_size}`，不破坏 v2 布局）；C 19 单元 + ctest 5（含 2 个生命周期往返）；C# 35（单实例管道转发、协议注册、WakeDescriptor）；Dart 55 + Flutter 6；四语言均对 fake_host 验证 休眠→唤醒跳过 sync→调用→再休眠 |
| Hub SDK uniffi 绑定 | `bindings/hub-uniffi`、`sdks/{kotlin/app-mcp-hub,python,swift}` | Rust 12；Kotlin 2、Python 3、Swift 2 集成（嵌入 Hub + 同进程 App：列工具、调用、导出+dispatch、事件、审批拒绝）；Python 自有 LLM 循环示例 |
| 生命周期 B：Web | `packages/{web,react,electron}`、`examples/shop` | web 168（新增 27，含真实 WASM 11）；URL 令牌/hashchange 唤醒、可见性、bfcache、freeze；`hold()` 返回 `{release()}`（与 Node 一致）；React `useHold`；WASM gzip 134.9 KB |
| 生命周期 Host 配合 | `crates/hub`、`crates/host` | hub 单元 66 + hub_api 12 + lifecycle 7（连跑 3 次稳定）；休眠实例、快速恢复、唤醒令牌、租约、`AppDormant`/`AppWaking`；`SystemWaker`（uri/aumid/apple-event/dbus/web-url，参数不经 shell）；`set_waker` 供厂商替换；死锁已修 + 回归测试 |
| 生命周期 B：Kotlin/Android、Swift、Python | `bindings/uniffi`、`sdks/{kotlin,swift,python}` | uniffi 9；Kotlin JVM 11 + Robolectric 7；Swift 20；Python 37；Android `WakeReceiver`→加急 WorkManager（无前台服务/WakeLock，令牌防伪）；SwiftUI `.appMcpLifecycle`；Python D-Bus / 单实例；**魅族 18 Pro 真机：空闲休眠→Home→广播唤醒→快速恢复→调用→再休眠 通过** |
| Hub 绑定：休眠 / 配置 / 自定义 Waker | `bindings/hub-*`、各语言 Hub 封装 | `AM_HUB_API_VERSION 2`（`am_hub_set_waker_cb` + `am_hub_waker_complete`）；Dormant 按名映射；6 个新配置；hub-c 14（死锁用例已恢复）、hub-uniffi 14、C# 9、TS 22、Kotlin 3、Python 4、Swift 3；全流程：休眠→列出 dormant→自定义 Waker→handleWake→调用成功 |
| Web 遗留 + e2e | `packages/{web,electron,dom}`、`e2e/`、`examples/shop` | Electron 桥接转发 `lifecycle.*`（按页面释放 hold）；dom 休眠/唤醒测试；WASM gzip 133.7 KB（sha2 compact）；**e2e 12/12**（`pnpm --filter e2e test`，约 45s，CDP + 手写 MCP 客户端，无新依赖）：M1 验收 7 + 生命周期 5（冷启动 web-url 唤醒、休眠不发 list_changed、hashchange 唤醒、惰性工具唤醒、可见自回连） |
| 编译期注释工具 `@mcp` | `packages/build`、`examples/shop` | 34 测试（新增 17）；虚拟模块 `virtual:app-mcp/annotated`；TS 类型推导 JSON Schema；shop 新增 `shop.info`、`shop.deliveryEstimate` |

## 优化队列（子代理串行，2026-10-01 起）

按顺序一个完成再开下一个。**原则（用户 2026-10-01）：从源码处解决问题，不在外层套壳**（不加转发进程、包装脚本、PATH 替身、monkeypatch、兜底转换）。
1. [x] Host 常驻服务：Hub 原生多会话（Streamable HTTP）+ 各平台用户级服务安装（systemd / launchd / Windows 登录任务），客户端直接连 HTTP；删除 `scripts/run-host.sh` 包装与 stdio 转发思路
   - 结果（2026-10-01）：`app-mcp-host serve` + `service install|uninstall|status|start|stop`（systemd --user / LaunchAgent / HKCU Run → 无窗口 `app-mcp-hostw.exe`）、`~/.app-mcp/config.json`、`/healthz` 单实例（已在运行退出码 0）、日志轮转、令牌（浏览器来源强制，`--auth all` 全部强制）；`.mcp.json` 改 HTTP；hub 单元 69 + host 单元 18 + serve 集成 5（另 systemd 实装 1，`--ignored`，已实跑并卸载）；e2e 12/12 改用 serve + HTTP 客户端；Windows 测试全过、Run 项实装/卸载与无窗口已验证
1b. [x] 去套壳清单（逐项源码修复）：Python hub 替换 `_uniffi_get_event_loop`（改为非 async 回调 + 完成句柄）；注释工具名 `shop.shop.info`（名称规则在 spec + build 插件源头统一为局部名）；e2e 的假 xdg-open 与去 wake 清单副本（Host 提供正式 Waker 配置）；隐藏态空闲休眠 reason 与 spec 一致；AUMID 传令牌改用 `IApplicationActivationManager`（并入第 7 项提前做）
   - 结果（2026-10-01）：hub-uniffi 回调改为同步 + 完成句柄（`ApprovalResponder` / `PairingResponder` / `WakeResponder`，删除 `CallbackError` / `WakeError`），Kotlin `suspend`（可指定 `CoroutineContext`）/ Swift `Task` / Python 线程池·事件循环·`asyncio.run` 各自适配，删除 monkeypatch 与后台事件循环；spec/protocol.md 3.1 局部名规则，build 注释扫描对 `<appId>.` 前缀报错、核心注册 / Host 同步 / 清单校验警告，shop 改 `@mcp info`；Host `lifecycle.waker` / `--waker system|none|{"exec":[...]}`（`ExecWaker` 把 WakeRequest JSON 写 stdin），e2e M1 用 `none`、生命周期用 `exec` + `e2e/src/record-wake.mjs`，删除 PATH 替身与清单副本；空闲计时到期一律 `idle`（`background` 仅进入后台立即休眠）；clippy `result_large_err` 以具名 `Callback` 实现消除（无 allow/expect）。验证：cargo test 363（serve 端口竞争偶发失败 1 次，与本次无关）、clippy Linux/Windows 0 警告、pnpm test 440 + e2e 12/12、Python 39、Kotlin JVM 11 + Hub 3 + Robolectric 7、Swift 18 + Hub 3、ctest 5/5、dotnet 36 + 9、dart 55
2. [x] 工具渐进暴露：默认只 `apps.*` + `apps.tools(appId)`，已选/最近用过的 App 直接列出
   - 结果（2026-10-01）：`HubConfig.tool_exposure: ToolExposure::{All, Progressive, Auto}`（默认 `Auto`，App + 上游工具数 > `tool_exposure_threshold` 默认 40 时渐进；工具少时行为不变）；新内置工具 `apps.tools {appId}`（任何模式可调用，渐进时才列出）；会话已列出的 App = `apps.tools` 展开 ∪ 调用过 ∪ `apps.select` / `select_instance` 选定；新增时 MCP 出口只向该会话发 `tools/list_changed`；路由与导出名不变（未列出的工具按全名 / 导出名仍可调用）；`ToolFilter.session`（`tools` / `export_tools` 按会话，显式 `apps` 不受影响）；`Hub::reset_waker`（绑定清除唤醒回调时恢复配置的 waker）；Host `tools: {exposure, threshold}` / `--tool-exposure` / `--tool-exposure-threshold`；hub-c / hub-node 配置 JSON 加 `toolExposure`、`toolExposureThreshold`、`waker`，hub-uniffi `HubConfig.tool_exposure / tool_exposure_threshold / waker`（`WakerConfig::{System, Disabled, Exec}`）与 `ToolFilter.session`，C# `HubOptions.ToolExposure / ToolExposureThreshold / Waker`、`ToolFilter.Session`，TS / Python / Kotlin / Swift 类型同步；spec/hub-api.md 3.7
3. [x] 原生本地 IPC 传输：Unix 域套接字 / Windows 命名管道 + 连接鉴权（回环 TCP 仅网页）
   - 结果（2026-10-01）：IPC 上跑同一套 WebSocket 帧（握手 URL `ws://localhost/`，无 TLS），协议逐字节不变；端点字符串 `unix:<绝对路径>` / `pipe:\\.\pipe\<名称>` / `ws(s)://`（`app_mcp_protocol::endpoint`，spec/protocol.md 第 1 节重写）；默认端点 Linux `$XDG_RUNTIME_DIR/app-mcp/hub.sock`（否则 `~/.app-mcp/run/hub.sock`）、macOS `~/.app-mcp/run/hub.sock`、Windows `\\.\pipe\app-mcp-<用户 SID>`、Android/iOS 无（仍 ws）；原生 SDK 缺省端点 = `APP_MCP_ENDPOINT` → 平台 IPC → ws（解析顺序，不是失败回退）；鉴权：Unix 目录 0700/套接字 0600 + 双向 `SO_PEERCRED`/`getpeereid` uid 核对，Windows 管道 SD `O:<sid>D:P(A;;GA;;;<sid>)` + 拒绝远程 + `FIRST_PIPE_INSTANCE`，SDK 核对管道所有者 SID（防抢占）；单实例：活套接字/已存在管道 → `AddrInUse`（serve 探测 /healthz 退出码 0），残留套接字删除重绑，普通文件拒绝覆盖，停止时删自己的套接字；`HubConfig.ipc_endpoint`（默认平台端点）+ `Hub::ipc_endpoint()` + `InstanceInfo.pid`（IPC 对端进程号）；Host `ipcEndpoint` / `--ipc-endpoint <ENDPOINT|none>`，/healthz 不变；hub-c `ipcEndpoint` + `am_hub_ipc_endpoint`（头文件 v4），hub-node `ipcEndpoint`，hub-uniffi `ipc_endpoint` / `enable_ipc` / `InstanceInfo.pid`，C# `IpcEndpoint`/`DisableIpc`，TS/Kotlin/Swift/Python getter；C ABI 布局不变（`host_url` 取值扩展，v4 说明）；所有测试的 Hub/Host 关闭默认端点或用临时端点。验证：cargo test 381（含 hub `tests/ipc.rs` 4、serve 经 IPC）、clippy Linux/Windows 0 警告、pnpm test 全过（hub 24、e2e 12/12）、Python 41、Kotlin JVM 11 + Hub 5 + Robolectric、Swift 18 + Hub 5、ctest 5/5、dotnet 36 + 10、dart 55；Windows（cargo.exe MSVC）protocol/hub/native/host/hub-c/hub-uniffi 全过（命名管道往返、pid、单实例、serve 经管道）+ dotnet Hub 10（C# App 经命名管道）；本机默认端点实测（serve 默认 → Python App 零配置经 IPC 配对，第二个 serve 退出码 0，SIGTERM 后套接字删除）。未测：macOS（`getpeereid`/`LOCAL_PEERPID` 路径）
4. [ ] Web：Chrome 本地网络访问 / CSP 拦截检测与提示；SharedWorker/主标签页单连接
5. [ ] WASM 瘦身：核心注册表去 BTreeMap、绑定改 JSON 交换（目标 gzip < 100 KB）
6. [ ] Android：绑定式 Service（Binder）传输与 bindService 唤醒；R8 规则、ABI 拆包
7. [x] Windows：`IApplicationActivationManager` 带参激活（AUMID 传令牌）
   - 结果（2026-10-01）：`SystemWaker` 的 aumid 改为 `ActivateApplication(aumid, "app-mcp-wake:<令牌>", AO_NONE)`（`windows` 0.62，COM STA，阻塞线程执行；`SystemWaker::action` 返回 `WakeAction`，去掉 explorer 与 `ignore_exit_code`）；Windows 实测：计算器空参数激活返回 PID 后结束（`--ignored activate_calculator`）；`wake-e2e.mjs` d 以令牌参数激活（计算器自身拒绝启动参数 0x80040904，判定为 Host 行为正确）；a/b 仍通过。打包 App 版的 d 需 MSIX 签名 + 开发者模式，本机未开启，未做
8. [ ] 鸿蒙 HarmonyOS NEXT：ArkTS SDK（Node-API 兼容）+ 意图框架代码生成
9. [ ] Tauri 插件（基于 crates/native，页面走 Tauri IPC）
10. [ ] 核心热路径：参数免完整解析、进程内共享运行时、休眠重建基准
11. [ ] Hub：Schema 校验缓存、导出名/会话 LRU·TTL、tracing 调用链与指标
12. [ ] MCP 2026-07-28 无状态协议迁移方案（文档 + 兼容层设计）

## 进行中（子代理）

> 2026-09-30 晚：会话额度用尽，Hub C/uniffi/Node 绑定、Host 生命周期配合、Node/Electron 生命周期、Android 真机验证 6 个代理中断，已恢复续跑。


- [ ] 生命周期 注册侧剩余：codegen 输出 `wake`；原生惰性 handler（工厂闭包）；`@app-mcp/node` 支持 `load`（恢复 electron 兼容性检查）；manifest 改用 protocol 的 `WakeDescriptor`



## 待办：集成阶段

### 必须修复
- [ ] Web：launchToken 如何传给网页未定义（spec 补充，如 URL 参数 `app_mcp_launch`）

### 集成与验证
- [ ] WASM 继续瘦身（需改代码）：core 注册表 BTreeMap→Vec/统一键类型（~87KB 未压缩）、`sort_unstable`、减少 Debug；wasm 绑定改 JSON 字符串交换去掉 serde_wasm_bindgen（~33KB）
- [ ] Android 其他 ABI（armeabi-v7a/x86/x86_64）.so 为旧版，发布前全 ABI 重编
- [ ] `bindings/uniffi/scripts/generate.sh` 加 conda 环境清理（`unset CFLAGS CPPFLAGS CXXFLAGS LDFLAGS CC CXX AR`）
- [ ] Android 未测：进程被杀后广播冷启动、Android ≤11 回退、加急配额耗尽、前台直接处理分支
- [ ] Python：dbus-daemon 拉起已退出进程、exit-when-idle 冷启动退出路径
- [ ] 已决定：`wake_from_launch` 默认关闭，spec/manifest.md 已注明（可随时改）
- [x] 核心：隐藏状态下空闲休眠上报 reason=`background`（非 `idle`）——已改：计时到期一律 `idle`，`background` 只用于进入后台立即休眠（spec/protocol.md 8.5、spec/lifecycle.md 4.1）
- [x] Python 事件循环替换依赖 uniffi 生成的 `_uniffi_get_event_loop`——已删除：hub-uniffi 回调改为同步 + 完成句柄
- [x] App 端 SDK 工具名是局部名（注册 `add` → `notes.add`），文档写清——spec/protocol.md 3.1；以 `<appId>.` 开头时核心 / Host / 清单校验警告，build 注释扫描报错
- [ ] Hub：API 会话无自动清理（需 `reset_session`）；导出名映射只增不删；`subscribe` 不区分厂商会话；MCP `serverInfo` 名仍为 app-mcp-host
- [ ] Python `qt_dispatcher` 未测（本机无 Qt）
- [ ] `crates/host/tests/serve.rs` `second_serve_exits_zero_when_healthy_instance_runs` 偶发失败（约 1/6；`free_port` 先绑后放的端口竞争，serve 启动即退出码 1）
- [x] hub-c / hub-node / hub-uniffi 的配置 JSON 尚未暴露 `waker`——已加（随优化队列第 2 项）；清除自定义回调时恢复配置的 waker（`Hub::reset_waker`）
- [ ] Kotlin jar 发布前用 `--release` 生成（debug `.so` 约 100 MB）
- [x] 全量验证：`cargo test --workspace`、`cargo clippy --workspace --all-targets`、`pnpm -r test/typecheck/build`、ctest、dotnet test、dart test（2026-09-30 全部通过，另含 Flutter / Python / Kotlin / Swift / codegen / e2e，见上方"全量验证"）
- [ ] 在 Claude Code 中实际操作 Demo（已配置：`.mcp.json` → `http://127.0.0.1:7718/mcp`，先 `app-mcp-host service install` 或 `serve`；需重启会话并批准项目 MCP 服务器）
- [x] 工具名重复前缀：shop 的注释工具 `shop.info` 全名变成 `shop.shop.info`——注释改为局部名 `@mcp info`；写成带 appId 前缀的全名时构建报错（不自动去前缀）

### 需在 Windows / macOS 上验证
- [x] SystemWaker Windows 实机（2026-10-01，Win11 25H2 build 26200，`tests/windows/wake-e2e.mjs` 7/7）：uri 冷启动 155ms、休眠实例经第二进程管道转交唤醒 145ms（`tools_current=true`，同 PID）、web-url(rundll32) 页面收到 `#app-mcp-wake=<token>`、aumid(explorer shell:AppsFolder) 拉起计算器
- [ ] SystemWaker 其他平台实机：macOS open -g / apple-event、Linux 桌面 gdbus；aumid 已经 `ActivateApplication` 传令牌（同 instanceId 回连判定保留为协议语义），真实打包 App 未测
- [x] C# 生命周期 Windows：真实 HKCU 注册表写入/删除（单元测试 `RealHkcuRegistrationOnWindows` + e2e）、未打包进程 `GetCurrentPackageFullName` 返回无包身份、跨进程 Mutex+命名管道（两个真实进程）、WPF 示例编译（0 警告）；dotnet test Windows 36 + Hub 9
- [ ] C# MSIX/AUMID 包身份检测实测（需打包 + 开发者模式）
- [ ] web-url 唤醒打开的浏览器标签页：页面脚本 `window.close()` 在 Chrome 中未能关闭外部打开的标签页——Web SDK 是否在回连后提示/自动关闭重复标签需设计
- [x] Rust 1.98 clippy（Windows 工具链较新）报 `result_large_err`：`crates/hub/src/app_server.rs:75`——握手回调改为具名类型实现 tungstenite `Callback` trait（签名归 trait 所有），无 allow / expect；Linux 1.93、Windows 1.98 均 0 警告
- [ ] Flutter 移动端唤醒（Android WakeReceiver→MethodChannel、iOS URL/app_links）真机验证
- [ ] codegen：App Intents 在 Xcode 实编；Windows App Actions 实际注册运行（Windows 上 `dotnet build` 生成的 provider 已通过，net9.0-windows10.0.26100.0；`Windows.AI.Actions` WinRT 类型本机可加载；注册需 MSIX + 开发者模式，本机未开）
- [ ] Host 增加「只暴露某个 App」模式，使 codegen 生成的 agent connector `static_responses` 与实际一致（与 ODR 网关设计合并考虑）
- [ ] codegen 输出清单 `wake` 字段（等注册侧 wake 格式定稿）
- [ ] Windows ODR：Host 工具是动态的，不能直接登记为 agent connector（要求 `tools/list` 静态响应）→ 设计固定工具集网关（`apps.list`/`apps.call`）转发到用户会话中的 Host；实测连接器隔离会话能否访问回环 TCP / 命名管道（需 build 26220.7262+；2026-10-01 本机 26200.9457：无 odr.exe、无 AgentRegistry 包，只有 `Microsoft.AIFabric.CBS.1.6`，ODR 不可用）
- [x] WPF 示例（`sdks/dotnet/samples/Wpf`）Windows 上编译通过（未运行：运行会注册 `wpf-sample` scheme）
- [x] Rust on Windows（MSVC 1.98.1）：protocol/core/manifest/native/hub/host/c/hub-c 全部测试通过，无需修复
- [x] Web on Windows：Windows Edge（headless）打开 WSL 中的 shop dev server（镜像网络），连上 WSL 的 Host，列出并调用 shop 工具
- [ ] Swift：SwiftUI 示例与 `.appMcpLifecycle` 修饰器在 Apple 平台编译；iOS/macOS URL 唤醒实机；发布改为 XCFramework 链接

### 文档
- [x] plan 文档 WebMCP 描述按规范最新版修正（`modelContext` 在 `document`；`unregisterTool`/`provideContext` 已移除；Chrome 149 起 Origin Trial）
- [ ] `@app-mcp/web`：`ResourceHandle` 可考虑加 `update({ description })`（dom 包需要）
- [x] plan 文档加「Hub SDK（厂商接入）」章节
- [x] Host README 中 Windows ODR / agent connector 说明是凭记忆写的，实现前需对照微软当前文档核实
- [ ] 测试用上游服务器是 `[[bin]] app-mcp-test-upstream`，`cargo install` 会一并装上——发布前处理
- [x] `app-mcp-plan.md`：附录对比表加入 WebMCP、MCP-B、MCP-FE、tauri-plugin-mcp、Windows 原生 MCP，写明"做超集"的决定；Web SDK 章节加 WebMCP 兼容
- [x] 原生开发者注意：appId 只能 `[a-z][a-z0-9-]`（不支持反向域名）
- [x] Dart：Flutter 热重启前需 `dispose`；Dart 无法实现 UI 线程无响应超时
- [x] Redux：`createAsyncThunk` 中应使用 `rejectWithValue({kind, message})` 传递错误类别

## 环境备注
- 另装：Gradle 8.14.3（`~/.local/gradle-8.14.3`，Java 联网需 `GRADLE_OPTS=-Dhttps.proxyHost=127.0.0.1 -Dhttps.proxyPort=10808`）、Swift 6.4.0（`~/.local/swift`）、JDK 21 另一份在 `~/.local/jdk-21`、Python venv `~/.cache/tastyrice/venv`。
- 根分区紧张：2026-09-30 曾满（剩 23MB），已删已完成代理的 target 目录，剩约 13G。开工前 `df -h /`。
- WSL2；仓库在 `/mnt/d`。**编译产物统一放仓库内 `target/`**（2026-09-30 起；原 `~/.cache/tastyrice/target-*` 已删除，根分区空间不足）。
- 工具链：Rust 1.93 + wasm32、wasm-bindgen 0.2.129（`~/.cargo/bin`）、Node 22、pnpm 10、.NET 9、Java 21、Android SDK/NDK（`~/Android/Sdk`）、Python 3.13、Dart 3.13.5（`~/.local/dart-sdk`）、Flutter 3.47.2（`~/sdk/flutter`）、带 javac 的 JDK（`~/sdk/jdk21`）。
- CMake 需 `CC=/usr/bin/gcc CXX=/usr/bin/g++`（conda 的旧编译器会被优先选中）。
- git 已 init，未提交任何内容。
