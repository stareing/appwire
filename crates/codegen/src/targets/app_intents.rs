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
//!
//! 可选输出（[`AppIntentsOptions`]，缺省全部关闭，关闭时输出与上面完全相同）：
//! - `extension`：共享 Swift 包 `<Module>Intents`（`Package.swift` + 两个源文件 + `AppIntentsPackage`）、
//!   App Intents 扩展入口（`AppIntentsExtension`，App 未运行时由扩展进程执行 intent）与 App 侧的包声明；
//!   handler 改为由开发者实现一次的 `<Module>IntentHandlersProviding`，App 与扩展的 `init` 各调用一次
//!   `<Module>IntentRuntime.configure(_:)`，首次执行 intent 时才构造（惰性）。
//! - `execution_targets`：`allowedExecutionTargets`（iOS / macOS 27），`foreground` → `.main`，
//!   `background` / `headless` → `[.main, .appIntentsExtension]`，以 `@available` 限定。
//! - `cancellable`：`CancellableIntent` + `withIntentCancellationHandler(operation:onCancel:isolation:)`
//!   （iOS / macOS 26.4），以 `#available` 限定，取消原因经 `<Module>IntentRuntime.onCancel` 通知 App。
//!
//! 可选输出的依据（developer.apple.com 文档 JSON，2026-10-02 抓取）：`AppIntentsExtension: AppExtension`（iOS 16）、
//! `AppIntentsPackage.includedPackages`（iOS 17）、`IntentExecutionTargets`（iOS 27）、`CancellableIntent` /
//! `IntentCancellationReason.timeout / .userCancelled`（iOS 26.4）。没有带 iOS SDK 的 Xcode，未在真实框架上编译，
//! 只对照 `scripts/stubs/AppIntents.swift` 做桩类型检查。

use app_mcp_protocol::{Activation, Risk};

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{self, Lang, NameScope};
use crate::schema::{Field, Model, ToolModel, Ty, TypeDecl, Warning, field_notes};
use crate::targets::{file, swift};

/// App Shortcuts 的数量上限（系统限制每个 App 最多 10 个）。
pub const MAX_APP_SHORTCUTS: usize = 10;

/// `swift-app-intents` 的可选输出；缺省全部关闭。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppIntentsOptions {
    /// 共享 Swift 包 + App Intents 扩展布局（App 不运行时也能由扩展执行）。
    pub extension: bool,
    /// 按 activation 声明 `allowedExecutionTargets`（iOS / macOS 27）。
    pub execution_targets: bool,
    /// intent 遵循 `CancellableIntent`（iOS / macOS 26.4）。
    pub cancellable: bool,
}

/// `CancellableIntent` 与 `IntentCancellationReason` 的最低系统版本。
const CANCELLABLE_AVAILABILITY: &str = "iOS 26.4, macOS 26.4, *";
/// `allowedExecutionTargets` / `IntentExecutionTargets` 的最低系统版本。
const EXECUTION_TARGETS_AVAILABILITY: &str = "iOS 27.0, macOS 27.0, *";

pub fn generate(
    model: &Model,
    options: &AppIntentsOptions,
    warnings: &mut Vec<Warning>,
) -> Vec<GeneratedFile> {
    let m = &model.module;
    let tools = swift::generate(model);
    let intents = intents_file(model, options, warnings);
    if !options.extension {
        return vec![tools, file(format!("{m}AppIntents.swift"), intents)];
    }
    if !options.execution_targets {
        warn_foreground_in_extension(model, warnings);
    }
    let package = shared_package_name(model);
    let sources = format!("{package}/Sources/{package}");
    vec![
        file(format!("{package}/Package.swift"), package_manifest(model)),
        file(
            format!("{sources}/{}", tools.path.display()),
            tools.contents,
        ),
        file(format!("{sources}/{m}AppIntents.swift"), intents),
        file(
            format!("{m}IntentsExtension/{m}IntentsExtension.swift"),
            extension_entry(model),
        ),
        file(format!("App/{m}AppIntentsPackage.swift"), app_package(model)),
    ]
}

/// 共享 Swift 包（及其库 / 模块）名：`<Module>Intents`。
fn shared_package_name(model: &Model) -> String {
    format!("{}Intents", model.module)
}

/// 开发者实现 handler 来源时使用的类型名（生成的扩展入口按此名引用）。
fn provider_type(model: &Model) -> String {
    format!("{}IntentHandlersProvider", model.module)
}

/// iOS 27 前无法限定执行进程：`foreground` 工具在扩展布局下也可能落到扩展进程（无界面）。
fn warn_foreground_in_extension(model: &Model, warnings: &mut Vec<Warning>) {
    let foreground = model
        .tools
        .iter()
        .filter(|t| t.info.activation.unwrap_or_default() == Activation::Foreground)
        .count();
    if foreground == 0 {
        return;
    }
    warnings.push(Warning {
        tool: String::new(),
        path: String::new(),
        message: format!(
            "swift-app-intents：扩展布局中有 {foreground} 个 activation 为 foreground 的工具，未声明 allowedExecutionTargets 时系统可能在扩展进程（无界面）执行；需要界面的请加 --app-intents-execution-targets（iOS 27 起生效）"
        ),
    });
}

/// 共享包的 `Package.swift`（`swift-tools-version` 必须在第一行）。
fn package_manifest(model: &Model) -> String {
    let package = shared_package_name(model);
    let mut c = Code::new("    ");
    c.line("// swift-tools-version: 6.0");
    let mut header = header_lines(model, "swift-app-intents");
    header.push(String::new());
    header.push("App 与 App Intents 扩展共用的 Swift 包：参数类型、handler 协议、各 AppIntent。".to_string());
    header.push(format!(
        "在 Xcode 中以本地包加入工程，把 {package} 库同时链接到 App target 与 App Intents 扩展 target。"
    ));
    c.comment("// ", &header);
    c.blank();
    c.line("import PackageDescription");
    c.blank();
    c.open("let package = Package(");
    c.line(format!("name: {},", string_literal(Lang::Swift, &package)));
    c.line("platforms: [.iOS(\"26.0\"), .macOS(\"26.0\")],");
    c.open("products: [");
    c.line(format!(
        ".library(name: {0}, targets: [{0}]),",
        string_literal(Lang::Swift, &package)
    ));
    c.close("],");
    c.open("targets: [");
    c.line(format!(
        ".target(name: {}),",
        string_literal(Lang::Swift, &package)
    ));
    c.close("]");
    c.close(")");
    c.finish()
}

/// 声明一个包含共享包 intent 的 `AppIntentsPackage`。
fn include_package(c: &mut Code, model: &Model, name: &str, doc: &str) {
    c.line(format!("/// {doc}"));
    c.open(format!("struct {name}: AppIntentsPackage {{"));
    c.open("static var includedPackages: [any AppIntentsPackage.Type] {");
    c.line(format!("[{}Package.self]", shared_package_name(model)));
    c.close("}");
    c.close("}");
}

/// App Intents 扩展入口：扩展进程启动时登记 handler 来源。
fn extension_entry(model: &Model) -> String {
    let m = &model.module;
    let package = shared_package_name(model);
    let provider = provider_type(model);
    let mut c = Code::new("    ");
    let mut header = header_lines(model, "swift-app-intents");
    header.push(String::new());
    header.push(format!(
        "App Intents 扩展的入口：App 未运行时，系统在扩展进程中执行 {package} 包里的 intent。"
    ));
    header.push(format!(
        "需要开发者提供 `{provider}`（实现 `{m}IntentHandlersProviding`），并对本扩展 target 可见。"
    ));
    c.comment("// ", &header);
    c.blank();
    c.line("import AppIntents");
    c.line(format!("import {package}"));
    c.blank();
    c.line("@main");
    c.open(format!("struct {m}IntentsExtension: AppIntentsExtension {{"));
    c.open("init() {");
    c.line(format!("{m}IntentRuntime.configure({provider}.self)"));
    c.close("}");
    c.close("}");
    c.blank();
    include_package(
        &mut c,
        model,
        &format!("{m}IntentsExtensionPackage"),
        &format!("让扩展包含 {package} 包中的 intent。"),
    );
    c.finish()
}

/// App 侧的包声明；handler 来源由 App 的 `init` 登记（与扩展入口同一行代码）。
fn app_package(model: &Model) -> String {
    let m = &model.module;
    let package = shared_package_name(model);
    let provider = provider_type(model);
    let mut c = Code::new("    ");
    let mut header = header_lines(model, "swift-app-intents");
    header.push(String::new());
    header.push("加入 App target。另需在 App 的 init 中调用（与扩展入口相同）：".to_string());
    header.push(format!("    {m}IntentRuntime.configure({provider}.self)"));
    c.comment("// ", &header);
    c.blank();
    c.line("import AppIntents");
    c.line(format!("import {package}"));
    c.blank();
    include_package(
        &mut c,
        model,
        &format!("{m}AppIntentsPackage"),
        &format!("让 App 包含 {package} 包中的 intent。"),
    );
    c.finish()
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

fn intents_file(model: &Model, options: &AppIntentsOptions, warnings: &mut Vec<Warning>) -> String {
    let m = &model.module;
    let mut c = Code::new("    ");
    let mut header = header_lines(model, "swift-app-intents");
    header.push(String::new());
    header.push(format!(
        "依赖同目录下的 {}Tools.swift（参数类型与 {m}ToolHandlers 协议）。",
        m
    ));
    header.push("需要 iOS / macOS 26 起的 App Intents（supportedModes）；".to_string());
    header.extend(setup_lines(model, options));
    if let Some(body) = model.overview.as_ref().and_then(|o| o.body.as_deref()) {
        header.push(String::new());
        header.extend(body.lines().map(str::to_string));
    }
    c.comment("// ", &header);
    c.blank();
    c.line("import AppIntents");
    c.line("import Foundation");
    if options.extension || options.cancellable {
        c.line("import Synchronization");
    }
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

    if options.extension {
        emit_provider_protocol(&mut c, model);
    }
    if options.cancellable {
        emit_cancellation_reason(&mut c, model);
    }

    // 运行时：handler 注入与 JSON 辅助
    c.line(format!("/// App Intents 调用 {m}ToolHandlers 的入口。"));
    c.open(format!("public enum {m}IntentRuntime {{"));
    if options.extension {
        emit_configure(&mut c, model);
    } else {
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
    }
    c.blank();
    if options.cancellable {
        emit_cancel_hook(&mut c, model);
        c.blank();
    }
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
        emit_intent(&mut c, model, options, tool, &intent);
        c.blank();
        if options.cancellable {
            c.line(format!("@available({CANCELLABLE_AVAILABILITY})"));
            c.line(format!("extension {intent}: CancellableIntent {{}}"));
            c.blank();
        }
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

    if options.extension {
        if !shortcuts.is_empty() {
            c.blank();
        }
        c.line(format!(
            "/// 共享包的 intent 声明；App 与扩展各自以 `includedPackages` 包含它（见 App/{m}AppIntentsPackage.swift 与扩展入口）。"
        ));
        c.line(format!(
            "public struct {}Package: AppIntentsPackage {{}}",
            shared_package_name(model)
        ));
    }

    c.finish()
}

/// 文件头中"如何接入 handler"的说明行。
fn setup_lines(model: &Model, options: &AppIntentsOptions) -> Vec<String> {
    let m = &model.module;
    let mut lines = Vec::new();
    if options.extension {
        lines.push(format!(
            "位于 App 与 App Intents 扩展共用的 Swift 包 {}；App 的 init 与扩展的 init 各调用一次",
            shared_package_name(model)
        ));
        lines.push(format!(
            "`{m}IntentRuntime.configure({}.self)`（扩展入口已生成），`{}` 由开发者实现一次（`{m}IntentHandlersProviding`），",
            provider_type(model),
            provider_type(model)
        ));
        lines.push("放在 App 与扩展都能访问的模块中；handler 在所在进程首次执行 intent 时才构造。".to_string());
    } else {
        lines.push(format!(
            "启动时设置 `{m}IntentRuntime.handlers = <实现 {m}ToolHandlers 的对象>`（可直接复用 MCP 的业务实现）。"
        ));
    }
    if options.execution_targets {
        lines.push(
            "allowedExecutionTargets 需 iOS / macOS 27（以 @available 限定，低版本不声明，由系统决定执行进程）。".to_string(),
        );
    }
    if options.cancellable {
        lines.push(format!(
            "CancellableIntent 需 iOS / macOS 26.4（以 #available 限定）；取消原因经 `{m}IntentRuntime.onCancel` 通知 App。"
        ));
    }
    if *options != AppIntentsOptions::default() {
        lines.push("以上可选输出未在真实 AppIntents SDK 上编译验证，首次接入请在 Xcode 中确认。".to_string());
    }
    lines
}

/// 扩展布局：开发者实现一次的 handler 来源协议。
fn emit_provider_protocol(c: &mut Code, model: &Model) {
    let m = &model.module;
    c.line(format!(
        "/// App 与扩展共用的 handler 来源。由开发者实现一次（类型名 `{}`，生成的扩展入口按此名引用），",
        provider_type(model)
    ));
    c.line("/// 放在 App target 与扩展 target 都能访问的模块中。`Sendable` 使其元类型可存入 `Mutex`（Swift 6.2 `SendableMetatype`）。");
    c.open(format!("public protocol {m}IntentHandlersProviding: Sendable {{"));
    c.line("/// 构造 handler；在所在进程（App 或扩展）首次执行 intent 时调用一次。");
    c.line(format!("static func makeHandlers() -> any {m}ToolHandlers"));
    c.close("}");
    c.blank();
}

/// 扩展布局：`configure` 与惰性构造 handler（不假定扩展 `init` 的执行者，用 `Mutex` 保护）。
fn emit_configure(c: &mut Code, model: &Model) {
    let m = &model.module;
    c.open("private struct State {");
    c.line(format!("var provider: (any {m}IntentHandlersProviding.Type)?"));
    c.line(format!("var handlers: (any {m}ToolHandlers)?"));
    c.close("}");
    c.blank();
    c.line("private static let state = Mutex(State())");
    c.blank();
    c.line("/// 在 App 的 `init` 与扩展的 `init` 中各调用一次；只记录来源，首次执行 intent 时才构造 handler。");
    c.open(format!(
        "public static func configure(_ provider: any {m}IntentHandlersProviding.Type) {{"
    ));
    c.line("state.withLock { $0.provider = provider }");
    c.close("}");
    c.blank();
    c.line(format!(
        "/// 未调用 `configure` 时抛出 `{m}ToolError.handlersNotSet`。"
    ));
    c.open(format!(
        "static func requireHandlers() throws -> any {m}ToolHandlers {{"
    ));
    c.open("try state.withLock { state in");
    c.line("if let handlers = state.handlers { return handlers }");
    c.line(format!(
        "guard let provider = state.provider else {{ throw {m}ToolError.handlersNotSet }}"
    ));
    c.line("let handlers = provider.makeHandlers()");
    c.line("state.handlers = handlers");
    c.line("return handlers");
    c.close("}");
    c.close("}");
}

/// `CancellableIntent`：与系统类型解耦的取消原因。
fn emit_cancellation_reason(c: &mut Code, model: &Model) {
    let m = &model.module;
    c.line("/// intent 被取消的原因（`CancellableIntent`，iOS / macOS 26.4 起）。");
    c.open(format!("public enum {m}IntentCancellationReason: Sendable {{"));
    c.line("/// 未报告进度且超过系统运行时限。");
    c.line("case timeout");
    c.line("/// 用户在 Siri、实时活动或快捷指令中取消。");
    c.line("case userCancelled");
    c.line("/// 其他原因（新系统版本可能新增）。");
    c.line("case other");
    c.close("}");
    c.blank();
}

/// `CancellableIntent`：取消通知的登记与转发（运行时成员）。
fn emit_cancel_hook(c: &mut Code, model: &Model) {
    let m = &model.module;
    let hook = format!("@Sendable (_ tool: String, _ reason: {m}IntentCancellationReason) -> Void");
    c.line(format!(
        "private static let cancelHook = Mutex<({hook})?>(nil)"
    ));
    c.blank();
    c.line("/// 设置取消通知（可选），参数为工具名与原因；在系统给出的收尾时间内调用，应尽快返回。handler 所在 Task 同时被取消。");
    c.open(format!("public static func onCancel(_ hook: ({hook})?) {{"));
    c.line("cancelHook.withLock { $0 = hook }");
    c.close("}");
    c.blank();
    c.line(format!("@available({CANCELLABLE_AVAILABILITY})"));
    c.open("static func cancelled(tool: String, reason: IntentCancellationReason) {");
    c.line(format!("let mapped: {m}IntentCancellationReason"));
    c.line("switch reason {");
    c.line("case .timeout: mapped = .timeout");
    c.line("case .userCancelled: mapped = .userCancelled");
    c.line("default: mapped = .other");
    c.line("}");
    c.line("let hook = cancelHook.withLock { $0 }");
    c.line("hook?(tool, mapped)");
    c.close("}");
}

fn emit_intent(c: &mut Code, model: &Model, options: &AppIntentsOptions, tool: &ToolModel, intent: &str) {
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
        "allowedExecutionTargets",
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
    let activation = tool.info.activation.unwrap_or_default();
    let modes = match activation {
        Activation::Foreground => ".foreground(.immediate)",
        Activation::Background | Activation::Headless => ".background",
    };
    c.line(format!(
        "public static let supportedModes: IntentModes = {modes}"
    ));
    if options.execution_targets {
        c.line(format!("@available({EXECUTION_TARGETS_AVAILABILITY})"));
        c.line(format!(
            "public static var allowedExecutionTargets: IntentExecutionTargets {{ {} }}",
            execution_targets(activation)
        ));
    }
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
    let call = format!(
        "try await {m}IntentRuntime.requireHandlers().{}(params)",
        ident::escape(Lang::Swift, &tool.camel)
    );
    if options.cancellable {
        c.line("let result: any Encodable & Sendable");
        c.open(format!("if #available({CANCELLABLE_AVAILABILITY}) {{"));
        c.open("result = try await withIntentCancellationHandler {");
        c.line(&call);
        c.dedent();
        c.line("} onCancel: { reason in");
        c.indent();
        c.line(format!(
            "{m}IntentRuntime.cancelled(tool: {}, reason: reason)",
            string_literal(Lang::Swift, &tool.info.name)
        ));
        c.close("}");
        c.dedent();
        c.line("} else {");
        c.indent();
        c.line(format!("result = {call}"));
        c.close("}");
    } else {
        c.line(format!("let result = {call}"));
    }
    c.line(format!(
        "return .result(value: try {m}IntentRuntime.text(result))"
    ));
    c.close("}");
    c.close("}");
}

/// `allowedExecutionTargets`：需要界面的工具只在 App 进程执行，其余 App 与扩展均可。
fn execution_targets(activation: Activation) -> &'static str {
    match activation {
        Activation::Foreground => ".main",
        Activation::Background | Activation::Headless => "[.main, .appIntentsExtension]",
    }
}
