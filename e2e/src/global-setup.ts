/**
 * 全局准备：定位（必要时构建）app-mcp-host 可执行文件与无头 Chromium，经 `provide` 交给测试。
 *
 * - Host：`APP_MCP_HOST_BIN` 指定时直接使用；否则运行 `cargo build -p app-mcp-host`（遵循 `CARGO_TARGET_DIR`），
 *   从 cargo 的 JSON 输出取可执行文件路径（已是最新时只需几秒）。
 * - Chromium：`CHROME_BIN` 指定时直接使用；否则在 `~/.cache/ms-playwright`（或 `PLAYWRIGHT_BROWSERS_PATH`）中查找
 *   `chromium_headless_shell-*` / `chromium-*`。
 */
import { spawnSync } from 'node:child_process'
import { existsSync, readdirSync } from 'node:fs'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'
import type { TestProject } from 'vitest/node'
import { REPO_ROOT } from './paths'

declare module 'vitest' {
  export interface ProvidedContext {
    hostBin: string
    chromeBin: string
  }
}

function findHostBin(): string {
  const fromEnv = process.env.APP_MCP_HOST_BIN
  if (fromEnv) {
    if (!existsSync(fromEnv)) throw new Error(`APP_MCP_HOST_BIN 不存在：${fromEnv}`)
    return fromEnv
  }
  const r = spawnSync('cargo', ['build', '-p', 'app-mcp-host', '--bin', 'app-mcp-host', '--message-format=json'], {
    cwd: REPO_ROOT,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
    maxBuffer: 64 * 1024 * 1024,
  })
  if (r.status !== 0) throw new Error('cargo build -p app-mcp-host 失败（可设置 APP_MCP_HOST_BIN 指定已构建的可执行文件）')
  let executable: string | undefined
  for (const line of r.stdout.split('\n')) {
    if (!line.startsWith('{')) continue
    const msg = JSON.parse(line) as { reason?: string; target?: { name?: string }; executable?: string | null }
    if (msg.reason === 'compiler-artifact' && msg.target?.name === 'app-mcp-host' && msg.executable) {
      executable = msg.executable
    }
  }
  if (!executable) throw new Error('cargo 输出中没有 app-mcp-host 可执行文件')
  return executable
}

function findChrome(): string {
  const fromEnv = process.env.CHROME_BIN
  if (fromEnv) {
    if (!existsSync(fromEnv)) throw new Error(`CHROME_BIN 不存在：${fromEnv}`)
    return fromEnv
  }
  const root = process.env.PLAYWRIGHT_BROWSERS_PATH || join(homedir(), '.cache', 'ms-playwright')
  const candidates: string[] = []
  let dirs: string[] = []
  try {
    dirs = readdirSync(root).sort().reverse()
  } catch {
    // 目录不存在
  }
  for (const d of dirs) {
    if (d.startsWith('chromium_headless_shell-')) {
      candidates.push(join(root, d, 'chrome-linux', 'headless_shell'), join(root, d, 'chrome-headless-shell-linux64', 'chrome-headless-shell'))
    }
  }
  for (const d of dirs) {
    if (/^chromium-\d+$/.test(d)) candidates.push(join(root, d, 'chrome-linux', 'chrome'))
  }
  const found = candidates.find((c) => existsSync(c))
  if (!found) throw new Error(`找不到 Chromium：请设置 CHROME_BIN，或用 Playwright 安装浏览器到 ${root}`)
  return resolve(found)
}

export default function setup(project: TestProject): void {
  project.provide('hostBin', findHostBin())
  project.provide('chromeBin', findChrome())
}
