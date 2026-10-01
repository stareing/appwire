// 在 Node 上运行 ArkTS 封装层（AppMcp.ets / Errors.ets / Types.ets / Harmony.ets）的单元测试：
// 用 OpenHarmony SDK 的 ohos-typescript 把 .ets 转译为 CommonJS（不依赖 Kit 与 .so，原生模块用假实现注入），
// 再以 node:test 执行 tests/*.test.cjs。
//
// 用法：node tests/run.cjs；环境变量 OHOS_SDK_ETS 同 scripts/arkts-check.cjs。
// @compat Harmony.ets 的 Kit / hilog / .so 由 fake-native.cjs 的假实现替换，只验证默认值与前后台转发；
//   真实的前后台回调时序需在设备上验证。
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

const PKG = path.resolve(__dirname, '..');
const SDK = process.env.OHOS_SDK_ETS || path.join(os.homedir(), 'sdk/ohos/sdk/ets');
const tsPath = path.join(SDK, 'build-tools/ets-loader/node_modules/typescript');
if (!fs.existsSync(tsPath)) {
  console.error(`找不到 OpenHarmony SDK 的 ohos-typescript：${tsPath}（设置 OHOS_SDK_ETS）`);
  process.exit(2);
}
const ts = require(tsPath);

const out = fs.mkdtempSync(path.join(os.tmpdir(), 'app-mcp-harmony-test-'));
const sources = ['AppMcp', 'Errors', 'Types', 'Harmony'];
for (const name of sources) {
  const text = fs.readFileSync(path.join(PKG, 'src/main/ets', `${name}.ets`), 'utf8');
  const js = ts.transpileModule(text, {
    fileName: `${name}.ts`,
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2021 },
  }).outputText;
  fs.writeFileSync(path.join(out, `${name}.js`), js);
}

const tests = fs
  .readdirSync(__dirname)
  .filter((f) => f.endsWith('.test.cjs'))
  .map((f) => path.join(__dirname, f));
const result = spawnSync(process.execPath, ['--test', ...tests], {
  stdio: 'inherit',
  env: { ...process.env, APP_MCP_HARMONY_BUILD: out },
});
fs.rmSync(out, { recursive: true, force: true });
process.exit(result.status === null ? 1 : result.status);
