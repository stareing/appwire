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
  returnIdempotencyKey?: boolean
  returnNothing?: boolean
  /** 能力 events：依次发出事件，结果 `{emitted: [true | false | "error"]}`（无其他结果时）。 */
  emit?: EmitSpec[]
}

/** 事件声明（能力 events；`app.events`、变更 `declareEvent`）。 */
export interface EventDecl {
  name: string
  description: string
  payloadSchema?: { [key: string]: Json }
}

/** handler `emit` 的一项；`payload` 原样交给 SDK（可能故意不是对象）。 */
export interface EmitSpec {
  name: string
  payload?: Json
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
  surface?: 'app' | 'view'
  page?: string
  backgroundTool?: string
  handler?: HandlerSpec
}

/** 导航行为（conformance/README.md 2.4）。 */
export interface NavigationSpec {
  mutate?: MutationOp[]
  deny?: string
  fail?: string
  userAction?: UserActionSpec
  throw?: string
  failParams?: boolean
}

export type NavigationOutcome =
  | { kind: 'ok' }
  | { kind: 'deny' | 'fail' | 'throw'; message: string }
  | ({ kind: 'userAction' } & UserActionSpec)

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
  | { op: 'busy'; value: boolean }
  | { op: 'declareEvent'; event: EventDecl }
  | { op: 'removeEvent'; name: string }

export interface ConformanceCase {
  id: string
  title: string
  requires?: string[]
  app: {
    config?: {
      lifecycle?: { mode?: 'persistent' | 'idle' | 'on-demand'; idleTimeoutMs?: number; graceMs?: number; mergeWindowMs?: number }
      callDedup?: { ttlMs?: number; maxEntries?: number }
      maxConcurrentCalls?: number
      maxQueuedCalls?: number
      navigateInBackground?: boolean
      busyPolicy?: 'reject' | 'queue'
    }
    visibility?: 'visible' | 'hidden' | 'frozen'
    /** 能力 busy：启动前 `setBusy(true)`。 */
    busy?: boolean
    /** 能力 events：启动前声明的事件。 */
    events?: EventDecl[]
    tools?: ToolDecl[]
    resources?: ResourceDecl[]
    navigation?: { [page: string]: NavigationSpec }
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
  /** handler 上下文中的幂等键（spec/protocol.md 3.3）；没有时 undefined / null。 */
  idempotencyKey?: string | null
  progress(progress: number, total?: number, message?: string): void
  isCancelled(): boolean
  mutate(op: MutationOp): void
  /** SDK 发事件 API（能力 events）：返回是否发送，本地错误抛出。 */
  emit?(name: string, payload: Json | undefined): boolean
}

export interface RegistryOps<H> {
  register(decl: ToolDecl): H
  update(handle: H, next: ToolDecl, set: { [key: string]: Json }): void
  remove(handle: H): void
  setEnabled(handle: H, enabled: boolean): void
  /** 变更 `{op: "busy", value}`（能力 busy）；未提供时该变更抛错。 */
  setBusy?(busy: boolean): void
  /** 变更 `{op: "declareEvent", event}` / `{op: "removeEvent", name}`（能力 events）；未提供时该变更抛错。 */
  declareEvent?(event: EventDecl): void
  removeEvent?(name: string): void
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
export declare function execNavigation(
  pages: { [page: string]: NavigationSpec },
  page: string,
  params: unknown,
  env: { mutate(op: MutationOp): void },
): NavigationOutcome
export declare function createRegistry<H>(ops: RegistryOps<H>): Registry
export declare function appConfig(testCase: ConformanceCase): {
  lifecycle?: NonNullable<ConformanceCase['app']['config']>['lifecycle']
  callDedup?: { ttlMs?: number; maxEntries?: number }
  maxConcurrentCalls?: number
  maxQueuedCalls?: number
  navigateInBackground?: boolean
  busyPolicy?: 'reject' | 'queue'
}
/** 用例 `app.events`（能力 events）；未给出时为空数组。 */
export declare function appEvents(testCase: ConformanceCase): EventDecl[]
/** 用例 `app.busy`（能力 busy）：为 true 时 runner 在启动前调用 `setBusy(true)`。 */
export declare function appBusy(testCase: ConformanceCase): boolean
/** 用例 `app.visibility`；未给出时为 undefined。 */
export declare function appVisibility(testCase: ConformanceCase): 'visible' | 'hidden' | 'frozen' | undefined
export declare function defined<T extends object>(obj: T): Partial<T>

/** `@app-mcp/node` / `@app-mcp/web` 实例的注册部分（两包同形，按结构取用）。 */
export interface JsRegistrar {
  tool(name: string, definition: any): { update(changes: any): void; dispose(): void }
  resource(name: string, definition: any): unknown
  /** 用户正在操作（能力 busy）。 */
  setBusy(busy: boolean): void
  /** 事件（能力 events）。 */
  declareEvent(event: EventDecl): void
  removeEvent(name: string): boolean
  emitEvent(name: string, payload?: any): boolean
}

/** `ToolCallError`（两包各自导出，结构相同）。 */
export interface JsToolCallErrorClass {
  new (kind: any, message: string, details?: Record<string, unknown>): Error
  userActionRequired(message: string, options?: { reason?: string; uri?: string }): Error
}

/** `navigate`：用例有 `app.navigation` 时的 JS 导航回调（拒绝 / 失败以 ToolCallError 抛出），否则 undefined。 */
export declare function registerJsApp(
  app: JsRegistrar,
  testCase: ConformanceCase,
  ToolCallError: JsToolCallErrorClass,
): { navigate: ((page: string, params: unknown) => Promise<void>) | undefined }
