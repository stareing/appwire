import { ErrorKind } from './Types';

const ERROR_KINDS: string[] = [
  'TOOL_NOT_FOUND', 'TOOL_DISABLED', 'INVALID_INPUT', 'USER_REJECTED', 'TIMEOUT', 'HANDLER_ERROR', 'CANCELLED',
  'APP_DISCONNECTED', 'APP_NOT_INSTALLED', 'LAUNCH_FAILED', 'APP_NOT_RESPONDING', 'INSTANCE_FROZEN',
  'RESOURCE_NOT_FOUND', 'UNAUTHORIZED', 'UNSUPPORTED_PROTOCOL',
];

export function isErrorKind(value: string): boolean {
  return ERROR_KINDS.includes(value);
}

/**
 * handler 抛出此错误以指定错误类别；其他异常归为 `HANDLER_ERROR`。与 @app-mcp/web、@app-mcp/node 同形。
 * `details` 随错误的 `data` 发给 Host（须可序列化为 JSON 对象）。
 */
export class ToolCallError extends Error {
  readonly kind: ErrorKind;
  readonly details?: Record<string, Object>;

  constructor(kind: ErrorKind, message: string, details?: Record<string, Object>) {
    super(message);
    this.name = 'ToolCallError';
    this.kind = kind;
    this.details = details;
  }
}

/** handler 失败的归一化结果。 */
export class Failure {
  readonly kind: ErrorKind;
  readonly message: string;
  /** 详情的 JSON 文本；没有或无法序列化时为 undefined。 */
  readonly detailsJson?: string;

  constructor(kind: ErrorKind, message: string, detailsJson?: string) {
    this.kind = kind;
    this.message = message;
    this.detailsJson = detailsJson;
  }
}

/** 把 handler 抛出 / 拒绝的值转换为 {@link Failure}。 */
export function toFailure(error: Object | null | undefined): Failure {
  if (error instanceof ToolCallError) {
    let detailsJson: string | undefined = undefined;
    if (error.details !== undefined) {
      try {
        detailsJson = JSON.stringify(error.details);
      } catch (e) {
        detailsJson = undefined;
      }
    }
    return new Failure(error.kind, error.message, detailsJson);
  }
  if (error instanceof Error) {
    return new Failure('HANDLER_ERROR', error.message !== '' ? error.message : 'handler 执行失败');
  }
  if (error === undefined || error === null) {
    return new Failure('HANDLER_ERROR', 'handler 执行失败');
  }
  return new Failure('HANDLER_ERROR', String(error));
}

/** 原生模块抛出的错误带 `code`（如 `DUPLICATE_NAME`、`ALREADY_COMPLETED`）。 */
export function nativeErrorCode(error: Object | null | undefined): string | undefined {
  if (error instanceof Error) {
    const code = (error as BusinessLikeError).code;
    return typeof code === 'string' ? code : undefined;
  }
  return undefined;
}

/** 带 `code` 的错误（Node-API 抛出的 Error）。 */
interface BusinessLikeError {
  code?: string;
}
