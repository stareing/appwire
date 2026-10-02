/**
 * 按名寻址 e2e（spec/naming.md，Linux）：私有 D-Bus 会话总线 + `app-mcp-host app install` 登记 +
 * `app-mcp-host stdio --name-service` 作为 Hub；`src/testing/named-app.mjs`（`registerName: true`）由 D-Bus 激活冷启动。
 *
 * 发现不激活 → 调用触发激活 → 宽限后通道关闭、App 进程退出 → 再次调用再激活（新进程）。
 *
 * 前置条件：原生模块（`build:native`）、dbus-daemon、cargo（或 APP_MCP_HOST_BIN 指定已构建的 app-mcp-host）；
 * 缺少任一条件时跳过。测试先用 tsup 构建 dist（被激活的 App 是独立 node 进程，加载 dist）。
 */
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { delimiter, dirname, join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'
import { nativeFileName } from './native.js'

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repoRoot = resolve(pkgDir, '..', '..')
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(repoRoot, process.env.CARGO_TARGET_DIR) : join(repoRoot, 'target')
const nativePath = process.env.APP_MCP_NODE_NATIVE ?? join(pkgDir, 'native', nativeFileName())
const APP = 'node-named-e2e'
const TIMEOUT_MS = 20_000
const GRACE_MS = 300

const SESSION_CONF = (socket: string) => `<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=${socket}</listen>
  <auth>EXTERNAL</auth>
  <standard_session_servicedirs/>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
`

function findInPath(name: string): string | undefined {
  return (process.env.PATH ?? '').split(delimiter).map((d) => join(d, name)).find((p) => existsSync(p))
}

function findHost(): string | undefined {
  if (process.env.APP_MCP_HOST_BIN) return process.env.APP_MCP_HOST_BIN
  const r = spawnSync('cargo', ['build', '-q', '-p', 'app-mcp-host'], { cwd: repoRoot, stdio: 'inherit' })
  const path = join(targetDir, 'debug', 'app-mcp-host')
  return r.status === 0 && existsSync(path) ? path : undefined
}

/** 构建 dist（被激活的 App 加载它）；tsup 在仓库根 node_modules。 */
function buildDist(): boolean {
  const tsup = join(repoRoot, 'node_modules', '.bin', 'tsup')
  return existsSync(tsup) && spawnSync(tsup, [], { cwd: pkgDir, stdio: 'ignore' }).status === 0
}

/** 去掉用户会话总线地址：测试只用私有总线。 */
function baseEnv(): NodeJS.ProcessEnv {
  return Object.fromEntries(
    Object.entries(process.env).filter(([k]) => !k.startsWith('DBUS_') && k !== 'APP_MCP_HOME'),
  )
}

interface Bus {
  address: string
  dataHome: string
  child: ChildProcessWithoutNullStreams
}

async function startBus(root: string, daemon: string, appEnv: Record<string, string>): Promise<Bus> {
  const dataHome = join(root, 'data')
  mkdirSync(join(dataHome, 'dbus-1', 'services'), { recursive: true })
  for (const d of ['sys', 'run']) mkdirSync(join(root, d))
  const conf = join(root, 'session.conf')
  writeFileSync(conf, SESSION_CONF(join(root, 'bus')))
  // @why 被激活的 App 继承总线进程的环境（APP_MCP_* 与临时 XDG 目录）。
  const child = spawn(daemon, [`--config-file=${conf}`, '--nofork', '--print-address=1'], {
    env: { ...baseEnv(), ...appEnv, XDG_DATA_HOME: dataHome, XDG_DATA_DIRS: join(root, 'sys'), XDG_RUNTIME_DIR: join(root, 'run') },
  })
  const address = await new Promise<string>((ok, fail) => {
    createInterface({ input: child.stdout }).once('line', (line) => ok(line.trim()))
    child.once('exit', (code) => fail(new Error(`dbus-daemon 提前退出（${code}）`)))
  })
  return { address, dataHome, child }
}

/** `app-mcp-host stdio` 的最小 MCP 客户端（换行分隔的 JSON-RPC）。 */
class StdioMcp {
  private nextId = 0
  private readonly pending = new Map<number, (msg: Record<string, unknown>) => void>()
  readonly stderr: string[] = []

  constructor(readonly child: ChildProcessWithoutNullStreams) {
    child.stderr.on('data', (d: Buffer) => this.stderr.push(d.toString()))
    createInterface({ input: child.stdout }).on('line', (line) => {
      if (!line.trim()) return
      const msg = JSON.parse(line) as Record<string, unknown>
      if (typeof msg.id === 'number' && ('result' in msg || 'error' in msg)) this.pending.get(msg.id)?.(msg)
    })
  }

  private send(msg: Record<string, unknown>): void {
    this.child.stdin.write(`${JSON.stringify(msg)}\n`)
  }

  async request(method: string, params: Record<string, unknown>): Promise<Record<string, unknown>> {
    const id = ++this.nextId
    const reply = new Promise<Record<string, unknown>>((ok, fail) => {
      const timer = setTimeout(() => fail(new Error(`${method} 超时：${this.stderr.join('').slice(-2000)}`)), TIMEOUT_MS)
      this.pending.set(id, (msg) => {
        clearTimeout(timer)
        this.pending.delete(id)
        ok(msg)
      })
    })
    this.send({ jsonrpc: '2.0', id, method, params })
    const msg = await reply
    expect(msg.error, JSON.stringify(msg)).toBeUndefined()
    return msg.result as Record<string, unknown>
  }

  async initialize(): Promise<void> {
    await this.request('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'node-e2e', version: '0' },
    })
    this.send({ jsonrpc: '2.0', method: 'notifications/initialized' })
  }

  async call(name: string, args: Record<string, unknown>): Promise<Record<string, unknown>> {
    const result = await this.request('tools/call', { name, arguments: args })
    expect(result.isError, JSON.stringify(result)).not.toBe(true)
    if (result.structuredContent) return result.structuredContent as Record<string, unknown>
    const content = result.content as { text: string }[]
    return JSON.parse(content[0]!.text) as Record<string, unknown>
  }
}

async function eventually(what: string, pred: () => boolean | Promise<boolean>): Promise<void> {
  const deadline = Date.now() + TIMEOUT_MS
  while (!(await pred())) {
    if (Date.now() > deadline) throw new Error(`等待超时：${what}`)
    await new Promise((r) => setTimeout(r, 20))
  }
}

function events(log: string): [string, number][] {
  if (!existsSync(log)) return []
  return readFileSync(log, 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const [kind, pid] = line.split(' ')
      return [kind!, Number(pid)]
    })
}

const count = (log: string, kind: string) => events(log).filter(([k]) => k === kind).length

let hostBin: string | undefined
let dbusDaemon: string | undefined
let ready = false
const cleanups: (() => void)[] = []

beforeAll(() => {
  if (process.platform !== 'linux' || !existsSync(nativePath)) return
  dbusDaemon = findInPath('dbus-daemon')
  if (!dbusDaemon) return
  hostBin = findHost()
  ready = hostBin !== undefined && buildDist()
}, 600_000)

afterEach(() => {
  for (const c of cleanups.splice(0).reverse()) c()
})

describe('按名寻址（Linux D-Bus 激活）', () => {
  it('registerName 的 Node App 由 Hub 按名激活、宽限后退出、再次调用再激活', async (ctx) => {
    if (!ready || !hostBin || !dbusDaemon) {
      ctx.skip()
      return
    }
    const root = mkdtempSync(join(tmpdir(), 'app-mcp-node-naming-'))
    const log = join(root, 'events.log')
    cleanups.push(() => rmSync(root, { recursive: true, force: true }))
    // 失败路径清理：结束已启动、未记录退出的 App 进程（激活启动的进程不是本测试的子进程）。
    cleanups.push(() => {
      const live = new Set<number>()
      for (const [kind, pid] of events(log)) {
        if (kind === 'start') live.add(pid)
        else live.delete(pid)
      }
      for (const pid of live) {
        try {
          process.kill(pid, 'SIGKILL')
        } catch {
          // 已退出
        }
      }
    })

    const bus = await startBus(join(root, 'bus'), dbusDaemon, { APP_MCP_EVENT_LOG: log, APP_MCP_APP_ID: APP })
    cleanups.push(() => bus.child.kill('SIGKILL'))

    // 激活时运行的程序：固定本测试的 node 运行 named-app.mjs（D-Bus 追加 --app-mcp-activation）。
    const launcher = join(root, 'named-app')
    writeFileSync(
      launcher,
      `#!/bin/sh\nexec ${JSON.stringify(process.execPath)} ${JSON.stringify(join(pkgDir, 'src', 'testing', 'named-app.mjs'))} "$@"\n`,
    )
    chmodSync(launcher, 0o755)
    const manifest = join(root, 'app-mcp.json')
    writeFileSync(
      manifest,
      JSON.stringify({
        manifestVersion: 1,
        appId: APP,
        name: '按名寻址 e2e（Node）',
        tools: [
          { name: 'echo', description: '原样返回 text', inputSchema: { type: 'object' } },
          { name: 'pid', description: '返回进程号', inputSchema: { type: 'object' } },
        ],
      }),
    )
    const home = join(root, 'home')
    const env = { ...baseEnv(), DBUS_SESSION_BUS_ADDRESS: bus.address }
    const install = spawnSync(
      hostBin,
      ['app', 'install', '--app-id', APP, '--exec', launcher, '--manifest', manifest, '--home', home, '--data-home', bus.dataHome],
      { env, encoding: 'utf8', timeout: TIMEOUT_MS },
    )
    expect(install.status, install.stderr).toBe(0)

    const hostArgs = ['stdio', '--home', home, '--listen', '127.0.0.1:0', '--ipc-endpoint', 'none', '--waker', 'none']
    hostArgs.push('--name-service', '--channel-grace-ms', String(GRACE_MS), '--lease-ms', '0')
    const mcp = new StdioMcp(spawn(hostBin, hostArgs, { env }))
    cleanups.push(() => mcp.child.kill('SIGKILL'))
    await mcp.initialize()

    // 1. 发现不激活。
    const namedEntry = async () => {
      const { apps } = (await mcp.call('apps.list', {})) as { apps: { appId: string; nameService?: Record<string, unknown> }[] }
      return apps.find((a) => a.appId === APP)?.nameService ?? null
    }
    await eventually('发现记录', async () => (await namedEntry()) !== null)
    expect(await namedEntry()).toMatchObject({ activatable: true, running: false })
    await new Promise((r) => setTimeout(r, 200))
    expect(count(log, 'start'), '发现不得启动 App').toBe(0)

    // 2. 调用触发激活冷启动。
    expect(await mcp.call(`${APP}.echo`, { text: '你好' })).toEqual({ echo: '你好' })
    expect(count(log, 'start')).toBe(1)
    const firstPid = (await mcp.call(`${APP}.pid`, {})).pid
    expect(count(log, 'start'), '宽限内的调用合并进同一通道').toBe(1)

    // 3. 宽限后通道关闭，激活启动的 App 退出。
    await eventually('App 进程退出', () => count(log, 'exit') === 1)

    // 4. 再次调用再激活（新进程）。
    const secondPid = (await mcp.call(`${APP}.pid`, {})).pid
    expect(count(log, 'start')).toBe(2)
    expect(secondPid).not.toBe(firstPid)
    await eventually('第二次宽限后退出', () => count(log, 'exit') === 2)
  }, 120_000)
})
