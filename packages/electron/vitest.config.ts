import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  resolve: {
    alias: {
      // @app-mcp/node 的包入口是 dist（供 Node / Electron 主进程直接加载），测试直接用源码，无需先构建。
      '@app-mcp/node': fileURLToPath(new URL('../node/src/index.ts', import.meta.url)),
    },
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    testTimeout: 30_000,
  },
})
