'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('path');
const { installFakeHarmonyKits, MemoryLogger } = require('./fake-native.cjs');

const build = process.env.APP_MCP_HARMONY_BUILD;
if (!build) throw new Error('请通过 node tests/run.cjs 运行');
const kits = installFakeHarmonyKits();
const { HarmonyAppMcp } = require(path.join(build, 'Harmony.js'));

function create(options = {}, scheme = 'shopapp') {
  const before = kits.clients.length;
  const mcp = HarmonyAppMcp.create(kits.context, { appId: 'shop', appName: '商城', logger: new MemoryLogger(), ...options }, scheme);
  return { mcp, client: kits.clients[before] };
}

test('平台默认：on-demand + 进入后台立即休眠，保留 keep 与唤醒描述', () => {
  assert.deepEqual(HarmonyAppMcp.defaultLifecycle('shopapp'), {
    mode: 'on-demand',
    residency: 'keep',
    sleepOnBackground: true,
    wake: { kind: 'uri', target: 'shopapp://app-mcp/wake', background: false },
  });
  assert.equal(HarmonyAppMcp.defaultLifecycle().wake, undefined);

  const { mcp, client } = create();
  const l = client.config.lifecycle;
  assert.equal(l.mode, 'on-demand');
  assert.equal(l.residency, 'keep');
  assert.equal(l.sleepOnBackground, true);
  assert.equal(l.wake.target, 'shopapp://app-mcp/wake');
  // 其余字段不传，取原生层默认值（合并窗口 2000、hostAbsentRetries 3、legacyTimers false）。
  assert.equal(l.mergeWindowMs, undefined);
  assert.equal(l.hostAbsentRetries, undefined);
  assert.equal(l.legacyTimers, undefined);
  assert.equal(client.config.heartbeat, undefined);
  mcp.dispose();
});

test('首次进入前台回连（on-demand 启动后 dormant），进入后台只报告可见性', () => {
  const { mcp, client } = create();
  assert.equal(client.started, 1);
  kits.appContext.fire('foreground');
  kits.appContext.fire('background');
  assert.deepEqual(client.calls, [
    ['setVisibility', 'visible', true],
    ['wake', 'visible'],
    ['setVisibility', 'hidden', false],
  ]);
  mcp.dispose();
});

test('显式生命周期整体生效，不与平台默认合并；新字段与 heartbeat 透传', () => {
  const { mcp, client } = create({
    heartbeat: 'off',
    callDedup: { ttlMs: 5, maxEntries: 2 },
    maxConcurrentCalls: 3,
    maxQueuedCalls: 7,
    lifecycle: { mode: 'idle', hostAbsentRetries: 0, legacyTimers: true, mergeWindowMs: 500, sleepOnBackground: false },
  });
  const l = client.config.lifecycle;
  assert.equal(l.mode, 'idle');
  assert.equal(l.sleepOnBackground, false);
  assert.equal(l.hostAbsentRetries, 0);
  assert.equal(l.legacyTimers, true);
  assert.equal(l.mergeWindowMs, 500);
  assert.equal(l.residency, undefined);
  // 未给 wake 时仍按 scheme 补唤醒描述。
  assert.equal(l.wake.target, 'shopapp://app-mcp/wake');
  assert.equal(client.config.heartbeat, 'off');
  assert.deepEqual(client.config.callDedup, { ttlMs: 5, maxEntries: 2 });
  assert.equal(client.config.maxConcurrentCalls, 3);
  assert.equal(client.config.maxQueuedCalls, 7);
  mcp.dispose();
});

test('onNavigate 与 navigateInBackground 透传给客户端', () => {
  const { mcp, client } = create({ onNavigate: () => {}, navigateInBackground: true });
  assert.notEqual(client.navigationHandler, undefined);
  assert.deepEqual(client.calls.filter((c) => c[0] === 'setNavigateInBackground'), [['setNavigateInBackground', true]]);
  mcp.dispose();
});

test('persistent 回到前台不调用 wake', () => {
  const { mcp, client } = create({ lifecycle: { mode: 'persistent' } });
  kits.appContext.fire('foreground');
  assert.deepEqual(client.calls, [['setVisibility', 'visible', true]]);
  mcp.dispose();
});
