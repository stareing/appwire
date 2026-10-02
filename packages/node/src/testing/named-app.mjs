// 按名寻址的最小 Node App（spec/naming.md；naming.test.ts 的被激活方）：registerName 登记名字、不主动连接 Hub，
// 由 Hub 拨号时接受通道；由激活启动（参数 --app-mcp-activation）时通道关闭后经 onIdleExit 退出。
//
// 运行前需构建 dist（tsup）与原生模块（build:native）。
// 环境变量：APP_MCP_APP_ID（默认 node-named）；APP_MCP_EVENT_LOG（可选，追加 `start <pid>` / `exit <pid>` 行）。
// 工具：echo（原样返回 text）、pid（返回进程号）。
import { appendFileSync } from 'node:fs'
import { createAppMcp } from '../../dist/index.js'

function note(event) {
  const path = process.env.APP_MCP_EVENT_LOG
  if (path) appendFileSync(path, `${event} ${process.pid}\n`)
}

note('start')
const appMcp = createAppMcp({
  appId: process.env.APP_MCP_APP_ID ?? 'node-named',
  appName: '按名寻址示例（Node）',
  lifecycle: { mode: 'on-demand', residency: 'exit-when-idle' },
  registerName: true,
  onIdleExit: () => {
    appMcp.dispose()
    note('exit')
    process.exit(0)
  },
})
appMcp.tool('echo', {
  description: '原样返回 text',
  input: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
  handler: ({ text }) => ({ echo: text }),
})
appMcp.tool('pid', { description: '返回进程号', handler: () => ({ pid: process.pid }) })
