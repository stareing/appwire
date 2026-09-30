/**
 * 集成测试：真实原生模块（native/*.node）+ crates/native 的 fake_host 示例。
 *
 * 前置条件：`pnpm --filter @app-mcp/node build:native`，且能构建 / 找到 fake_host
 * （`$CARGO_TARGET_DIR/debug/examples/fake_host`，或环境变量 APP_MCP_FAKE_HOST 指定路径）。
 * 缺少任一条件时跳过。
 */
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'
import { createAppMcp, ToolCallError, type AppMcp } from './index.js'
import { loadNativeBinding, nativeFileName, type NativeBinding } from './native.js'

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repoRoot = resolve(pkgDir, '..', '..')
const nativePath = process.env.APP_MCP_NODE_NATIVE ?? join(pkgDir, 'native', nativeFileName())
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(repoRoot, process.env.CARGO_TARGET_DIR) : join(repoRoot, 'target')
const exe = process.platform === 'win32' ? 'fake_host.exe' : 'fake_host'

function findFakeHost(): string | undefined {
  if (process.env.APP_MCP_FAKE_HOST) return process.env.APP_MCP_FAKE_HOST
  if (!existsSync(join(repoRoot, 'crates', 'native', 'examples', 'fake_host.rs'))) return undefined
  const r = spawnSync('cargo', ['build', '-q', '-p', 'app-mcp-native', '--example', 'fake_host'], {
    cwd: repoRoot,
    stdio: 'inherit',
  })
  const path = join(targetDir, 'debug', 'examples', exe)
  return r.status === 0 && existsSync(path) ? path : undefined
}

interface HostLine {
  type: string
  name?: string
  result?: unknown
  error?: { code: number; message: string; data?: { kind?: string } }
  tools?: string[]
  resources?: string[]
}

interface FakeHost {
  url: string
  lines: HostLine[]
  exit: Promise<number | null>
  stderr: string[]
}

async function startHost(bin: string, args: string[]): Promise<FakeHost> {
  const child: ChildProcessWithoutNullStreams = spawn(bin, ['--addr', '127.0.0.1:0', '--timeout-ms', '15000', ...args])
  running.push(child)
  const lines: HostLine[] = []
  const stderr: string[] = []
  child.stderr.on('data', (d: Buffer) => stderr.push(d.toString()))
  const exit = new Promise<number | null>((r) => child.on('exit', (code) => r(code)))
  const rl = createInterface({ input: child.stdout })
  const url = await new Promise<string>((resolveUrl, reject) => {
    rl.on('line', (line) => {
      if (line.startsWith('LISTENING ')) {
        resolveUrl(`ws://${line.slice('LISTENING '.length).trim()}`)
        return
      }
      if (line.trim()) lines.push(JSON.parse(line) as HostLine)
    })
    child.on('exit', (code) => reject(new Error(`fake_host 提前退出（${code}）：${stderr.join('')}`)))
  })
  return { url, lines, exit, stderr }
}

const running: ChildProcessWithoutNullStreams[] = []
const apps: AppMcp[] = []

let binding: NativeBinding | undefined
let fakeHost: string | undefined

beforeAll(() => {
  if (!existsSync(nativePath)) return
  binding = loadNativeBinding()
  fakeHost = findFakeHost()
}, 600_000)

afterEach(() => {
  for (const app of apps.splice(0)) app.dispose()
  for (const child of running.splice(0)) child.kill()
})

describe.skipIf(!existsSync(nativePath))('真实原生模块', () => {
  it('注册错误带 code', () => {
    const app = createAppMcp({ appId: 'demo', appName: 'Demo', binding: binding!, autoStart: false, keepAlive: false })
    apps.push(app)
    expect(app.state).toEqual({ status: 'idle' })
    expect(app.instanceId).not.toBe('')
    app.tool('a', { description: 'a', handler: () => 1 })
    expect(() => app.tool('a', { description: 'a', handler: () => 1 })).toThrow(
      expect.objectContaining({ code: 'DUPLICATE_NAME' }),
    )
    expect(() => app.tool('bad name!', { description: 'a', handler: () => 1 })).toThrow(
      expect.objectContaining({ code: 'INVALID_NAME' }),
    )
    expect(() => app.tool('r', { description: 'a', risk: 'nope' as never, handler: () => 1 })).toThrow(
      expect.objectContaining({ code: 'INVALID_ARG' }),
    )
    const scope = app.scope('page')
    scope.tool('b', { description: 'b', handler: () => 1 })
    scope.dispose()
    app.tool('b', { description: 'b', handler: () => 1 }) // 随 scope 注销后可重新注册
  })

  it('原生回调（weak ThreadsafeFunction）不阻止进程退出', () => {
    // 子进程：创建客户端、注册工具 / 资源 / 监听器并开始连接一个不存在的地址，不调用 stop。
    const script = `
      const b = require(${JSON.stringify(nativePath)})
      const c = new b.NativeClient({ appId: 'demo', appName: 'Demo', hostUrl: 'ws://127.0.0.1:9' }, () => {})
      c.registerTool({ name: 't', description: 't' }, () => {})
      c.registerResource({ name: 'r', description: 'r' }, () => {})
      c.start()
      setTimeout(() => console.log('tick'), 50)
    `
    const r = spawnSync(process.execPath, ['-e', script], { timeout: 10_000, encoding: 'utf8' })
    expect(r.error).toBeUndefined()
    expect(r.status).toBe(0)
    expect(r.stdout).toContain('tick')
  })

  describe('与 fake_host 往返', () => {
    it('调用工具、错误类别、资源读取、stateHints', async (ctx) => {
      if (!fakeHost) ctx.skip()
      const host = await startHost(fakeHost!, [
        '--invoke', 'math.add', '--args', '{"a":2,"b":3}',
        '--invoke', 'cart.add', '--args', '{}',
        '--invoke', 'fail.reject', '--args', '{}',
        '--invoke', 'fail.boom', '--args', '{}',
        '--read', 'cart',
      ])
      const paired: string[] = []
      const app = createAppMcp({
        appId: 'demo',
        appName: 'Demo',
        binding: binding!,
        hostUrl: host.url,
        overview: { summary: '测试 App' },
        autoStart: false,
        onPaired: (t) => paired.push(t),
      })
      apps.push(app)
      app.tool('math.add', {
        description: '加法',
        risk: 'read',
        input: {
          type: 'object',
          properties: { a: { type: 'number' }, b: { type: 'number' } },
          required: ['a', 'b'],
        },
        handler: async ({ a, b }: { a: number; b: number }) => ({ sum: a + b }),
      })
      const scope = app.scope('cart')
      scope.tool('cart.add', { description: '加入购物车', handler: () => ({ data: { count: 1 }, stateHints: ['cart'] }) })
      scope.resource('cart', { description: '购物车', read: () => ({ items: 1 }) })
      app.tool('fail.reject', {
        description: 'r',
        handler: () => {
          throw new ToolCallError('USER_REJECTED', '用户拒绝')
        },
      })
      app.tool('fail.boom', {
        description: 'b',
        handler: () => {
          throw new Error('炸了')
        },
      })
      const states: string[] = []
      app.onStateChange((s) => states.push(s.status))
      app.start()

      expect(await host.exit).toBe(0)
      const byType = (t: string) => host.lines.filter((l) => l.type === t)
      const tools = byType('tools')[0]
      expect(tools?.tools).toEqual(expect.arrayContaining(['math.add', 'cart.add', 'fail.reject', 'fail.boom']))
      expect(tools?.resources).toEqual(['cart'])
      const invokes = byType('invoke')
      expect(invokes[0]).toMatchObject({ name: 'math.add', result: { data: { sum: 5 } } })
      expect(invokes[1]).toMatchObject({ name: 'cart.add', result: { data: { count: 1 }, stateHints: ['cart'] } })
      expect(invokes[2]).toMatchObject({ name: 'fail.reject', error: { message: '用户拒绝', data: { kind: 'USER_REJECTED' } } })
      expect(invokes[3]).toMatchObject({ name: 'fail.boom', error: { message: '炸了', data: { kind: 'HANDLER_ERROR' } } })
      expect(byType('read')[0]).toMatchObject({ name: 'cart', result: { contents: { items: 1 } } })
      expect(states).toContain('connected')
      expect(paired).toEqual(['fake-token'])
      expect(app.token).toBe('fake-token')
    })
  })
})
