//! `--standard-intents`（spec/intents.md 第 3 节）：为声明了 `implements` 的工具额外生成 Apple App Intents 系统 schema 版本
//! （`@AppIntent(schema:)`），输出 `<Module>StandardIntents.swift`，`perform()` 复用 `<Module>ToolHandlers`（handler 只写一次）。
//!
//! 映射表依据（developer.apple.com 文档 JSON `https://developer.apple.com/tutorials/data/documentation/appintents/<页面>.json`，
//! 2026-10-04 抓取；没有带 iOS SDK 的 Xcode，未在真实框架上编译）：
//! - `link.open` → `.browser.openURLInTab`（`appschema/browserintent/openurlintab`：iOS 18 / macOS 15 / visionOS 2，无 tvOS /
//!   watchOS）：参数 `url: URL`、`tab: <TabEntity>`；`tab` 是 App 的实体（`appschema/browserentity/tab`，`@AppEntity(schema:
//!   .browser.tab)`），由 App 提供类型。browser 域只进快捷指令，不进 Siri / Apple Intelligence（`app-schema-domain-browser`）。
//!   显式 `: AppIntent` + `@Parameter` 的写法见 `assistantschemas/browserintent/openurlintab` 的示例。
//! - `message.send` → `.messages.sendMessage`、`calendar.create` → `.calendar.createEvent`、`media.play` → `.audio.playAudio`、
//!   `navigation.start` → `.maps.startNavigation`（均 iOS / macOS / visionOS 27）：参数是 App 自定义的实体 / 枚举类型
//!   （文档模板中的 `<#MessageDestination#>`、`<#CalendarEntity#>`、`<#AudioItem#>`、`<#MapsLocation#>` 等），平铺 JSON 参数
//!   表达不了，只给警告「需要实体解析，未生成」。messages 域另要求同时实现五个 schema（`app-schema-domain-messages`）。
//! - `file.share`：files 域（`appschema/filesintent`）只有 createFolder / deleteFiles / moveFiles / openFile / renameFile，无映射。
//!
//! 与可选输出的组合：扩展布局下文件放进共享包源码目录（与 `<Module>AppIntents.swift` 同处，App 实体也须放在该包内）；
//! `allowedExecutionTargets` / `CancellableIntent` / `supportedModes` 不加到系统 schema intent 上（schema 对这些成员的要求
//! 未经核实，按 P-08 保守处理）。

use app_mcp_protocol::intents::IntentDef;

use super::confirmation_action;
use crate::code::{Code, header_lines, string_literal};
use crate::ident::{Lang, NameScope};
use crate::schema::{Model, ToolModel, Ty, Warning};
use crate::standard_intents::{self, Binding};
use crate::targets::{needs_confirmation, swift};

/// 一个标准意图在 Apple 上对应的系统 schema。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AppleSchema {
    /// `@AppIntent(schema:)` 的参数，如 `.browser.openURLInTab`。
    pub expr: &'static str,
    /// 首个可用系统版本，用于警告与文档（`@available` 见 [`Support::Generate`]）。
    pub since: &'static str,
    pub support: Support,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Support {
    /// 可以生成：schema 的值参数逐个对应工具参数，实体参数的类型由 App 提供。
    Generate {
        /// 生成类型名的后缀（schema 动作名）。
        suffix: &'static str,
        /// `@available(...)` 的参数（含 `*`）。
        availability: &'static str,
        /// 文档平台列表中没有的平台，标为 `unavailable`。
        unavailable: &'static [&'static str],
        params: &'static [SchemaParam],
        entities: &'static [EntityParam],
    },
    /// 参数需要 App 实体 / 枚举解析，不生成；值为原因。
    NeedsEntities(&'static str),
}

/// schema 的值参数及其到工具参数的转换。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SchemaParam {
    pub name: &'static str,
    pub swift_type: &'static str,
    /// 对应的工具参数（词表必填参数名），工具中须为 string。
    pub tool_param: &'static str,
    /// 转为工具参数值的表达式，`$0` 为 schema 参数。
    pub convert: &'static str,
}

/// schema 要求的 App 实体参数：类型名为 `<Module><type_suffix>`，由 App 以 `@AppEntity(schema:)` 实现，不传给工具。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EntityParam {
    pub name: &'static str,
    pub type_suffix: &'static str,
    pub entity_schema: &'static str,
}

const IOS27: &str = "iOS / macOS / visionOS 27";

const OPEN_URL_IN_TAB: AppleSchema = AppleSchema {
    expr: ".browser.openURLInTab",
    since: "iOS 18 / macOS 15 / visionOS 2",
    support: Support::Generate {
        suffix: "OpenURLInTab",
        availability: "iOS 18.0, macOS 15.0, visionOS 2.0, *",
        unavailable: &["tvOS", "watchOS"],
        params: &[SchemaParam { name: "url", swift_type: "URL", tool_param: "url", convert: "$0.absoluteString" }],
        entities: &[EntityParam { name: "tab", type_suffix: "BrowserTab", entity_schema: ".browser.tab" }],
    },
};

const fn needs_entities(expr: &'static str, reason: &'static str) -> AppleSchema {
    AppleSchema { expr, since: IOS27, support: Support::NeedsEntities(reason) }
}

/// 映射表（一处定义）：词表动词版本 → Apple 系统 schema；`None` = Apple 没有对应 schema。
pub(super) fn apple_schema(def: &IntentDef) -> Option<AppleSchema> {
    match (def.verb, def.version) {
        ("link.open", 1) => Some(OPEN_URL_IN_TAB),
        ("message.send", 1) => Some(needs_entities(
            ".messages.sendMessage",
            "收件人 destination 是 App 自定义类型，且 messages 域要求同时实现 draftMessage / sendMessage / editSentMessage / unsendMessage / setMessageReadStatus",
        )),
        ("calendar.create", 1) => Some(needs_entities(
            ".calendar.createEvent",
            "要求 App 的日历实体 calendar、地点与参与者类型，并返回事件实体",
        )),
        ("media.play", 1) => Some(needs_entities(
            ".audio.playAudio",
            "要求 App 的音频实体 audioEntity（经 IntentValueQuery 从 MediaIntents 请求解析）",
        )),
        ("navigation.start", 1) => Some(needs_entities(
            ".maps.startNavigation",
            "要求 App 的地点类型 origin / destinations 与交通方式枚举，并返回导航会话实体",
        )),
        _ => None,
    }
}

/// 一个待生成的系统 schema intent。
struct Planned<'m> {
    binding: Binding<'m, AppleSchema>,
    /// 构造工具参数的实参列表（按工具参数声明顺序）。
    args: Vec<String>,
}

/// 生成 `<Module>StandardIntents.swift` 的内容；没有可生成的绑定时为 `None`（不输出文件）。
///
/// @input `extension` 只影响文件头的接入说明（文件位置由调用方决定）。
pub(super) fn generate<'m>(model: &'m Model, extension: bool, warnings: &mut Vec<Warning>) -> Option<String> {
    let resolve = |binding: Binding<'m, AppleSchema>, w: &mut Vec<Warning>| -> Option<Planned<'m>> {
        plan(model, binding)
            .map_err(|message| w.push(Warning { tool: binding.tool.info.name.clone(), path: String::new(), message }))
            .ok()
    };
    let planned = standard_intents::bindings(model, "Apple", apple_schema, resolve, warnings);
    if planned.is_empty() {
        return None;
    }
    Some(render(model, extension, &planned))
}

/// 检查绑定能否生成并算出工具参数；不能时返回警告文本。
fn plan<'m>(model: &'m Model, binding: Binding<'m, AppleSchema>) -> Result<Planned<'m>, String> {
    let id = binding.intent.id();
    let schema = binding.system;
    let (schema_params, entities) = match schema.support {
        Support::Generate { params, entities, .. } => (params, entities),
        Support::NeedsEntities(reason) => {
            return Err(format!(
                "`{id}` 对应的 Apple 系统 schema `{}`（{}）：{reason}；需要实体解析，未生成，只生成自定义意图",
                schema.expr, schema.since
            ));
        }
    };
    for entity in entities {
        let name = entity_type(model, entity);
        if model.types.iter().any(|t| t.name() == name) {
            return Err(format!("App 实体类型名 {name} 与生成的参数类型同名，未生成系统 schema `{}`", schema.expr));
        }
    }
    let params = model.params(binding.tool);
    let props = swift::property_names(params);
    let mut args = Vec::new();
    for (f, prop) in params.fields.iter().zip(&props) {
        let mapped = schema_params.iter().find(|p| p.tool_param == f.json_name);
        match mapped {
            Some(p) if f.ty != Ty::String => {
                return Err(format!("参数 {} 不是 string，无法由系统 schema `{}` 的 {} 提供，未生成", f.json_name, schema.expr, p.name));
            }
            Some(p) => args.push(format!("{}: {}", super::arg_label(prop), p.convert.replace("$0", p.name))),
            None if !f.optional() => {
                return Err(format!(
                    "必填参数 {} 不在系统 schema `{}` 中（系统无法提供），未生成，只生成自定义意图",
                    f.json_name, schema.expr
                ));
            }
            None => {}
        }
    }
    Ok(Planned { binding, args })
}

fn entity_type(model: &Model, entity: &EntityParam) -> String {
    format!("{}{}", model.module, entity.type_suffix)
}

fn render(model: &Model, extension: bool, planned: &[Planned<'_>]) -> String {
    let m = &model.module;
    let mut c = Code::new("    ");
    let mut header = header_lines(model, "swift-app-intents --standard-intents");
    header.push(String::new());
    header.push(format!(
        "Apple App Intents 系统 schema 版本（spec/intents.md 第 3 节）；依赖 {m}Tools.swift 与 {m}AppIntents.swift（{m}IntentRuntime）。"
    ));
    header.push("未在真实 AppIntents 框架上编译验证（本机无 iOS SDK），首次接入请在 Xcode 中确认。".to_string());
    let mut entities: Vec<(String, &EntityParam)> = Vec::new();
    for p in planned {
        if let Support::Generate { entities: list, .. } = p.binding.system.support {
            for e in list {
                let name = entity_type(model, e);
                if !entities.iter().any(|(n, _)| *n == name) {
                    entities.push((name, e));
                }
            }
        }
    }
    let place = if extension { "放在共享包 Sources 目录中（与本文件同一模块）" } else { "与本文件同一模块" };
    header.push(format!("需要 App 提供以下实体类型（public，{place}；属性与 defaultQuery 见 Apple 文档对应的实体 schema）："));
    for (name, e) in &entities {
        header.push(format!("  {name}：@AppEntity(schema: {})", e.entity_schema));
    }
    c.comment("// ", &header);
    c.blank();
    c.line("import AppIntents");
    c.line("import Foundation");
    // 自定义 intent 名（`<Pascal>Intent`）与参数类型名先占用，系统 schema intent 名与之冲突时加后缀
    let mut reserved: Vec<String> = model.types.iter().map(|t| t.name().to_string()).collect();
    reserved.extend(model.tools.iter().map(|t| format!("{}Intent", t.pascal)));
    let mut names = NameScope::with_reserved(&reserved.iter().map(String::as_str).collect::<Vec<_>>());
    for p in planned {
        c.blank();
        emit_intent(&mut c, model, p, &mut names);
    }
    c.finish()
}

fn emit_intent(c: &mut Code, model: &Model, planned: &Planned<'_>, names: &mut NameScope) {
    let Binding { tool, intent, system } = planned.binding;
    let Support::Generate { suffix, availability, unavailable, params, entities } = system.support else { return };
    let name = names.claim(&format!("{}{suffix}SchemaIntent", tool.pascal));
    let entity_names: Vec<String> = entities.iter().map(|e| format!("`{}`（{}）", e.name, entity_type(model, e))).collect();
    c.line(format!(
        "/// 系统 schema `{}`（标准意图 `{}`），调用工具 {}。实体参数 {}由 App 提供，不传给工具。",
        system.expr,
        intent.id(),
        tool.info.name,
        entity_names.join("、")
    ));
    c.line(format!("@available({availability})"));
    for os in unavailable {
        c.line(format!("@available({os}, unavailable)"));
    }
    c.line(format!("@AppIntent(schema: {})", system.expr));
    c.open(format!("public struct {name}: AppIntent {{"));
    for p in params {
        c.line("@Parameter");
        c.line(format!("public var {}: {}", p.name, p.swift_type));
        c.blank();
    }
    for e in entities {
        c.line("@Parameter");
        c.line(format!("public var {}: {}", e.name, entity_type(model, e)));
        c.blank();
    }
    c.line("public init() {}");
    c.blank();
    emit_perform(c, model, tool, &planned.args);
    c.close("}");
}

fn emit_perform(c: &mut Code, model: &Model, tool: &ToolModel, args: &[String]) {
    let m = &model.module;
    c.line("@MainActor");
    c.open("public func perform() async throws -> some IntentResult {");
    if needs_confirmation(tool.info.risk) {
        let dialog = format!("确认{}？", tool.display_title());
        c.line(format!(
            "try await requestConfirmation(actionName: {}, dialog: {})",
            confirmation_action(tool.info.risk),
            string_literal(Lang::Swift, &dialog)
        ));
    }
    c.line(format!("let params = {}({})", model.params(tool).name, args.join(", ")));
    c.comment("// ", &crate::deprecation::tool_doc_lines(tool));
    let call = crate::targets::swift::handler_call(model, tool, &format!("{m}IntentRuntime.requireHandlers()"), "params");
    c.line(format!("_ = try await {call}"));
    c.line("return .result()");
    c.close("}");
}

#[cfg(test)]
mod tests;
