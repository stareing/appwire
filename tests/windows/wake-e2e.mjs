// Windows 唤醒端到端测试（用 Windows 的 node 运行，见 tests/windows/README.md）：
//   a) App 未运行 → 调用工具 → Host 的 SystemWaker 以 `cmd /c start "" appmcp-wintest://…` 冷启动 → 回连 → 成功
//   b) App 运行中已休眠 → 调用 → 第二实例经单实例管道转交参数 → 快速恢复 → 成功 → 再休眠
//   c) web-url：rundll32 url.dll,FileProtocolHandler 打开本地 http 地址（页面回报令牌后 window.close()）
//   d) aumid：explorer.exe shell:AppsFolder\<计算器 AUMID>，确认启动后结束该进程
//
// 用法：node tests\windows\wake-e2e.mjs [a b c d]（缺省全部）
// 需要：target\win\debug\app-mcp-host.exe、target\win\dotnet\bin\WakeApp\debug\AppMcpWakeApp.exe（见 README）。
// 只写 HKCU\Software\Classes\appmcp-wintest，结束时删除。

import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import http from 'node:http'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const targetDir = process.env.CARGO_TARGET_DIR ?? path.join(repo, 'target', 'win')
const hostExe = process.env.APP_MCP_HOST_EXE ?? path.join(targetDir, 'debug', 'app-mcp-host.exe')
const appExe = process.env.WAKE_APP_EXE ?? path.join(targetDir, 'dotnet', 'bin', 'WakeApp', 'debug', 'AppMcpWakeApp.exe')
const work = path.join(targetDir, 'wintest')
const appLog = path.join(path.dirname(appExe), 'wakeapp.log')
const hostLog = path.join(work, 'host.log')
const WS_PORT = 7791
const CALC_AUMID = 'Microsoft.WindowsCalculator_8wekyb3d8bbwe!App'
const selected = new Set(process.argv.slice(2).length ? process.argv.slice(2) : ['a', 'b', 'c', 'd'])

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

const readAppLog = () => (fs.existsSync(appLog) ? fs.readFileSync(appLog, 'utf8') : '')
const readHostLog = () => (fs.existsSync(hostLog) ? fs.readFileSync(hostLog, 'utf8') : '')

function processes(image) {
  const r = spawnSync('tasklist', ['/FI', `IMAGENAME eq ${image}`, '/FO', 'CSV', '/NH'], { encoding: 'utf8' })
  return r.stdout
    .split(/\r?\n/)
    .map((l) => l.match(/^"([^"]+)","(\d+)"/))
    .filter(Boolean)
    .map((m) => Number(m[2]))
}

function regQuery(key) {
  const r = spawnSync('reg', ['query', key, '/s'], { encoding: 'utf8' })
  return r.status === 0 ? r.stdout : null
}

// ---------------------------------------------------------------- 最小 MCP 客户端（stdio）
class Mcp {
  constructor(args) {
    fs.mkdirSync(work, { recursive: true })
    this.logFd = fs.openSync(hostLog, 'w')
    this.child = spawn(hostExe, args, { stdio: ['pipe', 'pipe', this.logFd], windowsHide: true })
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
    const r = await this.request('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'wintest', version: '0.1.0' },
    })
    this.notify('notifications/initialized', {})
    return r
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

function writeManifest(file, m) {
  fs.writeFileSync(file, JSON.stringify({ manifestVersion: 1, version: '0.1.0', ...m }, null, 2))
  return file
}

const echoTool = {
  name: 'echo',
  description: '回显文本，并返回进程信息',
  inputSchema: { type: 'object', properties: { text: { type: 'string' } } },
  risk: 'read',
  activation: 'headless',
}

// ---------------------------------------------------------------- web-url 记录服务器
function startRecorder() {
  const hits = []
  const server = http.createServer((req, res) => {
    hits.push({ url: req.url, ua: req.headers['user-agent'] ?? '' })
    if (req.url.startsWith('/wake')) {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
      // 回报地址栏中的唤醒令牌，然后关闭本标签页（由外部打开、历史只有一项的标签页允许脚本关闭）。
      res.end(`<!doctype html><title>app-mcp wintest</title><script>
        addEventListener('pagehide', () => navigator.sendBeacon('/closed'))
        fetch('/report?hash=' + encodeURIComponent(location.hash)).finally(() => setTimeout(() => window.close(), 100))
      </script>`)
    } else {
      res.writeHead(204)
      res.end()
    }
  })
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve({ server, hits, port: server.address().port })))
}

// ---------------------------------------------------------------- 主流程
async function main() {
  if (!fs.existsSync(hostExe)) throw new Error(`找不到 ${hostExe}`)
  if (!fs.existsSync(appExe)) throw new Error(`找不到 ${appExe}`)
  fs.mkdirSync(work, { recursive: true })
  const emptyDir = path.join(work, 'no-manifests')
  fs.mkdirSync(emptyDir, { recursive: true })
  for (const pid of processes('AppMcpWakeApp.exe')) spawnSync('taskkill', ['/PID', String(pid), '/F'])
  if (fs.existsSync(appLog)) fs.rmSync(appLog)

  // 注册测试 scheme（真实 HKCU）
  spawnSync(appExe, ['--register'])
  const reg = regQuery('HKCU\\Software\\Classes\\appmcp-wintest')
  record('register', !!reg && reg.includes('URL Protocol') && reg.includes(appExe), (reg ?? '').split(/\r?\n/).filter((l) => l.includes('REG_SZ')).map((l) => l.trim()).join(' | '))

  const recorder = await startRecorder()
  const manifests = [
    writeManifest(path.join(work, 'wintest.json'), {
      appId: 'wintest',
      name: 'Windows 唤醒测试',
      tools: [echoTool],
      wake: { windows: [{ kind: 'uri', target: 'appmcp-wintest', background: true }] },
    }),
    writeManifest(path.join(work, 'webtest.json'), {
      appId: 'webtest',
      name: 'web-url 唤醒测试',
      tools: [echoTool],
      wake: { web: [{ kind: 'web-url', target: `http://127.0.0.1:${recorder.port}/wake` }] },
    }),
    writeManifest(path.join(work, 'calctest.json'), {
      appId: 'calctest',
      name: 'aumid 唤醒测试',
      tools: [echoTool],
      wake: { windows: [{ kind: 'aumid', target: CALC_AUMID }] },
    }),
  ]

  const mcp = new Mcp([
    '--ws-addr', `127.0.0.1:${WS_PORT}`,
    '--manifest-dir', emptyDir,
    ...manifests.flatMap((m) => ['--manifest', m]),
    '--lease-ms', '1000',
    '--wake-timeout-ms', '12000',
    '--log-level', 'debug',
  ])
  try {
    const init = await mcp.init()
    out(`host: ${init.serverInfo.name} ${init.serverInfo.version} protocol=${init.protocolVersion}`)
    const tools = (await mcp.request('tools/list', {})).tools.map((t) => t.name)
    record('tools/list', ['wintest.echo', 'webtest.echo', 'calctest.echo'].every((n) => tools.includes(n)), tools.join(', '))

    let firstPid
    if (selected.has('a') || selected.has('b')) {
      // a) 冷启动
      const { r, ms } = await mcp.call('wintest.echo', { text: 'cold' })
      const d = r.structuredContent ?? JSON.parse(r.content?.[0]?.text ?? '{}')
      firstPid = d.pid
      out(`a: ${ms}ms ${JSON.stringify(r.structuredContent ?? r.content)}`)
      const log = readAppLog()
      record('a 冷启动唤醒', !r.isError && d.text === 'cold' && d.launchedByWake === true && /HANDLE_WAKE cold=True/.test(log), `${ms}ms pid=${d.pid}`)
      await waitFor(() => /STATE Dormant/.test(readAppLog()), 15000, 'App 休眠')
      out('a: App 已休眠（STATE Dormant）')
    }

    if (selected.has('b')) {
      const before = readAppLog().length
      const { r, ms } = await mcp.call('wintest.echo', { text: 'warm' })
      const d = r.structuredContent ?? JSON.parse(r.content?.[0]?.text ?? '{}')
      out(`b: ${ms}ms ${JSON.stringify(r.structuredContent ?? r.content)}`)
      const tail = readAppLog().slice(before)
      const forwarded = /FORWARDED/.test(tail)
      const activated = /ACTIVATED args=\[appmcp-wintest:\/\/app-mcp\/wake\?token=[0-9a-f]+\] wakeHandled=True/.test(tail)
      record('b 休眠唤醒（同一进程）', !r.isError && d.text === 'warm' && d.pid === firstPid && forwarded && activated, `${ms}ms pid=${d.pid} forwarded=${forwarded} activated=${activated}`)
      await waitFor(() => /STATE Dormant/.test(readAppLog().slice(before)), 15000, 'App 再次休眠')
      out('b: App 再次休眠')
      const hl = readHostLog()
      const resumed = hl.split(/\r?\n/).filter((l) => /tools_current|toolsCurrent|快速恢复/i.test(l))
      out(`b: Host 日志中与快速恢复相关的行 ${resumed.length} 条`)
      for (const l of resumed.slice(-3)) out(`   ${l.replace(/\x1b\[[0-9;]*m/g, '').slice(0, 240)}`)
    }

    if (selected.has('c')) {
      const { r, ms } = await mcp.call('webtest.echo', { text: 'web' })
      out(`c: ${ms}ms isError=${r.isError} ${JSON.stringify(r.content).slice(0, 200)}`)
      await waitFor(() => recorder.hits.some((h) => h.url.startsWith('/report')), 10000, '页面回报').catch(() => {})
      const page = recorder.hits.find((h) => h.url.startsWith('/wake'))
      const report = recorder.hits.find((h) => h.url.startsWith('/report'))
      const hash = report ? decodeURIComponent(report.url.split('hash=')[1] ?? '') : ''
      out(`c: 收到请求 ${recorder.hits.map((h) => h.url).join(' , ')}`)
      const closed = await waitFor(() => recorder.hits.some((h) => h.url.startsWith('/closed')), 5000, '标签页关闭').catch(() => false)
      record('c web-url(rundll32)', !!page && /^#app-mcp-wake=[0-9a-f]{32}$/.test(hash), `hash=${hash} 标签页已关闭=${!!closed} ua=${(page?.ua ?? '').slice(0, 60)}`)
    }

    if (selected.has('d')) {
      const before = new Set(processes('CalculatorApp.exe'))
      const { r, ms } = await mcp.call('calctest.echo', { text: 'calc' })
      out(`d: ${ms}ms isError=${r.isError} ${JSON.stringify(r.content).slice(0, 200)}`)
      const started = await waitFor(() => {
        const now = processes('CalculatorApp.exe').filter((p) => !before.has(p))
        return now.length ? now : null
      }, 10000, '计算器进程').catch(() => [])
      const cmdLine = readHostLog().split(/\r?\n/).find((l) => l.includes('explorer.exe') && l.includes('shell:AppsFolder'))
      record('d aumid(explorer shell:AppsFolder)', started.length > 0 && !!cmdLine, `新进程 ${started.join(',') || '无'}`)
      for (const pid of started) spawnSync('taskkill', ['/PID', String(pid), '/F'])
    }
  } finally {
    // 结束测试 App、Host，删除注册表键
    spawnSync(appExe, ['--quit'])
    await sleep(1000)
    for (const pid of processes('AppMcpWakeApp.exe')) spawnSync('taskkill', ['/PID', String(pid), '/F'])
    mcp.close()
    recorder.server.closeAllConnections()
    recorder.server.close()
    spawnSync(appExe, ['--unregister'])
    const left = regQuery('HKCU\\Software\\Classes\\appmcp-wintest')
    record('cleanup', left === null && processes('AppMcpWakeApp.exe').length === 0, `注册表键已删除=${left === null}`)
  }

  out('\n---- Host 唤醒相关日志 ----')
  for (const l of readHostLog().split(/\r?\n/).filter((l) => /唤醒|wake|Waking|Dormant|休眠/i.test(l)).slice(0, 40)) {
    out(l.replace(/\x1b\[[0-9;]*m/g, '').slice(0, 260))
  }
  out('\n---- WakeApp 日志 ----')
  out(readAppLog())
  const failed = results.filter((r) => !r.ok)
  out(`\n结果：${results.length - failed.length}/${results.length} 通过`)
  process.exit(failed.length ? 1 : 0)
}

main().catch((e) => {
  out(`FAIL ${e.stack ?? e}`)
  spawnSync(appExe, ['--unregister'])
  process.exit(1)
})
