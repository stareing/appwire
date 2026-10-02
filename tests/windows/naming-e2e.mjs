// Windows 按名寻址端到端测试（spec/naming.md 4.3；TASKS 4d-E；用 Windows 的 node 运行，见 tests/windows/README.md）：
//   1) `app-mcp-host app install` 写 App 登记文件（%LOCALAPPDATA%\app-mcp\apps\<appId>.json，exec 激活）
//   2) Host（stdio，--name-service）不启动 App 即列出其工具（清单副本 <home>/manifests）
//   3) 调用 → Host 经命名管道按名拨号，管道不存在 → 运行登记的程序（--app-mcp-activation）冷启动 → 调用成功
//   4) 宽限内再调用合并进同一通道；宽限 + 租约后 Host 关闭通道 → App（由激活启动）退出、管道消失
//   5) 再次调用 → 重新激活（新进程）
//   6) `app uninstall` 删除登记 → Host 经目录通知移除发现记录
//
// 用法：node tests\windows\naming-e2e.mjs
// 需要：target\win\debug\app-mcp-host.exe、target\win\debug\examples\named_app.exe：
//   cargo build -p app-mcp-host && cargo build -p app-mcp-native --example named_app（CARGO_TARGET_DIR=target\win）
// 只写 %LOCALAPPDATA%\app-mcp\apps\<随机 appId>.json（结束时 app uninstall 删除）与 target\win\naming-wintest。

import { spawn, spawnSync } from 'node:child_process'
import crypto from 'node:crypto'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repo, 'target', 'win')
const hostExe = process.env.APP_MCP_HOST_EXE ?? path.join(targetDir, 'debug', 'app-mcp-host.exe')
const appExe = process.env.NAMED_APP_EXE ?? path.join(targetDir, 'debug', 'examples', 'named_app.exe')
const work = path.join(targetDir, 'naming-wintest')
const home = path.join(work, 'home')
const hostLog = path.join(work, 'host.log')
const eventLog = path.join(work, 'events.log')
const appId = `naming-wintest-${crypto.randomBytes(4).toString('hex')}`
const GRACE_MS = 500
const LEASE_MS = 1000

const results = []
const out = (s) => process.stdout.write(`${s}\n`)
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
function record(name, ok, detail) {
  results.push({ name, ok, detail })
  out(`${ok ? 'PASS' : 'FAIL'} ${name} ${detail ?? ''}`)
}

async function waitFor(pred, timeoutMs, what) {
  const end = Date.now() + timeoutMs
  while (Date.now() < end) {
    const v = await pred()
    if (v) return v
    await sleep(100)
  }
  throw new Error(`等待超时：${what}`)
}

const events = () => (fs.existsSync(eventLog) ? fs.readFileSync(eventLog, 'utf8').split(/\r?\n/).filter(Boolean) : [])
const count = (kind) => events().filter((l) => l.startsWith(kind)).length

// 管道是否存在：只列目录，不连接（PowerShell 的 \\.\pipe\ 枚举仅供人工排查，spec/naming.md U-06；测试中用来观察）。
function pipeExists() {
  const r = spawnSync('powershell', ['-NoProfile', '-Command',
    `[bool](Get-ChildItem \\\\.\\pipe\\ | Where-Object Name -like 'appmcp-*-${appId}')`], { encoding: 'utf8' })
  return r.stdout.trim() === 'True'
}

function host(args, extraEnv = {}) {
  return spawnSync(hostExe, args, { encoding: 'utf8', env: { ...process.env, ...extraEnv } })
}

// ---------------------------------------------------------------- 最小 MCP 客户端（stdio）
class Mcp {
  constructor(args, env) {
    this.logFd = fs.openSync(hostLog, 'w')
    this.child = spawn(hostExe, args, { stdio: ['pipe', 'pipe', this.logFd], windowsHide: true, env })
    this.next = 1
    this.pending = new Map()
    let buf = ''
    this.child.stdout.setEncoding('utf8')
    this.child.stdout.on('data', (d) => {
      buf += d
      let i
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim()
        buf = buf.slice(i + 1)
        if (!line) continue
        const msg = JSON.parse(line)
        if (msg.id != null && this.pending.has(msg.id)) {
          const p = this.pending.get(msg.id)
          this.pending.delete(msg.id)
          msg.error ? p.reject(new Error(JSON.stringify(msg.error))) : p.resolve(msg.result)
        }
      }
    })
    this.child.on('exit', (code) => {
      for (const p of this.pending.values()) p.reject(new Error(`Host 退出 code=${code}`))
    })
  }
  request(method, params, timeoutMs = 60000) {
    const id = this.next++
    this.child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`)
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`${method} 超时`))
      }, timeoutMs)
    })
  }
  notify(method, params) {
    this.child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method, params })}\n`)
  }
  async init() {
    await this.request('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'naming-wintest', version: '0.1.0' } })
    this.notify('notifications/initialized', {})
  }
  async call(name, args) {
    const t0 = Date.now()
    const r = await this.request('tools/call', { name, arguments: args })
    return { r, ms: Date.now() - t0 }
  }
  close() {
    this.child.stdin.end()
    this.child.kill()
    fs.closeSync(this.logFd)
  }
}

const structured = (r) => r.structuredContent ?? JSON.parse(r.content?.[0]?.text ?? '{}')

async function main() {
  for (const f of [hostExe, appExe]) if (!fs.existsSync(f)) throw new Error(`找不到 ${f}`)
  fs.rmSync(work, { recursive: true, force: true })
  fs.mkdirSync(work, { recursive: true })
  const manifest = path.join(work, 'app-mcp.json')
  fs.writeFileSync(manifest, JSON.stringify({
    manifestVersion: 1, appId, name: '按名寻址 Windows 实测',
    tools: [
      { name: 'echo', description: '原样返回参数', inputSchema: { type: 'object' } },
      { name: 'pid', description: '返回进程号', inputSchema: { type: 'object' } },
    ],
  }, null, 2))

  // 1) app install（真实 %LOCALAPPDATA%）
  const inst = host(['app', 'install', '--app-id', appId, '--exec', appExe, '--manifest', manifest, '--home', home])
  const regFile = path.join(process.env.LOCALAPPDATA, 'app-mcp', 'apps', `${appId}.json`)
  const reg = fs.existsSync(regFile) ? JSON.parse(fs.readFileSync(regFile, 'utf8')) : null
  record('app install', inst.status === 0 && reg?.activation?.kind === 'exec' && reg?.executable?.toLowerCase() === appExe.toLowerCase(),
    `${regFile} ${JSON.stringify(reg?.activation)} exe=${reg?.executable}`)

  // App 由 Host 激活，继承 Host 的环境：appId 与事件日志经环境变量传给示例 App。
  const env = { ...process.env, APP_MCP_APP_ID: appId, APP_MCP_EVENT_LOG: eventLog }
  const mcp = new Mcp([
    '--home', home,
    '--ipc-endpoint', 'none',
    '--listen', '127.0.0.1:0',
    // 不用唤醒描述：证明激活只经名字服务。
    '--waker', 'none',
    '--name-service',
    '--channel-grace-ms', String(GRACE_MS),
    '--lease-ms', String(LEASE_MS),
    '--log-level', 'debug',
  ], env)
  try {
    await mcp.init()
    // 2) 发现不启动
    const tools = (await mcp.request('tools/list', {})).tools.map((t) => t.name)
    await sleep(500)
    record('列出而不启动', tools.includes(`${appId}.echo`) && count('start') === 0 && !pipeExists(),
      `tools=${tools.filter((t) => t.startsWith(appId)).join(',')} start=${count('start')}`)
    const list = structured((await mcp.call('apps.list', {})).r)
    const entry = (list.apps ?? []).find((a) => a.appId === appId)?.nameService
    record('apps.list nameService', entry?.source === 'pipe' && entry?.activatable === true && entry?.running === false, JSON.stringify(entry))

    // 3) 冷启动
    const first = await mcp.call(`${appId}.echo`, { x: 1 })
    const echo = structured(first.r)
    record('冷启动 + 调用', !first.r.isError && echo?.echo?.x === 1 && count('start') === 1, `${first.ms}ms ${JSON.stringify(echo)}`)
    // 4) 宽限内合并
    const warm = await mcp.call(`${appId}.pid`, {})
    const pid1 = structured(warm.r).pid
    record('宽限内复用通道', !warm.r.isError && count('start') === 1, `${warm.ms}ms pid=${pid1}`)
    const t0 = Date.now()
    await waitFor(() => count('exit') === 1, 20000, 'App 宽限后退出')
    await waitFor(() => !pipeExists(), 10000, '管道消失')
    record('宽限后关闭 → App 退出、管道消失', true, `${Date.now() - t0}ms（宽限 ${GRACE_MS} + 租约 ${LEASE_MS}）`)

    // 5) 再激活
    const again = await mcp.call(`${appId}.pid`, {})
    const pid2 = structured(again.r).pid
    record('再次调用再激活', !again.r.isError && count('start') === 2 && pid2 !== pid1, `${again.ms}ms pid=${pid2}（之前 ${pid1}）`)
    await waitFor(() => count('exit') === 2, 20000, '第二次宽限后退出')

    // 6) app uninstall → 目录通知移除记录
    const un = host(['app', 'uninstall', '--app-id', appId, '--home', home])
    const gone = await waitFor(async () => {
      const l = structured((await mcp.call('apps.list', {})).r)
      return !(l.apps ?? []).some((a) => a.appId === appId && a.nameService) || null
    }, 10000, '发现记录移除').catch(() => false)
    record('app uninstall → 记录移除', un.status === 0 && !fs.existsSync(regFile) && gone === true, un.stdout.trim().replace(/\r?\n/g, ' | '))
  } finally {
    mcp.close()
    if (fs.existsSync(regFile)) host(['app', 'uninstall', '--app-id', appId, '--home', home])
  }
  const failed = results.filter((r) => !r.ok)
  out(`\n${results.length - failed.length}/${results.length} 通过；Host 日志 ${hostLog}`)
  process.exitCode = failed.length ? 1 : 0
}

main().catch((e) => {
  out(`ERROR ${e.stack ?? e}`)
  process.exitCode = 1
})
