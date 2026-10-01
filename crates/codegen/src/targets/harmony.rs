//! 鸿蒙（HarmonyOS NEXT / OpenHarmony）意图框架：装饰器方式的 InsightIntent（API 20+）+ ArkTS 类型化接口。
//!
//! 输出（路径相对于模块的 `src/main/`）：
//! - `ets/appmcp/<Module>Tools.ets`：参数类型（interface / 字符串字面量联合）、`<Module>ToolHandlers`、
//!   `register<Module>Tools(registrar, handlers)`（用 `@app-mcp/harmony` 把清单中的工具注册为 MCP 工具）；
//! - `ets/appmcp/<Module>InsightIntents.ets`：`<Module>IntentRuntime`（handlers 注入点、结果码、结果转换）；
//! - `ets/insightintents/<Module><Tool>Intent.ets`：每个工具一个 `@InsightIntentEntry` 执行器
//!   （`InsightIntentEntryExecutor<string>`，结果为 handler 返回值的 JSON 文本）；
//! - `resources/base/profile/insight_intent.json`：`insightIntentsSrcEntry` 列出各执行器文件。
//!
//! 映射：意图参数由系统入口按 `parameters`（JSON Schema）赋值给执行器的同名属性，因此属性名就是 JSON 属性名；
//! 含非标识符属性名（如 `is-urgent`）或与执行器基类成员同名的工具不生成意图（给出警告），仍可作为 MCP 工具使用。
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
    files.push(file(
        "resources/base/profile/insight_intent.json",
        insight_intent_json(model, &supported),
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
    None
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
    c.line("/** 缺少必填参数。 */");
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

// ---------------------------------------------------------------------------
// 意图执行器
// ---------------------------------------------------------------------------

/// 字段类型中直接引用的声明名（需从 Tools 文件导入）。
fn referenced_types(model: &Model, t: &Ty, out: &mut BTreeSet<String>) {
    match t {
        Ty::Enum(id) | Ty::Object(id) => {
            out.insert(model.decl(*id).name().to_string());
        }
        Ty::Array(inner) | Ty::Map(inner) => referenced_types(model, inner, out),
        _ => {}
    }
}

fn executor_file(model: &Model, tool: &ToolModel, domain: &str, ability: &str) -> String {
    let m = &model.module;
    let params = model.params(tool);
    let class = executor_name(model, tool);
    let lit = |s: &str| string_literal(Lang::TypeScript, s);

    let mut imports = BTreeSet::new();
    imports.insert(params.name.clone());
    imports.insert(format!("{m}ToolHandlers"));
    for f in &params.fields {
        referenced_types(model, &f.ty, &mut imports);
    }

    let mut c = Code::new("  ");
    let mut header = header_lines(model, "harmony-insight-intents");
    header.push(format!(
        "工具 `{}` 的意图执行器（insight_intent.json 的 insightIntentsSrcEntry 引用本文件）。",
        tool.info.name
    ));
    c.comment("// ", &header);
    c.blank();
    c.line("import { insightIntent, InsightIntentEntry, InsightIntentEntryExecutor } from '@kit.AbilityKit';");
    c.line(format!(
        "import {{ {m}IntentRuntime }} from '../appmcp/{m}InsightIntents';"
    ));
    let names: Vec<String> = imports.into_iter().collect();
    c.line(format!(
        "import {{ {} }} from '../appmcp/{m}Tools';",
        names.join(", ")
    ));
    c.blank();

    let mut doc = tool_doc(tool);
    if needs_confirmation(tool.info.risk) {
        doc.push(format!(
            "风险 {}：意图框架没有系统级确认，以前台模式执行，handler 应在执行前向用户确认。",
            risk_name(tool.info.risk)
        ));
    }
    c.block_doc(&doc);

    let first_line = tool.info.description.lines().next().unwrap_or("").trim();
    let display_description = if first_line.is_empty() {
        tool.display_title()
    } else {
        first_line
    };
    let mut keywords = vec![lit(tool.display_title())];
    if tool.info.title.is_some() {
        keywords.push(lit(&tool.info.name));
    }
    c.open("@InsightIntentEntry({");
    c.line(format!("intentName: {},", lit(&intent_name(model, tool))));
    c.line(format!("domain: {},", lit(domain)));
    c.line(format!("intentVersion: {},", lit(INTENT_VERSION)));
    c.line(format!("displayName: {},", lit(tool.display_title())));
    c.line(format!("displayDescription: {},", lit(display_description)));
    c.line(format!("llmDescription: {},", lit(&tool.info.description)));
    c.line(format!("keywords: [{}],", keywords.join(", ")));
    c.line(format!("abilityName: {},", lit(ability)));
    c.line(format!("executeMode: [{}],", execute_mode(tool)));
    let schema =
        serde_json::to_string_pretty(&intent_parameters(model, params)).unwrap_or_default();
    let mut lines = schema.lines();
    if let Some(first) = lines.next() {
        c.line(format!("parameters: {first}"));
        let rest: Vec<&str> = lines.collect();
        for (i, l) in rest.iter().enumerate() {
            let tail = if i + 1 == rest.len() { "," } else { "" };
            c.line(format!("{l}{tail}"));
        }
    }
    c.close("})");
    c.open(format!(
        "export default class {class} extends InsightIntentEntryExecutor<string> {{"
    ));
    for f in &params.fields {
        c.block_doc(&field_doc(f));
        c.line(format!("public {}?: {};", f.json_name, field_ty(model, f)));
    }
    if !params.fields.is_empty() {
        c.blank();
    }
    c.open("onExecute(): Promise<insightIntent.IntentResult<string>> {");
    let mut values = Vec::new();
    for f in &params.fields {
        if f.required {
            let local = format!("{}Value", f.json_name);
            c.line(format!("const {local} = this.{};", f.json_name));
            c.open(format!("if ({local} === undefined) {{"));
            c.line(format!(
                "return {m}IntentRuntime.missing({});",
                lit(&f.json_name)
            ));
            c.close("}");
            values.push(format!("{}: {local}", f.json_name));
        } else {
            values.push(format!("{0}: this.{0}", f.json_name));
        }
    }
    if values.is_empty() {
        c.line(format!("const params: {} = {{}};", params.name));
    } else {
        c.open(format!("const params: {} = {{", params.name));
        for v in &values {
            c.line(format!("{v},"));
        }
        c.close("};");
    }
    c.line(format!(
        "return {m}IntentRuntime.run((handlers: {m}ToolHandlers) => handlers.{}(params));",
        tool.camel
    ));
    c.close("}");
    c.close("}");
    c.finish()
}

/// 意图的 `parameters`：由类型模型重新生成的 JSON Schema。
///
/// @why 构建工具用 ajv（默认严格模式）编译 `parameters`：未知 `format` 与 `$ref` 会导致编译失败，
///   因此只输出类型、描述、枚举与数值 / 长度约束；降级为原始 JSON 的字段为空 schema（任意值）。
pub fn intent_parameters(model: &Model, params: &ObjectDecl) -> Value {
    object_schema(model, params)
}

fn object_schema(model: &Model, o: &ObjectDecl) -> Value {
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    if let Some(d) = &o.description {
        schema.insert("description".into(), json!(d));
    }
    let mut props = Map::new();
    for f in &o.fields {
        props.insert(f.json_name.clone(), field_schema(model, f));
    }
    schema.insert("properties".into(), Value::Object(props));
    let required: Vec<&str> = o
        .fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.json_name.as_str())
        .collect();
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    Value::Object(schema)
}

fn ty_schema(model: &Model, t: &Ty) -> Map<String, Value> {
    let mut s = Map::new();
    match t {
        Ty::String => {
            s.insert("type".into(), json!("string"));
        }
        Ty::Integer => {
            s.insert("type".into(), json!("integer"));
        }
        Ty::Number => {
            s.insert("type".into(), json!("number"));
        }
        Ty::Boolean => {
            s.insert("type".into(), json!("boolean"));
        }
        Ty::Enum(id) => {
            s.insert("type".into(), json!("string"));
            if let Some(e) = model.enum_decl(*id) {
                s.insert("enum".into(), json!(e.values));
            }
        }
        Ty::Object(id) => {
            if let Some(o) = model.object(*id) {
                if let Value::Object(map) = object_schema(model, o) {
                    s = map;
                }
            }
        }
        Ty::Array(inner) => {
            s.insert("type".into(), json!("array"));
            s.insert("items".into(), Value::Object(ty_schema(model, inner)));
        }
        Ty::Map(inner) => {
            s.insert("type".into(), json!("object"));
            s.insert(
                "additionalProperties".into(),
                Value::Object(ty_schema(model, inner)),
            );
        }
        Ty::Json => {}
    }
    s
}

fn field_schema(model: &Model, f: &Field) -> Value {
    let mut s = ty_schema(model, &f.ty);
    if let Some(d) = &f.description {
        s.insert("description".into(), json!(d));
    }
    let c = &f.constraints;
    let numbers = [
        ("minimum", &c.minimum),
        ("maximum", &c.maximum),
        ("exclusiveMinimum", &c.exclusive_minimum),
        ("exclusiveMaximum", &c.exclusive_maximum),
    ];
    for (key, v) in numbers {
        if let Some(n) = v {
            s.insert(key.into(), Value::Number(n.clone()));
        }
    }
    let counts = [
        ("minLength", c.min_length),
        ("maxLength", c.max_length),
        ("minItems", c.min_items),
        ("maxItems", c.max_items),
    ];
    for (key, v) in counts {
        if let Some(n) = v {
            s.insert(key.into(), json!(n));
        }
    }
    if let Some(p) = &c.pattern {
        s.insert("pattern".into(), json!(p));
    }
    if let Some(values) = &c.allowed {
        s.insert("enum".into(), Value::Array(values.clone()));
    }
    if let Some(d) = &c.default {
        s.insert("default".into(), d.clone());
    }
    Value::Object(s)
}

// ---------------------------------------------------------------------------
// insight_intent.json
// ---------------------------------------------------------------------------

fn insight_intent_json(model: &Model, tools: &[&ToolModel]) -> String {
    let entries: Vec<Value> = tools
        .iter()
        .map(|t| json!({ "srcEntry": format!("./ets/insightintents/{}.ets", executor_name(model, t)) }))
        .collect();
    let mut s = serde_json::to_string_pretty(&json!({ "insightIntentsSrcEntry": entries }))
        .unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_mcp_protocol::Risk;
    use serde_json::json;

    fn model(tools: Value) -> Model {
        let manifest = app_mcp_manifest::parse(
            &json!({ "manifestVersion": 1, "appId": "shop", "name": "Shop", "tools": tools })
                .to_string(),
        )
        .expect("清单");
        crate::schema::build(&manifest, None, None)
    }

    fn generate_all(m: &Model) -> (Vec<GeneratedFile>, Vec<Warning>) {
        let mut w = Vec::new();
        (generate(m, DEFAULT_DOMAIN, DEFAULT_ABILITY, &mut w), w)
    }

    #[test]
    fn execute_mode_follows_risk_and_activation() {
        let m = model(json!([
            { "name": "a.read", "description": "读", "inputSchema": { "type": "object" }, "risk": "read" },
            { "name": "a.pay", "description": "付", "inputSchema": { "type": "object" }, "risk": "payment" },
            { "name": "a.show", "description": "显示", "inputSchema": { "type": "object" }, "risk": "read", "activation": "foreground" }
        ]));
        let modes: Vec<&str> = m.tools.iter().map(execute_mode).collect();
        assert_eq!(
            modes,
            [
                "insightIntent.ExecuteMode.UI_ABILITY_BACKGROUND",
                "insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND",
                "insightIntent.ExecuteMode.UI_ABILITY_FOREGROUND"
            ]
        );
        assert!(needs_confirmation(Risk::Destructive));
        assert!(!needs_confirmation(Risk::Write));
    }

    #[test]
    fn skips_tools_whose_params_cannot_be_executor_properties() {
        let m = model(json!([
            { "name": "a.dash", "description": "x", "inputSchema": { "type": "object", "properties": { "is-x": { "type": "boolean" } } } },
            { "name": "a.ctx", "description": "x", "inputSchema": { "type": "object", "properties": { "context": { "type": "string" } } } },
            { "name": "a.ok", "description": "x", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } }
        ]));
        let (files, warnings) = generate_all(&m);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let executors: Vec<String> = files
            .iter()
            .filter(|f| f.path.starts_with("ets/insightintents"))
            .map(|f| f.path.display().to_string())
            .collect();
        assert_eq!(executors, ["ets/insightintents/ShopAOkIntent.ets"]);
        let json = files
            .iter()
            .find(|f| f.path.ends_with("insight_intent.json"))
            .expect("配置");
        assert!(
            json.contents
                .contains("./ets/insightintents/ShopAOkIntent.ets")
        );
        assert!(!json.contents.contains("ADash"));
        // 不能生成意图的工具仍注册为 MCP 工具
        let tools = &files[0].contents;
        assert!(tools.contains("\"a.dash\"") && tools.contains("\"is-x\"?: boolean;"));
    }

    #[test]
    fn parameters_drop_format_and_degraded_constructs() {
        let m = model(json!([{
            "name": "a.b",
            "description": "x",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "when": { "type": "string", "format": "date-time", "description": "时间" },
                    "n": { "type": "integer", "minimum": 1, "default": 2 },
                    "raw": { "$ref": "#/definitions/x" },
                    "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                },
                "required": ["n"]
            }
        }]));
        let schema = intent_parameters(&m, m.params(&m.tools[0]));
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "description": "x",
                "properties": {
                    "when": { "type": "string", "description": "时间" },
                    "n": { "type": "integer", "minimum": 1, "default": 2 },
                    "raw": {},
                    "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                },
                "required": ["n"]
            })
        );
    }

    #[test]
    fn executor_checks_required_and_passes_optional() {
        let m = model(json!([{
            "name": "cart.add",
            "title": "加入购物车",
            "description": "把商品加入购物车",
            "inputSchema": {
                "type": "object",
                "properties": { "productId": { "type": "string" }, "note": { "type": ["string", "null"] } },
                "required": ["productId"]
            }
        }]));
        let (files, _) = generate_all(&m);
        let exec = files
            .iter()
            .find(|f| f.path.ends_with("ShopCartAddIntent.ets"))
            .expect("执行器");
        let text = &exec.contents;
        assert!(text.contains("intentName: \"ShopCartAdd\","));
        assert!(text.contains("domain: \"ToolsDomain\","));
        assert!(text.contains("keywords: [\"加入购物车\", \"cart.add\"],"));
        assert!(text.contains("public productId?: string;"));
        assert!(text.contains("public note?: string | null;"));
        assert!(text.contains("if (productIdValue === undefined) {"));
        assert!(text.contains("return ShopIntentRuntime.missing(\"productId\");"));
        assert!(text.contains("note: this.note,"));
        assert!(text.contains("handlers.cartAdd(params)"));
    }

    #[test]
    fn custom_domain_and_ability() {
        let m = model(
            json!([{ "name": "a", "description": "x", "inputSchema": { "type": "object" } }]),
        );
        let mut w = Vec::new();
        let files = generate(&m, "ShoppingPlatformsDomain", "MainAbility", &mut w);
        let exec = files
            .iter()
            .find(|f| f.path.ends_with("ShopAIntent.ets"))
            .expect("执行器");
        assert!(
            exec.contents
                .contains("domain: \"ShoppingPlatformsDomain\",")
        );
        assert!(exec.contents.contains("abilityName: \"MainAbility\","));
        assert!(exec.contents.contains("const params: AParams = {};"));
    }
}
