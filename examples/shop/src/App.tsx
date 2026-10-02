import { useConnectionState, useRouterNavigation, useTool, type ConnectionState } from '@app-mcp/react'
import { useRef } from 'react'
import { catalogSearch } from './mcp/static-tools'
import { navigate, Routes, usePathname } from './router'
import { routes } from './routes'

const TABS: { path: string; label: string }[] = [
  { path: '/', label: '待办' },
  { path: '/products', label: '商品' },
  { path: '/cart', label: '购物车' },
  { path: '/orders', label: '订单' },
]

export function App() {
  const pathname = usePathname()
  const visited = useRef(new Set<string>())

  // 静态工具：定义来自 static-tools.ts（与清单一致），在根组件注册，任何页面下都可用。
  // handler 惰性加载：首次调用时才加载 ./mcp/catalog-search（冷启动唤醒只初始化被调用的模块）。
  useTool(catalogSearch.name, {
    ...catalogSearch,
    load: () => import('./mcp/catalog-search'),
  })

  // Host 调用不在当前页面的工具时（页面目录见清单 pages）请求导航：页面名 → 路由表中的 path。
  useRouterNavigation({ navigate, pages: routes })

  return (
    <div className="app">
      <header className="header">
        <h1>示例商城</h1>
        <ConnectionBadge />
      </header>
      <nav className="tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.path}
            role="tab"
            aria-selected={pathname === t.path}
            className={pathname === t.path ? 'tab active' : 'tab'}
            onClick={() => navigate(t.path)}
          >
            {t.label}
          </button>
        ))}
      </nav>
      <main className="page">
        {/* 离开页面时组件卸载，其工具与资源随之注销；商品页 keep-alive（隐藏），工具随可见性暂停 */}
        <Routes routes={routes} visited={visited.current} />
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
  'host-mismatch': '对端不是 app-mcp',
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
