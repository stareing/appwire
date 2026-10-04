// 一致性 runner 的 JS 公共部分（Node / Web / 鸿蒙 runner 共用）：列出用例、定位 fake_host、逐个用例驱动 fake_host 进程，
// 以及与 SDK 无关的 handler 描述解释（conformance/README.md 2.1–2.3）。各 SDK 的 runner 只负责把这些映射到自己的 API。
// 类型见同目录 support.d.mts。参考实现：crates/native/tests/it/conformance.rs（Rust）。
//
// @invariant 不做任何期望核对：核对只在 fake_host 内（crates/native/examples/support/conformance.rs）。
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
export const casesDir = join(repoRoot, 'conformance', 'cases');
export const reportDir = join(repoRoot, 'target', 'conformance');

/** 用例文件路径（按文件名排序）；环境变量 `APP_MCP_CONFORMANCE_CASES=id1,id2` 过滤。 */
export function caseFiles() {
  const only = process.env.APP_MCP_CONFORMANCE_CASES?.split(',').map((s) => s.trim()).filter(Boolean);
  return readdirSync(casesDir)
    .filter((f) => f.endsWith('.json'))
    .filter((f) => !only || only.includes(f.slice(0, -5)))
    .sort()
    .map((f) => join(casesDir, f));
}

export function loadCase(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

/** 用例 `requires` 中本 runner 不支持的能力。 */
export function missingFeatures(testCase, features) {
  return (testCase.requires ?? []).filter((f) => !features.includes(f));
}

/**
 * fake_host 可执行文件：环境变量 `APP_MCP_FAKE_HOST`，否则用 cargo 构建（产物在仓库 target/，已是最新时只做检查）。
 * @error 构建失败或找不到产物时返回 undefined（调用方跳过）。
 */
export function findFakeHost() {
  if (process.env.APP_MCP_FAKE_HOST) return process.env.APP_MCP_FAKE_HOST;
  const r = spawnSync('cargo', ['build', '-q', '-p', 'app-mcp-native', '--example', 'fake_host'], {
    cwd: repoRoot,
    stdio: 'inherit',
  });
  const targetDir = process.env.CARGO_TARGET_DIR ? resolve(repoRoot, process.env.CARGO_TARGET_DIR) : join(repoRoot, 'target');
  const path = join(targetDir, 'debug', 'examples', process.platform === 'win32' ? 'fake_host.exe' : 'fake_host');
  return r.status === 0 && existsSync(path) ? path : undefined;
}

/** `LISTENING` 行的地址 → SDK 的 Host 地址（TCP 为 `ws://<addr>/app`，IPC 端点原样）。 */
export function hostUrl(addr) {
  return addr.startsWith('unix:') || addr.startsWith('pipe:') ? addr : `ws://${addr}/app`;
}

/**
 * 跑一个用例：启动 fake_host，`LISTENING` 后调 `start` 创建并启动 App，转发 `wake` 行，等 fake_host 退出后停止 App。
 * @output fake_host 的结论行（`{"type":"verdict",status,failures?,…}`）与退出码；没有结论行时 verdict 为 undefined。
 * @side-effect fake_host 写 `target/conformance/<sdk>/<case>.json`。
 */
export async function runCase({ bin, casePath, sdk, features, start }) {
  const testCase = loadCase(casePath);
  const missing = missingFeatures(testCase, features);
  const args = ['--case', casePath, '--sdk', sdk, '--report-dir', reportDir];
  if (missing.length > 0) args.push('--skip', `runner 不支持：${missing.join(', ')}`);
  const child = spawn(bin, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  const stderr = [];
  child.stderr.on('data', (d) => stderr.push(d.toString()));
  const exited = new Promise((r) => child.on('close', (code) => r(code)));
  let session;
  let verdict;
  let error;
  // @invariant 行按到达顺序串行处理（wake 必须在 start 完成之后）。
  let queue = Promise.resolve();
  const rl = createInterface({ input: child.stdout });
  rl.on('line', (line) => {
    queue = queue.then(async () => {
      if (line.startsWith('LISTENING ')) {
        session = await start(testCase, hostUrl(line.slice('LISTENING '.length).trim()));
        return;
      }
      let v;
      try {
        v = JSON.parse(line);
      } catch {
        return;
      }
      if (v.type === 'wake') session?.handleWake(v.arg);
      else if (v.type === 'verdict') verdict = v;
    }).catch((e) => {
      error ??= e;
    });
  });
  const code = await exited;
  await queue;
  try {
    await session?.stop();
  } catch (e) {
    error ??= e;
  }
  return { id: testCase.id, verdict, code, stderr: stderr.join(''), error };
}

/** 结论是否可接受（pass / xfail / xpass / skip，且退出码为 0）。 */
export function verdictOk(outcome) {
  const status = outcome.verdict?.status;
  return outcome.code === 0 && outcome.error === undefined && ['pass', 'xfail', 'xpass', 'skip'].includes(status);
}

/** 失败说明（用于断言消息）。 */
export function describeFailure(outcome) {
  return [
    `用例 ${outcome.id}：结论 ${outcome.verdict?.status ?? '（无）'}，退出码 ${outcome.code}`,
    outcome.verdict?.failures ? `failures: ${JSON.stringify(outcome.verdict.failures, null, 2)}` : '',
    outcome.error ? `runner 错误：${outcome.error?.stack ?? outcome.error}` : '',
    outcome.stderr ? `fake_host stderr:\n${outcome.stderr}` : '',
  ].filter(Boolean).join('\n');
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * 按 handler 描述执行（顺序：progress → delayMs → mutate → emit → counter 计数 → 结果，conformance/README.md 2.1）。
 * @input spec handler 描述；env.count 本次执行序号（counter 用，从 1 开始，由调用方按工具计数）；
 *   env.args 调用参数；env.idempotencyKey handler 上下文中的幂等键（没有时 undefined / null）；
 *   env.progress / env.isCancelled / env.mutate 由 SDK 映射；env.emit(name, payload) 为 SDK 发事件 API（能力 events，
 *   返回是否发送，本地错误抛出；`payload` 未给出时为 undefined）。
 * @output 与 SDK 无关的结果描述（`{kind:'throw'|'userAction'|'result'|'value'|'nothing', …}`），由 runner 映射为该语言的写法。
 */
export async function execHandler(spec, env) {
  for (const p of spec.progress ?? []) env.progress(p.progress, p.total, p.message);
  if (typeof spec.delayMs === 'number') {
    const until = Date.now() + spec.delayMs;
    while (Date.now() < until && !env.isCancelled()) await sleep(10);
  }
  for (const op of spec.mutate ?? []) env.mutate(op);
  const emitted = Array.isArray(spec.emit) ? spec.emit.map((e) => emitOne(env, e)) : undefined;
  if (typeof spec.throw === 'string') return { kind: 'throw', message: spec.throw };
  if (spec.userAction) return { kind: 'userAction', ...spec.userAction };
  if (spec.result && typeof spec.result === 'object') return { kind: 'result', result: spec.result };
  if ('return' in spec) return { kind: 'value', value: spec.return };
  if (spec.echo === true) return { kind: 'value', value: env.args };
  if (spec.returnIdempotencyKey === true) return { kind: 'value', value: { idempotencyKey: env.idempotencyKey ?? null } };
  if (spec.counter === true) return { kind: 'value', value: { count: env.count } };
  if (emitted) return { kind: 'value', value: { emitted } };
  return { kind: 'nothing' };
}

/** handler `emit` 的一项：SDK 的结果 `true` / `false`，本地错误为 `"error"`。 */
function emitOne(env, e) {
  if (typeof env.emit !== 'function') throw new Error('runner 未提供 emit（能力 events）');
  try {
    return env.emit(e.name, e.payload) === true;
  } catch {
    return 'error';
  }
}

/**
 * 按导航行为表执行（conformance/README.md 2.4）：先 `mutate`，再给出结果。未列出的页面以失败完成。
 * @input pages 用例 `app.navigation`；env.mutate 由 SDK 映射。
 * @output `{kind:'ok'}` / `{kind:'deny'|'fail'|'throw', message}` / `{kind:'userAction', message, reason?, uri?}`，
 *   由 runner 映射为该语言的写法。
 */
export function execNavigation(pages, page, params, env) {
  const spec = Object.prototype.hasOwnProperty.call(pages, page) ? pages[page] : undefined;
  if (!spec) return { kind: 'fail', message: `未知页面：${page}` };
  if (typeof spec.throw === 'string') return { kind: 'throw', message: spec.throw };
  for (const op of spec.mutate ?? []) env.mutate(op);
  if (typeof spec.deny === 'string') return { kind: 'deny', message: spec.deny };
  if (typeof spec.fail === 'string') return { kind: 'fail', message: spec.fail };
  if (spec.userAction) return { kind: 'userAction', ...spec.userAction };
  if (spec.failParams === true) return { kind: 'fail', message: params === undefined ? '' : JSON.stringify(params) };
  return { kind: 'ok' };
}

/**
 * 注册表（handler 的 `mutate`，conformance/README.md 2.3）：记录每个工具的句柄与当前声明。
 * @input ops.register(decl) → 句柄；ops.update(handle, nextDecl, set)；ops.remove(handle)；ops.setEnabled(handle, on)；
 *   ops.setBusy(busy)（可选，能力 busy：变更 `{op: "busy", value}`）；ops.declareEvent(event) / ops.removeEvent(name)
 *   （可选，能力 events：变更 `{op: "declareEvent", event}` / `{op: "removeEvent", name}`）。
 */
export function createRegistry(ops) {
  const tools = new Map();
  const register = (decl) => {
    tools.set(decl.name, { handle: ops.register(decl), decl });
  };
  const mutate = (op) => {
    const entry = tools.get(op.name);
    switch (op.op) {
      case 'register':
        return register(op.tool);
      case 'update': {
        if (!entry) throw new Error(`update 未知工具 ${op.name}`);
        const next = { ...entry.decl };
        for (const [k, v] of Object.entries(op.set ?? {})) {
          if (v === null) delete next[k];
          else next[k] = v;
        }
        entry.decl = next;
        return ops.update(entry.handle, next, op.set ?? {});
      }
      case 'remove':
        if (entry) {
          tools.delete(op.name);
          ops.remove(entry.handle);
        }
        return undefined;
      case 'enable':
      case 'disable':
        if (!entry) throw new Error(`${op.op} 未知工具 ${op.name}`);
        return ops.setEnabled(entry.handle, op.op === 'enable');
      case 'busy':
        if (typeof ops.setBusy !== 'function') throw new Error('runner 未提供 setBusy（能力 busy）');
        return ops.setBusy(op.value === true);
      case 'declareEvent':
        if (typeof ops.declareEvent !== 'function') throw new Error('runner 未提供 declareEvent（能力 events）');
        return ops.declareEvent(op.event);
      case 'removeEvent':
        if (typeof ops.removeEvent !== 'function') throw new Error('runner 未提供 removeEvent（能力 events）');
        ops.removeEvent(op.name);
        return undefined;
      default:
        throw new Error(`未知的 mutate 操作 ${op.op}`);
    }
  };
  return { register, mutate };
}

/** 用例 `app.config` 中 SDK 共同的选项（各 SDK 的字段名与此相同）。 */
export function appConfig(testCase) {
  const c = testCase.app?.config ?? {};
  const out = {};
  if (c.lifecycle) out.lifecycle = { ...c.lifecycle };
  if (c.callDedup) out.callDedup = { ...c.callDedup };
  if (typeof c.maxConcurrentCalls === 'number') out.maxConcurrentCalls = c.maxConcurrentCalls;
  if (typeof c.maxQueuedCalls === 'number') out.maxQueuedCalls = c.maxQueuedCalls;
  if (typeof c.navigateInBackground === 'boolean') out.navigateInBackground = c.navigateInBackground;
  if (c.busyPolicy === 'reject' || c.busyPolicy === 'queue') out.busyPolicy = c.busyPolicy;
  return out;
}

/** 用例 `app.events`（能力 events）：启动前声明的事件，原样交给 SDK 的声明事件 API；未给出时为空数组。 */
export function appEvents(testCase) {
  return Array.isArray(testCase.app?.events) ? testCase.app.events : [];
}

/** 用例 `app.busy`（能力 busy）：为 true 时 runner 在启动前调用 SDK 的 `setBusy(true)`。 */
export function appBusy(testCase) {
  return testCase.app?.busy === true;
}

/** 用例 `app.visibility`（启动前设置的实例可见性）；未给出或取值不认识时为 undefined。 */
export function appVisibility(testCase) {
  const v = testCase.app?.visibility;
  return v === 'visible' || v === 'hidden' || v === 'frozen' ? v : undefined;
}

/** `{a: 1, b: undefined}` → `{a: 1}`（只传用例给出的字段）。 */
export function defined(obj) {
  return Object.fromEntries(Object.entries(obj).filter(([, v]) => v !== undefined));
}

// ---- @app-mcp/node 与 @app-mcp/web 共用（两者的注册 API 同形，见 packages/node/src/types.ts 文件头） ----

/** 用例的工具声明 → `ToolDefinition` 的声明字段（未给出的不传）。 */
function jsToolFields(decl) {
  return defined({
    description: decl.description,
    title: decl.title,
    input: decl.inputSchema,
    risk: decl.risk,
    annotations: decl.annotations,
    outputSchema: decl.outputSchema,
    activation: decl.activation,
    enabled: decl.enabled,
    surface: decl.surface,
    page: decl.page,
    backgroundTool: decl.backgroundTool,
    concurrency: decl.concurrency,
    exclusive: decl.exclusive,
    implements: decl.implements,
    cache: decl.cache,
    deprecated: decl.deprecated,
    undoable: decl.undoable,
  });
}

/** 结果描述 → JS SDK 的写法：抛出 / 返回值 / 结构化结果 / `undefined`（无返回值）。 */
function jsSettle(outcome, ToolCallError) {
  switch (outcome.kind) {
    case 'throw':
      throw new Error(outcome.message);
    case 'userAction':
      throw ToolCallError.userActionRequired(outcome.message, defined({ reason: outcome.reason, uri: outcome.uri }));
    case 'result':
      // 缺 data = 无返回值：信封的 data 为 undefined
      return { data: outcome.result.data, ...defined({ ...outcome.result, data: undefined }) };
    case 'value':
      return outcome.value;
    default:
      return undefined;
  }
}

async function jsRead(spec, ToolCallError) {
  if ('return' in spec) return spec.return;
  if (spec.fail) throw new ToolCallError(spec.fail.kind ?? 'HANDLER_ERROR', spec.fail.message, spec.fail.details);
  if (spec.userAction) {
    throw ToolCallError.userActionRequired(spec.userAction.message, defined({ reason: spec.userAction.reason, uri: spec.userAction.uri }));
  }
  throw new Error(spec.throw ?? '读取失败');
}

/**
 * 按用例 `app` 部分在 JS SDK 实例上注册工具与资源、声明事件（conformance/README.md 2.1–2.3）；`app.busy` 时随后调用
 * `setBusy(true)`（runner 在本函数之后启动 SDK）。
 * @input app `@app-mcp/node` / `@app-mcp/web` 的实例；ToolCallError 该包导出的错误类。
 * @why update 为补丁型 API：`set` 中为 null 的字段以显式 undefined 清除（两包的 `ToolHandle.update` 约定）。
 * @output `{ navigate }`：用例有 `app.navigation` 时为 JS 写法的导航回调 `(page, params) => Promise<void>`
 *   （拒绝抛 `NAVIGATION_DENIED` 的 ToolCallError、失败抛 `NAVIGATION_FAILED`、`userAction` 抛
 *   `ToolCallError.userActionRequired`、`throw` 抛普通 Error），由 runner 交给 SDK；否则 undefined。
 */
export function registerJsApp(app, testCase, ToolCallError) {
  const registry = createRegistry({
    register: (decl) => {
      let runs = 0;
      const spec = decl.handler ?? {};
      return app.tool(decl.name, {
        ...jsToolFields(decl),
        handler: async (input, ctx) =>
          jsSettle(
            await execHandler(spec, {
              count: ++runs,
              args: input,
              idempotencyKey: ctx.idempotencyKey,
              progress: (p, t, m) => ctx.progress?.(p, t, m),
              isCancelled: () => ctx.signal.aborted,
              mutate: (op) => registry.mutate(op),
              emit: (name, payload) => app.emitEvent(name, payload),
            }),
            ToolCallError,
          ),
      });
    },
    update: (handle, next, set) => {
      const changes = jsToolFields(next);
      for (const [k, v] of Object.entries(set)) {
        if (v === null) changes[k === 'inputSchema' ? 'input' : k] = undefined;
      }
      handle.update(changes);
    },
    remove: (handle) => handle.dispose(),
    setEnabled: (handle, enabled) => handle.update({ enabled }),
    setBusy: (busy) => app.setBusy(busy),
    declareEvent: (event) => app.declareEvent(event),
    removeEvent: (name) => app.removeEvent(name),
  });
  for (const t of testCase.app.tools ?? []) registry.register(t);
  for (const r of testCase.app.resources ?? []) {
    app.resource(r.name, {
      ...defined({ description: r.description, mimeType: r.mimeType, realtime: r.realtime, annotations: r.annotations, cache: r.cache }),
      read: () => jsRead(r.read, ToolCallError),
    });
  }
  for (const e of appEvents(testCase)) app.declareEvent(e);
  if (appBusy(testCase)) app.setBusy(true);
  const pages = testCase.app.navigation;
  if (typeof pages !== 'object' || pages === null) return { navigate: undefined };
  const navigate = async (page, params) => {
    const outcome = execNavigation(pages, page, params, { mutate: (op) => registry.mutate(op) });
    if (outcome.kind === 'throw') throw new Error(outcome.message);
    if (outcome.kind === 'deny') throw new ToolCallError('NAVIGATION_DENIED', outcome.message);
    if (outcome.kind === 'fail') throw new ToolCallError('NAVIGATION_FAILED', outcome.message);
    if (outcome.kind === 'userAction') {
      throw ToolCallError.userActionRequired(outcome.message, defined({ reason: outcome.reason, uri: outcome.uri }));
    }
  };
  return { navigate };
}
