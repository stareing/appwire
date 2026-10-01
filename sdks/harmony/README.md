# @app-mcp/harmony：鸿蒙 ArkTS SDK

把鸿蒙（HarmonyOS NEXT / OpenHarmony）App 内的业务动作注册为 MCP 工具；同一套 handler 还可以经
`crates/codegen` 生成的意图执行器接入系统意图框架（InsightIntent，小艺等系统入口）。

| 组成 | 位置 | 说明 |
|---|---|---|
| Node-API 原生模块 | `bindings/harmony` → `libs/<ABI>/libapp_mcp_harmony.so` | 用 [napi-ohos](https://crates.io/crates/napi-ohos)（napi-rs 的 OpenHarmony 分支）编译 `bindings/node/src/lib.rs` **同一份源码**，JS 形状与 `@app-mcp/node` 的原生模块逐字相同；另导出 `defaultHostUrl()` |
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

### 线程

原生运行时在自己的线程上连接 Host，回调经 Node-API 线程安全函数投递到**创建 `AppMcp` 的 ArkTS 线程**的事件循环。
在 `AbilityStage` / `UIAbility` 中创建即为主线程：handler 可以直接读写 UI 状态（`AppStorage` 等），
耗时工作请交给 `taskpool`，完成后再返回结果。不要在 Worker 中创建后又在主线程使用。

### 生命周期与唤醒（spec/lifecycle.md）

`HarmonyAppMcp.create` 的默认值：

- 生命周期 `idle` + `keep`（移动端默认），空闲后与 Host 握手休眠，释放连接与运行时线程；
- 跟随应用前后台（`ApplicationContext.on('applicationStateChange')`）上报可见性；回到前台时以 `visible` 原因回连；
- 给出 `wakeScheme` 时唤醒描述为 `uri` → `<scheme>://app-mcp/wake`；
- 实例标题缺省为 `appName`；日志写 hilog（domain `0xA3C0`，tag `app-mcp`）。

不申请长时任务、后台运行权限或 WakeLock：休眠后进程交给系统管理。

`HarmonyAppMcp.handleWant(want)` 识别两种唤醒：`want.uri` 为 `<scheme>://app-mcp/wake?token=<令牌>`，或
`want.parameters['app-mcp-wake']` 为令牌。要让 URI 拉起 App，在入口 Ability 的 `skills` 中声明：

```json5
"skills": [{ "actions": ["ohos.want.action.viewData"], "uris": [{ "scheme": "shopapp", "host": "app-mcp", "path": "wake" }] }]
```

开发调试时 Host 运行在电脑上：`hdc rport tcp:7717 tcp:7717` 把设备的 7717 端口反向转发到电脑（与 Android 的
`adb reverse` 相同），SDK 默认端点即 `ws://127.0.0.1:7717/app`。Host 目前没有内置鸿蒙唤醒器；可用 Host 的
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

## 验证

```bash
node sdks/harmony/scripts/arkts-check.cjs      # 用 SDK 自带 ohos-typescript 做类型检查 + ArkTSLinter（与 hvigor 相同的检查器）
node sdks/harmony/tests/run.cjs                # 封装层单元测试：.ets 转译后在 Node 上用假原生模块运行
bash crates/codegen/scripts/verify.sh harmony-insight-intents   # 生成代码的 ArkTS 检查 + 意图装饰器校验（ets-loader 规则）
```

`OHOS_SDK_ETS` 指定 SDK 的 `ets` 目录（缺省 `~/sdk/ohos/sdk/ets`）。

未验证（无 DevEco / hvigor 与鸿蒙设备）：hvigor 打包 HAR、`.so` 在 ArkVM 中加载与 Node-API 行为（线程安全函数、
weak 引用）、`applicationStateChange` 前后台、`Want` 唤醒、意图在系统入口中的注册与执行。
