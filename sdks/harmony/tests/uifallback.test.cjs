'use strict';
// 进程内控件兜底（spec/ui-fallback.md）：平台无关引擎 + ArkUI 适配，用假的检查器树（窗口驱动）在 Node 上运行。

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('path');
const Module = require('module');
const { fakeFactory, MemoryLogger } = require('./fake-native.cjs');

const build = process.env.APP_MCP_HARMONY_BUILD;
if (!build) throw new Error('请通过 node tests/run.cjs 运行');

// ArkUiDriver.ets 引用的 Kit 与全局量（ArkUI 运行时注入）的假实现。
const KEYCODES = { KEYCODE_ENTER: 2054, KEYCODE_ESCAPE: 2070, KEYCODE_SPACE: 2050, KEYCODE_TAB: 2049 };
const stubs = {
  '@kit.ArkUI': {},
  '@kit.InputKit': { KeyCode: KEYCODES, IntentionCode: { INTENTION_UNKNOWN: -1 } },
  '@kit.AbilityKit': {},
};
const load = Module._load;
Module._load = function (request, parent, isMain) {
  if (Object.prototype.hasOwnProperty.call(stubs, request)) return stubs[request];
  return load.call(this, request, parent, isMain);
};
globalThis.KeyType = { Down: 0, Up: 1 };
globalThis.KeySource = { Keyboard: 4 };
globalThis.EventQueryType = { ON_CLICK: 0 };

const { UiInspector } = require(path.join(build, 'uifallback/UiInspector.js'));
const { ArkUiPlatform, MCP_DECLARED_PROPERTY } = require(path.join(build, 'uifallback/ArkUiElements.js'));
const { UIContextDriver } = require(path.join(build, 'uifallback/ArkUiDriver.js'));
const { HarmonyUiFallback } = require(path.join(build, 'uifallback/HarmonyUiFallback.js'));
const { UiOutlineFormat } = require(path.join(build, 'uifallback/UiOutlineFormat.js'));
const { AppMcp } = require(path.join(build, 'AppMcp.js'));

const RECT = '[0.00, 0.00],[200.00,40.00]';

/** 假应用：状态 → 检查器树 JSON；动作改状态。 */
class FakeApp {
  constructor() {
    this.agree = false;
    this.dialog = false;
    this.focused = '';
    this.removed = new Set();
    this.log = [];
    this.declared = new Map([[3, 'cart.clear'], [20, 'cart.add']]);
    this.clickable = new Set([12]);
  }
  node(id, type, attrs, children = [], rect = RECT) {
    if (attrs.id !== undefined && this.focused === attrs.id) attrs = { ...attrs, focused: true };
    return { $type: type, $ID: id, $rect: rect, $attrs: attrs, $children: children };
  }
  tree() {
    const page = [
      this.node(2, 'Text', { content: '购物车' }),
      this.node(3, 'Button', { id: 'clear', label: '清空' }),
      this.node(4, 'Button', { id: 'pay', label: '结算', enabled: !this.agree ? 'false' : 'true' }),
      this.node(5, 'Toggle', { id: 'agree', type: 'ToggleType.Checkbox', isOn: String(this.agree), accessibilityText: '同意条款' }),
      this.node(6, 'TextInput', { id: 'note', placeholder: '备注', text: '尽快 \n 发货', focusable: true }),
      this.node(7, 'TextInput', { id: 'pwd', placeholder: '支付密码', text: 'secret', type: 'InputType.Password', focusable: true }),
      this.node(12, 'Row', {}, [this.node(13, 'Text', { content: '优惠券' })]),
      this.node(14, 'Button', { label: '无编号' }),
      this.node(20, 'Row', {}, [this.node(21, 'Button', { id: 'add', label: '加入' })]),
      this.node(22, 'Button', { id: 'ghost', label: '隐藏', visibility: 'Visibility.Hidden' }),
      this.node(30, 'List', {}, [
        this.node(31, 'Button', { id: 'item1', label: '商品一' }, [], '[0.00, 500.00],[100.00,540.00]'),
        this.node(32, 'Button', { id: 'item2', label: '商品二' }, [], '[0.00, 700.00],[100.00,740.00]'),
      ], '[0.00, 500.00],[100.00,600.00]'),
      this.node(40, 'Select', { id: 'size', value: '中杯' }),
    ].filter((n) => !this.removed.has(n.$ID));
    const children = [this.node(1, 'Column', {}, page, '[0.00, 0.00],[1000.00,1000.00]')];
    if (this.dialog) {
      children.push(this.node(50, 'Dialog', { title: '确认支付' }, [
        this.node(51, 'Button', { id: 'ok', label: '确定' }),
        this.node(52, 'Button', { id: 'cancel', label: '取消' }),
      ]));
    }
    return { $type: 'root', width: '1000.000000', height: '1000.000000', $children: children };
  }
  click(id) {
    this.log.push(['click', id]);
    if (id === 'agree') this.agree = !this.agree;
    else if (id === 'pay' && this.agree) this.dialog = true;
    else if (id === 'cancel' || id === 'ok') this.dialog = false;
    return true;
  }
}

class FakeDriver {
  constructor(app, title = '示例商城') {
    this.app = app;
    this.title = title;
  }
  inspectorTree() {
    return JSON.stringify(this.app.tree());
  }
  customProperty(id, name) {
    return name === MCP_DECLARED_PROPERTY ? this.app.declared.get(id) : undefined;
  }
  hasClickHandler(id) {
    return this.app.clickable.has(id);
  }
  clickById(id) {
    return this.app.click(id);
  }
  requestFocus(id) {
    this.app.log.push(['focus', id]);
    this.app.focused = id;
    return true;
  }
  dispatchKey(uniqueId, key) {
    this.app.log.push(['key', uniqueId, key]);
    if (key === 'Escape' && uniqueId === 50) {
      this.app.dialog = false;
      return true;
    }
    return key === 'Enter' && uniqueId === 6;
  }
}

function setup(maxItems = 60) {
  const app = new FakeApp();
  const platform = new ArkUiPlatform([new FakeDriver(app)], 0);
  return { app, ui: new UiInspector(platform, 'ui', maxItems) };
}

function refOf(outline, name) {
  const item = outline.items.find((i) => i.name === name);
  assert.ok(item, `大纲中没有「${name}」：\n${outline.text}`);
  return item.ref;
}

async function rejects(promiseOrFn, check) {
  let error;
  try {
    const r = typeof promiseOrFn === 'function' ? promiseOrFn() : promiseOrFn;
    await r;
  } catch (e) {
    error = e;
  }
  assert.ok(error, '应当失败');
  assert.equal(error.kind, check.kind ?? 'INVALID_INPUT');
  if (check.reason !== undefined) assert.equal(error.details?.reason, check.reason);
  if (check.message !== undefined) assert.match(error.message, check.message);
  return error;
}

test('大纲：角色、状态、值、密码掩码、已声明、隐藏 / 裁剪不列出、分组路径', () => {
  const { ui } = setup();
  const o = ui.outline(undefined, undefined, undefined);
  const lines = o.text.split('\n');
  assert.equal(lines[0], '» e1 窗口「示例商城」');
  assert.ok(lines.includes('  e2 按钮「清空」 [已声明：cart.clear]'), o.text);
  assert.ok(lines.includes('  e3 按钮「结算」 disabled'), o.text);
  assert.ok(lines.includes('  e4 复选框「同意条款」 unchecked'), o.text);
  assert.ok(lines.includes('  e5 输入框「备注」= "尽快 发货"'), o.text);
  assert.ok(lines.includes('  e6 密码框「支付密码」= "••••"'), o.text);
  assert.ok(!o.text.includes('secret'));
  // 绑定 onClick 的 Row 以子树文本为名；外层 Row 的声明给子树中唯一的控件。
  assert.ok(o.text.includes('按钮「优惠券」'), o.text);
  assert.equal(o.items.find((i) => i.name === '加入').declared, 'cart.add');
  assert.ok(!o.text.includes('隐藏'), '不可见的组件不列出');
  assert.ok(o.text.includes('商品一') && !o.text.includes('商品二'), '滚出列表视口的组件不列出');
  assert.ok(o.text.includes('下拉框 = "中杯"') || o.text.includes('下拉框= "中杯"'), o.text);
  assert.equal(o.items.find((i) => i.name === '商品一').group, '窗口「示例商城」 › 列表');
  assert.equal(o.hint, '标注 [已声明：…] 的元素已有对应工具，请优先直接调用该工具');
  assert.equal(o.total, o.items.length);
  assert.equal(o.remaining, undefined);
  // 引用稳定：再次大纲同一组件引用不变。
  assert.deepEqual(ui.outline(undefined, undefined, undefined).items.map((i) => i.ref), o.items.map((i) => i.ref));
});

test('大纲：limit、query、within 与无匹配', () => {
  const { ui } = setup();
  const all = ui.outline(undefined, undefined, undefined);
  const limited = ui.outline(undefined, undefined, 2);
  assert.equal(limited.items.length, 2);
  assert.equal(limited.remaining, all.total - 2);
  assert.match(limited.text, new RegExp(`…另有 ${all.total - 2} 个元素未列出，可用 query 或 within 缩小范围$`));
  // limit 上下限（1–500）
  assert.equal(ui.outline(undefined, undefined, 0).items.length, 1);
  const q = ui.outline('清空 按钮', undefined, undefined);
  assert.deepEqual(q.items.map((i) => i.name), ['清空']);
  assert.equal(ui.outline('不存在', undefined, undefined).text, '（没有与「不存在」匹配的可交互元素）');
  const listRef = all.text.split('\n').find((l) => l.includes('列表')).trim().split(' ')[1];
  const within = ui.outline(undefined, listRef, undefined);
  assert.deepEqual(within.items.map((i) => i.name), ['商品一']);
});

test('点击：按组件 id 派发，返回变化摘要；禁用 / 无 id / 失效 / 格式错误', async () => {
  const { app, ui } = setup();
  const o = ui.outline(undefined, undefined, undefined);
  await rejects(ui.click(refOf(o, '结算')), { reason: 'TOOL_DISABLED', message: /已禁用/ });
  const r = await ui.click(refOf(o, '同意条款'));
  assert.deepEqual(app.log, [['click', 'agree']]);
  assert.ok(r.changes.includes(`${refOf(o, '同意条款')} 同意条款 变为 checked`), r.changes.join('\n'));
  assert.ok(r.changes.includes(`${refOf(o, '结算')} 结算 不再 disabled`), r.changes.join('\n'));
  await rejects(ui.click(refOf(o, '无编号')), { reason: 'unsupported', message: /\.id\(\)/ });
  const declared = await ui.click(refOf(o, '清空'));
  assert.equal(declared.hint, '该元素已声明为工具 cart.clear，下次可直接调用');
  app.removed.add(3);
  const e = await rejects(ui.click(refOf(o, '清空')), {});
  assert.equal(e.message, `引用 ${refOf(o, '清空')} 已失效，请重新调用 ui.outline`);
  assert.equal(e.details.ref, refOf(o, '清空'));
  await rejects(ui.click('e999'), { message: /已失效/ });
});

test('对话框遮挡窗口；被遮挡的控件按不可见处理；Escape 关闭对话框', async () => {
  const { app, ui } = setup();
  let o = ui.outline(undefined, undefined, undefined);
  await ui.click(refOf(o, '同意条款'));
  const r = await ui.click(refOf(o, '结算'));
  const dialogLine = r.changes.find((c) => c.startsWith('新增对话框「确认支付」'));
  assert.ok(dialogLine, r.changes.join('\n'));
  assert.match(dialogLine, /，含 2 个可交互元素（可用 ui\.outline\(\{ within: "e\d+" \}\) 查看）$/);
  assert.ok(r.changes.some((c) => c.startsWith('窗口「示例商城」(e1) 已消失（含 ')), r.changes.join('\n'));
  const d = ui.outline(undefined, undefined, undefined);
  assert.deepEqual(d.items.map((i) => i.name), ['确定', '取消']);
  await rejects(ui.click(refOf(o, '清空')), { reason: 'hidden', message: /当前不可见/ });
  const esc = await ui.press(undefined, 'Escape');
  assert.equal(app.dialog, false);
  assert.ok(esc.changes.some((c) => c.includes('已消失')), esc.changes.join('\n'));
  o = ui.outline(undefined, undefined, undefined);
  assert.ok(o.text.includes('清空'), '对话框关闭后原引用恢复可用');
});

test('填写：复选框按布尔值；文本 / 下拉框不支持；密码拒绝；类型不符', async () => {
  const { app, ui } = setup();
  const o = ui.outline(undefined, undefined, undefined);
  const agree = refOf(o, '同意条款');
  const same = await ui.fill(agree, false);
  assert.deepEqual(same.changes, []);
  assert.deepEqual(app.log, []);
  await ui.fill(agree, true);
  assert.equal(app.agree, true);
  await rejects(ui.fill(agree, 'yes'), { message: /需要 true \/ false/ });
  await rejects(ui.fill(refOf(o, '支付密码'), 'x'), { reason: 'secure', message: /是密码类控件/ });
  await rejects(ui.fill(refOf(o, '备注'), '你好'), { reason: 'unsupported', message: /写入输入框文本/ });
  const size = o.items.find((i) => i.role === 'combobox').ref;
  await rejects(ui.fill(size, '大杯'), { reason: 'unsupported', message: /下拉/ });
  await rejects(ui.fill(refOf(o, '清空'), 'x'), { reason: 'unsupported', message: /只能填写/ });
});

test('按键：Tab 按树序移动焦点；Enter 交给焦点输入框；不支持的键；焦点在密码框时拒绝', async () => {
  const { app, ui } = setup();
  const o = ui.outline(undefined, undefined, undefined);
  await ui.press(undefined, 'Tab');
  assert.deepEqual(app.log.at(-1), ['focus', 'note']);
  await ui.press(undefined, 'Tab');
  assert.deepEqual(app.log.at(-1), ['focus', 'pwd']);
  app.focused = 'note';
  await ui.press(undefined, 'Shift+Tab');
  assert.deepEqual(app.log.at(-1), ['focus', 'pwd'], '从第一个可获焦组件反向循环到最后一个');
  app.focused = 'note';
  const enter = await ui.press(undefined, 'Enter');
  assert.deepEqual(app.log.at(-1), ['key', 6, 'Enter']);
  assert.equal(enter.hint, undefined);
  // Space 在按钮上：未被消费时激活（点击）。
  await ui.press(refOf(o, '清空'), 'Space');
  assert.deepEqual(app.log.at(-1), ['click', 'clear']);
  await rejects(ui.press(undefined, 'F5'), { message: /支持 Enter、Escape、Tab、Shift\+Tab、Space/ });
  app.focused = 'pwd';
  await rejects(ui.press(undefined, 'Enter'), { reason: 'secure' });
  await rejects(ui.press(refOf(o, '支付密码'), 'Enter'), { reason: 'secure' });
});

test('滚动：可见控件无 direction 时无变化；ArkUI 不支持滚动；失效引用', async () => {
  const { app, ui } = setup();
  const o = ui.outline(undefined, undefined, undefined);
  await rejects(ui.scroll(refOf(o, '商品一'), 'down'), { reason: 'unsupported', message: /Scroller/ });
  await rejects(ui.scroll(refOf(o, '商品一'), 'sideways'), { message: /direction/ });
  app.removed.add(30);
  await rejects(ui.scroll(refOf(o, '商品一'), undefined), { message: /已失效/ });
});

test('读取：折叠空白、密码掩码、截断、按引用', () => {
  const { ui } = setup();
  const all = ui.read(undefined, undefined);
  assert.equal(all.ref, 'root');
  assert.ok(all.text.startsWith('购物车 清空 结算 尽快 发货 ••••'), all.text);
  assert.ok(!all.text.includes('secret') && !all.text.includes('商品二') && !all.text.includes('隐藏'));
  const cut = ui.read(undefined, 5);
  assert.equal(cut.truncated, true);
  // 保留前 4 个码点、去掉尾部空白再加 …（spec/ui-fallback.md 4.2）。
  assert.equal(cut.text, '购物车…');
  assert.ok(cut.text.endsWith('…'));
  const o = ui.outline(undefined, undefined, undefined);
  assert.equal(ui.read(refOf(o, '优惠券'), undefined).text, '优惠券');
});

test('格式：截断按码点、diff 超过 15 条截断', () => {
  assert.equal(UiOutlineFormat.truncate('一二三四五', 3), '一二…');
  assert.equal(UiOutlineFormat.truncate('ab  cd', 4), 'ab…');
  const mk = (k) => ({ kind: 'item', key: k, role: 'button', label: '按钮', ref: k, name: k, states: [], required: false,
    depth: 0, chain: [], containers: [], get described() { return `按钮「${k}」`; } });
  const after = Array.from({ length: 17 }, (_, i) => mk(`e${i + 1}`));
  const d = UiOutlineFormat.diff([], after, 'ui');
  assert.equal(d.length, 16);
  assert.equal(d[15], '…另有 2 项变化，请调用 ui.outline 查看');
});

test('HarmonyUiFallback：surface view、按前后台启用、禁用时 TOOL_DISABLED、经客户端调用', async () => {
  const { factory, clients } = fakeFactory();
  const mcp = new AppMcp({ appId: 'shop', appName: '商城', logger: new MemoryLogger() }, factory);
  const client = clients[0];
  const app = new FakeApp();
  const callbacks = [];
  const appContext = {
    on: (event, cb) => callbacks.push(cb),
    off: (event, cb) => callbacks.splice(callbacks.indexOf(cb), 1),
  };
  const fallback = HarmonyUiFallback.enable(mcp, [new FakeDriver(app)], { settleDelayMs: 0, applicationContext: appContext });
  const names = [...client.tools.keys()].sort();
  assert.deepEqual(names, ['ui.click', 'ui.fill', 'ui.outline', 'ui.press', 'ui.read', 'ui.scroll']);
  const spec = client.tools.get('ui.outline').spec;
  assert.equal(spec.surface, 'view');
  assert.equal(spec.annotations.readOnlyHint, true);
  assert.equal(client.tools.get('ui.click').spec.annotations.readOnlyHint, false);
  assert.equal(JSON.parse(client.tools.get('ui.click').spec.inputSchemaJson).additionalProperties, false);
  assert.equal(fallback.enabled, true);
  assert.equal(client.tools.get('ui.outline').spec.enabled, true);

  const ok = await client.invoke('ui.outline', { query: '清空' }).done;
  assert.equal(ok.ok, true, JSON.stringify(ok));
  assert.match(JSON.parse(ok.dataJson).text, /按钮「清空」/);
  const bad = await client.invoke('ui.click', { ref: 'x1' }).done;
  assert.equal(bad.kind, 'INVALID_INPUT');

  callbacks[0].onApplicationBackground();
  assert.equal(fallback.enabled, false);
  assert.equal(client.tools.get('ui.outline').spec.enabled, false);
  const off = await client.invoke('ui.outline', {}).done;
  assert.equal(off.kind, 'TOOL_DISABLED');
  callbacks[0].onApplicationForeground();
  assert.equal(fallback.enabled, true);

  fallback.close();
  assert.equal(callbacks.length, 0);
  assert.deepEqual(client.scopeDisposals, ['ui-fallback']);
  mcp.dispose();
});

test('UIContextDriver：经 UIContext / FrameNode / sendEventByKey 派发', () => {
  const calls = [];
  const node = {
    getCustomProperty: (name) => (name === MCP_DECLARED_PROPERTY ? 'cart.clear' : undefined),
    getInteractionEventBindingInfo: (t) => (t === 0 ? { baseEventRegistered: true } : undefined),
  };
  const ctx = {
    getFilteredInspectorTree: () => '{"$type":"root"}',
    getFrameNodeByUniqueId: (id) => (id === 3 ? node : null),
    getFocusController: () => ({ requestFocus: (key) => { if (key === 'nope') throw new Error('150003'); calls.push(['focus', key]); } }),
    dispatchKeyEvent: (id, e) => { calls.push(['key', id, e.type, e.keyCode]); return e.type === 0; },
  };
  globalThis.sendEventByKey = (id, action, params) => { calls.push(['event', id, action, params]); return true; };
  const d = new UIContextDriver(ctx, '主窗口');
  assert.equal(d.inspectorTree(), '{"$type":"root"}');
  assert.equal(d.customProperty(3, MCP_DECLARED_PROPERTY), 'cart.clear');
  assert.equal(d.customProperty(4, MCP_DECLARED_PROPERTY), undefined);
  assert.equal(d.hasClickHandler(3), true);
  assert.equal(d.hasClickHandler(4), false);
  assert.equal(d.clickById('clear'), true);
  assert.equal(d.requestFocus('note'), true);
  assert.equal(d.requestFocus('nope'), false);
  assert.equal(d.dispatchKey(6, 'Enter'), true);
  assert.deepEqual(calls, [
    ['event', 'clear', 10, ''], ['focus', 'note'], ['key', 6, 0, 2054], ['key', 6, 1, 2054],
  ]);
  delete globalThis.sendEventByKey;
});
