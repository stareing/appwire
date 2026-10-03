# 一致性测试套件（第 15 项 Y1）

同一份用例（`cases/*.json`）由每个 SDK 的薄 runner 执行，核对 **Host 观察到的协议行为**（spec/protocol.md、
spec/lifecycle.md）。用例只描述数据，不含任何语言的代码；新增 SDK 只需写一个 runner，新增用例不需要改 runner
（除非用到新的 handler 能力）。

```
conformance/
  cases/*.json          用例（本文件第 2 节）
  divergences/<sdk>.json  已登记的偏差：{ "<caseId>": "原因" } → 结论 xfail（每个 SDK 一个文件）
  matrix.mjs            汇总各 runner 的报告，打印 SDK × 用例矩阵（`pnpm conformance:matrix`）
  runner/support.mjs    JS 系 runner（Node、Web、鸿蒙）共用：列用例、定位 / 构建 fake_host、驱动进程、解释 handler
```

## 1. 工作方式

```
runner（各语言）                         fake_host --case <用例> --sdk <名称> --report-dir target/conformance
  读用例 app 部分 ──注册工具/资源──▶ SDK ◀──WebSocket──▶  按用例 host 部分发调用 / 读取 / 取消 …
  看到 {"type":"wake"} 行 → handleWake(arg)                 打印每一步（--trace 时含 SDK 发来的每条消息）
  等 fake_host 退出                                         结束时按 expect / contains / absent 核对
                                                            打印 {"type":"verdict"}，写 target/conformance/<sdk>/<case>.json
```

- 假 Host 复用 `crates/native/examples/fake_host.rs`（各 SDK 的集成测试已经在用），一致性用到的参数都是新增的可选参数
  （`--case`、`--trace`、`--catalog`、`--delay`、`--call-id`、`--invoke-timeout-ms`、`--cancel-after-ms` 等，见该文件头注释）。
- **核对只有一份实现**：`crates/native/examples/support/conformance.rs`（Rust）。runner 不做匹配，只看 fake_host 的退出码
  与结论行；这样各语言 runner 都很薄，期望也只定义在用例里。
- 结论：`pass`；`fail`（退出码 3）；`xfail`（不通过但已登记在 `divergences/<sdk>.json`，退出码 0）；`xpass`（登记了偏差却通过了，
  应删除登记）；`skip`（runner 不支持用例的某项能力，见 `requires`）。
- 汇总：`node conformance/matrix.mjs`（`--markdown` 输出表格，`--strict` 有 fail / xpass 时退出码 1）。

## 2. 用例格式

```jsonc
{
  "id": "errors",                       // 与文件名相同
  "title": "…", "spec": ["protocol.md 4"],
  "requires": ["userAction"],           // runner 能力（第 4 节）；缺任一项则 skip
  "app": {
    "config": {                         // 可选
      "lifecycle": { "mode": "idle", "idleTimeoutMs": 300, "graceMs": …, "mergeWindowMs": … },
      "callDedup": { "ttlMs": …, "maxEntries": … },
      "maxConcurrentCalls": 1,
      "maxQueuedCalls": 64,             // 需能力 callScheduling
      "busyPolicy": "reject",           // 需能力 busy：'reject' | 'queue'（spec/protocol.md 5.3「用户正在操作」）
      "navigateInBackground": false    // 给出时调用 SDK 的对应设置（spec/protocol.md 3.4）；缺省用 SDK 的平台缺省
    },
    "visibility": "hidden",             // 可选：启动前把实例可见性设为该值（visible / hidden / frozen）
    "busy": true,                       // 可选（需能力 busy）：启动前调用 setBusy(true)
    "tools": [ <工具声明> ],
    "resources": [ <资源声明> ],
    "events": [ <事件声明> ],           // 可选（需能力 events）：启动前声明的事件（2.2）
    "navigation": { "<页面>": <导航行为> }   // 可选：给出时设置导航回调（2.4），缺省不设置
  },
  "host": {
    "toolInfo": false, "trace": true,   // trace 缺省开启
    "leaseMs": …, "rejectSleepMs": …, "timeoutMs": 10000,
    "ops": [ <操作> ]
  },
  "expect":   [ <模式> ],               // 按顺序出现（中间可夹其他行）
  "contains": [ <模式> ],               // 任意位置出现
  "absent":   [ <模式> ]                // 不得出现
}
```

App 固定为 `appId: "conf"`、`appName: "Conformance"`；runner 连接 fake_host 打印的 `LISTENING` 地址（TCP 时用
`ws://<addr>/app`）。

### 2.1 工具声明

`name`、`description`（必填）；`inputSchema`、`risk`、`activation`、`title`、`annotations`、`outputSchema`、`surface`、`page`、
`backgroundTool`、`enabled`（缺省 true）按协议同名字段原样传给 SDK 的注册 API；`concurrency`、`exclusive`（SDK 内的调用调度，
spec/protocol.md 5.3，需能力 `callScheduling`）同样传给注册 API。未给出的字段不传（SDK 用自己的缺省值）。`handler` 描述 handler 的行为：

| 键 | 含义 |
|---|---|
| `progress: [{progress, total?, message?}]` | 依次报告进度（`ctx.progress`） |
| `delayMs: n` | 等待 n 毫秒（被取消 / 超时后可提前结束；之后的完成结果由 SDK 丢弃） |
| `mutate: [<变更>]` | 修改注册表（2.3） |
| `counter: true` | 本工具的执行次数 +1（每个工具独立计数，从 1 开始） |
| 结果（以下取第一个出现的） | |
| `throw: "消息"` | 以该语言最普通的方式失败（抛异常 / 返回 Err / 回调 fail），期望 `HANDLER_ERROR` |
| `userAction: {message, reason?, uri?}` | SDK 的"需要用户操作"构造（`USER_ACTION_REQUIRED`） |
| `result: {data?, status?, stateResource?, summary?, stateHints?, annotations?}` | SDK 的完整结果 API；缺 `data` = 无返回值 |
| `return: <JSON>` | 直接返回该值（含 `null`：显式返回 null） |
| `echo: true` | 返回调用参数 |
| `returnIdempotencyKey: true` | 返回 `{"idempotencyKey": <handler 上下文中的幂等键，没有时为 null>}`（spec/protocol.md 3.3） |
| `counter: true`（无其他结果时） | 返回 `{"count": <执行次数>}` |
| `emit: [{name, payload?}]`（无其他结果时） | 返回 `{"emitted": [...]}`：每项为 SDK 发事件 API 的结果 `true` / `false`，本地错误（未声明、名称不合法、载荷不是对象或超限）为 `"error"`（需能力 `events`） |
| 都没有 / `returnNothing: true` | **该语言的"无返回值"**：`void` / `None` / `undefined` / `Unit` / `complete(null)` |

执行顺序：`progress` → `delayMs` → `mutate` → `emit`（依次发出事件，载荷原样以 JSON 交给 SDK）→ `counter` 计数 → 结果。

### 2.2 资源声明

`name`、`description`、`mimeType?`、`annotations?`、`realtime?`；`read` 描述读取：`{return: <JSON>}`、
`{fail: {message, kind?, details?}}`（带结构化详情失败，`kind` 缺省 `HANDLER_ERROR`；`details` 为对象时合并进错误 `data`）、
`{userAction: {message, reason?, uri?}}`、`{throw: "消息"}`。

事件声明（`app.events`、变更 `declareEvent`）：`name`、`description`、`payloadSchema?`，原样传给 SDK 的声明事件 API
（spec/protocol.md 3.5）。

### 2.3 注册表变更（handler 的 `mutate`）

| 变更 | 含义 |
|---|---|
| `{op: "register", tool: <工具声明>}` | 注册新工具 |
| `{op: "update", name, set: {字段: 值}}` | 更新声明：`set` 中的字段替换，**值为 `null` 表示清除该声明**（如 `annotations: null`），未列出的字段不变 |
| `{op: "remove", name}` | 注销 |
| `{op: "disable" / "enable", name}` | 禁用 / 启用 |
| `{op: "busy", value: bool}` | 调用 SDK 的 `setBusy(value)`（需能力 `busy`） |
| `{op: "declareEvent", event: <事件声明>}` / `{op: "removeEvent", name}` | 声明（同名替换）/ 撤销事件（需能力 `events`） |

runner 用该 SDK 最自然的 API 实现（整体替换型 API 先合并再整体更新；补丁型 API 直接传 `null`）。

### 2.4 导航行为（`app.navigation`）

给出 `app.navigation` 时，runner 用该 SDK 最自然的 API 设置导航回调（spec/protocol.md 3.4，回调在封装层约定的线程上执行）；
没有该键时**不设置**。回调按 `page` 查表，未列出的页面以"失败"（`NAVIGATION_FAILED`）完成，消息含页面名。每个页面的行为：

| 键 | 含义 |
|---|---|
| `mutate: [<变更>]` | 先修改注册表（2.3，如注册新页面的工具），再按下列结果完成 |
| `deny: "消息"` | 拒绝（`NAVIGATION_DENIED`） |
| `fail: "消息"` | 失败（`NAVIGATION_FAILED`） |
| `userAction: {message, reason?, uri?}` | 需要用户操作（`USER_ACTION_REQUIRED`，SDK 导航句柄的对应构造） |
| `throw: "消息"` | 以该语言最普通的方式在回调里出错（抛异常 / panic），期望封装层转为 `NAVIGATION_FAILED` |
| `failParams: true` | 失败，消息为收到的页面参数（JSON 文本，可被重新序列化；没有参数时为空） |
| 都没有 | 完成（`{ok: true}`） |

### 2.5 Host 操作（`host.ops`）

| 操作 | 含义 |
|---|---|
| `{invoke, args?, callId?, timeoutMs?, cancelAfterMs?, idempotencyKey?, priority?, noWait?}` | 发 `tools/invoke`（`idempotencyKey` / `priority` 给出时原样带上该字段），等结果（`cancelAfterMs` 到期仍未完成则发 `tools/cancel`）；`noWait: true` 时不等结果、立即执行下一步（结果到达时照常打印，全部操作完成后等齐再结束） |
| `{read}` | 发 `resources/read` |
| `{navigate, params?}` | 发 `app/navigate {page, params?}`，等回复 |
| `{catalog: settleMs}` | 继续处理消息 settleMs 后打印 Host 当前目录与按 8.4 计算的 `toolsHash` |
| `{delay: ms}` | 继续处理消息 ms 后执行下一步 |
| `{awaitSleep: true}` | 等 SDK 发 `app/sleep` 并接受 |
| `{wake: true}` | 紧跟 `awaitSleep`：打印 `{"type":"wake","arg"}`，runner 调 `handleWake(arg)`，Host 等回连 |

### 2.6 fake_host 打印的行（模式匹配的对象）

`{"type":"tools",…}`（每次 `app/ready`）、`{"type":"events","events"}`（`events/sync`）、`{"type":"event","name","eventId","payload"?}`
（`events/emit`）、`invoke` / `read` / `navigate`（`name`，`result` 或 `error`）、`progress`、`sleep`、`wake`、`hello`
（回连）、`catalog`、`cancel`、`recv`（`--trace`：`{"type":"recv","method","params"}`，不含 `ping`）。

### 2.7 模式

- 对象：模式中的每个键都要匹配（实际对象可以多出键）；数组：长度相同、逐个匹配；数字按数值比较（`1` 等于 `1.0`）。
- 操作符（单键对象）：`{"$absent": true}`（该键不存在）、`{"$any": true}`、`{"$type": "string"|"number"|…}`、
  `{"$contains": "子串"}`、`{"$oneOf": [模式…]}`、`{"$has": [模式…]}`（数组中每个模式至少匹配一个元素，顺序无关）、
  `{"$bind": "变量"}`（第一次出现时绑定，之后必须相等；在 `expect`、`contains` 之间共享）。

## 3. 写一个 runner

1. 列出 `conformance/cases/*.json`（可按环境变量 `APP_MCP_CONFORMANCE_CASES=id1,id2` 过滤）。
2. 用例 `requires` 中有本 runner 不支持的能力：以 `--skip "<原因>"` 运行 fake_host（只写报告），结束。
3. 启动 `fake_host --case <用例路径> --sdk <名称> --report-dir <仓库>/target/conformance`
   （可执行文件：环境变量 `APP_MCP_FAKE_HOST`，或该语言集成测试已有的定位方式）。
4. 读到 `LISTENING <地址>` 后：按 `app.config` 创建客户端（appId `conf`），按 2.1 / 2.2 注册，启动。
5. 逐行读 stdout：`{"type":"wake"}` → `handleWake(arg)`；`{"type":"verdict"}` → 记下结论。
6. 等 fake_host 退出，停止客户端。退出码非 0 或结论为 `fail` → 该用例失败（打印 `failures`）。

## 4. runner 能力（`requires`）

| 能力 | 含义 |
|---|---|
| `toolOptions` | 工具 `annotations`、`outputSchema` |
| `mutate` | handler 内注册 / 更新（含 null 清除）/ 注销 / 启用禁用 |
| `richResult` | 完整结果（status、summary、stateResource、stateHints、annotations） |
| `userAction` | `USER_ACTION_REQUIRED` 构造 |
| `progress` | `ctx.progress` |
| `resourceOptions` | 资源 `annotations` / `realtime` |
| `readFailure` | 资源读取失败带 details / `USER_ACTION_REQUIRED` |
| `lifecycle` | `idle` 模式与毫秒级空闲时长 |
| `wake` | `handleWake(arg)` |
| `surface` | 工具 `surface` / `page` |
| `navigation` | 设置导航回调（2.4） |
| `backgroundTool` | 工具 `backgroundTool` |
| `backgroundNavigation` | `app.visibility`、`app.config.navigateInBackground`、导航行为 `userAction` |
| `idempotencyKey` | handler 上下文中的幂等键（handler 结果 `returnIdempotencyKey`） |
| `callScheduling` | 工具 `concurrency` / `exclusive`、`app.config.maxQueuedCalls`，且 handler 能并发执行（`delayMs` 不独占分发线程） |
| `busy` | `app.busy`、`app.config.busyPolicy`、变更 `{op: "busy"}`（spec/protocol.md 5.3「用户正在操作」） |
| `events` | `app.events`、handler `emit`、变更 `declareEvent` / `removeEvent`（spec/protocol.md 3.5 事件；一期只有 Rust runner 支持，其他 runner 跳过） |

## 5. 各 SDK 的 runner

| SDK | runner | 运行 |
|---|---|---|
| Rust native | `crates/native/tests/conformance.rs` | `cargo test -p app-mcp-native --test conformance` |
| C++ / C ABI | `sdks/cpp/tests/conformance_runner.cpp`（`cpp` 走 C++ 封装；`c` 直接调 `am_*`） | `ctest -L conformance`（CTest `conformance_cpp` / `conformance_c`） |
| C# | `sdks/dotnet/tests/AppMcp.Tests/ConformanceTests.cs` | `dotnet test tests/AppMcp.Tests --filter FullyQualifiedName~ConformanceTests` |
| Dart | `sdks/dart/app_mcp/test/conformance_test.dart` | `dart test test/conformance_test.dart` |
| Python | `sdks/python/tests/test_conformance.py` | `python3 -m pytest tests/test_conformance.py` |
| Kotlin（JVM） | `sdks/kotlin/app-mcp/src/test/kotlin/dev/appmcp/ConformanceTest.kt` | `./gradlew :app-mcp:test --tests dev.appmcp.ConformanceTest` |
| Node | `packages/node/src/conformance.test.ts` | `pnpm --filter @app-mcp/node test` |
| Web（WASM，Node 上运行） | `packages/web/test/conformance.test.ts`（唤醒经页面地址 `#app-mcp-wake=`） | `pnpm --filter @app-mcp/web test` |
| 鸿蒙 ArkTS | `sdks/harmony/tests/conformance.test.cjs`（ArkTS 封装 + Node 版原生模块，不含真机运行时） | `node sdks/harmony/tests/run.cjs` |
| Swift | `sdks/swift/Tests/AppMcpTests/ConformanceTests.swift` | `swift test --filter ConformanceTests`（`PATH=~/.local/swift/usr/bin:$PATH`） |

汇总：`node conformance/matrix.mjs`。各 runner 都支持 `APP_MCP_CONFORMANCE_CASES=id1,id2` 与 `APP_MCP_FAKE_HOST`。
