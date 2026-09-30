import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/main.ts', 'src/preload.ts', 'src/renderer.ts'],
  format: ['esm'],
  dts: true,
  clean: true,
  sourcemap: true,
  target: 'es2022',
  // electron 不被 import（只用最小接口类型）；兄弟包与 zod 保持外部引用。
  external: ['electron', '@app-mcp/node', '@app-mcp/web', 'zod'],
})
