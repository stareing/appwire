//! Android AppFunctions（Jetpack `androidx.appfunctions` 1.0.0-alpha12，Android 16 / API 36+）。
//!
//! 输出两个文件：
//! - `<Module>Tools.kt`：与 `kotlin` target 完全相同（kotlinx.serialization 参数类型 + handler 接口）；
//! - `<Module>AppFunctions.kt`：`@AppFunctionServiceEntryPoint` 抽象服务，每个工具一个 `@AppFunction`
//!   方法；嵌套对象生成 `@AppFunctionSerializable` data class；方法把参数组装成 JSON 后解码为
//!   `<Tool>Params` 并调用 `<Module>ToolHandlers`。
//!
//! 依据（2026-09）：<https://developer.android.com/ai/appfunctions/add-appfunctions>、
//! <https://developer.android.com/jetpack/androidx/releases/appfunctions>（alpha10 起函数须位于
//! `AppFunctionService` 子类；alpha12 移除 `AppFunctionContext` / `AppFunctionConfiguration`）。

use std::collections::HashMap;

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang, NameScope};
use crate::schema::{Field, Model, ObjectDecl, ToolModel, Ty, TypeId, Warning, field_doc};
use crate::targets::{file, kotlin, risk_name};

mod standard;

pub const APPFUNCTIONS_VERSION: &str = "1.0.0-alpha12";

/// @input `standard_intents` 为真时，为声明了 `implements` 的工具追加 Android 系统意图输出（[`standard`]）；
/// 为假时输出与未声明 `implements` 相同。
pub fn generate(
    model: &Model,
    package: &str,
    standard_intents: bool,
    warnings: &mut Vec<Warning>,
) -> Vec<GeneratedFile> {
    let mut files = vec![
        kotlin::generate(model, package),
        file(
            format!("{}AppFunctions.kt", model.module),
            functions_file(model, package),
        ),
    ];
    if standard_intents {
        files.extend(standard::generate(model, package, warnings));
    }
    files
}

/// 嵌套对象 → `@AppFunctionSerializable` 类名。
struct InputNames(HashMap<usize, String>);

impl InputNames {
    fn get(&self, id: TypeId) -> &str {
        self.0.get(&id.0).map(String::as_str).unwrap_or("Unknown")
    }
}

/// 字段在 AppFunction 中的 Kotlin 类型；`None` 表示降级为 JSON 字符串。
fn af_ty(names: &InputNames, t: &Ty) -> Option<String> {
    Some(match t {
        Ty::String | Ty::Enum(_) => "String".into(),
        Ty::Integer => "Long".into(),
        Ty::Number => "Double".into(),
        Ty::Boolean => "Boolean".into(),
        Ty::Object(id) => names.get(*id).to_string(),
        Ty::Array(inner) => match inner.as_ref() {
            Ty::String | Ty::Enum(_) => "List<String>".into(),
            Ty::Integer => "LongArray".into(),
            Ty::Number => "DoubleArray".into(),
            Ty::Boolean => "BooleanArray".into(),
            Ty::Object(id) => format!("List<{}>", names.get(*id)),
            _ => return None,
        },
        Ty::Map(_) | Ty::Json => return None,
    })
}

/// 把 AppFunction 中的值（`expr`，非 null）转换为 `JsonElement` 的表达式。
fn to_json_expr(names: &InputNames, t: &Ty, expr: &str, m: &str) -> String {
    match af_ty(names, t) {
        None => format!("{m}AppFunctionsSupport.parseJson({expr})"),
        Some(_) => match t {
            Ty::Object(_) => format!("{expr}.toJson()"),
            Ty::Array(inner) => match inner.as_ref() {
                Ty::Object(_) => format!("JsonArray({expr}.map {{ e -> e.toJson() }})"),
                _ => format!("JsonArray({expr}.map {{ e -> JsonPrimitive(e) }})"),
            },
            _ => format!("JsonPrimitive({expr})"),
        },
    }
}

fn string_constraint(model: &Model, t: &Ty) -> Option<String> {
    let id = match t {
        Ty::Enum(id) => *id,
        Ty::Array(inner) => match inner.as_ref() {
            Ty::Enum(id) => *id,
            _ => return None,
        },
        _ => return None,
    };
    let e = model.enum_decl(id)?;
    let values: Vec<String> = e
        .values
        .iter()
        .map(|v| string_literal(Lang::Kotlin, v))
        .collect();
    Some(format!(
        "AppFunctionStringValueConstraint(enumValues = [{}])",
        values.join(", ")
    ))
}

fn field_kdoc(f: &Field, degraded: bool) -> Vec<String> {
    let mut lines = field_doc(f);
    if degraded {
        lines.push("以 JSON 字符串传入。".to_string());
    }
    lines
}

/// 收集参数中直接或经数组引用的嵌套对象（按依赖顺序）。
fn collect_inputs(model: &Model, t: &Ty, out: &mut Vec<TypeId>) {
    let id = match t {
        Ty::Object(id) => *id,
        Ty::Array(inner) => match inner.as_ref() {
            Ty::Object(id) => *id,
            _ => return,
        },
        _ => return,
    };
    if out.contains(&id) {
        return;
    }
    if let Some(o) = model.object(id) {
        for f in &o.fields {
            collect_inputs(model, &f.ty, out);
        }
    }
    out.push(id);
}

fn functions_file(model: &Model, package: &str) -> String {
    let m = &model.module;
    let service = format!("{m}AppFunctionService");
    let base = format!("Base{m}AppFunctionService");
    let xml = format!("{}_app_function_service", ident::snake(m, "app"));

    let mut inputs = Vec::new();
    for tool in &model.tools {
        for f in &model.params(tool).fields {
            collect_inputs(model, &f.ty, &mut inputs);
        }
    }
    let mut scope =
        NameScope::with_reserved(&model.types.iter().map(|t| t.name()).collect::<Vec<_>>());
    let names = InputNames(
        inputs
            .iter()
            .filter_map(|id| {
                model
                    .object(*id)
                    .map(|o| (id.0, scope.claim(&format!("{}Input", o.name))))
            })
            .collect(),
    );

    let mut c = Code::new("    ");
    let mut header = header_lines(model, "kotlin-appfunctions");
    header.extend([
        String::new(),
        format!("Android AppFunctions（androidx.appfunctions:{APPFUNCTIONS_VERSION}），需要 Android 16（API 36）+。"),
        format!("依赖同目录下的 {m}Tools.kt（参数类型与 {m}ToolHandlers 接口）。"),
        String::new(),
        "build.gradle.kts：".to_string(),
        format!("  implementation(\"androidx.appfunctions:appfunctions:{APPFUNCTIONS_VERSION}\")"),
        format!("  ksp(\"androidx.appfunctions:appfunctions-compiler:{APPFUNCTIONS_VERSION}\")"),
        "  ksp { arg(\"appfunctions:aggregateAppFunctions\", \"true\") }  // 应用模块".to_string(),
        "  以及 kotlinx-serialization-json、kotlinx-coroutines-android（KSP 生成的服务类使用）".to_string(),
        "  与 plugin.serialization。".to_string(),
        String::new(),
        format!("Application 实现 {m}ToolHandlersProvider（可直接复用 MCP 的业务实现），并在 AndroidManifest.xml 中声明："),
        format!("  <service android:name=\"{package}.{service}\""),
        "      android:permission=\"android.permission.BIND_APP_FUNCTION_SERVICE\"".to_string(),
        "      android:exported=\"true\" tools:targetApi=\"36\">".to_string(),
        "    <property android:name=\"android.app.appfunctions.schema\" android:value=\"app_functions_schema.xsd\" />".to_string(),
        format!("    <property android:name=\"android.app.appfunctions.v2\" android:value=\"{xml}.xml\" />"),
        "    <intent-filter><action android:name=\"android.app.appfunctions.AppFunctionService\" /></intent-filter>".to_string(),
        "  </service>".to_string(),
    ]);
    c.comment("// ", &header);
    c.line("@file:Suppress(\"unused\", \"RedundantVisibilityModifier\")");
    c.blank();
    c.line(format!("package {package}"));
    c.blank();
    for i in [
        "import androidx.annotation.RequiresApi",
        "import androidx.appfunctions.AppFunction",
        "import androidx.appfunctions.AppFunctionAppUnknownException",
        "import androidx.appfunctions.AppFunctionInvalidArgumentException",
        "import androidx.appfunctions.AppFunctionSerializable",
        "import androidx.appfunctions.AppFunctionService",
        "import androidx.appfunctions.AppFunctionServiceEntryPoint",
        "import androidx.appfunctions.AppFunctionStringValueConstraint",
        "import kotlinx.serialization.SerializationException",
        "import kotlinx.serialization.json.JsonArray",
        "import kotlinx.serialization.json.JsonElement",
        "import kotlinx.serialization.json.JsonObject",
        "import kotlinx.serialization.json.JsonPrimitive",
        "import kotlinx.serialization.json.buildJsonObject",
    ] {
        c.line(i);
    }
    c.blank();

    c.line(format!(
        "/** 由 Application 实现，向 AppFunctions 服务提供 {m}ToolHandlers。 */"
    ));
    c.open(format!("interface {m}ToolHandlersProvider {{"));
    c.line(format!(
        "val {}ToolHandlers: {m}ToolHandlers",
        ident::camel(m, "app")
    ));
    c.close("}");
    c.blank();

    // 嵌套对象
    for id in &inputs {
        let Some(o) = model.object(*id) else { continue };
        emit_input(&mut c, model, &names, o, names.get(*id), m);
        c.blank();
    }

    // 辅助
    c.line("/** 参数组装与结果转换。 */");
    c.open(format!("internal object {m}AppFunctionsSupport {{"));
    c.open("fun parseJson(text: String): JsonElement = try {");
    c.line(format!("{m}Tools.json.parseToJsonElement(text)"));
    c.dedent();
    c.line("} catch (e: SerializationException) {");
    c.indent();
    c.line("throw AppFunctionInvalidArgumentException(\"JSON 参数无法解析：${e.message}\")");
    c.close("}");
    c.blank();
    c.line("/** handler 结果转文本：JSON 字符串取其内容，其他值输出 JSON。 */");
    c.line("fun text(result: JsonElement): String =");
    c.indent();
    c.line("(result as? JsonPrimitive)?.takeIf { it.isString }?.content ?: result.toString()");
    c.dedent();
    c.close("}");
    c.blank();

    c.line("/**");
    c.line(format!(
        " * {} 的 AppFunctions 入口。KSP 生成具体服务类 `{service}`。",
        model.app_name
    ));
    c.line(" */");
    c.line("@RequiresApi(36)");
    c.line("@AppFunctionServiceEntryPoint(");
    c.indent();
    c.line(format!(
        "serviceName = {},",
        string_literal(Lang::Kotlin, &service)
    ));
    c.line(format!(
        "appFunctionXmlFileName = {},",
        string_literal(Lang::Kotlin, &xml)
    ));
    c.dedent();
    c.line(")");
    c.open(format!("abstract class {base} : AppFunctionService() {{"));
    c.line(format!("private val handlers: {m}ToolHandlers"));
    c.indent();
    c.line(format!(
        "get() = (applicationContext as? {m}ToolHandlersProvider)?.{}ToolHandlers",
        ident::camel(m, "app")
    ));
    c.indent();
    c.line(format!(
        "?: throw AppFunctionAppUnknownException(\"Application 未实现 {m}ToolHandlersProvider\")"
    ));
    c.dedent();
    c.dedent();
    c.blank();
    c.open("private suspend fun call(args: JsonObject, block: suspend (JsonObject) -> JsonElement): String {");
    c.open("val result = try {");
    c.line("block(args)");
    c.dedent();
    c.line("} catch (e: SerializationException) {");
    c.indent();
    c.line("throw AppFunctionInvalidArgumentException(\"参数不合法：${e.message}\")");
    c.close("}");
    c.line(format!("return {m}AppFunctionsSupport.text(result)"));
    c.close("}");

    for tool in &model.tools {
        c.blank();
        emit_function(&mut c, model, &names, tool, m);
    }
    c.close("}");
    c.finish()
}

fn property_kotlin_names(o: &ObjectDecl) -> Vec<String> {
    kotlin::property_names(o)
}

fn emit_input(
    c: &mut Code,
    model: &Model,
    names: &InputNames,
    o: &ObjectDecl,
    name: &str,
    m: &str,
) {
    let mut doc = crate::code::doc_lines(o.description.as_deref());
    if doc.is_empty() {
        doc.push(format!("{} 的 AppFunctions 表示。", o.name));
    }
    c.block_doc(&doc);
    c.line("@AppFunctionSerializable(isDescribedByKDoc = true)");
    let props = property_kotlin_names(o);
    if o.fields.is_empty() {
        // AppFunctionSerializable 需要主构造属性；空对象用一个占位属性
        c.open(format!("data class {name}(val unused: String? = null) {{"));
        c.line("fun toJson(): JsonObject = JsonObject(emptyMap())");
        c.close("}");
        return;
    }
    c.open(format!("data class {name}("));
    for (f, p) in o.fields.iter().zip(&props) {
        let t = af_ty(names, &f.ty);
        c.block_doc(&field_kdoc(f, t.is_none()));
        if let Some(con) = string_constraint(model, &f.ty) {
            c.line(format!("@property:{con}"));
        }
        let t = t.unwrap_or_else(|| "String".into());
        c.line(format!(
            "val {}: {t}{},",
            ident::escape(Lang::Kotlin, p),
            optional_suffix(f)
        ));
    }
    c.close(") {");
    c.indent();
    c.open("fun toJson(): JsonObject = buildJsonObject {");
    for (f, p) in o.fields.iter().zip(&props) {
        emit_put(c, names, f, &ident::escape(Lang::Kotlin, p), m);
    }
    c.close("}");
    c.close("}");
}

/// 可空标记与默认值：非必填字段 `? = null`（AppFunctions 元数据据此标为非必填），
/// 必填但可为 null 的字段只加 `?`。
fn optional_suffix(f: &Field) -> &'static str {
    match (f.required, f.nullable) {
        (false, _) => "? = null",
        (true, true) => "?",
        (true, false) => "",
    }
}

fn emit_put(c: &mut Code, names: &InputNames, f: &Field, var: &str, m: &str) {
    let key = string_literal(Lang::Kotlin, &f.json_name);
    if f.optional() {
        let conv = to_json_expr(names, &f.ty, "it", m);
        if f.required {
            // 必填但可为 null：总是输出键
            c.line(format!(
                "put({key}, {var}?.let {{ {conv} }} ?: kotlinx.serialization.json.JsonNull)"
            ));
        } else {
            c.line(format!("{var}?.let {{ put({key}, {conv}) }}"));
        }
    } else {
        c.line(format!(
            "put({key}, {})",
            to_json_expr(names, &f.ty, var, m)
        ));
    }
}

fn emit_function(c: &mut Code, model: &Model, names: &InputNames, tool: &ToolModel, m: &str) {
    let params = model.params(tool);
    let mut scope = NameScope::with_reserved(&["args", "handlers", "call", "it"]);
    let vars: Vec<String> = kotlin::property_names(params)
        .iter()
        .map(|n| ident::escape(Lang::Kotlin, &scope.claim(n)))
        .collect();

    // KDoc：描述 + @param + 风险
    let mut doc: Vec<String> = tool.info.description.lines().map(str::to_string).collect();
    if crate::targets::needs_confirmation(tool.info.risk) {
        doc.push(String::new());
        doc.push(format!(
            "风险：{}。调用方应在执行前向用户确认。",
            risk_name(tool.info.risk)
        ));
    }
    let types: Vec<Option<String>> = params.fields.iter().map(|f| af_ty(names, &f.ty)).collect();
    if !params.fields.is_empty() {
        doc.push(String::new());
    }
    for ((f, v), t) in params.fields.iter().zip(&vars).zip(&types) {
        let text = field_kdoc(f, t.is_none()).join(" ");
        let v = v.trim_matches('`');
        if text.is_empty() {
            doc.push(format!("@param {v} {}", f.json_name));
        } else {
            doc.push(format!("@param {v} {text}"));
        }
    }
    doc.push("@return 工具结果（文本或 JSON）。".to_string());
    c.block_doc(&doc);
    c.line("@AppFunction(isDescribedByKDoc = true)");
    if params.fields.is_empty() {
        c.open(format!(
            "suspend fun {}(): String {{",
            ident::escape(Lang::Kotlin, &tool.camel)
        ));
        c.line("val args = JsonObject(emptyMap())");
    } else {
        c.line(format!(
            "suspend fun {}(",
            ident::escape(Lang::Kotlin, &tool.camel)
        ));
        c.indent();
        for ((f, v), t) in params.fields.iter().zip(&vars).zip(&types) {
            let con = string_constraint(model, &f.ty)
                .map(|s| format!("@{s} "))
                .unwrap_or_default();
            let t = t.clone().unwrap_or_else(|| "String".into());
            c.line(format!("{con}{v}: {t}{},", optional_suffix(f)));
        }
        c.dedent();
        c.open("): String {");
        c.open("val args = buildJsonObject {");
        for (f, v) in params.fields.iter().zip(&vars) {
            emit_put(c, names, f, v, m);
        }
        c.close("}");
    }
    c.line(format!(
        "return call(args) {{ handlers.{}({m}Tools.json.decodeFromJsonElement({}.serializer(), it)) }}",
        ident::escape(Lang::Kotlin, &tool.camel),
        params.name
    ));
    c.close("}");
}
