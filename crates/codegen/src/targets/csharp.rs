//! C#：record 类型（System.Text.Json 特性）+ `I<Module>ToolHandlers` 接口 + 分派辅助类。

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang, NameScope};
use crate::schema::{EnumDecl, Field, Model, ObjectDecl, Ty, TypeDecl, field_doc};
use crate::targets::{decl_doc, file, tool_doc};

pub fn file_name(model: &Model) -> String {
    format!("{}Tools.cs", model.module)
}

pub fn ty(model: &Model, t: &Ty) -> String {
    match t {
        Ty::String => "string".into(),
        Ty::Integer => "long".into(),
        Ty::Number => "double".into(),
        Ty::Boolean => "bool".into(),
        Ty::Enum(id) | Ty::Object(id) => model.decl(*id).name().to_string(),
        Ty::Array(inner) => format!("IReadOnlyList<{}>", ty(model, inner)),
        Ty::Map(inner) => format!("IReadOnlyDictionary<string, {}>", ty(model, inner)),
        Ty::Json => "JsonElement".into(),
    }
}

/// 字段类型（含可空标记）。
pub fn field_ty(model: &Model, f: &Field) -> String {
    let t = ty(model, &f.ty);
    if f.optional() { format!("{t}?") } else { t }
}

/// 对象各字段的 C# 属性名（PascalCase，去重，避免与类型名相同）。
pub fn property_names(o: &ObjectDecl) -> Vec<String> {
    let mut scope = NameScope::with_reserved(&[o.name.as_str()]);
    o.fields
        .iter()
        .map(|f| {
            ident::escape(
                Lang::CSharp,
                &scope.claim(&ident::pascal(&f.json_name, "Field")),
            )
        })
        .collect()
}

/// 枚举成员名。
pub fn enum_members(e: &EnumDecl) -> Vec<String> {
    let mut scope = NameScope::with_reserved(&[e.name.as_str()]);
    e.values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let base = ident::pascal(v, &format!("Value{}", i + 1));
            ident::escape(Lang::CSharp, &scope.claim(&base))
        })
        .collect()
}

pub fn method_name(tool_pascal: &str) -> String {
    format!("{tool_pascal}Async")
}

fn emit_enum(c: &mut Code, e: &EnumDecl) {
    c.xml_doc(&decl_doc(e.description.as_deref()));
    c.line(format!(
        "[JsonConverter(typeof(JsonStringEnumConverter<{}>))]",
        e.name
    ));
    c.line(format!("public enum {}", e.name));
    c.open("{");
    for (value, member) in e.values.iter().zip(enum_members(e)) {
        c.line(format!(
            "[JsonStringEnumMemberName({})] {member},",
            string_literal(Lang::CSharp, value)
        ));
    }
    c.close("}");
}

fn emit_object(c: &mut Code, model: &Model, o: &ObjectDecl) {
    c.xml_doc(&decl_doc(o.description.as_deref()));
    c.line(format!("public sealed record {}", o.name));
    c.open("{");
    for (i, (f, prop)) in o.fields.iter().zip(property_names(o)).enumerate() {
        if i > 0 {
            c.blank();
        }
        c.xml_doc(&field_doc(f));
        c.line(format!(
            "[JsonPropertyName({})]",
            string_literal(Lang::CSharp, &f.json_name)
        ));
        let required = if f.required { "required " } else { "" };
        c.line(format!(
            "public {required}{} {prop} {{ get; init; }}",
            field_ty(model, f)
        ));
    }
    c.close("}");
}

/// 参数类型与 handler 接口（不含文件头与命名空间），供 windows target 复用。
pub fn emit_body(c: &mut Code, model: &Model) {
    let m = &model.module;
    for decl in &model.types {
        match decl {
            TypeDecl::Enum(e) => emit_enum(c, e),
            TypeDecl::Object(o) => emit_object(c, model, o),
        }
        c.blank();
    }

    c.xml_doc(&[
        format!(
            "{} 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，",
            model.app_name
        ),
        "返回值会序列化为 JSON 作为工具结果。".to_string(),
    ]);
    c.line(format!("public interface I{m}ToolHandlers"));
    c.open("{");
    for (i, tool) in model.tools.iter().enumerate() {
        if i > 0 {
            c.blank();
        }
        c.xml_doc(&tool_doc(tool));
        c.line(format!(
            "Task<object?> {}({} args, CancellationToken cancellationToken);",
            method_name(&tool.pascal),
            model.params(tool).name
        ));
    }
    c.close("}");
    c.blank();

    c.xml_doc(&["工具名与分派辅助。".to_string()]);
    c.line(format!("public static class {m}Tools"));
    c.open("{");
    c.xml_doc(&["清单中的全部工具名。".to_string()]);
    c.line("public static readonly IReadOnlyList<string> Names =");
    c.open("[");
    for tool in &model.tools {
        c.line(format!(
            "{},",
            string_literal(Lang::CSharp, &tool.info.name)
        ));
    }
    c.close("];");
    c.blank();
    c.xml_doc(&["反序列化参数使用的选项（属性名由特性指定，这里只放宽数字处理）。".to_string()]);
    c.line("public static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);");
    c.blank();
    c.xml_doc(&["按工具名把调用分派到对应的 handler。".to_string()]);
    c.line(format!(
        "public static Task<object?> DispatchAsync(I{m}ToolHandlers handlers, string name, JsonElement arguments, CancellationToken cancellationToken = default) =>"
    ));
    c.indent();
    c.line("name switch");
    c.open("{");
    for tool in &model.tools {
        c.line(format!(
            "{} => handlers.{}(Parse<{}>(arguments), cancellationToken),",
            string_literal(Lang::CSharp, &tool.info.name),
            method_name(&tool.pascal),
            model.params(tool).name
        ));
    }
    c.line("_ => throw new ArgumentException($\"未知工具：{name}\", nameof(name)),");
    c.close("};");
    c.dedent();
    c.blank();
    c.xml_doc(&["把参数 JSON 反序列化为参数类型；缺省或 null 视为空对象。".to_string()]);
    c.line("public static T Parse<T>(JsonElement arguments)");
    c.open("{");
    c.line("var value = arguments.ValueKind is JsonValueKind.Undefined or JsonValueKind.Null");
    c.indent();
    c.line("? JsonSerializer.Deserialize<T>(\"{}\", JsonOptions)");
    c.line(": arguments.Deserialize<T>(JsonOptions);");
    c.dedent();
    c.line("return value ?? throw new JsonException($\"无法解析 {typeof(T).Name}\");");
    c.close("}");
    c.close("}");
}

pub const USINGS: &[&str] = &[
    "using System;",
    "using System.Collections.Generic;",
    "using System.Text.Json;",
    "using System.Text.Json.Serialization;",
    "using System.Threading;",
    "using System.Threading.Tasks;",
];

pub fn generate(model: &Model, namespace: &str) -> GeneratedFile {
    let mut c = Code::new("    ");
    c.line("// <auto-generated />");
    c.comment("// ", &header_lines(model, "csharp"));
    c.line("#nullable enable");
    c.blank();
    for u in USINGS {
        c.line(*u);
    }
    c.blank();
    c.line(format!("namespace {namespace};"));
    c.blank();
    emit_body(&mut c, model);
    file(file_name(model), c.finish())
}
