/**
 * 对象锁（spec/hub-api.md 3.6「对象锁」）：`HubStartOptions.maxLocks`、`apps.lock` / `apps.unlock`、`HubStatus.locks`、`LOCKED`。
 *
 * 前置条件：`pnpm --filter @app-mcp/hub build:native`（或 APP_MCP_HUB_NATIVE 指定路径）；缺少时跳过。
 */
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterAll, afterEach, describe, expect, it } from 'vitest'
import { Hub, HubError, type HubStartOptions } from './index.js'
import { nativeFileName } from './native.js'

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const hubNative = process.env.APP_MCP_HUB_NATIVE ?? join(pkgDir, 'native', nativeFileName())

const dir = mkdtempSync(join(tmpdir(), 'app-mcp-hub-locks-'))
const manifest = join(dir, 'shop.json')
writeFileSync(
  manifest,
  JSON.stringify({
    manifestVersion: 1,
    appId: 'shop',
    name: '商城',
    tools: [{ name: 'cart.add', description: '加购', inputSchema: { type: 'object' } }],
  }),
)
afterAll(() => rmSync(dir, { recursive: true, force: true }))

const hubs: Hub[] = []
afterEach(async () => {
  for (const hub of hubs.splice(0)) await hub.shutdown()
})

async function startHub(options: HubStartOptions = {}): Promise<Hub> {
  const hub = await Hub.start({ listen: null, ipcEndpoint: null, keepAlive: false, manifestFiles: [manifest], ...options })
  hubs.push(hub)
  return hub
}

const builtinNames = (hub: Hub) => hub.tools().filter((t) => t.name.startsWith('apps.')).map((t) => t.name)

describe.skipIf(!existsSync(hubNative))('对象锁', () => {
  it('缺省列出 apps.lock / apps.unlock；他人持有 → LOCKED；status().locks 列出未到期的锁', async () => {
    const hub = await startHub()
    expect(builtinNames(hub)).toEqual(expect.arrayContaining(['apps.lock', 'apps.unlock']))

    const ok = await hub.callTool({ name: 'apps.lock', arguments: { appId: 'shop', ttlMs: 30000 }, session: 's1' })
    expect(ok.result.ok).toMatchObject({ appId: 'shop', ttlMs: 30000, renewed: false })
    const named = await hub.callTool({ name: 'apps.lock', arguments: { appId: 'shop', key: 'doc-1' }, session: 's1' })
    expect(named.result.error).toBeUndefined()

    const denied = await hub.callTool({ name: 'apps.lock', arguments: { appId: 'shop' }, session: 's2' })
    expect(denied.result.error?.kind).toBe('LOCKED')
    expect(denied.result.error?.details).toMatchObject({ appId: 'shop', holder: 'api' })

    const locks = hub.status().locks ?? []
    expect(locks.map(({ appId, key, caller, holder }) => ({ appId, key, caller, holder }))).toEqual([
      { appId: 'shop', key: undefined, caller: 'api:s1', holder: 'api' },
      { appId: 'shop', key: 'doc-1', caller: 'api:s1', holder: 'api' },
    ])
    expect(locks[0]!.expiresInMs).toBeGreaterThan(0)
    expect(locks[0]!.expiresInMs).toBeLessThanOrEqual(30000)

    const released = await hub.callTool({ name: 'apps.unlock', arguments: { appId: 'shop' }, session: 's1' })
    expect(released.result.ok).toMatchObject({ released: true })
    expect(hub.status().locks?.map((l) => l.key)).toEqual(['doc-1'])
  })

  it('maxLocks: 0 关闭对象锁；非法取值启动失败', async () => {
    const hub = await startHub({ maxLocks: 0 })
    expect(builtinNames(hub)).not.toContain('apps.lock')
    expect(builtinNames(hub)).not.toContain('apps.unlock')
    const out = await hub.callTool({ name: 'apps.lock', arguments: { appId: 'shop' }, session: 's1' })
    expect(out.result.error?.kind).toBe('TOOL_NOT_FOUND')
    await startHub({ maxLocks: 1 })
    for (const maxLocks of [-1, 1.5, '8' as unknown as number]) {
      await expect(Hub.start({ listen: null, ipcEndpoint: null, maxLocks })).rejects.toBeInstanceOf(HubError)
    }
  })
})
