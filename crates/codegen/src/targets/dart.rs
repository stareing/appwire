//! Dart：参数 class（`fromJson` / `toJson`，只依赖 dart:core / dart:async）+ 增强枚举 +
//! 抽象 handler + 分派函数。

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang, NameScope};
use crate::schema::{EnumDecl, Field, Model, ObjectDecl, Ty, TypeDecl, field_doc};
use crate::targets::{decl_doc, file, tool_doc};

pub fn file_name(model: &Model) -> String {
    format!("{}_tools.dart", ident::snake(&model.module, "app"))
}

pub fn ty(model: &Model, t: &Ty) -> String {
    match t {
        Ty::String => "String".into(),
        Ty::Integer => "int".into(),
        Ty::Number => "double".into(),
        Ty::Boolean => "bool".into(),
        Ty::Enum(id) | Ty::Object(id) => model.decl(*id).name().to_string(),
        Ty::Array(inner) => format!("List<{}>", ty(model, inner)),
        Ty::Map(inner) => format!("Map<String, {}>", ty(model, inner)),
        Ty::Json => "Object?".into(),
    }
}

fn field_ty(model: &Model, f: &Field) -> String {
    let t = ty(model, &f.ty);
    if f.optional() && f.ty != Ty::Json {
        format!("{t}?")
    } else {
        t
    }
}

const MEMBER_RESERVED: &[&str] = &[
    "hashCode",
    "runtimeType",
    "toString",
    "noSuchMethod",
    "toJson",
    "fromJson",
];

fn property_names(o: &ObjectDecl) -> Vec<String> {
    let mut scope = NameScope::with_reserved(MEMBER_RESERVED);
    o.fields
        .iter()
        .map(|f| {
            ident::escape(
                Lang::Dart,
                &scope.claim(&ident::camel(&f.json_name, "field")),
            )
        })
        .collect()
}

fn enum_constants(e: &EnumDecl) -> Vec<String> {
    let mut scope = NameScope::with_reserved(&[
        "values",
        "index",
        "hashCode",
        "name",
        "value",
        "runtimeType",
        "toString",
        "fromJson",
        "toJson",
        "noSuchMethod",
    ]);
    e.values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            ident::escape(
                Lang::Dart,
                &scope.claim(&ident::camel(v, &format!("value{}", i + 1))),
            )
        })
        .collect()
}

/// 解码表达式：把 `expr`（dynamic）转换为字段类型。`nullable` 时结果可为 null。
fn decode(model: &Model, t: &Ty, expr: &str, nullable: bool, depth: usize) -> String {
    let q = if nullable { "?" } else { "" };
    let v = format!("e{depth}");
    match t {
        Ty::String => format!("{expr} as String{q}"),
        Ty::Boolean => format!("{expr} as bool{q}"),
        Ty::Integer => format!("({expr} as num{q}){q}.toInt()"),
        Ty::Number => format!("({expr} as num{q}){q}.toDouble()"),
        Ty::Enum(id) | Ty::Object(id) => {
            let name = model.decl(*id).name();
            let arg = if matches!(t, Ty::Enum(_)) {
                format!("{expr} as String")
            } else {
                format!("{expr} as Map<String, dynamic>")
            };
            if nullable {
                format!("{expr} == null ? null : {name}.fromJson({arg})")
            } else {
                format!("{name}.fromJson({arg})")
            }
        }
        Ty::Array(inner) => {
            let item = decode(model, inner, &v, false, depth + 1);
            if item == format!("{v} as {}", ty(model, inner)) {
                format!(
                    "({expr} as List<dynamic>{q}){q}.cast<{}>()",
                    ty(model, inner)
                )
            } else {
                format!("({expr} as List<dynamic>{q}){q}.map(({v}) => {item}).toList()")
            }
        }
        Ty::Map(inner) => {
            let item = decode(model, inner, &v, false, depth + 1);
            format!(
                "({expr} as Map<String, dynamic>{q}){q}.map((k{depth}, {v}) => MapEntry(k{depth}, {item}))"
            )
        }
        Ty::Json => expr.to_string(),
    }
}

/// 编码表达式：`expr` 已确定非 null。
fn encode(t: &Ty, expr: &str, depth: usize) -> String {
    let v = format!("e{depth}");
    match t {
        Ty::String | Ty::Integer | Ty::Number | Ty::Boolean | Ty::Json => expr.to_string(),
        Ty::Enum(_) => format!("{expr}.value"),
        Ty::Object(_) => format!("{expr}.toJson()"),
        Ty::Array(inner) => {
            let item = encode(inner, &v, depth + 1);
            if item == v {
                expr.to_string()
            } else {
                format!("{expr}.map(({v}) => {item}).toList()")
            }
        }
        Ty::Map(inner) => {
            let item = encode(inner, &v, depth + 1);
            if item == v {
                expr.to_string()
            } else {
                format!("{expr}.map((k{depth}, {v}) => MapEntry(k{depth}, {item}))")
            }
        }
    }
}

fn emit_enum(c: &mut Code, e: &EnumDecl) {
    c.comment("/// ", &decl_doc(e.description.as_deref()));
    c.open(format!("enum {} {{", e.name));
    let consts = enum_constants(e);
    for (i, (value, name)) in e.values.iter().zip(&consts).enumerate() {
        let end = if i + 1 == consts.len() { ";" } else { "," };
        c.line(format!(
            "{name}({}){end}",
            string_literal(Lang::Dart, value)
        ));
    }
    c.blank();
    c.line(format!("const {}(this.value);", e.name));
    c.blank();
    c.line("/// JSON 中的取值。");
    c.line("final String value;");
    c.blank();
    c.line(format!(
        "static {} fromJson(String value) => values.firstWhere(",
        e.name
    ));
    c.indent();
    c.line("(e) => e.value == value,");
    c.line("orElse: () => throw ArgumentError.value(value, 'value', '未知取值'),");
    c.dedent();
    c.line(");");
    c.blank();
    c.line("String toJson() => value;");
    c.close("}");
}

fn emit_object(c: &mut Code, model: &Model, o: &ObjectDecl) {
    c.comment("/// ", &decl_doc(o.description.as_deref()));
    c.open(format!("class {} {{", o.name));
    let names = property_names(o);
    if o.fields.is_empty() {
        c.line(format!("const {}();", o.name));
        c.blank();
        c.line(format!(
            "factory {}.fromJson(Map<String, dynamic> json) => const {}();",
            o.name, o.name
        ));
        c.blank();
        c.line("Map<String, dynamic> toJson() => <String, dynamic>{};");
        c.close("}");
        return;
    }
    c.open(format!("const {}({{", o.name));
    for (f, name) in o.fields.iter().zip(&names) {
        let req = if f.optional() { "" } else { "required " };
        c.line(format!("{req}this.{name},"));
    }
    c.close("});");
    c.blank();
    c.line(format!(
        "factory {}.fromJson(Map<String, dynamic> json) => {}(",
        o.name, o.name
    ));
    c.indent();
    for (f, name) in o.fields.iter().zip(&names) {
        let expr = format!("json[{}]", string_literal(Lang::Dart, &f.json_name));
        c.line(format!(
            "{name}: {},",
            decode(model, &f.ty, &expr, f.optional(), 0)
        ));
    }
    c.dedent();
    c.line(");");
    for (f, name) in o.fields.iter().zip(&names) {
        c.blank();
        c.comment("/// ", &field_doc(f));
        c.line(format!("final {} {name};", field_ty(model, f)));
    }
    c.blank();
    c.open("Map<String, dynamic> toJson() => <String, dynamic>{");
    for (f, name) in o.fields.iter().zip(&names) {
        let key = string_literal(Lang::Dart, &f.json_name);
        if f.optional() && f.ty != Ty::Json {
            let bang = format!("{name}!");
            let value = encode(&f.ty, &bang, 0);
            let value = if value == bang { name.clone() } else { value };
            if f.required {
                // 必填但可为 null：总是输出键
                c.line(format!(
                    "{key}: {name} == null ? null : {},",
                    encode(&f.ty, &bang, 0)
                ));
            } else {
                c.line(format!("if ({name} != null) {key}: {value},"));
            }
        } else if f.ty == Ty::Json && !f.required {
            c.line(format!("if ({name} != null) {key}: {name},"));
        } else {
            c.line(format!("{key}: {},", encode(&f.ty, name, 0)));
        }
    }
    c.close("};");
    c.close("}");
}

pub fn generate(model: &Model) -> GeneratedFile {
    let m = &model.module;
    let camel_module = ident::camel(m, "app");
    let mut c = Code::new("  ");
    c.comment("// ", &header_lines(model, "dart"));
    c.blank();
    c.line("import 'dart:async';");
    c.blank();

    for decl in &model.types {
        match decl {
            TypeDecl::Enum(e) => emit_enum(&mut c, e),
            TypeDecl::Object(o) => emit_object(&mut c, model, o),
        }
        c.blank();
    }

    c.comment(
        "/// ",
        &[
            format!(
                "{} 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数，",
                model.app_name
            ),
            "返回值作为工具结果（可 JSON 编码的值）。".to_string(),
        ],
    );
    c.open(format!("abstract interface class {m}ToolHandlers {{"));
    for (i, tool) in model.tools.iter().enumerate() {
        if i > 0 {
            c.blank();
        }
        c.comment("/// ", &tool_doc(tool));
        c.line(format!(
            "FutureOr<Object?> {}({} params);",
            ident::escape(Lang::Dart, &tool.camel),
            model.params(tool).name
        ));
    }
    c.close("}");
    c.blank();

    c.line("/// 清单中的全部工具名。");
    c.open(format!(
        "const List<String> {camel_module}ToolNames = <String>["
    ));
    for tool in &model.tools {
        c.line(format!("{},", string_literal(Lang::Dart, &tool.info.name)));
    }
    c.close("];");
    c.blank();
    c.line("/// 按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。");
    c.open(format!(
        "FutureOr<Object?> dispatch{m}Tool({m}ToolHandlers handlers, String name, Map<String, dynamic>? arguments) {{"
    ));
    c.line("final args = arguments ?? const <String, dynamic>{};");
    c.open("switch (name) {");
    for tool in &model.tools {
        c.line(format!(
            "case {}:",
            string_literal(Lang::Dart, &tool.info.name)
        ));
        c.indent();
        c.line(format!(
            "return handlers.{}({}.fromJson(args));",
            ident::escape(Lang::Dart, &tool.camel),
            model.params(tool).name
        ));
        c.dedent();
    }
    c.close("}");
    c.line("throw ArgumentError.value(name, 'name', '未知工具');");
    c.close("}");

    file(file_name(model), c.finish())
}
