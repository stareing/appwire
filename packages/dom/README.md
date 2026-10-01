# @app-mcp/dom

> Part of [AppWire](https://github.com/stareing/appwire): declare MCP (Model Context Protocol) tools and resources with plain `data-mcp-*` HTML attributes — no JavaScript — so AI agents such as Claude, ChatGPT and Gemini can operate your page through declared actions instead of DOM scraping.

只写 HTML 属性就能把按钮、表单、列表、状态区域声明为 MCP 工具与资源，不需要写 JS。

与 chrome-devtools 这类"把整棵 DOM / 无障碍树交给模型"的方式不同：模型只看到页面**主动声明**的工具与资源，
以及一份一行一项的精简快照（`ui.snapshot`）。

同时兼容 [W3C WebMCP 声明式 API](https://developer.chrome.com/docs/ai/webmcp/declarative-api) 的表单属性
（`toolname`、`tooldescription`、`toolautosubmit`、`toolparamdescription`），按标准写的页面可以直接接入。

```ts
import { createAppMcp } from '@app-mcp/web'
import { attachDom } from '@app-mcp/dom'

const appMcp = createAppMcp({ appId: 'notes', appName: '便签' })
const detach = attachDom(appMcp /* 或任意 Registrar / Scope */, {
  root: document.body, // 观察范围，默认 document.body
  snapshot: true, // 是否登记精简快照资源；可写 { name: 'page.snapshot' } 改名，默认 'ui.snapshot'
  settleMs: 50, // 点击 / 提交后等待页面稳定的时间，默认 50 ms
})

detach() // 断开观察并注销全部由 DOM 声明的工具、资源与 scope
```

## 工具

```html
<button data-mcp-tool="cart.clear" data-mcp-desc="清空购物车" data-mcp-risk="destructive">清空</button>
```

| 属性 | 说明 |
|---|---|
| `data-mcp-tool="名称"` | 声明工具。名称规则同 SDK：`[a-zA-Z0-9_.-]{1,64}`，全局唯一。 |
| `data-mcp-desc` | 描述（必需）。缺失时依次用可见文本 / `aria-label` / `title` 兜底（表单先用 `aria-label` / `title`），并 `console.warn`。 |
| `data-mcp-title` | 标题。 |
| `data-mcp-risk` | `read` / `write` / `destructive` / `payment` / `os-sensitive`，缺省 `write`。 |
| `data-mcp-activation` | `headless` / `background` / `foreground`。 |
| `data-mcp-readonly` / `data-mcp-destructive` / `data-mcp-idempotent` / `data-mcp-open-world` | 标准 MCP 工具注解 `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`，见下文。 |
| `data-mcp-output-schema='{"type":"object",…}'` | 结果的 JSON Schema（MCP `outputSchema`），值为 JSON 对象。 |
| `data-mcp-result="事件名"` | 等待元素上派发的 `CustomEvent(事件名)`，以 `event.detail` 作为结果。 |
| `data-mcp-timeout="毫秒"` | 等待结果的超时，默认 10000，超时返回 `TIMEOUT`。 |
| `data-mcp-hints="a,b"` | 调用成功后作为 `stateHints` 返回（提示模型哪些资源可能已变化）。 |
| `data-mcp-key` / `data-mcp-label` | 集合条目，见下文。 |

### 工具注解与结果 schema

```html
<button data-mcp-tool="todos.archive" data-mcp-desc="归档已完成的待办" data-mcp-idempotent data-mcp-open-world="false">归档</button>
<form data-mcp-tool="order.lookup" data-mcp-desc="查询订单" data-mcp-readonly
      data-mcp-output-schema='{"type":"object","properties":{"status":{"type":"string"}},"required":["status"]}'>…</form>
```

- 四个注解属性按 HTML 布尔属性书写：出现（空值或 `true`）为 `true`，`="false"` 显式声明为 `false`；其他取值 `console.warn` 并忽略。
  都不写时不声明注解（与之前相同）。
- 与 `data-mcp-risk` 同时出现时，声明的注解字段逐个优先，未声明的按 `risk` 推导（spec/protocol.md 3.2）。
  注解只是如实传给 Agent 的声明，是否确认 / 放行由 Agent 决定，本库不据此拦截调用。
- `data-mcp-output-schema` 不是合法 JSON 对象时 `console.warn` 并忽略。根类型不是 `object` 的 schema 由 Host 包装为
  `{ result: … }` 给出。
- 属性被修改或移除时同步更新工具声明（移除即清除声明）。

### 按钮、链接与任意元素

调用 = `element.click()`，没有参数（传入参数返回 `INVALID_INPUT`）。

同名且都**没有** `data-mcp-key` 的多个元素视为同一工具的备选（例如移动端 / 桌面端各一个按钮），
调用时使用第一个可用的元素。

### 表单

```html
<form data-mcp-tool="notes.create" data-mcp-desc="新建便签" data-mcp-result="note-created">
  <label>标题 <input name="title" required maxlength="40"></label>
  <select name="color"><option value="yellow">黄色</option><option value="blue">蓝色</option></select>
  <label><input type="checkbox" name="pinned"> 置顶</label>
  <button>添加</button>
</form>
```

参数 schema 从表单字段推导（`additionalProperties: false`）：

| 字段 | 参数 |
|---|---|
| `name` 属性 | 参数名；没有 `name` 的字段忽略 |
| `required` | 列入 `required` |
| `type=number` / `range` | `number`，`min` / `max` → `minimum` / `maximum`；步长以 0 为基准时加 `multipleOf`（默认步长 1） |
| `type=checkbox` | `boolean`；多个同名 checkbox → 字符串枚举数组 |
| 同名 `type=radio` 组 | 字符串枚举（值为各 radio 的 `value`） |
| `<select>` | 字符串枚举（忽略空值与禁用选项）；`multiple` → 枚举数组 |
| `email` / `url` / `date` / `datetime-local` / `time` | `string` + `format`（`email` / `uri` / `date` / `date-time` / `time`） |
| `minlength` / `maxlength` / `pattern` | `minLength` / `maxLength` / `pattern`（自动加 `^(?:…)$`，与 HTML 的整体匹配一致） |
| `<textarea>` 及其他文本类 input | `string` |
| 排除 | `data-mcp-ignore`（字段自身或表单内的祖先）、`disabled`、`type=hidden/submit/button/reset/image/file` |

参数描述：`toolparamdescription` > `data-mcp-desc` > 关联 `<label>` 文本 > `aria-label` > `placeholder`；
radio / checkbox 组还会用 `<fieldset>` 的 `<legend>`。选项文本与值不同时，描述中附"可选值：值=文本"。

调用流程：

1. 校验参数（未知参数、类型不符、枚举值不存在 → `INVALID_INPUT`），全部通过后才写入；未提供的字段保持原值。
2. 填值（兼容 React 等框架的受控组件，见下文）。
3. 约束校验（`form.checkValidity()`，`novalidate` 时跳过）；不通过 → `INVALID_INPUT`，`details.fields` 给出每个字段的原因。
4. `form.requestSubmit()`（会触发页面的 submit 监听器，与用户点击提交一致）。
5. 收集结果（见"结果"）。

#### React 受控组件兼容

React 在受控 `<input>` 实例上覆盖了 `value` / `checked` 属性来记录"最后已知值"。直接 `input.value = x`
会被记录下来，随后的 `input` 事件被认为"值没变"而不触发 `onChange`。本包的做法：

- 文本、数字、`<select>`：调用**原型链上的原生 `value` setter**（绕过实例上的追踪器），再派发冒泡的 `input` 与 `change` 事件；
- `<select multiple>`：用 `HTMLOptionElement` 原型上的 `selected` setter 逐项设置，再派发 `input` / `change`；
- checkbox / radio：状态不同时调用原生 `click()`（浏览器切换状态并依次派发 `click` / `input` / `change`，React 通过 `click` 感知）；
  若页面阻止了默认行为导致状态没变，退回原生 `checked` setter + `input` / `change`。

Vue（`v-model`）、Svelte、Angular 等监听 `input` / `change` 的框架同样适用。

### 结果

默认结果为 `{ ok: true }`（点击 / 提交后等待 `settleMs`）。可以改为返回结构化结果：

- **`data-mcp-result="事件名"`**（元素或表单上）：等待该元素上派发的 `CustomEvent`，以 `detail` 作为结果；
- **`respondWith`**（表单，WebMCP 标准）：本包触发的 submit 事件带有 `agentInvoked === true` 与 `respondWith(promise)`，
  页面调用 `event.respondWith(...)` 后以其结果作为工具结果（并自动 `preventDefault()`，不再做表单跳转）；promise 失败则调用失败；
- 两者都存在时先到先得。

页面报告失败：在元素（或其子元素，事件需冒泡）上派发

```js
el.dispatchEvent(new CustomEvent('mcp:error', { detail: { kind: 'UNAUTHORIZED', message: '请先登录' } }))
```

`kind` 为协议错误类别（`INVALID_INPUT`、`USER_REJECTED`、`UNAUTHORIZED` 等），未知类别归为 `HANDLER_ERROR`。
`respondWith` 的 promise 以 `{ kind, message }` 拒绝时同样按类别返回。

工具返回值总是 `{ data, stateHints? }` 形式（`stateHints` 来自 `data-mcp-hints`）。

### 集合（列表中的重复工具）

```html
<ul>
  <li>买菜 <button data-mcp-tool="todos.remove" data-mcp-desc="删除待办" data-mcp-key="a1">删除</button></li>
  <li>写周报 <button data-mcp-tool="todos.remove" data-mcp-desc="删除待办" data-mcp-key="a2">删除</button></li>
</ul>
```

- 多个元素有相同的工具名且带 `data-mcp-key` 时合并为**一个**工具，自动增加必填参数 `key`（`enum` 为当前可用的 key）；
- 描述末尾列出 `（key：a1=买菜, a2=写周报）`（最多 30 项）。标签取 `data-mcp-label`，否则取所在行
  （`[data-mcp-row]`、`li`、`tr`、`[role=row]`、`[role=listitem]`、`[role=option]`）的可见文本（不含按钮与其他工具元素），截断到 40 字；
- 调用时点击对应 key 的元素；key 不存在 → `INVALID_INPUT`，该条目不可用 → `TOOL_DISABLED`；
- 列表变化时合并更新 schema 与描述；某个 key 不可用时从 `enum` 中去掉，全部不可用时工具禁用；
- 同名元素中既有带 key 的也有不带的：只使用带 key 的并警告；key 重复时只用第一个；
- 表单集合同理（每行一个带 `data-mcp-key` 的表单），参数为 `key` + 表单字段。

### 启用状态

元素或其祖先有 `disabled`、`aria-disabled="true"`、`hidden`、`inert`，或元素不可见（`display:none`，通过
`checkVisibility()` 判断）时，工具为 `enabled: false`（对 Host 不可见），快照中显示原因。
调用前还会再检查一次，刚刚被禁用的元素返回 `TOOL_DISABLED`。

### Scope

```html
<section data-mcp-scope="cart">
  <button data-mcp-tool="cart.clear" data-mcp-desc="清空购物车">清空</button>
</section>
```

`data-mcp-scope="名称"` 的子树中声明的工具与资源登记到 `registrar.scope(名称)`（可嵌套）；子树移除或改名时 scope 随之 dispose。

## 资源

```html
<div data-mcp-resource="cart.state" data-mcp-desc="购物车内容">…</div>
<ul data-mcp-resource="todos.list" data-mcp-desc="待办列表" data-mcp-json='[{"id":"a1","title":"买菜"}]'>…</ul>
```

- 默认内容为元素的精简可见文本（`text/plain`；折叠空白，截断到 4000 字；跳过 `script`/`style`/`hidden`/`aria-hidden`/内联 `display:none`）；
- 有 `data-mcp-json` 属性时内容为其 JSON 解析结果（`application/json`），页面可以把状态序列化到这个属性里；
- 子树或属性变化时合并（每帧最多一次）调用 `notifyChanged()`，内容没变不通知；SDK 另有节流，且只在 Host 订阅时发送；
- 资源元素被替换（例如列表重新渲染）时沿用同一登记，只切换读取的元素。

## 精简快照 `ui.snapshot`

只列出本次 `attachDom` 声明的工具与资源及其当前状态，一行一个，没有布局节点：

```
[页面] 购物车 · http://localhost:5174/#cart
工具：
- cart.clear  清空购物车  可用
- todos.remove  删除待办（key：a1=买菜, a2=写周报）  可用
- cart.checkout  结算  不可用：按钮已禁用
- todos.add  新建待办（参数：title*, pinned）  可用
资源：cart.state、todos.list
```

表单工具附带参数列表（`*` 表示必填）。内容变化时通知订阅者。`snapshot: false` 关闭。

## W3C WebMCP 声明式表单

```html
<form toolname="search_flights" tooldescription="搜索航班" toolautosubmit>
  <input name="from" toolparamdescription="出发城市三字码" required>
  <input name="date" type="date">
</form>
```

| 标准属性 | 等价于 |
|---|---|
| `<form toolname>` | `data-mcp-tool` |
| `<form tooldescription>` | `data-mcp-desc` |
| 字段上的 `toolparamdescription` | 字段上的 `data-mcp-desc` |
| `toolautosubmit` | 填值后自动 `requestSubmit()` |

- 两种写法同时出现时**以标准属性为准**（名称、描述、参数描述、提交方式）。
- 提交方式：`data-mcp-tool` 写法总是自动提交；标准写法（有 `toolname`）只有带 `toolautosubmit` 才自动提交，
  否则与标准一致：只填值，等待用户自己点提交（受 `data-mcp-timeout` 限制，默认 10 s）。等待期间表单带 `data-mcp-active`
  属性（对应标准的 `:tool-form-active` 伪类，可用 `[data-mcp-active]` 写样式），并在表单上派发冒泡的
  `toolactivated` 事件；超时或取消时派发 `toolcancel`；用户重置表单则调用以 `USER_REJECTED` 结束。
- submit 事件带有 `agentInvoked === true` 与 `respondWith(promise)`（在 document 捕获阶段附加，页面任何监听器都能拿到），
  两种写法都支持。
- 本包**不会**向浏览器的 `navigator.modelContext` / `document.modelContext` 注册任何工具。浏览器原生支持 WebMCP 时，
  标准写法的表单由浏览器自己登记给浏览器内的 Agent；本包仍把它们登记到 app-mcp SDK（经 Host 暴露给 MCP 客户端），
  两条通路相互独立，不会重复登记到同一处。
- 其余能力（按钮等非表单元素、集合 `data-mcp-key`、资源、scope、风险等级、`ui.snapshot`）标准没有覆盖，继续使用 `data-mcp-*`。

## 性能与零侵入

- 只用 `querySelectorAll('[data-mcp-tool],form[toolname],[data-mcp-resource],[data-mcp-scope]')` 查询声明过的元素，不遍历整棵 DOM；
- 一个 `MutationObserver`（`childList` + `subtree` + `characterData` + `attributes`，`attributeFilter` 只含声明属性、
  表单字段约束属性与 `disabled`/`hidden`/`aria-disabled`/`inert`/`style`/`class`）；回调只做标记，
  用 `requestAnimationFrame` 合并处理，**每帧最多一次**（后台标签页 rAF 暂停时用 100 ms 定时器兜底）；
- 处理时与上次登记的定义逐字段比较，只在确有变化时调用 `update()`（只带变化的字段）或 `notifyChanged()`；
- 文本基于 DOM 树提取，不读 `innerText` / 布局；可见性只对工具元素调用 `checkVisibility()`，每批每个元素最多一次；
- 不修改页面 DOM（唯一例外：标准写法等待用户提交时临时加 `data-mcp-active`，该属性不在观察列表中）。
