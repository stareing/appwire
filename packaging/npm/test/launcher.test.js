// @ts-check
import { spawnSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { LauncherError, hostExecutableName, platformPackageName, resolveHostBinary } from '../lib/launcher.js'
import { loadPlatforms, stageNpm } from '../../scripts/stage-npm.mjs'

const MAIN = 'appwire-cli'
const FAKE_EXIT_CODE = 7

/** @param {() => unknown} fn */
function launcherErrorCode(fn) {
  try {
    fn()
  } catch (error) {
    if (error instanceof LauncherError) return error.code
    throw error
  }
  return undefined
}

describe('resolveHostBinary', () => {
  const manifest = {
    name: MAIN,
    optionalDependencies: { 'appwire-cli-linux-x64': '0.1.0', 'appwire-cli-win32-x64': '0.1.0' },
  }

  it('解析到平台包内的 bin/app-mcp-host', () => {
    const path = resolveHostBinary({
      manifest,
      platform: 'linux',
      arch: 'x64',
      resolve: (request) => `/nm/${request}`,
    })
    expect(path).toBe(join('/nm/appwire-cli-linux-x64', 'bin', 'app-mcp-host'))
  })

  it('Windows 加 .exe', () => {
    const path = resolveHostBinary({ manifest, platform: 'win32', arch: 'x64', resolve: (r) => `C:/nm/${r}` })
    expect(path.endsWith(join('bin', 'app-mcp-host.exe'))).toBe(true)
  })

  it('主包未声明的平台：UNSUPPORTED_PLATFORM，并列出支持的平台', () => {
    const run = () => resolveHostBinary({ manifest, platform: 'freebsd', arch: 'x64', resolve: (r) => r })
    expect(launcherErrorCode(run)).toBe('UNSUPPORTED_PLATFORM')
    expect(run).toThrow(/linux-x64, win32-x64/)
  })

  it('声明了但未安装：PLATFORM_PACKAGE_MISSING，并说明可选依赖被跳过', () => {
    const run = () =>
      resolveHostBinary({
        manifest,
        platform: 'linux',
        arch: 'x64',
        resolve: () => {
          throw new Error('Cannot find module')
        },
      })
    expect(launcherErrorCode(run)).toBe('PLATFORM_PACKAGE_MISSING')
    expect(run).toThrow(/appwire-cli-linux-x64/)
    expect(run).toThrow(/--omit=optional/)
  })

  it('命名规则', () => {
    expect(platformPackageName(MAIN, 'darwin', 'arm64')).toBe('appwire-cli-darwin-arm64')
    expect(hostExecutableName('linux')).toBe('app-mcp-host')
  })
})

describe('stageNpm', () => {
  /** @type {string} */
  let work
  beforeAll(() => {
    work = mkdtempSync(join(tmpdir(), 'appwire-stage-'))
  })
  afterAll(() => rmSync(work, { recursive: true, force: true }))

  it('缺少构建产物时失败并列出缺失文件', () => {
    const bins = join(work, 'empty-bins')
    mkdirSync(bins, { recursive: true })
    expect(() => stageNpm({ version: '1.2.3', binsDir: bins, outDir: join(work, 'out-empty') })).toThrow(/app-mcp-hostw\.exe/)
  })

  it('拒绝不合法的版本号', () => {
    expect(() => stageNpm({ version: 'v1.2.3', binsDir: work, outDir: join(work, 'x') })).toThrow(/版本号/)
  })

  it('每个平台一个包 + 主包，主包 optionalDependencies 与平台表一致', () => {
    const bins = writeFakeBins(join(work, 'bins'), 'echo fake')
    const out = join(work, 'out')
    const dirs = stageNpm({ version: '1.2.3', binsDir: bins, outDir: out })
    const platforms = loadPlatforms()
    expect(dirs).toHaveLength(platforms.length + 1)

    const main = JSON.parse(readFileSync(join(out, MAIN, 'package.json'), 'utf8'))
    expect(main.version).toBe('1.2.3')
    expect(main.scripts).toBeUndefined()
    expect(Object.keys(main.optionalDependencies).sort()).toEqual(
      platforms.map((p) => `${MAIN}-${p.id}`).sort(),
    )
    expect(Object.values(main.optionalDependencies)).toEqual(platforms.map(() => '1.2.3'))
    expect(existsSync(join(out, MAIN, 'test'))).toBe(false)

    for (const p of platforms) {
      const dir = join(out, `${MAIN}-${p.id}`)
      const manifest = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
      expect(manifest).toMatchObject({ version: '1.2.3', os: [p.npm.os], cpu: [p.npm.cpu] })
      for (const b of p.binaries) {
        const file = join(dir, 'bin', b + p.exeSuffix)
        expect(existsSync(file)).toBe(true)
        if (process.platform !== 'win32') expect(statSync(file).mode & 0o111).not.toBe(0)
      }
    }
  })
})

// 端到端：按 npm 安装后的布局放好主包与平台包，运行 bin/appwire.js。假二进制是 shell 脚本，仅在 POSIX 上运行。
describe.skipIf(process.platform === 'win32')('appwire 命令', () => {
  /** @type {string} */
  let work
  /** @type {string} */
  let out
  /** @type {string} */
  let nodeModules
  const platformPkg = `${MAIN}-${process.platform}-${process.arch}`

  beforeAll(() => {
    work = mkdtempSync(join(tmpdir(), 'appwire-run-'))
    const script = [
      'if [ "$1" = "--signal" ]; then kill -TERM $$; fi',
      'for a in "$@"; do printf "<%s>" "$a"; done',
      `exit ${FAKE_EXIT_CODE}`,
    ].join('\n')
    const bins = writeFakeBins(join(work, 'bins'), script)
    out = join(work, 'out')
    stageNpm({ version: '0.0.1', binsDir: bins, outDir: out })
    nodeModules = join(work, 'node_modules')
    cpSync(join(out, MAIN), join(nodeModules, MAIN), { recursive: true })
    cpSync(join(out, platformPkg), join(nodeModules, platformPkg), { recursive: true })
  })
  afterAll(() => rmSync(work, { recursive: true, force: true }))

  /**
   * @param {string[]} args
   * @param {string} [modules]
   */
  const run = (args, modules = nodeModules) =>
    spawnSync(process.execPath, [join(modules, MAIN, 'bin', 'appwire.js'), ...args], { encoding: 'utf8' })

  it('参数原样透传、退出码回传', () => {
    const result = run(['serve', '--listen', '127.0.0.1:0', 'a b', ''])
    expect(result.stdout).toBe('<serve><--listen><127.0.0.1:0><a b><>')
    expect(result.status).toBe(FAKE_EXIT_CODE)
  })

  it('子进程被信号结束时，本进程以同一信号结束', () => {
    const result = run(['--signal'])
    expect(result.signal).toBe('SIGTERM')
  })

  it('平台包缺失时退出码 1 并说明', () => {
    // 独立临时目录：不能在 work 之下，否则向上解析会找到 work/node_modules 中的平台包。
    const isolated = mkdtempSync(join(tmpdir(), 'appwire-missing-'))
    try {
      const onlyMain = join(isolated, 'node_modules')
      cpSync(join(out, MAIN), join(onlyMain, MAIN), { recursive: true })
      const result = run(['status'], onlyMain)
      expect(result.status).toBe(1)
      expect(result.stderr).toContain(platformPkg)
    } finally {
      rmSync(isolated, { recursive: true, force: true })
    }
  })
})

/**
 * 为平台表中每个平台写假二进制。
 * @param {string} dir
 * @param {string} body shell 脚本正文
 */
function writeFakeBins(dir, body) {
  for (const p of loadPlatforms()) {
    mkdirSync(join(dir, p.id), { recursive: true })
    for (const b of p.binaries) writeFileSync(join(dir, p.id, b + p.exeSuffix), `#!/bin/sh\n${body}\n`, { mode: 0o755 })
  }
  return dir
}
