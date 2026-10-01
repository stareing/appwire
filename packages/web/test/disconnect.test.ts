import { describe, expect, it } from 'vitest'
import { channelDisconnectIssue, socketDisconnectIssue } from '../src/disconnect'

describe('断线归类（spec/protocol.md 10.1）', () => {
  it('WebSocket 事件：CloseEvent → CONNECTION_CLOSED，其他（error）→ CONNECTION_LOST', () => {
    expect(socketDisconnectIssue({ code: 1000, reason: '', wasClean: true })).toEqual({
      code: 'CONNECTION_CLOSED',
      message: 'Host 关闭了连接（关闭码 1000）',
    })
    expect(socketDisconnectIssue({ code: 4000, reason: 'bye', wasClean: true }).message).toBe(
      'Host 关闭了连接（关闭码 4000，原因：bye）',
    )
    expect(socketDisconnectIssue({ code: 1006, wasClean: false }).message).toContain('未经关闭握手')
    expect(socketDisconnectIssue({}).code).toBe('CONNECTION_LOST')
    expect(socketDisconnectIssue(undefined).code).toBe('CONNECTION_LOST')
    expect(socketDisconnectIssue({ code: '1000' }).code).toBe('CONNECTION_LOST')
  })

  it('通道关闭：持有方给出的码原样使用；缺省或不认识的码 → CONNECTION_LOST', () => {
    expect(channelDisconnectIssue({ code: 'CONNECTION_CLOSED', reason: 'Host 关闭了通道' })).toEqual({
      code: 'CONNECTION_CLOSED',
      message: 'Host 关闭了通道',
    })
    expect(channelDisconnectIssue({ code: 'CONNECTION_CLOSED' }).message).toBe('Host 关闭了连接')
    expect(channelDisconnectIssue({ code: 'CONNECTION_LOST' }).message).toBe('与 Host 的连接中断')
    expect(channelDisconnectIssue({ reason: '共享连接无响应' })).toEqual({
      code: 'CONNECTION_LOST',
      message: '共享连接中断：共享连接无响应',
    })
    expect(channelDisconnectIssue({ code: 'NOPE' as never }).code).toBe('CONNECTION_LOST')
    expect(channelDisconnectIssue(undefined)).toEqual({ code: 'CONNECTION_LOST', message: '共享连接中断' })
  })
})
