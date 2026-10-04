// 用 OpenHarmony SDK 构建工具（ets-loader）自带的意图装饰器校验规则检查 harmony-insight-intents 的输出：
// 对 insight_intent.json 的 insightIntentsSrcEntry 中每个文件，取 @InsightIntentEntry 的参数对象，
// 按 ets-loader `lib/userIntents_parser/intentType.js` 的 intentEntryInfoChecker 检查必填字段、允许字段与
// 各字段校验函数（`parameters` 用与构建时相同的 ajv 编译）；再用 ets-loader `lib/userIntents_parser/parseUserIntents.js` 的
// 解析器对每个执行器做构建时的同一套检查：装饰器参数（含标准意图 `schema` 读取 SDK 的标准意图定义并覆盖名称与参数）、
// 类属性与 `parameters` 的类型一致性（`object` 属性须为 @InsightIntentEntity 类）；标准意图另查 SDK 中有对应的 schema 文件
// （构建工具在文件不存在时不报错、按自定义意图处理）。@InsightIntentEntity 类检查装饰器参数与 `implements insightIntent.IntentEntity`。
//
// @why 类型检查器的编译选项取 ets-loader 的 tsconfig.json（构建时的意图解析不开 strict，可选属性的类型不含 undefined）。
//
// 类属性检查目前只用于标准意图执行器：自定义意图执行器的整数 / 枚举 / 对象属性与 `parameters` 不一致（构建时报 10110009，
// 待修），加 `--custom-classes` 时同样检查。
//
// 用法：node harmony-intents-check.cjs <生成目录（含 ets/ 与 resources/）> [--custom-classes]
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
const { intentEntryInfoChecker, IntentEntityInfoChecker } = require(path.join(loader, 'lib/userIntents_parser/intentType.js'));
const parser = require(path.join(loader, 'lib/userIntents_parser/parseUserIntents.js')).default;
const preDefine = require(path.join(loader, 'lib/pre_define.js'));
const SCHEMA_DIR = path.join(loader, 'insight_intents/schema');

const args = process.argv.slice(2);
const checkCustomClasses = args.includes('--custom-classes');
const root = path.resolve(args.find((a) => !a.startsWith('--')) || '.');
const config = JSON.parse(fs.readFileSync(path.join(root, 'resources/base/profile/insight_intent.json'), 'utf8'));
const entries = config.insightIntentsSrcEntry || [];
let failures = 0;
const names = new Set();

const cfgText = fs.readFileSync(path.join(loader, 'tsconfig.json'), 'utf8');
const options = ts.convertCompilerOptionsFromJson(ts.parseConfigFileTextToJson('tsconfig.json', cfgText).config.compilerOptions, SDK).options;
Object.assign(options, {
  noEmit: true,
  skipLibCheck: true,
  baseUrl: SDK,
  paths: { '@kit.*': ['kits/@kit.*'], '@ohos.*': ['api/@ohos.*'], '@arkts.*': ['arkts/@arkts.*'] },
});
const files = entries.map((e) => path.join(root, e.srcEntry)).filter((f) => fs.existsSync(f));
const program = ts.createProgram({ rootNames: files, options });
parser.clear();
parser.checker = program.getTypeChecker();

/** 用 ets-loader 的解析器检查一个装饰器与其类；返回解析后的装饰器信息。 */
function loaderCheck(srcEntry, decorator, classNode, infoChecker, decoratorType, checkClass) {
  parser.transformLog = [];
  parser.currentNode = decorator;
  const info = {};
  parser.analyzeDecoratorArgs(decorator.expression.arguments, info, infoChecker);
  if (checkClass && decoratorType === preDefine.COMPONENT_USER_INTENTS_DECORATOR_ENTRY) {
    const props = parser.parseClassNode(classNode, info.intentName, decoratorType);
    parser.schemaValidateSync(props, info.parameters);
  }
  for (const log of parser.transformLog) fail(srcEntry, `ets-loader ${log.code}：${log.message}`);
  return info;
}

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
  const source = program.getSourceFile(file);
  let found = 0;
  const visit = (node) => {
    if (ts.isDecorator(node) && ts.isCallExpression(node.expression) && node.expression.expression.getText() === 'InsightIntentEntity') {
      loaderCheck(srcEntry, node, node.parent, IntentEntityInfoChecker, preDefine.COMPONENT_USER_INTENTS_DECORATOR_ENTITY, false);
      const heritage = (node.parent.heritageClauses || []).map((h) => h.getText());
      if (!heritage.some((h) => /implements\b.*\binsightIntent\.IntentEntity\b/.test(h))) {
        fail(srcEntry, '@InsightIntentEntity 类须 implements insightIntent.IntentEntity');
      }
    }
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
      const schema = props.get('schema');
      if (schema) {
        const file = path.join(SCHEMA_DIR, `${schema.text}_${props.get('intentVersion')?.text}.json`);
        if (!fs.existsSync(file)) fail(srcEntry, `SDK 中没有标准意图 ${schema.text} 的该版本定义：${path.basename(file)}`);
      }
      const info = loaderCheck(srcEntry, node, node.parent, intentEntryInfoChecker, preDefine.COMPONENT_USER_INTENTS_DECORATOR_ENTRY, Boolean(schema) || checkCustomClasses);
      if (schema && info.intentName !== schema.text) fail(srcEntry, `标准意图 ${schema.text} 未被构建工具识别`);
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
