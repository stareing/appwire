/**
 * 按需加载 WASM 核心。
 *
 * `./wasm/` 由 `pnpm --filter @app-mcp/web build:wasm` 生成（不入库），`tsup` 构建时原样复制到 `dist/wasm/`。
 *
 * 胶水 JS 与 .wasm 都用 `new URL('./wasm/...', import.meta.url)` 定位：
 * - Vite（dev 与 build）、webpack 5 等打包器能识别这种写法，把两个文件作为资源输出；
 * - 生成物不存在时不会在转换阶段报错（不像静态分析的 `import('./wasm/...')`），
 *   因此未构建 WASM 时依赖本包的测试照常运行，只有真正加载时才失败。
 * wasm-bindgen `--target web` 的胶水代码没有外部 import，可以直接作为 ES 模块加载。
 */

import type { CoreClient, CoreConfig, CoreFactory } from './core'

interface WasmBindings {
  default: (init?: { module_or_path: string | URL }) => Promise<unknown>
  WasmClient: new (config: CoreConfig) => CoreClient
  parseWakeToken?: (args: string) => string | undefined
}

export async function loadWasmCore(wasmUrl?: string | URL): Promise<CoreFactory> {
  const glue = new URL('./wasm/app_mcp_wasm.js', import.meta.url)
  const wasm = wasmUrl ?? new URL('./wasm/app_mcp_wasm_bg.wasm', import.meta.url)
  const mod = (await import(/* @vite-ignore */ glue.href)) as WasmBindings
  await mod.default({ module_or_path: wasm })
  const factory: CoreFactory = (config) => new mod.WasmClient(config)
  if (typeof mod.parseWakeToken === 'function') {
    const parse = mod.parseWakeToken
    factory.parseWakeToken = (args) => parse(args) ?? undefined
  }
  return factory
}
