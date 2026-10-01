/**
 * 属性名常量与读取工具。
 *
 * 两种写法：
 * - 本包的 `data-mcp-*` 属性（按钮、表单、集合、资源、scope、风险等级等）；
 * - W3C WebMCP 声明式 API 的表单属性（`toolname`、`tooldescription`、`toolautosubmit`、
 *   `toolparamdescription`），同时出现时以标准属性为准。
 */

import type { Activation, ErrorKind, Risk } from '@app-mcp/web'

export const ATTR = {
  tool: 'data-mcp-tool',
  desc: 'data-mcp-desc',
  title: 'data-mcp-title',
  risk: 'data-mcp-risk',
  activation: 'data-mcp-activation',
  result: 'data-mcp-result',
  timeout: 'data-mcp-timeout',
  hints: 'data-mcp-hints',
  key: 'data-mcp-key',
  label: 'data-mcp-label',
  row: 'data-mcp-row',
  ignore: 'data-mcp-ignore',
  scope: 'data-mcp-scope',
  resource: 'data-mcp-resource',
  json: 'data-mcp-json',
  /** 调用进行中时加在表单上（对应标准的 `:tool-form-active`），不在观察列表中。 */
  active: 'data-mcp-active',
} as const

/** W3C WebMCP 声明式 API（https://developer.chrome.com/docs/ai/webmcp/declarative-api）。 */
export const WEBMCP = {
  toolname: 'toolname',
  tooldescription: 'tooldescription',
  toolautosubmit: 'toolautosubmit',
  toolparamdescription: 'toolparamdescription',
} as const

/** 只查询声明了工具、资源或 scope 的元素，不遍历整棵 DOM。 */
export const SELECTOR = `[${ATTR.tool}],form[${WEBMCP.toolname}],[${ATTR.resource}],[${ATTR.scope}]`

/** MutationObserver 的 attributeFilter：声明属性、表单字段约束与可用性相关属性。 */
export const OBSERVED_ATTRIBUTES: string[] = [
  ATTR.tool,
  ATTR.desc,
  ATTR.title,
  ATTR.risk,
  ATTR.activation,
  ATTR.hints,
  ATTR.key,
  ATTR.label,
  ATTR.ignore,
  ATTR.scope,
  ATTR.resource,
  ATTR.json,
  WEBMCP.toolname,
  WEBMCP.tooldescription,
  WEBMCP.toolparamdescription,
  'disabled',
  'hidden',
  'aria-disabled',
  'inert',
  'style',
  'class',
  // 表单字段（影响参数 schema）
  'name',
  'type',
  'required',
  'multiple',
  'min',
  'max',
  'step',
  'minlength',
  'maxlength',
  'pattern',
  'placeholder',
  'aria-label',
  'title',
]

const RISKS: readonly Risk[] = ['read', 'write', 'destructive', 'payment', 'os-sensitive']
const ACTIVATIONS: readonly Activation[] = ['headless', 'background', 'foreground']
const ERROR_KINDS: readonly ErrorKind[] = [
  'TOOL_NOT_FOUND',
  'TOOL_DISABLED',
  'INVALID_INPUT',
  'USER_REJECTED',
  'TIMEOUT',
  'HANDLER_ERROR',
  'CANCELLED',
  'APP_DISCONNECTED',
  'APP_NOT_INSTALLED',
  'LAUNCH_FAILED',
  'APP_NOT_RESPONDING',
  'INSTANCE_FROZEN',
  'RESOURCE_NOT_FOUND',
  'UNAUTHORIZED',
  'UNSUPPORTED_PROTOCOL',
  'RATE_LIMITED',
  'PAYLOAD_TOO_LARGE',
]

export function isRisk(v: string): v is Risk {
  return (RISKS as readonly string[]).includes(v)
}

export function isActivation(v: string): v is Activation {
  return (ACTIVATIONS as readonly string[]).includes(v)
}

export function toErrorKind(v: unknown): ErrorKind {
  return typeof v === 'string' && (ERROR_KINDS as readonly string[]).includes(v) ? (v as ErrorKind) : 'HANDLER_ERROR'
}

export function isForm(el: Element): el is HTMLFormElement {
  return el.tagName === 'FORM'
}

/** 标准写法的表单（带 `toolname`）。 */
export function isStandardForm(el: Element): boolean {
  return isForm(el) && el.hasAttribute(WEBMCP.toolname)
}

/** 元素声明的工具名：表单上 `toolname` 优先于 `data-mcp-tool`。 */
export function toolNameOf(el: Element): string | null {
  if (isStandardForm(el)) return el.getAttribute(WEBMCP.toolname)
  return el.getAttribute(ATTR.tool)
}

/** 非空的属性值（去掉首尾空白）。 */
export function attr(el: Element, name: string): string | undefined {
  const v = el.getAttribute(name)?.trim()
  return v ? v : undefined
}

export function parseList(v: string | undefined): string[] {
  if (!v) return []
  return v
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
}
