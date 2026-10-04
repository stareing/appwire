# @app-mcp/harmony：鸿蒙 ArkTS SDK

把鸿蒙（HarmonyOS NEXT / OpenHarmony）App 内的业务动作注册为 MCP 工具；同一套 handler 还可以经
`crates/codegen` 生成的意图执行器接入系统意图框架（InsightIntent，小艺等系统入口）。

| 组成 | 位置 | 说明 |
|---|---|---|
| Node-API 原生模块 | `bindings/harmony` → `libs/<ABI>/libapp_mcp_harmony.so` | 用 [napi-ohos](https://crates.io/crates/napi-ohos)（napi-rs 的 OpenHarmony 分支）编译 `bindings/node/src/lib.rs` **同一份源码**，JS 形状与 `@app-mcp/node` 的原生模块逐字相同 |
| 类型声明 | `src/main/cpp/types/libapp_mcp_harmony/index.d.ts` | `.so` 的 ArkTS 类型（ohpm 依赖 `libapp_mcp_harmony.so`） |
| ArkTS 封装 | `Index.ets`、`src/main/ets/` | `AppMcp`（工具 / 资源 / scope / 生命周期）、`HarmonyAppMcp`（前后台、`Want` 唤醒）、`ToolCallError`、`ToolResult` |
| 意图代码生成 | `crates/codegen --target harmony-insight-intents` | `@InsightIntentEntry` 执行器 + `insight_intent.json` + ArkTS 参数类型与注册函数 |

要求：SDK 本身 API 12+（Node-API、`applicationStateChange`）；生成的意图执行器使用装饰器方式，需要 API 20+。

## 构建原生模块

需要 OpenHarmony SDK 的 native 组件（NDK）与 Rust 的 ohos 目标：

```bash
# NDK：DevEco Studio 自带（sdk/default/openharmony/native），或从 OpenHarmony 发布页下载
#   https://repo.huaweicloud.com/openharmony/os/6.0-Release/ohos-sdk-windows_linux-public.tar.gz（linux/native-*.zip）
rustup target add aarch64-unknown-linux-ohos x86_64-unknown-linux-ohos
OHOS_NDK_HOME=<SDK 根目录，其下有 native/> sdks/harmony/scripts/build-native.sh arm64-v8a x86_64
```

`build-native.sh` 交叉编译 `bindings/harmony`（产物在仓库根 `target/<target>/release/`），复制到本包
`libs/arm64-v8a/`（真机）与 `libs/x86_64/`（模拟器）；hvigor 打包 HAR 时随包发布。NDK 的
`<target>-clang` 包装脚本需在 `PATH` 中（脚本已处理），`bindings/harmony/.cargo/config.toml` 指定链接器与
C 编译器（ring）。宿主机的 `CFLAGS` 等编译器环境变量（如 conda）会被清除。

## 接入

把本目录作为 HAR 模块加入工程（或 `ohpm install <路径>`），在 `oh-package.json5` 中依赖 `@app-mcp/harmony`。

```ts
// entry/src/main/ets/entrystage/EntryStage.ets（module.json5 的 srcEntry 指向它）
import { AbilityStage } from '@kit.AbilityKit';
import { HarmonyAppMcp, ToolCallError } from '@app-mcp/harmony';

export default class EntryStage extends AbilityStage {
  onCreate(): void {
    // 第三个参数：module.json5 skills.uris 中声明的 scheme（用于唤醒，见下文）
    const mcp = HarmonyAppMcp.create(this.context, { appId: 'shop', appName: '示例商城' }, 'shopapp');
    mcp.tool<CartAddParams, Object | null>('cart.add', {
      description: '把商品加入购物车',
      inputSchema: '{"type":"object","properties":{"productId":{"type":"string"}},"required":["productId"]}',
      risk: 'write',
      handler: (input: CartAddParams): Object | null => {
        if (input.productId === '') {
          throw new ToolCallError('INVALID_INPUT', '商品 ID 为空');
        }
        if (!isLoggedIn()) {
          // Agent 收到 USER_ACTION_REQUIRED 并转告用户；reason / uri 可省略
          throw ToolCallError.userActionRequired('登录已过期，请在 App 内重新登录后重试', { reason: 'login', uri: 'shopapp://login' });
        }
        return addToCart(input.productId);
      },
    });
  }
}

// entry/src/main/ets/entryability/EntryAbility.ets
onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): void { HarmonyAppMcp.handleWant(want); }
onNewWant(want: Want, launchParam: AbilityConstant.LaunchParam): void { HarmonyAppMcp.handleWant(want); }
```

与 `@app-mcp/node` 的差异（ArkTS 语言约束所致）：

- 输入 schema 用 JSON 文本 `inputSchema: string`（ArkTS 不允许无类型的嵌套对象字面量）；handler 的 `input`
  是 `JSON.parse(参数)`，类型由泛型参数声明（建议用 codegen 生成的参数接口）；
- 连接状态 `ConnectionState` 是单一接口（`status` + 可选 `retryAt` / `reason` / `code`）；
- 取消用 `context.onCancel(listener)` / `context.isCancelled()`（运行时没有 `AbortSignal`）；
- 带 stateHints 的结果返回 `new ToolResult(data, ['cart'])`；
- 不支持惰性 handler（`load`）与 zod。

### 工具声明与结构化结果（可选，spec/protocol.md 3.2）

```ts
mcp.tool<OrderParams, Object | null>('order.submit', {
  description: '提交订单',
  annotations: { destructiveHint: true, idempotentHint: false, openWorldHint: true },
  outputSchema: '{"type":"object","properties":{"orderId":{"type":"string"}}}',
  handler: (input: OrderParams): ToolResult<Object | null> => new ToolResult<Object | null>(null, [], {
    status: 'pending',                 // 'done'（缺省）| 'pending' | 'partial' | 'noop'
    stateResource: 'order.status',     // pending 时可读取后续状态的资源名
    summary: '已提交，等待用户在 App 内确认',
    annotations: { audience: ['user'] },
  }),
});
```

- `annotations`（`ToolAnnotations`：`title` / `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`）是标准 MCP
  工具注解，原样转发给 Agent；与旧写法 `risk` 同时给出时声明的字段逐个优先，缺少的按 `risk` 推导。AppWire 不据此拦截或放行调用。
- `outputSchema`：结果的 JSON Schema 文本；根类型不是 `object` 时 Hub 包装为 `{result: …}`。
- 调用调度（spec/protocol.md 5.3，只在 SDK 内生效、不发给 Host）：`AppMcpOptions.maxConcurrentCalls`（同时执行的调用数，缺省 1）与
  `maxQueuedCalls`（排队中的调用数，缺省 64，0 = 不限；队列满时新调用以 `RATE_LIMITED` 拒绝，`data` 为 `{ scope: 'queue', limit }`）；
  工具可声明 `concurrency`（本工具同时执行的上限，缺省 / 0 = 不单独限制）与 `exclusive`（互斥组名，同组工具同一时刻至多一个在执行，
  如操作同一份文档的写工具），`update` 中给出 `null` 清除。
- 标准意图（spec/intents.md）：工具可声明 `implements: ['message.send@1']`（每项 `<动词>@<主版本>`，最多 4 项），Agent 用 `apps.intents`
  按动作找 App；格式不合法时注册抛错，`update` 中给出 `null` 清除。
- 结果缓存声明（spec/protocol.md 3.6）：只读工具与资源可声明 `cache: { ttlMs: 30000, scope: 'shared' }`，Hub 在 TTL 内复用结果（命中
  不唤醒 App）；写工具上的声明被忽略并记警告，`shared` 只用于与调用方无关的数据（缺省 `private`）。`ttlMs` 须为整数 1..=86400000，
  越界时注册 / 更新抛错（`INVALID_CONFIG`）；`update` 中给出 `null` 清除。
- 事件（spec/protocol.md 3.5）：`mcp.declareEvent({ name: 'order.shipped', description: '订单已发货', payloadSchema: '{"type":"object"}' })`
  声明（`payloadSchema` 为 JSON 文本，同名替换；已连接时随即同步给 Host，否则下次握手后同步，不触发连接），
  `mcp.emitEvent('order.shipped', { orderId: 'o1' } as Record<string, Object>)` 发出：已连接返回 `true`；未连接（休眠、断线、重连中）
  丢弃并返回 `false`，不缓存、不唤醒、不推迟空闲休眠，需要可靠送达请改用资源。未声明 / 名称不合法抛 `code` 为 `INVALID_NAME` 的错误，
  载荷不是对象或超过 8 KiB 抛 `INVALID_JSON`。`removeEvent(name)` 撤销。Agent 用内置工具 `apps.events.subscribe` / `apps.events` 订阅、取件。
- 用户正在操作（spec/protocol.md 5.3，只在 SDK 内生效、不发给 Host）：`mcp.setBusy(true / false)` 声明用户此刻正在 App 内操作
  （何时算由 App 决定，如输入框获得焦点、拖拽中），`isBusy()` 读取。期间写调用（生效注解不是 `readOnlyHint: true` 的工具）按
  `AppMcpOptions.busyPolicy` 处理：`'reject'`（缺省）以 `RATE_LIMITED` 拒绝（`data` 为 `{ scope: 'busy' }`，未执行）；`'queue'` 排队，
  `setBusy(false)` 后按到达顺序执行。只读调用与已开始的调用不受影响；`setBusyPolicy(policy)` 运行时修改。`beginBusy()` 开始一个作用域
  （可嵌套、按引用计数，`release()` 结束）：有效值 = 显式开关 OR 有未结束的作用域，两者互不清除，`isBusy()` 返回有效值。
- 返回普通数据即 `done` 结果；需要业务状态、摘要或内容标注（`ContentAnnotations`：`audience` / `priority` / `lastModified`）时返回
  `new ToolResult(data, stateHints, options)`（`ToolResultOptions`）。无返回值（`data` 为 `null` / `undefined` 且无 `summary`、
  状态 `done`）时 Hub 对模型输出固定文本"已完成"。
- `ToolHandle.update`：未提供（或 `undefined`）的字段保持不变，显式给出 `null` 清除该声明（`title` / `annotations` /
  `outputSchema` / `activation` 删除，`inputSchema` 变为无参数，`risk` 恢复 `write`，`enabled` 恢复 true；`description`
  不可清除）；给出值则整体替换该字段。

### 页面与导航（spec/protocol.md 3.4）

- 工具可声明 `surface`（`'app'` 缺省；`'view'` 依赖界面，应在页面 `onPageShow` 注册、`onPageHide` 注销）与 `page`（所在页面名）。
- `onNavigate: (request) => { router.pushUrl(...) }`（或 `mcp.setNavigationHandler(handler | null)`，在连接前设置）：Hub 调用不在当前
  页面的工具时先请求切换页面；回调在创建客户端的 ArkTS 线程上执行，返回（或 Promise 兑现）即完成，抛出
  `ToolCallError.navigationDenied(message)` 拒绝，其他异常按失败回复。
- 后台时：需要前台的导航立即以 `USER_ACTION_REQUIRED`（`reason: "foreground"`）返回、不调用回调（`navigateInBackground`
  缺省 `false`：应用不能自行回到前台；置为 `true`（选项或 `mcp.setNavigateInBackground(true)`）时交给回调，回调可抛出
  `ToolCallError.userActionRequired(message, { reason: 'foreground', uri })`，如发通知请用户点开）。后台也要能用的能力做成
  `app` 工具，或给 `view` 工具声明 `backgroundTool`（同一 App 中一个 `app` 工具的名称，App 在后台时 Hub 改调它）；
  详见 spec/protocol.md 3.4「后台与前台」。

### 线程

原生运行时在自己的线程上连接 Host，回调经 Node-API 线程安全函数投递到**创建 `AppMcp` 的 ArkTS 线程**的事件循环。
在 `AbilityStage` / `UIAbility` 中创建即为主线程：handler 可以直接读写 UI 状态（`AppStorage` 等），
耗时工作请交给 `taskpool`，完成后再返回结果。不要在 Worker 中创建后又在主线程使用。

### 生命周期与唤醒（spec/lifecycle.md）

`HarmonyAppMcp.create` 的默认值：

- 生命周期 `HarmonyAppMcp.defaultLifecycle(scheme)`：`on-demand` + `keep` + `sleepOnBackground: true`
  （spec/lifecycle.md 第 13 节 B1「平台默认」、B4）。启动时不连接（`dormant`）；首次进入前台、Host 唤醒（`handleWant`）、
  `wake()` / `connectNow()` 时连接；调用完成后只多留 2 s 合并窗口（在线时长由 Host 租约决定），连上后无调用经 `graceMs`
  （10 s）休眠；进入后台且无进行中的调用 / 持有 / 实时订阅时立即休眠，不等租约；
- 跟随应用前后台（`ApplicationContext.on('applicationStateChange')`）上报可见性；进入前台时以 `visible` 原因回连
  （在 `AbilityStage.onCreate` / `EntryAbility.onCreate` 中 `create` 时，首次进入前台即经此连接）；
- 给出 `wakeScheme` 时唤醒描述为 `uri` → `<scheme>://app-mcp/wake`；
- 实例标题缺省为 `appName`；日志写 hilog（domain `0xA3C0`，tag `app-mcp`）。

不申请长时任务、后台运行权限或 WakeLock：休眠后进程交给系统管理。

`options.lifecycle` 给出时**整体**使用它（不与平台默认合并，未给出的字段取原生层默认：`persistent`、空闲 60 s 等）；
只改个别字段时先取默认再修改：

```ts
const lifecycle = HarmonyAppMcp.defaultLifecycle('shopapp');
lifecycle.mergeWindowMs = 5000;
HarmonyAppMcp.create(this.context, { appId: 'shop', appName: '示例商城', lifecycle: lifecycle }, 'shopapp');
```

`LifecycleOptions` 的 4e 开关（缺省值即原生层默认值）：

| 字段 | 缺省 | 含义 |
|---|---|---|
| `hostAbsentRetries` | 3 | `idle` / `on-demand` 下连续多少次"Host 不在"后转 `dormant`；0 = 一直重连 |
| `mergeWindowMs` | 2000 | 调用 / 资源读取后的合并窗口（毫秒） |
| `sleepOnBackground` | `false`（`defaultLifecycle` 为 `true`） | 进入后台且空闲时立即休眠 |
| `legacyTimers` | `false` | 一次性回退到 4e 之前的定时器行为 |

`AppMcpOptions.heartbeat`：`'auto'`（缺省；鸿蒙沙箱上的回环地址按远程处理，约每 15 s 单向心跳）/ `'always'` / `'off'`
（在设备上嵌入 Hub 时可用）。资源声明 `realtime: true`（缺省 `false`）表示模型在等待其变化：被 Host 订阅时阻止休眠、
休眠中变化时回连推送；普通状态（购物车、列表）不要声明。

`AppMcpOptions.callDedup`：`{ ttlMs?, maxEntries? }`（缺省 300000 ms / 64 条，任一为 0 关闭）——同一 `callId` 在有效期内再次到达时
重放首次结果、不再执行 handler（spec/protocol.md 3.3），命中时记一条警告日志。
handler 上下文 `ToolContext.idempotencyKey`：Agent 给出的幂等键（原样，跨重试不变；没有时为 `undefined`，spec/protocol.md 3.3），
App 可作为业务层去重键或传给后端。
资源可声明 `annotations`（MCP 内容注解 `audience` / `priority` / `lastModified`，Hub 放到 `resources/list` 的资源注解上）；
`read` 抛出 `ToolCallError`（含 `ToolCallError.userActionRequired`）时类别与详情（`reason` / `uri`）原样交给 Host，与工具一致。

`HarmonyAppMcp.handleWant(want)` 识别两种唤醒：`want.uri` 为 `<scheme>://app-mcp/wake?token=<令牌>`，或
`want.parameters['app-mcp-wake']` 为令牌。要让 URI 拉起 App，在入口 Ability 的 `skills` 中声明：

```json5
"skills": [{ "actions": ["ohos.want.action.viewData"], "uris": [{ "scheme": "shopapp", "host": "app-mcp", "path": "wake" }] }]
```

开发调试时 Host 运行在电脑上：`hdc rport tcp:7717 tcp:7717` 把设备的 7717 端口反向转发到电脑（与 Android 的
`adb reverse` 相同），SDK 默认端点即 `ws://127.0.0.1:7717/app`：未指定 `hostUrl` 时由原生层按
spec/protocol.md 1.3 解析——鸿蒙目标（`target_env = "ohos"`）与 Android 一样是应用沙箱，没有默认本地 IPC 端点、
不读登记文件，因此落到回环 WebSocket；也可用 `hostUrl` 显式指定。Host 目前没有内置鸿蒙唤醒器；可用 Host 的
`--waker '{"exec":[...]}'` 自定义唤醒命令，在其中执行 `hdc shell aa start -b <bundle> -a EntryAbility -U <唤醒 URI>`。

## 意图框架（InsightIntent）

```bash
app-mcp-codegen --manifest app-mcp.json --target harmony-insight-intents --out entry/src/main \
  [--module Shop] [--intent-domain ToolsDomain] [--ability EntryAbility]
```

生成（相对 `src/main/`）：

| 文件 | 内容 |
|---|---|
| `ets/appmcp/<Module>Tools.ets` | 参数类型、`<Module>ToolHandlers` 接口、`register<Module>Tools(appMcp, handlers)` |
| `ets/appmcp/<Module>InsightIntents.ets` | `<Module>IntentRuntime.handlers`（注入点）、结果码 `<Module>IntentCode` |
| `ets/insightintents/<Module><Tool>Intent.ets` | 每个工具一个 `@InsightIntentEntry` 执行器 |
| `resources/base/profile/insight_intent.json` | `insightIntentsSrcEntry` 列出各执行器；已有该文件时手动合并 |

同一个 handler 实现同时服务 MCP 与系统意图：

```ts
const handlers = new ShopHandlers();          // implements ShopToolHandlers
ShopIntentRuntime.handlers = handlers;        // 意图（后台执行时系统经 Call 拉起 UIAbility，不经过页面）
registerShopTools(mcp, handlers);             // MCP 工具
```

映射规则：意图名 `<Module><Tool>`；`parameters` 由类型模型重新生成（去掉 `format` / `$ref` / 组合关键字，保证
构建工具的 ajv 能编译）；需要确认的风险等级（destructive / payment / os-sensitive）与 `activation: foreground`
用前台执行模式，其余用后台执行模式；参数名不是标识符（如 `is-urgent`）或与执行器基类成员同名的工具不生成意图
（给出警告，仍可作为 MCP 工具）。执行结果 `IntentResult<string>`：`code` 0 成功，`result` 为返回值的 JSON；
失败时 `result` 为 `{"kind","message"}`。

## 控件兜底（`ui.*` 工具，第 4c 项 H）

开发者显式开启后注册 `ui.outline` / `ui.click` / `ui.fill` / `ui.press` / `ui.scroll` / `ui.read`（行为见
`spec/ui-fallback.md`）。兜底作用于 ArkUI 检查器树中的组件，**不截图、不按坐标点击**；默认关闭，建议只在调试包或用户
明确开启时使用。

```ts
import { HarmonyUiFallback, MCP_DECLARED_PROPERTY, UIContextDriver } from '@app-mcp/harmony';

// EntryAbility.onWindowStageCreate：loadContent 完成后（主线程）
const ui = windowStage.getMainWindowSync().getUIContext();
const fallback = HarmonyUiFallback.enable(mcp, [new UIContextDriver(ui, '示例商城')],
  { applicationContext: this.context.getApplicationContext() });
// fallback.addWindow(new UIContextDriver(subWindow.getUIContext(), '设置'))；fallback.close() 注销全部兜底工具

// 页面：需要被点击的组件设置 id；已有语义工具的组件标注工具名，大纲中显示 [已声明：cart.clear]
Button('清空').id('clear').customProperty(MCP_DECLARED_PROPERTY, 'cart.clear')
```

| | ArkUI 实现 |
|---|---|
| 控件树 | 每次调用时 `UIContext.getFilteredInspectorTree()`（JSON：`$type` / `$ID` / `$rect` / `$attrs` / `$children`）；按组件类型分类（Button、Toggle、Checkbox、Radio、TextInput / TextArea / Search、Select、Slider、Hyperlink、MenuItem 等），绑定了 onClick 的其他组件（`FrameNode.getInteractionEventBindingInfo(ON_CLICK)`，API 19+）作为按钮列出 |
| 窗口 / 模态 | `enable` / `addWindow` 传入的每个 UIContext 为 `window`；同一窗口中最上层的对话框类覆盖层（`Dialog` / `AlertDialog` / `ActionSheet` / `MenuWrapper` / `SheetWrapper` / `Popup` 等）为 `dialog` 并遮挡其余内容 |
| 引用 | 窗口序号 + 检查器 `$ID`（组件 uniqueId）；List / Grid / WaterFlow 下的组件按指纹核对（复用后作废） |
| 可见 | `visibility` 不为 Hidden / None、`opacity` 不为 0、`$rect` 非空且与窗口及滚动容器的矩形相交 |
| 启用 | 检查器属性 `enabled` |
| 点击 | `sendEventByKey(组件 id, 10, '')`：**组件必须设置 `.id()`**，否则返回 unsupported |
| 填写 | 复选框 / 开关 / 单选框：状态不同时点击；**文本框、下拉框、滑块不支持**（ArkUI 没有从组件外写入的接口，返回 unsupported） |
| 滚动 | **不支持**（Scroller 由页面持有，没有从组件外滚动任意容器的接口）：可见控件不带 direction 时返回空变化，其余返回 unsupported |
| 已声明 | `.customProperty(MCP_DECLARED_PROPERTY, 工具名)`（经 `FrameNode.getCustomProperty` 读取）；标在外层非控件组件上时给子树中唯一的控件 |
| 可见窗口 | `applicationStateChange` 前台（传入 `applicationContext` 时），或 `setVisible` 手动指定；缺省开启时视为可见 |
| 按键 | Tab / Shift+Tab：按树序在设置了 id 的可获焦组件间 `FocusController.requestFocus(id)`；Enter / Space / Escape：`UIContext.dispatchKeyEvent(uniqueId, 按下 + 抬起)` 交给目标 / 焦点组件（Escape 交给最上层对话框），Enter / Space 未被消费时点击非文本控件 |
| 密码类 | TextInput / TextArea 的 `type` 为 Password 类（`InputType.Password` / `NEW_PASSWORD` / `NUMBER_PASSWORD`） |
| 必填 | 不支持（检查器没有对应属性） |

未知项（需在设备上核对）：检查器 JSON 的字段名与对话框覆盖层的组件类型名（d.ts 只声明返回 JSON 字符串）；`$ID` 是否等于
`getFrameNodeByUniqueId` 的 uniqueId；`sendEventByKey` 在 global.d.ts 中标注 `@test`，正式包中是否可用；
`dispatchKeyEvent` 的 Enter 是否触发 TextInput 的 `onSubmit`、Escape 是否关闭对话框。

## 验证

```bash
node sdks/harmony/scripts/arkts-check.cjs      # 用 SDK 自带 ohos-typescript 做类型检查 + ArkTSLinter（与 hvigor 相同的检查器）
node sdks/harmony/tests/run.cjs                # 封装层单元测试：.ets 转译后在 Node 上用假原生模块运行
bash crates/codegen/scripts/verify.sh harmony-insight-intents   # 生成代码的 ArkTS 检查 + 意图装饰器校验（ets-loader 规则）
```

`OHOS_SDK_ETS` 指定 SDK 的 `ets` 目录（缺省 `~/sdk/ohos/sdk/ets`）。

默认端点在 ohos 目标上的解析（`app-mcp-protocol` 的 cfg 单元测试）可在 x86_64 Linux 上直接运行：静态链接 NDK 的 musl
后测试程序不依赖设备（动态链接版需要设备上的加载器）：

```bash
PATH="$OHOS_NDK_HOME/native/llvm/bin:$PATH" \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_OHOS_LINKER=x86_64-unknown-linux-ohos-clang \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_OHOS_RUSTFLAGS="-C target-feature=+crt-static" \
cargo test -p app-mcp-protocol --lib --target x86_64-unknown-linux-ohos --target-dir target/ohos-static
```

未验证（无 DevEco / hvigor 与鸿蒙设备）：hvigor 打包 HAR、`.so` 在 ArkVM 中加载与 Node-API 行为（线程安全函数、
weak 引用）、`applicationStateChange` 前后台、`Want` 唤醒、意图在系统入口中的注册与执行。
