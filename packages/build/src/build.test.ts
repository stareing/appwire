import { readFile, writeFile } from 'node:fs/promises'
import type { AddressInfo } from 'node:net'
import { join } from 'node:path'
import { build, createServer, type ViteDevServer } from 'vite'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { z } from 'zod'
import {
  appMcp,
  defineStaticTools,
  generateManifest,
  loadStaticTools,
  ManifestError,
  normalizeWake,
  toInputSchema,
  toOutputSchema,
  validateManifest,
  validateOverview,
  type AppMcpManifest,
} from './index'
import { createTempProjects } from './testing/temp-projects'

const projects = createTempProjects()
const makeProject = projects.make

const STATIC_TOOLS = `
import { z } from 'zod'
import { LIMIT } from './limit'

const tools: unknown[] = [
  {
    name: 'catalog.search',
    title: '搜索商品',
    description: '按关键词搜索商品',
    input: z.object({ keyword: z.string().optional(), limit: z.number().max(LIMIT).default(10) }),
    risk: 'read',
    activation: 'headless',
  },
]
export default tools
`

afterAll(() => projects.cleanup())

describe('generateManifest', () => {
  it('转换 zod / JSON Schema / toJSONSchema 对象，并生成符合规范的清单', () => {
    const tools = defineStaticTools([
      {
        name: 'orders.search',
        description: '搜索订单',
        input: z.object({ keyword: z.string().describe('关键词'), page: z.number().default(1) }),
        risk: 'read',
        activation: 'headless',
        title: '搜索订单',
      },
      {
        name: 'orders.export',
        description: '导出订单',
        input: { type: 'object', properties: { format: { type: 'string', enum: ['csv', 'xlsx'] } } },
      },
      {
        name: 'orders.custom',
        description: '自定义 schema',
        input: { toJSONSchema: () => ({ type: 'object', properties: { x: { type: 'number' } } }) },
      },
      { name: 'orders.refresh', description: '无参数' },
    ])
    const manifest = generateManifest(
      {
        appId: 'shop',
        name: '示例商城',
        version: '1.0.0',
        description: '演示',
        launch: { web: 'http://localhost:5173/', windows: [{ type: 'uri', scheme: 'shop-app' }] },
        resources: [{ name: 'cart.state', description: '购物车' }],
      },
      tools,
    )
    expect(manifest).toMatchObject({
      manifestVersion: 1,
      appId: 'shop',
      name: '示例商城',
      version: '1.0.0',
      description: '演示',
      launch: {
        web: [{ type: 'url', href: 'http://localhost:5173/' }],
        windows: [{ type: 'uri', scheme: 'shop-app' }],
      },
      resources: [{ name: 'cart.state', description: '购物车' }],
    })
    const [search, exp, custom, refresh] = manifest.tools!
    expect(search).toMatchObject({
      name: 'orders.search',
      title: '搜索订单',
      risk: 'read',
      activation: 'headless',
      inputSchema: {
        type: 'object',
        properties: { keyword: { type: 'string', description: '关键词' }, page: { type: 'number', default: 1 } },
        // io: 'input'：带默认值的字段不是必填
        required: ['keyword'],
      },
    })
    expect(exp!.inputSchema.properties).toEqual({ format: { type: 'string', enum: ['csv', 'xlsx'] } })
    expect(custom!.inputSchema).toEqual({ type: 'object', properties: { x: { type: 'number' } } })
    expect(refresh).toEqual({ name: 'orders.refresh', description: '无参数', inputSchema: { type: 'object', properties: {} } })
    // 结果可 JSON 序列化且与自身一致
    expect(JSON.parse(JSON.stringify(manifest))).toEqual(manifest)
  })

  it('不带工具时省略 tools 字段', () => {
    expect(generateManifest({ appId: 'a', name: 'A' })).toEqual({ manifestVersion: 1, appId: 'a', name: 'A' })
  })

  it('校验错误汇总后抛出 ManifestError', () => {
    let error: unknown
    try {
      generateManifest({ appId: 'Shop', name: '' }, [
        { name: 'a', description: 'x' },
        { name: 'a', description: 'x' },
        { name: 'bad name', description: 'x' },
        { name: 'arr', description: 'x', input: { type: 'array' } as never },
        { name: 'empty', description: '' },
      ])
    } catch (err) {
      error = err
    }
    expect(error).toBeInstanceOf(ManifestError)
    const errors = (error as ManifestError).errors
    expect(errors).toEqual(
      expect.arrayContaining([
        expect.stringContaining('appId "Shop" 格式不合法'),
        'name 不能为空',
        expect.stringContaining('tools[1]（a） 名称重复'),
        expect.stringContaining('tools[2]（bad name） 名称不合法'),
        expect.stringContaining('tools[3]（arr） inputSchema.type 必须为 "object"'),
        expect.stringContaining('tools[4]（empty） description 不能为空'),
      ]),
    )
  })

  it('保留 appId 与非法 risk 报错；未知 launch 类型只警告', () => {
    const manifest = {
      manifestVersion: 1,
      appId: 'host',
      name: 'x',
      launch: { web: [{ type: 'magic', foo: 1 }] },
      tools: [{ name: 't', description: 'd', inputSchema: { type: 'object' }, risk: 'danger' }],
      resources: [
        { name: 'r', description: 'd' },
        { name: 'r', description: 'd' },
      ],
    } as unknown as AppMcpManifest
    const { errors, warnings } = validateManifest(manifest)
    expect(errors).toEqual([
      expect.stringContaining('保留名'),
      expect.stringContaining('risk "danger" 不合法'),
      expect.stringContaining('resources[1]（r） 名称重复'),
    ])
    expect(warnings).toEqual([expect.stringContaining('"magic" 未知')])
  })

  it('工具名以 appId. 开头时警告（局部名，spec/protocol.md 3.1）', () => {
    const manifest = {
      manifestVersion: 1,
      appId: 'shop',
      name: '商城',
      tools: [
        { name: 'shop.info', description: 'd', inputSchema: { type: 'object' } },
        { name: 'shopping.list', description: 'd', inputSchema: { type: 'object' } },
      ],
    } as unknown as AppMcpManifest
    const { errors, warnings } = validateManifest(manifest)
    expect(errors).toEqual([])
    expect(warnings).toEqual([expect.stringMatching(/tools\[0\]（shop\.info）.*请改为 "info"/)])
  })

  it('静态工具的 annotations 与 outputSchema 写入清单（zod 按输出形态转换）', () => {
    const manifest = generateManifest({ appId: 'shop', name: '商城' }, [
      {
        name: 'orders.cancel',
        description: '取消订单',
        risk: 'destructive',
        annotations: { idempotentHint: true, openWorldHint: false },
        outputSchema: z.object({ cancelled: z.boolean().default(false) }),
      },
      { name: 'orders.ids', description: '订单号', outputSchema: { type: 'array', items: { type: 'string' } } },
      { name: 'plain', description: 'd' },
    ])
    expect(manifest.tools?.[0]).toMatchObject({
      risk: 'destructive',
      annotations: { idempotentHint: true, openWorldHint: false },
      outputSchema: { type: 'object', properties: { cancelled: { type: 'boolean' } }, required: ['cancelled'] },
    })
    expect(manifest.tools?.[1]?.outputSchema).toEqual({ type: 'array', items: { type: 'string' } })
    expect(manifest.tools?.[2]).not.toHaveProperty('annotations')
    expect(manifest.tools?.[2]).not.toHaveProperty('outputSchema')
    expect(() => toOutputSchema(5)).toThrow(/outputSchema 必须是/)
  })

  it('校验工具注解与 outputSchema：类型不对报错，未知注解字段警告', () => {
    const manifest = {
      manifestVersion: 1,
      appId: 'shop',
      name: '商城',
      tools: [
        {
          name: 't',
          description: 'd',
          inputSchema: { type: 'object' },
          annotations: { readOnlyHint: 'yes', title: '标题', fooHint: true },
          outputSchema: [],
        },
        { name: 'u', description: 'd', inputSchema: { type: 'object' }, annotations: null },
      ],
    } as unknown as AppMcpManifest
    const { errors, warnings } = validateManifest(manifest)
    expect(errors).toEqual([
      expect.stringContaining('tools[0]（t） annotations.readOnlyHint 必须是 boolean'),
      expect.stringContaining('tools[0]（t） outputSchema 必须是对象'),
      expect.stringContaining('tools[1]（u） annotations 必须是对象'),
    ])
    expect(warnings).toEqual([expect.stringContaining('annotations.fooHint 不是标准 MCP 工具注解字段')])
  })

  it('toInputSchema 拒绝非对象输入', () => {
    expect(() => toInputSchema('x')).toThrow(/input 必须是/)
  })
})

describe('wake', () => {
  it('web 应用默认从 launch.web 生成 web-url 唤醒（位于 launch 之后）', () => {
    const manifest = generateManifest({ appId: 'shop', name: 'x', launch: { web: 'http://localhost:5173/#/home' } })
    expect(manifest.wake).toEqual({ web: [{ kind: 'web-url', target: 'http://localhost:5173/' }] })
    expect(Object.keys(manifest)).toEqual(['manifestVersion', 'appId', 'name', 'launch', 'wake'])
    // 没有 launch.web 时不生成
    expect(generateManifest({ appId: 'shop', name: 'x' }).wake).toBeUndefined()
    expect(
      generateManifest({ appId: 'shop', name: 'x', launch: { windows: [{ type: 'uri', scheme: 'shop' }] } }).wake,
    ).toBeUndefined()
  })

  it('插件选项可覆盖、补充或关闭', () => {
    const launch = { web: 'http://localhost:5173/' }
    expect(generateManifest({ appId: 'a', name: 'x', launch, wake: false }).wake).toBeUndefined()
    expect(generateManifest({ appId: 'a', name: 'x', launch, wake: { web: false } }).wake).toBeUndefined()
    expect(generateManifest({ appId: 'a', name: 'x', launch, wake: { web: 'https://shop.example.com/app' } }).wake).toEqual({
      web: [{ kind: 'web-url', target: 'https://shop.example.com/app' }],
    })
    const manifest = generateManifest({
      appId: 'a',
      name: 'x',
      launch,
      wake: {
        windows: [{ kind: 'aumid', target: 'Co.Shop!App' }, { kind: 'uri', target: 'shop-app' }],
        android: { kind: 'android-intent', target: 'com.co.shop/dev.appmcp.WakeReceiver', background: true },
      },
    })
    expect(manifest.wake).toEqual({
      windows: [
        { kind: 'aumid', target: 'Co.Shop!App' },
        { kind: 'uri', target: 'shop-app' },
      ],
      android: [{ kind: 'android-intent', target: 'com.co.shop/dev.appmcp.WakeReceiver', background: true }],
      web: [{ kind: 'web-url', target: 'http://localhost:5173/' }],
    })
    expect(normalizeWake(undefined, undefined)).toBeUndefined()
  })

  it('非法唤醒描述报错，平台不匹配等只警告', () => {
    expect(() =>
      generateManifest({ appId: 'a', name: 'x', wake: { web: { kind: 'web-url', target: 'shop://x' } } }),
    ).toThrow(/http\(s\)/)
    const manifest = {
      manifestVersion: 1,
      appId: 'a',
      name: 'x',
      wake: {
        web: [{ kind: 'web-url' }, { kind: 'web-url', target: 'https://a/', background: true }],
        windows: [{ kind: 'uri', target: 'https' }, { kind: 'uri', target: 'bad scheme' }, { kind: 'dbus', target: 'a.b' }],
        android: [{ kind: 'android-intent', target: 'com.co.shop' }, { kind: 'teleport', target: 'x' }],
        ios: ['shop-app', { kind: 'uri', target: 1, background: 'yes' }],
        linux: [{ kind: 'dbus', target: 'shop' }, { kind: 'none', target: 'x' }],
        harmony: [{ kind: 'uri', target: 'shop' }],
        macos: { kind: 'uri', target: 'shop' },
      },
    } as unknown as AppMcpManifest
    const { errors, warnings } = validateManifest(manifest)
    expect(errors).toEqual([
      'wake.web[0]（web-url）缺少 target',
      expect.stringContaining('wake.windows[0].target 不能是 "https"'),
      expect.stringContaining('wake.windows[1].target 应为 URI scheme'),
      expect.stringContaining('wake.android[0].target 应为 <包名>/<接收器类名>'),
      'wake.ios[0] 必须是带字符串 kind 字段的对象',
      'wake.ios[1].target 必须是字符串',
      'wake.ios[1].background 必须是布尔值',
      'wake.macos 必须是数组',
    ])
    expect(warnings).toEqual([
      expect.stringContaining('wake.web[1] 网页无法在后台唤醒'),
      expect.stringContaining('wake.windows[2] 的唤醒方式 "dbus" 不适用于平台 windows'),
      expect.stringContaining('wake.android[1] 的 kind "teleport" 未知'),
      expect.stringContaining('wake.linux[0].target D-Bus 名称'),
      expect.stringContaining('"none" 不需要 target'),
      'wake.harmony 是未知的平台，将原样保留',
    ])
  })
})

describe('overview', () => {
  const BODY = '## 能力范围\n- 待办\n\n## 典型流程\n- 结算：cart.state → cart.checkout'

  it('写入清单（位于 description 之后），并校验 summary', () => {
    const manifest = generateManifest({
      appId: 'shop',
      name: '示例商城',
      description: 'd',
      overview: { summary: '演示商城', body: BODY, locale: 'zh-CN' },
      launch: { web: 'http://localhost:5173/' },
    })
    expect(manifest.overview).toEqual({ summary: '演示商城', body: BODY, locale: 'zh-CN' })
    expect(Object.keys(manifest)).toEqual(['manifestVersion', 'appId', 'name', 'description', 'overview', 'launch', 'wake'])
    expect(() => generateManifest({ appId: 'shop', name: 'x', overview: { summary: '  ' } })).toThrow(
      /overview.summary 不能为空/,
    )
  })

  it('超长与缺少建议小节时只警告', () => {
    expect(validateOverview({ summary: '好'.repeat(100), body: BODY })).toEqual({ errors: [], warnings: [] })
    const { errors, warnings } = validateOverview({ summary: '好'.repeat(101), body: `# 介绍\n${'字'.repeat(2001)}` })
    expect(errors).toEqual([])
    expect(warnings).toEqual([
      expect.stringContaining('summary 超过 100 字符（101）'),
      expect.stringContaining('body 超过 2000 字符'),
      expect.stringContaining('建议包含以下小节之一'),
    ])
    // 小节名只出现在正文而不是标题中，也给出警告
    expect(validateOverview({ summary: 's', body: '这里提到能力范围' }).warnings).toHaveLength(1)
    expect(validateOverview({ summary: 's', body: 1 }).errors).toEqual(['overview.body 必须是字符串'])
    expect(validateOverview(null).errors).toEqual(['overview 必须是对象'])
  })
})

describe('loadStaticTools', () => {
  it('用 runnerImport 加载 TS 模块，返回依赖文件', async () => {
    const dir = await makeProject({
      'static-tools.ts': STATIC_TOOLS,
      'limit.ts': 'export const LIMIT: number = 50\n',
    })
    const { tools, dependencies } = await loadStaticTools(join(dir, 'static-tools.ts'), dir)
    expect(tools).toHaveLength(1)
    expect(tools[0]!.name).toBe('catalog.search')
    expect(dependencies.some((d) => d.endsWith('/limit.ts'))).toBe(true)
    const manifest = generateManifest({ appId: 'shop', name: 'x' }, tools)
    expect(manifest.tools![0]!.inputSchema.properties).toMatchObject({ limit: { maximum: 50 } })
  })

  it('默认导出不是数组时报错', async () => {
    const dir = await makeProject({ 'bad.ts': 'export default { name: "x" }\n' })
    await expect(loadStaticTools(join(dir, 'bad.ts'), dir)).rejects.toThrow(/必须默认导出工具定义数组/)
  })
})

describe('dev 中间件', () => {
  let dir: string
  let server: ViteDevServer
  let base: string

  beforeAll(async () => {
    dir = await makeProject({
      'index.html': '<!doctype html><html><body></body></html>',
      'static-tools.ts': STATIC_TOOLS,
      'limit.ts': 'export const LIMIT = 50\n',
      'overview.md': '## 能力范围\n- 商品搜索\n',
    })
    server = await createServer({
      root: dir,
      configFile: false,
      logLevel: 'silent',
      server: { port: 0, host: '127.0.0.1', ws: false, watch: null },
      plugins: [
        appMcp({
          appId: 'shop',
          name: '示例商城',
          staticTools: './static-tools.ts',
          writeTo: 'out/app-mcp.json',
          overview: { summary: '演示商城', file: 'overview.md', locale: 'zh-CN' },
        }),
      ],
    })
    await server.listen()
    const { port } = server.httpServer!.address() as AddressInfo
    base = `http://127.0.0.1:${port}`
  })

  afterAll(async () => {
    await server?.close()
  })

  async function fetchManifest(): Promise<{ status: number; body: any; type: string | null }> {
    const res = await fetch(`${base}/.well-known/app-mcp.json`)
    return { status: res.status, body: await res.json(), type: res.headers.get('content-type') }
  }

  it('在 /.well-known/app-mcp.json 提供清单，并写入 writeTo', async () => {
    const { status, body, type } = await fetchManifest()
    expect(status).toBe(200)
    expect(type).toContain('application/json')
    expect(body).toMatchObject({ manifestVersion: 1, appId: 'shop', tools: [{ name: 'catalog.search' }] })
    const written = JSON.parse(await readFile(join(dir, 'out/app-mcp.json'), 'utf8'))
    expect(written).toEqual(body)
  })

  it('从 Markdown 文件读取总览正文', async () => {
    const { body } = await fetchManifest()
    expect(body.overview).toEqual({ summary: '演示商城', body: '## 能力范围\n- 商品搜索', locale: 'zh-CN' })
  })

  async function waitFor(check: () => Promise<boolean>): Promise<void> {
    for (let i = 0; i < 100; i++) {
      if (await check()) return
      await new Promise((r) => setTimeout(r, 50))
    }
    throw new Error('等待超时')
  }

  it('静态工具模块（含依赖）变化时重新生成', async () => {
    await writeFile(join(dir, 'limit.ts'), 'export const LIMIT = 99\n')
    server.watcher.emit('change', join(dir, 'limit.ts'))
    await waitFor(async () => (await fetchManifest()).body.tools?.[0]?.inputSchema?.properties?.limit?.maximum === 99)

    await writeFile(
      join(dir, 'static-tools.ts'),
      STATIC_TOOLS.replace('const tools: unknown[] = [', "const tools: unknown[] = [{ name: 'extra', description: '新增' },"),
    )
    server.watcher.emit('change', join(dir, 'static-tools.ts'))
    await waitFor(async () => (await fetchManifest()).body.tools?.length === 2)
  })

  it('总览文件变化时重新生成', async () => {
    await writeFile(join(dir, 'overview.md'), '## 典型流程\n- 搜索后加入购物车\n')
    server.watcher.emit('change', join(dir, 'overview.md'))
    await waitFor(async () => (await fetchManifest()).body.overview?.body === '## 典型流程\n- 搜索后加入购物车')
  })

  it('生成失败时返回 500 与错误信息', async () => {
    await writeFile(join(dir, 'static-tools.ts'), "export default [{ name: 'bad name', description: 'x' }]\n")
    server.watcher.emit('change', join(dir, 'static-tools.ts'))
    await waitFor(async () => (await fetch(`${base}/.well-known/app-mcp.json`)).status === 500)
    const body = (await fetchManifest()).body
    expect(body.error).toContain('名称不合法')
  })

  it('其他请求交给后续中间件', async () => {
    const res = await fetch(`${base}/`)
    expect(res.status).toBe(200)
    expect(await res.text()).toContain('<body>')
  })
})

describe('vite build', () => {
  it('输出清单到 outDir，并写入 writeTo', async () => {
    const dir = await makeProject({
      'index.html': '<!doctype html><html><body><script type="module" src="/main.ts"></script></body></html>',
      'main.ts': 'document.body.dataset.app = "shop"\n',
      'src/mcp/overview.md': '## 风险说明\n- 结算为支付操作\n',
      'src/mcp/static-tools.ts': STATIC_TOOLS.replace("'./limit'", "'../../limit'"),
      'limit.ts': 'export const LIMIT = 50\n',
    })
    await build({
      root: dir,
      configFile: false,
      logLevel: 'silent',
      plugins: [
        appMcp({
          appId: 'shop',
          name: '示例商城',
          launch: { web: 'http://localhost:5173/' },
          staticTools: 'src/mcp/static-tools.ts',
          writeTo: 'app-mcp.json',
          overview: { summary: '演示商城', file: 'src/mcp/overview.md' },
        }),
      ],
    })
    const emitted = JSON.parse(await readFile(join(dir, 'dist/.well-known/app-mcp.json'), 'utf8'))
    expect(emitted).toMatchObject({
      appId: 'shop',
      launch: { web: [{ type: 'url', href: 'http://localhost:5173/' }] },
      wake: { web: [{ kind: 'web-url', target: 'http://localhost:5173/' }] },
      overview: { summary: '演示商城', body: '## 风险说明\n- 结算为支付操作' },
      tools: [{ name: 'catalog.search', risk: 'read' }],
    })
    expect(JSON.parse(await readFile(join(dir, 'app-mcp.json'), 'utf8'))).toEqual(emitted)
  })

  it('outFile 可自定义；清单不合法时构建失败', async () => {
    const dir = await makeProject({
      'index.html': '<!doctype html><html><body></body></html>',
      'tools.ts': 'export default []\n',
    })
    await build({
      root: dir,
      configFile: false,
      logLevel: 'silent',
      plugins: [appMcp({ appId: 'shop', name: 'x', staticTools: 'tools.ts', outFile: '/app-mcp.json' })],
    })
    expect(JSON.parse(await readFile(join(dir, 'dist/app-mcp.json'), 'utf8'))).toEqual({
      manifestVersion: 1,
      appId: 'shop',
      name: 'x',
    })

    await expect(
      build({
        root: dir,
        configFile: false,
        logLevel: 'silent',
        plugins: [appMcp({ appId: 'apps', name: 'x' })],
      }),
    ).rejects.toThrow(/保留名/)

    await expect(
      build({
        root: dir,
        configFile: false,
        logLevel: 'silent',
        plugins: [appMcp({ appId: 'shop', name: 'x', overview: { summary: 's', file: 'missing.md' } })],
      }),
    ).rejects.toThrow(/无法读取总览文件 missing.md/)
  })
})
