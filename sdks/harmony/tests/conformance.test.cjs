// 一致性用例 runner（鸿蒙 ArkTS 封装层 @app-mcp/harmony）：按 conformance/cases/*.json 注册工具与资源，连接 fake_host
// （`--case` 模式，核对在 fake_host 内完成）。格式与约定见 conformance/README.md；公共部分在 conformance/runner/support.mjs。
//
// 被测对象：AppMcp.ets / Errors.ets / Types.ets（由 run.cjs 转译为 CommonJS）+ 真实原生客户端。
// @why 原生模块用 @app-mcp/node 的 Node 构建（packages/node/native/app_mcp_node.<平台>.node）：libapp_mcp_harmony.so
//   由 napi-ohos 编译同一份源码 bindings/node/src/lib.rs（bindings/harmony/src/lib.rs），JS 侧 API 逐字相同；
//   .so 只能在 ArkTS 运行时加载。因此本 runner 覆盖封装层 + 共享原生源码 + 真实协议，不覆盖 napi-ohos 与设备运行时。
// 前置条件：`pnpm --filter @app-mcp/node build:native`（或 APP_MCP_NODE_NATIVE）；能构建 / 找到 fake_host
// （或 APP_MCP_FAKE_HOST）。缺少时跳过。用法：node tests/run.cjs（APP_MCP_CONFORMANCE_CASES=id1,id2 只跑部分用例）。
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');

const SDK = 'harmony';
/** 本 runner 支持的用例能力（conformance/README.md 第 4 节）。 */
const FEATURES = [
  'toolOptions', 'mutate', 'lifecycle', 'wake', 'richResult', 'userAction', 'progress', 'resourceOptions', 'readFailure',
  'surface', 'navigation', 'backgroundTool', 'backgroundNavigation',
];

const build = process.env.APP_MCP_HARMONY_BUILD;
if (!build) throw new Error('请通过 node tests/run.cjs 运行');
const { AppMcp } = require(path.join(build, 'AppMcp.js'));
const { ToolCallError } = require(path.join(build, 'Errors.js'));
const { ToolResult } = require(path.join(build, 'Types.js'));

const repoRoot = path.resolve(__dirname, '..', '..', '..');
const nativePath =
  process.env.APP_MCP_NODE_NATIVE ||
  path.join(repoRoot, 'packages', 'node', 'native', `app_mcp_node.${process.platform}-${process.arch}.node`);

const silentLogger = { debug() {}, warn() {}, error() {} };

/** `{a: 1, b: undefined}` → `{a: 1}`。 */
const defined = (obj) => Object.fromEntries(Object.entries(obj).filter(([, v]) => v !== undefined));
const jsonText = (v) => (v === undefined ? undefined : JSON.stringify(v));

/** 用例的工具声明 → ArkTS `ToolDefinition` 的声明字段（schema 为 JSON 文本；未给出的不传）。 */
function toolFields(decl) {
  return defined({
    description: decl.description,
    title: decl.title,
    inputSchema: jsonText(decl.inputSchema),
    risk: decl.risk,
    annotations: decl.annotations,
    outputSchema: jsonText(decl.outputSchema),
    activation: decl.activation,
    enabled: decl.enabled,
    surface: decl.surface,
    page: decl.page,
    backgroundTool: decl.backgroundTool,
  });
}

/** 结果描述 → ArkTS 的写法：抛出 / 返回值 / `ToolResult` / `undefined`（无返回值）。 */
function settle(outcome) {
  switch (outcome.kind) {
    case 'throw':
      throw new Error(outcome.message);
    case 'userAction':
      throw ToolCallError.userActionRequired(outcome.message, defined({ reason: outcome.reason, uri: outcome.uri }));
    case 'result': {
      const r = outcome.result;
      const options = defined({ status: r.status, stateResource: r.stateResource, summary: r.summary, annotations: r.annotations });
      return new ToolResult(r.data, r.stateHints ?? [], options);
    }
    case 'value':
      return outcome.value;
    default:
      return undefined;
  }
}

async function readResource(spec) {
  if ('return' in spec) return spec.return;
  if (spec.fail) throw new ToolCallError(spec.fail.kind ?? 'HANDLER_ERROR', spec.fail.message, spec.fail.details);
  if (spec.userAction) {
    throw ToolCallError.userActionRequired(spec.userAction.message, defined({ reason: spec.userAction.reason, uri: spec.userAction.uri }));
  }
  throw new Error(spec.throw ?? '读取失败');
}

function startApp(support, native, testCase, url) {
  const app = new AppMcp(
    {
      appId: 'conf',
      appName: 'Conformance',
      hostUrl: url,
      autoStart: false,
      logger: silentLogger,
      ...support.appConfig(testCase),
    },
    (config, listener) => new native.NativeClient(config, listener),
  );
  const registry = support.createRegistry({
    register: (decl) => {
      let runs = 0;
      const spec = decl.handler ?? {};
      return app.tool(decl.name, {
        ...toolFields(decl),
        handler: async (input, ctx) =>
          settle(
            await support.execHandler(spec, {
              count: ++runs,
              args: input,
              progress: (p, t, m) => ctx.progress(p, t, m),
              isCancelled: () => ctx.isCancelled(),
              mutate: (op) => registry.mutate(op),
            }),
          ),
      });
    },
    // 补丁型 API（ToolChanges）：set 中为 null 的字段直接传 null 清除
    update: (handle, next, set) => {
      const changes = toolFields(next);
      for (const [k, v] of Object.entries(set)) {
        if (v === null) changes[k] = null;
      }
      handle.update(changes);
    },
    remove: (handle) => handle.dispose(),
    setEnabled: (handle, enabled) => handle.update({ enabled }),
  });
  for (const t of testCase.app.tools ?? []) registry.register(t);
  for (const r of testCase.app.resources ?? []) {
    app.resource(r.name, {
      ...defined({ description: r.description, mimeType: r.mimeType, realtime: r.realtime, annotations: r.annotations }),
      read: () => readResource(r.read),
    });
  }
  const pages = testCase.app.navigation;
  if (pages) {
    // 导航行为表（conformance/README.md 2.4）→ ArkTS 写法：拒绝抛 navigationDenied、失败抛 NAVIGATION_FAILED、
    // userAction 抛 userActionRequired、throw 抛普通 Error
    app.setNavigationHandler(async ({ page, params }) => {
      const outcome = support.execNavigation(pages, page, params, { mutate: (op) => registry.mutate(op) });
      if (outcome.kind === 'throw') throw new Error(outcome.message);
      if (outcome.kind === 'deny') throw ToolCallError.navigationDenied(outcome.message);
      if (outcome.kind === 'fail') throw ToolCallError.navigationFailed(outcome.message);
      if (outcome.kind === 'userAction') {
        throw ToolCallError.userActionRequired(outcome.message, defined({ reason: outcome.reason, uri: outcome.uri }));
      }
    });
  }
  const visibility = support.appVisibility(testCase);
  if (visibility) app.setVisibility(visibility, false);
  app.start();
  return { handleWake: (arg) => void app.handleWake(arg), stop: () => app.dispose() };
}

test('一致性用例（harmony）', async (t) => {
  if (!fs.existsSync(nativePath)) {
    t.skip(`没有 Node 原生模块 ${nativePath}（pnpm --filter @app-mcp/node build:native）`);
    return;
  }
  const support = await import(pathToFileURL(path.join(repoRoot, 'conformance', 'runner', 'support.mjs')).href);
  const bin = support.findFakeHost();
  if (!bin) {
    t.skip('找不到 fake_host（cargo build -p app-mcp-native --example fake_host，或设 APP_MCP_FAKE_HOST）');
    return;
  }
  const native = require(nativePath);
  for (const casePath of support.caseFiles()) {
    await t.test(path.basename(casePath, '.json'), { timeout: 60_000 }, async () => {
      const outcome = await support.runCase({
        bin,
        casePath,
        sdk: SDK,
        features: FEATURES,
        start: (testCase, url) => startApp(support, native, testCase, url),
      });
      assert.ok(support.verdictOk(outcome), support.describeFailure(outcome));
    });
  }
});
