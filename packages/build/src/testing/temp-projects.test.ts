import { access, readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { createTempProjects } from './temp-projects'

const exists = (path: string): Promise<boolean> =>
  access(path).then(
    () => true,
    () => false,
  )

describe('createTempProjects', () => {
  it('写入文件（含子目录）', async () => {
    const projects = createTempProjects()
    const dir = await projects.make({ 'a.txt': 'A', 'src/b.ts': 'B' })
    expect(await readFile(join(dir, 'a.txt'), 'utf8')).toBe('A')
    expect(await readFile(join(dir, 'src/b.ts'), 'utf8')).toBe('B')
    await projects.cleanup()
    expect(await exists(dir)).toBe(false)
  })

  // 回归：两个测试文件或同一测试文件的两次并行运行曾共用固定根目录，一方 afterAll 删除整个根目录，
  // 另一方进行中的 vite build 报 UNRESOLVED_ENTRY / ENOENT。
  it('各实例独占根目录，cleanup 不影响其他实例的项目', async () => {
    const first = createTempProjects()
    const second = createTempProjects()
    const kept = await second.make({ 'index.html': '<!doctype html>' })
    const removed = await first.make({ 'index.html': '<!doctype html>' })
    await first.cleanup()
    expect(await exists(removed)).toBe(false)
    expect(await readFile(join(kept, 'index.html'), 'utf8')).toBe('<!doctype html>')
    await second.cleanup()
  })

  it('未创建过项目时 cleanup 什么也不做', async () => {
    await expect(createTempProjects().cleanup()).resolves.toBeUndefined()
  })
})
