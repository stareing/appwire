import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/index.ts', 'src/zustand.ts', 'src/redux.ts', 'src/pinia.ts'],
  format: ['esm'],
  dts: true,
  clean: true,
  sourcemap: true,
  target: 'es2022',
  platform: 'browser',
  // 状态库只以结构类型描述，运行时不引入；@app-mcp/web 为依赖。
  external: ['@app-mcp/web', 'zustand', 'redux', '@reduxjs/toolkit', 'pinia', 'vue'],
})
