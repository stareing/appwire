#!/usr/bin/env node
// 构建 WASM 核心并生成 JS 绑定到 packages/web/src/wasm。
//
// 步骤：cargo build（wasm-release）→ wasm-bindgen --target web → 可选 wasm-opt → 打印体积。
// 参数：
//   --opt-only         跳过 cargo 与 wasm-bindgen，只对 src/wasm 中现有的 .wasm 运行 wasm-opt 并报告体积
// 环境变量：
//   CARGO_TARGET_DIR   cargo 输出目录（默认 <仓库>/target）
//   APP_MCP_WASM_DEBUG 设为 1 时启用 debug feature（console_error_panic_hook）
//   WASM_BINDGEN       wasm-bindgen 可执行文件路径
//   WASM_OPT           wasm-opt 可执行文件路径；设为 0 跳过（缺省在 PATH、~/.local/bin、~/.local/binaryen-*/bin 中查找）
//   WASM_OPT_FLAGS     覆盖默认的优化参数（空格分隔）
//
// 默认参数说明：-Oz / -Os 会内联大量小函数，原始体积变小但 gzip 后反而变大（实测 +6%）；
// 关闭内联（-aimfs/-fimfs/-ocimfs 0）的 -Os 在 gzip 后最小。优化后 gzip 体积变大时保留未优化的文件。
import { execFileSync, spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { gzipSync, constants as zlibConstants } from 'node:zlib'

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = resolve(here, '..')
const repoRoot = resolve(pkgDir, '../..')
const outDir = join(pkgDir, 'src', 'wasm')
const crate = 'app-mcp-wasm'
const artifact = 'app_mcp_wasm'
const profile = 'wasm-release'
const target = 'wasm32-unknown-unknown'
const BUDGET_GZIP = 150 * 1024

function run(cmd, args, opts = {}) {
  console.log(`$ ${cmd} ${args.join(' ')}`)
  execFileSync(cmd, args, { stdio: 'inherit', cwd: repoRoot, ...opts })
}

/** 在 PATH 中查找命令，找不到返回 null。 */
function which(cmd) {
  const r = spawnSync(process.platform === 'win32' ? 'where' : 'which', [cmd], { encoding: 'utf8' })
  return r.status === 0 ? r.stdout.split(/\r?\n/)[0].trim() || null : null
}

function findWasmBindgen() {
  if (process.env.WASM_BINDGEN) return process.env.WASM_BINDGEN
  const fromPath = which('wasm-bindgen')
  if (fromPath) return fromPath
  const home = process.env.HOME || homedir()
  const fallback = join(home, '.cargo', 'bin', process.platform === 'win32' ? 'wasm-bindgen.exe' : 'wasm-bindgen')
  if (existsSync(fallback)) return fallback
  throw new Error('找不到 wasm-bindgen，请运行 cargo install wasm-bindgen-cli --version 0.2.129')
}

function findWasmOpt() {
  if (process.env.WASM_OPT === '0') return null
  if (process.env.WASM_OPT) return process.env.WASM_OPT
  const fromPath = which('wasm-opt')
  if (fromPath) return fromPath
  const home = process.env.HOME || homedir()
  const exe = process.platform === 'win32' ? 'wasm-opt.exe' : 'wasm-opt'
  const candidates = [join(home, '.local', 'bin', exe)]
  try {
    for (const dir of readdirSync(join(home, '.local')).filter((d) => d.startsWith('binaryen-')).sort().reverse()) {
      candidates.push(join(home, '.local', dir, 'bin', exe))
    }
  } catch {
    // ~/.local 不存在
  }
  return candidates.find((c) => existsSync(c)) ?? null
}

const DEFAULT_OPT_FLAGS = [
  '-Os',
  // 关闭内联：内联减少原始体积，但重复代码让 gzip 变差
  '-aimfs', '0', '-fimfs', '0', '-ocimfs', '0',
  '--converge',
  '--strip-debug',
  '--strip-producers',
]
const FEATURE_FLAGS = ['--enable-bulk-memory', '--enable-nontrapping-float-to-int', '--enable-sign-ext', '--enable-mutable-globals']

function gzipSize(bytes) {
  return gzipSync(bytes, { level: zlibConstants.Z_BEST_COMPRESSION }).length
}

function kb(n) {
  return `${(n / 1024).toFixed(1)} KB`
}

const optOnly = process.argv.includes('--opt-only')
const wasmOut = join(outDir, `${artifact}_bg.wasm`)

if (!optOnly) {
  // 1. cargo build
  const cargoArgs = ['build', '--target', target, '--profile', profile, '-p', crate]
  if (process.env.APP_MCP_WASM_DEBUG === '1') cargoArgs.push('--features', 'debug')
  run('cargo', cargoArgs)

  const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repoRoot, 'target')
  const wasmIn = join(targetDir, target, profile, `${artifact}.wasm`)
  if (!existsSync(wasmIn)) throw new Error(`未找到构建产物：${wasmIn}`)

  // 2. wasm-bindgen
  mkdirSync(outDir, { recursive: true })
  run(findWasmBindgen(), ['--target', 'web', '--out-dir', outDir, '--out-name', artifact, wasmIn])
} else if (!existsSync(wasmOut)) {
  throw new Error(`--opt-only：未找到 ${wasmOut}，请先完整运行一次 build:wasm`)
}

// 3. wasm-opt（可选）
const original = readFileSync(wasmOut)
const before = { raw: original.length, gzip: gzipSize(original) }
const wasmOpt = findWasmOpt()
let optimized = false
if (wasmOpt) {
  const flags = process.env.WASM_OPT_FLAGS ? process.env.WASM_OPT_FLAGS.split(/\s+/).filter(Boolean) : DEFAULT_OPT_FLAGS
  const tmp = `${wasmOut}.opt`
  run(wasmOpt, [...flags, ...FEATURE_FLAGS, '-o', tmp, wasmOut])
  const after = readFileSync(tmp)
  if (gzipSize(after) < before.gzip) {
    copyFileSync(tmp, wasmOut)
    optimized = true
  } else {
    console.log('wasm-opt 后 gzip 体积没有变小，保留未优化的文件。')
  }
  rmSync(tmp, { force: true })
} else if (process.env.WASM_OPT !== '0') {
  console.log('提示：未找到 wasm-opt（binaryen），跳过体积优化。安装到 PATH 或 ~/.local/bin 后可进一步减小体积。')
}

// 4. 体积报告
const bytes = readFileSync(wasmOut)
const gzip = gzipSize(bytes)
const glue = readFileSync(join(outDir, `${artifact}.js`))
const glueGzip = gzipSync(glue, { level: zlibConstants.Z_BEST_COMPRESSION }).length
console.log('')
console.log(`wasm 原始大小：${kb(bytes.length)}${optimized ? `（wasm-opt 前 ${kb(before.raw)}）` : ''}`)
console.log(
  `wasm gzip 大小：${kb(gzip)}${optimized ? `（wasm-opt 前 ${kb(before.gzip)}）` : ''}（预算 ${kb(BUDGET_GZIP)}）${gzip < BUDGET_GZIP ? '，达标' : '，超出预算！'}`,
)
console.log(`JS 绑定：${kb(glue.length)}，gzip ${kb(glueGzip)}`)
if (gzip >= BUDGET_GZIP) process.exitCode = 1
