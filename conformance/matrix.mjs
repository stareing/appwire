#!/usr/bin/env node
// 汇总各 runner 写下的报告（target/conformance/<sdk>/<case>.json），打印 SDK × 用例矩阵。
// 用法：node conformance/matrix.mjs [--dir target/conformance] [--markdown] [--strict]
//   --strict：有 fail（或 xpass：已登记的偏差不再出现，应删除登记）时退出码 1。
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const dirFlag = args.indexOf('--dir');
const dir = dirFlag >= 0 ? args[dirFlag + 1] : join(here, '..', 'target', 'conformance');
const markdown = args.includes('--markdown');
const strict = args.includes('--strict');

const cases = readdirSync(join(here, 'cases'))
  .filter((f) => f.endsWith('.json'))
  .map((f) => f.slice(0, -5))
  .sort();
const sdks = existsSync(dir)
  ? readdirSync(dir, { withFileTypes: true }).filter((d) => d.isDirectory()).map((d) => d.name).sort()
  : [];
if (sdks.length === 0) {
  console.error(`没有报告：${dir}（先运行各 SDK 的 runner，见 conformance/README.md）`);
  process.exit(1);
}

const LABEL = { pass: 'pass', fail: 'FAIL', xfail: 'xfail', xpass: 'XPASS', skip: 'skip' };
const report = (sdk, id) => {
  const file = join(dir, sdk, `${id}.json`);
  if (!existsSync(file)) return null;
  try {
    return JSON.parse(readFileSync(file, 'utf8'));
  } catch {
    return { status: 'fail', failures: ['报告无法解析'] };
  }
};

const rows = cases.map((id) => [id, ...sdks.map((sdk) => {
  const r = report(sdk, id);
  return r ? (LABEL[r.status] ?? r.status) : '-';
})]);
const header = ['case', ...sdks];

if (markdown) {
  console.log(`| ${header.join(' | ')} |`);
  console.log(`|${header.map(() => '---').join('|')}|`);
  for (const r of rows) console.log(`| ${r.join(' | ')} |`);
} else {
  const widths = header.map((h, i) => Math.max(h.length, ...rows.map((r) => r[i].length)));
  const line = (r) => r.map((c, i) => c.padEnd(widths[i])).join('  ');
  console.log(line(header));
  for (const r of rows) console.log(line(r));
}

let bad = 0;
for (const sdk of sdks) {
  for (const id of cases) {
    const r = report(sdk, id);
    if (!r) continue;
    if (r.status === 'fail' || r.status === 'xpass') {
      bad += 1;
      console.log(`\n[${sdk}] ${id}: ${r.status}`);
      for (const f of r.failures ?? []) console.log(`  - ${f}`);
    } else if (r.status === 'xfail' || r.status === 'skip') {
      console.log(`\n[${sdk}] ${id}: ${r.status}（${r.divergence ?? r.reason ?? ''}）`);
    }
  }
}
process.exit(strict && bad > 0 ? 1 : 0);
