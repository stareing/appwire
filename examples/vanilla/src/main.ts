import { attachDom } from '@app-mcp/dom'
import { createAppMcp } from '@app-mcp/web'
import './notes'

const appMcp = createAppMcp({
  appId: 'vanilla-notes',
  appName: '便签示例',
  overview: {
    summary: '纯 HTML 便签页面：新建、删除、清空便签',
    body: [
      '## 能力范围',
      '- notes.create：新建便签（标题、颜色、是否置顶）',
      '- notes.remove：按 key 删除一条便签',
      '- notes.clear：删除全部便签（破坏性操作）',
      '',
      '## 资源',
      '- notes.list：当前全部便签（JSON）',
      '- ui.snapshot：页面上可用的工具与状态',
    ].join('\n'),
    locale: 'zh-CN',
  },
})

attachDom(appMcp)

// 连接状态显示（与 MCP 登记无关，仅便于调试）
const status = document.getElementById('status')
appMcp.onStateChange((s) => {
  if (status) status.textContent = s.status === 'connected' ? '已连接 Host' : s.status
})
