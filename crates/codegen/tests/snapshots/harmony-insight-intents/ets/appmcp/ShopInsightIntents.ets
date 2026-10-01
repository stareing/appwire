// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：示例商城（shop）
// 总览：演示用购物商城，可管理待办、浏览商品、操作购物车并结算
//
// 意图执行器（ets/insightintents/*.ets）调用 ShopToolHandlers 的入口。
// 在 AbilityStage.onCreate 中设置 `ShopIntentRuntime.handlers = <实现 ShopToolHandlers 的对象>`
// （意图在后台执行时系统经 Call 调用拉起 UIAbility，不经过页面；可与 MCP 共用同一个实现）。

import { insightIntent } from '@kit.AbilityKit';
import { ToolCallError, ToolResult } from "@app-mcp/harmony";
import { ShopToolHandlers } from './ShopTools';

/** 意图执行结果码（IntentResult.code）。失败时 result 为 {"kind","message"} 的 JSON 文本。 */
export enum ShopIntentCode {
  OK = 0,
  /** 缺少必填参数。 */
  INVALID_INPUT = 1,
  /** App 未设置 ShopIntentRuntime.handlers。 */
  HANDLERS_NOT_SET = 2,
  /** handler 抛出异常或返回值无法序列化。 */
  FAILED = 3,
}

/** 失败结果的内容。 */
class ShopIntentFailure {
  kind: string;
  message: string;

  constructor(kind: string, message: string) {
    this.kind = kind;
    this.message = message;
  }
}

export class ShopIntentRuntime {
  /** 由 App 在启动时设置；未设置时意图返回 ShopIntentCode.HANDLERS_NOT_SET。 */
  static handlers: ShopToolHandlers | undefined = undefined;

  /** 调用 handler 并把结果转换为 IntentResult。 */
  static run(call: (handlers: ShopToolHandlers) => Object | null | Promise<Object | null>): Promise<insightIntent.IntentResult<string>> {
    const handlers = ShopIntentRuntime.handlers;
    if (handlers === undefined) {
      return Promise.resolve(ShopIntentRuntime.failure(ShopIntentCode.HANDLERS_NOT_SET, 'HANDLER_ERROR', '未设置 ShopIntentRuntime.handlers'));
    }
    let pending: Promise<Object | null>;
    try {
      pending = Promise.resolve(call(handlers));
    } catch (error) {
      return Promise.resolve(ShopIntentRuntime.fromError(error as Object));
    }
    return pending.then((value: Object | null): insightIntent.IntentResult<string> => ShopIntentRuntime.success(value),
      (error: Object): insightIntent.IntentResult<string> => ShopIntentRuntime.fromError(error));
  }

  /** 缺少必填参数。 */
  static missing(field: string): Promise<insightIntent.IntentResult<string>> {
    return Promise.resolve(ShopIntentRuntime.failure(ShopIntentCode.INVALID_INPUT, 'INVALID_INPUT', `缺少参数 ${field}`));
  }

  private static success(value: Object | null): insightIntent.IntentResult<string> {
    const data: Object | null = value instanceof ToolResult ? (value as ToolResult<Object | null>).data : value;
    let text: string | undefined;
    try {
      text = JSON.stringify(data);
    } catch (error) {
      return ShopIntentRuntime.failure(ShopIntentCode.FAILED, 'HANDLER_ERROR', `返回值无法序列化为 JSON：${(error as Error).message}`);
    }
    const result: insightIntent.IntentResult<string> = { code: 0, result: text ?? 'null' };
    return result;
  }

  private static fromError(error: Object): insightIntent.IntentResult<string> {
    if (error instanceof ToolCallError) {
      return ShopIntentRuntime.failure(ShopIntentCode.FAILED, error.kind, error.message);
    }
    const message = error instanceof Error ? error.message : String(error);
    return ShopIntentRuntime.failure(ShopIntentCode.FAILED, 'HANDLER_ERROR', message);
  }

  private static failure(code: ShopIntentCode, kind: string, message: string): insightIntent.IntentResult<string> {
    const result: insightIntent.IntentResult<string> = { code: code, result: JSON.stringify(new ShopIntentFailure(kind, message)) };
    return result;
  }
}
