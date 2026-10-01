/**
 * 按需加载 WASM 核心。
 *
 * `./wasm/` 由 `pnpm --filter @app-mcp/web build:wasm` 生成（不入库），`tsup` 构建时原样复制到 `dist/wasm/`。
 *
 * 胶水 JS 与 .wasm 都用 `new URL('./wasm/...', import.meta.url)` 定位：
 * - Vite（dev 与 build）、webpack 5 等打包器能识别这种写法，把两个文件作为资源输出；
 * - 生成物不存在时不会在转换阶段报错（不像静态分析的 `import('./wasm/...')`），
 *   因此未构建 WASM 时依赖本包的测试照常运行，只有真正加载时才失败。
 * wasm-bindgen `--target web` 的胶水代码没有外部 import，可以直接作为 ES 模块加载。
 */

import type {
  CoreClient,
  CoreConfig,
  CoreEvent,
  CoreFactory,
  CoreOutcome,
  CoreResourceDef,
  CoreState,
  CoreToolDef,
  CoreToolUpdate,
  SleepReason,
  WakeReason,
} from './core'
import type { Visibility } from './types'

/**
 * WASM 导出的 `WasmClient`（bindings/wasm）。
 *
 * @invariant 与 `CoreClient` 方法一一对应；配置、定义、结果、状态与事件以 JSON 字符串交换，其余参数相同。
 * @why JSON 交换让 WASM 不必链接 serde-wasm-bindgen（体积）。
 */
export interface RawWasmClient {
  state(): string
  token(): string | undefined
  start(now: number): void
  stop(now: number): void
  createScope(name: string, parent?: number): number
  disposeScope(scope: number): void
  registerTool(defJson: string): number
  updateTool(tool: number, updateJson: string): void
  unregisterTool(tool: number): void
  registerResource(defJson: string): number
  notifyResourceChanged(resource: number, now: number): void
  unregisterResource(resource: number): void
  setVisibility(visibility: string, focused: boolean, now: number): void
  handleConnected(now: number): void
  handleDisconnected(now: number): void
  handleConnectFailed(code: string, message: string, now: number): void
  handleDisconnectedWith(code: string, message: string, now: number): void
  handleMessage(text: string, now: number): void
  handleTimeout(now: number): void
  completeCall(callId: string, outcomeJson: string, now: number): void
  completeRead(read: number, outcomeJson: string): void
  pollEvent(): string | undefined
  pollTimeout(): number | undefined
  handleWake(args: string, now: number): boolean
  wake(now: number): boolean
  wakeWithReason(reason: string, now: number): boolean
  connectNow(now: number): boolean
  sleep(now: number): boolean
  sleepWithReason(reason: string, now: number): boolean
  hold(now: number): number
  holdForCall(callId: string, now: number): number
  releaseHold(hold: number, now: number): boolean
  toolsHash(): string
  resumeToken(): string | undefined
  connectionId(): string | undefined
  reportIssue(code: string, message: string): void
  free(): void
}

/** wasm-bindgen 胶水模块（`./wasm/app_mcp_wasm.js`）的导出。 */
export interface WasmBindings {
  default: (init?: { module_or_path: string | URL | BufferSource }) => Promise<unknown>
  WasmClient: new (configJson: string) => RawWasmClient
  parseWakeToken?: (args: string) => string | undefined
}

/** 把 JSON 交换的 `RawWasmClient` 包装成驱动层使用的 `CoreClient`。 */
class WasmCore implements CoreClient {
  constructor(private readonly raw: RawWasmClient) {}

  state(): CoreState {
    return JSON.parse(this.raw.state()) as CoreState
  }
  token(): string | undefined {
    return this.raw.token() ?? undefined
  }
  start(now: number): void {
    this.raw.start(now)
  }
  stop(now: number): void {
    this.raw.stop(now)
  }
  createScope(name: string, parent?: number): number {
    return this.raw.createScope(name, parent)
  }
  disposeScope(scope: number): void {
    this.raw.disposeScope(scope)
  }
  registerTool(def: CoreToolDef): number {
    return this.raw.registerTool(JSON.stringify(def))
  }
  updateTool(tool: number, update: CoreToolUpdate): void {
    this.raw.updateTool(tool, JSON.stringify(update))
  }
  unregisterTool(tool: number): void {
    this.raw.unregisterTool(tool)
  }
  registerResource(def: CoreResourceDef): number {
    return this.raw.registerResource(JSON.stringify(def))
  }
  notifyResourceChanged(resource: number, now: number): void {
    this.raw.notifyResourceChanged(resource, now)
  }
  unregisterResource(resource: number): void {
    this.raw.unregisterResource(resource)
  }
  setVisibility(visibility: Visibility, focused: boolean, now: number): void {
    this.raw.setVisibility(visibility, focused, now)
  }
  handleConnected(now: number): void {
    this.raw.handleConnected(now)
  }
  handleDisconnected(now: number): void {
    this.raw.handleDisconnected(now)
  }
  handleConnectFailed(code: string, message: string, now: number): void {
    this.raw.handleConnectFailed(code, message, now)
  }
  handleDisconnectedWith(code: string, message: string, now: number): void {
    this.raw.handleDisconnectedWith(code, message, now)
  }
  handleMessage(text: string, now: number): void {
    this.raw.handleMessage(text, now)
  }
  handleTimeout(now: number): void {
    this.raw.handleTimeout(now)
  }
  completeCall(callId: string, outcome: CoreOutcome, now: number): void {
    this.raw.completeCall(callId, JSON.stringify(outcome), now)
  }
  completeRead(read: number, outcome: CoreOutcome): void {
    this.raw.completeRead(read, JSON.stringify(outcome))
  }
  pollEvent(): CoreEvent | undefined {
    const json = this.raw.pollEvent()
    return json === undefined ? undefined : (JSON.parse(json) as CoreEvent)
  }
  pollTimeout(): number | undefined {
    return this.raw.pollTimeout() ?? undefined
  }
  handleWake(args: string, now: number): boolean {
    return this.raw.handleWake(args, now)
  }
  wake(now: number): boolean {
    return this.raw.wake(now)
  }
  wakeWithReason(reason: WakeReason, now: number): boolean {
    return this.raw.wakeWithReason(reason, now)
  }
  connectNow(now: number): boolean {
    return this.raw.connectNow(now)
  }
  sleep(now: number): boolean {
    return this.raw.sleep(now)
  }
  sleepWithReason(reason: SleepReason, now: number): boolean {
    return this.raw.sleepWithReason(reason, now)
  }
  hold(now: number): number {
    return this.raw.hold(now)
  }
  holdForCall(callId: string, now: number): number {
    return this.raw.holdForCall(callId, now)
  }
  releaseHold(hold: number, now: number): boolean {
    return this.raw.releaseHold(hold, now)
  }
  toolsHash(): string {
    return this.raw.toolsHash()
  }
  resumeToken(): string | undefined {
    return this.raw.resumeToken() ?? undefined
  }
  connectionId(): string | undefined {
    return this.raw.connectionId() ?? undefined
  }
  reportIssue(code: string, message: string): void {
    this.raw.reportIssue(code, message)
  }
  free(): void {
    this.raw.free()
  }
}

/** 由已初始化的胶水模块生成核心构造函数（测试直接加载胶水模块时也用它）。 */
export function wasmCoreFactory(bindings: Pick<WasmBindings, 'WasmClient' | 'parseWakeToken'>): CoreFactory {
  const factory: CoreFactory = (config: CoreConfig) => new WasmCore(new bindings.WasmClient(JSON.stringify(config)))
  const parse = bindings.parseWakeToken
  if (typeof parse === 'function') factory.parseWakeToken = (args) => parse(args) ?? undefined
  return factory
}

export async function loadWasmCore(wasmUrl?: string | URL): Promise<CoreFactory> {
  const glue = new URL('./wasm/app_mcp_wasm.js', import.meta.url)
  const wasm = wasmUrl ?? new URL('./wasm/app_mcp_wasm_bg.wasm', import.meta.url)
  const mod = (await import(/* @vite-ignore */ glue.href)) as WasmBindings
  await mod.default({ module_or_path: wasm })
  return wasmCoreFactory(mod)
}
