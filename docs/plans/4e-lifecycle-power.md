# 4e 生命周期功耗：空闲、心跳、重连与唤醒（计划）

> 状态：计划（2026-10-01）。行为契约以 `spec/lifecycle.md` 为准，本文件只负责分析与任务拆分；实施完成后结果写入
> `TASKS.md` 4e，本文件保留为决策记录。分析框架：已知的已知 / 已知的未知 / 未知的已知 / 未知的未知。

## 0. 结论

App 端"定时心跳 + 定时空闲休眠"把在线时长和唤醒次数绑在猜测上。三处浪费可以直接从代码确认：

1. 每次调用后 App 至少在线约 2 分钟：租约与空闲计时是**串行**的（先等租约到期再计空闲）；
2. Host 不在时 App **无限重连**，最长每 30 秒醒一次；
3. 双向心跳使 Hub 自己的"无消息断开"机制永不触发，本地传输上心跳本身也是多余的。

方向：本地传输上用连接断开事件（EOF / 对端死亡通知）代替心跳；"何时休眠"由掌握全局调用信息的 Hub 用租约决定，
App 端只保留很短的合并窗口；最终形态由 4d"按调用临时建立连接"实现。

## 1. 已知的已知（代码可证的事实）

| # | 事实 | 证据 | 影响 |
|---|---|---|---|
| K1 | 每次调用后 Hub 发 60 秒租约 | `crates/hub/src/hub.rs` `lease_ttl` 默认 60 s；`crates/hub/src/call.rs` `grant_lease` | 见 K2 |
| K2 | 休眠时刻 = max(空闲起点, 租约到期) + 空闲时长 | `crates/core/src/lifecycle.rs` `sleep_deadline` | 默认前台每次调用后在线约 120 s（60 + 60），后台约 75 s（60 + 15）；真机测试需 `--lease-ms 0` 才看得到休眠 |
| K3 | 心跳双向各一份：Hub 每 15 s ping，SDK 每 15 s ping，与传输无关 | `crates/hub/src/app_server.rs` `run_session` 的 `ping`；`crates/hub/src/hub.rs` `ping_interval`；`crates/core/src/lib.rs` `Heartbeat` 默认 | 在线时约 8 次唤醒 / 分钟 |
| K4 | Hub 有"无消息断开"：前台 45 s / 后台 180 s | `crates/hub/src/hub.rs` `idle_timeout` / `hidden_idle_timeout`；`app_server.rs` | 有心跳时永不触发 |
| K5 | 连接失败无限重试：0.5 s 起、×2、上限 30 s，无放弃条件；只有休眠请求能把 `Backoff` 转为 `Dormant` | `crates/core/src/connection.rs` `backoff_delay` / `enter_backoff`；`lifecycle.rs` `on_sleep_requested` | Host 不在时每 30 s 一次唤醒，直到进程被冻结 |
| K6 | 空闲条件：无进行中 / 排队调用、无资源读取、**无资源订阅**、无持有、不在休眠握手中 | `lifecycle.rs` `idle_conditions_hold` | 被订阅资源的 App 永不休眠 |
| K7 | 回连有快速恢复（`resumeToken` + `toolsHash` 一致时跳过同步） | `connection.rs` 握手完成处 | 回连代价小，积极休眠划算 |
| K8 | Android 唤醒：广播 → `WakeReceiver.goAsync` → 加急 WorkManager，配额用尽退回普通任务 | `sdks/kotlin/app-mcp-android/.../WakeReceiver.kt` `OutOfQuotaPolicy` | 退回普通任务可能延迟数分钟（U3） |
| K9 | 已连接时收到的唤醒令牌直接丢弃（修复休眠后 11 ms 回连） | 提交 ff05d8f | 已消除 |
| K10 | 示例 App 前台 10 s / 后台 5 s 空闲；核心默认 60 s / 15 s | `sdks/kotlin/sample-android/.../SampleApp.kt`；`crates/core/src/lib.rs` | 真机数据需注明所用配置 |

## 2. 已知的未知（需测量 / 验证）

| # | 未知 | 处理 |
|---|---|---|
| U1 | 单次回连、单次心跳的 CPU 时间与唤醒次数 | 唤醒去重任务的真机测量作为基线；4e 完成后同法复测对比 |
| U2 | Android 冻结缓存进程（freezer）后持有的连接：Hub ping 无响应是否断开、解冻后是否误判 | 同上，真机测 |
| U3 | 加急 WorkManager 配额耗尽后的延迟；Flyme 等 OEM 后台限制（杀进程、拦截广播） | 真机构造配额耗尽；记为 OEM 差异项 |
| U4 | Doze 期间本机回环 / `adb reverse` 是否可达 | `adb shell dumpsys deviceidle force-idle` 下测 |
| U5 | Chrome 隐藏标签页计时器限流（≥ 1 分钟对齐）是否导致网页心跳误判；SharedWorker 是否同样受限 | 浏览器实测 | （**已实测 2026-10-02**，Chrome 154 / Windows，后台标签页经原始 CDP 驱动：页面链式 1 s 计时器约 2 分钟后进入 60 s 密集限流；SharedWorker 不受限（249 次平均 1008 ms）；`heartbeat: 'always'` 时共享连接与直连标签页在 8 分钟以上的密集限流中心跳保持约 15–16.5 s、无断开；隐藏 idle 标签页 5.96 s 休眠（设定 5 s）；调用限流中的标签页 4.6–6.2 ms；Host 停 10 s 后两标签页在其就绪后 0.6 s 回连。问题：URL 唤醒休眠的隐藏标签页会**新开**标签页，原标签页一直休眠（见 TASKS 已有"web-url 唤醒打开的浏览器标签页"条目）。未测：真实最小化 / 遮挡窗口、标签页冻结、省电模式）
| U6 | Windows EcoQoS / 效率模式、macOS App Nap 对桌面运行时线程的影响 | 桌面实测；macOS 无环境，记为待验证 | （**已实测 2026-10-02**，C# 示例 idle 5 s、Core Ultra 9 290HX Plus：效率模式（EcoQoS + IDLE 优先级）空闲机器上唤醒 + 调用 41–104 ms、末次调用到休眠 2009–2015 ms，与正常相当；24 线程满载下仅 EcoQoS 44–239 ms；**IDLE 优先级 + 满载时唤醒失败**（单实例管道监听线程得不到 CPU，唤醒转发 5 s 超时 → `LAUNCH_FAILED`，负载结束后恢复），满载下首次空闲休眠延迟到 8.8 s。另发现：Host 停 10 s 后 App 按 A2 转休眠，重启的 Host 不知道该实例（`TOOL_NOT_FOUND`）、无法唤醒——设计缺口，记入生命周期缺口。macOS App Nap 未测）
| U7 | 各 Agent 的调用节奏（同一轮工具调用内的调用间隔分布） | 不预设：由 Hub 统计（B2），默认值保守 |

## 3. 未知的已知（项目已有、未充分利用的能力）

- `app/lease` 支持续租与收回：会话关闭时 Hub 发 `ttlMs: 0`（`crates/hub/src/lifecycle.rs` `release_leases`）。由 Hub 决定休眠不需要新协议，只需改租约语义与时长策略。
- `on-demand` 模式与 `graceMs`：启动不连接，调用完成后经合并窗口即休眠；手机当前默认却是 `idle`。
- `sleep_with_reason(Background)`：进入后台立即休眠的入口已存在，移动端未默认启用。
- 本地 IPC 天然感知对端退出（Unix 套接字 / 命名管道读到 EOF），且连接时已核对同用户；心跳在此多余。
- Hub 掌握每个会话、每个 App 的调用时间（`session_key`），是自适应租约的数据来源。
- 网页多标签页经 SharedWorker 共用一条连接（spec/protocol.md 第 9 节）。
- 4d 已规划 Binder `linkToDeath`、按调用绑定 + 15 s 宽限，与本计划终态一致。

## 4. 未知的未知（无法枚举的风险与限制手段）

- **可观测**：`/status` 与 `doctor` 为每个 App 计数——回连次数、唤醒次数、在线秒数、心跳次数、未能休眠的原因（租约 / 订阅 / 持有 / 调用）。
- **功耗回归测试**：核心是 sans-IO 状态机，用确定性测试断言"空闲 1 小时内的定时器触发与连接发起次数上限""Host 不在时重试次数上限""调用后在线时长上限"；新增唤醒即 CI 失败。
- **开关与回退**：新策略为配置项（默认开启），可切回旧行为；按平台逐步放开。
- **唤醒速率上限**：每 App 每分钟最多被唤醒 N 次，超出时 Hub 拒绝派发并给出明确错误码，防止唤醒 / 休眠循环。

## 5. 方案与任务

### P0 修掉明显浪费（现有架构内）

- **A1 租约与空闲计时并行**：休眠时刻 = max(空闲起点 + 空闲时长, 租约到期)。前台调用后在线 ~120 s → ~60 s。
- **A2 Host 不在时停止无限重试**：`idle` / `on-demand` 下连续 N 次（配置，默认 3）以 `HOST_NOT_RUNNING` 失败 → 转 `Dormant`，
  等可见、App 主动唤醒或 Host 唤醒；`persistent` 保持现状。
- **A3 本地传输去掉心跳**：IPC 两端不发心跳，靠 EOF；本机回环 TCP 不发心跳，半开连接由下次派发调用的超时发现；
  跨机 / 远程连接只由 SDK 单向心跳，Hub 用"无消息断开"判断。在线唤醒 ~8 次 / 分钟 → 0（本地传输）。
- **A4 唤醒去重**：Hub 不重复发唤醒、实例先连上即作废令牌；Android 端已连接时丢弃唤醒。（进行中，单独提交）

### P1 由 Hub 决定休眠

- **B1 App 端只留合并窗口**：调用完成后 1–2 s 合并窗口即休眠（配置化）；移动端、托盘程序默认 `on-demand`，桌面 / 网页 `idle` + 短窗口；
  是否继续在线只由租约决定。
- **B2 自适应租约**：Hub 按（会话, App）统计相邻调用间隔（最近 N 次 p90 + 余量，设上下限）作为租约；会话结束或 MCP 请求流空闲时立即收回；
  无历史时用保守默认值。
- **B3 资源订阅不再强制在线**：订阅期间 App 可休眠；资源可声明"需实时推送"才保持连接；其余由 App 在变化时回连推送或模型下次读取时拉取。
- **B4 后台立即休眠**：移动端进入后台且无调用 / 持有时直接 `sleep_with_reason(Background)`。

### P2 根本解决（并入 4d）

- **C1 按调用临时建立连接**（Hub 拨 App，Android `bindService`），用完经宽限解绑；App 无常驻连接、无心跳、无空闲计时器。
- **C2 系统对端死亡通知**：Binder `linkToDeath`、D-Bus `NameOwnerChanged`、命名管道 EOF。
- 4d 之后 P1 的租约仍用于决定解绑时机。

### 观测与防护（随 P0 / P1 一起做）

- **O1** `/status`、`doctor` 每 App 计数与"未休眠原因"。
- **O2** 核心功耗回归测试（定时器 / 连接次数上限）。
- **O3** 新策略配置开关与回退。
- **O4** 每 App 唤醒速率上限。

## 6. 实施顺序与验证

1. 唤醒去重（A4）完成并提交，其真机测量为基线。
2. 4e 第一部分：A1、A2、A3、O1、O2、O3、O4（核心 + Hub + native + 网页驱动 + spec/lifecycle.md、spec/protocol.md）。
3. 4e 第二部分：B1–B4（含各平台封装默认值、Kotlin / Swift / Dart / 网页封装）。
4. 一轮批量真机复测（魅族 18 Pro）：一组调用后的在线秒数、唤醒次数、CPU 时间、Host 不在时 1 小时唤醒次数、冻结 / Doze 场景；
   浏览器隐藏标签页限流（U5）实测；Windows 效率模式（U6）实测。
5. C1、C2 随 4d 实施。

行为变化（调用后在线变短、本地传输无心跳）写入 `spec/lifecycle.md`，并保留切回旧行为的配置项。
