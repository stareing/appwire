/**
 * 测试环境：app-mcp-host serve（常驻，Streamable HTTP MCP；独立配置目录与端口）+ shop Demo（vite dev，独立端口）+ 无头 Chromium。
 */
import { type ChildProcess, spawn } from 'node:child_process'
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { Browser } from './browser'
import { McpClient } from './mcp-client'
import { SHOP_DIR, SHOP_MANIFEST } from './paths'
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
  /** 替身 `xdg-open` 记录的唤醒 URL（Host 的 web-url 唤醒会执行 `xdg-open <url>#app-mcp-wake=<令牌>`）。 */
  wakeUrls(): string[]
  /** 最近的 Host 日志（stderr）。 */
  log(lines?: number): string
  stop(): Promise<void>
}

/**
 * 启动常驻 Host（`app-mcp-host serve`），再以 Streamable HTTP 建立一个 MCP 会话。
 *
 * 隔离：临时配置目录（`--home`，不读取 ~/.app-mcp 的配置、令牌、清单）、随机的 WebSocket 与 HTTP 端口，
 * 不会与本机可能在运行的 Host 实例冲突。只加载 shop 的清单（`--manifest-dir` 指向空目录）。
 * PATH 前置一个替身 `xdg-open`：Host 执行唤醒命令时只把 URL 记到文件，由测试决定如何交给页面。
 */
export interface HostStartOptions {
  /** 去掉清单中的 `wake`（M1 行为：App 未运行时调用返回 APP_DISCONNECTED + 启动地址，不尝试唤醒）。 */
  stripManifestWake?: boolean
}

export async function startHost(bin: string, extraArgs: string[] = [], options: HostStartOptions = {}): Promise<HostHandle> {
  const wsPort = await freePort()
  const httpPort = await freePort()
  const dir = mkdtempSync(join(tmpdir(), 'app-mcp-e2e-host-'))
  const home = join(dir, 'home')
  const manifestDir = join(dir, 'manifests')
  const binDir = join(dir, 'bin')
  const wakeLog = join(dir, 'wake.log')
  for (const d of [home, manifestDir, binDir]) mkdirSync(d, { recursive: true })
  writeFileSync(wakeLog, '')
  // 使用清单的副本：vite dev 会重写 examples/shop/app-mcp.json
  const manifest = JSON.parse(readFileSync(SHOP_MANIFEST, 'utf8')) as Record<string, unknown>
  if (options.stripManifestWake) delete manifest.wake
  const manifestFile = join(dir, 'shop.json')
  writeFileSync(manifestFile, JSON.stringify(manifest, null, 2))
  for (const name of ['xdg-open', 'open']) {
    const stub = join(binDir, name)
    writeFileSync(stub, `#!/bin/sh\nprintf '%s\\n' "$*" >> "${wakeLog}"\n`)
    chmodSync(stub, 0o755)
  }
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
      manifestFile,
      '--manifest-dir',
      manifestDir,
      '--no-log-file',
      ...extraArgs,
    ],
    {
      env: { ...process.env, APP_MCP_HOME: home, PATH: `${binDir}:${process.env.PATH ?? ''}`, RUST_LOG: process.env.RUST_LOG ?? 'info' },
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
    wakeUrls: () =>
      readFileSync(wakeLog, 'utf8')
        .split('\n')
        .filter((l) => l.trim()),
    log,
    async stop() {
      await mcp.close()
      await kill(child)
      rmSync(dir, { recursive: true, force: true })
    },
  }
}

export { Browser }
