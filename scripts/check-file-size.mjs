#!/usr/bin/env node
// 源文件行数上限检查（CLAUDE.md「约定」；规范 M-01：按职责拆分，禁止长期把多类职责堆在单个大文件里）。
//
// - 上限：源码 SOURCE_LIMIT 行，测试 TEST_LIMIT 行（测试文件按路径判定，见 isTest）。
// - 棘轮：已超限的文件记在 scripts/file-size-baseline.json（路径 → 允许的最大行数），只许变小、不许变大；
//   新文件或不在基线中的文件超限即失败。
// - `--update`：只收紧基线（变小的文件降到当前行数、降到上限以内的移出基线），从不放宽或加入新文件。
// - `--init`：基线不存在时按当前超限文件建立（只用一次；已存在时拒绝）。
//
// 用法：node scripts/check-file-size.mjs [--update | --init]（或 pnpm check:size）。退出码：0 通过，1 有超限。

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SOURCE_LIMIT = 800;
const TEST_LIMIT = 1200;

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const BASELINE = join(ROOT, "scripts", "file-size-baseline.json");

const EXTENSIONS = /\.(rs|ts|tsx|mts|js|mjs|cjs|kt|kts|swift|py|cs|dart|c|cc|cpp|h|hpp|ets)$/;
/** 生成的代码与第三方产物不计（由工具产出，拆分无意义）。 */
const EXCLUDED = [
  /(^|\/)(node_modules|dist|build|target|generated|gen)\//,
  /\.d\.ts$/,
  /(^|\/)uniffi\/app_mcp(_hub)?\.(kt|swift|py)$/,
  /(^|\/)app_mcp(_hub)?(FFI)?\.(swift|h)$/,
];

/** 测试文件：tests/、test/、__tests__/ 目录下，或文件名带 test / Test / tests。 */
function isTest(path) {
  return /(^|\/)(tests?|__tests__|androidTest|test_support|testing)\//.test(path)
    || /(\.|_|-)(test|spec)s?\.[a-z]+$/.test(path)
    || /(^|\/)tests?\.rs$/.test(path)
    || /Tests?\.(kt|swift|cs|cpp)$/.test(path)
    || /(^|\/)test_[^/]+\.py$/.test(path);
}

function limitOf(path) {
  return isTest(path) ? TEST_LIMIT : SOURCE_LIMIT;
}

/** 已跟踪与未忽略的新文件（新文件在 git add 之前同样检查）。 */
function trackedFiles() {
  const out = execFileSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], { cwd: ROOT, encoding: "utf8", maxBuffer: 64 << 20 });
  return out.split("\0").filter((p) => p && EXTENSIONS.test(p) && !EXCLUDED.some((re) => re.test(p)));
}

function countLines(path) {
  const full = join(ROOT, path);
  if (!existsSync(full)) return 0; // 已删除但尚未提交
  const text = readFileSync(full, "utf8");
  if (text.length === 0) return 0;
  return text.split("\n").length - (text.endsWith("\n") ? 1 : 0);
}

function loadBaseline() {
  return existsSync(BASELINE) ? JSON.parse(readFileSync(BASELINE, "utf8")) : {};
}

function init() {
  if (existsSync(BASELINE)) {
    console.error(`${BASELINE} 已存在；基线只能经 --update 收紧`);
    process.exit(1);
  }
  const entries = trackedFiles().sort().map((p) => [p, countLines(p)]).filter(([p, n]) => n > limitOf(p));
  writeFileSync(BASELINE, JSON.stringify(Object.fromEntries(entries), null, 2) + "\n");
  console.log(`基线已建立：${entries.length} 个超限文件`);
}

function main() {
  if (process.argv.includes("--init")) return init();
  const update = process.argv.includes("--update");
  const baseline = loadBaseline();
  const failures = [];
  const tightened = {};
  for (const path of trackedFiles().sort()) {
    const lines = countLines(path);
    const limit = limitOf(path);
    const allowed = baseline[path];
    if (allowed === undefined) {
      if (lines > limit) failures.push(`${path}: ${lines} 行，超过上限 ${limit}（按职责拆成目录模块）`);
      continue;
    }
    if (lines > allowed) {
      failures.push(`${path}: ${lines} 行，超过基线 ${allowed}（已超限的文件只许变小；新代码放到新模块，或先拆分）`);
      tightened[path] = allowed;
    } else if (lines > limit) {
      tightened[path] = lines;
    }
  }
  if (update) {
    const before = Object.keys(baseline).length;
    writeFileSync(BASELINE, JSON.stringify(tightened, null, 2) + "\n");
    console.log(`基线已收紧：${before} → ${Object.keys(tightened).length} 个文件`);
  }
  if (failures.length > 0) {
    console.error(`行数检查失败（源码上限 ${SOURCE_LIMIT}、测试上限 ${TEST_LIMIT}）：`);
    for (const f of failures) console.error(`  ${f}`);
    process.exit(1);
  }
  const slack = Object.entries(baseline).filter(([p, n]) => (tightened[p] ?? 0) < n);
  if (!update && slack.length > 0) {
    console.log(`通过；${slack.length} 个基线文件已变小，可运行 --update 收紧基线`);
  } else if (!update) {
    console.log("通过");
  }
}

main();
