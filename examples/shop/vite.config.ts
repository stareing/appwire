import { appMcp } from '@app-mcp/build'
import react from '@vitejs/plugin-react'
import { defaultClientConditions, defineConfig } from 'vite'
import { OVERVIEW_FILE, OVERVIEW_LOCALE, OVERVIEW_SUMMARY } from './src/mcp/overview.ts'

export default defineConfig({
  resolve: {
    // 工作区包（@app-mcp/react 等）经 "@app-mcp/source" 导出条件直接使用源码，无需先构建。
    conditions: ['@app-mcp/source', ...defaultClientConditions],
  },
  plugins: [
    react(),
    appMcp({
      appId: 'shop',
      name: '示例商城',
      version: '0.1.0',
      description: 'app-mcp 演示页面：待办、商品、购物车与订单',
      overview: { summary: OVERVIEW_SUMMARY, file: OVERVIEW_FILE, locale: OVERVIEW_LOCALE },
      launch: { web: 'http://localhost:5173/' },
      staticTools: './src/mcp/static-tools.ts',
      // 扫描 src 下带 @mcp 注释的导出函数（如 src/mcp/annotated.ts），合并进清单并生成 virtual:app-mcp/annotated。
      annotations: true,
      // 页面目录（清单 pages）：扫描路由表（带 id 的路由是页面），购物车页的工具输入是 zod schema，在 pages 模块中显式声明。
      routes: { file: 'src/routes.tsx', router: 'react-router' },
      pages: './src/mcp/pages.ts',
      // dev / build 时额外写一份，供 Host 用 `--manifest examples/shop/app-mcp.json` 加载。
      writeTo: 'app-mcp.json',
    }),
  ],
})
