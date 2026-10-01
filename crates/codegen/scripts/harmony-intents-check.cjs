// 用 OpenHarmony SDK 构建工具（ets-loader）自带的意图装饰器校验规则检查 harmony-insight-intents 的输出：
// 对 insight_intent.json 的 insightIntentsSrcEntry 中每个文件，取 @InsightIntentEntry 的参数对象，
// 按 ets-loader `lib/userIntents_parser/intentType.js` 的 intentEntryInfoChecker 检查必填字段、允许字段与
// 各字段校验函数（`parameters` 用与构建时相同的 ajv 编译）。
//
// 用法：node harmony-intents-check.cjs <生成目录（含 ets/ 与 resources/）>
// 环境变量：OHOS_SDK_ETS = <SDK>/ets（缺省 ~/sdk/ohos/sdk/ets）
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const SDK = process.env.OHOS_SDK_ETS || path.join(os.homedir(), 'sdk/ohos/sdk/ets');
const loader = path.join(SDK, 'build-tools/ets-loader');
if (!fs.existsSync(loader)) {
  console.error(`找不到 OpenHarmony SDK 的 ets-loader：${loader}（设置 OHOS_SDK_ETS）`);
  process.exit(2);
}
const ts = require(path.join(loader, 'node_modules/typescript'));
const { intentEntryInfoChecker } = require(path.join(loader, 'lib/userIntents_parser/intentType.js'));

const root = path.resolve(process.argv[2] || '.');
const config = JSON.parse(fs.readFileSync(path.join(root, 'resources/base/profile/insight_intent.json'), 'utf8'));
const entries = config.insightIntentsSrcEntry || [];
let failures = 0;
const names = new Set();

function fail(file, message) {
  failures++;
  console.log(`FAIL ${file}: ${message}`);
}

for (const { srcEntry } of entries) {
  const file = path.join(root, srcEntry);
  if (!fs.existsSync(file)) {
    fail(srcEntry, '文件不存在');
    continue;
  }
  const source = ts.createSourceFile(file, fs.readFileSync(file, 'utf8'), ts.ScriptTarget.ES2021, true, ts.ScriptKind.ETS);
  let found = 0;
  const visit = (node) => {
    if (ts.isDecorator(node) && ts.isCallExpression(node.expression) && node.expression.expression.getText() === 'InsightIntentEntry') {
      found++;
      const arg = node.expression.arguments[0];
      if (!arg || !ts.isObjectLiteralExpression(arg)) {
        fail(srcEntry, '装饰器参数不是对象字面量');
        return;
      }
      const props = new Map();
      for (const p of arg.properties) {
        if (ts.isPropertyAssignment(p)) props.set(p.name.getText().replace(/^["']|["']$/g, ''), p.initializer);
      }
      for (const key of intentEntryInfoChecker.requiredFields) {
        if (!props.has(key)) fail(srcEntry, `缺少必填字段 ${key}`);
      }
      for (const [key, value] of props) {
        if (!intentEntryInfoChecker.allowFields.has(key)) fail(srcEntry, `不允许的字段 ${key}`);
        const validate = intentEntryInfoChecker.paramValidators[key];
        if (validate && !validate(value)) fail(srcEntry, `字段 ${key} 校验失败：${value.getText().slice(0, 80)}`);
      }
      const intentName = props.get('intentName');
      if (intentName) {
        const n = intentName.text;
        if (!/^[A-Z][A-Za-z0-9]*$/.test(n)) fail(srcEntry, `intentName 须首字母大写、只含字母数字：${n}`);
        if (names.has(n)) fail(srcEntry, `intentName 重复：${n}`);
        names.add(n);
      }
      const version = props.get('intentVersion');
      if (version && !/^\d+\.\d+\.\d+$/.test(version.text)) fail(srcEntry, `intentVersion 不是三段版本号：${version.text}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  if (found !== 1) fail(srcEntry, `应有且只有一个 @InsightIntentEntry，实际 ${found}`);
}
console.log(`已检查 ${entries.length} 个意图执行器：${failures === 0 ? '全部通过' : `${failures} 处问题`}`);
process.exit(failures === 0 ? 0 : 1);
