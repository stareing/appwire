// 用 OpenHarmony SDK 自带的 ohos-typescript 对 .ets 文件做类型检查 + ArkTS 语法检查（ArkTSLinter 1.1，
// 与 DevEco / hvigor 编译时使用的检查器相同）。不生成产物。
//
// 用法：node scripts/arkts-check.cjs [文件或目录 ...]（缺省检查本包 Index.ets 与 src/main/ets）
// 环境变量：OHOS_SDK_ETS = <SDK>/ets 目录（缺省 ~/sdk/ohos/sdk/ets）。
//
// @why SDK 的 global.d.ts 以 `export declare` 声明 console / setTimeout 等全局量（由 ets-loader 注入），
//   这里在临时目录生成去掉 `export` 的副本作为全局声明。
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const PKG = path.resolve(__dirname, '..');
const SDK = process.env.OHOS_SDK_ETS || path.join(os.homedir(), 'sdk/ohos/sdk/ets');
const loader = path.join(SDK, 'build-tools/ets-loader');
if (!fs.existsSync(loader)) {
  console.error(`找不到 OpenHarmony SDK 的 ets 组件：${SDK}（设置 OHOS_SDK_ETS）`);
  process.exit(2);
}
const ts = require(path.join(loader, 'node_modules/typescript'));

function collect(target, out) {
  const stat = fs.statSync(target);
  if (stat.isDirectory()) {
    for (const name of fs.readdirSync(target)) collect(path.join(target, name), out);
  } else if (target.endsWith('.ets')) {
    out.push(path.resolve(target));
  }
}

const args = process.argv.slice(2);
const targets = args.length > 0 ? args : [path.join(PKG, 'Index.ets'), path.join(PKG, 'src/main/ets')];
const files = [];
for (const t of targets) collect(t, files);

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'arkts-check-'));
const globalSrc = fs.readFileSync(path.join(SDK, 'api/@internal/full/global.d.ts'), 'utf8');
const globalDts = path.join(tmp, 'global.d.ts');
fs.writeFileSync(globalDts, globalSrc.replace(/^export declare/gm, 'declare'));

const cfgText = fs.readFileSync(path.join(loader, 'tsconfig.json'), 'utf8');
const cfg = ts.parseConfigFileTextToJson('tsconfig.json', cfgText).config;
const options = ts.convertCompilerOptionsFromJson(cfg.compilerOptions, SDK).options;
Object.assign(options, {
  noEmit: true,
  strict: true,
  noImplicitAny: true,
  skipLibCheck: true,
  baseUrl: SDK,
  packageManagerType: 'ohpm',
  // 与 hvigor 一致：开启 ArkTS 检查时才按 .d.ts 校验 .so 模块的导入（否则一律视为 any）。
  needDoArkTsLinter: true,
  isCompatibleVersion: false,
  paths: {
    '@kit.*': ['kits/@kit.*'],
    '@ohos.*': ['api/@ohos.*'],
    '@arkts.*': ['arkts/@arkts.*'],
    'libapp_mcp_harmony.so': [path.join(PKG, 'src/main/cpp/types/libapp_mcp_harmony/index.d.ts')],
    '@app-mcp/harmony': [path.join(PKG, 'Index.ets')],
  },
});
const components = fs
  .readdirSync(path.join(SDK, 'component'))
  .filter((f) => f.endsWith('.d.ts'))
  .map((f) => path.join(SDK, 'component', f));

const host = ts.createIncrementalCompilerHost(options);
const program = ts.createIncrementalProgram({ rootNames: [...files, ...components, globalDts], options, host });
const wanted = new Set(files);
const results = [];
for (const d of ts.getPreEmitDiagnostics(program.getProgram())) {
  if (d.file && wanted.has(path.resolve(d.file.fileName))) results.push(['tsc', d]);
}
for (const d of ts.ArkTSLinter_1_1.runArkTSLinter(program, undefined, undefined, 'ArkTS_1_1')) {
  if (d.file && wanted.has(path.resolve(d.file.fileName))) results.push(['arkts', d]);
}
let errors = 0;
for (const [kind, d] of results) {
  const isError = d.category === ts.DiagnosticCategory.Error;
  if (isError) errors++;
  const pos = d.file.getLineAndCharacterOfPosition(d.start || 0);
  const msg = ts.flattenDiagnosticMessageText(d.messageText, '\n');
  console.log(`${kind} ${isError ? 'error' : 'warning'} ${path.relative(process.cwd(), d.file.fileName)}:${pos.line + 1}:${pos.character + 1} ${msg}`);
}
fs.rmSync(tmp, { recursive: true, force: true });
console.log(`已检查 ${files.length} 个 .ets 文件：${errors} 个错误，${results.length - errors} 个警告`);
process.exit(errors > 0 ? 1 : 0);
