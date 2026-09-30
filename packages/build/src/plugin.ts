/**
 * Vite 插件：从静态工具定义生成 `app-mcp.json` 清单。
 *
 * - `vite build`：通过 `emitFile` 输出到 `outFile`（默认 `.well-known/app-mcp.json`）；
 * - `vite dev`：通过中间件在 `/.well-known/app-mcp.json` 提供，静态工具模块（及其依赖）或
 *   总览 Markdown 文件变化时重新生成；
 * - `writeTo`：额外把清单写到指定路径（供 Host 用 `--manifest` 加载）。
 *
 * 启用 `annotations` 时，还会用 TypeScript 编译器 API 扫描带 `@mcp` JSDoc 标签的导出函数（见 annotations.ts），
 * 把它们合并进清单，并提供虚拟模块 `virtual:app-mcp/annotated`（`registerAnnotated(registrar)`）。
 *
 * 静态工具模块用 Vite 的 `runnerImport` 加载（Vite 6.1+，在独立的 module runner 环境中
 * 转换并执行 TS 源文件，返回模块及其依赖文件列表）；裸模块导入（如 zod）由 Node 直接加载。
 */
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import type { IncomingMessage, ServerResponse } from 'node:http'
import { dirname, isAbsolute, resolve } from 'node:path'
import { normalizePath, runnerImport, type Plugin, type ResolvedConfig, type ViteDevServer } from 'vite'
import type { AppOverview } from '@app-mcp/web'
import type { AnnotatedTool, AnnotationScanner, AnnotationScanResult } from './annotations'
import type { StaticToolDefinition } from './define'
import { generateManifest, validateManifest, type AppMcpManifest, type ManifestInfo } from './manifest'

/** 总览：直接给出，或者从 Markdown 文件（相对 Vite root）读取 `body`。 */
export type OverviewOption = AppOverview | { summary: string; file: string; locale?: string }

export interface AppMcpPluginOptions extends Omit<ManifestInfo, 'overview'> {
  /** 静态工具模块路径（相对 Vite root），默认导出静态工具定义数组（见 `defineStaticTools`）。 */
  staticTools?: string
  /** App 总览（spec/protocol.md 第 7 节）。给出 `file` 时读取该 Markdown 文件作为 `body`，dev 下变化时重新生成。 */
  overview?: OverviewOption
  /** 构建产物中清单的路径（相对 outDir），默认 `.well-known/app-mcp.json`。 */
  outFile?: string
  /** 额外写入清单的文件路径（相对 Vite root），dev 与 build 时都会写。 */
  writeTo?: string
  /**
   * 编译期注释工具：扫描带 `@mcp` JSDoc 标签的导出函数，参数 schema 从 TypeScript 类型推导，
   * 合并进清单，并通过虚拟模块 `virtual:app-mcp/annotated` 注册。需要项目中可解析 `typescript`。
   * `true` 使用默认范围 `src/**\/*.{ts,tsx,mts,cts}`（排除测试文件）。
   */
  annotations?: boolean | AnnotationsOption
}

export interface AnnotationsOption {
  /** 扫描的文件 glob（相对 Vite root），默认 `['src/**\/*.{ts,tsx,mts,cts}']`。 */
  include?: string[]
  /** 排除的文件 glob，默认排除 `*.test.*`、`*.spec.*`、`__tests__`、`*.d.ts`、node_modules。 */
  exclude?: string[]
  /** 用于类型解析的 tsconfig（相对 Vite root），默认 root 下的 tsconfig.json。 */
  tsconfig?: string
}

const ANNOTATED_ID = 'virtual:app-mcp/annotated'
const RESOLVED_ANNOTATED_ID = '\0virtual:app-mcp/annotated'

interface Generated {
  manifest: AppMcpManifest
  json: string
}

const DEFAULT_OUT_FILE = '.well-known/app-mcp.json'

function stripSlash(path: string): string {
  return normalizePath(path).replace(/^\/+/, '')
}

/** 加载静态工具模块，返回工具定义与依赖文件（绝对路径，已规范化）。 */
export async function loadStaticTools(
  file: string,
  root: string,
): Promise<{ tools: StaticToolDefinition<any>[]; dependencies: string[] }> {
  const { module, dependencies } = await runnerImport<{ default?: unknown }>(file, {
    root,
    logLevel: 'error',
    configFile: false,
  })
  const tools = module.default
  if (!Array.isArray(tools)) {
    throw new Error(`静态工具模块 ${file} 必须默认导出工具定义数组（可使用 defineStaticTools）`)
  }
  return {
    tools: tools as StaticToolDefinition<any>[],
    dependencies: dependencies.filter((d): d is string => typeof d === 'string').map((d) => normalizePath(d)),
  }
}

/** 解析总览选项；`file` 形式读取 Markdown 作为 body。返回总览与需要监听的文件。 */
export async function resolveOverview(
  option: OverviewOption,
  root: string,
): Promise<{ overview: AppOverview; file: string | null }> {
  if ('file' in option && typeof option.file === 'string') {
    const file = resolve(root, option.file)
    let body: string
    try {
      body = await readFile(file, 'utf8')
    } catch (err) {
      throw new Error(`无法读取总览文件 ${option.file}：${(err as Error).message}`)
    }
    const overview: AppOverview = { summary: option.summary, body: body.trim() }
    if (option.locale !== undefined) overview.locale = option.locale
    return { overview, file: normalizePath(file) }
  }
  return { overview: option as AppOverview, file: null }
}

export function appMcp(options: AppMcpPluginOptions): Plugin {
  const outFile = stripSlash(options.outFile ?? DEFAULT_OUT_FILE)

  let config: ResolvedConfig
  let staticToolsFile: string | null = null
  /** 静态工具模块及其依赖、总览文件（规范化的绝对路径）。 */
  let watched = new Set<string>()
  let current: Promise<Generated> | null = null

  const annotationOptions: AnnotationsOption | null =
    options.annotations === true ? {} : options.annotations ? options.annotations : null
  let scanner: AnnotationScanner | null = null
  let annotationScan: Promise<AnnotationScanResult> | null = null
  let annotationsModule: Promise<typeof import('./annotations')> | null = null
  /** 最近一次提供给页面的虚拟模块代码（dev 下用于判断是否需要重新加载）。 */
  let servedAnnotatedCode: string | null = null

  function loadAnnotationsModule(): Promise<typeof import('./annotations')> {
    annotationsModule ??= import('./annotations').catch((err: unknown) => {
      annotationsModule = null
      throw new Error(`启用 annotations 需要安装 typescript：${(err as Error).message}`)
    })
    return annotationsModule
  }

  /** 扫描注释工具（结果缓存到下次相关文件变化）；警告在每次扫描后输出一次。 */
  function scanAnnotated(): Promise<AnnotationScanResult> {
    if (!annotationOptions) return Promise.resolve({ tools: [], warnings: [], errors: [], dependencies: [] })
    if (!annotationScan) {
      annotationScan = loadAnnotationsModule().then((mod) => {
        scanner ??= mod.createAnnotationScanner({ root: config.root, ...annotationOptions })
        const result = scanner.scan()
        for (const warning of result.warnings) config.logger.warn(`[app-mcp] ${warning}`)
        return result
      })
      annotationScan.catch(() => {
        annotationScan = null
      })
    }
    return annotationScan
  }

  async function annotatedModuleCode(): Promise<{ code: string; result: AnnotationScanResult }> {
    const result = await scanAnnotated()
    if (result.errors.length > 0) throw new Error(result.errors.join('\n'))
    const mod = await loadAnnotationsModule()
    return { code: mod.generateAnnotatedModule(result.tools), result }
  }

  function toStaticTool(tool: AnnotatedTool): StaticToolDefinition<any> {
    const def: StaticToolDefinition<any> = { name: tool.name, description: tool.description, input: tool.inputSchema }
    if (tool.title !== undefined) def.title = tool.title
    if (tool.risk !== undefined) def.risk = tool.risk
    if (tool.activation !== undefined) def.activation = tool.activation
    return def
  }

  async function generate(): Promise<Generated> {
    const nextWatched = new Set<string>()
    let tools: StaticToolDefinition<any>[] = []
    let overview: AppOverview | undefined
    try {
      if (options.overview) {
        const resolved = await resolveOverview(options.overview, config.root)
        overview = resolved.overview
        if (resolved.file) nextWatched.add(resolved.file)
      }
      if (staticToolsFile) {
        nextWatched.add(normalizePath(staticToolsFile))
        const loaded = await loadStaticTools(staticToolsFile, config.root)
        tools = loaded.tools
        for (const dep of loaded.dependencies) nextWatched.add(dep)
      }
      if (annotationOptions) {
        const scan = await scanAnnotated()
        if (scan.errors.length > 0) throw new Error(scan.errors.join('\n'))
        const staticNames = new Set(tools.map((t) => t?.name))
        const clashes = scan.tools.filter((t) => staticNames.has(t.name))
        if (clashes.length > 0) {
          throw new Error(
            clashes.map((t) => `注释工具 ${t.name}（${t.source} 的 ${t.exportName}）与静态工具重名`).join('\n'),
          )
        }
        tools = [...tools, ...scan.tools.map(toStaticTool)]
      }
    } finally {
      // 即使加载失败也监听这些文件，修复后可以自动恢复。
      watched = new Set([...watched, ...nextWatched])
    }
    watched = nextWatched
    const { staticTools: _s, outFile: _o, writeTo: _w, overview: _ov, annotations: _a, ...info } = options
    const manifest = generateManifest({ ...info, overview }, tools, { root: config.root })
    for (const warning of validateManifest(manifest).warnings) {
      config.logger.warn(`[app-mcp] ${warning}`)
    }
    const json = `${JSON.stringify(manifest, null, 2)}\n`
    if (options.writeTo) {
      const target = resolve(config.root, options.writeTo)
      await mkdir(dirname(target), { recursive: true })
      await writeFile(target, json)
    }
    return { manifest, json }
  }

  function regenerate(): Promise<Generated> {
    current = generate()
    // 避免未处理的 rejection；错误在使用处（中间件、构建）报告。
    current.catch(() => {})
    return current
  }

  function send(res: ServerResponse, status: number, body: string): void {
    res.statusCode = status
    res.setHeader('Content-Type', 'application/json; charset=utf-8')
    res.setHeader('Cache-Control', 'no-cache')
    res.setHeader('Access-Control-Allow-Origin', '*')
    res.end(body)
  }

  return {
    name: 'app-mcp',

    resolveId(id) {
      if (id === ANNOTATED_ID) return RESOLVED_ANNOTATED_ID
      return null
    },

    async load(id) {
      if (id !== RESOLVED_ANNOTATED_ID) return null
      if (!annotationOptions) {
        this.error(`[app-mcp] 导入了 ${ANNOTATED_ID}，但插件未启用 annotations 选项`)
      }
      try {
        const { code, result } = await annotatedModuleCode()
        for (const dep of result.dependencies) this.addWatchFile(dep)
        servedAnnotatedCode = code
        return code
      } catch (err) {
        this.error(`[app-mcp] 生成注释工具失败：${(err as Error).message}`)
      }
    },

    configResolved(resolved) {
      config = resolved
      if (options.staticTools) {
        staticToolsFile = isAbsolute(options.staticTools)
          ? options.staticTools
          : resolve(config.root, options.staticTools)
      }
    },

    async buildStart() {
      if (config.command !== 'build') return
      if (this.environment && this.environment.config.consumer !== 'client') return
      try {
        await regenerate()
      } catch (err) {
        this.error(`[app-mcp] 生成清单失败：${(err as Error).message}`)
      } finally {
        for (const file of watched) this.addWatchFile(file)
      }
    },

    async generateBundle() {
      if (config.command !== 'build' || !current) return
      if (this.environment && this.environment.config.consumer !== 'client') return
      const { json } = await current
      this.emitFile({ type: 'asset', fileName: outFile, source: json })
    },

    configureServer(server: ViteDevServer) {
      const log = (err: unknown) => config.logger.error(`[app-mcp] 生成清单失败：${(err as Error).message}`)
      regenerate().catch(log)

      /** 注释工具可能变化：代码与页面上次加载的不同时，使虚拟模块失效并让页面重新加载。 */
      const refreshAnnotated = async () => {
        if (servedAnnotatedCode === null) return
        const { code } = await annotatedModuleCode()
        if (code === servedAnnotatedCode) return
        servedAnnotatedCode = null
        for (const env of Object.values(server.environments)) {
          const mod = env.moduleGraph.getModuleById(RESOLVED_ANNOTATED_ID)
          if (mod) env.moduleGraph.invalidateModule(mod)
        }
        server.ws.send({ type: 'full-reload' })
      }

      const onFileChange = (file: string) => {
        const normalized = normalizePath(file)
        const annotated = scanner !== null && scanner.isRelevant(normalized)
        // 首次扫描前（scanner 未创建）不需要处理：扫描会读取最新内容。
        if (annotated) annotationScan = null
        if (!annotated && !watched.has(normalized)) return
        regenerate().then(() => config.logger.info('[app-mcp] 清单已更新', { timestamp: true }), log)
        if (annotated) refreshAnnotated().catch(log)
      }
      server.watcher.on('change', onFileChange)
      server.watcher.on('add', onFileChange)
      server.watcher.on('unlink', onFileChange)

      const base = config.base.endsWith('/') ? config.base : `${config.base}/`
      const paths = new Set([`/${outFile}`, `${base}${outFile}`])

      server.middlewares.use((req: IncomingMessage, res: ServerResponse, next: (err?: unknown) => void) => {
        const path = (req.url ?? '').split('?')[0] ?? ''
        if (!paths.has(path)) return next()
        ;(current ?? regenerate()).then(
          (g) => send(res, 200, g.json),
          (err: unknown) => send(res, 500, JSON.stringify({ error: `生成清单失败：${(err as Error).message}` })),
        )
      })
    },
  }
}
