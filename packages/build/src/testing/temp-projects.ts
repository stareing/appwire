import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 测试用临时 Vite 项目。
 *
 * @why 临时目录放在包的 node_modules 下，夹具中的 `import 'zod'` 才能被解析。
 * @invariant 每个实例（每个测试文件的每次运行）独占一个 mkdtemp 生成的根目录，cleanup 只删除它；
 *   同一包的测试被并行运行多次（多个 vitest 进程、pnpm -r 与手动运行重叠）时互不清理对方的项目。
 */
export interface TempProjects {
  /** @output 新建项目目录（绝对路径），写入 files（键为相对路径） */
  make(files: Record<string, string>): Promise<string>
  /** @side-effect 删除本实例的根目录；未创建过项目时什么也不做 */
  cleanup(): Promise<void>
}

const PKG_DIR = fileURLToPath(new URL('../..', import.meta.url))
const TEMP_PARENT = join(PKG_DIR, 'node_modules', '.tmp-tests')

export function createTempProjects(): TempProjects {
  let root: Promise<string> | undefined
  const ensureRoot = (): Promise<string> => {
    root ??= mkdir(TEMP_PARENT, { recursive: true }).then(() => mkdtemp(join(TEMP_PARENT, 'run-')))
    return root
  }
  return {
    async make(files) {
      const dir = await mkdtemp(join(await ensureRoot(), 'app-mcp-'))
      for (const [name, content] of Object.entries(files)) {
        await mkdir(join(dir, name, '..'), { recursive: true })
        await writeFile(join(dir, name), content)
      }
      return dir
    },
    async cleanup() {
      if (root === undefined) return
      await rm(await root, { recursive: true, force: true })
    },
  }
}
