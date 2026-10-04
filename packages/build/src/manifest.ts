/**
 * 清单生成与校验（规范见 spec/manifest.md）。
 */
import { createRequire } from 'node:module'
import { join } from 'node:path'
import type { Activation, AppOverview, CachePolicy, Risk, ToolAnnotations, ToolDeprecation, ToolSurface } from '@app-mcp/web'
import { cacheReadOnlyWarning, checkCache } from './cache'
import { checkDeprecation, deprecatedRequiredWarnings } from './deprecation'
import type { PageDefinition, StaticToolDefinition } from './define'
import { checkImplements } from './intents'
import { validateOverview } from './overview'

// ---------------------------------------------------------------------------
// 类型
// ---------------------------------------------------------------------------

export type LaunchEntry =
  | { type: 'url'; href: string }
  | { type: 'uri'; scheme: string }
  | { type: 'aumid'; id: string }
  | { type: 'exe'; path: string }
  | { type: 'bundle'; id: string }
  | { type: 'dbus'; name: string }
  | { type: 'desktop'; file: string }
  | { type: string; [key: string]: unknown }

export interface ManifestLaunch {
  web?: LaunchEntry[]
  windows?: LaunchEntry[]
  macos?: LaunchEntry[]
  linux?: LaunchEntry[]
}

/** 唤醒方式（spec/lifecycle.md 第 5 节、spec/manifest.md 第 2.2 节）。 */
export type WakeKind = 'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none'

/** 唤醒描述（WakeDescriptor）。 */
export interface WakeDescriptor {
  kind: WakeKind
  /** 各 kind 的定位信息；`none` 不需要。 */
  target?: string
  /** 能否不把窗口带到前台就唤醒，缺省 false。 */
  background?: boolean
}

export type WakePlatform = 'web' | 'windows' | 'macos' | 'linux' | 'android' | 'ios'

/** 清单 `wake` 字段：各平台按顺序尝试的唤醒描述。 */
export type ManifestWake = Partial<Record<WakePlatform, WakeDescriptor[]>>

/** 协议中的 ToolInfo。 */
export interface ManifestTool {
  name: string
  description: string
  inputSchema: { type: 'object'; [key: string]: unknown }
  risk?: Risk
  activation?: Activation
  title?: string
  /** 标准 MCP 工具注解（spec/protocol.md 第 3 节）。 */
  annotations?: ToolAnnotations
  /** 结果的 JSON Schema（MCP `outputSchema`，根类型不限于 object）。 */
  outputSchema?: Record<string, unknown>
  /** 对界面的依赖（spec/protocol.md 3.4），缺省 `app`。 */
  surface?: ToolSurface
  /** 所在页面名。 */
  page?: string
  /** 实现的标准意图（spec/intents.md），每项 `<动词>@<主版本>`。 */
  implements?: string[]
  /** 结果缓存声明（spec/protocol.md 3.6）：只对只读工具生效。 */
  cache?: CachePolicy
  /** 弃用声明（spec/protocol.md 3.7）。 */
  deprecated?: ToolDeprecation
  /** 成功结果可能带 `undo`（spec/protocol.md 3.8）；只写 `true`。 */
  undoable?: boolean
}

/** 清单 `pages` 条目（spec/manifest.md 2.3）。 */
export interface ManifestPage {
  name: string
  title?: string
  description?: string
  route?: string
  params?: Record<string, unknown>
  tools?: ManifestTool[]
  navigable?: boolean
  activation?: Activation
}

/** 协议中的 ResourceInfo。 */
export interface ManifestResource {
  name: string
  description: string
  mimeType?: string
  /** 读取结果缓存声明（spec/protocol.md 3.6）。 */
  cache?: CachePolicy
}

/** 清单 `events` 条目（spec/manifest.md 2.4，同协议 `EventInfo`）。 */
export interface ManifestEvent {
  /** 事件名 `[a-zA-Z0-9_.-]{1,64}`，清单内唯一（与工具名、资源名分属不同命名空间）。 */
  name: string
  /** 面向模型：事件何时发生、载荷含义。 */
  description: string
  /** 载荷的 JSON Schema（描述用，Hub 不校验）。 */
  payloadSchema?: Record<string, unknown>
}

export interface AppMcpManifest {
  manifestVersion: 1
  appId: string
  name: string
  version?: string
  description?: string
  /** App 总览，见 spec/protocol.md 第 7 节。 */
  overview?: AppOverview
  launch?: ManifestLaunch
  /** 各平台唤醒描述，见 spec/manifest.md 第 2.2 节。 */
  wake?: ManifestWake
  tools?: ManifestTool[]
  resources?: ManifestResource[]
  /** 页面目录（spec/manifest.md 2.3）。 */
  pages?: ManifestPage[]
  /** App 可发出的事件（spec/manifest.md 2.4）；SDK 仍需运行时 `declareEvent` 后才能发出。 */
  events?: ManifestEvent[]
}

/**
 * 插件 / 生成选项中的唤醒配置：
 * - `false`：不写 `wake`（也不生成 web 默认值）；
 * - 各平台可给单个描述、数组，或 `false`（不写该平台）；`web` 还可以直接写地址字符串（等价于 `web-url`）。
 * 未给出 `web` 时，默认取 `launch.web` 的第一个地址生成 `web-url`。
 */
export type WakeOption =
  | false
  | ({ web?: string | WakeDescriptor | WakeDescriptor[] | false } & {
      [P in Exclude<WakePlatform, 'web'>]?: WakeDescriptor | WakeDescriptor[] | false
    })

/** 生成清单所需的 App 信息（插件选项的子集）。 */
export interface ManifestInfo {
  appId: string
  name: string
  version?: string
  description?: string
  overview?: AppOverview
  /** `web` 可以直接写地址字符串，等价于 `[{ type: 'url', href }]`。 */
  launch?: Omit<ManifestLaunch, 'web'> & { web?: string | LaunchEntry[] }
  /** 唤醒描述；缺省时 web 平台由 `launch.web` 推导，见 {@link WakeOption}。 */
  wake?: WakeOption
  resources?: ManifestResource[]
  /** 静态事件声明（spec/manifest.md 2.4），原样写入清单 `events`。 */
  events?: ManifestEvent[]
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

export const APP_ID_PATTERN = /^[a-z][a-z0-9-]{0,62}$/
export const NAME_PATTERN = /^[a-zA-Z0-9_.-]{1,64}$/
export const RESERVED_APP_IDS = ['apps', 'os', 'ax', 'host'] as const
const RISKS: readonly string[] = ['read', 'write', 'destructive', 'payment', 'os-sensitive']
const ACTIVATIONS: readonly string[] = ['headless', 'background', 'foreground']
const SURFACES: readonly string[] = ['app', 'view']
/** 工具注解的字段 → 取值类型。 */
const TOOL_ANNOTATION_FIELDS: Readonly<Record<keyof ToolAnnotations, 'string' | 'boolean'>> = {
  title: 'string',
  readOnlyHint: 'boolean',
  destructiveHint: 'boolean',
  idempotentHint: 'boolean',
  openWorldHint: 'boolean',
}

const isJsonObject = (v: unknown): v is Record<string, unknown> =>
  v !== null && typeof v === 'object' && !Array.isArray(v)

/** 工具注解的问题列表（未知字段为警告，取值类型不对为错误）。 */
function checkToolAnnotations(annotations: unknown, label: string, errors: string[], warnings: string[]): void {
  if (!isJsonObject(annotations)) {
    errors.push(`${label} annotations 必须是对象`)
    return
  }
  for (const [key, value] of Object.entries(annotations)) {
    const expected = TOOL_ANNOTATION_FIELDS[key as keyof ToolAnnotations]
    if (expected === undefined) warnings.push(`${label} annotations.${key} 不是标准 MCP 工具注解字段`)
    else if (typeof value !== expected) errors.push(`${label} annotations.${key} 必须是 ${expected}`)
  }
}
const KNOWN_LAUNCH: Record<string, { platforms: string[]; field: string }> = {
  url: { platforms: ['web'], field: 'href' },
  uri: { platforms: ['windows', 'macos', 'linux'], field: 'scheme' },
  aumid: { platforms: ['windows'], field: 'id' },
  exe: { platforms: ['windows', 'linux'], field: 'path' },
  bundle: { platforms: ['macos'], field: 'id' },
  dbus: { platforms: ['linux'], field: 'name' },
  desktop: { platforms: ['linux'], field: 'file' },
}

export const WAKE_PLATFORMS: readonly WakePlatform[] = ['web', 'windows', 'macos', 'linux', 'android', 'ios']
const WAKE_KIND_PLATFORMS: Record<WakeKind, readonly string[]> = {
  uri: ['windows', 'macos', 'linux', 'android', 'ios'],
  aumid: ['windows'],
  'apple-event': ['macos'],
  dbus: ['linux'],
  'android-intent': ['android'],
  'web-url': ['web'],
  none: WAKE_PLATFORMS,
}
const URI_SCHEME = /^[a-zA-Z][a-zA-Z0-9+.-]*$/
const FORBIDDEN_SCHEMES = ['http', 'https', 'file', 'javascript', 'data']

function validateWakeEntry(entry: unknown, platform: string, path: string, errors: string[], warnings: string[]): void {
  if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) {
    errors.push(`${path} 必须是带字符串 kind 字段的对象`)
    return
  }
  const { kind, target, background } = entry as Record<string, unknown>
  if (typeof kind !== 'string') {
    errors.push(`${path} 必须是带字符串 kind 字段的对象`)
    return
  }
  const platforms = WAKE_KIND_PLATFORMS[kind as WakeKind]
  if (!platforms) {
    warnings.push(`${path} 的 kind "${kind}" 未知，将原样保留`)
    return
  }
  if (target !== undefined && typeof target !== 'string') errors.push(`${path}.target 必须是字符串`)
  if (background !== undefined && typeof background !== 'boolean') errors.push(`${path}.background 必须是布尔值`)
  if (!platforms.includes(platform)) warnings.push(`${path} 的唤醒方式 "${kind}" 不适用于平台 ${platform}`)
  const t = typeof target === 'string' ? target.trim() : ''
  if (kind === 'none') {
    if (target !== undefined) warnings.push(`${path} 的唤醒方式 "none" 不需要 target，已忽略`)
    return
  }
  if (typeof target === 'string' && t === '') errors.push(`${path}.target 不能为空（${kind}）`)
  else if (target === undefined) errors.push(`${path}（${kind}）缺少 target`)
  if (t === '') return
  switch (kind) {
    case 'web-url':
      if (!/^https?:\/\//.test(t)) errors.push(`${path}.target "${t}" 必须是 http(s) 地址`)
      else if (t.includes('#')) warnings.push(`${path}.target 不应包含片段（#），Host 会附加 #app-mcp-wake=<token>`)
      if (background === true) warnings.push(`${path} 网页无法在后台唤醒，background 将被忽略`)
      break
    case 'uri':
      if (!URI_SCHEME.test(t)) errors.push(`${path}.target 应为 URI scheme（如 shop-app），"${t}" 不合法`)
      else if (FORBIDDEN_SCHEMES.includes(t.toLowerCase())) errors.push(`${path}.target 不能是 "${t}"，网页请用 web-url`)
      break
    case 'android-intent': {
      const slash = t.indexOf('/')
      if (slash <= 0 || slash === t.length - 1) errors.push(`${path}.target 应为 <包名>/<接收器类名>，实际为 "${t}"`)
      break
    }
    case 'dbus':
      if (!t.includes('.')) warnings.push(`${path}.target D-Bus 名称 "${t}" 通常应为反向域名形式`)
      break
  }
}

export interface ValidationResult {
  errors: string[]
  warnings: string[]
}

/**
 * 工具 / 资源名是 App 内的局部名，Host 对外暴露为 `<appId>.<局部名>`（spec/protocol.md 3.1）。
 * 名称以 `<appId>.` 开头时返回提示文案（多半是误把全名写成了局部名），否则返回 `null`。
 */
export function appIdPrefixMessage(name: string, appId: string): string | null {
  if (appId === '' || !name.startsWith(`${appId}.`) || name.length <= appId.length + 1) return null
  const local = name.slice(appId.length + 1)
  return `名称 "${name}" 以 appId 前缀 "${appId}." 开头：名称是 App 内的局部名，Host 对外暴露为 "${appId}.${name}"；如果本意是全名，请改为 "${local}"（spec/protocol.md 3.1）`
}

/** 按 spec/manifest.md 第 3 节校验清单。 */
export function validateManifest(manifest: AppMcpManifest): ValidationResult {
  const errors: string[] = []
  const warnings: string[] = []

  if (manifest.manifestVersion !== 1) errors.push(`manifestVersion 必须为 1，实际为 ${String(manifest.manifestVersion)}`)
  if (typeof manifest.appId !== 'string' || !APP_ID_PATTERN.test(manifest.appId)) {
    errors.push(`appId "${String(manifest.appId)}" 格式不合法，应满足 [a-z][a-z0-9-]{0,62}`)
  } else if ((RESERVED_APP_IDS as readonly string[]).includes(manifest.appId)) {
    errors.push(`appId "${manifest.appId}" 是保留名（${RESERVED_APP_IDS.join('、')}）`)
  }
  if (typeof manifest.name !== 'string' || manifest.name.trim() === '') errors.push('name 不能为空')
  if (manifest.description !== undefined && manifest.description.trim() === '') {
    errors.push('description 不能为空字符串')
  }
  if (manifest.overview !== undefined) {
    const result = validateOverview(manifest.overview)
    errors.push(...result.errors)
    warnings.push(...result.warnings)
  }

  for (const [platform, entries] of Object.entries(manifest.launch ?? {})) {
    if (!Array.isArray(entries)) {
      errors.push(`launch.${platform} 必须是数组`)
      continue
    }
    entries.forEach((entry: LaunchEntry, i: number) => {
      const known = KNOWN_LAUNCH[entry?.type]
      if (!known) {
        warnings.push(`launch.${platform}[${i}] 的 type "${String(entry?.type)}" 未知，将原样保留`)
        return
      }
      const value = (entry as Record<string, unknown>)[known.field]
      if (typeof value !== 'string' || value === '') {
        errors.push(`launch.${platform}[${i}]（${entry.type}）缺少字段 ${known.field}`)
      }
      if (!known.platforms.includes(platform)) {
        warnings.push(`launch.${platform}[${i}] 的 type "${entry.type}" 通常不用于 ${platform}`)
      }
    })
  }

  if (manifest.wake !== undefined) {
    if (manifest.wake === null || typeof manifest.wake !== 'object' || Array.isArray(manifest.wake)) {
      errors.push('wake 必须是对象')
    } else {
      for (const [platform, entries] of Object.entries(manifest.wake as Record<string, unknown>)) {
        if (!(WAKE_PLATFORMS as readonly string[]).includes(platform)) {
          warnings.push(`wake.${platform} 是未知的平台，将原样保留`)
          continue
        }
        if (!Array.isArray(entries)) {
          errors.push(`wake.${platform} 必须是数组`)
          continue
        }
        entries.forEach((entry, i) => validateWakeEntry(entry, platform, `wake.${platform}[${i}]`, errors, warnings))
      }
    }
  }

  const toolNames = new Set<string>()
  const appId = typeof manifest.appId === 'string' ? manifest.appId : ''
  for (const [i, tool] of (manifest.tools ?? []).entries()) {
    checkTool(tool, `tools[${i}]`, appId, toolNames, errors, warnings)
  }
  const pageNames = checkPages(manifest.pages, appId, toolNames, errors, warnings)
  for (const [i, tool] of (manifest.tools ?? []).entries()) {
    if (typeof tool.page === 'string' && !pageNames.has(tool.page)) {
      warnings.push(`tools[${i}]（${tool.name}）的 page "${tool.page}" 没有在 pages 中声明`)
    }
  }

  const resourceNames = new Set<string>()
  for (const [i, resource] of (manifest.resources ?? []).entries()) {
    const label = `resources[${i}]${typeof resource.name === 'string' ? `（${resource.name}）` : ''}`
    if (typeof resource.name !== 'string' || !NAME_PATTERN.test(resource.name)) {
      errors.push(`${label} 名称不合法，应满足 [a-zA-Z0-9_.-]{1,64}`)
    } else if (resourceNames.has(resource.name)) {
      errors.push(`${label} 名称重复`)
    } else {
      resourceNames.add(resource.name)
    }
    if (typeof resource.description !== 'string' || resource.description.trim() === '') {
      errors.push(`${label} description 不能为空`)
    }
    if (resource.cache !== undefined) checkCache(resource.cache, label, errors)
  }
  checkEvents(manifest.events, appId, errors, warnings)

  return { errors, warnings }
}

/** `events` 的规则（spec/manifest.md 2.4，与 crates/manifest 的 `validate_events` 一致）。 */
function checkEvents(events: unknown, appId: string, errors: string[], warnings: string[]): void {
  if (events === undefined) return
  if (!Array.isArray(events)) {
    errors.push('events 必须是数组')
    return
  }
  const names = new Set<string>()
  for (const [i, event] of (events as unknown[]).entries()) {
    if (!isJsonObject(event)) {
      errors.push(`events[${i}] 必须是对象`)
      continue
    }
    const e = event as Partial<ManifestEvent>
    const label = `events[${i}]${typeof e.name === 'string' ? `（${e.name}）` : ''}`
    if (typeof e.name !== 'string' || !NAME_PATTERN.test(e.name)) {
      errors.push(`${label} 名称不合法，应满足 [a-zA-Z0-9_.-]{1,64}`)
    } else if (names.has(e.name)) {
      errors.push(`${label} 名称重复`)
    } else {
      names.add(e.name)
      const prefix = appIdPrefixMessage(e.name, appId)
      if (prefix) warnings.push(`${label} ${prefix}`)
    }
    if (typeof e.description !== 'string' || e.description.trim() === '') {
      errors.push(`${label} description 不能为空`)
    }
    if (e.payloadSchema !== undefined && !isJsonObject(e.payloadSchema)) {
      errors.push(`${label} payloadSchema 必须是对象`)
    }
  }
}

/**
 * 单个工具（顶层或页面内）的规则：名称、唯一性（`toolNames` 跨顶层与页面共享）、appId 前缀、description、inputSchema、
 * risk / activation / surface / page、annotations、outputSchema、implements、cache、deprecated、undoable。
 */
function checkTool(
  tool: ManifestTool,
  where: string,
  appId: string,
  toolNames: Set<string>,
  errors: string[],
  warnings: string[],
): void {
  const label = `${where}${typeof tool?.name === 'string' ? `（${tool.name}）` : ''}`
  if (!isJsonObject(tool)) {
    errors.push(`${where} 必须是对象`)
    return
  }
  if (typeof tool.name !== 'string' || !NAME_PATTERN.test(tool.name)) {
    errors.push(`${label} 名称不合法，应满足 [a-zA-Z0-9_.-]{1,64}`)
  } else if (toolNames.has(tool.name)) {
    errors.push(`${label} 名称重复（工具名在顶层 tools 与所有 pages[].tools 之间唯一）`)
  } else {
    toolNames.add(tool.name)
    const prefix = appIdPrefixMessage(tool.name, appId)
    if (prefix) warnings.push(`${label} ${prefix}`)
  }
  if (typeof tool.description !== 'string' || tool.description.trim() === '') {
    errors.push(`${label} description 不能为空`)
  }
  const schema = tool.inputSchema as unknown
  if (schema === null || typeof schema !== 'object' || Array.isArray(schema)) {
    errors.push(`${label} inputSchema 必须是对象`)
  } else if ((schema as { type?: unknown }).type !== 'object') {
    errors.push(`${label} inputSchema.type 必须为 "object"`)
  }
  if (tool.risk !== undefined && !RISKS.includes(tool.risk)) errors.push(`${label} risk "${tool.risk}" 不合法`)
  if (tool.activation !== undefined && !ACTIVATIONS.includes(tool.activation)) {
    errors.push(`${label} activation "${tool.activation}" 不合法`)
  }
  if (tool.surface !== undefined && !SURFACES.includes(tool.surface)) {
    errors.push(`${label} surface "${String(tool.surface)}" 不合法，应为 app 或 view`)
  }
  if (tool.page !== undefined && (typeof tool.page !== 'string' || !NAME_PATTERN.test(tool.page))) {
    errors.push(`${label} page 不合法，应满足 [a-zA-Z0-9_.-]{1,64}`)
  }
  if (tool.annotations !== undefined) checkToolAnnotations(tool.annotations, label, errors, warnings)
  if (tool.outputSchema !== undefined && !isJsonObject(tool.outputSchema)) {
    errors.push(`${label} outputSchema 必须是对象`)
  }
  if (tool.implements !== undefined) checkImplements(tool.implements, label, errors)
  if (tool.cache !== undefined) {
    checkCache(tool.cache, label, errors)
    const readOnly = cacheReadOnlyWarning(tool, label)
    if (readOnly) warnings.push(readOnly)
  }
  if (tool.deprecated !== undefined) checkDeprecation(tool.deprecated, tool.name, label, errors)
  warnings.push(...deprecatedRequiredWarnings(schema, label))
  if (tool.undoable !== undefined && typeof tool.undoable !== 'boolean') errors.push(`${label} undoable 必须是布尔值`)
}

/** `pages` 的规则（spec/manifest.md 第 3 节），返回已声明的页面名。 */
function checkPages(
  pages: unknown,
  appId: string,
  toolNames: Set<string>,
  errors: string[],
  warnings: string[],
): Set<string> {
  const names = new Set<string>()
  if (pages === undefined) return names
  if (!Array.isArray(pages)) {
    errors.push('pages 必须是数组')
    return names
  }
  for (const [i, page] of (pages as unknown[]).entries()) {
    if (!isJsonObject(page)) {
      errors.push(`pages[${i}] 必须是对象`)
      continue
    }
    const p = page as unknown as ManifestPage
    const label = `pages[${i}]${typeof p.name === 'string' ? `（${p.name}）` : ''}`
    if (typeof p.name !== 'string' || !NAME_PATTERN.test(p.name)) {
      errors.push(`${label} 页面名不合法，应满足 [a-zA-Z0-9_.-]{1,64}`)
    } else if (names.has(p.name)) {
      errors.push(`${label} 页面名重复`)
    } else {
      names.add(p.name)
    }
    if (p.description !== undefined && (typeof p.description !== 'string' || p.description.trim() === '')) {
      errors.push(`${label} description 不能为空字符串`)
    }
    if (p.params !== undefined && (!isJsonObject(p.params) || p.params.type !== 'object')) {
      errors.push(`${label} params 必须是 type 为 "object" 的 JSON Schema`)
    }
    if (p.navigable !== undefined && typeof p.navigable !== 'boolean') errors.push(`${label} navigable 必须是布尔值`)
    if (p.activation !== undefined && !ACTIVATIONS.includes(p.activation)) {
      errors.push(`${label} activation "${String(p.activation)}" 不合法`)
    }
    if (p.tools !== undefined && !Array.isArray(p.tools)) {
      errors.push(`${label} tools 必须是数组`)
      continue
    }
    for (const [j, tool] of (p.tools ?? []).entries()) {
      checkTool(tool, `${label}.tools[${j}]`, appId, toolNames, errors, warnings)
      if (isJsonObject(tool) && tool.page !== undefined && tool.page !== p.name) {
        errors.push(`${label}.tools[${j}]（${String(tool.name)}）的 page "${String(tool.page)}" 必须等于所在页面名`)
      }
    }
  }
  return names
}

/** 清单校验失败。 */
export class ManifestError extends Error {
  readonly errors: string[]
  constructor(errors: string[]) {
    super(`app-mcp 清单校验失败：\n${errors.map((e) => `  - ${e}`).join('\n')}`)
    this.name = 'ManifestError'
    this.errors = errors
  }
}

// ---------------------------------------------------------------------------
// 生成
// ---------------------------------------------------------------------------

type ZodToJsonSchema = (schema: unknown, params?: Record<string, unknown>) => unknown

/** 从项目根目录加载 zod 的 `toJSONSchema`（用于没有 `toJSONSchema()` 方法的 zod schema，如 zod/mini）。 */
function loadZodToJsonSchema(root: string): ZodToJsonSchema {
  const require = createRequire(join(root, 'package.json'))
  let mod: { toJSONSchema?: ZodToJsonSchema; z?: { toJSONSchema?: ZodToJsonSchema } }
  try {
    mod = require('zod') as typeof mod
  } catch {
    throw new Error('输入定义是 zod schema，但无法从项目中加载 zod；请安装 zod 或改用 JSON Schema')
  }
  const fn = mod.toJSONSchema ?? mod.z?.toJSONSchema
  if (typeof fn !== 'function') throw new Error('当前 zod 版本不支持 toJSONSchema，需要 zod v4')
  return fn
}

/**
 * 把工具输入定义转换为 JSON Schema：
 * - 缺省：`{ type: 'object', properties: {} }`；
 * - zod v4 schema：`toJSONSchema({ io: 'input' })`（按输入形态，带默认值的字段为可选）；
 * - 带 `toJSONSchema()` 的对象：调用它；
 * - 其他：视为 JSON Schema，深拷贝。
 */
export function toInputSchema(input: unknown, root: string = process.cwd()): Record<string, unknown> {
  if (input === undefined) return { type: 'object', properties: {} }
  return convertSchemaDefinition(input, 'input', root)
}

/** 输出定义（MCP `outputSchema`）→ JSON Schema，形式同 {@link toInputSchema}，zod 按输出形态转换。 */
export function toOutputSchema(output: unknown, root: string = process.cwd()): Record<string, unknown> {
  return convertSchemaDefinition(output, 'output', root)
}

function convertSchemaDefinition(definition: unknown, io: 'input' | 'output', root: string): Record<string, unknown> {
  const field = io === 'input' ? 'input' : 'outputSchema'
  if (definition === null || typeof definition !== 'object') {
    throw new Error(`${field} 必须是 JSON Schema 对象、zod schema 或带 toJSONSchema() 的对象`)
  }
  const candidate = definition as { _zod?: unknown; toJSONSchema?: unknown }
  const isZod = '_zod' in candidate
  let schema: unknown
  if (typeof candidate.toJSONSchema === 'function') {
    schema = isZod
      ? (candidate.toJSONSchema as (p: unknown) => unknown).call(definition, { io })
      : (candidate.toJSONSchema as () => unknown).call(definition)
  } else if (isZod) {
    schema = loadZodToJsonSchema(root)(definition, { io })
  } else {
    schema = definition
  }
  return JSON.parse(JSON.stringify(schema)) as Record<string, unknown>
}

function normalizeLaunch(launch: ManifestInfo['launch']): ManifestLaunch | undefined {
  if (!launch) return undefined
  const result: ManifestLaunch = {}
  for (const [platform, value] of Object.entries(launch) as [keyof ManifestLaunch, string | LaunchEntry[]][]) {
    if (value === undefined) continue
    result[platform] = typeof value === 'string' ? [{ type: 'url', href: value }] : value
  }
  return Object.keys(result).length > 0 ? result : undefined
}

/** `launch.web` 中第一个 url 条目的地址。 */
function firstWebUrl(launch: ManifestLaunch | undefined): string | undefined {
  for (const entry of launch?.web ?? []) {
    if (entry?.type === 'url' && typeof (entry as { href?: unknown }).href === 'string') {
      return (entry as { href: string }).href
    }
  }
  return undefined
}

/**
 * 规范化唤醒配置（见 {@link WakeOption}）。未给出 `web` 时从 `launch.web` 推导 `web-url`。
 */
export function normalizeWake(option: WakeOption | undefined, launch: ManifestLaunch | undefined): ManifestWake | undefined {
  if (option === false) return undefined
  const result: ManifestWake = {}
  const given = (option ?? {}) as Record<string, string | WakeDescriptor | WakeDescriptor[] | false | undefined>
  for (const [platform, value] of Object.entries(given)) {
    if (value === undefined || value === false) continue
    const list =
      typeof value === 'string'
        ? [{ kind: platform === 'web' ? 'web-url' : 'uri', target: value } as WakeDescriptor]
        : Array.isArray(value)
          ? value
          : [value]
    ;(result as Record<string, WakeDescriptor[]>)[platform] = list.map((d) => ({ ...d }))
  }
  if (!('web' in given)) {
    const href = firstWebUrl(launch)
    if (href !== undefined) result.web = [{ kind: 'web-url', target: href.split('#')[0] ?? href }]
  }
  return Object.keys(result).length > 0 ? result : undefined
}

export interface GenerateOptions {
  /** 解析 zod 的项目根目录，默认 `process.cwd()`。 */
  root?: string
}

/**
 * 根据 App 信息与静态工具定义生成清单，并按规范校验；有错误时抛出 {@link ManifestError}。
 */
export function generateManifest(
  info: ManifestInfo,
  tools: readonly StaticToolDefinition<any>[] = [],
  options: GenerateOptions = {},
  pages: readonly PageDefinition[] = [],
): AppMcpManifest {
  const errors: string[] = []
  const manifestTools: ManifestTool[] = []
  for (const [i, tool] of tools.entries()) {
    const entry = toManifestTool(tool, `tools[${i}]`, options.root, errors)
    if (entry) manifestTools.push(entry)
  }
  const manifestPages: ManifestPage[] = []
  for (const [i, page] of pages.entries()) {
    const entry = toManifestPage(page, `pages[${i}]`, options.root, errors)
    if (entry) manifestPages.push(entry)
  }

  const manifest: AppMcpManifest = { manifestVersion: 1, appId: info.appId, name: info.name }
  if (info.version !== undefined) manifest.version = info.version
  if (info.description !== undefined) manifest.description = info.description
  if (info.overview !== undefined) {
    const { summary, body, locale } = info.overview
    manifest.overview = { summary }
    if (body !== undefined) manifest.overview.body = body
    if (locale !== undefined) manifest.overview.locale = locale
  }
  const launch = normalizeLaunch(info.launch)
  if (launch) manifest.launch = launch
  const wake = normalizeWake(info.wake, launch)
  if (wake) manifest.wake = wake
  if (manifestTools.length > 0) manifest.tools = manifestTools
  if (info.resources && info.resources.length > 0) {
    manifest.resources = info.resources.map((r) => ({ ...r }))
  }
  if (manifestPages.length > 0) manifest.pages = manifestPages
  if (info.events && info.events.length > 0) {
    manifest.events = info.events.map((e) => JSON.parse(JSON.stringify(e)) as ManifestEvent)
  }

  errors.push(...validateManifest(manifest).errors)
  if (errors.length > 0) throw new ManifestError(errors)
  return manifest
}

/** 工具定义 → 清单条目（转换 input / outputSchema）；无法转换时记录错误并返回 undefined。 */
function toManifestTool(
  tool: StaticToolDefinition<any>,
  where: string,
  root: string | undefined,
  errors: string[],
): ManifestTool | undefined {
  if (tool === null || typeof tool !== 'object') {
    errors.push(`${where} 不是对象`)
    return undefined
  }
  let inputSchema: Record<string, unknown>
  let outputSchema: Record<string, unknown> | undefined
  try {
    inputSchema = toInputSchema(tool.input, root)
  } catch (err) {
    errors.push(`${where}（${String(tool.name)}）的 input 无法转换为 JSON Schema：${(err as Error).message}`)
    return undefined
  }
  try {
    outputSchema = tool.outputSchema === undefined ? undefined : toOutputSchema(tool.outputSchema, root)
  } catch (err) {
    errors.push(`${where}（${String(tool.name)}）的 outputSchema 无法转换为 JSON Schema：${(err as Error).message}`)
    return undefined
  }
  const entry: ManifestTool = {
    name: tool.name,
    description: tool.description,
    inputSchema: inputSchema as ManifestTool['inputSchema'],
  }
  if (tool.title !== undefined) entry.title = tool.title
  if (tool.risk !== undefined) entry.risk = tool.risk
  if (tool.activation !== undefined) entry.activation = tool.activation
  if (tool.annotations !== undefined) entry.annotations = { ...tool.annotations }
  if (outputSchema !== undefined) entry.outputSchema = outputSchema
  if (tool.surface !== undefined && tool.surface !== 'app') entry.surface = tool.surface
  if (tool.page !== undefined) entry.page = tool.page
  if (tool.implements !== undefined && tool.implements.length > 0) entry.implements = [...tool.implements]
  if (tool.cache !== undefined) entry.cache = { ...tool.cache }
  if (tool.deprecated !== undefined) entry.deprecated = { ...tool.deprecated }
  if (tool.undoable === true) entry.undoable = true
  return entry
}

/** 页面定义 → 清单条目（转换 params 与页面内工具）。 */
function toManifestPage(
  page: PageDefinition,
  where: string,
  root: string | undefined,
  errors: string[],
): ManifestPage | undefined {
  if (page === null || typeof page !== 'object') {
    errors.push(`${where} 不是对象`)
    return undefined
  }
  const entry: ManifestPage = { name: page.name }
  if (page.title !== undefined) entry.title = page.title
  if (page.description !== undefined) entry.description = page.description
  if (page.route !== undefined) entry.route = page.route
  if (page.params !== undefined) {
    try {
      entry.params = toInputSchema(page.params, root)
    } catch (err) {
      errors.push(`${where}（${String(page.name)}）的 params 无法转换为 JSON Schema：${(err as Error).message}`)
    }
  }
  const tools: ManifestTool[] = []
  for (const [j, tool] of (page.tools ?? []).entries()) {
    const converted = toManifestTool(tool, `${where}.tools[${j}]`, root, errors)
    if (converted) tools.push(converted)
  }
  if (tools.length > 0) entry.tools = tools
  if (page.navigable !== undefined) entry.navigable = page.navigable
  if (page.activation !== undefined) entry.activation = page.activation
  return entry
}
