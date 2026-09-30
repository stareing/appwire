import { defineOverview } from '@app-mcp/build/define'
import { createAppMcp } from '@app-mcp/web'
import overviewBody from './overview.md?raw'
import { OVERVIEW_LOCALE, OVERVIEW_SUMMARY } from './overview'

/** 与清单中的 overview 使用同一份 summary 与 Markdown 正文。 */
export const overview = defineOverview({
  summary: OVERVIEW_SUMMARY,
  body: overviewBody.trim(),
  locale: OVERVIEW_LOCALE,
})

/**
 * 生命周期演示：`VITE_APP_MCP_LIFECYCLE=idle pnpm dev`（或 `on-demand`）。
 * idle：空闲 30 秒（标签页隐藏时 5 秒）后休眠，页面重新可见或 Host 唤醒时回连。缺省 persistent（一直在线）。
 * 空闲时间可用 `VITE_APP_MCP_IDLE_MS` / `VITE_APP_MCP_HIDDEN_IDLE_MS` 覆盖（e2e 用短时间）。
 */
const env = import.meta.env
const lifecycleMode = env.VITE_APP_MCP_LIFECYCLE
const positiveInt = (value: unknown, fallback: number): number => {
  const n = Number(value)
  return Number.isInteger(n) && n > 0 ? n : fallback
}
const lifecycle =
  lifecycleMode === 'idle' || lifecycleMode === 'on-demand'
    ? {
        mode: lifecycleMode,
        idleTimeoutMs: positiveInt(env.VITE_APP_MCP_IDLE_MS, 30_000),
        hiddenIdleTimeoutMs: positiveInt(env.VITE_APP_MCP_HIDDEN_IDLE_MS, 5_000),
      }
    : undefined
/** Host 地址，缺省 ws://127.0.0.1:7717（e2e 用独立端口）。 */
const hostUrl: string | undefined = env.VITE_APP_MCP_HOST_URL || undefined

export const appMcp = createAppMcp({
  appId: 'shop',
  appName: '示例商城',
  appVersion: '0.1.0',
  enabled: import.meta.env.VITE_APP_MCP !== 'off',
  overview,
  ...(lifecycle && { lifecycle }),
  ...(hostUrl && { hostUrl }),
})
