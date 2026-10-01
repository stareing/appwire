/**
 * 测试环境：app-mcp-host serve（常驻，Streamable HTTP MCP；独立配置目录与端口）+ shop Demo（vite dev，独立端口）+ 无头 Chromium。
 */
import { type ChildProcess, spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { Browser } from './browser'
import { McpClient } from './mcp-client'
import { E2E_ROOT, SHOP_DIR, SHOP_MANIFEST } from './paths'
import { lineSplitter, waitFor } from './util'

export interface ShopServer {
  url: string
  port: number
  stop(): Promise<void>
}

/** 启动 shop 的 vite dev 服务器（VITE_* 环境变量在启动时生效）。端口 0，实际地址取自 vite 输出的 `Local:` 行。 */
export async function startShop(env: Record<string, string>): Promise<ShopServer> {
  const vite = resolve(SHOP_DIR, 'node_modules/vite/bin/vite.js')
  const log: string[] = []
  let url: string | undefined
  const child = spawn(process.execPath, [vite, '--port', '0', '--strictPort', '--host', '127.0.0.1'], {
    cwd: SHOP_DIR,
    env: { ...process.env, ...env, BROWSER: 'none', NO_COLOR: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  const collect = lineSplitter((line) => {
    log.push(line)
    const m = /Local:\s+(http:\/\/127\.0\.0\.1:\d+\/)/.exec(line.replace(/\x1b\[[0-9;]*m/g, ''))
    if (m?.[1] && url === undefined) url = m[1]
    if (process.env.E2E_VITE_LOG === '1') process.stderr.write(`[vite] ${line}\n`)
  })
  child.stdout!.on('data', collect)
  child.stderr!.on('data', collect)
  try {
    await waitFor(async () => {
      if (child.exitCode !== null) throw new Error(`vite 已退出：\n${log.join('\n')}`)
      if (url === undefined) return false
      const r = await fetch(url)
      return r.ok
    }, 'vite dev 启动', 60_000, 200)
    // 预热入口模块，减少首次打开页面时的依赖预构建与重载
    await fetch(`${url}src/main.tsx`).catch(() => undefined)
  } catch (error) {
    await kill(child)
    throw error
  }
  const ready = url as string
  return {
    url: ready,
    port: Number(new URL(ready).port),
    stop: () => kill(child),
  }
}

async function kill(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return
  const exited = new Promise((r) => child.once('exit', r))
  child.kill('SIGTERM')
  const timer = setTimeout(() => child.kill('SIGKILL'), 3000)
  await exited
  clearTimeout(timer)
}

export interface HostHandle {
  /** 连接到该 Host 的 MCP 会话（Streamable HTTP）。 */
  mcp: McpClient
  /** App 连接地址 `ws://127.0.0.1:<port>/app`（与 MCP 同一端口）。 */
  wsUrl: string
  /** MCP 端点 `http://127.0.0.1:<port>/mcp`。 */
  mcpUrl: string
  /** `waker: record` 时，Host 交给唤醒程序的 WakeRequest（按时间顺序）；`waker: none` 时恒为空。 */
  wakeRequests(): WakeRequest[]
  /** 最近的 Host 日志（stderr）。 */
  log(lines?: number): string
  stop(): Promise<void>
}

/** Host 交给唤醒程序的请求（spec/hub-api.md 3.5 `WakeRequest`）。 */
export interface WakeRequest {
  appId: string
  instanceId: string | null
  descriptor: { kind: string; target?: string; background?: boolean }
  token: string
  activationArg: string
}

/**
 * 启动常驻 Host（`app-mcp-host serve`），再以 Streamable HTTP 建立一个 MCP 会话。
 *
 * 隔离：临时配置目录（`--home`，不读取 ~/.app-mcp 的配置、令牌、清单；单实例锁与登记文件也在其中）、
 * 监听端口 0（实际地址读登记文件 `<home>/run/endpoints.json`）、临时目录中的本地 IPC 端点（`--ipc-endpoint`），
 * 不会与本机可能在运行的 Host 实例冲突。只加载 shop 的清单（`--manifest-dir` 指向空目录）。
 */
export interface HostStartOptions {
  /**
   * 唤醒器（Host 的 `--waker`）：
   * - `none`：不唤醒，App 未运行 / 休眠时调用返回 APP_DISCONNECTED + 启动地址（M1 行为）；
   * - `record`（默认）：`{"exec": [node, src/record-wake.mjs, <日志>]}`，只记录 WakeRequest，由测试决定如何交给页面。
   */
  waker?: 'none' | 'record'
}

export async function startHost(bin: string, extraArgs: string[] = [], options: HostStartOptions = {}): Promise<HostHandle> {
  const dir = mkdtempSync(join(tmpdir(), 'app-mcp-e2e-host-'))
  const home = join(dir, 'home')
  const manifestDir = join(dir, 'manifests')
  const wakeLog = join(dir, 'wake.jsonl')
  for (const d of [home, manifestDir]) mkdirSync(d, { recursive: true })
  writeFileSync(wakeLog, '')
  const waker =
    options.waker === 'none'
      ? 'none'
      : JSON.stringify({ exec: [process.execPath, join(E2E_ROOT, 'src/record-wake.mjs'), wakeLog] })
  const child = spawn(
    bin,
    [
      'serve',
      '--home',
      home,
      '--listen',
      '127.0.0.1:0',
      '--ipc-endpoint',
      process.platform === 'win32'
        ? `pipe:\\\\.\\pipe\\app-mcp-e2e-${basename(dir)}`
        : `unix:${join(dir, 'run', 'hub.sock')}`,
      '--manifest',
      SHOP_MANIFEST,
      '--manifest-dir',
      manifestDir,
      '--no-log-file',
      '--waker',
      waker,
      ...extraArgs,
    ],
    {
      env: { ...process.env, APP_MCP_HOME: home, RUST_LOG: process.env.RUST_LOG ?? 'info' },
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  )
  const logLines: string[] = []
  const echo = process.env.E2E_HOST_LOG === '1'
  const collect = lineSplitter((line) => {
    logLines.push(line)
    if (logLines.length > 500) logLines.shift()
    if (echo) process.stderr.write(`[host] ${line}\n`)
  })
  child.stdout!.on('data', collect)
  child.stderr!.on('data', collect)
  const log = (lines = 60) => logLines.slice(-lines).join('\n')
  const registry = join(home, 'run', 'endpoints.json')
  let listen: string | undefined
  try {
    await waitFor(async () => {
      if (child.exitCode !== null) throw new Error(`Host 已退出（code=${child.exitCode}）：\n${log()}`)
      let reg: { pid?: number; listen?: string }
      try {
        reg = JSON.parse(readFileSync(registry, 'utf8')) as { pid?: number; listen?: string }
      } catch {
        return false
      }
      if (reg.pid !== child.pid || !reg.listen) return false
      const h = (await (await fetch(`http://${reg.listen}/healthz`)).json()) as { service?: string; pid?: number }
      if (h.service !== 'app-mcp' || h.pid !== child.pid) return false
      listen = reg.listen
      return true
    }, `app-mcp-host serve 就绪（${registry}）`, 20_000, 50)
  } catch (error) {
    await kill(child)
    rmSync(dir, { recursive: true, force: true })
    throw error
  }
  const base = `http://${listen}`
  const mcpUrl = `${base}/mcp`
  const mcp = new McpClient({ url: mcpUrl, diagnostics: () => log() })
  await mcp.initialize()
  return {
    mcp,
    wsUrl: `ws://${listen}/app`,
    mcpUrl,
    wakeRequests: () =>
      readFileSync(wakeLog, 'utf8')
        .split('\n')
        .filter((l) => l.trim())
        .map((l) => JSON.parse(l) as WakeRequest),
    log,
    async stop() {
      await mcp.close()
      await kill(child)
      rmSync(dir, { recursive: true, force: true })
    },
  }
}

export { Browser }
