import { cpSync, existsSync, mkdirSync, readdirSync } from 'node:fs'
import { defineConfig } from 'tsup'

// WASM 胶水代码与 .wasm 不参与打包：dist/index.js 通过 `new URL('./wasm/...', import.meta.url)` 引用，
// 构建完成后把 src/wasm/ 原样复制到 dist/wasm/，由使用方的打包器（Vite 等）作为资源处理。
export default defineConfig({
  entry: { index: 'src/index.ts', 'webmcp/index': 'src/webmcp/index.ts' },
  format: ['esm'],
  dts: true,
  clean: true,
  sourcemap: true,
  target: 'es2022',
  platform: 'browser',
  external: ['zod'],
  onSuccess: async () => {
    const from = 'src/wasm'
    if (!existsSync(from)) {
      console.warn('警告：src/wasm 不存在，dist 中不含 WASM 核心。请先运行 pnpm --filter @app-mcp/web build:wasm')
      return
    }
    mkdirSync('dist/wasm', { recursive: true })
    for (const f of readdirSync(from)) {
      if (/\.(js|wasm)$/.test(f)) cpSync(`${from}/${f}`, `dist/wasm/${f}`)
    }
  },
})
