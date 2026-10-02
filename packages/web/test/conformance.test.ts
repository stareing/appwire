// @vitest-environment node
/**
 * 一致性用例 runner（@app-mcp/web）：真实 WASM 核心 + JS 驱动层，经 Node 22 全局 WebSocket 连接 fake_host
 * （`--case` 模式，核对在 fake_host 内完成）。格式与约定见 conformance/README.md；公共部分（含与 @app-mcp/node 共用的注册映射）在
 * conformance/runner/support.mjs。
 *
 * 前置条件：`pnpm --filter @app-mcp/web build:wasm`；能构建 / 找到 fake_host（或 APP_MCP_FAKE_HOST）。缺少时跳过。
 * 只跑部分用例：`APP_MCP_CONFORMANCE_CASES=handshake,errors pnpm --filter @app-mcp/web test conformance`。
 *
 * @why 页面环境：用 node 环境（全局 WebSocket 为真实实现），以最小的假 window 提供地址与 `hashchange`。
 *   网页 SDK 没有 `handleWake(arg)`：唤醒令牌经地址片段 `#app-mcp-wake=<token>` 到达（页面已打开时 Host 聚焦同一地址，
 *   触发 `hashchange`），runner 把 fake_host 的 `app-mcp-wake:<token>` 按此方式交给 SDK。不使用 SharedWorker 共享连接。
 */
import { existsSync, readFileSync } from 'node:fs'
import { basename, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { beforeAll, describe, expect, it } from 'vitest'
import {
  appConfig,
  caseFiles,
  describeFailure,
  findFakeHost,
  registerJsApp,
  runCase,
  verdictOk,
  type ConformanceCase,
  type Session,
} from '../../../conformance/runner/support.mjs'
import type { CoreFactory } from '../src/core'
import { createDriver } from '../src/driver'
import { ToolCallError } from '../src/types'
import { wasmCoreFactory, type WasmBindings } from '../src/wasm-loader'

const SDK = 'web'
/** 本 runner 支持的用例能力（conformance/README.md 第 4 节）。 */
const FEATURES = [
  'toolOptions', 'mutate', 'lifecycle', 'wake', 'richResult', 'userAction', 'progress', 'resourceOptions', 'readFailure',
  'surface', 'navigation',
]
const PAGE_URL = 'http://localhost:5173/conformance'
const WAKE_PREFIX = 'app-mcp-wake:'

// 测试从包目录运行（pnpm --filter / vitest 默认 root），与 wasm-smoke.test.ts 相同
const glue = resolve(process.cwd(), 'src/wasm/app_mcp_wasm.js')
const wasm = resolve(process.cwd(), 'src/wasm/app_mcp_wasm_bg.wasm')
const available = existsSync(glue) && existsSync(wasm) && typeof globalThis.WebSocket === 'function'

const silentLogger = { debug: () => {}, warn: () => {}, error: () => {} }

/** 最小页面：地址、`history.replaceState`（SDK 从地址栏移除唤醒片段）与 `hashchange` 事件。 */
class FakePage extends EventTarget {
  readonly location = { href: PAGE_URL }
  readonly history = {
    state: null,
    replaceState: (_state: unknown, _title: string, url: string): void => {
      this.location.href = new URL(url, this.location.href).href
    },
  }

  /** 唤醒：Host 聚焦带 `#app-mcp-wake=<token>` 的地址。 */
  navigateToWake(arg: string): void {
    const token = arg.startsWith(WAKE_PREFIX) ? arg.slice(WAKE_PREFIX.length) : arg
    this.location.href = `${PAGE_URL}#app-mcp-wake=${token}`
    this.dispatchEvent(new Event('hashchange'))
  }
}

function startApp(loadCore: () => Promise<CoreFactory>, testCase: ConformanceCase, url: string): Session {
  const page = new FakePage()
  const app = createDriver(
    { appId: 'conf', appName: 'Conformance', hostUrl: url, sharedConnection: false, logger: silentLogger, ...appConfig(testCase) },
    { loadCore, window: page as unknown as Window },
  )
  const { navigate } = registerJsApp(app, testCase, ToolCallError)
  // 导航回调在 start 之前设置（核心加载时打开导航能力，首次握手即声明）
  if (navigate) app.setNavigationHandler?.(({ page, params }) => navigate(page, params))
  return { handleWake: (arg) => page.navigateToWake(arg), stop: () => app.dispose() }
}

let loadCore: (() => Promise<CoreFactory>) | undefined
let fakeHost: string | undefined

beforeAll(async () => {
  if (!available) return
  const mod = (await import(/* @vite-ignore */ pathToFileURL(glue).href)) as WasmBindings
  await mod.default({ module_or_path: readFileSync(wasm) })
  const factory = wasmCoreFactory(mod)
  loadCore = () => Promise.resolve(factory)
  fakeHost = findFakeHost()
}, 600_000)

describe.skipIf(!available)('一致性用例（web）', () => {
  const cases = caseFiles().map((casePath) => ({ id: basename(casePath, '.json'), casePath }))
  it.for(cases)('$id', { timeout: 60_000 }, async ({ casePath }, ctx) => {
    if (!fakeHost || !loadCore) ctx.skip()
    const outcome = await runCase({
      bin: fakeHost!,
      casePath,
      sdk: SDK,
      features: FEATURES,
      start: (testCase, url) => startApp(loadCore!, testCase, url),
    })
    expect(verdictOk(outcome), describeFailure(outcome)).toBe(true)
  })
})
