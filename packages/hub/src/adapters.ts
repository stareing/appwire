/**
 * 常见 Node Agent 框架 / LLM SDK 的便利适配（纯函数，不引入依赖）。
 *
 * 全部基于 `exportTools` + `dispatch` 的 JSON 形态：工具名为导出名（`[a-zA-Z0-9_-]{1,64}`，
 * `shop.cart.add` → `shop__cart__add`），调用经过与 MCP 出口相同的逻辑（校验 → 审批 → 路由 → 首次附带总览）。
 */

import type {
  AnthropicToolDef,
  AnthropicToolResult,
  AnthropicToolUse,
  ExportedTools,
  GeminiFunctionCall,
  GeminiFunctionResponsePart,
  GeminiTools,
  JsonSchema,
  OpenAiChatToolCall,
  OpenAiChatToolDef,
  OpenAiChatToolMessage,
  OpenAiResponsesFunctionCall,
  OpenAiResponsesFunctionCallOutput,
  OpenAiResponsesToolDef,
  ToolCallInput,
  ToolFilter,
  ToolFormat,
  ToolResultMessage,
} from './types.js'

/** 适配器需要的 Hub 能力（`Hub` 实例即满足；测试可传入替身）。 */
export interface HubLike {
  exportTools<F extends ToolFormat>(format: F, filter?: ToolFilter): ExportedTools[F]
  dispatch<F extends ToolFormat>(format: F, toolCall: ToolCallInput[F], session?: string | null): Promise<ToolResultMessage[F]>
  cancelCall(callId: string): void
}

export interface DispatchOptions {
  /** 厂商会话 ID（总览首次附带、渐进暴露按会话计算；与导出时 `ToolFilter.session` 对应）；缺省为默认会话。 */
  session?: string | null
  /** 逐个执行（默认并发执行；同一 App 的调用本身按其并发上限排队）。 */
  sequential?: boolean
}

async function dispatchAll<F extends ToolFormat>(
  hub: HubLike,
  format: F,
  calls: ToolCallInput[F][],
  options: DispatchOptions = {},
): Promise<ToolResultMessage[F][]> {
  if (options.sequential) {
    const out: ToolResultMessage[F][] = []
    for (const c of calls) out.push(await hub.dispatch(format, c, options.session ?? null))
    return out
  }
  return Promise.all(calls.map((c) => hub.dispatch(format, c, options.session ?? null)))
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

/** Chat Completions 的 `tools` 参数。 */
export function toOpenAiTools(hub: HubLike, filter?: ToolFilter): OpenAiChatToolDef[] {
  return hub.exportTools('openai-chat', filter)
}

/**
 * 执行 assistant 消息中的 `tool_calls`，返回 `{ role: 'tool', tool_call_id, content }` 数组，
 * 按原顺序直接追加到 `messages`。`toolCalls` 为空 / undefined 时返回空数组。
 */
export function handleOpenAiToolCalls(
  hub: HubLike,
  toolCalls: readonly OpenAiChatToolCall[] | null | undefined,
  options?: DispatchOptions,
): Promise<OpenAiChatToolMessage[]> {
  const calls = (toolCalls ?? []).filter((c) => isObject(c) && (c.type === undefined || c.type === 'function'))
  return dispatchAll(hub, 'openai-chat', calls, options)
}

/** Responses API 的 `tools` 参数。 */
export function toOpenAiResponsesTools(hub: HubLike, filter?: ToolFilter): OpenAiResponsesToolDef[] {
  return hub.exportTools('openai-responses', filter)
}

/**
 * 执行 Responses API `output` 中的 `function_call` 项（其他项忽略），返回 `function_call_output` 数组，
 * 放进下一次请求的 `input`。
 */
export function handleOpenAiResponsesCalls(
  hub: HubLike,
  outputItems: readonly unknown[] | null | undefined,
  options?: DispatchOptions,
): Promise<OpenAiResponsesFunctionCallOutput[]> {
  const calls = (outputItems ?? []).filter(
    (i): i is OpenAiResponsesFunctionCall => isObject(i) && i.type === 'function_call',
  )
  return dispatchAll(hub, 'openai-responses', calls, options)
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

/** Messages API 的 `tools` 参数。 */
export function toAnthropicTools(hub: HubLike, filter?: ToolFilter): AnthropicToolDef[] {
  return hub.exportTools('anthropic', filter)
}

/**
 * 执行 assistant 回复 `content` 中的全部 `tool_use` 块（其他块忽略），返回 `tool_result` 块数组，
 * 作为下一条 `{ role: 'user', content: results }` 消息。
 */
export function handleAnthropicToolUses(
  hub: HubLike,
  contentBlocks: readonly unknown[] | null | undefined,
  options?: DispatchOptions,
): Promise<AnthropicToolResult[]> {
  const uses = (contentBlocks ?? []).filter((b): b is AnthropicToolUse => isObject(b) && b.type === 'tool_use')
  return dispatchAll(hub, 'anthropic', uses, options)
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

/** `tools: [toGeminiTools(hub)]`。 */
export function toGeminiTools(hub: HubLike, filter?: ToolFilter): GeminiTools {
  return hub.exportTools('gemini', filter)
}

/**
 * 执行回复 `parts` 中的 `functionCall`（其他 part 忽略），返回 `{ functionResponse }` part 数组。
 * 也接受 `response.functionCalls` 形式的裸 `{ name, args }` 数组。
 */
export function handleGeminiFunctionCalls(
  hub: HubLike,
  parts: readonly unknown[] | null | undefined,
  options?: DispatchOptions,
): Promise<GeminiFunctionResponsePart[]> {
  const calls: GeminiFunctionCall[] = []
  for (const p of parts ?? []) {
    if (!isObject(p)) continue
    if (isObject(p.functionCall)) calls.push(p.functionCall as unknown as GeminiFunctionCall)
    else if (typeof p.name === 'string') calls.push(p as unknown as GeminiFunctionCall)
  }
  return dispatchAll(hub, 'gemini', calls, options)
}

// ---------------------------------------------------------------------------
// Vercel AI SDK
// ---------------------------------------------------------------------------

/** 工具失败（`USER_REJECTED: …` 等），由 Vercel AI SDK 作为 tool-error 交给模型。 */
export class HubToolCallError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'HubToolCallError'
  }
}

/** `execute` 的第二个参数（与 AI SDK 的 `ToolCallOptions` 兼容的子集）。 */
export interface VercelAiExecuteOptions {
  toolCallId?: string
  abortSignal?: AbortSignal
}

export interface VercelAiTool<S = JsonSchema> {
  description: string
  inputSchema: S
  execute: (input: unknown, options?: VercelAiExecuteOptions) => Promise<string>
}

export interface VercelAiToolsOptions<S> {
  /** 会话 ID（总览首次附带按会话计算）。 */
  session?: string | null
  /**
   * 把 JSON Schema 包装为 AI SDK 的 Schema：传入 `ai` 包的 `jsonSchema`。
   * AI SDK v5 的 `streamText` / `generateText` 要求 `inputSchema` 为 `jsonSchema(...)` 或 zod；缺省时原样返回 JSON Schema。
   */
  jsonSchema?: (schema: JsonSchema) => S
}

let seq = 0
function newCallId(): string {
  const c = globalThis.crypto
  return typeof c?.randomUUID === 'function' ? `vai-${c.randomUUID()}` : `vai-${Date.now()}-${++seq}`
}

/**
 * 转为 Vercel AI SDK（v5）的 `tools` 对象：`{ [导出名]: { description, inputSchema, execute } }`。
 *
 * - `execute` 返回工具结果文本（与 MCP 出口一致：JSON 文本，首次接触附带总览）；工具失败时抛 {@link HubToolCallError}。
 * - `abortSignal` 触发时取消调用（`cancelCall`）。
 */
export function toVercelAiTools<S = JsonSchema>(
  hub: HubLike,
  filter?: ToolFilter,
  options: VercelAiToolsOptions<S> = {},
): Record<string, VercelAiTool<S>> {
  const wrapSchema = options.jsonSchema ?? ((s: JsonSchema) => s as unknown as S)
  const out: Record<string, VercelAiTool<S>> = {}
  for (const def of hub.exportTools('anthropic', filter)) {
    const name = def.name
    out[name] = {
      description: def.description,
      inputSchema: wrapSchema(def.input_schema),
      execute: async (input, execOptions) => {
        const id = execOptions?.toolCallId ?? newCallId()
        const signal = execOptions?.abortSignal
        if (signal?.aborted) throw new HubToolCallError('CANCELLED: 调用已取消')
        const onAbort = () => hub.cancelCall(id)
        signal?.addEventListener('abort', onAbort, { once: true })
        try {
          const r = await hub.dispatch(
            'anthropic',
            { type: 'tool_use', id, name, input: input ?? {} },
            options.session ?? null,
          )
          if (r.is_error) throw new HubToolCallError(r.content)
          return r.content
        } finally {
          signal?.removeEventListener('abort', onAbort)
        }
      },
    }
  }
  return out
}
