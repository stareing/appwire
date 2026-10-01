// 页面照常使用 @app-mcp/web：tauri-plugin-app-mcp 注入了宿主 IPC 桥接（window.appMcpBridge），
// createAppMcp 检测到后经 Tauri invoke 把工具登记到 Rust 侧客户端（不加载 WASM、不直连 Host）。
import { createAppMcp } from '@app-mcp/web'

const appMcp = createAppMcp({ appId: 'tauri-counter', appName: 'Tauri 计数器' })

let count = 0
const countEl = document.getElementById('count')
const render = () => {
  if (countEl) countEl.textContent = String(count)
}
document.getElementById('inc')?.addEventListener('click', () => {
  count += 1
  render()
})

appMcp.tool<{ by?: number }, { count: number }>('counter.increment', {
  description: '把页面上的计数加上 by（默认 1），返回新的计数',
  input: { type: 'object', properties: { by: { type: 'integer', minimum: 1 } } },
  handler: ({ by = 1 }) => {
    count += by
    render()
    return { count }
  },
})

appMcp.tool('counter.get', {
  description: '读取页面上的当前计数',
  risk: 'read',
  handler: () => ({ count }),
})

const status = document.getElementById('status')
appMcp.onStateChange((s) => {
  if (status) status.textContent = s.status
})
