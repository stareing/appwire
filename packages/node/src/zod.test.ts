/**
 * 用真实 zod v4 验证输入定义转换。zod 在本包中是可选 peer 依赖（未安装），
 * 这里借用工作区其他包安装的 zod；找不到时跳过。
 */
import { existsSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { describe, expect, it } from 'vitest'
import { createAppMcp } from './index.js'
import { FakeNativeClient, fakeBinding } from './testing/fake-native.js'

const zodEntry = fileURLToPath(new URL('../../web/node_modules/zod/index.js', import.meta.url))

describe.skipIf(!existsSync(zodEntry))('zod v4', () => {
  it('转换为输入侧 JSON Schema，并在调用前 parse（含默认值）', async () => {
    const { z } = (await import(/* @vite-ignore */ pathToFileURL(zodEntry).href)) as {
      z: {
        object(shape: Record<string, unknown>): unknown
        string(): { min(n: number): unknown }
        number(): { int(): { default(n: number): unknown } }
      }
    }
    const input = z.object({ q: z.string().min(1), limit: z.number().int().default(10) }) as never
    const app = createAppMcp({ appId: 'demo', appName: 'Demo', binding: fakeBinding, keepAlive: false })
    const native = FakeNativeClient.last as FakeNativeClient
    app.tool('search', { description: 's', input, handler: (i: unknown) => i })
    const schema = JSON.parse(native.tools.get('search')?.spec.inputSchemaJson ?? '{}')
    expect(schema.type).toBe('object')
    expect(schema.required).toEqual(['q'])
    expect(await native.call('search', { q: 'x' })).toEqual({ ok: true, data: { q: 'x', limit: 10 }, stateHints: [] })
    expect(await native.call('search', { q: '' })).toMatchObject({ ok: false, kind: 'INVALID_INPUT' })
    app.dispose()
  })
})
