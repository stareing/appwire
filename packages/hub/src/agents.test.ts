/**
 * Agent 登记（spec/hub-api.md 3.6「Agent 身份」）：`HubStartOptions.agents`、`Hub.setAgents`。
 *
 * 前置条件：`pnpm --filter @app-mcp/hub build:native`（或 APP_MCP_HUB_NATIVE 指定路径）；缺少时跳过。
 */
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import { Hub, type AgentCredential } from './index.js'
import { nativeFileName } from './native.js'

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const hubNative = process.env.APP_MCP_HUB_NATIVE ?? join(pkgDir, 'native', nativeFileName())

const CLAUDE = 'claude-0123456789abcdef0123456789abcdef'
const CURSOR = 'cursor-0123456789abcdef0123456789abcdef'

const hubs: Hub[] = []
afterEach(async () => {
  for (const hub of hubs.splice(0)) await hub.shutdown()
})

async function startHub(agents?: AgentCredential[]): Promise<Hub> {
  const hub = await Hub.start({ listen: '127.0.0.1:0', ipcEndpoint: null, keepAlive: false, mcpHttp: true, agents })
  hubs.push(hub)
  return hub
}

/** 经 `/mcp` 以 `token` 发无会话（2026-07-28）`apps.task.begin` → 任务 ID。 */
async function beginTask(hub: Hub, token: string): Promise<string> {
  const res = await fetch(`http://${hub.listenAddr}/mcp`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      accept: 'application/json, text/event-stream',
      authorization: `Bearer ${token}`,
      'mcp-protocol-version': '2026-07-28',
      'mcp-method': 'tools/call',
      'mcp-name': 'apps.task.begin',
    },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'tools/call',
      params: {
        name: 'apps.task.begin',
        arguments: {},
        _meta: {
          'io.modelcontextprotocol/protocolVersion': '2026-07-28',
          'io.modelcontextprotocol/clientCapabilities': {},
          'io.modelcontextprotocol/clientInfo': { name: 't', version: '1' },
        },
      },
    }),
  })
  const text = await res.text()
  const line = text
    .split('\n')
    .map((l) => (l.startsWith('data:') ? l.slice(5).trim() : l.trim()))
    .find((l) => l.startsWith('{'))
  if (!line) throw new Error(text)
  return JSON.parse(line).result.structuredContent.taskId as string
}

const taskAgent = (hub: Hub, id: string) => hub.status().tasks?.find((t) => t.id === id)?.agent

describe.skipIf(!existsSync(hubNative))('Agent 登记', () => {
  it('出示 Agent 令牌的 /mcp 请求归该 Agent；setAgents 替换，不合法时保留之前的登记', async () => {
    const hub = await startHub([{ name: 'claude', token: CLAUDE }])
    expect(hub.status().agents).toEqual(['claude'])
    expect(JSON.stringify(hub.status())).not.toContain(CLAUDE)
    expect(taskAgent(hub, await beginTask(hub, CLAUDE))).toBe('claude')

    const dup = [
      { name: 'a', token: CLAUDE },
      { name: 'a', token: CURSOR },
    ]
    expect(() => hub.setAgents(dup)).toThrow(expect.objectContaining({ kind: 'INVALID_INPUT' }))
    expect(() => hub.setAgents({ agents: [] } as never)).toThrow(expect.objectContaining({ kind: 'INVALID_ARG' }))
    expect(hub.status().agents).toEqual(['claude'])

    hub.setAgents([{ name: 'cursor', token: CURSOR }])
    expect(hub.status().agents).toEqual(['cursor'])
    expect(taskAgent(hub, await beginTask(hub, CURSOR))).toBe('cursor')
    hub.setAgents([])
    expect(hub.status().agents).toEqual([])
  })

  it('登记不合法 → 启动失败，信息不含令牌', async () => {
    const err = await startHub([{ name: 'a b', token: CLAUDE }]).catch((e: unknown) => e)
    expect(err).toMatchObject({ kind: 'START_FAILED' })
    expect(String((err as Error).message)).not.toContain(CLAUDE)
  })
})
