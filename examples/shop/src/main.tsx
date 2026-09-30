import { AppMcpProvider } from '@app-mcp/react'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { registerAnnotated } from 'virtual:app-mcp/annotated'
import { App } from './App'
import { appMcp } from './mcp/app'
import './styles.css'

// 注册 src 中用 @mcp 注释声明的工具（全局可用，不随页签变化）。
registerAnnotated(appMcp)

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AppMcpProvider value={appMcp}>
      <App />
    </AppMcpProvider>
  </StrictMode>,
)
