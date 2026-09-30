/**
 * 测试环境：app-mcp-host serve（常驻，Streamable HTTP MCP；独立配置目录与端口）+ shop Demo（vite dev，独立端口）+ 无头 Chromium。
 */
import { type ChildProcess, spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { Browser } from './browser'
import { McpClient } from './mcp-client'
import { E2E_ROOT, SHOP_DIR, SHOP_MANIFEST } from './paths'
import { freePort, lineSplitter, waitFor } from './util'

export interface ShopServer {
  url: string
  port: number
  stop(): Promise<void>
}

/** 启动 shop 的 vite dev 服务器（VITE_* 环境变量在启动时生效）。 */
export async function startShop(env: Record<string, string>): Promise<ShopServer> {
  const port = await freePort()
  const vite = resolve(SHOP_DIR, 'node_modules/vite/bin/vite.js')
  const log: string[] = []
  const child = spawn(process.execPath, [vite, '--port', String(port), '--strictPort', '--host', '127.0.0.1'], {
    cwd: SHOP_DIR,
    env: { ...process.env, ...env, BROWSER: 'none' },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  const collect = lineSplitter((line) => {
    log.push(line)
    if (process.env.E2E_VITE_LOG === '1') process.stderr.write(`[vite] ${line}\n`)
  })
  child.stdout!.on('data', collect)
  child.stderr!.on('data', collect)
  const url = `http://127.0.0.1:${port}/`
  try {
    await waitFor(async () => {
      if (child.exitCode !== null) throw new Error(`vite 已退出：\n${log.join('\n')}`)
      const r = await fetch(url)
      return r.ok
    }, `vite dev 启动（${url}）`, 60_000, 200)
    // 预热入口模块，减少首次打开页面时的依赖预构建与重载
    await fetch(`${url}src/main.tsx`).catch(() => undefined)
  } catch (error) {
    await kill(child)
    throw error
  }
  return {
    url,
    port,
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
 * 隔离：临时配置目录（`--home`，不读取 ~/.app-mcp 的配置、令牌、清单）、随机的 WebSocket 与 HTTP 端口，
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
  const wsPort = await freePort()
  const httpPort = await freePort()
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
      '--ws-addr',
      `127.0.0.1:${wsPort}`,
      '--http',
      `127.0.0.1:${httpPort}`,
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
  const base = `http://127.0.0.1:${httpPort}`
  try {
    await waitFor(async () => {
      if (child.exitCode !== null) throw new Error(`Host 已退出（code=${child.exitCode}）：\n${log()}`)
      const r = await fetch(`${base}/healthz`)
      const h = (await r.json()) as { service?: string; pid?: number }
      return h.service === 'app-mcp' && h.pid === child.pid
    }, `app-mcp-host serve 就绪（${base}/healthz）`, 20_000, 50)
  } catch (error) {
    await kill(child)
    rmSync(dir, { recursive: true, force: true })
    throw error
  }
  const mcpUrl = `${base}/mcp`
  const mcp = new McpClient({ url: mcpUrl, diagnostics: () => log() })
  await mcp.initialize()
  return {
    mcp,
    wsUrl: `ws://127.0.0.1:${wsPort}`,
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
