/** @app-mcp/hub 的公开类型：策略回调：审批、配对与唤醒（spec/hub-api.md 3.3、3.5）。 */

import type { Risk, ToolAnnotations } from '../types.js'

export interface ApprovalRequest {
  callId: string
  appId: string
  appName: string
  tool: string
  title: string | null
  description: string
  /** 旧写法；按声明决定是否确认时以 `annotations` 为准。 */
  risk: Risk
  /** 工具的 MCP 注解（与 {@link HubTool.annotations} 相同）。 */
  annotations: ToolAnnotations
  arguments: unknown
  /** Hub API 为 `CallRequest.session` 原样；MCP 出口为调用方键（`mcp:<n>` / `principal:<主体>`）。 */
  session: string | null
  /** MCP 出口：发起调用的认证主体（取自传输层凭据，现在恒为 `local`）；经 `callTool` 发起时缺省。 */
  principal?: string
  /**
   * MCP 出口：客户端自报的 `clientInfo.name`；经 `callTool` 发起时缺省。
   * 自报、不可信，**仅供显示**，不得据此做授权决定。
   */
  clientName?: string
}

export interface PairingRequest {
  appId: string
  appName: string
  origin: string | null
  clientKind: string
  instanceId: string
}

/** 唤醒方式（spec/lifecycle.md 第 5 节）。 */
export type WakeKind = 'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none'

export interface WakeDescriptor {
  kind: WakeKind
  /** scheme、AUMID、bundle id、D-Bus 名称、组件名或 URL。 */
  target?: string | null
  /** 能否不把窗口带到前台就唤醒。 */
  background: boolean
}

/** 交给 {@link Waker} 的唤醒请求（spec/hub-api.md 3.5）。 */
export interface WakeRequest {
  appId: string
  /** 被唤醒的休眠实例；null = App 未运行，按清单冷启动。 */
  instanceId: string | null
  descriptor: WakeDescriptor
  /** 一次性唤醒令牌（32 位十六进制）。 */
  token: string
  /** 通用激活参数 `app-mcp-wake:<token>`，App 端 SDK 的 `handleWake` 可识别。 */
  activationArg: string
}

/**
 * 自定义唤醒（如 Android 发送显式广播）：resolve = 已发出激活，Hub 随后等待 App 回连（`wakeTimeoutMs`）。
 * 抛错 / reject → 调用以 `LAUNCH_FAILED` 结束；抛出 `kind` 为协议错误类别的 {@link HubError}（如 `APP_NOT_INSTALLED`）则用该类别。
 */
export type Waker = (req: WakeRequest) => void | Promise<void>

/** 返回 true 同意；返回 false、抛错、reject、超时均视为拒绝。 */
export type ApprovalHandler = (req: ApprovalRequest) => boolean | Promise<boolean>
/** 返回 true 同意配对；返回 false、抛错、reject、超时均视为拒绝。 */
export type PairingHandler = (req: PairingRequest) => boolean | Promise<boolean>
