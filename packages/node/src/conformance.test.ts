/**
 * 一致性用例 runner（@app-mcp/node）：按 `conformance/cases/*.json` 注册工具与资源，连接 fake_host（`--case` 模式，
 * 核对在 fake_host 内完成）。格式与约定见 conformance/README.md；公共部分（含与 @app-mcp/web 共用的注册映射）在
 * conformance/runner/support.mjs。
 *
 * 前置条件同 integration.test.ts：`pnpm --filter @app-mcp/node build:native`，能构建 / 找到 fake_host（或 APP_MCP_FAKE_HOST）。
 * 只跑部分用例：`APP_MCP_CONFORMANCE_CASES=handshake,errors pnpm --filter @app-mcp/node test conformance`。
 */
import { existsSync } from 'node:fs'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { beforeAll, describe, expect, it } from 'vitest'
import {
  appConfig,
  appVisibility,
  caseFiles,
  describeFailure,
  findFakeHost,
  registerJsApp,
  runCase,
  verdictOk,
  type ConformanceCase,
  type Session,
} from '../../../conformance/runner/support.mjs'
import { createAppMcp, ToolCallError } from './index.js'
import { loadNativeBinding, nativeFileName, type NativeBinding } from './native.js'

const SDK = 'node'
/** 本 runner 支持的用例能力（conformance/README.md 第 4 节）。 */
const FEATURES = [
  'toolOptions', 'mutate', 'lifecycle', 'wake', 'richResult', 'userAction', 'progress', 'resourceOptions', 'readFailure',
  'surface', 'navigation', 'backgroundTool', 'backgroundNavigation', 'idempotencyKey', 'callScheduling', 'busy', 'events', 'implements',
]

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const nativePath = process.env.APP_MCP_NODE_NATIVE ?? join(pkgDir, 'native', nativeFileName())

const silentLogger = { debug: () => {}, warn: () => {}, error: () => {} }

function startApp(binding: NativeBinding, testCase: ConformanceCase, url: string): Session {
  const app = createAppMcp({
    appId: 'conf',
    appName: 'Conformance',
    binding,
    hostUrl: url,
    autoStart: false,
    keepAlive: false,
    logger: silentLogger,
    ...appConfig(testCase),
  })
  const { navigate } = registerJsApp(app, testCase, ToolCallError)
  if (navigate) app.setNavigationHandler(({ page, params }) => navigate(page, params))
  const visibility = appVisibility(testCase)
  if (visibility) app.setVisibility(visibility, false)
  app.start()
  return { handleWake: (arg) => void app.handleWake(arg), stop: () => app.dispose() }
}

let binding: NativeBinding | undefined
let fakeHost: string | undefined

beforeAll(() => {
  if (!existsSync(nativePath)) return
  binding = loadNativeBinding()
  fakeHost = findFakeHost()
}, 600_000)

describe.skipIf(!existsSync(nativePath))('一致性用例（node）', () => {
  const cases = caseFiles().map((casePath) => ({ id: basename(casePath, '.json'), casePath }))
  it.for(cases)('$id', { timeout: 60_000 }, async ({ casePath }, ctx) => {
    if (!fakeHost) ctx.skip()
    const outcome = await runCase({
      bin: fakeHost!,
      casePath,
      sdk: SDK,
      features: FEATURES,
      start: (testCase, url) => startApp(binding!, testCase, url),
    })
    expect(verdictOk(outcome), describeFailure(outcome)).toBe(true)
  })
})
