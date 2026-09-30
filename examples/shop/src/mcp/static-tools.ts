/**
 * 静态工具定义（不含 handler）。
 *
 * - 构建时由 @app-mcp/build 插件加载，写入 app-mcp.json（App 未打开时模型也能看到这些工具）；
 * - 运行时 App.tsx 导入同一份定义注册带 handler 的工具，保证两者一致。
 *
 * 注意：本模块会在 Node 中执行（构建时），只能导入 zod 与 @app-mcp/build/define 这类无副作用的模块。
 */
import { defineStaticTool, defineStaticTools } from '@app-mcp/build/define'
import { z } from 'zod'

export const catalogSearch = defineStaticTool({
  name: 'catalog.search',
  title: '搜索商品',
  description: '按关键词（商品名或分类）搜索商品，返回商品 ID、名称、价格；关键词为空时返回全部商品',
  input: z.object({
    keyword: z.string().optional().describe('商品名或分类，如“耳机”“家居”'),
  }),
  risk: 'read',
  activation: 'headless',
})

export default defineStaticTools([catalogSearch])
