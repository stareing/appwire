/**
 * 测试用：在隔离的 `window` 上执行插件真正注入的初始化脚本（crates/tauri-plugin/js/bridge.js），
 * 并模拟 Tauri 的 `__TAURI_INTERNALS__.invoke` 与 Rust 侧 `Webview::eval` 事件投递。
 */

import { readFileSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
import type { MainEvent, OpReply, RendererOp } from '@app-mcp/web'

export const BRIDGE_SCRIPT = readFileSync(
  new URL('../../../crates/tauri-plugin/js/bridge.js', import.meta.url),
  'utf8',
)

export interface FakeTauri {
  window: Record<string, any>
  /** 页面发来的操作（按顺序）。 */
  ops: RendererOp[]
  /** 设置某种操作的应答（缺省 `{ok: true}`；`hello` 缺省带 instanceId / state）。 */
  reply: (op: RendererOp) => OpReply | Promise<OpReply>
  /** 模拟 Rust 侧 `webview.eval("window.__APP_MCP_TAURI_DISPATCH__&&window.__APP_MCP_TAURI_DISPATCH__(<json>)")`。 */
  emit(event: MainEvent): void
  /** 等待满足条件的操作出现。 */
  waitFor(match: (op: RendererOp) => boolean): Promise<RendererOp>
}

export function defaultReply(op: RendererOp): OpReply {
  if (op.op === 'hello') return { ok: true, value: { instanceId: 'inst-1', state: { status: 'connected' } } }
  return { ok: true }
}

export function createFakeTauri(options: { internals?: boolean; window?: Record<string, any> } = {}): FakeTauri {
  const win: Record<string, any> = options.window ?? {}
  const fake: FakeTauri = {
    window: win,
    ops: [],
    reply: defaultReply,
    emit(event) {
      // 与 Rust 侧相同：经 eval 执行一段脚本（JSON 字面量即事件）。
      runInNewContext(
        `window.__APP_MCP_TAURI_DISPATCH__&&window.__APP_MCP_TAURI_DISPATCH__(${JSON.stringify(event)})`,
        { window: win },
      )
    },
    async waitFor(match) {
      for (let i = 0; i < 200; i++) {
        const found = fake.ops.find(match)
        if (found) return found
        await new Promise((resolve) => setTimeout(resolve, 5))
      }
      throw new Error('等待操作超时')
    },
  }
  if (options.internals !== false) {
    win.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args: { op: RendererOp }) => {
        if (cmd !== 'plugin:app-mcp|op') throw new Error(`未知命令 ${cmd}`)
        // 经 IPC 传输：结构化为 JSON。
        const op = JSON.parse(JSON.stringify(args.op)) as RendererOp
        fake.ops.push(op)
        return fake.reply(op)
      },
    }
  }
  runInNewContext(BRIDGE_SCRIPT, { window: win, console, Promise, Object })
  return fake
}
