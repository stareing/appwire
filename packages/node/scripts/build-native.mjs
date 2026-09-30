#!/usr/bin/env node
// 构建 napi-rs 原生模块（不依赖 @napi-rs/cli）：
//   cargo build -p app-mcp-node [--release]
// 然后把 cdylib 复制为 packages/node/native/app_mcp_node.<platform>-<arch>.node。
//
// 用法：node scripts/build-native.mjs [--release]
// 遵循 CARGO_TARGET_DIR；未设置时使用仓库根目录下的 target/。

import { spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = resolve(here, '..')
const repoRoot = resolve(pkgDir, '..', '..')
const release = process.argv.includes('--release')

const args = ['build', '-p', 'app-mcp-node']
if (release) args.push('--release')
console.error(`[build-native] cargo ${args.join(' ')}`)
const result = spawnSync('cargo', args, { cwd: repoRoot, stdio: 'inherit' })
if (result.error) {
  console.error(`[build-native] 无法运行 cargo：${result.error.message}`)
  process.exit(1)
}
if (result.status !== 0) process.exit(result.status ?? 1)

const libName = {
  linux: 'libapp_mcp_node.so',
  android: 'libapp_mcp_node.so',
  freebsd: 'libapp_mcp_node.so',
  darwin: 'libapp_mcp_node.dylib',
  win32: 'app_mcp_node.dll',
}[process.platform]
if (!libName) {
  console.error(`[build-native] 不支持的平台：${process.platform}`)
  process.exit(1)
}

const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(repoRoot, process.env.CARGO_TARGET_DIR)
  : join(repoRoot, 'target')
const source = join(targetDir, release ? 'release' : 'debug', libName)
if (!existsSync(source)) {
  console.error(`[build-native] 找不到构建产物：${source}`)
  process.exit(1)
}

const outDir = join(pkgDir, 'native')
mkdirSync(outDir, { recursive: true })
const dest = join(outDir, `app_mcp_node.${process.platform}-${process.arch}.node`)
copyFileSync(source, dest)
console.error(`[build-native] ${source} -> ${dest}`)
