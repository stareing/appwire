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
      "risk": "read",
      "activation": "headless"
    }
  ],
  "resources": [
    { "name": "cart.state", "description": "当前购物车内容与总价" }
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
| `tools` | array | 否 | 静态工具，结构同协议中的 `ToolInfo` |
| `resources` | array | 否 | 静态资源，结构同协议中的 `ResourceInfo` |

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

## 3. 校验规则

- `manifestVersion` 必须为 `1`。
- `appId` 格式合法，且不能是保留名 `apps`、`os`、`ax`、`host`。
- 工具名、资源名满足 `[a-zA-Z0-9_.-]{1,64}`，各自在清单内唯一。
- 工具 `inputSchema` 必须是对象且 `type` 为 `"object"`。
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

- App 未连接时：静态工具出现在 MCP 工具列表中（名称 `<appId>.<name>`），调用返回 `APP_DISCONNECTED`，
  错误信息中给出 `launch.web` 的地址等唤醒提示（M2 起改为自动唤醒）。
- App 已连接时：以运行中实例实际注册的工具为准；静态工具中未注册的，在列表中保留但描述前加
  `[当前不可用]`，调用返回 `TOOL_NOT_FOUND`，信息说明需要先打开对应界面。
