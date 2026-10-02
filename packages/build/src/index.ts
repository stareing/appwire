/**
 * @app-mcp/build：Vite 插件，从静态工具定义生成 `app-mcp.json` 清单（规范见 spec/manifest.md）。
 *
 * 浏览器运行时代码请从 `@app-mcp/build/define` 导入 `defineStaticTools` / `defineOverview`，
 * 避免把插件代码打进页面。
 *
 * 注释工具（`annotations` 选项）的扫描器与代码生成见 `@app-mcp/build/annotations`（依赖 typescript）；
 * 虚拟模块 `virtual:app-mcp/annotated` 的类型声明见 `@app-mcp/build/client`。
 */
export {
  appMcp,
  appMcp as default,
  loadPages,
  loadStaticTools,
  mergePages,
  resolveOverview,
  type AnnotationsOption,
  type AppMcpPluginOptions,
  type OverviewOption,
} from './plugin'
export {
  defineOverview,
  definePage,
  definePages,
  defineStaticTool,
  defineStaticTools,
  OVERVIEW_BODY_MAX,
  OVERVIEW_SECTIONS,
  OVERVIEW_SUMMARY_MAX,
  validateOverview,
  type PageDefinition,
  type StaticToolDefinition,
} from './define'
export type { RouterKind, RouteScanOption } from './routes'
export {
  APP_ID_PATTERN,
  appIdPrefixMessage,
  generateManifest,
  ManifestError,
  NAME_PATTERN,
  normalizeWake,
  WAKE_PLATFORMS,
  RESERVED_APP_IDS,
  toInputSchema,
  toOutputSchema,
  validateManifest,
  type AppMcpManifest,
  type GenerateOptions,
  type LaunchEntry,
  type ManifestInfo,
  type ManifestLaunch,
  type ManifestPage,
  type ManifestResource,
  type ManifestTool,
  type ManifestWake,
  type ValidationResult,
  type WakeDescriptor,
  type WakeKind,
  type WakeOption,
  type WakePlatform,
} from './manifest'
export type { AppOverview } from '@app-mcp/web'
