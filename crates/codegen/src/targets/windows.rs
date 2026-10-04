//! Windows：App Actions（Action 定义 JSON + C# `IActionProvider` 骨架）与 Windows 原生 MCP
//! agent connector 注册清单（On-Device Registry，MCPB `manifest.json`）。
//!
//! 依据（2026-09）：
//! - App Actions：<https://learn.microsoft.com/windows/ai/app-actions/>、
//!   <https://learn.microsoft.com/windows/ai/app-actions/actions-json>、
//!   <https://learn.microsoft.com/windows/ai/app-actions/actions-iactionprovider-manual>、
//!   <https://learn.microsoft.com/windows/ai/app-actions/actions-provider-manifest>；
//! - MCP on Windows：<https://learn.microsoft.com/windows/ai/mcp/overview>、
//!   <https://learn.microsoft.com/windows/ai/mcp/servers/mcp-windows-identity>。
//!
//! 官方文档没有表示 App Actions 已被 agent connector 取代，两者并列；App Actions 的输入只能是实体
//! （Text、File、Photo 等），因此只有全部参数为标量的工具生成 Action（以 Text 实体传入），其余工具
//! 给出警告，可通过 agent connector（MCP）调用。
//!
//! 弃用（spec/protocol.md 3.7）：弃用的工具照常生成 Action（App 仍需响应系统入口）；Action 定义 JSON 不加弃用字段（仓库内无该格式的
//! 弃用字段资料，未核实），只在提供者分派处注释，参数级 `deprecated: true` 随 agent connector 的 inputSchema 原样输出。

use serde_json::{Value, json};

use crate::GeneratedFile;
use crate::code::{Code, header_lines, string_literal, xml_escape};
use crate::deprecation;
use crate::ident::{self, Lang};
use crate::schema::{Model, ToolModel, Ty, Warning};
use crate::targets::{csharp, file, needs_confirmation, risk_name};

/// Action 定义 JSON 的 schema 版本。
pub const ACTIONS_SCHEMA_VERSION: u64 = 3;
/// agent connector 静态响应中声明的 MCP 协议版本。
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

pub fn generate(model: &Model, namespace: &str, warnings: &mut Vec<Warning>) -> Vec<GeneratedFile> {
    let m = &model.module;
    let clsid = clsid_for(&model.app_id);
    let supported: Vec<&ToolModel> = model
        .tools
        .iter()
        .filter(|tool| {
            let ok = model
                .params(tool)
                .fields
                .iter()
                .all(|f| f.ty.is_scalar() && f.degraded.is_none());
            if !ok {
                warnings.push(Warning {
                    tool: tool.info.name.clone(),
                    path: String::new(),
                    message: "windows-app-actions：App Actions 的输入只能是实体（Text 等），含对象 / 数组 / 原始 JSON 参数的工具未生成 Action，可通过 agent connector（MCP）调用".to_string(),
                });
            }
            ok
        })
        .collect();

    let actions = actions_json(model, &supported, &clsid);
    let connector = connector_manifest(model);
    vec![
        csharp::generate(model, namespace),
        file(
            format!("{m}ActionProvider.cs"),
            provider_cs(model, namespace, &supported, &clsid),
        ),
        file(format!("Assets/{m}Actions.json"), pretty(&actions)),
        file("Assets/McpServer/manifest.json", pretty(&connector)),
        file(
            "Package.appxmanifest.snippet.xml",
            manifest_snippet(model, &clsid),
        ),
    ]
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    s.push('\n');
    s
}

/// Action ID：`<Module>.<ToolPascal>`。
pub fn action_id(model: &Model, tool: &ToolModel) -> String {
    format!("{}.{}", model.module, tool.pascal)
}

/// 由 appId 派生的确定性 CLSID（FNV-1a 128 位折叠），重新生成时保持不变。
pub fn clsid_for(app_id: &str) -> String {
    let hash = |seed: u64| {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
        for b in app_id.bytes().chain(*b"app-mcp-action-provider") {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let hi = hash(0x5a5a_5a5a);
    let lo = hash(0xa5a5_a5a5_0000_0001);
    // 按 UUID v4 格式设置 version / variant 位
    let hi = (hi & 0xffff_ffff_ffff_0fff) | 0x0000_0000_0000_4000;
    let lo = (lo & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        hi >> 32,
        (hi >> 16) & 0xffff,
        hi & 0xffff,
        lo >> 48,
        lo & 0x0000_ffff_ffff_ffff
    )
}

fn input_name(json_name: &str) -> String {
    ident::camel(json_name, "input")
}

fn actions_json(model: &Model, tools: &[&ToolModel], clsid: &str) -> Value {
    let actions: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let params = model.params(tool);
            let inputs: Vec<Value> = params
                .fields
                .iter()
                .map(|f| {
                    json!({
                        "name": input_name(&f.json_name),
                        "kind": "Text",
                        "required": f.required,
                    })
                })
                .collect();
            let required: Vec<String> = params
                .fields
                .iter()
                .filter(|f| f.required)
                .map(|f| input_name(&f.json_name))
                .collect();
            let all: Vec<String> = params.fields.iter().map(|f| input_name(&f.json_name)).collect();
            let describe = |names: &[String]| {
                let slots: Vec<String> = names.iter().map(|n| format!("${{{n}.Text}}")).collect();
                if slots.is_empty() {
                    tool.display_title().to_string()
                } else {
                    format!("{}：{}", tool.display_title(), slots.join("，"))
                }
            };
            let mut combinations = vec![json!({ "inputs": required, "description": describe(&required) })];
            if all.len() > required.len() {
                combinations.push(json!({ "inputs": all, "description": describe(&all) }));
            }
            json!({
                "id": action_id(model, tool),
                "description": tool.info.description,
                "icon": "ms-resource://Files/Assets/StoreLogo.png",
                "usesGenerativeAI": false,
                "isAvailable": true,
                "displaysUI": matches!(tool.info.activation, Some(app_mcp_protocol::Activation::Foreground)),
                "allowedAppInvokers": ["*"],
                "inputs": inputs,
                "inputCombinations": combinations,
                "outputs": [{ "name": "response", "kind": "Text" }],
                // 文档表格写 "com"，同页示例写 "COM" 且 clsid 带花括号；按示例生成
                "invocation": { "type": "COM", "clsid": format!("{{{clsid}}}") },
            })
        })
        .collect();
    json!({ "version": ACTIONS_SCHEMA_VERSION, "actions": actions })
}

fn connector_manifest(model: &Model) -> Value {
    let full_name = |tool: &ToolModel| format!("{}.{}", model.app_id, tool.info.name);
    let tools_brief: Vec<Value> = model
        .tools
        .iter()
        .map(|t| json!({ "name": full_name(t), "description": t.info.description }))
        .collect();
    let tools_full: Vec<Value> = model
        .tools
        .iter()
        .map(|t| {
            let mut v = json!({
                "name": full_name(t),
                "description": t.info.description,
                "inputSchema": t.info.input_schema,
            });
            if let Some(title) = &t.info.title {
                v["title"] = json!(title);
            }
            v
        })
        .collect();
    let description = model
        .overview
        .as_ref()
        .map(|o| o.summary.clone())
        .or_else(|| model.app_description.clone())
        .unwrap_or_else(|| model.app_name.clone());
    json!({
        "manifest_version": "0.1",
        "name": model.app_id,
        "display_name": model.app_name,
        "version": "0.1.0",
        "description": description,
        "author": { "name": model.app_name },
        "server": {
            "type": "binary",
            "entry_point": "app-mcp-host.exe",
            "mcp_config": {
                "command": "app-mcp-host.exe",
                "args": ["--manifest", "app-mcp.json"],
            },
        },
        "tools": tools_brief,
        "tools_generated": false,
        "_meta": {
            "com.microsoft.windows": {
                "static_responses": {
                    "initialize": {
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": { "tools": { "listChanged": true } },
                        "serverInfo": { "name": "app-mcp-host", "version": env!("CARGO_PKG_VERSION") },
                    },
                    "tools/list": { "tools": tools_full },
                },
            },
        },
    })
}

fn manifest_snippet(model: &Model, clsid: &str) -> String {
    let m = &model.module;
    let name = xml_escape(&model.app_name);
    let mut c = Code::new("  ");
    c.line("<!--");
    for l in header_lines(model, "windows-app-actions") {
        c.line(format!("  {}", xml_escape(&l).replace("--", "- -")));
    }
    c.line("  把以下扩展合并进 Package.appxmanifest 的 <Application><Extensions>。");
    c.line("  App Actions 需要包标识（MSIX）与 mediumIL；目标 SDK 10.0.26100.0+。");
    c.line("  MCP agent connector 需要 Windows 26220.7262+（预发布功能）。");
    c.line("-->");
    c.open("<Extensions>");
    c.line("<!-- App Actions：Action 定义 -->");
    c.open("<uap3:Extension Category=\"windows.appExtension\">");
    c.open(format!(
        "<uap3:AppExtension Name=\"com.microsoft.windows.ai.actions\" DisplayName=\"{name}\" Id=\"{m}Actions\" PublicFolder=\"Assets\">"
    ));
    c.line(format!(
        "<uap3:Properties><Registration>{m}Actions.json</Registration></uap3:Properties>"
    ));
    c.close("</uap3:AppExtension>");
    c.close("</uap3:Extension>");
    c.line("<!-- App Actions：COM 激活 IActionProvider -->");
    c.open("<com2:Extension Category=\"windows.comServer\">");
    c.open("<com2:ComServer>");
    c.open(format!(
        "<com3:ExeServer Executable=\"{m}.exe\" DisplayName=\"{name}\">"
    ));
    c.line(format!(
        "<com:Class Id=\"{clsid}\" DisplayName=\"{m}ActionProvider\" />"
    ));
    c.close("</com3:ExeServer>");
    c.close("</com2:ComServer>");
    c.close("</com2:Extension>");
    c.line("<!-- MCP agent connector（On-Device Registry） -->");
    c.open("<uap3:Extension Category=\"windows.appExtension\">");
    c.open(format!(
        "<uap3:AppExtension Name=\"com.microsoft.windows.ai.mcpServer\" DisplayName=\"{name}\" Id=\"{m}McpServer\" PublicFolder=\"Assets\\McpServer\">"
    ));
    c.line("<uap3:Properties><Registration>manifest.json</Registration></uap3:Properties>");
    c.close("</uap3:AppExtension>");
    c.close("</uap3:Extension>");
    c.close("</Extensions>");
    c.finish()
}

/// 把 Text 实体转换为字段类型的 C# 表达式。
fn parse_expr(model: &Model, ty: &Ty, required: bool, key: &str) -> String {
    let key = string_literal(Lang::CSharp, key);
    let get = if required {
        format!("Require(inputs, {key})")
    } else {
        format!("Optional(inputs, {key})")
    };
    let suffix = if required { "" } else { "OrNull" };
    match ty {
        Ty::String => get,
        Ty::Integer => format!("ParseLong{suffix}({get})"),
        Ty::Number => format!("ParseDouble{suffix}({get})"),
        Ty::Boolean => format!("ParseBool{suffix}({get})"),
        Ty::Enum(id) => format!("ParseEnum{suffix}<{}>({get})", model.decl(*id).name()),
        // 只对标量工具生成 Action
        _ => get,
    }
}

fn provider_cs(model: &Model, namespace: &str, tools: &[&ToolModel], clsid: &str) -> String {
    let m = &model.module;
    let mut c = Code::new("    ");
    c.line("// <auto-generated />");
    let mut header = header_lines(model, "windows-app-actions");
    header.extend([
        String::new(),
        "App Actions 处理骨架：把 Text 实体解析为参数类型并调用 I{Module}ToolHandlers。".replace("{Module}", m),
        "需要目标框架 net9.0-windows10.0.26100.0+ 与包标识；COM 注册见 Package.appxmanifest.snippet.xml。".to_string(),
        "启动时（Main）注册：".to_string(),
        "  Microsoft.AI.Actions.Helpers.ActionRuntimeFactory.CreateActionRuntime();".to_string(),
        format!("  Microsoft.AI.Actions.Helpers.ActionProviderFactory.RegisterActionProvider<{m}ActionProvider, Windows.AI.Actions.Provider.IActionProvider>(new {m}ActionProvider(handlers));"),
    ]);
    c.comment("// ", &header);
    c.line("#nullable enable");
    c.blank();
    for u in [
        "using System;",
        "using System.Collections.Generic;",
        "using System.Globalization;",
        "using System.Runtime.InteropServices;",
        "using System.Text.Json;",
        "using System.Threading;",
        "using System.Threading.Tasks;",
        "using Windows.AI.Actions;",
        "using Windows.AI.Actions.Provider;",
        "using Windows.Foundation;",
    ] {
        c.line(u);
    }
    c.blank();
    c.line(format!("namespace {namespace};"));
    c.blank();
    c.line(format!(
        "/// <summary>{} 的 App Actions 提供者。CLSID 与 {m}Actions.json 中的 invocation.clsid 一致。</summary>",
        xml_escape(&model.app_name)
    ));
    c.line("[ComVisible(true)]");
    c.line(format!("[Guid({})]", string_literal(Lang::CSharp, clsid)));
    c.line(format!(
        "public sealed partial class {m}ActionProvider : IActionProvider"
    ));
    c.open("{");
    c.line(format!("private readonly I{m}ToolHandlers _handlers;"));
    c.blank();
    c.line(format!(
        "public {m}ActionProvider(I{m}ToolHandlers handlers) => _handlers = handlers;"
    ));
    c.blank();
    c.line("public IAsyncAction InvokeAsync(ActionInvocationContext context) => InvokeCoreAsync(context).AsAsyncAction();");
    c.blank();
    c.line("private async Task InvokeCoreAsync(ActionInvocationContext context)");
    c.open("{");
    c.line("var inputs = new Dictionary<string, string>(StringComparer.Ordinal);");
    c.line("foreach (var named in context.GetInputEntities())");
    c.open("{");
    c.line("if (named.Entity is TextActionEntity text)");
    c.open("{");
    c.line("inputs[named.Name] = text.Text;");
    c.close("}");
    c.close("}");
    c.blank();
    // 弃用的工具照常响应系统入口；分派处调用已弃用的 handler / 设置已弃用的属性，局部关闭 CS0618
    let suppress = model.has_deprecations();
    if suppress {
        c.line(csharp::SUPPRESS_OBSOLETE);
    }
    c.line("object? result = context.ActionId switch");
    c.open("{");
    for tool in tools {
        let params = model.params(tool);
        let props = csharp::property_names(params);
        let inits: Vec<String> = params
            .fields
            .iter()
            .zip(&props)
            .map(|(f, p)| {
                format!(
                    "{p} = {}",
                    parse_expr(model, &f.ty, f.required, &input_name(&f.json_name))
                )
            })
            .collect();
        let init = if inits.is_empty() {
            "()".to_string()
        } else {
            format!(" {{ {} }}", inits.join(", "))
        };
        c.comment("// ", &deprecation::tool_doc_lines(tool));
        if needs_confirmation(tool.info.risk) {
            c.line(format!(
                "// 风险 {}：App Actions 没有系统级确认，handler 应在执行前向用户确认。",
                risk_name(tool.info.risk)
            ));
        }
        c.line(format!(
            "{} => await _handlers.{}(new {}{init}, CancellationToken.None),",
            string_literal(Lang::CSharp, &action_id(model, tool)),
            csharp::method_name(&tool.pascal),
            params.name
        ));
    }
    c.line("_ => throw new ArgumentException($\"未知 Action：{context.ActionId}\"),");
    c.close("};");
    if suppress {
        c.line(csharp::RESTORE_OBSOLETE);
    }
    c.blank();
    c.line(format!(
        "var output = result as string ?? JsonSerializer.Serialize(result, {m}Tools.JsonOptions);"
    ));
    c.line(
        "context.SetOutputEntity(\"response\", context.EntityFactory.CreateTextEntity(output));",
    );
    c.close("}");
    c.blank();
    for helper in PROVIDER_HELPERS.lines() {
        c.line(helper);
    }
    c.close("}");
    c.finish()
}

const PROVIDER_HELPERS: &str = r#"private static string Require(Dictionary<string, string> inputs, string name) =>
    inputs.TryGetValue(name, out var value) ? value : throw new ArgumentException($"缺少输入：{name}");

private static string? Optional(Dictionary<string, string> inputs, string name) =>
    inputs.TryGetValue(name, out var value) && value.Length > 0 ? value : null;

private static long ParseLong(string value) => long.Parse(value, CultureInfo.InvariantCulture);

private static long? ParseLongOrNull(string? value) => value is null ? null : ParseLong(value);

private static double ParseDouble(string value) => double.Parse(value, CultureInfo.InvariantCulture);

private static double? ParseDoubleOrNull(string? value) => value is null ? null : ParseDouble(value);

private static bool ParseBool(string value) => bool.Parse(value);

private static bool? ParseBoolOrNull(string? value) => value is null ? null : ParseBool(value);

private static T ParseEnum<T>(string value) where T : struct, Enum =>
    JsonSerializer.Deserialize<T>(JsonSerializer.Serialize(value));

private static T? ParseEnumOrNull<T>(string? value) where T : struct, Enum =>
    value is null ? null : ParseEnum<T>(value);"#;
