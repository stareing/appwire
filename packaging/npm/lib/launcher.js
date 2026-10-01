// @ts-check
/**
 * appwire 命令：找到当前平台的预编译 app-mcp-host 并原样执行（参数透传、退出码 / 信号回传）。
 * 只是包管理器入口，不解析、不转发协议。
 *
 * @invariant 平台包名 = `${主包名}-${process.platform}-${process.arch}`，内含 `bin/app-mcp-host[.exe]`；
 *            平台包由主包 package.json 的 optionalDependencies 引入（发布时由 packaging/scripts/stage-npm.mjs 写入），
 *            运行时不联网下载。
 */

import { spawn } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'

/** crates/host/Cargo.toml 中的 [[bin]] 名。 */
export const HOST_BINARY = 'app-mcp-host'

/** 转发给子进程的信号（子进程与本进程通常同属一个进程组，但被父进程单独 kill 时也要传下去）。 */
const FORWARDED_SIGNALS = /** @type {const} */ (['SIGINT', 'SIGTERM', 'SIGHUP'])

/** 找不到二进制时的分类错误。 */
export class LauncherError extends Error {
  /**
   * @param {'UNSUPPORTED_PLATFORM' | 'PLATFORM_PACKAGE_MISSING'} code
   * @param {string} message
   */
  constructor(code, message) {
    super(message)
    this.name = 'LauncherError'
    this.code = code
  }
}

/**
 * @param {string} mainPackage 主包名
 * @param {string} platform `process.platform`
 * @param {string} arch `process.arch`
 */
export function platformPackageName(mainPackage, platform, arch) {
  return `${mainPackage}-${platform}-${arch}`
}

/** @param {string} platform */
export function hostExecutableName(platform) {
  return platform === 'win32' ? `${HOST_BINARY}.exe` : HOST_BINARY
}

/**
 * @typedef {object} ResolveOptions
 * @property {{ name: string, optionalDependencies?: Record<string, string> }} manifest 主包 package.json
 * @property {string} platform
 * @property {string} arch
 * @property {(request: string) => string} resolve 解析模块路径（通常是主包位置上的 require.resolve）
 */

/**
 * 定位当前平台的 app-mcp-host。
 * @param {ResolveOptions} options
 * @returns {string} 可执行文件绝对路径
 * @error LauncherError UNSUPPORTED_PLATFORM：主包未声明该平台包；PLATFORM_PACKAGE_MISSING：声明了但未安装
 */
export function resolveHostBinary({ manifest, platform, arch, resolve }) {
  const pkg = platformPackageName(manifest.name, platform, arch)
  const supported = Object.keys(manifest.optionalDependencies ?? {})
  if (!supported.includes(pkg)) {
    const list = supported.map((name) => name.slice(manifest.name.length + 1)).join(', ') || '（无）'
    throw new LauncherError(
      'UNSUPPORTED_PLATFORM',
      `${manifest.name} 没有 ${platform}-${arch} 的预编译 ${HOST_BINARY}（支持：${list}）。` +
        '可从源码构建：cargo build --release -p app-mcp-host（https://github.com/stareing/appwire）。',
    )
  }
  let packageJson
  try {
    packageJson = resolve(`${pkg}/package.json`)
  } catch {
    throw new LauncherError(
      'PLATFORM_PACKAGE_MISSING',
      `未安装平台包 ${pkg}（${manifest.name} 的可选依赖）。` +
        '常见原因：安装时用了 --no-optional / --omit=optional，或 node_modules 从其他平台拷贝而来。' +
        `请在本机重新安装 ${manifest.name}（不要跳过可选依赖）。`,
    )
  }
  return join(dirname(packageJson), 'bin', hostExecutableName(platform))
}

/**
 * 执行二进制并让当前进程以同样的方式结束。
 * @param {string} binary
 * @param {readonly string[]} args
 * @side-effect 继承 stdio；结束时调用 process.exit 或以同一信号结束本进程
 */
export function execHost(binary, args) {
  const child = spawn(binary, args, { stdio: 'inherit', windowsHide: false })
  /** @type {Map<NodeJS.Signals, () => void>} */
  const handlers = new Map()
  for (const signal of FORWARDED_SIGNALS) {
    const handler = () => {
      child.kill(signal)
    }
    handlers.set(signal, handler)
    process.on(signal, handler)
  }
  const detach = () => {
    for (const [signal, handler] of handlers) process.off(signal, handler)
  }
  child.on('error', (error) => {
    detach()
    console.error(`[appwire] 无法启动 ${binary}：${error.message}`)
    process.exit(1)
  })
  child.on('exit', (code, signal) => {
    detach()
    if (signal) {
      process.kill(process.pid, signal)
      return
    }
    process.exit(code ?? 1)
  })
}

/**
 * 命令入口。
 * @param {string} launcherUrl bin 脚本的 import.meta.url（主包内）
 * @param {readonly string[]} args
 */
export function main(launcherUrl, args) {
  const require = createRequire(launcherUrl)
  const manifestPath = require.resolve('../package.json')
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
  let binary
  try {
    binary = resolveHostBinary({
      manifest,
      platform: process.platform,
      arch: process.arch,
      resolve: (request) => require.resolve(request),
    })
  } catch (error) {
    if (!(error instanceof LauncherError)) throw error
    console.error(`[appwire] ${error.message}`)
    process.exit(1)
  }
  execHost(binary, args)
}
