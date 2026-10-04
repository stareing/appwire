//! 鸿蒙（HarmonyOS NEXT / OpenHarmony）意图框架：装饰器方式的 InsightIntent（API 20+）+ ArkTS 类型化接口。
//!
//! 输出（路径相对于模块的 `src/main/`）：
//! - `ets/appmcp/<Module>Tools.ets`：参数类型（interface / 字符串字面量联合）、`<Module>ToolHandlers`、
//!   `register<Module>Tools(registrar, handlers)`（用 `@app-mcp/harmony` 把清单中的工具注册为 MCP 工具）；
//! - `ets/appmcp/<Module>InsightIntents.ets`：`<Module>IntentRuntime`（handlers 注入点、结果码、结果转换）；
//! - `ets/insightintents/<Module><Tool>Intent.ets`：每个工具一个 `@InsightIntentEntry` 执行器
//!   （`InsightIntentEntryExecutor<string>`，结果为 handler 返回值的 JSON 文本）；
//! - `resources/base/profile/insight_intent.json`：`insightIntentsSrcEntry` 列出各执行器文件。
//! - `--standard-intents` 时另有鸿蒙标准意图执行器与媒体实体解析接口（见 [`standard`]）。
//!
//! 映射：意图参数由系统入口按 `parameters`（JSON Schema）赋值给执行器的同名属性，因此属性名就是 JSON 属性名；
//! 含非标识符属性名（如 `is-urgent`）或与执行器基类成员同名的工具不生成意图（给出警告），仍可作为 MCP 工具使用。
//! 构建工具按 `parameters` 校验执行器属性类型（10110009），顶层属性因此改变表示、执行时转换回工具参数类型
//! （见 `executor/convert.rs`）：整数为 `number`、枚举为 `string`、对象为 `@InsightIntentEntity` 类、字典与原始 JSON 为 JSON 文本；
//! 对象参数的属性名不是标识符或为 `entityId` 时同样不生成意图。
//! `parameters` 由类型模型重新生成（不含 `format`、`$ref`、组合关键字），保证能被构建工具的 ajv 编译。
//! 需要用户确认的风险等级（destructive / payment / os-sensitive）与 `activation: foreground` 使用前台执行模式，
//! 其余使用后台执行模式（系统经 Call 调用拉起 UIAbility，不显示界面）。
//!
//! 依据（2026-10，OpenHarmony SDK 6.0.0.47 / API 20 声明与 docs 仓库）：
//! - `@ohos.app.ability.InsightIntentDecorator.d.ts`（`InsightIntentEntry`、`EntryIntentDecoratorInfo`）；
//! - `@ohos.app.ability.InsightIntentEntryExecutor.d.ts`、`@ohos.app.ability.insightIntent.d.ts`（`ExecuteMode`、`IntentResult`）；
//! - ets-loader `lib/userIntents_parser/intentType.js`（必填字段、`parameters` 须为对象字面量且可被 ajv 编译）；
//! - docs `application-models/insight-intent-decorator-development.md`（`insight_intent.json` 的 `insightIntentsSrcEntry`）。

use std::collections::BTreeSet;

use app_mcp_protocol::Activation;
use serde_json::{Map, Value, json};

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang};
use crate::schema::{Field, Model, ObjectDecl, ToolModel, Ty, TypeDecl, Warning, field_doc};
use crate::targets::{decl_doc, file, needs_confirmation, risk_name, tool_doc};

mod entity;
mod executor;
pub mod standard;

pub use executor::intent_parameters;
use executor::{entity_unsupported, executor_file};

/// 意图版本（`intentVersion`，三段数字）。
pub const INTENT_VERSION: &str = "1.0.0";
/// 缺省意图垂域：实用工具垂域（标准意图规范中的 `ToolsDomain`）。
pub const DEFAULT_DOMAIN: &str = "ToolsDomain";
/// 缺省绑定的 UIAbility（DevEco 模板的入口 Ability 名）。
pub const DEFAULT_ABILITY: &str = "EntryAbility";
/// ArkTS SDK 的包名（sdks/harmony）。
pub const SDK_PACKAGE: &str = "@app-mcp/harmony";

/// `InsightIntentEntryExecutor` 的成员；参数与之同名时无法作为执行器属性。
const EXECUTOR_MEMBERS: [&str; 6] = [
    "executeMode",
    "context",
    "windowStage",
    "uiExtensionSession",
    "onExecute",
    "constructor",
];

pub fn generate(
    model: &Model,
    domain: &str,
    ability: &str,
    standard_intents: bool,
    warnings: &mut Vec<Warning>,
) -> Vec<GeneratedFile> {
    let m = &model.module;
    let supported: Vec<&ToolModel> = model
        .tools
        .iter()
        .filter(|tool| match unsupported_reason(model, tool) {
            None => true,
            Some(reason) => {
                warnings.push(Warning {
                    tool: tool.info.name.clone(),
                    path: String::new(),
                    message: format!(
                        "harmony-insight-intents：{reason}，未生成意图，仍可作为 MCP 工具调用"
                    ),
                });
                false
            }
        })
        .collect();

    let mut files = vec![
        file(format!("ets/appmcp/{m}Tools.ets"), tools_file(model)),
        file(
            format!("ets/appmcp/{m}InsightIntents.ets"),
            runtime_file(model),
        ),
    ];
    for tool in &supported {
        files.push(file(
            format!("ets/insightintents/{}.ets", executor_name(model, tool)),
            executor_file(model, tool, domain, ability),
        ));
    }
    let mut src_entries: Vec<String> = supported
        .iter()
        .map(|t| format!("./ets/insightintents/{}.ets", executor_name(model, t)))
        .collect();
    if standard_intents {
        let (standard_files, entries) = standard::generate(model, ability, warnings);
        files.extend(standard_files);
        src_entries.extend(entries);
    }
    files.push(file(
        "resources/base/profile/insight_intent.json",
        insight_intent_json(&src_entries),
    ));
    files
}

/// 意图名：`<Module><Tool>`（首字母大写、只含字母数字）。
pub fn intent_name(model: &Model, tool: &ToolModel) -> String {
    format!("{}{}", model.module, tool.pascal)
}

/// 执行器类名 / 文件名：`<Module><Tool>Intent`。
pub fn executor_name(model: &Model, tool: &ToolModel) -> String {
    format!("{}Intent", intent_name(model, tool))
}

/// 执行模式：需要确认或声明前台激活的工具在前台执行，其余在后台执行。
pub fn execute_mode(tool: &ToolModel) -> &'static str {
    let foreground = needs_confirmation(tool.info.risk)
        || matches!(tool.info.activation, Some(Activation::Foreground));
    if foreground {
        "insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND"
    } else {
        "insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND"
    }
}

fn unsupported_reason(model: &Model, tool: &ToolModel) -> Option<String> {
    let params = model.params(tool);
    if let Some(f) = params
        .fields
        .iter()
        .find(|f| !ident::is_plain_identifier(&f.json_name))
    {
        return Some(format!(
            "参数名 `{}` 不是标识符，无法作为意图执行器属性",
            f.json_name
        ));
    }
    if let Some(f) = params
        .fields
        .iter()
        .find(|f| EXECUTOR_MEMBERS.contains(&f.json_name.as_str()))
    {
        return Some(format!(
            "参数名 `{}` 与 InsightIntentEntryExecutor 的成员同名",
            f.json_name
        ));
    }
    params.fields.iter().find_map(|f| entity_unsupported(model, f))
}

// ---------------------------------------------------------------------------
// ArkTS 类型
// ---------------------------------------------------------------------------

/// 字段类型的 ArkTS 写法。原始 JSON 为 `Object | null`。
pub fn ty(model: &Model, t: &Ty) -> String {
    match t {
        Ty::String => "string".into(),
        Ty::Integer | Ty::Number => "number".into(),
        Ty::Boolean => "boolean".into(),
        Ty::Enum(id) | Ty::Object(id) => model.decl(*id).name().to_string(),
        Ty::Array(inner) => match inner.as_ref() {
            Ty::Json => "(Object | null)[]".into(),
            other => format!("{}[]", ty(model, other)),
        },
        Ty::Map(inner) => format!("Record<string, {}>", ty(model, inner)),
        Ty::Json => "Object | null".into(),
    }
}

/// 字段的完整类型（含 `| null`）。
fn field_ty(model: &Model, f: &Field) -> String {
    let t = ty(model, &f.ty);
    if f.nullable && !matches!(f.ty, Ty::Json) {
        format!("{t} | null")
    } else {
        t
    }
}

fn property_key(name: &str) -> String {
    if ident::is_plain_identifier(name) {
        name.to_string()
    } else {
        string_literal(Lang::TypeScript, name)
    }
}

/// handler 的返回类型。
const RESULT_TY: &str = "Object | null";
const HANDLER_RETURN: &str = "Object | null | Promise<Object | null>";

fn tools_file(model: &Model) -> String {
    let m = &model.module;
    let mut c = Code::new("  ");
    let mut header = header_lines(model, "harmony-insight-intents");
    header.push(String::new());
    header.push(format!(
        "注册为 MCP 工具：register{m}Tools(appMcp, handlers)；意图执行器使用 {m}IntentRuntime.handlers（见 {m}InsightIntents.ets）。"
    ));
    c.comment("// ", &header);
    c.blank();
    c.line(format!(
        "import {{ Registrar, ToolHandle }} from {};",
        string_literal(Lang::TypeScript, SDK_PACKAGE)
    ));
    c.blank();

    for decl in &model.types {
        match decl {
            TypeDecl::Enum(e) => {
                c.block_doc(&decl_doc(e.description.as_deref()));
                let values: Vec<String> = e
                    .values
                    .iter()
                    .map(|v| string_literal(Lang::TypeScript, v))
                    .collect();
                c.line(format!("export type {} = {};", e.name, values.join(" | ")));
                c.blank();
            }
            TypeDecl::Object(o) => {
                c.block_doc(&decl_doc(o.description.as_deref()));
                if o.fields.is_empty() {
                    c.line(format!("export interface {} {{}}", o.name));
                } else {
                    c.open(format!("export interface {} {{", o.name));
                    for f in &o.fields {
                        c.block_doc(&field_doc(f));
                        let q = if f.required { "" } else { "?" };
                        c.line(format!(
                            "{}{q}: {};",
                            property_key(&f.json_name),
                            field_ty(model, f)
                        ));
                    }
                    c.close("}");
                }
                c.blank();
            }
        }
    }

    c.block_doc(&[
        format!(
            "{} 的工具实现。MCP 调用时参数已由 Host 按 inputSchema 校验；",
            model.app_name
        ),
        "意图调用时参数由系统入口按意图 parameters 赋值。返回值序列化为 JSON。".to_string(),
    ]);
    c.open(format!("export interface {m}ToolHandlers {{"));
    for tool in &model.tools {
        c.block_doc(&tool_doc(tool));
        c.line(format!(
            "{}(params: {}): {HANDLER_RETURN};",
            tool.camel,
            model.params(tool).name
        ));
    }
    c.close("}");
    c.blank();

    c.block_doc(&[
        "把清单中的全部工具注册到 @app-mcp/harmony 的客户端或 scope，调用转给 handlers。"
            .to_string(),
        "@error 重名或 schema 非法时抛出原生错误（code 为 DUPLICATE_NAME / INVALID_SCHEMA）。"
            .to_string(),
    ]);
    c.open(format!(
        "export function register{m}Tools(registrar: Registrar, handlers: {m}ToolHandlers): ToolHandle[] {{"
    ));
    c.open("return [");
    for tool in &model.tools {
        let params = &model.params(tool).name;
        let schema = serde_json::to_string(&tool.info.input_schema).unwrap_or_else(|_| "{}".into());
        c.open(format!(
            "registrar.tool<{params}, {RESULT_TY}>({}, {{",
            string_literal(Lang::TypeScript, &tool.info.name)
        ));
        c.line(format!(
            "description: {},",
            string_literal(Lang::TypeScript, &tool.info.description)
        ));
        if let Some(title) = &tool.info.title {
            c.line(format!(
                "title: {},",
                string_literal(Lang::TypeScript, title)
            ));
        }
        c.line(format!(
            "inputSchema: {},",
            string_literal(Lang::TypeScript, &schema)
        ));
        c.line(format!("risk: \"{}\",", risk_name(tool.info.risk)));
        if let Some(a) = tool.info.activation {
            c.line(format!("activation: \"{}\",", activation_name(a)));
        }
        c.line(format!(
            "handler: (input: {params}): {HANDLER_RETURN} => handlers.{}(input),",
            tool.camel
        ));
        c.close("}),");
    }
    c.close("];");
    c.close("}");
    c.finish()
}

fn activation_name(a: Activation) -> &'static str {
    match a {
        Activation::Headless => "headless",
        Activation::Background => "background",
        Activation::Foreground => "foreground",
    }
}

// ---------------------------------------------------------------------------
// 运行时
// ---------------------------------------------------------------------------

fn runtime_file(model: &Model) -> String {
    let m = &model.module;
    let mut c = Code::new("  ");
    let mut header = header_lines(model, "harmony-insight-intents");
    header.extend([
        String::new(),
        "意图执行器（ets/insightintents/*.ets）调用 {M}ToolHandlers 的入口。".replace("{M}", m),
        format!("在 AbilityStage.onCreate 中设置 `{m}IntentRuntime.handlers = <实现 {m}ToolHandlers 的对象>`"),
        "（意图在后台执行时系统经 Call 调用拉起 UIAbility，不经过页面；可与 MCP 共用同一个实现）。".to_string(),
    ]);
    c.comment("// ", &header);
    c.blank();
    c.line("import { insightIntent } from '@kit.AbilityKit';");
    c.line(format!(
        "import {{ ToolCallError, ToolResult }} from {};",
        string_literal(Lang::TypeScript, SDK_PACKAGE)
    ));
    c.line(format!("import {{ {m}ToolHandlers }} from './{m}Tools';"));
    c.blank();
    c.line("/** 意图执行结果码（IntentResult.code）。失败时 result 为 {\"kind\",\"message\"} 的 JSON 文本。 */");
    c.open(format!("export enum {m}IntentCode {{"));
    c.line("OK = 0,");
    c.line("/** 缺少必填参数或参数无效。 */");
    c.line("INVALID_INPUT = 1,");
    c.line(format!("/** App 未设置 {m}IntentRuntime.handlers。 */"));
    c.line("HANDLERS_NOT_SET = 2,");
    c.line("/** handler 抛出异常或返回值无法序列化。 */");
    c.line("FAILED = 3,");
    c.close("}");
    c.blank();
    c.line("/** 失败结果的内容。 */");
    c.open(format!("class {m}IntentFailure {{"));
    c.line("kind: string;");
    c.line("message: string;");
    c.blank();
    c.open("constructor(kind: string, message: string) {");
    c.line("this.kind = kind;");
    c.line("this.message = message;");
    c.close("}");
    c.close("}");
    c.blank();
    c.open(format!("export class {m}IntentRuntime {{"));
    c.line(format!(
        "/** 由 App 在启动时设置；未设置时意图返回 {m}IntentCode.HANDLERS_NOT_SET。 */"
    ));
    c.line(format!(
        "static handlers: {m}ToolHandlers | undefined = undefined;"
    ));
    c.blank();
    c.line("/** 调用 handler 并把结果转换为 IntentResult。 */");
    c.open(format!(
        "static run(call: (handlers: {m}ToolHandlers) => {HANDLER_RETURN}): Promise<insightIntent.IntentResult<string>> {{"
    ));
    c.line(format!("const handlers = {m}IntentRuntime.handlers;"));
    c.open("if (handlers === undefined) {");
    c.line(format!(
        "return Promise.resolve({m}IntentRuntime.failure({m}IntentCode.HANDLERS_NOT_SET, 'HANDLER_ERROR', '未设置 {m}IntentRuntime.handlers'));"
    ));
    c.close("}");
    c.line(format!("let pending: Promise<{RESULT_TY}>;"));
    c.open("try {");
    c.line("pending = Promise.resolve(call(handlers));");
    c.dedent();
    c.line("} catch (error) {");
    c.indent();
    c.line(format!(
        "return Promise.resolve({m}IntentRuntime.fromError(error as Object));"
    ));
    c.close("}");
    c.line(format!(
        "return pending.then((value: {RESULT_TY}): insightIntent.IntentResult<string> => {m}IntentRuntime.success(value),"
    ));
    c.line(format!(
        "  (error: Object): insightIntent.IntentResult<string> => {m}IntentRuntime.fromError(error));"
    ));
    c.close("}");
    c.blank();
    c.line("/** 缺少必填参数。 */");
    c.open("static missing(field: string): Promise<insightIntent.IntentResult<string>> {");
    c.line(format!(
        "return Promise.resolve({m}IntentRuntime.failure({m}IntentCode.INVALID_INPUT, 'INVALID_INPUT', `缺少参数 ${{field}}`));"
    ));
    c.close("}");
    c.blank();
    c.line("/** 参数无效（整数、枚举取值、实体必填属性或 JSON 文本不合要求）。 */");
    c.open("static invalid(field: string, reason: string): Promise<insightIntent.IntentResult<string>> {");
    c.line(format!(
        "return Promise.resolve({m}IntentRuntime.failure({m}IntentCode.INVALID_INPUT, 'INVALID_INPUT', `参数 ${{field}} 无效：${{reason}}`));"
    ));
    c.close("}");
    c.blank();
    json_text_helpers(&mut c);
    c.blank();
    c.open(format!(
        "private static success(value: {RESULT_TY}): insightIntent.IntentResult<string> {{"
    ));
    c.line("const data: Object | null = value instanceof ToolResult ? (value as ToolResult<Object | null>).data : value;");
    c.line("let text: string | undefined;");
    c.open("try {");
    c.line("text = JSON.stringify(data);");
    c.dedent();
    c.line("} catch (error) {");
    c.indent();
    c.line(format!(
        "return {m}IntentRuntime.failure({m}IntentCode.FAILED, 'HANDLER_ERROR', `返回值无法序列化为 JSON：${{(error as Error).message}}`);"
    ));
    c.close("}");
    c.line(
        "const result: insightIntent.IntentResult<string> = { code: 0, result: text ?? 'null' };",
    );
    c.line("return result;");
    c.close("}");
    c.blank();
    c.open("private static fromError(error: Object): insightIntent.IntentResult<string> {");
    c.open("if (error instanceof ToolCallError) {");
    c.line(format!(
        "return {m}IntentRuntime.failure({m}IntentCode.FAILED, error.kind, error.message);"
    ));
    c.close("}");
    c.line("const message = error instanceof Error ? error.message : String(error);");
    c.line(format!(
        "return {m}IntentRuntime.failure({m}IntentCode.FAILED, 'HANDLER_ERROR', message);"
    ));
    c.close("}");
    c.blank();
    c.open(format!(
        "private static failure(code: {m}IntentCode, kind: string, message: string): insightIntent.IntentResult<string> {{"
    ));
    c.line(format!(
        "const result: insightIntent.IntentResult<string> = {{ code: code, result: JSON.stringify(new {m}IntentFailure(kind, message)) }};"
    ));
    c.line("return result;");
    c.close("}");
    c.close("}");
    c.finish()
}

/// JSON 文本参数（字典与原始 JSON，见 `executor/convert.rs`）的校验与解析。
fn json_text_helpers(c: &mut Code) {
    c.line("/** 是否为合法 JSON 文本。 */");
    c.open("static isJson(text: string): boolean {");
    c.open("try {");
    c.line("JSON.parse(text);");
    c.line("return true;");
    c.dedent();
    c.line("} catch (error) {");
    c.indent();
    c.line("return false;");
    c.close("}");
    c.close("}");
    c.blank();
    c.line("/** 是否为 JSON 对象文本（不含数组）；allowNull 时 `null` 也可。 */");
    c.open("static isJsonObject(text: string, allowNull: boolean): boolean {");
    c.line("let value: Object | null;");
    c.open("try {");
    c.line("value = JSON.parse(text) as Object | null;");
    c.dedent();
    c.line("} catch (error) {");
    c.indent();
    c.line("return false;");
    c.close("}");
    c.open("if (value === null) {");
    c.line("return allowNull;");
    c.close("}");
    c.line("return typeof value === 'object' && !Array.isArray(value);");
    c.close("}");
    c.blank();
    c.line("/** 解析已校验的 JSON 文本。 */");
    c.open("static parseJson(text: string): Object | null {");
    c.line("return JSON.parse(text) as Object | null;");
    c.close("}");
}

// ---------------------------------------------------------------------------
// insight_intent.json
// ---------------------------------------------------------------------------

/// @input `src_entries` 各执行器文件相对 `src/main/` 的路径（`./ets/...`）。
fn insight_intent_json(src_entries: &[String]) -> String {
    let entries: Vec<Value> = src_entries
        .iter()
        .map(|e| json!({ "srcEntry": e }))
        .collect();
    let mut s = serde_json::to_string_pretty(&json!({ "insightIntentsSrcEntry": entries }))
        .unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests;
