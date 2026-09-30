//! Apple App Intents（Swift）：每个工具一个 `AppIntent`，`perform()` 调用 `<Module>ToolHandlers`。
//!
//! 输出两个文件：
//! - `<Module>Tools.swift`：与 `swift` target 完全相同（参数类型 + handler 协议）；
//! - `<Module>AppIntents.swift`：`AppEnum` 扩展、各 `AppIntent`、`AppShortcutsProvider`。
//!
//! 参数映射：string / integer / number / boolean / 字符串枚举（→ `AppEnum`）/ 标量数组直接作为
//! `@Parameter`；`format: date-time` / `date` 的字符串映射为 `Date`；嵌套对象、对象数组、字典与
//! 原始 JSON 降级为 JSON 字符串参数，在 `perform()` 中解码为对应的 Codable 类型。
//!
//! 依据（2026-09）：<https://developer.apple.com/documentation/appintents/appintent>、
//! `requestConfirmation(conditions:actionName:dialog:)`（iOS 18+）、
//! `supportedModes: IntentModes`（iOS 26+，取代已弃用的 `openAppWhenRun`）、
//! `AppShortcut(intent:phrases:shortTitle:systemImageName:)`。

use app_mcp_protocol::{Activation, Risk};

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang, NameScope};
use crate::schema::{Field, Model, ToolModel, Ty, TypeDecl, Warning, field_notes};
use crate::targets::{file, swift};

/// App Shortcuts 的数量上限（系统限制每个 App 最多 10 个）。
pub const MAX_APP_SHORTCUTS: usize = 10;

pub fn generate(model: &Model, warnings: &mut Vec<Warning>) -> Vec<GeneratedFile> {
    vec![
        swift::generate(model),
        file(
            format!("{}AppIntents.swift", model.module),
            intents_file(model, warnings),
        ),
    ]
}

/// 一个工具参数在 App Intent 中的表示。
enum ParamKind {
    /// 类型可以直接作为 `@Parameter`。
    Direct,
    /// `Date` 参数，转换为 ISO 8601 字符串（`date_only` 时只取日期）。
    Date { date_only: bool },
    /// 降级为 JSON 字符串参数，在 perform 中解码。
    JsonString,
}

fn param_kind(f: &Field) -> ParamKind {
    let fmt = f.constraints.format.as_deref();
    match &f.ty {
        Ty::String if fmt == Some("date-time") => ParamKind::Date { date_only: false },
        Ty::String if fmt == Some("date") => ParamKind::Date { date_only: true },
        t if t.is_scalar() => ParamKind::Direct,
        Ty::Array(inner) if inner.is_scalar() => ParamKind::Direct,
        _ => ParamKind::JsonString,
    }
}

fn confirmation_action(risk: Risk) -> &'static str {
    match risk {
        Risk::Payment => ".pay",
        _ => ".`continue`",
    }
}

fn shortcut_image(risk: Risk) -> &'static str {
    match risk {
        Risk::Read => "magnifyingglass",
        _ => "square.and.pencil",
    }
}

fn swift_default(f: &Field) -> Option<String> {
    let d = f.constraints.default.as_ref()?;
    match (&f.ty, d) {
        (Ty::String, serde_json::Value::String(s)) => Some(string_literal(Lang::Swift, s)),
        (Ty::Integer, serde_json::Value::Number(n)) if n.is_i64() || n.is_u64() => {
            Some(n.to_string())
        }
        (Ty::Number, serde_json::Value::Number(n)) => Some(n.to_string()),
        (Ty::Boolean, serde_json::Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// 参数的描述：原描述 + 约束说明（JSON 字符串参数额外说明格式）。
fn param_description(f: &Field, kind: &ParamKind) -> Option<String> {
    let mut parts: Vec<String> = f.description.iter().cloned().collect();
    let notes = field_notes(f);
    if !notes.is_empty() {
        parts.push(notes.join("；"));
    }
    if matches!(kind, ParamKind::JsonString) {
        parts.push("以 JSON 字符串传入".to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("。"))
    }
}

/// 生成 `perform()` 中把 JSON 字符串解码为目标类型的表达式。
fn json_decode_expr(model: &Model, m: &str, f: &Field, var: &str) -> String {
    let target = swift::ty(model, &f.ty);
    if f.optional() {
        format!("try {m}IntentRuntime.decodeIfPresent({target}.self, from: {var})")
    } else {
        format!("try {m}IntentRuntime.decode({target}.self, from: {var})")
    }
}

fn intents_file(model: &Model, warnings: &mut Vec<Warning>) -> String {
    let m = &model.module;
    let mut c = Code::new("    ");
    let mut header = header_lines(model, "swift-app-intents");
    header.push(String::new());
    header.push(format!(
        "依赖同目录下的 {}Tools.swift（参数类型与 {m}ToolHandlers 协议）。",
        m
    ));
    header.push("需要 iOS / macOS 26 起的 App Intents（supportedModes）；".to_string());
    header.push(format!(
        "启动时设置 `{m}IntentRuntime.handlers = <实现 {m}ToolHandlers 的对象>`（可直接复用 MCP 的业务实现）。"
    ));
    if let Some(body) = model.overview.as_ref().and_then(|o| o.body.as_deref()) {
        header.push(String::new());
        header.extend(body.lines().map(str::to_string));
    }
    c.comment("// ", &header);
    c.blank();
    c.line("import AppIntents");
    c.line("import Foundation");
    c.blank();

    // AppEnum 扩展：复用 Tools.swift 中的枚举
    for decl in &model.types {
        let TypeDecl::Enum(e) = decl else { continue };
        c.open(format!("extension {}: AppEnum {{", e.name));
        let display = e.description.clone().unwrap_or_else(|| e.name.clone());
        c.line(format!(
            "public static let typeDisplayRepresentation = TypeDisplayRepresentation(name: {})",
            string_literal(Lang::Swift, &display)
        ));
        c.open(format!(
            "public static let caseDisplayRepresentations: [{}: DisplayRepresentation] = [",
            e.name
        ));
        for (value, case) in e.values.iter().zip(swift::enum_cases(e)) {
            c.line(format!(
                ".{}: DisplayRepresentation(title: {}),",
                ident::escape(Lang::Swift, &case),
                string_literal(Lang::Swift, value)
            ));
        }
        c.close("]");
        c.close("}");
        c.blank();
    }

    // 运行时：handler 注入与 JSON 辅助
    c.line(format!("/// App Intents 调用 {m}ToolHandlers 的入口。"));
    c.open(format!("public enum {m}IntentRuntime {{"));
    c.line(format!(
        "/// 由 App 在启动时设置（如 `App.init`）；未设置时 intent 抛出 `{m}ToolError.handlersNotSet`。"
    ));
    c.line(format!(
        "@MainActor public static var handlers: (any {m}ToolHandlers)?"
    ));
    c.blank();
    c.open(format!(
        "@MainActor static func requireHandlers() throws -> any {m}ToolHandlers {{"
    ));
    c.line(format!(
        "guard let handlers else {{ throw {m}ToolError.handlersNotSet }}"
    ));
    c.line("return handlers");
    c.close("}");
    c.blank();
    c.open("static func decode<T: Decodable>(_ type: T.Type, from json: String) throws -> T {");
    c.line("try JSONDecoder().decode(type, from: Data(json.utf8))");
    c.close("}");
    c.blank();
    c.open("static func decodeIfPresent<T: Decodable>(_ type: T.Type, from json: String?) throws -> T? {");
    c.line("guard let json, !json.isEmpty else { return nil }");
    c.line("return try decode(type, from: json)");
    c.close("}");
    c.blank();
    c.line("/// 把 handler 的返回值转为 Siri / 快捷指令展示的文本：String 原样返回，其他值编码为 JSON。");
    c.open("static func text(_ value: any Encodable & Sendable) throws -> String {");
    c.line("if let string = value as? String { return string }");
    c.line("let data = try JSONEncoder().encode(value)");
    c.line("return String(decoding: data, as: UTF8.self)");
    c.close("}");
    c.close("}");
    c.blank();

    let mut intent_names =
        NameScope::with_reserved(&model.types.iter().map(|t| t.name()).collect::<Vec<_>>());
    let mut shortcuts: Vec<(String, &ToolModel)> = Vec::new();
    for tool in &model.tools {
        let intent = intent_names.claim(&format!("{}Intent", tool.pascal));
        emit_intent(&mut c, model, tool, &intent);
        c.blank();
        let params = model.params(tool);
        let eligible = matches!(tool.info.risk, Risk::Read | Risk::Write)
            && params.fields.iter().all(|f| f.optional());
        if eligible {
            shortcuts.push((intent, tool));
        }
    }

    if shortcuts.len() > MAX_APP_SHORTCUTS {
        warnings.push(Warning {
            tool: String::new(),
            path: String::new(),
            message: format!(
                "swift-app-intents：符合条件的 App Shortcut 有 {} 个，超过系统上限 {MAX_APP_SHORTCUTS}，只保留前 {MAX_APP_SHORTCUTS} 个",
                shortcuts.len()
            ),
        });
        shortcuts.truncate(MAX_APP_SHORTCUTS);
    }
    if !shortcuts.is_empty() {
        c.line("/// 只读 / 写入且没有必填参数的工具提供 App Shortcut（短语由工具描述生成，建议在 String Catalog 中本地化与精简）。");
        c.open(format!(
            "public struct {m}AppShortcuts: AppShortcutsProvider {{"
        ));
        c.open("public static var appShortcuts: [AppShortcut] {");
        for (intent, tool) in &shortcuts {
            // 插值 `\(.applicationName)` 手工拼接，描述部分走字面量转义（去掉两端引号）
            let literal = string_literal(Lang::Swift, &tool.info.description);
            let inner = &literal[1..literal.len() - 1];
            let phrase = format!("\\(.applicationName) {inner}");
            c.open("AppShortcut(");
            c.line(format!("intent: {intent}(),"));
            c.line(format!("phrases: [\"{phrase}\"],"));
            c.line(format!(
                "shortTitle: {},",
                string_literal(Lang::Swift, tool.display_title())
            ));
            c.line(format!(
                "systemImageName: {}",
                string_literal(Lang::Swift, shortcut_image(tool.info.risk))
            ));
            c.close(")");
        }
        c.close("}");
        c.close("}");
    }

    c.finish()
}

fn emit_intent(c: &mut Code, model: &Model, tool: &ToolModel, intent: &str) {
    let m = &model.module;
    let params = model.params(tool);
    let prop_names = swift::property_names(params);
    // 避免与 AppIntent 成员及生成代码中的局部名冲突
    const MEMBERS: &[&str] = &[
        "perform",
        "title",
        "description",
        "supportedModes",
        "parameterSummary",
        "params",
        "result",
        "handlers",
        "systemContext",
    ];
    let mut scope = NameScope::with_reserved(MEMBERS);
    let vars: Vec<String> = prop_names
        .iter()
        .map(|n| {
            if MEMBERS.contains(&n.as_str()) {
                scope.claim(&format!("{n}Value"))
            } else {
                scope.claim(n)
            }
        })
        .collect();

    c.comment("/// ", &crate::targets::tool_doc(tool));
    c.open(format!("public struct {intent}: AppIntent {{"));
    c.line(format!(
        "public static let title: LocalizedStringResource = {}",
        string_literal(Lang::Swift, tool.display_title())
    ));
    c.line(format!(
        "public static let description = IntentDescription({})",
        string_literal(Lang::Swift, &tool.info.description)
    ));
    let modes = match tool.info.activation.unwrap_or_default() {
        Activation::Foreground => ".foreground(.immediate)",
        Activation::Background | Activation::Headless => ".background",
    };
    c.line(format!(
        "public static let supportedModes: IntentModes = {modes}"
    ));
    c.blank();

    let kinds: Vec<ParamKind> = params.fields.iter().map(param_kind).collect();
    for ((f, var), kind) in params.fields.iter().zip(&vars).zip(&kinds) {
        let (base, note) = match kind {
            ParamKind::Direct => (swift::ty(model, &f.ty), None),
            ParamKind::Date { .. } => ("Date".to_string(), None),
            ParamKind::JsonString => (
                "String".to_string(),
                Some(format!(
                    "{} 不能直接作为 App Intent 参数，降级为 JSON 字符串",
                    swift::ty(model, &f.ty)
                )),
            ),
        };
        let ty = if f.optional() {
            format!("{base}?")
        } else {
            base
        };
        if let Some(note) = note {
            c.line(format!("// {note}"));
        }
        let mut args = vec![format!(
            "title: {}",
            string_literal(Lang::Swift, &f.json_name)
        )];
        if let Some(desc) = param_description(f, kind) {
            args.push(format!(
                "description: {}",
                string_literal(Lang::Swift, &desc)
            ));
        }
        if matches!(kind, ParamKind::Direct)
            && let Some(d) = swift_default(f)
        {
            args.push(format!("default: {d}"));
        }
        c.line(format!("@Parameter({})", args.join(", ")));
        c.line(format!(
            "public var {}: {ty}",
            ident::escape(Lang::Swift, var)
        ));
        c.blank();
    }
    c.line("public init() {}");
    c.blank();

    c.line("@MainActor");
    c.open("public func perform() async throws -> some IntentResult & ReturnsValue<String> {");
    if crate::targets::needs_confirmation(tool.info.risk) {
        let dialog = format!("确认{}？", tool.display_title());
        c.line(format!(
            "try await requestConfirmation(actionName: {}, dialog: {})",
            confirmation_action(tool.info.risk),
            string_literal(Lang::Swift, &dialog)
        ));
    }
    let mut args = Vec::new();
    for (((f, var), kind), prop) in params.fields.iter().zip(&vars).zip(&kinds).zip(&prop_names) {
        let v = ident::escape(Lang::Swift, var);
        let expr = match kind {
            ParamKind::Direct => v,
            ParamKind::Date { date_only } => {
                let fmt = if *date_only {
                    "$0.formatted(Date.ISO8601FormatStyle(timeZone: .current).year().month().day())"
                } else {
                    "$0.ISO8601Format()"
                };
                if f.optional() {
                    format!("{v}.map {{ {fmt} }}")
                } else {
                    fmt.replace("$0", &v)
                }
            }
            ParamKind::JsonString => json_decode_expr(model, m, f, &v),
        };
        // 调用处的参数标签除 inout / var / let 外不需要转义
        let label = if matches!(prop.as_str(), "inout" | "var" | "let") {
            format!("`{prop}`")
        } else {
            prop.clone()
        };
        args.push(format!("{label}: {expr}"));
    }
    c.line(format!("let params = {}({})", params.name, args.join(", ")));
    c.line(format!(
        "let result = try await {m}IntentRuntime.requireHandlers().{}(params)",
        ident::escape(Lang::Swift, &tool.camel)
    ));
    c.line(format!(
        "return .result(value: try {m}IntentRuntime.text(result))"
    ));
    c.close("}");
    c.close("}");
}
