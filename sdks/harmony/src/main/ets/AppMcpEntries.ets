/**
 * AppMcp 内部实现：工具 / 资源条目与注册入口基类、Scope（从 AppMcp.ets 拆出，不属于公开 API）。
 */

import type {
  Call, Read, Resource, ResourceSpecInit, Scope as NativeScope, Tool, ToolSpecInit,
} from 'libapp_mcp_harmony.so';
import { Failure, ToolCallError, nativeErrorCode, toFailure } from './Errors';
import {
  Activation, CancelReason, Logger, Registrar, ResourceDefinition, ResourceHandle, Risk, Scope, ToolAnnotations,
  ToolChanges, ToolDefinition, ToolHandle, ToolResult, ToolSurface,
} from './Types';
import {
  CallContext, Child, NativeRegistrar, callResultInit, contentAnnotationsInit, errorText, hasResultExtras, patchField,
  stringifyJson, submitFailure, toolAnnotationsInit,
} from './AppMcpSupport';

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

export class ToolEntry<I, O> implements ToolHandle, Child {
  readonly name: string;
  private def: ToolDefinition<I, O>;
  private readonly native: Tool | null;
  private readonly owner: Set<Child> | null;
  private readonly logger: Logger;
  private disposed: boolean = false;

  constructor(registrar: NativeRegistrar | null, owner: Set<Child> | null, name: string,
    definition: ToolDefinition<I, O>, logger: Logger) {
    this.name = name;
    this.def = definition;
    this.owner = owner;
    this.logger = logger;
    if (registrar === null) {
      this.native = null;
      return;
    }
    // 同步注册：重名、非法名称、非法 schema 直接抛给调用方。
    this.native = registrar.registerTool(this.spec(), (call: Call): void => this.onCall(call));
    if (owner !== null) {
      owner.add(this);
    }
  }

  private spec(): ToolSpecInit {
    const d = this.def;
    const spec: ToolSpecInit = { name: this.name, description: d.description, enabled: d.enabled ?? true };
    spec.inputSchemaJson = d.inputSchema;
    spec.risk = d.risk;
    spec.activation = d.activation;
    spec.title = d.title;
    if (d.annotations !== undefined) {
      spec.annotations = toolAnnotationsInit(d.annotations);
    }
    spec.outputSchemaJson = d.outputSchema;
    spec.surface = d.surface;
    spec.page = d.page;
    spec.backgroundTool = d.backgroundTool;
    return spec;
  }

  update(changes: ToolChanges): void {
    if (this.disposed) {
      return;
    }
    const d = this.def;
    const next: ToolDefinition<I, O> = {
      description: changes.description ?? d.description,
      title: patchField<string>(changes.title, d.title),
      inputSchema: patchField<string>(changes.inputSchema, d.inputSchema),
      risk: patchField<Risk>(changes.risk, d.risk),
      annotations: patchField<ToolAnnotations>(changes.annotations, d.annotations),
      outputSchema: patchField<string>(changes.outputSchema, d.outputSchema),
      activation: patchField<Activation>(changes.activation, d.activation),
      enabled: patchField<boolean>(changes.enabled, d.enabled),
      surface: patchField<ToolSurface>(changes.surface, d.surface),
      page: patchField<string>(changes.page, d.page),
      backgroundTool: patchField<string>(changes.backgroundTool, d.backgroundTool),
      handler: d.handler,
    };
    this.def = next;
    if (this.native !== null) {
      // 定义始终完整：用 updateWith 一并替换注解与输出 schema
      this.native.updateWith(this.spec());
    }
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    if (this.owner !== null) {
      this.owner.delete(this);
    }
    if (this.native !== null) {
      this.native.dispose();
    }
  }

  detach(): void {
    this.disposed = true;
  }

  private onCall(call: Call): void {
    const context = new CallContext(call, this.logger);
    let settled = false;
    const finish = (action: () => void): void => {
      if (settled) {
        return;
      }
      settled = true;
      try {
        action();
      } catch (error) {
        // 已取消 / 已完成：原生侧拒绝重复完成，属正常情况。
        if (nativeErrorCode(error as Object) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交工具 ${this.name} 的结果失败：${errorText(error as Object)}`);
        }
      }
    };
    const fail = (failure: Failure): void => {
      finish(() => submitFailure(failure, (k: string, m: string): void => call.fail(k, m),
        (k: string, m: string, d: string): void => call.failWithDetails(k, m, d)));
    };

    try {
      call.setCancelListener((reason: string): void => {
        settled = true;
        context.cancel(reason as CancelReason);
      });
    } catch (error) {
      this.logger.warn(`[app-mcp] 设置取消监听失败：${errorText(error as Object)}`);
    }

    let input: I;
    try {
      const text = call.argumentsJson;
      input = (text !== '' ? JSON.parse(text) : JSON.parse('{}')) as I;
    } catch (error) {
      fail(toFailure(new ToolCallError('INVALID_INPUT', '参数不是合法的 JSON')));
      return;
    }

    const handler = this.def.handler;
    let pending: Promise<O | ToolResult<O>>;
    try {
      pending = Promise.resolve(handler(input, context));
    } catch (error) {
      fail(toFailure(error as Object));
      return;
    }
    pending.then((result: O | ToolResult<O>): void => {
      if (context.isCancelled()) {
        return;
      }
      const structured: ToolResult<O> | null = result instanceof ToolResult ? result : null;
      const data: O = structured !== null ? structured.data : result as O;
      const hints: string[] = structured !== null ? structured.stateHints : [];
      let json: string;
      try {
        json = stringifyJson(data as Object);
      } catch (error) {
        fail(toFailure(new ToolCallError('HANDLER_ERROR', `返回值无法序列化为 JSON：${errorText(error as Object)}`)));
        return;
      }
      if (structured === null || !hasResultExtras(structured)) {
        finish(() => call.complete(json, hints));
        return;
      }
      try {
        call.completeWith(callResultInit(json, structured));
        finish(() => {});
      } catch (error) {
        if (nativeErrorCode(error as Object) === 'ALREADY_COMPLETED') {
          finish(() => {});
          return;
        }
        // 取值不合法（如未知的 status / audience）：失败完成，不让调用挂起到超时
        fail(toFailure(new ToolCallError('HANDLER_ERROR', `返回的结构化结果无效：${errorText(error as Object)}`)));
      }
    }, (error: Object): void => {
      if (context.isCancelled()) {
        return;
      }
      fail(toFailure(error));
    });
  }
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

export class ResourceEntry<T> implements ResourceHandle, Child {
  readonly name: string;
  private readonly def: ResourceDefinition<T>;
  private readonly native: Resource | null;
  private readonly owner: Set<Child> | null;
  private readonly logger: Logger;
  private disposed: boolean = false;

  constructor(registrar: NativeRegistrar | null, owner: Set<Child> | null, name: string,
    definition: ResourceDefinition<T>, logger: Logger) {
    this.name = name;
    this.def = definition;
    this.owner = owner;
    this.logger = logger;
    if (registrar === null) {
      this.native = null;
      return;
    }
    const spec: ResourceSpecInit = {
      name: name,
      description: definition.description,
      // 未声明时不发送（toolsHash 输入与其他 SDK 一致；Host 按 application/json 处理）
      mimeType: definition.mimeType,
      realtime: definition.realtime,
    };
    if (definition.annotations !== undefined) {
      spec.annotations = contentAnnotationsInit(definition.annotations);
    }
    this.native = registrar.registerResource(spec, (read: Read): void => this.onRead(read));
    if (owner !== null) {
      owner.add(this);
    }
  }

  notifyChanged(): void {
    if (this.disposed || this.native === null) {
      return;
    }
    try {
      this.native.notifyChanged();
    } catch (error) {
      this.logger.warn(`[app-mcp] 资源 ${this.name} 变更通知失败：${errorText(error as Object)}`);
    }
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    if (this.owner !== null) {
      this.owner.delete(this);
    }
    if (this.native !== null) {
      this.native.dispose();
    }
  }

  detach(): void {
    this.disposed = true;
  }

  private failRead(read: Read, failure: Failure): void {
    submitFailure(failure, (k: string, m: string): void => read.fail(k, m),
      (k: string, m: string, d: string): void => read.failWithDetails(k, m, d));
  }

  private onRead(read: Read): void {
    const submit = (action: () => void): void => {
      try {
        action();
      } catch (error) {
        if (nativeErrorCode(error as Object) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交资源 ${this.name} 的内容失败：${errorText(error as Object)}`);
        }
      }
    };
    let pending: Promise<T>;
    try {
      pending = Promise.resolve(this.def.read());
    } catch (error) {
      const failure = toFailure(error as Object);
      submit(() => this.failRead(read, failure));
      return;
    }
    pending.then((value: T): void => {
      let json: string;
      try {
        json = stringifyJson(value as Object);
      } catch (error) {
        submit(() => read.fail('HANDLER_ERROR', `资源内容无法序列化为 JSON：${errorText(error as Object)}`));
        return;
      }
      submit(() => read.complete(json));
    }, (error: Object): void => {
      const failure = toFailure(error);
      submit(() => this.failRead(read, failure));
    });
  }
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

export abstract class RegistrarBase implements Registrar {
  protected readonly children: Set<Child> = new Set<Child>();
  protected readonly logger: Logger;

  constructor(logger: Logger) {
    this.logger = logger;
  }

  /** 原生注册入口；为 null 时（未启用 / 已注销）注册为空操作。 */
  protected abstract registrar(): NativeRegistrar | null;

  tool<I, O>(name: string, definition: ToolDefinition<I, O>): ToolHandle {
    return new ToolEntry<I, O>(this.registrar(), this.children, name, definition, this.logger);
  }

  resource<T>(name: string, definition: ResourceDefinition<T>): ResourceHandle {
    return new ResourceEntry<T>(this.registrar(), this.children, name, definition, this.logger);
  }

  scope(name: string): Scope {
    const registrar = this.registrar();
    const scope = new ScopeEntry(registrar !== null ? registrar.createScope(name) : null, this.children, name,
      this.logger);
    if (registrar !== null) {
      this.children.add(scope);
    }
    return scope;
  }

  /** 本地标记全部子项已注销（原生侧已由父级递归注销）。 */
  protected detachChildren(): void {
    this.children.forEach((child: Child) => child.detach());
    this.children.clear();
  }
}

export class ScopeEntry extends RegistrarBase implements Scope, Child {
  readonly name: string;
  private readonly native: NativeScope | null;
  private readonly owner: Set<Child>;
  private disposed: boolean = false;

  constructor(native: NativeScope | null, owner: Set<Child>, name: string, logger: Logger) {
    super(logger);
    this.native = native;
    this.owner = owner;
    this.name = name;
  }

  protected registrar(): NativeRegistrar | null {
    return this.disposed ? null : this.native;
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    this.owner.delete(this);
    this.detachChildren();
    if (this.native !== null) {
      this.native.dispose();
    }
  }

  detach(): void {
    this.disposed = true;
    this.detachChildren();
  }
}
