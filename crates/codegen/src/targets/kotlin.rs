//! Kotlin：`@Serializable` data class / enum（kotlinx.serialization）+ `<Module>ToolHandlers`
//! 接口 + 分派辅助。

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::deprecation;
use crate::ident::{self, Lang, NameScope};
use crate::schema::{EnumDecl, Field, Model, ObjectDecl, Ty, TypeDecl, field_doc};
use crate::targets::{decl_doc, file, tool_doc};

pub fn file_name(model: &Model) -> String {
    format!("{}Tools.kt", model.module)
}

pub fn ty(model: &Model, t: &Ty) -> String {
    match t {
        Ty::String => "String".into(),
        Ty::Integer => "Long".into(),
        Ty::Number => "Double".into(),
        Ty::Boolean => "Boolean".into(),
        Ty::Enum(id) | Ty::Object(id) => model.decl(*id).name().to_string(),
        Ty::Array(inner) => format!("List<{}>", ty(model, inner)),
        Ty::Map(inner) => format!("Map<String, {}>", ty(model, inner)),
        Ty::Json => "JsonElement".into(),
    }
}

pub fn field_ty(model: &Model, f: &Field) -> String {
    let t = ty(model, &f.ty);
    if f.optional() { format!("{t}?") } else { t }
}

/// 对象各字段的 Kotlin 属性名（camelCase，未转义，去重）。
pub fn property_names(o: &ObjectDecl) -> Vec<String> {
    let mut scope = NameScope::new();
    o.fields
        .iter()
        .map(|f| scope.claim(&ident::camel(&f.json_name, "field")))
        .collect()
}

/// 枚举常量名（SCREAMING_SNAKE_CASE，去重）。
pub fn enum_constants(e: &EnumDecl) -> Vec<String> {
    // entries / values / valueOf 是 Kotlin 枚举的合成成员
    let mut scope = NameScope::with_reserved(&["ENTRIES"]);
    e.values
        .iter()
        .enumerate()
        .map(|(i, v)| scope.claim(&ident::screaming(v, &format!("VALUE_{}", i + 1))))
        .collect()
}

fn emit_enum(c: &mut Code, e: &EnumDecl) {
    c.block_doc(&decl_doc(e.description.as_deref()));
    c.line("@Serializable");
    c.open(format!("enum class {} {{", e.name));
    let consts = enum_constants(e);
    for (value, name) in e.values.iter().zip(&consts) {
        c.line(format!(
            "@SerialName({}) {name},",
            string_literal(Lang::Kotlin, value)
        ));
    }
    c.close("}");
}

fn emit_object(c: &mut Code, model: &Model, o: &ObjectDecl) {
    c.block_doc(&decl_doc(o.description.as_deref()));
    c.line("@Serializable");
    if o.fields.is_empty() {
        // data class 至少需要一个属性
        c.open(format!("class {} {{", o.name));
        c.line(format!(
            "override fun equals(other: Any?): Boolean = other is {}",
            o.name
        ));
        c.line("override fun hashCode(): Int = 0");
        c.line(format!(
            "override fun toString(): String = \"{}()\"",
            o.name
        ));
        c.close("}");
        return;
    }
    c.open(format!("data class {}(", o.name));
    for (f, name) in o.fields.iter().zip(property_names(o)) {
        c.block_doc(&field_doc(f));
        if f.deprecated {
            c.line(deprecated_annotation(deprecation::FIELD_MESSAGE));
        }
        // 非必填字段默认 null；必填但可为 null 的字段不给默认值（JSON 中必须出现）
        let default = if f.required { "" } else { " = null" };
        c.line(format!(
            "@SerialName({}) val {}: {}{default},",
            string_literal(Lang::Kotlin, &f.json_name),
            ident::escape(Lang::Kotlin, &name),
            field_ty(model, f)
        ));
    }
    c.close(")");
}

/// 生成代码自身调用已弃用 handler 处的局部抑制（App 的实现与其他调用处仍有提示）。
pub const SUPPRESS_DEPRECATION: &str = "@Suppress(\"DEPRECATION\")";

/// `@Deprecated("…")`（消息按 Kotlin 字符串字面量转义）。
///
/// @why 不生成 `ReplaceWith`：替代工具的参数类型不同，给不出能直接替换的合法表达式。
pub fn deprecated_annotation(message: &str) -> String {
    format!("@Deprecated({})", string_literal(Lang::Kotlin, message))
}

pub const IMPORTS: &[&str] = &[
    "import kotlinx.serialization.SerialName",
    "import kotlinx.serialization.Serializable",
    "import kotlinx.serialization.json.Json",
    "import kotlinx.serialization.json.JsonElement",
    "import kotlinx.serialization.json.JsonObject",
];

pub fn generate(model: &Model, package: &str) -> GeneratedFile {
    let m = &model.module;
    let mut c = Code::new("    ");
    c.comment("// ", &header_lines(model, "kotlin"));
    c.line("@file:Suppress(\"unused\", \"RedundantVisibilityModifier\")");
    c.blank();
    c.line(format!("package {package}"));
    c.blank();
    for i in IMPORTS {
        c.line(*i);
    }
    c.blank();

    for decl in &model.types {
        match decl {
            TypeDecl::Enum(e) => emit_enum(&mut c, e),
            TypeDecl::Object(o) => emit_object(&mut c, model, o),
        }
        c.blank();
    }

    c.block_doc(&[
        format!(
            "{} 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，",
            model.app_name
        ),
        "返回值作为工具结果（JSON）。".to_string(),
    ]);
    c.open(format!("interface {m}ToolHandlers {{"));
    for tool in &model.tools {
        c.block_doc(&tool_doc(tool));
        if let Some(message) = deprecation::tool_deprecation(tool) {
            c.line(deprecated_annotation(&message));
        }
        c.line(format!(
            "suspend fun {}(params: {}): JsonElement",
            ident::escape(Lang::Kotlin, &tool.camel),
            model.params(tool).name
        ));
    }
    c.close("}");
    c.blank();

    c.line("/** 工具名与分派辅助。 */");
    c.open(format!("object {m}Tools {{"));
    c.line("/** 清单中的全部工具名。 */");
    c.open("val names: List<String> = listOf(");
    for tool in &model.tools {
        c.line(format!(
            "{},",
            string_literal(Lang::Kotlin, &tool.info.name)
        ));
    }
    c.close(")");
    c.blank();
    c.line("/** 解析参数使用的 Json 实例（忽略未知字段）。 */");
    c.line("val json: Json = Json { ignoreUnknownKeys = true }");
    c.blank();
    c.line("/** 按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。 */");
    if model.tools.iter().any(|t| t.deprecation().is_some()) {
        c.line(SUPPRESS_DEPRECATION);
    }
    c.open(format!(
        "suspend fun dispatch(handlers: {m}ToolHandlers, name: String, arguments: JsonElement?): JsonElement {{"
    ));
    c.line("val args = arguments ?: JsonObject(emptyMap())");
    c.open("return when (name) {");
    for tool in &model.tools {
        c.line(format!(
            "{} -> handlers.{}(json.decodeFromJsonElement({}.serializer(), args))",
            string_literal(Lang::Kotlin, &tool.info.name),
            ident::escape(Lang::Kotlin, &tool.camel),
            model.params(tool).name
        ));
    }
    c.line("else -> throw IllegalArgumentException(\"未知工具：$name\")");
    c.close("}");
    c.close("}");
    c.close("}");

    file(file_name(model), c.finish())
}
