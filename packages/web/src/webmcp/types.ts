/**
 * W3C WebMCP 命令式 API 的类型（依据 https://webmachinelearning.github.io/webmcp/ ，
 * Draft Community Group Report 2026-09-29，以及已从标准移除但仍被第三方库使用的旧接口）。
 */

import type { Logger } from '../types'

/** 标准 `ToolAnnotations`；`destructiveHint` 等为 MCP / 旧版库使用的扩展字段。 */
export interface ToolAnnotations {
  readOnlyHint?: boolean
  untrustedContentHint?: boolean
  consequentialHint?: boolean
  debugging?: boolean
  /** 非标准（MCP ToolAnnotations），旧版库仍在使用。 */
  destructiveHint?: boolean
  idempotentHint?: boolean
  openWorldHint?: boolean
  [key: string]: unknown
}

/**
 * `execute` 的第二个参数。标准只定义了 `signal`（ToolExecuteCallbackOptions）；
 * `requestUserInteraction` 来自已移除的 `ModelContextClient`（2026-06 移除），为兼容旧代码保留。
 */
export interface ToolExecuteOptions {
  readonly signal: AbortSignal
  requestUserInteraction<T>(callback: () => T | Promise<T>): Promise<T>
}

/** 标准 `ModelContextTool`。 */
export interface ModelContextTool {
  name: string
  title?: string
  description: string
  inputSchema?: object | string
  execute: (input: any, options: ToolExecuteOptions) => unknown
  annotations?: ToolAnnotations
}

export interface ModelContextRegisterToolOptions {
  exposedTo?: string[]
  signal?: AbortSignal
}

export interface ModelContextGetToolOptions {
  fromOrigins?: string[]
}

export interface ModelContextExecuteToolOptions {
  signal?: AbortSignal
}

/** 标准 `RegisteredTool`。 */
export interface RegisteredTool {
  name: string
  title?: string
  description: string
  inputSchema?: object
  window: Window
  origin: string
  annotations?: ToolAnnotations
}

/** 旧版 `provideContext()` 的参数（2026-03 从标准移除）。 */
export interface ModelContextOptions {
  tools?: ModelContextTool[]
}

/**
 * `registerTool()` 的返回值：标准规定为 `Promise<undefined>`；
 * 额外带 `unregister()`，兼容旧版 MCP-B polyfill 返回 `{ unregister }` 的写法。
 */
export type ToolRegistration = Promise<undefined> & { unregister(): void }

/** 本 SDK 提供的 `modelContext`：标准接口 + 旧接口（超集）。 */
export interface ModelContext extends EventTarget {
  registerTool(tool: ModelContextTool, options?: ModelContextRegisterToolOptions): ToolRegistration
  getTools(options?: ModelContextGetToolOptions): Promise<RegisteredTool[]>
  executeTool(tool: RegisteredTool | { name: string }, input?: object, options?: ModelContextExecuteToolOptions): Promise<string | null>
  /** 旧接口（2026-03 从标准移除）。 */
  unregisterTool(name: string): void
  /** 旧接口：替换经 modelContext 注册的全部工具（不影响 `appMcp.tool()` 注册的工具）。 */
  provideContext(options?: ModelContextOptions): void
  /** 旧接口：注销经 modelContext 注册的全部工具（不影响 `appMcp.tool()` 注册的工具）。 */
  clearContext(): void
  ontoolchange: ((event: Event) => unknown) | null
  ontoolactivated: ((event: Event) => unknown) | null
  ontoolcancel: ((event: Event) => unknown) | null
}

export interface WebMcpOptions {
  /**
   * 用 `appMcp.tool()` 注册的工具也注册到浏览器原生 modelContext（仅桥接模式），
   * 使浏览器内置 AI 可以调用；polyfill 模式下则出现在 `getTools()` 中。默认 true。
   */
  mirrorOwnTools?: boolean
  /** 默认全局 `document`。 */
  document?: Document
  /** 默认全局 `navigator`。 */
  navigator?: Navigator
  /** 默认使用 appMcp 的 logger，否则 console。 */
  logger?: Logger
}

/**
 * - `polyfill`：没有原生实现，安装了本 SDK 的实现；
 * - `bridge`：包装了原生实现的方法，两边同步；
 * - `native-readonly`：原生对象的方法无法覆盖，只能把 `appMcp.tool()` 的工具镜像到原生，
 *   页面经原生接口注册的工具不会进入本 SDK。
 */
export type WebMcpMode = 'polyfill' | 'bridge' | 'native-readonly'

/** `installWebMcp()` 的返回值：调用即卸载。 */
export type WebMcpUninstall = (() => void) & {
  readonly mode: WebMcpMode
  /** 页面可见的 modelContext（polyfill 实例或原生对象）。 */
  readonly modelContext: ModelContext
}
