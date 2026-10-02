import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/index.ts', 'src/define.ts', 'src/annotations.ts', 'src/routes.ts'],
  format: ['esm'],
  dts: true,
  clean: true,
  sourcemap: true,
  platform: 'node',
  target: 'node20',
  external: ['vite', '@app-mcp/web', 'typescript'],
})
