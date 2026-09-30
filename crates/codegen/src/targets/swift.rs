//! Swift：Codable struct / enum + `<Module>ToolHandlers` 协议 + 分派辅助。
//!
//! swift-app-intents 会原样输出同一个文件，并在另一个文件中引用这里的类型。

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang, NameScope};
use crate::schema::{EnumDecl, Field, Model, ObjectDecl, Ty, TypeDecl, field_doc};
use crate::targets::{decl_doc, file, tool_doc};

pub fn file_name(model: &Model) -> String {
    format!("{}Tools.swift", model.module)
}

pub fn ty(model: &Model, t: &Ty) -> String {
    match t {
        Ty::String => "String".into(),
        Ty::Integer => "Int".into(),
        Ty::Number => "Double".into(),
        Ty::Boolean => "Bool".into(),
        Ty::Enum(id) | Ty::Object(id) => model.decl(*id).name().to_string(),
        Ty::Array(inner) => format!("[{}]", ty(model, inner)),
        Ty::Map(inner) => format!("[String: {}]", ty(model, inner)),
        Ty::Json => "JSONValue".into(),
    }
}

pub fn field_ty(model: &Model, f: &Field) -> String {
    let t = ty(model, &f.ty);
    if f.optional() { format!("{t}?") } else { t }
}

/// 对象各字段的 Swift 属性名（camelCase，未转义，去重）。
pub fn property_names(o: &ObjectDecl) -> Vec<String> {
    let mut scope = NameScope::new();
    o.fields
        .iter()
        .map(|f| scope.claim(&ident::camel(&f.json_name, "field")))
        .collect()
}

/// 枚举 case 名（未转义）。
pub fn enum_cases(e: &EnumDecl) -> Vec<String> {
    let mut scope = NameScope::new();
    e.values
        .iter()
        .enumerate()
        .map(|(i, v)| scope.claim(&ident::camel(v, &format!("value{}", i + 1))))
        .collect()
}

fn emit_enum(c: &mut Code, e: &EnumDecl) {
    c.comment("/// ", &decl_doc(e.description.as_deref()));
    c.open(format!(
        "public enum {}: String, Codable, Sendable, CaseIterable {{",
        e.name
    ));
    for (value, case) in e.values.iter().zip(enum_cases(e)) {
        c.line(format!(
            "case {} = {}",
            ident::escape(Lang::Swift, &case),
            string_literal(Lang::Swift, value)
        ));
    }
    c.close("}");
}

fn emit_object(c: &mut Code, model: &Model, o: &ObjectDecl) {
    c.comment("/// ", &decl_doc(o.description.as_deref()));
    c.open(format!(
        "public struct {}: Codable, Sendable, Equatable {{",
        o.name
    ));
    let names = property_names(o);
    for (f, name) in o.fields.iter().zip(&names) {
        c.comment("/// ", &field_doc(f));
        c.line(format!(
            "public var {}: {}",
            ident::escape(Lang::Swift, name),
            field_ty(model, f)
        ));
    }
    if !o.fields.is_empty() {
        c.blank();
    }
    // 逐字段初始化器：可选字段默认 nil
    let params: Vec<String> = o
        .fields
        .iter()
        .zip(&names)
        .map(|(f, name)| {
            let default = if f.optional() { " = nil" } else { "" };
            format!(
                "{}: {}{default}",
                ident::escape(Lang::Swift, name),
                field_ty(model, f)
            )
        })
        .collect();
    if params.is_empty() {
        c.line("public init() {}");
    } else {
        c.open(format!("public init({}) {{", params.join(", ")));
        for name in &names {
            c.line(format!(
                "self.{name} = {}",
                ident::escape(Lang::Swift, name)
            ));
        }
        c.close("}");
        c.blank();
        c.open("enum CodingKeys: String, CodingKey {");
        for (f, name) in o.fields.iter().zip(&names) {
            c.line(format!(
                "case {} = {}",
                ident::escape(Lang::Swift, name),
                string_literal(Lang::Swift, &f.json_name)
            ));
        }
        c.close("}");
    }
    c.close("}");
}

const JSON_VALUE: &str = r#"/// 任意 JSON 值（用于 schema 未约束或不支持的构造）。
public enum JSONValue: Codable, Sendable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        }
    }
}"#;

pub fn generate(model: &Model) -> GeneratedFile {
    let m = &model.module;
    let mut c = Code::new("    ");
    c.comment("// ", &header_lines(model, "swift"));
    c.blank();
    c.line("import Foundation");
    c.blank();

    for decl in &model.types {
        match decl {
            TypeDecl::Enum(e) => emit_enum(&mut c, e),
            TypeDecl::Object(o) => emit_object(&mut c, model, o),
        }
        c.blank();
    }
    if model.uses_json() {
        for l in JSON_VALUE.lines() {
            c.line(l);
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
            "返回值会编码为 JSON 作为工具结果（返回 String 时原样作为文本）。".to_string(),
        ],
    );
    c.open(format!("public protocol {m}ToolHandlers: Sendable {{"));
    for tool in &model.tools {
        c.comment("/// ", &tool_doc(tool));
        c.line(format!(
            "func {}(_ params: {}) async throws -> any Encodable & Sendable",
            ident::escape(Lang::Swift, &tool.camel),
            model.params(tool).name
        ));
    }
    c.close("}");
    c.blank();

    c.line(format!("public enum {m}ToolError: Error, Equatable {{"));
    c.indent();
    c.line("/// 未知的工具名。");
    c.line("case unknownTool(String)");
    c.line("/// 尚未设置 handler（原生意图框架调用时）。");
    c.line("case handlersNotSet");
    c.close("}");
    c.blank();

    c.comment("/// ", &["工具名与分派辅助。".to_string()]);
    c.open(format!("public enum {m}Tools {{"));
    c.line("/// 清单中的全部工具名。");
    c.open("public static let names: [String] = [");
    for tool in &model.tools {
        c.line(format!("{},", string_literal(Lang::Swift, &tool.info.name)));
    }
    c.close("]");
    c.blank();
    c.line("/// 按工具名把调用分派到对应的 handler。`arguments` 为参数 JSON（空数据视为 `{}`）。");
    c.open(format!(
        "public static func dispatch(_ handlers: any {m}ToolHandlers, name: String, arguments: Data) async throws -> any Encodable & Sendable {{"
    ));
    c.line("let data = arguments.isEmpty ? Data(\"{}\".utf8) : arguments");
    c.line("let decoder = JSONDecoder()");
    c.line("switch name {");
    for tool in &model.tools {
        c.line(format!(
            "case {}:",
            string_literal(Lang::Swift, &tool.info.name)
        ));
        c.indent();
        c.line(format!(
            "return try await handlers.{}(decoder.decode({}.self, from: data))",
            ident::escape(Lang::Swift, &tool.camel),
            model.params(tool).name
        ));
        c.dedent();
    }
    c.line("default:");
    c.indent();
    c.line(format!("throw {m}ToolError.unknownTool(name)"));
    c.dedent();
    c.line("}");
    c.close("}");
    c.close("}");

    file(file_name(model), c.finish())
}
