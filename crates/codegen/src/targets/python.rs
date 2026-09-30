//! Python（3.11+）：TypedDict 参数类型 + `Literal` 枚举 + `Protocol` handler + 分派函数。
//!
//! TypedDict 的键与 JSON 属性名一致；属性名不是合法标识符或是关键字时改用函数式写法。

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang};
use crate::schema::{Field, Model, ObjectDecl, Ty, TypeDecl, field_doc};
use crate::targets::{decl_doc, file, tool_doc};

pub fn file_name(model: &Model) -> String {
    format!("{}_tools.py", ident::snake(&model.module, "app"))
}

pub fn ty(model: &Model, t: &Ty) -> String {
    match t {
        Ty::String => "str".into(),
        Ty::Integer => "int".into(),
        Ty::Number => "float".into(),
        Ty::Boolean => "bool".into(),
        Ty::Enum(id) | Ty::Object(id) => model.decl(*id).name().to_string(),
        Ty::Array(inner) => format!("list[{}]", ty(model, inner)),
        Ty::Map(inner) => format!("dict[str, {}]", ty(model, inner)),
        Ty::Json => "Any".into(),
    }
}

fn field_ty(model: &Model, f: &Field) -> String {
    let mut t = ty(model, &f.ty);
    if f.nullable {
        t = format!("{t} | None");
    }
    if !f.required {
        t = format!("NotRequired[{t}]");
    }
    t
}

fn docstring(c: &mut Code, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let escaped: Vec<String> = lines
        .iter()
        .map(|l| l.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\""))
        .collect();
    if escaped.len() == 1 {
        c.line(format!("\"\"\"{}\"\"\"", escaped[0]));
    } else {
        c.line(format!("\"\"\"{}", escaped[0]));
        for l in &escaped[1..] {
            c.line(l);
        }
        c.line("\"\"\"");
    }
}

fn emit_object(c: &mut Code, model: &Model, o: &ObjectDecl) {
    let class_syntax = o.fields.iter().all(|f| {
        ident::is_plain_identifier(&f.json_name) && !ident::is_reserved(Lang::Python, &f.json_name)
    });
    if class_syntax {
        c.open(format!("class {}(TypedDict):", o.name));
        let doc = decl_doc(o.description.as_deref());
        docstring(c, &doc);
        if o.fields.is_empty() && doc.is_empty() {
            c.line("pass");
        }
        for f in &o.fields {
            c.line(format!("{}: {}", f.json_name, field_ty(model, f)));
            docstring(c, &field_doc(f));
        }
        c.dedent();
    } else {
        c.comment("# ", &decl_doc(o.description.as_deref()));
        c.open(format!("{} = TypedDict(", o.name));
        c.line(format!("{},", string_literal(Lang::Python, &o.name)));
        c.open("{");
        for f in &o.fields {
            c.comment("# ", &field_doc(f));
            c.line(format!(
                "{}: {},",
                string_literal(Lang::Python, &f.json_name),
                field_ty(model, f)
            ));
        }
        c.close("},");
        c.close(")");
    }
}

pub fn generate(model: &Model) -> GeneratedFile {
    let m = &model.module;
    let snake_module = ident::snake(m, "app");
    let mut c = Code::new("    ");
    let mut header = header_lines(model, "python");
    header.push(String::new());
    header.push("需要 Python 3.11+（typing.NotRequired）。".to_string());
    docstring(&mut c, &header);
    c.blank();
    c.line("from collections.abc import Awaitable, Mapping");
    c.line("from typing import Any, Literal, NotRequired, Protocol, TypedDict, cast");
    c.blank();
    c.line(format!(
        "__all__ = [{}]",
        model
            .types
            .iter()
            .map(|t| string_literal(Lang::Python, t.name()))
            .chain([
                string_literal(Lang::Python, &format!("{m}ToolHandlers")),
                string_literal(
                    Lang::Python,
                    &format!("{}_TOOL_NAMES", snake_module.to_uppercase())
                ),
                string_literal(Lang::Python, &format!("dispatch_{snake_module}_tool")),
            ])
            .collect::<Vec<_>>()
            .join(", ")
    ));
    c.blank();
    c.blank();

    for decl in &model.types {
        match decl {
            TypeDecl::Enum(e) => {
                c.comment("# ", &decl_doc(e.description.as_deref()));
                let values: Vec<String> = e
                    .values
                    .iter()
                    .map(|v| string_literal(Lang::Python, v))
                    .collect();
                c.line(format!("{} = Literal[{}]", e.name, values.join(", ")));
            }
            TypeDecl::Object(o) => emit_object(&mut c, model, o),
        }
        c.blank();
        c.blank();
    }

    c.open(format!("class {m}ToolHandlers(Protocol):"));
    docstring(
        &mut c,
        &[
            format!(
                "{} 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数。",
                model.app_name
            ),
            String::new(),
            "方法可以是普通函数或 async 函数；返回值作为工具结果（JSON）。".to_string(),
        ],
    );
    for tool in &model.tools {
        c.blank();
        c.open(format!(
            "def {}(self, params: {}) -> Any | Awaitable[Any]:",
            ident::escape(Lang::Python, &tool.snake),
            model.params(tool).name
        ));
        docstring(&mut c, &tool_doc(tool));
        c.line("...");
        c.dedent();
    }
    c.dedent();
    c.blank();
    c.blank();

    c.line(format!(
        "{}_TOOL_NAMES: tuple[str, ...] = (",
        snake_module.to_uppercase()
    ));
    c.indent();
    for tool in &model.tools {
        c.line(format!(
            "{},",
            string_literal(Lang::Python, &tool.info.name)
        ));
    }
    c.dedent();
    c.line(")");
    c.line("\"\"\"清单中的全部工具名。\"\"\"");
    c.blank();
    c.blank();
    c.open(format!(
        "def dispatch_{snake_module}_tool(handlers: {m}ToolHandlers, name: str, arguments: Mapping[str, Any] | None) -> Any:"
    ));
    docstring(
        &mut c,
        &["按工具名把调用分派到对应的 handler。参数应已由 Host 按 inputSchema 校验。".to_string()],
    );
    c.line("args: Any = dict(arguments or {})");
    for tool in &model.tools {
        c.open(format!(
            "if name == {}:",
            string_literal(Lang::Python, &tool.info.name)
        ));
        c.line(format!(
            "return handlers.{}(cast({}, args))",
            ident::escape(Lang::Python, &tool.snake),
            model.params(tool).name
        ));
        c.dedent();
    }
    c.line("raise KeyError(f\"未知工具：{name}\")");
    c.dedent();

    file(file_name(model), c.finish())
}
