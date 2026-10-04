//! TypeScript：参数 interface（属性名与 JSON 一致）+ handler 接口 + 分派函数。

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::deprecation;
use crate::ident::{self, Lang};
use crate::schema::{Field, Model, Ty, TypeDecl, field_doc};
use crate::targets::{decl_doc, file, tool_doc};

pub fn file_name(model: &Model) -> String {
    format!("{}Tools.ts", ident::camel(&model.module, "app"))
}

pub fn ty(model: &Model, t: &Ty) -> String {
    match t {
        Ty::String => "string".into(),
        Ty::Integer | Ty::Number => "number".into(),
        Ty::Boolean => "boolean".into(),
        Ty::Enum(id) | Ty::Object(id) => model.decl(*id).name().to_string(),
        Ty::Array(inner) => format!("{}[]", ty(model, inner)),
        Ty::Map(inner) => format!("Record<string, {}>", ty(model, inner)),
        Ty::Json => "unknown".into(),
    }
}

fn property_key(name: &str) -> String {
    if ident::is_plain_identifier(name) {
        name.to_string()
    } else {
        string_literal(Lang::TypeScript, name)
    }
}

fn field_line(model: &Model, f: &Field) -> String {
    let mut t = ty(model, &f.ty);
    if f.nullable {
        t.push_str(" | null");
    }
    let q = if f.required { "" } else { "?" };
    format!("{}{q}: {t};", property_key(&f.json_name))
}

pub fn generate(model: &Model) -> GeneratedFile {
    let m = &model.module;
    let mut c = Code::new("  ");
    c.comment("// ", &header_lines(model, "typescript"));
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
                    c.line(format!("export type {} = Record<string, never>;", o.name));
                } else {
                    c.open(format!("export interface {} {{", o.name));
                    for f in &o.fields {
                        let mut doc = field_doc(f);
                        if f.deprecated {
                            doc.push(format!("@deprecated {}", deprecation::FIELD_MESSAGE));
                        }
                        c.block_doc(&doc);
                        c.line(field_line(model, f));
                    }
                    c.close("}");
                }
                c.blank();
            }
        }
    }

    // handler 接口
    c.block_doc(&[
        format!(
            "{} 的工具实现。每个方法接收已由 Host 按 inputSchema 校验过的参数。",
            model.app_name
        ),
        "返回值会序列化为 JSON 作为工具结果。".to_string(),
    ]);
    c.open(format!("export interface {m}ToolHandlers {{"));
    for tool in &model.tools {
        let mut doc = tool_doc(tool);
        if let Some(message) = deprecation::tool_deprecation(tool) {
            doc.extend(deprecation::prefixed_lines("@deprecated ", &message));
        }
        c.block_doc(&doc);
        let params = model.params(tool);
        c.line(format!(
            "{}(params: {}): unknown | Promise<unknown>;",
            tool.camel, params.name
        ));
    }
    c.close("}");
    c.blank();

    // 工具名与参数映射
    let camel_module = ident::camel(m, "app");
    c.line("/** 清单中的全部工具名。 */");
    c.open(format!("export const {camel_module}ToolNames = ["));
    for tool in &model.tools {
        c.line(format!(
            "{},",
            string_literal(Lang::TypeScript, &tool.info.name)
        ));
    }
    c.close("] as const;");
    c.blank();
    c.line(format!(
        "export type {m}ToolName = (typeof {camel_module}ToolNames)[number];"
    ));
    c.blank();
    c.line("/** 工具名 → 参数类型。 */");
    c.open(format!("export interface {m}ToolParams {{"));
    for tool in &model.tools {
        c.line(format!(
            "{}: {};",
            string_literal(Lang::TypeScript, &tool.info.name),
            model.params(tool).name
        ));
    }
    c.close("}");
    c.blank();

    // 分派
    c.block_doc(&[
        "按工具名把调用分派到对应的 handler。".to_string(),
        "参数应已由 Host 按 inputSchema 校验；这里只做类型断言。".to_string(),
    ]);
    c.open(format!(
        "export function dispatch{m}Tool(handlers: {m}ToolHandlers, name: string, args: unknown): unknown | Promise<unknown> {{"
    ));
    c.line("const params: unknown = args ?? {};");
    c.open("switch (name) {");
    for tool in &model.tools {
        c.line(format!(
            "case {}:",
            string_literal(Lang::TypeScript, &tool.info.name)
        ));
        c.indent();
        c.line(format!(
            "return handlers.{}(params as {});",
            tool.camel,
            model.params(tool).name
        ));
        c.dedent();
    }
    c.line("default:");
    c.indent();
    c.line("throw new Error(`未知工具：${name}`);");
    c.dedent();
    c.close("}");
    c.close("}");

    file(file_name(model), c.finish())
}
