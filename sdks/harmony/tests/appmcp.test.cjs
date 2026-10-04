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
    callDedup: { ttlMs: 1000, maxEntries: 0 },
    maxQueuedCalls: 0,
    lifecycle: { mode: 'on-demand', hostAbsentRetries: 0, legacyTimers: true, mergeWindowMs: 0, sleepOnBackground: true },
  });
  assert.equal(client.config.heartbeat, 'always');
  assert.deepEqual(client.config.callDedup, { ttlMs: 1000, maxEntries: 0 });
  assert.equal(client.config.maxQueuedCalls, 0);
  const l = client.config.lifecycle;
  assert.equal(l.mode, 'on-demand');
  // 0 = 一直重连（napi 与 spec 同义，不做编码转换）。
  assert.equal(l.hostAbsentRetries, 0);
  assert.equal(l.legacyTimers, true);
  assert.equal(l.mergeWindowMs, 0);
  assert.equal(l.sleepOnBackground, true);

  const plain = create({ lifecycle: { mode: 'idle' } }).client.config;
  assert.equal(plain.heartbeat, undefined);
  assert.equal(plain.callDedup, undefined);
  assert.equal(plain.maxQueuedCalls, undefined);
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

test('handler 上下文带 Agent 的幂等键（spec/protocol.md 3.3），没有时为 undefined', async () => {
  const { mcp, client } = create();
  mcp.tool('order.create', { description: '下单', handler: (_input, ctx) => ({ key: ctx.idempotencyKey ?? 'none' }) });
  assert.equal((await client.invoke('order.create', {}, 'order-7').done).dataJson, '{"key":"order-7"}');
  assert.equal((await client.invoke('order.create').done).dataJson, '{"key":"none"}');
});

test('注解与 outputSchema 随注册下发；update 整体替换', () => {
  const { mcp, client } = create();
  const h = mcp.tool('order.submit', {
    description: '下单',
    risk: 'payment',
    annotations: { idempotentHint: false, openWorldHint: true, title: '提交订单' },
    outputSchema: '{"type":"object","properties":{"orderId":{"type":"string"}}}',
    handler: () => ({ orderId: 'o1' }),
  });
  const spec = client.tools.get('order.submit').spec;
  assert.deepEqual(
    { ...spec.annotations },
    { title: '提交订单', readOnlyHint: undefined, destructiveHint: undefined, idempotentHint: false, openWorldHint: true },
  );
  assert.equal(spec.outputSchemaJson, '{"type":"object","properties":{"orderId":{"type":"string"}}}');
  h.update({ annotations: { readOnlyHint: true }, description: '下单2' });
  const next = client.tools.get('order.submit').spec;
  assert.equal(next.annotations.readOnlyHint, true);
  assert.equal(next.annotations.openWorldHint, undefined);
  assert.equal(next.outputSchemaJson, spec.outputSchemaJson, '未给出的 outputSchema 保持不变');
  assert.equal(next.description, '下单2');
});

test('update：未提供的字段保持不变，显式 null 清除，给值则替换', () => {
  const { mcp, client } = create();
  const h = mcp.tool('doc.save', {
    description: '保存',
    title: '保存文档',
    inputSchema: '{"type":"object","properties":{}}',
    risk: 'destructive',
    annotations: { readOnlyHint: false },
    outputSchema: '{"type":"object"}',
    activation: 'foreground',
    enabled: false,
    handler: () => null,
  });
  const fields = () => {
    const s = client.tools.get('doc.save').spec;
    return {
      description: s.description,
      title: s.title,
      inputSchemaJson: s.inputSchemaJson,
      risk: s.risk,
      readOnlyHint: s.annotations === undefined ? undefined : s.annotations.readOnlyHint,
      outputSchemaJson: s.outputSchemaJson,
      activation: s.activation,
      enabled: s.enabled,
    };
  };
  const before = fields();
  h.update({ description: '保存2', title: undefined });
  assert.deepEqual(fields(), { ...before, description: '保存2' }, '未提供 / undefined 的字段保持不变');

  h.update({ title: '另存', activation: 'background', outputSchema: '{"type":"array"}' });
  assert.deepEqual(fields(), {
    ...before, description: '保存2', title: '另存', activation: 'background', outputSchemaJson: '{"type":"array"}',
  });

  h.update({
    description: null, title: null, inputSchema: null, risk: null, annotations: null, outputSchema: null,
    activation: null, enabled: null,
  });
  assert.deepEqual(fields(), {
    description: '保存2', title: undefined, inputSchemaJson: undefined, risk: undefined, readOnlyHint: undefined,
    outputSchemaJson: undefined, activation: undefined, enabled: true,
  }, 'null 清除声明，description 不可清除');
});

test('结构化结果：pending + stateResource + summary + 内容注解；普通返回值不变', async () => {
  const { mcp, client } = create();
  mcp.tool('order.pay', {
    description: '付款',
    handler: () => new ToolResult({ orderId: 'o1' }, ['cart'], {
      status: 'pending', stateResource: 'order.state', summary: '等待付款', annotations: { audience: ['user'], priority: 0.5 },
    }),
  });
  const r = await client.invoke('order.pay').done;
  assert.deepEqual(r, {
    ok: true, dataJson: '{"orderId":"o1"}', stateHints: ['cart'], status: 'pending', stateResource: 'order.state',
    summary: '等待付款', annotations: { audience: ['user'], priority: 0.5, lastModified: undefined },
  });
  // 普通对象（即使含 data / status 键）按原样作为数据
  mcp.tool('plain', { description: 'p', handler: () => ({ data: 1, status: 'pending' }) });
  assert.deepEqual(await client.invoke('plain').done, { ok: true, dataJson: '{"data":1,"status":"pending"}', stateHints: [] });
  // 取值不合法：HANDLER_ERROR，不挂起
  mcp.tool('bad', { description: 'b', handler: () => new ToolResult(1, [], { status: 'bogus' }) });
  const bad = await client.invoke('bad').done;
  assert.equal(bad.ok, false);
  assert.equal(bad.kind, 'HANDLER_ERROR');
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

  mcp.tool('login', {
    description: '登录',
    handler: () => {
      throw ToolCallError.userActionRequired('请先登录', { reason: 'login', uri: 'shop://login' });
    },
  });
  assert.deepEqual(await client.invoke('login').done, {
    ok: false,
    kind: 'USER_ACTION_REQUIRED',
    message: '请先登录',
    detailsJson: '{"reason":"login","uri":"shop://login"}',
  });
  mcp.tool('fg', { description: '前台', handler: async () => Promise.reject(ToolCallError.userActionRequired('请切到前台')) });
  assert.deepEqual(await client.invoke('fg').done, { ok: false, kind: 'USER_ACTION_REQUIRED', message: '请切到前台' });
  mcp.tool('perm', {
    description: '权限',
    handler: () => {
      throw ToolCallError.userActionRequired('请授予相机权限', { reason: 'permission' });
    },
  });
  assert.deepEqual(await client.invoke('perm').done, {
    ok: false,
    kind: 'USER_ACTION_REQUIRED',
    message: '请授予相机权限',
    detailsJson: '{"reason":"permission"}',
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

test('context.progress 交给原生层；调用结束后无副作用、不抛出', async () => {
  const { mcp, client } = create();
  let saved;
  mcp.tool('export', {
    description: 'x',
    handler: (_i, ctx) => {
      ctx.progress(1, 3, '第 1 页');
      ctx.progress(2);
      saved = ctx;
      return 1;
    },
  });
  const call = client.invoke('export');
  await call.done;
  assert.deepEqual(call.progressLog, [
    [1, 3, '第 1 页'],
    [2, null, null],
  ]);
  assert.doesNotThrow(() => saved.progress(3));
  assert.equal(call.progressLog.length, 2);
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
  // 未声明 mimeType 时不发送缺省值（Host 按 application/json 处理；toolsHash 输入与其他 SDK 一致）
  assert.equal(client.resources.get('cart').spec.mimeType, undefined);
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
  assert.equal(client.resources.get('cart').spec.annotations, undefined);
});

test('资源的内容标注随登记下发；读取失败携带详情（userActionRequired 的 reason / uri）', async () => {
  const { mcp, client } = create();
  mcp.resource('profile', {
    description: '个人资料',
    annotations: { audience: ['user'], priority: 0.5 },
    read: () => {
      throw ToolCallError.userActionRequired('登录已过期', { reason: 'login', uri: 'shop://login' });
    },
  });
  assert.deepEqual(client.resources.get('profile').spec.annotations, { audience: ['user'], priority: 0.5, lastModified: undefined });
  assert.deepEqual(await client.read('profile').done, {
    ok: false,
    kind: 'USER_ACTION_REQUIRED',
    message: '登录已过期',
    detailsJson: '{"reason":"login","uri":"shop://login"}',
  });
  mcp.resource('stock', { description: '库存', read: async () => Promise.reject(new ToolCallError('RESOURCE_NOT_FOUND', '离线', { warehouse: 'sh' })) });
  assert.deepEqual(await client.read('stock').done, {
    ok: false,
    kind: 'RESOURCE_NOT_FOUND',
    message: '离线',
    detailsJson: '{"warehouse":"sh"}',
  });
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

test('surface / page 声明；update 以 null 清除', () => {
  const { mcp, client } = create();
  mcp.tool('a.plain', { description: 'a', handler: () => 1 });
  assert.equal(client.tools.get('a.plain').spec.surface, undefined);
  assert.equal(client.tools.get('a.plain').spec.page, undefined);
  const t = mcp.tool('cart.checkout', { description: '结算', surface: 'view', page: 'cart', handler: () => 1 });
  assert.equal(client.tools.get('cart.checkout').spec.surface, 'view');
  assert.equal(client.tools.get('cart.checkout').spec.page, 'cart');
  t.update({ description: '结算2' });
  assert.equal(client.tools.get('cart.checkout').spec.page, 'cart');
  t.update({ page: null, surface: null });
  assert.equal(client.tools.get('cart.checkout').spec.page, undefined);
  assert.equal(client.tools.get('cart.checkout').spec.surface, undefined);
});

test('onNavigate：完成 / 拒绝 / 失败 / 同步抛出 / 参数', async () => {
  const seen = [];
  const { mcp, client } = create({
    onNavigate: async ({ page, params }) => {
      seen.push([page, params]);
      if (page === 'login') throw ToolCallError.navigationDenied('需要先登录');
      if (page === 'broken') throw new Error('页面加载失败');
    },
  });
  assert.deepEqual(await client.navigate('cart'), { ok: true });
  assert.deepEqual(await client.navigate('detail', { sku: 'A-42' }), { ok: true });
  assert.deepEqual(seen, [['cart', undefined], ['detail', { sku: 'A-42' }]]);
  assert.deepEqual(await client.navigate('login'), { ok: false, kind: 'deny', message: '需要先登录' });
  assert.deepEqual(await client.navigate('broken'), { ok: false, kind: 'fail', message: '页面加载失败' });
  assert.deepEqual(await client.navigate('bad', '{x'), { ok: false, kind: 'fail', message: '页面参数不是合法的 JSON' });
  mcp.setNavigationHandler(() => {
    throw new Error('同步异常');
  });
  assert.deepEqual(await client.navigate('x'), { ok: false, kind: 'fail', message: '同步异常' });
  mcp.setNavigationHandler(null);
  assert.equal((await client.navigate('cart')).kind, 'unsupported');
});

test('backgroundTool 声明；update 以 null 清除', () => {
  const { mcp, client } = create();
  mcp.tool('cart.add', { description: '加入', handler: () => 1 });
  assert.equal(client.tools.get('cart.add').spec.backgroundTool, undefined);
  const t = mcp.tool('cart.viewAdd', { description: 'V', surface: 'view', page: 'cart', backgroundTool: 'cart.add', handler: () => 1 });
  assert.equal(client.tools.get('cart.viewAdd').spec.backgroundTool, 'cart.add');
  t.update({ description: 'V2' });
  assert.equal(client.tools.get('cart.viewAdd').spec.backgroundTool, 'cart.add');
  t.update({ backgroundTool: null });
  assert.equal(client.tools.get('cart.viewAdd').spec.backgroundTool, undefined);
});

test('标准意图 implements 声明（缺省或空数组不带）；update 替换、未给出保持、null 清除', () => {
  const { mcp, client } = create();
  mcp.tool('plain', { description: 'P', handler: () => 1 });
  mcp.tool('empty', { description: 'E', implements: [], handler: () => 1 });
  assert.equal(client.tools.get('plain').spec.implements, undefined);
  assert.equal(client.tools.get('empty').spec.implements, undefined);
  const t = mcp.tool('mail.send', { description: 'S', implements: ['message.send@1'], handler: () => 1 });
  assert.deepEqual(client.tools.get('mail.send').spec.implements, ['message.send@1']);
  t.update({ description: 'S2' });
  assert.deepEqual(client.tools.get('mail.send').spec.implements, ['message.send@1']);
  t.update({ implements: ['message.send@1', 'file.share@1'] });
  assert.deepEqual(client.tools.get('mail.send').spec.implements, ['message.send@1', 'file.share@1']);
  t.update({ implements: null });
  assert.equal(client.tools.get('mail.send').spec.implements, undefined);
});

test('结果缓存声明 cache（工具与资源，未声明不带，复制）；update 替换、未给出保持、null 清除', () => {
  const { mcp, client } = create();
  mcp.tool('plain', { description: 'P', handler: () => 1 });
  assert.equal(client.tools.get('plain').spec.cache, undefined);
  const decl = { ttlMs: 5000, scope: 'shared' };
  const t = mcp.tool('feed.list', { description: 'L', risk: 'read', cache: decl, handler: () => 1 });
  assert.deepEqual(client.tools.get('feed.list').spec.cache, { ttlMs: 5000, scope: 'shared' });
  assert.notEqual(client.tools.get('feed.list').spec.cache, decl);
  mcp.tool('feed.mine', { description: 'M', risk: 'read', cache: { ttlMs: 10 }, handler: () => 1 });
  assert.deepEqual(client.tools.get('feed.mine').spec.cache, { ttlMs: 10, scope: undefined });
  t.update({ description: 'L2' });
  assert.deepEqual(client.tools.get('feed.list').spec.cache, { ttlMs: 5000, scope: 'shared' });
  t.update({ cache: { ttlMs: 1000 } });
  assert.deepEqual(client.tools.get('feed.list').spec.cache, { ttlMs: 1000, scope: undefined });
  t.update({ cache: null });
  assert.equal(client.tools.get('feed.list').spec.cache, undefined);
  mcp.resource('feed', { description: 'F', cache: { ttlMs: 30000, scope: 'shared' }, read: () => [] });
  mcp.resource('cart', { description: 'C', read: () => [] });
  assert.deepEqual(client.resources.get('feed').spec.cache, { ttlMs: 30000, scope: 'shared' });
  assert.equal(client.resources.get('cart').spec.cache, undefined);
});

test('撤销：undoable 随定义注册，update 未给出保持、false / null 取消；结果 undo 经 completeWith 交给原生（参数为 JSON 文本）', async () => {
  const { mcp, client } = create();
  mcp.tool('plain', { description: 'P', handler: () => 1 });
  assert.equal(client.tools.get('plain').spec.undoable, undefined);
  const t = mcp.tool('todo.add', {
    description: '添加',
    undoable: true,
    handler: () => new ToolResult({ id: 3 }, [], { undo: { tool: 'todo.remove', arguments: { id: 3 }, label: '删除刚添加的待办' } }),
  });
  assert.equal(client.tools.get('todo.add').spec.undoable, true);
  t.update({ description: '添加 2' });
  assert.equal(client.tools.get('todo.add').spec.undoable, true);
  t.update({ undoable: false });
  assert.equal(client.tools.get('todo.add').spec.undoable, false);
  t.update({ undoable: true });
  t.update({ undoable: null });
  assert.equal(client.tools.get('todo.add').spec.undoable, undefined);

  assert.deepEqual(await client.invoke('todo.add').done, {
    ok: true, dataJson: '{"id":3}', stateHints: [], status: undefined, stateResource: undefined, summary: undefined,
    undo: { tool: 'todo.remove', argumentsJson: '{"id":3}', label: '删除刚添加的待办' },
  });
  // 只有 undo（无其他附加字段）也走 completeWith；内容不在封装层校验
  mcp.tool('toggle', { description: 'T', handler: () => new ToolResult(true, [], { undo: { tool: 'bad name' } }) });
  const r = await client.invoke('toggle').done;
  assert.deepEqual(r.undo, { tool: 'bad name', label: undefined });
});

test('弃用声明 deprecated（未声明不带，复制）；update 替换、未给出保持、null 清除', () => {
  const { mcp, client } = create();
  mcp.tool('plain', { description: 'P', handler: () => 1 });
  assert.equal(client.tools.get('plain').spec.deprecated, undefined);
  const decl = { message: '改用 o.new', replacement: 'o.new', until: '2027-06-30' };
  const t = mcp.tool('o.old', { description: 'O', deprecated: decl, handler: () => 1 });
  assert.deepEqual(client.tools.get('o.old').spec.deprecated, { message: '改用 o.new', replacement: 'o.new', until: '2027-06-30' });
  assert.notEqual(client.tools.get('o.old').spec.deprecated, decl);
  t.update({ description: 'O2' });
  assert.deepEqual(client.tools.get('o.old').spec.deprecated, { message: '改用 o.new', replacement: 'o.new', until: '2027-06-30' });
  t.update({ deprecated: { message: '即将移除' } });
  assert.deepEqual(client.tools.get('o.old').spec.deprecated, { message: '即将移除', replacement: undefined, until: undefined });
  t.update({ deprecated: null });
  assert.equal(client.tools.get('o.old').spec.deprecated, undefined);
});

test('调用调度声明 concurrency / exclusive；update 以 null 清除', () => {
  const { mcp, client } = create();
  mcp.tool('plain', { description: 'P', handler: () => 1 });
  assert.equal(client.tools.get('plain').spec.concurrency, undefined);
  assert.equal(client.tools.get('plain').spec.exclusive, undefined);
  const t = mcp.tool('doc.edit', { description: '改', concurrency: 2, exclusive: 'doc', handler: () => 1 });
  assert.equal(client.tools.get('doc.edit').spec.concurrency, 2);
  assert.equal(client.tools.get('doc.edit').spec.exclusive, 'doc');
  t.update({ description: '改2' });
  assert.equal(client.tools.get('doc.edit').spec.exclusive, 'doc');
  t.update({ concurrency: null, exclusive: null });
  assert.equal(client.tools.get('doc.edit').spec.concurrency, undefined);
  assert.equal(client.tools.get('doc.edit').spec.exclusive, undefined);
});

test('导航回调抛出 userActionRequired → USER_ACTION_REQUIRED（带 reason / uri）', async () => {
  const { client } = create({
    onNavigate: ({ page }) => {
      if (page === 'bare') throw ToolCallError.userActionRequired('请切到前台');
      throw ToolCallError.userActionRequired('已发通知，请点开', { reason: 'foreground', uri: 'shop://cart' });
    },
  });
  assert.deepEqual(await client.navigate('cart'), {
    ok: false, kind: 'userAction', message: '已发通知，请点开', reason: 'foreground', uri: 'shop://cart',
  });
  assert.deepEqual(await client.navigate('bare'), { ok: false, kind: 'userAction', message: '请切到前台', reason: null, uri: null });
});

test('navigateInBackground：缺省不调用原生设置（平台缺省 false，后台导航不调用回调）；选项与 setNavigateInBackground 传给原生', async () => {
  let runs = 0;
  const { mcp, client } = create({ onNavigate: () => { runs++; } });
  assert.equal(client.calls.some((c) => c[0] === 'setNavigateInBackground'), false);
  mcp.setVisibility('hidden', false);
  assert.deepEqual(await client.navigate('cart'), {
    ok: false, kind: 'userAction', message: 'App 在后台，无法切换到页面「cart」', reason: 'foreground', uri: null,
  });
  assert.equal(runs, 0);
  mcp.setNavigateInBackground(true);
  assert.deepEqual(await client.navigate('cart'), { ok: true });
  assert.equal(runs, 1);
  const other = create({ navigateInBackground: true });
  assert.deepEqual(other.client.calls.filter((c) => c[0] === 'setNavigateInBackground'), [['setNavigateInBackground', true]]);
});

test('未设置导航回调时不声明', () => {
  const { client } = create();
  assert.equal(client.navigationHandler, undefined);
});

test('用户正在操作：busyPolicy 配置透传（缺省不传）；setBusy / isBusy / setBusyPolicy 转给原生，非法值抛错', () => {
  assert.equal(create().client.config.busyPolicy, undefined);
  const { mcp, client } = create({ busyPolicy: 'queue' });
  assert.equal(client.config.busyPolicy, 'queue');
  assert.equal(mcp.isBusy(), false);
  mcp.setBusy(true);
  assert.equal(mcp.isBusy(), true);
  mcp.setBusy(false);
  mcp.setBusyPolicy('reject');
  assert.throws(() => mcp.setBusyPolicy('drop'), /busyPolicy/);
  assert.deepEqual(
    client.calls.filter((c) => c[0] === 'setBusy' || c[0] === 'setBusyPolicy'),
    [['setBusy', true], ['setBusy', false], ['setBusyPolicy', 'reject']],
  );
  mcp.dispose();
  mcp.setBusy(true);
  assert.equal(mcp.isBusy(), false);
  assert.equal(client.calls.filter((c) => c[0] === 'setBusy').length, 2);
});

test('用户正在操作：beginBusy 作用域可嵌套、按引用计数；与显式开关互不清除；只在有效值变化时调用原生', () => {
  const { mcp, client } = create();
  const outer = mcp.beginBusy();
  const inner = mcp.beginBusy();
  outer.release();
  outer.release();
  assert.equal(mcp.isBusy(), true);
  mcp.setBusy(true);
  inner.release();
  assert.equal(mcp.isBusy(), true);
  const scope = mcp.beginBusy();
  mcp.setBusy(false);
  assert.equal(mcp.isBusy(), true);
  scope.release();
  assert.equal(mcp.isBusy(), false);
  assert.deepEqual(client.calls.filter((c) => c[0] === 'setBusy'), [['setBusy', true], ['setBusy', false]]);
  const late = mcp.beginBusy();
  mcp.dispose();
  late.release();
  mcp.beginBusy();
  assert.equal(client.calls.filter((c) => c[0] === 'setBusy').length, 3);
});

test('事件：declareEvent 把 payloadSchema 文本交给原生；emitEvent 未连接 false、已连接 true；错误码透传；释放后空操作', () => {
  const { mcp, client } = create();
  mcp.declareEvent({ name: 'order.shipped', description: '订单已发货', payloadSchema: '{"type":"object"}' });
  mcp.declareEvent({ name: 'download.done', description: '下载完成' });
  assert.deepEqual(client.calls.filter((c) => c[0] === 'declareEvent'), [
    ['declareEvent', { name: 'order.shipped', description: '订单已发货', payloadSchemaJson: '{"type":"object"}' }],
    ['declareEvent', { name: 'download.done', description: '下载完成', payloadSchemaJson: undefined }],
  ]);
  assert.equal(mcp.emitEvent('order.shipped', { orderId: 'o0' }), false);
  client.state = { status: 'connected' };
  assert.equal(mcp.emitEvent('order.shipped', { orderId: 'o1' }), true);
  assert.equal(mcp.emitEvent('download.done'), true);
  assert.throws(() => mcp.emitEvent('nope'), (e) => e.code === 'INVALID_NAME');
  assert.throws(() => mcp.emitEvent('order.shipped', [1]), (e) => e.code === 'INVALID_JSON');
  assert.throws(() => mcp.emitEvent('order.shipped', 1), (e) => e.code === 'INVALID_JSON');
  assert.deepEqual(client.calls.filter((c) => c[0] === 'emitEvent'), [
    ['emitEvent', 'order.shipped', '{"orderId":"o0"}'],
    ['emitEvent', 'order.shipped', '{"orderId":"o1"}'],
    ['emitEvent', 'download.done', null],
  ]);
  assert.equal(mcp.removeEvent('download.done'), true);
  assert.equal(mcp.removeEvent('download.done'), false);
  mcp.dispose();
  mcp.declareEvent({ name: 'late', description: 'x' });
  assert.equal(mcp.emitEvent('order.shipped'), false);
  assert.equal(mcp.removeEvent('order.shipped'), false);
  assert.equal(client.calls.filter((c) => c[0] === 'declareEvent').length, 2);
});
