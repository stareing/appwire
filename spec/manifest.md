# 静态能力清单 `app-mcp.json` 规范 v1

静态清单让 App 在**未运行**时也能向模型展示能力，并告诉 Host 如何启动（`launch`）与唤醒（`wake`）它。
类型与校验实现见 `crates/manifest`；网页清单由 `@app-mcp/build` 在构建时生成。

## 1. 示例

```jsonc
{
  "manifestVersion": 1,
  "appId": "shop",
  "name": "示例商城",
  "version": "1.2.0",
  "description": "一个用于演示的购物商城",
  "overview": {
    "summary": "演示用购物商城，可管理待办、浏览商品、操作购物车并结算",
    "body": "## 能力范围\n- 待办的增删改查\n- 商品搜索、购物车管理、结算\n\n## 典型流程\n- 结算：cart.state → cart.checkout\n\n## 不支持的操作\n- 退款：请引导用户到订单页面人工处理",
    "locale": "zh-CN"
  },
  "launch": {
    "web": [{ "type": "url", "href": "http://localhost:5173/" }],
    "windows": [{ "type": "uri", "scheme": "shop-app" },
                { "type": "aumid", "id": "Company.Shop_xxx!App" },
                { "type": "exe", "path": "%LOCALAPPDATA%\\Shop\\Shop.exe" }],
    "macos": [{ "type": "bundle", "id": "com.company.shop" }],
    "linux": [{ "type": "dbus", "name": "com.company.Shop" },
              { "type": "desktop", "file": "com.company.Shop.desktop" }]
  },
  "wake": {
    "web":     [{ "kind": "web-url", "target": "http://localhost:5173/" }],
    "windows": [{ "kind": "aumid", "target": "Company.Shop_xxx!App" },
                { "kind": "uri", "target": "shop-app" }],
    "macos":   [{ "kind": "uri", "target": "shop-app", "background": true }],
    "linux":   [{ "kind": "dbus", "target": "com.company.Shop", "background": true }],
    "android": [{ "kind": "android-intent", "target": "com.company.shop/dev.appmcp.WakeReceiver", "background": true }],
    "ios":     [{ "kind": "uri", "target": "shop-app" }]
  },
  "tools": [
    {
      "name": "orders.search",
      "title": "搜索订单",
      "description": "按关键词搜索历史订单",
      "inputSchema": { "type": "object", "properties": { "keyword": { "type": "string" } } },
      "annotations": { "readOnlyHint": true, "openWorldHint": false },
      "activation": "headless",
      "outputSchema": { "type": "array", "items": { "type": "object" } }
    }
  ],
  "resources": [
    { "name": "cart.state", "description": "当前购物车内容与总价", "annotations": { "audience": ["assistant"] } }
  ],
  "pages": [
    {
      "name": "cart",
      "title": "购物车",
      "description": "查看与结算购物车",
      "route": "/cart",
      "tools": [
        { "name": "cart.checkout", "description": "结算当前购物车", "inputSchema": { "type": "object" }, "surface": "view" }
      ]
    },
    {
      "name": "orders.detail",
      "title": "订单详情",
      "route": "/orders/:id",
      "params": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] },
      "navigable": false
    }
  ]
}
```

## 2. 字段

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `manifestVersion` | number | 是 | 当前为 `1` |
| `appId` | string | 是 | `[a-z][a-z0-9-]{0,62}`，与 SDK 握手的 `appId` 一致 |
| `name` | string | 是 | 显示名称 |
| `version` | string | 否 | App 版本 |
| `description` | string | 否 | App 简介，出现在 `apps.list` 中 |
| `overview` | object | 否 | App 总览：`summary`（≤ 100 字符）、`body`（Markdown，≤ 2000 字符）、`locale`；模型首次接触该 App 时由 Host 附带，规则见 spec/protocol.md 第 7 节 |
| `launch` | object | 否 | 各平台启动方式（冷启动，不带令牌），键为 `web` / `windows` / `macos` / `linux`，值为按顺序尝试的数组 |
| `wake` | object | 否 | 各平台唤醒描述（spec/lifecycle.md 第 5 节），键为 `web` / `windows` / `macos` / `linux` / `android` / `ios`，值为按顺序尝试的 WakeDescriptor 数组；规则见 2.2 节 |
| `tools` | array | 否 | 静态工具，结构同协议中的 `ToolInfo`：含可选 `annotations`（标准 MCP 工具注解）与 `outputSchema`（结果的 JSON Schema），语义见 spec/protocol.md 3.2；`risk` 为旧写法（与 `annotations` 同时出现时声明的注解字段优先）；可选 `surface`（`app` / `view`）与 `page`（所在页面名），语义见 spec/protocol.md 3.4；可选 `implements`（实现的标准意图，spec/intents.md）；可选 `cache`（结果缓存声明，spec/protocol.md 3.6） |
| `pages` | array | 否 | 页面目录（第 4c 项）：App 内各页面的说明、导航参数与页面内工具，结构见 2.3 节 |
| `events` | array | 否 | App 可发出的事件（第 16 项 N3），结构见 2.4 节；Agent 据此订阅，语义见 spec/protocol.md 3.5 |
| `resources` | array | 否 | 静态资源，结构同协议中的 `ResourceInfo`（含可选 `realtime`：需实时推送，被订阅时 App 保持连接，spec/lifecycle.md 第 13 节 B3；可选 `annotations`：标准 MCP 内容注解，spec/protocol.md 3.2；可选 `cache`：读取结果缓存声明，spec/protocol.md 3.6） |

### 2.1 `launch` 条目

| `type` | 平台 | 字段 |
|---|---|---|
| `url` | web | `href` |
| `uri` | windows / macos / linux | `scheme` |
| `aumid` | windows | `id` |
| `exe` | windows / linux | `path`（可含环境变量） |
| `bundle` | macos | `id` |
| `dbus` | linux | `name` |
| `desktop` | linux | `file` |

未知的 `type` 在解析时保留（原样透传），校验时给出警告而不是错误，便于向后兼容。

### 2.2 `wake` 条目（WakeDescriptor）

与 spec/lifecycle.md 第 5 节的 `WakeDescriptor` 同构：`{ "kind", "target"?, "background"? }`。
`launch` 用于冷启动一个**不知道本 SDK 唤醒协议**的进程；`wake` 声明 App 能接收一次性 `wakeToken`
（`handleWake`）并回连，Host 据此把挂起的调用派发给被唤醒的实例。

| `kind` | 适用平台 | `target` | Host 动作 |
|---|---|---|---|
| `web-url` | web | http(s) 地址，不含 `#` 片段 | 打开 / 聚焦 `<target>#app-mcp-wake=<token>` |
| `uri` | windows / macos / linux / android / ios | URI scheme，如 `shop-app`（不能是 `http`、`https`、`file`、`javascript`、`data`）| 打开 `<scheme>://app-mcp/wake?token=<token>`（macOS 用 `open -g`）|
| `aumid` | windows | AUMID，如 `Company.Shop_xxx!App` | `ActivateApplication(aumid, "app-mcp-wake:<token>")` |
| `apple-event` | macos | bundle id | 向该 bundle 发送 Apple Event |
| `dbus` | linux | 可激活的 D-Bus 名称，如 `com.company.Shop` | `org.freedesktop.Application.ActivateAction("app-mcp-wake", [token])` |
| `android-intent` | android | `<包名>/<接收器类名>`，如 `com.company.shop/dev.appmcp.WakeReceiver` | 显式广播 `dev.appmcp.action.WAKE`，token 为 extra |
| `none` | 任意 | 不需要 | 明确声明该平台不可唤醒（不回退到 `launch.web` 推导） |

- `background`（布尔，缺省 `false`）：能否不把窗口带到前台就唤醒；`web-url` 恒为前台，设为 `true` 时给出警告。
- 同一平台的数组按顺序尝试；运行时 `app/sleep` 上报的描述优先于清单。
- **Web 默认值**：未声明 `wake.web` 时，Host 把 `launch.web` 中第一个 `url` 视为 `{ "kind": "web-url", "target": <href> }`。
  该推导**仅在 Host 开启 `wake_from_launch`（命令行 `--wake-from-launch`）时生效**，默认关闭——避免模型调用静态工具时在开发机上意外打开浏览器；显式声明的 `wake` 始终生效。`@app-mcp/build` 默认会显式写出 `wake.web`，因此用构建插件生成清单的 Web 应用不受此开关影响。
  `@app-mcp/build` 也会显式写出这一条（可通过插件选项 `wake` 覆盖或关闭）。
- 解析顺序（Host）：运行时上报的 `wake` → 清单 `wake.<平台>` → 清单 `launch.<平台>`（冷启动新实例）。
- 未知 `kind` / 未知平台键原样保留，校验时给出警告；已知 `kind` 的字段类型错误（`target` 非字符串、`background` 非布尔）为错误。

### 2.3 `pages` 条目

页面目录描述"App 内有哪些页面、各页面上有哪些工具、能否由 Agent 导航过去"。导航请求 `app/navigate` 与工具的
`surface` / `page` 语义见 spec/protocol.md 3.4；Hub 如何披露页面目录、调用页面工具时如何导航见 spec/hub-api.md 3.14。

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `name` | string | 是 | 页面名 `[a-zA-Z0-9_.-]{1,64}`，清单内唯一；即 `app/navigate` 的 `page` |
| `title` | string | 否 | 显示标题 |
| `description` | string | 否 | 页面说明（给模型读） |
| `route` | string | 否 | App 内路由（如 `/orders/:id`），供 App 与构建工具使用；Host 不解析 |
| `params` | object | 否 | 导航参数的 JSON Schema，`type` 必须为 `"object"` |
| `tools` | array | 否 | 页面内的工具，结构同 `ToolInfo`（通常 `surface: "view"`）；可写 `page`，写了必须等于所在页面名；可写 `backgroundTool`（后台替代，spec/protocol.md 3.4） |
| `navigable` | boolean | 否 | 能否由 Agent 导航到该页面，缺省 `true`；`false` 时 Hub 不导航（只能由用户自己打开） |
| `activation` | string | 否 | 导航到该页面需要的激活方式（`headless` / `background` / `foreground`），同 `ToolInfo.activation` |

### 2.4 `events` 条目

结构同协议中的 `EventInfo`（spec/protocol.md 3.5）。

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `name` | string | 是 | 事件名 `[a-zA-Z0-9_.-]{1,64}`，清单内唯一（与工具名、资源名分属不同命名空间） |
| `description` | string | 是 | 面向模型：事件何时发生、载荷含义；不能为空字符串 |
| `payloadSchema` | object | 否 | 载荷的 JSON Schema（描述用，Hub 不校验） |

## 3. 校验规则

- `manifestVersion` 必须为 `1`。
- `appId` 格式合法，且不能是保留名 `apps`、`os`、`ax`、`host`。
- 工具名、资源名满足 `[a-zA-Z0-9_.-]{1,64}`，各自在清单内唯一；工具名在顶层 `tools` 与所有 `pages[].tools` 之间同样唯一
  （工具名在 App 内唯一）。页面内工具与顶层工具适用相同的工具规则（名称、`inputSchema`、`outputSchema`、`description`、appId 前缀警告）。
- `pages`：页面名满足 `[a-zA-Z0-9_.-]{1,64}` 且唯一；`description` 若给出不能为空字符串；`params` 若给出必须是 `type` 为 `"object"`
  的对象；页面内工具的 `page` 若给出必须等于所在页面名（以上违反为错误）。顶层工具的 `page` 指向未声明的页面给出警告。
- `events`：名称满足 `[a-zA-Z0-9_.-]{1,64}` 且唯一，`description` 非空，`payloadSchema` 若给出必须是对象（以上违反为错误）；
  名称以 `<appId>.` 开头给出与工具相同的前缀警告。
- 工具 `implements`（顶层与页面内工具相同）：每项须为 `<动词>@<主版本>` 形式、不重复、最多 4 项（违反为错误）；动词或版本不在
  spec/intents.md 词表中、或词表必填参数不在 `inputSchema.properties` 中给出警告。
- `cache`（工具与资源，顶层与页面内工具相同）：`ttlMs` 为 0 或超过 86 400 000 为错误；`ttlMs` 不是非负整数、`scope` 不是
  `private` / `shared` 时清单解析失败（同 `annotations` 字段类型错误）；
  工具的生效注解 `readOnlyHint` 不为 `true` 时给出警告（Hub 忽略写工具上的声明）。
- 工具 `inputSchema` 必须是对象且 `type` 为 `"object"`；`outputSchema` 若给出必须是对象（根类型不限）。
- 工具 `backgroundTool`（顶层与页面内工具相同）：名称不合法、指向自身、指向清单中 `surface` 不是 `app` 的工具为错误；指向清单中
  未声明的工具（只在运行时注册）、或声明在 `surface` 为 `app` 的工具上（无意义，Hub 忽略）给出警告。
- `annotations` 各字段类型不对（如 `readOnlyHint` 不是布尔、`audience` 取值不是 `user` / `assistant`）时清单解析失败。
- `description` 不能为空字符串。
- `overview.summary` 不能为空；`summary` 超过 100 字符、`body` 超过 2000 字符时给出警告（Host 会截断）。
- `wake`：除 `none` 外 `target` 必填且非空；`web-url` 必须是 http(s) 地址；`uri` 必须是合法 scheme；
  `android-intent` 必须是 `<包名>/<类名>`（以上违反为错误）。`kind` 与平台不匹配、`dbus` 名称不含 `.`、
  `web-url` 带 `#` 片段或 `background: true`、`none` 带 `target` 给出警告。

## 4. Host 加载规则（M1）

- 命令行 `--manifest <文件>`（可重复）与 `--manifest-dir <目录>`（默认 `~/.app-mcp/manifests`，读取其中所有 `*.json`）。
- 同一 `appId` 出现多次时，后加载的覆盖先加载的，并记录警告。
- 校验失败的清单跳过并记录错误，不影响其他清单。

## 5. 静态工具与运行时工具的关系

- `tools[].name` / `resources[].name` 是局部名（spec/protocol.md 3.1），不写 appId 前缀；以 `<appId>.` 开头时
  校验给出警告。
- App 未连接时：静态工具出现在 MCP 工具列表中（名称 `<appId>.<name>`），调用返回 `APP_DISCONNECTED`，
  错误信息中给出 `launch.web` 的地址等唤醒提示（M2 起改为自动唤醒）。
- `pages[].tools` 中的页面工具**不作为静态工具列出**，只进入页面目录：Hub 经 `apps.tools` / `apps.page` 渐进披露，
  调用时先导航到所在页面再派发（spec/protocol.md 3.4、spec/hub-api.md 3.14）。`app-mcp-codegen` 的原生意图 target
  （App Intents、AppFunctions、Windows App Actions、鸿蒙意图）只生成 `surface: "app"` 的顶层工具，`view` 工具跳过并给出警告；
  页面工具不参与代码生成。
- App 已连接时：以运行中实例实际注册的工具为准；静态工具中未注册的，在列表中保留但描述前加
  `[当前不可用]`，调用返回 `TOOL_NOT_FOUND`，信息说明需要先打开对应界面。
