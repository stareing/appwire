import { ToolScope, useConnectionState, useTool, type ConnectionState } from '@app-mcp/react'
import { useState } from 'react'
import { catalogSearch } from './mcp/static-tools'
import { CartPage } from './pages/CartPage'
import { TodoPage } from './pages/TodoPage'

type Tab = 'todos' | 'cart'

const TABS: { id: Tab; label: string }[] = [
  { id: 'todos', label: '待办' },
  { id: 'cart', label: '购物车' },
]

export function App() {
  const [tab, setTab] = useState<Tab>('todos')

  // 静态工具：定义来自 static-tools.ts（与清单一致），在根组件注册，任何页签下都可用。
  // handler 惰性加载：首次调用时才加载 ./mcp/catalog-search（冷启动唤醒只初始化被调用的模块）。
  useTool(catalogSearch.name, {
    ...catalogSearch,
    load: () => import('./mcp/catalog-search'),
  })

  return (
    <div className="app">
      <header className="header">
        <h1>示例商城</h1>
        <ConnectionBadge />
      </header>
      <nav className="tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={tab === t.id}
            className={tab === t.id ? 'tab active' : 'tab'}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>
      <main className="page">
        {/* 切换页签时页面组件卸载，其 scope 下的工具与资源随之注销 */}
        {tab === 'todos' ? (
          <ToolScope name="todos">
            <TodoPage />
          </ToolScope>
        ) : (
          <ToolScope name="cart">
            <CartPage />
          </ToolScope>
        )}
      </main>
    </div>
  )
}

const STATUS_TEXT: Record<ConnectionState['status'], string> = {
  disabled: '未启用',
  idle: '未连接',
  connecting: '连接中',
  handshaking: '握手中',
  'pending-pairing': '等待配对',
  connected: '已连接',
  backoff: '重连中',
  rejected: '被拒绝',
  stopped: '已停止',
  dormant: '休眠中',
  waking: '唤醒中',
  blocked: '被浏览器拦截',
}

/** 连接状态指示。只有这个组件订阅状态，其他组件不因连接变化而重新渲染。 */
function ConnectionBadge() {
  const state = useConnectionState()
  const detail =
    state.status === 'rejected' ? `：${state.reason}` : state.status === 'blocked' ? `：${state.message}` : ''
  return (
    <span className={`badge badge-${state.status}`} title="app-mcp 与 Host 的连接状态">
      <span className="dot" />
      MCP {STATUS_TEXT[state.status]}
      {detail}
    </span>
  )
}
