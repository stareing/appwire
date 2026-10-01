/**
 * handler 返回值的类型兼容性（tsc 检查，见 typecheck 脚本）：普通返回值即使含 `data` 与同名字段也能通过，
 * 结构化结果的字段仍受类型约束。运行时行为见 result.test.ts。
 */
import { describe, expect, it } from 'vitest'
import { createAppMcp } from '../src/index'

describe('ToolResult 类型', () => {
  it('普通返回值与结构化结果都能注册', () => {
    const app = createAppMcp({ appId: 'a', appName: 'a', enabled: false })
    const handles = [
      // 旧代码：含 data 与业务 status 的普通对象（运行时整体作为 data）
      app.tool('legacy', { description: '', handler: () => ({ data: [1], status: 'success' }) }),
      app.tool('legacy.async', { description: '', handler: async () => ({ data: [1], status: 'success', code: 200 }) }),
      app.tool('hints', { description: '', handler: () => ({ data: 1, stateHints: ['a'] }) }),
      app.tool('pending', {
        description: '',
        handler: () => ({ data: 1, status: 'pending' as const, stateResource: 'r', summary: 's', annotations: { priority: 1 } }),
      }),
      // @ts-expect-error stateHints 必须是字符串数组
      app.tool('bad', { description: '', handler: () => ({ data: 1, stateHints: 'x' }) }),
    ]
    expect(handles).toHaveLength(5)
  })
})
