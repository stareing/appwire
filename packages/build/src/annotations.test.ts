import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build, createServer, type ViteDevServer } from 'vite'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import {
  createAnnotationScanner,
  generateAnnotatedModule,
  globToRegExp,
  scanAnnotations,
  type AnnotatedTool,
  type AnnotationScanResult,
} from './annotations'
import { appMcp } from './index'

// 与 build.test.ts 使用不同的临时目录，避免并行运行时互相清理。
const pkgDir = fileURLToPath(new URL('..', import.meta.url))
const tmpRoot = join(pkgDir, 'node_modules', '.tmp-annotations')

async function makeProject(files: Record<string, string>): Promise<string> {
  await mkdir(tmpRoot, { recursive: true })
  const dir = await mkdtemp(join(tmpRoot, 'app-mcp-'))
  for (const [name, content] of Object.entries(files)) {
    await mkdir(join(dir, name, '..'), { recursive: true })
    await writeFile(join(dir, name), content)
  }
  return dir
}

afterAll(async () => {
  await rm(tmpRoot, { recursive: true, force: true })
})

const CART = `
export interface Order { id: string; total: number }

/**
 * 结算当前购物车
 * @mcp cart.checkout
 * @risk payment
 * @activation foreground
 * @title 结算
 * @param addressId 收货地址 ID
 */
export async function checkout(addressId: string, note?: string, count: number = 1): Promise<Order> {
  return { id: addressId + (note ?? '') + count, total: count }
}

/** @mcp 清空购物车 */
export function clear(): void {}

/**
 * @mcp 按条件添加商品
 * @param input.productId 商品 ID
 */
export const add = (input: {
  productId: string
  /** 数量 */
  quantity?: number
}) => ({ ok: true, ...input })

interface ToolContext { callId: string; signal: AbortSignal }

/** @mcp cart.withContext 读取调用上下文 @risk read */
export function withContext(id: string, context: ToolContext) {
  return { id, callId: context.callId }
}

/** 内部函数 @mcp 不会导出 */
function hidden(): void {}
hidden()

/** @mcp 通过 export 列表导出 */
function renamed(): string { return 'renamed' }
export { renamed as exportedName }

export class Store {
  /** @mcp 静态方法 @risk read */
  static count(n: number): number { return n * 2 }
  /** @mcp 实例方法 */
  instance(): void {}
}
`

const TYPES = `
export enum Color { Red = 'red', Blue = 'blue' }
export interface Address {
  /** 城市 */
  city: string
  zip?: string
  tags: string[]
}
interface Node1 { child?: Node1; value: number }

/** @mcp 各种类型 */
export function kinds(
  s: string,
  n: number,
  b: boolean,
  lit: 'a' | 'b' | 'c',
  nums: 1 | 2,
  color: Color,
  list: Array<{ x: number }>,
  addr: Address,
  when: Date,
  tuple: [string, number],
  dict: Record<string, number>,
  nullable: string | null,
  mixed: string | number,
  flag?: boolean,
  tree?: Node1,
): void {}

/** @mcp 单个 Date 参数不展开 */
export function onlyDate(at: Date): void {}

/** @mcp 单个数组参数 */
export function onlyList(ids: string[]): void {}

/** @mcp 解构参数 @param a 第一个 */
export function destructured({ a, b }: { a: string; b?: number }) { return a + String(b) }

/** @mcp 函数参数 */
export function fnParam(cb: () => void): void {}

/** @mcp Symbol 参数 */
export function symParam(s: symbol): void {}

/** @mcp 泛型参数 */
export function generic<T>(value: T): T { return value }

/** @mcp 含方法的对象 */
export function withMethod(m: Map<string, number>): void {}

/** @mcp 剩余参数 */
export function restParam(...ids: string[]): void {}

/** @mcp 非法风险 @risk dangerous */
export function badRisk(): void {}

/** @mcp */
export function noDescription(): void {}
`

describe('globToRegExp', () => {
  it('支持 **、*、{a,b}', () => {
    const re = globToRegExp('src/**/*.{ts,tsx}')
    expect(re.test('src/a.ts')).toBe(true)
    expect(re.test('src/x/y/b.tsx')).toBe(true)
    expect(re.test('src/a.js')).toBe(false)
    expect(re.test('lib/a.ts')).toBe(false)
    expect(globToRegExp('**/*.test.*').test('src/a.test.ts')).toBe(true)
  })
})

describe('scanAnnotations', () => {
  let dir: string
  let result: AnnotationScanResult
  const tool = (name: string): AnnotatedTool => {
    const found = result.tools.find((t) => t.name === name)
    if (!found) throw new Error(`没有工具 ${name}：${result.tools.map((t) => t.name).join(', ')}`)
    return found
  }

  beforeAll(async () => {
    dir = await makeProject({
      'src/cart.ts': CART,
      'src/types.ts': TYPES,
      'src/orders/index.ts': '/** @mcp 列出订单 @risk read */\nexport const list = () => []\n',
      'src/cart.test.ts': '/** @mcp 测试文件 */\nexport function inTest() {}\n',
      'src/plain.ts': 'export function noTag() {}\n',
      'other/outside.ts': '/** @mcp 范围之外 */\nexport function outside() {}\n',
    })
    result = scanAnnotations({ root: dir })
  })

  it('推导名称与描述，读取 risk / activation / title', () => {
    expect(tool('cart.checkout')).toMatchObject({
      description: '结算当前购物车',
      risk: 'payment',
      activation: 'foreground',
      title: '结算',
      source: 'src/cart.ts',
      exportName: 'checkout',
    })
    expect(tool('cart.clear')).toMatchObject({ description: '清空购物车', exportName: 'clear' })
    expect(tool('cart.clear').risk).toBeUndefined()
    expect(tool('cart.withContext')).toMatchObject({ description: '读取调用上下文', risk: 'read' })
    // export 列表导出：名称用本地函数名，导入用导出名。
    expect(tool('cart.renamed')).toMatchObject({ exportName: 'exportedName', description: '通过 export 列表导出' })
    // index.ts 用目录名。
    expect(tool('orders.list')).toMatchObject({ risk: 'read', source: 'src/orders/index.ts' })
    // 类的静态方法。
    expect(tool('cart.count')).toMatchObject({ exportName: 'Store', member: 'count', risk: 'read' })
  })

  it('只扫描 include 范围，排除测试文件与无标签文件', () => {
    const names = result.tools.map((t) => t.name)
    expect(names).not.toContain('cart.inTest')
    expect(names).not.toContain('outside.outside')
    expect(names.some((n) => n.includes('noTag'))).toBe(false)
    expect(result.dependencies.some((d) => d.endsWith('/src/cart.ts'))).toBe(true)
    expect(result.dependencies.some((d) => d.endsWith('/src/plain.ts'))).toBe(false)
  })

  it('多个位置参数组成对象，可选参数非必填，@param 作为描述', () => {
    const t = tool('cart.checkout')
    expect(t.inputSchema).toEqual({
      type: 'object',
      properties: {
        addressId: { type: 'string', description: '收货地址 ID' },
        note: { type: 'string' },
        count: { type: 'number' },
      },
      required: ['addressId'],
    })
    expect(t.call).toEqual({ mode: 'positional', params: ['addressId', 'note', 'count'], context: false })
    expect(tool('cart.clear')).toMatchObject({
      inputSchema: { type: 'object', properties: {} },
      call: { mode: 'none', context: false },
    })
  })

  it('单个对象参数直接展开；属性 JSDoc 与 @param input.x 作为描述', () => {
    const t = tool('cart.add')
    expect(t.inputSchema).toEqual({
      type: 'object',
      properties: {
        productId: { type: 'string', description: '商品 ID' },
        quantity: { type: 'number', description: '数量' },
      },
      required: ['productId'],
    })
    expect(t.call).toEqual({ mode: 'object', context: false })
    expect(tool('types.destructured')).toMatchObject({
      inputSchema: {
        type: 'object',
        properties: { a: { type: 'string', description: '第一个' }, b: { type: 'number' } },
        required: ['a'],
      },
      call: { mode: 'object' },
    })
  })

  it('末尾的 ToolContext 参数不进入 schema，调用时传入上下文', () => {
    const t = tool('cart.withContext')
    expect(t.inputSchema).toEqual({ type: 'object', properties: { id: { type: 'string' } }, required: ['id'] })
    expect(t.call).toEqual({ mode: 'positional', params: ['id'], context: true })
  })

  it('各种类型转换为 JSON Schema', () => {
    const { properties, required } = tool('types.kinds').inputSchema as unknown as {
      properties: Record<string, unknown>
      required: string[]
    }
    expect(properties).toMatchObject({
      s: { type: 'string' },
      n: { type: 'number' },
      b: { type: 'boolean' },
      lit: { type: 'string', enum: ['a', 'b', 'c'] },
      nums: { type: 'number', enum: [1, 2] },
      color: { type: 'string', enum: ['red', 'blue'] },
      list: {
        type: 'array',
        items: { type: 'object', properties: { x: { type: 'number' } }, required: ['x'] },
      },
      addr: {
        type: 'object',
        properties: {
          city: { type: 'string', description: '城市' },
          zip: { type: 'string' },
          tags: { type: 'array', items: { type: 'string' } },
        },
        required: ['city', 'tags'],
      },
      when: { type: 'string', format: 'date-time' },
      tuple: { type: 'array', prefixItems: [{ type: 'string' }, { type: 'number' }], minItems: 2, maxItems: 2 },
      dict: { type: 'object', properties: {}, additionalProperties: { type: 'number' } },
      nullable: { type: ['string', 'null'] },
      mixed: { anyOf: [{ type: 'string' }, { type: 'number' }] },
      flag: { type: 'boolean' },
    })
    expect(required).toEqual([
      's', 'n', 'b', 'lit', 'nums', 'color', 'list', 'addr', 'when', 'tuple', 'dict', 'nullable', 'mixed',
    ])
    // 递归类型在深度限制处退化为任意值，并给出警告。
    let node = properties.tree as Record<string, any>
    let depth = 0
    while (node?.properties?.child) {
      node = node.properties.child
      depth++
    }
    expect(depth).toBeLessThanOrEqual(5)
    expect(node).toEqual({})
    expect(result.warnings.some((w) => w.includes('tree') && w.includes('嵌套超过'))).toBe(true)
  })

  it('单个 Date / 数组参数不展开', () => {
    expect(tool('types.onlyDate').inputSchema).toEqual({
      type: 'object',
      properties: { at: { type: 'string', format: 'date-time' } },
      required: ['at'],
    })
    expect(tool('types.onlyList').call).toEqual({ mode: 'positional', params: ['ids'], context: false })
  })

  it('无法表示的类型与非法标签报警告并跳过', () => {
    const names = result.tools.map((t) => t.name)
    for (const skipped of [
      'types.fnParam',
      'types.symParam',
      'types.generic',
      'types.withMethod',
      'types.restParam',
      'types.badRisk',
      'types.noDescription',
      'cart.hidden',
      'cart.instance',
    ]) {
      expect(names).not.toContain(skipped)
    }
    const warnings = result.warnings.join('\n')
    expect(warnings).toMatch(/函数 fnParam .*函数类型/)
    expect(warnings).toMatch(/函数 symParam .*Symbol/)
    expect(warnings).toMatch(/函数 generic .*泛型参数/)
    expect(warnings).toMatch(/函数 withMethod .*含方法/)
    expect(warnings).toMatch(/函数 restParam .*剩余参数/)
    expect(warnings).toMatch(/函数 badRisk 的 @risk "dangerous" 不合法/)
    expect(warnings).toMatch(/函数 noDescription 缺少描述/)
    expect(warnings).toMatch(/函数 hidden 未导出/)
    expect(warnings).toMatch(/静态方法 instance 只支持类的静态方法/)
    expect(result.errors).toEqual([])
  })

  it('工具重名时报错', async () => {
    const dup = await makeProject({
      'src/a.ts': '/** @mcp same.name 甲 */\nexport function a() {}\n',
      'src/b.ts': '/** @mcp same.name 乙 */\nexport function b() {}\n',
    })
    expect(scanAnnotations({ root: dup }).errors[0]).toMatch(/注释工具重名：same\.name/)
  })

  it('增量扫描：isRelevant 判断文件变化是否相关', async () => {
    const proj = await makeProject({
      'src/a.ts': "import type { Input } from './types'\n/** @mcp 甲 */\nexport function a(input: Input) { return input }\n",
      'src/types.ts': 'export interface Input { x: string }\n',
      'src/plain.ts': 'export const y = 1\n',
    })
    const scanner = createAnnotationScanner({ root: proj })
    expect(scanner.scan().tools[0]!.inputSchema).toMatchObject({ properties: { x: { type: 'string' } } })
    expect(scanner.isRelevant(join(proj, 'src/types.ts'))).toBe(true)
    expect(scanner.isRelevant(join(proj, 'src/plain.ts'))).toBe(false)
    await writeFile(join(proj, 'src/types.ts'), 'export interface Input { x: number }\n')
    expect(scanner.scan().tools[0]!.inputSchema).toMatchObject({ properties: { x: { type: 'number' } } })
    await writeFile(join(proj, 'src/plain.ts'), '/** @mcp 乙 */\nexport const y = () => 1\n')
    expect(scanner.isRelevant(join(proj, 'src/plain.ts'))).toBe(true)
    expect(scanner.scan().tools.map((t) => t.name).sort()).toEqual(['a.a', 'plain.y'])
  })
})

describe('generateAnnotatedModule', () => {
  it('无工具时生成空的注册函数', () => {
    const code = generateAnnotatedModule([])
    expect(code).toContain('export const annotatedTools = []')
    expect(code).toContain('export function registerAnnotated(registrar)')
  })
})

interface FakeCall {
  name: string
  definition: { description: string; input: unknown; risk?: string; handler: (input: any, ctx: any) => unknown }
}

function fakeRegistrar() {
  const calls: FakeCall[] = []
  const disposed: string[] = []
  return {
    calls,
    disposed,
    tool(name: string, definition: FakeCall['definition']) {
      calls.push({ name, definition })
      return { name, dispose: () => disposed.push(name) }
    },
  }
}

describe('虚拟模块 virtual:app-mcp/annotated', () => {
  let dir: string
  let server: ViteDevServer

  beforeAll(async () => {
    dir = await makeProject({
      'index.html': '<!doctype html><html><body></body></html>',
      'src/cart.ts': CART,
      'src/main.ts': "import { registerAnnotated } from 'virtual:app-mcp/annotated'\nexport { registerAnnotated }\n",
    })
    server = await createServer({
      root: dir,
      configFile: false,
      logLevel: 'silent',
      server: { middlewareMode: true, ws: false, watch: null },
      plugins: [appMcp({ appId: 'shop', name: '示例商城', annotations: true, writeTo: 'app-mcp.json' })],
    })
  })

  afterAll(async () => {
    await server?.close()
  })

  it('registerAnnotated 注册全部工具，handler 按参数映射调用函数，返回统一 dispose', async () => {
    const mod = (await server.ssrLoadModule('virtual:app-mcp/annotated')) as {
      annotatedTools: { name: string; source: string }[]
      registerAnnotated: (r: unknown) => () => void
    }
    expect(mod.annotatedTools.map((t) => t.name)).toEqual([
      'cart.checkout',
      'cart.clear',
      'cart.add',
      'cart.withContext',
      'cart.renamed',
      'cart.count',
    ])
    expect(mod.annotatedTools[0]).not.toHaveProperty('file')

    const registrar = fakeRegistrar()
    const dispose = mod.registerAnnotated(registrar)
    const byName = new Map(registrar.calls.map((c) => [c.name, c.definition]))
    const checkout = byName.get('cart.checkout')!
    expect(checkout).toMatchObject({ description: '结算当前购物车', risk: 'payment', activation: 'foreground', title: '结算' })
    expect(checkout.input).toMatchObject({ required: ['addressId'] })
    await expect(checkout.handler({ addressId: 'A', note: '-n', count: 3 }, {})).resolves.toEqual({ id: 'A-n3', total: 3 })
    await expect(checkout.handler({ addressId: 'B' }, {})).resolves.toEqual({ id: 'B1', total: 1 })
    expect(byName.get('cart.clear')!).not.toHaveProperty('risk')
    expect(byName.get('cart.add')!.handler({ productId: 'p1', quantity: 2 }, {})).toEqual({
      ok: true,
      productId: 'p1',
      quantity: 2,
    })
    expect(byName.get('cart.withContext')!.handler({ id: 'x' }, { callId: 'c1' })).toEqual({ id: 'x', callId: 'c1' })
    expect(byName.get('cart.renamed')!.handler(undefined, {})).toBe('renamed')
    expect(byName.get('cart.count')!.handler({ n: 21 }, {})).toBe(42)

    dispose()
    expect(registrar.disposed.sort()).toEqual([...registrar.calls.map((c) => c.name)].sort())
    dispose()
    expect(registrar.disposed).toHaveLength(registrar.calls.length)
  })

  it('注册中途失败时注销已注册的工具', async () => {
    const mod = (await server.ssrLoadModule('virtual:app-mcp/annotated')) as {
      registerAnnotated: (r: unknown) => () => void
    }
    const disposed: string[] = []
    let n = 0
    const registrar = {
      tool(name: string) {
        if (++n === 3) throw new Error('重复注册')
        return { name, dispose: () => disposed.push(name) }
      },
    }
    expect(() => mod.registerAnnotated(registrar)).toThrow('重复注册')
    expect(disposed).toEqual(['cart.clear', 'cart.checkout'])
  })

  it('源文件中的注释变化后虚拟模块与清单更新', async () => {
    const file = join(dir, 'src/extra.ts')
    await writeFile(file, '/** @mcp 新增的工具 @risk read */\nexport function more(): number { return 1 }\n')
    server.watcher.emit('add', file)
    for (let i = 0; i < 100; i++) {
      const manifest = JSON.parse(await readFile(join(dir, 'app-mcp.json'), 'utf8').catch(() => '{}'))
      if (manifest.tools?.some((t: { name: string }) => t.name === 'extra.more')) break
      await new Promise((r) => setTimeout(r, 50))
    }
    const manifest = JSON.parse(await readFile(join(dir, 'app-mcp.json'), 'utf8'))
    expect(manifest.tools.map((t: { name: string }) => t.name)).toContain('extra.more')
    const mod = (await server.ssrLoadModule('virtual:app-mcp/annotated')) as { annotatedTools: { name: string }[] }
    expect(mod.annotatedTools.map((t) => t.name)).toContain('extra.more')
  })
})

describe('与静态清单合并', () => {
  const STATIC = "export default [{ name: 'catalog.search', description: '搜索商品', risk: 'read' }]\n"

  it('注释工具追加在静态工具之后写入清单', async () => {
    const dir = await makeProject({
      'index.html': '<!doctype html><html><body><script type="module" src="/src/main.ts"></script></body></html>',
      'src/main.ts':
        "import { registerAnnotated } from 'virtual:app-mcp/annotated'\nregisterAnnotated({ tool: (name: string) => ({ name, dispose() {} }) })\n",
      'src/cart.ts': CART,
      'static-tools.ts': STATIC,
    })
    await build({
      root: dir,
      configFile: false,
      logLevel: 'silent',
      plugins: [appMcp({ appId: 'shop', name: '示例商城', staticTools: 'static-tools.ts', annotations: {} })],
    })
    const manifest = JSON.parse(await readFile(join(dir, 'dist/.well-known/app-mcp.json'), 'utf8'))
    expect(manifest.tools.map((t: { name: string }) => t.name)).toEqual([
      'catalog.search',
      'cart.checkout',
      'cart.clear',
      'cart.add',
      'cart.withContext',
      'cart.renamed',
      'cart.count',
    ])
    expect(manifest.tools[1]).toEqual({
      name: 'cart.checkout',
      title: '结算',
      description: '结算当前购物车',
      inputSchema: {
        type: 'object',
        properties: {
          addressId: { type: 'string', description: '收货地址 ID' },
          note: { type: 'string' },
          count: { type: 'number' },
        },
        required: ['addressId'],
      },
      risk: 'payment',
      activation: 'foreground',
    })
  })

  it('与静态工具重名时报错；未启用 annotations 时导入虚拟模块报错', async () => {
    const dir = await makeProject({
      'index.html': '<!doctype html><html><body><script type="module" src="/src/main.ts"></script></body></html>',
      'src/main.ts': "import { registerAnnotated } from 'virtual:app-mcp/annotated'\nconsole.log(registerAnnotated)\n",
      'src/catalog.ts': '/** @mcp 重名 */\nexport function search() {}\n',
      'static-tools.ts': STATIC,
    })
    await expect(
      build({
        root: dir,
        configFile: false,
        logLevel: 'silent',
        plugins: [appMcp({ appId: 'shop', name: 'x', staticTools: 'static-tools.ts', annotations: true })],
      }),
    ).rejects.toThrow(/注释工具 catalog\.search（src\/catalog\.ts 的 search）与静态工具重名/)

    await expect(
      build({
        root: dir,
        configFile: false,
        logLevel: 'silent',
        plugins: [appMcp({ appId: 'shop', name: 'x' })],
      }),
    ).rejects.toThrow(/未启用 annotations/)
  })
})
