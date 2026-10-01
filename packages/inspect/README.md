# @app-mcp/inspect

> Part of [AppWire](https://github.com/stareing/appwire): an opt-in fallback that lets AI agents operate web interfaces that declare no MCP tools, via a compact outline of interactive elements and short references (`e12`) for click, fill, key and submit.

为**没有声明工具**的网页界面提供兜底的通用操作能力：读出一份精简的可交互元素大纲，再用短引用（`e12`）点击、填写、按键、提交。

> **定位：能力超集中的最低一级，默认关闭。**
>
> 语义工具（`useTool` / `appMcp.tool`）> 声明式（`@app-mcp/dom` 的 `data-mcp-*`、`@app-mcp/store`）> 本兜底。
> 只有显式调用 `attachInspect` 才会注册工具；建议只在开发环境或用户明确开启时使用。
> 大纲会把已声明的元素标出来（`[已声明：cart.clear]`），提示模型优先调用对应工具。

## 用法

```ts
import { createAppMcp } from '@app-mcp/web'
import { attachInspect } from '@app-mcp/inspect'

const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城' })

// 只在开发环境启用
if (import.meta.env.DEV) {
  const detach = attachInspect(appMcp, {
    root: document.body, // 默认 document.body（调用时取值）
    prefix: 'ui',        // 工具名前缀，默认 'ui'
    maxItems: 60,        // 大纲默认最多列出的元素数
    allowScript: false,  // 是否提供 ui.eval，默认 false
  })
  // detach() 注销全部工具
}
```

不在后台观察 DOM：没有 MutationObserver、没有定时器，所有内容都在工具被调用时同步计算；不调用就没有任何开销。

## 工具

| 工具 | 风险 | 说明 |
|---|---|---|
| `ui.outline({ query?, within?, limit? })` | read | 可见的可交互 / 有意义元素大纲，每个元素一行，按标题与区域分组 |
| `ui.click({ ref })` | write | 派发 pointer / mouse 事件并点击 |
| `ui.fill({ ref, value })` | write | 填写输入框、多行输入框、下拉框（按值或选项文本）、复选框 / 单选框（true / false）、contenteditable；兼容 React 受控组件 |
| `ui.press({ ref?, key })` | write | 派发 keydown / keypress / keyup（如 `Enter`、`Escape`、`Shift+Tab`、`Control+a`），并模拟 Enter 提交、空格激活、Tab 移焦 |
| `ui.scroll({ ref })` | write | 滚动到视口中央（可能触发懒加载） |
| `ui.submit({ ref })` | write | 表单或其中元素 → `requestSubmit`（先做浏览器校验，失败时报出无效字段） |
| `ui.read({ ref?, maxChars? })` | read | 元素的精简可见文本，默认 1000 字 |
| `ui.eval({ code })` | destructive | 执行任意脚本，**仅 `allowScript: true` 时注册** |

### 大纲

```
e1 链接「示例商城」→ /
» e2 导航「主菜单」
  e3 链接「首页」→ /
  e5 链接「我的订单」→ /orders
» e6 搜索区「搜索」
  e7 搜索框「搜索商品」
  e8 按钮「搜索」
» e9 主区域
  ## 购物车
  e20 按钮「清空」 [已声明：cart.clear]
  e21 按钮「结算」 disabled
  » e30 表单「收货」
    e31 输入框「收货地址」= "北京市朝阳区…" (必填)
    e32 复选框「同意条款」 unchecked
```

- 只列可交互或有意义的元素：按钮、链接、输入类控件、带交互 role 的元素、可聚焦 / 可点击（`cursor:pointer`）元素、提示区（`role=status/alert`、`aria-live`）；h1–h3 作为分组标题；`nav` / `main` / `form` / `dialog`（及对应 role）作为分组（`»` 开头，带引用，可用于 `within`）。
- 名称依次取 aria-labelledby、aria-label、label、文本、title、placeholder，截断到 40 字；密码只显示 `••••`。
- 跳过不可见元素：`display:none`、`visibility:hidden`、零尺寸（有布局时）、`hidden`、`inert`、`aria-hidden`、未打开的 `<dialog>`、未展开的 `<details>`。
- `query` 按名称与所在分组模糊过滤（空格分隔多个词）；`within` 限定到某个引用的子树；超过 `limit` 时截断并说明剩余数量。
- 结果同时给出 `text`（上面的行文本）与 `items`（结构化数组：`ref`、`role`、`name`、`value`、`href`、`states`、`required`、`declared`、`group`）。
- 引用在同一页面生命周期内稳定（WeakMap 元素 → 引用）；元素移除后引用失效，调用时返回
  `INVALID_INPUT: 引用 e12 已失效，请重新调用 ui.outline`。

### 操作结果只返回变化

操作执行后等待页面稳定（一帧 + 50ms），对大纲范围内的元素做前后对比，只返回变化摘要：

```json
{
  "ok": true,
  "changes": [
    "e21 结算 变为 disabled",
    "新增对话框「确认支付」(e40)，含 2 个可交互元素（可用 ui.outline({ within: \"e40\" }) 查看）",
    "按钮「删除商品」(e15) 已消失"
  ],
  "url": "https://shop.example.com/orders"
}
```

`url` 只在变化时出现；变化超过 15 条时截断并提示重新调用 `ui.outline`。操作禁用元素时返回
`INVALID_INPUT: e21 按钮「结算」已禁用，当前无法操作`（`details.reason = 'TOOL_DISABLED'`）。

## 与 chrome-devtools MCP 的对比

chrome-devtools MCP 的 `take_snapshot` 返回整棵无障碍树：每个布局容器、每段静态文本、每张图片都是一行，
一个普通电商页往往几百到上千行；每次点击后还要重新拿整页快照才能知道发生了什么。

本包更短的原因：

1. **只列可交互元素**：静态文本、布局节点、装饰图片都不出现；标题只作为分组行。需要文字内容时用 `ui.read` 按需读取。
2. **每个元素一行**：角色、名称、当前值、链接目标、状态、是否必填都压缩在一行里，没有层级嵌套噪音。
3. **操作后只返回变化**：不返回整页，只说"哪个按钮变禁用了、弹出了什么对话框、URL 变了"。
4. **可缩小范围**：`query` / `within` / `limit` 让模型只看关心的区域。

以测试中的示例商城页面为例：页面 HTML 约 5400 字符，大纲 19 个元素、约 450 字符。

它不替代 chrome-devtools MCP 的网络、性能、控制台等调试能力，只覆盖"读页面 + 模拟输入"这部分。

## 安全提示

- **只在受信任的页面、开发环境或用户明确开启时启用。** 兜底工具可以点击页面上的任何按钮，包括付款、删除等操作；
  它们没有业务语义，Host 无法据此判断具体动作的风险。
- 操作类工具的风险等级为 `write`，Host 仍会按风险策略确认；但真正高风险的动作请用语义工具并标注 `payment` / `destructive`。
- `ui.eval` 可执行任意脚本、读写页面上的任何数据（包括登录态），风险等级为 `destructive`，**默认不注册**，
  除非在本地调试时明确需要，否则不要开启 `allowScript`。在启用严格 CSP（禁止 `unsafe-eval`）的页面上它会报错。
- 大纲不会输出密码框的值；`ui.read` 对密码框也只返回 `••••`。

## 限制

- 事件为合成事件（`isTrusted === false`）：依赖可信事件的行为（打开文件选择框、全屏、剪贴板等）无法触发；文件选择不支持。
- 不进入 iframe、Shadow DOM 与 canvas。
- 只在调用时计算，不监听 DOM；变化摘要只覆盖大纲范围内的元素（不含普通文本变化，提示区 `role=status/alert` 除外）。
