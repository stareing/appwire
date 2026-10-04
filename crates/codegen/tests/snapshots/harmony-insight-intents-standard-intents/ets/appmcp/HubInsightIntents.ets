// 由 app-mcp-codegen 生成（target: harmony-insight-intents），请勿手动修改。
// App：生活助手（hub）
// 简介：标准意图快照用例：六个试点动词各一个实现者，另含跳过规则
//
// 意图执行器（ets/insightintents/*.ets）调用 HubToolHandlers 的入口。
// 在 AbilityStage.onCreate 中设置 `HubIntentRuntime.handlers = <实现 HubToolHandlers 的对象>`
// （意图在后台执行时系统经 Call 调用拉起 UIAbility，不经过页面；可与 MCP 共用同一个实现）。

import { insightIntent } from '@kit.AbilityKit';
import { ToolCallError, ToolResult } from "@app-mcp/harmony";
import { HubToolHandlers } from './HubTools';

/** 意图执行结果码（IntentResult.code）。失败时 result 为 {"kind","message"} 的 JSON 文本。 */
export enum HubIntentCode {
  OK = 0,
  /** 缺少必填参数。 */
  INVALID_INPUT = 1,
  /** App 未设置 HubIntentRuntime.handlers。 */
  HANDLERS_NOT_SET = 2,
  /** handler 抛出异常或返回值无法序列化。 */
  FAILED = 3,
}

/** 失败结果的内容。 */
class HubIntentFailure {
  kind: string;
  message: string;

  constructor(kind: string, message: string) {
    this.kind = kind;
    this.message = message;
  }
}

export class HubIntentRuntime {
  /** 由 App 在启动时设置；未设置时意图返回 HubIntentCode.HANDLERS_NOT_SET。 */
  static handlers: HubToolHandlers | undefined = undefined;

  /** 调用 handler 并把结果转换为 IntentResult。 */
  static run(call: (handlers: HubToolHandlers) => Object | null | Promise<Object | null>): Promise<insightIntent.IntentResult<string>> {
    const handlers = HubIntentRuntime.handlers;
    if (handlers === undefined) {
      return Promise.resolve(HubIntentRuntime.failure(HubIntentCode.HANDLERS_NOT_SET, 'HANDLER_ERROR', '未设置 HubIntentRuntime.handlers'));
    }
    let pending: Promise<Object | null>;
    try {
      pending = Promise.resolve(call(handlers));
    } catch (error) {
      return Promise.resolve(HubIntentRuntime.fromError(error as Object));
    }
    return pending.then((value: Object | null): insightIntent.IntentResult<string> => HubIntentRuntime.success(value),
      (error: Object): insightIntent.IntentResult<string> => HubIntentRuntime.fromError(error));
  }

  /** 缺少必填参数。 */
  static missing(field: string): Promise<insightIntent.IntentResult<string>> {
    return Promise.resolve(HubIntentRuntime.failure(HubIntentCode.INVALID_INPUT, 'INVALID_INPUT', `缺少参数 ${field}`));
  }

  private static success(value: Object | null): insightIntent.IntentResult<string> {
    const data: Object | null = value instanceof ToolResult ? (value as ToolResult<Object | null>).data : value;
    let text: string | undefined;
    try {
      text = JSON.stringify(data);
    } catch (error) {
      return HubIntentRuntime.failure(HubIntentCode.FAILED, 'HANDLER_ERROR', `返回值无法序列化为 JSON：${(error as Error).message}`);
    }
    const result: insightIntent.IntentResult<string> = { code: 0, result: text ?? 'null' };
    return result;
  }

  private static fromError(error: Object): insightIntent.IntentResult<string> {
    if (error instanceof ToolCallError) {
      return HubIntentRuntime.failure(HubIntentCode.FAILED, error.kind, error.message);
    }
    const message = error instanceof Error ? error.message : String(error);
    return HubIntentRuntime.failure(HubIntentCode.FAILED, 'HANDLER_ERROR', message);
  }

  private static failure(code: HubIntentCode, kind: string, message: string): insightIntent.IntentResult<string> {
    const result: insightIntent.IntentResult<string> = { code: code, result: JSON.stringify(new HubIntentFailure(kind, message)) };
    return result;
  }
}
