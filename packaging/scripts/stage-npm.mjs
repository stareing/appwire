#!/usr/bin/env node
// @ts-check
/**
 * 生成待发布的 npm 包：每个平台一个二进制包 + 主包（写入 optionalDependencies 与版本号）。
 *
 * 用法：node packaging/scripts/stage-npm.mjs --version <x.y.z> --bins <dir> --out <dir>
 * @input  <bins>/<平台 id>/<二进制>[.exe]（平台与二进制列表见 packaging/platforms.json；发布流水线的构建产物布局）
 * @output <out>/<主包名>/、<out>/<主包名>-<平台 id>/，各自可直接 `npm publish`
 * @error  缺少任一平台的任一二进制即失败（主包只能引用实际发布的平台包）
 */

import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseArgs } from 'node:util'
import { platformPackageName } from '../npm/lib/launcher.js'

const packagingDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repoRoot = resolve(packagingDir, '..')
const launcherDir = join(packagingDir, 'npm')
const LICENSE_FILES = ['LICENSE-MIT', 'LICENSE-APACHE']
const MAIN_FILES = ['bin', 'lib', 'README.md']
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/

/**
 * @typedef {{ id: string, binaries: string[], exeSuffix: string, npm: { os: string, cpu: string } }} Platform
 */

/** @returns {Platform[]} */
export function loadPlatforms() {
  return JSON.parse(readFileSync(join(packagingDir, 'platforms.json'), 'utf8')).platforms
}

/**
 * @param {{ version: string, binsDir: string, outDir: string, platforms?: Platform[] }} options
 * @returns {string[]} 生成的包目录（平台包在前、主包最后，即发布顺序）
 */
export function stageNpm({ version, binsDir, outDir, platforms = loadPlatforms() }) {
  if (!SEMVER.test(version)) throw new Error(`版本号不合法：${version}`)
  const launcherManifest = JSON.parse(readFileSync(join(launcherDir, 'package.json'), 'utf8'))
  const mainName = launcherManifest.name
  const missing = platforms.flatMap((p) =>
    p.binaries.map((b) => join(binsDir, p.id, b + p.exeSuffix)).filter((f) => !existsSync(f)),
  )
  if (missing.length > 0) throw new Error(`缺少构建产物：\n  ${missing.join('\n  ')}`)

  rmSync(outDir, { recursive: true, force: true })
  const staged = platforms.map((platform) => stagePlatformPackage(platform, mainName, launcherManifest, version, binsDir, outDir))
  staged.push(stageMainPackage(launcherManifest, platforms, version, outDir))
  return staged
}

/**
 * @param {Platform} platform
 * @param {string} mainName
 * @param {Record<string, unknown>} launcherManifest
 * @param {string} version
 * @param {string} binsDir
 * @param {string} outDir
 */
function stagePlatformPackage(platform, mainName, launcherManifest, version, binsDir, outDir) {
  const name = platformPackageName(mainName, platform.npm.os, platform.npm.cpu)
  const dir = join(outDir, name)
  mkdirSync(join(dir, 'bin'), { recursive: true })
  for (const binary of platform.binaries) {
    const file = binary + platform.exeSuffix
    copyFileSync(join(binsDir, platform.id, file), join(dir, 'bin', file))
    chmodSync(join(dir, 'bin', file), 0o755)
  }
  copyLicenses(dir)
  const manifest = {
    name,
    version,
    description: `Prebuilt AppWire Host (app-mcp-host) for ${platform.id}; installed automatically by ${mainName}`,
    homepage: launcherManifest.homepage,
    repository: launcherManifest.repository,
    bugs: launcherManifest.bugs,
    license: launcherManifest.license,
    os: [platform.npm.os],
    cpu: [platform.npm.cpu],
    files: ['bin', ...LICENSE_FILES],
    preferUnplugged: true,
  }
  writeFileSync(join(dir, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`)
  writeFileSync(
    join(dir, 'README.md'),
    `# ${name}\n\nThe prebuilt \`app-mcp-host\` binary of [AppWire](https://github.com/stareing/appwire) for \`${platform.id}\`.\n` +
      `Do not install this package directly; install \`${mainName}\` instead.\n`,
  )
  return dir
}

/**
 * @param {Record<string, unknown>} launcherManifest
 * @param {Platform[]} platforms
 * @param {string} version
 * @param {string} outDir
 */
function stageMainPackage(launcherManifest, platforms, version, outDir) {
  const mainName = /** @type {string} */ (launcherManifest.name)
  const dir = join(outDir, mainName)
  mkdirSync(dir, { recursive: true })
  for (const entry of MAIN_FILES) copyTree(join(launcherDir, entry), join(dir, entry))
  chmodSync(join(dir, 'bin', 'appwire.js'), 0o755)
  copyLicenses(dir)
  const { scripts: _scripts, devDependencies: _dev, ...published } = launcherManifest
  const optionalDependencies = Object.fromEntries(
    platforms.map((p) => [platformPackageName(mainName, p.npm.os, p.npm.cpu), version]),
  )
  const manifest = { ...published, version, files: [...MAIN_FILES, ...LICENSE_FILES], optionalDependencies }
  writeFileSync(join(dir, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`)
  return dir
}

/** @param {string} dir */
function copyLicenses(dir) {
  for (const file of LICENSE_FILES) copyFileSync(join(repoRoot, file), join(dir, file))
}

/**
 * 复制文件或目录（跳过测试文件）。
 * @param {string} from
 * @param {string} to
 */
function copyTree(from, to) {
  rmSync(to, { recursive: true, force: true })
  cpSync(from, to, { recursive: true, filter: (src) => !src.endsWith('.test.js') })
}

const isMain = process.argv[1] !== undefined && import.meta.url === pathToFileURL(resolve(process.argv[1])).href
if (isMain) {
  const { values } = parseArgs({
    options: {
      version: { type: 'string' },
      bins: { type: 'string' },
      out: { type: 'string' },
    },
  })
  if (!values.version || !values.bins || !values.out) {
    console.error('用法：node packaging/scripts/stage-npm.mjs --version <x.y.z> --bins <dir> --out <dir>')
    process.exit(2)
  }
  try {
    for (const dir of stageNpm({ version: values.version, binsDir: resolve(values.bins), outDir: resolve(values.out) })) {
      console.log(dir)
    }
  } catch (error) {
    console.error(`[stage-npm] ${error instanceof Error ? error.message : String(error)}`)
    process.exit(1)
  }
}
