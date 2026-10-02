/** @app-mcp/hub 的公开类型：工具格式导出与分派（spec/hub-api.md 第 5 节）。 */

import type { JsonSchema } from './apps.js'

export type ToolFormat = 'mcp' | 'openai-chat' | 'openai-responses' | 'anthropic' | 'gemini'

export interface McpToolDef {
  name: string
  title?: string
  description: string
  inputSchema: JsonSchema
  annotations?: Record<string, unknown>
  /** 工具声明了 `outputSchema` 时给出（根类型不是 object 时已包装为 `{ result }`）。 */
  outputSchema?: JsonSchema
}
export interface McpToolCall {
  name: string
  arguments?: unknown
}
export interface McpCallToolResult {
  content: Array<{ type: string; text?: string; [k: string]: unknown }>
  isError?: boolean
  structuredContent?: unknown
  [k: string]: unknown
}

export interface OpenAiChatToolDef {
  type: 'function'
  function: { name: string; description: string; parameters: JsonSchema }
}
export interface OpenAiChatToolCall {
  id: string
  type?: 'function'
  function: { name: string; arguments: string }
}
export interface OpenAiChatToolMessage {
  role: 'tool'
  tool_call_id: string
  content: string
}

export interface OpenAiResponsesToolDef {
  type: 'function'
  name: string
  description: string
  parameters: JsonSchema
}
export interface OpenAiResponsesFunctionCall {
  type: 'function_call'
  call_id: string
  name: string
  arguments: string
  id?: string
}
export interface OpenAiResponsesFunctionCallOutput {
  type: 'function_call_output'
  call_id: string
  output: string
}

export interface AnthropicToolDef {
  name: string
  description: string
  input_schema: JsonSchema
}
export interface AnthropicToolUse {
  type: 'tool_use'
  id: string
  name: string
  input: unknown
}
export interface AnthropicToolResult {
  type: 'tool_result'
  tool_use_id: string
  content: string
  is_error?: boolean
}

export interface GeminiTools {
  functionDeclarations: Array<{ name: string; description: string; parameters?: JsonSchema }>
}
export interface GeminiFunctionCall {
  name: string
  args?: unknown
  id?: string
}
export interface GeminiFunctionResponsePart {
  functionResponse: { name: string; id?: string; response: { output: string } | { error: string } }
}

/** `exportTools(format)` 的返回类型。 */
export interface ExportedTools {
  mcp: McpToolDef[]
  'openai-chat': OpenAiChatToolDef[]
  'openai-responses': OpenAiResponsesToolDef[]
  anthropic: AnthropicToolDef[]
  gemini: GeminiTools
}

/** `dispatch(format, call)` 接受的工具调用。 */
export interface ToolCallInput {
  mcp: McpToolCall | { params: McpToolCall; [k: string]: unknown }
  'openai-chat': OpenAiChatToolCall
  'openai-responses': OpenAiResponsesFunctionCall
  anthropic: AnthropicToolUse
  gemini: GeminiFunctionCall | { functionCall: GeminiFunctionCall }
}

/** `dispatch(format, call)` 返回的工具结果消息。 */
export interface ToolResultMessage {
  mcp: McpCallToolResult
  'openai-chat': OpenAiChatToolMessage
  'openai-responses': OpenAiResponsesFunctionCallOutput
  anthropic: AnthropicToolResult
  gemini: GeminiFunctionResponsePart
}
