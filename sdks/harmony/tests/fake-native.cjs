// 测试支持：libapp_mcp_harmony.so 的假实现（只记录调用，行为与 bindings/node 的约定一致：
// 重名抛 DUPLICATE_NAME、重复完成抛 ALREADY_COMPLETED）。
'use strict';

function nativeError(code, message) {
  const e = new Error(message);
  e.code = code;
  return e;
}

class FakeHold {
  constructor(log) {
    this.log = log;
  }
  release() {
    this.log.push('release');
  }
}

class FakeCall {
  constructor(toolName, argumentsJson) {
    this.callId = `c-${Math.random().toString(16).slice(2)}`;
    this.toolName = toolName;
    this.argumentsJson = argumentsJson;
    this.result = undefined;
    this.cancelListener = undefined;
    this.holdLog = [];
    this.done = new Promise((resolve) => {
      this.resolve = resolve;
    });
  }
  isCancelled() {
    return false;
  }
  setCancelListener(listener) {
    this.cancelListener = listener;
  }
  settle(result) {
    if (this.result !== undefined) throw nativeError('ALREADY_COMPLETED', 'call or read already completed or cancelled');
    this.result = result;
    this.resolve(result);
  }
  complete(dataJson, stateHints) {
    this.settle({ ok: true, dataJson, stateHints });
  }
  fail(kind, message) {
    this.settle({ ok: false, kind, message });
  }
  failWithDetails(kind, message, detailsJson) {
    this.settle({ ok: false, kind, message, detailsJson });
  }
  hold() {
    return new FakeHold(this.holdLog);
  }
  /** 测试用：模拟原生侧取消（之后完成会抛 ALREADY_COMPLETED）。 */
  cancel(reason) {
    this.result = { cancelled: reason };
    this.resolve(this.result);
    if (this.cancelListener) this.cancelListener(reason);
  }
}

class FakeRead {
  constructor(resourceName) {
    this.resourceName = resourceName;
    this.done = new Promise((resolve) => {
      this.resolve = resolve;
    });
  }
  complete(contentsJson) {
    this.resolve({ ok: true, contentsJson });
  }
  fail(kind, message) {
    this.resolve({ ok: false, kind, message });
  }
}

class FakeRegistrar {
  constructor(root) {
    this.root = root;
    this.disposed = false;
  }
  registerTool(spec, handler) {
    if (this.root.tools.has(spec.name)) throw nativeError('DUPLICATE_NAME', `duplicate ${spec.name}`);
    const tool = {
      name: spec.name,
      spec,
      handler,
      disposed: false,
      update: (next) => {
        tool.spec = next;
      },
      setEnabled: () => {},
      dispose: () => {
        tool.disposed = true;
        this.root.tools.delete(spec.name);
      },
    };
    this.root.tools.set(spec.name, tool);
    return tool;
  }
  registerResource(spec, reader) {
    const res = {
      name: spec.name,
      spec,
      reader,
      changed: 0,
      notifyChanged: () => {
        res.changed++;
      },
      dispose: () => {
        this.root.resources.delete(spec.name);
      },
    };
    this.root.resources.set(spec.name, res);
    return res;
  }
  createScope(name) {
    const scope = new FakeRegistrar(this.root);
    scope.name = name;
    scope.dispose = () => {
      scope.disposed = true;
      this.root.scopeDisposals.push(name);
    };
    return scope;
  }
}

class FakeNativeClient extends FakeRegistrar {
  constructor(config, listener) {
    super(null);
    this.root = this;
    this.config = config;
    this.listener = listener;
    this.tools = new Map();
    this.resources = new Map();
    this.scopeDisposals = [];
    this.calls = [];
    this.instanceId = 'inst-1';
    this.state = { status: 'idle' };
    this.connectionId = undefined;
    this.token = config.token;
    this.started = 0;
    this.stopped = 0;
    this.wakes = [];
  }
  start() {
    this.started++;
  }
  stop() {
    this.stopped++;
  }
  handleWake(args) {
    this.calls.push(['handleWake', args]);
    return args.startsWith('app-mcp-wake:') || args.includes('://app-mcp/wake');
  }
  wake(reason) {
    this.calls.push(['wake', reason]);
    return true;
  }
  connectNow() {
    return true;
  }
  sleep(reason) {
    this.calls.push(['sleep', reason]);
    return true;
  }
  hold() {
    return new FakeHold(this.calls);
  }
  toolsHash() {
    return 'abcd';
  }
  runtimeActive() {
    return true;
  }
  setVisibility(visibility, focused) {
    this.calls.push(['setVisibility', visibility, focused]);
  }
  /** 测试用：模拟 Host 调用工具。 */
  invoke(name, args) {
    const call = new FakeCall(name, args === undefined ? '{}' : typeof args === 'string' ? args : JSON.stringify(args));
    this.tools.get(name).handler(call);
    return call;
  }
  /** 测试用：模拟 Host 读取资源。 */
  read(name) {
    const read = new FakeRead(name);
    this.resources.get(name).reader(read);
    return read;
  }
  emit(event) {
    this.listener(event);
  }
}

/** 返回 `{ factory, clients }`：factory 传给 `new AppMcp(options, factory)`。 */
function fakeFactory() {
  const clients = [];
  const factory = (config, listener) => {
    const client = new FakeNativeClient(config, listener);
    clients.push(client);
    return client;
  };
  return { factory, clients };
}

class MemoryLogger {
  constructor() {
    this.lines = [];
  }
  debug(m) {
    this.lines.push(['debug', m]);
  }
  warn(m) {
    this.lines.push(['warn', m]);
  }
  error(m) {
    this.lines.push(['error', m]);
  }
}

module.exports = { fakeFactory, MemoryLogger, FakeCall };
