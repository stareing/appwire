# 进程内控件兜底（`ui.*` 工具）规范

本文件是"没有声明工具的界面"兜底操作的唯一定义（第 4c 项 H）：工具集、参数、大纲 / 引用 / 结果格式、执行前核对、错误与安全规则。
各实现只写平台映射（见第 8 节与各 SDK README），不重复定义格式。

实现：网页 `@app-mcp/inspect`（`attachInspect`）、Flutter `app_mcp_flutter`（`McpUiFallback`）、WPF `AppMcp.Wpf`
（`WpfUiFallback`）。后续平台（Android View / Compose、WinUI 3、UIKit、Qt、鸿蒙）沿用本文件。

## 1. 定位

- **能力超集中的最低一级**：语义工具（`McpTool` / `RegisterTool` / `useTool`）> 声明式 > 系统意图 > 本兜底 > 进程外无障碍树。
  只在开发者**显式开启**时注册（默认关闭），建议只在开发环境或用户明确开启时使用。
- **只作用于控件对象**：模型只看到"短引用 + 角色 + 名称 + 值 + 状态"，动作经框架的控件 / 无障碍 / 语义接口直接作用于控件；
  **不截图、不按坐标点击**（CLAUDE.md"微内核范围"）。
- 纯 SDK 侧能力：协议、核心与 Hub 不感知，工具与普通工具一样经 `tools/changed` 同步。
- 不在后台观察界面：所有内容都在工具被调用时计算；不调用就没有开销（Flutter 的语义树例外，见 8.2）。

## 2. 工具

名称前缀可配，缺省 `ui`。原生 SDK 的全部工具以 `surface: "view"`（spec/protocol.md 3.4）注册，且**只在 App 有可见窗口时启用**
（全部窗口隐藏 / 最小化、进入后台时禁用，对 Host 即注销）；不要求窗口获得焦点（Agent 通常在另一个窗口中）。注解如实声明：

| 工具 | 参数 | 注解 | 说明 |
|---|---|---|---|
| `ui.outline` | `{query?, within?, limit?}` | `readOnlyHint: true`（risk read） | 可见的可交互 / 有意义控件大纲（第 4 节） |
| `ui.click` | `{ref}` | `readOnlyHint: false`（risk write） | 激活控件（按钮的点击、复选框的切换、选项的选中、可展开项的展开 / 收起） |
| `ui.fill` | `{ref, value}` | `readOnlyHint: false` | 填写文本框；复选框 / 开关传 `true` / `false`；下拉框 / 列表传选项文本 |
| `ui.press` | `{ref?, key}` | `readOnlyHint: false` | 按键（`ref` 缺省为当前焦点控件），见 2.1 |
| `ui.scroll` | `{ref, direction?}` | `readOnlyHint: false` | 无 `direction`：把控件滚动到可见；有 `direction`（`up` / `down` / `left` / `right`，查看方向：`down` = 向下翻看更多内容）：滚动 `ref`（或其所在的滚动区）一页 |
| `ui.read` | `{ref?, maxChars?}` | `readOnlyHint: true`（risk read） | 控件（缺省为全部窗口）的可见文本，折叠空白，缺省 1000 字、上限 20000 |

- `ref` / `within`：引用字符串，匹配 `^e[1-9]\d*$`（第 3 节）。`limit`：1–500 的整数，缺省 60（可配）。
  `query`：按名称、角色、值、所在分组模糊过滤，空格分隔多个词、全部匹配、不区分大小写。
- 参数不合法 → `INVALID_INPUT`。所有工具 `additionalProperties: false`。
- 网页 `@app-mcp/inspect` 另有 `ui.submit`（`requestSubmit`）与可选的 `ui.eval`（`allowScript: true`，destructive），且以 `surface: "app"`
  注册（页面关闭即断开）；`ui.scroll` 只支持滚动到可见（不接受 `direction`）。

### 2.1 按键

`key` 为单个键名，可带修饰键前缀（`Shift+Tab`、`Control+a`）。所有实现至少支持：

| 键 | 行为 |
|---|---|
| `Enter` | 文本框：提交（框架的"完成 / 提交"动作或默认按钮）；其他控件：激活 |
| `Escape` | 关闭 / 取消（对话框的取消按钮、框架的 Dismiss） |
| `Tab` / `Shift+Tab` | 焦点移到下一个 / 上一个控件 |
| 空格 `" "` / `Space` | 激活焦点控件 |

不支持的键 → `INVALID_INPUT`（`message` 列出支持的键）。输入文本请用 `ui.fill`。

## 3. 引用

- 形如 `eN`（N 从 1 递增），在同一个兜底实例的生命周期内稳定：同一控件对象多次出现在大纲中引用不变。
- **弱映射**：引用 → 控件对象不阻止回收（网页 `WeakRef`、Flutter `WeakReference`、.NET `WeakReference`）。
- 控件不在界面上（已移除 / 已回收 / 所在窗口已关闭）时引用失效：
  `INVALID_INPUT`，`message = "引用 e12 已失效，请重新调用 ui.outline"`，`data.ref = "e12"`。
- 框架会重建或复用控件对象时（Flutter 语义节点），引用另记**指纹**（角色 + 名称 + 所在分组）：
  - 原对象不在时按指纹在当前界面中**唯一**匹配，匹配到即沿用该引用；
  - 原对象还在但指纹变了（框架把节点复用给了另一个控件，或控件改了名称）时旧引用作废、分配新引用——宁可失效也不指向别的控件。
- 被模态层从控件树中移除的控件（Flutter 的 `BlockSemantics`）在模态层打开期间按失效处理，关闭后原引用恢复可用。

## 4. 大纲

### 4.1 结果

```jsonc
{
  "text": "…",          // 每行一个控件 / 分组 / 标题的文本大纲（4.2）
  "items": [ /* 4.3 */ ],
  "total": 23,          // 匹配 query 的控件总数
  "remaining": 3,       // 超过 limit 未列出的控件数（没有时省略）
  "hint": "…"           // 有已声明控件时的提示（没有时省略）
}
```

### 4.2 文本行

```
» e1 窗口「示例商城」
  e2 按钮「清空」 [已声明：cart.clear]
  e3 按钮「结算」 disabled
  » e4 对话框「确认支付」
    ## 支付方式
    e5 单选框「余额」 checked
    e6 输入框「备注」= "尽快发货" (必填)
    e7 密码框「支付密码」= "••••"
```

- 控件行：`{ref} {标签}「{名称}」`；有值时接 `= "{值}"`；链接接 `→ {目标}`；有状态时接空格分隔的状态；必填接 ` (必填)`；
  已声明接 ` [已声明：{工具名}]`。名称为空时只写标签。空白折叠为一个空格；名称截断到 40 字（提示区 80 字）、值 30 字、
  链接目标 60 字（保留前 max − 1 字并去掉尾部空白，再加 `…`）。
- 分组行：`» {ref} {标签}「{名称}」`，下级缩进两个空格；分组有引用，可用于 `within`。标题行：`## {名称}`（级别 1–3 用 1–3 个 `#`），
  只作分组，不带引用。只输出含有已列出控件的分组与标题。
- 没有控件时：`（没有可见的可交互元素）`；有 `query` 但无匹配：`（没有与「{query}」匹配的可交互元素）`。
- 超过 `limit`：末行 `…另有 {remaining} 个元素未列出，可用 query 或 within 缩小范围`。

### 4.3 结构化条目

`{ref, role, name, value?, href?, states?, required?, declared?, group?}`：`role` 为第 5 节的英文角色；`states` 为第 6 节的状态；
`declared` 为已声明的工具名；`group` 为分组路径（外层在前，以 ` › ` 连接，如 `主区域 › 购物车`）。没有的字段省略。

### 4.4 列出规则

- 只列**可见**且可交互或有意义的控件：按钮、链接、输入类控件、复选 / 单选 / 开关、下拉框、滑块、标签页、菜单项、树节点、
  选项、有内容的提示区。静态文本、布局容器、装饰图片不列（需要文字时用 `ui.read`）。
- 跳过：不可见 / 折叠 / 屏外（被裁剪或滚出视口）、被模态层遮挡、已与界面分离的控件。
- 窗口、对话框、表单、导航、分组框、滚动区等作为分组。

## 5. 角色

`role`（英文，ARIA 风格）与文本中的中文标签：

| role | 标签 | role | 标签 | role | 标签 |
|---|---|---|---|---|---|
| `button` | 按钮 | `link` | 链接 | `textbox` | 输入框（密码类：密码框） |
| `searchbox` | 搜索框 | `checkbox` | 复选框 | `radio` | 单选框 |
| `switch` | 开关 | `combobox` | 下拉框 | `listbox` | 列表框 |
| `option` | 选项 | `slider` | 滑块 | `spinbutton` | 数字框 |
| `tab` | 标签页 | `menuitem` | 菜单项 | `treeitem` | 树节点 |
| `gridcell` | 单元格 | `status` | 提示 | `alert` | 警告 |
| `generic` | 元素 | | | | |

分组：`window` 窗口、`dialog` 对话框、`alertdialog` 警告框、`navigation` 导航、`main` 主区域、`form` 表单、`search` 搜索区、
`group` 分组、`list` 列表、`scrollable` 滚动区。标题：`heading`。网页另有输入类型细分的标签（邮箱框、日期框等，role 仍为
`textbox`）。

## 6. 状态

`disabled`、`checked` / `unchecked` / `mixed`、`expanded` / `collapsed`、`selected`、`pressed`、`current`、`readonly`、`invalid`、
`focused`。变化摘要中 `focused` 不算变化；同组互斥状态（`checked` / `unchecked` / `mixed`、`expanded` / `collapsed`）只说新状态。

## 7. 操作

### 7.1 执行前核对

每个动作在 **UI 线程上重新按引用取控件**，再依次核对：

| 条件 | 失败时 |
|---|---|
| 仍在界面上（3） | `INVALID_INPUT`，`data = {ref}`："引用 eN 已失效，请重新调用 ui.outline" |
| 可见（未折叠 / 未被遮挡 / 所在窗口可见） | `INVALID_INPUT`，`data = {ref, reason: "hidden"}`："{控件}当前不可见，请重新调用 ui.outline" |
| 启用 | `INVALID_INPUT`，`data = {ref, reason: "TOOL_DISABLED"}`："{控件}已禁用，当前无法操作" |
| 不是密码类控件（只对 `ui.fill`、对其按键） | `INVALID_INPUT`，`data = {ref, reason: "secure"}`："{控件}是密码类控件，兜底工具不填写" |
| 控件支持该动作 | `INVALID_INPUT`，`data = {ref, reason: "unsupported"}`："{控件}不支持该操作" |

`ui.scroll` 只核对第一条（屏外的控件正是"滚动到可见"的对象）。

### 7.2 结果

```json
{ "ok": true, "changes": ["e3 结算 不再 disabled", "新增对话框「确认支付」(e4)，含 2 个可交互元素（可用 ui.outline({ within: \"e4\" }) 查看）"] }
```

- 执行后等界面稳定（一帧 + 短暂延迟；框架的异步激活如 WPF `Invoke` 先让出调度器），在全部窗口范围内做前后对比，只返回变化：
  - 新增：`新增{标签}「{名称}」({ref})`，控件另接 ` = "{值}"` 与状态；新增分组内的控件只计数：`，含 {n} 个可交互元素（可用 ui.outline({ within: "{ref}" }) 查看）`；
  - 消失：`{标签}「{名称}」({ref}) 已消失`，分组另接 `（含 {n} 个可交互元素）`；
  - 变化：`{ref} {旧名称或标签} {部分}`，部分为 `名称变为「…」`、`值变为 "…"` / `值已清空`、`变为 {状态}`、`不再 {状态}`，以 `，` 连接；
  - 标题：`标题「旧」变为「新」`。
- 超过 15 条时截断：`…另有 {n} 项变化，请调用 ui.outline 查看`。
- 可选字段：`url`（网页地址变化时）、`hint`（操作的控件已有声明的工具时：`该元素已声明为工具 {name}，下次可直接调用`）。

## 8. 安全与平台映射

### 8.1 共同规则

- **密码类控件**（网页 `type=password`、WPF `PasswordBox`、Flutter `obscureText`）：大纲与 `ui.read` 中值固定显示 `••••`
  （不泄露长度；为空时不显示值），`ui.fill` 与对其按键一律拒绝（7.1）。框架本身不替我们拦（WPF 可经 Value 模式写入 PasswordBox、
  Flutter 语义值按长度输出掩码），由兜底实现统一处理。
- 兜底工具可以点击界面上的任何按钮，包括付款、删除；它们没有业务语义，Host 无法据此判断具体动作的风险。真正高风险的动作请声明语义工具
  并标注注解。
- 大纲把已有声明工具的控件标为 `declared`，提示模型优先调用那个工具。

### 8.2 平台映射

| | 网页（`@app-mcp/inspect`） | Flutter（`McpUiFallback`） | WPF（`WpfUiFallback`） |
|---|---|---|---|
| 控件树 | DOM | 语义树（`SemanticsBinding.ensureSemantics()`，只在启用且已连接时持有句柄） | `Application.Current.Windows` + `AutomationPeer.GetChildren()` |
| 引用 | `WeakRef<Element>` | `WeakReference<SemanticsNode>` + 指纹 | `WeakReference<AutomationPeer>` |
| 可见 | 样式 / 布局 / `hidden` / `inert` | 非 `isHidden` / `isInvisible`、未合并进父节点；模态路由由框架屏蔽下层 | 窗口 `IsVisible`、`!IsOffscreen()` |
| 启用 | `disabled` / `aria-disabled` | `isEnabled != false` | `IsEnabled()` |
| 点击 | 合成 pointer / mouse 事件 | `SemanticsOwner.performAction(tap)` | `Invoke` / `Toggle` / `SelectionItem.Select` / `ExpandCollapse` |
| 填写 | 原生 setter + `input` / `change` | 聚焦、等一帧、`setText` | `Value.SetValue`（文本框再 `UpdateSource()`）、`Toggle`、`Selection` |
| 滚动 | `scrollIntoView` | `showOnScreen` / `scrollUp` 等 | `ScrollItem.ScrollIntoView` / `Scroll` |
| 已声明 | `data-mcp-tool` / `toolname` | `McpDeclared(tool: …)` 写入语义 identifier `mcp:<工具名>`：标在该节点上，或（落在外层非控件节点时）其子树中唯一的控件上 | `WpfViewTools.Bind(控件, 工具)` 绑定的控件 |
| 可见窗口 | 页面打开即可（`surface: "app"`） | `AppLifecycleState` 为 resumed / inactive | 任一窗口 `IsVisible` 且未最小化 |
| 按键 | 合成 keydown / keyup | Tab：`FocusNode.nextFocus`；Escape：`DismissIntent`；Enter：焦点文本框的输入动作，否则激活 | `InputManager.ProcessInput` 按下 / 抬起；Enter / Escape 未处理时交给默认 / 取消按钮（`AccessKeyManager`）；Tab：`MoveFocus` |
