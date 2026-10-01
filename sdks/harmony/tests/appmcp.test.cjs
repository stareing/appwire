'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('path');
const { fakeFactory, MemoryLogger } = require('./fake-native.cjs');

const build = process.env.APP_MCP_HARMONY_BUILD;
if (!build) throw new Error('请通过 node tests/run.cjs 运行');
const { AppMcp, mapState } = require(path.join(build, 'AppMcp.js'));
const { ToolCallError } = require(path.join(build, 'Errors.js'));
const { ToolResult } = require(path.join(build, 'Types.js'));

function create(options = {}) {
  const { factory, clients } = fakeFactory();
  const logger = new MemoryLogger();
  const mcp = new AppMcp({ appId: 'shop', appName: '商城', logger, ...options }, factory);
  return { mcp, client: clients[0], logger, clients };
}

test('配置映射：默认端点、生命周期、自动 start', () => {
  const { mcp, client } = create({
    lifecycle: { mode: 'idle', residency: 'keep', wake: { kind: 'uri', target: 'shopapp://app-mcp/wake', background: false } },
    overview: { summary: '演示' },
  });
  // 未指定 hostUrl 时不传，由原生层解析默认端点（spec/protocol.md 1.3）。
  assert.equal(client.config.hostUrl, undefined);
  assert.equal(client.config.clientKind, 'native');
  assert.equal(client.config.lifecycle.mode, 'idle');
  assert.deepEqual(client.config.lifecycle.wake, { kind: 'uri', target: 'shopapp://app-mcp/wake', background: false });
  assert.equal(client.config.overview.summary, '演示');
  assert.equal(client.started, 1);
  assert.equal(mcp.instanceId, 'inst-1');
});

test('配置映射：生命周期新字段、heartbeat 透传；未给出时不传（取原生默认）', () => {
  const { client } = create({
    heartbeat: 'always',
    lifecycle: { mode: 'on-demand', hostAbsentRetries: 0, legacyTimers: true, mergeWindowMs: 0, sleepOnBackground: true },
  });
  assert.equal(client.config.heartbeat, 'always');
  const l = client.config.lifecycle;
  assert.equal(l.mode, 'on-demand');
  // 0 = 一直重连（napi 与 spec 同义，不做编码转换）。
  assert.equal(l.hostAbsentRetries, 0);
  assert.equal(l.legacyTimers, true);
  assert.equal(l.mergeWindowMs, 0);
  assert.equal(l.sleepOnBackground, true);

  const plain = create({ lifecycle: { mode: 'idle' } }).client.config;
  assert.equal(plain.heartbeat, undefined);
  for (const key of ['hostAbsentRetries', 'legacyTimers', 'mergeWindowMs', 'sleepOnBackground']) {
    assert.equal(plain.lifecycle[key], undefined, key);
  }
  assert.equal(create().client.config.lifecycle, undefined);
});

test('hostUrl 显式指定时不用默认值；autoStart false 不连接', () => {
  const { client } = create({ hostUrl: 'ws://10.0.0.2:7717/app', autoStart: false });
  assert.equal(client.config.hostUrl, 'ws://10.0.0.2:7717/app');
  assert.equal(client.started, 0);
});

test('工具调用：返回值序列化为 JSON，ToolResult 带 stateHints', async () => {
  const { mcp, client } = create();
  mcp.tool('cart.add', {
    description: '加入购物车',
    inputSchema: '{"type":"object","properties":{"qty":{"type":"integer"}}}',
    risk: 'write',
    handler: (input) => ({ added: input.qty }),
  });
  assert.equal(client.tools.get('cart.add').spec.inputSchemaJson.includes('qty'), true);
  assert.equal(client.tools.get('cart.add').spec.enabled, true);
  const r1 = await client.invoke('cart.add', { qty: 2 }).done;
  assert.deepEqual(r1, { ok: true, dataJson: '{"added":2}', stateHints: [] });

  mcp.tool('cart.clear', { description: '清空', handler: async () => new ToolResult(null, ['cart']) });
  const r2 = await client.invoke('cart.clear').done;
  assert.deepEqual(r2, { ok: true, dataJson: 'null', stateHints: ['cart'] });

  mcp.tool('noop', { description: '无返回', handler: () => undefined });
  assert.equal((await client.invoke('noop').done).dataJson, 'null');
});

test('失败：ToolCallError 带详情、普通异常、非法 JSON、同步抛出', async () => {
  const { mcp, client } = create();
  mcp.tool('pay', {
    description: '支付',
    handler: () => {
      throw new ToolCallError('USER_REJECTED', '用户拒绝', { step: 'confirm' });
    },
  });
  assert.deepEqual(await client.invoke('pay').done, {
    ok: false,
    kind: 'USER_REJECTED',
    message: '用户拒绝',
    detailsJson: '{"step":"confirm"}',
  });

  mcp.tool('boom', { description: 'x', handler: async () => Promise.reject(new Error('坏了')) });
  assert.deepEqual(await client.invoke('boom').done, { ok: false, kind: 'HANDLER_ERROR', message: '坏了' });

  mcp.tool('json', { description: 'x', handler: (i) => i });
  const bad = await client.invoke('json', '{not json').done;
  assert.equal(bad.kind, 'INVALID_INPUT');

  mcp.tool('str', {
    description: 'x',
    handler: () => {
      throw 'plain';
    },
  });
  assert.deepEqual(await client.invoke('str').done, { ok: false, kind: 'HANDLER_ERROR', message: 'plain' });
});

test('取消：通知 onCancel，之后不再完成，ALREADY_COMPLETED 不记错误', async () => {
  const { mcp, client, logger } = create();
  let release;
  const gate = new Promise((r) => {
    release = r;
  });
  const reasons = [];
  mcp.tool('slow', {
    description: '慢',
    handler: async (_input, ctx) => {
      ctx.onCancel((reason) => reasons.push(reason));
      await gate;
      return ctx.isCancelled() ? 'late' : 'ok';
    },
  });
  const call = client.invoke('slow');
  await new Promise((r) => setImmediate(r));
  call.cancel('timeout');
  release();
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(reasons, ['timeout']);
  assert.deepEqual(call.result, { cancelled: 'timeout' });
  assert.equal(logger.lines.filter(([l]) => l === 'error').length, 0);

  // 已取消后再注册的监听器也会被回调一次
  const late = [];
  mcp.tool('t2', {
    description: 'x',
    handler: async (_i, ctx) => {
      await new Promise((r) => setImmediate(r));
      ctx.onCancel((reason) => late.push(reason));
      return 1;
    },
  });
  const c2 = client.invoke('t2');
  c2.cancel('requested');
  await new Promise((r) => setTimeout(r, 5));
  assert.deepEqual(late, ['requested']);
});

test('context.hold 返回幂等持有', async () => {
  const { mcp, client } = create();
  let call;
  mcp.tool('long', {
    description: 'x',
    handler: (_i, ctx) => {
      const h = ctx.hold();
      h.release();
      h.release();
      return 1;
    },
  });
  call = client.invoke('long');
  await call.done;
  assert.deepEqual(call.holdLog, ['release']);
});

test('重名抛出原生错误；update 合并变更；dispose 注销', () => {
  const { mcp, client } = create();
  const h = mcp.tool('a', { description: 'A', risk: 'read', handler: () => 1 });
  assert.throws(() => mcp.tool('a', { description: 'A', handler: () => 1 }), (e) => e.code === 'DUPLICATE_NAME');
  h.update({ description: 'A2', enabled: false });
  assert.deepEqual(
    { d: client.tools.get('a').spec.description, r: client.tools.get('a').spec.risk, e: client.tools.get('a').spec.enabled },
    { d: 'A2', r: 'read', e: false },
  );
  h.dispose();
  assert.equal(client.tools.has('a'), false);
  h.dispose();
});

test('scope：dispose 后子项变为空操作', () => {
  const { mcp, client } = create();
  const scope = mcp.scope('page');
  const t = scope.tool('page.x', { description: 'x', handler: () => 1 });
  assert.equal(client.tools.has('page.x'), true);
  scope.dispose();
  assert.deepEqual(client.scopeDisposals, ['page']);
  t.update({ description: 'y' });
  assert.equal(client.tools.get('page.x').spec.description, 'x');
  // 已注销的 scope 再注册为空操作
  scope.tool('page.y', { description: 'y', handler: () => 1 });
  assert.equal(client.tools.has('page.y'), false);
});

test('资源读取与变更通知', async () => {
  const { mcp, client } = create();
  const r = mcp.resource('cart', { description: '购物车', read: async () => ({ items: 1 }) });
  assert.equal(client.resources.get('cart').spec.mimeType, 'application/json');
  assert.equal(client.resources.get('cart').spec.realtime, undefined);
  mcp.resource('order.status', { description: '订单状态', realtime: true, read: () => 'paid' });
  assert.equal(client.resources.get('order.status').spec.realtime, true);
  assert.deepEqual(await client.read('cart').done, { ok: true, contentsJson: '{"items":1}' });
  r.notifyChanged();
  assert.equal(client.resources.get('cart').changed, 1);
  mcp.resource('bad', {
    description: 'x',
    read: () => {
      throw new ToolCallError('RESOURCE_NOT_FOUND', '无');
    },
  });
  assert.deepEqual(await client.read('bad').done, { ok: false, kind: 'RESOURCE_NOT_FOUND', message: '无' });
});

test('状态事件映射与监听', () => {
  const { mcp, client } = create();
  const seen = [];
  const off = mcp.onStateChange((s) => seen.push(s));
  const before = Date.now();
  client.emit({ type: 'state', state: { status: 'backoff', retryInMs: 1000, reason: '连不上', code: 'HOST_NOT_RUNNING' } });
  assert.equal(seen[0].status, 'backoff');
  assert.ok(seen[0].retryAt >= before + 1000);
  assert.equal(seen[0].code, 'HOST_NOT_RUNNING');
  client.emit({ type: 'state', state: { status: 'rejected' } });
  assert.deepEqual(seen[1], { status: 'rejected', reason: '', code: 'REJECTED' });
  off();
  client.emit({ type: 'state', state: { status: 'connected' } });
  assert.equal(seen.length, 2);
  assert.equal(mcp.state.status, 'connected');
  assert.deepEqual(mapState({ status: 'host-mismatch', reason: '不是本用户' }, 0), {
    status: 'host-mismatch',
    reason: '不是本用户',
    code: 'HOST_NOT_APP_MCP',
  });
  assert.equal(mapState({ status: 'future-status' }, 0).status, 'idle');
});

test('配对、日志、idle-exit 事件', () => {
  const tokens = [];
  let exits = 0;
  const { mcp, client, logger } = create({ onPaired: (t) => tokens.push(t), onIdleExit: () => exits++ });
  let extra = 0;
  mcp.onIdleExit(() => extra++);
  client.emit({ type: 'paired', token: 'tok' });
  client.emit({ type: 'log', level: 'warn', message: '注意' });
  client.emit({ type: 'idle-exit' });
  assert.deepEqual(tokens, ['tok']);
  assert.deepEqual(logger.lines, [['warn', '[app-mcp] 注意']]);
  assert.equal(exits, 1);
  assert.equal(extra, 1);
});

test('生命周期方法转发；handleWake 逐个尝试', () => {
  const { mcp, client } = create();
  assert.equal(mcp.handleWake(['shopapp://other', 'app-mcp-wake:abc']), true);
  assert.equal(mcp.handleWake('nothing'), false);
  assert.equal(mcp.wake('visible'), true);
  assert.equal(mcp.sleep(), true);
  assert.equal(mcp.toolsHash(), 'abcd');
  mcp.setVisibility('hidden', false);
  assert.deepEqual(client.calls.filter((c) => c[0] !== 'handleWake'), [
    ['wake', 'visible'],
    ['sleep', undefined],
    ['setVisibility', 'hidden', false],
  ]);
});

test('dispose：停止原生客户端、状态 stopped、之后全部为空操作', () => {
  const { mcp, client } = create();
  const states = [];
  mcp.onStateChange((s) => states.push(s.status));
  mcp.dispose();
  mcp.dispose();
  assert.equal(client.stopped, 1);
  assert.deepEqual(states, ['stopped']);
  assert.equal(mcp.isDisposed, true);
  assert.equal(mcp.handleWake('app-mcp-wake:x'), false);
  assert.equal(mcp.token, null);
  mcp.tool('after', { description: 'x', handler: () => 1 });
  assert.equal(client.tools.has('after'), false);
});

test('enabled false：不创建原生客户端', () => {
  const { mcp, clients } = create({ enabled: false });
  assert.equal(clients.length, 0);
  assert.equal(mcp.state.status, 'disabled');
  mcp.tool('x', { description: 'x', handler: () => 1 }).dispose();
  mcp.hold().release();
  assert.equal(mcp.toolsHash(), '');
});
