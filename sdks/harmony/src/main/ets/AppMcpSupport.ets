/**
 * AppMcp 内部支撑：原生注册入口、日志、持有、调用上下文与工具 / 结果字段转换（从 AppMcp.ets 拆出，不属于公开 API）。
 */

import type {
  Call, CallResultInit, ContentAnnotationsInit, Hold, LifecycleInit, Navigate, Read, Resource, ResourceSpecInit,
  Scope as NativeScope, Tool, ToolAnnotationsInit, ToolSpecInit,
} from 'libapp_mcp_harmony.so';
import { Failure, ToolCallError, toFailure } from './Errors';
import {
  CancelReason, ContentAnnotations, HoldHandle, LifecycleOptions, Logger, ToolAnnotations, ToolContext, ToolResult,
} from './Types';

/** 原生注册入口（客户端或 scope）。 */
export interface NativeRegistrar {
  registerTool(spec: ToolSpecInit, handler: (call: Call) => void): Tool;
  registerResource(spec: ResourceSpecInit, reader: (read: Read) => void): Resource;
  createScope(name: string): NativeScope;
}

/** 父级（scope / 客户端）在原生侧批量注销后，子项只更新本地状态。 */
export interface Child {
  detach(): void;
}

export class ConsoleLogger implements Logger {
  debug(message: string): void {
  }

  warn(message: string): void {
    console.warn(message);
  }

  error(message: string): void {
    console.error(message);
  }
}

export function errorText(error: Object | null | undefined): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

/** `JSON.stringify`，`undefined` 记为 `null`。 */
export function stringifyJson(value: Object | null | undefined): string {
  if (value === undefined) {
    return 'null';
  }
  const text: string | undefined = JSON.stringify(value);
  return text === undefined ? 'null' : text;
}

/** 生命周期选项 → 原生字段。 */
export function lifecycleInit(options: LifecycleOptions): LifecycleInit {
  const init: LifecycleInit = {};
  init.mode = options.mode;
  init.idleTimeoutMs = options.idleTimeoutMs;
  init.hiddenIdleTimeoutMs = options.hiddenIdleTimeoutMs;
  init.graceMs = options.graceMs;
  init.residency = options.residency;
  if (options.wake !== undefined) {
    init.wake = { kind: options.wake.kind, target: options.wake.target, background: options.wake.background };
  }
  init.hostAbsentRetries = options.hostAbsentRetries;
  init.legacyTimers = options.legacyTimers;
  init.mergeWindowMs = options.mergeWindowMs;
  init.sleepOnBackground = options.sleepOnBackground;
  return init;
}

export class NoopHold implements HoldHandle {
  release(): void {
  }
}

/** 包装原生持有：`release` 幂等。 */
export class OnceHold implements HoldHandle {
  private readonly inner: Hold;
  private released: boolean = false;

  constructor(inner: Hold) {
    this.inner = inner;
  }

  release(): void {
    if (this.released) {
      return;
    }
    this.released = true;
    this.inner.release();
  }
}

// ---------------------------------------------------------------------------
// 调用上下文
// ---------------------------------------------------------------------------

export class CallContext implements ToolContext {
  readonly callId: string;
  readonly idempotencyKey?: string;
  private readonly call: Call;
  private cancelled: boolean = false;
  private cancelReason: CancelReason | undefined = undefined;
  private readonly listeners: ((reason: CancelReason) => void)[] = [];
  private readonly logger: Logger;

  constructor(call: Call, logger: Logger) {
    this.call = call;
    this.callId = call.callId;
    const key = call.idempotencyKey;
    if (key !== undefined && key !== null) {
      this.idempotencyKey = key;
    }
    this.logger = logger;
  }

  isCancelled(): boolean {
    return this.cancelled;
  }

  onCancel(listener: (reason: CancelReason) => void): void {
    const reason = this.cancelReason;
    if (reason !== undefined) {
      Promise.resolve().then(() => this.notify(listener, reason));
      return;
    }
    this.listeners.push(listener);
  }

  hold(): HoldHandle {
    return new OnceHold(this.call.hold());
  }

  progress(progress: number, total?: number, message?: string): void {
    if (this.cancelled) {
      return;
    }
    try {
      this.call.reportProgress(progress, total ?? null, message ?? null);
    } catch (error) {
      // 调用刚结束时原生层报 ALREADY_COMPLETED：进度只是提示，不影响结果。
      this.logger.debug(`[app-mcp] 进度未送达：${String(error)}`);
    }
  }

  /** 原生侧通知取消。 */
  cancel(reason: CancelReason): void {
    if (this.cancelled) {
      return;
    }
    this.cancelled = true;
    this.cancelReason = reason;
    const listeners = this.listeners.splice(0, this.listeners.length);
    for (const listener of listeners) {
      this.notify(listener, reason);
    }
  }

  private notify(listener: (reason: CancelReason) => void, reason: CancelReason): void {
    try {
      listener(reason);
    } catch (error) {
      this.logger.error(`[app-mcp] onCancel 监听器抛出异常：${errorText(error as Object)}`);
    }
  }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

export function toolAnnotationsInit(a: ToolAnnotations): ToolAnnotationsInit {
  const init: ToolAnnotationsInit = {};
  init.title = a.title;
  init.readOnlyHint = a.readOnlyHint;
  init.destructiveHint = a.destructiveHint;
  init.idempotentHint = a.idempotentHint;
  init.openWorldHint = a.openWorldHint;
  return init;
}

/**
 * 以失败完成调用或读取（原生 `Call` / `Read`）：带详情（如 `USER_ACTION_REQUIRED` 的 `reason` / `uri`）时用 `failWithDetails`。
 * @why 以函数传入而非接口：ArkTS 不支持结构化类型，`Call` / `Read` 不能直接当作同一接口。
 */
export function submitFailure(failure: Failure, fail: (kind: string, message: string) => void,
  failWithDetails: (kind: string, message: string, detailsJson: string) => void): void {
  const detailsJson = failure.detailsJson;
  if (detailsJson !== undefined) {
    failWithDetails(failure.kind, failure.message, detailsJson);
  } else {
    fail(failure.kind, failure.message);
  }
}

/**
 * 导航回调的失败 → 原生导航句柄：`NAVIGATION_DENIED` 拒绝；`USER_ACTION_REQUIRED`（`ToolCallError.userActionRequired`）
 * 带详情中的 `reason` / `uri`；其他按失败。
 */
export function submitNavigationFailure(navigate: Navigate, error: Object | null | undefined): void {
  const failure: Failure = toFailure(error);
  if (failure.kind === 'NAVIGATION_DENIED') {
    navigate.deny(failure.message);
    return;
  }
  if (failure.kind === 'USER_ACTION_REQUIRED' && error instanceof ToolCallError) {
    navigate.failUserAction(failure.message, detailText(error, 'reason'), detailText(error, 'uri'));
    return;
  }
  navigate.fail(failure.message);
}

/** 错误详情中的字符串字段；没有或不是字符串时为 undefined。 */
export function detailText(error: ToolCallError, key: string): string | undefined {
  const details = error.details;
  if (details === undefined) {
    return undefined;
  }
  const value: Object | undefined = details[key];
  return typeof value === 'string' ? value as string : undefined;
}

export function contentAnnotationsInit(a: ContentAnnotations): ContentAnnotationsInit {
  const init: ContentAnnotationsInit = {};
  if (a.audience !== undefined) {
    init.audience = a.audience.map((role: string): string => role);
  }
  init.priority = a.priority;
  init.lastModified = a.lastModified;
  return init;
}

/** 是否带有 data / stateHints 以外的字段（需要 `completeWith`）。 */
export function hasResultExtras<O>(r: ToolResult<O>): boolean {
  return r.status !== undefined || r.stateResource !== undefined || r.summary !== undefined ||
    r.annotations !== undefined;
}

export function callResultInit<O>(json: string, r: ToolResult<O>): CallResultInit {
  const init: CallResultInit = { dataJson: json, stateHints: r.stateHints };
  init.status = r.status;
  init.stateResource = r.stateResource;
  init.summary = r.summary;
  if (r.annotations !== undefined) {
    init.annotations = contentAnnotationsInit(r.annotations);
  }
  return init;
}

/**
 * `ToolHandle.update` 单个字段的合并规则。
 * @output `undefined`（未提供）→ 当前值；`null` → `undefined`（清除，回到缺省）；其他 → 新值。
 */
export function patchField<T>(change: T | null | undefined, current: T | undefined): T | undefined {
  if (change === undefined) {
    return current;
  }
  return change === null ? undefined : change;
}
