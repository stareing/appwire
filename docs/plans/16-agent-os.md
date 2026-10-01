# 16 Agent OS：数据、权限、事件、事务与协同（计划）

> 状态：计划（2026-10-02）。把 AppWire 视为 Agent 的"系统调用层"（模型 = 用户态程序，App 工具 = 系统调用，Hub = 内核），
> 按操作系统子系统找差距。行为契约分别落在 `spec/protocol.md`、`spec/hub-api.md`、`spec/manifest.md`；本文件只负责分析与任务拆分，
> 实施结果写入 `TASKS.md` 第 16 项。与第 13–15 项已规划内容不重复。

## 0. 子系统对照

| 子系统 | 已有 / 已规划 | 本项补的 |
|---|---|---|
| 进程管理 | 生命周期、休眠唤醒、4e 功耗、4d 连接即唤醒 | O5 冷启动预算与预测预热 |
| 进程间通信 | IPC / WebSocket、按名寻址（4d） | N1 数据句柄（App 间传数据不经模型） |
| 命名与发现 | 4d 多来源发现 | N4 标准意图（按动作找 App） |
| 权限 | 确认与授权归 Agent / App（第 14 项第 1 节）；本库只如实传递声明、限流 | N5 Agent 身份（只用于句柄绑定与调用日志）；N2 句柄访问范围 |
| 调度 | 按实例路由、`apps.select` | N6 多 Agent / 人机并发仲裁 |
| 中断与事件 | 资源订阅 | N3 事件与触发器 |
| 事务 | 无 | N7 预演、幂等键、跨 App 补偿 |
| 上下文（内存） | 渐进暴露、4c 界面级暴露 | O1 工具检索与排序；O3 只读结果缓存 |
| 驱动 | 第 15 项导入器、OS 层 | — |
| 包管理 | 第 13 项分发 | O4 工具 schema 演进与弃用 |
| 可观测 | doctor / status、第 11 项 tracing | 端到端链路（Agent 一轮 → 调用 → App handler）并入第 11 项 |

## 1. 已知的已知（代码可证的事实）

| # | 事实 | 证据 |
|---|---|---|
| K1 | 协议无进度通知，长调用只能等到 `response_timeout` | `spec/protocol.md` 无 progress 消息；全仓无 `notifications/progress` 实现 |
| K2 | 无调用级幂等键 / 去重；`idempot` 只出现在句柄 `dispose` / `release` 语义中 | `crates/native/src/lib.rs` 334、377、439 行 |
| K3 | 无预演（dry-run / preview）、无工具检索、无信息流标签、无事件触发器 | 全仓搜索 `preview` / `embedding` / `taint` / `trigger` 均无对应实现（`crates/hub/src/lifecycle.rs` 的 trigger 为唤醒触发，非事件） |
| K4 | Hub 只知道 MCP 会话与 `clientInfo`，不区分 Agent 身份与授权 | `crates/hub/src/call.rs` `ApprovalRequest::session` |
| K5 | 资源订阅与 `realtime` 声明已有（4e B3）；`toolsHash` 可判断工具列表变化 | `spec/lifecycle.md` 第 13 节；`crates/protocol/src/hash.rs` |
| K6 | codegen 已把工具映射到 App Intents / AppFunctions / Windows App Actions / 鸿蒙意图 | `crates/codegen/src/targets` |
| K7 | 渐进暴露按 App 分层（`apps.*` + `apps.tools(appId)`） | `TASKS.md` 第 2 项；`spec/hub-api.md` 3.7 |
| K8 | rmcp 3.5.0 已支持 MCP 进度通知与 elicitation（服务端 `Peer`） | `~/.cargo/registry/.../rmcp-3.5.0/src/service/server.rs`：`notify_progress`（914 行）、`elicit`（1144 行） |

## 2. 已知的未知

| # | 未知 | 处理 |
|---|---|---|
| U1 | 断线 / 唤醒重连后，进行中的写调用是否会被重发（决定 N7 幂等的紧迫度） | **先验证**：读 `crates/core` 调用队列与 `crates/hub` 派发重试路径，写复现测试；结论写回本文件 |
| U2 | MCP 侧传句柄的载体（`resource_link` 内容 + Hub 自有 URI 方案）与各 Agent 的呈现 | **验证**：以 MCP 2025-11-25 规范与 Claude Code 实测为准；第 12 项无会话协议下句柄的生命周期一并设计 |
| U3 | MCP Apps（UI 资源扩展）的规范现状与 Agent 支持面 | **验证**：查官方规范与 Claude Code 支持情况；未确认前不实施 N8 |
| U4 | 标准意图词表的来源（自定义 vs 对齐 schema.org Actions / App Intents 领域 / AppFunctions 预置 schema） | **调研**后定词表；先做 5 个高频动词试点 |
| U5 | 本地向量检索的体积与依赖（移动端 `mobile` 精简包能否承受） | **保守**：默认关键词 + 使用统计排序；向量检索为可选特性（cargo feature），默认关闭 |
| U6 | 信息流控制的标签传播粒度（整次结果 vs 字段级）与误伤率 | **保守**：先做结果级标签 + 只拦"私密 → 外发"一类；记录拦截次数后再细化 |
| U7 | 人机并发的"用户正在操作"信号来源（各 UI 框架焦点 / 编辑状态） | SDK 侧显式 API（`busy()` / 对象锁），不自动推断 |

## 3. 未知的已知（可复用）

- 唤醒、租约、合并窗口（4e）可直接承载 N3 触发器：事件到达 → 按规则唤醒 Agent 侧回调，不需新传输。
- 资源订阅 + `realtime`（4e B3）是事件的现成通道；N3 只需加"事件"语义与规则表。
- App 的注解与内容标注由第 14 项如实传递，Agent 据此自行决定确认；本项不新增确认通道。
- 第 15 项 X2 `undo` 是 N7 跨 App 补偿的原语；第 11 项调用日志是补偿的依据。
- codegen 的系统意图映射（K6）是 N4 标准意图的输出端。
- `toolsHash` 与快速恢复（K5）是 O4 schema 演进的检测基础。

## 4. 未知的未知（限制手段）

- **默认关闭或保守默认**：N1 句柄有 TTL 与大小上限；N3 触发器每条规则有频率上限并经第 14 项限流；O1 向量检索默认关；N9 远程默认关。
- **单一定义**：意图词表、数据标签、句柄 URI 格式各自只在一处定义（spec），其他位置引用。
- **回归测试**：每项附确定性测试（句柄过期 / 越权访问、标签传播、触发器风暴、重复写调用去重、并发排队公平性）。
- **可观测**：`/status`、`doctor` 增加句柄数量与内存、拦截次数、触发器触发次数、去重命中数。

## 5. 方案与任务

### 第一部分：正确性（最先做）

- **N7a 幂等键**：写及以上风险的调用带 `callId`（Hub 生成，重发保持不变），SDK 在有效期内按 `callId` 去重并返回首次结果；先完成 U1 验证。
- **O2 进度与取消**：协议新增进度消息，Hub 透传为 MCP `notifications/progress`；取消一路传到 App handler（已有取消路径则复用）。

### 第二部分：数据面与信息流

- **N1 数据句柄**（由第 17 项第二部分实现，契约见 `docs/plans/17-content-files.md`，此处不另行定义）：工具结果可返回句柄（`resource_link` + TTL + 大小上限），其他工具参数可引用句柄，Hub 在 App 间搬运，模型只见元信息；
  句柄绑定 MCP 会话与授权范围，过期即删。
- **N2 句柄访问范围**：句柄只能被签发它的会话使用、有 TTL，App 可声明句柄只读或仅限指定 App 消费；本库只执行 App 的声明，不做数据分级或外发判定（第 14 项第 1 节）。

### 第三部分：多 Agent 与协同

- **N5 Agent 身份**：Agent 首次连接时登记身份（`clientInfo` + 本机令牌 / 进程信息），用于句柄绑定与调用日志；授权由 Agent 自身配置负责，本库不做。
- **N6 并发仲裁**：SDK 提供 `busy()` / 对象锁；Hub 对写调用排队或返回明确错误；多会话对同一 App 公平排队。

### 第四部分：场景扩展

- **N3 事件与触发器**：App 在清单声明可发出的事件；用户 / Agent 注册"事件 → 提示"规则，Hub 在事件到达时回调 Agent 宿主（Hub SDK 回调；Host 侧经 MCP 通知）；本库只投递事件，不代 Agent 发起调用（调用及其授权由 Agent 负责）。
- **N4 标准意图**：定义通用动词 schema（先试点 `message.send`、`calendar.create`、`media.play`、`file.share`、`navigation.open`），App 声明实现；
  Hub 按用户默认 App 路由；codegen 输出到系统意图框架。
- **O1 工具检索**：`apps.search(query)`，按关键词、最近使用、成功率、当前可见界面（4c）排序；可选本地向量索引（U5）。
- **O3 只读结果缓存**：`read` 工具与资源按 App 声明的 TTL / 版本号缓存，命中时不唤醒 App。
- **O4 schema 演进**：字段弃用标记、兼容规则与 Agent 侧缓存失效策略，写入 `spec/manifest.md`。
- **O5 冷启动预算**：App 声明唤醒耗时，Hub 返回预计等待并决定预热。

### 第五部分：依赖外部规范 / 远程（最后）

- **N7b 预演与跨 App 补偿**：工具可选 `preview`（返回将执行的操作，供 Agent 展示）；多步操作失败时按 `undo` 逆序补偿（依赖第 15 项 X2）。
- **N8 App 界面嵌入 Agent**：U3 确认后再定。
- **N9 跨设备接力**：依赖第 15 项 R1 远程鉴权设计。
- **N10 本地小模型辅助**：工具排序、参数提示、敏感信息识别；可选、默认关闭。

## 6. 顺序与验收

1. 第一部分（N7a、O2）与第 14 项同期完成。
2. 第二部分（N1 → N2）在 4c 之后；第三部分（N5、N6）在 4d 之后（Agent 身份与按名寻址共用身份模型）。
3. 第四部分与第 15 项穿插；第五部分最后。

验收：
- 断线重连后重复的写调用只执行一次（回归测试复现 U1 场景）。
- 长任务在 Claude Code 中显示进度，取消后 App handler 收到取消。
- "把截图发给联系人"全程图片不进入模型上下文；句柄被其他会话或未声明的 App 使用时被拒绝。
- 两个 Agent 只能看到各自被授权的 App；用户编辑中的对象不被 Agent 覆盖。
- 事件触发器、标准意图、工具检索各有端到端示例。
