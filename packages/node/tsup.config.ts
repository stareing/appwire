import { defineConfig } from 'tsup'

// 原生模块（native/*.node）不参与打包：dist/index.js 运行时按平台从 ../native/ 加载。
export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm'],
  dts: true,
  clean: true,
  sourcemap: true,
  platform: 'node',
  target: 'node20',
  external: ['zod'],
})
