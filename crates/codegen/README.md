# app-mcp-codegen

从静态清单 `app-mcp.json`（spec/manifest.md）生成各平台原生意图声明与各语言的类型化接口。

```bash
app-mcp-codegen --manifest app-mcp.json --target <target> --out <dir> \
  [--package <name>] [--module <name>] [--intent-domain <垂域>] [--ability <UIAbility>] \
  [--app-intents-extension] [--app-intents-execution-targets] [--app-intents-cancellable] [--standard-intents]
```

| target | 输出 |
|---|---|
| `swift-app-intents` | Apple App Intents（每个工具一个 `AppIntent`）+ Swift 类型文件 |
| `kotlin-appfunctions` | Android AppFunctions + Kotlin 类型文件 |
| `windows-app-actions` | Windows App Actions 定义 JSON + C# 处理骨架 |
| `harmony-insight-intents` | 鸿蒙意图框架（InsightIntent）+ ArkTS 类型 |
| `typescript`、`csharp`、`swift`、`kotlin`、`python`、`dart` | 参数类型 + handler 接口 |

原生意图 target 只生成 `surface: "app"` 的顶层工具（spec/manifest.md）。降级与跳过都以警告输出到 stderr。

`harmony-insight-intents` 的自定义意图：构建工具（ets-loader）要求执行器属性类型与意图 `parameters` 一致，因此顶层参数在意图中
改变表示，执行器校验后转换回 `<Module>ToolHandlers` 的参数类型（类型化接口 `<Module>Tools.ets` 不变）：整数在 `parameters` 中为
`number`（执行时校验整数）、枚举属性为 `string`（校验取值）、对象为生成的 `@InsightIntentEntity` 类（校验必填属性）、字典与原始
JSON 为 JSON 文本（校验并解析）；校验失败返回 `INVALID_INPUT`。嵌套位置保持原类型与 schema。

## swift-app-intents 的可选输出

三个选项都只用于 `--target swift-app-intents`，缺省关闭；关闭时输出与原来完全相同。依据与背景见 spec/naming.md 4.5
（第三方 App 不能调用其他 App 的 App Intents，App Intents 只由 Siri、快捷指令等系统入口执行）。

- `--app-intents-extension`：App 未运行时也能由 **App Intents 扩展**执行 intent。输出布局：

  ```
  <Module>Intents/Package.swift                              共享 Swift 包（iOS / macOS 26）
  <Module>Intents/Sources/<Module>Intents/<Module>Tools.swift      参数类型 + handler 协议
  <Module>Intents/Sources/<Module>Intents/<Module>AppIntents.swift 各 AppIntent、运行时、<Module>IntentsPackage
  <Module>IntentsExtension/<Module>IntentsExtension.swift    扩展入口（@main AppIntentsExtension）
  App/<Module>AppIntentsPackage.swift                        App 侧的 AppIntentsPackage 声明
  ```

  把共享包同时链接到 App target 与扩展 target。handler 只写一次：在两个 target 都能访问的模块中实现

  ```swift
  enum ShopIntentHandlersProvider: ShopIntentHandlersProviding {
      static func makeHandlers() -> any ShopToolHandlers { MyHandlers() }
  }
  ```

  扩展入口已调用 `ShopIntentRuntime.configure(ShopIntentHandlersProvider.self)`，App 的 `init` 中调用同一行即可；
  handler 在所在进程首次执行 intent 时才构造。扩展布局下若有 `activation: foreground` 的工具且未加下一个选项，会给出警告。
- `--app-intents-execution-targets`：按 `activation` 声明 `allowedExecutionTargets`——`foreground` → `.main`，
  `background` / `headless` → `[.main, .appIntentsExtension]`。该 API 从 iOS / macOS 27 起提供，以 `@available` 限定。
- `--app-intents-cancellable`：intent 遵循 `CancellableIntent`，`perform()` 经 `withIntentCancellationHandler` 执行
  （iOS / macOS 26.4 起，以 `#available` 限定，低版本走原路径）；取消原因（`timeout` / `userCancelled` / `other`）经
  `<Module>IntentRuntime.onCancel { tool, reason in … }` 通知 App，handler 所在 Task 同时被取消。

**未验证**：本机没有带 iOS SDK 的 Xcode，可选输出只对照 `scripts/stubs/AppIntents.swift`（按 Apple 文档签名写的桩模块）
做了类型检查，未在真实 AppIntents 框架上编译；首次接入请在 Xcode 中确认（spec/naming.md U-22）。

## 标准意图的系统意图版本（`--standard-intents`）

用于 `swift-app-intents`、`kotlin-appfunctions`、`harmony-insight-intents`，缺省关闭（关闭时输出不变）。为声明了 `implements`
（spec/intents.md）、动词在词表中且 `inputSchema` 兼容的工具，在自定义意图之外另生成平台的系统意图版本，调用复用同一
`<Module>ToolHandlers`。各平台能生成哪些动词见 spec/intents.md 第 3 节；不能生成的、同一动词的后续工具都以警告说明。

- **Android**（`kotlin-appfunctions`）：`<Module>StandardIntents.kt`（`parse(intent)` → `Matched(call)` / `Invalid(tool, reason)` /
  `Unrecognized`，`call.risk` 照清单）与 `<Module>StandardIntentFilters.xml`（把其中的 `<intent-filter>` 复制进 App 自己的
  `android:exported="true"` Activity，在 `onCreate` / `onNewIntent` 调 `parse`）。`ACTION_SEND` 带 `EXTRA_STREAM` 归 `file.share`，
  不带归 `message.send`。Intent 可能来自任意 App：有副作用的工具先按 `call.risk` 确认再调用。`calendar.create` 需要 API 26+。
- **Apple**（`swift-app-intents`）：`<Module>StandardIntents.swift`（`@AppIntent(schema:)`）。目前只有 `link.open` →
  `.browser.openURLInTab`（iOS 18 / macOS 15，仅快捷指令），需 App 提供 `@AppEntity(schema: .browser.tab)` 的 `<Module>BrowserTab`；
  其余 schema（iOS 27）要求 App 实体，只给警告。与 `--app-intents-extension` 同用时文件放进共享包。**未在真实 AppIntents 框架上编译验证。**
- **鸿蒙**（`harmony-insight-intents`）：`media.play` → `PlayVideo` / `PlayAudio` / `PlayMusicList`（按工具 `kind` 枚举选择；系统只给
  实体 ID，App 在 AbilityStage 中设置 `<Module>StandardIntents.mediaResolver` 把实体解析为工具参数）、`navigation.start` → `StartNavigate`。
  执行器在 `ets/insightintents/standard/`，并追加到 `insight_intent.json`；`--ability` 生效，`--intent-domain` 不生效（垂域由标准意图固定）。
  上线还需华为意图框架的接入申请与审核；未经 hvigor 打包与小艺实际调用验证。

## 弃用声明（spec/protocol.md 3.7）

工具级 `deprecated {message, replacement?, until?}` 与参数级 `deprecated: true` 映射为各语言的弃用标注；消息为 `message`，有
`replacement` / `until` 时追加「（改用工具 X；计划于 YYYY-MM-DD 移除）」，按目标语言转义。参数级标注的消息固定为
「参数已弃用（inputSchema 中 deprecated: true）」。生成代码自身（分派、初始化器、意图）调用弃用成员时局部抑制警告，App 的
实现与直接调用处照常收到提示。没有任何弃用声明时输出不变。

| target | 工具级（handler 方法） | 参数级（字段） |
|---|---|---|
| `typescript` | JSDoc `@deprecated` | JSDoc `@deprecated` |
| `csharp` | `[Obsolete("…")]`；分派处 `#pragma warning disable CS0618` | 可选属性 `[Obsolete]`；必填属性只写文档（`required` 成员标 `[Obsolete]` 是 CS9042 错误） |
| `swift` | `@available(*, deprecated, message:)`；分派经 `<Module>DeprecatedToolCaller` 转发 | 计算属性带 `@available`，值存于私有存储属性（初始化器不产生警告） |
| `kotlin` | `@Deprecated("…")`（不生成 `ReplaceWith`：替代工具参数类型不同）；分派 `@Suppress("DEPRECATION")` | 构造属性 `@Deprecated` |
| `python` | docstring「已弃用：…」 | docstring / 注释 |
| `dart` | `@Deprecated("…")` | 字段 `@Deprecated`（同一库内使用不报告） |

Python 不用 `warnings.deprecated`（3.13+）或 `typing_extensions`（新的运行时依赖），生成文件只依赖 3.11+ 标准库。

原生意图 target 中弃用的工具**照常生成**（App 仍需响应系统入口）：

- `swift-app-intents`：intent 结构体加 `@available(*, deprecated, message:)`，不再列入 App Shortcuts；弃用参数在 `@Parameter` 前注释。
  App Intents 的声明（`title`、`IntentDescription`）不变。
- `kotlin-appfunctions`：`@AppFunction` 方法加 `@Deprecated("…")`——AppFunctions 编译器把消息写进函数元数据
  （`AppFunctionDeprecationMetadata`、XML `<deprecation>`，alpha12 实测）；参数只注释。
- `windows-app-actions`：Action 定义 JSON 不加字段（无资料可核实），提供者分派处注释并局部关闭 CS0618；agent connector
  的 `inputSchema` 原样带参数级 `deprecated`。
- `harmony-insight-intents`：意图装饰器没有弃用字段（ets-loader `allowFields`），执行器只写注释；`<Module>Tools.ets` 用 `@deprecated`。

## 测试与验证

```bash
cargo test -p app-mcp-codegen                      # 快照：tests/snapshots/<target>/，UPDATE_SNAPSHOTS=1 更新
bash crates/codegen/scripts/verify.sh [target ...] # 用各语言真实编译器检查输出；缺少工具链的步骤记为 SKIP
bash crates/codegen/scripts/verify.sh swift-app-intents swift-app-intents-extension
bash crates/codegen/scripts/verify.sh kotlin-appfunctions-standard-intents   # Android 系统意图：manifest 合并 + Robolectric
bash crates/codegen/scripts/verify.sh deprecated   # tests/fixtures/deprecated.json：各语言弃用标注在严格选项下编译运行
```
