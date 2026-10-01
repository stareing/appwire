// support.mjs 的类型（TS runner 使用）。用例格式见 conformance/README.md 第 2 节。

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

export interface ProgressStep {
  progress: number
  total?: number
  message?: string
}

export interface UserActionSpec {
  message: string
  reason?: string
  uri?: string
}

export interface ResultSpec {
  data?: Json
  status?: string
  stateResource?: string
  summary?: string
  stateHints?: string[]
  annotations?: { [key: string]: Json }
}

export interface HandlerSpec {
  progress?: ProgressStep[]
  delayMs?: number
  mutate?: MutationOp[]
  counter?: boolean
  throw?: string
  userAction?: UserActionSpec
  result?: ResultSpec
  return?: Json
  echo?: boolean
  returnNothing?: boolean
}

export interface ToolDecl {
  name: string
  description: string
  title?: string
  inputSchema?: { [key: string]: Json }
  outputSchema?: { [key: string]: Json }
  risk?: string
  activation?: string
  annotations?: { [key: string]: Json }
  enabled?: boolean
  handler?: HandlerSpec
}

export interface ReadSpec {
  return?: Json
  fail?: { message: string; kind?: string; details?: { [key: string]: Json } }
  userAction?: UserActionSpec
  throw?: string
}

export interface ResourceDecl {
  name: string
  description: string
  mimeType?: string
  realtime?: boolean
  annotations?: { [key: string]: Json }
  read: ReadSpec
}

export type MutationOp =
  | { op: 'register'; tool: ToolDecl }
  | { op: 'update'; name: string; set: { [key: string]: Json } }
  | { op: 'remove' | 'enable' | 'disable'; name: string }

export interface ConformanceCase {
  id: string
  title: string
  requires?: string[]
  app: {
    config?: {
      lifecycle?: { mode?: 'persistent' | 'idle' | 'on-demand'; idleTimeoutMs?: number; graceMs?: number; mergeWindowMs?: number }
      callDedup?: { ttlMs?: number; maxEntries?: number }
      maxConcurrentCalls?: number
    }
    tools?: ToolDecl[]
    resources?: ResourceDecl[]
  }
  host: { [key: string]: Json }
}

export interface Session {
  handleWake(arg: string): void
  stop(): void | Promise<void>
}

export interface Verdict {
  type: 'verdict'
  status: 'pass' | 'fail' | 'xfail' | 'xpass' | 'skip'
  failures?: Json
  [key: string]: Json | undefined
}

export interface CaseOutcome {
  id: string
  verdict: Verdict | undefined
  code: number | null
  stderr: string
  error: unknown
}

export type HandlerOutcome =
  | { kind: 'throw'; message: string }
  | ({ kind: 'userAction' } & UserActionSpec)
  | { kind: 'result'; result: ResultSpec }
  | { kind: 'value'; value: unknown }
  | { kind: 'nothing' }

export interface HandlerEnv {
  count: number
  args: unknown
  progress(progress: number, total?: number, message?: string): void
  isCancelled(): boolean
  mutate(op: MutationOp): void
}

export interface RegistryOps<H> {
  register(decl: ToolDecl): H
  update(handle: H, next: ToolDecl, set: { [key: string]: Json }): void
  remove(handle: H): void
  setEnabled(handle: H, enabled: boolean): void
}

export interface Registry {
  register(decl: ToolDecl): void
  mutate(op: MutationOp): void
}

export declare const repoRoot: string
export declare const casesDir: string
export declare const reportDir: string
export declare function caseFiles(): string[]
export declare function loadCase(path: string): ConformanceCase
export declare function missingFeatures(testCase: ConformanceCase, features: readonly string[]): string[]
export declare function findFakeHost(): string | undefined
export declare function hostUrl(addr: string): string
export declare function runCase(options: {
  bin: string
  casePath: string
  sdk: string
  features: readonly string[]
  start(testCase: ConformanceCase, url: string): Session | Promise<Session>
}): Promise<CaseOutcome>
export declare function verdictOk(outcome: CaseOutcome): boolean
export declare function describeFailure(outcome: CaseOutcome): string
export declare function execHandler(spec: HandlerSpec, env: HandlerEnv): Promise<HandlerOutcome>
export declare function createRegistry<H>(ops: RegistryOps<H>): Registry
export declare function appConfig(testCase: ConformanceCase): {
  lifecycle?: NonNullable<ConformanceCase['app']['config']>['lifecycle']
  callDedup?: { ttlMs?: number; maxEntries?: number }
  maxConcurrentCalls?: number
}
export declare function defined<T extends object>(obj: T): Partial<T>

/** `@app-mcp/node` / `@app-mcp/web` 实例的注册部分（两包同形，按结构取用）。 */
export interface JsRegistrar {
  tool(name: string, definition: any): { update(changes: any): void; dispose(): void }
  resource(name: string, definition: any): unknown
}

/** `ToolCallError`（两包各自导出，结构相同）。 */
export interface JsToolCallErrorClass {
  new (kind: any, message: string, details?: Record<string, unknown>): Error
  userActionRequired(message: string, options?: { reason?: string; uri?: string }): Error
}

export declare function registerJsApp(app: JsRegistrar, testCase: ConformanceCase, ToolCallError: JsToolCallErrorClass): void
