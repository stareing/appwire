import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
    environment: 'node',
    globalSetup: ['./src/global-setup.ts'],
    // 每个测试文件各自启动 Host / Vite / Chromium；串行运行，避免共用 Vite 依赖缓存与端口竞争。
    fileParallelism: false,
    testTimeout: 60_000,
    hookTimeout: 180_000,
  },
})
