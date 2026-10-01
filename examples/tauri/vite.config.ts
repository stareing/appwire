import { defaultClientConditions, defineConfig } from 'vite'

export default defineConfig({
  resolve: {
    // 工作区包（@app-mcp/dom 等）经 "@app-mcp/source" 导出条件直接使用源码，无需先构建。
    conditions: ['@app-mcp/source', ...defaultClientConditions],
  },
})
